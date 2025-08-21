// ABOUTME: Core application logic extracted from Application struct
// ABOUTME: Handles V2 event processing and central coordination logic

use helix_view::graphics::Rect;
use helix_view::{DocumentId, ViewId};
use nucleotide_events::v2::handler::EventHandler;
use nucleotide_logging::{debug, error, info, instrument, warn};
use std::sync::Arc;

use crate::application::{
    CompletionHandler, DocumentHandler, EditorHandler, LspHandler, ViewHandler, WorkspaceHandler,
};
use nucleotide_core::event_bridge;

/// Core application logic for event processing and coordination
/// Separated from the main Application struct to reduce complexity
pub struct ApplicationCore {
    /// V2 Event System Handlers - Phase 1
    pub document_handler: DocumentHandler,
    pub view_handler: ViewHandler,
    pub editor_handler: EditorHandler,

    /// V2 Event System Handlers - Phase 2
    pub lsp_handler: LspHandler,
    pub completion_handler: CompletionHandler,
    pub workspace_handler: WorkspaceHandler,

    /// Initialization state
    initialized: bool,
}

impl ApplicationCore {
    /// Create a new application core instance
    pub fn new() -> Self {
        Self {
            document_handler: DocumentHandler::new(),
            view_handler: ViewHandler::new(),
            editor_handler: EditorHandler::new(),
            lsp_handler: LspHandler::new(),
            completion_handler: CompletionHandler::new(),
            workspace_handler: WorkspaceHandler::new(),
            initialized: false,
        }
    }

    /// Initialize all event handlers
    #[instrument(skip(self))]
    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.initialized {
            warn!("ApplicationCore already initialized");
            return Ok(());
        }

        info!("Initializing ApplicationCore with V2 event handlers");

        // Initialize Phase 1 handlers
        self.document_handler
            .initialize()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.view_handler
            .initialize()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.editor_handler
            .initialize()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Initialize Phase 2 handlers
        self.lsp_handler
            .initialize()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.completion_handler
            .initialize()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.workspace_handler
            .initialize()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        self.initialized = true;
        info!("ApplicationCore initialized successfully");
        Ok(())
    }

    /// Process events through V2 event system domain handlers
    /// This is the core event processing logic extracted from Application
    #[instrument(skip(self, bridged_event, editor))]
    pub async fn process_v2_event(
        &mut self,
        bridged_event: &event_bridge::BridgedEvent,
        editor: &mut helix_view::Editor,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.initialized {
            error!("ApplicationCore not initialized");
            return Err("ApplicationCore not initialized".into());
        }

        use nucleotide_events::v2::document::ChangeType;
        use nucleotide_events::v2::document::Event as DocumentEvent;

        // Process V2 events for all supported event types
        match bridged_event {
            event_bridge::BridgedEvent::DocumentChanged { doc_id } => {
                // Extract actual document revision
                let revision = if let Some(document) = editor.document_mut(*doc_id) {
                    document.get_current_revision() as u64
                } else {
                    warn!(doc_id = ?doc_id, "Document not found when processing DocumentChanged event");
                    0
                };

                // Create a V2 document event with actual revision
                let v2_event = DocumentEvent::ContentChanged {
                    doc_id: *doc_id,
                    revision,
                    change_summary: ChangeType::Insert, // TODO: Determine actual change type based on operation
                };

                debug!(
                    doc_id = ?doc_id,
                    revision = revision,
                    "Processing DocumentChanged through V2 handler"
                );

                self.document_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::SelectionChanged { doc_id, view_id } => {
                // Extract actual selection from the view
                let (selection, was_movement) = if let Some(view) = editor.tree.get(*view_id) {
                    (view.doc_selection(*doc_id).clone(), true) // Assume movement for now
                } else {
                    warn!(view_id = ?view_id, "View not found when processing SelectionChanged event");
                    (helix_core::Selection::point(0), false)
                };

                let v2_event = nucleotide_events::v2::view::Event::SelectionChanged {
                    view_id: *view_id,
                    doc_id: *doc_id,
                    selection,
                    was_movement,
                };

                debug!(
                    doc_id = ?doc_id,
                    view_id = ?view_id,
                    "Processing SelectionChanged through V2 ViewHandler"
                );

                self.view_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::ModeChanged { old_mode, new_mode } => {
                let v2_event = nucleotide_events::v2::editor::Event::ModeChanged {
                    previous_mode: *old_mode,
                    new_mode: *new_mode,
                    context: nucleotide_events::v2::editor::ModeChangeContext::UserAction,
                };

                debug!(
                    old_mode = ?old_mode,
                    new_mode = ?new_mode,
                    "Processing ModeChanged through V2 EditorHandler"
                );

                self.editor_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::DocumentOpened { doc_id } => {
                // Extract document information for enriched event
                let (path, language_id) = if let Some(document) = editor.document(*doc_id) {
                    let path = document
                        .path()
                        .cloned()
                        .unwrap_or_else(|| std::path::PathBuf::from("untitled"));
                    let language_id = document.language().map(|lang| lang.to_string());
                    (path, language_id)
                } else {
                    warn!(doc_id = ?doc_id, "Document not found when processing DocumentOpened event");
                    (std::path::PathBuf::from("unknown"), None)
                };

                let v2_event = DocumentEvent::Opened {
                    doc_id: *doc_id,
                    path,
                    language_id,
                };

                debug!(doc_id = ?doc_id, "Processing DocumentOpened through V2 DocumentHandler");

                self.document_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::DocumentClosed { doc_id } => {
                // Note: By the time we get this event, the document might already be removed
                // So we use placeholder data
                let v2_event = DocumentEvent::Closed {
                    doc_id: *doc_id,
                    path: std::path::PathBuf::from("closed_document"),
                };

                debug!(doc_id = ?doc_id, "Processing DocumentClosed through V2 DocumentHandler");

                self.document_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::DiagnosticsChanged { doc_id } => {
                // Extract diagnostic counts from the document
                let (diagnostic_count, error_count, warning_count) = if let Some(document) =
                    editor.document(*doc_id)
                {
                    let diagnostics = document.diagnostics();
                    let total = diagnostics.len();
                    let errors = diagnostics
                        .iter()
                        .filter(|d| d.severity == Some(helix_lsp::lsp::DiagnosticSeverity::ERROR))
                        .count();
                    let warnings = diagnostics
                        .iter()
                        .filter(|d| d.severity == Some(helix_lsp::lsp::DiagnosticSeverity::WARNING))
                        .count();
                    (total, errors, warnings)
                } else {
                    (0, 0, 0)
                };

                let v2_event = DocumentEvent::DiagnosticsUpdated {
                    doc_id: *doc_id,
                    diagnostic_count: diagnostic_count as u32,
                    error_count: error_count as u32,
                    warning_count: warning_count as u32,
                };

                debug!(
                    doc_id = ?doc_id,
                    diagnostic_count = diagnostic_count,
                    error_count = error_count,
                    warning_count = warning_count,
                    "Processing DiagnosticsChanged through V2 DocumentHandler"
                );

                self.document_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::ViewFocused { view_id } => {
                // Extract associated document ID from the view
                let (doc_id, previous_view) = if let Some(view) = editor.tree.get(*view_id) {
                    let doc_id = view.doc;
                    let previous_view = self.view_handler.get_focused_view();
                    (doc_id, previous_view)
                } else {
                    warn!(view_id = ?view_id, "View not found when processing ViewFocused event");
                    (DocumentId::default(), None)
                };

                let v2_event = nucleotide_events::v2::view::Event::Focused {
                    view_id: *view_id,
                    doc_id,
                    previous_view,
                };

                debug!(
                    view_id = ?view_id,
                    doc_id = ?doc_id,
                    "Processing ViewFocused through V2 ViewHandler"
                );

                self.view_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            // Phase 2 Events - LSP Integration
            event_bridge::BridgedEvent::LanguageServerInitialized { server_id } => {
                // Create LSP server initialized event
                let v2_event = nucleotide_events::v2::lsp::Event::ServerInitialized {
                    server_id: *server_id,
                    server_name: "unknown".to_string(), // TODO: Extract actual server name
                    capabilities: nucleotide_events::v2::lsp::ServerCapabilities::new(),
                    workspace_root: std::path::PathBuf::from("unknown"), // TODO: Extract workspace root
                };

                debug!(
                    server_id = ?server_id,
                    "Processing LanguageServerInitialized through V2 LspHandler"
                );

                self.lsp_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::LanguageServerExited { server_id } => {
                let v2_event = nucleotide_events::v2::lsp::Event::ServerExited {
                    server_id: *server_id,
                    server_name: "unknown".to_string(), // TODO: Extract actual server name
                    exit_code: None,                    // TODO: Extract actual exit code
                    workspace_root: std::path::PathBuf::from("unknown"), // TODO: Extract workspace root
                };

                debug!(
                    server_id = ?server_id,
                    "Processing LanguageServerExited through V2 LspHandler"
                );

                self.lsp_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            event_bridge::BridgedEvent::LspServerStartupRequested {
                workspace_root,
                server_name,
                language_id,
            } => {
                let v2_event = nucleotide_events::v2::lsp::Event::ServerStartupRequested {
                    workspace_root: workspace_root.clone(),
                    server_name: server_name.clone(),
                    language_id: language_id.clone(),
                };

                debug!(
                    workspace = %workspace_root.display(),
                    server_name = %server_name,
                    language_id = %language_id,
                    "Processing LspServerStartupRequested through V2 LspHandler"
                );

                self.lsp_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            // Phase 2 Events - Completion Integration
            event_bridge::BridgedEvent::CompletionRequested {
                doc_id,
                view_id,
                trigger,
            } => {
                // Generate a unique request ID
                let request_id = self.completion_handler.next_request_id().await;

                let v2_event = nucleotide_events::v2::completion::Event::Requested {
                    doc_id: *doc_id,
                    view_id: *view_id,
                    trigger: match trigger {
                        nucleotide_types::CompletionTrigger::Invoked => {
                            nucleotide_events::v2::completion::CompletionTrigger::Manual
                        }
                        nucleotide_types::CompletionTrigger::TriggerCharacter(ch) => {
                            nucleotide_events::v2::completion::CompletionTrigger::Character(*ch)
                        }
                        nucleotide_types::CompletionTrigger::TriggerForIncompleteCompletions => {
                            nucleotide_events::v2::completion::CompletionTrigger::Automatic
                        }
                    },
                    cursor_position: nucleotide_events::v2::completion::Position {
                        line: 0,
                        column: 0,
                    }, // TODO: Extract actual cursor position
                    request_id,
                };

                debug!(
                    doc_id = ?doc_id,
                    view_id = ?view_id,
                    request_id = ?request_id,
                    "Processing CompletionRequested through V2 CompletionHandler"
                );

                self.completion_handler
                    .handle(v2_event)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            }

            _ => {
                debug!(event = ?bridged_event, "V2 processing not yet implemented for this event type");
                // Other workspace events will be integrated as needed
            }
        }

        Ok(())
    }

    /// Get access to document handler for external coordination
    pub fn document_handler(&self) -> &DocumentHandler {
        &self.document_handler
    }

    /// Get mutable access to document handler
    pub fn document_handler_mut(&mut self) -> &mut DocumentHandler {
        &mut self.document_handler
    }

    /// Get access to view handler for external coordination
    pub fn view_handler(&self) -> &ViewHandler {
        &self.view_handler
    }

    /// Get mutable access to view handler
    pub fn view_handler_mut(&mut self) -> &mut ViewHandler {
        &mut self.view_handler
    }

    /// Get access to editor handler for external coordination
    pub fn editor_handler(&self) -> &EditorHandler {
        &self.editor_handler
    }

    /// Get mutable access to editor handler
    pub fn editor_handler_mut(&mut self) -> &mut EditorHandler {
        &mut self.editor_handler
    }

    /// Get access to LSP handler for external coordination
    pub fn lsp_handler(&self) -> &LspHandler {
        &self.lsp_handler
    }

    /// Get mutable access to LSP handler
    pub fn lsp_handler_mut(&mut self) -> &mut LspHandler {
        &mut self.lsp_handler
    }

    /// Get access to completion handler for external coordination
    pub fn completion_handler(&self) -> &CompletionHandler {
        &self.completion_handler
    }

    /// Get mutable access to completion handler
    pub fn completion_handler_mut(&mut self) -> &mut CompletionHandler {
        &mut self.completion_handler
    }

    /// Get access to workspace handler for external coordination
    pub fn workspace_handler(&self) -> &WorkspaceHandler {
        &self.workspace_handler
    }

    /// Get mutable access to workspace handler
    pub fn workspace_handler_mut(&mut self) -> &mut WorkspaceHandler {
        &mut self.workspace_handler
    }

    /// Check if the core is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

impl Default for ApplicationCore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::{Document, DocumentId, Editor};
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_application_core_initialization() {
        let mut core = ApplicationCore::new();
        assert!(!core.is_initialized());

        core.initialize().unwrap();
        assert!(core.is_initialized());
    }

    #[tokio::test]
    async fn test_document_changed_event_processing() {
        let mut core = ApplicationCore::new();
        core.initialize().unwrap();

        let mut editor = Editor::new(
            Rect::default(),
            Arc::new(helix_core::syntax::Loader::new(&[])),
            Arc::new(parking_lot::RwLock::new(
                helix_view::theme::Loader::new(&[]),
            )),
            Some(Box::new(|| {
                Arc::new(helix_view::handlers::Handlers::default())
            })),
        );

        let doc_id = DocumentId::default();
        let event = event_bridge::BridgedEvent::DocumentChanged { doc_id };

        // This should not crash even with a missing document
        let result = core.process_v2_event(&event, &mut editor).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mode_changed_event_processing() {
        let mut core = ApplicationCore::new();
        core.initialize().unwrap();

        let mut editor = Editor::new(
            Rect::default(),
            Arc::new(helix_core::syntax::Loader::new(&[])),
            Arc::new(parking_lot::RwLock::new(
                helix_view::theme::Loader::new(&[]),
            )),
            Some(Box::new(|| {
                Arc::new(helix_view::handlers::Handlers::default())
            })),
        );

        let event = event_bridge::BridgedEvent::ModeChanged {
            old_mode: helix_view::document::Mode::Normal,
            new_mode: helix_view::document::Mode::Insert,
        };

        let result = core.process_v2_event(&event, &mut editor).await;
        assert!(result.is_ok());

        // Verify mode was updated in handler
        assert_eq!(
            core.editor_handler().get_current_mode(),
            helix_view::document::Mode::Insert
        );
    }

    #[tokio::test]
    async fn test_uninitialized_core_error() {
        let mut core = ApplicationCore::new();
        let mut editor = Editor::new(
            Rect::default(),
            Arc::new(helix_core::syntax::Loader::new(&[])),
            Arc::new(parking_lot::RwLock::new(
                helix_view::theme::Loader::new(&[]),
            )),
            Some(Box::new(|| {
                Arc::new(helix_view::handlers::Handlers::default())
            })),
        );

        let event = event_bridge::BridgedEvent::DocumentChanged {
            doc_id: DocumentId::default(),
        };

        let result = core.process_v2_event(&event, &mut editor).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not initialized"));
    }
}
