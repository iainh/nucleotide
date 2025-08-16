// ABOUTME: Advanced styling system for nucleotide-ui components
// ABOUTME: Provides style computation, variants, responsive design, and animations

use crate::{Theme, DesignTokens};
use gpui::{Hsla, Pixels, px};
use std::time::Duration;

pub mod variants;
pub mod responsive;
pub mod animations;
pub mod combinations;

pub use variants::*;
pub use responsive::*;
pub use animations::*;
pub use combinations::*;

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
}

impl<'a> StyleContext<'a> {
    /// Create a new style context
    pub fn new(
        theme: &'a Theme, 
        state: StyleState, 
        variant: &'a str, 
        size: &'a str
    ) -> Self {
        Self {
            theme,
            tokens: &theme.tokens,
            state,
            variant,
            size,
            is_dark_theme: theme.is_dark(),
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
    
    /// Apply variant-specific styles
    pub fn apply_variant_styles(&self, mut style: ComputedStyle) -> ComputedStyle {
        match self.variant {
            "primary" => {
                style.background = self.tokens.colors.primary;
                style.foreground = self.tokens.colors.text_on_primary;
                style.border_color = self.tokens.colors.primary;
            }
            "secondary" => {
                style.background = self.tokens.colors.surface;
                style.foreground = self.tokens.colors.text_primary;
                style.border_color = self.tokens.colors.border_default;
                style.border_width = px(1.0);
            }
            "ghost" => {
                style.background = Hsla::transparent_black();
                style.foreground = self.tokens.colors.text_primary;
                style.border_color = Hsla::transparent_black();
            }
            "danger" => {
                style.background = self.tokens.colors.error;
                style.foreground = self.tokens.colors.text_on_primary;
                style.border_color = self.tokens.colors.error;
            }
            "success" => {
                style.background = self.tokens.colors.success;
                style.foreground = self.tokens.colors.text_on_primary;
                style.border_color = self.tokens.colors.success;
            }
            "warning" => {
                style.background = self.tokens.colors.warning;
                style.foreground = self.tokens.colors.text_primary;
                style.border_color = self.tokens.colors.warning;
            }
            _ => {
                // Default variant styling already applied in base
            }
        }
        
        style
    }
    
    /// Apply state-specific styles
    pub fn apply_state_styles(&self, mut style: ComputedStyle) -> ComputedStyle {
        match self.state {
            StyleState::Hover => {
                style.background = match self.variant {
                    "primary" => self.tokens.colors.primary_hover,
                    "secondary" => self.tokens.colors.surface_hover,
                    "ghost" => self.tokens.colors.surface_hover,
                    "danger" => self.tokens.colors.primary_hover,  // Use primary hover as fallback
                    "success" => self.tokens.colors.primary_hover,
                    "warning" => self.tokens.colors.primary_hover,
                    _ => self.tokens.colors.surface_hover,
                };
            }
            StyleState::Active => {
                style.background = match self.variant {
                    "primary" => self.tokens.colors.primary_active,
                    "secondary" => self.tokens.colors.surface_active,
                    "ghost" => self.tokens.colors.surface_active,
                    "danger" => self.tokens.colors.primary_active,
                    "success" => self.tokens.colors.primary_active,
                    "warning" => self.tokens.colors.primary_active,
                    _ => self.tokens.colors.surface_active,
                };
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
                style.background = self.tokens.colors.primary;
                style.foreground = self.tokens.colors.text_on_primary;
                style.border_color = self.tokens.colors.primary;
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