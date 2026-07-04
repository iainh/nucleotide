// ABOUTME: Shared native context menu surface
// ABOUTME: Provides consistent menu chrome, backdrop, and item selection styling

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, Context, FocusHandle, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, SharedString, Styled, Window, anchored, div, point, px,
};

use crate::ThemedContext;
use crate::actions::menu::{Cancel, Confirm, SelectDown, SelectUp};
use crate::menu::POPUP_MENU_CONTEXT;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuEntry<T> {
    Action {
        value: T,
        label: SharedString,
        disabled: bool,
    },
    Separator,
}

impl<T> ContextMenuEntry<T> {
    pub fn action(value: T, label: impl Into<SharedString>) -> Self {
        Self::Action {
            value,
            label: label.into(),
            disabled: false,
        }
    }

    pub fn disabled_action(value: T, label: impl Into<SharedString>) -> Self {
        Self::Action {
            value,
            label: label.into(),
            disabled: true,
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn is_action(&self) -> bool {
        matches!(self, Self::Action { .. })
    }
}

pub struct ContextMenuState<'a, T> {
    pub position: (f32, f32),
    pub anchor: Anchor,
    pub offset: (f32, f32),
    pub min_width: Pixels,
    pub selected_index: usize,
    pub focus_handle: Option<FocusHandle>,
    pub entries: &'a [ContextMenuEntry<T>],
}

pub type ContextMenu<'a, T> = ContextMenuState<'a, T>;

impl<'a, T> ContextMenuState<'a, T> {
    pub fn new(position: (f32, f32), entries: &'a [ContextMenuEntry<T>]) -> Self {
        Self {
            position,
            anchor: Anchor::TopLeft,
            offset: (0.0, 0.0),
            min_width: px(200.0),
            selected_index: 0,
            focus_handle: None,
            entries,
        }
    }

    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn offset(mut self, x: f32, y: f32) -> Self {
        self.offset = (x, y);
        self
    }

    pub fn min_width(mut self, min_width: Pixels) -> Self {
        self.min_width = min_width;
        self
    }

    pub fn selected_index(mut self, selected_index: usize) -> Self {
        self.selected_index = selected_index;
        self
    }

    pub fn focus_handle(mut self, focus_handle: FocusHandle) -> Self {
        self.focus_handle = Some(focus_handle);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuKeyboardAction {
    SelectNext,
    SelectPrevious,
    Confirm,
    Cancel,
}

pub struct ContextMenuCallbacks<Hover, Activate, Dismiss, Keyboard> {
    pub on_item_hover: Hover,
    pub on_item_activate: Activate,
    pub on_dismiss: Dismiss,
    pub on_keyboard_action: Keyboard,
}

pub fn render_context_menu<T, I, Hover, Activate, Dismiss, Keyboard>(
    state: ContextMenuState<'_, I>,
    cx: &mut Context<T>,
    callbacks: ContextMenuCallbacks<Hover, Activate, Dismiss, Keyboard>,
) -> gpui::AnyElement
where
    T: 'static,
    I: Copy + 'static,
    Hover: Fn(&mut T, usize, &MouseMoveEvent, &mut Window, &mut Context<T>) + Copy + 'static,
    Activate: Fn(&mut T, I, &MouseDownEvent, &mut Window, &mut Context<T>) + Copy + 'static,
    Dismiss: Fn(&mut T, &MouseDownEvent, &mut Window, &mut Context<T>) + Copy + 'static,
    Keyboard: Fn(&mut T, ContextMenuKeyboardAction, &mut Window, &mut Context<T>) + Copy + 'static,
{
    let ContextMenuCallbacks {
        on_item_hover,
        on_item_activate,
        on_dismiss,
        on_keyboard_action,
    } = callbacks;
    let tokens = &cx.theme().tokens;
    let dropdown_tokens = tokens.dropdown_tokens();
    let focus_handle = state.focus_handle.clone();
    let item_count = state
        .entries
        .iter()
        .filter(|entry| entry.is_action())
        .count();
    let inner_radius = tokens.sizes.radius_md - px(0.5);
    let (x, y) = state.position;
    let (offset_x, offset_y) = state.offset;

    let popup = div()
        .bg(dropdown_tokens.container_background)
        .border_1()
        .border_color(dropdown_tokens.border)
        .rounded(tokens.sizes.radius_md)
        .shadow(vec![
            tokens.chrome.shadow_md.to_box_shadow(false),
            tokens.chrome.inset_highlight.to_box_shadow(true),
        ])
        .min_w(state.min_width)
        .py(tokens.sizes.space_1)
        .px(tokens.sizes.space_1)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .children(state.entries.iter().scan(0usize, |action_index, entry| {
            let ContextMenuEntry::Action {
                value,
                label,
                disabled,
            } = entry
            else {
                return Some(
                    div()
                        .h(px(1.0))
                        .mx(tokens.sizes.space_2)
                        .my(tokens.sizes.space_1)
                        .bg(dropdown_tokens.separator)
                        .into_any_element(),
                );
            };

            let index = *action_index;
            *action_index += 1;
            let value = *value;
            let label = label.clone();
            let disabled = *disabled;
            let is_selected = state.selected_index == index;
            let is_first = index == 0;
            let is_last = index + 1 == item_count;

            Some(
                div()
                    .w_full()
                    .when(is_selected && !disabled, |item| {
                        item.bg(dropdown_tokens.item_background_selected)
                    })
                    .when(is_selected && !disabled && is_first, |item| {
                        item.rounded_tl(inner_radius).rounded_tr(inner_radius)
                    })
                    .when(is_selected && !disabled && is_last, |item| {
                        item.rounded_bl(inner_radius).rounded_br(inner_radius)
                    })
                    .when(!disabled, |item| {
                        item.on_mouse_move(cx.listener(move |state, event, window, cx| {
                            on_item_hover(state, index, event, window, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |state, event, window, cx| {
                                window.prevent_default();
                                on_item_activate(state, value, event, window, cx);
                            }),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .text_size(tokens.sizes.text_sm)
                            .px(tokens.sizes.space_2)
                            .py(tokens.sizes.space_1)
                            .text_color(if disabled {
                                dropdown_tokens.item_text_disabled
                            } else if is_selected {
                                dropdown_tokens.item_text_selected
                            } else {
                                dropdown_tokens.item_text
                            })
                            .child(label),
                    )
                    .into_any_element(),
            )
        }));

    div()
        .absolute()
        .size_full()
        .top_0()
        .left_0()
        .occlude()
        .when_some(focus_handle, |menu, focus_handle| {
            menu.key_context(POPUP_MENU_CONTEXT)
                .track_focus(&focus_handle)
        })
        .on_action(cx.listener(move |state, _: &SelectDown, window, cx| {
            on_keyboard_action(state, ContextMenuKeyboardAction::SelectNext, window, cx);
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |state, _: &SelectUp, window, cx| {
            on_keyboard_action(state, ContextMenuKeyboardAction::SelectPrevious, window, cx);
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |state, _: &Confirm, window, cx| {
            on_keyboard_action(state, ContextMenuKeyboardAction::Confirm, window, cx);
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |state, _: &Cancel, window, cx| {
            on_keyboard_action(state, ContextMenuKeyboardAction::Cancel, window, cx);
            cx.stop_propagation();
        }))
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Left, cx.listener(on_dismiss))
        .on_mouse_down(MouseButton::Right, cx.listener(on_dismiss))
        .child(
            anchored()
                .position(point(px(x), px(y)))
                .anchor(state.anchor)
                .offset(point(px(offset_x), px(offset_y)))
                .snap_to_window_with_margin(tokens.sizes.space_2)
                .child(popup),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::{Context, IntoElement, ParentElement as _, Render, TestAppContext, Window, div};

    use crate::actions::menu::{Cancel, Confirm, SelectDown, SelectUp};
    use crate::{DesignTokens, Theme};

    use super::*;

    #[test]
    fn entry_reports_action_state() {
        assert!(ContextMenuEntry::action(1, "Open").is_action());
        assert!(ContextMenuEntry::disabled_action(1, "Open").is_action());
        assert!(!ContextMenuEntry::<u8>::separator().is_action());
    }

    struct ContextMenuHarness {
        focus_handle: FocusHandle,
        selected_index: usize,
        activated: Option<u8>,
        dismissed: bool,
        entries: Vec<ContextMenuEntry<u8>>,
    }

    impl ContextMenuHarness {
        fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                selected_index: 0,
                activated: None,
                dismissed: false,
                entries: vec![
                    ContextMenuEntry::action(1, "Open"),
                    ContextMenuEntry::action(2, "Rename"),
                ],
            }
        }
    }

    impl Render for ContextMenuHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(render_context_menu(
                ContextMenuState::new((0.0, 0.0), &self.entries)
                    .selected_index(self.selected_index)
                    .focus_handle(self.focus_handle.clone()),
                cx,
                ContextMenuCallbacks {
                    on_item_hover:
                        |harness: &mut ContextMenuHarness,
                         index: usize,
                         _event: &MouseMoveEvent,
                         _window: &mut Window,
                         cx: &mut Context<ContextMenuHarness>| {
                            harness.selected_index = index;
                            cx.notify();
                        },
                    on_item_activate:
                        |harness: &mut ContextMenuHarness,
                         value: u8,
                         _event: &MouseDownEvent,
                         _window: &mut Window,
                         cx: &mut Context<ContextMenuHarness>| {
                            harness.activated = Some(value);
                            cx.notify();
                        },
                    on_dismiss:
                        |harness: &mut ContextMenuHarness,
                         _event: &MouseDownEvent,
                         _window: &mut Window,
                         cx: &mut Context<ContextMenuHarness>| {
                            harness.dismissed = true;
                            cx.notify();
                        },
                    on_keyboard_action:
                        |harness: &mut ContextMenuHarness,
                         action: ContextMenuKeyboardAction,
                         _window: &mut Window,
                         cx: &mut Context<ContextMenuHarness>| {
                            match action {
                                ContextMenuKeyboardAction::SelectNext => {
                                    harness.selected_index =
                                        (harness.selected_index + 1) % harness.entries.len();
                                }
                                ContextMenuKeyboardAction::SelectPrevious => {
                                    harness.selected_index =
                                        (harness.selected_index + harness.entries.len() - 1)
                                            % harness.entries.len();
                                }
                                ContextMenuKeyboardAction::Confirm => {
                                    harness.activated =
                                        Some(match harness.entries.get(harness.selected_index) {
                                            Some(ContextMenuEntry::Action { value, .. }) => *value,
                                            Some(ContextMenuEntry::Separator) | None => 0,
                                        });
                                }
                                ContextMenuKeyboardAction::Cancel => {
                                    harness.dismissed = true;
                                }
                            }
                            cx.notify();
                        },
                },
            ))
        }
    }

    fn init_context_menu_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::menu::init(cx);
            cx.set_global(Theme::from_tokens(DesignTokens::dark()));
        });
    }

    #[gpui::test]
    fn context_menu_handles_keyboard_actions(cx: &mut TestAppContext) {
        init_context_menu_test(cx);
        let (harness, cx) = cx.add_window_view(ContextMenuHarness::new);
        let focus = harness.read_with(cx, |harness, _| harness.focus_handle.clone());

        cx.update(|window, cx| {
            window.focus(&focus, cx);
            focus.dispatch_action(&SelectDown, window, cx);
            focus.dispatch_action(&Confirm, window, cx);
            focus.dispatch_action(&Cancel, window, cx);
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(harness.selected_index, 1);
            assert_eq!(harness.activated, Some(2));
            assert!(harness.dismissed);
        });
    }

    #[gpui::test]
    fn context_menu_wraps_keyboard_selection(cx: &mut TestAppContext) {
        init_context_menu_test(cx);
        let (harness, cx) = cx.add_window_view(ContextMenuHarness::new);
        let focus = harness.read_with(cx, |harness, _| harness.focus_handle.clone());

        cx.update(|window, cx| {
            window.focus(&focus, cx);
            focus.dispatch_action(&SelectUp, window, cx);
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(harness.selected_index, 1);
        });
    }
}
