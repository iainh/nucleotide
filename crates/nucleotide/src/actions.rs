// ABOUTME: Re-exports UI actions from nucleotide-ui
// ABOUTME: All action definitions have been moved to the UI layer

// Re-export all actions from nucleotide-ui
pub use nucleotide_ui::actions::*;

pub mod project_tree {
    use crate::file_tree::sidebar::ProjectTreeContextMenuIntent;

    #[derive(Clone, PartialEq, Debug, gpui::Action)]
    #[action(namespace = project_tree, no_json)]
    pub struct Operation {
        pub intent: ProjectTreeContextMenuIntent,
    }
}
