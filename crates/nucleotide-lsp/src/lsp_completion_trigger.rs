// ABOUTME: Simple LSP completion triggering utilities for Nucleotide
// ABOUTME: Provides basic integration with helix-view completion system

use helix_core::ropey;
use helix_core::syntax::config::LanguageConfiguration;
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

    let view = match editor.tree.try_get(focused_view_id) {
        Some(v) => v,
        None => {
            debug!("Focused view not found for completion");
            return;
        }
    };

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
    editor: &helix_view::Editor,
    character: Option<char>,
) -> bool {
    // Get the focused view and document
    let focused_view_id = match editor.tree.focus {
        view_id if editor.tree.contains(view_id) => view_id,
        _ => {
            debug!("No focused view available for auto-completion check");
            return false;
        }
    };

    let view = match editor.tree.try_get(focused_view_id) {
        Some(v) => v,
        None => {
            debug!("Focused view not found for auto-completion check");
            return false;
        }
    };
    let doc = match editor.documents.get(&view.doc) {
        Some(doc) => doc,
        None => {
            debug!("Document not found for auto-completion check");
            return false;
        }
    };

    // Get the language server for this document
    let language_server = match doc.language_servers().next() {
        Some(server) => server,
        None => {
            debug!("No language server available for auto-completion");
            return false;
        }
    };

    // Check if character is a trigger character for this language server
    let should_trigger_for_char = if let Some(ch) = character {
        is_trigger_character(language_server, ch)
    } else {
        false
    };

    // Get cursor position and text context
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    let text = doc.text().slice(..);

    // Check if we're in an appropriate context for completion
    let in_completion_context = is_in_completion_context(text, cursor, doc.language.as_deref());

    // Determine if we should trigger
    let should_trigger = should_trigger_for_char
        || (character.is_none()
            && in_completion_context
            && is_word_boundary_appropriate(text, cursor));

    debug!(
        cursor = cursor,
        character = ?character,
        should_trigger_for_char = should_trigger_for_char,
        in_completion_context = in_completion_context,
        should_trigger = should_trigger,
        "Auto-completion trigger evaluation"
    );

    should_trigger
}

/// Check if the given character is a trigger character for the language server
fn is_trigger_character(language_server: &helix_lsp::Client, character: char) -> bool {
    let capabilities = language_server.capabilities();
    if let Some(completion_provider) = &capabilities.completion_provider {
        if let Some(trigger_characters) = &completion_provider.trigger_characters {
            return trigger_characters
                .iter()
                .any(|trigger| trigger.chars().any(|ch| ch == character));
        }
    }
    false
}

/// Check if the cursor is in an appropriate context for completion
fn is_in_completion_context(
    text: ropey::RopeSlice,
    cursor: usize,
    language: Option<&LanguageConfiguration>,
) -> bool {
    use helix_core::chars::char_is_word;

    // Don't complete in the middle of a word unless at the end
    if cursor < text.len_chars() {
        let char_at_cursor = text.char(cursor);
        if char_is_word(char_at_cursor) {
            return false;
        }
    }

    // Check if we have meaningful text before cursor to potentially complete
    if cursor == 0 {
        return true; // Beginning of document is fine
    }

    let preceding_char = text.char(cursor.saturating_sub(1));

    // Skip completion in strings and comments for most languages
    if let Some(lang) = language {
        match lang.language_id.as_str() {
            "rust" | "javascript" | "typescript" | "python" | "go" | "java" | "c" | "cpp" => {
                // Basic heuristic: don't complete inside strings (very basic)
                if preceding_char == '"' || preceding_char == '\'' {
                    return false;
                }
                // Don't complete after line comments
                if cursor >= 2 {
                    let two_chars_before = text.slice((cursor - 2)..cursor).to_string();
                    if two_chars_before == "//" {
                        return false;
                    }
                }
            }
            _ => {} // Allow completion for unknown languages
        }
    }

    true
}

/// Check if we're at an appropriate word boundary for completion
fn is_word_boundary_appropriate(text: ropey::RopeSlice, cursor: usize) -> bool {
    use helix_core::chars::char_is_word;

    if cursor == 0 {
        return true;
    }

    let preceding_char = text.char(cursor.saturating_sub(1));

    // Good contexts for completion:
    // - After whitespace
    // - After punctuation like . :: ->
    // - After word characters (partial word completion)
    match preceding_char {
        '.' | ':' => true,         // Method calls, namespaces
        ' ' | '\t' | '\n' => true, // After whitespace
        '(' | '[' | '{' => true,   // After opening brackets
        ',' | ';' => true,         // After separators
        c if char_is_word(c) => {
            // Check if we have a partial identifier (at least 1 character)
            let mut word_start = cursor;
            while word_start > 0 && char_is_word(text.char(word_start - 1)) {
                word_start -= 1;
            }
            cursor - word_start >= 1 // At least one character typed
        }
        _ => false,
    }
}
