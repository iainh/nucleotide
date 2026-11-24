//! One vocabulary for version control status: the letter and colour that the
//! file tree, the Changes list, History and file icons show for a `VcsStatus`.

use gpui::Hsla;
use nucleotide_types::VcsStatus;

use crate::DesignTokens;

/// Badge letter and colour for a status, shared by every VCS surface.
pub trait VcsStatusStyle {
    /// The one-character badge shown beside a changed path; empty when clean.
    fn label(self) -> &'static str;
    /// The badge colour, from the editor's VCS tokens so badges match the gutter.
    fn color(self, tokens: &DesignTokens) -> Hsla;
}

impl VcsStatusStyle for VcsStatus {
    fn label(self) -> &'static str {
        match self {
            VcsStatus::Untracked => "?",
            VcsStatus::Clean => "",
            VcsStatus::Modified => "M",
            VcsStatus::Added => "A",
            VcsStatus::Deleted => "D",
            VcsStatus::Renamed => "R",
            // Copied and Conflicted use git's own letters, as `git status` prints them.
            VcsStatus::Copied => "C",
            VcsStatus::Conflicted => "U",
            VcsStatus::Unknown => "!",
        }
    }

    fn color(self, tokens: &DesignTokens) -> Hsla {
        match self {
            VcsStatus::Modified | VcsStatus::Renamed => tokens.editor.vcs_modified,
            VcsStatus::Added | VcsStatus::Copied | VcsStatus::Untracked => tokens.editor.vcs_added,
            VcsStatus::Deleted => tokens.editor.vcs_deleted,
            VcsStatus::Conflicted | VcsStatus::Unknown => tokens.editor.error,
            VcsStatus::Clean => tokens.editor.text_secondary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [VcsStatus; 9] = [
        VcsStatus::Untracked,
        VcsStatus::Clean,
        VcsStatus::Modified,
        VcsStatus::Added,
        VcsStatus::Deleted,
        VcsStatus::Renamed,
        VcsStatus::Copied,
        VcsStatus::Conflicted,
        VcsStatus::Unknown,
    ];

    #[test]
    fn every_change_has_its_own_letter() {
        let labels: Vec<_> = ALL
            .iter()
            .filter(|status| **status != VcsStatus::Clean)
            .map(|status| status.label())
            .collect();
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "duplicate letters: {labels:?}");
        assert_eq!(VcsStatus::Clean.label(), "");
        assert_eq!(VcsStatus::Copied.label(), "C");
        assert_eq!(VcsStatus::Conflicted.label(), "U");
        assert_eq!(VcsStatus::Untracked.label(), "?");
        assert_eq!(VcsStatus::Unknown.label(), "!");
    }

    #[test]
    fn colors_follow_editor_vcs_tokens_in_both_appearances() {
        for tokens in [DesignTokens::dark(), DesignTokens::light()] {
            assert_eq!(
                VcsStatus::Modified.color(&tokens),
                tokens.editor.vcs_modified
            );
            assert_eq!(
                VcsStatus::Renamed.color(&tokens),
                tokens.editor.vcs_modified
            );
            assert_eq!(VcsStatus::Added.color(&tokens), tokens.editor.vcs_added);
            assert_eq!(VcsStatus::Copied.color(&tokens), tokens.editor.vcs_added);
            assert_eq!(VcsStatus::Untracked.color(&tokens), tokens.editor.vcs_added);
            assert_eq!(VcsStatus::Deleted.color(&tokens), tokens.editor.vcs_deleted);
            assert_eq!(VcsStatus::Conflicted.color(&tokens), tokens.editor.error);
            assert_eq!(VcsStatus::Unknown.color(&tokens), tokens.editor.error);
        }
    }
}
