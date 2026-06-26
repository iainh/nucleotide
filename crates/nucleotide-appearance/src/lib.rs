// ABOUTME: Platform appearance facade for native/system UI chrome.
// ABOUTME: Keeps OS color/material decisions separate from reusable UI tokens.

use gpui::{App, Global, Hsla, WindowAppearance, WindowBackgroundAppearance, hsla};

/// Extracted colors from a Helix theme used to derive editor and themed chrome.
#[derive(Debug, Clone, Copy)]
pub struct HelixThemeColors {
    // Core selection and cursor colors
    pub selection: Hsla,
    pub cursor_normal: Hsla,
    pub cursor_insert: Hsla,
    pub cursor_select: Hsla,
    pub cursor_match: Hsla,

    // Semantic feedback colors
    pub error: Hsla,
    pub warning: Hsla,
    pub success: Hsla,

    // VCS colors from Helix diff scopes
    pub vcs_added: Hsla,
    pub vcs_modified: Hsla,
    pub vcs_deleted: Hsla,

    // UI component backgrounds
    pub statusline: Hsla,
    pub statusline_inactive: Hsla,
    pub popup: Hsla,

    // Buffer and tab system
    pub bufferline_background: Hsla,
    pub bufferline_active: Hsla,
    pub bufferline_inactive: Hsla,

    // Gutter and line number system
    pub gutter_background: Hsla,
    pub gutter_selected: Hsla,
    pub line_number: Hsla,
    pub line_number_active: Hsla,

    // Menu and popup system
    pub menu_background: Hsla,
    pub menu_selected: Hsla,
    pub menu_separator: Hsla,

    // Separator and focus system
    pub separator: Hsla,
    pub focus: Hsla,

    // Text colors
    pub text_primary: Hsla,
}

/// System appearance state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SystemAppearance {
    #[default]
    Light,
    Dark,
}

impl SystemAppearance {
    /// Initializes the global SystemAppearance based on the current window appearance.
    pub fn init(cx: &mut App) {
        *cx.default_global::<GlobalSystemAppearance>() =
            GlobalSystemAppearance(SystemAppearance::from(cx.window_appearance()));
    }

    /// Returns the global SystemAppearance.
    pub fn global(cx: &App) -> Self {
        cx.global::<GlobalSystemAppearance>().0
    }

    /// Returns a mutable reference to the global SystemAppearance.
    pub fn global_mut(cx: &mut App) -> &mut Self {
        &mut cx.global_mut::<GlobalSystemAppearance>().0
    }

    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

impl From<WindowAppearance> for SystemAppearance {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => SystemAppearance::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => SystemAppearance::Dark,
        }
    }
}

/// Source for non-editor UI chrome styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiChromeStyle {
    /// Derive chrome surfaces from the active Helix theme.
    #[default]
    Theme,
    /// Derive chrome surfaces from the current platform appearance.
    System,
}

/// Global SystemAppearance state for GPUI integration.
#[derive(Default)]
struct GlobalSystemAppearance(SystemAppearance);

impl Global for GlobalSystemAppearance {}

/// Native/system palette inputs used by UI token construction.
#[derive(Debug, Clone, Copy)]
pub struct NativeChromePalette {
    pub appearance: SystemAppearance,
    pub accent: Hsla,
    pub mica_base: Hsla,
    pub layer_base: Hsla,
    pub layer_alt_base: Hsla,
    pub elevated_base: Hsla,
    pub acrylic: Hsla,
    pub stroke: Hsla,
    pub stroke_subtle: Hsla,
    pub text: Hsla,
    pub secondary_text_alpha: f32,
    pub disabled_text_alpha: f32,
    pub shadow_alpha: f32,
    pub strong_shadow_alpha: f32,
    pub mica_alpha: f32,
    pub layer_alpha: f32,
    pub dense_text_layer_alpha: f32,
    pub elevated_alpha: f32,
}

impl NativeChromePalette {
    /// Resolve the current platform palette using the OS accent when available.
    pub fn current(appearance: SystemAppearance) -> Self {
        let accent = platform_system_accent_color().unwrap_or_else(default_system_accent_color);
        Self::with_accent(appearance, accent)
    }

    /// Resolve the platform palette with a caller-provided accent.
    pub fn with_accent(appearance: SystemAppearance, accent: Hsla) -> Self {
        #[cfg(target_os = "windows")]
        {
            return Self::windows_fluent(appearance, accent);
        }

        #[cfg(target_os = "macos")]
        {
            return Self::macos_native(appearance, accent);
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            Self::solid_fallback(appearance, accent)
        }
    }

    pub fn windows_fluent(appearance: SystemAppearance, accent: Hsla) -> Self {
        let is_dark = appearance.is_dark();
        let (
            mica_base,
            layer_base,
            layer_alt_base,
            elevated_base,
            acrylic,
            stroke,
            stroke_subtle,
            text,
            secondary_text_alpha,
            disabled_text_alpha,
            shadow_alpha,
            strong_shadow_alpha,
        ) = if is_dark {
            (
                hsla_from_rgb_u8(32, 32, 32, 1.0),
                hsla_from_rgb_u8(43, 43, 43, 1.0),
                hsla_from_rgb_u8(38, 38, 38, 1.0),
                hsla_from_rgb_u8(48, 48, 48, 1.0),
                hsla_from_rgb_u8(44, 44, 44, 0.96),
                hsla_from_rgb_u8(65, 65, 65, 1.0),
                hsla_from_rgb_u8(78, 78, 78, 0.74),
                hsla_from_rgb_u8(243, 243, 243, 1.0),
                0.78,
                0.42,
                0.36,
                0.52,
            )
        } else {
            (
                hsla_from_rgb_u8(243, 243, 243, 1.0),
                hsla_from_rgb_u8(255, 255, 255, 1.0),
                hsla_from_rgb_u8(249, 249, 249, 1.0),
                hsla_from_rgb_u8(255, 255, 255, 1.0),
                hsla_from_rgb_u8(252, 252, 252, 0.96),
                hsla_from_rgb_u8(225, 225, 225, 1.0),
                hsla_from_rgb_u8(199, 199, 199, 0.82),
                hsla_from_rgb_u8(26, 26, 26, 1.0),
                0.72,
                0.36,
                0.14,
                0.22,
            )
        };

        let (mica_alpha, layer_alpha, dense_text_layer_alpha, elevated_alpha) = if is_dark {
            (0.0, 0.62, 0.62, 0.78)
        } else {
            (0.0, 0.58, 0.58, 0.76)
        };

        Self {
            appearance,
            accent,
            mica_base,
            layer_base,
            layer_alt_base,
            elevated_base,
            acrylic,
            stroke,
            stroke_subtle,
            text,
            secondary_text_alpha,
            disabled_text_alpha,
            shadow_alpha,
            strong_shadow_alpha,
            mica_alpha,
            layer_alpha,
            dense_text_layer_alpha,
            elevated_alpha,
        }
    }

    pub fn macos_native(appearance: SystemAppearance, accent: Hsla) -> Self {
        let is_dark = appearance.is_dark();
        let (
            mica_base,
            layer_base,
            layer_alt_base,
            elevated_base,
            acrylic,
            stroke,
            stroke_subtle,
            text,
            secondary_text_alpha,
            disabled_text_alpha,
            shadow_alpha,
            strong_shadow_alpha,
        ) = if is_dark {
            (
                hsla_from_rgb_u8(30, 30, 30, 1.0),
                hsla_from_rgb_u8(44, 44, 46, 1.0),
                hsla_from_rgb_u8(36, 36, 38, 1.0),
                hsla_from_rgb_u8(58, 58, 60, 1.0),
                hsla_from_rgb_u8(44, 44, 46, 0.92),
                hsla_from_rgb_u8(84, 84, 88, 0.58),
                hsla_from_rgb_u8(99, 99, 102, 0.52),
                hsla_from_rgb_u8(245, 245, 247, 1.0),
                0.76,
                0.40,
                0.28,
                0.42,
            )
        } else {
            (
                hsla_from_rgb_u8(246, 246, 246, 1.0),
                hsla_from_rgb_u8(255, 255, 255, 1.0),
                hsla_from_rgb_u8(248, 248, 248, 1.0),
                hsla_from_rgb_u8(255, 255, 255, 1.0),
                hsla_from_rgb_u8(250, 250, 250, 0.92),
                hsla_from_rgb_u8(198, 198, 200, 0.72),
                hsla_from_rgb_u8(174, 174, 178, 0.56),
                hsla_from_rgb_u8(28, 28, 30, 1.0),
                0.70,
                0.34,
                0.12,
                0.20,
            )
        };

        let (mica_alpha, layer_alpha, dense_text_layer_alpha, elevated_alpha) = if is_dark {
            (0.0, 0.58, 0.62, 0.72)
        } else {
            (0.0, 0.54, 0.58, 0.70)
        };

        Self {
            appearance,
            accent,
            mica_base,
            layer_base,
            layer_alt_base,
            elevated_base,
            acrylic,
            stroke,
            stroke_subtle,
            text,
            secondary_text_alpha,
            disabled_text_alpha,
            shadow_alpha,
            strong_shadow_alpha,
            mica_alpha,
            layer_alpha,
            dense_text_layer_alpha,
            elevated_alpha,
        }
    }

    pub fn solid_fallback(appearance: SystemAppearance, accent: Hsla) -> Self {
        let mut palette = Self::windows_fluent(appearance, accent);
        palette.mica_alpha = 1.0;
        palette.layer_alpha = 1.0;
        palette.dense_text_layer_alpha = 1.0;
        palette.elevated_alpha = 1.0;
        palette
    }
}

/// Return the native material GPUI should request for the configured UI look.
pub fn window_background_appearance(
    ui_chrome_style: UiChromeStyle,
    is_dark_chrome: bool,
    blur_dark_themes: bool,
) -> WindowBackgroundAppearance {
    if ui_chrome_style == UiChromeStyle::System {
        return system_window_background_appearance();
    }

    if is_dark_chrome && blur_dark_themes {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

#[cfg(target_os = "windows")]
fn system_window_background_appearance() -> WindowBackgroundAppearance {
    WindowBackgroundAppearance::MicaBackdrop
}

#[cfg(target_os = "macos")]
fn system_window_background_appearance() -> WindowBackgroundAppearance {
    WindowBackgroundAppearance::Blurred
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn system_window_background_appearance() -> WindowBackgroundAppearance {
    WindowBackgroundAppearance::Opaque
}

pub fn default_windows_accent_color() -> Hsla {
    hsla_from_rgb_u8(0, 120, 212, 1.0)
}

pub fn default_macos_accent_color() -> Hsla {
    hsla_from_rgb_u8(0, 122, 255, 1.0)
}

pub fn default_system_accent_color() -> Hsla {
    #[cfg(target_os = "macos")]
    {
        return default_macos_accent_color();
    }

    #[cfg(not(target_os = "macos"))]
    {
        default_windows_accent_color()
    }
}

pub fn platform_system_accent_color() -> Option<Hsla> {
    platform::system_accent_color()
}

pub fn hsla_from_rgb_u8(red: u8, green: u8, blue: u8, alpha: f32) -> Hsla {
    let r = red as f32 / 255.0;
    let g = green as f32 / 255.0;
    let b = blue as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let chroma = max - min;
    let lightness = (max + min) / 2.0;

    if chroma == 0.0 {
        return hsla(0.0, 0.0, lightness, alpha);
    }

    let saturation = chroma / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        ((b - r) / chroma) + 2.0
    } else {
        ((r - g) / chroma) + 4.0
    } / 6.0;

    hsla(hue, saturation, lightness, alpha)
}

#[cfg(target_os = "windows")]
mod platform {
    use gpui::Hsla;

    pub fn system_accent_color() -> Option<Hsla> {
        use windows_sys::Win32::Graphics::Gdi::{COLOR_HIGHLIGHT, GetSysColor};

        // SAFETY: GetSysColor reads a process-global system color for the
        // supplied COLOR_* index and has no pointer or lifetime requirements.
        let color = unsafe { GetSysColor(COLOR_HIGHLIGHT) };
        let red = (color & 0xFF) as u8;
        let green = ((color >> 8) & 0xFF) as u8;
        let blue = ((color >> 16) & 0xFF) as u8;

        Some(super::hsla_from_rgb_u8(red, green, blue, 1.0))
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use gpui::Hsla;

    pub fn system_accent_color() -> Option<Hsla> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_palette_uses_platform_material_alpha_policy() {
        let light = NativeChromePalette::with_accent(
            SystemAppearance::Light,
            default_system_accent_color(),
        );
        let dark =
            NativeChromePalette::with_accent(SystemAppearance::Dark, default_system_accent_color());

        assert!(light.layer_base.l > dark.layer_base.l);
        assert!(light.text.l < dark.text.l);

        if cfg!(any(target_os = "windows", target_os = "macos")) {
            assert_eq!(light.mica_alpha, 0.0);
            assert_eq!(dark.mica_alpha, 0.0);
            assert!(light.layer_alpha < 1.0);
            assert!(dark.layer_alpha < 1.0);
        } else {
            assert_eq!(light.mica_alpha, 1.0);
            assert_eq!(dark.mica_alpha, 1.0);
            assert_eq!(light.layer_alpha, 1.0);
            assert_eq!(dark.layer_alpha, 1.0);
        }
    }

    #[test]
    fn system_look_requests_native_window_material() {
        let material = window_background_appearance(UiChromeStyle::System, false, false);

        #[cfg(target_os = "windows")]
        assert_eq!(material, WindowBackgroundAppearance::MicaBackdrop);

        #[cfg(target_os = "macos")]
        assert_eq!(material, WindowBackgroundAppearance::Blurred);

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        assert_eq!(material, WindowBackgroundAppearance::Opaque);
    }
}
