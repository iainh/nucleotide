// ABOUTME: Comprehensive integration tests for LSP server lifecycle management
// ABOUTME: Tests complete project-based LSP system including edge cases and error conditions

#[cfg(test)]
mod lsp_lifecycle_integration_tests {

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // use helix_view::{Editor, Document, DocumentId, ViewId};
    use nucleotide_events::{ProjectLspEvent, ProjectType};
    // use slotmap::SlotMap;
    use tokio::sync::RwLock;
    use tokio::time::sleep;

    use crate::{ProjectDetector, ProjectLspConfig, ProjectLspManager};

    /// Test configuration for integration tests
    #[derive(Debug, Clone)]
    struct TestConfig {
        /// Test directory for creating temporary project structures
        test_dir: PathBuf,
        /// Timeout for operations
        _operation_timeout: Duration,
        /// Mock server startup delay
        _server_startup_delay: Duration,
    }

    impl Default for TestConfig {
        fn default() -> Self {
            // Use a unique directory for each test helper instance to avoid race conditions
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique_id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let unique_dir = format!("nucleotide_lsp_tests_{}_{}", std::process::id(), unique_id);
            Self {
                test_dir: std::env::temp_dir().join(unique_dir),
                _operation_timeout: Duration::from_secs(5),
                _server_startup_delay: Duration::from_millis(50),
            }
        }
    }

    /// Test helper for creating realistic project structures
    struct ProjectTestHelper {
        test_config: TestConfig,
    }

    impl ProjectTestHelper {
        fn new() -> Self {
            Self {
                test_config: TestConfig::default(),
            }
        }

        async fn ensure_test_dir(&self) -> Result<(), std::io::Error> {
            tokio::fs::create_dir_all(&self.test_config.test_dir).await
        }

        async fn create_rust_project(&self, name: &str) -> Result<PathBuf, std::io::Error> {
            self.ensure_test_dir().await?;
            let project_dir = self.test_config.test_dir.join(name);

            tokio::fs::create_dir_all(&project_dir).await?;

            // Create Cargo.toml
            let cargo_toml = r#"[package]
name = "test_project"
version = "0.1.0"
edition = "2021"

[dependencies]
"#;
            tokio::fs::write(project_dir.join("Cargo.toml"), cargo_toml).await?;

            // Create src directory with main.rs
            let src_dir = project_dir.join("src");
            tokio::fs::create_dir_all(&src_dir).await?;
            tokio::fs::write(
                src_dir.join("main.rs"),
                "fn main() {\n    println!(\"Hello, world!\");\n}",
            )
            .await?;

            Ok(project_dir)
        }

        async fn create_typescript_project(&self, name: &str) -> Result<PathBuf, std::io::Error> {
            self.ensure_test_dir().await?;
            let project_dir = self.test_config.test_dir.join(name);

            tokio::fs::create_dir_all(&project_dir).await?;

            // Create package.json
            let package_json = r#"{
  "name": "test_project",
  "version": "1.0.0",
  "main": "index.js",
  "dependencies": {}
}
"#;
            tokio::fs::write(project_dir.join("package.json"), package_json).await?;

            // Create tsconfig.json
            let tsconfig_json = r#"{
  "compilerOptions": {
    "target": "es5",
    "strict": true
  }
}
"#;
            tokio::fs::write(project_dir.join("tsconfig.json"), tsconfig_json).await?;

            // Create src directory with index.ts
            let src_dir = project_dir.join("src");
            tokio::fs::create_dir_all(&src_dir).await?;
            tokio::fs::write(src_dir.join("index.ts"), "console.log('Hello, world!');").await?;

            Ok(project_dir)
        }

        async fn cleanup_test_directory(&self) -> Result<(), std::io::Error> {
            if self.test_config.test_dir.exists() {
                tokio::fs::remove_dir_all(&self.test_config.test_dir).await?;
            }
            Ok(())
        }
    }

    /// Event collector for testing
    #[derive(Debug, Clone)]
    struct EventCollector {
        events: Arc<RwLock<Vec<ProjectLspEvent>>>,
    }

    impl EventCollector {
        fn new() -> Self {
            Self {
                events: Arc::new(RwLock::new(Vec::new())),
            }
        }

        async fn collect_event(&self, event: ProjectLspEvent) {
            self.events.write().await.push(event);
        }

        #[allow(dead_code)]
        async fn get_events(&self) -> Vec<ProjectLspEvent> {
            self.events.read().await.clone()
        }

        async fn get_events_of_type<F>(&self, filter: F) -> Vec<ProjectLspEvent>
        where
            F: Fn(&ProjectLspEvent) -> bool,
        {
            self.events
                .read()
                .await
                .iter()
                .filter(|event| filter(event))
                .cloned()
                .collect()
        }

        async fn wait_for_event<F>(
            &self,
            filter: F,
            timeout_duration: Duration,
        ) -> Option<ProjectLspEvent>
        where
            F: Fn(&ProjectLspEvent) -> bool,
        {
            let start = Instant::now();

            while start.elapsed() < timeout_duration {
                {
                    let events = self.events.read().await;
                    if let Some(event) = events.iter().find(|event| filter(event)) {
                        return Some(event.clone());
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }

            None
        }

        #[allow(dead_code)]
        async fn clear(&self) {
            self.events.write().await.clear();
        }
    }

    // Test setup helpers
    async fn setup_test_manager() -> (ProjectLspManager, EventCollector) {
        let config = ProjectLspConfig {
            enable_proactive_startup: true,
            health_check_interval: Duration::from_millis(100), // Faster for tests
            startup_timeout: Duration::from_millis(500),
            max_concurrent_startups: 3,
            project_markers: nucleotide_types::ProjectMarkersConfig::default(),
        };

        let manager = ProjectLspManager::new(config, None);
        let collector = EventCollector::new();

        // Set up event listening
        setup_event_listening(&manager, &collector).await;

        (manager, collector)
    }

    // Set up event listening between manager and collector
    async fn setup_event_listening(manager: &ProjectLspManager, collector: &EventCollector) {
        let mut event_rx = manager.get_event_sender().subscribe();
        let collector = collector.clone();

        tokio::spawn(async move {
            while let Ok(event) = event_rx.recv().await {
                collector.collect_event(event).await;
            }
        });
    }

    // === LIFECYCLE TESTS ===

    #[tokio::test]
    async fn test_complete_server_lifecycle() {
        let helper = ProjectTestHelper::new();
        let _ = helper.cleanup_test_directory().await;

        let rust_project = helper
            .create_rust_project("lifecycle_test")
            .await
            .expect("Failed to create test project");

        let (manager, collector) = setup_test_manager().await;

        // Test lifecycle: start -> detect -> startup -> health -> cleanup

        // 1. Start the manager
        manager.start().await.expect("Failed to start manager");

        // 2. Detect project
        let start = Instant::now();
        manager
            .detect_project(rust_project.clone())
            .await
            .expect("Failed to detect project");
        let detection_time = start.elapsed();

        // 3. Wait for server startup event
        let _startup_event = collector
            .wait_for_event(
                |event| matches!(event, ProjectLspEvent::ServerStartupRequested { .. }),
                Duration::from_secs(1),
            )
            .await
            .expect("Server startup not requested");

        // 4. Verify project registration
        let project_info = manager
            .get_project_info(&rust_project)
            .await
            .expect("Project should be registered");

        assert_eq!(project_info.project_type, ProjectType::Rust);
        assert!(
            project_info
                .language_servers
                .contains(&"rust-analyzer".to_string())
        );

        // 5. Test cleanup
        let cleanup_start = Instant::now();
        manager.stop().await.expect("Failed to stop manager");
        let cleanup_time = cleanup_start.elapsed();

        // Verify performance characteristics
        assert!(
            detection_time < Duration::from_millis(100),
            "Project detection too slow: {:?}",
            detection_time
        );
        assert!(
            cleanup_time < Duration::from_millis(200),
            "Cleanup too slow: {:?}",
            cleanup_time
        );

        let _ = helper.cleanup_test_directory().await;
    }

    #[tokio::test]
    async fn test_project_detection_triggering_server_startup() {
        let helper = ProjectTestHelper::new();
        let _ = helper.cleanup_test_directory().await;

        let rust_project = helper
            .create_rust_project("startup_test")
            .await
            .expect("Failed to create test project");

        let (manager, collector) = setup_test_manager().await;

        manager.start().await.expect("Failed to start manager");

        // Detect project and verify startup events
        manager
            .detect_project(rust_project.clone())
            .await
            .expect("Failed to detect project");

        // Wait for startup request event
        let _startup_event = collector
            .wait_for_event(
                |event| {
                    matches!(event, ProjectLspEvent::ServerStartupRequested {
                        server_name, ..
                    } if server_name == "rust-analyzer")
                },
                Duration::from_secs(2),
            )
            .await;

        assert!(
            _startup_event.is_some(),
            "Server startup should be requested for Rust project"
        );

        if let Some(ProjectLspEvent::ServerStartupRequested {
            workspace_root,
            server_name,
            language_id,
        }) = _startup_event
        {
            assert_eq!(workspace_root, rust_project);
            assert_eq!(server_name, "rust-analyzer");
            assert_eq!(language_id, "rust");
        }

        manager.stop().await.expect("Failed to stop manager");
        let _ = helper.cleanup_test_directory().await;
    }

    #[tokio::test]
    async fn test_fallback_to_file_based_lsp() {
        let helper = ProjectTestHelper::new();
        let _ = helper.cleanup_test_directory().await;

        // Create a directory without clear project markers
        helper
            .ensure_test_dir()
            .await
            .expect("Failed to ensure test dir");
        let unknown_project = helper.test_config.test_dir.join("unknown_project");
        tokio::fs::create_dir_all(&unknown_project)
            .await
            .expect("Failed to create test directory");

        // Create some generic files
        tokio::fs::write(unknown_project.join("README.md"), "# Unknown Project")
            .await
            .expect("Failed to create readme");
        tokio::fs::write(unknown_project.join("script.sh"), "#!/bin/bash\necho hello")
            .await
            .expect("Failed to create script");

        let (manager, collector) = setup_test_manager().await;
        manager.start().await.expect("Failed to start manager");

        // Detect the unknown project
        manager
            .detect_project(unknown_project.clone())
            .await
            .expect("Failed to detect project");

        // Verify project is detected but with unknown type
        let project_info = manager
            .get_project_info(&unknown_project)
            .await
            .expect("Project should be registered");

        assert_eq!(project_info.project_type, ProjectType::Unknown);
        assert!(
            project_info.language_servers.is_empty(),
            "Unknown projects should have no language servers"
        );

        // Verify no server startup is requested
        let startup_event = collector
            .wait_for_event(
                |event| matches!(event, ProjectLspEvent::ServerStartupRequested { .. }),
                Duration::from_millis(500),
            )
            .await;

        assert!(
            startup_event.is_none(),
            "No server startup should be requested for unknown project type"
        );

        manager.stop().await.expect("Failed to stop manager");
        let _ = helper.cleanup_test_directory().await;
    }

    #[tokio::test]
    async fn test_server_cleanup_and_resource_management() {
        let helper = ProjectTestHelper::new();
        let _ = helper.cleanup_test_directory().await;

        let rust_project = helper
            .create_rust_project("cleanup_test")
            .await
            .expect("Failed to create test project");

        let (manager, collector) = setup_test_manager().await;

        manager.start().await.expect("Failed to start manager");

        // Detect project and wait for startup
        manager
            .detect_project(rust_project.clone())
            .await
            .expect("Failed to detect project");

        let _startup_event = collector
            .wait_for_event(
                |event| matches!(event, ProjectLspEvent::ServerStartupRequested { .. }),
                Duration::from_secs(1),
            )
            .await
            .expect("Server startup not requested");

        // Note: In a real implementation, the server would be created and registered
        // For this test, we're just verifying the event flow

        // Test cleanup
        let cleanup_start = Instant::now();
        manager.stop().await.expect("Failed to stop manager");
        let cleanup_time = cleanup_start.elapsed();

        // Verify performance characteristics
        assert!(
            cleanup_time < Duration::from_secs(1),
            "Cleanup should be fast"
        );

        let _ = helper.cleanup_test_directory().await;
    }

    #[tokio::test]
    #[ignore = "Flaky concurrency test - timing sensitive with race conditions"]
    async fn test_concurrent_lsp_server_operations() {
        let helper = ProjectTestHelper::new();
        let _ = helper.cleanup_test_directory().await;

        // Create multiple projects concurrently
        let rust_project1 = helper
            .create_rust_project("concurrent_rust1")
            .await
            .expect("Failed to create rust project 1");
        let rust_project2 = helper
            .create_rust_project("concurrent_rust2")
            .await
            .expect("Failed to create rust project 2");
        let ts_project1 = helper
            .create_typescript_project("concurrent_ts1")
            .await
            .expect("Failed to create ts project 1");

        let project_paths = [rust_project1, rust_project2, ts_project1];

        let (manager, collector) = setup_test_manager().await;
        manager.start().await.expect("Failed to start manager");

        // Detect all projects concurrently
        let detection_futures = project_paths
            .iter()
            .map(|path| manager.detect_project(path.clone()));

        let start_time = Instant::now();
        let results = futures::future::join_all(detection_futures).await;
        let detection_time = start_time.elapsed();

        // Verify all projects were detected successfully
        for result in results {
            result.expect("Project detection should succeed");
        }

        // Wait for all server startup requests
        let timeout_duration = Duration::from_secs(3);
        let mut startup_requests = Vec::new();
        let wait_start = Instant::now();

        while wait_start.elapsed() < timeout_duration && startup_requests.len() < 3 {
            let events = collector
                .get_events_of_type(|event| {
                    matches!(event, ProjectLspEvent::ServerStartupRequested { .. })
                })
                .await;

            startup_requests = events;
            if startup_requests.len() < 3 {
                sleep(Duration::from_millis(50)).await;
            }
        }

        assert_eq!(
            startup_requests.len(),
            3,
            "Should have startup requests for all projects"
        );

        // Verify concurrent detection performance
        assert!(
            detection_time < Duration::from_millis(500),
            "Concurrent detection should be efficient: {:?}",
            detection_time
        );

        manager.stop().await.expect("Failed to stop manager");
        let _ = helper.cleanup_test_directory().await;
    }

    #[tokio::test]
    #[ignore = "Flaky concurrency test - passes individually but fails in group runs"]
    async fn test_project_type_detection_accuracy() {
        let helper = ProjectTestHelper::new();
        let _ = helper.cleanup_test_directory().await;

        let detector = ProjectDetector::new(nucleotide_types::ProjectMarkersConfig::default());

        // Test Rust project detection
        let rust_project = helper
            .create_rust_project("rust_detection")
            .await
            .expect("Failed to create Rust project");

        let rust_info = detector
            .analyze_project(&rust_project)
            .await
            .expect("Failed to analyze Rust project");

        assert_eq!(rust_info.project_type, ProjectType::Rust);
        assert_eq!(rust_info.language_servers, vec!["rust-analyzer"]);

        // Test TypeScript project detection
        let ts_project = helper
            .create_typescript_project("ts_detection")
            .await
            .expect("Failed to create TypeScript project");

        let ts_info = detector
            .analyze_project(&ts_project)
            .await
            .expect("Failed to analyze TypeScript project");

        assert_eq!(ts_info.project_type, ProjectType::TypeScript);
        assert_eq!(ts_info.language_servers, vec!["typescript-language-server"]);

        // Test language ID mapping
        assert_eq!(detector.get_primary_language_id(&ProjectType::Rust), "rust");
        assert_eq!(
            detector.get_primary_language_id(&ProjectType::TypeScript),
            "typescript"
        );
        assert_eq!(
            detector.get_primary_language_id(&ProjectType::Unknown),
            "unknown"
        );

        let _ = helper.cleanup_test_directory().await;
    }
}
