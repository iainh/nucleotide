// ABOUTME: Native GPUI surface element for editor viewport input
// ABOUTME: Wraps editor content while owning scroll-wheel capture for the viewport

use std::{cell::Cell, rc::Rc};

use gpui::InteractiveElement as _;
use gpui::{
    AnyElement, App, Bounds, Component, EntityId, FocusHandle, Hsla, IntoElement, KeyDownEvent,
    Modifiers, MouseButton, ParentElement as _, Pixels, Point, RenderOnce, ScrollWheelEvent,
    Styled as _, Window, div, fill, point, px,
};

use crate::{
    EditorScrollbar, EditorScrollbarState, EditorViewport, LineLayoutCache, ViewportScrollUpdate,
    scrollbar::editor_scrollbar_width,
};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;
type PointerCallback = Rc<dyn Fn(EditorSurfacePointerEvent, &mut App)>;
type KeyDownCallback = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorSurfaceMetricSnapshot {
    pub line_height: Pixels,
    pub cell_width: Pixels,
}

#[derive(Clone)]
pub struct EditorSurfaceMetrics {
    current: Rc<Cell<EditorSurfaceMetricSnapshot>>,
    line_cache: LineLayoutCache,
}

impl EditorSurfaceMetrics {
    pub fn new(line_height: Pixels, cell_width: Pixels) -> Self {
        Self {
            current: Rc::new(Cell::new(EditorSurfaceMetricSnapshot {
                line_height,
                cell_width,
            })),
            line_cache: LineLayoutCache::new(),
        }
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
    scrollbar_state: EditorScrollbarState,
    child: AnyElement,
    focus: Option<FocusHandle>,
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
        scrollbar_state: EditorScrollbarState,
        child: impl IntoElement,
    ) -> Self {
        Self {
            view_entity_id,
            viewport,
            metrics,
            scrollbar_state,
            child: child.into_any_element(),
            focus: None,
            on_key_down: None,
            on_scroll: None,
            on_mouse_down: None,
            on_mouse_drag: None,
            on_mouse_up: None,
        }
    }

    pub fn track_focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    pub fn on_key_down(
        mut self,
        callback: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
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

    pub fn on_mouse_down(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) + 'static,
    ) -> Self {
        self.on_mouse_down = Some(Rc::new(callback));
        self
    }

    pub fn on_mouse_drag(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) + 'static,
    ) -> Self {
        self.on_mouse_drag = Some(Rc::new(callback));
        self
    }

    pub fn on_mouse_up(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) + 'static,
    ) -> Self {
        self.on_mouse_up = Some(Rc::new(callback));
        self
    }

    fn scrollbar(&self) -> EditorScrollbar {
        let mut scrollbar = EditorScrollbar::new(
            self.view_entity_id,
            self.viewport.clone(),
            self.scrollbar_state.clone(),
        );

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
        let scrollbar = if editor_scrollbar_width(&self.viewport) > px(0.0) {
            Some(self.scrollbar())
        } else {
            None
        };
        let content_bounds = Rc::new(Cell::new(None::<Bounds<Pixels>>));
        let mut content = div()
            .key_context("Editor")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_hidden()
            .child(self.child);

        if let Some(focus) = self.focus.clone() {
            content = content.track_focus(&focus);
        }

        if let Some(on_key_down) = self.on_key_down.clone() {
            content = content.on_key_down(move |event, window, cx| {
                on_key_down(event, window, cx);
                cx.stop_propagation();
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
            let delta = point(px(0.0), raw_delta.y);
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

        if let Some(on_mouse_down) = self.on_mouse_down.clone() {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;
            let content_bounds = Rc::clone(&content_bounds);

            content = content.on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                let Some(bounds) = content_bounds.get() else {
                    return;
                };
                if !bounds.contains(&event.position) {
                    return;
                }

                on_mouse_down(
                    Self::surface_event(metrics.clone(), bounds, event.position, event.modifiers),
                    cx,
                );

                cx.notify(view_entity_id);
                cx.stop_propagation();
            });
        }

        if let Some(on_mouse_drag) = self.on_mouse_drag.clone() {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;
            let content_bounds = Rc::clone(&content_bounds);

            content = content.on_mouse_move(move |event, _window, cx| {
                if !event.dragging() {
                    return;
                }
                let Some(bounds) = content_bounds.get() else {
                    return;
                };
                if !bounds.contains(&event.position) {
                    return;
                }

                on_mouse_drag(
                    Self::surface_event(metrics.clone(), bounds, event.position, event.modifiers),
                    cx,
                );

                cx.notify(view_entity_id);
                cx.stop_propagation();
            });
        }

        if let Some(on_mouse_up) = self.on_mouse_up.clone() {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;
            let content_bounds = Rc::clone(&content_bounds);

            content = content.on_mouse_up(MouseButton::Left, move |event, _window, cx| {
                let Some(bounds) = content_bounds.get() else {
                    return;
                };
                if !bounds.contains(&event.position) {
                    return;
                }

                on_mouse_up(
                    Self::surface_event(metrics.clone(), bounds, event.position, event.modifiers),
                    cx,
                );

                cx.notify(view_entity_id);
                cx.stop_propagation();
            });
        }

        let mut surface = div()
            .flex()
            .size_full()
            .on_children_prepainted({
                let content_bounds = Rc::clone(&content_bounds);
                move |bounds, _window, _cx| {
                    content_bounds.set(bounds.into_iter().next());
                }
            })
            .child(content);

        if let Some(scrollbar) = scrollbar {
            surface = surface.child(scrollbar);
        }

        surface
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        AppContext as _, Empty, Entity, EntityId, FocusHandle, IntoElement, Keystroke, MouseButton,
        Render, ScrollDelta, ScrollWheelEvent, Styled, TestAppContext, TouchPhase, Window, div,
        point, px, size,
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
                    div().size_full(),
                )
                .on_scroll({
                    let saw_scroll = Rc::clone(&saw_scroll);
                    move |_, _, _| saw_scroll.set(true)
                })
                .on_mouse_down({
                    let saw_down = Rc::clone(&saw_down);
                    move |_, _| saw_down.set(true)
                })
                .on_mouse_drag({
                    let saw_drag = Rc::clone(&saw_drag);
                    move |_, _| saw_drag.set(true)
                })
                .on_mouse_up({
                    let saw_up = Rc::clone(&saw_up);
                    move |_, _| saw_up.set(true)
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

    struct SurfaceKeyDispatchHost {
        view_entity_id: EntityId,
        viewport: EditorViewport,
        metrics: EditorSurfaceMetrics,
        scrollbar_state: EditorScrollbarState,
        focus: FocusHandle,
        saw_key: Rc<Cell<bool>>,
    }

    impl Render for SurfaceKeyDispatchHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            EditorSurface::new(
                self.view_entity_id,
                self.viewport.clone(),
                self.metrics.clone(),
                self.scrollbar_state.clone(),
                div().size_full(),
            )
            .track_focus(self.focus.clone())
            .on_key_down({
                let saw_key = Rc::clone(&self.saw_key);
                move |event, _, _| {
                    saw_key.set(event.keystroke.key == "a");
                }
            })
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
                })
            })
            .unwrap()
        });

        window
            .update(cx, |host, window, _cx| window.focus(&host.focus))
            .unwrap();

        cx.dispatch_keystroke(*window, Keystroke::parse("a").unwrap());

        assert!(saw_key.get());
    }
}
