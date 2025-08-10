// ABOUTME: Reusable cursor input component for text fields
// ABOUTME: Handles cursor rendering and text input with proper unicode support

use gpui::prelude::FluentBuilder;
use gpui::*;

/// A text input field with cursor rendering
pub struct CursorInput {
    text: SharedString,
    cursor_position: usize,
    cursor_color: Hsla,
    text_color: Hsla,
    placeholder: Option<SharedString>,
    placeholder_color: Hsla,
    font: Font,
    font_size: AbsoluteLength,
    is_focused: bool,
}

impl CursorInput {
    pub fn new(font: Font) -> Self {
        Self {
            text: SharedString::default(),
            cursor_position: 0,
            cursor_color: hsla(0.0, 0.0, 0.9, 1.0),
            text_color: hsla(0.0, 0.0, 0.9, 1.0),
            placeholder: None,
            placeholder_color: hsla(0.0, 0.0, 0.5, 1.0),
            font,
            font_size: px(14.0).into(),
            is_focused: true,
        }
    }

    pub fn with_text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = text.into();
        self
    }

    pub fn with_cursor_position(mut self, position: usize) -> Self {
        self.cursor_position = position;
        self
    }

    pub fn with_cursor_color(mut self, color: Hsla) -> Self {
        self.cursor_color = color;
        self
    }

    pub fn with_text_color(mut self, color: Hsla) -> Self {
        self.text_color = color;
        self
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn with_placeholder_color(mut self, color: Hsla) -> Self {
        self.placeholder_color = color;
        self
    }

    pub fn with_font(mut self, font: Font) -> Self {
        self.font = font;
        self
    }

    pub fn with_font_size(mut self, size: AbsoluteLength) -> Self {
        self.font_size = size;
        self
    }

    pub fn with_focused(mut self, focused: bool) -> Self {
        self.is_focused = focused;
        self
    }
}

impl RenderOnce for CursorInput {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let chars: Vec<char> = self.text.chars().collect();
        let cursor_position = self.cursor_position.min(chars.len());

        // Calculate byte position from character position
        let mut byte_pos = 0;
        for (i, ch) in chars.iter().enumerate() {
            if i >= cursor_position {
                break;
            }
            byte_pos += ch.len_utf8();
        }

        let text_str = self.text.as_ref();
        let before_cursor = &text_str[..byte_pos];
        let after_cursor = &text_str[byte_pos..];

        // Check if we should show placeholder
        let show_placeholder = self.text.is_empty() && self.placeholder.is_some();

        div()
            .flex()
            .items_center()
            .font(self.font)
            .text_size(self.font_size.to_pixels(px(14.0)))
            .when(show_placeholder, |this| {
                if let Some(placeholder) = self.placeholder {
                    this.child(
                        div()
                            .absolute()
                            .text_color(self.placeholder_color)
                            .child(placeholder),
                    )
                } else {
                    this
                }
            })
            .when(!show_placeholder, |this| {
                this.child(
                    // Text before cursor
                    div().when(!before_cursor.is_empty(), |this| {
                        this.child(before_cursor.to_string())
                            .text_color(self.text_color)
                    }),
                )
                .child(
                    // Cursor
                    div()
                        .w(px(2.0))
                        .h_full()
                        .bg(self.cursor_color)
                        .when(!self.is_focused, |this| this.opacity(0.5)),
                )
                .child(
                    // Text after cursor
                    div().when(!after_cursor.is_empty(), |this| {
                        this.child(after_cursor.to_string())
                            .text_color(self.text_color)
                    }),
                )
            })
    }
}

/// Helper functions for cursor input manipulation
pub mod cursor_utils {
    /// Calculate the character position from a byte offset
    pub fn byte_to_char_pos(text: &str, byte_offset: usize) -> usize {
        text[..byte_offset.min(text.len())].chars().count()
    }

    /// Calculate the byte offset from a character position
    pub fn char_to_byte_pos(text: &str, char_pos: usize) -> usize {
        let mut byte_pos = 0;
        for (i, ch) in text.chars().enumerate() {
            if i >= char_pos {
                break;
            }
            byte_pos += ch.len_utf8();
        }
        byte_pos
    }

    /// Insert a character at the given position (in characters, not bytes)
    pub fn insert_char_at(text: &mut String, ch: char, char_pos: usize) {
        let byte_pos = char_to_byte_pos(text, char_pos);
        text.insert(byte_pos, ch);
    }

    /// Remove a character at the given position (in characters, not bytes)
    pub fn remove_char_at(text: &mut String, char_pos: usize) -> Option<char> {
        let chars: Vec<char> = text.chars().collect();
        if char_pos < chars.len() {
            let byte_pos = char_to_byte_pos(text, char_pos);
            let ch = chars[char_pos];
            let ch_len = ch.len_utf8();
            text.drain(byte_pos..byte_pos + ch_len);
            Some(ch)
        } else {
            None
        }
    }
}
