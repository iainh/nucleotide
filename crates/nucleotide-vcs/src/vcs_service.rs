// ABOUTME: Centralized VCS service for monitoring git status across the application
// ABOUTME: Provides events and queries for file modification status in version control

use gpui::{App, AppContext, Context, Entity, EventEmitter};
use helix_core::Rope;
use helix_vcs::{DiffHandle, DiffProviderRegistry, Hunk};
use nucleotide_env::{
    WslWorkspace, build_wsl_shell_command, load_wsl_remote_file_content_blocking,
};
use nucleotide_events::{
    EventBus,
    v2::vcs::{
        DiffHunk as DomainDiffHunk, Event as DomainVcsEvent, StageStatus as DomainStageStatus,
        WorkingStatus as DomainWorkingStatus,
    },
};
use nucleotide_logging::{debug, error, info, warn};
use nucleotide_remote::{FileContentResponse, decode_base64};
use nucleotide_types::{DiffChangeType, DiffHunkInfo, VcsStatus};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
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
    },
    /// VCS service has started monitoring a repository
    RepositoryStarted { root_path: PathBuf },
    /// VCS service encountered an error
    Error { message: String },
}

/// Convert VCS service event to domain event for the event bus
fn vcs_event_to_domain_events(event: &VcsEvent) -> Vec<DomainVcsEvent> {
    match event {
        VcsEvent::DiffHunksUpdated { file_path, hunks } => {
            // Convert DiffHunkInfo to DomainDiffHunk
            let domain_hunks: Vec<DomainDiffHunk> = hunks
                .iter()
                .map(|hunk| {
                    let change_type = match hunk.change_type {
                        DiffChangeType::Addition => {
                            nucleotide_events::v2::vcs::HunkChangeType::Addition
                        }
                        DiffChangeType::Deletion => {
                            nucleotide_events::v2::vcs::HunkChangeType::Deletion
                        }
                        DiffChangeType::Modification => {
                            nucleotide_events::v2::vcs::HunkChangeType::Modification
                        }
                    };

                    DomainDiffHunk {
                        after_start: hunk.after_start,
                        after_end: hunk.after_end,
                        before_start: hunk.before_start,
                        before_end: hunk.before_end,
                        change_type,
                    }
                })
                .collect();

            vec![DomainVcsEvent::DiffStatusChanged {
                doc_id: helix_view::DocumentId::default(), // TODO: Get actual doc_id when available
                path: file_path.clone(),
                hunks: domain_hunks,
                diff_base_revision: diff_base_revision_for_file(file_path),
            }]
        }
        VcsEvent::RepositoryStarted { root_path } => vec![DomainVcsEvent::RepositoryHeadChanged {
            repository_path: root_path.clone(),
            previous_head: None,
            current_head: current_git_head(root_path).unwrap_or_else(|| "HEAD".to_string()),
        }],
        VcsEvent::StatusUpdated { changes } => changes
            .iter()
            .map(|(path, status)| DomainVcsEvent::FileStageStatusChanged {
                path: path.clone(),
                stage_status: domain_stage_status(*status),
                working_status: domain_working_status(*status),
            })
            .collect(),
        VcsEvent::Error { .. } => {
            // Error events could be mapped to a domain error event if needed
            Vec::new()
        }
    }
}

fn domain_stage_status(status: VcsStatus) -> DomainStageStatus {
    match status {
        VcsStatus::Added => DomainStageStatus::Staged,
        _ => DomainStageStatus::Unstaged,
    }
}

fn domain_working_status(status: VcsStatus) -> Option<DomainWorkingStatus> {
    match status {
        VcsStatus::Modified => Some(DomainWorkingStatus::Modified),
        VcsStatus::Untracked => Some(DomainWorkingStatus::Untracked),
        VcsStatus::Deleted => Some(DomainWorkingStatus::Deleted),
        VcsStatus::Renamed => Some(DomainWorkingStatus::Renamed),
        VcsStatus::Conflicted => Some(DomainWorkingStatus::Conflicted),
        VcsStatus::Clean | VcsStatus::Added | VcsStatus::Unknown => None,
    }
}

fn current_git_head(root_path: &Path) -> Option<String> {
    let mut command = git_command_for_root(root_path, "rev-parse --verify HEAD");
    let output = command.output().ok()?;

    if !output.status.success() {
        return None;
    }

    parse_git_head_output(&output.stdout)
}

fn diff_base_revision_for_file(file_path: &Path) -> Option<String> {
    file_path.parent().and_then(current_git_head)
}

fn parse_git_head_output(stdout: &[u8]) -> Option<String> {
    let head = std::str::from_utf8(stdout).ok()?.trim();
    (!head.is_empty()).then(|| head.to_string())
}

fn git_command_for_root(root_path: &Path, script: &str) -> std::process::Command {
    if let Some(workspace) = WslWorkspace::from_unc_path(root_path) {
        build_wsl_shell_command(&workspace, "/bin/sh", script)
    } else {
        let mut command = nucleotide_process::command("git");
        command.current_dir(root_path);
        command.args(script.split_ascii_whitespace());
        command
    }
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
    /// Current VCS status cache
    status_cache: HashMap<PathBuf, VcsStatus>,
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
    /// Event bus for forwarding VCS events
    event_bus: Option<Box<dyn EventBus + Send + Sync>>,
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
const WSL_DIFF_CONTENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffContentSource {
    Local,
    WslRemote,
}

fn diff_content_source_for_path(path: &Path) -> DiffContentSource {
    if WslWorkspace::from_unc_path(path).is_some() {
        DiffContentSource::WslRemote
    } else {
        DiffContentSource::Local
    }
}

fn diff_content_from_disk(abs_path: &Path) -> Result<Option<Rope>, String> {
    match diff_content_source_for_path(abs_path) {
        DiffContentSource::Local => local_diff_content_from_disk(abs_path),
        DiffContentSource::WslRemote => wsl_remote_diff_content_from_disk(abs_path),
    }
}

fn local_diff_content_from_disk(abs_path: &Path) -> Result<Option<Rope>, String> {
    if !abs_path.is_file() {
        return Ok(None);
    }

    std::fs::read_to_string(abs_path)
        .map(|content| Some(Rope::from_str(&content)))
        .map_err(|error| error.to_string())
}

fn wsl_remote_diff_content_from_disk(abs_path: &Path) -> Result<Option<Rope>, String> {
    let response = load_wsl_remote_file_content_blocking(abs_path, WSL_DIFF_CONTENT_TIMEOUT)
        .map_err(|error| error.to_string())?;

    wsl_remote_diff_rope_from_response(response).map(Some)
}

fn wsl_remote_diff_rope_from_response(response: FileContentResponse) -> Result<Rope, String> {
    let bytes = decode_base64(&response.content_base64).map_err(|error| error.to_string())?;
    let content = String::from_utf8(bytes).map_err(|error| error.to_string())?;

    Ok(Rope::from_str(&content))
}

impl VcsService {
    /// Create a new VCS service
    pub fn new(config: VcsConfig) -> Self {
        Self {
            root_path: None,
            status_cache: HashMap::new(),
            cache_timestamps: HashMap::new(),
            diff_provider: DiffProviderRegistry::default(),
            diff_handles: HashMap::new(),
            diff_hunks_cache: HashMap::new(),
            diff_access_order: VecDeque::new(),
            event_bus: None,
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

    /// Set the event bus for forwarding VCS events
    pub fn set_event_bus(&mut self, event_bus: Box<dyn EventBus + Send + Sync>) {
        self.event_bus = Some(event_bus);
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
        self.is_monitoring = self.config.enabled;

        if self.is_monitoring {
            // Initial status check. This runs asynchronously so startup does
            // not block on spawning git or reading repository state.
            self.refresh_status_async(cx);

            // Schedule periodic updates
            self.schedule_next_check();

            // Broadcast that we started monitoring
            self.emit_vcs_event(VcsEvent::RepositoryStarted { root_path }, cx);
        }
    }

    /// Stop monitoring
    pub fn stop_monitoring(&mut self) {
        info!("VCS: Stopping monitoring");
        self.is_monitoring = false;
        self.root_path = None;
        self.status_cache.clear();
        self.diff_handles.clear();
        self.diff_hunks_cache.clear();
        self.diff_access_order.clear();
        self.last_check = None;
        self.status_refresh_in_flight = false;
    }

    fn absolute_path(&self, path: &Path) -> Option<PathBuf> {
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            self.root_path.as_ref().map(|root| root.join(path))
        }
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

        // Get the diff base from VCS
        if let Some(diff_base_bytes) = self.diff_provider.get_diff_base(&abs_path) {
            // Convert bytes to Rope
            let diff_base_string = String::from_utf8_lossy(&diff_base_bytes);
            let diff_base = Rope::from_str(&diff_base_string);

            // Create or update diff handle
            let diff_handle = DiffHandle::new(diff_base, file_content);

            // Load the diff and cache hunks
            let hunks: Vec<DiffHunkInfo> = {
                let diff = diff_handle.load();
                (0..diff.len())
                    .map(|i| diff.nth_hunk(i))
                    .map(|hunk| hunk_to_diff_info(&hunk))
                    .collect()
            };

            // Cache the hunks and emit event
            self.insert_file_diff(abs_path.clone(), diff_handle, hunks.clone());

            debug!(
                file_path = %abs_path.display(),
                hunk_count = hunks.len(),
                "Updated diff hunks for file"
            );

            // Emit diff hunks updated event
            self.emit_vcs_event(
                VcsEvent::DiffHunksUpdated {
                    file_path: abs_path,
                    hunks,
                },
                cx,
            );
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
                    },
                    cx,
                );
            }
        }
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
            self.refresh_diff_metadata_from_disk(&path, cx);
        }
    }

    fn refresh_diff_metadata_from_disk(&mut self, abs_path: &Path, cx: &mut Context<Self>) {
        match diff_content_from_disk(abs_path) {
            Ok(Some(content)) => self.update_file_diff(abs_path, content, cx),
            Ok(None) => self.clear_file_diff(abs_path, cx),
            Err(error) => {
                debug!(
                    file_path = %abs_path.display(),
                    error,
                    "VCS: Could not refresh diff metadata from disk"
                );
                self.clear_file_diff(abs_path, cx);
            }
        }
    }

    fn clear_file_diff(&mut self, abs_path: &Path, cx: &mut Context<Self>) {
        self.diff_handles.remove(abs_path);
        self.diff_access_order.retain(|path| path != abs_path);
        if self.diff_hunks_cache.remove(abs_path).is_some() {
            self.emit_vcs_event(
                VcsEvent::DiffHunksUpdated {
                    file_path: abs_path.to_path_buf(),
                    hunks: Vec::new(),
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

        debug!(root_path = %root_path.display(), "VCS: Starting async status refresh");
        cx.spawn(async move |this, cx| {
            let refresh_result = cx
                .background_executor()
                .spawn(async move { run_git_status(&refresh_root_path, max_files) })
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
                        Ok(new_status) => {
                            debug!(
                                status_count = new_status.len(),
                                "VCS: Got async git status results"
                            );
                            service.update_status_cache(new_status, cx);
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

/// Run git status command and parse results
fn run_git_status(
    root_path: &Path,
    max_files: usize,
) -> Result<HashMap<PathBuf, VcsStatus>, String> {
    let mut command = git_command_for_root(root_path, "status --porcelain");
    let output = command
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if !output.status.success() {
        return Err(format!("Git command failed with status: {}", output.status));
    }

    parse_git_status_output(root_path, &output.stdout, max_files)
}

fn parse_git_status_output(
    root_path: &Path,
    stdout: &[u8],
    max_files: usize,
) -> Result<HashMap<PathBuf, VcsStatus>, String> {
    let mut status_map = HashMap::new();
    let git_output = std::str::from_utf8(stdout)
        .map_err(|error| format!("Git status output was not UTF-8: {error}"))?;
    let mut file_count = 0;

    for line in git_output.lines() {
        if file_count >= max_files {
            warn!(max_files, "VCS: Reached maximum file limit");
            break;
        }

        if line.len() >= 3 {
            let status_chars = &line[0..2];
            let file_path = git_status_relative_path(status_chars, line[3..].trim());

            // Parse git status format
            let status = match status_chars {
                "??" => VcsStatus::Untracked,
                " M" | "MM" | " T" => VcsStatus::Modified,
                "M " | "MT" => VcsStatus::Modified,
                "A " => VcsStatus::Added,
                "AM" => VcsStatus::Modified, // Added but then modified
                " D" | "D " | "AD" => VcsStatus::Deleted,
                "R " | "RM" => VcsStatus::Renamed,
                "C " => VcsStatus::Added, // Copied, treat as added
                "UU" | "AA" | "DD" => VcsStatus::Conflicted,
                _ => continue, // Skip unknown status
            };

            let full_path = repository_path_from_git_relative(root_path, file_path);
            status_map.insert(full_path, status);
            file_count += 1;
        }
    }

    debug!(file_count, "VCS: Processed git status results");
    Ok(status_map)
}

fn git_status_relative_path<'a>(status_chars: &str, path: &'a str) -> &'a str {
    if matches!(status_chars, "R " | "RM" | "C ") {
        path.rsplit_once(" -> ")
            .map(|(_, new_path)| new_path)
            .unwrap_or(path)
    } else {
        path
    }
}

fn repository_path_from_git_relative(root_path: &Path, relative_path: &str) -> PathBuf {
    let mut full_path = root_path.to_path_buf();

    for component in relative_path
        .trim_matches('"')
        .split('/')
        .filter(|component| !component.is_empty())
    {
        full_path.push(component);
    }

    full_path
}

impl EventEmitter<VcsEvent> for VcsService {}

impl VcsService {
    /// Emit a VCS event and forward it to the event bus if available
    fn emit_vcs_event(&self, event: VcsEvent, cx: &mut Context<Self>) {
        // Forward to event bus if available
        if let Some(ref event_bus) = self.event_bus {
            for domain_event in vcs_event_to_domain_events(&event) {
                event_bus.dispatch_vcs(domain_event);
            }
        }

        // Also emit the local event for subscribers
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

    /// Set the event bus for forwarding VCS events to the application event system
    pub fn set_event_bus(&self, event_bus: Box<dyn EventBus + Send + Sync>, cx: &mut App) {
        self.service.update(cx, |service, _cx| {
            service.set_event_bus(event_bus);
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
    fn diff_content_source_uses_remote_helper_for_wsl_unc_paths() {
        assert_eq!(
            diff_content_source_for_path(Path::new(
                r"\\wsl.localhost\Ubuntu\home\iain\repo\src\lib.rs"
            )),
            DiffContentSource::WslRemote
        );
        assert_eq!(
            diff_content_source_for_path(Path::new(r"C:\Users\iain\repo\src\lib.rs")),
            DiffContentSource::Local
        );
    }

    #[test]
    fn local_diff_content_reads_utf8_files_and_skips_directories() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let file_path = temp.path().join("lib.rs");
        std::fs::write(&file_path, "pub fn answer() -> u8 { 42 }\n").expect("write file");

        let content = local_diff_content_from_disk(&file_path)
            .expect("read local diff content")
            .expect("file content");
        assert_eq!(content.to_string(), "pub fn answer() -> u8 { 42 }\n");

        assert!(
            local_diff_content_from_disk(temp.path())
                .expect("directory should not error")
                .is_none()
        );
    }

    #[test]
    fn wsl_remote_diff_rope_decodes_helper_content() {
        let response = FileContentResponse {
            protocol_version: nucleotide_remote::PROTOCOL_VERSION,
            current_dir: PathBuf::from("/home/iain/repo"),
            path: PathBuf::from("/home/iain/repo/src/lib.rs"),
            content_base64: nucleotide_remote::encode_base64(b"fn main() {}\n"),
            size: 13,
            modified_unix_millis: Some(1_234),
            readonly: false,
        };

        let content = wsl_remote_diff_rope_from_response(response).expect("decode content");

        assert_eq!(content.to_string(), "fn main() {}\n");
    }

    #[test]
    fn git_command_for_wsl_root_runs_inside_distribution() {
        let command = git_command_for_root(
            Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo"),
            "status --porcelain",
        );
        let command_debug = format!("{command:?}");

        assert!(command_debug.contains("wsl.exe"));
        assert!(command_debug.contains("--distribution"));
        assert!(command_debug.contains("Ubuntu"));
        assert!(command_debug.contains("status --porcelain"));
    }

    #[test]
    fn parse_git_status_output_maps_wsl_relative_paths_to_unc_root() {
        let root_path = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\iain\repo");
        let changes = parse_git_status_output(
            &root_path,
            b" M src/lib.rs\n?? README.md\nR  old.rs -> src/new.rs\n",
            10,
        )
        .expect("parse git status");

        assert_eq!(
            changes.get(&repository_path_from_git_relative(&root_path, "src/lib.rs")),
            Some(&VcsStatus::Modified)
        );
        assert_eq!(
            changes.get(&repository_path_from_git_relative(&root_path, "README.md")),
            Some(&VcsStatus::Untracked)
        );
        assert_eq!(
            changes.get(&repository_path_from_git_relative(&root_path, "src/new.rs")),
            Some(&VcsStatus::Renamed)
        );
    }

    #[test]
    fn parse_git_status_output_respects_max_files() {
        let root_path = PathBuf::from("/repo");
        let changes = parse_git_status_output(&root_path, b" M src/lib.rs\n?? README.md\n", 1)
            .expect("parse git status");

        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes.get(&repository_path_from_git_relative(&root_path, "src/lib.rs")),
            Some(&VcsStatus::Modified)
        );
    }

    #[test]
    fn status_updated_maps_each_changed_file_to_domain_event() {
        let modified = PathBuf::from("/repo/src/lib.rs");
        let untracked = PathBuf::from("/repo/notes.md");
        let clean = PathBuf::from("/repo/old.rs");
        let mut changes = HashMap::new();
        changes.insert(modified.clone(), VcsStatus::Modified);
        changes.insert(untracked.clone(), VcsStatus::Untracked);
        changes.insert(clean.clone(), VcsStatus::Clean);

        let mut events = vcs_event_to_domain_events(&VcsEvent::StatusUpdated { changes });
        events.sort_by(|a, b| {
            let a_path = match a {
                DomainVcsEvent::FileStageStatusChanged { path, .. } => path,
                _ => panic!("expected file stage status event"),
            };
            let b_path = match b {
                DomainVcsEvent::FileStageStatusChanged { path, .. } => path,
                _ => panic!("expected file stage status event"),
            };
            a_path.cmp(b_path)
        });

        assert_eq!(events.len(), 3);
        assert!(events.iter().any(|event| matches!(
            event,
            DomainVcsEvent::FileStageStatusChanged {
                path,
                stage_status: DomainStageStatus::Unstaged,
                working_status: Some(DomainWorkingStatus::Modified),
            } if path == &modified
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            DomainVcsEvent::FileStageStatusChanged {
                path,
                stage_status: DomainStageStatus::Unstaged,
                working_status: Some(DomainWorkingStatus::Untracked),
            } if path == &untracked
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            DomainVcsEvent::FileStageStatusChanged {
                path,
                stage_status: DomainStageStatus::Unstaged,
                working_status: None,
            } if path == &clean
        )));
    }

    #[test]
    fn added_status_maps_to_staged_domain_event() {
        let path = PathBuf::from("/repo/src/new.rs");
        let mut changes = HashMap::new();
        changes.insert(path.clone(), VcsStatus::Added);

        let events = vcs_event_to_domain_events(&VcsEvent::StatusUpdated { changes });

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DomainVcsEvent::FileStageStatusChanged {
                path: event_path,
                stage_status: DomainStageStatus::Staged,
                working_status: None,
            } if event_path == &path
        ));
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

    #[test]
    fn diff_hunks_include_current_head_as_base_revision() {
        let repo = tempfile::tempdir().expect("create temp git repository");
        run_git(repo.path(), &["init"]);
        run_git(repo.path(), &["config", "user.name", "Nucleotide Test"]);
        run_git(
            repo.path(),
            &["config", "user.email", "nucleotide-test@example.com"],
        );

        let file_path = repo.path().join("src/lib.rs");
        std::fs::create_dir_all(file_path.parent().expect("file has parent"))
            .expect("create source directory");
        std::fs::write(&file_path, "pub fn answer() -> u8 { 42 }\n").expect("write test file");
        run_git(repo.path(), &["add", "."]);
        run_git(repo.path(), &["commit", "-m", "initial"]);

        let head = current_git_head(repo.path()).expect("repo should have a HEAD commit");
        let events = vcs_event_to_domain_events(&VcsEvent::DiffHunksUpdated {
            file_path: file_path.clone(),
            hunks: vec![DiffHunkInfo {
                change_type: DiffChangeType::Modification,
                after_start: 1,
                after_end: 2,
                before_start: 1,
                before_end: 1,
            }],
        });

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DomainVcsEvent::DiffStatusChanged {
                path,
                diff_base_revision: Some(base_revision),
                ..
            } if path == &file_path && base_revision == &head
        ));
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("execute git command");

        assert!(
            output.status.success(),
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
