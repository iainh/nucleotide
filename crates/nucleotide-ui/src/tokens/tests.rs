// ABOUTME: WCAG contrast compliance tests for TitleBarTokens
// ABOUTME: Ensures titlebar colors meet accessibility standards across all themes

use crate::DesignTokens;
use crate::styling::ColorTheory;
use crate::tokens::{ColorContext, TitleBarTokens};

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
                    theme_name,
                    context_name,
                    border_contrast
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

#[cfg(test)]
mod component_token_tests {
    use super::*;
    use gpui::hsla;

    /// Test hybrid color system integration for component tokens
    #[test]
    fn test_hybrid_component_tokens() {
        let helix_colors = crate::theme_manager::HelixThemeColors {
            selection: hsla(220.0 / 360.0, 0.7, 0.8, 0.3),
            cursor_normal: hsla(220.0 / 360.0, 0.7, 0.6, 1.0),
            cursor_insert: hsla(120.0 / 360.0, 0.7, 0.6, 1.0),
            cursor_select: hsla(280.0 / 360.0, 0.7, 0.6, 1.0),
            cursor_match: hsla(40.0 / 360.0, 0.7, 0.6, 1.0),
            error: hsla(0.0, 0.7, 0.6, 1.0),
            warning: hsla(40.0 / 360.0, 0.8, 0.6, 1.0),
            success: hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
            statusline: hsla(0.0, 0.0, 0.2, 1.0),
            statusline_inactive: hsla(0.0, 0.0, 0.15, 1.0),
            popup: hsla(0.0, 0.0, 0.1, 1.0),
            bufferline_background: hsla(0.0, 0.0, 0.12, 1.0),
            bufferline_active: hsla(0.0, 0.0, 0.05, 1.0),
            bufferline_inactive: hsla(0.0, 0.0, 0.08, 1.0),
            gutter_background: hsla(0.0, 0.0, 0.05, 1.0),
            gutter_selected: hsla(0.0, 0.0, 0.1, 1.0),
            line_number: hsla(0.0, 0.0, 0.4, 1.0),
            line_number_active: hsla(0.0, 0.0, 0.8, 1.0),
            menu_background: hsla(0.0, 0.0, 0.08, 1.0),
            menu_selected: hsla(220.0 / 360.0, 0.7, 0.8, 0.3),
            menu_separator: hsla(0.0, 0.0, 0.15, 1.0),
            separator: hsla(0.0, 0.0, 0.2, 1.0),
            focus: hsla(220.0 / 360.0, 0.7, 0.6, 1.0),
        };

        let surface_color = hsla(0.0, 0.0, 0.05, 1.0); // Dark surface
        let tokens = DesignTokens::from_helix_and_surface(helix_colors, surface_color, true);

        // Test that all component token generators work
        let titlebar = tokens.titlebar_tokens();
        let file_tree = tokens.file_tree_tokens();
        let status_bar = tokens.status_bar_tokens();
        let tab_bar = tokens.tab_bar_tokens();

        // Verify that chrome backgrounds are computed (different from Helix colors)
        assert_ne!(titlebar.background, helix_colors.statusline);
        assert_ne!(file_tree.background, helix_colors.gutter_background);
        assert_ne!(status_bar.background_active, helix_colors.statusline);
        assert_ne!(tab_bar.container_background, helix_colors.bufferline_background);

        // Verify that editor content colors are preserved from Helix
        assert_eq!(file_tree.item_background_selected, helix_colors.selection);
        assert_eq!(status_bar.mode_normal, helix_colors.cursor_normal);
        assert_eq!(status_bar.mode_insert, helix_colors.cursor_insert);
        assert_eq!(tab_bar.tab_modified_indicator, helix_colors.warning);

        println!("Hybrid component tokens validation passed ✓");
    }

    /// Test file tree token contrast ratios
    #[test]
    fn test_file_tree_contrast() {
        let tokens = DesignTokens::dark();
        let file_tree = tokens.file_tree_tokens();

        // Test background vs text contrast
        let bg_text_contrast = ColorTheory::contrast_ratio(
            file_tree.background,
            file_tree.item_text,
        );
        assert!(
            bg_text_contrast >= 4.5,
            "File tree background/text contrast {:.2} below WCAG AA (4.5:1)",
            bg_text_contrast
        );

        // Test hover state contrast
        let hover_contrast = ColorTheory::contrast_ratio(
            file_tree.item_background_hover,
            file_tree.item_text,
        );
        assert!(
            hover_contrast >= 3.0, // Relaxed for interactive elements
            "File tree hover contrast {:.2} below minimum (3.0:1)",
            hover_contrast
        );

        println!("File tree contrast ratios: bg/text={:.2}, hover={:.2} ✓", 
                 bg_text_contrast, hover_contrast);
    }

    /// Test status bar token functionality
    #[test]
    fn test_status_bar_modes() {
        let tokens = DesignTokens::light();
        let status_bar = tokens.status_bar_tokens();

        // Verify mode colors are distinct
        assert_ne!(status_bar.mode_normal, status_bar.mode_insert);
        assert_ne!(status_bar.mode_normal, status_bar.mode_select);
        assert_ne!(status_bar.mode_insert, status_bar.mode_select);

        // Verify background contrast
        let active_contrast = ColorTheory::contrast_ratio(
            status_bar.background_active,
            status_bar.text_primary,
        );
        let inactive_contrast = ColorTheory::contrast_ratio(
            status_bar.background_inactive,
            status_bar.text_secondary,
        );

        assert!(active_contrast >= 4.5);
        assert!(inactive_contrast >= 3.0); // Relaxed for inactive state

        println!("Status bar contrast: active={:.2}, inactive={:.2} ✓",
                 active_contrast, inactive_contrast);
    }

    /// Test tab bar active/inactive distinction
    #[test]
    fn test_tab_bar_states() {
        let tokens = DesignTokens::dark();
        let tab_bar = tokens.tab_bar_tokens();

        // Verify tab states are visually distinct
        assert_ne!(tab_bar.tab_active_background, tab_bar.tab_inactive_background);
        assert_ne!(tab_bar.tab_active_background, tab_bar.container_background);
        assert_ne!(tab_bar.tab_text_active, tab_bar.tab_text_inactive);

        // Test active tab contrast (should be high for readability)
        let active_contrast = ColorTheory::contrast_ratio(
            tab_bar.tab_active_background,
            tab_bar.tab_text_active,
        );
        assert!(
            active_contrast >= 7.0, // Higher standard for active tabs
            "Active tab contrast {:.2} below enhanced standard (7.0:1)",
            active_contrast
        );

        // Test inactive tab contrast (can be lower)
        let inactive_contrast = ColorTheory::contrast_ratio(
            tab_bar.tab_inactive_background,
            tab_bar.tab_text_inactive,
        );
        assert!(
            inactive_contrast >= 3.0,
            "Inactive tab contrast {:.2} below minimum (3.0:1)",
            inactive_contrast
        );

        println!("Tab bar contrast: active={:.2}, inactive={:.2} ✓",
                 active_contrast, inactive_contrast);
    }

    /// Test component token consistency across themes
    #[test]
    fn test_component_token_consistency() {
        let test_themes = [
            ("Light", DesignTokens::light()),
            ("Dark", DesignTokens::dark()),
        ];

        for (theme_name, tokens) in test_themes {
            let titlebar = tokens.titlebar_tokens();
            let file_tree = tokens.file_tree_tokens();
            let status_bar = tokens.status_bar_tokens();
            let tab_bar = tokens.tab_bar_tokens();

            // Verify all components have consistent separator colors
            assert_eq!(titlebar.border, tokens.chrome.separator_color);
            assert_eq!(file_tree.separator, tokens.chrome.separator_color);
            assert_eq!(status_bar.border, tokens.chrome.separator_color);
            assert_eq!(tab_bar.tab_separator, tokens.chrome.separator_color);

            // Verify chrome backgrounds are computed correctly
            assert_eq!(titlebar.background, tokens.chrome.titlebar_background);
            assert_eq!(file_tree.background, tokens.chrome.file_tree_background);
            assert_eq!(status_bar.background_active, tokens.chrome.footer_background);
            assert_eq!(tab_bar.container_background, tokens.chrome.tab_empty_background);

            // Verify titlebar and status bar have matching backgrounds (USER FIX)
            assert_eq!(
                titlebar.background, 
                status_bar.background_active,
                "{} theme: titlebar background ({:?}) should match status bar background ({:?})",
                theme_name, titlebar.background, status_bar.background_active
            );

            println!("{} theme component consistency validated ✓", theme_name);
        }
    }

    /// Test extension methods work correctly
    #[test]
    fn test_extension_methods() {
        let tokens = DesignTokens::dark();

        // Test ChromeTokens extension methods
        let titlebar_ext = tokens.chrome.titlebar_tokens(&tokens.sizes);
        let file_tree_ext = tokens.chrome.file_tree_tokens(&tokens.editor);
        let status_bar_ext = tokens.chrome.status_bar_tokens(&tokens.editor);
        let tab_bar_ext = tokens.chrome.tab_bar_tokens(&tokens.editor);

        // Test DesignTokens convenience methods
        let titlebar_conv = tokens.titlebar_tokens();
        let file_tree_conv = tokens.file_tree_tokens();
        let status_bar_conv = tokens.status_bar_tokens();
        let tab_bar_conv = tokens.tab_bar_tokens();

        // Verify both approaches produce identical results
        assert_eq!(titlebar_ext.background, titlebar_conv.background);
        assert_eq!(file_tree_ext.background, file_tree_conv.background);
        assert_eq!(status_bar_ext.background_active, status_bar_conv.background_active);
        assert_eq!(tab_bar_ext.container_background, tab_bar_conv.container_background);

        println!("Extension method consistency validated ✓");
    }
}
