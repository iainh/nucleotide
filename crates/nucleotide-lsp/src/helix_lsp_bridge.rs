// ABOUTME: Bridge between ProjectLspManager and Helix's LSP Registry system
// ABOUTME: Provides seamless integration without breaking existing LSP infrastructure

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use helix_lsp::LanguageServerId;
use helix_view::Editor;
use nucleotide_events::{ProjectLspEvent, ServerStartupResult};
use nucleotide_logging::{debug, error, info, instrument, warn};
use serde_json::Value as JsonValue;
use tokio::sync::broadcast;

use crate::{ProjectLspError, ProjectLspManager};

// Define a dyn-compatible trait for environment providers using boxed futures
pub trait EnvironmentProvider: Send + Sync {
    /// Get environment variables for LSP servers in the given directory
    fn get_lsp_environment(
        &self,
        directory: &std::path::Path,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        std::collections::HashMap<String, String>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send
                + '_,
        >,
    >;
}

/// Bridge between ProjectLspManager and Helix's LSP system
#[derive(Clone)]
pub struct HelixLspBridge {
    /// Event sender for project events
    project_event_tx: broadcast::Sender<ProjectLspEvent>,
    /// Environment provider for LSP server startup
    environment_provider: Option<Arc<dyn EnvironmentProvider>>,
}

impl HelixLspBridge {
    /// Create a new bridge without environment provider (legacy)
    pub fn new(project_event_tx: broadcast::Sender<ProjectLspEvent>) -> Self {
        Self {
            project_event_tx,
            environment_provider: None,
        }
    }

    /// Create a new bridge with environment provider for dynamic environment injection
    pub fn new_with_environment(
        project_event_tx: broadcast::Sender<ProjectLspEvent>,
        environment_provider: Arc<dyn EnvironmentProvider>,
    ) -> Self {
        Self {
            project_event_tx,
            environment_provider: Some(environment_provider),
        }
    }

    /// Start a language server through Helix's registry
    #[instrument(skip(self, editor), fields(
        workspace_root = %workspace_root.display(),
        server_name = %server_name,
        language_id = %language_id
    ))]
    pub async fn start_server(
        &self,
        editor: &mut Editor,
        workspace_root: &PathBuf,
        server_name: &str,
        language_id: &str,
    ) -> Result<LanguageServerId, ProjectLspError> {
        info!("Starting server through Helix registry");

        // Check if server is already running for this workspace
        if let Some(existing_server) =
            self.find_existing_server(editor, server_name, workspace_root)
        {
            info!(
                server_id = ?existing_server,
                "Server already running for workspace"
            );
            return Ok(existing_server);
        }

        // Start the server through Helix's registry
        let server_id = self
            .start_server_via_registry(editor, language_id, workspace_root, server_name)
            .await?;

        // Send success event
        let _ = self
            .project_event_tx
            .send(ProjectLspEvent::ServerStartupCompleted {
                workspace_root: workspace_root.clone(),
                server_name: server_name.to_string(),
                server_id,
                status: ServerStartupResult::Success,
            });

        info!(server_id = ?server_id, "Server started successfully");
        Ok(server_id)
    }

    /// Stop a language server through Helix's registry
    #[instrument(skip(self, editor), fields(server_id = ?server_id))]
    pub async fn stop_server(
        &self,
        editor: &mut Editor,
        server_id: LanguageServerId,
    ) -> Result<(), ProjectLspError> {
        info!("Stopping server through Helix registry");

        // Remove server from registry
        editor.language_servers.remove_by_id(server_id);

        info!("Server stopped successfully");
        Ok(())
    }

    /// Find existing server for workspace and server name
    fn find_existing_server(
        &self,
        editor: &Editor,
        server_name: &str,
        _workspace_root: &PathBuf,
    ) -> Option<LanguageServerId> {
        // Check all active language servers
        for client in editor.language_servers.iter_clients() {
            if client.name() == server_name {
                // Check if this server is serving the workspace
                // This is a simplified check - in practice would need to verify
                // the server's workspace folders
                return Some(client.id());
            }
        }
        None
    }

    /// Start server via Helix's registry system
    async fn start_server_via_registry(
        &self,
        editor: &mut Editor,
        language_id: &str,
        workspace_root: &PathBuf,
        server_name: &str,
    ) -> Result<LanguageServerId, ProjectLspError> {
        // Get the language configuration for this language
        let syntax_loader = editor.syn_loader.load();
        let language_config = syntax_loader
            .language_configs()
            .find(|config| config.language_id == language_id)
            .ok_or_else(|| {
                ProjectLspError::Configuration(format!(
                    "No language configuration found for language ID: {}",
                    language_id
                ))
            })?;

        // Inject environment variables if environment provider is available
        let mut original_env_vars = Vec::new();

        if let Some(ref env_provider) = self.environment_provider {
            debug!("Injecting environment variables for LSP server startup");

            match env_provider.get_lsp_environment(workspace_root).await {
                Ok(project_env) => {
                    info!(
                        env_count = project_env.len(),
                        workspace_root = %workspace_root.display(),
                        "Successfully retrieved project environment for LSP server"
                    );

                    // TEMPORARY SOLUTION: Set environment variables in the current process
                    // This works because Helix will inherit the environment when starting servers
                    // TODO: This is not ideal as it affects the entire process, but it's a working solution
                    for (key, value) in &project_env {
                        // Store original value for restoration
                        let original = std::env::var(key).ok();
                        original_env_vars.push((key.clone(), original));

                        // Set the new environment variable
                        // SAFETY: This is safe because we're setting environment variables
                        // in a single-threaded context during server startup
                        unsafe {
                            std::env::set_var(key, value);
                        }

                        // Log key variables for debugging
                        if key == "PATH" || key == "CARGO_HOME" || key == "RUSTC" || key == "CARGO"
                        {
                            debug!(key = %key, value = %value, "Set environment variable for LSP server");
                        }
                    }

                    info!(
                        "Temporarily set {} environment variables for LSP server startup",
                        project_env.len()
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        workspace_root = %workspace_root.display(),
                        "Failed to get project environment, using default"
                    );
                }
            }
        } else {
            debug!("No environment provider configured, using default environment");
        }

        // Create root directories for server startup
        let root_dirs = vec![workspace_root.clone()];

        // Get language servers for this configuration
        // This integrates with the existing Helix LSP infrastructure
        // The environment variables we just set will be inherited by the server process
        //
        // NOTE: This call can potentially hang if the server binary is not found
        // The outer timeout in handle_start_server_command should catch this
        info!(
            server_name = %server_name,
            language_id = %language_id,
            workspace_root = %workspace_root.display(),
            "Starting language server lookup through Helix registry"
        );

        let mut servers: Vec<_> = editor
            .language_servers
            .get(
                language_config,
                Some(workspace_root),
                &root_dirs,
                true, // enable_snippets
            )
            .collect();

        let server_count = servers.len();
        info!(
            server_count = server_count,
            "Language server lookup completed"
        );

        // Restore original environment variables after server startup
        for (key, original_value) in original_env_vars {
            match original_value {
                Some(value) => {
                    // SAFETY: This is safe because we're restoring environment variables
                    // in a single-threaded context after server startup
                    unsafe {
                        std::env::set_var(&key, &value);
                    }
                }
                None => {
                    // SAFETY: This is safe because we're removing environment variables
                    // in a single-threaded context after server startup
                    unsafe {
                        std::env::remove_var(&key);
                    }
                }
            }
        }

        // Find the server with matching name
        for (name, result) in &mut servers {
            if name == server_name {
                match result {
                    Ok(client) => {
                        info!(
                            server_id = ?client.id(),
                            server_name = %name,
                            "Server started successfully via registry"
                        );
                        return Ok(client.id());
                    }
                    Err(e) => {
                        let error_msg = format!("Failed to start server: {}", e);
                        error!(error = %error_msg);

                        // Send failure event
                        let _ =
                            self.project_event_tx
                                .send(ProjectLspEvent::ServerStartupCompleted {
                                    workspace_root: workspace_root.clone(),
                                    server_name: server_name.to_string(),
                                    server_id: slotmap::KeyData::from_ffi(0).into(), // Invalid ID for failure
                                    status: ServerStartupResult::Failed {
                                        error: error_msg.clone(),
                                    },
                                });

                        return Err(ProjectLspError::ServerStartup(error_msg));
                    }
                }
            }
        }

        Err(ProjectLspError::Configuration(format!(
            "No server configuration found for: {}",
            server_name
        )))
    }

    /// Ensure document is tracked by language server
    #[instrument(skip(self, editor), fields(
        server_id = ?server_id,
        doc_id = ?doc_id
    ))]
    pub fn ensure_document_tracked(
        &self,
        editor: &mut Editor,
        server_id: LanguageServerId,
        doc_id: helix_view::DocumentId,
    ) -> Result<(), ProjectLspError> {
        debug!("Ensuring document is tracked by language server");

        // Get the language server first (immutable borrow)
        let supports_server = {
            let doc = editor
                .document(doc_id)
                .ok_or_else(|| ProjectLspError::Internal("Document not found".to_string()))?;
            doc.supports_language_server(server_id)
        };

        if supports_server {
            // Get document info before getting language server
            let (url, version, text, language_id) = {
                let doc = editor
                    .document(doc_id)
                    .ok_or_else(|| ProjectLspError::Internal("Document not found".to_string()))?;

                let url = doc.url();
                let version = doc.version();
                let text = doc.text();
                let language_id = doc.language_id().map(ToOwned::to_owned).unwrap_or_default();

                (url, version, text, language_id)
            };

            if let Some(url) = url {
                // Now get the language server
                let language_server = editor.language_server_by_id(server_id).ok_or_else(|| {
                    ProjectLspError::Internal("Language server not found".to_string())
                })?;

                language_server.text_document_did_open(url, version, text, language_id);
                debug!("Document tracking ensured");
            } else {
                warn!("Document has no URL, cannot track with LSP server");
            }
        } else {
            debug!("Document does not support this language server");
        }

        Ok(())
    }

    /// Get server capabilities for diagnostics and features
    pub fn get_server_capabilities(
        &self,
        editor: &Editor,
        server_id: LanguageServerId,
    ) -> Result<JsonValue, ProjectLspError> {
        let language_server = editor
            .language_server_by_id(server_id)
            .ok_or_else(|| ProjectLspError::Internal("Language server not found".to_string()))?;

        // Convert ServerCapabilities to JSON Value
        serde_json::to_value(language_server.capabilities()).map_err(|e| {
            ProjectLspError::Internal(format!("Failed to serialize capabilities: {}", e))
        })
    }

    /// Check if server is initialized and ready
    pub fn is_server_ready(&self, editor: &Editor, server_id: LanguageServerId) -> bool {
        editor
            .language_server_by_id(server_id)
            .map(|ls| ls.is_initialized())
            .unwrap_or(false)
    }
}

/// Helper trait for integrating ProjectLspManager with Editor
pub trait EditorLspIntegration {
    /// Get or create project LSP manager
    fn get_project_lsp_manager(&mut self) -> Option<&mut ProjectLspManager>;

    /// Detect and register project for current document
    fn detect_and_register_project(&mut self, workspace_root: PathBuf);

    /// Cleanup project when workspace closes
    fn cleanup_project(&mut self, workspace_root: &PathBuf);
}

// Note: This would be implemented as an extension to the Editor struct
// For now, we provide the interface that would be used

#[cfg(test)]
/// Mock implementation of HelixLspBridge for testing
#[derive(Clone)]
pub struct MockHelixLspBridge {
    /// Event sender for project events
    project_event_tx: broadcast::Sender<ProjectLspEvent>,
    /// Predefined responses for testing
    pub should_fail: bool,
    pub mock_server_id: Option<LanguageServerId>,
}

#[cfg(test)]
impl MockHelixLspBridge {
    /// Create a new mock bridge
    pub fn new(project_event_tx: broadcast::Sender<ProjectLspEvent>) -> Self {
        Self {
            project_event_tx,
            should_fail: false,
            mock_server_id: Some(slotmap::KeyData::from_ffi(12345).into()),
        }
    }

    /// Create a mock bridge that will fail server startup
    pub fn new_failing(project_event_tx: broadcast::Sender<ProjectLspEvent>) -> Self {
        Self {
            project_event_tx,
            should_fail: true,
            mock_server_id: None,
        }
    }

    /// Configure mock to return specific server ID
    pub fn with_server_id(mut self, server_id: LanguageServerId) -> Self {
        self.mock_server_id = Some(server_id);
        self
    }

    /// Start a language server (mock implementation)
    #[instrument(skip(self, _editor), fields(
        workspace_root = %workspace_root.display(),
        server_name = %server_name,
        language_id = %language_id
    ))]
    pub async fn start_server(
        &self,
        _editor: &mut Editor,
        workspace_root: &PathBuf,
        server_name: &str,
        language_id: &str,
    ) -> Result<LanguageServerId, ProjectLspError> {
        info!("Mock: Starting server through Helix registry");

        if self.should_fail {
            let error_msg = "Mock server startup failure".to_string();

            // Send failure event
            let _ = self
                .project_event_tx
                .send(ProjectLspEvent::ServerStartupCompleted {
                    workspace_root: workspace_root.clone(),
                    server_name: server_name.to_string(),
                    server_id: slotmap::KeyData::from_ffi(0).into(),
                    status: ServerStartupResult::Failed {
                        error: error_msg.clone(),
                    },
                });

            return Err(ProjectLspError::ServerStartup(error_msg));
        }

        let server_id = self
            .mock_server_id
            .unwrap_or_else(|| slotmap::KeyData::from_ffi(rand::random::<u64>()).into());

        // Send success event
        let _ = self
            .project_event_tx
            .send(ProjectLspEvent::ServerStartupCompleted {
                workspace_root: workspace_root.clone(),
                server_name: server_name.to_string(),
                server_id,
                status: ServerStartupResult::Success,
            });

        info!(server_id = ?server_id, "Mock: Server started successfully");
        Ok(server_id)
    }

    /// Stop a language server (mock implementation)
    #[instrument(skip(self, _editor), fields(server_id = ?server_id))]
    pub async fn stop_server(
        &self,
        _editor: &mut Editor,
        server_id: LanguageServerId,
    ) -> Result<(), ProjectLspError> {
        info!("Mock: Stopping server through Helix registry");

        if self.should_fail {
            return Err(ProjectLspError::Internal(
                "Mock server stop failure".to_string(),
            ));
        }

        info!("Mock: Server stopped successfully");
        Ok(())
    }

    /// Ensure document is tracked (mock implementation)
    #[instrument(skip(self, _editor), fields(
        server_id = ?server_id,
        doc_id = ?doc_id
    ))]
    pub fn ensure_document_tracked(
        &self,
        _editor: &mut Editor,
        server_id: LanguageServerId,
        doc_id: helix_view::DocumentId,
    ) -> Result<(), ProjectLspError> {
        debug!("Mock: Ensuring document is tracked by language server");

        if self.should_fail {
            return Err(ProjectLspError::Internal(
                "Mock document tracking failure".to_string(),
            ));
        }

        debug!("Mock: Document tracking ensured");
        Ok(())
    }

    /// Check if server is ready (mock implementation)
    pub fn is_server_ready(&self, _editor: &Editor, _server_id: LanguageServerId) -> bool {
        !self.should_fail // Mock servers are ready unless configured to fail
    }

    /// Get server capabilities (mock implementation)
    pub fn get_server_capabilities(
        &self,
        _editor: &Editor,
        _server_id: LanguageServerId,
    ) -> Result<JsonValue, ProjectLspError> {
        if self.should_fail {
            return Err(ProjectLspError::Internal(
                "Mock capabilities failure".to_string(),
            ));
        }

        // Return mock capabilities
        Ok(serde_json::json!({
            "textDocumentSync": 2,
            "completionProvider": {
                "triggerCharacters": ["."],
                "resolveProvider": true
            },
            "hoverProvider": true,
            "definitionProvider": true
        }))
    }
}
