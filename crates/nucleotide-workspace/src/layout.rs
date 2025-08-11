// ABOUTME: Layout management for workspace panes and panels
// ABOUTME: Handles split views and panel visibility

use std::collections::HashMap;

/// Layout direction for splits
#[derive(Debug, Clone, Copy)]
pub enum LayoutDirection {
    Horizontal,
    Vertical,
}

/// Panel types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    FileTree,
    Terminal,
    Search,
    Diagnostics,
}

/// Layout configuration
pub struct Layout {
    /// Current splits
    splits: Vec<Split>,

    /// Panel visibility states
    panel_states: HashMap<Panel, bool>,

    /// Active split index
    active_split: usize,
}

/// A split in the layout
#[derive(Debug, Clone)]
pub struct Split {
    /// Split direction
    direction: Option<LayoutDirection>,

    /// Split ratio (0.0 to 1.0)
    ratio: f32,

    /// Child splits (if any)
    children: Vec<Split>,
}

impl Layout {
    /// Create a new layout
    pub fn new() -> Self {
        let mut panel_states = HashMap::new();
        panel_states.insert(Panel::FileTree, false);
        panel_states.insert(Panel::Terminal, false);
        panel_states.insert(Panel::Search, false);
        panel_states.insert(Panel::Diagnostics, false);

        Self {
            splits: vec![Split::new()],
            panel_states,
            active_split: 0,
        }
    }

    /// Split the current active pane
    pub fn split(&mut self, direction: LayoutDirection) {
        if let Some(split) = self.splits.get_mut(self.active_split) {
            split.split(direction);
        }
    }

    /// Toggle a panel
    pub fn toggle_panel(&mut self, panel: Panel) {
        let state = self.panel_states.entry(panel).or_insert(false);
        *state = !*state;
    }

    /// Check if a panel is visible
    pub fn is_panel_visible(&self, panel: Panel) -> bool {
        *self.panel_states.get(&panel).unwrap_or(&false)
    }

    /// Get the active split index
    pub fn active_split(&self) -> usize {
        self.active_split
    }

    /// Set the active split
    pub fn set_active_split(&mut self, index: usize) {
        if index < self.splits.len() {
            self.active_split = index;
        }
    }
}

impl Default for Split {
    fn default() -> Self {
        Self::new()
    }
}

impl Split {
    /// Create a new split
    pub fn new() -> Self {
        Self {
            direction: None,
            ratio: 1.0,
            children: Vec::new(),
        }
    }

    /// Split this pane
    pub fn split(&mut self, direction: LayoutDirection) {
        if self.children.is_empty() {
            // First split
            self.direction = Some(direction);
            self.children = vec![Split::new(), Split::new()];
            self.ratio = 0.5;
        } else {
            // Add another split
            self.children.push(Split::new());
        }
    }

    /// Get the split ratio
    pub fn ratio(&self) -> f32 {
        self.ratio
    }

    /// Set the split ratio
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio.clamp(0.1, 0.9);
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}
