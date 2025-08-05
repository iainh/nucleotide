// ABOUTME: Global tracker for preview documents to ensure proper cleanup
// ABOUTME: Prevents memory leaks from unclosed preview documents in pickers

use helix_view::{DocumentId, ViewId};
use std::sync::{Arc, Mutex};

/// Tracks active preview documents to ensure they're properly cleaned up
#[derive(Clone, Default)]
pub struct PreviewTracker {
    active_previews: Arc<Mutex<Vec<(DocumentId, ViewId)>>>,
}

impl PreviewTracker {
    pub fn new() -> Self {
        Self {
            active_previews: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    /// Register a new preview document
    pub fn register(&self, doc_id: DocumentId, view_id: ViewId) {
        if let Ok(mut previews) = self.active_previews.lock() {
            previews.push((doc_id, view_id));
        }
    }
    
    /// Unregister a preview document
    pub fn unregister(&self, doc_id: DocumentId, view_id: ViewId) {
        if let Ok(mut previews) = self.active_previews.lock() {
            previews.retain(|&(d, v)| d != doc_id || v != view_id);
        }
    }
    
    /// Get all active preview documents
    pub fn get_active(&self) -> Vec<(DocumentId, ViewId)> {
        if let Ok(previews) = self.active_previews.lock() {
            previews.clone()
        } else {
            Vec::new()
        }
    }
    
    /// Clean up all active preview documents
    pub fn cleanup_all(&self, editor: &mut helix_view::Editor) {
        if let Ok(mut previews) = self.active_previews.lock() {
            for (doc_id, view_id) in previews.drain(..) {
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