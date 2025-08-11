// ABOUTME: Workspace management crate that orchestrates UI components without circular deps
// ABOUTME: Uses capability traits to depend on abstractions rather than concrete implementations

pub mod layout;
pub mod tab_manager;
pub mod workspace_manager;

pub use layout::{Layout, LayoutDirection, Panel};
pub use tab_manager::{Tab, TabManager};
pub use workspace_manager::WorkspaceManager;
