// ABOUTME: Cross-platform in-window application menu for platforms without a global menubar
// ABOUTME: Bridges GPUI-owned app menus into Nucleotide's reusable popup menu surface

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, Context, DismissEvent, ElementId, Entity, FocusHandle, Focusable, HighlightStyle,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, OwnedMenu,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, StyledText,
    Subscription, UnderlineStyle, Window, anchored, deferred, div, px,
};

use crate::Theme;
use crate::actions::menu::{
    ActivateMenuBar, AltTapMenuBar, Cancel, Confirm, SelectDown, SelectLeft, SelectRight,
};
use crate::menu::{APP_MENU_BAR_CONTEXT, PopupMenu};

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
            gap: px(0.0),
            leading_padding: px(8.0),
            trigger_height: px(24.0),
            trigger_padding_x: px(8.0),
            trigger_radius: px(2.0),
            popup_gap: px(2.0),
            window_margin: px(8.0),
        }
    }
}

fn popup_top_offset(
    embedded_in_titlebar: bool,
    row_height: Pixels,
    metrics: MenuBarMetrics,
) -> Pixels {
    if embedded_in_titlebar {
        px((f32::from(row_height) + f32::from(metrics.trigger_height)) / 2.0)
    } else {
        // Deferred anchored children start above the standalone menu row.
        row_height + metrics.popup_gap + px(20.0)
    }
}

/// Byte offset of each menu's access key: the first letter of its name not
/// already taken by an earlier menu, falling back to later letters.
fn mnemonic_offsets<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<Option<usize>> {
    let mut used = Vec::new();
    names
        .into_iter()
        .map(|name| {
            let (offset, ch) = name.char_indices().find(|(_, ch)| {
                ch.is_alphanumeric() && !used.contains(&ch.to_lowercase().to_string())
            })?;
            used.push(ch.to_lowercase().to_string());
            Some(offset)
        })
        .collect()
}

pub struct ApplicationMenu {
    id: ElementId,
    entries: Vec<MenuEntry>,
    open_index: Option<usize>,
    /// Menu highlighted after Alt or F10 moved keyboard focus to the bar.
    keyboard_index: Option<usize>,
    /// Access keys are underlined only while the bar is driven from the keyboard.
    show_mnemonics: bool,
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

        let titlebar_height = cx.global::<Theme>().tokens.titlebar_tokens().height;

        Self {
            id: ElementId::from("application-menu"),
            entries: menus.into_iter().map(|menu| MenuEntry { menu }).collect(),
            open_index: None,
            keyboard_index: None,
            show_mnemonics: false,
            popup_index: None,
            popup_menu: None,
            action_context: None,
            row_height: titlebar_height,
            focus_handle: cx.focus_handle(),
            embedded_in_titlebar: false,
            _subscription: None,
        }
    }

    pub fn new_embedded_in_titlebar(cx: &mut Context<Self>) -> Self {
        let mut menu = Self::new(cx);
        menu.embedded_in_titlebar = true;
        menu
    }

    pub fn set_row_height(&mut self, row_height: Pixels) {
        self.row_height = row_height;
    }

    /// Alt or F10: focus the menu bar with its first menu highlighted, or
    /// leave the menu bar and return focus if it is already active.
    pub fn toggle_keyboard_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_index.is_some() || self.keyboard_index.is_some() {
            self.set_open_index(None, window, cx);
            return;
        }
        if self.entries.is_empty() {
            return;
        }

        self.action_context = window.focused(cx);
        self.keyboard_index = Some(0);
        self.show_mnemonics = true;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn activate_menu_bar(
        &mut self,
        _: &ActivateMenuBar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_keyboard_activation(window, cx);
        cx.stop_propagation();
    }

    fn alt_tap_menu_bar(&mut self, _: &AltTapMenuBar, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_keyboard_activation(window, cx);
        cx.stop_propagation();
    }

    fn mnemonics(&self) -> Vec<Option<usize>> {
        mnemonic_offsets(self.entries.iter().map(|entry| entry.menu.name.as_ref()))
    }

    fn menu_for_mnemonic(&self, key: &str) -> Option<usize> {
        let mut key_chars = key.chars();
        let key = key_chars.next().filter(|_| key_chars.next().is_none())?;
        self.mnemonics()
            .into_iter()
            .zip(&self.entries)
            .position(|(offset, entry)| {
                offset
                    .and_then(|offset| entry.menu.name[offset..].chars().next())
                    .is_some_and(|ch| ch.to_lowercase().eq(key.to_lowercase()))
            })
    }

    fn is_keyboard_driven_bar(&self, window: &Window) -> bool {
        self.open_index.is_none()
            && self.keyboard_index.is_some()
            && self.focus_handle.is_focused(window)
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_keyboard_driven_bar(window) {
            return;
        }
        // The focused bar owns plain keys; none reach the editor behind it.
        cx.stop_propagation();

        let modifiers = &event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        if let Some(index) = self.menu_for_mnemonic(&event.keystroke.key) {
            self.set_open_index(Some(index), window, cx);
        }
    }

    fn open_keyboard_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.keyboard_index {
            Some(index) if self.is_keyboard_driven_bar(window) => {
                self.set_open_index(Some(index), window, cx);
                cx.stop_propagation();
            }
            _ => cx.propagate(),
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        self.open_keyboard_menu(window, cx);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        self.open_keyboard_menu(window, cx);
    }

    fn set_open_index(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_open = self.open_index.is_some();

        // Keyboard activation already captured the focus behind the menu bar.
        if !was_open && index.is_some() && self.action_context.is_none() {
            self.action_context = window.focused(cx);
        }

        if self.open_index != index {
            self.popup_menu.take();
            self.popup_index = None;
            self._subscription.take();
        }

        self.open_index = index;
        if index.is_some() && self.show_mnemonics {
            self.keyboard_index = index;
        }

        if index.is_none() {
            if let Some(action_context) = self.action_context.as_ref() {
                action_context.focus(window, cx);
            }
            self.action_context = None;
            self.keyboard_index = None;
            self.show_mnemonics = false;
        }

        cx.notify();
    }

    /// Move the open menu, or the keyboard highlight when none is open.
    fn move_selection(
        &mut self,
        new_index: impl Fn(usize) -> usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(selected_index) = self.open_index {
            self.set_open_index(Some(new_index(selected_index)), window, cx);
        } else if let Some(selected_index) = self.keyboard_index {
            self.keyboard_index = Some(new_index(selected_index));
            cx.notify();
        } else {
            return;
        }
        cx.stop_propagation();
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.entries.len();
        self.move_selection(
            |index| {
                if index == 0 {
                    len.saturating_sub(1)
                } else {
                    index - 1
                }
            },
            window,
            cx,
        );
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.entries.len();
        self.move_selection(
            |index| if index + 1 >= len { 0 } else { index + 1 },
            window,
            cx,
        );
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
            menu.shortcut_min_w(px(280.0))
                .max_w(px(420.0))
                .with_menu_items(items, window, cx)
        });
        let select_first = self.show_mnemonics;
        popup_menu.update(cx, |menu, cx| {
            menu.set_action_context(action_context, window, cx);
            if select_first {
                menu.select_first_item(cx);
            }
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
        let titlebar_tokens = if window.is_window_active() {
            tokens.titlebar_tokens()
        } else {
            tokens.titlebar_tokens().inactive_window(&tokens.chrome)
        };

        let row_h = self.row_height;
        let metrics = menu_bar_metrics(self.embedded_in_titlebar);
        let popup_top = popup_top_offset(self.embedded_in_titlebar, row_h, metrics);
        let chrome = tokens.chrome;

        // Clicking elsewhere leaves keyboard mode without restoring focus.
        if self.open_index.is_none()
            && self.keyboard_index.is_some()
            && !self.focus_handle.contains_focused(window, cx)
        {
            self.keyboard_index = None;
            self.show_mnemonics = false;
            self.action_context = None;
        }
        let mnemonics = self.mnemonics();

        let mut container = div()
            .id(self.id.clone())
            .role(gpui::accesskit::Role::MenuBar)
            .aria_label("Application menu")
            .key_context(APP_MENU_BAR_CONTEXT)
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::activate_menu_bar))
            .on_action(cx.listener(Self::alt_tap_menu_bar))
            .on_key_down(cx.listener(Self::handle_key_down))
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

        for (index, mnemonic) in mnemonics.into_iter().enumerate() {
            let name = self.entries[index].menu.name.clone();
            let id = SharedString::from(format!("menu-trigger-{}", name));
            let is_open = self.open_index == Some(index);
            let is_highlighted =
                is_open || (self.open_index.is_none() && self.keyboard_index == Some(index));
            let trigger_debug_name = name.clone();

            let mut trigger = div()
                .id(id)
                .debug_selector(move || format!("application-menu-trigger-{trigger_debug_name}"))
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
                .when(is_highlighted, |trigger| {
                    trigger
                        .bg(chrome.surface_hover)
                        .when_some(chrome.contrast, |trigger, c| {
                            trigger.text_color(c.highlight_text)
                        })
                })
                .hover(|trigger| {
                    let hovered = trigger.bg(chrome.surface_hover);
                    if let Some(c) = chrome.contrast {
                        hovered.text_color(c.highlight_text)
                    } else {
                        hovered
                    }
                })
                .when(chrome.is_windows_system(), |trigger| {
                    let focus = self.focus_handle.clone();
                    let outlined = index == self.keyboard_index.unwrap_or(0);
                    trigger.child(
                        chrome.keyboard_focus_outline(metrics.trigger_radius, move |window| {
                            is_open || (outlined && focus.is_focused(window))
                        }),
                    )
                })
                .when(!chrome.is_windows_system(), |trigger| {
                    trigger.focus_visible(|style| style.border_color(chrome.border_focus))
                })
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
                let popup_debug_name = name.clone();
                let popup = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(metrics.window_margin)
                    .child(
                        div()
                            .debug_selector(move || {
                                format!("application-menu-popup-{popup_debug_name}")
                            })
                            .left(-metrics.trigger_padding_x - px(1.0))
                            .top(popup_top)
                            .occlude()
                            .child(popup_menu),
                    );

                trigger = trigger.child(deferred(popup).with_priority(500));
            }

            let mnemonic = mnemonic.filter(|_| self.show_mnemonics).and_then(|offset| {
                let len = name[offset..].chars().next()?.len_utf8();
                Some(offset..offset + len)
            });
            trigger = match mnemonic {
                Some(range) => trigger.child(StyledText::new(name).with_highlights([(
                    range,
                    HighlightStyle {
                        underline: Some(UnderlineStyle {
                            thickness: px(1.0),
                            color: None,
                            wavy: false,
                        }),
                        ..HighlightStyle::default()
                    },
                )])),
                None => trigger.child(name),
            };
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
    fn standalone_menu_bar_uses_compact_linux_spacing() {
        let metrics = menu_bar_metrics(false);

        assert_eq!(metrics.gap, px(0.0));
        assert_eq!(metrics.leading_padding, px(8.0));
        assert_eq!(metrics.trigger_height, px(24.0));
        assert_eq!(metrics.trigger_padding_x, px(8.0));
    }

    #[test]
    fn embedded_menu_bar_uses_titlebar_spacing() {
        let metrics = menu_bar_metrics(true);

        assert_eq!(metrics.gap, px(0.0));
        assert_eq!(metrics.leading_padding, px(4.0));
        assert_eq!(metrics.trigger_height, px(22.0));
        assert_eq!(metrics.trigger_padding_x, px(7.0));
        assert_eq!(popup_top_offset(true, px(34.0), metrics), px(28.0));
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
                    keyboard_index: None,
                    show_mnemonics: false,
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

    #[gpui::test]
    fn popup_aligns_with_trigger_below_menu_bar(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::from_tokens(crate::DesignTokens::dark()));
        });

        let (root, cx) = cx.add_window_view(|_, cx| TestRoot {
            menu: cx.new(|cx| ApplicationMenu {
                id: ElementId::from("application-menu-geometry-test"),
                entries: vec![
                    MenuEntry {
                        menu: Menu::new("File")
                            .items([MenuItem::action("Open", Confirm)])
                            .owned(),
                    },
                    MenuEntry {
                        menu: Menu::new("Edit")
                            .items([MenuItem::action("Copy", Confirm)])
                            .owned(),
                    },
                ],
                open_index: Some(0),
                keyboard_index: None,
                show_mnemonics: false,
                popup_index: None,
                popup_menu: None,
                action_context: None,
                row_height: px(34.0),
                focus_handle: cx.focus_handle(),
                embedded_in_titlebar: false,
                _subscription: None,
            }),
            first_focus: cx.focus_handle(),
            second_focus: cx.focus_handle(),
        });

        cx.run_until_parked();

        let file_trigger = cx
            .debug_bounds("application-menu-trigger-File")
            .expect("File trigger should be rendered");
        let edit_trigger = cx
            .debug_bounds("application-menu-trigger-Edit")
            .expect("Edit trigger should be rendered");
        let popup = cx
            .debug_bounds("application-menu-popup-File")
            .expect("File popup should be rendered");

        assert_eq!(file_trigger.left(), px(8.0));
        assert_eq!(edit_trigger.left(), file_trigger.right());
        assert_eq!(popup.left(), file_trigger.left());
        assert!(popup.top() >= px(56.0));

        drop(root);
    }

    // Alt and F10 activation is bound only off macOS, where the system menu bar owns them.
    #[cfg(not(target_os = "macos"))]
    gpui::actions!(application_menu_test, [ShowPalette]);

    #[cfg(not(target_os = "macos"))]
    struct EditorRoot {
        menu: Entity<ApplicationMenu>,
        editor_focus: FocusHandle,
        alt_chords: usize,
        palette_opened: usize,
    }

    #[cfg(not(target_os = "macos"))]
    impl Render for EditorRoot {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let menu = self.menu.clone();
            div()
                .on_action(cx.listener({
                    let menu = menu.clone();
                    move |_, _: &ActivateMenuBar, window, cx| {
                        menu.update(cx, |menu, cx| menu.toggle_keyboard_activation(window, cx));
                    }
                }))
                .on_action(cx.listener(move |_, _: &AltTapMenuBar, window, cx| {
                    menu.update(cx, |menu, cx| menu.toggle_keyboard_activation(window, cx));
                }))
                .on_action(cx.listener(|root, _: &ShowPalette, _, _| {
                    root.palette_opened += 1;
                }))
                .child(
                    div()
                        .id("editor")
                        .track_focus(&self.editor_focus)
                        .on_key_down(cx.listener(|root, event: &KeyDownEvent, _, _| {
                            if event.keystroke.modifiers.alt {
                                root.alt_chords += 1;
                            }
                        })),
                )
                .child(self.menu.clone())
        }
    }

    #[test]
    fn mnemonics_skip_letters_taken_by_earlier_menus() {
        assert_eq!(
            mnemonic_offsets(["File", "Edit", "Find", "Help"]),
            vec![Some(0), Some(0), Some(1), Some(0)]
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn alt_tap_and_f10_drive_menu_bar_without_stealing_alt_chords(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::from_tokens(crate::DesignTokens::dark()));
            crate::menu::init(cx);
            cx.bind_keys([gpui::KeyBinding::new("ctrl-shift-p", ShowPalette, None)]);
        });

        let (root, cx) = cx.add_window_view(|window, cx| {
            let editor_focus = cx.focus_handle();
            editor_focus.focus(window, cx);
            EditorRoot {
                menu: cx.new(|cx| ApplicationMenu {
                    id: ElementId::from("application-menu-keyboard-test"),
                    entries: vec![
                        MenuEntry {
                            menu: Menu::new("File")
                                .items([MenuItem::action("Open", Confirm)])
                                .owned(),
                        },
                        MenuEntry {
                            menu: Menu::new("Edit")
                                .items([MenuItem::action("Command Palette", ShowPalette)])
                                .owned(),
                        },
                    ],
                    open_index: None,
                    keyboard_index: None,
                    show_mnemonics: false,
                    popup_index: None,
                    popup_menu: None,
                    action_context: None,
                    row_height: px(34.0),
                    focus_handle: cx.focus_handle(),
                    embedded_in_titlebar: false,
                    _subscription: None,
                }),
                editor_focus,
                alt_chords: 0,
                palette_opened: 0,
            }
        });
        cx.run_until_parked();
        let (menu, editor_focus) =
            root.read_with(cx, |root, _| (root.menu.clone(), root.editor_focus.clone()));
        let editor_is_focused = |cx: &mut gpui::VisualTestContext| {
            let editor_focus = editor_focus.clone();
            cx.update(|window, _| editor_focus.is_focused(window))
        };

        // Alt held for a chord, as in Helix `Alt+x`, reaches the editor and leaves the bar alone.
        cx.simulate_modifiers_change(gpui::Modifiers::alt());
        cx.simulate_keystrokes("alt-x");
        cx.simulate_modifiers_change(gpui::Modifiers::none());
        assert_eq!(root.read_with(cx, |root, _| root.alt_chords), 1);
        assert_eq!(menu.read_with(cx, |menu, _| menu.keyboard_index), None);
        assert!(editor_is_focused(cx));

        // A solo Alt tap focuses the bar on its first menu and shows access keys.
        cx.simulate_modifiers_change(gpui::Modifiers::alt());
        cx.simulate_modifiers_change(gpui::Modifiers::none());
        menu.read_with(cx, |menu, _| {
            assert_eq!(menu.keyboard_index, Some(0));
            assert!(menu.show_mnemonics);
            assert_eq!(menu.open_index, None);
        });
        assert!(!editor_is_focused(cx));

        cx.simulate_keystrokes("right");
        assert_eq!(menu.read_with(cx, |menu, _| menu.keyboard_index), Some(1));

        // Escape leaves the bar and returns focus to the editor.
        cx.simulate_keystrokes("escape");
        assert_eq!(menu.read_with(cx, |menu, _| menu.keyboard_index), None);
        assert!(editor_is_focused(cx));

        // F10 then the `E` access key opens Edit with platform-formatted shortcuts.
        cx.simulate_keystrokes("f10 e");
        assert_eq!(menu.read_with(cx, |menu, _| menu.open_index), Some(1));
        let popup = menu
            .read_with(cx, |menu, _| menu.popup_menu.clone())
            .expect("Edit popup should be open");
        let shortcut = popup.read_with(cx, |popup, _| match popup.menu_items.first() {
            Some(crate::PopupMenuItem::Item { shortcut, .. }) => shortcut.clone(),
            _ => None,
        });
        let expected = if cfg!(target_os = "macos") {
            "⌃⇧P"
        } else {
            "Ctrl+Shift+P"
        };
        assert_eq!(shortcut.as_deref(), Some(expected));

        // The first item is highlighted, so Enter runs it against the editor.
        cx.simulate_keystrokes("enter");
        assert_eq!(root.read_with(cx, |root, _| root.palette_opened), 1);
        assert_eq!(menu.read_with(cx, |menu, _| menu.open_index), None);
        assert!(editor_is_focused(cx));
    }
}
