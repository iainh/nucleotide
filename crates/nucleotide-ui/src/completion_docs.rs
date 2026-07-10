// ABOUTME: Documentation system for completion items with async loading and markdown rendering
// ABOUTME: Provides cached markdown rendering and side panel documentation display

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Task, div, px,
};
use std::collections::HashMap;
use std::future::Future;
use std::task::Poll;
use std::time::{Duration, Instant};

use crate::completion_v2::CompletionItem;
use crate::markdown::{MarkdownStyle, markdown_extended};
use crate::{StateView, StateViewTone};

/// Documentation content with metadata
#[derive(Debug, Clone)]
pub struct DocumentationContent {
    /// Raw markdown content
    pub markdown: String,
    /// Rendered HTML content (cached)
    pub html: Option<String>,
    /// When this content was fetched
    pub fetched_at: Instant,
    /// Source of the documentation
    pub source: DocumentationSource,
    /// Loading state
    pub state: DocumentationState,
}

/// Source of documentation content
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentationSource {
    /// Inline documentation from completion item
    Inline,
    /// Fetched from language server
    LanguageServer,
    /// Loaded from external documentation
    External,
    /// Generated from type information
    Generated,
}

/// State of documentation loading
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentationState {
    /// Not yet loaded
    NotLoaded,
    /// Currently loading
    Loading,
    /// Successfully loaded
    Loaded,
    /// Failed to load
    Failed(String),
}

impl DocumentationContent {
    pub fn new(markdown: String, source: DocumentationSource) -> Self {
        Self {
            markdown,
            html: None,
            fetched_at: Instant::now(),
            source,
            state: DocumentationState::Loaded,
        }
    }

    pub fn loading(source: DocumentationSource) -> Self {
        Self {
            markdown: String::new(),
            html: None,
            fetched_at: Instant::now(),
            source,
            state: DocumentationState::Loading,
        }
    }

    pub fn failed(error: String, source: DocumentationSource) -> Self {
        Self {
            markdown: String::new(),
            html: None,
            fetched_at: Instant::now(),
            source,
            state: DocumentationState::Failed(error),
        }
    }

    /// Check if this content has expired
    pub fn is_expired(&self, max_age: Duration) -> bool {
        self.fetched_at.elapsed() > max_age
    }

    /// Check if content is available for display
    pub fn is_available(&self) -> bool {
        matches!(self.state, DocumentationState::Loaded) && !self.markdown.is_empty()
    }
}

/// Cache configuration for documentation
#[derive(Debug, Clone)]
pub struct DocumentationCacheConfig {
    /// Maximum number of cached entries
    pub max_entries: usize,
    /// Maximum age for cached content
    pub max_age: Duration,
    /// Whether to cache markdown rendering
    pub cache_rendering: bool,
}

impl Default for DocumentationCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 100,
            max_age: Duration::from_secs(600), // 10 minutes
            cache_rendering: true,
        }
    }
}

/// LRU cache for documentation content
pub struct DocumentationCache {
    cache: HashMap<String, DocumentationContent>,
    access_order: Vec<String>,
    config: DocumentationCacheConfig,
}

impl DocumentationCache {
    pub fn new(config: DocumentationCacheConfig) -> Self {
        Self {
            cache: HashMap::new(),
            access_order: Vec::new(),
            config,
        }
    }

    /// Get documentation content from cache
    pub fn get(&mut self, key: &str) -> Option<DocumentationContent> {
        if let Some(content) = self.cache.get(key) {
            // Check if content has expired
            if content.is_expired(self.config.max_age) {
                self.cache.remove(key);
                self.access_order.retain(|k| k != key);
                return None;
            }

            // Update access order
            self.access_order.retain(|k| k != key);
            self.access_order.push(key.to_string());

            Some(content.clone())
        } else {
            None
        }
    }

    /// Insert documentation content into cache
    pub fn insert(&mut self, key: String, content: DocumentationContent) {
        // Remove existing entry if present
        if self.cache.contains_key(&key) {
            self.access_order.retain(|k| k != &key);
        }

        // Evict old entries if needed
        while self.cache.len() >= self.config.max_entries {
            if let Some(oldest_key) = self.access_order.first().cloned() {
                self.cache.remove(&oldest_key);
                self.access_order.remove(0);
            } else {
                break;
            }
        }

        // Insert new content
        self.cache.insert(key.clone(), content);
        self.access_order.push(key);
    }

    /// Clear expired entries
    pub fn cleanup_expired(&mut self) {
        let expired_keys: Vec<String> = self
            .cache
            .iter()
            .filter(|(_, content)| content.is_expired(self.config.max_age))
            .map(|(key, _)| key.clone())
            .collect();

        for key in expired_keys {
            self.cache.remove(&key);
            self.access_order.retain(|k| k != &key);
        }
    }

    /// Get cache size
    pub fn size(&self) -> usize {
        self.cache.len()
    }
}

/// Async documentation loader
pub struct DocumentationLoader {
    cache: DocumentationCache,
    pending_requests: HashMap<String, Task<DocumentationContent>>,
}

impl DocumentationLoader {
    pub fn new(config: DocumentationCacheConfig) -> Self {
        Self {
            cache: DocumentationCache::new(config),
            pending_requests: HashMap::new(),
        }
    }

    /// Load documentation for a completion item
    pub fn load_documentation<T: 'static>(
        &mut self,
        item: &CompletionItem,
        cx: &mut Context<T>,
    ) -> Option<DocumentationContent> {
        let cache_key = self.generate_cache_key(item);

        // Check cache first
        if let Some(cached) = self.cache.get(&cache_key) {
            return Some(cached);
        }

        // Check if already loading
        if self.pending_requests.contains_key(&cache_key) {
            return Some(DocumentationContent::loading(
                DocumentationSource::LanguageServer,
            ));
        }

        // Check for inline documentation
        if let Some(ref docs) = item.documentation {
            let content = DocumentationContent::new(docs.to_string(), DocumentationSource::Inline);
            self.cache.insert(cache_key, content.clone());
            return Some(content);
        }

        // Start async loading from language server
        self.start_async_loading(cache_key, item, cx);
        Some(DocumentationContent::loading(
            DocumentationSource::LanguageServer,
        ))
    }

    /// Generate cache key for completion item
    fn generate_cache_key(&self, item: &CompletionItem) -> String {
        // Create a unique key based on item content
        format!("{}:{:?}", item.text, item.kind)
    }

    /// Start async loading from external source
    fn start_async_loading<T: 'static>(
        &mut self,
        cache_key: String,
        item: &CompletionItem,
        cx: &mut Context<T>,
    ) {
        let item_text = item.text.clone();
        let item_kind = item.kind;

        let task = cx.spawn(async move |_this, _cx| {
            // Simulate async documentation fetching
            // In a real implementation, this would call the language server
            // Remove the artificial delay since this is just simulation
            // In a real implementation, this would be the actual async LSP call time

            // Generate sample documentation based on item
            let markdown = generate_sample_documentation(&item_text, &item_kind);

            DocumentationContent::new(markdown, DocumentationSource::LanguageServer)
        });

        self.pending_requests.insert(cache_key, task);
    }

    /// Update with completed documentation loading
    pub fn update_completed_requests(&mut self) {
        let completed = self
            .pending_requests
            .iter()
            .filter(|(_, task)| task.is_ready())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();

        for key in completed {
            if let Some(task) = self.pending_requests.remove(&key)
                && let Some(content) = resolve_ready_task(task)
            {
                self.cache.insert(key, content);
            }
        }
    }
}

fn resolve_ready_task<T: 'static>(task: Task<T>) -> Option<T> {
    if !task.is_ready() {
        return None;
    }

    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    let mut task = std::pin::pin!(task);

    match task.as_mut().poll(&mut cx) {
        Poll::Ready(value) => Some(value),
        Poll::Pending => None,
    }
}

/// Generate sample documentation for testing
fn generate_sample_documentation(
    item_text: &str,
    item_kind: &Option<crate::completion_v2::CompletionItemKind>,
) -> String {
    use crate::completion_v2::CompletionItemKind;

    match item_kind {
        Some(CompletionItemKind::Function) => {
            format!(
                r#"# Function: `{}`

A function that performs a specific operation.

## Syntax
```rust
fn {}() -> ReturnType
```

## Description
This function implements core functionality for the application.

## Parameters
- None currently documented

## Returns
Returns a value of the appropriate type.

## Examples
```rust
let result = {}();
```

## See Also
- Related functions
- Documentation links
"#,
                item_text, item_text, item_text
            )
        }
        Some(CompletionItemKind::Variable) => {
            format!(
                r#"# Variable: `{}`

A variable that stores application state.

## Type
```rust
let {}: SomeType;
```

## Description
This variable holds important data for the application.
"#,
                item_text, item_text
            )
        }
        Some(CompletionItemKind::Class) => {
            format!(
                r#"# Class: `{}`

A class that encapsulates related functionality.

## Definition
```rust
struct {} {{
    // fields
}}
```

## Description
This class provides a structured way to organize data and behavior.

## Methods
- `new()` - Constructor
- `method()` - Example method

## Usage
```rust
let instance = {}::new();
```
"#,
                item_text, item_text, item_text
            )
        }
        _ => {
            format!(
                r#"# Documentation: `{}`

Documentation for this item.

## Description
This is a completion item in the code editor.

## Usage
Use this item in your code as appropriate.
"#,
                item_text
            )
        }
    }
}

/// Documentation panel component
#[derive(Clone)]
pub struct DocumentationPanel {
    content: Option<DocumentationContent>,
    visible: bool,
    width: f32,
    scroll_position: f32,
}

impl Default for DocumentationPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentationPanel {
    pub fn new() -> Self {
        Self {
            content: None,
            visible: false,
            width: 300.0,
            scroll_position: 0.0,
        }
    }

    pub fn set_content(&mut self, content: Option<DocumentationContent>) {
        self.content = content;
        // Reset scroll position when content changes
        self.scroll_position = 0.0;
        // Mark for UI refresh to apply scroll reset
        // In a real implementation, this would trigger container scroll reset
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn set_width(&mut self, width: f32) {
        self.width = width;
    }
}

impl Render for DocumentationPanel {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().id("hidden-doc-panel");
        }

        let theme = cx.global::<crate::Theme>();
        let tokens = &theme.tokens;

        div()
            .id("documentation-panel")
            .flex()
            .flex_col()
            .w(px(self.width))
            .h_full()
            .bg(tokens.chrome.surface_elevated)
            .border_l_1()
            .border_color(tokens.chrome.border_default)
            .child(
                // Header
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(tokens.chrome.border_muted)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(tokens.chrome.text_on_chrome)
                            .child("Documentation"),
                    ),
            )
            .child(
                // Content area
                div()
                    .id("documentation-panel-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_3()
                    .py_2()
                    .child(self.render_content(tokens)),
            )
    }
}

impl IntoElement for DocumentationPanel {
    type Element = gpui::AnyElement;

    fn into_element(self) -> Self::Element {
        self.into_any_element()
    }
}

impl DocumentationPanel {
    fn render_content(&mut self, tokens: &crate::DesignTokens) -> gpui::AnyElement {
        match &self.content {
            Some(content) => match &content.state {
                DocumentationState::Loading => {
                    StateView::new("completion-documentation-loading", "Loading documentation")
                        .loading(true)
                        .compact(true)
                        .into_any_element()
                }
                DocumentationState::Failed(error) => StateView::new(
                    "completion-documentation-failed",
                    "Documentation unavailable",
                )
                .detail(error.clone())
                .icon("icons/triangle-alert.svg")
                .tone(StateViewTone::Error)
                .compact(true)
                .into_any_element(),
                DocumentationState::Loaded => {
                    if content.markdown.is_empty() {
                        StateView::new(
                            "completion-documentation-empty",
                            "No documentation available",
                        )
                        .icon("icons/book-text.svg")
                        .compact(true)
                        .into_any_element()
                    } else {
                        div()
                            .child(markdown_extended(
                                content.markdown.clone(),
                                MarkdownStyle::from_tokens(tokens),
                            ))
                            .into_any_element()
                    }
                }
                DocumentationState::NotLoaded => StateView::new(
                    "completion-documentation-idle",
                    "Select an item to view documentation",
                )
                .icon("icons/book-text.svg")
                .compact(true)
                .into_any_element(),
            },
            None => StateView::new("completion-documentation-none", "No item selected")
                .icon("icons/book-text.svg")
                .compact(true)
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_documentation_content_creation() {
        let content = DocumentationContent::new(
            "# Test\nSome content".to_string(),
            DocumentationSource::Inline,
        );

        assert_eq!(content.markdown, "# Test\nSome content");
        assert_eq!(content.source, DocumentationSource::Inline);
        assert_eq!(content.state, DocumentationState::Loaded);
        assert!(content.is_available());
    }

    #[test]
    fn test_documentation_content_states() {
        let loading = DocumentationContent::loading(DocumentationSource::LanguageServer);
        assert_eq!(loading.state, DocumentationState::Loading);
        assert!(!loading.is_available());

        let failed = DocumentationContent::failed(
            "Network error".to_string(),
            DocumentationSource::External,
        );
        assert!(matches!(failed.state, DocumentationState::Failed(_)));
        assert!(!failed.is_available());
    }

    #[test]
    fn test_documentation_cache() {
        let config = DocumentationCacheConfig::default();
        let mut cache = DocumentationCache::new(config);

        assert_eq!(cache.size(), 0);

        let content =
            DocumentationContent::new("Test content".to_string(), DocumentationSource::Inline);

        cache.insert("key1".to_string(), content.clone());
        assert_eq!(cache.size(), 1);

        let retrieved = cache.get("key1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().markdown, "Test content");

        // Test cache miss
        let missing = cache.get("nonexistent");
        assert!(missing.is_none());
    }

    #[test]
    fn test_documentation_cache_lru_eviction() {
        let config = DocumentationCacheConfig {
            max_entries: 2,
            ..Default::default()
        };
        let mut cache = DocumentationCache::new(config);

        let content1 =
            DocumentationContent::new("Content 1".to_string(), DocumentationSource::Inline);
        let content2 =
            DocumentationContent::new("Content 2".to_string(), DocumentationSource::Inline);
        let content3 =
            DocumentationContent::new("Content 3".to_string(), DocumentationSource::Inline);

        cache.insert("key1".to_string(), content1);
        cache.insert("key2".to_string(), content2);
        assert_eq!(cache.size(), 2);

        // Access key1 to make it more recently used
        cache.get("key1");

        // Insert key3, should evict key2 (least recently used)
        cache.insert("key3".to_string(), content3);
        assert_eq!(cache.size(), 2);

        assert!(cache.get("key1").is_some());
        assert!(cache.get("key2").is_none()); // Should be evicted
        assert!(cache.get("key3").is_some());
    }

    #[test]
    fn test_documentation_loader_caches_completed_requests() {
        let mut loader = DocumentationLoader::new(DocumentationCacheConfig::default());
        let content = DocumentationContent::new(
            "Loaded documentation".to_string(),
            DocumentationSource::LanguageServer,
        );

        loader
            .pending_requests
            .insert("item:function".to_string(), Task::ready(content));

        loader.update_completed_requests();

        assert!(loader.pending_requests.is_empty());
        let cached = loader.cache.get("item:function").unwrap();
        assert_eq!(cached.markdown, "Loaded documentation");
        assert_eq!(cached.source, DocumentationSource::LanguageServer);
    }

    #[test]
    fn test_generate_sample_documentation() {
        use crate::completion_v2::CompletionItemKind;

        let function_doc =
            generate_sample_documentation("my_function", &Some(CompletionItemKind::Function));
        assert!(function_doc.contains("# Function: `my_function`"));
        assert!(function_doc.contains("```rust"));

        let variable_doc =
            generate_sample_documentation("my_var", &Some(CompletionItemKind::Variable));
        assert!(variable_doc.contains("# Variable: `my_var`"));
    }

    #[test]
    fn test_documentation_panel_creation() {
        let panel = DocumentationPanel::new();
        assert!(!panel.visible);
        assert_eq!(panel.width, 300.0);
        assert!(panel.content.is_none());
    }
}
