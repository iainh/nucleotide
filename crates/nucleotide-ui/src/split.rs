//! Unified split and resize components for GPUI
//! Provides reusable sidebar, bottom panel, and two-pane split with consistent drag behavior

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Div, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Stateful, Styled, Window, div, px, relative,
};

pub const SPLITTER_HITBOX_PX: f32 = 10.0;
pub const SPLITTER_LINE_PX: f32 = 1.0;

const RESIZE_HANDLE_MIN_HITBOX_PX: f32 = 8.0;
const RESIZE_HANDLE_MAX_HITBOX_PX: f32 = 12.0;
const RESIZE_HANDLE_VISUAL_PX: f32 = SPLITTER_LINE_PX;

#[inline]
fn clamp_primary(start: f32, delta: f32, min_px: f32, max_px: f32) -> f32 {
    (start + delta).clamp(min_px, max_px)
}

#[inline]
fn clamp_primary_vertical(start: f32, delta_y: f32, min_px: f32, max_px: f32) -> f32 {
    // Top-handle dragging upward increases height => subtract dy
    (start - delta_y).clamp(min_px, max_px)
}

#[inline]
fn resize_handle_hitbox_px(handle_px: f32) -> Option<f32> {
    if handle_px <= 0.0 {
        None
    } else {
        Some(handle_px.clamp(RESIZE_HANDLE_MIN_HITBOX_PX, RESIZE_HANDLE_MAX_HITBOX_PX))
    }
}

#[inline]
fn resize_handle_visual_offset(hitbox_px: f32) -> f32 {
    ((hitbox_px - RESIZE_HANDLE_VISUAL_PX) * 0.5).max(0.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitterAxis {
    Vertical,
    Horizontal,
}

/// A single splitter component: a transparent, symmetric drag hitbox with one
/// centered visible separator line.
pub fn splitter(id: impl Into<ElementId>, axis: SplitterAxis, handle_px: f32) -> Stateful<Div> {
    let hitbox_px = resize_handle_hitbox_px(handle_px).unwrap_or(RESIZE_HANDLE_MIN_HITBOX_PX);
    let visual_offset = resize_handle_visual_offset(hitbox_px);
    let theme = crate::providers::ProviderHooks::theme();
    let sep_color = crate::tokens::with_alpha(theme.tokens.chrome.separator_color, 0.7);
    let sep_hover_color = crate::tokens::with_alpha(theme.tokens.editor.focus_ring, 0.55);

    let base = div()
        .id(id)
        .relative()
        .occlude()
        .when(axis == SplitterAxis::Vertical, |d| {
            d.w(px(hitbox_px))
                .h_full()
                .cursor(gpui::CursorStyle::ResizeLeftRight)
        })
        .when(axis == SplitterAxis::Horizontal, |d| {
            d.w_full()
                .h(px(hitbox_px))
                .cursor(gpui::CursorStyle::ResizeRow)
        });

    match axis {
        SplitterAxis::Vertical => base.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(visual_offset))
                .w(px(RESIZE_HANDLE_VISUAL_PX))
                .bg(sep_color)
                .hover(move |d| d.bg(sep_hover_color)),
        ),
        SplitterAxis::Horizontal => base.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(visual_offset))
                .h(px(RESIZE_HANDLE_VISUAL_PX))
                .bg(sep_color)
                .hover(move |d| d.bg(sep_hover_color)),
        ),
    }
}

#[derive(Clone)]
struct DragRuntimeState {
    dragging: Rc<Cell<bool>>,
    start_mouse: Rc<Cell<(f32, f32)>>,
    start_primary: Rc<Cell<f32>>, // width for horizontal, height for vertical
}

impl DragRuntimeState {
    fn new() -> Self {
        Self {
            dragging: Rc::new(Cell::new(false)),
            start_mouse: Rc::new(Cell::new((0.0, 0.0))),
            start_primary: Rc::new(Cell::new(0.0)),
        }
    }
}

/// A sidebar split with a fixed-width left pane and flexible right pane.
/// - `width_px`: current left pane width in pixels
/// - `min_px`/`max_px`: constraints for the left pane
/// - `handle_px`: drag handle hit target; the visible separator remains 1px
/// - `on_change`: callback invoked with the new width during drag
/// - `default_px`: width to snap to on double-click
#[allow(clippy::too_many_arguments)]
pub fn sidebar_split<L: IntoElement, R: IntoElement>(
    width_px: f32,
    min_px: f32,
    max_px: f32,
    handle_px: f32,
    default_px: f32,
    on_change: impl Fn(f32, &mut App) + 'static,
    left: L,
    right: R,
) -> impl IntoElement {
    let drag = DragRuntimeState::new();
    let min_px = min_px.max(0.0);
    let max_px = if max_px < min_px { min_px } else { max_px };
    let width_px = width_px.clamp(min_px, max_px);
    let on_change = Rc::new(on_change);

    // debug logging removed

    // The container captures move/up to provide robust dragging beyond the handle bounds
    let mut root = div()
        .flex()
        .relative()
        .w_full()
        .flex_1()
        .min_h(px(0.0)) // allow vertical shrink inside column parents
        .on_mouse_move({
            let drag = drag.clone();
            let on_change = on_change.clone();
            move |ev: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
                if drag.dragging.get() && ev.dragging() {
                    let start_x = drag.start_mouse.get().0;
                    let dx = f32::from(ev.position.x) - start_x;
                    let start_w = drag.start_primary.get();
                    // Ensure a minimum width for right pane (200px) so editor never collapses
                    let viewport_w = f32::from(window.viewport_size().width);
                    let max_allowed = (viewport_w - 200.0).max(min_px);
                    let new_w = clamp_primary(start_w, dx, min_px, max_allowed.min(max_px));
                    on_change(new_w, cx);
                    window.refresh();
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                if drag.dragging.get() {
                    drag.dragging.set(false);
                    window.refresh();
                }
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                if drag.dragging.get() {
                    drag.dragging.set(false);
                    window.refresh();
                }
            }
        });

    // Left pane: fixed width, vertically scrollable if content exceeds available height
    root = root.child(
        div()
            .w(px(width_px))
            .h_full()
            .flex_shrink_0()
            .min_h(px(0.0))
            .child(
                div()
                    .id("sidebar-left")
                    .w_full()
                    .h_full()
                    .min_h(px(0.0))
                    // Do not set overflow here; let inner views (e.g., uniform_list) manage scrolling
                    .child(left),
            ),
    );

    // Handle: transparent hitbox centered over the pane boundary, with a
    // single visible separator line centered inside.
    let handle_hit_w = resize_handle_hitbox_px(handle_px).unwrap_or(RESIZE_HANDLE_MIN_HITBOX_PX);

    root = root.child({
        let mut handle = splitter("sidebar-resize-handle", SplitterAxis::Vertical, handle_px)
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(width_px - handle_hit_w * 0.5));

        handle = handle.on_mouse_down(MouseButton::Left, {
            let drag = drag.clone();
            let on_change = on_change.clone();
            move |ev: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                if ev.click_count >= 2 {
                    let viewport_w = f32::from(window.viewport_size().width);
                    let max_allowed = (viewport_w - 200.0).max(min_px);
                    on_change(default_px.clamp(min_px, max_allowed.min(max_px)), cx);
                    window.refresh();
                    cx.stop_propagation();
                    return;
                }
                drag.dragging.set(true);
                drag.start_mouse
                    .set((f32::from(ev.position.x), f32::from(ev.position.y)));
                drag.start_primary.set(width_px);
                window.refresh();
                cx.stop_propagation();
            }
        });

        handle
    });

    // Right pane fills remaining space; do not allow it to overflow its box
    root.child(
        div()
            .flex_1()
            .h_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(right),
    )
}

/// A bottom-docked panel with a draggable top edge.
/// - `height_px`: current panel height
/// - `min_px`/`max_px`: constraints for the panel height
/// - `handle_px`: drag handle hit target at the top; the visible separator remains 1px.
///   Use `0.0` to suppress the built-in handle.
/// - `on_change`: callback invoked with new height during drag
/// - `default_px`: height to snap to on double-click
pub fn bottom_panel_split<C: IntoElement>(
    height_px: f32,
    min_px: f32,
    max_px: f32,
    handle_px: f32,
    default_px: f32,
    on_change: impl Fn(f32, &mut App) + 'static,
    content: C,
) -> impl IntoElement {
    let drag = DragRuntimeState::new();
    let min_px = min_px.max(0.0);
    let max_px = if max_px < min_px { min_px } else { max_px };
    let height_px = height_px.clamp(min_px, max_px);
    let on_change = Rc::new(on_change);

    let mut root = div()
        .relative()
        .size_full()
        .on_mouse_move({
            let drag = drag.clone();
            let on_change = on_change.clone();
            move |ev: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
                if drag.dragging.get() && ev.dragging() {
                    let start_y = drag.start_mouse.get().1;
                    let dy = f32::from(ev.position.y) - start_y;
                    let start_h = drag.start_primary.get();
                    let new_h = clamp_primary_vertical(start_h, dy, min_px, max_px);
                    on_change(new_h, cx);
                    window.refresh();
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                if drag.dragging.get() {
                    drag.dragging.set(false);
                    window.refresh();
                }
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                if drag.dragging.get() {
                    drag.dragging.set(false);
                    window.refresh();
                }
            }
        });

    // Panel shell at bottom
    let mut panel = div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(height_px))
        .child(content);

    if let Some(handle_hit_h) = resize_handle_hitbox_px(handle_px) {
        panel = panel.child({
            splitter(
                "bottom-panel-resize-handle",
                SplitterAxis::Horizontal,
                handle_px,
            )
            .absolute()
            .left_0()
            .right_0()
            .top(px(-handle_hit_h * 0.5))
            .on_mouse_down(MouseButton::Left, {
                let drag = drag.clone();
                let on_change = on_change.clone();
                move |ev: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    if ev.click_count >= 2 {
                        on_change(default_px.clamp(min_px, max_px), cx);
                        window.refresh();
                        cx.stop_propagation();
                        return;
                    }
                    drag.dragging.set(true);
                    drag.start_mouse
                        .set((f32::from(ev.position.x), f32::from(ev.position.y)));
                    drag.start_primary.set(height_px);
                    window.refresh();
                    cx.stop_propagation();
                }
            })
        });
    }

    root = root.child(panel);

    root
}

/// A two-pane adjustable split with a draggable divider.
/// Uses a fraction in [0,1] for the first pane size along the axis.
#[allow(clippy::too_many_arguments)]
pub fn two_pane_split<A: IntoElement, B: IntoElement>(
    horizontal: bool,
    fraction: f32,
    _min_a_px: f32,
    _min_b_px: f32,
    handle_px: f32,
    on_change_fraction: impl Fn(f32, &mut App) + 'static,
    a: A,
    b: B,
) -> impl IntoElement {
    let drag = DragRuntimeState::new();
    let handle_px = handle_px.max(2.0);
    let fraction = fraction.clamp(0.0, 1.0);

    let mut root = div().relative().size_full();

    root = root.on_mouse_up(MouseButton::Left, {
        let drag = drag.clone();
        move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
            if drag.dragging.get() {
                drag.dragging.set(false);
                window.refresh();
            }
        }
    });

    root = root.on_mouse_up_out(MouseButton::Left, {
        let drag = drag.clone();
        move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
            if drag.dragging.get() {
                drag.dragging.set(false);
                window.refresh();
            }
        }
    });

    // Divider element for pointer semantics; layout specifics are up to caller
    root = root.child(
        splitter(
            "two-pane-resize-handle",
            if horizontal {
                SplitterAxis::Vertical
            } else {
                SplitterAxis::Horizontal
            },
            handle_px,
        )
        .absolute()
        .when(horizontal, |d| {
            d.left(relative(fraction))
                .top_0()
                .bottom_0()
                .ml(px(-handle_px * 0.5))
        })
        .when(!horizontal, |d| {
            d.top(relative(fraction))
                .left_0()
                .right_0()
                .mt(px(-handle_px * 0.5))
        })
        .on_mouse_down(MouseButton::Left, {
            let drag = drag.clone();
            move |ev: &MouseDownEvent, window: &mut Window, _cx: &mut App| {
                drag.dragging.set(true);
                drag.start_mouse
                    .set((f32::from(ev.position.x), f32::from(ev.position.y)));
                window.refresh();
            }
        })
        .on_mouse_move({
            let _drag = drag.clone();
            let on_change_fraction = Rc::new(on_change_fraction);
            move |_ev: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
                on_change_fraction(fraction, cx);
                window.refresh();
            }
        }),
    );

    root.child(a).child(b)
}

#[cfg(test)]
mod tests {
    use super::{
        RESIZE_HANDLE_MAX_HITBOX_PX, RESIZE_HANDLE_MIN_HITBOX_PX, clamp_primary,
        clamp_primary_vertical, resize_handle_hitbox_px, resize_handle_visual_offset,
    };

    #[test]
    fn test_clamp_primary_horizontal() {
        assert_eq!(clamp_primary(200.0, 50.0, 150.0, 600.0), 250.0);
        assert_eq!(clamp_primary(160.0, -20.0, 150.0, 600.0), 150.0);
        assert_eq!(clamp_primary(580.0, 50.0, 150.0, 600.0), 600.0);
    }

    #[test]
    fn test_clamp_primary_vertical() {
        // Negative dy (mouse moved up) increases height
        assert_eq!(clamp_primary_vertical(200.0, -20.0, 80.0, 800.0), 220.0);
        // Positive dy (mouse moved down) decreases height
        assert_eq!(clamp_primary_vertical(200.0, 50.0, 80.0, 800.0), 150.0);
        // Clamp to min/max
        assert_eq!(clamp_primary_vertical(90.0, 30.0, 80.0, 800.0), 80.0);
        assert_eq!(clamp_primary_vertical(790.0, -30.0, 80.0, 800.0), 800.0);
    }

    #[test]
    fn resize_handle_metrics_keep_hitbox_larger_than_visual_separator() {
        assert_eq!(resize_handle_hitbox_px(0.0), None);
        assert_eq!(
            resize_handle_hitbox_px(4.0),
            Some(RESIZE_HANDLE_MIN_HITBOX_PX)
        );
        assert_eq!(
            resize_handle_hitbox_px(20.0),
            Some(RESIZE_HANDLE_MAX_HITBOX_PX)
        );
        assert_eq!(resize_handle_visual_offset(8.0), 3.5);
    }
}
