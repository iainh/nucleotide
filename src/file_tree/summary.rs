// ABOUTME: Summary types for SumTree aggregation of file tree data
// ABOUTME: Enables efficient queries like "how many files in this subtree"

use sum_tree::{Dimension, Summary};

use std::path::PathBuf;

/// Summary of a file tree subtree
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileTreeSummary {
    /// Total number of entries
    pub count: usize,
    /// Number of visible entries
    pub visible_count: usize,
    /// Number of file entries
    pub file_count: usize,
    /// Number of directory entries
    pub directory_count: usize,
    /// Total size of all files in bytes
    pub total_size: u64,
    /// Maximum depth in this subtree
    pub max_depth: usize,
    /// Maximum path in this subtree (rightmost path for navigation)
    pub max_path: PathBuf,
}

impl Summary for FileTreeSummary {
    type Context = ();

    fn zero(_: &Self::Context) -> Self {
        Self::default()
    }

    fn add_summary(&mut self, other: &Self, _: &Self::Context) {
        self.count += other.count;
        self.visible_count += other.visible_count;
        self.file_count += other.file_count;
        self.directory_count += other.directory_count;
        self.total_size += other.total_size;
        self.max_depth = self.max_depth.max(other.max_depth);
        // Keep the rightmost (last) path for navigation
        if !other.max_path.as_os_str().is_empty() {
            self.max_path = other.max_path.clone();
        }
    }
}

/// Dimension for counting total entries
#[derive(Debug, Clone, Copy, Default)]
pub struct Count(pub usize);

impl<'a> Dimension<'a, FileTreeSummary> for Count {
    fn zero(_: &()) -> Self {
        Count(0)
    }

    fn add_summary(&mut self, summary: &'a FileTreeSummary, _: &()) {
        self.0 += summary.count;
    }
}

/// Dimension for counting visible entries only
#[derive(Debug, Clone, Copy, Default)]
pub struct VisibleCount(pub usize);

impl<'a> Dimension<'a, FileTreeSummary> for VisibleCount {
    fn zero(_: &()) -> Self {
        VisibleCount(0)
    }

    fn add_summary(&mut self, summary: &'a FileTreeSummary, _: &()) {
        self.0 += summary.visible_count;
    }
}

/// Dimension for measuring file sizes
#[derive(Debug, Clone, Copy, Default)]
pub struct TotalSize(pub u64);

impl<'a> Dimension<'a, FileTreeSummary> for TotalSize {
    fn zero(_: &()) -> Self {
        TotalSize(0)
    }

    fn add_summary(&mut self, summary: &'a FileTreeSummary, _: &()) {
        self.0 += summary.total_size;
    }
}

/// Dimension for measuring tree depth
#[derive(Debug, Clone, Copy, Default)]
pub struct MaxDepth(pub usize);

impl<'a> Dimension<'a, FileTreeSummary> for MaxDepth {
    fn zero(_: &()) -> Self {
        MaxDepth(0)
    }

    fn add_summary(&mut self, summary: &'a FileTreeSummary, _: &()) {
        self.0 = self.0.max(summary.max_depth);
    }
}
