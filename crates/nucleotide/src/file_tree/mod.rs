// ABOUTME: Path-first file tree implementation for sidebar navigation
// ABOUTME: Provides hierarchical file system representation with git integration

pub mod entry;
pub mod icons;
// pub mod project_header;
pub mod sidebar;
pub mod tree;
pub mod view;
pub mod watcher;

pub use entry::{FileKind, FileTreeEntry};
pub use icons::{get_file_icon, get_symlink_icon};
// pub use project_header::{CompactProjectStatus, ProjectHeader, ProjectHeaderEvent};
pub use tree::{FileTree, FileTreeDirectoryEntry, FileTreeDirectoryEntryKind};
pub use view::FileTreeView;
pub use watcher::DebouncedFileTreeWatcher;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Events emitted by the file tree
#[derive(Debug, Clone, PartialEq)]
pub enum FileTreeEvent {
    /// A file or directory was selected
    SelectionChanged { path: Option<PathBuf> },
    /// The selected file tree path set changed
    SelectionSetChanged { paths: Vec<PathBuf> },
    /// A file should be opened
    OpenFile { path: PathBuf, focus_editor: bool },
    /// A directory was expanded or collapsed
    DirectoryToggled { path: PathBuf, expanded: bool },
    /// Context menu requested on a specific entry at screen position (x, y)
    ContextMenuRequested { path: PathBuf, x: f32, y: f32 },
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
    /// Request that the workspace opens the file tree search prompt
    SearchRequested { initial_query: Option<String> },
}

/// Types of file system events
#[derive(Debug, Clone, PartialEq)]
pub enum FileSystemEventKind {
    Created,
    Modified,
    Deleted,
    Renamed { from: PathBuf, to: PathBuf },
}

/// Search projection strategy for the file tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTreeSearchMode {
    /// Expand ancestors of matching rows and keep matching branches visible.
    ExpandMatches,
    /// Keep the normal tree shape but only expand branches that contain matches.
    CollapseNonMatches,
    /// Hide rows that are neither matches nor ancestors of matches.
    HideNonMatches,
}

/// Collision behaviour for path-first file tree moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTreeCollisionStrategy {
    /// Return an error when the destination already exists.
    Error,
    /// Remove the destination subtree before moving the source subtree.
    Replace,
    /// Leave both subtrees unchanged when the destination already exists.
    Skip,
}

/// Display density preset for project-tree rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileTreeDisplayDensity {
    /// Tighter rows and spacing.
    Compact,
    /// Standard project-tree rows and spacing.
    #[default]
    Default,
    /// Roomier rows and spacing.
    Relaxed,
}

impl FileTreeDisplayDensity {
    pub fn spacing_factor(self) -> f32 {
        match self {
            Self::Compact => 0.8,
            Self::Default => 1.0,
            Self::Relaxed => 1.2,
        }
    }

    pub fn row_height_px(self) -> f32 {
        match self {
            Self::Compact => 24.0,
            Self::Default => 30.0,
            Self::Relaxed => 36.0,
        }
    }
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
    /// Collapse single-child directory chains into one visible row.
    pub flatten_empty_directories: bool,
    /// Search projection strategy.
    pub search_mode: FileTreeSearchMode,
    /// Project-tree display density.
    pub density: FileTreeDisplayDensity,
    /// Render the file tree over a translucent native window backdrop.
    pub translucent_background: bool,
}

impl Default for FileTreeConfig {
    fn default() -> Self {
        Self {
            show_hidden: false,
            show_ignored: false,
            initial_depth: 3,
            watch_filesystem: true,
            flatten_empty_directories: true,
            search_mode: FileTreeSearchMode::ExpandMatches,
            density: FileTreeDisplayDensity::Default,
            translucent_background: false,
        }
    }
}
