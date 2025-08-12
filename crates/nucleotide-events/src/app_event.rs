// ABOUTME: Aggregated event type for the main application
// ABOUTME: Top-level event dispatcher and router

use crate::{CoreEvent, LspEvent, UiEvent, WorkspaceEvent};

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
