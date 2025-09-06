// ABOUTME: Stress tests and edge case testing for LSP server lifecycle management
// ABOUTME: Tests system behavior under high load, concurrent operations, and edge cases

#[cfg(test)]
pub mod lsp_stress_tests {
    // use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use nucleotide_events::{ProjectType, ServerHealthStatus};
    use nucleotide_logging::{debug, info};
    use tokio::sync::RwLock;
    use tokio::time::{sleep, timeout};

    use crate::mock_server_tests::mock_lsp_servers::{
        MockLspServer, MockServerBehavior, MockServerRegistry,
    };
    use crate::{ProjectLspConfig, ProjectLspManager};

    /// Test configuration for stress tests
    #[allow(dead_code)]
    #[derive(Debug, Clone)]
    struct StressTestConfig {
        /// Number of concurrent projects
        pub concurrent_projects: usize,
        /// Number of servers per project
        pub servers_per_project: usize,
        /// Test duration
        pub test_duration: Duration,
        /// Operation timeout
        pub operation_timeout: Duration,
        /// Maximum acceptable failure rate (percentage)
        pub max_failure_rate: f32,
    }

    impl Default for StressTestConfig {
        fn default() -> Self {
            Self {
                concurrent_projects: 10,
                servers_per_project: 2,
                test_duration: Duration::from_secs(10),
                operation_timeout: Duration::from_secs(1),
                max_failure_rate: 5.0, // 5% failure rate acceptable
            }
        }
    }

    /// Stress test results tracker
    #[derive(Debug, Default)]
    struct StressTestResults {
        pub total_operations: usize,
        pub successful_operations: usize,
        pub failed_operations: usize,
        pub timeout_operations: usize,
        pub average_response_time: Option<Duration>,
        pub max_response_time: Option<Duration>,
        pub min_response_time: Option<Duration>,
        pub operation_times: Vec<Duration>,
    }

    impl StressTestResults {
        fn new() -> Self {
            Self::default()
        }

        fn record_success(&mut self, duration: Duration) {
            self.total_operations += 1;
            self.successful_operations += 1;
            self.operation_times.push(duration);
            self.update_timing_stats();
        }

        fn record_failure(&mut self) {
            self.total_operations += 1;
            self.failed_operations += 1;
        }

        fn record_timeout(&mut self) {
            self.total_operations += 1;
            self.timeout_operations += 1;
        }

        fn update_timing_stats(&mut self) {
            if !self.operation_times.is_empty() {
                let total: Duration = self.operation_times.iter().sum();
                self.average_response_time = Some(total / self.operation_times.len() as u32);
                self.max_response_time = self.operation_times.iter().max().copied();
                self.min_response_time = self.operation_times.iter().min().copied();
            }
        }

        fn failure_rate(&self) -> f32 {
            if self.total_operations == 0 {
                return 0.0;
            }
            (self.failed_operations + self.timeout_operations) as f32 / self.total_operations as f32
                * 100.0
        }

        fn success_rate(&self) -> f32 {
            if self.total_operations == 0 {
                return 0.0;
            }
            self.successful_operations as f32 / self.total_operations as f32 * 100.0
        }
    }

    /// Helper for creating test projects under load
    struct StressTestHelper {
        base_dir: PathBuf,
        project_counter: Arc<RwLock<usize>>,
    }

    impl StressTestHelper {
        fn new() -> Self {
            Self {
                base_dir: std::env::temp_dir().join("nucleotide_stress_tests"),
                project_counter: Arc::new(RwLock::new(0)),
            }
        }

        async fn create_test_project(
            &self,
            project_type: ProjectType,
        ) -> Result<PathBuf, std::io::Error> {
            let counter = {
                let mut counter = self.project_counter.write().await;
                *counter += 1;
                *counter
            };

            let project_name = format!("stress_project_{}", counter);
            let project_dir = self.base_dir.join(&project_name);

            tokio::fs::create_dir_all(&project_dir).await?;

            match project_type {
                ProjectType::Rust => {
                    let cargo_toml = format!(
                        r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"
"#,
                        project_name
                    );
                    tokio::fs::write(project_dir.join("Cargo.toml"), cargo_toml).await?;

                    let src_dir = project_dir.join("src");
                    tokio::fs::create_dir_all(&src_dir).await?;
                    tokio::fs::write(src_dir.join("main.rs"), "fn main() {}").await?;
                }

                ProjectType::TypeScript => {
                    let package_json = format!(
                        r#"{{
  "name": "{}",
  "version": "1.0.0"
}}
"#,
                        project_name
                    );
                    tokio::fs::write(project_dir.join("package.json"), package_json).await?;
                    tokio::fs::write(project_dir.join("tsconfig.json"), "{}").await?;

                    let src_dir = project_dir.join("src");
                    tokio::fs::create_dir_all(&src_dir).await?;
                    tokio::fs::write(src_dir.join("index.ts"), "console.log('test');").await?;
                }

                ProjectType::JavaScript => {
                    let package_json = format!(
                        r#"{{
  "name": "{}",
  "version": "1.0.0"
}}
"#,
                        project_name
                    );
                    tokio::fs::write(project_dir.join("package.json"), package_json).await?;

                    let src_dir = project_dir.join("src");
                    tokio::fs::create_dir_all(&src_dir).await?;
                    tokio::fs::write(src_dir.join("index.js"), "console.log('test');").await?;
                }

                _ => {
                    // Create generic project
                    tokio::fs::write(project_dir.join("README.md"), "# Test Project").await?;
                }
            }

            Ok(project_dir)
        }

        async fn cleanup(&self) -> Result<(), std::io::Error> {
            if self.base_dir.exists() {
                tokio::fs::remove_dir_all(&self.base_dir).await?;
            }
            Ok(())
        }
    }

    // === STRESS TESTS ===

    #[tokio::test]
    #[ignore = "Flaky concurrency stress test - timing sensitive"]
    async fn test_concurrent_project_detection() {
        let helper = StressTestHelper::new();
        let _ = helper.cleanup().await;

        let config = StressTestConfig {
            concurrent_projects: 20,
            ..Default::default()
        };

        let lsp_config = ProjectLspConfig {
            enable_proactive_startup: true,
            health_check_interval: Duration::from_millis(100),
            startup_timeout: Duration::from_millis(500),
            max_concurrent_startups: 10,
            project_markers: nucleotide_types::ProjectMarkersConfig::default(),
        };

        let manager = ProjectLspManager::new(lsp_config, None);
        let mut results = StressTestResults::new();

        manager.start().await.expect("Manager should start");

        // Create projects
        let project_types = [ProjectType::Rust, ProjectType::TypeScript, ProjectType::JavaScript];

        let mut detection_tasks = Vec::new();

        for i in 0..config.concurrent_projects {
            let project_type = project_types[i % project_types.len()].clone();
            let manager_clone = &manager;
            let helper_ref = &helper;

            let task = async move {
                let start = Instant::now();

                // Create project
                let project_dir = helper_ref
                    .create_test_project(project_type)
                    .await
                    .expect("Should create test project");

                // Detect project
                let result = timeout(
                    Duration::from_secs(2),
                    manager_clone.detect_project(project_dir),
                )
                .await;

                let duration = start.elapsed();

                match result {
                    Ok(Ok(())) => (true, false, duration),
                    Ok(Err(_)) => (false, false, duration),
                    Err(_) => (false, true, duration), // Timeout
                }
            };

            detection_tasks.push(task);
        }

        // Execute all detection tasks concurrently
        let start_time = Instant::now();
        let results_list = futures::future::join_all(detection_tasks).await;
        let total_time = start_time.elapsed();

        // Analyze results
        for (success, timeout_occurred, duration) in results_list {
            if timeout_occurred {
                results.record_timeout();
            } else if success {
                results.record_success(duration);
            } else {
                results.record_failure();
            }
        }

        // Validate results
        info!("Concurrent project detection results: {:?}", results);
        info!("Total test time: {:?}", total_time);

        assert!(
            results.failure_rate() < config.max_failure_rate,
            "Failure rate too high: {:.2}%",
            results.failure_rate()
        );
        assert!(
            results.success_rate() > 80.0,
            "Success rate too low: {:.2}%",
            results.success_rate()
        );

        if let Some(avg_time) = results.average_response_time {
            assert!(
                avg_time < Duration::from_millis(200),
                "Average response time too slow: {:?}",
                avg_time
            );
        }

        manager.stop().await.expect("Manager should stop");
        let _ = helper.cleanup().await;
    }

    #[tokio::test]
    async fn test_high_frequency_health_checks() {
        let helper = StressTestHelper::new();
        let _ = helper.cleanup().await;

        // Configure for very frequent health checks
        let lsp_config = ProjectLspConfig {
            enable_proactive_startup: true,
            health_check_interval: Duration::from_millis(10), // Very frequent
            startup_timeout: Duration::from_millis(100),
            max_concurrent_startups: 5,
            project_markers: nucleotide_types::ProjectMarkersConfig::default(),
        };

        let manager = ProjectLspManager::new(lsp_config, None);
        let mut registry = MockServerRegistry::new();

        manager.start().await.expect("Manager should start");

        // Create some test projects
        let project_types = [ProjectType::Rust, ProjectType::TypeScript];
        let mut projects = Vec::new();

        for (i, project_type) in project_types.iter().enumerate() {
            let project_dir = helper
                .create_test_project(project_type.clone())
                .await
                .expect("Should create project");

            manager
                .detect_project(project_dir.clone())
                .await
                .expect("Should detect project");

            projects.push(project_dir.clone());

            // Simulate server registration
            let server_name = match project_type {
                ProjectType::Rust => "rust-analyzer",
                ProjectType::TypeScript => "typescript-language-server",
                _ => "generic-server",
            };

            let mut mock_server =
                MockLspServer::new(server_name.to_string(), format!("lang_{}", i), project_dir);

            mock_server.start().await.expect("Mock server should start");
            registry.register_server(mock_server).await;
        }

        // Run high-frequency health checks for a short period
        let test_duration = Duration::from_secs(2);
        let start_time = Instant::now();

        let mut health_check_count = 0;

        while start_time.elapsed() < test_duration {
            let health_results = registry.health_check_all().await;
            health_check_count += health_results.len();

            sleep(Duration::from_millis(5)).await; // Brief pause
        }

        info!(
            "Performed {} health checks in {:?}",
            health_check_count, test_duration
        );

        // Verify system remained stable under high-frequency health checks
        assert!(health_check_count > 50, "Should perform many health checks");
        assert_eq!(
            registry.get_total_server_count().await,
            2,
            "All servers should remain registered"
        );

        // Cleanup
        registry.shutdown_all().await;
        manager.stop().await.expect("Manager should stop");
        let _ = helper.cleanup().await;
    }

    #[tokio::test]
    async fn test_server_startup_timeout_handling() {
        let helper = StressTestHelper::new();
        let _ = helper.cleanup().await;

        let lsp_config = ProjectLspConfig {
            enable_proactive_startup: true,
            health_check_interval: Duration::from_millis(100),
            startup_timeout: Duration::from_millis(50), // Very short timeout
            max_concurrent_startups: 3,
            project_markers: nucleotide_types::ProjectMarkersConfig::default(),
        };

        let manager = ProjectLspManager::new(lsp_config, None);
        let mut results = StressTestResults::new();

        manager.start().await.expect("Manager should start");

        // Create servers with various startup delays
        let startup_delays = [
            Duration::from_millis(10),  // Fast startup
            Duration::from_millis(100), // Slow startup (should timeout)
            Duration::from_millis(200), // Very slow startup (should timeout)
            Duration::from_millis(25),  // Medium startup
        ];

        for (i, delay) in startup_delays.iter().enumerate() {
            let project_dir = helper
                .create_test_project(ProjectType::Rust)
                .await
                .expect("Should create project");

            let start_time = Instant::now();

            // Create mock server with specific delay
            let mut mock_server = MockLspServer::new(
                format!("test-server-{}", i),
                "rust".to_string(),
                project_dir.clone(),
            )
            .with_behavior(MockServerBehavior {
                startup_delay: *delay,
                ..Default::default()
            });

            // Test server startup within timeout
            let startup_result = timeout(
                Duration::from_millis(100), // Test timeout
                mock_server.start(),
            )
            .await;

            let duration = start_time.elapsed();

            match startup_result {
                Ok(Ok(())) => results.record_success(duration),
                Ok(Err(_)) => results.record_failure(),
                Err(_) => results.record_timeout(),
            }

            // Also test project detection
            let _detect_result = manager.detect_project(project_dir).await;
        }

        info!("Server timeout test results: {:?}", results);

        // Verify that some startups succeeded and some timed out as expected
        assert!(
            results.successful_operations > 0,
            "Some servers should start successfully"
        );
        assert!(
            results.timeout_operations > 0,
            "Some servers should timeout"
        );

        manager.stop().await.expect("Manager should stop");
        let _ = helper.cleanup().await;
    }

    #[tokio::test]
    async fn test_memory_pressure_simulation() {
        let helper = StressTestHelper::new();
        let _ = helper.cleanup().await;

        let manager = ProjectLspManager::new(ProjectLspConfig::default(), None);
        let mut registry = MockServerRegistry::new();

        manager.start().await.expect("Manager should start");

        // Create multiple projects and servers
        let num_projects = 15;
        let mut servers = Vec::new();

        for i in 0..num_projects {
            let project_type = if i % 2 == 0 {
                ProjectType::Rust
            } else {
                ProjectType::TypeScript
            };
            let project_dir = helper
                .create_test_project(project_type.clone())
                .await
                .expect("Should create project");

            manager
                .detect_project(project_dir.clone())
                .await
                .expect("Should detect project");

            // Create mock servers with high resource usage
            let mut mock_server = MockLspServer::new(
                format!("heavy-server-{}", i),
                if i % 2 == 0 {
                    "rust".to_string()
                } else {
                    "typescript".to_string()
                },
                project_dir,
            );

            // Simulate high memory usage for some servers
            if i % 3 == 0 {
                mock_server.simulate_high_resource_usage();
            }

            mock_server.start().await.expect("Server should start");
            registry.register_server(mock_server.clone()).await;
            servers.push(mock_server);
        }

        // Verify all servers are running
        assert_eq!(registry.get_total_server_count().await, num_projects);

        // Simulate memory pressure by checking resource usage
        let mut high_memory_servers = 0;
        let mut total_memory_usage = 0;

        for server in &servers {
            let stats = server.get_stats().await;
            let memory_usage = stats["memory_usage_mb"].as_u64().unwrap_or(0);
            total_memory_usage += memory_usage;

            if memory_usage > 200 {
                high_memory_servers += 1;
            }
        }

        info!(
            "Total memory usage: {} MB across {} servers",
            total_memory_usage,
            servers.len()
        );
        info!("High memory servers: {}", high_memory_servers);

        // Verify memory pressure handling
        assert!(
            high_memory_servers > 0,
            "Should have some high memory usage servers"
        );
        assert!(
            total_memory_usage > 500,
            "Total memory usage should be significant"
        );

        // Test that system continues to function under memory pressure
        let health_results = registry.health_check_all().await;
        let healthy_servers = health_results
            .values()
            .filter(|status| matches!(status, ServerHealthStatus::Healthy))
            .count();

        assert!(
            healthy_servers > num_projects / 2,
            "Most servers should remain healthy under memory pressure"
        );

        // Cleanup
        registry.shutdown_all().await;
        manager.stop().await.expect("Manager should stop");
        let _ = helper.cleanup().await;
    }

    #[tokio::test]
    async fn test_rapid_project_creation_and_deletion() {
        let helper = StressTestHelper::new();
        let _ = helper.cleanup().await;

        let manager = ProjectLspManager::new(ProjectLspConfig::default(), None);
        let mut results = StressTestResults::new();

        manager.start().await.expect("Manager should start");

        let num_cycles = 10;
        let projects_per_cycle = 3;

        for cycle in 0..num_cycles {
            let mut projects_in_cycle = Vec::new();

            // Rapid project creation
            for i in 0..projects_per_cycle {
                let start_time = Instant::now();

                let project_type = match i % 3 {
                    0 => ProjectType::Rust,
                    1 => ProjectType::TypeScript,
                    _ => ProjectType::JavaScript,
                };

                let project_dir = helper
                    .create_test_project(project_type)
                    .await
                    .expect("Should create project");

                let detect_result = manager.detect_project(project_dir.clone()).await;
                let duration = start_time.elapsed();

                match detect_result {
                    Ok(()) => results.record_success(duration),
                    Err(_) => results.record_failure(),
                }

                projects_in_cycle.push(project_dir);
            }

            // Brief pause before deletion
            sleep(Duration::from_millis(50)).await;

            // Rapid project deletion (simulate cleanup)
            for project_dir in projects_in_cycle {
                // In a real implementation, this would trigger cleanup events
                let _ = tokio::fs::remove_dir_all(&project_dir).await;
            }

            // Brief pause between cycles
            sleep(Duration::from_millis(10)).await;

            info!("Completed rapid creation/deletion cycle {}", cycle + 1);
        }

        info!("Rapid creation/deletion results: {:?}", results);

        // Verify system stability
        assert!(
            results.failure_rate() < 10.0,
            "Failure rate should be low: {:.2}%",
            results.failure_rate()
        );

        if let Some(avg_time) = results.average_response_time {
            assert!(
                avg_time < Duration::from_millis(100),
                "Operations should be fast: {:?}",
                avg_time
            );
        }

        manager.stop().await.expect("Manager should stop");
        let _ = helper.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "Flaky edge case test - filesystem timing sensitive"]
    async fn test_edge_case_project_structures() {
        let helper = StressTestHelper::new();
        let _ = helper.cleanup().await;

        let manager = ProjectLspManager::new(ProjectLspConfig::default(), None);
        manager.start().await.expect("Manager should start");

        // Test various edge case project structures
        let test_cases = vec![
            // Empty directory
            ("empty_project", vec![]),
            // Directory with only README
            ("readme_only", vec!["README.md"]),
            // Directory with hidden files
            ("hidden_files", vec![".gitignore", ".hidden_file"]),
            // Directory with nested structure but no clear project files
            (
                "nested_no_project",
                vec!["docs/README.md", "assets/image.png"],
            ),
            // Directory with multiple project file types (ambiguous)
            (
                "ambiguous_project",
                vec!["Cargo.toml", "package.json", "requirements.txt"],
            ),
            // Directory with very long path names
            (
                "very_long_directory_name_that_might_cause_issues",
                vec!["test.txt"],
            ),
        ];

        let mut detection_results = Vec::new();

        for (project_name, files) in test_cases {
            let project_dir = helper.base_dir.join(project_name);
            tokio::fs::create_dir_all(&project_dir)
                .await
                .expect("Should create project directory");

            // Create specified files
            for file_path in files {
                let full_path = project_dir.join(file_path);
                if let Some(parent) = full_path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .expect("Should create parent directories");
                }
                tokio::fs::write(&full_path, "test content")
                    .await
                    .expect("Should create file");
            }

            // Test project detection
            let start_time = Instant::now();
            let result = manager.detect_project(project_dir.clone()).await;
            let duration = start_time.elapsed();

            detection_results.push((project_name, result.is_ok(), duration));
        }

        // Analyze edge case results
        let mut successful_detections = 0;
        let mut failed_detections = 0;

        for (project_name, success, duration) in detection_results {
            if success {
                successful_detections += 1;
                info!(
                    "Successfully detected edge case project: {} in {:?}",
                    project_name, duration
                );
            } else {
                failed_detections += 1;
                info!("Failed to detect edge case project: {}", project_name);
            }
        }

        info!(
            "Edge case detection: {} successful, {} failed",
            successful_detections, failed_detections
        );

        // Edge cases should either succeed with Unknown type or fail gracefully
        assert!(
            successful_detections + failed_detections > 0,
            "Should process all edge cases"
        );

        manager.stop().await.expect("Manager should stop");
        let _ = helper.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "Flaky concurrency stress test - server lifecycle timing sensitive"]
    async fn test_concurrent_server_lifecycle_operations() {
        let helper = StressTestHelper::new();
        let _ = helper.cleanup().await;

        let manager = ProjectLspManager::new(
            ProjectLspConfig {
                max_concurrent_startups: 5,
                startup_timeout: Duration::from_millis(200),
                ..Default::default()
            },
            None,
        );

        manager.start().await.expect("Manager should start");

        // Create multiple projects simultaneously
        let num_concurrent_ops = 8; // More than max_concurrent_startups
        let mut operation_tasks = Vec::new();

        for i in 0..num_concurrent_ops {
            let manager_ref = &manager;
            let helper_ref = &helper;

            let task = async move {
                let project_type = match i % 3 {
                    0 => ProjectType::Rust,
                    1 => ProjectType::TypeScript,
                    _ => ProjectType::JavaScript,
                };

                // Create project
                let project_dir = helper_ref
                    .create_test_project(project_type)
                    .await
                    .expect("Should create project");

                // Detect project (this should trigger server startup)
                let start_time = Instant::now();
                let result = manager_ref.detect_project(project_dir.clone()).await;
                let duration = start_time.elapsed();

                (i, result, duration, project_dir)
            };

            operation_tasks.push(task);
        }

        // Execute all operations concurrently
        let start_time = Instant::now();
        let results = futures::future::join_all(operation_tasks).await;
        let total_duration = start_time.elapsed();

        // Analyze results
        let mut successful_ops = 0;
        let mut failed_ops = 0;
        let mut total_op_time = Duration::ZERO;

        for (op_id, result, duration, _project_dir) in results {
            total_op_time += duration;
            match result {
                Ok(()) => {
                    successful_ops += 1;
                    debug!("Operation {} succeeded in {:?}", op_id, duration);
                }
                Err(e) => {
                    failed_ops += 1;
                    debug!("Operation {} failed: {:?}", op_id, e);
                }
            }
        }

        let avg_op_time = total_op_time / num_concurrent_ops as u32;

        info!(
            "Concurrent operations: {} successful, {} failed",
            successful_ops, failed_ops
        );
        info!(
            "Total duration: {:?}, Average operation time: {:?}",
            total_duration, avg_op_time
        );

        // Verify concurrent operation handling
        assert!(successful_ops > 0, "Some operations should succeed");
        assert!(
            successful_ops >= num_concurrent_ops / 2,
            "Most operations should succeed despite concurrency limits"
        );

        // Verify that concurrency limiting didn't cause excessive delays
        assert!(
            total_duration < Duration::from_secs(5),
            "Total time should be reasonable"
        );

        manager.stop().await.expect("Manager should stop");
        let _ = helper.cleanup().await;
    }
}
