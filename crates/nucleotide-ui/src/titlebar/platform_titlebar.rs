// ABOUTME: Platform-agnostic titlebar implementation that handles window decorations and controls
// ABOUTME: Provides consistent titlebar behavior across Linux, macOS, and Windows platforms

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, Context, Decorations, ElementId, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Render, Styled, Window, WindowControlArea,
};

use crate::titlebar::window_controls::WindowControls;

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

pub struct PlatformTitleBar {
    id: ElementId,
    platform_style: PlatformStyle,
    title: String,
}

impl PlatformTitleBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let platform_style = PlatformStyle::platform();
        Self {
            id: id.into(),
            platform_style,
            title: String::new(),
        }
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn height(window: &Window) -> Pixels {
        #[cfg(target_os = "windows")]
        return px(32.0);

        #[cfg(not(target_os = "windows"))]
        return (1.75 * window.rem_size()).max(px(34.0));
    }

    pub fn title_bar_color(&self, _window: &Window, cx: &App) -> Hsla {
        // Get color from theme manager
        let ui_theme = cx.global::<crate::Theme>();
        ui_theme.surface
    }
}

impl Render for PlatformTitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let decorations = window.window_decorations();
        let height = Self::height(window);
        let titlebar_color = self.title_bar_color(window, cx);
        // Get the border color from UI theme
        let ui_theme = cx.global::<crate::Theme>();
        let border_color = ui_theme.border;

        // macOS traffic light padding
        const MAC_TRAFFIC_LIGHT_PADDING: f32 = 71.0;

        // Set window insets based on decoration type
        match decorations {
            Decorations::Client { .. } => window.set_client_inset(px(0.0)), // We'll handle shadows separately
            Decorations::Server => window.set_client_inset(px(0.0)),
        }

        // Build the titlebar
        div()
            .flex()
            .flex_row()
            .id(self.id.clone())
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(height)
            .bg(titlebar_color)
            .border_b_1()
            .border_color(border_color)
            .map(|this| {
                if window.is_fullscreen() {
                    this.pl_2()
                } else if self.platform_style == PlatformStyle::Mac {
                    this.pl(px(MAC_TRAFFIC_LIGHT_PADDING))
                } else {
                    this.pl_2()
                }
            })
            .map(|el| match decorations {
                Decorations::Server => el,
                Decorations::Client { tiling } => el
                    .when(!(tiling.top || tiling.right), gpui::Styled::rounded_tr_md)
                    .when(!(tiling.top || tiling.left), gpui::Styled::rounded_tl_md),
            })
            .content_stretch()
            .child(
                // Main content area
                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .h_full()
                    .items_center()
                    .justify_center() // Center the content
                    .relative() // For absolute positioning of window controls
                    .px_2()
                    // Stop propagation on titlebar interactions
                    .on_mouse_down(MouseButton::Left, |event, window, cx| {
                        // Only start window move if not clicking on controls
                        let bounds = window.window_bounds().get_bounds();
                        let control_area_start = bounds.size.width.0 - 150.0;

                        if event.position.x.0 < control_area_start {
                            window.start_window_move();
                        }
                        cx.stop_propagation();
                    })
                    .on_mouse_move(|_, _, cx| cx.stop_propagation())
                    .child(
                        // Title text - centered and bold
                        div().flex().items_center().gap_2().child(
                            div()
                                .text_sm() // Standard small text size
                                .font_weight(gpui::FontWeight::SEMIBOLD) // Semibold weight
                                .text_color(ui_theme.text)
                                .child(self.title.clone()),
                        ),
                    ),
            )
            .when(!window.is_fullscreen(), |title_bar| {
                // Add platform-specific window controls
                match self.platform_style {
                    PlatformStyle::Mac => {
                        // macOS uses native traffic lights, no custom controls needed
                        title_bar
                    }
                    PlatformStyle::Linux | PlatformStyle::Windows => {
                        title_bar.child(
                            WindowControls::new(self.platform_style), // Note: WindowControlArea doesn't have WindowControls variant in this GPUI version
                        )
                    }
                }
            })
    }
}
