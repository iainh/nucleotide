// ABOUTME: File tree entry types representing files and directories
// ABOUTME: Implements SumTree traits for efficient tree operations

use std::path::PathBuf;
use std::time::SystemTime;
use sum_tree::{Dimension, Item, KeyedItem};

/// A single entry in the file tree
#[derive(Debug, Clone, PartialEq)]
pub struct FileTreeEntry {
    /// Unique identifier for this entry
    pub id: FileTreeEntryId,
    /// Full path to the file/directory
    pub path: PathBuf,
    /// Type of file system entry
    pub kind: FileKind,
    /// File size in bytes (0 for directories)
    pub size: u64,
    /// Last modified time
    pub mtime: Option<SystemTime>,
    /// Git status if available
    pub git_status: Option<GitStatus>,
    /// Whether this entry should be visible
    pub is_visible: bool,
    /// Whether this directory is expanded (only for directories)
    pub is_expanded: bool,
    /// Depth in the tree (0 = root level)
    pub depth: usize,
    /// Whether this entry is ignored by git
    pub is_ignored: bool,
    /// Whether this is a hidden file (starts with .)
    pub is_hidden: bool,
}

/// Unique identifier for file tree entries
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileTreeEntryId(pub u64);

/// Types of file system entries
#[derive(Debug, Clone, PartialEq)]
pub enum FileKind {
    File {
        extension: Option<String>,
    },
    Directory {
        /// Whether this directory has been loaded
        is_loaded: bool,
        /// Number of child entries
        child_count: usize,
    },
    Symlink {
        /// Target of the symlink
        target: Option<PathBuf>,
        /// Whether the target exists
        target_exists: bool,
    },
}

/// Git status for a file
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum GitStatus {
    /// File is untracked
    Untracked,
    /// File has been modified
    Modified,
    /// File has been added to index
    Added,
    /// File has been deleted
    Deleted,
    /// File has been renamed
    Renamed,
    /// File is up to date
    UpToDate,
    /// File has conflicts
    Conflicted,
}

impl FileTreeEntry {
    /// Create a new file entry
    pub fn new_file(
        id: FileTreeEntryId,
        path: PathBuf,
        size: u64,
        mtime: Option<SystemTime>,
    ) -> Self {
        let extension = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_string());
        let is_hidden = path
            .file_name()
            .map(|name| name.to_string_lossy().starts_with('.'))
            .unwrap_or(false);

        Self {
            id,
            path,
            kind: FileKind::File { extension },
            size,
            mtime,
            git_status: None,
            is_visible: true,
            is_expanded: false,
            depth: 0,
            is_ignored: false,
            is_hidden,
        }
    }

    /// Create a new directory entry
    pub fn new_directory(id: FileTreeEntryId, path: PathBuf, mtime: Option<SystemTime>) -> Self {
        let is_hidden = path
            .file_name()
            .map(|name| name.to_string_lossy().starts_with('.'))
            .unwrap_or(false);

        Self {
            id,
            path,
            kind: FileKind::Directory {
                is_loaded: false,
                child_count: 0,
            },
            size: 0,
            mtime,
            git_status: None,
            is_visible: true,
            is_expanded: false,
            depth: 0,
            is_ignored: false,
            is_hidden,
        }
    }

    /// Create a new symlink entry
    pub fn new_symlink(
        id: FileTreeEntryId,
        path: PathBuf,
        target: Option<PathBuf>,
        target_exists: bool,
        mtime: Option<SystemTime>,
    ) -> Self {
        let is_hidden = path
            .file_name()
            .map(|name| name.to_string_lossy().starts_with('.'))
            .unwrap_or(false);

        Self {
            id,
            path,
            kind: FileKind::Symlink {
                target,
                target_exists,
            },
            size: 0,
            mtime,
            git_status: None,
            is_visible: true,
            is_expanded: false,
            depth: 0,
            is_ignored: false,
            is_hidden,
        }
    }

    /// Get the file name
    pub fn file_name(&self) -> Option<&str> {
        self.path.file_name().map(|name| name.to_str()).flatten()
    }

    /// Check if this is a directory
    pub fn is_directory(&self) -> bool {
        matches!(self.kind, FileKind::Directory { .. })
    }

    /// Check if this is a file
    pub fn is_file(&self) -> bool {
        matches!(self.kind, FileKind::File { .. })
    }

    /// Check if this is a symlink
    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, FileKind::Symlink { .. })
    }

    /// Get the file extension if it's a file
    pub fn extension(&self) -> Option<&str> {
        match &self.kind {
            FileKind::File { extension } => extension.as_deref(),
            _ => None,
        }
    }

    /// Check if this directory is expanded
    pub fn is_expanded(&self) -> bool {
        self.is_directory() && self.is_expanded
    }

    /// Set the expanded state (only valid for directories)
    pub fn set_expanded(&mut self, expanded: bool) {
        if self.is_directory() {
            self.is_expanded = expanded;
        }
    }

    /// Set git status
    pub fn set_git_status(&mut self, status: GitStatus) {
        self.git_status = Some(status);
    }

    /// Clear git status
    pub fn clear_git_status(&mut self) {
        self.git_status = None;
    }
}

impl Item for FileTreeEntry {
    type Summary = crate::file_tree::FileTreeSummary;

    fn summary(&self, _cx: &()) -> Self::Summary {
        crate::file_tree::FileTreeSummary {
            count: 1,
            visible_count: if self.is_visible { 1 } else { 0 },
            file_count: if self.is_file() { 1 } else { 0 },
            directory_count: if self.is_directory() { 1 } else { 0 },
            total_size: self.size,
            max_depth: self.depth,
            max_path: self.path.clone(),
        }
    }
}

/// Key for path-based lookups in the file tree
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileTreePathKey(pub PathBuf);

impl KeyedItem for FileTreeEntry {
    type Key = FileTreePathKey;

    fn key(&self) -> Self::Key {
        FileTreePathKey(self.path.clone())
    }
}

/// Index-based dimension for navigation by position
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct IndexDimension(pub usize);

/// Path-based dimension for lookups by path
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct PathDimension(pub PathBuf);

/// Depth-based dimension for tree operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct DepthDimension(pub usize);

/// Size-based dimension for sorting/filtering by file size
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct SizeDimension(pub u64);

impl<'a> Dimension<'a, crate::file_tree::FileTreeSummary> for IndexDimension {
    fn zero(_cx: &()) -> Self {
        IndexDimension(0)
    }

    fn add_summary(&mut self, summary: &'a crate::file_tree::FileTreeSummary, _cx: &()) {
        self.0 += summary.visible_count;
    }
}

impl<'a> Dimension<'a, crate::file_tree::FileTreeSummary> for PathDimension {
    fn zero(_cx: &()) -> Self {
        PathDimension(PathBuf::new())
    }

    fn add_summary(&mut self, summary: &'a crate::file_tree::FileTreeSummary, _cx: &()) {
        self.0 = summary.max_path.clone();
    }
}

impl<'a> Dimension<'a, crate::file_tree::FileTreeSummary> for DepthDimension {
    fn zero(_cx: &()) -> Self {
        DepthDimension(0)
    }

    fn add_summary(&mut self, summary: &'a crate::file_tree::FileTreeSummary, _cx: &()) {
        self.0 = self.0.max(summary.max_depth);
    }
}

impl<'a> Dimension<'a, crate::file_tree::FileTreeSummary> for SizeDimension {
    fn zero(_cx: &()) -> Self {
        SizeDimension(0)
    }

    fn add_summary(&mut self, summary: &'a crate::file_tree::FileTreeSummary, _cx: &()) {
        self.0 += summary.total_size;
    }
}

impl<'a> Dimension<'a, crate::file_tree::FileTreeSummary> for FileTreePathKey {
    fn zero(_cx: &()) -> Self {
        FileTreePathKey(PathBuf::new())
    }

    fn add_summary(&mut self, _summary: &'a crate::file_tree::FileTreeSummary, _cx: &()) {
        // For path key, we don't update based on summary
        // The key represents a specific path, not accumulated paths
    }
}

impl PartialOrd for FileTreeEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileTreeEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Simple alphabetical path comparison
        self.path.cmp(&other.path)
    }
}

impl Eq for FileTreeEntry {}
