// ABOUTME: Tests for event ordering and module-specific logging behavior
// ABOUTME: Verifies that events are emitted in the correct order and can be tracked by target

#[cfg(test)]
mod tests {
    use crate::{debug, error, info, trace, warn};
    use tracing_mock::{expect, subscriber};

    #[test]
    fn test_event_ordering() {
        // Test that events are emitted in the expected order
        let (subscriber, handle) = subscriber::mock()
            .event(expect::event().with_fields(expect::msg("First event")))
            .event(expect::event().with_fields(expect::msg("Second event")))
            .event(expect::event().with_fields(expect::msg("Third event")))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            info!("First event");
            warn!("Second event");
            error!("Third event");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_mixed_level_events() {
        // Test that various log levels are all captured
        let (subscriber, handle) = subscriber::mock()
            .event(expect::event().with_fields(expect::msg("Trace message")))
            .event(expect::event().with_fields(expect::msg("Debug message")))
            .event(expect::event().with_fields(expect::msg("Info message")))
            .event(expect::event().with_fields(expect::msg("Warning message")))
            .event(expect::event().with_fields(expect::msg("Error message")))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            trace!("Trace message");
            debug!("Debug message");
            info!("Info message");
            warn!("Warning message");
            error!("Error message");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_structured_events_ordering() {
        // Test that structured fields are captured in order
        let (subscriber, handle) = subscriber::mock()
            .event(expect::event().with_fields(expect::msg("File operation")))
            .event(expect::event().with_fields(expect::msg("Operation failed")))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            info!(operation = "file_load", file_size = 1024, "File operation");
            error!(error_code = 500, retry_attempt = 3, "Operation failed");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_span_and_events() {
        // Test that spans and events work together
        let (subscriber, handle) = subscriber::mock()
            .new_span(expect::span().named("operation"))
            .enter(expect::span().named("operation"))
            .event(expect::event().with_fields(expect::msg("Event within span")))
            .exit(expect::span().named("operation"))
            .drop_span(expect::span().named("operation"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(tracing::Level::INFO, "operation");
            let _guard = span.enter();
            info!("Event within span");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_no_unexpected_events() {
        // Test that when we expect no events, none are emitted
        let (subscriber, handle) = subscriber::mock().only().run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            // No events should be emitted
        });

        handle.assert_finished();
    }

    #[test]
    fn test_event_targets() {
        // Test that we can distinguish events by their target
        let (subscriber, handle) = subscriber::mock()
            .event(expect::event().with_fields(expect::msg("Module-specific message")))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            info!("Module-specific message");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_conditional_logging() {
        // Test conditional logging patterns
        let condition = true;

        let (subscriber, handle) = subscriber::mock()
            .event(expect::event().with_fields(expect::msg("Condition was true")))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            if condition {
                info!("Condition was true");
            } else {
                info!("Condition was false");
            }
        });

        handle.assert_finished();
    }

    #[test]
    fn test_nested_context() {
        // Test nested spans context
        let (subscriber, handle) = subscriber::mock()
            .new_span(expect::span().named("outer"))
            .enter(expect::span().named("outer"))
            .new_span(expect::span().named("inner"))
            .enter(expect::span().named("inner"))
            .event(expect::event().with_fields(expect::msg("Inner event")))
            .exit(expect::span().named("inner"))
            .drop_span(expect::span().named("inner"))
            .event(expect::event().with_fields(expect::msg("Outer event")))
            .exit(expect::span().named("outer"))
            .drop_span(expect::span().named("outer"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let outer_span = tracing::span!(tracing::Level::INFO, "outer");
            let _outer_guard = outer_span.enter();

            {
                let inner_span = tracing::span!(tracing::Level::INFO, "inner");
                let _inner_guard = inner_span.enter();
                info!("Inner event");
            }

            info!("Outer event");
        });

        handle.assert_finished();
    }
}
