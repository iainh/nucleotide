// ABOUTME: Workspace domain events for file operations and project management
// ABOUTME: Immutable fact-based events following Domain-Driven Design principles

use std::path::{Path, PathBuf};

/// Workspace domain events - covers file operations, project management, and layout operations
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// Project events
    ProjectOpened {
        workspace_root: PathBuf,
        project_type: Option<ProjectType>,
        file_count: usize,
    },

    ProjectClosed {
        workspace_root: PathBuf,
    },

    /// File tree events
    FileTreeToggled {
        is_visible: bool,
    },

    FileSelected {
        path: PathBuf,
        source: SelectionSource,
    },

    DirectoryExpanded {
        path: PathBuf,
        child_count: usize,
    },

    DirectoryCollapsed {
        path: PathBuf,
    },

    /// Layout events
    LayoutChanged {
        layout_type: LayoutType,
        panel_configuration: PanelConfiguration,
    },

    PanelToggled {
        panel_type: PanelType,
        is_visible: bool,
    },

    /// Tab management
    TabCreated {
        tab_id: TabId,
        doc_id: helix_view::DocumentId,
        title: String,
    },

    TabSwitched {
        previous_tab: Option<TabId>,
        new_tab: TabId,
    },

    TabClosed {
        tab_id: TabId,
        doc_id: helix_view::DocumentId,
    },

    /// File operations
    FileOpened {
        path: PathBuf,
        source: FileOpenSource,
    },

    FileCreated {
        path: PathBuf,
        parent_directory: PathBuf,
    },

    FileDeleted {
        path: PathBuf,
        was_directory: bool,
    },

    FileRenamed {
        old_path: PathBuf,
        new_path: PathBuf,
    },

    /// Workspace navigation
    WorkingDirectoryChanged {
        previous_directory: Option<PathBuf>,
        new_directory: PathBuf,
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

/// Source of file opening
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpenSource {
    FileTree,
    FilePicker,
    RecentFiles,
    External, // Opened from outside the app
    Api,
}

/// Layout types for workspace
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutType {
    Single,
    Horizontal,
    Vertical,
    Grid,
}

/// Panel configuration
#[derive(Debug, Clone)]
pub struct PanelConfiguration {
    pub file_tree_width: Option<f32>,
    pub sidebar_panels: Vec<PanelType>,
    pub bottom_panels: Vec<PanelType>,
}

/// Types of panels in the workspace
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelType {
    FileTree,
    Search,
    Problems,
    Output,
    Terminal,
    Extensions,
}

/// Project type identification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    CSharp,
    Cpp,
    Unknown,
}

/// Tab identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(pub usize);

impl Default for PanelConfiguration {
    fn default() -> Self {
        Self::new()
    }
}

impl PanelConfiguration {
    pub fn new() -> Self {
        Self {
            file_tree_width: None,
            sidebar_panels: Vec::new(),
            bottom_panels: Vec::new(),
        }
    }

    pub fn with_file_tree_width(mut self, width: f32) -> Self {
        self.file_tree_width = Some(width);
        self
    }

    pub fn with_sidebar_panel(mut self, panel: PanelType) -> Self {
        self.sidebar_panels.push(panel);
        self
    }

    pub fn with_bottom_panel(mut self, panel: PanelType) -> Self {
        self.bottom_panels.push(panel);
        self
    }

    pub fn is_panel_visible(&self, panel_type: PanelType) -> bool {
        self.sidebar_panels.contains(&panel_type) || self.bottom_panels.contains(&panel_type)
    }
}

impl TabId {
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }
}

impl From<usize> for TabId {
    fn from(id: usize) -> Self {
        Self(id)
    }
}

impl ProjectType {
    pub fn from_path(path: &Path) -> Self {
        // Check for common project files
        if path.join("Cargo.toml").exists() {
            return Self::Rust;
        }

        if path.join("package.json").exists() {
            if path.join("tsconfig.json").exists() {
                return Self::TypeScript;
            }
            return Self::JavaScript;
        }

        if path.join("pyproject.toml").exists()
            || path.join("requirements.txt").exists()
            || path.join("setup.py").exists()
        {
            return Self::Python;
        }

        if path.join("go.mod").exists() {
            return Self::Go;
        }

        if path.join("pom.xml").exists() || path.join("build.gradle").exists() {
            return Self::Java;
        }

        if path.join("*.csproj").exists() || path.join("*.sln").exists() {
            return Self::CSharp;
        }

        if path.join("CMakeLists.txt").exists() || path.join("Makefile").exists() {
            return Self::Cpp;
        }

        Self::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_configuration() {
        let config = PanelConfiguration::new()
            .with_file_tree_width(200.0)
            .with_sidebar_panel(PanelType::FileTree)
            .with_bottom_panel(PanelType::Problems);

        assert_eq!(config.file_tree_width, Some(200.0));
        assert!(config.is_panel_visible(PanelType::FileTree));
        assert!(config.is_panel_visible(PanelType::Problems));
        assert!(!config.is_panel_visible(PanelType::Terminal));
    }
}
