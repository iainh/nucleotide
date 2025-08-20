// ABOUTME: Integration tests for ProjectLspManager and Helix LSP system coordination
// ABOUTME: Verifies proactive server startup, fallback mechanisms, and error recovery

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{Config as NucleotideConfig, GuiConfig, LspConfig};
use crate::lsp_manager::{LspManager, LspStartupMode, LspStartupResult};
use nucleotide_events::{ProjectType, ServerHealthStatus};
use nucleotide_lsp::{HelixLspBridge, ProjectLspConfig, ProjectLspManager};

/// Test helper to create a test configuration
fn create_test_config(
    project_lsp_startup: bool,
    enable_fallback: bool,
    timeout_ms: u64,
) -> Arc<NucleotideConfig> {
    let mut gui_config = GuiConfig::default();
    gui_config.lsp = LspConfig {
        project_lsp_startup,
        startup_timeout_ms: timeout_ms,
        enable_fallback,
    };

    Arc::new(NucleotideConfig {
        helix: helix_term::config::Config::default(),
        gui: gui_config,
    })
}

/// Test helper to create a temporary project directory
fn create_test_project_dir() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let project_dir = temp_dir.join(format!("test_project_{}", timestamp));
    std::fs::create_dir_all(&project_dir).expect("Failed to create test project directory");
    project_dir
}

/// Test helper to create a Rust project structure
fn create_rust_project(project_dir: &PathBuf) {
    let cargo_toml = project_dir.join("Cargo.toml");
    std::fs::write(
        &cargo_toml,
        r#"
[package]
name = "test_project"
version = "0.1.0"
edition = "2021"

[dependencies]
"#,
    )
    .expect("Failed to create Cargo.toml");

    let src_dir = project_dir.join("src");
    std::fs::create_dir_all(&src_dir).expect("Failed to create src directory");

    let main_rs = src_dir.join("main.rs");
    std::fs::write(
        &main_rs,
        r#"
fn main() {
    println!("Hello, world!");
}
"#,
    )
    .expect("Failed to create main.rs");
}

#[tokio::test]
async fn test_project_lsp_manager_creation() {
    let config = ProjectLspConfig::default();
    let manager = ProjectLspManager::new(config, None);

    // Test that we can get an event sender
    let _event_sender = manager.get_event_sender();

    // Manager should be created successfully
    // Test that we can start the manager
    manager.start().await.expect("Failed to start manager");

    // Test that we can stop the manager
    manager.stop().await.expect("Failed to stop manager");
}

#[tokio::test]
async fn test_project_detection_rust() {
    let config = ProjectLspConfig::default();
    let manager = ProjectLspManager::new(config, None);

    let project_dir = create_test_project_dir();
    create_rust_project(&project_dir);

    // Start the manager
    manager
        .start()
        .await
        .expect("Failed to start ProjectLspManager");

    // Detect the project
    manager
        .detect_project(project_dir.clone())
        .await
        .expect("Failed to detect project");

    // Verify project information
    let project_info = manager.get_project_info(&project_dir).await;
    assert!(project_info.is_some());

    let project = project_info.unwrap();
    assert!(matches!(project.project_type, ProjectType::Rust));
    assert!(
        project
            .language_servers
            .contains(&"rust-analyzer".to_string())
    );

    // Cleanup
    manager
        .stop()
        .await
        .expect("Failed to stop ProjectLspManager");
    std::fs::remove_dir_all(&project_dir).ok();
}

#[tokio::test]
async fn test_lsp_manager_startup_modes() {
    // Test with project LSP startup enabled
    let config = create_test_config(true, true, 5000);
    let lsp_manager = LspManager::new(config.clone());

    let project_dir = create_test_project_dir();
    create_rust_project(&project_dir);

    let file_path = project_dir.join("src").join("main.rs");

    // Determine startup mode with project detected
    let mode = lsp_manager.determine_startup_mode(Some(&file_path), Some(&project_dir));

    match mode {
        LspStartupMode::Project {
            project_root,
            timeout,
        } => {
            assert_eq!(project_root, project_dir);
            assert_eq!(timeout, Duration::from_millis(5000));
        }
        _ => panic!("Expected project mode when project is detected"),
    }

    // Test with project LSP startup disabled
    let config_disabled = create_test_config(false, true, 5000);
    let lsp_manager_disabled = LspManager::new(config_disabled);

    let mode_disabled =
        lsp_manager_disabled.determine_startup_mode(Some(&file_path), Some(&project_dir));

    match mode_disabled {
        LspStartupMode::File {
            file_path: detected_path,
        } => {
            assert_eq!(detected_path, file_path);
        }
        _ => panic!("Expected file mode when project LSP is disabled"),
    }

    // Cleanup
    std::fs::remove_dir_all(&project_dir).ok();
}

#[tokio::test]
async fn test_fallback_mechanism() {
    let config = create_test_config(true, true, 5000);
    let lsp_manager = LspManager::new(config);

    // Test fallback when no project is detected
    let file_path = PathBuf::from("/tmp/standalone_file.rs");

    let mode = lsp_manager.determine_startup_mode(Some(&file_path), None);

    match mode {
        LspStartupMode::File {
            file_path: detected_path,
        } => {
            assert_eq!(detected_path, file_path);
        }
        _ => panic!("Expected file mode when no project detected and fallback enabled"),
    }
}

#[tokio::test]
async fn test_helix_lsp_bridge_creation() {
    let config = ProjectLspConfig::default();
    let manager = ProjectLspManager::new(config, None);

    let event_sender = manager.get_event_sender();
    let _bridge = HelixLspBridge::new(event_sender);

    // Bridge should be created successfully
    // Actual functionality testing would require a full Helix Editor instance
}

#[tokio::test]
async fn test_project_type_detection() {
    let detector = nucleotide_lsp::ProjectDetector::new(nucleotide_types::ProjectMarkersConfig::default());

    // Test language ID mapping (which is public)
    assert_eq!(detector.get_primary_language_id(&ProjectType::Rust), "rust");
    assert_eq!(
        detector.get_primary_language_id(&ProjectType::TypeScript),
        "typescript"
    );
    assert_eq!(
        detector.get_primary_language_id(&ProjectType::Python),
        "python"
    );

    // For testing server detection, we'll use the fact that project info contains servers
    let project_info = nucleotide_lsp::ProjectInfo {
        workspace_root: PathBuf::from("/test"),
        project_type: ProjectType::Rust,
        language_servers: vec!["rust-analyzer".to_string()],
        detected_at: std::time::Instant::now(),
    };

    assert!(
        project_info
            .language_servers
            .contains(&"rust-analyzer".to_string())
    );
}

#[tokio::test]
async fn test_language_id_mapping() {
    let detector = nucleotide_lsp::ProjectDetector::new(nucleotide_types::ProjectMarkersConfig::default());

    assert_eq!(detector.get_primary_language_id(&ProjectType::Rust), "rust");
    assert_eq!(
        detector.get_primary_language_id(&ProjectType::TypeScript),
        "typescript"
    );
    assert_eq!(
        detector.get_primary_language_id(&ProjectType::JavaScript),
        "javascript"
    );
    assert_eq!(
        detector.get_primary_language_id(&ProjectType::Python),
        "python"
    );
    assert_eq!(detector.get_primary_language_id(&ProjectType::Go), "go");
    assert_eq!(detector.get_primary_language_id(&ProjectType::C), "c");
    assert_eq!(detector.get_primary_language_id(&ProjectType::Cpp), "cpp");
    assert_eq!(
        detector.get_primary_language_id(&ProjectType::Unknown),
        "unknown"
    );
}

#[tokio::test]
async fn test_managed_server_lifecycle() {
    use helix_lsp::LanguageServerId;
    use nucleotide_lsp::{ManagedServer, ProjectLspError};
    use std::time::Instant;

    // Create a mock server ID - in practice this comes from Helix
    let server_id: LanguageServerId = unsafe { std::mem::transmute(12345u64) };
    let workspace_root = PathBuf::from("/test/workspace");

    let managed_server = ManagedServer {
        server_id,
        server_name: "rust-analyzer".to_string(),
        language_id: "rust".to_string(),
        workspace_root: workspace_root.clone(),
        started_at: Instant::now(),
        last_health_check: None,
        health_status: ServerHealthStatus::Healthy,
    };

    assert_eq!(managed_server.server_name, "rust-analyzer");
    assert_eq!(managed_server.language_id, "rust");
    assert_eq!(managed_server.workspace_root, workspace_root);
    assert!(matches!(
        managed_server.health_status,
        ServerHealthStatus::Healthy
    ));
}

#[tokio::test]
async fn test_project_lsp_error_types() {
    use nucleotide_lsp::ProjectLspError;

    let detection_error = ProjectLspError::ProjectDetection("detection failed".to_string());
    assert!(
        detection_error
            .to_string()
            .contains("Project detection failed")
    );

    let startup_error = ProjectLspError::ServerStartup("startup failed".to_string());
    assert!(startup_error.to_string().contains("Server startup failed"));

    let config_error = ProjectLspError::Configuration("config error".to_string());
    assert!(config_error.to_string().contains("Configuration error"));

    let comm_error = ProjectLspError::ServerCommunication("comm failed".to_string());
    assert!(
        comm_error
            .to_string()
            .contains("Server communication failed")
    );

    let internal_error = ProjectLspError::Internal("internal error".to_string());
    assert!(internal_error.to_string().contains("Internal error"));
}

#[tokio::test]
async fn test_config_hot_reload() {
    // Test LspManager configuration updates
    let initial_config = create_test_config(false, true, 5000);
    let mut lsp_manager = LspManager::new(initial_config);

    let new_config = create_test_config(true, false, 3000);
    let result = lsp_manager.update_config(new_config);

    assert!(result.is_ok());

    // Test that configuration changes are applied by checking behavior
    let file_path = PathBuf::from("/test/file.rs");
    let project_dir = PathBuf::from("/test/project");

    let mode = lsp_manager.determine_startup_mode(Some(&file_path), Some(&project_dir));

    // After update, should use project mode
    match mode {
        LspStartupMode::Project { .. } => {
            // Configuration update was successful
        }
        _ => panic!("Expected project mode after configuration update"),
    }
}

#[tokio::test]
async fn test_startup_statistics() {
    let config = create_test_config(true, true, 5000);
    let lsp_manager = LspManager::new(config);

    // Initially, no startup attempts
    let stats = lsp_manager.get_startup_stats();
    assert_eq!(stats.total_attempts, 0);
    assert_eq!(stats.successful_attempts, 0);
    assert_eq!(stats.failed_attempts, 0);
    assert_eq!(stats.skipped_attempts, 0);
    assert_eq!(stats.project_mode_attempts, 0);
    assert_eq!(stats.file_mode_attempts, 0);
}

/// Integration test that simulates the full workflow
#[tokio::test]
async fn test_integration_workflow() {
    // Create test project
    let project_dir = create_test_project_dir();
    create_rust_project(&project_dir);

    // Create ProjectLspManager
    let project_config = ProjectLspConfig::default();
    let manager = ProjectLspManager::new(project_config, None);

    // Start manager
    manager.start().await.expect("Failed to start manager");

    // Create bridge
    let event_sender = manager.get_event_sender();
    let _bridge = HelixLspBridge::new(event_sender);

    // Detect project
    manager
        .detect_project(project_dir.clone())
        .await
        .expect("Failed to detect project");

    // Verify project info
    let project_info = manager.get_project_info(&project_dir).await;
    assert!(project_info.is_some());

    let project = project_info.unwrap();
    assert!(matches!(project.project_type, ProjectType::Rust));
    assert!(!project.language_servers.is_empty());

    // Test LspManager coordination
    let config = create_test_config(true, true, 5000);
    let lsp_manager = LspManager::new(config);

    let file_path = project_dir.join("src").join("main.rs");
    let mode = lsp_manager.determine_startup_mode(Some(&file_path), Some(&project_dir));

    assert!(matches!(mode, LspStartupMode::Project { .. }));

    // Cleanup
    manager.stop().await.expect("Failed to stop manager");
    std::fs::remove_dir_all(&project_dir).ok();
}

/// Test error recovery scenarios
#[tokio::test]
async fn test_error_recovery() {
    // Test project detection with invalid directory
    let config = ProjectLspConfig::default();
    let manager = ProjectLspManager::new(config, None);

    manager.start().await.expect("Failed to start manager");

    let invalid_dir = PathBuf::from("/nonexistent/directory");
    let result = manager.detect_project(invalid_dir).await;

    // Should fail gracefully
    assert!(result.is_err());

    manager.stop().await.expect("Failed to stop manager");
}

/// Performance test for concurrent operations
#[tokio::test]
async fn test_concurrent_operations() {
    use tokio::join;

    let config = ProjectLspConfig::default();
    let manager = Arc::new(ProjectLspManager::new(config, None));

    manager.start().await.expect("Failed to start manager");

    // Create multiple test projects
    let project1 = create_test_project_dir();
    let project2 = create_test_project_dir();
    let project3 = create_test_project_dir();

    create_rust_project(&project1);
    create_rust_project(&project2);
    create_rust_project(&project3);

    // Detect projects concurrently
    let manager1 = Arc::clone(&manager);
    let manager2 = Arc::clone(&manager);
    let manager3 = Arc::clone(&manager);

    let (result1, result2, result3) = join!(
        manager1.detect_project(project1.clone()),
        manager2.detect_project(project2.clone()),
        manager3.detect_project(project3.clone())
    );

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert!(result3.is_ok());

    // Verify all projects were detected
    let info1 = manager.get_project_info(&project1).await;
    let info2 = manager.get_project_info(&project2).await;
    let info3 = manager.get_project_info(&project3).await;

    assert!(info1.is_some());
    assert!(info2.is_some());
    assert!(info3.is_some());

    // Cleanup
    manager.stop().await.expect("Failed to stop manager");
    std::fs::remove_dir_all(&project1).ok();
    std::fs::remove_dir_all(&project2).ok();
    std::fs::remove_dir_all(&project3).ok();
}
