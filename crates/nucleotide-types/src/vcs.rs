// ABOUTME: Version control system types for diff information and file status
// ABOUTME: Core VCS types shared across the application without dependencies

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum VcsStatus {
    /// File is not tracked by VCS
    Untracked,
    /// File is tracked and not modified
    #[default]
    Clean,
    /// File is tracked and has been modified
    Modified,
    /// File has been added to VCS
    Added,
    /// File has been deleted from VCS
    Deleted,
    /// File has been renamed in VCS
    Renamed,
    /// File was created as a copy of another tracked file
    Copied,
    /// File has conflicts from merge
    Conflicted,
    /// Unknown VCS status
    Unknown,
}

impl VcsStatus {
    /// Returns true if the file has been modified in some way
    pub fn is_modified(&self) -> bool {
        matches!(
            self,
            Self::Modified
                | Self::Added
                | Self::Deleted
                | Self::Renamed
                | Self::Copied
                | Self::Conflicted
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

    /// Returns a human-readable description of this status
    pub fn description(&self) -> &'static str {
        match self {
            Self::Untracked => "Untracked",
            Self::Clean => "Clean",
            Self::Modified => "Modified",
            Self::Added => "Added",
            Self::Deleted => "Deleted",
            Self::Renamed => "Renamed",
            Self::Copied => "Copied",
            Self::Conflicted => "Conflicted",
            Self::Unknown => "Unknown",
        }
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
}
