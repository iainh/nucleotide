// ABOUTME: Tests for tracing instrumentation using #[instrument] and manual spans
// ABOUTME: Verifies span creation, entry, exit, and field recording behavior

#[cfg(test)]
mod tests {
    use crate::{info, instrument};
    use tracing_mock::{expect, subscriber};

    #[instrument]
    fn simple_instrumented_function(param: u32) -> u32 {
        info!("Processing value");
        param * 2
    }

    #[instrument(skip(large_data))]
    fn instrumented_with_skip(id: u32, large_data: &[u8]) -> u32 {
        info!(data_size = large_data.len(), "Processing large data");
        id
    }

    #[test]
    fn test_simple_instrumented_function() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(expect::span().named("simple_instrumented_function"))
            .enter(expect::span().named("simple_instrumented_function"))
            .event(expect::event().with_fields(expect::msg("Processing value")))
            .exit(expect::span().named("simple_instrumented_function"))
            .drop_span(expect::span().named("simple_instrumented_function"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let result = simple_instrumented_function(42);
            assert_eq!(result, 84);
        });

        handle.assert_finished();
    }

    #[test]
    fn test_instrumented_with_skip() {
        let large_data = vec![0u8; 1000];

        let (subscriber, handle) = subscriber::mock()
            .new_span(expect::span().named("instrumented_with_skip"))
            .enter(expect::span().named("instrumented_with_skip"))
            .event(expect::event().with_fields(expect::msg("Processing large data")))
            .exit(expect::span().named("instrumented_with_skip"))
            .drop_span(expect::span().named("instrumented_with_skip"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let result = instrumented_with_skip(123, &large_data);
            assert_eq!(result, 123);
        });

        handle.assert_finished();
    }

    #[test]
    fn test_manual_span_creation() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(
                expect::span()
                    .named("manual_operation")
                    .with_fields(expect::field("task_id").with_value(&"task_123")),
            )
            .enter(expect::span().named("manual_operation"))
            .event(
                expect::event()
                    .at_level(tracing::Level::INFO)
                    .with_fields(expect::msg("Task processing started")),
            )
            .exit(expect::span().named("manual_operation"))
            .drop_span(expect::span().named("manual_operation"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(
                tracing::Level::INFO,
                "manual_operation",
                task_id = "task_123"
            );
            let _guard = span.enter();

            info!("Task processing started");
        });

        handle.assert_finished();
    }

    #[test]
    fn test_nested_spans() {
        let (subscriber, handle) = subscriber::mock()
            .new_span(
                expect::span()
                    .named("outer_operation")
                    .with_fields(expect::field("operation_id").with_value(&"op_456")),
            )
            .enter(expect::span().named("outer_operation"))
            .new_span(
                expect::span()
                    .named("inner_operation")
                    .with_fields(expect::field("step").with_value(&1)),
            )
            .enter(expect::span().named("inner_operation"))
            .event(
                expect::event()
                    .at_level(tracing::Level::INFO)
                    .with_fields(expect::msg("Inner operation completed")),
            )
            .exit(expect::span().named("inner_operation"))
            .drop_span(expect::span().named("inner_operation"))
            .event(
                expect::event()
                    .at_level(tracing::Level::INFO)
                    .with_fields(expect::msg("Outer operation completed")),
            )
            .exit(expect::span().named("outer_operation"))
            .drop_span(expect::span().named("outer_operation"))
            .only()
            .run_with_handle();

        tracing::subscriber::with_default(subscriber, || {
            let outer_span = tracing::span!(
                tracing::Level::INFO,
                "outer_operation",
                operation_id = "op_456"
            );
            let _outer_guard = outer_span.enter();

            {
                let inner_span = tracing::span!(tracing::Level::INFO, "inner_operation", step = 1);
                let _inner_guard = inner_span.enter();
                info!("Inner operation completed");
            }

            info!("Outer operation completed");
        });

        handle.assert_finished();
    }
}
