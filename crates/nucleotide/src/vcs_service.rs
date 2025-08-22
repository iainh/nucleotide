// ABOUTME: Centralized VCS service for monitoring git status across the application
// ABOUTME: Provides events and queries for file modification status in version control

use gpui::{App, AppContext, Context, Entity, EventEmitter};
use nucleotide_logging::{debug, error, info, warn};
use nucleotide_ui::VcsStatus;
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

/// Central VCS service that monitors git status and broadcasts changes
pub struct VcsService {
    /// Root path of the repository being monitored
    root_path: Option<PathBuf>,
    /// Current VCS status cache
    status_cache: HashMap<PathBuf, VcsStatus>,
    /// Configuration
    config: VcsConfig,
    /// Last time VCS status was checked
    last_check: Option<Instant>,
    /// Whether monitoring is currently active
    is_monitoring: bool,
}

impl VcsService {
    /// Create a new VCS service
    pub fn new(config: VcsConfig) -> Self {
        Self {
            root_path: None,
            status_cache: HashMap::new(),
            config,
            last_check: None,
            is_monitoring: false,
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

    /// Force refresh of VCS status with rate limiting
    pub fn force_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.is_monitoring {
            return;
        }

        // Rate limit: don't refresh more than once every 2 seconds
        if let Some(last_check) = self.last_check {
            if last_check.elapsed().as_secs() < 2 {
                debug!(
                    "VCS: Force refresh requested but rate limited (last check was {} seconds ago)",
                    last_check.elapsed().as_secs()
                );
                return;
            }
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
        let Some(ref root_path) = self.root_path else {
            return;
        };

        debug!(root_path = %root_path.display(), "VCS: Refreshing status");

        // Run git status synchronously for now to avoid async issues
        match run_git_status(root_path, self.config.max_files) {
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
                    changes.insert(path.clone(), status.clone());
                }
                None => {
                    changes.insert(path.clone(), status.clone());
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

        // Update cache
        self.status_cache = new_status;

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
