// ABOUTME: Cross-crate event definitions for decoupled communication
// ABOUTME: V2 architecture with domain-driven bounded contexts

// Core event system modules
pub mod event_bus;
pub mod lsp_events;

// V2 Bounded Context Event Modules
pub mod bridge;
pub mod completion;
pub mod document;
pub mod editor;
pub mod handler;
pub mod integration;
pub mod lsp;
pub mod ui;
pub mod vcs;
pub mod view;
pub mod workspace;

// Re-export V2 bounded context events
pub mod v2 {
    pub use crate::bridge;
    pub use crate::completion;
    pub use crate::document;
    pub use crate::editor;
    pub use crate::handler;
    pub use crate::integration;
    pub use crate::lsp;
    pub use crate::ui;
    pub use crate::vcs;
    pub use crate::view;
    pub use crate::workspace;
}

// Essential re-exports for event system functionality
pub use event_bus::{EventBus, EventHandler};
pub use lsp_events::{
    ActiveServerInfo, LspEvent, ProjectDetectionResult, ProjectHealthStatus, ProjectLspCommand,
    ProjectLspCommandError, ProjectLspEvent, ProjectStatus, ProjectType, ServerHealthStatus,
    ServerStartResult, ServerStartupResult,
};
