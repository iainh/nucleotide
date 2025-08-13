// ABOUTME: Manages scroll state synchronization between Helix editor and GPUI UI
// ABOUTME: Converts between pixel-based scrolling (GPUI) and line-based anchors (Helix)

use gpui::*;
use helix_view::Document;
use nucleotide_logging::debug;
use std::cell::Cell;
use std::rc::Rc;

/// ViewOffset represents the scroll position of a view in the document
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewOffset {
    pub anchor: usize,
    pub horizontal_offset: usize,
    pub vertical_offset: usize,
}

/// Manages scroll state for a document view, synchronizing between
/// GPUI's pixel-based scrolling and Helix's line-based viewport
#[derive(Clone, Debug)]
pub struct ScrollManager {
    /// Unique ID for debugging
    id: usize,
    /// Cached line height in pixels
    pub line_height: Rc<Cell<Pixels>>,
    /// Total number of lines in the document
    pub total_lines: Rc<Cell<usize>>,
    /// Current scroll offset in pixels
    pub scroll_offset: Rc<Cell<Point<Pixels>>>,
    /// Viewport size in pixels
    pub viewport_size: Rc<Cell<Size<Pixels>>>,
    /// Track if scroll was changed by scrollbar (needs sync to Helix)
    pub scrollbar_changed: Rc<Cell<bool>>,
}

impl ScrollManager {
    pub fn new(line_height: Pixels) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: NEXT_ID.fetch_add(1, Ordering::SeqCst),
            line_height: Rc::new(Cell::new(line_height)),
            total_lines: Rc::new(Cell::new(1)),
            scroll_offset: Rc::new(Cell::new(point(px(0.0), px(0.0)))),
            viewport_size: Rc::new(Cell::new(size(px(800.0), px(600.0)))),
            scrollbar_changed: Rc::new(Cell::new(false)),
        }
    }

    /// Update the total number of lines in the document
    pub fn set_total_lines(&mut self, total_lines: usize) {
        self.total_lines.set(total_lines);
    }

    /// Update the viewport size
    pub fn set_viewport_size(&mut self, size: Size<Pixels>) {
        self.viewport_size.set(size);
    }

    /// Update the line height
    pub fn set_line_height(&mut self, line_height: Pixels) {
        self.line_height.set(line_height);
    }

    /// Get the maximum scroll offset in pixels
    pub fn max_scroll_offset(&self) -> Size<Pixels> {
        let total_lines = self.total_lines.get();
        let line_height = self.line_height.get();
        let content_height = px(total_lines as f32 * line_height.0);
        let viewport_height = self.viewport_size.get().height;
        let max_y = (content_height - viewport_height).max(px(0.0));

        size(px(0.0), max_y)
    }

    /// Get the current scroll offset in pixels
    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_offset.get()
    }

    /// Set the scroll offset in pixels from scrollbar interaction
    /// This marks the position as needing sync back to Helix
    pub fn set_scroll_offset(&self, offset: Point<Pixels>) {
        self.set_scroll_offset_internal(offset, true);
    }

    /// Set the scroll offset from Helix sync (doesn't mark as scrollbar-changed)
    pub fn set_scroll_offset_from_helix(&self, offset: Point<Pixels>) {
        self.set_scroll_offset_internal(offset, false);
    }

    /// Internal method to set scroll offset with control over scrollbar_changed flag
    fn set_scroll_offset_internal(&self, offset: Point<Pixels>, from_scrollbar: bool) {
        let max_offset = self.max_scroll_offset();
        // GPUI convention: offsets are negative when scrolled, clamped between -max and 0
        let clamped_offset = point(
            offset.x.min(px(0.0)).max(-max_offset.width),
            offset.y.min(px(0.0)).max(-max_offset.height),
        );
        let old_offset = self.scroll_offset.get();
        self.scroll_offset.set(clamped_offset);

        if old_offset != clamped_offset {
            debug!(
                scroll_manager_id = self.id,
                old_offset = ?old_offset,
                new_offset = ?clamped_offset,
                from_scrollbar = from_scrollbar,
                "ScrollManager offset changed"
            );
            if from_scrollbar {
                // Mark that scrollbar changed the position (needs sync to Helix)
                self.scrollbar_changed.set(true);
            }
        }
    }

    /// Convert a pixel scroll offset to a Helix viewport anchor (line number)
    pub fn pixels_to_anchor(&self, y: Pixels) -> usize {
        let line_height = self.line_height.get();
        let total_lines = self.total_lines.get();
        let line = (y.0 / line_height.0).floor() as usize;
        line.min(total_lines.saturating_sub(1))
    }

    /// Convert a Helix viewport anchor (line number) to pixel scroll offset
    pub fn anchor_to_pixels(&self, anchor: usize) -> Pixels {
        let line_height = self.line_height.get();
        px(anchor as f32 * line_height.0)
    }

    /// Update scroll position from Helix's ViewOffset
    pub fn sync_from_helix(&mut self, view_offset: &ViewOffset, document: &Document) {
        // ViewOffset.anchor is a character position, convert to line
        let text = document.text();
        let anchor_line = text.char_to_line(view_offset.anchor);
        let y = self.anchor_to_pixels(anchor_line);
        // GPUI convention: negative offset when scrolled down
        let new_offset = point(px(0.0), -y);

        // Update scroll offset without marking as scrollbar-changed
        // This is a sync FROM Helix, not a scrollbar action
        let max_offset = self.max_scroll_offset();
        let clamped_offset = point(
            new_offset.x.min(px(0.0)).max(-max_offset.width),
            new_offset.y.min(px(0.0)).max(-max_offset.height),
        );
        self.scroll_offset.set(clamped_offset);
        // Don't set scrollbar_changed flag - this is Helix updating us
    }

    /// Update Helix's ViewOffset from current scroll position
    pub fn sync_to_helix(&self, document: &Document) -> ViewOffset {
        let y = self.scroll_offset.get().y;
        // GPUI convention: offset.y is negative when scrolled down
        let anchor_line = self.pixels_to_anchor(-y);
        let text = document.text();
        let anchor = text.line_to_char(anchor_line);

        ViewOffset {
            anchor,
            horizontal_offset: 0,
            vertical_offset: 0,
        }
    }

    /// Get the visible line range for the current scroll position
    pub fn visible_line_range(&self) -> (usize, usize) {
        let offset = self.scroll_offset.get();
        let viewport = self.viewport_size.get();

        // GPUI convention: offset.y is negative when scrolled down
        // So -offset.y gives us the positive scroll distance
        let first_line = self.pixels_to_anchor(-offset.y);
        let last_line = self.pixels_to_anchor(-offset.y + viewport.height) + 1;

        let total_lines = self.total_lines.get();
        let result = (first_line, last_line.min(total_lines));
        debug!(
            scroll_manager_id = self.id,
            offset = ?offset,
            viewport = ?viewport,
            line_range = ?result,
            "ScrollManager visible line range calculated"
        );
        result
    }

    /// Check if a line is visible in the current viewport
    pub fn is_line_visible(&self, line: usize) -> bool {
        let (first, last) = self.visible_line_range();
        line >= first && line < last
    }

    /// Scroll to make a specific line visible
    pub fn scroll_to_line(&mut self, line: usize, strategy: ScrollStrategy) {
        let viewport_height = self.viewport_size.get().height;
        let line_height = self.line_height.get();
        let lines_per_viewport = (viewport_height.0 / line_height.0) as usize;

        let target_y = match strategy {
            ScrollStrategy::Top => self.anchor_to_pixels(line),
            ScrollStrategy::Center => {
                let center_offset = lines_per_viewport / 2;
                self.anchor_to_pixels(line.saturating_sub(center_offset))
            }
            ScrollStrategy::Bottom => {
                self.anchor_to_pixels(line.saturating_sub(lines_per_viewport - 1))
            }
        };

        // GPUI convention: negative offset when scrolled down
        self.set_scroll_offset(point(px(0.0), -target_y));
    }
}

/// Strategy for scrolling to a specific line
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollStrategy {
    /// Position the line at the top of the viewport
    Top,
    /// Position the line at the center of the viewport
    Center,
    /// Position the line at the bottom of the viewport
    Bottom,
}
