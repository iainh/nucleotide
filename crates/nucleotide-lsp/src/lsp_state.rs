// ABOUTME: LSP state management for reactive UI updates
// ABOUTME: Provides a GPUI Model for LSP status, diagnostics, and progress

use helix_core::Uri;
use helix_core::diagnostic::Diagnostic;
use helix_lsp::LanguageServerId;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Spinner frames matching helix-term
pub const SPINNER_FRAMES: &[&str] = &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
pub const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// Progress information for a single LSP operation
#[derive(Clone, Debug)]
pub struct LspProgress {
    pub server_id: LanguageServerId,
    pub token: String,
    pub title: String,
    pub message: Option<String>,
    pub percentage: Option<u32>,
}

/// Status of a language server
#[derive(Clone, Debug, PartialEq)]
pub enum ServerStatus {
    Starting,
    Initializing,
    Running,
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectServerLifecycle {
    Planned,
    PreparingEnvironment,
    Starting,
    Initializing,
    Running,
    RetryScheduled {
        attempt: usize,
        next_retry_at: Instant,
    },
    Failed {
        error: String,
    },
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectEnvironmentSource {
    Project,
    ProcessFallback,
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct PlannedServerStatus {
    pub language_id: String,
    pub server_name: String,
    pub lifecycle: ProjectServerLifecycle,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProjectLspSessionStatus {
    pub generation: u64,
    pub workspace_root: PathBuf,
    pub opened_at: Instant,
    pub environment_source: Option<ProjectEnvironmentSource>,
    pub servers: Vec<PlannedServerStatus>,
}

/// Information about a language server
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub id: LanguageServerId,
    pub name: String,
    pub status: ServerStatus,
    pub root_uri: Option<String>,
    pub capabilities: Option<serde_json::Value>,
}

/// Diagnostic information with source server
#[derive(Clone, Debug)]
pub struct DiagnosticInfo {
    pub diagnostic: Diagnostic,
    pub server_id: LanguageServerId,
}

/// LSP state that can be observed by UI components
#[derive(Clone)]
pub struct LspState {
    /// Active language servers
    pub servers: HashMap<LanguageServerId, ServerInfo>,

    /// Current progress operations
    pub progress: HashMap<String, LspProgress>,

    /// Diagnostics by file URI
    pub diagnostics: BTreeMap<Uri, Vec<DiagnosticInfo>>,

    /// Last status message
    pub status_message: Option<String>,

    /// Observable project-scoped startup and retry status.
    pub project_session: Option<ProjectLspSessionStatus>,

    /// Spinner state
    pub spinner_frame: usize,
    pub last_spinner_update: Instant,
}

impl LspState {
    pub fn new() -> Self {
        let state = Self {
            servers: HashMap::new(),
            progress: HashMap::new(),
            diagnostics: BTreeMap::new(),
            status_message: None,
            project_session: None,
            spinner_frame: 0,
            last_spinner_update: Instant::now(),
        };

        // TEMPORARY: Test progress will be added via real LSP integration
        nucleotide_logging::debug!(
            "LSP state initialized - ready for server registration and progress tracking"
        );

        state
    }

    /// Clear all LSP state (used when project root changes)
    pub fn clear_all_state(&mut self) {
        self.servers.clear();
        self.progress.clear();
        self.diagnostics.clear();
        self.status_message = None;
        self.project_session = None;
        self.spinner_frame = 0;
        self.last_spinner_update = Instant::now();

        nucleotide_logging::debug!("LSP state cleared - ready for new project LSP servers");
    }

    pub fn begin_project_session(&mut self, generation: u64, workspace_root: PathBuf) {
        self.project_session = Some(ProjectLspSessionStatus {
            generation,
            workspace_root,
            opened_at: Instant::now(),
            environment_source: None,
            servers: Vec::new(),
        });
    }

    pub fn set_project_server_plan(&mut self, servers: &[(String, String)]) {
        let Some(session) = &mut self.project_session else {
            return;
        };
        session.servers = servers
            .iter()
            .map(|(language_id, server_name)| PlannedServerStatus {
                language_id: language_id.clone(),
                server_name: server_name.clone(),
                lifecycle: ProjectServerLifecycle::Planned,
                last_error: None,
            })
            .collect();
    }

    pub fn extend_project_server_plan(&mut self, servers: &[(String, String)]) {
        let Some(session) = &mut self.project_session else {
            return;
        };
        for (language_id, server_name) in servers {
            if session.servers.iter().any(|server| {
                server.language_id == *language_id && server.server_name == *server_name
            }) {
                continue;
            }
            session.servers.push(PlannedServerStatus {
                language_id: language_id.clone(),
                server_name: server_name.clone(),
                lifecycle: ProjectServerLifecycle::Planned,
                last_error: None,
            });
        }
    }

    pub fn set_project_environment_source(&mut self, source: ProjectEnvironmentSource) {
        if let Some(session) = &mut self.project_session {
            session.environment_source = Some(source);
        }
    }

    pub fn update_project_server_lifecycle(
        &mut self,
        language_id: &str,
        server_name: &str,
        lifecycle: ProjectServerLifecycle,
        error: Option<String>,
    ) {
        let Some(server) = self.project_session.as_mut().and_then(|session| {
            session.servers.iter_mut().find(|server| {
                server.language_id == language_id && server.server_name == server_name
            })
        }) else {
            return;
        };
        server.lifecycle = lifecycle;
        server.last_error = error;
    }

    /// Get the current spinner frame, advancing if needed
    pub fn get_spinner_frame(&mut self) -> &'static str {
        let now = Instant::now();
        if now.duration_since(self.last_spinner_update) >= SPINNER_INTERVAL {
            self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
            self.last_spinner_update = now;
        }
        SPINNER_FRAMES[self.spinner_frame]
    }

    /// Register a new language server
    pub fn register_server(
        &mut self,
        id: LanguageServerId,
        name: String,
        root_uri: Option<String>,
    ) {
        self.servers.insert(
            id,
            ServerInfo {
                id,
                name,
                status: ServerStatus::Starting,
                root_uri,
                capabilities: None,
            },
        );
    }

    /// Update server status
    pub fn update_server_status(&mut self, id: LanguageServerId, status: ServerStatus) {
        if let Some(server) = self.servers.get_mut(&id) {
            server.status = status;
        }
    }

    /// Update server capabilities
    pub fn update_server_capabilities(
        &mut self,
        id: LanguageServerId,
        capabilities: serde_json::Value,
    ) {
        if let Some(server) = self.servers.get_mut(&id) {
            server.capabilities = Some(capabilities);
            server.status = ServerStatus::Running;
        }
    }

    /// Remove a language server
    pub fn remove_server(&mut self, id: LanguageServerId) {
        self.servers.remove(&id);

        // Remove all progress for this server
        self.progress.retain(|_, p| p.server_id != id);

        // Remove all diagnostics from this server
        for diagnostics in self.diagnostics.values_mut() {
            diagnostics.retain(|d| d.server_id != id);
        }
        self.diagnostics.retain(|_, diags| !diags.is_empty());
        nucleotide_logging::debug!(server_id = ?id, "DIAG: LspState cleared diagnostics for server");
    }

    /// Start a progress operation
    pub fn start_progress(&mut self, server_id: LanguageServerId, token: String, title: String) {
        let key = format!("{server_id}-{token}");
        self.progress.insert(
            key,
            LspProgress {
                server_id,
                token,
                title,
                message: None,
                percentage: None,
            },
        );
    }

    /// Update progress
    pub fn update_progress(
        &mut self,
        server_id: LanguageServerId,
        token: String,
        message: Option<String>,
        percentage: Option<u32>,
    ) {
        let key = format!("{server_id}-{token}");
        if let Some(progress) = self.progress.get_mut(&key) {
            if let Some(msg) = message {
                progress.message = Some(msg);
            }
            if let Some(pct) = percentage {
                progress.percentage = Some(pct);
            }
        }
    }

    /// End a progress operation
    pub fn end_progress(&mut self, server_id: LanguageServerId, token: String) {
        let key = format!("{server_id}-{token}");
        self.progress.remove(&key);
    }

    /// Set diagnostics for a file
    pub fn set_diagnostics(&mut self, uri: Uri, diagnostics: Vec<DiagnosticInfo>) {
        if diagnostics.is_empty() {
            nucleotide_logging::debug!(uri = %uri.to_string(), "DIAG: Clearing diagnostics for URI");
            self.diagnostics.remove(&uri);
        } else {
            let total = diagnostics.len();
            let mut by_server: std::collections::BTreeMap<LanguageServerId, usize> =
                Default::default();
            for d in diagnostics.iter() {
                *by_server.entry(d.server_id).or_default() += 1;
            }
            nucleotide_logging::debug!(
                uri = %uri.to_string(),
                total = total,
                by_server = ?by_server,
                "DIAG: Setting diagnostics for URI"
            );
            self.diagnostics.insert(uri, diagnostics);
        }
    }

    /// Get diagnostics for a file
    pub fn get_diagnostics(&self, uri: &Uri) -> Option<&[DiagnosticInfo]> {
        self.diagnostics.get(uri).map(|v| v.as_slice())
    }

    /// Get all active progress operations
    pub fn active_progress(&self) -> Vec<&LspProgress> {
        self.progress.values().collect()
    }

    /// Get running servers count
    pub fn running_servers_count(&self) -> usize {
        self.servers
            .values()
            .filter(|s| matches!(s.status, ServerStatus::Running))
            .count()
    }

    /// Check if any server is busy
    pub fn is_busy(&self) -> bool {
        !self.progress.is_empty()
    }

    /// Get a formatted status string
    pub fn status_string(&self) -> Option<String> {
        if let Some(msg) = &self.status_message {
            return Some(msg.clone());
        }

        if !self.progress.is_empty() {
            let progress_items: Vec<String> = self
                .progress
                .values()
                .map(|p| {
                    let mut status = p.title.clone();
                    if let Some(msg) = &p.message {
                        status.push_str(": ");
                        status.push_str(msg);
                    }
                    if let Some(pct) = p.percentage {
                        status.push_str(&format!(" ({pct}%)"));
                    }
                    status
                })
                .collect();
            return Some(progress_items.join(", "));
        }

        None
    }

    /// Check if we should show spinner (when there's LSP activity)
    pub fn should_show_spinner(&self) -> bool {
        // Show spinner if:
        // - There are active, non-idle progress operations
        // - Any server is starting/initializing
        let has_active_progress = self.progress.values().any(|p| p.token.as_str() != "idle");
        let has_busy_servers = self.servers.values().any(|s| {
            matches!(
                s.status,
                ServerStatus::Starting | ServerStatus::Initializing
            )
        });
        let has_busy_project_server = self.project_session.as_ref().is_some_and(|session| {
            session.servers.iter().any(|server| {
                matches!(
                    server.lifecycle,
                    ProjectServerLifecycle::Planned
                        | ProjectServerLifecycle::PreparingEnvironment
                        | ProjectServerLifecycle::Starting
                        | ProjectServerLifecycle::Initializing
                        | ProjectServerLifecycle::RetryScheduled { .. }
                )
            })
        });

        has_active_progress || has_busy_servers || has_busy_project_server
    }

    /// Check if any LSP servers are present (regardless of activity)
    pub fn has_any_lsp_server(&self) -> bool {
        !self.servers.is_empty()
    }

    /// Get the appropriate LSP indicator with server name and progress messages (like Helix)
    /// Returns different variants based on available space and content priority
    pub fn get_lsp_indicator(&mut self) -> Option<String> {
        self.get_lsp_indicator_with_max_width(None)
    }

    /// Get LSP indicator with intelligent content adaptation based on max width
    pub fn get_lsp_indicator_with_max_width(&mut self, max_width: Option<usize>) -> Option<String> {
        if !self.has_any_lsp_server() {
            // Show placeholder when no LSP servers are registered
            return Some("LSP: ●".to_string());
        }

        // If we have active progress, show detailed status like Helix
        if let Some(progress) = self.get_most_important_progress() {
            // Get server name and clone progress to avoid borrow conflicts
            let server_name = self
                .servers
                .get(&progress.server_id)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "LSP".to_string());
            let progress_clone = progress.clone();

            // Try different detail levels based on available space
            if let Some(max_len) = max_width {
                return self.format_progress_adaptive(&progress_clone, &server_name, max_len);
            } else {
                // Default behavior - full message with reasonable truncation
                return self.format_progress_full(&progress_clone, &server_name);
            }
        }

        // If no progress but servers are running, show appropriate indicator
        let indicator = if self.should_show_spinner() {
            self.get_spinner_frame().to_string()
        } else {
            "◉".to_string()
        };

        // Show server count if multiple servers
        let server_count = self.servers.len();
        if server_count > 1 {
            Some(format!("{} {}x", indicator, server_count))
        } else if let Some(server) = self.servers.values().next() {
            // Show single server name if space permits
            let display = format!("{} {}", indicator, server.name);
            if max_width.is_none_or(|max| display.len() <= max) {
                Some(display)
            } else {
                Some(indicator)
            }
        } else {
            Some(indicator)
        }
    }

    /// Format progress with full detail including visual indicator
    fn format_progress_full(
        &mut self,
        progress: &LspProgress,
        server_name: &str,
    ) -> Option<String> {
        // Get visual indicator (spinner for active, solid for idle)
        let visual_indicator = if progress.token == "idle" {
            "◉".to_string() // Solid circle for idle/ready state
        } else {
            self.get_spinner_frame().to_string() // Animated spinner for active progress
        };

        let mut status = format!("{} {}: ", visual_indicator, server_name);

        // Add percentage if available
        if let Some(percentage) = progress.percentage {
            status.push_str(&format!("{:>2}% ", percentage));
        }

        // Add title
        status.push_str(&progress.title);

        // Add message with separator if both exist
        if let Some(message) = &progress.message {
            status.push_str(" ⋅ ");
            status.push_str(message);
        }

        Some(status)
    }

    /// Format progress adaptively based on available space
    fn format_progress_adaptive(
        &mut self,
        progress: &LspProgress,
        server_name: &str,
        max_len: usize,
    ) -> Option<String> {
        // Strategy: Try progressively simpler formats until we fit

        // Full format: "ServerName: 85% Title ⋅ Message"
        if let Some(full) = self.format_progress_full(progress, server_name)
            && full.len() <= max_len
        {
            return Some(full);
        }

        // Medium format: "ServerName: 85% Title"
        let mut medium = format!("{}: ", server_name);
        if let Some(percentage) = progress.percentage {
            medium.push_str(&format!("{:>2}% ", percentage));
        }
        medium.push_str(&progress.title);
        if medium.len() <= max_len {
            return Some(medium);
        }

        // Compact format: "ServerName: Title"
        let compact = format!("{}: {}", server_name, progress.title);
        if compact.len() <= max_len {
            return Some(compact);
        }

        // Short server format: "Server: Title"
        let short_server = self.get_short_server_name(server_name);
        let short_format = format!("{}: {}", short_server, progress.title);
        if short_format.len() <= max_len {
            return Some(short_format);
        }

        // Minimal format: "Server" or spinner only
        if short_server.len() <= max_len {
            Some(short_server)
        } else if progress.token.as_str() == "idle" {
            // For idle progress, always show a solid indicator even under width constraints
            Some("◉".to_string())
        } else {
            // Active progress: show spinner frame when we cannot fit any text
            Some(self.get_spinner_frame().to_string())
        }
    }

    /// Get abbreviated server name for compact display
    fn get_short_server_name(&self, server_name: &str) -> String {
        match server_name {
            "rust-analyzer" => "rust".to_string(),
            "typescript-language-server" => "ts".to_string(),
            "pyright" => "py".to_string(),
            "clangd" => "cpp".to_string(),
            "gopls" => "go".to_string(),
            "java-language-server" => "java".to_string(),
            name if name.chars().count() > 8 => {
                let prefix = name.chars().take(6).collect::<String>();
                format!("{prefix}..")
            }
            name => name.to_string(),
        }
    }

    /// Get the most important progress message (prioritize those with messages)
    fn get_most_important_progress(&self) -> Option<&LspProgress> {
        // Active work must always outrank synthetic/legacy idle entries from
        // another server in the same project.
        if let Some(progress) = self
            .progress
            .values()
            .find(|p| p.token != "idle" && p.message.is_some() && !p.title.is_empty())
        {
            return Some(progress);
        }

        if let Some(progress) = self
            .progress
            .values()
            .find(|p| p.token != "idle" && p.message.is_some())
        {
            return Some(progress);
        }

        if let Some(progress) = self
            .progress
            .values()
            .find(|p| p.token != "idle" && !p.title.is_empty())
        {
            return Some(progress);
        }

        self.progress.values().find(|p| !p.title.is_empty())
    }

    /// Add a progress message (for testing and integration)
    pub fn add_progress(&mut self, progress: LspProgress) {
        self.progress.insert(progress.token.clone(), progress);
    }

    /// Remove a progress message
    pub fn remove_progress(&mut self, token: &str) {
        self.progress.remove(token);
    }
}

impl Default for LspState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_server_name_truncates_unicode_on_char_boundaries() {
        let state = LspState::new();

        assert_eq!(state.get_short_server_name("↪a↪server"), "↪a↪ser..");
    }

    #[test]
    fn short_server_name_counts_chars_instead_of_bytes() {
        let state = LspState::new();

        assert_eq!(state.get_short_server_name("ééééé"), "ééééé");
    }

    #[test]
    fn project_session_exposes_plan_environment_and_lifecycle() {
        let mut state = LspState::new();
        state.begin_project_session(7, PathBuf::from("/workspace"));
        state.set_project_server_plan(&[("toml".to_string(), "taplo".to_string())]);
        state.set_project_environment_source(ProjectEnvironmentSource::Project);
        state.update_project_server_lifecycle(
            "toml",
            "taplo",
            ProjectServerLifecycle::Initializing,
            None,
        );

        let session = state.project_session.as_ref().unwrap();
        assert_eq!(session.generation, 7);
        assert_eq!(
            session.environment_source,
            Some(ProjectEnvironmentSource::Project)
        );
        assert_eq!(session.servers.len(), 1);
        assert_eq!(session.servers[0].server_name, "taplo");
        assert_eq!(
            session.servers[0].lifecycle,
            ProjectServerLifecycle::Initializing
        );
        assert!(state.should_show_spinner());
    }

    #[test]
    fn active_progress_outranks_idle_progress_from_another_server() {
        let idle_server = slotmap::KeyData::from_ffi(1).into();
        let active_server = slotmap::KeyData::from_ffi(2).into();
        let mut state = LspState::new();
        state.register_server(idle_server, "taplo".to_string(), None);
        state.register_server(active_server, "rust-analyzer".to_string(), None);
        state.add_progress(LspProgress {
            server_id: idle_server,
            token: "idle".to_string(),
            title: "Connected".to_string(),
            message: Some("Ready".to_string()),
            percentage: None,
        });
        state.add_progress(LspProgress {
            server_id: active_server,
            token: "workspace-index".to_string(),
            title: "Indexing".to_string(),
            message: Some("123/456 crates".to_string()),
            percentage: Some(27),
        });

        let indicator = state.get_lsp_indicator().unwrap();
        assert!(indicator.contains("rust-analyzer"));
        assert!(indicator.contains("Indexing"));
        assert!(indicator.contains("123/456 crates"));
        assert!(!indicator.contains("taplo"));
    }
}
