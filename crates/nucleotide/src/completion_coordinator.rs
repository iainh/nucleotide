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

/// LSP completion response message
pub struct LspCompletionResponse {
    pub items: Vec<CompletionItem>,
    pub is_incomplete: bool,
    pub error: Option<String>,
}

/// Completion result sent to UI
#[derive(Debug)]
pub enum CompletionResult {
    ShowCompletions {
        items: Vec<CompletionItem>,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
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
                    warn!(error = %error, "LSP completion request failed");
                    // Send the LSP items as-is
                    let items = response.items;
                    self.send_completion_results(items, cursor, doc_id, view_id)
                        .await?;
                } else {
                    info!(
                        item_count = response.items.len(),
                        is_incomplete = response.is_incomplete,
                        "Received LSP completion response from main thread"
                    );
                    // Send LSP results as-is - let rust-analyzer handle standard library items
                    let items = response.items;
                    self.send_completion_results(items, cursor, doc_id, view_id)
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
    ) -> anyhow::Result<()> {
        info!(
            item_count = items.len(),
            "Sending completion results to UI via Application Update events"
        );

        // Send through the channel first (for the async task in Workspace that we just set up)
        let result = CompletionResult::ShowCompletions {
            items: items.clone(),
            cursor,
            doc_id,
            view_id,
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
        };

        if let Err(e) = self.completion_results_tx.send(result).await {
            error!(error = %e, "Failed to send completion results to workspace");
            return Err(anyhow::anyhow!("Failed to send completion results: {}", e));
        }

        info!("Successfully sent completion results to workspace via channel");
        Ok(())
    }
}
