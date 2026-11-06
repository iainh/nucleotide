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
    pub statusline_foreground: Hsla,
    pub statusline_inactive: Hsla,
    pub statusline_inactive_foreground: Hsla,
    pub popup: Hsla,
    pub popup_foreground: Hsla,

    // Buffer and tab system
    pub bufferline_background: Hsla,
    pub bufferline_foreground: Hsla,
    pub bufferline_active: Hsla,
    pub bufferline_active_foreground: Hsla,
    pub bufferline_inactive: Hsla,
    pub bufferline_inactive_foreground: Hsla,

    // Gutter and line number system
    pub gutter_background: Hsla,
    pub gutter_selected: Hsla,
    pub line_number: Hsla,
    pub line_number_active: Hsla,

    // Menu and popup system
    pub menu_background: Hsla,
    pub menu_foreground: Hsla,
    pub menu_selected: Hsla,
    pub menu_selected_foreground: Hsla,
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
    /// Use the platform palette with light/dark surfaces matching the editor theme.
    System,
}

/// Global SystemAppearance state for GPUI integration.
#[derive(Default)]
struct GlobalSystemAppearance(SystemAppearance);

impl Global for GlobalSystemAppearance {}

/// Windows personalisation accent and the OS-provided light/dark variants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemAccentRamp {
    pub accent: Hsla,
    pub light1: Hsla,
    pub light2: Hsla,
    pub light3: Hsla,
    pub dark1: Hsla,
    pub dark2: Hsla,
    pub dark3: Hsla,
}

impl SystemAccentRamp {
    /// Fluent accent fill uses Light2 on dark surfaces and Dark1 on light ones.
    pub fn control_colors(self, appearance: SystemAppearance) -> (Hsla, Hsla, Hsla) {
        let accent = if appearance.is_dark() {
            self.light2
        } else {
            self.dark1
        };
        (accent, Hsla { a: 0.9, ..accent }, Hsla { a: 0.8, ..accent })
    }
}

/// Native/system palette inputs used by UI token construction.
#[derive(Debug, Clone, Copy)]
pub struct NativeChromePalette {
    pub appearance: SystemAppearance,
    pub accent: Hsla,
    pub accent_ramp: Option<SystemAccentRamp>,
    pub mica_base: Hsla,
    pub layer_base: Hsla,
    pub layer_alt_base: Hsla,
    pub elevated_base: Hsla,
    /// Popup fill colour, not a compositor material. Windows uses an opaque
    /// fallback until GPUI supports separately composited transient surfaces.
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
        #[cfg(target_os = "windows")]
        if let Some(ramp) = platform::system_accent_ramp() {
            let mut palette = Self::with_accent(appearance, ramp.accent);
            palette.accent_ramp = Some(ramp);
            return palette;
        }
        let accent = platform_system_accent_color().unwrap_or_else(default_system_accent_color);
        Self::with_accent(appearance, accent)
    }

    /// Resolve the platform palette with a caller-provided accent.
    pub fn with_accent(appearance: SystemAppearance, accent: Hsla) -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::windows_fluent(appearance, accent)
        }

        #[cfg(target_os = "macos")]
        {
            Self::macos_native(appearance, accent)
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
                hsla_from_rgb_u8(44, 44, 44, 1.0),
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
                hsla_from_rgb_u8(252, 252, 252, 1.0),
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
            accent_ramp: None,
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
                hsla_from_rgb_u8(28, 28, 30, 1.0),
                hsla_from_rgb_u8(44, 44, 46, 1.0),
                hsla_from_rgb_u8(39, 40, 43, 1.0),
                hsla_from_rgb_u8(58, 58, 60, 1.0),
                hsla_from_rgb_u8(44, 44, 46, 0.90),
                hsla_from_rgb_u8(84, 84, 88, 0.44),
                hsla_from_rgb_u8(112, 112, 117, 0.42),
                hsla_from_rgb_u8(245, 245, 247, 1.0),
                0.72,
                0.40,
                0.24,
                0.38,
            )
        } else {
            (
                hsla_from_rgb_u8(247, 249, 252, 1.0),
                hsla_from_rgb_u8(255, 255, 255, 1.0),
                // Finder's source-list material is a cool, quiet blue-grey rather
                // than a second white content panel.
                hsla_from_rgb_u8(224, 230, 239, 1.0),
                hsla_from_rgb_u8(255, 255, 255, 1.0),
                hsla_from_rgb_u8(250, 251, 253, 0.90),
                hsla_from_rgb_u8(60, 60, 67, 0.18),
                hsla_from_rgb_u8(60, 60, 67, 0.24),
                hsla_from_rgb_u8(28, 28, 30, 1.0),
                0.68,
                0.34,
                0.10,
                0.18,
            )
        };

        let (mica_alpha, layer_alpha, dense_text_layer_alpha, elevated_alpha) = if is_dark {
            (0.84, 0.86, 0.90, 0.94)
        } else {
            (0.88, 0.86, 0.90, 0.94)
        };

        Self {
            appearance,
            accent,
            accent_ramp: None,
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

/// Whether the native window material surrounds rounded content islands.
pub fn content_islands_enabled(ui_chrome_style: UiChromeStyle) -> bool {
    cfg!(any(target_os = "windows", target_os = "macos"))
        && ui_chrome_style == UiChromeStyle::System
}

/// Match this window's native frame and Mica tint to the application theme.
/// This does not change Windows settings or GPUI's observed OS appearance.
#[cfg(target_os = "windows")]
pub fn set_native_window_theme(window: &gpui::Window, is_dark: bool) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};

    let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("Window does not have a Win32 handle".into());
    };
    let dark_mode = i32::from(is_dark);
    // SAFETY: The HWND is borrowed from the live window; DWM reads a BOOL
    // whose storage remains valid for the duration of this synchronous call.
    let result = unsafe {
        DwmSetWindowAttribute(
            handle.hwnd.get() as _,
            DWMWA_USE_IMMERSIVE_DARK_MODE as _,
            &dark_mode as *const _ as _,
            std::mem::size_of_val(&dark_mode) as u32,
        )
    };
    if result < 0 {
        Err(format!("DWM window theme update failed: {result:#x}"))
    } else {
        Ok(())
    }
}

/// Match this window's native controls and vibrancy to the application theme.
/// Call on the UI thread; this does not change the app-wide or OS appearance.
#[cfg(target_os = "macos")]
pub fn set_native_window_theme(window: &gpui::Window, is_dark: bool) -> Result<(), String> {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSView, NSVisualEffectView, NSWindowButton,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err("Window does not have an AppKit handle".into());
    };
    // SAFETY: GPUI lends the live NSView on its UI thread. Its window and
    // appearance are retained by AppKit throughout this synchronous update.
    let native_view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let native_window = native_view
        .window()
        .ok_or("AppKit view is not attached to a window")?;
    let name = unsafe {
        if is_dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    let appearance = NSAppearance::appearanceNamed(name).ok_or("AppKit appearance unavailable")?;
    // Leave the NSWindow and GPUI view inheriting the OS appearance so GPUI's
    // existing observer still receives actual system light/dark changes.
    // The material and raw-handle GPUIView are siblings in the content view.
    for view in native_window
        .contentView()
        .ok_or("AppKit window has no content view")?
        .subviews()
    {
        if let Some(effect) = view.downcast_ref::<NSVisualEffectView>() {
            effect.setAppearance(Some(&appearance));
        }
    }
    for button in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = native_window.standardWindowButton(button) {
            button.setAppearance(Some(&appearance));
        }
    }
    Ok(())
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
        default_macos_accent_color()
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
    use super::{Hsla, SystemAccentRamp, hsla_from_rgb_u8};
    use windows::UI::ViewManagement::{UIColorType, UISettings};

    pub fn system_accent_color() -> Option<Hsla> {
        let settings = UISettings::new().ok()?;
        color(&settings, UIColorType::Accent)
    }

    fn color(settings: &UISettings, kind: UIColorType) -> Option<Hsla> {
        let color = settings.GetColorValue(kind).ok()?;
        Some(hsla_from_rgb_u8(color.R, color.G, color.B, 1.0))
    }

    pub fn system_accent_ramp() -> Option<SystemAccentRamp> {
        let settings = UISettings::new().ok()?;
        Some(SystemAccentRamp {
            accent: color(&settings, UIColorType::Accent)?,
            light1: color(&settings, UIColorType::AccentLight1)?,
            light2: color(&settings, UIColorType::AccentLight2)?,
            light3: color(&settings, UIColorType::AccentLight3)?,
            dark1: color(&settings, UIColorType::AccentDark1)?,
            dark2: color(&settings, UIColorType::AccentDark2)?,
            dark3: color(&settings, UIColorType::AccentDark3)?,
        })
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

    #[cfg(target_os = "windows")]
    #[test]
    fn current_windows_palette_reads_ui_settings_ramp() {
        use windows::UI::ViewManagement::{UIColorType, UISettings};
        let settings = UISettings::new().unwrap();
        let palette = NativeChromePalette::current(SystemAppearance::Dark);
        let ramp = palette.accent_ramp.expect("Windows UISettings accent ramp");
        for (actual, kind) in [
            (palette.accent, UIColorType::Accent),
            (ramp.light2, UIColorType::AccentLight2),
            (ramp.dark1, UIColorType::AccentDark1),
        ] {
            let expected = settings.GetColorValue(kind).unwrap();
            println!(
                "UISettings {kind:?}: #{:02X}{:02X}{:02X}",
                expected.R, expected.G, expected.B
            );
            assert_eq!(
                actual,
                hsla_from_rgb_u8(expected.R, expected.G, expected.B, 1.0)
            );
        }
    }

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

        if cfg!(target_os = "windows") {
            assert_eq!(light.mica_alpha, 0.0);
            assert_eq!(dark.mica_alpha, 0.0);
            assert_eq!(light.acrylic.a, 1.0, "popup fallback must be opaque");
            assert_eq!(dark.acrylic.a, 1.0, "popup fallback must be opaque");
            assert!(light.layer_alpha < 1.0);
            assert!(dark.layer_alpha < 1.0);
        } else if cfg!(target_os = "macos") {
            assert!(light.mica_alpha >= 0.85);
            assert!(dark.mica_alpha >= 0.80);
            assert!(light.layer_alpha >= light.mica_alpha - 0.05);
            assert!(dark.layer_alpha >= dark.mica_alpha - 0.05);
            assert!(light.dense_text_layer_alpha > light.layer_alpha);
            assert!(dark.dense_text_layer_alpha > dark.layer_alpha);
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

    #[test]
    fn content_islands_require_system_look_and_native_material() {
        assert!(!content_islands_enabled(UiChromeStyle::Theme));
        assert_eq!(
            content_islands_enabled(UiChromeStyle::System),
            cfg!(any(target_os = "windows", target_os = "macos"))
        );
    }

    #[test]
    fn windows_and_macos_native_palettes_are_distinct() {
        let accent = default_system_accent_color();
        let windows = NativeChromePalette::windows_fluent(SystemAppearance::Light, accent);
        let macos = NativeChromePalette::macos_native(SystemAppearance::Light, accent);

        assert_eq!(windows.accent, accent);
        assert_eq!(macos.accent, accent);
        assert_ne!(windows.stroke, macos.stroke);
        assert_ne!(windows.layer_alpha, macos.layer_alpha);
        assert!(windows.layer_alpha < 1.0);
        assert!(macos.layer_alpha < 1.0);
        assert!(macos.layer_alt_base.s > macos.layer_base.s);
        assert!(macos.mica_alpha >= 0.85);
        assert!(macos.layer_alpha >= 0.85);
        assert!(macos.dense_text_layer_alpha > macos.layer_alpha);
    }
}
