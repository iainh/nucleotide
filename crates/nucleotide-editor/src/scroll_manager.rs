// ABOUTME: Manages scroll state synchronization between Helix editor and GPUI UI
// ABOUTME: Converts between pixel-based scrolling (GPUI) and line-based anchors (Helix)

use gpui::{point, px, size, Pixels, Point, Size};
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
    /// Current scroll position in pixels (positive when scrolled down/right)
    pub scroll_position: Rc<Cell<Point<Pixels>>>,
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
            scroll_position: Rc::new(Cell::new(point(px(0.0), px(0.0)))),
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

    /// Get the current scroll position in pixels (positive when scrolled down/right)
    pub fn scroll_position(&self) -> Point<Pixels> {
        self.scroll_position.get()
    }

    /// Get the scroll offset for GPUI scrollable (negative when scrolled down/right)
    pub fn scroll_offset(&self) -> Point<Pixels> {
        let pos = self.scroll_position.get();
        point(-pos.x, -pos.y)
    }

    /// Set the scroll position in pixels from scrollbar interaction (positive when scrolled down/right)
    /// This marks the position as needing sync back to Helix
    pub fn set_scroll_position(&self, position: Point<Pixels>) {
        self.set_scroll_position_internal(position, true);
    }

    /// Set the scroll offset in pixels from scrollbar interaction (negative when scrolled down/right)
    /// This marks the position as needing sync back to Helix
    pub fn set_scroll_offset(&self, offset: Point<Pixels>) {
        let position = point(-offset.x, -offset.y);
        self.set_scroll_position_internal(position, true);
    }

    /// Set the scroll position from Helix sync (doesn't mark as scrollbar-changed)
    pub fn set_scroll_position_from_helix(&self, position: Point<Pixels>) {
        self.set_scroll_position_internal(position, false);
    }

    /// Set the scroll offset from Helix sync (doesn't mark as scrollbar-changed)
    pub fn set_scroll_offset_from_helix(&self, offset: Point<Pixels>) {
        let position = point(-offset.x, -offset.y);
        self.set_scroll_position_internal(position, false);
    }

    /// Internal method to set scroll position with control over scrollbar_changed flag
    fn set_scroll_position_internal(&self, position: Point<Pixels>, from_scrollbar: bool) {
        let max_offset = self.max_scroll_offset();
        // Zed convention: positions are positive when scrolled, clamped between 0 and max
        let clamped_position = point(
            position.x.max(px(0.0)).min(max_offset.width),
            position.y.max(px(0.0)).min(max_offset.height),
        );
        let old_position = self.scroll_position.get();
        self.scroll_position.set(clamped_position);

        if old_position != clamped_position {
            debug!(
                scroll_manager_id = self.id,
                old_position = ?old_position,
                new_position = ?clamped_position,
                from_scrollbar = from_scrollbar,
                "ScrollManager position changed"
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
        // Zed convention: positive position when scrolled down
        let new_position = point(px(0.0), y);

        // Update scroll position without marking as scrollbar-changed
        // This is a sync FROM Helix, not a scrollbar action
        self.set_scroll_position_from_helix(new_position);
    }

    /// Update Helix's ViewOffset from current scroll position
    pub fn sync_to_helix(&self, document: &Document) -> ViewOffset {
        let y = self.scroll_position.get().y;
        // Zed convention: position.y is positive when scrolled down
        let anchor_line = self.pixels_to_anchor(y);
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
        let position = self.scroll_position.get();
        let viewport = self.viewport_size.get();

        // Zed convention: position.y is positive when scrolled down
        let first_line = self.pixels_to_anchor(position.y);
        let last_line = self.pixels_to_anchor(position.y + viewport.height) + 1;

        let total_lines = self.total_lines.get();
        let result = (first_line, last_line.min(total_lines));
        debug!(
            scroll_manager_id = self.id,
            position = ?position,
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

        // Zed convention: positive position when scrolled down
        self.set_scroll_position(point(px(0.0), target_y));
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
