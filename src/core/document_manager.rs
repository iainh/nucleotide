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
    
    /// Get the current document ID for a view
    pub fn current_document_id(&self, view_id: helix_view::ViewId) -> Option<DocumentId> {
        self.editor.tree.try_get(view_id).map(|view| view.doc)
    }
    
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

    /// Save a document
    pub fn save_document(&mut self, doc_id: DocumentId, force: bool) -> Result<(), anyhow::Error> {
        
        // Get the document path - we need to get it before the mutable borrow
        let path = self.editor.document(doc_id)
            .ok_or_else(|| anyhow::anyhow!("Document not found"))?
            .path()
            .ok_or_else(|| anyhow::anyhow!("Document has no path"))?
            .to_path_buf();
        
        // Save the document
        self.editor.save(doc_id, Some(&path), force)
            .map_err(|e| anyhow::anyhow!("Failed to save document: {}", e))
    }
}