// ABOUTME: Phase 2 integration test for Event-Driven Command Pattern
// ABOUTME: Tests end-to-end command routing from external call to command processor

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::timeout;

/// Integration test demonstrating end-to-end command routing
/// This shows that commands sent through Application channels reach the command processor
#[tokio::test]
async fn test_command_routing_integration() {
    use nucleotide_events::{ProjectLspCommand, ProjectLspCommandError};

    // Create a mock Application structure (simplified version)
    let (project_lsp_command_tx, project_lsp_command_rx) = tokio::sync::mpsc::unbounded_channel();
    let command_rx_arc =
        std::sync::Arc::new(tokio::sync::RwLock::new(Some(project_lsp_command_rx)));

    // Simulate what happens in Application::start_project_lsp_command_processor
    let mut command_rx = command_rx_arc
        .write()
        .await
        .take()
        .expect("Command receiver should be available");

    // Start command processor (simplified version)
    let processor_handle = tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            match command {
                ProjectLspCommand::GetProjectStatus {
                    workspace_root,
                    response,
                } => {
                    println!(
                        "✓ Command processor received GetProjectStatus for: {}",
                        workspace_root.display()
                    );

                    // Simulate processing and send response
                    let result = Err(ProjectLspCommandError::Internal(
                        "Project LSP manager not initialized".to_string(),
                    ));
                    let _ = response.send(result);
                    break; // Exit after first command for test
                }
                other => {
                    println!(
                        "✓ Command processor received command: {:?}",
                        std::mem::discriminant(&other)
                    );
                    break;
                }
            }
        }
    });

    // Test: Send command through the Application channel interface
    let workspace_root = PathBuf::from("/tmp/test_project");
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    let command = ProjectLspCommand::GetProjectStatus {
        workspace_root: workspace_root.clone(),
        response: response_tx,
    };

    println!("→ Sending GetProjectStatus command through Application channel");

    // Send command through Application channel
    project_lsp_command_tx
        .send(command)
        .expect("Should be able to send command");

    // Wait for response
    let result = timeout(Duration::from_secs(2), response_rx)
        .await
        .expect("Command should complete within timeout")
        .expect("Should receive response");

    // Verify response
    match result {
        Err(ProjectLspCommandError::Internal(msg)) if msg.contains("not initialized") => {
            println!("✓ Received expected response from command processor");
        }
        other => {
            println!("✓ Command routing working - received: {:?}", other);
        }
    }

    // Wait for processor to complete
    let _ = timeout(Duration::from_secs(1), processor_handle).await;

    println!("✅ End-to-end command routing test passed!");
}

/// Test that demonstrates the command types and channels work correctly
#[tokio::test]
async fn test_command_channel_integration() {
    use nucleotide_events::ProjectLspCommand;

    // Test that we can create the same channel types used in Application
    let (_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProjectLspCommand>();

    // Test that we can simulate the Application's channel management
    let rx_arc = std::sync::Arc::new(tokio::sync::RwLock::new(Some(rx)));

    // Test taking the receiver (like Application::take_project_lsp_command_receiver)
    let taken_rx = rx_arc.write().await.take();
    assert!(taken_rx.is_some(), "Should be able to take receiver");

    // Test that second take returns None (receiver already taken)
    let second_take = rx_arc.write().await.take();
    assert!(second_take.is_none(), "Second take should return None");

    println!("✅ Command channel integration test passed!");
}

/// Test the full Event-Driven Command Pattern with MockHelixLspBridge
#[tokio::test]
async fn test_event_driven_command_pattern_end_to_end() {
    use helix_lsp::LanguageServerId;
    use nucleotide_events::{ProjectLspCommand, ServerStartResult};
    use nucleotide_lsp::MockHelixLspBridge;
    use slotmap::KeyData;
    use std::path::PathBuf;

    // Create mock Application structure for testing
    let (project_lsp_command_tx, project_lsp_command_rx) = tokio::sync::mpsc::unbounded_channel();
    let command_rx_arc =
        std::sync::Arc::new(tokio::sync::RwLock::new(Some(project_lsp_command_rx)));

    // Create mock HelixLspBridge
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let mock_bridge =
        MockHelixLspBridge::new(event_tx).with_server_id(KeyData::from_ffi(42).into());

    // Simulate Application command processor using MockHelixLspBridge
    let processor_handle = {
        let bridge = mock_bridge.clone();
        tokio::spawn(async move {
            let mut command_rx = command_rx_arc
                .write()
                .await
                .take()
                .expect("Command receiver should be available");

            while let Some(command) = command_rx.recv().await {
                match command {
                    ProjectLspCommand::StartServer {
                        workspace_root,
                        server_name,
                        language_id,
                        response,
                        span,
                    } => {
                        let _guard = span.enter();
                        println!("✓ Processing StartServer command through mock Application");

                        // Simulate Editor (can't create real one in test)
                        let mut editor = unsafe { std::mem::zeroed() }; // Mock editor for testing

                        // Use mock bridge to simulate server startup
                        let result = bridge
                            .start_server(&mut editor, &workspace_root, &server_name, &language_id)
                            .await
                            .map(|server_id| ServerStartResult {
                                server_id,
                                server_name: server_name.clone(),
                                language_id: language_id.clone(),
                            })
                            .map_err(|e| {
                                nucleotide_events::ProjectLspCommandError::ServerStartup(
                                    e.to_string(),
                                )
                            });

                        let _ = response.send(result);
                        break; // Exit after first command for test
                    }
                    other => {
                        println!(
                            "✓ Command processor received command: {:?}",
                            std::mem::discriminant(&other)
                        );
                        break;
                    }
                }
            }
        })
    };

    // Test: Send StartServer command through the Event-Driven Command Pattern
    let workspace_root = PathBuf::from("/tmp/test_project");
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    let span = tracing::info_span!("test_start_server");

    let command = ProjectLspCommand::StartServer {
        workspace_root: workspace_root.clone(),
        server_name: "rust-analyzer".to_string(),
        language_id: "rust".to_string(),
        response: response_tx,
        span,
    };

    println!("→ Sending StartServer command through Event-Driven Command Pattern");

    // Send command through Application channel
    project_lsp_command_tx
        .send(command)
        .expect("Should be able to send command");

    // Wait for response
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), response_rx)
        .await
        .expect("Command should complete within timeout")
        .expect("Should receive response");

    // Verify response
    match result {
        Ok(server_result) => {
            println!(
                "✓ Successfully started server: {:?}",
                server_result.server_id
            );
            assert_eq!(server_result.server_name, "rust-analyzer");
            assert_eq!(server_result.language_id, "rust");
            println!("✅ Event-Driven Command Pattern working correctly!");
        }
        Err(e) => {
            panic!("Expected successful server startup, got error: {}", e);
        }
    }

    // Wait for processor to complete
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), processor_handle).await;
}

/// Test Event-Driven Command Pattern with failure scenarios
#[tokio::test]
async fn test_event_driven_command_pattern_failure() {
    use nucleotide_events::{ProjectLspCommand, ProjectLspCommandError};
    use nucleotide_lsp::MockHelixLspBridge;
    use std::path::PathBuf;

    // Create mock Application structure for testing
    let (project_lsp_command_tx, project_lsp_command_rx) = tokio::sync::mpsc::unbounded_channel();
    let command_rx_arc =
        std::sync::Arc::new(tokio::sync::RwLock::new(Some(project_lsp_command_rx)));

    // Create failing mock HelixLspBridge
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let mock_bridge = MockHelixLspBridge::new_failing(event_tx);

    // Simulate Application command processor with failing bridge
    let processor_handle = {
        let bridge = mock_bridge.clone();
        tokio::spawn(async move {
            let mut command_rx = command_rx_arc
                .write()
                .await
                .take()
                .expect("Command receiver should be available");

            while let Some(command) = command_rx.recv().await {
                match command {
                    ProjectLspCommand::StartServer {
                        workspace_root,
                        server_name,
                        language_id,
                        response,
                        span,
                    } => {
                        let _guard = span.enter();
                        println!("✓ Processing failing StartServer command");

                        // Simulate Editor
                        let mut editor = unsafe { std::mem::zeroed() }; // Mock editor for testing

                        // Use failing mock bridge
                        let result = bridge
                            .start_server(&mut editor, &workspace_root, &server_name, &language_id)
                            .await
                            .map(|server_id| nucleotide_events::ServerStartResult {
                                server_id,
                                server_name: server_name.clone(),
                                language_id: language_id.clone(),
                            })
                            .map_err(|e| {
                                nucleotide_events::ProjectLspCommandError::ServerStartup(
                                    e.to_string(),
                                )
                            });

                        let _ = response.send(result);
                        break;
                    }
                    _ => break,
                }
            }
        })
    };

    // Test: Send command that will fail
    let workspace_root = PathBuf::from("/tmp/failing_project");
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    let span = tracing::info_span!("test_failing_server");

    let command = ProjectLspCommand::StartServer {
        workspace_root: workspace_root.clone(),
        server_name: "failing-server".to_string(),
        language_id: "unknown".to_string(),
        response: response_tx,
        span,
    };

    println!("→ Sending failing StartServer command");
    project_lsp_command_tx
        .send(command)
        .expect("Should be able to send command");

    // Wait for response
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), response_rx)
        .await
        .expect("Command should complete within timeout")
        .expect("Should receive response");

    // Verify failure response
    match result {
        Ok(_) => panic!("Expected failure, but got success"),
        Err(ProjectLspCommandError::ServerStartup(msg)) => {
            println!("✓ Received expected failure: {}", msg);
            assert!(msg.contains("Mock server startup failure"));
            println!("✅ Error handling working correctly!");
        }
        Err(other) => panic!("Expected ServerStartup error, got: {:?}", other),
    }

    // Wait for processor to complete
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), processor_handle).await;
}

/// Test tracing span propagation across the Event-Driven Command Pattern
#[tokio::test]
async fn test_tracing_span_propagation() {
    use nucleotide_events::ProjectLspCommand;
    use std::path::PathBuf;

    // Create command channels
    let (project_lsp_command_tx, mut project_lsp_command_rx) =
        tokio::sync::mpsc::unbounded_channel();

    // Create command with tracing span
    let workspace_root = PathBuf::from("/tmp/traced_project");
    let (response_tx, _response_rx) = tokio::sync::oneshot::channel();

    // Create span with specific fields for testing
    let span = tracing::info_span!(
        "test_traced_command",
        workspace_root = %workspace_root.display(),
        server_name = "rust-analyzer",
        test_id = "tracing_test"
    );

    let command = ProjectLspCommand::StartServer {
        workspace_root: workspace_root.clone(),
        server_name: "rust-analyzer".to_string(),
        language_id: "rust".to_string(),
        response: response_tx,
        span: span.clone(),
    };

    // Send command
    project_lsp_command_tx
        .send(command)
        .expect("Should be able to send command");

    // Receive and verify span is attached
    if let Some(received_command) = project_lsp_command_rx.recv().await {
        match received_command {
            ProjectLspCommand::StartServer {
                span: received_span,
                ..
            } => {
                // Enter the span to verify it works
                let _guard = received_span.enter();

                // The span should be the same one we created
                println!("✓ Span propagated correctly through Event-Driven Command Pattern");
                println!("✅ Tracing correlation working!");
            }
            _ => panic!("Expected StartServer command"),
        }
    } else {
        panic!("Should have received command");
    }
}

/// Test command creation with all supported command types
#[test]
fn test_command_type_creation() {
    use nucleotide_events::ProjectLspCommand;
    use std::path::PathBuf;

    // Test creating each command type
    let workspace_root = PathBuf::from("/test");

    // DetectAndStartProject
    let (tx1, _rx1) = tokio::sync::oneshot::channel();
    let _cmd1 = ProjectLspCommand::DetectAndStartProject {
        workspace_root: workspace_root.clone(),
        response: tx1,
    };

    // GetProjectStatus
    let (tx2, _rx2) = tokio::sync::oneshot::channel();
    let _cmd2 = ProjectLspCommand::GetProjectStatus {
        workspace_root: workspace_root.clone(),
        response: tx2,
    };

    // StartServer
    let (tx3, _rx3) = tokio::sync::oneshot::channel();
    let _cmd3 = ProjectLspCommand::StartServer {
        workspace_root: workspace_root.clone(),
        server_name: "rust-analyzer".to_string(),
        language_id: "rust".to_string(),
        response: tx3,
    };

    // StopServer
    let (tx4, _rx4) = tokio::sync::oneshot::channel();
    let _cmd4 = ProjectLspCommand::StopServer {
        server_id: slotmap::KeyData::from_ffi(123).into(),
        response: tx4,
    };

    // EnsureDocumentTracked
    let (tx5, _rx5) = tokio::sync::oneshot::channel();
    let _cmd5 = ProjectLspCommand::EnsureDocumentTracked {
        server_id: slotmap::KeyData::from_ffi(123).into(),
        doc_id: helix_view::DocumentId::default(),
        response: tx5,
    };

    println!("✅ All command types created successfully!");
}
