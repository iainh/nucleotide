// ABOUTME: Event bridge between Helix's event system and GPUI's Update events
// ABOUTME: Provides a channel-based system to forward Helix events to GPUI UI updates

use helix_view::document::Mode;
use helix_view::DocumentId;
use helix_view::ViewId;
use std::sync::OnceLock;
use tokio::sync::mpsc;

/// Events that can be bridged from Helix to GPUI
#[derive(Debug, Clone)]
pub enum BridgedEvent {
    DocumentChanged {
        doc_id: DocumentId,
    },
    SelectionChanged {
        doc_id: DocumentId,
        view_id: ViewId,
    },
    ModeChanged {
        old_mode: Mode,
        new_mode: Mode,
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
    ViewFocused {
        view_id: ViewId,
    },
    LanguageServerInitialized {
        server_id: helix_lsp::LanguageServerId,
    },
    LanguageServerExited {
        server_id: helix_lsp::LanguageServerId,
    },
    CompletionRequested {
        doc_id: DocumentId,
        view_id: ViewId,
        trigger: CompletionTrigger,
    },
}

// Use the CompletionTrigger from shared_types
pub use crate::shared_types::CompletionTrigger;

/// Global event bridge sender - initialized once when application starts
static EVENT_BRIDGE_SENDER: OnceLock<mpsc::UnboundedSender<BridgedEvent>> = OnceLock::new();

/// Initialize the event bridge system with a sender
pub fn initialize_bridge(sender: mpsc::UnboundedSender<BridgedEvent>) {
    if EVENT_BRIDGE_SENDER.set(sender).is_err() {
        log::warn!("Event bridge was already initialized");
    }
}

/// Send a bridged event - used by Helix event hooks
pub fn send_bridged_event(event: BridgedEvent) {
    if let Some(sender) = EVENT_BRIDGE_SENDER.get() {
        if let Err(e) = sender.send(event) {
            log::warn!("Failed to send bridged event: {}", e);
        }
    } else {
        log::warn!("Event bridge not initialized, dropping event: {:?}", event);
    }
}

/// Register Helix event hooks that bridge to GPUI events
pub fn register_event_hooks() {
    use helix_event::register_hook;
    use helix_term::events::*;
    use helix_view::events::*;

    // Document change events
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        send_bridged_event(BridgedEvent::DocumentChanged {
            doc_id: event.doc.id(),
        });
        Ok(())
    });

    // Selection change events
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        send_bridged_event(BridgedEvent::SelectionChanged {
            doc_id: event.doc.id(),
            view_id: event.view,
        });
        Ok(())
    });

    // Mode switch events (from helix-term)
    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        send_bridged_event(BridgedEvent::ModeChanged {
            old_mode: event.old_mode,
            new_mode: event.new_mode,
        });
        Ok(())
    });

    // Diagnostics change events
    register_hook!(move |event: &mut DiagnosticsDidChange<'_>| {
        send_bridged_event(BridgedEvent::DiagnosticsChanged { doc_id: event.doc });
        Ok(())
    });

    // Document open events
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        send_bridged_event(BridgedEvent::DocumentOpened { doc_id: event.doc });
        Ok(())
    });

    // Document close events
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        send_bridged_event(BridgedEvent::DocumentClosed {
            doc_id: event.doc.id(),
        });
        Ok(())
    });

    // Language server initialized events
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        send_bridged_event(BridgedEvent::LanguageServerInitialized {
            server_id: event.server_id,
        });
        Ok(())
    });

    // Language server exited events
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        send_bridged_event(BridgedEvent::LanguageServerExited {
            server_id: event.server_id,
        });
        Ok(())
    });

    // Post insert character events - trigger completion
    register_hook!(move |event: &mut PostInsertChar<'_, '_>| {
        // Get the current view and document
        let view_id = event.cx.editor.tree.focus;
        let doc_id = event.cx.editor.tree.get(view_id).doc;

        send_bridged_event(BridgedEvent::CompletionRequested {
            doc_id,
            view_id,
            trigger: CompletionTrigger::Character(event.c),
        });
        Ok(())
    });

    log::info!("Registered Helix event hooks for event bridge");
}

/// Receiver type for bridged events
pub type BridgedEventReceiver = mpsc::UnboundedReceiver<BridgedEvent>;

/// Create a channel pair for bridged events
pub fn create_bridge_channel() -> (mpsc::UnboundedSender<BridgedEvent>, BridgedEventReceiver) {
    mpsc::unbounded_channel()
}
