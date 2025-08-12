// ABOUTME: Language Server Protocol events
// ABOUTME: Events for LSP server lifecycle and communication

use helix_view::DocumentId;

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
