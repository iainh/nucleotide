// ABOUTME: Native GPUI surface element for editor viewport input
// ABOUTME: Wraps editor content while owning scroll-wheel capture for the viewport

use std::{cell::Cell, rc::Rc};

use gpui::prelude::{InteractiveElement, IntoElement, ParentElement, Styled};
use gpui::{
    AnyElement, App, Bounds, Div, EntityId, Hsla, MouseButton, Pixels, Point, Window, div, fill,
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
    view_entity_id: EntityId,
    viewport: EditorViewport,
    line_height: Pixels,
    cell_width: Pixels,
    child: AnyElement,
    on_scroll: Option<ScrollCallback>,
    on_mouse_down: Option<PointerCallback>,
    on_mouse_up: Option<PointerCallback>,
}

pub fn paint_editor_background(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    window.paint_quad(fill(bounds, color));
}

impl EditorSurface {
    pub fn new(
        view_entity_id: EntityId,
        viewport: EditorViewport,
        line_height: Pixels,
        cell_width: Pixels,
        child: impl IntoElement,
    ) -> Self {
        Self {
            view_entity_id,
            viewport,
            line_height,
            cell_width,
            child: child.into_any_element(),
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
    type Element = Div;

    fn into_element(self) -> Div {
        let child_bounds = Rc::new(Cell::new(None));
        let mut element = div()
            .w_full()
            .h_full()
            .on_children_prepainted({
                let child_bounds = Rc::clone(&child_bounds);

                move |bounds, _window, _cx| {
                    child_bounds.set(bounds.into_iter().next());
                }
            })
            .child(self.child);

        if let Some(on_scroll) = self.on_scroll {
            let viewport = self.viewport.clone();
            let line_height = self.line_height;
            let view_entity_id = self.view_entity_id;

            element = element.on_scroll_wheel(move |event, _window, cx| {
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

        if let Some(on_mouse_down) = self.on_mouse_down {
            let line_height = self.line_height;
            let cell_width = self.cell_width;
            let view_entity_id = self.view_entity_id;
            let child_bounds = Rc::clone(&child_bounds);

            element = element.on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                if let Some(bounds) = child_bounds.get() {
                    on_mouse_down(
                        EditorSurfacePointerEvent {
                            position: event.position,
                            bounds,
                            line_height,
                            cell_width,
                        },
                        cx,
                    );

                    cx.notify(view_entity_id);
                    cx.stop_propagation();
                }
            });
        }

        if let Some(on_mouse_up) = self.on_mouse_up {
            let line_height = self.line_height;
            let cell_width = self.cell_width;
            let view_entity_id = self.view_entity_id;
            let child_bounds = Rc::clone(&child_bounds);

            element = element.on_mouse_up(MouseButton::Left, move |event, _window, cx| {
                if let Some(bounds) = child_bounds.get() {
                    on_mouse_up(
                        EditorSurfacePointerEvent {
                            position: event.position,
                            bounds,
                            line_height,
                            cell_width,
                        },
                        cx,
                    );

                    cx.notify(view_entity_id);
                    cx.stop_propagation();
                }
            });
        }

        element
    }
}
