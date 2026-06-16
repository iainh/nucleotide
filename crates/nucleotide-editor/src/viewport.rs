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
pub struct EditorViewportViewPositionPlan {
    pub top_visual_row: usize,
    pub previous_view_position: ViewPosition,
    pub view_position: ViewPosition,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorViewportViewAreaPlan {
    pub previous_area: Rect,
    pub target_area: Rect,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorViewportSurfaceUpdate {
    pub gutter_columns: u16,
    pub visual_rows: usize,
    pub soft_wrap: bool,
    pub view_area_plan: EditorViewportViewAreaPlan,
    pub view_position: ViewPosition,
    pub view_position_plan: EditorViewportViewPositionPlan,
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
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorViewportScrollDirection {
    Backward,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorViewportCursorTarget {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorViewportCursorRequest {
    pub target: EditorViewportCursorTarget,
    pub count: usize,
}

impl EditorViewportCursorRequest {
    pub fn target_visual_row(
        self,
        top_visual_row: usize,
        visible_rows: usize,
        content_visual_rows: usize,
        scrolloff: usize,
    ) -> usize {
        let visible_rows = visible_rows.max(1);
        let content_visual_rows = content_visual_rows.max(1);
        let last_content_row = content_visual_rows.saturating_sub(1);
        let last_visible_row = top_visual_row
            .saturating_add(visible_rows.saturating_sub(1))
            .min(last_content_row);
        let last_visible_offset = last_visible_row.saturating_sub(top_visual_row);
        let scrolloff = scrolloff.min(visible_rows.saturating_sub(1) / 2);
        let count_offset = self.count.max(1).saturating_sub(1);

        let target = match self.target {
            EditorViewportCursorTarget::Top => top_visual_row
                .saturating_add(scrolloff)
                .saturating_add(count_offset),
            EditorViewportCursorTarget::Center => {
                top_visual_row.saturating_add(last_visible_offset / 2)
            }
            EditorViewportCursorTarget::Bottom => top_visual_row.saturating_add(
                last_visible_offset.saturating_sub(scrolloff.saturating_add(count_offset)),
            ),
        };

        target
            .max(top_visual_row.saturating_add(scrolloff))
            .min(top_visual_row.saturating_add(last_visible_offset.saturating_sub(scrolloff)))
            .min(last_content_row)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorViewportScrollRequest {
    VisualRows(isize),
    VisualPages(isize),
    VisualPageFraction { pages: isize, divisor: usize },
    VisualPageWithCursor { pages: isize, divisor: usize },
    CursorReveal(EditorCursorReveal),
}

impl EditorViewportScrollRequest {
    pub fn page_cursor_sync_direction(self) -> Option<EditorViewportScrollDirection> {
        match self {
            Self::VisualPages(pages) if pages > 0 => Some(EditorViewportScrollDirection::Forward),
            Self::VisualPages(pages) if pages < 0 => Some(EditorViewportScrollDirection::Backward),
            Self::VisualPageFraction { pages, .. } if pages > 0 => {
                Some(EditorViewportScrollDirection::Forward)
            }
            Self::VisualPageFraction { pages, .. } if pages < 0 => {
                Some(EditorViewportScrollDirection::Backward)
            }
            _ => None,
        }
    }
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
    pub scrolloff: usize,
    pub cursor_reveal: Option<EditorCursorReveal>,
}

impl<'a> EditorViewportSurfaceLayout<'a> {
    pub fn for_editor(
        theme: Option<&'a Theme>,
        bounds: Bounds<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        scrolloff: usize,
        cursor_reveal: Option<EditorCursorReveal>,
    ) -> Self {
        Self {
            theme,
            bounds,
            cell_width,
            line_height,
            minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            scrolloff,
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

    pub fn apply_scroll_request(
        &self,
        request: EditorViewportScrollRequest,
    ) -> ViewportScrollUpdate {
        match request {
            EditorViewportScrollRequest::VisualRows(rows) => self.scroll_by_visual_rows(rows),
            EditorViewportScrollRequest::VisualPages(pages) => self.scroll_by_visual_pages(pages),
            EditorViewportScrollRequest::VisualPageFraction { pages, divisor }
            | EditorViewportScrollRequest::VisualPageWithCursor { pages, divisor } => {
                self.scroll_by_visual_page_fraction(pages, divisor)
            }
            EditorViewportScrollRequest::CursorReveal(reveal) => {
                self.request_cursor_reveal(reveal);
                ViewportScrollUpdate {
                    changed: false,
                    crossed_visual_rows: 0,
                    top_visual_row: self.top_visual_row(),
                    offset_within_row: self.offset_within_row(),
                }
            }
        }
    }

    pub fn scroll_by_visual_pages(&self, pages: isize) -> ViewportScrollUpdate {
        self.scroll_by_visual_page_fraction(pages, 1)
    }

    pub fn scroll_by_visual_page_fraction(
        &self,
        pages: isize,
        divisor: usize,
    ) -> ViewportScrollUpdate {
        let visible_rows = self.visible_visual_rows() / divisor.max(1);
        let visible_rows = isize::try_from(visible_rows).unwrap_or(isize::MAX);
        self.scroll_by_visual_rows(visible_rows.saturating_mul(pages))
    }

    pub fn scroll_by_visual_rows(&self, rows: isize) -> ViewportScrollUpdate {
        let old_position = self.scroll_position();
        let old_top_visual_row = self.top_visual_row();

        if rows != 0 {
            let delta_y = self.scroll.line_height() * rows as f32;
            self.scroll
                .set_scroll_position(point(old_position.x, old_position.y + delta_y));
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
            EditorCursorReveal::Top => Some(visual_row),
            EditorCursorReveal::Bottom => {
                Some(visual_row.saturating_sub(visible_rows.saturating_sub(1)))
            }
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

    pub fn sync_view_position(
        &self,
        document: &mut Document,
        view: &helix_view::View,
        view_id: ViewId,
        text_format: &TextFormat,
    ) -> bool {
        let plan = self.plan_view_position(document, view, view_id, text_format);
        if !plan.changed {
            return false;
        }

        debug!(
            view_id = ?view_id,
            top_visual_row = plan.top_visual_row,
            old_anchor = plan.previous_view_position.anchor,
            new_anchor = plan.view_position.anchor,
            old_vertical_offset = plan.previous_view_position.vertical_offset,
            new_vertical_offset = plan.view_position.vertical_offset,
            "Syncing GUI scroll position to Helix view"
        );

        apply_view_position_plan(document, view_id, plan)
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
        let mut view = editor.tree.try_get(view_id)?.clone();
        let update = {
            let document = editor.document_mut(doc_id)?;
            self.sync_surface_layout_for_view(document, &mut view, view_id, layout)
        };
        // Keep the resized view local to this surface. The Helix tree owns split
        // geometry; writing per-pane paint bounds back into it causes layout
        // feedback between independently rendered panes.

        Some(update)
    }

    pub fn sync_surface_layout_for_view(
        &mut self,
        document: &mut Document,
        view: &mut helix_view::View,
        view_id: ViewId,
        layout: EditorViewportSurfaceLayout<'_>,
    ) -> EditorViewportSurfaceUpdate {
        self.set_line_height(layout.line_height);
        self.set_viewport_size(editor_viewport_size_for_bounds(layout.bounds));

        let content_layout = EditorViewportContentLayout {
            theme: layout.theme,
            bounds: layout.bounds,
            cell_width: layout.cell_width,
            minimum_columns: layout.minimum_columns,
        };
        let (gutter_columns, metrics) = editor_viewport_surface_metrics(
            document,
            view,
            content_layout,
            self.visible_visual_rows(),
        );
        self.set_content_visual_rows(metrics.visual_rows);
        let view_area_plan = helix_view_area_plan_for_surface(
            view.area,
            gutter_columns,
            metrics.viewport_columns,
            self.visible_visual_rows(),
        );
        apply_helix_view_area_plan(view, view_area_plan);

        let mut helix_view_synced = if self.has_pending_view_sync() {
            let synced = self.sync_view_position(document, view, view_id, &metrics.text_format);
            self.clear_pending_view_sync();
            synced
        } else {
            false
        };

        let cursor_revealed = if let Some(cursor_reveal) = layout.cursor_reveal {
            let cursor_visual_row =
                { document_cursor_visual_row(document, view, view_id, &metrics.text_format) };
            let scroll_update =
                self.reveal_visual_row(cursor_visual_row, cursor_reveal, layout.scrolloff);

            helix_view_synced |=
                self.sync_view_position(document, view, view_id, &metrics.text_format);

            scroll_update.changed
        } else {
            false
        };

        let helix_snapshot =
            self.sync_from_helix_view(document, view, view_id, &metrics.text_format);
        let view_position_plan =
            self.plan_view_position(document, view, view_id, &metrics.text_format);
        let view_position = view_position_plan.view_position;

        EditorViewportSurfaceUpdate {
            gutter_columns,
            visual_rows: metrics.visual_rows,
            soft_wrap: metrics.soft_wrap,
            view_area_plan,
            view_position,
            view_position_plan,
            helix_view_synced,
            cursor_revealed,
            helix_snapshot,
        }
    }

    pub fn plan_view_position(
        &self,
        document: &Document,
        view: &helix_view::View,
        view_id: ViewId,
        text_format: &TextFormat,
    ) -> EditorViewportViewPositionPlan {
        let annotations = view.text_annotations(document, None);
        view_position_plan_for_top_visual_row(
            document.text().slice(..),
            document.view_offset(view_id),
            self.top_visual_row(),
            text_format,
            &annotations,
        )
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

fn editor_viewport_surface_metrics(
    document: &Document,
    view: &helix_view::View,
    layout: EditorViewportContentLayout<'_>,
    visible_rows: usize,
) -> (u16, EditorDocumentMetrics) {
    let (current_gutter_columns, current_metrics) =
        editor_viewport_content_metrics(document, view, layout);
    let mut surface_view = view.clone();
    surface_view.area =
        helix_view_area_for_surface(0, current_metrics.viewport_columns, visible_rows);
    let surface_gutter_columns = surface_view.gutter_offset(document);

    if surface_gutter_columns == current_gutter_columns {
        return (current_gutter_columns, current_metrics);
    }

    let metrics = EditorDocumentMetrics::resolve(
        document,
        layout.theme,
        layout.bounds,
        surface_gutter_columns,
        layout.cell_width,
        layout.minimum_columns,
    );

    (surface_gutter_columns, metrics)
}

fn apply_helix_view_area_plan(
    view: &mut helix_view::View,
    plan: EditorViewportViewAreaPlan,
) -> bool {
    if !plan.changed {
        return false;
    }

    debug!(
        old_area = ?plan.previous_area,
        new_area = ?plan.target_area,
        "Syncing native viewport dimensions to Helix view area"
    );
    view.area = plan.target_area;
    true
}

pub fn helix_view_area_plan_for_surface(
    previous_area: Rect,
    gutter_columns: u16,
    viewport_columns: u16,
    visible_rows: usize,
) -> EditorViewportViewAreaPlan {
    let target_area = helix_view_area_for_surface(gutter_columns, viewport_columns, visible_rows);

    EditorViewportViewAreaPlan {
        previous_area,
        target_area,
        changed: previous_area != target_area,
    }
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

pub fn view_position_for_top_visual_row(
    text: RopeSlice<'_>,
    top_visual_row: usize,
    horizontal_offset: usize,
    text_format: &TextFormat,
    annotations: &TextAnnotations<'_>,
) -> ViewPosition {
    let (anchor, vertical_offset) = char_idx_at_visual_offset(
        text,
        0,
        isize::try_from(top_visual_row).unwrap_or(isize::MAX),
        0,
        text_format,
        annotations,
    );

    ViewPosition {
        anchor,
        vertical_offset,
        horizontal_offset: if text_format.soft_wrap {
            0
        } else {
            horizontal_offset
        },
    }
}

pub fn view_position_plan_for_top_visual_row(
    text: RopeSlice<'_>,
    previous_view_position: ViewPosition,
    top_visual_row: usize,
    text_format: &TextFormat,
    annotations: &TextAnnotations<'_>,
) -> EditorViewportViewPositionPlan {
    let view_position = view_position_for_top_visual_row(
        text,
        top_visual_row,
        previous_view_position.horizontal_offset,
        text_format,
        annotations,
    );

    EditorViewportViewPositionPlan {
        top_visual_row,
        previous_view_position,
        view_position,
        changed: previous_view_position != view_position,
    }
}

fn apply_view_position_plan(
    document: &mut Document,
    view_id: ViewId,
    plan: EditorViewportViewPositionPlan,
) -> bool {
    if !plan.changed {
        return false;
    }

    document.set_view_offset(view_id, plan.view_position);
    true
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
    fn viewport_scroll_request_moves_by_visual_rows() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(100.0)), 50);

        let update = viewport.apply_scroll_request(EditorViewportScrollRequest::VisualRows(3));

        assert!(update.changed);
        assert_eq!(viewport.scroll_position().y, px(60.0));
        assert_eq!(update.crossed_visual_rows, 3);
        assert_eq!(update.top_visual_row, 3);
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_scroll_request_clamps_above_document_start() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(100.0)), 50);
        viewport.apply_scroll_request(EditorViewportScrollRequest::VisualRows(2));

        let update = viewport.apply_scroll_request(EditorViewportScrollRequest::VisualRows(-10));

        assert!(update.changed);
        assert_eq!(viewport.scroll_position().y, px(0.0));
        assert_eq!(update.crossed_visual_rows, -2);
        assert_eq!(update.top_visual_row, 0);
    }

    #[test]
    fn viewport_page_scroll_request_moves_by_visible_rows() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(100.0)), 50);

        let update = viewport.apply_scroll_request(EditorViewportScrollRequest::VisualPages(1));

        assert!(update.changed);
        assert_eq!(viewport.visible_visual_rows(), 5);
        assert_eq!(viewport.scroll_position().y, px(100.0));
        assert_eq!(update.crossed_visual_rows, 5);
        assert_eq!(update.top_visual_row, 5);
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_page_fraction_request_moves_by_fractional_visible_rows() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(100.0)), 50);

        let update =
            viewport.apply_scroll_request(EditorViewportScrollRequest::VisualPageFraction {
                pages: 1,
                divisor: 2,
            });

        assert!(update.changed);
        assert_eq!(viewport.visible_visual_rows(), 5);
        assert_eq!(viewport.scroll_position().y, px(40.0));
        assert_eq!(update.crossed_visual_rows, 2);
        assert_eq!(update.top_visual_row, 2);
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_page_cursor_sync_direction_excludes_cursor_page_requests() {
        assert_eq!(
            EditorViewportScrollRequest::VisualPageFraction {
                pages: 1,
                divisor: 2,
            }
            .page_cursor_sync_direction(),
            Some(EditorViewportScrollDirection::Forward)
        );
        assert_eq!(
            EditorViewportScrollRequest::VisualPageWithCursor {
                pages: 1,
                divisor: 2,
            }
            .page_cursor_sync_direction(),
            None
        );
    }

    #[test]
    fn viewport_cursor_request_resolves_top_center_and_bottom_rows() {
        assert_eq!(
            EditorViewportCursorRequest {
                target: EditorViewportCursorTarget::Top,
                count: 1,
            }
            .target_visual_row(10, 20, 100, 5),
            15
        );
        assert_eq!(
            EditorViewportCursorRequest {
                target: EditorViewportCursorTarget::Top,
                count: 3,
            }
            .target_visual_row(10, 20, 100, 5),
            17
        );
        assert_eq!(
            EditorViewportCursorRequest {
                target: EditorViewportCursorTarget::Center,
                count: 1,
            }
            .target_visual_row(10, 20, 100, 5),
            19
        );
        assert_eq!(
            EditorViewportCursorRequest {
                target: EditorViewportCursorTarget::Bottom,
                count: 1,
            }
            .target_visual_row(10, 20, 100, 5),
            24
        );
        assert_eq!(
            EditorViewportCursorRequest {
                target: EditorViewportCursorTarget::Bottom,
                count: 3,
            }
            .target_visual_row(10, 20, 100, 5),
            22
        );
    }

    #[test]
    fn viewport_scroll_request_can_defer_cursor_reveal() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(100.0), px(100.0)), 50);

        let update = viewport.apply_scroll_request(EditorViewportScrollRequest::CursorReveal(
            EditorCursorReveal::Center,
        ));

        assert!(!update.changed);
        assert_eq!(viewport.scroll_position().y, px(0.0));
        assert_eq!(update.top_visual_row, 0);
        assert!(!viewport.has_pending_view_sync());
        assert_eq!(
            viewport.take_cursor_reveal_request(),
            Some(EditorCursorReveal::Center)
        );
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
    fn viewport_cursor_reveal_can_align_visual_row_top() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(100.0)), 100);
        viewport.sync_from_helix_top_visual_row(0);

        let update = viewport.reveal_visual_row(20, EditorCursorReveal::Top, 0);

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, 20);
        assert_eq!(viewport.top_visual_row(), 20);
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_cursor_reveal_can_align_visual_row_bottom() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(100.0)), 100);
        viewport.sync_from_helix_top_visual_row(0);

        let update = viewport.reveal_visual_row(20, EditorCursorReveal::Bottom, 0);

        assert!(update.changed);
        assert_eq!(update.crossed_visual_rows, 16);
        assert_eq!(viewport.top_visual_row(), 16);
        assert!(viewport.has_pending_view_sync());
    }

    #[test]
    fn viewport_reveal_scrolls_to_max_when_bottom_row_is_partial() {
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(800.0), px(99.0)), 10);

        assert_eq!(viewport.visible_visual_rows(), 4);

        let update = viewport.reveal_visual_row(9, EditorCursorReveal::Scrolloff, 0);

        assert!(update.changed);
        assert_eq!(viewport.top_visual_row(), 5);
        assert_eq!(viewport.offset_within_row(), px(1.0));
        assert_eq!(
            viewport.scroll_position().y,
            viewport.max_scroll_offset().height
        );
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
            EditorViewportSurfaceLayout::for_editor(None, bounds, px(8.0), px(20.0), 0, None);

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

    #[test]
    fn helix_view_area_plan_reports_target_and_change() {
        let previous_area = Rect::new(0, 0, 10, 4);

        let plan = helix_view_area_plan_for_surface(previous_area, 4, 20, 5);

        assert!(plan.changed);
        assert_eq!(plan.previous_area, previous_area);
        assert_eq!(plan.target_area, Rect::new(0, 0, 24, 6));
    }

    #[test]
    fn helix_view_area_plan_reports_noop_for_matching_area() {
        let previous_area = Rect::new(0, 0, 24, 6);

        let plan = helix_view_area_plan_for_surface(previous_area, 4, 20, 5);

        assert!(!plan.changed);
        assert_eq!(plan.previous_area, previous_area);
        assert_eq!(plan.target_area, previous_area);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn surface_layout_sync_uses_native_cells_without_rewriting_tree_area() {
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\ntwo\nthree\n");
        let mut viewport = EditorViewport::new(px(20.0));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(101.0)));
        let original_area = editor.tree.get(view_id).area;

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
                    scrolloff: Config::default().scrolloff,
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

        assert_eq!(view.area, original_area);
        assert_eq!(
            update.view_area_plan.target_area,
            helix_view_area_for_surface(
                update.gutter_columns,
                expected.viewport_columns,
                viewport.visible_visual_rows()
            )
        );
        assert!(update.view_area_plan.changed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn surface_layout_sync_preserves_split_tree_area() {
        let (mut editor, doc_id, _view_id) = test_editor_with_text("one\ntwo\nthree\n");
        editor.switch(doc_id, Action::VerticalSplit);

        let split_view_id = editor.tree.focus;
        let original_area = editor.tree.get(split_view_id).area;
        assert!(original_area.x > 0);

        let mut viewport = EditorViewport::new(px(20.0));
        let update = viewport
            .sync_surface_layout(
                &mut editor,
                doc_id,
                split_view_id,
                EditorViewportSurfaceLayout {
                    theme: None,
                    bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(101.0))),
                    cell_width: px(8.0),
                    line_height: px(20.0),
                    minimum_columns: 1,
                    scrolloff: Config::default().scrolloff,
                    cursor_reveal: None,
                },
            )
            .unwrap();

        assert_eq!(editor.tree.get(split_view_id).area, original_area);
        assert_eq!(update.view_area_plan.target_area.x, 0);
        assert_ne!(update.view_area_plan.target_area, original_area);
    }

    #[test]
    fn surface_layout_sync_for_view_updates_native_and_helix_layout() {
        let (mut document, mut view) = test_document_and_view("one\ntwo\nthree\n");
        let view_id = view.id;
        let mut viewport = EditorViewport::new(px(20.0));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(101.0)));

        let update = viewport.sync_surface_layout_for_view(
            &mut document,
            &mut view,
            view_id,
            EditorViewportSurfaceLayout {
                theme: None,
                bounds,
                cell_width: px(8.0),
                line_height: px(20.0),
                minimum_columns: 1,
                scrolloff: Config::default().scrolloff,
                cursor_reveal: None,
            },
        );

        assert_eq!(view.area, update.view_area_plan.target_area);
        assert_eq!(update.gutter_columns, view.gutter_offset(&document));
        assert_eq!(viewport.content_visual_rows(), update.visual_rows);
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
                    scrolloff: Config::default().scrolloff,
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
            scrolloff: Config::default().scrolloff,
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
    async fn surface_layout_resize_clamps_bottom_scroll_and_resyncs_helix() {
        let text = (0..30)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let (mut editor, doc_id, view_id) = test_editor_with_text(&text);
        let mut viewport = EditorViewport::new(px(20.0));
        let narrow_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(101.0)));
        let tall_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(201.0)));
        let narrow_layout = EditorViewportSurfaceLayout {
            theme: None,
            bounds: narrow_bounds,
            cell_width: px(8.0),
            line_height: px(20.0),
            minimum_columns: 1,
            scrolloff: Config::default().scrolloff,
            cursor_reveal: None,
        };
        let tall_layout = EditorViewportSurfaceLayout {
            bounds: tall_bounds,
            ..narrow_layout
        };

        viewport
            .sync_surface_layout(&mut editor, doc_id, view_id, narrow_layout)
            .unwrap();
        viewport.scroll_to_vertical_position_from_scrollbar(viewport.max_scroll_offset().height);
        viewport
            .sync_surface_layout(&mut editor, doc_id, view_id, narrow_layout)
            .unwrap();

        let update = viewport
            .sync_surface_layout(&mut editor, doc_id, view_id, tall_layout)
            .unwrap();
        let expected_top_visual_row = viewport.top_visual_row();

        assert!(update.helix_view_synced);
        assert_eq!(
            viewport.scroll_position().y,
            viewport.max_scroll_offset().height
        );
        assert_eq!(
            update.helix_snapshot.top_visual_row,
            expected_top_visual_row
        );

        let doc = editor.document(doc_id).unwrap();
        assert_eq!(
            doc.text().char_to_line(doc.view_offset(view_id).anchor),
            expected_top_visual_row
        );
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
                    scrolloff: Config::default().scrolloff,
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
    fn view_position_for_top_visual_row_clamps_to_text() {
        let text = Rope::from("one\ntwo");
        let text_format = TextFormat::default();
        let annotations = default_annotations();

        let view_position =
            view_position_for_top_visual_row(text.slice(..), 1_000, 7, &text_format, &annotations);

        assert_eq!(text.char_to_line(view_position.anchor), 1);
        assert_eq!(view_position.vertical_offset, 0);
        assert_eq!(view_position.horizontal_offset, 7);
    }

    #[test]
    fn view_position_for_top_visual_row_clears_soft_wrap_horizontal_offset() {
        let text = "abcdef\nzz";
        let text_format = TextFormat {
            soft_wrap: true,
            viewport_width: 3,
            ..TextFormat::default()
        };
        let annotations = default_annotations();

        let view_position =
            view_position_for_top_visual_row(text.into(), 1, 7, &text_format, &annotations);

        assert_eq!(view_position.horizontal_offset, 0);
    }

    #[test]
    fn view_position_plan_reports_noop_for_matching_native_row() {
        let text = Rope::from("one\ntwo\nthree");
        let text_format = TextFormat::default();
        let annotations = default_annotations();
        let current_position = ViewPosition {
            anchor: text.line_to_char(1),
            vertical_offset: 0,
            horizontal_offset: 7,
        };

        let plan = view_position_plan_for_top_visual_row(
            text.slice(..),
            current_position,
            1,
            &text_format,
            &annotations,
        );

        assert!(!plan.changed);
        assert_eq!(plan.top_visual_row, 1);
        assert_eq!(plan.previous_view_position, current_position);
        assert_eq!(plan.view_position, current_position);
    }

    #[test]
    fn view_position_plan_maps_trailing_newline_eof_row() {
        let text = Rope::from("one\ntwo\n");
        let text_format = TextFormat::default();
        let annotations = default_annotations();
        let current_position = ViewPosition {
            anchor: 0,
            vertical_offset: 0,
            horizontal_offset: 0,
        };
        let eof_row = text.len_lines().saturating_sub(1);

        let plan = view_position_plan_for_top_visual_row(
            text.slice(..),
            current_position,
            eof_row,
            &text_format,
            &annotations,
        );

        assert!(plan.changed);
        assert_eq!(plan.top_visual_row, eof_row);
        assert_eq!(plan.view_position.anchor, text.len_chars());
        assert_eq!(text.char_to_line(plan.view_position.anchor), eof_row);
    }

    #[test]
    fn view_position_plan_clears_soft_wrap_horizontal_offset() {
        let text = Rope::from("abcdef\nzz");
        let text_format = TextFormat {
            soft_wrap: true,
            viewport_width: 3,
            ..TextFormat::default()
        };
        let annotations = default_annotations();
        let current_position = ViewPosition {
            anchor: 0,
            vertical_offset: 0,
            horizontal_offset: 7,
        };

        let plan = view_position_plan_for_top_visual_row(
            text.slice(..),
            current_position,
            0,
            &text_format,
            &annotations,
        );

        assert!(plan.changed);
        assert_eq!(plan.view_position.horizontal_offset, 0);
    }

    #[test]
    fn viewport_sync_view_position_updates_document_offset() {
        let (mut document, view) = test_document_and_view("one\ntwo\nthree\n");
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(240.0), px(20.0)), 4);
        viewport.sync_from_helix_top_visual_row(1);

        let synced =
            viewport.sync_view_position(&mut document, &view, view.id, &TextFormat::default());

        assert!(synced);
        assert_eq!(
            document
                .text()
                .char_to_line(document.view_offset(view.id).anchor),
            1
        );
    }

    #[test]
    fn viewport_sync_view_position_reports_noop_for_matching_offset() {
        let (mut document, view) = test_document_and_view("one\ntwo\nthree\n");
        let mut viewport = EditorViewport::new(px(20.0));
        viewport.set_layout(px(20.0), size(px(240.0), px(20.0)), 4);
        viewport.sync_from_helix_top_visual_row(1);
        document.set_view_offset(
            view.id,
            ViewPosition {
                anchor: document.text().line_to_char(1),
                vertical_offset: 0,
                horizontal_offset: 0,
            },
        );

        let synced =
            viewport.sync_view_position(&mut document, &view, view.id, &TextFormat::default());

        assert!(!synced);
        assert_eq!(
            document
                .text()
                .char_to_line(document.view_offset(view.id).anchor),
            1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn surface_layout_render_position_clamps_stale_helix_anchor() {
        let (mut editor, doc_id, view_id) = test_editor_with_text("one\ntwo");
        let mut viewport = EditorViewport::new(px(20.0));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(41.0)));

        {
            let doc = editor.document_mut(doc_id).unwrap();
            doc.set_view_offset(
                view_id,
                ViewPosition {
                    anchor: 1_000,
                    vertical_offset: 0,
                    horizontal_offset: 7,
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
                    bounds,
                    cell_width: px(8.0),
                    line_height: px(20.0),
                    minimum_columns: 1,
                    scrolloff: Config::default().scrolloff,
                    cursor_reveal: None,
                },
            )
            .unwrap();

        let doc = editor.document(doc_id).unwrap();
        assert_eq!(doc.view_offset(view_id).anchor, 1_000);
        assert_eq!(
            update.view_position_plan.view_position,
            update.view_position
        );
        assert!(update.view_position.anchor <= doc.text().len_chars());
        assert_eq!(doc.text().char_to_line(update.view_position.anchor), 1);
        assert_eq!(update.view_position.horizontal_offset, 7);
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
