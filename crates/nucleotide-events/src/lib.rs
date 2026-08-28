// ABOUTME: Cross-crate event definitions for decoupled communication
// ABOUTME: Domain events shared by application and UI crates

// Core event system modules
pub mod lsp_events;

// Bounded context event modules
pub mod completion;
pub mod document;
pub mod run;
pub mod terminal;
pub mod ui;
pub mod workspace;

// Essential re-exports for event system functionality
pub use lsp_events::{
    ActiveServerInfo, LspEvent, PlannedProjectLanguage, ProjectDetectionResult,
    ProjectHealthStatus, ProjectLanguageEvidence, ProjectLspCommand, ProjectLspCommandError,
    ProjectLspEvent, ProjectLspPlan, ProjectSessionResult, ProjectStatus, ProjectType,
    ServerHealthStatus, ServerStartResult, ServerStartupResult,
};
