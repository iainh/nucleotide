// ABOUTME: Shared keyboard navigation wrapper for focusable list-like UI
// ABOUTME: Packages GPUI action-driven focus traversal for menus, panels, and pickers

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ElementId, FocusHandle, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Refineable, RenderOnce, ScrollAnchor, ScrollHandle, ScrollStrategy,
    StyleRefinement, Styled, UniformListScrollHandle, Window, div,
};

use crate::actions::menu::{Confirm, SelectDown, SelectUp, ToggleSelected};

pub const NAVIGABLE_CONTEXT: &str = "Navigable";
/// Key context of a [`NavigableList`].
pub const NAVIGABLE_LIST_CONTEXT: &str = "NavigableList";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some(NAVIGABLE_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(NAVIGABLE_CONTEXT)),
        KeyBinding::new("ctrl-p", SelectUp, Some(NAVIGABLE_CONTEXT)),
        KeyBinding::new("ctrl-n", SelectDown, Some(NAVIGABLE_CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(NAVIGABLE_LIST_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(NAVIGABLE_LIST_CONTEXT)),
        KeyBinding::new("k", SelectUp, Some(NAVIGABLE_LIST_CONTEXT)),
        KeyBinding::new("j", SelectDown, Some(NAVIGABLE_LIST_CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(NAVIGABLE_LIST_CONTEXT)),
        KeyBinding::new("space", ToggleSelected, Some(NAVIGABLE_LIST_CONTEXT)),
    ]);
}

type IndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Keyboard navigation for a virtualized list whose rows are built only when
/// on screen, so they cannot each hold focus. The list holds focus instead and
/// tracks a selected index: up and down (or `k` and `j`) select the previous
/// or next row and scroll it into view, Enter confirms it, and space toggles it.
#[derive(IntoElement)]
pub struct NavigableList {
    id: ElementId,
    focus_handle: FocusHandle,
    len: usize,
    selected: Option<usize>,
    scroll_handle: Option<UniformListScrollHandle>,
    on_select: Option<IndexHandler>,
    on_confirm: Option<IndexHandler>,
    on_toggle: Option<IndexHandler>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl NavigableList {
    pub fn new(id: impl Into<ElementId>, focus_handle: &FocusHandle, len: usize) -> Self {
        Self {
            id: id.into(),
            focus_handle: focus_handle.clone(),
            len,
            selected: None,
            scroll_handle: None,
            on_select: None,
            on_confirm: None,
            on_toggle: None,
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// The list's scroll handle, which keeps each new selection in view.
    pub fn track_scroll(mut self, scroll_handle: &UniformListScrollHandle) -> Self {
        self.scroll_handle = Some(scroll_handle.clone());
        self
    }

    /// Called with the row up or down selects.
    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Called with the selected row when Enter is pressed.
    pub fn on_confirm(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_confirm = Some(Rc::new(handler));
        self
    }

    /// Called with the selected row when space is pressed.
    pub fn on_toggle(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }
}

impl ParentElement for NavigableList {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for NavigableList {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for NavigableList {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (len, selected) = (self.len, self.selected);
        let select = {
            let on_select = self.on_select.clone();
            let scroll = self.scroll_handle.clone();
            move |index: Option<usize>, window: &mut Window, cx: &mut App| {
                let (Some(index), Some(on_select)) = (index, &on_select) else {
                    return;
                };
                if let Some(scroll) = &scroll {
                    scroll.scroll_to_item(index, ScrollStrategy::Nearest);
                }
                on_select(index, window, cx);
                cx.stop_propagation();
            }
        };
        let select_down = select.clone();
        let focus = self.focus_handle.clone();
        let mut list = div()
            .id(self.id)
            .key_context(NAVIGABLE_LIST_CONTEXT)
            .track_focus(&self.focus_handle)
            // Rows and their controls stop their own clicks, so take focus first.
            .capture_any_mouse_down(move |event, window, cx| {
                if event.button == gpui::MouseButton::Left {
                    window.focus(&focus, cx);
                }
            })
            .on_action(move |_: &SelectDown, window, cx| {
                select_down(Navigable::next_index(selected, len, false), window, cx)
            })
            .on_action(move |_: &SelectUp, window, cx| {
                select(Navigable::previous_index(selected, len, false), window, cx)
            })
            .when_some(self.on_confirm.zip(selected), |list, (confirm, index)| {
                list.on_action(move |_: &Confirm, window, cx| {
                    confirm(index, window, cx);
                    cx.stop_propagation();
                })
            })
            .when_some(self.on_toggle.zip(selected), |list, (toggle, index)| {
                list.on_action(move |_: &ToggleSelected, window, cx| {
                    toggle(index, window, cx);
                    cx.stop_propagation();
                })
            })
            .children(self.children);
        list.style().refine(&self.style);
        list
    }
}

#[derive(Clone)]
pub struct NavigableEntry {
    pub focus_handle: FocusHandle,
    pub scroll_anchor: Option<ScrollAnchor>,
}

impl NavigableEntry {
    pub fn new(scroll_handle: &ScrollHandle, cx: &App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            scroll_anchor: Some(ScrollAnchor::for_handle(scroll_handle.clone())),
        }
    }

    pub fn focusable(cx: &App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            scroll_anchor: None,
        }
    }

    pub fn from_focus_handle(focus_handle: FocusHandle) -> Self {
        Self {
            focus_handle,
            scroll_anchor: None,
        }
    }

    pub fn with_scroll_anchor(mut self, scroll_anchor: ScrollAnchor) -> Self {
        self.scroll_anchor = Some(scroll_anchor);
        self
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle.focus(window, cx);
        if let Some(anchor) = &self.scroll_anchor {
            anchor.scroll_to(window, cx);
        }
    }
}

#[derive(IntoElement)]
pub struct Navigable {
    child: AnyElement,
    entries: Vec<NavigableEntry>,
    wrap: bool,
    key_context: Option<&'static str>,
}

impl Navigable {
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            entries: Vec::new(),
            wrap: true,
            key_context: Some(NAVIGABLE_CONTEXT),
        }
    }

    pub fn entry(mut self, entry: NavigableEntry) -> Self {
        self.entries.push(entry);
        self
    }

    pub fn entries(mut self, entries: impl IntoIterator<Item = NavigableEntry>) -> Self {
        self.entries.extend(entries);
        self
    }

    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    pub fn key_context(mut self, key_context: &'static str) -> Self {
        self.key_context = Some(key_context);
        self
    }

    pub fn without_key_context(mut self) -> Self {
        self.key_context = None;
        self
    }

    fn focused_index(
        entries: &[NavigableEntry],
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        entries
            .iter()
            .position(|entry| entry.focus_handle.contains_focused(window, cx))
    }

    fn next_index(current: Option<usize>, len: usize, wrap: bool) -> Option<usize> {
        match (current, len) {
            (_, 0) => None,
            (None, _) => Some(0),
            (Some(index), _) if index + 1 < len => Some(index + 1),
            (Some(_), _) if wrap => Some(0),
            (Some(index), _) => Some(index),
        }
    }

    fn previous_index(current: Option<usize>, len: usize, wrap: bool) -> Option<usize> {
        match (current, len) {
            (_, 0) => None,
            (None, _) => len.checked_sub(1),
            (Some(index), _) if index > 0 => Some(index - 1),
            (Some(_), _) if wrap => len.checked_sub(1),
            (Some(index), _) => Some(index),
        }
    }
}

impl RenderOnce for Navigable {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let entries_for_down = self.entries.clone();
        let entries_for_up = self.entries.clone();
        let wrap_down = self.wrap;
        let wrap_up = self.wrap;

        div()
            .when_some(self.key_context, |this, key_context| {
                this.key_context(key_context)
            })
            .on_action(move |_: &SelectDown, window, cx| {
                let Some(index) = Self::next_index(
                    Self::focused_index(&entries_for_down, window, cx),
                    entries_for_down.len(),
                    wrap_down,
                ) else {
                    cx.propagate();
                    return;
                };
                if let Some(entry) = entries_for_down.get(index) {
                    entry.focus(window, cx);
                    cx.stop_propagation();
                }
            })
            .on_action(move |_: &SelectUp, window, cx| {
                let Some(index) = Self::previous_index(
                    Self::focused_index(&entries_for_up, window, cx),
                    entries_for_up.len(),
                    wrap_up,
                ) else {
                    cx.propagate();
                    return;
                };
                if let Some(entry) = entries_for_up.get(index) {
                    entry.focus(window, cx);
                    cx.stop_propagation();
                }
            })
            .size_full()
            .child(self.child)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        Context, InteractiveElement as _, ParentElement as _, Render, Styled as _, TestAppContext,
        Window, div, px,
    };

    use super::*;

    struct NavigableHost {
        entries: Vec<NavigableEntry>,
    }

    impl NavigableHost {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                entries: vec![
                    NavigableEntry::from_focus_handle(cx.focus_handle()),
                    NavigableEntry::from_focus_handle(cx.focus_handle()),
                    NavigableEntry::from_focus_handle(cx.focus_handle()),
                ],
            }
        }
    }

    impl Render for NavigableHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            Navigable::new(
                div()
                    .size_full()
                    .children(self.entries.iter().enumerate().map(|(index, entry)| {
                        div()
                            .id(("navigable-entry", index))
                            .track_focus(&entry.focus_handle)
                            .h(px(20.0))
                    })),
            )
            .entries(self.entries.clone())
        }
    }

    #[gpui::test]
    fn default_key_context_maps_down_to_next_entry(cx: &mut TestAppContext) {
        cx.update(init);
        let (host, cx) = cx.add_window_view(|_, cx| NavigableHost::new(cx));
        let entries = host.read_with(cx, |host, _| host.entries.clone());

        cx.update(|window, cx| {
            entries[0].focus_handle.focus(window, cx);
            window.dispatch_keystroke(gpui::Keystroke::parse("down").unwrap(), cx);
        });

        cx.update(|window, _cx| {
            assert!(entries[1].focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    fn default_key_context_maps_ctrl_p_to_previous_entry(cx: &mut TestAppContext) {
        cx.update(init);
        let (host, cx) = cx.add_window_view(|_, cx| NavigableHost::new(cx));
        let entries = host.read_with(cx, |host, _| host.entries.clone());

        cx.update(|window, cx| {
            entries[1].focus_handle.focus(window, cx);
            window.dispatch_keystroke(gpui::Keystroke::parse("ctrl-p").unwrap(), cx);
        });

        cx.update(|window, _cx| {
            assert!(entries[0].focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    fn select_down_focuses_the_next_entry(cx: &mut TestAppContext) {
        let (host, cx) = cx.add_window_view(|_, cx| NavigableHost::new(cx));
        let entries = host.read_with(cx, |host, _| host.entries.clone());

        cx.update(|window, cx| {
            entries[0].focus_handle.focus(window, cx);
            entries[0]
                .focus_handle
                .dispatch_action(&SelectDown, window, cx);
        });

        cx.update(|window, _cx| {
            assert!(entries[1].focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    fn select_up_wraps_to_the_last_entry(cx: &mut TestAppContext) {
        let (host, cx) = cx.add_window_view(|_, cx| NavigableHost::new(cx));
        let entries = host.read_with(cx, |host, _| host.entries.clone());

        cx.update(|window, cx| {
            entries[0].focus_handle.focus(window, cx);
            entries[0]
                .focus_handle
                .dispatch_action(&SelectUp, window, cx);
        });

        cx.update(|window, _cx| {
            assert!(entries[2].focus_handle.is_focused(window));
        });
    }

    struct ListHost {
        focus: FocusHandle,
        scroll: UniformListScrollHandle,
        selected: Option<usize>,
        confirmed: Vec<usize>,
        toggled: Vec<usize>,
    }

    impl Render for ListHost {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let entity = cx.entity();
            let (select, confirm, toggle) = (entity.clone(), entity.clone(), entity);
            NavigableList::new("rows", &self.focus, 30)
                .selected(self.selected)
                .track_scroll(&self.scroll)
                .on_select(move |index, _, cx| {
                    select.update(cx, |host, cx| {
                        host.selected = Some(index);
                        cx.notify();
                    })
                })
                .on_confirm(move |index, _, cx| confirm.update(cx, |h, _| h.confirmed.push(index)))
                .on_toggle(move |index, _, cx| toggle.update(cx, |h, _| h.toggled.push(index)))
                .h(px(100.0))
                .child(
                    gpui::uniform_list("rows", 30, |range, _, _| {
                        range.map(|_| div().h(px(20.0))).collect::<Vec<_>>()
                    })
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
        }
    }

    #[gpui::test]
    fn list_keys_move_the_selection_into_view_and_act_on_it(cx: &mut TestAppContext) {
        use crate::scrollbar::ScrollableHandle as _;
        cx.update(init);
        let (host, cx) = cx.add_window_view(|_, cx| ListHost {
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            selected: None,
            confirmed: Vec::new(),
            toggled: Vec::new(),
        });
        let focus = host.read_with(cx, |host, _| host.focus.clone());
        cx.update(|window, cx| focus.focus(window, cx));
        // Without a selection, space and Enter have no row to act on.
        cx.simulate_keystrokes("space enter");
        host.read_with(cx, |host, _| {
            assert!(host.toggled.is_empty() && host.confirmed.is_empty())
        });

        // The first press selects the first row; row 7 then lies below the 100px viewport.
        cx.simulate_keystrokes("down j down down down down down down");
        cx.run_until_parked();
        let (selected, offset) =
            host.read_with(cx, |host, _| (host.selected, host.scroll.offset().y));
        assert_eq!(selected, Some(7));
        // Row 7 spans 140..160px, so at least 60px must have scrolled off the top.
        assert!(offset <= px(-60.0), "row 7 is out of view at {offset:?}");

        cx.simulate_keystrokes("k space enter");
        host.read_with(cx, |host, _| {
            assert_eq!(host.selected, Some(6));
            assert_eq!(host.toggled, vec![6]);
            assert_eq!(host.confirmed, vec![6]);
        });

        // Selection stops at the ends rather than wrapping.
        for _ in 0..8 {
            cx.simulate_keystrokes("up");
        }
        host.read_with(cx, |host, _| assert_eq!(host.selected, Some(0)));
        assert_eq!(
            host.read_with(cx, |host, _| host.scroll.offset().y),
            px(0.0)
        );
    }

    #[test]
    fn non_wrapping_navigation_stays_at_edges() {
        assert_eq!(Navigable::next_index(Some(2), 3, false), Some(2));
        assert_eq!(Navigable::previous_index(Some(0), 3, false), Some(0));
    }
}
