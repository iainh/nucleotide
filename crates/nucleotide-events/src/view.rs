// ABOUTME: View domain events for selection, cursor, scrolling, and viewport changes
// ABOUTME: Immutable fact-based events following Domain-Driven Design principles

use helix_view::{DocumentId, ViewId};

/// View domain events - covers selection, cursor movement, scrolling, and viewport changes
/// Following event sourcing principles: all events are immutable facts about what has happened
#[derive(Debug, Clone)]
pub enum Event {
    /// Selection changed in view
    SelectionChanged {
        view_id: ViewId,
        doc_id: DocumentId,
        selection: Selection,
        was_movement: bool,
    },

    /// View gained focus
    Focused {
        view_id: ViewId,
        doc_id: DocumentId,
        previous_view: Option<ViewId>,
    },

    /// Viewport scrolled
    Scrolled {
        view_id: ViewId,
        scroll_position: ScrollPosition,
        direction: ScrollDirection,
    },

    /// Cursor position changed
    CursorMoved {
        view_id: ViewId,
        doc_id: DocumentId,
        position: Position,
        selection_index: usize,
    },

    /// View split created
    SplitCreated {
        new_view_id: ViewId,
        parent_view_id: ViewId,
        direction: SplitDirection,
    },

    /// View closed
    Closed { view_id: ViewId, doc_id: DocumentId },
}

/// Selection state with multiple ranges and primary selection
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub ranges: Vec<SelectionRange>,
    pub primary_index: usize,
}

/// Individual selection range
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionRange {
    pub anchor: Position,
    pub head: Position,
}

/// Position in document
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// Scroll position in viewport
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollPosition {
    pub line: usize,
    pub column: usize,
}

/// Direction of scroll movement
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
}

/// Direction for view splits
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

impl Selection {
    pub fn new(ranges: Vec<SelectionRange>, primary_index: usize) -> Self {
        assert!(!ranges.is_empty(), "Selection must have at least one range");
        assert!(primary_index < ranges.len(), "Primary index must be valid");

        Self {
            ranges,
            primary_index,
        }
    }

    pub fn primary(&self) -> &SelectionRange {
        &self.ranges[self.primary_index]
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn point(pos: usize) -> Self {
        let position = Position::new(pos, 0);
        let range = SelectionRange::new(position, position);
        Self::new(vec![range], 0)
    }
}

impl SelectionRange {
    pub fn new(anchor: Position, head: Position) -> Self {
        Self { anchor, head }
    }

    pub fn is_cursor(&self) -> bool {
        self.anchor == self.head
    }

    pub fn cursor(&self) -> usize {
        // For simplicity, return the line number as cursor position
        // In a real implementation, this would convert line/column to absolute position
        self.head.line
    }
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

impl ScrollPosition {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_creation() {
        let range = SelectionRange::new(Position::new(0, 0), Position::new(0, 5));
        let selection = Selection::new(vec![range], 0);

        assert_eq!(selection.primary().anchor, Position::new(0, 0));
        assert_eq!(selection.primary().head, Position::new(0, 5));
        assert!(!selection.primary().is_cursor());
    }

    #[test]
    fn test_cursor_selection() {
        let cursor_pos = Position::new(2, 10);
        let range = SelectionRange::new(cursor_pos, cursor_pos);
        let selection = Selection::new(vec![range], 0);

        assert!(selection.primary().is_cursor());
    }

    #[test]
    fn test_view_event_creation() {
        let view_id = ViewId::default();
        let doc_id = DocumentId::default();
        let selection = Selection::new(
            vec![SelectionRange::new(
                Position::new(0, 0),
                Position::new(0, 1),
            )],
            0,
        );

        let event = Event::SelectionChanged {
            view_id,
            doc_id,
            selection: selection.clone(),
            was_movement: false,
        };

        match event {
            Event::SelectionChanged {
                selection: sel,
                was_movement,
                ..
            } => {
                assert_eq!(sel.ranges.len(), selection.ranges.len());
                assert!(!was_movement);
            }
            _ => panic!("Expected SelectionChanged event"),
        }
    }

    #[test]
    fn test_scroll_directions() {
        let directions = [
            ScrollDirection::Up,
            ScrollDirection::Down,
            ScrollDirection::Left,
            ScrollDirection::Right,
            ScrollDirection::PageUp,
            ScrollDirection::PageDown,
        ];

        // All directions should be valid
        for direction in directions {
            let _event = Event::Scrolled {
                view_id: ViewId::default(),
                scroll_position: ScrollPosition::new(10, 5),
                direction,
            };
        }
    }
}
