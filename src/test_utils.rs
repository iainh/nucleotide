// ABOUTME: Test utilities module for unit testing the event system
// ABOUTME: Provides mock implementations and helpers for testing event batching and deduplication

#[cfg(test)]
pub mod test_support {
    use crate::event_bridge::{create_bridge_channel, BridgedEvent};
    use crate::Update;
    use helix_view::document::Mode;
    use helix_view::{DocumentId, ViewId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// Mock update counter for tracking how many updates are emitted
    pub struct UpdateCounter {
        count: Arc<AtomicUsize>,
    }

    impl UpdateCounter {
        pub fn new() -> Self {
            Self {
                count: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub fn increment(&self) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }

        pub fn get(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }

        pub fn clone_counter(&self) -> Arc<AtomicUsize> {
            self.count.clone()
        }
    }

    /// Create a channel with a mock receiver that counts updates
    pub fn create_counting_channel() -> (
        mpsc::UnboundedSender<BridgedEvent>,
        mpsc::UnboundedReceiver<Update>,
        UpdateCounter,
    ) {
        let (tx, mut rx) = create_bridge_channel();
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let counter = UpdateCounter::new();
        let counter_clone = counter.clone_counter();

        // Spawn a task to convert BridgedEvents to Updates and count them
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                counter_clone.fetch_add(1, Ordering::SeqCst);

                let update = match event {
                    BridgedEvent::DocumentChanged { doc_id } => Update::DocumentChanged { doc_id },
                    BridgedEvent::SelectionChanged { doc_id, view_id } => {
                        Update::SelectionChanged { doc_id, view_id }
                    }
                    BridgedEvent::ModeChanged { old_mode, new_mode } => {
                        Update::ModeChanged { old_mode, new_mode }
                    }
                    BridgedEvent::DiagnosticsChanged { doc_id } => {
                        Update::DiagnosticsChanged { doc_id }
                    }
                    BridgedEvent::DocumentOpened { doc_id } => Update::DocumentOpened { doc_id },
                    BridgedEvent::DocumentClosed { doc_id } => Update::DocumentClosed { doc_id },
                    BridgedEvent::ViewFocused { view_id } => Update::ViewFocused { view_id },
                    BridgedEvent::LanguageServerInitialized { server_id } => {
                        Update::LanguageServerInitialized { server_id }
                    }
                    BridgedEvent::LanguageServerExited { server_id } => {
                        Update::LanguageServerExited { server_id }
                    }
                    BridgedEvent::CompletionRequested {
                        doc_id,
                        view_id,
                        trigger,
                    } => Update::CompletionRequested {
                        doc_id,
                        view_id,
                        trigger,
                    },
                };

                let _ = update_tx.send(update);
            }
        });

        (tx, update_rx, counter)
    }

    /// Helper to create test events
    pub fn create_test_selection_events(count: usize) -> Vec<BridgedEvent> {
        let doc_id = DocumentId::default();
        let view_id = ViewId::default();

        (0..count)
            .map(|_| BridgedEvent::SelectionChanged { doc_id, view_id })
            .collect()
    }

    pub fn create_test_document_events(count: usize) -> Vec<BridgedEvent> {
        let doc_id = DocumentId::default();

        (0..count)
            .map(|_| BridgedEvent::DocumentChanged { doc_id })
            .collect()
    }

    pub fn create_test_diagnostic_events(count: usize) -> Vec<BridgedEvent> {
        let doc_id = DocumentId::default();

        (0..count)
            .map(|_| BridgedEvent::DiagnosticsChanged { doc_id })
            .collect()
    }
}
