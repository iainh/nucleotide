// ABOUTME: Workspace backend abstractions for local and remote project operations
// ABOUTME: Keeps editor-facing workspace services independent of transport details

use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};
use thiserror::Error;

const DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("file was modified externally: {path}")]
    Modified { path: PathBuf },

    #[error("path is not a file: {path}")]
    NotFile { path: PathBuf },

    #[error("search pattern is invalid: {0}")]
    InvalidSearchPattern(#[from] regex::Error),

    #[error("{operation} failed for {path}: {message}")]
    CommandFailed {
        operation: &'static str,
        path: PathBuf,
        message: String,
    },

    #[error("remote {operation} failed for {path}: {message}")]
    Remote {
        operation: &'static str,
        path: PathBuf,
        message: String,
        diagnostic: Option<String>,
    },
}

pub type Result<T> = std::result::Result<T, WorkspaceError>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkspaceIdentity {
    Local,
    Remote(RemoteWorkspaceIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteWorkspaceIdentity {
    pub kind: RemoteWorkspaceKind,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RemoteWorkspaceKind {
    Wsl,
    Ssh,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkspaceLocation {
    Local {
        path: PathBuf,
    },
    Wsl {
        original_path: PathBuf,
        distro: String,
        linux_path: PathBuf,
    },
    Ssh {
        original_path: PathBuf,
        target: SshWorkspaceTarget,
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshWorkspaceTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

impl WorkspaceLocation {
    pub fn display_root(&self) -> &Path {
        match self {
            WorkspaceLocation::Local { path }
            | WorkspaceLocation::Wsl {
                original_path: path,
                ..
            }
            | WorkspaceLocation::Ssh {
                original_path: path,
                ..
            } => path,
        }
    }

    pub fn native_root(&self) -> &Path {
        match self {
            WorkspaceLocation::Local { path }
            | WorkspaceLocation::Wsl {
                linux_path: path, ..
            }
            | WorkspaceLocation::Ssh { path, .. } => path,
        }
    }

    pub fn is_remote(&self) -> bool {
        !matches!(self, WorkspaceLocation::Local { .. })
    }

    pub fn path_mapping(&self) -> WorkspacePathMapping {
        WorkspacePathMapping::new(self.display_root(), self.native_root())
    }

    pub fn display_path_for_native_path(&self, native_path: &Path) -> PathBuf {
        match self {
            WorkspaceLocation::Local { .. } => native_path.to_path_buf(),
            WorkspaceLocation::Wsl {
                original_path,
                distro,
                linux_path,
            } => {
                if let Ok(relative_path) = native_path.strip_prefix(linux_path) {
                    return join_display_root(original_path, relative_path);
                }
                wsl_display_path_for_native_path(original_path, distro, native_path)
            }
            WorkspaceLocation::Ssh {
                original_path,
                target,
                path,
            } => {
                if let Ok(relative_path) = native_path.strip_prefix(path) {
                    return join_display_root(original_path, relative_path);
                }
                ssh_display_path_for_native_path(target, native_path)
            }
        }
    }

    pub fn matches_remote_identity(&self, other: &WorkspaceLocation) -> bool {
        match (self, other) {
            (
                WorkspaceLocation::Wsl { distro, .. },
                WorkspaceLocation::Wsl {
                    distro: other_distro,
                    ..
                },
            ) => distro == other_distro,
            (
                WorkspaceLocation::Ssh { target, .. },
                WorkspaceLocation::Ssh {
                    target: other_target,
                    ..
                },
            ) => target == other_target,
            _ => false,
        }
    }
}

pub fn classify_workspace_location(path: impl AsRef<Path>) -> WorkspaceLocation {
    let path = path.as_ref();
    let text = path.to_string_lossy();

    if let Some((distro, linux_path)) = parse_wsl_unc_path(&text) {
        return WorkspaceLocation::Wsl {
            original_path: path.to_path_buf(),
            distro,
            linux_path,
        };
    }

    if let Some((target, remote_path)) = parse_ssh_uri_path(&text) {
        return WorkspaceLocation::Ssh {
            original_path: path.to_path_buf(),
            target,
            path: remote_path,
        };
    }

    WorkspaceLocation::Local {
        path: path.to_path_buf(),
    }
}

pub fn workspace_path_is_absolute(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    path.has_root() || classify_workspace_location(path).is_remote()
}

pub fn absolutize_workspace_path(root: &Path, path: &Path) -> PathBuf {
    if path.starts_with(root) || workspace_path_is_absolute(path) {
        path.to_path_buf()
    } else if classify_workspace_location(root).is_remote() {
        join_path_with_separator(root, path, display_separator_for_root(root))
    } else {
        root.join(path)
    }
}

/// Returns a best-effort workspace root for a remote startup argument without
/// probing the host filesystem.
///
/// CLI positional arguments do not tell us whether a remote path is a file or a
/// directory. When the path looks file-like, this returns the remote parent;
/// otherwise it returns the remote path itself. Local paths return `None` so
/// callers can keep using precise local filesystem checks.
pub fn remote_startup_workspace_root(path: impl AsRef<Path>) -> Option<PathBuf> {
    let path = path.as_ref();

    match classify_workspace_location(path) {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            original_path,
            linux_path,
            ..
        } => Some(startup_display_workspace_root(&original_path, &linux_path)),
        WorkspaceLocation::Ssh {
            original_path,
            path: remote_path,
            ..
        } => Some(startup_display_workspace_root(&original_path, &remote_path)),
    }
}

/// Returns a best-effort file/directory hint for a remote path without probing
/// the host filesystem. Local paths return `None`.
pub fn remote_path_is_probably_file(path: impl AsRef<Path>) -> Option<bool> {
    let path = path.as_ref();

    match classify_workspace_location(path) {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            original_path,
            linux_path,
            ..
        } => Some(remote_startup_path_is_probably_file(
            &original_path,
            &linux_path,
        )),
        WorkspaceLocation::Ssh {
            original_path,
            path: remote_path,
            ..
        } => Some(remote_startup_path_is_probably_file(
            &original_path,
            &remote_path,
        )),
    }
}

fn remote_startup_path_is_probably_file(original_path: &Path, native_path: &Path) -> bool {
    let original_text = original_path.to_string_lossy();
    if original_text.ends_with('/') || original_text.ends_with('\\') {
        return false;
    }

    let Some(file_name) = native_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    if native_path.extension().is_some() {
        return true;
    }

    matches!(
        file_name,
        ".env"
            | ".envrc"
            | ".editorconfig"
            | ".gitattributes"
            | ".gitignore"
            | ".gitmodules"
            | "Containerfile"
            | "Dockerfile"
            | "Gemfile"
            | "Justfile"
            | "LICENSE"
            | "Makefile"
            | "Rakefile"
            | "README"
    )
}

fn startup_display_workspace_root(original_path: &Path, native_path: &Path) -> PathBuf {
    let original_text = original_path.to_string_lossy();
    let trimmed = original_text.trim_end_matches(['/', '\\']);

    if remote_startup_path_is_probably_file(original_path, native_path)
        && let Some((index, _)) = trimmed
            .char_indices()
            .rfind(|(_, ch)| matches!(ch, '/' | '\\'))
    {
        return PathBuf::from(&trimmed[..index]);
    }

    PathBuf::from(trimmed)
}

fn join_display_root(display_root: &Path, relative_path: &Path) -> PathBuf {
    if relative_path.as_os_str().is_empty() {
        display_root.to_path_buf()
    } else {
        join_path_with_separator(
            display_root,
            relative_path,
            display_separator_for_root(display_root),
        )
    }
}

fn display_separator_for_root(root: &Path) -> char {
    let root_text = root.to_string_lossy();
    if strip_prefix_ignore_ascii_case(&root_text, "ssh://").is_some() {
        '/'
    } else if root_text.contains('\\') {
        '\\'
    } else {
        '/'
    }
}

fn join_path_with_separator(root: &Path, relative_path: &Path, separator: char) -> PathBuf {
    let relative_text = relative_path_to_separated_string(relative_path, separator);
    if relative_text.is_empty() {
        return root.to_path_buf();
    }

    let root_text = root.to_string_lossy();
    let trimmed_root = root_text.trim_end_matches(['/', '\\']);
    if trimmed_root.is_empty() {
        PathBuf::from(format!("{separator}{relative_text}"))
    } else {
        PathBuf::from(format!("{trimmed_root}{separator}{relative_text}"))
    }
}

fn relative_path_to_separated_string(relative_path: &Path, separator: char) -> String {
    let separator = separator.to_string();
    relative_path
        .components()
        .filter_map(|component| match component {
            std::path::Component::CurDir => None,
            std::path::Component::ParentDir => Some("..".to_string()),
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => None,
        })
        .collect::<Vec<_>>()
        .join(&separator)
}

fn ssh_display_path_for_native_path(target: &SshWorkspaceTarget, native_path: &Path) -> PathBuf {
    let authority = ssh_authority(target);
    let path = percent_encode_posix_path(&posix_path_string(native_path));
    PathBuf::from(format!("ssh://{authority}{path}"))
}

pub fn ssh_display_path(target: &SshWorkspaceTarget, native_path: impl AsRef<Path>) -> PathBuf {
    ssh_display_path_for_native_path(target, native_path.as_ref())
}

fn ssh_authority(target: &SshWorkspaceTarget) -> String {
    let mut authority = String::new();
    if let Some(user) = &target.user {
        authority.push_str(user);
        authority.push('@');
    }

    if target.host.contains(':') {
        authority.push('[');
        authority.push_str(&target.host);
        authority.push(']');
    } else {
        authority.push_str(&target.host);
    }

    if let Some(port) = target.port {
        authority.push(':');
        authority.push_str(&port.to_string());
    }

    authority
}

fn wsl_display_path_for_native_path(
    original_path: &Path,
    distro: &str,
    native_path: &Path,
) -> PathBuf {
    let original_text = original_path.to_string_lossy();
    let native_text = posix_path_string(native_path);
    if original_text.contains('\\') {
        let relative_text = native_text.trim_start_matches('/').replace('/', "\\");
        if relative_text.is_empty() {
            PathBuf::from(format!(r"\\wsl.localhost\{distro}"))
        } else {
            PathBuf::from(format!(r"\\wsl.localhost\{distro}\{relative_text}"))
        }
    } else {
        let relative_text = native_text.trim_start_matches('/');
        if relative_text.is_empty() {
            PathBuf::from(format!("//wsl.localhost/{distro}"))
        } else {
            PathBuf::from(format!("//wsl.localhost/{distro}/{relative_text}"))
        }
    }
}

pub fn wsl_display_path(distro: &str, native_path: impl AsRef<Path>) -> PathBuf {
    wsl_display_path_for_native_path(
        Path::new(&format!("//wsl.localhost/{distro}")),
        distro,
        native_path.as_ref(),
    )
}

pub fn posix_path_string(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    path.to_string_lossy().replace('\\', "/")
}

fn percent_encode_posix_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_uri_component)
        .collect::<Vec<_>>()
        .join("/")
}

fn percent_encode_uri_component(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push_str(&format!("{byte:02X}"));
        }
    }
    output
}

fn parse_wsl_unc_path(value: &str) -> Option<(String, PathBuf)> {
    let normalized = value.replace('\\', "/");
    let rest = strip_prefix_ignore_ascii_case(&normalized, "//wsl.localhost/")
        .or_else(|| strip_prefix_ignore_ascii_case(&normalized, "//wsl$/"))?;
    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let distro = parts.next()?.to_string();
    let linux_path = path_from_posix_segments(parts);

    Some((distro, linux_path))
}

fn parse_ssh_uri_path(value: &str) -> Option<(SshWorkspaceTarget, PathBuf)> {
    let rest = strip_prefix_ignore_ascii_case(value, "ssh://")?;
    let (authority, remote_path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() {
        return None;
    }

    let (user, host_and_port) = authority
        .rsplit_once('@')
        .map(|(user, host)| (Some(user.to_string()), host))
        .unwrap_or((None, authority));
    let (host, port) = parse_ssh_host_and_port(host_and_port)?;

    Some((
        SshWorkspaceTarget { host, user, port },
        path_from_percent_encoded_posix_path(remote_path),
    ))
}

fn parse_ssh_host_and_port(value: &str) -> Option<(String, Option<u16>)> {
    if value.is_empty() {
        return None;
    }

    if let Some(bracketed) = value.strip_prefix('[') {
        let (host, rest) = bracketed.split_once(']')?;
        if host.is_empty() {
            return None;
        }

        return match rest {
            "" => Some((host.to_string(), None)),
            rest => {
                let port = rest.strip_prefix(':')?.parse::<u16>().ok()?;
                Some((host.to_string(), Some(port)))
            }
        };
    }

    if let Some((host, port)) = value.rsplit_once(':')
        && !host.is_empty()
        && !host.contains(':')
        && let Ok(port) = port.parse::<u16>()
    {
        return Some((host.to_string(), Some(port)));
    }

    Some((value.to_string(), None))
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .then(|| &value[prefix.len()..])
}

fn path_from_posix_segments<'a>(segments: impl IntoIterator<Item = &'a str>) -> PathBuf {
    let mut path = String::from("/");
    for segment in segments {
        if !segment.is_empty() {
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(segment);
        }
    }
    PathBuf::from(path)
}

fn path_from_percent_encoded_posix_path(value: &str) -> PathBuf {
    let mut path = String::from("/");
    for segment in value.split('/').filter(|segment| !segment.is_empty()) {
        if !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(&percent_decode_uri_component(segment));
    }
    PathBuf::from(path)
}

fn percent_decode_uri_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                hex_digit_value(bytes[index + 1]),
                hex_digit_value(bytes[index + 2]),
            )
        {
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8_lossy(&output).into_owned()
}

fn hex_digit_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: PathBuf,
    pub stat: FileStat,
    pub symlink_target: Option<PathBuf>,
    pub target_exists: Option<bool>,
    pub ignored: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListing {
    pub path: PathBuf,
    pub entries: Vec<DirectoryEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadOptions {
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub readonly: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteOptions {
    pub create_parent_dirs: bool,
    pub expected_modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSearchQuery {
    pub root: PathBuf,
    pub pattern: Option<String>,
    pub limit: usize,
    pub hidden: bool,
    pub parents: bool,
    pub ignore: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
    pub follow_links: bool,
    pub max_depth: Option<usize>,
    pub excluded_relative_prefixes: Vec<PathBuf>,
}

impl Default for FileSearchQuery {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            pattern: None,
            limit: 1_000,
            hidden: false,
            parents: true,
            ignore: true,
            git_ignore: true,
            git_global: true,
            git_exclude: true,
            follow_links: false,
            max_depth: None,
            excluded_relative_prefixes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSearchResult {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSearchQuery {
    pub root: PathBuf,
    pub pattern: String,
    pub limit: usize,
    pub smart_case: bool,
    pub hidden: bool,
    pub parents: bool,
    pub ignore: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
    pub follow_links: bool,
    pub max_depth: Option<usize>,
    pub max_file_bytes: u64,
    pub excluded_relative_paths: Vec<PathBuf>,
    pub custom_ignore_filenames: Vec<PathBuf>,
}

impl Default for TextSearchQuery {
    fn default() -> Self {
        let file_query = FileSearchQuery::default();
        Self {
            root: file_query.root,
            pattern: String::new(),
            limit: 1_000,
            smart_case: true,
            hidden: file_query.hidden,
            parents: file_query.parents,
            ignore: file_query.ignore,
            git_ignore: file_query.git_ignore,
            git_global: file_query.git_global,
            git_exclude: file_query.git_exclude,
            follow_links: file_query.follow_links,
            max_depth: file_query.max_depth,
            max_file_bytes: 1_000_000,
            excluded_relative_paths: Vec::new(),
            custom_ignore_filenames: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSearchMatch {
    pub relative_path: PathBuf,
    pub line_number: usize,
    pub line_text: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSearchResult {
    pub root: PathBuf,
    pub matches: Vec<TextSearchMatch>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectEnvironmentOrigin {
    NativeFlake,
    DirectoryShell,
    ProcessBaseline,
    Cli,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEnvironmentSnapshot {
    pub root: PathBuf,
    pub variables: BTreeMap<String, String>,
    pub origin: ProjectEnvironmentOrigin,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitStatusOptions {
    pub include_untracked: bool,
    pub limit: usize,
}

impl GitStatusOptions {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatusKind {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusEntry {
    pub relative_path: PathBuf,
    pub original_relative_path: Option<PathBuf>,
    pub index_status: GitStatusKind,
    pub working_tree_status: GitStatusKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitStatusResult {
    pub root: PathBuf,
    pub entries: Vec<GitStatusEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHeadResult {
    pub root: PathBuf,
    pub head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub clear_env: bool,
    pub inherit_project_environment: bool,
    pub stdin: Vec<u8>,
    pub max_output_bytes: Option<usize>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status_code: Option<i32>,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWatchRequest {
    pub roots: Vec<PathBuf>,
    pub debounce_ms: u32,
    pub max_events_per_batch: u32,
}

impl WorkspaceWatchRequest {
    pub fn expanded_dirs(roots: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        Self {
            roots: roots.into_iter().map(Into::into).collect(),
            debounce_ms: 200,
            max_events_per_batch: 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWatchUpdate {
    pub watch_id: u64,
    pub accepted_roots: Vec<PathBuf>,
    pub degraded_roots: Vec<PathBuf>,
    pub unsupported_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWatchDirectoryGeneration {
    pub path: PathBuf,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceWatchChangeKind {
    Created,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWatchChange {
    pub kind: WorkspaceWatchChangeKind,
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWatchBatch {
    pub watch_id: u64,
    pub sequence: u64,
    pub directory_generations: Vec<WorkspaceWatchDirectoryGeneration>,
    pub events: Vec<WorkspaceWatchChange>,
    pub overflow: bool,
    pub resync_required: bool,
}

#[derive(Clone)]
pub struct WorkspaceWatch {
    pub watch_id: u64,
    pub event_stream_id: u64,
    receiver: Arc<Mutex<mpsc::Receiver<WorkspaceWatchBatch>>>,
}

impl WorkspaceWatch {
    pub fn new(
        watch_id: u64,
        event_stream_id: u64,
        receiver: mpsc::Receiver<WorkspaceWatchBatch>,
    ) -> Self {
        Self {
            watch_id,
            event_stream_id,
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }

    pub fn recv(&self) -> std::result::Result<WorkspaceWatchBatch, mpsc::RecvError> {
        match self.receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(_) => Err(mpsc::RecvError),
        }
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<WorkspaceWatchBatch, mpsc::RecvTimeoutError> {
        match self.receiver.lock() {
            Ok(receiver) => receiver.recv_timeout(timeout),
            Err(_) => Err(mpsc::RecvTimeoutError::Disconnected),
        }
    }

    pub fn try_recv(&self) -> std::result::Result<WorkspaceWatchBatch, mpsc::TryRecvError> {
        match self.receiver.lock() {
            Ok(receiver) => receiver.try_recv(),
            Err(_) => Err(mpsc::TryRecvError::Disconnected),
        }
    }
}

impl Default for GitStatusOptions {
    fn default() -> Self {
        Self {
            include_untracked: true,
            limit: 10_000,
        }
    }
}

#[async_trait]
pub trait WorkspaceBackend: Send + Sync {
    fn identity(&self) -> WorkspaceIdentity;

    async fn stat(&self, path: &Path) -> Result<FileStat>;

    async fn list_dir(&self, path: &Path) -> Result<DirectoryListing>;

    async fn list_dirs(&self, paths: Vec<PathBuf>) -> Vec<(PathBuf, Result<DirectoryListing>)> {
        let mut listings = Vec::with_capacity(paths.len());
        for path in paths {
            let listing = self.list_dir(&path).await;
            listings.push((path, listing));
        }
        listings
    }

    async fn find_ancestor_file(
        &self,
        start: &Path,
        file_name: &str,
        limit: usize,
    ) -> Result<Option<PathBuf>>;

    async fn create_file(&self, path: &Path) -> Result<FileStat>;

    async fn create_dir(&self, path: &Path) -> Result<FileStat>;

    async fn rename_path(&self, from: &Path, to: &Path) -> Result<FileStat>;

    async fn delete_path(&self, path: &Path) -> Result<FileStat>;

    async fn copy_path(&self, from: &Path, to: &Path) -> Result<FileStat>;

    async fn read_file(&self, path: &Path, options: ReadOptions) -> Result<FileRead>;

    async fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        options: WriteOptions,
    ) -> Result<WriteResult>;

    async fn file_search(&self, query: FileSearchQuery) -> Result<FileSearchResult>;

    async fn text_search(&self, query: TextSearchQuery) -> Result<TextSearchResult>;

    async fn project_environment(&self, root: &Path) -> Result<ProjectEnvironmentSnapshot>;

    async fn git_head(&self, root: &Path) -> Result<GitHeadResult>;

    async fn git_status(&self, root: &Path, options: GitStatusOptions) -> Result<GitStatusResult>;

    async fn run_process(&self, spec: ProcessSpec) -> Result<ProcessOutput>;

    async fn start_watch(&self, _request: WorkspaceWatchRequest) -> Result<Option<WorkspaceWatch>> {
        Ok(None)
    }

    async fn update_watch(
        &self,
        _watch_id: u64,
        _add_roots: Vec<PathBuf>,
        _remove_roots: Vec<PathBuf>,
    ) -> Result<Option<WorkspaceWatchUpdate>> {
        Ok(None)
    }

    async fn stop_watch(&self, _watch_id: u64) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
pub struct LocalWorkspaceBackend;

pub type WorkspaceBackendHandle = Arc<dyn WorkspaceBackend>;

pub fn local_workspace_backend() -> WorkspaceBackendHandle {
    Arc::new(LocalWorkspaceBackend)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePathMapping {
    display_root: PathBuf,
    native_root: PathBuf,
}

impl WorkspacePathMapping {
    pub fn new(display_root: impl Into<PathBuf>, native_root: impl Into<PathBuf>) -> Self {
        Self {
            display_root: display_root.into(),
            native_root: native_root.into(),
        }
    }

    pub fn display_root(&self) -> &Path {
        &self.display_root
    }

    pub fn native_root(&self) -> &Path {
        &self.native_root
    }

    pub fn to_native_path(&self, path: &Path) -> PathBuf {
        let mapped_path = rebase_workspace_path(path, &self.display_root, &self.native_root);
        if mapped_path != path {
            return mapped_path;
        }

        let display_location = classify_workspace_location(&self.display_root);
        if !display_location.is_remote() {
            return path.to_path_buf();
        }

        let path_location = classify_workspace_location(path);
        if display_location.matches_remote_identity(&path_location) {
            path_location.native_root().to_path_buf()
        } else {
            path.to_path_buf()
        }
    }

    pub fn to_display_path(&self, path: &Path) -> PathBuf {
        let mapped_path = rebase_workspace_path(path, &self.native_root, &self.display_root);
        if mapped_path != path {
            return mapped_path;
        }

        let display_location = classify_workspace_location(&self.display_root);
        if display_location.is_remote() && path.has_root() {
            display_location.display_path_for_native_path(path)
        } else {
            path.to_path_buf()
        }
    }
}

pub struct PathMappedWorkspaceBackend {
    inner: WorkspaceBackendHandle,
    mapping: WorkspacePathMapping,
}

impl PathMappedWorkspaceBackend {
    pub fn new(inner: WorkspaceBackendHandle, mapping: WorkspacePathMapping) -> Self {
        Self { inner, mapping }
    }

    pub fn mapping(&self) -> &WorkspacePathMapping {
        &self.mapping
    }

    fn map_file_stat_to_display(&self, mut stat: FileStat) -> FileStat {
        stat.path = self.mapping.to_display_path(&stat.path);
        stat
    }

    fn map_directory_entry_to_display(&self, mut entry: DirectoryEntry) -> DirectoryEntry {
        entry.path = self.mapping.to_display_path(&entry.path);
        entry.stat = self.map_file_stat_to_display(entry.stat);
        entry.symlink_target = entry
            .symlink_target
            .map(|target| self.mapping.to_display_path(&target));
        entry
    }

    fn map_directory_listing_to_display(&self, mut listing: DirectoryListing) -> DirectoryListing {
        listing.path = self.mapping.to_display_path(&listing.path);
        listing.entries = listing
            .entries
            .into_iter()
            .map(|entry| self.map_directory_entry_to_display(entry))
            .collect();
        listing
    }

    fn map_file_read_to_display(&self, mut read: FileRead) -> FileRead {
        read.path = self.mapping.to_display_path(&read.path);
        read
    }

    fn map_write_result_to_display(&self, mut result: WriteResult) -> WriteResult {
        result.path = self.mapping.to_display_path(&result.path);
        result
    }

    fn map_file_search_query_to_native(&self, mut query: FileSearchQuery) -> FileSearchQuery {
        query.root = self.mapping.to_native_path(&query.root);
        query
    }

    fn map_file_search_result_to_display(&self, mut result: FileSearchResult) -> FileSearchResult {
        result.root = self.mapping.to_display_path(&result.root);
        result
    }

    fn map_text_search_query_to_native(&self, mut query: TextSearchQuery) -> TextSearchQuery {
        query.root = self.mapping.to_native_path(&query.root);
        query
    }

    fn map_text_search_result_to_display(&self, mut result: TextSearchResult) -> TextSearchResult {
        result.root = self.mapping.to_display_path(&result.root);
        result
    }

    fn map_project_environment_to_display(
        &self,
        mut snapshot: ProjectEnvironmentSnapshot,
    ) -> ProjectEnvironmentSnapshot {
        snapshot.root = self.mapping.to_display_path(&snapshot.root);
        snapshot
    }

    fn map_git_head_to_display(&self, mut result: GitHeadResult) -> GitHeadResult {
        result.root = self.mapping.to_display_path(&result.root);
        result
    }

    fn map_git_status_to_display(&self, mut result: GitStatusResult) -> GitStatusResult {
        result.root = self.mapping.to_display_path(&result.root);
        result
    }

    fn map_process_spec_to_native(&self, mut spec: ProcessSpec) -> ProcessSpec {
        spec.cwd = self.mapping.to_native_path(&spec.cwd);
        spec
    }

    fn map_watch_request_to_native(
        &self,
        mut request: WorkspaceWatchRequest,
    ) -> WorkspaceWatchRequest {
        request.roots = request
            .roots
            .into_iter()
            .map(|root| self.mapping.to_native_path(&root))
            .collect();
        request
    }

    fn map_watch_update_to_display(
        &self,
        mut update: WorkspaceWatchUpdate,
    ) -> WorkspaceWatchUpdate {
        update.accepted_roots = update
            .accepted_roots
            .into_iter()
            .map(|root| self.mapping.to_display_path(&root))
            .collect();
        update.degraded_roots = update
            .degraded_roots
            .into_iter()
            .map(|root| self.mapping.to_display_path(&root))
            .collect();
        update.unsupported_roots = update
            .unsupported_roots
            .into_iter()
            .map(|root| self.mapping.to_display_path(&root))
            .collect();
        update
    }

    fn map_watch_batch_with_mapping(
        mapping: &WorkspacePathMapping,
        batch: &mut WorkspaceWatchBatch,
    ) {
        batch.directory_generations = batch
            .directory_generations
            .drain(..)
            .map(|generation| WorkspaceWatchDirectoryGeneration {
                path: mapping.to_display_path(&generation.path),
                generation: generation.generation,
            })
            .collect();
        batch.events = batch
            .events
            .drain(..)
            .map(|event| WorkspaceWatchChange {
                kind: event.kind,
                path: mapping.to_display_path(&event.path),
                old_path: event
                    .old_path
                    .map(|old_path| mapping.to_display_path(&old_path)),
                is_dir: event.is_dir,
            })
            .collect();
    }

    fn map_watch_to_display(&self, watch: WorkspaceWatch) -> WorkspaceWatch {
        let mapping = self.mapping.clone();
        let (sender, receiver) = mpsc::channel();
        let watch_id = watch.watch_id;
        let event_stream_id = watch.event_stream_id;
        std::thread::Builder::new()
            .name("nucleotide-workspace-watch-map".to_string())
            .spawn(move || {
                while let Ok(mut batch) = watch.recv() {
                    PathMappedWorkspaceBackend::map_watch_batch_with_mapping(&mapping, &mut batch);
                    let mapped = batch;
                    if sender.send(mapped).is_err() {
                        break;
                    }
                }
            })
            .ok();
        WorkspaceWatch::new(watch_id, event_stream_id, receiver)
    }
}

pub fn path_mapped_workspace_backend(
    inner: WorkspaceBackendHandle,
    mapping: WorkspacePathMapping,
) -> WorkspaceBackendHandle {
    Arc::new(PathMappedWorkspaceBackend::new(inner, mapping))
}

#[async_trait]
impl WorkspaceBackend for PathMappedWorkspaceBackend {
    fn identity(&self) -> WorkspaceIdentity {
        self.inner.identity()
    }

    async fn stat(&self, path: &Path) -> Result<FileStat> {
        let native_path = self.mapping.to_native_path(path);
        self.inner
            .stat(&native_path)
            .await
            .map(|stat| self.map_file_stat_to_display(stat))
    }

    async fn list_dir(&self, path: &Path) -> Result<DirectoryListing> {
        let native_path = self.mapping.to_native_path(path);
        self.inner
            .list_dir(&native_path)
            .await
            .map(|listing| self.map_directory_listing_to_display(listing))
    }

    async fn find_ancestor_file(
        &self,
        start: &Path,
        file_name: &str,
        limit: usize,
    ) -> Result<Option<PathBuf>> {
        let native_start = self.mapping.to_native_path(start);
        self.inner
            .find_ancestor_file(&native_start, file_name, limit)
            .await
            .map(|path| path.map(|path| self.mapping.to_display_path(&path)))
    }

    async fn create_file(&self, path: &Path) -> Result<FileStat> {
        let native_path = self.mapping.to_native_path(path);
        self.inner
            .create_file(&native_path)
            .await
            .map(|stat| self.map_file_stat_to_display(stat))
    }

    async fn create_dir(&self, path: &Path) -> Result<FileStat> {
        let native_path = self.mapping.to_native_path(path);
        self.inner
            .create_dir(&native_path)
            .await
            .map(|stat| self.map_file_stat_to_display(stat))
    }

    async fn rename_path(&self, from: &Path, to: &Path) -> Result<FileStat> {
        let native_from = self.mapping.to_native_path(from);
        let native_to = self.mapping.to_native_path(to);
        self.inner
            .rename_path(&native_from, &native_to)
            .await
            .map(|stat| self.map_file_stat_to_display(stat))
    }

    async fn delete_path(&self, path: &Path) -> Result<FileStat> {
        let native_path = self.mapping.to_native_path(path);
        self.inner
            .delete_path(&native_path)
            .await
            .map(|stat| self.map_file_stat_to_display(stat))
    }

    async fn copy_path(&self, from: &Path, to: &Path) -> Result<FileStat> {
        let native_from = self.mapping.to_native_path(from);
        let native_to = self.mapping.to_native_path(to);
        self.inner
            .copy_path(&native_from, &native_to)
            .await
            .map(|stat| self.map_file_stat_to_display(stat))
    }

    async fn read_file(&self, path: &Path, options: ReadOptions) -> Result<FileRead> {
        let native_path = self.mapping.to_native_path(path);
        self.inner
            .read_file(&native_path, options)
            .await
            .map(|read| self.map_file_read_to_display(read))
    }

    async fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        options: WriteOptions,
    ) -> Result<WriteResult> {
        let native_path = self.mapping.to_native_path(path);
        self.inner
            .write_file(&native_path, bytes, options)
            .await
            .map(|result| self.map_write_result_to_display(result))
    }

    async fn file_search(&self, query: FileSearchQuery) -> Result<FileSearchResult> {
        self.inner
            .file_search(self.map_file_search_query_to_native(query))
            .await
            .map(|result| self.map_file_search_result_to_display(result))
    }

    async fn text_search(&self, query: TextSearchQuery) -> Result<TextSearchResult> {
        self.inner
            .text_search(self.map_text_search_query_to_native(query))
            .await
            .map(|result| self.map_text_search_result_to_display(result))
    }

    async fn project_environment(&self, root: &Path) -> Result<ProjectEnvironmentSnapshot> {
        let native_root = self.mapping.to_native_path(root);
        self.inner
            .project_environment(&native_root)
            .await
            .map(|snapshot| self.map_project_environment_to_display(snapshot))
    }

    async fn git_head(&self, root: &Path) -> Result<GitHeadResult> {
        let native_root = self.mapping.to_native_path(root);
        self.inner
            .git_head(&native_root)
            .await
            .map(|result| self.map_git_head_to_display(result))
    }

    async fn git_status(&self, root: &Path, options: GitStatusOptions) -> Result<GitStatusResult> {
        let native_root = self.mapping.to_native_path(root);
        self.inner
            .git_status(&native_root, options)
            .await
            .map(|result| self.map_git_status_to_display(result))
    }

    async fn run_process(&self, spec: ProcessSpec) -> Result<ProcessOutput> {
        self.inner
            .run_process(self.map_process_spec_to_native(spec))
            .await
    }

    async fn start_watch(&self, request: WorkspaceWatchRequest) -> Result<Option<WorkspaceWatch>> {
        let watch = self
            .inner
            .start_watch(self.map_watch_request_to_native(request))
            .await?;
        Ok(watch.map(|watch| self.map_watch_to_display(watch)))
    }

    async fn update_watch(
        &self,
        watch_id: u64,
        add_roots: Vec<PathBuf>,
        remove_roots: Vec<PathBuf>,
    ) -> Result<Option<WorkspaceWatchUpdate>> {
        let add_roots = add_roots
            .into_iter()
            .map(|root| self.mapping.to_native_path(&root))
            .collect();
        let remove_roots = remove_roots
            .into_iter()
            .map(|root| self.mapping.to_native_path(&root))
            .collect();
        Ok(self
            .inner
            .update_watch(watch_id, add_roots, remove_roots)
            .await?
            .map(|update| self.map_watch_update_to_display(update)))
    }

    async fn stop_watch(&self, watch_id: u64) -> Result<()> {
        self.inner.stop_watch(watch_id).await
    }
}

fn rebase_workspace_path(path: &Path, from_root: &Path, to_root: &Path) -> PathBuf {
    if classify_workspace_location(from_root).is_remote()
        && let Some(relative_path) = strip_remote_workspace_prefix(path, from_root)
    {
        return join_rebased_workspace_path(from_root, to_root, &relative_path);
    }

    match path.strip_prefix(from_root) {
        Ok(relative_path) if relative_path.as_os_str().is_empty() => to_root.to_path_buf(),
        Ok(relative_path) => join_rebased_workspace_path(from_root, to_root, relative_path),
        Err(_) => path.to_path_buf(),
    }
}

fn join_rebased_workspace_path(from_root: &Path, to_root: &Path, relative_path: &Path) -> PathBuf {
    if relative_path.as_os_str().is_empty() {
        return to_root.to_path_buf();
    }

    if classify_workspace_location(to_root).is_remote() {
        join_path_with_separator(to_root, relative_path, display_separator_for_root(to_root))
    } else if classify_workspace_location(from_root).is_remote() {
        join_path_with_separator(to_root, relative_path, '/')
    } else {
        to_root.join(relative_path)
    }
}

fn strip_remote_workspace_prefix(path: &Path, root: &Path) -> Option<PathBuf> {
    let path_text = path.to_string_lossy();
    let root_text = root.to_string_lossy();
    let trimmed_root = root_text.trim_end_matches(['/', '\\']);

    if path_text == trimmed_root {
        return Some(PathBuf::new());
    }

    let rest = path_text.strip_prefix(trimmed_root)?;
    let rest = rest.strip_prefix('/').or_else(|| rest.strip_prefix('\\'))?;

    Some(path_from_mixed_separators(rest))
}

fn path_from_mixed_separators(value: &str) -> PathBuf {
    value
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .collect()
}

#[async_trait]
impl WorkspaceBackend for LocalWorkspaceBackend {
    fn identity(&self) -> WorkspaceIdentity {
        WorkspaceIdentity::Local
    }

    async fn stat(&self, path: &Path) -> Result<FileStat> {
        local_stat(path)
    }

    async fn list_dir(&self, path: &Path) -> Result<DirectoryListing> {
        local_list_dir(path)
    }

    async fn find_ancestor_file(
        &self,
        start: &Path,
        file_name: &str,
        limit: usize,
    ) -> Result<Option<PathBuf>> {
        local_find_ancestor_file(start, file_name, limit)
    }

    async fn create_file(&self, path: &Path) -> Result<FileStat> {
        local_create_file(path)
    }

    async fn create_dir(&self, path: &Path) -> Result<FileStat> {
        local_create_dir(path)
    }

    async fn rename_path(&self, from: &Path, to: &Path) -> Result<FileStat> {
        local_rename_path(from, to)
    }

    async fn delete_path(&self, path: &Path) -> Result<FileStat> {
        local_delete_path(path)
    }

    async fn copy_path(&self, from: &Path, to: &Path) -> Result<FileStat> {
        local_copy_path(from, to)
    }

    async fn read_file(&self, path: &Path, options: ReadOptions) -> Result<FileRead> {
        local_read_file(path, options)
    }

    async fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        options: WriteOptions,
    ) -> Result<WriteResult> {
        local_write_file(path, bytes, options)
    }

    async fn file_search(&self, query: FileSearchQuery) -> Result<FileSearchResult> {
        local_file_search(query)
    }

    async fn text_search(&self, query: TextSearchQuery) -> Result<TextSearchResult> {
        local_text_search(query)
    }

    async fn project_environment(&self, root: &Path) -> Result<ProjectEnvironmentSnapshot> {
        local_project_environment(root)
    }

    async fn git_head(&self, root: &Path) -> Result<GitHeadResult> {
        local_git_head(root)
    }

    async fn git_status(&self, root: &Path, options: GitStatusOptions) -> Result<GitStatusResult> {
        local_git_status(root, options)
    }

    async fn run_process(&self, spec: ProcessSpec) -> Result<ProcessOutput> {
        local_run_process(spec)
    }
}

fn local_stat(path: &Path) -> Result<FileStat> {
    let metadata = fs::symlink_metadata(path).map_err(|source| WorkspaceError::Io {
        operation: "stat",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(file_stat_from_metadata(path.to_path_buf(), metadata))
}

fn local_list_dir(path: &Path) -> Result<DirectoryListing> {
    let entries = fs::read_dir(path).map_err(|source| WorkspaceError::Io {
        operation: "list directory",
        path: path.to_path_buf(),
        source,
    })?;
    let mut entries = entries
        .map(|entry| {
            let entry = entry.map_err(|source| WorkspaceError::Io {
                operation: "read directory entry",
                path: path.to_path_buf(),
                source,
            })?;
            let entry_path = entry.path();
            let metadata =
                fs::symlink_metadata(&entry_path).map_err(|source| WorkspaceError::Io {
                    operation: "stat directory entry",
                    path: entry_path.clone(),
                    source,
                })?;
            let file_type = metadata.file_type();
            let symlink_target = file_type
                .is_symlink()
                .then(|| fs::read_link(&entry_path))
                .transpose()
                .map_err(|source| WorkspaceError::Io {
                    operation: "read symlink",
                    path: entry_path.clone(),
                    source,
                })?;
            let target_exists = symlink_target.as_ref().map(|target| {
                if target.is_absolute() {
                    target.exists()
                } else {
                    entry_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(target)
                        .exists()
                }
            });

            Ok(DirectoryEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry_path.clone(),
                stat: file_stat_from_metadata(entry_path, metadata),
                symlink_target,
                target_exists,
                ignored: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(DirectoryListing {
        path: path.to_path_buf(),
        entries,
    })
}

fn ensure_not_exists(path: &Path, operation: &'static str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(WorkspaceError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::AlreadyExists, "path already exists"),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(WorkspaceError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn is_case_only_name_change(from: &Path, to: &Path) -> bool {
    let same_parent = from.parent() == to.parent();
    let from_name = from.file_name().and_then(|name| name.to_str());
    let to_name = to.file_name().and_then(|name| name.to_str());

    same_parent
        && matches!(
            (from_name, to_name),
            (Some(from_name), Some(to_name))
                if from_name.eq_ignore_ascii_case(to_name) && from_name != to_name
        )
}

fn rename_target_is_source(from: &Path, to: &Path) -> Result<bool> {
    let to_metadata = match fs::symlink_metadata(to) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(WorkspaceError::Io {
                operation: "stat rename target",
                path: to.to_path_buf(),
                source,
            });
        }
    };

    let from_metadata = fs::symlink_metadata(from).map_err(|source| WorkspaceError::Io {
        operation: "stat rename source",
        path: from.to_path_buf(),
        source,
    })?;

    same_file_metadata(from, to, &from_metadata, &to_metadata)
}

#[cfg(unix)]
fn same_file_metadata(
    _left_path: &Path,
    _right_path: &Path,
    left: &fs::Metadata,
    right: &fs::Metadata,
) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(windows)]
fn same_file_metadata(
    left_path: &Path,
    right_path: &Path,
    _left: &fs::Metadata,
    _right: &fs::Metadata,
) -> Result<bool> {
    let left = windows_file_id(left_path).map_err(|source| WorkspaceError::Io {
        operation: "identify rename source",
        path: left_path.to_path_buf(),
        source,
    })?;
    let right = windows_file_id(right_path).map_err(|source| WorkspaceError::Io {
        operation: "identify rename target",
        path: right_path.to_path_buf(),
        source,
    })?;

    Ok(left.has_file_index() && left == right)
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsFileId {
    volume_serial_number: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
impl WindowsFileId {
    fn has_file_index(self) -> bool {
        self.file_index_high != 0 || self.file_index_low != 0
    }
}

#[cfg(windows)]
fn windows_file_id(path: &Path) -> std::io::Result<WindowsFileId> {
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    };

    let file = fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_DELETE | FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;

    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let info = unsafe { info.assume_init() };
    Ok(WindowsFileId {
        volume_serial_number: info.dwVolumeSerialNumber,
        file_index_high: info.nFileIndexHigh,
        file_index_low: info.nFileIndexLow,
    })
}

#[cfg(not(any(unix, windows)))]
fn same_file_metadata(
    _left_path: &Path,
    _right_path: &Path,
    _left: &fs::Metadata,
    _right: &fs::Metadata,
) -> Result<bool> {
    Ok(false)
}

fn lexical_absolute(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| WorkspaceError::Io {
                operation: "resolve current directory",
                path: path.to_path_buf(),
                source,
            })?
            .join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }

    Ok(normalized)
}

fn path_is_self_or_descendant(path: &Path, ancestor: &Path) -> Result<bool> {
    let path = lexical_absolute(path)?;
    let ancestor = lexical_absolute(ancestor)?;
    Ok(path == ancestor || path.starts_with(ancestor))
}

fn validate_ancestor_file_name(file_name: &str) -> Result<()> {
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains(std::path::MAIN_SEPARATOR)
        || file_name.contains('/')
        || file_name.contains('\\')
    {
        return Err(WorkspaceError::CommandFailed {
            operation: "find ancestor file",
            path: PathBuf::from(file_name),
            message: "file name must not contain path separators".to_string(),
        });
    }

    Ok(())
}

fn local_find_ancestor_file(
    start: &Path,
    file_name: &str,
    limit: usize,
) -> Result<Option<PathBuf>> {
    validate_ancestor_file_name(file_name)?;

    let start_stat = local_stat(start)?;
    let mut current = if start_stat.kind == FileKind::Directory {
        start.to_path_buf()
    } else {
        match start.parent() {
            Some(parent) => parent.to_path_buf(),
            None => return Ok(None),
        }
    };

    for _ in 0..=limit {
        let candidate = current.join(file_name);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                return Ok(Some(candidate));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(WorkspaceError::Io {
                    operation: "find ancestor file",
                    path: candidate,
                    source,
                });
            }
        }

        if !current.pop() {
            break;
        }
    }

    Ok(None)
}

fn local_create_file(path: &Path) -> Result<FileStat> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| WorkspaceError::Io {
            operation: "create parent directories",
            path: parent.to_path_buf(),
            source,
        })?;
    }

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| WorkspaceError::Io {
            operation: "create file",
            path: path.to_path_buf(),
            source,
        })?;

    local_stat(path)
}

fn local_create_dir(path: &Path) -> Result<FileStat> {
    ensure_not_exists(path, "create directory")?;
    fs::create_dir_all(path).map_err(|source| WorkspaceError::Io {
        operation: "create directory",
        path: path.to_path_buf(),
        source,
    })?;

    local_stat(path)
}

fn local_rename_path(from: &Path, to: &Path) -> Result<FileStat> {
    let case_only_name_change = is_case_only_name_change(from, to);
    let target_is_source = case_only_name_change && rename_target_is_source(from, to)?;
    if !target_is_source {
        ensure_not_exists(to, "rename path")?;
    }

    let rename = |source_path: &Path, target_path: &Path| {
        fs::rename(source_path, target_path).map_err(|source| WorkspaceError::Io {
            operation: "rename path",
            path: source_path.to_path_buf(),
            source,
        })
    };

    match rename(from, to) {
        Ok(()) => local_stat(to),
        Err(first_error) => {
            if case_only_name_change
                && let (Some(to_name), Some(parent)) =
                    (to.file_name().and_then(|name| name.to_str()), from.parent())
            {
                let temp_path = parent.join(format!(
                    ".nucleotide-rename-{}-{to_name}",
                    std::process::id()
                ));
                ensure_not_exists(&temp_path, "rename path")?;
                rename(from, &temp_path)?;
                rename(&temp_path, to)?;
                return local_stat(to);
            }

            Err(first_error)
        }
    }
}

fn local_delete_path(path: &Path) -> Result<FileStat> {
    let stat = local_stat(path)?;
    match stat.kind {
        FileKind::Directory => fs::remove_dir_all(path).map_err(|source| WorkspaceError::Io {
            operation: "delete directory",
            path: path.to_path_buf(),
            source,
        })?,
        FileKind::File | FileKind::Symlink | FileKind::Other => {
            fs::remove_file(path).map_err(|source| WorkspaceError::Io {
                operation: "delete file",
                path: path.to_path_buf(),
                source,
            })?;
        }
    }

    Ok(stat)
}

fn local_copy_path(from: &Path, to: &Path) -> Result<FileStat> {
    ensure_not_exists(to, "copy path")?;
    let from_stat = local_stat(from)?;
    match from_stat.kind {
        FileKind::Directory => {
            if path_is_self_or_descendant(to, from)? {
                return Err(WorkspaceError::CommandFailed {
                    operation: "copy path",
                    path: from.to_path_buf(),
                    message: "cannot copy a directory into itself".to_string(),
                });
            }
            copy_dir_recursive(from, to)?;
        }
        FileKind::File => {
            fs::copy(from, to).map_err(|source| WorkspaceError::Io {
                operation: "copy file",
                path: from.to_path_buf(),
                source,
            })?;
        }
        FileKind::Symlink => copy_symlink_target(from, to)?,
        FileKind::Other => {
            return Err(WorkspaceError::NotFile {
                path: from.to_path_buf(),
            });
        }
    }

    local_stat(to)
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).map_err(|source| WorkspaceError::Io {
        operation: "create copied directory",
        path: to.to_path_buf(),
        source,
    })?;

    for entry in fs::read_dir(from).map_err(|source| WorkspaceError::Io {
        operation: "read directory for copy",
        path: from.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| WorkspaceError::Io {
            operation: "read directory entry for copy",
            path: from.to_path_buf(),
            source,
        })?;
        let entry_from = entry.path();
        let entry_to = to.join(entry.file_name());
        let entry_stat = local_stat(&entry_from)?;
        match entry_stat.kind {
            FileKind::Directory => copy_dir_recursive(&entry_from, &entry_to)?,
            FileKind::File => {
                fs::copy(&entry_from, &entry_to).map_err(|source| WorkspaceError::Io {
                    operation: "copy file",
                    path: entry_from,
                    source,
                })?;
            }
            FileKind::Symlink => copy_symlink_target(&entry_from, &entry_to)?,
            FileKind::Other => {}
        }
    }

    Ok(())
}

fn copy_symlink_target(from: &Path, to: &Path) -> Result<()> {
    let target = fs::read_link(from).map_err(|source| WorkspaceError::Io {
        operation: "read symlink for copy",
        path: from.to_path_buf(),
        source,
    })?;
    let target = if target.is_absolute() {
        target
    } else {
        from.parent().unwrap_or_else(|| Path::new(".")).join(target)
    };
    let target_stat = local_stat(&target)?;
    match target_stat.kind {
        FileKind::Directory => copy_dir_recursive(&target, to),
        FileKind::File | FileKind::Symlink => {
            fs::copy(&target, to).map_err(|source| WorkspaceError::Io {
                operation: "copy symlink target",
                path: target,
                source,
            })?;
            Ok(())
        }
        FileKind::Other => Err(WorkspaceError::NotFile { path: target }),
    }
}

fn local_read_file(path: &Path, options: ReadOptions) -> Result<FileRead> {
    let metadata = fs::metadata(path).map_err(|source| WorkspaceError::Io {
        operation: "stat file",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(WorkspaceError::NotFile {
            path: path.to_path_buf(),
        });
    }

    let size = metadata.len();
    let read_len = options.max_bytes.unwrap_or(size).min(size);
    let mut file = File::open(path).map_err(|source| WorkspaceError::Io {
        operation: "open file",
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(read_len.try_into().unwrap_or(0));
    std::io::Read::by_ref(&mut file)
        .take(read_len)
        .read_to_end(&mut bytes)
        .map_err(|source| WorkspaceError::Io {
            operation: "read file",
            path: path.to_path_buf(),
            source,
        })?;

    Ok(FileRead {
        path: path.to_path_buf(),
        bytes,
        size,
        modified: metadata.modified().ok(),
        readonly: metadata.permissions().readonly(),
        truncated: read_len < size,
    })
}

fn local_write_file(path: &Path, bytes: &[u8], options: WriteOptions) -> Result<WriteResult> {
    if let Some(parent) = path.parent()
        && options.create_parent_dirs
    {
        fs::create_dir_all(parent).map_err(|source| WorkspaceError::Io {
            operation: "create parent directories",
            path: parent.to_path_buf(),
            source,
        })?;
    }

    if let Some(expected_modified) = options.expected_modified {
        let modified = fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .map_err(|source| WorkspaceError::Io {
                operation: "stat file before write",
                path: path.to_path_buf(),
                source,
            })?;
        if modified != expected_modified {
            return Err(WorkspaceError::Modified {
                path: path.to_path_buf(),
            });
        }
    }

    let target_path = write_target_for_path(path)?;
    let existing_permissions = match fs::metadata(&target_path) {
        Ok(metadata) if metadata.is_file() => Some(metadata.permissions()),
        Ok(_) => {
            return Err(WorkspaceError::NotFile {
                path: path.to_path_buf(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(WorkspaceError::Io {
                operation: "stat write target",
                path: target_path.clone(),
                source,
            });
        }
    };
    let parent = target_path.parent().ok_or_else(|| WorkspaceError::Io {
        operation: "resolve write parent",
        path: target_path.clone(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent"),
    })?;
    let mut temp = tempfile::Builder::new()
        .prefix(".nucleotide-write-")
        .tempfile_in(parent)
        .map_err(|source| WorkspaceError::Io {
            operation: "create temporary file",
            path: parent.to_path_buf(),
            source,
        })?;
    temp.write_all(bytes)
        .and_then(|_| temp.flush())
        .map_err(|source| WorkspaceError::Io {
            operation: "write temporary file",
            path: target_path.clone(),
            source,
        })?;
    if let Some(permissions) = existing_permissions {
        temp.as_file()
            .set_permissions(permissions)
            .map_err(|source| WorkspaceError::Io {
                operation: "set temporary file permissions",
                path: target_path.clone(),
                source,
            })?;
    }
    temp.as_file()
        .sync_all()
        .map_err(|source| WorkspaceError::Io {
            operation: "sync temporary file",
            path: target_path.clone(),
            source,
        })?;

    let temp_path = temp.into_temp_path();
    fs::rename(&temp_path, &target_path).map_err(|source| WorkspaceError::Io {
        operation: "replace file",
        path: target_path.clone(),
        source,
    })?;

    let metadata = fs::metadata(&target_path).map_err(|source| WorkspaceError::Io {
        operation: "stat written file",
        path: target_path,
        source,
    })?;

    Ok(WriteResult {
        path: path.to_path_buf(),
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn local_file_search(query: FileSearchQuery) -> Result<FileSearchResult> {
    let pattern = query
        .pattern
        .as_ref()
        .map(|pattern| RegexBuilder::new(pattern).case_insensitive(true).build())
        .transpose()?;
    let mut walker = WalkBuilder::new(&query.root);
    walker
        .hidden(!query.hidden)
        .parents(query.parents)
        .ignore(query.ignore)
        .git_ignore(query.git_ignore)
        .git_global(query.git_global)
        .git_exclude(query.git_exclude)
        .follow_links(query.follow_links)
        .add_custom_ignore_filename(".helix/ignore");
    if !query.excluded_relative_prefixes.is_empty() {
        let root = query.root.clone();
        let excluded_relative_prefixes = query.excluded_relative_prefixes.clone();
        walker.filter_entry(move |entry| {
            let relative_path = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            !excluded_relative_prefixes
                .iter()
                .any(|prefix| relative_path.starts_with(prefix))
        });
    }
    if let Some(max_depth) = query.max_depth {
        walker.max_depth(Some(max_depth));
    }

    let mut files = Vec::new();
    let mut truncated = false;
    for entry in walker.build() {
        let entry = entry.map_err(|source| WorkspaceError::Io {
            operation: "walk directory",
            path: query.root.clone(),
            source: std::io::Error::other(source),
        })?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let relative_path = entry
            .path()
            .strip_prefix(&query.root)
            .unwrap_or(entry.path())
            .to_path_buf();
        if relative_path.as_os_str().is_empty() {
            continue;
        }
        if let Some(pattern) = &pattern
            && !pattern.is_match(&relative_path.to_string_lossy())
        {
            continue;
        }
        if files.len() >= query.limit {
            truncated = true;
            break;
        }
        files.push(relative_path);
    }
    files.sort();

    Ok(FileSearchResult {
        root: query.root,
        files,
        truncated,
    })
}

fn local_text_search(query: TextSearchQuery) -> Result<TextSearchResult> {
    let case_insensitive = query.smart_case && !query.pattern.chars().any(char::is_uppercase);
    let pattern = RegexBuilder::new(&query.pattern)
        .case_insensitive(case_insensitive)
        .multi_line(true)
        .build()?;
    let mut walker = WalkBuilder::new(&query.root);
    walker
        .hidden(!query.hidden)
        .parents(query.parents)
        .ignore(query.ignore)
        .git_ignore(query.git_ignore)
        .git_global(query.git_global)
        .git_exclude(query.git_exclude)
        .follow_links(query.follow_links)
        .add_custom_ignore_filename(".helix/ignore");
    for filename in &query.custom_ignore_filenames {
        walker.add_custom_ignore_filename(filename);
    }
    if let Some(max_depth) = query.max_depth {
        walker.max_depth(Some(max_depth));
    }

    let mut matches = Vec::new();
    let mut truncated = false;
    let excluded_relative_paths = query
        .excluded_relative_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    'walk: for entry in walker.build() {
        let entry = entry.map_err(|source| WorkspaceError::Io {
            operation: "walk directory",
            path: query.root.clone(),
            source: std::io::Error::other(source),
        })?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let metadata = fs::metadata(entry.path()).map_err(|source| WorkspaceError::Io {
            operation: "stat search file",
            path: entry.path().to_path_buf(),
            source,
        })?;
        if metadata.len() > query.max_file_bytes {
            continue;
        }

        let Ok(contents) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let relative_path = entry
            .path()
            .strip_prefix(&query.root)
            .unwrap_or(entry.path())
            .to_path_buf();
        if relative_path.as_os_str().is_empty() {
            continue;
        }
        if excluded_relative_paths.contains(&relative_path) {
            continue;
        }

        for (line_index, line_text) in contents.lines().enumerate() {
            for found in pattern.find_iter(line_text) {
                if matches.len() >= query.limit {
                    truncated = true;
                    break 'walk;
                }
                matches.push(TextSearchMatch {
                    relative_path: relative_path.clone(),
                    line_number: line_index + 1,
                    line_text: line_text.to_string(),
                    start: found.start(),
                    end: found.end(),
                });
            }
        }
    }

    Ok(TextSearchResult {
        root: query.root,
        matches,
        truncated,
    })
}

fn local_project_environment(root: &Path) -> Result<ProjectEnvironmentSnapshot> {
    Ok(ProjectEnvironmentSnapshot {
        root: root.to_path_buf(),
        variables: std::env::vars().collect(),
        origin: ProjectEnvironmentOrigin::ProcessBaseline,
        diagnostics: Vec::new(),
    })
}

fn local_run_process(spec: ProcessSpec) -> Result<ProcessOutput> {
    let cwd = spec.cwd.clone();
    let process_env = if spec.inherit_project_environment {
        let mut project_env = local_project_environment(&cwd)?.variables;
        project_env.extend(spec.env.clone());
        project_env
    } else {
        spec.env.clone()
    };

    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if spec.clear_env {
        command.env_clear();
    }
    apply_process_environment(&mut command, &process_env);
    configure_workspace_process(&mut command);

    let mut child = command.spawn().map_err(|source| WorkspaceError::Io {
        operation: "spawn process",
        path: cwd.clone(),
        source,
    })?;

    let output_limit = spec
        .max_output_bytes
        .unwrap_or(DEFAULT_PROCESS_OUTPUT_LIMIT_BYTES);
    let mut stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "spawn process",
            path: cwd.clone(),
            message: "child process stdout was not piped".to_string(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "spawn process",
            path: cwd.clone(),
            message: "child process stderr was not piped".to_string(),
        })?;

    let stdout_thread = std::thread::spawn(move || read_limited(stdout, output_limit));
    let stderr_thread = std::thread::spawn(move || read_limited(stderr, output_limit));
    let input = spec.stdin;
    let stdin_thread = stdin.take().map(|mut stdin| {
        std::thread::spawn(move || match stdin.write_all(&input) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(error),
        })
    });

    let (status, timed_out) = wait_for_process(&mut child, spec.timeout_ms, &cwd)?;

    if let Some(thread) = stdin_thread {
        join_io_thread(thread, "write process stdin", &cwd)?;
    }
    let (stdout, stdout_truncated) = join_io_thread(stdout_thread, "read process stdout", &cwd)?;
    let (stderr, stderr_truncated) = join_io_thread(stderr_thread, "read process stderr", &cwd)?;

    Ok(ProcessOutput {
        status_code: status.code(),
        success: status.success(),
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        timed_out,
    })
}

fn wait_for_process(
    child: &mut Child,
    timeout_ms: Option<u64>,
    path: &Path,
) -> Result<(std::process::ExitStatus, bool)> {
    let Some(timeout_ms) = timeout_ms else {
        return child
            .wait()
            .map(|status| (status, false))
            .map_err(|source| WorkspaceError::Io {
                operation: "wait for process",
                path: path.to_path_buf(),
                source,
            });
    };

    let timeout = Duration::from_millis(timeout_ms);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|source| WorkspaceError::Io {
            operation: "poll process",
            path: path.to_path_buf(),
            source,
        })? {
            return Ok((status, false));
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            kill_timed_out_process(child, path)?;
            return child.wait().map(|status| (status, true)).map_err(|source| {
                WorkspaceError::Io {
                    operation: "wait for killed process",
                    path: path.to_path_buf(),
                    source,
                }
            });
        }

        let remaining = timeout.saturating_sub(elapsed);
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

fn apply_process_environment(command: &mut Command, environment: &BTreeMap<String, String>) {
    for (key, value) in environment {
        if process_environment_entry_is_valid(key, value) {
            command.env(key, value);
        }
    }
}

fn process_environment_entry_is_valid(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains(['=', '\0']) && !value.contains('\0')
}

#[cfg(unix)]
fn configure_workspace_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_workspace_process(_command: &mut Command) {}

fn kill_timed_out_process(child: &mut Child, path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        if kill_process_group(child.id()).is_ok() {
            return Ok(());
        }
    }

    child.kill().map_err(|source| WorkspaceError::Io {
        operation: "kill timed out process",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn kill_process_group(process_id: u32) -> std::io::Result<()> {
    let status = Command::new("kill")
        .arg("-KILL")
        .arg(format!("-{process_id}"))
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "kill process group exited with {status}"
        )))
    }
}

fn read_limited<R: Read>(mut reader: R, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        if remaining >= read {
            output.extend_from_slice(&buffer[..read]);
        } else {
            output.extend_from_slice(&buffer[..remaining]);
            truncated = true;
        }
        if remaining < read {
            truncated = true;
        }
    }

    Ok((output, truncated))
}

fn join_io_thread<T>(
    thread: std::thread::JoinHandle<std::io::Result<T>>,
    operation: &'static str,
    path: &Path,
) -> Result<T> {
    thread
        .join()
        .map_err(|_| WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: "I/O thread panicked".to_string(),
        })?
        .map_err(|source| WorkspaceError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })
}

fn local_git_head(root: &Path) -> Result<GitHeadResult> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|source| WorkspaceError::Io {
            operation: "run git rev-parse",
            path: root.to_path_buf(),
            source,
        })?;

    if !output.status.success() {
        return Ok(GitHeadResult {
            root: root.to_path_buf(),
            head: None,
        });
    }

    let head = std::str::from_utf8(&output.stdout)
        .ok()
        .map(str::trim)
        .filter(|head| !head.is_empty())
        .map(ToOwned::to_owned);

    Ok(GitHeadResult {
        root: root.to_path_buf(),
        head,
    })
}

fn local_git_status(root: &Path, options: GitStatusOptions) -> Result<GitStatusResult> {
    let mut command = Command::new("git");
    command
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(root);
    if options.include_untracked {
        command.arg("--untracked-files=all");
    } else {
        command.arg("--untracked-files=no");
    }

    let output = command.output().map_err(|source| WorkspaceError::Io {
        operation: "run git status",
        path: root.to_path_buf(),
        source,
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if git_error_is_not_repository(&stderr) {
            return Ok(GitStatusResult {
                root: root.to_path_buf(),
                entries: Vec::new(),
                truncated: false,
            });
        }

        let message = if stderr.is_empty() {
            format!("git exited with status {}", output.status)
        } else {
            format!("git exited with status {}: {stderr}", output.status)
        };
        return Err(WorkspaceError::CommandFailed {
            operation: "git status",
            path: root.to_path_buf(),
            message,
        });
    }

    Ok(parse_git_status_output(root, &output.stdout, options.limit))
}

fn git_error_is_not_repository(message: &str) -> bool {
    message.contains("not a git repository")
}

fn parse_git_status_output(root: &Path, output: &[u8], limit: usize) -> GitStatusResult {
    let mut entries = Vec::new();
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut truncated = false;

    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            continue;
        }

        let index = field[0];
        let worktree = field[1];
        let relative_path = path_from_git_bytes(&field[3..]);
        let original_relative_path = if matches!(index, b'R' | b'C') {
            fields.next().map(path_from_git_bytes)
        } else {
            None
        };

        if entries.len() >= limit {
            truncated = true;
            break;
        }

        entries.push(GitStatusEntry {
            relative_path,
            original_relative_path,
            index_status: git_status_kind(index, worktree),
            working_tree_status: git_status_kind(worktree, index),
        });
    }

    GitStatusResult {
        root: root.to_path_buf(),
        entries,
        truncated,
    }
}

fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

fn git_status_kind(status: u8, other: u8) -> GitStatusKind {
    if is_conflict_pair(status, other) {
        return GitStatusKind::Conflicted;
    }

    match status {
        b' ' => GitStatusKind::Unmodified,
        b'M' => GitStatusKind::Modified,
        b'A' => GitStatusKind::Added,
        b'D' => GitStatusKind::Deleted,
        b'R' => GitStatusKind::Renamed,
        b'C' => GitStatusKind::Copied,
        b'T' => GitStatusKind::TypeChanged,
        b'?' => GitStatusKind::Untracked,
        b'U' => GitStatusKind::Conflicted,
        _ => GitStatusKind::Unknown,
    }
}

fn is_conflict_pair(left: u8, right: u8) -> bool {
    matches!(
        (left, right),
        (b'D', b'D')
            | (b'A', b'U')
            | (b'U', b'D')
            | (b'U', b'A')
            | (b'D', b'U')
            | (b'A', b'A')
            | (b'U', b'U')
    )
}

fn file_stat_from_metadata(path: PathBuf, metadata: fs::Metadata) -> FileStat {
    let file_type = metadata.file_type();
    let kind = if file_type.is_file() {
        FileKind::File
    } else if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    };

    FileStat {
        path,
        kind,
        size: metadata.len(),
        modified: metadata.modified().ok(),
        readonly: metadata.permissions().readonly(),
    }
}

fn write_target_for_path(path: &Path) -> Result<PathBuf> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(path.to_path_buf());
        }
        Err(source) => {
            return Err(WorkspaceError::Io {
                operation: "stat write target",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if !metadata.file_type().is_symlink() {
        return Ok(path.to_path_buf());
    }

    let target = fs::read_link(path).map_err(|source| WorkspaceError::Io {
        operation: "read write symlink",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or_else(|| Path::new(".")).join(target)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    #[test]
    fn local_workspace_backend_handle_identifies_as_local() {
        let backend = local_workspace_backend();

        assert_eq!(backend.identity(), WorkspaceIdentity::Local);
    }

    #[cfg(unix)]
    #[test]
    fn local_backend_run_process_collects_output_and_status() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;

        let output = block_on(backend.run_process(ProcessSpec {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "dir=$(pwd -P); printf '%s:%s:' \"$FOO\" \"$dir\"; cat".to_string(),
            ],
            cwd: temp.path().to_path_buf(),
            env: BTreeMap::from([("FOO".to_string(), "bar".to_string())]),
            clear_env: false,
            inherit_project_environment: false,
            stdin: b"stdin".to_vec(),
            max_output_bytes: None,
            timeout_ms: None,
        }))
        .unwrap();

        assert!(output.success);
        assert_eq!(output.status_code, Some(0));
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "bar:{}:stdin",
                temp.path().canonicalize().unwrap().display()
            )
        );
        assert_eq!(output.stderr, Vec::<u8>::new());
        assert!(!output.stdout_truncated);
        assert!(!output.stderr_truncated);
        assert!(!output.timed_out);
    }

    #[test]
    fn process_environment_validation_rejects_invalid_entries() {
        assert!(process_environment_entry_is_valid("GOOD", "value"));
        assert!(!process_environment_entry_is_valid("", "value"));
        assert!(!process_environment_entry_is_valid("BAD=KEY", "value"));
        assert!(!process_environment_entry_is_valid("BAD\0KEY", "value"));
        assert!(!process_environment_entry_is_valid(
            "BAD_VALUE",
            "bad\0value"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn local_backend_run_process_ignores_invalid_environment_entries() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;

        let output = block_on(backend.run_process(ProcessSpec {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf '%s:%s' \"$GOOD\" \"${BAD_VALUE-unset}\"".to_string(),
            ],
            cwd: temp.path().to_path_buf(),
            env: BTreeMap::from([
                ("GOOD".to_string(), "yes".to_string()),
                ("BAD=KEY".to_string(), "ignored".to_string()),
                ("BAD\0KEY".to_string(), "ignored".to_string()),
                ("BAD_VALUE".to_string(), "bad\0value".to_string()),
            ]),
            clear_env: true,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: None,
            timeout_ms: None,
        }))
        .unwrap();

        assert!(output.success);
        assert_eq!(output.stdout, b"yes:unset");
    }

    #[cfg(unix)]
    #[test]
    fn local_backend_run_process_can_inherit_project_environment() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;
        let expected_path = std::env::var("PATH").unwrap_or_else(|_| "unset".to_string());

        let output = block_on(backend.run_process(ProcessSpec {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf '%s:%s' \"${PATH-unset}\" \"$FOO\"".to_string(),
            ],
            cwd: temp.path().to_path_buf(),
            env: BTreeMap::from([("FOO".to_string(), "bar".to_string())]),
            clear_env: true,
            inherit_project_environment: true,
            stdin: Vec::new(),
            max_output_bytes: None,
            timeout_ms: None,
        }))
        .unwrap();

        assert!(output.success);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{expected_path}:bar")
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_backend_run_process_truncates_stored_output_after_limit() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;

        let output = block_on(backend.run_process(ProcessSpec {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf abcdef; printf ghij >&2".to_string(),
            ],
            cwd: temp.path().to_path_buf(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: Some(3),
            timeout_ms: None,
        }))
        .unwrap();

        assert!(output.success);
        assert_eq!(output.stdout, b"abc");
        assert_eq!(output.stderr, b"ghi");
        assert!(output.stdout_truncated);
        assert!(output.stderr_truncated);
        assert!(!output.timed_out);
    }

    #[cfg(unix)]
    #[test]
    fn local_backend_run_process_kills_timed_out_child() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;

        let output = block_on(backend.run_process(ProcessSpec {
            program: "tail".to_string(),
            args: vec!["-f".to_string(), "/dev/null".to_string()],
            cwd: temp.path().to_path_buf(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: None,
            timeout_ms: Some(20),
        }))
        .unwrap();

        assert!(!output.success);
        assert!(output.timed_out);
    }

    #[cfg(unix)]
    #[test]
    fn local_backend_run_process_kills_timed_out_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;
        let started = Instant::now();

        let output = block_on(backend.run_process(ProcessSpec {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "sleep 2 & wait".to_string()],
            cwd: temp.path().to_path_buf(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: None,
            timeout_ms: Some(20),
        }))
        .unwrap();

        assert!(!output.success);
        assert!(output.timed_out);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timed-out descendant kept process pipes open"
        );
    }

    #[test]
    fn workspace_location_classifies_wsl_localhost_unc_without_probing() {
        let path = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Wsl {
                original_path: path,
                distro: "Ubuntu-24.04".to_string(),
                linux_path: PathBuf::from("/home/me/project"),
            }
        );
    }

    #[test]
    fn workspace_location_classifies_legacy_wsl_unc_without_probing() {
        let path = PathBuf::from(r"\\wsl$\Debian\var\www");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Wsl {
                original_path: path,
                distro: "Debian".to_string(),
                linux_path: PathBuf::from("/var/www"),
            }
        );
    }

    #[test]
    fn workspace_location_classifies_wsl_unc_prefix_case_insensitively() {
        let path = PathBuf::from(r"\\WSL.LocalHost\Ubuntu-24.04\home\me\project");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Wsl {
                original_path: path,
                distro: "Ubuntu-24.04".to_string(),
                linux_path: PathBuf::from("/home/me/project"),
            }
        );
    }

    #[test]
    fn workspace_location_classifies_ssh_uri_without_probing() {
        let path = PathBuf::from("ssh://me@example.com:2222/home/me/project");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Ssh {
                original_path: path,
                target: SshWorkspaceTarget {
                    host: "example.com".to_string(),
                    user: Some("me".to_string()),
                    port: Some(2222),
                },
                path: PathBuf::from("/home/me/project"),
            }
        );
    }

    #[test]
    fn workspace_location_classifies_ssh_uri_scheme_case_insensitively() {
        let path = PathBuf::from("SSH://me@example.com:2222/home/me/project");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Ssh {
                original_path: path,
                target: SshWorkspaceTarget {
                    host: "example.com".to_string(),
                    user: Some("me".to_string()),
                    port: Some(2222),
                },
                path: PathBuf::from("/home/me/project"),
            }
        );
    }

    #[test]
    fn workspace_location_classifies_ssh_uri_with_bracketed_ipv6_host() {
        let path = PathBuf::from("ssh://me@[2001:db8::1]:2222/home/me/project");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Ssh {
                original_path: path,
                target: SshWorkspaceTarget {
                    host: "2001:db8::1".to_string(),
                    user: Some("me".to_string()),
                    port: Some(2222),
                },
                path: PathBuf::from("/home/me/project"),
            }
        );
    }

    #[test]
    fn workspace_location_classifies_ssh_uri_with_unbracketed_ipv6_host_without_port() {
        let path = PathBuf::from("ssh://2001:db8::1/home/me/project");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Ssh {
                original_path: path,
                target: SshWorkspaceTarget {
                    host: "2001:db8::1".to_string(),
                    user: None,
                    port: None,
                },
                path: PathBuf::from("/home/me/project"),
            }
        );
    }

    #[test]
    fn workspace_location_decodes_ssh_uri_path_escapes() {
        let path = PathBuf::from("ssh://me@example.com/home/me/Project%20One/%E2%9C%93");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Ssh {
                original_path: path,
                target: SshWorkspaceTarget {
                    host: "example.com".to_string(),
                    user: Some("me".to_string()),
                    port: None,
                },
                path: PathBuf::from("/home/me/Project One/\u{2713}"),
            }
        );
    }

    #[test]
    fn workspace_location_classifies_local_paths_as_local() {
        let path = PathBuf::from("/tmp/project");

        assert_eq!(
            classify_workspace_location(&path),
            WorkspaceLocation::Local { path }
        );
    }

    #[test]
    fn workspace_location_exposes_display_to_native_mapping() {
        let path = PathBuf::from("ssh://me@example.com/home/me/project");
        let location = classify_workspace_location(&path);

        assert!(location.is_remote());
        assert_eq!(location.display_root(), path.as_path());
        assert_eq!(location.native_root(), Path::new("/home/me/project"));
        assert_eq!(
            location
                .path_mapping()
                .to_native_path(&path.join("src").join("main.rs")),
            PathBuf::from("/home/me/project/src/main.rs")
        );
    }

    #[test]
    fn workspace_location_maps_external_ssh_native_path_to_same_target_display_path() {
        let path = PathBuf::from("ssh://me@example.com:2222/home/me/project");
        let location = classify_workspace_location(&path);

        assert_eq!(
            location.display_path_for_native_path(Path::new("/nix/store/rust source/lib.rs")),
            PathBuf::from("ssh://me@example.com:2222/nix/store/rust%20source/lib.rs")
        );
    }

    #[test]
    fn workspace_location_normalizes_mixed_ssh_native_separators_for_display_path() {
        let path = PathBuf::from("ssh://me@example.com:2222/home/me/project");
        let location = classify_workspace_location(&path);

        assert_eq!(
            location.display_path_for_native_path(Path::new(r"/home\me/project\src/lib.rs")),
            PathBuf::from("ssh://me@example.com:2222/home/me/project/src/lib.rs")
        );
    }

    #[test]
    fn ssh_display_path_encodes_native_path_for_target() {
        let target = SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: Some(2222),
        };

        assert_eq!(
            ssh_display_path(&target, Path::new("/home/me/Project One/src/lib.rs")),
            PathBuf::from("ssh://me@example.com:2222/home/me/Project%20One/src/lib.rs")
        );
    }

    #[test]
    fn ssh_display_path_normalizes_mixed_native_separators() {
        let target = SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: Some(2222),
        };

        assert_eq!(
            ssh_display_path(&target, Path::new(r"/home\me/Project One\src/lib.rs")),
            PathBuf::from("ssh://me@example.com:2222/home/me/Project%20One/src/lib.rs")
        );
    }

    #[test]
    fn workspace_location_maps_external_wsl_native_path_to_same_distro_display_path() {
        let path = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project");
        let location = classify_workspace_location(&path);

        assert_eq!(
            location.display_path_for_native_path(Path::new("/nix/store/rust/lib.rs")),
            PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\nix\store\rust\lib.rs")
        );
    }

    #[test]
    fn wsl_display_path_normalizes_mixed_linux_separators() {
        assert_eq!(
            wsl_display_path("Ubuntu-24.04", Path::new(r"/home\me/project\src")),
            PathBuf::from("//wsl.localhost/Ubuntu-24.04/home/me/project/src")
        );
    }

    #[test]
    fn wsl_display_path_uses_localhost_unc_shape() {
        assert_eq!(
            wsl_display_path("Ubuntu-24.04", Path::new("/home/me/project")),
            PathBuf::from("//wsl.localhost/Ubuntu-24.04/home/me/project")
        );
    }

    #[test]
    fn workspace_mapping_maps_external_same_remote_paths() {
        let mapping =
            WorkspacePathMapping::new("ssh://me@example.com/home/me/project", "/home/me/project");

        assert_eq!(
            mapping.to_display_path(Path::new("/home/me/.cargo/git/checkout/lib.rs")),
            PathBuf::from("ssh://me@example.com/home/me/.cargo/git/checkout/lib.rs")
        );
        assert_eq!(
            mapping.to_native_path(Path::new(
                "ssh://me@example.com/home/me/.cargo/git/checkout/lib.rs"
            )),
            PathBuf::from("/home/me/.cargo/git/checkout/lib.rs")
        );
    }

    #[test]
    fn workspace_mapping_maps_windows_joined_ssh_display_path_to_native_path() {
        let mapping =
            WorkspacePathMapping::new("ssh://me@example.com/home/me/project", "/home/me/project");

        assert_eq!(
            mapping.to_native_path(Path::new(
                r"ssh://me@example.com/home/me/project\src\main.rs"
            )),
            PathBuf::from("/home/me/project/src/main.rs")
        );
    }

    #[test]
    fn workspace_mapping_preserves_ssh_display_separator_style() {
        let mapping =
            WorkspacePathMapping::new("ssh://me@example.com/home/me/project", "/home/me/project");

        assert_eq!(
            mapping.to_display_path(Path::new("/home/me/project/src/main.rs")),
            PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs")
        );
    }

    #[test]
    fn workspace_mapping_preserves_wsl_unc_display_separator_style() {
        let mapping = WorkspacePathMapping::new(
            r"\\wsl.localhost\Ubuntu\home\me\project",
            "/home/me/project",
        );

        assert_eq!(
            mapping.to_display_path(Path::new("/home/me/project/src/main.rs")),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project\src\main.rs")
        );
    }

    #[test]
    fn workspace_path_is_absolute_accepts_remote_display_paths() {
        assert!(workspace_path_is_absolute(Path::new(
            "ssh://me@example.com/home/me/project"
        )));
        assert!(workspace_path_is_absolute(Path::new(
            r"\\wsl.localhost\Ubuntu\home\me\project"
        )));
        assert!(workspace_path_is_absolute(Path::new("/tmp/project")));
        assert!(!workspace_path_is_absolute(Path::new("src/main.rs")));
    }

    #[test]
    fn absolutize_workspace_path_keeps_remote_display_paths_rooted() {
        let root = Path::new("ssh://me@example.com/home/me/project");
        let rooted_file = PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs");
        let relative_file = Path::new("src/main.rs");

        assert_eq!(absolutize_workspace_path(root, &rooted_file), rooted_file);
        assert_eq!(
            absolutize_workspace_path(root, relative_file),
            PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs")
        );
    }

    #[test]
    fn remote_startup_workspace_root_uses_parent_for_wsl_file_hint() {
        let path = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project\src\main.rs");

        assert_eq!(
            remote_startup_workspace_root(&path),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu-24.04\home\me\project\src"
            ))
        );
    }

    #[test]
    fn remote_path_is_probably_file_uses_wsl_file_hint() {
        let path = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project\src\main.rs");

        assert_eq!(remote_path_is_probably_file(&path), Some(true));
    }

    #[test]
    fn remote_path_is_probably_file_uses_ssh_directory_hint() {
        let path = PathBuf::from("ssh://me@example.com/home/me/project/");

        assert_eq!(remote_path_is_probably_file(&path), Some(false));
    }

    #[test]
    fn remote_path_is_probably_file_ignores_local_paths() {
        assert_eq!(
            remote_path_is_probably_file(Path::new("/tmp/project/src/main.rs")),
            None
        );
    }

    #[test]
    fn remote_startup_workspace_root_keeps_wsl_directory_hint() {
        let path = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\project");

        assert_eq!(
            remote_startup_workspace_root(&path),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu-24.04\home\me\project"
            ))
        );
    }

    #[test]
    fn remote_startup_workspace_root_uses_parent_for_ssh_file_hint() {
        let path = PathBuf::from("ssh://me@example.com:2222/home/me/project/src/main.rs");

        assert_eq!(
            remote_startup_workspace_root(&path),
            Some(PathBuf::from(
                "ssh://me@example.com:2222/home/me/project/src"
            ))
        );
    }

    #[test]
    fn remote_startup_workspace_root_encodes_ssh_display_parent() {
        let path = PathBuf::from("ssh://me@example.com/home/me/Project%20One/src/main.rs");

        assert_eq!(
            remote_startup_workspace_root(&path),
            Some(PathBuf::from(
                "ssh://me@example.com/home/me/Project%20One/src"
            ))
        );
    }

    #[test]
    fn remote_startup_workspace_root_uses_parent_for_known_extensionless_files() {
        let path = PathBuf::from("ssh://example.com/home/me/project/Makefile");

        assert_eq!(
            remote_startup_workspace_root(&path),
            Some(PathBuf::from("ssh://example.com/home/me/project"))
        );
    }

    #[test]
    fn remote_startup_workspace_root_keeps_ssh_directory_hint() {
        let path = PathBuf::from("ssh://me@example.com/home/me/project");

        assert_eq!(
            remote_startup_workspace_root(&path),
            Some(PathBuf::from("ssh://me@example.com/home/me/project"))
        );
    }

    #[test]
    fn remote_startup_workspace_root_keeps_trailing_slash_directory_hint() {
        let path = PathBuf::from("ssh://me@example.com/home/me/project.v1/");

        assert_eq!(
            remote_startup_workspace_root(&path),
            Some(PathBuf::from("ssh://me@example.com/home/me/project.v1"))
        );
    }

    #[test]
    fn remote_startup_workspace_root_ignores_local_paths() {
        assert_eq!(
            remote_startup_workspace_root(Path::new("/tmp/project/src/main.rs")),
            None
        );
    }

    #[test]
    fn path_mapped_backend_lists_native_directory_as_display_paths() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("README.md"), "").unwrap();
        let display_root = PathBuf::from("/remote/project");
        let backend = path_mapped_workspace_backend(
            local_workspace_backend(),
            WorkspacePathMapping::new(display_root.clone(), temp.path()),
        );

        let listing = block_on(backend.list_dir(&display_root)).unwrap();

        assert_eq!(listing.path, display_root);
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec![display_root.join("README.md"), display_root.join("src")]
        );
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|entry| entry.stat.path.clone())
                .collect::<Vec<_>>(),
            vec![display_root.join("README.md"), display_root.join("src")]
        );
    }

    #[test]
    fn path_mapped_backend_reads_and_writes_display_paths() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        let display_root = PathBuf::from("/remote/project");
        let display_path = display_root.join("src").join("main.rs");
        let native_path = temp.path().join("src").join("main.rs");
        let backend = path_mapped_workspace_backend(
            local_workspace_backend(),
            WorkspacePathMapping::new(display_root.clone(), temp.path()),
        );

        let write =
            block_on(backend.write_file(&display_path, b"fn main() {}\n", WriteOptions::default()))
                .unwrap();
        let read = block_on(backend.read_file(&display_path, ReadOptions::default())).unwrap();

        assert_eq!(write.path, display_path);
        assert_eq!(read.path, display_path);
        assert_eq!(read.bytes, b"fn main() {}\n");
        assert_eq!(
            std::fs::read_to_string(native_path).unwrap(),
            "fn main() {}\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_mapped_backend_runs_process_in_native_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let native_src = temp.path().join("src");
        fs::create_dir(&native_src).unwrap();
        let display_root = PathBuf::from("/remote/project");
        let backend = path_mapped_workspace_backend(
            local_workspace_backend(),
            WorkspacePathMapping::new(display_root.clone(), temp.path()),
        );

        let output = block_on(backend.run_process(ProcessSpec {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "pwd -P".to_string()],
            cwd: display_root.join("src"),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: None,
            timeout_ms: None,
        }))
        .unwrap();

        assert!(output.success);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            native_src.canonicalize().unwrap().display().to_string()
        );
    }

    #[test]
    fn local_backend_file_operations_return_affected_stats() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;
        let file = temp.path().join("src").join("main.rs");
        let renamed = temp.path().join("src").join("lib.rs");
        let copied = temp.path().join("src").join("lib-copy.rs");
        let dir = temp.path().join("src").join("nested");

        let created_file = block_on(backend.create_file(&file)).unwrap();
        let created_dir = block_on(backend.create_dir(&dir)).unwrap();
        fs::write(&file, "fn main() {}\n").unwrap();
        let renamed_stat = block_on(backend.rename_path(&file, &renamed)).unwrap();
        let copied_stat = block_on(backend.copy_path(&renamed, &copied)).unwrap();
        let deleted_file = block_on(backend.delete_path(&renamed)).unwrap();
        let deleted_dir = block_on(backend.delete_path(&dir)).unwrap();

        assert_eq!(created_file.path, file);
        assert_eq!(created_file.kind, FileKind::File);
        assert_eq!(created_dir.path, dir);
        assert_eq!(created_dir.kind, FileKind::Directory);
        assert_eq!(renamed_stat.path, renamed);
        assert_eq!(copied_stat.path, copied);
        assert_eq!(deleted_file.path, renamed);
        assert_eq!(deleted_file.kind, FileKind::File);
        assert_eq!(deleted_dir.path, dir);
        assert_eq!(deleted_dir.kind, FileKind::Directory);
        assert!(!renamed.exists());
        assert!(!dir.exists());
        assert_eq!(fs::read_to_string(copied).unwrap(), "fn main() {}\n");
    }

    #[test]
    fn local_backend_supports_case_only_rename() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;
        let file = temp.path().join("readme.md");
        let renamed = temp.path().join("README.md");

        fs::write(&file, "hello\n").unwrap();

        let renamed_stat = block_on(backend.rename_path(&file, &renamed)).unwrap();

        assert_eq!(renamed_stat.path, renamed);
        assert_eq!(renamed_stat.kind, FileKind::File);
        assert!(renamed.exists());
        assert_eq!(fs::read_to_string(renamed).unwrap(), "hello\n");
    }

    #[test]
    fn local_backend_rejects_copying_directory_into_itself() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;
        let source = temp.path().join("source");
        let descendant = source.join("nested").join("copy");

        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file.txt"), "hello\n").unwrap();

        let result = block_on(backend.copy_path(&source, &descendant));

        assert!(matches!(result, Err(WorkspaceError::CommandFailed { .. })));
        assert!(!descendant.exists());
    }

    #[test]
    fn local_backend_finds_ancestor_file() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;
        let manifest = temp.path().join("Cargo.toml");
        let file = temp.path().join("src").join("bin").join("main.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&manifest, "[package]\n").unwrap();
        fs::write(&file, "fn main() {}\n").unwrap();

        let found = block_on(backend.find_ancestor_file(&file, "Cargo.toml", 8)).unwrap();

        assert_eq!(found, Some(manifest));
    }

    #[test]
    fn path_mapped_backend_maps_file_operations_to_display_paths() {
        let temp = tempfile::tempdir().unwrap();
        let display_root = PathBuf::from("/remote/project");
        let display_src = display_root.join("src");
        let display_file = display_src.join("main.rs");
        let display_renamed = display_src.join("lib.rs");
        let native_renamed = temp.path().join("src").join("lib.rs");
        let backend = path_mapped_workspace_backend(
            local_workspace_backend(),
            WorkspacePathMapping::new(display_root.clone(), temp.path()),
        );

        let dir = block_on(backend.create_dir(&display_src)).unwrap();
        let file = block_on(backend.create_file(&display_file)).unwrap();
        let renamed = block_on(backend.rename_path(&display_file, &display_renamed)).unwrap();

        assert_eq!(dir.path, display_src);
        assert_eq!(file.path, display_file);
        assert_eq!(renamed.path, display_renamed);
        assert!(native_renamed.exists());
    }

    #[test]
    fn path_mapped_backend_maps_ancestor_file_to_display_path() {
        let temp = tempfile::tempdir().unwrap();
        let native_manifest = temp.path().join("Cargo.toml");
        let native_file = temp.path().join("src").join("main.rs");
        fs::create_dir_all(native_file.parent().unwrap()).unwrap();
        fs::write(&native_manifest, "[package]\n").unwrap();
        fs::write(&native_file, "fn main() {}\n").unwrap();

        let display_root = PathBuf::from("/remote/project");
        let display_file = display_root.join("src").join("main.rs");
        let display_manifest = display_root.join("Cargo.toml");
        let backend = path_mapped_workspace_backend(
            local_workspace_backend(),
            WorkspacePathMapping::new(display_root, temp.path()),
        );

        let found = block_on(backend.find_ancestor_file(&display_file, "Cargo.toml", 8)).unwrap();

        assert_eq!(found, Some(display_manifest));
    }

    #[test]
    fn path_mapped_backend_keeps_search_paths_display_rooted() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("main.rs"), "needle\n").unwrap();
        let display_root = PathBuf::from("/remote/project");
        let backend = path_mapped_workspace_backend(
            local_workspace_backend(),
            WorkspacePathMapping::new(display_root.clone(), temp.path()),
        );

        let files = block_on(backend.file_search(FileSearchQuery {
            root: display_root.clone(),
            pattern: None,
            limit: 10,
            ..FileSearchQuery::default()
        }))
        .unwrap();
        let matches = block_on(backend.text_search(TextSearchQuery {
            root: display_root.clone(),
            pattern: "needle".to_string(),
            limit: 10,
            ..TextSearchQuery::default()
        }))
        .unwrap();

        assert_eq!(files.root, display_root);
        assert_eq!(files.files, vec![PathBuf::from("src/main.rs")]);
        assert_eq!(matches.root, display_root);
        assert_eq!(
            matches.matches[0].relative_path,
            PathBuf::from("src/main.rs")
        );
    }

    #[test]
    fn local_backend_lists_directory_entries_sorted_by_name() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("b.rs"), "").unwrap();
        fs::write(temp.path().join("A.rs"), "").unwrap();

        let backend = LocalWorkspaceBackend;
        let listing = block_on(backend.list_dir(temp.path())).unwrap();

        let names = listing
            .entries
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["A.rs", "b.rs"]);
    }

    #[test]
    fn local_backend_reads_bounded_file_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("main.rs");
        fs::write(&path, "abcdef").unwrap();

        let backend = LocalWorkspaceBackend;
        let read = block_on(backend.read_file(&path, ReadOptions { max_bytes: Some(3) })).unwrap();

        assert_eq!(read.bytes, b"abc");
        assert_eq!(read.size, 6);
        assert!(read.truncated);
    }

    #[test]
    fn local_backend_rejects_external_modification_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("main.rs");
        fs::write(&path, "old").unwrap();

        let backend = LocalWorkspaceBackend;
        let result = block_on(backend.write_file(
            &path,
            b"new",
            WriteOptions {
                create_parent_dirs: false,
                expected_modified: Some(SystemTime::UNIX_EPOCH),
            },
        ));

        assert!(matches!(result, Err(WorkspaceError::Modified { .. })));
        assert_eq!(fs::read_to_string(path).unwrap(), "old");
    }

    #[test]
    fn local_backend_writes_file_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("main.rs");

        let backend = LocalWorkspaceBackend;
        let result = block_on(backend.write_file(
            &path,
            b"fn main() {}\n",
            WriteOptions {
                create_parent_dirs: false,
                expected_modified: None,
            },
        ))
        .unwrap();

        assert_eq!(result.path, path);
        assert_eq!(result.size, 13);
        assert_eq!(fs::read_to_string(result.path).unwrap(), "fn main() {}\n");
    }

    #[cfg(unix)]
    #[test]
    fn local_backend_preserves_existing_file_permissions_on_write() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("script.sh");
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();

        let backend = LocalWorkspaceBackend;
        block_on(backend.write_file(&path, b"#!/bin/sh\nexit 1\n", WriteOptions::default()))
            .unwrap();

        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn local_backend_preserves_symlink_on_write() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.rs");
        let link = temp.path().join("link.rs");
        fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let backend = LocalWorkspaceBackend;
        block_on(backend.write_file(&link, b"new", WriteOptions::default())).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(target).unwrap(), "new");
    }

    #[test]
    fn local_backend_search_respects_limit_and_pattern() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("main.rs"), "").unwrap();
        fs::write(temp.path().join("src").join("lib.rs"), "").unwrap();
        fs::write(temp.path().join("README.md"), "").unwrap();

        let backend = LocalWorkspaceBackend;
        let result = block_on(backend.file_search(FileSearchQuery {
            root: temp.path().to_path_buf(),
            pattern: Some(r"\.rs$".to_string()),
            limit: 1,
            ..FileSearchQuery::default()
        }))
        .unwrap();

        assert_eq!(result.files.len(), 1);
        assert!(result.truncated);
        assert!(result.files[0].to_string_lossy().ends_with(".rs"));
    }

    #[test]
    fn local_backend_file_search_excludes_relative_prefixes_before_limit() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("skip")).unwrap();
        fs::write(temp.path().join("skip").join("a.rs"), "").unwrap();
        fs::write(temp.path().join("skip").join("b.rs"), "").unwrap();
        fs::write(temp.path().join("main.rs"), "").unwrap();

        let backend = LocalWorkspaceBackend;
        let result = block_on(backend.file_search(FileSearchQuery {
            root: temp.path().to_path_buf(),
            limit: 1,
            excluded_relative_prefixes: vec![PathBuf::from("skip")],
            ..FileSearchQuery::default()
        }))
        .unwrap();

        assert_eq!(result.files, vec![PathBuf::from("main.rs")]);
        assert!(!result.truncated);
    }

    #[test]
    fn local_backend_text_search_respects_limit_and_smart_case() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("main.rs"), "Needle\nneedle\n").unwrap();
        fs::write(temp.path().join("README.md"), "needle\n").unwrap();

        let backend = LocalWorkspaceBackend;
        let smart_case_result = block_on(backend.text_search(TextSearchQuery {
            root: temp.path().to_path_buf(),
            pattern: "Needle".to_string(),
            limit: 10,
            smart_case: true,
            ..TextSearchQuery::default()
        }))
        .unwrap();

        assert_eq!(smart_case_result.matches.len(), 1);
        assert!(!smart_case_result.truncated);
        assert_eq!(smart_case_result.matches[0].line_text, "Needle");
        assert_eq!(smart_case_result.matches[0].line_number, 1);

        let limited_result = block_on(backend.text_search(TextSearchQuery {
            root: temp.path().to_path_buf(),
            pattern: "needle".to_string(),
            limit: 1,
            smart_case: true,
            ..TextSearchQuery::default()
        }))
        .unwrap();

        assert_eq!(limited_result.matches.len(), 1);
        assert!(limited_result.truncated);
    }

    #[test]
    fn local_backend_text_search_excludes_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("main.rs"), "needle\n").unwrap();
        fs::write(temp.path().join("README.md"), "needle\n").unwrap();

        let backend = LocalWorkspaceBackend;
        let result = block_on(backend.text_search(TextSearchQuery {
            root: temp.path().to_path_buf(),
            pattern: "needle".to_string(),
            limit: 10,
            excluded_relative_paths: vec![PathBuf::from("src/main.rs")],
            ..TextSearchQuery::default()
        }))
        .unwrap();

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].relative_path, PathBuf::from("README.md"));
    }

    #[test]
    fn local_backend_text_search_uses_custom_ignore_filenames() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(".nucleotide-ignore"), "ignored.txt\n").unwrap();
        fs::write(temp.path().join("ignored.txt"), "needle\n").unwrap();
        fs::write(temp.path().join("visible.txt"), "needle\n").unwrap();

        let backend = LocalWorkspaceBackend;
        let result = block_on(backend.text_search(TextSearchQuery {
            root: temp.path().to_path_buf(),
            pattern: "needle".to_string(),
            limit: 10,
            custom_ignore_filenames: vec![temp.path().join(".nucleotide-ignore")],
            ..TextSearchQuery::default()
        }))
        .unwrap();

        assert_eq!(result.matches.len(), 1);
        assert_eq!(
            result.matches[0].relative_path,
            PathBuf::from("visible.txt")
        );
    }

    #[test]
    fn local_backend_project_environment_returns_process_baseline() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;
        let snapshot = block_on(backend.project_environment(temp.path())).unwrap();

        assert_eq!(snapshot.root, temp.path());
        assert_eq!(snapshot.origin, ProjectEnvironmentOrigin::ProcessBaseline);
        assert!(snapshot.diagnostics.is_empty());
    }

    #[test]
    fn local_backend_git_head_returns_current_commit() {
        let temp = tempfile::tempdir().unwrap();
        init_git_repo(temp.path());
        fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        let expected = git_output(temp.path(), &["rev-parse", "--verify", "HEAD"]);
        let backend = LocalWorkspaceBackend;
        let head = block_on(backend.git_head(temp.path())).unwrap();

        assert_eq!(head.root, temp.path());
        assert_eq!(head.head, Some(expected.trim().to_string()));
    }

    #[test]
    fn local_backend_git_status_returns_structured_entries() {
        let temp = tempfile::tempdir().unwrap();
        init_git_repo(temp.path());
        fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        fs::write(temp.path().join("move-me.txt"), "move\n").unwrap();
        run_git(temp.path(), &["add", "tracked.txt", "move-me.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("tracked.txt"), "changed\n").unwrap();
        run_git(temp.path(), &["mv", "move-me.txt", "renamed.txt"]);
        fs::write(temp.path().join("notes.md"), "untracked\n").unwrap();

        let backend = LocalWorkspaceBackend;
        let status =
            block_on(backend.git_status(temp.path(), GitStatusOptions::default())).unwrap();

        let modified = status
            .entries
            .iter()
            .find(|entry| entry.relative_path == PathBuf::from("tracked.txt"))
            .unwrap();
        assert_eq!(modified.index_status, GitStatusKind::Unmodified);
        assert_eq!(modified.working_tree_status, GitStatusKind::Modified);

        let renamed = status
            .entries
            .iter()
            .find(|entry| entry.relative_path == PathBuf::from("renamed.txt"))
            .unwrap();
        assert_eq!(renamed.index_status, GitStatusKind::Renamed);
        assert_eq!(renamed.working_tree_status, GitStatusKind::Unmodified);
        assert_eq!(
            renamed.original_relative_path,
            Some(PathBuf::from("move-me.txt"))
        );

        let untracked = status
            .entries
            .iter()
            .find(|entry| entry.relative_path == PathBuf::from("notes.md"))
            .unwrap();
        assert_eq!(untracked.index_status, GitStatusKind::Untracked);
        assert_eq!(untracked.working_tree_status, GitStatusKind::Untracked);
        assert!(!status.truncated);
    }

    #[test]
    fn local_backend_git_status_returns_empty_outside_repository() {
        let temp = tempfile::tempdir().unwrap();
        let backend = LocalWorkspaceBackend;

        let status =
            block_on(backend.git_status(temp.path(), GitStatusOptions::default())).unwrap();

        assert_eq!(status.root, temp.path());
        assert!(status.entries.is_empty());
        assert!(!status.truncated);
    }

    #[test]
    fn local_backend_git_status_respects_limit() {
        let temp = tempfile::tempdir().unwrap();
        init_git_repo(temp.path());
        fs::write(temp.path().join("a.txt"), "a\n").unwrap();
        fs::write(temp.path().join("b.txt"), "b\n").unwrap();

        let backend = LocalWorkspaceBackend;
        let status =
            block_on(backend.git_status(temp.path(), GitStatusOptions::with_limit(1))).unwrap();

        assert_eq!(status.entries.len(), 1);
        assert!(status.truncated);
    }

    fn init_git_repo(root: &Path) {
        run_git(root, &["init"]);
        run_git(root, &["config", "user.email", "nucleotide@example.test"]);
        run_git(root, &["config", "user.name", "Nucleotide Tests"]);
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}
