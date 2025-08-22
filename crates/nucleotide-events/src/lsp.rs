// ABOUTME: LSP domain events for language server lifecycle and diagnostics
// ABOUTME: Consolidates ProjectLspEvent and LspEvent into unified domain

use helix_lsp::LanguageServerId;
use std::path::PathBuf;

/// LSP domain events - covers language server lifecycle, diagnostics, and LSP operations
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// Server lifecycle events
    ServerInitialized {
        server_id: LanguageServerId,
        server_name: String,
        capabilities: ServerCapabilities,
        workspace_root: PathBuf,
    },

    ServerExited {
        server_id: LanguageServerId,
        server_name: String,
        exit_code: Option<i32>,
        workspace_root: PathBuf,
    },

    ServerError {
        server_id: LanguageServerId,
        error: LspError,
        is_fatal: bool,
    },

    /// Project management
    ProjectDetected {
        workspace_root: PathBuf,
        project_type: ProjectType,
        recommended_servers: Vec<String>,
    },

    ProjectServersReady {
        workspace_root: PathBuf,
        active_servers: Vec<ActiveServer>,
    },

    /// Health monitoring
    HealthCheckCompleted {
        server_id: LanguageServerId,
        status: ServerHealth,
        response_time_ms: u64,
    },

    /// Progress reporting
    ProgressStarted {
        server_id: LanguageServerId,
        token: ProgressToken,
        title: String,
        message: Option<String>,
    },

    ProgressUpdated {
        server_id: LanguageServerId,
        token: ProgressToken,
        percentage: Option<u32>,
        message: Option<String>,
    },

    ProgressCompleted {
        server_id: LanguageServerId,
        token: ProgressToken,
        final_message: Option<String>,
    },

    /// Server startup requested (command pattern event)
    ServerStartupRequested {
        workspace_root: PathBuf,
        server_name: String,
        language_id: String,
    },

    /// Server restart completed
    ServerRestarted {
        server_id: LanguageServerId,
        server_name: String,
        downtime_ms: u64,
    },
}

/// Server capabilities reported during initialization
#[derive(Debug, Clone)]
pub struct ServerCapabilities {
    pub completion: bool,
    pub hover: bool,
    pub signature_help: bool,
    pub definition: bool,
    pub diagnostics: bool,
    pub code_action: bool,
    pub formatting: bool,
    pub rename: bool,
}

/// Active server information
#[derive(Debug, Clone)]
pub struct ActiveServer {
    pub server_id: LanguageServerId,
    pub server_name: String,
    pub language_ids: Vec<String>,
    pub health: ServerHealth,
    pub startup_time_ms: u64,
}

/// Health status of language server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerHealth {
    Healthy,
    Degraded,
    Unhealthy,
    Starting,
    Stopped,
}

/// LSP error information
#[derive(Debug, Clone)]
pub struct LspError {
    pub code: i32,
    pub message: String,
    pub data: Option<String>,
}

/// Project type detected from workspace
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    CSharp,
    Cpp,
    Unknown,
}

/// Progress token for tracking long-running operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressToken(pub String);

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerCapabilities {
    pub fn new() -> Self {
        Self {
            completion: false,
            hover: false,
            signature_help: false,
            definition: false,
            diagnostics: false,
            code_action: false,
            formatting: false,
            rename: false,
        }
    }

    pub fn with_completion(mut self) -> Self {
        self.completion = true;
        self
    }

    pub fn with_hover(mut self) -> Self {
        self.hover = true;
        self
    }

    pub fn with_diagnostics(mut self) -> Self {
        self.diagnostics = true;
        self
    }

    pub fn has_basic_features(&self) -> bool {
        self.completion || self.hover || self.diagnostics
    }
}

impl LspError {
    pub fn new(code: i32, message: String) -> Self {
        Self {
            code,
            message,
            data: None,
        }
    }

    pub fn with_data(mut self, data: String) -> Self {
        self.data = Some(data);
        self
    }
}

impl ProgressToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ProjectType {
    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "ts" | "tsx" => Self::TypeScript,
            "js" | "jsx" => Self::JavaScript,
            "py" => Self::Python,
            "go" => Self::Go,
            "java" => Self::Java,
            "cs" => Self::CSharp,
            "cpp" | "cc" | "cxx" | "c" | "h" | "hpp" => Self::Cpp,
            _ => Self::Unknown,
        }
    }

    pub fn recommended_servers(&self) -> Vec<&'static str> {
        match self {
            Self::Rust => vec!["rust-analyzer"],
            Self::TypeScript => vec!["typescript-language-server"],
            Self::JavaScript => vec!["typescript-language-server"],
            Self::Python => vec!["pylsp", "pyright"],
            Self::Go => vec!["gopls"],
            Self::Java => vec!["jdtls"],
            Self::CSharp => vec!["omnisharp"],
            Self::Cpp => vec!["clangd"],
            Self::Unknown => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_capabilities() {
        let caps = ServerCapabilities::new()
            .with_completion()
            .with_hover()
            .with_diagnostics();

        assert!(caps.completion);
        assert!(caps.hover);
        assert!(caps.diagnostics);
        assert!(!caps.formatting);
        assert!(caps.has_basic_features());
    }

    #[test]
    fn test_project_type_detection() {
        assert_eq!(ProjectType::from_extension("rs"), ProjectType::Rust);
        assert_eq!(ProjectType::from_extension("ts"), ProjectType::TypeScript);
        assert_eq!(ProjectType::from_extension("py"), ProjectType::Python);
        assert_eq!(ProjectType::from_extension("unknown"), ProjectType::Unknown);
    }

    #[test]
    fn test_recommended_servers() {
        let rust_servers = ProjectType::Rust.recommended_servers();
        assert!(rust_servers.contains(&"rust-analyzer"));

        let python_servers = ProjectType::Python.recommended_servers();
        assert!(python_servers.len() > 1); // Multiple options for Python
    }

    #[test]
    fn test_lsp_error_creation() {
        let error = LspError::new(-32601, "Method not found".to_string())
            .with_data("Additional error context".to_string());

        assert_eq!(error.code, -32601);
        assert_eq!(error.message, "Method not found");
        assert!(error.data.is_some());
    }

    #[test]
    fn test_progress_token() {
        let token = ProgressToken::new("work-done-123");
        assert_eq!(token.as_str(), "work-done-123");

        let token2 = ProgressToken::new(String::from("another-token"));
        assert_eq!(token2.0, "another-token");
    }

    #[test]
    fn test_server_health_states() {
        let health_states = [
            ServerHealth::Healthy,
            ServerHealth::Degraded,
            ServerHealth::Unhealthy,
            ServerHealth::Starting,
            ServerHealth::Stopped,
        ];

        for health in health_states {
            let _server = ActiveServer {
                server_id: LanguageServerId::default(),
                server_name: "test-server".to_string(),
                language_ids: vec!["rust".to_string()],
                health,
                startup_time_ms: 1000,
            };
        }
    }
}
