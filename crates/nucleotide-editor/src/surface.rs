// ABOUTME: Native GPUI surface element for editor viewport input
// ABOUTME: Wraps editor content while owning scroll-wheel capture for the viewport

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Window,
    point, px,
};

use crate::{EditorViewport, ViewportScrollUpdate};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;
type PointerCallback = Rc<dyn Fn(EditorSurfacePointerEvent, &mut App)>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorSurfacePointerEvent {
    pub position: Point<Pixels>,
    pub bounds: Bounds<Pixels>,
    pub line_height: Pixels,
    pub cell_width: Pixels,
}

pub struct EditorSurface {
    viewport: EditorViewport,
    line_height: Pixels,
    cell_width: Pixels,
    child: Option<AnyElement>,
    on_scroll: Option<ScrollCallback>,
    on_mouse_down: Option<PointerCallback>,
    on_mouse_up: Option<PointerCallback>,
}

impl EditorSurface {
    pub fn new(
        viewport: EditorViewport,
        line_height: Pixels,
        cell_width: Pixels,
        child: impl IntoElement,
    ) -> Self {
        Self {
            viewport,
            line_height,
            cell_width,
            child: Some(child.into_any_element()),
            on_scroll: None,
            on_mouse_down: None,
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

    pub fn on_mouse_up(
        mut self,
        callback: impl Fn(EditorSurfacePointerEvent, &mut App) + 'static,
    ) -> Self {
        self.on_mouse_up = Some(Rc::new(callback));
        self
    }
}

impl IntoElement for EditorSurface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorSurface {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

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
        let mut child = self
            .child
            .take()
            .expect("EditorSurface child is consumed once per frame");
        let layout_id = child.request_layout(window, cx);
        (layout_id, child)
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.on_mouse_event({
            let viewport = self.viewport.clone();
            let line_height = self.line_height;
            let on_scroll = self.on_scroll.clone();
            let view_entity_id = window.current_view();

            move |event: &ScrollWheelEvent, phase, _window, cx| {
                if !(bounds.contains(&event.position) && phase.bubble()) {
                    return;
                }

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
            }
        });

        window.on_mouse_event({
            let line_height = self.line_height;
            let cell_width = self.cell_width;
            let on_mouse_down = self.on_mouse_down.clone();
            let view_entity_id = window.current_view();

            move |event: &MouseDownEvent, phase, _window, cx| {
                if event.button != MouseButton::Left
                    || !(bounds.contains(&event.position) && phase.bubble())
                {
                    return;
                }

                if let Some(on_mouse_down) = &on_mouse_down {
                    on_mouse_down(
                        EditorSurfacePointerEvent {
                            position: event.position,
                            bounds,
                            line_height,
                            cell_width,
                        },
                        cx,
                    );
                }

                cx.notify(view_entity_id);
                cx.stop_propagation();
            }
        });

        window.on_mouse_event({
            let line_height = self.line_height;
            let cell_width = self.cell_width;
            let on_mouse_up = self.on_mouse_up.clone();
            let view_entity_id = window.current_view();

            move |event: &MouseUpEvent, phase, _window, cx| {
                if event.button != MouseButton::Left
                    || !(bounds.contains(&event.position) && phase.bubble())
                {
                    return;
                }

                if let Some(on_mouse_up) = &on_mouse_up {
                    on_mouse_up(
                        EditorSurfacePointerEvent {
                            position: event.position,
                            bounds,
                            line_height,
                            cell_width,
                        },
                        cx,
                    );
                }

                cx.notify(view_entity_id);
                cx.stop_propagation();
            }
        });

        child.paint(window, cx);
    }
}
