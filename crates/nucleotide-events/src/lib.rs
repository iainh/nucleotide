// ABOUTME: Cross-crate event definitions for decoupled communication
// ABOUTME: V2 architecture with domain-driven bounded contexts

// Legacy modules (will be removed during migration)
pub mod app_event;
pub mod completion_events;
pub mod core_events;
pub mod event_bus;
pub mod lsp_events;
pub mod ui_events;
pub mod workspace_events;

// V2 Bounded Context Event Modules
pub mod completion;
pub mod document;
pub mod editor;
pub mod integration;
pub mod lsp;
pub mod ui;
pub mod view;
pub mod workspace;

// Re-export V2 bounded context events
pub mod v2 {
    pub use crate::completion;
    pub use crate::document;
    pub use crate::editor;
    pub use crate::integration;
    pub use crate::lsp;
    pub use crate::ui;
    pub use crate::view;
    pub use crate::workspace;
}

// Legacy re-exports (for backward compatibility during migration)
pub use app_event::AppEvent;
pub use completion_events::*;
pub use core_events::*;
pub use event_bus::{EventBus, EventHandler};
pub use lsp_events::{
    ActiveServerInfo, LspEvent, ProjectDetectionResult, ProjectHealthStatus, ProjectLspCommand,
    ProjectLspCommandError, ProjectLspEvent, ProjectStatus, ProjectType, ServerHealthStatus,
    ServerStartResult, ServerStartupResult,
};
pub use ui_events::*;
pub use workspace_events::*;
