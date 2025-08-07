// ABOUTME: Window control buttons implementation for custom titlebars on Linux and Windows
// ABOUTME: Provides minimize, maximize/restore, and close buttons with platform-specific styling

use gpui::{
    svg, App, Hsla, InteractiveElement, IntoElement, 
    MouseButton, ParentElement, RenderOnce, 
    Styled, Window, WindowControlArea, hsla,
};

use crate::titlebar::platform_titlebar::PlatformStyle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowControlType {
    Minimize,
    Restore,
    Maximize,
    Close,
}

impl WindowControlType {
    pub fn icon_path(&self) -> &'static str {
        match self {
            WindowControlType::Minimize => "M 2 7 L 10 7",
            WindowControlType::Restore => "M 2 2 L 8 2 L 8 8 L 2 8 Z M 4 4 L 10 4 L 10 10 L 4 10",
            WindowControlType::Maximize => "M 2 2 L 10 2 L 10 10 L 2 10 Z",
            WindowControlType::Close => "M 2 2 L 10 10 M 10 2 L 2 10",
        }
    }
}

pub struct WindowControlStyle {
    pub background: Hsla,
    pub background_hover: Hsla,
    pub icon: Hsla,
    pub icon_hover: Hsla,
}

impl WindowControlStyle {
    pub fn default(cx: &App) -> Self {
        let ui_theme = cx.global::<crate::ui::Theme>();
        
        Self {
            background: hsla(0.0, 0.0, 0.0, 0.0), // transparent
            background_hover: ui_theme.surface_hover,
            icon: ui_theme.text_muted,
            icon_hover: ui_theme.text,
        }
    }
    
    pub fn close(cx: &App) -> Self {
        let ui_theme = cx.global::<crate::ui::Theme>();
        
        Self {
            background: hsla(0.0, 0.0, 0.0, 0.0), // transparent
            background_hover: hsla(0.0, 0.7, 0.5, 1.0), // red
            icon: ui_theme.text_muted,
            icon_hover: hsla(0.0, 0.0, 1.0, 1.0), // white
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
    pub fn new(
        id: impl Into<gpui::ElementId>,
        control_type: WindowControlType,
        cx: &App,
    ) -> Self {
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
}

impl RenderOnce for WindowControl {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let icon = svg()
            .size_4()
            .flex_none()
            .path(self.control_type.icon_path())
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
                        // window.minimize_window();
                    }
                    WindowControlType::Restore | WindowControlType::Maximize => {
                        window.toggle_fullscreen();
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
}

impl WindowControls {
    pub fn new(platform_style: PlatformStyle) -> Self {
        Self {
            platform_style,
            window_control_area: None,
        }
    }
    
    pub fn window_control_area(mut self, area: WindowControlArea) -> Self {
        self.window_control_area = Some(area);
        self
    }
}

impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut controls = gpui::div()
            .flex()
            .flex_row()
            .id("window-controls")
            .px_2()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            
        if let Some(area) = self.window_control_area {
            controls = controls.window_control_area(area);
        }
        
        // Add controls based on platform
        match self.platform_style {
            PlatformStyle::Linux => {
                controls
                    .child(WindowControl::new("minimize", WindowControlType::Minimize, cx))
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
            PlatformStyle::Windows => {
                // Windows order: minimize, maximize, close
                controls
                    .child(WindowControl::new("minimize", WindowControlType::Minimize, cx))
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