// ABOUTME: Theme provider component for distributing theme state across the component tree
// ABOUTME: Enables theme switching, inheritance, and context-aware styling

use super::{Provider, ProviderContainer, use_provider, use_provider_or_default};
use crate::Theme;
use gpui::{AnyElement, App, IntoElement, SharedString};
use std::collections::HashMap;

/// Theme provider for managing theme state
#[derive(Debug, Clone)]
pub struct ThemeProvider {
    /// Current active theme
    pub current_theme: Theme,
    /// Available themes by name
    pub available_themes: HashMap<SharedString, Theme>,
    /// Theme inheritance chain
    pub theme_inheritance: Vec<SharedString>,
    /// Theme customization overrides
    pub theme_overrides: ThemeOverrides,
    /// Animation settings for theme transitions
    pub transition_config: ThemeTransitionConfig,
}

/// Theme customization overrides
#[derive(Debug, Clone, Default)]
pub struct ThemeOverrides {
    /// Color overrides
    pub color_overrides: HashMap<String, gpui::Hsla>,
    /// Size overrides
    pub size_overrides: HashMap<String, gpui::Pixels>,
    /// Typography overrides
    pub typography_overrides: TypographyOverrides,
}

/// Typography customization
#[derive(Debug, Clone, Default)]
pub struct TypographyOverrides {
    /// Font family overrides
    pub font_family: Option<SharedString>,
    /// Font size scale factor
    pub font_scale: Option<f32>,
    /// Line height overrides
    pub line_height: Option<f32>,
    /// Letter spacing overrides
    pub letter_spacing: Option<gpui::Pixels>,
}

/// Theme transition configuration
#[derive(Debug, Clone)]
pub struct ThemeTransitionConfig {
    /// Enable smooth transitions between themes
    pub enable_transitions: bool,
    /// Transition duration
    pub transition_duration: std::time::Duration,
    /// Properties to animate during transition
    pub animated_properties: Vec<ThemeAnimatedProperty>,
}

/// Properties that can be animated during theme transitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeAnimatedProperty {
    BackgroundColor,
    ForegroundColor,
    BorderColor,
    Opacity,
}

impl Default for ThemeTransitionConfig {
    fn default() -> Self {
        Self {
            enable_transitions: true,
            transition_duration: std::time::Duration::from_millis(200),
            animated_properties: vec![
                ThemeAnimatedProperty::BackgroundColor,
                ThemeAnimatedProperty::ForegroundColor,
                ThemeAnimatedProperty::BorderColor,
            ],
        }
    }
}

/// Typed color override keys for safer updates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorKey {
    Primary,
    TextSecondary,
}

impl ThemeProvider {
    /// Create a new theme provider with a default theme
    pub fn new(default_theme: Theme) -> Self {
        use nucleotide_logging::debug;
        debug!(
            "TITLEBAR THEME_PROVIDER: Creating new ThemeProvider with surface={:?}, background={:?}",
            default_theme.tokens.chrome.surface, default_theme.tokens.editor.background
        );

        let mut available_themes = HashMap::new();
        let theme_name: SharedString = if default_theme.is_dark() {
            "dark".into()
        } else {
            "light".into()
        };

        available_themes.insert(theme_name.clone(), default_theme.clone());

        Self {
            current_theme: default_theme,
            available_themes,
            theme_inheritance: vec![theme_name],
            theme_overrides: ThemeOverrides::default(),
            transition_config: ThemeTransitionConfig::default(),
        }
    }

    /// Create a theme provider with multiple themes
    pub fn with_themes(themes: HashMap<SharedString, Theme>, default_theme_name: &str) -> Self {
        let current_theme = themes
            .get(default_theme_name)
            .cloned()
            .unwrap_or_else(Theme::dark);
        let default_name: SharedString = default_theme_name.to_string().into();

        Self {
            current_theme,
            available_themes: themes,
            theme_inheritance: vec![default_name],
            theme_overrides: ThemeOverrides::default(),
            transition_config: ThemeTransitionConfig::default(),
        }
    }

    /// Switch to a different theme
    pub fn switch_theme(&mut self, theme_name: &str) -> bool {
        if let Some(new_theme) = self.available_themes.get(theme_name).cloned() {
            self.current_theme = self.apply_overrides(new_theme);
            self.theme_inheritance = vec![theme_name.to_string().into()];

            nucleotide_logging::info!(theme_name = theme_name, "Theme switched");
            nucleotide_logging::debug!(
                "TITLEBAR THEME_PROVIDER: Switched to theme '{}' with surface={:?}, background={:?}",
                theme_name,
                self.current_theme.tokens.chrome.surface,
                self.current_theme.tokens.editor.background
            );

            true
        } else {
            nucleotide_logging::warn!(
                theme_name = theme_name,
                available_themes = ?self.available_themes.keys().collect::<Vec<_>>(),
                "Attempted to switch to unknown theme"
            );
            false
        }
    }

    /// Add a new theme
    pub fn add_theme(&mut self, name: impl Into<SharedString>, theme: Theme) {
        let name = name.into();
        self.available_themes.insert(name.clone(), theme);

        nucleotide_logging::debug!(
            theme_name = %name,
            "Added new theme"
        );
    }

    /// Remove a theme
    pub fn remove_theme(&mut self, name: &str) -> bool {
        if self.available_themes.len() <= 1 {
            nucleotide_logging::warn!(theme_name = name, "Cannot remove last available theme");
            return false;
        }

        let removed = self.available_themes.remove(name).is_some();

        // If we removed the current theme, switch to the first available
        if removed && self.theme_inheritance.first().map(|s| s.as_ref()) == Some(name) {
            let first_name = self.available_themes.keys().next().cloned();
            if let Some(first_name) = first_name {
                let first_name_str = first_name.as_ref();
                self.switch_theme(first_name_str);
            }
        }

        removed
    }

    /// Get the current theme
    pub fn current_theme(&self) -> &Theme {
        &self.current_theme
    }

    /// Get all available theme names
    pub fn available_theme_names(&self) -> Vec<&str> {
        self.available_themes.keys().map(|s| s.as_ref()).collect()
    }

    /// Check if a theme exists
    pub fn has_theme(&self, name: &str) -> bool {
        self.available_themes.contains_key(name)
    }

    /// Override a color in the current theme
    pub fn override_color(&mut self, color_key: impl Into<String>, color: gpui::Hsla) {
        self.theme_overrides
            .color_overrides
            .insert(color_key.into(), color);
        self.current_theme = self.apply_overrides(self.current_theme.clone());
    }

    /// Override a size in the current theme
    pub fn override_size(&mut self, size_key: impl Into<String>, size: gpui::Pixels) {
        self.theme_overrides
            .size_overrides
            .insert(size_key.into(), size);
        self.current_theme = self.apply_overrides(self.current_theme.clone());
    }

    /// Override a color using a typed key (preferred)
    pub fn override_color_key(&mut self, key: ColorKey, color: gpui::Hsla) {
        match key {
            ColorKey::Primary => {
                // Update chrome primary and its hover/active variants
                self.current_theme.tokens.chrome.primary = color;
                self.current_theme.tokens.chrome.primary_hover = crate::tokens::lighten(color, 0.1);
                self.current_theme.tokens.chrome.primary_active = crate::tokens::darken(color, 0.1);
            }
            ColorKey::TextSecondary => {
                self.current_theme.tokens.chrome.text_chrome_secondary = color;
            }
        }
    }

    /// Set typography overrides
    pub fn set_typography_overrides(&mut self, overrides: TypographyOverrides) {
        self.theme_overrides.typography_overrides = overrides;
        self.current_theme = self.apply_overrides(self.current_theme.clone());
    }

    /// Clear all overrides
    pub fn clear_overrides(&mut self) {
        self.theme_overrides = ThemeOverrides::default();
        // Reapply the base theme
        if let Some(current_name) = self.theme_inheritance.first()
            && let Some(base_theme) = self.available_themes.get(current_name)
        {
            self.current_theme = base_theme.clone();
        }
    }

    /// Configure theme transitions
    pub fn set_transition_config(&mut self, config: ThemeTransitionConfig) {
        self.transition_config = config;
    }

    /// Apply overrides to a theme
    fn apply_overrides(&self, mut theme: Theme) -> Theme {
        // Apply color overrides
        for (key, color) in &self.theme_overrides.color_overrides {
            match key.as_str() {
                "primary" => {
                    theme.tokens.chrome.primary = *color;
                    theme.tokens.chrome.primary_hover = crate::tokens::lighten(*color, 0.1);
                    theme.tokens.chrome.primary_active = crate::tokens::darken(*color, 0.1);
                }
                "surface" => theme.tokens.chrome.surface = *color,
                "background" => theme.tokens.editor.background = *color,
                "text_primary" => theme.tokens.chrome.text_on_chrome = *color,
                "text_secondary" => theme.tokens.chrome.text_chrome_secondary = *color,
                "border_default" => theme.tokens.chrome.border_default = *color,
                "error" => theme.tokens.editor.error = *color,
                "warning" => theme.tokens.editor.warning = *color,
                "success" => theme.tokens.editor.success = *color,
                _ => {
                    nucleotide_logging::debug!(color_key = key, "Unknown color key in override");
                }
            }
        }

        // Apply size overrides
        for (key, size) in &self.theme_overrides.size_overrides {
            match key.as_str() {
                "space_1" => theme.tokens.sizes.space_1 = *size,
                "space_2" => theme.tokens.sizes.space_2 = *size,
                "space_3" => theme.tokens.sizes.space_3 = *size,
                "space_4" => theme.tokens.sizes.space_4 = *size,
                "radius_sm" => theme.tokens.sizes.radius_sm = *size,
                "radius_md" => theme.tokens.sizes.radius_md = *size,
                "radius_lg" => theme.tokens.sizes.radius_lg = *size,
                _ => {
                    nucleotide_logging::debug!(size_key = key, "Unknown size key in override");
                }
            }
        }

        theme
    }

    /// Get computed styles for theme transitions
    pub fn get_transition_styles(&self) -> ThemeTransitionStyles {
        ThemeTransitionStyles {
            duration: self.transition_config.transition_duration,
            timing_function: if self.transition_config.enable_transitions {
                "ease-out".to_string()
            } else {
                "none".to_string()
            },
            properties: self.transition_config.animated_properties.clone(),
        }
    }

    /// Create a derived theme with modifications
    pub fn derive_theme(
        &self,
        _name: impl Into<SharedString>,
        modifier: impl FnOnce(&mut Theme),
    ) -> Theme {
        let mut derived = self.current_theme.clone();
        modifier(&mut derived);
        derived
    }

    /// Check if the current theme is dark
    pub fn is_dark_theme(&self) -> bool {
        self.current_theme.is_dark()
    }

    /// Toggle between light and dark theme variants
    pub fn toggle_dark_mode(&mut self) -> bool {
        let target_theme = if self.is_dark_theme() {
            "light"
        } else {
            "dark"
        };

        self.switch_theme(target_theme)
    }

    /// Update the current theme with a new theme (typically from ThemeManager)
    pub fn update_theme(&mut self, new_theme: crate::Theme) {
        use nucleotide_logging::debug;
        debug!(
            "TITLEBAR THEME_PROVIDER: Updating current theme with surface={:?}, background={:?}",
            new_theme.tokens.chrome.surface, new_theme.tokens.editor.background
        );

        // Update the current theme
        self.current_theme = new_theme.clone();

        // Update the theme in available_themes for consistency
        let theme_name = if new_theme.is_dark() { "dark" } else { "light" };

        self.available_themes.insert(theme_name.into(), new_theme);
        self.theme_inheritance = vec![theme_name.into()];

        debug!("TITLEBAR THEME_PROVIDER: Theme updated successfully");
    }

    /// Get titlebar tokens for a specific color context
    pub fn titlebar_tokens(
        &self,
        ctx: crate::tokens::ColorContext,
    ) -> crate::tokens::TitleBarTokens {
        use nucleotide_logging::debug;
        debug!(
            "TITLEBAR THEME_PROVIDER: Requested titlebar tokens for context {:?}",
            ctx
        );

        debug!(
            "TITLEBAR THEME_PROVIDER: Current theme surface color: {:?}, background: {:?}",
            self.current_theme.tokens.chrome.surface, self.current_theme.tokens.editor.background
        );

        let tokens = match ctx {
            crate::tokens::ColorContext::OnSurface => {
                // Use the new hybrid token system for surface context
                self.current_theme.tokens.titlebar_tokens()
            }
            crate::tokens::ColorContext::OnPrimary => {
                // Fallback to old system for specialized contexts until migrated
                crate::tokens::TitleBarTokens::on_primary(&self.current_theme.tokens)
            }
            crate::tokens::ColorContext::Floating => {
                // Fallback to old system for specialized contexts until migrated
                crate::tokens::TitleBarTokens::floating(&self.current_theme.tokens)
            }
            crate::tokens::ColorContext::Overlay => {
                // Fallback to old system for specialized contexts until migrated
                crate::tokens::TitleBarTokens::overlay(&self.current_theme.tokens)
            }
        };

        debug!(
            "TITLEBAR THEME_PROVIDER: Returning tokens - bg={:?}, fg={:?}, border={:?}",
            tokens.background, tokens.foreground, tokens.border
        );

        tokens
    }
}

impl Default for ThemeProvider {
    fn default() -> Self {
        Self::new(Theme::dark())
    }
}

/// Theme transition styles for animations
#[derive(Debug, Clone)]
pub struct ThemeTransitionStyles {
    pub duration: std::time::Duration,
    pub timing_function: String,
    pub properties: Vec<ThemeAnimatedProperty>,
}

impl Provider for ThemeProvider {
    fn type_name(&self) -> &'static str {
        "ThemeProvider"
    }

    fn initialize(&mut self, _cx: &mut App) {
        nucleotide_logging::info!(
            current_theme = ?self.theme_inheritance.first(),
            available_themes = ?self.available_theme_names(),
            "ThemeProvider initialized"
        );
    }

    fn cleanup(&mut self, _cx: &mut App) {
        nucleotide_logging::debug!("ThemeProvider cleaned up");
    }
}

/// Create a theme provider component
pub fn theme_provider(provider: ThemeProvider) -> ThemeProviderComponent {
    ThemeProviderComponent::new(provider)
}

/// Theme provider component wrapper
pub struct ThemeProviderComponent {
    provider: ThemeProvider,
    children: Vec<AnyElement>,
}

impl ThemeProviderComponent {
    pub fn new(provider: ThemeProvider) -> Self {
        Self {
            provider,
            children: Vec::new(),
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(|child| child.into_any_element()));
        self
    }
}

impl IntoElement for ThemeProviderComponent {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        ProviderContainer::new("theme-provider", self.provider)
            .children(self.children)
            .into_any_element()
    }
}

/// Hook to use the theme provider
pub fn use_theme() -> Theme {
    use_provider_or_default::<ThemeProvider>().current_theme
}

/// Hook to use the theme provider itself
pub fn use_theme_provider() -> Option<ThemeProvider> {
    use_provider::<ThemeProvider>()
}

/// Hook to check if dark theme is active
pub fn use_is_dark_theme() -> bool {
    use_provider::<ThemeProvider>()
        .map(|provider| provider.is_dark_theme())
        .unwrap_or(false)
}

/// Helper to create common theme configurations
pub struct ThemeConfigurations;

impl ThemeConfigurations {
    /// Create a standard light/dark theme provider
    pub fn light_dark() -> ThemeProvider {
        let mut themes = HashMap::new();
        themes.insert("light".into(), Theme::light());
        themes.insert("dark".into(), Theme::dark());

        ThemeProvider::with_themes(themes, "light")
    }

    /// Create a high contrast theme provider
    pub fn high_contrast() -> ThemeProvider {
        let mut provider = Self::light_dark();

        // Add high contrast variants
        let mut high_contrast_light = Theme::light();
        high_contrast_light.tokens.chrome.text_on_chrome = gpui::Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        };
        high_contrast_light.tokens.editor.background = gpui::Hsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        };

        let mut high_contrast_dark = Theme::dark();
        high_contrast_dark.tokens.chrome.text_on_chrome = gpui::Hsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        };
        high_contrast_dark.tokens.editor.background = gpui::Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        };

        provider.add_theme("high-contrast-light", high_contrast_light);
        provider.add_theme("high-contrast-dark", high_contrast_dark);

        provider
    }

    /// Create a provider with custom brand colors
    pub fn with_brand_colors(
        primary_color: gpui::Hsla,
        secondary_color: gpui::Hsla,
    ) -> ThemeProvider {
        let mut provider = Self::light_dark();

        provider.override_color_key(ColorKey::Primary, primary_color);
        provider.override_color_key(ColorKey::TextSecondary, secondary_color);

        provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_provider_creation() {
        let theme = Theme::dark();
        let provider = ThemeProvider::new(theme.clone());

        assert_eq!(provider.current_theme.is_dark(), theme.is_dark());
        assert_eq!(provider.available_themes.len(), 1);
        assert!(provider.has_theme("dark"));
    }

    #[test]
    fn test_theme_switching() {
        let mut themes = HashMap::new();
        themes.insert("light".into(), Theme::light());
        themes.insert("dark".into(), Theme::dark());

        let mut provider = ThemeProvider::with_themes(themes, "light");

        assert!(!provider.is_dark_theme());

        let switched = provider.switch_theme("dark");
        assert!(switched);
        assert!(provider.is_dark_theme());

        let invalid_switch = provider.switch_theme("nonexistent");
        assert!(!invalid_switch);
        assert!(provider.is_dark_theme()); // Should remain dark
    }

    #[test]
    fn test_theme_addition_and_removal() {
        let mut provider = ThemeProvider::new(Theme::light());

        provider.add_theme("custom", Theme::dark());
        assert!(provider.has_theme("custom"));
        assert_eq!(provider.available_theme_names().len(), 2);

        let removed = provider.remove_theme("custom");
        assert!(removed);
        assert!(!provider.has_theme("custom"));
        assert_eq!(provider.available_theme_names().len(), 1);
    }

    #[test]
    fn test_theme_overrides() {
        let mut provider = ThemeProvider::new(Theme::light());
        let custom_color = gpui::Hsla {
            h: 200.0,
            s: 0.8,
            l: 0.5,
            a: 1.0,
        };

        provider.override_color("primary", custom_color);

        assert_eq!(provider.current_theme().tokens.chrome.primary, custom_color);

        provider.clear_overrides();
        // After clearing overrides, should revert to original theme
        assert_ne!(provider.current_theme().tokens.chrome.primary, custom_color);
    }

    #[test]
    fn test_typography_overrides() {
        let mut provider = ThemeProvider::new(Theme::light());

        let typography_overrides = TypographyOverrides {
            font_family: Some("CustomFont".into()),
            font_scale: Some(1.2),
            line_height: Some(1.5),
            letter_spacing: Some(gpui::px(0.5)),
        };

        provider.set_typography_overrides(typography_overrides.clone());

        assert_eq!(
            provider.theme_overrides.typography_overrides.font_family,
            typography_overrides.font_family
        );
        assert_eq!(
            provider.theme_overrides.typography_overrides.font_scale,
            typography_overrides.font_scale
        );
    }

    #[test]
    fn test_theme_transition_config() {
        let mut provider = ThemeProvider::new(Theme::light());

        let transition_config = ThemeTransitionConfig {
            enable_transitions: false,
            transition_duration: std::time::Duration::from_millis(500),
            animated_properties: vec![ThemeAnimatedProperty::BackgroundColor],
        };

        provider.set_transition_config(transition_config.clone());

        assert_eq!(provider.transition_config.enable_transitions, false);
        assert_eq!(
            provider.transition_config.transition_duration,
            std::time::Duration::from_millis(500)
        );
        assert_eq!(provider.transition_config.animated_properties.len(), 1);
    }

    #[test]
    fn test_dark_mode_toggle() {
        let mut themes = HashMap::new();
        themes.insert("light".into(), Theme::light());
        themes.insert("dark".into(), Theme::dark());

        let mut provider = ThemeProvider::with_themes(themes, "light");

        assert!(!provider.is_dark_theme());

        let toggled = provider.toggle_dark_mode();
        assert!(toggled);
        assert!(provider.is_dark_theme());

        let toggled_back = provider.toggle_dark_mode();
        assert!(toggled_back);
        assert!(!provider.is_dark_theme());
    }

    #[test]
    fn test_theme_configurations() {
        let light_dark_provider = ThemeConfigurations::light_dark();
        assert!(light_dark_provider.has_theme("light"));
        assert!(light_dark_provider.has_theme("dark"));
        assert_eq!(light_dark_provider.available_theme_names().len(), 2);

        let high_contrast_provider = ThemeConfigurations::high_contrast();
        assert!(high_contrast_provider.has_theme("high-contrast-light"));
        assert!(high_contrast_provider.has_theme("high-contrast-dark"));
        assert_eq!(high_contrast_provider.available_theme_names().len(), 4);

        let brand_color = gpui::Hsla {
            h: 220.0,
            s: 0.9,
            l: 0.6,
            a: 1.0,
        };
        let secondary_color = gpui::Hsla {
            h: 180.0,
            s: 0.7,
            l: 0.5,
            a: 1.0,
        };
        let brand_provider = ThemeConfigurations::with_brand_colors(brand_color, secondary_color);

        assert_eq!(
            brand_provider.current_theme().tokens.chrome.primary,
            brand_color
        );
        // Note: "secondary" maps to text_secondary since there's no dedicated secondary color field
        assert_eq!(
            brand_provider
                .current_theme()
                .tokens
                .chrome
                .text_chrome_secondary,
            secondary_color
        );
    }

    #[test]
    fn test_derive_theme() {
        let provider = ThemeProvider::new(Theme::light());

        let derived = provider.derive_theme("custom", |theme| {
            theme.tokens.chrome.primary = gpui::Hsla {
                h: 300.0,
                s: 0.8,
                l: 0.6,
                a: 1.0,
            };
        });

        // Original theme should be unchanged
        assert_ne!(
            provider.current_theme().tokens.chrome.primary,
            derived.tokens.chrome.primary
        );

        // Derived theme should have the modification
        assert_eq!(
            derived.tokens.chrome.primary,
            gpui::Hsla {
                h: 300.0,
                s: 0.8,
                l: 0.6,
                a: 1.0
            }
        );
    }

    #[test]
    fn test_transition_styles() {
        let mut provider = ThemeProvider::new(Theme::light());

        provider.transition_config.enable_transitions = true;
        provider.transition_config.transition_duration = std::time::Duration::from_millis(300);

        let styles = provider.get_transition_styles();
        assert_eq!(styles.duration, std::time::Duration::from_millis(300));
        assert_eq!(styles.timing_function, "ease-out");

        provider.transition_config.enable_transitions = false;
        let styles_disabled = provider.get_transition_styles();
        assert_eq!(styles_disabled.timing_function, "none");
    }
}
