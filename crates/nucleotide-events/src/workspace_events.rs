// ABOUTME: Workspace-level events for layout and tab management
// ABOUTME: Events for workspace operations, panels, and file management

use std::path::PathBuf;

/// Workspace events (for future nucleotide-workspace crate)
#[derive(Debug, Clone)]
pub enum WorkspaceEvent {
    /// Tab opened
    TabOpened { id: String },

    /// Tab closed
    TabClosed { id: String },

    /// Tab switched
    TabSwitched { id: String },

    /// Split created
    SplitCreated { direction: SplitDirection },

    /// Panel toggled
    PanelToggled { panel: PanelType },

    /// Open file
    OpenFile { path: PathBuf },

    /// Open directory
    OpenDirectory { path: PathBuf },

    /// File tree event
    FileTreeToggled,

    /// File selected in tree
    FileSelected { path: PathBuf },
}

#[derive(Debug, Clone, Copy)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy)]
pub enum PanelType {
    FileTree,
    Terminal,
    Search,
    Diagnostics,
}
