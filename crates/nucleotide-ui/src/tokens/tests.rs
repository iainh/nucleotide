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

        let surface_color = hsla(0.0, 0.0, 0.05, 1.0); // Dark surface (chrome)
        let editor_bg = helix_colors.gutter_background; // Use gutter background for editor content
        let tokens =
            DesignTokens::from_helix_and_surface(helix_colors, surface_color, editor_bg, true);

        // Test that all component token generators work
        let titlebar = tokens.titlebar_tokens();
        let file_tree = tokens.file_tree_tokens();
        let status_bar = tokens.status_bar_tokens();
        let tab_bar = tokens.tab_bar_tokens();

        // Verify that chrome backgrounds are computed (different from Helix colors)
        assert_ne!(titlebar.background, helix_colors.statusline);
        assert_ne!(file_tree.background, helix_colors.gutter_background);
        assert_ne!(status_bar.background_active, helix_colors.statusline);
        assert_ne!(
            tab_bar.container_background,
            helix_colors.bufferline_background
        );

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
        let bg_text_contrast =
            ColorTheory::contrast_ratio(file_tree.background, file_tree.item_text);
        assert!(
            bg_text_contrast >= 4.5,
            "File tree background/text contrast {:.2} below WCAG AA (4.5:1)",
            bg_text_contrast
        );

        // Test hover state contrast
        let hover_contrast =
            ColorTheory::contrast_ratio(file_tree.item_background_hover, file_tree.item_text);
        assert!(
            hover_contrast >= 3.0, // Relaxed for interactive elements
            "File tree hover contrast {:.2} below minimum (3.0:1)",
            hover_contrast
        );

        println!(
            "File tree contrast ratios: bg/text={:.2}, hover={:.2} ✓",
            bg_text_contrast, hover_contrast
        );
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
        let active_contrast =
            ColorTheory::contrast_ratio(status_bar.background_active, status_bar.text_primary);
        let inactive_contrast =
            ColorTheory::contrast_ratio(status_bar.background_inactive, status_bar.text_secondary);

        assert!(active_contrast >= 4.5);
        assert!(inactive_contrast >= 3.0); // Relaxed for inactive state

        println!(
            "Status bar contrast: active={:.2}, inactive={:.2} ✓",
            active_contrast, inactive_contrast
        );
    }

    /// Test tab bar active/inactive distinction
    #[test]
    fn test_tab_bar_states() {
        let tokens = DesignTokens::dark();
        let tab_bar = tokens.tab_bar_tokens();

        // Verify tab states are visually distinct
        assert_ne!(
            tab_bar.tab_active_background,
            tab_bar.tab_inactive_background
        );
        assert_ne!(tab_bar.tab_active_background, tab_bar.container_background);
        assert_ne!(tab_bar.tab_text_active, tab_bar.tab_text_inactive);

        // Test active tab contrast (should be high for readability)
        let active_contrast =
            ColorTheory::contrast_ratio(tab_bar.tab_active_background, tab_bar.tab_text_active);
        assert!(
            active_contrast >= 7.0, // Higher standard for active tabs
            "Active tab contrast {:.2} below enhanced standard (7.0:1)",
            active_contrast
        );

        // Test inactive tab contrast (can be lower)
        let inactive_contrast =
            ColorTheory::contrast_ratio(tab_bar.tab_inactive_background, tab_bar.tab_text_inactive);
        assert!(
            inactive_contrast >= 3.0,
            "Inactive tab contrast {:.2} below minimum (3.0:1)",
            inactive_contrast
        );

        println!(
            "Tab bar contrast: active={:.2}, inactive={:.2} ✓",
            active_contrast, inactive_contrast
        );
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
            assert_eq!(
                status_bar.background_active,
                tokens.chrome.footer_background
            );
            assert_eq!(
                tab_bar.container_background,
                tokens.chrome.tab_empty_background
            );

            // Verify titlebar and status bar have matching backgrounds (USER FIX)
            assert_eq!(
                titlebar.background, status_bar.background_active,
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
        assert_eq!(
            status_bar_ext.background_active,
            status_bar_conv.background_active
        );
        assert_eq!(
            tab_bar_ext.container_background,
            tab_bar_conv.container_background
        );

        println!("Extension method consistency validated ✓");
    }

    /// Test button token functionality and color integration
    #[test]
    fn test_button_tokens() {
        let tokens = DesignTokens::dark();
        let button_tokens = tokens.button_tokens();

        // Test that primary buttons use chrome colors
        assert_ne!(button_tokens.primary_background, tokens.editor.background);

        // Test that semantic buttons use Helix editor colors
        assert_eq!(button_tokens.danger_background, tokens.editor.error);
        assert_eq!(button_tokens.success_background, tokens.editor.success);
        assert_eq!(button_tokens.warning_background, tokens.editor.warning);
        assert_eq!(button_tokens.info_background, tokens.editor.info);

        // Test that focus rings use Helix focus colors
        assert_eq!(button_tokens.focus_ring, tokens.editor.focus_ring);
        assert_eq!(
            button_tokens.focus_ring_danger,
            tokens.editor.focus_ring_error
        );

        // Test contrast ratios for primary button
        let primary_contrast = ColorTheory::contrast_ratio(
            button_tokens.primary_background,
            button_tokens.primary_text,
        );
        assert!(
            primary_contrast >= 4.5,
            "Primary button contrast {:.2} below WCAG AA (4.5:1)",
            primary_contrast
        );

        // Test contrast ratios for semantic buttons
        let danger_contrast =
            ColorTheory::contrast_ratio(button_tokens.danger_background, button_tokens.danger_text);
        assert!(
            danger_contrast >= 4.5,
            "Danger button contrast {:.2} below WCAG AA (4.5:1)",
            danger_contrast
        );

        println!("Button tokens validation passed ✓");
    }

    /// Test button token color states and hover interactions
    #[test]
    fn test_button_token_states() {
        let tokens = DesignTokens::light();
        let button_tokens = tokens.button_tokens();

        // Test that hover states are different from base states
        assert_ne!(
            button_tokens.primary_background,
            button_tokens.primary_background_hover
        );
        assert_ne!(
            button_tokens.secondary_background,
            button_tokens.secondary_background_hover
        );
        assert_ne!(
            button_tokens.ghost_background_hover,
            button_tokens.ghost_background // should be transparent
        );

        // Test that ghost button is transparent
        assert_eq!(button_tokens.ghost_background.a, 0.0);
        assert!(button_tokens.ghost_background_hover.a > 0.0);

        // Test that disabled states have reduced opacity
        assert!(button_tokens.disabled_background.a < 1.0);
        assert!(button_tokens.disabled_text.a < 1.0);

        println!("Button token states validation passed ✓");
    }

    /// Test picker token functionality and color integration
    #[test]
    fn test_picker_tokens() {
        let tokens = DesignTokens::dark();
        let picker_tokens = tokens.picker_tokens();

        // Test that container uses chrome colors
        assert_eq!(
            picker_tokens.container_background,
            tokens.chrome.surface_elevated
        );
        assert_eq!(
            picker_tokens.header_background,
            tokens.chrome.titlebar_background
        );

        // Test that items use Helix selection colors for familiarity
        assert_eq!(
            picker_tokens.item_background_selected,
            tokens.editor.selection_primary
        );
        assert_eq!(
            picker_tokens.item_text_selected,
            tokens.editor.text_on_primary
        );

        // Test that input focus uses Helix focus colors
        assert_eq!(picker_tokens.input_border_focus, tokens.editor.focus_ring);

        // Test contrast ratios
        let container_contrast = ColorTheory::contrast_ratio(
            picker_tokens.container_background,
            picker_tokens.item_text,
        );
        assert!(
            container_contrast >= 4.5,
            "Picker container contrast {:.2} below WCAG AA (4.5:1)",
            container_contrast
        );

        // Test overlay transparency
        assert!(picker_tokens.overlay_background.a < 1.0);
        assert!(picker_tokens.shadow.a < 1.0);

        println!("Picker tokens validation passed ✓");
    }

    /// Test dropdown token functionality and color integration
    #[test]
    fn test_dropdown_tokens() {
        let tokens = DesignTokens::light();
        let dropdown_tokens = tokens.dropdown_tokens();

        // Test that container uses chrome colors for UI consistency
        assert_eq!(
            dropdown_tokens.container_background,
            tokens.chrome.surface_elevated
        );
        assert_eq!(dropdown_tokens.border, tokens.chrome.border_strong);

        // Test that items use Helix selection colors
        assert_eq!(
            dropdown_tokens.item_background_selected,
            tokens.editor.selection_primary
        );
        assert_eq!(
            dropdown_tokens.item_text_selected,
            tokens.editor.text_on_primary
        );

        // Test that trigger uses chrome colors
        assert_eq!(
            dropdown_tokens.trigger_background,
            tokens.chrome.surface_hover
        );

        // Test that separators use consistent chrome colors
        assert_eq!(dropdown_tokens.separator, tokens.chrome.separator_color);

        // Test contrast ratios for accessibility
        let item_contrast = ColorTheory::contrast_ratio(
            dropdown_tokens.container_background,
            dropdown_tokens.item_text,
        );
        assert!(
            item_contrast >= 4.5,
            "Dropdown item contrast {:.2} below WCAG AA (4.5:1)",
            item_contrast
        );

        // Test trigger button contrast
        let trigger_contrast = ColorTheory::contrast_ratio(
            dropdown_tokens.trigger_background,
            dropdown_tokens.trigger_text,
        );
        assert!(
            trigger_contrast >= 4.5,
            "Dropdown trigger contrast {:.2} below WCAG AA (4.5:1)",
            trigger_contrast
        );

        // Test disabled states have reduced opacity
        assert!(dropdown_tokens.item_text_disabled.a < 1.0);
        assert!(dropdown_tokens.icon_color_disabled.a < 1.0);

        println!("Dropdown tokens validation passed ✓");
    }

    /// Test component token consistency across all new components
    #[test]
    fn test_new_component_token_consistency() {
        let test_themes = [
            ("Light", DesignTokens::light()),
            ("Dark", DesignTokens::dark()),
        ];

        for (theme_name, tokens) in test_themes {
            let button_tokens = tokens.button_tokens();
            let picker_tokens = tokens.picker_tokens();
            let dropdown_tokens = tokens.dropdown_tokens();

            // Test that all components use consistent chrome colors for containers
            assert_eq!(
                picker_tokens.container_background, tokens.chrome.surface_elevated,
                "{} theme: picker container should use chrome surface_raised",
                theme_name
            );
            assert_eq!(
                dropdown_tokens.container_background, tokens.chrome.surface_elevated,
                "{} theme: dropdown container should use chrome surface_raised",
                theme_name
            );

            // Test that all components use consistent Helix selection colors
            assert_eq!(
                button_tokens.danger_background, tokens.editor.error,
                "{} theme: button danger should use Helix error color",
                theme_name
            );
            assert_eq!(
                picker_tokens.item_background_selected, tokens.editor.selection_primary,
                "{} theme: picker selection should use Helix selection",
                theme_name
            );
            assert_eq!(
                dropdown_tokens.item_background_selected, tokens.editor.selection_primary,
                "{} theme: dropdown selection should use Helix selection",
                theme_name
            );

            // Test that all components use consistent separator colors
            assert_eq!(
                picker_tokens.separator, tokens.chrome.separator_color,
                "{} theme: picker separator should use chrome separator",
                theme_name
            );
            assert_eq!(
                dropdown_tokens.separator, tokens.chrome.separator_color,
                "{} theme: dropdown separator should use chrome separator",
                theme_name
            );

            println!("{} theme new component consistency validated ✓", theme_name);
        }
    }

    #[test]
    fn test_input_tokens() {
        let tokens = DesignTokens::dark();

        let input_tokens = tokens.input_tokens();

        // Test basic structure
        assert_ne!(input_tokens.background, input_tokens.background_hover);
        assert_ne!(input_tokens.background, input_tokens.background_focus);
        assert_ne!(input_tokens.text, input_tokens.placeholder);

        // Test focus uses Helix colors
        assert_eq!(input_tokens.focus_ring, tokens.editor.focus_ring);
        assert_eq!(input_tokens.border_focus, tokens.editor.focus_ring);

        // Test error uses Helix colors
        assert_eq!(input_tokens.border_error, tokens.editor.error);
        assert_eq!(input_tokens.error_text, tokens.editor.error);

        // Test disabled states have reduced opacity
        assert!(input_tokens.background_disabled.a < input_tokens.background.a);
        assert!(input_tokens.text_disabled.a < input_tokens.text.a);

        println!("✓ Input tokens validation passed");
    }

    #[test]
    fn test_tooltip_tokens() {
        let tokens = DesignTokens::dark();

        let tooltip_tokens = tokens.tooltip_tokens();

        // Test uses elevated surface
        assert_eq!(tooltip_tokens.background, tokens.chrome.surface_elevated);
        assert_eq!(
            tooltip_tokens.arrow_background,
            tokens.chrome.surface_elevated
        );

        // Test border consistency
        assert_eq!(tooltip_tokens.border, tokens.chrome.border_strong);
        assert_eq!(tooltip_tokens.arrow_border, tokens.chrome.border_strong);

        // Test shadow has transparency
        assert!(tooltip_tokens.shadow.a < 1.0);

        println!("✓ Tooltip tokens validation passed");
    }

    #[test]
    fn test_notification_tokens() {
        let tokens = DesignTokens::dark();

        let notification_tokens = tokens.notification_tokens();

        // Test semantic colors use Helix colors
        assert_eq!(notification_tokens.success_border, tokens.editor.success);
        assert_eq!(notification_tokens.warning_border, tokens.editor.warning);
        assert_eq!(notification_tokens.error_border, tokens.editor.error);

        // Test semantic backgrounds have transparency
        assert!(notification_tokens.success_background.a < 1.0);
        assert!(notification_tokens.warning_background.a < 1.0);
        assert!(notification_tokens.error_background.a < 1.0);

        // Test info uses chrome colors
        assert_eq!(notification_tokens.info_border, tokens.chrome.border_strong);

        // Test close button has hover state
        assert!(notification_tokens.close_button_background_hover.a > 0.0);

        println!("✓ Notification tokens validation passed");
    }

    #[test]
    fn test_advanced_component_token_consistency() {
        let tokens = DesignTokens::dark();

        let input_tokens = tokens.input_tokens();
        let tooltip_tokens = tokens.tooltip_tokens();
        let notification_tokens = tokens.notification_tokens();

        // Test focus consistency across components
        assert_eq!(input_tokens.focus_ring, tokens.editor.focus_ring);
        assert_eq!(input_tokens.border_focus, tokens.editor.focus_ring);

        // Test error consistency across components
        assert_eq!(input_tokens.error_text, tokens.editor.error);
        assert_eq!(notification_tokens.error_border, tokens.editor.error);

        // Test elevated surfaces consistency
        assert_eq!(tooltip_tokens.background, tokens.chrome.surface_elevated);

        // Test semantic color consistency
        assert_eq!(notification_tokens.success_border, tokens.editor.success);
        assert_eq!(notification_tokens.warning_border, tokens.editor.warning);

        println!("✓ Advanced component token consistency validation passed");
    }
}
