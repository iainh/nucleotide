// ABOUTME: Tab management for workspace with support for multiple documents
// ABOUTME: Handles tab lifecycle without depending on concrete document types

/// A tab in the workspace
#[derive(Debug, Clone)]
pub struct Tab {
    /// Unique tab identifier
    pub id: String,

    /// Tab title
    pub title: String,

    /// Whether the tab has unsaved changes
    pub dirty: bool,

    /// Whether this is a preview tab
    pub preview: bool,
}

impl Tab {
    /// Create a new tab
    pub fn new(id: String, title: String) -> Self {
        Self {
            id,
            title,
            dirty: false,
            preview: false,
        }
    }

    /// Create a preview tab
    pub fn preview(id: String, title: String) -> Self {
        Self {
            id,
            title,
            dirty: false,
            preview: true,
        }
    }

    /// Mark the tab as dirty
    pub fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
    }

    /// Convert preview to permanent tab
    pub fn make_permanent(&mut self) {
        self.preview = false;
    }
}

/// Tab manager
pub struct TabManager {
    /// All tabs
    tabs: Vec<Tab>,

    /// Active tab index
    active_tab: Option<usize>,

    /// Tab history for navigation
    history: Vec<String>,
}

impl TabManager {
    /// Create a new tab manager
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active_tab: None,
            history: Vec::new(),
        }
    }

    /// Add a tab
    pub fn add_tab(&mut self, tab: Tab) {
        let id = tab.id.clone();
        self.tabs.push(tab);
        self.set_active(&id);
    }

    /// Remove a tab by ID
    pub fn remove_tab(&mut self, id: &str) -> Option<Tab> {
        if let Some(index) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(index);

            // Update active tab
            if let Some(active) = self.active_tab {
                if active == index {
                    self.active_tab = if !self.tabs.is_empty() {
                        Some(index.min(self.tabs.len() - 1))
                    } else {
                        None
                    };
                } else if active > index {
                    self.active_tab = Some(active - 1);
                }
            }

            // Remove from history
            self.history.retain(|h| h != id);

            Some(tab)
        } else {
            None
        }
    }

    /// Get all tabs
    pub fn get_tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Get a tab by ID
    pub fn get_tab(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// Get mutable tab by ID
    pub fn get_tab_mut(&mut self, id: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Set the active tab
    pub fn set_active(&mut self, id: &str) {
        if let Some(index) = self.tabs.iter().position(|t| t.id == id) {
            self.active_tab = Some(index);

            // Update history
            self.history.retain(|h| h != id);
            self.history.push(id.to_string());

            // Limit history size
            if self.history.len() > 20 {
                self.history.remove(0);
            }
        }
    }

    /// Get the active tab
    pub fn active_tab(&self) -> Option<&Tab> {
        self.active_tab.and_then(|i| self.tabs.get(i))
    }

    /// Navigate to previous tab in history
    pub fn navigate_back(&mut self) -> Option<String> {
        if self.history.len() > 1 {
            self.history.pop(); // Remove current
            self.history.last().cloned()
        } else {
            None
        }
    }

    /// Navigate to next tab
    pub fn next_tab(&mut self) -> Option<String> {
        if let Some(active) = self.active_tab {
            let next = (active + 1) % self.tabs.len();
            Some(self.tabs[next].id.clone())
        } else {
            None
        }
    }

    /// Navigate to previous tab
    pub fn prev_tab(&mut self) -> Option<String> {
        if let Some(active) = self.active_tab {
            let prev = if active == 0 {
                self.tabs.len() - 1
            } else {
                active - 1
            };
            Some(self.tabs[prev].id.clone())
        } else {
            None
        }
    }
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}
