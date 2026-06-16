// ABOUTME: Native editor render snapshot planning
// ABOUTME: Collects cursor and viewport facts used by GPUI paint paths

use helix_core::RopeSlice;
use helix_view::{Document, ViewId};

use crate::{
    CursorViewportPosition, LineViewportPlan, cursor_document_line, cursor_viewport_position,
    line_viewport_plan,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorRenderSnapshot {
    pub cursor_char_idx: usize,
    pub cursor_line: usize,
    pub cursor_lines: Vec<usize>,
    pub line_viewport: LineViewportPlan,
    pub last_row: usize,
    pub cursor_doc_line: usize,
    pub cursor_viewport_position: Option<CursorViewportPosition>,
}

pub fn document_render_snapshot(
    document: &Document,
    view_id: ViewId,
    first_row: usize,
    last_row_from_scroll: usize,
) -> EditorRenderSnapshot {
    let text = document.text().slice(..);
    let selection = document.selection(view_id);
    let cursor_char_idx = selection.primary().cursor(text);
    let cursor_lines = selection
        .iter()
        .map(|range| range.cursor_line(text))
        .collect();

    render_snapshot_for_cursor(
        text,
        cursor_char_idx,
        cursor_lines,
        first_row,
        last_row_from_scroll,
    )
}

pub fn render_snapshot_for_cursor(
    text: RopeSlice<'_>,
    cursor_char_idx: usize,
    cursor_lines: Vec<usize>,
    first_row: usize,
    last_row_from_scroll: usize,
) -> EditorRenderSnapshot {
    let line_viewport = line_viewport_plan(text, first_row, last_row_from_scroll, cursor_char_idx);
    let last_row = line_viewport.last_row;
    let cursor_at_trailing_newline =
        line_viewport.cursor_at_end && line_viewport.file_ends_with_newline;
    let cursor_doc_line = cursor_document_line(text, cursor_char_idx, cursor_at_trailing_newline);
    let cursor_viewport_position =
        cursor_viewport_position(cursor_doc_line, first_row, last_row_from_scroll);
    let cursor_line = text.char_to_line(cursor_char_idx.min(text.len_chars()));

    EditorRenderSnapshot {
        cursor_char_idx,
        cursor_line,
        cursor_lines,
        line_viewport,
        last_row,
        cursor_doc_line,
        cursor_viewport_position,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_tracks_visible_cursor() {
        let snapshot = render_snapshot_for_cursor("one\ntwo\nthree".into(), 5, vec![1], 0, 3);

        assert_eq!(snapshot.cursor_char_idx, 5);
        assert_eq!(snapshot.cursor_line, 1);
        assert_eq!(snapshot.cursor_doc_line, 1);
        assert_eq!(
            snapshot.cursor_viewport_position,
            Some(CursorViewportPosition {
                line: 1,
                viewport_row: 1
            })
        );
        assert_eq!(snapshot.cursor_lines, vec![1]);
        assert_eq!(snapshot.last_row, 3);
    }

    #[test]
    fn snapshot_reports_cursor_outside_viewport() {
        let snapshot = render_snapshot_for_cursor("one\ntwo\nthree".into(), 12, vec![2], 0, 1);

        assert_eq!(snapshot.cursor_doc_line, 2);
        assert_eq!(snapshot.cursor_viewport_position, None);
    }

    #[test]
    fn snapshot_extends_viewport_for_trailing_newline_cursor() {
        let snapshot = render_snapshot_for_cursor("one\n".into(), 4, vec![1], 0, 1);

        assert!(snapshot.line_viewport.cursor_at_end);
        assert!(snapshot.line_viewport.file_ends_with_newline);
        assert_eq!(snapshot.cursor_doc_line, 1);
        assert_eq!(snapshot.last_row, 2);
    }

    #[test]
    fn snapshot_does_not_make_trailing_newline_cursor_visible_outside_scroll_range() {
        let snapshot = render_snapshot_for_cursor("one\n".into(), 4, vec![1], 0, 1);

        assert_eq!(snapshot.last_row, 2);
        assert_eq!(snapshot.cursor_doc_line, 1);
        assert_eq!(snapshot.cursor_viewport_position, None);
    }
}
