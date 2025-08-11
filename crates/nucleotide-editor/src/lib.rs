// ABOUTME: Editor view crate that renders documents without circular dependencies
// ABOUTME: Uses capability traits to depend on abstractions rather than concrete types

pub mod document_renderer;
pub mod editor_view;
pub mod line_cache;
pub mod scroll_manager;
pub mod scroll_state;

pub use document_renderer::DocumentRenderer;
pub use editor_view::EditorView;
pub use line_cache::{LineLayout, LineLayoutCache, ShapedLineKey};
pub use scroll_manager::ScrollManager;
pub use scroll_state::ScrollState;
