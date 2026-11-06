// ABOUTME: Native editor scrollbar composed from GPUI element handlers
// ABOUTME: Keeps editor viewport scrolling inside nucleotide-editor

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::InteractiveElement as _;
use gpui::{
    Along, App, Axis, Bounds, Component, EntityId, Hsla, IntoElement, MouseButton,
    ParentElement as _, Pixels, RenderOnce, StatefulInteractiveElement, Styled as _, Window,
    canvas, div, fill, hsla, point, px, size,
};
use nucleotide_types::scrollbar::{
    SCROLLBAR_ALPHA_DRAGGING, SCROLLBAR_ALPHA_INACTIVE, SCROLLBAR_ALPHA_THUMB_HOVER,
    SCROLLBAR_THICKNESS, ScrollbarPart, ScrollbarStyle, ScrollbarThumb,
    scrollbar_padded_track_length, scrollbar_scroll_position_for_pointer, scrollbar_thumb,
    scrollbar_width_ratio,
};

use crate::{EditorViewport, ViewportScrollUpdate};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;

pub use nucleotide_types::scrollbar::ScrollbarThumb as EditorScrollbarThumb;

const SCROLLBAR_MARKER_HEIGHT: Pixels = px(2.0);
const SCROLLBAR_MARKER_WIDTH: Pixels = px(10.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorScrollbarMarker {
    pub position: f32,
    pub color: Hsla,
}

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
    track_hovered: Rc<Cell<bool>>,
    pressed: Rc<Cell<Option<(u64, ScrollbarPart)>>>,
    press_generation: Rc<Cell<u64>>,
}

impl EditorScrollbarState {
    pub fn clear_drag(&self) {
        self.set_thumb_state(EditorThumbState::Inactive);
        self.pressed.set(None);
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
        self.track_hovered.get() || self.thumb_state.get().is_active()
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
    style: ScrollbarStyle,
    track_color: Hsla,
    contrast_disabled_color: Option<Hsla>,
    markers: Vec<EditorScrollbarMarker>,
}

#[derive(Clone, Copy)]
struct ScrollbarPointerGeometry {
    style: ScrollbarStyle,
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
            style: ScrollbarStyle::Themed,
            track_color: gpui::transparent_black(),
            contrast_disabled_color: None,
            markers: Vec::new(),
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

    pub fn with_style(
        mut self,
        style: ScrollbarStyle,
        track_color: Hsla,
        contrast_disabled_color: Option<Hsla>,
    ) -> Self {
        self.style = style;
        self.track_color = track_color;
        self.contrast_disabled_color = contrast_disabled_color;
        self
    }

    pub fn with_markers(mut self, markers: Vec<EditorScrollbarMarker>) -> Self {
        self.markers = markers;
        self
    }
}

fn thumb_for_bounds(
    viewport: &EditorViewport,
    style: ScrollbarStyle,
    axis: Axis,
    bounds: Bounds<Pixels>,
) -> Option<EditorScrollbarThumb> {
    style.thumb(
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
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if !self.is_visible() {
            self.state.set_track_bounds(None);
            return empty_scrollbar_track(self.axis);
        }

        let style = self.style;
        let (_, alpha) = self.state.target_values();
        let alpha = if self.contrast_disabled_color.is_some() {
            1.0
        } else if style == ScrollbarStyle::Windows {
            0.45
        } else {
            alpha
        };
        let thumb_color = with_alpha(self.thumb_color, alpha);
        let expanded = self.state.is_expanded();
        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let on_scroll = self.on_scroll.clone();
        let view_entity_id = self.view_entity_id;
        let axis = self.axis;
        let mut track = div()
            .relative()
            .size_full()
            .id(("editor-scrollbar", Rc::as_ptr(&state.track_bounds) as usize))
            .on_hover({
                let state = state.clone();
                move |hovered, window, _cx| {
                    if style == ScrollbarStyle::Windows {
                        state.track_hovered.set(*hovered);
                        if !*hovered && !state.is_dragging() {
                            state.set_thumb_hovered(false);
                        }
                        window.refresh();
                    }
                }
            })
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let Some(bounds) = state.track_bounds() else {
                    return;
                };
                if !bounds.contains(&event.position) {
                    return;
                }

                let Some((thumb, thumb_bounds)) =
                    thumb_geometry_for_bounds(&viewport, &state, style, axis, bounds)
                else {
                    return;
                };
                let pointer = event.position.along(axis) - bounds.origin.along(axis);
                let thumb_start = thumb_bounds.origin.along(axis) - bounds.origin.along(axis);
                let thumb_end = thumb_start + thumb_bounds.size.along(axis);
                if style == ScrollbarStyle::Windows && !thumb_bounds.contains(&event.position) {
                    let part = style.part(thumb, bounds.size.along(axis), pointer);
                    let generation = state.press_generation.get().wrapping_add(1);
                    state.press_generation.set(generation);
                    state.pressed.set(Some((generation, part)));
                    apply_scrollbar_part(
                        &viewport,
                        on_scroll.as_ref(),
                        view_entity_id,
                        axis,
                        part,
                        cx,
                    );
                    let state = state.clone();
                    let viewport = viewport.clone();
                    let on_scroll = on_scroll.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let mut delay = Duration::from_millis(250);
                            loop {
                                cx.background_executor().timer(delay).await;
                                let repeat = cx
                                    .update(|window, cx| {
                                        if !window.is_window_active()
                                            || state.pressed.get() != Some((generation, part))
                                        {
                                            return false;
                                        }
                                        let Some(bounds) = state.track_bounds() else {
                                            return false;
                                        };
                                        let Some(thumb) =
                                            thumb_for_bounds(&viewport, style, axis, bounds)
                                        else {
                                            return false;
                                        };
                                        let pointer = window.mouse_position();
                                        if bounds.contains(&pointer)
                                            && style.part(
                                                thumb,
                                                bounds.size.along(axis),
                                                pointer.along(axis) - bounds.origin.along(axis),
                                            ) == part
                                        {
                                            apply_scrollbar_part(
                                                &viewport,
                                                on_scroll.as_ref(),
                                                view_entity_id,
                                                axis,
                                                part,
                                                cx,
                                            );
                                            window.refresh();
                                        }
                                        true
                                    })
                                    .unwrap_or(false);
                                if !repeat {
                                    break;
                                }
                                delay = Duration::from_millis(50);
                            }
                        })
                        .detach();
                    window.refresh();
                    cx.stop_propagation();
                    return;
                }
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
                        style,
                        bounds,
                        thumb,
                        pointer,
                        drag_offset,
                    },
                );
                window.refresh();
            });

        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let on_scroll = self.on_scroll.clone();
        let view_entity_id = self.view_entity_id;
        let axis = self.axis;
        track = track.on_mouse_move(move |event, window, cx| {
            let Some(bounds) = state.track_bounds() else {
                return;
            };
            let Some((thumb, thumb_bounds)) =
                thumb_geometry_for_bounds(&viewport, &state, style, axis, bounds)
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
                        style,
                        bounds,
                        thumb,
                        pointer: event.position.along(axis) - bounds.origin.along(axis),
                        drag_offset,
                    },
                );
                return;
            }

            if event.pressed_button.is_none() {
                state.pressed.set(None);
                let over_thumb = thumb_bounds.contains(&event.position);
                let was_thumb_hover = matches!(state.thumb_state.get(), EditorThumbState::Hover);
                state.set_thumb_hovered(over_thumb);
                if over_thumb != was_thumb_hover {
                    cx.notify(view_entity_id);
                }
            }
            if style == ScrollbarStyle::Windows {
                window.refresh();
            }
        });

        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let axis = self.axis;
        track = track.on_mouse_up(MouseButton::Left, move |event, window, cx| {
            if state.pressed.replace(None).is_some() {
                window.refresh();
                cx.stop_propagation();
            }
            if state.is_dragging() {
                if let Some(bounds) = state.track_bounds() {
                    let over_thumb =
                        thumb_geometry_for_bounds(&viewport, &state, style, axis, bounds)
                            .is_some_and(|(_, thumb_bounds)| {
                                thumb_bounds.contains(&event.position)
                            });
                    state.set_thumb_hovered(over_thumb);
                } else {
                    state.set_thumb_hovered(false);
                }

                cx.stop_propagation();
            }
        });

        let state = self.state.clone();
        track = track.on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
            if style == ScrollbarStyle::Windows
                && (state.pressed.replace(None).is_some() || state.is_dragging())
            {
                state.clear_drag();
                state.track_hovered.set(false);
                window.refresh();
                cx.stop_propagation();
            }
        });
        if style == ScrollbarStyle::Windows && expanded {
            track = track.bg(self.track_color).rounded(px(3.0));
            for part in [ScrollbarPart::Decrease, ScrollbarPart::Increase] {
                let hovered = self.state.track_bounds().is_some_and(|bounds| {
                    bounds.contains(&window.mouse_position())
                        && thumb_for_bounds(&self.viewport, style, axis, bounds).is_some_and(
                            |thumb| {
                                style.part(
                                    thumb,
                                    bounds.size.along(axis),
                                    window.mouse_position().along(axis) - bounds.origin.along(axis),
                                ) == part
                            },
                        )
                });
                let position = self.viewport.scroll_position().along(axis);
                let disabled = if part == ScrollbarPart::Decrease {
                    position <= px(0.0)
                } else {
                    position >= self.viewport.max_scroll_offset().along(axis)
                };
                let arrow_color = if let Some(gray_text) = self.contrast_disabled_color {
                    if disabled {
                        gray_text
                    } else {
                        self.thumb_color
                    }
                } else {
                    with_alpha(
                        self.thumb_color,
                        if disabled {
                            0.2
                        } else if hovered {
                            0.8
                        } else {
                            0.45
                        },
                    )
                };
                let mut arrow = div()
                    .absolute()
                    .size(SCROLLBAR_THICKNESS)
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family("Segoe Fluent Icons")
                    .line_height(gpui::relative(1.0))
                    .text_size(px(
                        if !disabled
                            && hovered
                            && self.state.pressed.get().is_some_and(|(_, p)| p == part)
                        {
                            7.0
                        } else {
                            8.0
                        },
                    ))
                    .text_color(arrow_color)
                    .child(part.arrow_glyph(axis));
                arrow = match (axis, part) {
                    (Axis::Vertical, ScrollbarPart::Decrease) => arrow.top_0().pt(px(4.0)),
                    (Axis::Vertical, _) => arrow.bottom_0().pb(px(4.0)),
                    (Axis::Horizontal, ScrollbarPart::Decrease) => arrow.left_0().pl(px(4.0)),
                    (Axis::Horizontal, _) => arrow.right_0().pr(px(4.0)),
                };
                track = track.child(arrow);
            }
        }

        let viewport = self.viewport.clone();
        let drag_viewport = viewport.clone();
        let drag_state = self.state.clone();
        let on_scroll = self.on_scroll.clone();
        let markers = self.markers;
        track = track.child(
            canvas(
                move |bounds, _, _| {
                    thumb_for_bounds(&viewport, style, axis, bounds)
                        .map(|thumb| style.visual(thumb, bounds.size.along(axis), expanded))
                },
                move |bounds, visual, window, _| {
                    if style == ScrollbarStyle::Windows {
                        // A thumb owns its drag even beyond the track's hitbox.
                        window.on_mouse_event(
                            move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                                if phase != gpui::DispatchPhase::Capture || !event.dragging() {
                                    return;
                                }
                                let Some(drag_offset) = drag_state.drag_offset() else {
                                    return;
                                };
                                let Some(bounds) = drag_state.track_bounds() else {
                                    return;
                                };
                                let Some(thumb) =
                                    thumb_for_bounds(&drag_viewport, style, axis, bounds)
                                else {
                                    return;
                                };
                                apply_scrollbar_pointer(
                                    &drag_viewport,
                                    on_scroll.as_ref(),
                                    view_entity_id,
                                    axis,
                                    cx,
                                    ScrollbarPointerGeometry {
                                        style,
                                        bounds,
                                        thumb,
                                        pointer: event.position.along(axis)
                                            - bounds.origin.along(axis),
                                        drag_offset,
                                    },
                                );
                                window.refresh();
                            },
                        );
                    }
                    // Use this frame's track bounds, never the previous frame's size.
                    if axis == Axis::Vertical {
                        for marker in &markers {
                            let origin = bounds.origin
                                + point(
                                    (SCROLLBAR_THICKNESS - SCROLLBAR_MARKER_WIDTH) / 2.0,
                                    scrollbar_marker_top(marker.position, bounds.size.height),
                                );
                            let mut quad = fill(
                                Bounds::new(
                                    origin,
                                    size(SCROLLBAR_MARKER_WIDTH, SCROLLBAR_MARKER_HEIGHT),
                                ),
                                marker.color,
                            );
                            quad.corner_radii = px(1.0).into();
                            window.paint_quad(quad);
                        }
                    }
                    if let Some(visual) = visual {
                        let (offset, size) = if axis == Axis::Vertical {
                            (
                                point(visual.cross_offset, visual.along_offset),
                                size(visual.cross_size, visual.along_size),
                            )
                        } else {
                            (
                                point(visual.along_offset, visual.cross_offset),
                                size(visual.along_size, visual.cross_size),
                            )
                        };
                        let mut quad = fill(Bounds::new(bounds.origin + offset, size), thumb_color);
                        quad.corner_radii = (visual.cross_size / 2.0).into();
                        window.paint_quad(quad);
                    }
                },
            )
            .size_full(),
        );

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
    style: ScrollbarStyle,
    axis: Axis,
    bounds: Bounds<Pixels>,
) -> Option<(ScrollbarThumb, Bounds<Pixels>)> {
    let thumb = thumb_for_bounds(viewport, style, axis, bounds)?;
    let thumb_bounds = style.thumb_bounds(thumb, axis, bounds, state.is_expanded());

    Some((thumb, thumb_bounds))
}

fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    hsla(color.h, color.s, color.l, alpha.clamp(0.0, 1.0))
}

fn scrollbar_marker_top(position: f32, track_length: Pixels) -> Pixels {
    let max_top = (track_length - SCROLLBAR_MARKER_HEIGHT).max(px(0.0));
    (track_length * position.clamp(0.0, 1.0) - SCROLLBAR_MARKER_HEIGHT / 2.0)
        .clamp(px(0.0), max_top)
}

fn apply_scrollbar_pointer(
    viewport: &EditorViewport,
    on_scroll: Option<&ScrollCallback>,
    view_entity_id: EntityId,
    axis: Axis,
    cx: &mut App,
    geometry: ScrollbarPointerGeometry,
) {
    let scroll_position = geometry.style.scroll_position(
        geometry.bounds.size.along(axis),
        viewport.max_scroll_offset().along(axis),
        geometry.thumb,
        geometry.pointer,
        geometry.drag_offset,
    );
    apply_scrollbar_position(
        viewport,
        on_scroll,
        view_entity_id,
        axis,
        scroll_position,
        cx,
    );
}

fn apply_scrollbar_part(
    viewport: &EditorViewport,
    on_scroll: Option<&ScrollCallback>,
    view_entity_id: EntityId,
    axis: Axis,
    part: ScrollbarPart,
    cx: &mut App,
) {
    let position = part.scroll_position(
        viewport.scroll_position().along(axis),
        viewport.max_scroll_offset().along(axis),
        viewport.viewport_bounds().size.along(axis),
        viewport.line_height(),
    );
    apply_scrollbar_position(viewport, on_scroll, view_entity_id, axis, position, cx);
}

fn apply_scrollbar_position(
    viewport: &EditorViewport,
    on_scroll: Option<&ScrollCallback>,
    view_entity_id: EntityId,
    axis: Axis,
    scroll_position: Pixels,
    cx: &mut App,
) {
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
        scroll_position_for_scrollbar_pointer, scrollbar_marker_top,
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

    #[test]
    fn diagnostic_marker_position_is_centered_and_clamped_to_track() {
        assert_eq!(scrollbar_marker_top(0.5, px(200.0)), px(99.0));
        assert_eq!(scrollbar_marker_top(0.0, px(200.0)), px(0.0));
        assert_eq!(scrollbar_marker_top(1.0, px(200.0)), px(198.0));
    }

    #[gpui::test]
    fn windows_editor_scrollbar_pages_and_steps_on_both_axes(cx: &mut TestAppContext) {
        use gpui::{Along, Axis, Modifiers};
        use nucleotide_types::scrollbar::ScrollbarStyle;

        struct Harness {
            viewport: EditorViewport,
            state: EditorScrollbarState,
            axis: Axis,
        }
        impl gpui::Render for Harness {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                let (width, height, scrollbar) = if self.axis == Axis::Vertical {
                    (
                        12.0,
                        200.0,
                        EditorScrollbar::vertical(
                            cx.entity_id(),
                            self.viewport.clone(),
                            self.state.clone(),
                        ),
                    )
                } else {
                    (
                        200.0,
                        12.0,
                        EditorScrollbar::horizontal(
                            cx.entity_id(),
                            self.viewport.clone(),
                            self.state.clone(),
                        ),
                    )
                };
                div().w(px(width)).h(px(height)).child(scrollbar.with_style(
                    ScrollbarStyle::Windows,
                    gpui::black(),
                    None,
                ))
            }
        }
        for axis in [Axis::Vertical, Axis::Horizontal] {
            let mut viewport = EditorViewport::new(px(20.0));
            viewport.set_layout(px(20.0), size(px(200.0), px(200.0)), 50);
            viewport.set_content_width(px(1000.0));
            let (_, window) = cx.add_window_view(|_, _| Harness {
                viewport: viewport.clone(),
                state: EditorScrollbarState::default(),
                axis,
            });
            let pointer = |along| point(px(6.0), px(6.0)).apply_along(axis, |_| px(along));
            window.simulate_mouse_down(pointer(130.0), MouseButton::Left, Modifiers::none());
            assert_eq!(
                viewport.scroll_position().along(axis),
                px(200.0),
                "track press pages, not pointer-centres"
            );
            window.simulate_mouse_move(pointer(160.0), MouseButton::Left, Modifiers::none());
            assert_eq!(
                viewport.scroll_position().along(axis),
                px(200.0),
                "page press must not turn into dragging"
            );
            window.simulate_mouse_up(pointer(130.0), MouseButton::Left, Modifiers::none());
            window.simulate_mouse_down(pointer(195.0), MouseButton::Left, Modifiers::none());
            window.simulate_mouse_up(pointer(195.0), MouseButton::Left, Modifiers::none());
            assert_eq!(viewport.scroll_position().along(axis), px(220.0));
            window.simulate_mouse_down(pointer(5.0), MouseButton::Left, Modifiers::none());
            window.simulate_mouse_up(pointer(5.0), MouseButton::Left, Modifiers::none());
            assert_eq!(viewport.scroll_position().along(axis), px(200.0));

            window.simulate_mouse_down(pointer(60.0), MouseButton::Left, Modifiers::none());
            window.simulate_mouse_move(pointer(250.0), MouseButton::Left, Modifiers::none());
            window.simulate_mouse_up(pointer(250.0), MouseButton::Left, Modifiers::none());
            assert_eq!(viewport.scroll_position().along(axis), px(800.0));
            window.simulate_mouse_down(pointer(170.0), MouseButton::Left, Modifiers::none());
            window.simulate_mouse_move(pointer(-25.0), MouseButton::Left, Modifiers::none());
            window.simulate_mouse_up(pointer(-25.0), MouseButton::Left, Modifiers::none());
            assert_eq!(viewport.scroll_position().along(axis), px(0.0));
        }
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
