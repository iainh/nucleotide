//! Scrollbar component for GPUI based on Zed's implementation
//! Provides vertical and horizontal scrollbars for scrollable content

use std::{any::Any, cell::Cell, fmt::Debug, ops::Range, rc::Rc, sync::Arc};

use crate::Core;
use gpui::*;
use helix_view::{DocumentId, ViewId};

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

/// A scroll handle that integrates with the Helix editor
#[derive(Clone, Debug)]
pub struct HelixEditorScrollHandle {
    core: Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    viewport_size: Rc<Cell<Size<Pixels>>>,
    input: Entity<crate::Input>,
}

impl HelixEditorScrollHandle {
    pub fn new(
        core: Entity<Core>,
        doc_id: DocumentId,
        view_id: ViewId,
        input: Entity<crate::Input>,
    ) -> Self {
        Self {
            core,
            doc_id,
            view_id,
            viewport_size: Rc::new(Cell::new(size(px(800.0), px(600.0)))), // Default size
            input,
        }
    }

    pub fn set_viewport_size(&self, size: Size<Pixels>) {
        self.viewport_size.set(size);
    }
}

impl ScrollableHandle for HelixEditorScrollHandle {
    fn max_offset(&self) -> Size<Pixels> {
        // Calculate based on actual document lines and viewport
        // TODO: Fix this when we need the HelixEditorScrollHandle
        /*if let Ok(core) = self.core.try_read() {
            let editor = &core.editor;
            if let Some(document) = editor.document(self.doc_id) {
                let total_lines = document.text().len_lines();
                let line_height = px(20.0); // TODO: Get from theme/config
                let content_height = px(total_lines as f32 * line_height.0);
                let viewport_height = self.viewport_size.get().height;

                // Max offset is content height minus viewport height
                let max_y = (content_height - viewport_height).max(px(0.0));
                return size(px(0.0), max_y);
            }
        }*/

        // Fallback to default for now
        size(px(0.0), px(2000.0)) // Large vertical scrollable area for testing
    }

    fn set_offset(&self, point: Point<Pixels>) {
        // Convert pixel offset to line-based scrolling for Helix
        let line_height = px(20.0);
        let lines_offset = (-point.y / line_height) as usize;

        // This would need to emit a scroll event to the helix editor
        // For now, we'll leave this as a placeholder
        log::debug!("Scrollbar setting offset to {} lines", lines_offset);
    }

    fn offset(&self) -> Point<Pixels> {
        // Return current scroll offset in pixels
        // This would need to be calculated from the helix editor's current position
        point(px(0.0), px(0.0)) // Placeholder
    }

    fn viewport(&self) -> Bounds<Pixels> {
        let size = self.viewport_size.get();
        Bounds::new(point(px(0.0), px(0.0)), size)
    }
}

/// Scrollbar state that should be persisted across frames
#[derive(Clone, Debug)]
pub struct ScrollbarState {
    thumb_state: Rc<Cell<ThumbState>>,
    scroll_handle: Arc<dyn ScrollableHandle>,
}

impl ScrollbarState {
    pub fn new(scroll: impl ScrollableHandle) -> Self {
        Self {
            thumb_state: Default::default(),
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
        let start_offset = if max_offset.0 > 0.0 {
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
        // Only create scrollbar if content doesn't fit in viewport
        let thumb = state.thumb_range(axis)?;
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

        // Recalculate thumb position every paint to reflect current scroll state
        self.thumb = self.state.thumb_range(self.axis).unwrap_or(0.0..1.0);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let axis = self.axis;
            let thumb_state = self.state.thumb_state.get();

            // Use theme colors - fallback to simple grays if theme is not available
            let (thumb_bg, track_bg) = {
                if let Some(_theme) = cx.try_global::<crate::ui::Theme>() {
                    let thumb_base_color = match thumb_state {
                        ThumbState::Dragging(_) => hsla(0.0, 0.0, 0.6, 0.9),
                        ThumbState::Hover => hsla(0.0, 0.0, 0.5, 0.7),
                        ThumbState::Inactive => hsla(0.0, 0.0, 0.4, 0.5),
                    };
                    (thumb_base_color, hsla(0.0, 0.0, 0.2, 0.2)) // Subtle track
                } else {
                    // Fallback colors - more subtle
                    let thumb_base_color = match thumb_state {
                        ThumbState::Dragging(_) => hsla(0.0, 0.0, 0.6, 0.9),
                        ThumbState::Hover => hsla(0.0, 0.0, 0.5, 0.7),
                        ThumbState::Inactive => hsla(0.0, 0.0, 0.4, 0.5),
                    };
                    (thumb_base_color, hsla(0.0, 0.0, 0.2, 0.2)) // Subtle track
                }
            };

            // Paint the track background first
            window.paint_quad(fill(bounds, track_bg));

            let padded_bounds = Bounds::from_corners(
                bounds
                    .origin
                    .apply_along(axis, |origin| origin + EXTRA_PADDING),
                bounds
                    .bottom_right()
                    .apply_along(axis, |track_end| track_end - EXTRA_PADDING),
            );

            let thumb_offset = self.thumb.start * padded_bounds.size.along(axis);
            let thumb_end = self.thumb.end * padded_bounds.size.along(axis);

            // Center the thumb within the scrollbar gutter
            let thumb_width = padded_bounds.size.along(axis.invert()) * 0.5; // Make thumb half the gutter width
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
                        // Handle hover state in bubble phase
                        state.set_thumb_hovered(actual_thumb_bounds.contains(&event.position))
                    }
                }
            });

            // Mouse up events
            window.on_mouse_event({
                let state = self.state.clone();
                move |event: &MouseUpEvent, phase, _window, _| {
                    // Handle in capture phase if we were dragging
                    if phase.capture() && state.is_dragging() {
                        state.scroll_handle().drag_ended();
                        state.set_thumb_hovered(actual_thumb_bounds.contains(&event.position));
                        // Event is consumed by handling it in capture phase
                    } else if phase.bubble() && !state.is_dragging() {
                        // Update hover state for non-drag releases
                        state.set_thumb_hovered(actual_thumb_bounds.contains(&event.position));
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
