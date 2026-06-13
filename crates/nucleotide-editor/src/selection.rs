// ABOUTME: Native editor pointer-selection state and Helix selection updates
// ABOUTME: Keeps click, shift-click, and drag selection logic with editor input

use std::{cell::Cell, rc::Rc};

use helix_core::{Range, Selection, SmallVec};
use helix_view::{Document, ViewId};

use crate::{
    EditorHitTestResult, EditorSurfacePointerEvent, LineLayoutCache, hit_test_document_position,
};

#[derive(Clone, Default)]
pub struct EditorSelectionDragState {
    anchor: Rc<Cell<Option<usize>>>,
}

impl EditorSelectionDragState {
    pub fn anchor(&self) -> Option<usize> {
        self.anchor.get()
    }

    pub fn set_anchor(&self, anchor: usize) {
        self.anchor.set(Some(anchor));
    }

    pub fn clear(&self) {
        self.anchor.set(None);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorSelectionUpdate {
    pub anchor: usize,
    pub head: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorPointerSelectionUpdate {
    pub hit_test: EditorHitTestResult,
    pub selection: EditorSelectionUpdate,
}

pub fn pointer_selection_anchor(
    hit_char_idx: usize,
    primary_anchor: usize,
    extend_selection: bool,
) -> usize {
    if extend_selection {
        primary_anchor
    } else {
        hit_char_idx
    }
}

pub fn selection_for_range(
    text_len: usize,
    anchor: usize,
    head: usize,
) -> (Selection, EditorSelectionUpdate) {
    let update = EditorSelectionUpdate {
        anchor: anchor.min(text_len),
        head: head.min(text_len),
    };
    let range = Range::new(update.anchor, update.head);
    let selection = Selection::new(SmallVec::from([range]), 0);

    (selection, update)
}

pub fn primary_selection_anchor(document: &Document, view_id: ViewId) -> usize {
    document.selection(view_id).primary().anchor
}

pub fn apply_pointer_selection(
    document: &mut Document,
    view_id: ViewId,
    anchor: usize,
    head: usize,
) -> EditorSelectionUpdate {
    let (selection, update) = selection_for_range(document.text().len_chars(), anchor, head);
    document.set_selection(view_id, selection);
    update
}

pub fn begin_pointer_selection(
    document: &mut Document,
    view_id: ViewId,
    drag_state: &EditorSelectionDragState,
    hit_char_idx: usize,
    extend_selection: bool,
) -> EditorSelectionUpdate {
    let anchor = pointer_selection_anchor(
        hit_char_idx,
        primary_selection_anchor(document, view_id),
        extend_selection,
    );
    let update = apply_pointer_selection(document, view_id, anchor, hit_char_idx);
    drag_state.set_anchor(update.anchor);
    update
}

pub fn update_pointer_selection(
    document: &mut Document,
    view_id: ViewId,
    drag_state: &EditorSelectionDragState,
    hit_char_idx: usize,
) -> Option<EditorSelectionUpdate> {
    let anchor = drag_state.anchor()?;
    Some(apply_pointer_selection(
        document,
        view_id,
        anchor,
        hit_char_idx,
    ))
}

pub fn begin_pointer_selection_at_event(
    document: &mut Document,
    view_id: ViewId,
    gutter_columns: u16,
    line_cache: &LineLayoutCache,
    drag_state: &EditorSelectionDragState,
    event: EditorSurfacePointerEvent,
) -> Option<EditorPointerSelectionUpdate> {
    let Some(hit_test) = hit_test_document_position(event, gutter_columns, line_cache, document)
    else {
        drag_state.clear();
        return None;
    };

    let selection = begin_pointer_selection(
        document,
        view_id,
        drag_state,
        hit_test.char_idx,
        event.modifiers.shift,
    );

    Some(EditorPointerSelectionUpdate {
        hit_test,
        selection,
    })
}

pub fn update_pointer_selection_at_event(
    document: &mut Document,
    view_id: ViewId,
    gutter_columns: u16,
    line_cache: &LineLayoutCache,
    drag_state: &EditorSelectionDragState,
    event: EditorSurfacePointerEvent,
) -> Option<EditorPointerSelectionUpdate> {
    let hit_test = hit_test_document_position(event, gutter_columns, line_cache, document)?;
    let selection = update_pointer_selection(document, view_id, drag_state, hit_test.char_idx)?;

    Some(EditorPointerSelectionUpdate {
        hit_test,
        selection,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        EditorPointerSelectionUpdate, EditorSelectionDragState, EditorSelectionUpdate,
        pointer_selection_anchor, selection_for_range,
    };

    #[test]
    fn pointer_anchor_uses_hit_for_normal_clicks() {
        assert_eq!(pointer_selection_anchor(12, 4, false), 12);
    }

    #[test]
    fn pointer_anchor_uses_primary_anchor_for_extension() {
        assert_eq!(pointer_selection_anchor(12, 4, true), 4);
    }

    #[test]
    fn selection_for_range_clamps_to_document_length() {
        let (selection, update) = selection_for_range(10, 2, 40);

        assert_eq!(
            update,
            EditorSelectionUpdate {
                anchor: 2,
                head: 10
            }
        );
        let range = selection.primary();
        assert_eq!(range.anchor, 2);
        assert_eq!(range.head, 10);
    }

    #[test]
    fn drag_state_tracks_and_clears_anchor() {
        let state = EditorSelectionDragState::default();

        assert_eq!(state.anchor(), None);

        state.set_anchor(42);
        assert_eq!(state.anchor(), Some(42));

        state.clear();
        assert_eq!(state.anchor(), None);
    }

    #[test]
    fn pointer_selection_update_carries_hit_test_and_selection() {
        let update = EditorPointerSelectionUpdate {
            hit_test: crate::EditorHitTestResult {
                line_idx: 1,
                char_offset: 2,
                char_idx: 12,
            },
            selection: EditorSelectionUpdate {
                anchor: 4,
                head: 12,
            },
        };

        assert_eq!(update.hit_test.char_idx, update.selection.head);
    }
}
