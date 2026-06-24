// ABOUTME: Cross-platform in-window application menu for platforms without a global menubar
// ABOUTME: Bridges GPUI-owned app menus into Nucleotide's reusable popup menu surface

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, AnchoredPositionMode, Context, DismissEvent, ElementId, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, OwnedMenu,
    ParentElement, Pixels, Render, SharedString, Styled, Subscription, Window, anchored, deferred,
    div, point, px,
};

use crate::actions::menu::{Cancel, SelectLeft, SelectRight};
use crate::menu::{APP_MENU_BAR_CONTEXT, PopupMenu};
use crate::{Theme, tokens::ColorContext};

#[derive(Clone)]
struct MenuEntry {
    menu: OwnedMenu,
}

#[derive(Clone, Copy)]
struct MenuBarMetrics {
    gap: Pixels,
    leading_padding: Pixels,
    trigger_height: Pixels,
    trigger_padding_x: Pixels,
    trigger_radius: Pixels,
}

fn menu_bar_metrics(embedded_in_titlebar: bool) -> MenuBarMetrics {
    if embedded_in_titlebar {
        MenuBarMetrics {
            gap: px(0.0),
            leading_padding: px(4.0),
            trigger_height: px(22.0),
            trigger_padding_x: px(7.0),
            trigger_radius: px(2.0),
        }
    } else {
        MenuBarMetrics {
            gap: px(4.0),
            leading_padding: px(8.0),
            trigger_height: px(28.0),
            trigger_padding_x: px(12.0),
            trigger_radius: px(4.0),
        }
    }
}

pub struct ApplicationMenu {
    id: ElementId,
    entries: Vec<MenuEntry>,
    open_index: Option<usize>,
    popup_index: Option<usize>,
    popup_menu: Option<Entity<PopupMenu>>,
    action_context: Option<FocusHandle>,
    anchor_x: Option<f32>,
    row_height: Pixels,
    focus_handle: FocusHandle,
    embedded_in_titlebar: bool,
    _subscription: Option<Subscription>,
}

impl ApplicationMenu {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let menus = cx.get_menus().unwrap_or_default();

        let titlebar_height = if let Some(provider) =
            crate::providers::use_provider::<crate::providers::ThemeProvider>()
        {
            provider.titlebar_tokens(ColorContext::OnSurface).height
        } else {
            px(34.0)
        };

        Self {
            id: ElementId::from("application-menu"),
            entries: menus.into_iter().map(|menu| MenuEntry { menu }).collect(),
            open_index: None,
            popup_index: None,
            popup_menu: None,
            action_context: None,
            anchor_x: None,
            row_height: titlebar_height,
            focus_handle: cx.focus_handle(),
            embedded_in_titlebar: false,
            _subscription: None,
        }
    }

    #[cfg(target_os = "windows")]
    pub fn new_embedded_in_titlebar(cx: &mut Context<Self>) -> Self {
        let mut menu = Self::new(cx);
        menu.embedded_in_titlebar = true;
        menu
    }

    fn set_open_index(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_open = self.open_index.is_some();

        if !was_open && index.is_some() {
            self.action_context = window.focused(cx);
        }

        if self.open_index != index {
            self.popup_menu.take();
            self.popup_index = None;
            self._subscription.take();
        }

        self.open_index = index;

        if index.is_none() {
            if let Some(action_context) = self.action_context.as_ref() {
                action_context.focus(window, cx);
            }
            self.action_context = None;
            self.anchor_x = None;
        }

        cx.notify();
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected_index) = self.open_index else {
            return;
        };

        let new_index = if selected_index == 0 {
            self.entries.len().saturating_sub(1)
        } else {
            selected_index.saturating_sub(1)
        };
        self.set_open_index(Some(new_index), window, cx);
        cx.stop_propagation();
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected_index) = self.open_index else {
            return;
        };

        let new_index = if selected_index + 1 >= self.entries.len() {
            0
        } else {
            selected_index + 1
        };
        self.set_open_index(Some(new_index), window, cx);
        cx.stop_propagation();
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open_index(None, window, cx);
        cx.stop_propagation();
    }

    fn set_anchor_from_mouse(&mut self, event: &MouseMoveEvent) {
        self.anchor_x = Some(f32::from(event.position.x));
    }

    fn build_popup_menu(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PopupMenu> {
        if self.popup_index == Some(index)
            && let Some(popup_menu) = self.popup_menu.as_ref()
        {
            return popup_menu.clone();
        }

        self._subscription.take();
        let items = self.entries[index].menu.items.clone();
        let action_context = self.action_context.clone();
        let popup_menu = PopupMenu::build(window, cx, |menu, window, cx| {
            menu.min_w(px(220.0))
                .max_w(px(420.0))
                .with_menu_items(items, window, cx)
        });
        popup_menu.update(cx, |menu, cx| {
            menu.set_action_context(action_context, cx);
        });
        self._subscription = Some(cx.subscribe_in(&popup_menu, window, Self::handle_popup_dismiss));
        self.popup_index = Some(index);
        self.popup_menu = Some(popup_menu.clone());

        let focus_handle = popup_menu.read(cx).focus_handle(cx);
        if !focus_handle.contains_focused(window, cx) {
            focus_handle.focus(window, cx);
        }

        popup_menu
    }

    fn handle_popup_dismiss(
        &mut self,
        _: &Entity<PopupMenu>,
        _: &DismissEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_open_index(None, window, cx);
    }
}

impl Render for ApplicationMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let titlebar_tokens = if let Some(provider) =
            crate::providers::use_provider::<crate::providers::ThemeProvider>()
        {
            provider.titlebar_tokens(ColorContext::OnSurface)
        } else {
            theme.tokens.titlebar_tokens()
        };

        let row_h = self.row_height;
        let metrics = menu_bar_metrics(self.embedded_in_titlebar);
        let chrome = theme.tokens.chrome;

        let mut container = div()
            .id(self.id.clone())
            .key_context(APP_MENU_BAR_CONTEXT)
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::cancel))
            .relative()
            .h(row_h)
            .min_h(row_h)
            .flex()
            .flex_row()
            .items_center()
            .gap(metrics.gap)
            .pl(metrics.leading_padding)
            .bg(titlebar_tokens.background)
            .border_color(titlebar_tokens.border)
            .when(!self.embedded_in_titlebar, |container| {
                container.w_full().border_b_1()
            })
            .track_focus(&self.focus_handle);

        for (index, entry) in self.entries.iter().enumerate() {
            let name = entry.menu.name.clone();
            let id = SharedString::from(format!("menu-trigger-{}", name));
            let is_open = self.open_index == Some(index);

            container = container.child(
                div()
                    .id(id)
                    .h(metrics.trigger_height)
                    .px(metrics.trigger_padding_x)
                    .flex()
                    .items_center()
                    .rounded(metrics.trigger_radius)
                    .text_size(theme.tokens.sizes.text_sm)
                    .text_color(titlebar_tokens.foreground)
                    .cursor_pointer()
                    .when(is_open, |trigger| trigger.bg(chrome.surface_hover))
                    .hover(|trigger| trigger.bg(chrome.surface_hover))
                    .on_mouse_move(cx.listener(move |this, event, window, cx| {
                        if this.open_index.is_some() && this.open_index != Some(index) {
                            this.set_anchor_from_mouse(event);
                            this.set_open_index(Some(index), window, cx);
                        } else if this.open_index == Some(index) {
                            this.set_anchor_from_mouse(event);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.anchor_x = Some(f32::from(event.position.x));

                            let new_index = if this.open_index == Some(index) {
                                None
                            } else {
                                Some(index)
                            };
                            this.set_open_index(new_index, window, cx);
                        }),
                    )
                    .child(name),
            );
        }

        if let Some(index) = self.open_index {
            let left = px(self.anchor_x.unwrap_or(8.0));
            let popup_menu = self.build_popup_menu(index, window, cx);
            let popup = anchored()
                .position_mode(AnchoredPositionMode::Local)
                .snap_to_window_with_margin(px(8.0))
                .anchor(Anchor::TopLeft)
                .position(point(left, row_h))
                .child(div().occlude().child(popup_menu));

            container = container.child(deferred(popup).with_priority(500));
        }

        container
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::menu::Confirm;
    use gpui::AppContext as _;
    use gpui::TestAppContext;
    use gpui::{Menu, MenuItem};

    struct TestRoot {
        menu: Entity<ApplicationMenu>,
        first_focus: FocusHandle,
        second_focus: FocusHandle,
    }

    impl Render for TestRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(div().id("first").track_focus(&self.first_focus))
                .child(div().id("second").track_focus(&self.second_focus))
                .child(self.menu.clone())
        }
    }

    #[gpui::test]
    fn preserves_action_context_while_switching_menus(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::from_tokens(crate::DesignTokens::dark()));
        });

        let (root, cx) = cx.add_window_view(|window, cx| {
            let first_focus = cx.focus_handle();
            let second_focus = cx.focus_handle();
            first_focus.focus(window, cx);

            TestRoot {
                menu: cx.new(|cx| ApplicationMenu {
                    id: ElementId::from("application-menu-test"),
                    entries: vec![
                        MenuEntry {
                            menu: Menu::new("File")
                                .items([
                                    MenuItem::action("Open", Confirm),
                                    MenuItem::submenu(
                                        Menu::new("Recent")
                                            .items([MenuItem::action("Project", Confirm)]),
                                    ),
                                ])
                                .owned(),
                        },
                        MenuEntry {
                            menu: Menu::new("Edit")
                                .items([MenuItem::action("Copy", Confirm)])
                                .owned(),
                        },
                    ],
                    open_index: None,
                    popup_index: None,
                    popup_menu: None,
                    action_context: None,
                    anchor_x: None,
                    row_height: px(34.0),
                    focus_handle: cx.focus_handle(),
                    embedded_in_titlebar: false,
                    _subscription: None,
                }),
                first_focus,
                second_focus,
            }
        });

        let (menu, first_focus, second_focus) = root.read_with(cx, |root, _| {
            (
                root.menu.clone(),
                root.first_focus.clone(),
                root.second_focus.clone(),
            )
        });

        menu.update_in(cx, |menu, window, cx| {
            menu.set_open_index(Some(0), window, cx);
            assert_eq!(menu.action_context.as_ref(), Some(&first_focus));

            second_focus.focus(window, cx);
            menu.set_open_index(Some(1), window, cx);
            assert_eq!(menu.action_context.as_ref(), Some(&first_focus));

            menu.set_open_index(None, window, cx);
            assert!(menu.action_context.is_none());
            assert_eq!(window.focused(cx).as_ref(), Some(&first_focus));

            second_focus.focus(window, cx);
            menu.set_open_index(Some(0), window, cx);
            assert_eq!(menu.action_context.as_ref(), Some(&second_focus));
        });
    }
}
