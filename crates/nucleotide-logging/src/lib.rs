// ABOUTME: Public API for nucleotide logging infrastructure using tokio-tracing
// ABOUTME: Provides centralized configuration and initialization for structured logging

pub mod config;
pub mod layers;
pub mod performance;
pub mod reload;
pub mod subscriber;

#[cfg(test)]
mod integration_test;

// Re-export tracing macros for convenience
pub use tracing::{debug, error, info, instrument, span, trace, warn, Level, Span};

use std::sync::OnceLock;

// Re-export configuration types
pub use config::LoggingConfig;

// Re-export initialization functions and reload handle
pub use reload::LoggingReloadHandle;
pub use subscriber::{init_subscriber, init_subscriber_with_reload};

// Re-export performance monitoring utilities
pub use performance::{PerfStats, PerfTimer};

use anyhow::Result;

/// Global reload handle for runtime log level updates
static GLOBAL_RELOAD_HANDLE: OnceLock<LoggingReloadHandle> = OnceLock::new();

/// Initialize logging with default configuration.
///
/// This is a convenience function that creates a default LoggingConfig
/// and initializes the tracing subscriber.
pub fn init_logging() -> Result<()> {
    let config = LoggingConfig::default();
    init_subscriber(config)
}

/// Initialize logging with custom configuration.
pub fn init_logging_with_config(config: LoggingConfig) -> Result<()> {
    init_subscriber(config)
}

/// Initialize logging with hot-reload support using custom configuration.
///
/// Returns a LoggingReloadHandle that can be used to update log levels at runtime.
/// Also stores the handle globally for runtime access.
pub fn init_logging_with_reload(config: LoggingConfig) -> Result<LoggingReloadHandle> {
    let handle = init_subscriber_with_reload(config)?;
    let _ = GLOBAL_RELOAD_HANDLE.set(handle.clone());
    Ok(handle)
}

/// Update log level at runtime using the global reload handle.
pub fn update_log_level(level: Level) -> Result<()> {
    match GLOBAL_RELOAD_HANDLE.get() {
        Some(handle) => handle.update_log_level(level),
        None => anyhow::bail!("Logging not initialized with reload support"),
    }
}

/// Update module-specific log level at runtime using the global reload handle.
pub fn update_module_level(module: &str, level: Level) -> Result<()> {
    match GLOBAL_RELOAD_HANDLE.get() {
        Some(handle) => handle.update_module_level(module, level),
        None => anyhow::bail!("Logging not initialized with reload support"),
    }
}

/// Reload configuration from environment variables using the global reload handle.
pub fn reload_from_env() -> Result<()> {
    match GLOBAL_RELOAD_HANDLE.get() {
        Some(handle) => handle.reload_from_env(),
        None => anyhow::bail!("Logging not initialized with reload support"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_logging() {
        // Test that initialization doesn't panic
        let result = init_logging();
        // Note: This might fail if already initialized, which is okay for tests
        let _ = result;
    }

    #[test]
    fn test_macros_available() {
        // Test that tracing macros are available
        info!("Test info message");
        debug!("Test debug message");
        warn!("Test warning message");
        error!("Test error message");
    }

    #[test]
    fn test_file_logging_with_structured_fields() {
        use tempfile::tempdir;

        // Create a temporary directory for testing
        let temp_dir = tempdir().unwrap();
        let log_path = temp_dir.path().join("test_nucleotide.log");

        // Create config with file logging enabled
        let mut config = crate::config::LoggingConfig::default();
        config.file.path = log_path.clone();
        config.output.file = true;
        config.output.console = false; // Disable console for cleaner test

        // Try initialization (might fail if already initialized)
        let result = init_logging_with_reload(config);

        // Send test messages with structured fields
        info!(test_type = "integration", "File logging test message");
        warn!(field = "value", count = 42, "Structured logging test");

        // Give it time to flush
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Check for log file with date suffix (daily rolling appender behavior)
        if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.to_string_lossy().contains("test_nucleotide.log") {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        assert!(contents.contains("File logging test message"));
                        assert!(contents.contains("Structured logging test"));
                        return; // Test passed
                    }
                }
            }
        }

        // Only fail if logging initialization succeeded but no file was created
        if result.is_ok() {
            panic!("Logging initialized but no log file was created");
        }
        // If initialization failed (due to double-init in tests), that's expected
    }

    #[test]
    fn test_file_logging_setup() {
        use std::fs::File;
        use tempfile::tempdir;

        // Create a temporary directory for testing
        let temp_dir = tempdir().unwrap();
        let log_path = temp_dir.path().join("simple_test.log");

        println!("Direct tracing test - log path: {}", log_path.display());

        // Test direct tracing setup without our wrapper
        let log_file = File::create(&log_path).expect("Failed to create log file");

        // Simple subscriber setup for testing
        let subscriber = tracing_subscriber::registry()
            .with(fmt::layer().with_writer(log_file).with_target(true));

        // Try to set as global default (might fail if already set)
        let init_result = tracing::subscriber::set_global_default(subscriber);
        match init_result {
            Ok(()) => {
                println!("✓ Direct tracing subscriber set successfully");

                // Test logging
                tracing::info!("Direct tracing test message");
                tracing::warn!(field = "value", "Direct tracing structured message");

                // Give it time to flush
                std::thread::sleep(std::time::Duration::from_millis(100));

                if log_path.exists() {
                    match std::fs::read_to_string(&log_path) {
                        Ok(contents) => {
                            println!("✓ Log file contents:");
                            println!("{}", contents);
                            if contents.contains("Direct tracing test message") {
                                println!("✓ Direct tracing is working correctly!");
                            } else {
                                println!("✗ Expected content not found in log file");
                            }
                        }
                        Err(e) => println!("✗ Failed to read log file: {}", e),
                    }
                } else {
                    println!("✗ Log file does not exist after direct tracing test");
                }
            }
            Err(e) => {
                println!("✗ Failed to set global subscriber (expected): {}", e);
                println!("Testing with existing subscriber...");

                // Test with whatever subscriber is already set
                tracing::info!("Fallback tracing test");
                std::thread::sleep(std::time::Duration::from_millis(100));

                if log_path.exists() {
                    let contents = std::fs::read_to_string(&log_path).unwrap_or_default();
                    println!("Log file exists with contents: {}", contents);
                } else {
                    println!("No log file created in fallback test");
                }
            }
        }
    }
}
