// ABOUTME: Public API for nucleotide logging infrastructure using tokio-tracing
// ABOUTME: Provides centralized configuration and initialization for structured logging

pub mod config;
pub mod layers;
pub mod subscriber;

#[cfg(test)]
mod integration_test;

// Re-export tracing macros for convenience
pub use tracing::{debug, error, info, instrument, span, trace, warn, Level, Span};

// Re-export configuration types
pub use config::LoggingConfig;

// Re-export initialization functions
pub use subscriber::init_subscriber;

use anyhow::Result;

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
}
