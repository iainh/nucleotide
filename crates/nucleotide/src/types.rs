// ABOUTME: Application-specific type re-exports
// ABOUTME: Re-exports shared types from nucleotide-core

// Re-export shared types from core (now from nucleotide-types via nucleotide-core)
pub use nucleotide_core::{
    CompletionTrigger, EditorFontConfig, EditorStatus, FontSettings, Severity, UiFontConfig,
};

// Re-export domain event types
pub use nucleotide_core::{DocumentEvent, UiEvent, WorkspaceEvent};
pub use nucleotide_events::LspEvent;

// Re-export UI enums from domain events
pub use nucleotide_events::ui::SystemAppearance;

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

/// The single application-level event model emitted by `Application` and its UI components.
pub enum Update {
    // Domain events
    Document(DocumentEvent),
    Lsp(LspEvent),
    Ui(UiEvent),
    Workspace(WorkspaceEvent),

    // Complex UI components with behavior (closures/callbacks)
    Prompt(crate::prompt::Prompt),
    Picker(crate::picker::Picker),
    DirectoryPicker(crate::picker::Picker),
    RemoteConnectionManager,
    Completion(gpui::Entity<nucleotide_ui::completion_v2::CompletionView>),
    HoverDocs(Vec<HoverDocEntry>),
    CompletionEvent(helix_view::handlers::completion::CompletionEvent),
    Info(helix_view::info::Info),

    // Application UI events
    EditorConfigChanged(helix_view::editor::ConfigEvent),
    EditorStatus(EditorStatus),
    Redraw,
    ShouldQuit,
    CommandSubmitted(String),
    SearchSubmitted(String),
    GlobalSearchSubmitted(String),
    FileTreeSearchSubmitted(String),
    OpenFile(std::path::PathBuf),
    OpenDirectory(std::path::PathBuf),
    OpenRemote(String),
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
    RunTask(nucleotide_events::run::ResolvedTask),
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
    ToggleTerminal,
}

impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Update::Document(event) => write!(f, "Document({event:?})"),
            Update::Lsp(event) => write!(f, "Lsp({event:?})"),
            Update::Ui(event) => write!(f, "Ui({event:?})"),
            Update::Workspace(event) => write!(f, "Workspace({event:?})"),
            Update::Prompt(_) => write!(f, "Prompt(...)"),
            Update::Picker(_) => write!(f, "Picker(...)"),
            Update::DirectoryPicker(_) => write!(f, "DirectoryPicker(...)"),
            Update::RemoteConnectionManager => write!(f, "RemoteConnectionManager"),
            Update::Completion(_) => write!(f, "Completion(...)"),
            Update::Info(_) => write!(f, "Info(...)"),
            Update::HoverDocs(entries) => write!(f, "HoverDocs(len={})", entries.len()),
            Update::EditorConfigChanged(_) => write!(f, "EditorConfigChanged(...)"),
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
            Update::OpenRemoteWithBootstrap { input, .. } => {
                write!(f, "OpenRemoteWithBootstrap({input:?})")
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
            Update::CompletionEvent(_) => write!(f, "CompletionEvent(...)"),
            Update::ShowFilePicker => write!(f, "ShowFilePicker"),
            Update::ShowFilePickerAt(path) => {
                write!(f, "ShowFilePickerAt({path:?})")
            }
            Update::ShowBufferPicker => write!(f, "ShowBufferPicker"),
            Update::ShowCodeActions => write!(f, "ShowCodeActions"),
            Update::ShowRunnables => write!(f, "ShowRunnables"),
            Update::ShowHoverDocs => write!(f, "ShowHoverDocs"),
            Update::RunTask(task) => write!(f, "RunTask({:?})", task.label()),
            Update::ToggleFileTree => write!(f, "ToggleFileTree"),
            Update::SemanticShortcut(intent) => {
                write!(f, "SemanticShortcut({intent:?})")
            }
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
