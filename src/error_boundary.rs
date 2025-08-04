// ABOUTME: Error boundary utilities for safe async operation handling
// ABOUTME: Provides helpers and patterns for robust error handling in async contexts

use std::future::Future;

/// Result type for async operations that might fail
pub type AsyncResult<T> = Result<T, AsyncError>;

/// Error type for async operations
#[derive(Debug, Clone)]
pub enum AsyncError {
    /// Operation was cancelled or context was dropped
    Cancelled,
    /// Operation failed with a specific error message
    Failed(String),
    /// Operation panicked (if caught)
    Panicked(String),
}

impl std::fmt::Display for AsyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsyncError::Cancelled => write!(f, "Async operation was cancelled"),
            AsyncError::Failed(msg) => write!(f, "Async operation failed: {}", msg),
            AsyncError::Panicked(msg) => write!(f, "Async operation panicked: {}", msg),
        }
    }
}

impl std::error::Error for AsyncError {}

/// Helper trait for converting errors to AsyncError
pub trait IntoAsyncError {
    fn into_async_error(self) -> AsyncError;
}

impl IntoAsyncError for anyhow::Error {
    fn into_async_error(self) -> AsyncError {
        AsyncError::Failed(self.to_string())
    }
}

impl IntoAsyncError for std::io::Error {
    fn into_async_error(self) -> AsyncError {
        AsyncError::Failed(self.to_string())
    }
}

/// Helper function to log and ignore async errors
pub fn log_async_error<T>(result: Result<T, impl std::error::Error>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(e) => {
            log::error!("Async operation failed: {}", e);
            None
        }
    }
}

/// Helper function to safely execute async operations with logging
pub async fn safe_async_operation<F, T, E>(operation: F) -> Option<T>
where
    F: Future<Output = Result<T, E>>,
    E: std::error::Error,
{
    match operation.await {
        Ok(value) => Some(value),
        Err(e) => {
            log::error!("Async operation failed: {}", e);
            None
        }
    }
}

/// Macro for wrapping async blocks with error boundaries
#[macro_export]
macro_rules! async_boundary {
    ($($body:tt)*) => {
        $crate::error_boundary::safe_async_operation(async move { $($body)* })
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_error_boundary_catches_panic() {
        let result = safe_async_operation(async {
            panic!("Test panic");
            #[allow(unreachable_code)]
            42
        })
        .await;
        
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Test panic"));
    }

    #[tokio::test]
    async fn test_error_boundary_passes_success() {
        let result = safe_async_operation(async { 42 }).await;
        assert_eq!(result.unwrap(), 42);
    }
}