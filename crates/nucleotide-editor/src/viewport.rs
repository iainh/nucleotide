// ABOUTME: Native GPUI editor viewport state for pixel and visual-row scrolling
// ABOUTME: Owns GUI scroll state before it is synced into Helix view offsets

use std::{cell::Cell, rc::Rc};

use gpui::{Bounds, Pixels, Point, Size, point, px, size};
use helix_core::{
    RopeSlice, char_idx_at_visual_offset, doc_formatter::TextFormat,
    text_annotations::TextAnnotations, visual_offset_from_block,
};
use helix_view::{Document, DocumentId, Editor, Theme, ViewId, graphics::Rect, view::ViewPosition};
use nucleotide_logging::debug;

use crate::{
    EDITOR_MINIMUM_VIEWPORT_COLUMNS, EditorDocumentMetrics, ScrollManager,
    soft_wrap_visual_position,
};

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
    pub cursor_revealed: bool,
    pub helix_snapshot: HelixViewportSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorViewportContentUpdate {
    pub gutter_columns: u16,
    pub visual_rows: usize,
    pub soft_wrap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorCursorReveal {
    Scrolloff,
    Center,
}

#[derive(Debug, Clone, Copy)]
pub struct EditorViewportContentLayout<'a> {
    pub theme: Option<&'a Theme>,
    pub bounds: Bounds<Pixels>,
    pub cell_width: Pixels,
    pub minimum_columns: u16,
}

impl<'a> EditorViewportContentLayout<'a> {
    pub fn for_editor(
        theme: Option<&'a Theme>,
        bounds: Bounds<Pixels>,
        cell_width: Pixels,
    ) -> Self {
        Self {
            theme,
            bounds,
            cell_width,
            minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EditorViewportSurfaceLayout<'a> {
    pub theme: Option<&'a Theme>,
    pub bounds: Bounds<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub minimum_columns: u16,
    pub cursor_reveal: Option<EditorCursorReveal>,
}

impl<'a> EditorViewportSurfaceLayout<'a> {
    pub fn for_editor(
        theme: Option<&'a Theme>,
        bounds: Bounds<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        cursor_reveal: Option<EditorCursorReveal>,
    ) -> Self {
        Self {
            theme,
            bounds,
            cell_width,
            line_height,
            minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            cursor_reveal,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EditorViewport {
    scroll: ScrollManager,
    cursor_reveal_request: Rc<Cell<Option<EditorCursorReveal>>>,
}

impl EditorViewport {
    pub fn new(line_height: Pixels) -> Self {
        Self {
            scroll: ScrollManager::new(line_height),
            cursor_reveal_request: Rc::new(Cell::new(None)),
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
        self.scroll.line_height()
    }

    pub fn set_viewport_size(&mut self, size: Size<Pixels>) {
        self.scroll.set_viewport_size(size);
    }

    pub fn set_content_visual_rows(&mut self, rows: usize) {
        self.scroll.set_total_lines(rows.max(1));
    }

    pub fn content_visual_rows(&self) -> usize {
        self.scroll.total_lines()
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
        Bounds::new(point(px(0.0), px(0.0)), self.scroll.viewport_size())
    }

    pub fn has_pending_view_sync(&self) -> bool {
        self.scroll.has_pending_view_sync()
    }

    pub fn clear_pending_view_sync(&self) {
        self.scroll.clear_pending_view_sync();
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

    pub fn request_cursor_reveal(&self, reveal: EditorCursorReveal) {
        self.cursor_reveal_request.set(Some(reveal));
    }

    pub fn pending_cursor_reveal_request(&self) -> Option<EditorCursorReveal> {
        self.cursor_reveal_request.get()
    }

    pub fn take_cursor_reveal_request(&self) -> Option<EditorCursorReveal> {
        self.cursor_reveal_request.replace(None)
    }

    pub fn ensure_visual_row_visible(
        &self,
        visual_row: usize,
        scrolloff: usize,
    ) -> ViewportScrollUpdate {
        self.reveal_visual_row(visual_row, EditorCursorReveal::Scrolloff, scrolloff)
    }

    pub fn reveal_visual_row(
        &self,
        visual_row: usize,
        reveal: EditorCursorReveal,
        scrolloff: usize,
    ) -> ViewportScrollUpdate {
        let old_position = self.scroll_position();
        let old_top_visual_row = self.top_visual_row();
        let visible_rows = self.visible_visual_rows();

        let target_top = match reveal {
            EditorCursorReveal::Scrolloff => {
                let margin = scrolloff.min(visible_rows.saturating_sub(1) / 2);
                let top_visual_row = self.top_visual_row();
                let lower_bound = top_visual_row.saturating_add(margin);
                let upper_bound =
                    top_visual_row.saturating_add(visible_rows.saturating_sub(margin));

                if visual_row < lower_bound {
                    Some(visual_row.saturating_sub(margin))
                } else if visual_row >= upper_bound {
                    Some(
                        visual_row
                            .saturating_add(margin)
                            .saturating_add(1)
                            .saturating_sub(visible_rows),
                    )
                } else {
                    None
                }
            }
            EditorCursorReveal::Center => Some(visual_row.saturating_sub(visible_rows / 2)),
        };

        if let Some(target_top) = target_top {
            self.scroll
                .set_scroll_position(point(px(0.0), self.scroll.anchor_to_pixels(target_top)));
        }

        let new_position = self.scroll_position();
        let new_top_visual_row = self.top_visual_row();

        ViewportScrollUpdate {
            changed: old_position != new_position,
            crossed_visual_rows: new_top_visual_row as isize - old_top_visual_row as isize,
            top_visual_row: new_top_visual_row,
            offset_within_row: self.offset_within_row(),
        }
    }

    pub fn visible_visual_rows(&self) -> usize {
        let line_height = self.line_height();
        if f32::from(line_height) <= 0.0 {
            return 1;
        }

        ((self.scroll.viewport_size().height / line_height).floor() as usize).max(1)
    }

    pub fn sync_from_helix_top_visual_row(&self, top_visual_row: usize) {
        let y = self.scroll.anchor_to_pixels(top_visual_row);
        self.scroll
            .set_scroll_position_from_view_sync_preserving_subrow_offset(point(px(0.0), y));
    }

    pub fn sync_from_helix_view(
        &self,
        document: &Document,
        view: &helix_view::View,
        view_id: ViewId,
        text_format: &TextFormat,
    ) -> HelixViewportSnapshot {
        let annotations = view.text_annotations(document, None);
        let snapshot = helix_viewport_snapshot(
            document.text().slice(..),
            document.view_offset(view_id),
            text_format,
            &annotations,
        );
        self.sync_from_helix_top_visual_row(snapshot.top_visual_row);
        snapshot
    }

    pub fn sync_to_helix_view(
        &self,
        editor: &mut Editor,
        doc_id: DocumentId,
        view_id: ViewId,
        text_format: &TextFormat,
    ) -> bool {
        let Some(view) = editor.tree.try_get(view_id).cloned() else {
            return false;
        };

        let Some(doc) = editor.document_mut(doc_id) else {
            return false;
        };

        let top_visual_row = self.top_visual_row();
        let mut view_offset = doc.view_offset(view_id);
        let (anchor, vertical_offset) = {
            let doc_text = doc.text().slice(..);
            let annotations = view.text_annotations(doc, None);
            let (anchor, vertical_offset) = char_idx_at_visual_offset(
                doc_text,
                0,
                top_visual_row as isize,
                0,
                text_format,
                &annotations,
            );
            (anchor, vertical_offset)
        };

        if view_offset.anchor == anchor
            && view_offset.vertical_offset == vertical_offset
            && (!text_format.soft_wrap || view_offset.horizontal_offset == 0)
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
        if text_format.soft_wrap {
            view_offset.horizontal_offset = 0;
        }
        doc.set_view_offset(view_id, view_offset);
        true
    }

    pub fn sync_content_layout(
        &mut self,
        document: &Document,
        view: &helix_view::View,
        layout: EditorViewportContentLayout<'_>,
    ) -> EditorViewportContentUpdate {
        let (gutter_columns, metrics) = editor_viewport_content_metrics(document, view, layout);
        self.set_content_visual_rows(metrics.visual_rows);

        EditorViewportContentUpdate {
            gutter_columns,
            visual_rows: metrics.visual_rows,
            soft_wrap: metrics.soft_wrap,
        }
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

        let view = editor.tree.try_get(view_id)?;
        let document = editor.document(doc_id)?;
        let (gutter_columns, metrics) = editor_viewport_content_metrics(
            document,
            view,
            EditorViewportContentLayout {
                theme: layout.theme,
                bounds: layout.bounds,
                cell_width: layout.cell_width,
                minimum_columns: layout.minimum_columns,
            },
        );
        self.set_content_visual_rows(metrics.visual_rows);
        sync_helix_view_area(
            editor,
            view_id,
            gutter_columns,
            metrics.viewport_columns,
            self.visible_visual_rows(),
        );

        let mut helix_view_synced = if self.has_pending_view_sync() {
            let synced = self.sync_to_helix_view(editor, doc_id, view_id, &metrics.text_format);
            self.clear_pending_view_sync();
            synced
        } else {
            false
        };

        let cursor_revealed = if let Some(cursor_reveal) = layout.cursor_reveal {
            let scrolloff = editor.config().scrolloff;
            let cursor_visual_row = {
                let view = editor.tree.try_get(view_id)?;
                let document = editor.document(doc_id)?;
                document_cursor_visual_row(document, view, view_id, &metrics.text_format)
            };
            let scroll_update = self.reveal_visual_row(cursor_visual_row, cursor_reveal, scrolloff);

            helix_view_synced |=
                self.sync_to_helix_view(editor, doc_id, view_id, &metrics.text_format);

            scroll_update.changed
        } else {
            false
        };

        let view = editor.tree.try_get(view_id)?;
        let document = editor.document(doc_id)?;
        let helix_snapshot =
            self.sync_from_helix_view(document, view, view_id, &metrics.text_format);

        Some(EditorViewportSurfaceUpdate {
            gutter_columns,
            visual_rows: metrics.visual_rows,
            soft_wrap: metrics.soft_wrap,
            helix_view_synced,
            cursor_revealed,
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
        let position = self.scroll_position();
        let viewport = self.scroll.viewport_size();
        let first_row = self.scroll.pixels_to_anchor(position.y);
        let last_row = self
            .scroll
            .pixels_to_anchor(position.y + viewport.height)
            .saturating_add(1)
            .min(self.content_visual_rows());

        (first_row, last_row)
    }
}

fn editor_viewport_content_metrics(
    document: &Document,
    view: &helix_view::View,
    layout: EditorViewportContentLayout<'_>,
) -> (u16, EditorDocumentMetrics) {
    let gutter_columns = view.gutter_offset(document);
    let metrics = EditorDocumentMetrics::resolve(
        document,
        layout.theme,
        layout.bounds,
        gutter_columns,
        layout.cell_width,
        layout.minimum_columns,
    );

    (gutter_columns, metrics)
}

fn sync_helix_view_area(
    editor: &mut Editor,
    view_id: ViewId,
    gutter_columns: u16,
    viewport_columns: u16,
    visible_rows: usize,
) -> bool {
    let target_area = helix_view_area_for_surface(gutter_columns, viewport_columns, visible_rows);
    let Some(current_area) = editor.tree.try_get(view_id).map(|view| view.area) else {
        return false;
    };
    if current_area == target_area {
        return false;
    }

    debug!(
        view_id = ?view_id,
        old_area = ?current_area,
        new_area = ?target_area,
        "Syncing native viewport dimensions to Helix view area"
    );
    editor.tree.get_mut(view_id).area = target_area;
    true
}

fn helix_view_area_for_surface(
    gutter_columns: u16,
    viewport_columns: u16,
    visible_rows: usize,
) -> Rect {
    let width = gutter_columns.saturating_add(viewport_columns).max(1);
    let height = u16::try_from(visible_rows.saturating_add(1))
        .unwrap_or(u16::MAX)
        .max(1);

    Rect::new(0, 0, width, height)
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
    text_format: &TextFormat,
    annotations: &TextAnnotations<'_>,
) -> HelixViewportSnapshot {
    let anchor = view_offset.anchor.min(text.len_chars());
    let anchor_line = text.char_to_line(anchor);
    let anchor_visual_row = visual_offset_from_block(text, 0, anchor, text_format, annotations)
        .0
        .row;
    let top_visual_row = anchor_visual_row.saturating_add(view_offset.vertical_offset);

    HelixViewportSnapshot {
        anchor_line,
        vertical_offset: view_offset.vertical_offset,
        top_visual_row,
    }
}

pub fn document_cursor_visual_row(
    document: &Document,
    view: &helix_view::View,
    view_id: ViewId,
    text_format: &TextFormat,
) -> usize {
    let text = document.text().slice(..);
    let cursor_char_idx = document.selection(view_id).primary().cursor(text);
    let cursor_at_trailing_newline = cursor_char_idx == text.len_chars()
        && text.len_chars() > 0
        && text.char(text.len_chars() - 1) == '\n';

    if cursor_at_trailing_newline {
        return if text_format.soft_wrap {
            soft_wrap_visual_position(text, text_format, 0, cursor_char_idx)
                .map(|position| position.visual_line)
                .unwrap_or_else(|| text.len_lines().saturating_sub(1))
        } else {
            text.len_lines().saturating_sub(1)
        };
    }

    let annotations = view.text_annotations(document, None);
    visual_offset_from_block(text, 0, cursor_char_idx, text_format, &annotations)
        .0
        .row
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::{ArcSwap, access::Map};
    use gpui::{Bounds, point, px, size};
    use helix_core::{
        Rope, Selection, Transaction, doc_formatter::TextFormat, syntax,
        text_annotations::TextAnnotations,
    };
    use helix_view::{
        Document, DocumentId, Editor, View,
        editor::Action,
        editor::{Config, GutterConfig},
        graphics::Rect,
        handlers::Handlers,
        theme,
        view::ViewPosition,
    };

    use super::*;

    fn default_annotations() -> TextAnnotations<'static> {
        TextAnnotations::default()
    }

    fn test_document_and_view(text: &str) -> (Document, View) {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let mut document = Document::from(Rope::from(text), None, config, syntax_loader);
        let view = View::new(DocumentId::default(), GutterConfig::default());
        document.ensure_view_init(view.id);

        (document, view)
    }

    fn test_handlers() -> Handlers {
        let (completion_tx, _) = tokio::sync::mpsc::channel(1);
        let (signature_tx, _) = tokio::sync::mpsc::channel(1);
        let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);

        Handlers {
            completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            word_index: helix_view::handlers::word_index::Handler::spawn(),
        }
    }

    fn test_editor_with_text(text: &str) -> (Editor, DocumentId, ViewId) {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| config)),
            test_handlers(),
        );
        let doc_id = editor.new_file(Action::VerticalSplit);
        let view_id = editor.tree.focus;
        let doc = editor.document_mut(doc_id).unwrap();
        let transaction = Transaction::change(doc.text(), [(0, 0, Some(text.into()))].into_iter());
        doc.apply(&transaction, view_id);

        (editor, doc_id, view_id)
    }

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
    fn viewport_cursor_reveal_requests_are_shared_across_clones() {
        let viewport = EditorViewport::new(px(20.0));
        let clone = viewport.clone();

        viewport.request_cursor_reveal(EditorCursorReveal::Scrolloff);

        assert_eq!(
            clone.pending_cursor_reveal_request(),
            Some(EditorCursorReveal::Scrolloff)
        );
        assert_eq!(
            clone.take_cursor_reveal_request(),
            Some(EditorCursorReveal::Scrolloff)
        );
        assert_eq!(viewport.pending_cursor_reveal_request(), None);
    }

    #[test]
    fn viewport_cursor_reveal_requests_use_latest_request() {
        let viewport = EditorViewport::new(px(20.0));

        viewport.request_cursor_reveal(EditorCursorReveal::Scrolloff);
        viewport.request_cursor_reveal(EditorCursorReveal::Center);

        assert_eq!(
            viewport.take_cursor_reveal_request(),
            Some(EditorCursorReveal::Center)
        );
        assert_eq!(viewport.take_cursor_reveal_request(), None);
    }

    #[test]
    fn viewport_cursor_reveal_keeps_visible_rows_unchanged() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(100.0)), 100);
        viewport.sync_from_helix_top_visual_row(10);

        let update = viewport.ensure_visual_row_visible(12, 1);

        assert!(!update.changed);
        assert_eq!(update.crossed_visual_rows, 0);
        assert_eq!(viewport.top_visual_row(), 10);
        assert!(!viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_cursor_reveal_scrolls_above_margin() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(100.0)), 100);
        viewport.sync_from_helix_top_visual_row(10);

        let update = viewport.ensure_visual_row_visible(7, 2);

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, -5);
        assert_eq!(viewport.top_visual_row(), 5);
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_cursor_reveal_scrolls_below_margin() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(100.0)), 100);
        viewport.sync_from_helix_top_visual_row(10);

        let update = viewport.ensure_visual_row_visible(14, 1);

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, 1);
        assert_eq!(viewport.top_visual_row(), 11);
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_cursor_reveal_clamps_scrolloff_for_small_viewports() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(40.0)), 100);
        viewport.sync_from_helix_top_visual_row(10);

        let update = viewport.ensure_visual_row_visible(12, 10);

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, 1);
        assert_eq!(viewport.top_visual_row(), 11);
    }

    #[test]
    fn viewport_cursor_reveal_can_center_visual_row() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(100.0)), 100);
        viewport.sync_from_helix_top_visual_row(0);

        let update = viewport.reveal_visual_row(20, EditorCursorReveal::Center, 0);

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, 18);
        assert_eq!(viewport.top_visual_row(), 18);
        assert!(viewport.has_pending_view_sync());
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
    fn content_layout_sync_updates_visual_row_count_from_document_metrics() {
        let (document, view) = test_document_and_view("one\ntwo\nthree\n");
        let mut viewport = EditorViewport::new(px(20.0));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(80.0)));
        viewport.set_viewport_size(bounds.size);

        let update = viewport.sync_content_layout(
            &document,
            &view,
            EditorViewportContentLayout {
                theme: None,
                bounds,
                cell_width: px(8.0),
                minimum_columns: 1,
            },
        );

        let expected = EditorDocumentMetrics::resolve(
            &document,
            None,
            bounds,
            update.gutter_columns,
            px(8.0),
            1,
        );

        assert_eq!(update.visual_rows, expected.visual_rows);
        assert_eq!(update.soft_wrap, expected.soft_wrap);
        assert_eq!(viewport.content_visual_rows(), expected.visual_rows);
    }

    #[test]
    fn editor_layout_constructors_use_shared_minimum_columns() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(80.0)));

        let content_layout = EditorViewportContentLayout::for_editor(None, bounds, px(8.0));
        let surface_layout =
            EditorViewportSurfaceLayout::for_editor(None, bounds, px(8.0), px(20.0), None);

        assert_eq!(
            content_layout.minimum_columns,
            EDITOR_MINIMUM_VIEWPORT_COLUMNS
        );
        assert_eq!(
            surface_layout.minimum_columns,
            EDITOR_MINIMUM_VIEWPORT_COLUMNS
        );
    }

    #[test]
    fn helix_view_area_includes_gutter_and_status_row() {
        assert_eq!(
            helix_view_area_for_surface(4, 20, 5),
            Rect::new(0, 0, 24, 6)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn surface_layout_sync_updates_helix_view_area_from_native_cells() {
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\ntwo\nthree\n");
        let mut viewport = EditorViewport::new(px(20.0));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(101.0)));

        let update = viewport
            .sync_surface_layout(
                &mut editor,
                doc_id,
                view_id,
                EditorViewportSurfaceLayout {
                    theme: None,
                    bounds,
                    cell_width: px(8.0),
                    line_height: px(20.0),
                    minimum_columns: 1,
                    cursor_reveal: None,
                },
            )
            .unwrap();

        let document = editor.document(doc_id).unwrap();
        let expected = EditorDocumentMetrics::resolve(
            document,
            None,
            bounds,
            update.gutter_columns,
            px(8.0),
            1,
        );
        let view = editor.tree.get(view_id);

        assert_eq!(view.gutter_offset(document), update.gutter_columns);
        assert_eq!(view.inner_width(document), expected.viewport_columns);
        assert_eq!(view.inner_height(), viewport.visible_visual_rows());
        assert_eq!(
            view.area,
            helix_view_area_for_surface(
                update.gutter_columns,
                expected.viewport_columns,
                viewport.visible_visual_rows()
            )
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cursor_reveal_keeps_native_scroll_authoritative_when_helix_offset_is_stale() {
        let text = (0..50)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let (mut editor, doc_id, view_id) = test_editor_with_text(&text);
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(240.0), px(100.0)), 50);
        viewport.sync_from_helix_top_visual_row(10);

        {
            let doc = editor.document_mut(doc_id).unwrap();
            let cursor = doc.text().line_to_char(12);
            doc.set_selection(view_id, Selection::point(cursor));
            let stale_anchor = doc.text().line_to_char(20);
            doc.set_view_offset(
                view_id,
                ViewPosition {
                    anchor: stale_anchor,
                    vertical_offset: 0,
                    horizontal_offset: 0,
                },
            );
        }

        let update = viewport
            .sync_surface_layout(
                &mut editor,
                doc_id,
                view_id,
                EditorViewportSurfaceLayout {
                    theme: None,
                    bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(101.0))),
                    cell_width: px(8.0),
                    line_height: px(20.0),
                    minimum_columns: 1,
                    cursor_reveal: Some(EditorCursorReveal::Scrolloff),
                },
            )
            .unwrap();

        assert!(!update.cursor_revealed);
        assert!(update.helix_view_synced);
        assert_eq!(viewport.top_visual_row(), 10);
        assert_eq!(update.helix_snapshot.top_visual_row, 10);

        let doc = editor.document(doc_id).unwrap();
        assert_eq!(doc.text().char_to_line(doc.view_offset(view_id).anchor), 10);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn surface_layout_roundtrips_scroll_to_trailing_newline_eof() {
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\ntwo\n");
        let mut viewport = EditorViewport::new(px(20.0));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(21.0)));
        let layout = EditorViewportSurfaceLayout {
            theme: None,
            bounds,
            cell_width: px(8.0),
            line_height: px(20.0),
            minimum_columns: 1,
            cursor_reveal: None,
        };

        viewport
            .sync_surface_layout(&mut editor, doc_id, view_id, layout)
            .unwrap();
        let eof_row = viewport.content_visual_rows().saturating_sub(1);
        viewport.scroll_to_vertical_position_from_scrollbar(viewport.max_scroll_offset().height);

        let update = viewport
            .sync_surface_layout(&mut editor, doc_id, view_id, layout)
            .unwrap();

        assert!(update.helix_view_synced);
        assert_eq!(viewport.top_visual_row(), eof_row);
        assert_eq!(update.helix_snapshot.top_visual_row, eof_row);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cursor_reveal_uses_trailing_newline_eof_visual_row() {
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\ntwo\n");
        let mut viewport = EditorViewport::new(px(20.0));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(21.0)));
        {
            let doc = editor.document_mut(doc_id).unwrap();
            doc.set_selection(view_id, Selection::point(doc.text().len_chars()));
        }

        let update = viewport
            .sync_surface_layout(
                &mut editor,
                doc_id,
                view_id,
                EditorViewportSurfaceLayout {
                    theme: None,
                    bounds,
                    cell_width: px(8.0),
                    line_height: px(20.0),
                    minimum_columns: 1,
                    cursor_reveal: Some(EditorCursorReveal::Scrolloff),
                },
            )
            .unwrap();

        let eof_row = viewport.content_visual_rows().saturating_sub(1);
        assert!(update.cursor_revealed);
        assert!(update.helix_view_synced);
        assert_eq!(viewport.top_visual_row(), eof_row);
        assert_eq!(update.helix_snapshot.top_visual_row, eof_row);
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
        let text_format = TextFormat::default();
        let annotations = default_annotations();
        let snapshot = helix_viewport_snapshot(
            text.into(),
            ViewPosition {
                anchor: 4,
                vertical_offset: 2,
                horizontal_offset: 7,
            },
            &text_format,
            &annotations,
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
        let text_format = TextFormat::default();
        let annotations = default_annotations();
        let snapshot = helix_viewport_snapshot(
            text.into(),
            ViewPosition {
                anchor: 1_000,
                vertical_offset: 0,
                horizontal_offset: 0,
            },
            &text_format,
            &annotations,
        );

        assert_eq!(snapshot.anchor_line, 1);
        assert_eq!(snapshot.top_visual_row, 1);
    }

    #[test]
    fn helix_viewport_snapshot_uses_soft_wrap_visual_rows() {
        let text = "abcdef\nzz";
        let text_format = TextFormat {
            soft_wrap: true,
            viewport_width: 3,
            ..TextFormat::default()
        };
        let annotations = default_annotations();
        let snapshot = helix_viewport_snapshot(
            text.into(),
            ViewPosition {
                anchor: 7,
                vertical_offset: 0,
                horizontal_offset: 0,
            },
            &text_format,
            &annotations,
        );

        assert_eq!(snapshot.anchor_line, 1);
        assert!(snapshot.top_visual_row > snapshot.anchor_line);
    }

    #[test]
    fn helix_viewport_snapshot_adds_vertical_offset_to_visual_row() {
        let text = "abcdef\nzz";
        let text_format = TextFormat {
            soft_wrap: true,
            viewport_width: 3,
            ..TextFormat::default()
        };
        let annotations = default_annotations();
        let snapshot = helix_viewport_snapshot(
            text.into(),
            ViewPosition {
                anchor: 7,
                vertical_offset: 2,
                horizontal_offset: 0,
            },
            &text_format,
            &annotations,
        );

        assert_eq!(snapshot.anchor_line, 1);
        assert!(snapshot.top_visual_row >= snapshot.anchor_line + 2);
    }

    #[test]
    fn document_cursor_visual_row_matches_unwrapped_line() {
        let (mut document, view) = test_document_and_view("one\ntwo\nthree");
        document.set_selection(view.id, Selection::single(5, 5));

        let row = document_cursor_visual_row(&document, &view, view.id, &TextFormat::default());

        assert_eq!(row, 1);
    }

    #[test]
    fn document_cursor_visual_row_uses_final_empty_line_at_trailing_newline_eof() {
        let (mut document, view) = test_document_and_view("one\n");
        document.set_selection(view.id, Selection::single(4, 4));

        let row = document_cursor_visual_row(&document, &view, view.id, &TextFormat::default());

        assert_eq!(row, 1);
    }

    #[test]
    fn document_cursor_visual_row_uses_soft_wrap_rows() {
        let (mut document, view) = test_document_and_view("abcdef\nzz");
        document.set_selection(view.id, Selection::single(7, 7));
        let text_format = TextFormat {
            soft_wrap: true,
            viewport_width: 3,
            ..TextFormat::default()
        };

        let row = document_cursor_visual_row(&document, &view, view.id, &text_format);

        assert!(row > 1);
    }
}
