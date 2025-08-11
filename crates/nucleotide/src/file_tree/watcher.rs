// ABOUTME: File system watcher using notify crate for real-time file tree updates
// ABOUTME: Monitors file system changes and emits events for tree synchronization

use anyhow::{Context, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::time::Duration;
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

        Ok(Self {
            _watcher: watcher,
            event_receiver: rx,
            root_path,
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
                    if let Some(file_tree_event) = self.convert_event(event) {
                        return Some(file_tree_event);
                    }
                }
                Err(e) => {
                    log::warn!("File system watcher error: {}", e);
                }
            }
        }
        None
    }

    /// Convert a notify Event to a FileTreeEvent
    fn convert_event(&self, event: Event) -> Option<FileTreeEvent> {
        // Filter out events for paths outside our root
        let paths: Vec<_> = event
            .paths
            .into_iter()
            .filter(|path| path.starts_with(&self.root_path))
            .collect();

        if paths.is_empty() {
            return None;
        }

        let kind = match event.kind {
            EventKind::Create(_) => FileSystemEventKind::Created,
            EventKind::Modify(_) => FileSystemEventKind::Modified,
            EventKind::Remove(_) => FileSystemEventKind::Deleted,
            _ => {
                // For other event types, just treat as modified
                FileSystemEventKind::Modified
            }
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
    /// Timer handle for debouncing
    debounce_timer: Option<tokio::time::Interval>,
}

impl DebouncedFileTreeWatcher {
    /// Create a new debounced file system watcher
    pub fn new(root_path: PathBuf, debounce_duration: Duration) -> Result<Self> {
        let watcher = FileTreeWatcher::new(root_path)?;

        Ok(Self {
            watcher,
            debounce_duration,
            pending_events: std::collections::HashMap::new(),
            debounce_timer: None,
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
            // Split the mutable borrows by extracting what we need
            let has_pending = !self.pending_events.is_empty();
            let has_timer = self.debounce_timer.is_some();

            if has_pending && has_timer {
                tokio::select! {
                    // New file system event
                    event = self.watcher.next_event() => {
                        if let Some(event) = event {
                            self.handle_new_event(event);
                            // Start or reset debounce timer
                            self.reset_debounce_timer();
                        }
                    }

                    // Debounce timer expired
                    _ = async {
                        if let Some(ref mut timer) = self.debounce_timer {
                            timer.tick().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {
                        if let Some(event) = self.flush_pending_events() {
                            return Some(event);
                        }
                    }
                }
            } else if has_pending {
                // Only check for debounce timeout if we have pending events
                if let Some(event) = self.flush_pending_events() {
                    return Some(event);
                }
            } else {
                // No pending events, just wait for new file system events
                if let Some(event) = self.watcher.next_event().await {
                    self.handle_new_event(event);
                    self.reset_debounce_timer();
                }
            }
        }
    }

    /// Handle a new file system event
    fn handle_new_event(&mut self, event: FileTreeEvent) {
        if let FileTreeEvent::FileSystemChanged { path, kind } = event {
            // Store the latest event for this path
            self.pending_events.insert(
                path.clone(),
                FileTreeEvent::FileSystemChanged { path, kind },
            );
        }
    }

    /// Reset the debounce timer
    fn reset_debounce_timer(&mut self) {
        self.debounce_timer = Some(tokio::time::interval(self.debounce_duration));
    }

    /// Flush pending events and return the next one
    fn flush_pending_events(&mut self) -> Option<FileTreeEvent> {
        self.debounce_timer = None;

        if let Some((_, event)) = self.pending_events.drain().next() {
            Some(event)
        } else {
            None
        }
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
}
