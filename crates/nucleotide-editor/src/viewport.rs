// ABOUTME: Native GPUI editor viewport state for pixel and visual-row scrolling
// ABOUTME: Owns GUI scroll state before it is synced into Helix view offsets

use gpui::{Bounds, Pixels, Point, Size, point, px, size};
use helix_core::{RopeSlice, char_idx_at_visual_offset};
use helix_view::{Document, DocumentId, Editor, Theme, ViewId, view::ViewPosition};
use nucleotide_logging::debug;

use crate::{EditorDocumentMetrics, ScrollManager};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportScrollUpdate {
    pub changed: bool,
    pub crossed_visual_rows: isize,
    pub top_visual_row: usize,
    pub offset_within_row: Pixels,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelixViewportSnapshot {
    pub anchor_line: usize,
    pub vertical_offset: usize,
    pub top_visual_row: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorViewportSurfaceUpdate {
    pub gutter_columns: u16,
    pub visual_rows: usize,
    pub soft_wrap: bool,
    pub helix_view_synced: bool,
    pub helix_snapshot: HelixViewportSnapshot,
}

#[derive(Debug, Clone, Copy)]
pub struct EditorViewportSurfaceLayout<'a> {
    pub theme: Option<&'a Theme>,
    pub bounds: Bounds<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub minimum_columns: u16,
}

#[derive(Clone, Debug)]
pub struct EditorViewport {
    scroll: ScrollManager,
}

impl EditorViewport {
    pub fn new(line_height: Pixels) -> Self {
        Self {
            scroll: ScrollManager::new(line_height),
        }
    }

    pub fn set_layout(
        &mut self,
        line_height: Pixels,
        viewport_size: Size<Pixels>,
        content_visual_rows: usize,
    ) {
        self.set_line_height(line_height);
        self.set_viewport_size(viewport_size);
        self.set_content_visual_rows(content_visual_rows);
    }

    pub fn set_line_height(&mut self, line_height: Pixels) {
        self.scroll.set_line_height(line_height);
    }

    pub fn line_height(&self) -> Pixels {
        self.scroll.line_height.get()
    }

    pub fn set_viewport_size(&mut self, size: Size<Pixels>) {
        self.scroll.set_viewport_size(size);
    }

    pub fn set_content_visual_rows(&mut self, rows: usize) {
        self.scroll.set_total_lines(rows.max(1));
    }

    pub fn content_visual_rows(&self) -> usize {
        self.scroll.total_lines.get()
    }

    pub fn max_scroll_offset(&self) -> Size<Pixels> {
        self.scroll.max_scroll_offset()
    }

    pub fn scroll_position(&self) -> Point<Pixels> {
        self.scroll.scroll_position()
    }

    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll.scroll_offset()
    }

    pub fn set_scroll_offset_from_scrollbar(&self, offset: Point<Pixels>) {
        self.scroll.set_scroll_offset(offset);
    }

    pub fn viewport_bounds(&self) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), self.scroll.viewport_size.get())
    }

    pub fn has_pending_view_sync(&self) -> bool {
        self.scroll.pending_view_sync.get()
    }

    pub fn clear_pending_view_sync(&self) {
        self.scroll.pending_view_sync.set(false);
    }

    pub fn scroll_by_delta(&self, delta: Point<Pixels>) -> ViewportScrollUpdate {
        let (changed, crossed_visual_rows) = self.scroll.scroll_by_delta(delta);

        ViewportScrollUpdate {
            changed,
            crossed_visual_rows,
            top_visual_row: self.top_visual_row(),
            offset_within_row: self.offset_within_row(),
        }
    }

    pub fn scroll_to_vertical_position_from_scrollbar(&self, y: Pixels) -> ViewportScrollUpdate {
        let old_position = self.scroll_position();
        let old_top_visual_row = self.top_visual_row();

        self.scroll.set_scroll_position(point(px(0.0), y));

        let new_position = self.scroll_position();
        let new_top_visual_row = self.top_visual_row();

        ViewportScrollUpdate {
            changed: old_position != new_position,
            crossed_visual_rows: new_top_visual_row as isize - old_top_visual_row as isize,
            top_visual_row: new_top_visual_row,
            offset_within_row: self.offset_within_row(),
        }
    }

    pub fn sync_from_helix_top_visual_row(&self, top_visual_row: usize) {
        let y = self.scroll.anchor_to_pixels(top_visual_row);
        self.scroll
            .set_scroll_position_from_helix_preserving_intra_line_offset(point(px(0.0), y));
    }

    pub fn sync_from_helix_view(
        &self,
        document: &Document,
        view_id: ViewId,
    ) -> HelixViewportSnapshot {
        let snapshot =
            helix_viewport_snapshot(document.text().slice(..), document.view_offset(view_id));
        self.sync_from_helix_top_visual_row(snapshot.top_visual_row);
        snapshot
    }

    pub fn sync_to_helix_view(
        &self,
        editor: &mut Editor,
        doc_id: DocumentId,
        view_id: ViewId,
    ) -> bool {
        let Some(view) = editor.tree.try_get(view_id).cloned() else {
            return false;
        };

        let Some(doc) = editor.document_mut(doc_id) else {
            return false;
        };

        let top_visual_row = self.top_visual_row();
        let mut view_offset = doc.view_offset(view_id);
        let (anchor, vertical_offset, soft_wrap) = {
            let doc_text = doc.text().slice(..);
            let viewport = view.inner_area(doc);
            let text_fmt = doc.text_format(viewport.width.max(1), None);
            let annotations = view.text_annotations(doc, None);
            let (anchor, vertical_offset) = char_idx_at_visual_offset(
                doc_text,
                0,
                top_visual_row as isize,
                0,
                &text_fmt,
                &annotations,
            );
            (anchor, vertical_offset, text_fmt.soft_wrap)
        };

        if view_offset.anchor == anchor
            && view_offset.vertical_offset == vertical_offset
            && (!soft_wrap || view_offset.horizontal_offset == 0)
        {
            return false;
        }

        debug!(
            view_id = ?view_id,
            top_visual_row,
            old_anchor = view_offset.anchor,
            new_anchor = anchor,
            old_vertical_offset = view_offset.vertical_offset,
            new_vertical_offset = vertical_offset,
            "Syncing GUI scroll position to Helix view"
        );

        view_offset.anchor = anchor;
        view_offset.vertical_offset = vertical_offset;
        if soft_wrap {
            view_offset.horizontal_offset = 0;
        }
        doc.set_view_offset(view_id, view_offset);
        true
    }

    pub fn sync_surface_layout(
        &mut self,
        editor: &mut Editor,
        doc_id: DocumentId,
        view_id: ViewId,
        layout: EditorViewportSurfaceLayout<'_>,
    ) -> Option<EditorViewportSurfaceUpdate> {
        self.set_line_height(layout.line_height);
        self.set_viewport_size(editor_viewport_size_for_bounds(layout.bounds));

        let helix_view_synced = if self.has_pending_view_sync() {
            let synced = self.sync_to_helix_view(editor, doc_id, view_id);
            self.clear_pending_view_sync();
            synced
        } else {
            false
        };

        let view = editor.tree.try_get(view_id)?;
        let document = editor.document(doc_id)?;
        let gutter_columns = view.gutter_offset(document);
        let metrics = EditorDocumentMetrics::resolve(
            document,
            layout.theme,
            layout.bounds,
            gutter_columns,
            layout.cell_width,
            layout.minimum_columns,
        );
        self.set_content_visual_rows(metrics.visual_rows);
        let helix_snapshot = self.sync_from_helix_view(document, view_id);

        Some(EditorViewportSurfaceUpdate {
            gutter_columns,
            visual_rows: metrics.visual_rows,
            soft_wrap: metrics.soft_wrap,
            helix_view_synced,
            helix_snapshot,
        })
    }

    pub fn top_visual_row(&self) -> usize {
        self.scroll.pixels_to_anchor(self.scroll_position().y)
    }

    pub fn offset_within_row(&self) -> Pixels {
        self.scroll.vertical_offset_within_line()
    }

    pub fn visible_visual_range(&self) -> (usize, usize) {
        self.scroll.visible_line_range()
    }
}

pub fn editor_viewport_size_for_bounds(bounds: Bounds<Pixels>) -> Size<Pixels> {
    size(
        bounds.size.width,
        (bounds.size.height - px(1.0)).max(px(0.0)),
    )
}

pub fn helix_viewport_snapshot(
    text: RopeSlice<'_>,
    view_offset: ViewPosition,
) -> HelixViewportSnapshot {
    let anchor = view_offset.anchor.min(text.len_chars());
    let anchor_line = text.char_to_line(anchor);
    let top_visual_row = anchor_line.saturating_add(view_offset.vertical_offset);

    HelixViewportSnapshot {
        anchor_line,
        vertical_offset: view_offset.vertical_offset,
        top_visual_row,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};
    use helix_view::view::ViewPosition;

    use super::*;

    #[test]
    fn viewport_reports_subrow_wheel_scroll() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(400.0)), 100);

        let update = viewport.scroll_by_delta(point(px(0.0), px(-5.0)));

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, 0);
        assert_eq!(update.top_visual_row, 0);
        assert_eq!(update.offset_within_row, px(5.0));
        assert!(!viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_reports_crossed_visual_rows() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(400.0)), 100);

        let update = viewport.scroll_by_delta(point(px(0.0), px(-25.0)));

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, 1);
        assert_eq!(update.top_visual_row, 1);
        assert_eq!(update.offset_within_row, px(5.0));
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_reports_scrollbar_position_changes() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(400.0)), 100);

        let update = viewport.scroll_to_vertical_position_from_scrollbar(px(65.0));

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, 3);
        assert_eq!(update.top_visual_row, 3);
        assert_eq!(update.offset_within_row, px(5.0));
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_clamps_scrollbar_position() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(100.0)), 10);

        let update = viewport.scroll_to_vertical_position_from_scrollbar(px(500.0));

        assert!(update.changed);
        assert_eq!(viewport.scroll_position().y, px(100.0));
        assert_eq!(update.top_visual_row, 5);
    }

    #[test]
    fn viewport_preserves_fractional_offset_for_same_helix_row() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(400.0)), 100);
        viewport.scroll_by_delta(point(px(0.0), px(-25.0)));

        viewport.sync_from_helix_top_visual_row(1);

        assert_eq!(viewport.top_visual_row(), 1);
        assert_eq!(viewport.offset_within_row(), px(5.0));
    }

    #[test]
    fn viewport_uses_visual_row_count_for_scroll_range() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(100.0)), 30);

        assert_eq!(viewport.content_visual_rows(), 30);
        assert_eq!(viewport.max_scroll_offset().height, px(500.0));
    }

    #[test]
    fn surface_viewport_size_uses_text_area_height() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(300.0), px(101.0)));

        assert_eq!(
            editor_viewport_size_for_bounds(bounds),
            size(px(300.0), px(100.0))
        );
    }

    #[test]
    fn surface_viewport_size_clamps_empty_height() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(300.0), px(0.5)));

        assert_eq!(
            editor_viewport_size_for_bounds(bounds),
            size(px(300.0), px(0.0))
        );
    }

    #[test]
    fn helix_viewport_snapshot_reports_top_visual_row() {
        let text = "one\ntwo\nthree";
        let snapshot = helix_viewport_snapshot(
            text.into(),
            ViewPosition {
                anchor: 4,
                vertical_offset: 2,
                horizontal_offset: 7,
            },
        );

        assert_eq!(
            snapshot,
            HelixViewportSnapshot {
                anchor_line: 1,
                vertical_offset: 2,
                top_visual_row: 3,
            }
        );
    }

    #[test]
    fn helix_viewport_snapshot_clamps_stale_anchor() {
        let text = "one\ntwo";
        let snapshot = helix_viewport_snapshot(
            text.into(),
            ViewPosition {
                anchor: 1_000,
                vertical_offset: 0,
                horizontal_offset: 0,
            },
        );

        assert_eq!(snapshot.anchor_line, 1);
        assert_eq!(snapshot.top_visual_row, 1);
    }
}
