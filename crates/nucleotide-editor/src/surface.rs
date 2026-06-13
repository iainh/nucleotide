// ABOUTME: Native GPUI surface element for editor viewport input
// ABOUTME: Wraps editor content while owning scroll-wheel capture for the viewport

use std::{cell::Cell, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, EntityId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Style, Window, fill, point, px,
    relative, size,
};

use crate::{
    EditorScrollbar, EditorScrollbarState, EditorViewport, ViewportScrollUpdate,
    scrollbar::editor_scrollbar_width,
};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;
type PointerCallback = Rc<dyn Fn(EditorSurfacePointerEvent, &mut App)>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorSurfaceMetricSnapshot {
    pub line_height: Pixels,
    pub cell_width: Pixels,
}

#[derive(Clone)]
pub struct EditorSurfaceMetrics {
    current: Rc<Cell<EditorSurfaceMetricSnapshot>>,
}

impl EditorSurfaceMetrics {
    pub fn new(line_height: Pixels, cell_width: Pixels) -> Self {
        Self {
            current: Rc::new(Cell::new(EditorSurfaceMetricSnapshot {
                line_height,
                cell_width,
            })),
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
            on_scroll: None,
            on_mouse_down: None,
            on_mouse_drag: None,
            on_mouse_up: None,
        }
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
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub struct EditorSurfacePrepaintState {
    content_bounds: Bounds<Pixels>,
    scrollbar: Option<AnyElement>,
}

impl Element for EditorSurface {
    type RequestLayoutState = ();
    type PrepaintState = EditorSurfacePrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let scrollbar_width = editor_scrollbar_width(&self.viewport).min(bounds.size.width);
        let content_width = (bounds.size.width - scrollbar_width).max(px(0.0));
        let content_bounds = Bounds::new(bounds.origin, size(content_width, bounds.size.height));

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.child
                .layout_as_root(content_bounds.size.into(), window, cx);
            self.child.prepaint_at(content_bounds.origin, window, cx);
        });

        let scrollbar = if scrollbar_width > px(0.0) {
            let scrollbar_bounds = Bounds::new(
                point(bounds.origin.x + content_width, bounds.origin.y),
                size(scrollbar_width, bounds.size.height),
            );
            let mut scrollbar = self.scrollbar().into_any_element();
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                scrollbar.layout_as_root(scrollbar_bounds.size.into(), window, cx);
                scrollbar.prepaint_at(scrollbar_bounds.origin, window, cx);
            });
            Some(scrollbar)
        } else {
            None
        };

        EditorSurfacePrepaintState {
            content_bounds,
            scrollbar,
        }
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_scroll_listener(prepaint.content_bounds, window);
        self.paint_pointer_listeners(prepaint.content_bounds, window);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.child.paint(window, cx);
            if let Some(scrollbar) = prepaint.scrollbar.as_mut() {
                scrollbar.paint(window, cx);
            }
        });
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

    fn paint_scroll_listener(&self, content_bounds: Bounds<Pixels>, window: &mut Window) {
        let Some(on_scroll) = self.on_scroll.clone() else {
            return;
        };

        let viewport = self.viewport.clone();
        let metrics = self.metrics.clone();
        let view_entity_id = self.view_entity_id;
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _window, cx| {
            if !phase.bubble() || !content_bounds.contains(&event.position) {
                return;
            }

            let line_height = metrics.get().line_height;
            let raw_delta = event.delta.pixel_delta(line_height);
            let delta = point(px(0.0), raw_delta.y);
            let scroll_update = viewport.scroll_by_delta(delta);

            if !scroll_update.changed {
                return;
            }

            on_scroll(&viewport, scroll_update, cx);

            cx.notify(view_entity_id);
            cx.stop_propagation();
        });
    }

    fn paint_pointer_listeners(&self, content_bounds: Bounds<Pixels>, window: &mut Window) {
        if let Some(on_mouse_down) = self.on_mouse_down.clone() {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;

            window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, cx| {
                if !phase.bubble()
                    || event.button != MouseButton::Left
                    || !content_bounds.contains(&event.position)
                {
                    return;
                }

                on_mouse_down(
                    Self::surface_event(
                        metrics.clone(),
                        content_bounds,
                        event.position,
                        event.modifiers,
                    ),
                    cx,
                );

                cx.notify(view_entity_id);
                cx.stop_propagation();
            });
        }

        if let Some(on_mouse_drag) = self.on_mouse_drag.clone() {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;

            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                if !phase.capture()
                    || !event.dragging()
                    || !content_bounds.contains(&event.position)
                {
                    return;
                }

                on_mouse_drag(
                    Self::surface_event(
                        metrics.clone(),
                        content_bounds,
                        event.position,
                        event.modifiers,
                    ),
                    cx,
                );

                cx.notify(view_entity_id);
                cx.stop_propagation();
            });
        }

        if let Some(on_mouse_up) = self.on_mouse_up.clone() {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;

            window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                if !phase.capture()
                    || event.button != MouseButton::Left
                    || !content_bounds.contains(&event.position)
                {
                    return;
                }

                on_mouse_up(
                    Self::surface_event(
                        metrics.clone(),
                        content_bounds,
                        event.position,
                        event.modifiers,
                    ),
                    cx,
                );

                cx.notify(view_entity_id);
                cx.stop_propagation();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        AppContext as _, Empty, Entity, IntoElement as _, MouseButton, ScrollDelta,
        ScrollWheelEvent, Styled, TestAppContext, TouchPhase, div, point, px, size,
    };

    use super::{EditorSurface, EditorSurfaceMetrics};
    use crate::{EditorScrollbarState, EditorViewport};

    #[test]
    fn shared_surface_metrics_reflect_updates_across_clones() {
        let metrics = EditorSurfaceMetrics::new(px(20.0), px(8.0));
        let clone = metrics.clone();

        metrics.set(px(24.0), px(9.0));

        let snapshot = clone.get();
        assert_eq!(snapshot.line_height, px(24.0));
        assert_eq!(snapshot.cell_width, px(9.0));
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
}
