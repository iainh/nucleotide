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
            AsyncError::Failed(msg) => write!(f, "Async operation failed: {msg}"),
            AsyncError::Panicked(msg) => write!(f, "Async operation panicked: {msg}"),
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
            log::error!("Async operation failed: {e}");
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
            log::error!("Async operation failed: {e}");
            None
        }
    }
}

/// Helper function to catch panics in async operations
pub async fn catch_panic<F, T>(operation: F) -> Result<T, String>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    use tokio::task;
    
    let handle = task::spawn(operation);
    match handle.await {
        Ok(value) => Ok(value),
        Err(e) => {
            if e.is_panic() {
                Err("Task panicked".to_string())
            } else {
                Err("Task cancelled".to_string())
            }
        }
    }
}

/// Retry an async operation with exponential backoff
pub async fn retry_async<F, T, E, Fut>(
    mut operation: F,
    max_attempts: usize,
) -> Result<T, AsyncError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::error::Error + 'static,
{
    use tokio::time::{sleep, Duration};
    
    let mut backoff_ms = 100;
    
    for attempt in 1..=max_attempts {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                log::warn!("Attempt {attempt}/{max_attempts} failed: {e}");
                
                if attempt < max_attempts {
                    sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(5000); // Cap at 5 seconds
                } else {
                    return Err(AsyncError::Failed(format!(
                        "Operation failed after {max_attempts} attempts: {e}"
                    )));
                }
            }
        }
    }
    
    unreachable!()
}

/// Execute an async operation with a timeout
pub async fn with_timeout<F, T>(
    operation: F,
    duration: std::time::Duration,
) -> Result<T, AsyncError>
where
    F: Future<Output = T>,
{
    use tokio::time::timeout;
    
    match timeout(duration, operation).await {
        Ok(value) => Ok(value),
        Err(_) => Err(AsyncError::Failed(format!(
            "Operation timed out after {duration:?}"
        ))),
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
    async fn test_catch_panic() {
        let result = catch_panic(async {
            panic!("Test panic");
            #[allow(unreachable_code)]
            42
        })
        .await;
        
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("panicked"));
    }

    #[tokio::test]
    async fn test_safe_async_operation_success() {
        let result = safe_async_operation(async { Ok::<i32, std::io::Error>(42) }).await;
        assert_eq!(result, Some(42));
    }

    #[tokio::test]
    async fn test_safe_async_operation_error() {
        let result = safe_async_operation(async { 
            Err::<i32, std::io::Error>(std::io::Error::new(
                std::io::ErrorKind::NotFound, 
                "Not found"
            ))
        }).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_retry_async() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let counter = AtomicUsize::new(0);
        
        let result = retry_async(|| async {
            let count = counter.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                Err(std::io::Error::new(std::io::ErrorKind::Other, "Temporary failure"))
            } else {
                Ok(42)
            }
        }, 3).await;
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_with_timeout() {
        use std::time::Duration;
        
        // Test timeout
        let result = with_timeout(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            42
        }, Duration::from_millis(100)).await;
        
        assert!(result.is_err());
        
        // Test success
        let result = with_timeout(async { 42 }, Duration::from_secs(1)).await;
        assert_eq!(result.unwrap(), 42);
    }
}