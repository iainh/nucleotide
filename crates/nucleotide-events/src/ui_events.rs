// ABOUTME: UI-specific events for UI components and styling
// ABOUTME: Events for theme changes, UI interactions, and widget state

use std::any::Any;
use std::path::PathBuf;
use std::sync::Arc;

/// UI-specific events (for nucleotide-ui crate)
#[derive(Clone)]
pub enum UiEvent {
    /// Theme changed
    ThemeChanged { theme_name: String },

    /// UI scale changed
    ScaleChanged { scale: f32 },

    /// Font changed
    FontChanged { font_family: String },

    /// Layout changed
    LayoutChanged,

    /// Search command submitted from overlay
    SearchSubmitted { query: String },

    /// Command submitted from overlay
    CommandSubmitted { command: String },

    /// File should be opened
    FileOpenRequested { path: PathBuf },

    /// Directory should be opened
    DirectoryOpenRequested { path: PathBuf },

    /// Info box should be shown
    ShowInfo { title: String, body: Vec<String> },

    /// Completion triggered
    CompletionTriggered,

    /// Show prompt (with boxed prompt object for transition period)
    ShowPrompt {
        prompt_text: String,
        initial_value: String,
        // Temporary: store the prompt object during transition
        prompt_object: Option<Arc<dyn Any + Send + Sync>>,
    },

    /// Show picker (with boxed picker object for transition period)
    ShowPicker {
        picker_type: PickerType,
        // Temporary: store the picker object during transition
        picker_object: Option<Arc<dyn Any + Send + Sync>>,
    },

    /// Show completion widget
    ShowCompletion,

    /// Hide completion widget
    HideCompletion,

    /// System appearance changed (dark/light mode)
    SystemAppearanceChanged { appearance: SystemAppearance },
}

#[derive(Debug, Clone, Copy)]
pub enum PickerType {
    File,
    Buffer,
    Directory,
    Command,
    Symbol,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SystemAppearance {
    Light,
    Dark,
}

impl std::fmt::Debug for UiEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiEvent::ThemeChanged { theme_name } => f
                .debug_struct("ThemeChanged")
                .field("theme_name", theme_name)
                .finish(),
            UiEvent::ScaleChanged { scale } => f
                .debug_struct("ScaleChanged")
                .field("scale", scale)
                .finish(),
            UiEvent::FontChanged { font_family } => f
                .debug_struct("FontChanged")
                .field("font_family", font_family)
                .finish(),
            UiEvent::LayoutChanged => write!(f, "LayoutChanged"),
            UiEvent::SearchSubmitted { query } => f
                .debug_struct("SearchSubmitted")
                .field("query", query)
                .finish(),
            UiEvent::CommandSubmitted { command } => f
                .debug_struct("CommandSubmitted")
                .field("command", command)
                .finish(),
            UiEvent::FileOpenRequested { path } => f
                .debug_struct("FileOpenRequested")
                .field("path", path)
                .finish(),
            UiEvent::DirectoryOpenRequested { path } => f
                .debug_struct("DirectoryOpenRequested")
                .field("path", path)
                .finish(),
            UiEvent::ShowInfo { title, body } => f
                .debug_struct("ShowInfo")
                .field("title", title)
                .field("body", body)
                .finish(),
            UiEvent::CompletionTriggered => write!(f, "CompletionTriggered"),
            UiEvent::ShowPrompt {
                prompt_text,
                initial_value,
                ..
            } => f
                .debug_struct("ShowPrompt")
                .field("prompt_text", prompt_text)
                .field("initial_value", initial_value)
                .finish(),
            UiEvent::ShowPicker { picker_type, .. } => f
                .debug_struct("ShowPicker")
                .field("picker_type", picker_type)
                .finish(),
            UiEvent::ShowCompletion => write!(f, "ShowCompletion"),
            UiEvent::HideCompletion => write!(f, "HideCompletion"),
            UiEvent::SystemAppearanceChanged { appearance } => f
                .debug_struct("SystemAppearanceChanged")
                .field("appearance", appearance)
                .finish(),
        }
    }
}
