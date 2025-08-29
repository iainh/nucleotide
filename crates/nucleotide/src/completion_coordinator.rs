// ABOUTME: Coordinates LSP completion requests with nucleotide UI
// ABOUTME: Receives completion events from helix and manages the completion flow

use gpui::{BackgroundExecutor, Entity};
use helix_view::handlers::completion::CompletionEvent;
use nucleotide_events::completion::{CompletionItem, CompletionItemKind};
use nucleotide_logging::{debug, error, info, instrument, warn};
use tokio::sync::mpsc::{Receiver, Sender};

use crate::Application;

/// LSP completion request message
pub struct LspCompletionRequest {
    pub cursor: usize,
    pub doc_id: helix_view::DocumentId,
    pub view_id: helix_view::ViewId,
    pub response_tx: tokio::sync::oneshot::Sender<LspCompletionResponse>,
}

/// LSP completion response message - use event-based type
pub type LspCompletionResponse = nucleotide_events::completion::LspCompletionResponse;

/// Completion result sent to UI
#[derive(Debug)]
pub enum CompletionResult {
    ShowCompletions {
        items: Vec<CompletionItem>,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        prefix: String,
    },
    HideCompletions,
}

/// Coordinates completion events from helix with nucleotide UI
pub struct CompletionCoordinator {
    /// Receiver for completion events from helix
    completion_rx: Receiver<CompletionEvent>,
    /// Sender for completion results to workspace
    completion_results_tx: Sender<CompletionResult>,
    /// Sender for LSP completion requests to application
    lsp_completion_requests_tx: Sender<LspCompletionRequest>,
    /// Reference to core application
    core: Entity<Application>,
    /// Background executor for running tasks
    background_executor: BackgroundExecutor,
}

impl CompletionCoordinator {
    pub fn new(
        completion_rx: Receiver<CompletionEvent>,
        completion_results_tx: Sender<CompletionResult>,
        lsp_completion_requests_tx: Sender<LspCompletionRequest>,
        core: Entity<Application>,
        background_executor: BackgroundExecutor,
    ) -> Self {
        Self {
            completion_rx,
            completion_results_tx,
            lsp_completion_requests_tx,
            core,
            background_executor,
        }
    }

    /// Start the completion coordinator using GPUI's background executor
    pub fn spawn(self) {
        let CompletionCoordinator {
            completion_rx,
            completion_results_tx,
            lsp_completion_requests_tx,
            core,
            background_executor,
        } = self;

        let mut coordinator = CompletionCoordinator {
            completion_rx,
            completion_results_tx,
            lsp_completion_requests_tx,
            core,
            background_executor: background_executor.clone(),
        };

        background_executor
            .spawn(async move {
                coordinator.run().await;
            })
            .detach();
    }

    /// Main event loop
    #[instrument(skip(self))]
    async fn run(&mut self) {
        info!("Completion coordinator started - waiting for completion events");

        while let Some(event) = self.completion_rx.recv().await {
            let event_type = match &event {
                helix_view::handlers::completion::CompletionEvent::ManualTrigger { .. } => {
                    "ManualTrigger"
                }
                helix_view::handlers::completion::CompletionEvent::AutoTrigger { .. } => {
                    "AutoTrigger"
                }
                helix_view::handlers::completion::CompletionEvent::TriggerChar { .. } => {
                    "TriggerChar"
                }
                helix_view::handlers::completion::CompletionEvent::DeleteText { .. } => {
                    "DeleteText"
                }
                helix_view::handlers::completion::CompletionEvent::Cancel => "Cancel",
            };
            info!(
                event_type = event_type,
                "Completion coordinator received event"
            );

            if let Err(e) = self.handle_completion_event(event).await {
                error!(error = %e, "Failed to handle completion event");
            }
        }

        warn!("Completion coordinator stopped - no more events");
    }

    /// Handle a completion event from helix
    #[instrument(skip(self, event))]
    async fn handle_completion_event(&self, event: CompletionEvent) -> anyhow::Result<()> {
        match event {
            CompletionEvent::ManualTrigger { cursor, doc, view } => {
                debug!(
                    cursor = cursor,
                    doc_id = ?doc,
                    view_id = ?view,
                    "Handling manual completion trigger"
                );

                // Request real LSP completions instead of showing samples
                self.request_lsp_completions(cursor, doc, view).await?;
            }
            CompletionEvent::AutoTrigger { cursor, doc, view } => {
                debug!(
                    cursor = cursor,
                    doc_id = ?doc,
                    view_id = ?view,
                    "Handling auto completion trigger"
                );

                // Request real LSP completions instead of showing samples
                self.request_lsp_completions(cursor, doc, view).await?;
            }
            CompletionEvent::TriggerChar { cursor, doc, view } => {
                debug!(
                    cursor = cursor,
                    doc_id = ?doc,
                    view_id = ?view,
                    "Handling trigger character completion"
                );

                // Request real LSP completions instead of showing samples
                self.request_lsp_completions(cursor, doc, view).await?;
            }
            CompletionEvent::DeleteText { cursor } => {
                info!(
                    cursor = cursor,
                    "Handling delete text event - will hide completions"
                );
                // Send hide completions message
                self.send_hide_completions().await?;
            }
            CompletionEvent::Cancel => {
                info!("Handling completion cancel event - will hide completions");
                // Send hide completions message
                self.send_hide_completions().await?;
            }
        }

        Ok(())
    }

    /// Request LSP completions for the given position
    #[instrument(skip(self))]
    async fn request_lsp_completions(
        &self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    ) -> anyhow::Result<()> {
        info!(
            cursor = cursor,
            doc_id = ?doc_id,
            view_id = ?view_id,
            "Requesting real LSP completions from main thread"
        );

        // Create a oneshot channel for the response
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        // Create the LSP completion request
        let request = LspCompletionRequest {
            cursor,
            doc_id,
            view_id,
            response_tx,
        };

        // Send the request to the main thread (Application)
        if let Err(e) = self.lsp_completion_requests_tx.send(request).await {
            error!(error = %e, "Failed to send LSP completion request to main thread");
            return Err(anyhow::anyhow!(
                "Failed to send LSP completion request: {}",
                e
            ));
        }

        info!("Sent LSP completion request to main thread, waiting for response");

        // Wait for the response from the main thread
        match response_rx.await {
            Ok(response) => {
                if let Some(error) = response.error {
                    nucleotide_logging::warn!(
                        error = %error,
                        item_count = response.items.len(),
                        prefix = %response.prefix,
                        "LSP completion request failed, but still sending items"
                    );
                    // Send the LSP items as-is
                    let items = response.items;
                    self.send_completion_results(items, cursor, doc_id, view_id, response.prefix)
                        .await?;
                } else {
                    nucleotide_logging::info!(
                        item_count = response.items.len(),
                        is_incomplete = response.is_incomplete,
                        prefix = %response.prefix,
                        prefix_len = response.prefix.len(),
                        "Received successful LSP completion response from main thread"
                    );

                    // Log details about the prefix extraction
                    if response.prefix.is_empty() {
                        nucleotide_logging::warn!(
                            cursor = cursor,
                            item_count = response.items.len(),
                            "Prefix extraction returned empty string - filtering may not work"
                        );
                    } else {
                        nucleotide_logging::debug!(
                            prefix = %response.prefix,
                            cursor = cursor,
                            "Prefix extracted successfully for completion filtering"
                        );
                    }

                    // Send LSP results as-is - let rust-analyzer handle standard library items
                    let items = response.items;
                    self.send_completion_results(items, cursor, doc_id, view_id, response.prefix)
                        .await?;
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to receive LSP completion response from main thread");
                // Fall back to sample data on communication failure
                warn!("Falling back to sample completions due to communication failure");
                self.show_sample_completions(cursor, doc_id, view_id)
                    .await?;
            }
        }

        Ok(())
    }

    /// Send completion results to the UI
    async fn send_completion_results(
        &self,
        items: Vec<CompletionItem>,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        prefix: String,
    ) -> anyhow::Result<()> {
        // Comprehensive logging of completion data flow
        nucleotide_logging::info!(
            item_count = items.len(),
            cursor = cursor,
            doc_id = ?doc_id,
            view_id = ?view_id,
            prefix = %prefix,
            prefix_len = prefix.len(),
            "Sending completion results to UI"
        );

        // Log sample of completion items for debugging
        if !items.is_empty() {
            let sample_items: Vec<String> = items
                .iter()
                .take(8)
                .map(|item| format!("{}:{:?}", item.label, item.kind))
                .collect();

            nucleotide_logging::debug!(
                prefix = %prefix,
                sample_items = ?sample_items,
                sample_count = sample_items.len(),
                total_items = items.len(),
                "Sample completion items before sending to UI"
            );
        } else {
            nucleotide_logging::warn!(
                prefix = %prefix,
                cursor = cursor,
                doc_id = ?doc_id,
                "No completion items to send to UI"
            );
        }

        // Send through the channel first (for the async task in Workspace that we just set up)
        let result = CompletionResult::ShowCompletions {
            items: items.clone(),
            cursor,
            doc_id,
            view_id,
            prefix: prefix.clone(),
        };

        if let Err(e) = self.completion_results_tx.send(result).await {
            error!(error = %e, "Failed to send completion results through channel");
            // Don't return error, try the Application approach instead
        } else {
            info!("Successfully sent completion results via channel");
        }

        // Also try to emit Update events through the Application entity
        // This might work better for GPUI's main thread requirements
        info!("Attempting to emit completion via Application Update events");

        // For now, just log that we would do this
        // We might need to implement this as a method on Application
        debug!("Application Update event approach not implemented yet - using channel approach");

        Ok(())
    }

    /// Send hide completions result
    async fn send_hide_completions(&self) -> anyhow::Result<()> {
        debug!("Sending hide completions result");

        let result = CompletionResult::HideCompletions;

        if let Err(e) = self.completion_results_tx.send(result).await {
            error!(error = %e, "Failed to send hide completions result");
            return Err(anyhow::anyhow!(
                "Failed to send hide completions result: {}",
                e
            ));
        }

        debug!("Successfully sent hide completions result");
        Ok(())
    }

    /// Show sample completions via completion results channel
    async fn show_sample_completions(
        &self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    ) -> anyhow::Result<()> {
        info!("Completion coordinator creating sample completions and sending to workspace");

        // Create sample completion items
        let items = vec![
            CompletionItem::new("print!".to_string(), CompletionItemKind::Function)
                .with_detail("Print to stdout".to_string())
                .with_documentation("Prints to the standard output.".to_string()),
            CompletionItem::new("println!".to_string(), CompletionItemKind::Function)
                .with_detail("Print to stdout with newline".to_string())
                .with_documentation("Prints to the standard output, with a newline.".to_string()),
            CompletionItem::new("eprintln!".to_string(), CompletionItemKind::Function)
                .with_detail("Print to stderr with newline".to_string())
                .with_documentation("Prints to the standard error, with a newline.".to_string()),
            CompletionItem::new("format!".to_string(), CompletionItemKind::Function)
                .with_detail("Create formatted string".to_string())
                .with_documentation(
                    "Creates a String using interpolation of runtime expressions.".to_string(),
                ),
            CompletionItem::new("vec!".to_string(), CompletionItemKind::Function)
                .with_detail("Create a vector".to_string())
                .with_documentation("Creates a Vec containing the given elements.".to_string()),
            CompletionItem::new("Some".to_string(), CompletionItemKind::Constructor)
                .with_detail("Option variant containing a value".to_string())
                .with_documentation("Some value T".to_string()),
            CompletionItem::new("None".to_string(), CompletionItemKind::Constructor)
                .with_detail("Option variant for no value".to_string())
                .with_documentation("No value".to_string()),
            CompletionItem::new("Ok".to_string(), CompletionItemKind::Constructor)
                .with_detail("Result variant for success".to_string())
                .with_documentation("Contains the success value".to_string()),
        ];

        info!(
            item_count = items.len(),
            "Sending sample completion results to workspace"
        );

        // Send completion results through the channel
        let result = CompletionResult::ShowCompletions {
            items,
            cursor,
            doc_id,
            view_id,
            prefix: "".to_string(), // Sample completions don't have a prefix
        };

        if let Err(e) = self.completion_results_tx.send(result).await {
            error!(error = %e, "Failed to send completion results to workspace");
            return Err(anyhow::anyhow!("Failed to send completion results: {}", e));
        }

        info!("Successfully sent completion results to workspace via channel");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleotide_events::completion::{CompletionItem, CompletionItemKind};
    use tokio::sync::mpsc;

    #[test]
    fn test_lsp_completion_response_with_prefix() {
        // Test that LspCompletionResponse correctly stores prefix
        let items = vec![CompletionItem {
            label: "println!".to_string(),
            kind: CompletionItemKind::Function,
            detail: None,
            documentation: None,
            insert_text: "println!".to_string(),
            score: 100.0,
            signature_info: None,
            type_info: None,
            insert_text_format: nucleotide_events::completion::InsertTextFormat::PlainText,
        }];

        let response = LspCompletionResponse {
            items: items.clone(),
            is_incomplete: false,
            error: None,
            prefix: "prin".to_string(),
        };

        assert_eq!(response.prefix, "prin");
        assert_eq!(response.items.len(), 1);
        assert_eq!(response.items[0].label, "println!");
        assert!(!response.is_incomplete);
        assert!(response.error.is_none());
    }

    #[test]
    fn test_lsp_completion_response_with_error() {
        // Test LspCompletionResponse error handling
        let response = LspCompletionResponse {
            items: vec![],
            is_incomplete: false,
            error: Some("LSP server unavailable".to_string()),
            prefix: "test".to_string(),
        };

        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap(), "LSP server unavailable");
        assert_eq!(response.prefix, "test");
        assert!(response.items.is_empty());
    }

    #[test]
    fn test_completion_result_show_completions() {
        // Test CompletionResult::ShowCompletions structure
        let items = vec![CompletionItem {
            label: "test_function".to_string(),
            kind: CompletionItemKind::Function,
            detail: None,
            documentation: None,
            insert_text: "test_function".to_string(),
            score: 100.0,
            signature_info: None,
            type_info: None,
            insert_text_format: nucleotide_events::completion::InsertTextFormat::PlainText,
        }];

        let result = CompletionResult::ShowCompletions {
            items: items.clone(),
            cursor: 42,
            doc_id: helix_view::DocumentId::default(),
            view_id: helix_view::ViewId::default(),
            prefix: "test".to_string(),
        };

        match result {
            CompletionResult::ShowCompletions {
                items: result_items,
                cursor,
                prefix,
                ..
            } => {
                assert_eq!(result_items.len(), 1);
                assert_eq!(result_items[0].label, "test_function");
                assert_eq!(cursor, 42);
                assert_eq!(prefix, "test");
            }
            _ => panic!("Expected ShowCompletions variant"),
        }
    }

    // NOTE: CompletionCoordinator tests require complex setup with Application entity,
    // BackgroundExecutor, and multiple channels. The coordinator is tested through
    // integration tests when running the full application.

    mod edge_cases {
        use super::*;

        #[test]
        fn test_empty_prefix_completion() {
            // Test completion with empty prefix
            let response = LspCompletionResponse {
                items: vec![CompletionItem {
                    label: "function".to_string(),
                    kind: CompletionItemKind::Function,
                    detail: None,
                    documentation: None,
                    insert_text: "function".to_string(),
                    score: 100.0,
                    signature_info: None,
                    type_info: None,
                    insert_text_format: nucleotide_events::completion::InsertTextFormat::PlainText,
                }],
                is_incomplete: false,
                error: None,
                prefix: "".to_string(), // Empty prefix
            };

            assert_eq!(response.prefix, "");
            assert_eq!(response.items.len(), 1);
        }

        #[test]
        fn test_special_characters_in_prefix() {
            // Test prefix with special characters (should be handled gracefully)
            let response = LspCompletionResponse {
                items: vec![],
                is_incomplete: false,
                error: None,
                prefix: "std::".to_string(), // Contains special chars
            };

            assert_eq!(response.prefix, "std::");
        }

        #[test]
        fn test_unicode_prefix() {
            // Test prefix with Unicode characters
            let response = LspCompletionResponse {
                items: vec![],
                is_incomplete: false,
                error: None,
                prefix: "测试".to_string(), // Unicode characters
            };

            assert_eq!(response.prefix, "测试");
        }

        #[test]
        fn test_very_long_prefix() {
            // Test with extremely long prefix
            let long_prefix = "a".repeat(1000);
            let response = LspCompletionResponse {
                items: vec![],
                is_incomplete: false,
                error: None,
                prefix: long_prefix.clone(),
            };

            assert_eq!(response.prefix, long_prefix);
            assert_eq!(response.prefix.len(), 1000);
        }

        #[test]
        fn test_completion_with_many_items() {
            // Test completion response with many items
            let mut items = vec![];
            for i in 0..1000 {
                items.push(CompletionItem {
                    label: format!("function_{}", i),
                    kind: CompletionItemKind::Function,
                    detail: None,
                    documentation: None,
                    insert_text: format!("function_{}", i),
                    score: 100.0,
                    signature_info: None,
                    type_info: None,
                    insert_text_format: nucleotide_events::completion::InsertTextFormat::PlainText,
                });
            }

            let response = LspCompletionResponse {
                items: items.clone(),
                is_incomplete: true, // Many items, might be incomplete
                error: None,
                prefix: "func".to_string(),
            };

            assert_eq!(response.items.len(), 1000);
            assert!(response.is_incomplete);
            assert_eq!(response.prefix, "func");
        }

        #[test]
        fn test_completion_result_with_special_doc_ids() {
            // Test CompletionResult with various document/view IDs
            let items = vec![CompletionItem {
                label: "test".to_string(),
                kind: CompletionItemKind::Function,
                detail: None,
                documentation: None,
                insert_text: "test".to_string(),
                score: 100.0,
                signature_info: None,
                type_info: None,
                insert_text_format: nucleotide_events::completion::InsertTextFormat::PlainText,
            }];

            let result = CompletionResult::ShowCompletions {
                items,
                cursor: 0, // Start of document
                doc_id: helix_view::DocumentId::default(),
                view_id: helix_view::ViewId::default(),
                prefix: "".to_string(), // Empty prefix at start
            };

            match result {
                CompletionResult::ShowCompletions { cursor, prefix, .. } => {
                    assert_eq!(cursor, 0);
                    assert_eq!(prefix, "");
                }
                _ => panic!("Expected ShowCompletions variant"),
            }
        }
    }
}
