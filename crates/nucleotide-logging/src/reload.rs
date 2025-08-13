// ABOUTME: Runtime log level reloading and configuration updates
// ABOUTME: Provides mechanism to update log filters without restarting the application

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::sync::Arc;
use tracing_subscriber::reload;

use crate::config::LoggingConfig;
use crate::layers::create_env_filter;

/// Handle for updating log configuration at runtime
#[derive(Clone)]
pub struct LoggingReloadHandle {
    filter_handle: reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>,
    current_config: Arc<RwLock<LoggingConfig>>,
}

impl LoggingReloadHandle {
    /// Create a new reload handle with the given initial configuration
    pub fn new(
        filter_handle: reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>,
        config: LoggingConfig,
    ) -> Self {
        Self {
            filter_handle,
            current_config: Arc::new(RwLock::new(config)),
        }
    }

    /// Update the log level at runtime
    pub fn update_log_level(&self, level: tracing::Level) -> Result<()> {
        let mut config = self.current_config.write();
        config.level = level.into();

        let new_filter =
            create_env_filter(&config).context("Failed to create new environment filter")?;

        self.filter_handle
            .reload(new_filter)
            .context("Failed to reload log filter")?;

        tracing::info!(
            new_level = %level,
            "Log level updated at runtime"
        );

        Ok(())
    }

    /// Update module-specific log levels
    pub fn update_module_level(&self, module: &str, level: tracing::Level) -> Result<()> {
        let mut config = self.current_config.write();
        config
            .module_levels
            .insert(module.to_string(), level.into());

        let new_filter =
            create_env_filter(&config).context("Failed to create new environment filter")?;

        self.filter_handle
            .reload(new_filter)
            .context("Failed to reload log filter")?;

        tracing::info!(
            module = %module,
            new_level = %level,
            "Module log level updated at runtime"
        );

        Ok(())
    }

    /// Reload configuration from environment variables
    pub fn reload_from_env(&self) -> Result<()> {
        let mut new_config =
            LoggingConfig::from_env().context("Failed to load configuration from environment")?;

        // Keep file and output settings from current config
        let current_config = self.current_config.read();
        new_config.output = current_config.output.clone();
        new_config.file = current_config.file.clone();
        drop(current_config);

        let new_filter =
            create_env_filter(&new_config).context("Failed to create new environment filter")?;

        self.filter_handle
            .reload(new_filter)
            .context("Failed to reload log filter")?;

        *self.current_config.write() = new_config.clone();

        tracing::info!(
            level = %new_config.level.0,
            module_count = new_config.module_levels.len(),
            "Logging configuration reloaded from environment"
        );

        Ok(())
    }

    /// Get current configuration (clone)
    pub fn current_config(&self) -> LoggingConfig {
        self.current_config.read().clone()
    }
}
