// ABOUTME: Native GPUI surface element for editor viewport input
// ABOUTME: Wraps editor content while owning scroll-wheel capture for the viewport

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, ScrollWheelEvent, Window, point, px,
};

use crate::{EditorViewport, ViewportScrollUpdate};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;

pub struct EditorSurface {
    viewport: EditorViewport,
    line_height: Pixels,
    child: Option<AnyElement>,
    on_scroll: Option<ScrollCallback>,
}

impl EditorSurface {
    pub fn new(viewport: EditorViewport, line_height: Pixels, child: impl IntoElement) -> Self {
        Self {
            viewport,
            line_height,
            child: Some(child.into_any_element()),
            on_scroll: None,
        }
    }

    pub fn on_scroll(
        mut self,
        callback: impl Fn(&EditorViewport, ViewportScrollUpdate, &mut App) + 'static,
    ) -> Self {
        self.on_scroll = Some(Rc::new(callback));
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
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
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

        child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}
