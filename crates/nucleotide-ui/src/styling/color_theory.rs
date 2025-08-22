// ABOUTME: Color theory utilities for intelligent color selection and contrast optimization
// ABOUTME: Provides WCAG-compliant contrast calculations and contextual color awareness

use crate::DesignTokens;
use gpui::{Hsla, hsla};

/// WCAG contrast ratios for accessibility compliance
pub struct ContrastRatios;

impl ContrastRatios {
    pub const AAA_NORMAL: f32 = 7.0;
    pub const AA_NORMAL: f32 = 4.5;
    pub const AA_LARGE: f32 = 3.0;
    pub const MIN_READABLE: f32 = 2.0;
}

/// Color theory utilities for intelligent styling
pub struct ColorTheory;

impl ColorTheory {
    /// Calculate relative luminance for contrast calculations
    /// Based on WCAG 2.1 specification
    pub fn relative_luminance(color: Hsla) -> f32 {
        let (r, g, b) = Self::hsl_to_rgb(color.h, color.s, color.l);

        // Convert to linear RGB
        let r_linear = if r <= 0.03928 {
            r / 12.92
        } else {
            ((r + 0.055) / 1.055).powf(2.4)
        };
        let g_linear = if g <= 0.03928 {
            g / 12.92
        } else {
            ((g + 0.055) / 1.055).powf(2.4)
        };
        let b_linear = if b <= 0.03928 {
            b / 12.92
        } else {
            ((b + 0.055) / 1.055).powf(2.4)
        };

        // Calculate luminance
        0.2126 * r_linear + 0.7152 * g_linear + 0.0722 * b_linear
    }

    /// Calculate contrast ratio between two colors
    pub fn contrast_ratio(color1: Hsla, color2: Hsla) -> f32 {
        let lum1 = Self::relative_luminance(color1);
        let lum2 = Self::relative_luminance(color2);

        let lighter = lum1.max(lum2);
        let darker = lum1.min(lum2);

        (lighter + 0.05) / (darker + 0.05)
    }

    /// Find the best text color for a given background
    pub fn best_text_color(background: Hsla, tokens: &DesignTokens) -> Hsla {
        // For transparent backgrounds, use primary text
        if background.a < 0.1 {
            return tokens.colors.text_primary;
        }

        let candidates = [
            tokens.colors.text_primary,
            tokens.colors.text_secondary,
            tokens.colors.text_on_primary,
            hsla(0.0, 0.0, 0.95, 1.0), // Near white
            hsla(0.0, 0.0, 0.05, 1.0), // Near black
        ];

        let mut best_color = tokens.colors.text_primary;
        let mut best_contrast = 0.0;

        for &candidate in &candidates {
            let contrast = Self::contrast_ratio(background, candidate);
            if contrast > best_contrast {
                best_contrast = contrast;
                best_color = candidate;
            }
        }

        // If still not good enough, generate a high-contrast color
        if best_contrast < ContrastRatios::AA_NORMAL {
            let high_contrast = Self::generate_high_contrast_text(background);
            let high_contrast_ratio = Self::contrast_ratio(background, high_contrast);

            // Use the generated high-contrast color if it's better
            if high_contrast_ratio > best_contrast {
                high_contrast
            } else {
                best_color
            }
        } else {
            best_color
        }
    }

    /// Generate a high-contrast text color for any background
    fn generate_high_contrast_text(background: Hsla) -> Hsla {
        // Try pure white and pure black for maximum contrast
        let pure_white = hsla(0.0, 0.0, 1.0, 1.0);
        let pure_black = hsla(0.0, 0.0, 0.0, 1.0);

        let white_contrast = Self::contrast_ratio(background, pure_white);
        let black_contrast = Self::contrast_ratio(background, pure_black);

        // Choose the option with better contrast
        if white_contrast > black_contrast {
            pure_white
        } else {
            pure_black
        }
    }

    /// Get contextually appropriate colors based on surrounding UI
    pub fn contextual_colors(
        variant: &str,
        is_dark_theme: bool,
        context: crate::tokens::ColorContext,
        tokens: &DesignTokens,
    ) -> ContextualColors {
        match context {
            crate::tokens::ColorContext::OnSurface => Self::on_surface_colors(variant, tokens),
            crate::tokens::ColorContext::OnPrimary => Self::on_primary_colors(variant, tokens),
            crate::tokens::ColorContext::Floating => {
                Self::floating_colors(variant, is_dark_theme, tokens)
            }
            crate::tokens::ColorContext::Overlay => {
                Self::overlay_colors(variant, is_dark_theme, tokens)
            }
        }
    }

    /// Colors for elements on surface backgrounds
    fn on_surface_colors(variant: &str, tokens: &DesignTokens) -> ContextualColors {
        let (background, foreground) = match variant {
            "primary" => (
                tokens.colors.primary,
                Self::best_text_color(tokens.colors.primary, tokens),
            ),
            "secondary" => (tokens.colors.surface, tokens.colors.text_primary),
            "ghost" => {
                // Ghost variant: transparent background, foreground based on surface
                let bg = hsla(0.0, 0.0, 0.0, 0.0); // Transparent
                let fg = Self::best_text_color(tokens.colors.surface, tokens);
                (bg, fg)
            }
            "danger" => (
                tokens.colors.error,
                Self::best_text_color(tokens.colors.error, tokens),
            ),
            "success" => (
                tokens.colors.success,
                Self::best_text_color(tokens.colors.success, tokens),
            ),
            "warning" => (
                tokens.colors.warning,
                Self::best_text_color(tokens.colors.warning, tokens),
            ),
            _ => (tokens.colors.surface, tokens.colors.text_primary),
        };

        let border = if variant == "ghost" {
            hsla(0.0, 0.0, 0.0, 0.0) // Transparent border for ghost
        } else {
            Self::subtle_border_color(background, tokens)
        };

        ContextualColors {
            background,
            foreground,
            border,
        }
    }

    /// Colors for elements on primary backgrounds
    fn on_primary_colors(variant: &str, tokens: &DesignTokens) -> ContextualColors {
        let background = match variant {
            "ghost" => hsla(0.0, 0.0, 1.0, 0.1), // Subtle overlay
            _ => Self::adjust_for_primary_context(tokens.colors.primary, tokens),
        };

        ContextualColors {
            background,
            foreground: tokens.colors.text_on_primary,
            border: Self::subtle_border_color(background, tokens),
        }
    }

    /// Colors for floating elements (modals, popups)
    fn floating_colors(
        variant: &str,
        is_dark_theme: bool,
        tokens: &DesignTokens,
    ) -> ContextualColors {
        let base_bg = if is_dark_theme {
            tokens.colors.surface_elevated
        } else {
            tokens.colors.surface_overlay
        };

        let (background, foreground) = match variant {
            "ghost" => {
                // Ghost variant: transparent background, foreground based on floating surface
                let bg = hsla(0.0, 0.0, 0.0, 0.0); // Transparent
                let fg = Self::best_text_color(base_bg, tokens);
                (bg, fg)
            }
            _ => {
                let bg = base_bg;
                let fg = Self::best_text_color(bg, tokens);
                (bg, fg)
            }
        };

        let border = if variant == "ghost" {
            hsla(0.0, 0.0, 0.0, 0.0) // Transparent border for ghost
        } else {
            tokens.colors.border_default
        };

        ContextualColors {
            background,
            foreground,
            border,
        }
    }

    /// Colors for overlay elements
    fn overlay_colors(
        variant: &str,
        is_dark_theme: bool,
        tokens: &DesignTokens,
    ) -> ContextualColors {
        let background = match variant {
            "primary" => tokens.colors.primary,
            "ghost" => hsla(0.0, 0.0, 0.0, 0.0),
            _ => {
                if is_dark_theme {
                    hsla(0.0, 0.0, 0.1, 0.95) // Dark overlay
                } else {
                    hsla(0.0, 0.0, 0.95, 0.95) // Light overlay
                }
            }
        };

        ContextualColors {
            background,
            foreground: Self::best_text_color(background, tokens),
            border: tokens.colors.border_default,
        }
    }

    /// Create a subtle border color that works with the background
    pub fn subtle_border_color(background: Hsla, _tokens: &DesignTokens) -> Hsla {
        let bg_luminance = Self::relative_luminance(background);

        // Aim for at least 3:1 contrast ratio for borders
        let mut border_color = if bg_luminance > 0.5 {
            // Light background - use darker border
            Self::darken(background, 0.15)
        } else {
            // Dark background - use lighter border
            Self::lighten(background, 0.15)
        };

        // Check contrast and adjust if needed
        let mut contrast = Self::contrast_ratio(background, border_color);
        let mut adjustment = 0.05;

        while contrast < 3.0 && adjustment < 0.5 {
            border_color = if bg_luminance > 0.5 {
                Self::darken(background, 0.15 + adjustment)
            } else {
                Self::lighten(background, 0.15 + adjustment)
            };
            contrast = Self::contrast_ratio(background, border_color);
            adjustment += 0.05;
        }

        border_color
    }

    /// Adjust color for primary context
    fn adjust_for_primary_context(primary: Hsla, tokens: &DesignTokens) -> Hsla {
        // Create a variant that works well on primary background
        Self::mix(primary, tokens.colors.surface, 0.15)
    }

    /// Create a lighter variant of a color
    pub fn lighten(color: Hsla, amount: f32) -> Hsla {
        hsla(
            color.h,
            color.s,
            (color.l + amount).clamp(0.0, 1.0),
            color.a,
        )
    }

    /// Create a darker variant of a color
    pub fn darken(color: Hsla, amount: f32) -> Hsla {
        hsla(
            color.h,
            color.s,
            (color.l - amount).clamp(0.0, 1.0),
            color.a,
        )
    }

    /// Mix two colors
    pub fn mix(color1: Hsla, color2: Hsla, ratio: f32) -> Hsla {
        let ratio = ratio.clamp(0.0, 1.0);
        hsla(
            color1.h + (color2.h - color1.h) * ratio,
            color1.s + (color2.s - color1.s) * ratio,
            color1.l + (color2.l - color1.l) * ratio,
            color1.a + (color2.a - color1.a) * ratio,
        )
    }

    /// Convert HSL to RGB
    fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
        let m = l - c / 2.0;

        let (r, g, b) = if h < 1.0 / 6.0 {
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

        (r + m, g + m, b + m)
    }

    /// Derive chrome colors from a surface color for UI components
    /// Returns computed colors based on surface brightness with WCAG contrast validation
    pub fn derive_chrome_colors(surface_color: Hsla) -> ChromeColors {
        let is_dark_theme = surface_color.l < 0.5;
        
        nucleotide_logging::debug!(
            surface_color = ?surface_color,
            surface_lightness = surface_color.l,
            is_dark_theme = is_dark_theme,
            "Computing chrome colors from surface color"
        );

        // Compute titlebar and footer backgrounds (darker/lighter than surface)
        let titlebar_background = if is_dark_theme {
            // Dark theme: make titlebar lighter than surface
            Self::lighten(surface_color, 0.12)
        } else {
            // Light theme: make titlebar darker than surface
            Self::darken(surface_color, 0.12)
        };

        // Footer uses same approach as titlebar for consistency
        let footer_background = titlebar_background;

        // File tree and tab backgrounds: subtle variation from surface
        let file_tree_background = if is_dark_theme {
            // Dark theme: slightly lighter than surface
            Self::lighten(surface_color, 0.05)
        } else {
            // Light theme: slightly darker than surface
            Self::darken(surface_color, 0.05)
        };

        // Tab empty areas use same approach as file tree
        let tab_empty_background = file_tree_background;

        // Compute separator color with proper contrast
        let separator_color = if is_dark_theme {
            // Dark theme: lighter separator with reduced saturation
            hsla(
                surface_color.h,
                surface_color.s * 0.3,
                (surface_color.l + 0.15).min(1.0),
                0.8
            )
        } else {
            // Light theme: darker separator with reduced saturation
            hsla(
                surface_color.h,
                surface_color.s * 0.3,
                (surface_color.l - 0.15).max(0.0),
                0.8
            )
        };

        let chrome_colors = ChromeColors {
            titlebar_background,
            footer_background,
            file_tree_background,
            tab_empty_background,
            separator_color,
        };

        // Validate contrast ratios for accessibility
        Self::validate_chrome_colors(&chrome_colors, surface_color);

        nucleotide_logging::info!(
            titlebar_bg = ?chrome_colors.titlebar_background,
            file_tree_bg = ?chrome_colors.file_tree_background,
            separator = ?chrome_colors.separator_color,
            theme_type = if is_dark_theme { "dark" } else { "light" },
            "Chrome colors computed successfully"
        );

        chrome_colors
    }

    /// Validate that chrome colors meet accessibility standards
    fn validate_chrome_colors(chrome_colors: &ChromeColors, surface_color: Hsla) {
        // Chrome backgrounds need lower contrast than text (1.2:1 minimum for visual distinction)
        const CHROME_MIN_CONTRAST: f32 = 1.2;

        let validations = [
            ("titlebar_background", chrome_colors.titlebar_background),
            ("footer_background", chrome_colors.footer_background),
            ("file_tree_background", chrome_colors.file_tree_background),
            ("tab_empty_background", chrome_colors.tab_empty_background),
        ];

        for (name, color) in &validations {
            let contrast = Self::contrast_ratio(surface_color, *color);
            
            if contrast < CHROME_MIN_CONTRAST {
                nucleotide_logging::warn!(
                    color_name = name,
                    contrast_ratio = contrast,
                    min_required = CHROME_MIN_CONTRAST,
                    surface_color = ?surface_color,
                    chrome_color = ?color,
                    "Chrome color contrast below minimum visual distinction threshold"
                );
            } else {
                nucleotide_logging::debug!(
                    color_name = name,
                    contrast_ratio = contrast,
                    "Chrome color contrast validation passed"
                );
            }
        }
    }
}

/// Computed chrome colors for UI components based on surface color
#[derive(Debug, Clone, Copy)]
pub struct ChromeColors {
    /// Titlebar background color (darker/lighter than surface)
    pub titlebar_background: Hsla,
    /// Footer background color (consistent with titlebar)
    pub footer_background: Hsla,
    /// File tree background color (subtle variation from surface)
    pub file_tree_background: Hsla,
    /// Tab bar empty areas background (consistent with file tree)
    pub tab_empty_background: Hsla,
    /// Separator color with proper contrast
    pub separator_color: Hsla,
}

impl ChromeColors {
    /// Create ChromeColors with all colors set to a single color (for testing)
    pub fn uniform(color: Hsla) -> Self {
        Self {
            titlebar_background: color,
            footer_background: color,
            file_tree_background: color,
            tab_empty_background: color,
            separator_color: color,
        }
    }
}

/// Contextually computed colors
#[derive(Debug, Clone, Copy)]
pub struct ContextualColors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DesignTokens;

    #[test]
    fn test_contrast_calculation() {
        let white = hsla(0.0, 0.0, 1.0, 1.0);
        let black = hsla(0.0, 0.0, 0.0, 1.0);

        let contrast = ColorTheory::contrast_ratio(white, black);
        assert!(contrast > 20.0); // Should be ~21:1
    }

    #[test]
    fn test_best_text_color() {
        let tokens = DesignTokens::light();

        // Test on light background
        let light_bg = hsla(0.0, 0.0, 0.9, 1.0);
        let text_color = ColorTheory::best_text_color(light_bg, &tokens);
        let contrast = ColorTheory::contrast_ratio(light_bg, text_color);
        assert!(contrast >= ContrastRatios::AA_NORMAL);

        // Test on dark background
        let dark_bg = hsla(0.0, 0.0, 0.1, 1.0);
        let text_color = ColorTheory::best_text_color(dark_bg, &tokens);
        let contrast = ColorTheory::contrast_ratio(dark_bg, text_color);
        assert!(contrast >= ContrastRatios::AA_NORMAL);
    }

    #[test]
    fn test_chrome_colors_light_theme() {
        let light_surface = hsla(0.0, 0.0, 0.95, 1.0); // Light surface
        let chrome_colors = ColorTheory::derive_chrome_colors(light_surface);

        // In light theme, chrome colors should be darker than surface
        assert!(chrome_colors.titlebar_background.l < light_surface.l);
        assert!(chrome_colors.footer_background.l < light_surface.l);
        assert!(chrome_colors.file_tree_background.l < light_surface.l);
        assert!(chrome_colors.tab_empty_background.l < light_surface.l);

        // Titlebar and footer should be consistent
        assert_eq!(chrome_colors.titlebar_background, chrome_colors.footer_background);
        
        // File tree and tabs should be consistent
        assert_eq!(chrome_colors.file_tree_background, chrome_colors.tab_empty_background);

        // Validate some contrast ratios - chrome colors need less contrast than text
        let titlebar_contrast = ColorTheory::contrast_ratio(light_surface, chrome_colors.titlebar_background);
        // Chrome backgrounds need at least 1.2:1 contrast to be visually distinguishable
        assert!(titlebar_contrast >= 1.2);
    }

    #[test]
    fn test_chrome_colors_dark_theme() {
        let dark_surface = hsla(0.0, 0.0, 0.1, 1.0); // Dark surface
        let chrome_colors = ColorTheory::derive_chrome_colors(dark_surface);

        // In dark theme, chrome colors should be lighter than surface
        assert!(chrome_colors.titlebar_background.l > dark_surface.l);
        assert!(chrome_colors.footer_background.l > dark_surface.l);
        assert!(chrome_colors.file_tree_background.l > dark_surface.l);
        assert!(chrome_colors.tab_empty_background.l > dark_surface.l);

        // Titlebar and footer should be consistent
        assert_eq!(chrome_colors.titlebar_background, chrome_colors.footer_background);
        
        // File tree and tabs should be consistent
        assert_eq!(chrome_colors.file_tree_background, chrome_colors.tab_empty_background);

        // Validate some contrast ratios - chrome colors need less contrast than text  
        let titlebar_contrast = ColorTheory::contrast_ratio(dark_surface, chrome_colors.titlebar_background);
        // Chrome backgrounds need at least 1.2:1 contrast to be visually distinguishable
        assert!(titlebar_contrast >= 1.2);
    }

    #[test]
    fn test_chrome_colors_preserve_hue() {
        // Test with a blue surface to ensure hue is preserved
        let blue_surface = hsla(240.0 / 360.0, 0.3, 0.5, 1.0); // Blue surface
        let chrome_colors = ColorTheory::derive_chrome_colors(blue_surface);

        // All chrome colors should preserve the blue hue
        assert!((chrome_colors.titlebar_background.h - blue_surface.h).abs() < 0.01);
        assert!((chrome_colors.file_tree_background.h - blue_surface.h).abs() < 0.01);
        
        // Separator may have different hue due to saturation reduction, but should be similar
        let hue_diff = (chrome_colors.separator_color.h - blue_surface.h).abs();
        assert!(hue_diff < 0.1 || hue_diff > (1.0 - 0.1)); // Allow for hue wrap-around
    }

    #[test]
    fn test_chrome_colors_uniform() {
        let test_color = hsla(120.0 / 360.0, 0.5, 0.6, 1.0);
        let uniform_colors = ChromeColors::uniform(test_color);

        assert_eq!(uniform_colors.titlebar_background, test_color);
        assert_eq!(uniform_colors.footer_background, test_color);
        assert_eq!(uniform_colors.file_tree_background, test_color);
        assert_eq!(uniform_colors.tab_empty_background, test_color);
        assert_eq!(uniform_colors.separator_color, test_color);
    }
}
