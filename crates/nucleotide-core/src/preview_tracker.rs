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

    /// Get all active preview documents
    pub fn get_active(&self) -> Vec<(DocumentId, ViewId)> {
        if let Ok(previews) = self.active_previews.lock() {
            previews.iter().map(|e| (e.doc_id, e.view_id)).collect()
        } else {
            Vec::new()
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
