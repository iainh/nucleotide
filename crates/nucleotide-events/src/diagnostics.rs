// ABOUTME: Diagnostics domain events for per-document and workspace-level updates
// ABOUTME: Carries structured diagnostic data and provider info (server/id)

use helix_core::{
    diagnostic::{Diagnostic, DiagnosticProvider},
    Uri,
};
use helix_lsp::LanguageServerId;

/// Diagnostics domain events - immutable facts about diagnostics state changes
#[derive(Debug, Clone)]
pub enum Event {
    /// Full diagnostics set for a document from a specific provider (server/identifier)
    DocumentDiagnosticsSet {
        uri: Uri,
        diagnostics: Vec<Diagnostic>,
        provider: DiagnosticProvider,
    },

    /// Clear diagnostics for a document from a specific provider
    DocumentDiagnosticsCleared {
        uri: Uri,
        provider: DiagnosticProvider,
    },

    /// Clear all diagnostics for a language server (e.g., server exit)
    WorkspaceDiagnosticsClearedForServer {
        server_id: LanguageServerId,
    },

    /// Aggregated workspace summary (optional helper for statusline/indicators)
    WorkspaceDiagnosticsSummary {
        total: usize,
        errors: usize,
        warnings: usize,
        info: usize,
        hints: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_set_event() {
        let uri = Uri::from_path("/tmp/test.rs");
        let provider = DiagnosticProvider::Lsp {
            server_id: LanguageServerId::default(),
            identifier: None,
        };
        let ev = Event::DocumentDiagnosticsSet {
            uri,
            diagnostics: Vec::new(),
            provider,
        };

        match ev {
            Event::DocumentDiagnosticsSet { diagnostics, .. } => assert!(diagnostics.is_empty()),
            _ => panic!("expected DocumentDiagnosticsSet"),
        }
    }
}

