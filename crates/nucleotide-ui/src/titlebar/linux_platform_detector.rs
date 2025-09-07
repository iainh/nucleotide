// ABOUTME: Linux-specific platform detection for desktop environments and window manager capabilities
// ABOUTME: Detects GNOME, KDE, window manager support for decorations and optimal button layouts

// No local usage; keep imports minimal
use std::env;

use nucleotide_logging::{debug, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEnvironment {
    Gnome,
    Kde,
    Xfce,
    Lxde,
    I3,
    Sway,
    Awesome,
    Bspwm,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowManager {
    Mutter,  // GNOME's window manager
    KWin,    // KDE's window manager
    Xfwm4,   // XFCE's window manager
    I3,      // i3 tiling window manager
    Sway,    // Wayland compositor
    Awesome, // Dynamic window manager
    Bspwm,   // Binary space partitioning window manager
    Openbox, // Lightweight window manager
    Fluxbox, // Lightweight window manager
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowButtonLayout {
    /// Buttons on the right: minimize, maximize, close
    Right,
    /// Buttons on the left: close, minimize, maximize (GNOME style)
    Left,
    /// Custom layout detected from environment
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositorCapability {
    /// Supports client-side decorations
    ClientSideDecorations,
    /// Only supports server-side decorations
    ServerSideDecorations,
    /// Mixed support - prefer client-side
    Mixed,
    /// Unknown capability
    Unknown,
}

#[derive(Debug, Clone)]
pub struct LinuxPlatformInfo {
    pub desktop_environment: DesktopEnvironment,
    pub window_manager: WindowManager,
    pub button_layout: WindowButtonLayout,
    pub compositor_capability: CompositorCapability,
    pub supports_minimize: bool,
    pub supports_maximize: bool,
    pub theme_variant: Option<String>, // "dark" or "light"
}

impl Default for LinuxPlatformInfo {
    fn default() -> Self {
        Self {
            desktop_environment: DesktopEnvironment::Unknown,
            window_manager: WindowManager::Unknown,
            button_layout: WindowButtonLayout::Right,
            compositor_capability: CompositorCapability::ClientSideDecorations,
            supports_minimize: true,
            supports_maximize: true,
            theme_variant: None,
        }
    }
}

impl LinuxPlatformInfo {
    /// Detect Linux platform information from environment variables and system state
    pub fn detect() -> Self {
        let desktop_environment = detect_desktop_environment();
        debug!("Detected desktop environment: {:?}", desktop_environment);

        let window_manager = detect_window_manager();
        debug!("Detected window manager: {:?}", window_manager);

        let button_layout = detect_button_layout(desktop_environment);
        debug!("Using button layout: {:?}", button_layout);

        let compositor_capability = detect_compositor_capability(window_manager);
        debug!("Compositor capability: {:?}", compositor_capability);

        let (supports_minimize, supports_maximize) = detect_window_capabilities(window_manager);
        debug!(
            "Window capabilities - minimize: {}, maximize: {}",
            supports_minimize, supports_maximize
        );

        let theme_variant = detect_system_theme();
        debug!("System theme variant: {:?}", theme_variant);

        Self {
            desktop_environment,
            window_manager,
            button_layout,
            compositor_capability,
            supports_minimize,
            supports_maximize,
            theme_variant,
        }
    }
}

fn detect_desktop_environment() -> DesktopEnvironment {
    // Check XDG_CURRENT_DESKTOP first (most reliable)
    if let Ok(desktop) = env::var("XDG_CURRENT_DESKTOP") {
        let desktop = desktop.to_lowercase();
        debug!("XDG_CURRENT_DESKTOP: {}", desktop);

        if desktop.contains("gnome") {
            return DesktopEnvironment::Gnome;
        } else if desktop.contains("kde") || desktop.contains("plasma") {
            return DesktopEnvironment::Kde;
        } else if desktop.contains("xfce") {
            return DesktopEnvironment::Xfce;
        } else if desktop.contains("lxde") || desktop.contains("lxqt") {
            return DesktopEnvironment::Lxde;
        } else if desktop.contains("i3") {
            return DesktopEnvironment::I3;
        } else if desktop.contains("sway") {
            return DesktopEnvironment::Sway;
        } else if desktop.contains("awesome") {
            return DesktopEnvironment::Awesome;
        } else if desktop.contains("bspwm") {
            return DesktopEnvironment::Bspwm;
        }
    }

    // Fallback to DESKTOP_SESSION
    if let Ok(session) = env::var("DESKTOP_SESSION") {
        let session = session.to_lowercase();
        debug!("DESKTOP_SESSION: {}", session);

        if session.contains("gnome") {
            return DesktopEnvironment::Gnome;
        } else if session.contains("kde") || session.contains("plasma") {
            return DesktopEnvironment::Kde;
        } else if session.contains("xfce") {
            return DesktopEnvironment::Xfce;
        } else if session.contains("lxde") {
            return DesktopEnvironment::Lxde;
        }
    }

    // Check for specific environment variables
    if env::var("GNOME_DESKTOP_SESSION_ID").is_ok() || env::var("GNOME_SHELL_SESSION_MODE").is_ok()
    {
        return DesktopEnvironment::Gnome;
    }

    if env::var("KDE_FULL_SESSION").is_ok() || env::var("KDE_SESSION_VERSION").is_ok() {
        return DesktopEnvironment::Kde;
    }

    warn!("Could not detect desktop environment, using Unknown");
    DesktopEnvironment::Unknown
}

fn detect_window_manager() -> WindowManager {
    // Check specific window manager environment variables
    let env_vars = [
        ("SWAYSOCK", WindowManager::Sway),
        ("I3SOCK", WindowManager::I3),
        ("AWESOME_VERSION", WindowManager::Awesome),
        ("BSPWM_SOCKET", WindowManager::Bspwm),
    ];

    for (var, wm) in env_vars {
        if env::var(var).is_ok() {
            debug!("Detected window manager from {}: {:?}", var, wm);
            return wm;
        }
    }

    // Check window manager by desktop environment
    match detect_desktop_environment() {
        DesktopEnvironment::Gnome => WindowManager::Mutter,
        DesktopEnvironment::Kde => WindowManager::KWin,
        DesktopEnvironment::Xfce => WindowManager::Xfwm4,
        DesktopEnvironment::I3 => WindowManager::I3,
        DesktopEnvironment::Sway => WindowManager::Sway,
        DesktopEnvironment::Awesome => WindowManager::Awesome,
        DesktopEnvironment::Bspwm => WindowManager::Bspwm,
        _ => {
            // Try to detect from process list or other methods
            // For now, use Unknown
            warn!("Could not detect window manager, using Unknown");
            WindowManager::Unknown
        }
    }
}

fn detect_button_layout(_de: DesktopEnvironment) -> WindowButtonLayout {
    // Project policy: always render controls on the right (minimize, maximize, close)
    // Many Linux environments vary or allow user customization, but our custom
    // titlebar uses a consistent right-side layout to avoid confusion and
    // simplify spacing logic across DEs.
    if let Ok(layout) = env::var("NUCL_LINUX_BUTTON_LAYOUT") {
        // Optional override for development/testing: set to "left" or "right"
        let layout = layout.to_lowercase();
        if layout.contains("left") {
            return WindowButtonLayout::Left;
        }
    }

    WindowButtonLayout::Right
}

fn detect_compositor_capability(wm: WindowManager) -> CompositorCapability {
    match wm {
        WindowManager::Mutter => CompositorCapability::ClientSideDecorations, // GNOME supports CSD well
        WindowManager::KWin => CompositorCapability::Mixed,                   // KDE supports both
        WindowManager::Sway => CompositorCapability::ClientSideDecorations,   // Wayland compositor
        WindowManager::I3 | WindowManager::Awesome | WindowManager::Bspwm => {
            CompositorCapability::ServerSideDecorations
        } // Tiling WMs prefer SSD
        WindowManager::Xfwm4 => CompositorCapability::Mixed,                  // XFCE supports both
        _ => CompositorCapability::ClientSideDecorations,                     // Default to CSD
    }
}

fn detect_window_capabilities(wm: WindowManager) -> (bool, bool) {
    match wm {
        // Tiling window managers don't typically support minimize/maximize
        WindowManager::I3 | WindowManager::Sway | WindowManager::Awesome | WindowManager::Bspwm => {
            (false, false)
        }

        // Traditional window managers support both
        WindowManager::Mutter
        | WindowManager::KWin
        | WindowManager::Xfwm4
        | WindowManager::Openbox
        | WindowManager::Fluxbox => (true, true),

        // Unknown - assume basic capabilities
        WindowManager::Unknown => (true, true),
    }
}

fn detect_system_theme() -> Option<String> {
    // Check GTK theme preference
    if let Ok(theme) = env::var("GTK_THEME") {
        let theme_lower = theme.to_lowercase();
        if theme_lower.contains("dark") {
            return Some("dark".to_string());
        } else if theme_lower.contains("light") {
            return Some("light".to_string());
        }
    }

    // Check gsettings would require process spawning
    // TODO: Could implement: gsettings get org.gnome.desktop.interface gtk-theme

    // Check QT theme for KDE
    if let Ok(theme) = env::var("QT_STYLE_OVERRIDE") {
        let theme_lower = theme.to_lowercase();
        if theme_lower.contains("dark") {
            return Some("dark".to_string());
        }
    }

    None
}

/// Get current Linux platform information (cached)
static PLATFORM_INFO: std::sync::OnceLock<LinuxPlatformInfo> = std::sync::OnceLock::new();

pub fn get_platform_info() -> &'static LinuxPlatformInfo {
    PLATFORM_INFO.get_or_init(LinuxPlatformInfo::detect)
}

/// Force re-detection of platform information (useful for runtime changes)
/// Note: Due to OnceLock design, this will only work if called before the first get_platform_info()
pub fn refresh_platform_info() {
    // With OnceLock, we can't refresh after initialization
    // This is intentional for thread safety - platform info should be stable during runtime
    warn!(
        "Platform info refresh not supported with OnceLock - platform detection is cached permanently"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    // No environment mutations required in these tests

    // Environment-dependent detection tests removed to avoid mutating process env
    // which can be restricted in some CI environments.

    #[test]
    fn test_button_layout_detection() {
        // Policy: always right by default across DEs
        assert_eq!(
            detect_button_layout(DesktopEnvironment::Gnome),
            WindowButtonLayout::Right
        );
        assert_eq!(
            detect_button_layout(DesktopEnvironment::Kde),
            WindowButtonLayout::Right
        );
    }

    #[test]
    fn test_window_capabilities() {
        let (min, max) = detect_window_capabilities(WindowManager::I3);
        assert!(!min && !max); // Tiling WM shouldn't support minimize/maximize

        let (min, max) = detect_window_capabilities(WindowManager::Mutter);
        assert!(min && max); // Traditional WM should support both
    }
}
