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

// Cross‑platform in-window application menu (non-macOS only)
#[cfg(not(target_os = "macos"))]
mod application_menu;

pub use platform_titlebar::PlatformTitleBar;

#[cfg(target_os = "linux")]
pub use linux_platform_detector::{LinuxPlatformInfo, get_platform_info, refresh_platform_info};
#[cfg(target_os = "linux")]
pub use linux_titlebar::LinuxTitlebar;
#[cfg(target_os = "linux")]
pub use linux_window_controls::LinuxWindowControls;

use gpui::{AppContext, Context, Entity, IntoElement, Render, Window};
#[cfg(not(target_os = "macos"))]
use gpui::{ParentElement, Styled, div, px};

#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_BUTTON_SIZE: f32 = 24.0;
#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_BUTTON_COUNT: f32 = 3.0;
#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_GAP: f32 = 4.0;
#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_HORIZONTAL_PADDING: f32 = 16.0;
#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_RIGHT_INSET: f32 = 8.0;

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    filename: String,
    #[cfg(not(target_os = "macos"))]
    application_menu: Option<Entity<application_menu::ApplicationMenu>>,
}

impl TitleBar {
    pub fn new(id: impl Into<gpui::ElementId>, cx: &mut Context<Self>) -> Self {
        let platform_titlebar = cx.new(|_cx| PlatformTitleBar::new(id));

        #[cfg(not(target_os = "macos"))]
        let application_menu = Some(cx.new(|cx| {
            #[cfg(target_os = "windows")]
            {
                application_menu::ApplicationMenu::new_embedded_in_titlebar(cx)
            }

            #[cfg(not(target_os = "windows"))]
            {
                application_menu::ApplicationMenu::new(cx)
            }
        }));

        Self {
            platform_titlebar,
            filename: "Nucleotide".to_string(),
            #[cfg(not(target_os = "macos"))]
            application_menu,
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(not(target_os = "windows"))]
        let _ = window;

        // Update platform titlebar with content
        self.platform_titlebar.update(cx, |titlebar, _cx| {
            titlebar.set_title(self.filename.clone());
            #[cfg(target_os = "windows")]
            titlebar.set_show_title(false);
        });

        #[cfg(target_os = "windows")]
        {
            if let Some(menu) = &self.application_menu {
                let control_width = WINDOWS_CONTROL_BUTTON_SIZE * WINDOWS_CONTROL_BUTTON_COUNT
                    + WINDOWS_CONTROL_GAP * (WINDOWS_CONTROL_BUTTON_COUNT - 1.0)
                    + WINDOWS_CONTROL_HORIZONTAL_PADDING
                    + WINDOWS_CONTROL_RIGHT_INSET;
                let menu_width =
                    px((f32::from(window.viewport_size().width) - control_width).max(0.0));

                return div()
                    .relative()
                    .w_full()
                    .child(self.platform_titlebar.clone())
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .w(menu_width)
                            .child(menu.clone()),
                    )
                    .into_any_element();
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            if let Some(menu) = &self.application_menu {
                let titlebar_view = self.platform_titlebar.clone();
                return div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(titlebar_view)
                    .child(menu.clone())
                    .into_any_element();
            }
        }

        // macOS (or fallback): just the platform titlebar
        self.platform_titlebar.clone().into_any_element()
    }
}
