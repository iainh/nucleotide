// ABOUTME: Native GPUI editor viewport state for pixel and visual-row scrolling
// ABOUTME: Owns GUI scroll state before it is synced into Helix view offsets

use gpui::{Bounds, Pixels, Point, Size, point, px};

use crate::ScrollManager;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportScrollUpdate {
    pub changed: bool,
    pub crossed_visual_rows: isize,
    pub top_visual_row: usize,
    pub offset_within_row: Pixels,
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

    pub fn has_pending_scrollbar_sync(&self) -> bool {
        self.scroll.scrollbar_changed.get()
    }

    pub fn clear_pending_scrollbar_sync(&self) {
        self.scroll.scrollbar_changed.set(false);
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

    pub fn sync_from_helix_top_visual_row(&self, top_visual_row: usize) {
        let y = self.scroll.anchor_to_pixels(top_visual_row);
        self.scroll
            .set_scroll_position_from_helix_preserving_intra_line_offset(point(px(0.0), y));
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

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

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
}
