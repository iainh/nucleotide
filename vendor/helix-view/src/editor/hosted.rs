//! Workspace-owned presentation of real Helix editing slots.
//!
//! Drain facts after each command/job, before executing its continuation. These are
//! not pane commands: a buffer close may remove a slot without closing a pane.
//! Explicit pane operations belong to the host command router. Projection is silent.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotRemovalReason {
    ExplicitClose,
    DocumentClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentReplacementReason {
    Switch,
    DocumentClose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostedEffect {
    DocumentRegistered {
        document: DocumentId,
    },
    DocumentLoaded {
        document: DocumentId,
        slot: Option<ViewId>,
    },
    SlotCreated {
        slot: ViewId,
        document: DocumentId,
        source: Option<ViewId>,
        action: Action,
    },
    DocumentReplaced {
        slot: ViewId,
        previous: DocumentId,
        document: DocumentId,
        reason: DocumentReplacementReason,
    },
    SlotRemoved {
        slot: ViewId,
        document: DocumentId,
        reason: SlotRemovalReason,
    },
    DocumentClosed {
        document: DocumentId,
    },
    Focused {
        slot: ViewId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostedError {
    NotHosted,
    MissingSlot,
    MissingDocument,
}

#[derive(Default)]
pub(super) struct HostedState {
    pub active: Option<ViewId>,
    pub projecting: bool,
    pub effects: Vec<HostedEffect>,
}

impl Editor {
    /// Adopt existing real slots. The host bootstraps pane membership once, then
    /// consumes ordered effects; enabling does not invent creation effects.
    pub fn enable_hosted(&mut self) {
        if self.hosted.is_none() {
            self.hosted = Some(HostedState {
                active: self.tree.try_get(self.tree.focus).map(|view| view.id),
                ..HostedState::default()
            });
            self.tree.enable_hosted();
        }
    }

    pub fn is_hosted(&self) -> bool {
        self.hosted.is_some()
    }

    /// None means a content-only pane is focused (or no editor slot exists).
    /// Never route editing through `tree.focus` without checking this first.
    pub fn hosted_active_slot(&self) -> Option<ViewId> {
        self.hosted.as_ref().and_then(|host| host.active)
    }

    /// Whether a command continuation must yield for host reconciliation.
    /// This observes pending facts without consuming them or inferring pane intent.
    pub fn has_hosted_effects(&self) -> bool {
        self.hosted
            .as_ref()
            .is_some_and(|host| !host.effects.is_empty())
    }

    pub fn drain_hosted_effects(&mut self) -> Vec<HostedEffect> {
        self.hosted
            .as_mut()
            .map(|host| std::mem::take(&mut host.effects))
            .unwrap_or_default()
    }

    pub(super) fn record_hosted(&mut self, effect: HostedEffect) {
        if let Some(host) = &mut self.hosted {
            if !host.projecting {
                host.effects.push(effect);
            }
        }
    }

    pub(super) fn activate_hosted(&mut self, slot: ViewId) {
        if let Some(host) = &mut self.hosted {
            host.active = Some(slot);
        }
    }

    fn validate_hosted_slot(&self, slot: ViewId) -> Result<(), HostedError> {
        self.hosted.as_ref().ok_or(HostedError::NotHosted)?;
        self.tree.try_get(slot).ok_or(HostedError::MissingSlot)?;
        Ok(())
    }

    fn validate_hosted_document(&self, document: DocumentId) -> Result<(), HostedError> {
        self.hosted.as_ref().ok_or(HostedError::NotHosted)?;
        self.documents
            .get(&document)
            .ok_or(HostedError::MissingDocument)?;
        Ok(())
    }

    // Only narrow projection operations use this scope. Normal commands and async
    // jobs never run inside it, so their structural effects cannot be swallowed.
    fn project_hosted<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.hosted.as_mut().unwrap().projecting;
        self.hosted.as_mut().unwrap().projecting = true;
        let result = f(self);
        self.hosted.as_mut().unwrap().projecting = previous;
        result
    }

    /// Create and focus a real document slot, without asking for a new GUI pane.
    /// All hosted documents (including undisplayed scratch tabs) are retained
    /// until `close_document`; releasing a presentation never deletes a buffer.
    pub fn project_hosted_slot(&mut self, document: DocumentId) -> Result<ViewId, HostedError> {
        self.validate_hosted_document(document)?;
        Ok(self.project_hosted(|editor| {
            editor.switch(document, Action::VerticalSplit);
            editor.tree.focus
        }))
    }

    /// Replace using the ordinary switch history/jump/mode/hooks contract.
    /// Existing per-slot selection and scroll state are presentation state, not
    /// a request to reveal the cursor. Newly initialized targets align normally.
    /// This also focuses the slot; callers restoring background tabs should
    /// explicitly project their final active slot afterwards.
    pub fn project_hosted_replace(
        &mut self,
        slot: ViewId,
        document: DocumentId,
    ) -> Result<(), HostedError> {
        self.validate_hosted_slot(slot)?;
        self.validate_hosted_document(document)?;
        let doc = &self.documents[&document];
        let retained = doc
            .selections()
            .get(&slot)
            .cloned()
            .zip(doc.get_view_offset(slot));
        self.project_hosted_focus(Some(slot))?;
        self.project_hosted(|editor| {
            editor.switch(document, Action::Replace);
            if let Some((selection, offset)) = retained {
                let doc = doc_mut!(editor, &document);
                if doc.selection(slot) != &selection {
                    doc.set_selection(slot, selection);
                }
                doc.set_view_offset(slot, offset);
            }
        });
        Ok(())
    }

    pub fn project_hosted_release(&mut self, slot: ViewId) -> Result<(), HostedError> {
        self.validate_hosted_slot(slot)?;
        self.project_hosted(|editor| editor.close(slot));
        Ok(())
    }

    /// None commits pending history and disarms editing, but retains hidden real
    /// slots/selections. The command router must stop editor continuations here.
    pub fn project_hosted_focus(&mut self, slot: Option<ViewId>) -> Result<(), HostedError> {
        self.hosted.as_ref().ok_or(HostedError::NotHosted)?;
        if let Some(slot) = slot {
            self.validate_hosted_slot(slot)?;
            let document = self.tree.get(slot).doc;
            let doc = &self.documents[&document];
            let selection = doc.selection(slot).clone();
            let offset = doc.view_offset(slot);
            self.project_hosted(|editor| {
                editor.focus(slot);
                let doc = doc_mut!(editor, &document);
                if doc.selection(slot) != &selection {
                    doc.set_selection(slot, selection);
                }
                doc.set_view_offset(slot, offset);
            });
        } else {
            self.project_hosted(|editor| {
                let previous = editor.hosted_active_slot();
                if previous.is_some() {
                    editor.enter_normal_mode();
                    let (view, doc) = current!(editor);
                    doc.append_changes_to_history(view);
                }
                editor.hosted.as_mut().unwrap().active = None;
                if let Some(previous) = previous {
                    let doc = editor.tree.get(previous).doc;
                    dispatch(DocumentFocusLost { editor, doc });
                }
            });
        }
        Ok(())
    }

    /// Cell geometry already derived from the host's logical-pixel content rect.
    /// Tree split/remove/resize/recalculate cannot overwrite this projection.
    pub fn project_hosted_viewport(&mut self, slot: ViewId, area: Rect) -> Result<(), HostedError> {
        self.validate_hosted_slot(slot)?;
        self.tree.get_mut(slot).area = area;
        self.ensure_cursor_in_view(slot);
        Ok(())
    }
}
