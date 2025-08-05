// ABOUTME: Core modules for the helix-gpui application
// ABOUTME: Provides domain-specific functionality extracted from the main Application struct

pub mod document_manager;
pub mod lsp_manager;
pub mod lsp_state;
// Removed input_handler - now using event-driven input system
// Removed ui_factory - now using event-driven UI creation

pub use document_manager::{DocumentManager, DocumentManagerMut};
pub use lsp_manager::LspManager;
// pub use lsp_state::LspState;
// pub use input_handler::InputHandler;
// pub use ui_factory::UiFactory;