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
pub use tracing::{Level, Span, debug, error, info, instrument, span, trace, warn};

use std::sync::OnceLock;

// Re-export configuration types
pub use config::{LoggingConfig, default_log_file_path, default_remote_log_file_path};

// Re-export initialization functions and reload handle
pub use reload::LoggingReloadHandle;
pub use subscriber::{
    init_file_subscriber, init_stderr_subscriber, init_subscriber, init_subscriber_with_reload,
};

// Re-export performance monitoring utilities
pub use performance::PerfTimer;

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

/// Initialize synchronous file-only logging for a protocol helper process.
pub fn init_file_logging(config: LoggingConfig) -> Result<()> {
    init_file_subscriber(config)
}

/// Initialize synchronous stderr-only logging for a protocol helper process.
pub fn init_stderr_logging(config: LoggingConfig) -> Result<()> {
    init_stderr_subscriber(config)
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
