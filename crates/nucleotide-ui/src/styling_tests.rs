// ABOUTME: Tests for the nucleotide-ui styling system
// ABOUTME: Ensures style computation, variants, responsive design, and animations work correctly

#[cfg(test)]
mod tests {
    use crate::styling::animations::TimingFunction;
    use crate::{
        compute_component_style, compute_style_for_states, AnimationConfig, AnimationDuration,
        AnimationPreset, AnimationType, Breakpoint, ResponsiveValue, StyleCombiner, StyleContext,
        StylePresets, StyleSize, StyleState, StyleUtils, StyleVariant, Theme, VariantColors,
        VariantStyler, ViewportContext,
    };
    use gpui::{hsla, px, Size};

    #[test]
    fn test_style_state_properties() {
        // Test interactive states
        assert!(StyleState::Default.is_interactive());
        assert!(StyleState::Hover.is_interactive());
        assert!(StyleState::Active.is_interactive());
        assert!(StyleState::Focused.is_interactive());
        assert!(!StyleState::Disabled.is_interactive());
        assert!(!StyleState::Loading.is_interactive());
        assert!(StyleState::Selected.is_interactive());

        // Test user interaction states
        assert!(!StyleState::Default.is_user_interaction());
        assert!(StyleState::Hover.is_user_interaction());
        assert!(StyleState::Active.is_user_interaction());
        assert!(StyleState::Focused.is_user_interaction());
        assert!(!StyleState::Disabled.is_user_interaction());
        assert!(!StyleState::Loading.is_user_interaction());
        assert!(!StyleState::Selected.is_user_interaction());

        // Test priority ordering
        assert!(StyleState::Disabled.priority() > StyleState::Active.priority());
        assert!(StyleState::Active.priority() > StyleState::Hover.priority());
        assert!(StyleState::Hover.priority() > StyleState::Default.priority());
    }

    #[test]
    fn test_style_context_creation() {
        let theme = Theme::dark();
        let context = StyleContext::new(&theme, StyleState::Default, "primary", "medium");

        assert_eq!(context.variant, "primary");
        assert_eq!(context.size, "medium");
        assert_eq!(context.state, StyleState::Default);
        assert!(context.is_dark_theme);
    }

    #[test]
    fn test_base_style_computation() {
        let theme = Theme::dark();
        let context = StyleContext::new(&theme, StyleState::Default, "primary", "medium");

        let style = context.compute_base_style();

        // Check that base styles are applied
        assert_eq!(style.background, theme.tokens.colors.surface);
        assert_eq!(style.foreground, theme.tokens.colors.text_primary);
        assert_eq!(style.border_color, theme.tokens.colors.border_default);
        assert_eq!(style.padding_x, theme.tokens.sizes.space_3);
        assert_eq!(style.padding_y, theme.tokens.sizes.space_2);
        assert_eq!(style.font_size, px(14.0));
        assert_eq!(style.border_radius, theme.tokens.sizes.radius_md);
    }

    #[test]
    fn test_variant_style_application() {
        let theme = Theme::dark();
        let context = StyleContext::new(&theme, StyleState::Default, "primary", "medium");

        let base_style = context.compute_base_style();
        let variant_style = context.apply_variant_styles(base_style);

        // Primary variant should use primary colors
        assert_eq!(variant_style.background, theme.tokens.colors.primary);
        assert_eq!(
            variant_style.foreground,
            theme.tokens.colors.text_on_primary
        );
        assert_eq!(variant_style.border_color, theme.tokens.colors.primary);
    }

    #[test]
    fn test_state_style_application() {
        let theme = Theme::dark();

        // Test hover state
        let hover_context = StyleContext::new(&theme, StyleState::Hover, "primary", "medium");
        let base_style = hover_context.compute_base_style();
        let variant_style = hover_context.apply_variant_styles(base_style);
        let state_style = hover_context.apply_state_styles(variant_style);

        assert_eq!(state_style.background, theme.tokens.colors.primary_hover);

        // Test disabled state
        let disabled_context = StyleContext::new(&theme, StyleState::Disabled, "primary", "medium");
        let base_style = disabled_context.compute_base_style();
        let variant_style = disabled_context.apply_variant_styles(base_style);
        let state_style = disabled_context.apply_state_styles(variant_style);

        assert_eq!(state_style.background, theme.tokens.colors.surface_disabled);
        assert_eq!(state_style.opacity, 0.6);
    }

    #[test]
    fn test_complete_style_computation() {
        let theme = Theme::dark();

        let style = compute_component_style(&theme, StyleState::Hover, "secondary", "large");

        // Should have large size properties
        assert_eq!(style.padding_x, theme.tokens.sizes.space_4);
        assert_eq!(style.padding_y, theme.tokens.sizes.space_3);
        assert_eq!(style.font_size, px(16.0));
        assert_eq!(style.border_radius, theme.tokens.sizes.radius_lg);

        // Should have secondary variant with hover state
        assert_eq!(style.background, theme.tokens.colors.surface_hover);
        assert_eq!(style.border_width, px(1.0)); // Secondary has border
    }

    #[test]
    fn test_style_state_priority_resolution() {
        let theme = Theme::dark();
        let states = vec![StyleState::Hover, StyleState::Disabled, StyleState::Active];

        let style = compute_style_for_states(&theme, &states, "primary", "medium");

        // Disabled should win due to highest priority
        assert_eq!(style.background, theme.tokens.colors.surface_disabled);
        assert_eq!(style.opacity, 0.6);
    }

    #[test]
    fn test_style_variants() {
        // Test variant string conversion
        assert_eq!(StyleVariant::Primary.as_str(), "primary");
        assert_eq!(StyleVariant::Secondary.as_str(), "secondary");
        assert_eq!(StyleVariant::Ghost.as_str(), "ghost");
        assert_eq!(StyleVariant::Danger.as_str(), "danger");

        // Test semantic roles
        assert_eq!(
            StyleVariant::Primary.semantic_role(),
            crate::styling::variants::VariantRole::Primary
        );
        assert_eq!(
            StyleVariant::Danger.semantic_role(),
            crate::styling::variants::VariantRole::Destructive
        );

        // Test emphasis
        assert!(StyleVariant::Primary.is_emphasis());
        assert!(StyleVariant::Danger.is_emphasis());
        assert!(!StyleVariant::Ghost.is_emphasis());
        assert!(!StyleVariant::Secondary.is_emphasis());
    }

    #[test]
    fn test_style_sizes() {
        // Test size string conversion
        assert_eq!(StyleSize::Small.as_str(), "sm");
        assert_eq!(StyleSize::Medium.as_str(), "md");
        assert_eq!(StyleSize::Large.as_str(), "lg");

        // Test scale factors
        assert_eq!(StyleSize::Small.scale_factor(), 0.875);
        assert_eq!(StyleSize::Medium.scale_factor(), 1.0);
        assert_eq!(StyleSize::Large.scale_factor(), 1.125);

        // Test font size scaling
        let base_size = px(14.0);
        assert_eq!(StyleSize::Small.font_size(base_size), px(12.25));
        assert_eq!(StyleSize::Medium.font_size(base_size), px(14.0));
        assert_eq!(StyleSize::Large.font_size(base_size), px(15.75));
    }

    #[test]
    fn test_variant_colors() {
        let theme = Theme::dark();

        let primary_colors = VariantColors::for_variant(StyleVariant::Primary, &theme);
        assert_eq!(primary_colors.background, theme.tokens.colors.primary);
        assert_eq!(
            primary_colors.foreground,
            theme.tokens.colors.text_on_primary
        );
        assert_eq!(
            primary_colors.hover_background,
            theme.tokens.colors.primary_hover
        );

        let ghost_colors = VariantColors::for_variant(StyleVariant::Ghost, &theme);
        assert_eq!(ghost_colors.background, hsla(0.0, 0.0, 0.0, 0.0)); // Transparent
        assert_eq!(ghost_colors.foreground, theme.tokens.colors.text_primary);
    }

    #[test]
    fn test_variant_styler() {
        let theme = Theme::dark();

        let variant_style =
            VariantStyler::compute_variant_style(StyleVariant::Primary, StyleSize::Medium, &theme);

        assert_eq!(variant_style.variant, StyleVariant::Primary);
        assert_eq!(variant_style.size, StyleSize::Medium);
        assert_eq!(variant_style.colors.background, theme.tokens.colors.primary);
        assert_eq!(variant_style.padding_x, theme.tokens.sizes.space_3);
        assert_eq!(variant_style.font_size, px(14.0));
        assert_eq!(variant_style.border_width, px(0.0)); // Primary has no border
    }

    #[test]
    fn test_breakpoints() {
        // Test breakpoint from width
        assert_eq!(Breakpoint::from_width(px(500.0)), Breakpoint::ExtraSmall);
        assert_eq!(Breakpoint::from_width(px(700.0)), Breakpoint::Small);
        assert_eq!(Breakpoint::from_width(px(900.0)), Breakpoint::Medium);
        assert_eq!(Breakpoint::from_width(px(1200.0)), Breakpoint::Large);
        assert_eq!(Breakpoint::from_width(px(1400.0)), Breakpoint::ExtraLarge);
        assert_eq!(Breakpoint::from_width(px(1600.0)), Breakpoint::XXLarge);

        // Test string identifiers
        assert_eq!(Breakpoint::ExtraSmall.as_str(), "xs");
        assert_eq!(Breakpoint::Medium.as_str(), "md");
        assert_eq!(Breakpoint::XXLarge.as_str(), "2xl");
    }

    #[test]
    fn test_responsive_values() {
        let responsive = ResponsiveValue::new(px(16.0))
            .set(Breakpoint::Small, px(18.0))
            .set(Breakpoint::Large, px(20.0));

        // Test value resolution
        assert_eq!(*responsive.get(Breakpoint::ExtraSmall), px(16.0)); // Default
        assert_eq!(*responsive.get(Breakpoint::Small), px(18.0));
        assert_eq!(*responsive.get(Breakpoint::Medium), px(18.0)); // Falls back to Small
        assert_eq!(*responsive.get(Breakpoint::Large), px(20.0));
        assert_eq!(*responsive.get(Breakpoint::ExtraLarge), px(20.0)); // Falls back to Large
    }

    #[test]
    fn test_viewport_context() {
        let mobile_size = Size {
            width: px(400.0),
            height: px(800.0),
        };
        let mobile_context = ViewportContext::from_size(mobile_size);

        assert_eq!(mobile_context.breakpoint, Breakpoint::ExtraSmall);
        assert!(mobile_context.is_mobile);
        assert!(!mobile_context.is_tablet);
        assert!(!mobile_context.is_desktop);

        let desktop_size = Size {
            width: px(1200.0),
            height: px(800.0),
        };
        let desktop_context = ViewportContext::from_size(desktop_size);

        assert_eq!(desktop_context.breakpoint, Breakpoint::Large);
        assert!(!desktop_context.is_mobile);
        assert!(!desktop_context.is_tablet);
        assert!(desktop_context.is_desktop);
    }

    #[test]
    fn test_animation_durations() {
        assert_eq!(AnimationDuration::Instant.as_millis(), 0);
        assert_eq!(AnimationDuration::Fast.as_millis(), 100);
        assert_eq!(AnimationDuration::Normal.as_millis(), 150);
        assert_eq!(AnimationDuration::Slow.as_millis(), 300);

        assert!(AnimationDuration::Fast.is_micro());
        assert!(!AnimationDuration::Slow.is_micro());
        assert!(AnimationDuration::Slow.is_major());
        assert!(!AnimationDuration::Fast.is_major());
    }

    #[test]
    fn test_timing_functions() {
        let ease_out = TimingFunction::EaseOut;
        let (x1, y1, x2, y2) = ease_out.control_points();
        assert_eq!((x1, y1, x2, y2), (0.0, 0.0, 0.58, 1.0));

        assert!(ease_out.is_fast());
        assert!(!TimingFunction::EaseInOut.is_fast());
        assert!(TimingFunction::EaseInOut.is_slow());
    }

    #[test]
    fn test_animation_presets() {
        let button_hover = AnimationPreset::button_hover();
        assert_eq!(button_hover.duration, AnimationDuration::Fast);
        assert_eq!(button_hover.timing_function, TimingFunction::EaseOut);
        assert!(button_hover
            .properties
            .contains(&crate::styling::animations::AnimationProperty::Background));

        let focus_ring = AnimationPreset::focus_ring();
        assert_eq!(focus_ring.duration, AnimationDuration::Normal);
        assert!(focus_ring
            .properties
            .contains(&crate::styling::animations::AnimationProperty::BorderColor));
    }

    #[test]
    fn test_animation_config() {
        let default_config = AnimationConfig::default();
        assert!(default_config.enabled);
        assert!(default_config.hover_animations);
        assert!(!default_config.reduce_motion);

        let reduced_config = AnimationConfig::reduced_motion();
        assert!(reduced_config.enabled);
        assert!(reduced_config.reduce_motion);
        assert!(!reduced_config.hover_animations);
        assert!(reduced_config.focus_animations); // Keep for accessibility

        // Test animation type checking
        assert!(default_config.should_animate(AnimationType::Hover));
        assert!(!reduced_config.should_animate(AnimationType::Hover));
        assert!(reduced_config.should_animate(AnimationType::Focus));
    }

    #[test]
    fn test_style_combiner() {
        let base_style = crate::styling::ComputedStyle {
            background: hsla(0.0, 0.0, 0.0, 1.0),
            foreground: hsla(0.0, 0.0, 1.0, 1.0),
            opacity: 1.0,
            ..Default::default()
        };

        let overlay_style = crate::styling::ComputedStyle {
            background: hsla(120.0, 0.5, 0.5, 1.0),
            opacity: 0.8,
            ..Default::default()
        };

        let combined = StyleCombiner::new(base_style).add(overlay_style).compute();

        // Override strategy should use overlay values
        assert_eq!(combined.background, hsla(120.0, 0.5, 0.5, 1.0));
        assert_eq!(combined.opacity, 0.8);
        assert_eq!(combined.foreground, hsla(0.0, 0.0, 0.0, 0.0)); // Overlay default (transparent black)
    }

    #[test]
    fn test_style_utils() {
        let base_style = crate::styling::ComputedStyle {
            background: hsla(240.0, 0.5, 0.5, 1.0),
            ..Default::default()
        };

        // Test hover style creation
        let hover_style = StyleUtils::create_hover_style(&base_style, true);
        assert!(hover_style.background.l > base_style.background.l); // Lightened for dark theme

        let hover_style_light = StyleUtils::create_hover_style(&base_style, false);
        assert!(hover_style_light.background.l < base_style.background.l); // Darkened for light theme

        // Test disabled style creation
        let disabled_style = StyleUtils::create_disabled_style(&base_style);
        assert_eq!(disabled_style.opacity, 0.6);
        assert!(disabled_style.background.s < base_style.background.s); // Desaturated
    }

    #[test]
    fn test_style_presets() {
        let base_style = crate::styling::ComputedStyle {
            background: hsla(240.0, 0.5, 0.5, 1.0),
            ..Default::default()
        };

        let focus_color = hsla(200.0, 0.8, 0.6, 1.0);
        let button_styles = StylePresets::button_set(base_style.clone(), focus_color, true);

        // Check that all required states are present
        assert!(button_styles.contains_key(&StyleState::Default));
        assert!(button_styles.contains_key(&StyleState::Hover));
        assert!(button_styles.contains_key(&StyleState::Active));
        assert!(button_styles.contains_key(&StyleState::Disabled));
        assert!(button_styles.contains_key(&StyleState::Focused));

        // Check disabled style properties
        let disabled = &button_styles[&StyleState::Disabled];
        assert_eq!(disabled.opacity, 0.6);

        // Check focus style properties
        let focused = &button_styles[&StyleState::Focused];
        assert_eq!(focused.border_color, focus_color);
        assert!(focused.shadow.is_some());
    }
}
