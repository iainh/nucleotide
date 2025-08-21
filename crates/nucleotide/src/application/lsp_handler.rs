// ABOUTME: LSP domain event handler for language server lifecycle and diagnostics
// ABOUTME: Processes V2 LSP events and maintains server state tracking

use helix_lsp::LanguageServerId;
use nucleotide_events::v2::handler::EventHandler;
use nucleotide_events::v2::lsp::{
    ActiveServer, Event, LspError, ProgressToken, ProjectType, ServerCapabilities, ServerHealth,
};
use nucleotide_logging::{debug, error, info, instrument, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// LSP event handler for V2 domain events
/// Tracks language server lifecycle, health, and workspace coordination
pub struct LspHandler {
    /// Active servers by workspace root
    active_servers: Arc<RwLock<HashMap<PathBuf, Vec<ActiveServer>>>>,
    /// Server health status tracking
    server_health: Arc<RwLock<HashMap<LanguageServerId, ServerHealth>>>,
    /// Progress tracking by server
    progress_tokens: Arc<RwLock<HashMap<LanguageServerId, Vec<String>>>>,
    /// Initialization state
    initialized: bool,
}

impl LspHandler {
    /// Create a new LSP handler
    pub fn new() -> Self {
        Self {
            active_servers: Arc::new(RwLock::new(HashMap::new())),
            server_health: Arc::new(RwLock::new(HashMap::new())),
            progress_tokens: Arc::new(RwLock::new(HashMap::new())),
            initialized: false,
        }
    }

    /// Initialize the handler
    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.initialized {
            warn!("LspHandler already initialized");
            return Ok(());
        }

        info!("Initializing LspHandler for V2 event processing");
        self.initialized = true;
        Ok(())
    }

    /// Get active servers for a workspace
    pub async fn get_active_servers(&self, workspace_root: &PathBuf) -> Vec<ActiveServer> {
        let servers = self.active_servers.read().await;
        servers.get(workspace_root).cloned().unwrap_or_default()
    }

    /// Get server health status
    pub async fn get_server_health(&self, server_id: &LanguageServerId) -> Option<ServerHealth> {
        let health = self.server_health.read().await;
        health.get(server_id).cloned()
    }

    /// Check if handler is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Handle server initialization event
    #[instrument(skip(self))]
    async fn handle_server_initialized(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::ServerInitialized {
            server_id,
            server_name,
            capabilities,
            workspace_root,
        } = event
        {
            info!(
                server_id = ?server_id,
                server_name = %server_name,
                workspace = %workspace_root.display(),
                "Language server initialized"
            );

            // Add to active servers
            let active_server = ActiveServer {
                server_id: *server_id,
                server_name: server_name.clone(),
                language_ids: vec![], // Will be populated when we know the language
                health: ServerHealth::Healthy,
                startup_time_ms: 0, // Will be tracked in future iteration
            };

            let mut servers = self.active_servers.write().await;
            servers
                .entry(workspace_root.clone())
                .or_insert_with(Vec::new)
                .push(active_server);

            // Initialize health tracking
            let mut health = self.server_health.write().await;
            health.insert(*server_id, ServerHealth::Healthy);

            debug!(server_count = servers.len(), "Updated active server list");
        }
        Ok(())
    }

    /// Handle server exit event
    #[instrument(skip(self))]
    async fn handle_server_exited(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::ServerExited {
            server_id,
            server_name,
            exit_code,
            workspace_root,
        } = event
        {
            info!(
                server_id = ?server_id,
                server_name = %server_name,
                exit_code = ?exit_code,
                workspace = %workspace_root.display(),
                "Language server exited"
            );

            // Remove from active servers
            let mut servers = self.active_servers.write().await;
            if let Some(server_list) = servers.get_mut(workspace_root) {
                server_list.retain(|s| s.server_id != *server_id);
                if server_list.is_empty() {
                    servers.remove(workspace_root);
                }
            }

            // Remove health tracking
            let mut health = self.server_health.write().await;
            health.remove(server_id);

            // Clear progress tokens
            let mut progress = self.progress_tokens.write().await;
            progress.remove(server_id);
        }
        Ok(())
    }

    /// Handle server error event
    #[instrument(skip(self))]
    async fn handle_server_error(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::ServerError {
            server_id,
            error,
            is_fatal,
        } = event
        {
            if *is_fatal {
                error!(
                    server_id = ?server_id,
                    error = ?error,
                    "Fatal LSP server error"
                );

                // Mark server as unhealthy
                let mut health = self.server_health.write().await;
                health.insert(*server_id, ServerHealth::Unhealthy);
            } else {
                warn!(
                    server_id = ?server_id,
                    error = ?error,
                    "Non-fatal LSP server error"
                );
            }
        }
        Ok(())
    }

    /// Handle health check completion
    #[instrument(skip(self))]
    async fn handle_health_check(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Event::HealthCheckCompleted {
            server_id,
            status,
            response_time_ms,
        } = event
        {
            debug!(
                server_id = ?server_id,
                status = ?status,
                response_time_ms = response_time_ms,
                "Health check completed"
            );

            let mut health = self.server_health.write().await;
            health.insert(*server_id, status.clone());
        }
        Ok(())
    }

    /// Handle progress events
    #[instrument(skip(self))]
    async fn handle_progress_event(
        &mut self,
        event: &Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            Event::ProgressStarted {
                server_id,
                token,
                title,
                message,
            } => {
                debug!(
                    server_id = ?server_id,
                    token = %token.0,
                    title = %title,
                    message = ?message,
                    "Progress started"
                );

                let mut progress = self.progress_tokens.write().await;
                progress
                    .entry(*server_id)
                    .or_insert_with(Vec::new)
                    .push(token.0.clone());
            }
            Event::ProgressUpdated {
                server_id,
                token,
                percentage,
                message,
            } => {
                debug!(
                    server_id = ?server_id,
                    token = %token.0,
                    percentage = ?percentage,
                    message = ?message,
                    "Progress updated"
                );
            }
            Event::ProgressCompleted {
                server_id,
                token,
                final_message,
            } => {
                debug!(
                    server_id = ?server_id,
                    token = %token.0,
                    final_message = ?final_message,
                    "Progress completed"
                );

                // Remove completed token
                let mut progress = self.progress_tokens.write().await;
                if let Some(tokens) = progress.get_mut(server_id) {
                    tokens.retain(|t| t != &token.0);
                    if tokens.is_empty() {
                        progress.remove(server_id);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EventHandler<Event> for LspHandler {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    #[instrument(skip(self, event))]
    async fn handle(&mut self, event: Event) -> Result<(), Self::Error> {
        if !self.initialized {
            error!("LspHandler not initialized");
            return Err("LspHandler not initialized".into());
        }

        debug!(event_type = ?std::mem::discriminant(&event), "Processing LSP event");

        match event {
            Event::ServerInitialized { .. } => {
                self.handle_server_initialized(&event).await?;
            }
            Event::ServerExited { .. } => {
                self.handle_server_exited(&event).await?;
            }
            Event::ServerError { .. } => {
                self.handle_server_error(&event).await?;
            }
            Event::ProjectDetected {
                workspace_root,
                project_type,
                recommended_servers,
            } => {
                info!(
                    workspace = %workspace_root.display(),
                    project_type = ?project_type,
                    servers = ?recommended_servers,
                    "Project detected"
                );
            }
            Event::ProjectServersReady {
                workspace_root,
                active_servers,
            } => {
                info!(
                    workspace = %workspace_root.display(),
                    server_count = active_servers.len(),
                    "Project servers ready"
                );
            }
            Event::HealthCheckCompleted { .. } => {
                self.handle_health_check(&event).await?;
            }
            Event::ProgressStarted { .. }
            | Event::ProgressUpdated { .. }
            | Event::ProgressCompleted { .. } => {
                self.handle_progress_event(&event).await?;
            }
            Event::ServerStartupRequested {
                workspace_root,
                server_name,
                language_id,
            } => {
                info!(
                    workspace = %workspace_root.display(),
                    server_name = %server_name,
                    language_id = %language_id,
                    "Server startup requested"
                );
                // This is a command event - actual startup would be handled elsewhere
            }
            Event::ServerRestarted {
                server_id,
                server_name,
                downtime_ms,
            } => {
                info!(
                    server_id = ?server_id,
                    server_name = %server_name,
                    downtime_ms = downtime_ms,
                    "Server restarted"
                );
            }
        }

        Ok(())
    }
}

impl Default for LspHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleotide_events::v2::lsp::ServerCapabilities;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_lsp_handler_initialization() {
        let mut handler = LspHandler::new();
        assert!(!handler.is_initialized());

        handler.initialize().unwrap();
        assert!(handler.is_initialized());
    }

    #[tokio::test]
    async fn test_server_lifecycle() {
        let mut handler = LspHandler::new();
        handler.initialize().unwrap();

        let server_id = LanguageServerId(1);
        let workspace_root = PathBuf::from("/test/workspace");

        // Test server initialization
        let init_event = Event::ServerInitialized {
            server_id,
            server_name: "rust-analyzer".to_string(),
            capabilities: ServerCapabilities {
                completion: true,
                hover: true,
                signature_help: true,
                definition: true,
                diagnostics: true,
                code_action: false,
                formatting: true,
                rename: true,
            },
            workspace_root: workspace_root.clone(),
        };

        let result = handler.handle(init_event).await;
        assert!(result.is_ok());

        // Verify server was added
        let servers = handler.get_active_servers(&workspace_root).await;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].server_name, "rust-analyzer");

        // Test server exit
        let exit_event = Event::ServerExited {
            server_id,
            server_name: "rust-analyzer".to_string(),
            exit_code: Some(0),
            workspace_root: workspace_root.clone(),
        };

        let result = handler.handle(exit_event).await;
        assert!(result.is_ok());

        // Verify server was removed
        let servers = handler.get_active_servers(&workspace_root).await;
        assert_eq!(servers.len(), 0);
    }

    #[tokio::test]
    async fn test_health_tracking() {
        let mut handler = LspHandler::new();
        handler.initialize().unwrap();

        let server_id = LanguageServerId(1);

        let health_event = Event::HealthCheckCompleted {
            server_id,
            status: ServerHealth::Healthy,
            response_time_ms: 50,
        };

        let result = handler.handle(health_event).await;
        assert!(result.is_ok());

        let health = handler.get_server_health(&server_id).await;
        assert_eq!(health, Some(ServerHealth::Healthy));
    }

    #[tokio::test]
    async fn test_uninitialized_handler_error() {
        let mut handler = LspHandler::new();

        let event = Event::ProjectDetected {
            workspace_root: PathBuf::from("/test"),
            project_type: ProjectType::Rust,
            recommended_servers: vec!["rust-analyzer".to_string()],
        };

        let result = handler.handle(event).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not initialized"));
    }
}
