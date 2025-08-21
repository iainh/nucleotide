// ABOUTME: Application module decomposition for V2 event system migration
// ABOUTME: Contains domain-specific handlers replacing monolithic Application.rs

pub mod document_handler;
pub mod editor_handler;
pub mod view_handler;

pub use document_handler::DocumentHandler;
pub use editor_handler::EditorHandler;
pub use view_handler::ViewHandler;
