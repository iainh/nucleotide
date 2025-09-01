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
        // For Rust, prefer Cargo workspace roots or nested Cargo.toml directories
        let root_dirs = if language_id == "rust" {
            let rust_roots = find_rust_workspace_roots(workspace_root);
            if !rust_roots.is_empty() {
                info!(
                    count = rust_roots.len(),
                    "Using detected Rust workspace roots"
                );
                rust_roots
            } else {
                vec![workspace_root.clone()]
            }
        } else {
            vec![workspace_root.clone()]
        };

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

        // Find a representative file within the workspace for rust-analyzer to determine proper workspace root
        // This is critical - Helix expects a file path, not the workspace root directory
        //
        // IMPORTANT: If we detected a Cargo workspace (i.e. multiple roots including the workspace root),
        // prefer the workspace Cargo.toml as the representative file. This ensures rust-analyzer initializes
        // at the workspace root instead of a nested crate, avoiding overlapping roots and missing completions.
        let doc_path = if language_id == "rust" {
            let workspace_manifest = workspace_root.join("Cargo.toml");
            let is_workspace =
                workspace_manifest.is_file() && cargo_toml_has_workspace(&workspace_manifest);

            if is_workspace && root_dirs.len() > 1 {
                // Force workspace-level initialization for rust-analyzer
                Some(workspace_manifest)
            } else {
                // First, prefer the currently active Rust document (so the chosen root matches the open file)
                if let Some(active_rs) = find_active_rust_document(editor, workspace_root) {
                    Some(active_rs)
                } else {
                    // Prefer a Rust source file from one of the detected roots; fall back as needed
                    // Skip choosing from the bare workspace root if we also detected member roots.
                    let mut candidate: Option<PathBuf> = None;
                    for root in &root_dirs {
                        // If this is the workspace root and we have more than one root, skip it for representative file selection
                        if root == workspace_root && root_dirs.len() > 1 {
                            continue;
                        }
                        if let Some(rs) = find_rs_file_shallow(root, 3) {
                            candidate = Some(rs);
                            break;
                        }
                    }
                    // As a very last resort, try the workspace root
                    if candidate.is_none() {
                        candidate = find_rs_file_shallow(workspace_root, 2);
                    }
                    candidate.or_else(|| find_representative_file(workspace_root, language_id))
                }
            }
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

/// Find a representative file within the workspace that rust-analyzer can use
/// to determine the proper workspace root and configuration
fn find_representative_file(workspace_root: &PathBuf, language_id: &str) -> Option<PathBuf> {
    // For Rust projects, try to find common files that rust-analyzer can use
    if language_id == "rust" {
        // First, try Cargo.toml (the project root marker)
        let cargo_toml = workspace_root.join("Cargo.toml");
        if cargo_toml.exists() && cargo_toml.is_file() {
            return Some(cargo_toml);
        }

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

/// For Rust workspaces, try to identify better workspace roots than the VCS root.
/// Strategy:
/// - If the provided root has a Cargo.toml with a [workspace] table, use it.
/// - Else, collect immediate and shallow nested directories that contain a Cargo.toml.
/// - Limit breadth and depth to avoid costly scans in very large repos.
fn find_rust_workspace_roots(workspace_root: &PathBuf) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    let cargo_toml = workspace_root.join("Cargo.toml");
    let is_workspace =
        cargo_toml.exists() && cargo_toml.is_file() && cargo_toml_has_workspace(&cargo_toml);
    if is_workspace {
        // Include the workspace root to serve as a workspaceFolder for RA
        roots.push(workspace_root.clone());
        // Fall through to also collect shallow member crates so we can choose better representative files
    } else if cargo_toml.exists() && cargo_toml.is_file() {
        // Single crate root
        roots.push(workspace_root.clone());
        return roots;
    }

    // Collect shallow nested Cargo.toml directories (depth 2). This helps pick member crates in a workspace.
    const MAX_DIRS: usize = 64; // safety cap
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(workspace_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.file_name().and_then(|s| s.to_str()) != Some("target") {
                let cargo = path.join("Cargo.toml");
                if cargo.exists() && cargo.is_file() {
                    roots.push(path.clone());
                    count += 1;
                    if count >= MAX_DIRS {
                        break;
                    }
                    continue;
                }
                // One more level deep
                if let Ok(subs) = std::fs::read_dir(&path) {
                    for sub in subs.flatten() {
                        let subpath = sub.path();
                        if subpath.is_dir()
                            && subpath.file_name().and_then(|s| s.to_str()) != Some("target")
                        {
                            let cargo2 = subpath.join("Cargo.toml");
                            if cargo2.exists() && cargo2.is_file() {
                                roots.push(subpath.clone());
                                count += 1;
                                if count >= MAX_DIRS {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if count >= MAX_DIRS {
                break;
            }
        }
    }

    // Provide a stable ordering to reduce nondeterminism; lexicographic is fine
    roots.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    roots
}

fn cargo_toml_has_workspace(path: &PathBuf) -> bool {
    if let Ok(contents) = std::fs::read_to_string(path) {
        // Cheap check to avoid pulling in a full TOML parser here
        contents.contains("[workspace]")
    } else {
        false
    }
}

/// Find any .rs file within the directory up to a limited depth to use as a representative file.
fn find_rs_file_shallow(root: &PathBuf, max_depth: usize) -> Option<PathBuf> {
    // Prefer conventional entry points if present
    let lib_rs = root.join("src").join("lib.rs");
    if lib_rs.is_file() {
        return Some(lib_rs);
    }
    let main_rs = root.join("src").join("main.rs");
    if main_rs.is_file() {
        return Some(main_rs);
    }

    fn walk(dir: &PathBuf, depth: usize, max_depth: usize) -> Option<PathBuf> {
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
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if matches!(name, "target" | ".git" | "node_modules" | ".cache") {
                        continue;
                    }
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
fn find_active_rust_document(editor: &Editor, workspace_root: &PathBuf) -> Option<PathBuf> {
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
        if let Some(doc) = editor.documents.get(&view.doc) {
            if let Some(path) = doc.path() {
                if is_candidate(&path) {
                    return Some(path.to_path_buf());
                }
            }
        }
    }

    // 2) Fall back to any open view within the workspace
    for (view_ref, _) in editor.tree.views() {
        let view = editor.tree.get(view_ref.id);
        if let Some(doc) = editor.documents.get(&view.doc) {
            if let Some(path) = doc.path() {
                if is_candidate(&path) {
                    return Some(path.to_path_buf());
                }
            }
        }
    }

    None
}

// removed unused helper find_cargo_root_for to eliminate dead code warnings
