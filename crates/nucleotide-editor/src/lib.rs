// ABOUTME: Editor view crate that renders documents without circular dependencies
// ABOUTME: Uses capability traits to depend on abstractions rather than concrete types

pub mod cursor;
pub mod cursor_style;
pub mod diagnostics;
pub mod document_metrics;
pub mod document_renderer;
pub mod editor_view;
pub mod geometry;
pub mod gutter;
pub mod highlight;
pub mod hit_test;
pub mod line_cache;
pub mod line_painter;
pub mod line_plan;
pub mod line_text;
pub mod metrics;
pub mod render_snapshot;
pub mod ruler;
pub mod scroll_manager;
pub mod scroll_state;
pub mod scrollbar;
pub mod selection;
pub mod soft_wrap;
mod style;
pub mod surface;
pub mod viewport;

pub use cursor::{
    CursorLinePosition, CursorTextShape, CursorViewportPosition, EditorCursor, block_cursor_text,
    cursor_background_color, cursor_document_line, cursor_foreground_color, cursor_line_position,
    cursor_text_run, cursor_viewport_position, phantom_line_cursor_paint_position,
    shape_cursor_text, soft_wrap_cursor_paint_position, unwrapped_cursor_paint_position,
};
pub use cursor_style::{cursor_has_reversed_modifier, cursor_style_for_mode};
pub use diagnostics::{
    DiagnosticMarkerHighlight, DiagnosticMarkerPaintStyle, DiagnosticMarkerPlan,
    DiagnosticMarkerShape, DiagnosticSeverityByLine, diagnostic_marker_paint_style,
    diagnostic_marker_plan, diagnostic_severity_by_line, paint_diagnostic_marker,
};
pub use document_metrics::{
    EditorDocumentMetrics, document_text_format_for_surface, visual_rows_for_text,
};
pub use document_renderer::DocumentRenderer;
pub use editor_view::EditorView;
pub use geometry::{EditorLayout, EditorSurfaceGeometry};
pub use gutter::{
    GutterLine, GutterLineParams, SoftWrapGutterLinePaintPlan, SoftWrapGutterLinePlan,
    build_gutter_lines, build_soft_wrap_gutter_lines, paint_gutter_lines,
    paint_soft_wrap_gutter_lines, soft_wrap_gutter_line_paint_plans, soft_wrap_gutter_line_plans,
};
pub use highlight::{
    DiagnosticOverlaySpans, HighlightLineParams, diagnostic_overlay_spans,
    gpui_hsla_to_helix_color, highlight_line, text_style_at_position,
};
pub use hit_test::{EditorHitTestResult, hit_test_document_position};
pub use line_cache::{LineLayout, LineLayoutCache};
pub use line_painter::{
    EditorLineBackgroundStyle, paint_cursorline_background, paint_editor_line,
    paint_line_backgrounds,
};
pub use line_plan::{
    LineViewportPlan, UnwrappedLinePaintPlan, UnwrappedRenderPlan, UnwrappedRenderPlanParams,
    VisibleLinePlan, line_viewport_plan, unwrapped_line_paint_plans, unwrapped_render_plan,
    unwrapped_visible_line_plans,
};
pub use line_text::{
    byte_offset_for_char_offset, line_text_without_trailing_newline,
    shared_line_text_without_trailing_newline,
};
pub use metrics::EditorTextMetrics;
pub use render_snapshot::{
    EditorRenderSnapshot, document_render_snapshot, render_snapshot_for_cursor,
};
pub use ruler::{
    RulerPaintPlan, paint_visible_rulers, visible_ruler_bounds, visible_ruler_paint_plans,
};
pub use scroll_manager::ScrollManager;
pub use scroll_state::ScrollState;
pub use scrollbar::{
    EditorScrollbar, EditorScrollbarState, EditorScrollbarThumb, editor_scrollbar_thumb,
    scroll_position_for_scrollbar_pointer,
};
pub use selection::{
    EditorPointerSelectionUpdate, EditorSelectionDragState, EditorSelectionUpdate,
    apply_pointer_selection, begin_editor_pointer_selection_at_event, begin_pointer_selection,
    begin_pointer_selection_at_event, pointer_selection_anchor, primary_selection_anchor,
    selection_for_range, update_editor_pointer_selection_at_event, update_pointer_selection,
    update_pointer_selection_at_event,
};
pub use soft_wrap::{
    DocumentSoftWrapRenderPlanParams, SoftWrapLinePaintPlan, SoftWrapRenderPlan,
    SoftWrapRenderPlanParams, SoftWrapVisualLine, SoftWrapVisualPosition,
    decorate_soft_wrap_line_runs, document_soft_wrap_render_plan, soft_wrap_line_paint_plans,
    soft_wrap_render_plan, soft_wrap_viewport_height, soft_wrap_visual_lines,
    soft_wrap_visual_position,
};
pub use surface::{
    EditorSurface, EditorSurfaceMetricSnapshot, EditorSurfaceMetrics, EditorSurfacePointerEvent,
    paint_editor_background,
};
pub use viewport::{
    EditorViewport, EditorViewportSurfaceLayout, EditorViewportSurfaceUpdate,
    HelixViewportSnapshot, ViewportScrollUpdate, editor_viewport_size_for_bounds,
    helix_viewport_snapshot,
};
