// ABOUTME: Test utilities module for unit testing the event system
// ABOUTME: Provides mock implementations and helpers for testing event batching and deduplication

#[cfg(test)]
#[allow(dead_code, clippy::new_without_default)]
pub mod test_support {
    use helix_view::DocumentId;
    use nucleotide_core::event_bridge::{BridgedEvent, create_bridge_channel};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    // Test-only Update enum that doesn't include GPUI entities to avoid compilation issues
    #[derive(Debug, Clone)]
    pub enum TestUpdate {
        DocumentChanged {
            doc_id: DocumentId,
        },
        DiagnosticsChanged {
            doc_id: DocumentId,
        },
        DocumentOpened {
            doc_id: DocumentId,
        },
        DocumentClosed {
            doc_id: DocumentId,
        },
        LanguageServerInitialized {
            server_id: helix_lsp::LanguageServerId,
            server_name: String,
        },
        LanguageServerExited {
            server_id: helix_lsp::LanguageServerId,
        },
    }

    /// Mock update counter for tracking how many updates are emitted
    pub struct UpdateCounter {
        count: Arc<AtomicUsize>,
    }

    impl Default for UpdateCounter {
        fn default() -> Self {
            Self::new()
        }
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
        mpsc::UnboundedReceiver<TestUpdate>,
        UpdateCounter,
    ) {
        let (tx, mut rx) = create_bridge_channel();
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let counter = UpdateCounter::new();
        let counter_clone = counter.clone_counter();

        // Spawn a task to convert BridgedEvents to TestUpdates and count them
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                counter_clone.fetch_add(1, Ordering::SeqCst);

                let update = match event {
                    BridgedEvent::DocumentChanged {
                        doc_id,
                        change_summary: _,
                        line_change: _,
                    } => TestUpdate::DocumentChanged { doc_id },
                    BridgedEvent::DiagnosticsChanged { doc_id } => {
                        TestUpdate::DiagnosticsChanged { doc_id }
                    }
                    BridgedEvent::DocumentOpened { doc_id } => {
                        TestUpdate::DocumentOpened { doc_id }
                    }
                    BridgedEvent::DocumentClosed {
                        doc_id,
                        was_modified: _,
                    } => TestUpdate::DocumentClosed { doc_id },
                    BridgedEvent::LanguageServerInitialized { server_id } => {
                        TestUpdate::LanguageServerInitialized {
                            server_id,
                            server_name: format!("LSP-{:?}", server_id),
                        }
                    }
                    BridgedEvent::LanguageServerExited { server_id } => {
                        TestUpdate::LanguageServerExited { server_id }
                    }
                    // Ignore UI picker-related bridged events in tests
                    BridgedEvent::DiagnosticsPickerRequested { .. }
                    | BridgedEvent::FilePickerRequested
                    | BridgedEvent::BufferPickerRequested => TestUpdate::DocumentChanged {
                        doc_id: helix_view::DocumentId::default(),
                    },
                    // No fallback arm; all current variants handled above
                };

                let _ = update_tx.send(update);
            }
        });

        (tx, update_rx, counter)
    }

    pub fn create_test_document_events(count: usize) -> Vec<BridgedEvent> {
        let doc_id = DocumentId::default();

        (0..count)
            .map(|_| BridgedEvent::DocumentChanged {
                doc_id,
                change_summary: nucleotide_events::v2::document::ChangeType::Insert,
                line_change: nucleotide_events::v2::document::DocumentLineChange {
                    old_lines: 0..1,
                    new_lines: 0..1,
                },
            })
            .collect()
    }

    pub fn create_test_diagnostic_events(count: usize) -> Vec<BridgedEvent> {
        let doc_id = DocumentId::default();

        (0..count)
            .map(|_| BridgedEvent::DiagnosticsChanged { doc_id })
            .collect()
    }
}
