// ABOUTME: Native editor scrollbar composed from GPUI element handlers
// ABOUTME: Keeps editor viewport scrolling inside nucleotide-editor

use std::{cell::Cell, rc::Rc};

use gpui::InteractiveElement as _;
use gpui::{
    Along, App, Axis, Bounds, Component, EntityId, Hsla, IntoElement, MouseButton,
    ParentElement as _, Pixels, RenderOnce, Styled as _, Window, div, hsla, px,
};

use crate::{EditorViewport, ViewportScrollUpdate};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;

const TRACK_THICKNESS: Pixels = px(12.0);
const THUMB_INSET: Pixels = px(3.0);
const MIN_THUMB_LENGTH: Pixels = px(24.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorScrollbarThumb {
    pub start: Pixels,
    pub length: Pixels,
}

#[derive(Clone, Default)]
pub struct EditorScrollbarState {
    track_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    drag_offset: Rc<Cell<Option<Pixels>>>,
}

impl EditorScrollbarState {
    pub fn clear_drag(&self) {
        self.drag_offset.set(None);
    }

    fn set_track_bounds(&self, bounds: Option<Bounds<Pixels>>) {
        self.track_bounds.set(bounds);
    }

    fn track_bounds(&self) -> Option<Bounds<Pixels>> {
        self.track_bounds.get()
    }

    fn set_drag_offset(&self, offset: Pixels) {
        self.drag_offset.set(Some(offset));
    }

    fn drag_offset(&self) -> Option<Pixels> {
        self.drag_offset.get()
    }
}

pub struct EditorScrollbar {
    view_entity_id: EntityId,
    viewport: EditorViewport,
    state: EditorScrollbarState,
    axis: Axis,
    on_scroll: Option<ScrollCallback>,
    track_color: Hsla,
    thumb_color: Hsla,
}

#[derive(Clone, Copy)]
struct ScrollbarPointerGeometry {
    bounds: Bounds<Pixels>,
    thumb: EditorScrollbarThumb,
    pointer: Pixels,
    drag_offset: Pixels,
}

impl EditorScrollbar {
    pub fn vertical(
        view_entity_id: EntityId,
        viewport: EditorViewport,
        state: EditorScrollbarState,
    ) -> Self {
        Self::new(view_entity_id, viewport, state, Axis::Vertical)
    }

    pub fn horizontal(
        view_entity_id: EntityId,
        viewport: EditorViewport,
        state: EditorScrollbarState,
    ) -> Self {
        Self::new(view_entity_id, viewport, state, Axis::Horizontal)
    }

    fn new(
        view_entity_id: EntityId,
        viewport: EditorViewport,
        state: EditorScrollbarState,
        axis: Axis,
    ) -> Self {
        Self {
            view_entity_id,
            viewport,
            state,
            axis,
            on_scroll: None,
            track_color: hsla(0.0, 0.0, 0.0, 0.0),
            thumb_color: hsla(0.0, 0.0, 0.72, 0.36),
        }
    }

    pub fn on_scroll(
        mut self,
        callback: impl Fn(&EditorViewport, ViewportScrollUpdate, &mut App) + 'static,
    ) -> Self {
        self.on_scroll = Some(Rc::new(callback));
        self
    }

    pub fn with_colors(mut self, track_color: Hsla, thumb_color: Hsla) -> Self {
        self.track_color = track_color;
        self.thumb_color = thumb_color;
        self
    }
}

fn thumb_for_bounds(
    viewport: &EditorViewport,
    axis: Axis,
    bounds: Bounds<Pixels>,
) -> Option<EditorScrollbarThumb> {
    editor_scrollbar_thumb(
        bounds.size.along(axis),
        viewport.viewport_bounds().size.along(axis),
        viewport.max_scroll_offset().along(axis),
        viewport.scroll_position().along(axis),
    )
}

impl EditorScrollbar {
    fn is_visible(&self) -> bool {
        editor_scrollbar_thickness(&self.viewport, self.axis) > px(0.0)
    }
}

impl IntoElement for EditorScrollbar {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for EditorScrollbar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if !self.is_visible() {
            self.state.set_track_bounds(None);
            return empty_scrollbar_track(self.axis);
        }

        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let on_scroll = self.on_scroll.clone();
        let view_entity_id = self.view_entity_id;
        let axis = self.axis;
        let mut track = div()
            .relative()
            .size_full()
            .bg(self.track_color)
            .on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                let Some(bounds) = state.track_bounds() else {
                    return;
                };
                if !bounds.contains(&event.position) {
                    return;
                }

                let Some(thumb) = thumb_for_bounds(&viewport, axis, bounds) else {
                    return;
                };
                let pointer = event.position.along(axis) - bounds.origin.along(axis);
                let drag_offset = if pointer >= thumb.start && pointer <= thumb.start + thumb.length
                {
                    pointer - thumb.start
                } else {
                    thumb.length / 2.0
                };
                state.set_drag_offset(drag_offset);

                apply_scrollbar_pointer(
                    &viewport,
                    on_scroll.as_ref(),
                    view_entity_id,
                    axis,
                    cx,
                    ScrollbarPointerGeometry {
                        bounds,
                        thumb,
                        pointer,
                        drag_offset,
                    },
                );
            });

        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let on_scroll = self.on_scroll.clone();
        let view_entity_id = self.view_entity_id;
        let axis = self.axis;
        track = track.on_mouse_move(move |event, _window, cx| {
            if event.dragging() {
                let Some(drag_offset) = state.drag_offset() else {
                    return;
                };
                let Some(bounds) = state.track_bounds() else {
                    return;
                };
                let Some(thumb) = thumb_for_bounds(&viewport, axis, bounds) else {
                    return;
                };

                apply_scrollbar_pointer(
                    &viewport,
                    on_scroll.as_ref(),
                    view_entity_id,
                    axis,
                    cx,
                    ScrollbarPointerGeometry {
                        bounds,
                        thumb,
                        pointer: event.position.along(axis) - bounds.origin.along(axis),
                        drag_offset,
                    },
                );
            }
        });

        let state = self.state.clone();
        track = track.on_mouse_up(MouseButton::Left, move |_event, _window, cx| {
            if state.drag_offset().is_some() {
                state.clear_drag();
                cx.stop_propagation();
            }
        });

        let track_length = self
            .state
            .track_bounds()
            .map(|bounds| bounds.size.along(self.axis))
            .unwrap_or_else(|| self.viewport.viewport_bounds().size.along(self.axis));
        if let Some(thumb) = editor_scrollbar_thumb(
            track_length,
            self.viewport.viewport_bounds().size.along(self.axis),
            self.viewport.max_scroll_offset().along(self.axis),
            self.viewport.scroll_position().along(self.axis),
        ) {
            let thumb_el = div().absolute().bg(self.thumb_color);
            track = if self.axis == Axis::Vertical {
                track.child(
                    thumb_el
                        .left(THUMB_INSET)
                        .top(thumb.start)
                        .w(TRACK_THICKNESS - (THUMB_INSET * 2.0))
                        .h(thumb.length),
                )
            } else {
                track.child(
                    thumb_el
                        .left(thumb.start)
                        .top(THUMB_INSET)
                        .w(thumb.length)
                        .h(TRACK_THICKNESS - (THUMB_INSET * 2.0)),
                )
            };
        }

        scrollbar_track(self.axis)
            .on_children_prepainted({
                let state = self.state.clone();
                move |bounds, _window, _cx| {
                    state.set_track_bounds(bounds.into_iter().next());
                }
            })
            .child(track)
    }
}

fn empty_scrollbar_track(axis: Axis) -> gpui::Div {
    scrollbar_track(axis)
}

fn scrollbar_track(axis: Axis) -> gpui::Div {
    let base = div().relative().flex_shrink_0();
    if axis == Axis::Vertical {
        base.w(TRACK_THICKNESS).h_full()
    } else {
        base.flex_1().min_w(px(0.0)).h(TRACK_THICKNESS)
    }
}

fn apply_scrollbar_pointer(
    viewport: &EditorViewport,
    on_scroll: Option<&ScrollCallback>,
    view_entity_id: EntityId,
    axis: Axis,
    cx: &mut App,
    geometry: ScrollbarPointerGeometry,
) {
    let scroll_position = scroll_position_for_scrollbar_pointer(
        geometry.bounds.size.along(axis),
        viewport.max_scroll_offset().along(axis),
        geometry.thumb,
        geometry.pointer,
        geometry.drag_offset,
    );
    let update = if axis == Axis::Vertical {
        viewport.scroll_to_vertical_position_from_scrollbar(scroll_position)
    } else {
        viewport.scroll_to_horizontal_position_from_scrollbar(scroll_position)
    };

    if !update.changed {
        cx.stop_propagation();
        return;
    }

    if let Some(on_scroll) = on_scroll {
        on_scroll(viewport, update, cx);
    }

    cx.notify(view_entity_id);
    cx.stop_propagation();
}

pub fn editor_scrollbar_thumb(
    track_length: Pixels,
    viewport_length: Pixels,
    max_scroll: Pixels,
    scroll_position: Pixels,
) -> Option<EditorScrollbarThumb> {
    if track_length <= px(0.0) || viewport_length <= px(0.0) || max_scroll <= px(0.0) {
        return None;
    }

    let content_length = viewport_length + max_scroll;
    if content_length <= viewport_length {
        return None;
    }

    let thumb_length = (track_length * (viewport_length / content_length))
        .max(MIN_THUMB_LENGTH)
        .min(track_length);
    let max_thumb_start = (track_length - thumb_length).max(px(0.0));
    let scroll_ratio = (scroll_position / max_scroll).clamp(0.0, 1.0);

    Some(EditorScrollbarThumb {
        start: max_thumb_start * scroll_ratio,
        length: thumb_length,
    })
}

pub fn editor_scrollbar_thickness(viewport: &EditorViewport, axis: Axis) -> Pixels {
    if editor_scrollbar_thumb(
        viewport.viewport_bounds().size.along(axis),
        viewport.viewport_bounds().size.along(axis),
        viewport.max_scroll_offset().along(axis),
        viewport.scroll_position().along(axis),
    )
    .is_some()
    {
        TRACK_THICKNESS
    } else {
        px(0.0)
    }
}

pub fn editor_vertical_scrollbar_width(viewport: &EditorViewport) -> Pixels {
    editor_scrollbar_thickness(viewport, Axis::Vertical)
}

pub fn editor_horizontal_scrollbar_height(viewport: &EditorViewport) -> Pixels {
    editor_scrollbar_thickness(viewport, Axis::Horizontal)
}

pub fn scroll_position_for_scrollbar_pointer(
    track_length: Pixels,
    max_scroll: Pixels,
    thumb: EditorScrollbarThumb,
    pointer: Pixels,
    drag_offset: Pixels,
) -> Pixels {
    let max_thumb_start = (track_length - thumb.length).max(px(0.0));
    if max_thumb_start <= px(0.0) || max_scroll <= px(0.0) {
        return px(0.0);
    }

    let thumb_start = (pointer - drag_offset).clamp(px(0.0), max_thumb_start);
    max_scroll * (thumb_start / max_thumb_start)
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext as _, Empty, Entity, IntoElement as _, MouseButton, ParentElement as _, Styled,
        TestAppContext, div, point, px, size,
    };

    use super::{
        EditorScrollbar, EditorScrollbarState, EditorScrollbarThumb, editor_scrollbar_thumb,
        scroll_position_for_scrollbar_pointer,
    };
    use crate::EditorViewport;

    #[test]
    fn thumb_is_absent_when_content_fits() {
        assert_eq!(
            editor_scrollbar_thumb(px(200.0), px(200.0), px(0.0), px(0.0)),
            None
        );
    }

    #[test]
    fn thumb_scales_to_viewport_fraction() {
        let thumb = editor_scrollbar_thumb(px(200.0), px(200.0), px(800.0), px(0.0)).unwrap();

        assert_eq!(
            thumb,
            EditorScrollbarThumb {
                start: px(0.0),
                length: px(40.0),
            }
        );
    }

    #[test]
    fn thumb_tracks_scroll_ratio() {
        let thumb = editor_scrollbar_thumb(px(200.0), px(200.0), px(800.0), px(400.0)).unwrap();

        assert_eq!(
            thumb,
            EditorScrollbarThumb {
                start: px(80.0),
                length: px(40.0),
            }
        );
    }

    #[test]
    fn thumb_has_minimum_height() {
        let thumb = editor_scrollbar_thumb(px(200.0), px(20.0), px(1980.0), px(0.0)).unwrap();

        assert_eq!(thumb.length, px(24.0));
    }

    #[test]
    fn pointer_position_maps_to_scroll_position() {
        let thumb = EditorScrollbarThumb {
            start: px(0.0),
            length: px(40.0),
        };

        let scroll_y =
            scroll_position_for_scrollbar_pointer(px(200.0), px(800.0), thumb, px(100.0), px(20.0));

        assert_eq!(scroll_y, px(400.0));
    }

    #[gpui::test]
    fn editor_scrollbar_draws_and_handles_drag(cx: &mut TestAppContext) {
        let view_entity_id = cx.update(|cx| {
            let entity: Entity<Empty> = cx.new(|_| Empty);
            entity.entity_id()
        });
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(200.0)), 50);
        let state = EditorScrollbarState::default();

        let window = cx.add_empty_window();
        window.draw(
            point(px(0.0), px(0.0)),
            size(px(12.0), px(200.0)),
            |_, _| {
                div()
                    .w(px(12.0))
                    .h(px(200.0))
                    .child(EditorScrollbar::vertical(
                        view_entity_id,
                        viewport.clone(),
                        state.clone(),
                    ))
                    .into_element()
            },
        );

        window.simulate_mouse_down(
            point(px(6.0), px(60.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        window.simulate_mouse_move(
            point(px(6.0), px(100.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        window.simulate_mouse_up(
            point(px(6.0), px(100.0)),
            MouseButton::Left,
            gpui::Modifiers::none(),
        );

        assert!(viewport.scroll_position().y > px(0.0));
        assert!(viewport.has_pending_view_sync());
    }
}
