// ABOUTME: Workspace file selection events and file operation requests
// ABOUTME: Shared intent payloads for application and UI consumers

use std::path::PathBuf;

/// Workspace file selections and user-initiated file operation requests.
#[derive(Debug, Clone)]
pub enum Event {
    FileSelected {
        path: PathBuf,
        source: SelectionSource,
    },

    // File operation intents (user-initiated requests)
    FileOpRequested {
        intent: FileOpIntent,
    },
}

/// File operation intent kinds initiated from UI (e.g., context menu)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOpIntent {
    NewFile { parent: PathBuf, name: String },
    NewFolder { parent: PathBuf, name: String },
    Rename { path: PathBuf, new_name: String },
    Delete { path: PathBuf, mode: DeleteMode },
    Duplicate { path: PathBuf, target_name: String },
    CopyPath { path: PathBuf, kind: PathCopyKind },
    RevealInOs { path: PathBuf },
}

/// Kind of path to copy to clipboard
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathCopyKind {
    Absolute,
    RelativeToWorkspace,
}

/// Delete behavior mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    Trash,
    Permanent,
}

/// Source of file selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionSource {
    Click,
    Keyboard,
    Search,
    Command,
    Api,
}
