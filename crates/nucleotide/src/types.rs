// ABOUTME: Application-specific type re-exports
// ABOUTME: Re-exports shared types from nucleotide-core

// Re-export shared types from core
pub use nucleotide_core::shared_types::{
    CompletionTrigger, EditorFontConfig, EditorStatus, FontSettings, UiFontConfig,
};

// Re-export event types
pub use nucleotide_core::events::{
    AppEvent, CoreEvent, LspEvent, MessageSeverity, PanelType, PickerType, SplitDirection, UiEvent,
    WorkspaceEvent,
};

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
    Completion(gpui::Entity<crate::completion::CompletionView>),
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
    OpenFile(std::path::PathBuf),
    OpenDirectory(std::path::PathBuf),
    DocumentChanged {
        doc_id: helix_view::DocumentId,
    },
    SelectionChanged {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    },
    ModeChanged {
        old_mode: helix_view::document::Mode,
        new_mode: helix_view::document::Mode,
    },
    DiagnosticsChanged {
        doc_id: helix_view::DocumentId,
    },
    DocumentOpened {
        doc_id: helix_view::DocumentId,
    },
    DocumentClosed {
        doc_id: helix_view::DocumentId,
    },
    ViewFocused {
        view_id: helix_view::ViewId,
    },
    LanguageServerInitialized {
        server_id: helix_lsp::LanguageServerId,
    },
    LanguageServerExited {
        server_id: helix_lsp::LanguageServerId,
    },
    CompletionRequested {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: CompletionTrigger,
    },
    ShowFilePicker,
    ShowBufferPicker,
}

impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Update::Event(event) => write!(f, "Event({:?})", event),
            Update::Prompt(_) => write!(f, "Prompt(...)"),
            Update::Picker(_) => write!(f, "Picker(...)"),
            Update::DirectoryPicker(_) => write!(f, "DirectoryPicker(...)"),
            Update::Completion(_) => write!(f, "Completion(...)"),
            Update::Info(_) => write!(f, "Info(...)"),
            Update::EditorEvent(_) => write!(f, "EditorEvent(...)"),
            Update::EditorStatus(status) => write!(f, "EditorStatus({:?})", status),
            Update::Redraw => write!(f, "Redraw"),
            Update::OpenFile(path) => write!(f, "OpenFile({:?})", path),
            Update::OpenDirectory(path) => write!(f, "OpenDirectory({:?})", path),
            Update::ShouldQuit => write!(f, "ShouldQuit"),
            Update::CommandSubmitted(cmd) => write!(f, "CommandSubmitted({:?})", cmd),
            Update::SearchSubmitted(query) => write!(f, "SearchSubmitted({:?})", query),
            Update::DocumentChanged { doc_id } => write!(f, "DocumentChanged({:?})", doc_id),
            Update::SelectionChanged { doc_id, view_id } => {
                write!(
                    f,
                    "SelectionChanged(doc: {:?}, view: {:?})",
                    doc_id, view_id
                )
            }
            Update::ModeChanged { old_mode, new_mode } => {
                write!(f, "ModeChanged({:?} -> {:?})", old_mode, new_mode)
            }
            Update::DiagnosticsChanged { doc_id } => write!(f, "DiagnosticsChanged({:?})", doc_id),
            Update::DocumentOpened { doc_id } => write!(f, "DocumentOpened({:?})", doc_id),
            Update::DocumentClosed { doc_id } => write!(f, "DocumentClosed({:?})", doc_id),
            Update::ViewFocused { view_id } => write!(f, "ViewFocused({:?})", view_id),
            Update::LanguageServerInitialized { server_id } => {
                write!(f, "LanguageServerInitialized({:?})", server_id)
            }
            Update::LanguageServerExited { server_id } => {
                write!(f, "LanguageServerExited({:?})", server_id)
            }
            Update::CompletionRequested {
                doc_id,
                view_id,
                trigger,
            } => {
                write!(
                    f,
                    "CompletionRequested(doc: {:?}, view: {:?}, trigger: {:?})",
                    doc_id, view_id, trigger
                )
            }
            Update::FileTreeEvent(_) => write!(f, "FileTreeEvent(...)"),
            Update::ShowFilePicker => write!(f, "ShowFilePicker"),
            Update::ShowBufferPicker => write!(f, "ShowBufferPicker"),
        }
    }
}
