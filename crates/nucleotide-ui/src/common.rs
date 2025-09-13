// ABOUTME: Common UI components and patterns to reduce duplication
// ABOUTME: Provides reusable building blocks for picker, prompt, and other views

#![allow(dead_code)]

use gpui::prelude::FluentBuilder;
use gpui::{
    Div, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement, ParentElement, Stateful,
    Styled, Window, div, hsla, px,
};

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
        let dd = tokens.dropdown_tokens();
        Self {
            background: tokens.chrome.popup_background,
            text: crate::styling::ColorTheory::ensure_contrast(
                tokens.chrome.popup_background,
                tokens.chrome.text_on_chrome,
                crate::styling::color_theory::ContrastRatios::AA_NORMAL,
            ),
            border: tokens.chrome.popup_border,
            // Align picker selection with dropdown menus
            selected_background: dd.item_background_selected,
            selected_text: dd.item_text_selected,
            prompt_text: tokens.chrome.text_chrome_secondary,
        }
    }
}

impl ModalStyle {
    /// Create ModalStyle using our ThemeProvider tokens when available (OKLab/OKLCH-driven)
    /// Falls back to Helix theme mapping only if provider is unavailable
    pub fn from_theme(theme: &helix_view::Theme) -> Self {
        if let Some(provider) = crate::providers::use_theme_provider() {
            let theme = provider.current_theme();
            let dt = theme.tokens;
            let dd = dt.dropdown_tokens();
            return Self {
                background: dt.chrome.popup_background,
                text: crate::styling::ColorTheory::ensure_contrast(
                    dt.chrome.popup_background,
                    dt.chrome.text_on_chrome,
                    crate::styling::color_theory::ContrastRatios::AA_NORMAL,
                ),
                border: dt.chrome.popup_border,
                // Align selection with dropdowns
                selected_background: dd.item_background_selected,
                selected_text: dd.item_text_selected,
                prompt_text: dt.chrome.text_chrome_secondary,
            };
        }

        // Fallback: Derive from Helix theme (legacy path)
        use crate::theme_utils::color_to_hsla;
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

/// Common focus handling utilities
pub trait FocusableModal {
    fn ensure_focus(&self, window: &mut Window, focus_handle: &FocusHandle) {
        if !focus_handle.is_focused(window) {
            focus_handle.focus(window);
        }
    }
}

// Text input helper trait removed; picker_view handles input directly.
