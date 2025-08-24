// ABOUTME: VCS domain events for git status, diff detection, and repository changes
// ABOUTME: Immutable fact-based events following Domain-Driven Design principles

use helix_view::DocumentId;
use std::path::PathBuf;

/// VCS domain events - covers version control status, diff detection, and repository changes
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// File diff status changed (added/modified/removed lines)
    DiffStatusChanged {
        doc_id: DocumentId,
        path: PathBuf,
        hunks: Vec<DiffHunk>,
        diff_base_revision: Option<String>,
    },

    /// Repository head changed (branch switch, commit, etc.)
    RepositoryHeadChanged {
        repository_path: PathBuf,
        previous_head: Option<String>,
        current_head: String,
    },

    /// File stage status changed in git
    FileStageStatusChanged {
        path: PathBuf,
        stage_status: StageStatus,
        working_status: Option<WorkingStatus>,
    },

    /// Diff provider became available or unavailable
    DiffProviderStatusChanged {
        repository_path: PathBuf,
        provider_type: VcsProviderType,
        is_available: bool,
    },

    /// File was added to or removed from VCS tracking
    FileTrackingChanged {
        path: PathBuf,
        is_tracked: bool,
        file_change_type: FileChangeType,
    },

    /// Diff calculation completed for a document
    DiffCalculationCompleted {
        doc_id: DocumentId,
        path: PathBuf,
        calculation_duration_ms: u64,
        hunk_count: usize,
    },

    /// Diff calculation failed
    DiffCalculationFailed {
        doc_id: DocumentId,
        path: PathBuf,
        error: String,
    },
}

/// Represents a diff hunk with line change information
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// Start line in the new version (0-indexed)
    pub after_start: u32,
    /// End line in the new version (0-indexed)
    pub after_end: u32,
    /// Start line in the base version (0-indexed)  
    pub before_start: u32,
    /// End line in the base version (0-indexed)
    pub before_end: u32,
    /// Type of change this hunk represents
    pub change_type: HunkChangeType,
}

/// Type of change represented by a diff hunk
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HunkChangeType {
    /// Lines were added
    Addition,
    /// Lines were removed
    Deletion,
    /// Lines were modified
    Modification,
}

/// Git staging area status
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StageStatus {
    /// File is staged for commit
    Staged,
    /// File is not staged
    Unstaged,
    /// File is partially staged (some hunks staged, some not)
    PartiallyStaged,
}

/// Git working directory status
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WorkingStatus {
    /// File has been modified
    Modified,
    /// File is new/untracked
    Untracked,
    /// File has been deleted
    Deleted,
    /// File has conflicts
    Conflicted,
    /// File has been renamed
    Renamed,
}

/// Type of VCS provider
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VcsProviderType {
    Git,
    // Future: Mercurial, SVN, etc.
}

/// Type of file change in VCS
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileChangeType {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
}

impl From<helix_vcs::Hunk> for DiffHunk {
    fn from(hunk: helix_vcs::Hunk) -> Self {
        let change_type = if hunk.is_pure_insertion() {
            HunkChangeType::Addition
        } else if hunk.is_pure_removal() {
            HunkChangeType::Deletion
        } else {
            HunkChangeType::Modification
        };

        DiffHunk {
            after_start: hunk.after.start,
            after_end: hunk.after.end,
            before_start: hunk.before.start,
            before_end: hunk.before.end,
            change_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::DocumentId;

    #[test]
    fn test_vcs_event_creation() {
        let doc_id = DocumentId::default();
        let event = Event::DiffStatusChanged {
            doc_id,
            path: PathBuf::from("/test/file.rs"),
            hunks: vec![DiffHunk {
                after_start: 10,
                after_end: 15,
                before_start: 10,
                before_end: 12,
                change_type: HunkChangeType::Modification,
            }],
            diff_base_revision: Some("abc123".to_string()),
        };

        match event {
            Event::DiffStatusChanged { hunks, .. } => {
                assert_eq!(hunks.len(), 1);
                assert_eq!(hunks[0].change_type, HunkChangeType::Modification);
            }
            _ => panic!("Expected DiffStatusChanged event"),
        }
    }

    #[test]
    fn test_diff_hunk_types() {
        let addition = DiffHunk {
            after_start: 5,
            after_end: 10,
            before_start: 5,
            before_end: 5,
            change_type: HunkChangeType::Addition,
        };

        let deletion = DiffHunk {
            after_start: 5,
            after_end: 5,
            before_start: 5,
            before_end: 10,
            change_type: HunkChangeType::Deletion,
        };

        let modification = DiffHunk {
            after_start: 5,
            after_end: 8,
            before_start: 5,
            before_end: 7,
            change_type: HunkChangeType::Modification,
        };

        assert_eq!(addition.change_type, HunkChangeType::Addition);
        assert_eq!(deletion.change_type, HunkChangeType::Deletion);
        assert_eq!(modification.change_type, HunkChangeType::Modification);
    }

    #[test]
    fn test_stage_status_variants() {
        let statuses = [
            StageStatus::Staged,
            StageStatus::Unstaged,
            StageStatus::PartiallyStaged,
        ];

        for status in statuses {
            let _event = Event::FileStageStatusChanged {
                path: PathBuf::from("/test/file.rs"),
                stage_status: status,
                working_status: Some(WorkingStatus::Modified),
            };
        }
    }
}
