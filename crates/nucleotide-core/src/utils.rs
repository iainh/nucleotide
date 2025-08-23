// ABOUTME: Core utility functions for runtime detection and key translation
// ABOUTME: Bridge utilities between GPUI and Helix systems

use gpui::Keystroke;
use helix_view::input::KeyEvent;
use helix_view::keyboard::{KeyCode, KeyModifiers};
use nucleotide_logging::{debug, warn};

/// Detect the runtime directory when running from a macOS bundle
#[cfg(target_os = "macos")]
pub fn detect_bundle_runtime() -> Option<std::path::PathBuf> {
    if let Ok(mut exe) = std::env::current_exe() {
        exe.pop(); // nucl or nucleotide-bin
        exe.pop(); // MacOS
        exe.push("Resources");
        exe.push("runtime");
        if exe.is_dir() {
            return Some(exe);
        }
    }
    None
}

/// Map of Shift + number key combinations to their symbols
const SHIFT_NUMBER_MAP: &[(char, char)] = &[
    ('1', '!'),
    ('2', '@'),
    ('3', '#'),
    ('4', '$'),
    ('5', '%'),
    ('6', '^'),
    ('7', '&'),
    ('8', '*'),
    ('9', '('),
    ('0', ')'),
    ('-', '_'),
    ('=', '+'),
    ('[', '{'),
    (']', '}'),
    ('\\', '|'),
    (';', ':'),
    ('\'', '"'),
    (',', '<'),
    ('.', '>'),
    ('/', '?'),
    ('`', '~'),
];

/// Translate a GPUI keystroke to a Helix key event
pub fn translate_key(ks: &Keystroke) -> KeyEvent {
    let mut modifiers = KeyModifiers::NONE;

    // Handle all GPUI modifiers
    if ks.modifiers.alt {
        modifiers |= KeyModifiers::ALT;
    }
    if ks.modifiers.control {
        modifiers |= KeyModifiers::CONTROL;
    }
    if ks.modifiers.shift {
        modifiers |= KeyModifiers::SHIFT;
    }
    if ks.modifiers.platform {
        // Platform modifier: Cmd on macOS, Super on Linux/Windows
        modifiers |= KeyModifiers::SUPER;
    }
    if ks.modifiers.function {
        // Function modifier key (if supported by Helix)
        debug!(key = %ks.key, "Function modifier detected but not mapped to Helix equivalent");
    }

    let key = &ks.key;
    let code = match key.as_str() {
        // Basic editing keys
        "backspace" => KeyCode::Backspace,
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "delete" => KeyCode::Delete,

        // Navigation keys
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,

        // Additional navigation/editing keys
        "insert" => KeyCode::Insert,

        // Function keys F1-F12
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),

        // Extended function keys (if supported by terminal)
        "f13" => KeyCode::F(13),
        "f14" => KeyCode::F(14),
        "f15" => KeyCode::F(15),
        "f16" => KeyCode::F(16),
        "f17" => KeyCode::F(17),
        "f18" => KeyCode::F(18),
        "f19" => KeyCode::F(19),
        "f20" => KeyCode::F(20),
        "f21" => KeyCode::F(21),
        "f22" => KeyCode::F(22),
        "f23" => KeyCode::F(23),
        "f24" => KeyCode::F(24),

        // Handle single character keys
        _ => translate_character_key(key, ks.modifiers.shift),
    };

    KeyEvent { code, modifiers }
}

/// Translate a single character key, handling shift modifiers appropriately
fn translate_character_key(key: &str, shift_pressed: bool) -> KeyCode {
    // Optimize: use iterator instead of collecting into Vec
    let mut chars = key.chars();
    let first_char = chars.next();

    // Ensure it's exactly one character
    if chars.next().is_some() {
        // Multi-character string - log for debugging and return Null
        warn!(key = %key, "Unmapped multi-character key");
        return KeyCode::Null;
    }

    match first_char {
        Some(ch) => {
            if shift_pressed {
                // Handle shift combinations
                if ch.is_ascii_alphabetic() && ch.is_lowercase() {
                    // Shift + letter = uppercase letter
                    KeyCode::Char(ch.to_ascii_uppercase())
                } else {
                    // Check for shift + symbol combinations
                    match SHIFT_NUMBER_MAP.iter().find(|&&(base, _)| base == ch) {
                        Some(&(_, shifted)) => KeyCode::Char(shifted),
                        None => KeyCode::Char(ch), // Keep original character
                    }
                }
            } else {
                KeyCode::Char(ch)
            }
        }
        None => {
            warn!(key = %key, "Empty key string");
            KeyCode::Null
        }
    }
}

/// Handle events by looking them up in `self.keymaps`. Returns None
/// if event was handled (a command was executed or a subkeymap was
/// activated). Only KeymapResult::{NotFound, Cancelled} is returned
/// otherwise.
#[allow(unused)]
pub fn handle_key_result(
    mode: helix_view::document::Mode,
    cxt: &mut helix_term::commands::Context,
    key_result: helix_term::keymap::KeymapResult,
) -> Option<helix_term::keymap::KeymapResult> {
    use helix_term::events::{OnModeSwitch, PostCommand};
    use helix_term::keymap::KeymapResult;
    use helix_view::document::Mode;

    let mut last_mode = mode;

    let mut execute_command = |command: &helix_term::commands::MappableCommand| {
        command.execute(cxt);
        helix_event::dispatch(PostCommand { command, cx: cxt });

        let current_mode = cxt.editor.mode();
        if current_mode != last_mode {
            helix_event::dispatch(OnModeSwitch {
                old_mode: last_mode,
                new_mode: current_mode,
                cx: cxt,
            });

            // HAXX: if we just entered insert mode from normal, clear key buf
            // and record the command that got us into this mode.
            if current_mode == Mode::Insert {
                // how we entered insert mode is important, and we should track that so
                // we can repeat the side effect.
            }
        }

        last_mode = current_mode;
    };

    match &key_result {
        KeymapResult::Matched(command) => {
            execute_command(command);
        }
        KeymapResult::Pending(node) => cxt.editor.autoinfo = Some(node.infobox()),
        KeymapResult::MatchedSequence(commands) => {
            for command in commands {
                execute_command(command);
            }
        }
        KeymapResult::NotFound | KeymapResult::Cancelled(_) => return Some(key_result),
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    /// Create a test keystroke with given key and modifiers
    fn create_keystroke(key: &str, modifiers: Modifiers) -> Keystroke {
        gpui::Keystroke {
            key: key.into(),
            modifiers,
            key_char: None,
        }
    }

    #[test]
    fn test_basic_character_translation() {
        let keystroke = create_keystroke("a", Modifiers::default());
        let key_event = translate_key(&keystroke);

        assert_eq!(key_event.code, KeyCode::Char('a'));
        assert_eq!(key_event.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn test_uppercase_with_shift() {
        let keystroke = create_keystroke(
            "a",
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        let key_event = translate_key(&keystroke);

        assert_eq!(key_event.code, KeyCode::Char('A'));
        assert!(key_event.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn test_shift_number_symbols() {
        let test_cases = [
            ("1", '!'),
            ("2", '@'),
            ("3", '#'),
            ("4", '$'),
            ("5", '%'),
            ("9", '('),
            ("0", ')'),
        ];

        for (input, expected) in test_cases {
            let keystroke = create_keystroke(
                input,
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
            );
            let key_event = translate_key(&keystroke);

            assert_eq!(
                key_event.code,
                KeyCode::Char(expected),
                "Failed for input '{}', expected '{}', got {:?}",
                input,
                expected,
                key_event.code
            );
        }
    }

    #[test]
    fn test_platform_modifier() {
        let keystroke = create_keystroke(
            "s",
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );
        let key_event = translate_key(&keystroke);

        assert_eq!(key_event.code, KeyCode::Char('s'));
        assert!(key_event.modifiers.contains(KeyModifiers::SUPER));
    }

    #[test]
    fn test_multiple_modifiers() {
        let keystroke = create_keystroke(
            "a",
            Modifiers {
                control: true,
                alt: true,
                shift: true,
                ..Default::default()
            },
        );
        let key_event = translate_key(&keystroke);

        assert_eq!(key_event.code, KeyCode::Char('A')); // Should be uppercase due to shift
        assert!(key_event.modifiers.contains(KeyModifiers::CONTROL));
        assert!(key_event.modifiers.contains(KeyModifiers::ALT));
        assert!(key_event.modifiers.contains(KeyModifiers::SHIFT));
    }

    #[test]
    fn test_special_keys() {
        let test_cases = [
            ("escape", KeyCode::Esc),
            ("enter", KeyCode::Enter),
            ("tab", KeyCode::Tab),
            ("backspace", KeyCode::Backspace),
            ("delete", KeyCode::Delete),
            ("left", KeyCode::Left),
            ("right", KeyCode::Right),
            ("up", KeyCode::Up),
            ("down", KeyCode::Down),
            ("home", KeyCode::Home),
            ("end", KeyCode::End),
            ("pageup", KeyCode::PageUp),
            ("pagedown", KeyCode::PageDown),
            ("insert", KeyCode::Insert),
        ];

        for (input, expected) in test_cases {
            let keystroke = create_keystroke(input, Modifiers::default());
            let key_event = translate_key(&keystroke);

            assert_eq!(
                key_event.code, expected,
                "Failed for input '{}', expected {:?}, got {:?}",
                input, expected, key_event.code
            );
        }
    }

    #[test]
    fn test_function_keys() {
        for i in 1..=24 {
            let key_str = format!("f{}", i);
            let keystroke = create_keystroke(&key_str, Modifiers::default());
            let key_event = translate_key(&keystroke);

            assert_eq!(
                key_event.code,
                KeyCode::F(i as u8),
                "Failed for function key F{}",
                i
            );
        }
    }

    #[test]
    fn test_space_key() {
        let keystroke = create_keystroke("space", Modifiers::default());
        let key_event = translate_key(&keystroke);

        assert_eq!(key_event.code, KeyCode::Char(' '));
    }

    #[test]
    fn test_shift_symbol_mappings() {
        let test_cases = [
            ("-", '_'),
            ("=", '+'),
            ("[", '{'),
            ("]", '}'),
            ("\\", '|'),
            (";", ':'),
            ("'", '"'),
            (",", '<'),
            (".", '>'),
            ("/", '?'),
            ("`", '~'),
        ];

        for (base, shifted) in test_cases {
            let keystroke = create_keystroke(
                base,
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
            );
            let key_event = translate_key(&keystroke);

            assert_eq!(
                key_event.code,
                KeyCode::Char(shifted),
                "Failed shift mapping: {} -> {}, got {:?}",
                base,
                shifted,
                key_event.code
            );
        }
    }
}
