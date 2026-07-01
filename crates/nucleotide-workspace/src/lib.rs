// ABOUTME: Workspace backend abstractions for local and remote project operations
// ABOUTME: Keeps editor-facing workspace services independent of transport details

use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use thiserror::Error;

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
}

#[derive(Debug, Default, Clone)]
pub struct LocalWorkspaceBackend;

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
