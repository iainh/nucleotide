// ABOUTME: Completion domain events for lifecycle and user interactions
// ABOUTME: Complete rewrite with proper lifecycle management

use helix_view::{DocumentId, ViewId};

/// Completion domain events - covers completion lifecycle, item selection, and display coordination  
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// Completion request lifecycle
    Requested {
        doc_id: DocumentId,
        view_id: ViewId,
        trigger: CompletionTrigger,
        cursor_position: Position,
        request_id: CompletionRequestId,
    },

    Cancelled {
        request_id: CompletionRequestId,
        reason: CancellationReason,
    },

    /// Results available
    ResultsAvailable {
        request_id: CompletionRequestId,
        items: Vec<CompletionItem>,
        is_incomplete: bool,
        provider: CompletionProvider,
        latency_ms: u64,
    },

    /// User interaction
    ItemSelected {
        request_id: CompletionRequestId,
        item_index: usize,
        selection_method: SelectionMethod,
    },

    ItemAccepted {
        item: CompletionItem,
        doc_id: DocumentId,
        view_id: ViewId,
        insert_position: Position,
    },

    /// Display management
    MenuShown {
        request_id: CompletionRequestId,
        item_count: usize,
        position: MenuPosition,
    },

    MenuHidden {
        request_id: CompletionRequestId,
        was_accepted: bool,
    },

    /// Filtering and ranking
    FilteringCompleted {
        request_id: CompletionRequestId,
        original_count: usize,
        filtered_count: usize,
        filter_text: String,
    },

    /// Error handling
    RequestFailed {
        request_id: CompletionRequestId,
        error: CompletionError,
    },

    /// Performance tracking
    PerformanceMetrics {
        request_id: CompletionRequestId,
        metrics: CompletionMetrics,
    },
}

/// Completion trigger types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionTrigger {
    Manual,
    Character(char),
    TriggerSequence(String),
    Automatic,
}

/// How an item was selected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMethod {
    Keyboard,
    Mouse,
    Tab,
    Enter,
}

/// Position in document
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// Unique identifier for completion requests
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompletionRequestId(pub u64);

/// Completion item details
#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionItemKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: String,
    pub score: f32,
}

/// Types of completion items
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionItemKind {
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

/// Source of completion results
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionProvider {
    LSP,
    Buffer,
    Snippet,
    Path,
}

/// Reason for cancellation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationReason {
    UserCancelled,
    NewRequest,
    DocumentClosed,
    Timeout,
    Error,
}

/// Position for completion menu
#[derive(Debug, Clone, Copy)]
pub struct MenuPosition {
    pub x: f32,
    pub y: f32,
    pub anchor: PositionAnchor,
}

/// Anchor point for menu positioning
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionAnchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Cursor,
}

/// Completion error information
#[derive(Debug, Clone)]
pub struct CompletionError {
    pub message: String,
    pub code: Option<i32>,
    pub recoverable: bool,
}

/// Performance metrics for completion
#[derive(Debug, Clone)]
pub struct CompletionMetrics {
    pub request_duration_ms: u64,
    pub filter_duration_ms: u64,
    pub render_duration_ms: u64,
    pub total_items: usize,
    pub visible_items: usize,
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

impl CompletionRequestId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl CompletionItem {
    pub fn new(label: String, kind: CompletionItemKind) -> Self {
        Self {
            insert_text: label.clone(),
            label,
            kind,
            detail: None,
            documentation: None,
            score: 0.0,
        }
    }

    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn with_documentation(mut self, documentation: String) -> Self {
        self.documentation = Some(documentation);
        self
    }

    pub fn with_insert_text(mut self, insert_text: String) -> Self {
        self.insert_text = insert_text;
        self
    }

    pub fn with_score(mut self, score: f32) -> Self {
        self.score = score;
        self
    }
}

impl MenuPosition {
    pub fn new(x: f32, y: f32, anchor: PositionAnchor) -> Self {
        Self { x, y, anchor }
    }

    pub fn at_cursor(x: f32, y: f32) -> Self {
        Self::new(x, y, PositionAnchor::Cursor)
    }
}

impl CompletionError {
    pub fn new(message: String) -> Self {
        Self {
            message,
            code: None,
            recoverable: true,
        }
    }

    pub fn with_code(mut self, code: i32) -> Self {
        self.code = Some(code);
        self
    }

    pub fn unrecoverable(mut self) -> Self {
        self.recoverable = false;
        self
    }
}

impl Default for CompletionMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionMetrics {
    pub fn new() -> Self {
        Self {
            request_duration_ms: 0,
            filter_duration_ms: 0,
            render_duration_ms: 0,
            total_items: 0,
            visible_items: 0,
        }
    }

    pub fn total_duration_ms(&self) -> u64 {
        self.request_duration_ms + self.filter_duration_ms + self.render_duration_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_request_lifecycle() {
        let request_id = CompletionRequestId::new(1);

        let request_event = Event::Requested {
            doc_id: DocumentId::default(),
            view_id: ViewId::default(),
            trigger: CompletionTrigger::Manual,
            cursor_position: Position::new(0, 10),
            request_id,
        };

        match request_event {
            Event::Requested {
                request_id: id,
                trigger,
                ..
            } => {
                assert_eq!(id, request_id);
                assert_eq!(trigger, CompletionTrigger::Manual);
            }
            _ => panic!("Expected Requested event"),
        }
    }

    #[test]
    fn test_completion_item_builder() {
        let item = CompletionItem::new("test_function".to_string(), CompletionItemKind::Function)
            .with_detail("fn test_function() -> bool".to_string())
            .with_documentation("A test function that returns a boolean".to_string())
            .with_score(0.95);

        assert_eq!(item.label, "test_function");
        assert_eq!(item.kind, CompletionItemKind::Function);
        assert!(item.detail.is_some());
        assert!(item.documentation.is_some());
        assert_eq!(item.score, 0.95);
    }

    #[test]
    fn test_completion_triggers() {
        let triggers = [
            CompletionTrigger::Manual,
            CompletionTrigger::Character('.'),
            CompletionTrigger::TriggerSequence("::".to_string()),
            CompletionTrigger::Automatic,
        ];

        for trigger in triggers {
            let _event = Event::Requested {
                doc_id: DocumentId::default(),
                view_id: ViewId::default(),
                trigger,
                cursor_position: Position::new(0, 0),
                request_id: CompletionRequestId::new(1),
            };
        }
    }

    #[test]
    fn test_completion_item_kinds() {
        let kinds = [
            CompletionItemKind::Text,
            CompletionItemKind::Method,
            CompletionItemKind::Function,
            CompletionItemKind::Constructor,
            CompletionItemKind::Field,
            CompletionItemKind::Variable,
            CompletionItemKind::Class,
            CompletionItemKind::Interface,
            CompletionItemKind::Module,
            CompletionItemKind::Property,
            CompletionItemKind::Unit,
            CompletionItemKind::Value,
            CompletionItemKind::Enum,
            CompletionItemKind::Keyword,
            CompletionItemKind::Snippet,
            CompletionItemKind::Color,
            CompletionItemKind::File,
            CompletionItemKind::Reference,
            CompletionItemKind::Folder,
        ];

        for kind in kinds {
            let _item = CompletionItem::new("test".to_string(), kind);
        }
    }

    #[test]
    fn test_menu_position() {
        let position = MenuPosition::at_cursor(100.0, 200.0);
        assert_eq!(position.x, 100.0);
        assert_eq!(position.y, 200.0);
        assert_eq!(position.anchor, PositionAnchor::Cursor);
    }

    #[test]
    fn test_completion_error() {
        let error = CompletionError::new("LSP server unavailable".to_string())
            .with_code(-32001)
            .unrecoverable();

        assert_eq!(error.message, "LSP server unavailable");
        assert_eq!(error.code, Some(-32001));
        assert!(!error.recoverable);
    }

    #[test]
    fn test_completion_metrics() {
        let mut metrics = CompletionMetrics::new();
        metrics.request_duration_ms = 100;
        metrics.filter_duration_ms = 50;
        metrics.render_duration_ms = 25;

        assert_eq!(metrics.total_duration_ms(), 175);
    }

    #[test]
    fn test_selection_methods() {
        let methods = [
            SelectionMethod::Keyboard,
            SelectionMethod::Mouse,
            SelectionMethod::Tab,
            SelectionMethod::Enter,
        ];

        for method in methods {
            let _event = Event::ItemSelected {
                request_id: CompletionRequestId::new(1),
                item_index: 0,
                selection_method: method,
            };
        }
    }

    #[test]
    fn test_cancellation_reasons() {
        let reasons = [
            CancellationReason::UserCancelled,
            CancellationReason::NewRequest,
            CancellationReason::DocumentClosed,
            CancellationReason::Timeout,
            CancellationReason::Error,
        ];

        for reason in reasons {
            let _event = Event::Cancelled {
                request_id: CompletionRequestId::new(1),
                reason,
            };
        }
    }
}
