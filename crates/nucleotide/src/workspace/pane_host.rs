//! The workspace owns placements; Helix owns retained editing slots.
//!
//! Import the CLI tree exactly once, before enabling hosted mode. From then on
//! only ordered editor effects and explicit workspace operations change layout.

use super::pane_layout::{Axis, Edge, InitialLayout, PaneId, PaneLayout, SplitMode};
use crate::tab::TabId;
use helix_view::editor::hosted::{DocumentReplacementReason, HostedEffect, SlotRemovalReason};
use helix_view::{DocumentId, Editor, ViewId, editor::Action, tree};
use std::collections::HashMap;

pub(super) struct PaneHost {
    pub layout: PaneLayout,
    pub slots: HashMap<PaneId, ViewId>,
    transfers: HashMap<PaneId, (ViewId, Vec<TabId>)>,
}

impl PaneHost {
    pub fn import(editor: &Editor) -> Self {
        fn topology(tree: &tree::Tree, id: ViewId, leaves: &mut Vec<ViewId>) -> InitialLayout {
            match tree.content(id).expect("initial tree node exists") {
                tree::Content::View(view) => {
                    leaves.push(view.id);
                    InitialLayout::Pane {
                        tabs: vec![TabId::Document(view.doc)],
                        active: Some(TabId::Document(view.doc)),
                    }
                }
                tree::Content::Container(container) if container.children().len() == 1 => {
                    topology(tree, container.children()[0], leaves)
                }
                tree::Content::Container(container) => InitialLayout::Split {
                    axis: match container.layout() {
                        tree::Layout::Vertical => Axis::Horizontal,
                        tree::Layout::Horizontal => Axis::Vertical,
                    },
                    children: container
                        .children()
                        .iter()
                        .map(|id| {
                            let area = match tree.content(*id).unwrap() {
                                tree::Content::View(view) => view.area,
                                tree::Content::Container(container) => container.area(),
                            };
                            let weight = match container.layout() {
                                tree::Layout::Vertical => area.width,
                                tree::Layout::Horizontal => area.height,
                            }
                            .max(1) as f32;
                            (weight, topology(tree, *id, leaves))
                        })
                        .collect(),
                },
            }
        }

        let mut leaves = Vec::new();
        let mut layout = if editor.tree.is_empty() {
            PaneLayout::new(Vec::new())
        } else {
            PaneLayout::from_topology(topology(&editor.tree, editor.tree.root(), &mut leaves))
                .expect("the initial editor topology is valid")
        };
        let slots: HashMap<_, _> = layout.main_panes().into_iter().zip(leaves).collect();
        if let Some(pane) = slots
            .iter()
            .find_map(|(pane, slot)| (*slot == editor.tree.focus).then_some(*pane))
        {
            layout.activate_pane(pane).unwrap();
        }
        // Loaded buffers not displayed by CLI splits belong to the initial active pane.
        layout
            .assign_unplaced_tabs(
                layout.active_pane(),
                editor.documents.keys().copied().map(TabId::Document),
            )
            .unwrap();
        Self {
            layout,
            slots,
            transfers: HashMap::new(),
        }
    }

    /// Capture the editing owner before a model mutation removes its pane.
    pub fn transfer_state(&mut self, source: PaneId, target: PaneId) {
        if source != target
            && let Some(slot) = self.slots.get(&source)
        {
            let existing = self
                .layout
                .pane(target)
                .map(|pane| pane.tabs().to_vec())
                .unwrap_or_default();
            self.transfers.insert(target, (*slot, existing));
        }
    }

    pub fn pane_for_slot(&self, slot: ViewId) -> Option<PaneId> {
        self.slots
            .iter()
            .find_map(|(pane, id)| (*id == slot).then_some(*pane))
    }

    pub fn active_slot(&self) -> Option<ViewId> {
        let pane = self.layout.active_pane();
        matches!(
            self.layout.pane(pane)?.active_tab(),
            Some(TabId::Document(_))
        )
        .then(|| self.slots.get(&pane).copied())
        .flatten()
    }

    /// Consume facts at a command/job boundary, never during rendering.
    pub fn apply_effects(&mut self, effects: Vec<HostedEffect>) -> Result<(), String> {
        let mut unplaced = Vec::new();
        for effect in effects {
            match effect {
                HostedEffect::DocumentRegistered { document } => {
                    unplaced.push(TabId::Document(document));
                }
                HostedEffect::DocumentLoaded { document, slot } => {
                    let pane = slot
                        .and_then(|slot| self.pane_for_slot(slot))
                        .unwrap_or(self.layout.active_pane());
                    self.layout
                        .assign_unplaced_tabs(pane, [TabId::Document(document)])
                        .map_err(|error| format!("{error:?}"))?;
                }
                HostedEffect::SlotCreated {
                    slot,
                    document,
                    source,
                    action,
                } => {
                    let source = source
                        .and_then(|source| self.pane_for_slot(source))
                        .unwrap_or(self.layout.active_pane());
                    if matches!(
                        action,
                        Action::HorizontalSplit | Action::VerticalSplit | Action::SplitAt { .. }
                    ) {
                        self.split_document(source, document, action, slot)?;
                    } else {
                        if self.slots.contains_key(&source) {
                            self.transfers.insert(source, (slot, Vec::new()));
                        } else {
                            self.slots.insert(source, slot);
                        }
                        self.layout
                            .open_tab(source, TabId::Document(document), true)
                            .map_err(|error| format!("{error:?}"))?;
                    }
                }
                HostedEffect::DocumentReplaced {
                    slot,
                    document,
                    reason,
                    ..
                } => {
                    if reason == DocumentReplacementReason::DocumentClose {
                        continue;
                    }
                    if let Some(pane) = self.pane_for_slot(slot) {
                        self.layout
                            .open_tab(pane, TabId::Document(document), true)
                            .map_err(|error| format!("{error:?}"))?;
                    }
                }
                HostedEffect::SlotRemoved { slot, reason, .. } => {
                    if let Some(pane) = self.pane_for_slot(slot) {
                        self.slots.remove(&pane);
                        // Buffer fallback is not a pane-close command.
                        if reason == SlotRemovalReason::ExplicitClose {
                            self.layout
                                .close_pane(pane, None)
                                .map_err(|error| format!("{error:?}"))?;
                        }
                    }
                }
                HostedEffect::DocumentClosed { document } => self.remove_document(document),
                HostedEffect::Focused { slot } => {
                    if let Some(pane) = self.pane_for_slot(slot) {
                        self.layout
                            .activate_pane(pane)
                            .map_err(|error| format!("{error:?}"))?;
                    }
                }
            }
        }
        self.layout
            .assign_unplaced_tabs(self.layout.active_pane(), unplaced)
            .map_err(|error| format!("{error:?}"))?;
        Ok(())
    }

    /// Create only real document slots. A content-only pane never gets a placeholder.
    pub fn project(&mut self, editor: &mut Editor) -> Result<(), String> {
        // Include transient slots created while content-only focus retained an
        // existing editing owner. Every real slot must be retained or released.
        let snapshots: HashMap<_, _> = editor
            .tree
            .views()
            .map(|(view, _)| {
                let slot = view.id;
                let documents: Vec<_> = editor
                    .documents
                    .iter()
                    .filter_map(|(id, doc)| {
                        Some((
                            *id,
                            doc.selections().get(&slot)?.clone(),
                            doc.view_offset(slot),
                        ))
                    })
                    .collect();
                (slot, (view.clone(), documents))
            })
            .collect();
        let dead: Vec<_> = self
            .slots
            .iter()
            .filter_map(|(pane, slot)| {
                (self.layout.pane(*pane).is_none() || !editor.tree.contains(*slot))
                    .then_some((*pane, *slot))
            })
            .collect();
        for (pane, slot) in dead {
            self.slots.remove(&pane);
            // Keep the real slot alive until destination state has been copied.
            let _ = slot;
        }
        for pane in self.layout.panes() {
            let state = self.layout.pane(pane).unwrap();
            let document = match state.active_tab() {
                Some(TabId::Document(document)) => Some(document),
                _ => self
                    .slots
                    .get(&pane)
                    .and_then(|slot| editor.tree.try_get(*slot))
                    .map(|view| view.doc)
                    .filter(|document| state.tabs().contains(&TabId::Document(*document)))
                    .or_else(|| {
                        state.tabs().iter().find_map(|tab| match tab {
                            TabId::Document(document) => Some(*document),
                            _ => None,
                        })
                    }),
            };
            if let Some(document) = document {
                let existing = self.slots.get(&pane).copied();
                let transfer = self.transfers.remove(&pane);
                let preserve_target = existing.is_some()
                    && transfer.as_ref().map_or_else(
                        || {
                            existing
                                .and_then(|slot| editor.tree.try_get(slot))
                                .is_some_and(|view| view.doc == document)
                        },
                        |(_, tabs)| tabs.contains(&TabId::Document(document)),
                    );
                let source = transfer.map(|(source, _)| source).or_else(|| {
                    existing
                        .is_none()
                        .then(|| {
                            snapshots.iter().find_map(|(slot, (view, _))| {
                                (view.doc == document).then_some(*slot)
                            })
                        })
                        .flatten()
                });
                if let Some(slot) = self.slots.get(&pane).copied() {
                    if editor.tree.get(slot).doc != document {
                        editor
                            .project_hosted_replace(slot, document)
                            .map_err(|error| format!("{error:?}"))?;
                    }
                } else {
                    let slot = editor
                        .project_hosted_slot(document)
                        .map_err(|error| format!("{error:?}"))?;
                    self.slots.insert(pane, slot);
                }
                if !preserve_target
                    && let Some((view, documents)) =
                        source.and_then(|source| snapshots.get(&source))
                {
                    let slot = self.slots[&pane];
                    // A new placement inherits history. An existing destination
                    // retains its own history and all unrelated document state.
                    if existing.is_none() {
                        let mut copied = view.clone();
                        copied.id = slot;
                        copied.doc = document;
                        *editor.tree.get_mut(slot) = copied;
                    }
                    for (id, selection, offset) in documents {
                        if *id == document
                            && let Some(doc) = editor.documents.get_mut(id)
                        {
                            doc.set_selection(slot, selection.clone());
                            doc.set_view_offset(slot, *offset);
                        }
                    }
                }
            } else if let Some(slot) = self.slots.remove(&pane) {
                editor
                    .project_hosted_release(slot)
                    .map_err(|error| format!("{error:?}"))?;
            }
        }
        self.transfers.clear();
        let retained: Vec<_> = self.slots.values().copied().collect();
        for slot in snapshots.keys().filter(|slot| !retained.contains(slot)) {
            if editor.tree.contains(*slot) {
                editor
                    .project_hosted_release(*slot)
                    .map_err(|error| format!("{error:?}"))?;
            }
        }
        editor
            .project_hosted_focus(self.active_slot())
            .map_err(|error| format!("{error:?}"))
    }

    pub fn split_document(
        &mut self,
        source: PaneId,
        document: DocumentId,
        action: Action,
        slot: ViewId,
    ) -> Result<PaneId, String> {
        let (edge, mode) = match action {
            Action::HorizontalSplit => (Edge::Bottom, SplitMode::Sibling),
            Action::VerticalSplit => (Edge::Right, SplitMode::Sibling),
            Action::SplitAt { layout, before } => (
                match (layout, before) {
                    (tree::Layout::Vertical, true) => Edge::Left,
                    (tree::Layout::Vertical, false) => Edge::Right,
                    (tree::Layout::Horizontal, true) => Edge::Top,
                    (tree::Layout::Horizontal, false) => Edge::Bottom,
                },
                SplitMode::TargetOnly,
            ),
            _ => return Err("slot creation did not carry a split action".into()),
        };
        let pane = self
            .layout
            .split_pane(source, edge, mode, vec![TabId::Document(document)])
            .map_err(|error| format!("{error:?}"))?;
        self.slots.insert(pane, slot);
        Ok(pane)
    }

    pub fn remove_document(&mut self, document: DocumentId) {
        for pane in self.layout.panes() {
            if self
                .layout
                .pane(pane)
                .is_some_and(|state| state.tabs().contains(&TabId::Document(document)))
            {
                let _ = self.layout.close_tab(pane, TabId::Document(document));
            }
        }
    }
}
