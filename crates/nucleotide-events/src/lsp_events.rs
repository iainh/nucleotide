// ABOUTME: Language Server Protocol events
// ABOUTME: Events for LSP server lifecycle and communication

use helix_view::DocumentId;
use std::path::PathBuf;
use tokio::sync::oneshot;
use tracing::Span;

use crate::ProjectType;

/// LSP events (already in nucleotide-lsp crate)
#[derive(Debug, Clone)]
pub enum LspEvent {
    /// Server initialized
    ServerInitialized {
        server_id: helix_lsp::LanguageServerId,
    },

    /// Server exited
    ServerExited {
        server_id: helix_lsp::LanguageServerId,
    },

    /// Progress update
    Progress {
        server_id: usize,
        percentage: Option<u32>,
        message: String,
    },

    /// Completion available
    CompletionAvailable { doc_id: DocumentId },
}

/// Why a language should be started for a project before a document opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectLanguageEvidence {
    /// A primary project marker directly identifies the language.
    EagerProject,
    /// A project marker is itself written in this supporting language.
    EagerManifest,
    /// A bounded workspace inventory found a file for this language.
    DiscoveredLanguage,
}

/// One language in the proactive project LSP plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedProjectLanguage {
    pub language_id: String,
    pub evidence: ProjectLanguageEvidence,
}

/// Language-level plan produced when a project session opens.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectLspPlan {
    pub project_type: ProjectType,
    pub languages: Vec<PlannedProjectLanguage>,
}

/// Project-level LSP session operations.
///
/// Helix owns every language-server process. The application dispatcher is the
/// only command consumer and translates these session requests into Helix
/// registry operations.
#[derive(Debug)]
pub enum ProjectLspCommand {
    /// Open one authoritative project session, replacing any previous session.
    OpenProjectSession {
        workspace_root: PathBuf,
        response: oneshot::Sender<Result<ProjectSessionResult, ProjectLspCommandError>>,
        span: Span,
    },

    /// Restart the authoritative project session, even when the root is unchanged.
    RestartProjectSession {
        workspace_root: PathBuf,
        response: oneshot::Sender<Result<ProjectSessionResult, ProjectLspCommandError>>,
        span: Span,
    },
}

/// Result of opening the current project session.
#[derive(Debug, Clone)]
pub struct ProjectSessionResult {
    pub generation: u64,
    pub plan: ProjectLspPlan,
    pub language_servers: Vec<String>,
    pub servers_started: Vec<ServerStartResult>,
}

/// Result of server start
#[derive(Debug, Clone)]
pub struct ServerStartResult {
    pub server_id: helix_lsp::LanguageServerId,
    pub server_name: String,
    pub language_id: String,
}

/// Command execution errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProjectLspCommandError {
    #[error("Server startup failed: {0}")]
    ServerStartup(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Project session was superseded by a newer workspace")]
    StaleProjectSession,
}
