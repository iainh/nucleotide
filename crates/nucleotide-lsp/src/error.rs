// ABOUTME: Errors returned while translating application LSP requests to Helix.
// ABOUTME: Keeps lifecycle failures independent from project detection and UI state.

#[derive(Debug, thiserror::Error)]
pub enum ProjectLspError {
    #[error("Server startup failed: {0}")]
    ServerStartup(String),

    #[error("Server communication failed: {0}")]
    ServerCommunication(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("Internal error: {0}")]
    Internal(String),
}
