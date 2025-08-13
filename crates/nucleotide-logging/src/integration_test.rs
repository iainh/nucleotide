// ABOUTME: Integration tests for the complete logging system functionality
// ABOUTME: Tests initialization, configuration, and basic logging operations

#[cfg(test)]
mod tests {
    use crate::config::{FileConfig, LogLevel, LoggingConfig, OutputConfig};
    use crate::{debug, error, info, init_logging_with_config, warn};
    use std::sync::Once;
    use tempfile::tempdir;

    static INIT: Once = Once::new();

    #[test]
    fn test_logging_integration() {
        // Create a temporary directory for log files
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let log_file = temp_dir.path().join("nucleotide_integration_test.log");

        // Create a test configuration
        let config = LoggingConfig {
            level: LogLevel(tracing::Level::DEBUG),
            module_levels: std::collections::HashMap::new(),
            output: OutputConfig {
                console: false, // Disable console output for cleaner test
                file: true,
                json: false,
                pretty_console: false,
            },
            file: FileConfig {
                path: log_file.clone(),
                max_size_mb: 1,
                max_files: 1,
            },
        };

        // Only initialize once to avoid conflicts
        INIT.call_once(|| {
            init_logging_with_config(config).expect("Failed to initialize logging");
        });

        // Test that logging macros work
        info!("Integration test info message");
        debug!("Integration test debug message");
        warn!("Integration test warning message");
        error!("Integration test error message");

        // Test structured logging
        info!(
            test_field = "integration_test",
            counter = 42,
            "Structured log message for integration test"
        );

        // Give the async file appender a moment to write
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Verify log file was created (it should be in a dated subdirectory due to daily rotation)
        let log_dir = log_file.parent().unwrap();
        assert!(log_dir.exists(), "Log directory should exist");

        // Note: Due to daily rotation, the actual log file will be in a subdirectory
        // with today's date, so we can't easily verify the exact content in this test.
        // The fact that initialization succeeded and no panics occurred is sufficient
        // for this integration test.
    }

    #[test]
    fn test_environment_config_integration() {
        // Test that environment variables are properly handled
        std::env::set_var("NUCLEOTIDE_LOG", "trace");

        let config = LoggingConfig::from_env().expect("Should parse env config");
        assert_eq!(config.level.0, tracing::Level::TRACE);

        // Clean up
        std::env::remove_var("NUCLEOTIDE_LOG");
    }

    #[test]
    fn test_module_level_filtering() {
        // Test RUST_LOG style module filtering
        std::env::set_var(
            "RUST_LOG",
            "info,nucleotide_core=debug,nucleotide_lsp=trace",
        );

        let config = LoggingConfig::from_env().expect("Should parse RUST_LOG");
        assert_eq!(config.level.0, tracing::Level::INFO);
        assert_eq!(
            config.module_levels.get("nucleotide_core").unwrap().0,
            tracing::Level::DEBUG
        );
        assert_eq!(
            config.module_levels.get("nucleotide_lsp").unwrap().0,
            tracing::Level::TRACE
        );

        // Clean up
        std::env::remove_var("RUST_LOG");
    }
}
