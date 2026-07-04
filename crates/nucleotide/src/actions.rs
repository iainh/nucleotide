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

pub(crate) mod tab_menus {
    use crate::workspace::{TabBarNewMenuIntent, TabBarSplitMenuIntent, TabContextMenuIntent};

    #[derive(Clone, PartialEq, Debug, gpui::Action)]
    #[action(namespace = tab_context_menu, no_json)]
    pub(crate) struct ContextOperation {
        pub(crate) intent: TabContextMenuIntent,
    }

    #[derive(Clone, PartialEq, Debug, gpui::Action)]
    #[action(namespace = tab_bar_split_menu, no_json)]
    pub(crate) struct SplitOperation {
        pub(crate) intent: TabBarSplitMenuIntent,
    }

    #[derive(Clone, PartialEq, Debug, gpui::Action)]
    #[action(namespace = tab_bar_new_menu, no_json)]
    pub(crate) struct NewOperation {
        pub(crate) intent: TabBarNewMenuIntent,
    }
}
