// ABOUTME: Keyboard handling utilities for nucleotide-ui components
// ABOUTME: Provides keyboard shortcut management, key mapping, and input helpers

use gpui::{KeyDownEvent, SharedString};
use std::collections::HashMap;

/// Keyboard shortcut definition
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyboardShortcut {
    pub key: String,
    pub modifiers: ModifierSet,
}

/// Set of keyboard modifiers
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModifierSet {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool, // Cmd on macOS, Ctrl on other platforms
}

impl ModifierSet {
    /// Create an empty modifier set
    pub fn none() -> Self {
        Self {
            control: false,
            alt: false,
            shift: false,
            platform: false,
        }
    }

    /// Create a modifier set with only Control
    pub fn control() -> Self {
        Self {
            control: true,
            alt: false,
            shift: false,
            platform: false,
        }
    }

    /// Create a modifier set with only Alt
    pub fn alt() -> Self {
        Self {
            control: false,
            alt: true,
            shift: false,
            platform: false,
        }
    }

    /// Create a modifier set with only Shift
    pub fn shift() -> Self {
        Self {
            control: false,
            alt: false,
            shift: true,
            platform: false,
        }
    }

    /// Create a modifier set with only platform modifier (Cmd/Ctrl)
    pub fn platform() -> Self {
        Self {
            control: false,
            alt: false,
            shift: false,
            platform: true,
        }
    }

    /// Chain modifier combinations
    pub fn with_control(mut self) -> Self {
        self.control = true;
        self
    }

    pub fn with_alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub fn with_platform(mut self) -> Self {
        self.platform = true;
        self
    }
}

impl From<&gpui::Modifiers> for ModifierSet {
    fn from(modifiers: &gpui::Modifiers) -> Self {
        Self {
            control: modifiers.control,
            alt: modifiers.alt,
            shift: modifiers.shift,
            platform: modifiers.platform,
        }
    }
}

impl KeyboardShortcut {
    /// Create a new keyboard shortcut
    pub fn new(key: impl Into<String>, modifiers: ModifierSet) -> Self {
        Self {
            key: key.into(),
            modifiers,
        }
    }

    /// Create a shortcut with no modifiers
    pub fn key(key: impl Into<String>) -> Self {
        Self::new(key, ModifierSet::none())
    }

    /// Create a shortcut with Control modifier
    pub fn ctrl(key: impl Into<String>) -> Self {
        Self::new(key, ModifierSet::control())
    }

    /// Create a shortcut with Alt modifier
    pub fn alt(key: impl Into<String>) -> Self {
        Self::new(key, ModifierSet::alt())
    }

    /// Create a shortcut with Shift modifier
    pub fn shift(key: impl Into<String>) -> Self {
        Self::new(key, ModifierSet::shift())
    }

    /// Create a shortcut with platform modifier (Cmd on macOS, Ctrl elsewhere)
    pub fn cmd(key: impl Into<String>) -> Self {
        Self::new(key, ModifierSet::platform())
    }

    /// Check if this shortcut matches a keyboard event
    pub fn matches(&self, event: &KeyDownEvent) -> bool {
        let event_key = event.keystroke.key.as_str();
        let event_modifiers = ModifierSet::from(&event.keystroke.modifiers);

        self.key == event_key && self.modifiers == event_modifiers
    }

    /// Get a human-readable string representation
    pub fn to_string(&self) -> String {
        let mut parts = Vec::new();

        if self.modifiers.platform {
            parts.push(if cfg!(target_os = "macos") {
                "⌘"
            } else {
                "Ctrl"
            });
        }
        if self.modifiers.control {
            parts.push("Ctrl");
        }
        if self.modifiers.alt {
            parts.push(if cfg!(target_os = "macos") {
                "⌥"
            } else {
                "Alt"
            });
        }
        if self.modifiers.shift {
            parts.push("⇧");
        }

        parts.push(&self.key);
        parts.join("+")
    }
}

/// Keyboard shortcut registry for managing application shortcuts
#[derive(Debug, Default)]
pub struct ShortcutRegistry {
    shortcuts: HashMap<KeyboardShortcut, SharedString>,
    action_shortcuts: HashMap<SharedString, KeyboardShortcut>,
}

impl ShortcutRegistry {
    /// Create a new shortcut registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a keyboard shortcut for an action
    pub fn register(&mut self, shortcut: KeyboardShortcut, action: impl Into<SharedString>) {
        let action = action.into();

        // Remove any existing shortcut for this action
        if let Some(old_shortcut) = self.action_shortcuts.get(&action) {
            self.shortcuts.remove(old_shortcut);
        }

        self.shortcuts.insert(shortcut.clone(), action.clone());
        self.action_shortcuts.insert(action, shortcut);
    }

    /// Unregister a shortcut
    pub fn unregister(&mut self, action: &str) {
        if let Some(shortcut) = self.action_shortcuts.remove(action) {
            self.shortcuts.remove(&shortcut);
        }
    }

    /// Find the action for a keyboard event
    pub fn find_action(&self, event: &KeyDownEvent) -> Option<&SharedString> {
        self.shortcuts
            .iter()
            .find(|(shortcut, _)| shortcut.matches(event))
            .map(|(_, action)| action)
    }

    /// Get the shortcut for an action
    pub fn get_shortcut(&self, action: &str) -> Option<&KeyboardShortcut> {
        self.action_shortcuts.get(action)
    }

    /// Get all registered shortcuts
    pub fn all_shortcuts(&self) -> Vec<(&KeyboardShortcut, &SharedString)> {
        self.shortcuts.iter().collect()
    }

    /// Load default shortcuts
    pub fn load_defaults(&mut self) {
        // Common shortcuts
        self.register(KeyboardShortcut::cmd("n"), "new");
        self.register(KeyboardShortcut::cmd("o"), "open");
        self.register(KeyboardShortcut::cmd("s"), "save");
        self.register(
            KeyboardShortcut::new("s", ModifierSet::platform().with_shift()),
            "save_as",
        );
        self.register(KeyboardShortcut::cmd("z"), "undo");
        self.register(
            KeyboardShortcut::new("z", ModifierSet::platform().with_shift()),
            "redo",
        );
        self.register(KeyboardShortcut::cmd("x"), "cut");
        self.register(KeyboardShortcut::cmd("c"), "copy");
        self.register(KeyboardShortcut::cmd("v"), "paste");
        self.register(KeyboardShortcut::cmd("a"), "select_all");
        self.register(KeyboardShortcut::cmd("f"), "find");
        self.register(
            KeyboardShortcut::new("f", ModifierSet::platform().with_shift()),
            "find_in_files",
        );
        self.register(KeyboardShortcut::cmd("w"), "close");
        self.register(KeyboardShortcut::cmd("q"), "quit");

        // Navigation shortcuts
        self.register(KeyboardShortcut::key("escape"), "escape");
        self.register(KeyboardShortcut::key("enter"), "activate");
        self.register(KeyboardShortcut::key(" "), "toggle");
        self.register(KeyboardShortcut::key("tab"), "next_field");
        self.register(KeyboardShortcut::shift("tab"), "previous_field");

        // Arrow navigation
        self.register(KeyboardShortcut::key("up"), "navigate_up");
        self.register(KeyboardShortcut::key("down"), "navigate_down");
        self.register(KeyboardShortcut::key("left"), "navigate_left");
        self.register(KeyboardShortcut::key("right"), "navigate_right");
        self.register(KeyboardShortcut::key("home"), "navigate_home");
        self.register(KeyboardShortcut::key("end"), "navigate_end");
        self.register(KeyboardShortcut::key("pageup"), "navigate_page_up");
        self.register(KeyboardShortcut::key("pagedown"), "navigate_page_down");
    }
}

/// Key mapping utilities
pub struct KeyMappings;

impl KeyMappings {
    /// Normalize key names to standard form
    pub fn normalize_key(key: &str) -> String {
        match key.to_lowercase().as_str() {
            "arrowup" => "up".to_string(),
            "arrowdown" => "down".to_string(),
            "arrowleft" => "left".to_string(),
            "arrowright" => "right".to_string(),
            "spacebar" | " " => "space".to_string(),
            "esc" => "escape".to_string(),
            "return" => "enter".to_string(),
            "del" => "delete".to_string(),
            "backspace" => "backspace".to_string(),
            _ => key.to_lowercase(),
        }
    }

    /// Check if a key is a navigation key
    pub fn is_navigation_key(key: &str) -> bool {
        matches!(
            key.to_lowercase().as_str(),
            "up" | "down"
                | "left"
                | "right"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "arrowup"
                | "arrowdown"
                | "arrowleft"
                | "arrowright"
        )
    }

    /// Check if a key is a modifier key
    pub fn is_modifier_key(key: &str) -> bool {
        matches!(
            key.to_lowercase().as_str(),
            "control" | "ctrl" | "alt" | "shift" | "meta" | "cmd" | "command" | "option"
        )
    }

    /// Check if a key is printable (produces text)
    pub fn is_printable_key(key: &str) -> bool {
        key.len() == 1 && !key.chars().next().unwrap().is_control()
    }

    /// Get the display name for a key
    pub fn display_name(key: &str) -> String {
        match key.to_lowercase().as_str() {
            "up" | "arrowup" => "↑".to_string(),
            "down" | "arrowdown" => "↓".to_string(),
            "left" | "arrowleft" => "←".to_string(),
            "right" | "arrowright" => "→".to_string(),
            "space" | " " => "Space".to_string(),
            "enter" | "return" => "⏎".to_string(),
            "escape" | "esc" => "Esc".to_string(),
            "backspace" => "⌫".to_string(),
            "delete" | "del" => "⌦".to_string(),
            "tab" => "⇥".to_string(),
            "pageup" => "Page ↑".to_string(),
            "pagedown" => "Page ↓".to_string(),
            "home" => "Home".to_string(),
            "end" => "End".to_string(),
            _ => key.to_string(),
        }
    }
}

/// Input validation utilities
pub struct InputValidation;

impl InputValidation {
    /// Check if input should be handled by a text field
    pub fn is_text_input(event: &KeyDownEvent) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;

        // Don't handle if there are significant modifiers (except shift for uppercase)
        if modifiers.control || modifiers.alt || modifiers.platform {
            return false;
        }

        // Handle printable characters and basic editing keys
        KeyMappings::is_printable_key(key)
            || matches!(key, "backspace" | "delete" | "enter" | "tab")
    }

    /// Check if an event represents a shortcut
    pub fn is_shortcut(event: &KeyDownEvent) -> bool {
        let modifiers = &event.keystroke.modifiers;
        modifiers.control || modifiers.alt || modifiers.platform
    }

    /// Filter out system-handled keys
    pub fn should_handle_key(event: &KeyDownEvent) -> bool {
        let key = event.keystroke.key.as_str();

        // Don't handle system function keys
        if key.starts_with('F') && key.len() <= 3 && key[1..].parse::<u8>().is_ok() {
            return false; // Function keys like F1, F2, etc.
        }

        // Handle most other keys
        !matches!(
            key.to_lowercase().as_str(),
            "capslock" | "numlock" | "scrolllock" | "contextmenu" | "help" | "printscreen"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};

    fn create_key_event(key: &str, modifiers: gpui::Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                key: key.into(),
                modifiers,
                key_char: None,
            },
            is_held: false,
        }
    }

    #[test]
    fn test_keyboard_shortcut_creation() {
        let shortcut = KeyboardShortcut::cmd("s");
        assert_eq!(shortcut.key, "s");
        assert!(shortcut.modifiers.platform);
        assert!(!shortcut.modifiers.control);

        let complex_shortcut = KeyboardShortcut::new("z", ModifierSet::platform().with_shift());
        assert!(complex_shortcut.modifiers.platform);
        assert!(complex_shortcut.modifiers.shift);
        assert!(!complex_shortcut.modifiers.control);
    }

    #[test]
    fn test_shortcut_matching() {
        let shortcut = KeyboardShortcut::cmd("s");

        let matching_event = create_key_event(
            "s",
            Modifiers {
                platform: true,
                control: false,
                alt: false,
                shift: false,
                function: false,
            },
        );

        let non_matching_event = create_key_event(
            "s",
            Modifiers {
                platform: false,
                control: true,
                alt: false,
                shift: false,
                function: false,
            },
        );

        assert!(shortcut.matches(&matching_event));
        assert!(!shortcut.matches(&non_matching_event));
    }

    #[test]
    fn test_shortcut_registry() {
        let mut registry = ShortcutRegistry::new();

        let save_shortcut = KeyboardShortcut::cmd("s");
        registry.register(save_shortcut.clone(), "save");

        let event = create_key_event(
            "s",
            Modifiers {
                platform: true,
                control: false,
                alt: false,
                shift: false,
                function: false,
            },
        );

        let action = registry.find_action(&event);
        assert_eq!(action, Some(&"save".into()));

        let retrieved_shortcut = registry.get_shortcut("save");
        assert_eq!(retrieved_shortcut, Some(&save_shortcut));
    }

    #[test]
    fn test_key_mappings() {
        assert_eq!(KeyMappings::normalize_key("ArrowUp"), "up");
        assert_eq!(KeyMappings::normalize_key("Spacebar"), "space");
        assert_eq!(KeyMappings::normalize_key("Esc"), "escape");

        assert!(KeyMappings::is_navigation_key("up"));
        assert!(KeyMappings::is_navigation_key("ArrowDown"));
        assert!(!KeyMappings::is_navigation_key("a"));

        assert!(KeyMappings::is_modifier_key("control"));
        assert!(KeyMappings::is_modifier_key("cmd"));
        assert!(!KeyMappings::is_modifier_key("a"));

        assert!(KeyMappings::is_printable_key("a"));
        assert!(KeyMappings::is_printable_key("1"));
        assert!(!KeyMappings::is_printable_key("escape"));
        assert!(!KeyMappings::is_printable_key("tab"));
    }

    #[test]
    fn test_display_names() {
        assert_eq!(KeyMappings::display_name("up"), "↑");
        assert_eq!(KeyMappings::display_name("space"), "Space");
        assert_eq!(KeyMappings::display_name("enter"), "⏎");
        assert_eq!(KeyMappings::display_name("backspace"), "⌫");
    }

    #[test]
    fn test_input_validation() {
        let text_event = create_key_event(
            "a",
            Modifiers {
                platform: false,
                control: false,
                alt: false,
                shift: false,
                function: false,
            },
        );

        let shortcut_event = create_key_event(
            "s",
            Modifiers {
                platform: true,
                control: false,
                alt: false,
                shift: false,
                function: false,
            },
        );

        assert!(InputValidation::is_text_input(&text_event));
        assert!(!InputValidation::is_text_input(&shortcut_event));

        assert!(!InputValidation::is_shortcut(&text_event));
        assert!(InputValidation::is_shortcut(&shortcut_event));
    }

    #[test]
    fn test_shortcut_string_representation() {
        let simple = KeyboardShortcut::key("a");
        assert_eq!(simple.to_string(), "a");

        let cmd_shortcut = KeyboardShortcut::cmd("s");
        let expected = if cfg!(target_os = "macos") {
            "⌘+s"
        } else {
            "Ctrl+s"
        };
        assert_eq!(cmd_shortcut.to_string(), expected);

        let complex = KeyboardShortcut::new("z", ModifierSet::platform().with_shift());
        let expected_complex = if cfg!(target_os = "macos") {
            "⌘+⇧+z"
        } else {
            "Ctrl+⇧+z"
        };
        assert_eq!(complex.to_string(), expected_complex);
    }
}
