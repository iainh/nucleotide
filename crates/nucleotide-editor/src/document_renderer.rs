// ABOUTME: Document rendering logic that converts Helix document state to GPUI elements
// ABOUTME: Depends on capability traits rather than concrete types to avoid circular deps

use gpui::*;
use helix_core::Rope;
use helix_view::DocumentId;
use nucleotide_core::EditorState;
use std::sync::Arc;

/// Document renderer that converts Helix documents to GPUI elements
pub struct DocumentRenderer<S: EditorState> {
    editor_state: Arc<S>,
}

impl<S: EditorState> DocumentRenderer<S> {
    /// Create a new document renderer
    pub fn new(editor_state: Arc<S>) -> Self {
        Self { editor_state }
    }

    /// Render a document
    pub fn render_document(&self, doc_id: DocumentId, scroll_y: Pixels) -> Div {
        if let Some(rope) = self.editor_state.get_document(doc_id) {
            self.render_rope(rope, scroll_y)
        } else {
            self.render_empty()
        }
    }

    /// Render a rope (document content)
    fn render_rope(&self, rope: &Rope, _scroll_y: Pixels) -> Div {
        let lines: Vec<String> = rope
            .lines()
            .take(100) // Limit to first 100 lines for now
            .map(|line| line.to_string())
            .collect();

        div().size_full().overflow_hidden().child(
            div().children(
                lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, line)| self.render_line(i + 1, &line)),
            ),
        )
    }

    /// Render a single line
    fn render_line(&self, line_number: usize, content: &str) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .h(px(20.0))
            .child(
                // Line number gutter
                div()
                    .w(px(50.0))
                    .pr(px(10.0))
                    .text_right()
                    .text_color(rgb(0x6c7086))
                    .child(format!("{}", line_number)),
            )
            .child(
                // Line content
                div()
                    .flex_1()
                    .text_color(rgb(0xcdd6f4))
                    .child(content.to_string()),
            )
    }

    /// Render empty document
    fn render_empty(&self) -> Div {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0x6c7086))
            .child("[Empty Document]")
    }

    /// Render with syntax highlighting (placeholder for future implementation)
    pub fn render_with_highlighting(
        &self,
        doc_id: DocumentId,
        scroll_y: Pixels,
    ) -> impl IntoElement {
        // For now, just render without highlighting
        self.render_document(doc_id, scroll_y)
    }
}
