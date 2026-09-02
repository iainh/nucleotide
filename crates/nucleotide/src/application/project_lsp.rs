// ABOUTME: Owns project-wide LSP session state, planning, and command coordination.
// ABOUTME: Keeps project lifecycle invariants out of the general Application state.

use std::{
    collections::{BTreeSet, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    task::{Context as TaskContext, Poll},
    time::Duration,
};

use helix_view::Editor;
use nucleotide_events::{
    PlannedProjectLanguage, ProjectLanguageEvidence, ProjectLspCommand, ProjectLspCommandError,
    ProjectLspPlan, ProjectSessionResult,
};
use nucleotide_lsp::{HelixLspBridge, RemoteLspSessionProvider};
use nucleotide_workspace::{
    DirectoryListing, FileKind, ProcessSpec, WorkspaceBackendHandle, classify_workspace_location,
};

use super::ProjectEnvironmentProvider;

struct ProjectLspSystem {
    bridge: HelixLspBridge,
    environment_provider: Arc<ProjectEnvironmentProvider>,
}

#[derive(Debug, Default)]
struct ProjectLspSupervisor {
    generation: u64,
    active_root: Option<PathBuf>,
    retrying_servers: HashSet<(u64, String, String)>,
    /// Servers already launched on behalf of an opened document in this generation.
    document_started_servers: HashSet<(u64, String, String)>,
    session_in_flight: bool,
    session_result: Option<ProjectSessionResult>,
    session_waiters:
        Vec<tokio::sync::oneshot::Sender<Result<ProjectSessionResult, ProjectLspCommandError>>>,
}

#[derive(Debug)]
pub(super) struct ProjectSessionTransition {
    pub(super) generation: u64,
    pub(super) previous_root: Option<PathBuf>,
}

impl ProjectLspSupervisor {
    fn open_session(&mut self, workspace_root: PathBuf) -> ProjectSessionTransition {
        if self.active_root.as_ref() == Some(&workspace_root) {
            return ProjectSessionTransition {
                generation: self.generation,
                previous_root: None,
            };
        }

        self.generation = self.generation.wrapping_add(1).max(1);
        self.retrying_servers.clear();
        self.document_started_servers.clear();
        ProjectSessionTransition {
            generation: self.generation,
            previous_root: self.active_root.replace(workspace_root),
        }
    }

    fn queue_session_open(
        &mut self,
        workspace_root: PathBuf,
        response: tokio::sync::oneshot::Sender<
            Result<ProjectSessionResult, ProjectLspCommandError>,
        >,
    ) -> Option<ProjectSessionTransition> {
        if self.active_root.as_ref() == Some(&workspace_root) {
            if let Some(result) = &self.session_result {
                let _ = response.send(Ok(result.clone()));
                return None;
            }
            if self.session_in_flight {
                self.session_waiters.push(response);
                return None;
            }

            self.session_in_flight = true;
            self.session_waiters.push(response);
            return Some(ProjectSessionTransition {
                generation: self.generation,
                previous_root: None,
            });
        }

        for waiter in self.session_waiters.drain(..) {
            let _ = waiter.send(Err(ProjectLspCommandError::StaleProjectSession));
        }
        let transition = self.open_session(workspace_root);
        self.session_in_flight = true;
        self.session_result = None;
        self.session_waiters.push(response);
        Some(transition)
    }

    fn queue_session_restart(
        &mut self,
        workspace_root: PathBuf,
        response: tokio::sync::oneshot::Sender<
            Result<ProjectSessionResult, ProjectLspCommandError>,
        >,
    ) -> ProjectSessionTransition {
        for waiter in self.session_waiters.drain(..) {
            let _ = waiter.send(Err(ProjectLspCommandError::StaleProjectSession));
        }

        self.generation = self.generation.wrapping_add(1).max(1);
        self.retrying_servers.clear();
        self.document_started_servers.clear();
        self.session_in_flight = true;
        self.session_result = None;
        self.session_waiters.push(response);

        ProjectSessionTransition {
            generation: self.generation,
            previous_root: self.active_root.replace(workspace_root),
        }
    }

    fn complete_session(
        &mut self,
        generation: u64,
        result: Result<ProjectSessionResult, ProjectLspCommandError>,
    ) {
        if self.generation != generation {
            return;
        }
        self.session_in_flight = false;
        if let Ok(session) = &result {
            self.session_result = Some(session.clone());
        }
        for waiter in self.session_waiters.drain(..) {
            let _ = waiter.send(result.clone());
        }
    }

    fn is_current(&self, generation: u64, workspace_root: &Path) -> bool {
        self.generation == generation && self.active_root.as_deref() == Some(workspace_root)
    }

    /// Generation of the active session for `workspace_root` once its preparation has
    /// settled. While preparation is in flight the session itself decides what to start.
    fn settled_generation(&self, workspace_root: &Path) -> Option<u64> {
        (self.active_root.as_deref() == Some(workspace_root) && !self.session_in_flight)
            .then_some(self.generation)
    }

    /// Records a document-driven start; returns `false` when this generation already
    /// launched that server for a document.
    fn begin_document_start(
        &mut self,
        generation: u64,
        language_id: &str,
        server_name: &str,
    ) -> bool {
        self.document_started_servers.insert((
            generation,
            language_id.to_string(),
            server_name.to_string(),
        ))
    }

    fn begin_retry(&mut self, generation: u64, language_id: &str, server_name: &str) -> bool {
        self.retrying_servers
            .insert((generation, language_id.to_string(), server_name.to_string()))
    }

    fn finish_retry(&mut self, generation: u64, language_id: &str, server_name: &str) {
        self.retrying_servers.remove(&(
            generation,
            language_id.to_string(),
            server_name.to_string(),
        ));
    }
}

/// Sole owner of project-session channels, bridge state, and generation invariants.
pub(super) struct ProjectLspCoordinator {
    system: Option<ProjectLspSystem>,
    /// Remote process-session provider for the active workspace connection. Kept here so
    /// a bridge created after the connection still launches servers on the remote side.
    remote_session_provider: Option<Arc<dyn RemoteLspSessionProvider>>,
    command_tx: tokio::sync::mpsc::UnboundedSender<ProjectLspCommand>,
    command_rx: Option<tokio::sync::mpsc::UnboundedReceiver<ProjectLspCommand>>,
    initialization_attempted: bool,
    supervisor: ProjectLspSupervisor,
}

impl ProjectLspCoordinator {
    pub(super) fn with_remote_session_provider(
        remote_session_provider: Option<Arc<dyn RemoteLspSessionProvider>>,
    ) -> Self {
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            system: None,
            remote_session_provider,
            command_tx,
            command_rx: Some(command_rx),
            initialization_attempted: false,
            supervisor: ProjectLspSupervisor::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn new_initialized() -> Self {
        Self {
            initialization_attempted: true,
            ..Self::with_remote_session_provider(None)
        }
    }

    pub(super) fn command_sender(&self) -> tokio::sync::mpsc::UnboundedSender<ProjectLspCommand> {
        self.command_tx.clone()
    }

    pub(super) fn is_initialized(&self) -> bool {
        self.system.is_some()
    }

    pub(super) fn poll_command(
        &mut self,
        task_cx: &mut TaskContext<'_>,
    ) -> (Option<ProjectLspCommand>, bool) {
        match self.command_rx.as_mut() {
            Some(rx) => match rx.poll_recv(task_cx) {
                Poll::Ready(Some(command)) => (Some(command), false),
                Poll::Ready(None) => {
                    nucleotide_logging::info!("LSP command channel disconnected");
                    self.command_rx = None;
                    (None, true)
                }
                Poll::Pending => (None, false),
            },
            None => (None, true),
        }
    }

    pub(super) fn begin_initialization(&mut self) -> bool {
        if self.initialization_attempted {
            return false;
        }
        self.initialization_attempted = true;
        true
    }

    pub(super) fn initialize(
        &mut self,
        bridge: HelixLspBridge,
        environment_provider: Arc<ProjectEnvironmentProvider>,
    ) -> bool {
        if self.system.is_some() {
            return false;
        }
        bridge.set_remote_session_provider(self.remote_session_provider.clone());
        self.system = Some(ProjectLspSystem {
            bridge,
            environment_provider,
        });
        true
    }

    pub(super) fn bridge(&self) -> Option<HelixLspBridge> {
        self.system.as_ref().map(|system| system.bridge.clone())
    }

    /// Point environment capture at a new workspace backend. A plain backend change has no
    /// remote process-session provider; `set_remote_session_provider` restores one when the
    /// backend comes from a remote connection.
    pub(super) fn set_workspace_backend(&mut self, workspace_backend: WorkspaceBackendHandle) {
        self.set_remote_session_provider(None);
        if let Some(system) = &self.system {
            system
                .environment_provider
                .set_workspace_backend(workspace_backend);
        }
    }

    pub(super) fn set_remote_session_provider(
        &mut self,
        provider: Option<Arc<dyn RemoteLspSessionProvider>>,
    ) {
        self.remote_session_provider = provider.clone();
        if let Some(system) = &self.system {
            system.bridge.set_remote_session_provider(provider);
        }
    }

    pub(super) fn queue_session(
        &mut self,
        workspace_root: PathBuf,
        response: tokio::sync::oneshot::Sender<
            Result<ProjectSessionResult, ProjectLspCommandError>,
        >,
        restart: bool,
    ) -> Option<ProjectSessionTransition> {
        if restart {
            Some(
                self.supervisor
                    .queue_session_restart(workspace_root, response),
            )
        } else {
            self.supervisor.queue_session_open(workspace_root, response)
        }
    }

    pub(super) fn complete_session(
        &mut self,
        generation: u64,
        result: Result<ProjectSessionResult, ProjectLspCommandError>,
    ) {
        self.supervisor.complete_session(generation, result);
    }

    pub(super) fn is_current(&self, generation: u64, workspace_root: &Path) -> bool {
        self.supervisor.is_current(generation, workspace_root)
    }

    pub(super) fn settled_generation(&self, workspace_root: &Path) -> Option<u64> {
        self.supervisor.settled_generation(workspace_root)
    }

    pub(super) fn begin_document_start(
        &mut self,
        generation: u64,
        language_id: &str,
        server_name: &str,
    ) -> bool {
        self.supervisor
            .begin_document_start(generation, language_id, server_name)
    }

    pub(super) fn begin_retry(
        &mut self,
        generation: u64,
        language_id: &str,
        server_name: &str,
    ) -> bool {
        self.supervisor
            .begin_retry(generation, language_id, server_name)
    }

    pub(super) fn finish_retry(&mut self, generation: u64, language_id: &str, server_name: &str) {
        self.supervisor
            .finish_retry(generation, language_id, server_name);
    }

    pub(super) fn take_active_root(&mut self) -> Option<PathBuf> {
        self.supervisor.active_root.take()
    }

    pub(super) fn clear_system(&mut self) {
        self.system = None;
    }
}

pub(super) fn canonical_project_lsp_root(workspace_root: &Path) -> PathBuf {
    if classify_workspace_location(workspace_root).is_remote() {
        return workspace_root.to_path_buf();
    }
    std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf())
}

pub(super) fn project_lsp_error_is_retryable(error: &ProjectLspCommandError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    !message.contains("not configured")
        && !message.contains("no language configuration")
        && !message.contains("not found")
        && !message.contains("missing executable")
        && !matches!(error, ProjectLspCommandError::StaleProjectSession)
}

pub(super) fn configured_project_servers(
    syntax_loader: &helix_core::syntax::Loader,
    plan: &ProjectLspPlan,
) -> Vec<(String, String)> {
    let mut servers = Vec::new();
    let mut seen = HashSet::new();

    for language in &plan.languages {
        let Some(language_config) = syntax_loader
            .language_configs()
            .find(|config| config.language_id == language.language_id)
        else {
            nucleotide_logging::debug!(language_id = %language.language_id, "No effective language configuration for project plan");
            continue;
        };

        for features in &language_config.language_servers {
            let key = (language.language_id.clone(), features.name.clone());
            if seen.insert(key.clone()) {
                servers.push(key);
            }
        }
    }
    servers
}

pub(super) fn configured_language_server_commands(editor: &Editor) -> HashMap<String, String> {
    editor
        .syn_loader
        .load()
        .language_server_configs()
        .iter()
        .map(|(name, config)| (name.clone(), config.command.clone()))
        .collect()
}

/// Commands the given servers would launch, excluding ones whose availability is already known.
/// Probing only these keeps remote startup from checking every configured language server.
pub(super) fn commands_to_probe_for_servers(
    servers: &[(String, String)],
    server_commands: &HashMap<String, String>,
    known_available: Option<&HashSet<String>>,
) -> Vec<String> {
    servers
        .iter()
        .filter_map(|(_, server_name)| server_commands.get(server_name))
        .filter(|command| !known_available.is_some_and(|known| known.contains(*command)))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Status shown while the project session detects languages, loads the project environment and
/// (for remote workspaces) probes server availability.
pub(super) fn project_lsp_preparation_status(
    remote_workspace: bool,
    proactive_startup_enabled: bool,
) -> String {
    match (remote_workspace, proactive_startup_enabled) {
        (true, true) => {
            "Preparing language servers: loading remote project environment and checking servers"
                .to_string()
        }
        (false, true) => "Preparing language servers: loading project environment".to_string(),
        (_, false) => {
            "Loading project environment; language servers start when a file is opened".to_string()
        }
    }
}

/// Status shown once preparation finished and the planned servers are about to launch.
pub(super) fn project_lsp_startup_status(
    server_names: &[String],
    proactive_startup_enabled: bool,
) -> String {
    if !proactive_startup_enabled {
        return "Project environment ready; language servers start when a file is opened"
            .to_string();
    }
    if server_names.is_empty() {
        return "Project environment ready; no language servers to start".to_string();
    }
    format!("Starting language servers: {}", server_names.join(", "))
}

pub(super) fn retain_servers_with_available_commands(
    servers: &mut Vec<(String, String)>,
    server_commands: &HashMap<String, String>,
    available_commands: Option<&HashSet<String>>,
) {
    let Some(available_commands) = available_commands else {
        return;
    };

    servers.retain(|(_, server_name)| {
        server_commands
            .get(server_name)
            .is_none_or(|command| available_commands.contains(command))
    });
}

pub(super) async fn probe_available_language_server_commands(
    workspace_root: &Path,
    workspace_backend: WorkspaceBackendHandle,
    commands: Vec<String>,
    environment: Option<&HashMap<String, String>>,
    timeout: Duration,
) -> Result<HashSet<String>, String> {
    let commands = commands.into_iter().collect::<BTreeSet<_>>();
    if commands.is_empty() {
        return Ok(HashSet::new());
    }
    if environment.is_some_and(|environment| {
        environment
            .get("PATH")
            .is_none_or(|path| path.trim().is_empty())
    }) {
        return Err("captured remote project environment has no PATH".to_string());
    }

    let mut args = vec![
        "-c".to_string(),
        concat!(
            "for command_name do ",
            "if command -v \"$command_name\" >/dev/null 2>&1; then ",
            "printf '%s\\n' \"$command_name\"; ",
            "fi; done"
        )
        .to_string(),
        "nucleotide-lsp-probe".to_string(),
    ];
    args.extend(commands.iter().cloned());

    let output = workspace_backend
        .run_process(ProcessSpec {
            program: "sh".to_string(),
            args,
            cwd: workspace_root.to_path_buf(),
            env: environment
                .into_iter()
                .flatten()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            clear_env: true,
            inherit_project_environment: environment.is_none(),
            stdin: Vec::new(),
            max_output_bytes: Some(64 * 1024),
            timeout_ms: Some(timeout.as_millis().min(u128::from(u64::MAX)) as u64),
        })
        .await
        .map_err(|error| error.to_string())?;

    if !output.success || output.timed_out || output.stdout_truncated {
        return Err(format!(
            "remote language server probe failed (status: {:?}, timed out: {}, output truncated: {})",
            output.status_code, output.timed_out, output.stdout_truncated
        ));
    }

    let available = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect::<HashSet<_>>();
    nucleotide_logging::info!(
        workspace_root = %workspace_root.display(),
        configured_command_count = commands.len(),
        available_command_count = available.len(),
        "Probed remote language server availability"
    );
    Ok(available)
}

pub(super) async fn detect_project_lsp_plan_with_backend(
    workspace_root: &Path,
    workspace_backend: WorkspaceBackendHandle,
) -> ProjectLspPlan {
    let names = workspace_backend
        .list_dir(workspace_root)
        .await
        .map(project_marker_names)
        .unwrap_or_default();

    project_lsp_plan_from_names(&names)
}

pub(super) fn project_marker_names(listing: DirectoryListing) -> Vec<String> {
    listing
        .entries
        .into_iter()
        .filter(|entry| matches!(entry.stat.kind, FileKind::File | FileKind::Symlink))
        .take(512)
        .map(|entry| entry.name)
        .collect()
}

pub(super) fn project_lsp_plan_from_names(names: &[String]) -> ProjectLspPlan {
    let project_type = nucleotide_project::classify_project_markers(names);

    let mut languages = Vec::new();
    let mut add_language = |language_id: &str, evidence: ProjectLanguageEvidence| {
        if !languages
            .iter()
            .any(|language: &PlannedProjectLanguage| language.language_id == language_id)
        {
            languages.push(PlannedProjectLanguage {
                language_id: language_id.to_string(),
                evidence,
            });
        }
    };

    for language_id in nucleotide_project::project_language_ids(&project_type) {
        add_language(&language_id, ProjectLanguageEvidence::EagerProject);
    }

    if names.iter().any(|name| name.ends_with(".toml")) {
        add_language("toml", ProjectLanguageEvidence::EagerManifest);
    }
    if names.iter().any(|name| name.ends_with(".json")) {
        add_language("json", ProjectLanguageEvidence::EagerManifest);
    }
    if names
        .iter()
        .any(|name| name.ends_with(".yaml") || name.ends_with(".yml"))
    {
        add_language("yaml", ProjectLanguageEvidence::DiscoveredLanguage);
    }

    ProjectLspPlan {
        project_type,
        languages,
    }
}

/// Plan for languages found by scanning project files rather than project markers.
pub(super) fn project_lsp_plan_from_discovered_languages(
    language_ids: impl IntoIterator<Item = String>,
) -> ProjectLspPlan {
    ProjectLspPlan {
        project_type: nucleotide_events::ProjectType::Unknown,
        languages: language_ids
            .into_iter()
            .map(|language_id| PlannedProjectLanguage {
                language_id,
                evidence: ProjectLanguageEvidence::DiscoveredLanguage,
            })
            .collect(),
    }
}

pub(super) async fn discover_project_languages_with_backend(
    workspace_root: &Path,
    workspace_backend: WorkspaceBackendHandle,
    existing_languages: &HashSet<String>,
) -> Vec<String> {
    const MAX_ENTRIES: usize = 512;
    const MAX_DEPTH: usize = 4;

    let mut queue = VecDeque::from([(workspace_root.to_path_buf(), 0usize)]);
    let mut visited_entries = 0usize;
    let mut languages = HashSet::new();

    while let Some((directory, depth)) = queue.pop_front() {
        if visited_entries >= MAX_ENTRIES {
            break;
        }
        let Ok(listing) = workspace_backend.list_dir(&directory).await else {
            continue;
        };

        for entry in listing.entries {
            visited_entries += 1;
            if visited_entries > MAX_ENTRIES {
                break;
            }

            match entry.stat.kind {
                FileKind::Directory
                    if depth < MAX_DEPTH
                        && !matches!(
                            entry.name.as_str(),
                            ".git" | ".cache" | "node_modules" | "target" | "vendor"
                        ) =>
                {
                    queue.push_back((entry.path, depth + 1));
                }
                FileKind::File | FileKind::Symlink => {
                    if let Some(language_id) = discovered_language_id_for_name(&entry.name)
                        && !existing_languages.contains(language_id)
                    {
                        languages.insert(language_id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let mut languages = languages.into_iter().collect::<Vec<_>>();
    languages.sort();
    languages
}

fn discovered_language_id_for_name(name: &str) -> Option<&'static str> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some("rust"),
        "toml" => Some("toml"),
        "go" => Some("go"),
        "py" | "pyi" => Some("python"),
        "ts" | "tsx" => Some("typescript"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" => Some("cpp"),
        "md" | "mdx" => Some("markdown"),
        "sh" | "bash" => Some("bash"),
        "lua" => Some("lua"),
        "zig" => Some("zig"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> =
        std::sync::LazyLock::new(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime")
        });

    #[test]
    fn project_lsp_status_messages_describe_the_current_phase() {
        assert!(project_lsp_preparation_status(true, true).contains("remote project environment"));
        assert!(project_lsp_preparation_status(false, true).contains("project environment"));
        assert!(project_lsp_preparation_status(true, false).contains("when a file is opened"));

        assert_eq!(
            project_lsp_startup_status(&["rust-analyzer".to_string()], true),
            "Starting language servers: rust-analyzer"
        );
        assert!(project_lsp_startup_status(&[], true).contains("no language servers"));
        assert!(
            project_lsp_startup_status(&["rust-analyzer".to_string()], false)
                .contains("when a file is opened")
        );
    }

    #[test]
    fn supervisor_document_starts_wait_for_settled_session_and_dedupe_per_generation() {
        let root = PathBuf::from("/workspace/first");
        let mut supervisor = ProjectLspSupervisor::default();
        assert_eq!(supervisor.settled_generation(&root), None);

        let (response, _rx) = tokio::sync::oneshot::channel();
        let transition = supervisor
            .queue_session_open(root.clone(), response)
            .expect("first open schedules a session");
        assert_eq!(
            supervisor.settled_generation(&root),
            None,
            "in-flight sessions own startup decisions"
        );

        supervisor.complete_session(
            transition.generation,
            Ok(ProjectSessionResult {
                generation: transition.generation,
                plan: project_lsp_plan_from_names(&[]),
                language_servers: Vec::new(),
                servers_started: Vec::new(),
            }),
        );
        let generation = supervisor
            .settled_generation(&root)
            .expect("settled session exposes its generation");
        assert_eq!(generation, transition.generation);
        assert_eq!(
            supervisor.settled_generation(Path::new("/workspace/other")),
            None
        );

        assert!(supervisor.begin_document_start(generation, "rust", "rust-analyzer"));
        assert!(!supervisor.begin_document_start(generation, "rust", "rust-analyzer"));

        let next = supervisor.open_session(PathBuf::from("/workspace/second"));
        assert!(supervisor.begin_document_start(next.generation, "rust", "rust-analyzer"));
    }

    #[test]
    fn supervisor_makes_same_root_idempotent_and_rejects_stale_generations() {
        let first_root = PathBuf::from("/workspace/first");
        let second_root = PathBuf::from("/workspace/second");
        let mut supervisor = ProjectLspSupervisor::default();

        let first = supervisor.open_session(first_root.clone());
        assert!(first.previous_root.is_none());
        assert!(supervisor.is_current(first.generation, &first_root));

        let repeated = supervisor.open_session(first_root.clone());
        assert_eq!(repeated.generation, first.generation);

        let second = supervisor.open_session(second_root.clone());
        assert_eq!(second.previous_root.as_deref(), Some(first_root.as_path()));
        assert!(second.generation > first.generation);
        assert!(!supervisor.is_current(first.generation, &first_root));
        assert!(supervisor.is_current(second.generation, &second_root));
    }

    #[test]
    fn supervisor_coalesces_concurrent_session_requests() {
        let root = PathBuf::from("/workspace/project");
        let mut supervisor = ProjectLspSupervisor::default();
        let (first_tx, first_rx) = tokio::sync::oneshot::channel();
        let (second_tx, second_rx) = tokio::sync::oneshot::channel();

        let transition = supervisor
            .queue_session_open(root.clone(), first_tx)
            .expect("first request starts the session");
        assert!(
            supervisor
                .queue_session_open(root.clone(), second_tx)
                .is_none()
        );

        let result = ProjectSessionResult {
            generation: transition.generation,
            plan: ProjectLspPlan {
                project_type: nucleotide_events::ProjectType::Rust,
                languages: Vec::new(),
            },
            language_servers: Vec::new(),
            servers_started: Vec::new(),
        };
        supervisor.complete_session(transition.generation, Ok(result.clone()));

        let (first, second) = TEST_RUNTIME.block_on(async {
            (
                first_rx.await.unwrap().unwrap(),
                second_rx.await.unwrap().unwrap(),
            )
        });
        assert_eq!(first.generation, transition.generation);
        assert_eq!(second.generation, transition.generation);

        let (cached_tx, cached_rx) = tokio::sync::oneshot::channel();
        assert!(supervisor.queue_session_open(root, cached_tx).is_none());
        let cached = TEST_RUNTIME.block_on(cached_rx).unwrap().unwrap();
        assert_eq!(cached.generation, result.generation);
    }

    #[test]
    fn supervisor_restart_replaces_same_root_session_and_invalidates_waiters() {
        let root = PathBuf::from("/workspace/project");
        let mut supervisor = ProjectLspSupervisor::default();
        let (open_tx, open_rx) = tokio::sync::oneshot::channel();
        let opened = supervisor
            .queue_session_open(root.clone(), open_tx)
            .expect("open starts a session");
        let result = ProjectSessionResult {
            generation: opened.generation,
            plan: ProjectLspPlan {
                project_type: nucleotide_events::ProjectType::Rust,
                languages: Vec::new(),
            },
            language_servers: Vec::new(),
            servers_started: Vec::new(),
        };
        supervisor.complete_session(opened.generation, Ok(result));
        TEST_RUNTIME.block_on(open_rx).unwrap().unwrap();

        let (first_restart_tx, first_restart_rx) = tokio::sync::oneshot::channel();
        let first_restart = supervisor.queue_session_restart(root.clone(), first_restart_tx);
        assert_eq!(first_restart.previous_root.as_deref(), Some(root.as_path()));
        assert!(first_restart.generation > opened.generation);
        assert!(supervisor.session_result.is_none());

        let (coalesced_tx, coalesced_rx) = tokio::sync::oneshot::channel();
        assert!(
            supervisor
                .queue_session_open(root.clone(), coalesced_tx)
                .is_none()
        );

        let (second_restart_tx, _second_restart_rx) = tokio::sync::oneshot::channel();
        let second_restart = supervisor.queue_session_restart(root.clone(), second_restart_tx);
        assert_eq!(
            second_restart.previous_root.as_deref(),
            Some(root.as_path())
        );
        assert!(second_restart.generation > first_restart.generation);
        assert!(supervisor.is_current(second_restart.generation, &root));

        let (first_restart_result, coalesced_result) = TEST_RUNTIME
            .block_on(async { (first_restart_rx.await.unwrap(), coalesced_rx.await.unwrap()) });
        assert!(matches!(
            first_restart_result,
            Err(ProjectLspCommandError::StaleProjectSession)
        ));
        assert!(matches!(
            coalesced_result,
            Err(ProjectLspCommandError::StaleProjectSession)
        ));
    }
}
