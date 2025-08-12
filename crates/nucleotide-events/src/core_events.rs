// ABOUTME: Core editor events that don't depend on UI components
// ABOUTME: Foundation events for editor state changes

use helix_view::{DocumentId, ViewId};
use nucleotide_types::CompletionTrigger;

/// Core editor events that don't depend on UI components
#[derive(Debug, Clone)]
pub enum CoreEvent {
    /// Document was modified
    DocumentChanged { doc_id: DocumentId },

    /// Selection changed in a view
    SelectionChanged { doc_id: DocumentId, view_id: ViewId },

    /// Editor mode changed
    ModeChanged {
        old_mode: helix_view::document::Mode,
        new_mode: helix_view::document::Mode,
    },

    /// Diagnostics updated for a document
    DiagnosticsChanged { doc_id: DocumentId },

    /// Document opened
    DocumentOpened { doc_id: DocumentId },

    /// Document closed
    DocumentClosed { doc_id: DocumentId },

    /// View gained focus
    ViewFocused { view_id: ViewId },

    /// Editor needs redraw
    RedrawRequested,

    /// Status message to display
    StatusMessage {
        message: String,
        severity: MessageSeverity,
    },

    /// Document saved
    DocumentSaved {
        doc_id: DocumentId,
        path: Option<String>,
    },

    /// Command submitted
    CommandSubmitted { command: String },

    /// Search submitted
    SearchSubmitted { query: String },

    /// Should quit the application
    ShouldQuit,

    /// Status changed with message and severity
    StatusChanged {
        message: String,
        severity: MessageSeverity,
    },

    /// Completion requested
    CompletionRequested {
        doc_id: DocumentId,
        view_id: ViewId,
        trigger: CompletionTrigger,
    },
}

/// Message severity levels
#[derive(Debug, Clone, Copy)]
pub enum MessageSeverity {
    Info,
    Warning,
    Error,
}
