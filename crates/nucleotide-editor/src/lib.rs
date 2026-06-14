// ABOUTME: Editor view crate that renders documents without circular dependencies
// ABOUTME: Uses capability traits to depend on abstractions rather than concrete types

/// Minimum text columns used when computing native editor soft-wrap layout.
///
/// The viewport scroll model and frame painter must use the same floor so
/// visual-row counts match the rows that are actually painted.
pub const EDITOR_MINIMUM_VIEWPORT_COLUMNS: u16 = 10;

pub mod cursor;
pub mod cursor_style;
pub mod diagnostics;
pub mod document_element;
pub mod document_frame;
pub mod document_frame_painter;
pub mod document_metrics;
pub mod geometry;
pub mod gutter;
pub mod highlight;
pub mod hit_test;
pub mod line_cache;
pub mod line_painter;
pub mod line_plan;
pub mod line_text;
pub mod metrics;
pub mod overlay_state;
pub mod render_snapshot;
pub mod ruler;
pub mod scroll_manager;
pub mod scrollbar;
pub mod selection;
pub mod soft_wrap;
mod style;
pub mod surface;
pub mod view_component;
pub mod view_state;
pub mod viewport;

pub use cursor::{
    CursorLinePosition, CursorOverlayPlan, CursorTextShape, CursorViewportPosition, EditorCursor,
    EditorCursorPresentation, EditorCursorPresentationParams, EditorCursorTextPaintParams,
    ShapedEditorCursorPaintParams, ShapedEditorCursorPlan, ShapedEditorCursorPlanParams,
    SoftWrapCursorPaintPlan, SoftWrapCursorPaintPlanParams, UnwrappedCursorPaintPlan,
    UnwrappedCursorPaintPlanParams, UnwrappedCursorPaintPlanSource, block_cursor_text,
    cursor_background_color, cursor_document_line, cursor_document_line_for_view,
    cursor_foreground_color, cursor_line_position, cursor_overlay_plan, cursor_text_run,
    cursor_viewport_position, editor_cursor_presentation, paint_shaped_editor_cursor,
    phantom_line_cursor_paint_position, shape_and_paint_editor_cursor, shape_cursor_text,
    shaped_editor_cursor_plan, soft_wrap_cursor_paint_plan, soft_wrap_cursor_paint_position,
    unwrapped_cursor_paint_plan, unwrapped_cursor_paint_position,
};
pub use cursor_style::{cursor_has_reversed_modifier, cursor_style_for_mode};
pub use diagnostics::{
    DiagnosticGutterMarkerPaintPlan, DiagnosticGutterMarkerPaintPlanParams,
    DiagnosticGutterMarkersPaintParams, DiagnosticMarkerHighlight, DiagnosticMarkerPaintStyle,
    DiagnosticMarkerPlan, DiagnosticMarkerShape, DiagnosticSeverityByLine,
    diagnostic_gutter_marker_paint_plan, diagnostic_marker_paint_style, diagnostic_marker_plan,
    diagnostic_severity_by_line, diagnostic_severity_color, diagnostic_severity_theme_key,
    paint_diagnostic_gutter_markers, paint_diagnostic_marker,
};
pub use document_element::EditorDocumentElement;
pub use document_frame::{
    EditorDocumentFrame, EditorDocumentFrameFromEditorParams, EditorDocumentFrameGutterParams,
    EditorDocumentFrameParams, editor_document_frame, editor_document_frame_from_editor,
};
pub use document_frame_painter::{
    DocumentFramePaintParams, NativeEditorFramePaintParams, NativeEditorFramePaintPlan,
    NativeEditorFramePaintStyle, NativeEditorFramePaintStyleParams, NativeEditorFramePalette,
    NativeEditorFramePlanParams, NativeEditorFramePrepareParams, NativeEditorFrameRenderParams,
    NativeEditorFrameThemeStyles, NativeEditorPreparedFrame, native_editor_frame_paint_plan,
    native_editor_frame_paint_style, paint_document_frame, paint_native_editor_frame,
    prepare_native_editor_frame, render_native_editor_frame,
};
pub use document_metrics::{
    EditorDocumentMetrics, document_text_format_for_surface, visual_rows_for_text,
};
pub use geometry::{EditorLayout, EditorSurfaceGeometry};
pub use gutter::{
    GutterLine, GutterLineParams, GutterLinePlan, GutterLinePlanParams,
    SoftWrapGutterLinePaintPlan, SoftWrapGutterLinePlan, SoftWrapGutterPaintParams,
    build_gutter_line_plans, build_gutter_lines, build_gutter_lines_from_plans,
    build_soft_wrap_gutter_for_visual_lines, build_soft_wrap_gutter_lines, paint_gutter_lines,
    paint_soft_wrap_gutter, paint_soft_wrap_gutter_lines, soft_wrap_gutter_line_paint_plans,
    soft_wrap_gutter_line_plans,
};
pub use highlight::{
    DiagnosticOverlaySpans, EditorLineHighlightContext, HighlightLineParams,
    SoftWrapHighlightedLineRunsParams, UnwrappedHighlightedLine, UnwrappedHighlightedLineParams,
    diagnostic_overlay_spans, gpui_hsla_to_helix_color, highlight_line,
    soft_wrap_highlighted_line_runs, text_style_at_position, unwrapped_highlighted_line,
};
pub use hit_test::{EditorHitTestResult, hit_test_document_position};
pub use line_cache::{LineLayout, LineLayoutCache};
pub use line_painter::{
    EditorLineBackgroundStyle, SoftWrapEditorLinePaintParams, UnwrappedEditorLinePaintParams,
    paint_cursorline_background, paint_editor_line, paint_line_backgrounds,
    paint_soft_wrap_editor_line, paint_unwrapped_editor_line,
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
pub use overlay_state::EditorOverlayState;
pub use render_snapshot::{
    EditorRenderSnapshot, document_render_snapshot, render_snapshot_for_cursor,
};
pub use ruler::{
    DocumentRulerPaintParams, RulerPaintPlan, document_ruler_paint_plans, paint_document_rulers,
    paint_visible_rulers, visible_ruler_bounds, visible_ruler_paint_plans,
};
pub use scroll_manager::ScrollManager;
pub use scrollbar::{
    EditorScrollbar, EditorScrollbarState, EditorScrollbarThumb, editor_scrollbar_thumb,
    scroll_position_for_scrollbar_pointer,
};
pub use selection::{
    EditorPointerSelectionPhase, EditorPointerSelectionUpdate, EditorSelectionDragState,
    EditorSelectionUpdate, apply_pointer_selection, begin_editor_pointer_selection_at_event,
    begin_pointer_selection, begin_pointer_selection_at_event, pointer_selection_anchor,
    primary_selection_anchor, selection_for_range, update_editor_pointer_selection_at_event,
    update_pointer_selection, update_pointer_selection_at_event,
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
pub use view_component::NativeEditorView;
pub use view_state::{
    EditorViewContentPrepareParams, EditorViewContentState, EditorViewFrameState,
    EditorViewLayoutSnapshot, EditorViewState,
};
pub use viewport::{
    EditorCursorReveal, EditorViewport, EditorViewportContentLayout, EditorViewportContentUpdate,
    EditorViewportSurfaceLayout, EditorViewportSurfaceUpdate, HelixViewportSnapshot,
    ViewportScrollUpdate, document_cursor_visual_row, editor_viewport_size_for_bounds,
    helix_viewport_snapshot,
};
