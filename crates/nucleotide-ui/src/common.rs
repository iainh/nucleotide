// ABOUTME: Common UI components and patterns to reduce duplication
// ABOUTME: Provides reusable building blocks for picker, prompt, and other views

#![allow(dead_code)]

use gpui::prelude::FluentBuilder;
use gpui::{
    Div, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement, ParentElement, Stateful,
    Styled, Window, black, div, hsla, px,
};

/// Common modal container styling
pub struct ModalContainer;

impl ModalContainer {
    /// Create a modal container with standard styling
    pub fn container() -> Div {
        div()
            .absolute()
            .bg(black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .size_full()
    }

    /// Create a modal panel with standard styling
    pub fn panel(style: &ModalStyle) -> Div {
        div()
            .flex()
            .flex_col()
            .bg(style.background)
            .border_1()
            .border_color(style.border)
            .rounded_md()
            .shadow_lg()
            .overflow_hidden()
    }
}

/// Common modal styling configuration
#[derive(Clone)]
pub struct ModalStyle {
    pub background: Hsla,
    pub text: Hsla,
    pub border: Hsla,
    pub selected_background: Hsla,
    pub selected_text: Hsla,
    pub prompt_text: Hsla,
}

impl Default for ModalStyle {
    fn default() -> Self {
        // Use design tokens for better theme consistency
        let tokens = crate::DesignTokens::dark();
        Self {
            background: tokens.colors.popup_background,
            text: tokens.colors.text_primary,
            border: tokens.colors.border_default,
            selected_background: tokens.colors.selection_primary,
            selected_text: tokens.colors.text_on_primary,
            prompt_text: tokens.colors.text_secondary,
        }
    }
}

impl ModalStyle {
    /// Create ModalStyle from helix theme
    pub fn from_theme(theme: &helix_view::Theme) -> Self {
        use crate::theme_utils::color_to_hsla;

        // Use design tokens for intelligent fallbacks instead of hardcoded grays
        let fallback_tokens = Self::default();

        let background = theme
            .get("ui.popup")
            .bg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.background").bg.and_then(color_to_hsla))
            .unwrap_or(fallback_tokens.background);

        let text = theme
            .get("ui.text")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(fallback_tokens.text);

        let selected_background = theme
            .get("ui.menu.selected")
            .bg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.selection").bg.and_then(color_to_hsla))
            .unwrap_or(fallback_tokens.selected_background);

        let selected_text = theme
            .get("ui.menu.selected")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(text);

        let border = theme
            .get("ui.popup")
            .fg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.text").fg.and_then(color_to_hsla))
            .map(|color| hsla(color.h, color.s, color.l * 0.5, color.a))
            .unwrap_or(fallback_tokens.border);

        let prompt_text = theme
            .get("ui.text")
            .fg
            .and_then(color_to_hsla)
            .map(|color| hsla(color.h, color.s, color.l * 0.7, color.a))
            .unwrap_or(fallback_tokens.prompt_text);

        Self {
            background,
            text,
            border,
            selected_background,
            selected_text,
            prompt_text,
        }
    }
}

/// Common search/filter input component
pub struct SearchInput;

impl SearchInput {
    /// Create a search input with cursor rendering
    pub fn render(
        query: &str,
        cursor_position: usize,
        cursor_color: Hsla,
        text_color: Hsla,
        is_focused: bool,
    ) -> impl IntoElement {
        let chars: Vec<char> = query.chars().collect();

        // Calculate byte position from character position
        let mut byte_pos = 0;
        for (i, ch) in chars.iter().enumerate() {
            if i >= cursor_position {
                break;
            }
            byte_pos += ch.len_utf8();
        }

        let before_cursor = query[..byte_pos].to_string();
        let after_cursor = query[byte_pos..].to_string();

        div()
            .flex()
            .items_center()
            .child(
                // Text before cursor
                div().when(!before_cursor.is_empty(), |this| {
                    this.child(before_cursor).text_color(text_color)
                }),
            )
            .child(
                // Cursor
                div()
                    .w(px(2.0))
                    .h(px(16.0))
                    .bg(cursor_color)
                    .when(!is_focused, |this| this.opacity(0.5)),
            )
            .child(
                // Text after cursor
                div().when(!after_cursor.is_empty(), |this| {
                    this.child(after_cursor).text_color(text_color)
                }),
            )
    }
}

/// Common list item component
pub struct ListItemElement;

impl ListItemElement {
    /// Create a list item with optional selection styling
    pub fn create(
        id: impl Into<ElementId>,
        is_selected: bool,
        style: &ModalStyle,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_col()
            .px_3()
            .min_h_8()
            .justify_center()
            .cursor_pointer()
            .when(is_selected, |this| {
                this.bg(style.selected_background)
                    .text_color(style.selected_text)
            })
            .when(!is_selected, |this| this.text_color(style.text))
    }
}

/// Common header/footer styling
pub struct ModalHeader;

impl ModalHeader {
    /// Create a modal header with title
    pub fn create(style: &ModalStyle) -> Div {
        div()
            .flex()
            .items_center()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(style.border)
    }
}

pub struct ModalFooter;

impl ModalFooter {
    /// Create a modal footer with instructions
    pub fn create(style: &ModalStyle) -> Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(style.border)
            .text_size(px(11.))
            .text_color(style.prompt_text)
    }
}

/// Common focus handling utilities
pub trait FocusableModal {
    fn ensure_focus(&self, window: &mut Window, focus_handle: &FocusHandle) {
        if !focus_handle.is_focused(window) {
            focus_handle.focus(window);
        }
    }
}

/// Common keyboard input handling for text fields
pub trait TextInputHandler {
    fn cursor_position(&self) -> usize;
    fn set_cursor_position(&mut self, pos: usize);
    fn input_text(&self) -> &str;
    fn set_input_text(&mut self, text: String);

    fn handle_char_input(&mut self, ch: char) {
        if ch.is_alphanumeric()
            || ch.is_ascii_punctuation()
            || ch == ' '
            || ch == '/'
            || ch == '.'
            || ch == '-'
            || ch == '_'
        {
            self.insert_char(ch);
        }
    }

    fn insert_char(&mut self, ch: char) {
        let mut input = self.input_text().to_string();
        let chars: Vec<char> = input.chars().collect();
        let cursor_pos = self.cursor_position();

        // Calculate byte position from character position
        let mut byte_pos = 0;
        for (i, c) in chars.iter().enumerate() {
            if i >= cursor_pos {
                break;
            }
            byte_pos += c.len_utf8();
        }

        input.insert(byte_pos, ch);
        self.set_input_text(input);
        self.set_cursor_position(cursor_pos + 1);
        // Notification should be handled by the implementing type
    }

    fn delete_char_backward(&mut self) {
        let cursor_pos = self.cursor_position();
        if cursor_pos > 0 {
            let mut input = self.input_text().to_string();
            let char_pos = cursor_pos.saturating_sub(1);
            let chars: Vec<char> = input.chars().collect();

            if char_pos < chars.len() {
                // Find the byte position for the character position
                let mut byte_pos = 0;
                for (i, ch) in input.chars().enumerate() {
                    if i == char_pos {
                        break;
                    }
                    byte_pos += ch.len_utf8();
                }

                // Safe access to character at position
                if let Some(ch) = input.chars().nth(char_pos) {
                    let ch_len = ch.len_utf8();
                    input.drain(byte_pos..byte_pos + ch_len);
                    self.set_input_text(input);
                    self.set_cursor_position(char_pos);
                    // Notification should be handled by the implementing type
                }
            }
        }
    }

    fn move_cursor_left(&mut self) {
        let pos = self.cursor_position();
        if pos > 0 {
            self.set_cursor_position(pos - 1);
            // Notification should be handled by the implementing type
        }
    }

    fn move_cursor_right(&mut self) {
        let pos = self.cursor_position();
        let char_count = self.input_text().chars().count();
        if pos < char_count {
            self.set_cursor_position(pos + 1);
            // Notification should be handled by the implementing type
        }
    }

    fn move_cursor_home(&mut self) {
        self.set_cursor_position(0);
        // Notification should be handled by the implementing type
    }

    fn move_cursor_end(&mut self) {
        let char_count = self.input_text().chars().count();
        self.set_cursor_position(char_count);
        // Notification should be handled by the implementing type
    }
}
