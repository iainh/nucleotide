// ABOUTME: Main Nucleotide library crate
// ABOUTME: Provides the core application and UI integration

pub mod actions;
pub mod application;
pub mod application_v2;
pub mod completion_coordinator;
pub mod completions;
pub mod config;
pub mod document;
pub mod editor_capabilities_impl;
pub mod editor_provider;
pub mod file_tree;
pub mod input_coordinator;
#[cfg(test)]
pub mod integration_test_phase2;
pub mod lsp_completion_trigger;
pub mod lsp_manager;
pub mod overlay;
pub mod project_indicator;
pub mod project_status_service;
pub mod shell_env;
#[cfg(test)]
pub mod shell_env_focused_test;
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
pub mod vcs_service;
pub mod workspace;

// Re-export preview_tracker from nucleotide-core
pub use nucleotide_core::preview_tracker;

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
