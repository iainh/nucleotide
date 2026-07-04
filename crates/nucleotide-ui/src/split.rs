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
pub struct ResizeDragController {
    dragging: Rc<Cell<bool>>,
    start_mouse: Rc<Cell<(f32, f32)>>,
    start_primary: Rc<Cell<f32>>,
}

impl ResizeDragController {
    pub fn new() -> Self {
        Self {
            dragging: Rc::new(Cell::new(false)),
            start_mouse: Rc::new(Cell::new((0.0, 0.0))),
            start_primary: Rc::new(Cell::new(0.0)),
        }
    }

    pub fn begin(&self, mouse_x: f32, mouse_y: f32, start_primary: f32) {
        self.dragging.set(true);
        self.start_mouse.set((mouse_x, mouse_y));
        self.start_primary.set(start_primary);
    }

    pub fn begin_from_mouse_down(&self, event: &MouseDownEvent, start_primary: f32) {
        self.begin(
            f32::from(event.position.x),
            f32::from(event.position.y),
            start_primary,
        );
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.get()
    }

    pub fn any_dragging<'a>(controllers: impl IntoIterator<Item = &'a Self>) -> bool {
        controllers
            .into_iter()
            .any(ResizeDragController::is_dragging)
    }

    pub fn finish(&self) -> bool {
        let was_dragging = self.dragging.get();
        self.dragging.set(false);
        was_dragging
    }

    pub fn finish_all<'a>(controllers: impl IntoIterator<Item = &'a Self>) -> bool {
        let mut finished_any = false;
        for controller in controllers {
            finished_any = controller.finish() || finished_any;
        }
        finished_any
    }

    pub fn finish_with_refresh(&self, window: &mut Window) -> bool {
        let finished = self.finish();
        if finished {
            window.refresh();
        }
        finished
    }

    pub fn horizontal_value(&self, mouse_x: f32, min_px: f32, max_px: f32) -> Option<f32> {
        if !self.dragging.get() {
            return None;
        }

        let start_x = self.start_mouse.get().0;
        let dx = mouse_x - start_x;
        Some(clamp_primary(self.start_primary.get(), dx, min_px, max_px))
    }

    pub fn horizontal_value_from_mouse_move(
        &self,
        event: &MouseMoveEvent,
        min_px: f32,
        max_px: f32,
    ) -> Option<f32> {
        self.horizontal_value(f32::from(event.position.x), min_px, max_px)
    }

    pub fn left_edge_value(&self, mouse_x: f32, min_px: f32, max_px: f32) -> Option<f32> {
        if !self.dragging.get() {
            return None;
        }

        let start_x = self.start_mouse.get().0;
        let dx = mouse_x - start_x;
        Some(clamp_primary(self.start_primary.get(), -dx, min_px, max_px))
    }

    pub fn left_edge_value_from_mouse_move(
        &self,
        event: &MouseMoveEvent,
        min_px: f32,
        max_px: f32,
    ) -> Option<f32> {
        self.left_edge_value(f32::from(event.position.x), min_px, max_px)
    }

    pub fn top_edge_value(&self, mouse_y: f32, min_px: f32, max_px: f32) -> Option<f32> {
        if !self.dragging.get() {
            return None;
        }

        let start_y = self.start_mouse.get().1;
        let dy = mouse_y - start_y;
        Some(clamp_primary_vertical(
            self.start_primary.get(),
            dy,
            min_px,
            max_px,
        ))
    }

    pub fn top_edge_value_from_mouse_move(
        &self,
        event: &MouseMoveEvent,
        min_px: f32,
        max_px: f32,
    ) -> Option<f32> {
        self.top_edge_value(f32::from(event.position.y), min_px, max_px)
    }
}

impl Default for ResizeDragController {
    fn default() -> Self {
        Self::new()
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
    let drag = ResizeDragController::new();
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
                if ev.dragging() {
                    // Ensure a minimum width for right pane (200px) so editor never collapses
                    let viewport_w = f32::from(window.viewport_size().width);
                    let max_allowed = (viewport_w - 200.0).max(min_px);
                    if let Some(new_w) =
                        drag.horizontal_value_from_mouse_move(ev, min_px, max_allowed.min(max_px))
                    {
                        on_change(new_w, cx);
                        window.refresh();
                    }
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                drag.finish_with_refresh(window);
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                drag.finish_with_refresh(window);
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
                drag.begin_from_mouse_down(ev, width_px);
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

/// A right sidebar split with flexible main content and a fixed-width right pane.
/// - `width_px`: current right pane width in pixels
/// - `min_px`/`max_px`: constraints for the right pane
/// - `handle_px`: drag handle hit target; the visible separator remains 1px
/// - `on_change`: callback invoked with the new width during drag
/// - `default_px`: width to snap to on double-click
#[allow(clippy::too_many_arguments)]
pub fn right_sidebar_split<L: IntoElement, R: IntoElement>(
    width_px: f32,
    min_px: f32,
    max_px: f32,
    handle_px: f32,
    default_px: f32,
    on_change: impl Fn(f32, &mut App) + 'static,
    left: L,
    right: R,
) -> impl IntoElement {
    let drag = ResizeDragController::new();
    let min_px = min_px.max(0.0);
    let max_px = if max_px < min_px { min_px } else { max_px };
    let width_px = width_px.clamp(min_px, max_px);
    let on_change = Rc::new(on_change);

    let mut root = div()
        .flex()
        .relative()
        .size_full()
        .min_h(px(0.0))
        .on_mouse_move({
            let drag = drag.clone();
            let on_change = on_change.clone();
            move |ev: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
                if ev.dragging()
                    && let Some(new_w) = drag.left_edge_value_from_mouse_move(ev, min_px, max_px)
                {
                    on_change(new_w, cx);
                    window.refresh();
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                drag.finish_with_refresh(window);
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                drag.finish_with_refresh(window);
            }
        });

    root = root.child(
        div()
            .flex_1()
            .h_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(left),
    );

    root = root.child(
        div()
            .w(px(width_px))
            .h_full()
            .flex_shrink_0()
            .min_h(px(0.0))
            .child(right),
    );

    if let Some(handle_hit_w) = resize_handle_hitbox_px(handle_px) {
        root = root.child(
            splitter(
                "right-sidebar-resize-handle",
                SplitterAxis::Vertical,
                handle_px,
            )
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(width_px - handle_hit_w * 0.5))
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

                    drag.begin_from_mouse_down(ev, width_px);
                    window.refresh();
                    cx.stop_propagation();
                }
            }),
        );
    }

    root
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
    let drag = ResizeDragController::new();
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
                if ev.dragging()
                    && let Some(new_h) = drag.top_edge_value_from_mouse_move(ev, min_px, max_px)
                {
                    on_change(new_h, cx);
                    window.refresh();
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                drag.finish_with_refresh(window);
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let drag = drag.clone();
            move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
                drag.finish_with_refresh(window);
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
                    drag.begin_from_mouse_down(ev, height_px);
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
    let drag = ResizeDragController::new();
    let handle_px = handle_px.max(2.0);
    let fraction = fraction.clamp(0.0, 1.0);

    let mut root = div().relative().size_full();

    root = root.on_mouse_up(MouseButton::Left, {
        let drag = drag.clone();
        move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
            drag.finish_with_refresh(window);
        }
    });

    root = root.on_mouse_up_out(MouseButton::Left, {
        let drag = drag.clone();
        move |_ev: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
            drag.finish_with_refresh(window);
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
                drag.begin_from_mouse_down(ev, fraction);
                window.refresh();
            }
        })
        .on_mouse_move({
            let drag = drag.clone();
            let on_change_fraction = Rc::new(on_change_fraction);
            move |_ev: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
                if drag.is_dragging() {
                    on_change_fraction(fraction, cx);
                    window.refresh();
                }
            }
        }),
    );

    root.child(a).child(b)
}

#[cfg(test)]
mod tests {
    use super::{
        RESIZE_HANDLE_MAX_HITBOX_PX, RESIZE_HANDLE_MIN_HITBOX_PX, ResizeDragController,
        clamp_primary, clamp_primary_vertical, resize_handle_hitbox_px,
        resize_handle_visual_offset,
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

    #[test]
    fn resize_drag_controller_tracks_horizontal_drag() {
        let drag = ResizeDragController::new();
        assert_eq!(drag.horizontal_value(120.0, 100.0, 400.0), None);

        drag.begin(100.0, 25.0, 200.0);

        assert!(drag.is_dragging());
        assert_eq!(drag.horizontal_value(150.0, 100.0, 400.0), Some(250.0));
        assert_eq!(drag.horizontal_value(0.0, 150.0, 400.0), Some(150.0));
        assert_eq!(drag.horizontal_value(500.0, 100.0, 300.0), Some(300.0));
    }

    #[test]
    fn resize_drag_controller_tracks_left_edge_drag() {
        let drag = ResizeDragController::new();
        assert_eq!(drag.left_edge_value(120.0, 100.0, 400.0), None);

        drag.begin(300.0, 25.0, 200.0);

        assert_eq!(drag.left_edge_value(250.0, 100.0, 400.0), Some(250.0));
        assert_eq!(drag.left_edge_value(500.0, 150.0, 400.0), Some(150.0));
        assert_eq!(drag.left_edge_value(-100.0, 100.0, 300.0), Some(300.0));
    }

    #[test]
    fn resize_drag_controller_tracks_top_edge_drag() {
        let drag = ResizeDragController::new();
        drag.begin(0.0, 100.0, 240.0);

        assert_eq!(drag.top_edge_value(70.0, 120.0, 500.0), Some(270.0));
        assert_eq!(drag.top_edge_value(400.0, 120.0, 500.0), Some(120.0));
        assert_eq!(drag.top_edge_value(-300.0, 120.0, 500.0), Some(500.0));
    }

    #[test]
    fn resize_drag_controller_finish_reports_active_state() {
        let drag = ResizeDragController::new();

        assert!(!drag.finish());
        drag.begin(0.0, 0.0, 100.0);
        assert!(drag.finish());
        assert!(!drag.is_dragging());
        assert!(!drag.finish());
    }

    #[test]
    fn resize_drag_controller_finish_all_clears_every_active_drag() {
        let first = ResizeDragController::new();
        let second = ResizeDragController::new();
        let third = ResizeDragController::new();

        first.begin(10.0, 0.0, 100.0);
        third.begin(40.0, 0.0, 300.0);

        assert!(ResizeDragController::any_dragging([
            &first, &second, &third,
        ]));
        assert!(ResizeDragController::finish_all([&first, &second, &third]));
        assert!(!first.is_dragging());
        assert!(!second.is_dragging());
        assert!(!third.is_dragging());
        assert!(!ResizeDragController::finish_all(
            [&first, &second, &third,]
        ));
    }
}
