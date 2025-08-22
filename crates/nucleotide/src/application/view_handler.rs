// ABOUTME: View event handler for V2 event system
// ABOUTME: Handles view selection, focus, and scroll events

use async_trait::async_trait;
use nucleotide_events::handler::{EventHandler, HandlerError};
use nucleotide_events::v2::view::Event as ViewEvent;
use nucleotide_logging::{debug, error, info, instrument, warn};

use helix_view::{DocumentId, ViewId};
use nucleotide_events::view::Selection;
use std::collections::HashMap;

/// Handler for view domain events
/// Manages view state, selection changes, focus, and scroll coordination
pub struct ViewHandler {
    /// Cache of view metadata for quick access
    view_metadata: HashMap<ViewId, ViewMetadata>,

    /// Track the currently focused view
    focused_view: Option<ViewId>,

    /// Flag to track if handler is initialized
    initialized: bool,
}

/// Cached metadata about views
#[derive(Debug, Clone)]
struct ViewMetadata {
    pub associated_doc_id: DocumentId,
    pub last_selection: Selection,
    pub is_focused: bool,
    pub scroll_offset: (usize, usize), // (line, column)
    pub last_focus_time: Option<std::time::Instant>,
}

impl ViewHandler {
    /// Create a new view handler
    pub fn new() -> Self {
        Self {
            view_metadata: HashMap::new(),
            focused_view: None,
            initialized: false,
        }
    }

    /// Initialize the handler
    #[instrument(skip(self))]
    pub fn initialize(&mut self) -> Result<(), HandlerError> {
        if self.initialized {
            warn!("ViewHandler already initialized");
            return Ok(());
        }

        info!("Initializing ViewHandler");
        self.initialized = true;
        Ok(())
    }

    /// Handle selection changed event
    #[instrument(skip(self, selection), fields(view_id = ?view_id, doc_id = ?doc_id))]
    async fn handle_selection_changed(
        &mut self,
        view_id: ViewId,
        doc_id: DocumentId,
        selection: Selection,
        was_movement: bool,
    ) -> Result<(), HandlerError> {
        debug!(
            view_id = ?view_id,
            doc_id = ?doc_id,
            selection_len = selection.len(),
            was_movement = was_movement,
            "Processing view selection change"
        );

        // Update metadata cache
        if let Some(metadata) = self.view_metadata.get_mut(&view_id) {
            metadata.last_selection = selection.clone();
            metadata.associated_doc_id = doc_id;
        } else {
            // Create new metadata entry
            let metadata = ViewMetadata {
                associated_doc_id: doc_id,
                last_selection: selection.clone(),
                is_focused: false,
                scroll_offset: (0, 0),
                last_focus_time: None,
            };
            self.view_metadata.insert(view_id, metadata);
        }

        info!(
            view_id = ?view_id,
            doc_id = ?doc_id,
            primary_cursor = ?selection.primary().cursor(helix_core::ropey::Rope::from("").slice(..)),
            "View selection change processed successfully"
        );

        Ok(())
    }

    /// Handle view focused event
    #[instrument(skip(self), fields(view_id = ?view_id, doc_id = ?doc_id))]
    async fn handle_focused(
        &mut self,
        view_id: ViewId,
        doc_id: DocumentId,
        previous_view: Option<ViewId>,
    ) -> Result<(), HandlerError> {
        info!(
            view_id = ?view_id,
            doc_id = ?doc_id,
            previous_view = ?previous_view,
            "Processing view focus change"
        );

        // Update previous view to unfocused
        if let Some(prev_view) = previous_view
            && let Some(prev_metadata) = self.view_metadata.get_mut(&prev_view)
        {
            prev_metadata.is_focused = false;
        }

        // Update current view to focused
        if let Some(metadata) = self.view_metadata.get_mut(&view_id) {
            metadata.is_focused = true;
            metadata.associated_doc_id = doc_id;
            metadata.last_focus_time = Some(std::time::Instant::now());
        } else {
            // Create new metadata entry for focused view
            let metadata = ViewMetadata {
                associated_doc_id: doc_id,
                last_selection: Selection::point(0),
                is_focused: true,
                scroll_offset: (0, 0),
                last_focus_time: Some(std::time::Instant::now()),
            };
            self.view_metadata.insert(view_id, metadata);
        }

        // Update focused view tracker
        self.focused_view = Some(view_id);

        info!(
            view_id = ?view_id,
            doc_id = ?doc_id,
            "View focus change processed successfully"
        );

        Ok(())
    }

    /// Handle view scrolled event
    #[instrument(skip(self), fields(view_id = ?view_id))]
    async fn handle_scrolled(
        &mut self,
        view_id: ViewId,
        scroll_position: nucleotide_events::v2::view::ScrollPosition,
        direction: nucleotide_events::v2::view::ScrollDirection,
    ) -> Result<(), HandlerError> {
        debug!(
            view_id = ?view_id,
            scroll_position = ?scroll_position,
            direction = ?direction,
            "Processing view scroll"
        );

        // Update scroll offset in metadata
        if let Some(metadata) = self.view_metadata.get_mut(&view_id) {
            metadata.scroll_offset = (scroll_position.line, scroll_position.column);
        } else {
            // Create new metadata entry for scrolled view
            let metadata = ViewMetadata {
                associated_doc_id: helix_view::DocumentId::default(), // Default doc_id for scroll-only events
                last_selection: Selection::point(0),
                is_focused: false,
                scroll_offset: (scroll_position.line, scroll_position.column),
                last_focus_time: None,
            };
            self.view_metadata.insert(view_id, metadata);
        }

        info!(
            view_id = ?view_id,
            line = scroll_position.line,
            column = scroll_position.column,
            "View scroll processed successfully"
        );

        Ok(())
    }

    /// Get view metadata (for debugging/testing)
    pub fn get_metadata(&self, view_id: &ViewId) -> Option<&ViewMetadata> {
        self.view_metadata.get(view_id)
    }

    /// Get currently focused view
    pub fn get_focused_view(&self) -> Option<ViewId> {
        self.focused_view
    }
}

#[async_trait]
impl EventHandler<ViewEvent> for ViewHandler {
    type Error = HandlerError;

    #[instrument(skip(self, event))]
    async fn handle(&mut self, event: ViewEvent) -> Result<(), Self::Error> {
        if !self.initialized {
            error!("ViewHandler not initialized");
            return Err(HandlerError::NotInitialized);
        }

        match event {
            ViewEvent::SelectionChanged {
                view_id,
                doc_id,
                selection,
                was_movement,
            } => {
                self.handle_selection_changed(view_id, doc_id, selection, was_movement)
                    .await
            }
            ViewEvent::Focused {
                view_id,
                doc_id,
                previous_view,
            } => self.handle_focused(view_id, doc_id, previous_view).await,
            ViewEvent::Scrolled {
                view_id,
                scroll_position,
                direction,
            } => {
                self.handle_scrolled(view_id, scroll_position, direction)
                    .await
            }
            ViewEvent::CursorMoved { .. } => {
                // TODO: Implement cursor moved handling
                debug!("CursorMoved event received but not yet implemented");
                Ok(())
            }
            ViewEvent::SplitCreated { .. } => {
                // TODO: Implement split created handling
                debug!("SplitCreated event received but not yet implemented");
                Ok(())
            }
            ViewEvent::Closed { .. } => {
                // TODO: Implement view closed handling
                debug!("Closed event received but not yet implemented");
                Ok(())
            }
        }
    }
}

impl Default for ViewHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::{DocumentId, ViewId};
    use nucleotide_events::v2::view::{Event as ViewEvent, ScrollDirection, ScrollPosition};

    #[tokio::test]
    async fn test_view_handler_initialization() {
        let mut handler = ViewHandler::new();
        assert!(!handler.initialized);
        assert!(handler.get_focused_view().is_none());

        handler.initialize().unwrap();
        assert!(handler.initialized);
    }

    #[tokio::test]
    async fn test_view_selection_changed_event() {
        let mut handler = ViewHandler::new();
        handler.initialize().unwrap();

        let view_id = ViewId::default();
        let doc_id = DocumentId::default();
        let selection = Selection::point(10);
        let event = ViewEvent::SelectionChanged {
            view_id,
            doc_id,
            selection: selection.clone(),
            was_movement: true,
        };

        handler.handle(event).await.unwrap();

        let metadata = handler.get_metadata(&view_id).unwrap();
        assert_eq!(metadata.associated_doc_id, doc_id);
        assert_eq!(metadata.last_selection, selection);
    }

    #[tokio::test]
    async fn test_view_focused_event() {
        let mut handler = ViewHandler::new();
        handler.initialize().unwrap();

        let view_id = ViewId::default();
        let doc_id = DocumentId::default();
        let event = ViewEvent::Focused {
            view_id,
            doc_id,
            previous_view: None,
        };

        handler.handle(event).await.unwrap();

        assert_eq!(handler.get_focused_view(), Some(view_id));
        let metadata = handler.get_metadata(&view_id).unwrap();
        assert_eq!(metadata.associated_doc_id, doc_id);
        assert!(metadata.is_focused);
        assert!(metadata.last_focus_time.is_some());
    }

    #[tokio::test]
    async fn test_view_scrolled_event() {
        let mut handler = ViewHandler::new();
        handler.initialize().unwrap();

        let view_id = ViewId::default();
        let scroll_position = ScrollPosition {
            line: 100,
            column: 20,
        };
        let event = ViewEvent::Scrolled {
            view_id,
            scroll_position: scroll_position.clone(),
            direction: ScrollDirection::Down,
        };

        handler.handle(event).await.unwrap();

        let metadata = handler.get_metadata(&view_id).unwrap();
        assert_eq!(metadata.scroll_offset, (100, 20));
    }

    #[tokio::test]
    async fn test_focus_tracking_with_previous_view() {
        let mut handler = ViewHandler::new();
        handler.initialize().unwrap();

        let view1 = ViewId::default();
        // Create a unique ViewId by using unsafe transmute (for testing only)
        let view2 = unsafe { std::mem::transmute::<usize, ViewId>(1usize) };
        let doc_id = DocumentId::default();

        // Focus first view
        let event1 = ViewEvent::Focused {
            view_id: view1,
            doc_id,
            previous_view: None,
        };
        handler.handle(event1).await.unwrap();
        assert_eq!(handler.get_focused_view(), Some(view1));

        // Focus second view (should unfocus first)
        let event2 = ViewEvent::Focused {
            view_id: view2,
            doc_id,
            previous_view: Some(view1),
        };
        handler.handle(event2).await.unwrap();

        assert_eq!(handler.get_focused_view(), Some(view2));
        let metadata2 = handler.get_metadata(&view2).unwrap();
        assert!(metadata2.is_focused);

        // Check that previous view was unfocused
        if let Some(metadata1) = handler.get_metadata(&view1) {
            assert!(!metadata1.is_focused);
        }
    }

    #[tokio::test]
    async fn test_uninitialized_handler_error() {
        let mut handler = ViewHandler::new();
        let view_id = ViewId::default();
        let doc_id = DocumentId::default();
        let event = ViewEvent::SelectionChanged {
            view_id,
            doc_id,
            selection: Selection::point(0),
            was_movement: false,
        };

        let result = handler.handle(event).await;
        assert!(matches!(result, Err(HandlerError::NotInitialized)));
    }
}
