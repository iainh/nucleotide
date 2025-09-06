// ABOUTME: Linux-specific window controls with desktop environment aware layouts and styling
// ABOUTME: Supports GNOME, KDE, and tiling window manager specific button arrangements and capabilities

use gpui::{
    App, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, div, hsla, svg,
};

use crate::styling::{ColorTheory, StyleSize, StyleState, StyleVariant, compute_component_style};
use crate::titlebar::linux_platform_detector::{
    DesktopEnvironment, LinuxPlatformInfo, WindowButtonLayout, WindowManager, get_platform_info,
};
use crate::tokens::TitleBarTokens;
use nucleotide_logging::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxControlType {
    Minimize,
    Restore,
    Maximize,
    Close,
    // Linux-specific controls
    Shade, // Roll up window (XFCE, some WMs)
    Pin,   // Keep on top (various WMs)
    Menu,  // Window menu (right-click alternative)
}

impl LinuxControlType {
    pub fn asset_icon_path(&self) -> &'static str {
        match self {
            LinuxControlType::Minimize => "icons/window-minimize.svg",
            LinuxControlType::Restore => "icons/window-restore.svg",
            LinuxControlType::Maximize => "icons/window-maximize.svg",
            LinuxControlType::Close => "icons/close.svg",
            LinuxControlType::Shade => "icons/chevron-up.svg",
            LinuxControlType::Pin => "icons/pin.svg",
            LinuxControlType::Menu => "icons/menu.svg",
        }
    }

    /// Get the accessibility label for screen readers
    pub fn accessibility_label(&self) -> &'static str {
        match self {
            LinuxControlType::Minimize => "Minimize window",
            LinuxControlType::Restore => "Restore window",
            LinuxControlType::Maximize => "Maximize window",
            LinuxControlType::Close => "Close window",
            LinuxControlType::Shade => "Shade window",
            LinuxControlType::Pin => "Keep window on top",
            LinuxControlType::Menu => "Window menu",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LinuxControlStyle {
    pub background: Hsla,
    pub background_hover: Hsla,
    pub background_active: Hsla,
    pub icon: Hsla,
    pub icon_hover: Hsla,
    pub border: Option<Hsla>,
    pub border_radius: f32,
}

impl LinuxControlStyle {
    /// Create GNOME-style controls (Adwaita theme inspired)
    pub fn gnome_style(
        titlebar_tokens: TitleBarTokens,
        _theme_tokens: &crate::DesignTokens,
    ) -> Self {
        let bg = titlebar_tokens.background;
        let fg = titlebar_tokens.foreground;

        // GNOME uses more prominent button styling
        let button_bg = ColorTheory::mix_oklch(bg, fg, 0.08); // Subtle background
        let hover_bg = ColorTheory::mix_oklch(bg, fg, 0.15);
        let active_bg = ColorTheory::mix_oklch(bg, fg, 0.25);

        debug!(
            "Creating GNOME-style controls - bg: {:?}, hover: {:?}, active: {:?}",
            button_bg, hover_bg, active_bg
        );

        Self {
            background: button_bg,
            background_hover: hover_bg,
            background_active: active_bg,
            icon: ColorTheory::mix_oklch(fg, bg, 0.2), // Softer icon color
            icon_hover: fg,
            border: Some(ColorTheory::mix_oklch(bg, fg, 0.12)),
            border_radius: 8.0, // GNOME's rounded corners
        }
    }

    /// Create KDE-style controls (Breeze theme inspired)  
    pub fn kde_style(titlebar_tokens: TitleBarTokens, _theme_tokens: &crate::DesignTokens) -> Self {
        let bg = titlebar_tokens.background;
        let fg = titlebar_tokens.foreground;

        // KDE uses flatter, more subtle styling
        let button_bg = hsla(0.0, 0.0, 0.0, 0.0); // Transparent by default
        let hover_bg = ColorTheory::mix_oklch(bg, fg, 0.1);
        let active_bg = ColorTheory::mix_oklch(bg, fg, 0.2);

        debug!(
            "Creating KDE-style controls - bg: {:?}, hover: {:?}, active: {:?}",
            button_bg, hover_bg, active_bg
        );

        Self {
            background: button_bg,
            background_hover: hover_bg,
            background_active: active_bg,
            icon: ColorTheory::mix_oklch(fg, bg, 0.25),
            icon_hover: fg,
            border: None,       // KDE typically doesn't use borders
            border_radius: 4.0, // Subtle rounded corners
        }
    }

    /// Create minimal style for tiling window managers
    pub fn minimal_style(
        titlebar_tokens: TitleBarTokens,
        _theme_tokens: &crate::DesignTokens,
    ) -> Self {
        let bg = titlebar_tokens.background;
        let fg = titlebar_tokens.foreground;

        // Minimal styling - just icon colors
        debug!("Creating minimal-style controls for tiling WM");

        Self {
            background: hsla(0.0, 0.0, 0.0, 0.0), // Fully transparent
            background_hover: ColorTheory::mix_oklch(bg, fg, 0.05), // Very subtle
            background_active: ColorTheory::mix_oklch(bg, fg, 0.1),
            icon: ColorTheory::mix_oklch(fg, bg, 0.4), // More subtle icons
            icon_hover: fg,
            border: None,
            border_radius: 2.0,
        }
    }

    /// Create close button style with danger coloring
    pub fn close_style(base_style: &Self, theme_tokens: &crate::DesignTokens) -> Self {
        let error_color = theme_tokens.colors.error;
        let error_text = ColorTheory::best_text_color(error_color, theme_tokens);

        debug!(
            "Creating close button style with danger colors - bg: {:?}, text: {:?}",
            error_color, error_text
        );

        Self {
            background: base_style.background,
            background_hover: error_color,
            background_active: ColorTheory::darken(error_color, 0.1),
            icon: base_style.icon,
            icon_hover: error_text,
            border: base_style.border,
            border_radius: base_style.border_radius,
        }
    }
}

#[derive(IntoElement)]
pub struct LinuxWindowControl {
    id: gpui::ElementId,
    control_type: LinuxControlType,
    style: LinuxControlStyle,
    is_enabled: bool,
}

impl LinuxWindowControl {
    pub fn new(
        id: impl Into<gpui::ElementId>,
        control_type: LinuxControlType,
        titlebar_tokens: TitleBarTokens,
        theme_tokens: &crate::DesignTokens,
        platform_info: &LinuxPlatformInfo,
    ) -> Self {
        // Choose style based on desktop environment
        let base_style = match platform_info.desktop_environment {
            DesktopEnvironment::Gnome => {
                LinuxControlStyle::gnome_style(titlebar_tokens, theme_tokens)
            }
            DesktopEnvironment::Kde => LinuxControlStyle::kde_style(titlebar_tokens, theme_tokens),
            _ => LinuxControlStyle::minimal_style(titlebar_tokens, theme_tokens),
        };

        // Apply close button styling if needed
        let style = match control_type {
            LinuxControlType::Close => LinuxControlStyle::close_style(&base_style, theme_tokens),
            _ => base_style,
        };

        // Check if control is enabled based on window manager capabilities
        let is_enabled = match control_type {
            LinuxControlType::Minimize => platform_info.supports_minimize,
            LinuxControlType::Maximize | LinuxControlType::Restore => {
                platform_info.supports_maximize
            }
            LinuxControlType::Close => true, // Always enabled
            LinuxControlType::Shade => {
                // Shade is supported by XFCE and some other WMs
                matches!(platform_info.desktop_environment, DesktopEnvironment::Xfce)
                    || matches!(
                        platform_info.window_manager,
                        WindowManager::Openbox | WindowManager::Fluxbox
                    )
            }
            LinuxControlType::Pin => {
                // Pin is supported by most traditional window managers
                !matches!(
                    platform_info.window_manager,
                    WindowManager::I3 | WindowManager::Sway | WindowManager::Bspwm
                )
            }
            LinuxControlType::Menu => true, // Menu is always available
        };

        debug!(
            "Created Linux window control {:?} - enabled: {}",
            control_type, is_enabled
        );

        Self {
            id: id.into(),
            control_type,
            style,
            is_enabled,
        }
    }
}

impl RenderOnce for LinuxWindowControl {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if !self.is_enabled {
            // Return invisible placeholder for disabled controls
            return div().id(self.id).w_6().h_6();
        }

        debug!(
            "Rendering Linux window control {:?} with style - bg: {:?}, hover: {:?}, icon: {:?}",
            self.control_type, self.style.background, self.style.background_hover, self.style.icon
        );

        let icon = svg()
            .size_4()
            .flex_none()
            .path(self.control_type.asset_icon_path())
            .text_color(self.style.icon)
            .group_hover("", |this| this.text_color(self.style.icon_hover));

        let mut button = div()
            .flex()
            .flex_row()
            .id(self.id)
            .group("")
            .cursor_pointer()
            .justify_center()
            .items_center()
            .w_6()
            .h_6()
            .bg(self.style.background)
            .hover(|this| this.bg(self.style.background_hover))
            .active(|this| this.bg(self.style.background_active))
            .child(icon);

        // Apply border if specified
        if let Some(border_color) = self.style.border {
            button = button.border_1().border_color(border_color);
        }

        // Apply border radius
        if self.style.border_radius > 0.0 {
            button = button.rounded(gpui::px(self.style.border_radius));
        }

        button
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();

                match self.control_type {
                    LinuxControlType::Minimize => {
                        debug!("Minimize button clicked -> minimizing window via GPUI");
                        window.minimize_window();
                    }
                    LinuxControlType::Restore | LinuxControlType::Maximize => {
                        // Use native zoom/maximize behavior provided by GPUI
                        debug!("Toggling window zoom (maximize/restore) via GPUI");
                        window.zoom_window();
                    }
                    LinuxControlType::Close => {
                        debug!("Close button clicked");
                        cx.quit();
                    }
                    LinuxControlType::Shade => {
                        debug!("Shade button clicked (custom implementation needed)");
                        // TODO: Implement window shading
                    }
                    LinuxControlType::Pin => {
                        debug!("Pin button clicked (custom implementation needed)");
                        // TODO: Implement always-on-top
                    }
                    LinuxControlType::Menu => {
                        debug!("Menu button clicked (custom implementation needed)");
                        // TODO: Show window context menu
                    }
                }
            })
    }
}

#[derive(IntoElement)]
pub struct LinuxWindowControls {
    titlebar_tokens: TitleBarTokens,
    platform_info: LinuxPlatformInfo,
}

impl LinuxWindowControls {
    pub fn new(titlebar_tokens: TitleBarTokens) -> Self {
        let platform_info = get_platform_info().clone();

        debug!(
            "Creating Linux window controls for DE: {:?}, WM: {:?}, Layout: {:?}",
            platform_info.desktop_environment,
            platform_info.window_manager,
            platform_info.button_layout
        );

        Self {
            titlebar_tokens,
            platform_info,
        }
    }

    /// Get the controls in the correct order based on desktop environment
    fn get_control_layout(&self, window: &Window) -> Vec<LinuxControlType> {
        let mut controls = Vec::new();

        // Determine maximize/restore button from native maximize state.
        let maximize_button = if window.is_maximized() {
            LinuxControlType::Restore
        } else {
            LinuxControlType::Maximize
        };

        match self.platform_info.button_layout {
            WindowButtonLayout::Left => {
                // GNOME style: close on the left, then minimize/maximize
                if self.platform_info.supports_minimize || self.platform_info.supports_maximize {
                    controls.push(LinuxControlType::Close);
                    if self.platform_info.supports_minimize {
                        controls.push(LinuxControlType::Minimize);
                    }
                    if self.platform_info.supports_maximize {
                        controls.push(maximize_button);
                    }
                } else {
                    // Just close for tiling WMs
                    controls.push(LinuxControlType::Close);
                }
            }
            WindowButtonLayout::Right | WindowButtonLayout::Custom => {
                // Traditional style: minimize, maximize, close
                if self.platform_info.supports_minimize {
                    controls.push(LinuxControlType::Minimize);
                }
                if self.platform_info.supports_maximize {
                    controls.push(maximize_button);
                }
                controls.push(LinuxControlType::Close);

                // Add extra controls for certain environments
                if matches!(
                    self.platform_info.desktop_environment,
                    DesktopEnvironment::Xfce
                ) {
                    // XFCE can have shade button
                    controls.insert(0, LinuxControlType::Shade);
                }
            }
        }

        debug!("Control layout: {:?}", controls);
        controls
    }
}

impl RenderOnce for LinuxWindowControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme_tokens = if let Some(theme_provider) =
            crate::providers::use_provider::<crate::providers::ThemeProvider>()
        {
            theme_provider.current_theme.tokens
        } else {
            cx.global::<crate::Theme>().tokens
        };

        let controls = self.get_control_layout(window);
        let mut container = div()
            .flex()
            .flex_row()
            .id("linux-window-controls")
            .items_center()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        // Position controls based on layout
        match self.platform_info.button_layout {
            WindowButtonLayout::Left => {
                container = container.absolute().left_2().top_0().bottom_0();
            }
            WindowButtonLayout::Right | WindowButtonLayout::Custom => {
                container = container.absolute().right_2().top_0().bottom_0();
            }
        }

        // Add all controls
        for control_type in controls.iter() {
            let id = match control_type {
                LinuxControlType::Minimize => "linux-minimize",
                LinuxControlType::Maximize => "linux-maximize",
                LinuxControlType::Restore => "linux-restore",
                LinuxControlType::Close => "linux-close",
                LinuxControlType::Shade => "linux-shade",
                LinuxControlType::Pin => "linux-pin",
                LinuxControlType::Menu => "linux-menu",
            };

            container = container.child(LinuxWindowControl::new(
                id,
                *control_type,
                self.titlebar_tokens,
                &theme_tokens,
                &self.platform_info,
            ));
        }

        container
    }
}
