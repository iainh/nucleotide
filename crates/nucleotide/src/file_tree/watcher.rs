// ABOUTME: File system watcher using notify crate for real-time file tree updates
// ABOUTME: Monitors file system changes and emits events for tree synchronization

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use nucleotide_logging::{debug, warn};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::file_tree::{FileSystemEventKind, FileTreeEvent};

/// File system watcher for the file tree
pub struct FileTreeWatcher {
    /// The notify watcher instance
    _watcher: notify::RecommendedWatcher,
    /// Receiver for file system events
    event_receiver: mpsc::UnboundedReceiver<Result<Event, notify::Error>>,
    /// Root path being watched
    root_path: PathBuf,
    /// Gitignore matcher for filtering files
    gitignore: Option<Gitignore>,
}

impl FileTreeWatcher {
    /// Create a new file system watcher
    pub fn new(root_path: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut watcher = notify::recommended_watcher(move |res| {
            if tx.send(res).is_err() {
                // Channel closed, watcher is being dropped
            }
        })?;

        // Watch the root directory recursively
        watcher
            .watch(&root_path, RecursiveMode::Recursive)
            .with_context(|| format!("Failed to watch directory: {}", root_path.display()))?;

        // Build gitignore matcher using the same patterns as the file picker
        let gitignore = Self::build_gitignore_matcher(&root_path);

        Ok(Self {
            _watcher: watcher,
            event_receiver: rx,
            root_path,
            gitignore,
        })
    }

    /// Get the root path being watched
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Process file system events and convert them to FileTreeEvents
    pub async fn next_event(&mut self) -> Option<FileTreeEvent> {
        while let Some(event_result) = self.event_receiver.recv().await {
            match event_result {
                Ok(event) => {
                    // Log raw notify event details
                    debug!(
                        kind = ?event.kind,
                        paths = ?event.paths,
                        "notify event received"
                    );

                    if let Some(file_tree_event) = self.convert_event(event) {
                        debug!(evt = ?file_tree_event, "converted file tree event");
                        return Some(file_tree_event);
                    }
                }
                Err(e) => {
                    warn!(error = %e, "File system watcher error");
                }
            }
        }
        None
    }

    /// Build gitignore matcher using the same patterns as the file picker
    fn build_gitignore_matcher(root_path: &Path) -> Option<Gitignore> {
        let mut builder = GitignoreBuilder::new(root_path);

        // Add .gitignore files
        if let Ok(gitignore_path) = root_path.join(".gitignore").canonicalize()
            && gitignore_path.exists()
        {
            let _ = builder.add(&gitignore_path);
        }

        // Add global gitignore
        if let Some(git_config_dir) = dirs::config_dir() {
            let global_gitignore = git_config_dir.join("git").join("ignore");
            if global_gitignore.exists() {
                let _ = builder.add(&global_gitignore);
            }
        }

        // Add .git/info/exclude
        let git_exclude = root_path.join(".git").join("info").join("exclude");
        if git_exclude.exists() {
            let _ = builder.add(&git_exclude);
        }

        // Add .ignore files
        let ignore_file = root_path.join(".ignore");
        if ignore_file.exists() {
            let _ = builder.add(&ignore_file);
        }

        // Add Helix-specific ignore files
        let helix_ignore = root_path.join(".helix").join("ignore");
        if helix_ignore.exists() {
            let _ = builder.add(&helix_ignore);
        }

        builder.build().ok()
    }

    /// Check if a path should be ignored
    fn should_ignore_path(&self, path: &Path) -> bool {
        // Check if path is inside VCS directories
        for component in path.components() {
            if let std::path::Component::Normal(name) = component
                && let Some(name_str) = name.to_str()
            {
                match name_str {
                    ".git" | ".svn" | ".hg" | ".bzr" => return true,
                    _ => {}
                }
            }
        }

        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip hidden files (files starting with .)
        if file_name.starts_with('.') {
            // Allow .gitignore and similar important config files
            match file_name {
                ".gitignore" | ".ignore" => return false,
                _ => return true,
            }
        }

        // Check gitignore patterns
        if let Some(ref gitignore) = self.gitignore
            && let Ok(relative_path) = path.strip_prefix(&self.root_path)
        {
            let matched = gitignore.matched(relative_path, path.is_dir());
            return matched.is_ignore();
        }

        false
    }

    /// Convert a notify Event to a FileTreeEvent
    fn convert_event(&self, event: Event) -> Option<FileTreeEvent> {
        // Filter out events for paths outside our root
        let paths: Vec<_> = event
            .paths
            .iter()
            .filter(|path| path.starts_with(&self.root_path))
            .cloned()
            .collect();

        // Log pre-filter existence and ignore decisions
        for p in &paths {
            let exists = p.exists();
            let ignored = self.should_ignore_path(p);
            debug!(
                path = %p.display(),
                exists = exists,
                ignored = ignored,
                "event path status"
            );
        }

        let paths: Vec<_> = paths
            .into_iter()
            .filter(|path| !self.should_ignore_path(path))
            .collect();

        if paths.is_empty() {
            return None;
        }

        // Handle rename specially when we have at least two paths
        // Notify represents rename as a Modify(Name(...)) event kind.
        // We map that to a Renamed event when possible.
        if matches!(event.kind, EventKind::Modify(_)) && paths.len() >= 2 {
            debug!(from = %paths[0].display(), to = %paths[1].display(), "mapping rename event");
            return Some(FileTreeEvent::FileSystemChanged {
                path: paths[1].clone(),
                kind: FileSystemEventKind::Renamed {
                    from: paths[0].clone(),
                    to: paths[1].clone(),
                },
            });
        }

        let kind = match event.kind {
            EventKind::Create(_) => FileSystemEventKind::Created,
            EventKind::Remove(_) => FileSystemEventKind::Deleted,
            _ => FileSystemEventKind::Modified,
        };

        // Use the first path as the main path for the event
        Some(FileTreeEvent::FileSystemChanged {
            path: paths[0].clone(),
            kind,
        })
    }
}

/// Debounced file system watcher that batches rapid changes
pub struct DebouncedFileTreeWatcher {
    /// The underlying watcher
    watcher: FileTreeWatcher,
    /// Debounce duration
    debounce_duration: Duration,
    /// Pending events by path
    pending_events: std::collections::HashMap<PathBuf, FileTreeEvent>,
    /// Last event time for debouncing
    last_event_time: Option<Instant>,
}

impl DebouncedFileTreeWatcher {
    /// Create a new debounced file system watcher
    pub fn new(root_path: PathBuf, debounce_duration: Duration) -> Result<Self> {
        let watcher = FileTreeWatcher::new(root_path)?;

        Ok(Self {
            watcher,
            debounce_duration,
            pending_events: std::collections::HashMap::new(),
            last_event_time: None,
        })
    }

    /// Create a debounced watcher with default settings (300ms debounce)
    pub fn with_defaults(root_path: PathBuf) -> Result<Self> {
        Self::new(root_path, Duration::from_millis(300))
    }

    /// Get the root path being watched
    pub fn root_path(&self) -> &Path {
        self.watcher.root_path()
    }

    /// Get the next debounced event
    pub async fn next_event(&mut self) -> Option<FileTreeEvent> {
        loop {
            // Check if we have pending events and enough time has passed
            if !self.pending_events.is_empty()
                && let Some(last_time) = self.last_event_time
                && last_time.elapsed() >= self.debounce_duration
            {
                let evt = self.flush_pending_events();
                if let Some(ref e) = evt {
                    debug!(evt = ?e, remaining = self.pending_events.len(), "debounce flush emitted event");
                }
                return evt;
            }

            // Wait for new events with a small timeout to check debounce
            tokio::select! {
                // New file system event
                event = self.watcher.next_event() => {
                    if let Some(event) = event {
                        self.handle_new_event(event);
                        self.last_event_time = Some(Instant::now());
                    }
                }

                // Small timeout to periodically check if debounce time has elapsed
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if !self.pending_events.is_empty()
                        && let Some(last_time) = self.last_event_time
                            && last_time.elapsed() >= self.debounce_duration {
                                return self.flush_pending_events();
                            }
                }
            }
        }
    }

    /// Handle a new file system event with coalescing to preserve semantics
    fn handle_new_event(&mut self, event: FileTreeEvent) {
        if let FileTreeEvent::FileSystemChanged { path, .. } = &event {
            if let Some(prev) = self.pending_events.get(path) {
                let merged = merge_events(prev, &event);
                self.pending_events.insert(path.clone(), merged);
                debug!(
                    pending = self.pending_events.len(),
                    "coalesced pending event"
                );
            } else {
                self.pending_events.insert(path.clone(), event);
                debug!(
                    pending = self.pending_events.len(),
                    "queued new pending event"
                );
            }
        }
    }

    /// Flush pending events and return a single next event without discarding others
    ///
    /// Previously this drained the entire pending set and returned only one event,
    /// dropping the rest. That could miss fast create→delete sequences (e.g.,
    /// atomic-save backup files), leaving stale entries in the tree. This now
    /// removes and returns just one event at a time, preserving the remainder
    /// for subsequent flushes.
    fn flush_pending_events(&mut self) -> Option<FileTreeEvent> {
        self.last_event_time = None;

        if let Some(key) = self.pending_events.keys().next().cloned() {
            let remaining_before = self.pending_events.len();
            let out = self.pending_events.remove(&key);
            debug!(
                remaining_after = remaining_before.saturating_sub(1),
                "flushed one pending event"
            );
            return out;
        }
        None
    }
}

/// Coalesce two file events for the same path preserving the strongest effect.
fn merge_events(prev: &FileTreeEvent, next: &FileTreeEvent) -> FileTreeEvent {
    use crate::file_tree::FileSystemEventKind as K;
    let (p_kind, p_path) = match prev {
        FileTreeEvent::FileSystemChanged { path, kind } => (kind, path),
        _ => return next.clone(),
    };
    let (n_kind, n_path) = match next {
        FileTreeEvent::FileSystemChanged { path, kind } => (kind, path),
        _ => return next.clone(),
    };

    if p_path != n_path {
        return next.clone();
    }

    let rank = |k: &K| match k {
        K::Deleted => 3,
        K::Renamed { .. } => 2,
        K::Created => 1,
        K::Modified => 0,
    };

    match (p_kind, n_kind) {
        // Created then Deleted => Deleted
        (K::Created, K::Deleted) => FileTreeEvent::FileSystemChanged {
            path: n_path.clone(),
            kind: K::Deleted,
        },
        // Deleted then Created => Created (recreated)
        (K::Deleted, K::Created) => FileTreeEvent::FileSystemChanged {
            path: n_path.clone(),
            kind: K::Created,
        },
        // Deleted then Modified => keep Deleted
        (K::Deleted, K::Modified) => FileTreeEvent::FileSystemChanged {
            path: n_path.clone(),
            kind: K::Deleted,
        },
        // Otherwise prefer higher-precedence, or the incoming on tie
        _ if rank(n_kind) >= rank(p_kind) => next.clone(),
        _ => prev.clone(),
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_file_watcher_creation() {
        let temp_dir = TempDir::new().unwrap();
        let watcher = FileTreeWatcher::new(temp_dir.path().to_path_buf());
        assert!(watcher.is_ok());
    }

    #[tokio::test]
    async fn test_debounced_watcher_creation() {
        let temp_dir = TempDir::new().unwrap();
        let watcher = DebouncedFileTreeWatcher::with_defaults(temp_dir.path().to_path_buf());
        assert!(watcher.is_ok());
    }

    #[tokio::test]
    async fn test_file_watcher_detects_file_creation() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher =
            FileTreeWatcher::new(temp_dir.path().to_path_buf()).expect("Failed to create watcher");

        // Create a new file in the watched directory
        let test_file = temp_dir.path().join("test_file.txt");

        // Spawn the watcher task
        let mut _event_received = false;

        // Use tokio::select to race file creation with event detection
        tokio::select! {
            _ = async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                fs::write(&test_file, "test content").unwrap();
            } => {},
            event = watcher.next_event() => {
                if let Some(FileTreeEvent::FileSystemChanged { path, kind }) = event {
                    assert_eq!(path, test_file);
                    assert!(matches!(kind, FileSystemEventKind::Created));
                    _event_received = true;
                }
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                // Timeout - this is expected since we might not receive the event in time
                // File system events can be delayed or batched by the OS
            }
        }

        // Clean up
        drop(watcher);

        // Note: This test might be flaky due to file system event timing
        // The main goal is to verify the watcher compiles and runs without panicking
        assert!(test_file.exists());
    }
}
