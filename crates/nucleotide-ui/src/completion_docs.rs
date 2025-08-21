// ABOUTME: Documentation system for completion items with async loading and markdown rendering
// ABOUTME: Provides cached markdown rendering and side panel documentation display

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Task, div, px,
    relative,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::completion_v2::CompletionItem;

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
        let item_kind = item.kind.clone();

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
        let completed = Vec::new();

        // TODO: Check if tasks are completed when API is available
        // For now, just assume all requests complete immediately

        for key in completed {
            if let Some(_task) = self.pending_requests.remove(&key) {
                // In a real implementation, we'd get the result and cache it
                // For now, we'll just simulate successful completion
                let content = DocumentationContent::new(
                    "Documentation loaded successfully".to_string(),
                    DocumentationSource::LanguageServer,
                );
                self.cache.insert(key, content);
            }
        }
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

/// Simple markdown renderer (basic implementation)
#[derive(Clone)]
pub struct MarkdownRenderer {
    cache: HashMap<String, String>,
}

impl MarkdownRenderer {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Render markdown to HTML (simplified implementation)
    pub fn render(&mut self, markdown: &str) -> String {
        if let Some(cached) = self.cache.get(markdown) {
            return cached.clone();
        }

        let html = self.markdown_to_html(markdown);
        self.cache.insert(markdown.to_string(), html.clone());
        html
    }

    /// Convert markdown to HTML (basic implementation)
    fn markdown_to_html(&self, markdown: &str) -> String {
        let mut html = String::new();
        let lines: Vec<&str> = markdown.lines().collect();
        let mut in_code_block = false;
        let mut code_lang = String::new();

        for line in lines {
            if line.starts_with("```") {
                if in_code_block {
                    html.push_str("</code></pre>\n");
                    in_code_block = false;
                } else {
                    code_lang = line[3..].trim().to_string();
                    html.push_str(&format!("<pre><code class=\"language-{}\">", code_lang));
                    in_code_block = true;
                }
                continue;
            }

            if in_code_block {
                html.push_str(&html_escape(line));
                html.push('\n');
                continue;
            }

            if line.starts_with("# ") {
                html.push_str(&format!("<h1>{}</h1>\n", html_escape(&line[2..])));
            } else if line.starts_with("## ") {
                html.push_str(&format!("<h2>{}</h2>\n", html_escape(&line[3..])));
            } else if line.starts_with("### ") {
                html.push_str(&format!("<h3>{}</h3>\n", html_escape(&line[4..])));
            } else if line.starts_with("- ") {
                html.push_str(&format!("<li>{}</li>\n", html_escape(&line[2..])));
            } else if line.trim().is_empty() {
                html.push_str("<br>\n");
            } else {
                html.push_str(&format!("<p>{}</p>\n", html_escape(line)));
            }
        }

        if in_code_block {
            html.push_str("</code></pre>\n");
        }

        html
    }
}

/// Escape HTML special characters
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Documentation panel component
#[derive(Clone)]
pub struct DocumentationPanel {
    content: Option<DocumentationContent>,
    renderer: MarkdownRenderer,
    visible: bool,
    width: f32,
}

impl DocumentationPanel {
    pub fn new() -> Self {
        Self {
            content: None,
            renderer: MarkdownRenderer::new(),
            visible: false,
            width: 300.0,
        }
    }

    pub fn set_content(&mut self, content: Option<DocumentationContent>) {
        self.content = content;
        // Reset scroll position when content changes
        // TODO: Implement scroll position reset when API is available
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
            .bg(tokens.colors.surface_elevated)
            .border_l_1()
            .border_color(tokens.colors.border_default)
            .child(
                // Header
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(tokens.colors.border_muted)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(tokens.colors.text_primary)
                            .child("Documentation"),
                    ),
            )
            .child(
                // Content area
                div()
                    .flex_1()
                    .overflow_y_hidden()
                    .px_3()
                    .py_2()
                    // TODO: Add scroll tracking when API is available
                    .child(self.render_content(&tokens)),
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
    fn render_content(&mut self, tokens: &crate::DesignTokens) -> impl IntoElement {
        match &self.content {
            Some(content) => match &content.state {
                DocumentationState::Loading => {
                    div().flex().items_center().justify_center().h_20().child(
                        div()
                            .text_sm()
                            .text_color(tokens.colors.text_secondary)
                            .child("Loading documentation..."),
                    )
                }
                DocumentationState::Failed(error) => {
                    div().flex().items_center().justify_center().h_20().child(
                        div()
                            .text_sm()
                            .text_color(tokens.colors.error)
                            .child(format!("Failed to load: {}", error)),
                    )
                }
                DocumentationState::Loaded => {
                    if content.markdown.is_empty() {
                        div().flex().items_center().justify_center().h_20().child(
                            div()
                                .text_sm()
                                .text_color(tokens.colors.text_tertiary)
                                .child("No documentation available"),
                        )
                    } else {
                        // Render markdown content
                        let _html = self.renderer.render(&content.markdown);
                        div()
                            .text_sm()
                            .text_color(tokens.colors.text_primary)
                            .line_height(relative(1.5))
                            .child(
                                // For now, render as plain text
                                // In a full implementation, you'd render the HTML
                                div().child(content.markdown.clone()),
                            )
                    }
                }
                DocumentationState::NotLoaded => {
                    div().flex().items_center().justify_center().h_20().child(
                        div()
                            .text_sm()
                            .text_color(tokens.colors.text_tertiary)
                            .child("Select an item to view documentation"),
                    )
                }
            },
            None => div().flex().items_center().justify_center().h_20().child(
                div()
                    .text_sm()
                    .text_color(tokens.colors.text_tertiary)
                    .child("No item selected"),
            ),
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
    fn test_markdown_renderer() {
        let mut renderer = MarkdownRenderer::new();

        let markdown = "# Header\nSome text\n## Subheader";
        let html = renderer.render(markdown);

        assert!(html.contains("<h1>Header</h1>"));
        assert!(html.contains("<h2>Subheader</h2>"));
        assert!(html.contains("<p>Some text</p>"));
    }

    #[test]
    fn test_markdown_code_blocks() {
        let mut renderer = MarkdownRenderer::new();

        let markdown = "```rust\nfn main() {}\n```";
        let html = renderer.render(markdown);

        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(html.contains("fn main() {}"));
        assert!(html.contains("</code></pre>"));
    }

    #[test]
    fn test_html_escaping() {
        let escaped = html_escape("<script>alert('xss')</script>");
        assert_eq!(
            escaped,
            "&lt;script&gt;alert(&#x27;xss&#x27;)&lt;/script&gt;"
        );
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
