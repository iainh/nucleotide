// ABOUTME: Cross-platform in-window application menu for platforms without a global menubar
// ABOUTME: Inspired by Zed's ApplicationMenu, simplified for Nucleotide's UI stack

use gpui::{
    Context, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, Render,
    SharedString, Styled, Window, actions, div,
};

use gpui::{OwnedMenu, OwnedMenuItem};

use crate::{Button, ButtonSize, ButtonVariant, Theme, tokens::ColorContext};

actions!(
    app_menu,
    [
        /// Close the open application menu
        CloseMenus,
    ]
);

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

    fn render_dropdown_for(&self, idx: usize) -> impl IntoElement {
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
                        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                            // Dispatch the associated action and close menus
                            window.dispatch_action(action.boxed_clone(), cx);
                            cx.dispatch_action(&CloseMenus);
                        })
                        .child(label),
                )
            }
            OwnedMenuItem::Submenu(_) | OwnedMenuItem::SystemMenu(_) => panel,
        })
    }
}

impl Render for ApplicationMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .border_color(titlebar_tokens.border);

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
                            this.anchor_x = Some(ev.position.x.0);
                            cx.notify();
                        }),
                    ),
            );
        }

        // Close menus when requested
        container = container.on_action(cx.listener(|this, _: &CloseMenus, _w, cx| {
            this.open_index = None;
            this.anchor_x = None;
            cx.notify();
        }));

        // Dropdown panel
        if let Some(idx) = self.open_index {
            let left = self.anchor_x.unwrap_or(12.0);
            container = container.child(
                div()
                    .absolute()
                    .left(gpui::px(left))
                    .top(row_h)
                    .child(self.render_dropdown_for(idx)),
            );

            // Lightweight click-away within this row area: clicking the row background closes it
            container = container.on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.dispatch_action(&CloseMenus);
            });
        }

        container
    }
}
