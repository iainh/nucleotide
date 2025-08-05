// ABOUTME: Document management functionality extracted from Application
// ABOUTME: Provides safe access patterns for document operations

use helix_view::{Document, DocumentId, Editor};
use std::path::Path;

/// Manages document operations with safe access patterns
pub struct DocumentManager<'a> {
    editor: &'a Editor,
}

/// Mutable document manager for operations that need write access
pub struct DocumentManagerMut<'a> {
    editor: &'a mut Editor,
}

impl<'a> DocumentManager<'a> {
    pub fn new(editor: &'a Editor) -> Self {
        Self { editor }
    }

    /// Safe document access API - read only
    pub fn with_document<F, R>(&self, doc_id: DocumentId, f: F) -> Option<R>
    where
        F: FnOnce(&Document) -> R,
    {
        self.editor.document(doc_id).map(f)
    }
    
    // Removed current_document_id - can access directly via editor.tree.try_get(view_id).map(|v| v.doc)
    
    /// Safe document access API - returns Result instead of Option
    pub fn try_with_document<F, R, E>(&self, doc_id: DocumentId, f: F) -> Result<R, E>
    where
        F: FnOnce(&Document) -> Result<R, E>,
        E: From<String>,
    {
        match self.editor.document(doc_id) {
            Some(doc) => f(doc),
            None => Err(E::from(format!("Document {doc_id} not found"))),
        }
    }
    

}

impl<'a> DocumentManagerMut<'a> {
    pub fn new(editor: &'a mut Editor) -> Self {
        Self { editor }
    }

    /// Safe document access API - mutable
    pub fn with_document_mut<F, R>(&mut self, doc_id: DocumentId, f: F) -> Option<R>
    where
        F: FnOnce(&mut Document) -> R,
    {
        self.editor.document_mut(doc_id).map(f)
    }
    
    /// Safe document access API - mutable with Result
    pub fn try_with_document_mut<F, R, E>(&mut self, doc_id: DocumentId, f: F) -> Result<R, E>
    where
        F: FnOnce(&mut Document) -> Result<R, E>,
        E: From<String>,
    {
        match self.editor.document_mut(doc_id) {
            Some(doc) => f(doc),
            None => Err(E::from(format!("Document {doc_id} not found"))),
        }
    }

    /// Open a file in the editor
    pub fn open_file(&mut self, path: &Path) -> Result<(), anyhow::Error> {
        use helix_view::editor::Action;
        self.editor.open(path, Action::Replace)
            .map(|_| ())
            .map_err(anyhow::Error::new)
    }

    // Removed save_document - can save directly via editor.save() or commands
}