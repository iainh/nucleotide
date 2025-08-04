// ABOUTME: Unified theme management for consistent styling across the application
// ABOUTME: Bridges between Helix themes and GPUI styling requirements

use gpui::*;
use helix_view::Theme as HelixTheme;
use crate::ui::Theme as UITheme;
use crate::utils::color_to_hsla;

/// Manages theme state and provides consistent access to theme colors
#[derive(Clone)]
pub struct ThemeManager {
    /// The current Helix theme
    helix_theme: HelixTheme,
    /// Cached UI theme derived from the Helix theme
    ui_theme: UITheme,
}

impl ThemeManager {
    /// Create a new ThemeManager from a Helix theme
    pub fn new(helix_theme: HelixTheme) -> Self {
        let ui_theme = Self::derive_ui_theme(&helix_theme);
        Self {
            helix_theme,
            ui_theme,
        }
    }
    
    /// Update the theme
    pub fn set_theme(&mut self, helix_theme: HelixTheme) {
        self.ui_theme = Self::derive_ui_theme(&helix_theme);
        self.helix_theme = helix_theme;
    }
    
    /// Get the current Helix theme
    pub fn helix_theme(&self) -> &HelixTheme {
        &self.helix_theme
    }
    
    /// Get the UI theme
    pub fn ui_theme(&self) -> &UITheme {
        &self.ui_theme
    }
    
    /// Derive a UI theme from a Helix theme
    fn derive_ui_theme(helix_theme: &HelixTheme) -> UITheme {
        // Extract colors from Helix theme with fallbacks
        let ui_bg = helix_theme.get("ui.background");
        let ui_text = helix_theme.get("ui.text");
        let ui_selection = helix_theme.get("ui.selection");
        let ui_cursor = helix_theme.get("ui.cursor.primary");
        let ui_window = helix_theme.get("ui.window");
        let ui_menu = helix_theme.get("ui.menu");
        let error_style = helix_theme.get("error");
        let warning_style = helix_theme.get("warning");
        let info_style = helix_theme.get("info");
        
        // Convert to GPUI colors with sensible defaults
        let background = ui_bg.bg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.05, 1.0));
            
        let surface = ui_menu.bg
            .and_then(color_to_hsla)
            .or_else(|| ui_bg.bg.and_then(color_to_hsla))
            .map(|c| hsla(c.h, c.s, c.l + 0.05, c.a))
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.1, 1.0));
            
        let text = ui_text.fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.9, 1.0));
            
        let border = ui_window.fg
            .and_then(color_to_hsla)
            .or_else(|| ui_text.fg.and_then(color_to_hsla))
            .map(|c| hsla(c.h, c.s, c.l * 0.3, c.a))
            .unwrap_or_else(|| hsla(0.0, 0.0, 0.2, 1.0));
            
        let accent = ui_selection.bg
            .and_then(color_to_hsla)
            .or_else(|| ui_cursor.bg.and_then(color_to_hsla))
            .unwrap_or_else(|| hsla(220.0 / 360.0, 0.6, 0.5, 1.0));
            
        let error = error_style.fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(0.0, 0.8, 0.5, 1.0));
            
        let warning = warning_style.fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(40.0 / 360.0, 0.8, 0.5, 1.0));
            
        let success = info_style.fg
            .and_then(color_to_hsla)
            .unwrap_or_else(|| hsla(120.0 / 360.0, 0.6, 0.5, 1.0));
        
        UITheme {
            background,
            surface,
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
        }
    }
}

impl gpui::Global for ThemeManager {}

/// Extension trait for easy theme access from contexts
pub trait ThemedContext {
    fn theme_manager(&self) -> &ThemeManager;
    fn helix_theme(&self) -> &HelixTheme;
    fn ui_theme(&self) -> &UITheme;
}

impl ThemedContext for gpui::App {
    fn theme_manager(&self) -> &ThemeManager {
        self.global::<ThemeManager>()
    }
    
    fn helix_theme(&self) -> &HelixTheme {
        &self.global::<ThemeManager>().helix_theme
    }
    
    fn ui_theme(&self) -> &UITheme {
        &self.global::<ThemeManager>().ui_theme
    }
}

impl<V: 'static> ThemedContext for gpui::Context<'_, V> {
    fn theme_manager(&self) -> &ThemeManager {
        self.global::<ThemeManager>()
    }
    
    fn helix_theme(&self) -> &HelixTheme {
        &self.global::<ThemeManager>().helix_theme
    }
    
    fn ui_theme(&self) -> &UITheme {
        &self.global::<ThemeManager>().ui_theme
    }
}