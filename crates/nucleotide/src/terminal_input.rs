// ABOUTME: App-level terminal key encoder wrapper
// ABOUTME: Keeps terminal input call sites on the shared UI encoder

use gpui::KeyDownEvent;

/// Encode a GPUI key event into terminal bytes using the shared xterm mapping.
#[cfg(not(feature = "terminal-emulator"))]
pub fn encode_key_event(event: &KeyDownEvent) -> Vec<u8> {
    nucleotide_ui::encode_terminal_key_event(event)
}

#[cfg(feature = "terminal-emulator")]
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
