// ABOUTME: Versioned protocol primitives for the Nucleotide remote helper
// ABOUTME: Shared by the helper binary and future host-side remote clients

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub const PROTOCOL_VERSION: u32 = 1;

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

        assert_eq!(response.protocol_version, 1);
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
        assert!(line.contains("\"protocol_version\":1"));
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
}
