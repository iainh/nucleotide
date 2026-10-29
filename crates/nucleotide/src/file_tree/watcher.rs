// ABOUTME: File system watcher using notify crate for real-time file tree updates
// ABOUTME: Monitors file system changes and emits events for tree synchronization

use anyhow::{Context, Result};
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use nucleotide_logging::{debug, warn};
use std::collections::VecDeque;
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
    /// Converted paths still to deliver from a batched notify event.
    pending_events: VecDeque<FileTreeEvent>,
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
            pending_events: VecDeque::new(),
        })
    }

    /// Get the root path being watched
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Process file system events and convert them to FileTreeEvents
    pub async fn next_event(&mut self) -> Option<FileTreeEvent> {
        loop {
            if let Some(event) = self.pending_events.pop_front() {
                debug!(evt = ?event, "converted file tree event");
                return Some(event);
            }

            match self.event_receiver.recv().await? {
                Ok(event) => {
                    // Log raw notify event details
                    debug!(
                        kind = ?event.kind,
                        paths = ?event.paths,
                        "notify event received"
                    );

                    self.pending_events = self.convert_event(event).into();
                }
                Err(e) => {
                    warn!(error = %e, "File system watcher error");
                }
            }
        }
    }

    // Keep gitignored paths: the tree can display them and owns ignore-rule filtering.
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

    /// Convert every relevant path, preserving rename endpoints before filtering.
    fn convert_event(&self, event: Event) -> Vec<FileTreeEvent> {
        let visible =
            |path: &Path| path.starts_with(&self.root_path) && !self.should_hide_path(path);

        // Only Both guarantees an ordered (from, to) pair; Name(Any) may be batched.
        if matches!(
            event.kind,
            EventKind::Modify(ModifyKind::Name(RenameMode::Both))
        ) && let [from, to] = event.paths.as_slice()
        {
            let (path, kind) = match (visible(from), visible(to)) {
                (true, true) => (
                    to.clone(),
                    FileSystemEventKind::Renamed {
                        from: from.clone(),
                        to: to.clone(),
                    },
                ),
                (true, false) => (from.clone(), FileSystemEventKind::Deleted),
                (false, true) => (to.clone(), FileSystemEventKind::Created),
                (false, false) => return Vec::new(),
            };
            return vec![FileTreeEvent::FileSystemChanged { path, kind }];
        }

        let kind = match event.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                FileSystemEventKind::Created
            }
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                FileSystemEventKind::Deleted
            }
            _ => FileSystemEventKind::Modified,
        };

        event
            .paths
            .into_iter()
            .filter(|path| visible(path))
            .map(|path| FileTreeEvent::FileSystemChanged {
                path,
                kind: kind.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind};
    use std::fs;
    use tempfile::TempDir;

    // Inject notify batches so tests do not depend on a platform's event grouping.
    async fn deliver(watcher: &mut FileTreeWatcher, event: Event) -> Vec<FileTreeEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        watcher.event_receiver = rx;
        tx.send(Ok(event)).unwrap();
        drop(tx);
        let mut events = Vec::new();
        while let Some(event) = watcher.next_event().await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn test_file_watcher_keeps_gitignored_file_events() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join(".gitignore"), "ignored.log\n").unwrap();

        let mut watcher =
            FileTreeWatcher::new(temp_dir.path().to_path_buf()).expect("Failed to create watcher");
        let ignored_file = temp_dir.path().join("ignored.log");
        let event = Event::new(EventKind::Create(CreateKind::File)).add_path(ignored_file.clone());

        assert_eq!(
            deliver(&mut watcher, event).await,
            vec![FileTreeEvent::FileSystemChanged {
                path: ignored_file.clone(),
                kind: FileSystemEventKind::Created,
            }]
        );
    }

    #[tokio::test]
    async fn test_file_watcher_keeps_unignored_file_events() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join(".gitignore"), "ignored.log\n").unwrap();

        let mut watcher =
            FileTreeWatcher::new(temp_dir.path().to_path_buf()).expect("Failed to create watcher");
        let visible_file = temp_dir.path().join("visible.log");
        let event = Event::new(EventKind::Create(CreateKind::File)).add_path(visible_file.clone());

        let converted = deliver(&mut watcher, event).await;

        assert_eq!(
            converted,
            vec![FileTreeEvent::FileSystemChanged {
                path: visible_file,
                kind: FileSystemEventKind::Created,
            }]
        );
    }

    #[tokio::test]
    async fn test_file_watcher_keeps_gitignored_directory_delete_events() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join(".gitignore"), "target/\n").unwrap();

        let mut watcher =
            FileTreeWatcher::new(temp_dir.path().to_path_buf()).expect("Failed to create watcher");
        let ignored_dir = temp_dir.path().join("target");
        let event = Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(ignored_dir.clone());

        assert_eq!(
            deliver(&mut watcher, event).await,
            vec![FileTreeEvent::FileSystemChanged {
                path: ignored_dir.clone(),
                kind: FileSystemEventKind::Deleted,
            }]
        );
    }

    #[tokio::test]
    async fn test_file_watcher_keeps_events_after_ignore_file_changes() {
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
        assert_eq!(deliver(&mut watcher, gitignore_event).await.len(), 1);

        let ignored_file = temp_dir.path().join("ignored_after_reload.txt");
        let ignored_event =
            Event::new(EventKind::Create(CreateKind::File)).add_path(ignored_file.clone());

        assert_eq!(
            deliver(&mut watcher, ignored_event).await,
            vec![FileTreeEvent::FileSystemChanged {
                path: ignored_file,
                kind: FileSystemEventKind::Created,
            }]
        );
    }

    #[tokio::test]
    async fn batched_events_deliver_every_visible_path_without_inventing_renames() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let mut watcher = FileTreeWatcher::new(root.to_path_buf()).unwrap();
        let paths = [
            root.join("first.rs"),
            root.join("second.txt"),
            root.join("third"),
        ];

        for (event_kind, expected_kind) in [
            (
                EventKind::Create(CreateKind::Any),
                FileSystemEventKind::Created,
            ),
            (
                EventKind::Remove(RemoveKind::Any),
                FileSystemEventKind::Deleted,
            ),
            (
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                FileSystemEventKind::Modified,
            ),
            (
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)),
                FileSystemEventKind::Modified,
            ),
            (
                EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
                FileSystemEventKind::Modified,
            ),
        ] {
            let paths = if matches!(
                event_kind,
                EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_))
            ) {
                &paths[..2]
            } else {
                &paths[..]
            };
            let mut event = Event::new(event_kind)
                .add_path(root.join(".hidden"))
                .add_path(root.join(".git/index"))
                .add_path(root.parent().unwrap().join("outside"));
            for path in paths {
                event = event.add_path(path.clone());
            }
            assert_eq!(
                deliver(&mut watcher, event).await,
                paths
                    .iter()
                    .map(|path| FileTreeEvent::FileSystemChanged {
                        path: path.clone(),
                        kind: expected_kind.clone(),
                    })
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            deliver(&mut watcher, Event::new(EventKind::Any))
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rename_events_preserve_endpoints_and_handle_visibility_boundaries() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        let mut watcher = FileTreeWatcher::new(root.to_path_buf()).unwrap();
        let from = root.join("old.rs");
        let to = root.join("new.txt");
        let mut cases = vec![(
            from.clone(),
            to.clone(),
            Some((
                to.clone(),
                FileSystemEventKind::Renamed {
                    from: from.clone(),
                    to: to.clone(),
                },
            )),
        )];
        for excluded in [
            root.join(".hidden"),
            root.join(".git/index"),
            root.parent().unwrap().join("outside"),
        ] {
            cases.extend([
                (
                    from.clone(),
                    excluded.clone(),
                    Some((from.clone(), FileSystemEventKind::Deleted)),
                ),
                (
                    excluded.clone(),
                    to.clone(),
                    Some((to.clone(), FileSystemEventKind::Created)),
                ),
                (excluded.clone(), excluded, None),
            ]);
        }
        for (from, to, expected) in cases {
            let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                .add_path(from)
                .add_path(to);
            assert_eq!(
                deliver(&mut watcher, event).await,
                expected
                    .into_iter()
                    .map(|(path, kind)| FileTreeEvent::FileSystemChanged { path, kind })
                    .collect::<Vec<_>>()
            );
        }
        for (mode, kind) in [
            (RenameMode::From, FileSystemEventKind::Deleted),
            (RenameMode::To, FileSystemEventKind::Created),
        ] {
            let event =
                Event::new(EventKind::Modify(ModifyKind::Name(mode))).add_path(from.clone());
            assert_eq!(
                deliver(&mut watcher, event).await,
                vec![FileTreeEvent::FileSystemChanged {
                    path: from.clone(),
                    kind
                }]
            );
        }
    }
}
