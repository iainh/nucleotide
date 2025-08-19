// ABOUTME: Coordinates LSP completion requests with nucleotide UI
// ABOUTME: Receives completion events from helix and manages the completion flow

use gpui::{AppContext, BackgroundExecutor, Entity};
use helix_view::handlers::completion::CompletionEvent;
use nucleotide_events::completion_events::{CompletionEventItem, CompletionResult};
use nucleotide_logging::{debug, error, info, instrument, warn};
use tokio::sync::mpsc::{Receiver, Sender};

use crate::Application;

/// Coordinates completion events from helix with nucleotide UI
pub struct CompletionCoordinator {
    /// Receiver for completion events from helix
    completion_rx: Receiver<CompletionEvent>,
    /// Sender for completion results to workspace
    completion_results_tx: Sender<CompletionResult>,
    /// Sender for LSP completion requests to application
    lsp_completion_requests_tx: Sender<nucleotide_events::completion_events::LspCompletionRequest>,
    /// Reference to core application
    core: Entity<Application>,
    /// Background executor for running tasks
    background_executor: BackgroundExecutor,
}

impl CompletionCoordinator {
    pub fn new(
        completion_rx: Receiver<CompletionEvent>,
        completion_results_tx: Sender<CompletionResult>,
        lsp_completion_requests_tx: Sender<
            nucleotide_events::completion_events::LspCompletionRequest,
        >,
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
                debug!(cursor = cursor, "Handling delete text event");
                // Send hide completions message
                self.send_hide_completions().await?;
            }
            CompletionEvent::Cancel => {
                debug!("Handling completion cancel event");
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
        let request = nucleotide_events::completion_events::LspCompletionRequest {
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
        items: Vec<CompletionEventItem>,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    ) -> anyhow::Result<()> {
        debug!(item_count = items.len(), "Sending completion results to UI");

        let result = CompletionResult::ShowCompletions {
            items,
            cursor,
            doc_id,
            view_id,
        };

        if let Err(e) = self.completion_results_tx.send(result).await {
            error!(error = %e, "Failed to send completion results");
            return Err(anyhow::anyhow!("Failed to send completion results: {}", e));
        }

        debug!("Successfully sent completion results to UI");
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
            CompletionEventItem {
                text: "print!".to_string(),
                kind: "Function".to_string(),
                description: Some("Print to stdout".to_string()),
                documentation: Some("Prints to the standard output.".to_string()),
            },
            CompletionEventItem {
                text: "println!".to_string(),
                kind: "Function".to_string(),
                description: Some("Print to stdout with newline".to_string()),
                documentation: Some("Prints to the standard output, with a newline.".to_string()),
            },
            CompletionEventItem {
                text: "eprintln!".to_string(),
                kind: "Function".to_string(),
                description: Some("Print to stderr with newline".to_string()),
                documentation: Some("Prints to the standard error, with a newline.".to_string()),
            },
            CompletionEventItem {
                text: "format!".to_string(),
                kind: "Function".to_string(),
                description: Some("Create formatted string".to_string()),
                documentation: Some(
                    "Creates a String using interpolation of runtime expressions.".to_string(),
                ),
            },
            CompletionEventItem {
                text: "vec!".to_string(),
                kind: "Function".to_string(),
                description: Some("Create a vector".to_string()),
                documentation: Some("Creates a Vec containing the given elements.".to_string()),
            },
            CompletionEventItem {
                text: "Some".to_string(),
                kind: "Constructor".to_string(),
                description: Some("Option variant containing a value".to_string()),
                documentation: Some("Some value T".to_string()),
            },
            CompletionEventItem {
                text: "None".to_string(),
                kind: "Constructor".to_string(),
                description: Some("Option variant for no value".to_string()),
                documentation: Some("No value".to_string()),
            },
            CompletionEventItem {
                text: "Ok".to_string(),
                kind: "Constructor".to_string(),
                description: Some("Result variant for success".to_string()),
                documentation: Some("Contains the success value".to_string()),
            },
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
