// ABOUTME: View event handler for V2 event system
// ABOUTME: Handles view selection, focus, and scroll events

use async_trait::async_trait;
use nucleotide_events::handler::{EventHandler, HandlerError};
use nucleotide_events::v2::view::Event as ViewEvent;
use nucleotide_logging::{debug, error, info, instrument, warn};

use helix_view::{DocumentId, ViewId};
use nucleotide_events::view::{Position, Selection, SelectionRange, SplitDirection};
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
pub struct ViewMetadata {
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

        debug!(
            view_id = ?view_id,
            doc_id = ?doc_id,
            primary_cursor = ?selection.primary().cursor(),
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

    /// Handle cursor moved event
    #[instrument(skip(self), fields(view_id = ?view_id, doc_id = ?doc_id))]
    async fn handle_cursor_moved(
        &mut self,
        view_id: ViewId,
        doc_id: DocumentId,
        position: Position,
        selection_index: usize,
    ) -> Result<(), HandlerError> {
        debug!(
            view_id = ?view_id,
            doc_id = ?doc_id,
            position = ?position,
            selection_index = selection_index,
            "Processing view cursor movement"
        );

        let cursor_range = SelectionRange::new(position, position);
        let next_selection = if let Some(metadata) = self.view_metadata.get(&view_id) {
            let mut selection = metadata.last_selection.clone();
            if selection_index < selection.ranges.len() {
                selection.ranges[selection_index] = cursor_range;
                selection.primary_index = selection_index;
                selection
            } else {
                Selection::new(vec![cursor_range], 0)
            }
        } else {
            Selection::new(vec![cursor_range], 0)
        };

        if let Some(metadata) = self.view_metadata.get_mut(&view_id) {
            metadata.associated_doc_id = doc_id;
            metadata.last_selection = next_selection;
        } else {
            let metadata = ViewMetadata {
                associated_doc_id: doc_id,
                last_selection: next_selection,
                is_focused: false,
                scroll_offset: (0, 0),
                last_focus_time: None,
            };
            self.view_metadata.insert(view_id, metadata);
        }

        debug!(
            view_id = ?view_id,
            doc_id = ?doc_id,
            line = position.line,
            column = position.column,
            "View cursor movement processed successfully"
        );

        Ok(())
    }

    /// Handle view split created event
    #[instrument(skip(self), fields(new_view_id = ?new_view_id, parent_view_id = ?parent_view_id))]
    async fn handle_split_created(
        &mut self,
        new_view_id: ViewId,
        parent_view_id: ViewId,
        direction: SplitDirection,
    ) -> Result<(), HandlerError> {
        debug!(
            new_view_id = ?new_view_id,
            parent_view_id = ?parent_view_id,
            direction = ?direction,
            "Processing view split creation"
        );

        let metadata = if let Some(parent_metadata) = self.view_metadata.get(&parent_view_id) {
            let mut metadata = parent_metadata.clone();
            metadata.is_focused = false;
            metadata.last_focus_time = None;
            metadata
        } else {
            ViewMetadata {
                associated_doc_id: DocumentId::default(),
                last_selection: Selection::point(0),
                is_focused: false,
                scroll_offset: (0, 0),
                last_focus_time: None,
            }
        };

        self.view_metadata.insert(new_view_id, metadata);

        debug!(
            new_view_id = ?new_view_id,
            parent_view_id = ?parent_view_id,
            "View split creation processed successfully"
        );

        Ok(())
    }

    /// Handle view closed event
    #[instrument(skip(self), fields(view_id = ?view_id, doc_id = ?doc_id))]
    async fn handle_closed(
        &mut self,
        view_id: ViewId,
        doc_id: DocumentId,
    ) -> Result<(), HandlerError> {
        info!(
            view_id = ?view_id,
            doc_id = ?doc_id,
            "Processing view close"
        );

        // Remove metadata
        self.view_metadata.remove(&view_id);

        // Update focused view if needed
        if self.focused_view == Some(view_id) {
            self.focused_view = None;
            info!("Focused view closed, clearing focus");
        }

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
            ViewEvent::CursorMoved {
                view_id,
                doc_id,
                position,
                selection_index,
            } => {
                self.handle_cursor_moved(view_id, doc_id, position, selection_index)
                    .await
            }
            ViewEvent::SplitCreated {
                new_view_id,
                parent_view_id,
                direction,
            } => {
                self.handle_split_created(new_view_id, parent_view_id, direction)
                    .await
            }
            ViewEvent::Closed { view_id, doc_id } => self.handle_closed(view_id, doc_id).await,
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
    use nucleotide_events::v2::view::{
        Event as ViewEvent, Position, ScrollDirection, ScrollPosition, SelectionRange,
        SplitDirection,
    };

    fn view_id(raw: usize) -> ViewId {
        // ViewId has no public test constructor; this mirrors existing tests
        // that need distinct IDs without a full Helix view tree.
        unsafe { std::mem::transmute::<usize, ViewId>(raw) }
    }

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
            scroll_position,
            direction: ScrollDirection::Down,
        };

        handler.handle(event).await.unwrap();

        let metadata = handler.get_metadata(&view_id).unwrap();
        assert_eq!(metadata.scroll_offset, (100, 20));
    }

    #[tokio::test]
    async fn test_view_cursor_moved_event_updates_selection() {
        let mut handler = ViewHandler::new();
        handler.initialize().unwrap();

        let view_id = ViewId::default();
        let doc_id = DocumentId::default();
        let existing_selection = Selection::new(
            vec![
                SelectionRange::new(Position::new(1, 0), Position::new(1, 1)),
                SelectionRange::new(Position::new(2, 0), Position::new(2, 1)),
            ],
            0,
        );

        handler
            .handle(ViewEvent::SelectionChanged {
                view_id,
                doc_id,
                selection: existing_selection,
                was_movement: false,
            })
            .await
            .unwrap();

        let cursor_position = Position::new(4, 8);
        handler
            .handle(ViewEvent::CursorMoved {
                view_id,
                doc_id,
                position: cursor_position,
                selection_index: 1,
            })
            .await
            .unwrap();

        let metadata = handler.get_metadata(&view_id).unwrap();
        assert_eq!(metadata.associated_doc_id, doc_id);
        assert_eq!(metadata.last_selection.ranges.len(), 2);
        assert_eq!(metadata.last_selection.primary_index, 1);
        assert_eq!(
            metadata.last_selection.ranges[1],
            SelectionRange::new(cursor_position, cursor_position)
        );
        assert_eq!(
            metadata.last_selection.ranges[0],
            SelectionRange::new(Position::new(1, 0), Position::new(1, 1))
        );
    }

    #[tokio::test]
    async fn test_view_split_created_event_clones_parent_metadata() {
        let mut handler = ViewHandler::new();
        handler.initialize().unwrap();

        let parent_view_id = ViewId::default();
        let new_view_id = view_id(1);
        let doc_id = DocumentId::default();
        let selection = Selection::new(
            vec![SelectionRange::new(
                Position::new(3, 1),
                Position::new(3, 5),
            )],
            0,
        );

        handler
            .handle(ViewEvent::Focused {
                view_id: parent_view_id,
                doc_id,
                previous_view: None,
            })
            .await
            .unwrap();
        handler
            .handle(ViewEvent::SelectionChanged {
                view_id: parent_view_id,
                doc_id,
                selection: selection.clone(),
                was_movement: false,
            })
            .await
            .unwrap();
        handler
            .handle(ViewEvent::Scrolled {
                view_id: parent_view_id,
                scroll_position: ScrollPosition::new(12, 4),
                direction: ScrollDirection::Down,
            })
            .await
            .unwrap();

        handler
            .handle(ViewEvent::SplitCreated {
                new_view_id,
                parent_view_id,
                direction: SplitDirection::Vertical,
            })
            .await
            .unwrap();

        let metadata = handler.get_metadata(&new_view_id).unwrap();
        assert_eq!(metadata.associated_doc_id, doc_id);
        assert_eq!(metadata.last_selection, selection);
        assert_eq!(metadata.scroll_offset, (12, 4));
        assert!(!metadata.is_focused);
        assert!(metadata.last_focus_time.is_none());
        assert_eq!(handler.get_focused_view(), Some(parent_view_id));
    }

    #[tokio::test]
    async fn test_focus_tracking_with_previous_view() {
        let mut handler = ViewHandler::new();
        handler.initialize().unwrap();

        let view1 = ViewId::default();
        // Create a distinct ViewId without constructing a full Helix view tree.
        let view2 = view_id(1);
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
