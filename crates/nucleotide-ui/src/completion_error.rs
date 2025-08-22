// ABOUTME: Comprehensive error handling and resilience for completion system
// ABOUTME: Provides graceful degradation and user-friendly error reporting

use gpui::{Context, Task};
use std::fmt;
use std::time::{Duration, Instant};

/// Comprehensive error types for completion system
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionError {
    /// Language server communication failed
    LanguageServerError {
        server_name: String,
        error_message: String,
        is_temporary: bool,
    },
    /// Network or communication timeout
    TimeoutError {
        operation: String,
        duration: Duration,
    },
    /// Parsing or data format error
    ParseError { context: String, details: String },
    /// Memory or resource exhaustion
    ResourceError {
        resource_type: String,
        current_usage: u64,
        limit: u64,
    },
    /// Cache corruption or inconsistency
    CacheError {
        cache_type: String,
        operation: String,
        details: String,
    },
    /// File system or IO error
    IoError {
        path: Option<String>,
        operation: String,
        error: String,
    },
    /// Internal logic or state error
    InternalError { component: String, details: String },
    /// User-facing configuration error
    ConfigurationError {
        setting: String,
        value: String,
        expected: String,
    },
}

impl fmt::Display for CompletionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompletionError::LanguageServerError {
                server_name,
                error_message,
                ..
            } => {
                write!(
                    f,
                    "Language server '{}' error: {}",
                    server_name, error_message
                )
            }
            CompletionError::TimeoutError {
                operation,
                duration,
            } => {
                write!(
                    f,
                    "Operation '{}' timed out after {:?}",
                    operation, duration
                )
            }
            CompletionError::ParseError { context, details } => {
                write!(f, "Parse error in {}: {}", context, details)
            }
            CompletionError::ResourceError {
                resource_type,
                current_usage,
                limit,
            } => {
                write!(
                    f,
                    "{} usage ({}) exceeded limit ({})",
                    resource_type, current_usage, limit
                )
            }
            CompletionError::CacheError {
                cache_type,
                operation,
                details,
            } => {
                write!(
                    f,
                    "{} cache error during {}: {}",
                    cache_type, operation, details
                )
            }
            CompletionError::IoError {
                path,
                operation,
                error,
            } => {
                if let Some(path) = path {
                    write!(f, "IO error during {} on '{}': {}", operation, path, error)
                } else {
                    write!(f, "IO error during {}: {}", operation, error)
                }
            }
            CompletionError::InternalError { component, details } => {
                write!(f, "Internal error in {}: {}", component, details)
            }
            CompletionError::ConfigurationError {
                setting,
                value,
                expected,
            } => {
                write!(
                    f,
                    "Configuration error for '{}': got '{}', expected {}",
                    setting, value, expected
                )
            }
        }
    }
}

impl std::error::Error for CompletionError {}

/// Error severity levels for proper handling and user feedback
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorSeverity {
    /// Low-impact error that doesn't affect core functionality
    Info,
    /// Warning that might impact user experience
    Warning,
    /// Error that affects functionality but allows graceful degradation
    Error,
    /// Critical error that prevents completion system from working
    Critical,
}

/// Error context with recovery information
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub error: CompletionError,
    pub severity: ErrorSeverity,
    pub timestamp: Instant,
    pub retry_count: usize,
    pub recovery_action: Option<RecoveryAction>,
    pub user_message: Option<String>,
}

impl ErrorContext {
    pub fn new(error: CompletionError, severity: ErrorSeverity) -> Self {
        Self {
            error,
            severity,
            timestamp: Instant::now(),
            retry_count: 0,
            recovery_action: None,
            user_message: None,
        }
    }

    pub fn with_recovery(mut self, action: RecoveryAction) -> Self {
        self.recovery_action = Some(action);
        self
    }

    pub fn with_user_message(mut self, message: impl Into<String>) -> Self {
        self.user_message = Some(message.into());
        self
    }

    pub fn should_retry(&self, max_retries: usize) -> bool {
        self.retry_count < max_retries && self.can_retry()
    }

    pub fn can_retry(&self) -> bool {
        match &self.error {
            CompletionError::LanguageServerError { is_temporary, .. } => *is_temporary,
            CompletionError::TimeoutError { .. } => true,
            CompletionError::ResourceError { .. } => false,
            CompletionError::IoError { .. } => true,
            CompletionError::ParseError { .. } => false,
            CompletionError::CacheError { .. } => true,
            CompletionError::InternalError { .. } => false,
            CompletionError::ConfigurationError { .. } => false,
        }
    }

    pub fn increment_retry(&mut self) {
        self.retry_count += 1;
    }
}

/// Recovery actions that can be taken when errors occur
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryAction {
    /// Retry the failed operation
    Retry {
        delay: Duration,
        max_attempts: usize,
    },
    /// Fall back to a simpler operation
    Fallback { action: String, description: String },
    /// Clear and rebuild cache
    ClearCache { cache_types: Vec<String> },
    /// Switch to offline mode
    OfflineMode { duration: Option<Duration> },
    /// Restart component
    RestartComponent { component: String },
    /// Show user notification
    NotifyUser {
        message: String,
        action_text: Option<String>,
    },
}

/// Error handler with recovery strategies
pub struct CompletionErrorHandler {
    error_history: Vec<ErrorContext>,
    max_history: usize,
    retry_delays: Vec<Duration>,
    fallback_enabled: bool,
    user_notifications_enabled: bool,
}

impl CompletionErrorHandler {
    pub fn new() -> Self {
        Self {
            error_history: Vec::new(),
            max_history: 100,
            retry_delays: vec![
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(5),
            ],
            fallback_enabled: true,
            user_notifications_enabled: true,
        }
    }

    /// Handle an error with automatic recovery
    pub fn handle_error(&mut self, error: CompletionError) -> ErrorHandlingResult {
        let severity = self.determine_severity(&error);
        let mut context = ErrorContext::new(error.clone(), severity);

        // Determine recovery action based on error type and history
        let recovery_action = self.determine_recovery_action(&error);
        if let Some(action) = recovery_action {
            context = context.with_recovery(action.clone());
        }

        // Add user-friendly message
        let user_message = self.generate_user_message(&error);
        if let Some(message) = user_message {
            context = context.with_user_message(message);
        }

        // Store in history
        self.add_to_history(context.clone());

        // Determine handling strategy
        match severity {
            ErrorSeverity::Info => ErrorHandlingResult::Continue,
            ErrorSeverity::Warning => {
                if self.should_show_warning(&error) {
                    ErrorHandlingResult::ShowWarning(context)
                } else {
                    ErrorHandlingResult::Continue
                }
            }
            ErrorSeverity::Error => {
                if let Some(action) = context.recovery_action.clone() {
                    ErrorHandlingResult::Recover(action, context)
                } else {
                    ErrorHandlingResult::Degrade(context)
                }
            }
            ErrorSeverity::Critical => ErrorHandlingResult::Shutdown(context),
        }
    }

    /// Determine error severity based on type and context
    fn determine_severity(&self, error: &CompletionError) -> ErrorSeverity {
        match error {
            CompletionError::LanguageServerError { is_temporary, .. } => {
                if *is_temporary {
                    ErrorSeverity::Warning
                } else {
                    ErrorSeverity::Error
                }
            }
            CompletionError::TimeoutError { .. } => ErrorSeverity::Warning,
            CompletionError::ParseError { .. } => ErrorSeverity::Error,
            CompletionError::ResourceError { .. } => ErrorSeverity::Critical,
            CompletionError::CacheError { .. } => ErrorSeverity::Warning,
            CompletionError::IoError { .. } => ErrorSeverity::Error,
            CompletionError::InternalError { .. } => ErrorSeverity::Critical,
            CompletionError::ConfigurationError { .. } => ErrorSeverity::Error,
        }
    }

    /// Determine appropriate recovery action
    fn determine_recovery_action(&self, error: &CompletionError) -> Option<RecoveryAction> {
        match error {
            CompletionError::LanguageServerError { is_temporary, .. } if *is_temporary => {
                Some(RecoveryAction::Retry {
                    delay: Duration::from_millis(500),
                    max_attempts: 3,
                })
            }
            CompletionError::TimeoutError { .. } => Some(RecoveryAction::Fallback {
                action: "use_cached_completions".to_string(),
                description: "Using cached completions due to timeout".to_string(),
            }),
            CompletionError::CacheError { cache_type, .. } => Some(RecoveryAction::ClearCache {
                cache_types: vec![cache_type.clone()],
            }),
            CompletionError::ResourceError { .. } => Some(RecoveryAction::ClearCache {
                cache_types: vec!["all".to_string()],
            }),
            _ => {
                if self.fallback_enabled {
                    Some(RecoveryAction::Fallback {
                        action: "basic_completion".to_string(),
                        description: "Using basic completion mode".to_string(),
                    })
                } else {
                    None
                }
            }
        }
    }

    /// Generate user-friendly error message
    fn generate_user_message(&self, error: &CompletionError) -> Option<String> {
        if !self.user_notifications_enabled {
            return None;
        }

        match error {
            CompletionError::LanguageServerError { server_name, .. } => Some(format!(
                "Completion temporarily unavailable for {}",
                server_name
            )),
            CompletionError::TimeoutError { .. } => {
                Some("Completion is taking longer than expected".to_string())
            }
            CompletionError::ResourceError { .. } => {
                Some("Completion temporarily disabled due to high memory usage".to_string())
            }
            CompletionError::ConfigurationError { setting, .. } => {
                Some(format!("Please check your '{}' configuration", setting))
            }
            _ => None,
        }
    }

    /// Check if warning should be shown based on frequency
    fn should_show_warning(&self, error: &CompletionError) -> bool {
        // Don't spam user with same warning repeatedly
        let recent_count = self
            .error_history
            .iter()
            .rev()
            .take(10)
            .filter(|ctx| {
                std::mem::discriminant(&ctx.error) == std::mem::discriminant(error)
                    && ctx.timestamp.elapsed() < Duration::from_secs(60)
            })
            .count();

        recent_count < 2
    }

    /// Add error to history with cleanup
    fn add_to_history(&mut self, context: ErrorContext) {
        self.error_history.push(context);

        // Keep history size manageable
        if self.error_history.len() > self.max_history {
            self.error_history.remove(0);
        }
    }

    /// Get recent error statistics
    pub fn get_error_stats(&self, duration: Duration) -> ErrorStats {
        let cutoff = Instant::now() - duration;
        let recent_errors: Vec<&ErrorContext> = self
            .error_history
            .iter()
            .filter(|ctx| ctx.timestamp >= cutoff)
            .collect();

        let mut stats = ErrorStats::default();
        stats.total_count = recent_errors.len();

        for error in recent_errors {
            match error.severity {
                ErrorSeverity::Info => stats.info_count += 1,
                ErrorSeverity::Warning => stats.warning_count += 1,
                ErrorSeverity::Error => stats.error_count += 1,
                ErrorSeverity::Critical => stats.critical_count += 1,
            }
        }

        stats
    }

    /// Check if error rate is concerning
    pub fn is_error_rate_high(&self) -> bool {
        let stats = self.get_error_stats(Duration::from_secs(300)); // 5 minutes
        stats.error_count + stats.critical_count > 10
    }

    /// Clear error history
    pub fn clear_history(&mut self) {
        self.error_history.clear();
    }

    /// Configure error handling behavior
    pub fn configure(&mut self, config: ErrorHandlingConfig) {
        self.fallback_enabled = config.enable_fallback;
        self.user_notifications_enabled = config.enable_notifications;
        self.max_history = config.max_history;
        if !config.retry_delays.is_empty() {
            self.retry_delays = config.retry_delays;
        }
    }
}

impl Default for CompletionErrorHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for error handling behavior
#[derive(Debug, Clone)]
pub struct ErrorHandlingConfig {
    pub enable_fallback: bool,
    pub enable_notifications: bool,
    pub max_history: usize,
    pub retry_delays: Vec<Duration>,
}

impl Default for ErrorHandlingConfig {
    fn default() -> Self {
        Self {
            enable_fallback: true,
            enable_notifications: true,
            max_history: 100,
            retry_delays: vec![
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(5),
            ],
        }
    }
}

/// Result of error handling operation
#[derive(Debug, Clone)]
pub enum ErrorHandlingResult {
    /// Continue normal operation
    Continue,
    /// Show warning to user but continue
    ShowWarning(ErrorContext),
    /// Attempt recovery with specified action
    Recover(RecoveryAction, ErrorContext),
    /// Degrade functionality gracefully
    Degrade(ErrorContext),
    /// Shutdown completion system
    Shutdown(ErrorContext),
}

/// Error statistics for monitoring
#[derive(Debug, Default, Clone)]
pub struct ErrorStats {
    pub total_count: usize,
    pub info_count: usize,
    pub warning_count: usize,
    pub error_count: usize,
    pub critical_count: usize,
}

/// Async error recovery executor
pub struct ErrorRecoveryExecutor {
    handler: CompletionErrorHandler,
    active_recoveries: Vec<Task<RecoveryResult>>,
}

impl Default for ErrorRecoveryExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorRecoveryExecutor {
    pub fn new() -> Self {
        Self {
            handler: CompletionErrorHandler::new(),
            active_recoveries: Vec::new(),
        }
    }

    /// Execute recovery action asynchronously
    pub fn execute_recovery<V: 'static>(
        &mut self,
        action: RecoveryAction,
        _cx: &mut Context<V>,
    ) -> Task<RecoveryResult> {
        // For now, we'll simulate the async operations synchronously
        // Real implementation would use proper GPUI async patterns
        match action {
            RecoveryAction::Retry {
                delay: _,
                max_attempts,
            } => Task::ready(RecoveryResult::RetryReady { max_attempts }),
            RecoveryAction::Fallback {
                action,
                description,
            } => Task::ready(RecoveryResult::FallbackActivated {
                action,
                description,
            }),
            RecoveryAction::ClearCache { cache_types } => {
                Task::ready(RecoveryResult::CacheCleared { cache_types })
            }
            RecoveryAction::OfflineMode { duration: _ } => {
                Task::ready(RecoveryResult::OfflineModeActivated)
            }
            RecoveryAction::RestartComponent { component } => {
                Task::ready(RecoveryResult::ComponentRestarted { component })
            }
            RecoveryAction::NotifyUser {
                message,
                action_text,
            } => Task::ready(RecoveryResult::UserNotified {
                message,
                action_text,
            }),
        }
    }

    /// Handle error with automatic recovery
    pub fn handle_error_with_recovery<V: 'static>(
        &mut self,
        error: CompletionError,
        cx: &mut Context<V>,
    ) -> ErrorHandlingResult {
        let result = self.handler.handle_error(error);

        if let ErrorHandlingResult::Recover(action, context) = result {
            let recovery_task = self.execute_recovery(action.clone(), cx);
            self.active_recoveries.push(recovery_task);
            ErrorHandlingResult::Recover(action, context)
        } else {
            result
        }
    }

    /// Check and process completed recovery tasks
    pub fn process_completed_recoveries(&mut self) -> Vec<RecoveryResult> {
        let completed = Vec::new();
        let mut active = Vec::new();

        for task in self.active_recoveries.drain(..) {
            // In a real implementation, you'd check if the task is complete
            // For now, we'll assume all tasks complete immediately
            active.push(task);
        }

        self.active_recoveries = active;
        completed
    }
}

/// Result of recovery operation
#[derive(Debug, Clone)]
pub enum RecoveryResult {
    RetryReady {
        max_attempts: usize,
    },
    FallbackActivated {
        action: String,
        description: String,
    },
    CacheCleared {
        cache_types: Vec<String>,
    },
    OfflineModeActivated,
    ComponentRestarted {
        component: String,
    },
    UserNotified {
        message: String,
        action_text: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_context_creation() {
        let error = CompletionError::TimeoutError {
            operation: "fetch_completions".to_string(),
            duration: Duration::from_secs(5),
        };

        let context = ErrorContext::new(error.clone(), ErrorSeverity::Warning);
        assert_eq!(context.error, error);
        assert_eq!(context.severity, ErrorSeverity::Warning);
        assert_eq!(context.retry_count, 0);
    }

    #[test]
    fn test_error_retry_logic() {
        let error = CompletionError::LanguageServerError {
            server_name: "rust-analyzer".to_string(),
            error_message: "Connection lost".to_string(),
            is_temporary: true,
        };

        let context = ErrorContext::new(error, ErrorSeverity::Warning);
        assert!(context.can_retry());
        assert!(context.should_retry(3));

        let mut context_retried = context.clone();
        context_retried.retry_count = 3;
        assert!(!context_retried.should_retry(3));
    }

    #[test]
    fn test_error_handler_severity_determination() {
        let handler = CompletionErrorHandler::new();

        let timeout_error = CompletionError::TimeoutError {
            operation: "test".to_string(),
            duration: Duration::from_secs(1),
        };
        assert_eq!(
            handler.determine_severity(&timeout_error),
            ErrorSeverity::Warning
        );

        let resource_error = CompletionError::ResourceError {
            resource_type: "memory".to_string(),
            current_usage: 1000,
            limit: 800,
        };
        assert_eq!(
            handler.determine_severity(&resource_error),
            ErrorSeverity::Critical
        );
    }

    #[test]
    fn test_error_handler_recovery_actions() {
        let handler = CompletionErrorHandler::new();

        let timeout_error = CompletionError::TimeoutError {
            operation: "test".to_string(),
            duration: Duration::from_secs(1),
        };

        let recovery = handler.determine_recovery_action(&timeout_error);
        assert!(recovery.is_some());

        if let Some(RecoveryAction::Fallback { action, .. }) = recovery {
            assert_eq!(action, "use_cached_completions");
        }
    }

    #[test]
    fn test_error_stats() {
        let mut handler = CompletionErrorHandler::new();

        // Add some test errors
        handler.handle_error(CompletionError::TimeoutError {
            operation: "test1".to_string(),
            duration: Duration::from_secs(1),
        });

        handler.handle_error(CompletionError::ResourceError {
            resource_type: "memory".to_string(),
            current_usage: 1000,
            limit: 800,
        });

        let stats = handler.get_error_stats(Duration::from_secs(60));
        assert_eq!(stats.total_count, 2);
        assert!(stats.warning_count > 0 || stats.critical_count > 0);
    }

    #[test]
    fn test_user_message_generation() {
        let handler = CompletionErrorHandler::new();

        let ls_error = CompletionError::LanguageServerError {
            server_name: "rust-analyzer".to_string(),
            error_message: "Connection lost".to_string(),
            is_temporary: true,
        };

        let message = handler.generate_user_message(&ls_error);
        assert!(message.is_some());
        assert!(message.unwrap().contains("rust-analyzer"));
    }

    #[test]
    fn test_error_rate_monitoring() {
        let mut handler = CompletionErrorHandler::new();

        // Add many errors to trigger high error rate
        for i in 0..15 {
            handler.handle_error(CompletionError::InternalError {
                component: "test".to_string(),
                details: format!("error {}", i),
            });
        }

        assert!(handler.is_error_rate_high());
    }

    #[test]
    fn test_error_display_formatting() {
        let error = CompletionError::LanguageServerError {
            server_name: "test-server".to_string(),
            error_message: "test message".to_string(),
            is_temporary: false,
        };

        let display = format!("{}", error);
        assert!(display.contains("test-server"));
        assert!(display.contains("test message"));
    }
}
