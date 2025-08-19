// ABOUTME: Simple LSP completion triggering utilities for Nucleotide
// ABOUTME: Provides basic integration with helix-view completion system

use nucleotide_logging::{debug, info, instrument};

/// Trigger completion for the current cursor position
#[instrument(skip(editor))]
pub fn trigger_completion(editor: &helix_view::Editor, manual: bool) {
    use helix_view::handlers::completion::CompletionEvent;

    // Safety check: ensure we have a focused view and document
    let focused_view_id = match editor.tree.focus {
        view_id if editor.tree.contains(view_id) => view_id,
        _ => {
            debug!("No focused view available for completion");
            return;
        }
    };

    let view = editor.tree.get(focused_view_id);

    let doc = match editor.documents.get(&view.doc) {
        Some(doc) => doc,
        None => {
            debug!("Document not found for completion");
            return;
        }
    };

    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));

    let event = if manual {
        CompletionEvent::ManualTrigger {
            cursor,
            doc: doc.id(),
            view: view.id,
        }
    } else {
        CompletionEvent::AutoTrigger {
            cursor,
            doc: doc.id(),
            view: view.id,
        }
    };

    info!(
        cursor = cursor,
        doc_id = ?doc.id(),
        view_id = ?view.id,
        manual = manual,
        "Triggering LSP completion - sending event to helix handlers"
    );

    // Send the event to Helix's completion handler
    editor.handlers.completions.event(event);

    info!(
        "LSP completion event sent to helix completion handler - handler should process and send to our channel"
    );
}

/// Check if LSP completion should be triggered automatically based on context
pub fn should_trigger_auto_completion(
    _editor: &helix_view::Editor,
    _character: Option<char>,
) -> bool {
    // For now, always allow auto completion
    // TODO: Add smarter logic based on:
    // - LSP trigger characters
    // - File type
    // - Cursor position
    // - Whether we're in a completion-appropriate context
    true
}
