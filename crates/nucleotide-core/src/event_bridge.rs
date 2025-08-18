// ABOUTME: Event bridge between Helix's event system and GPUI's Update events
// ABOUTME: Provides a channel-based system to forward Helix events to GPUI UI updates

use helix_view::DocumentId;
use helix_view::ViewId;
use helix_view::document::Mode;
use nucleotide_logging::{debug, info, instrument, warn};
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

// Use the CompletionTrigger from nucleotide-types
pub use nucleotide_types::CompletionTrigger;

/// Global event bridge sender - initialized once when application starts
static EVENT_BRIDGE_SENDER: OnceLock<mpsc::UnboundedSender<BridgedEvent>> = OnceLock::new();

/// Initialize the event bridge system with a sender
#[instrument(skip(sender))]
pub fn initialize_bridge(sender: mpsc::UnboundedSender<BridgedEvent>) {
    if EVENT_BRIDGE_SENDER.set(sender).is_err() {
        warn!("Event bridge was already initialized");
    } else {
        info!("Event bridge initialized successfully");
    }
}

/// Send a bridged event - used by Helix event hooks
pub fn send_bridged_event(event: BridgedEvent) {
    if let Some(sender) = EVENT_BRIDGE_SENDER.get() {
        debug!(
            event.type = ?std::mem::discriminant(&event),
            "Sending bridged event"
        );
        if let Err(e) = sender.send(event) {
            warn!(
                error = %e,
                "Failed to send bridged event"
            );
        }
    } else {
        warn!(
            event = ?event,
            "Event bridge not initialized, dropping event"
        );
    }
}

/// Register Helix event hooks that bridge to GPUI events
#[instrument]
pub fn register_event_hooks() {
    use helix_event::register_hook;
    use helix_term::events::{OnModeSwitch, PostInsertChar};
    use helix_view::events::{
        DiagnosticsDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen,
        LanguageServerExited, LanguageServerInitialized, SelectionDidChange,
    };

    info!("Registering Helix event hooks for event bridge");

    // Document change events
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        let doc_id = event.doc.id();
        debug!(
            doc_id = ?doc_id,
            "Document changed event"
        );
        send_bridged_event(BridgedEvent::DocumentChanged { doc_id });
        Ok(())
    });

    // Selection change events
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        let doc_id = event.doc.id();
        let view_id = event.view;
        debug!(
            doc_id = ?doc_id,
            view_id = ?view_id,
            "Selection changed event"
        );
        send_bridged_event(BridgedEvent::SelectionChanged { doc_id, view_id });
        Ok(())
    });

    // Mode switch events (from helix-term)
    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        let old_mode = event.old_mode;
        let new_mode = event.new_mode;
        info!(
            old_mode = ?old_mode,
            new_mode = ?new_mode,
            "Mode switch event"
        );
        send_bridged_event(BridgedEvent::ModeChanged { old_mode, new_mode });
        Ok(())
    });

    // Diagnostics change events
    register_hook!(move |event: &mut DiagnosticsDidChange<'_>| {
        let doc_id = event.doc;
        debug!(
            doc_id = ?doc_id,
            "Diagnostics changed event"
        );
        send_bridged_event(BridgedEvent::DiagnosticsChanged { doc_id });
        Ok(())
    });

    // Document open events
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let doc_id = event.doc;
        info!(
            doc_id = ?doc_id,
            "Document opened event"
        );
        send_bridged_event(BridgedEvent::DocumentOpened { doc_id });
        Ok(())
    });

    // Document close events
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        let doc_id = event.doc.id();
        info!(
            doc_id = ?doc_id,
            "Document closed event"
        );
        send_bridged_event(BridgedEvent::DocumentClosed { doc_id });
        Ok(())
    });

    // Language server initialized events
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        let server_id = event.server_id;
        info!(
            server_id = ?server_id,
            "Language server initialized event"
        );
        send_bridged_event(BridgedEvent::LanguageServerInitialized { server_id });
        Ok(())
    });

    // Language server exited events
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        let server_id = event.server_id;
        info!(
            server_id = ?server_id,
            "Language server exited event"
        );
        send_bridged_event(BridgedEvent::LanguageServerExited { server_id });
        Ok(())
    });

    // Post insert character events - trigger completion
    register_hook!(move |event: &mut PostInsertChar<'_, '_>| {
        // Get the current view and document
        let view_id = event.cx.editor.tree.focus;
        let doc_id = event.cx.editor.tree.get(view_id).doc;

        debug!(
            doc_id = ?doc_id,
            view_id = ?view_id,
            character = %event.c,
            "Completion requested after character insert"
        );

        send_bridged_event(BridgedEvent::CompletionRequested {
            doc_id,
            view_id,
            trigger: CompletionTrigger::Character(event.c),
        });
        Ok(())
    });

    info!("Successfully registered all Helix event hooks for event bridge");
}

/// Receiver type for bridged events
pub type BridgedEventReceiver = mpsc::UnboundedReceiver<BridgedEvent>;

/// Create a channel pair for bridged events
pub fn create_bridge_channel() -> (mpsc::UnboundedSender<BridgedEvent>, BridgedEventReceiver) {
    mpsc::unbounded_channel()
}
