// ABOUTME: Core application state extracted from Application struct
// ABOUTME: Owns V2 event handlers and shared coordination state

use nucleotide_logging::{info, instrument, warn};

use crate::application::{DocumentHandler, EditorHandler, ViewHandler};

/// Core application logic for event processing and coordination
/// Separated from the main Application struct to reduce complexity
pub struct ApplicationCore {
    /// V2 Event System Handlers - Phase 1
    pub document_handler: DocumentHandler,
    pub view_handler: ViewHandler,
    pub editor_handler: EditorHandler,

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
        self.document_handler.initialize()?;
        self.view_handler.initialize()?;
        self.editor_handler.initialize()?;

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
