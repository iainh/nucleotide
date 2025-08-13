// ABOUTME: Main editor view component that renders documents using capability traits
// ABOUTME: Avoids circular deps by depending on EditorState trait not concrete Application

use gpui::*;
use helix_view::{DocumentId, ViewId};
use nucleotide_core::{CoreEvent, EditorState, EventHandler};
use nucleotide_logging::{debug, instrument};
use std::sync::{Arc, RwLock};

/// Editor view that renders documents without depending on concrete Application
pub struct EditorView<S: EditorState> {
    /// Editor state through capability trait (wrapped in RwLock for interior mutability)
    editor_state: Arc<RwLock<S>>,

    /// Current document being viewed
    current_doc: Option<DocumentId>,

    /// Current view
    current_view: Option<ViewId>,

    /// Scroll state
    scroll_state: nucleotide_editor::ScrollState,

    /// Cached line height
    line_height: Pixels,
}

impl<S: EditorState + 'static> EditorView<S> {
    /// Create a new editor view
    pub fn new(editor_state: Arc<RwLock<S>>) -> Self {
        Self {
            editor_state,
            current_doc: None,
            current_view: None,
            scroll_state: nucleotide_editor::ScrollState::new(),
            line_height: px(20.0),
        }
    }

    /// Open a document for viewing
    pub fn open_document(&mut self, doc_id: DocumentId) {
        self.current_doc = Some(doc_id);

        // Create a view for the document (acquire write lock)
        let view_id = {
            let mut state = self.editor_state.write().unwrap();
            state.create_view(doc_id)
        };
        self.current_view = Some(view_id);

        // Focus the view (acquire write lock again)
        {
            let mut state = self.editor_state.write().unwrap();
            state.focus_view(view_id);
        }

        // Reset scroll state
        self.scroll_state.reset();
    }

    /// Close the current document
    pub fn close_document(&mut self) {
        if let Some(view_id) = self.current_view {
            let mut state = self.editor_state.write().unwrap();
            state.close_view(view_id);
        }

        if let Some(doc_id) = self.current_doc {
            let mut state = self.editor_state.write().unwrap();
            let _ = state.close_document(doc_id);
        }

        self.current_doc = None;
        self.current_view = None;
    }

    /// Scroll by a number of lines
    pub fn scroll_by_lines(&mut self, lines: i32) {
        self.scroll_state.scroll_by_lines(lines, self.line_height);
    }

    /// Scroll to a specific line
    pub fn scroll_to_line(&mut self, line: usize) {
        self.scroll_state.scroll_to_line(line, self.line_height);
    }

    /// Get the current theme (returns a clone since we can't return a reference through RwLock)
    pub fn theme(&self) -> helix_view::Theme {
        let state = self.editor_state.read().unwrap();
        state.current_theme()
    }

    /// Check if we have a document open
    pub fn has_document(&self) -> bool {
        self.current_doc.is_some()
    }

    /// Get the current document ID
    pub fn current_document(&self) -> Option<DocumentId> {
        self.current_doc
    }

    /// Get the current view ID
    pub fn current_view(&self) -> Option<ViewId> {
        self.current_view
    }
}

impl<S: EditorState> EventHandler for EditorView<S> {
    fn handle_core(&mut self, event: &CoreEvent) {
        match event {
            CoreEvent::DocumentChanged { doc_id } => {
                if Some(*doc_id) == self.current_doc {
                    // Document changed, may need to adjust scroll
                    debug!(doc_id = ?doc_id, "Document changed");
                }
            }
            CoreEvent::SelectionChanged { doc_id, view_id } => {
                if Some(*doc_id) == self.current_doc && Some(*view_id) == self.current_view {
                    // Selection changed, ensure cursor is visible
                    debug!(view_id = ?view_id, "Selection changed in view");
                }
            }
            CoreEvent::ViewFocused { view_id } => {
                if Some(*view_id) == self.current_view {
                    debug!(view_id = ?view_id, "View focused");
                }
            }
            _ => {}
        }
    }
}

/// GPUI view implementation
impl<S: EditorState + 'static> Render for EditorView<S> {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .child(if self.has_document() {
                div()
                    .size_full()
                    .child("Document content would be rendered here")
            } else {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("No document open")
            })
    }
}
