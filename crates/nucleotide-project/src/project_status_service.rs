// ABOUTME: Service for publishing classified project and LSP status to the UI.
// ABOUTME: Stores backend-independent snapshots without owning detection or LSP state.

use gpui::Global;
use nucleotide_logging::{debug, info, warn};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Service handle for accessing project status throughout the application
#[derive(Clone)]
pub struct ProjectStatusHandle {
    inner: Arc<parking_lot::RwLock<ProjectStatusService>>,
}

impl ProjectStatusHandle {
    pub fn new(service: ProjectStatusService) -> Self {
        Self {
            inner: Arc::new(parking_lot::RwLock::new(service)),
        }
    }

    /// Get the current project root
    pub fn project_root(&self) -> Option<PathBuf> {
        self.inner.read().project_root.clone()
    }

    /// Update the project root and clear its previous classification.
    pub fn set_project_root(&self, path: Option<PathBuf>) {
        let mut service = self.inner.write();
        service.set_project_root(path);
    }

    /// Replace the canonical project type with a classifier result.
    pub fn set_project_type(&self, project_type: crate::ProjectType) {
        let mut service = self.inner.write();
        service.set_project_type(project_type);
    }

    /// Update the project-wide language-server status snapshot.
    pub fn update_lsp_status(&self, lsp_status: crate::project_indicator::ProjectLspStatus) {
        let mut service = self.inner.write();
        service.update_lsp_status(lsp_status);
    }

    /// Get project info for UI components
    pub fn get_project_info(&self) -> crate::project_indicator::ProjectInfo {
        self.inner.read().get_project_info().clone()
    }

    /// Get the canonical type detected for the current project.
    pub fn get_project_type(&self) -> crate::ProjectType {
        self.inner.read().get_project_info().project_type.clone()
    }

    /// Get LSP status for the current project
    pub fn get_lsp_status(&self) -> crate::project_indicator::ProjectLspStatus {
        self.inner.read().get_project_info().lsp_status.clone()
    }
}

impl Global for ProjectStatusHandle {}

/// Stores the latest project status for UI consumers.
pub struct ProjectStatusService {
    project_root: Option<PathBuf>,
    project_info: crate::project_indicator::ProjectInfo,
}

impl Default for ProjectStatusService {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectStatusService {
    pub fn new() -> Self {
        let project_info = crate::project_indicator::ProjectInfo::new(None);
        Self {
            project_root: None,
            project_info,
        }
    }

    /// Get the current project root directory
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// Set the project root directory. Classification is supplied separately by
    /// the backend-independent project classifier.
    pub fn set_project_root(&mut self, path: Option<PathBuf>) {
        if self.project_root == path {
            return;
        }

        info!(
            project_path = ?path,
            "Setting project root"
        );

        self.project_root = path.clone();
        self.project_info.root_path = path;
        self.project_info.project_type = crate::ProjectType::Unknown;
        self.project_info.last_updated = Instant::now();
    }

    /// Update the canonical type using a backend-independent classifier result.
    pub fn set_project_type(&mut self, project_type: crate::ProjectType) {
        info!(?project_type, "Setting classified project type");

        self.project_info.set_project_type(project_type);
    }

    /// Update project status from an application-owned LSP status snapshot.
    pub fn update_lsp_status(&mut self, lsp_status: crate::project_indicator::ProjectLspStatus) {
        if self.project_info.lsp_status == lsp_status {
            return;
        }

        debug!(
            server_count = lsp_status.total_servers,
            diagnostic_count = lsp_status.diagnostic_count,
            "Updating project LSP status"
        );

        self.project_info.update_lsp_status(lsp_status);

        info!(
            lsp_servers = self.project_info.lsp_status.total_servers,
            running_servers = self.project_info.lsp_status.running_servers,
            failed_servers = self.project_info.lsp_status.failed_servers,
            diagnostics = self.project_info.lsp_status.diagnostic_count,
            "Project LSP status updated"
        );
    }

    /// Get project info for UI components
    pub fn get_project_info(&self) -> &crate::project_indicator::ProjectInfo {
        &self.project_info
    }
}

/// Initialize the project status service and set it as a global
pub fn initialize_project_status_service(cx: &mut gpui::App) -> ProjectStatusHandle {
    info!("Initializing project status service");

    // Create the service using a simple constructor
    let service = ProjectStatusService::new();
    let handle = ProjectStatusHandle::new(service);

    // Set it as a global so other parts of the app can access it
    cx.set_global(handle.clone());

    info!("Project status service initialized and registered as global");
    debug!("Project status service handle: service created successfully");
    handle
}

/// Get project status service from global context
pub fn project_status_service(cx: &gpui::App) -> ProjectStatusHandle {
    match cx.try_global::<ProjectStatusHandle>() {
        Some(handle) => {
            debug!("Retrieved project status service from global context");
            handle.clone()
        }
        None => {
            warn!("Project status service not found in global context - this should not happen");
            // Create a temporary handle as fallback
            let service = ProjectStatusService::new();
            ProjectStatusHandle::new(service)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[tokio::test]
    async fn test_project_status_service_creation() {
        // Test basic functionality - simplified without GPUI context
        let service = ProjectStatusService::new();
        let project_root = service.project_root();
        assert!(project_root.is_none());
    }

    #[tokio::test]
    async fn test_set_project_root_resets_classification() {
        let temp_dir = TempDir::new().unwrap();
        let mut service = ProjectStatusService::new();
        service.set_project_type(crate::ProjectType::Rust);
        service.set_project_root(Some(temp_dir.path().to_path_buf()));

        let project_root = service.project_root();
        assert!(project_root.is_some());
        assert_eq!(project_root.unwrap(), temp_dir.path());
        assert_eq!(
            service.get_project_info().project_type,
            crate::ProjectType::Unknown
        );
    }

    #[tokio::test]
    async fn test_set_same_project_root_preserves_classification() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().to_path_buf();
        let mut service = ProjectStatusService::new();
        service.set_project_root(Some(root.clone()));
        service.set_project_type(crate::ProjectType::Rust);

        service.set_project_root(Some(root));

        assert_eq!(
            service.get_project_info().project_type,
            crate::ProjectType::Rust
        );
    }

    #[tokio::test]
    async fn test_set_project_type_updates_project_info() {
        let mut service = ProjectStatusService::new();
        service.set_project_type(crate::ProjectType::Mixed(vec![
            crate::ProjectType::Rust,
            crate::ProjectType::Python,
        ]));

        assert_eq!(
            service.get_project_info().project_type,
            crate::ProjectType::Mixed(vec![crate::ProjectType::Rust, crate::ProjectType::Python,])
        );
    }

    #[tokio::test]
    async fn test_lsp_status_update() {
        let mut service = ProjectStatusService::new();
        let status = crate::ProjectLspStatus {
            total_servers: 2,
            running_servers: 1,
            failed_servers: 0,
            initializing_servers: 1,
            has_diagnostics: true,
            diagnostic_count: 3,
        };
        service.update_lsp_status(status.clone());

        assert_eq!(service.get_project_info().lsp_status, status);
    }
}
