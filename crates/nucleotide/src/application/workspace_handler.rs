// ABOUTME: Workspace domain event handler for file operations and project management
// ABOUTME: Processes V2 workspace events and maintains workspace state

use helix_view::DocumentId;
use nucleotide_events::v2::handler::EventHandler;
use nucleotide_events::v2::workspace::{
    Event, FileOpenSource, LayoutType, PanelConfiguration, PanelType, ProjectType, SelectionSource,
    TabId,
};
use nucleotide_logging::{debug, error, info, instrument, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Workspace event handler for V2 domain events
/// Manages workspace state, file operations, and layout coordination
pub struct WorkspaceHandler {
    /// Current workspace root
    current_workspace: Arc<RwLock<Option<PathBuf>>>,
    /// Project type for current workspace
    project_type: Arc<RwLock<Option<ProjectType>>>,
    /// Active tabs by tab ID
    active_tabs: Arc<RwLock<HashMap<TabId, TabInfo>>>,
    /// Panel visibility state
    panel_state: Arc<RwLock<HashMap<PanelType, bool>>>,
    /// File tree expanded directories
    expanded_directories: Arc<RwLock<Vec<PathBuf>>>,
    /// Current layout configuration
    layout_config: Arc<RwLock<WorkspaceLayout>>,
    /// Initialization state
    initialized: bool,
}

/// Information about an active tab
#[derive(Debug, Clone)]
struct TabInfo {
    doc_id: DocumentId,
    title: String,
    path: Option<PathBuf>,
    is_modified: bool,
}

/// Current workspace layout state
#[derive(Debug, Clone)]
struct WorkspaceLayout {
    layout_type: LayoutType,
    panel_config: PanelConfiguration,
    file_tree_visible: bool,
}

impl WorkspaceHandler {
    /// Create a new workspace handler
    pub fn new() -> Self {
        Self {
            current_workspace: Arc::new(RwLock::new(None)),
            project_type: Arc::new(RwLock::new(None)),
            active_tabs: Arc::new(RwLock::new(HashMap::new())),
            panel_state: Arc::new(RwLock::new(HashMap::new())),
            expanded_directories: Arc::new(RwLock::new(Vec::new())),
            layout_config: Arc::new(RwLock::new(WorkspaceLayout {
                layout_type: LayoutType::Single,
                panel_config: PanelConfiguration::new(),
                file_tree_visible: true,
            })),
            initialized: false,
        }
    }

    /// Initialize the handler
    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.initialized {
            warn!("WorkspaceHandler already initialized");
            return Ok(());
        }

        info!("Initializing WorkspaceHandler for V2 event processing");

        // Initialize default panel visibility
        let default_panels = vec![
            (PanelType::FileTree, true),
            (PanelType::Search, false),
            (PanelType::Problems, false),
            (PanelType::Output, false),
            (PanelType::Terminal, false),
            (PanelType::Extensions, false),
        ];

        tokio::spawn({
            let panel_state = self.panel_state.clone();
            async move {
                let mut state = panel_state.write().await;
                for (panel, visible) in default_panels {
                    state.insert(panel, visible);
                }
            }
        });

        self.initialized = true;
        Ok(())
    }

    /// Get current workspace root
    pub async fn get_current_workspace(&self) -> Option<PathBuf> {
        let workspace = self.current_workspace.read().await;
        workspace.clone()
    }

    /// Get project type for current workspace
    pub async fn get_project_type(&self) -> Option<ProjectType> {
        let project_type = self.project_type.read().await;
        *project_type
    }

    /// Get active tab information
    pub async fn get_tab_info(&self, tab_id: &TabId) -> Option<TabInfo> {
        let tabs = self.active_tabs.read().await;
        tabs.get(tab_id).cloned()
    }

    /// Check if panel is visible
    pub async fn is_panel_visible(&self, panel_type: PanelType) -> bool {
        let panel_state = self.panel_state.read().await;
        panel_state.get(&panel_type).copied().unwrap_or(false)
    }

    /// Get current layout configuration
    pub async fn get_layout_config(&self) -> WorkspaceLayout {
        let layout = self.layout_config.read().await;
        layout.clone()
    }

    /// Check if handler is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Handle project events
    #[instrument(skip(self))]
    async fn handle_project_event(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            Event::ProjectOpened {
                workspace_root,
                project_type,
                file_count,
            } => {
                info!(
                    workspace = %workspace_root.display(),
                    project_type = ?project_type,
                    file_count = file_count,
                    "Project opened"
                );

                let mut workspace = self.current_workspace.write().await;
                *workspace = Some(workspace_root.clone());

                let mut proj_type = self.project_type.write().await;
                *proj_type = *project_type;
            }
            Event::ProjectClosed { workspace_root } => {
                info!(
                    workspace = %workspace_root.display(),
                    "Project closed"
                );

                let mut workspace = self.current_workspace.write().await;
                *workspace = None;

                let mut proj_type = self.project_type.write().await;
                *proj_type = None;

                // Clear tabs and expanded directories
                let mut tabs = self.active_tabs.write().await;
                tabs.clear();

                let mut expanded = self.expanded_directories.write().await;
                expanded.clear();
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle file tree events
    #[instrument(skip(self))]
    async fn handle_file_tree_event(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            Event::FileTreeToggled { is_visible } => {
                debug!(is_visible = is_visible, "File tree visibility changed");

                let mut panel_state = self.panel_state.write().await;
                panel_state.insert(PanelType::FileTree, *is_visible);

                let mut layout = self.layout_config.write().await;
                layout.file_tree_visible = *is_visible;
            }
            Event::FileSelected { path, source } => {
                debug!(
                    path = %path.display(),
                    source = ?source,
                    "File selected in tree"
                );
            }
            Event::DirectoryExpanded { path, child_count } => {
                debug!(
                    path = %path.display(),
                    child_count = child_count,
                    "Directory expanded"
                );

                let mut expanded = self.expanded_directories.write().await;
                if !expanded.contains(path) {
                    expanded.push(path.clone());
                }
            }
            Event::DirectoryCollapsed { path } => {
                debug!(path = %path.display(), "Directory collapsed");

                let mut expanded = self.expanded_directories.write().await;
                expanded.retain(|p| p != path);
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle layout events
    #[instrument(skip(self))]
    async fn handle_layout_event(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            Event::LayoutChanged {
                layout_type,
                panel_configuration,
            } => {
                info!(
                    layout_type = ?layout_type,
                    "Workspace layout changed"
                );

                let mut layout = self.layout_config.write().await;
                layout.layout_type = *layout_type;
                layout.panel_config = panel_configuration.clone();
            }
            Event::PanelToggled {
                panel_type,
                is_visible,
            } => {
                debug!(
                    panel_type = ?panel_type,
                    is_visible = is_visible,
                    "Panel visibility toggled"
                );

                let mut panel_state = self.panel_state.write().await;
                panel_state.insert(*panel_type, *is_visible);
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle tab events
    #[instrument(skip(self))]
    async fn handle_tab_event(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            Event::TabCreated {
                tab_id,
                doc_id,
                title,
            } => {
                info!(
                    tab_id = ?tab_id,
                    doc_id = ?doc_id,
                    title = %title,
                    "Tab created"
                );

                let tab_info = TabInfo {
                    doc_id: *doc_id,
                    title: title.clone(),
                    path: None, // Would be populated from document info
                    is_modified: false,
                };

                let mut tabs = self.active_tabs.write().await;
                tabs.insert(*tab_id, tab_info);
            }
            Event::TabSwitched {
                previous_tab,
                new_tab,
            } => {
                debug!(
                    previous_tab = ?previous_tab,
                    new_tab = ?new_tab,
                    "Tab switched"
                );
            }
            Event::TabClosed { tab_id, doc_id } => {
                info!(
                    tab_id = ?tab_id,
                    doc_id = ?doc_id,
                    "Tab closed"
                );

                let mut tabs = self.active_tabs.write().await;
                tabs.remove(tab_id);
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle file operation events
    #[instrument(skip(self))]
    async fn handle_file_operation(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            Event::FileOpened { path, source } => {
                info!(
                    path = %path.display(),
                    source = ?source,
                    "File opened"
                );
            }
            Event::FileCreated {
                path,
                parent_directory,
            } => {
                info!(
                    path = %path.display(),
                    parent = %parent_directory.display(),
                    "File created"
                );
            }
            Event::FileDeleted {
                path,
                was_directory,
            } => {
                info!(
                    path = %path.display(),
                    was_directory = was_directory,
                    "File deleted"
                );

                // Remove from expanded directories if it was a directory
                if *was_directory {
                    let mut expanded = self.expanded_directories.write().await;
                    expanded.retain(|p| !p.starts_with(path));
                }
            }
            Event::FileRenamed { old_path, new_path } => {
                info!(
                    old_path = %old_path.display(),
                    new_path = %new_path.display(),
                    "File renamed"
                );

                // Update expanded directories if this was a directory rename
                let mut expanded = self.expanded_directories.write().await;
                for expanded_path in expanded.iter_mut() {
                    if expanded_path.starts_with(old_path) {
                        if let Ok(relative) = expanded_path.strip_prefix(old_path) {
                            *expanded_path = new_path.join(relative);
                        }
                    }
                }
            }
            Event::WorkingDirectoryChanged {
                previous_directory,
                new_directory,
            } => {
                info!(
                    previous = ?previous_directory.as_ref().map(|p| p.display()),
                    new = %new_directory.display(),
                    "Working directory changed"
                );
            }
            _ => {}
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EventHandler<Event> for WorkspaceHandler {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    #[instrument(skip(self, event))]
    async fn handle(&mut self, event: Event) -> Result<(), Self::Error> {
        if !self.initialized {
            error!("WorkspaceHandler not initialized");
            return Err("WorkspaceHandler not initialized".into());
        }

        debug!(event_type = ?std::mem::discriminant(&event), "Processing workspace event");

        match event {
            Event::ProjectOpened { .. } | Event::ProjectClosed { .. } => {
                self.handle_project_event(&event).await?;
            }
            Event::FileTreeToggled { .. }
            | Event::FileSelected { .. }
            | Event::DirectoryExpanded { .. }
            | Event::DirectoryCollapsed { .. } => {
                self.handle_file_tree_event(&event).await?;
            }
            Event::LayoutChanged { .. } | Event::PanelToggled { .. } => {
                self.handle_layout_event(&event).await?;
            }
            Event::TabCreated { .. } | Event::TabSwitched { .. } | Event::TabClosed { .. } => {
                self.handle_tab_event(&event).await?;
            }
            Event::FileOpened { .. }
            | Event::FileCreated { .. }
            | Event::FileDeleted { .. }
            | Event::FileRenamed { .. }
            | Event::WorkingDirectoryChanged { .. } => {
                self.handle_file_operation(&event).await?;
            }
        }

        Ok(())
    }
}

impl Default for WorkspaceHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_workspace_handler_initialization() {
        let mut handler = WorkspaceHandler::new();
        assert!(!handler.is_initialized());

        handler.initialize().unwrap();
        assert!(handler.is_initialized());
    }

    #[tokio::test]
    async fn test_project_lifecycle() {
        let mut handler = WorkspaceHandler::new();
        handler.initialize().unwrap();

        let workspace_root = PathBuf::from("/test/project");

        // Test project opening
        let open_event = Event::ProjectOpened {
            workspace_root: workspace_root.clone(),
            project_type: Some(ProjectType::Rust),
            file_count: 42,
        };

        let result = handler.handle(open_event).await;
        assert!(result.is_ok());

        // Verify workspace was set
        let current = handler.get_current_workspace().await;
        assert_eq!(current, Some(workspace_root.clone()));

        let project_type = handler.get_project_type().await;
        assert_eq!(project_type, Some(ProjectType::Rust));

        // Test project closing
        let close_event = Event::ProjectClosed {
            workspace_root: workspace_root.clone(),
        };

        let result = handler.handle(close_event).await;
        assert!(result.is_ok());

        // Verify workspace was cleared
        let current = handler.get_current_workspace().await;
        assert_eq!(current, None);
    }

    #[tokio::test]
    async fn test_tab_management() {
        let mut handler = WorkspaceHandler::new();
        handler.initialize().unwrap();

        let tab_id = TabId::new(1);
        let doc_id = DocumentId::default();

        // Test tab creation
        let create_event = Event::TabCreated {
            tab_id,
            doc_id,
            title: "test.rs".to_string(),
        };

        let result = handler.handle(create_event).await;
        assert!(result.is_ok());

        // Verify tab was created
        let tab_info = handler.get_tab_info(&tab_id).await;
        assert!(tab_info.is_some());
        assert_eq!(tab_info.unwrap().title, "test.rs");

        // Test tab closing
        let close_event = Event::TabClosed { tab_id, doc_id };

        let result = handler.handle(close_event).await;
        assert!(result.is_ok());

        // Verify tab was removed
        let tab_info = handler.get_tab_info(&tab_id).await;
        assert!(tab_info.is_none());
    }

    #[tokio::test]
    async fn test_panel_visibility() {
        let mut handler = WorkspaceHandler::new();
        handler.initialize().unwrap();

        // Test panel toggle
        let toggle_event = Event::PanelToggled {
            panel_type: PanelType::Search,
            is_visible: true,
        };

        let result = handler.handle(toggle_event).await;
        assert!(result.is_ok());

        // Verify panel visibility
        let is_visible = handler.is_panel_visible(PanelType::Search).await;
        assert!(is_visible);
    }

    #[tokio::test]
    async fn test_uninitialized_handler_error() {
        let mut handler = WorkspaceHandler::new();

        let event = Event::ProjectOpened {
            workspace_root: PathBuf::from("/test"),
            project_type: Some(ProjectType::Rust),
            file_count: 1,
        };

        let result = handler.handle(event).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not initialized"));
    }
}
