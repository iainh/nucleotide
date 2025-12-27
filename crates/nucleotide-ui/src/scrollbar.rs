//! Scrollbar component for GPUI based on Zed's implementation
//! Provides vertical and horizontal scrollbars for scrollable content

use std::{any::Any, cell::Cell, cell::RefCell, fmt::Debug, ops::Range, rc::Rc, sync::Arc};

use gpui::{
    Along, App, Axis, BorderStyle, Bounds, ContentMask, Corners, CursorStyle, Edges, Element,
    ElementId, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, IsZero,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollHandle, ScrollWheelEvent, Size, Style, UniformListScrollHandle, Window, hsla, px, quad,
    relative,
};

/// A scrollbar component that can be attached to scrollable content
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
        self.max_offset()
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
        self.0.borrow().base_handle.max_offset()
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

/// Animated values for smooth scrollbar transitions
#[derive(Debug, Clone)]
struct AnimatedValues {
    /// Current interpolated thumb width ratio (0.0 to 1.0)
    width_ratio: f32,
    /// Current interpolated alpha (0.0 to 1.0)
    alpha: f32,
}

impl Default for AnimatedValues {
    fn default() -> Self {
        Self {
            width_ratio: 0.35, // Start at inactive width
            alpha: 0.25,       // Start at inactive alpha
        }
    }
}

/// Scrollbar state that should be persisted across frames
#[derive(Clone, Debug)]
pub struct ScrollbarState {
    thumb_state: Rc<Cell<ThumbState>>,
    track_hovered: Rc<Cell<bool>>,
    animated: Rc<RefCell<AnimatedValues>>,
    scroll_handle: Arc<dyn ScrollableHandle>,
}

/// Animation speed factor (higher = faster animation)
const ANIMATION_SPEED: f32 = 0.25;
/// Threshold for considering animation complete
const ANIMATION_THRESHOLD: f32 = 0.01;

impl ScrollbarState {
    pub fn new(scroll: impl ScrollableHandle) -> Self {
        Self {
            thumb_state: Rc::default(),
            track_hovered: Rc::default(),
            animated: Rc::default(),
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

    /// Animate values toward targets, returns true if animation is still in progress
    fn animate(&self) -> bool {
        let (target_width, target_alpha) = self.target_values();
        let mut animated = self.animated.borrow_mut();

        // Lerp toward targets
        let width_diff = target_width - animated.width_ratio;
        let alpha_diff = target_alpha - animated.alpha;

        animated.width_ratio += width_diff * ANIMATION_SPEED;
        animated.alpha += alpha_diff * ANIMATION_SPEED;

        // Check if we're close enough to snap to final values
        let width_animating = width_diff.abs() > ANIMATION_THRESHOLD;
        let alpha_animating = alpha_diff.abs() > ANIMATION_THRESHOLD;

        if !width_animating {
            animated.width_ratio = target_width;
        }
        if !alpha_animating {
            animated.alpha = target_alpha;
        }

        width_animating || alpha_animating
    }

    /// Get current animated values
    fn current_values(&self) -> (f32, f32) {
        let animated = self.animated.borrow();
        (animated.width_ratio, animated.alpha)
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

impl Element for Scrollbar {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = if self.axis == Axis::Vertical {
            Style {
                flex_grow: 0.,
                flex_shrink: 0.,
                size: Size {
                    width: px(12.).into(), // Scrollbar width
                    height: relative(1.).into(),
                },
                ..Default::default()
            }
        } else {
            Style {
                flex_grow: 0.,
                flex_shrink: 0.,
                size: Size {
                    width: relative(1.).into(),
                    height: px(12.).into(), // Scrollbar height
                },
                ..Default::default()
            }
        };

        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.insert_hitbox(bounds, HitboxBehavior::Normal)
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        const EXTRA_PADDING: Pixels = px(2.0); // Padding for scrollbar track

        // If content fits entirely in the viewport, don't paint the scrollbar.
        let maybe_thumb = self.state.thumb_range(self.axis);
        if maybe_thumb.is_none() {
            // No scrolling required; skip painting and event wiring.
            return;
        }
        // Recalculate thumb position every paint to reflect current scroll state
        self.thumb = maybe_thumb.unwrap();

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let axis = self.axis;

            // Animate toward target values and request refresh if still animating
            let is_animating = self.state.animate();
            if is_animating {
                window.refresh();
            }

            // Get current animated values
            let (current_width_ratio, current_alpha) = self.state.current_values();

            // Use chrome tokens to ensure visibility in file tree/editor contexts
            let (thumb_bg, gutter_bg) = if let Some(theme) = cx.try_global::<crate::Theme>() {
                let chrome = &theme.tokens.chrome;
                // Base the thumb on readable text-on-chrome color for contrast
                let base_thumb = chrome.text_on_chrome;
                // Use animated alpha value
                let thumb = crate::styling::ColorTheory::with_alpha(base_thumb, current_alpha);
                // Make the scrollbar track (gutter) transparent
                let track = crate::styling::ColorTheory::with_alpha(chrome.separator_color, 0.0);
                (thumb, track)
            } else {
                (
                    hsla(0.0, 0.0, 0.8, current_alpha), // thumb with animated alpha
                    hsla(0.0, 0.0, 0.0, 0.0),           // gutter transparent
                )
            };

            let padded_bounds = Bounds::from_corners(
                bounds
                    .origin
                    .apply_along(axis, |origin| origin + EXTRA_PADDING),
                bounds
                    .bottom_right()
                    .apply_along(axis, |track_end| track_end - EXTRA_PADDING),
            );

            // Paint gutter behind the thumb
            window.paint_quad(quad(
                padded_bounds,
                Corners::all(px(6.0)),
                gutter_bg,
                Edges::default(),
                hsla(0.0, 0.0, 0.0, 0.0),
                BorderStyle::default(),
            ));

            let thumb_offset = self.thumb.start * padded_bounds.size.along(axis);
            let thumb_end = self.thumb.end * padded_bounds.size.along(axis);

            // Use animated width ratio for smooth expansion
            let thumb_width = padded_bounds.size.along(axis.invert()) * current_width_ratio;
            let thumb_center_offset = (padded_bounds.size.along(axis.invert()) - thumb_width) / 2.0;

            let thumb_bounds = Bounds::new(
                padded_bounds
                    .origin
                    .apply_along(axis, |origin| origin + thumb_offset)
                    .apply_along(axis.invert(), |origin| origin + thumb_center_offset),
                padded_bounds
                    .size
                    .apply_along(axis, |_| thumb_end - thumb_offset)
                    .apply_along(axis.invert(), |_| thumb_width),
            );

            let corners = Corners::all(thumb_bounds.size.along(axis.invert()) / 2.0);

            // Paint the thumb
            window.paint_quad(quad(
                thumb_bounds,
                corners,
                thumb_bg,
                Edges::default(),
                hsla(0.0, 0.0, 0.0, 0.0),
                BorderStyle::default(),
            ));

            // Always use arrow cursor for scrollbar
            window.set_cursor_style(CursorStyle::Arrow, hitbox);

            enum ScrollbarMouseEvent {
                GutterClick,
                ThumbDrag(Pixels),
            }

            // Store the actual thumb dimensions for use in event handlers
            let actual_thumb_bounds = thumb_bounds;

            let compute_click_offset =
                move |event_position: Point<Pixels>,
                      max_offset: Size<Pixels>,
                      event_type: ScrollbarMouseEvent| {
                    let viewport_size = padded_bounds.size.along(axis);
                    let thumb_size = actual_thumb_bounds.size.along(axis);

                    let thumb_offset = match event_type {
                        ScrollbarMouseEvent::GutterClick => thumb_size / 2.,
                        ScrollbarMouseEvent::ThumbDrag(thumb_offset) => thumb_offset,
                    };

                    let thumb_start = (event_position.along(axis)
                        - padded_bounds.origin.along(axis)
                        - thumb_offset)
                        .clamp(px(0.), viewport_size - thumb_size);

                    let max_offset = max_offset.along(axis);
                    let percentage = if viewport_size > thumb_size {
                        thumb_start / (viewport_size - thumb_size)
                    } else {
                        0.
                    };

                    -max_offset * percentage
                };

            // Mouse down events - capture them before they reach the editor
            window.on_mouse_event({
                let state = self.state.clone();
                move |event: &MouseDownEvent, phase, window, _| {
                    if event.button != MouseButton::Left {
                        return;
                    }

                    // Only handle events within scrollbar bounds
                    if !bounds.contains(&event.position) {
                        return;
                    }

                    // Handle during capture phase to prevent editor selection
                    if phase.capture() {
                        if actual_thumb_bounds.contains(&event.position) {
                            let offset =
                                event.position.along(axis) - actual_thumb_bounds.origin.along(axis);
                            state.set_dragging(offset);
                        } else {
                            let scroll_handle = state.scroll_handle();
                            let click_offset = compute_click_offset(
                                event.position,
                                scroll_handle.max_offset(),
                                ScrollbarMouseEvent::GutterClick,
                            );
                            scroll_handle.set_offset(
                                scroll_handle.offset().apply_along(axis, |_| click_offset),
                            );
                            window.refresh();
                        }
                        // Event is consumed by handling it in capture phase
                    }
                }
            });

            // Scroll wheel events
            window.on_mouse_event({
                let scroll_handle = self.state.scroll_handle().clone();
                move |event: &ScrollWheelEvent, phase, window, _| {
                    if phase.bubble() && bounds.contains(&event.position) {
                        let current_offset = scroll_handle.offset();
                        scroll_handle.set_offset(
                            current_offset + event.delta.pixel_delta(window.line_height()),
                        );
                        window.refresh();
                    }
                }
            });

            // Mouse move events
            window.on_mouse_event({
                let state = self.state.clone();
                move |event: &MouseMoveEvent, phase, window, _| {
                    // Handle dragging in capture phase to prevent text selection
                    if phase.capture() && state.thumb_state.get().is_dragging() && event.dragging()
                    {
                        let scroll_handle = state.scroll_handle();
                        if let ThumbState::Dragging(drag_state) = state.thumb_state.get() {
                            let drag_offset = compute_click_offset(
                                event.position,
                                scroll_handle.max_offset(),
                                ScrollbarMouseEvent::ThumbDrag(drag_state),
                            );
                            scroll_handle.set_offset(
                                scroll_handle.offset().apply_along(axis, |_| drag_offset),
                            );
                            window.refresh();
                            // Event is consumed by handling it in capture phase
                        }
                    } else if phase.bubble() && event.pressed_button.is_none() {
                        // Track hover over entire scrollbar track for expansion effect
                        let over_track = bounds.contains(&event.position);
                        let was_track_hovered = state.track_hovered.get();
                        state.set_track_hovered(over_track);

                        // Track hover over thumb for thumb-specific styling
                        let over_thumb = actual_thumb_bounds.contains(&event.position);
                        let was_thumb_hover = matches!(state.thumb_state.get(), ThumbState::Hover);
                        state.set_thumb_hovered(over_thumb);

                        // Refresh if any hover state changed
                        if over_track != was_track_hovered
                            || over_thumb != was_thumb_hover
                            || was_thumb_hover
                        {
                            window.refresh();
                        }
                    }
                }
            });

            // Mouse up events
            window.on_mouse_event({
                let state = self.state.clone();
                move |event: &MouseUpEvent, phase, window, _| {
                    // Handle in capture phase if we were dragging
                    if phase.capture() && state.is_dragging() {
                        state.scroll_handle().drag_ended();
                        state.set_track_hovered(bounds.contains(&event.position));
                        state.set_thumb_hovered(actual_thumb_bounds.contains(&event.position));
                        window.refresh();
                    } else if phase.bubble() && !state.is_dragging() {
                        // Update hover state for non-drag releases
                        state.set_track_hovered(bounds.contains(&event.position));
                        state.set_thumb_hovered(actual_thumb_bounds.contains(&event.position));
                        window.refresh();
                    }
                }
            });
        })
    }
}

impl IntoElement for Scrollbar {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
