// ABOUTME: Reads Linux desktop appearance from the XDG Desktop Portal.
// ABOUTME: Falls back to GTK/KDE config files and desktop defaults when it is unavailable.

use std::{path::PathBuf, sync::mpsc, time::Duration};

use ashpd::desktop::settings::{ColorScheme, Settings};
use gpui::Hsla;

use crate::{SystemAppearance, hsla_from_rgb_u8};

/// A missing portal backend can leave D-Bus activation pending for its full
/// method timeout. Startup and window activation must not wait for that.
const PORTAL_TIMEOUT: Duration = Duration::from_millis(500);
const GNOME_INTERFACE: &str = "org.gnome.desktop.interface";
const KDE_GENERAL: &str = "org.kde.kdeglobals.General";

/// Linux desktop family whose native palette the system look follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxDesktop {
    /// GNOME and other GTK/libadwaita desktops.
    Adwaita,
    /// KDE Plasma.
    Breeze,
}

impl LinuxDesktop {
    pub fn current() -> Self {
        Self::from_xdg_current_desktop(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref())
    }

    fn from_xdg_current_desktop(value: Option<&str>) -> Self {
        let is_kde = value.is_some_and(|value| {
            value
                .split(':')
                .any(|desktop| desktop.eq_ignore_ascii_case("KDE"))
        });
        if is_kde { Self::Breeze } else { Self::Adwaita }
    }

    /// Point size of each desktop's default interface font (Cantarell 11, Noto Sans 10).
    fn default_font_points(self) -> f32 {
        match self {
            Self::Adwaita => 11.0,
            Self::Breeze => 10.0,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct PortalSettings {
    pub accent: Option<Hsla>,
    pub color_scheme: Option<SystemAppearance>,
    gnome_font_name: Option<String>,
    text_scaling_factor: Option<f64>,
    kde_font: Option<String>,
}

/// Read `org.freedesktop.appearance` and the desktop font from the portal.
/// Every field is `None` when the portal, its backend, or the key is missing.
pub(crate) fn read_portal_settings() -> PortalSettings {
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("xdg-portal-settings".into())
        .spawn(move || {
            let _ = sender.send(futures_executor::block_on(read_portal_settings_async()));
        });
    if spawned.is_err() {
        return PortalSettings::default();
    }
    receiver
        .recv_timeout(PORTAL_TIMEOUT)
        .ok()
        .flatten()
        .unwrap_or_default()
}

async fn read_portal_settings_async() -> Option<PortalSettings> {
    let settings = Settings::new().await.ok()?;
    Some(PortalSettings {
        accent: settings
            .accent_color()
            .await
            .ok()
            .and_then(|color| accent_from_portal_rgb(color.red(), color.green(), color.blue())),
        color_scheme: settings
            .color_scheme()
            .await
            .ok()
            .and_then(|scheme| match scheme {
                ColorScheme::PreferDark => Some(SystemAppearance::Dark),
                ColorScheme::PreferLight => Some(SystemAppearance::Light),
                ColorScheme::NoPreference => None,
            }),
        gnome_font_name: settings.read(GNOME_INTERFACE, "font-name").await.ok(),
        text_scaling_factor: settings
            .read(GNOME_INTERFACE, "text-scaling-factor")
            .await
            .ok(),
        kde_font: settings.read(KDE_GENERAL, "font").await.ok(),
    })
}

/// The portal reports an unset accent with components outside 0.0..=1.0.
fn accent_from_portal_rgb(red: f64, green: f64, blue: f64) -> Option<Hsla> {
    let channel = |value: f64| {
        (0.0..=1.0)
            .contains(&value)
            .then(|| (value * 255.0).round() as u8)
    };
    Some(hsla_from_rgb_u8(
        channel(red)?,
        channel(green)?,
        channel(blue)?,
        1.0,
    ))
}

/// Interface font size in logical pixels, from the portal, then the desktop's
/// own config file, then the desktop's default font.
pub fn ui_font_size() -> f32 {
    let desktop = LinuxDesktop::current();
    let portal = read_portal_settings();
    let gnome = || {
        portal
            .gnome_font_name
            .as_deref()
            .and_then(parse_pango_font_size)
            .or_else(|| {
                ["gtk-4.0", "gtk-3.0"].into_iter().find_map(|dir| {
                    let contents = read_config_file(&format!("{dir}/settings.ini"))?;
                    parse_pango_font_size(&ini_value(&contents, "Settings", "gtk-font-name")?)
                })
            })
    };
    let kde = || {
        portal
            .kde_font
            .as_deref()
            .and_then(parse_qt_font_size)
            .or_else(|| {
                let contents = read_config_file("kdeglobals")?;
                parse_qt_font_size(&ini_value(&contents, "General", "font")?)
            })
    };
    let size = match desktop {
        LinuxDesktop::Adwaita => gnome().or_else(kde),
        LinuxDesktop::Breeze => kde().or_else(gnome),
    }
    .unwrap_or(FontSize::Points(desktop.default_font_points()));
    let scale = portal
        .text_scaling_factor
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0) as f32;
    size.logical_pixels() * scale
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FontSize {
    Points(f32),
    Pixels(f32),
}

impl FontSize {
    /// Toolkits render points at 96 DPI before the display scale factor.
    fn logical_pixels(self) -> f32 {
        match self {
            Self::Points(points) => points * 96.0 / 72.0,
            Self::Pixels(pixels) => pixels,
        }
    }

    fn positive(self) -> Option<Self> {
        let (Self::Points(size) | Self::Pixels(size)) = self;
        (size.is_finite() && size > 0.0).then_some(self)
    }
}

/// Parse a Pango font description such as `Cantarell 11` or `Inter Bold 14px`.
fn parse_pango_font_size(description: &str) -> Option<FontSize> {
    let size = description.trim().rsplit(' ').next()?;
    match size.strip_suffix("px") {
        Some(pixels) => pixels.parse().ok().map(FontSize::Pixels),
        None => size.parse().ok().map(FontSize::Points),
    }?
    .positive()
}

/// Parse a Qt font string such as `Noto Sans,10,-1,5,50,0,0,0,0,0`, whose
/// second field is the point size or -1 with the pixel size third.
fn parse_qt_font_size(description: &str) -> Option<FontSize> {
    let mut fields = description.split(',').skip(1).map(str::trim);
    let points: f32 = fields.next()?.parse().ok()?;
    if points > 0.0 {
        return FontSize::Points(points).positive();
    }
    FontSize::Pixels(fields.next()?.parse().ok()?).positive()
}

fn read_config_file(relative: &str) -> Option<String> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    std::fs::read_to_string(config_home.join(relative)).ok()
}

fn ini_value(contents: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in contents.lines().map(str::trim) {
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            in_section = name == section;
        } else if in_section
            && let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kde_sessions_use_breeze_and_others_use_adwaita() {
        let desktop = LinuxDesktop::from_xdg_current_desktop;
        assert_eq!(desktop(Some("KDE")), LinuxDesktop::Breeze);
        assert_eq!(desktop(Some("ubuntu:KDE")), LinuxDesktop::Breeze);
        assert_eq!(desktop(Some("ubuntu:GNOME")), LinuxDesktop::Adwaita);
        assert_eq!(desktop(Some("KDEish")), LinuxDesktop::Adwaita);
        assert_eq!(desktop(None), LinuxDesktop::Adwaita);
    }

    #[test]
    fn portal_accent_rejects_unset_out_of_range_components() {
        assert_eq!(
            accent_from_portal_rgb(0.2078, 0.5176, 0.8941),
            Some(hsla_from_rgb_u8(53, 132, 228, 1.0))
        );
        assert_eq!(accent_from_portal_rgb(-1.0, -1.0, -1.0), None);
        assert_eq!(accent_from_portal_rgb(0.5, 1.5, 0.5), None);
    }

    #[test]
    fn desktop_font_descriptions_resolve_to_logical_pixels() {
        let pango = |text| parse_pango_font_size(text).map(FontSize::logical_pixels);
        let qt = |text| parse_qt_font_size(text).map(FontSize::logical_pixels);

        assert_eq!(pango("Cantarell 11"), Some(11.0 * 4.0 / 3.0));
        assert_eq!(pango("Noto Sans Bold Italic 10.5"), Some(14.0));
        assert_eq!(pango("Inter 15px"), Some(15.0));
        assert_eq!(pango("Cantarell"), None);
        assert_eq!(pango("Cantarell 0"), None);

        assert_eq!(qt("Noto Sans,10,-1,5,50,0,0,0,0,0"), Some(10.0 * 4.0 / 3.0));
        assert_eq!(qt("Inter,10.5,-1,5,400,0,0,0,0,0,0,0,0,0,0,1"), Some(14.0));
        assert_eq!(qt("Noto Sans,-1,16,5,50,0,0,0,0,0"), Some(16.0));
        assert_eq!(qt("Noto Sans"), None);
    }

    #[test]
    fn ini_values_are_scoped_to_their_section() {
        let contents = "[Colors:View]\nfont=Wrong,9\n\n[General]\nColorScheme=BreezeDark\n font = Noto Sans,10,-1\n";
        assert_eq!(
            ini_value(contents, "General", "font").as_deref(),
            Some("Noto Sans,10,-1")
        );
        assert_eq!(ini_value(contents, "Settings", "font"), None);
    }
}
