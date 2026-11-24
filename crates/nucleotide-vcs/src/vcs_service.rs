// ABOUTME: Centralized VCS service for monitoring git status across the application
// ABOUTME: Provides events and queries for file modification status in version control

use gpui::{App, AppContext, Context, Entity, EventEmitter};
use nucleotide_logging::{debug, error, info};
use nucleotide_types::VcsStatus;
use nucleotide_workspace::{
    GitStatusEntry, GitStatusKind, GitStatusOptions, GitStatusResult, WorkspaceBackendHandle,
    absolutize_workspace_path, local_workspace_backend,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Events broadcast by the VCS service
#[derive(Debug, Clone)]
pub enum VcsEvent {
    /// VCS status has been updated for files
    StatusUpdated {
        /// Map of file paths to their VCS status
        changes: HashMap<PathBuf, VcsStatus>,
    },
    /// VCS service has started monitoring a repository
    RepositoryStarted {
        root_path: PathBuf,
        current_head: Option<String>,
    },
    /// VCS repository head changed
    RepositoryHeadChanged {
        root_path: PathBuf,
        previous_head: Option<String>,
        current_head: String,
    },
    /// VCS service encountered an error
    Error { message: String },
}

/// Configuration for the VCS service
#[derive(Debug, Clone)]
pub struct VcsConfig {
    /// How often to check for VCS changes (in milliseconds)
    pub poll_interval_ms: u64,
    /// Whether to enable VCS monitoring
    pub enabled: bool,
    /// Maximum number of files to track
    pub max_files: usize,
}

impl Default for VcsConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 2000, // Poll every 2 seconds
            enabled: true,
            max_files: 10000,
        }
    }
}

/// Central VCS service that monitors git status and broadcasts changes
pub struct VcsService {
    /// Root path of the repository being monitored
    root_path: Option<PathBuf>,
    /// Last repository head observed through the workspace backend.
    repository_head: Option<String>,
    /// Human-readable branch/ref for chrome; distinct from the diff base SHA.
    repository_ref: Option<String>,
    /// Workspace backend used for repository operations.
    workspace_backend: WorkspaceBackendHandle,
    /// Distinguishes async results across stop/start cycles, even for the same root.
    monitoring_generation: u64,
    /// Current VCS status cache
    status_cache: HashMap<PathBuf, VcsStatus>,
    /// Status of each directory that contains changes, derived from `status_cache`.
    directory_status: HashMap<PathBuf, VcsStatus>,
    /// Raw `git status` entries, which keep staged and unstaged changes apart.
    status_entries: Vec<GitStatusEntry>,
    /// Monotonic revision for presentation caches that consume status data.
    status_revision: u64,
    /// Why the last status refresh failed, until one succeeds.
    status_error: Option<String>,
    /// Whether the last status listed only the first `config.max_files` changes.
    status_truncated: bool,
    /// Configuration
    config: VcsConfig,
    /// Last time VCS status was checked
    last_check: Option<Instant>,
    /// Whether an async status refresh is already running
    status_refresh_in_flight: bool,
    /// Coalesces requests received during a refresh into one follow-up refresh.
    status_refresh_pending: bool,
    /// Whether monitoring is currently active
    is_monitoring: bool,
}

impl VcsService {
    /// Create a new VCS service
    pub fn new(config: VcsConfig) -> Self {
        Self {
            root_path: None,
            repository_head: None,
            repository_ref: None,
            workspace_backend: local_workspace_backend(),
            monitoring_generation: 0,
            status_cache: HashMap::new(),
            directory_status: HashMap::new(),
            status_entries: Vec::new(),
            status_revision: 0,
            status_error: None,
            status_truncated: false,
            config,
            last_check: None,
            status_refresh_in_flight: false,
            status_refresh_pending: false,
            is_monitoring: false,
        }
    }

    pub fn set_workspace_backend(&mut self, backend: WorkspaceBackendHandle) {
        self.workspace_backend = backend;
    }

    /// Start monitoring a repository
    pub fn start_monitoring(&mut self, root_path: PathBuf, cx: &mut Context<Self>) {
        // If we're already monitoring this directory, just refresh (with rate limiting)
        if self.root_path.as_ref() == Some(&root_path) {
            debug!(root_path = %root_path.display(), "VCS: Already monitoring this directory");
            if self.is_monitoring {
                // Only refresh if it's been a while since last check
                if let Some(last_check) = self.last_check {
                    if last_check.elapsed().as_secs() >= 3 {
                        debug!(
                            "VCS: Refreshing due to monitoring request (last check was {} seconds ago)",
                            last_check.elapsed().as_secs()
                        );
                        self.refresh_status_async(cx);
                    } else {
                        debug!(
                            "VCS: Skipping refresh due to recent check ({}s ago)",
                            last_check.elapsed().as_secs()
                        );
                    }
                } else {
                    // No previous check, do an initial refresh
                    self.refresh_status_async(cx);
                }
            }
            return;
        }

        info!(root_path = %root_path.display(), "VCS: Starting monitoring");

        // Stop monitoring previous directory if any
        if self.is_monitoring {
            self.stop_monitoring();
        }

        self.root_path = Some(root_path.clone());
        self.monitoring_generation = self.monitoring_generation.wrapping_add(1);
        self.repository_head = None;
        self.repository_ref = None;
        self.is_monitoring = self.config.enabled;

        if self.is_monitoring {
            // Initial status check. This runs asynchronously so startup does
            // not block on spawning git or reading repository state.
            self.refresh_status_async(cx);

            // Schedule periodic updates
            self.schedule_next_check();

            // Broadcast that we started monitoring
            self.emit_vcs_event(
                VcsEvent::RepositoryStarted {
                    root_path,
                    current_head: self.repository_head.clone(),
                },
                cx,
            );
        }
    }

    /// Stop monitoring
    pub fn stop_monitoring(&mut self) {
        info!("VCS: Stopping monitoring");
        self.is_monitoring = false;
        self.monitoring_generation = self.monitoring_generation.wrapping_add(1);
        self.root_path = None;
        self.repository_head = None;
        self.repository_ref = None;
        if !self.status_cache.is_empty() {
            self.status_cache.clear();
            self.status_revision = self.status_revision.wrapping_add(1);
        }
        self.directory_status.clear();
        self.status_entries.clear();
        self.status_error = None;
        self.status_truncated = false;
        self.last_check = None;
        self.status_refresh_in_flight = false;
        self.status_refresh_pending = false;
    }

    fn absolute_path(&self, path: &Path) -> Option<PathBuf> {
        self.root_path
            .as_ref()
            .map(|root| absolutize_workspace_path(root, path))
            .or_else(|| path.is_absolute().then(|| path.to_path_buf()))
    }

    /// Get the VCS status for a file, or for a directory the most important
    /// change inside it.
    pub fn get_status(&self, path: &Path) -> Option<VcsStatus> {
        // Convert to absolute path if relative
        let Some(abs_path) = self.absolute_path(path) else {
            debug!(path = %path.display(), "No root path set for VCS service");
            return None;
        };

        let status = self
            .status_cache
            .get(&abs_path)
            .or_else(|| self.directory_status.get(&abs_path))
            .copied();
        debug!(
            path = %abs_path.display(),
            status = ?status,
            cache_size = self.status_cache.len(),
            "VCS status lookup"
        );
        status
    }

    /// Get all files with VCS status
    pub fn get_all_status(&self) -> &HashMap<PathBuf, VcsStatus> {
        &self.status_cache
    }

    /// `git status` entries with separate index and working-tree states, and
    /// paths relative to the repository root. A change that only moves lines
    /// between staged and unstaged leaves `get_all_status` unchanged, so it
    /// bumps `status_revision` and notifies observers without a `StatusUpdated`
    /// event.
    pub fn status_entries(&self) -> &[GitStatusEntry] {
        &self.status_entries
    }

    pub fn status_revision(&self) -> u64 {
        self.status_revision
    }

    /// Why the last status refresh failed, cleared when one succeeds.
    pub fn status_error(&self) -> Option<&str> {
        self.status_error.as_deref()
    }

    /// The status limit, when the last status reached it and lists only that many changes.
    pub fn status_limit_reached(&self) -> Option<usize> {
        self.status_truncated.then_some(self.config.max_files)
    }

    pub fn repository_head(&self) -> Option<&str> {
        self.repository_head.as_deref()
    }

    pub fn repository_ref(&self) -> Option<&str> {
        self.repository_ref.as_deref()
    }

    /// Refresh VCS state after debounced filesystem watcher events.
    pub fn refresh_after_file_system_changes(
        &mut self,
        changed_paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        if !self.is_monitoring {
            debug!("VCS: Ignoring filesystem changes while monitoring is disabled");
            return;
        }

        let Some(root_path) = self.root_path.clone() else {
            debug!("VCS: Ignoring filesystem changes without a monitored root");
            return;
        };

        let change_count = changed_paths
            .iter()
            .filter_map(|path| self.absolute_path(path))
            .filter(|path| path.starts_with(&root_path))
            .count();

        if change_count == 0 {
            debug!("VCS: No filesystem changes within monitored root");
            return;
        }

        debug!(change_count, "VCS: Refreshing after filesystem changes");
        self.refresh_status_async(cx);
    }

    /// Force refresh of VCS status with rate limiting
    pub fn force_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.is_monitoring {
            return;
        }

        // Rate limit: don't refresh more than once every 2 seconds
        if let Some(last_check) = self.last_check
            && last_check.elapsed().as_secs() < 2
        {
            debug!(
                "VCS: Force refresh requested but rate limited (last check was {} seconds ago)",
                last_check.elapsed().as_secs()
            );
            return;
        }

        debug!("VCS: Force refresh requested");
        self.refresh_status_async(cx);
    }

    /// Refresh after the repository changed, such as when the app stages or
    /// commits, or the file tree sees git's HEAD, index or refs change.
    /// Unlike `force_refresh`, this is never rate limited.
    pub fn refresh_now(&mut self, cx: &mut Context<Self>) {
        if self.is_monitoring {
            self.refresh_status_async(cx);
        }
    }

    /// Check if a repository is being monitored
    pub fn is_monitoring(&self) -> bool {
        self.is_monitoring && self.root_path.is_some()
    }

    /// Get the root path being monitored
    pub fn root_path(&self) -> Option<&Path> {
        self.root_path.as_deref()
    }

    /// Refresh VCS status without blocking the UI/startup path.
    fn refresh_status_async(&mut self, cx: &mut Context<Self>) {
        let root_path = match &self.root_path {
            Some(path) => path.clone(),
            None => return,
        };

        if self.status_refresh_in_flight {
            self.status_refresh_pending = true;
            debug!(root_path = %root_path.display(), "VCS: Status refresh already in flight");
            return;
        }

        self.status_refresh_in_flight = true;
        let max_files = self.config.max_files;
        let refresh_root_path = root_path.clone();
        let workspace_backend = self.workspace_backend.clone();
        let monitoring_generation = self.monitoring_generation;

        debug!(root_path = %root_path.display(), "VCS: Starting async status refresh");
        cx.spawn(async move |this, cx| {
            let refresh_result = cx
                .background_executor()
                .spawn(async move {
                    run_git_refresh_with_backend(workspace_backend, &refresh_root_path, max_files)
                        .await
                })
                .await;

            if let Some(this) = this.upgrade() {
                this.update(cx, |service, cx| {
                    if service.monitoring_generation != monitoring_generation
                        || !service.is_monitoring
                    {
                        debug!(
                            root_path = %root_path.display(),
                            "VCS: Ignoring stale async status result"
                        );
                        return;
                    }

                    service.status_refresh_in_flight = false;
                    match refresh_result {
                        Ok(refresh) => {
                            debug!(
                                status_count = refresh.status.len(),
                                current_head = ?refresh.head,
                                "VCS: Got async git status results"
                            );
                            service.update_repository_state(refresh.head, refresh.display_ref, cx);
                            service.update_status_cache(refresh.status, cx);
                            service.update_status_entries(refresh.entries, cx);
                            if service.status_error.take().is_some()
                                || service.status_truncated != refresh.truncated
                            {
                                service.status_truncated = refresh.truncated;
                                cx.notify();
                            }
                        }
                        Err(error) => {
                            error!(error = %error, "VCS: Failed to get async git status");
                            let message = error;
                            // Each refresh retries; only a new failure is news.
                            if service.status_error.as_ref() != Some(&message) {
                                service.status_error = Some(message.clone());
                                cx.notify();
                                service.emit_vcs_event(VcsEvent::Error { message }, cx);
                            }
                        }
                    }

                    service.last_check = Some(Instant::now());
                    if std::mem::take(&mut service.status_refresh_pending) {
                        service.refresh_status_async(cx);
                    }
                });
            }
        })
        .detach();
    }

    fn update_repository_head(&mut self, new_head: Option<String>, cx: &mut Context<Self>) {
        let Some(current_head) = new_head else {
            return;
        };

        if self.repository_head.as_deref() == Some(current_head.as_str()) {
            return;
        }

        let previous_head = self.repository_head.replace(current_head.clone());
        if let Some(root_path) = self.root_path.clone() {
            self.emit_vcs_event(
                VcsEvent::RepositoryHeadChanged {
                    root_path,
                    previous_head,
                    current_head,
                },
                cx,
            );
        }
    }

    fn update_repository_state(
        &mut self,
        new_head: Option<String>,
        display_ref: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let display_ref_changed = self.repository_ref != display_ref;
        self.repository_ref = display_ref;
        self.update_repository_head(new_head, cx);

        if display_ref_changed {
            self.status_revision = self.status_revision.wrapping_add(1);
            cx.notify();
        }
    }

    /// Update the status cache and emit events for changes
    fn update_status_cache(
        &mut self,
        new_status: HashMap<PathBuf, VcsStatus>,
        cx: &mut Context<Self>,
    ) {
        let mut changes = HashMap::new();

        // Find changes
        for (path, status) in &new_status {
            match self.status_cache.get(path) {
                Some(old_status) if old_status != status => {
                    changes.insert(path.clone(), *status);
                }
                None => {
                    changes.insert(path.clone(), *status);
                }
                _ => {} // No change
            }
        }

        // Find removed files
        for path in self.status_cache.keys() {
            if !new_status.contains_key(path) {
                changes.insert(path.clone(), VcsStatus::Clean);
            }
        }

        self.status_cache = new_status;
        if !changes.is_empty() {
            self.directory_status = self
                .root_path
                .as_deref()
                .map(|root| directory_status(root, &self.status_cache))
                .unwrap_or_default();
            self.status_revision = self.status_revision.wrapping_add(1);
        }

        // Emit changes if any
        if !changes.is_empty() {
            debug!(change_count = changes.len(), "VCS: Status changes detected");
            self.emit_vcs_event(VcsEvent::StatusUpdated { changes }, cx);
        }
    }

    fn update_status_entries(&mut self, entries: Vec<GitStatusEntry>, cx: &mut Context<Self>) {
        if self.status_entries != entries {
            self.status_entries = entries;
            self.status_revision = self.status_revision.wrapping_add(1);
            cx.notify();
        }
    }

    /// Schedule the next status check
    fn schedule_next_check(&self) {
        // For now, we'll only refresh on demand to avoid async complexity
        // TODO: Add periodic background refresh using a timer
        debug!("VCS: Status check scheduled (currently on-demand only)");
    }
}

struct GitRefreshResult {
    status: HashMap<PathBuf, VcsStatus>,
    entries: Vec<GitStatusEntry>,
    truncated: bool,
    head: Option<String>,
    display_ref: Option<String>,
}

async fn run_git_refresh_with_backend(
    backend: WorkspaceBackendHandle,
    root_path: &Path,
    max_files: usize,
) -> Result<GitRefreshResult, String> {
    let head = backend
        .git_head(root_path)
        .await
        .map_err(|error| format!("Workspace git head failed: {error}"))?;
    let GitStatusResult {
        root,
        entries,
        truncated,
    } = run_git_status_with_backend(backend, root_path, max_files).await?;
    let status = entries
        .iter()
        .map(|entry| {
            (
                root.join(&entry.relative_path),
                vcs_status_from_workspace_entry(entry),
            )
        })
        .collect();

    Ok(GitRefreshResult {
        status,
        entries,
        truncated,
        head: head.head,
        display_ref: head.display_ref,
    })
}

async fn run_git_status_with_backend(
    backend: WorkspaceBackendHandle,
    root_path: &Path,
    max_files: usize,
) -> Result<GitStatusResult, String> {
    match backend
        .git_status(root_path, GitStatusOptions::with_limit(max_files))
        .await
    {
        Ok(status) => Ok(status),
        Err(error) if git_error_is_not_repository(&error.to_string()) => {
            debug!(
                root_path = %root_path.display(),
                "VCS: Monitored workspace root is not inside a git repository"
            );
            Ok(GitStatusResult {
                root: root_path.to_path_buf(),
                entries: Vec::new(),
                truncated: false,
            })
        }
        Err(error) => Err(format!("Workspace git status failed: {error}")),
    }
}

fn vcs_status_from_workspace_entry(entry: &GitStatusEntry) -> VcsStatus {
    use GitStatusKind::{
        Added, Conflicted, Copied, Deleted, Modified, Renamed, TypeChanged, Unknown, Unmodified,
        Untracked,
    };

    match (entry.index_status, entry.working_tree_status) {
        (Conflicted, _) | (_, Conflicted) => VcsStatus::Conflicted,
        (Untracked, _) | (_, Untracked) => VcsStatus::Untracked,
        (Deleted, _) | (_, Deleted) => VcsStatus::Deleted,
        (Renamed, _) | (_, Renamed) => VcsStatus::Renamed,
        (Added, Unmodified) => VcsStatus::Added,
        (Copied, Unmodified) => VcsStatus::Copied,
        (Modified | TypeChanged, _) | (_, Modified | TypeChanged) => VcsStatus::Modified,
        (Unknown, _) | (_, Unknown) => VcsStatus::Unknown,
        (Unmodified, Unmodified) => VcsStatus::Clean,
        (Added | Copied, _) | (_, Added | Copied) => VcsStatus::Modified,
    }
}

/// Rank used to pick a directory's status from the changes inside it.
fn directory_precedence(status: VcsStatus) -> u8 {
    match status {
        VcsStatus::Clean => 0,
        VcsStatus::Unknown => 1,
        VcsStatus::Untracked => 2,
        VcsStatus::Added => 3,
        VcsStatus::Copied => 4,
        VcsStatus::Renamed => 5,
        VcsStatus::Deleted => 6,
        VcsStatus::Modified => 7,
        VcsStatus::Conflicted => 8,
    }
}

/// Gives every directory below `root` the highest-precedence status among
/// the files inside it, so a collapsed folder still shows its changes.
fn directory_status(
    root: &Path,
    files: &HashMap<PathBuf, VcsStatus>,
) -> HashMap<PathBuf, VcsStatus> {
    let mut directories: HashMap<PathBuf, VcsStatus> = HashMap::new();
    for (path, &status) in files {
        if status == VcsStatus::Clean {
            continue;
        }
        for directory in path.ancestors().skip(1) {
            if directory == root || !directory.starts_with(root) {
                break;
            }
            match directories.get_mut(directory) {
                // Directories above this one rank at least as high as it does, so stop.
                Some(current) if directory_precedence(*current) >= directory_precedence(status) => {
                    break;
                }
                Some(current) => *current = status,
                None => {
                    directories.insert(directory.to_path_buf(), status);
                }
            }
        }
    }
    directories
}

fn git_error_is_not_repository(message: &str) -> bool {
    message.contains("not a git repository")
}

impl EventEmitter<VcsEvent> for VcsService {}

impl VcsService {
    /// Emit a VCS event to GPUI subscribers.
    fn emit_vcs_event(&self, event: VcsEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }
}

/// Global VCS service instance
pub struct VcsServiceHandle {
    service: Entity<VcsService>,
}

impl VcsServiceHandle {
    /// Create a new VCS service handle
    pub fn new(config: VcsConfig, cx: &mut App) -> Self {
        let service = cx.new(|_cx| VcsService::new(config));
        Self { service }
    }

    /// Get the VCS service model
    pub fn service(&self) -> &Entity<VcsService> {
        &self.service
    }

    /// Start monitoring a repository
    pub fn start_monitoring(&self, root_path: PathBuf, cx: &mut App) {
        self.service.update(cx, |service, cx| {
            service.start_monitoring(root_path, cx);
        });
    }

    /// Stop monitoring
    pub fn stop_monitoring(&self, cx: &mut App) {
        self.service.update(cx, |service, _cx| {
            service.stop_monitoring();
        });
    }

    /// Get VCS status for a file
    pub fn get_status(&self, path: &Path, cx: &App) -> Option<VcsStatus> {
        self.service.read(cx).get_status(path)
    }

    pub fn status_revision(&self, cx: &App) -> u64 {
        self.service.read(cx).status_revision()
    }

    pub fn repository_head(&self, cx: &App) -> Option<String> {
        self.service
            .read(cx)
            .repository_head()
            .map(ToOwned::to_owned)
    }

    pub fn repository_ref(&self, cx: &App) -> Option<String> {
        self.service
            .read(cx)
            .repository_ref()
            .map(ToOwned::to_owned)
    }

    /// Force refresh VCS status
    pub fn force_refresh(&self, cx: &mut App) {
        self.service.update(cx, |service, cx| {
            service.force_refresh(cx);
        });
    }

    /// Subscribe to VCS events
    pub fn subscribe<T>(&self, _subscriber: &Entity<T>, cx: &mut App) -> gpui::Subscription
    where
        T: EventEmitter<VcsEvent>,
    {
        cx.subscribe(&self.service, move |_subscriber, event: &VcsEvent, _cx| {
            // For now, just log the event
            debug!(event = ?event, "VCS event received");
        })
    }
}

impl gpui::Global for VcsServiceHandle {}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleotide_workspace::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    /// Gates actual Git snapshots so tests can change the workspace while a read is in flight.
    struct GatedBackend {
        inner: WorkspaceBackendHandle,
        executor: gpui::BackgroundExecutor,
        status_gates: Mutex<VecDeque<oneshot::Receiver<bool>>>,
    }

    impl GatedBackend {
        fn new(cx: &gpui::TestAppContext) -> Self {
            Self {
                inner: local_workspace_backend(),
                executor: cx.executor(),
                status_gates: Mutex::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl WorkspaceBackend for GatedBackend {
        fn identity(&self) -> WorkspaceIdentity {
            WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
                kind: RemoteWorkspaceKind::Other("test".into()),
                name: "gated local repository".into(),
            })
        }

        async fn git_head(&self, root: &Path) -> nucleotide_workspace::Result<GitHeadResult> {
            assert!(!self.executor.is_main_thread());
            self.inner.git_head(root).await
        }

        async fn git_status(
            &self,
            root: &Path,
            options: GitStatusOptions,
        ) -> nucleotide_workspace::Result<GitStatusResult> {
            assert!(!self.executor.is_main_thread());
            let snapshot = self.inner.git_status(root, options).await;
            let gate = self.status_gates.lock().unwrap().pop_front();
            if let Some(gate) = gate
                && gate.await.unwrap()
            {
                return Err(WorkspaceError::Io {
                    operation: "test git status",
                    path: root.to_path_buf(),
                    source: std::io::Error::other("injected status failure"),
                });
            }
            snapshot
        }

        async fn run_process(&self, _: ProcessSpec) -> nucleotide_workspace::Result<ProcessOutput> {
            unimplemented!()
        }
        async fn stat(&self, _: &Path) -> nucleotide_workspace::Result<FileStat> {
            unimplemented!()
        }
        async fn read_file(
            &self,
            _: &Path,
            _: ReadOptions,
        ) -> nucleotide_workspace::Result<FileRead> {
            unimplemented!()
        }

        async fn list_dir(&self, _: &Path) -> nucleotide_workspace::Result<DirectoryListing> {
            unimplemented!()
        }
        async fn find_ancestor_file(
            &self,
            _: &Path,
            _: &str,
            _: usize,
        ) -> nucleotide_workspace::Result<Option<PathBuf>> {
            unimplemented!()
        }
        async fn create_file(&self, _: &Path) -> nucleotide_workspace::Result<FileStat> {
            unimplemented!()
        }
        async fn create_dir(&self, _: &Path) -> nucleotide_workspace::Result<FileStat> {
            unimplemented!()
        }
        async fn rename_path(&self, _: &Path, _: &Path) -> nucleotide_workspace::Result<FileStat> {
            unimplemented!()
        }
        async fn delete_path(&self, _: &Path) -> nucleotide_workspace::Result<FileStat> {
            unimplemented!()
        }
        async fn copy_path(&self, _: &Path, _: &Path) -> nucleotide_workspace::Result<FileStat> {
            unimplemented!()
        }
        async fn write_file(
            &self,
            _: &Path,
            _: &[u8],
            _: WriteOptions,
        ) -> nucleotide_workspace::Result<WriteResult> {
            unimplemented!()
        }
        async fn file_search(
            &self,
            _: FileSearchQuery,
        ) -> nucleotide_workspace::Result<FileSearchResult> {
            unimplemented!()
        }
        async fn text_search(
            &self,
            _: TextSearchQuery,
        ) -> nucleotide_workspace::Result<TextSearchResult> {
            unimplemented!()
        }
        async fn project_environment(
            &self,
            _: &Path,
        ) -> nucleotide_workspace::Result<ProjectEnvironmentSnapshot> {
            unimplemented!()
        }
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = nucleotide_process::command("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn test_repository() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-b", "vcs-test"]);
        git(temp.path(), &["config", "user.name", "VCS Test"]);
        git(
            temp.path(),
            &["config", "user.email", "vcs-test@example.com"],
        );
        git(temp.path(), &["config", "commit.gpgsign", "false"]);
        git(temp.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(temp.path().join("file.txt"), "first\nsecond\nthird\n").unwrap();
        git(temp.path(), &["add", "."]);
        git(temp.path(), &["commit", "-m", "initial"]);
        temp
    }

    #[gpui::test]
    fn status_refresh_preserves_renamed_and_quoted_paths(cx: &mut gpui::TestAppContext) {
        let temp = test_repository();
        // Windows does not allow quotes, tabs or newlines in file names.
        let renamed = if cfg!(windows) {
            "renamed file.txt"
        } else {
            " renamed -> \"é\"\t.txt "
        };
        let untracked = if cfg!(windows) {
            "untracked file.txt"
        } else {
            " notes\n\"quoted\".txt "
        };
        git(temp.path(), &["mv", "file.txt", renamed]);
        std::fs::write(temp.path().join(untracked), "notes\n").unwrap();
        let service = cx.new(|_| VcsService::new(VcsConfig::default()));
        service.update(cx, |service, cx| {
            service.start_monitoring(temp.path().to_path_buf(), cx)
        });
        cx.run_until_parked();
        service.read_with(cx, |service, _| {
            assert_eq!(
                service.get_status(Path::new(renamed)),
                Some(VcsStatus::Renamed)
            );
            assert_eq!(
                service.get_status(Path::new(untracked)),
                Some(VcsStatus::Untracked)
            );
            assert_eq!(service.get_all_status().len(), 2);
            assert_eq!(
                service.repository_head(),
                Some(git(temp.path(), &["rev-parse", "HEAD"]).trim())
            );
            assert_eq!(service.repository_ref(), Some("vcs-test"));
        });
    }

    #[gpui::test]
    fn staging_right_after_a_refresh_updates_status_entries(cx: &mut gpui::TestAppContext) {
        let temp = test_repository();
        std::fs::write(temp.path().join("file.txt"), "changed\n").unwrap();
        let service = cx.new(|_| VcsService::new(VcsConfig::default()));
        service.update(cx, |service, cx| {
            service.start_monitoring(temp.path().to_path_buf(), cx)
        });
        cx.run_until_parked();
        let entry = |service: &VcsService| {
            let [entry] = service.status_entries() else {
                panic!("expected one entry: {:?}", service.status_entries());
            };
            (entry.index_status, entry.working_tree_status)
        };
        let revision = service.read_with(cx, |service, _| {
            assert_eq!(
                entry(service),
                (GitStatusKind::Unmodified, GitStatusKind::Modified)
            );
            service.status_revision()
        });

        // Within force_refresh's rate limit, and without changing the merged
        // `Modified` status.
        git(temp.path(), &["add", "file.txt"]);
        service.update(cx, |service, cx| service.refresh_now(cx));
        cx.run_until_parked();
        service.read_with(cx, |service, _| {
            assert_eq!(
                entry(service),
                (GitStatusKind::Modified, GitStatusKind::Unmodified)
            );
            assert_eq!(
                service.get_status(&temp.path().join("file.txt")),
                Some(VcsStatus::Modified)
            );
            assert_ne!(service.status_revision(), revision);
        });
    }

    #[gpui::test(iterations = 10)]
    fn refresh_requested_during_status_read_runs_after_success_or_error(
        cx: &mut gpui::TestAppContext,
    ) {
        for fail_first_read in [false, true] {
            let temp = test_repository();
            let backend = GatedBackend::new(cx);
            let (release, gate) = oneshot::channel();
            backend.status_gates.lock().unwrap().push_back(gate);
            let service = cx.new(|_| {
                let mut service = VcsService::new(VcsConfig::default());
                service.set_workspace_backend(Arc::new(backend));
                service
            });
            service.update(cx, |service, cx| {
                service.start_monitoring(temp.path().to_path_buf(), cx)
            });
            cx.run_until_parked();
            std::fs::write(temp.path().join("file.txt"), "changed\n").unwrap();
            service.update(cx, |service, cx| {
                service.force_refresh(cx);
                service.force_refresh(cx);
                service.force_refresh(cx);
            });
            release.send(fail_first_read).unwrap();
            cx.run_until_parked();
            service.read_with(cx, |service, _| {
                assert_eq!(
                    service.get_status(Path::new("file.txt")),
                    Some(VcsStatus::Modified)
                );
            });
        }
    }

    #[gpui::test]
    fn status_failure_and_truncation_last_until_a_refresh_clears_them(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = test_repository();
        std::fs::write(temp.path().join("a.txt"), "a\n").unwrap();
        std::fs::write(temp.path().join("b.txt"), "b\n").unwrap();
        let backend = GatedBackend::new(cx);
        let (first, first_gate) = oneshot::channel();
        let (second, second_gate) = oneshot::channel();
        backend
            .status_gates
            .lock()
            .unwrap()
            .extend([first_gate, second_gate]);
        let service = cx.new(|_| {
            let mut service = VcsService::new(VcsConfig {
                max_files: 1,
                ..VcsConfig::default()
            });
            service.set_workspace_backend(Arc::new(backend));
            service
        });
        let errors = Rc::new(RefCell::new(0));
        cx.update(|cx| {
            let errors = errors.clone();
            cx.subscribe(&service, move |_, event, _| {
                if matches!(event, VcsEvent::Error { .. }) {
                    *errors.borrow_mut() += 1;
                }
            })
            .detach();
        });
        service.update(cx, |service, cx| {
            service.start_monitoring(temp.path().to_path_buf(), cx)
        });
        first.send(true).unwrap();
        cx.run_until_parked();
        // Failing again with the same error is not news.
        service.update(cx, |service, cx| service.refresh_now(cx));
        second.send(true).unwrap();
        cx.run_until_parked();
        service.read_with(cx, |service, _| {
            assert!(
                service
                    .status_error()
                    .is_some_and(|error| error.contains("injected status failure")),
                "{:?}",
                service.status_error()
            );
            assert_eq!(service.status_limit_reached(), None);
        });
        assert_eq!(*errors.borrow(), 1);

        service.update(cx, |service, cx| service.refresh_now(cx));
        cx.run_until_parked();
        service.read_with(cx, |service, _| {
            assert_eq!(service.status_error(), None);
            assert_eq!(service.status_limit_reached(), Some(1));
            assert_eq!(service.status_entries().len(), 1);
        });

        std::fs::remove_file(temp.path().join("b.txt")).unwrap();
        service.update(cx, |service, cx| service.refresh_now(cx));
        cx.run_until_parked();
        service.read_with(cx, |service, _| {
            assert_eq!(service.status_limit_reached(), None)
        });
    }

    #[gpui::test(iterations = 10)]
    fn restarting_same_repository_ignores_old_status_completion(cx: &mut gpui::TestAppContext) {
        let temp = test_repository();
        let backend = GatedBackend::new(cx);
        let (old_release, old_gate) = oneshot::channel();
        let (new_release, new_gate) = oneshot::channel();
        backend
            .status_gates
            .lock()
            .unwrap()
            .extend([old_gate, new_gate]);
        let service = cx.new(|_| {
            let mut service = VcsService::new(VcsConfig::default());
            service.set_workspace_backend(Arc::new(backend));
            service
        });
        service.update(cx, |service, cx| {
            service.start_monitoring(temp.path().to_path_buf(), cx)
        });
        cx.run_until_parked();
        std::fs::write(temp.path().join("file.txt"), "changed\n").unwrap();
        service.update(cx, |service, cx| {
            service.force_refresh(cx);
            service.stop_monitoring();
            service.start_monitoring(temp.path().to_path_buf(), cx);
        });
        cx.run_until_parked();
        old_release.send(false).unwrap();
        cx.run_until_parked();
        service.read_with(cx, |service, _| assert_eq!(service.repository_head(), None));
        git(temp.path(), &["checkout", "--", "file.txt"]);
        service.update(cx, |service, cx| service.force_refresh(cx));
        new_release.send(false).unwrap();
        cx.run_until_parked();
        service.read_with(cx, |service, _| {
            assert!(service.get_all_status().is_empty())
        });
    }

    #[test]
    fn git_error_recognizes_not_repository_message() {
        assert!(git_error_is_not_repository(
            "fatal: not a git repository (or any of the parent directories): .git"
        ));
    }

    #[tokio::test]
    async fn local_git_status_returns_empty_outside_repository() {
        let temp = tempfile::tempdir().unwrap();

        let status = run_git_status_with_backend(local_workspace_backend(), temp.path(), 10)
            .await
            .unwrap();

        assert!(status.entries.is_empty());
    }

    #[test]
    fn absolute_path_keeps_remote_display_paths_rooted() {
        let mut service = VcsService::new(VcsConfig::default());
        let root = PathBuf::from("ssh://devbox/home/me/project");
        let rooted = PathBuf::from("ssh://devbox/home/me/project/src/lib.rs");
        service.root_path = Some(root);

        assert_eq!(service.absolute_path(&rooted), Some(rooted));
        assert_eq!(
            service.absolute_path(Path::new("src/main.rs")),
            Some(PathBuf::from("ssh://devbox/home/me/project/src/main.rs"))
        );
    }

    #[test]
    fn status_revision_changes_only_when_status_presentation_changes() {
        let mut service = VcsService::new(VcsConfig::default());
        service
            .status_cache
            .insert(PathBuf::from("/repo/src/lib.rs"), VcsStatus::Modified);

        assert_eq!(service.status_revision(), 0);
        service.stop_monitoring();
        assert_eq!(service.status_revision(), 1);

        service.stop_monitoring();
        assert_eq!(service.status_revision(), 1);
    }

    #[test]
    fn directories_take_the_most_important_change_inside_them() {
        let files: HashMap<PathBuf, VcsStatus> = [
            ("/repo/src/parser/token.rs", VcsStatus::Modified),
            ("/repo/src/render/shader.rs", VcsStatus::Added),
            ("/repo/src/render/old.rs", VcsStatus::Deleted),
            ("/repo/src/config.rs", VcsStatus::Conflicted),
            ("/repo/assets/icons/logo.svg", VcsStatus::Untracked),
            ("/repo/docs/clean.md", VcsStatus::Clean),
            ("/repo/README.md", VcsStatus::Modified),
            ("/elsewhere/lib.rs", VcsStatus::Modified),
        ]
        .into_iter()
        .map(|(path, status)| (PathBuf::from(path), status))
        .collect();

        let directories = directory_status(Path::new("/repo"), &files);
        let status = |path: &str| directories.get(Path::new(path)).copied();

        // A conflict outranks the modification and addition deeper down.
        assert_eq!(status("/repo/src"), Some(VcsStatus::Conflicted));
        assert_eq!(status("/repo/src/parser"), Some(VcsStatus::Modified));
        assert_eq!(status("/repo/src/render"), Some(VcsStatus::Deleted));
        // Every ancestor of a nested file is marked, not only its parent.
        assert_eq!(status("/repo/assets"), Some(VcsStatus::Untracked));
        assert_eq!(status("/repo/assets/icons"), Some(VcsStatus::Untracked));
        // Clean files, the workspace root and paths outside it are not marked.
        assert_eq!(status("/repo/docs"), None);
        assert_eq!(status("/repo"), None);
        assert_eq!(status("/elsewhere"), None);
        assert_eq!(directories.len(), 5);
    }

    #[test]
    fn workspace_git_status_entries_map_to_vcs_status() {
        let entry = |index_status, working_tree_status| GitStatusEntry {
            relative_path: PathBuf::from("src/lib.rs"),
            original_relative_path: None,
            index_status,
            working_tree_status,
        };

        assert_eq!(
            vcs_status_from_workspace_entry(&entry(
                GitStatusKind::Unmodified,
                GitStatusKind::Modified
            )),
            VcsStatus::Modified
        );
        assert_eq!(
            vcs_status_from_workspace_entry(&entry(
                GitStatusKind::Untracked,
                GitStatusKind::Untracked
            )),
            VcsStatus::Untracked
        );
        assert_eq!(
            vcs_status_from_workspace_entry(&entry(
                GitStatusKind::Added,
                GitStatusKind::Unmodified
            )),
            VcsStatus::Added
        );
        assert_eq!(
            vcs_status_from_workspace_entry(&entry(
                GitStatusKind::Conflicted,
                GitStatusKind::Modified
            )),
            VcsStatus::Conflicted
        );
    }
}
