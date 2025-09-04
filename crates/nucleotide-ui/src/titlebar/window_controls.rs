// ABOUTME: Window control buttons implementation for custom titlebars on Linux and Windows
// ABOUTME: Provides minimize, maximize/restore, and close buttons with platform-specific styling

use gpui::{
    App, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce, Styled,
    Window, WindowControlArea, hsla, svg,
};

use crate::styling::{ColorTheory, StyleSize, StyleState, StyleVariant, compute_component_style};
use crate::titlebar::platform_titlebar::PlatformStyle;
use crate::tokens::TitleBarTokens;
use nucleotide_logging::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowControlType {
    Minimize,
    Restore,
    Maximize,
    Close,
}

impl WindowControlType {
    pub fn asset_icon_path(&self) -> &'static str {
        match self {
            WindowControlType::Minimize => "icons/window-minimize.svg",
            WindowControlType::Restore => "icons/window-restore.svg",
            WindowControlType::Maximize => "icons/window-maximize.svg",
            WindowControlType::Close => "icons/close.svg",
        }
    }
}

pub struct WindowControlStyle {
    #[allow(dead_code)]
    pub background: Hsla,
    pub background_hover: Hsla,
    pub icon: Hsla,
    pub icon_hover: Hsla,
}

impl WindowControlStyle {
    pub fn default_from_tokens(
        titlebar_tokens: TitleBarTokens,
        _theme_tokens: &crate::DesignTokens,
    ) -> Self {
        // Create ghost button style that works on the titlebar background
        let bg = titlebar_tokens.background;
        let fg = titlebar_tokens.foreground;

        // Create hover background that's subtle on titlebar
        let hover_bg = ColorTheory::lighten(bg, 0.05);

        debug!(
            "TITLEBAR WINDOW_CONTROL: Creating default control style - bg={:?}, fg={:?}, hover_bg={:?}",
            bg, fg, hover_bg
        );

        let icon_color = ColorTheory::mix_oklch(fg, bg, 0.3);
        debug!(
            "TITLEBAR WINDOW_CONTROL: Computed icon colors - normal={:?}, hover={:?}",
            icon_color, fg
        );

        Self {
            background: hsla(0.0, 0.0, 0.0, 0.0), // Transparent by default
            background_hover: hover_bg,
            icon: icon_color, // More subtle icon color
            icon_hover: fg,
        }
    }

    pub fn default(cx: &App) -> Self {
        // Use enhanced styling system with provider support
        let ui_theme = crate::providers::use_provider::<crate::providers::ThemeProvider>()
            .map(|provider| provider.current_theme().clone())
            .unwrap_or_else(|| cx.global::<crate::Theme>().clone());

        // Use ghost variant for subtle window controls
        let default_style = compute_component_style(
            &ui_theme,
            StyleState::Default,
            StyleVariant::Ghost.as_str(),
            StyleSize::Small.as_str(),
        );
        let hover_style = compute_component_style(
            &ui_theme,
            StyleState::Hover,
            StyleVariant::Ghost.as_str(),
            StyleSize::Small.as_str(),
        );

        Self {
            background: default_style.background, // transparent by default for ghost variant
            background_hover: hover_style.background,
            icon: ui_theme.tokens.chrome.text_chrome_secondary,
            icon_hover: ui_theme.tokens.chrome.text_on_chrome,
        }
    }

    pub fn close_from_tokens(
        titlebar_tokens: TitleBarTokens,
        theme_tokens: &crate::DesignTokens,
    ) -> Self {
        // Create danger button style for close button
        let bg = titlebar_tokens.background;
        let fg = titlebar_tokens.foreground;
        let error_color = theme_tokens.editor.error;

        debug!(
            "TITLEBAR WINDOW_CONTROL: Creating close button style - bg={:?}, fg={:?}, error_color={:?}",
            bg, fg, error_color
        );

        let icon_color = ColorTheory::mix_oklch(fg, bg, 0.3);
        let icon_hover = ColorTheory::best_text_color(error_color, theme_tokens);

        debug!(
            "TITLEBAR WINDOW_CONTROL: Close button computed colors - icon={:?}, icon_hover={:?}",
            icon_color, icon_hover
        );

        Self {
            background: hsla(0.0, 0.0, 0.0, 0.0), // Transparent by default
            background_hover: error_color,
            icon: icon_color, // More subtle icon color
            icon_hover,
        }
    }

    pub fn close(cx: &App) -> Self {
        // Use enhanced styling system with provider support
        let ui_theme = crate::providers::use_provider::<crate::providers::ThemeProvider>()
            .map(|provider| provider.current_theme().clone())
            .unwrap_or_else(|| cx.global::<crate::Theme>().clone());

        // Use danger variant for close button
        let danger_style = compute_component_style(
            &ui_theme,
            StyleState::Hover,
            StyleVariant::Danger.as_str(),
            StyleSize::Small.as_str(),
        );

        Self {
            background: hsla(0.0, 0.0, 0.0, 0.0),      // transparent default
            background_hover: danger_style.background, // Use computed danger color
            icon: ui_theme.tokens.chrome.text_chrome_secondary,
            icon_hover: danger_style.foreground, // Use computed text color on danger background
        }
    }
}

#[derive(IntoElement)]
pub struct WindowControl {
    id: gpui::ElementId,
    control_type: WindowControlType,
    style: WindowControlStyle,
}

impl WindowControl {
    pub fn new(id: impl Into<gpui::ElementId>, control_type: WindowControlType, cx: &App) -> Self {
        let style = match control_type {
            WindowControlType::Close => WindowControlStyle::close(cx),
            _ => WindowControlStyle::default(cx),
        };

        Self {
            id: id.into(),
            control_type,
            style,
        }
    }

    pub fn with_tokens(
        id: impl Into<gpui::ElementId>,
        control_type: WindowControlType,
        titlebar_tokens: TitleBarTokens,
        theme_tokens: &crate::DesignTokens,
    ) -> Self {
        let style = match control_type {
            WindowControlType::Close => {
                WindowControlStyle::close_from_tokens(titlebar_tokens, theme_tokens)
            }
            _ => WindowControlStyle::default_from_tokens(titlebar_tokens, theme_tokens),
        };

        Self {
            id: id.into(),
            control_type,
            style,
        }
    }
}

impl RenderOnce for WindowControl {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        debug!(
            "TITLEBAR WINDOW_CONTROL: Rendering {:?} control with colors - icon={:?}, icon_hover={:?}, bg_hover={:?}",
            self.control_type, self.style.icon, self.style.icon_hover, self.style.background_hover
        );

        let icon = svg()
            .size_4()
            .flex_none()
            .path(self.control_type.asset_icon_path())
            .text_color(self.style.icon)
            .group_hover("", |this| this.text_color(self.style.icon_hover));

        gpui::div()
            .flex()
            .flex_row()
            .id(self.id)
            .group("")
            .cursor_pointer()
            .justify_center()
            .items_center()
            .rounded_md()
            .w_6()
            .h_6()
            .hover(|this| this.bg(self.style.background_hover))
            .child(icon)
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                match self.control_type {
                    WindowControlType::Minimize => {
                        // Note: minimize is not available in GPUI yet
                    }
                    WindowControlType::Restore | WindowControlType::Maximize => {
                        if std::env::var("NUCL_DISABLE_FULLSCREEN").ok().as_deref() == Some("1") {
                            debug!("Fullscreen/maximize disabled via NUCL_DISABLE_FULLSCREEN=1");
                        } else {
                            window.toggle_fullscreen();
                        }
                    }
                    WindowControlType::Close => {
                        cx.quit();
                    }
                }
            })
    }
}

#[derive(IntoElement)]
pub struct WindowControls {
    platform_style: PlatformStyle,
    window_control_area: Option<WindowControlArea>,
    titlebar_tokens: Option<TitleBarTokens>,
}

impl WindowControls {
    pub fn new(platform_style: PlatformStyle) -> Self {
        Self {
            platform_style,
            window_control_area: None,
            titlebar_tokens: None,
        }
    }

    #[allow(dead_code)]
    pub fn window_control_area(mut self, area: WindowControlArea) -> Self {
        self.window_control_area = Some(area);
        self
    }

    pub fn with_titlebar_tokens(mut self, titlebar_tokens: TitleBarTokens) -> Self {
        self.titlebar_tokens = Some(titlebar_tokens);
        self
    }
}

impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut controls = gpui::div()
            .flex()
            .flex_row()
            .id("window-controls")
            .absolute()
            .right_2()
            .top_0()
            .bottom_0()
            .items_center()
            .px_2()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        if let Some(area) = self.window_control_area {
            controls = controls.window_control_area(area);
        }

        // Use tokens if available, otherwise fallback to old system
        if let Some(titlebar_tokens) = self.titlebar_tokens {
            debug!(
                "TITLEBAR WINDOW_CONTROLS: Using titlebar tokens for controls - bg={:?}, fg={:?}, border={:?}",
                titlebar_tokens.background, titlebar_tokens.foreground, titlebar_tokens.border
            );

            // Get theme tokens for creating controls
            let theme_tokens = if let Some(theme_provider) =
                crate::providers::use_provider::<crate::providers::ThemeProvider>()
            {
                debug!("TITLEBAR WINDOW_CONTROLS: Using theme provider for theme tokens");
                theme_provider.current_theme.tokens
            } else {
                debug!("TITLEBAR WINDOW_CONTROLS: Using global theme for theme tokens");
                cx.global::<crate::Theme>().tokens
            };

            // Add controls based on platform with tokens
            match self.platform_style {
                PlatformStyle::Linux => controls
                    .child(WindowControl::with_tokens(
                        "minimize",
                        WindowControlType::Minimize,
                        titlebar_tokens,
                        &theme_tokens,
                    ))
                    .child(WindowControl::with_tokens(
                        "maximize-or-restore",
                        if window.is_maximized() {
                            WindowControlType::Restore
                        } else {
                            WindowControlType::Maximize
                        },
                        titlebar_tokens,
                        &theme_tokens,
                    ))
                    .child(WindowControl::with_tokens(
                        "close",
                        WindowControlType::Close,
                        titlebar_tokens,
                        &theme_tokens,
                    )),
                PlatformStyle::Windows => {
                    // Windows order: minimize, maximize, close
                    controls
                        .child(WindowControl::with_tokens(
                            "minimize",
                            WindowControlType::Minimize,
                            titlebar_tokens,
                            &theme_tokens,
                        ))
                        .child(WindowControl::with_tokens(
                            "maximize-or-restore",
                            if window.is_maximized() {
                                WindowControlType::Restore
                            } else {
                                WindowControlType::Maximize
                            },
                            titlebar_tokens,
                            &theme_tokens,
                        ))
                        .child(WindowControl::with_tokens(
                            "close",
                            WindowControlType::Close,
                            titlebar_tokens,
                            &theme_tokens,
                        ))
                }
                PlatformStyle::Mac => {
                    // macOS uses native traffic lights, return empty container
                    controls
                }
            }
        } else {
            debug!(
                "TITLEBAR WINDOW_CONTROLS: No titlebar tokens available, using fallback styling system"
            );
            // Fallback to old system without tokens
            match self.platform_style {
                PlatformStyle::Linux => controls
                    .child(WindowControl::new(
                        "minimize",
                        WindowControlType::Minimize,
                        cx,
                    ))
                    .child(WindowControl::new(
                        "maximize-or-restore",
                        if window.is_maximized() {
                            WindowControlType::Restore
                        } else {
                            WindowControlType::Maximize
                        },
                        cx,
                    ))
                    .child(WindowControl::new("close", WindowControlType::Close, cx)),
                PlatformStyle::Windows => {
                    // Windows order: minimize, maximize, close
                    controls
                        .child(WindowControl::new(
                            "minimize",
                            WindowControlType::Minimize,
                            cx,
                        ))
                        .child(WindowControl::new(
                            "maximize-or-restore",
                            if window.is_maximized() {
                                WindowControlType::Restore
                            } else {
                                WindowControlType::Maximize
                            },
                            cx,
                        ))
                        .child(WindowControl::new("close", WindowControlType::Close, cx))
                }
                PlatformStyle::Mac => {
                    // macOS uses native traffic lights, return empty container
                    controls
                }
            }
        }
    }
}
