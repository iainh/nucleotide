// ABOUTME: Linux-specific titlebar implementation with enhanced desktop environment integration
// ABOUTME: Provides native-like titlebar experience across GNOME, KDE, and tiling window managers

use gpui::prelude::FluentBuilder;
use gpui::{
    Context, Decorations, ElementId, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Pixels, Render, Styled, Window, WindowControlArea, div, px,
};

use crate::titlebar::linux_platform_detector::{
    CompositorCapability, DesktopEnvironment, LinuxPlatformInfo, WindowButtonLayout,
    get_platform_info,
};
use crate::titlebar::linux_window_controls::LinuxWindowControls;
use crate::tokens::{ColorContext, TitleBarTokens};
use nucleotide_logging::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct LinuxTitlebarStyle {
    pub background: gpui::Hsla,
    pub foreground: gpui::Hsla,
    pub border: gpui::Hsla,
    pub height: Pixels,
    pub padding_left: f32,
    pub padding_right: f32,
    pub title_alignment: TitleAlignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAlignment {
    Left,
    Center,
    Right,
}

impl LinuxTitlebarStyle {
    /// Create GNOME-style titlebar (Adwaita inspired)
    pub fn gnome_style(tokens: TitleBarTokens, platform_info: &LinuxPlatformInfo) -> Self {
        debug!("Creating GNOME-style titlebar");

        Self {
            background: tokens.background,
            foreground: tokens.foreground,
            border: tokens.border,
            height: tokens.height.max(px(40.0)), // GNOME prefers taller titlebars
            padding_left: match platform_info.button_layout {
                WindowButtonLayout::Left => 120.0, // Space for controls on left
                _ => 12.0,
            },
            padding_right: match platform_info.button_layout {
                WindowButtonLayout::Left => 12.0,
                _ => 120.0, // Space for controls on right
            },
            title_alignment: TitleAlignment::Center, // GNOME centers titles
        }
    }

    /// Create KDE-style titlebar (Breeze inspired)
    pub fn kde_style(tokens: TitleBarTokens, platform_info: &LinuxPlatformInfo) -> Self {
        debug!("Creating KDE-style titlebar");

        Self {
            background: tokens.background,
            foreground: tokens.foreground,
            border: tokens.border,
            height: tokens.height.max(px(32.0)), // KDE uses standard height
            padding_left: 16.0,
            padding_right: match platform_info.button_layout {
                WindowButtonLayout::Right => 100.0, // Space for controls
                _ => 16.0,
            },
            title_alignment: TitleAlignment::Left, // KDE left-aligns titles
        }
    }

    /// Create minimal titlebar for tiling window managers
    pub fn minimal_style(tokens: TitleBarTokens, _platform_info: &LinuxPlatformInfo) -> Self {
        debug!("Creating minimal-style titlebar for tiling WM");

        Self {
            background: tokens.background,
            foreground: tokens.foreground,
            border: tokens.border,
            height: tokens.height.min(px(28.0)), // Minimal height
            padding_left: 8.0,
            padding_right: 60.0,                   // Just space for close button
            title_alignment: TitleAlignment::Left, // Simple left alignment
        }
    }
}

pub struct LinuxTitlebar {
    id: ElementId,
    title: String,
    platform_info: LinuxPlatformInfo,
    style: LinuxTitlebarStyle,
}

impl LinuxTitlebar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let platform_info = get_platform_info().clone();

        info!(
            "Creating Linux titlebar for DE: {:?}, WM: {:?}, Layout: {:?}",
            platform_info.desktop_environment,
            platform_info.window_manager,
            platform_info.button_layout
        );

        // Get titlebar tokens from theme provider
        let tokens = if let Some(theme_provider) =
            crate::providers::use_provider::<crate::providers::ThemeProvider>()
        {
            theme_provider.titlebar_tokens(ColorContext::OnSurface)
        } else {
            // Fallback to default tokens
            warn!("No theme provider available, using default titlebar tokens");
            TitleBarTokens {
                background: gpui::hsla(0.0, 0.0, 0.95, 1.0),
                foreground: gpui::hsla(0.0, 0.0, 0.2, 1.0),
                border: gpui::hsla(0.0, 0.0, 0.8, 1.0),
                height: px(34.0),
            }
        };

        // Choose style based on desktop environment
        let style = match platform_info.desktop_environment {
            DesktopEnvironment::Gnome => LinuxTitlebarStyle::gnome_style(tokens, &platform_info),
            DesktopEnvironment::Kde => LinuxTitlebarStyle::kde_style(tokens, &platform_info),
            _ => LinuxTitlebarStyle::minimal_style(tokens, &platform_info),
        };

        Self {
            id: id.into(),
            title: String::new(),
            platform_info,
            style,
        }
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn height(&self) -> Pixels {
        self.style.height
    }

    /// Check if we should create this titlebar based on compositor capabilities
    pub fn should_create_for_decorations(decorations: &Decorations) -> bool {
        let platform_info = get_platform_info();

        match decorations {
            Decorations::Server => {
                // Only create custom titlebar if compositor explicitly supports client decorations
                // or if it's a tiling WM that might not provide good server decorations
                matches!(
                    platform_info.compositor_capability,
                    CompositorCapability::ClientSideDecorations | CompositorCapability::Mixed
                ) || matches!(
                    platform_info.desktop_environment,
                    DesktopEnvironment::I3
                        | DesktopEnvironment::Sway
                        | DesktopEnvironment::Awesome
                        | DesktopEnvironment::Bspwm
                )
            }
            Decorations::Client { .. } => true, // Always create for client decorations
        }
    }
}

impl LinuxTitlebar {
    pub fn render_element(&mut self, window: &mut Window) -> gpui::Stateful<gpui::Div> {
        let decorations = window.window_decorations();

        debug!(
            "Rendering Linux titlebar - DE: {:?}, decorations: {:?}, height: {:?}",
            self.platform_info.desktop_environment, decorations, self.style.height
        );

        // Set window insets based on decoration type
        match decorations {
            Decorations::Client { .. } => window.set_client_inset(px(0.0)),
            Decorations::Server => window.set_client_inset(px(0.0)),
        }

        // Create the main titlebar container
        let mut titlebar = div()
            .flex()
            .flex_row()
            .id(self.id.clone())
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(self.style.height)
            .bg(self.style.background)
            .border_b_1()
            .border_color(self.style.border);

        // Do not apply rounded corners on Linux.
        // Many compositors already round the window and applying an additional
        // clip here causes visible corner artifacts. Leaving the corners square
        // avoids double-rounding and blends correctly with the window frame.
        titlebar = titlebar.map(|el| match decorations {
            // Keep behavior identical for both decoration modes: no extra rounding.
            Decorations::Server => el,
            Decorations::Client { .. } => el,
        });

        // Apply padding based on button layout
        titlebar = titlebar
            .pl(px(self.style.padding_left))
            .pr(px(self.style.padding_right));

        // Create title content area
        let title_content = div()
            .flex()
            .flex_row()
            .w_full()
            .h_full()
            .items_center()
            .relative()
            .map(|el| match self.style.title_alignment {
                TitleAlignment::Left => el.justify_start(),
                TitleAlignment::Center => el.justify_center(),
                TitleAlignment::Right => el.justify_end(),
            })
            .child(
                div().flex().items_center().gap_2().child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(self.style.foreground)
                        .child(self.title.clone()),
                ),
            )
            .on_mouse_down(MouseButton::Left, {
                let button_layout = self.platform_info.button_layout;
                move |event, window, cx| {
                    // Handle titlebar dragging, but avoid interfering with window controls
                    let bounds = window.window_bounds().get_bounds();
                    let control_area_width = 120.0; // Approximate width of control area

                    let should_drag = match button_layout {
                        WindowButtonLayout::Left => event.position.x.0 > control_area_width,
                        WindowButtonLayout::Right | WindowButtonLayout::Custom => {
                            event.position.x.0 < (bounds.size.width.0 - control_area_width)
                        }
                    };

                    if should_drag {
                        window.start_window_move();
                    }
                    cx.stop_propagation();
                }
            })
            .on_mouse_move(|_, _, cx| cx.stop_propagation());

        // Add the title content to the titlebar
        titlebar = titlebar.child(title_content);

        // Always show window controls on Linux, even when maximized/fullscreen.
        // We use fullscreen as a stand-in for maximize on some WMs; hiding the
        // controls makes it impossible to restore the window.
        let titlebar_tokens = TitleBarTokens {
            background: self.style.background,
            foreground: self.style.foreground,
            border: self.style.border,
            height: self.style.height,
        };

        titlebar = titlebar.child(LinuxWindowControls::new(titlebar_tokens));

        titlebar
    }
}
