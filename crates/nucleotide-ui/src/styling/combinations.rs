// ABOUTME: Style combination utilities for merging and composing styles
// ABOUTME: Provides utilities for combining styles, handling conflicts, and creating complex compositions

use super::{BoxShadow, ComputedStyle, StyleState, Transition};
use gpui::{px, Hsla};
use std::collections::HashMap;

/// Style merge strategy for handling conflicts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Override completely (default)
    Override,
    /// Merge properties intelligently
    Merge,
    /// Keep original values
    Preserve,
    /// Use additive blending where possible
    Additive,
}

/// Style combiner for merging multiple styles
#[derive(Debug)]
pub struct StyleCombiner {
    base_style: ComputedStyle,
    layers: Vec<(ComputedStyle, MergeStrategy)>,
}

impl StyleCombiner {
    /// Create a new style combiner with a base style
    pub fn new(base_style: ComputedStyle) -> Self {
        Self {
            base_style,
            layers: Vec::new(),
        }
    }

    /// Add a style layer with merge strategy
    pub fn add_layer(mut self, style: ComputedStyle, strategy: MergeStrategy) -> Self {
        self.layers.push((style, strategy));
        self
    }

    /// Add a style layer with override strategy (default)
    pub fn add(self, style: ComputedStyle) -> Self {
        self.add_layer(style, MergeStrategy::Override)
    }

    /// Add a style layer with merge strategy
    pub fn merge(self, style: ComputedStyle) -> Self {
        self.add_layer(style, MergeStrategy::Merge)
    }

    /// Add a style layer with preserve strategy
    pub fn preserve(self, style: ComputedStyle) -> Self {
        self.add_layer(style, MergeStrategy::Preserve)
    }

    /// Add a style layer with additive strategy
    pub fn additive(self, style: ComputedStyle) -> Self {
        self.add_layer(style, MergeStrategy::Additive)
    }

    /// Compute the final combined style
    pub fn compute(self) -> ComputedStyle {
        let mut result = self.base_style;

        for (layer_style, strategy) in self.layers {
            result = merge_styles(result, layer_style, strategy);
        }

        result
    }
}

/// Merge two styles with a given strategy
pub fn merge_styles(
    base: ComputedStyle,
    overlay: ComputedStyle,
    strategy: MergeStrategy,
) -> ComputedStyle {
    match strategy {
        MergeStrategy::Override => overlay,
        MergeStrategy::Preserve => base,
        MergeStrategy::Merge => merge_styles_intelligent(base, overlay),
        MergeStrategy::Additive => merge_styles_additive(base, overlay),
    }
}

/// Intelligently merge two styles
fn merge_styles_intelligent(mut base: ComputedStyle, overlay: ComputedStyle) -> ComputedStyle {
    // Override non-transparent colors
    if overlay.background.a > 0.0 {
        base.background = overlay.background;
    }
    if overlay.foreground.a > 0.0 {
        base.foreground = overlay.foreground;
    }
    if overlay.border_color.a > 0.0 {
        base.border_color = overlay.border_color;
    }

    // Override non-zero values
    if overlay.border_width.0 > 0.0 {
        base.border_width = overlay.border_width;
    }
    if overlay.border_radius.0 > 0.0 {
        base.border_radius = overlay.border_radius;
    }
    if overlay.padding_x.0 > 0.0 {
        base.padding_x = overlay.padding_x;
    }
    if overlay.padding_y.0 > 0.0 {
        base.padding_y = overlay.padding_y;
    }
    if overlay.font_size.0 > 0.0 {
        base.font_size = overlay.font_size;
    }
    if overlay.font_weight != 400 {
        base.font_weight = overlay.font_weight;
    }
    if overlay.opacity != 1.0 {
        base.opacity = overlay.opacity;
    }

    // Override if present
    if overlay.shadow.is_some() {
        base.shadow = overlay.shadow;
    }
    if overlay.transition.is_some() {
        base.transition = overlay.transition;
    }

    base
}

/// Additively merge two styles (combine values where possible)
fn merge_styles_additive(mut base: ComputedStyle, overlay: ComputedStyle) -> ComputedStyle {
    // Blend colors
    base.background = blend_colors(base.background, overlay.background);
    base.foreground = blend_colors(base.foreground, overlay.foreground);
    base.border_color = blend_colors(base.border_color, overlay.border_color);

    // Add dimensions
    base.border_width = px(base.border_width.0 + overlay.border_width.0);
    base.padding_x = px(base.padding_x.0 + overlay.padding_x.0);
    base.padding_y = px(base.padding_y.0 + overlay.padding_y.0);

    // Multiply opacity
    base.opacity = base.opacity * overlay.opacity;

    // Combine shadows (if both exist)
    match (base.shadow.clone(), overlay.shadow.clone()) {
        (Some(base_shadow), Some(overlay_shadow)) => {
            base.shadow = Some(combine_shadows(base_shadow, overlay_shadow));
        }
        (None, Some(overlay_shadow)) => {
            base.shadow = Some(overlay_shadow);
        }
        _ => {} // Keep base shadow or None
    }

    // Combine transitions
    match (base.transition.clone(), overlay.transition.clone()) {
        (Some(base_transition), Some(overlay_transition)) => {
            base.transition = Some(combine_transitions(base_transition, overlay_transition));
        }
        (None, Some(overlay_transition)) => {
            base.transition = Some(overlay_transition);
        }
        _ => {} // Keep base transition or None
    }

    base
}

/// Blend two colors using alpha compositing
fn blend_colors(base: Hsla, overlay: Hsla) -> Hsla {
    if overlay.a == 0.0 {
        return base;
    }
    if overlay.a == 1.0 || base.a == 0.0 {
        return overlay;
    }

    // Alpha compositing formula
    let a_out = overlay.a + base.a * (1.0 - overlay.a);

    if a_out == 0.0 {
        return Hsla::transparent_black();
    }

    // For simplicity in HSL space, we'll use a weighted average
    // In a real implementation, you might want to convert to RGB, blend, then back to HSL
    let weight = overlay.a / a_out;

    Hsla {
        h: overlay.h * weight + base.h * (1.0 - weight),
        s: overlay.s * weight + base.s * (1.0 - weight),
        l: overlay.l * weight + base.l * (1.0 - weight),
        a: a_out,
    }
}

/// Combine two box shadows
fn combine_shadows(_base: BoxShadow, overlay: BoxShadow) -> BoxShadow {
    // For simplicity, we'll just use the overlay shadow
    // In a real implementation, you might want to create multiple shadows
    overlay
}

/// Combine two transitions
fn combine_transitions(base: Transition, overlay: Transition) -> Transition {
    // Combine properties and use the overlay's timing
    let mut combined_properties = base.properties;
    for prop in overlay.properties {
        if !combined_properties.contains(&prop) {
            combined_properties.push(prop);
        }
    }

    Transition {
        duration: overlay.duration, // Use overlay timing
        timing_function: overlay.timing_function,
        properties: combined_properties,
    }
}

/// Conditional style application
pub struct ConditionalStyle {
    condition: Box<dyn Fn() -> bool>,
    style: ComputedStyle,
}

impl ConditionalStyle {
    /// Create a new conditional style
    pub fn new<F>(condition: F, style: ComputedStyle) -> Self
    where
        F: Fn() -> bool + 'static,
    {
        Self {
            condition: Box::new(condition),
            style,
        }
    }

    /// Check if this conditional style should be applied
    pub fn should_apply(&self) -> bool {
        (self.condition)()
    }

    /// Get the style if condition is met
    pub fn get_style(&self) -> Option<&ComputedStyle> {
        if self.should_apply() {
            Some(&self.style)
        } else {
            None
        }
    }
}

/// Style variant composer for creating complex style combinations
pub struct StyleComposer {
    base_styles: HashMap<String, ComputedStyle>,
    conditional_styles: Vec<ConditionalStyle>,
    state_styles: HashMap<StyleState, ComputedStyle>,
}

impl StyleComposer {
    /// Create a new style composer
    pub fn new() -> Self {
        Self {
            base_styles: HashMap::new(),
            conditional_styles: Vec::new(),
            state_styles: HashMap::new(),
        }
    }

    /// Add a base style variant
    pub fn add_base_style(mut self, key: String, style: ComputedStyle) -> Self {
        self.base_styles.insert(key, style);
        self
    }

    /// Add a conditional style
    pub fn add_conditional_style<F>(mut self, condition: F, style: ComputedStyle) -> Self
    where
        F: Fn() -> bool + 'static,
    {
        self.conditional_styles
            .push(ConditionalStyle::new(condition, style));
        self
    }

    /// Add a state-specific style
    pub fn add_state_style(mut self, state: StyleState, style: ComputedStyle) -> Self {
        self.state_styles.insert(state, style);
        self
    }

    /// Compose styles for given base variant and state
    pub fn compose(&self, base_variant: &str, state: StyleState) -> Option<ComputedStyle> {
        let base_style = self.base_styles.get(base_variant)?.clone();

        let mut combiner = StyleCombiner::new(base_style);

        // Apply conditional styles
        for conditional in &self.conditional_styles {
            if let Some(style) = conditional.get_style() {
                combiner = combiner.merge(style.clone());
            }
        }

        // Apply state-specific styles
        if let Some(state_style) = self.state_styles.get(&state) {
            combiner = combiner.add(state_style.clone());
        }

        Some(combiner.compute())
    }
}

impl Default for StyleComposer {
    fn default() -> Self {
        Self::new()
    }
}

/// Utility functions for common style combinations
pub struct StyleUtils;

impl StyleUtils {
    /// Create a hover style by lightening/darkening colors
    pub fn create_hover_style(base: &ComputedStyle, is_dark_theme: bool) -> ComputedStyle {
        let mut hover_style = base.clone();

        // Adjust background color for hover
        hover_style.background = if is_dark_theme {
            lighten_color(hover_style.background, 0.1)
        } else {
            darken_color(hover_style.background, 0.1)
        };

        hover_style
    }

    /// Create an active/pressed style
    pub fn create_active_style(base: &ComputedStyle, is_dark_theme: bool) -> ComputedStyle {
        let mut active_style = base.clone();

        // Adjust background color for active state
        active_style.background = if is_dark_theme {
            lighten_color(active_style.background, 0.15)
        } else {
            darken_color(active_style.background, 0.15)
        };

        active_style
    }

    /// Create a disabled style
    pub fn create_disabled_style(base: &ComputedStyle) -> ComputedStyle {
        let mut disabled_style = base.clone();

        disabled_style.opacity = 0.6;
        disabled_style.background = desaturate_color(disabled_style.background, 0.5);
        disabled_style.foreground = desaturate_color(disabled_style.foreground, 0.5);

        disabled_style
    }

    /// Create a focus style with focus ring
    pub fn create_focus_style(base: &ComputedStyle, focus_color: Hsla) -> ComputedStyle {
        let mut focus_style = base.clone();

        focus_style.border_color = focus_color;
        focus_style.border_width = px(2.0);

        // Add focus ring shadow
        focus_style.shadow = Some(BoxShadow {
            offset_x: px(0.0),
            offset_y: px(0.0),
            blur_radius: px(0.0),
            spread_radius: px(2.0),
            color: Hsla {
                a: 0.2,
                ..focus_color
            },
        });

        focus_style
    }
}

/// Color manipulation utilities
fn lighten_color(color: Hsla, amount: f32) -> Hsla {
    Hsla {
        l: (color.l + amount).min(1.0),
        ..color
    }
}

fn darken_color(color: Hsla, amount: f32) -> Hsla {
    Hsla {
        l: (color.l - amount).max(0.0),
        ..color
    }
}

fn desaturate_color(color: Hsla, amount: f32) -> Hsla {
    Hsla {
        s: (color.s * (1.0 - amount)).max(0.0),
        ..color
    }
}

/// Preset style combinations for common patterns
pub struct StylePresets;

impl StylePresets {
    /// Create a button style set (normal, hover, active, disabled, focus)
    pub fn button_set(
        base: ComputedStyle,
        focus_color: Hsla,
        is_dark_theme: bool,
    ) -> HashMap<StyleState, ComputedStyle> {
        let mut styles = HashMap::new();

        styles.insert(StyleState::Default, base.clone());
        styles.insert(
            StyleState::Hover,
            StyleUtils::create_hover_style(&base, is_dark_theme),
        );
        styles.insert(
            StyleState::Active,
            StyleUtils::create_active_style(&base, is_dark_theme),
        );
        styles.insert(
            StyleState::Disabled,
            StyleUtils::create_disabled_style(&base),
        );
        styles.insert(
            StyleState::Focused,
            StyleUtils::create_focus_style(&base, focus_color),
        );

        styles
    }

    /// Create a card style set with elevation
    pub fn card_set(
        base: ComputedStyle,
        elevation_levels: &[u8],
    ) -> HashMap<String, ComputedStyle> {
        let mut styles = HashMap::new();

        for &level in elevation_levels {
            let mut card_style = base.clone();

            // Add shadow based on elevation level
            let shadow_opacity = (level as f32) * 0.05;
            let shadow_blur = px((level as f32) * 2.0);
            let shadow_offset = px((level as f32) * 1.0);

            card_style.shadow = Some(BoxShadow {
                offset_x: px(0.0),
                offset_y: shadow_offset,
                blur_radius: shadow_blur,
                spread_radius: px(0.0),
                color: Hsla {
                    h: 0.0,
                    s: 0.0,
                    l: 0.0,
                    a: shadow_opacity,
                },
            });

            styles.insert(format!("elevation-{}", level), card_style);
        }

        styles
    }
}
