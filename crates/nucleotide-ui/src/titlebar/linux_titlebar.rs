// ABOUTME: Linux-specific titlebar implementation with enhanced desktop environment integration
// ABOUTME: Provides native-like titlebar experience across GNOME, KDE, and tiling window managers

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyView, Decorations, ElementId, InteractiveElement, MouseButton, ParentElement, Pixels,
    StatefulInteractiveElement, Styled, Window, WindowControlArea, div, px,
};

use crate::titlebar::linux_platform_detector::{
    CompositorCapability, DesktopEnvironment, LinuxPlatformInfo, WindowButtonLayout,
    get_platform_info,
};
use crate::titlebar::linux_window_controls::LinuxWindowControls;
use crate::tokens::TitleBarTokens;
use nucleotide_logging::debug;
#[cfg(debug_assertions)]
use nucleotide_logging::info;

#[derive(Debug, Clone)]
pub struct LinuxTitlebarStyle {
    pub background: gpui::Hsla,
    pub foreground: gpui::Hsla,
    pub border: gpui::Hsla,
    pub height: Pixels,
    pub title_alignment: TitleAlignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAlignment {
    Left,
    Center,
}

impl LinuxTitlebarStyle {
    /// Create GNOME-style titlebar (Adwaita inspired)
    pub fn gnome_style(tokens: TitleBarTokens, _platform_info: &LinuxPlatformInfo) -> Self {
        debug!("Creating GNOME-style titlebar");

        Self {
            background: tokens.background,
            foreground: tokens.foreground,
            border: tokens.border,
            height: tokens.height.max(px(40.0)), // GNOME prefers taller titlebars
            title_alignment: TitleAlignment::Center, // GNOME centers titles
        }
    }

    /// Create KDE-style titlebar (Breeze inspired)
    pub fn kde_style(tokens: TitleBarTokens, _platform_info: &LinuxPlatformInfo) -> Self {
        debug!("Creating KDE-style titlebar");

        Self {
            background: tokens.background,
            foreground: tokens.foreground,
            border: tokens.border,
            height: tokens.height.max(px(32.0)), // KDE uses standard height
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
            title_alignment: TitleAlignment::Left, // Simple left alignment
        }
    }
}

pub struct LinuxTitlebar {
    id: ElementId,
    title: String,
    platform_info: LinuxPlatformInfo,
    style: LinuxTitlebarStyle,
    title_text_size: Pixels,
    inset_applied: bool,
}

impl LinuxTitlebar {
    pub fn height_for_theme(theme: &crate::Theme) -> Pixels {
        let platform_info = get_platform_info();
        let tokens = theme.tokens.titlebar_tokens();

        match platform_info.desktop_environment {
            DesktopEnvironment::Gnome => tokens.height.max(px(40.0)),
            DesktopEnvironment::Kde => tokens.height.max(px(32.0)),
            _ => tokens.height.min(px(28.0)),
        }
    }

    pub fn new(id: impl Into<ElementId>, theme: &crate::Theme) -> Self {
        let platform_info = get_platform_info().clone();

        #[cfg(debug_assertions)]
        info!(
            "Creating Linux titlebar for DE: {:?}, WM: {:?}, Layout: {:?}",
            platform_info.desktop_environment,
            platform_info.window_manager,
            platform_info.button_layout
        );

        let tokens = theme.tokens.titlebar_tokens();

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
            title_text_size: theme.tokens.sizes.text_md,
            inset_applied: false,
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

/// A segment of the Linux titlebar row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitlebarSlot {
    WindowControls,
    ApplicationMenu,
    DragRegion,
    TrailingActions,
}

/// Left-to-right order of the titlebar row. The window controls sit on the
/// edge the desktop uses for them, and the draggable title region separates
/// the application menu from the trailing action lane.
fn slot_order(button_layout: WindowButtonLayout) -> [TitlebarSlot; 4] {
    use TitlebarSlot::*;
    match button_layout {
        WindowButtonLayout::Left => [WindowControls, ApplicationMenu, DragRegion, TrailingActions],
        WindowButtonLayout::Right | WindowButtonLayout::Custom => {
            [ApplicationMenu, DragRegion, TrailingActions, WindowControls]
        }
    }
}

impl LinuxTitlebar {
    /// Render the titlebar as a single row. `application_menu` and
    /// `trailing_view` are optional application-owned slots; `drag_pending`
    /// must outlive the frame so a press in the drag region can start a window
    /// move once the pointer moves.
    pub fn render_element(
        &mut self,
        window: &mut Window,
        application_menu: Option<AnyView>,
        trailing_view: Option<AnyView>,
        drag_pending: Rc<Cell<bool>>,
    ) -> gpui::Stateful<gpui::Div> {
        #[cfg(debug_assertions)]
        debug!(
            "Rendering Linux titlebar - DE: {:?}, decorations: {:?}, height: {:?}",
            self.platform_info.desktop_environment,
            window.window_decorations(),
            self.style.height
        );

        // Set window insets only once to avoid redundant per-frame calls
        if !self.inset_applied {
            window.set_client_inset(px(0.0));
            self.inset_applied = true;
        }

        let button_layout = self.platform_info.button_layout;

        // Do not apply rounded corners on Linux. Many compositors already
        // round the window, and an extra clip here causes corner artifacts.
        let mut titlebar = div()
            .id(self.id.clone())
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(self.style.height)
            .overflow_hidden()
            .bg(self.style.background)
            .border_b_1()
            .border_color(self.style.border);

        for slot in slot_order(button_layout) {
            titlebar = match slot {
                TitlebarSlot::WindowControls => {
                    // Always show window controls on Linux, even when
                    // maximized or fullscreen: some WMs use fullscreen as a
                    // stand-in for maximize, and hiding the controls makes it
                    // impossible to restore the window.
                    titlebar.child(LinuxWindowControls::new(TitleBarTokens {
                        background: self.style.background,
                        foreground: self.style.foreground,
                        border: self.style.border,
                        height: self.style.height,
                    }))
                }
                TitlebarSlot::ApplicationMenu => titlebar
                    .when_some(application_menu.clone(), |titlebar, menu| {
                        titlebar.child(div().flex_none().h_full().child(menu))
                    }),
                TitlebarSlot::DragRegion => titlebar.child(self.render_drag_region(&drag_pending)),
                TitlebarSlot::TrailingActions => {
                    titlebar.when_some(trailing_view.clone(), |titlebar, trailing_view| {
                        titlebar.child(
                            div()
                                .flex_none()
                                .w(px(super::TITLEBAR_ACTION_LANE_WIDTH))
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(trailing_view),
                        )
                    })
                }
            };
        }

        // With controls on the left, the trailing lane ends the row, so keep
        // the same inset from the window edge it has on other platforms.
        titlebar.when(
            button_layout == WindowButtonLayout::Left && trailing_view.is_some(),
            |titlebar| titlebar.pr(px(super::TITLEBAR_ACTION_RIGHT_INSET)),
        )
    }

    /// The flexible title area between the menu and the trailing actions.
    /// Dragging it moves the window and double-clicking it toggles maximize.
    fn render_drag_region(&self, drag_pending: &Rc<Cell<bool>>) -> gpui::Stateful<gpui::Div> {
        div()
            .id("linux-titlebar-drag-region")
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .h_full()
            .px_2()
            .items_center()
            .overflow_hidden()
            .map(|el| match self.style.title_alignment {
                TitleAlignment::Left => el.justify_start(),
                TitleAlignment::Center => el.justify_center(),
            })
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(self.title_text_size)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(self.style.foreground)
                    .child(self.title.clone()),
            )
            // Start the move on the first pointer motion rather than on press,
            // so the compositor's move grab does not swallow a double-click.
            .on_mouse_down(MouseButton::Left, {
                let drag_pending = drag_pending.clone();
                move |event, _window, cx| {
                    drag_pending.set(event.click_count == 1);
                    cx.stop_propagation();
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let drag_pending = drag_pending.clone();
                move |_, _, _| drag_pending.set(false)
            })
            .on_mouse_move({
                let drag_pending = drag_pending.clone();
                move |event, window, cx| {
                    if drag_pending.get() && event.pressed_button == Some(MouseButton::Left) {
                        drag_pending.set(false);
                        window.start_window_move();
                    }
                    cx.stop_propagation();
                }
            })
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.zoom_window();
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{TitlebarSlot, slot_order};
    use crate::titlebar::linux_platform_detector::WindowButtonLayout;

    #[test]
    fn window_controls_take_the_edge_named_by_the_button_layout() {
        use TitlebarSlot::*;
        assert_eq!(
            slot_order(WindowButtonLayout::Left),
            [WindowControls, ApplicationMenu, DragRegion, TrailingActions]
        );
        for layout in [WindowButtonLayout::Right, WindowButtonLayout::Custom] {
            assert_eq!(
                slot_order(layout),
                [ApplicationMenu, DragRegion, TrailingActions, WindowControls]
            );
        }
    }
}
