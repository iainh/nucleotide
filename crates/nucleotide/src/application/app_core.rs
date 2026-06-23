// ABOUTME: Core application state extracted from Application struct
// ABOUTME: Owns V2 event handlers and shared coordination state

use nucleotide_logging::{info, instrument, warn};

use crate::application::{
    CompletionHandler, DocumentHandler, EditorHandler, LspHandler, ViewHandler, WorkspaceHandler,
};

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

    #[tokio::test]
    async fn test_application_core_initialization() {
        let mut core = ApplicationCore::new();
        assert!(!core.is_initialized());

        core.initialize().unwrap();
        assert!(core.is_initialized());
    }
}
