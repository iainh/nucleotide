// ABOUTME: Complex UI widgets for Nucleotide
// ABOUTME: Provides file tree, pickers, overlays, and other composite UI components

use gpui::{HighlightStyle, SharedString, StyledText, TextStyle};
use helix_core::diagnostic::Severity;

/// Editor status information for notifications
#[derive(Clone, Debug)]
pub struct EditorStatus {
    pub status: String,
    pub severity: Severity,
}

/// Styled text utility for rendering text with highlights
#[derive(Debug, Clone)]
pub struct TextWithStyle {
    text: SharedString,
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
}

impl TextWithStyle {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            highlights: Vec::new(),
        }
    }

    pub fn with_highlights(
        mut self,
        highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    ) -> Self {
        self.highlights = highlights;
        self
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn into_styled_text(self, _default_style: &TextStyle) -> StyledText {
        StyledText::new(self.text).with_highlights(self.highlights)
    }

    pub fn style(&self, idx: usize) -> Option<&HighlightStyle> {
        self.highlights.get(idx).map(|(_, style)| style)
    }
}

pub mod actions;
pub mod completion;
pub mod file_tree;
pub mod info_box;
pub mod key_hint_view;
pub mod notification;
pub mod overlay;
pub mod picker;
pub mod picker_delegate;
pub mod picker_element;
pub mod picker_view;
pub mod preview_tracker;
pub mod prompt;
pub mod prompt_view;

// Re-export commonly used items
pub use completion::{CompletionItem, CompletionItemKind, CompletionView};
pub use file_tree::{FileTreeConfig, FileTreeEvent, FileTreeView};
pub use info_box::InfoBoxView;
pub use key_hint_view::KeyHintView;
pub use notification::NotificationView;
pub use overlay::OverlayView;
// PickerDelegate is in picker_delegate module, not picker
pub use picker_view::{PickerItem, PickerView};
pub use prompt::Prompt;
pub use prompt_view::PromptView;

// Types are exported automatically as they're defined in this module
