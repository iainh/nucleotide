// ABOUTME: UI domain events for window, theme, and interface interactions
// ABOUTME: Pure UI events without business logic dependencies

/// UI domain events - covers window events, theme changes, and pure UI interactions
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// Theme and appearance
    ThemeChanged {
        theme_name: String,
        is_dark_mode: bool,
    },

    SystemAppearanceChanged {
        appearance: SystemAppearance,
    },

    FontChanged {
        font_family: String,
        font_size: f32,
    },

    ScaleChanged {
        scale_factor: f32,
    },

    /// Window events
    WindowResized {
        width: u32,
        height: u32,
    },

    WindowFocused {
        gained_focus: bool,
    },

    WindowMinimized {
        is_minimized: bool,
    },

    WindowMoved {
        x: i32,
        y: i32,
    },

    WindowMaximized {
        is_maximized: bool,
    },

    WindowFullscreen {
        is_fullscreen: bool,
    },

    /// Overlay management
    OverlayShown {
        overlay_type: OverlayType,
        overlay_id: String,
    },

    OverlayHidden {
        overlay_type: OverlayType,
        overlay_id: String,
    },

    /// Input events
    KeyboardShortcutTriggered {
        shortcut: String,
        modifiers: KeyModifiers,
    },

    /// Menu events
    MenuOpened {
        menu_type: MenuType,
    },

    MenuClosed {
        menu_type: MenuType,
    },

    /// Accessibility events
    AccessibilityModeChanged {
        enabled: bool,
        features: Vec<AccessibilityFeature>,
    },

    /// Cursor events
    CursorShapeChanged {
        shape: CursorShape,
    },
}

/// System appearance states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAppearance {
    Light,
    Dark,
    Auto,
}

/// Types of overlays
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayType {
    CommandPalette,
    FilePicker,
    Search,
    Completion,
    Prompt,
    Dialog,
    Tooltip,
    ContextMenu,
}

/// Keyboard modifiers
#[derive(Debug, Clone)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Menu types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuType {
    Main,
    Context,
    Tab,
    StatusBar,
}

/// Accessibility features
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilityFeature {
    ScreenReader,
    HighContrast,
    ReducedMotion,
    LargeText,
    VoiceControl,
}

/// Cursor shapes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Line,
    Underline,
    Hidden,
}

impl KeyModifiers {
    pub fn new() -> Self {
        Self {
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        }
    }

    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub fn with_ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    pub fn with_alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub fn with_meta(mut self) -> Self {
        self.meta = true;
        self
    }

    pub fn is_empty(&self) -> bool {
        !self.shift && !self.ctrl && !self.alt && !self.meta
    }

    pub fn has_any(&self) -> bool {
        !self.is_empty()
    }
}

impl Default for KeyModifiers {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_change_event() {
        let event = Event::ThemeChanged {
            theme_name: "dracula".to_string(),
            is_dark_mode: true,
        };

        match event {
            Event::ThemeChanged {
                theme_name,
                is_dark_mode,
            } => {
                assert_eq!(theme_name, "dracula");
                assert!(is_dark_mode);
            }
            _ => panic!("Expected ThemeChanged event"),
        }
    }

    #[test]
    fn test_key_modifiers() {
        let modifiers = KeyModifiers::new().with_ctrl().with_shift();

        assert!(modifiers.ctrl);
        assert!(modifiers.shift);
        assert!(!modifiers.alt);
        assert!(!modifiers.meta);
        assert!(modifiers.has_any());
        assert!(!modifiers.is_empty());

        let empty_modifiers = KeyModifiers::new();
        assert!(empty_modifiers.is_empty());
        assert!(!empty_modifiers.has_any());
    }

    #[test]
    fn test_system_appearance() {
        let appearances = [
            SystemAppearance::Light,
            SystemAppearance::Dark,
            SystemAppearance::Auto,
        ];

        for appearance in appearances {
            let _event = Event::SystemAppearanceChanged { appearance };
        }
    }

    #[test]
    fn test_overlay_types() {
        let overlay_types = [
            OverlayType::CommandPalette,
            OverlayType::FilePicker,
            OverlayType::Search,
            OverlayType::Completion,
            OverlayType::Prompt,
            OverlayType::Dialog,
            OverlayType::Tooltip,
            OverlayType::ContextMenu,
        ];

        for overlay_type in overlay_types {
            let _event = Event::OverlayShown {
                overlay_type,
                overlay_id: "test-overlay".to_string(),
            };
        }
    }

    #[test]
    fn test_window_events() {
        let events = [
            Event::WindowResized {
                width: 1920,
                height: 1080,
            },
            Event::WindowFocused { gained_focus: true },
            Event::WindowMinimized {
                is_minimized: false,
            },
            Event::WindowMoved { x: 100, y: 200 },
            Event::WindowMaximized { is_maximized: true },
            Event::WindowFullscreen {
                is_fullscreen: false,
            },
        ];

        // All window events should be valid
        for _event in events {
            // Event creation successful
        }
    }

    #[test]
    fn test_accessibility_features() {
        let features = vec![
            AccessibilityFeature::ScreenReader,
            AccessibilityFeature::HighContrast,
            AccessibilityFeature::ReducedMotion,
        ];

        let _event = Event::AccessibilityModeChanged {
            enabled: true,
            features,
        };
    }

    #[test]
    fn test_cursor_shapes() {
        let shapes = [
            CursorShape::Block,
            CursorShape::Line,
            CursorShape::Underline,
            CursorShape::Hidden,
        ];

        for shape in shapes {
            let _event = Event::CursorShapeChanged { shape };
        }
    }

    #[test]
    fn test_menu_types() {
        let menu_types = [
            MenuType::Main,
            MenuType::Context,
            MenuType::Tab,
            MenuType::StatusBar,
        ];

        for menu_type in menu_types {
            let _event = Event::MenuOpened { menu_type };
        }
    }
}
