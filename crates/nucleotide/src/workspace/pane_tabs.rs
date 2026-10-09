use std::collections::{HashMap, HashSet};

use gpui::{Bounds, Pixels, ScrollHandle};
use helix_view::{DocumentId, ViewId};

use crate::tab::TabId;

/// Tabs belong to views; buffers remain shared by the Helix editor.
pub(super) struct PaneTabs {
    pub(super) tabs: Vec<TabId>,
    pub(super) active_image: Option<u64>,
    document: DocumentId,
    pub(super) scroll: ScrollHandle,
    pub(super) last_scrolled: Option<TabId>,
    pub(super) suppress_auto_scroll: bool,
    pub(super) split_button_bounds: Option<Bounds<Pixels>>,
}

impl PaneTabs {
    fn new(document: DocumentId) -> Self {
        Self {
            tabs: vec![TabId::Document(document)],
            active_image: None,
            document,
            scroll: ScrollHandle::new(),
            last_scrolled: None,
            suppress_auto_scroll: false,
            split_button_bounds: None,
        }
    }

    pub(super) fn active(&self) -> TabId {
        self.active_image
            .map(TabId::Image)
            .unwrap_or(TabId::Document(self.document))
    }
}

#[derive(Default)]
pub(super) struct PaneTabState {
    pub(super) panes: HashMap<ViewId, PaneTabs>,
    pub(super) focused: Option<ViewId>,
}

impl PaneTabState {
    /// Reconcile commands issued through Helix too, not just GUI split buttons.
    /// A new split starts with its displayed document. Orphaned tabs from a
    /// closed split (and newly loaded buffers) go to the focused pane.
    pub(super) fn sync(
        &mut self,
        views: &[(ViewId, DocumentId)],
        focused: ViewId,
        available: &[TabId],
        active_image: &mut Option<u64>,
    ) {
        let live: HashSet<_> = views.iter().map(|(id, _)| *id).collect();
        let available_set: HashSet<_> = available.iter().copied().collect();
        if active_image.is_some_and(|id| !available_set.contains(&TabId::Image(id))) {
            *active_image = None;
        }
        self.panes.retain(|id, _| live.contains(id));
        for &(id, document) in views {
            let pane = self
                .panes
                .entry(id)
                .or_insert_with(|| PaneTabs::new(document));
            pane.tabs.retain(|tab| available_set.contains(tab));
            if pane.document != document {
                pane.active_image = None;
                pane.document = document;
                if self.focused == Some(id) && id == focused {
                    *active_image = None;
                }
            }
            if pane.active_image.is_none() && !pane.tabs.contains(&TabId::Document(document)) {
                pane.tabs.push(TabId::Document(document));
            }
            if pane
                .active_image
                .is_some_and(|id| !available_set.contains(&TabId::Image(id)))
            {
                pane.active_image = None;
            }
        }
        if self.focused.is_some() && self.focused != Some(focused) {
            *active_image = self.panes.get(&focused).and_then(|pane| pane.active_image);
        }
        self.focused = Some(focused);
        let assigned: HashSet<_> = self
            .panes
            .values()
            .flat_map(|pane| pane.tabs.iter().copied())
            .collect();
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.tabs.extend(
                available
                    .iter()
                    .copied()
                    .filter(|tab| !assigned.contains(tab)),
            );
            pane.active_image = *active_image;
            if let Some(image) = *active_image
                && !pane.tabs.contains(&TabId::Image(image))
            {
                pane.tabs.push(TabId::Image(image));
            }
        }
    }

    /// Transfer rather than copy. Dropping on an existing copy deduplicates it.
    /// `before` also supports reordering within the same pane.
    pub(super) fn move_tab(
        &mut self,
        source: ViewId,
        target: ViewId,
        tab: TabId,
        before: Option<TabId>,
    ) -> bool {
        if before == Some(tab) && source == target {
            return false;
        }
        if !self
            .panes
            .get(&source)
            .is_some_and(|pane| pane.tabs.contains(&tab))
            || !self
                .panes
                .get(&target)
                .is_some_and(|pane| before.is_none_or(|id| pane.tabs.contains(&id)))
        {
            return false;
        }
        self.panes
            .get_mut(&source)
            .unwrap()
            .tabs
            .retain(|id| *id != tab);
        let pane = self.panes.get_mut(&target).unwrap();
        pane.tabs.retain(|id| *id != tab);
        let index = before
            .and_then(|id| pane.tabs.iter().position(|candidate| *candidate == id))
            .unwrap_or(pane.tabs.len());
        pane.tabs.insert(index, tab);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::SlotMap;

    #[test]
    fn split_move_reorder_and_close_preserve_tab_membership() {
        let mut views = SlotMap::<ViewId, ()>::with_key();
        let left = views.insert(());
        let right = views.insert(());
        let doc = DocumentId::default();
        let a = TabId::Document(doc);
        let b = TabId::Image(7);
        let c = TabId::Image(9);
        let mut state = PaneTabState::default();
        let mut image = None;
        state.sync(&[(left, doc)], left, &[a, b, c], &mut image);
        state.sync(&[(left, doc), (right, doc)], right, &[a, b, c], &mut image);
        assert_eq!(state.panes[&left].tabs, [a, b, c]);
        assert_eq!(state.panes[&right].tabs, [a]);
        assert!(state.move_tab(left, right, b, Some(a)));
        assert_eq!(state.panes[&left].tabs, [a, c]);
        assert_eq!(state.panes[&right].tabs, [b, a]);
        assert!(state.move_tab(right, right, a, Some(b)));
        assert_eq!(state.panes[&right].tabs, [a, b]);
        assert!(state.move_tab(left, right, a, None));
        assert_eq!(state.panes[&left].tabs, [c]);
        assert_eq!(state.panes[&right].tabs, [b, a]);
        state.sync(&[(right, doc)], right, &[a, b, c], &mut image);
        assert_eq!(state.panes[&right].tabs, [b, a, c]);
        state.sync(&[(right, doc)], right, &[a, c], &mut image);
        assert_eq!(state.panes[&right].tabs, [a, c]);
    }

    #[test]
    fn image_selection_and_scroll_are_independent_per_pane() {
        let mut views = SlotMap::<ViewId, ()>::with_key();
        let left = views.insert(());
        let right = views.insert(());
        let doc = DocumentId::default();
        let mut state = PaneTabState::default();
        let tabs = [TabId::Document(doc), TabId::Image(4)];
        let mut image = None;
        state.sync(&[(left, doc), (right, doc)], left, &tabs, &mut image);
        image = Some(4);
        state.sync(&[(left, doc), (right, doc)], left, &tabs, &mut image);
        state.panes.get_mut(&left).unwrap().suppress_auto_scroll = true;
        state.sync(&[(left, doc), (right, doc)], right, &tabs, &mut image);
        assert_eq!(image, None);
        assert!(!state.panes[&right].suppress_auto_scroll);
        state.sync(&[(left, doc), (right, doc)], left, &tabs, &mut image);
        assert_eq!(image, Some(4));
    }
}
