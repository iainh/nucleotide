// ABOUTME: Simple working tests demonstrating tracing-mock integration
// ABOUTME: Basic verification that events and spans can be captured and asserted

#[cfg(test)]
mod tests {
    use crate::{info, warn};
    use tracing_mock::{expect, subscriber};

    #[test]
    fn test_basic_event_capture() {
        let (subscriber, handle) = subscriber::mock()
            .event(expect::event().with_fields(expect::msg("Test message")))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            info!("Test message");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_multiple_events() {
        let (subscriber, handle) = subscriber::mock()
            .event(expect::event().with_fields(expect::msg("First message")))
            .event(expect::event().with_fields(expect::msg("Second message")))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            info!("First message");
            warn!("Second message");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_basic_span() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(expect::span().named("test_span"))
            .enter(expect::span().named("test_span"))
            .event(expect::event().with_fields(expect::msg("Inside span")))
            .exit(expect::span().named("test_span"))
            .drop_span(expect::span().named("test_span"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(tracing::Level::INFO, "test_span");
            let _guard = span.enter();
            info!("Inside span");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_span_with_field() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(
                expect::span()
                    .named("operation")
                    .with_fields(expect::field("id").with_value(&42)),
            )
            .enter(expect::span().named("operation"))
            .exit(expect::span().named("operation"))
            .drop_span(expect::span().named("operation"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(tracing::Level::INFO, "operation", id = 42);
            let _guard = span.enter();
        });

        handle.assert_finished();
    }

    #[test]
    fn test_structured_logging() {
        let (subscriber, handle) = subscriber::mock()
            .event(
                expect::event().with_fields(
                    expect::field("user_id")
                        .with_value(&123)
                        .and(expect::field("action").with_value(&"login"))
                        .and(expect::msg("User action")),
                ),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            info!(user_id = 123, action = "login", "User action");
        });

        handle.assert_finished();
    }
}
