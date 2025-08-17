// ABOUTME: Unified theme management for consistent styling across the application
// ABOUTME: Bridges between Helix themes and GPUI styling requirements

use crate::theme_utils::color_to_hsla;
use crate::Theme as UITheme;
use gpui::{hsla, App, Global, Hsla, WindowAppearance};
use helix_view::Theme as HelixTheme;

/// Extracted colors from Helix theme for comprehensive design token creation
#[derive(Debug, Clone, Copy)]
pub struct HelixThemeColors {
    pub selection: Hsla,
    pub cursor_normal: Hsla,
    pub cursor_insert: Hsla,
    pub cursor_select: Hsla,
    pub cursor_match: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub statusline: Hsla,
    pub popup: Hsla,
}

/// System appearance state
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SystemAppearance {
    #[default]
    Light,
    Dark,
}

impl From<WindowAppearance> for SystemAppearance {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => SystemAppearance::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => SystemAppearance::Dark,
        }
    }
}

/// Global SystemAppearance state for GPUI integration
#[derive(Default)]
struct GlobalSystemAppearance(SystemAppearance);

impl Global for GlobalSystemAppearance {}

impl SystemAppearance {
    /// Initializes the global SystemAppearance based on the current window appearance
    pub fn init(cx: &mut App) {
        *cx.default_global::<GlobalSystemAppearance>() =
            GlobalSystemAppearance(SystemAppearance::from(cx.window_appearance()));
    }

    /// Returns the global SystemAppearance
    pub fn global(cx: &App) -> Self {
        cx.global::<GlobalSystemAppearance>().0
    }

    /// Returns a mutable reference to the global SystemAppearance
    pub fn global_mut(cx: &mut App) -> &mut Self {
        &mut cx.global_mut::<GlobalSystemAppearance>().0
    }
}

/// Manages theme state and provides consistent access to theme colors
#[derive(Clone)]
pub struct ThemeManager {
    /// The current Helix theme
    helix_theme: HelixTheme,
    /// Cached UI theme derived from the Helix theme
    ui_theme: UITheme,
    /// Current system appearance
    system_appearance: SystemAppearance,
}

impl ThemeManager {
    /// Create a new ThemeManager from a Helix theme
    pub fn new(helix_theme: HelixTheme) -> Self {
        let system_appearance = SystemAppearance::default();
        let ui_theme = Self::derive_ui_theme_with_appearance(&helix_theme, system_appearance);
        Self {
            helix_theme,
            ui_theme,
            system_appearance,
        }
    }

    /// Update the theme
    pub fn set_theme(&mut self, helix_theme: HelixTheme) {
        self.ui_theme = Self::derive_ui_theme_with_appearance(&helix_theme, self.system_appearance);
        self.helix_theme = helix_theme;
    }

    /// Get the current Helix theme
    pub fn helix_theme(&self) -> &HelixTheme {
        &self.helix_theme
    }

    /// Get a theme style with fallback testing support
    /// Use this instead of helix_theme().get() for testing-aware color lookups
    pub fn theme_style(&self, key: &str) -> helix_view::graphics::Style {
        let test_fallback = std::env::var("NUCLEOTIDE_DISABLE_THEME_LOADING")
            .map(|val| val == "1" || val.to_lowercase() == "true")
            .unwrap_or(false);

        if test_fallback {
            nucleotide_logging::debug!(
                key = key,
                "TESTING: Returning computed UI color for theme key"
            );
            // Return computed colors from our UI theme instead of Helix theme
            self.compute_style_for_key(key)
        } else {
            self.helix_theme.get(key)
        }
    }

    /// Get computed style for a given theme key using our UI theme colors
    fn compute_style_for_key(&self, key: &str) -> helix_view::graphics::Style {
        use helix_view::graphics::{Color, Style};

        // Convert our UI theme colors to Helix Color format
        let to_helix_color = |hsla: gpui::Hsla| -> Option<Color> {
            // Convert HSLA to RGB
            let (r, g, b) = hsla_to_rgb(hsla.h, hsla.s, hsla.l);
            Some(Color::Rgb(
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
            ))
        };

        let ui_theme = &self.ui_theme;

        match key {
            "ui.background" => Style {
                bg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.text" => Style {
                fg: to_helix_color(ui_theme.text),
                ..Default::default()
            },
            "ui.cursor" | "ui.cursor.primary" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.cursor_normal),
                fg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.selection" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.selection_primary),
                ..Default::default()
            },
            "ui.statusline" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.statusline_active),
                fg: to_helix_color(ui_theme.text),
                ..Default::default()
            },
            "ui.window" => Style {
                fg: to_helix_color(ui_theme.border),
                ..Default::default()
            },
            "ui.menu" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.menu_background),
                fg: to_helix_color(ui_theme.text),
                ..Default::default()
            },
            "ui.popup" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.popup_background),
                fg: to_helix_color(ui_theme.tokens.colors.popup_border), // fg is used for borders in popups
                ..Default::default()
            },
            "ui.menu.selected" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.menu_selected),
                fg: to_helix_color(ui_theme.text),
                ..Default::default()
            },
            "ui.background.separator" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.separator_horizontal),
                ..Default::default()
            },
            "ui.cursor.primary.insert" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.cursor_insert),
                fg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.cursor.primary.select" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.cursor_select),
                fg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.cursor.primary.normal" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.cursor_normal),
                fg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.cursorline.primary" => Style {
                bg: to_helix_color(ui_theme.surface_hover),
                ..Default::default()
            },
            "ui.virtual.ruler" => Style {
                bg: to_helix_color(ui_theme.border),
                ..Default::default()
            },
            "ui.virtual.wrap" => Style {
                fg: to_helix_color(ui_theme.text_muted),
                ..Default::default()
            },
            "ui.gutter" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.line_number),
                bg: to_helix_color(ui_theme.tokens.colors.gutter_background),
                ..Default::default()
            },
            "ui.gutter.selected" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.line_number_active),
                bg: to_helix_color(ui_theme.tokens.colors.gutter_selected),
                ..Default::default()
            },
            "ui.gutter.virtual" => Style {
                fg: to_helix_color(ui_theme.text_disabled),
                bg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.gutter.selected.virtual" => Style {
                fg: to_helix_color(ui_theme.text_muted),
                bg: to_helix_color(ui_theme.surface_hover),
                ..Default::default()
            },
            "error" => Style {
                fg: to_helix_color(ui_theme.error),
                ..Default::default()
            },
            "warning" => Style {
                fg: to_helix_color(ui_theme.warning),
                ..Default::default()
            },
            "info" => Style {
                fg: to_helix_color(ui_theme.success),
                ..Default::default()
            },
            "hint" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.diagnostic_hint),
                ..Default::default()
            },
            // Enhanced cursor and selection mappings
            "ui.cursor.normal" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.cursor_normal),
                fg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.cursor.insert" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.cursor_insert),
                fg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.cursor.select" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.cursor_select),
                fg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.cursor.match" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.cursor_match),
                fg: to_helix_color(ui_theme.background),
                ..Default::default()
            },
            "ui.highlight" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.selection_secondary),
                ..Default::default()
            },
            // Enhanced gutter mappings
            "ui.linenr" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.line_number),
                bg: to_helix_color(ui_theme.tokens.colors.gutter_background),
                ..Default::default()
            },
            "ui.linenr.selected" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.line_number_active),
                bg: to_helix_color(ui_theme.tokens.colors.gutter_selected),
                ..Default::default()
            },
            // Enhanced status and buffer mappings
            "ui.statusline.inactive" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.statusline_inactive),
                fg: to_helix_color(ui_theme.text_muted),
                ..Default::default()
            },
            "ui.bufferline" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.bufferline_background),
                fg: to_helix_color(ui_theme.text),
                ..Default::default()
            },
            "ui.bufferline.active" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.bufferline_active),
                fg: to_helix_color(ui_theme.text),
                ..Default::default()
            },
            // Enhanced diagnostic mappings
            "diagnostic.error" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.diagnostic_error),
                ..Default::default()
            },
            "diagnostic.warning" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.diagnostic_warning),
                ..Default::default()
            },
            "diagnostic.info" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.diagnostic_info),
                ..Default::default()
            },
            "diagnostic.hint" => Style {
                fg: to_helix_color(ui_theme.tokens.colors.diagnostic_hint),
                ..Default::default()
            },
            // Diagnostic background mappings (for error/warning underlines and highlights)
            "diagnostic.error.bg" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.diagnostic_error_bg),
                ..Default::default()
            },
            "diagnostic.warning.bg" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.diagnostic_warning_bg),
                ..Default::default()
            },
            "diagnostic.info.bg" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.diagnostic_info_bg),
                ..Default::default()
            },
            "diagnostic.hint.bg" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.diagnostic_hint_bg),
                ..Default::default()
            },
            // Enhanced popup and menu mappings
            "ui.menu.scroll" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.menu_background),
                fg: to_helix_color(ui_theme.text_muted),
                ..Default::default()
            },
            // Focus ring mappings
            "ui.cursor.primary.focus" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.focus_ring),
                ..Default::default()
            },
            // Additional separator mappings
            "ui.background.separator.vertical" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.separator_vertical),
                ..Default::default()
            },
            // Additional buffer line mappings
            "ui.bufferline.inactive" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.bufferline_inactive),
                fg: to_helix_color(ui_theme.text_muted),
                ..Default::default()
            },
            // Menu separator mapping
            "ui.menu.separator" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.menu_separator),
                ..Default::default()
            },
            // Enhanced focus ring variants for accessibility
            "ui.focus" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.focus_ring),
                ..Default::default()
            },
            "ui.focus.error" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.focus_ring_error),
                ..Default::default()
            },
            "ui.focus.warning" => Style {
                bg: to_helix_color(ui_theme.tokens.colors.focus_ring_warning),
                ..Default::default()
            },
            _ => {
                nucleotide_logging::debug!(key = key, "Using fallback style for unknown theme key");
                Style {
                    fg: to_helix_color(ui_theme.text),
                    bg: to_helix_color(ui_theme.background),
                    ..Default::default()
                }
            }
        }
    }

    /// Get the UI theme
    pub fn ui_theme(&self) -> &UITheme {
        &self.ui_theme
    }

    /// Get the current system appearance
    pub fn system_appearance(&self) -> SystemAppearance {
        self.system_appearance
    }

    /// Set the system appearance
    pub fn set_system_appearance(&mut self, appearance: SystemAppearance) {
        self.system_appearance = appearance;
        // Re-derive the UI theme with the new system appearance for proper fallback colors
        self.ui_theme =
            Self::derive_ui_theme_with_appearance(&self.helix_theme, self.system_appearance);
    }

    /// Check if the current theme is dark based on background luminance
    pub fn is_dark_theme(&self) -> bool {
        // HSLA uses lightness directly, so we can check that
        // A theme is considered dark if its background lightness is below 0.5
        let bg = self.ui_theme.background;
        bg.l < 0.5
    }

    /// Derive a UI theme from a Helix theme
    fn derive_ui_theme(helix_theme: &HelixTheme) -> UITheme {
        Self::derive_ui_theme_with_appearance(helix_theme, SystemAppearance::default())
    }

    /// Derive a UI theme from a Helix theme with system appearance for fallback colors
    fn derive_ui_theme_with_appearance(
        helix_theme: &HelixTheme,
        system_appearance: SystemAppearance,
    ) -> UITheme {
        // Check if theme fallback testing is enabled
        let test_fallback = std::env::var("NUCLEOTIDE_DISABLE_THEME_LOADING")
            .map(|val| val == "1" || val.to_lowercase() == "true")
            .unwrap_or(false);

        // Extract colors from Helix theme with fallbacks
        // If testing fallback colors, ignore the theme completely
        let (
            ui_bg,
            ui_text,
            ui_selection,
            ui_cursor,
            ui_cursor_insert,
            ui_cursor_select,
            ui_cursor_match,
            ui_window,
            ui_menu,
            ui_statusline,
            ui_popup,
            error_style,
            warning_style,
            info_style,
        ) = if test_fallback {
            nucleotide_logging::warn!(
                "TESTING MODE: Ignoring all theme colors to force fallback computation"
            );
            // Return empty styles to force all fallbacks
            use helix_view::graphics::Style;
            let empty_style = Style::default();
            (
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
                empty_style,
            )
        } else {
            let ui_bg = helix_theme.get("ui.background");
            let ui_text = helix_theme.get("ui.text");
            let ui_selection = helix_theme.get("ui.selection");
            let ui_cursor = helix_theme.get("ui.cursor.primary");
            let ui_cursor_insert = helix_theme.get("ui.cursor.insert");
            let ui_cursor_select = helix_theme.get("ui.cursor.select");
            let ui_cursor_match = helix_theme.get("ui.cursor.match");
            let ui_window = helix_theme.get("ui.window");
            let ui_menu = helix_theme.get("ui.menu");
            let ui_statusline = helix_theme.get("ui.statusline");
            let ui_popup = helix_theme.get("ui.popup");
            let error_style = helix_theme.get("error");
            let warning_style = helix_theme.get("warning");
            let info_style = helix_theme.get("info");

            (
                ui_bg,
                ui_text,
                ui_selection,
                ui_cursor,
                ui_cursor_insert,
                ui_cursor_select,
                ui_cursor_match,
                ui_window,
                ui_menu,
                ui_statusline,
                ui_popup,
                error_style,
                warning_style,
                info_style,
            )
        };

        if test_fallback {
            nucleotide_logging::info!(
                ui_background_available = ui_bg.bg.is_some(),
                ui_text_available = ui_text.fg.is_some(),
                ui_selection_available = ui_selection.bg.is_some(),
                ui_cursor_available = ui_cursor.bg.is_some(),
                ui_cursor_insert_available = ui_cursor_insert.bg.is_some(),
                ui_cursor_select_available = ui_cursor_select.bg.is_some(),
                ui_cursor_match_available = ui_cursor_match.bg.is_some(),
                ui_window_available = ui_window.fg.is_some(),
                ui_menu_available = ui_menu.bg.is_some(),
                ui_statusline_available = ui_statusline.bg.is_some(),
                ui_popup_available = ui_popup.bg.is_some(),
                error_available = error_style.fg.is_some(),
                warning_available = warning_style.fg.is_some(),
                info_available = info_style.fg.is_some(),
                "Theme color availability analysis (should all be false in test mode)"
            );
        }

        // Convert to GPUI colors with sensible defaults based on system appearance
        let background_from_theme = ui_bg.bg.and_then(color_to_hsla);
        let background = background_from_theme.unwrap_or_else(|| {
            let fallback_color = match system_appearance {
                SystemAppearance::Light => hsla(0.0, 0.0, 0.98, 1.0), // Light background
                SystemAppearance::Dark => hsla(0.0, 0.0, 0.05, 1.0),  // Dark background
            };
            nucleotide_logging::debug!(
                system_appearance = ?system_appearance,
                fallback_background = ?fallback_color,
                "Using fallback background color"
            );
            fallback_color
        });

        let surface_from_theme = {
            let menu_color = ui_menu.bg.and_then(color_to_hsla);
            let bg_color = ui_bg.bg.and_then(color_to_hsla);

            match (menu_color, bg_color) {
                (Some(menu), Some(bg)) => {
                    // If menu is darker than background, compute a lighter surface from background
                    if menu.l < bg.l {
                        nucleotide_logging::debug!(
                            menu_lightness = menu.l,
                            bg_lightness = bg.l,
                            "ui.menu is darker than ui.background, computing surface from background"
                        );
                        Some(hsla(bg.h, bg.s, bg.l + 0.05, bg.a))
                    } else {
                        // Menu is lighter than or equal to background, use it as surface
                        Some(menu)
                    }
                }
                (Some(menu), None) => {
                    // Only menu available, add lightness
                    Some(hsla(menu.h, menu.s, menu.l + 0.05, menu.a))
                }
                (None, Some(bg)) => {
                    // Only background available, add lightness
                    Some(hsla(bg.h, bg.s, bg.l + 0.05, bg.a))
                }
                (None, None) => None,
            }
        };

        let surface = surface_from_theme.unwrap_or_else(|| {
            let fallback_color = match system_appearance {
                SystemAppearance::Light => hsla(0.0, 0.0, 0.95, 1.0), // Light surface
                SystemAppearance::Dark => hsla(0.0, 0.0, 0.1, 1.0),   // Dark surface
            };
            nucleotide_logging::warn!(
                system_appearance = ?system_appearance,
                fallback_surface = ?fallback_color,
                ui_menu_bg_available = ui_menu.bg.is_some(),
                ui_background_available = ui_bg.bg.is_some(),
                "Using fallback surface color - Helix theme may not define ui.menu or ui.background"
            );
            fallback_color
        });

        let text_from_theme = ui_text.fg.and_then(color_to_hsla);
        let text = text_from_theme.unwrap_or_else(|| {
            match system_appearance {
                SystemAppearance::Light => hsla(0.0, 0.0, 0.1, 1.0), // Dark text on light background
                SystemAppearance::Dark => hsla(0.0, 0.0, 0.9, 1.0), // Light text on dark background
            }
        });

        let border_from_theme = ui_window
            .fg
            .and_then(color_to_hsla)
            .or_else(|| ui_text.fg.and_then(color_to_hsla))
            .map(|c| hsla(c.h, c.s * 0.5, c.l * 0.5, c.a * 0.8));
        let border = border_from_theme.unwrap_or_else(|| {
            match system_appearance {
                SystemAppearance::Light => hsla(0.0, 0.0, 0.8, 1.0), // Light border
                SystemAppearance::Dark => hsla(0.0, 0.0, 0.2, 1.0),  // Dark border
            }
        });

        let accent_from_theme = ui_selection
            .bg
            .and_then(color_to_hsla)
            .or_else(|| ui_cursor.bg.and_then(color_to_hsla));
        let accent = accent_from_theme.unwrap_or_else(|| hsla(220.0 / 360.0, 0.6, 0.5, 1.0));

        let error_from_theme = error_style.fg.and_then(color_to_hsla);
        let error = error_from_theme.unwrap_or_else(|| hsla(0.0, 0.8, 0.5, 1.0));

        let warning_from_theme = warning_style.fg.and_then(color_to_hsla);
        let warning = warning_from_theme.unwrap_or_else(|| hsla(40.0 / 360.0, 0.8, 0.5, 1.0));

        let success_from_theme = info_style.fg.and_then(color_to_hsla);
        let success = success_from_theme.unwrap_or_else(|| hsla(120.0 / 360.0, 0.6, 0.5, 1.0));

        // Extract additional cursor colors from Helix theme
        let cursor_insert_from_theme = ui_cursor_insert.bg.and_then(color_to_hsla);
        let cursor_insert = cursor_insert_from_theme.unwrap_or(success); // Fallback to success color

        let cursor_select_from_theme = ui_cursor_select.bg.and_then(color_to_hsla);
        let cursor_select = cursor_select_from_theme.unwrap_or(warning); // Fallback to warning color

        let cursor_match_from_theme = ui_cursor_match.bg.and_then(color_to_hsla);
        let cursor_match = cursor_match_from_theme.unwrap_or(accent); // Fallback to accent color

        // Extract statusline and popup colors
        let statusline_from_theme = ui_statusline.bg.and_then(color_to_hsla);
        let statusline = statusline_from_theme.unwrap_or(surface);

        let popup_from_theme = ui_popup.bg.and_then(color_to_hsla);
        let popup = popup_from_theme.unwrap_or(surface);

        if test_fallback {
            nucleotide_logging::info!(
                background_from_theme = background_from_theme.is_some(),
                surface_from_theme = surface_from_theme.is_some(),
                text_from_theme = text_from_theme.is_some(),
                border_from_theme = border_from_theme.is_some(),
                accent_from_theme = accent_from_theme.is_some(),
                error_from_theme = error_from_theme.is_some(),
                warning_from_theme = warning_from_theme.is_some(),
                success_from_theme = success_from_theme.is_some(),
                background_color = ?background,
                text_color = ?text,
                accent_color = ?accent,
                "Computed theme colors - showing which are fallback vs from theme"
            );
        }

        // Create comprehensive theme colors struct
        let theme_colors = HelixThemeColors {
            selection: accent,
            cursor_normal: accent,
            cursor_insert,
            cursor_select,
            cursor_match,
            error,
            warning,
            success,
            statusline,
            popup,
        };

        let tokens = if background.l < 0.5 {
            crate::DesignTokens::dark_with_helix_colors(theme_colors)
        } else {
            crate::DesignTokens::light_with_helix_colors(theme_colors)
        };

        UITheme {
            background,
            surface,
            surface_background: hsla(surface.h, surface.s, surface.l - 0.02, surface.a),
            surface_hover: hsla(surface.h, surface.s, surface.l + 0.05, surface.a),
            surface_active: hsla(surface.h, surface.s, surface.l + 0.1, surface.a),
            border,
            border_focused: accent,
            text,
            text_muted: hsla(text.h, text.s, text.l * 0.7, text.a),
            text_disabled: hsla(text.h, text.s, text.l * 0.5, text.a),
            accent,
            accent_hover: hsla(accent.h, accent.s, accent.l + 0.1, accent.a),
            accent_active: hsla(accent.h, accent.s, accent.l - 0.1, accent.a),
            error,
            warning,
            success,
            tokens,
        }
    }
}

impl gpui::Global for ThemeManager {}

/// Extension trait for easy Helix theme access from contexts
pub trait HelixThemedContext {
    fn theme_manager(&self) -> &ThemeManager;
    fn helix_theme(&self) -> &HelixTheme;
    fn ui_theme(&self) -> &UITheme;
    /// Get a theme style with testing-aware fallback support
    fn theme_style(&self, key: &str) -> helix_view::graphics::Style;
}

impl HelixThemedContext for gpui::App {
    fn theme_manager(&self) -> &ThemeManager {
        self.global::<ThemeManager>()
    }

    fn helix_theme(&self) -> &HelixTheme {
        &self.global::<ThemeManager>().helix_theme
    }

    fn ui_theme(&self) -> &UITheme {
        &self.global::<ThemeManager>().ui_theme
    }

    fn theme_style(&self, key: &str) -> helix_view::graphics::Style {
        self.global::<ThemeManager>().theme_style(key)
    }
}

impl<V: 'static> HelixThemedContext for gpui::Context<'_, V> {
    fn theme_manager(&self) -> &ThemeManager {
        self.global::<ThemeManager>()
    }

    fn helix_theme(&self) -> &HelixTheme {
        &self.global::<ThemeManager>().helix_theme
    }

    fn ui_theme(&self) -> &UITheme {
        &self.global::<ThemeManager>().ui_theme
    }

    fn theme_style(&self, key: &str) -> helix_view::graphics::Style {
        self.global::<ThemeManager>().theme_style(key)
    }
}

/// Convert HSLA to RGB
fn hsla_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r_prime, g_prime, b_prime) = if h < 1.0 / 6.0 {
        (c, x, 0.0)
    } else if h < 2.0 / 6.0 {
        (x, c, 0.0)
    } else if h < 3.0 / 6.0 {
        (0.0, c, x)
    } else if h < 4.0 / 6.0 {
        (0.0, x, c)
    } else if h < 5.0 / 6.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (r_prime + m, g_prime + m, b_prime + m)
}
