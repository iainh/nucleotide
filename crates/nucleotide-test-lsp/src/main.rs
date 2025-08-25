// ABOUTME: Nucleotide Testing LSP Server - provides mock completions for testing completion functionality
// ABOUTME: This server implements the Language Server Protocol over stdio for integration with Helix/Nucleotide

use anyhow::{Result, anyhow};
use lsp_server::{Connection, Message, Request, RequestId, Response};
use lsp_types::*;
use tracing::{debug, error, info, warn};

mod completion_engine;
mod config;
mod protocol;
mod test_scenarios;

use completion_engine::CompletionEngine;
use config::TestLspConfig;
use protocol::ProtocolHandler;

/// Main entry point for the Nucleotide Test LSP server
fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting Nucleotide Test LSP Server");

    // Create the connection via stdio
    let (connection, io_threads) = Connection::stdio();

    // Run the main server loop
    let result = run_server(connection);

    // Clean shutdown
    io_threads.join()?;

    match result {
        Ok(()) => {
            info!("Nucleotide Test LSP Server shutdown successfully");
            Ok(())
        }
        Err(e) => {
            error!("Server error: {}", e);
            Err(e)
        }
    }
}

/// Main server loop that handles LSP protocol messages
fn run_server(connection: Connection) -> Result<()> {
    // Initialize server capabilities
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

    // Perform LSP initialization handshake
    let initialization_params =
        connection.initialize(serde_json::to_value(server_capabilities)?)?;
    let _params: InitializeParams = serde_json::from_value(initialization_params)?;

    info!("LSP initialization completed");

    // Load configuration and initialize components
    let config = TestLspConfig::load_default()?;
    let completion_engine = CompletionEngine::new(config.clone());
    let protocol_handler = ProtocolHandler::new(config);

    debug!("Server components initialized");

    // Main message loop
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    info!("Received shutdown request");
                    return Ok(());
                }

                match handle_request(&connection, req, &completion_engine, &protocol_handler) {
                    Ok(()) => {}
                    Err(e) => {
                        error!("Error handling request: {}", e);
                    }
                }
            }
            Message::Response(resp) => {
                debug!("Received response: {:?}", resp.id);
            }
            Message::Notification(not) => match handle_notification(not, &protocol_handler) {
                Ok(()) => {}
                Err(e) => {
                    warn!("Error handling notification: {}", e);
                }
            },
        }
    }

    Ok(())
}

/// Handle incoming LSP requests
fn handle_request(
    connection: &Connection,
    req: Request,
    completion_engine: &CompletionEngine,
    _protocol_handler: &ProtocolHandler,
) -> Result<()> {
    match req.method.as_str() {
        "textDocument/completion" => {
            handle_completion_request(connection, req, completion_engine)?;
        }
        method => {
            debug!("Unhandled request method: {}", method);
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
        }
    }
    Ok(())
}

/// Handle textDocument/completion requests
fn handle_completion_request(
    connection: &Connection,
    req: Request,
    completion_engine: &CompletionEngine,
) -> Result<()> {
    let (id, params) = extract_completion_params(req)?;
    let completion_params: CompletionParams = params;

    debug!(
        "Completion request for URI: {:?} at position {}:{}",
        completion_params.text_document_position.text_document.uri,
        completion_params.text_document_position.position.line,
        completion_params.text_document_position.position.character
    );

    // Generate completion response
    let completions = completion_engine.generate_completions(&completion_params)?;

    let result = CompletionResponse::Array(completions);
    let response = Response {
        id,
        result: Some(serde_json::to_value(result)?),
        error: None,
    };

    connection.sender.send(Message::Response(response))?;
    Ok(())
}

/// Extract completion parameters from request
fn extract_completion_params(req: Request) -> Result<(RequestId, CompletionParams)> {
    let id = req.id.clone();

    if req.method != "textDocument/completion" {
        return Err(anyhow!(
            "Expected textDocument/completion, got {}",
            req.method
        ));
    }

    let params: CompletionParams = serde_json::from_value(req.params)?;
    Ok((id, params))
}

/// Handle LSP notifications
fn handle_notification(
    not: lsp_server::Notification,
    _protocol_handler: &ProtocolHandler,
) -> Result<()> {
    match not.method.as_str() {
        "textDocument/didOpen" => {
            debug!("Document opened");
        }
        "textDocument/didChange" => {
            debug!("Document changed");
        }
        "textDocument/didSave" => {
            debug!("Document saved");
        }
        "textDocument/didClose" => {
            debug!("Document closed");
        }
        method => {
            debug!("Unhandled notification method: {}", method);
        }
    }
    Ok(())
}
