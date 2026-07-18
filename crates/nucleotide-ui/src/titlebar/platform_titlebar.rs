// ABOUTME: Platform-agnostic titlebar implementation that handles window decorations and controls
// ABOUTME: Provides consistent titlebar behavior across Linux, macOS, and Windows platforms

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, Decorations, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, Styled, Window, WindowControlArea, div, px,
};

use crate::titlebar::window_controls::WindowControls;
use nucleotide_logging::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PlatformStyle {
    Linux,
    Mac,
    Windows,
}

impl PlatformStyle {
    pub fn platform() -> Self {
        #[cfg(target_os = "macos")]
        return Self::Mac;

        #[cfg(target_os = "windows")]
        return Self::Windows;

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Self::Linux;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TitleBarLeadingSidebarBackground {
    pub width: Pixels,
    pub background: Hsla,
    pub separator: Hsla,
}

pub struct PlatformTitleBar {
    id: ElementId,
    platform_style: PlatformStyle,
    title: String,
    show_title: bool,
    inset_applied: bool,
    leading_sidebar_background: Option<TitleBarLeadingSidebarBackground>,
}

impl PlatformTitleBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let platform_style = PlatformStyle::platform();
        Self {
            id: id.into(),
            platform_style,
            title: String::new(),
            show_title: true,
            inset_applied: false,
            leading_sidebar_background: None,
        }
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn set_show_title(&mut self, show_title: bool) {
        self.show_title = show_title;
    }

    pub fn set_leading_sidebar_background(
        &mut self,
        background: Option<TitleBarLeadingSidebarBackground>,
    ) -> bool {
        if self.leading_sidebar_background == background {
            return false;
        }

        self.leading_sidebar_background = background;
        true
    }

    pub fn height(_window: &Window, cx: &App) -> Pixels {
        cx.global::<crate::Theme>().tokens.titlebar_tokens().height
    }
}

impl Render for PlatformTitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let decorations = window.window_decorations();

        // On Linux, use enhanced Linux titlebar if available and appropriate
        #[cfg(target_os = "linux")]
        {
            use crate::titlebar::linux_titlebar::LinuxTitlebar;

            if LinuxTitlebar::should_create_for_decorations(&decorations) {
                debug!("Using enhanced Linux titlebar");
                let mut linux_titlebar =
                    LinuxTitlebar::new(self.id.clone(), cx.global::<crate::Theme>());
                linux_titlebar.set_title(self.title.clone());

                return linux_titlebar.render_element(window);
            } else {
                debug!("Using fallback platform titlebar on Linux");
            }
        }

        let titlebar_tokens = cx.global::<crate::Theme>().tokens.titlebar_tokens();

        const MAC_UNIFIED_TITLEBAR_MIN_HEIGHT: f32 = 44.0;
        let height = titlebar_tokens.height;
        let native_macos_titlebar = self.platform_style == PlatformStyle::Mac
            && f32::from(height) >= MAC_UNIFIED_TITLEBAR_MIN_HEIGHT;
        #[cfg(debug_assertions)]
        debug!("TITLEBAR RENDER: Final titlebar height: {:?}", height);

        // Reserve the native traffic-light cluster symmetrically so the title
        // remains visually centered in compact unified chrome.
        const MAC_TRAFFIC_LIGHT_PADDING: f32 = 82.0;

        // Set window insets based on decoration type only once to avoid per-frame calls
        if !self.inset_applied {
            match decorations {
                Decorations::Client { .. } => window.set_client_inset(px(0.0)), // We'll handle shadows separately
                Decorations::Server => window.set_client_inset(px(0.0)),
            }
            self.inset_applied = true;
        }

        #[cfg(debug_assertions)]
        debug!(
            "TITLEBAR RENDER: Applying styles - background={:?}, border={:?}",
            titlebar_tokens.background, titlebar_tokens.border
        );

        let leading_sidebar_background = (self.platform_style == PlatformStyle::Mac)
            .then_some(self.leading_sidebar_background)
            .flatten()
            .filter(|background| f32::from(background.width) > 0.0);

        // Build the titlebar
        let title_bar = div()
            .flex()
            .flex_row()
            .id(self.id.clone())
            .relative()
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(height)
            .min_h(height)
            .flex_shrink_0() // prevent vertical compression when layout is tight
            .when(leading_sidebar_background.is_none(), |titlebar| {
                titlebar.bg(titlebar_tokens.background)
            })
            .when(leading_sidebar_background.is_some(), |titlebar| {
                titlebar.overflow_hidden()
            })
            .when(
                self.platform_style != PlatformStyle::Windows
                    && leading_sidebar_background.is_none(),
                |titlebar| titlebar.border_b_1().border_color(titlebar_tokens.border),
            )
            .map(|this| {
                if window.is_fullscreen() {
                    this.pl_2()
                } else if native_macos_titlebar {
                    this.px(px(MAC_TRAFFIC_LIGHT_PADDING))
                } else if self.platform_style == PlatformStyle::Mac {
                    this.pl(px(71.0))
                } else {
                    this.pl_2()
                }
            })
            // Avoid rounded corners on Linux to prevent visible corner artifacts
            .map(|el| {
                if cfg!(target_os = "linux") {
                    // No extra rounding on Linux
                    return el;
                }

                match decorations {
                    Decorations::Server => el,
                    Decorations::Client { tiling } => el
                        .when(!(tiling.top || tiling.right), gpui::Styled::rounded_tr_md)
                        .when(!(tiling.top || tiling.left), gpui::Styled::rounded_tl_md),
                }
            })
            .content_stretch()
            .when_some(leading_sidebar_background, |titlebar, sidebar| {
                titlebar
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .bottom_0()
                            .w(sidebar.width)
                            .bg(sidebar.background),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(sidebar.width)
                            .right_0()
                            .bottom_0()
                            .bg(titlebar_tokens.background),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(sidebar.width)
                            .w(px(1.0))
                            .bg(sidebar.separator),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(sidebar.width)
                            .right_0()
                            .bottom_0()
                            .h(px(1.0))
                            .bg(titlebar_tokens.border),
                    )
            });

        title_bar
            .child(
                // Main content area
                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .relative() // For absolute positioning of window controls
                    .px_2()
                    .when(self.show_title, |content| {
                        content.child(
                            // Title text - centered and styled with computed colors
                            div().flex().items_center().gap_2().child({
                                #[cfg(debug_assertions)]
                                debug!(
                                    "TITLEBAR RENDER: Applying text color to title: {:?}",
                                    titlebar_tokens.foreground
                                );
                                div()
                                    .text_size(cx.global::<crate::Theme>().tokens.sizes.text_md) // Themed titlebar font size
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(titlebar_tokens.foreground)
                                    .child(self.title.clone())
                            }),
                        )
                    }),
            )
            // Always add window controls on Linux/Windows. macOS uses native controls.
            .map(|title_bar| match self.platform_style {
                PlatformStyle::Mac => {
                    // macOS uses native traffic lights, no custom controls needed
                    title_bar
                }
                PlatformStyle::Linux | PlatformStyle::Windows => title_bar.child(
                    WindowControls::new(self.platform_style).with_titlebar_tokens(titlebar_tokens),
                ),
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{hsla, px};

    use super::{PlatformTitleBar, TitleBarLeadingSidebarBackground};

    #[test]
    fn leading_sidebar_background_reports_state_changes() {
        let mut titlebar = PlatformTitleBar::new("titlebar");
        let background = TitleBarLeadingSidebarBackground {
            width: px(240.0),
            background: hsla(0.7, 0.1, 0.4, 0.7),
            separator: hsla(0.7, 0.1, 0.2, 0.5),
        };

        assert!(titlebar.set_leading_sidebar_background(Some(background)));
        assert!(!titlebar.set_leading_sidebar_background(Some(background)));
        assert!(titlebar.set_leading_sidebar_background(None));
        assert!(!titlebar.set_leading_sidebar_background(None));
    }
}
