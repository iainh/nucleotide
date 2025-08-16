// ABOUTME: Design token system providing semantic color and spacing values
// ABOUTME: Replaces hardcoded values with systematic, theme-aware design tokens

use gpui::{hsla, px, Hsla, Pixels};

/// Base color palette - raw color definitions
#[derive(Debug, Clone, Copy)]
pub struct BaseColors {
    // Neutral colors
    pub neutral_50: Hsla,
    pub neutral_100: Hsla,
    pub neutral_200: Hsla,
    pub neutral_300: Hsla,
    pub neutral_400: Hsla,
    pub neutral_500: Hsla,
    pub neutral_600: Hsla,
    pub neutral_700: Hsla,
    pub neutral_800: Hsla,
    pub neutral_900: Hsla,
    pub neutral_950: Hsla,

    // Primary colors
    pub primary_50: Hsla,
    pub primary_100: Hsla,
    pub primary_200: Hsla,
    pub primary_300: Hsla,
    pub primary_400: Hsla,
    pub primary_500: Hsla,
    pub primary_600: Hsla,
    pub primary_700: Hsla,
    pub primary_800: Hsla,
    pub primary_900: Hsla,

    // Semantic colors
    pub success_500: Hsla,
    pub warning_500: Hsla,
    pub error_500: Hsla,
    pub info_500: Hsla,
}

impl BaseColors {
    /// Light theme base colors
    pub fn light() -> Self {
        Self {
            // Neutral scale (light theme)
            neutral_50: hsla(0.0, 0.0, 0.98, 1.0),
            neutral_100: hsla(0.0, 0.0, 0.96, 1.0),
            neutral_200: hsla(0.0, 0.0, 0.94, 1.0),
            neutral_300: hsla(0.0, 0.0, 0.91, 1.0),
            neutral_400: hsla(0.0, 0.0, 0.78, 1.0),
            neutral_500: hsla(0.0, 0.0, 0.64, 1.0),
            neutral_600: hsla(0.0, 0.0, 0.52, 1.0),
            neutral_700: hsla(0.0, 0.0, 0.42, 1.0),
            neutral_800: hsla(0.0, 0.0, 0.25, 1.0),
            neutral_900: hsla(0.0, 0.0, 0.15, 1.0),
            neutral_950: hsla(0.0, 0.0, 0.09, 1.0),

            // Primary scale (blue)
            primary_50: hsla(220.0 / 360.0, 0.95, 0.97, 1.0),
            primary_100: hsla(220.0 / 360.0, 0.88, 0.94, 1.0),
            primary_200: hsla(220.0 / 360.0, 0.83, 0.89, 1.0),
            primary_300: hsla(220.0 / 360.0, 0.78, 0.81, 1.0),
            primary_400: hsla(220.0 / 360.0, 0.70, 0.69, 1.0),
            primary_500: hsla(220.0 / 360.0, 0.62, 0.55, 1.0),
            primary_600: hsla(220.0 / 360.0, 0.58, 0.44, 1.0),
            primary_700: hsla(220.0 / 360.0, 0.55, 0.35, 1.0),
            primary_800: hsla(220.0 / 360.0, 0.50, 0.28, 1.0),
            primary_900: hsla(220.0 / 360.0, 0.45, 0.22, 1.0),

            // Semantic colors
            success_500: hsla(120.0 / 360.0, 0.60, 0.50, 1.0),
            warning_500: hsla(40.0 / 360.0, 0.80, 0.50, 1.0),
            error_500: hsla(0.0, 0.80, 0.50, 1.0),
            info_500: hsla(200.0 / 360.0, 0.70, 0.50, 1.0),
        }
    }

    /// Dark theme base colors
    pub fn dark() -> Self {
        Self {
            // Neutral scale (dark theme - inverted)
            neutral_50: hsla(0.0, 0.0, 0.05, 1.0),
            neutral_100: hsla(0.0, 0.0, 0.08, 1.0),
            neutral_200: hsla(0.0, 0.0, 0.12, 1.0),
            neutral_300: hsla(0.0, 0.0, 0.16, 1.0),
            neutral_400: hsla(0.0, 0.0, 0.24, 1.0),
            neutral_500: hsla(0.0, 0.0, 0.38, 1.0),
            neutral_600: hsla(0.0, 0.0, 0.52, 1.0),
            neutral_700: hsla(0.0, 0.0, 0.64, 1.0),
            neutral_800: hsla(0.0, 0.0, 0.78, 1.0),
            neutral_900: hsla(0.0, 0.0, 0.89, 1.0),
            neutral_950: hsla(0.0, 0.0, 0.95, 1.0),

            // Primary scale (same hue, adjusted for dark theme)
            primary_50: hsla(220.0 / 360.0, 0.45, 0.22, 1.0),
            primary_100: hsla(220.0 / 360.0, 0.50, 0.28, 1.0),
            primary_200: hsla(220.0 / 360.0, 0.55, 0.35, 1.0),
            primary_300: hsla(220.0 / 360.0, 0.58, 0.44, 1.0),
            primary_400: hsla(220.0 / 360.0, 0.62, 0.55, 1.0),
            primary_500: hsla(220.0 / 360.0, 0.70, 0.69, 1.0),
            primary_600: hsla(220.0 / 360.0, 0.78, 0.81, 1.0),
            primary_700: hsla(220.0 / 360.0, 0.83, 0.89, 1.0),
            primary_800: hsla(220.0 / 360.0, 0.88, 0.94, 1.0),
            primary_900: hsla(220.0 / 360.0, 0.95, 0.97, 1.0),

            // Semantic colors (slightly brighter for dark themes)
            success_500: hsla(120.0 / 360.0, 0.60, 0.60, 1.0),
            warning_500: hsla(40.0 / 360.0, 0.80, 0.60, 1.0),
            error_500: hsla(0.0, 0.80, 0.60, 1.0),
            info_500: hsla(200.0 / 360.0, 0.70, 0.60, 1.0),
        }
    }
}

/// Semantic color tokens - meaningful names for UI elements
#[derive(Debug, Clone, Copy)]
pub struct SemanticColors {
    // Surface colors
    pub background: Hsla,
    pub surface: Hsla,
    pub surface_elevated: Hsla,
    pub surface_overlay: Hsla,

    // Interactive states
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub surface_selected: Hsla,
    pub surface_disabled: Hsla,

    // Text colors
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_disabled: Hsla,
    pub text_on_primary: Hsla,

    // Border colors
    pub border_default: Hsla,
    pub border_muted: Hsla,
    pub border_strong: Hsla,
    pub border_focus: Hsla,

    // Brand colors
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub primary_active: Hsla,

    // Semantic feedback
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub info: Hsla,
}

impl SemanticColors {
    /// Create semantic colors from base colors for light theme
    pub fn from_base_light(base: &BaseColors) -> Self {
        Self {
            // Surface colors
            background: base.neutral_50,
            surface: base.neutral_100,
            surface_elevated: base.neutral_200,
            surface_overlay: hsla(0.0, 0.0, 1.0, 0.95),

            // Interactive states
            surface_hover: base.neutral_200,
            surface_active: base.neutral_300,
            surface_selected: base.primary_100,
            surface_disabled: base.neutral_100,

            // Text colors
            text_primary: base.neutral_900,
            text_secondary: base.neutral_700,
            text_tertiary: base.neutral_500,
            text_disabled: base.neutral_400,
            text_on_primary: base.neutral_50,

            // Border colors
            border_default: base.neutral_300,
            border_muted: base.neutral_200,
            border_strong: base.neutral_400,
            border_focus: base.primary_500,

            // Brand colors
            primary: base.primary_500,
            primary_hover: base.primary_600,
            primary_active: base.primary_700,

            // Semantic feedback
            success: base.success_500,
            warning: base.warning_500,
            error: base.error_500,
            info: base.info_500,
        }
    }

    /// Create semantic colors from base colors for dark theme
    pub fn from_base_dark(base: &BaseColors) -> Self {
        Self {
            // Surface colors
            background: base.neutral_50,
            surface: base.neutral_100,
            surface_elevated: base.neutral_200,
            surface_overlay: hsla(0.0, 0.0, 0.0, 0.95),

            // Interactive states
            surface_hover: base.neutral_200,
            surface_active: base.neutral_300,
            surface_selected: base.primary_200,
            surface_disabled: base.neutral_100,

            // Text colors
            text_primary: base.neutral_900,
            text_secondary: base.neutral_700,
            text_tertiary: base.neutral_500,
            text_disabled: base.neutral_400,
            text_on_primary: base.neutral_50,

            // Border colors
            border_default: base.neutral_300,
            border_muted: base.neutral_200,
            border_strong: base.neutral_400,
            border_focus: base.primary_500,

            // Brand colors
            primary: base.primary_500,
            primary_hover: base.primary_400,
            primary_active: base.primary_300,

            // Semantic feedback
            success: base.success_500,
            warning: base.warning_500,
            error: base.error_500,
            info: base.info_500,
        }
    }
}

/// Size and spacing tokens
#[derive(Debug, Clone, Copy)]
pub struct SizeTokens {
    // Spacing scale
    pub space_0: Pixels,  // 0px
    pub space_1: Pixels,  // 2px
    pub space_2: Pixels,  // 4px
    pub space_3: Pixels,  // 8px
    pub space_4: Pixels,  // 12px
    pub space_5: Pixels,  // 16px
    pub space_6: Pixels,  // 20px
    pub space_7: Pixels,  // 24px
    pub space_8: Pixels,  // 32px
    pub space_9: Pixels,  // 40px
    pub space_10: Pixels, // 48px

    // Component sizes
    pub button_height_sm: Pixels,
    pub button_height_md: Pixels,
    pub button_height_lg: Pixels,

    // Border radius
    pub radius_sm: Pixels,
    pub radius_md: Pixels,
    pub radius_lg: Pixels,
    pub radius_full: Pixels,

    // Font sizes
    pub text_xs: Pixels,
    pub text_sm: Pixels,
    pub text_md: Pixels,
    pub text_lg: Pixels,
    pub text_xl: Pixels,
}

impl SizeTokens {
    pub fn default() -> Self {
        Self {
            // Spacing scale
            space_0: px(0.0),
            space_1: px(2.0),
            space_2: px(4.0),
            space_3: px(8.0),
            space_4: px(12.0),
            space_5: px(16.0),
            space_6: px(20.0),
            space_7: px(24.0),
            space_8: px(32.0),
            space_9: px(40.0),
            space_10: px(48.0),

            // Component sizes
            button_height_sm: px(28.0),
            button_height_md: px(36.0),
            button_height_lg: px(44.0),

            // Border radius
            radius_sm: px(4.0),
            radius_md: px(6.0),
            radius_lg: px(8.0),
            radius_full: px(9999.0),

            // Font sizes
            text_xs: px(11.0),
            text_sm: px(12.0),
            text_md: px(14.0),
            text_lg: px(16.0),
            text_xl: px(18.0),
        }
    }
}

/// Design tokens combining colors and sizes
#[derive(Debug, Clone, Copy)]
pub struct DesignTokens {
    pub colors: SemanticColors,
    pub sizes: SizeTokens,
}

impl DesignTokens {
    /// Create design tokens for light theme
    pub fn light() -> Self {
        let base_colors = BaseColors::light();
        Self {
            colors: SemanticColors::from_base_light(&base_colors),
            sizes: SizeTokens::default(),
        }
    }

    /// Create design tokens for dark theme
    pub fn dark() -> Self {
        let base_colors = BaseColors::dark();
        Self {
            colors: SemanticColors::from_base_dark(&base_colors),
            sizes: SizeTokens::default(),
        }
    }
}

/// Token utility functions for color manipulation
pub mod utils {
    use super::*;

    /// Create a color with adjusted opacity
    pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
        hsla(color.h, color.s, color.l, alpha)
    }

    /// Create a lighter variant of a color
    pub fn lighten(color: Hsla, amount: f32) -> Hsla {
        hsla(color.h, color.s, color.l + amount, color.a)
    }

    /// Create a darker variant of a color
    pub fn darken(color: Hsla, amount: f32) -> Hsla {
        hsla(color.h, color.s, color.l - amount, color.a)
    }

    /// Interpolate between two colors
    pub fn mix(color1: Hsla, color2: Hsla, ratio: f32) -> Hsla {
        let ratio = ratio.clamp(0.0, 1.0);
        hsla(
            color1.h + (color2.h - color1.h) * ratio,
            color1.s + (color2.s - color1.s) * ratio,
            color1.l + (color2.l - color1.l) * ratio,
            color1.a + (color2.a - color1.a) * ratio,
        )
    }
}

// Re-export commonly used types
pub use utils::*;

/// Backward compatibility - maps to old spacing values
#[deprecated(note = "Use DesignTokens::sizes instead")]
pub mod spacing {
    use super::*;

    pub const XS: Pixels = px(2.0);
    pub const SM: Pixels = px(4.0);
    pub const MD: Pixels = px(8.0);
    pub const LG: Pixels = px(12.0);
}

#[cfg(test)]
mod tests;
