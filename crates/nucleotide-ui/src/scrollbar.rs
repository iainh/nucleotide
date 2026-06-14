//! Scrollbar component for GPUI based on Zed's implementation
//! Provides vertical and horizontal scrollbars for scrollable content

use std::{any::Any, cell::Cell, fmt::Debug, ops::Range, rc::Rc, sync::Arc};

use gpui::prelude::{InteractiveElement, ParentElement, StatefulInteractiveElement, Styled};
use gpui::{
    Along, App, Axis, Bounds, Div, IntoElement, IsZero, MouseButton, Pixels, Point, RenderOnce,
    ScrollHandle, Size, UniformListScrollHandle, Window, div, hsla, px,
};

/// A scrollbar component that can be attached to scrollable content
#[derive(IntoElement)]
pub struct Scrollbar {
    thumb: Range<f32>,
    state: ScrollbarState,
    axis: Axis,
}

#[derive(Default, Debug, Clone, Copy)]
enum ThumbState {
    #[default]
    Inactive,
    Hover,
    Dragging(Pixels),
}

impl ThumbState {
    fn is_dragging(&self) -> bool {
        matches!(*self, ThumbState::Dragging(_))
    }

    fn is_active(&self) -> bool {
        matches!(*self, ThumbState::Hover | ThumbState::Dragging(_))
    }
}

/// Trait for objects that can be scrolled by a scrollbar
pub trait ScrollableHandle: Any + Debug {
    /// Get the total content size
    fn content_size(&self) -> Size<Pixels> {
        self.viewport().size + self.max_offset()
    }

    /// Get the maximum scroll offset
    fn max_offset(&self) -> Size<Pixels>;

    /// Set the current scroll offset
    fn set_offset(&self, point: Point<Pixels>);

    /// Get the current scroll offset
    fn offset(&self) -> Point<Pixels>;

    /// Get the viewport bounds
    fn viewport(&self) -> Bounds<Pixels>;

    /// Called when dragging starts
    fn drag_started(&self) {}

    /// Called when dragging ends
    fn drag_ended(&self) {}
}

impl ScrollableHandle for ScrollHandle {
    fn max_offset(&self) -> Size<Pixels> {
        self.max_offset().into()
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.set_offset(point);
    }

    fn offset(&self) -> Point<Pixels> {
        self.offset()
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.bounds()
    }
}

impl ScrollableHandle for UniformListScrollHandle {
    fn max_offset(&self) -> Size<Pixels> {
        self.0.borrow().base_handle.max_offset().into()
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.0.borrow().base_handle.set_offset(point);
    }

    fn offset(&self) -> Point<Pixels> {
        self.0.borrow().base_handle.offset()
    }

    fn viewport(&self) -> Bounds<Pixels> {
        self.0.borrow().base_handle.bounds()
    }
}

/// Scrollbar state that should be persisted across frames
#[derive(Clone, Debug)]
pub struct ScrollbarState {
    thumb_state: Rc<Cell<ThumbState>>,
    track_hovered: Rc<Cell<bool>>,
    track_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    scroll_handle: Arc<dyn ScrollableHandle>,
}

const SCROLLBAR_THICKNESS: Pixels = px(12.);
const EXTRA_PADDING: Pixels = px(2.);

impl ScrollbarState {
    pub fn new(scroll: impl ScrollableHandle) -> Self {
        Self {
            thumb_state: Rc::default(),
            track_hovered: Rc::default(),
            track_bounds: Rc::default(),
            scroll_handle: Arc::new(scroll),
        }
    }

    pub fn scroll_handle(&self) -> &Arc<dyn ScrollableHandle> {
        &self.scroll_handle
    }

    pub fn is_dragging(&self) -> bool {
        matches!(self.thumb_state.get(), ThumbState::Dragging(_))
    }

    fn set_dragging(&self, drag_offset: Pixels) {
        self.set_thumb_state(ThumbState::Dragging(drag_offset));
        self.scroll_handle.drag_started();
    }

    fn set_thumb_hovered(&self, hovered: bool) {
        self.set_thumb_state(if hovered {
            ThumbState::Hover
        } else {
            ThumbState::Inactive
        });
    }

    fn set_thumb_state(&self, state: ThumbState) {
        self.thumb_state.set(state);
    }

    fn set_track_hovered(&self, hovered: bool) {
        self.track_hovered.set(hovered);
    }

    fn set_track_bounds(&self, bounds: Option<Bounds<Pixels>>) {
        self.track_bounds.set(bounds);
    }

    fn track_bounds(&self) -> Option<Bounds<Pixels>> {
        self.track_bounds.get()
    }

    fn is_expanded(&self) -> bool {
        self.track_hovered.get() || self.thumb_state.get().is_active()
    }

    /// Get target values based on current state
    fn target_values(&self) -> (f32, f32) {
        let thumb_state = self.thumb_state.get();
        let is_expanded = self.is_expanded();

        // Target width ratio
        let target_width = if is_expanded { 0.70 } else { 0.35 };

        // Target alpha based on state
        let target_alpha = match thumb_state {
            ThumbState::Dragging(_) => 0.75,
            ThumbState::Hover => 0.60,
            ThumbState::Inactive if is_expanded => 0.45,
            ThumbState::Inactive => 0.25,
        };

        (target_width, target_alpha)
    }

    fn thumb_range(&self, axis: Axis) -> Option<Range<f32>> {
        const MINIMUM_THUMB_SIZE: Pixels = px(20.); // Minimum thumb size
        let max_offset = self.scroll_handle.max_offset().along(axis);
        let viewport_size = self.scroll_handle.viewport().size.along(axis);

        // If content fits entirely, don't show scrollbar
        if max_offset.is_zero() {
            return None;
        }

        if viewport_size.is_zero() {
            return None;
        }

        let content_size = viewport_size + max_offset;
        let visible_percentage = viewport_size / content_size;
        let thumb_size = MINIMUM_THUMB_SIZE.max(viewport_size * visible_percentage);

        // Allow thumb even if it's large relative to viewport
        let thumb_size = thumb_size.min(viewport_size * 0.95); // Cap at 95% of viewport

        let raw_offset = self.scroll_handle.offset();
        let offset_along_axis = raw_offset.along(axis);

        // GPUI convention: offsets are negative when scrolled (scrolling down = negative y)
        // Clamp between -max_offset and 0, then take absolute value for calculations
        let current_offset = offset_along_axis.clamp(-max_offset, Pixels::ZERO).abs();

        // Handle division by zero
        let max_offset_value = f32::from(max_offset);
        let start_offset = if max_offset_value > 0.0 {
            (current_offset / max_offset) * (viewport_size - thumb_size)
        } else {
            px(0.0)
        };

        let thumb_percentage_start = start_offset / viewport_size;
        let thumb_percentage_end = (start_offset + thumb_size) / viewport_size;

        Some(thumb_percentage_start..thumb_percentage_end)
    }
}

impl Scrollbar {
    pub fn vertical(state: ScrollbarState) -> Option<Self> {
        Self::new(state, Axis::Vertical)
    }

    pub fn horizontal(state: ScrollbarState) -> Option<Self> {
        Self::new(state, Axis::Horizontal)
    }

    fn new(state: ScrollbarState, axis: Axis) -> Option<Self> {
        // Always create the scrollbar element so it can react once
        // layout information is available. Paint will short‑circuit
        // if scrolling isn't required.
        let thumb = state.thumb_range(axis).unwrap_or(0.0..1.0);
        Some(Self { thumb, state, axis })
    }
}

impl RenderOnce for Scrollbar {
    fn render(mut self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(thumb) = self.state.thumb_range(self.axis) else {
            self.state.set_track_bounds(None);
            return empty_scrollbar_track(self.axis);
        };
        self.thumb = thumb;

        let (width_ratio, alpha) = self.state.target_values();
        let (thumb_bg, gutter_bg) = if let Some(theme) = cx.try_global::<crate::Theme>() {
            let chrome = &theme.tokens.chrome;
            (
                crate::styling::ColorTheory::with_alpha(chrome.text_on_chrome, alpha),
                crate::styling::ColorTheory::with_alpha(chrome.separator_color, 0.0),
            )
        } else {
            (hsla(0.0, 0.0, 0.8, alpha), hsla(0.0, 0.0, 0.0, 0.0))
        };

        let track_len = self
            .state
            .track_bounds()
            .map(|bounds| bounds.size.along(self.axis))
            .unwrap_or_else(|| self.state.scroll_handle().viewport().size.along(self.axis))
            .max(px(0.0));
        let visual = scrollbar_visual(self.thumb.clone(), track_len, width_ratio);
        let state = self.state.clone();
        let axis = self.axis;
        let state_id = Rc::as_ptr(&self.state.track_hovered) as usize;

        let mut track = scrollbar_track(self.axis)
            .id(("scrollbar-track", state_id))
            .bg(gutter_bg)
            .cursor_pointer()
            .on_hover({
                let state = state.clone();
                move |hovered, window, _cx| {
                    state.set_track_hovered(*hovered);
                    if !*hovered && !state.is_dragging() {
                        state.set_thumb_hovered(false);
                    }
                    window.refresh();
                }
            })
            .on_mouse_down(MouseButton::Left, {
                let state = state.clone();
                move |event, window, cx| {
                    let Some(bounds) = state.track_bounds() else {
                        return;
                    };
                    let Some(thumb_bounds) = thumb_bounds_for_track(&state, axis, bounds) else {
                        return;
                    };

                    if thumb_bounds.contains(&event.position) {
                        let offset = event.position.along(axis) - thumb_bounds.origin.along(axis);
                        state.set_dragging(offset);
                    } else {
                        let scroll_handle = state.scroll_handle();
                        let click_offset = scroll_offset_for_pointer(
                            axis,
                            bounds,
                            thumb_bounds,
                            scroll_handle.max_offset(),
                            event.position,
                            thumb_bounds.size.along(axis) / 2.0,
                        );
                        scroll_handle
                            .set_offset(scroll_handle.offset().apply_along(axis, |_| click_offset));
                    }

                    window.refresh();
                    cx.stop_propagation();
                }
            })
            .on_mouse_move({
                let state = state.clone();
                move |event, window, cx| {
                    let Some(bounds) = state.track_bounds() else {
                        return;
                    };
                    let Some(thumb_bounds) = thumb_bounds_for_track(&state, axis, bounds) else {
                        return;
                    };

                    if state.thumb_state.get().is_dragging()
                        && event.dragging()
                        && let ThumbState::Dragging(drag_offset) = state.thumb_state.get()
                    {
                        let scroll_handle = state.scroll_handle();
                        let pointer_offset = scroll_offset_for_pointer(
                            axis,
                            bounds,
                            thumb_bounds,
                            scroll_handle.max_offset(),
                            event.position,
                            drag_offset,
                        );
                        scroll_handle.set_offset(
                            scroll_handle.offset().apply_along(axis, |_| pointer_offset),
                        );
                        window.refresh();
                        cx.stop_propagation();
                        return;
                    }

                    if event.pressed_button.is_none() {
                        let over_thumb = thumb_bounds.contains(&event.position);
                        let was_thumb_hover = matches!(state.thumb_state.get(), ThumbState::Hover);
                        state.set_thumb_hovered(over_thumb);
                        if over_thumb != was_thumb_hover {
                            window.refresh();
                        }
                    }
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let state = state.clone();
                move |event, window, cx| {
                    if state.is_dragging() {
                        state.scroll_handle().drag_ended();
                    }

                    if let Some(bounds) = state.track_bounds() {
                        state.set_track_hovered(bounds.contains(&event.position));
                        let over_thumb = thumb_bounds_for_track(&state, axis, bounds)
                            .is_some_and(|thumb_bounds| thumb_bounds.contains(&event.position));
                        state.set_thumb_hovered(over_thumb);
                    } else {
                        state.set_track_hovered(false);
                        state.set_thumb_hovered(false);
                    }

                    window.refresh();
                    cx.stop_propagation();
                }
            })
            .on_mouse_up_out(MouseButton::Left, {
                let state = state.clone();
                move |_event, window, cx| {
                    if state.is_dragging() {
                        state.scroll_handle().drag_ended();
                    }

                    state.set_track_hovered(false);
                    state.set_thumb_hovered(false);
                    window.refresh();
                    cx.stop_propagation();
                }
            })
            .on_scroll_wheel({
                let scroll_handle = self.state.scroll_handle().clone();
                move |event, window, cx| {
                    let current_offset = scroll_handle.offset();
                    scroll_handle
                        .set_offset(current_offset + event.delta.pixel_delta(window.line_height()));
                    window.refresh();
                    cx.stop_propagation();
                }
            });

        if self.axis == Axis::Vertical {
            track = track.child(
                div()
                    .absolute()
                    .top(visual.along_offset)
                    .left(visual.cross_offset)
                    .w(visual.cross_size)
                    .h(visual.along_size)
                    .rounded(visual.cross_size / 2.0)
                    .bg(thumb_bg),
            );
        } else {
            track = track.child(
                div()
                    .absolute()
                    .left(visual.along_offset)
                    .top(visual.cross_offset)
                    .w(visual.along_size)
                    .h(visual.cross_size)
                    .rounded(visual.cross_size / 2.0)
                    .bg(thumb_bg),
            );
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

struct ScrollbarVisual {
    along_offset: Pixels,
    along_size: Pixels,
    cross_offset: Pixels,
    cross_size: Pixels,
}

fn empty_scrollbar_track(axis: Axis) -> Div {
    scrollbar_track(axis)
}

fn scrollbar_track(axis: Axis) -> Div {
    let base = div().relative().flex_shrink_0();
    if axis == Axis::Vertical {
        base.w(SCROLLBAR_THICKNESS).h_full()
    } else {
        base.w_full().h(SCROLLBAR_THICKNESS)
    }
}

fn scrollbar_visual(thumb: Range<f32>, track_len: Pixels, width_ratio: f32) -> ScrollbarVisual {
    let padded_len = (track_len - EXTRA_PADDING * 2.0).max(px(0.0));
    let along_offset = EXTRA_PADDING + thumb.start * padded_len;
    let along_size = ((thumb.end - thumb.start) * padded_len).max(px(0.0));
    let cross_len = SCROLLBAR_THICKNESS;
    let cross_size = cross_len * width_ratio;
    let cross_offset = (cross_len - cross_size) / 2.0;

    ScrollbarVisual {
        along_offset,
        along_size,
        cross_offset,
        cross_size,
    }
}

fn thumb_bounds_for_track(
    state: &ScrollbarState,
    axis: Axis,
    bounds: Bounds<Pixels>,
) -> Option<Bounds<Pixels>> {
    let thumb = state.thumb_range(axis)?;
    let (width_ratio, _) = state.target_values();
    let visual = scrollbar_visual(thumb, bounds.size.along(axis), width_ratio);

    Some(Bounds::new(
        bounds
            .origin
            .apply_along(axis, |origin| origin + visual.along_offset)
            .apply_along(axis.invert(), |origin| origin + visual.cross_offset),
        bounds
            .size
            .apply_along(axis, |_| visual.along_size)
            .apply_along(axis.invert(), |_| visual.cross_size),
    ))
}

fn scroll_offset_for_pointer(
    axis: Axis,
    track_bounds: Bounds<Pixels>,
    thumb_bounds: Bounds<Pixels>,
    max_offset: Size<Pixels>,
    event_position: Point<Pixels>,
    thumb_offset: Pixels,
) -> Pixels {
    let viewport_size = (track_bounds.size.along(axis) - EXTRA_PADDING * 2.0).max(px(0.0));
    let thumb_size = thumb_bounds.size.along(axis);
    let max_thumb_start = (viewport_size - thumb_size).max(px(0.0));
    let thumb_start = (event_position.along(axis)
        - track_bounds.origin.along(axis)
        - EXTRA_PADDING
        - thumb_offset)
        .clamp(px(0.), max_thumb_start);
    let percentage = if max_thumb_start > px(0.0) {
        thumb_start / max_thumb_start
    } else {
        0.0
    };

    -max_offset.along(axis) * percentage
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;

    #[test]
    fn scrollbar_visual_applies_track_padding() {
        let visual = scrollbar_visual(0.25..0.75, px(104.0), 0.5);

        assert_eq!(visual.along_offset, px(27.0));
        assert_eq!(visual.along_size, px(50.0));
        assert_eq!(visual.cross_offset, px(3.0));
        assert_eq!(visual.cross_size, px(6.0));
    }

    #[test]
    fn scroll_offset_for_pointer_uses_negative_gpui_offsets() {
        let track_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(12.0), px(104.0)));
        let thumb_bounds = Bounds::new(point(px(3.0), px(2.0)), size(px(6.0), px(20.0)));

        let offset = scroll_offset_for_pointer(
            Axis::Vertical,
            track_bounds,
            thumb_bounds,
            size(px(0.0), px(200.0)),
            point(px(6.0), px(52.0)),
            px(10.0),
        );

        assert_eq!(offset, px(-100.0));
    }
}
