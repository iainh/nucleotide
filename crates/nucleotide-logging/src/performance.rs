// ABOUTME: Performance monitoring utilities using tracing spans
// ABOUTME: Provides macros and helpers for tracking operation timing and performance metrics

use std::time::{Duration, Instant};
use tracing::{Level, Span, field, span, warn};

/// Create a performance monitoring span for timing operations
#[macro_export]
macro_rules! perf_span {
    ($name:expr) => {
        tracing::span!(tracing::Level::DEBUG, "perf", operation = $name, elapsed_ms = field::Empty)
    };
    ($name:expr, $($field:tt)*) => {
        tracing::span!(tracing::Level::DEBUG, "perf", operation = $name, elapsed_ms = field::Empty, $($field)*)
    };
}

/// Timer guard that records elapsed time when dropped
pub struct PerfTimer {
    span: Span,
    start: Instant,
    operation: String,
    warn_threshold: Option<Duration>,
}

impl PerfTimer {
    /// Create a new performance timer
    pub fn new(operation: &str) -> Self {
        let span =
            span!(Level::DEBUG, "perf_timer", operation = %operation, elapsed_ms = field::Empty);

        Self {
            span,
            start: Instant::now(),
            operation: operation.to_string(),
            warn_threshold: None,
        }
    }

    /// Set a warning threshold - operations taking longer than this will log a warning
    pub fn with_warn_threshold(mut self, threshold: Duration) -> Self {
        self.warn_threshold = Some(threshold);
        self
    }

    /// Get elapsed time so far
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Manually finish timing and record result
    pub fn finish(self) {
        // Drop will handle recording
    }
}

impl Drop for PerfTimer {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        // Note: Precision loss is acceptable for logging milliseconds
        #[allow(clippy::cast_precision_loss)]
        let elapsed_ms = elapsed.as_millis() as f64;

        // Record the elapsed time in the span
        self.span.record("elapsed_ms", elapsed_ms);

        // Check if we should warn about slow operations
        if let Some(threshold) = self.warn_threshold {
            if elapsed > threshold {
                // Note: Precision loss is acceptable for logging milliseconds
                #[allow(clippy::cast_precision_loss)]
                let threshold_ms = threshold.as_millis() as f64;
                warn!(
                    operation = %self.operation,
                    elapsed_ms = elapsed_ms,
                    threshold_ms = threshold_ms,
                    "Slow operation detected"
                );
            }
        }
    }
}

/// Convenience macro to time a block of code
#[macro_export]
macro_rules! timed {
    ($name:expr, $code:block) => {{
        let _timer = $crate::performance::PerfTimer::new($name);
        $code
    }};
    ($name:expr, warn_threshold: $threshold:expr, $code:block) => {{
        let _timer = $crate::performance::PerfTimer::new($name).with_warn_threshold($threshold);
        $code
    }};
}

/// Performance statistics aggregator
pub struct PerfStats {
    operation_counts: std::collections::HashMap<String, u64>,
    total_time: std::collections::HashMap<String, Duration>,
}

impl PerfStats {
    pub fn new() -> Self {
        Self {
            operation_counts: std::collections::HashMap::new(),
            total_time: std::collections::HashMap::new(),
        }
    }

    pub fn record_operation(&mut self, operation: &str, duration: Duration) {
        *self
            .operation_counts
            .entry(operation.to_string())
            .or_insert(0) += 1;
        *self
            .total_time
            .entry(operation.to_string())
            .or_insert(Duration::ZERO) += duration;
    }

    pub fn get_stats(&self, operation: &str) -> Option<(u64, Duration, Duration)> {
        let count = *self.operation_counts.get(operation)?;
        let total = *self.total_time.get(operation)?;
        let average = total / u32::try_from(count).unwrap_or(u32::MAX);
        Some((count, total, average))
    }

    pub fn clear(&mut self) {
        self.operation_counts.clear();
        self.total_time.clear();
    }
}

impl Default for PerfStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;
    use tracing_mock::{expect, subscriber};

    #[test]
    fn test_perf_timer() {
        let timer = PerfTimer::new("test_operation");
        thread::sleep(Duration::from_millis(10));
        let elapsed = timer.elapsed();
        assert!(elapsed >= Duration::from_millis(10));
    }

    #[test]
    fn test_perf_stats() {
        let mut stats = PerfStats::new();
        stats.record_operation("test_op", Duration::from_millis(100));
        stats.record_operation("test_op", Duration::from_millis(200));

        let (count, total, average) = stats.get_stats("test_op").unwrap();
        assert_eq!(count, 2);
        assert_eq!(total, Duration::from_millis(300));
        assert_eq!(average, Duration::from_millis(150));
    }

    #[test]
    fn test_timed_macro() {
        let result = timed!("test_macro", {
            thread::sleep(Duration::from_millis(1));
            42
        });
        assert_eq!(result, 42);
    }

    #[test]
    fn test_perf_timer_span_creation() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(expect::span().named("perf_timer"))
            .drop_span(expect::span().named("perf_timer"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let timer = PerfTimer::new("test_operation");
            // Let timer drop to trigger span recording
            drop(timer);
        });

        handle.assert_finished();
    }

    #[test]
    fn test_perf_timer_with_warn_threshold() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(expect::span().named("perf_timer"))
            .event(expect::event().with_fields(expect::msg("Slow operation detected")))
            .drop_span(expect::span().named("perf_timer"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let timer =
                PerfTimer::new("slow_operation").with_warn_threshold(Duration::from_millis(1));
            thread::sleep(Duration::from_millis(10));
            drop(timer);
        });

        handle.assert_finished();
    }

    #[test]
    fn test_perf_span_macro() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(
                expect::span()
                    .named("perf")
                    .with_fields(expect::field("operation").with_value(&"test_macro_operation")),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let _span = perf_span!("test_macro_operation");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_perf_span_macro_with_fields() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(
                expect::span().named("perf").with_fields(
                    expect::field("operation")
                        .with_value(&"complex_operation")
                        .and(expect::field("items").with_value(&42))
                        .and(expect::field("category").with_value(&"processing")),
                ),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let _span = perf_span!("complex_operation", items = 42, category = "processing");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_timed_macro_with_tracing() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(expect::span().named("perf_timer"))
            .drop_span(expect::span().named("perf_timer"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let result = timed!("timed_macro_test", {
                thread::sleep(Duration::from_millis(1));
                42
            });
            assert_eq!(result, 42);
        });

        handle.assert_finished();
    }
}
