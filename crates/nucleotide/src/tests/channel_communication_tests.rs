// ABOUTME: Tests for LSP channel communication race condition fixes
// ABOUTME: Verifies timeout handling and proper response channel lifecycle management

use nucleotide_env::ProjectEnvironment;
use nucleotide_events::{ProjectLspCommand, ProjectLspCommandError, ServerStartResult};
use nucleotide_lsp::{EnvironmentProvider, HelixLspBridge};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

/// Test the timeout handling in LSP command response system
#[tokio::test]
async fn test_lsp_command_timeout_handling() {
    // Create a command channel but don't process commands to simulate hanging
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<ProjectLspCommand>();

    // Create a mock command
    let (response_tx, response_rx) = oneshot::channel();
    let test_workspace = PathBuf::from("/tmp/test_workspace");

    let command = ProjectLspCommand::StartServer {
        workspace_root: test_workspace.clone(),
        server_name: "rust-analyzer".to_string(),
        language_id: "rust".to_string(),
        response: response_tx,
        span: tracing::info_span!("test_timeout"),
    };

    // Send command
    command_tx.send(command).expect("Failed to send command");

    // Test timeout behavior - should timeout after reasonable time
    let timeout_duration = Duration::from_millis(100); // Short timeout for test
    let timeout_result = timeout(timeout_duration, response_rx).await;

    // Verify that the timeout occurred (receiver should timeout)
    assert!(timeout_result.is_err(), "Response should have timed out");

    // Verify command was sent but not processed
    assert!(
        command_rx.try_recv().is_ok(),
        "Command should be in the channel"
    );

    println!("✅ Timeout handling test passed - channels properly timeout when no response");
}

/// Test environment provider timeout handling
#[tokio::test]
async fn test_environment_provider_timeout() {
    // Create a mock environment provider that takes too long
    struct SlowEnvironmentProvider;

    impl EnvironmentProvider for SlowEnvironmentProvider {
        fn get_lsp_environment(
            &self,
            _directory: &std::path::Path,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            std::collections::HashMap<String, String>,
                            Box<dyn std::error::Error + Send + Sync>,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                // Simulate slow environment capture
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok(HashMap::new())
            })
        }
    }

    let provider = Arc::new(SlowEnvironmentProvider);
    let test_dir = PathBuf::from("/tmp/test");

    // Test with short timeout
    let timeout_duration = Duration::from_millis(100);
    let timeout_result = timeout(timeout_duration, provider.get_lsp_environment(&test_dir)).await;

    // Should timeout before environment capture completes
    assert!(
        timeout_result.is_err(),
        "Environment provider should have timed out"
    );

    println!("✅ Environment provider timeout test passed");
}

/// Test successful channel communication under normal conditions
#[tokio::test]
async fn test_successful_channel_communication() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<ProjectLspCommand>();

    // Spawn a task to process commands normally
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            match command {
                ProjectLspCommand::StartServer {
                    response,
                    server_name,
                    ..
                } => {
                    // Simulate successful server startup
                    let success_result = Ok(ServerStartResult {
                        server_id: slotmap::KeyData::from_ffi(123).into(),
                        server_name: server_name.clone(),
                        language_id: "rust".to_string(),
                    });

                    let _ = response.send(success_result);
                }
                _ => {} // Handle other commands as needed
            }
        }
    });

    // Create and send command
    let (response_tx, response_rx) = oneshot::channel();
    let command = ProjectLspCommand::StartServer {
        workspace_root: PathBuf::from("/tmp/test"),
        server_name: "rust-analyzer".to_string(),
        language_id: "rust".to_string(),
        response: response_tx,
        span: tracing::info_span!("test_success"),
    };

    command_tx.send(command).expect("Failed to send command");

    // Wait for response with reasonable timeout
    let timeout_duration = Duration::from_secs(1);
    let response_result = timeout(timeout_duration, response_rx).await;

    // Verify successful communication
    assert!(
        response_result.is_ok(),
        "Should receive response within timeout"
    );
    let response = response_result.unwrap().unwrap();
    assert!(response.is_ok(), "Response should indicate success");

    println!("✅ Successful channel communication test passed");
}

/// Test race condition handling with concurrent operations
#[tokio::test]
async fn test_concurrent_lsp_operations() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<ProjectLspCommand>();

    // Process commands with some delay to simulate real conditions
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            match command {
                ProjectLspCommand::StartServer {
                    response,
                    server_name,
                    ..
                } => {
                    // Small delay to simulate processing time
                    tokio::time::sleep(Duration::from_millis(50)).await;

                    let result = Ok(ServerStartResult {
                        server_id: slotmap::KeyData::from_ffi(456).into(),
                        server_name: server_name.clone(),
                        language_id: "rust".to_string(),
                    });

                    let _ = response.send(result);
                }
                ProjectLspCommand::RestartServersForWorkspaceChange { response, .. } => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = response.send(Ok(vec![]));
                }
                _ => {} // Handle other commands
            }
        }
    });

    // Send multiple commands concurrently
    let mut handles = vec![];

    for i in 0..5 {
        let tx = command_tx.clone();
        let handle = tokio::spawn(async move {
            let (response_tx, response_rx) = oneshot::channel();
            let command = ProjectLspCommand::StartServer {
                workspace_root: PathBuf::from(format!("/tmp/test{}", i)),
                server_name: "rust-analyzer".to_string(),
                language_id: "rust".to_string(),
                response: response_tx,
                span: tracing::info_span!("concurrent_test", i = i),
            };

            tx.send(command).expect("Failed to send command");

            // Wait with timeout
            let timeout_result = timeout(Duration::from_secs(2), response_rx).await;
            timeout_result
                .expect("Should not timeout")
                .expect("Should receive response")
        });

        handles.push(handle);
    }

    // Wait for all operations to complete
    let mut success_count = 0;
    for handle in handles {
        let result = handle.await.expect("Task should complete");
        if result.is_ok() {
            success_count += 1;
        }
    }

    // All operations should succeed
    assert_eq!(success_count, 5, "All concurrent operations should succeed");

    println!(
        "✅ Concurrent operations test passed - {} operations completed successfully",
        success_count
    );
}

/// Test environment integration with timeout handling
#[tokio::test]
async fn test_environment_integration_with_timeout() {
    // Create ProjectEnvironment
    let project_env = Arc::new(ProjectEnvironment::new(None));

    // Test with timeout
    let temp_dir = tempfile::tempdir().unwrap();
    let test_path = temp_dir.path();

    let timeout_duration = Duration::from_secs(5); // Reasonable timeout
    let env_result = timeout(timeout_duration, project_env.get_lsp_environment(test_path)).await;

    // Should complete within timeout (even if it just returns process env)
    assert!(
        env_result.is_ok(),
        "Environment capture should complete within timeout"
    );

    let env = env_result
        .unwrap()
        .expect("Should successfully get environment");

    // Should have basic environment variables
    assert!(env.contains_key("PATH"), "Environment should contain PATH");
    assert!(!env.is_empty(), "Environment should not be empty");

    println!(
        "✅ Environment integration with timeout test passed - captured {} variables",
        env.len()
    );
}

/// Test fallback behavior when channels fail
#[tokio::test]
async fn test_channel_failure_fallback() {
    // Test what happens when sender is dropped
    let (command_tx, _command_rx) = mpsc::unbounded_channel::<ProjectLspCommand>();

    // Drop receiver to simulate channel failure
    drop(_command_rx);

    let (response_tx, response_rx) = oneshot::channel();
    let command = ProjectLspCommand::StartServer {
        workspace_root: PathBuf::from("/tmp/test"),
        server_name: "rust-analyzer".to_string(),
        language_id: "rust".to_string(),
        response: response_tx,
        span: tracing::info_span!("fallback_test"),
    };

    // Try to send command - should fail
    let send_result = command_tx.send(command);
    assert!(
        send_result.is_err(),
        "Send should fail when receiver is dropped"
    );

    // Response channel should also fail
    let timeout_result = timeout(Duration::from_millis(100), response_rx).await;
    assert!(
        timeout_result.is_err(),
        "Response should timeout when channel is broken"
    );

    println!("✅ Channel failure fallback test passed");
}
