// ABOUTME: WCAG contrast compliance tests for TitleBarTokens
// ABOUTME: Ensures titlebar colors meet accessibility standards across all themes

use crate::styling::ColorTheory;
use crate::tokens::{ColorContext, TitleBarTokens};
use crate::DesignTokens;

#[cfg(test)]
mod titlebar_contrast_tests {
    use super::*;

    /// Test that titlebar tokens meet WCAG AA contrast requirements
    #[test]
    fn test_titlebar_tokens_contrast_compliance() {
        let test_cases = [
            ("Light Theme", DesignTokens::light()),
            ("Dark Theme", DesignTokens::dark()),
        ];

        for (theme_name, tokens) in test_cases {
            println!("Testing {}", theme_name);

            let contexts = [
                ("OnSurface", ColorContext::OnSurface),
                ("OnPrimary", ColorContext::OnPrimary),
                ("Floating", ColorContext::Floating),
                ("Overlay", ColorContext::Overlay),
            ];

            for (context_name, context) in contexts {
                let titlebar_tokens = match context {
                    ColorContext::OnSurface => TitleBarTokens::on_surface(&tokens),
                    ColorContext::OnPrimary => TitleBarTokens::on_primary(&tokens),
                    ColorContext::Floating => TitleBarTokens::floating(&tokens),
                    ColorContext::Overlay => TitleBarTokens::overlay(&tokens),
                };

                let contrast_ratio = ColorTheory::contrast_ratio(
                    titlebar_tokens.background,
                    titlebar_tokens.foreground,
                );

                // WCAG AA requires at least 4.5:1 contrast ratio for normal text
                assert!(
                    contrast_ratio >= 4.5,
                    "{} {} titlebar contrast ratio {:.2} is below WCAG AA standard (4.5:1)",
                    theme_name,
                    context_name,
                    contrast_ratio
                );

                // Also test border contrast
                let border_contrast =
                    ColorTheory::contrast_ratio(titlebar_tokens.background, titlebar_tokens.border);

                // Border should have at least 2.5:1 contrast ratio (more lenient for borders)
                assert!(
                    border_contrast >= 2.5,
                    "{} {} titlebar border contrast ratio {:.2} is below recommended standard (2.5:1)",
                    theme_name, context_name, border_contrast
                );

                println!(
                    "  {} {} - Text: {:.2}:1, Border: {:.2}:1 ✓",
                    context_name, theme_name, contrast_ratio, border_contrast
                );
            }
        }
    }

    /// Test titlebar tokens with Helix theme colors
    #[test]
    fn test_titlebar_tokens_with_helix_colors() {
        // Create a mock Helix theme colors struct for testing
        let helix_colors = crate::theme_manager::HelixThemeColors {
            selection: gpui::hsla(220.0 / 360.0, 0.7, 0.6, 1.0), // Blue selection
            cursor_normal: gpui::hsla(220.0 / 360.0, 0.8, 0.5, 1.0),
            cursor_insert: gpui::hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
            cursor_select: gpui::hsla(40.0 / 360.0, 0.8, 0.5, 1.0),
            cursor_match: gpui::hsla(200.0 / 360.0, 0.7, 0.5, 1.0),
            error: gpui::hsla(0.0, 0.8, 0.5, 1.0),
            warning: gpui::hsla(40.0 / 360.0, 0.8, 0.5, 1.0),
            success: gpui::hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
            statusline: gpui::hsla(0.0, 0.0, 0.95, 1.0),
            statusline_inactive: gpui::hsla(0.0, 0.0, 0.9, 1.0),
            popup: gpui::hsla(0.0, 0.0, 0.98, 1.0),
            bufferline_background: gpui::hsla(0.0, 0.0, 0.92, 1.0),
            bufferline_active: gpui::hsla(0.0, 0.0, 1.0, 1.0),
            bufferline_inactive: gpui::hsla(0.0, 0.0, 0.85, 1.0),
            gutter_background: gpui::hsla(0.0, 0.0, 0.98, 1.0),
            gutter_selected: gpui::hsla(0.0, 0.0, 0.95, 1.0),
            line_number: gpui::hsla(0.0, 0.0, 0.6, 1.0),
            line_number_active: gpui::hsla(0.0, 0.0, 0.4, 1.0),
            menu_background: gpui::hsla(0.0, 0.0, 0.98, 1.0),
            menu_selected: gpui::hsla(220.0 / 360.0, 0.7, 0.9, 1.0),
            menu_separator: gpui::hsla(0.0, 0.0, 0.9, 1.0),
            separator: gpui::hsla(0.0, 0.0, 0.9, 1.0),
            focus: gpui::hsla(220.0 / 360.0, 0.7, 0.6, 1.0),
        };

        let test_cases = [
            (
                "Light with Helix",
                DesignTokens::light_with_helix_colors(helix_colors),
            ),
            (
                "Dark with Helix",
                DesignTokens::dark_with_helix_colors(helix_colors),
            ),
        ];

        for (theme_name, tokens) in test_cases {
            let titlebar_tokens = TitleBarTokens::on_surface(&tokens);

            let contrast_ratio =
                ColorTheory::contrast_ratio(titlebar_tokens.background, titlebar_tokens.foreground);

            assert!(
                contrast_ratio >= 4.5,
                "{} titlebar contrast ratio {:.2} is below WCAG AA standard (4.5:1)",
                theme_name,
                contrast_ratio
            );

            println!(
                "{} titlebar contrast: {:.2}:1 ✓",
                theme_name, contrast_ratio
            );
        }
    }

    /// Test that titlebar height uses the token system
    #[test]
    fn test_titlebar_height_uses_tokens() {
        let tokens = DesignTokens::light();
        let titlebar_tokens = TitleBarTokens::on_surface(&tokens);

        // Verify height matches the size token
        assert_eq!(titlebar_tokens.height, tokens.sizes.titlebar_height);

        println!(
            "Titlebar height: {:?} matches token system ✓",
            titlebar_tokens.height
        );
    }
}
