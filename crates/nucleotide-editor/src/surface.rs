// ABOUTME: Native GPUI surface element for editor viewport input
// ABOUTME: Wraps editor content while owning scroll-wheel capture for the viewport

use std::{cell::Cell, rc::Rc};

use gpui::prelude::{InteractiveElement, IntoElement, ParentElement, Styled};
use gpui::{
    AnyElement, App, Bounds, Div, EntityId, Hsla, Modifiers, MouseButton, Pixels, Point, Window,
    div, fill, point, px,
};

use crate::{EditorScrollbar, EditorScrollbarState, EditorViewport, ViewportScrollUpdate};

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
}

impl IntoElement for EditorSurface {
    type Element = Div;

    fn into_element(self) -> Div {
        let child_bounds = Rc::new(Cell::new(None));
        let scrollbar_on_scroll = self.on_scroll.clone();
        let mut scrollbar = EditorScrollbar::new(
            self.view_entity_id,
            self.viewport.clone(),
            self.scrollbar_state.clone(),
        );
        if let Some(on_scroll) = scrollbar_on_scroll {
            scrollbar = scrollbar.on_scroll(move |viewport, update, cx| {
                on_scroll(viewport, update, cx);
            });
        }

        let mut element = div()
            .w_full()
            .h_full()
            .flex()
            .on_children_prepainted({
                let child_bounds = Rc::clone(&child_bounds);

                move |bounds, _window, _cx| {
                    child_bounds.set(bounds.into_iter().next());
                }
            })
            .child(div().w_full().h_full().flex_1().child(self.child))
            .child(scrollbar);

        if let Some(on_scroll) = self.on_scroll {
            let viewport = self.viewport.clone();
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;

            element = element.on_scroll_wheel(move |event, _window, cx| {
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

        if let Some(on_mouse_down) = self.on_mouse_down {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;
            let child_bounds = Rc::clone(&child_bounds);

            element = element.on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                if let Some(bounds) = child_bounds.get() {
                    let metrics = metrics.get();
                    on_mouse_down(
                        EditorSurfacePointerEvent {
                            position: event.position,
                            modifiers: event.modifiers,
                            bounds,
                            line_height: metrics.line_height,
                            cell_width: metrics.cell_width,
                        },
                        cx,
                    );

                    cx.notify(view_entity_id);
                    cx.stop_propagation();
                }
            });
        }

        if let Some(on_mouse_drag) = self.on_mouse_drag {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;
            let child_bounds = Rc::clone(&child_bounds);

            element = element.on_mouse_move(move |event, _window, cx| {
                if !event.dragging() {
                    return;
                }

                if let Some(bounds) = child_bounds.get() {
                    let metrics = metrics.get();
                    on_mouse_drag(
                        EditorSurfacePointerEvent {
                            position: event.position,
                            modifiers: event.modifiers,
                            bounds,
                            line_height: metrics.line_height,
                            cell_width: metrics.cell_width,
                        },
                        cx,
                    );

                    cx.notify(view_entity_id);
                    cx.stop_propagation();
                }
            });
        }

        if let Some(on_mouse_up) = self.on_mouse_up {
            let metrics = self.metrics.clone();
            let view_entity_id = self.view_entity_id;
            let child_bounds = Rc::clone(&child_bounds);

            element = element.on_mouse_up(MouseButton::Left, move |event, _window, cx| {
                if let Some(bounds) = child_bounds.get() {
                    let metrics = metrics.get();
                    on_mouse_up(
                        EditorSurfacePointerEvent {
                            position: event.position,
                            modifiers: event.modifiers,
                            bounds,
                            line_height: metrics.line_height,
                            cell_width: metrics.cell_width,
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

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::EditorSurfaceMetrics;

    #[test]
    fn shared_surface_metrics_reflect_updates_across_clones() {
        let metrics = EditorSurfaceMetrics::new(px(20.0), px(8.0));
        let clone = metrics.clone();

        metrics.set(px(24.0), px(9.0));

        let snapshot = clone.get();
        assert_eq!(snapshot.line_height, px(24.0));
        assert_eq!(snapshot.cell_width, px(9.0));
    }
}
