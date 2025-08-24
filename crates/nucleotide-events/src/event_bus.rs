// ABOUTME: Event bus and handler traits for decoupled communication
// ABOUTME: Provides publish-subscribe pattern for cross-crate events

use crate::v2::{
    document::Event as DocumentEvent, editor::Event as EditorEvent, lsp::Event as LspEvent,
    ui::Event as UiEvent, vcs::Event as VcsEvent, workspace::Event as WorkspaceEvent,
};

/// Event bus trait for dispatching events using V2 domain events
pub trait EventBus {
    /// Dispatch a document event
    fn dispatch_document(&self, event: DocumentEvent);

    /// Dispatch an editor event
    fn dispatch_editor(&self, event: EditorEvent);

    /// Dispatch a UI event
    fn dispatch_ui(&self, event: UiEvent);

    /// Dispatch a workspace event
    fn dispatch_workspace(&self, event: WorkspaceEvent);

    /// Dispatch an LSP event
    fn dispatch_lsp(&self, event: LspEvent);

    /// Dispatch a VCS event
    fn dispatch_vcs(&self, event: VcsEvent);
}

/// Event handler trait for receiving V2 domain events
pub trait EventHandler {
    /// Handle a document event
    fn handle_document(&mut self, _event: &DocumentEvent) {}

    /// Handle an editor event
    fn handle_editor(&mut self, _event: &EditorEvent) {}

    /// Handle a UI event
    fn handle_ui(&mut self, _event: &UiEvent) {}

    /// Handle a workspace event
    fn handle_workspace(&mut self, _event: &WorkspaceEvent) {}

    /// Handle an LSP event
    fn handle_lsp(&mut self, _event: &LspEvent) {}

    /// Handle a VCS event
    fn handle_vcs(&mut self, _event: &VcsEvent) {}
}
