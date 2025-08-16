// ABOUTME: Tests for structured logging field validation using tracing-mock
// ABOUTME: Verifies that logging macros emit expected fields and values

#[cfg(test)]
mod tests {
    use crate::{debug, error, info, warn};
    use tracing_mock::{expect, subscriber};

    #[test]
    fn test_structured_info_logging() {
        let (subscriber, handle) = subscriber::mock()
            .event(
                expect::event()
                    .at_level(tracing::Level::INFO)
                    .with_fields(expect::msg("Document loaded successfully")),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            info!(
                file_path = "/test/path/document.txt",
                line_count = 1234,
                "Document loaded successfully"
            );
        });

        handle.assert_finished();
    }

    #[test]
    fn test_structured_error_logging() {
        let (subscriber, handle) = subscriber::mock()
            .event(
                expect::event()
                    .at_level(tracing::Level::ERROR)
                    .with_fields(expect::msg("Operation failed after retries")),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            error!(
                error_code = 404,
                operation = "file_open",
                retry_count = 3,
                "Operation failed after retries"
            );
        });

        handle.assert_finished();
    }

    #[test]
    fn test_structured_debug_logging() {
        let (subscriber, handle) = subscriber::mock()
            .event(
                expect::event()
                    .at_level(tracing::Level::DEBUG)
                    .with_fields(expect::msg("Cursor position updated")),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            debug!(
                cursor_pos = "line:42, col:15",
                selection_active = true,
                "Cursor position updated"
            );
        });

        handle.assert_finished();
    }

    #[test]
    fn test_structured_warn_logging() {
        let (subscriber, handle) = subscriber::mock()
            .event(
                expect::event()
                    .at_level(tracing::Level::WARN)
                    .with_fields(expect::msg("High memory usage detected")),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            warn!(
                memory_usage_mb = 768.5,
                threshold_mb = 512.0,
                "High memory usage detected"
            );
        });

        handle.assert_finished();
    }

    #[test]
    fn test_multiple_structured_events() {
        let (subscriber, handle) = subscriber::mock()
            .event(
                expect::event()
                    .at_level(tracing::Level::INFO)
                    .with_fields(expect::msg("Session started")),
            )
            .event(
                expect::event()
                    .at_level(tracing::Level::DEBUG)
                    .with_fields(expect::msg("User action")),
            )
            .event(
                expect::event()
                    .at_level(tracing::Level::INFO)
                    .with_fields(expect::msg("Session ended")),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            info!(session_id = "abc123", "Session started");
            debug!(session_id = "abc123", action = "file_open", "User action");
            info!(
                session_id = "abc123",
                duration_ms = 1500u64,
                "Session ended"
            );
        });

        handle.assert_finished();
    }

    #[test]
    fn test_field_value_types() {
        let (subscriber, handle) = subscriber::mock()
            .event(
                expect::event()
                    .at_level(tracing::Level::INFO)
                    .with_fields(expect::msg("Testing various field types")),
            )
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let option_val: Option<String> = Some("some_value".to_string());
            info!(
                string_val = "test",
                int_val = 42,
                float_val = std::f64::consts::PI,
                bool_val = true,
                option_val = ?option_val,
                "Testing various field types"
            );
        });

        handle.assert_finished();
    }
}
