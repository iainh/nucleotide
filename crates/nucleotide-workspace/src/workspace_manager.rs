// ABOUTME: Main workspace manager that coordinates between editor state and UI
// ABOUTME: Uses capability traits to avoid circular dependencies

use gpui::Result;
use nucleotide_core::{CoreEvent, EditorState, EventBus, EventHandler, WorkspaceEvent};
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
        self.event_bus
            .dispatch_workspace(WorkspaceEvent::TabOpened { id: tab_id.clone() });

        // Focus the new tab
        self.focus_tab(&tab_id);

        Ok(())
    }

    /// Close a tab
    pub fn close_tab(&mut self, tab_id: &str) -> Result<(), String> {
        self.tabs.remove_tab(tab_id);

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::TabClosed {
                id: tab_id.to_string(),
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
        self.focused_tab = Some(tab_id.to_string());
        self.tabs.set_active(tab_id);

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::TabSwitched {
                id: tab_id.to_string(),
            });
    }

    /// Split the view
    pub fn split(&mut self, direction: crate::layout::LayoutDirection) {
        self.layout.split(direction);

        let split_dir = match direction {
            crate::layout::LayoutDirection::Horizontal => {
                nucleotide_core::SplitDirection::Horizontal
            }
            crate::layout::LayoutDirection::Vertical => nucleotide_core::SplitDirection::Vertical,
        };

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::SplitCreated {
                direction: split_dir,
            });
    }

    /// Toggle a panel
    pub fn toggle_panel(&mut self, panel: crate::layout::Panel) {
        self.layout.toggle_panel(panel);

        let panel_type = match panel {
            crate::layout::Panel::FileTree => nucleotide_core::PanelType::FileTree,
            crate::layout::Panel::Terminal => nucleotide_core::PanelType::Terminal,
            crate::layout::Panel::Search => nucleotide_core::PanelType::Search,
            crate::layout::Panel::Diagnostics => nucleotide_core::PanelType::Diagnostics,
        };

        self.event_bus
            .dispatch_workspace(WorkspaceEvent::PanelToggled { panel: panel_type });
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
    fn handle_core(&mut self, event: &CoreEvent) {
        match event {
            CoreEvent::DocumentOpened { doc_id } => {
                // Handle document opened
                info!(doc_id = ?doc_id, "Document opened");
            }
            CoreEvent::DocumentClosed { doc_id } => {
                // Handle document closed
                info!(doc_id = ?doc_id, "Document closed");
            }
            CoreEvent::RedrawRequested => {
                // Request UI redraw
                debug!("Redraw requested");
            }
            _ => {}
        }
    }
}
