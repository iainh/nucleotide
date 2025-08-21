// ABOUTME: Editor domain events for mode changes, commands, and editor status
// ABOUTME: Immutable fact-based events following Domain-Driven Design principles

use helix_view::document::Mode;

/// Editor domain events - covers mode changes, command execution, and editor status
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// Editor mode changed
    ModeChanged {
        previous_mode: Mode,
        new_mode: Mode,
        context: ModeChangeContext,
    },

    /// Command executed
    CommandExecuted {
        command_name: String,
        execution_time_ms: u64,
        success: bool,
        context: CommandContext,
    },

    /// Editor status changed
    StatusChanged {
        message: String,
        severity: StatusSeverity,
        timeout_ms: Option<u64>,
    },

    /// Editor configuration changed
    ConfigurationChanged {
        section: ConfigSection,
        key: String,
        previous_value: Option<String>,
        new_value: String,
    },

    /// Macro recording started/stopped
    MacroRecordingChanged {
        is_recording: bool,
        register: Option<char>,
    },

    /// Editor shutdown requested
    ShutdownRequested { reason: ShutdownReason, force: bool },

    /// Redraw requested
    RedrawRequested { reason: RedrawReason, urgent: bool },

    /// Search operation completed
    SearchCompleted {
        query: String,
        match_count: usize,
        search_time_ms: u64,
    },

    /// Replace operation completed  
    ReplaceCompleted {
        pattern: String,
        replacement: String,
        replacement_count: usize,
    },
}

/// Context for mode changes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeChangeContext {
    UserAction,
    Command,
    AutoSwitch,
    Error,
}

/// Context for command execution
#[derive(Debug, Clone)]
pub struct CommandContext {
    pub source: CommandSource,
    pub args: Vec<String>,
}

/// Source of command execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    Keyboard,
    Menu,
    CommandPalette,
    Api,
    Macro,
}

/// Severity of status messages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSeverity {
    Info,
    Warning,
    Error,
    Success,
}

/// Configuration sections
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSection {
    Editor,
    Theme,
    Keys,
    Languages,
    Ui,
}

/// Reasons for shutdown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    UserRequest,
    SystemShutdown,
    Error,
    AutoSave,
}

/// Reasons for redraw
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedrawReason {
    ContentChange,
    ModeChange,
    Selection,
    StatusUpdate,
    ThemeChange,
    WindowResize,
    Force,
}

impl CommandContext {
    pub fn new(source: CommandSource) -> Self {
        Self {
            source,
            args: Vec::new(),
        }
    }

    pub fn with_args(source: CommandSource, args: Vec<String>) -> Self {
        Self { source, args }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_change_event() {
        let event = Event::ModeChanged {
            previous_mode: Mode::Normal,
            new_mode: Mode::Insert,
            context: ModeChangeContext::UserAction,
        };

        match event {
            Event::ModeChanged {
                previous_mode,
                new_mode,
                context,
            } => {
                assert_eq!(previous_mode, Mode::Normal);
                assert_eq!(new_mode, Mode::Insert);
                assert_eq!(context, ModeChangeContext::UserAction);
            }
            _ => panic!("Expected ModeChanged event"),
        }
    }

    #[test]
    fn test_command_context() {
        let context = CommandContext::with_args(
            CommandSource::CommandPalette,
            vec!["arg1".to_string(), "arg2".to_string()],
        );

        assert_eq!(context.source, CommandSource::CommandPalette);
        assert_eq!(context.args.len(), 2);
        assert_eq!(context.args[0], "arg1");
        assert_eq!(context.args[1], "arg2");
    }

    #[test]
    fn test_status_severities() {
        let severities = [
            StatusSeverity::Info,
            StatusSeverity::Warning,
            StatusSeverity::Error,
            StatusSeverity::Success,
        ];

        for severity in severities {
            let _event = Event::StatusChanged {
                message: "Test message".to_string(),
                severity,
                timeout_ms: Some(5000),
            };
        }
    }

    #[test]
    fn test_shutdown_reasons() {
        let reasons = [
            ShutdownReason::UserRequest,
            ShutdownReason::SystemShutdown,
            ShutdownReason::Error,
            ShutdownReason::AutoSave,
        ];

        for reason in reasons {
            let _event = Event::ShutdownRequested {
                reason,
                force: false,
            };
        }
    }

    #[test]
    fn test_redraw_reasons() {
        let reasons = [
            RedrawReason::ContentChange,
            RedrawReason::ModeChange,
            RedrawReason::Selection,
            RedrawReason::StatusUpdate,
            RedrawReason::ThemeChange,
            RedrawReason::WindowResize,
            RedrawReason::Force,
        ];

        for reason in reasons {
            let _event = Event::RedrawRequested {
                reason,
                urgent: false,
            };
        }
    }
}
