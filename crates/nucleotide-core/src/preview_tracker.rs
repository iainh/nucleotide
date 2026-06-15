// ABOUTME: Global tracker for preview documents to ensure proper cleanup
// ABOUTME: Prevents memory leaks from unclosed preview documents in pickers

use helix_view::{DocumentId, ViewId};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Tracks active preview documents to ensure they're properly cleaned up
#[derive(Clone, Debug)]
struct PreviewEntry {
    doc_id: DocumentId,
    view_id: ViewId,
    /// Whether this preview opened a new ephemeral document (not user-opened)
    ephemeral: bool,
}

#[derive(Clone, Default)]
pub struct PreviewTracker {
    active_previews: Arc<Mutex<Vec<PreviewEntry>>>,
}

impl PreviewTracker {
    pub fn new() -> Self {
        Self {
            active_previews: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register a new preview document (defaults to ephemeral = true)
    pub fn register(&self, doc_id: DocumentId, view_id: ViewId) {
        self.register_with_flag(doc_id, view_id, true);
    }

    /// Register a preview with an explicit ephemeral flag
    pub fn register_with_flag(&self, doc_id: DocumentId, view_id: ViewId, ephemeral: bool) {
        if let Ok(mut previews) = self.active_previews.lock() {
            previews.push(PreviewEntry {
                doc_id,
                view_id,
                ephemeral,
            });
        }
    }

    /// Unregister a preview document
    pub fn unregister(&self, doc_id: DocumentId, view_id: ViewId) {
        if let Ok(mut previews) = self.active_previews.lock() {
            previews.retain(|e| e.doc_id != doc_id || e.view_id != view_id);
        }
    }

    /// Unregister all preview entries for a document.
    pub fn unregister_doc(&self, doc_id: DocumentId) {
        if let Ok(mut previews) = self.active_previews.lock() {
            previews.retain(|e| e.doc_id != doc_id);
        }
    }

    /// Clear all preview markers without closing any documents.
    pub fn clear(&self) {
        if let Ok(mut previews) = self.active_previews.lock() {
            previews.clear();
        }
    }

    /// Get all active preview documents
    pub fn get_active(&self) -> Vec<(DocumentId, ViewId)> {
        if let Ok(previews) = self.active_previews.lock() {
            previews.iter().map(|e| (e.doc_id, e.view_id)).collect()
        } else {
            Vec::new()
        }
    }

    /// Get the set of doc_ids currently marked as preview tabs.
    pub fn preview_doc_ids(&self) -> HashSet<DocumentId> {
        if let Ok(previews) = self.active_previews.lock() {
            previews.iter().map(|e| e.doc_id).collect()
        } else {
            HashSet::new()
        }
    }

    /// Get the set of doc_ids for previews that are ephemeral (created by preview)
    pub fn ephemeral_doc_ids(&self) -> HashSet<DocumentId> {
        if let Ok(previews) = self.active_previews.lock() {
            previews
                .iter()
                .filter(|e| e.ephemeral)
                .map(|e| e.doc_id)
                .collect()
        } else {
            HashSet::new()
        }
    }

    /// Check if a given document is marked ephemeral in active previews
    pub fn is_ephemeral_doc(&self, doc_id: DocumentId) -> bool {
        if let Ok(previews) = self.active_previews.lock() {
            previews.iter().any(|e| e.doc_id == doc_id && e.ephemeral)
        } else {
            false
        }
    }

    /// Check if a given document is currently marked as a preview tab.
    pub fn is_preview_doc(&self, doc_id: DocumentId) -> bool {
        if let Ok(previews) = self.active_previews.lock() {
            previews.iter().any(|e| e.doc_id == doc_id)
        } else {
            false
        }
    }

    /// Replace all current preview markers with a single document.
    pub fn replace_with_doc(
        &self,
        doc_id: DocumentId,
        view_id: ViewId,
        ephemeral: bool,
    ) -> Vec<DocumentId> {
        if let Ok(mut previews) = self.active_previews.lock() {
            let mut replaced = Vec::new();
            for preview in previews.iter().filter(|e| e.doc_id != doc_id) {
                if !replaced.contains(&preview.doc_id) {
                    replaced.push(preview.doc_id);
                }
            }

            previews.clear();
            previews.push(PreviewEntry {
                doc_id,
                view_id,
                ephemeral,
            });

            replaced
        } else {
            Vec::new()
        }
    }

    /// Clean up all active preview documents
    pub fn cleanup_all(&self, editor: &mut helix_view::Editor) {
        if let Ok(mut previews) = self.active_previews.lock() {
            for PreviewEntry {
                doc_id, view_id, ..
            } in previews.drain(..)
            {
                // Close the view first if it exists
                if editor.tree.contains(view_id) {
                    editor.close(view_id);
                }
                // Then close the document without saving
                let _ = editor.close_document(doc_id, false);
            }
        }
    }
}

impl gpui::Global for PreviewTracker {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unregister_doc_removes_ephemeral_preview_entries() {
        let tracker = PreviewTracker::new();
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();

        tracker.register(doc_id, view_id);
        assert!(tracker.is_ephemeral_doc(doc_id));

        tracker.unregister_doc(doc_id);

        assert!(!tracker.is_ephemeral_doc(doc_id));
        assert!(tracker.get_active().is_empty());
    }

    #[test]
    fn unregister_doc_removes_all_entries_for_document() {
        let tracker = PreviewTracker::new();
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();

        tracker.register_with_flag(doc_id, view_id, true);
        tracker.register_with_flag(doc_id, view_id, false);

        tracker.unregister_doc(doc_id);

        assert!(tracker.get_active().is_empty());
    }

    #[test]
    fn clear_removes_preview_markers_without_requiring_editor_cleanup() {
        let tracker = PreviewTracker::new();
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();

        tracker.register(doc_id, view_id);
        assert!(tracker.is_preview_doc(doc_id));

        tracker.clear();

        assert!(!tracker.is_preview_doc(doc_id));
        assert!(tracker.get_active().is_empty());
    }

    #[test]
    fn preview_doc_ids_include_non_ephemeral_entries() {
        let tracker = PreviewTracker::new();
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();

        tracker.register_with_flag(doc_id, view_id, false);

        let preview_docs = tracker.preview_doc_ids();
        assert!(preview_docs.contains(&doc_id));
        assert!(tracker.is_preview_doc(doc_id));
        assert!(!tracker.is_ephemeral_doc(doc_id));
    }

    #[test]
    fn replace_with_doc_marks_document_with_requested_cleanup_policy() {
        let tracker = PreviewTracker::new();
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();

        tracker.register(doc_id, view_id);

        let replaced = tracker.replace_with_doc(doc_id, view_id, false);

        assert!(replaced.is_empty());
        assert!(tracker.is_preview_doc(doc_id));
        assert!(!tracker.is_ephemeral_doc(doc_id));
    }
}
