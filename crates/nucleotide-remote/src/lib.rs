// ABOUTME: Versioned protocol primitives for the Nucleotide remote helper
// ABOUTME: Shared by the helper binary and future host-side remote clients

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

pub const PROTOCOL_VERSION: u32 = 3;
pub const DEFAULT_FILE_SEARCH_LIMIT: usize = 1_000;

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
        assert!(line.contains("\"protocol_version\":3"));
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
}
