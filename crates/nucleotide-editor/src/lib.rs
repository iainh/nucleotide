// ABOUTME: Editor view crate that renders documents without circular dependencies
// ABOUTME: Uses capability traits to depend on abstractions rather than concrete types

pub mod document_metrics;
pub mod document_renderer;
pub mod editor_view;
pub mod geometry;
pub mod hit_test;
pub mod line_cache;
pub mod metrics;
pub mod scroll_manager;
pub mod scroll_state;
pub mod surface;
pub mod viewport;

pub use document_metrics::{
    EditorDocumentMetrics, document_text_format_for_surface, visual_rows_for_text,
};
pub use document_renderer::DocumentRenderer;
pub use editor_view::EditorView;
pub use geometry::{EditorLayout, EditorSurfaceGeometry};
pub use hit_test::{EditorHitTestResult, hit_test_document_position};
pub use line_cache::{LineLayout, LineLayoutCache, ShapedLineKey, text_runs_hash};
pub use metrics::EditorTextMetrics;
pub use scroll_manager::ScrollManager;
pub use scroll_state::ScrollState;
pub use surface::{EditorSurface, EditorSurfacePointerEvent};
pub use viewport::{EditorViewport, ViewportScrollUpdate};
