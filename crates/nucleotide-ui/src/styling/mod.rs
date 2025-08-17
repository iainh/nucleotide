// ABOUTME: Advanced styling system for nucleotide-ui components
// ABOUTME: Provides style computation, variants, responsive design, and animations

use crate::{DesignTokens, Theme};
use gpui::{px, Hsla, Pixels};
use std::time::Duration;

pub mod animations;
pub mod color_theory;
pub mod combinations;
pub mod responsive;
pub mod variants;

pub use animations::*;
pub use color_theory::*;
pub use combinations::*;
pub use responsive::*;
pub use variants::*;

/// Component style state for style computation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StyleState {
    Default,
    Hover,
    Active,
    Focused,
    Disabled,
    Loading,
    Selected,
}

impl StyleState {
    /// Check if this state allows interaction
    pub fn is_interactive(self) -> bool {
        !matches!(self, Self::Disabled | Self::Loading)
    }

    /// Check if this state indicates user interaction
    pub fn is_user_interaction(self) -> bool {
        matches!(self, Self::Hover | Self::Active | Self::Focused)
    }

    /// Get the priority of this state for style resolution
    pub fn priority(self) -> u8 {
        match self {
            Self::Disabled => 10,
            Self::Loading => 9,
            Self::Active => 8,
            Self::Focused => 7,
            Self::Selected => 6,
            Self::Hover => 5,
            Self::Default => 0,
        }
    }
}

/// Computed style values for a component
#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border_color: Hsla,
    pub border_width: Pixels,
    pub border_radius: Pixels,
    pub padding_x: Pixels,
    pub padding_y: Pixels,
    pub font_size: Pixels,
    pub font_weight: u16,
    pub opacity: f32,
    pub shadow: Option<BoxShadow>,
    pub transition: Option<Transition>,
}

/// Box shadow configuration
#[derive(Debug, Clone)]
pub struct BoxShadow {
    pub offset_x: Pixels,
    pub offset_y: Pixels,
    pub blur_radius: Pixels,
    pub spread_radius: Pixels,
    pub color: Hsla,
}

/// Animation transition configuration
#[derive(Debug, Clone)]
pub struct Transition {
    pub duration: Duration,
    pub timing_function: TimingFunction,
    pub properties: Vec<TransitionProperty>,
}

/// Timing functions for animations
#[derive(Debug, Clone, Copy)]
pub enum TimingFunction {
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    Linear,
    Custom { x1: f32, y1: f32, x2: f32, y2: f32 },
}

/// Properties that can be animated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionProperty {
    Background,
    Foreground,
    BorderColor,
    Opacity,
    Transform,
    All,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        Self {
            background: Hsla::default(),
            foreground: Hsla::default(),
            border_color: Hsla::default(),
            border_width: px(0.0),
            border_radius: px(0.0),
            padding_x: px(0.0),
            padding_y: px(0.0),
            font_size: px(14.0),
            font_weight: 400,
            opacity: 1.0,
            shadow: None,
            transition: None,
        }
    }
}

/// Style computation context
#[derive(Debug)]
pub struct StyleContext<'a> {
    pub theme: &'a Theme,
    pub tokens: &'a DesignTokens,
    pub state: StyleState,
    pub variant: &'a str,
    pub size: &'a str,
    pub is_dark_theme: bool,
    pub color_context: crate::tokens::ColorContext,
}

impl<'a> StyleContext<'a> {
    /// Create a new style context
    pub fn new(theme: &'a Theme, state: StyleState, variant: &'a str, size: &'a str) -> Self {
        Self {
            theme,
            tokens: &theme.tokens,
            state,
            variant,
            size,
            is_dark_theme: theme.is_dark(),
            color_context: crate::tokens::ColorContext::OnSurface, // Default context
        }
    }

    /// Create a style context with specific color context
    pub fn with_context(
        theme: &'a Theme,
        state: StyleState,
        variant: &'a str,
        size: &'a str,
        context: crate::tokens::ColorContext,
    ) -> Self {
        Self {
            theme,
            tokens: &theme.tokens,
            state,
            variant,
            size,
            is_dark_theme: theme.is_dark(),
            color_context: context,
        }
    }

    /// Compute the base style for this context
    pub fn compute_base_style(&self) -> ComputedStyle {
        let mut style = ComputedStyle::default();

        // Apply base colors from tokens
        style.background = self.tokens.colors.surface;
        style.foreground = self.tokens.colors.text_primary;
        style.border_color = self.tokens.colors.border_default;

        // Apply size-based properties
        match self.size {
            "small" => {
                style.padding_x = self.tokens.sizes.space_2;
                style.padding_y = self.tokens.sizes.space_1;
                style.font_size = px(12.0);
                style.border_radius = self.tokens.sizes.radius_sm;
            }
            "medium" => {
                style.padding_x = self.tokens.sizes.space_3;
                style.padding_y = self.tokens.sizes.space_2;
                style.font_size = px(14.0);
                style.border_radius = self.tokens.sizes.radius_md;
            }
            "large" => {
                style.padding_x = self.tokens.sizes.space_4;
                style.padding_y = self.tokens.sizes.space_3;
                style.font_size = px(16.0);
                style.border_radius = self.tokens.sizes.radius_lg;
            }
            _ => {
                // Default to medium
                style.padding_x = self.tokens.sizes.space_3;
                style.padding_y = self.tokens.sizes.space_2;
                style.font_size = px(14.0);
                style.border_radius = self.tokens.sizes.radius_md;
            }
        }

        style
    }

    /// Apply variant-specific styles using color theory
    pub fn apply_variant_styles(&self, mut style: ComputedStyle) -> ComputedStyle {
        // Use color theory to get contextually appropriate colors
        let contextual_colors = ColorTheory::contextual_colors(
            self.variant,
            self.is_dark_theme,
            self.color_context,
            self.tokens,
        );

        // Apply the contextual colors
        style.background = contextual_colors.background;
        style.foreground = contextual_colors.foreground;
        style.border_color = contextual_colors.border;

        // Set border width and shadow for non-ghost variants
        if self.variant != "ghost" {
            style.border_width = px(1.0);

            // Add subtle shadow for depth and visual hierarchy
            style.shadow = Some(BoxShadow {
                offset_x: px(0.0),
                offset_y: px(1.0),
                blur_radius: px(1.5), // Reduced blur radius for crisper borders
                spread_radius: px(0.0),
                color: if self.is_dark_theme {
                    // For dark themes, use a darker shadow
                    gpui::hsla(0.0, 0.0, 0.0, 0.25)
                } else {
                    // For light themes, use a lighter shadow
                    gpui::hsla(0.0, 0.0, 0.0, 0.1)
                },
            });
        }

        style
    }

    /// Apply state-specific styles with intelligent color selection
    pub fn apply_state_styles(&self, mut style: ComputedStyle) -> ComputedStyle {
        match self.state {
            StyleState::Hover => {
                // Create hover state by intelligently modifying current background
                style.background = self.create_hover_color(style.background);
                // Ensure text contrast is maintained
                style.foreground = ColorTheory::best_text_color(style.background, self.tokens);

                // Enhance shadow on hover for non-ghost variants
                if self.variant != "ghost" {
                    style.shadow = Some(BoxShadow {
                        offset_x: px(0.0),
                        offset_y: px(2.0),
                        blur_radius: px(2.5), // Reduced blur radius for crisper borders
                        spread_radius: px(0.0),
                        color: if self.is_dark_theme {
                            gpui::hsla(0.0, 0.0, 0.0, 0.3)
                        } else {
                            gpui::hsla(0.0, 0.0, 0.0, 0.15)
                        },
                    });
                }
            }
            StyleState::Active => {
                // Create active state by further darkening/lightening
                style.background = self.create_active_color(style.background);
                style.foreground = ColorTheory::best_text_color(style.background, self.tokens);

                // Reduce shadow on active for pressed feeling
                if self.variant != "ghost" {
                    style.shadow = Some(BoxShadow {
                        offset_x: px(0.0),
                        offset_y: px(0.5),
                        blur_radius: px(1.0), // Minimal blur for active state
                        spread_radius: px(0.0),
                        color: if self.is_dark_theme {
                            gpui::hsla(0.0, 0.0, 0.0, 0.4)
                        } else {
                            gpui::hsla(0.0, 0.0, 0.0, 0.2)
                        },
                    });
                }
            }
            StyleState::Focused => {
                style.border_color = self.tokens.colors.border_focus;
                style.border_width = px(2.0);

                // Add focus ring shadow
                style.shadow = Some(BoxShadow {
                    offset_x: px(0.0),
                    offset_y: px(0.0),
                    blur_radius: px(0.0),
                    spread_radius: px(2.0),
                    color: {
                        let focus_color = self.tokens.colors.border_focus;
                        Hsla {
                            h: focus_color.h,
                            s: focus_color.s,
                            l: focus_color.l,
                            a: 0.2,
                        }
                    },
                });
            }
            StyleState::Disabled => {
                style.background = self.tokens.colors.surface_disabled;
                style.foreground = self.tokens.colors.text_disabled;
                style.border_color = self.tokens.colors.border_muted;
                style.opacity = 0.6;
            }
            StyleState::Loading => {
                style.opacity = 0.8;
                // Loading styles could include animations
            }
            StyleState::Selected => {
                // Use primary selection color for active selection
                style.background = self.tokens.colors.selection_primary;
                style.foreground = self.tokens.colors.text_primary;
                style.border_color = self.tokens.colors.selection_primary;
            }
            StyleState::Default => {
                // Default state already handled in base and variant styles
            }
        }

        style
    }

    /// Apply animations if enabled
    pub fn apply_animations(&self, mut style: ComputedStyle) -> ComputedStyle {
        // Only add animations if not disabled or loading (performance)
        if self.state.is_interactive() {
            style.transition = Some(Transition {
                duration: Duration::from_millis(150),
                timing_function: TimingFunction::EaseOut,
                properties: vec![
                    TransitionProperty::Background,
                    TransitionProperty::BorderColor,
                    TransitionProperty::Opacity,
                ],
            });
        }

        style
    }

    /// Compute the complete style for this context
    pub fn compute_style(&self) -> ComputedStyle {
        let base_style = self.compute_base_style();
        let variant_style = self.apply_variant_styles(base_style);
        let state_style = self.apply_state_styles(variant_style);
        let animated_style = self.apply_animations(state_style);

        animated_style
    }

    /// Create an appropriate hover color based on background
    fn create_hover_color(&self, background: Hsla) -> Hsla {
        use gpui::hsla;

        // For ghost variant, use secondary selection color for better list selection UX
        if self.variant == "ghost" {
            return self.tokens.colors.selection_secondary;
        }

        // For transparent backgrounds, use surface hover
        if background.a < 0.1 {
            return self.tokens.colors.surface_hover;
        }

        let luminance = ColorTheory::relative_luminance(background);

        // Lighten dark colors, darken light colors for hover
        if luminance < 0.5 {
            // Dark background - lighten
            hsla(
                background.h,
                background.s,
                (background.l + 0.08).min(1.0),
                background.a,
            )
        } else {
            // Light background - darken
            hsla(
                background.h,
                background.s,
                (background.l - 0.08).max(0.0),
                background.a,
            )
        }
    }

    /// Create an appropriate active color based on background
    fn create_active_color(&self, background: Hsla) -> Hsla {
        use gpui::hsla;

        // For ghost variant, create a more pronounced overlay
        if self.variant == "ghost" {
            return if self.is_dark_theme {
                // Brighter overlay for dark themes
                hsla(0.0, 0.0, 1.0, 0.15)
            } else {
                // Darker overlay for light themes
                hsla(0.0, 0.0, 0.0, 0.15)
            };
        }

        // For transparent backgrounds, use surface active
        if background.a < 0.1 {
            return self.tokens.colors.surface_active;
        }

        let luminance = ColorTheory::relative_luminance(background);

        // More pronounced change for active state
        if luminance < 0.5 {
            // Dark background - lighten more
            hsla(
                background.h,
                background.s,
                (background.l + 0.12).min(1.0),
                background.a,
            )
        } else {
            // Light background - darken more
            hsla(
                background.h,
                background.s,
                (background.l - 0.12).max(0.0),
                background.a,
            )
        }
    }
}

/// Utility function to compute component styles
pub fn compute_component_style(
    theme: &Theme,
    state: StyleState,
    variant: &str,
    size: &str,
) -> ComputedStyle {
    let context = StyleContext::new(theme, state, variant, size);
    context.compute_style()
}

/// Utility function to compute component styles with specific color context
pub fn compute_contextual_style(
    theme: &Theme,
    state: StyleState,
    variant: &str,
    size: &str,
    color_context: crate::tokens::ColorContext,
) -> ComputedStyle {
    let context = StyleContext::with_context(theme, state, variant, size, color_context);
    context.compute_style()
}

/// Style computation for lists of states (priority-based resolution)
pub fn compute_style_for_states(
    theme: &Theme,
    states: &[StyleState],
    variant: &str,
    size: &str,
) -> ComputedStyle {
    // Find the highest priority state
    let primary_state = states
        .iter()
        .max_by_key(|state| state.priority())
        .copied()
        .unwrap_or(StyleState::Default);

    compute_component_style(theme, primary_state, variant, size)
}

/// Check if animations should be enabled based on context
pub fn should_enable_animations(_theme: &Theme, state: StyleState) -> bool {
    // Don't animate disabled or loading states
    if !state.is_interactive() {
        return false;
    }

    // Check if animations are enabled in the theme/config
    // This would integrate with the UIFeatures from our initialization system
    true // For now, always enable animations when interactive
}
