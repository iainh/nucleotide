// ABOUTME: Version control system types for diff information and file status
// ABOUTME: Core VCS types shared across the application without dependencies

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum VcsStatus {
    /// File is not tracked by VCS
    Untracked,
    /// File is tracked and not modified
    Clean,
    /// File is tracked and has been modified
    Modified,
    /// File has been added to VCS
    Added,
    /// File has been deleted from VCS
    Deleted,
    /// File has been renamed in VCS
    Renamed,
    /// File has conflicts from merge
    Conflicted,
    /// Unknown VCS status
    Unknown,
}

impl Default for VcsStatus {
    fn default() -> Self {
        Self::Clean
    }
}

impl VcsStatus {
    /// Returns true if the file has been modified in some way
    pub fn is_modified(&self) -> bool {
        matches!(
            self,
            Self::Modified | Self::Added | Self::Deleted | Self::Renamed | Self::Conflicted
        )
    }

    /// Returns true if the file is tracked by VCS
    pub fn is_tracked(&self) -> bool {
        !matches!(self, Self::Untracked)
    }

    /// Returns true if the status indicates the file needs attention
    pub fn needs_attention(&self) -> bool {
        matches!(self, Self::Conflicted | Self::Unknown)
    }

    /// Returns a single-character symbol representing this status
    pub fn symbol(&self) -> char {
        match self {
            Self::Untracked => '?',
            Self::Clean => ' ',
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Conflicted => 'C',
            Self::Unknown => '!',
        }
    }

    /// Returns a human-readable description of this status
    pub fn description(&self) -> &'static str {
        match self {
            Self::Untracked => "Untracked",
            Self::Clean => "Clean",
            Self::Modified => "Modified", 
            Self::Added => "Added",
            Self::Deleted => "Deleted",
            Self::Renamed => "Renamed",
            Self::Conflicted => "Conflicted",
            Self::Unknown => "Unknown",
        }
    }
}

/// Information about a diff hunk for gutter rendering
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiffHunkInfo {
    /// Start line in the new version (0-indexed)
    pub after_start: u32,
    /// End line in the new version (0-indexed)
    pub after_end: u32,
    /// Start line in the base version (0-indexed)
    pub before_start: u32,
    /// End line in the base version (0-indexed)
    pub before_end: u32,
    /// Type of change this hunk represents
    pub change_type: DiffChangeType,
}

impl DiffHunkInfo {
    /// Create a new diff hunk info
    pub fn new(
        after_start: u32,
        after_end: u32,
        before_start: u32,
        before_end: u32,
        change_type: DiffChangeType,
    ) -> Self {
        Self {
            after_start,
            after_end,
            before_start,
            before_end,
            change_type,
        }
    }

    /// Returns the number of lines affected in the new version
    pub fn lines_affected(&self) -> u32 {
        match self.change_type {
            DiffChangeType::Deletion => 0, // No lines in new version
            _ => self.after_end - self.after_start,
        }
    }

    /// Returns the number of lines affected in the old version
    pub fn lines_removed(&self) -> u32 {
        match self.change_type {
            DiffChangeType::Addition => 0, // No lines removed
            _ => self.before_end - self.before_start,
        }
    }

    /// Returns true if this hunk represents a pure insertion (no lines removed)
    pub fn is_pure_insertion(&self) -> bool {
        self.change_type == DiffChangeType::Addition && self.before_start == self.before_end
    }

    /// Returns true if this hunk represents a pure removal (no lines added)
    pub fn is_pure_removal(&self) -> bool {
        self.change_type == DiffChangeType::Deletion && self.after_start == self.after_end
    }

    /// Returns true if this line number is within this hunk's range
    pub fn contains_line(&self, line: u32) -> bool {
        match self.change_type {
            DiffChangeType::Addition | DiffChangeType::Modification => {
                line >= self.after_start && line < self.after_end
            }
            DiffChangeType::Deletion => {
                // Show deletion indicator on the line where content was removed
                line == self.after_start
            }
        }
    }
}

/// Type of change represented by a diff hunk
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DiffChangeType {
    /// Lines were added
    Addition,
    /// Lines were removed
    Deletion,
    /// Lines were modified
    Modification,
}

impl DiffChangeType {
    /// Returns a symbol representing this change type (for gutter display)
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Addition | Self::Modification => "▍", // Vertical bar for additions and modifications
            Self::Deletion => "▔", // Horizontal bar for deletions
        }
    }

    /// Returns a human-readable description of this change type
    pub fn description(&self) -> &'static str {
        match self {
            Self::Addition => "Addition",
            Self::Deletion => "Deletion", 
            Self::Modification => "Modification",
        }
    }

    /// Returns a single character code for this change type
    pub fn code(&self) -> char {
        match self {
            Self::Addition => '+',
            Self::Deletion => '-',
            Self::Modification => '~',
        }
    }
}

impl Default for DiffChangeType {
    fn default() -> Self {
        Self::Modification
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vcs_status_properties() {
        assert!(VcsStatus::Modified.is_modified());
        assert!(VcsStatus::Added.is_modified());
        assert!(!VcsStatus::Clean.is_modified());
        
        assert!(VcsStatus::Clean.is_tracked());
        assert!(!VcsStatus::Untracked.is_tracked());
        
        assert!(VcsStatus::Conflicted.needs_attention());
        assert!(!VcsStatus::Clean.needs_attention());
    }

    #[test]
    fn test_vcs_status_symbols() {
        assert_eq!(VcsStatus::Modified.symbol(), 'M');
        assert_eq!(VcsStatus::Added.symbol(), 'A');
        assert_eq!(VcsStatus::Deleted.symbol(), 'D');
        assert_eq!(VcsStatus::Untracked.symbol(), '?');
        assert_eq!(VcsStatus::Clean.symbol(), ' ');
    }

    #[test]
    fn test_diff_hunk_info_creation() {
        let hunk = DiffHunkInfo::new(5, 8, 5, 5, DiffChangeType::Addition);
        
        assert_eq!(hunk.after_start, 5);
        assert_eq!(hunk.after_end, 8);
        assert_eq!(hunk.change_type, DiffChangeType::Addition);
        assert_eq!(hunk.lines_affected(), 3);
        assert_eq!(hunk.lines_removed(), 0);
        assert!(hunk.is_pure_insertion());
        assert!(!hunk.is_pure_removal());
    }

    #[test]
    fn test_diff_hunk_line_detection() {
        let addition_hunk = DiffHunkInfo::new(5, 8, 5, 5, DiffChangeType::Addition);
        let deletion_hunk = DiffHunkInfo::new(10, 10, 8, 11, DiffChangeType::Deletion);
        let modification_hunk = DiffHunkInfo::new(15, 18, 12, 15, DiffChangeType::Modification);

        // Test addition
        assert!(addition_hunk.contains_line(6));
        assert!(!addition_hunk.contains_line(4));
        assert!(!addition_hunk.contains_line(8));

        // Test deletion
        assert!(deletion_hunk.contains_line(10));
        assert!(!deletion_hunk.contains_line(9));
        assert!(!deletion_hunk.contains_line(11));

        // Test modification
        assert!(modification_hunk.contains_line(16));
        assert!(!modification_hunk.contains_line(14));
        assert!(!modification_hunk.contains_line(18));
    }

    #[test]
    fn test_diff_change_type_symbols() {
        assert_eq!(DiffChangeType::Addition.symbol(), "▍");
        assert_eq!(DiffChangeType::Modification.symbol(), "▍");
        assert_eq!(DiffChangeType::Deletion.symbol(), "▔");
    }

    #[test]
    fn test_diff_change_type_codes() {
        assert_eq!(DiffChangeType::Addition.code(), '+');
        assert_eq!(DiffChangeType::Modification.code(), '~');
        assert_eq!(DiffChangeType::Deletion.code(), '-');
    }
}