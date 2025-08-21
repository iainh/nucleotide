// ABOUTME: View management logic extracted from Workspace
// ABOUTME: Handles document view creation, focus management, and view state coordination

use crate::document::DocumentView;
use crate::workspace::Workspace;
use gpui::{AppContext, Context, Entity, Focusable, SharedString, Window};
use helix_view::ViewId;
use nucleotide_logging::{debug, info, instrument, warn};
use std::collections::{HashMap, HashSet};

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
            view.update(cx, |view, _cx| {
                view.set_focused(is_focused);
            });
        }
    }

    /// Update document views based on editor state
    #[instrument(skip(self, cx))]
    pub fn update_document_views(&mut self, cx: &mut Context<Workspace>) -> Option<SharedString> {
        let mut view_ids = HashSet::new();
        let mut focused_file_name = None;

        // Read editor state to get current views and collect view information
        let workspace = cx.entity();
        let core = workspace.read(cx).core.clone();

        // Collect all the data we need in one go to avoid borrowing conflicts
        // Clone the necessary data to avoid lifetime issues
        let (view_data, right_borders_set, focused_file_name_result, focused_view_id_result) = {
            let core_read = core.read(cx);
            let editor = &core_read.editor;

            // Clone view data to avoid borrowing issues
            let view_data: Vec<(helix_view::View, bool)> = editor
                .tree
                .views()
                .map(|(view, is_focused)| (view.clone(), is_focused))
                .collect();

            let mut right_borders = HashSet::new();
            let mut focused_file_name = None;
            let mut focused_view_id = None;

            for (view, is_focused) in &view_data {
                let view_id = view.id;

                // Check if this view has a right border (part of split layout)
                if editor
                    .tree
                    .find_split_in_direction(view_id, helix_view::tree::Direction::Right)
                    .is_some()
                {
                    right_borders.insert(view_id);
                }

                // Get filename for focused view
                if *is_focused {
                    if let Some(doc) = editor.document(view.doc) {
                        focused_file_name = doc.path().and_then(|p| {
                            p.file_name()
                                .and_then(|name| name.to_str())
                                .map(|s| SharedString::from(s.to_string()))
                        });
                    }
                    focused_view_id = Some(view_id);
                }
            }

            (view_data, right_borders, focused_file_name, focused_view_id)
        };
        // Important: core_read is dropped here

        // Update the focused file name and view ID
        focused_file_name = focused_file_name_result;
        if let Some(fv_id) = focused_view_id_result {
            self.focused_view_id = Some(fv_id);
        }

        // Track which views have right borders (split layout detection)
        let right_borders = right_borders_set;

        // Update or create document views for all active views
        for (view, is_focused) in view_data {
            let view_id = view.id;
            view_ids.insert(view_id);

            // Update existing view or create new one
            if let Some(view_entity) = self.documents.get(&view_id) {
                view_entity.update(cx, |view, _cx| {
                    view.set_focused(is_focused);
                    // TODO: Update text style when needed
                });
            } else {
                // Create new view if it doesn't exist
                let view_entity = cx.new(|cx| {
                    let doc_focus_handle = cx.focus_handle();
                    // Create default text style
                    let default_style = gpui::TextStyle {
                        color: gpui::white(),
                        font_family: gpui::SharedString::from("Monaco"),
                        font_fallbacks: Default::default(),
                        font_features: Default::default(),
                        font_size: gpui::AbsoluteLength::Pixels(gpui::px(14.0)),
                        font_weight: Default::default(),
                        font_style: Default::default(),
                        line_height: Default::default(),
                        line_clamp: Default::default(),
                        background_color: None,
                        underline: Default::default(),
                        strikethrough: Default::default(),
                        white_space: Default::default(),
                        text_align: Default::default(),
                        text_overflow: Default::default(),
                    };
                    DocumentView::new(
                        core.clone(),
                        view_id,
                        default_style,
                        &doc_focus_handle,
                        is_focused,
                    )
                });
                self.documents.insert(view_id, view_entity);
            }
        }

        // Remove views that no longer exist in the editor
        self.documents
            .retain(|view_id, _| view_ids.contains(view_id));

        focused_file_name
    }

    /// Update only the currently focused document view
    #[instrument(skip(self, cx))]
    pub fn update_current_document_view(&mut self, cx: &mut Context<Workspace>) {
        if let Some(focused_view_id) = self.focused_view_id {
            if let Some(view_entity) = self.documents.get(&focused_view_id) {
                view_entity.update(cx, |_view, cx| {
                    cx.notify();
                });
            }
        }
    }

    /// Focus the editor area by focusing the active document view
    #[instrument(skip(self, cx, window))]
    pub fn focus_editor_area(&mut self, cx: &mut Context<Workspace>, window: &mut Window) {
        debug!("Focusing editor area");

        // Find the currently active document view and focus it
        if let Some(view_id) = self.focused_view_id {
            if let Some(doc_view) = self.documents.get(&view_id) {
                let doc_focus = doc_view.focus_handle(cx);
                window.focus(&doc_focus);
                debug!(view_id = ?view_id, "Focused active document view");
                return;
            }
        }

        // If no focused view, try to focus the first available view
        if let Some((view_id, doc_view)) = self.documents.iter().next() {
            let doc_focus = doc_view.focus_handle(cx);
            window.focus(&doc_focus);
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
