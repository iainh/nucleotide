// ABOUTME: Data-only application events delivered through GPUI updates.

use nucleotide_events::v2::{
    document::Event as DocumentEvent, ui::Event as UiEvent, workspace::Event as WorkspaceEvent,
};

#[derive(Debug, Clone)]
pub enum AppEvent {
    Document(DocumentEvent),
    Ui(UiEvent),
    Workspace(WorkspaceEvent),
}
