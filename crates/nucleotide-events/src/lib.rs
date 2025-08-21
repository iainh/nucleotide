// ABOUTME: Cross-crate event definitions for decoupled communication
// ABOUTME: Second layer in the nucleotide architecture

pub mod app_event;
pub mod completion_events;
pub mod core_events;
pub mod event_bus;
pub mod lsp_events;
pub mod ui_events;
pub mod workspace_events;

// Re-export event types
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
