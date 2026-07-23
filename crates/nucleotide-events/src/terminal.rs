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

    /// Terminal process exited
    Exited {
        id: TerminalId,
        code: Option<i32>,
        signal: Option<i32>,
    },
}
