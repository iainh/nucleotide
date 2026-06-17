// ABOUTME: File tree entry types representing files and directories
// ABOUTME: Stores path-first metadata used by the sidebar tree projection

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// A path segment that is rendered as part of a flattened directory chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTreeFlattenedSegment {
    pub name: String,
    pub path: PathBuf,
    pub is_terminal: bool,
}

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
    /// VCS status if available
    pub git_status: Option<nucleotide_types::VcsStatus>,
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
    /// Ancestor paths in the current visible projection.
    pub ancestor_paths: Arc<[PathBuf]>,
    /// ARIA-like tree level in the current visible projection.
    pub level: usize,
    /// 1-based position within its current sibling set.
    pub pos_in_set: usize,
    /// Number of siblings in its current sibling set.
    pub set_size: usize,
    /// Flattened directory segments represented by this visible row.
    pub flattened_segments: Option<Arc<[FileTreeFlattenedSegment]>>,
    /// Whether this row directly matches the current tree search query.
    pub is_search_match: bool,
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

// GitStatus moved to nucleotide_types::VcsStatus for centralized VCS handling

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
            ancestor_paths: Arc::from([]),
            level: 1,
            pos_in_set: 1,
            set_size: 1,
            flattened_segments: None,
            is_search_match: false,
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
            ancestor_paths: Arc::from([]),
            level: 1,
            pos_in_set: 1,
            set_size: 1,
            flattened_segments: None,
            is_search_match: false,
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
            ancestor_paths: Arc::from([]),
            level: 1,
            pos_in_set: 1,
            set_size: 1,
            flattened_segments: None,
            is_search_match: false,
        }
    }

    /// Get the file name
    pub fn file_name(&self) -> Option<&str> {
        self.path.file_name().and_then(|name| name.to_str())
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
    #[allow(dead_code)]
    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, FileKind::Symlink { .. })
    }

    /// Get the file extension if it's a file
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn set_expanded(&mut self, expanded: bool) {
        if self.is_directory() {
            self.is_expanded = expanded;
        }
    }

    /// Set VCS status
    #[allow(dead_code)]
    pub fn set_git_status(&mut self, status: nucleotide_types::VcsStatus) {
        self.git_status = Some(status);
    }

    /// Clear git status
    #[allow(dead_code)]
    pub fn clear_git_status(&mut self) {
        self.git_status = None;
    }
}

impl Eq for FileTreeEntry {}
