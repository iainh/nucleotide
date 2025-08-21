// ABOUTME: Editor event handler for V2 event system
// ABOUTME: Handles editor mode changes, command execution, and global editor state

use async_trait::async_trait;
use nucleotide_events::handler::{EventHandler, HandlerError};
use nucleotide_events::v2::editor::Event as EditorEvent;
use nucleotide_logging::{debug, error, info, instrument, warn};

use helix_view::document::Mode;
use std::collections::HashMap;

/// Handler for editor domain events
/// Manages global editor state, mode transitions, and command execution tracking
pub struct EditorHandler {
    /// Current editor mode
    current_mode: Mode,

    /// Command execution history for performance monitoring
    command_history: Vec<CommandExecution>,

    /// Mode change statistics
    mode_statistics: HashMap<Mode, ModeStats>,

    /// Flag to track if handler is initialized
    initialized: bool,
}

/// Command execution record
#[derive(Debug, Clone)]
struct CommandExecution {
    pub command_name: String,
    pub execution_time_ms: u64,
    pub success: bool,
    pub timestamp: std::time::Instant,
}

/// Statistics for mode usage
#[derive(Debug, Clone)]
struct ModeStats {
    pub total_entries: u64,
    pub total_time_spent_ms: u64,
    pub last_entered: Option<std::time::Instant>,
    pub current_session_start: Option<std::time::Instant>,
}

impl EditorHandler {
    /// Create a new editor handler
    pub fn new() -> Self {
        Self {
            current_mode: Mode::Normal, // Default mode
            command_history: Vec::new(),
            mode_statistics: HashMap::new(),
            initialized: false,
        }
    }

    /// Initialize the handler
    #[instrument(skip(self))]
    pub fn initialize(&mut self) -> Result<(), HandlerError> {
        if self.initialized {
            warn!("EditorHandler already initialized");
            return Ok(());
        }

        info!("Initializing EditorHandler");

        // Initialize mode statistics for all modes
        for mode in [Mode::Normal, Mode::Insert, Mode::Select] {
            self.mode_statistics.insert(
                mode,
                ModeStats {
                    total_entries: 0,
                    total_time_spent_ms: 0,
                    last_entered: None,
                    current_session_start: None,
                },
            );
        }

        self.initialized = true;
        Ok(())
    }

    /// Handle mode changed event
    #[instrument(skip(self), fields(previous_mode = ?previous_mode, new_mode = ?new_mode))]
    async fn handle_mode_changed(
        &mut self,
        previous_mode: Mode,
        new_mode: Mode,
        context: nucleotide_events::v2::editor::ModeChangeContext,
    ) -> Result<(), HandlerError> {
        info!(
            previous_mode = ?previous_mode,
            new_mode = ?new_mode,
            context = ?context,
            "Processing editor mode change"
        );

        let now = std::time::Instant::now();

        // Update statistics for previous mode (end session)
        if let Some(prev_stats) = self.mode_statistics.get_mut(&previous_mode) {
            if let Some(session_start) = prev_stats.current_session_start {
                let session_duration = now.duration_since(session_start).as_millis() as u64;
                prev_stats.total_time_spent_ms += session_duration;
                prev_stats.current_session_start = None;
            }
        }

        // Update statistics for new mode (start session)
        if let Some(new_stats) = self.mode_statistics.get_mut(&new_mode) {
            new_stats.total_entries += 1;
            new_stats.last_entered = Some(now);
            new_stats.current_session_start = Some(now);
        }

        // Update current mode
        self.current_mode = new_mode;

        info!(
            previous_mode = ?previous_mode,
            new_mode = ?new_mode,
            "Editor mode change processed successfully"
        );

        Ok(())
    }

    /// Handle command executed event
    #[instrument(skip(self), fields(command_name = %command_name, execution_time_ms = execution_time_ms))]
    async fn handle_command_executed(
        &mut self,
        command_name: String,
        execution_time_ms: u64,
        success: bool,
        context: nucleotide_events::v2::editor::CommandContext,
    ) -> Result<(), HandlerError> {
        debug!(
            command_name = %command_name,
            execution_time_ms = execution_time_ms,
            success = success,
            context = ?context,
            "Processing command execution"
        );

        // Record command execution
        let execution = CommandExecution {
            command_name: command_name.clone(),
            execution_time_ms,
            success,
            timestamp: std::time::Instant::now(),
        };

        // Maintain command history (keep last 100 commands)
        self.command_history.push(execution);
        if self.command_history.len() > 100 {
            self.command_history.remove(0);
        }

        // Log slow commands
        if execution_time_ms > 100 {
            warn!(
                command_name = %command_name,
                execution_time_ms = execution_time_ms,
                "Slow command execution detected"
            );
        }

        info!(
            command_name = %command_name,
            execution_time_ms = execution_time_ms,
            success = success,
            "Command execution processed successfully"
        );

        Ok(())
    }

    /// Get current editor mode
    pub fn get_current_mode(&self) -> Mode {
        self.current_mode
    }

    /// Get mode statistics (for debugging/monitoring)
    pub fn get_mode_stats(&self, mode: &Mode) -> Option<&ModeStats> {
        self.mode_statistics.get(mode)
    }

    /// Get recent command history
    pub fn get_recent_commands(&self, limit: usize) -> &[CommandExecution] {
        let start = self.command_history.len().saturating_sub(limit);
        &self.command_history[start..]
    }
}

#[async_trait]
impl EventHandler<EditorEvent> for EditorHandler {
    type Error = HandlerError;

    #[instrument(skip(self, event))]
    async fn handle(&mut self, event: EditorEvent) -> Result<(), Self::Error> {
        if !self.initialized {
            error!("EditorHandler not initialized");
            return Err(HandlerError::NotInitialized);
        }

        match event {
            EditorEvent::ModeChanged {
                previous_mode,
                new_mode,
                context,
            } => {
                self.handle_mode_changed(previous_mode, new_mode, context)
                    .await
            }
            EditorEvent::CommandExecuted {
                command_name,
                execution_time_ms,
                success,
                context,
            } => {
                self.handle_command_executed(command_name, execution_time_ms, success, context)
                    .await
            }
            _ => {
                // Other events not yet implemented - log for future reference
                debug!(event = ?event, "Editor event not yet handled by V2 system");
                Ok(())
            }
        }
    }
}

impl Default for EditorHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::document::Mode;
    use nucleotide_events::v2::editor::{CommandContext, Event as EditorEvent, ModeChangeContext};

    #[tokio::test]
    async fn test_editor_handler_initialization() {
        let mut handler = EditorHandler::new();
        assert!(!handler.initialized);
        assert_eq!(handler.get_current_mode(), Mode::Normal);

        handler.initialize().unwrap();
        assert!(handler.initialized);

        // Check that mode statistics were initialized
        assert!(handler.get_mode_stats(&Mode::Normal).is_some());
        assert!(handler.get_mode_stats(&Mode::Insert).is_some());
        assert!(handler.get_mode_stats(&Mode::Select).is_some());
    }

    #[tokio::test]
    async fn test_mode_changed_event() {
        let mut handler = EditorHandler::new();
        handler.initialize().unwrap();

        let event = EditorEvent::ModeChanged {
            previous_mode: Mode::Normal,
            new_mode: Mode::Insert,
            context: ModeChangeContext::UserAction,
        };

        handler.handle(event).await.unwrap();

        assert_eq!(handler.get_current_mode(), Mode::Insert);

        let insert_stats = handler.get_mode_stats(&Mode::Insert).unwrap();
        assert_eq!(insert_stats.total_entries, 1);
        assert!(insert_stats.last_entered.is_some());
        assert!(insert_stats.current_session_start.is_some());
    }

    #[tokio::test]
    async fn test_command_executed_event() {
        let mut handler = EditorHandler::new();
        handler.initialize().unwrap();

        let event = EditorEvent::CommandExecuted {
            command_name: "open_file".to_string(),
            execution_time_ms: 50,
            success: true,
            context: CommandContext::new(nucleotide_events::v2::editor::CommandSource::Keyboard),
        };

        handler.handle(event).await.unwrap();

        let recent_commands = handler.get_recent_commands(10);
        assert_eq!(recent_commands.len(), 1);
        assert_eq!(recent_commands[0].command_name, "open_file");
        assert_eq!(recent_commands[0].execution_time_ms, 50);
        assert!(recent_commands[0].success);
    }

    #[tokio::test]
    async fn test_mode_session_timing() {
        let mut handler = EditorHandler::new();
        handler.initialize().unwrap();

        // Change to Insert mode
        let event1 = EditorEvent::ModeChanged {
            previous_mode: Mode::Normal,
            new_mode: Mode::Insert,
            context: ModeChangeContext::UserAction,
        };
        handler.handle(event1).await.unwrap();

        // Small delay simulation
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;

        // Change back to Normal mode
        let event2 = EditorEvent::ModeChanged {
            previous_mode: Mode::Insert,
            new_mode: Mode::Normal,
            context: ModeChangeContext::UserAction,
        };
        handler.handle(event2).await.unwrap();

        // Check that Insert mode session was recorded
        let insert_stats = handler.get_mode_stats(&Mode::Insert).unwrap();
        assert_eq!(insert_stats.total_entries, 1);
        assert!(insert_stats.total_time_spent_ms > 0);
        assert!(insert_stats.current_session_start.is_none()); // Session ended
    }

    #[tokio::test]
    async fn test_command_history_limit() {
        let mut handler = EditorHandler::new();
        handler.initialize().unwrap();

        // Add 105 commands (should only keep last 100)
        for i in 0..105 {
            let event = EditorEvent::CommandExecuted {
                command_name: format!("command_{}", i),
                execution_time_ms: 10,
                success: true,
                context: CommandContext::new(
                    nucleotide_events::v2::editor::CommandSource::Keyboard,
                ),
            };
            handler.handle(event).await.unwrap();
        }

        let recent_commands = handler.get_recent_commands(200);
        assert_eq!(recent_commands.len(), 100);
        assert_eq!(recent_commands[0].command_name, "command_5"); // First 5 were removed
        assert_eq!(recent_commands[99].command_name, "command_104");
    }

    #[tokio::test]
    async fn test_uninitialized_handler_error() {
        let mut handler = EditorHandler::new();
        let event = EditorEvent::ModeChanged {
            previous_mode: Mode::Normal,
            new_mode: Mode::Insert,
            context: ModeChangeContext::UserAction,
        };

        let result = handler.handle(event).await;
        assert!(matches!(result, Err(HandlerError::NotInitialized)));
    }
}
