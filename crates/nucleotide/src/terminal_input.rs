// ABOUTME: App-level terminal key encoder wrapper
// ABOUTME: Keeps terminal input call sites on the shared UI encoder

use gpui::KeyDownEvent;
use nucleotide_core::EventBus;
use nucleotide_events::v2::terminal::{Event as TerminalEvent, TerminalId};

/// Encode a GPUI key event into terminal bytes using the shared xterm mapping.
#[cfg(not(feature = "terminal-emulator-core"))]
pub fn encode_key_event(event: &KeyDownEvent) -> Vec<u8> {
    nucleotide_ui::encode_terminal_key_event(event)
}

#[cfg(feature = "terminal-emulator-core")]
pub fn encode_key_event_with_mode(
    event: &KeyDownEvent,
    mode: nucleotide_terminal::frame::TerminalInputMode,
) -> Vec<u8> {
    nucleotide_ui::encode_terminal_key_event_with_mode(
        event,
        nucleotide_ui::TerminalKeyEncodingMode {
            application_cursor: mode.application_cursor,
        },
    )
}

/// Encode a key event for a concrete terminal, using emulator mode state when available.
#[cfg(not(feature = "terminal-emulator-core"))]
pub fn encode_key_event_for_terminal(_id: TerminalId, event: &KeyDownEvent) -> Vec<u8> {
    encode_key_event(event)
}

/// Encode a key event for a concrete terminal, using emulator mode state when available.
#[cfg(feature = "terminal-emulator-core")]
pub fn encode_key_event_for_terminal(id: TerminalId, event: &KeyDownEvent) -> Vec<u8> {
    let mode = nucleotide_terminal_view::get_view_model(id)
        .and_then(|vm| vm.lock().ok().map(|guard| guard.input_mode()))
        .unwrap_or_default();
    encode_key_event_with_mode(event, mode)
}

#[cfg(feature = "terminal-emulator-core")]
fn scroll_terminal_to_bottom(id: TerminalId) {
    if let Some(vm) = nucleotide_terminal_view::get_view_model(id)
        && let Ok(mut guard) = vm.lock()
    {
        guard.scroll_to_bottom();
    }
}

/// Send terminal input bytes through the direct PTY sender when available, falling back to events.
pub fn send_terminal_input<C: gpui::AppContext>(
    core: &gpui::Entity<crate::Core>,
    id: TerminalId,
    bytes: Vec<u8>,
    cx: &mut C,
) -> bool {
    if bytes.is_empty() {
        return false;
    }

    #[cfg(feature = "terminal-emulator-core")]
    {
        scroll_terminal_to_bottom(id);
        let sent = core.read_with(cx, |app, _| {
            app.terminal_input_senders
                .lock()
                .ok()
                .and_then(|senders| {
                    senders.get(&id).map(|tx| {
                        let _ = tx.send(bytes.clone());
                    })
                })
                .is_some()
        });
        if sent {
            return true;
        }
    }

    core.update(cx, |app, _| {
        if let Some(bus) = &app.event_aggregator {
            bus.dispatch_terminal(TerminalEvent::Input { id, bytes });
            true
        } else {
            false
        }
    })
}
