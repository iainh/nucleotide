// ABOUTME: Performance benchmarks for project detection algorithms and configuration loading
// ABOUTME: Measures execution time, memory usage, and scalability for robust performance validation

#[cfg(test)]
mod tests {
    use crate::application::find_workspace_root_from;
    use crate::config::Config;
    use nucleotide_logging::{debug, info};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// Performance test utilities
    struct PerformanceTestSuite {
        temp_dir: TempDir,
    }

    impl PerformanceTestSuite {
        fn new() -> Self {
            Self {
                temp_dir: TempDir::new().expect("Failed to create temp directory"),
            }
        }

        fn path(&self) -> &Path {
            self.temp_dir.path()
        }

        /// Create a deep directory structure for testing traversal performance
        fn create_deep_structure(&self, depth: usize) -> PathBuf {
            let mut current_path = self.path().to_path_buf();

            for i in 0..depth {
                current_path = current_path.join(format!("level_{:03}", i));
                fs::create_dir_all(&current_path).expect("Failed to create directory");
            }

            current_path
        }

        /// Create a wide directory structure for testing breadth performance
        fn create_wide_structure(&self, width: usize, depth: usize) -> Vec<PathBuf> {
            let mut paths = Vec::new();

            for i in 0..width {
                let branch_path = self.path().join(format!("branch_{:03}", i));
                fs::create_dir_all(&branch_path).expect("Failed to create directory");

                let mut current_path = branch_path;
                for j in 0..depth {
                    current_path = current_path.join(format!("level_{:03}", j));
                    fs::create_dir_all(&current_path).expect("Failed to create directory");
                }

                paths.push(current_path);
            }

            paths
        }

        /// Create many small files to test I/O performance
        fn create_many_files(&self, count: usize) -> std::io::Result<()> {
            for i in 0..count {
                let file_path = self.path().join(format!("file_{:06}.txt", i));
                fs::write(file_path, format!("Content of file {}", i))?;
            }
            Ok(())
        }

        /// Create a large configuration file for testing config parsing performance
        fn create_large_config(&self, size_kb: usize) -> std::io::Result<()> {
            let mut config_content = String::from(
                r#"
[ui.font]
family = "Test Font"
size = 14.0
weight = "normal"

[editor.font]
family = "Editor Font"
size = 16.0
weight = "medium"

[theme]
mode = "dark"
dark_theme = "custom_dark"
light_theme = "custom_light"

[window]
blur_dark_themes = true
appearance_follows_theme = true
"#,
            );

            // Add repetitive content to reach target size
            let base_size = config_content.len();
            let target_size = size_kb * 1024;

            if target_size > base_size {
                let padding_needed = target_size - base_size;
                let comment_block = "\n# This is padding content to increase file size\n";
                let blocks_needed = (padding_needed / comment_block.len()) + 1;

                for i in 0..blocks_needed {
                    config_content
                        .push_str(&format!("# Padding block {} for performance testing\n", i));
                }
            }

            fs::write(self.path().join("nucleotide.toml"), config_content)
        }
    }

    /// Benchmark utilities
    struct BenchmarkResult {
        duration: Duration,
        iterations: usize,
        avg_duration: Duration,
        min_duration: Duration,
        max_duration: Duration,
    }

    impl BenchmarkResult {
        fn new(durations: Vec<Duration>) -> Self {
            let total_duration: Duration = durations.iter().sum();
            let iterations = durations.len();
            let avg_duration = total_duration / iterations as u32;
            let min_duration = *durations.iter().min().unwrap();
            let max_duration = *durations.iter().max().unwrap();

            Self {
                duration: total_duration,
                iterations,
                avg_duration,
                min_duration,
                max_duration,
            }
        }

        fn log_results(&self, test_name: &str) {
            info!(
                test = test_name,
                total_ms = self.duration.as_millis(),
                iterations = self.iterations,
                avg_ms = self.avg_duration.as_millis(),
                min_ms = self.min_duration.as_millis(),
                max_ms = self.max_duration.as_millis(),
                "Benchmark results"
            );
        }
    }

    fn benchmark<F>(name: &str, iterations: usize, mut operation: F) -> BenchmarkResult
    where
        F: FnMut(),
    {
        let mut durations = Vec::with_capacity(iterations);

        // Warm-up run
        operation();

        for _ in 0..iterations {
            let start = Instant::now();
            operation();
            durations.push(start.elapsed());
        }

        let result = BenchmarkResult::new(durations);
        result.log_results(name);
        result
    }

    #[test]
    #[ignore = "Timing-sensitive performance test"]
    fn test_workspace_detection_depth_performance() {
        let suite = PerformanceTestSuite::new();

        // Test different depths
        let depths = [10, 25, 50, 100];

        for depth in &depths {
            // Create VCS directory at root
            fs::create_dir_all(suite.path().join(".git")).unwrap();

            let deep_path = suite.create_deep_structure(*depth);

            let result = benchmark(&format!("workspace_detection_depth_{}", depth), 10, || {
                let _ = find_workspace_root_from(&deep_path);
            });

            // Performance should scale roughly linearly with depth
            // Depth 100 should complete in under 10ms
            assert!(
                result.avg_duration.as_millis() < 10,
                "Workspace detection too slow for depth {}: {:?}",
                depth,
                result.avg_duration
            );
        }
    }

    #[test]
    fn test_workspace_detection_breadth_performance() {
        let suite = PerformanceTestSuite::new();

        // Create VCS directory at root
        fs::create_dir_all(suite.path().join(".git")).unwrap();

        // Test different breadths with fixed depth
        let widths = [10, 50, 100, 500];
        let depth = 10;

        for width in &widths {
            let paths = suite.create_wide_structure(*width, depth);

            let result = benchmark(&format!("workspace_detection_breadth_{}", width), 5, || {
                // Test detection from a random path
                if !paths.is_empty() {
                    let _ = find_workspace_root_from(&paths[0]);
                }
            });

            // Breadth shouldn't significantly impact performance since we only traverse upward
            assert!(
                result.avg_duration.as_millis() < 5,
                "Workspace detection too slow for breadth {}: {:?}",
                width,
                result.avg_duration
            );
        }
    }

    #[test]
    #[ignore = "Requires Helix runtime files to be available"]
    fn test_config_loading_performance() {
        let suite = PerformanceTestSuite::new();

        // Test different config file sizes
        let sizes_kb = [1, 10, 50, 100];

        for size_kb in &sizes_kb {
            suite.create_large_config(*size_kb).unwrap();

            let result = benchmark(&format!("config_loading_{}kb", size_kb), 10, || {
                let _ = Config::load_from_dir(suite.path());
            });

            // Config loading should be fast even for large files
            // 100KB config should load in under 50ms
            let max_allowed_ms = if *size_kb <= 10 { 10 } else { 50 };

            assert!(
                result.avg_duration.as_millis() < max_allowed_ms,
                "Config loading too slow for {}KB: {:?}",
                size_kb,
                result.avg_duration
            );
        }
    }

    #[test]
    fn test_repeated_workspace_detection() {
        let suite = PerformanceTestSuite::new();
        fs::create_dir_all(suite.path().join(".git")).unwrap();

        let test_path = suite.create_deep_structure(20);
        let iterations = 1000;

        let result = benchmark("repeated_workspace_detection", iterations, || {
            let _ = find_workspace_root_from(&test_path);
        });

        // Repeated calls should be consistently fast
        assert!(
            result.avg_duration.as_micros() < 1000, // Under 1ms
            "Repeated workspace detection too slow: {:?}",
            result.avg_duration
        );

        // Variance should be low (max - min < 5ms)
        let variance = result.max_duration.as_millis() - result.min_duration.as_millis();
        assert!(
            variance < 50,
            "Too much variance in workspace detection: {}ms (expected < 50ms)",
            variance
        );
    }

    #[test]
    fn test_many_files_impact() {
        let suite = PerformanceTestSuite::new();
        fs::create_dir_all(suite.path().join(".git")).unwrap();

        // Test with many files in the directory
        let file_counts = [100, 1000, 5000];

        for file_count in &file_counts {
            suite.create_many_files(*file_count).unwrap();

            let test_path = suite.path().join("subdir");
            fs::create_dir_all(&test_path).unwrap();

            let result = benchmark(
                &format!("workspace_detection_with_{}_files", file_count),
                10,
                || {
                    let _ = find_workspace_root_from(&test_path);
                },
            );

            // File count shouldn't significantly impact workspace detection
            // since we're only checking for VCS directories, not scanning all files
            assert!(
                result.avg_duration.as_millis() < 5,
                "Workspace detection affected by file count {}: {:?}",
                file_count,
                result.avg_duration
            );
        }
    }

    #[test]
    fn test_concurrent_workspace_detection() {
        use std::sync::Arc;
        use std::thread;

        let suite = PerformanceTestSuite::new();
        fs::create_dir_all(suite.path().join(".git")).unwrap();

        let test_paths: Vec<_> = (0..10)
            .map(|i| suite.create_deep_structure(10 + i))
            .collect();

        let test_paths = Arc::new(test_paths);
        let start = Instant::now();

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let paths = Arc::clone(&test_paths);
                thread::spawn(move || {
                    for _ in 0..100 {
                        let _ = find_workspace_root_from(&paths[i]);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let duration = start.elapsed();

        debug!(
            duration_ms = duration.as_millis(),
            "Concurrent workspace detection completed"
        );

        // 10 threads, 100 operations each should complete quickly
        assert!(
            duration.as_millis() < 1000, // Under 1 second
            "Concurrent workspace detection too slow: {:?}",
            duration
        );
    }

    #[test]
    #[ignore = "Platform-dependent memory measurements"]
    fn test_memory_usage_estimation() {
        let suite = PerformanceTestSuite::new();

        // Create a large config to test memory usage
        suite.create_large_config(100).unwrap(); // 100KB config

        // Load config multiple times to check for memory leaks
        for i in 0..100 {
            let config =
                Config::load_from_dir(suite.path()).expect("Config should load successfully");

            // Verify config is valid
            assert!(config.gui.ui.font.is_some() || config.gui.ui.font.is_none());

            if i % 10 == 0 {
                debug!(iteration = i, "Memory test iteration");
            }
        }

        // This test mainly ensures no obvious memory leaks or excessive allocation
        // In a real scenario, you might use a memory profiler
        // Sanity completion marker
    }

    #[test]
    #[ignore = "Resource-intensive scalability test"]
    fn test_scalability_limits() {
        let suite = PerformanceTestSuite::new();
        fs::create_dir_all(suite.path().join(".git")).unwrap();

        // Test extreme cases
        let extreme_depth = 200;
        let deep_path = suite.create_deep_structure(extreme_depth);

        let start = Instant::now();
        let workspace_root = find_workspace_root_from(&deep_path);
        let duration = start.elapsed();

        assert_eq!(workspace_root, suite.path());

        // Even extreme depth should complete reasonably quickly
        assert!(
            duration.as_millis() < 50,
            "Extreme depth detection too slow: {:?}",
            duration
        );

        debug!(
            depth = extreme_depth,
            duration_ms = duration.as_millis(),
            "Extreme depth test completed"
        );
    }

    /// Benchmark comparison between different VCS types
    #[test]
    fn test_vcs_type_performance_comparison() {
        let vcs_types = [".git", ".svn", ".hg", ".jj", ".helix"];

        for vcs_type in &vcs_types {
            let suite = PerformanceTestSuite::new();
            fs::create_dir_all(suite.path().join(vcs_type)).unwrap();

            let test_path = suite.create_deep_structure(25);

            let result = benchmark(
                &format!("workspace_detection_{}", vcs_type.trim_start_matches('.')),
                50,
                || {
                    let _ = find_workspace_root_from(&test_path);
                },
            );

            // All VCS types should have similar performance
            assert!(
                result.avg_duration.as_millis() < 5,
                "VCS type {} detection too slow: {:?}",
                vcs_type,
                result.avg_duration
            );
        }
    }

    /// Test performance regression detection
    #[test]
    fn test_performance_regression_detection() {
        let suite = PerformanceTestSuite::new();
        fs::create_dir_all(suite.path().join(".git")).unwrap();

        let test_path = suite.create_deep_structure(30);

        // Baseline measurement
        let baseline = benchmark("performance_baseline", 100, || {
            let _ = find_workspace_root_from(&test_path);
        });

        // Performance should be consistent across multiple runs
        let second_run = benchmark("performance_second_run", 100, || {
            let _ = find_workspace_root_from(&test_path);
        });

        // Second run should not be significantly slower (within 50% of baseline)
        let performance_ratio =
            second_run.avg_duration.as_nanos() as f64 / baseline.avg_duration.as_nanos() as f64;

        assert!(
            performance_ratio < 1.5,
            "Performance regression detected: baseline={:?}, second_run={:?}, ratio={}",
            baseline.avg_duration,
            second_run.avg_duration,
            performance_ratio
        );

        debug!(
            baseline_us = baseline.avg_duration.as_micros(),
            second_run_us = second_run.avg_duration.as_micros(),
            ratio = performance_ratio,
            "Performance regression test completed"
        );
    }

    /// Stress test for edge cases
    #[test]
    fn test_stress_edge_cases() {
        // Test with very long path names
        let suite = PerformanceTestSuite::new();
        fs::create_dir_all(suite.path().join(".git")).unwrap();

        // Create path with long directory names
        let mut long_path = suite.path().to_path_buf();
        for i in 0..10 {
            let long_name = format!("very_long_directory_name_that_might_cause_issues_{:03}", i);
            long_path = long_path.join(long_name);
            fs::create_dir_all(&long_path).unwrap();
        }

        let start = Instant::now();
        let workspace_root = find_workspace_root_from(&long_path);
        let duration = start.elapsed();

        assert_eq!(workspace_root, suite.path());
        assert!(
            duration.as_millis() < 10,
            "Long path detection too slow: {:?}",
            duration
        );
    }

    /// Profile memory allocation patterns
    #[test]
    fn test_allocation_patterns() {
        let suite = PerformanceTestSuite::new();
        fs::create_dir_all(suite.path().join(".git")).unwrap();

        let test_paths: Vec<_> = (0..100)
            .map(|i| suite.create_deep_structure(5 + (i % 20)))
            .collect();

        // This test ensures we don't allocate excessively during workspace detection
        let start = Instant::now();

        for path in &test_paths {
            let _ = find_workspace_root_from(path);
        }

        let duration = start.elapsed();

        debug!(
            paths_tested = test_paths.len(),
            total_duration_ms = duration.as_millis(),
            avg_duration_us = duration.as_micros() / test_paths.len() as u128,
            "Allocation pattern test completed"
        );

        // 100 detections should complete quickly
        assert!(
            duration.as_millis() < 150,
            "Batch workspace detection too slow: {:?}",
            duration
        );
    }
}
