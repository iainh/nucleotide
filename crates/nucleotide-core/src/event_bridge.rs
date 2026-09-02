// ABOUTME: Private transport for Helix events consumed by the application
// ABOUTME: Forwards Helix hooks through a channel without defining application events

use helix_core::{Assoc, ChangeSet, Operation, Rope};
use helix_view::DocumentId;
use nucleotide_events::document::{ChangeType, DocumentLineChange};
use nucleotide_logging::{debug, info, instrument, trace, warn};
use std::sync::OnceLock;
use tokio::sync::mpsc;

/// Internal events forwarded from Helix hooks to the application.
#[derive(Debug, Clone)]
pub enum HelixEvent {
    DocumentChanged {
        doc_id: DocumentId,
        change_summary: ChangeType,
        line_change: DocumentLineChange,
    },
    DiagnosticsChanged {
        doc_id: DocumentId,
    },
    DocumentOpened {
        doc_id: DocumentId,
    },
    DocumentClosed {
        doc_id: DocumentId,
        was_modified: bool,
    },
    LanguageServerInitialized {
        server_id: helix_lsp::LanguageServerId,
    },
    LanguageServerExited {
        server_id: helix_lsp::LanguageServerId,
    },
}

/// Global event bridge sender - initialized once when application starts
static EVENT_BRIDGE_SENDER: OnceLock<mpsc::UnboundedSender<HelixEvent>> = OnceLock::new();

/// Initialize the event bridge system with a sender
#[instrument(skip(sender))]
pub fn initialize_bridge(sender: mpsc::UnboundedSender<HelixEvent>) {
    if EVENT_BRIDGE_SENDER.set(sender).is_err() {
        warn!("Event bridge was already initialized");
    } else {
        info!("Event bridge initialized successfully");
    }
}

/// Send a Helix event from an event hook.
pub fn send_helix_event(event: HelixEvent) {
    if let Some(sender) = EVENT_BRIDGE_SENDER.get() {
        debug!(event.type = ?std::mem::discriminant(&event), "Sending Helix event");
        if let HelixEvent::DiagnosticsChanged { doc_id } = &event {
            trace!(doc_id = ?doc_id, "DIAG: Bridging DiagnosticsChanged to GPUI");
        }
        if let Err(e) = sender.send(event) {
            warn!(
                error = %e,
                "Failed to send Helix event"
            );
        }
    } else {
        warn!(
            event = ?event,
            "Event bridge not initialized, dropping event"
        );
    }
}

/// Analyze a ChangeSet to determine the type of change that occurred
fn analyze_change_type(changes: &ChangeSet) -> ChangeType {
    let operations = changes.changes();

    if operations.is_empty() {
        return ChangeType::Bulk; // No operations, but a change occurred
    }

    let mut has_insert = false;
    let mut has_delete = false;
    let mut operation_count = 0;

    for operation in operations {
        operation_count += 1;
        match operation {
            Operation::Insert(_) => has_insert = true,
            Operation::Delete(_) => has_delete = true,
            Operation::Retain(_) => {} // Just positioning, doesn't count as change
        }
    }

    match (has_insert, has_delete, operation_count > 2) {
        (true, true, _) => ChangeType::Replace, // Both insert and delete = replace
        (true, false, false) => ChangeType::Insert, // Only insert
        (false, true, false) => ChangeType::Delete, // Only delete
        _ => ChangeType::Bulk,                  // Complex multi-operation change
    }
}

fn affected_line_range(text: &Rope, start: usize, end: usize) -> std::ops::Range<usize> {
    let text_len = text.len_chars();
    let line_count = text.len_lines().max(1);
    let start_line = text.char_to_line(start.min(text_len));
    let end_line = text.char_to_line(end.min(text_len));
    start_line..end_line.saturating_add(1).min(line_count)
}

fn document_line_change(
    old_text: &Rope,
    new_text: &Rope,
    changes: &ChangeSet,
) -> DocumentLineChange {
    let mut changed = changes.changes_iter();
    let Some((first_start, first_end, _)) = changed.next() else {
        return DocumentLineChange {
            old_lines: 0..old_text.len_lines().max(1),
            new_lines: 0..new_text.len_lines().max(1),
        };
    };

    let mut old_start = first_start;
    let mut old_end = first_end;
    for (start, end, _) in changed {
        old_start = old_start.min(start);
        old_end = old_end.max(end);
    }

    let new_start = changes.map_pos(old_start, Assoc::Before);
    let new_end = changes.map_pos(old_end, Assoc::After);
    DocumentLineChange {
        old_lines: affected_line_range(old_text, old_start, old_end),
        new_lines: affected_line_range(new_text, new_start, new_end),
    }
}

/// Register Helix event hooks that bridge to GPUI events
#[instrument]
pub fn register_event_hooks() {
    use helix_event::register_hook;
    use helix_view::doc_mut;
    use helix_view::events::{
        DiagnosticsDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen,
        DocumentFocusLost, LanguageServerExited, LanguageServerInitialized, SelectionDidChange,
    };

    info!("Registering Helix event hooks for event bridge");

    // Document change events
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if let Some(snippet) = &mut event.doc.active_snippet {
            let invalid = snippet.map(event.changes);
            if invalid {
                event.doc.active_snippet = None;
            }
        }

        let doc_id = event.doc.id();
        let change_summary = analyze_change_type(event.changes);
        let line_change = document_line_change(event.old_text, event.doc.text(), event.changes);
        debug!(
            doc_id = ?doc_id,
            change_type = ?change_summary,
            "Document changed event"
        );
        send_helix_event(HelixEvent::DocumentChanged {
            doc_id,
            change_summary,
            line_change,
        });
        Ok(())
    });

    // Selection change events
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        if let Some(snippet) = &event.doc.active_snippet
            && !snippet.is_valid(event.doc.selection(event.view))
        {
            event.doc.active_snippet = None;
        }

        Ok(())
    });

    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        let editor = &mut event.editor;
        doc_mut!(editor, &event.doc).active_snippet = None;
        Ok(())
    });

    // Diagnostics change events
    register_hook!(move |event: &mut DiagnosticsDidChange<'_>| {
        let doc_id = event.doc;
        debug!(
            doc_id = ?doc_id,
            "DIAG: Helix DiagnosticsDidChange observed"
        );
        send_helix_event(HelixEvent::DiagnosticsChanged { doc_id });
        Ok(())
    });

    // Document open events
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let doc_id = event.doc;
        info!(
            doc_id = ?doc_id,
            "Document opened event"
        );
        send_helix_event(HelixEvent::DocumentOpened { doc_id });
        Ok(())
    });

    // Document close events
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        let doc_id = event.doc.id();
        let was_modified = event.doc.is_modified();
        info!(
            doc_id = ?doc_id,
            was_modified = was_modified,
            "Document closed event"
        );
        send_helix_event(HelixEvent::DocumentClosed {
            doc_id,
            was_modified,
        });
        Ok(())
    });

    // Language server initialized events
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        let server_id = event.server_id;
        info!(
            server_id = ?server_id,
            "Language server initialized event"
        );
        send_helix_event(HelixEvent::LanguageServerInitialized { server_id });
        Ok(())
    });

    // Language server exited events
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        let server_id = event.server_id;
        info!(
            server_id = ?server_id,
            "Language server exited event"
        );
        send_helix_event(HelixEvent::LanguageServerExited { server_id });
        Ok(())
    });

    info!("Successfully registered all Helix event hooks for event bridge");
}

/// Receiver type for Helix events.
pub type HelixEventReceiver = mpsc::UnboundedReceiver<HelixEvent>;

/// Create a channel pair for Helix events.
pub fn create_bridge_channel() -> (mpsc::UnboundedSender<HelixEvent>, HelixEventReceiver) {
    mpsc::unbounded_channel()
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_core::Transaction;

    #[test]
    fn document_line_change_tracks_inserted_lines() {
        let old_text = Rope::from("one\ntwo\nthree\n");
        let transaction =
            Transaction::change(&old_text, [(4, 4, Some("inserted\n".into()))].into_iter());
        let mut new_text = old_text.clone();
        assert!(transaction.apply(&mut new_text));

        assert_eq!(
            document_line_change(&old_text, &new_text, transaction.changes()),
            DocumentLineChange {
                old_lines: 1..2,
                new_lines: 1..3,
            }
        );
    }

    #[test]
    fn document_line_change_tracks_deleted_lines() {
        let old_text = Rope::from("one\ntwo\nthree\n");
        let transaction =
            Transaction::change(&old_text, [(4, 8, None::<helix_core::Tendril>)].into_iter());
        let mut new_text = old_text.clone();
        assert!(transaction.apply(&mut new_text));

        assert_eq!(
            document_line_change(&old_text, &new_text, transaction.changes()),
            DocumentLineChange {
                old_lines: 1..3,
                new_lines: 1..2,
            }
        );
    }
}
