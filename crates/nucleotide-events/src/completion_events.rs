// ABOUTME: Event definitions for completion system communication
// ABOUTME: Bridges helix completion system with nucleotide UI components

use nucleotide_types::completion::CompletionTrigger;

/// Event sent when completion is requested
#[derive(Debug, Clone)]
pub struct CompletionRequestEvent {
    /// Type of trigger that initiated this completion
    pub trigger: CompletionTrigger,
    /// Cursor position where completion was requested
    pub cursor_position: usize,
    /// Document ID for the completion request
    pub document_id: String,
    /// View ID for the completion request  
    pub view_id: String,
}

/// Simplified completion item for event passing
#[derive(Debug, Clone)]
pub struct CompletionEventItem {
    /// Display text
    pub text: String,
    /// Item kind
    pub kind: String,
    /// Description/detail text
    pub description: Option<String>,
    /// Documentation
    pub documentation: Option<String>,
}

/// Event sent when completion results are available
#[derive(Debug, Clone)]
pub struct CompletionResponseEvent {
    /// Completion items to display
    pub items: Vec<CompletionEventItem>,
    /// Whether the completion list is incomplete (more results available)
    pub is_incomplete: bool,
    /// Provider that generated these completions
    pub provider: String,
}

/// Event sent when completion is cancelled
#[derive(Debug, Clone)]
pub struct CompletionCancelEvent {
    /// Reason for cancellation
    pub reason: String,
}

/// Event sent when a completion item is selected/accepted
#[derive(Debug, Clone)]
pub struct CompletionAcceptEvent {
    /// The selected completion item text
    pub text: String,
    /// Additional text edits to apply (if any)
    pub additional_edits: Vec<CompletionTextEdit>,
}

/// Text edit to apply when accepting completion
#[derive(Debug, Clone)]
pub struct CompletionTextEdit {
    /// Range to replace (start, end)
    pub range: (usize, usize),
    /// New text to insert
    pub new_text: String,
}

/// Request to get LSP completions from the main thread
#[derive(Debug)]
pub struct LspCompletionRequest {
    /// Cursor position where completion was requested
    pub cursor: usize,
    /// Document ID for the completion request
    pub doc_id: helix_view::DocumentId,
    /// View ID for the completion request  
    pub view_id: helix_view::ViewId,
    /// Response channel to send results back to coordinator
    pub response_tx: tokio::sync::oneshot::Sender<LspCompletionResponse>,
}

/// Response containing LSP completion data
#[derive(Debug, Clone)]
pub struct LspCompletionResponse {
    /// Completion items from LSP
    pub items: Vec<CompletionEventItem>,
    /// Whether this is an incomplete completion list
    pub is_incomplete: bool,
    /// Error message if the request failed
    pub error: Option<String>,
}

/// Results from the completion coordinator to be displayed in the UI
#[derive(Debug, Clone)]
pub enum CompletionResult {
    /// Display these completion items
    ShowCompletions {
        items: Vec<CompletionEventItem>,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    },
    /// Hide any currently displayed completions
    HideCompletions,
    /// An error occurred during completion processing
    Error {
        message: String,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
    },
}
