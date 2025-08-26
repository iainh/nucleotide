// ABOUTME: Main Nucleotide library crate
// ABOUTME: Provides the core application and UI integration

pub mod actions;
pub mod application;
pub mod cli;
// application_v2 merged into application module
pub mod completion_coordinator;
pub mod completion_interception;
pub mod completions;
pub mod config;
pub mod document;
pub mod editor_capabilities_impl;
pub mod editor_provider;
pub mod file_tree;
pub mod input_coordinator;
#[cfg(test)]
pub mod integration_test_phase2;
pub mod overlay;
pub mod statusline;
pub mod tab;
pub mod tab_bar;
pub mod tab_overflow_dropdown;
#[cfg(test)]
pub mod test_utils;
#[cfg(test)]
pub mod tests;
pub mod types;
pub mod utils;
pub mod workspace;

// Re-export from nucleotide-core
pub use nucleotide_core::preview_tracker;

// Re-export from new specialized crates
pub use nucleotide_env::{
    CachedEnvironment, EnvironmentOrigin, ProjectEnvironment, ShellEnvError, ShellEnvironmentCache,
    ShellEnvironmentError,
};
pub use nucleotide_lsp::{LspError, LspManager, LspManagerConfig, LspStartupMode};
pub use nucleotide_project::{
    ProjectInfo, ProjectLspStatus, ProjectStatusHandle, ProjectStatusService, ProjectType,
};
pub use nucleotide_vcs::{VcsConfig, VcsEvent, VcsService, VcsServiceHandle};

// Re-export modules that were moved to other crates
pub use nucleotide_ui::{
    Picker, Prompt, PromptElement, completion_v2 as completion, info_box, key_hint_view,
    notification, picker, picker_view, prompt, prompt_view, titlebar,
};

// Re-export commonly used items
pub use application::{Application, Input, InputEvent};
pub use nucleotide_ui::theme_manager::ThemeManager;
pub use types::{
    EditorFontConfig, EditorStatus as EditorStatusType, FontSettings, UiFontConfig, Update,
};
// TextWithStyle moved to nucleotide-ui
pub use nucleotide_ui::text_utils::TextWithStyle;

// Type alias for Core
pub type Core = Application;
