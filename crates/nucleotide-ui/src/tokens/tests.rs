// ABOUTME: WCAG contrast compliance tests for TitleBarTokens
// ABOUTME: Ensures titlebar colors meet accessibility standards across all themes

use crate::DesignTokens;
use crate::styling::{ColorTheory, ContrastRatios};
use crate::tokens::{
    ChromeTokens, ColorContext, ControlDensity, DensityMetrics, EditorTokens, SizeTokens,
    TitleBarTokens,
};
use nucleotide_appearance::{
    HelixThemeColors, NativeChromePalette, SystemAppearance, default_windows_accent_color,
};

#[cfg(test)]
mod typography_token_tests {
    use super::*;

    #[test]
    fn text_scale_is_centered_on_configured_ui_font_size() {
        let tokens = SizeTokens::with_text_md(gpui::px(13.0));

        assert_eq!(tokens.text_xs, gpui::px(11.0));
        assert_eq!(tokens.text_sm, gpui::px(12.0));
        assert_eq!(tokens.text_base, gpui::px(13.0));
        assert_eq!(tokens.text_md, gpui::px(13.0));
        assert_eq!(tokens.text_lg, gpui::px(14.0));
        assert_eq!(tokens.text_xl, gpui::px(15.0));
    }
}

#[cfg(test)]
mod density_token_tests {
    use super::*;

    #[test]
    fn density_profiles_share_stable_chrome_metrics() {
        let compact = DensityMetrics::for_density(ControlDensity::Compact);
        let comfortable = DensityMetrics::for_density(ControlDensity::Comfortable);
        let relaxed = DensityMetrics::for_density(ControlDensity::Relaxed);

        assert_eq!(compact.row_height, gpui::px(28.0));
        assert_eq!(comfortable.row_height, gpui::px(32.0));
        assert_eq!(relaxed.row_height, gpui::px(36.0));
        assert_eq!(comfortable.icon_size, gpui::px(16.0));
        assert_eq!(comfortable.icon_slot, gpui::px(24.0));
        assert!(compact.padding_x < comfortable.padding_x);
        assert!(comfortable.padding_x < relaxed.padding_x);
    }
}

#[cfg(test)]
mod system_chrome_token_tests {
    use super::*;
    use gpui::hsla;

    #[test]
    fn system_chrome_uses_fluent_neutral_surfaces() {
        let editor_background = hsla(0.0, 0.0, 0.08, 1.0);
        let system_accent = default_windows_accent_color();
        let light = ChromeTokens::from_native_chrome_palette(NativeChromePalette::windows_fluent(
            SystemAppearance::Light,
            system_accent,
        ));
        let dark = ChromeTokens::from_native_chrome_palette(NativeChromePalette::windows_fluent(
            SystemAppearance::Dark,
            system_accent,
        ));

        assert!(light.surface.l > dark.surface.l);
        assert!(light.titlebar_background.l > dark.titlebar_background.l);
        assert!(light.primary.s > 0.8);
        assert!(dark.primary.l > light.primary.l);
        assert_eq!(light.tab_empty_background, light.titlebar_background);
        assert_eq!(dark.tab_empty_background, dark.titlebar_background);
        assert_eq!(light.bufferline_background, light.titlebar_background);
        assert_eq!(dark.bufferline_background, dark.titlebar_background);
        assert_eq!(light.bufferline_active, light.surface_elevated);
        assert_eq!(dark.bufferline_active, dark.surface_elevated);
        assert_eq!(light.bufferline_inactive, light.titlebar_background);
        assert_eq!(dark.bufferline_inactive, dark.titlebar_background);
        assert_ne!(light.bufferline_active, editor_background);
        assert_ne!(dark.bufferline_active, editor_background);
        let light_tabs = light.tab_bar_tokens(&EditorTokens::fallback(false));
        let dark_tabs = dark.tab_bar_tokens(&EditorTokens::fallback(true));
        assert_eq!(light_tabs.container_background, light.bufferline_background);
        assert_eq!(dark_tabs.container_background, dark.bufferline_background);
        assert_eq!(light_tabs.tab_active_background, light.bufferline_active);
        assert_eq!(dark_tabs.tab_active_background, dark.bufferline_active);
        assert_eq!(
            light_tabs.tab_inactive_background,
            light.bufferline_inactive
        );
        assert_eq!(dark_tabs.tab_inactive_background, dark.bufferline_inactive);
        assert!(light.popup_background.a < 1.0);
        assert!(dark.popup_background.a < 1.0);
        assert!(light.popup_background.a >= 0.94);
        assert!(dark.popup_background.a >= 0.94);
        assert!(light.menu_background.a >= 0.94);
        assert!(dark.menu_background.a >= 0.94);

        assert_eq!(light.titlebar_background.a, 0.0);
        assert_eq!(dark.titlebar_background.a, 0.0);
        assert_eq!(light.footer_background.a, 0.0);
        assert_eq!(dark.footer_background.a, 0.0);
        assert_eq!(light.bufferline_background.a, 0.0);
        assert_eq!(dark.bufferline_background.a, 0.0);
        assert_eq!(light.bufferline_inactive.a, 0.0);
        assert_eq!(dark.bufferline_inactive.a, 0.0);
        assert!(light.file_tree_background.a >= 0.56);
        assert!(light.file_tree_background.a <= 0.62);
        assert!(dark.file_tree_background.a >= 0.60);
        assert!(dark.file_tree_background.a <= 0.66);
        assert!(light.surface.a <= 0.6);
        assert!(dark.surface.a <= 0.65);
    }

    #[test]
    fn system_chrome_accent_can_follow_platform_color() {
        let platform_accent = hsla(300.0 / 360.0, 0.70, 0.45, 1.0);
        let system = ChromeTokens::from_native_chrome_palette(NativeChromePalette::windows_fluent(
            SystemAppearance::Light,
            platform_accent,
        ));

        assert!((system.primary.h - platform_accent.h).abs() < 0.08);
        assert_eq!(system.border_focus, system.primary);
        assert_eq!(system.surface_selected.h, system.primary.h);
    }

    #[test]
    fn system_chrome_button_hovers_keep_native_overlay_alpha() {
        let chrome = ChromeTokens::from_native_chrome_palette(NativeChromePalette::windows_fluent(
            SystemAppearance::Light,
            default_windows_accent_color(),
        ));
        let buttons = chrome.button_tokens(&EditorTokens::fallback(false));

        assert!(chrome.surface_hover.a <= 0.05);
        assert_eq!(buttons.secondary_background_hover.a, chrome.surface_hover.a);
        assert_eq!(buttons.ghost_background_hover.a, chrome.surface_hover.a);
        assert_eq!(buttons.disabled_background.a, chrome.surface_hover.a);
    }
}

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
        let helix_colors = HelixThemeColors {
            selection: gpui::hsla(220.0 / 360.0, 0.7, 0.6, 1.0), // Blue selection
            cursor_normal: gpui::hsla(220.0 / 360.0, 0.8, 0.5, 1.0),
            cursor_insert: gpui::hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
            cursor_select: gpui::hsla(40.0 / 360.0, 0.8, 0.5, 1.0),
            cursor_match: gpui::hsla(200.0 / 360.0, 0.7, 0.5, 1.0),
            error: gpui::hsla(0.0, 0.8, 0.5, 1.0),
            warning: gpui::hsla(40.0 / 360.0, 0.8, 0.5, 1.0),
            success: gpui::hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
            vcs_added: gpui::hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
            vcs_modified: gpui::hsla(210.0 / 360.0, 0.7, 0.5, 1.0),
            vcs_deleted: gpui::hsla(0.0, 0.8, 0.5, 1.0),
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
            text_primary: gpui::hsla(0.0, 0.0, 0.10, 1.0),
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

    #[test]
    fn native_chrome_uses_platform_titlebar_geometry() {
        let sizes = SizeTokens::native_chrome();

        #[cfg(target_os = "macos")]
        assert_eq!(sizes.titlebar_height, gpui::px(44.0));

        #[cfg(not(target_os = "macos"))]
        assert_eq!(sizes.titlebar_height, SizeTokens::default().titlebar_height);
    }

    /// Test that status bar height is intentionally compact and independent of titlebar height
    #[test]
    fn test_statusbar_height_is_compact() {
        let tokens = DesignTokens::light();

        assert_eq!(tokens.sizes.statusbar_height, gpui::px(32.0));
        assert!(tokens.sizes.statusbar_height < tokens.sizes.titlebar_height);
        assert_eq!(tokens.sizes.statusbar_mode_width, gpui::px(68.0));
        assert_eq!(tokens.sizes.statusbar_environment_width, gpui::px(32.0));
        assert_eq!(tokens.sizes.statusbar_position_width, gpui::px(48.0));
        assert_eq!(tokens.sizes.statusbar_lsp_width_wide, gpui::px(132.0));
        assert_eq!(tokens.sizes.statusbar_lsp_width_medium, gpui::px(112.0));
        assert_eq!(tokens.sizes.statusbar_lsp_width_compact, gpui::px(36.0));
        assert_eq!(tokens.sizes.statusbar_utility_width, gpui::px(64.0));
    }
}

#[cfg(test)]
mod component_token_tests {
    use super::*;
    use gpui::hsla;

    /// Test hybrid color system integration for component tokens
    #[test]
    fn test_hybrid_component_tokens() {
        let helix_colors = HelixThemeColors {
            selection: hsla(220.0 / 360.0, 0.7, 0.8, 0.3),
            cursor_normal: hsla(220.0 / 360.0, 0.7, 0.6, 1.0),
            cursor_insert: hsla(120.0 / 360.0, 0.7, 0.6, 1.0),
            cursor_select: hsla(280.0 / 360.0, 0.7, 0.6, 1.0),
            cursor_match: hsla(40.0 / 360.0, 0.7, 0.6, 1.0),
            error: hsla(0.0, 0.7, 0.6, 1.0),
            warning: hsla(40.0 / 360.0, 0.8, 0.6, 1.0),
            success: hsla(120.0 / 360.0, 0.6, 0.5, 1.0),
            vcs_added: hsla(145.0 / 360.0, 0.7, 0.5, 1.0),
            vcs_modified: hsla(205.0 / 360.0, 0.75, 0.55, 1.0),
            vcs_deleted: hsla(350.0 / 360.0, 0.8, 0.55, 1.0),
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
            text_primary: hsla(0.0, 0.0, 0.90, 1.0),
        };

        let surface_color = hsla(0.0, 0.0, 0.08, 1.0); // Dark surface (chrome)
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

        // Verify UI state colors use chrome/system tokens while editor content stays Helix-derived.
        assert_eq!(
            file_tree.item_background_selected,
            tokens.chrome.surface_selected
        );
        assert_eq!(file_tree.item_text_selected, tokens.chrome.text_on_chrome);
        assert_eq!(tab_bar.tab_active_background, tokens.editor.background);
        assert_eq!(tab_bar.tab_text_active, tokens.chrome.text_on_chrome);
        assert_eq!(tab_bar.tab_modified_indicator, helix_colors.warning);
        assert_eq!(tokens.editor.vcs_added, helix_colors.vcs_added);
        assert_eq!(tokens.editor.vcs_modified, helix_colors.vcs_modified);
        assert_eq!(tokens.editor.vcs_deleted, helix_colors.vcs_deleted);
        assert_eq!(file_tree.background, tab_bar.container_background);
        assert_ne!(file_tree.background, titlebar.background);

        let editor_l = ColorTheory::hsla_to_oklab(tokens.editor.background).L;
        let titlebar_l = ColorTheory::hsla_to_oklab(titlebar.background).L;
        let file_tree_l = ColorTheory::hsla_to_oklab(file_tree.background).L;
        assert!(
            file_tree_l > editor_l.min(titlebar_l) && file_tree_l < editor_l.max(titlebar_l),
            "file tree OKLab L {file_tree_l:.3} should sit between editor {editor_l:.3} and titlebar {titlebar_l:.3}"
        );

        // Status bar text should derive from chrome tokens rather than editor cursors
        let expected_accent = ColorTheory::ensure_contrast(
            tokens.chrome.footer_background,
            tokens.chrome.primary,
            ContrastRatios::AA_NORMAL,
        );
        assert_eq!(status_bar.text_primary, tokens.chrome.text_on_chrome);
        assert_eq!(status_bar.text_accent, expected_accent);
        assert_eq!(status_bar.mode_normal, tokens.chrome.primary);
        assert_eq!(status_bar.mode_text, tokens.editor.text_on_primary);
        assert_eq!(status_bar.mode_insert, tokens.chrome.primary_hover);
        assert_eq!(status_bar.mode_select, tokens.chrome.primary_active);
        assert_ne!(status_bar.mode_insert, helix_colors.cursor_insert);
        assert_ne!(status_bar.mode_select, helix_colors.cursor_select);

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

        let selected_contrast =
            ColorTheory::contrast_ratio(file_tree.background, file_tree.item_text_selected);
        assert!(
            selected_contrast >= 4.5,
            "File tree selected contrast {:.2} below WCAG AA (4.5:1)",
            selected_contrast
        );
        assert!(file_tree.item_background_selected.a > 0.0);
        assert!(file_tree.item_background_selected.a < 1.0);

        println!(
            "File tree contrast ratios: bg/text={:.2}, hover={:.2}, selected={:.2} ✓",
            bg_text_contrast, hover_contrast, selected_contrast
        );
    }

    #[test]
    fn translucent_sidebar_tokens_reduce_chrome_opacity() {
        let tokens = DesignTokens::dark();
        let file_tree = tokens.file_tree_tokens();
        let translucent = file_tree.translucent_sidebar();

        assert!(translucent.background.a < file_tree.background.a);
        assert!(translucent.item_background_hover.a < file_tree.item_background_hover.a);
        assert!(translucent.item_background_selected.a <= file_tree.item_background_selected.a);
        assert!(translucent.border.a <= file_tree.border.a);
        assert!(translucent.separator.a <= file_tree.separator.a);
        assert!(translucent.background.a < 1.0);

        assert_eq!(translucent.item_text, file_tree.item_text);
        assert_eq!(
            translucent.item_text_secondary,
            file_tree.item_text_secondary
        );
        assert_eq!(translucent.item_text_selected, file_tree.item_text_selected);
        assert_eq!(translucent.icon_color, file_tree.icon_color);
        assert_eq!(
            translucent.icon_color_selected,
            file_tree.icon_color_selected
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

        // Mode colors should align with chrome accents
        assert_ne!(status_bar.mode_normal, status_bar.text_primary);

        let expected_accent = ColorTheory::ensure_contrast(
            tokens.chrome.footer_background,
            tokens.chrome.primary,
            ContrastRatios::AA_NORMAL,
        );
        assert_eq!(status_bar.text_accent, expected_accent);
        assert_eq!(status_bar.mode_insert, tokens.chrome.primary_hover);
        assert_eq!(status_bar.mode_select, tokens.chrome.primary_active);

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
        assert_eq!(tab_bar.tab_active_background, tokens.editor.background);
        assert_eq!(tab_bar.tab_text_active, tokens.chrome.text_on_chrome);
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

            // Verify outer component edges use the shaded border color.
            assert_eq!(titlebar.border, tokens.chrome.border_shadow);
            assert_eq!(file_tree.border, tokens.chrome.border_shadow);
            assert_eq!(status_bar.border, tokens.chrome.border_shadow);
            assert_eq!(tab_bar.tab_border, tokens.chrome.border_shadow);

            // Verify internal separators remain consistent.
            assert_eq!(file_tree.separator, tokens.chrome.separator_color);
            assert_eq!(tab_bar.tab_separator, tokens.chrome.separator_color);

            // Verify row content states are represented by file-tree tokens.
            assert_eq!(file_tree.item_text, tokens.chrome.text_on_chrome);
            assert_eq!(
                file_tree.item_text_secondary,
                tokens.chrome.text_chrome_secondary
            );
            assert_eq!(
                file_tree.item_text_hidden,
                tokens.chrome.text_chrome_disabled
            );
            assert_eq!(file_tree.item_text_selected, tokens.chrome.text_on_chrome);
            assert_eq!(file_tree.icon_color, tokens.chrome.text_chrome_secondary);
            assert_eq!(
                file_tree.icon_color_secondary,
                tokens.chrome.text_chrome_disabled
            );
            assert_eq!(file_tree.icon_color_selected, tokens.chrome.text_on_chrome);
            assert_eq!(
                file_tree.icon_color_hidden,
                tokens.chrome.text_chrome_disabled
            );

            // Verify chrome backgrounds are computed correctly
            assert_eq!(titlebar.background, tokens.chrome.titlebar_background);
            assert_eq!(file_tree.background, tokens.chrome.file_tree_background);
            assert_eq!(
                status_bar.background_active,
                tokens.chrome.footer_background
            );
            assert_eq!(
                tab_bar.container_background,
                tokens.chrome.bufferline_background
            );
            assert_eq!(tokens.chrome.bufferline_active, tokens.editor.background);
            assert_eq!(tab_bar.tab_active_background, tokens.editor.background);
            assert_eq!(tab_bar.tab_text_active, tokens.chrome.text_on_chrome);

            // Verify titlebar and status bar have matching backgrounds (USER FIX)
            assert_eq!(
                titlebar.background, status_bar.background_active,
                "{} theme: titlebar background ({:?}) should match status bar background ({:?})",
                theme_name, titlebar.background, status_bar.background_active
            );

            println!("{} theme component consistency validated ✓", theme_name);
        }
    }

    /// Test border shading and shadow depth tokens across themes.
    #[test]
    fn test_chrome_depth_tokens() {
        let test_themes = [
            ("Light", DesignTokens::light()),
            ("Dark", DesignTokens::dark()),
        ];

        for (theme_name, tokens) in test_themes {
            assert_ne!(
                tokens.chrome.border_highlight, tokens.chrome.border_shadow,
                "{} theme should provide distinct highlight and shadow border shades",
                theme_name
            );
            assert!(tokens.chrome.border_highlight.a > 0.0);
            assert!(tokens.chrome.border_shadow.a > 0.0);

            let highlight_contrast =
                ColorTheory::contrast_ratio(tokens.chrome.surface, tokens.chrome.border_highlight);
            let shadow_contrast =
                ColorTheory::contrast_ratio(tokens.chrome.surface, tokens.chrome.border_shadow);
            assert!(
                highlight_contrast > 1.0,
                "{} theme highlight border should be visible against the surface",
                theme_name
            );
            assert!(
                shadow_contrast > 1.0,
                "{} theme shadow border should be visible against the surface",
                theme_name
            );

            assert!(tokens.chrome.shadow_sm.color.a > 0.0);
            assert!(tokens.chrome.shadow_md.color.a >= tokens.chrome.shadow_sm.color.a);
            assert!(tokens.chrome.shadow_lg.color.a >= tokens.chrome.shadow_md.color.a);
            assert!(
                f32::from(tokens.chrome.shadow_lg.blur_radius)
                    > f32::from(tokens.chrome.shadow_sm.blur_radius)
            );
            assert_eq!(f32::from(tokens.chrome.shadow_sm.spread_radius), 0.0);

            println!("{} theme depth tokens validated ✓", theme_name);
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

        // Test that focus rings use chrome/system focus colors
        assert_eq!(button_tokens.focus_ring, tokens.chrome.border_focus);
        assert_eq!(button_tokens.focus_ring_danger, tokens.chrome.border_focus);

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
            tokens.chrome.popup_background
        );
        assert_eq!(
            picker_tokens.header_background,
            tokens.chrome.titlebar_background
        );

        // Test that items use chrome/system selection colors
        assert_eq!(
            picker_tokens.item_background_selected,
            tokens.chrome.menu_selected
        );
        assert_eq!(
            picker_tokens.item_text_selected,
            tokens.chrome.text_on_chrome
        );

        // Test that input focus uses chrome/system focus colors
        assert_eq!(picker_tokens.input_border_focus, tokens.chrome.border_focus);

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
            tokens.chrome.menu_background
        );
        assert_eq!(dropdown_tokens.border, tokens.chrome.border_shadow);

        // Test that items use chrome/system selection colors
        assert_eq!(
            dropdown_tokens.item_background_selected,
            tokens.chrome.menu_selected
        );
        assert_eq!(
            dropdown_tokens.item_text_selected,
            tokens.chrome.text_on_chrome
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
                picker_tokens.container_background, tokens.chrome.popup_background,
                "{} theme: picker container should use chrome popup background",
                theme_name
            );
            assert_eq!(
                dropdown_tokens.container_background, tokens.chrome.menu_background,
                "{} theme: dropdown container should use chrome menu background",
                theme_name
            );

            // Test that all components use consistent chrome/system selection colors
            assert_eq!(
                button_tokens.danger_background, tokens.editor.error,
                "{} theme: button danger should use Helix error color",
                theme_name
            );
            assert_eq!(
                picker_tokens.item_background_selected, tokens.chrome.menu_selected,
                "{} theme: picker selection should use chrome selection",
                theme_name
            );
            assert_eq!(
                dropdown_tokens.item_background_selected, tokens.chrome.menu_selected,
                "{} theme: dropdown selection should use chrome selection",
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
        assert_eq!(input_tokens.background, tokens.editor.background);
        assert_eq!(input_tokens.background_hover, tokens.editor.background);
        assert_eq!(input_tokens.background_focus, tokens.editor.background);
        assert_ne!(input_tokens.text, input_tokens.placeholder);

        // Test focus uses chrome/system colors
        assert_eq!(input_tokens.focus_ring, tokens.chrome.border_focus);
        assert_eq!(input_tokens.border_focus, tokens.chrome.border_focus);

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
        assert_eq!(tooltip_tokens.border, tokens.chrome.border_shadow);
        assert_eq!(tooltip_tokens.arrow_border, tokens.chrome.border_shadow);

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
        assert_eq!(notification_tokens.info_border, tokens.chrome.border_shadow);

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
        assert_eq!(input_tokens.focus_ring, tokens.chrome.border_focus);
        assert_eq!(input_tokens.border_focus, tokens.chrome.border_focus);

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
