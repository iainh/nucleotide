// ABOUTME: Style variant system for consistent component styling
// ABOUTME: Provides variant definitions and style computations

use crate::{DesignTokens, Theme};
use gpui::{Hsla, Pixels, px};

/// Standard component variants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
    Success,
    Warning,
    Info,
    Accent,
}

impl StyleVariant {
    /// Convert variant to string identifier
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Ghost => "ghost",
            Self::Danger => "danger",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Accent => "accent",
        }
    }

    /// Get the semantic role of this variant
    pub fn semantic_role(self) -> VariantRole {
        match self {
            Self::Primary | Self::Accent => VariantRole::Primary,
            Self::Secondary => VariantRole::Secondary,
            Self::Ghost => VariantRole::Subtle,
            Self::Danger => VariantRole::Destructive,
            Self::Success => VariantRole::Positive,
            Self::Warning => VariantRole::Warning,
            Self::Info => VariantRole::Informational,
        }
    }

    /// Check if this variant should have strong visual emphasis
    pub fn is_emphasis(self) -> bool {
        matches!(
            self,
            Self::Primary | Self::Danger | Self::Success | Self::Accent
        )
    }

    /// Get all available variants
    pub fn all() -> &'static [StyleVariant] {
        &[
            Self::Primary,
            Self::Secondary,
            Self::Ghost,
            Self::Danger,
            Self::Success,
            Self::Warning,
            Self::Info,
            Self::Accent,
        ]
    }
}

/// Semantic roles for variants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantRole {
    Primary,
    Secondary,
    Subtle,
    Destructive,
    Positive,
    Warning,
    Informational,
}

/// Style size variants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleSize {
    ExtraSmall,
    Small,
    Medium,
    Large,
    ExtraLarge,
}

impl StyleSize {
    /// Convert size to string identifier
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExtraSmall => "xs",
            Self::Small => "sm",
            Self::Medium => "md",
            Self::Large => "lg",
            Self::ExtraLarge => "xl",
        }
    }

    /// Get the scale factor for this size
    pub fn scale_factor(self) -> f32 {
        match self {
            Self::ExtraSmall => 0.75,
            Self::Small => 0.875,
            Self::Medium => 1.0,
            Self::Large => 1.125,
            Self::ExtraLarge => 1.25,
        }
    }

    /// Get padding values for this size
    pub fn padding(self, tokens: &DesignTokens) -> (Pixels, Pixels) {
        match self {
            Self::ExtraSmall => (tokens.sizes.space_1, tokens.sizes.space_0),
            Self::Small => (tokens.sizes.space_2, tokens.sizes.space_1),
            Self::Medium => (tokens.sizes.space_3, tokens.sizes.space_2),
            Self::Large => (tokens.sizes.space_4, tokens.sizes.space_3),
            Self::ExtraLarge => (tokens.sizes.space_5, tokens.sizes.space_4),
        }
    }

    /// Get font size for this size
    pub fn font_size(self, base_size: Pixels) -> Pixels {
        let scale = self.scale_factor();
        px(base_size.0 * scale)
    }

    /// Get border radius for this size
    pub fn border_radius(self, tokens: &DesignTokens) -> Pixels {
        match self {
            Self::ExtraSmall => tokens.sizes.radius_sm,
            Self::Small => tokens.sizes.radius_sm,
            Self::Medium => tokens.sizes.radius_md,
            Self::Large => tokens.sizes.radius_lg,
            Self::ExtraLarge => tokens.sizes.radius_lg,
        }
    }

    /// Get icon size for this size variant
    pub fn icon_size(self) -> Pixels {
        match self {
            Self::ExtraSmall => px(10.0),
            Self::Small => px(12.0),
            Self::Medium => px(16.0),
            Self::Large => px(20.0),
            Self::ExtraLarge => px(24.0),
        }
    }
}

/// Variant colors for different states
#[derive(Debug, Clone)]
pub struct VariantColors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
    pub hover_background: Hsla,
    pub active_background: Hsla,
}

impl VariantColors {
    /// Get variant colors for a specific variant and theme
    pub fn for_variant(variant: StyleVariant, theme: &Theme) -> Self {
        let tokens = &theme.tokens;

        match variant {
            StyleVariant::Primary => Self {
                background: tokens.colors.primary,
                foreground: tokens.colors.text_on_primary,
                border: tokens.colors.primary,
                hover_background: tokens.colors.primary_hover,
                active_background: tokens.colors.primary_active,
            },
            StyleVariant::Secondary => Self {
                background: tokens.colors.surface,
                foreground: tokens.colors.text_primary,
                border: tokens.colors.border_default,
                hover_background: tokens.colors.surface_hover,
                active_background: tokens.colors.surface_active,
            },
            StyleVariant::Ghost => Self {
                background: Hsla::transparent_black(),
                foreground: tokens.colors.text_primary,
                border: Hsla::transparent_black(),
                hover_background: tokens.colors.surface_hover,
                active_background: tokens.colors.surface_active,
            },
            StyleVariant::Danger => Self {
                background: tokens.colors.error,
                foreground: tokens.colors.text_on_primary,
                border: tokens.colors.error,
                hover_background: tokens.colors.primary_hover, // Use similar color
                active_background: tokens.colors.primary_active,
            },
            StyleVariant::Success => Self {
                background: tokens.colors.success,
                foreground: tokens.colors.text_on_primary,
                border: tokens.colors.success,
                hover_background: tokens.colors.primary_hover,
                active_background: tokens.colors.primary_active,
            },
            StyleVariant::Warning => Self {
                background: tokens.colors.warning,
                foreground: tokens.colors.text_primary,
                border: tokens.colors.warning,
                hover_background: tokens.colors.primary_hover,
                active_background: tokens.colors.primary_active,
            },
            StyleVariant::Info => Self {
                background: tokens.colors.info,
                foreground: tokens.colors.text_on_primary,
                border: tokens.colors.info,
                hover_background: tokens.colors.primary_hover,
                active_background: tokens.colors.primary_active,
            },
            StyleVariant::Accent => Self {
                background: tokens.colors.primary, // Use primary as accent
                foreground: tokens.colors.text_on_primary,
                border: tokens.colors.primary,
                hover_background: tokens.colors.primary_hover,
                active_background: tokens.colors.primary_active,
            },
        }
    }

    /// Get outline variant (border-only) colors
    pub fn outline_variant(variant: StyleVariant, theme: &Theme) -> Self {
        let base_colors = Self::for_variant(variant, theme);

        Self {
            background: Hsla::transparent_black(),
            foreground: base_colors.background, // Use original background as foreground
            border: base_colors.background,
            hover_background: {
                let bg = base_colors.background;
                Hsla { a: 0.1, ..bg } // Semi-transparent background on hover
            },
            active_background: {
                let bg = base_colors.background;
                Hsla { a: 0.2, ..bg } // More opaque on active
            },
        }
    }

    /// Get soft variant (muted) colors
    pub fn soft_variant(variant: StyleVariant, theme: &Theme) -> Self {
        let base_colors = Self::for_variant(variant, theme);
        let _tokens = &theme.tokens;

        Self {
            background: {
                let bg = base_colors.background;
                if theme.is_dark() {
                    Hsla { a: 0.2, ..bg }
                } else {
                    Hsla { a: 0.1, ..bg }
                }
            },
            foreground: base_colors.background, // Use variant color as text
            border: Hsla::transparent_black(),
            hover_background: {
                let bg = base_colors.background;
                if theme.is_dark() {
                    Hsla { a: 0.3, ..bg }
                } else {
                    Hsla { a: 0.15, ..bg }
                }
            },
            active_background: {
                let bg = base_colors.background;
                if theme.is_dark() {
                    Hsla { a: 0.4, ..bg }
                } else {
                    Hsla { a: 0.2, ..bg }
                }
            },
        }
    }
}

/// Style computation utilities for variants
pub struct VariantStyler;

impl VariantStyler {
    /// Compute variant-specific styles
    pub fn compute_variant_style(
        variant: StyleVariant,
        size: StyleSize,
        theme: &Theme,
    ) -> VariantStyle {
        let colors = VariantColors::for_variant(variant, theme);
        let tokens = &theme.tokens;
        let (padding_x, padding_y) = size.padding(tokens);

        VariantStyle {
            variant,
            size,
            colors,
            padding_x,
            padding_y,
            font_size: size.font_size(px(14.0)),
            border_radius: size.border_radius(tokens),
            border_width: if variant == StyleVariant::Secondary {
                px(1.0)
            } else {
                px(0.0)
            },
        }
    }

    /// Compute outline variant styles
    pub fn compute_outline_style(
        variant: StyleVariant,
        size: StyleSize,
        theme: &Theme,
    ) -> VariantStyle {
        let colors = VariantColors::outline_variant(variant, theme);
        let tokens = &theme.tokens;
        let (padding_x, padding_y) = size.padding(tokens);

        VariantStyle {
            variant,
            size,
            colors,
            padding_x,
            padding_y,
            font_size: size.font_size(px(14.0)),
            border_radius: size.border_radius(tokens),
            border_width: px(1.0), // Always has border for outline
        }
    }

    /// Compute soft variant styles
    pub fn compute_soft_style(
        variant: StyleVariant,
        size: StyleSize,
        theme: &Theme,
    ) -> VariantStyle {
        let colors = VariantColors::soft_variant(variant, theme);
        let tokens = &theme.tokens;
        let (padding_x, padding_y) = size.padding(tokens);

        VariantStyle {
            variant,
            size,
            colors,
            padding_x,
            padding_y,
            font_size: size.font_size(px(14.0)),
            border_radius: size.border_radius(tokens),
            border_width: px(0.0),
        }
    }
}

/// Complete variant style definition
#[derive(Debug, Clone)]
pub struct VariantStyle {
    pub variant: StyleVariant,
    pub size: StyleSize,
    pub colors: VariantColors,
    pub padding_x: Pixels,
    pub padding_y: Pixels,
    pub font_size: Pixels,
    pub border_radius: Pixels,
    pub border_width: Pixels,
}
