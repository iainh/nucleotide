// ABOUTME: Document event handler for V2 event system
// ABOUTME: Handles document lifecycle, content changes, and persistence events

use async_trait::async_trait;
use nucleotide_events::handler::{EventHandler, HandlerError};
use nucleotide_events::v2::document::Event as DocumentEvent;
use nucleotide_logging::{debug, error, info, instrument, warn};

use helix_view::DocumentId;
use std::collections::HashMap;

/// Handler for document domain events
/// Replaces document-related channel communication with event-based handling
pub struct DocumentHandler {
    /// Cache of document metadata for quick access
    document_metadata: HashMap<DocumentId, DocumentMetadata>,

    /// Flag to track if handler is initialized
    initialized: bool,
}

/// Cached metadata about documents
#[derive(Debug, Clone)]
struct DocumentMetadata {
    pub path: Option<std::path::PathBuf>,
    pub language_id: Option<String>,
    pub revision: u64,
    pub is_modified: bool,
    pub last_saved: Option<std::time::Instant>,
}

impl DocumentHandler {
    /// Create a new document handler
    pub fn new() -> Self {
        Self {
            document_metadata: HashMap::new(),
            initialized: false,
        }
    }

    /// Initialize the handler with application context
    #[instrument(skip(self))]
    pub fn initialize(&mut self) -> Result<(), HandlerError> {
        if self.initialized {
            warn!("DocumentHandler already initialized");
            return Ok(());
        }

        info!("Initializing DocumentHandler");
        self.initialized = true;
        Ok(())
    }

    /// Handle document content change event
    #[instrument(skip(self), fields(doc_id = ?doc_id, revision = revision))]
    async fn handle_content_changed(
        &mut self,
        doc_id: DocumentId,
        revision: u64,
        change_summary: nucleotide_events::v2::document::ChangeType,
    ) -> Result<(), HandlerError> {
        debug!(
            doc_id = ?doc_id,
            revision = revision,
            change_type = ?change_summary,
            "Processing document content change"
        );

        // Update metadata cache
        if let Some(metadata) = self.document_metadata.get_mut(&doc_id) {
            metadata.revision = revision;
            metadata.is_modified = true;
        } else {
            // Create new metadata entry
            let metadata = DocumentMetadata {
                path: None,
                language_id: None,
                revision,
                is_modified: true,
                last_saved: None,
            };
            self.document_metadata.insert(doc_id, metadata);
        }

        // TODO: Emit UI update event to refresh document view
        // This will be handled by UI event handlers once they're implemented

        info!(
            doc_id = ?doc_id,
            revision = revision,
            "Document content change processed successfully"
        );

        Ok(())
    }

    /// Handle document opened event
    #[instrument(skip(self), fields(doc_id = ?doc_id, path = ?path.display()))]
    async fn handle_opened(
        &mut self,
        doc_id: DocumentId,
        path: std::path::PathBuf,
        language_id: Option<String>,
    ) -> Result<(), HandlerError> {
        info!(
            doc_id = ?doc_id,
            path = %path.display(),
            language_id = ?language_id,
            "Processing document opened event"
        );

        // Create metadata entry
        let metadata = DocumentMetadata {
            path: Some(path.clone()),
            language_id: language_id.clone(),
            revision: 0,
            is_modified: false,
            last_saved: None,
        };
        self.document_metadata.insert(doc_id, metadata);

        // TODO: Emit integration event for LSP server association
        // TODO: Emit UI event to update file tree and tab bar

        info!(
            doc_id = ?doc_id,
            path = %path.display(),
            "Document opened event processed successfully"
        );

        Ok(())
    }

    /// Handle document closed event
    #[instrument(skip(self), fields(doc_id = ?doc_id))]
    async fn handle_closed(
        &mut self,
        doc_id: DocumentId,
        was_modified: bool,
    ) -> Result<(), HandlerError> {
        info!(
            doc_id = ?doc_id,
            was_modified = was_modified,
            "Processing document closed event"
        );

        // Remove from metadata cache
        if let Some(metadata) = self.document_metadata.remove(&doc_id) {
            debug!(
                doc_id = ?doc_id,
                path = ?metadata.path,
                final_revision = metadata.revision,
                "Removed document metadata from cache"
            );
        } else {
            warn!(
                doc_id = ?doc_id,
                "Document closed but no metadata found in cache"
            );
        }

        // TODO: Emit integration event for LSP server cleanup
        // TODO: Emit UI event to update file tree and close tabs

        info!(
            doc_id = ?doc_id,
            "Document closed event processed successfully"
        );

        Ok(())
    }

    /// Handle document saved event
    #[instrument(skip(self), fields(doc_id = ?doc_id, path = ?path.display()))]
    async fn handle_saved(
        &mut self,
        doc_id: DocumentId,
        path: std::path::PathBuf,
        revision: u64,
    ) -> Result<(), HandlerError> {
        info!(
            doc_id = ?doc_id,
            path = %path.display(),
            revision = revision,
            "Processing document saved event"
        );

        // Update metadata cache
        if let Some(metadata) = self.document_metadata.get_mut(&doc_id) {
            metadata.revision = revision;
            metadata.is_modified = false;
            metadata.last_saved = Some(std::time::Instant::now());
            metadata.path = Some(path.clone());
        }

        // TODO: Emit UI event to update save indicators

        info!(
            doc_id = ?doc_id,
            path = %path.display(),
            revision = revision,
            "Document saved event processed successfully"
        );

        Ok(())
    }

    /// Get document metadata (for debugging/testing)
    pub fn get_metadata(&self, doc_id: &DocumentId) -> Option<&DocumentMetadata> {
        self.document_metadata.get(doc_id)
    }
}

#[async_trait]
impl EventHandler<DocumentEvent> for DocumentHandler {
    type Error = HandlerError;

    #[instrument(skip(self, event))]
    async fn handle(&mut self, event: DocumentEvent) -> Result<(), Self::Error> {
        if !self.initialized {
            error!("DocumentHandler not initialized");
            return Err(HandlerError::NotInitialized);
        }

        match event {
            DocumentEvent::ContentChanged {
                doc_id,
                revision,
                change_summary,
            } => {
                self.handle_content_changed(doc_id, revision, change_summary)
                    .await
            }
            DocumentEvent::Opened {
                doc_id,
                path,
                language_id,
            } => self.handle_opened(doc_id, path, language_id).await,
            DocumentEvent::Closed {
                doc_id,
                was_modified,
            } => self.handle_closed(doc_id, was_modified).await,
            DocumentEvent::Saved {
                doc_id,
                path,
                revision,
            } => self.handle_saved(doc_id, path, revision).await,
            DocumentEvent::SaveFailed {
                doc_id,
                path,
                error,
            } => {
                warn!(
                    doc_id = ?doc_id,
                    path = %path.display(),
                    error = %error,
                    "Document save failed"
                );
                // TODO: Emit UI event to show save failure
                Ok(())
            }
            DocumentEvent::LanguageDetected {
                doc_id,
                language_id,
            } => {
                debug!(
                    doc_id = ?doc_id,
                    language_id = ?language_id,
                    "Language detected for document"
                );

                // Update metadata cache
                if let Some(metadata) = self.document_metadata.get_mut(&doc_id) {
                    metadata.language_id = Some(language_id.clone());
                }

                // TODO: Emit integration event for LSP server association
                Ok(())
            }
            DocumentEvent::DiagnosticsUpdated {
                doc_id,
                diagnostic_count,
                error_count,
                warning_count,
            } => {
                debug!(
                    doc_id = ?doc_id,
                    diagnostic_count = diagnostic_count,
                    error_count = error_count,
                    warning_count = warning_count,
                    "Diagnostics updated for document"
                );
                // TODO: Emit UI event to update diagnostic indicators
                Ok(())
            }
        }
    }
}

impl Default for DocumentHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::DocumentId;
    use nucleotide_events::v2::document::{ChangeType, Event as DocumentEvent};
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_document_handler_initialization() {
        let mut handler = DocumentHandler::new();
        assert!(!handler.initialized);

        handler.initialize().unwrap();
        assert!(handler.initialized);
    }

    #[tokio::test]
    async fn test_document_opened_event() {
        let mut handler = DocumentHandler::new();
        handler.initialize().unwrap();

        let doc_id = DocumentId::default();
        let path = PathBuf::from("/test/file.rs");
        let event = DocumentEvent::Opened {
            doc_id,
            path: path.clone(),
            language_id: Some("rust".to_string()),
        };

        handler.handle(event).await.unwrap();

        let metadata = handler.get_metadata(&doc_id).unwrap();
        assert_eq!(metadata.path, Some(path));
        assert_eq!(metadata.language_id, Some("rust".to_string()));
    }

    #[tokio::test]
    async fn test_document_content_changed_event() {
        let mut handler = DocumentHandler::new();
        handler.initialize().unwrap();

        let doc_id = DocumentId::default();
        let event = DocumentEvent::ContentChanged {
            doc_id,
            revision: 1,
            change_summary: ChangeType::Edit,
        };

        handler.handle(event).await.unwrap();

        let metadata = handler.get_metadata(&doc_id).unwrap();
        assert_eq!(metadata.revision, 1);
        assert!(metadata.is_modified);
    }

    #[tokio::test]
    async fn test_document_closed_event() {
        let mut handler = DocumentHandler::new();
        handler.initialize().unwrap();

        let doc_id = DocumentId::default();

        // First open the document
        let open_event = DocumentEvent::Opened {
            doc_id,
            path: PathBuf::from("/test/file.rs"),
            language_id: Some("rust".to_string()),
        };
        handler.handle(open_event).await.unwrap();

        // Verify it exists
        assert!(handler.get_metadata(&doc_id).is_some());

        // Close the document
        let close_event = DocumentEvent::Closed {
            doc_id,
            was_modified: false,
        };
        handler.handle(close_event).await.unwrap();

        // Verify it was removed
        assert!(handler.get_metadata(&doc_id).is_none());
    }

    #[tokio::test]
    async fn test_uninitialized_handler_error() {
        let mut handler = DocumentHandler::new();
        let doc_id = DocumentId::default();
        let event = DocumentEvent::ContentChanged {
            doc_id,
            revision: 1,
            change_summary: ChangeType::Edit,
        };

        let result = handler.handle(event).await;
        assert!(matches!(result, Err(HandlerError::NotInitialized)));
    }
}
