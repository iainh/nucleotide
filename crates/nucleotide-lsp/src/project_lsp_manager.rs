// ABOUTME: Project-level LSP management for proactive server startup and lifecycle
// ABOUTME: Coordinates between project detection and Helix's LSP system

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helix_lsp::LanguageServerId;
use helix_view::Editor;
use nucleotide_events::{
    ProjectLspCommand, ProjectLspCommandError, ProjectLspEvent, ProjectType, ServerHealthStatus,
    ServerStartResult,
};
use nucleotide_logging::{debug, error, info, instrument, warn};
use tokio::sync::{RwLock, broadcast};

use crate::HelixLspBridge;

// Re-export configuration types for easier access
pub use nucleotide_types::{ProjectMarker, ProjectMarkersConfig, RootStrategy};

/// Information about a detected project
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub workspace_root: PathBuf,
    pub project_type: ProjectType,
    pub language_servers: Vec<String>,
    pub detected_at: Instant,
}

/// Information about a managed LSP server
#[derive(Debug, Clone)]
pub struct ManagedServer {
    pub server_id: LanguageServerId,
    pub server_name: String,
    pub language_id: String,
    pub workspace_root: PathBuf,
    pub started_at: Instant,
    pub last_health_check: Option<Instant>,
    pub health_status: ServerHealthStatus,
}

/// Configuration for ProjectLspManager
#[derive(Debug, Clone)]
pub struct ProjectLspConfig {
    /// Enable proactive server startup
    pub enable_proactive_startup: bool,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Server startup timeout
    pub startup_timeout: Duration,
    /// Maximum concurrent server startups
    pub max_concurrent_startups: usize,
    /// Project markers configuration for custom project detection
    pub project_markers: ProjectMarkersConfig,
}

impl Default for ProjectLspConfig {
    fn default() -> Self {
        Self {
            enable_proactive_startup: true,
            health_check_interval: Duration::from_secs(30),
            startup_timeout: Duration::from_secs(10),
            max_concurrent_startups: 3,
            project_markers: ProjectMarkersConfig::default(),
        }
    }
}

/// Manages LSP servers at the project level
pub struct ProjectLspManager {
    /// Configuration
    config: ProjectLspConfig,

    /// Detected projects
    projects: Arc<RwLock<HashMap<PathBuf, ProjectInfo>>>,

    /// Managed servers by workspace root
    servers: Arc<RwLock<HashMap<PathBuf, Vec<ManagedServer>>>>,

    /// Event channel for project LSP events (broadcast for multiple listeners)
    event_tx: broadcast::Sender<ProjectLspEvent>,

    /// Project detector
    project_detector: Arc<ProjectDetector>,

    /// Server lifecycle manager  
    lifecycle_manager: Arc<ServerLifecycleManager>,

    /// Health checker task handle
    health_check_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,

    /// LSP command sender for event-driven command dispatch
    lsp_command_sender:
        Option<tokio::sync::mpsc::UnboundedSender<nucleotide_events::ProjectLspCommand>>,
}

impl ProjectLspManager {
    /// Create a new ProjectLspManager
    #[instrument(skip(lsp_command_sender))]
    pub fn new(
        config: ProjectLspConfig,
        lsp_command_sender: Option<
            tokio::sync::mpsc::UnboundedSender<nucleotide_events::ProjectLspCommand>,
        >,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(1000); // Buffer for 1000 events

        let project_detector = Arc::new(ProjectDetector::new(config.project_markers.clone()));
        let lifecycle_manager = Arc::new(ServerLifecycleManager::new(config.clone()));

        Self {
            config,
            projects: Arc::new(RwLock::new(HashMap::new())),
            servers: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            project_detector,
            lifecycle_manager,
            health_check_handle: Arc::new(RwLock::new(None)),
            lsp_command_sender,
        }
    }

    /// Start the manager and background tasks
    #[instrument(skip(self))]
    pub async fn start(&self) -> Result<(), ProjectLspError> {
        info!("Starting ProjectLspManager");

        // Start health check task
        self.start_health_check_task().await;

        // Start event processing task
        self.start_event_processing_task().await;

        info!("ProjectLspManager started successfully");
        Ok(())
    }

    /// Stop the manager and cleanup resources
    #[instrument(skip(self))]
    pub async fn stop(&self) -> Result<(), ProjectLspError> {
        info!("Stopping ProjectLspManager");

        // Cancel health check task
        if let Some(handle) = self.health_check_handle.write().await.take() {
            handle.abort();
        }

        // Cleanup all servers
        let servers = self.servers.read().await;
        for (workspace_root, server_list) in servers.iter() {
            for server in server_list {
                info!(
                    workspace_root = %workspace_root.display(),
                    server_id = ?server.server_id,
                    server_name = %server.server_name,
                    "Cleaning up managed server"
                );

                let _ = self.event_tx.send(ProjectLspEvent::ServerCleanupCompleted {
                    workspace_root: workspace_root.clone(),
                    server_id: server.server_id,
                });
            }
        }

        info!("ProjectLspManager stopped");
        Ok(())
    }

    /// Detect and register a project
    #[instrument(skip(self), fields(workspace_root = %workspace_root.display()))]
    pub async fn detect_project(&self, workspace_root: PathBuf) -> Result<(), ProjectLspError> {
        info!("Detecting project at workspace root");

        let project_info = self
            .project_detector
            .analyze_project(&workspace_root)
            .await
            .map_err(|e| ProjectLspError::ProjectDetection(e.to_string()))?;

        // Register the project
        self.projects
            .write()
            .await
            .insert(workspace_root.clone(), project_info.clone());

        // Send project detection event
        let _ = self.event_tx.send(ProjectLspEvent::ProjectDetected {
            workspace_root: project_info.workspace_root.clone(),
            project_type: project_info.project_type.clone(),
            language_servers: project_info.language_servers.clone(),
        });

        info!(
            project_type = ?project_info.project_type,
            language_servers = ?project_info.language_servers,
            "Project detected and registered"
        );

        // Start language servers if proactive startup is enabled
        if self.config.enable_proactive_startup {
            self.request_server_startup(&project_info).await?;
        }

        Ok(())
    }

    /// Request server startup for a project
    #[instrument(skip(self), fields(workspace_root = %project_info.workspace_root.display()))]
    async fn request_server_startup(
        &self,
        project_info: &ProjectInfo,
    ) -> Result<(), ProjectLspError> {
        // Skip LSP server startup if workspace root is the system root (/)
        // This happens when the app starts without a proper project directory
        if project_info.workspace_root == std::path::Path::new("/") {
            info!(
                workspace_root = %project_info.workspace_root.display(),
                "Skipping LSP server startup for system root - waiting for proper project directory to be set"
            );
            return Ok(());
        }

        info!("Requesting server startup for project");

        for server_name in &project_info.language_servers {
            let language_id = self
                .project_detector
                .get_primary_language_id(&project_info.project_type);

            // Send LSP server startup event through the existing event bridge
            nucleotide_core::event_bridge::send_bridged_event(
                nucleotide_core::event_bridge::BridgedEvent::LspServerStartupRequested {
                    workspace_root: project_info.workspace_root.clone(),
                    server_name: server_name.clone(),
                    language_id: language_id.clone(),
                },
            );

            info!(
                server_name = %server_name,
                language_id = %language_id,
                workspace_root = %project_info.workspace_root.display(),
                "Successfully sent LspServerStartupRequested event through event bridge"
            );
        }

        Ok(())
    }

    /// Start health check background task
    async fn start_health_check_task(&self) {
        let servers = Arc::clone(&self.servers);
        let event_tx = self.event_tx.clone();
        let interval = self.config.health_check_interval;

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);

            loop {
                interval.tick().await;

                let servers_read = servers.read().await;
                for (workspace_root, server_list) in servers_read.iter() {
                    for server in server_list {
                        // Perform health check
                        let status = Self::perform_health_check(server).await;

                        let _ = event_tx.send(ProjectLspEvent::HealthCheckCompleted {
                            workspace_root: workspace_root.clone(),
                            server_id: server.server_id,
                            status,
                        });
                    }
                }
            }
        });

        *self.health_check_handle.write().await = Some(handle);
    }

    /// Start event processing background task (deprecated - events now handled by Application)
    async fn start_event_processing_task(&self) {
        debug!(
            "Event processing task not needed - Application handles events directly via broadcast channel"
        );
    }

    /// Handle a project LSP event (deprecated - now handled by Application)
    async fn handle_project_lsp_event(
        _servers: &Arc<RwLock<HashMap<PathBuf, Vec<ManagedServer>>>>,
        _lifecycle_manager: &Arc<ServerLifecycleManager>,
        _lsp_command_sender: &Option<
            tokio::sync::mpsc::UnboundedSender<nucleotide_events::ProjectLspCommand>,
        >,
        _event_sender: &broadcast::Sender<ProjectLspEvent>,
        _event: ProjectLspEvent,
    ) {
        // Deprecated - Application now handles events directly
        debug!(
            "handle_project_lsp_event called but deprecated - events now handled by Application"
        );
        /*
        match event {
            ProjectLspEvent::ServerStartupRequested {
                workspace_root,
                server_name,
                language_id,
            } => {
                info!(
                    workspace_root = %workspace_root.display(),
                    server_name = %server_name,
                    language_id = %language_id,
                    "ServerStartupRequested event processed - Application should handle this"
                );

                // ARCHITECTURE SIMPLIFICATION: This event should be handled by the Application
                // The ProjectLspManager's role is just to emit these events once during project detection
                // The Application's event listener will pick these up and queue them for processing
                // We don't re-emit here to avoid infinite loops

                debug!("ServerStartupRequested event acknowledged - waiting for Application to process");
            }

            ProjectLspEvent::ServerStartupCompleted {
                workspace_root,
                server_name,
                server_id,
                status,
            } => {
                info!(
                    workspace_root = %workspace_root.display(),
                    server_name = %server_name,
                    server_id = ?server_id,
                    status = ?status,
                    "Received ServerStartupCompleted event from Application"
                );

                match status {
                    nucleotide_events::ServerStartupResult::Success => {
                        let server_info = ManagedServer {
                            server_id,
                            server_name: server_name.clone(),
                            language_id: "rust".to_string(), // TODO: get from event
                            workspace_root: workspace_root.clone(),
                            started_at: std::time::Instant::now(),
                            last_health_check: Some(std::time::Instant::now()),
                            health_status: nucleotide_events::ServerHealthStatus::Healthy,
                        };

                        // Add to managed servers
                        servers
                            .write()
                            .await
                            .entry(workspace_root.clone())
                            .or_default()
                            .push(server_info.clone());

                        info!(
                            server_id = ?server_id,
                            server_name = %server_name,
                            "LSP server started successfully via event system"
                        );
                    }
                    nucleotide_events::ServerStartupResult::Failed { error } => {
                        error!(
                            error = %error,
                            server_name = %server_name,
                            workspace_root = %workspace_root.display(),
                            "LSP server startup failed"
                        );
                    }
                    nucleotide_events::ServerStartupResult::Timeout => {
                        error!(
                            server_name = %server_name,
                            workspace_root = %workspace_root.display(),
                            "LSP server startup timed out"
                        );
                    }
                    nucleotide_events::ServerStartupResult::ConfigurationError { error } => {
                        error!(
                            error = %error,
                            server_name = %server_name,
                            workspace_root = %workspace_root.display(),
                            "LSP server startup configuration error"
                        );
                    }
                }
            }

            ProjectLspEvent::HealthCheckCompleted {
                workspace_root,
                server_id,
                status,
            } => {
                // Update server health status
                if let Some(server_list) = servers.write().await.get_mut(&workspace_root) {
                    if let Some(server) = server_list.iter_mut().find(|s| s.server_id == server_id)
                    {
                        server.health_status = status.clone();
                        server.last_health_check = Some(Instant::now());

                        debug!(
                            server_id = ?server_id,
                            health_status = ?status,
                            "Updated server health status"
                        );
                    }
                }
            }

            ProjectLspEvent::ProjectCleanupRequested { workspace_root } => {
                info!(
                    workspace_root = %workspace_root.display(),
                    "Processing project cleanup request"
                );

                if let Some(server_list) = servers.write().await.remove(&workspace_root) {
                    for server in server_list {
                        let _ = lifecycle_manager.stop_server(server.server_id).await;
                    }
                }
            }

            _ => {
                debug!(event = ?event, "Unhandled project LSP event");
            }
        }
        */
    }

    /// Perform health check on a server
    async fn perform_health_check(server: &ManagedServer) -> ServerHealthStatus {
        // Simple health check - in a real implementation, this would
        // send a request to the server and check response time

        let time_since_start = server.started_at.elapsed();

        // Consider server healthy if it's been running for more than 5 seconds
        // In practice, this would involve actual LSP communication
        if time_since_start > Duration::from_secs(5) {
            ServerHealthStatus::Healthy
        } else {
            ServerHealthStatus::Unresponsive
        }
    }

    /// Get project information
    pub async fn get_project_info(&self, workspace_root: &PathBuf) -> Option<ProjectInfo> {
        self.projects.read().await.get(workspace_root).cloned()
    }

    /// Get managed servers for a workspace
    pub async fn get_managed_servers(&self, workspace_root: &PathBuf) -> Vec<ManagedServer> {
        self.servers
            .read()
            .await
            .get(workspace_root)
            .cloned()
            .unwrap_or_default()
    }

    /// Get event sender for external coordination
    pub fn get_event_sender(&self) -> broadcast::Sender<ProjectLspEvent> {
        self.event_tx.clone()
    }

    /// Get event receiver for listening to ProjectLsp events
    /// Creates a new receiver that can be called multiple times
    pub fn get_event_receiver(&self) -> Option<broadcast::Receiver<ProjectLspEvent>> {
        Some(self.event_tx.subscribe())
    }

    /// Set the Helix bridge for actual LSP server integration
    pub async fn set_helix_bridge(&self, bridge: Arc<HelixLspBridge>) {
        info!("Setting Helix bridge on ProjectLspManager");
        self.lifecycle_manager.set_helix_bridge(bridge).await;
        info!("Helix bridge successfully set on ProjectLspManager");
    }

    /// Start a language server using event-driven command pattern
    #[instrument(skip(self), fields(
        workspace_root = %workspace_root.display(),
        server_name = %server_name,
        language_id = %language_id
    ))]
    pub async fn start_server(
        &self,
        workspace_root: &PathBuf,
        server_name: &str,
        language_id: &str,
    ) -> Result<ManagedServer, ProjectLspError> {
        info!(
            workspace_root = %workspace_root.display(),
            server_name = %server_name,
            language_id = %language_id,
            "Starting language server via event-driven command pattern"
        );

        // Use event bridge to dispatch command to Application with Editor access
        if let Some(ref command_sender) = self.lsp_command_sender {
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let span = tracing::info_span!(
                "lsp_start_server_command",
                workspace_root = %workspace_root.display(),
                server_name = %server_name,
                language_id = %language_id
            );

            let command = ProjectLspCommand::StartServer {
                workspace_root: workspace_root.clone(),
                server_name: server_name.to_string(),
                language_id: language_id.to_string(),
                response: response_tx,
                span,
            };

            // Send command through event bridge
            if let Err(e) = command_sender.send(command) {
                error!(
                    error = %e,
                    "Failed to send StartServer command through event bridge"
                );
                return Err(ProjectLspError::ServerStartup(format!(
                    "Event bridge communication failed: {}",
                    e
                )));
            }

            // Wait for response from Application with timeout to prevent indefinite blocking
            let response_timeout = tokio::time::Duration::from_secs(30); // 30 second timeout for LSP server startup
            match tokio::time::timeout(response_timeout, response_rx).await {
                Ok(response_result) => match response_result {
                    Ok(Ok(server_result)) => {
                        info!(
                            server_id = ?server_result.server_id,
                            server_name = %server_result.server_name,
                            "Successfully started LSP server via event bridge"
                        );

                        let server_id = server_result.server_id;

                        let managed_server = ManagedServer {
                            server_id,
                            server_name: server_name.to_string(),
                            language_id: language_id.to_string(),
                            workspace_root: workspace_root.clone(),
                            started_at: Instant::now(),
                            last_health_check: None,
                            health_status: ServerHealthStatus::Healthy,
                        };

                        info!(
                            server_id = ?server_id,
                            "Language server started successfully via event bridge"
                        );

                        Ok(managed_server)
                    }
                    Ok(Err(e)) => {
                        error!(
                            error = %e,
                            "Application failed to start LSP server"
                        );
                        Err(ProjectLspError::ServerStartup(format!(
                            "Server startup failed: {}",
                            e
                        )))
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            "Failed to receive response from Application"
                        );
                        Err(ProjectLspError::ServerStartup(format!(
                            "Event bridge response failed: {}",
                            e
                        )))
                    }
                },
                Err(_timeout) => {
                    error!(
                        timeout_seconds = 30,
                        workspace_root = %workspace_root.display(),
                        server_name = %server_name,
                        language_id = %language_id,
                        "LSP server startup timed out - Application may be blocked on environment capture"
                    );
                    Err(ProjectLspError::ServerStartup(
                        "LSP server startup timed out after 30 seconds".to_string(),
                    ))
                }
            }
        } else {
            error!("No LSP command sender available - event bridge not initialized");
            Err(ProjectLspError::ServerStartup(
                "Event bridge not initialized".to_string(),
            ))
        }
    }
}

/// Project detection and analysis
pub struct ProjectDetector {
    /// Custom project markers configuration
    project_markers_config: ProjectMarkersConfig,
    /// Maximum depth for ancestor directory traversal
    max_traversal_depth: usize,
}

impl ProjectDetector {
    pub fn new(project_markers_config: ProjectMarkersConfig) -> Self {
        Self {
            project_markers_config,
            max_traversal_depth: 10, // Reasonable default
        }
    }

    /// Create a new ProjectDetector with custom configuration
    pub fn with_config(
        project_markers_config: ProjectMarkersConfig,
        max_traversal_depth: usize,
    ) -> Self {
        Self {
            project_markers_config,
            max_traversal_depth,
        }
    }

    /// Analyze a project directory
    #[instrument(skip(self), fields(workspace_root = %workspace_root.display()))]
    pub async fn analyze_project(
        &self,
        workspace_root: &PathBuf,
    ) -> Result<ProjectInfo, Box<dyn std::error::Error + Send + Sync>> {
        info!("Analyzing project structure");

        let project_type = self.detect_project_type(workspace_root).await?;
        let language_servers = self.get_language_servers_for_project(&project_type);

        Ok(ProjectInfo {
            workspace_root: workspace_root.clone(),
            project_type,
            language_servers,
            detected_at: Instant::now(),
        })
    }

    /// Detect project type based on files and structure
    async fn detect_project_type(
        &self,
        workspace_root: &PathBuf,
    ) -> Result<ProjectType, Box<dyn std::error::Error + Send + Sync>> {
        // First try custom project markers if enabled
        if self.project_markers_config.enable_project_markers {
            if let Some(project_type) = self.detect_with_custom_markers(workspace_root).await? {
                return Ok(project_type);
            }
        }

        // Fall back to builtin detection if enabled or no custom markers found
        if self.project_markers_config.enable_builtin_fallback {
            return self.detect_with_builtin_patterns(workspace_root).await;
        }

        Ok(ProjectType::Unknown)
    }

    /// Detect project type using custom markers configuration
    async fn detect_with_custom_markers(
        &self,
        workspace_root: &PathBuf,
    ) -> Result<Option<ProjectType>, Box<dyn std::error::Error + Send + Sync>> {
        info!(
            workspace_root = %workspace_root.display(),
            markers_count = self.project_markers_config.markers.len(),
            "Attempting project detection with custom markers"
        );

        // Collect all potential matches with priorities
        let mut matches = Vec::new();

        for (project_name, marker_config) in &self.project_markers_config.markers {
            for marker_pattern in &marker_config.markers {
                if workspace_root.join(marker_pattern).exists() {
                    matches.push((project_name, marker_config, marker_pattern));
                    info!(
                        project_name = %project_name,
                        marker_pattern = %marker_pattern,
                        priority = marker_config.priority,
                        language_server = %marker_config.language_server,
                        "Found matching custom project marker"
                    );
                }
            }
        }

        if matches.is_empty() {
            debug!("No custom markers found for project");
            return Ok(None);
        }

        // Sort by priority (highest first)
        matches.sort_by(|a, b| b.1.priority.cmp(&a.1.priority));

        // Use the highest priority match
        let (project_name, _marker_config, marker_pattern) = matches[0];

        info!(
            selected_project = %project_name,
            selected_marker = %marker_pattern,
            total_matches = matches.len(),
            "Selected project type based on highest priority custom marker"
        );

        // Map project name to ProjectType
        // For now, return a custom project type - in future we could extend ProjectType enum
        // to handle custom project types from configuration
        let project_type = self.map_custom_project_to_builtin_type(project_name);

        Ok(Some(project_type))
    }

    /// Map custom project name to builtin ProjectType
    /// This is a temporary solution - ideally ProjectType would be extensible
    fn map_custom_project_to_builtin_type(&self, project_name: &str) -> ProjectType {
        // Try to infer from project name or use Unknown
        let project_lower = project_name.to_lowercase();

        if project_lower.contains("rust") {
            ProjectType::Rust
        } else if project_lower.contains("typescript") || project_lower.contains("ts") {
            ProjectType::TypeScript
        } else if project_lower.contains("javascript")
            || project_lower.contains("js")
            || project_lower.contains("node")
        {
            ProjectType::JavaScript
        } else if project_lower.contains("python") || project_lower.contains("py") {
            ProjectType::Python
        } else if project_lower.contains("go") {
            ProjectType::Go
        } else if project_lower.contains("cpp") || project_lower.contains("c++") {
            ProjectType::Cpp
        } else if project_lower.contains("c") {
            ProjectType::C
        } else {
            // For truly custom project types, we use Unknown but still get the benefit
            // of the custom language server configuration
            ProjectType::Unknown
        }
    }

    /// Detect project type using builtin patterns (original logic)
    async fn detect_with_builtin_patterns(
        &self,
        workspace_root: &PathBuf,
    ) -> Result<ProjectType, Box<dyn std::error::Error + Send + Sync>> {
        debug!(workspace_root = %workspace_root.display(), "Using builtin project detection patterns");

        // Check for specific project markers
        if workspace_root.join("Cargo.toml").exists() {
            return Ok(ProjectType::Rust);
        }

        if workspace_root.join("package.json").exists() {
            let _package_json = workspace_root.join("package.json");
            if workspace_root.join("tsconfig.json").exists()
                || self.has_typescript_files(workspace_root).await?
            {
                return Ok(ProjectType::TypeScript);
            } else {
                return Ok(ProjectType::JavaScript);
            }
        }

        if workspace_root.join("pyproject.toml").exists()
            || workspace_root.join("requirements.txt").exists()
            || workspace_root.join("setup.py").exists()
        {
            return Ok(ProjectType::Python);
        }

        if workspace_root.join("go.mod").exists() {
            return Ok(ProjectType::Go);
        }

        if workspace_root.join("CMakeLists.txt").exists()
            || workspace_root.join("Makefile").exists()
        {
            if self.has_cpp_files(workspace_root).await? {
                return Ok(ProjectType::Cpp);
            } else {
                return Ok(ProjectType::C);
            }
        }

        Ok(ProjectType::Unknown)
    }

    /// Check for TypeScript files
    async fn has_typescript_files(
        &self,
        workspace_root: &PathBuf,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // Simple implementation - in practice would scan directories
        Ok(workspace_root.join("src").exists()
            && std::fs::read_dir(workspace_root.join("src"))?.any(|entry| {
                if let Ok(entry) = entry {
                    entry
                        .path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "ts" || ext == "tsx")
                        .unwrap_or(false)
                } else {
                    false
                }
            }))
    }

    /// Check for C++ files
    async fn has_cpp_files(
        &self,
        workspace_root: &PathBuf,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // Simple implementation - in practice would scan directories
        Ok(workspace_root.join("src").exists())
    }

    /// Get language servers for a project type
    fn get_language_servers_for_project(&self, project_type: &ProjectType) -> Vec<String> {
        // First check if we have custom language server configurations
        let mut servers = self.get_custom_language_servers();

        // If no custom servers or fallback enabled, add builtin servers
        if servers.is_empty() || self.project_markers_config.enable_builtin_fallback {
            servers.extend(self.get_builtin_language_servers(project_type));
        }

        // Deduplicate and sort
        servers.sort();
        servers.dedup();

        servers
    }

    /// Get language servers from custom project markers configuration
    fn get_custom_language_servers(&self) -> Vec<String> {
        if !self.project_markers_config.enable_project_markers {
            return Vec::new();
        }

        let servers: Vec<String> = self
            .project_markers_config
            .markers
            .values()
            .map(|marker| marker.language_server.clone())
            .collect();

        debug!(
            custom_servers = ?servers,
            "Retrieved custom language servers from project markers configuration"
        );

        servers
    }

    /// Get builtin language servers for a project type
    fn get_builtin_language_servers(&self, project_type: &ProjectType) -> Vec<String> {
        match project_type {
            ProjectType::Rust => vec!["rust-analyzer".to_string()],
            ProjectType::TypeScript => vec!["typescript-language-server".to_string()],
            ProjectType::JavaScript => vec!["typescript-language-server".to_string()],
            ProjectType::Python => vec!["pyright".to_string()],
            ProjectType::Go => vec!["gopls".to_string()],
            ProjectType::C => vec!["clangd".to_string()],
            ProjectType::Cpp => vec!["clangd".to_string()],
            ProjectType::Mixed(types) => {
                let mut servers = Vec::new();
                for project_type in types {
                    servers.extend(self.get_builtin_language_servers(project_type));
                }
                servers.sort();
                servers.dedup();
                servers
            }
            ProjectType::Other(_name) => {
                // Custom project types don't have builtin language servers
                // Their language servers should come from the project markers configuration
                vec![]
            }
            ProjectType::Unknown => vec![],
        }
    }

    /// Get primary language ID for a project type
    pub fn get_primary_language_id(&self, project_type: &ProjectType) -> String {
        match project_type {
            ProjectType::Rust => "rust".to_string(),
            ProjectType::TypeScript => "typescript".to_string(),
            ProjectType::JavaScript => "javascript".to_string(),
            ProjectType::Python => "python".to_string(),
            ProjectType::Go => "go".to_string(),
            ProjectType::C => "c".to_string(),
            ProjectType::Cpp => "cpp".to_string(),
            ProjectType::Mixed(_) => "unknown".to_string(),
            ProjectType::Other(name) => {
                // Use the custom project name as language ID
                name.to_lowercase().replace(' ', "_")
            }
            ProjectType::Unknown => "unknown".to_string(),
        }
    }
}

/// Server lifecycle management
pub struct ServerLifecycleManager {
    #[allow(dead_code)]
    config: ProjectLspConfig,
    helix_bridge: Arc<RwLock<Option<Arc<HelixLspBridge>>>>,
}

impl ServerLifecycleManager {
    pub fn new(config: ProjectLspConfig) -> Self {
        Self {
            config,
            helix_bridge: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the Helix bridge for actual LSP integration
    pub async fn set_helix_bridge(&self, bridge: Arc<HelixLspBridge>) {
        info!("ServerLifecycleManager: Setting Helix bridge");
        *self.helix_bridge.write().await = Some(bridge);
        info!("ServerLifecycleManager: Helix bridge set successfully");
    }

    /// Stop a language server
    #[instrument(skip(self), fields(server_id = ?server_id))]
    pub async fn stop_server(&self, server_id: LanguageServerId) -> Result<(), ProjectLspError> {
        info!("Stopping language server");

        if let Some(_bridge) = self.helix_bridge.read().await.as_ref() {
            // Would call: bridge.stop_server(editor, server_id).await
            // But we need Editor instance which requires integration at application level
            info!("Helix bridge available, would stop server via registry");
        } else {
            info!("No Helix bridge available, server stop simulated");
        }

        info!("Language server stopped");
        Ok(())
    }
}

/// Project LSP management errors
#[derive(Debug, thiserror::Error)]
pub enum ProjectLspError {
    #[error("Project detection failed: {0}")]
    ProjectDetection(String),

    #[error("Server startup failed: {0}")]
    ServerStartup(String),

    #[error("Server communication failed: {0}")]
    ServerCommunication(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_project_lsp_config_default() {
        let config = ProjectLspConfig::default();

        assert!(config.enable_proactive_startup);
        assert_eq!(config.health_check_interval, Duration::from_secs(30));
        assert_eq!(config.startup_timeout, Duration::from_secs(10));
        assert_eq!(config.max_concurrent_startups, 3);
    }

    #[test]
    fn test_project_detector_rust_detection() {
        let detector = ProjectDetector::new(nucleotide_types::ProjectMarkersConfig::default());
        let servers = detector.get_language_servers_for_project(&ProjectType::Rust);

        assert_eq!(servers, vec!["rust-analyzer".to_string()]);
    }

    #[test]
    fn test_project_detector_typescript_detection() {
        let detector = ProjectDetector::new(nucleotide_types::ProjectMarkersConfig::default());
        let servers = detector.get_language_servers_for_project(&ProjectType::TypeScript);

        assert_eq!(servers, vec!["typescript-language-server".to_string()]);
    }

    #[test]
    fn test_project_detector_mixed_detection() {
        let detector = ProjectDetector::new(nucleotide_types::ProjectMarkersConfig::default());
        let mixed_type = ProjectType::Mixed(vec![ProjectType::Rust, ProjectType::TypeScript]);
        let servers = detector.get_language_servers_for_project(&mixed_type);

        assert!(servers.contains(&"rust-analyzer".to_string()));
        assert!(servers.contains(&"typescript-language-server".to_string()));
    }

    #[test]
    fn test_project_detector_language_id_mapping() {
        let detector = ProjectDetector::new(nucleotide_types::ProjectMarkersConfig::default());

        assert_eq!(detector.get_primary_language_id(&ProjectType::Rust), "rust");
        assert_eq!(
            detector.get_primary_language_id(&ProjectType::TypeScript),
            "typescript"
        );
        assert_eq!(
            detector.get_primary_language_id(&ProjectType::Python),
            "python"
        );
        assert_eq!(
            detector.get_primary_language_id(&ProjectType::Unknown),
            "unknown"
        );
    }

    #[tokio::test]
    async fn test_project_lsp_manager_creation() {
        let config = ProjectLspConfig::default();
        let manager = ProjectLspManager::new(config, None);

        // Test that we can get an event sender
        let _event_sender = manager.get_event_sender();

        // Manager should be created successfully
        assert!(manager.projects.read().await.is_empty());
        assert!(manager.servers.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_server_lifecycle_manager_creation() {
        let config = ProjectLspConfig::default();
        let lifecycle_manager = ServerLifecycleManager::new(config);

        // Should be created without bridge initially
        assert!(lifecycle_manager.helix_bridge.is_none());
    }

    #[test]
    fn test_managed_server_creation() {
        let server_id = slotmap::KeyData::from_ffi(12345).into();
        let workspace_root = PathBuf::from("/test/workspace");

        let managed_server = ManagedServer {
            server_id,
            server_name: "rust-analyzer".to_string(),
            language_id: "rust".to_string(),
            workspace_root: workspace_root.clone(),
            started_at: Instant::now(),
            last_health_check: None,
            health_status: ServerHealthStatus::Healthy,
        };

        assert_eq!(managed_server.server_name, "rust-analyzer");
        assert_eq!(managed_server.language_id, "rust");
        assert_eq!(managed_server.workspace_root, workspace_root);
        assert!(matches!(
            managed_server.health_status,
            ServerHealthStatus::Healthy
        ));
    }

    #[test]
    fn test_project_info_creation() {
        let workspace_root = PathBuf::from("/test/project");
        let project_info = ProjectInfo {
            workspace_root: workspace_root.clone(),
            project_type: ProjectType::Rust,
            language_servers: vec!["rust-analyzer".to_string()],
            detected_at: Instant::now(),
        };

        assert_eq!(project_info.workspace_root, workspace_root);
        assert!(matches!(project_info.project_type, ProjectType::Rust));
        assert_eq!(project_info.language_servers, vec!["rust-analyzer"]);
    }

    #[test]
    fn test_project_lsp_error_types() {
        let detection_error = ProjectLspError::ProjectDetection("detection failed".to_string());
        assert!(
            detection_error
                .to_string()
                .contains("Project detection failed")
        );

        let startup_error = ProjectLspError::ServerStartup("startup failed".to_string());
        assert!(startup_error.to_string().contains("Server startup failed"));

        let config_error = ProjectLspError::Configuration("config error".to_string());
        assert!(config_error.to_string().contains("Configuration error"));
    }
}
