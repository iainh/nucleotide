// ABOUTME: Runtime theme switching system for hot-swapping themes without restart
// ABOUTME: Provides immediate theme updates, persistence, and state management

use crate::Theme;
use gpui::SharedString;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

/// Runtime theme switcher for immediate theme changes
#[derive(Debug)]
pub struct RuntimeThemeSwitcher {
    /// Current theme state
    current_state: Arc<RwLock<ThemeSwitcherState>>,
    /// Switching configuration
    config: SwitchingConfig,
    /// Theme history for undo/redo
    theme_history: Arc<RwLock<ThemeHistory>>,
    /// Persistence manager
    persistence: PersistenceManager,
    /// Performance monitor
    performance_monitor: SwitchingPerformanceMonitor,
}

/// Theme switcher state
#[derive(Debug, Clone)]
pub struct ThemeSwitcherState {
    /// Currently active theme
    pub active_theme: Theme,
    /// Active theme name
    pub active_theme_name: Option<SharedString>,
    /// Previous theme for quick switching
    pub previous_theme: Option<Theme>,
    /// Previous theme name
    pub previous_theme_name: Option<SharedString>,
    /// Switch timestamp
    pub last_switch_time: SystemTime,
    /// Whether switching is in progress
    pub is_switching: bool,
    /// Switch operation ID
    pub switch_operation_id: Option<u64>,
}

/// Theme switching configuration
#[derive(Debug, Clone)]
pub struct SwitchingConfig {
    /// Enable immediate switching
    pub immediate_switching: bool,
    /// Enable theme persistence
    pub persist_theme_choice: bool,
    /// Persistence storage key
    pub storage_key: String,
    /// Enable theme history
    pub enable_history: bool,
    /// Maximum history size
    pub max_history_size: usize,
    /// Enable performance monitoring
    pub monitor_performance: bool,
    /// Auto-save interval
    pub auto_save_interval: Duration,
    /// Enable crash recovery
    pub enable_crash_recovery: bool,
}

/// Theme history for undo/redo operations
#[derive(Debug, Clone)]
pub struct ThemeHistory {
    /// History entries
    pub entries: Vec<ThemeHistoryEntry>,
    /// Current position in history
    pub current_position: usize,
    /// Maximum history size
    pub max_size: usize,
}

/// Theme history entry
#[derive(Debug, Clone)]
pub struct ThemeHistoryEntry {
    /// Theme at this point in history
    pub theme: Theme,
    /// Theme name
    pub theme_name: Option<SharedString>,
    /// Timestamp of this entry
    pub timestamp: SystemTime,
    /// Operation that led to this state
    pub operation: ThemeOperation,
    /// User-provided description
    pub description: Option<String>,
}

/// Theme operations for history tracking
#[derive(Debug, Clone)]
pub enum ThemeOperation {
    /// Theme switched by user
    UserSwitch,
    /// Theme loaded from file
    LoadFromFile,
    /// Theme imported
    Import,
    /// Theme modified
    Modify,
    /// Theme reset to default
    Reset,
    /// Automatic theme change (e.g., system preference)
    Automatic,
}

/// Persistence manager for theme choices
#[derive(Debug, Clone)]
pub struct PersistenceManager {
    /// Storage backend
    pub backend: StorageBackend,
    /// Last save time
    pub last_save_time: Option<SystemTime>,
    /// Auto-save enabled
    pub auto_save_enabled: bool,
    /// Crash recovery data
    pub recovery_data: Option<RecoveryData>,
}

/// Storage backend options
#[derive(Debug, Clone)]
pub enum StorageBackend {
    /// Local filesystem storage
    FileSystem { config_dir: std::path::PathBuf },
    /// In-memory storage (no persistence)
    Memory,
    /// Environment variables
    Environment,
    /// Custom storage implementation
    Custom(Arc<dyn CustomStorage>),
}

/// Custom storage trait
pub trait CustomStorage: std::fmt::Debug + Send + Sync {
    /// Save theme choice
    fn save_theme(
        &self,
        key: &str,
        theme_name: &str,
        theme_data: &Theme,
    ) -> Result<(), StorageError>;

    /// Load theme choice
    fn load_theme(&self, key: &str) -> Result<Option<(String, Theme)>, StorageError>;

    /// Clear saved theme
    fn clear_theme(&self, key: &str) -> Result<(), StorageError>;

    /// List available saved themes
    fn list_themes(&self) -> Result<Vec<String>, StorageError>;
}

/// Crash recovery data
#[derive(Debug, Clone)]
pub struct RecoveryData {
    /// Theme at time of crash
    pub theme_at_crash: Theme,
    /// Theme name at time of crash
    pub theme_name: Option<SharedString>,
    /// Crash timestamp
    pub crash_timestamp: SystemTime,
    /// Recovery timestamp
    pub recovery_timestamp: SystemTime,
}

/// Performance monitoring for theme switching
#[derive(Debug, Clone)]
pub struct SwitchingPerformanceMonitor {
    /// Switch operation metrics
    pub switch_metrics: Vec<SwitchMetric>,
    /// Average switch time
    pub average_switch_time: Duration,
    /// Maximum switch time recorded
    pub max_switch_time: Duration,
    /// Total switches performed
    pub total_switches: u64,
    /// Failed switches
    pub failed_switches: u64,
}

/// Individual switch operation metric
#[derive(Debug, Clone)]
pub struct SwitchMetric {
    /// Operation ID
    pub operation_id: u64,
    /// Switch start time
    pub start_time: SystemTime,
    /// Switch completion time
    pub completion_time: SystemTime,
    /// Switch duration
    pub duration: Duration,
    /// Source theme name
    pub from_theme: Option<SharedString>,
    /// Target theme name
    pub to_theme: Option<SharedString>,
    /// Whether switch was successful
    pub success: bool,
    /// Error message if failed
    pub error_message: Option<String>,
}

/// Runtime switching result
#[derive(Debug, Clone)]
pub struct SwitchingResult {
    /// Whether switch was successful
    pub success: bool,
    /// Previous theme
    pub previous_theme: Option<Theme>,
    /// New active theme
    pub new_theme: Theme,
    /// Switch duration
    pub switch_duration: Duration,
    /// Operation ID
    pub operation_id: u64,
    /// Any warnings during switch
    pub warnings: Vec<String>,
}

impl Default for SwitchingConfig {
    fn default() -> Self {
        Self {
            immediate_switching: true,
            persist_theme_choice: true,
            storage_key: "nucleotide_ui_theme".to_string(),
            enable_history: true,
            max_history_size: 50,
            monitor_performance: true,
            auto_save_interval: Duration::from_secs(30),
            enable_crash_recovery: true,
        }
    }
}

impl Default for ThemeHistory {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            current_position: 0,
            max_size: 50,
        }
    }
}

impl Default for PersistenceManager {
    fn default() -> Self {
        Self {
            backend: StorageBackend::FileSystem {
                config_dir: dirs::config_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join("nucleotide-ui"),
            },
            last_save_time: None,
            auto_save_enabled: true,
            recovery_data: None,
        }
    }
}

impl Default for SwitchingPerformanceMonitor {
    fn default() -> Self {
        Self {
            switch_metrics: Vec::new(),
            average_switch_time: Duration::ZERO,
            max_switch_time: Duration::ZERO,
            total_switches: 0,
            failed_switches: 0,
        }
    }
}

impl RuntimeThemeSwitcher {
    /// Create a new runtime theme switcher
    pub fn new() -> Self {
        let initial_state = ThemeSwitcherState {
            active_theme: Theme::dark(),
            active_theme_name: Some("dark".into()),
            previous_theme: None,
            previous_theme_name: None,
            last_switch_time: SystemTime::now(),
            is_switching: false,
            switch_operation_id: None,
        };

        Self {
            current_state: Arc::new(RwLock::new(initial_state)),
            config: SwitchingConfig::default(),
            theme_history: Arc::new(RwLock::new(ThemeHistory::default())),
            persistence: PersistenceManager::default(),
            performance_monitor: SwitchingPerformanceMonitor::default(),
        }
    }

    /// Create switcher with custom configuration
    pub fn with_config(config: SwitchingConfig) -> Self {
        let mut switcher = Self::new();
        switcher.config = config;
        switcher
    }

    /// Switch to a new theme immediately
    pub fn switch_theme_immediate(
        &mut self,
        new_theme: Theme,
        theme_name: Option<SharedString>,
    ) -> Result<SwitchingResult, SwitchingError> {
        let operation_id = self.generate_operation_id();
        let start_time = SystemTime::now();

        nucleotide_logging::info!(
            operation_id = operation_id,
            theme_name = ?theme_name,
            "Starting immediate theme switch"
        );

        // Mark switching as in progress
        let previous_state = {
            if let Ok(mut state) = self.current_state.write() {
                let previous = state.clone();
                state.is_switching = true;
                state.switch_operation_id = Some(operation_id);
                previous
            } else {
                return Err(SwitchingError::LockError(
                    "Failed to acquire state lock".into(),
                ));
            }
        };

        let mut warnings = Vec::new();

        // Perform the switch
        let (_completion_time, duration) = {
            if let Ok(mut state) = self.current_state.write() {
                // Store previous theme
                state.previous_theme = Some(state.active_theme.clone());
                state.previous_theme_name = state.active_theme_name.clone();

                // Apply new theme
                state.active_theme = new_theme.clone();
                state.active_theme_name = theme_name.clone();
                state.last_switch_time = SystemTime::now();
                state.is_switching = false;
                state.switch_operation_id = None;

                let completion_time = SystemTime::now();
                let duration = completion_time
                    .duration_since(start_time)
                    .unwrap_or_default();

                (completion_time, duration)
            } else {
                return Err(SwitchingError::LockError(
                    "Failed to acquire state lock for switch".into(),
                ));
            }
        };

        // Now do operations that might add warnings (outside the lock)
        let switch_result = {
            // Add to history
            if self.config.enable_history {
                self.add_to_history(
                    new_theme.clone(),
                    theme_name.clone(),
                    ThemeOperation::UserSwitch,
                    None,
                );
            }

            // Persist if enabled
            if self.config.persist_theme_choice {
                if let Some(name) = &theme_name {
                    if let Err(e) = self.persist_theme_choice(name, &new_theme) {
                        nucleotide_logging::warn!(
                            error = %e,
                            "Failed to persist theme choice"
                        );
                        warnings.push(format!("Failed to persist theme: {}", e));
                    }
                }
            }

            SwitchingResult {
                success: true,
                previous_theme: previous_state.previous_theme,
                new_theme: new_theme.clone(),
                switch_duration: duration,
                operation_id,
                warnings,
            }
        };

        // Record performance metrics
        if self.config.monitor_performance {
            self.record_switch_metric(SwitchMetric {
                operation_id,
                start_time,
                completion_time: SystemTime::now(),
                duration: switch_result.switch_duration,
                from_theme: previous_state.active_theme_name,
                to_theme: theme_name,
                success: true,
                error_message: None,
            });
        }

        nucleotide_logging::info!(
            operation_id = operation_id,
            duration_ms = switch_result.switch_duration.as_millis(),
            "Theme switch completed successfully"
        );

        Ok(switch_result)
    }

    /// Switch back to previous theme
    pub fn switch_to_previous(&mut self) -> Result<SwitchingResult, SwitchingError> {
        let (previous_theme, previous_name) = {
            if let Ok(state) = self.current_state.read() {
                (
                    state.previous_theme.clone(),
                    state.previous_theme_name.clone(),
                )
            } else {
                return Err(SwitchingError::LockError("Failed to read state".into()));
            }
        };

        if let Some(theme) = previous_theme {
            self.switch_theme_immediate(theme, previous_name)
        } else {
            Err(SwitchingError::NoPreviousTheme)
        }
    }

    /// Load persisted theme choice
    pub fn load_persisted_theme(
        &mut self,
    ) -> Result<Option<(SharedString, Theme)>, SwitchingError> {
        match &self.persistence.backend {
            StorageBackend::FileSystem { config_dir } => {
                let theme_file = config_dir
                    .join(&self.config.storage_key)
                    .with_extension("json");

                if theme_file.exists() {
                    let _content = std::fs::read_to_string(&theme_file)
                        .map_err(|e| SwitchingError::PersistenceError(e.to_string()))?;

                    // This would be a proper JSON deserialization in a real implementation
                    // For now, return None to indicate no persisted theme
                    Ok(None)
                } else {
                    Ok(None)
                }
            }
            StorageBackend::Environment => {
                if let Ok(theme_name) = std::env::var(&self.config.storage_key) {
                    // This would load the actual theme - simplified for now
                    Ok(Some((theme_name.into(), Theme::dark())))
                } else {
                    Ok(None)
                }
            }
            StorageBackend::Memory => {
                // Memory storage doesn't persist across restarts
                Ok(None)
            }
            StorageBackend::Custom(storage) => match storage.load_theme(&self.config.storage_key) {
                Ok(Some((name, theme))) => Ok(Some((name.into(), theme))),
                Ok(None) => Ok(None),
                Err(e) => Err(SwitchingError::PersistenceError(e.to_string())),
            },
        }
    }

    /// Get current theme state
    pub fn get_current_state(&self) -> Result<ThemeSwitcherState, SwitchingError> {
        self.current_state
            .read()
            .map(|state| state.clone())
            .map_err(|_| SwitchingError::LockError("Failed to read current state".into()))
    }

    /// Check if switching is in progress
    pub fn is_switching(&self) -> bool {
        self.current_state
            .read()
            .map(|state| state.is_switching)
            .unwrap_or(false)
    }

    /// Get theme history
    pub fn get_theme_history(&self) -> Result<ThemeHistory, SwitchingError> {
        self.theme_history
            .read()
            .map(|history| history.clone())
            .map_err(|_| SwitchingError::LockError("Failed to read theme history".into()))
    }

    /// Navigate to previous theme in history
    pub fn history_previous(&mut self) -> Result<Option<SwitchingResult>, SwitchingError> {
        let history_entry = {
            if let Ok(mut history) = self.theme_history.write() {
                if history.current_position > 0 {
                    history.current_position -= 1;
                    Some(history.entries[history.current_position].clone())
                } else {
                    None
                }
            } else {
                return Err(SwitchingError::LockError(
                    "Failed to acquire history lock".into(),
                ));
            }
        };

        if let Some(entry) = history_entry {
            let result = self.switch_theme_immediate(entry.theme, entry.theme_name)?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Navigate to next theme in history
    pub fn history_next(&mut self) -> Result<Option<SwitchingResult>, SwitchingError> {
        let history_entry = {
            if let Ok(mut history) = self.theme_history.write() {
                if history.current_position + 1 < history.entries.len() {
                    history.current_position += 1;
                    Some(history.entries[history.current_position].clone())
                } else {
                    None
                }
            } else {
                return Err(SwitchingError::LockError(
                    "Failed to acquire history lock".into(),
                ));
            }
        };

        if let Some(entry) = history_entry {
            let result = self.switch_theme_immediate(entry.theme, entry.theme_name)?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Clear theme history
    pub fn clear_history(&mut self) -> Result<(), SwitchingError> {
        if let Ok(mut history) = self.theme_history.write() {
            history.entries.clear();
            history.current_position = 0;
            Ok(())
        } else {
            Err(SwitchingError::LockError(
                "Failed to acquire history lock".into(),
            ))
        }
    }

    /// Get performance statistics
    pub fn get_performance_stats(&self) -> SwitchingPerformanceStats {
        SwitchingPerformanceStats {
            total_switches: self.performance_monitor.total_switches,
            failed_switches: self.performance_monitor.failed_switches,
            average_switch_time: self.performance_monitor.average_switch_time,
            max_switch_time: self.performance_monitor.max_switch_time,
            success_rate: if self.performance_monitor.total_switches > 0 {
                ((self.performance_monitor.total_switches
                    - self.performance_monitor.failed_switches) as f32
                    / self.performance_monitor.total_switches as f32)
                    * 100.0
            } else {
                0.0
            },
            recent_switches: self
                .performance_monitor
                .switch_metrics
                .iter()
                .rev()
                .take(10)
                .cloned()
                .collect(),
        }
    }

    /// Configure the switcher
    pub fn configure<F>(&mut self, configurator: F)
    where
        F: FnOnce(&mut SwitchingConfig),
    {
        configurator(&mut self.config);
    }

    /// Add entry to theme history
    fn add_to_history(
        &self,
        theme: Theme,
        theme_name: Option<SharedString>,
        operation: ThemeOperation,
        description: Option<String>,
    ) {
        if let Ok(mut history) = self.theme_history.write() {
            let entry = ThemeHistoryEntry {
                theme,
                theme_name,
                timestamp: SystemTime::now(),
                operation,
                description,
            };

            // Trim history to current position if we're not at the end
            let current_pos = history.current_position;
            if current_pos < history.entries.len() {
                history.entries.truncate(current_pos + 1);
            }

            // Add new entry
            history.entries.push(entry);
            history.current_position = history.entries.len() - 1;

            // Trim to max size
            if history.entries.len() > history.max_size {
                let excess = history.entries.len() - history.max_size;
                history.entries.drain(0..excess);
                history.current_position = history.current_position.saturating_sub(excess);
            }
        }
    }

    /// Persist theme choice
    fn persist_theme_choice(
        &mut self,
        theme_name: &str,
        theme: &Theme,
    ) -> Result<(), SwitchingError> {
        match &self.persistence.backend {
            StorageBackend::FileSystem { config_dir } => {
                std::fs::create_dir_all(config_dir)
                    .map_err(|e| SwitchingError::PersistenceError(e.to_string()))?;

                let theme_file = config_dir
                    .join(&self.config.storage_key)
                    .with_extension("json");

                // This would be proper JSON serialization in a real implementation
                let content = format!("{{\"theme_name\": \"{}\"}}", theme_name);

                std::fs::write(&theme_file, content)
                    .map_err(|e| SwitchingError::PersistenceError(e.to_string()))?;

                self.persistence.last_save_time = Some(SystemTime::now());
                Ok(())
            }
            StorageBackend::Environment => {
                // SAFETY: Setting environment variables in a multi-threaded context can lead to
                // undefined behavior. This is acceptable here as theme switching is rare and
                // the environment variable is only used for theme persistence.
                unsafe {
                    std::env::set_var(&self.config.storage_key, theme_name);
                }
                Ok(())
            }
            StorageBackend::Memory => {
                // Memory storage doesn't persist
                Ok(())
            }
            StorageBackend::Custom(storage) => storage
                .save_theme(&self.config.storage_key, theme_name, theme)
                .map_err(|e| SwitchingError::PersistenceError(e.to_string())),
        }
    }

    /// Record switch performance metric
    fn record_switch_metric(&mut self, metric: SwitchMetric) {
        self.performance_monitor.switch_metrics.push(metric.clone());
        self.performance_monitor.total_switches += 1;

        if !metric.success {
            self.performance_monitor.failed_switches += 1;
        }

        // Update max switch time
        if metric.duration > self.performance_monitor.max_switch_time {
            self.performance_monitor.max_switch_time = metric.duration;
        }

        // Update average (simplified calculation)
        let total_time: Duration = self
            .performance_monitor
            .switch_metrics
            .iter()
            .map(|m| m.duration)
            .sum();

        self.performance_monitor.average_switch_time =
            total_time / self.performance_monitor.switch_metrics.len() as u32;

        // Keep only recent metrics
        if self.performance_monitor.switch_metrics.len() > 100 {
            self.performance_monitor.switch_metrics.remove(0);
        }
    }

    /// Generate unique operation ID
    fn generate_operation_id(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        SystemTime::now().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        std::thread::current().id().hash(&mut hasher);
        hasher.finish()
    }
}

impl Default for RuntimeThemeSwitcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Performance statistics for theme switching
#[derive(Debug, Clone)]
pub struct SwitchingPerformanceStats {
    /// Total number of theme switches
    pub total_switches: u64,
    /// Number of failed switches
    pub failed_switches: u64,
    /// Average time to complete a switch
    pub average_switch_time: Duration,
    /// Maximum switch time recorded
    pub max_switch_time: Duration,
    /// Switch success rate as percentage
    pub success_rate: f32,
    /// Recent switch operations (last 10)
    pub recent_switches: Vec<SwitchMetric>,
}

/// Runtime switching errors
#[derive(Debug, Clone)]
pub enum SwitchingError {
    /// No previous theme available
    NoPreviousTheme,
    /// Lock acquisition failed
    LockError(String),
    /// Persistence operation failed
    PersistenceError(String),
    /// Invalid theme data
    InvalidTheme(String),
    /// Switch operation failed
    SwitchFailed(String),
}

/// Storage errors
#[derive(Debug, Clone)]
pub enum StorageError {
    /// File operation failed
    FileError(String),
    /// Serialization failed
    SerializationError(String),
    /// Invalid data format
    InvalidData(String),
    /// Access denied
    AccessDenied,
}

impl std::fmt::Display for SwitchingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwitchingError::NoPreviousTheme => write!(f, "No previous theme available"),
            SwitchingError::LockError(msg) => write!(f, "Lock error: {}", msg),
            SwitchingError::PersistenceError(msg) => write!(f, "Persistence error: {}", msg),
            SwitchingError::InvalidTheme(msg) => write!(f, "Invalid theme: {}", msg),
            SwitchingError::SwitchFailed(msg) => write!(f, "Switch failed: {}", msg),
        }
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::FileError(msg) => write!(f, "File error: {}", msg),
            StorageError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            StorageError::InvalidData(msg) => write!(f, "Invalid data: {}", msg),
            StorageError::AccessDenied => write!(f, "Access denied"),
        }
    }
}

impl std::error::Error for SwitchingError {}
impl std::error::Error for StorageError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_switcher_creation() {
        let switcher = RuntimeThemeSwitcher::new();
        assert!(!switcher.is_switching());

        let state = switcher.get_current_state().unwrap();
        assert!(state.active_theme_name.is_some());
    }

    #[test]
    fn test_immediate_theme_switch() {
        let mut switcher = RuntimeThemeSwitcher::new();
        let light_theme = Theme::light();

        let result = switcher
            .switch_theme_immediate(light_theme.clone(), Some("light".into()))
            .unwrap();

        assert!(result.success);
        assert!(result.switch_duration > Duration::ZERO);

        let state = switcher.get_current_state().unwrap();
        assert_eq!(state.active_theme_name, Some("light".into()));
    }

    #[test]
    fn test_previous_theme_switch() {
        let mut switcher = RuntimeThemeSwitcher::new();
        let light_theme = Theme::light();

        // Switch to light theme
        switcher
            .switch_theme_immediate(light_theme, Some("light".into()))
            .unwrap();

        // Switch back to previous
        let result = switcher.switch_to_previous().unwrap();
        assert!(result.success);

        let state = switcher.get_current_state().unwrap();
        assert_eq!(state.active_theme_name, Some("dark".into()));
    }

    #[test]
    fn test_theme_history() {
        let mut switcher = RuntimeThemeSwitcher::new();
        let light_theme = Theme::light();

        // Make a switch to add to history
        switcher
            .switch_theme_immediate(light_theme, Some("light".into()))
            .unwrap();

        let history = switcher.get_theme_history().unwrap();
        assert!(!history.entries.is_empty());
        assert_eq!(
            history.entries.last().unwrap().theme_name,
            Some("light".into())
        );
    }

    #[test]
    fn test_performance_monitoring() {
        let mut switcher = RuntimeThemeSwitcher::new();
        let light_theme = Theme::light();

        // Perform some switches
        switcher
            .switch_theme_immediate(light_theme.clone(), Some("light".into()))
            .unwrap();
        switcher.switch_to_previous().unwrap();

        let stats = switcher.get_performance_stats();
        assert_eq!(stats.total_switches, 2);
        assert_eq!(stats.failed_switches, 0);
        assert_eq!(stats.success_rate, 100.0);
    }

    #[test]
    fn test_configuration() {
        let mut switcher = RuntimeThemeSwitcher::new();

        switcher.configure(|config| {
            config.persist_theme_choice = false;
            config.enable_history = false;
            config.max_history_size = 10;
        });

        assert!(!switcher.config.persist_theme_choice);
        assert!(!switcher.config.enable_history);
        assert_eq!(switcher.config.max_history_size, 10);
    }

    #[test]
    fn test_history_navigation() {
        let mut switcher = RuntimeThemeSwitcher::new();
        let light_theme = Theme::light();

        // Create some history
        switcher
            .switch_theme_immediate(light_theme.clone(), Some("light".into()))
            .unwrap();
        switcher
            .switch_theme_immediate(Theme::dark(), Some("dark".into()))
            .unwrap();

        // Navigate backwards
        let result = switcher.history_previous().unwrap();
        assert!(result.is_some());

        // Navigate forwards
        let result = switcher.history_next().unwrap();
        assert!(result.is_some());
    }
}
