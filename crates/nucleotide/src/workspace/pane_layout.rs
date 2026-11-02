//! Workspace-owned, content-neutral pane topology. No editor or UI handles live here.
//!
//! Import editor topology once with `from_topology`; controllers then apply commands
//! to this model and maintain their own retained editor-slot mapping. Tab identity is
//! unique only *within* a pane. Controllers may impose stronger policies for terminals.

use crate::tab::TabId;
use slotmap::{Key, SlotMap, new_key_type};
use std::collections::HashSet;

new_key_type! {
    pub struct PaneId;
    pub struct SplitId;
}

impl PaneId {
    /// Opaque generational key for presentation maps; never persist across sessions.
    pub fn key(self) -> u64 {
        self.data().as_ffi()
    }
}

impl SplitId {
    pub fn key(self) -> u64 {
        self.data().as_ffi()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Pane(PaneId),
    Split(SplitId),
}

/// Direction in which children are laid out (not the divider's orientation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Top | Self::Bottom => Axis::Vertical,
        }
    }

    fn before(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitMode {
    /// Bisect only this pane, even if its parent has the same axis.
    TargetOnly,
    /// Insert in a matching-axis parent and equalize all siblings (Helix split
    /// command semantics); otherwise wrap the target.
    Sibling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabSuccessor {
    MostRecent,
    /// Next tab in strip order, falling back to the previous tab at the end.
    Adjacent,
    /// Previous tab in strip order, falling back to the next tab at the start.
    LeftAdjacent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaneSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneGeometry {
    pub pane: PaneId,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct Pane {
    tabs: Vec<TabId>,
    /// Least recently active first; the final entry is the active tab.
    history: Vec<TabId>,
}

impl Pane {
    fn new(tabs: Vec<TabId>) -> Self {
        let mut pane = Self {
            tabs: Vec::new(),
            history: Vec::new(),
        };
        for tab in tabs {
            pane.insert(tab, usize::MAX, false);
        }
        pane
    }

    pub fn tabs(&self) -> &[TabId] {
        &self.tabs
    }
    pub fn history(&self) -> &[TabId] {
        &self.history
    }
    pub fn active_tab(&self) -> Option<TabId> {
        self.history.last().copied()
    }

    fn activate(&mut self, tab: TabId) {
        self.history.retain(|item| *item != tab);
        self.history.push(tab);
    }

    fn insert(&mut self, tab: TabId, index: usize, activate: bool) {
        if !self.tabs.contains(&tab) {
            self.tabs.insert(index.min(self.tabs.len()), tab);
            self.history.insert(0, tab);
        }
        if activate {
            self.activate(tab);
        }
    }

    fn remove(&mut self, tab: TabId, successor: TabSuccessor) {
        let adjacent = if successor != TabSuccessor::MostRecent && self.active_tab() == Some(tab) {
            let index = self.tabs.iter().position(|item| *item == tab).unwrap();
            let left = index.checked_sub(1).map(|i| self.tabs[i]);
            let right = self.tabs.get(index + 1).copied();
            if successor == TabSuccessor::LeftAdjacent {
                left.or(right)
            } else {
                right.or(left)
            }
        } else {
            None
        };
        self.tabs.retain(|item| *item != tab);
        self.history.retain(|item| *item != tab);
        if let Some(tab) = adjacent {
            self.activate(tab);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeightedChild {
    pub node: Node,
    /// Positive, finite relative weight. Not necessarily normalized to one.
    pub weight: f32,
}

#[derive(Clone, Debug)]
pub struct Split {
    axis: Axis,
    children: Vec<WeightedChild>,
}

impl Split {
    pub fn axis(&self) -> Axis {
        self.axis
    }
    pub fn children(&self) -> &[WeightedChild] {
        &self.children
    }
}

/// An import description, deliberately independent of Helix's tree and view IDs.
/// Match imported view IDs to `main_panes()` in depth-first child order.
#[derive(Clone, Debug)]
pub enum InitialLayout {
    Pane {
        tabs: Vec<TabId>,
        active: Option<TabId>,
    },
    Split {
        axis: Axis,
        children: Vec<(f32, InitialLayout)>,
    },
}

#[derive(Clone, Debug)]
pub struct Dock {
    root: Node,
    visible: bool,
    size: f32,
}

impl Dock {
    pub fn root(&self) -> Node {
        self.root
    }
    pub fn visible(&self) -> bool {
        self.visible
    }
    /// Remembered logical-pixel height, unaffected by viewport clamping.
    pub fn size(&self) -> f32 {
        self.size
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    MissingPane,
    MissingSplit,
    MissingTab,
    HiddenPane,
    LastPane,
    SamePane,
    InvalidWeight,
    InvalidGeometry,
    InvalidTopology,
}

#[derive(Clone, Debug)]
pub struct PaneLayout {
    panes: SlotMap<PaneId, Pane>,
    splits: SlotMap<SplitId, Split>,
    root: Node,
    dock: Option<Dock>,
    active: PaneId,
    pane_history: Vec<PaneId>,
}

impl PaneLayout {
    /// A workspace always retains at least one main pane, possibly empty.
    pub fn new(tabs: Vec<TabId>) -> Self {
        let mut panes = SlotMap::with_key();
        let active = panes.insert(Pane::new(tabs));
        Self {
            panes,
            splits: SlotMap::with_key(),
            root: Node::Pane(active),
            dock: None,
            active,
            pane_history: vec![active],
        }
    }

    pub fn from_topology(initial: InitialLayout) -> Result<Self, LayoutError> {
        fn check(node: &InitialLayout) -> Result<(), LayoutError> {
            match node {
                InitialLayout::Pane { tabs, active } => {
                    if active.is_some_and(|tab| !tabs.contains(&tab)) {
                        return Err(LayoutError::MissingTab);
                    }
                }
                InitialLayout::Split { children, .. } => {
                    if children.len() < 2 {
                        return Err(LayoutError::InvalidTopology);
                    }
                    for (weight, child) in children {
                        valid_weight(*weight)?;
                        check(child)?;
                    }
                }
            }
            Ok(())
        }
        fn import(layout: &mut PaneLayout, node: InitialLayout) -> Node {
            match node {
                InitialLayout::Pane { tabs, active } => {
                    let mut pane = Pane::new(tabs);
                    if let Some(tab) = active {
                        pane.activate(tab);
                    }
                    Node::Pane(layout.panes.insert(pane))
                }
                InitialLayout::Split { axis, children } => {
                    let children = children
                        .into_iter()
                        .map(|(weight, node)| WeightedChild {
                            node: import(layout, node),
                            weight,
                        })
                        .collect();
                    Node::Split(layout.splits.insert(Split { axis, children }))
                }
            }
        }
        check(&initial)?;
        let mut layout = Self::new(Vec::new());
        layout.panes.clear();
        layout.root = import(&mut layout, initial);
        layout.active = layout.main_panes()[0];
        layout.pane_history = vec![layout.active];
        Ok(layout)
    }

    pub fn root(&self) -> Node {
        self.root
    }
    pub fn dock(&self) -> Option<&Dock> {
        self.dock.as_ref()
    }
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(id)
    }
    pub fn split(&self, id: SplitId) -> Option<&Split> {
        self.splits.get(id)
    }
    pub fn active_pane(&self) -> PaneId {
        self.active
    }
    pub fn pane_history(&self) -> &[PaneId] {
        &self.pane_history
    }
    pub fn main_panes(&self) -> Vec<PaneId> {
        self.leaves(self.root)
    }

    /// Main panes followed by dock panes, including hidden dock panes.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut panes = self.main_panes();
        if let Some(dock) = &self.dock {
            panes.extend(self.leaves(dock.root));
        }
        panes
    }

    fn leaves(&self, node: Node) -> Vec<PaneId> {
        match node {
            Node::Pane(id) => vec![id],
            Node::Split(id) => self.splits[id]
                .children
                .iter()
                .flat_map(|child| self.leaves(child.node))
                .collect(),
        }
    }

    fn parent(&self, node: Node) -> Option<(SplitId, usize)> {
        self.splits.iter().find_map(|(id, split)| {
            split
                .children
                .iter()
                .position(|child| child.node == node)
                .map(|index| (id, index))
        })
    }

    fn visible(&self, pane: PaneId) -> bool {
        self.panes.contains_key(pane)
            && (self.main_panes().contains(&pane)
                || self.dock.as_ref().is_some_and(|dock| dock.visible))
    }

    pub fn activate_pane(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        if !self.panes.contains_key(pane) {
            return Err(LayoutError::MissingPane);
        }
        if !self.visible(pane) {
            return Err(LayoutError::HiddenPane);
        }
        self.active = pane;
        self.pane_history.retain(|id| *id != pane);
        self.pane_history.push(pane);
        Ok(())
    }

    /// Opening an existing target tab activates it without reordering it.
    /// Hidden dock panes accept background opens, but cannot take focus.
    pub fn open_tab(
        &mut self,
        pane: PaneId,
        tab: TabId,
        activate: bool,
    ) -> Result<(), LayoutError> {
        self.require_pane(pane)?;
        if activate && !self.visible(pane) {
            return Err(LayoutError::HiddenPane);
        }
        self.panes[pane].insert(tab, usize::MAX, activate);
        if activate {
            self.activate_pane(pane)?;
        }
        Ok(())
    }

    pub fn activate_tab(&mut self, pane: PaneId, tab: TabId) -> Result<(), LayoutError> {
        self.require_tab(pane, tab)?;
        self.activate_pane(pane)?;
        self.panes[pane].activate(tab);
        Ok(())
    }

    /// Index is in the final tab order, clamped to the last position.
    pub fn reorder_tab(
        &mut self,
        pane: PaneId,
        tab: TabId,
        index: usize,
    ) -> Result<(), LayoutError> {
        self.require_tab(pane, tab)?;
        let tabs = &mut self.panes[pane].tabs;
        tabs.retain(|item| *item != tab);
        tabs.insert(index.min(tabs.len()), tab);
        Ok(())
    }

    /// Explicit policy for newly loaded buffers: assign only globally unplaced
    /// tabs. Existing placements (including hidden dock tabs) are left alone.
    pub fn assign_unplaced_tabs(
        &mut self,
        target: PaneId,
        tabs: impl IntoIterator<Item = TabId>,
    ) -> Result<(), LayoutError> {
        self.require_pane(target)?;
        for tab in tabs {
            if !self.panes.values().any(|pane| pane.tabs.contains(&tab)) {
                self.panes[target].insert(tab, usize::MAX, false);
            }
        }
        Ok(())
    }

    /// Move, dedup at the target, activate, then collapse an empty source.
    /// A duplicate retains its target position; same-pane transfer is reorder.
    pub fn transfer_tab(
        &mut self,
        source: PaneId,
        tab: TabId,
        target: PaneId,
        index: usize,
    ) -> Result<(), LayoutError> {
        self.require_tab(source, tab)?;
        self.require_pane(target)?;
        if !self.visible(target) {
            return Err(LayoutError::HiddenPane);
        }
        if source == target {
            self.reorder_tab(target, tab, index)?;
        } else {
            self.panes[source].remove(tab, TabSuccessor::MostRecent);
            self.panes[target].insert(tab, index, true);
            self.collapse_empty(source);
        }
        self.activate_tab(target, tab)
    }

    pub fn split_pane(
        &mut self,
        target: PaneId,
        edge: Edge,
        mode: SplitMode,
        tabs: Vec<TabId>,
    ) -> Result<PaneId, LayoutError> {
        self.require_pane(target)?;
        if !self.visible(target) {
            return Err(LayoutError::HiddenPane);
        }
        let new = self.panes.insert(Pane::new(tabs));
        let parent = self.parent(Node::Pane(target));
        if let Some((id, index)) = parent
            .filter(|(id, _)| mode == SplitMode::Sibling && self.splits[*id].axis == edge.axis())
        {
            let children = &mut self.splits[id].children;
            children.insert(
                index + usize::from(!edge.before()),
                WeightedChild {
                    node: Node::Pane(new),
                    weight: 1.0,
                },
            );
            for child in children {
                child.weight = 1.0;
            }
        } else {
            let mut children = vec![
                WeightedChild {
                    node: Node::Pane(target),
                    weight: 1.0,
                },
                WeightedChild {
                    node: Node::Pane(new),
                    weight: 1.0,
                },
            ];
            if edge.before() {
                children.reverse();
            }
            let split = self.splits.insert(Split {
                axis: edge.axis(),
                children,
            });
            self.replace(Node::Pane(target), Node::Split(split), parent);
        }
        self.activate_pane(new)?;
        Ok(new)
    }

    /// Edge drops are always target-only. Validation precedes any mutation.
    pub fn split_tab(
        &mut self,
        source: PaneId,
        tab: TabId,
        target: PaneId,
        edge: Edge,
    ) -> Result<PaneId, LayoutError> {
        self.require_tab(source, tab)?;
        if source == target && self.panes[source].tabs.len() == 1 {
            return Ok(source);
        }
        let new = self.split_pane(target, edge, SplitMode::TargetOnly, Vec::new())?;
        self.transfer_tab(source, tab, new, 0)?;
        Ok(new)
    }

    pub fn close_tab(&mut self, pane: PaneId, tab: TabId) -> Result<(), LayoutError> {
        self.close_tab_with_policy(pane, tab, TabSuccessor::MostRecent)
    }

    pub fn close_tab_with_policy(
        &mut self,
        pane: PaneId,
        tab: TabId,
        successor: TabSuccessor,
    ) -> Result<(), LayoutError> {
        self.require_tab(pane, tab)?;
        self.panes[pane].remove(tab, successor);
        self.collapse_empty(pane);
        self.repair_focus();
        Ok(())
    }

    /// Merge tabs into `destination`, preserving its order and active tab.
    /// Without a destination choose the next, then previous pane in this region.
    /// Closing the sole pane without a destination is rejected, not destructive.
    /// A sole main/dock pane moved to another region remains as an empty root;
    /// an empty dock root is hidden until reopened.
    pub fn close_pane(
        &mut self,
        source: PaneId,
        destination: Option<PaneId>,
    ) -> Result<PaneId, LayoutError> {
        self.require_pane(source)?;
        let region = self.region_panes(source);
        let index = region.iter().position(|id| *id == source).unwrap();
        let target = destination
            .or_else(|| region.get(index + 1).copied())
            .or_else(|| index.checked_sub(1).map(|index| region[index]))
            .ok_or(LayoutError::LastPane)?;
        self.require_pane(target)?;
        if target == source {
            return Err(LayoutError::SamePane);
        }
        if !self.visible(target) {
            return Err(LayoutError::HiddenPane);
        }
        let tabs = self.panes[source].tabs.clone();
        for tab in tabs {
            self.panes[target].insert(tab, usize::MAX, false);
        }
        self.panes[source].tabs.clear();
        self.panes[source].history.clear();
        self.collapse_empty(source);
        self.activate_pane(target)?;
        Ok(target)
    }

    /// Keep only this pane in its region, recovering all other tabs in traversal
    /// order. Main and dock are separate regions; neither destroys the other.
    pub fn keep_only(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        self.require_pane(pane)?;
        if !self.visible(pane) {
            return Err(LayoutError::HiddenPane);
        }
        for other in self.region_panes(pane) {
            if other != pane {
                self.close_pane(other, Some(pane))?;
            }
        }
        self.activate_pane(pane)
    }

    fn region_panes(&self, pane: PaneId) -> Vec<PaneId> {
        let main = self.main_panes();
        if main.contains(&pane) {
            main
        } else {
            self.leaves(self.dock.as_ref().unwrap().root)
        }
    }

    fn require_pane(&self, pane: PaneId) -> Result<(), LayoutError> {
        if self.panes.contains_key(pane) {
            Ok(())
        } else {
            Err(LayoutError::MissingPane)
        }
    }

    fn require_tab(&self, pane: PaneId, tab: TabId) -> Result<(), LayoutError> {
        self.require_pane(pane)?;
        if self.panes[pane].tabs.contains(&tab) {
            Ok(())
        } else {
            Err(LayoutError::MissingTab)
        }
    }

    fn replace(&mut self, old: Node, new: Node, parent: Option<(SplitId, usize)>) {
        if let Some((id, index)) = parent {
            self.splits[id].children[index].node = new;
        } else if self.root == old {
            self.root = new;
        } else {
            self.dock.as_mut().unwrap().root = new;
        }
    }

    fn collapse_empty(&mut self, pane: PaneId) {
        if !self.panes[pane].tabs.is_empty() {
            return;
        }
        let Some((parent, index)) = self.parent(Node::Pane(pane)) else {
            if self
                .dock
                .as_ref()
                .is_some_and(|dock| dock.root == Node::Pane(pane))
            {
                self.set_dock_visible(false);
            }
            return;
        };
        self.panes.remove(pane);
        self.pane_history.retain(|id| *id != pane);
        self.splits[parent].children.remove(index);
        if self.splits[parent].children.len() == 1 {
            let remaining = self.splits[parent].children[0].node;
            let grandparent = self.parent(Node::Split(parent));
            self.replace(Node::Split(parent), remaining, grandparent);
            self.splits.remove(parent);
        }
        self.repair_focus();
    }

    fn repair_focus(&mut self) {
        if !self.visible(self.active) {
            let pane = self
                .pane_history
                .iter()
                .rev()
                .copied()
                .find(|id| self.visible(*id))
                .unwrap_or_else(|| self.main_panes()[0]);
            self.activate_pane(pane).unwrap();
        }
    }

    /// Lazily allocate a hidden dock. Repeated calls retain topology and size.
    pub fn ensure_dock(&mut self, size: f32) -> Result<PaneId, LayoutError> {
        valid_weight(size)?;
        if let Some(dock) = &self.dock {
            return Ok(self.leaves(dock.root)[0]);
        }
        let pane = self.panes.insert(Pane::new(Vec::new()));
        self.dock = Some(Dock {
            root: Node::Pane(pane),
            visible: false,
            size,
        });
        Ok(pane)
    }

    pub fn set_dock_visible(&mut self, visible: bool) {
        if let Some(dock) = &mut self.dock {
            dock.visible = visible;
        }
        self.repair_focus();
    }

    pub fn set_dock_size(&mut self, size: f32) -> Result<(), LayoutError> {
        valid_weight(size)?;
        let dock = self.dock.as_mut().ok_or(LayoutError::MissingPane)?;
        dock.size = size;
        Ok(())
    }

    pub fn set_weights(&mut self, split: SplitId, weights: &[f32]) -> Result<(), LayoutError> {
        let split = self
            .splits
            .get_mut(split)
            .ok_or(LayoutError::MissingSplit)?;
        if weights.len() != split.children.len() {
            return Err(LayoutError::InvalidWeight);
        }
        for weight in weights {
            valid_weight(*weight)?;
        }
        for (child, weight) in split.children.iter_mut().zip(weights) {
            child.weight = *weight;
        }
        Ok(())
    }

    /// Resize a divider between adjacent children using their actual pixel
    /// allocations. Minimum is per leaf, identical to `layout`, propagated through
    /// subtrees. An undersized container retains proportionally shrunk minima.
    /// Returns the applied delta, which can be smaller than the request.
    pub fn resize_divider(
        &mut self,
        split: SplitId,
        before: usize,
        delta: f32,
        extent: f32,
        minimum: PaneSize,
    ) -> Result<f32, LayoutError> {
        if !delta.is_finite()
            || !extent.is_finite()
            || extent <= 0.0
            || !minimum.width.is_finite()
            || !minimum.height.is_finite()
            || minimum.width < 0.0
            || minimum.height < 0.0
        {
            return Err(LayoutError::InvalidGeometry);
        }
        let container = self.splits.get(split).ok_or(LayoutError::MissingSplit)?;
        if before >= container.children.len() - 1 {
            return Err(LayoutError::InvalidTopology);
        }
        let minima: Vec<_> = container
            .children
            .iter()
            .map(|child| {
                let (width, height) = self.minimum(child.node, minimum);
                if container.axis == Axis::Horizontal {
                    width
                } else {
                    height
                }
            })
            .collect();
        let sizes = allocate(&container.children, extent as f64, &minima);
        let pair = sizes[before] + sizes[before + 1];
        let left = if minima.iter().sum::<f64>() >= extent as f64 {
            sizes[before]
        } else {
            let lower = minima[before];
            let upper = (pair - minima[before + 1]).max(lower);
            (sizes[before] + delta as f64).clamp(lower, upper)
        };
        let split = &mut self.splits[split];
        for (child, size) in split.children.iter_mut().zip(&sizes) {
            child.weight = (*size / extent as f64).max(f32::from_bits(1) as f64) as f32;
        }
        split.children[before].weight = (left / extent as f64).max(f32::from_bits(1) as f64) as f32;
        split.children[before + 1].weight =
            ((pair - left) / extent as f64).max(f32::from_bits(1) as f64) as f32;
        Ok((left - sizes[before]) as f32)
    }

    /// Logical-pixel geometry in explicit traversal order. Hidden docks produce
    /// no rectangles. Minima propagate through mixed-axis subtrees; when the
    /// viewport is too small, minima shrink proportionally rather than overflow.
    pub fn layout(
        &self,
        bounds: Rect,
        minimum: PaneSize,
    ) -> Result<Vec<PaneGeometry>, LayoutError> {
        if ![
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            minimum.width,
            minimum.height,
            bounds.x + bounds.width,
            bounds.y + bounds.height,
        ]
        .iter()
        .all(|v| v.is_finite())
            || bounds.width < 0.0
            || bounds.height < 0.0
            || minimum.width < 0.0
            || minimum.height < 0.0
        {
            return Err(LayoutError::InvalidGeometry);
        }
        let mut main = bounds;
        let mut result = Vec::new();
        let dock_bounds = self.dock.as_ref().filter(|dock| dock.visible).map(|dock| {
            let main_min = self.minimum(self.root, minimum).1;
            let dock_min = self.minimum(dock.root, minimum).1;
            let height = bounds.height as f64;
            let size = if main_min + dock_min > height {
                height * dock_min / (main_min + dock_min)
            } else {
                (dock.size as f64).clamp(dock_min, height - main_min)
            } as f32;
            main.height = (bounds.height - size).max(0.0);
            (
                dock.root,
                Rect {
                    y: bounds.y + main.height,
                    height: size,
                    ..bounds
                },
            )
        });
        self.layout_node(self.root, main, minimum, &mut result);
        if let Some((root, bounds)) = dock_bounds {
            self.layout_node(root, bounds, minimum, &mut result);
        }
        Ok(result)
    }

    fn minimum(&self, node: Node, minimum: PaneSize) -> (f64, f64) {
        match node {
            Node::Pane(_) => (minimum.width as f64, minimum.height as f64),
            Node::Split(id) => self.splits[id]
                .children
                .iter()
                .fold((0.0, 0.0), |(w, h), child| {
                    let (cw, ch) = self.minimum(child.node, minimum);
                    match self.splits[id].axis {
                        Axis::Horizontal => (w + cw, h.max(ch)),
                        Axis::Vertical => (w.max(cw), h + ch),
                    }
                }),
        }
    }

    fn layout_node(
        &self,
        node: Node,
        bounds: Rect,
        minimum: PaneSize,
        result: &mut Vec<PaneGeometry>,
    ) {
        match node {
            Node::Pane(pane) => result.push(PaneGeometry { pane, rect: bounds }),
            Node::Split(id) => {
                let split = &self.splits[id];
                let horizontal = split.axis == Axis::Horizontal;
                let extent = if horizontal {
                    bounds.width
                } else {
                    bounds.height
                };
                let minima: Vec<_> = split
                    .children
                    .iter()
                    .map(|child| {
                        let (w, h) = self.minimum(child.node, minimum);
                        if horizontal { w } else { h }
                    })
                    .collect();
                let sizes = allocate(&split.children, extent as f64, &minima);
                let mut offset = 0.0f64;
                for (index, child) in split.children.iter().enumerate() {
                    let end = if index + 1 == sizes.len() {
                        extent
                    } else {
                        (offset + sizes[index]) as f32
                    };
                    let start = offset as f32;
                    let rect = if horizontal {
                        Rect {
                            x: bounds.x + start,
                            width: (end - start).max(0.0),
                            ..bounds
                        }
                    } else {
                        Rect {
                            y: bounds.y + start,
                            height: (end - start).max(0.0),
                            ..bounds
                        }
                    };
                    self.layout_node(child.node, rect, minimum, result);
                    offset += sizes[index];
                }
            }
        }
    }

    /// Helix-compatible navigation: ascend to the next sibling in the requested
    /// axis, then descend by nearest top/left origin in each container's axis.
    /// Ties use child order. A visible dock behaves as a bottom region; no wrap.
    pub fn neighbor(
        &self,
        pane: PaneId,
        direction: Edge,
        bounds: Rect,
        minimum: PaneSize,
    ) -> Result<Option<PaneId>, LayoutError> {
        self.require_pane(pane)?;
        let geometry = self.layout(bounds, minimum)?;
        let source = geometry
            .iter()
            .find(|entry| entry.pane == pane)
            .ok_or(LayoutError::HiddenPane)?
            .rect;
        let mut node = Node::Pane(pane);
        let destination = loop {
            if let Some((parent, index)) = self.parent(node) {
                let split = &self.splits[parent];
                if split.axis == direction.axis() {
                    let next = if direction.before() {
                        index.checked_sub(1)
                    } else {
                        Some(index + 1)
                    };
                    if let Some(child) = next.and_then(|i| split.children.get(i)) {
                        break child.node;
                    }
                }
                node = Node::Split(parent);
            } else {
                let destination =
                    self.dock.as_ref().filter(|dock| dock.visible).and_then(
                        |dock| match direction {
                            Edge::Bottom if node == self.root => Some(dock.root),
                            Edge::Top if node == dock.root => Some(self.root),
                            _ => None,
                        },
                    );
                let Some(destination) = destination else {
                    return Ok(None);
                };
                break destination;
            }
        };
        let mut node = destination;
        while let Node::Split(id) = node {
            let split = &self.splits[id];
            node = split
                .children
                .iter()
                .min_by(|a, b| {
                    let distance = |child: &WeightedChild| {
                        let first = self.leaves(child.node)[0];
                        let origin = geometry
                            .iter()
                            .find(|entry| entry.pane == first)
                            .unwrap()
                            .rect;
                        match split.axis {
                            Axis::Horizontal => (source.x as f64 - origin.x as f64).abs(),
                            Axis::Vertical => (source.y as f64 - origin.y as f64).abs(),
                        }
                    };
                    distance(a).total_cmp(&distance(b))
                })
                .unwrap()
                .node;
        }
        let Node::Pane(pane) = node else {
            unreachable!()
        };
        Ok(Some(pane))
    }

    /// Swap leaf positions, not tab contents or identities; focus follows identity.
    pub fn swap_panes(&mut self, first: PaneId, second: PaneId) -> Result<(), LayoutError> {
        self.require_pane(first)?;
        self.require_pane(second)?;
        if first == second {
            return Ok(());
        }
        let first_parent = self.parent(Node::Pane(first));
        let second_parent = self.parent(Node::Pane(second));
        let first_main = self.root == Node::Pane(first);
        let second_main = self.root == Node::Pane(second);
        for (parent, main, replacement) in [
            (first_parent, first_main, second),
            (second_parent, second_main, first),
        ] {
            if let Some((id, index)) = parent {
                self.splits[id].children[index].node = Node::Pane(replacement);
            } else if main {
                self.root = Node::Pane(replacement);
            } else {
                self.dock.as_mut().unwrap().root = Node::Pane(replacement);
            }
        }
        self.repair_focus();
        Ok(())
    }

    pub fn swap_direction(
        &mut self,
        pane: PaneId,
        direction: Edge,
        bounds: Rect,
        minimum: PaneSize,
    ) -> Result<Option<PaneId>, LayoutError> {
        let neighbor = self.neighbor(pane, direction, bounds, minimum)?;
        if let Some(other) = neighbor {
            self.swap_panes(pane, other)?;
        }
        Ok(neighbor)
    }

    /// Toggle only this split, or this pane's immediate parent, like Helix's
    /// transpose-view command. Descendant axes, IDs, weights and order survive.
    pub fn transpose(&mut self, node: Node) -> Result<(), LayoutError> {
        match node {
            Node::Pane(id) => {
                self.require_pane(id)?;
                if let Some((parent, _)) = self.parent(node) {
                    self.transpose(Node::Split(parent))?;
                }
                Ok(())
            }
            Node::Split(id) => {
                let split = self.splits.get_mut(id).ok_or(LayoutError::MissingSplit)?;
                split.axis = match split.axis {
                    Axis::Horizontal => Axis::Vertical,
                    Axis::Vertical => Axis::Horizontal,
                };
                Ok(())
            }
        }
    }

    /// Check ownership, reachability, generational references, unique local tabs,
    /// active/history membership, finite weights and visible focus.
    pub fn validate(&self) -> Result<(), LayoutError> {
        fn visit(
            layout: &PaneLayout,
            node: Node,
            seen: &mut HashSet<Node>,
        ) -> Result<(), LayoutError> {
            if !seen.insert(node) {
                return Err(LayoutError::InvalidTopology);
            }
            match node {
                Node::Pane(id) => {
                    let pane = layout.panes.get(id).ok_or(LayoutError::MissingPane)?;
                    let tabs: HashSet<_> = pane.tabs.iter().copied().collect();
                    let history: HashSet<_> = pane.history.iter().copied().collect();
                    if tabs.len() != pane.tabs.len()
                        || history.len() != pane.history.len()
                        || tabs != history
                    {
                        return Err(LayoutError::InvalidTopology);
                    }
                }
                Node::Split(id) => {
                    let split = layout.splits.get(id).ok_or(LayoutError::MissingSplit)?;
                    if split.children.len() < 2 {
                        return Err(LayoutError::InvalidTopology);
                    }
                    for child in &split.children {
                        valid_weight(child.weight)?;
                        visit(layout, child.node, seen)?;
                    }
                }
            }
            Ok(())
        }
        let mut seen = HashSet::new();
        visit(self, self.root, &mut seen)?;
        if let Some(dock) = &self.dock {
            valid_weight(dock.size)?;
            visit(self, dock.root, &mut seen)?;
        }
        if seen.len() != self.panes.len() + self.splits.len()
            || !self.visible(self.active)
            || self.pane_history.last() != Some(&self.active)
            || self
                .pane_history
                .iter()
                .any(|id| !self.panes.contains_key(*id))
            || self
                .pane_history
                .iter()
                .copied()
                .collect::<HashSet<_>>()
                .len()
                != self.pane_history.len()
        {
            return Err(LayoutError::InvalidTopology);
        }
        Ok(())
    }
}

fn valid_weight(weight: f32) -> Result<(), LayoutError> {
    if weight.is_finite() && weight > 0.0 {
        Ok(())
    } else {
        Err(LayoutError::InvalidWeight)
    }
}

/// Weighted water filling with per-subtree minima, calculated in f64 to avoid
/// overflowing sums or losing tiny positive weights supplied through f32 APIs.
fn allocate(children: &[WeightedChild], extent: f64, minima: &[f64]) -> Vec<f64> {
    let total_min: f64 = minima.iter().sum();
    if total_min >= extent && total_min > 0.0 {
        return minima
            .iter()
            .map(|minimum| extent * minimum / total_min)
            .collect();
    }
    let mut sizes = vec![0.0; children.len()];
    let mut pending: Vec<_> = (0..children.len()).collect();
    let mut remaining = extent;
    while !pending.is_empty() {
        let weight: f64 = pending.iter().map(|i| children[*i].weight as f64).sum();
        let constrained: Vec<_> = pending
            .iter()
            .copied()
            .filter(|i| remaining * children[*i].weight as f64 / weight < minima[*i])
            .collect();
        if constrained.is_empty() {
            for index in pending {
                sizes[index] = remaining * children[index].weight as f64 / weight;
            }
            break;
        }
        for index in &constrained {
            sizes[*index] = minima[*index];
            remaining -= minima[*index];
        }
        pending.retain(|index| !constrained.contains(index));
    }
    sizes
}

#[cfg(test)]
#[path = "pane_layout_tests.rs"]
mod tests;
