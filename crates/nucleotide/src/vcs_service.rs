// ABOUTME: Centralized VCS service for monitoring git status across the application
// ABOUTME: Provides events and queries for file modification status in version control

use gpui::{App, AppContext, Context, Entity, EventEmitter};
use nucleotide_logging::{debug, error, info, warn};
use nucleotide_ui::VcsStatus;
use std::cell::RefCell;
use std::collections::HashMap;
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
    /// VCS service has started monitoring a repository
    RepositoryStarted { root_path: PathBuf },
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
    /// Configuration
    config: VcsConfig,
    /// Last time VCS status was checked
    last_check: Option<Instant>,
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

impl VcsService {
    /// Create a new VCS service
    pub fn new(config: VcsConfig) -> Self {
        Self {
            root_path: None,
            status_cache: HashMap::new(),
            cache_timestamps: HashMap::new(),
            config,
            last_check: None,
            cache_ttl: Duration::from_secs(5), // 5 second cache TTL
            is_monitoring: false,
            cache_stats: RefCell::new(CacheStats::default()),
            last_cache_cleanup: None,
            cache_cleanup_interval: Duration::from_secs(30), // Clean every 30 seconds
            max_cache_size: 5000,                            // Maximum 5000 cached entries
        }
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
                        self.refresh_status(cx);
                    } else {
                        debug!(
                            "VCS: Skipping refresh due to recent check ({}s ago)",
                            last_check.elapsed().as_secs()
                        );
                    }
                } else {
                    // No previous check, do an initial refresh
                    self.refresh_status(cx);
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
            // Initial status check
            self.refresh_status(cx);

            // Schedule periodic updates
            self.schedule_next_check(cx);

            // Broadcast that we started monitoring
            cx.emit(VcsEvent::RepositoryStarted { root_path });
        }
    }

    /// Stop monitoring
    pub fn stop_monitoring(&mut self) {
        info!("VCS: Stopping monitoring");
        self.is_monitoring = false;
        self.root_path = None;
        self.status_cache.clear();
        self.last_check = None;
    }

    /// Get the VCS status for a specific file
    pub fn get_status(&self, path: &Path) -> Option<VcsStatus> {
        // Convert to absolute path if relative
        let abs_path = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(ref root) = self.root_path {
            root.join(path)
        } else {
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

    /// Get VCS status with automatic cache refresh if stale
    pub fn get_status_cached(&self, path: &Path) -> Option<VcsStatus> {
        let abs_path = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(ref root) = self.root_path {
            root.join(path)
        } else {
            return None;
        };

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
            self.refresh_status(cx);
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
            self.refresh_status(cx);
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

        info!("VCS: Force refresh requested");
        self.refresh_status(cx);
    }

    /// Check if a repository is being monitored
    pub fn is_monitoring(&self) -> bool {
        self.is_monitoring && self.root_path.is_some()
    }

    /// Get the root path being monitored
    pub fn root_path(&self) -> Option<&Path> {
        self.root_path.as_deref()
    }

    /// Internal method to refresh VCS status
    fn refresh_status(&mut self, cx: &mut Context<Self>) {
        let root_path = match &self.root_path {
            Some(path) => path.clone(),
            None => return,
        };

        debug!(root_path = %root_path.display(), "VCS: Refreshing status");

        // Perform cache maintenance before refresh
        self.maintain_cache();

        // Run git status synchronously for now to avoid async issues
        match run_git_status(&root_path, self.config.max_files) {
            Ok(new_status) => {
                info!(
                    status_count = new_status.len(),
                    "VCS: Got git status results"
                );
                self.update_status_cache(new_status, cx);
            }
            Err(e) => {
                error!(error = %e, "VCS: Failed to get git status");
                cx.emit(VcsEvent::Error {
                    message: format!("Git status failed: {}", e),
                });
            }
        }

        self.last_check = Some(Instant::now());
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
                changes.insert(path.clone(), VcsStatus::UpToDate);
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
            cx.emit(VcsEvent::StatusUpdated { changes });
        }
    }

    /// Schedule the next status check
    fn schedule_next_check(&self, _cx: &mut Context<Self>) {
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
    let mut status_map = HashMap::new();

    // Run git status --porcelain
    let output = std::process::Command::new("git")
        .arg("status")
        .arg("--porcelain")
        .current_dir(root_path)
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if !output.status.success() {
        return Err(format!("Git command failed with status: {}", output.status));
    }

    let git_output = String::from_utf8_lossy(&output.stdout);
    let mut file_count = 0;

    for line in git_output.lines() {
        if file_count >= max_files {
            warn!(max_files, "VCS: Reached maximum file limit");
            break;
        }

        if line.len() >= 3 {
            let status_chars = &line[0..2];
            let file_path = line[3..].trim();

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

            let full_path = root_path.join(file_path);
            status_map.insert(full_path, status);
            file_count += 1;
        }
    }

    debug!(file_count, "VCS: Processed git status results");
    Ok(status_map)
}

impl EventEmitter<VcsEvent> for VcsService {}

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
