// ABOUTME: Legacy core events - DEPRECATED in favor of V2 event system
// ABOUTME: This file contains stubs for backward compatibility during Phase 3.1 migration

use helix_view::{DocumentId, ViewId};
use nucleotide_types::CompletionTrigger;

/// Legacy core editor events - DEPRECATED
/// Use nucleotide_events::v2::document::Event and other V2 events instead
#[derive(Debug, Clone)]
#[deprecated(note = "Use V2 event system: nucleotide_events::v2::document::Event, etc.")]
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

    /// Completion accepted - insert the selected completion text
    CompletionAccepted {
        text: String,
        doc_id: Option<DocumentId>,
        view_id: Option<ViewId>,
    },
}

/// Message severity levels
#[derive(Debug, Clone, Copy)]
pub enum MessageSeverity {
    Info,
    Warning,
    Error,
}
