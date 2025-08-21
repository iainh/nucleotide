// ABOUTME: Document domain events for lifecycle, content changes, and file operations
// ABOUTME: Immutable fact-based events following Domain-Driven Design principles

use helix_view::DocumentId;
use std::path::PathBuf;

/// Document domain events - covers document lifecycle, content changes, and save operations
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// Document content was modified
    ContentChanged {
        doc_id: DocumentId,
        revision: u64,
        change_summary: ChangeType,
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

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::DocumentId;

    #[test]
    fn test_document_event_creation() {
        let doc_id = DocumentId::default();
        let event = Event::Opened {
            doc_id,
            path: PathBuf::from("/test/file.rs"),
            language_id: Some("rust".to_string()),
        };

        match event {
            Event::Opened {
                doc_id: _,
                path,
                language_id,
            } => {
                assert_eq!(path, PathBuf::from("/test/file.rs"));
                assert_eq!(language_id, Some("rust".to_string()));
            }
            _ => panic!("Expected Opened event"),
        }
    }

    #[test]
    fn test_change_type_variants() {
        let change_types = [
            ChangeType::Insert,
            ChangeType::Delete,
            ChangeType::Replace,
            ChangeType::Bulk,
        ];

        // All variants should be valid
        for change_type in change_types {
            let _event = Event::ContentChanged {
                doc_id: DocumentId::default(),
                revision: 1,
                change_summary: change_type,
            };
        }
    }
}
