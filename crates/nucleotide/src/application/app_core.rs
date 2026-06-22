// ABOUTME: Core application logic extracted from Application struct
// ABOUTME: Handles V2 event processing and central coordination logic

use nucleotide_events::v2::handler::EventHandler;
use nucleotide_logging::{debug, error, info, instrument, warn};

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

    /// Create a new application core with application handle for LSP integration
    pub fn with_app_handle(app_handle: gpui::WeakEntity<crate::Application>) -> Self {
        Self {
            document_handler: DocumentHandler::new(),
            view_handler: ViewHandler::new(),
            editor_handler: EditorHandler::new(),
            lsp_handler: LspHandler::new(),
            completion_handler: CompletionHandler::with_app_handle(app_handle),
            workspace_handler: WorkspaceHandler::new(),
            initialized: false,
        }
    }

    /// Set the application handle for LSP completion
    pub fn set_app_handle(&mut self, app_handle: gpui::WeakEntity<crate::Application>) {
        self.completion_handler.set_app_handle(app_handle);
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
        self.document_handler.initialize()?;
        self.view_handler.initialize()?;
        self.editor_handler.initialize()?;

        // Initialize Phase 2 handlers
        self.lsp_handler.initialize()?;
        self.completion_handler.initialize()?;
        self.workspace_handler.initialize()?;

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

        use nucleotide_events::v2::document::Event as DocumentEvent;

        // Process V2 events for all supported event types
        match bridged_event {
            event_bridge::BridgedEvent::DocumentChanged {
                doc_id,
                change_summary,
            } => {
                // Extract actual document revision
                let revision = if let Some(document) = editor.document_mut(*doc_id) {
                    document.get_current_revision() as u64
                } else {
                    warn!(doc_id = ?doc_id, "Document not found when processing DocumentChanged event");
                    0
                };

                // Create a V2 document event with actual change type
                let v2_event = DocumentEvent::ContentChanged {
                    doc_id: *doc_id,
                    revision,
                    change_summary: *change_summary,
                };

                debug!(
                    doc_id = ?doc_id,
                    revision = revision,
                    "Processing DocumentChanged through V2 handler"
                );

                self.document_handler.handle(v2_event).await?;
            }
            event_bridge::BridgedEvent::FilePickerRequested => {
                // Forward to main application loop with GPUI context
                // The main loop in application/mod.rs handles Update::ShowFilePicker
            }
            event_bridge::BridgedEvent::BufferPickerRequested => {
                // Forward to main application loop with GPUI context
                // The main loop in application/mod.rs handles Update::ShowBufferPicker
            }
            event_bridge::BridgedEvent::DiagnosticsPickerRequested { .. } => {
                // Handled in Application main loop with GPUI context
            }

            event_bridge::BridgedEvent::SelectionChanged { doc_id, view_id } => {
                if let Some(v2_event) = Self::build_v2_selection_changed(editor, *doc_id, *view_id)
                {
                    debug!(
                        doc_id = ?doc_id,
                        view_id = ?view_id,
                        "Processing SelectionChanged through V2 ViewHandler"
                    );
                    self.view_handler.handle(v2_event).await?;
                } else {
                    warn!(view_id = ?view_id, "Ignoring selection event for unknown view");
                }
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

                self.editor_handler.handle(v2_event).await?;
            }

            event_bridge::BridgedEvent::DocumentOpened { doc_id } => {
                // Extract document information for enriched event
                let (path, language_id) = if let Some(document) = editor.document(*doc_id) {
                    let path = document
                        .path()
                        .cloned()
                        .unwrap_or_else(|| std::path::PathBuf::from("untitled"));
                    let language_id = document.language_name().map(|lang| lang.to_string());
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

                self.document_handler.handle(v2_event).await?;
            }

            event_bridge::BridgedEvent::DocumentClosed {
                doc_id,
                was_modified,
            } => {
                // Use the actual modification state from the Helix event
                let v2_event = DocumentEvent::Closed {
                    doc_id: *doc_id,
                    was_modified: *was_modified,
                };

                debug!(doc_id = ?doc_id, "Processing DocumentClosed through V2 DocumentHandler");

                self.document_handler.handle(v2_event).await?;
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
                        .filter(|d| {
                            matches!(d.severity, Some(helix_core::diagnostic::Severity::Error))
                        })
                        .count();
                    let warnings = diagnostics
                        .iter()
                        .filter(|d| {
                            matches!(d.severity, Some(helix_core::diagnostic::Severity::Warning))
                        })
                        .count();
                    (total, errors, warnings)
                } else {
                    (0, 0, 0)
                };

                let v2_event = DocumentEvent::DiagnosticsUpdated {
                    doc_id: *doc_id,
                    diagnostic_count,
                    error_count,
                    warning_count,
                };

                debug!(
                    doc_id = ?doc_id,
                    diagnostic_count = diagnostic_count,
                    error_count = error_count,
                    warning_count = warning_count,
                    "DIAG: Processing DiagnosticsChanged through V2 DocumentHandler"
                );

                self.document_handler.handle(v2_event).await?;
            }

            event_bridge::BridgedEvent::ViewFocused { view_id } => {
                // Extract associated document ID from the view
                if let Some(view) = editor.tree.try_get(*view_id) {
                    let doc_id = view.doc;
                    let previous_view = self.view_handler.get_focused_view();

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

                    self.view_handler.handle(v2_event).await?;
                } else {
                    warn!(view_id = ?view_id, "Ignoring focus event for unknown view");
                }
            }

            // Phase 2 Events - LSP Integration
            event_bridge::BridgedEvent::LanguageServerInitialized { server_id } => {
                // Extract actual server information from the language server registry
                let (server_name, workspace_root) =
                    if let Some(client) = editor.language_servers.get_by_id(*server_id) {
                        let name = client.name().to_string();
                        // Since root_path is private, we'll use the current working directory as approximation
                        let root = std::env::current_dir()
                            .unwrap_or_else(|_| std::path::PathBuf::from("."));
                        (name, root)
                    } else {
                        warn!(server_id = ?server_id, "Language server not found in registry");
                        ("unknown".to_string(), std::path::PathBuf::from("unknown"))
                    };

                // Create LSP server initialized event with actual data
                let v2_event = nucleotide_events::v2::lsp::Event::ServerInitialized {
                    server_id: *server_id,
                    server_name: server_name.clone(),
                    capabilities: nucleotide_events::v2::lsp::ServerCapabilities::new(),
                    workspace_root: workspace_root.clone(),
                };

                debug!(
                    server_id = ?server_id,
                    "Processing LanguageServerInitialized through V2 LspHandler"
                );

                self.lsp_handler.handle(v2_event).await?;
            }

            event_bridge::BridgedEvent::LanguageServerExited { server_id } => {
                // Try to extract server information before the client is removed from registry
                let (server_name, workspace_root) =
                    if let Some(client) = editor.language_servers.get_by_id(*server_id) {
                        let name = client.name().to_string();
                        let root = std::env::current_dir()
                            .unwrap_or_else(|_| std::path::PathBuf::from("."));
                        (name, root)
                    } else {
                        // Server may have already been removed, use defaults
                        ("unknown".to_string(), std::path::PathBuf::from("unknown"))
                    };

                let v2_event = nucleotide_events::v2::lsp::Event::ServerExited {
                    server_id: *server_id,
                    server_name: server_name.clone(),
                    exit_code: None, // Exit code is not available through Helix events
                    workspace_root: workspace_root.clone(),
                };

                debug!(
                    server_id = ?server_id,
                    "Processing LanguageServerExited through V2 LspHandler"
                );

                self.lsp_handler.handle(v2_event).await?;
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

                self.lsp_handler.handle(v2_event).await?;

                // Note: Actual LSP server startup will be handled by sync processor
                // when the main application processes this same bridged event
            }

            // Phase 2 Events - Completion Integration
            event_bridge::BridgedEvent::CompletionRequested {
                doc_id,
                view_id,
                trigger,
            } => {
                // Generate a unique request ID
                let request_id = self.completion_handler.next_request_id().await;

                let cursor_position = Self::completion_cursor_position(editor, *doc_id, *view_id)
                    .unwrap_or_else(|| {
                        warn!(
                            doc_id = ?doc_id,
                            view_id = ?view_id,
                            "Completion requested for unknown view or document; using origin cursor"
                        );
                        nucleotide_events::v2::completion::Position::new(0, 0)
                    });

                let v2_event = nucleotide_events::v2::completion::Event::Requested {
                    doc_id: *doc_id,
                    view_id: *view_id,
                    trigger: match trigger {
                        nucleotide_types::CompletionTrigger::Manual => {
                            nucleotide_events::v2::completion::CompletionTrigger::Manual
                        }
                        nucleotide_types::CompletionTrigger::Character(ch) => {
                            nucleotide_events::v2::completion::CompletionTrigger::Character(*ch)
                        }
                        nucleotide_types::CompletionTrigger::Automatic => {
                            nucleotide_events::v2::completion::CompletionTrigger::Automatic
                        }
                    },
                    cursor_position,
                    request_id,
                };

                debug!(
                    doc_id = ?doc_id,
                    view_id = ?view_id,
                    request_id = ?request_id,
                    "Processing CompletionRequested through V2 CompletionHandler"
                );

                self.completion_handler.handle(v2_event).await?;
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

impl ApplicationCore {
    /// Build a V2 selection changed event from the current editor state.
    fn build_v2_selection_changed(
        editor: &helix_view::Editor,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    ) -> Option<nucleotide_events::v2::view::Event> {
        let view = editor.tree.try_get(view_id)?;
        let selection = if let Some(doc) = editor.document(view.doc) {
            doc.selection(view.id).clone()
        } else {
            helix_core::Selection::point(0)
        };

        let v2_selection = nucleotide_events::view::Selection {
            ranges: selection
                .ranges()
                .iter()
                .map(|range| nucleotide_events::view::SelectionRange {
                    anchor: nucleotide_events::view::Position::new(range.anchor, range.anchor),
                    head: nucleotide_events::view::Position::new(range.head, range.head),
                })
                .collect(),
            primary_index: selection.primary_index(),
        };

        Some(nucleotide_events::v2::view::Event::SelectionChanged {
            view_id,
            doc_id,
            selection: v2_selection,
            was_movement: true,
        })
    }

    fn completion_cursor_position(
        editor: &helix_view::Editor,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    ) -> Option<nucleotide_events::v2::completion::Position> {
        let view = editor.tree.try_get(view_id)?;
        if view.doc != doc_id {
            return None;
        }

        let doc = editor.document(doc_id)?;
        let text = doc.text().slice(..);
        let cursor = doc.selection(view.id).primary().cursor(text);

        Some(Self::completion_position_from_cursor(text, cursor))
    }

    fn completion_position_from_cursor(
        text: helix_core::RopeSlice<'_>,
        cursor: usize,
    ) -> nucleotide_events::v2::completion::Position {
        let coords = helix_core::coords_at_pos(text, cursor);
        nucleotide_events::v2::completion::Position::new(coords.row, coords.col)
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
    use std::sync::Arc;

    use arc_swap::{ArcSwap, access::Map};
    use helix_core::syntax;
    use helix_view::{
        DocumentId, Editor, document::Mode, editor::Config, graphics::Rect, handlers::Handlers,
        theme,
    };
    use nucleotide_core::event_bridge::BridgedEvent;
    use nucleotide_events::v2::document::ChangeType;

    fn test_handlers() -> Handlers {
        let (completion_tx, _) = tokio::sync::mpsc::channel(1);
        let (signature_tx, _) = tokio::sync::mpsc::channel(1);
        let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);

        Handlers {
            completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            word_index: helix_view::handlers::word_index::Handler::spawn(),
        }
    }

    fn test_editor() -> Editor {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));

        Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| config)),
            test_handlers(),
        )
    }

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
        let mut editor = test_editor();
        let doc_id = DocumentId::default();

        core.process_v2_event(
            &BridgedEvent::DocumentChanged {
                doc_id,
                change_summary: ChangeType::Replace,
            },
            &mut editor,
        )
        .await
        .unwrap();

        let metadata = core.document_handler().get_metadata(&doc_id).unwrap();
        assert_eq!(metadata.revision, 0);
        assert!(metadata.is_modified);
    }

    #[tokio::test]
    async fn test_mode_changed_event_processing() {
        let mut core = ApplicationCore::new();
        core.initialize().unwrap();
        let mut editor = test_editor();

        core.process_v2_event(
            &BridgedEvent::ModeChanged {
                old_mode: Mode::Normal,
                new_mode: Mode::Insert,
            },
            &mut editor,
        )
        .await
        .unwrap();

        assert_eq!(core.editor_handler().get_current_mode(), Mode::Insert);
    }

    #[tokio::test]
    async fn test_uninitialized_core_error() {
        let mut core = ApplicationCore::new();
        let mut editor = test_editor();

        let result = core
            .process_v2_event(
                &BridgedEvent::ModeChanged {
                    old_mode: Mode::Normal,
                    new_mode: Mode::Insert,
                },
                &mut editor,
            )
            .await;

        assert!(result.is_err());
        assert!(!core.is_initialized());
    }

    #[test]
    fn test_completion_position_from_cursor_uses_line_and_column() {
        let text = helix_core::Rope::from("alpha\nbeta\ngamma");
        let cursor = text.line_to_char(1) + 2;

        let position = ApplicationCore::completion_position_from_cursor(text.slice(..), cursor);

        assert_eq!(
            position,
            nucleotide_events::v2::completion::Position::new(1, 2)
        );
    }
}
