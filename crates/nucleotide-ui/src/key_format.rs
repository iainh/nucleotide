// ABOUTME: Formats keystrokes for display using each platform's native shortcut conventions
// ABOUTME: macOS uses modifier symbols (⇧⌘P); Windows and Linux use title-case names (Ctrl+Shift+P)

use gpui::Modifiers;

/// The platform whose shortcut conventions a label follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPlatform {
    MacOS,
    Windows,
    Linux,
}

impl KeyPlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOS
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// Modifier labels in the order the platform lists them: ⌃⌥⇧⌘ on macOS,
/// Ctrl, Alt, Shift, then Win (Windows) or Super (Linux) elsewhere.
pub fn modifier_labels(modifiers: &Modifiers, platform: KeyPlatform) -> Vec<&'static str> {
    let mac = platform == KeyPlatform::MacOS;
    let mut labels = Vec::new();

    if modifiers.function {
        labels.push(if mac { "fn" } else { "Fn" });
    }
    if modifiers.control {
        labels.push(if mac { "⌃" } else { "Ctrl" });
    }
    if modifiers.alt {
        labels.push(if mac { "⌥" } else { "Alt" });
    }
    if modifiers.shift {
        labels.push(if mac { "⇧" } else { "Shift" });
    }
    if modifiers.platform {
        labels.push(match platform {
            KeyPlatform::MacOS => "⌘",
            KeyPlatform::Windows => "Win",
            KeyPlatform::Linux => "Super",
        });
    }

    labels
}

/// Display label for a GPUI key name, such as `enter` or `p`. Letters are
/// upper-cased, as printed on keycaps and in native menus.
pub fn key_label(key: &str, platform: KeyPlatform) -> String {
    let mac = platform == KeyPlatform::MacOS;
    let named = match key {
        "enter" => Some(if mac { "↩" } else { "Enter" }),
        "escape" => Some(if mac { "⎋" } else { "Esc" }),
        "backspace" => Some(if mac { "⌫" } else { "Backspace" }),
        "delete" => Some(if mac { "⌦" } else { "Del" }),
        "tab" => Some(if mac { "⇥" } else { "Tab" }),
        "space" => Some("Space"),
        "up" => Some(if mac { "↑" } else { "Up" }),
        "down" => Some(if mac { "↓" } else { "Down" }),
        "left" => Some(if mac { "←" } else { "Left" }),
        "right" => Some(if mac { "→" } else { "Right" }),
        "pageup" => Some(if mac { "⇞" } else { "PgUp" }),
        "pagedown" => Some(if mac { "⇟" } else { "PgDn" }),
        "home" => Some(if mac { "↖" } else { "Home" }),
        "end" => Some(if mac { "↘" } else { "End" }),
        "insert" => Some("Ins"),
        _ => None,
    };
    if let Some(named) = named {
        return named.to_string();
    }

    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// One keystroke as a native shortcut label: `⇧⌘P` on macOS, `Ctrl+Shift+P` elsewhere.
pub fn format_keystroke(modifiers: &Modifiers, key: &str, platform: KeyPlatform) -> String {
    let mut parts = modifier_labels(modifiers, platform);
    let key = key_label(key, platform);
    parts.push(&key);

    if platform == KeyPlatform::MacOS {
        parts.concat()
    } else {
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifiers(control: bool, alt: bool, shift: bool, platform: bool) -> Modifiers {
        Modifiers {
            control,
            alt,
            shift,
            platform,
            function: false,
        }
    }

    #[test]
    fn macos_uses_modifier_symbols_in_apple_order() {
        // Apple lists Control, Option, Shift, Command regardless of how the binding is written.
        assert_eq!(
            format_keystroke(
                &modifiers(false, false, true, true),
                "p",
                KeyPlatform::MacOS
            ),
            "⇧⌘P"
        );
        assert_eq!(
            format_keystroke(
                &modifiers(true, true, false, false),
                "o",
                KeyPlatform::MacOS
            ),
            "⌃⌥O"
        );
        assert_eq!(
            format_keystroke(
                &modifiers(false, false, false, true),
                "s",
                KeyPlatform::MacOS
            ),
            "⌘S"
        );
    }

    #[test]
    fn windows_and_linux_join_title_case_names_with_plus() {
        let ctrl_shift = modifiers(true, false, true, false);
        assert_eq!(
            format_keystroke(&ctrl_shift, "p", KeyPlatform::Windows),
            "Ctrl+Shift+P"
        );
        assert_eq!(
            format_keystroke(&ctrl_shift, "p", KeyPlatform::Linux),
            "Ctrl+Shift+P"
        );

        let platform_key = modifiers(false, false, false, true);
        assert_eq!(
            format_keystroke(&platform_key, "e", KeyPlatform::Windows),
            "Win+E"
        );
        assert_eq!(
            format_keystroke(&platform_key, "e", KeyPlatform::Linux),
            "Super+E"
        );
    }

    #[test]
    fn named_keys_follow_platform_conventions() {
        let none = Modifiers::default();
        assert_eq!(format_keystroke(&none, "f10", KeyPlatform::Linux), "F10");
        assert_eq!(
            format_keystroke(&none, "escape", KeyPlatform::Windows),
            "Esc"
        );
        assert_eq!(format_keystroke(&none, "escape", KeyPlatform::MacOS), "⎋");
        assert_eq!(
            format_keystroke(
                &modifiers(true, false, false, false),
                "enter",
                KeyPlatform::Linux
            ),
            "Ctrl+Enter"
        );
        assert_eq!(
            format_keystroke(
                &modifiers(false, false, false, true),
                "enter",
                KeyPlatform::MacOS
            ),
            "⌘↩"
        );
        // Punctuation keys are shown as typed.
        assert_eq!(
            format_keystroke(
                &modifiers(true, false, false, false),
                "=",
                KeyPlatform::Windows
            ),
            "Ctrl+="
        );
    }
}
