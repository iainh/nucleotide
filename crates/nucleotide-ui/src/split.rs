//! Unified split and resize components for GPUI
//! Provides reusable sidebar, bottom panel, and two-pane split with consistent drag behavior

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext as _, Context, Div, DragMoveEvent, ElementId, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Render, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px, relative,
};

use crate::layout::PanelLayout;

pub const SPLITTER_HITBOX_PX: f32 = 10.0;
pub const SPLITTER_LINE_PX: f32 = 1.0;

const RESIZE_HANDLE_MIN_HITBOX_PX: f32 = 8.0;
const RESIZE_HANDLE_MAX_HITBOX_PX: f32 = 12.0;
const RESIZE_HANDLE_VISUAL_PX: f32 = SPLITTER_LINE_PX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeDragScope {
    Global,
    LeftSidebar,
    RightSidebar,
    BottomPanel,
}

#[derive(Clone)]
struct ResizeDragToken {
    scope: ResizeDragScope,
    click_offset: Rc<Cell<(f32, f32)>>,
    handle_hitbox_px: f32,
}

impl ResizeDragToken {
    fn new(scope: ResizeDragScope, handle_hitbox_px: f32) -> Self {
        Self {
            scope,
            click_offset: Rc::new(Cell::new((0.0, 0.0))),
            handle_hitbox_px,
        }
    }

    fn set_click_offset(&self, x: f32, y: f32) {
        self.click_offset.set((x, y));
    }

    fn click_offset(&self) -> (f32, f32) {
        self.click_offset.get()
    }

    fn handle_center_offset(&self) -> f32 {
        self.handle_hitbox_px * 0.5
    }
}

struct ResizeDragPreview;

impl Render for ResizeDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size(px(0.0))
    }
}

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

fn cursor_for_axis(axis: SplitterAxis) -> gpui::CursorStyle {
    match axis {
        SplitterAxis::Vertical => gpui::CursorStyle::ResizeLeftRight,
        SplitterAxis::Horizontal => gpui::CursorStyle::ResizeRow,
    }
}

fn resize_drag_source(handle: Stateful<Div>, token: ResizeDragToken) -> Stateful<Div> {
    handle.on_drag(token, |drag, offset, _window, cx| {
        drag.set_click_offset(f32::from(offset.x), f32::from(offset.y));
        cx.new(|_| ResizeDragPreview)
    })
}

fn resize_drag_surface(
    surface: Stateful<Div>,
    on_move: impl Fn(&MouseMoveEvent, &mut Window, &mut App) + 'static,
    on_finish: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    on_finish_out: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let on_move = Rc::new(on_move);

    surface
        .on_drag_move::<ResizeDragToken>({
            let on_move = on_move.clone();
            move |event, window, cx| {
                on_move(&event.event, window, cx);
            }
        })
        .on_mouse_move(move |event, window, cx| {
            on_move(event, window, cx);
        })
        .on_mouse_up(MouseButton::Left, on_finish)
        .on_mouse_up_out(MouseButton::Left, on_finish_out)
}

fn resize_scoped_drag_surface(
    surface: Stateful<Div>,
    scope: ResizeDragScope,
    on_move: impl Fn(&DragMoveEvent<ResizeDragToken>, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    surface.on_drag_move::<ResizeDragToken>(move |event, window, cx| {
        if event.drag(cx).scope == scope {
            on_move(event, window, cx);
        }
    })
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
    let theme = crate::providers::use_theme();
    let sep_color = crate::tokens::with_alpha(theme.tokens.chrome.separator_color, 0.7);
    let sep_hover_color = crate::tokens::with_alpha(theme.tokens.editor.focus_ring, 0.55);

    let base = div()
        .id(id)
        .relative()
        .occlude()
        .when(axis == SplitterAxis::Vertical, |d| {
            d.w(px(hitbox_px)).h_full().cursor(cursor_for_axis(axis))
        })
        .when(axis == SplitterAxis::Horizontal, |d| {
            d.w_full().h(px(hitbox_px)).cursor(cursor_for_axis(axis))
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

/// Transparent resize hitbox that owns the standard pointer lifecycle hooks.
pub fn resize_handle(
    id: impl Into<ElementId>,
    axis: SplitterAxis,
    handle_px: f32,
    on_start: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_finish: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    on_finish_out: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    resize_handle_with_scope(
        id,
        axis,
        handle_px,
        ResizeDragScope::Global,
        on_start,
        on_finish,
        on_finish_out,
    )
}

fn resize_handle_with_scope(
    id: impl Into<ElementId>,
    axis: SplitterAxis,
    handle_px: f32,
    scope: ResizeDragScope,
    on_start: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_finish: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    on_finish_out: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let hitbox_px = resize_handle_hitbox_px(handle_px).unwrap_or(RESIZE_HANDLE_MIN_HITBOX_PX);
    let token = ResizeDragToken::new(scope, hitbox_px);
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

    resize_drag_source(base, token)
        .on_mouse_down(MouseButton::Left, on_start)
        .on_mouse_up(MouseButton::Left, on_finish)
        .on_mouse_up_out(MouseButton::Left, on_finish_out)
}

/// Applies active resize capture behaviour to a surface while a drag is active.
pub fn resize_capture_area(
    surface: Stateful<Div>,
    axis: SplitterAxis,
    on_move: impl Fn(&MouseMoveEvent, &mut Window, &mut App) + 'static,
    on_finish: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    on_finish_out: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    resize_drag_surface(
        surface.cursor(cursor_for_axis(axis)),
        on_move,
        on_finish,
        on_finish_out,
    )
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
/// - `handle_px`: drag handle hit target
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
    let layout = PanelLayout::new(width_px, min_px, max_px, default_px);
    let width_px = layout.current_px();
    let on_change = Rc::new(on_change);

    // debug logging removed

    // The container captures move/up to provide robust dragging beyond the handle bounds
    let root = div()
        .id("sidebar-split")
        .flex()
        .relative()
        .w_full()
        .h_full()
        .flex_1()
        .min_h(px(0.0)); // allow vertical shrink inside column parents
    let mut root = resize_scoped_drag_surface(root, ResizeDragScope::LeftSidebar, {
        let on_change = on_change.clone();
        move |ev: &DragMoveEvent<ResizeDragToken>, window: &mut Window, cx: &mut App| {
            if ev.event.dragging() {
                let drag = ev.drag(cx);
                let (offset_x, _) = drag.click_offset();
                let root_left = f32::from(ev.bounds.left());
                let boundary_x = f32::from(ev.event.position.x) - root_left - offset_x
                    + drag.handle_center_offset();
                let viewport_w = f32::from(window.viewport_size().width);
                let constrained = layout.with_reserved_trailing_space(viewport_w, 200.0);
                on_change(
                    boundary_x.clamp(constrained.min_px(), constrained.max_px()),
                    cx,
                );
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

    // Handle: transparent hitbox centered over the pane boundary.
    let handle_hit_w = resize_handle_hitbox_px(handle_px).unwrap_or(RESIZE_HANDLE_MIN_HITBOX_PX);

    root = root.child({
        resize_handle_with_scope(
            "sidebar-resize-handle",
            SplitterAxis::Vertical,
            handle_px,
            ResizeDragScope::LeftSidebar,
            {
                let on_change = on_change.clone();
                move |ev: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                    if ev.click_count >= 2 {
                        let viewport_w = f32::from(window.viewport_size().width);
                        let constrained = layout.with_reserved_trailing_space(viewport_w, 200.0);
                        on_change(constrained.reset_px(), cx);
                        window.refresh();
                        cx.stop_propagation();
                        return;
                    }
                    cx.stop_propagation();
                }
            },
            {
                move |_ev: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                    if cx.stop_active_drag(window) {
                        window.refresh();
                    }
                }
            },
            {
                move |_ev: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                    if cx.stop_active_drag(window) {
                        window.refresh();
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(width_px - handle_hit_w * 0.5))
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
/// - `handle_px`: drag handle hit target
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
    let layout = PanelLayout::new(width_px, min_px, max_px, default_px);
    let width_px = layout.current_px();
    let on_change = Rc::new(on_change);

    let root = div()
        .id("right-sidebar-split")
        .flex()
        .relative()
        .size_full()
        .min_h(px(0.0));
    let mut root = resize_scoped_drag_surface(root, ResizeDragScope::RightSidebar, {
        let on_change = on_change.clone();
        move |ev: &DragMoveEvent<ResizeDragToken>, window: &mut Window, cx: &mut App| {
            if ev.event.dragging() {
                let drag = ev.drag(cx);
                let (offset_x, _) = drag.click_offset();
                let boundary_x =
                    f32::from(ev.event.position.x) - offset_x + drag.handle_center_offset();
                let new_w = f32::from(ev.bounds.right()) - boundary_x;
                on_change(new_w.clamp(layout.min_px(), layout.max_px()), cx);
                window.refresh();
            }
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
            resize_handle_with_scope(
                "right-sidebar-resize-handle",
                SplitterAxis::Vertical,
                handle_px,
                ResizeDragScope::RightSidebar,
                {
                    let on_change = on_change.clone();
                    move |ev: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                        if ev.click_count >= 2 {
                            on_change(layout.reset_px(), cx);
                            window.refresh();
                            cx.stop_propagation();
                            return;
                        }

                        cx.stop_propagation();
                    }
                },
                {
                    move |_ev: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                        if cx.stop_active_drag(window) {
                            window.refresh();
                        }
                    }
                },
                {
                    move |_ev: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                        if cx.stop_active_drag(window) {
                            window.refresh();
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(width_px - handle_hit_w * 0.5)),
        );
    }

    root
}

/// A bottom-docked panel with a draggable top edge.
/// - `height_px`: current panel height
/// - `min_px`/`max_px`: constraints for the panel height
/// - `handle_px`: drag handle hit target at the top.
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
    let layout = PanelLayout::new(height_px, min_px, max_px, default_px);
    let height_px = layout.current_px();
    let on_change = Rc::new(on_change);

    let root = div().id("bottom-panel-split").relative().size_full();
    let mut root = resize_scoped_drag_surface(root, ResizeDragScope::BottomPanel, {
        let on_change = on_change.clone();
        move |ev: &DragMoveEvent<ResizeDragToken>, window: &mut Window, cx: &mut App| {
            if ev.event.dragging() {
                let drag = ev.drag(cx);
                let (_, offset_y) = drag.click_offset();
                let boundary_y =
                    f32::from(ev.event.position.y) - offset_y + drag.handle_center_offset();
                let new_h = f32::from(ev.bounds.bottom()) - boundary_y;
                on_change(new_h.clamp(layout.min_px(), layout.max_px()), cx);
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
            resize_handle_with_scope(
                "bottom-panel-resize-handle",
                SplitterAxis::Horizontal,
                handle_px,
                ResizeDragScope::BottomPanel,
                {
                    let on_change = on_change.clone();
                    move |ev: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                        if ev.click_count >= 2 {
                            on_change(layout.reset_px(), cx);
                            window.refresh();
                            cx.stop_propagation();
                            return;
                        }
                        cx.stop_propagation();
                    }
                },
                {
                    move |_ev: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                        if cx.stop_active_drag(window) {
                            window.refresh();
                        }
                    }
                },
                {
                    move |_ev: &MouseUpEvent, window: &mut Window, cx: &mut App| {
                        if cx.stop_active_drag(window) {
                            window.refresh();
                        }
                    }
                },
            )
            .absolute()
            .left_0()
            .right_0()
            .top(px(-handle_hit_h * 0.5))
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
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton, ParentElement as _,
        Render, Styled as _, TestAppContext, Window, div, point, px,
    };

    use super::{
        RESIZE_HANDLE_MAX_HITBOX_PX, RESIZE_HANDLE_MIN_HITBOX_PX, ResizeDragController,
        SplitterAxis, bottom_panel_split, clamp_primary, clamp_primary_vertical,
        resize_capture_area, resize_handle, resize_handle_hitbox_px, resize_handle_visual_offset,
        right_sidebar_split, sidebar_split,
    };

    struct ResizeHandleHarness;

    impl Render for ResizeHandleHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(resize_handle(
                "test-resize-handle",
                SplitterAxis::Vertical,
                4.0,
                |_, _, _| {},
                |_, _, _| {},
                |_, _, _| {},
            ))
        }
    }

    #[gpui::test]
    fn resize_handle_renders_in_test_harness(cx: &mut TestAppContext) {
        let (_harness, _cx) = cx.add_window_view(|_, _| ResizeHandleHarness);
    }

    struct ResizeCaptureHarness;

    impl Render for ResizeCaptureHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            resize_capture_area(
                div().id("test-resize-capture").size_full(),
                SplitterAxis::Horizontal,
                |_, _, _| {},
                |_, _, _| {},
                |_, _, _| {},
            )
        }
    }

    #[gpui::test]
    fn resize_capture_area_renders_in_test_harness(cx: &mut TestAppContext) {
        let (_harness, _cx) = cx.add_window_view(|_, _| ResizeCaptureHarness);
    }

    struct PanelSplitHarness;

    impl Render for PanelSplitHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let editor = div().id("test-editor").size_full().child("Editor");
            let docs = div().id("test-docs").size_full().child("Docs");
            let file_tree = div().id("test-file-tree").size_full().child("Files");

            let editor_with_docs =
                right_sidebar_split(240.0, 120.0, 420.0, 10.0, 240.0, |_, _| {}, editor, docs);

            let editor_with_bottom =
                bottom_panel_split(160.0, 80.0, 360.0, 10.0, 160.0, |_, _| {}, editor_with_docs);

            div().size_full().child(sidebar_split(
                220.0,
                120.0,
                420.0,
                10.0,
                220.0,
                |_, _| {},
                file_tree,
                editor_with_bottom,
            ))
        }
    }

    #[gpui::test]
    fn panel_split_wrappers_render_in_test_harness(cx: &mut TestAppContext) {
        let (_harness, _cx) = cx.add_window_view(|_, _| PanelSplitHarness);
    }

    struct PanelSplitDragHarness {
        left_width: Rc<Cell<f32>>,
        right_width: Rc<Cell<f32>>,
        bottom_height: Rc<Cell<f32>>,
    }

    impl Render for PanelSplitDragHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(800.0)).h(px(300.0)).child(sidebar_split(
                200.0,
                100.0,
                500.0,
                10.0,
                200.0,
                {
                    let left_width = Rc::clone(&self.left_width);
                    move |width, _| left_width.set(width)
                },
                div().size_full(),
                right_sidebar_split(
                    200.0,
                    100.0,
                    500.0,
                    10.0,
                    200.0,
                    {
                        let right_width = Rc::clone(&self.right_width);
                        move |width, _| right_width.set(width)
                    },
                    div().size_full(),
                    bottom_panel_split(
                        120.0,
                        80.0,
                        240.0,
                        10.0,
                        120.0,
                        {
                            let bottom_height = Rc::clone(&self.bottom_height);
                            move |height, _| bottom_height.set(height)
                        },
                        div().size_full(),
                    ),
                ),
            ))
        }
    }

    #[gpui::test]
    fn panel_split_wrappers_emit_changes_during_drag(cx: &mut TestAppContext) {
        let left_width = Rc::new(Cell::new(200.0));
        let right_width = Rc::new(Cell::new(200.0));
        let bottom_height = Rc::new(Cell::new(120.0));

        let (_harness, window) = cx.add_window_view({
            let left_width = Rc::clone(&left_width);
            let right_width = Rc::clone(&right_width);
            let bottom_height = Rc::clone(&bottom_height);
            move |_, _| PanelSplitDragHarness {
                left_width,
                right_width,
                bottom_height,
            }
        });

        let modifiers = Modifiers::none();

        window.simulate_mouse_down(point(px(200.0), px(30.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_move(point(px(206.0), px(30.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_move(point(px(250.0), px(30.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_up(point(px(250.0), px(30.0)), MouseButton::Left, modifiers);
        assert!(
            left_width.get() > 200.0,
            "left sidebar did not resize: {}",
            left_width.get()
        );

        window.simulate_mouse_down(point(px(600.0), px(30.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_move(point(px(594.0), px(30.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_move(point(px(550.0), px(30.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_up(point(px(550.0), px(30.0)), MouseButton::Left, modifiers);
        assert!(
            right_width.get() > 200.0,
            "right sidebar did not resize: {}",
            right_width.get()
        );

        window.simulate_mouse_down(point(px(700.0), px(180.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_move(point(px(700.0), px(174.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_move(point(px(700.0), px(140.0)), MouseButton::Left, modifiers);
        window.simulate_mouse_up(point(px(700.0), px(140.0)), MouseButton::Left, modifiers);
        assert!(
            bottom_height.get() > 120.0,
            "bottom panel did not resize: {}",
            bottom_height.get()
        );
    }

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
