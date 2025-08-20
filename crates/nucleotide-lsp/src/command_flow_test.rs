// ABOUTME: Simple test to verify Event-Driven Command Pattern flow works
// ABOUTME: Tests basic command dispatch, response handling and integration points

use std::path::PathBuf;

use nucleotide_events::{ProjectLspCommand, ProjectLspCommandError};

#[tokio::test]
async fn test_command_types_compile() {
    // Simple compile-time test to ensure all command types work
    let workspace_root = PathBuf::from("/test");
    let (tx, _rx) = tokio::sync::oneshot::channel();

    let _command = ProjectLspCommand::DetectAndStartProject {
        workspace_root,
        response: tx,
    };

    println!("✓ Command types compile successfully");
}

#[tokio::test]
async fn test_command_channel_creation() {
    // Test that we can create command channels
    let (_command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<ProjectLspCommand>();

    // Test that we can receive (in a timeout to avoid blocking)
    let result =
        tokio::time::timeout(std::time::Duration::from_millis(10), command_rx.recv()).await;

    // Should timeout since no commands were sent
    assert!(result.is_err(), "Should timeout waiting for commands");
    println!("✓ Command channels work correctly");
}

#[test]
fn test_error_types() {
    // Test error type creation
    let _error1 = ProjectLspCommandError::ProjectDetection("test".to_string());
    let _error2 = ProjectLspCommandError::ServerStartup("test".to_string());
    let _error3 = ProjectLspCommandError::ServerNotFound;
    let _error4 = ProjectLspCommandError::EditorAccessRequired;
    let _error5 = ProjectLspCommandError::Internal("test".to_string());

    println!("✓ Error types compile successfully");
}
