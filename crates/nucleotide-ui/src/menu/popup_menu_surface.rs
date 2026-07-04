// ABOUTME: Shared full-window surface for anchored popup menus
// ABOUTME: Owns light-dismiss, occlusion, click containment, and window snapping

use gpui::{
    Anchor, AnyElement, App, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, RenderOnce, Styled, Window, anchored, div, point, px,
};

type LightDismissHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

/// Full-window backdrop for one anchored popup menu.
#[derive(IntoElement)]
pub struct PopupMenuSurface {
    child: AnyElement,
    position: Point<Pixels>,
    anchor: Anchor,
    offset: Point<Pixels>,
    snap_margin: Pixels,
    on_light_dismiss: Option<LightDismissHandler>,
}

impl PopupMenuSurface {
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            position: point(px(0.0), px(0.0)),
            anchor: Anchor::TopLeft,
            offset: point(px(0.0), px(0.0)),
            snap_margin: px(8.0),
            on_light_dismiss: None,
        }
    }

    pub fn position(mut self, position: Point<Pixels>) -> Self {
        self.position = position;
        self
    }

    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn offset(mut self, offset: Point<Pixels>) -> Self {
        self.offset = offset;
        self
    }

    pub fn snap_margin(mut self, margin: impl Into<Pixels>) -> Self {
        self.snap_margin = margin.into();
        self
    }

    pub fn on_light_dismiss(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_light_dismiss = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for PopupMenuSurface {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut backdrop = div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .occlude()
            .on_mouse_move(|_, _, cx| cx.stop_propagation());

        if let Some(on_light_dismiss) = self.on_light_dismiss {
            backdrop = backdrop.on_any_mouse_down(move |event, window, cx| {
                if matches!(event.button, MouseButton::Left | MouseButton::Right) {
                    on_light_dismiss(event, window, cx);
                    cx.stop_propagation();
                }
            });
        }

        backdrop.child(
            anchored()
                .position(self.position)
                .anchor(self.anchor)
                .offset(self.offset)
                .snap_to_window_with_margin(self.snap_margin)
                .child(
                    div()
                        .occlude()
                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .child(self.child),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        Context, IntoElement, Modifiers, ParentElement as _, Render, TestAppContext, Window, div,
    };

    use super::*;

    struct PopupMenuSurfaceHarness {
        dismiss_count: Rc<Cell<usize>>,
    }

    impl PopupMenuSurfaceHarness {
        fn new(dismiss_count: Rc<Cell<usize>>) -> Self {
            Self { dismiss_count }
        }
    }

    impl Render for PopupMenuSurfaceHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let dismiss_count = Rc::clone(&self.dismiss_count);

            PopupMenuSurface::new(div().size(px(20.0)).child("menu"))
                .position(point(px(100.0), px(100.0)))
                .on_light_dismiss(move |_, _, _| {
                    dismiss_count.set(dismiss_count.get() + 1);
                })
        }
    }

    #[gpui::test]
    fn popup_menu_surface_light_dismisses_on_backdrop_click(cx: &mut TestAppContext) {
        let dismiss_count = Rc::new(Cell::new(0));
        let (_harness, cx) =
            cx.add_window_view(|_, _| PopupMenuSurfaceHarness::new(Rc::clone(&dismiss_count)));

        cx.run_until_parked();
        cx.simulate_click(point(px(1.0), px(1.0)), Modifiers::default());

        assert_eq!(dismiss_count.get(), 1);
    }

    #[gpui::test]
    fn popup_menu_surface_contains_child_clicks(cx: &mut TestAppContext) {
        let dismiss_count = Rc::new(Cell::new(0));
        let (_harness, cx) =
            cx.add_window_view(|_, _| PopupMenuSurfaceHarness::new(Rc::clone(&dismiss_count)));

        cx.run_until_parked();
        cx.simulate_click(point(px(105.0), px(105.0)), Modifiers::default());

        assert_eq!(dismiss_count.get(), 0);
    }
}
