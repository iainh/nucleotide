// ABOUTME: Implementation of editor capability traits for Application
// ABOUTME: Allows Application to be used through abstract capability interfaces

use crate::application::Application;
use helix_core::Rope;
use helix_view::{Document, DocumentId, Editor, Theme, ViewId};
use nucleotide_core::editor_capabilities::*;
use std::path::Path;

impl DocumentAccess for Application {
    fn get_document(&self, id: DocumentId) -> Option<&Document> {
        self.editor.documents.get(&id)
    }

    fn get_document_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.editor.documents.get_mut(&id)
    }

    fn get_document_rope(&self, id: DocumentId) -> Option<&Rope> {
        self.editor.documents.get(&id).map(|doc| doc.text())
    }

    fn get_document_path(&self, id: DocumentId) -> Option<&Path> {
        self.editor.documents.get(&id)?.path().map(|p| p.as_path())
    }

    fn get_document_language(&self, id: DocumentId) -> Option<String> {
        self.editor
            .documents
            .get(&id)
            .and_then(|doc| doc.language_name().map(|s| s.to_string()))
    }
}

impl ViewManagement for Application {
    fn get_view(&self, view_id: ViewId) -> Option<helix_view::View> {
        self.editor.tree.try_get(view_id).cloned()
    }

    fn get_view_document(&self, view_id: ViewId) -> Option<DocumentId> {
        self.editor.tree.try_get(view_id).map(|view| view.doc)
    }

    fn update_viewport(&mut self, view_id: ViewId, _offset: usize) {
        // Store doc_id first to avoid borrow checker issues
        let doc_id = self.editor.tree.get(view_id).doc;
        if let Some(doc) = self.editor.documents.get_mut(&doc_id) {
            let view = self.editor.tree.get_mut(view_id);
            view.ensure_cursor_in_view(doc, 0);
        }
    }

    fn get_cursor_position(&self, view_id: ViewId) -> Option<(usize, usize)> {
        self.editor.tree.try_get(view_id).and_then(|view| {
            let doc = self.editor.documents.get(&view.doc)?;
            let pos = doc
                .selection(view.id)
                .primary()
                .cursor(doc.text().slice(..));
            let coords = helix_core::coords_at_pos(doc.text().slice(..), pos);
            Some((coords.row, coords.col))
        })
    }
}

impl EditorAccess for Application {
    fn editor(&self) -> &Editor {
        &self.editor
    }

    fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    fn theme(&self) -> Theme {
        self.editor.theme.clone()
    }

    fn mode(&self) -> helix_view::document::Mode {
        self.editor.mode()
    }
}

impl CommandExecution for Application {
    fn execute_command(&mut self, _command: &str, _args: &[String]) -> Result<(), String> {
        // This would need to be implemented based on how commands are executed
        // For now, returning a placeholder
        Ok(())
    }

    fn execute_normal_command(&mut self, _keys: &str) -> Result<(), String> {
        // This would need to be implemented based on key handling
        Ok(())
    }
}

impl StatusInfo for Application {
    fn get_status_line(&self, doc_id: DocumentId, _view_id: ViewId) -> String {
        // Get document and view info for status line
        if let Some(doc) = self.editor.documents.get(&doc_id) {
            let path = doc
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "[scratch]".to_string());

            let modified = if doc.is_modified() { "[+]" } else { "" };

            format!("{}{}", path, modified)
        } else {
            String::new()
        }
    }

    fn get_mode_string(&self) -> String {
        use helix_view::document::Mode;
        match self.editor.mode() {
            Mode::Normal => "NORMAL",
            Mode::Select => "SELECT",
            Mode::Insert => "INSERT",
        }
        .to_string()
    }

    fn get_diagnostics_count(&self, doc_id: DocumentId) -> (usize, usize, usize) {
        if let Some(doc) = self.editor.documents.get(&doc_id) {
            let diagnostics = doc.diagnostics();
            let mut errors = 0;
            let mut warnings = 0;
            let mut info = 0;

            for diag in diagnostics {
                match diag.severity {
                    Some(helix_core::diagnostic::Severity::Error) => errors += 1,
                    Some(helix_core::diagnostic::Severity::Warning) => warnings += 1,
                    Some(helix_core::diagnostic::Severity::Info) => info += 1,
                    Some(helix_core::diagnostic::Severity::Hint) => info += 1,
                    None => info += 1,
                }
            }

            (errors, warnings, info)
        } else {
            (0, 0, 0)
        }
    }
}

impl EditorCapabilities for Application {
    fn request_redraw(&self) {
        // This would trigger a redraw through the event system
    }

    fn has_unsaved_changes(&self) -> bool {
        self.editor.documents.values().any(|doc| doc.is_modified())
    }

    fn save_all(&mut self) -> Result<(), String> {
        // Would implement save all logic
        Ok(())
    }
}

// Implement the basic capability traits from nucleotide_core::capabilities

impl nucleotide_core::BufferStore for Application {
    fn open_document(&self, _path: &Path) -> Result<DocumentId, String> {
        // This would need proper implementation
        Err("Not implemented".to_string())
    }

    fn get_document(&self, id: DocumentId) -> Option<&Rope> {
        DocumentAccess::get_document_rope(self, id)
    }

    fn get_document_mut(&mut self, _id: DocumentId) -> Option<&mut Rope> {
        // Helix doesn't expose text_mut(), we need a different approach
        // For now, return None as mutation should go through proper Helix commands
        None
    }

    fn close_document(&mut self, _id: DocumentId) -> Result<(), String> {
        // This would need proper implementation
        Ok(())
    }

    fn list_documents(&self) -> Vec<DocumentId> {
        self.editor.documents.keys().copied().collect()
    }
}

impl nucleotide_core::ViewStore for Application {
    fn create_view(&mut self, doc_id: DocumentId) -> ViewId {
        // new_file_from_document is private, use switch to open the document
        self.editor
            .switch(doc_id, helix_view::editor::Action::Replace);
        self.editor.tree.focus
    }

    fn focused_view(&self) -> Option<ViewId> {
        Some(self.editor.tree.focus)
    }

    fn focus_view(&mut self, view_id: ViewId) {
        self.editor.tree.focus = view_id;
    }

    fn close_view(&mut self, _view_id: ViewId) {
        // This would need proper implementation
    }

    fn view_document(&self, view_id: ViewId) -> Option<DocumentId> {
        self.get_view_document(view_id)
    }
}

impl nucleotide_core::ThemeProvider for Application {
    fn current_theme(&self) -> Theme {
        self.theme().clone()
    }

    fn set_theme(&mut self, theme: Theme) {
        self.editor.theme = theme;
    }

    fn available_themes(&self) -> Vec<String> {
        Vec::new() // Would list available themes
    }
}

impl nucleotide_core::CommandExecutor for Application {
    fn execute_command(&self, _name: &str, _args: Vec<String>) -> Result<(), String> {
        // Note: changed to &self to match trait definition
        Ok(())
    }

    fn has_command(&self, _name: &str) -> bool {
        false
    }

    fn list_commands(&self) -> Vec<String> {
        Vec::new()
    }
}

impl nucleotide_core::EditorState for Application {
    fn current_mode(&self) -> String {
        self.get_mode_string()
    }

    fn has_unsaved_changes(&self) -> bool {
        EditorCapabilities::has_unsaved_changes(self)
    }

    fn save_all(&mut self) -> Result<(), String> {
        EditorCapabilities::save_all(self)
    }
}
