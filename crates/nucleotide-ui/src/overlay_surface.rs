// ABOUTME: Shared overlay surface for centred prompt, picker, and manager panels
// ABOUTME: Owns light-dismiss, occlusion, and click containment for app overlays

use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent,
    ParentElement, Pixels, RenderOnce, Styled, Window, div, px,
};

use crate::actions::dialog::Cancel as CancelDialogAction;

type LightDismissHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;
type CancelHandler = Box<dyn Fn(&CancelDialogAction, &mut Window, &mut App) + 'static>;

pub const OVERLAY_SURFACE_CONTEXT: &str = "OverlaySurface";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelDialogAction,
        Some(OVERLAY_SURFACE_CONTEXT),
    )]);
}

/// Full-window overlay wrapper for one centred, top-aligned surface.
#[derive(IntoElement)]
pub struct OverlaySurface {
    children: Vec<AnyElement>,
    top: Pixels,
    key_context: Option<&'static str>,
    on_light_dismiss: Option<LightDismissHandler>,
    on_cancel: Option<CancelHandler>,
}

impl OverlaySurface {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            top: px(32.0),
            key_context: Some(OVERLAY_SURFACE_CONTEXT),
            on_light_dismiss: None,
            on_cancel: None,
        }
    }

    pub fn top(mut self, top: impl Into<Pixels>) -> Self {
        self.top = top.into();
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

    pub fn on_light_dismiss(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_light_dismiss = Some(Box::new(handler));
        self
    }

    pub fn on_cancel(
        mut self,
        handler: impl Fn(&CancelDialogAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_cancel = Some(Box::new(handler));
        self
    }
}

impl Default for OverlaySurface {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for OverlaySurface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for OverlaySurface {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut root = div().absolute().size_full().bottom_0().left_0().occlude();

        if let Some(key_context) = self.key_context {
            root = root.key_context(key_context);
        }

        if let Some(on_light_dismiss) = self.on_light_dismiss {
            root = root.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                on_light_dismiss(event, window, cx);
                cx.stop_propagation();
            });
        }

        if let Some(on_cancel) = self.on_cancel {
            root = root.on_action(move |event: &CancelDialogAction, window, cx| {
                on_cancel(event, window, cx);
                cx.stop_propagation();
            });
        }

        root.child(
            div()
                .flex()
                .size_full()
                .justify_center()
                .items_start()
                .pt(self.top)
                .child(
                    div()
                        .occlude()
                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .children(self.children),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        Context, FocusHandle, IntoElement, Modifiers, ParentElement as _, Render, TestAppContext,
        Window, div, point, px,
    };

    use super::*;

    struct OverlaySurfaceHarness {
        focus_handle: FocusHandle,
        cancel_count: Rc<Cell<usize>>,
        light_dismiss_count: Rc<Cell<usize>>,
        child_mouse_down_count: Rc<Cell<usize>>,
        ancestor_mouse_down_count: Rc<Cell<usize>>,
    }

    impl OverlaySurfaceHarness {
        fn new(cancel_count: Rc<Cell<usize>>, cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                cancel_count,
                light_dismiss_count: Rc::new(Cell::new(0)),
                child_mouse_down_count: Rc::new(Cell::new(0)),
                ancestor_mouse_down_count: Rc::new(Cell::new(0)),
            }
        }
    }

    impl Render for OverlaySurfaceHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let cancel_count = Rc::clone(&self.cancel_count);
            let light_dismiss_count = Rc::clone(&self.light_dismiss_count);
            let child_mouse_down_count = Rc::clone(&self.child_mouse_down_count);
            let ancestor_mouse_down_count = Rc::clone(&self.ancestor_mouse_down_count);

            div()
                .size_full()
                .on_any_mouse_down(move |_, _, _| {
                    ancestor_mouse_down_count.set(ancestor_mouse_down_count.get() + 1);
                })
                .child(
                    OverlaySurface::new()
                        .top(px(16.0))
                        .on_light_dismiss(move |_, _, _| {
                            light_dismiss_count.set(light_dismiss_count.get() + 1);
                        })
                        .on_cancel(move |_, _, _| {
                            cancel_count.set(cancel_count.get() + 1);
                        })
                        .child(
                            div()
                                .debug_selector(|| "overlay-content".into())
                                .track_focus(&self.focus_handle)
                                .w(px(200.0))
                                .h(px(80.0))
                                .on_any_mouse_down(move |_, _, _| {
                                    child_mouse_down_count.set(child_mouse_down_count.get() + 1);
                                })
                                .child("overlay"),
                        ),
                )
        }
    }

    #[gpui::test]
    fn light_dismiss_only_handles_left_clicks_outside_content(cx: &mut TestAppContext) {
        let (harness, cx) =
            cx.add_window_view(|_, cx| OverlaySurfaceHarness::new(Rc::new(Cell::new(0)), cx));
        let (dismiss_count, child_count, ancestor_count) = harness.read_with(cx, |harness, _| {
            (
                Rc::clone(&harness.light_dismiss_count),
                Rc::clone(&harness.child_mouse_down_count),
                Rc::clone(&harness.ancestor_mouse_down_count),
            )
        });
        cx.run_until_parked();
        let content = cx.debug_bounds("overlay-content").unwrap();
        cx.update(|window, _| {
            assert_eq!(content.size.width, px(200.0));
            assert_eq!(content.size.height, px(80.0));
            assert_eq!(content.top(), px(16.0));
            assert_eq!(content.center().x, window.viewport_size().width / 2.0);
        });

        for (index, button) in [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
            .into_iter()
            .enumerate()
        {
            cx.simulate_mouse_down(content.center(), button, Modifiers::default());
            cx.simulate_mouse_up(content.center(), button, Modifiers::default());
            assert_eq!(child_count.get(), index + 1);
            assert_eq!(dismiss_count.get(), 0);
            assert_eq!(ancestor_count.get(), 0);
        }

        let outside = [
            point(content.center().x, content.top() - px(1.0)),
            point(content.left() - px(1.0), content.center().y),
            point(content.right() + px(1.0), content.center().y),
            point(content.center().x, content.bottom() + px(1.0)),
        ];
        for (index, position) in outside.into_iter().enumerate() {
            cx.simulate_click(position, Modifiers::default());
            assert_eq!(dismiss_count.get(), index + 1, "backdrop at {position:?}");
            assert_eq!(child_count.get(), 3);
            assert_eq!(ancestor_count.get(), 0);
        }

        for button in [MouseButton::Right, MouseButton::Middle] {
            cx.simulate_mouse_down(outside[0], button, Modifiers::default());
            cx.simulate_mouse_up(outside[0], button, Modifiers::default());
            assert_eq!(dismiss_count.get(), outside.len());
        }
    }

    #[gpui::test]
    fn overlay_surface_renders_in_test_harness(cx: &mut TestAppContext) {
        let cancel_count = Rc::new(Cell::new(0));
        let (_harness, cx) =
            cx.add_window_view(|_, cx| OverlaySurfaceHarness::new(Rc::clone(&cancel_count), cx));

        cx.update(|window, _cx| {
            assert!(window.viewport_size().width > px(0.0));
        });
    }

    #[gpui::test]
    fn default_context_maps_escape_to_cancel(cx: &mut TestAppContext) {
        cx.update(init);
        let cancel_count = Rc::new(Cell::new(0));
        let (harness, cx) =
            cx.add_window_view(|_, cx| OverlaySurfaceHarness::new(Rc::clone(&cancel_count), cx));
        let focus_handle = harness.read_with(cx, |harness, _| harness.focus_handle.clone());

        cx.update(|window, cx| {
            window.focus(&focus_handle, cx);
            window.dispatch_keystroke(gpui::Keystroke::parse("escape").unwrap(), cx);
        });

        assert_eq!(cancel_count.get(), 1);
    }
}
