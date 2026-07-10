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

// Cross-platform in-window application menu. It is only rendered on non-macOS,
// but compiling it everywhere keeps the shared menu code typechecked locally.
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod application_menu;

pub use platform_titlebar::PlatformTitleBar;

use gpui::prelude::FluentBuilder;

#[cfg(target_os = "linux")]
pub use linux_platform_detector::{LinuxPlatformInfo, get_platform_info, refresh_platform_info};
#[cfg(target_os = "linux")]
pub use linux_titlebar::LinuxTitlebar;
#[cfg(target_os = "linux")]
pub use linux_window_controls::LinuxWindowControls;

use gpui::{
    AnyView, AppContext, Context, Entity, Hsla, IntoElement, ParentElement, Pixels, Render, Styled,
    Window, div, px,
};
#[cfg(target_os = "windows")]
use gpui::{InteractiveElement, WindowControlArea};

#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_BUTTON_SIZE: f32 = 46.0;
#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_BUTTON_COUNT: f32 = 3.0;
#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_GAP: f32 = 0.0;
#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_HORIZONTAL_PADDING: f32 = 0.0;
#[cfg(target_os = "windows")]
const WINDOWS_CONTROL_RIGHT_INSET: f32 = 0.0;
#[cfg(target_os = "windows")]
const WINDOWS_UI_FONT_FAMILY: &str = "Segoe UI Variable";

const TITLEBAR_ACTION_LANE_WIDTH: f32 = 32.0;
const TITLEBAR_ACTION_RIGHT_INSET: f32 = 8.0;

#[cfg(target_os = "windows")]
fn windows_caption_controls_width() -> f32 {
    WINDOWS_CONTROL_BUTTON_SIZE * WINDOWS_CONTROL_BUTTON_COUNT
        + WINDOWS_CONTROL_GAP * (WINDOWS_CONTROL_BUTTON_COUNT - 1.0)
        + WINDOWS_CONTROL_HORIZONTAL_PADDING
        + WINDOWS_CONTROL_RIGHT_INSET
}

#[cfg(target_os = "windows")]
fn windows_titlebar_content_width(viewport_width: f32) -> f32 {
    (viewport_width - windows_caption_controls_width()).max(0.0)
}

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    filename: String,
    leading_sidebar_background: Option<platform_titlebar::TitleBarLeadingSidebarBackground>,
    trailing_view: Option<AnyView>,
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
            leading_sidebar_background: None,
            trailing_view: None,
            #[cfg(not(target_os = "macos"))]
            application_menu,
        }
    }

    pub fn set_filename(&mut self, filename: String) -> bool {
        if self.filename == filename {
            return false;
        }
        self.filename = filename;
        true
    }

    pub fn set_leading_sidebar_background(
        &mut self,
        width: Pixels,
        background: Hsla,
        separator: Hsla,
    ) -> bool {
        let next = Some(platform_titlebar::TitleBarLeadingSidebarBackground {
            width,
            background,
            separator,
        });
        if self.leading_sidebar_background == next {
            return false;
        }

        self.leading_sidebar_background = next;
        true
    }

    pub fn clear_leading_sidebar_background(&mut self) -> bool {
        if self.leading_sidebar_background.is_none() {
            return false;
        }

        self.leading_sidebar_background = None;
        true
    }

    /// Set an application-owned view in the title bar's fixed trailing action lane.
    pub fn set_trailing_view(&mut self, trailing_view: Option<AnyView>) -> bool {
        if self.trailing_view == trailing_view {
            return false;
        }

        self.trailing_view = trailing_view;
        true
    }

    pub fn height(window: &Window, cx: &gpui::App) -> gpui::Pixels {
        PlatformTitleBar::height(window, cx)
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(not(target_os = "windows"))]
        let _ = window;

        // Update platform titlebar with content
        let leading_sidebar_background = self.leading_sidebar_background;
        self.platform_titlebar.update(cx, |titlebar, _cx| {
            titlebar.set_title(self.filename.clone());
            titlebar.set_leading_sidebar_background(leading_sidebar_background);
            #[cfg(target_os = "windows")]
            titlebar.set_show_title(false);
        });

        #[cfg(target_os = "windows")]
        {
            if let Some(menu) = &self.application_menu {
                let content_width = px(windows_titlebar_content_width(f32::from(
                    window.viewport_size().width,
                )));
                let titlebar_tokens = cx.global::<crate::Theme>().tokens.titlebar_tokens();
                let tokens = cx.global::<crate::Theme>().tokens;

                return div()
                    .relative()
                    .w_full()
                    .h(TitleBar::height(window, cx))
                    .min_h(TitleBar::height(window, cx))
                    .flex_shrink_0()
                    .child(self.platform_titlebar.clone())
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .w(content_width)
                            .h_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .pr(px(TITLEBAR_ACTION_LANE_WIDTH))
                            .overflow_hidden()
                            .child(menu.clone())
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .items_center()
                                    .window_control_area(WindowControlArea::Drag)
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_family(WINDOWS_UI_FONT_FAMILY)
                                    .text_size(tokens.sizes.text_sm)
                                    .text_color(tokens.chrome.text_chrome_secondary)
                                    .child(self.filename.clone()),
                            ),
                    )
                    .when_some(self.trailing_view.clone(), |titlebar, trailing_view| {
                        titlebar.child(
                            div()
                                .absolute()
                                .top_0()
                                .right(px(windows_caption_controls_width()))
                                .w(px(TITLEBAR_ACTION_LANE_WIDTH))
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(trailing_view),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(1.0))
                            .bg(titlebar_tokens.border),
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
                    .child(div().relative().w_full().child(titlebar_view).when_some(
                        self.trailing_view.clone(),
                        |titlebar, trailing_view| {
                            titlebar.child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right(px(TITLEBAR_ACTION_RIGHT_INSET))
                                    .w(px(TITLEBAR_ACTION_LANE_WIDTH))
                                    .h(TitleBar::height(window, cx))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(trailing_view),
                            )
                        },
                    ))
                    .child(menu.clone())
                    .into_any_element();
            }
        }

        // macOS (or fallback): overlay application actions without affecting the centred title.
        div()
            .relative()
            .w_full()
            .child(self.platform_titlebar.clone())
            .when_some(self.trailing_view.clone(), |titlebar, trailing_view| {
                titlebar.child(
                    div()
                        .absolute()
                        .top_0()
                        .right(px(TITLEBAR_ACTION_RIGHT_INSET))
                        .w(px(TITLEBAR_ACTION_LANE_WIDTH))
                        .h(TitleBar::height(window, cx))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(trailing_view),
                )
            })
            .into_any_element()
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{windows_caption_controls_width, windows_titlebar_content_width};

    #[test]
    fn windows_caption_gutter_matches_three_system_buttons() {
        assert_eq!(windows_caption_controls_width(), 138.0);
    }

    #[test]
    fn windows_titlebar_content_width_reserves_caption_gutter() {
        assert_eq!(windows_titlebar_content_width(1200.0), 1062.0);
        assert_eq!(windows_titlebar_content_width(100.0), 0.0);
    }
}
