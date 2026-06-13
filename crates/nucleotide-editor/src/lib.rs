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
pub mod ruler;
pub mod scroll_manager;
pub mod scroll_state;
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
pub use diagnostics::{DiagnosticSeverityByLine, diagnostic_severity_by_line};
pub use document_metrics::{
    EditorDocumentMetrics, document_text_format_for_surface, visual_rows_for_text,
};
pub use document_renderer::DocumentRenderer;
pub use editor_view::EditorView;
pub use geometry::{EditorLayout, EditorSurfaceGeometry};
pub use gutter::{GutterLine, GutterLineParams, build_gutter_lines};
pub use highlight::{
    DiagnosticOverlaySpans, HighlightLineParams, diagnostic_overlay_spans,
    gpui_hsla_to_helix_color, highlight_line, text_style_at_position,
};
pub use hit_test::{EditorHitTestResult, hit_test_document_position};
pub use line_cache::{LineLayout, LineLayoutCache};
pub use line_painter::{EditorLineBackgroundStyle, paint_line_backgrounds};
pub use line_plan::{
    LineViewportPlan, VisibleLinePlan, line_viewport_plan, unwrapped_visible_line_plans,
};
pub use line_text::{
    byte_offset_for_char_offset, line_text_without_trailing_newline,
    shared_line_text_without_trailing_newline,
};
pub use metrics::EditorTextMetrics;
pub use ruler::visible_ruler_bounds;
pub use scroll_manager::ScrollManager;
pub use scroll_state::ScrollState;
pub use soft_wrap::{
    SoftWrapVisualLine, SoftWrapVisualPosition, soft_wrap_visual_lines, soft_wrap_visual_position,
};
pub use surface::{EditorSurface, EditorSurfacePointerEvent};
pub use viewport::{
    EditorViewport, HelixViewportSnapshot, ViewportScrollUpdate, helix_viewport_snapshot,
};
