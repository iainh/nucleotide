// ABOUTME: Capability traits that define abstract interfaces for core editor functionality
// ABOUTME: These traits enable dependency inversion to break circular dependencies

use helix_core::Rope;
use helix_view::{Document, DocumentId, Editor, Theme, View, ViewId};
use std::path::Path;

/// Buffer/Document management capabilities
pub trait BufferStore {
    /// Open a document from a path
    fn open_document(&self, path: &Path) -> Result<DocumentId, String>;

    /// Get document content by ID
    fn get_document(&self, id: DocumentId) -> Option<&Rope>;

    /// Get mutable document content
    fn get_document_mut(&mut self, id: DocumentId) -> Option<&mut Rope>;

    /// Close a document
    fn close_document(&mut self, id: DocumentId) -> Result<(), String>;

    /// List all open documents
    fn list_documents(&self) -> Vec<DocumentId>;
}

/// View management capabilities
pub trait ViewStore {
    /// Create a new view for a document
    fn create_view(&mut self, doc_id: DocumentId) -> ViewId;

    /// Get the current focused view
    fn focused_view(&self) -> Option<ViewId>;

    /// Set the focused view
    fn focus_view(&mut self, view_id: ViewId);

    /// Close a view
    fn close_view(&mut self, view_id: ViewId);

    /// Get document ID for a view
    fn view_document(&self, view_id: ViewId) -> Option<DocumentId>;
}

/// Theme and styling capabilities
pub trait ThemeProvider {
    /// Get the current theme (may return owned for interior mutability)
    fn current_theme(&self) -> Theme;

    /// Set the theme
    fn set_theme(&mut self, theme: Theme);

    /// List available themes
    fn available_themes(&self) -> Vec<String>;
}

/// Command execution capabilities
pub trait CommandExecutor {
    /// Execute a command by name with arguments
    fn execute_command(&self, name: &str, args: Vec<String>) -> Result<(), String>;

    /// Check if a command exists
    fn has_command(&self, name: &str) -> bool;

    /// List available commands
    fn list_commands(&self) -> Vec<String>;
}

/// Editor state capabilities
pub trait EditorState: BufferStore + ViewStore + ThemeProvider + CommandExecutor {
    /// Get the current mode as a string
    fn current_mode(&self) -> String;

    /// Check if editor has unsaved changes
    fn has_unsaved_changes(&self) -> bool;

    /// Save all documents
    fn save_all(&mut self) -> Result<(), String>;
}

/// Event emission capabilities
pub trait EventEmitter {
    type Event;

    /// Emit an event
    fn emit(&self, event: Self::Event);
}

/// Event subscription capabilities
pub trait EventSubscriber {
    type Event;

    /// Subscribe to events with a callback
    fn subscribe<F>(&mut self, callback: F) -> SubscriptionId
    where
        F: FnMut(&Self::Event) + 'static;

    /// Unsubscribe from events
    fn unsubscribe(&mut self, id: SubscriptionId);
}

/// Subscription identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

/// Capability provider aggregates all capabilities
pub trait CapabilityProvider: EditorState + Send + Sync {}

/// Direct read-only access to the Helix editor
/// This is a more pragmatic trait for the transition period
pub trait EditorReadAccess {
    /// Get a reference to the Helix editor
    fn editor(&self) -> &Editor;

    /// Get a specific document by ID
    fn document(&self, doc_id: DocumentId) -> Option<&Document> {
        self.editor().document(doc_id)
    }

    /// Get a specific view by ID
    fn view(&self, view_id: ViewId) -> &View {
        self.editor().tree.get(view_id)
    }
}

/// Direct mutable access to the Helix editor
pub trait EditorWriteAccess: EditorReadAccess {
    /// Get a mutable reference to the Helix editor
    fn editor_mut(&mut self) -> &mut Editor;

    /// Get a mutable document by ID
    fn document_mut(&mut self, doc_id: DocumentId) -> Option<&mut Document> {
        self.editor_mut().document_mut(doc_id)
    }
}

/// Job system access
pub trait JobSystemAccess {
    /// Get mutable access to the job system
    fn jobs_mut(&mut self) -> &mut helix_term::job::Jobs;
}

/// Scroll management for views
pub trait ScrollManager {
    /// Get scroll offset for a view
    fn get_scroll_offset(&self, view_id: ViewId) -> (usize, usize);

    /// Set scroll offset for a view
    fn set_scroll_offset(&mut self, view_id: ViewId, offset: (usize, usize));

    /// Scroll by a number of lines
    fn scroll_lines(&mut self, view_id: ViewId, lines: isize);
}

/// Line caching for rendering optimization
pub trait LineCache {
    /// Get cached line layout information
    fn get_line_layout(&self, doc_id: DocumentId, line: usize) -> Option<LineLayoutInfo>;

    /// Cache line layout information
    fn cache_line_layout(&mut self, doc_id: DocumentId, line: usize, layout: LineLayoutInfo);

    /// Invalidate cache for a document
    fn invalidate_document(&mut self, doc_id: DocumentId);
}

/// Line layout information
#[derive(Clone, Debug)]
pub struct LineLayoutInfo {
    pub text: String,
    pub width: f32,
    pub height: f32,
    pub graphemes: Vec<(usize, f32)>, // (byte_offset, x_position)
}

/// Overlay interaction capabilities
pub trait OverlayProvider {
    /// Get helix theme for styling overlays
    fn get_helix_theme(&self) -> helix_view::Theme;
}

/// Optional capabilities used by UI pickers to preview files
/// Implementations can choose how to open and close lightweight preview documents/views.
pub trait PickerCapability {
    /// Open a preview for the given path, returning the associated document and view IDs.
    /// Accepts any GPUI context so implementations can update entities.
    fn open_preview<C: gpui::AppContext>(
        &mut self,
        path: &Path,
        cx: &mut C,
    ) -> Result<(helix_view::DocumentId, helix_view::ViewId), String>;

    /// Close a previously opened preview identified by document and view IDs.
    /// Accepts any GPUI context so implementations can update entities.
    fn close_preview<C: gpui::AppContext>(
        &mut self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        cx: &mut C,
    ) -> Result<(), String>;
}
