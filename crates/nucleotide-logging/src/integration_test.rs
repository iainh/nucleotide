// ABOUTME: Integration tests for the complete logging system functionality
// ABOUTME: Tests initialization, configuration, and basic logging operations

#[cfg(test)]
mod tests {
    use crate::config::LoggingConfig;
    use std::sync::Mutex;

    /// Mutex to ensure environment variable tests don't run concurrently
    static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_environment_config_integration() {
        // Acquire mutex to prevent concurrent environment variable modification
        let _guard = ENV_TEST_MUTEX.lock().unwrap();

        // Save existing environment state
        let original_nucleotide_log = std::env::var("NUCLEOTIDE_LOG").ok();
        let original_rust_log = std::env::var("RUST_LOG").ok();

        // Clean up any existing environment variables that could interfere
        unsafe {
            std::env::remove_var("NUCLEOTIDE_LOG");
            std::env::remove_var("RUST_LOG");
        }

        // Test that environment variables are properly handled
        unsafe {
            std::env::set_var("NUCLEOTIDE_LOG", "trace");
        }

        let config = LoggingConfig::from_env().expect("Should parse env config");
        assert_eq!(config.level.0, tracing::Level::TRACE);

        // Restore original environment state
        unsafe {
            std::env::remove_var("NUCLEOTIDE_LOG");
            std::env::remove_var("RUST_LOG");

            if let Some(val) = original_nucleotide_log {
                std::env::set_var("NUCLEOTIDE_LOG", val);
            }
            if let Some(val) = original_rust_log {
                std::env::set_var("RUST_LOG", val);
            }
        }
    }

    #[test]
    fn test_module_level_filtering() {
        // Acquire mutex to prevent concurrent environment variable modification
        let _guard = ENV_TEST_MUTEX.lock().unwrap();

        // Save existing environment state
        let original_nucleotide_log = std::env::var("NUCLEOTIDE_LOG").ok();
        let original_rust_log = std::env::var("RUST_LOG").ok();

        // Clean up any existing environment variables that could interfere
        unsafe {
            std::env::remove_var("NUCLEOTIDE_LOG");
            std::env::remove_var("RUST_LOG");
        }

        // Test RUST_LOG style module filtering
        unsafe {
            std::env::set_var(
                "RUST_LOG",
                "info,nucleotide_core=debug,nucleotide_lsp=trace",
            );
        }

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

        // Restore original environment state
        unsafe {
            std::env::remove_var("NUCLEOTIDE_LOG");
            std::env::remove_var("RUST_LOG");

            if let Some(val) = original_nucleotide_log {
                std::env::set_var("NUCLEOTIDE_LOG", val);
            }
            if let Some(val) = original_rust_log {
                std::env::set_var("RUST_LOG", val);
            }
        }
    }
}
