// ABOUTME: Shared completion items and metadata for application and UI consumers
// ABOUTME: Language-server edit payloads use the negotiated offset encoding

/// Completion item details
#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionItemKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: String,
    pub score: f32,
    pub signature_info: Option<String>,
    pub type_info: Option<String>,
    pub insert_text_format: InsertTextFormat,
    pub edit: Option<CompletionEdit>,
    pub sort_text: Option<String>,
    pub filter_text: Option<String>,
    pub preselect: bool,
    pub commit_characters: Vec<String>,
    pub tags: Vec<CompletionItemTag>,
    pub data: Option<serde_json::Value>,
    pub source_index: usize,
    pub server_id: Option<u64>,
    pub raw_lsp_item: Option<serde_json::Value>,
}

/// LSP-style edit metadata for accepting a completion item.
///
/// Ranges use the language server's negotiated offset encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEdit {
    pub offset_encoding: CompletionOffsetEncoding,
    pub text_edit: Option<CompletionTextEdit>,
    pub additional_text_edits: Vec<CompletionTextEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionTextEdit {
    pub range: CompletionRange,
    pub new_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionRange {
    pub start: CompletionPosition,
    pub end: CompletionPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionOffsetEncoding {
    Utf8,
    Utf16,
    Utf32,
}

/// LSP Insert Text Format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertTextFormat {
    /// Plain text insertion
    PlainText,
    /// Snippet with tabstops and placeholders ($0, $1, ${1:placeholder})
    Snippet,
}

/// Types of completion items
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionItemTag {
    Deprecated,
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
            signature_info: None,
            type_info: None,
            insert_text_format: InsertTextFormat::PlainText,
            edit: None,
            sort_text: None,
            filter_text: None,
            preselect: false,
            commit_characters: Vec::new(),
            tags: Vec::new(),
            data: None,
            source_index: 0,
            server_id: None,
            raw_lsp_item: None,
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

    pub fn with_signature_info(mut self, signature_info: String) -> Self {
        self.signature_info = Some(signature_info);
        self
    }

    pub fn with_type_info(mut self, type_info: String) -> Self {
        self.type_info = Some(type_info);
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

    pub fn with_insert_text_format(mut self, format: InsertTextFormat) -> Self {
        self.insert_text_format = format;
        self
    }

    pub fn with_edit(mut self, edit: CompletionEdit) -> Self {
        self.edit = Some(edit);
        self
    }

    pub fn with_optional_edit(mut self, edit: Option<CompletionEdit>) -> Self {
        self.edit = edit;
        self
    }

    pub fn with_sort_text(mut self, sort_text: Option<String>) -> Self {
        self.sort_text = sort_text;
        self
    }

    pub fn with_filter_text(mut self, filter_text: Option<String>) -> Self {
        self.filter_text = filter_text;
        self
    }

    pub fn with_preselect(mut self, preselect: bool) -> Self {
        self.preselect = preselect;
        self
    }

    pub fn with_commit_characters(mut self, commit_characters: Vec<String>) -> Self {
        self.commit_characters = commit_characters;
        self
    }

    pub fn with_tags(mut self, tags: Vec<CompletionItemTag>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_data(mut self, data: Option<serde_json::Value>) -> Self {
        self.data = data;
        self
    }

    pub fn with_source_index(mut self, source_index: usize) -> Self {
        self.source_index = source_index;
        self
    }

    pub fn with_server_id(mut self, server_id: Option<u64>) -> Self {
        self.server_id = server_id;
        self
    }

    pub fn with_raw_lsp_item(mut self, raw_lsp_item: Option<serde_json::Value>) -> Self {
        self.raw_lsp_item = raw_lsp_item;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_item_builder() {
        let item = CompletionItem::new("test_function".to_string(), CompletionItemKind::Function)
            .with_detail("fn test_function() -> bool".to_string())
            .with_documentation("A test function that returns a boolean".to_string())
            .with_score(0.95)
            .with_sort_text(Some("001".to_string()))
            .with_filter_text(Some("test_function".to_string()))
            .with_preselect(true)
            .with_commit_characters(vec!["(".to_string()])
            .with_tags(vec![CompletionItemTag::Deprecated])
            .with_data(Some(serde_json::json!({"server": "rust-analyzer"})))
            .with_source_index(3)
            .with_server_id(Some(42))
            .with_raw_lsp_item(Some(serde_json::json!({
                "label": "test_function",
                "data": { "server": "rust-analyzer" }
            })));

        assert_eq!(item.label, "test_function");
        assert_eq!(item.kind, CompletionItemKind::Function);
        assert!(item.detail.is_some());
        assert!(item.documentation.is_some());
        assert_eq!(item.score, 0.95);
        assert_eq!(item.sort_text.as_deref(), Some("001"));
        assert_eq!(item.filter_text.as_deref(), Some("test_function"));
        assert!(item.preselect);
        assert_eq!(item.commit_characters, vec!["("]);
        assert_eq!(item.tags, vec![CompletionItemTag::Deprecated]);
        assert_eq!(
            item.data,
            Some(serde_json::json!({"server": "rust-analyzer"}))
        );
        assert_eq!(item.source_index, 3);
        assert_eq!(item.server_id, Some(42));
        assert_eq!(
            item.raw_lsp_item,
            Some(serde_json::json!({
                "label": "test_function",
                "data": { "server": "rust-analyzer" }
            }))
        );
    }
}
