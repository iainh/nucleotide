// ABOUTME: Color theory utilities for intelligent color selection and contrast optimization
// ABOUTME: Provides WCAG-compliant contrast calculations and contextual color awareness

use crate::DesignTokens;
use gpui::{hsla, Hsla};

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
        let r_linear = if r <= 0.03928 { r / 12.92 } else { ((r + 0.055) / 1.055).powf(2.4) };
        let g_linear = if g <= 0.03928 { g / 12.92 } else { ((g + 0.055) / 1.055).powf(2.4) };
        let b_linear = if b <= 0.03928 { b / 12.92 } else { ((b + 0.055) / 1.055).powf(2.4) };
        
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
            Self::generate_high_contrast_text(background)
        } else {
            best_color
        }
    }
    
    /// Generate a high-contrast text color for any background
    fn generate_high_contrast_text(background: Hsla) -> Hsla {
        let bg_luminance = Self::relative_luminance(background);
        
        // Use white text for dark backgrounds, black for light
        if bg_luminance < 0.5 {
            hsla(0.0, 0.0, 0.95, 1.0) // Near white
        } else {
            hsla(0.0, 0.0, 0.05, 1.0) // Near black
        }
    }
    
    /// Get contextually appropriate colors based on surrounding UI
    pub fn contextual_colors(
        variant: &str,
        is_dark_theme: bool,
        context: ColorContext,
        tokens: &DesignTokens,
    ) -> ContextualColors {
        match context {
            ColorContext::OnSurface => Self::on_surface_colors(variant, tokens),
            ColorContext::OnPrimary => Self::on_primary_colors(variant, tokens),
            ColorContext::Floating => Self::floating_colors(variant, is_dark_theme, tokens),
            ColorContext::Overlay => Self::overlay_colors(variant, is_dark_theme, tokens),
        }
    }
    
    /// Colors for elements on surface backgrounds
    fn on_surface_colors(variant: &str, tokens: &DesignTokens) -> ContextualColors {
        let (background, foreground) = match variant {
            "primary" => (tokens.colors.primary, Self::best_text_color(tokens.colors.primary, tokens)),
            "secondary" => (tokens.colors.surface, tokens.colors.text_primary),
            "ghost" => {
                // Ghost variant: transparent background, foreground based on surface
                let bg = hsla(0.0, 0.0, 0.0, 0.0); // Transparent
                let fg = Self::best_text_color(tokens.colors.surface, tokens);
                (bg, fg)
            },
            "danger" => (tokens.colors.error, Self::best_text_color(tokens.colors.error, tokens)),
            "success" => (tokens.colors.success, Self::best_text_color(tokens.colors.success, tokens)),
            "warning" => (tokens.colors.warning, Self::best_text_color(tokens.colors.warning, tokens)),
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
    fn floating_colors(variant: &str, is_dark_theme: bool, tokens: &DesignTokens) -> ContextualColors {
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
            },
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
    fn overlay_colors(variant: &str, is_dark_theme: bool, tokens: &DesignTokens) -> ContextualColors {
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
    fn subtle_border_color(background: Hsla, tokens: &DesignTokens) -> Hsla {
        let bg_luminance = Self::relative_luminance(background);
        
        if bg_luminance > 0.5 {
            // Light background - use darker border
            Self::darken(background, 0.1)
        } else {
            // Dark background - use lighter border
            Self::lighten(background, 0.1)
        }
    }
    
    /// Adjust color for primary context
    fn adjust_for_primary_context(primary: Hsla, tokens: &DesignTokens) -> Hsla {
        // Create a variant that works well on primary background
        Self::mix(primary, tokens.colors.surface, 0.15)
    }
    
    /// Create a lighter variant of a color
    fn lighten(color: Hsla, amount: f32) -> Hsla {
        hsla(color.h, color.s, (color.l + amount).clamp(0.0, 1.0), color.a)
    }
    
    /// Create a darker variant of a color
    fn darken(color: Hsla, amount: f32) -> Hsla {
        hsla(color.h, color.s, (color.l - amount).clamp(0.0, 1.0), color.a)
    }
    
    /// Mix two colors
    fn mix(color1: Hsla, color2: Hsla, ratio: f32) -> Hsla {
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
        
        let (r, g, b) = if h < 1.0/6.0 {
            (c, x, 0.0)
        } else if h < 2.0/6.0 {
            (x, c, 0.0)
        } else if h < 3.0/6.0 {
            (0.0, c, x)
        } else if h < 4.0/6.0 {
            (0.0, x, c)
        } else if h < 5.0/6.0 {
            (x, 0.0, c)
        } else {
            (c, 0.0, x)
        };
        
        (r + m, g + m, b + m)
    }
}

/// UI context for color selection
#[derive(Debug, Clone, Copy)]
pub enum ColorContext {
    /// Element sits on a surface background
    OnSurface,
    /// Element sits on a primary color background
    OnPrimary,
    /// Floating element (modal, popup)
    Floating,
    /// Overlay element
    Overlay,
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
}