// ABOUTME: Event handler traits and implementations for V2 event system
// ABOUTME: Provides domain-specific event handlers replacing channel-based communication

use async_trait::async_trait;
use std::fmt::Debug;

/// Generic event handler trait for domain events
/// Each domain (document, view, editor, etc.) can have specific handlers
#[async_trait]
pub trait EventHandler<E: Debug + Send + Sync + 'static> {
    type Error: Debug + Send + Sync;

    /// Handle a domain event asynchronously
    async fn handle(&mut self, event: E) -> Result<(), Self::Error>;

    /// Optional: Handle multiple events in batch for performance
    async fn handle_batch(&mut self, events: Vec<E>) -> Result<(), Self::Error> {
        for event in events {
            self.handle(event).await?;
        }
        Ok(())
    }
}

/// Event handler error types
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("Handler not initialized")]
    NotInitialized,

    #[error("Handler failed to process event: {message}")]
    ProcessingFailed { message: String },

    #[error("Handler timed out")]
    Timeout,

    #[error("Handler internal error: {source}")]
    Internal {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Macro to implement EventHandler for multiple event types
#[macro_export]
macro_rules! impl_multi_event_handler {
    ($handler:ty, $($event_type:ty),*) => {
        $(
            #[async_trait]
            impl EventHandler<$event_type> for $handler {
                type Error = HandlerError;

                async fn handle(&mut self, event: $event_type) -> Result<(), Self::Error> {
                    self.handle_event(event.into()).await
                }
            }
        )*
    };
}

// Re-export for convenience
pub use impl_multi_event_handler;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestEvent(String);

    struct TestHandler {
        handled_events: Vec<String>,
    }

    #[async_trait]
    impl EventHandler<TestEvent> for TestHandler {
        type Error = HandlerError;

        async fn handle(&mut self, event: TestEvent) -> Result<(), Self::Error> {
            self.handled_events.push(event.0);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_event_handler() {
        let mut handler = TestHandler {
            handled_events: Vec::new(),
        };

        let event = TestEvent("test".to_string());
        handler.handle(event).await.unwrap();

        assert_eq!(handler.handled_events, vec!["test".to_string()]);
    }

    #[tokio::test]
    async fn test_batch_handling() {
        let mut handler = TestHandler {
            handled_events: Vec::new(),
        };

        let events = vec![
            TestEvent("event1".to_string()),
            TestEvent("event2".to_string()),
        ];

        handler.handle_batch(events).await.unwrap();

        assert_eq!(
            handler.handled_events,
            vec!["event1".to_string(), "event2".to_string()]
        );
    }
}
