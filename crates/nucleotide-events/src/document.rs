// ABOUTME: Document domain events for lifecycle, content changes, and file operations
// ABOUTME: Immutable fact-based events following Domain-Driven Design principles

use helix_view::DocumentId;
use std::{ops::Range, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentLineChange {
    pub old_lines: Range<usize>,
    pub new_lines: Range<usize>,
}

/// Document domain events - covers document lifecycle, content changes, and save operations
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// Document content was modified
    ContentChanged {
        doc_id: DocumentId,
        revision: u64,
        change_summary: ChangeType,
        line_change: DocumentLineChange,
    },

    /// Document opened successfully
    Opened {
        doc_id: DocumentId,
        path: PathBuf,
        language_id: Option<String>,
    },

    /// Document closed
    Closed {
        doc_id: DocumentId,
        was_modified: bool,
    },

    /// Document saved to disk
    Saved {
        doc_id: DocumentId,
        path: PathBuf,
        revision: u64,
    },

    /// Document save failed
    SaveFailed {
        doc_id: DocumentId,
        path: PathBuf,
        error: String,
    },

    /// Document language detected/changed
    LanguageDetected {
        doc_id: DocumentId,
        language_id: String,
        previous_language: Option<String>,
    },

    /// Diagnostics updated for document
    DiagnosticsUpdated {
        doc_id: DocumentId,
        diagnostic_count: usize,
        error_count: usize,
        warning_count: usize,
    },
}

/// Type of content change that occurred
#[derive(Debug, Clone, Copy)]
pub enum ChangeType {
    Insert,
    Delete,
    Replace,
    Bulk, // Multiple changes in single transaction
}
