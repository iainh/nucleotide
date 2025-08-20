// ABOUTME: File tree implementation using Zed's SumTree for efficient navigation
// ABOUTME: Provides hierarchical file system representation with git integration

pub mod entry;
pub mod icons;
// pub mod project_header;
pub mod summary;
pub mod tree;
pub mod view;
pub mod watcher;

pub use entry::{FileKind, FileTreeEntry};
pub use icons::{get_file_icon, get_symlink_icon};
// pub use project_header::{CompactProjectStatus, ProjectHeader, ProjectHeaderEvent};
pub use summary::FileTreeSummary;
pub use tree::FileTree;
pub use view::FileTreeView;
pub use watcher::DebouncedFileTreeWatcher;

use std::path::PathBuf;

/// Events emitted by the file tree
#[derive(Debug, Clone, PartialEq)]
pub enum FileTreeEvent {
    /// A file or directory was selected
    SelectionChanged { path: Option<PathBuf> },
    /// A file should be opened
    OpenFile { path: PathBuf },
    /// A directory was expanded or collapsed
    DirectoryToggled { path: PathBuf, expanded: bool },
    /// File system change detected
    FileSystemChanged {
        path: PathBuf,
        kind: FileSystemEventKind,
    },
    /// VCS status refresh has started
    VcsRefreshStarted { repository_root: PathBuf },
    /// VCS status has been updated
    VcsStatusChanged {
        repository_root: PathBuf,
        affected_files: Vec<PathBuf>,
    },
    /// VCS refresh failed
    VcsRefreshFailed {
        repository_root: PathBuf,
        error: String,
    },
    /// Request to refresh VCS status
    RefreshVcs { force: bool },
    /// Toggle file tree visibility
    ToggleVisibility,
}

/// Types of file system events
#[derive(Debug, Clone, PartialEq)]
pub enum FileSystemEventKind {
    Created,
    Modified,
    Deleted,
    Renamed { from: PathBuf, to: PathBuf },
}

/// Configuration for file tree behavior
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileTreeConfig {
    /// Show hidden files (starting with .)
    pub show_hidden: bool,
    /// Show ignored files (from .gitignore)
    pub show_ignored: bool,
    /// Maximum depth to scan initially
    pub initial_depth: usize,
    /// Enable file system watching
    pub watch_filesystem: bool,
}

impl Default for FileTreeConfig {
    fn default() -> Self {
        Self {
            show_hidden: false,
            show_ignored: false,
            initial_depth: 3,
            watch_filesystem: true,
        }
    }
}
