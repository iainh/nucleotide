// ABOUTME: Versioned protocol primitives for the Nucleotide remote helper
// ABOUTME: Shared by the helper binary and future host-side remote clients

use serde::{Deserialize, Serialize};
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
}
