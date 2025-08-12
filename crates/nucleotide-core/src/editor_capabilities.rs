// ABOUTME: Extended editor capabilities for UI components
// ABOUTME: Provides fine-grained traits to replace Entity<Application> dependencies

use helix_core::Rope;
use helix_view::{Document, DocumentId, Editor, Theme, ViewId};
use std::path::Path;

/// Document access capabilities
pub trait DocumentAccess {
    /// Get a document by ID
    fn get_document(&self, id: DocumentId) -> Option<&Document>;

    /// Get mutable document by ID
    fn get_document_mut(&mut self, id: DocumentId) -> Option<&mut Document>;

    /// Get document content as rope
    fn get_document_rope(&self, id: DocumentId) -> Option<&Rope>;

    /// Get document path
    fn get_document_path(&self, id: DocumentId) -> Option<&Path>;

    /// Get document language name
    fn get_document_language(&self, id: DocumentId) -> Option<String>;
}

/// View management capabilities
pub trait ViewManagement {
    /// Get current view for a document
    fn get_view(&self, view_id: ViewId) -> Option<helix_view::View>;

    /// Get view's document
    fn get_view_document(&self, view_id: ViewId) -> Option<DocumentId>;

    /// Update view's viewport
    fn update_viewport(&mut self, view_id: ViewId, offset: usize);

    /// Get cursor position in view
    fn get_cursor_position(&self, view_id: ViewId) -> Option<(usize, usize)>;
}

/// Editor access capabilities
pub trait EditorAccess {
    /// Get the editor instance
    fn editor(&self) -> &Editor;

    /// Get mutable editor instance
    fn editor_mut(&mut self) -> &mut Editor;

    /// Get current theme (may return owned for interior mutability)
    fn theme(&self) -> Theme;

    /// Get current mode
    fn mode(&self) -> helix_view::document::Mode;
}

/// Command execution capabilities  
pub trait CommandExecution {
    /// Execute a typed command
    fn execute_command(&mut self, command: &str, args: &[String]) -> Result<(), String>;

    /// Execute a normal mode command
    fn execute_normal_command(&mut self, keys: &str) -> Result<(), String>;
}

/// Status and info capabilities
pub trait StatusInfo {
    /// Get status line text
    fn get_status_line(&self, doc_id: DocumentId, view_id: ViewId) -> String;

    /// Get current mode display string
    fn get_mode_string(&self) -> String;

    /// Get document diagnostics count
    fn get_diagnostics_count(&self, doc_id: DocumentId) -> (usize, usize, usize); // (errors, warnings, info)
}

/// Combined editor capabilities trait
pub trait EditorCapabilities:
    DocumentAccess + ViewManagement + EditorAccess + CommandExecution + StatusInfo
{
    /// Request a redraw
    fn request_redraw(&self);

    /// Check if there are unsaved changes
    fn has_unsaved_changes(&self) -> bool;

    /// Save all documents
    fn save_all(&mut self) -> Result<(), String>;
}

/// Weak reference capabilities for non-owning access
pub trait WeakEditorCapabilities {
    /// Try to upgrade to strong reference
    fn upgrade(&self) -> Option<Box<dyn EditorCapabilities>>;

    /// Check if the reference is still valid
    fn is_valid(&self) -> bool;
}
