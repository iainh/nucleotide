// ABOUTME: Main Nucleotide library crate
// ABOUTME: Provides the core application and UI integration

pub mod actions;
pub mod application;
pub mod config;
pub mod document;
pub mod editor_capabilities_impl;
pub mod editor_provider;
pub mod event_bridge;
pub mod file_tree;
pub mod gpui_to_helix_bridge;
pub mod overlay;
pub mod statusline;
pub mod types;
pub mod utils;
pub mod workspace;

// Re-export preview_tracker from nucleotide-core
pub use nucleotide_core::preview_tracker;

// Re-export modules that were moved to other crates
pub use nucleotide_ui::{
    completion, info_box, key_hint_view, notification, picker, picker_view, prompt, prompt_view,
    titlebar, Picker, Prompt, PromptElement,
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
