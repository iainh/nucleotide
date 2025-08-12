// ABOUTME: Text rendering utilities for styled text display
// ABOUTME: Provides helpers for text with highlighting and formatting

use gpui::{HighlightStyle, SharedString, StyledText, TextStyle};

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
