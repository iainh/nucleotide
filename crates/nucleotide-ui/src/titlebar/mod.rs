// ABOUTME: Titlebar module implementing custom window decorations with platform-specific controls
// ABOUTME: Based on Zed's titlebar implementation pattern for consistent GPUI window management

mod platform_titlebar;
mod window_controls;

// Linux-specific titlebar components
#[cfg(target_os = "linux")]
mod linux_platform_detector;
#[cfg(target_os = "linux")]
mod linux_titlebar;
#[cfg(target_os = "linux")]
mod linux_window_controls;

pub use platform_titlebar::PlatformTitleBar;

#[cfg(target_os = "linux")]
pub use linux_platform_detector::{LinuxPlatformInfo, get_platform_info, refresh_platform_info};
#[cfg(target_os = "linux")]
pub use linux_titlebar::LinuxTitlebar;
#[cfg(target_os = "linux")]
pub use linux_window_controls::LinuxWindowControls;

use gpui::{AppContext, Context, Entity, IntoElement, Render, Window};

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    filename: String,
}

impl TitleBar {
    pub fn new(id: impl Into<gpui::ElementId>, cx: &mut Context<Self>) -> Self {
        let platform_titlebar = cx.new(|_cx| PlatformTitleBar::new(id));

        Self {
            platform_titlebar,
            filename: "Nucleotide".to_string(),
        }
    }

    pub fn set_filename(&mut self, filename: String) {
        self.filename = filename;
    }

    pub fn height(window: &Window) -> gpui::Pixels {
        // Use PlatformTitleBar's height calculation which now uses tokens
        PlatformTitleBar::height(window)
    }
}

impl Render for TitleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update platform titlebar with content
        self.platform_titlebar.update(cx, |titlebar, _cx| {
            titlebar.set_title(self.filename.clone());
        });

        self.platform_titlebar.clone()
    }
}
