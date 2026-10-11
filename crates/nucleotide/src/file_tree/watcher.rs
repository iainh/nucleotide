// ABOUTME: File system watcher using notify crate for real-time file tree updates
// ABOUTME: Monitors file system changes and emits events for tree synchronization

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{CreateKind, RemoveKind};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use nucleotide_logging::{debug, warn};
use std::path::{Path, PathBuf};
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
    fn rebuild_gitignore_matcher(&mut self) {
        self.gitignore = Self::build_gitignore_matcher(&self.root_path);
    }

    fn is_ignore_config_path(&self, path: &Path) -> bool {
        if path.file_name().and_then(|name| name.to_str()) == Some(".gitignore")
            || path.file_name().and_then(|name| name.to_str()) == Some(".ignore")
        {
            return true;
        }

        path.strip_prefix(&self.root_path)
            .is_ok_and(|relative_path| {
                relative_path == Path::new(".git/info/exclude")
                    || relative_path == Path::new(".helix/ignore")
            })
    }

    fn is_dir_event(path: &Path, event_kind: &EventKind) -> bool {
        path.is_dir()
            || matches!(
                event_kind,
                EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder)
            )
    }

    /// Check if a path should be ignored
    fn should_ignore_path(&self, path: &Path, is_dir: bool) -> bool {
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
            let matched = gitignore.matched_path_or_any_parents(relative_path, is_dir);
            return matched.is_ignore();
        }

        false
    }

    fn should_hide_path(&self, path: &Path) -> bool {
        for component in path.components() {
            if let std::path::Component::Normal(name) = component
                && matches!(name.to_str(), Some(".git" | ".svn" | ".hg" | ".bzr"))
            {
                return true;
            }
        }

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        file_name.starts_with('.') && !matches!(file_name, ".gitignore" | ".ignore")
    }

    /// Convert a notify Event to a FileTreeEvent
    fn convert_event(&mut self, event: Event) -> Option<FileTreeEvent> {
        let event_kind = event.kind;

        // Filter out events for paths outside our root
        let paths: Vec<_> = event
            .paths
            .iter()
            .filter(|path| path.starts_with(&self.root_path))
            .cloned()
            .collect();

        if paths.iter().any(|path| self.is_ignore_config_path(path)) {
            self.rebuild_gitignore_matcher();
        }

        // Log pre-filter existence and ignore decisions
        for p in &paths {
            let exists = p.exists();
            let is_dir = Self::is_dir_event(p, &event_kind);
            let ignored = self.should_ignore_path(p, is_dir);
            debug!(
                path = %p.display(),
                exists = exists,
                is_dir = is_dir,
                ignored = ignored,
                "event path status"
            );
        }

        let paths: Vec<_> = paths
            .into_iter()
            .filter(|path| !self.should_hide_path(path))
            .collect();

        if paths.is_empty() {
            return None;
        }

        // Handle rename specially when we have at least two paths
        // Notify represents rename as a Modify(Name(...)) event kind.
        // We map that to a Renamed event when possible.
        if matches!(event_kind, EventKind::Modify(_)) && paths.len() >= 2 {
            debug!(from = %paths[0].display(), to = %paths[1].display(), "mapping rename event");
            return Some(FileTreeEvent::FileSystemChanged {
                path: paths[1].clone(),
                kind: FileSystemEventKind::Renamed {
                    from: paths[0].clone(),
                    to: paths[1].clone(),
                },
            });
        }

        let kind = match event_kind {
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

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_file_watcher_keeps_gitignored_file_events() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join(".gitignore"), "ignored.log\n").unwrap();

        let mut watcher =
            FileTreeWatcher::new(temp_dir.path().to_path_buf()).expect("Failed to create watcher");
        let ignored_file = temp_dir.path().join("ignored.log");
        let event = Event::new(EventKind::Create(CreateKind::File)).add_path(ignored_file.clone());

        assert_eq!(
            watcher.convert_event(event),
            Some(FileTreeEvent::FileSystemChanged {
                path: ignored_file.clone(),
                kind: FileSystemEventKind::Created,
            })
        );
        assert!(watcher.should_ignore_path(&ignored_file, false));
    }

    #[test]
    fn test_file_watcher_keeps_unignored_file_events() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join(".gitignore"), "ignored.log\n").unwrap();

        let mut watcher =
            FileTreeWatcher::new(temp_dir.path().to_path_buf()).expect("Failed to create watcher");
        let visible_file = temp_dir.path().join("visible.log");
        let event = Event::new(EventKind::Create(CreateKind::File)).add_path(visible_file.clone());

        let converted = watcher.convert_event(event);

        assert_eq!(
            converted,
            Some(FileTreeEvent::FileSystemChanged {
                path: visible_file,
                kind: FileSystemEventKind::Created,
            })
        );
    }

    #[test]
    fn test_file_watcher_keeps_gitignored_directory_delete_events() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join(".gitignore"), "target/\n").unwrap();

        let mut watcher =
            FileTreeWatcher::new(temp_dir.path().to_path_buf()).expect("Failed to create watcher");
        let ignored_dir = temp_dir.path().join("target");
        let event = Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(ignored_dir.clone());

        assert_eq!(
            watcher.convert_event(event),
            Some(FileTreeEvent::FileSystemChanged {
                path: ignored_dir.clone(),
                kind: FileSystemEventKind::Deleted,
            })
        );
        assert!(watcher.should_ignore_path(&ignored_dir, true));
    }

    #[test]
    fn test_file_watcher_reloads_gitignore_after_ignore_file_event() {
        let temp_dir = TempDir::new().unwrap();
        let mut watcher =
            FileTreeWatcher::new(temp_dir.path().to_path_buf()).expect("Failed to create watcher");

        fs::write(
            temp_dir.path().join(".gitignore"),
            "ignored_after_reload.txt\n",
        )
        .unwrap();

        let gitignore_event = Event::new(EventKind::Create(CreateKind::File))
            .add_path(temp_dir.path().join(".gitignore"));
        let _ = watcher.convert_event(gitignore_event);

        let ignored_file = temp_dir.path().join("ignored_after_reload.txt");
        let ignored_event =
            Event::new(EventKind::Create(CreateKind::File)).add_path(ignored_file.clone());

        assert_eq!(
            watcher.convert_event(ignored_event),
            Some(FileTreeEvent::FileSystemChanged {
                path: ignored_file,
                kind: FileSystemEventKind::Created,
            })
        );
    }
}
