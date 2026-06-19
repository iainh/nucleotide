// ABOUTME: Terminal domain events for spawning sessions, IO, resize, focus and lifecycle
// ABOUTME: Immutable fact-based events following Domain-Driven Design principles

use std::path::PathBuf;

/// Strongly-typed identifier for terminal sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalId(pub u64);

/// Terminal domain events
#[derive(Debug, Clone)]
pub enum Event {
    /// Request to spawn a terminal session
    SpawnRequested {
        id: TerminalId,
        cwd: Option<PathBuf>,
        shell: Option<String>,
        env: Vec<(String, String)>,
    },

    /// Request to spawn a specific command in a terminal session
    CommandSpawnRequested {
        id: TerminalId,
        cwd: Option<PathBuf>,
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },

    /// Terminal viewport resized (in character cells)
    Resized {
        id: TerminalId,
        cols: u16,
        rows: u16,
    },

    /// Terminal viewport resized with explicit cell metrics (recommended)
    /// Allows the emulator to construct an accurate SizeInfo in pixels.
    ResizedWithMetrics {
        id: TerminalId,
        cols: u16,
        rows: u16,
        cell_width: f32,
        cell_height: f32,
    },

    /// Input bytes sent to the terminal (raw)
    Input { id: TerminalId, bytes: Vec<u8> },

    /// Output bytes produced by the terminal (raw)
    Output { id: TerminalId, bytes: Vec<u8> },

    /// Terminal process exited
    Exited {
        id: TerminalId,
        code: Option<i32>,
        signal: Option<i32>,
    },

    /// Focus changed for a terminal view
    FocusChanged { id: TerminalId, focused: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_requested_event() {
        let id = TerminalId(1);
        let e = Event::SpawnRequested {
            id,
            cwd: Some(PathBuf::from("/tmp")),
            shell: Some("/bin/zsh".into()),
            env: vec![("RUST_LOG".into(), "info".into())],
        };

        match e {
            Event::SpawnRequested {
                id: tid,
                cwd,
                shell,
                env,
            } => {
                assert_eq!(tid, id);
                assert_eq!(cwd, Some(PathBuf::from("/tmp")));
                assert_eq!(shell, Some("/bin/zsh".into()));
                assert_eq!(env.len(), 1);
            }
            _ => panic!("expected SpawnRequested"),
        }
    }

    #[test]
    fn test_command_spawn_requested_event() {
        let id = TerminalId(2);
        let e = Event::CommandSpawnRequested {
            id,
            cwd: Some(PathBuf::from("/tmp")),
            program: "cargo".into(),
            args: vec!["test".into()],
            env: vec![("RUST_LOG".into(), "info".into())],
        };

        match e {
            Event::CommandSpawnRequested {
                id: tid,
                cwd,
                program,
                args,
                env,
            } => {
                assert_eq!(tid, id);
                assert_eq!(cwd, Some(PathBuf::from("/tmp")));
                assert_eq!(program, "cargo");
                assert_eq!(args, vec!["test"]);
                assert_eq!(env.len(), 1);
            }
            _ => panic!("expected CommandSpawnRequested"),
        }
    }
}
