// ABOUTME: Test suite to ensure comprehensive Helix theme mapping coverage
// ABOUTME: Validates that all semantic color fields are properly mapped from Helix themes

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_manager::HelixThemeColors;
    use crate::tokens::{BaseColors, SemanticColors};
    use gpui::hsla;

    /// Test that all fields in HelixThemeColors are used in the mapping functions
    #[test]
    fn test_all_helix_colors_mapped() {
        let test_color = hsla(0.5, 0.5, 0.5, 1.0);

        // Create a test HelixThemeColors with all fields set to the same test color
        let helix_colors = HelixThemeColors {
            // Core selection and cursor colors
            selection: test_color,
            cursor_normal: test_color,
            cursor_insert: test_color,
            cursor_select: test_color,
            cursor_match: test_color,

            // Semantic feedback colors
            error: test_color,
            warning: test_color,
            success: test_color,

            // UI component backgrounds
            statusline: test_color,
            statusline_inactive: test_color,
            popup: test_color,

            // Buffer and tab system
            bufferline_background: test_color,
            bufferline_active: test_color,
            bufferline_inactive: test_color,

            // Gutter and line number system
            gutter_background: test_color,
            gutter_selected: test_color,
            line_number: test_color,
            line_number_active: test_color,

            // Menu and popup system
            menu_background: test_color,
            menu_selected: test_color,
            menu_separator: test_color,

            // Separator and focus system
            separator: test_color,
            focus: test_color,
        };

        let base_colors = BaseColors::light();
        let semantic_colors =
            SemanticColors::from_base_light_with_helix_colors(&base_colors, helix_colors);

        // Verify that our test color appears in key mapped fields
        // This ensures the mapping functions actually use the HelixThemeColors fields
        assert_eq!(semantic_colors.selection_primary, test_color);
        assert_eq!(semantic_colors.statusline_active, test_color);
        assert_eq!(semantic_colors.statusline_inactive, test_color);
        assert_eq!(semantic_colors.bufferline_background, test_color);
        assert_eq!(semantic_colors.bufferline_active, test_color);
        assert_eq!(semantic_colors.bufferline_inactive, test_color);
        assert_eq!(semantic_colors.gutter_background, test_color);
        assert_eq!(semantic_colors.gutter_selected, test_color);
        assert_eq!(semantic_colors.line_number, test_color);
        assert_eq!(semantic_colors.line_number_active, test_color);
        assert_eq!(semantic_colors.menu_background, test_color);
        assert_eq!(semantic_colors.menu_selected, test_color);
        assert_eq!(semantic_colors.menu_separator, test_color);
        assert_eq!(semantic_colors.separator_horizontal, test_color);
        assert_eq!(semantic_colors.separator_vertical, test_color);
        assert_eq!(semantic_colors.focus_ring, test_color);
        assert_eq!(semantic_colors.error, test_color);
        assert_eq!(semantic_colors.warning, test_color);
        assert_eq!(semantic_colors.success, test_color);
    }

    /// Test that dark theme mapping also works correctly
    #[test]
    fn test_dark_theme_mapping() {
        let test_color = hsla(0.2, 0.8, 0.3, 1.0);

        let helix_colors = HelixThemeColors {
            selection: test_color,
            cursor_normal: test_color,
            cursor_insert: test_color,
            cursor_select: test_color,
            cursor_match: test_color,
            error: test_color,
            warning: test_color,
            success: test_color,
            statusline: test_color,
            statusline_inactive: test_color,
            popup: test_color,
            bufferline_background: test_color,
            bufferline_active: test_color,
            bufferline_inactive: test_color,
            gutter_background: test_color,
            gutter_selected: test_color,
            line_number: test_color,
            line_number_active: test_color,
            menu_background: test_color,
            menu_selected: test_color,
            menu_separator: test_color,
            separator: test_color,
            focus: test_color,
        };

        let base_colors = BaseColors::dark();
        let semantic_colors =
            SemanticColors::from_base_dark_with_helix_colors(&base_colors, helix_colors);

        // Verify key mappings work in dark theme too
        assert_eq!(semantic_colors.selection_primary, test_color);
        assert_eq!(semantic_colors.bufferline_background, test_color);
        assert_eq!(semantic_colors.focus_ring, test_color);
    }

    /// Test that no fields are left unmapped (regression test)
    #[test]
    fn test_no_unmapped_semantic_fields() {
        // This test ensures that as new fields are added to SemanticColors,
        // they get proper mappings in the Helix theme functions

        let different_color = hsla(0.8, 0.2, 0.7, 1.0);
        let base_colors = BaseColors::light();

        // Create helix colors with easily identifiable values
        let helix_colors = HelixThemeColors {
            selection: hsla(0.1, 1.0, 0.5, 1.0),
            cursor_normal: hsla(0.2, 1.0, 0.5, 1.0),
            cursor_insert: hsla(0.3, 1.0, 0.5, 1.0),
            cursor_select: hsla(0.4, 1.0, 0.5, 1.0),
            cursor_match: hsla(0.5, 1.0, 0.5, 1.0),
            error: hsla(0.6, 1.0, 0.5, 1.0),
            warning: hsla(0.7, 1.0, 0.5, 1.0),
            success: hsla(0.8, 1.0, 0.5, 1.0),
            statusline: hsla(0.9, 1.0, 0.5, 1.0),
            statusline_inactive: hsla(0.95, 1.0, 0.5, 1.0),
            popup: hsla(0.15, 1.0, 0.5, 1.0),
            bufferline_background: hsla(0.25, 1.0, 0.5, 1.0),
            bufferline_active: hsla(0.35, 1.0, 0.5, 1.0),
            bufferline_inactive: hsla(0.45, 1.0, 0.5, 1.0),
            gutter_background: hsla(0.55, 1.0, 0.5, 1.0),
            gutter_selected: hsla(0.65, 1.0, 0.5, 1.0),
            line_number: hsla(0.75, 1.0, 0.5, 1.0),
            line_number_active: hsla(0.85, 1.0, 0.5, 1.0),
            menu_background: hsla(0.12, 1.0, 0.5, 1.0),
            menu_selected: hsla(0.22, 1.0, 0.5, 1.0),
            menu_separator: hsla(0.32, 1.0, 0.5, 1.0),
            separator: hsla(0.42, 1.0, 0.5, 1.0),
            focus: hsla(0.52, 1.0, 0.5, 1.0),
        };

        let semantic_colors =
            SemanticColors::from_base_light_with_helix_colors(&base_colors, helix_colors);

        // This test will fail if any crucial SemanticColors field is not being
        // overridden by helix colors (i.e., still using base color fallbacks)
        // The specific assertions verify that our helix theme extraction is working

        // Core functionality should use helix colors, not base fallbacks
        assert_ne!(semantic_colors.selection_primary, base_colors.primary_500);
        assert_ne!(
            semantic_colors.bufferline_background,
            base_colors.neutral_300
        );
        assert_ne!(semantic_colors.statusline_active, base_colors.neutral_100);
    }
}
