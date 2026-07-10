// ABOUTME: Centralized VCS service for monitoring git status across the application
// ABOUTME: Provides events and queries for file modification status in version control

use gpui::{App, AppContext, Context, Entity, EventEmitter};
use helix_core::Rope;
use helix_vcs::{DiffHandle, DiffProviderRegistry, Hunk};
use nucleotide_logging::{debug, error, info, warn};
use nucleotide_types::{DiffChangeType, DiffHunkInfo, VcsStatus};
use nucleotide_workspace::{
    FileKind, GitStatusEntry, GitStatusKind, GitStatusOptions, ProcessSpec, ReadOptions,
    WorkspaceBackendHandle, WorkspaceIdentity, absolutize_workspace_path,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Events broadcast by the VCS service
#[derive(Debug, Clone)]
pub enum VcsEvent {
    /// VCS status has been updated for files
    StatusUpdated {
        /// Map of file paths to their VCS status
        changes: HashMap<PathBuf, VcsStatus>,
    },
    /// VCS diff hunks have been updated for a file
    DiffHunksUpdated {
        /// Path to the file that changed
        file_path: PathBuf,
        /// Diff hunks for the file
        hunks: Vec<DiffHunkInfo>,
        /// Git revision used as the diff base, if known.
        diff_base_revision: Option<String>,
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

fn current_git_head(root_path: &Path) -> Option<String> {
    let output = nucleotide_process::command("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(root_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_git_head_output(&output.stdout)
}

fn parse_git_head_output(stdout: &[u8]) -> Option<String> {
    let head = std::str::from_utf8(stdout).ok()?.trim();
    (!head.is_empty()).then(|| head.to_string())
}

/// Convert Helix Hunk to DiffHunkInfo
fn hunk_to_diff_info(hunk: &Hunk) -> DiffHunkInfo {
    let change_type = if hunk.is_pure_insertion() {
        DiffChangeType::Addition
    } else if hunk.is_pure_removal() {
        DiffChangeType::Deletion
    } else {
        DiffChangeType::Modification
    };

    DiffHunkInfo {
        after_start: hunk.after.start,
        after_end: hunk.after.end,
        before_start: hunk.before.start,
        before_end: hunk.before.end,
        change_type,
    }
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

/// Cache performance statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of cache hits
    pub hits: u64,
    /// Number of cache misses
    pub misses: u64,
    /// Number of stale cache entries accessed
    pub stale_hits: u64,
    /// Number of cache invalidations performed
    pub invalidations: u64,
}

impl CacheStats {
    /// Calculate cache hit ratio as a percentage
    pub fn hit_ratio(&self) -> f64 {
        if self.hits + self.misses == 0 {
            0.0
        } else {
            (self.hits as f64) / ((self.hits + self.misses) as f64) * 100.0
        }
    }

    /// Reset all statistics
    pub fn reset(&mut self) {
        self.hits = 0;
        self.misses = 0;
        self.stale_hits = 0;
        self.invalidations = 0;
    }
}

/// Central VCS service that monitors git status and broadcasts changes
pub struct VcsService {
    /// Root path of the repository being monitored
    root_path: Option<PathBuf>,
    /// Last repository head observed through the workspace backend.
    repository_head: Option<String>,
    /// Workspace backend used for repository operations.
    workspace_backend: Option<WorkspaceBackendHandle>,
    /// Current VCS status cache
    status_cache: HashMap<PathBuf, VcsStatus>,
    /// Monotonic revision for presentation caches that consume status data.
    status_revision: u64,
    /// Timestamp of last cache update for each path
    cache_timestamps: HashMap<PathBuf, Instant>,
    /// Diff provider registry for getting diff base files
    diff_provider: DiffProviderRegistry,
    /// Active diff handles for files
    diff_handles: HashMap<PathBuf, DiffHandle>,
    /// Cached diff hunks for files
    diff_hunks_cache: HashMap<PathBuf, Vec<DiffHunkInfo>>,
    /// Access order for bounded diff metadata caches
    diff_access_order: VecDeque<PathBuf>,
    /// Configuration
    config: VcsConfig,
    /// Last time VCS status was checked
    last_check: Option<Instant>,
    /// Whether an async status refresh is already running
    status_refresh_in_flight: bool,
    /// Cache TTL for individual entries
    cache_ttl: Duration,
    /// Whether monitoring is currently active
    is_monitoring: bool,
    /// Cache hit/miss statistics for performance monitoring (interior mutability)
    cache_stats: RefCell<CacheStats>,
    /// Last time cache was cleaned of stale entries
    last_cache_cleanup: Option<Instant>,
    /// Interval for automatic cache cleanup
    cache_cleanup_interval: Duration,
    /// Maximum cache size before forcing cleanup
    max_cache_size: usize,
}

const DIFF_CACHE_CAPACITY: usize = 128;
const DIFF_METADATA_READ_LIMIT_BYTES: u64 = 4 * 1024 * 1024;
const DIFF_METADATA_COMMAND_TIMEOUT_MS: u64 = 10_000;

impl VcsService {
    /// Create a new VCS service
    pub fn new(config: VcsConfig) -> Self {
        Self {
            root_path: None,
            repository_head: None,
            workspace_backend: None,
            status_cache: HashMap::new(),
            status_revision: 0,
            cache_timestamps: HashMap::new(),
            diff_provider: DiffProviderRegistry::default(),
            diff_handles: HashMap::new(),
            diff_hunks_cache: HashMap::new(),
            diff_access_order: VecDeque::new(),
            config,
            last_check: None,
            status_refresh_in_flight: false,
            cache_ttl: Duration::from_secs(5), // 5 second cache TTL
            is_monitoring: false,
            cache_stats: RefCell::new(CacheStats::default()),
            last_cache_cleanup: None,
            cache_cleanup_interval: Duration::from_secs(30), // Clean every 30 seconds
            max_cache_size: 5000,                            // Maximum 5000 cached entries
        }
    }

    pub fn set_workspace_backend(&mut self, backend: WorkspaceBackendHandle) {
        self.workspace_backend = Some(backend);
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
        self.repository_head = None;
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
        self.root_path = None;
        self.repository_head = None;
        if !self.status_cache.is_empty() {
            self.status_cache.clear();
            self.status_revision = self.status_revision.wrapping_add(1);
        }
        self.diff_handles.clear();
        self.diff_hunks_cache.clear();
        self.diff_access_order.clear();
        self.last_check = None;
        self.status_refresh_in_flight = false;
    }

    fn absolute_path(&self, path: &Path) -> Option<PathBuf> {
        self.root_path
            .as_ref()
            .map(|root| absolutize_workspace_path(root, path))
            .or_else(|| path.is_absolute().then(|| path.to_path_buf()))
    }

    /// Get the VCS status for a specific file
    pub fn get_status(&self, path: &Path) -> Option<VcsStatus> {
        // Convert to absolute path if relative
        let Some(abs_path) = self.absolute_path(path) else {
            debug!(path = %path.display(), "No root path set for VCS service");
            return None;
        };

        let status = self.status_cache.get(&abs_path).cloned();
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

    pub fn status_revision(&self) -> u64 {
        self.status_revision
    }

    /// Get diff hunks for a specific file
    pub fn get_diff_hunks(&self, path: &Path) -> Option<&[DiffHunkInfo]> {
        let abs_path = self.absolute_path(path)?;

        self.diff_hunks_cache.get(&abs_path).map(|v| v.as_slice())
    }

    /// Create or update a diff handle for a file
    pub fn update_file_diff(
        &mut self,
        file_path: &Path,
        file_content: Rope,
        cx: &mut Context<Self>,
    ) {
        let Some(abs_path) = self.absolute_path(file_path) else {
            debug!("Cannot update file diff without root path");
            return;
        };

        if self
            .workspace_backend
            .as_ref()
            .is_some_and(|backend| matches!(backend.identity(), WorkspaceIdentity::Remote(_)))
        {
            self.update_remote_file_diff(abs_path, file_content, cx);
            return;
        }

        // Get the diff base from VCS
        if let Some(diff_base_bytes) = self.diff_provider.get_diff_base(&abs_path, true) {
            self.update_file_diff_from_base(abs_path, diff_base_bytes, file_content, cx);
        } else {
            debug!(
                file_path = %abs_path.display(),
                "No diff base found for file, removing diff data"
            );

            // Remove diff data if no base is available
            self.diff_handles.remove(&abs_path);
            if self.diff_hunks_cache.remove(&abs_path).is_some() {
                // Emit empty diff if we had hunks before
                self.emit_vcs_event(
                    VcsEvent::DiffHunksUpdated {
                        file_path: abs_path,
                        hunks: vec![],
                        diff_base_revision: self.repository_head.clone(),
                    },
                    cx,
                );
            }
        }
    }

    fn update_remote_file_diff(
        &mut self,
        abs_path: PathBuf,
        file_content: Rope,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_backend) = self.workspace_backend.clone() else {
            self.clear_file_diff(&abs_path, cx);
            return;
        };

        cx.spawn(async move |this, cx| {
            let path = abs_path.clone();
            let diff_base_result =
                cx.background_executor()
                    .spawn(async move {
                        read_git_diff_base_from_workspace(workspace_backend, &path).await
                    })
                    .await;

            if let Some(this) = this.upgrade() {
                this.update(cx, |service, cx| {
                    let still_monitored = service
                        .root_path
                        .as_ref()
                        .is_some_and(|root_path| abs_path.starts_with(root_path))
                        && service.is_monitoring;

                    if !still_monitored {
                        debug!(
                            file_path = %abs_path.display(),
                            "VCS: Ignoring stale remote diff update"
                        );
                        return;
                    }

                    match diff_base_result {
                        Ok(Some(diff_base_bytes)) => {
                            service.update_file_diff_from_base(
                                abs_path,
                                diff_base_bytes,
                                file_content,
                                cx,
                            );
                        }
                        Ok(None) => {
                            service.clear_file_diff(&abs_path, cx);
                        }
                        Err(error) => {
                            debug!(
                                file_path = %abs_path.display(),
                                error = %error,
                                "VCS: Could not load remote diff base"
                            );
                            service.clear_file_diff(&abs_path, cx);
                        }
                    }
                });
            }
        })
        .detach();
    }

    fn update_file_diff_from_base(
        &mut self,
        abs_path: PathBuf,
        diff_base_bytes: Vec<u8>,
        file_content: Rope,
        cx: &mut Context<Self>,
    ) {
        let diff_base_string = String::from_utf8_lossy(&diff_base_bytes);
        let diff_base = Rope::from_str(&diff_base_string);
        let diff_handle = DiffHandle::new(diff_base, file_content);

        let hunks: Vec<DiffHunkInfo> = {
            let diff = diff_handle.load();
            (0..diff.len())
                .map(|i| diff.nth_hunk(i))
                .map(|hunk| hunk_to_diff_info(&hunk))
                .collect()
        };

        self.insert_file_diff(abs_path.clone(), diff_handle, hunks.clone());

        debug!(
            file_path = %abs_path.display(),
            hunk_count = hunks.len(),
            "Updated diff hunks for file"
        );

        self.emit_vcs_event(
            VcsEvent::DiffHunksUpdated {
                file_path: abs_path,
                hunks,
                diff_base_revision: self.repository_head.clone(),
            },
            cx,
        );
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

        let mut seen = HashSet::new();
        let affected_paths: Vec<PathBuf> = changed_paths
            .iter()
            .filter_map(|path| self.absolute_path(path))
            .filter(|path| path.starts_with(&root_path))
            .filter(|path| seen.insert(path.clone()))
            .collect();

        if affected_paths.is_empty() {
            debug!("VCS: No filesystem changes within monitored root");
            return;
        }

        debug!(
            change_count = affected_paths.len(),
            "VCS: Refreshing after filesystem changes"
        );
        self.refresh_status_async(cx);

        for path in affected_paths {
            self.refresh_diff_metadata_from_workspace(&path, cx);
        }
    }

    fn refresh_diff_metadata_from_workspace(&mut self, abs_path: &Path, cx: &mut Context<Self>) {
        let path = abs_path.to_path_buf();
        let workspace_backend = self.workspace_backend.clone();

        cx.spawn(async move |this, cx| {
            let path_for_read = path.clone();
            let read_result = cx
                .background_executor()
                .spawn(async move {
                    read_diff_text_from_workspace(workspace_backend, &path_for_read).await
                })
                .await;

            if let Some(this) = this.upgrade() {
                this.update(cx, |service, cx| {
                    let still_monitored = service
                        .root_path
                        .as_ref()
                        .is_some_and(|root_path| path.starts_with(root_path))
                        && service.is_monitoring;

                    if !still_monitored {
                        debug!(
                            file_path = %path.display(),
                            "VCS: Ignoring stale diff metadata refresh"
                        );
                        return;
                    }

                    match read_result {
                        Ok(Some(content)) => {
                            service.update_file_diff(&path, Rope::from_str(&content), cx);
                        }
                        Ok(None) => {
                            service.clear_file_diff(&path, cx);
                        }
                        Err(error) => {
                            debug!(
                                file_path = %path.display(),
                                error = %error,
                                "VCS: Could not refresh diff metadata from workspace"
                            );
                            service.clear_file_diff(&path, cx);
                        }
                    }
                });
            }
        })
        .detach();
    }

    fn clear_file_diff(&mut self, abs_path: &Path, cx: &mut Context<Self>) {
        self.diff_handles.remove(abs_path);
        self.diff_access_order.retain(|path| path != abs_path);
        if self.diff_hunks_cache.remove(abs_path).is_some() {
            self.emit_vcs_event(
                VcsEvent::DiffHunksUpdated {
                    file_path: abs_path.to_path_buf(),
                    hunks: Vec::new(),
                    diff_base_revision: self.repository_head.clone(),
                },
                cx,
            );
        }
    }

    fn insert_file_diff(
        &mut self,
        abs_path: PathBuf,
        diff_handle: DiffHandle,
        hunks: Vec<DiffHunkInfo>,
    ) {
        self.diff_handles.insert(abs_path.clone(), diff_handle);
        self.diff_hunks_cache.insert(abs_path.clone(), hunks);
        self.touch_diff_cache_entry(&abs_path);
        self.evict_diff_cache_to_capacity();
    }

    fn touch_diff_cache_entry(&mut self, abs_path: &Path) {
        self.diff_access_order.retain(|path| path != abs_path);
        self.diff_access_order.push_back(abs_path.to_path_buf());
    }

    fn evict_diff_cache_to_capacity(&mut self) {
        while self.diff_handles.len() > DIFF_CACHE_CAPACITY
            || self.diff_hunks_cache.len() > DIFF_CACHE_CAPACITY
        {
            let Some(evicted) = self.diff_access_order.pop_front() else {
                break;
            };
            self.diff_handles.remove(&evicted);
            self.diff_hunks_cache.remove(&evicted);
        }
    }

    /// Get VCS status with automatic cache refresh if stale
    pub fn get_status_cached(&self, path: &Path) -> Option<VcsStatus> {
        let abs_path = self.absolute_path(path)?;

        // Check if cache entry exists and is still fresh
        if let Some(timestamp) = self.cache_timestamps.get(&abs_path) {
            if timestamp.elapsed() < self.cache_ttl {
                // Fresh cache hit - update statistics
                if let Ok(mut stats) = self.cache_stats.try_borrow_mut() {
                    stats.hits += 1;
                }
                return self.status_cache.get(&abs_path).copied();
            } else {
                // Stale cache hit - update statistics
                if let Ok(mut stats) = self.cache_stats.try_borrow_mut() {
                    stats.stale_hits += 1;
                }
            }
        } else {
            // Cache miss - update statistics
            if let Ok(mut stats) = self.cache_stats.try_borrow_mut() {
                stats.misses += 1;
            }
        }

        // Fallback to regular get_status if cache is stale or missing
        self.status_cache.get(&abs_path).copied()
    }

    /// Bulk update cache for multiple paths (efficient for pickers)
    pub fn update_cache_bulk(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        debug!(path_count = paths.len(), "Bulk updating VCS cache");

        // For now, trigger a full refresh if we have many paths to check
        // In a more sophisticated implementation, we could run git status
        // specifically for these paths
        if paths.len() > 10 && self.root_path.is_some() {
            self.refresh_status_async(cx);
        }
    }

    /// Get VCS status for multiple paths at once (bulk operation)
    pub fn get_status_bulk(&self, paths: &[PathBuf]) -> Vec<(PathBuf, Option<VcsStatus>)> {
        let mut results = Vec::with_capacity(paths.len());

        for path in paths {
            let abs_path = if path.is_absolute() {
                path.clone()
            } else if let Some(ref root) = self.root_path {
                root.join(path)
            } else {
                results.push((path.clone(), None));
                continue;
            };

            // Check cache with statistics tracking
            let status = if let Some(timestamp) = self.cache_timestamps.get(&abs_path) {
                if timestamp.elapsed() < self.cache_ttl {
                    // Fresh cache hit
                    if let Ok(mut stats) = self.cache_stats.try_borrow_mut() {
                        stats.hits += 1;
                    }
                    self.status_cache.get(&abs_path).copied()
                } else {
                    // Stale cache hit
                    if let Ok(mut stats) = self.cache_stats.try_borrow_mut() {
                        stats.stale_hits += 1;
                    }
                    self.status_cache.get(&abs_path).copied()
                }
            } else {
                // Cache miss
                if let Ok(mut stats) = self.cache_stats.try_borrow_mut() {
                    stats.misses += 1;
                }
                None
            };

            results.push((path.clone(), status));
        }

        debug!(
            requested_count = paths.len(),
            results_count = results.len(),
            "Bulk VCS status lookup completed"
        );

        results
    }

    /// Populate cache for picker items efficiently
    pub fn populate_picker_cache(
        &mut self,
        picker_paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) -> Vec<(PathBuf, Option<VcsStatus>)> {
        debug!(
            picker_path_count = picker_paths.len(),
            "Populating picker cache"
        );

        // Check how many paths are missing from cache
        let missing_count = picker_paths
            .iter()
            .filter(|path| {
                let abs_path = if path.is_absolute() {
                    (*path).clone()
                } else if let Some(ref root) = self.root_path {
                    root.join(path)
                } else {
                    return true; // No root path, consider missing
                };

                !self.is_cache_fresh(&abs_path)
            })
            .count();

        // If more than 25% of paths are missing/stale, do a full refresh first
        if missing_count > picker_paths.len() / 4 && self.root_path.is_some() {
            info!(
                missing_count,
                total_count = picker_paths.len(),
                "High cache miss ratio, performing full refresh before picker population"
            );
            self.refresh_status_async(cx);
        }

        // Now return the bulk results
        self.get_status_bulk(picker_paths)
    }

    /// Check if cache entry is fresh
    fn is_cache_fresh(&self, path: &Path) -> bool {
        if let Some(timestamp) = self.cache_timestamps.get(path) {
            timestamp.elapsed() < self.cache_ttl
        } else {
            false
        }
    }

    /// Clear stale cache entries
    pub fn clear_stale_cache(&mut self) {
        let now = Instant::now();
        let stale_paths: Vec<PathBuf> = self
            .cache_timestamps
            .iter()
            .filter(|(_, timestamp)| now.duration_since(**timestamp) > self.cache_ttl)
            .map(|(path, _)| path.clone())
            .collect();

        let stale_count = stale_paths.len();
        for path in stale_paths {
            self.status_cache.remove(&path);
            self.cache_timestamps.remove(&path);
        }

        if stale_count > 0 {
            self.status_revision = self.status_revision.wrapping_add(1);
            if let Ok(mut stats) = self.cache_stats.try_borrow_mut() {
                stats.invalidations += stale_count as u64;
            }
            debug!(stale_count, "Cleared stale cache entries");
        }

        self.last_cache_cleanup = Some(now);
    }

    /// Get cache statistics for performance monitoring
    pub fn get_cache_stats(&self) -> CacheStats {
        self.cache_stats.borrow().clone()
    }

    /// Reset cache statistics
    pub fn reset_cache_stats(&self) {
        if let Ok(mut stats) = self.cache_stats.try_borrow_mut() {
            stats.reset();
        }
    }

    /// Get cache size information
    pub fn get_cache_info(&self) -> (usize, usize, f64) {
        let cache_size = self.status_cache.len();
        let stats = self.cache_stats.borrow();
        let hit_ratio = stats.hit_ratio();
        (cache_size, self.max_cache_size, hit_ratio)
    }

    /// Perform automatic cache maintenance
    pub fn maintain_cache(&mut self) {
        let now = Instant::now();

        // Check if it's time for automatic cleanup
        let should_cleanup = match self.last_cache_cleanup {
            Some(last) => now.duration_since(last) >= self.cache_cleanup_interval,
            None => true, // First cleanup
        };

        // Force cleanup if cache is too large
        let force_cleanup = self.status_cache.len() > self.max_cache_size;

        if should_cleanup || force_cleanup {
            if force_cleanup {
                debug!(
                    current_size = self.status_cache.len(),
                    max_size = self.max_cache_size,
                    "Cache size exceeded maximum, forcing cleanup"
                );
            }
            self.clear_stale_cache();
        }
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
            debug!(root_path = %root_path.display(), "VCS: Status refresh already in flight");
            return;
        }

        self.maintain_cache();
        self.status_refresh_in_flight = true;
        let max_files = self.config.max_files;
        let refresh_root_path = root_path.clone();
        let workspace_backend = self.workspace_backend.clone();

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
                    service.status_refresh_in_flight = false;

                    if service.root_path.as_ref() != Some(&root_path) || !service.is_monitoring {
                        debug!(
                            root_path = %root_path.display(),
                            "VCS: Ignoring stale async status result"
                        );
                        return;
                    }

                    match refresh_result {
                        Ok(refresh) => {
                            debug!(
                                status_count = refresh.status.len(),
                                current_head = ?refresh.head,
                                "VCS: Got async git status results"
                            );
                            service.update_repository_head(refresh.head, cx);
                            service.update_status_cache(refresh.status, cx);
                        }
                        Err(error) => {
                            error!(error = %error, "VCS: Failed to get async git status");
                            service.emit_vcs_event(
                                VcsEvent::Error {
                                    message: format!("Git status failed: {}", error),
                                },
                                cx,
                            );
                        }
                    }

                    service.last_check = Some(Instant::now());
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

        // Update cache and timestamps
        let now = Instant::now();
        self.status_cache = new_status;
        if !changes.is_empty() {
            self.status_revision = self.status_revision.wrapping_add(1);
        }

        // Update timestamps for all paths in the new status
        for path in self.status_cache.keys() {
            self.cache_timestamps.insert(path.clone(), now);
        }

        // Remove timestamps for paths no longer in cache
        let paths_to_remove: Vec<PathBuf> = self
            .cache_timestamps
            .keys()
            .filter(|path| !self.status_cache.contains_key(*path))
            .cloned()
            .collect();

        for path in paths_to_remove {
            self.cache_timestamps.remove(&path);
        }

        // Emit changes if any
        if !changes.is_empty() {
            debug!(change_count = changes.len(), "VCS: Status changes detected");
            self.emit_vcs_event(VcsEvent::StatusUpdated { changes }, cx);
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
    head: Option<String>,
}

async fn read_diff_text_from_workspace(
    backend: Option<WorkspaceBackendHandle>,
    path: &Path,
) -> Result<Option<String>, String> {
    let Some(backend) = backend else {
        return read_local_diff_text(path);
    };

    let stat = backend
        .stat(path)
        .await
        .map_err(|error| format!("Workspace stat failed: {error}"))?;
    if stat.kind != FileKind::File {
        return Ok(None);
    }

    let read = backend
        .read_file(
            path,
            ReadOptions {
                max_bytes: Some(DIFF_METADATA_READ_LIMIT_BYTES),
            },
        )
        .await
        .map_err(|error| format!("Workspace file read failed: {error}"))?;

    if read.truncated {
        return Ok(None);
    }

    String::from_utf8(read.bytes)
        .map(Some)
        .map_err(|error| format!("Workspace file is not valid UTF-8: {error}"))
}

async fn read_git_diff_base_from_workspace(
    backend: WorkspaceBackendHandle,
    path: &Path,
) -> Result<Option<Vec<u8>>, String> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    let Some(file_name) = path.file_name() else {
        return Ok(None);
    };

    let ls_files = backend
        .run_process(ProcessSpec {
            program: "git".to_string(),
            args: vec![
                "ls-files".to_string(),
                "-z".to_string(),
                "--full-name".to_string(),
                "--".to_string(),
                file_name.to_string_lossy().into_owned(),
            ],
            cwd: parent.to_path_buf(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: Some(DIFF_METADATA_READ_LIMIT_BYTES as usize),
            timeout_ms: Some(DIFF_METADATA_COMMAND_TIMEOUT_MS),
        })
        .await
        .map_err(|error| format!("Workspace git ls-files failed: {error}"))?;

    if !ls_files.success || ls_files.timed_out || ls_files.stdout_truncated {
        return Ok(None);
    }

    let relative_path = ls_files
        .stdout
        .split(|byte| *byte == 0)
        .find(|entry| !entry.is_empty());
    let Some(relative_path) = relative_path else {
        return Ok(None);
    };
    let relative_path = String::from_utf8_lossy(relative_path);

    let show = backend
        .run_process(ProcessSpec {
            program: "git".to_string(),
            args: vec!["show".to_string(), format!("HEAD:{relative_path}")],
            cwd: parent.to_path_buf(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: Some(DIFF_METADATA_READ_LIMIT_BYTES as usize),
            timeout_ms: Some(DIFF_METADATA_COMMAND_TIMEOUT_MS),
        })
        .await
        .map_err(|error| format!("Workspace git show failed: {error}"))?;

    if show.success && !show.timed_out && !show.stdout_truncated {
        Ok(Some(show.stdout))
    } else {
        Ok(None)
    }
}

fn read_local_diff_text(path: &Path) -> Result<Option<String>, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("Local file metadata failed: {error}"))?;
    if !metadata.is_file() || metadata.len() > DIFF_METADATA_READ_LIMIT_BYTES {
        return Ok(None);
    }

    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|error| format!("Local file read failed: {error}"))
}

async fn run_git_refresh_with_backend(
    backend: Option<WorkspaceBackendHandle>,
    root_path: &Path,
    max_files: usize,
) -> Result<GitRefreshResult, String> {
    let head = run_git_head_with_backend(backend.clone(), root_path).await?;
    let status = run_git_status_with_backend(backend, root_path, max_files).await?;

    Ok(GitRefreshResult { status, head })
}

async fn run_git_head_with_backend(
    backend: Option<WorkspaceBackendHandle>,
    root_path: &Path,
) -> Result<Option<String>, String> {
    let Some(backend) = backend else {
        return Ok(current_git_head(root_path));
    };

    backend
        .git_head(root_path)
        .await
        .map(|result| result.head)
        .map_err(|error| format!("Workspace git head failed: {error}"))
}

async fn run_git_status_with_backend(
    backend: Option<WorkspaceBackendHandle>,
    root_path: &Path,
    max_files: usize,
) -> Result<HashMap<PathBuf, VcsStatus>, String> {
    let Some(backend) = backend else {
        return run_git_status(root_path, max_files);
    };

    let status = match backend
        .git_status(root_path, GitStatusOptions::with_limit(max_files))
        .await
    {
        Ok(status) => status,
        Err(error) if git_error_is_not_repository(&error.to_string()) => {
            debug!(
                root_path = %root_path.display(),
                "VCS: Monitored workspace root is not inside a git repository"
            );
            return Ok(HashMap::new());
        }
        Err(error) => return Err(format!("Workspace git status failed: {error}")),
    };

    Ok(status
        .entries
        .into_iter()
        .map(|entry| {
            (
                status.root.join(&entry.relative_path),
                vcs_status_from_workspace_entry(&entry),
            )
        })
        .collect())
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
        (Copied, Unmodified) => VcsStatus::Added,
        (Modified | TypeChanged, _) | (_, Modified | TypeChanged) => VcsStatus::Modified,
        (Unknown, _) | (_, Unknown) => VcsStatus::Unknown,
        (Unmodified, Unmodified) => VcsStatus::Clean,
        (Added | Copied, _) | (_, Added | Copied) => VcsStatus::Modified,
    }
}

fn parse_git_status_line(line: &str) -> Option<(&str, VcsStatus)> {
    let bytes = line.as_bytes();
    if bytes.len() < 3 {
        return None;
    }

    let status = match &bytes[0..2] {
        b"??" => VcsStatus::Untracked,
        b" M" | b"MM" | b" T" => VcsStatus::Modified,
        b"M " | b"MT" => VcsStatus::Modified,
        b"A " => VcsStatus::Added,
        b"AM" => VcsStatus::Modified,
        b" D" | b"D " | b"AD" => VcsStatus::Deleted,
        b"R " | b"RM" => VcsStatus::Renamed,
        b"C " => VcsStatus::Added,
        b"UU" | b"AA" | b"DD" => VcsStatus::Conflicted,
        _ => return None,
    };

    let file_path = line.get(3..)?.trim();
    Some((file_path, status))
}

/// Run git status command and parse results
fn run_git_status(
    root_path: &Path,
    max_files: usize,
) -> Result<HashMap<PathBuf, VcsStatus>, String> {
    let mut status_map = HashMap::new();

    // Run git status --porcelain
    let output = nucleotide_process::command("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(root_path)
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if !output.status.success() {
        if git_error_is_not_repository(&String::from_utf8_lossy(&output.stderr)) {
            debug!(
                root_path = %root_path.display(),
                "VCS: Monitored root is not inside a git repository"
            );
            return Ok(HashMap::new());
        }
        return Err(format!("Git command failed with status: {}", output.status));
    }

    let git_output = String::from_utf8_lossy(&output.stdout);
    let mut file_count = 0;

    for line in git_output.lines() {
        if file_count >= max_files {
            warn!(max_files, "VCS: Reached maximum file limit");
            break;
        }

        if let Some((file_path, status)) = parse_git_status_line(line) {
            let full_path = root_path.join(file_path);
            status_map.insert(full_path, status);
            file_count += 1;
        }
    }

    debug!(file_count, "VCS: Processed git status results");
    Ok(status_map)
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

    /// Force refresh VCS status
    pub fn force_refresh(&self, cx: &mut App) {
        self.service.update(cx, |service, cx| {
            service.force_refresh(cx);
        });
    }

    /// Get VCS status with caching (preferred method for all components)
    pub fn get_status_cached(&self, path: &Path, cx: &App) -> Option<VcsStatus> {
        self.service.read(cx).get_status_cached(path)
    }

    /// Bulk update cache for multiple paths (efficient for pickers)
    pub fn update_cache_bulk(&self, paths: &[PathBuf], cx: &mut App) {
        self.service.update(cx, |service, cx| {
            service.update_cache_bulk(paths, cx);
        });
    }

    /// Clear stale cache entries
    pub fn clear_stale_cache(&self, cx: &mut App) {
        self.service.update(cx, |service, _cx| {
            service.clear_stale_cache();
        });
    }

    /// Get cache statistics for performance monitoring
    pub fn get_cache_stats(&self, cx: &App) -> CacheStats {
        self.service.read(cx).get_cache_stats()
    }

    /// Reset cache statistics
    pub fn reset_cache_stats(&self, cx: &App) {
        self.service.read(cx).reset_cache_stats()
    }

    /// Get cache size and hit ratio information
    pub fn get_cache_info(&self, cx: &App) -> (usize, usize, f64) {
        self.service.read(cx).get_cache_info()
    }

    /// Perform manual cache maintenance
    pub fn maintain_cache(&self, cx: &mut App) {
        self.service.update(cx, |service, _cx| {
            service.maintain_cache();
        });
    }

    /// Get VCS status for multiple paths at once (bulk operation)
    pub fn get_status_bulk(
        &self,
        paths: &[PathBuf],
        cx: &App,
    ) -> Vec<(PathBuf, Option<VcsStatus>)> {
        self.service.read(cx).get_status_bulk(paths)
    }

    /// Populate cache for picker items efficiently
    pub fn populate_picker_cache(
        &self,
        picker_paths: &[PathBuf],
        cx: &mut App,
    ) -> Vec<(PathBuf, Option<VcsStatus>)> {
        self.service.update(cx, |service, cx| {
            service.populate_picker_cache(picker_paths, cx)
        })
    }

    /// Get diff hunks for a file
    pub fn get_diff_hunks(&self, path: &Path, cx: &App) -> Option<Vec<DiffHunkInfo>> {
        self.service
            .read(cx)
            .get_diff_hunks(path)
            .map(|s| s.to_vec())
    }

    /// Update file diff hunks
    pub fn update_file_diff(&self, file_path: &Path, file_content: helix_core::Rope, cx: &mut App) {
        self.service.update(cx, |service, cx| {
            service.update_file_diff(file_path, file_content, cx);
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
    use nucleotide_workspace::{
        WorkspacePathMapping, local_workspace_backend, path_mapped_workspace_backend,
    };

    fn insert_test_diff(service: &mut VcsService, index: usize) -> PathBuf {
        let path = PathBuf::from(format!("/repo/file-{index}.rs"));
        let diff_handle = DiffHandle::new(Rope::from_str("base\n"), Rope::from_str("current\n"));
        service.insert_file_diff(path.clone(), diff_handle, Vec::new());
        path
    }

    #[test]
    fn parse_git_head_output_trims_sha() {
        assert_eq!(
            parse_git_head_output(b"abc123\n"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn parse_git_head_output_rejects_empty_output() {
        assert_eq!(parse_git_head_output(b"\n"), None);
    }

    #[test]
    fn parse_git_status_line_reads_ascii_status_prefix() {
        assert_eq!(
            parse_git_status_line(" M src/lib.rs"),
            Some(("src/lib.rs", VcsStatus::Modified))
        );
        assert_eq!(
            parse_git_status_line("?? docs/notes.md"),
            Some(("docs/notes.md", VcsStatus::Untracked))
        );
    }

    #[test]
    fn parse_git_status_line_ignores_unicode_without_panicking() {
        assert_eq!(parse_git_status_line("↪ src/lib.rs"), None);
    }

    #[test]
    fn git_error_recognizes_not_repository_message() {
        assert!(git_error_is_not_repository(
            "fatal: not a git repository (or any of the parent directories): .git"
        ));
    }

    #[test]
    fn local_git_status_returns_empty_outside_repository() {
        let temp = tempfile::tempdir().unwrap();

        let status = run_git_status(temp.path(), 10).unwrap();

        assert!(status.is_empty());
    }

    #[tokio::test]
    async fn diff_metadata_read_uses_backend_for_display_paths() {
        let temp = tempfile::tempdir().unwrap();
        let native_root = temp.path().to_path_buf();
        let native_src = native_root.join("src");
        std::fs::create_dir_all(&native_src).unwrap();
        std::fs::write(native_src.join("lib.rs"), "remote text\n").unwrap();

        let display_root = PathBuf::from("/__nucleotide_remote_virtual__/project");
        let display_file = display_root.join("src/lib.rs");
        let backend = path_mapped_workspace_backend(
            local_workspace_backend(),
            WorkspacePathMapping::new(display_root, native_root),
        );

        let text = read_diff_text_from_workspace(Some(backend), &display_file)
            .await
            .unwrap();

        assert_eq!(text.as_deref(), Some("remote text\n"));
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

    #[tokio::test]
    async fn diff_metadata_read_skips_large_backend_files() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("large.rs");
        std::fs::write(
            &file,
            vec![b'a'; DIFF_METADATA_READ_LIMIT_BYTES as usize + 1],
        )
        .unwrap();

        let text = read_diff_text_from_workspace(Some(local_workspace_backend()), &file)
            .await
            .unwrap();

        assert!(text.is_none());
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

    #[tokio::test]
    async fn diff_cache_stays_within_capacity() {
        let mut service = VcsService::new(VcsConfig::default());

        for index in 0..(DIFF_CACHE_CAPACITY + 10) {
            insert_test_diff(&mut service, index);
        }

        assert_eq!(service.diff_handles.len(), DIFF_CACHE_CAPACITY);
        assert_eq!(service.diff_hunks_cache.len(), DIFF_CACHE_CAPACITY);
        assert!(
            !service
                .diff_handles
                .contains_key(&PathBuf::from("/repo/file-0.rs"))
        );
        assert!(service.diff_handles.contains_key(&PathBuf::from(format!(
            "/repo/file-{}.rs",
            DIFF_CACHE_CAPACITY + 9
        ))));
    }

    #[tokio::test]
    async fn diff_cache_keeps_refreshed_entries() {
        let mut service = VcsService::new(VcsConfig::default());

        for index in 0..DIFF_CACHE_CAPACITY {
            insert_test_diff(&mut service, index);
        }
        insert_test_diff(&mut service, 0);
        insert_test_diff(&mut service, DIFF_CACHE_CAPACITY);

        assert!(
            service
                .diff_handles
                .contains_key(&PathBuf::from("/repo/file-0.rs"))
        );
        assert!(
            !service
                .diff_handles
                .contains_key(&PathBuf::from("/repo/file-1.rs"))
        );
        assert_eq!(service.diff_handles.len(), DIFF_CACHE_CAPACITY);
    }
}
