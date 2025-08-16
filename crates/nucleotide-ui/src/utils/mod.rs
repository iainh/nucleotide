// ABOUTME: Performance monitoring and utility functions for nucleotide-ui components
// ABOUTME: Provides performance measurement, conditional compilation helpers, and common UI utilities

use std::time::{Duration, Instant};
use std::collections::HashMap;
use gpui::{App, SharedString};

pub mod performance;
pub mod focus;
pub mod keyboard;
pub mod feature_flags;
pub mod ui_helpers;
pub mod render_utils;

pub use performance::*;
pub use focus::*;
pub use keyboard::*;
pub use feature_flags::*;
pub use ui_helpers::*;
pub use render_utils::*;

/// Global performance monitoring state
static mut PERFORMANCE_MONITOR: Option<PerformanceMonitor> = None;
static INIT_MONITOR: std::sync::Once = std::sync::Once::new();

/// Initialize the performance monitoring system
pub fn init_performance_monitoring(config: PerformanceConfig) {
    INIT_MONITOR.call_once(|| {
        unsafe {
            PERFORMANCE_MONITOR = Some(PerformanceMonitor::new(config));
        }
    });
}

/// Get access to the global performance monitor
pub fn with_performance_monitor<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut PerformanceMonitor) -> R,
{
    unsafe {
        PERFORMANCE_MONITOR.as_mut().map(f)
    }
}

/// Performance monitoring configuration
#[derive(Debug, Clone)]
pub struct PerformanceConfig {
    /// Enable render time tracking
    pub enable_render_timing: bool,
    /// Enable memory usage tracking
    pub enable_memory_tracking: bool,
    /// Enable event handling timing
    pub enable_event_timing: bool,
    /// Maximum number of entries to keep in history
    pub max_history_entries: usize,
    /// Warning threshold for slow renders (in milliseconds)
    pub slow_render_threshold_ms: u64,
    /// Warning threshold for slow events (in milliseconds)
    pub slow_event_threshold_ms: u64,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            enable_render_timing: true,
            enable_memory_tracking: false, // Disabled by default for performance
            enable_event_timing: true,
            max_history_entries: 1000,
            slow_render_threshold_ms: 16, // 60 FPS target
            slow_event_threshold_ms: 100,
        }
    }
}

/// Main performance monitoring system
#[derive(Debug)]
pub struct PerformanceMonitor {
    config: PerformanceConfig,
    render_times: HashMap<SharedString, Vec<Duration>>,
    event_times: HashMap<SharedString, Vec<Duration>>,
    memory_snapshots: Vec<MemorySnapshot>,
    warnings: Vec<PerformanceWarning>,
    total_renders: u64,
    total_events: u64,
}

impl PerformanceMonitor {
    /// Create a new performance monitor
    pub fn new(config: PerformanceConfig) -> Self {
        Self {
            config,
            render_times: HashMap::new(),
            event_times: HashMap::new(),
            memory_snapshots: Vec::new(),
            warnings: Vec::new(),
            total_renders: 0,
            total_events: 0,
        }
    }

    /// Record a component render time
    pub fn record_render_time(&mut self, component_name: impl Into<SharedString>, duration: Duration) {
        if !self.config.enable_render_timing {
            return;
        }

        let name = component_name.into();
        let entry = self.render_times.entry(name.clone()).or_insert_with(Vec::new);
        
        // Keep only recent entries
        if entry.len() >= self.config.max_history_entries {
            entry.remove(0);
        }
        
        entry.push(duration);
        self.total_renders += 1;

        // Check for slow renders
        if duration.as_millis() as u64 > self.config.slow_render_threshold_ms {
            self.warnings.push(PerformanceWarning {
                warning_type: WarningType::SlowRender,
                component_name: name,
                duration,
                timestamp: Instant::now(),
                details: format!("Render took {}ms (threshold: {}ms)", 
                    duration.as_millis(), self.config.slow_render_threshold_ms),
            });
        }
    }

    /// Record an event handling time
    pub fn record_event_time(&mut self, event_name: impl Into<SharedString>, duration: Duration) {
        if !self.config.enable_event_timing {
            return;
        }

        let name = event_name.into();
        let entry = self.event_times.entry(name.clone()).or_insert_with(Vec::new);
        
        if entry.len() >= self.config.max_history_entries {
            entry.remove(0);
        }
        
        entry.push(duration);
        self.total_events += 1;

        // Check for slow events
        if duration.as_millis() as u64 > self.config.slow_event_threshold_ms {
            self.warnings.push(PerformanceWarning {
                warning_type: WarningType::SlowEvent,
                component_name: name,
                duration,
                timestamp: Instant::now(),
                details: format!("Event took {}ms (threshold: {}ms)", 
                    duration.as_millis(), self.config.slow_event_threshold_ms),
            });
        }
    }

    /// Take a memory snapshot
    pub fn take_memory_snapshot(&mut self, label: impl Into<SharedString>) {
        if !self.config.enable_memory_tracking {
            return;
        }

        // Note: This is a simplified implementation
        // In a real implementation, you might use system calls or a memory profiling library
        let snapshot = MemorySnapshot {
            label: label.into(),
            timestamp: Instant::now(),
            estimated_usage_kb: 0, // Would be populated with actual memory usage
        };

        self.memory_snapshots.push(snapshot);
        
        // Keep only recent snapshots
        if self.memory_snapshots.len() > self.config.max_history_entries {
            self.memory_snapshots.remove(0);
        }
    }

    /// Get performance statistics for a component
    pub fn get_component_stats(&self, component_name: &str) -> Option<ComponentStats> {
        self.render_times.get(component_name).map(|times| {
            let total_time: Duration = times.iter().sum();
            let count = times.len();
            let avg_time = if count > 0 { total_time / count as u32 } else { Duration::ZERO };
            let min_time = times.iter().min().copied().unwrap_or(Duration::ZERO);
            let max_time = times.iter().max().copied().unwrap_or(Duration::ZERO);

            ComponentStats {
                component_name: component_name.to_string(),
                render_count: count,
                total_render_time: total_time,
                average_render_time: avg_time,
                min_render_time: min_time,
                max_render_time: max_time,
            }
        })
    }

    /// Get all performance warnings
    pub fn get_warnings(&self) -> &[PerformanceWarning] {
        &self.warnings
    }

    /// Clear all recorded data
    pub fn clear(&mut self) {
        self.render_times.clear();
        self.event_times.clear();
        self.memory_snapshots.clear();
        self.warnings.clear();
        self.total_renders = 0;
        self.total_events = 0;
    }

    /// Get overall performance summary
    pub fn get_summary(&self) -> PerformanceSummary {
        PerformanceSummary {
            total_renders: self.total_renders,
            total_events: self.total_events,
            total_warnings: self.warnings.len(),
            tracked_components: self.render_times.len(),
            memory_snapshots: self.memory_snapshots.len(),
        }
    }
}

/// Performance warning
#[derive(Debug, Clone)]
pub struct PerformanceWarning {
    pub warning_type: WarningType,
    pub component_name: SharedString,
    pub duration: Duration,
    pub timestamp: Instant,
    pub details: String,
}

/// Types of performance warnings
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningType {
    SlowRender,
    SlowEvent,
    HighMemoryUsage,
}

/// Memory usage snapshot
#[derive(Debug, Clone)]
pub struct MemorySnapshot {
    pub label: SharedString,
    pub timestamp: Instant,
    pub estimated_usage_kb: u64,
}

/// Component performance statistics
#[derive(Debug, Clone)]
pub struct ComponentStats {
    pub component_name: String,
    pub render_count: usize,
    pub total_render_time: Duration,
    pub average_render_time: Duration,
    pub min_render_time: Duration,
    pub max_render_time: Duration,
}

/// Overall performance summary
#[derive(Debug, Clone)]
pub struct PerformanceSummary {
    pub total_renders: u64,
    pub total_events: u64,
    pub total_warnings: usize,
    pub tracked_components: usize,
    pub memory_snapshots: usize,
}

/// Macro for timing component renders
#[macro_export]
macro_rules! time_render {
    ($component_name:expr, $block:block) => {{
        let start = std::time::Instant::now();
        let result = $block;
        let duration = start.elapsed();
        
        $crate::utils::with_performance_monitor(|monitor| {
            monitor.record_render_time($component_name, duration);
        });
        
        result
    }};
}

/// Macro for timing event handling
#[macro_export]
macro_rules! time_event {
    ($event_name:expr, $block:block) => {{
        let start = std::time::Instant::now();
        let result = $block;
        let duration = start.elapsed();
        
        $crate::utils::with_performance_monitor(|monitor| {
            monitor.record_event_time($event_name, duration);
        });
        
        result
    }};
}

/// Utility function to initialize all utils systems
pub fn init_utils(cx: &mut App) {
    // Initialize performance monitoring
    let perf_config = PerformanceConfig::default();
    init_performance_monitoring(perf_config);
    
    // Initialize focus management
    init_focus_management(cx);
    
    // Initialize feature flags if not already done
    init_feature_flags();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_performance_monitor_creation() {
        let config = PerformanceConfig::default();
        let monitor = PerformanceMonitor::new(config);
        
        assert_eq!(monitor.total_renders, 0);
        assert_eq!(monitor.total_events, 0);
        assert!(monitor.warnings.is_empty());
    }

    #[test]
    fn test_render_time_recording() {
        let mut monitor = PerformanceMonitor::new(PerformanceConfig::default());
        
        monitor.record_render_time("Button", Duration::from_millis(5));
        monitor.record_render_time("Button", Duration::from_millis(10));
        
        assert_eq!(monitor.total_renders, 2);
        
        let stats = monitor.get_component_stats("Button").unwrap();
        assert_eq!(stats.render_count, 2);
        assert_eq!(stats.min_render_time, Duration::from_millis(5));
        assert_eq!(stats.max_render_time, Duration::from_millis(10));
    }

    #[test]
    fn test_slow_render_warning() {
        let mut config = PerformanceConfig::default();
        config.slow_render_threshold_ms = 10;
        let mut monitor = PerformanceMonitor::new(config);
        
        monitor.record_render_time("SlowComponent", Duration::from_millis(20));
        
        assert_eq!(monitor.warnings.len(), 1);
        assert_eq!(monitor.warnings[0].warning_type, WarningType::SlowRender);
    }

    #[test]
    fn test_performance_macros() {
        // Initialize a local monitor for testing
        init_performance_monitoring(PerformanceConfig::default());
        
        let result = time_render!("TestComponent", {
            std::thread::sleep(Duration::from_millis(1));
            42
        });
        
        assert_eq!(result, 42);
        
        // The timing should have been recorded
        // Note: In real tests, you'd need better isolation of the global state
    }
}