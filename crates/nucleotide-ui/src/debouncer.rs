// ABOUTME: Debouncing system for completion filtering to prevent excessive API calls
// ABOUTME: Delays processing until user stops typing for a configurable duration

use gpui::{Context, Task};
use std::time::{Duration, Instant};

/// Configuration for debouncing behavior
#[derive(Debug, Clone)]
pub struct DebounceConfig {
    /// How long to wait after the last input before triggering
    pub delay: Duration,
    /// Maximum delay before forcing execution
    pub max_delay: Duration,
    /// Whether to execute immediately on first input
    pub immediate: bool,
}

impl Default for DebounceConfig {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(150),      // 150ms default delay
            max_delay: Duration::from_millis(1000), // 1s max delay
            immediate: false,
        }
    }
}

/// Debouncer for completion filtering operations
pub struct Debouncer<T> {
    /// Configuration
    config: DebounceConfig,
    /// Current pending task
    pending_task: Option<Task<()>>,
    /// Last input time
    last_input: Option<Instant>,
    /// First input time (for max delay calculation)
    first_input: Option<Instant>,
    /// Whether we've executed immediately
    executed_immediate: bool,
    /// Phantom data for the input type
    _phantom: std::marker::PhantomData<T>,
}

impl<T> Debouncer<T>
where
    T: Clone + Send + 'static,
{
    /// Create a new debouncer with default configuration
    pub fn new() -> Self {
        Self::with_config(DebounceConfig::default())
    }

    /// Create a new debouncer with custom configuration
    pub fn with_config(config: DebounceConfig) -> Self {
        Self {
            config,
            pending_task: None,
            last_input: None,
            first_input: None,
            executed_immediate: false,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Debounce a function call with input data
    pub fn debounce<F, V>(&mut self, input: T, handler: F, cx: &mut Context<V>)
    where
        F: FnOnce(T) + Send + 'static,
        V: 'static,
    {
        let now = Instant::now();

        // Track timing
        if self.first_input.is_none() {
            self.first_input = Some(now);
            self.executed_immediate = false;
        }
        self.last_input = Some(now);

        // Cancel any pending task
        self.cancel();

        // Check if we should execute immediately
        if self.config.immediate && !self.executed_immediate {
            self.executed_immediate = true;
            handler(input);
            return;
        }

        // Check if we've exceeded max delay
        if let Some(first) = self.first_input {
            if now.duration_since(first) >= self.config.max_delay {
                // Execute immediately and reset
                self.reset();
                handler(input);
                return;
            }
        }

        // Schedule delayed execution
        let delay = self.config.delay;
        self.pending_task = Some(cx.spawn(async move |_this, _cx| {
            // Wait for the delay
            gpui::Timer::after(delay).await;

            // Execute the handler
            handler(input);
        }));
    }

    /// Cancel any pending debounced operation
    pub fn cancel(&mut self) {
        self.pending_task = None;
    }

    /// Reset the debouncer state
    pub fn reset(&mut self) {
        self.cancel();
        self.last_input = None;
        self.first_input = None;
        self.executed_immediate = false;
    }

    /// Check if there's a pending operation
    pub fn is_pending(&self) -> bool {
        self.pending_task.is_some()
    }

    /// Get the time since last input
    pub fn time_since_last_input(&self) -> Option<Duration> {
        self.last_input.map(|last| last.elapsed())
    }

    /// Update the debounce configuration
    pub fn set_config(&mut self, config: DebounceConfig) {
        self.config = config;
    }
}

impl<T> Default for Debouncer<T>
where
    T: Clone + Send + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Specialized debouncer for completion queries
pub type CompletionDebouncer = Debouncer<String>;

/// Helper for creating completion-specific debouncers
pub fn create_completion_debouncer() -> CompletionDebouncer {
    CompletionDebouncer::with_config(DebounceConfig {
        delay: Duration::from_millis(150),     // Good balance for typing
        max_delay: Duration::from_millis(800), // Prevent very long delays
        immediate: false,                      // Wait for user to stop typing
    })
}

/// Fast debouncer for very responsive filtering
pub fn create_fast_debouncer() -> CompletionDebouncer {
    CompletionDebouncer::with_config(DebounceConfig {
        delay: Duration::from_millis(50),      // Very responsive
        max_delay: Duration::from_millis(300), // Short max delay
        immediate: true,                       // Execute first input immediately
    })
}

/// Slow debouncer for expensive operations
pub fn create_slow_debouncer() -> CompletionDebouncer {
    CompletionDebouncer::with_config(DebounceConfig {
        delay: Duration::from_millis(300),      // Wait longer
        max_delay: Duration::from_millis(2000), // Allow longer delays
        immediate: false,                       // Always wait
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // use std::sync::{Arc, Mutex}; // Unused for now
    use std::thread;

    #[test]
    fn test_debounce_config_defaults() {
        let config = DebounceConfig::default();
        assert_eq!(config.delay, Duration::from_millis(150));
        assert_eq!(config.max_delay, Duration::from_millis(1000));
        assert!(!config.immediate);
    }

    #[test]
    fn test_debouncer_creation() {
        let debouncer = CompletionDebouncer::new();
        assert!(!debouncer.is_pending());
        assert!(debouncer.time_since_last_input().is_none());
    }

    #[test]
    fn test_debouncer_state_management() {
        let mut debouncer = CompletionDebouncer::new();

        // Initially not pending
        assert!(!debouncer.is_pending());

        // Reset should work even when empty
        debouncer.reset();
        assert!(!debouncer.is_pending());
        assert!(debouncer.time_since_last_input().is_none());
    }

    #[test]
    fn test_time_tracking() {
        let mut debouncer = CompletionDebouncer::new();

        // Simulate input (we can't actually test the cx.spawn part in unit tests)
        let now = std::time::Instant::now();
        debouncer.last_input = Some(now);

        // Should have time since last input
        thread::sleep(Duration::from_millis(1));
        let elapsed = debouncer.time_since_last_input().unwrap();
        assert!(elapsed >= Duration::from_millis(1));
    }

    #[test]
    fn test_cancel_and_reset() {
        let mut debouncer = CompletionDebouncer::new();

        // Set some state
        debouncer.last_input = Some(Instant::now());
        debouncer.first_input = Some(Instant::now());
        debouncer.executed_immediate = true;

        // Cancel should only clear pending task
        debouncer.cancel();
        assert!(debouncer.last_input.is_some());

        // Reset should clear everything
        debouncer.reset();
        assert!(debouncer.last_input.is_none());
        assert!(debouncer.first_input.is_none());
        assert!(!debouncer.executed_immediate);
    }

    #[test]
    fn test_specialized_debouncers() {
        let completion = create_completion_debouncer();
        assert_eq!(completion.config.delay, Duration::from_millis(150));
        assert!(!completion.config.immediate);

        let fast = create_fast_debouncer();
        assert_eq!(fast.config.delay, Duration::from_millis(50));
        assert!(fast.config.immediate);

        let slow = create_slow_debouncer();
        assert_eq!(slow.config.delay, Duration::from_millis(300));
        assert!(!slow.config.immediate);
    }

    #[test]
    fn test_config_update() {
        let mut debouncer = CompletionDebouncer::new();

        let new_config = DebounceConfig {
            delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            immediate: true,
        };

        debouncer.set_config(new_config.clone());
        assert_eq!(debouncer.config.delay, new_config.delay);
        assert_eq!(debouncer.config.max_delay, new_config.max_delay);
        assert_eq!(debouncer.config.immediate, new_config.immediate);
    }
}
