// ABOUTME: Core modules for the helix-gpui application
// ABOUTME: Provides domain-specific functionality extracted from the main Application struct

pub mod document_manager;
pub mod lsp_manager;
pub mod lsp_state;
pub mod input_handler;
pub mod ui_factory;

pub use document_manager::{DocumentManager, DocumentManagerMut};
pub use lsp_manager::LspManager;
// pub use lsp_state::LspState;
// pub use input_handler::InputHandler;
// pub use ui_factory::UiFactory;