// ABOUTME: Stateful popup menu with keyboard navigation, checked items, and nested submenus
// ABOUTME: Repurposes gpui-component menu behaviour using Nucleotide design tokens

use gpui::prelude::FluentBuilder;
use gpui::{
    Action, Anchor, App, AppContext as _, Context, DismissEvent, Edges, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    OwnedMenuItem, ParentElement, Pixels, Render, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, anchored, div, px, svg,
};

use crate::ThemedContext;
use crate::actions::menu::{Cancel, Confirm, SelectDown, SelectLeft, SelectRight, SelectUp};

use super::POPUP_MENU_CONTEXT;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuCheckSide {
    #[default]
    Left,
    Right,
}

impl MenuCheckSide {
    fn is_left(self) -> bool {
        matches!(self, Self::Left)
    }

    fn is_right(self) -> bool {
        matches!(self, Self::Right)
    }
}

pub enum PopupMenuItem {
    Separator,
    Label(SharedString),
    Item {
        label: SharedString,
        disabled: bool,
        checked: bool,
        action: Option<Box<dyn Action>>,
    },
    Submenu {
        label: SharedString,
        disabled: bool,
        menu: Entity<PopupMenu>,
    },
}

impl PopupMenuItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self::Item {
            label: label.into(),
            disabled: false,
            checked: false,
            action: None,
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn label(label: impl Into<SharedString>) -> Self {
        Self::Label(label.into())
    }

    pub fn submenu(label: impl Into<SharedString>, menu: Entity<PopupMenu>) -> Self {
        Self::Submenu {
            label: label.into(),
            disabled: false,
            menu,
        }
    }

    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        if let Self::Item {
            action: item_action,
            ..
        } = &mut self
        {
            *item_action = Some(action);
        }
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        if let Self::Item {
            checked: item_checked,
            ..
        } = &mut self
        {
            *item_checked = checked;
        }
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        match &mut self {
            Self::Item {
                disabled: item_disabled,
                ..
            }
            | Self::Submenu {
                disabled: item_disabled,
                ..
            } => *item_disabled = disabled,
            Self::Separator | Self::Label(_) => {}
        }
        self
    }

    fn is_clickable(&self) -> bool {
        matches!(
            self,
            Self::Item {
                disabled: false,
                ..
            } | Self::Submenu {
                disabled: false,
                ..
            }
        )
    }

    fn is_separator(&self) -> bool {
        matches!(self, Self::Separator)
    }

    fn is_checked(&self) -> bool {
        matches!(self, Self::Item { checked: true, .. })
    }
}

pub struct PopupMenu {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) menu_items: Vec<PopupMenuItem>,
    pub(crate) action_context: Option<FocusHandle>,
    selected_index: Option<usize>,
    min_width: Option<Pixels>,
    max_width: Option<Pixels>,
    max_height: Option<Pixels>,
    check_side: MenuCheckSide,
    parent_menu: Option<WeakEntity<Self>>,
    scrollable: bool,
    scroll_handle: ScrollHandle,
    submenu_anchor: (Anchor, Pixels),
    _subscriptions: Vec<Subscription>,
}

impl PopupMenu {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            action_context: None,
            menu_items: Vec::new(),
            selected_index: None,
            min_width: None,
            max_width: None,
            max_height: None,
            check_side: MenuCheckSide::Left,
            parent_menu: None,
            scrollable: false,
            scroll_handle: ScrollHandle::new(),
            submenu_anchor: (Anchor::TopLeft, Pixels::ZERO),
            _subscriptions: Vec::new(),
        }
    }

    pub fn build(
        window: &mut Window,
        cx: &mut App,
        build: impl FnOnce(Self, &mut Window, &mut Context<PopupMenu>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| build(Self::new(cx), window, cx))
    }

    pub fn action_context(mut self, handle: FocusHandle) -> Self {
        self.action_context = Some(handle);
        self
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn set_action_context(
        &mut self,
        action_context: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        self.action_context = action_context.clone();

        for item in &self.menu_items {
            if let PopupMenuItem::Submenu { menu, .. } = item {
                menu.update(cx, |menu, cx| {
                    menu.set_action_context(action_context.clone(), cx);
                });
            }
        }
    }

    pub fn min_w(mut self, width: impl Into<Pixels>) -> Self {
        self.min_width = Some(width.into());
        self
    }

    pub fn max_w(mut self, width: impl Into<Pixels>) -> Self {
        self.max_width = Some(width.into());
        self
    }

    pub fn max_h(mut self, height: impl Into<Pixels>) -> Self {
        self.max_height = Some(height.into());
        self
    }

    pub fn scrollable(mut self, scrollable: bool) -> Self {
        self.scrollable = scrollable;
        self
    }

    pub fn check_side(mut self, side: MenuCheckSide) -> Self {
        self.check_side = side;
        self
    }

    pub fn menu(self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        self.menu_with_check_and_disabled(label, false, action, false)
    }

    pub fn menu_with_check_and_disabled(
        mut self,
        label: impl Into<SharedString>,
        checked: bool,
        action: Box<dyn Action>,
        disabled: bool,
    ) -> Self {
        self.menu_items.push(
            PopupMenuItem::new(label)
                .checked(checked)
                .disabled(disabled)
                .action(action),
        );
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.menu_items.push(PopupMenuItem::label(label));
        self
    }

    pub fn separator(mut self) -> Self {
        if !self.menu_items.is_empty()
            && !matches!(self.menu_items.last(), Some(PopupMenuItem::Separator))
        {
            self.menu_items.push(PopupMenuItem::separator());
        }
        self
    }

    pub fn submenu(
        mut self,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        let submenu = PopupMenu::build(window, cx, build);
        let parent_menu = cx.entity().downgrade();
        submenu.update(cx, |view, _| {
            view.parent_menu = Some(parent_menu);
        });
        self.menu_items.push(PopupMenuItem::submenu(label, submenu));
        self
    }

    pub fn item(mut self, item: PopupMenuItem) -> Self {
        self.menu_items.push(item);
        self
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn with_menu_items<I>(
        mut self,
        items: impl IntoIterator<Item = I>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self
    where
        I: Into<OwnedMenuItem>,
    {
        for item in items {
            match item.into() {
                OwnedMenuItem::Action {
                    name,
                    action,
                    checked,
                    disabled,
                    ..
                } => {
                    self = self.menu_with_check_and_disabled(
                        name,
                        checked,
                        action.boxed_clone(),
                        disabled,
                    );
                }
                OwnedMenuItem::Separator => {
                    self = self.separator();
                }
                OwnedMenuItem::Submenu(submenu) => {
                    self = self.submenu(submenu.name, window, cx, move |menu, window, cx| {
                        menu.with_menu_items(submenu.items.clone(), window, cx)
                    });
                }
                OwnedMenuItem::SystemMenu(_) => {}
            }
        }

        if self.menu_items.len() > 20 {
            self.scrollable = true;
        }

        self
    }

    pub fn is_empty(&self) -> bool {
        self.menu_items.is_empty()
    }

    pub(crate) fn active_submenu(&self) -> Option<Entity<PopupMenu>> {
        self.selected_index
            .and_then(|index| self.menu_items.get(index))
            .and_then(|item| match item {
                PopupMenuItem::Submenu { menu, .. } => Some(menu.clone()),
                PopupMenuItem::Separator | PopupMenuItem::Label(_) | PopupMenuItem::Item { .. } => {
                    None
                }
            })
    }

    fn clickable_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.menu_items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| item.is_clickable().then_some(index))
    }

    fn set_selected_index(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if self.selected_index != index {
            self.selected_index = index;
            if let Some(index) = index {
                self.scroll_handle.scroll_to_item(index);
            }
            cx.notify();
        }
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();

        let selected = self.selected_index.unwrap_or(0);
        let previous = self
            .menu_items
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, item)| (index < selected && item.is_clickable()).then_some(index));
        let next = previous.or_else(|| self.clickable_indices().last());
        self.set_selected_index(next, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();

        let next = match self.selected_index {
            Some(selected) => self.clickable_indices().find(|index| *index > selected),
            None => self.clickable_indices().next(),
        }
        .or_else(|| self.clickable_indices().next());

        self.set_selected_index(next, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.parent_side(cx).is_right() {
            self.focus_parent_menu(window, cx);
            return;
        }

        if self.unselect_submenu(cx) {
            return;
        }

        if self.parent_menu.is_none() {
            cx.propagate();
        }
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.select_submenu(window, cx) {
            return;
        }

        if self.parent_side(cx).is_left() && self.parent_menu.is_some() {
            self.focus_parent_menu(window, cx);
            return;
        }

        if self.parent_menu.is_none() {
            cx.propagate();
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_index else {
            return;
        };

        match self.menu_items.get(index) {
            Some(PopupMenuItem::Item { action, .. }) => {
                if let Some(action) = action.as_ref() {
                    self.dispatch_confirm_action(action.as_ref(), window, cx);
                }
                self.dismiss(&Cancel, window, cx);
            }
            Some(PopupMenuItem::Submenu { .. }) => {
                self.select_submenu(window, cx);
            }
            Some(PopupMenuItem::Separator | PopupMenuItem::Label(_)) | None => {}
        }
    }

    fn on_click(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.prevent_default();
        cx.stop_propagation();
        self.selected_index = Some(index);
        self.confirm(&Confirm, window, cx);
    }

    fn dispatch_confirm_action(
        &self,
        action: &dyn Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(context) = self.action_context.as_ref() {
            context.focus(window, cx);
        }
        window.dispatch_action(action.boxed_clone(), cx);
    }

    fn select_submenu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(submenu) = self.active_submenu() else {
            return false;
        };

        submenu.update(cx, |submenu, cx| {
            let first = submenu.clickable_indices().next();
            submenu.set_selected_index(first, cx);
            submenu.focus_handle.focus(window, cx);
        });
        cx.notify();
        true
    }

    fn unselect_submenu(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(submenu) = self.active_submenu() else {
            return false;
        };

        submenu.update(cx, |submenu, cx| {
            submenu.set_selected_index(None, cx);
        });
        true
    }

    fn focus_parent_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self
            .parent_menu
            .as_ref()
            .and_then(|parent| parent.upgrade())
        else {
            return;
        };

        self.selected_index = None;
        parent.update(cx, |parent, cx| {
            parent.focus_handle.focus(window, cx);
            cx.notify();
        });
    }

    fn parent_side(&self, cx: &App) -> MenuCheckSide {
        let Some(parent) = self
            .parent_menu
            .as_ref()
            .and_then(|parent| parent.upgrade())
        else {
            return MenuCheckSide::Left;
        };

        match parent.read(cx).submenu_anchor.0 {
            Anchor::TopRight | Anchor::BottomRight => MenuCheckSide::Right,
            _ => MenuCheckSide::Left,
        }
    }

    fn dismiss(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);

        if let Some(action_context) = self.action_context.as_ref() {
            action_context.focus(window, cx);
        }

        if let Some(parent_menu) = self.parent_menu.clone() {
            let _ = parent_menu.update(cx, |parent, cx| {
                parent.selected_index = None;
                parent.dismiss(&Cancel, window, cx);
            });
        }
    }

    fn on_mouse_down_out(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss(&Cancel, window, cx);
        cx.stop_propagation();
    }

    fn max_width(&self) -> Pixels {
        self.max_width.unwrap_or(px(420.0))
    }

    fn update_submenu_anchor(&mut self) {
        let left = self.min_width.unwrap_or(px(180.0)) - px(4.0);
        self.submenu_anchor = (Anchor::TopLeft, left);
    }

    fn render_indicator(
        &self,
        checked: bool,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tokens = cx.theme().tokens;
        let dropdown = tokens.dropdown_tokens();

        div()
            .w(tokens.sizes.space_5)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .text_size(tokens.sizes.text_sm)
            .text_color(if disabled {
                dropdown.icon_color_disabled
            } else {
                dropdown.icon_color
            })
            .when(checked, |this| this.child("✓"))
    }

    fn render_item(
        &self,
        index: usize,
        item: &PopupMenuItem,
        has_check_column: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.theme().tokens;
        let dropdown = tokens.dropdown_tokens();
        let selected = self.selected_index == Some(index);
        let item_height = px(26.0);
        match item {
            PopupMenuItem::Separator => div()
                .h(px(1.0))
                .mx(tokens.sizes.space_2)
                .my(tokens.sizes.space_1)
                .bg(dropdown.separator)
                .into_any_element(),
            PopupMenuItem::Label(label) => div()
                .h(item_height)
                .px(tokens.sizes.space_2)
                .flex()
                .items_center()
                .text_size(tokens.sizes.text_sm)
                .text_color(dropdown.item_text_secondary)
                .child(label.clone())
                .into_any_element(),
            PopupMenuItem::Item {
                label,
                disabled,
                checked,
                ..
            } => {
                let is_checked_left = self.check_side.is_left() && *checked;
                let is_checked_right = self.check_side.is_right() && *checked;
                self.render_row(index, selected, *disabled, cx)
                    .child(
                        div()
                            .h(item_height)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens.sizes.space_2)
                            .when(has_check_column, |this| {
                                this.child(self.render_indicator(is_checked_left, *disabled, cx))
                            })
                            .child(div().flex_1().child(label.clone()))
                            .when(is_checked_right, |this| {
                                this.child(self.render_indicator(true, *disabled, cx))
                            }),
                    )
                    .into_any_element()
            }
            PopupMenuItem::Submenu {
                label,
                disabled,
                menu,
            } => {
                let (anchor, left) = self.submenu_anchor;
                let opens_up = matches!(anchor, Anchor::BottomLeft | Anchor::BottomRight);

                self.render_row(index, selected, *disabled, cx)
                    .child(
                        div()
                            .h(item_height)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens.sizes.space_2)
                            .when(has_check_column, |this| {
                                this.child(self.render_indicator(false, *disabled, cx))
                            })
                            .child(div().flex_1().child(label.clone()))
                            .child(
                                svg()
                                    .path("icons/chevron-right.svg")
                                    .size(tokens.sizes.text_sm)
                                    .text_color(if *disabled {
                                        dropdown.icon_color_disabled
                                    } else {
                                        dropdown.icon_color
                                    })
                                    .flex_shrink_0(),
                            ),
                    )
                    .when(selected && !disabled, |this| {
                        this.child(
                            anchored()
                                .anchor(anchor)
                                .snap_to_window_with_margin(Edges::all(tokens.sizes.space_2))
                                .child(
                                    div()
                                        .occlude()
                                        .when(opens_up, |this| this.bottom_0())
                                        .when(!opens_up, |this| this.top_0())
                                        .left(left)
                                        .child(menu.clone()),
                                ),
                        )
                    })
                    .into_any_element()
            }
        }
    }

    fn render_row(
        &self,
        index: usize,
        selected: bool,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let tokens = cx.theme().tokens;
        let dropdown = tokens.dropdown_tokens();

        div()
            .id(index)
            .relative()
            .w_full()
            .px(tokens.sizes.space_2)
            .rounded(tokens.sizes.radius_sm)
            .text_size(tokens.sizes.text_sm)
            .text_color(if disabled {
                dropdown.item_text_disabled
            } else if selected {
                dropdown.item_text_selected
            } else {
                dropdown.item_text
            })
            .when(selected && !disabled, |this| {
                this.bg(dropdown.item_background_selected)
            })
            .when(!selected && !disabled, |this| {
                this.hover(|this| this.bg(dropdown.item_background_hover))
            })
            .when(disabled, |this| this.cursor_not_allowed())
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .on_mouse_move(cx.listener(move |menu, _, _, cx| {
                        menu.set_selected_index(Some(index), cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |menu, _, window, cx| {
                            menu.on_click(index, window, cx);
                        }),
                    )
            })
    }
}

impl EventEmitter<DismissEvent> for PopupMenu {}

impl Focusable for PopupMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PopupMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_submenu_anchor();

        let tokens = cx.theme().tokens;
        let dropdown = tokens.dropdown_tokens();
        let items_count = self.menu_items.len();
        let has_check_column = self
            .menu_items
            .iter()
            .any(|item| self.check_side.is_left() && item.is_checked());
        let max_height = self.max_height.unwrap_or_else(|| {
            let window_half_height = window.window_bounds().get_bounds().size.height * 0.5;
            window_half_height.min(px(450.0))
        });

        div()
            .id("popup-menu")
            .key_context(POPUP_MENU_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down_out(cx.listener(Self::on_mouse_down_out))
            .relative()
            .occlude()
            .bg(dropdown.container_background)
            .border_1()
            .border_color(dropdown.border)
            .rounded(tokens.sizes.radius_md)
            .shadow(vec![
                tokens.chrome.shadow_md.to_box_shadow(false),
                tokens.chrome.inset_highlight.to_box_shadow(true),
            ])
            .child(
                div()
                    .id("popup-menu-items")
                    .flex()
                    .flex_col()
                    .p(tokens.sizes.space_1)
                    .min_w(self.min_width.unwrap_or(px(180.0)))
                    .max_w(self.max_width())
                    .when(self.scrollable, |this| {
                        this.max_h(max_height)
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                    })
                    .children(
                        self.menu_items
                            .iter()
                            .enumerate()
                            .filter(|(index, item)| {
                                !(*index + 1 == items_count && item.is_separator())
                            })
                            .map(|(index, item)| {
                                self.render_item(index, item, has_check_column, cx)
                            }),
                    ),
            )
    }
}
