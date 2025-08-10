// ABOUTME: Theme conversion utilities for bridging helix themes to GPUI
// ABOUTME: Centralized location for color conversions and theme extraction

use gpui::Hsla;
use helix_view::document::Mode;
use helix_view::graphics::Color;
use helix_view::Theme as HelixTheme;

/// Convert helix Color to GPUI Hsla
pub fn color_to_hsla(color: Color) -> Option<Hsla> {
    crate::utils::color_to_hsla(color)
}

/// Extract cursor color from helix theme based on mode
pub fn get_cursor_color(theme: &HelixTheme, mode: Mode) -> Option<Hsla> {
    let cursor_style = match mode {
        Mode::Normal => theme.get("ui.cursor"),
        Mode::Insert => theme.get("ui.cursor.insert"),
        Mode::Select => theme.get("ui.cursor.select"),
    };

    cursor_style
        .fg
        .or(cursor_style.bg)
        .and_then(color_to_hsla)
        .or_else(|| theme.get("ui.cursor.primary").fg.and_then(color_to_hsla))
}

/// Extract common UI colors from helix theme
pub struct ThemeColors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
    pub selection: Hsla,
    pub cursor: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub hint: Hsla,
}

impl ThemeColors {
    pub fn from_helix_theme(theme: &HelixTheme) -> Self {
        use gpui::hsla;

        let background = theme
            .get("ui.background")
            .bg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.1, 1.0));

        let foreground = theme
            .get("ui.text")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.9, 1.0));

        let border = theme
            .get("ui.window")
            .fg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.text").fg.and_then(color_to_hsla))
            .map(|color| hsla(color.h, color.s, color.l * 0.5, color.a))
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.3, 1.0));

        let selection = theme
            .get("ui.selection")
            .bg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(220.0 / 360.0, 0.6, 0.5, 0.3));

        let cursor = get_cursor_color(theme, Mode::Normal).unwrap_or(foreground);

        let error = theme
            .get("diagnostic.error")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(0.0, 0.8, 0.5, 1.0));

        let warning = theme
            .get("diagnostic.warning")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(40.0 / 360.0, 0.8, 0.5, 1.0));

        let info = theme
            .get("diagnostic.info")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(220.0 / 360.0, 0.6, 0.5, 1.0));

        let hint = theme
            .get("diagnostic.hint")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(280.0 / 360.0, 0.6, 0.5, 1.0));

        Self {
            background,
            foreground,
            border,
            selection,
            cursor,
            error,
            warning,
            info,
            hint,
        }
    }
}

/// Extract modal-specific colors from helix theme
pub fn get_modal_colors(theme: &HelixTheme) -> crate::ui::common::ModalStyle {
    crate::ui::common::ModalStyle::from_theme(theme)
}

/// Extract list/menu colors from helix theme
#[derive(Clone)]
pub struct ListColors {
    pub background: Hsla,
    pub hover_background: Hsla,
    pub selected_background: Hsla,
    pub text: Hsla,
    pub selected_text: Hsla,
}

impl ListColors {
    pub fn from_helix_theme(theme: &HelixTheme) -> Self {
        use gpui::hsla;

        let background = theme
            .get("ui.popup")
            .bg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.background").bg.and_then(color_to_hsla))
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.1, 1.0));

        let text = theme
            .get("ui.text")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.9, 1.0));

        let selected_background = theme
            .get("ui.menu.selected")
            .bg
            .and_then(color_to_hsla)
            .or_else(|| theme.get("ui.selection").bg.and_then(color_to_hsla))
            .unwrap_or_else(|| hsla(220.0 / 360.0, 0.6, 0.5, 1.0));

        let selected_text = theme
            .get("ui.menu.selected")
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(text);

        let hover_background = theme
            .get("ui.menu.hover")
            .bg
            .and_then(color_to_hsla)
            .or_else(|| {
                // Create a hover color by lightening the background
                Some(hsla(
                    background.h,
                    background.s,
                    (background.l + 0.05).min(1.0),
                    background.a,
                ))
            })
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.15, 1.0));

        Self {
            background,
            hover_background,
            selected_background,
            text,
            selected_text,
        }
    }
}
