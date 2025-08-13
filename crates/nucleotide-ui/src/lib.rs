// ABOUTME: Reusable UI components library following Zed's patterns
// ABOUTME: Provides consistent, styled components for the application

pub mod actions;
pub mod assets;
pub mod button;
pub mod common;
pub mod completion;
pub mod file_icon;
pub mod info_box;
pub mod key_hint_view;
pub mod list_item;
pub mod notification;
pub mod picker;
pub mod picker_delegate;
pub mod picker_element;
pub mod picker_view;
pub mod prompt;
pub mod prompt_view;
pub mod scrollbar;
pub mod style_utils;
pub mod text_utils;
pub mod theme_manager;
pub mod theme_utils;
pub mod titlebar;
pub mod vcs_indicator;

pub use assets::Assets;
pub use file_icon::FileIcon;
pub use list_item::*;
pub use picker::Picker;
pub use prompt::{Prompt, PromptElement};
pub use vcs_indicator::{VcsIndicator, VcsStatus};

use gpui::{hsla, App, Hsla};

/// Standard spacing values following Zed's design system
pub mod spacing {
    use gpui::px;

    pub const XS: gpui::Pixels = px(2.);
    pub const SM: gpui::Pixels = px(4.);
    pub const MD: gpui::Pixels = px(8.);
    pub const LG: gpui::Pixels = px(12.);
}

/// Theme trait for consistent styling
pub trait Themed {
    fn theme(&self, cx: &App) -> &Theme;
}

/// Application theme following Zed's pattern
#[derive(Clone, Debug)]
pub struct Theme {
    pub background: Hsla,
    pub surface: Hsla,
    pub surface_background: Hsla,
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub border: Hsla,
    pub border_focused: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_disabled: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_active: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            background: hsla(0.0, 0.0, 0.05, 1.0),
            surface: hsla(0.0, 0.0, 0.1, 1.0),
            surface_background: hsla(0.0, 0.0, 0.08, 1.0),
            surface_hover: hsla(0.0, 0.0, 0.15, 1.0),
            surface_active: hsla(0.0, 0.0, 0.2, 1.0),
            border: hsla(0.0, 0.0, 0.2, 1.0),
            border_focused: hsla(220.0 / 360.0, 0.6, 0.5, 1.0),
            text: hsla(0.0, 0.0, 0.9, 1.0),
            text_muted: hsla(0.0, 0.0, 0.7, 1.0),
            text_disabled: hsla(0.0, 0.0, 0.5, 1.0),
            accent: hsla(220.0 / 360.0, 0.6, 0.5, 1.0),
            accent_hover: hsla(220.0 / 360.0, 0.6, 0.6, 1.0),
            accent_active: hsla(220.0 / 360.0, 0.6, 0.4, 1.0),
            error: hsla(0.0, 0.8, 0.5, 1.0),
            warning: hsla(40.0 / 360.0, 0.8, 0.5, 1.0),
            success: hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
        }
    }

    pub fn light() -> Self {
        Self {
            background: hsla(0.0, 0.0, 1.0, 1.0),
            surface: hsla(0.0, 0.0, 0.98, 1.0),
            surface_background: hsla(0.0, 0.0, 0.99, 1.0),
            surface_hover: hsla(0.0, 0.0, 0.95, 1.0),
            surface_active: hsla(0.0, 0.0, 0.92, 1.0),
            border: hsla(0.0, 0.0, 0.9, 1.0),
            border_focused: hsla(220.0 / 360.0, 0.6, 0.5, 1.0),
            text: hsla(0.0, 0.0, 0.1, 1.0),
            text_muted: hsla(0.0, 0.0, 0.4, 1.0),
            text_disabled: hsla(0.0, 0.0, 0.6, 1.0),
            accent: hsla(220.0 / 360.0, 0.6, 0.5, 1.0),
            accent_hover: hsla(220.0 / 360.0, 0.6, 0.4, 1.0),
            accent_active: hsla(220.0 / 360.0, 0.6, 0.6, 1.0),
            error: hsla(0.0, 0.8, 0.5, 1.0),
            warning: hsla(40.0 / 360.0, 0.8, 0.5, 1.0),
            success: hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
        }
    }
}

impl gpui::Global for Theme {}
