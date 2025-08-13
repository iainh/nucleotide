// ABOUTME: Custom layer implementations for different logging output formats
// ABOUTME: Provides console, file, and JSON layers with appropriate formatting

use anyhow::{Context, Result};
use std::fs;
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    EnvFilter, Layer, Registry,
};

use crate::config::{FileConfig, LoggingConfig, OutputConfig};

/// Create a console output layer with pretty formatting.
pub fn create_console_layer(
    config: &OutputConfig,
) -> Option<Box<dyn Layer<Registry> + Send + Sync + 'static>> {
    if !config.console {
        return None;
    }

    let layer = if config.pretty_console {
        fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_file(false)
            .with_line_number(false)
            .with_span_events(FmtSpan::CLOSE)
            .pretty()
            .boxed()
    } else {
        fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_file(false)
            .with_line_number(false)
            .compact()
            .boxed()
    };

    Some(layer)
}

/// Create a file output layer with rotation.
pub fn create_file_layer(
    config: &FileConfig,
) -> Result<Option<Box<dyn Layer<Registry> + Send + Sync + 'static>>> {
    // Ensure the parent directory exists
    if let Some(parent) = config.path.parent() {
        fs::create_dir_all(parent).context(format!(
            "Failed to create log directory: {}",
            parent.display()
        ))?;
    }

    let file_name = config
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid log file path")?;

    let directory = config
        .path
        .parent()
        .context("Log file path has no parent directory")?;

    // Create rolling file appender
    // Note: For now we'll use daily rotation, but we could extend this
    // to use size-based rotation based on config.max_size_mb
    let file_appender = rolling::daily(directory, file_name);
    let (non_blocking_writer, _guard) = non_blocking(file_appender);

    let layer = fmt::layer()
        .with_writer(non_blocking_writer)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .boxed();

    // Note: We're intentionally leaking the guard here because it needs to live
    // for the lifetime of the application. In a real application, you'd want to
    // store this guard somewhere and drop it on shutdown.
    std::mem::forget(_guard);

    Ok(Some(layer))
}

/// Create a JSON output layer for structured logging.
pub fn create_json_layer(
    config: &OutputConfig,
) -> Option<Box<dyn Layer<Registry> + Send + Sync + 'static>> {
    if !config.json {
        return None;
    }

    let layer = fmt::layer()
        .json()
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .boxed();

    Some(layer)
}

/// Create an environment filter from the logging configuration.
pub fn create_env_filter(config: &LoggingConfig) -> Result<EnvFilter> {
    let mut filter = EnvFilter::new(format!("{}", config.level.0));

    // Add module-specific filters
    for (module, level) in &config.module_levels {
        filter = filter.add_directive(format!("{}={}", module, level.0).parse()?);
    }

    // Allow environment variable overrides
    if let Ok(env_filter) = std::env::var("RUST_LOG") {
        filter = EnvFilter::new(env_filter);
    }

    Ok(filter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FileConfig, OutputConfig};
    use tempfile::tempdir;

    #[test]
    fn test_create_console_layer() {
        let config = OutputConfig {
            console: true,
            pretty_console: true,
            ..Default::default()
        };
        let layer = create_console_layer(&config);
        assert!(layer.is_some());

        let config = OutputConfig {
            console: false,
            ..Default::default()
        };
        let layer = create_console_layer(&config);
        assert!(layer.is_none());
    }

    #[test]
    fn test_create_json_layer() {
        let config = OutputConfig {
            json: true,
            ..Default::default()
        };
        let layer = create_json_layer(&config);
        assert!(layer.is_some());

        let config = OutputConfig {
            json: false,
            ..Default::default()
        };
        let layer = create_json_layer(&config);
        assert!(layer.is_none());
    }

    #[test]
    fn test_create_file_layer() {
        let temp_dir = tempdir().unwrap();
        let log_path = temp_dir.path().join("test.log");

        let config = FileConfig {
            path: log_path,
            max_size_mb: 10,
            max_files: 3,
        };

        let result = create_file_layer(&config);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn test_create_env_filter() {
        use crate::config::LogLevel;
        use tracing::Level;

        let mut config = LoggingConfig {
            level: LogLevel(Level::DEBUG),
            ..Default::default()
        };
        config
            .module_levels
            .insert("nucleotide_core".to_string(), LogLevel(Level::TRACE));

        let filter = create_env_filter(&config);
        assert!(filter.is_ok());
    }
}
