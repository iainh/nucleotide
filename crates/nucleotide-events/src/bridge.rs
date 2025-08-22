// ABOUTME: Event bridge integration for V2 event system migration
// ABOUTME: Provides translation layer between old BridgedEvent system and new domain events

use crate::completion::Event as CompletionEvent;
use crate::document::Event as DocumentEvent;
use crate::editor::Event as EditorEvent;
use crate::lsp::Event as LspEvent;
use crate::view::Event as ViewEvent;

/// Bridge events from Helix system to V2 event system
/// Provides translation layer for event integration
#[derive(Debug)]
pub enum BridgeEvent {
    Document(DocumentEvent),
    View(ViewEvent),
    Editor(EditorEvent),
    Lsp(LspEvent),
    Completion(CompletionEvent),
}

/// Convert old BridgedEvent to new V2 domain events  
/// Event conversion is handled in the Application layer to avoid circular dependencies.

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::DocumentId;

    #[test]
    fn test_bridge_event_enum() {
        // Test that BridgeEvent can be created with V2 events
        let doc_id = DocumentId::default();

        let doc_event = BridgeEvent::Document(crate::document::Event::ContentChanged {
            doc_id,
            revision: 1,
            change_summary: crate::document::ChangeType::Insert,
        });

        match doc_event {
            BridgeEvent::Document(_) => {
                // Success - can create bridge events
            }
            _ => panic!("Expected Document bridge event"),
        }
    }

    #[test]
    fn test_editor_bridge_event() {
        let editor_event = BridgeEvent::Editor(crate::editor::Event::ModeChanged {
            previous_mode: helix_view::document::Mode::Normal,
            new_mode: helix_view::document::Mode::Insert,
            context: crate::editor::ModeChangeContext::UserAction,
        });

        match editor_event {
            BridgeEvent::Editor(_) => {
                // Success - can create editor bridge events
            }
            _ => panic!("Expected Editor bridge event"),
        }
    }
}
