// ABOUTME: LSP integration layer for Nucleotide
// ABOUTME: Manages language servers, diagnostics, and code intelligence features

pub mod document_manager;
pub mod error;
pub mod helix_lsp_bridge;
pub mod lsp_completion_trigger;
pub mod lsp_state;
pub mod lsp_status;

pub use document_manager::{DocumentManager, DocumentManagerMut};
pub use error::ProjectLspError;
pub use helix_lsp_bridge::{
    EnvironmentProvider, HelixLspBridge, LspLaunchProxy, LspLaunchProxyProvider,
};
// Note: lsp_completion_trigger module only contains functions, no LspCompletionTrigger type
pub use lsp_state::{
    LspProgress, LspState, LspStatusKind, LspStatusSummary, PlannedServerStatus,
    ProjectEnvironmentSource, ProjectLspSessionStatus, ProjectServerLifecycle, ServerStatus,
};
pub use lsp_status::LspStatus;
