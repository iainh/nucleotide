// ABOUTME: Alacritty-compatible key encoder for terminal input
// Emits xterm-style CSI sequences with proper modifier encoding where applicable.

use gpui::KeyDownEvent;

/// Encode a GPUI key event into terminal bytes using an Alacritty-compatible mapping.
/// Rules:
/// - Printable text: UTF-8; Alt prefixes ESC (Meta) unless combined with control navigation.
/// - Ctrl-letter: 0x01..0x1A; Ctrl-@/[/\\/]/^/_ supported.
/// - Navigation: xterm modifiers (1 + Shift(1) + Alt(2) + Ctrl(4)).
/// - Shift+Tab: CSI Z.
pub fn encode_key_event(event: &KeyDownEvent) -> Vec<u8> {
    use gpui::Modifiers;

    let ks = &event.keystroke;
    let mods: &Modifiers = &ks.modifiers;

    // App shortcuts use platform/cmd; do not send to PTY
    if mods.platform {
        return Vec::new();
    }

    // Printable text via IME
    if let Some(s) = &ks.key_char {
        if mods.alt && !mods.control {
            let mut out = vec![0x1B];
            out.extend_from_slice(s.as_bytes());
            return out;
        }
        return s.as_bytes().to_vec();
    }

    // Ctrl-modified keys
    if mods.control
        && let Some(b) = ctrl_byte_for(ks.key.as_str())
    {
        if mods.alt {
            return vec![0x1B, b];
        }
        return vec![b];
    }

    // Navigation and non-printables
    match ks.key.as_str() {
        // Basics
        "enter" => {
            if mods.alt && !mods.control {
                return vec![0x1B, b'\r'];
            }
            return b"\r".to_vec();
        }
        "tab" => {
            if mods.shift {
                return b"\x1b[Z".to_vec();
            }
            if mods.alt && !mods.control {
                return vec![0x1B, b'\t'];
            }
            return b"\t".to_vec();
        }
        "backspace" => {
            if mods.alt && !mods.control {
                return vec![0x1B, 0x7F];
            }
            return vec![0x7F];
        }
        "escape" => return vec![0x1B],

        // Arrows with xterm modifiers
        "up" | "down" | "right" | "left" => {
            let final_byte = match ks.key.as_str() {
                "up" => b'A',
                "down" => b'B',
                "right" => b'C',
                _ => b'D',
            };
            if mods.shift || mods.alt || mods.control {
                return csi_with_mod_final(b"1", xterm_mod_value(mods), final_byte);
            } else {
                return vec![0x1B, b'[', final_byte];
            }
        }

        // Home/End with xterm modifiers
        "home" | "end" => {
            let final_byte = if ks.key.as_str() == "home" {
                b'H'
            } else {
                b'F'
            };
            if mods.shift || mods.alt || mods.control {
                return csi_with_mod_final(b"1", xterm_mod_value(mods), final_byte);
            } else {
                return vec![0x1B, b'[', final_byte];
            }
        }

        // Insert/Delete/PageUp/PageDown with xterm modifiers
        "insert" | "delete" | "pageup" | "pagedown" => {
            let code = match ks.key.as_str() {
                "insert" => 2,
                "delete" => 3,
                "pageup" => 5,
                _ => 6,
            };
            return csi_with_mod_tilde(code, mods);
        }

        // Function keys (basic, without modifiers)
        "f1" => return b"\x1bOP".to_vec(),
        "f2" => return b"\x1bOQ".to_vec(),
        "f3" => return b"\x1bOR".to_vec(),
        "f4" => return b"\x1bOS".to_vec(),
        "f5" => return b"\x1b[15~".to_vec(),
        "f6" => return b"\x1b[17~".to_vec(),
        "f7" => return b"\x1b[18~".to_vec(),
        "f8" => return b"\x1b[19~".to_vec(),
        "f9" => return b"\x1b[20~".to_vec(),
        "f10" => return b"\x1b[21~".to_vec(),
        "f11" => return b"\x1b[23~".to_vec(),
        "f12" => return b"\x1b[24~".to_vec(),

        _ => {}
    }

    // Synthesize printable from key name if single char
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
