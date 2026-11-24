// ABOUTME: File system watcher using notify crate for real-time file tree updates
// ABOUTME: Monitors file system changes and emits events for tree synchronization

use anyhow::{Context, Result};
use notify::event::{AccessKind, AccessMode, ModifyKind, RenameMode};
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
    /// `root_path` with symlinks resolved, when that differs. The OS reports
    /// event paths under it, such as `/private/tmp` for macOS `/tmp`.
    resolved_root: Option<PathBuf>,
    /// Converted paths still to deliver from a batched notify event.
    pending_events: VecDeque<FileTreeEvent>,
    /// Where git keeps this worktree's HEAD and index, and its refs.
    git_dirs: Option<GitDirs>,
}

struct GitDirs {
    /// Holds HEAD and the index.
    git_dir: PathBuf,
    /// Holds refs and packed-refs. A linked worktree shares the main
    /// repository's; otherwise it is `git_dir`.
    common_dir: PathBuf,
}

impl GitDirs {
    /// Finds the git dirs for `root`, which may be a subdirectory of the
    /// worktree or a linked worktree.
    fn find(root: &Path) -> Option<Self> {
        let dot_git = root
            .ancestors()
            .map(|dir| dir.join(".git"))
            .find(|path| path.exists())?;
        // A linked worktree's `.git` is a file: `gitdir: <path>`.
        let git_dir = if dot_git.is_dir() {
            dot_git
        } else {
            let contents = std::fs::read_to_string(&dot_git).ok()?;
            let target = contents.trim().strip_prefix("gitdir:")?.trim();
            dot_git.parent()?.join(target)
        };
        let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
            .map(|dir| git_dir.join(dir.trim()))
            .unwrap_or_else(|_| git_dir.clone());
        // The root watch reports paths under the root as opened; watches added
        // for directories outside it report canonical paths.
        let normalize = |dir: PathBuf| {
            if dir.starts_with(root) {
                dir
            } else {
                std::fs::canonicalize(&dir).unwrap_or(dir)
            }
        };
        Some(Self {
            git_dir: normalize(git_dir),
            common_dir: normalize(common_dir),
        })
    }

    /// Whether `path` is HEAD, the index or a ref. Git writes each through a
    /// `.lock` file renamed into place, so the rename reports the final path.
    fn contains_metadata(&self, path: &Path) -> bool {
        if path
            .extension()
            .is_some_and(|extension| extension == "lock")
        {
            return false;
        }
        let in_git_dir = path
            .strip_prefix(&self.git_dir)
            .is_ok_and(|relative| relative == Path::new("HEAD") || relative == Path::new("index"));
        in_git_dir || self.contains_ref(path)
    }

    /// Whether `path` is a branch, tag or remote ref, loose or packed.
    fn contains_ref(&self, path: &Path) -> bool {
        path.extension().is_none_or(|extension| extension != "lock")
            && path.strip_prefix(&self.common_dir).is_ok_and(|relative| {
                relative.starts_with("refs") || relative == Path::new("packed-refs")
            })
    }

    /// Watches the parts of each git dir outside `root`, such as a linked
    /// worktree's or the repository's when `root` is a subdirectory, without
    /// the objects under them. The root watch covers the rest.
    fn watch_outside(&self, root: &Path, watcher: &mut notify::RecommendedWatcher) {
        let watches = [
            (self.git_dir.clone(), RecursiveMode::NonRecursive),
            (self.common_dir.clone(), RecursiveMode::NonRecursive),
            (self.common_dir.join("refs"), RecursiveMode::Recursive),
        ];
        for (path, mode) in watches {
            if !path.starts_with(root)
                && path.exists()
                && let Err(error) = watcher.watch(&path, mode)
            {
                warn!(error = %error, path = ?path, "Failed to watch git metadata");
            }
        }
    }
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

        let resolved_root = std::fs::canonicalize(&root_path)
            .ok()
            .filter(|resolved| *resolved != root_path);

        let git_dirs = GitDirs::find(&root_path);
        if let Some(git_dirs) = &git_dirs {
            git_dirs.watch_outside(&root_path, &mut watcher);
        }

        Ok(Self {
            _watcher: watcher,
            event_receiver: rx,
            root_path,
            resolved_root,
            pending_events: VecDeque::new(),
            git_dirs,
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
    fn convert_event(&self, mut event: Event) -> Vec<FileTreeEvent> {
        // Report paths under the root the tree was opened with, so they match its entries.
        if let Some(resolved_root) = &self.resolved_root {
            for path in &mut event.paths {
                if let Ok(relative) = path.strip_prefix(resolved_root) {
                    *path = self.root_path.join(relative);
                }
            }
        }

        // inotify reports opens and reads, including `git status` reading the
        // worktree after each refresh; treating them as changes refreshes forever.
        if matches!(event.kind, EventKind::Access(kind) if kind != AccessKind::Close(AccessMode::Write))
        {
            return Vec::new();
        }

        let git_metadata_change = self
            .git_dirs
            .as_ref()
            .filter(|_| !matches!(event.kind, EventKind::Modify(ModifyKind::Metadata(_))))
            .filter(|dirs| event.paths.iter().any(|path| dirs.contains_metadata(path)))
            .map(|dirs| FileTreeEvent::GitMetadataChanged {
                refs: event.paths.iter().any(|path| dirs.contains_ref(path)),
            });
        let mut events = self.convert_visible_paths(event);
        events.extend(git_metadata_change);
        events
    }

    fn convert_visible_paths(&self, event: Event) -> Vec<FileTreeEvent> {
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
        // Reading a file or listing a directory changes nothing.
        for access in [
            AccessKind::Open(AccessMode::Any),
            AccessKind::Read,
            AccessKind::Close(AccessMode::Read),
        ] {
            let event = Event::new(EventKind::Access(access))
                .add_path(paths[0].clone())
                .add_path(root.to_path_buf());
            assert!(deliver(&mut watcher, event).await.is_empty(), "{access:?}");
        }
    }

    #[tokio::test]
    async fn git_metadata_writes_report_a_change_in_main_and_linked_worktrees() {
        let temp_dir = TempDir::new().unwrap();
        let main = temp_dir.path().canonicalize().unwrap().join("main");
        let git_dir = main.join(".git");
        let linked_git_dir = git_dir.join("worktrees/linked");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::create_dir_all(&linked_git_dir).unwrap();
        fs::write(linked_git_dir.join("commondir"), "../..\n").unwrap();
        let linked = temp_dir.path().join("linked");
        fs::create_dir(&linked).unwrap();
        fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", linked_git_dir.display()),
        )
        .unwrap();
        let write = EventKind::Modify(ModifyKind::Data(DataChange::Content));
        let rename = EventKind::Modify(ModifyKind::Name(RenameMode::Both));

        let mut watcher = FileTreeWatcher::new(main).unwrap();
        // `expected` is whether a change is reported, and whether refs moved.
        for (kind, paths, expected) in [
            (write, vec![git_dir.join("index")], Some(false)),
            (write, vec![git_dir.join("HEAD")], Some(false)),
            (write, vec![git_dir.join("packed-refs")], Some(true)),
            (
                EventKind::Create(CreateKind::File),
                vec![git_dir.join("refs/remotes/origin/main")],
                Some(true),
            ),
            (
                rename,
                vec![git_dir.join("index.lock"), git_dir.join("index")],
                Some(false),
            ),
            (write, vec![git_dir.join("index.lock")], None),
            (write, vec![git_dir.join("objects/ab/cdef")], None),
            (write, vec![git_dir.join("FETCH_HEAD")], None),
            (write, vec![git_dir.join("logs/HEAD")], None),
            // Our own `git status` opens and reads the index.
            (
                EventKind::Access(AccessKind::Open(AccessMode::Any)),
                vec![git_dir.join("index")],
                None,
            ),
        ] {
            let event = paths
                .iter()
                .fold(Event::new(kind), |event, path| event.add_path(path.clone()));
            assert_eq!(
                deliver(&mut watcher, event).await,
                expected
                    .map(|refs| FileTreeEvent::GitMetadataChanged { refs })
                    .into_iter()
                    .collect::<Vec<_>>(),
                "{paths:?}"
            );
        }

        // The linked worktree's HEAD and index live in its own git dir, and its
        // branches in the main repository's.
        let mut watcher = FileTreeWatcher::new(linked).unwrap();
        for (path, expected) in [
            (linked_git_dir.join("HEAD"), Some(false)),
            (linked_git_dir.join("index"), Some(false)),
            (git_dir.join("refs/heads/feature"), Some(true)),
            (git_dir.join("index"), None),
            (git_dir.join("worktrees/other/HEAD"), None),
        ] {
            assert_eq!(
                deliver(&mut watcher, Event::new(write).add_path(path.clone())).await,
                expected
                    .map(|refs| FileTreeEvent::GitMetadataChanged { refs })
                    .into_iter()
                    .collect::<Vec<_>>(),
                "{path:?}"
            );
        }
    }

    // macOS reports `/tmp/repo` events as `/private/tmp/repo`.
    #[cfg(unix)]
    #[tokio::test]
    async fn events_under_a_symlinked_root_use_the_root_as_opened() {
        let temp_dir = TempDir::new().unwrap();
        let real_root = temp_dir.path().canonicalize().unwrap().join("real");
        fs::create_dir(&real_root).unwrap();
        let link_root = temp_dir.path().join("link");
        std::os::unix::fs::symlink(&real_root, &link_root).unwrap();
        let mut watcher = FileTreeWatcher::new(link_root.clone()).unwrap();

        let event = Event::new(EventKind::Create(CreateKind::File))
            .add_path(real_root.join("new.txt"))
            .add_path(real_root.join(".git/index"));
        assert_eq!(
            deliver(&mut watcher, event).await,
            vec![FileTreeEvent::FileSystemChanged {
                path: link_root.join("new.txt"),
                kind: FileSystemEventKind::Created,
            }]
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
