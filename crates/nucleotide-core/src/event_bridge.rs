// ABOUTME: Event bridge between Helix's event system and GPUI's Update events
// ABOUTME: Provides a channel-based system to forward Helix events to GPUI UI updates

use helix_core::{ChangeSet, Operation};
use helix_view::DocumentId;
use helix_view::ViewId;
use helix_view::document::Mode;
use nucleotide_events::ProjectLspCommand;
use nucleotide_events::v2::document::ChangeType;
use nucleotide_logging::{debug, info, instrument, warn};
use std::sync::OnceLock;
use tokio::sync::mpsc;

/// Events that can be bridged from Helix to GPUI
#[derive(Debug, Clone)]
pub enum BridgedEvent {
    DocumentChanged {
        doc_id: DocumentId,
        change_summary: ChangeType,
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
        was_modified: bool,
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
    /// Request to show diagnostics picker (mapped from Helix keybindings)
    DiagnosticsPickerRequested {
        workspace: bool,
    },
    /// Request to show file picker (mapped from Helix keybindings)
    FilePickerRequested,
    /// Request to show buffer picker (mapped from Helix keybindings)
    BufferPickerRequested,
    /// LSP server startup requested for a project
    LspServerStartupRequested {
        workspace_root: std::path::PathBuf,
        server_name: String,
        language_id: String,
    },
}

// Use the CompletionTrigger from nucleotide-types
pub use nucleotide_types::CompletionTrigger;

/// Global event bridge sender - initialized once when application starts
static EVENT_BRIDGE_SENDER: OnceLock<mpsc::UnboundedSender<BridgedEvent>> = OnceLock::new();

/// Global LSP command sender - initialized once when application starts
static LSP_COMMAND_SENDER: OnceLock<mpsc::UnboundedSender<ProjectLspCommand>> = OnceLock::new();

/// Initialize the event bridge system with a sender
#[instrument(skip(sender))]
pub fn initialize_bridge(sender: mpsc::UnboundedSender<BridgedEvent>) {
    if EVENT_BRIDGE_SENDER.set(sender).is_err() {
        warn!("Event bridge was already initialized");
    } else {
        info!("Event bridge initialized successfully");
    }
}

/// Initialize the LSP command bridge system with a sender
#[instrument(skip(sender))]
pub fn initialize_lsp_command_bridge(sender: mpsc::UnboundedSender<ProjectLspCommand>) {
    if LSP_COMMAND_SENDER.set(sender).is_err() {
        warn!("LSP command bridge was already initialized");
    } else {
        info!("LSP command bridge initialized successfully");
    }
}

/// Send a bridged event - used by Helix event hooks
pub fn send_bridged_event(event: BridgedEvent) {
    if let Some(sender) = EVENT_BRIDGE_SENDER.get() {
        debug!(event.type = ?std::mem::discriminant(&event), "Sending bridged event");
        // DIAG: Special-case diagnostics/picker for clearer tracing
        match &event {
            BridgedEvent::DiagnosticsChanged { doc_id } => {
                info!(doc_id = ?doc_id, "DIAG: Bridging DiagnosticsChanged to GPUI");
            }
            BridgedEvent::DiagnosticsPickerRequested { workspace } => {
                info!(
                    workspace = *workspace,
                    "DIAG: Bridging DiagnosticsPickerRequested to GPUI"
                );
            }
            _ => {}
        }
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

/// Register Helix event hooks that bridge to GPUI events
#[instrument]
pub fn register_event_hooks() {
    use helix_event::register_hook;
    use helix_term::events::{OnModeSwitch, PostCommand, PostInsertChar};
    use helix_view::events::{
        DiagnosticsDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen,
        LanguageServerExited, LanguageServerInitialized, SelectionDidChange,
    };

    info!("Registering Helix event hooks for event bridge");

    // Document change events
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        let doc_id = event.doc.id();
        let change_summary = analyze_change_type(event.changes);
        debug!(
            doc_id = ?doc_id,
            change_type = ?change_summary,
            "Document changed event"
        );
        send_bridged_event(BridgedEvent::DocumentChanged {
            doc_id,
            change_summary,
        });
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
            "DIAG: Helix DiagnosticsDidChange observed"
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
        let was_modified = event.doc.is_modified();
        info!(
            doc_id = ?doc_id,
            was_modified = was_modified,
            "Document closed event"
        );
        send_bridged_event(BridgedEvent::DocumentClosed {
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
        let doc_id = match event.cx.editor.tree.try_get(view_id) {
            Some(v) => v.doc,
            None => {
                // No focused view; skip triggering completion
                return Ok(());
            }
        };

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

    // Map Helix diagnostics picker commands to bridged events
    register_hook!(move |event: &mut PostCommand<'_, '_>| {
        use helix_term::keymap::MappableCommand;
        // Log every command name to aid integration/mapping
        if let MappableCommand::Static { name, .. } = event.command {
            debug!(command = *name, "PostCommand observed");
        }

        let show = match event.command {
            MappableCommand::Static { name, .. } => match *name {
                "diagnostics_picker" => Some(false),
                "workspace_diagnostics_picker" => Some(true),
                _ => None,
            },
            _ => None,
        };
        if let Some(workspace) = show {
            info!(
                workspace = workspace,
                "DIAG: Diagnostics picker command observed"
            );
            send_bridged_event(BridgedEvent::DiagnosticsPickerRequested { workspace });
        }

        // Map file/buffer picker commands
        if let MappableCommand::Static { name, .. } = event.command {
            match *name {
                "file_picker" => {
                    info!("DIAG: File picker command observed");
                    send_bridged_event(BridgedEvent::FilePickerRequested);
                }
                "buffer_picker" => {
                    info!("DIAG: Buffer picker command observed");
                    send_bridged_event(BridgedEvent::BufferPickerRequested);
                }
                _ => {}
            }
        }
        Ok(())
    });

    info!("Successfully registered all Helix event hooks for event bridge");
}

/// Send an LSP command - used by ProjectLspManager
pub fn send_lsp_command(command: ProjectLspCommand) {
    if let Some(sender) = LSP_COMMAND_SENDER.get() {
        debug!(
            command.type = ?std::mem::discriminant(&command),
            "Sending LSP command"
        );
        if let Err(e) = sender.send(command) {
            warn!(
                error = %e,
                "Failed to send LSP command"
            );
        }
    } else {
        warn!(
            command = ?command,
            "LSP command bridge not initialized, dropping command"
        );
    }
}

/// Receiver type for bridged events
pub type BridgedEventReceiver = mpsc::UnboundedReceiver<BridgedEvent>;

/// Receiver type for LSP commands
pub type LspCommandReceiver = mpsc::UnboundedReceiver<ProjectLspCommand>;

/// Create a channel pair for bridged events
pub fn create_bridge_channel() -> (mpsc::UnboundedSender<BridgedEvent>, BridgedEventReceiver) {
    mpsc::unbounded_channel()
}

/// Create a channel pair for LSP commands
pub fn create_lsp_command_channel() -> (mpsc::UnboundedSender<ProjectLspCommand>, LspCommandReceiver)
{
    mpsc::unbounded_channel()
}
