// ABOUTME: Versioned protocol primitives for the Nucleotide remote helper
// ABOUTME: Shared by the helper binary and future host-side remote clients

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

pub const PROTOCOL_VERSION: u32 = 12;
pub const DEFAULT_FILE_SEARCH_LIMIT: usize = 1_000;
pub const DEFAULT_GLOBAL_SEARCH_LIMIT: usize = 1_000;
pub const DEFAULT_FILE_READ_LIMIT: usize = 10_000;
pub const DEFAULT_WORKSPACE_SYMBOL_FILE_LIMIT: usize = 10_000;
pub const DEFAULT_WORKSPACE_SYMBOL_FILE_BYTE_LIMIT: usize = 1_000_000;
pub const DEFAULT_WORKSPACE_SYMBOL_TOTAL_BYTE_LIMIT: usize = 16_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResponse {
    pub protocol_version: u32,
    pub helper_version: String,
    pub os: String,
    pub arch: String,
    pub current_dir: PathBuf,
}

impl HelloResponse {
    pub fn current() -> std::io::Result<Self> {
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            current_dir: std::env::current_dir()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub variables: BTreeMap<String, String>,
}

impl EnvironmentResponse {
    pub fn current() -> std::io::Result<Self> {
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir: std::env::current_dir()?,
            variables: std::env::vars().collect(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMetadataResponse {
    pub protocol_version: u32,
    pub helper_version: String,
    pub os: String,
    pub arch: String,
    pub current_dir: PathBuf,
    pub home_dir: Option<PathBuf>,
    pub path_separator: String,
    #[serde(default)]
    pub workspace_markers: Option<BTreeSet<String>>,
    #[serde(default)]
    pub source_extensions: Option<BTreeSet<String>>,
    #[serde(default)]
    pub src_dir_exists: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRootResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub workspace_root: Option<PathBuf>,
    pub workspace_marker: Option<String>,
    pub project_root: Option<PathBuf>,
    pub project_marker: Option<String>,
}

impl WorkspaceRootResponse {
    pub fn current() -> std::io::Result<Self> {
        let current_dir = std::env::current_dir()?;
        let (workspace_root, workspace_marker) = detect_workspace_root_from_dir(&current_dir);
        let (project_root, project_marker) = detect_project_root_from_dir(&current_dir);

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            workspace_root,
            workspace_marker,
            project_root,
            project_marker,
        })
    }
}

impl WorkspaceMetadataResponse {
    pub fn current() -> std::io::Result<Self> {
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            current_dir: std::env::current_dir()?,
            home_dir: std::env::var_os("HOME").map(PathBuf::from),
            path_separator: std::path::MAIN_SEPARATOR.to_string(),
            workspace_markers: Some(detect_workspace_markers()?),
            source_extensions: Some(detect_source_extensions()?),
            src_dir_exists: Some(std::env::current_dir()?.join("src").is_dir()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteFileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntryResponse {
    pub name: String,
    pub kind: RemoteFileKind,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
    pub symlink_target: Option<PathBuf>,
    pub target_exists: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryListingResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub entries: Vec<DirectoryEntryResponse>,
}

impl DirectoryListingResponse {
    pub fn current() -> std::io::Result<Self> {
        let current_dir = std::env::current_dir()?;
        let mut entries = Vec::new();

        for entry in std::fs::read_dir(&current_dir)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            let file_type = metadata.file_type();
            let kind = if file_type.is_dir() {
                RemoteFileKind::Directory
            } else if file_type.is_file() {
                RemoteFileKind::File
            } else if file_type.is_symlink() {
                RemoteFileKind::Symlink
            } else {
                RemoteFileKind::Other
            };
            let modified_unix_millis = metadata.modified().ok().and_then(|modified| {
                modified
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_millis() as i64)
            });
            let symlink_target = if file_type.is_symlink() {
                std::fs::read_link(&path).ok()
            } else {
                None
            };
            let target_exists = symlink_target.as_ref().map(|target| {
                if target.is_absolute() {
                    target.exists()
                } else {
                    current_dir.join(target).exists()
                }
            });

            entries.push(DirectoryEntryResponse {
                name: entry.file_name().to_string_lossy().to_string(),
                kind,
                size: metadata.len(),
                modified_unix_millis,
                symlink_target,
                target_exists,
            });
        }

        entries.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.name.cmp(&right.name))
        });

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            entries,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCreateResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub path: PathBuf,
    pub kind: RemoteFileKind,
}

impl FileCreateResponse {
    pub fn current_file(name: &str) -> std::io::Result<Self> {
        let name = sanitize_child_name(name)?;
        let current_dir = std::env::current_dir()?;
        let path = current_dir.join(name);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            path,
            kind: RemoteFileKind::File,
        })
    }

    pub fn current_directory(name: &str) -> std::io::Result<Self> {
        let name = sanitize_child_name(name)?;
        let current_dir = std::env::current_dir()?;
        let path = current_dir.join(name);
        std::fs::create_dir(&path)?;

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            path,
            kind: RemoteFileKind::Directory,
        })
    }
}

fn sanitize_child_name(name: &str) -> std::io::Result<&str> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid child name",
        ));
    }

    Ok(name)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRenameResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    pub kind: RemoteFileKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDeleteResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub path: PathBuf,
    pub kind: RemoteFileKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDuplicateResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    pub kind: RemoteFileKind,
}

impl FileDeleteResponse {
    pub fn current(name: &str) -> std::io::Result<Self> {
        let name = sanitize_child_name(name)?;
        let current_dir = std::env::current_dir()?;
        let path = current_dir.join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        let kind = remote_file_kind_from_metadata(&metadata);

        if matches!(kind, RemoteFileKind::Directory) {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            path,
            kind,
        })
    }
}

impl FileDuplicateResponse {
    pub fn current(old_name: &str, target_name: &str) -> std::io::Result<Self> {
        let old_name = sanitize_child_name(old_name)?;
        let target_name = sanitize_child_name(target_name)?;
        let current_dir = std::env::current_dir()?;
        let old_path = current_dir.join(old_name);
        let new_path = current_dir.join(target_name);
        let metadata = std::fs::symlink_metadata(&old_path)?;
        let kind = remote_file_kind_from_metadata(&metadata);

        if new_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "target exists",
            ));
        }

        if old_path.is_dir() {
            copy_dir_recursive(&old_path, &new_path)?;
        } else {
            std::fs::copy(&old_path, &new_path)?;
        }

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            old_path,
            new_path,
            kind,
        })
    }
}

impl FileRenameResponse {
    pub fn current(old_name: &str, new_name: &str) -> std::io::Result<Self> {
        let old_name = sanitize_child_name(old_name)?;
        let new_name = sanitize_child_name(new_name)?;
        let current_dir = std::env::current_dir()?;
        let old_path = current_dir.join(old_name);
        let new_path = current_dir.join(new_name);
        let metadata = std::fs::symlink_metadata(&old_path)?;
        let kind = remote_file_kind_from_metadata(&metadata);

        if new_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "target exists",
            ));
        }

        std::fs::rename(&old_path, &new_path)?;

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            old_path,
            new_path,
            kind,
        })
    }
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to)?;
        } else if file_type.is_symlink()
            && let Ok(target) = std::fs::read_link(&from)
        {
            let absolute_target = if target.is_absolute() {
                target
            } else {
                from.parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join(target)
            };
            if absolute_target.is_dir() {
                copy_dir_recursive(&absolute_target, &to)?;
            } else {
                std::fs::copy(&absolute_target, &to)?;
            }
        }
    }
    Ok(())
}

fn remote_file_kind_from_metadata(metadata: &std::fs::Metadata) -> RemoteFileKind {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        RemoteFileKind::Directory
    } else if file_type.is_file() {
        RemoteFileKind::File
    } else if file_type.is_symlink() {
        RemoteFileKind::Symlink
    } else {
        RemoteFileKind::Other
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchEntryResponse {
    pub relative_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub files: Vec<FileSearchEntryResponse>,
    pub truncated: bool,
}

impl FileSearchResponse {
    pub fn current() -> anyhow::Result<Self> {
        Self::current_with_limit(DEFAULT_FILE_SEARCH_LIMIT)
    }

    pub fn current_with_limit(limit: usize) -> anyhow::Result<Self> {
        let current_dir = std::env::current_dir()?;
        let mut walker = ignore::WalkBuilder::new(&current_dir);
        walker
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .ignore(true)
            .parents(true)
            .hidden(true)
            .add_custom_ignore_filename(".helix/ignore")
            .filter_entry(|entry| {
                let file_name = entry
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");

                if entry.path().is_dir() {
                    return !matches!(
                        file_name,
                        ".git" | ".svn" | ".hg" | ".bzr" | ".jj" | "target" | "node_modules"
                    );
                }

                true
            });

        let mut files = Vec::new();
        let mut truncated = false;
        for entry in walker.build() {
            let entry = entry?;
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }

            let relative_path = entry
                .path()
                .strip_prefix(&current_dir)
                .unwrap_or(entry.path())
                .to_path_buf();

            if relative_path.as_os_str().is_empty() || relative_path.starts_with("zed-source") {
                continue;
            }

            if files.len() >= limit {
                truncated = true;
                break;
            }

            files.push(FileSearchEntryResponse { relative_path });
        }

        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            files,
            truncated,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSearchMatchResponse {
    pub relative_path: PathBuf,
    pub line: usize,
    pub line_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSearchResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub matches: Vec<GlobalSearchMatchResponse>,
    pub truncated: bool,
}

impl GlobalSearchResponse {
    pub fn current(query: &str, smart_case: bool, limit: usize) -> anyhow::Result<Self> {
        let current_dir = std::env::current_dir()?;
        let case_insensitive = smart_case && !query.chars().any(char::is_uppercase);
        let regex = regex::RegexBuilder::new(query)
            .case_insensitive(case_insensitive)
            .multi_line(true)
            .build()?;
        let mut walker = ignore::WalkBuilder::new(&current_dir);
        walker
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .ignore(true)
            .parents(true)
            .hidden(true)
            .add_custom_ignore_filename(".helix/ignore")
            .filter_entry(|entry| {
                let file_name = entry
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");

                if entry.path().is_dir() {
                    return !matches!(
                        file_name,
                        ".git" | ".svn" | ".hg" | ".bzr" | ".jj" | "target" | "node_modules"
                    );
                }

                true
            });

        let mut matches = Vec::new();
        let mut truncated = false;
        'walk: for entry in walker.build() {
            let entry = entry?;
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }

            let relative_path = entry
                .path()
                .strip_prefix(&current_dir)
                .unwrap_or(entry.path())
                .to_path_buf();
            if relative_path.as_os_str().is_empty() || relative_path.starts_with("zed-source") {
                continue;
            }

            let Ok(contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };

            for (line, line_text) in contents.lines().enumerate() {
                if !regex.is_match(line_text) {
                    continue;
                }

                if matches.len() >= limit {
                    truncated = true;
                    break 'walk;
                }

                matches.push(GlobalSearchMatchResponse {
                    relative_path: relative_path.clone(),
                    line,
                    line_text: line_text.trim_end().to_string(),
                });
            }
        }

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            matches,
            truncated,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub path: PathBuf,
    pub content: Option<String>,
    pub binary: bool,
    pub size: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSymbolFileEntryResponse {
    pub relative_path: PathBuf,
    pub content: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSymbolFilesResponse {
    pub protocol_version: u32,
    pub current_dir: PathBuf,
    pub files: Vec<WorkspaceSymbolFileEntryResponse>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceSymbolFilesOptions {
    pub hidden: bool,
    pub parents: bool,
    pub ignore: bool,
    pub follow_links: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
    pub deduplicate_links: bool,
    pub max_depth: Option<usize>,
    pub file_limit: usize,
    pub file_byte_limit: usize,
    pub total_byte_limit: usize,
}

impl Default for WorkspaceSymbolFilesOptions {
    fn default() -> Self {
        Self {
            hidden: false,
            parents: true,
            ignore: true,
            follow_links: false,
            git_ignore: true,
            git_global: true,
            git_exclude: true,
            deduplicate_links: true,
            max_depth: None,
            file_limit: DEFAULT_WORKSPACE_SYMBOL_FILE_LIMIT,
            file_byte_limit: DEFAULT_WORKSPACE_SYMBOL_FILE_BYTE_LIMIT,
            total_byte_limit: DEFAULT_WORKSPACE_SYMBOL_TOTAL_BYTE_LIMIT,
        }
    }
}

impl WorkspaceSymbolFilesResponse {
    pub fn current(options: WorkspaceSymbolFilesOptions) -> anyhow::Result<Self> {
        let current_dir = std::env::current_dir()?;
        let absolute_root = current_dir
            .canonicalize()
            .unwrap_or_else(|_| current_dir.clone());
        let mut walker = ignore::WalkBuilder::new(&current_dir);
        walker
            .hidden(options.hidden)
            .parents(options.parents)
            .ignore(options.ignore)
            .follow_links(options.follow_links)
            .git_ignore(options.git_ignore)
            .git_global(options.git_global)
            .git_exclude(options.git_exclude)
            .max_depth(options.max_depth)
            .filter_entry(move |entry| {
                filter_workspace_symbol_entry(entry, &absolute_root, options.deduplicate_links)
            })
            .add_custom_ignore_filename(".helix/ignore");

        let mut files = Vec::new();
        let mut total_bytes = 0usize;
        let mut truncated = false;

        for entry in walker.build() {
            let entry = entry?;
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }

            if files.len() >= options.file_limit {
                truncated = true;
                break;
            }

            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            let file_size = metadata.len();
            if file_size > options.file_byte_limit as u64 {
                continue;
            }
            if total_bytes.saturating_add(file_size as usize) > options.total_byte_limit {
                truncated = true;
                break;
            }

            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            total_bytes = total_bytes.saturating_add(content.len());
            let relative_path = entry
                .path()
                .strip_prefix(&current_dir)
                .unwrap_or(entry.path())
                .to_path_buf();
            if relative_path.as_os_str().is_empty() {
                continue;
            }

            files.push(WorkspaceSymbolFileEntryResponse {
                relative_path,
                content,
                size: file_size,
            });
        }

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            files,
            truncated,
        })
    }
}

impl FileReadResponse {
    pub fn current(path: &std::path::Path, limit: usize) -> std::io::Result<Self> {
        let current_dir = std::env::current_dir()?;
        let metadata = std::fs::metadata(path)?;
        let size = metadata.len();
        let (bytes, truncated) = read_file_prefix(path, limit)?;
        let (content, binary) = match utf8_prefix_from_bytes(&bytes) {
            Some(content) => (Some(content.to_string()), false),
            None => (None, true),
        };

        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            current_dir,
            path: path.to_path_buf(),
            content,
            binary,
            size,
            truncated,
        })
    }
}

fn read_file_prefix(path: &std::path::Path, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let read_limit = limit.saturating_add(1);
    let read_limit = u64::try_from(read_limit).unwrap_or(u64::MAX);
    let mut bytes = Vec::with_capacity(limit.min(DEFAULT_FILE_READ_LIMIT));
    std::fs::File::open(path)?
        .take(read_limit)
        .read_to_end(&mut bytes)?;

    let truncated = bytes.len() > limit;
    if truncated {
        bytes.truncate(limit);
    }

    Ok((bytes, truncated))
}

fn utf8_prefix_from_bytes(bytes: &[u8]) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
}

const WORKSPACE_MARKERS: &[&str] = &[
    "Cargo.toml",
    "tsconfig.json",
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "Pipfile",
    "go.mod",
    "go.sum",
    "CMakeLists.txt",
    "Makefile",
];
const VCS_WORKSPACE_MARKERS: &[&str] = &[".git", ".svn", ".hg", ".jj", ".helix"];
const PROJECT_ROOT_MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
    "pom.xml",
    "build.gradle",
    ".git",
    ".hg",
    ".svn",
];

fn detect_workspace_root_from_dir(
    start_dir: &std::path::Path,
) -> (Option<PathBuf>, Option<String>) {
    for ancestor in start_dir.ancestors() {
        let candidate = ancestor.join("Cargo.toml");
        if candidate.is_file()
            && std::fs::read_to_string(&candidate)
                .is_ok_and(|contents| contents.contains("[workspace]"))
        {
            return (Some(ancestor.to_path_buf()), Some("Cargo.toml".to_string()));
        }
    }

    for ancestor in start_dir.ancestors() {
        for marker in VCS_WORKSPACE_MARKERS {
            if ancestor.join(marker).exists() {
                return (Some(ancestor.to_path_buf()), Some((*marker).to_string()));
            }
        }
    }

    (None, None)
}

fn detect_project_root_from_dir(start_dir: &std::path::Path) -> (Option<PathBuf>, Option<String>) {
    for ancestor in start_dir.ancestors() {
        for marker in PROJECT_ROOT_MARKERS {
            if ancestor.join(marker).exists() {
                return (Some(ancestor.to_path_buf()), Some((*marker).to_string()));
            }
        }
    }

    (None, None)
}

fn filter_workspace_symbol_entry(
    entry: &ignore::DirEntry,
    root: &std::path::Path,
    deduplicate_links: bool,
) -> bool {
    if matches!(
        entry.file_name().to_str(),
        Some(".git" | ".pijul" | ".jj" | ".hg" | ".svn")
    ) {
        return false;
    }

    if deduplicate_links && entry.path_is_symlink() {
        return entry
            .path()
            .canonicalize()
            .ok()
            .is_some_and(|path| !path.starts_with(root));
    }

    true
}

fn detect_workspace_markers() -> std::io::Result<BTreeSet<String>> {
    let current_dir = std::env::current_dir()?;
    let mut markers = BTreeSet::new();
    for marker in WORKSPACE_MARKERS {
        if current_dir.join(marker).exists() {
            markers.insert((*marker).to_string());
        }
    }

    Ok(markers)
}

fn detect_source_extensions() -> std::io::Result<BTreeSet<String>> {
    let src_dir = std::env::current_dir()?.join("src");
    let mut extensions = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(src_dir) else {
        return Ok(extensions);
    };

    for entry in entries.flatten() {
        if let Some(extension) = entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
        {
            extensions.insert(extension.to_string());
        }
    }

    Ok(extensions)
}

pub fn encode_json_line<T: Serialize>(value: &T) -> serde_json::Result<String> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    static CURRENT_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn hello_response_uses_protocol_version() {
        let response = HelloResponse {
            protocol_version: PROTOCOL_VERSION,
            helper_version: "0.1.0".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            current_dir: PathBuf::from("/workspace"),
        };

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn json_line_encoding_is_newline_terminated() {
        let response = HelloResponse {
            protocol_version: PROTOCOL_VERSION,
            helper_version: "0.1.0".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            current_dir: PathBuf::from("/workspace"),
        };

        let line = encode_json_line(&response).unwrap();

        assert!(line.ends_with('\n'));
        assert!(line.contains(&format!("\"protocol_version\":{PROTOCOL_VERSION}")));
        assert!(line.contains("\"current_dir\":\"/workspace\""));
    }

    #[test]
    fn environment_response_uses_sorted_variables() {
        let response = EnvironmentResponse {
            protocol_version: PROTOCOL_VERSION,
            current_dir: PathBuf::from("/workspace"),
            variables: BTreeMap::from([
                ("ZED_ENVIRONMENT".to_string(), "wsl-shell".to_string()),
                ("PATH".to_string(), "/usr/bin".to_string()),
            ]),
        };

        let line = encode_json_line(&response).unwrap();
        let path_index = line.find("\"PATH\"").expect("PATH key");
        let zed_index = line
            .find("\"ZED_ENVIRONMENT\"")
            .expect("ZED_ENVIRONMENT key");

        assert!(path_index < zed_index);
        assert!(line.contains("\"variables\""));
    }

    #[test]
    fn workspace_metadata_response_encodes_remote_shape() {
        let response = WorkspaceMetadataResponse {
            protocol_version: PROTOCOL_VERSION,
            helper_version: "0.1.0".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            current_dir: PathBuf::from("/workspace"),
            home_dir: Some(PathBuf::from("/home/iain")),
            path_separator: "/".to_string(),
            workspace_markers: Some(BTreeSet::from(["Cargo.toml".to_string()])),
            source_extensions: Some(BTreeSet::from(["rs".to_string()])),
            src_dir_exists: Some(true),
        };

        let line = encode_json_line(&response).unwrap();

        assert!(line.contains("\"helper_version\":\"0.1.0\""));
        assert!(line.contains("\"home_dir\":\"/home/iain\""));
        assert!(line.contains("\"path_separator\":\"/\""));
        assert!(line.contains("\"workspace_markers\":[\"Cargo.toml\"]"));
        assert!(line.contains("\"source_extensions\":[\"rs\"]"));
        assert!(line.contains("\"src_dir_exists\":true"));
    }

    #[test]
    fn workspace_root_response_encodes_detected_roots() {
        let response = WorkspaceRootResponse {
            protocol_version: PROTOCOL_VERSION,
            current_dir: PathBuf::from("/workspace/project/src"),
            workspace_root: Some(PathBuf::from("/workspace")),
            workspace_marker: Some(".git".to_string()),
            project_root: Some(PathBuf::from("/workspace/project")),
            project_marker: Some("Cargo.toml".to_string()),
        };

        let line = encode_json_line(&response).unwrap();

        assert!(line.contains("\"current_dir\":\"/workspace/project/src\""));
        assert!(line.contains("\"workspace_root\":\"/workspace\""));
        assert!(line.contains("\"workspace_marker\":\".git\""));
        assert!(line.contains("\"project_root\":\"/workspace/project\""));
        assert!(line.contains("\"project_marker\":\"Cargo.toml\""));
    }

    #[test]
    fn workspace_root_detection_prefers_cargo_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let project_dir = temp.path().join("crates").join("app").join("src");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();

        let (root, marker) = detect_workspace_root_from_dir(&project_dir);

        assert_eq!(root.as_deref(), Some(temp.path()));
        assert_eq!(marker.as_deref(), Some("Cargo.toml"));
    }

    #[test]
    fn project_root_detection_finds_language_markers() {
        let temp = tempfile::tempdir().unwrap();
        let src_dir = temp.path().join("src").join("nested");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(temp.path().join("package.json"), "{}").unwrap();

        let (root, marker) = detect_project_root_from_dir(&src_dir);

        assert_eq!(root.as_deref(), Some(temp.path()));
        assert_eq!(marker.as_deref(), Some("package.json"));
    }

    #[test]
    fn directory_listing_response_encodes_file_metadata() {
        let response = DirectoryListingResponse {
            protocol_version: PROTOCOL_VERSION,
            current_dir: PathBuf::from("/workspace"),
            entries: vec![DirectoryEntryResponse {
                name: "src".to_string(),
                kind: RemoteFileKind::Directory,
                size: 4096,
                modified_unix_millis: Some(1_700_000_000_000),
                symlink_target: None,
                target_exists: None,
            }],
        };

        let line = encode_json_line(&response).unwrap();

        assert!(line.contains("\"current_dir\":\"/workspace\""));
        assert!(line.contains("\"name\":\"src\""));
        assert!(line.contains("\"kind\":\"directory\""));
        assert!(line.contains("\"modified_unix_millis\":1700000000000"));
    }

    #[test]
    fn file_search_response_encodes_relative_paths() {
        let response = FileSearchResponse {
            protocol_version: PROTOCOL_VERSION,
            current_dir: PathBuf::from("/workspace"),
            files: vec![FileSearchEntryResponse {
                relative_path: PathBuf::from("src/main.rs"),
            }],
            truncated: false,
        };

        let line = encode_json_line(&response).unwrap();

        assert!(line.contains("\"current_dir\":\"/workspace\""));
        assert!(line.contains("\"relative_path\":\"src/main.rs\""));
        assert!(line.contains("\"truncated\":false"));
    }

    #[test]
    fn global_search_response_encodes_matches() {
        let response = GlobalSearchResponse {
            protocol_version: PROTOCOL_VERSION,
            current_dir: PathBuf::from("/workspace"),
            matches: vec![GlobalSearchMatchResponse {
                relative_path: PathBuf::from("src/main.rs"),
                line: 2,
                line_text: "let needle = true;".to_string(),
            }],
            truncated: false,
        };

        let line = encode_json_line(&response).unwrap();

        assert!(line.contains("\"current_dir\":\"/workspace\""));
        assert!(line.contains("\"relative_path\":\"src/main.rs\""));
        assert!(line.contains("\"line\":2"));
        assert!(line.contains("\"line_text\":\"let needle = true;\""));
        assert!(line.contains("\"truncated\":false"));
    }

    #[test]
    fn workspace_symbol_files_response_encodes_file_contents() {
        let response = WorkspaceSymbolFilesResponse {
            protocol_version: PROTOCOL_VERSION,
            current_dir: PathBuf::from("/workspace"),
            files: vec![WorkspaceSymbolFileEntryResponse {
                relative_path: PathBuf::from("src/main.rs"),
                content: "fn main() {}\n".to_string(),
                size: 13,
            }],
            truncated: false,
        };

        let line = encode_json_line(&response).unwrap();

        assert!(line.contains("\"current_dir\":\"/workspace\""));
        assert!(line.contains("\"relative_path\":\"src/main.rs\""));
        assert!(line.contains("\"content\":\"fn main() {}\\n\""));
        assert!(line.contains("\"size\":13"));
        assert!(line.contains("\"truncated\":false"));
    }

    #[test]
    fn file_create_response_creates_new_file_in_current_dir() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = FileCreateResponse::current_file("created.rs").unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.path, temp.path().join("created.rs"));
        assert_eq!(response.kind, RemoteFileKind::File);
        assert!(temp.path().join("created.rs").is_file());
    }

    #[test]
    fn file_create_response_creates_new_directory_in_current_dir() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = FileCreateResponse::current_directory("created").unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.path, temp.path().join("created"));
        assert_eq!(response.kind, RemoteFileKind::Directory);
        assert!(temp.path().join("created").is_dir());
    }

    #[test]
    fn file_create_response_rejects_existing_or_invalid_names() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("exists.rs"), "").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let existing = FileCreateResponse::current_file("exists.rs").unwrap_err();
        let nested = FileCreateResponse::current_file("src/main.rs").unwrap_err();
        let windows_separator = FileCreateResponse::current_file(r"src\main.rs").unwrap_err();
        let parent = FileCreateResponse::current_file("..").unwrap_err();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(existing.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(nested.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(windows_separator.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(parent.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn file_rename_response_renames_file_in_current_dir() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("old.rs"), "").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = FileRenameResponse::current("old.rs", "new.rs").unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.old_path, temp.path().join("old.rs"));
        assert_eq!(response.new_path, temp.path().join("new.rs"));
        assert_eq!(response.kind, RemoteFileKind::File);
        assert!(!temp.path().join("old.rs").exists());
        assert!(temp.path().join("new.rs").is_file());
    }

    #[test]
    fn file_rename_response_renames_directory_in_current_dir() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("old")).unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = FileRenameResponse::current("old", "new").unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.kind, RemoteFileKind::Directory);
        assert!(!temp.path().join("old").exists());
        assert!(temp.path().join("new").is_dir());
    }

    #[test]
    fn file_rename_response_rejects_existing_target_or_invalid_names() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("old.rs"), "").unwrap();
        std::fs::write(temp.path().join("exists.rs"), "").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let existing = FileRenameResponse::current("old.rs", "exists.rs").unwrap_err();
        let nested = FileRenameResponse::current("old.rs", "src/main.rs").unwrap_err();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(existing.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(nested.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn file_delete_response_deletes_file_in_current_dir() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("delete.rs"), "").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = FileDeleteResponse::current("delete.rs").unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.path, temp.path().join("delete.rs"));
        assert_eq!(response.kind, RemoteFileKind::File);
        assert!(!temp.path().join("delete.rs").exists());
    }

    #[test]
    fn file_delete_response_deletes_directory_in_current_dir() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("delete-me").join("nested")).unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = FileDeleteResponse::current("delete-me").unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.kind, RemoteFileKind::Directory);
        assert!(!temp.path().join("delete-me").exists());
    }

    #[test]
    fn file_delete_response_rejects_invalid_names() {
        let nested = FileDeleteResponse::current("src/main.rs").unwrap_err();
        let parent = FileDeleteResponse::current("..").unwrap_err();

        assert_eq!(nested.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(parent.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn file_duplicate_response_duplicates_file_in_current_dir() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("main.rs"), "fn main() {}\n").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = FileDuplicateResponse::current("main.rs", "main copy.rs").unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.old_path, temp.path().join("main.rs"));
        assert_eq!(response.new_path, temp.path().join("main copy.rs"));
        assert_eq!(response.kind, RemoteFileKind::File);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("main copy.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn file_duplicate_response_duplicates_directory_in_current_dir() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src").join("nested")).unwrap();
        std::fs::write(temp.path().join("src").join("lib.rs"), "").unwrap();
        std::fs::write(temp.path().join("src").join("nested").join("mod.rs"), "").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = FileDuplicateResponse::current("src", "src copy").unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.kind, RemoteFileKind::Directory);
        assert!(temp.path().join("src copy").join("lib.rs").exists());
        assert!(
            temp.path()
                .join("src copy")
                .join("nested")
                .join("mod.rs")
                .exists()
        );
    }

    #[test]
    fn file_duplicate_response_rejects_existing_or_invalid_names() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("main.rs"), "").unwrap();
        std::fs::write(temp.path().join("copy.rs"), "").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let existing = FileDuplicateResponse::current("main.rs", "copy.rs").unwrap_err();
        let nested_source = FileDuplicateResponse::current("src/main.rs", "new.rs").unwrap_err();
        let nested_target = FileDuplicateResponse::current("main.rs", "src/new.rs").unwrap_err();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(existing.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(nested_source.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(nested_target.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn workspace_symbol_files_current_returns_text_files() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src").join("main.rs"), "fn main() {}\n").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = WorkspaceSymbolFilesResponse::current(WorkspaceSymbolFilesOptions {
            file_limit: 10,
            file_byte_limit: 100,
            total_byte_limit: 1_000,
            ..WorkspaceSymbolFilesOptions::default()
        })
        .unwrap();

        std::env::set_current_dir(original).unwrap();

        assert_eq!(response.files.len(), 1);
        assert_eq!(
            response.files[0].relative_path,
            PathBuf::from("src/main.rs")
        );
        assert_eq!(response.files[0].content, "fn main() {}\n");
        assert!(!response.truncated);
    }

    #[test]
    fn workspace_symbol_files_current_respects_total_byte_limit() {
        let _guard = CURRENT_DIR_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("first.rs"), "fn first() {}\n").unwrap();
        std::fs::write(temp.path().join("second.rs"), "fn second() {}\n").unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();

        let response = WorkspaceSymbolFilesResponse::current(WorkspaceSymbolFilesOptions {
            file_limit: 10,
            file_byte_limit: 100,
            total_byte_limit: 1,
            ..WorkspaceSymbolFilesOptions::default()
        })
        .unwrap();

        std::env::set_current_dir(original).unwrap();

        assert!(response.files.is_empty());
        assert!(response.truncated);
    }

    #[test]
    fn file_read_response_encodes_text_preview() {
        let response = FileReadResponse {
            protocol_version: PROTOCOL_VERSION,
            current_dir: PathBuf::from("/workspace"),
            path: PathBuf::from("src/main.rs"),
            content: Some("fn main() {}\n".to_string()),
            binary: false,
            size: 13,
            truncated: false,
        };

        let line = encode_json_line(&response).unwrap();

        assert!(line.contains("\"current_dir\":\"/workspace\""));
        assert!(line.contains("\"path\":\"src/main.rs\""));
        assert!(line.contains("\"content\":\"fn main() {}\\n\""));
        assert!(line.contains("\"binary\":false"));
        assert!(line.contains("\"size\":13"));
        assert!(line.contains("\"truncated\":false"));
    }

    #[test]
    fn file_read_prefix_keeps_utf8_boundary() {
        assert_eq!(utf8_prefix_from_bytes("abcdef".as_bytes()), Some("abcdef"));
        assert_eq!(utf8_prefix_from_bytes(&"éclair".as_bytes()[..1]), Some(""));
        assert_eq!(utf8_prefix_from_bytes(&"éclair".as_bytes()[..2]), Some("é"));
        assert_eq!(utf8_prefix_from_bytes(&[0xff]), None);
    }

    #[test]
    fn file_read_current_reads_only_bounded_text_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.txt");
        std::fs::write(&path, "abcdef").unwrap();

        let response = FileReadResponse::current(&path, 3).unwrap();

        assert_eq!(response.content.as_deref(), Some("abc"));
        assert!(!response.binary);
        assert_eq!(response.size, 6);
        assert!(response.truncated);
    }

    #[test]
    fn file_read_current_truncates_on_utf8_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("unicode.txt");
        std::fs::write(&path, "éclair").unwrap();

        let response = FileReadResponse::current(&path, 1).unwrap();

        assert_eq!(response.content.as_deref(), Some(""));
        assert!(!response.binary);
        assert_eq!(response.size, 7);
        assert!(response.truncated);
    }

    #[test]
    fn file_read_current_marks_invalid_utf8_prefix_as_binary() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("image.bin");
        std::fs::write(&path, [0xff, 0x00, 0x01]).unwrap();

        let response = FileReadResponse::current(&path, 2).unwrap();

        assert!(response.content.is_none());
        assert!(response.binary);
        assert_eq!(response.size, 3);
        assert!(response.truncated);
    }

    #[test]
    fn file_read_current_handles_zero_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("empty-preview.txt");
        std::fs::write(&path, "abc").unwrap();

        let response = FileReadResponse::current(&path, 0).unwrap();

        assert_eq!(response.content.as_deref(), Some(""));
        assert!(!response.binary);
        assert_eq!(response.size, 3);
        assert!(response.truncated);
    }
}
