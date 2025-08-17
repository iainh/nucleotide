// ABOUTME: Platform-agnostic titlebar implementation that handles window decorations and controls
// ABOUTME: Provides consistent titlebar behavior across Linux, macOS, and Windows platforms

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, Context, Decorations, ElementId, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Render, Styled, Window, WindowControlArea,
};

use crate::titlebar::window_controls::WindowControls;
use crate::tokens::{ColorContext, TitleBarTokens};
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
        // Use theme provider for consistent height with token system
        if let Some(theme_provider) =
            crate::providers::use_provider::<crate::providers::ThemeProvider>()
        {
            let tokens = theme_provider.titlebar_tokens(ColorContext::OnSurface);
            debug!(
                "TITLEBAR HEIGHT: Using theme provider tokens, height={:?}",
                tokens.height
            );
            return (1.75 * window.rem_size()).max(tokens.height);
        }

        debug!("TITLEBAR HEIGHT: No theme provider, using fallback heights");
        #[cfg(target_os = "windows")]
        return px(32.0);

        #[cfg(not(target_os = "windows"))]
        return (1.75 * window.rem_size()).max(px(34.0));
    }
}

impl Render for PlatformTitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let decorations = window.window_decorations();

        // Get titlebar tokens from theme provider
        let theme_provider = crate::providers::use_provider::<crate::providers::ThemeProvider>();
        debug!(
            "TITLEBAR RENDER: Theme provider available: {}",
            theme_provider.is_some()
        );

        let titlebar_tokens = if let Some(provider) = theme_provider {
            // Use OnSurface context for standard titlebar appearance
            let tokens = provider.titlebar_tokens(ColorContext::OnSurface);
            debug!("TITLEBAR RENDER: Using theme provider tokens - bg={:?}, fg={:?}, border={:?}, height={:?}", 
                tokens.background, tokens.foreground, tokens.border, tokens.height);
            tokens
        } else {
            // Fallback: use global theme for tokens
            let ui_theme = cx.global::<crate::Theme>();
            let tokens = TitleBarTokens::on_surface(&ui_theme.tokens);
            debug!("TITLEBAR RENDER: Using fallback global theme tokens - bg={:?}, fg={:?}, border={:?}, height={:?}", 
                tokens.background, tokens.foreground, tokens.border, tokens.height);
            tokens
        };

        let height = titlebar_tokens.height;
        debug!("TITLEBAR RENDER: Final titlebar height: {:?}", height);

        // macOS traffic light padding
        const MAC_TRAFFIC_LIGHT_PADDING: f32 = 71.0;

        // Set window insets based on decoration type
        match decorations {
            Decorations::Client { .. } => window.set_client_inset(px(0.0)), // We'll handle shadows separately
            Decorations::Server => window.set_client_inset(px(0.0)),
        }

        debug!(
            "TITLEBAR RENDER: Applying styles - background={:?}, border={:?}",
            titlebar_tokens.background, titlebar_tokens.border
        );

        // Build the titlebar
        div()
            .flex()
            .flex_row()
            .id(self.id.clone())
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(height)
            .bg(titlebar_tokens.background)
            .border_b_1()
            .border_color(titlebar_tokens.border)
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
                        // Title text - centered and styled with computed colors
                        div().flex().items_center().gap_2().child({
                            debug!(
                                "TITLEBAR RENDER: Applying text color to title: {:?}",
                                titlebar_tokens.foreground
                            );
                            div()
                                .text_size(px(14.0)) // Standard titlebar font size
                                .font_weight(gpui::FontWeight::MEDIUM) // Slightly bold for titlebar
                                .text_color(titlebar_tokens.foreground)
                                .child(self.title.clone())
                        }),
                    ),
            )
            .when(!window.is_fullscreen(), |title_bar| {
                // Add platform-specific window controls
                match self.platform_style {
                    PlatformStyle::Mac => {
                        // macOS uses native traffic lights, no custom controls needed
                        title_bar
                    }
                    PlatformStyle::Linux | PlatformStyle::Windows => title_bar.child(
                        WindowControls::new(self.platform_style)
                            .with_titlebar_tokens(titlebar_tokens),
                    ),
                }
            })
    }
}
