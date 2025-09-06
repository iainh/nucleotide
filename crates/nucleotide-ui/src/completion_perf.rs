// ABOUTME: Performance monitoring and metrics for the completion system
// ABOUTME: Tracks timing, memory usage, and provides optimization insights

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Performance metrics for completion operations
#[derive(Debug, Clone)]
pub struct CompletionMetrics {
    /// Total number of filter operations
    pub total_filters: u64,
    /// Number of cache hits
    pub cache_hits: u64,
    /// Number of cache misses  
    pub cache_misses: u64,
    /// Number of optimized queries (using query extension)
    pub optimized_queries: u64,
    /// Average filter time in milliseconds
    pub avg_filter_time_ms: f64,
    /// Maximum filter time observed
    pub max_filter_time: Duration,
    /// Minimum filter time observed
    pub min_filter_time: Duration,
    /// Number of cancelled operations
    pub cancelled_operations: u64,
    /// Memory usage estimates
    pub estimated_memory_kb: u64,
}

impl Default for CompletionMetrics {
    fn default() -> Self {
        Self {
            total_filters: 0,
            cache_hits: 0,
            cache_misses: 0,
            optimized_queries: 0,
            avg_filter_time_ms: 0.0,
            max_filter_time: Duration::ZERO,
            min_filter_time: Duration::MAX,
            cancelled_operations: 0,
            estimated_memory_kb: 0,
        }
    }
}

impl CompletionMetrics {
    /// Calculate cache hit ratio as percentage
    pub fn cache_hit_ratio(&self) -> f64 {
        let total_cache_ops = self.cache_hits + self.cache_misses;
        if total_cache_ops == 0 {
            0.0
        } else {
            (self.cache_hits as f64 / total_cache_ops as f64) * 100.0
        }
    }

    /// Calculate optimization ratio as percentage
    pub fn optimization_ratio(&self) -> f64 {
        if self.total_filters == 0 {
            0.0
        } else {
            (self.optimized_queries as f64 / self.total_filters as f64) * 100.0
        }
    }

    /// Calculate cancellation ratio as percentage
    pub fn cancellation_ratio(&self) -> f64 {
        if self.total_filters == 0 {
            0.0
        } else {
            (self.cancelled_operations as f64 / self.total_filters as f64) * 100.0
        }
    }

    /// Reset all metrics
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Performance timer for measuring operation duration
pub struct PerformanceTimer {
    start: Instant,
    operation: String,
}

impl PerformanceTimer {
    /// Start timing an operation
    pub fn start(operation: impl Into<String>) -> Self {
        Self {
            start: Instant::now(),
            operation: operation.into(),
        }
    }

    /// Stop timing and return the duration
    pub fn stop(self) -> (String, Duration) {
        (self.operation, self.start.elapsed())
    }

    /// Stop timing and record in performance monitor
    pub fn stop_and_record(self, monitor: &mut PerformanceMonitor) {
        let (operation, duration) = self.stop();
        monitor.record_operation(&operation, duration);
    }
}

/// Rolling window for tracking recent performance data
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RollingWindow {
    values: VecDeque<Duration>,
    max_size: usize,
}

impl RollingWindow {
    fn new(max_size: usize) -> Self {
        Self {
            values: VecDeque::new(),
            max_size,
        }
    }

    fn push(&mut self, value: Duration) {
        if self.values.len() >= self.max_size {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    fn average(&self) -> Duration {
        if self.values.is_empty() {
            Duration::ZERO
        } else {
            let total_nanos: u64 = self.values.iter().map(|d| d.as_nanos() as u64).sum();
            Duration::from_nanos(total_nanos / self.values.len() as u64)
        }
    }

    #[cfg(test)]
    fn max_value(&self) -> Duration {
        self.values.iter().max().copied().unwrap_or(Duration::ZERO)
    }

    #[cfg(test)]
    fn min_value(&self) -> Duration {
        self.values.iter().min().copied().unwrap_or(Duration::ZERO)
    }
}

/// Performance monitor for completion system
pub struct PerformanceMonitor {
    /// Overall metrics
    metrics: CompletionMetrics,
    /// Recent filter times
    filter_times: RollingWindow,
    /// Performance thresholds
    slow_filter_threshold: Duration,
    warning_threshold: Duration,
    /// Whether monitoring is enabled
    enabled: bool,
}

impl PerformanceMonitor {
    /// Create a new performance monitor
    pub fn new() -> Self {
        Self {
            metrics: CompletionMetrics::default(),
            filter_times: RollingWindow::new(100), // Track last 100 operations
            slow_filter_threshold: Duration::from_millis(100),
            warning_threshold: Duration::from_millis(50),
            enabled: true,
        }
    }

    /// Create a disabled monitor (no-op)
    pub fn disabled() -> Self {
        let mut monitor = Self::new();
        monitor.enabled = false;
        monitor
    }

    /// Record a filter operation
    pub fn record_filter(&mut self, duration: Duration, was_cached: bool, was_optimized: bool) {
        if !self.enabled {
            return;
        }

        self.metrics.total_filters += 1;

        if was_cached {
            self.metrics.cache_hits += 1;
        } else {
            self.metrics.cache_misses += 1;
        }

        if was_optimized {
            self.metrics.optimized_queries += 1;
        }

        self.filter_times.push(duration);

        // Update timing statistics
        if self.metrics.max_filter_time < duration {
            self.metrics.max_filter_time = duration;
        }
        if self.metrics.min_filter_time > duration {
            self.metrics.min_filter_time = duration;
        }

        let avg_duration = self.filter_times.average();
        self.metrics.avg_filter_time_ms = avg_duration.as_secs_f64() * 1000.0;

        // Log slow operations
        if duration > self.slow_filter_threshold {
            println!(
                "⚠️  Slow completion filter: {:.2}ms (threshold: {:.2}ms)",
                duration.as_secs_f64() * 1000.0,
                self.slow_filter_threshold.as_secs_f64() * 1000.0
            );
        }
    }

    /// Record a cancelled operation
    pub fn record_cancellation(&mut self) {
        if !self.enabled {
            return;
        }
        self.metrics.cancelled_operations += 1;
    }

    /// Record a generic operation
    pub fn record_operation(&mut self, operation: &str, duration: Duration) {
        if !self.enabled {
            return;
        }

        if duration > self.warning_threshold {
            println!(
                "⚠️  Slow completion operation '{}': {:.2}ms",
                operation,
                duration.as_secs_f64() * 1000.0
            );
        }
    }

    /// Update memory usage estimate
    pub fn update_memory_usage(&mut self, items_count: usize, cache_size: usize) {
        if !self.enabled {
            return;
        }

        // Rough estimate: each item ~100 bytes, each cache entry ~200 bytes
        let items_kb = (items_count * 100) / 1024;
        let cache_kb = (cache_size * 200) / 1024;
        self.metrics.estimated_memory_kb = (items_kb + cache_kb) as u64;
    }

    /// Get current metrics
    pub fn metrics(&self) -> &CompletionMetrics {
        &self.metrics
    }

    /// Reset all metrics
    pub fn reset(&mut self) {
        self.metrics.reset();
        self.filter_times = RollingWindow::new(100);
    }

    /// Set performance thresholds
    pub fn set_thresholds(&mut self, warning: Duration, slow: Duration) {
        self.warning_threshold = warning;
        self.slow_filter_threshold = slow;
    }

    /// Enable or disable monitoring
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if performance is concerning
    pub fn is_performance_concerning(&self) -> bool {
        if !self.enabled || self.metrics.total_filters < 10 {
            return false;
        }

        // Consider performance concerning if:
        // - Average filter time > 50ms
        // - Cache hit ratio < 30%
        // - Cancellation ratio > 20%
        self.metrics.avg_filter_time_ms > 50.0
            || self.metrics.cache_hit_ratio() < 30.0
            || self.metrics.cancellation_ratio() > 20.0
    }

    /// Get performance recommendations
    pub fn get_recommendations(&self) -> Vec<String> {
        let mut recommendations = Vec::new();

        if !self.enabled || self.metrics.total_filters < 10 {
            return recommendations;
        }

        if self.metrics.avg_filter_time_ms > 50.0 {
            recommendations.push(
                "Consider reducing completion item count or optimizing fuzzy matching".to_string(),
            );
        }

        if self.metrics.cache_hit_ratio() < 30.0 {
            recommendations.push("Cache hit ratio is low - consider increasing cache size or adjusting invalidation logic".to_string());
        }

        if self.metrics.optimization_ratio() < 20.0 {
            recommendations.push("Query optimization ratio is low - check if query extension logic is working correctly".to_string());
        }

        if self.metrics.cancellation_ratio() > 20.0 {
            recommendations.push("High cancellation ratio - consider reducing debounce delay or improving filter performance".to_string());
        }

        if self.metrics.estimated_memory_kb > 10_000 {
            recommendations.push(
                "High memory usage - consider reducing cache size or completion item count"
                    .to_string(),
            );
        }

        recommendations
    }

    /// Get average filter time
    pub fn get_average_filter_time(&self) -> Duration {
        if self.filter_times.values.is_empty() {
            Duration::ZERO
        } else {
            let total_time: Duration = self.filter_times.values.iter().sum();
            total_time / self.filter_times.values.len() as u32
        }
    }

    /// Print performance summary
    pub fn print_summary(&self) {
        if !self.enabled {
            println!("Performance monitoring is disabled");
            return;
        }

        println!("📊 Completion Performance Summary:");
        println!("  Total operations: {}", self.metrics.total_filters);
        println!("  Average time: {:.2}ms", self.metrics.avg_filter_time_ms);
        println!("  Cache hit ratio: {:.1}%", self.metrics.cache_hit_ratio());
        println!(
            "  Optimization ratio: {:.1}%",
            self.metrics.optimization_ratio()
        );
        println!(
            "  Cancellation ratio: {:.1}%",
            self.metrics.cancellation_ratio()
        );
        println!("  Memory usage: {}KB", self.metrics.estimated_memory_kb);

        if self.is_performance_concerning() {
            println!("⚠️  Performance issues detected!");
            for recommendation in self.get_recommendations() {
                println!("   💡 {}", recommendation);
            }
        } else {
            println!("✅ Performance looks good!");
        }
    }
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_metrics_defaults() {
        let metrics = CompletionMetrics::default();
        assert_eq!(metrics.total_filters, 0);
        assert_eq!(metrics.cache_hit_ratio(), 0.0);
        assert_eq!(metrics.optimization_ratio(), 0.0);
        assert_eq!(metrics.cancellation_ratio(), 0.0);
    }

    #[test]
    fn test_completion_metrics_ratios() {
        let metrics = CompletionMetrics {
            total_filters: 100,
            cache_hits: 70,
            cache_misses: 30,
            optimized_queries: 40,
            cancelled_operations: 10,
            ..Default::default()
        };

        assert_eq!(metrics.cache_hit_ratio(), 70.0);
        assert_eq!(metrics.optimization_ratio(), 40.0);
        assert_eq!(metrics.cancellation_ratio(), 10.0);
    }

    #[test]
    fn test_performance_timer() {
        let timer = PerformanceTimer::start("test_operation");
        std::thread::sleep(Duration::from_millis(1));
        let (operation, duration) = timer.stop();

        assert_eq!(operation, "test_operation");
        assert!(duration >= Duration::from_millis(1));
    }

    #[test]
    fn test_rolling_window() {
        let mut window = RollingWindow::new(3);

        window.push(Duration::from_millis(10));
        window.push(Duration::from_millis(20));
        window.push(Duration::from_millis(30));

        assert_eq!(window.average(), Duration::from_millis(20));
        assert_eq!(window.max_value(), Duration::from_millis(30));
        assert_eq!(window.min_value(), Duration::from_millis(10));

        // Test overflow
        window.push(Duration::from_millis(40));
        assert_eq!(window.values.len(), 3);
        assert_eq!(window.average(), Duration::from_millis(30)); // (20+30+40)/3
    }

    #[test]
    fn test_performance_monitor() {
        let mut monitor = PerformanceMonitor::new();

        // Record some operations
        monitor.record_filter(Duration::from_millis(25), true, false);
        monitor.record_filter(Duration::from_millis(35), false, true);
        monitor.record_cancellation();

        let metrics = monitor.metrics();
        assert_eq!(metrics.total_filters, 2);
        assert_eq!(metrics.cache_hits, 1);
        assert_eq!(metrics.cache_misses, 1);
        assert_eq!(metrics.optimized_queries, 1);
        assert_eq!(metrics.cancelled_operations, 1);

        assert_eq!(metrics.cache_hit_ratio(), 50.0);
        assert_eq!(metrics.optimization_ratio(), 50.0);
        assert_eq!(metrics.cancellation_ratio(), 50.0);
    }

    #[test]
    fn test_performance_monitor_disabled() {
        let mut monitor = PerformanceMonitor::disabled();

        monitor.record_filter(Duration::from_millis(100), false, false);
        monitor.record_cancellation();

        let metrics = monitor.metrics();
        assert_eq!(metrics.total_filters, 0);
        assert_eq!(metrics.cancelled_operations, 0);
    }

    #[test]
    fn test_performance_concerning() {
        let mut monitor = PerformanceMonitor::new();

        // Not concerning with few operations
        assert!(!monitor.is_performance_concerning());

        // Add enough operations to trigger analysis
        for _ in 0..15 {
            monitor.record_filter(Duration::from_millis(100), false, false); // Very slow
        }

        assert!(monitor.is_performance_concerning());
    }

    #[test]
    fn test_recommendations() {
        let mut monitor = PerformanceMonitor::new();

        // Add slow operations with poor cache performance
        for _ in 0..15 {
            monitor.record_filter(Duration::from_millis(100), false, false);
        }

        let recommendations = monitor.get_recommendations();
        assert!(!recommendations.is_empty());
        assert!(
            recommendations
                .iter()
                .any(|r| r.contains("reducing completion item count"))
        );
        assert!(
            recommendations
                .iter()
                .any(|r| r.contains("Cache hit ratio is low"))
        );
    }
}
