// ABOUTME: Animation and transition system for nucleotide-ui components
// ABOUTME: Provides timing functions, presets, and animation utilities

use std::time::Duration;
// use gpui::{Hsla, px, Pixels};

/// Animation timing functions (easing curves)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimingFunction {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    EaseInSine,
    EaseOutSine,
    EaseInOutSine,
    EaseInQuad,
    EaseOutQuad,
    EaseInOutQuad,
    EaseInCubic,
    EaseOutCubic,
    EaseInOutCubic,
    EaseInQuart,
    EaseOutQuart,
    EaseInOutQuart,
    EaseInQuint,
    EaseOutQuint,
    EaseInOutQuint,
    EaseInExpo,
    EaseOutExpo,
    EaseInOutExpo,
    EaseInCirc,
    EaseOutCirc,
    EaseInOutCirc,
    EaseInBack,
    EaseOutBack,
    EaseInOutBack,
    Custom { x1: f32, y1: f32, x2: f32, y2: f32 },
}

impl TimingFunction {
    /// Get the cubic-bezier control points for this timing function
    pub fn control_points(self) -> (f32, f32, f32, f32) {
        match self {
            Self::Linear => (0.0, 0.0, 1.0, 1.0),
            Self::Ease => (0.25, 0.1, 0.25, 1.0),
            Self::EaseIn => (0.42, 0.0, 1.0, 1.0),
            Self::EaseOut => (0.0, 0.0, 0.58, 1.0),
            Self::EaseInOut => (0.42, 0.0, 0.58, 1.0),
            Self::EaseInSine => (0.12, 0.0, 0.39, 0.0),
            Self::EaseOutSine => (0.61, 1.0, 0.88, 1.0),
            Self::EaseInOutSine => (0.37, 0.0, 0.63, 1.0),
            Self::EaseInQuad => (0.11, 0.0, 0.5, 0.0),
            Self::EaseOutQuad => (0.5, 1.0, 0.89, 1.0),
            Self::EaseInOutQuad => (0.45, 0.0, 0.55, 1.0),
            Self::EaseInCubic => (0.32, 0.0, 0.67, 0.0),
            Self::EaseOutCubic => (0.33, 1.0, 0.68, 1.0),
            Self::EaseInOutCubic => (0.65, 0.0, 0.35, 1.0),
            Self::EaseInQuart => (0.5, 0.0, 0.75, 0.0),
            Self::EaseOutQuart => (0.25, 1.0, 0.5, 1.0),
            Self::EaseInOutQuart => (0.76, 0.0, 0.24, 1.0),
            Self::EaseInQuint => (0.64, 0.0, 0.78, 0.0),
            Self::EaseOutQuint => (0.22, 1.0, 0.36, 1.0),
            Self::EaseInOutQuint => (0.83, 0.0, 0.17, 1.0),
            Self::EaseInExpo => (0.7, 0.0, 0.84, 0.0),
            Self::EaseOutExpo => (0.16, 1.0, 0.3, 1.0),
            Self::EaseInOutExpo => (0.87, 0.0, 0.13, 1.0),
            Self::EaseInCirc => (0.55, 0.0, 1.0, 0.45),
            Self::EaseOutCirc => (0.0, 0.55, 0.45, 1.0),
            Self::EaseInOutCirc => (0.85, 0.0, 0.15, 1.0),
            Self::EaseInBack => (0.36, 0.0, 0.66, -0.56),
            Self::EaseOutBack => (0.34, 1.56, 0.64, 1.0),
            Self::EaseInOutBack => (0.68, -0.6, 0.32, 1.6),
            Self::Custom { x1, y1, x2, y2 } => (x1, y1, x2, y2),
        }
    }

    /// Get a CSS cubic-bezier string representation
    pub fn to_css_string(self) -> String {
        let (x1, y1, x2, y2) = self.control_points();
        format!("cubic-bezier({}, {}, {}, {})", x1, y1, x2, y2)
    }

    /// Check if this is a fast timing function (good for micro-interactions)
    pub fn is_fast(self) -> bool {
        matches!(self, Self::EaseOut | Self::EaseOutQuad | Self::EaseOutCubic)
    }

    /// Check if this is a slow timing function (good for major transitions)
    pub fn is_slow(self) -> bool {
        matches!(
            self,
            Self::EaseInOut | Self::EaseInOutQuad | Self::EaseInOutCubic
        )
    }
}

/// Animation duration presets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationDuration {
    Instant,     // 0ms - for immediate changes
    Fastest,     // 50ms - for micro-interactions
    Fast,        // 100ms - for quick feedback
    Normal,      // 150ms - default for most interactions
    Slow,        // 300ms - for noticeable transitions
    Slower,      // 500ms - for major state changes
    Slowest,     // 800ms - for dramatic effects
    Custom(u64), // Custom milliseconds
}

impl AnimationDuration {
    /// Get the duration in milliseconds
    pub fn as_millis(self) -> u64 {
        match self {
            Self::Instant => 0,
            Self::Fastest => 50,
            Self::Fast => 100,
            Self::Normal => 150,
            Self::Slow => 300,
            Self::Slower => 500,
            Self::Slowest => 800,
            Self::Custom(ms) => ms,
        }
    }

    /// Get the duration as a Duration
    pub fn as_duration(self) -> Duration {
        Duration::from_millis(self.as_millis())
    }

    /// Check if this is suitable for micro-interactions
    pub fn is_micro(self) -> bool {
        self.as_millis() <= 100
    }

    /// Check if this is suitable for major transitions
    pub fn is_major(self) -> bool {
        self.as_millis() >= 300
    }
}

/// Animation presets for common use cases
#[derive(Debug, Clone)]
pub struct AnimationPreset {
    pub duration: AnimationDuration,
    pub timing_function: TimingFunction,
    pub properties: Vec<AnimationProperty>,
}

/// Properties that can be animated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationProperty {
    Background,
    Foreground,
    BorderColor,
    BorderWidth,
    BorderRadius,
    Opacity,
    Transform,
    BoxShadow,
    All,
}

impl AnimationProperty {
    /// Get CSS property name
    pub fn css_name(self) -> &'static str {
        match self {
            Self::Background => "background-color",
            Self::Foreground => "color",
            Self::BorderColor => "border-color",
            Self::BorderWidth => "border-width",
            Self::BorderRadius => "border-radius",
            Self::Opacity => "opacity",
            Self::Transform => "transform",
            Self::BoxShadow => "box-shadow",
            Self::All => "all",
        }
    }
}

impl AnimationPreset {
    /// Button hover animation
    pub fn button_hover() -> Self {
        Self {
            duration: AnimationDuration::Fast,
            timing_function: TimingFunction::EaseOut,
            properties: vec![
                AnimationProperty::Background,
                AnimationProperty::BorderColor,
            ],
        }
    }

    /// Button active/press animation
    pub fn button_active() -> Self {
        Self {
            duration: AnimationDuration::Fastest,
            timing_function: TimingFunction::EaseOut,
            properties: vec![AnimationProperty::Background, AnimationProperty::Transform],
        }
    }

    /// Focus ring animation
    pub fn focus_ring() -> Self {
        Self {
            duration: AnimationDuration::Normal,
            timing_function: TimingFunction::EaseOut,
            properties: vec![AnimationProperty::BorderColor, AnimationProperty::BoxShadow],
        }
    }

    /// Modal/overlay enter animation
    pub fn modal_enter() -> Self {
        Self {
            duration: AnimationDuration::Slow,
            timing_function: TimingFunction::EaseOutCubic,
            properties: vec![AnimationProperty::Opacity, AnimationProperty::Transform],
        }
    }

    /// Modal/overlay exit animation
    pub fn modal_exit() -> Self {
        Self {
            duration: AnimationDuration::Normal,
            timing_function: TimingFunction::EaseInCubic,
            properties: vec![AnimationProperty::Opacity, AnimationProperty::Transform],
        }
    }

    /// List item hover animation
    pub fn list_item_hover() -> Self {
        Self {
            duration: AnimationDuration::Fast,
            timing_function: TimingFunction::EaseOut,
            properties: vec![AnimationProperty::Background],
        }
    }

    /// Loading state animation
    pub fn loading_state() -> Self {
        Self {
            duration: AnimationDuration::Normal,
            timing_function: TimingFunction::EaseInOut,
            properties: vec![AnimationProperty::Opacity],
        }
    }

    /// Notification slide-in animation
    pub fn notification_enter() -> Self {
        Self {
            duration: AnimationDuration::Slow,
            timing_function: TimingFunction::EaseOutBack,
            properties: vec![AnimationProperty::Transform, AnimationProperty::Opacity],
        }
    }

    /// Notification slide-out animation
    pub fn notification_exit() -> Self {
        Self {
            duration: AnimationDuration::Normal,
            timing_function: TimingFunction::EaseInBack,
            properties: vec![AnimationProperty::Transform, AnimationProperty::Opacity],
        }
    }

    /// Micro-interaction (subtle feedback)
    pub fn micro_interaction() -> Self {
        Self {
            duration: AnimationDuration::Fastest,
            timing_function: TimingFunction::EaseOut,
            properties: vec![AnimationProperty::Transform],
        }
    }
}

/// Animation configuration for components
#[derive(Debug, Clone)]
pub struct AnimationConfig {
    pub enabled: bool,
    pub reduce_motion: bool,
    pub hover_animations: bool,
    pub focus_animations: bool,
    pub state_animations: bool,
    pub micro_animations: bool,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reduce_motion: false, // Should be set from system preferences
            hover_animations: true,
            focus_animations: true,
            state_animations: true,
            micro_animations: true,
        }
    }
}

impl AnimationConfig {
    /// Create config with all animations disabled
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            reduce_motion: true,
            hover_animations: false,
            focus_animations: false,
            state_animations: false,
            micro_animations: false,
        }
    }

    /// Create config respecting reduced motion preferences
    pub fn reduced_motion() -> Self {
        Self {
            enabled: true,
            reduce_motion: true,
            hover_animations: false,
            focus_animations: true, // Keep focus for accessibility
            state_animations: false,
            micro_animations: false,
        }
    }

    /// Check if a specific animation type should be enabled
    pub fn should_animate(&self, animation_type: AnimationType) -> bool {
        if !self.enabled {
            return false;
        }

        if self.reduce_motion {
            // Only allow essential animations when reduce motion is enabled
            return matches!(animation_type, AnimationType::Focus);
        }

        match animation_type {
            AnimationType::Hover => self.hover_animations,
            AnimationType::Focus => self.focus_animations,
            AnimationType::State => self.state_animations,
            AnimationType::Micro => self.micro_animations,
        }
    }
}

/// Types of animations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationType {
    Hover,
    Focus,
    State,
    Micro,
}

/// Animation utilities
pub struct AnimationUtils;

impl AnimationUtils {
    /// Get appropriate animation preset for an interaction
    pub fn preset_for_interaction(interaction: InteractionType) -> AnimationPreset {
        match interaction {
            InteractionType::Hover => AnimationPreset::button_hover(),
            InteractionType::Active => AnimationPreset::button_active(),
            InteractionType::Focus => AnimationPreset::focus_ring(),
            InteractionType::Loading => AnimationPreset::loading_state(),
        }
    }

    /// Adjust animation duration based on user preferences
    pub fn adjust_duration(
        duration: AnimationDuration,
        config: &AnimationConfig,
    ) -> AnimationDuration {
        if config.reduce_motion {
            // Reduce all animations to fastest or instant
            match duration {
                AnimationDuration::Instant => duration,
                _ => AnimationDuration::Fastest,
            }
        } else {
            duration
        }
    }

    /// Choose timing function based on animation type
    pub fn timing_for_type(animation_type: AnimationType) -> TimingFunction {
        match animation_type {
            AnimationType::Hover => TimingFunction::EaseOut,
            AnimationType::Focus => TimingFunction::EaseOut,
            AnimationType::State => TimingFunction::EaseInOut,
            AnimationType::Micro => TimingFunction::EaseOut,
        }
    }

    /// Create a transition with fallback for reduced motion
    pub fn create_transition(
        properties: Vec<AnimationProperty>,
        duration: AnimationDuration,
        timing_function: TimingFunction,
        config: &AnimationConfig,
    ) -> Option<AnimationPreset> {
        if !config.enabled {
            return None;
        }

        let adjusted_duration = Self::adjust_duration(duration, config);

        Some(AnimationPreset {
            duration: adjusted_duration,
            timing_function,
            properties,
        })
    }
}

/// Types of user interactions that trigger animations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionType {
    Hover,
    Active,
    Focus,
    Loading,
}
