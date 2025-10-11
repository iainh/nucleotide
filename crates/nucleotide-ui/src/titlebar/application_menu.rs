// ABOUTME: Cross-platform in-window application menu for platforms without a global menubar
// ABOUTME: Inspired by Zed's ApplicationMenu, simplified for Nucleotide's UI stack

use gpui::{
    AnchoredPositionMode, Context, Corner, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Pixels, Render, SharedString, Styled, Window,
    anchored, deferred, div, point, px,
};

use gpui::{OwnedMenu, OwnedMenuItem};

use crate::{Button, ButtonSize, ButtonVariant, Theme, tokens::ColorContext};

#[derive(Clone)]
struct MenuEntry {
    menu: OwnedMenu,
}

pub struct ApplicationMenu {
    id: ElementId,
    entries: Vec<MenuEntry>,
    open_index: Option<usize>,
    // Anchor x position for the dropdown (relative to this view)
    anchor_x: Option<f32>,
    row_height: Pixels,
    // Focus handle to capture Esc while menu is open
    focus_handle: FocusHandle,
}

impl ApplicationMenu {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let menus = cx.get_menus().unwrap_or_default();

        // Use titlebar height for the menu row for visual consistency
        let titlebar_height = if let Some(provider) =
            crate::providers::use_provider::<crate::providers::ThemeProvider>()
        {
            provider.titlebar_tokens(ColorContext::OnSurface).height
        } else {
            // Fallback height
            gpui::px(34.0)
        };

        Self {
            id: ElementId::from("application-menu"),
            entries: menus.into_iter().map(|menu| MenuEntry { menu }).collect(),
            open_index: None,
            anchor_x: None,
            row_height: titlebar_height,
            focus_handle: cx.focus_handle(),
        }
    }

    fn sanitize(items: Vec<OwnedMenuItem>) -> Vec<OwnedMenuItem> {
        let mut cleaned = Vec::new();
        let mut last_sep = false;
        for item in items {
            match item {
                OwnedMenuItem::Separator => {
                    if !last_sep {
                        cleaned.push(OwnedMenuItem::Separator);
                        last_sep = true;
                    }
                }
                OwnedMenuItem::Submenu(sub) => {
                    // Skip empty submenus
                    if !sub.items.is_empty() {
                        cleaned.push(OwnedMenuItem::Submenu(sub));
                        last_sep = false;
                    }
                }
                OwnedMenuItem::SystemMenu(_) => {
                    // System menus don't make sense in custom menu
                }
                action @ OwnedMenuItem::Action { .. } => {
                    cleaned.push(action);
                    last_sep = false;
                }
            }
        }
        // Drop trailing separator
        if let Some(OwnedMenuItem::Separator) = cleaned.last() {
            cleaned.pop();
        }
        cleaned
    }

    fn render_dropdown_for(&self, idx: usize, cx: &mut Context<Self>) -> impl IntoElement {
        // Read theme colors for menu surfaces
        let theme = crate::ProviderHooks::theme();
        let chrome = &theme.tokens.chrome;

        let mut items: Vec<OwnedMenuItem> = Vec::new();
        for item in &self.entries[idx].menu.items {
            match item {
                OwnedMenuItem::Submenu(sub) => {
                    // Flatten first-level submenus (simple, pragmatic)
                    if !items.is_empty() {
                        items.push(OwnedMenuItem::Separator);
                    }
                    items.extend(sub.items.clone());
                }
                other => items.push(other.clone()),
            }
        }
        let items = Self::sanitize(items);

        // Menu panel container
        let panel = div()
            .id(SharedString::from("menu-dropdown"))
            .bg(chrome.menu_background)
            .border_1()
            .border_color(chrome.menu_separator)
            .rounded_md()
            .shadow_md()
            .min_w(gpui::px(220.0))
            .max_w(gpui::px(420.0));

        // Render items
        items.into_iter().fold(panel, |panel, item| match item {
            OwnedMenuItem::Separator => {
                panel.child(div().h_0p5().my_1().bg(chrome.menu_separator.alpha(0.7)))
            }
            OwnedMenuItem::Action { name, action, .. } => {
                // Each action item is a row with hover highlight
                let label: SharedString = name.into();
                panel.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(theme.tokens.sizes.text_sm)
                        .text_color(chrome.text_chrome_secondary)
                        .hover(|el| el.bg(chrome.menu_selected))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, window, cx| {
                                // Dispatch the associated action and close menus locally
                                window.dispatch_action(action.boxed_clone(), cx);
                                this.open_index = None;
                                this.anchor_x = None;
                                cx.notify();
                            }),
                        )
                        .child(label),
                )
            }
            OwnedMenuItem::Submenu(_) | OwnedMenuItem::SystemMenu(_) => panel,
        })
    }
}

impl Render for ApplicationMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Pull theme for row colors
        let theme = cx.global::<Theme>();
        let titlebar_tokens = if let Some(provider) =
            crate::providers::use_provider::<crate::providers::ThemeProvider>()
        {
            provider.titlebar_tokens(ColorContext::OnSurface)
        } else {
            theme.tokens.titlebar_tokens()
        };

        let row_h = self.row_height;

        let mut container = div()
            .id(self.id.clone())
            .relative()
            .w_full()
            .h(row_h)
            .min_h(row_h)
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .pl_2()
            .bg(titlebar_tokens.background)
            .border_b_1()
            .border_color(titlebar_tokens.border)
            .track_focus(&self.focus_handle)
            // Handle Esc to close the menu while focused
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                if ev.keystroke.key == "escape" {
                    this.open_index = None;
                    this.anchor_x = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }));

        // Menu triggers
        for (i, entry) in self.entries.iter().enumerate() {
            let name = entry.menu.name.clone();
            container = container.child(
                Button::new(SharedString::from(format!("menu-trigger-{}", name)), name)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Small)
                    .on_click(
                        cx.listener(move |this, ev: &gpui::MouseUpEvent, _window, cx| {
                            this.open_index = Some(i);
                            this.anchor_x = Some(f32::from(ev.position.x));
                            cx.notify();
                        }),
                    ),
            );
        }

        // Close menus handled locally by listeners

        // Dropdown panel (rendered as a deferred anchored element to ensure top-most layering)
        if let Some(idx) = self.open_index {
            // Ensure we capture keyboard focus for Escape handling
            window.focus(&self.focus_handle);

            // Full-window click-away blocker below the dropdown to prevent underlying interactions
            let viewport = window.viewport_size();
            let click_away = anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(px(0.0), px(0.0)))
                .child(
                    div()
                        .w(viewport.width)
                        .h(viewport.height)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.open_index = None;
                                this.anchor_x = None;
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        ),
                );
            container = container.child(deferred(click_away).with_priority(450));

            let left = self.anchor_x.unwrap_or(12.0);
            let popup = anchored()
                .position_mode(AnchoredPositionMode::Local)
                .snap_to_window_with_margin(px(8.0))
                .anchor(Corner::TopLeft)
                .position(point(px(left), row_h))
                .child(div().occlude().child(self.render_dropdown_for(idx, cx)));

            container = container.child(deferred(popup).with_priority(500));
        }

        container
    }
}
