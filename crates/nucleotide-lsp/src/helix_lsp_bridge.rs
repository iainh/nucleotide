// ABOUTME: Bridge between ProjectLspManager and Helix's LSP Registry system
// ABOUTME: Provides seamless integration without breaking existing LSP infrastructure

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use helix_lsp::LanguageServerId;
use helix_view::Editor;
use nucleotide_env::WslWorkspace;
use nucleotide_events::{ProjectLspEvent, ServerStartupResult};
use nucleotide_logging::{debug, error, info, instrument, warn};
use serde_json::Value as JsonValue;
use tokio::sync::broadcast;

use crate::{ProjectLspError, ProjectLspManager};

// Define a dyn-compatible trait for environment providers using boxed futures
#[allow(clippy::type_complexity)]
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
    /// Map of (workspace_root, server_name) -> LanguageServerId to scope reuse by workspace
    workspace_server_map: Arc<std::sync::Mutex<HashMap<(PathBuf, String), LanguageServerId>>>,
}

impl HelixLspBridge {
    /// Create a new bridge without environment provider (legacy)
    pub fn new(project_event_tx: broadcast::Sender<ProjectLspEvent>) -> Self {
        Self {
            project_event_tx,
            environment_provider: None,
            workspace_server_map: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Send a custom JSON-RPC notification to a server. Prototype: returns Unsupported until helix_lsp exposes it.
    #[instrument(skip(self, editor), fields(server_id = ?server_id, method = %method))]
    pub async fn send_custom_notification(
        &self,
        editor: &mut Editor,
        server_id: LanguageServerId,
        method: &str,
        params: JsonValue,
    ) -> Result<(), ProjectLspError> {
        // Try to find the target server for logging context
        if let Some(ls) = editor.language_server_by_id(server_id) {
            info!(
                server_name = ls.name(),
                "Attempting custom LSP notification (prototype)"
            );
        } else {
            return Err(ProjectLspError::ServerCommunication(
                "Target language server not found".to_string(),
            ));
        }

        // Currently helix_lsp does not expose a generic custom notify surface in our dependency.
        // Return a clear error so callers know this path requires upstream support.
        Err(ProjectLspError::ServerCommunication(format!(
            "Custom notification '{}' not supported by helix-lsp (prototype)",
            method
        )))
    }

    /// Send a custom JSON-RPC request to a server and await a raw JSON response. Prototype stub.
    #[instrument(skip(self, editor), fields(server_id = ?server_id, method = %method))]
    pub async fn send_custom_request(
        &self,
        editor: &mut Editor,
        server_id: LanguageServerId,
        method: &str,
        params: JsonValue,
    ) -> Result<JsonValue, ProjectLspError> {
        if let Some(ls) = editor.language_server_by_id(server_id) {
            info!(
                server_name = ls.name(),
                "Attempting custom LSP request (prototype)"
            );
        } else {
            return Err(ProjectLspError::ServerCommunication(
                "Target language server not found".to_string(),
            ));
        }

        Err(ProjectLspError::ServerCommunication(format!(
            "Custom request '{}' not supported by helix-lsp (prototype)",
            method
        )))
    }
    /// Create a new bridge with environment provider for dynamic environment injection
    pub fn new_with_environment(
        project_event_tx: broadcast::Sender<ProjectLspEvent>,
        environment_provider: Arc<dyn EnvironmentProvider>,
    ) -> Self {
        Self {
            project_event_tx,
            environment_provider: Some(environment_provider),
            workspace_server_map: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Start a language server through Helix's registry
    #[instrument(skip(self, editor), fields(
        workspace_root = %workspace_root.display(),
        server_name = %server_name,
        language_id = %language_id
    ))]
    #[allow(clippy::ptr_arg)]
    #[allow(clippy::ptr_arg)]
    pub async fn start_server(
        &self,
        editor: &mut Editor,
        workspace_root: &std::path::Path,
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

        // Record mapping for future scoped reuse
        {
            let mut map = self.workspace_server_map.lock().unwrap();
            map.insert(
                (workspace_root.to_path_buf(), server_name.to_string()),
                server_id,
            );
        }

        // Send success event
        let _ = self
            .project_event_tx
            .send(ProjectLspEvent::ServerStartupCompleted {
                workspace_root: workspace_root.to_path_buf(),
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

        // Remove any mapping entries referencing this server_id
        {
            let mut map = self.workspace_server_map.lock().unwrap();
            map.retain(|_, &mut v| v != server_id);
        }

        info!("Server stopped successfully");
        Ok(())
    }

    /// Find existing server for workspace and server name
    #[allow(clippy::ptr_arg)]
    fn find_existing_server(
        &self,
        editor: &Editor,
        server_name: &str,
        workspace_root: &std::path::Path,
    ) -> Option<LanguageServerId> {
        // First, consult our scoped map to ensure reuse only within the same workspace
        if let Some(&id) = self
            .workspace_server_map
            .lock()
            .unwrap()
            .get(&(workspace_root.to_path_buf(), server_name.to_string()))
        {
            // Verify the client is still alive in the editor
            if editor.language_server_by_id(id).is_some() {
                return Some(id);
            } else {
                // Clean up stale mapping
                let mut map = self.workspace_server_map.lock().unwrap();
                map.remove(&(workspace_root.to_path_buf(), server_name.to_string()));
            }
        }

        // If future helix_lsp exposes workdir/workspace_folders, we could fall back to checking those here.
        None
    }

    /// Start server via Helix's registry system
    #[allow(clippy::ptr_arg)]
    async fn start_server_via_registry(
        &self,
        editor: &mut Editor,
        language_id: &str,
        workspace_root: &std::path::Path,
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

        let wsl_workspace = WslWorkspace::from_unc_path(workspace_root);

        // Inject environment variables if environment provider is available
        let mut original_env_vars = Vec::new();

        if let Some(ref env_provider) = self.environment_provider {
            debug!("Injecting environment variables for LSP server startup");

            match env_provider.get_lsp_environment(workspace_root).await {
                Ok(project_env) => {
                    info!(
                        env_count = project_env.len(),
                        workspace_root = %workspace_root.display(),
                        home = %project_env.get("HOME").map(String::as_str).unwrap_or("<unset>"),
                        cargo_home = %project_env.get("CARGO_HOME").map(String::as_str).unwrap_or("<unset>"),
                        xdg_cache_home = %project_env.get("XDG_CACHE_HOME").map(String::as_str).unwrap_or("<unset>"),
                        xdg_config_home = %project_env.get("XDG_CONFIG_HOME").map(String::as_str).unwrap_or("<unset>"),
                        xdg_data_home = %project_env.get("XDG_DATA_HOME").map(String::as_str).unwrap_or("<unset>"),
                        xdg_state_home = %project_env.get("XDG_STATE_HOME").map(String::as_str).unwrap_or("<unset>"),
                        "Successfully retrieved project environment for LSP server"
                    );

                    if should_inject_project_env_into_process(wsl_workspace.as_ref(), &project_env)
                    {
                        // TEMPORARY SOLUTION: Set environment variables in the current process
                        // This works because Helix will inherit the environment when starting native servers
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
                            if key == "PATH"
                                || key == "HOME"
                                || key == "CARGO_HOME"
                                || key == "XDG_CACHE_HOME"
                                || key == "XDG_CONFIG_HOME"
                                || key == "XDG_DATA_HOME"
                                || key == "XDG_STATE_HOME"
                                || key == "RUSTC"
                                || key == "CARGO"
                            {
                                debug!(key = %key, value = %value, "Set environment variable for LSP server");
                            }
                        }

                        info!(
                            "Temporarily set {} environment variables for LSP server startup",
                            project_env.len()
                        );
                    } else {
                        info!(
                            workspace_root = %workspace_root.display(),
                            "Skipping process environment injection for remote LSP startup"
                        );
                    }
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
        // For Rust, prefer a single Cargo workspace root and let rust-analyzer expand members
        let root_dirs = if language_id == "rust" {
            rust_root_dirs(workspace_root)
        } else {
            vec![workspace_root.to_path_buf()]
        };

        // Optionally wrap the server launch through our stdio proxy by PATH shimming.
        // Controlled via env var NUCLEOTIDE_LSP_USE_PROXY=1 for native servers.
        // WSL workspaces use the proxy automatically so URI/path mapping is transparent.
        let mut shim_dir_to_cleanup: Option<std::path::PathBuf> = None;
        let proxy_requested = std::env::var("NUCLEOTIDE_LSP_USE_PROXY")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        if proxy_requested || wsl_workspace.is_some() {
            // Best-effort: build a shim dir with an executable named `server_name` that execs our proxy.
            if wsl_workspace.is_some() || which::which(server_name).is_ok() {
                use std::fs;
                use std::io::Write as _;
                let shim_dir =
                    std::env::temp_dir().join(format!("nuc-lsp-shims-{}", std::process::id()));
                let _ = fs::create_dir_all(&shim_dir);
                let shim_path = shim_path_for_server(&shim_dir, server_name);

                let log_dir = std::path::Path::new("logs").join("lsp");
                let _ = fs::create_dir_all(&log_dir);
                let log_file = log_dir.join(format!(
                    "proxy-{}-{}.jsonl",
                    server_name,
                    chrono::Utc::now().timestamp_millis()
                ));

                let script = if let Some(wsl_workspace) = &wsl_workspace {
                    wsl_proxy_shim_script(server_name, &log_file, wsl_workspace, workspace_root)
                } else {
                    match which::which(server_name) {
                        Ok(real_path) => native_proxy_shim_script(&real_path, &log_file),
                        Err(error) => {
                            warn!(
                                server_name = %server_name,
                                error = %error,
                                "LSP proxy enabled but real server was not found in PATH"
                            );
                            String::new()
                        }
                    }
                };

                if let Ok(mut f) = fs::File::create(&shim_path) {
                    let _ = f.write_all(script.as_bytes());
                    let _ = f.flush();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755));
                    }
                }

                // Prepend to PATH via our temporary env injection mechanism
                let original = std::env::var("PATH").ok();
                original_env_vars.push(("PATH".to_string(), original));
                if let Some(new_path) = path_with_prepended_dir(&shim_dir) {
                    unsafe { std::env::set_var("PATH", &new_path) };
                }
                shim_dir_to_cleanup = Some(shim_dir);
                info!(
                    server_name = %server_name,
                    shim_dir = %shim_dir_to_cleanup.as_ref().unwrap().display(),
                    is_wsl = wsl_workspace.is_some(),
                    "Enabled LSP proxy via PATH shim"
                );
            } else {
                warn!(server_name = %server_name, "NUCLEOTIDE_LSP_USE_PROXY enabled but real server not found in PATH");
            }
        }

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

        // Representative document for server lookup
        // For Rust, avoid using Cargo.toml; prefer an active .rs or a conventional entry point, else None
        let doc_path = if language_id == "rust" {
            find_active_rust_document(editor, workspace_root).or_else(|| {
                for root in &root_dirs {
                    if let Some(rs) = find_rs_file_shallow(root, 3) {
                        return Some(rs);
                    }
                }
                None
            })
        } else {
            find_representative_file(workspace_root, language_id)
        };

        info!(
            doc_path = ?doc_path,
            workspace_root = %workspace_root.display(),
            language_id = %language_id,
            "Representative file found for LSP initialization"
        );

        // Keep all detected workspace roots to support multi-crate workspaces.
        // This allows rust-analyzer to serve files across all member crates.

        let mut servers: Vec<_> = editor
            .language_servers
            .get(
                language_config,
                doc_path.as_ref(),
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

        // Best-effort cleanup of shims (directory may remain if in use)
        if let Some(dir) = shim_dir_to_cleanup {
            let _ = std::fs::remove_file(dir.join(server_name));
            let _ = std::fs::remove_file(shim_path_for_server(&dir, server_name));
            let _ = std::fs::remove_dir(dir);
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
                                    workspace_root: workspace_root.to_path_buf(),
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
                // Prefer explicit language_id; fall back to lowercased language_name if needed
                let language_id = doc
                    .language_id()
                    .map(ToOwned::to_owned)
                    .or_else(|| doc.language_name().map(|s| s.to_ascii_lowercase()))
                    .unwrap_or_default();

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
    #[allow(clippy::ptr_arg)]
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
        workspace_root: &std::path::Path,
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
                    workspace_root: workspace_root.to_path_buf(),
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
                workspace_root: workspace_root.to_path_buf(),
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

fn shim_path_for_server(shim_dir: &std::path::Path, server_name: &str) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        shim_dir.join(format!("{server_name}.cmd"))
    }

    #[cfg(not(windows))]
    {
        shim_dir.join(server_name)
    }
}

fn should_inject_project_env_into_process(
    wsl_workspace: Option<&WslWorkspace>,
    project_env: &HashMap<String, String>,
) -> bool {
    wsl_workspace.is_none()
        && !matches!(
            project_env
                .get("NUCLEOTIDE_REMOTE_KIND")
                .map(String::as_str),
            Some("wsl")
        )
}

fn native_proxy_shim_script(real_path: &std::path::Path, log_file: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        format!(
            "@echo off\r\nnucleotide-lsp-proxy --server-cmd {} --log {} -- %*\r\n",
            quote_cmd_arg(&real_path.display().to_string()),
            quote_cmd_arg(&log_file.display().to_string())
        )
    }

    #[cfg(not(windows))]
    {
        format!(
            "#!/bin/sh\nexec nucleotide-lsp-proxy --server-cmd '{}' --log '{}' -- \"$@\"\n",
            quote_posix_single(real_path),
            quote_posix_single(log_file)
        )
    }
}

fn wsl_proxy_shim_script(
    server_name: &str,
    log_file: &std::path::Path,
    workspace: &WslWorkspace,
    windows_root: &std::path::Path,
) -> String {
    #[cfg(windows)]
    {
        format!(
            "@echo off\r\nnucleotide-lsp-proxy --server-cmd wsl.exe --log {} --wsl-distro {} --wsl-linux-root {} --wsl-windows-root {} -- --distribution {} --cd {} -- {} %*\r\n",
            quote_cmd_arg(&log_file.display().to_string()),
            quote_cmd_arg(workspace.distro()),
            quote_cmd_arg(workspace.linux_path()),
            quote_cmd_arg(&windows_root.display().to_string()),
            quote_cmd_arg(workspace.distro()),
            quote_cmd_arg(workspace.linux_path()),
            quote_cmd_arg(server_name)
        )
    }

    #[cfg(not(windows))]
    {
        format!(
            "#!/bin/sh\nexec nucleotide-lsp-proxy --server-cmd wsl.exe --log '{}' --wsl-distro '{}' --wsl-linux-root '{}' --wsl-windows-root '{}' -- --distribution '{}' --cd '{}' -- '{}' \"$@\"\n",
            quote_posix_single(log_file),
            workspace.distro().replace('\'', "'\"'\"'"),
            workspace.linux_path().replace('\'', "'\"'\"'"),
            windows_root.display().to_string().replace('\'', "'\"'\"'"),
            workspace.distro().replace('\'', "'\"'\"'"),
            workspace.linux_path().replace('\'', "'\"'\"'"),
            server_name.replace('\'', "'\"'\"'")
        )
    }
}

#[cfg(windows)]
fn quote_cmd_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('%', "%%").replace('"', "\"\""))
}

#[cfg(not(windows))]
fn quote_posix_single(path: &std::path::Path) -> String {
    path.display().to_string().replace('\'', "'\"'\"'")
}

fn path_with_prepended_dir(shim_dir: &std::path::Path) -> Option<std::ffi::OsString> {
    let mut paths = vec![shim_dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).ok()
}

/// Find a representative file within the workspace that rust-analyzer can use
/// to determine the proper workspace root and configuration
fn find_representative_file(
    workspace_root: &std::path::Path,
    language_id: &str,
) -> Option<PathBuf> {
    // For Rust projects, try to find common files that rust-analyzer can use
    if language_id == "rust" {
        // Try src/main.rs
        let main_rs = workspace_root.join("src").join("main.rs");
        if main_rs.exists() && main_rs.is_file() {
            return Some(main_rs);
        }

        // Try src/lib.rs
        let lib_rs = workspace_root.join("src").join("lib.rs");
        if lib_rs.exists() && lib_rs.is_file() {
            return Some(lib_rs);
        }

        // Try to find any .rs file in src/ or shallow subdirectories (limited depth)
        if let Some(found) = find_rs_file_shallow(workspace_root, 2) {
            return Some(found);
        }
    }

    // For other languages, add similar logic here
    // For now, fall back to the old behavior for non-Rust languages
    None
}

/// For Rust, prefer a single workspace root and let rust-analyzer expand the workspace members.
fn rust_root_dirs(workspace_root: &std::path::Path) -> Vec<PathBuf> {
    vec![workspace_root.to_path_buf()]
}

// removed: cargo_toml_has_workspace (no longer used)

/// Find any .rs file within the directory up to a limited depth to use as a representative file.
fn find_rs_file_shallow(root: &std::path::Path, max_depth: usize) -> Option<PathBuf> {
    // Prefer conventional entry points if present
    let lib_rs = root.join("src").join("lib.rs");
    if lib_rs.is_file() {
        return Some(lib_rs);
    }
    let main_rs = root.join("src").join("main.rs");
    if main_rs.is_file() {
        return Some(main_rs);
    }

    fn walk(dir: &std::path::Path, depth: usize, max_depth: usize) -> Option<PathBuf> {
        if depth > max_depth {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("rs") {
                return Some(path);
            } else if path.is_dir() {
                // Skip common large/irrelevant directories
                if let Some(name) = path.file_name().and_then(|s| s.to_str())
                    && matches!(name, "target" | ".git" | "node_modules" | ".cache")
                {
                    continue;
                }
                if let Some(found) = walk(&path, depth + 1, max_depth) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(root, 0, max_depth)
}
/// Try to pick the currently active Rust document within the given workspace root
fn find_active_rust_document(editor: &Editor, workspace_root: &std::path::Path) -> Option<PathBuf> {
    // Prefer the focused view if available, otherwise scan visible views
    // Fallback: first Rust document whose path is inside the workspace root
    // Note: We intentionally avoid borrowing the editor mutably here

    // Helper to validate a document path
    let is_candidate = |path: &std::path::Path| -> bool {
        path.extension().map(|e| e == "rs").unwrap_or(false) && path.starts_with(workspace_root)
    };

    // 1) Try focused view first
    let focused = editor.tree.focus;
    if editor.tree.contains(focused) {
        let view = editor.tree.get(focused);
        if let Some(doc) = editor.documents.get(&view.doc)
            && let Some(path) = doc.path()
            && is_candidate(path)
        {
            return Some(path.to_path_buf());
        }
    }

    // 2) Fall back to any open view within the workspace
    for (view_ref, _) in editor.tree.views() {
        let view = editor.tree.get(view_ref.id);
        if let Some(doc) = editor.documents.get(&view.doc)
            && let Some(path) = doc.path()
            && is_candidate(path)
        {
            return Some(path.to_path_buf());
        }
    }

    None
}

// removed unused helper find_cargo_root_for to eliminate dead code warnings

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn proxy_shim_path_uses_platform_executable_name() {
        let path = shim_path_for_server(Path::new("C:\\Temp\\shims"), "rust-analyzer");

        #[cfg(windows)]
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("rust-analyzer.cmd")
        );

        #[cfg(not(windows))]
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("rust-analyzer")
        );
    }

    #[test]
    fn wsl_proxy_shim_script_contains_mapping_arguments() {
        let workspace =
            WslWorkspace::from_unc_path(Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo"))
                .expect("expected WSL workspace");
        let script = wsl_proxy_shim_script(
            "rust-analyzer",
            Path::new("logs/lsp/proxy.jsonl"),
            &workspace,
            Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo"),
        );

        assert!(script.contains("nucleotide-lsp-proxy"));
        assert!(script.contains("--server-cmd"));
        assert!(script.contains("wsl.exe"));
        assert!(script.contains("--wsl-distro"));
        assert!(script.contains("Ubuntu"));
        assert!(script.contains("--wsl-linux-root"));
        assert!(script.contains("/home/iain/repo"));
        assert!(script.contains("--wsl-windows-root"));
        assert!(script.contains("rust-analyzer"));
    }

    #[test]
    fn lsp_process_env_injection_skips_wsl_workspaces() {
        let workspace =
            WslWorkspace::from_unc_path(Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo"))
                .expect("expected WSL workspace");
        let env = HashMap::from([("PATH".to_string(), "/usr/bin".to_string())]);

        assert!(!should_inject_project_env_into_process(
            Some(&workspace),
            &env
        ));
    }

    #[test]
    fn lsp_process_env_injection_skips_wsl_tagged_snapshots() {
        let env = HashMap::from([
            ("NUCLEOTIDE_REMOTE_KIND".to_string(), "wsl".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ]);

        assert!(!should_inject_project_env_into_process(None, &env));
    }

    #[test]
    fn lsp_process_env_injection_keeps_native_snapshots() {
        let env = HashMap::from([("PATH".to_string(), "/usr/bin".to_string())]);

        assert!(should_inject_project_env_into_process(None, &env));
    }

    #[test]
    fn native_proxy_shim_script_forwards_original_args() {
        let script = native_proxy_shim_script(
            Path::new("C:\\Tools\\rust-analyzer.exe"),
            Path::new("logs/lsp/proxy.jsonl"),
        );

        assert!(script.contains("nucleotide-lsp-proxy"));
        assert!(script.contains("--server-cmd"));

        #[cfg(windows)]
        assert!(script.contains("%*"));

        #[cfg(not(windows))]
        assert!(script.contains("\"$@\""));
    }

    #[test]
    #[cfg(windows)]
    fn windows_proxy_shim_escapes_percent_literals() {
        let script = native_proxy_shim_script(
            Path::new(r"C:\Tools\%LSP_HOME%\rust-analyzer.exe"),
            Path::new(r"logs\lsp\proxy-%USERNAME%.jsonl"),
        );

        assert!(script.contains(r"C:\Tools\%%LSP_HOME%%\rust-analyzer.exe"));
        assert!(script.contains(r"logs\lsp\proxy-%%USERNAME%%.jsonl"));
    }
}
