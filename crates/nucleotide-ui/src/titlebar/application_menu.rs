// ABOUTME: Cross-platform in-window application menu for platforms without a global menubar
// ABOUTME: Bridges GPUI-owned app menus into Nucleotide's reusable popup menu surface

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, Context, DismissEvent, ElementId, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, OwnedMenu, ParentElement, Pixels, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, anchored, deferred,
    div, point, px,
};

use crate::actions::menu::{Cancel, SelectLeft, SelectRight};
use crate::menu::{APP_MENU_BAR_CONTEXT, PopupMenu};
use crate::{Theme, tokens::ColorContext};

#[cfg(target_os = "windows")]
const WINDOWS_UI_FONT_FAMILY: &str = "Segoe UI Variable";

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
    popup_gap: Pixels,
    window_margin: Pixels,
}

fn menu_bar_metrics(embedded_in_titlebar: bool) -> MenuBarMetrics {
    if embedded_in_titlebar {
        MenuBarMetrics {
            gap: px(0.0),
            leading_padding: px(4.0),
            trigger_height: px(22.0),
            trigger_padding_x: px(7.0),
            trigger_radius: px(2.0),
            popup_gap: px(2.0),
            window_margin: px(8.0),
        }
    } else {
        MenuBarMetrics {
            gap: px(4.0),
            leading_padding: px(8.0),
            trigger_height: px(28.0),
            trigger_padding_x: px(12.0),
            trigger_radius: px(4.0),
            popup_gap: px(2.0),
            window_margin: px(8.0),
        }
    }
}

fn menu_popup_offset_y(row_height: Pixels, trigger_height: Pixels, popup_gap: Pixels) -> Pixels {
    let bottom_inset = ((f32::from(row_height) - f32::from(trigger_height)).max(0.0)) / 2.0;
    trigger_height + px(bottom_inset) + popup_gap
}

pub struct ApplicationMenu {
    id: ElementId,
    entries: Vec<MenuEntry>,
    open_index: Option<usize>,
    popup_index: Option<usize>,
    popup_menu: Option<Entity<PopupMenu>>,
    action_context: Option<FocusHandle>,
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
            menu.shortcut_min_w(px(220.0))
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
        let tokens = cx.global::<Theme>().tokens;
        let titlebar_tokens = if let Some(provider) =
            crate::providers::use_provider::<crate::providers::ThemeProvider>()
        {
            provider.titlebar_tokens(ColorContext::OnSurface)
        } else {
            tokens.titlebar_tokens()
        };

        let row_h = self.row_height;
        let metrics = menu_bar_metrics(self.embedded_in_titlebar);
        let chrome = tokens.chrome;

        let mut container = div()
            .id(self.id.clone())
            .role(gpui::accesskit::Role::MenuBar)
            .aria_label("Application menu")
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

        for index in 0..self.entries.len() {
            let name = self.entries[index].menu.name.clone();
            let id = SharedString::from(format!("menu-trigger-{}", name));
            let is_open = self.open_index == Some(index);

            let mut trigger = div()
                .id(id)
                .relative()
                .h(metrics.trigger_height)
                .px(metrics.trigger_padding_x)
                .flex()
                .items_center()
                .rounded(metrics.trigger_radius)
                .border_1()
                .border_color(crate::tokens::utils::with_alpha(chrome.border_focus, 0.0))
                .role(gpui::accesskit::Role::MenuItem)
                .aria_label(name.clone())
                .aria_expanded(is_open)
                .text_size(tokens.sizes.text_sm)
                .text_color(titlebar_tokens.foreground)
                .cursor_pointer()
                .when(self.embedded_in_titlebar, |trigger| {
                    #[cfg(target_os = "windows")]
                    {
                        trigger.font_family(WINDOWS_UI_FONT_FAMILY)
                    }

                    #[cfg(not(target_os = "windows"))]
                    {
                        trigger
                    }
                })
                .when(is_open, |trigger| trigger.bg(chrome.surface_hover))
                .hover(|trigger| trigger.bg(chrome.surface_hover))
                .focus_visible(|style| style.border_color(chrome.border_focus))
                .on_mouse_move(cx.listener(move |this, _, window, cx| {
                    if this.open_index.is_some() && this.open_index != Some(index) {
                        this.set_open_index(Some(index), window, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();

                        let new_index = if this.open_index == Some(index) {
                            None
                        } else {
                            Some(index)
                        };
                        this.set_open_index(new_index, window, cx);
                    }),
                );

            if is_open {
                let popup_menu = self.build_popup_menu(index, window, cx);
                let popup = anchored()
                    .anchor(Anchor::TopLeft)
                    .offset(point(
                        px(0.0),
                        menu_popup_offset_y(row_h, metrics.trigger_height, metrics.popup_gap),
                    ))
                    .snap_to_window_with_margin(metrics.window_margin)
                    .child(div().occlude().child(popup_menu));

                trigger = trigger.child(deferred(popup).with_priority(500));
            }

            trigger = trigger.child(name);
            container = container.child(trigger);
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

    #[test]
    fn popup_offset_places_flyout_below_titlebar_row() {
        assert_eq!(menu_popup_offset_y(px(34.0), px(22.0), px(2.0)), px(30.0));
        assert_eq!(menu_popup_offset_y(px(34.0), px(28.0), px(2.0)), px(33.0));
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
