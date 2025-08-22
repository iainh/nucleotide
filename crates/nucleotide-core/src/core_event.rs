// ABOUTME: Core application events that bridge legacy patterns with V2 domain events
// ABOUTME: Provides temporary compatibility layer during migration to pure V2 events

use crate::CompletionTrigger;
use nucleotide_types::Severity;

/// Core application events - maps to V2 domain events
/// This is a temporary compatibility layer during the migration to V2 events
#[derive(Debug, Clone)]
pub enum CoreEvent {
    // UI events
    RedrawRequested,
    ShouldQuit,
    StatusChanged {
        message: String,
        severity: Severity,
    },
    CommandSubmitted {
        command: String,
    },
    SearchSubmitted {
        query: String,
    },

    // Document events
    DocumentChanged {
        doc_id: helix_view::DocumentId,
    },
    DocumentOpened {
        doc_id: helix_view::DocumentId,
    },
    DocumentClosed {
        doc_id: helix_view::DocumentId,
    },
    DocumentSaved {
        doc_id: helix_view::DocumentId,
        path: Option<String>,
    },

    // Editor/view events
    SelectionChanged {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    },
    ViewFocused {
        view_id: helix_view::ViewId,
    },
    ModeChanged {
        old_mode: helix_view::document::Mode,
        new_mode: helix_view::document::Mode,
    },
    DiagnosticsChanged {
        doc_id: helix_view::DocumentId,
    },
    CompletionRequested {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: CompletionTrigger,
    },
}
