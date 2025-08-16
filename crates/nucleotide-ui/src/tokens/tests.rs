// ABOUTME: Comprehensive tests for the design token system
// ABOUTME: Ensures token consistency, theme integration, and utility functions

#[cfg(test)]
mod tests {
    use crate::{DesignTokens, SemanticColors, SizeTokens};
    use crate::tokens::{BaseColors, with_alpha, lighten, darken, mix};
    use gpui::{hsla, px};

    #[test]
    fn test_base_colors_consistency() {
        let light = BaseColors::light();
        let dark = BaseColors::dark();

        // Test that we have all required colors
        assert_ne!(light.neutral_50, light.neutral_950);
        assert_ne!(dark.neutral_50, dark.neutral_950);
        
        // Test primary colors are consistent across themes (same hue)
        assert_eq!(light.primary_500.h, dark.primary_500.h);
        
        // Test semantic colors exist
        assert_ne!(light.success_500, hsla(0.0, 0.0, 0.0, 0.0));
        assert_ne!(light.warning_500, hsla(0.0, 0.0, 0.0, 0.0));
        assert_ne!(light.error_500, hsla(0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn test_semantic_colors_mapping() {
        let light_base = BaseColors::light();
        let light_semantic = SemanticColors::from_base_light(&light_base);

        // Test that semantic colors map correctly to base colors
        assert_eq!(light_semantic.background, light_base.neutral_50);
        assert_eq!(light_semantic.primary, light_base.primary_500);
        assert_eq!(light_semantic.success, light_base.success_500);

        let dark_base = BaseColors::dark();
        let dark_semantic = SemanticColors::from_base_dark(&dark_base);

        // Test dark theme mappings
        assert_eq!(dark_semantic.background, dark_base.neutral_50);
        assert_eq!(dark_semantic.primary, dark_base.primary_500);
    }

    #[test]
    fn test_size_tokens() {
        let sizes = SizeTokens::default();

        // Test spacing scale progression
        assert!(sizes.space_1 < sizes.space_2);
        assert!(sizes.space_2 < sizes.space_3);
        assert!(sizes.space_3 < sizes.space_4);

        // Test specific values
        assert_eq!(sizes.space_0, px(0.0));
        assert_eq!(sizes.space_1, px(2.0));
        assert_eq!(sizes.space_2, px(4.0));
        assert_eq!(sizes.space_3, px(8.0));

        // Test component sizes
        assert!(sizes.button_height_sm < sizes.button_height_md);
        assert!(sizes.button_height_md < sizes.button_height_lg);

        // Test text sizes
        assert!(sizes.text_xs < sizes.text_sm);
        assert!(sizes.text_sm < sizes.text_md);
    }

    #[test]
    fn test_design_tokens_creation() {
        let light_tokens = DesignTokens::light();
        let dark_tokens = DesignTokens::dark();

        // Test that light and dark tokens are different
        assert_ne!(light_tokens.colors.background, dark_tokens.colors.background);
        
        // Test that sizes are the same across themes
        assert_eq!(light_tokens.sizes.space_3, dark_tokens.sizes.space_3);
        
        // Test semantic colors exist and are valid
        assert_ne!(light_tokens.colors.text_primary, hsla(0.0, 0.0, 0.0, 0.0));
        assert_ne!(dark_tokens.colors.text_primary, hsla(0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn test_color_utilities() {
        let test_color = hsla(0.5, 0.6, 0.5, 1.0);

        // Test with_alpha
        let transparent = with_alpha(test_color, 0.5);
        assert_eq!(transparent.a, 0.5);
        assert_eq!(transparent.h, test_color.h);
        assert_eq!(transparent.s, test_color.s);
        assert_eq!(transparent.l, test_color.l);

        // Test lighten
        let lighter = lighten(test_color, 0.1);
        assert!(lighter.l > test_color.l);
        assert_eq!(lighter.h, test_color.h);
        assert_eq!(lighter.s, test_color.s);

        // Test darken
        let darker = darken(test_color, 0.1);
        assert!(darker.l < test_color.l);
        assert_eq!(darker.h, test_color.h);
        assert_eq!(darker.s, test_color.s);

        // Test mix
        let color1 = hsla(0.0, 1.0, 0.5, 1.0); // Red
        let color2 = hsla(0.33, 1.0, 0.5, 1.0); // Green
        let mixed = mix(color1, color2, 0.5);
        
        // Should be roughly between the two
        assert!(mixed.h > color1.h && mixed.h < color2.h);
    }

    #[test]
    fn test_backward_compatibility_spacing() {
        // Test that old spacing constants still work
        #[allow(deprecated)]
        {
            use crate::spacing::*;
            assert_eq!(XS, px(2.0));
            assert_eq!(SM, px(4.0));
            assert_eq!(MD, px(8.0));
            assert_eq!(LG, px(12.0));
        }
    }

    #[test]
    fn test_theme_integration() {
        let theme = crate::Theme::dark();
        
        // Test that tokens are properly integrated
        assert_eq!(theme.background, theme.tokens.colors.background);
        assert_eq!(theme.text, theme.tokens.colors.text_primary);
        assert_eq!(theme.accent, theme.tokens.colors.primary);

        // Test new APIs
        assert!(theme.is_dark());
        
        let light_theme = crate::Theme::light();
        assert!(!light_theme.is_dark());
    }

    #[test]
    fn test_surface_elevation() {
        let theme = crate::Theme::dark();
        
        // Test different elevation levels
        let surface_0 = theme.surface_at_elevation(0);
        let surface_1 = theme.surface_at_elevation(1);
        let surface_2 = theme.surface_at_elevation(2);
        let surface_3 = theme.surface_at_elevation(3);

        // Test that elevation creates progression
        assert_eq!(surface_0, theme.tokens.colors.background);
        assert_eq!(surface_1, theme.tokens.colors.surface);
        assert_eq!(surface_2, theme.tokens.colors.surface_elevated);
        
        // Higher elevations should be lighter in dark theme
        assert!(surface_3.l > surface_2.l);
    }

    #[test]
    fn test_theme_from_tokens() {
        let custom_tokens = DesignTokens::light();
        let theme = crate::Theme::from_tokens(custom_tokens);
        
        // Test that theme correctly uses the provided tokens
        assert_eq!(theme.tokens.colors.background, custom_tokens.colors.background);
        assert_eq!(theme.background, custom_tokens.colors.background);
    }

    #[test]
    fn test_color_validation() {
        let light_tokens = DesignTokens::light();
        let dark_tokens = DesignTokens::dark();

        // Test that all colors have valid alpha
        assert_eq!(light_tokens.colors.background.a, 1.0);
        assert_eq!(dark_tokens.colors.background.a, 1.0);

        // Test that colors are within valid ranges
        assert!(light_tokens.colors.background.h >= 0.0 && light_tokens.colors.background.h <= 1.0);
        assert!(light_tokens.colors.background.s >= 0.0 && light_tokens.colors.background.s <= 1.0);
        assert!(light_tokens.colors.background.l >= 0.0 && light_tokens.colors.background.l <= 1.0);
    }

    #[test]
    fn test_semantic_color_relationships() {
        let tokens = DesignTokens::dark();

        // Test that interactive states are related
        assert_ne!(tokens.colors.surface, tokens.colors.surface_hover);
        assert_ne!(tokens.colors.surface_hover, tokens.colors.surface_active);
        
        // Test that primary variants are related
        assert_ne!(tokens.colors.primary, tokens.colors.primary_hover);
        assert_ne!(tokens.colors.primary_hover, tokens.colors.primary_active);

        // Test text hierarchy
        assert_ne!(tokens.colors.text_primary, tokens.colors.text_secondary);
        assert_ne!(tokens.colors.text_secondary, tokens.colors.text_tertiary);
    }
}