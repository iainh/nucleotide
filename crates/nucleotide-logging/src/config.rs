// ABOUTME: Configuration structures and environment variable parsing for logging
// ABOUTME: Handles log levels, output targets, and file path configuration

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;
use tracing::Level;

/// Wrapper for tracing::Level that implements Serialize/Deserialize
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLevel(pub Level);

impl Serialize for LogLevel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let level_str = match self.0 {
            Level::TRACE => "trace",
            Level::DEBUG => "debug",
            Level::INFO => "info",
            Level::WARN => "warn",
            Level::ERROR => "error",
        };
        serializer.serialize_str(level_str)
    }
}

impl<'de> Deserialize<'de> for LogLevel {
    fn deserialize<D>(deserializer: D) -> Result<LogLevel, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let level = parse_log_level(&s).map_err(serde::de::Error::custom)?;
        Ok(LogLevel(level))
    }
}

impl From<Level> for LogLevel {
    fn from(level: Level) -> Self {
        LogLevel(level)
    }
}

impl From<LogLevel> for Level {
    fn from(log_level: LogLevel) -> Self {
        log_level.0
    }
}

/// Main configuration structure for the logging system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Global log level (trace, debug, info, warn, error)
    pub level: LogLevel,

    /// Per-module log level overrides
    pub module_levels: HashMap<String, LogLevel>,

    /// Output configuration
    pub output: OutputConfig,

    /// File logging configuration
    pub file: FileConfig,
}

/// Configuration for different output targets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Enable console output
    pub console: bool,

    /// Enable file output
    pub file: bool,

    /// Enable JSON structured output
    pub json: bool,

    /// Pretty-print console output (vs compact)
    pub pretty_console: bool,
}

/// Configuration for file logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    /// Path prefix for daily log files.
    pub path: PathBuf,

    /// Maximum file size before rotation (in MB)
    pub max_size_mb: u64,

    /// Number of rotated files to keep
    pub max_files: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel(Level::INFO),
            module_levels: HashMap::new(),
            output: OutputConfig::default(),
            file: FileConfig::default(),
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            console: true,
            file: true,
            json: false,
            pretty_console: true,
        }
    }
}

impl Default for FileConfig {
    fn default() -> Self {
        let path = default_log_file_path();
        Self {
            path,
            max_size_mb: 50,
            max_files: 5,
        }
    }
}

impl LoggingConfig {
    /// Create a new configuration with environment variable overrides.
    pub fn from_env() -> Result<Self> {
        let mut config = Self::default();
        config.apply_env_overrides()?;
        Ok(config)
    }

    /// Apply environment variable overrides to this configuration.
    pub fn apply_env_overrides(&mut self) -> Result<()> {
        // Check NUCLEOTIDE_LOG first, then RUST_LOG
        if let Ok(level_str) = env::var("NUCLEOTIDE_LOG") {
            self.level =
                LogLevel(parse_log_level(&level_str).context("Invalid NUCLEOTIDE_LOG level")?);
        } else if let Ok(level_str) = env::var("RUST_LOG") {
            // Parse RUST_LOG format (e.g., "debug" or "nucleotide=debug,info")
            self.parse_rust_log(&level_str)?;
        }

        // Check for output format overrides
        if env::var("NUCLEOTIDE_LOG_JSON").is_ok() {
            self.output.json = true;
        }

        if env::var("NUCLEOTIDE_LOG_NO_CONSOLE").is_ok() {
            self.output.console = false;
        }

        if env::var("NUCLEOTIDE_LOG_NO_FILE").is_ok() {
            self.output.file = false;
        }

        Ok(())
    }

    /// Parse RUST_LOG format environment variable.
    fn parse_rust_log(&mut self, rust_log: &str) -> Result<()> {
        for directive in rust_log.split(',') {
            let directive = directive.trim();
            if directive.is_empty() {
                continue;
            }

            if let Some((module, level_str)) = directive.split_once('=') {
                let level = parse_log_level(level_str).context(format!(
                    "Invalid log level '{level_str}' for module '{module}'"
                ))?;
                self.module_levels
                    .insert(module.to_string(), LogLevel(level));
            } else {
                // Global level
                self.level = LogLevel(
                    parse_log_level(directive)
                        .context(format!("Invalid global log level '{directive}'"))?,
                );
            }
        }
        Ok(())
    }
}

/// Get the default application log file path prefix.
pub fn default_log_file_path() -> PathBuf {
    if let Some(directory) = env::var_os("NUCLEOTIDE_LOG_DIR").map(PathBuf::from) {
        return directory.join("nucleotide.log");
    }

    #[cfg(windows)]
    {
        return windows_log_file_path(
            &dirs::data_local_dir().unwrap_or_else(std::env::temp_dir),
            "nucleotide.log",
        );
    }

    #[cfg(not(windows))]
    {
        dirs::config_dir()
            .map(|config_dir| config_dir.join("nucleotide").join("nucleotide.log"))
            .unwrap_or_else(|| PathBuf::from("nucleotide.log"))
    }
}

/// Get the default `nucleotide-remote` log file path prefix on the helper host.
pub fn default_remote_log_file_path() -> PathBuf {
    if let Some(directory) = env::var_os("NUCLEOTIDE_LOG_DIR").map(PathBuf::from) {
        return directory.join("nucleotide-remote.log");
    }

    #[cfg(windows)]
    {
        return windows_log_file_path(
            &dirs::data_local_dir().unwrap_or_else(std::env::temp_dir),
            "nucleotide-remote.log",
        );
    }

    #[cfg(not(windows))]
    {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .or_else(dirs::config_dir)
            .unwrap_or_else(std::env::temp_dir)
            .join("nucleotide")
            .join("logs")
            .join("nucleotide-remote.log")
    }
}

#[cfg(any(windows, test))]
fn windows_log_file_path(base: &Path, file_name: &str) -> PathBuf {
    base.join("Spiralpoint")
        .join("Nucleotide")
        .join("logs")
        .join(file_name)
}

/// Parse a log level string (case-insensitive).
fn parse_log_level(level_str: &str) -> Result<Level> {
    match level_str.to_lowercase().as_str() {
        "trace" => Ok(Level::TRACE),
        "debug" => Ok(Level::DEBUG),
        "info" => Ok(Level::INFO),
        "warn" | "warning" => Ok(Level::WARN),
        "error" => Ok(Level::ERROR),
        _ => anyhow::bail!(
            "Invalid log level: {}. Must be one of: trace, debug, info, warn, error",
            level_str
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LoggingConfig::default();
        assert_eq!(config.level.0, Level::INFO);
        assert!(config.output.console);
        assert!(config.output.file);
        assert!(!config.output.json);
        assert!(config.output.pretty_console);
    }

    #[test]
    fn test_parse_log_level() {
        assert_eq!(parse_log_level("trace").unwrap(), Level::TRACE);
        assert_eq!(parse_log_level("DEBUG").unwrap(), Level::DEBUG);
        assert_eq!(parse_log_level("Info").unwrap(), Level::INFO);
        assert_eq!(parse_log_level("WARN").unwrap(), Level::WARN);
        assert_eq!(parse_log_level("error").unwrap(), Level::ERROR);

        assert!(parse_log_level("invalid").is_err());
    }

    #[test]
    fn test_default_log_path() {
        let path = default_log_file_path();
        assert!(path.to_string_lossy().contains("nucleotide.log"));
    }

    #[test]
    fn windows_log_path_uses_vendor_and_product_directories() {
        let path = windows_log_file_path(Path::new("local-app-data"), "nucleotide.log");
        assert_eq!(
            path,
            Path::new("local-app-data")
                .join("Spiralpoint")
                .join("Nucleotide")
                .join("logs")
                .join("nucleotide.log")
        );
    }

    #[test]
    fn remote_log_path_has_distinct_file_name() {
        let path = default_remote_log_file_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("nucleotide-remote.log")
        );
    }

    #[test]
    fn test_parse_rust_log() {
        let mut config = LoggingConfig::default();

        // Test simple global level
        config.parse_rust_log("debug").unwrap();
        assert_eq!(config.level.0, Level::DEBUG);

        // Test module-specific levels
        let mut config = LoggingConfig::default();
        config
            .parse_rust_log("info,nucleotide_core=debug,nucleotide_lsp=trace")
            .unwrap();
        assert_eq!(config.level.0, Level::INFO);
        assert_eq!(
            config.module_levels.get("nucleotide_core"),
            Some(&LogLevel(Level::DEBUG))
        );
        assert_eq!(
            config.module_levels.get("nucleotide_lsp"),
            Some(&LogLevel(Level::TRACE))
        );
    }
}
