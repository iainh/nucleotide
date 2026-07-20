// ABOUTME: Application-specific type re-exports
// ABOUTME: Re-exports shared types from nucleotide-core

// Re-export shared types from core (now from nucleotide-types via nucleotide-core)
pub use nucleotide_core::{
    CompletionTrigger, EditorFontConfig, EditorStatus, FontSettings, Severity, UiFontConfig,
};

// Re-export V2 event types from nucleotide-core
pub use nucleotide_core::{AppEvent, DocumentEvent, UiEvent, WorkspaceEvent};

// Re-export UI enums from V2 events
pub use nucleotide_events::v2::ui::SystemAppearance;

// Local enums that haven't been migrated to V2 yet
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickerType {
    File,
    Buffer,
    Directory,
    Command,
    Symbol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverDocEntry {
    pub server_name: String,
    pub markdown: String,
}

#[derive(Debug, Clone)]
pub struct LspLocation {
    pub path: std::path::PathBuf,
    pub range: helix_lsp::lsp::Range,
    pub offset_encoding: helix_lsp::OffsetEncoding,
}

#[derive(Debug, Clone)]
pub struct JumpLocation {
    pub doc_id: helix_view::DocumentId,
    pub selection: helix_core::Selection,
}

#[derive(Debug, Clone)]
pub struct SyntaxFileLocation {
    pub path: std::path::PathBuf,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSearchLocation {
    pub path: std::path::PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticLocation {
    pub doc_id: helix_view::DocumentId,
    pub path: Option<std::path::PathBuf>,
    pub offset: usize,
}

// Hybrid Update enum for event system
// Uses Event(AppEvent) for data-only events and direct variants for complex UI components with behavior
pub enum Update {
    // Event-based updates (data only)
    Event(AppEvent),

    // Complex UI components with behavior (closures/callbacks)
    // These cannot be easily serialized into events
    Prompt(crate::prompt::Prompt),
    Picker(crate::picker::Picker),
    DirectoryPicker(crate::picker::Picker),
    RemoteConnectionManager,
    Completion(gpui::Entity<nucleotide_ui::completion_v2::CompletionView>),
    HoverDocs(Vec<HoverDocEntry>),
    CompletionEvent(helix_view::handlers::completion::CompletionEvent),
    Info(helix_view::info::Info),

    // Legacy events still being migrated
    EditorEvent(helix_view::editor::EditorEvent),
    EditorStatus(EditorStatus),
    FileTreeEvent(crate::file_tree::FileTreeEvent),

    // Temporary - will be removed once all code is updated to use Event(AppEvent)
    Redraw,
    ShouldQuit,
    CommandSubmitted(String),
    SearchSubmitted(String),
    GlobalSearchSubmitted(String),
    FileTreeSearchSubmitted(String),
    RegexSelectionSubmitted {
        action: RegexSelectionAction,
        regex: String,
    },
    OpenFile(std::path::PathBuf),
    OpenDirectory(std::path::PathBuf),
    OpenRemote(String),
    OpenRemoteWithOptions {
        input: String,
        options: nucleotide_remote::RemoteWorkspaceBackendOptions,
    },
    OpenRemoteWithBootstrap {
        input: String,
        bootstrap: nucleotide_remote::RemoteWorkspaceBootstrap,
    },
    SelectionChanged {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    },
    SelectionRestored {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    },
    ViewFocused {
        view_id: helix_view::ViewId,
    },
    ViewportScroll {
        view_id: helix_view::ViewId,
        request: nucleotide_editor::EditorViewportScrollRequest,
    },
    ViewportCursor {
        view_id: helix_view::ViewId,
        request: nucleotide_editor::EditorViewportCursorRequest,
    },
    ShowFilePicker,
    ShowFilePickerAt(std::path::PathBuf),
    ShowBufferPicker,
    ShowCodeActions,
    ShowRunnables,
    ShowHoverDocs,
    RunTask(nucleotide_events::v2::run::ResolvedTask),
    ToggleFileTree,
    SemanticShortcut(SemanticShortcutIntent),
    TerminalPanel(gpui::Entity<nucleotide_terminal_panel::TerminalPanel>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticShortcutIntent {
    Quit,
    OpenFile,
    OpenDirectory,
    Save,
    CloseFile,
    NewFile,
    ShowFileFinder,
    ShowCommandPrompt,
    ShowBufferPicker,
    ShowCodeActions,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    OpenSettings,
    ShowRunnables,
    RunNearest,
    RunLast,
    RunFileTests,
    ToggleFileTree,
}

impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Update::Event(event) => write!(f, "Event({event:?})"),
            Update::Prompt(_) => write!(f, "Prompt(...)"),
            Update::Picker(_) => write!(f, "Picker(...)"),
            Update::DirectoryPicker(_) => write!(f, "DirectoryPicker(...)"),
            Update::RemoteConnectionManager => write!(f, "RemoteConnectionManager"),
            Update::Completion(_) => write!(f, "Completion(...)"),
            Update::Info(_) => write!(f, "Info(...)"),
            Update::HoverDocs(entries) => write!(f, "HoverDocs(len={})", entries.len()),
            Update::EditorEvent(_) => write!(f, "EditorEvent(...)"),
            Update::EditorStatus(status) => write!(f, "EditorStatus({status:?})"),
            Update::Redraw => write!(f, "Redraw"),
            Update::OpenFile(path) => write!(f, "OpenFile({path:?})"),
            Update::OpenDirectory(path) => write!(f, "OpenDirectory({path:?})"),
            Update::ShouldQuit => write!(f, "ShouldQuit"),
            Update::CommandSubmitted(cmd) => write!(f, "CommandSubmitted({cmd:?})"),
            Update::SearchSubmitted(query) => write!(f, "SearchSubmitted({query:?})"),
            Update::GlobalSearchSubmitted(query) => {
                write!(f, "GlobalSearchSubmitted({query:?})")
            }
            Update::FileTreeSearchSubmitted(query) => {
                write!(f, "FileTreeSearchSubmitted({query:?})")
            }
            Update::OpenRemote(input) => write!(f, "OpenRemote({input:?})"),
            Update::OpenRemoteWithOptions { input, .. } => {
                write!(f, "OpenRemoteWithOptions({input:?})")
            }
            Update::OpenRemoteWithBootstrap { input, .. } => {
                write!(f, "OpenRemoteWithBootstrap({input:?})")
            }
            Update::RegexSelectionSubmitted { action, regex } => {
                write!(f, "RegexSelectionSubmitted({action:?}, {regex:?})")
            }
            Update::SelectionChanged { doc_id, view_id } => {
                write!(f, "SelectionChanged(doc: {doc_id:?}, view: {view_id:?})")
            }
            Update::SelectionRestored { doc_id, view_id } => {
                write!(f, "SelectionRestored(doc: {doc_id:?}, view: {view_id:?})")
            }
            Update::ViewFocused { view_id } => write!(f, "ViewFocused({view_id:?})"),
            Update::ViewportScroll { view_id, request } => {
                write!(f, "ViewportScroll(view: {view_id:?}, request: {request:?})")
            }
            Update::ViewportCursor { view_id, request } => {
                write!(f, "ViewportCursor(view: {view_id:?}, request: {request:?})")
            }
            Update::FileTreeEvent(_) => write!(f, "FileTreeEvent(...)"),
            Update::CompletionEvent(_) => write!(f, "CompletionEvent(...)"),
            Update::ShowFilePicker => write!(f, "ShowFilePicker"),
            Update::ShowFilePickerAt(path) => write!(f, "ShowFilePickerAt({path:?})"),
            Update::ShowBufferPicker => write!(f, "ShowBufferPicker"),
            Update::ShowCodeActions => write!(f, "ShowCodeActions"),
            Update::ShowRunnables => write!(f, "ShowRunnables"),
            Update::ShowHoverDocs => write!(f, "ShowHoverDocs"),
            Update::RunTask(task) => write!(f, "RunTask({:?})", task.label()),
            Update::ToggleFileTree => write!(f, "ToggleFileTree"),
            Update::SemanticShortcut(intent) => write!(f, "SemanticShortcut({intent:?})"),
            Update::TerminalPanel(_) => write!(f, "TerminalPanel(...)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexSelectionAction {
    Select,
    Split,
    Keep,
    Remove,
}
