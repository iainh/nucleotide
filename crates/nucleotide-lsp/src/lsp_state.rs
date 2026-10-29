// ABOUTME: LSP state management for reactive UI updates
// ABOUTME: Provides presentation-neutral LSP status, diagnostics, and progress

use helix_core::Uri;
use helix_core::diagnostic::Diagnostic;
use helix_lsp::LanguageServerId;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

/// Progress information for a single LSP operation
#[derive(Clone, Debug)]
pub struct LspProgress {
    pub server_id: LanguageServerId,
    pub token: String,
    pub title: String,
    pub message: Option<String>,
    pub percentage: Option<u32>,
}

/// Presentation-neutral activity state for compact status surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LspStatusKind {
    Unavailable,
    Idle,
    Starting,
    Working,
    Failed,
}

/// Structured LSP status for status bars, tooltips, and menus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspStatusSummary {
    pub server_id: Option<LanguageServerId>,
    pub server_name: String,
    pub is_remote: bool,
    pub kind: LspStatusKind,
    pub title: String,
    pub message: Option<String>,
    pub percentage: Option<u32>,
    pub server_count: usize,
}

impl LspStatusSummary {
    pub fn full_text(&self) -> String {
        let location = if self.is_remote { " (Remote)" } else { "" };
        let mut text = format!("{}{location}: {}", self.server_name, self.title);
        if let Some(percentage) = self.percentage {
            text.push_str(&format!(" ({percentage}%)"));
        }
        if let Some(message) = self
            .message
            .as_deref()
            .filter(|message| !message.is_empty())
        {
            text.push_str(" — ");
            text.push_str(message);
        }
        text
    }
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
    pub is_remote: bool,
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

    /// Observable project-scoped startup and retry status.
    pub project_session: Option<ProjectLspSessionStatus>,
}

impl LspState {
    pub fn new() -> Self {
        let state = Self {
            servers: HashMap::new(),
            progress: HashMap::new(),
            diagnostics: BTreeMap::new(),
            project_session: None,
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
        self.project_session = None;

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
                is_remote: false,
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

    /// Build a structured status summary without coupling state to a UI layout.
    pub fn status_summary(
        &self,
        preferred_server_id: Option<LanguageServerId>,
    ) -> LspStatusSummary {
        let server_count = self.servers.len();

        if let Some(progress) = self.get_most_important_progress() {
            let server_name = self
                .servers
                .get(&progress.server_id)
                .map(|server| server.name.clone())
                .unwrap_or_else(|| "LSP".to_string());
            return LspStatusSummary {
                server_id: Some(progress.server_id),
                server_name,
                is_remote: self
                    .servers
                    .get(&progress.server_id)
                    .is_some_and(|server| server.is_remote),
                kind: LspStatusKind::Working,
                title: progress.title.clone(),
                message: progress.message.clone(),
                percentage: progress.percentage,
                server_count,
            };
        }

        let mut servers = self.servers.values().collect::<Vec<_>>();
        servers.sort_by(|left, right| left.name.cmp(&right.name));

        if let Some(server) = servers
            .iter()
            .copied()
            .find(|server| matches!(server.status, ServerStatus::Failed(_)))
        {
            let message = match &server.status {
                ServerStatus::Failed(error) => Some(error.clone()),
                _ => None,
            };
            return LspStatusSummary {
                server_id: Some(server.id),
                server_name: server.name.clone(),
                is_remote: server.is_remote,
                kind: LspStatusKind::Failed,
                title: "Failed".to_string(),
                message,
                percentage: None,
                server_count,
            };
        }

        let preferred = preferred_server_id.and_then(|id| self.servers.get(&id));
        let server = preferred.or_else(|| servers.first().copied());
        let Some(server) = server else {
            return LspStatusSummary {
                server_id: None,
                server_name: "LSP".to_string(),
                is_remote: false,
                kind: LspStatusKind::Unavailable,
                title: "No servers".to_string(),
                message: None,
                percentage: None,
                server_count,
            };
        };

        let (kind, title, message) = match &server.status {
            ServerStatus::Starting => (LspStatusKind::Starting, "Starting", None),
            ServerStatus::Initializing => (LspStatusKind::Starting, "Initializing", None),
            ServerStatus::Running => (LspStatusKind::Idle, "Ready", None),
            ServerStatus::Stopped => (LspStatusKind::Unavailable, "Stopped", None),
            ServerStatus::Failed(error) => (LspStatusKind::Failed, "Failed", Some(error.clone())),
        };

        LspStatusSummary {
            server_id: Some(server.id),
            server_name: server.name.clone(),
            is_remote: server.is_remote,
            kind,
            title: title.to_string(),
            message,
            percentage: None,
            server_count,
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

        let indicator = state.status_summary(None).full_text();
        assert!(indicator.contains("rust-analyzer"));
        assert!(indicator.contains("Indexing"));
        assert!(indicator.contains("123/456 crates"));
        assert!(!indicator.contains("taplo"));
    }

    #[test]
    fn structured_status_preserves_progress_details() {
        let server_id = slotmap::KeyData::from_ffi(3).into();
        let mut state = LspState::new();
        state.register_server(server_id, "rust-analyzer".to_string(), None);
        state.add_progress(LspProgress {
            server_id,
            token: "cache-priming".to_string(),
            title: "Indexing".to_string(),
            message: Some("723/724 (nucl)".to_string()),
            percentage: Some(99),
        });

        let summary = state.status_summary(Some(server_id));

        assert_eq!(summary.kind, LspStatusKind::Working);
        assert_eq!(summary.server_name, "rust-analyzer");
        assert_eq!(summary.title, "Indexing");
        assert_eq!(summary.message.as_deref(), Some("723/724 (nucl)"));
        assert_eq!(summary.percentage, Some(99));
        assert_eq!(
            summary.full_text(),
            "rust-analyzer: Indexing (99%) — 723/724 (nucl)"
        );
    }

    #[test]
    fn structured_status_prioritizes_failure_and_preferred_idle_server() {
        let rust_id = slotmap::KeyData::from_ffi(4).into();
        let taplo_id = slotmap::KeyData::from_ffi(5).into();
        let mut state = LspState::new();
        state.register_server(rust_id, "rust-analyzer".to_string(), None);
        state.update_server_status(rust_id, ServerStatus::Running);
        state.register_server(taplo_id, "taplo".to_string(), None);
        state.update_server_status(taplo_id, ServerStatus::Running);

        let idle = state.status_summary(Some(rust_id));
        assert_eq!(idle.kind, LspStatusKind::Idle);
        assert_eq!(idle.server_name, "rust-analyzer");
        assert_eq!(idle.title, "Ready");

        state.update_server_status(taplo_id, ServerStatus::Failed("exited".to_string()));
        let failed = state.status_summary(Some(rust_id));
        assert_eq!(failed.kind, LspStatusKind::Failed);
        assert_eq!(failed.server_name, "taplo");
        assert_eq!(failed.message.as_deref(), Some("exited"));
    }

    #[test]
    fn structured_status_tracks_the_selected_servers_execution_location() {
        let local_id = slotmap::KeyData::from_ffi(6).into();
        let remote_id = slotmap::KeyData::from_ffi(7).into();
        let mut state = LspState::new();
        for id in [local_id, remote_id] {
            state.register_server(
                id,
                "rust-analyzer".to_string(),
                Some("file:///workspace".to_string()),
            );
            state.update_server_status(id, ServerStatus::Running);
        }
        state.servers.get_mut(&remote_id).unwrap().is_remote = true;

        assert!(!state.status_summary(Some(local_id)).is_remote);
        assert_eq!(
            state.status_summary(Some(local_id)).full_text(),
            "rust-analyzer: Ready"
        );
        assert_eq!(
            state.status_summary(Some(remote_id)).full_text(),
            "rust-analyzer (Remote): Ready"
        );

        state.start_progress(remote_id, "index".to_string(), "Indexing".to_string());
        let working = state.status_summary(Some(local_id));
        assert!(working.is_remote);
        assert_eq!(working.full_text(), "rust-analyzer (Remote): Indexing");
        state.end_progress(remote_id, "index".to_string());

        state.update_server_status(remote_id, ServerStatus::Failed("disconnected".to_string()));
        assert!(state.status_summary(Some(local_id)).is_remote);
        state.remove_server(remote_id);
        assert!(!state.status_summary(Some(local_id)).is_remote);
        state.remove_server(local_id);
        assert!(!state.status_summary(None).is_remote);
    }
}
