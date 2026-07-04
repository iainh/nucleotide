// ABOUTME: Shared native context menu surface
// ABOUTME: Provides consistent menu chrome, backdrop, and item selection styling

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, Context, FocusHandle, InteractiveElement, IntoElement, MouseButton, ParentElement,
    Pixels, SharedString, StatefulInteractiveElement, Styled, Window, accesskit::Role, anchored,
    div, point, px,
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

    pub fn is_enabled_action(&self) -> bool {
        matches!(
            self,
            Self::Action {
                disabled: false,
                ..
            }
        )
    }
}

pub struct ContextMenuState<'a, T> {
    pub position: (f32, f32),
    pub anchor: Anchor,
    pub offset: (f32, f32),
    pub min_width: Pixels,
    pub selected_index: usize,
    pub focus_handle: Option<FocusHandle>,
    pub restore_focus_handle: Option<FocusHandle>,
    pub entries: &'a [ContextMenuEntry<T>],
}

pub type ContextMenu<'a, T> = ContextMenuState<'a, T>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextMenuController {
    open: bool,
    position: (f32, f32),
    selected_index: usize,
}

impl ContextMenuController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn position(&self) -> (f32, f32) {
        self.position
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn open_at(&mut self, position: (f32, f32)) {
        self.open = true;
        self.position = position;
        self.selected_index = 0;
    }

    pub fn close(&mut self) -> bool {
        if !self.open {
            return false;
        }

        self.open = false;
        true
    }

    pub fn select(&mut self, selected_index: usize) -> bool {
        if self.selected_index == selected_index {
            return false;
        }

        self.selected_index = selected_index;
        true
    }

    pub fn state<'a, T>(&self, entries: &'a [ContextMenuEntry<T>]) -> ContextMenuState<'a, T> {
        ContextMenuState::new(self.position, entries).selected_index(self.selected_index)
    }
}

impl Default for ContextMenuController {
    fn default() -> Self {
        Self {
            open: false,
            position: (0.0, 0.0),
            selected_index: 0,
        }
    }
}

impl<'a, T> ContextMenuState<'a, T> {
    pub fn new(position: (f32, f32), entries: &'a [ContextMenuEntry<T>]) -> Self {
        Self {
            position,
            anchor: Anchor::TopLeft,
            offset: (0.0, 0.0),
            min_width: px(200.0),
            selected_index: 0,
            focus_handle: None,
            restore_focus_handle: None,
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

    pub fn restore_focus_handle(mut self, focus_handle: FocusHandle) -> Self {
        self.restore_focus_handle = Some(focus_handle);
        self
    }

    pub fn next_enabled_action_index(&self) -> Option<usize> {
        let mut first = None;
        let mut next = None;
        let mut action_index = 0;

        for entry in self.entries {
            if entry.is_action() {
                if entry.is_enabled_action() {
                    first.get_or_insert(action_index);
                    if action_index > self.selected_index && next.is_none() {
                        next = Some(action_index);
                    }
                }
                action_index += 1;
            }
        }

        next.or(first)
    }

    pub fn previous_enabled_action_index(&self) -> Option<usize> {
        let mut previous = None;
        let mut last = None;
        let mut action_index = 0;

        for entry in self.entries {
            if entry.is_action() {
                if entry.is_enabled_action() {
                    last = Some(action_index);
                    if action_index < self.selected_index {
                        previous = Some(action_index);
                    }
                }
                action_index += 1;
            }
        }

        previous.or(last)
    }
}

impl<T: Copy> ContextMenuState<'_, T> {
    pub fn normalized_selected_index(&self) -> usize {
        let Some(first_enabled) = self.next_enabled_action_index() else {
            return self.selected_index;
        };

        if self.selected_enabled_action_value().is_some() {
            self.selected_index
        } else {
            first_enabled
        }
    }

    pub fn selected_enabled_action_value(&self) -> Option<T> {
        let mut action_index = 0;

        for entry in self.entries {
            match entry {
                ContextMenuEntry::Action {
                    value,
                    disabled: false,
                    ..
                } if action_index == self.selected_index => return Some(*value),
                ContextMenuEntry::Action { .. } => action_index += 1,
                ContextMenuEntry::Separator => {}
            }
        }

        None
    }
}

pub struct ContextMenuCallbacks<Select, Activate, Dismiss> {
    pub on_item_select: Select,
    pub on_item_activate: Activate,
    pub on_dismiss: Dismiss,
}

pub fn render_context_menu<T, I, Select, Activate, Dismiss>(
    state: ContextMenuState<'_, I>,
    cx: &mut Context<T>,
    callbacks: ContextMenuCallbacks<Select, Activate, Dismiss>,
) -> gpui::AnyElement
where
    T: 'static,
    I: Copy + 'static,
    Select: Fn(&mut T, usize, &mut Window, &mut Context<T>) + Copy + 'static,
    Activate: Fn(&mut T, I, &mut Window, &mut Context<T>) + Copy + 'static,
    Dismiss: Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
{
    let ContextMenuCallbacks {
        on_item_select,
        on_item_activate,
        on_dismiss,
    } = callbacks;
    let tokens = &cx.theme().tokens;
    let dropdown_tokens = tokens.dropdown_tokens();
    let focus_handle = state.focus_handle.clone();
    let restore_focus_on_confirm = state.restore_focus_handle.clone();
    let restore_focus_on_cancel = state.restore_focus_handle.clone();
    let restore_focus_on_left_dismiss = state.restore_focus_handle.clone();
    let restore_focus_on_right_dismiss = state.restore_focus_handle.clone();
    let item_count = state
        .entries
        .iter()
        .filter(|entry| entry.is_action())
        .count();
    let inner_radius = tokens.sizes.radius_md - px(0.5);
    let (x, y) = state.position;
    let (offset_x, offset_y) = state.offset;
    let mut state = state;
    let selected_index = state.normalized_selected_index();
    state.selected_index = selected_index;
    let next_index = state.next_enabled_action_index();
    let previous_index = state.previous_enabled_action_index();
    let selected_value = state.selected_enabled_action_value();

    let popup = div()
        .id("context-menu")
        .role(Role::Menu)
        .aria_label("Context menu")
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
            let is_selected = selected_index == index;
            let is_first = index == 0;
            let is_last = index + 1 == item_count;
            let restore_focus_on_item_activate = state.restore_focus_handle.clone();

            Some(
                div()
                    .id(("context-menu-item", index))
                    .role(Role::MenuItem)
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
                        item.on_mouse_move(cx.listener(move |state, _event, window, cx| {
                            on_item_select(state, index, window, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |state, _event, window, cx| {
                                window.prevent_default();
                                if let Some(focus_handle) = restore_focus_on_item_activate.as_ref()
                                {
                                    focus_handle.focus(window, cx);
                                }
                                on_item_activate(state, value, window, cx);
                                cx.stop_propagation();
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
            if let Some(index) = next_index {
                on_item_select(state, index, window, cx);
            }
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |state, _: &SelectUp, window, cx| {
            if let Some(index) = previous_index {
                on_item_select(state, index, window, cx);
            }
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |state, _: &Confirm, window, cx| {
            if let Some(value) = selected_value {
                if let Some(focus_handle) = restore_focus_on_confirm.as_ref() {
                    focus_handle.focus(window, cx);
                }
                on_item_activate(state, value, window, cx);
            }
            cx.stop_propagation();
        }))
        .on_action(cx.listener(move |state, _: &Cancel, window, cx| {
            if let Some(focus_handle) = restore_focus_on_cancel.as_ref() {
                focus_handle.focus(window, cx);
            }
            on_dismiss(state, window, cx);
            cx.stop_propagation();
        }))
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |state, _event, window, cx| {
                if let Some(focus_handle) = restore_focus_on_left_dismiss.as_ref() {
                    focus_handle.focus(window, cx);
                }
                on_dismiss(state, window, cx);
                cx.stop_propagation();
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |state, _event, window, cx| {
                if let Some(focus_handle) = restore_focus_on_right_dismiss.as_ref() {
                    focus_handle.focus(window, cx);
                }
                on_dismiss(state, window, cx);
                cx.stop_propagation();
            }),
        )
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

    #[test]
    fn controller_opens_at_position_and_resets_selection() {
        let mut controller = ContextMenuController::new();

        assert!(!controller.is_open());
        controller.select(3);
        controller.open_at((12.0, 24.0));

        assert!(controller.is_open());
        assert_eq!(controller.position(), (12.0, 24.0));
        assert_eq!(controller.selected_index(), 0);
    }

    #[test]
    fn controller_close_and_select_report_changes() {
        let mut controller = ContextMenuController::new();

        assert!(!controller.close());
        assert!(controller.select(2));
        assert!(!controller.select(2));

        controller.open_at((1.0, 2.0));
        assert!(controller.close());
        assert!(!controller.is_open());
        assert!(!controller.close());
    }

    #[test]
    fn controller_builds_context_menu_state() {
        let mut controller = ContextMenuController::new();
        let entries = vec![ContextMenuEntry::action(1, "Open")];

        controller.open_at((5.0, 6.0));
        controller.select(1);

        let state = controller.state(&entries);
        assert_eq!(state.position, (5.0, 6.0));
        assert_eq!(state.selected_index, 1);
        assert_eq!(state.entries, entries.as_slice());
    }

    #[test]
    fn state_navigation_skips_disabled_actions_and_separators() {
        let entries = vec![
            ContextMenuEntry::disabled_action(1, "Open"),
            ContextMenuEntry::separator(),
            ContextMenuEntry::action(2, "Rename"),
            ContextMenuEntry::action(3, "Delete"),
            ContextMenuEntry::disabled_action(4, "Duplicate"),
        ];

        let state = ContextMenuState::new((0.0, 0.0), &entries).selected_index(0);
        assert_eq!(state.normalized_selected_index(), 1);
        assert_eq!(state.next_enabled_action_index(), Some(1));
        assert_eq!(state.previous_enabled_action_index(), Some(2));
        assert_eq!(state.selected_enabled_action_value(), None);

        let selected_state = state.selected_index(1);
        assert_eq!(selected_state.normalized_selected_index(), 1);
        assert_eq!(selected_state.selected_enabled_action_value(), Some(2));
        assert_eq!(selected_state.next_enabled_action_index(), Some(2));
        assert_eq!(selected_state.previous_enabled_action_index(), Some(2));
    }

    #[test]
    fn state_navigation_preserves_selection_when_no_enabled_actions_exist() {
        let entries = vec![
            ContextMenuEntry::disabled_action(1, "Open"),
            ContextMenuEntry::separator(),
        ];

        let state = ContextMenuState::new((0.0, 0.0), &entries).selected_index(3);

        assert_eq!(state.normalized_selected_index(), 3);
        assert_eq!(state.next_enabled_action_index(), None);
        assert_eq!(state.previous_enabled_action_index(), None);
        assert_eq!(state.selected_enabled_action_value(), None);
    }

    struct ContextMenuHarness {
        focus_handle: FocusHandle,
        restore_focus_handle: FocusHandle,
        selected_index: usize,
        activated: Option<u8>,
        dismissed: bool,
        entries: Vec<ContextMenuEntry<u8>>,
    }

    impl ContextMenuHarness {
        fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                restore_focus_handle: cx.focus_handle(),
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
            div()
                .size_full()
                .child(
                    div()
                        .id("restore-focus")
                        .track_focus(&self.restore_focus_handle)
                        .size(px(1.0)),
                )
                .child(render_context_menu(
                    ContextMenuState::new((0.0, 0.0), &self.entries)
                        .selected_index(self.selected_index)
                        .focus_handle(self.focus_handle.clone())
                        .restore_focus_handle(self.restore_focus_handle.clone()),
                    cx,
                    ContextMenuCallbacks {
                        on_item_select:
                            |harness: &mut ContextMenuHarness,
                             index: usize,
                             _window: &mut Window,
                             cx: &mut Context<ContextMenuHarness>| {
                                harness.selected_index = index;
                                cx.notify();
                            },
                        on_item_activate:
                            |harness: &mut ContextMenuHarness,
                             value: u8,
                             _window: &mut Window,
                             cx: &mut Context<ContextMenuHarness>| {
                                harness.activated = Some(value);
                                cx.notify();
                            },
                        on_dismiss:
                            |harness: &mut ContextMenuHarness,
                             _window: &mut Window,
                             cx: &mut Context<ContextMenuHarness>| {
                                harness.dismissed = true;
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
        let (focus, restore_focus) = harness.read_with(cx, |harness, _| {
            (
                harness.focus_handle.clone(),
                harness.restore_focus_handle.clone(),
            )
        });

        cx.update(|window, cx| {
            window.focus(&focus, cx);
            focus.dispatch_action(&SelectDown, window, cx);
        });

        cx.update(|window, cx| {
            focus.dispatch_action(&Confirm, window, cx);
        });

        cx.update(|window, cx| {
            focus.dispatch_action(&Cancel, window, cx);
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(harness.selected_index, 1);
            assert_eq!(harness.activated, Some(2));
            assert!(harness.dismissed);
        });
        cx.update(|window, _cx| {
            assert!(restore_focus.is_focused(window));
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

    #[test]
    fn context_menu_navigation_skips_disabled_actions() {
        let entries = vec![
            ContextMenuEntry::disabled_action(1, "Disabled"),
            ContextMenuEntry::separator(),
            ContextMenuEntry::action(2, "Open"),
        ];
        let state = ContextMenuState::new((0.0, 0.0), &entries).selected_index(0);

        assert_eq!(state.next_enabled_action_index(), Some(1));
        assert_eq!(state.previous_enabled_action_index(), Some(1));
        assert_eq!(state.selected_enabled_action_value(), None);
        assert_eq!(
            ContextMenuState::new((0.0, 0.0), &entries)
                .selected_index(1)
                .selected_enabled_action_value(),
            Some(2)
        );
        assert_eq!(state.normalized_selected_index(), 1);
    }

    #[gpui::test]
    fn context_menu_confirms_first_enabled_action(cx: &mut TestAppContext) {
        init_context_menu_test(cx);
        let (harness, cx) = cx.add_window_view(ContextMenuHarness::new);
        let focus = harness.read_with(cx, |harness, _| harness.focus_handle.clone());

        harness.update(cx, |harness, cx| {
            harness.entries = vec![
                ContextMenuEntry::disabled_action(1, "Disabled"),
                ContextMenuEntry::action(2, "Open"),
            ];
            harness.selected_index = 0;
            cx.notify();
        });

        cx.update(|window, cx| {
            window.focus(&focus, cx);
            focus.dispatch_action(&Confirm, window, cx);
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(harness.activated, Some(2));
        });
    }
}
