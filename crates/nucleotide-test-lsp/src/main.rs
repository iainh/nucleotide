// ABOUTME: Nucleotide Testing LSP Server - provides mock completions for testing completion functionality
// ABOUTME: This server implements the Language Server Protocol over stdio for integration with Helix/Nucleotide

use anyhow::Result;
use lsp_server::{Connection, Message, Request, Response};
use lsp_types::*;
use std::io::Write;

/// Main entry point for the Nucleotide Test LSP server
fn main() -> Result<()> {
    // Set up panic handler to catch any panics
    std::panic::set_hook(Box::new(|info| {
        eprintln!("NUCLEOTIDE-TEST-LSP: PANIC OCCURRED: {:?}", info);
        let _ = std::io::stderr().flush();
    }));

    // Skip tracing initialization when run as subprocess - it causes hangs
    // Only use eprintln for debugging to avoid tokio/tracing issues
    eprintln!("NUCLEOTIDE-TEST-LSP: Process starting (stderr)");

    // Force stderr flush immediately
    let _ = std::io::stderr().flush();

    // Create the connection via stdio
    eprintln!("NUCLEOTIDE-TEST-LSP: Creating stdio connection");
    let (connection, io_threads) = Connection::stdio();
    eprintln!("NUCLEOTIDE-TEST-LSP: Stdio connection created successfully");

    // Run the main server loop
    eprintln!("NUCLEOTIDE-TEST-LSP: About to call run_server");
    let _ = std::io::stderr().flush();
    let result = run_server(connection);
    eprintln!("NUCLEOTIDE-TEST-LSP: run_server returned: {:?}", result);
    let _ = std::io::stderr().flush();

    eprintln!("NUCLEOTIDE-TEST-LSP: About to join io_threads");
    let _ = std::io::stderr().flush();

    // Clean shutdown
    let join_result = io_threads.join();
    eprintln!(
        "NUCLEOTIDE-TEST-LSP: io_threads.join() returned: {:?}",
        join_result
    );
    let _ = std::io::stderr().flush();

    join_result?;

    match result {
        Ok(()) => {
            eprintln!("NUCLEOTIDE-TEST-LSP: Shutdown successfully");
            Ok(())
        }
        Err(e) => {
            eprintln!("NUCLEOTIDE-TEST-LSP: Server error: {}", e);
            Err(e)
        }
    }
}

/// Main server loop that handles LSP protocol messages
fn run_server(connection: Connection) -> Result<()> {
    eprintln!("NUCLEOTIDE-TEST-LSP: Starting run_server function");
    let _ = std::io::stderr().flush();

    // Initialize server capabilities
    eprintln!("NUCLEOTIDE-TEST-LSP: Creating server capabilities");
    let _ = std::io::stderr().flush();
    let server_capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![
                ".".to_string(),
                ":".to_string(),
                "(".to_string(),
                "[".to_string(),
                "<".to_string(),
                " ".to_string(),
            ]),
            all_commit_characters: None,
            work_done_progress_options: WorkDoneProgressOptions::default(),
            completion_item: None,
        }),
        ..Default::default()
    };
    eprintln!("NUCLEOTIDE-TEST-LSP: Server capabilities created");
    let _ = std::io::stderr().flush();

    // Serialize and initialize
    eprintln!("NUCLEOTIDE-TEST-LSP: About to serialize and initialize");
    let _ = std::io::stderr().flush();

    eprintln!("NUCLEOTIDE-TEST-LSP: About to serialize capabilities");
    let _ = std::io::stderr().flush();
    let capabilities_value = serde_json::to_value(server_capabilities)?;
    eprintln!("NUCLEOTIDE-TEST-LSP: Capabilities serialized successfully");
    let _ = std::io::stderr().flush();

    eprintln!("NUCLEOTIDE-TEST-LSP: About to call connection.initialize()");
    let _ = std::io::stderr().flush();
    let initialization_params = connection.initialize(capabilities_value)?;
    eprintln!("NUCLEOTIDE-TEST-LSP: connection.initialize() returned successfully");
    let _ = std::io::stderr().flush();

    eprintln!("NUCLEOTIDE-TEST-LSP: About to parse initialization params");
    let _ = std::io::stderr().flush();
    let _params: InitializeParams = serde_json::from_value(initialization_params)?;
    eprintln!("NUCLEOTIDE-TEST-LSP: Initialization params parsed successfully");
    let _ = std::io::stderr().flush();

    eprintln!("NUCLEOTIDE-TEST-LSP: Initialization completed successfully");
    let _ = std::io::stderr().flush();
    // REMOVED THE PROBLEMATIC STDOUT FLUSH!

    // Skip progress notifications for now - just focus on entering message loop
    eprintln!(
        "NUCLEOTIDE-TEST-LSP: Skipping progress notifications, going directly to message loop"
    );
    let _ = std::io::stderr().flush();

    eprintln!(
        "NUCLEOTIDE-TEST-LSP: About to enter main message loop - line {}",
        line!()
    );
    let _ = std::io::stderr().flush();

    // Main message loop
    eprintln!(
        "NUCLEOTIDE-TEST-LSP: Entering main message loop - line {}",
        line!()
    );
    let _ = std::io::stderr().flush();
    let mut message_count = 0;

    eprintln!(
        "NUCLEOTIDE-TEST-LSP: Starting to iterate over connection.receiver - line {}",
        line!()
    );
    let _ = std::io::stderr().flush();

    eprintln!("NUCLEOTIDE-TEST-LSP: About to start blocking on connection.receiver");
    let _ = std::io::stderr().flush();

    eprintln!("NUCLEOTIDE-TEST-LSP: Entering for loop over connection.receiver");
    let _ = std::io::stderr().flush();

    for msg in &connection.receiver {
        eprintln!("NUCLEOTIDE-TEST-LSP: INSIDE MESSAGE LOOP - Got a message!");
        let _ = std::io::stderr().flush();
        message_count += 1;
        eprintln!("NUCLEOTIDE-TEST-LSP: Message #{} received", message_count);
        let _ = std::io::stderr().flush();

        match msg {
            Message::Request(req) => {
                eprintln!("NUCLEOTIDE-TEST-LSP: Processing request: {}", req.method);
                let _ = std::io::stderr().flush();

                if connection.handle_shutdown(&req)? {
                    eprintln!("NUCLEOTIDE-TEST-LSP: Received shutdown request");
                    let _ = std::io::stderr().flush();
                    return Ok(());
                }

                match handle_request_simple(&connection, req) {
                    Ok(()) => {
                        eprintln!("NUCLEOTIDE-TEST-LSP: Request handled successfully");
                        let _ = std::io::stderr().flush();
                    }
                    Err(e) => {
                        eprintln!("NUCLEOTIDE-TEST-LSP: Error handling request: {}", e);
                        let _ = std::io::stderr().flush();
                    }
                }
            }
            Message::Response(resp) => {
                eprintln!("NUCLEOTIDE-TEST-LSP: Received response: id={:?}", resp.id);
                let _ = std::io::stderr().flush();
            }
            Message::Notification(not) => {
                eprintln!("NUCLEOTIDE-TEST-LSP: Received notification: {}", not.method);
                let _ = std::io::stderr().flush();
                match handle_notification_simple(not) {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("NUCLEOTIDE-TEST-LSP: Error handling notification: {}", e);
                        let _ = std::io::stderr().flush();
                    }
                }
            }
        }
    }

    eprintln!("NUCLEOTIDE-TEST-LSP: MESSAGE LOOP EXITED - this means the channel was closed!");
    let _ = std::io::stderr().flush();

    eprintln!("NUCLEOTIDE-TEST-LSP: Exiting message loop - function ending");
    let _ = std::io::stderr().flush();
    Ok(())
}

/// Handle incoming LSP requests (simplified for debugging)
fn handle_request_simple(connection: &Connection, req: Request) -> Result<()> {
    eprintln!(
        "NUCLEOTIDE-TEST-LSP: Received request: method={}, id={:?}",
        req.method, req.id
    );
    let _ = std::io::stderr().flush();

    match req.method.as_str() {
        "textDocument/completion" => {
            eprintln!("NUCLEOTIDE-TEST-LSP: Handling textDocument/completion request");
            let _ = std::io::stderr().flush();
            handle_completion_request_simple(connection, req)?;
        }
        method => {
            eprintln!("NUCLEOTIDE-TEST-LSP: Unhandled request method: {}", method);
            let _ = std::io::stderr().flush();
            let response = Response {
                id: req.id,
                result: None,
                error: Some(lsp_server::ResponseError {
                    code: lsp_server::ErrorCode::MethodNotFound as i32,
                    message: format!("Method not found: {}", method),
                    data: None,
                }),
            };
            connection.sender.send(Message::Response(response))?;
            // Removed stdout flush - let lsp-server handle it
        }
    }
    Ok(())
}

/// Handle textDocument/completion requests (simplified for debugging)
fn handle_completion_request_simple(connection: &Connection, req: Request) -> Result<()> {
    eprintln!(
        "NUCLEOTIDE-TEST-LSP: Processing simple completion request: {:?}",
        req.method
    );
    let _ = std::io::stderr().flush();

    let id = req.id.clone();
    eprintln!("NUCLEOTIDE-TEST-LSP: Completion request ID: {:?}", id);
    let _ = std::io::stderr().flush();

    // Create simple test completions without complex processing
    let completions = vec![
        CompletionItem {
            label: "simple_test_1".to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("Simple test completion 1".to_string()),
            documentation: Some(Documentation::String(
                "A simple test completion".to_string(),
            )),
            insert_text: Some("simple_test_1()".to_string()),
            ..Default::default()
        },
        CompletionItem {
            label: "simple_test_2".to_string(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some("Simple test completion 2".to_string()),
            documentation: Some(Documentation::String("Another simple test".to_string())),
            insert_text: Some("simple_test_2".to_string()),
            ..Default::default()
        },
    ];

    eprintln!(
        "NUCLEOTIDE-TEST-LSP: Generated {} simple completions",
        completions.len()
    );
    let _ = std::io::stderr().flush();

    let result = CompletionResponse::Array(completions);
    let response = Response {
        id,
        result: Some(serde_json::to_value(result)?),
        error: None,
    };

    eprintln!(
        "NUCLEOTIDE-TEST-LSP: Sending simple completion response with ID: {:?}",
        response.id
    );
    let _ = std::io::stderr().flush();
    connection.sender.send(Message::Response(response))?;

    // Removed stdout flush - let lsp-server handle it

    eprintln!("NUCLEOTIDE-TEST-LSP: Simple completion response sent successfully");
    let _ = std::io::stderr().flush();
    Ok(())
}

/// Handle LSP notifications (simplified for debugging)
fn handle_notification_simple(not: lsp_server::Notification) -> Result<()> {
    match not.method.as_str() {
        "textDocument/didOpen" => {
            eprintln!("NUCLEOTIDE-TEST-LSP: Document opened notification");
            let _ = std::io::stderr().flush();
        }
        "textDocument/didChange" => {
            eprintln!("NUCLEOTIDE-TEST-LSP: Document changed notification");
            let _ = std::io::stderr().flush();
        }
        "textDocument/didSave" => {
            eprintln!("NUCLEOTIDE-TEST-LSP: Document saved notification");
            let _ = std::io::stderr().flush();
        }
        "textDocument/didClose" => {
            eprintln!("NUCLEOTIDE-TEST-LSP: Document closed notification");
            let _ = std::io::stderr().flush();
        }
        method => {
            eprintln!(
                "NUCLEOTIDE-TEST-LSP: Unhandled notification method: {}",
                method
            );
            let _ = std::io::stderr().flush();
        }
    }
    Ok(())
}

// End of file
