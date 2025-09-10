//! Unified split and resize components for GPUI
//! Provides reusable sidebar, bottom panel, and two-pane split with consistent drag behavior

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Styled, Window, div, px,
};
use nucleotide_logging::info;

#[inline]
fn clamp_primary(start: f32, delta: f32, min_px: f32, max_px: f32) -> f32 {
    (start + delta).clamp(min_px, max_px)
}

#[inline]
fn clamp_primary_vertical(start: f32, delta_y: f32, min_px: f32, max_px: f32) -> f32 {
    // Top-handle dragging upward increases height => subtract dy
    (start - delta_y).clamp(min_px, max_px)
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
/// - `handle_px`: drag handle thickness (visual/hit)
/// - `on_change`: callback invoked with the new width during drag
/// - `default_px`: width to snap to on double-click
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

    info!(
        prefix = "[SPLIT_DBG]",
        width = width_px,
        min = min_px,
        max = max_px,
        handle_px = handle_px,
        default = default_px,
        "sidebar_split: init"
    );

    // The container captures move/up to provide robust dragging beyond the handle bounds
    let mut root = div()
        .flex()
        .w_full()
        .flex_1()
        .min_h(px(0.0)) // allow vertical shrink inside column parents
        .on_mouse_move({
            let drag = drag.clone();
            let on_change = on_change.clone();
            move |ev: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
                if drag.dragging.get() && ev.dragging() {
                    let start_x = drag.start_mouse.get().0;
                    let dx = ev.position.x.0 - start_x;
                    let start_w = drag.start_primary.get();
                    // Ensure a minimum width for right pane (200px) so editor never collapses
                    let viewport_w = window.viewport_size().width.0;
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
        });

    // Visuals
    let _handle_visual_bg: Option<Hsla> = None; // keep neutral; borders used in caller’s theme

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

    // Handle: wider hitbox for easy grabbing, with a thin visible separator centered inside
    let handle_visual_w = handle_px.max(2.0);
    let handle_hit_w = (handle_visual_w + 6.0).min(12.0); // 6px padding each side, cap at 12px

    root = root.child({
        // Separator color (neutral gray)
        let sep_color = gpui::hsla(0.0, 0.0, 0.55, 0.6);

        let pad = ((handle_hit_w - handle_visual_w) * 0.5).max(0.0);

        let mut handle = div()
            .id("sidebar-resize-handle")
            .w(px(handle_hit_w))
            .h_full()
            .flex_shrink_0()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .pl(px(pad))
            .pr(px(pad))
            // Visible thin separator centered in the hitbox
            .child(
                div()
                    .w(px(handle_visual_w))
                    .h_full()
                    .bg(sep_color)
                    .hover(|d| d.bg(gpui::hsla(0.0, 0.0, 0.55, 0.9))),
            );

        handle = handle.on_mouse_down(MouseButton::Left, {
            let drag = drag.clone();
            let on_change = on_change.clone();
            move |ev: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                if ev.click_count >= 2 {
                    let viewport_w = window.viewport_size().width.0;
                    let max_allowed = (viewport_w - 200.0).max(min_px);
                    on_change(default_px.clamp(min_px, max_allowed.min(max_px)), cx);
                    window.refresh();
                    cx.stop_propagation();
                    return;
                }
                drag.dragging.set(true);
                drag.start_mouse.set((ev.position.x.0, ev.position.y.0));
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
/// - `handle_px`: thickness of the drag handle at the top
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
                    let dy = ev.position.y.0 - start_y;
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
        });

    // Panel shell at bottom
    root = root.child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(height_px))
            .border_t_1()
            .child({
                // Top drag handle
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(px(handle_px.max(2.0)))
                    .cursor(gpui::CursorStyle::ResizeRow)
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
                            drag.start_mouse.set((ev.position.x.0, ev.position.y.0));
                            drag.start_primary.set(height_px);
                            window.refresh();
                            cx.stop_propagation();
                        }
                    })
            })
            .child(content),
    );

    root
}

/// A two-pane adjustable split with a draggable divider.
/// Uses a fraction in [0,1] for the first pane size along the axis.
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

    // Divider element for pointer semantics; layout specifics are up to caller
    root = root.child(
        div()
            .absolute()
            .when(horizontal, |d| d.cursor(gpui::CursorStyle::ResizeLeftRight))
            .when(!horizontal, |d| d.cursor(gpui::CursorStyle::ResizeRow))
            .w(px(if horizontal { handle_px } else { 0.0 }))
            .h(px(if horizontal { 0.0 } else { handle_px }))
            .on_mouse_down(MouseButton::Left, {
                let drag = drag.clone();
                move |ev: &MouseDownEvent, window: &mut Window, _cx: &mut App| {
                    drag.dragging.set(true);
                    drag.start_mouse.set((ev.position.x.0, ev.position.y.0));
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
    use super::{clamp_primary, clamp_primary_vertical};

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
}
