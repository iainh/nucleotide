//! Scrollbar component for GPUI based on Zed's implementation
//! Provides vertical and horizontal scrollbars for scrollable content

use std::{any::Any, cell::Cell, fmt::Debug, rc::Rc, sync::Arc, time::Duration};

use gpui::prelude::{
    FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
};
use gpui::{
    Along, App, Axis, Bounds, Div, IntoElement, IsZero, MouseButton, Pixels, Point, RenderOnce,
    ScrollHandle, Size, UniformListScrollHandle, Window, div, px,
};
use nucleotide_types::scrollbar::{
    SCROLLBAR_ALPHA_DRAGGING, SCROLLBAR_ALPHA_INACTIVE, SCROLLBAR_ALPHA_THUMB_HOVER,
    SCROLLBAR_ALPHA_TRACK_HOVER, SCROLLBAR_THICKNESS, ScrollbarPart, ScrollbarStyle,
    ScrollbarThumb, scrollbar_width_ratio,
};

/// A scrollbar component that can be attached to scrollable content
#[derive(IntoElement)]
pub struct Scrollbar {
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

    /// A small-change arrow advances one content line, not a fraction of a page.
    fn line_step(&self, _axis: Axis, window: &Window) -> Pixels {
        window.line_height()
    }
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

    fn line_step(&self, axis: Axis, window: &Window) -> Pixels {
        if axis == Axis::Vertical {
            self.0
                .borrow()
                .last_item_height
                .unwrap_or(window.line_height())
        } else {
            window.line_height()
        }
    }
}

/// Scrollbar state that should be persisted across frames
#[derive(Clone, Debug)]
pub struct ScrollbarState {
    thumb_state: Rc<Cell<ThumbState>>,
    track_hovered: Rc<Cell<bool>>,
    track_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    style: Rc<Cell<ScrollbarStyle>>,
    pressed: Rc<Cell<Option<(u64, ScrollbarPart)>>>,
    press_generation: Rc<Cell<u64>>,
    scroll_handle: Arc<dyn ScrollableHandle>,
}

impl ScrollbarState {
    pub fn new(scroll: impl ScrollableHandle) -> Self {
        Self {
            thumb_state: Rc::default(),
            track_hovered: Rc::default(),
            track_bounds: Rc::default(),
            style: Rc::default(),
            pressed: Rc::default(),
            press_generation: Rc::default(),
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

        let target_width = scrollbar_width_ratio(is_expanded);
        let target_alpha = match thumb_state {
            ThumbState::Dragging(_) => SCROLLBAR_ALPHA_DRAGGING,
            ThumbState::Hover => SCROLLBAR_ALPHA_THUMB_HOVER,
            ThumbState::Inactive if is_expanded => SCROLLBAR_ALPHA_TRACK_HOVER,
            ThumbState::Inactive => SCROLLBAR_ALPHA_INACTIVE,
        };

        (target_width, target_alpha)
    }

    fn thumb_for_track(&self, axis: Axis, track_length: Pixels) -> Option<ScrollbarThumb> {
        let max_offset = self.scroll_handle.max_offset().along(axis);
        let viewport_size = self.scroll_handle.viewport().size.along(axis);

        if max_offset.is_zero() {
            return None;
        }

        if viewport_size.is_zero() {
            return None;
        }

        let raw_offset = self.scroll_handle.offset();
        let offset_along_axis = raw_offset.along(axis);

        let current_offset = offset_along_axis.clamp(-max_offset, Pixels::ZERO).abs();

        self.style
            .get()
            .thumb(track_length, viewport_size, max_offset, current_offset)
    }

    fn apply_part(&self, part: ScrollbarPart, axis: Axis, window: &Window) {
        let scroll = self.scroll_handle();
        let position = part.scroll_position(
            -scroll.offset().along(axis),
            scroll.max_offset().along(axis),
            scroll.viewport().size.along(axis),
            scroll.line_step(axis, window),
        );
        scroll.set_offset(scroll.offset().apply_along(axis, |_| -position));
    }

    fn press_part(&self, part: ScrollbarPart, axis: Axis, window: &mut Window, cx: &mut App) {
        let generation = self.press_generation.get().wrapping_add(1);
        self.press_generation.set(generation);
        self.pressed.set(Some((generation, part)));
        self.apply_part(part, axis, window);
        let state = self.clone();
        window
            .spawn(cx, async move |cx| {
                let mut delay = Duration::from_millis(250);
                loop {
                    cx.background_executor().timer(delay).await;
                    let keep_repeating = cx
                        .update(|window, _cx| {
                            if !window.is_window_active()
                                || state.pressed.get() != Some((generation, part))
                            {
                                return false;
                            }
                            let Some(bounds) = state.track_bounds() else {
                                return false;
                            };
                            let Some(thumb) = state.thumb_for_track(axis, bounds.size.along(axis))
                            else {
                                return false;
                            };
                            let pointer = window.mouse_position();
                            // Page-repeat stops when the thumb reaches the pointer. Moving
                            // outside an arrow suspends its repeat until the pointer returns.
                            if bounds.contains(&pointer)
                                && state.style.get().part(
                                    thumb,
                                    bounds.size.along(axis),
                                    pointer.along(axis) - bounds.origin.along(axis),
                                ) == part
                            {
                                state.apply_part(part, axis, window);
                                window.refresh();
                            }
                            true
                        })
                        .unwrap_or(false);
                    if !keep_repeating {
                        break;
                    }
                    delay = Duration::from_millis(50);
                }
            })
            .detach();
    }
}

/// Resolve the system look at the UI boundary; editor callers pass it down
/// explicitly to keep the editor crate independent of UI theme globals.
pub fn scrollbar_style(cx: &App) -> ScrollbarStyle {
    if cfg!(target_os = "windows")
        && cx.try_global::<crate::Theme>().is_some_and(|theme| {
            theme.tokens.chrome.style == nucleotide_appearance::UiChromeStyle::System
        })
    {
        ScrollbarStyle::Windows
    } else {
        ScrollbarStyle::Themed
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
        Some(Self { state, axis })
    }
}

impl RenderOnce for Scrollbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let style = scrollbar_style(cx);
        self.state.style.set(style);
        let (_, alpha) = self.state.target_values();
        let alpha = if style == ScrollbarStyle::Windows {
            0.45
        } else {
            alpha
        };
        let thumb_bg = if let Some(theme) = cx.try_global::<crate::Theme>() {
            let chrome = &theme.tokens.chrome;
            chrome.contrast.map_or_else(
                || crate::styling::ColorTheory::with_alpha(chrome.text_on_chrome, alpha),
                |c| c.window_text,
            )
        } else {
            let tokens = crate::DesignTokens::dark();
            let chrome = &tokens.chrome;
            crate::styling::ColorTheory::with_alpha(chrome.text_on_chrome, alpha)
        };

        let track_len = self
            .state
            .track_bounds()
            .map(|bounds| bounds.size.along(self.axis))
            .unwrap_or_else(|| self.state.scroll_handle().viewport().size.along(self.axis))
            .max(px(0.0));
        let Some(thumb) = self.state.thumb_for_track(self.axis, track_len) else {
            self.state.set_track_bounds(None);
            return empty_scrollbar_track(self.axis);
        };
        let visual = style.visual(thumb, track_len, self.state.is_expanded());
        let state = self.state.clone();
        let axis = self.axis;
        let state_id = Rc::as_ptr(&self.state.track_hovered) as usize;

        let mut track = scrollbar_track(self.axis)
            .id(("scrollbar-track", state_id))
            .cursor_pointer()
            .when(style == ScrollbarStyle::Windows, |track| {
                track.cursor_default()
            })
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
                    let Some((thumb, thumb_bounds)) =
                        thumb_geometry_for_track(&state, axis, bounds)
                    else {
                        return;
                    };

                    if thumb_bounds.contains(&event.position) {
                        let offset = event.position.along(axis) - thumb_bounds.origin.along(axis);
                        state.set_dragging(offset);
                    } else if style == ScrollbarStyle::Windows {
                        let part = style.part(
                            thumb,
                            bounds.size.along(axis),
                            event.position.along(axis) - bounds.origin.along(axis),
                        );
                        state.press_part(part, axis, window, cx);
                    } else {
                        let scroll_handle = state.scroll_handle();
                        let click_offset = scroll_offset_for_pointer(
                            style,
                            axis,
                            bounds,
                            thumb,
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
                    let Some((thumb, thumb_bounds)) =
                        thumb_geometry_for_track(&state, axis, bounds)
                    else {
                        return;
                    };

                    if state.thumb_state.get().is_dragging()
                        && event.dragging()
                        && let ThumbState::Dragging(drag_offset) = state.thumb_state.get()
                    {
                        let scroll_handle = state.scroll_handle();
                        let pointer_offset = scroll_offset_for_pointer(
                            style,
                            axis,
                            bounds,
                            thumb,
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
                        state.pressed.set(None);
                        let over_thumb = thumb_bounds.contains(&event.position);
                        let was_thumb_hover = matches!(state.thumb_state.get(), ThumbState::Hover);
                        state.set_thumb_hovered(over_thumb);
                        if over_thumb != was_thumb_hover || style == ScrollbarStyle::Windows {
                            window.refresh();
                        }
                    }
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let state = state.clone();
                move |event, window, cx| {
                    state.pressed.set(None);
                    if state.is_dragging() {
                        state.scroll_handle().drag_ended();
                    }

                    if let Some(bounds) = state.track_bounds() {
                        state.set_track_hovered(bounds.contains(&event.position));
                        let over_thumb = thumb_geometry_for_track(&state, axis, bounds)
                            .is_some_and(|(_, thumb_bounds)| {
                                thumb_bounds.contains(&event.position)
                            });
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
                    let was_pressed = state.pressed.replace(None).is_some();
                    if !state.is_dragging() {
                        if was_pressed {
                            window.refresh();
                            cx.stop_propagation();
                        }
                        return;
                    }
                    state.scroll_handle().drag_ended();
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

        if style == ScrollbarStyle::Windows && self.state.is_expanded() {
            if let Some(theme) = cx.try_global::<crate::Theme>() {
                track = track
                    .bg(theme.tokens.chrome.surface_elevated)
                    .rounded(px(3.0));
            }
            for part in [ScrollbarPart::Decrease, ScrollbarPart::Increase] {
                let hovered = self.state.track_bounds().is_some_and(|bounds| {
                    bounds.contains(&window.mouse_position())
                        && style.part(
                            thumb,
                            track_len,
                            window.mouse_position().along(self.axis)
                                - bounds.origin.along(self.axis),
                        ) == part
                });
                let position = -self.state.scroll_handle.offset().along(self.axis);
                let disabled = if part == ScrollbarPart::Decrease {
                    position <= px(0.0)
                } else {
                    position >= self.state.scroll_handle.max_offset().along(self.axis)
                };
                let contrast = cx
                    .try_global::<crate::Theme>()
                    .and_then(|t| t.tokens.chrome.contrast);
                let arrow_bg = if let Some(c) = contrast {
                    if disabled { c.gray_text } else { c.window_text }
                } else {
                    crate::styling::ColorTheory::with_alpha(
                        thumb_bg,
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
                    .text_color(arrow_bg)
                    .child(part.arrow_glyph(self.axis));
                arrow = match (self.axis, part) {
                    (Axis::Vertical, ScrollbarPart::Decrease) => arrow.top_0().pt(px(4.0)),
                    (Axis::Vertical, _) => arrow.bottom_0().pb(px(4.0)),
                    (Axis::Horizontal, ScrollbarPart::Decrease) => arrow.left_0().pl(px(4.0)),
                    (Axis::Horizontal, _) => arrow.right_0().pr(px(4.0)),
                };
                track = track.child(arrow);
            }
        }

        if style == ScrollbarStyle::Windows {
            let state = self.state.clone();
            let axis = self.axis;
            track = track.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        window.on_mouse_event(
                            move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                                if phase != gpui::DispatchPhase::Capture || !event.dragging() {
                                    return;
                                }
                                let ThumbState::Dragging(drag_offset) = state.thumb_state.get()
                                else {
                                    return;
                                };
                                let Some(bounds) = state.track_bounds() else {
                                    return;
                                };
                                let Some(thumb) =
                                    state.thumb_for_track(axis, bounds.size.along(axis))
                                else {
                                    return;
                                };
                                let scroll = state.scroll_handle();
                                let offset = scroll_offset_for_pointer(
                                    style,
                                    axis,
                                    bounds,
                                    thumb,
                                    scroll.max_offset(),
                                    event.position,
                                    drag_offset,
                                );
                                scroll.set_offset(scroll.offset().apply_along(axis, |_| offset));
                                window.refresh();
                                cx.stop_propagation();
                            },
                        );
                    },
                )
                .absolute()
                .size_full(),
            );
        }

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

fn thumb_geometry_for_track(
    state: &ScrollbarState,
    axis: Axis,
    bounds: Bounds<Pixels>,
) -> Option<(ScrollbarThumb, Bounds<Pixels>)> {
    let thumb = state.thumb_for_track(axis, bounds.size.along(axis))?;
    let thumb_bounds = state
        .style
        .get()
        .thumb_bounds(thumb, axis, bounds, state.is_expanded());

    Some((thumb, thumb_bounds))
}

fn scroll_offset_for_pointer(
    style: ScrollbarStyle,
    axis: Axis,
    track_bounds: Bounds<Pixels>,
    thumb: ScrollbarThumb,
    max_offset: Size<Pixels>,
    event_position: Point<Pixels>,
    thumb_offset: Pixels,
) -> Pixels {
    -style.scroll_position(
        track_bounds.size.along(axis),
        max_offset.along(axis),
        thumb,
        event_position.along(axis) - track_bounds.origin.along(axis),
        thumb_offset,
    )
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Modifiers, Render, TestAppContext, point, size};

    use super::*;

    struct ScrollbarReleaseHarness {
        scroll: ScrollHandle,
        state: ScrollbarState,
        releases: Rc<Cell<usize>>,
    }

    impl Render for ScrollbarReleaseHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let releases = self.releases.clone();
            div()
                .size_full()
                .on_mouse_up(MouseButton::Left, move |_, _, _| {
                    releases.set(releases.get() + 1);
                })
                .child(
                    div()
                        .relative()
                        .flex()
                        .w(px(112.0))
                        .h(px(100.0))
                        .child(
                            div()
                                .id("overflowing-content")
                                .w(px(100.0))
                                .h_full()
                                .flex_none()
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll)
                                .child(div().h(px(400.0))),
                        )
                        .children(Scrollbar::vertical(self.state.clone())),
                )
        }
    }

    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn system_list_arrow_uses_measured_row_not_viewport(cx: &mut TestAppContext) {
        struct Harness {
            scroll: UniformListScrollHandle,
            state: ScrollbarState,
        }
        impl Render for Harness {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .flex()
                    .w(px(112.0))
                    .h(px(100.0))
                    .child(
                        gpui::uniform_list("rows", 30, |range, _, _| {
                            range.map(|_| div().h(px(23.0))).collect::<Vec<_>>()
                        })
                        .w(px(100.0))
                        .h_full()
                        .track_scroll(&self.scroll),
                    )
                    .children(Scrollbar::vertical(self.state.clone()))
            }
        }
        cx.update(|cx| {
            let mut tokens = crate::DesignTokens::dark();
            tokens.chrome.style = nucleotide_appearance::UiChromeStyle::System;
            cx.set_global(crate::Theme::from_tokens(tokens));
        });
        let (harness, cx) = cx.add_window_view(|_, _| {
            let scroll = UniformListScrollHandle::new();
            Harness {
                state: ScrollbarState::new(scroll.clone()),
                scroll,
            }
        });
        for _ in 0..2 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear();
            });
        }
        let arrow = point(px(106.0), px(96.0));
        cx.simulate_mouse_down(arrow, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(arrow, MouseButton::Left, Modifiers::none());
        assert_eq!(harness.read_with(cx, |h, _| h.scroll.offset().y), px(-23.0));
    }

    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn system_scrollbar_pages_and_repeats_arrows_without_track_drag(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut tokens = crate::DesignTokens::dark();
            tokens.chrome.style = nucleotide_appearance::UiChromeStyle::System;
            cx.set_global(crate::Theme::from_tokens(tokens));
        });
        let (harness, cx) = cx.add_window_view(|_, _| {
            let scroll = ScrollHandle::new();
            ScrollbarReleaseHarness {
                state: ScrollbarState::new(scroll.clone()),
                scroll,
                releases: Rc::default(),
            }
        });
        for _ in 0..2 {
            cx.update(|window, cx| {
                window.activate_window();
                window.refresh();
                window.draw(cx).clear();
            });
        }
        let offset =
            |cx: &gpui::VisualTestContext| harness.read_with(cx, |h, _| h.scroll.offset().y);
        let page = point(px(106.0), px(65.0));
        cx.simulate_mouse_move(page, None, Modifiers::none());
        cx.simulate_mouse_down(page, MouseButton::Left, Modifiers::none());
        assert_eq!(offset(cx), px(-100.0));
        cx.simulate_mouse_move(
            point(px(106.0), px(85.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(offset(cx), px(-100.0), "page click must not begin a drag");
        cx.simulate_mouse_up(page, MouseButton::Left, Modifiers::none());

        let arrow = point(px(106.0), px(96.0));
        let line = cx.update(|window, _| window.line_height());
        cx.simulate_mouse_move(arrow, None, Modifiers::none());
        cx.simulate_mouse_down(arrow, MouseButton::Left, Modifiers::none());
        assert_eq!(offset(cx), px(-100.0) - line);
        cx.run_until_parked();
        cx.background_executor
            .advance_clock(Duration::from_millis(250));
        cx.run_until_parked();
        assert_eq!(offset(cx), px(-100.0) - line * 2.0);
        let outside = point(px(150.0), px(50.0));
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
        cx.background_executor
            .advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        assert_eq!(
            offset(cx),
            px(-100.0) - line * 2.0,
            "outside release must cancel repeat"
        );

        let thumb = point(px(106.0), px(45.0));
        let beyond_bottom = point(px(150.0), px(150.0));
        cx.simulate_mouse_down(thumb, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(beyond_bottom, MouseButton::Left, Modifiers::none());
        assert_eq!(offset(cx), px(-300.0), "drag continues outside the track");
        cx.simulate_mouse_up(beyond_bottom, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_down(
            point(px(106.0), px(80.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        let beyond_top = point(px(150.0), px(-25.0));
        cx.simulate_mouse_move(beyond_top, MouseButton::Left, Modifiers::none());
        assert_eq!(offset(cx), px(0.0));
        cx.simulate_mouse_up(beyond_top, MouseButton::Left, Modifiers::none());
    }

    #[gpui::test]
    fn idle_scrollbar_does_not_consume_outside_releases(cx: &mut TestAppContext) {
        let releases = Rc::new(Cell::new(0));
        let (harness, cx) = cx.add_window_view(|_, _| {
            let scroll = ScrollHandle::new();
            ScrollbarReleaseHarness {
                state: ScrollbarState::new(scroll.clone()),
                scroll,
                releases: releases.clone(),
            }
        });
        // The first layout populates the scroll handle; the second renders
        // the thumb and its mouse listeners.
        for _ in 0..2 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear();
            });
        }
        harness.read_with(cx, |harness, _| {
            assert!(harness.scroll.max_offset().y > px(0.0));
        });
        let outside = point(px(150.0), px(50.0));
        cx.simulate_mouse_move(outside, None, Modifiers::none());
        cx.simulate_mouse_down(outside, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
        assert_eq!(releases.get(), 1);

        // A scrollbar-owned drag still finishes outside the track, then stops
        // consuming subsequent unrelated releases.
        let thumb = point(px(106.0), px(10.0));
        cx.simulate_mouse_move(thumb, None, Modifiers::none());
        cx.simulate_mouse_down(thumb, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            point(px(106.0), px(50.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.simulate_mouse_move(outside, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
        harness.read_with(cx, |harness, _| {
            assert!(harness.scroll.offset().y < px(0.0));
        });
        assert_eq!(releases.get(), 1);
        cx.simulate_mouse_down(outside, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());
        assert_eq!(releases.get(), 2);
    }

    #[test]
    fn scroll_offset_for_pointer_uses_negative_gpui_offsets() {
        let track_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(12.0), px(104.0)));

        let offset = scroll_offset_for_pointer(
            ScrollbarStyle::Themed,
            Axis::Vertical,
            track_bounds,
            ScrollbarThumb {
                start: px(0.0),
                length: px(20.0),
            },
            size(px(0.0), px(200.0)),
            point(px(6.0), px(52.0)),
            px(10.0),
        );

        assert_eq!(offset, px(-100.0));
    }
}
