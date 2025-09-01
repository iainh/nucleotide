use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use gpui::{AppContext, Entity};
use helix_view::{DocumentId, ViewId};

use crate::Core;

/// Concrete implementation that uses the application's Helix editor
pub struct HelixPickerCapability {
    core: gpui::WeakEntity<Core>,
}

impl HelixPickerCapability {
    pub fn new(core: &Entity<Core>) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            core: core.downgrade(),
        }))
    }
}

impl nucleotide_core::capabilities::PickerCapability for HelixPickerCapability {
    fn open_preview<C: AppContext>(
        &mut self,
        path: &Path,
        cx: &mut C,
    ) -> Result<(DocumentId, ViewId), String> {
        let Some(core) = self.core.upgrade() else {
            return Err("Core unavailable".into());
        };

        // Open the file into the current focus (Replace). This avoids creating extra splits.
        let mut result: Result<(DocumentId, ViewId), String> = Err("Failed to open".into());
        core.update(cx, |core, _cx| {
            let action = if core.editor.tree.views().count() == 0 {
                helix_view::editor::Action::VerticalSplit
            } else {
                helix_view::editor::Action::Replace
            };
            match core.editor.open(path, action) {
                Ok(doc_id) => {
                    let view_id = core.editor.tree.focus;
                    result = Ok((doc_id, view_id));
                }
                Err(e) => {
                    result = Err(format!("Open failed: {e}"));
                }
            }
        });
        result
    }

    fn close_preview<C: AppContext>(
        &mut self,
        doc_id: DocumentId,
        view_id: ViewId,
        cx: &mut C,
    ) -> Result<(), String> {
        let Some(core) = self.core.upgrade() else {
            return Err("Core unavailable".into());
        };
        let mut result: Result<(), String> = Ok(());
        core.update(cx, |core, _cx| {
            if core.editor.tree.contains(view_id) {
                core.editor.close(view_id);
            }
            let _ = core.editor.close_document(doc_id, false);
        });
        result
    }
}
