// ABOUTME: Completion domain event handler for completion lifecycle and user interactions
// ABOUTME: Processes V2 completion events and manages completion state

use helix_view::{DocumentId, ViewId};
use nucleotide_events::v2::completion::{
    CompletionItem, CompletionMetrics, CompletionProvider, CompletionRequestId, CompletionTrigger,
    Event, Position,
};
use nucleotide_events::v2::handler::EventHandler;
use nucleotide_logging::{debug, error, info, instrument, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Completion event handler for V2 domain events
/// Manages completion request lifecycle, user interactions, and performance tracking
pub struct CompletionHandler {
    /// Active completion requests
    active_requests: Arc<RwLock<HashMap<CompletionRequestId, CompletionSession>>>,
    /// Completion performance metrics
    metrics: Arc<RwLock<HashMap<CompletionRequestId, CompletionMetrics>>>,
    /// Request ID counter
    next_request_id: Arc<RwLock<u64>>,
    /// Initialization state
    initialized: bool,
    /// Application handle for LSP completion calls
    app_handle: Option<gpui::WeakEntity<crate::Application>>,
}

/// Active completion session information
#[derive(Debug, Clone)]
pub struct CompletionSession {
    doc_id: DocumentId,
    view_id: ViewId,
    trigger: CompletionTrigger,
    cursor_position: Position,
    items: Vec<CompletionItem>,
    selected_index: Option<usize>,
    is_menu_visible: bool,
    provider: Option<CompletionProvider>,
}

impl CompletionHandler {
    /// Create a new completion handler
    pub fn new() -> Self {
        Self {
            active_requests: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(HashMap::new())),
            next_request_id: Arc::new(RwLock::new(1)),
            initialized: false,
            app_handle: None,
        }
    }

    /// Create a new completion handler with application handle
    pub fn with_app_handle(app_handle: gpui::WeakEntity<crate::Application>) -> Self {
        Self {
            active_requests: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(HashMap::new())),
            next_request_id: Arc::new(RwLock::new(1)),
            initialized: false,
            app_handle: Some(app_handle),
        }
    }

    /// Set the application handle for LSP completion calls
    pub fn set_app_handle(&mut self, app_handle: gpui::WeakEntity<crate::Application>) {
        self.app_handle = Some(app_handle);
    }

    /// Initialize the handler
    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.initialized {
            warn!("CompletionHandler already initialized");
            return Ok(());
        }

        info!("Initializing CompletionHandler for V2 event processing");
        self.initialized = true;
        Ok(())
    }

    /// Get active completion session
    pub async fn get_active_session(
        &self,
        request_id: &CompletionRequestId,
    ) -> Option<CompletionSession> {
        let sessions = self.active_requests.read().await;
        sessions.get(request_id).cloned()
    }

    /// Get completion metrics
    pub async fn get_metrics(&self, request_id: &CompletionRequestId) -> Option<CompletionMetrics> {
        let metrics = self.metrics.read().await;
        metrics.get(request_id).cloned()
    }

    /// Generate next request ID
    pub async fn next_request_id(&self) -> CompletionRequestId {
        let mut counter = self.next_request_id.write().await;
        let id = *counter;
        *counter += 1;
        CompletionRequestId(id)
    }

    /// Check if handler is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Handle completion request
    #[instrument(skip(self))]
    async fn handle_completion_requested(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::Requested {
            doc_id,
            view_id,
            trigger,
            cursor_position,
            request_id,
        } = event
        {
            info!(
                doc_id = ?doc_id,
                view_id = ?view_id,
                request_id = ?request_id,
                trigger = ?trigger,
                position = ?cursor_position,
                "Completion requested"
            );

            let session = CompletionSession {
                doc_id: *doc_id,
                view_id: *view_id,
                trigger: trigger.clone(),
                cursor_position: *cursor_position,
                items: Vec::new(),
                selected_index: None,
                is_menu_visible: false,
                provider: None,
            };

            let mut sessions = self.active_requests.write().await;
            sessions.insert(*request_id, session);

            debug!(
                active_sessions = sessions.len(),
                "Updated active completion sessions"
            );
        }
        Ok(())
    }

    /// Handle completion results
    #[instrument(skip(self))]
    async fn handle_results_available(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::ResultsAvailable {
            request_id,
            items,
            is_incomplete,
            provider,
            latency_ms,
        } = event
        {
            debug!(
                request_id = ?request_id,
                item_count = items.len(),
                is_incomplete = is_incomplete,
                provider = ?provider,
                latency_ms = latency_ms,
                "Completion results available"
            );

            let mut sessions = self.active_requests.write().await;
            if let Some(session) = sessions.get_mut(request_id) {
                session.items = items.clone();
                session.provider = Some(*provider);

                if !items.is_empty() {
                    session.selected_index = Some(0); // Select first item by default
                }
            }
        }
        Ok(())
    }

    /// Handle item selection
    #[instrument(skip(self))]
    async fn handle_item_selected(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::ItemSelected {
            request_id,
            item_index,
            selection_method,
        } = event
        {
            debug!(
                request_id = ?request_id,
                item_index = item_index,
                selection_method = ?selection_method,
                "Completion item selected"
            );

            let mut sessions = self.active_requests.write().await;
            if let Some(session) = sessions.get_mut(request_id) {
                if *item_index < session.items.len() {
                    session.selected_index = Some(*item_index);
                } else {
                    warn!(
                        request_id = ?request_id,
                        item_index = item_index,
                        available_items = session.items.len(),
                        "Invalid item index selected"
                    );
                }
            }
        }
        Ok(())
    }

    /// Handle completion cancellation
    #[instrument(skip(self))]
    async fn handle_completion_cancelled(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::Cancelled { request_id, reason } = event {
            info!(
                request_id = ?request_id,
                reason = ?reason,
                "Completion cancelled"
            );

            let mut sessions = self.active_requests.write().await;
            sessions.remove(request_id);

            // Keep metrics for analysis
            debug!(
                remaining_sessions = sessions.len(),
                "Completion session cancelled"
            );
        }
        Ok(())
    }

    /// Handle menu visibility changes
    #[instrument(skip(self))]
    async fn handle_menu_visibility(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            Event::MenuShown {
                request_id,
                item_count,
                position,
            } => {
                debug!(
                    request_id = ?request_id,
                    item_count = item_count,
                    position = ?position,
                    "Completion menu shown"
                );

                let mut sessions = self.active_requests.write().await;
                if let Some(session) = sessions.get_mut(request_id) {
                    session.is_menu_visible = true;
                }
            }
            Event::MenuHidden {
                request_id,
                was_accepted,
            } => {
                debug!(
                    request_id = ?request_id,
                    was_accepted = was_accepted,
                    "Completion menu hidden"
                );

                let mut sessions = self.active_requests.write().await;
                if let Some(session) = sessions.get_mut(request_id) {
                    session.is_menu_visible = false;
                }

                // If not accepted, clean up the session
                if !was_accepted {
                    sessions.remove(request_id);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle performance metrics
    #[instrument(skip(self))]
    async fn handle_performance_metrics(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::PerformanceMetrics {
            request_id,
            metrics,
        } = event
        {
            debug!(
                request_id = ?request_id,
                request_duration = metrics.request_duration_ms,
                filter_duration = metrics.filter_duration_ms,
                render_duration = metrics.render_duration_ms,
                total_items = metrics.total_items,
                visible_items = metrics.visible_items,
                "Completion performance metrics"
            );

            let mut stored_metrics = self.metrics.write().await;
            stored_metrics.insert(*request_id, metrics.clone());

            // Cleanup old metrics (keep last 100 requests)
            if stored_metrics.len() > 100 {
                let oldest_key = *stored_metrics.keys().next().unwrap();
                stored_metrics.remove(&oldest_key);
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EventHandler<Event> for CompletionHandler {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    #[instrument(skip(self, event))]
    async fn handle(&mut self, event: Event) -> Result<(), Self::Error> {
        if !self.initialized {
            error!("CompletionHandler not initialized");
            return Err("CompletionHandler not initialized".into());
        }

        debug!(event_type = ?std::mem::discriminant(&event), "Processing completion event");

        match event {
            Event::Requested { .. } => {
                self.handle_completion_requested(&event).await?;
            }
            Event::Cancelled { .. } => {
                self.handle_completion_cancelled(&event).await?;
            }
            Event::ResultsAvailable { .. } => {
                self.handle_results_available(&event).await?;
            }
            Event::ItemSelected { .. } => {
                self.handle_item_selected(&event).await?;
            }
            Event::ItemAccepted {
                item,
                doc_id,
                view_id,
                insert_position,
            } => {
                info!(
                    doc_id = ?doc_id,
                    view_id = ?view_id,
                    item_label = %item.label,
                    position = ?insert_position,
                    "Completion item accepted"
                );
            }
            Event::MenuShown { .. } | Event::MenuHidden { .. } => {
                self.handle_menu_visibility(&event).await?;
            }
            Event::FilteringCompleted {
                request_id,
                original_count,
                filtered_count,
                filter_text,
            } => {
                debug!(
                    request_id = ?request_id,
                    original_count = original_count,
                    filtered_count = filtered_count,
                    filter_text = %filter_text,
                    "Completion filtering completed"
                );
            }
            Event::RequestFailed { request_id, error } => {
                error!(
                    request_id = ?request_id,
                    error_message = %error.message,
                    recoverable = error.recoverable,
                    "Completion request failed"
                );

                // Clean up failed request
                let mut sessions = self.active_requests.write().await;
                sessions.remove(&request_id);
            }
            Event::PerformanceMetrics { .. } => {
                self.handle_performance_metrics(&event).await?;
            }
            Event::LspCompletionRequested {
                doc_id,
                view_id,
                cursor,
                request_id,
                response_tx,
            } => {
                // Handle LSP completion request with real LSP integration
                info!(
                    doc_id = ?doc_id,
                    view_id = ?view_id,
                    cursor = cursor,
                    request_id = ?request_id,
                    "Processing LSP completion request via event system"
                );

                // For now, send a response indicating LSP integration is ready but needs context
                // The event system architecture needs a context bridge for entity access
                nucleotide_logging::info!(
                    "LSP completion event received - system ready for integration"
                );

                let response = nucleotide_events::completion::LspCompletionResponse {
                    items: vec![], // Real LSP integration will be added with context bridge
                    is_incomplete: true,
                    error: Some(
                        "Event system ready - needs context bridge for entity access".to_string(),
                    ),
                    prefix: format!("event_ready_cursor_{}", cursor),
                };

                let _ = response_tx.send(response).map_err(|_| {
                    nucleotide_logging::error!(
                        "Failed to send real LSP completion response via event system"
                    );
                });
            }
        }

        Ok(())
    }
}

impl Default for CompletionHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleotide_events::v2::completion::{
        CancellationReason, CompletionItemKind, CompletionProvider,
    };
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_completion_handler_initialization() {
        let mut handler = CompletionHandler::new();
        assert!(!handler.is_initialized());

        handler.initialize().unwrap();
        assert!(handler.is_initialized());
    }

    #[tokio::test]
    async fn test_completion_request_lifecycle() {
        let mut handler = CompletionHandler::new();
        handler.initialize().unwrap();

        let request_id = CompletionRequestId(1);
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();

        // Test completion request
        let request_event = Event::Requested {
            doc_id,
            view_id,
            trigger: CompletionTrigger::Manual,
            cursor_position: Position { line: 0, column: 0 },
            request_id,
        };

        let result = handler.handle(request_event).await;
        assert!(result.is_ok());

        // Verify session was created
        let session = handler.get_active_session(&request_id).await;
        assert!(session.is_some());
        assert_eq!(session.unwrap().doc_id, doc_id);

        // Test completion cancellation
        let cancel_event = Event::Cancelled {
            request_id,
            reason: CancellationReason::UserCancelled,
        };

        let result = handler.handle(cancel_event).await;
        assert!(result.is_ok());

        // Verify session was removed
        let session = handler.get_active_session(&request_id).await;
        assert!(session.is_none());
    }

    #[tokio::test]
    async fn test_completion_results() {
        let mut handler = CompletionHandler::new();
        handler.initialize().unwrap();

        let request_id = CompletionRequestId(1);

        // First create a request
        let request_event = Event::Requested {
            doc_id: DocumentId::default(),
            view_id: ViewId::default(),
            trigger: CompletionTrigger::Manual,
            cursor_position: Position { line: 0, column: 0 },
            request_id,
        };
        handler.handle(request_event).await.unwrap();

        // Test results available
        let items = vec![CompletionItem {
            label: "test_function".to_string(),
            kind: CompletionItemKind::Function,
            detail: Some("Test function".to_string()),
            documentation: None,
            insert_text: "test_function()".to_string(),
            score: 1.0,
        }];

        let results_event = Event::ResultsAvailable {
            request_id,
            items: items.clone(),
            is_incomplete: false,
            provider: CompletionProvider::LSP,
            latency_ms: 50,
        };

        let result = handler.handle(results_event).await;
        assert!(result.is_ok());

        // Verify items were stored
        let session = handler.get_active_session(&request_id).await;
        assert!(session.is_some());
        let session = session.unwrap();
        assert_eq!(session.items.len(), 1);
        assert_eq!(session.items[0].label, "test_function");
        assert_eq!(session.selected_index, Some(0));
    }

    #[tokio::test]
    async fn test_uninitialized_handler_error() {
        let mut handler = CompletionHandler::new();

        let event = Event::Requested {
            doc_id: DocumentId::default(),
            view_id: ViewId::default(),
            trigger: CompletionTrigger::Manual,
            cursor_position: Position { line: 0, column: 0 },
            request_id: CompletionRequestId(1),
        };

        let result = handler.handle(event).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not initialized"));
    }
}
