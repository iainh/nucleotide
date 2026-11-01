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

    /// Request a login shell from the connected workspace PTY service.
    ServiceSpawnRequested {
        id: TerminalId,
        cwd: PathBuf,
        shell: Option<String>,
        env: Vec<(String, String)>,
    },

    /// Request a command from the connected workspace PTY service.
    ServiceCommandSpawnRequested {
        id: TerminalId,
        cwd: PathBuf,
        program: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },

    /// Terminal viewport resized with explicit cell metrics.
    Resized {
        id: TerminalId,
        cols: u16,
        rows: u16,
        cell_width: f32,
        cell_height: f32,
    },

    /// Input bytes sent to the terminal (raw)
    Input { id: TerminalId, bytes: Vec<u8> },

    /// Explicit user/workspace request to close a session, including a pending spawn.
    TerminateRequested { id: TerminalId },

    /// A process was registered successfully. This is a fact, not a spawn request.
    Spawned { id: TerminalId },

    /// Launch failed; the session's diagnostic output remains until explicit close.
    SpawnFailed { id: TerminalId, error: String },

    /// Observed process completion. Never interpreted as a termination request.
    Exited {
        id: TerminalId,
        code: Option<i32>,
        signal: Option<i32>,
    },
}
