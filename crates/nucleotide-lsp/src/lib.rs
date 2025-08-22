// ABOUTME: LSP integration layer for Nucleotide
// ABOUTME: Manages language servers, diagnostics, and code intelligence features

pub mod coordination_manager;
pub mod document_manager;
pub mod helix_lsp_bridge;
pub mod lsp_completion_trigger;
pub mod lsp_state;
pub mod lsp_status;
pub mod project_lsp_manager;

#[cfg(test)]
pub mod integration_tests;

#[cfg(test)]
pub mod mock_server_tests;

#[cfg(test)]
pub mod stress_tests;

#[cfg(test)]
pub mod command_flow_test;

pub use coordination_manager::{
    LspError, LspManager, LspManagerConfig, LspStartupMode, LspStartupResult, LspStartupStats,
};
pub use document_manager::{DocumentManager, DocumentManagerMut};
pub use helix_lsp_bridge::{EditorLspIntegration, EnvironmentProvider, HelixLspBridge};
// Note: lsp_completion_trigger module only contains functions, no LspCompletionTrigger type
pub use lsp_state::{LspProgress, LspState, ServerStatus};
pub use lsp_status::LspStatus;
pub use project_lsp_manager::{
    ManagedServer, ProjectDetector, ProjectInfo, ProjectLspConfig, ProjectLspError,
    ProjectLspManager, ServerLifecycleManager,
};
