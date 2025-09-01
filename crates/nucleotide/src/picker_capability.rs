use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use gpui::{Entity, ParentElement};
use helix_view::{DocumentId, ViewId};
use std::collections::HashMap;

use crate::Core;

/// Concrete implementation that uses the application's Helix editor
pub struct HelixPickerCapability {
    core: gpui::WeakEntity<Core>,
    previews: HashMap<(DocumentId, ViewId), Entity<crate::document::DocumentView>>,
}

impl HelixPickerCapability {
    pub fn new(core: &Entity<Core>) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            core: core.downgrade(),
            previews: HashMap::new(),
        }))
    }
}

impl nucleotide_core::capabilities::PickerCapability for HelixPickerCapability {
    fn render_preview(&self, doc_id: DocumentId, view_id: ViewId) -> gpui::AnyElement {
        // For now, render a simple text preview container. A richer integration
        // could embed a DocumentView in the future.
        use gpui::prelude::FluentBuilder;
        use gpui::{IntoElement, ParentElement as _, Styled, div};

        if let Some(entity) = self.previews.get(&(doc_id, view_id)).cloned() {
            // Wrap the DocumentView entity in a container to ensure sizing
            div().w_full().h_full().child(entity).into_any_element()
        } else {
            // Fallback message if the entity wasn't registered
            use gpui::px;
            div()
                .px_3()
                .py_2()
                .text_size(px(12.))
                .text_color(gpui::white())
                .font_family("monospace")
                .child("Preview view not available")
                .into_any_element()
        }
    }
}

impl HelixPickerCapability {
    pub fn register_preview_entity(
        &mut self,
        doc_id: DocumentId,
        view_id: ViewId,
        entity: Entity<crate::document::DocumentView>,
    ) {
        self.previews.insert((doc_id, view_id), entity);
    }

    pub fn unregister_preview_entity(&mut self, doc_id: DocumentId, view_id: ViewId) {
        self.previews.remove(&(doc_id, view_id));
    }
}

/// Sketch of an event-based preview executor used by the overlay to open/close previews
pub struct PickerPreviewExecutor {
    core: gpui::WeakEntity<Core>,
}

impl PickerPreviewExecutor {
    pub fn new_from_weak(core: gpui::WeakEntity<Core>) -> Self {
        Self { core }
    }

    /// Attempt to open a lightweight preview for a path; returns (doc_id, view_id) if a preview view is created.
    /// Current minimal impl avoids changing the editor layout and returns None.
    /// Future: dispatch an internal command/event to open a temporary preview view.
    pub fn open(
        &self,
        _path: &std::path::Path,
        cx: &mut gpui::Context<nucleotide_ui::picker_view::PickerView>,
    ) -> Option<(helix_view::DocumentId, helix_view::ViewId)> {
        let Some(core) = self.core.upgrade() else {
            return None;
        };

        let mut out: Option<(helix_view::DocumentId, helix_view::ViewId)> = None;
        core.update(cx, |core, picker_cx| {
            // Detect if a document for this path already exists
            let existed_already = core
                .editor
                .documents
                .iter()
                .any(|(_, d)| d.path().map(|p| p.as_path() == _path).unwrap_or(false));
            // Open in a vertical split so we don't replace the user's current view
            let action = helix_view::editor::Action::VerticalSplit;
            match core.editor.open(_path, action) {
                Ok(doc_id) => {
                    let view_id = core.editor.tree.focus;
                    // Track this preview with ephemeral flag
                    if let Some(tracker) =
                        picker_cx.try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
                    {
                        tracker.register_with_flag(doc_id, view_id, !existed_already);
                    }
                    out = Some((doc_id, view_id));
                }
                Err(_e) => {
                    // Failed to open; keep None
                }
            }
        });
        out
    }

    /// Close a previously opened preview if it was created via open().
    pub fn close(
        &self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        cx: &mut gpui::Context<nucleotide_ui::picker_view::PickerView>,
    ) {
        if let Some(core) = self.core.upgrade() {
            // Step 1: Close the preview view and determine if other views reference the doc
            let referenced_elsewhere = core.update(cx, |core, _| {
                if core.editor.tree.contains(view_id) {
                    core.editor.close(view_id);
                }
                core.editor.tree.views().any(|(v, _)| v.doc == doc_id)
            });

            // Step 2: Determine should_close_doc using tracker outside the update borrow
            let should_close_doc = if let Some(tracker) =
                cx.try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            {
                tracker.is_ephemeral_doc(doc_id) && !referenced_elsewhere
            } else {
                !referenced_elsewhere
            };

            // Step 3: Close the document if needed
            if should_close_doc {
                core.update(cx, |core, _| {
                    let _ = core.editor.close_document(doc_id, false);
                });
            }

            // Step 4: Unregister from preview tracker
            if let Some(tracker) =
                cx.try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            {
                tracker.unregister(doc_id, view_id);
            }
        }
    }
}
