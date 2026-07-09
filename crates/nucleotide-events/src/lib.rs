// ABOUTME: Cross-crate event definitions for decoupled communication
// ABOUTME: V2 architecture with domain-driven bounded contexts

// Core event system modules
pub mod lsp_events;

// V2 Bounded Context Event Modules
pub mod completion;
pub mod document;
pub mod run;
pub mod terminal;
pub mod ui;
pub mod workspace;

// Re-export V2 bounded context events
pub mod v2 {
    pub use crate::completion;
    pub use crate::document;
    pub use crate::run;
    pub use crate::terminal;
    pub use crate::ui;
    pub use crate::workspace;
}

// Essential re-exports for event system functionality
pub use lsp_events::{
    ActiveServerInfo, LspEvent, ProjectDetectionResult, ProjectHealthStatus, ProjectLspCommand,
    ProjectLspCommandError, ProjectLspEvent, ProjectStatus, ProjectType, ServerHealthStatus,
    ServerStartResult, ServerStartupResult,
};
