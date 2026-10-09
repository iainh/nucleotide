// ABOUTME: Native GPUI surface element for editor viewport input
// ABOUTME: Wraps editor content while owning scroll-wheel capture for the viewport

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::InteractiveElement as _;
use gpui::{
    AnyElement, App, Bounds, Component, DispatchPhase, EntityId, FocusHandle, Hsla, IntoElement,
    KeyDownEvent, Modifiers, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels,
    Point, RenderOnce, ScrollWheelEvent, Styled as _, Window, canvas, div, fill, hsla, point, px,
};

use crate::{
    EditorScrollbar, EditorScrollbarMarker, EditorScrollbarState, EditorViewport, LineLayoutCache,
    ViewportScrollUpdate,
};
use nucleotide_types::scrollbar::SCROLLBAR_THICKNESS;

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;
type PointerCallback = Rc<dyn Fn(EditorSurfacePointerEvent, &mut App) -> bool>;
type KeyDownCallback = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App) -> bool>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorSurfaceMetricSnapshot {
    pub line_height: Pixels,
    pub cell_width: Pixels,
}

#[derive(Clone)]
pub struct EditorSurfaceMetrics {
    current: Rc<Cell<EditorSurfaceMetricSnapshot>>,
    line_cache: LineLayoutCache,
    drag: Rc<Cell<Option<EditorSurfacePointerEvent>>>,
    drag_tick: Rc<Cell<Option<Instant>>>,
    drag_tick_scheduled: Rc<Cell<bool>>,
}

impl EditorSurfaceMetrics {
    pub fn new(line_height: Pixels, cell_width: Pixels) -> Self {
        Self {
            current: Rc::new(Cell::new(EditorSurfaceMetricSnapshot {
                line_height,
                cell_width,
            })),
            line_cache: LineLayoutCache::new(),
            drag: Rc::default(),
            drag_tick: Rc::default(),
            drag_tick_scheduled: Rc::default(),
        }
    }

    pub fn clear_pointer_drag(&self) {
        self.drag.set(None);
        self.drag_tick.set(None);
    }

    pub fn set(&self, line_height: Pixels, cell_width: Pixels) {
        self.current.set(EditorSurfaceMetricSnapshot {
            line_height,
            cell_width,
        });
    }

    pub fn get(&self) -> EditorSurfaceMetricSnapshot {
        self.current.get()
    }

    pub fn line_cache(&self) -> LineLayoutCache {
        self.line_cache.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorSurfacePointerEvent {
    pub position: Point<Pixels>,
    pub modifiers: Modifiers,
    pub bounds: Bounds<Pixels>,
    pub line_height: Pixels,
    pub cell_width: Pixels,
}

pub struct EditorSurface {
    view_entity_id: EntityId,
    viewport: EditorViewport,
    metrics: EditorSurfaceMetrics,
    vertical_scrollbar_state: EditorScrollbarState,
    horizontal_scrollbar_state: EditorScrollbarState,
    child: AnyElement,
    focus: Option<FocusHandle>,
    scrollbar_thumb_color: Hsla,
    scrollbar_markers: Vec<EditorScrollbarMarker>,
    on_key_down: Option<KeyDownCallback>,
    on_scroll: Option<ScrollCallback>,
    on_mouse_down: Option<PointerCallback>,
    on_mouse_drag: Option<PointerCallback>,
    on_mouse_up: Option<PointerCallback>,
}

pub fn paint_editor_background(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    window.paint_quad(fill(bounds, color));
}

impl EditorSurface {
    pub fn new(
        view_entity_id: EntityId,
        viewport: EditorViewport,
        metrics: EditorSurfaceMetrics,
        vertical_scrollbar_state: EditorScrollbarState,
        horizontal_scrollbar_state: EditorScrollbarState,
        child: impl IntoElement,
    ) -> Self {
        Self {
            view_entity_id,
            viewport,
            metrics,
            vertical_scrollbar_state,
            horizontal_scrollbar_state,
            child: child.into_any_element(),
            focus: None,
            scrollbar_thumb_color: hsla(0.0, 0.0, 0.72, 1.0),
            scrollbar_markers: Vec::new(),
            on_key_down: None,
            on_scroll: None,
            on_mouse_down: None,
            on_mouse_drag: None,
            on_mouse_up: None,
        }
    }

    pub fn scrollbar_thumb_color(mut self, color: Hsla) -> Self {
        self.scrollbar_thumb_color = color;
        self
    }

    pub fn scrollbar_markers(mut self, markers: Vec<EditorScrollbarMarker>) -> Self {
        self.scrollbar_markers = markers;
        self
    }

    pub fn track_focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    pub fn on_key_down(
        mut self,
        callback: impl Fn(&KeyDownEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.on_key_down = Some(Rc::new(callback));
        self
    }

    pub fn on_scroll(
        mut self,
        callback: impl Fn(&EditorViewport, ViewportScrollUpdate, &mut App) + 'static,
    ) -> Self {
        self.on_scroll = Some(Rc::new(callback));
        self
    }

    /// Return true when the press starts a selection drag. Rejected presses
    /// (for example a gutter action or failed hit test) do not arm autoscroll.
    pub fn on_mouse_down(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) -> bool + 'static,
    ) -> Self {
        self.on_mouse_down = Some(Rc::new(callback));
        self
    }

    pub fn on_mouse_drag(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) -> bool + 'static,
    ) -> Self {
        self.on_mouse_drag = Some(Rc::new(callback));
        self
    }

    pub fn on_mouse_up(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) -> bool + 'static,
    ) -> Self {
        self.on_mouse_up = Some(Rc::new(callback));
        self
    }

    fn vertical_scrollbar(&self) -> EditorScrollbar {
        let mut scrollbar = EditorScrollbar::vertical(
            self.view_entity_id,
            self.viewport.clone(),
            self.vertical_scrollbar_state.clone(),
        )
        .with_thumb_color(self.scrollbar_thumb_color)
        .with_markers(self.scrollbar_markers.clone());

        if let Some(on_scroll) = self.on_scroll.clone() {
            scrollbar = scrollbar.on_scroll(move |viewport, update, cx| {
                on_scroll(viewport, update, cx);
            });
        }

        scrollbar
    }

    fn horizontal_scrollbar(&self) -> EditorScrollbar {
        let mut scrollbar = EditorScrollbar::horizontal(
            self.view_entity_id,
            self.viewport.clone(),
            self.horizontal_scrollbar_state.clone(),
        )
        .with_thumb_color(self.scrollbar_thumb_color);

        if let Some(on_scroll) = self.on_scroll.clone() {
            scrollbar = scrollbar.on_scroll(move |viewport, update, cx| {
                on_scroll(viewport, update, cx);
            });
        }

        scrollbar
    }
}

impl IntoElement for EditorSurface {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl EditorSurface {
    fn edge_velocity(event: EditorSurfacePointerEvent) -> Point<Pixels> {
        fn axis(position: Pixels, start: Pixels, end: Pixels, margin: Pixels) -> Pixels {
            let margin = margin.max(px(1.0)).min((end - start) / 2.0);
            if position < start + margin {
                ((start + margin - position) / margin).min(4.0) * margin * 20.0
            } else if position > end - margin {
                -((position - end + margin) / margin).min(4.0) * margin * 20.0
            } else {
                px(0.0)
            }
        }
        point(
            axis(
                event.position.x,
                event.bounds.left(),
                event.bounds.right(),
                event.cell_width * 2.0,
            ),
            axis(
                event.position.y,
                event.bounds.top(),
                event.bounds.bottom(),
                event.line_height,
            ),
        )
    }

    fn surface_event(
        metrics: EditorSurfaceMetrics,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> EditorSurfacePointerEvent {
        let metrics = metrics.get();
        EditorSurfacePointerEvent {
            position,
            modifiers,
            bounds,
            line_height: metrics.line_height,
            cell_width: metrics.cell_width,
        }
    }
}

impl RenderOnce for EditorSurface {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let vertical_scrollbar = self.vertical_scrollbar();
        let horizontal_scrollbar = self.horizontal_scrollbar();
        let content_bounds = Rc::new(Cell::new(None::<Bounds<Pixels>>));
        let mut content = div()
            .key_context("Editor")
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .child(self.child);

        let focus = self.focus.clone();
        if let Some(focus) = focus.clone() {
            content = content.track_focus(&focus);
        }

        if let Some(on_key_down) = self.on_key_down.clone() {
            content = content.on_key_down(move |event, window, cx| {
                if on_key_down(event, window, cx) {
                    cx.stop_propagation();
                }
            });
        }

        let viewport = self.viewport.clone();
        let metrics = self.metrics.clone();
        let view_entity_id = self.view_entity_id;
        let scroll_content_bounds = Rc::clone(&content_bounds);
        let on_scroll = self.on_scroll.clone();

        content = content.on_scroll_wheel(move |event: &ScrollWheelEvent, _window, cx| {
            let Some(bounds) = scroll_content_bounds.get() else {
                return;
            };
            if !bounds.contains(&event.position) {
                return;
            }

            let line_height = metrics.get().line_height;
            let raw_delta = event.delta.pixel_delta(line_height);
            let delta = point(raw_delta.x, raw_delta.y);
            let scroll_update = viewport.scroll_by_delta(delta);

            if !scroll_update.changed {
                return;
            }

            if let Some(on_scroll) = &on_scroll {
                on_scroll(&viewport, scroll_update, cx);
            }

            cx.notify(view_entity_id);
            cx.stop_propagation();
        });

        if self.on_mouse_down.is_some()
            || self.on_mouse_drag.is_some()
            || self.on_mouse_up.is_some()
        {
            let on_mouse_down = self.on_mouse_down.clone();
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;
            let content_bounds = Rc::clone(&content_bounds);
            let focus = focus.clone();

            content = content.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let Some(bounds) = content_bounds.get() else {
                    return;
                };
                if !bounds.contains(&event.position) {
                    return;
                }

                if let Some(focus) = &focus {
                    focus.focus(window, cx);
                }

                let accepted = on_mouse_down.as_ref().is_none_or(|on_mouse_down| {
                    on_mouse_down(
                        Self::surface_event(
                            metrics.clone(),
                            bounds,
                            event.position,
                            event.modifiers,
                        ),
                        cx,
                    )
                });

                if accepted {
                    metrics.drag.set(Some(Self::surface_event(
                        metrics.clone(),
                        bounds,
                        event.position,
                        event.modifiers,
                    )));
                    metrics.drag_tick.set(None);
                    cx.notify(view_entity_id);
                }
                cx.stop_propagation();
            });
        }

        // Div mouse-move handlers are hover-only. Register window listeners in
        // paint instead, gated by this pane's persisted left-button origin.
        let metrics = self.metrics.clone();
        let viewport = self.viewport.clone();
        let on_drag = self.on_mouse_drag.clone();
        let on_up = self.on_mouse_up.clone();
        let on_scroll = self.on_scroll.clone();
        let drag_bounds = content_bounds.clone();
        content = content.child(
            canvas(
                |_, _, _| (),
                move |_, _, window, cx| {
                    let Some(bounds) = drag_bounds.get() else {
                        return;
                    };
                    if let Some(mut event) = metrics.drag.get() {
                        event.bounds = bounds;
                        let snapshot = metrics.get();
                        event.line_height = snapshot.line_height;
                        event.cell_width = snapshot.cell_width;
                        metrics.drag.set(Some(event));
                        if metrics.drag_tick.get().is_some()
                            && Self::edge_velocity(event) != point(px(0.0), px(0.0))
                            && !metrics.drag_tick_scheduled.replace(true)
                        {
                            let metrics = metrics.clone();
                            let viewport = viewport.clone();
                            let on_drag = on_drag.clone();
                            let on_scroll = on_scroll.clone();
                            let focus = focus.clone();
                            let timer = cx.background_executor().timer(Duration::from_millis(16));
                            window
                                .spawn(cx, async move |cx| {
                                    timer.await;
                                    let _ = cx.update(move |window, cx| {
                                        metrics.drag_tick_scheduled.set(false);
                                        if !window.is_window_active()
                                            || focus
                                                .as_ref()
                                                .is_some_and(|focus| !focus.is_focused(window))
                                        {
                                            metrics.clear_pointer_drag();
                                            return;
                                        }
                                        let Some(event) = metrics.drag.get() else {
                                            return;
                                        };
                                        // Hit-test the frame just painted, then scroll for the next
                                        // frame. Never combine a new offset with an old line cache.
                                        if let Some(on_drag) = &on_drag {
                                            on_drag(event, cx);
                                        }
                                        let now = cx.background_executor().now();
                                        let elapsed = metrics
                                            .drag_tick
                                            .replace(Some(now))
                                            .map_or(0.0, |last| {
                                                now.duration_since(last).as_secs_f32().min(0.05)
                                            });
                                        let update = viewport
                                            .scroll_by_delta(Self::edge_velocity(event) * elapsed);
                                        if update.changed {
                                            if let Some(on_scroll) = &on_scroll {
                                                on_scroll(&viewport, update, cx);
                                            }
                                            cx.notify(view_entity_id);
                                        }
                                    });
                                })
                                .detach();
                        } else if metrics.drag_tick.get().is_some()
                            && Self::edge_velocity(event) == point(px(0.0), px(0.0))
                        {
                            metrics.drag_tick.set(Some(cx.background_executor().now()));
                        }
                    }
                    let move_metrics = metrics.clone();
                    let on_drag_move = on_drag.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                        if phase != DispatchPhase::Capture || move_metrics.drag.get().is_none() {
                            return;
                        }
                        if !event.dragging() {
                            move_metrics.clear_pointer_drag();
                            return;
                        }
                        let event = Self::surface_event(
                            move_metrics.clone(),
                            bounds,
                            event.position,
                            event.modifiers,
                        );
                        move_metrics.drag.set(Some(event));
                        if move_metrics.drag_tick.get().is_none() {
                            move_metrics
                                .drag_tick
                                .set(Some(cx.background_executor().now()));
                        }
                        if let Some(on_drag) = &on_drag_move {
                            on_drag(event, cx);
                        }
                        cx.notify(view_entity_id);
                        cx.stop_propagation();
                    });
                    let up_metrics = metrics.clone();
                    let on_drag_up = on_drag.clone();
                    let on_up = on_up.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                        if phase != DispatchPhase::Capture
                            || event.button != MouseButton::Left
                            || up_metrics.drag.get().is_none()
                        {
                            return;
                        }
                        let event = Self::surface_event(
                            up_metrics.clone(),
                            bounds,
                            event.position,
                            event.modifiers,
                        );
                        if (up_metrics.drag_tick.get().is_some()
                            || up_metrics
                                .drag
                                .get()
                                .is_some_and(|last| last.position != event.position))
                            && let Some(on_drag) = &on_drag_up
                        {
                            on_drag(event, cx);
                        }
                        up_metrics.clear_pointer_drag();
                        if let Some(on_up) = &on_up {
                            on_up(event, cx);
                        }
                        cx.notify(view_entity_id);
                        cx.stop_propagation();
                    });
                },
            )
            .absolute()
            .size_full(),
        );

        let mut surface = div()
            .relative()
            .size_full()
            .on_children_prepainted({
                let content_bounds = Rc::clone(&content_bounds);
                move |bounds, _window, _cx| {
                    content_bounds.set(bounds.into_iter().next());
                }
            })
            .child(content);

        surface = surface.child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(SCROLLBAR_THICKNESS)
                .child(vertical_scrollbar),
        );

        surface = surface.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(SCROLLBAR_THICKNESS)
                .child(horizontal_scrollbar),
        );

        surface
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        AppContext as _, Empty, Entity, EntityId, FocusHandle, InteractiveElement as _,
        IntoElement, Keystroke, MouseButton, ParentElement as _, Render, ScrollDelta,
        ScrollWheelEvent, Styled, TestAppContext, TouchPhase, Window, div, point, px, size,
    };

    use super::{EditorSurface, EditorSurfaceMetrics};
    use crate::{EditorScrollbarState, EditorViewport, LineLayout};

    #[test]
    fn shared_surface_metrics_reflect_updates_across_clones() {
        let metrics = EditorSurfaceMetrics::new(px(20.0), px(8.0));
        let clone = metrics.clone();

        metrics.set(px(24.0), px(9.0));

        let snapshot = clone.get();
        assert_eq!(snapshot.line_height, px(24.0));
        assert_eq!(snapshot.cell_width, px(9.0));
    }

    #[test]
    fn shared_surface_metrics_share_line_cache() {
        let metrics = EditorSurfaceMetrics::new(px(20.0), px(8.0));
        let clone = metrics.clone();

        metrics.line_cache().clear();
        metrics
            .line_cache()
            .push(LineLayout::unwrapped(7, Default::default(), px(12.0)));

        assert!(clone.line_cache().find_line_by_index(7).is_some());
    }

    #[gpui::test]
    fn editor_surface_draws_and_dispatches_input(cx: &mut TestAppContext) {
        let view_entity_id = cx.update(|cx| {
            let entity: Entity<Empty> = cx.new(|_| Empty);
            entity.entity_id()
        });

        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(200.0)), 50);
        let metrics = EditorSurfaceMetrics::new(px(20.0), px(8.0));
        let scrollbar_state = EditorScrollbarState::default();
        let saw_scroll = Rc::new(Cell::new(false));
        let saw_down = Rc::new(Cell::new(false));
        let saw_drag = Rc::new(Cell::new(false));
        let saw_up = Rc::new(Cell::new(false));

        let window = cx.add_empty_window();
        window.draw(
            point(px(0.0), px(0.0)),
            size(px(112.0), px(200.0)),
            |_, _| {
                EditorSurface::new(
                    view_entity_id,
                    viewport.clone(),
                    metrics.clone(),
                    scrollbar_state.clone(),
                    EditorScrollbarState::default(),
                    div().size_full(),
                )
                .on_scroll({
                    let saw_scroll = Rc::clone(&saw_scroll);
                    move |_, _, _| saw_scroll.set(true)
                })
                .on_mouse_down({
                    let saw_down = Rc::clone(&saw_down);
                    move |_, _| {
                        saw_down.set(true);
                        true
                    }
                })
                .on_mouse_drag({
                    let saw_drag = Rc::clone(&saw_drag);
                    move |_, _| {
                        saw_drag.set(true);
                        true
                    }
                })
                .on_mouse_up({
                    let saw_up = Rc::clone(&saw_up);
                    move |_, _| {
                        saw_up.set(true);
                        true
                    }
                })
                .into_element()
            },
        );

        window.simulate_event(ScrollWheelEvent {
            position: point(px(10.0), px(10.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        window.simulate_mouse_down(
            point(px(10.0), px(10.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        window.simulate_mouse_move(
            point(px(10.0), px(30.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        window.simulate_mouse_up(
            point(px(10.0), px(30.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );

        assert!(saw_scroll.get());
        assert!(saw_down.get());
        assert!(saw_drag.get());
        assert!(saw_up.get());
        assert!(viewport.scroll_position().y > px(0.0));
    }

    #[gpui::test]
    fn editor_surface_dispatches_mouse_up_outside_bounds(cx: &mut TestAppContext) {
        let view_entity_id = cx.update(|cx| {
            let entity: Entity<Empty> = cx.new(|_| Empty);
            entity.entity_id()
        });

        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(200.0)), 50);
        let metrics = EditorSurfaceMetrics::new(px(20.0), px(8.0));
        let scrollbar_state = EditorScrollbarState::default();
        let saw_up = Rc::new(Cell::new(false));

        let window = cx.add_empty_window();
        window.draw(
            point(px(0.0), px(0.0)),
            size(px(220.0), px(200.0)),
            |_, _| {
                div().w(px(112.0)).h(px(200.0)).child(
                    EditorSurface::new(
                        view_entity_id,
                        viewport.clone(),
                        metrics.clone(),
                        scrollbar_state.clone(),
                        EditorScrollbarState::default(),
                        div().size_full(),
                    )
                    .on_mouse_up({
                        let saw_up = Rc::clone(&saw_up);
                        move |_, _| {
                            saw_up.set(true);
                            true
                        }
                    }),
                )
            },
        );

        window.simulate_mouse_down(
            point(px(10.0), px(10.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        window.simulate_mouse_up(
            point(px(150.0), px(30.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );

        assert!(saw_up.get());
    }

    #[gpui::test]
    fn editor_surface_scrolls_without_observer_callback(cx: &mut TestAppContext) {
        let view_entity_id = cx.update(|cx| {
            let entity: Entity<Empty> = cx.new(|_| Empty);
            entity.entity_id()
        });

        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(200.0)), 50);
        let metrics = EditorSurfaceMetrics::new(px(20.0), px(8.0));
        let scrollbar_state = EditorScrollbarState::default();

        let window = cx.add_empty_window();
        window.draw(
            point(px(0.0), px(0.0)),
            size(px(112.0), px(200.0)),
            |_, _| {
                EditorSurface::new(
                    view_entity_id,
                    viewport.clone(),
                    metrics.clone(),
                    scrollbar_state.clone(),
                    EditorScrollbarState::default(),
                    div().size_full(),
                )
                .into_element()
            },
        );

        window.simulate_event(ScrollWheelEvent {
            position: point(px(10.0), px(10.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });

        assert!(viewport.scroll_position().y > px(0.0));
    }

    struct SelectionDragHost {
        viewport: [EditorViewport; 2],
        metrics: [EditorSurfaceMetrics; 2],
        drags: [Rc<Cell<usize>>; 2],
    }

    impl Render for SelectionDragHost {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            let mut row = div().flex().gap(px(20.0));
            for index in 0..2 {
                let drags = self.drags[index].clone();
                row = row.child(
                    div().w(px(200.0)).h(px(120.0)).child(
                        EditorSurface::new(
                            cx.entity_id(),
                            self.viewport[index].clone(),
                            self.metrics[index].clone(),
                            EditorScrollbarState::default(),
                            EditorScrollbarState::default(),
                            div().size_full(),
                        )
                        .on_mouse_down(|_, _| true)
                        .on_mouse_drag(move |_, _| {
                            drags.set(drags.get() + 1);
                            true
                        }),
                    ),
                );
            }
            row
        }
    }

    #[gpui::test]
    fn selection_drag_owns_origin_and_scrolls_while_stationary(cx: &mut TestAppContext) {
        use std::time::Duration;
        let (host, cx) = cx.add_window_view(|window, _cx| {
            window.activate_window();
            let viewport = std::array::from_fn(|_| {
                let mut viewport = EditorViewport::new(px(20.0));
                viewport.set_layout(px(20.0), size(px(200.0), px(120.0)), 100);
                viewport.set_content_width(px(2000.0));
                viewport
            });
            SelectionDragHost {
                viewport,
                metrics: std::array::from_fn(|_| EditorSurfaceMetrics::new(px(20.0), px(8.0))),
                drags: std::array::from_fn(|_| Rc::new(Cell::new(0))),
            }
        });
        let (viewport, drags) = cx.update(|_, cx| {
            let host = host.read(cx);
            (host.viewport.clone(), host.drags.clone())
        });
        let none = gpui::Modifiers::none();
        // A pressed button entering from elsewhere, and a right-button drag,
        // must not arm either editor's selection or scrolling.
        cx.simulate_mouse_down(point(px(500.0), px(200.0)), MouseButton::Left, none);
        cx.simulate_mouse_move(point(px(190.0), px(115.0)), MouseButton::Left, none);
        cx.simulate_mouse_up(point(px(190.0), px(115.0)), MouseButton::Left, none);
        cx.simulate_mouse_down(point(px(50.0), px(40.0)), MouseButton::Right, none);
        cx.simulate_mouse_move(point(px(190.0), px(115.0)), MouseButton::Right, none);
        cx.simulate_mouse_up(point(px(190.0), px(115.0)), MouseButton::Right, none);
        assert_eq!(drags[0].get(), 0);
        assert_eq!(viewport[0].scroll_position(), point(px(0.0), px(0.0)));

        cx.simulate_mouse_down(point(px(50.0), px(40.0)), MouseButton::Left, none);
        // Cross the originating pane's bounds into the second pane.
        cx.simulate_mouse_move(point(px(260.0), px(115.0)), MouseButton::Left, none);
        assert!(drags[0].get() > 0);
        assert_eq!(drags[1].get(), 0);
        for _ in 0..3 {
            cx.executor().advance_clock(Duration::from_millis(20));
            cx.run_until_parked();
        }
        let first = viewport[0].scroll_position();
        assert!(first.x > px(0.0) && first.y > px(0.0));
        for _ in 0..3 {
            cx.executor().advance_clock(Duration::from_millis(20));
            cx.run_until_parked();
        }
        let later = viewport[0].scroll_position();
        assert!(
            later.x > first.x && later.y > first.y,
            "stationary pointer must keep scrolling"
        );
        assert_eq!(viewport[1].scroll_position(), point(px(0.0), px(0.0)));
        assert_eq!(drags[1].get(), 0);

        // Re-entering the centre pauses scrolling without relinquishing the drag.
        cx.simulate_mouse_move(point(px(100.0), px(60.0)), MouseButton::Left, none);
        let centre = viewport[0].scroll_position();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(viewport[0].scroll_position(), centre);
        // Top/left overshoot reverses both axes.
        cx.simulate_mouse_move(point(px(-10.0), px(-10.0)), MouseButton::Left, none);
        cx.executor().advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        let reversed = viewport[0].scroll_position();
        assert!(reversed.x < centre.x && reversed.y < centre.y);
        cx.simulate_mouse_up(point(px(260.0), px(150.0)), MouseButton::Left, none);
        let released = viewport[0].scroll_position();
        let count = drags[0].get();
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        cx.simulate_mouse_move(point(px(190.0), px(115.0)), MouseButton::Left, none);
        assert_eq!(viewport[0].scroll_position(), released);
        assert_eq!(drags[0].get(), count);

        cx.simulate_mouse_down(point(px(50.0), px(40.0)), MouseButton::Left, none);
        cx.simulate_mouse_move(point(px(190.0), px(115.0)), MouseButton::Left, none);
        cx.deactivate_window();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(
            viewport[0].scroll_position(),
            released,
            "window blur stops scrolling"
        );
    }

    struct SurfacePointerFocusHost {
        view_entity_id: EntityId,
        focus: FocusHandle,
        saw_down: Rc<Cell<bool>>,
    }

    impl Render for SurfacePointerFocusHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let mut viewport = EditorViewport::new(px(20.0));
            viewport.set_layout(px(20.0), size(px(100.0), px(200.0)), 50);
            let saw_down = Rc::clone(&self.saw_down);

            EditorSurface::new(
                self.view_entity_id,
                viewport,
                EditorSurfaceMetrics::new(px(20.0), px(8.0)),
                EditorScrollbarState::default(),
                EditorScrollbarState::default(),
                div().size_full(),
            )
            .track_focus(self.focus.clone())
            .on_mouse_down(move |_, _| {
                saw_down.set(true);
                true
            })
        }
    }

    #[gpui::test]
    fn editor_surface_focuses_on_mouse_down(cx: &mut TestAppContext) {
        let saw_down = Rc::new(Cell::new(false));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let saw_down = Rc::clone(&saw_down);
            SurfacePointerFocusHost {
                view_entity_id: cx.entity_id(),
                focus: cx.focus_handle(),
                saw_down,
            }
        });

        cx.update(|window, cx| {
            host.update(cx, |host, _cx| {
                assert!(!host.focus.is_focused(window));
            });
        });

        cx.simulate_mouse_down(
            point(px(10.0), px(10.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );

        cx.update(|window, cx| {
            host.update(cx, |host, _cx| {
                assert!(host.focus.is_focused(window));
            });
        });
        assert!(saw_down.get());
    }

    struct SurfaceKeyDispatchHost {
        view_entity_id: EntityId,
        viewport: EditorViewport,
        metrics: EditorSurfaceMetrics,
        scrollbar_state: EditorScrollbarState,
        focus: FocusHandle,
        saw_key: Rc<Cell<bool>>,
        saw_parent_key: Rc<Cell<bool>>,
        consume_key: bool,
    }

    impl Render for SurfaceKeyDispatchHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .on_key_down({
                    let saw_parent_key = Rc::clone(&self.saw_parent_key);
                    move |event, _, _| {
                        saw_parent_key.set(event.keystroke.key == "a");
                    }
                })
                .child(
                    EditorSurface::new(
                        self.view_entity_id,
                        self.viewport.clone(),
                        self.metrics.clone(),
                        self.scrollbar_state.clone(),
                        EditorScrollbarState::default(),
                        div().size_full(),
                    )
                    .track_focus(self.focus.clone())
                    .on_key_down({
                        let saw_key = Rc::clone(&self.saw_key);
                        let consume_key = self.consume_key;
                        move |event, _, _| {
                            saw_key.set(event.keystroke.key == "a");
                            consume_key
                        }
                    }),
                )
        }
    }

    #[gpui::test]
    fn editor_surface_dispatches_key_events_from_focus(cx: &mut TestAppContext) {
        let saw_key = Rc::new(Cell::new(false));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let mut viewport = EditorViewport::new(px(20.0));
                viewport.set_layout(px(20.0), size(px(100.0), px(200.0)), 50);
                let saw_key = Rc::clone(&saw_key);
                cx.new(|cx| SurfaceKeyDispatchHost {
                    view_entity_id: cx.entity_id(),
                    viewport,
                    metrics: EditorSurfaceMetrics::new(px(20.0), px(8.0)),
                    scrollbar_state: EditorScrollbarState::default(),
                    focus: cx.focus_handle(),
                    saw_key,
                    saw_parent_key: Rc::new(Cell::new(false)),
                    consume_key: true,
                })
            })
            .unwrap()
        });

        window
            .update(cx, |host, window, cx| window.focus(&host.focus, cx))
            .unwrap();

        cx.dispatch_keystroke(*window, Keystroke::parse("a").unwrap());

        assert!(saw_key.get());
    }

    #[gpui::test]
    fn editor_surface_allows_unconsumed_key_events_to_bubble(cx: &mut TestAppContext) {
        let saw_key = Rc::new(Cell::new(false));
        let saw_parent_key = Rc::new(Cell::new(false));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let mut viewport = EditorViewport::new(px(20.0));
                viewport.set_layout(px(20.0), size(px(100.0), px(200.0)), 50);
                let saw_key = Rc::clone(&saw_key);
                let saw_parent_key = Rc::clone(&saw_parent_key);
                cx.new(|cx| SurfaceKeyDispatchHost {
                    view_entity_id: cx.entity_id(),
                    viewport,
                    metrics: EditorSurfaceMetrics::new(px(20.0), px(8.0)),
                    scrollbar_state: EditorScrollbarState::default(),
                    focus: cx.focus_handle(),
                    saw_key,
                    saw_parent_key,
                    consume_key: false,
                })
            })
            .unwrap()
        });

        window
            .update(cx, |host, window, cx| window.focus(&host.focus, cx))
            .unwrap();

        cx.dispatch_keystroke(*window, Keystroke::parse("a").unwrap());

        assert!(saw_key.get());
        assert!(saw_parent_key.get());
    }
}
