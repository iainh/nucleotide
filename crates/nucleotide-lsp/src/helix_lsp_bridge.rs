// ABOUTME: Bridge between ProjectLspManager and Helix's LSP Registry system
// ABOUTME: Provides seamless integration without breaking existing LSP infrastructure

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use helix_lsp::{Client, LanguageServerId, LspWorkspaceContext};
use helix_view::Editor;
use nucleotide_events::{ProjectLspEvent, ServerStartupResult};
use nucleotide_logging::{debug, error, info, instrument, warn};
use nucleotide_workspace::{WorkspacePathMapping, classify_workspace_location, posix_path_string};
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

#[derive(Debug, Clone)]
pub struct LspLaunchProxy {
    pub path_dir: PathBuf,
    pub cleanup_paths: Vec<PathBuf>,
    pub description: String,
}

#[allow(clippy::type_complexity)]
pub trait LspLaunchProxyProvider: Send + Sync {
    fn create_lsp_launch_proxy(
        &self,
        workspace_root: &std::path::Path,
        server_name: &str,
        server_command: &str,
    ) -> Result<Option<LspLaunchProxy>, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, Default)]
struct LaunchProxyCleanupRegistry {
    paths: std::sync::Mutex<HashMap<LanguageServerId, Vec<PathBuf>>>,
}

impl LaunchProxyCleanupRegistry {
    fn retain(&self, server_id: LanguageServerId, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }

        match self.paths.lock() {
            Ok(mut retained_paths) => retained_paths.entry(server_id).or_default().extend(paths),
            Err(_) => cleanup_launch_proxy_paths(paths),
        }
    }

    fn release(&self, server_id: LanguageServerId) {
        let paths = self
            .paths
            .lock()
            .ok()
            .and_then(|mut retained_paths| retained_paths.remove(&server_id));
        if let Some(paths) = paths {
            cleanup_launch_proxy_paths(paths);
        }
    }
}

impl Drop for LaunchProxyCleanupRegistry {
    fn drop(&mut self) {
        let Ok(mut paths) = self.paths.lock() else {
            return;
        };
        let retained = std::mem::take(&mut *paths);
        drop(paths);
        cleanup_launch_proxy_paths(retained.into_values().flatten().collect());
    }
}

/// Bridge between ProjectLspManager and Helix's LSP system
#[derive(Clone)]
pub struct HelixLspBridge {
    /// Event sender for project events
    project_event_tx: broadcast::Sender<ProjectLspEvent>,
    /// Environment provider for LSP server startup
    environment_provider: Option<Arc<dyn EnvironmentProvider>>,
    /// Optional provider for temporary launch shims, used by remote workspaces.
    launch_proxy_provider: Option<Arc<dyn LspLaunchProxyProvider>>,
    /// Map of (workspace_root, server_name) -> LanguageServerId to scope reuse by workspace
    workspace_server_map: Arc<std::sync::Mutex<HashMap<(PathBuf, String), LanguageServerId>>>,
    /// Temporary proxy shims must outlive server startup because POSIX shebang
    /// scripts are reopened by /bin/sh after exec.
    launch_proxy_cleanup_registry: Arc<LaunchProxyCleanupRegistry>,
}

impl HelixLspBridge {
    /// Create a new bridge without environment provider (legacy)
    pub fn new(project_event_tx: broadcast::Sender<ProjectLspEvent>) -> Self {
        Self {
            project_event_tx,
            environment_provider: None,
            launch_proxy_provider: None,
            workspace_server_map: Arc::new(std::sync::Mutex::new(HashMap::new())),
            launch_proxy_cleanup_registry: Arc::new(LaunchProxyCleanupRegistry::default()),
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
            launch_proxy_provider: None,
            workspace_server_map: Arc::new(std::sync::Mutex::new(HashMap::new())),
            launch_proxy_cleanup_registry: Arc::new(LaunchProxyCleanupRegistry::default()),
        }
    }

    pub fn new_with_environment_and_launch_proxy(
        project_event_tx: broadcast::Sender<ProjectLspEvent>,
        environment_provider: Arc<dyn EnvironmentProvider>,
        launch_proxy_provider: Arc<dyn LspLaunchProxyProvider>,
    ) -> Self {
        Self {
            project_event_tx,
            environment_provider: Some(environment_provider),
            launch_proxy_provider: Some(launch_proxy_provider),
            workspace_server_map: Arc::new(std::sync::Mutex::new(HashMap::new())),
            launch_proxy_cleanup_registry: Arc::new(LaunchProxyCleanupRegistry::default()),
        }
    }

    /// Start a language server through Helix's registry
    #[instrument(skip(self, editor), fields(
        workspace_root = %workspace_root.display(),
        server_name = %server_name,
        language_id = %language_id
    ))]
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

        let environment = self.prepare_server_environment(workspace_root).await;
        self.start_server_prepared(
            editor,
            workspace_root,
            server_name,
            language_id,
            environment,
        )
    }

    /// Resolve the project environment away from the UI thread before server startup.
    pub async fn prepare_server_environment(
        &self,
        workspace_root: &Path,
    ) -> Result<Option<HashMap<String, String>>, String> {
        let Some(environment_provider) = &self.environment_provider else {
            return Ok(None);
        };

        environment_provider
            .get_lsp_environment(workspace_root)
            .await
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Complete server startup using environment data prepared asynchronously.
    pub fn start_server_prepared(
        &self,
        editor: &mut Editor,
        workspace_root: &Path,
        server_name: &str,
        language_id: &str,
        environment: Result<Option<HashMap<String, String>>, String>,
    ) -> Result<LanguageServerId, ProjectLspError> {
        if let Some(existing_server) =
            self.find_existing_server(editor, server_name, workspace_root)
        {
            return Ok(existing_server);
        }

        let server_id = self.start_server_via_registry(
            editor,
            language_id,
            workspace_root,
            server_name,
            environment,
        )?;

        // Record mapping for future scoped reuse
        {
            let mut map = self.workspace_server_map.lock().unwrap();
            map.insert(
                (workspace_root.to_path_buf(), server_name.to_string()),
                server_id,
            );
        }

        info!(server_id = ?server_id, "Server process started; awaiting initialization");
        Ok(server_id)
    }

    /// Stop a language server through Helix's registry
    #[instrument(skip(self, editor), fields(server_id = ?server_id))]
    pub fn stop_server(
        &self,
        editor: &mut Editor,
        server_id: LanguageServerId,
    ) -> Result<(), ProjectLspError> {
        info!("Stopping server through Helix registry");

        if let Some(client) = editor.language_servers.get_by_id(server_id).cloned() {
            detach_server_from_documents(editor, &client);
            client.force_shutdown();
        }

        editor.language_servers.remove_by_id(server_id);
        self.launch_proxy_cleanup_registry.release(server_id);

        // Remove any mapping entries referencing this server_id
        {
            let mut map = self.workspace_server_map.lock().unwrap();
            map.retain(|_, &mut v| v != server_id);
        }

        info!("Server stopped successfully");
        Ok(())
    }

    /// Stop every language server owned by one workspace.
    ///
    /// Removed clients are returned so the application can retain them briefly
    /// while shutdown and exit flush. Dropping them provides the force-kill
    /// fallback through Helix's child-process ownership.
    pub fn stop_workspace_servers(
        &self,
        editor: &mut Editor,
        workspace_root: &Path,
    ) -> Vec<Arc<Client>> {
        let server_ids = {
            let map = self.workspace_server_map.lock().unwrap();
            map.iter()
                .filter_map(|((root, _), server_id)| (root == workspace_root).then_some(*server_id))
                .collect::<Vec<_>>()
        };

        let mut stopped = Vec::with_capacity(server_ids.len());
        for server_id in server_ids {
            if let Some(client) = editor.language_servers.get_by_id(server_id).cloned() {
                detach_server_from_documents(editor, &client);
                client.force_shutdown();
            }
            if let Some(client) = editor.language_servers.take_by_id(server_id) {
                stopped.push(client);
            }
            self.launch_proxy_cleanup_registry.release(server_id);
        }

        self.workspace_server_map
            .lock()
            .unwrap()
            .retain(|(root, _), _| root != workspace_root);
        stopped
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
            if let Some(client) = editor.language_server_by_id(id) {
                if !cached_lsp_client_matches_workspace(
                    workspace_root,
                    client.root_path(),
                    client.root_uri(),
                ) {
                    warn!(
                        server_id = ?id,
                        server_name = %server_name,
                        workspace_root = %workspace_root.display(),
                        client_root = %client.root_path().display(),
                        client_root_uri = ?client.root_uri().map(|url| url.as_str().to_string()),
                        expected_root = ?remote_lsp_expected_native_root(workspace_root),
                        expected_root_uri = ?remote_lsp_expected_native_root_uri(workspace_root).map(|url| url.as_str().to_string()),
                        "Ignoring cached remote LSP server because its root does not match the native workspace root and URI"
                    );
                    let mut map = self.workspace_server_map.lock().unwrap();
                    map.remove(&(workspace_root.to_path_buf(), server_name.to_string()));
                    return None;
                }

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
    fn start_server_via_registry(
        &self,
        editor: &mut Editor,
        language_id: &str,
        workspace_root: &std::path::Path,
        server_name: &str,
        environment: Result<Option<HashMap<String, String>>, String>,
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
        let server_command = syntax_loader
            .language_server_configs()
            .get(server_name)
            .ok_or_else(|| {
                ProjectLspError::Configuration(format!(
                    "No server configuration found for: {}",
                    server_name
                ))
            })?
            .command
            .clone();

        let mut original_env_vars = Vec::new();
        let mut launch_proxy_cleanup_paths = Vec::new();
        let mut launch_proxy_enabled = false;

        if let Some(ref launch_proxy_provider) = self.launch_proxy_provider {
            match launch_proxy_provider.create_lsp_launch_proxy(
                workspace_root,
                server_name,
                &server_command,
            ) {
                Ok(Some(proxy)) => {
                    let original = std::env::var("PATH").ok();
                    original_env_vars.push(("PATH".to_string(), original));
                    let new_path = prepend_path_entry(&proxy.path_dir);
                    // SAFETY: This mirrors the existing launch-time environment shim. The
                    // variable is restored immediately after Helix performs server lookup.
                    unsafe { std::env::set_var("PATH", &new_path) };
                    launch_proxy_cleanup_paths = proxy.cleanup_paths;
                    launch_proxy_enabled = true;
                    info!(
                        server_name = %server_name,
                        shim_dir = %proxy.path_dir.display(),
                        proxy = %proxy.description,
                        "Enabled LSP launch proxy"
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(ProjectLspError::ServerStartup(format!(
                        "Failed to create LSP launch proxy for {server_name}: {error}"
                    )));
                }
            }
        }

        // Inject environment variables only for direct local server launches. Remote
        // launch proxies load the project environment inside nucleotide-remote so the
        // host PATH keeps transport binaries such as ssh.exe and wsl.exe visible.
        if launch_proxy_enabled {
            debug!(
                server_name = %server_name,
                workspace_root = %workspace_root.display(),
                "Skipping host environment injection for proxied LSP launch"
            );
        } else if let Ok(Some(project_env)) = environment {
            debug!("Injecting environment variables for LSP server startup");
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

            for (key, value) in &project_env {
                let original = std::env::var(key).ok();
                original_env_vars.push((key.clone(), original));

                // SAFETY: Server startup runs serially on the UI thread and restores every key.
                unsafe {
                    std::env::set_var(key, value);
                }

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
        } else if let Err(error) = environment {
            warn!(
                %error,
                workspace_root = %workspace_root.display(),
                "Failed to get project environment, using default"
            );
        } else {
            debug!("No environment provider configured, using default environment");
        }

        let remote_path_mapping = remote_lsp_path_mapping(workspace_root, launch_proxy_enabled);
        let lsp_workspace_root = remote_path_mapping
            .as_ref()
            .map(|mapping| PathBuf::from(posix_path_string(mapping.native_root())))
            .unwrap_or_else(|| workspace_root.to_path_buf());

        // Create root directories for server startup. For remote launch proxies
        // these are the native remote roots sent in initialize, not display URIs.
        let root_dirs = if language_id == "rust" {
            rust_root_dirs(&lsp_workspace_root)
        } else {
            vec![lsp_workspace_root.clone()]
        };

        let lsp_workspace_context = Some(if remote_path_mapping.is_some() {
            LspWorkspaceContext::remote(lsp_workspace_root.clone(), host_process_cwd())
        } else {
            LspWorkspaceContext::new(lsp_workspace_root.clone(), true, lsp_workspace_root.clone())
        });

        if let Some(context) = &lsp_workspace_context {
            info!(
                display_workspace_root = %workspace_root.display(),
                lsp_workspace_root = %context.workspace_root.display(),
                process_cwd = %context.process_cwd.display(),
                "Using explicit remote LSP workspace context"
            );
        };

        // Optionally wrap the server launch through our stdio proxy by PATH shimming.
        // Controlled via env var NUCLEOTIDE_LSP_USE_PROXY=1.
        let mut shim_dir_to_cleanup: Option<std::path::PathBuf> = None;
        if std::env::var("NUCLEOTIDE_LSP_USE_PROXY")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        {
            // Best-effort: build a shim dir with an executable named `server_name` that execs our proxy.
            if let Ok(real_path) = which::which(server_name) {
                use std::fs;
                use std::io::Write as _;
                let shim_dir =
                    std::env::temp_dir().join(format!("nuc-lsp-shims-{}", std::process::id()));
                let _ = fs::create_dir_all(&shim_dir);
                let shim_path = shim_dir.join(server_name);

                let log_dir = std::path::Path::new("logs").join("lsp");
                let _ = fs::create_dir_all(&log_dir);
                let log_file = log_dir.join(format!(
                    "proxy-{}-{}.jsonl",
                    server_name,
                    chrono::Utc::now().timestamp_millis()
                ));

                // Write a simple POSIX shell wrapper
                let script = format!(
                    "#!/bin/sh\nexec nucleotide-lsp-proxy --server-cmd '{}' --log '{}' -- \"$@\"\n",
                    real_path.display(),
                    log_file.display()
                );
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
                let new_path = prepend_path_entry(&shim_dir);
                unsafe { std::env::set_var("PATH", &new_path) };
                shim_dir_to_cleanup = Some(shim_dir);
                info!(
                    server_name = %server_name,
                    shim_dir = %shim_dir_to_cleanup.as_ref().unwrap().display(),
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

        // Project startup has an explicit workspace context and does not depend
        // on a representative or already-open document.
        let doc_path: Option<PathBuf> = None;

        info!(
            doc_path = ?doc_path,
            workspace_root = %workspace_root.display(),
            lsp_workspace_root = %lsp_workspace_root.display(),
            language_id = %language_id,
            "Using explicit workspace context for LSP initialization"
        );

        // Keep all detected workspace roots to support multi-crate workspaces.
        // This allows rust-analyzer to serve files across all member crates.

        let server = editor.language_servers.get_named_with_workspace_context(
            language_config,
            server_name,
            doc_path.as_deref(),
            &root_dirs,
            true, // enable_snippets
            lsp_workspace_context.as_ref(),
        );

        let server_count = usize::from(server.is_some());
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
            let _ = std::fs::remove_dir(dir);
        }

        match server {
            Some(Ok(client)) => {
                if !launch_proxy_cleanup_paths.is_empty() {
                    debug!(
                        server_id = ?client.id(),
                        server_name = %server_name,
                        retained_paths = launch_proxy_cleanup_paths.len(),
                        "Retaining LSP launch proxy shims for active server"
                    );
                    self.launch_proxy_cleanup_registry
                        .retain(client.id(), std::mem::take(&mut launch_proxy_cleanup_paths));
                }
                info!(
                    server_id = ?client.id(),
                    server_name = %server_name,
                    "Server started successfully via registry"
                );
                Ok(client.id())
            }
            Some(Err(e)) => {
                cleanup_launch_proxy_paths(std::mem::take(&mut launch_proxy_cleanup_paths));
                let error_msg = format!("Failed to start server: {}", e);
                error!(error = %error_msg);

                // Send failure event
                let _ = self
                    .project_event_tx
                    .send(ProjectLspEvent::ServerStartupCompleted {
                        workspace_root: workspace_root.to_path_buf(),
                        server_name: server_name.to_string(),
                        server_id: slotmap::KeyData::from_ffi(0).into(), // Invalid ID for failure
                        status: ServerStartupResult::Failed {
                            error: error_msg.clone(),
                        },
                    });

                Err(ProjectLspError::ServerStartup(error_msg))
            }
            None => {
                cleanup_launch_proxy_paths(launch_proxy_cleanup_paths);
                Err(ProjectLspError::Configuration(format!(
                    "No server configuration found for: {}",
                    server_name
                )))
            }
        }
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

        if editor.ensure_document_tracked_by_language_server(doc_id, server_id) {
            debug!("Document tracking ensured");
        } else if editor.document(doc_id).is_none() {
            return Err(ProjectLspError::Internal("Document not found".to_string()));
        } else {
            warn!(
                doc_id = ?doc_id,
                server_id = ?server_id,
                "Document cannot be tracked by language server"
            );
            return Err(ProjectLspError::Internal(
                "Document cannot be tracked by language server".to_string(),
            ));
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

fn detach_server_from_documents(editor: &mut Editor, client: &Arc<Client>) {
    for document in editor.documents.values_mut() {
        let identifier = document.url().map(|_| document.identifier());
        let attached = document
            .remove_language_server_by_name(client.name())
            .filter(|attached| attached.id() == client.id());
        if attached.is_some()
            && let Some(identifier) = identifier
        {
            client.text_document_did_close(identifier);
        }
        document.clear_diagnostics_for_language_server(client.id());
    }
}

fn prepend_path_entry(path_entry: &Path) -> String {
    let mut paths = vec![path_entry.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }

    std::env::join_paths(paths)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path_entry.display().to_string())
}

fn cleanup_launch_proxy_paths(paths: Vec<PathBuf>) {
    for path in paths {
        if path.is_dir() {
            let _ = std::fs::remove_dir(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn remote_lsp_path_mapping(
    workspace_root: &std::path::Path,
    launch_proxy_enabled: bool,
) -> Option<WorkspacePathMapping> {
    if !launch_proxy_enabled {
        return None;
    }

    let location = classify_workspace_location(workspace_root);
    location.is_remote().then(|| location.path_mapping())
}

fn remote_lsp_native_root(workspace_root: &std::path::Path) -> Option<PathBuf> {
    let location = classify_workspace_location(workspace_root);
    location
        .is_remote()
        .then(|| PathBuf::from(posix_path_string(location.native_root())))
}

#[cfg(test)]
fn lsp_document_path_for_launch(
    path: PathBuf,
    remote_path_mapping: Option<&WorkspacePathMapping>,
) -> PathBuf {
    remote_path_mapping
        .map(|mapping| PathBuf::from(posix_path_string(mapping.to_native_path(&path))))
        .unwrap_or(path)
}

fn host_process_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir())
}

fn remote_lsp_expected_native_root(workspace_root: &std::path::Path) -> Option<PathBuf> {
    remote_lsp_native_root(workspace_root)
}

fn remote_lsp_expected_native_root_uri(workspace_root: &std::path::Path) -> Option<helix_lsp::Url> {
    remote_lsp_expected_native_root(workspace_root).and_then(|root| remote_native_file_uri(&root))
}

fn remote_native_file_uri(path: &Path) -> Option<helix_lsp::Url> {
    helix_lsp::file_uri_from_path(Path::new(&posix_path_string(path)))
}

fn cached_lsp_client_matches_workspace(
    workspace_root: &std::path::Path,
    client_root: &Path,
    client_root_uri: Option<&helix_lsp::Url>,
) -> bool {
    let Some(expected_root) = remote_lsp_expected_native_root(workspace_root) else {
        return true;
    };
    if posix_path_string(client_root) != posix_path_string(&expected_root) {
        return false;
    }

    client_root_uri
        .map(|uri| uri.as_str())
        .zip(remote_lsp_expected_native_root_uri(workspace_root).map(|uri| uri.to_string()))
        .is_some_and(|(client_uri, expected_uri)| client_uri == expected_uri)
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

/// For Rust, prefer a single workspace root and let rust-analyzer expand the workspace members.
fn rust_root_dirs(workspace_root: &std::path::Path) -> Vec<PathBuf> {
    vec![workspace_root.to_path_buf()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestEnvironmentProvider;

    impl EnvironmentProvider for TestEnvironmentProvider {
        fn get_lsp_environment(
            &self,
            _directory: &Path,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            HashMap<String, String>,
                            Box<dyn std::error::Error + Send + Sync>,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Ok(HashMap::from([(
                    "PATH".to_string(),
                    "/test/bin".to_string(),
                )]))
            })
        }
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "nucleotide-lsp-{name}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[tokio::test]
    async fn server_environment_can_be_prepared_without_an_editor() {
        let (event_tx, _) = broadcast::channel(4);
        let bridge =
            HelixLspBridge::new_with_environment(event_tx, Arc::new(TestEnvironmentProvider));

        let environment = bridge
            .prepare_server_environment(Path::new("/workspace"))
            .await
            .expect("prepared environment")
            .expect("environment provider");

        assert_eq!(
            environment.get("PATH").map(String::as_str),
            Some("/test/bin")
        );
    }

    #[test]
    fn remote_lsp_path_mapping_uses_native_root_for_proxied_ssh_launches() {
        let display_root = PathBuf::from("ssh://me@example.com/home/me/project");

        let mapping = remote_lsp_path_mapping(&display_root, true).expect("remote mapping");
        let root_dirs = rust_root_dirs(mapping.native_root());
        let display_doc = display_root.join("src").join("main.rs");
        let lsp_doc = lsp_document_path_for_launch(display_doc, Some(&mapping));

        assert_eq!(mapping.display_root(), display_root.as_path());
        assert_eq!(mapping.native_root(), Path::new("/home/me/project"));
        assert_eq!(root_dirs, vec![PathBuf::from("/home/me/project")]);
        assert_eq!(lsp_doc, PathBuf::from("/home/me/project/src/main.rs"));
    }

    #[test]
    fn remote_lsp_path_mapping_is_none_without_launch_proxy() {
        let display_root = PathBuf::from("ssh://me@example.com/home/me/project");

        assert!(remote_lsp_path_mapping(&display_root, false).is_none());
    }

    #[test]
    fn launch_proxy_cleanup_registry_releases_server_paths() {
        let temp = TestDir::new("launch-proxy-cleanup");
        let shim_dir = temp.path().join("shims");
        let shim_path = shim_dir.join("rust-analyzer");
        std::fs::create_dir_all(&shim_dir).unwrap();
        std::fs::write(&shim_path, "#!/bin/sh\n").unwrap();

        {
            let registry = LaunchProxyCleanupRegistry::default();
            let server_id = slotmap::KeyData::from_ffi(42).into();
            registry.retain(server_id, vec![shim_path.clone(), shim_dir.clone()]);

            assert!(shim_path.exists());
            assert!(shim_dir.exists());

            registry.release(server_id);
            assert!(!shim_path.exists());
            assert!(!shim_dir.exists());
        }

        assert!(!shim_path.exists());
        assert!(!shim_dir.exists());
    }

    #[test]
    fn cached_lsp_root_matches_remote_native_root_only() {
        let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
        let native_uri = helix_lsp::Url::parse("file:///home/me/project").unwrap();
        let display_uri =
            helix_lsp::Url::parse("file:///ssh%3A/me@example.com/home/me/project").unwrap();

        assert!(cached_lsp_client_matches_workspace(
            &display_root,
            Path::new("/home/me/project"),
            Some(&native_uri)
        ));
        assert!(!cached_lsp_client_matches_workspace(
            &display_root,
            Path::new("/home/me/project"),
            None
        ));
        assert!(!cached_lsp_client_matches_workspace(
            &display_root,
            Path::new("/home/me/project"),
            Some(&display_uri)
        ));
        assert!(!cached_lsp_client_matches_workspace(
            &display_root,
            display_root.as_path(),
            Some(&native_uri)
        ));
        assert!(!cached_lsp_client_matches_workspace(
            &display_root,
            Path::new("/Users/me/projects/nucleotide"),
            Some(&native_uri)
        ));
    }

    #[test]
    fn remote_lsp_expected_root_uri_uses_posix_file_uri() {
        let display_root = PathBuf::from("ssh://me@example.com/home/me/Project One");

        let uri = remote_lsp_expected_native_root_uri(&display_root).unwrap();

        assert_eq!(uri.as_str(), "file:///home/me/Project%20One");
    }

    #[test]
    fn cached_lsp_root_accepts_local_workspace_roots() {
        assert!(cached_lsp_client_matches_workspace(
            Path::new("/Users/me/project"),
            Path::new("/Users/me/project"),
            None
        ));
        assert!(cached_lsp_client_matches_workspace(
            Path::new("/Users/me/project"),
            Path::new("/tmp/other-root"),
            None
        ));
    }
}
