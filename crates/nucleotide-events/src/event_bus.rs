// ABOUTME: Event bus and handler traits for decoupled communication
// ABOUTME: Provides publish-subscribe pattern for cross-crate events

use crate::{CoreEvent, LspEvent, UiEvent, WorkspaceEvent};

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
