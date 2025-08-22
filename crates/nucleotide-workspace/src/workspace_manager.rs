// ABOUTME: Main workspace manager that coordinates between editor state and UI
// ABOUTME: Uses capability traits to avoid circular dependencies

use gpui::Result;
use nucleotide_core::{
    DocumentEvent, EditorState, EventBus, EventHandler, WorkspaceEvent,
    command_system::SplitDirection,
};
use nucleotide_events::v2::workspace::{LayoutType, PanelConfiguration, PanelType, TabId};
use nucleotide_logging::{debug, info};
use std::sync::{Arc, RwLock};

/// Workspace manager that coordinates UI without depending on concrete Application
pub struct WorkspaceManager<S: EditorState> {
    /// Editor state provided through capability trait
    editor_state: Arc<RwLock<S>>,

    /// Event bus for communication
    event_bus: Arc<dyn EventBus>,

    /// Current layout
    layout: crate::layout::Layout,

    /// Tab manager
    tabs: crate::tab_manager::TabManager,

    /// Focus state
    focused_tab: Option<String>,
}

impl<S: EditorState + 'static> WorkspaceManager<S> {
    /// Create a new workspace manager
    pub fn new(editor_state: Arc<RwLock<S>>, event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            editor_state,
            event_bus,
            layout: crate::layout::Layout::new(),
            tabs: crate::tab_manager::TabManager::new(),
            focused_tab: None,
        }
    }

    /// Open a file in the workspace
    pub fn open_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        // Use capability trait to open document (acquire read lock)
        let doc_id = {
            let state = self.editor_state.read().unwrap();
            state.open_document(path)?
        };

        // Create a tab for it - use debug format since DocumentId fields are private
        let tab_id = format!("doc_{doc_id:?}");
        let tab = crate::tab_manager::Tab::new(tab_id.clone(), path.display().to_string());
        self.tabs.add_tab(tab);

        // Emit workspace event
        // Note: Convert DocumentId to TabId using a hash-based approach since DocumentId internals are private
        let tab_numeric_id = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            doc_id.hash(&mut hasher);
            hasher.finish() as usize
        };

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::TabCreated {
                tab_id: TabId::new(tab_numeric_id),
                doc_id,
                title: path.display().to_string(),
            });

        // Focus the new tab
        self.focus_tab(&tab_id);

        Ok(())
    }

    /// Close a tab
    pub fn close_tab(&mut self, tab_id: &str) -> Result<(), String> {
        self.tabs.remove_tab(tab_id);

        // Note: We need doc_id for the event but don't have it here in the current architecture
        // For now, use a default DocumentId - in practice this should be tracked properly
        let doc_id = helix_view::DocumentId::default();

        // Use hash-based approach for tab_id conversion
        let tab_numeric_id = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            tab_id.hash(&mut hasher);
            hasher.finish() as usize
        };

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::TabClosed {
                tab_id: TabId::new(tab_numeric_id),
                doc_id,
            });

        // Focus next available tab
        let next_tab_id = self.tabs.get_tabs().first().map(|t| t.id.clone());
        if let Some(id) = next_tab_id {
            self.focus_tab(&id);
        } else {
            self.focused_tab = None;
        }

        Ok(())
    }

    /// Focus a tab
    pub fn focus_tab(&mut self, tab_id: &str) {
        let previous_tab = self.focused_tab.clone();
        self.focused_tab = Some(tab_id.to_string());
        self.tabs.set_active(tab_id);

        // Use hash-based approach for tab_id conversion
        let tab_numeric_id = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            tab_id.hash(&mut hasher);
            hasher.finish() as usize
        };

        let previous_tab_id = previous_tab.as_ref().map(|prev| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            use std::hash::{Hash, Hasher};
            prev.hash(&mut hasher);
            TabId::new(hasher.finish() as usize)
        });

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::TabSwitched {
                previous_tab: previous_tab_id,
                new_tab: TabId::new(tab_numeric_id),
            });
    }

    /// Split the view
    pub fn split(&mut self, direction: crate::layout::LayoutDirection) {
        self.layout.split(direction);

        let split_dir = match direction {
            crate::layout::LayoutDirection::Horizontal => SplitDirection::Horizontal,
            crate::layout::LayoutDirection::Vertical => SplitDirection::Vertical,
        };

        // Note: SplitCreated event doesn't exist in V2, using LayoutChanged instead
        let layout_type = match split_dir {
            SplitDirection::Horizontal => LayoutType::Horizontal,
            SplitDirection::Vertical => LayoutType::Vertical,
        };

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::LayoutChanged {
                layout_type,
                panel_configuration: PanelConfiguration {
                    file_tree_width: None,
                    sidebar_panels: Vec::new(),
                    bottom_panels: Vec::new(),
                },
            });
    }

    /// Toggle a panel
    pub fn toggle_panel(&mut self, panel: crate::layout::Panel) {
        self.layout.toggle_panel(panel);

        let panel_type = match panel {
            crate::layout::Panel::FileTree => PanelType::FileTree,
            crate::layout::Panel::Terminal => PanelType::Terminal,
            crate::layout::Panel::Search => PanelType::Search,
            crate::layout::Panel::Diagnostics => PanelType::Problems, // Diagnostics -> Problems
        };

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::PanelToggled {
                panel_type,
                is_visible: self.layout.is_panel_visible(panel),
            });
    }

    /// Get current theme (returns clone due to RwLock)
    pub fn theme(&self) -> helix_view::Theme {
        let state = self.editor_state.read().unwrap();
        state.current_theme()
    }

    /// Execute a command
    pub fn execute_command(&self, command: &str, args: Vec<String>) -> Result<(), String> {
        let state = self.editor_state.read().unwrap();
        state.execute_command(command, args)
    }
}

impl<S: EditorState> EventHandler for WorkspaceManager<S> {
    fn handle_document(&mut self, event: &DocumentEvent) {
        use nucleotide_core::DocumentEvent as Event;
        match event {
            Event::Opened { doc_id, .. } => {
                // Handle document opened
                info!(doc_id = ?doc_id, "Document opened");
            }
            Event::Closed { doc_id, .. } => {
                // Handle document closed
                info!(doc_id = ?doc_id, "Document closed");
            }
            Event::ContentChanged { doc_id, .. } => {
                // Handle document content change
                debug!(doc_id = ?doc_id, "Document content changed");
                debug!("Redraw requested");
            }
            _ => {}
        }
    }
}
