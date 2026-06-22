// ABOUTME: Window control buttons implementation for custom titlebars on Linux and Windows
// ABOUTME: Provides minimize, maximize/restore, and close buttons with platform-specific styling

use gpui::{
    App, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce, Rgba,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, svg,
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

    pub fn windows_caption_icon(&self) -> &'static str {
        match self {
            WindowControlType::Minimize => "\u{e921}",
            WindowControlType::Restore => "\u{e923}",
            WindowControlType::Maximize => "\u{e922}",
            WindowControlType::Close => "\u{e8bb}",
        }
    }

    pub fn window_control_area(&self) -> WindowControlArea {
        match self {
            WindowControlType::Minimize => WindowControlArea::Min,
            WindowControlType::Restore | WindowControlType::Maximize => WindowControlArea::Max,
            WindowControlType::Close => WindowControlArea::Close,
        }
    }
}

const WINDOWS_CAPTION_BUTTON_WIDTH: f32 = 46.0;
const WINDOWS_CAPTION_ICON_SIZE: f32 = 10.0;

fn windows_close_hover_background() -> Hsla {
    Rgba {
        r: 232.0 / 255.0,
        g: 17.0 / 255.0,
        b: 35.0 / 255.0,
        a: 1.0,
    }
    .into()
}

#[cfg(target_os = "windows")]
fn show_window_with_command(window: &Window, command: i32) -> bool {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::ShowWindowAsync;

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return false;
    };

    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };

    unsafe { ShowWindowAsync(handle.hwnd.get() as HWND, command) != 0 }
}

#[cfg(target_os = "windows")]
fn restore_window(window: &Window) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_NORMAL;

    show_window_with_command(window, SW_NORMAL)
}

#[cfg(target_os = "windows")]
fn maximize_window(window: &Window) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_MAXIMIZE;

    show_window_with_command(window, SW_MAXIMIZE)
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
        theme_tokens: &crate::DesignTokens,
    ) -> Self {
        // Create ghost button style that works on the titlebar background
        let bg = titlebar_tokens.background;

        // Create hover background that's subtle on titlebar
        let hover_bg = theme_tokens.chrome.surface_hover;

        debug!(
            "TITLEBAR WINDOW_CONTROL: Creating default control style - bg={:?}, fg={:?}, hover_bg={:?}",
            bg, titlebar_tokens.foreground, hover_bg
        );

        let icon_color = theme_tokens.chrome.text_chrome_secondary;
        debug!(
            "TITLEBAR WINDOW_CONTROL: Computed icon colors - normal={:?}, hover={:?}",
            icon_color, theme_tokens.chrome.text_on_chrome
        );

        Self {
            background: ColorTheory::transparent(),
            background_hover: hover_bg,
            icon: icon_color,
            icon_hover: theme_tokens.chrome.text_on_chrome,
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
        let error_color = theme_tokens.editor.error;

        debug!(
            "TITLEBAR WINDOW_CONTROL: Creating close button style - bg={:?}, fg={:?}, error_color={:?}",
            bg, titlebar_tokens.foreground, error_color
        );

        let icon_color = theme_tokens.chrome.text_chrome_secondary;
        let icon_hover = ColorTheory::best_text_color(error_color, theme_tokens);

        debug!(
            "TITLEBAR WINDOW_CONTROL: Close button computed colors - icon={:?}, icon_hover={:?}",
            icon_color, icon_hover
        );

        Self {
            background: ColorTheory::transparent(),
            background_hover: error_color,
            icon: icon_color,
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
            background: ColorTheory::transparent(),
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
                        window.minimize_window();
                    }
                    WindowControlType::Restore => {
                        #[cfg(target_os = "windows")]
                        if restore_window(window) {
                            return;
                        }

                        window.zoom_window();
                    }
                    WindowControlType::Maximize => {
                        #[cfg(target_os = "windows")]
                        if maximize_window(window) {
                            return;
                        }

                        window.zoom_window();
                    }
                    WindowControlType::Close => {
                        cx.quit();
                    }
                }
            })
    }
}

#[derive(IntoElement)]
struct WindowsCaptionButton {
    id: gpui::ElementId,
    control_type: WindowControlType,
    titlebar_tokens: TitleBarTokens,
    theme_tokens: crate::DesignTokens,
}

impl WindowsCaptionButton {
    fn new(
        id: impl Into<gpui::ElementId>,
        control_type: WindowControlType,
        titlebar_tokens: TitleBarTokens,
        theme_tokens: crate::DesignTokens,
    ) -> Self {
        Self {
            id: id.into(),
            control_type,
            titlebar_tokens,
            theme_tokens,
        }
    }
}

impl RenderOnce for WindowsCaptionButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (hover_bg, hover_fg, active_bg, active_fg) = match self.control_type {
            WindowControlType::Close => {
                let close_bg = windows_close_hover_background();
                (
                    close_bg,
                    gpui::white(),
                    ColorTheory::with_alpha(close_bg, 0.8),
                    ColorTheory::with_alpha(gpui::white(), 0.8),
                )
            }
            WindowControlType::Minimize
            | WindowControlType::Restore
            | WindowControlType::Maximize => (
                self.theme_tokens.chrome.surface_hover,
                self.titlebar_tokens.foreground,
                self.theme_tokens.chrome.surface_active,
                self.titlebar_tokens.foreground,
            ),
        };

        gpui::div()
            .id(self.id)
            .occlude()
            .flex()
            .justify_center()
            .items_center()
            .w(gpui::px(WINDOWS_CAPTION_BUTTON_WIDTH))
            .h_full()
            .font_family("Segoe MDL2 Assets")
            .text_size(gpui::px(WINDOWS_CAPTION_ICON_SIZE))
            .text_color(self.titlebar_tokens.foreground)
            .hover(|button| button.bg(hover_bg).text_color(hover_fg))
            .active(|button| button.bg(active_bg).text_color(active_fg))
            .window_control_area(self.control_type.window_control_area())
            .child(self.control_type.windows_caption_icon())
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
            .top_0()
            .bottom_0()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        controls = if self.platform_style == PlatformStyle::Windows {
            controls
                .right(gpui::px(0.0))
                .content_stretch()
                .px(gpui::px(0.0))
                .gap(gpui::px(0.0))
        } else {
            controls.right_2().items_center().px_2().gap_1()
        };

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
                PlatformStyle::Windows => controls
                    .child(WindowsCaptionButton::new(
                        "minimize",
                        WindowControlType::Minimize,
                        titlebar_tokens,
                        theme_tokens,
                    ))
                    .child(WindowsCaptionButton::new(
                        "maximize-or-restore",
                        if window.is_maximized() {
                            WindowControlType::Restore
                        } else {
                            WindowControlType::Maximize
                        },
                        titlebar_tokens,
                        theme_tokens,
                    ))
                    .child(WindowsCaptionButton::new(
                        "close",
                        WindowControlType::Close,
                        titlebar_tokens,
                        theme_tokens,
                    )),
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
                    let theme_tokens = cx.global::<crate::Theme>().tokens;
                    controls
                        .child(WindowsCaptionButton::new(
                            "minimize",
                            WindowControlType::Minimize,
                            theme_tokens.titlebar_tokens(),
                            theme_tokens,
                        ))
                        .child(WindowsCaptionButton::new(
                            "maximize-or-restore",
                            if window.is_maximized() {
                                WindowControlType::Restore
                            } else {
                                WindowControlType::Maximize
                            },
                            theme_tokens.titlebar_tokens(),
                            theme_tokens,
                        ))
                        .child(WindowsCaptionButton::new(
                            "close",
                            WindowControlType::Close,
                            theme_tokens.titlebar_tokens(),
                            theme_tokens,
                        ))
                }
                PlatformStyle::Mac => {
                    // macOS uses native traffic lights, return empty container
                    controls
                }
            }
        }
    }
}
