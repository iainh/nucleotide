// ABOUTME: Native editor scrollbar composed from GPUI element handlers
// ABOUTME: Keeps editor viewport scrolling inside nucleotide-editor

use std::{cell::Cell, rc::Rc};

use gpui::InteractiveElement as _;
use gpui::{
    Along, App, Axis, Bounds, Component, EntityId, Hsla, IntoElement, MouseButton,
    ParentElement as _, Pixels, RenderOnce, Styled as _, Window, div, hsla, px,
};
use nucleotide_types::scrollbar::{
    SCROLLBAR_ALPHA_DRAGGING, SCROLLBAR_ALPHA_INACTIVE, SCROLLBAR_ALPHA_THUMB_HOVER,
    SCROLLBAR_THICKNESS, ScrollbarThumb, scrollbar_padded_track_length,
    scrollbar_scroll_position_for_pointer, scrollbar_thumb, scrollbar_thumb_bounds,
    scrollbar_visual, scrollbar_width_ratio,
};

use crate::{EditorViewport, ViewportScrollUpdate};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;

pub use nucleotide_types::scrollbar::ScrollbarThumb as EditorScrollbarThumb;

#[derive(Debug, Clone, Copy, Default)]
enum EditorThumbState {
    #[default]
    Inactive,
    Hover,
    Dragging(Pixels),
}

impl EditorThumbState {
    fn is_active(&self) -> bool {
        matches!(
            *self,
            EditorThumbState::Hover | EditorThumbState::Dragging(_)
        )
    }

    fn is_dragging(&self) -> bool {
        matches!(*self, EditorThumbState::Dragging(_))
    }
}

#[derive(Clone, Default)]
pub struct EditorScrollbarState {
    thumb_state: Rc<Cell<EditorThumbState>>,
    track_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl EditorScrollbarState {
    pub fn clear_drag(&self) {
        self.set_thumb_state(EditorThumbState::Inactive);
    }

    fn set_track_bounds(&self, bounds: Option<Bounds<Pixels>>) {
        self.track_bounds.set(bounds);
    }

    fn track_bounds(&self) -> Option<Bounds<Pixels>> {
        self.track_bounds.get()
    }

    fn set_drag_offset(&self, offset: Pixels) {
        self.set_thumb_state(EditorThumbState::Dragging(offset));
    }

    fn drag_offset(&self) -> Option<Pixels> {
        match self.thumb_state.get() {
            EditorThumbState::Dragging(offset) => Some(offset),
            EditorThumbState::Inactive | EditorThumbState::Hover => None,
        }
    }

    fn is_dragging(&self) -> bool {
        self.thumb_state.get().is_dragging()
    }

    fn set_thumb_hovered(&self, hovered: bool) {
        self.set_thumb_state(if hovered {
            EditorThumbState::Hover
        } else {
            EditorThumbState::Inactive
        });
    }

    fn set_thumb_state(&self, state: EditorThumbState) {
        self.thumb_state.set(state);
    }

    fn is_expanded(&self) -> bool {
        self.thumb_state.get().is_active()
    }

    fn target_values(&self) -> (f32, f32) {
        let thumb_state = self.thumb_state.get();
        let target_width = scrollbar_width_ratio(self.is_expanded());
        let target_alpha = match thumb_state {
            EditorThumbState::Dragging(_) => SCROLLBAR_ALPHA_DRAGGING,
            EditorThumbState::Hover => SCROLLBAR_ALPHA_THUMB_HOVER,
            EditorThumbState::Inactive => SCROLLBAR_ALPHA_INACTIVE,
        };

        (target_width, target_alpha)
    }
}

pub struct EditorScrollbar {
    view_entity_id: EntityId,
    viewport: EditorViewport,
    state: EditorScrollbarState,
    axis: Axis,
    on_scroll: Option<ScrollCallback>,
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
            thumb_color: hsla(0.0, 0.0, 0.72, 1.0),
        }
    }

    pub fn on_scroll(
        mut self,
        callback: impl Fn(&EditorViewport, ViewportScrollUpdate, &mut App) + 'static,
    ) -> Self {
        self.on_scroll = Some(Rc::new(callback));
        self
    }

    pub fn with_colors(mut self, _track_color: Hsla, thumb_color: Hsla) -> Self {
        self.thumb_color = thumb_color;
        self
    }

    pub fn with_thumb_color(mut self, thumb_color: Hsla) -> Self {
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

        let (width_ratio, alpha) = self.state.target_values();
        let thumb_color = with_alpha(self.thumb_color, alpha);
        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let on_scroll = self.on_scroll.clone();
        let view_entity_id = self.view_entity_id;
        let axis = self.axis;
        let mut track = div().relative().size_full().on_mouse_down(
            MouseButton::Left,
            move |event, _window, cx| {
                let Some(bounds) = state.track_bounds() else {
                    return;
                };
                if !bounds.contains(&event.position) {
                    return;
                }

                let Some((thumb, thumb_bounds)) =
                    thumb_geometry_for_bounds(&viewport, &state, axis, bounds)
                else {
                    return;
                };
                let pointer = event.position.along(axis) - bounds.origin.along(axis);
                let thumb_start = thumb_bounds.origin.along(axis) - bounds.origin.along(axis);
                let thumb_end = thumb_start + thumb_bounds.size.along(axis);
                let drag_offset = if pointer >= thumb_start && pointer <= thumb_end {
                    pointer - thumb_start
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
            },
        );

        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let on_scroll = self.on_scroll.clone();
        let view_entity_id = self.view_entity_id;
        let axis = self.axis;
        track = track.on_mouse_move(move |event, _window, cx| {
            let Some(bounds) = state.track_bounds() else {
                return;
            };
            let Some((thumb, thumb_bounds)) =
                thumb_geometry_for_bounds(&viewport, &state, axis, bounds)
            else {
                return;
            };

            if event.dragging() {
                let Some(drag_offset) = state.drag_offset() else {
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
                return;
            }

            if event.pressed_button.is_none() {
                let over_thumb = thumb_bounds.contains(&event.position);
                let was_thumb_hover = matches!(state.thumb_state.get(), EditorThumbState::Hover);
                state.set_thumb_hovered(over_thumb);
                if over_thumb != was_thumb_hover {
                    cx.notify(view_entity_id);
                }
            }
        });

        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let axis = self.axis;
        track = track.on_mouse_up(MouseButton::Left, move |event, _window, cx| {
            if state.is_dragging() {
                if let Some(bounds) = state.track_bounds() {
                    let over_thumb = thumb_geometry_for_bounds(&viewport, &state, axis, bounds)
                        .is_some_and(|(_, thumb_bounds)| thumb_bounds.contains(&event.position));
                    state.set_thumb_hovered(over_thumb);
                } else {
                    state.set_thumb_hovered(false);
                }

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
            let visual = scrollbar_visual(thumb, track_length, width_ratio);
            let thumb_el = div()
                .absolute()
                .rounded(visual.cross_size / 2.0)
                .bg(thumb_color);
            track = if self.axis == Axis::Vertical {
                track.child(
                    thumb_el
                        .left(visual.cross_offset)
                        .top(visual.along_offset)
                        .w(visual.cross_size)
                        .h(visual.along_size),
                )
            } else {
                track.child(
                    thumb_el
                        .left(visual.along_offset)
                        .top(visual.cross_offset)
                        .w(visual.along_size)
                        .h(visual.cross_size),
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
        base.w(SCROLLBAR_THICKNESS).h_full()
    } else {
        base.w_full().h(SCROLLBAR_THICKNESS)
    }
}

fn thumb_geometry_for_bounds(
    viewport: &EditorViewport,
    state: &EditorScrollbarState,
    axis: Axis,
    bounds: Bounds<Pixels>,
) -> Option<(ScrollbarThumb, Bounds<Pixels>)> {
    let thumb = thumb_for_bounds(viewport, axis, bounds)?;
    let (width_ratio, _) = state.target_values();
    let thumb_bounds = scrollbar_thumb_bounds(thumb, axis, bounds, width_ratio);

    Some((thumb, thumb_bounds))
}

fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    hsla(color.h, color.s, color.l, alpha.clamp(0.0, 1.0))
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
    scrollbar_thumb(
        scrollbar_padded_track_length(track_length),
        viewport_length,
        max_scroll,
        scroll_position,
    )
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
        SCROLLBAR_THICKNESS
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
    scrollbar_scroll_position_for_pointer(track_length, max_scroll, thumb, pointer, drag_offset)
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
                length: px(39.2),
            }
        );
    }

    #[test]
    fn thumb_tracks_scroll_ratio() {
        let thumb = editor_scrollbar_thumb(px(200.0), px(200.0), px(800.0), px(400.0)).unwrap();

        assert_eq!(
            thumb,
            EditorScrollbarThumb {
                start: px(78.4),
                length: px(39.2),
            }
        );
    }

    #[test]
    fn thumb_has_minimum_height() {
        let thumb = editor_scrollbar_thumb(px(200.0), px(20.0), px(1980.0), px(0.0)).unwrap();

        assert_eq!(thumb.length, px(20.0));
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
