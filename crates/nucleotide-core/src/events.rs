// ABOUTME: Core event definitions that are crate-specific and composable
// ABOUTME: Replaces the monolithic Update enum with modular event types

use helix_view::{DocumentId, ViewId};
use std::any::Any;
use std::sync::Arc;

/// Core editor events that don't depend on UI components
#[derive(Debug, Clone)]
pub enum CoreEvent {
    /// Document was modified
    DocumentChanged { doc_id: DocumentId },

    /// Selection changed in a view
    SelectionChanged { doc_id: DocumentId, view_id: ViewId },

    /// Editor mode changed
    ModeChanged {
        old_mode: helix_view::document::Mode,
        new_mode: helix_view::document::Mode,
    },

    /// Diagnostics updated for a document
    DiagnosticsChanged { doc_id: DocumentId },

    /// Document opened
    DocumentOpened { doc_id: DocumentId },

    /// Document closed
    DocumentClosed { doc_id: DocumentId },

    /// View gained focus
    ViewFocused { view_id: ViewId },

    /// Editor needs redraw
    RedrawRequested,

    /// Status message to display
    StatusMessage {
        message: String,
        severity: MessageSeverity,
    },

    /// Document saved
    DocumentSaved {
        doc_id: DocumentId,
        path: Option<String>,
    },

    /// Command submitted
    CommandSubmitted { command: String },

    /// Search submitted
    SearchSubmitted { query: String },

    /// Should quit the application
    ShouldQuit,

    /// Status changed with message and severity
    StatusChanged {
        message: String,
        severity: MessageSeverity,
    },

    /// Completion requested
    CompletionRequested {
        doc_id: DocumentId,
        view_id: ViewId,
        trigger: crate::shared_types::CompletionTrigger,
    },
}

/// Message severity levels
#[derive(Debug, Clone, Copy)]
pub enum MessageSeverity {
    Info,
    Warning,
    Error,
}

/// UI-specific events (for nucleotide-ui crate)
#[derive(Clone)]
pub enum UiEvent {
    /// Theme changed
    ThemeChanged { theme_name: String },

    /// UI scale changed
    ScaleChanged { scale: f32 },

    /// Font changed
    FontChanged { font_family: String },

    /// Layout changed
    LayoutChanged,

    /// Search command submitted from overlay
    SearchSubmitted { query: String },

    /// Command submitted from overlay
    CommandSubmitted { command: String },

    /// File should be opened
    FileOpenRequested { path: std::path::PathBuf },

    /// Directory should be opened
    DirectoryOpenRequested { path: std::path::PathBuf },

    /// Info box should be shown
    ShowInfo { title: String, body: Vec<String> },

    /// Completion triggered
    CompletionTriggered,

    /// Show prompt (with boxed prompt object for transition period)
    ShowPrompt {
        prompt_text: String,
        initial_value: String,
        // Temporary: store the prompt object during transition
        prompt_object: Option<Arc<dyn Any + Send + Sync>>,
    },

    /// Show picker (with boxed picker object for transition period)
    ShowPicker {
        picker_type: PickerType,
        // Temporary: store the picker object during transition
        picker_object: Option<Arc<dyn Any + Send + Sync>>,
    },

    /// Show completion widget
    ShowCompletion,

    /// Hide completion widget
    HideCompletion,
}

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
    OpenFile { path: std::path::PathBuf },

    /// Open directory
    OpenDirectory { path: std::path::PathBuf },

    /// File tree event
    FileTreeToggled,

    /// File selected in tree
    FileSelected { path: std::path::PathBuf },
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

#[derive(Debug, Clone, Copy)]
pub enum PickerType {
    File,
    Buffer,
    Directory,
    Command,
    Symbol,
}

/// LSP events (already in nucleotide-lsp crate)
#[derive(Debug, Clone)]
pub enum LspEvent {
    /// Server initialized
    ServerInitialized {
        server_id: helix_lsp::LanguageServerId,
    },

    /// Server exited
    ServerExited {
        server_id: helix_lsp::LanguageServerId,
    },

    /// Progress update
    Progress {
        server_id: usize,
        percentage: Option<u32>,
        message: String,
    },

    /// Completion available
    CompletionAvailable { doc_id: DocumentId },
}

impl std::fmt::Debug for UiEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiEvent::ThemeChanged { theme_name } => f
                .debug_struct("ThemeChanged")
                .field("theme_name", theme_name)
                .finish(),
            UiEvent::ScaleChanged { scale } => f
                .debug_struct("ScaleChanged")
                .field("scale", scale)
                .finish(),
            UiEvent::FontChanged { font_family } => f
                .debug_struct("FontChanged")
                .field("font_family", font_family)
                .finish(),
            UiEvent::LayoutChanged => write!(f, "LayoutChanged"),
            UiEvent::SearchSubmitted { query } => f
                .debug_struct("SearchSubmitted")
                .field("query", query)
                .finish(),
            UiEvent::CommandSubmitted { command } => f
                .debug_struct("CommandSubmitted")
                .field("command", command)
                .finish(),
            UiEvent::FileOpenRequested { path } => f
                .debug_struct("FileOpenRequested")
                .field("path", path)
                .finish(),
            UiEvent::DirectoryOpenRequested { path } => f
                .debug_struct("DirectoryOpenRequested")
                .field("path", path)
                .finish(),
            UiEvent::ShowInfo { title, body } => f
                .debug_struct("ShowInfo")
                .field("title", title)
                .field("body", body)
                .finish(),
            UiEvent::CompletionTriggered => write!(f, "CompletionTriggered"),
            UiEvent::ShowPrompt {
                prompt_text,
                initial_value,
                ..
            } => f
                .debug_struct("ShowPrompt")
                .field("prompt_text", prompt_text)
                .field("initial_value", initial_value)
                .finish(),
            UiEvent::ShowPicker { picker_type, .. } => f
                .debug_struct("ShowPicker")
                .field("picker_type", picker_type)
                .finish(),
            UiEvent::ShowCompletion => write!(f, "ShowCompletion"),
            UiEvent::HideCompletion => write!(f, "HideCompletion"),
        }
    }
}

/// Aggregated event type for the main application
#[derive(Clone)]
pub enum AppEvent {
    Core(CoreEvent),
    Ui(UiEvent),
    Workspace(WorkspaceEvent),
    Lsp(LspEvent),
}

impl std::fmt::Debug for AppEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppEvent::Core(e) => write!(f, "Core({:?})", e),
            AppEvent::Ui(e) => write!(f, "Ui({:?})", e),
            AppEvent::Workspace(e) => write!(f, "Workspace({:?})", e),
            AppEvent::Lsp(e) => write!(f, "Lsp({:?})", e),
        }
    }
}

/// Event bus trait for dispatching events
pub trait EventBus {
    /// Dispatch a core event
    fn dispatch_core(&self, event: CoreEvent);

    /// Dispatch a UI event
    fn dispatch_ui(&self, event: UiEvent);

    /// Dispatch a workspace event
    fn dispatch_workspace(&self, event: WorkspaceEvent);

    /// Dispatch an LSP event
    fn dispatch_lsp(&self, event: LspEvent);
}

/// Event handler trait for receiving events
pub trait EventHandler {
    /// Handle a core event
    fn handle_core(&mut self, _event: &CoreEvent) {}

    /// Handle a UI event
    fn handle_ui(&mut self, _event: &UiEvent) {}

    /// Handle a workspace event
    fn handle_workspace(&mut self, _event: &WorkspaceEvent) {}

    /// Handle an LSP event
    fn handle_lsp(&mut self, _event: &LspEvent) {}
}
