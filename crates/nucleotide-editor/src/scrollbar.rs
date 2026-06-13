// ABOUTME: Native editor scrollbar composed from GPUI element handlers
// ABOUTME: Keeps editor viewport scrolling inside nucleotide-editor

use std::{cell::Cell, rc::Rc};

use gpui::InteractiveElement as _;
use gpui::{
    App, Bounds, Component, EntityId, Hsla, IntoElement, MouseButton, ParentElement as _, Pixels,
    RenderOnce, Styled as _, Window, div, hsla, px,
};

use crate::{EditorViewport, ViewportScrollUpdate};

type ScrollCallback = Rc<dyn Fn(&EditorViewport, ViewportScrollUpdate, &mut App)>;

const TRACK_WIDTH: Pixels = px(12.0);
const THUMB_INSET: Pixels = px(3.0);
const MIN_THUMB_HEIGHT: Pixels = px(24.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorScrollbarThumb {
    pub top: Pixels,
    pub height: Pixels,
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
    on_scroll: Option<ScrollCallback>,
    track_color: Hsla,
    thumb_color: Hsla,
}

#[derive(Clone, Copy)]
struct ScrollbarPointerGeometry {
    bounds: Bounds<Pixels>,
    thumb: EditorScrollbarThumb,
    pointer_y: Pixels,
    drag_offset: Pixels,
}

impl EditorScrollbar {
    pub fn new(
        view_entity_id: EntityId,
        viewport: EditorViewport,
        state: EditorScrollbarState,
    ) -> Self {
        Self {
            view_entity_id,
            viewport,
            state,
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
    bounds: Bounds<Pixels>,
) -> Option<EditorScrollbarThumb> {
    editor_scrollbar_thumb(
        bounds.size.height,
        viewport.viewport_bounds().size.height,
        viewport.max_scroll_offset().height,
        viewport.scroll_position().y,
    )
}

impl EditorScrollbar {
    fn is_visible(&self) -> bool {
        editor_scrollbar_width(&self.viewport) > px(0.0)
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
            return empty_scrollbar_track();
        }

        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let on_scroll = self.on_scroll.clone();
        let view_entity_id = self.view_entity_id;
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

                let Some(thumb) = thumb_for_bounds(&viewport, bounds) else {
                    return;
                };
                let pointer_y = event.position.y - bounds.origin.y;
                let drag_offset = if pointer_y >= thumb.top && pointer_y <= thumb.top + thumb.height
                {
                    pointer_y - thumb.top
                } else {
                    thumb.height / 2.0
                };
                state.set_drag_offset(drag_offset);

                apply_scrollbar_pointer(
                    &viewport,
                    on_scroll.as_ref(),
                    view_entity_id,
                    cx,
                    ScrollbarPointerGeometry {
                        bounds,
                        thumb,
                        pointer_y,
                        drag_offset,
                    },
                );
            });

        let state = self.state.clone();
        let viewport = self.viewport.clone();
        let on_scroll = self.on_scroll.clone();
        let view_entity_id = self.view_entity_id;
        track = track.on_mouse_move(move |event, _window, cx| {
            if event.dragging() {
                let Some(drag_offset) = state.drag_offset() else {
                    return;
                };
                let Some(bounds) = state.track_bounds() else {
                    return;
                };
                let Some(thumb) = thumb_for_bounds(&viewport, bounds) else {
                    return;
                };

                apply_scrollbar_pointer(
                    &viewport,
                    on_scroll.as_ref(),
                    view_entity_id,
                    cx,
                    ScrollbarPointerGeometry {
                        bounds,
                        thumb,
                        pointer_y: event.position.y - bounds.origin.y,
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

        let track_height = self
            .state
            .track_bounds()
            .map(|bounds| bounds.size.height)
            .unwrap_or_else(|| self.viewport.viewport_bounds().size.height);
        if let Some(thumb) = editor_scrollbar_thumb(
            track_height,
            self.viewport.viewport_bounds().size.height,
            self.viewport.max_scroll_offset().height,
            self.viewport.scroll_position().y,
        ) {
            track = track.child(
                div()
                    .absolute()
                    .left(THUMB_INSET)
                    .top(thumb.top)
                    .w(TRACK_WIDTH - (THUMB_INSET * 2.0))
                    .h(thumb.height)
                    .bg(self.thumb_color),
            );
        }

        scrollbar_track()
            .on_children_prepainted({
                let state = self.state.clone();
                move |bounds, _window, _cx| {
                    state.set_track_bounds(bounds.into_iter().next());
                }
            })
            .child(track)
    }
}

fn empty_scrollbar_track() -> gpui::Div {
    scrollbar_track()
}

fn scrollbar_track() -> gpui::Div {
    div().relative().flex_shrink_0().w(TRACK_WIDTH).h_full()
}

fn apply_scrollbar_pointer(
    viewport: &EditorViewport,
    on_scroll: Option<&ScrollCallback>,
    view_entity_id: EntityId,
    cx: &mut App,
    geometry: ScrollbarPointerGeometry,
) {
    let scroll_y = scroll_position_for_scrollbar_pointer(
        geometry.bounds.size.height,
        viewport.max_scroll_offset().height,
        geometry.thumb,
        geometry.pointer_y,
        geometry.drag_offset,
    );
    let update = viewport.scroll_to_vertical_position_from_scrollbar(scroll_y);

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
    track_height: Pixels,
    viewport_height: Pixels,
    max_scroll_y: Pixels,
    scroll_y: Pixels,
) -> Option<EditorScrollbarThumb> {
    if track_height <= px(0.0) || viewport_height <= px(0.0) || max_scroll_y <= px(0.0) {
        return None;
    }

    let content_height = viewport_height + max_scroll_y;
    if content_height <= viewport_height {
        return None;
    }

    let thumb_height = (track_height * (viewport_height / content_height))
        .max(MIN_THUMB_HEIGHT)
        .min(track_height);
    let max_thumb_top = (track_height - thumb_height).max(px(0.0));
    let scroll_ratio = (scroll_y / max_scroll_y).clamp(0.0, 1.0);

    Some(EditorScrollbarThumb {
        top: max_thumb_top * scroll_ratio,
        height: thumb_height,
    })
}

pub fn editor_scrollbar_width(viewport: &EditorViewport) -> Pixels {
    if editor_scrollbar_thumb(
        viewport.viewport_bounds().size.height,
        viewport.viewport_bounds().size.height,
        viewport.max_scroll_offset().height,
        viewport.scroll_position().y,
    )
    .is_some()
    {
        TRACK_WIDTH
    } else {
        px(0.0)
    }
}

pub fn scroll_position_for_scrollbar_pointer(
    track_height: Pixels,
    max_scroll_y: Pixels,
    thumb: EditorScrollbarThumb,
    pointer_y: Pixels,
    drag_offset: Pixels,
) -> Pixels {
    let max_thumb_top = (track_height - thumb.height).max(px(0.0));
    if max_thumb_top <= px(0.0) || max_scroll_y <= px(0.0) {
        return px(0.0);
    }

    let thumb_top = (pointer_y - drag_offset).clamp(px(0.0), max_thumb_top);
    max_scroll_y * (thumb_top / max_thumb_top)
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
                top: px(0.0),
                height: px(40.0),
            }
        );
    }

    #[test]
    fn thumb_tracks_scroll_ratio() {
        let thumb = editor_scrollbar_thumb(px(200.0), px(200.0), px(800.0), px(400.0)).unwrap();

        assert_eq!(
            thumb,
            EditorScrollbarThumb {
                top: px(80.0),
                height: px(40.0),
            }
        );
    }

    #[test]
    fn thumb_has_minimum_height() {
        let thumb = editor_scrollbar_thumb(px(200.0), px(20.0), px(1980.0), px(0.0)).unwrap();

        assert_eq!(thumb.height, px(24.0));
    }

    #[test]
    fn pointer_position_maps_to_scroll_position() {
        let thumb = EditorScrollbarThumb {
            top: px(0.0),
            height: px(40.0),
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
                    .child(EditorScrollbar::new(
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
