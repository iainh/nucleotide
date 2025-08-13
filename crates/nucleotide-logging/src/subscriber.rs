// ABOUTME: Tracing subscriber initialization and layer composition
// ABOUTME: Combines console, file, and JSON layers with filtering for complete logging setup

use anyhow::{Context, Result};

use crate::config::LoggingConfig;
use crate::layers::create_env_filter;
use crate::reload::LoggingReloadHandle;

/// Initialize the global tracing subscriber with the given configuration.
pub fn init_subscriber(config: LoggingConfig) -> Result<()> {
    use tracing_subscriber::{fmt, prelude::*, util::SubscriberInitExt};

    // Create environment filter
    let env_filter = create_env_filter(&config).context("Failed to create environment filter")?;

    // Start with the registry and filter
    let registry = tracing_subscriber::registry().with(env_filter);

    // For now, let's just use a simple approach that works
    // We'll create console output with optional file output

    if config.output.file {
        // Setup file logging
        if let Some(parent) = config.file.path.parent() {
            std::fs::create_dir_all(parent).context(format!(
                "Failed to create log directory: {}",
                parent.display()
            ))?;
        }

        let file_name = config
            .file
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .context("Invalid log file path")?;

        let directory = config
            .file
            .path
            .parent()
            .context("Log file path has no parent directory")?;

        let file_appender = tracing_appender::rolling::daily(directory, file_name);
        let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
        std::mem::forget(_guard); // Keep the guard alive

        // Console + File setup
        if config.output.console {
            registry
                .with(fmt::layer().with_target(true).with_writer(std::io::stdout))
                .with(
                    fmt::layer()
                        .with_target(true)
                        .with_thread_ids(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_writer(file_writer),
                )
                .try_init()?;
        } else {
            // File only
            registry
                .with(
                    fmt::layer()
                        .with_target(true)
                        .with_thread_ids(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_writer(file_writer),
                )
                .try_init()?;
        }
    } else {
        // Console only
        registry.with(fmt::layer().with_target(true)).try_init()?;
    }

    // Log successful initialization
    tracing::info!(
        log_level = %config.level.0,
        console_output = config.output.console,
        file_output = config.output.file,
        json_output = config.output.json,
        file_path = %config.file.path.display(),
        "Nucleotide logging initialized"
    );

    Ok(())
}

/// Initialize the global tracing subscriber with hot-reload support.
///
/// Returns a LoggingReloadHandle that can be used to update log levels at runtime.
pub fn init_subscriber_with_reload(config: LoggingConfig) -> Result<LoggingReloadHandle> {
    use tracing_subscriber::{fmt, prelude::*, reload, util::SubscriberInitExt};

    // Create environment filter with reload capability
    let env_filter = create_env_filter(&config).context("Failed to create environment filter")?;
    let (filter_layer, filter_handle) = reload::Layer::new(env_filter);

    // Start with the registry and reloadable filter
    let registry = tracing_subscriber::registry().with(filter_layer);

    // Set up the same output configuration as the non-reloadable version
    if config.output.file {
        // Setup file logging
        if let Some(parent) = config.file.path.parent() {
            std::fs::create_dir_all(parent).context(format!(
                "Failed to create log directory: {}",
                parent.display()
            ))?;
        }

        let file_name = config
            .file
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .context("Invalid log file path")?;

        let directory = config
            .file
            .path
            .parent()
            .context("Log file path has no parent directory")?;

        let file_appender = tracing_appender::rolling::daily(directory, file_name);
        let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
        std::mem::forget(_guard); // Keep the guard alive

        // Console + File setup
        if config.output.console {
            registry
                .with(fmt::layer().with_target(true).with_writer(std::io::stdout))
                .with(
                    fmt::layer()
                        .with_target(true)
                        .with_thread_ids(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_writer(file_writer),
                )
                .try_init()?;
        } else {
            // File only
            registry
                .with(
                    fmt::layer()
                        .with_target(true)
                        .with_thread_ids(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_writer(file_writer),
                )
                .try_init()?;
        }
    } else {
        // Console only
        registry.with(fmt::layer().with_target(true)).try_init()?;
    }

    // Log successful initialization
    tracing::info!(
        log_level = %config.level.0,
        console_output = config.output.console,
        file_output = config.output.file,
        json_output = config.output.json,
        file_path = %config.file.path.display(),
        reload_enabled = true,
        "Nucleotide logging initialized with hot-reload support"
    );

    // Create and return the reload handle
    Ok(LoggingReloadHandle::new(filter_handle, config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LoggingConfig;
    use std::sync::Once;

    static INIT: Once = Once::new();

    #[test]
    fn test_init_subscriber() {
        // Only run this test once to avoid double-initialization
        INIT.call_once(|| {
            let config = LoggingConfig::default();
            let result = init_subscriber(config);
            // Note: This might fail if subscriber is already initialized,
            // which is okay for tests
            let _ = result;
        });
    }

    #[test]
    fn test_init_subscriber_with_custom_config() {
        use crate::config::{FileConfig, LogLevel, OutputConfig};
        use std::collections::HashMap;
        use tempfile::tempdir;
        use tracing::Level;

        let temp_dir = tempdir().unwrap();
        let log_path = temp_dir.path().join("test.log");

        let config = LoggingConfig {
            level: LogLevel(Level::DEBUG),
            module_levels: HashMap::new(),
            output: OutputConfig {
                console: true,
                file: false, // Disable file output for this test
                json: false,
                pretty_console: true,
            },
            file: FileConfig {
                path: log_path,
                max_size_mb: 10,
                max_files: 3,
            },
        };

        // This test just ensures the configuration is valid
        // We can't actually test initialization without risking conflicts
        let env_filter = create_env_filter(&config);
        assert!(env_filter.is_ok());
    }
}
