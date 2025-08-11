// ABOUTME: Shared types used across the application
// ABOUTME: Provides common enums and structs for inter-module communication

use crate::{completion, file_tree, picker, prompt};
use gpui::Entity;
use helix_core::diagnostic::Severity;
use helix_lsp::LanguageServerId;
use nucleotide_core::CompletionTrigger;

/// Editor status information
#[derive(Clone, Debug)]
pub struct EditorStatus {
    pub status: String,
    pub severity: Severity,
}

/// Update events that can be sent between components
pub enum Update {
    Redraw,
    Prompt(prompt::Prompt),
    Picker(picker::Picker),
    DirectoryPicker(picker::Picker),
    Completion(Entity<completion::CompletionView>),
    Info(helix_view::info::Info),
    EditorEvent(helix_view::editor::EditorEvent),
    EditorStatus(EditorStatus),
    OpenFile(std::path::PathBuf),
    OpenDirectory(std::path::PathBuf),
    ShouldQuit,
    CommandSubmitted(String),
    SearchSubmitted(String),
    // Helix event bridge - these allow UI to respond to Helix events
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
    // Additional granular events for better UI responsiveness
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
        server_id: LanguageServerId,
        server_name: String,
    },
    LanguageServerExited {
        server_id: LanguageServerId,
    },
    CompletionRequested {
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: CompletionTrigger,
    },
    // File tree events
    FileTreeEvent(file_tree::FileTreeEvent),
    // Picker request events - emitted when helix wants to show a picker
    ShowFilePicker,
    ShowBufferPicker,
}

// Manual Debug implementation to avoid proc macro issues with Entity<T>
impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Update::Redraw => write!(f, "Update::Redraw"),
            Update::Prompt(_) => write!(f, "Update::Prompt"),
            Update::Picker(_) => write!(f, "Update::Picker"),
            Update::DirectoryPicker(_) => write!(f, "Update::DirectoryPicker"),
            Update::Completion(_) => write!(f, "Update::Completion"),
            Update::Info(info) => write!(f, "Update::Info({:?})", info),
            Update::EditorEvent(event) => write!(f, "Update::EditorEvent({:?})", event),
            Update::EditorStatus(status) => write!(f, "Update::EditorStatus({:?})", status),
            Update::OpenFile(path) => write!(f, "Update::OpenFile({:?})", path),
            Update::OpenDirectory(path) => write!(f, "Update::OpenDirectory({:?})", path),
            Update::ShouldQuit => write!(f, "Update::ShouldQuit"),
            Update::CommandSubmitted(cmd) => write!(f, "Update::CommandSubmitted({:?})", cmd),
            Update::SearchSubmitted(search) => write!(f, "Update::SearchSubmitted({:?})", search),
            Update::DocumentChanged { doc_id } => {
                write!(f, "Update::DocumentChanged({:?})", doc_id)
            }
            Update::SelectionChanged { doc_id, view_id } => {
                write!(f, "Update::SelectionChanged({:?}, {:?})", doc_id, view_id)
            }
            Update::ModeChanged { old_mode, new_mode } => {
                write!(f, "Update::ModeChanged({:?} -> {:?})", old_mode, new_mode)
            }
            Update::DiagnosticsChanged { doc_id } => {
                write!(f, "Update::DiagnosticsChanged({:?})", doc_id)
            }
            Update::DocumentOpened { doc_id } => write!(f, "Update::DocumentOpened({:?})", doc_id),
            Update::DocumentClosed { doc_id } => write!(f, "Update::DocumentClosed({:?})", doc_id),
            Update::ViewFocused { view_id } => write!(f, "Update::ViewFocused({:?})", view_id),
            Update::LanguageServerInitialized {
                server_id,
                server_name,
            } => {
                write!(
                    f,
                    "Update::LanguageServerInitialized({:?}, {:?})",
                    server_id, server_name
                )
            }
            Update::LanguageServerExited { server_id } => {
                write!(f, "Update::LanguageServerExited({:?})", server_id)
            }
            Update::CompletionRequested {
                doc_id,
                view_id,
                trigger,
            } => {
                write!(
                    f,
                    "Update::CompletionRequested({:?}, {:?}, {:?})",
                    doc_id, view_id, trigger
                )
            }
            Update::FileTreeEvent(event) => write!(f, "Update::FileTreeEvent({:?})", event),
            Update::ShowFilePicker => write!(f, "Update::ShowFilePicker"),
            Update::ShowBufferPicker => write!(f, "Update::ShowBufferPicker"),
        }
    }
}
