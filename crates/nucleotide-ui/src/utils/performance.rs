// ABOUTME: Performance measurement utilities for nucleotide-ui components
// ABOUTME: Provides timing, profiling, and measurement tools for component optimization

use gpui::SharedString;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Performance timer for measuring operation duration
pub struct PerfTimer {
    name: SharedString,
    start_time: Instant,
    fields: HashMap<String, String>,
}

impl PerfTimer {
    /// Create a new performance timer
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            start_time: Instant::now(),
            fields: HashMap::new(),
        }
    }

    /// Add a field to the timer for additional context
    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    /// Start the timer and return it for method chaining
    pub fn start(self) -> TimerGuard {
        TimerGuard {
            name: self.name,
            start_time: self.start_time,
            fields: self.fields,
        }
    }
}

/// Timer guard that automatically records timing when dropped
pub struct TimerGuard {
    name: SharedString,
    start_time: Instant,
    fields: HashMap<String, String>,
}

impl Drop for TimerGuard {
    fn drop(&mut self) {
        let duration = self.start_time.elapsed();

        // Record the timing with the performance monitor
        crate::utils::with_performance_monitor(|monitor| {
            monitor.record_render_time(&self.name, duration);
        });

        // Log structured performance data
        nucleotide_logging::debug!(
            timer_name = %self.name,
            duration_ms = duration.as_millis(),
            ?self.fields,
            "Performance timer completed"
        );
    }
}

/// Performance profiler for tracking multiple measurements
#[derive(Debug, Default)]
pub struct Profiler {
    measurements: HashMap<String, Vec<Duration>>,
    active_timers: HashMap<String, Instant>,
}

impl Profiler {
    /// Create a new profiler
    pub fn new() -> Self {
        Self::default()
    }

    /// Start timing an operation
    pub fn start_timer(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.active_timers.insert(name, Instant::now());
    }

    /// Stop timing an operation and record the duration
    pub fn stop_timer(&mut self, name: &str) -> Option<Duration> {
        if let Some(start_time) = self.active_timers.remove(name) {
            let duration = start_time.elapsed();
            self.measurements
                .entry(name.to_string())
                .or_insert_with(Vec::new)
                .push(duration);
            Some(duration)
        } else {
            None
        }
    }

    /// Get statistics for a measurement
    pub fn get_stats(&self, name: &str) -> Option<ProfilerStats> {
        self.measurements.get(name).map(|durations| {
            let count = durations.len();
            let total: Duration = durations.iter().sum();
            let average = if count > 0 {
                total / count as u32
            } else {
                Duration::ZERO
            };
            let min = durations.iter().min().copied().unwrap_or(Duration::ZERO);
            let max = durations.iter().max().copied().unwrap_or(Duration::ZERO);

            ProfilerStats {
                name: name.to_string(),
                count,
                total,
                average,
                min,
                max,
            }
        })
    }

    /// Get all recorded measurements
    pub fn get_all_stats(&self) -> Vec<ProfilerStats> {
        self.measurements
            .keys()
            .filter_map(|name| self.get_stats(name))
            .collect()
    }

    /// Clear all measurements
    pub fn clear(&mut self) {
        self.measurements.clear();
        self.active_timers.clear();
    }
}

/// Statistics for a profiled operation
#[derive(Debug, Clone)]
pub struct ProfilerStats {
    pub name: String,
    pub count: usize,
    pub total: Duration,
    pub average: Duration,
    pub min: Duration,
    pub max: Duration,
}

/// Memory usage tracker (simplified implementation)
#[derive(Debug, Default)]
pub struct MemoryTracker {
    snapshots: Vec<MemoryReading>,
}

/// Memory usage reading
#[derive(Debug, Clone)]
pub struct MemoryReading {
    pub timestamp: Instant,
    pub label: String,
    pub estimated_bytes: usize,
}

impl MemoryTracker {
    /// Create a new memory tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Take a memory snapshot with a label
    pub fn snapshot(&mut self, label: impl Into<String>) {
        // Note: This is a simplified implementation
        // In a real implementation, you would use platform-specific APIs
        // or memory profiling libraries to get actual memory usage

        let reading = MemoryReading {
            timestamp: Instant::now(),
            label: label.into(),
            estimated_bytes: 0, // Would be populated with actual data
        };

        self.snapshots.push(reading);

        // Keep only recent snapshots
        if self.snapshots.len() > 100 {
            self.snapshots.remove(0);
        }
    }

    /// Get the latest memory reading
    pub fn latest(&self) -> Option<&MemoryReading> {
        self.snapshots.last()
    }

    /// Get all memory readings
    pub fn readings(&self) -> &[MemoryReading] {
        &self.snapshots
    }

    /// Clear all readings
    pub fn clear(&mut self) {
        self.snapshots.clear();
    }
}

/// Utility macros for performance measurement

/// Time a block of code and record the result
#[macro_export]
macro_rules! timed {
    ($name:expr, $block:block) => {{
        let _timer = $crate::utils::PerfTimer::new($name).start();
        $block
    }};

    ($name:expr, warn_threshold: $threshold:expr, $block:block) => {{
        let start = std::time::Instant::now();
        let result = $block;
        let duration = start.elapsed();

        if duration > $threshold {
            nucleotide_logging::warn!(
                operation = $name,
                duration_ms = duration.as_millis(),
                threshold_ms = $threshold.as_millis(),
                "Slow operation detected"
            );
        }

        $crate::utils::with_performance_monitor(|monitor| {
            monitor.record_render_time($name, duration);
        });

        result
    }};
}

/// Profile a function with automatic timing
#[macro_export]
macro_rules! profile {
    ($profiler:expr, $name:expr, $block:block) => {{
        $profiler.start_timer($name);
        let result = $block;
        $profiler.stop_timer($name);
        result
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_perf_timer() {
        let timer = PerfTimer::new("test_operation")
            .with_field("component", "Button")
            .with_field("variant", "primary");

        let _guard = timer.start();
        thread::sleep(Duration::from_millis(1));
        // Timer automatically records when guard is dropped
    }

    #[test]
    fn test_profiler() {
        let mut profiler = Profiler::new();

        profiler.start_timer("operation1");
        thread::sleep(Duration::from_millis(1));
        let duration1 = profiler.stop_timer("operation1");

        assert!(duration1.is_some());
        assert!(duration1.unwrap() >= Duration::from_millis(1));

        let stats = profiler.get_stats("operation1").unwrap();
        assert_eq!(stats.count, 1);
        assert!(stats.total >= Duration::from_millis(1));
    }

    #[test]
    fn test_profiler_multiple_measurements() {
        let mut profiler = Profiler::new();

        // Record multiple measurements
        for i in 0..3 {
            profiler.start_timer("test_op");
            thread::sleep(Duration::from_millis(1));
            profiler.stop_timer("test_op");
        }

        let stats = profiler.get_stats("test_op").unwrap();
        assert_eq!(stats.count, 3);
        assert!(stats.average >= Duration::from_millis(1));
        assert!(stats.min <= stats.average);
        assert!(stats.max >= stats.average);
    }

    #[test]
    fn test_memory_tracker() {
        let mut tracker = MemoryTracker::new();

        tracker.snapshot("initial");
        tracker.snapshot("after_allocation");

        assert_eq!(tracker.readings().len(), 2);
        assert_eq!(tracker.latest().unwrap().label, "after_allocation");
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
    fn test_profile_macro() {
        let mut profiler = Profiler::new();

        let result = profile!(profiler, "macro_test", {
            thread::sleep(Duration::from_millis(1));
            "success"
        });

        assert_eq!(result, "success");
        assert!(profiler.get_stats("macro_test").is_some());
    }
}
