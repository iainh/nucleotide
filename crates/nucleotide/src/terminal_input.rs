// ABOUTME: App-level terminal key encoder wrapper
// ABOUTME: Keeps terminal input call sites on the shared UI encoder

use gpui::KeyDownEvent;

/// Encode a GPUI key event into terminal bytes using the shared xterm mapping.
pub fn encode_key_event(event: &KeyDownEvent) -> Vec<u8> {
    nucleotide_ui::global_input::encode_terminal_key_event(event)
}
