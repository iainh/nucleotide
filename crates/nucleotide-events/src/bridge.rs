// ABOUTME: Event bridge integration for V2 event system migration
// ABOUTME: Provides translation layer between old BridgedEvent system and new domain events

use crate::document::{ChangeType, Event as DocumentEvent};
use crate::view::{Event as ViewEvent};
use crate::editor::{Event as EditorEvent, ModeChangeContext};
use crate::lsp::{Event as LspEvent};
use crate::completion::{Event as CompletionEvent};

use helix_view::document::Mode;

/// Bridge events from old BridgedEvent system to new V2 event system
/// This provides backward compatibility during migration
#[derive(Debug)]
pub enum BridgeEvent {
    Document(DocumentEvent),
    View(ViewEvent),
    Editor(EditorEvent),
    Lsp(LspEvent),
    Completion(CompletionEvent),
}

/// Convert old BridgedEvent to new V2 domain events
/// This will be implemented properly once the event migration is complete
pub fn create_mock_document_event(doc_id: helix_view::DocumentId) -> BridgeEvent {
    BridgeEvent::Document(DocumentEvent::ContentChanged {
        doc_id,
        revision: 0,
        change_summary: ChangeType::Insert, // Using existing enum variant
    })
}

/// Create a mock editor mode change event for testing
pub fn create_mock_mode_event(old_mode: Mode, new_mode: Mode) -> BridgeEvent {
    BridgeEvent::Editor(EditorEvent::ModeChanged {
        previous_mode: old_mode,
        new_mode,
        context: ModeChangeContext::UserAction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_document_event_creation() {
        let doc_id = DocumentId::default();
        let bridge_event = create_mock_document_event(doc_id);
        
        match bridge_event {
            BridgeEvent::Document(DocumentEvent::ContentChanged { doc_id: converted_id, .. }) => {
                assert_eq!(converted_id, doc_id);
            }
            _ => panic!("Expected Document::ContentChanged event"),
        }
    }

    #[test]
    fn test_mock_mode_event_creation() {
        let bridge_event = create_mock_mode_event(Mode::Normal, Mode::Insert);
        
        match bridge_event {
            BridgeEvent::Editor(EditorEvent::ModeChanged { previous_mode, new_mode, .. }) => {
                assert_eq!(previous_mode, Mode::Normal);
                assert_eq!(new_mode, Mode::Insert);
            }
            _ => panic!("Expected Editor::ModeChanged event"),
        }
    }
}