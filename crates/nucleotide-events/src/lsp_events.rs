// ABOUTME: Language Server Protocol events
// ABOUTME: Events for LSP server lifecycle and communication

use helix_view::DocumentId;
use std::path::PathBuf;
use tokio::sync::oneshot;
use tracing::Span;

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

/// Project-level LSP events for proactive server management
#[derive(Debug, Clone)]
pub enum ProjectLspEvent {
    /// The authoritative project LSP session has opened.
    ProjectSessionOpened {
        generation: u64,
        workspace_root: PathBuf,
        backend_identity: String,
        proactive_startup: bool,
    },

    /// Project detected with language servers needed
    ProjectDetected {
        workspace_root: PathBuf,
        project_type: ProjectType,
        language_servers: Vec<String>,
    },

    /// Language server startup requested for project
    ServerStartupRequested {
        workspace_root: PathBuf,
        server_name: String,
        language_id: String,
    },

    /// Language server startup completed
    ServerStartupCompleted {
        workspace_root: PathBuf,
        server_name: String,
        server_id: helix_lsp::LanguageServerId,
        status: ServerStartupResult,
    },

    /// Health check completed
    HealthCheckCompleted {
        workspace_root: PathBuf,
        server_id: helix_lsp::LanguageServerId,
        status: ServerHealthStatus,
    },

    /// Project cleanup requested
    ProjectCleanupRequested { workspace_root: PathBuf },

    /// Server cleanup completed  
    ServerCleanupCompleted {
        workspace_root: PathBuf,
        server_id: helix_lsp::LanguageServerId,
    },
}

/// Type of project detected
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectType {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    C,
    Cpp,
    Mixed(Vec<ProjectType>),
    Other(String), // Custom project type with name
    Unknown,
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

/// Result of server startup attempt
#[derive(Debug, Clone)]
pub enum ServerStartupResult {
    Success,
    Failed { error: String },
    Timeout,
    ConfigurationError { error: String },
}

/// Server health status
#[derive(Debug, Clone)]
pub enum ServerHealthStatus {
    Healthy,
    Unresponsive,
    Failed { error: String },
    Crashed,
}

/// Command-based LSP operations with response handling and tracing
#[derive(Debug)]
pub enum ProjectLspCommand {
    /// Open one authoritative project session, replacing any previous session.
    OpenProjectSession {
        workspace_root: PathBuf,
        response: oneshot::Sender<Result<ProjectSessionResult, ProjectLspCommandError>>,
        span: Span,
    },

    /// Detect project and start servers if needed
    DetectAndStartProject {
        workspace_root: PathBuf,
        response: oneshot::Sender<Result<ProjectDetectionResult, ProjectLspCommandError>>,
        span: Span,
    },

    /// Start specific server for workspace
    StartServer {
        workspace_root: PathBuf,
        server_name: String,
        language_id: String,
        response: oneshot::Sender<Result<ServerStartResult, ProjectLspCommandError>>,
        span: Span,
    },

    /// Stop specific server
    StopServer {
        server_id: helix_lsp::LanguageServerId,
        response: oneshot::Sender<Result<(), ProjectLspCommandError>>,
        span: Span,
    },

    /// Restart LSP servers for workspace directory change
    RestartServersForWorkspaceChange {
        old_workspace_root: Option<PathBuf>,
        new_workspace_root: PathBuf,
        response: oneshot::Sender<Result<Vec<ServerStartResult>, ProjectLspCommandError>>,
        span: Span,
    },

    /// Get project status
    GetProjectStatus {
        workspace_root: PathBuf,
        response: oneshot::Sender<Result<ProjectStatus, ProjectLspCommandError>>,
        span: Span,
    },

    /// Ensure document is tracked by LSP server
    EnsureDocumentTracked {
        server_id: helix_lsp::LanguageServerId,
        doc_id: DocumentId,
        response: oneshot::Sender<Result<(), ProjectLspCommandError>>,
        span: Span,
    },

    /// LSP server startup requested (fire-and-forget)
    LspServerStartupRequested {
        server_name: String,
        workspace_root: PathBuf,
        language_id: String,
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

/// Result of project detection
#[derive(Debug, Clone)]
pub struct ProjectDetectionResult {
    pub project_type: ProjectType,
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

/// Current project status
#[derive(Debug, Clone)]
pub struct ProjectStatus {
    pub project_type: ProjectType,
    pub active_servers: Vec<ActiveServerInfo>,
    pub health_status: ProjectHealthStatus,
}

/// Information about active server
#[derive(Debug, Clone)]
pub struct ActiveServerInfo {
    pub server_id: helix_lsp::LanguageServerId,
    pub server_name: String,
    pub language_id: String,
    pub health: ServerHealthStatus,
}

/// Overall project health
#[derive(Debug, Clone)]
pub enum ProjectHealthStatus {
    Healthy,
    PartiallyHealthy,
    Degraded,
    Failed,
}

/// Command execution errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProjectLspCommandError {
    #[error("Project detection failed: {0}")]
    ProjectDetection(String),

    #[error("Server startup failed: {0}")]
    ServerStartup(String),

    #[error("Server not found")]
    ServerNotFound,

    #[error("Editor access required")]
    EditorAccessRequired,

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Project session was superseded by a newer workspace")]
    StaleProjectSession,
}
