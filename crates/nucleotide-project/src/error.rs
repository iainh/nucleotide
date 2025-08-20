// ABOUTME: Error types for project detection and manifest provider operations
// ABOUTME: Provides structured error handling with context and proper error chains

use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ProjectError>;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse manifest file at {path}: {source}")]
    ManifestParse {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Invalid manifest format in {path}: {reason}")]
    InvalidManifest { path: PathBuf, reason: String },

    #[error("Manifest provider '{name}' not found")]
    ProviderNotFound { name: String },

    #[error("Multiple providers registered for manifest '{name}'")]
    DuplicateProvider { name: String },

    #[error("Path traversal exceeded maximum depth of {max_depth}")]
    MaxDepthExceeded { max_depth: usize },

    #[error("Invalid file path: {path}")]
    InvalidPath { path: PathBuf },

    #[error("Provider registration failed: {reason}")]
    ProviderRegistration { reason: String },

    #[error("File system access denied for path: {path}")]
    AccessDenied { path: PathBuf },

    #[error("Circular dependency detected in manifest search")]
    CircularDependency,
}

impl ProjectError {
    /// Create a manifest parse error with context
    pub fn manifest_parse<E>(path: PathBuf, error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::ManifestParse {
            path,
            source: Box::new(error),
        }
    }

    /// Create an invalid manifest error
    pub fn invalid_manifest<S: Into<String>>(path: PathBuf, reason: S) -> Self {
        Self::InvalidManifest {
            path,
            reason: reason.into(),
        }
    }

    /// Create a provider not found error
    pub fn provider_not_found<S: Into<String>>(name: S) -> Self {
        Self::ProviderNotFound { name: name.into() }
    }

    /// Create a duplicate provider error
    pub fn duplicate_provider<S: Into<String>>(name: S) -> Self {
        Self::DuplicateProvider { name: name.into() }
    }

    /// Create a max depth exceeded error
    pub fn max_depth_exceeded(max_depth: usize) -> Self {
        Self::MaxDepthExceeded { max_depth }
    }

    /// Create an invalid path error
    pub fn invalid_path(path: PathBuf) -> Self {
        Self::InvalidPath { path }
    }

    /// Create a provider registration error
    pub fn provider_registration<S: Into<String>>(reason: S) -> Self {
        Self::ProviderRegistration {
            reason: reason.into(),
        }
    }

    /// Create an access denied error
    pub fn access_denied(path: PathBuf) -> Self {
        Self::AccessDenied { path }
    }

    /// Check if this error is recoverable (i.e., should continue searching)
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::ManifestParse { .. } | Self::InvalidManifest { .. } | Self::AccessDenied { .. }
        )
    }

    /// Check if this error should stop all searching
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::MaxDepthExceeded { .. } | Self::CircularDependency | Self::InvalidPath { .. }
        )
    }
}

/// Helper trait for converting results with path context
pub trait WithPathContext<T> {
    fn with_path_context(self, path: PathBuf) -> Result<T>;
}

impl<T, E> WithPathContext<T> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn with_path_context(self, path: PathBuf) -> Result<T> {
        self.map_err(|e| ProjectError::manifest_parse(path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_error_creation() {
        let path = PathBuf::from("/test/path");

        let parse_error = ProjectError::manifest_parse(
            path.clone(),
            std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"),
        );
        assert!(matches!(parse_error, ProjectError::ManifestParse { .. }));

        let invalid_error = ProjectError::invalid_manifest(path.clone(), "Invalid TOML");
        assert!(matches!(
            invalid_error,
            ProjectError::InvalidManifest { .. }
        ));

        let not_found_error = ProjectError::provider_not_found("test");
        assert!(matches!(
            not_found_error,
            ProjectError::ProviderNotFound { .. }
        ));
    }

    #[test]
    fn test_error_properties() {
        let recoverable = ProjectError::invalid_manifest(PathBuf::from("/test"), "test");
        assert!(recoverable.is_recoverable());
        assert!(!recoverable.is_fatal());

        let fatal = ProjectError::max_depth_exceeded(10);
        assert!(!fatal.is_recoverable());
        assert!(fatal.is_fatal());
    }

    #[test]
    fn test_with_path_context() {
        let path = PathBuf::from("/test/path");
        let io_error: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Access denied",
        ));

        let result = io_error.with_path_context(path.clone());
        assert!(result.is_err());

        if let Err(ProjectError::ManifestParse {
            path: error_path, ..
        }) = result
        {
            assert_eq!(error_path, path);
        } else {
            panic!("Expected ManifestParse error");
        }
    }
}
