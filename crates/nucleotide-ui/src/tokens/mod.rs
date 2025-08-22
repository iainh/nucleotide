// ABOUTME: Design token system providing semantic color and spacing values
// ABOUTME: Replaces hardcoded values with systematic, theme-aware design tokens

use gpui::{Hsla, Pixels, hsla, px};
use nucleotide_logging::debug;

/// Base color palette - raw color definitions
#[derive(Debug, Clone, Copy)]
pub struct BaseColors {
    // Neutral colors
    pub neutral_50: Hsla,
    pub neutral_100: Hsla,
    pub neutral_200: Hsla,
    pub neutral_300: Hsla,
    pub neutral_400: Hsla,
    pub neutral_500: Hsla,
    pub neutral_600: Hsla,
    pub neutral_700: Hsla,
    pub neutral_800: Hsla,
    pub neutral_900: Hsla,
    pub neutral_950: Hsla,

    // Primary colors
    pub primary_50: Hsla,
    pub primary_100: Hsla,
    pub primary_200: Hsla,
    pub primary_300: Hsla,
    pub primary_400: Hsla,
    pub primary_500: Hsla,
    pub primary_600: Hsla,
    pub primary_700: Hsla,
    pub primary_800: Hsla,
    pub primary_900: Hsla,

    // Semantic colors
    pub success_500: Hsla,
    pub warning_500: Hsla,
    pub error_500: Hsla,
    pub info_500: Hsla,
}

impl BaseColors {
    /// Light theme base colors
    pub fn light() -> Self {
        Self {
            // Neutral scale (light theme)
            neutral_50: hsla(0.0, 0.0, 0.98, 1.0),
            neutral_100: hsla(0.0, 0.0, 0.96, 1.0),
            neutral_200: hsla(0.0, 0.0, 0.94, 1.0),
            neutral_300: hsla(0.0, 0.0, 0.91, 1.0),
            neutral_400: hsla(0.0, 0.0, 0.78, 1.0),
            neutral_500: hsla(0.0, 0.0, 0.64, 1.0),
            neutral_600: hsla(0.0, 0.0, 0.52, 1.0),
            neutral_700: hsla(0.0, 0.0, 0.42, 1.0),
            neutral_800: hsla(0.0, 0.0, 0.25, 1.0),
            neutral_900: hsla(0.0, 0.0, 0.15, 1.0),
            neutral_950: hsla(0.0, 0.0, 0.09, 1.0),

            // Primary scale (blue)
            primary_50: hsla(220.0 / 360.0, 0.95, 0.97, 1.0),
            primary_100: hsla(220.0 / 360.0, 0.88, 0.94, 1.0),
            primary_200: hsla(220.0 / 360.0, 0.83, 0.89, 1.0),
            primary_300: hsla(220.0 / 360.0, 0.78, 0.81, 1.0),
            primary_400: hsla(220.0 / 360.0, 0.70, 0.69, 1.0),
            primary_500: hsla(220.0 / 360.0, 0.62, 0.55, 1.0),
            primary_600: hsla(220.0 / 360.0, 0.58, 0.44, 1.0),
            primary_700: hsla(220.0 / 360.0, 0.55, 0.35, 1.0),
            primary_800: hsla(220.0 / 360.0, 0.50, 0.28, 1.0),
            primary_900: hsla(220.0 / 360.0, 0.45, 0.22, 1.0),

            // Semantic colors
            success_500: hsla(120.0 / 360.0, 0.60, 0.50, 1.0),
            warning_500: hsla(40.0 / 360.0, 0.80, 0.50, 1.0),
            error_500: hsla(0.0, 0.80, 0.50, 1.0),
            info_500: hsla(200.0 / 360.0, 0.70, 0.50, 1.0),
        }
    }

    /// Dark theme base colors
    pub fn dark() -> Self {
        Self {
            // Neutral scale (dark theme - inverted)
            neutral_50: hsla(0.0, 0.0, 0.05, 1.0),
            neutral_100: hsla(0.0, 0.0, 0.08, 1.0),
            neutral_200: hsla(0.0, 0.0, 0.12, 1.0),
            neutral_300: hsla(0.0, 0.0, 0.16, 1.0),
            neutral_400: hsla(0.0, 0.0, 0.24, 1.0),
            neutral_500: hsla(0.0, 0.0, 0.38, 1.0),
            neutral_600: hsla(0.0, 0.0, 0.52, 1.0),
            neutral_700: hsla(0.0, 0.0, 0.64, 1.0),
            neutral_800: hsla(0.0, 0.0, 0.78, 1.0),
            neutral_900: hsla(0.0, 0.0, 0.89, 1.0),
            neutral_950: hsla(0.0, 0.0, 0.95, 1.0),

            // Primary scale (same hue, adjusted for dark theme)
            primary_50: hsla(220.0 / 360.0, 0.45, 0.22, 1.0),
            primary_100: hsla(220.0 / 360.0, 0.50, 0.28, 1.0),
            primary_200: hsla(220.0 / 360.0, 0.55, 0.35, 1.0),
            primary_300: hsla(220.0 / 360.0, 0.58, 0.44, 1.0),
            primary_400: hsla(220.0 / 360.0, 0.62, 0.55, 1.0),
            primary_500: hsla(220.0 / 360.0, 0.70, 0.69, 1.0),
            primary_600: hsla(220.0 / 360.0, 0.78, 0.81, 1.0),
            primary_700: hsla(220.0 / 360.0, 0.83, 0.89, 1.0),
            primary_800: hsla(220.0 / 360.0, 0.88, 0.94, 1.0),
            primary_900: hsla(220.0 / 360.0, 0.95, 0.97, 1.0),

            // Semantic colors (slightly brighter for dark themes)
            success_500: hsla(120.0 / 360.0, 0.60, 0.60, 1.0),
            warning_500: hsla(40.0 / 360.0, 0.80, 0.60, 1.0),
            error_500: hsla(0.0, 0.80, 0.60, 1.0),
            info_500: hsla(200.0 / 360.0, 0.70, 0.60, 1.0),
        }
    }
}

/// Semantic color tokens - meaningful names for UI elements
#[derive(Debug, Clone, Copy)]
pub struct SemanticColors {
    // Surface colors
    pub background: Hsla,
    pub surface: Hsla,
    pub surface_elevated: Hsla,
    pub surface_overlay: Hsla,

    // Interactive states
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub surface_selected: Hsla,
    pub surface_disabled: Hsla,

    // Text colors
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub text_disabled: Hsla,
    pub text_on_primary: Hsla,

    // Border colors
    pub border_default: Hsla,
    pub border_muted: Hsla,
    pub border_strong: Hsla,
    pub border_focus: Hsla,

    // Brand colors
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub primary_active: Hsla,

    // Semantic feedback
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub info: Hsla,

    // Cursor and selection system
    pub cursor_normal: Hsla,
    pub cursor_insert: Hsla,
    pub cursor_select: Hsla,
    pub cursor_match: Hsla,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,

    // Enhanced diagnostic system
    pub diagnostic_error: Hsla,
    pub diagnostic_warning: Hsla,
    pub diagnostic_info: Hsla,
    pub diagnostic_hint: Hsla,
    pub diagnostic_error_bg: Hsla,
    pub diagnostic_warning_bg: Hsla,
    pub diagnostic_info_bg: Hsla,
    pub diagnostic_hint_bg: Hsla,

    // Gutter and line number system
    pub gutter_background: Hsla,
    pub gutter_selected: Hsla,
    pub line_number: Hsla,
    pub line_number_active: Hsla,

    // Enhanced status and buffer system
    pub statusline_active: Hsla,
    pub statusline_inactive: Hsla,
    pub bufferline_background: Hsla,
    pub bufferline_active: Hsla,
    pub bufferline_inactive: Hsla,

    // Enhanced popup and menu system
    pub popup_background: Hsla,
    pub popup_border: Hsla,
    pub menu_background: Hsla,
    pub menu_selected: Hsla,
    pub menu_separator: Hsla,

    // Separator and UI enhancement system
    pub separator_horizontal: Hsla,
    pub separator_vertical: Hsla,
    pub separator_subtle: Hsla,
    pub focus_ring: Hsla,
    pub focus_ring_error: Hsla,
    pub focus_ring_warning: Hsla,
}

impl SemanticColors {
    /// Create semantic colors from base colors for light theme
    pub fn from_base_light(base: &BaseColors) -> Self {
        Self {
            // Surface colors
            background: base.neutral_50,
            surface: base.neutral_100,
            surface_elevated: base.neutral_200,
            surface_overlay: hsla(0.0, 0.0, 1.0, 0.95),

            // Interactive states
            surface_hover: base.neutral_200,
            surface_active: base.neutral_300,
            surface_selected: base.primary_100,
            surface_disabled: base.neutral_100,

            // Text colors
            text_primary: base.neutral_900,
            text_secondary: base.neutral_700,
            text_tertiary: base.neutral_500,
            text_disabled: base.neutral_400,
            text_on_primary: base.neutral_50,

            // Border colors
            border_default: base.neutral_300,
            border_muted: base.neutral_200,
            border_strong: base.neutral_400,
            border_focus: base.primary_500,

            // Brand colors
            primary: base.primary_500,
            primary_hover: base.primary_600,
            primary_active: base.primary_700,

            // Semantic feedback
            success: base.success_500,
            warning: base.warning_500,
            error: base.error_500,
            info: base.info_500,

            // Cursor and selection system
            cursor_normal: base.primary_500,
            cursor_insert: base.success_500,
            cursor_select: base.warning_500,
            cursor_match: base.info_500,
            selection_primary: base.primary_100,
            selection_secondary: base.neutral_100,

            // Enhanced diagnostic system
            diagnostic_error: base.error_500,
            diagnostic_warning: base.warning_500,
            diagnostic_info: base.info_500,
            diagnostic_hint: base.neutral_600,
            diagnostic_error_bg: utils::with_alpha(base.error_500, 0.1),
            diagnostic_warning_bg: utils::with_alpha(base.warning_500, 0.1),
            diagnostic_info_bg: utils::with_alpha(base.info_500, 0.1),
            diagnostic_hint_bg: utils::with_alpha(base.neutral_600, 0.1),

            // Gutter and line number system
            gutter_background: base.neutral_50,
            gutter_selected: base.neutral_100,
            line_number: base.neutral_500,
            line_number_active: base.neutral_700,

            // Enhanced status and buffer system
            statusline_active: base.neutral_100,     // surface
            statusline_inactive: base.neutral_200,   // more distinct from active
            bufferline_background: base.neutral_300, // distinct tab bar background (91% lightness)
            bufferline_active: base.neutral_50, // background (active tab matches editor - 98% lightness)
            bufferline_inactive: base.neutral_400, // inactive tabs (78% lightness - high contrast with active)

            // Enhanced popup and menu system
            popup_background: base.neutral_200, // surface_elevated
            popup_border: base.neutral_300,     // border_default
            menu_background: base.neutral_200,  // surface_elevated
            menu_selected: base.primary_100,    // surface_selected
            menu_separator: base.neutral_200,   // border_muted

            // Separator and UI enhancement system
            separator_horizontal: base.neutral_200, // border_muted
            separator_vertical: base.neutral_200,   // border_muted
            separator_subtle: utils::with_alpha(base.neutral_200, 0.5), // border_muted + alpha
            focus_ring: base.primary_500,
            focus_ring_error: base.error_500,
            focus_ring_warning: base.warning_500,
        }
    }

    /// Create semantic colors from base colors for dark theme
    pub fn from_base_dark(base: &BaseColors) -> Self {
        Self {
            // Surface colors
            background: base.neutral_50,
            surface: base.neutral_100,
            surface_elevated: base.neutral_200,
            surface_overlay: hsla(0.0, 0.0, 0.0, 0.95),

            // Interactive states
            surface_hover: base.neutral_200,
            surface_active: base.neutral_300,
            surface_selected: base.primary_200,
            surface_disabled: base.neutral_100,

            // Text colors
            text_primary: base.neutral_900,
            text_secondary: base.neutral_700,
            text_tertiary: base.neutral_500,
            text_disabled: base.neutral_400,
            text_on_primary: base.neutral_50,

            // Border colors
            border_default: base.neutral_300,
            border_muted: base.neutral_200,
            border_strong: base.neutral_400,
            border_focus: base.primary_500,

            // Brand colors
            primary: base.primary_500,
            primary_hover: base.primary_400,
            primary_active: base.primary_300,

            // Semantic feedback
            success: base.success_500,
            warning: base.warning_500,
            error: base.error_500,
            info: base.info_500,

            // Cursor and selection system
            cursor_normal: base.primary_500,
            cursor_insert: base.success_500,
            cursor_select: base.warning_500,
            cursor_match: base.info_500,
            selection_primary: base.primary_200,
            selection_secondary: base.neutral_200,

            // Enhanced diagnostic system
            diagnostic_error: base.error_500,
            diagnostic_warning: base.warning_500,
            diagnostic_info: base.info_500,
            diagnostic_hint: base.neutral_600,
            diagnostic_error_bg: utils::with_alpha(base.error_500, 0.1),
            diagnostic_warning_bg: utils::with_alpha(base.warning_500, 0.1),
            diagnostic_info_bg: utils::with_alpha(base.info_500, 0.1),
            diagnostic_hint_bg: utils::with_alpha(base.neutral_600, 0.1),

            // Gutter and line number system
            gutter_background: base.neutral_50,
            gutter_selected: base.neutral_100,
            line_number: base.neutral_500,
            line_number_active: base.neutral_700,

            // Enhanced status and buffer system
            statusline_active: base.neutral_100,     // surface
            statusline_inactive: base.neutral_200,   // more distinct from active
            bufferline_background: base.neutral_300, // distinct tab bar background (16% lightness)
            bufferline_active: base.neutral_50, // background (active tab matches editor - 5% lightness)
            bufferline_inactive: base.neutral_400, // inactive tabs (24% lightness - high contrast with active)

            // Enhanced popup and menu system
            popup_background: base.neutral_200, // surface_elevated
            popup_border: base.neutral_300,     // border_default
            menu_background: base.neutral_200,  // surface_elevated
            menu_selected: base.primary_200,    // surface_selected
            menu_separator: base.neutral_200,   // border_muted

            // Separator and UI enhancement system
            separator_horizontal: base.neutral_200, // border_muted
            separator_vertical: base.neutral_200,   // border_muted
            separator_subtle: utils::with_alpha(base.neutral_200, 0.5), // border_muted + alpha
            focus_ring: base.primary_500,
            focus_ring_error: base.error_500,
            focus_ring_warning: base.warning_500,
        }
    }

    /// Create semantic colors from base colors for light theme with Helix-derived selection color
    pub fn from_base_light_with_selection(base: &BaseColors, selection_color: Hsla) -> Self {
        let mut colors = Self::from_base_light(base);

        // Override selection colors with Helix theme's selection color
        colors.selection_primary = selection_color;
        // Create a lighter variant for secondary selection (hover)
        colors.selection_secondary = utils::with_alpha(selection_color, 0.3);

        colors
    }

    /// Create semantic colors from base colors for dark theme with Helix-derived selection color
    pub fn from_base_dark_with_selection(base: &BaseColors, selection_color: Hsla) -> Self {
        let mut colors = Self::from_base_dark(base);

        // Override selection colors with Helix theme's selection color
        colors.selection_primary = selection_color;
        // Create a lighter variant for secondary selection (hover)
        colors.selection_secondary = utils::with_alpha(selection_color, 0.3);

        colors
    }

    /// Create semantic colors from base colors for light theme with comprehensive Helix-derived colors
    pub fn from_base_light_with_helix_colors(
        base: &BaseColors,
        helix_colors: crate::theme_manager::HelixThemeColors,
    ) -> Self {
        let mut colors = Self::from_base_light(base);

        // Override colors with Helix theme's extracted colors
        colors.selection_primary = helix_colors.selection;
        colors.selection_secondary = utils::with_alpha(helix_colors.selection, 0.3);

        // Cursor colors
        colors.cursor_normal = helix_colors.cursor_normal;
        colors.cursor_insert = helix_colors.cursor_insert;
        colors.cursor_select = helix_colors.cursor_select;
        colors.cursor_match = helix_colors.cursor_match;

        // Semantic feedback colors
        colors.error = helix_colors.error;
        colors.warning = helix_colors.warning;
        colors.success = helix_colors.success;
        colors.diagnostic_error = helix_colors.error;
        colors.diagnostic_warning = helix_colors.warning;
        colors.diagnostic_info = helix_colors.success;

        // UI component colors
        colors.statusline_active = helix_colors.statusline;
        colors.statusline_inactive = helix_colors.statusline_inactive;
        colors.popup_background = helix_colors.popup;

        // Buffer and tab system
        colors.bufferline_background = helix_colors.bufferline_background;
        colors.bufferline_active = helix_colors.bufferline_active;
        colors.bufferline_inactive = helix_colors.bufferline_inactive;

        // Gutter and line number system
        colors.gutter_background = helix_colors.gutter_background;
        colors.gutter_selected = helix_colors.gutter_selected;
        colors.line_number = helix_colors.line_number;
        colors.line_number_active = helix_colors.line_number_active;

        // Menu and popup system
        colors.menu_background = helix_colors.menu_background;
        colors.menu_selected = helix_colors.menu_selected;
        colors.menu_separator = helix_colors.menu_separator;

        // Separator and focus system
        colors.separator_horizontal = helix_colors.separator;
        colors.separator_vertical = helix_colors.separator;
        colors.separator_subtle = utils::with_alpha(helix_colors.separator, 0.5);
        colors.focus_ring = helix_colors.focus;
        colors.focus_ring_error = helix_colors.error;
        colors.focus_ring_warning = helix_colors.warning;

        // Also update primary brand color to match selection for consistency
        colors.primary = helix_colors.selection;
        colors.primary_hover = utils::lighten(helix_colors.selection, 0.1);
        colors.primary_active = utils::darken(helix_colors.selection, 0.1);
        colors.border_focus = helix_colors.selection;

        colors
    }

    /// Create semantic colors from base colors for dark theme with comprehensive Helix-derived colors
    pub fn from_base_dark_with_helix_colors(
        base: &BaseColors,
        helix_colors: crate::theme_manager::HelixThemeColors,
    ) -> Self {
        let mut colors = Self::from_base_dark(base);

        // Override colors with Helix theme's extracted colors
        colors.selection_primary = helix_colors.selection;
        colors.selection_secondary = utils::with_alpha(helix_colors.selection, 0.3);

        // Cursor colors
        colors.cursor_normal = helix_colors.cursor_normal;
        colors.cursor_insert = helix_colors.cursor_insert;
        colors.cursor_select = helix_colors.cursor_select;
        colors.cursor_match = helix_colors.cursor_match;

        // Semantic feedback colors
        colors.error = helix_colors.error;
        colors.warning = helix_colors.warning;
        colors.success = helix_colors.success;
        colors.diagnostic_error = helix_colors.error;
        colors.diagnostic_warning = helix_colors.warning;
        colors.diagnostic_info = helix_colors.success;

        // UI component colors
        colors.statusline_active = helix_colors.statusline;
        colors.statusline_inactive = helix_colors.statusline_inactive;
        colors.popup_background = helix_colors.popup;

        // Buffer and tab system
        colors.bufferline_background = helix_colors.bufferline_background;
        colors.bufferline_active = helix_colors.bufferline_active;
        colors.bufferline_inactive = helix_colors.bufferline_inactive;

        // Gutter and line number system
        colors.gutter_background = helix_colors.gutter_background;
        colors.gutter_selected = helix_colors.gutter_selected;
        colors.line_number = helix_colors.line_number;
        colors.line_number_active = helix_colors.line_number_active;

        // Menu and popup system
        colors.menu_background = helix_colors.menu_background;
        colors.menu_selected = helix_colors.menu_selected;
        colors.menu_separator = helix_colors.menu_separator;

        // Separator and focus system
        colors.separator_horizontal = helix_colors.separator;
        colors.separator_vertical = helix_colors.separator;
        colors.separator_subtle = utils::with_alpha(helix_colors.separator, 0.5);
        colors.focus_ring = helix_colors.focus;
        colors.focus_ring_error = helix_colors.error;
        colors.focus_ring_warning = helix_colors.warning;

        // Also update primary brand color to match selection for consistency
        colors.primary = helix_colors.selection;
        colors.primary_hover = utils::lighten(helix_colors.selection, 0.1);
        colors.primary_active = utils::darken(helix_colors.selection, 0.1);
        colors.border_focus = helix_colors.selection;

        colors
    }
}

/// Size and spacing tokens
#[derive(Debug, Clone, Copy)]
pub struct SizeTokens {
    // Spacing scale
    pub space_0: Pixels,  // 0px
    pub space_1: Pixels,  // 2px
    pub space_2: Pixels,  // 4px
    pub space_3: Pixels,  // 8px
    pub space_4: Pixels,  // 12px
    pub space_5: Pixels,  // 16px
    pub space_6: Pixels,  // 20px
    pub space_7: Pixels,  // 24px
    pub space_8: Pixels,  // 32px
    pub space_9: Pixels,  // 40px
    pub space_10: Pixels, // 48px

    // Component sizes
    pub button_height_sm: Pixels,
    pub button_height_md: Pixels,
    pub button_height_lg: Pixels,

    // Border radius
    pub radius_sm: Pixels,
    pub radius_md: Pixels,
    pub radius_lg: Pixels,
    pub radius_full: Pixels,

    // Font sizes
    pub text_xs: Pixels,
    pub text_sm: Pixels,
    pub text_md: Pixels,
    pub text_lg: Pixels,
    pub text_xl: Pixels,

    // Component sizes
    pub titlebar_height: Pixels,
}

impl SizeTokens {
    pub fn default() -> Self {
        Self {
            // Spacing scale
            space_0: px(0.0),
            space_1: px(2.0),
            space_2: px(4.0),
            space_3: px(8.0),
            space_4: px(12.0),
            space_5: px(16.0),
            space_6: px(20.0),
            space_7: px(24.0),
            space_8: px(32.0),
            space_9: px(40.0),
            space_10: px(48.0),

            // Component sizes
            button_height_sm: px(28.0),
            button_height_md: px(36.0),
            button_height_lg: px(44.0),

            // Border radius
            radius_sm: px(4.0),
            radius_md: px(6.0),
            radius_lg: px(8.0),
            radius_full: px(9999.0),

            // Font sizes
            text_xs: px(11.0),
            text_sm: px(12.0),
            text_md: px(14.0),
            text_lg: px(16.0),
            text_xl: px(18.0),

            // Component sizes
            titlebar_height: px(34.0),
        }
    }
}

/// Editor-specific tokens derived from Helix theme
#[derive(Debug, Clone, Copy)]
pub struct EditorTokens {
    // Selection and cursor system
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
    pub cursor_normal: Hsla,
    pub cursor_insert: Hsla,
    pub cursor_select: Hsla,
    pub cursor_match: Hsla,

    // Text colors from Helix theme
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_on_primary: Hsla,

    // Semantic feedback from Helix
    pub error: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub info: Hsla,

    // Diagnostic system from Helix
    pub diagnostic_error: Hsla,
    pub diagnostic_warning: Hsla,
    pub diagnostic_info: Hsla,
    pub diagnostic_hint: Hsla,
    pub diagnostic_error_bg: Hsla,
    pub diagnostic_warning_bg: Hsla,
    pub diagnostic_info_bg: Hsla,
    pub diagnostic_hint_bg: Hsla,

    // Gutter and line number system from Helix
    pub gutter_background: Hsla,
    pub gutter_selected: Hsla,
    pub line_number: Hsla,
    pub line_number_active: Hsla,

    // Focus indicators for editor elements
    pub focus_ring: Hsla,
    pub focus_ring_error: Hsla,
    pub focus_ring_warning: Hsla,
}

/// Chrome-specific tokens computed from surface color using color theory
#[derive(Debug, Clone, Copy)]
pub struct ChromeTokens {
    // Computed chrome backgrounds
    pub titlebar_background: Hsla,
    pub footer_background: Hsla,
    pub file_tree_background: Hsla,
    pub tab_empty_background: Hsla,
    pub separator_color: Hsla,

    // UI component backgrounds (computed or from system)
    pub surface: Hsla,
    pub surface_elevated: Hsla,
    pub surface_overlay: Hsla,
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub surface_selected: Hsla,
    pub surface_disabled: Hsla,

    // Border system for chrome elements
    pub border_default: Hsla,
    pub border_muted: Hsla,
    pub border_strong: Hsla,
    pub border_focus: Hsla,

    // Interactive states for chrome
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub primary_active: Hsla,

    // Menu and popup system (chrome elements)
    pub popup_background: Hsla,
    pub popup_border: Hsla,
    pub menu_background: Hsla,
    pub menu_selected: Hsla,
    pub menu_separator: Hsla,

    // Status and buffer system (chrome backgrounds)
    pub statusline_active: Hsla,
    pub statusline_inactive: Hsla,
    pub bufferline_background: Hsla,
    pub bufferline_active: Hsla,
    pub bufferline_inactive: Hsla,

    // Chrome text colors (computed for contrast)
    pub text_on_chrome: Hsla,
    pub text_chrome_secondary: Hsla,
    pub text_chrome_disabled: Hsla,
}

/// Design tokens combining colors and sizes
/// Now composed of separated editor and chrome token systems
#[derive(Debug, Clone, Copy)]
pub struct DesignTokens {
    pub editor: EditorTokens,
    pub chrome: ChromeTokens,
    pub colors: SemanticColors, // Keep for backwards compatibility
    pub sizes: SizeTokens,
}

impl EditorTokens {
    /// Create editor tokens from Helix theme colors
    pub fn from_helix_colors(helix_colors: crate::theme_manager::HelixThemeColors) -> Self {
        // Compute text colors from gutter background (approximation of editor background)
        let editor_bg = helix_colors.gutter_background;
        let text_primary = if editor_bg.l > 0.5 {
            // Light background, use dark text
            hsla(0.0, 0.0, 0.1, 1.0)
        } else {
            // Dark background, use light text
            hsla(0.0, 0.0, 0.9, 1.0)
        };
        let text_secondary = utils::with_alpha(text_primary, 0.7);
        let text_on_primary = if text_primary.l > 0.5 {
            hsla(0.0, 0.0, 0.1, 1.0)
        } else {
            hsla(0.0, 0.0, 0.9, 1.0)
        };

        Self {
            // Selection and cursor system
            selection_primary: helix_colors.selection,
            selection_secondary: utils::with_alpha(helix_colors.selection, 0.3),
            cursor_normal: helix_colors.cursor_normal,
            cursor_insert: helix_colors.cursor_insert,
            cursor_select: helix_colors.cursor_select,
            cursor_match: helix_colors.cursor_match,

            // Text colors computed from editor background
            text_primary,
            text_secondary,
            text_on_primary,

            // Semantic feedback from Helix
            error: helix_colors.error,
            warning: helix_colors.warning,
            success: helix_colors.success,
            info: helix_colors.success, // Use success color for info if no separate info color

            // Diagnostic system from Helix
            diagnostic_error: helix_colors.error,
            diagnostic_warning: helix_colors.warning,
            diagnostic_info: helix_colors.success,
            diagnostic_hint: text_secondary,
            diagnostic_error_bg: utils::with_alpha(helix_colors.error, 0.1),
            diagnostic_warning_bg: utils::with_alpha(helix_colors.warning, 0.1),
            diagnostic_info_bg: utils::with_alpha(helix_colors.success, 0.1),
            diagnostic_hint_bg: utils::with_alpha(text_secondary, 0.1),

            // Gutter and line number system from Helix
            gutter_background: helix_colors.gutter_background,
            gutter_selected: helix_colors.gutter_selected,
            line_number: helix_colors.line_number,
            line_number_active: helix_colors.line_number_active,

            // Focus indicators for editor elements
            focus_ring: helix_colors.selection,
            focus_ring_error: helix_colors.error,
            focus_ring_warning: helix_colors.warning,
        }
    }

    /// Create fallback editor tokens for testing or when Helix colors are unavailable
    pub fn fallback(is_dark: bool) -> Self {
        let base_colors = if is_dark {
            BaseColors::dark()
        } else {
            BaseColors::light()
        };

        Self {
            selection_primary: base_colors.primary_200,
            selection_secondary: utils::with_alpha(base_colors.primary_200, 0.3),
            cursor_normal: base_colors.primary_500,
            cursor_insert: base_colors.success_500,
            cursor_select: base_colors.warning_500,
            cursor_match: base_colors.info_500,

            text_primary: if is_dark { base_colors.neutral_900 } else { base_colors.neutral_100 },
            text_secondary: if is_dark { base_colors.neutral_700 } else { base_colors.neutral_300 },
            text_on_primary: if is_dark { base_colors.neutral_100 } else { base_colors.neutral_900 },

            error: base_colors.error_500,
            warning: base_colors.warning_500,
            success: base_colors.success_500,
            info: base_colors.info_500,

            diagnostic_error: base_colors.error_500,
            diagnostic_warning: base_colors.warning_500,
            diagnostic_info: base_colors.success_500,
            diagnostic_hint: if is_dark { base_colors.neutral_600 } else { base_colors.neutral_400 },
            diagnostic_error_bg: utils::with_alpha(base_colors.error_500, 0.1),
            diagnostic_warning_bg: utils::with_alpha(base_colors.warning_500, 0.1),
            diagnostic_info_bg: utils::with_alpha(base_colors.success_500, 0.1),
            diagnostic_hint_bg: utils::with_alpha(if is_dark { base_colors.neutral_600 } else { base_colors.neutral_400 }, 0.1),

            gutter_background: if is_dark { base_colors.neutral_50 } else { base_colors.neutral_100 },
            gutter_selected: if is_dark { base_colors.neutral_100 } else { base_colors.neutral_200 },
            line_number: if is_dark { base_colors.neutral_500 } else { base_colors.neutral_500 },
            line_number_active: if is_dark { base_colors.neutral_700 } else { base_colors.neutral_700 },

            focus_ring: base_colors.primary_500,
            focus_ring_error: base_colors.error_500,
            focus_ring_warning: base_colors.warning_500,
        }
    }
}

impl ChromeTokens {
    /// Create chrome tokens from surface color using color theory
    pub fn from_surface_color(surface_color: Hsla, is_dark: bool) -> Self {
        use crate::styling::color_theory::ColorTheory;

        // Compute chrome colors using color theory
        let chrome_colors = ColorTheory::derive_chrome_colors(surface_color);
        let base_colors = if is_dark {
            BaseColors::dark()
        } else {
            BaseColors::light()
        };

        // Compute contrasting text colors for chrome backgrounds
        let text_on_chrome = if surface_color.l > 0.5 {
            // Light surface, use dark text
            utils::darken(surface_color, 0.7)
        } else {
            // Dark surface, use light text
            utils::lighten(surface_color, 0.7)
        };

        Self {
            // Computed chrome backgrounds from color theory
            titlebar_background: chrome_colors.titlebar_background,
            footer_background: chrome_colors.footer_background,
            file_tree_background: chrome_colors.file_tree_background,
            tab_empty_background: chrome_colors.tab_empty_background,
            separator_color: chrome_colors.separator_color,

            // Surface system based on computed surface
            surface: surface_color,
            surface_elevated: if is_dark {
                utils::lighten(surface_color, 0.05)
            } else {
                utils::darken(surface_color, 0.05)
            },
            surface_overlay: if is_dark {
                hsla(surface_color.h, surface_color.s, 0.0, 0.95)
            } else {
                hsla(surface_color.h, surface_color.s, 1.0, 0.95)
            },
            surface_hover: if is_dark {
                utils::lighten(surface_color, 0.03)
            } else {
                utils::darken(surface_color, 0.03)
            },
            surface_active: if is_dark {
                utils::lighten(surface_color, 0.08)
            } else {
                utils::darken(surface_color, 0.08)
            },
            surface_selected: utils::with_alpha(base_colors.primary_500, 0.2),
            surface_disabled: utils::with_alpha(surface_color, 0.6),

            // Border system for chrome elements
            border_default: chrome_colors.separator_color,
            border_muted: utils::with_alpha(chrome_colors.separator_color, 0.5),
            border_strong: if is_dark {
                utils::lighten(chrome_colors.separator_color, 0.1)
            } else {
                utils::darken(chrome_colors.separator_color, 0.1)
            },
            border_focus: base_colors.primary_500,

            // Interactive states for chrome
            primary: base_colors.primary_500,
            primary_hover: base_colors.primary_600,
            primary_active: base_colors.primary_700,

            // Menu and popup system (chrome elements)
            popup_background: chrome_colors.file_tree_background, // Consistent with file tree
            popup_border: chrome_colors.separator_color,
            menu_background: chrome_colors.file_tree_background,
            menu_selected: utils::with_alpha(base_colors.primary_500, 0.2),
            menu_separator: chrome_colors.separator_color,

            // Status and buffer system (chrome backgrounds)
            statusline_active: chrome_colors.footer_background,
            statusline_inactive: utils::with_alpha(chrome_colors.footer_background, 0.8),
            bufferline_background: chrome_colors.tab_empty_background,
            bufferline_active: surface_color, // Active tab matches editor background
            bufferline_inactive: utils::with_alpha(chrome_colors.tab_empty_background, 0.9),

            // Chrome text colors (computed for contrast)
            text_on_chrome: text_on_chrome,
            text_chrome_secondary: utils::with_alpha(text_on_chrome, 0.7),
            text_chrome_disabled: utils::with_alpha(text_on_chrome, 0.4),
        }
    }

    /// Create fallback chrome tokens for testing
    pub fn fallback(is_dark: bool) -> Self {
        let base_colors = if is_dark {
            BaseColors::dark()
        } else {
            BaseColors::light()
        };
        let surface = if is_dark {
            base_colors.neutral_100
        } else {
            base_colors.neutral_50
        };

        Self::from_surface_color(surface, is_dark)
    }
}

impl DesignTokens {
    /// Create design tokens for light theme
    pub fn light() -> Self {
        let base_colors = BaseColors::light();
        Self {
            editor: EditorTokens::fallback(false),
            chrome: ChromeTokens::fallback(false),
            colors: SemanticColors::from_base_light(&base_colors),
            sizes: SizeTokens::default(),
        }
    }

    /// Create design tokens for dark theme
    pub fn dark() -> Self {
        let base_colors = BaseColors::dark();
        Self {
            editor: EditorTokens::fallback(true),
            chrome: ChromeTokens::fallback(true),
            colors: SemanticColors::from_base_dark(&base_colors),
            sizes: SizeTokens::default(),
        }
    }

    /// Create design tokens for light theme with Helix-derived selection color
    pub fn light_with_selection(selection_color: Hsla) -> Self {
        let base_colors = BaseColors::light();
        Self {
            editor: EditorTokens::fallback(false),
            chrome: ChromeTokens::fallback(false),
            colors: SemanticColors::from_base_light_with_selection(&base_colors, selection_color),
            sizes: SizeTokens::default(),
        }
    }

    /// Create design tokens for dark theme with Helix-derived selection color  
    pub fn dark_with_selection(selection_color: Hsla) -> Self {
        let base_colors = BaseColors::dark();
        Self {
            editor: EditorTokens::fallback(true),
            chrome: ChromeTokens::fallback(true),
            colors: SemanticColors::from_base_dark_with_selection(&base_colors, selection_color),
            sizes: SizeTokens::default(),
        }
    }

    /// Create design tokens for light theme with comprehensive Helix-derived colors
    pub fn light_with_helix_colors(helix_colors: crate::theme_manager::HelixThemeColors) -> Self {
        let base_colors = BaseColors::light();
        Self {
            editor: EditorTokens::from_helix_colors(helix_colors),
            chrome: ChromeTokens::fallback(false), // Temporary fallback, will use surface color later
            colors: SemanticColors::from_base_light_with_helix_colors(&base_colors, helix_colors),
            sizes: SizeTokens::default(),
        }
    }

    /// Create design tokens for dark theme with comprehensive Helix-derived colors
    pub fn dark_with_helix_colors(helix_colors: crate::theme_manager::HelixThemeColors) -> Self {
        let base_colors = BaseColors::dark();
        Self {
            editor: EditorTokens::from_helix_colors(helix_colors),
            chrome: ChromeTokens::fallback(true), // Temporary fallback, will use surface color later
            colors: SemanticColors::from_base_dark_with_helix_colors(&base_colors, helix_colors),
            sizes: SizeTokens::default(),
        }
    }

    /// Create design tokens from Helix theme and surface color (hybrid approach)
    /// This is the main factory method for the hybrid color system
    pub fn from_helix_and_surface(
        helix_colors: crate::theme_manager::HelixThemeColors,
        surface_color: Hsla,
        is_dark_theme: bool,
    ) -> Self {
        let base_colors = if is_dark_theme {
            BaseColors::dark()
        } else {
            BaseColors::light()
        };

        // Create editor tokens from Helix colors
        let editor = EditorTokens::from_helix_colors(helix_colors);

        // Create chrome tokens from surface color using color theory
        let chrome = ChromeTokens::from_surface_color(surface_color, is_dark_theme);

        // Create semantic colors for backwards compatibility
        let colors = if is_dark_theme {
            SemanticColors::from_base_dark_with_helix_colors(&base_colors, helix_colors)
        } else {
            SemanticColors::from_base_light_with_helix_colors(&base_colors, helix_colors)
        };

        Self {
            editor,
            chrome,
            colors, // Keep for backwards compatibility
            sizes: SizeTokens::default(),
        }
    }
}

/// Token utility functions for color manipulation
pub mod utils {
    use super::*;

    /// Create a color with adjusted opacity
    pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
        hsla(color.h, color.s, color.l, alpha)
    }

    /// Create a lighter variant of a color
    pub fn lighten(color: Hsla, amount: f32) -> Hsla {
        hsla(color.h, color.s, color.l + amount, color.a)
    }

    /// Create a darker variant of a color
    pub fn darken(color: Hsla, amount: f32) -> Hsla {
        hsla(color.h, color.s, color.l - amount, color.a)
    }

    /// Interpolate between two colors
    pub fn mix(color1: Hsla, color2: Hsla, ratio: f32) -> Hsla {
        let ratio = ratio.clamp(0.0, 1.0);
        hsla(
            color1.h + (color2.h - color1.h) * ratio,
            color1.s + (color2.s - color1.s) * ratio,
            color1.l + (color2.l - color1.l) * ratio,
            color1.a + (color2.a - color1.a) * ratio,
        )
    }
}

/// UI context for color selection
#[derive(Debug, Clone, Copy)]
pub enum ColorContext {
    /// Element sits on a surface background
    OnSurface,
    /// Element sits on a primary color background
    OnPrimary,
    /// Floating element (modal, popup)
    Floating,
    /// Overlay element
    Overlay,
}

/// Component-specific tokens for titlebar styling
#[derive(Clone, Copy, Debug)]
pub struct TitleBarTokens {
    pub background: Hsla,
    pub foreground: Hsla,
    pub border: Hsla,
    pub height: Pixels,
}

impl TitleBarTokens {
    pub fn on_surface(dt: &DesignTokens) -> Self {
        let bg = dt.colors.surface;
        let fg = crate::styling::ColorTheory::best_text_color(bg, dt);
        let border = crate::styling::ColorTheory::subtle_border_color(bg, dt);
        let height = dt.sizes.titlebar_height;

        debug!(
            "TITLEBAR TOKENS: Creating on_surface tokens - bg={:?}, fg={:?}, border={:?}, height={:?}",
            bg, fg, border, height
        );

        Self {
            background: bg,
            foreground: fg,
            border,
            height,
        }
    }

    pub fn on_primary(dt: &DesignTokens) -> Self {
        let bg = dt.colors.primary;
        let fg = crate::styling::ColorTheory::best_text_color(bg, dt);
        let border = crate::styling::ColorTheory::subtle_border_color(bg, dt);
        let height = dt.sizes.titlebar_height;

        debug!(
            "TITLEBAR TOKENS: Creating on_primary tokens - bg={:?}, fg={:?}, border={:?}, height={:?}",
            bg, fg, border, height
        );

        Self {
            background: bg,
            foreground: fg,
            border,
            height,
        }
    }

    pub fn floating(dt: &DesignTokens) -> Self {
        let bg = dt.colors.surface_elevated;
        let fg = crate::styling::ColorTheory::best_text_color(bg, dt);
        let border = crate::styling::ColorTheory::subtle_border_color(bg, dt);
        let height = dt.sizes.titlebar_height;

        debug!(
            "TITLEBAR TOKENS: Creating floating tokens - bg={:?}, fg={:?}, border={:?}, height={:?}",
            bg, fg, border, height
        );

        Self {
            background: bg,
            foreground: fg,
            border,
            height,
        }
    }

    pub fn overlay(dt: &DesignTokens) -> Self {
        let bg = dt.colors.surface_overlay;
        let fg = crate::styling::ColorTheory::best_text_color(bg, dt);
        let border = crate::styling::ColorTheory::subtle_border_color(bg, dt);
        let height = dt.sizes.titlebar_height;

        debug!(
            "TITLEBAR TOKENS: Creating overlay tokens - bg={:?}, fg={:?}, border={:?}, height={:?}",
            bg, fg, border, height
        );

        Self {
            background: bg,
            foreground: fg,
            border,
            height,
        }
    }
    /// Create titlebar tokens using computed chrome colors
    pub fn from_chrome_tokens(chrome: &ChromeTokens, sizes: &SizeTokens) -> Self {
        let bg = chrome.titlebar_background;
        let fg = chrome.text_on_chrome;
        let border = chrome.separator_color;
        let height = sizes.titlebar_height;

        nucleotide_logging::debug!(
            titlebar_bg = ?bg,
            titlebar_fg = ?fg,
            titlebar_border = ?border,
            titlebar_height = ?height,
            chrome_titlebar_bg = ?chrome.titlebar_background,
            chrome_footer_bg = ?chrome.footer_background,
            colors_match = (bg == chrome.footer_background),
            "Creating titlebar tokens from computed chrome colors"
        );

        Self {
            background: bg,
            foreground: fg,
            border,
            height,
        }
    }
}

/// File tree component tokens for background and content styling
#[derive(Debug, Clone, Copy)]
pub struct FileTreeTokens {
    pub background: Hsla,
    pub item_background_hover: Hsla,
    pub item_background_selected: Hsla,
    pub item_text: Hsla,
    pub item_text_secondary: Hsla,
    pub border: Hsla,
    pub separator: Hsla,
}

impl FileTreeTokens {
    /// Create file tree tokens using computed chrome colors for backgrounds 
    /// and editor colors for content
    pub fn from_tokens(chrome: &ChromeTokens, editor: &EditorTokens) -> Self {
        let bg = chrome.file_tree_background;
        let item_hover = chrome.surface_hover;
        let item_selected = editor.selection_primary;
        let item_text = chrome.text_on_chrome;
        let item_text_secondary = chrome.text_chrome_secondary;
        let border = chrome.border_muted;
        let separator = chrome.separator_color;

        nucleotide_logging::debug!(
            file_tree_bg = ?bg,
            item_hover = ?item_hover,
            item_selected = ?item_selected,
            item_text = ?item_text,
            "Creating file tree tokens from chrome and editor colors"
        );

        Self {
            background: bg,
            item_background_hover: item_hover,
            item_background_selected: item_selected,
            item_text,
            item_text_secondary,
            border,
            separator,
        }
    }

    /// Create fallback file tree tokens from design tokens
    pub fn from_design_tokens(dt: &DesignTokens) -> Self {
        Self::from_tokens(&dt.chrome, &dt.editor)
    }
}

/// Status bar component tokens for background and status content
#[derive(Debug, Clone, Copy)]
pub struct StatusBarTokens {
    pub background_active: Hsla,
    pub background_inactive: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_accent: Hsla,
    pub border: Hsla,
    pub mode_normal: Hsla,
    pub mode_insert: Hsla,
    pub mode_select: Hsla,
}

impl StatusBarTokens {
    /// Create status bar tokens using computed chrome colors for backgrounds
    /// and editor colors for status content
    pub fn from_tokens(chrome: &ChromeTokens, editor: &EditorTokens) -> Self {
        let bg_active = chrome.footer_background;
        let bg_inactive = chrome.footer_background; // Use same chrome color for consistency
        let text_primary = chrome.text_on_chrome;
        let text_secondary = chrome.text_chrome_secondary;
        let text_accent = editor.cursor_normal;
        let border = chrome.separator_color;
        let mode_normal = editor.cursor_normal;
        let mode_insert = editor.cursor_insert;
        let mode_select = editor.cursor_select;

        nucleotide_logging::debug!(
            status_bg_active = ?bg_active,
            status_bg_inactive = ?bg_inactive,
            status_text = ?text_primary,
            mode_colors = ?(mode_normal, mode_insert, mode_select),
            footer_bg = ?chrome.footer_background,
            titlebar_bg = ?chrome.titlebar_background,
            colors_match = (bg_active == chrome.titlebar_background),
            "Creating status bar tokens from chrome and editor colors"
        );

        Self {
            background_active: bg_active,
            background_inactive: bg_inactive,
            text_primary,
            text_secondary,
            text_accent,
            border,
            mode_normal,
            mode_insert,
            mode_select,
        }
    }

    /// Create fallback status bar tokens from design tokens
    pub fn from_design_tokens(dt: &DesignTokens) -> Self {
        Self::from_tokens(&dt.chrome, &dt.editor)
    }
}

/// Tab bar component tokens for tab container and individual tabs
#[derive(Debug, Clone, Copy)]
pub struct TabBarTokens {
    pub container_background: Hsla,
    pub tab_active_background: Hsla,
    pub tab_inactive_background: Hsla,
    pub tab_hover_background: Hsla,
    pub tab_text_active: Hsla,
    pub tab_text_inactive: Hsla,
    pub tab_border: Hsla,
    pub tab_separator: Hsla,
    pub tab_close_button: Hsla,
    pub tab_modified_indicator: Hsla,
}

impl TabBarTokens {
    /// Create tab bar tokens using computed chrome colors for container
    /// and editor colors for tab content
    pub fn from_tokens(chrome: &ChromeTokens, editor: &EditorTokens) -> Self {
        let container_bg = chrome.tab_empty_background;
        let tab_active_bg = chrome.surface; // Active tab matches editor surface
        let tab_inactive_bg = chrome.bufferline_inactive;
        let tab_hover_bg = chrome.surface_hover;
        let tab_text_active = editor.text_primary;
        let tab_text_inactive = chrome.text_chrome_secondary;
        let tab_border = chrome.border_muted;
        let tab_separator = chrome.separator_color;
        let tab_close = chrome.text_chrome_secondary;
        let tab_modified = editor.warning;

        nucleotide_logging::debug!(
            tab_container_bg = ?container_bg,
            tab_active_bg = ?tab_active_bg,
            tab_inactive_bg = ?tab_inactive_bg,
            tab_text_active = ?tab_text_active,
            tab_text_inactive = ?tab_text_inactive,
            "Creating tab bar tokens from chrome and editor colors"
        );

        Self {
            container_background: container_bg,
            tab_active_background: tab_active_bg,
            tab_inactive_background: tab_inactive_bg,
            tab_hover_background: tab_hover_bg,
            tab_text_active,
            tab_text_inactive,
            tab_border,
            tab_separator,
            tab_close_button: tab_close,
            tab_modified_indicator: tab_modified,
        }
    }

    /// Create fallback tab bar tokens from design tokens
    pub fn from_design_tokens(dt: &DesignTokens) -> Self {
        Self::from_tokens(&dt.chrome, &dt.editor)
    }
}

/// Extension methods for ChromeTokens to generate component-specific tokens
impl ChromeTokens {
    /// Generate titlebar tokens from chrome colors
    pub fn titlebar_tokens(&self, sizes: &SizeTokens) -> TitleBarTokens {
        TitleBarTokens::from_chrome_tokens(self, sizes)
    }

    /// Generate file tree tokens (requires editor tokens for content colors)
    pub fn file_tree_tokens(&self, editor: &EditorTokens) -> FileTreeTokens {
        FileTreeTokens::from_tokens(self, editor)
    }

    /// Generate status bar tokens (requires editor tokens for mode colors)
    pub fn status_bar_tokens(&self, editor: &EditorTokens) -> StatusBarTokens {
        StatusBarTokens::from_tokens(self, editor)
    }

    /// Generate tab bar tokens (requires editor tokens for content colors)
    pub fn tab_bar_tokens(&self, editor: &EditorTokens) -> TabBarTokens {
        TabBarTokens::from_tokens(self, editor)
    }
}

/// Extension methods for DesignTokens to generate component-specific tokens
impl DesignTokens {
    /// Generate titlebar tokens using the hybrid system
    pub fn titlebar_tokens(&self) -> TitleBarTokens {
        self.chrome.titlebar_tokens(&self.sizes)
    }

    /// Generate file tree tokens using the hybrid system
    pub fn file_tree_tokens(&self) -> FileTreeTokens {
        self.chrome.file_tree_tokens(&self.editor)
    }

    /// Generate status bar tokens using the hybrid system
    pub fn status_bar_tokens(&self) -> StatusBarTokens {
        self.chrome.status_bar_tokens(&self.editor)
    }

    /// Generate tab bar tokens using the hybrid system
    pub fn tab_bar_tokens(&self) -> TabBarTokens {
        self.chrome.tab_bar_tokens(&self.editor)
    }
}

// Re-export commonly used types
pub use utils::*;

#[cfg(test)]
mod tests;
