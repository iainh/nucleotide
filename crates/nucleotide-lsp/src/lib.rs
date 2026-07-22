// ABOUTME: LSP integration layer for Nucleotide
// ABOUTME: Manages language servers, diagnostics, and code intelligence features

pub mod document_manager;
pub mod extensions;
pub mod helix_lsp_bridge;
pub mod jdtls;
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

pub use document_manager::{DocumentManager, DocumentManagerMut};
pub use extensions::{
    CustomCommand, CustomNotification, CustomRequest, ExtensionMessageSeverity,
    LanguageServerExtension, ServerExtensionNotification, decode_server_notification,
    initialization_options_for_server,
};
pub use helix_lsp_bridge::{
    EditorLspIntegration, EnvironmentProvider, HelixLspBridge, LspLaunchProxy,
    LspLaunchProxyProvider,
};
// Note: lsp_completion_trigger module only contains functions, no LspCompletionTrigger type
pub use lsp_state::{
    LspProgress, LspState, LspStatusKind, LspStatusSummary, PlannedServerStatus,
    ProjectEnvironmentSource, ProjectLspSessionStatus, ProjectServerLifecycle, ServerStatus,
};
pub use lsp_status::LspStatus;
pub use project_lsp_manager::{
    ManagedServer, ProjectDetector, ProjectInfo, ProjectLspConfig, ProjectLspError,
    ProjectLspManager, ServerLifecycleManager,
};
