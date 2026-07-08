// ABOUTME: Shared GPUI-to-terminal key encoding.
// ABOUTME: Keeps terminal input translation separate from legacy input dispatch.

use gpui::KeyDownEvent;

/// Encode a GPUI key event into terminal bytes using an xterm-compatible mapping.
pub fn encode_terminal_key_event(event: &KeyDownEvent) -> Vec<u8> {
    encode_terminal_key_event_with_mode(event, TerminalKeyEncodingMode::default())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalKeyEncodingMode {
    pub application_cursor: bool,
}

/// Encode a GPUI key event into terminal bytes using terminal mode-sensitive mappings.
pub fn encode_terminal_key_event_with_mode(
    event: &KeyDownEvent,
    mode: TerminalKeyEncodingMode,
) -> Vec<u8> {
    use gpui::Modifiers;

    let ks = &event.keystroke;
    let mods: &Modifiers = &ks.modifiers;

    // App shortcuts use platform/cmd; do not send to PTY.
    if mods.platform {
        return Vec::new();
    }

    if mods.control
        && let Some(b) = ctrl_byte_for(&ks.key)
    {
        if mods.alt {
            return vec![0x1B, b];
        }
        return vec![b];
    }

    // Named terminal keys must win over key_char. Some platforms/tests can attach
    // control-character text to keys like Enter or Backspace, but terminals expect
    // xterm control bytes/sequences for those keys.
    if let Some(bytes) = encode_named_terminal_key(&ks.key, mods, mode) {
        return bytes;
    }

    if let Some(s) = &ks.key_char {
        if mods.alt && !mods.control {
            let mut out = vec![0x1B];
            out.extend_from_slice(s.as_bytes());
            return out;
        }
        return s.as_bytes().to_vec();
    }

    if ks.key.len() == 1 {
        let mut ch = ks.key.as_bytes()[0] as char;
        if mods.shift {
            ch = ch.to_ascii_uppercase();
        }
        let mut out = Vec::new();
        if mods.alt {
            out.push(0x1B);
        }
        out.extend_from_slice(ch.to_string().as_bytes());
        return out;
    }

    Vec::new()
}

fn encode_named_terminal_key(
    key: &str,
    mods: &gpui::Modifiers,
    mode: TerminalKeyEncodingMode,
) -> Option<Vec<u8>> {
    match key {
        "enter" => {
            if mods.alt && !mods.control {
                Some(vec![0x1B, b'\r'])
            } else {
                Some(b"\r".to_vec())
            }
        }
        "tab" => {
            if mods.shift {
                Some(b"\x1b[Z".to_vec())
            } else if mods.alt && !mods.control {
                Some(vec![0x1B, b'\t'])
            } else {
                Some(b"\t".to_vec())
            }
        }
        "backspace" => {
            if mods.alt && !mods.control {
                Some(vec![0x1B, 0x7F])
            } else {
                Some(vec![0x7F])
            }
        }
        "escape" => Some(vec![0x1B]),
        "up" | "down" | "right" | "left" => {
            let final_byte = match key {
                "up" => b'A',
                "down" => b'B',
                "right" => b'C',
                _ => b'D',
            };
            if mods.shift || mods.alt || mods.control {
                Some(csi_with_mod_final(b"1", xterm_mod_value(mods), final_byte))
            } else if mode.application_cursor {
                Some(vec![0x1B, b'O', final_byte])
            } else {
                Some(vec![0x1B, b'[', final_byte])
            }
        }
        "home" | "end" => {
            let final_byte = if key == "home" { b'H' } else { b'F' };
            if mods.shift || mods.alt || mods.control {
                Some(csi_with_mod_final(b"1", xterm_mod_value(mods), final_byte))
            } else {
                Some(vec![0x1B, b'[', final_byte])
            }
        }
        "insert" | "delete" | "pageup" | "pagedown" => {
            let code = match key {
                "insert" => 2,
                "delete" => 3,
                "pageup" => 5,
                _ => 6,
            };
            Some(csi_with_mod_tilde(code, mods))
        }
        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            Some(encode_function_key(key, mods))
        }
        _ => None,
    }
}

fn encode_function_key(key: &str, mods: &gpui::Modifiers) -> Vec<u8> {
    let modified = mods.shift || mods.alt || mods.control;

    match key {
        "f1" | "f2" | "f3" | "f4" => {
            let final_byte = match key {
                "f1" => b'P',
                "f2" => b'Q',
                "f3" => b'R',
                _ => b'S',
            };
            if modified {
                csi_with_mod_final(b"1", xterm_mod_value(mods), final_byte)
            } else {
                vec![0x1B, b'O', final_byte]
            }
        }
        "f5" => csi_function_key_tilde(15, mods),
        "f6" => csi_function_key_tilde(17, mods),
        "f7" => csi_function_key_tilde(18, mods),
        "f8" => csi_function_key_tilde(19, mods),
        "f9" => csi_function_key_tilde(20, mods),
        "f10" => csi_function_key_tilde(21, mods),
        "f11" => csi_function_key_tilde(23, mods),
        "f12" => csi_function_key_tilde(24, mods),
        _ => Vec::new(),
    }
}

fn csi_function_key_tilde(code: u8, mods: &gpui::Modifiers) -> Vec<u8> {
    if mods.shift || mods.alt || mods.control {
        csi_with_mod_tilde(code, mods)
    } else {
        let mut out = Vec::with_capacity(6);
        out.extend_from_slice(b"\x1b[");
        out.extend_from_slice(code.to_string().as_bytes());
        out.push(b'~');
        out
    }
}

#[inline]
fn xterm_mod_value(mods: &gpui::Modifiers) -> u8 {
    let mut n: u8 = 1;
    if mods.shift {
        n += 1;
    }
    if mods.alt {
        n += 2;
    }
    if mods.control {
        n += 4;
    }
    n
}

#[inline]
fn csi_with_mod_final(prefix: &[u8], n: u8, final_byte: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(b"\x1b[");
    out.extend_from_slice(prefix);
    out.push(b';');
    out.extend_from_slice(n.to_string().as_bytes());
    out.push(final_byte);
    out
}

#[inline]
fn csi_with_mod_tilde(code: u8, mods: &gpui::Modifiers) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    out.extend_from_slice(b"\x1b[");
    out.extend_from_slice(code.to_string().as_bytes());
    if mods.shift || mods.alt || mods.control {
        out.push(b';');
        out.extend_from_slice(xterm_mod_value(mods).to_string().as_bytes());
    }
    out.push(b'~');
    out
}

#[inline]
fn ctrl_byte_for(key: &str) -> Option<u8> {
    if key.len() == 1 {
        let ch = key.as_bytes()[0].to_ascii_uppercase();
        if ch.is_ascii_uppercase() {
            return Some(ch - b'@');
        }
        if ch == b' ' {
            return Some(0x00);
        }
    }
    match key {
        "@" => Some(0x00),
        "[" => Some(0x1B),
        "\\" => Some(0x1C),
        "]" => Some(0x1D),
        "^" => Some(0x1E),
        "_" => Some(0x1F),
        "space" => Some(0x00),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(
        key: &str,
        key_char: Option<&str>,
        modifiers: gpui::Modifiers,
    ) -> gpui::KeyDownEvent {
        gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers,
                key: key.to_string(),
                key_char: key_char.map(str::to_string),
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn terminal_backspace_ignores_key_char_payload() {
        let event = key_event("backspace", Some("\u{8}"), gpui::Modifiers::none());

        assert_eq!(encode_terminal_key_event(&event), vec![0x7f]);
    }

    #[test]
    fn terminal_enter_uses_carriage_return_with_key_char_payload() {
        let event = key_event("enter", Some("\n"), gpui::Modifiers::none());

        assert_eq!(encode_terminal_key_event(&event), b"\r".to_vec());
    }

    #[test]
    fn terminal_ctrl_key_uses_control_byte_before_key_char() {
        let mut modifiers = gpui::Modifiers::none();
        modifiers.control = true;
        let event = key_event("c", Some("\u{3}"), modifiers);

        assert_eq!(encode_terminal_key_event(&event), vec![0x03]);
    }

    #[test]
    fn terminal_printable_key_char_still_passes_through() {
        let event = key_event("x", Some("x"), gpui::Modifiers::none());

        assert_eq!(encode_terminal_key_event(&event), b"x".to_vec());
    }

    #[test]
    fn terminal_shift_f5_uses_xterm_modifier_sequence() {
        let event = key_event("f5", None, gpui::Modifiers::shift());

        assert_eq!(encode_terminal_key_event(&event), b"\x1b[15;2~".to_vec());
    }

    #[test]
    fn terminal_application_cursor_mode_uses_ss3_arrows() {
        let event = key_event("up", None, gpui::Modifiers::none());
        let mode = TerminalKeyEncodingMode {
            application_cursor: true,
        };

        assert_eq!(
            encode_terminal_key_event_with_mode(&event, mode),
            b"\x1bOA".to_vec()
        );
    }
}
