// ABOUTME: Common UI components and patterns to reduce duplication
// ABOUTME: Provides reusable building blocks for picker, prompt, and other views

use gpui::{App, FocusHandle, Hsla, Window};

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

        // Fallback: derive chrome tokens from the Helix popup/background surface.
        use crate::theme_utils::color_to_hsla;
        let fallback_tokens = Self::default();
        let surface = theme
            .get("ui.popup")
            .bg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.background").bg.and_then(color_to_hsla))
            .unwrap_or(fallback_tokens.background);
        let is_dark = surface.l < 0.5;
        let chrome = crate::tokens::ChromeTokens::from_surface_color(surface, is_dark);
        let editor = crate::tokens::EditorTokens::fallback(is_dark);
        let tokens = crate::DesignTokens {
            editor,
            chrome,
            sizes: crate::tokens::SizeTokens::default(),
        };
        let dd = tokens.dropdown_tokens();
        let background = tokens.chrome.popup_background;
        let text = crate::styling::ColorTheory::ensure_contrast(
            background,
            tokens.chrome.text_on_chrome,
            crate::styling::color_theory::ContrastRatios::AA_NORMAL,
        );

        Self {
            background,
            text,
            border: tokens.chrome.popup_border,
            selected_background: dd.item_background_selected,
            selected_text: dd.item_text_selected,
            prompt_text: tokens.chrome.text_chrome_secondary,
        }
    }
}

/// Common focus handling utilities
pub trait FocusableModal {
    fn ensure_focus(&self, window: &mut Window, cx: &mut App, focus_handle: &FocusHandle) {
        if !focus_handle.is_focused(window) {
            focus_handle.focus(window, cx);
        }
    }
}

// Text input helper trait removed; picker_view handles input directly.
