// ABOUTME: Shared keyboard navigation wrapper for focusable list-like UI
// ABOUTME: Packages GPUI action-driven focus traversal for menus, panels, and pickers

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, FocusHandle, InteractiveElement, IntoElement, KeyBinding, ParentElement,
    RenderOnce, ScrollAnchor, ScrollHandle, Styled, Window, div,
};

use crate::actions::menu::{SelectDown, SelectUp};

pub const NAVIGABLE_CONTEXT: &str = "Navigable";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some(NAVIGABLE_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(NAVIGABLE_CONTEXT)),
        KeyBinding::new("ctrl-p", SelectUp, Some(NAVIGABLE_CONTEXT)),
        KeyBinding::new("ctrl-n", SelectDown, Some(NAVIGABLE_CONTEXT)),
    ]);
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

    #[test]
    fn non_wrapping_navigation_stays_at_edges() {
        assert_eq!(Navigable::next_index(Some(2), 3, false), Some(2));
        assert_eq!(Navigable::previous_index(Some(0), 3, false), Some(0));
    }
}
