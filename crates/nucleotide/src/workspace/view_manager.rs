// ABOUTME: View management logic extracted from Workspace
// ABOUTME: Handles document view creation, focus management, and view state coordination

use crate::document::DocumentView;
use crate::workspace::Workspace;
use gpui::{Context, Entity, Focusable, Window};
use helix_view::ViewId;
use nucleotide_logging::{debug, info, instrument};
use std::collections::HashMap;

/// Manages document views, focus state, and view coordination
/// Extracted from Workspace to reduce complexity and improve modularity
pub struct ViewManager {
    /// Map of view IDs to document views
    documents: HashMap<ViewId, Entity<DocumentView>>,

    /// Currently focused view ID
    focused_view_id: Option<ViewId>,

    /// Whether focus needs to be restored after operations
    needs_focus_restore: bool,
}

impl ViewManager {
    /// Create a new view manager
    pub fn new() -> Self {
        Self {
            documents: HashMap::new(),
            focused_view_id: None,
            needs_focus_restore: false,
        }
    }

    /// Get the currently focused view ID
    pub fn focused_view_id(&self) -> Option<ViewId> {
        self.focused_view_id
    }

    /// Get a document view by view ID
    pub fn get_document_view(&self, view_id: &ViewId) -> Option<&Entity<DocumentView>> {
        self.documents.get(view_id)
    }

    /// Get all document views
    pub fn document_views(&self) -> &HashMap<ViewId, Entity<DocumentView>> {
        &self.documents
    }

    /// Set the focused view ID
    pub fn set_focused_view_id(&mut self, view_id: Option<ViewId>) {
        self.focused_view_id = view_id;
    }

    /// Check if focus needs to be restored
    pub fn needs_focus_restore(&self) -> bool {
        self.needs_focus_restore
    }

    /// Set whether focus needs to be restored
    pub fn set_needs_focus_restore(&mut self, needs_restore: bool) {
        self.needs_focus_restore = needs_restore;
    }

    /// Insert a document view
    pub fn insert_document_view(&mut self, view_id: ViewId, view: Entity<DocumentView>) {
        self.documents.insert(view_id, view);
    }

    /// Remove a document view
    pub fn remove_document_view(&mut self, view_id: &ViewId) -> Option<Entity<DocumentView>> {
        self.documents.remove(view_id)
    }

    /// Handle view focus change
    #[instrument(skip(self, cx))]
    pub fn handle_view_focused(&mut self, view_id: ViewId, cx: &mut Context<Workspace>) {
        info!(view_id = ?view_id, "View focused");
        self.focused_view_id = Some(view_id);

        // Update focus state in document views
        for (id, view) in &self.documents {
            let is_focused = *id == view_id;
            view.update(cx, |view, cx| {
                if view.set_focused(is_focused) {
                    cx.notify();
                }
            });
        }
    }

    /// Focus the editor area by focusing the active document view
    #[instrument(skip(self, cx, window))]
    pub fn focus_editor_area(&mut self, cx: &mut Context<Workspace>, window: &mut Window) {
        debug!("Focusing editor area");

        // Find the currently active document view and focus it
        if let Some(view_id) = self.focused_view_id
            && let Some(doc_view) = self.documents.get(&view_id)
        {
            let doc_focus = doc_view.focus_handle(cx);
            if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
                coord.set_editor_focus(doc_focus.clone());
            }
            window.focus(&doc_focus, cx);
            debug!(view_id = ?view_id, "Focused active document view");
            return;
        }

        // If no focused view, try to focus the first available view
        if let Some((view_id, doc_view)) = self.documents.iter().next() {
            let doc_focus = doc_view.focus_handle(cx);
            if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
                coord.set_editor_focus(doc_focus.clone());
            }
            window.focus(&doc_focus, cx);
            self.focused_view_id = Some(*view_id);
            debug!(view_id = ?view_id, "Focused first available document view");
        }
    }

    /// Check if any document view is focused
    #[instrument(skip(self, cx, window))]
    pub fn is_document_view_focused(&self, cx: &Context<Workspace>, window: &Window) -> bool {
        self.focused_view_id
            .and_then(|view_id| self.documents.get(&view_id))
            .map(|doc_view| doc_view.focus_handle(cx).is_focused(window))
            .unwrap_or(false)
    }

    /// Get the focused document view entity
    pub fn get_focused_document_view(&self) -> Option<&Entity<DocumentView>> {
        self.focused_view_id
            .and_then(|view_id| self.documents.get(&view_id))
    }

    /// Clear all document views (useful for cleanup)
    pub fn clear_views(&mut self) {
        self.documents.clear();
        self.focused_view_id = None;
        self.needs_focus_restore = false;
    }

    /// Get the number of active document views
    pub fn view_count(&self) -> usize {
        self.documents.len()
    }

    /// Check if a specific view exists
    pub fn has_view(&self, view_id: &ViewId) -> bool {
        self.documents.contains_key(view_id)
    }
}

impl Default for ViewManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_manager_creation() {
        let manager = ViewManager::new();
        assert!(manager.focused_view_id().is_none());
        assert_eq!(manager.view_count(), 0);
        assert!(!manager.needs_focus_restore());
    }

    #[test]
    fn test_view_manager_focus_tracking() {
        let mut manager = ViewManager::new();
        let view_id = ViewId::default();

        // Initially no focused view
        assert!(manager.focused_view_id().is_none());

        // Can't test actual focus handling without GPUI context
        // but we can test the basic state management
        assert!(!manager.has_view(&view_id));

        manager.set_needs_focus_restore(true);
        assert!(manager.needs_focus_restore());
    }

    #[test]
    fn test_view_manager_clear() {
        let mut manager = ViewManager::new();
        manager.set_needs_focus_restore(true);

        manager.clear_views();
        assert!(manager.focused_view_id().is_none());
        assert_eq!(manager.view_count(), 0);
        assert!(!manager.needs_focus_restore());
    }
}
