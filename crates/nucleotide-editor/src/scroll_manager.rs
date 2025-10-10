// ABOUTME: Manages scroll state synchronization between Helix editor and GPUI UI
// ABOUTME: Converts between pixel-based scrolling (GPUI) and line-based anchors (Helix)

use gpui::{Pixels, Point, Size, point, px, size};
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
        let content_height = line_height * (total_lines as f32);
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
        let line = (y / line_height).floor() as usize;
        line.min(total_lines.saturating_sub(1))
    }

    /// Convert a Helix viewport anchor (line number) to pixel scroll offset
    pub fn anchor_to_pixels(&self, anchor: usize) -> Pixels {
        let line_height = self.line_height.get();
        line_height * (anchor as f32)
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
        let ratio = viewport_height / line_height;
        let lines_per_viewport = ratio as usize;

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

#[cfg(test)]
mod scroll_manager_tests {
    use super::*;

    #[test]
    fn test_scroll_manager_creation() {
        let line_height = px(20.0);
        let manager = ScrollManager::new(line_height);

        assert_eq!(manager.line_height.get(), line_height);
        assert_eq!(manager.total_lines.get(), 1);
        assert_eq!(manager.scroll_position(), point(px(0.0), px(0.0)));
        assert!(!manager.scrollbar_changed.get());
    }

    #[test]
    fn test_scroll_position_and_offset_conversion() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100); // Allow scrolling up to 100 lines
        manager.set_viewport_size(size(px(800.0), px(400.0))); // 400px viewport

        // Test positive position (Zed convention) - only vertical since horizontal is clamped to 0
        let position = point(px(0.0), px(100.0)); // X clamped to 0, Y should work
        manager.set_scroll_position(position);
        assert_eq!(manager.scroll_position(), position);

        // Test offset conversion (negative for GPUI)
        let expected_offset = point(px(0.0), px(-100.0)); // X clamped to 0
        assert_eq!(manager.scroll_offset(), expected_offset);

        // Test setting offset (should convert to positive position)
        let offset = point(px(0.0), px(-150.0)); // Only vertical scrolling
        manager.set_scroll_offset(offset);
        let expected_position = point(px(0.0), px(150.0));
        assert_eq!(manager.scroll_position(), expected_position);
    }

    #[test]
    fn test_pixels_to_anchor_conversion() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);

        // Test basic conversions
        assert_eq!(manager.pixels_to_anchor(px(0.0)), 0);
        assert_eq!(manager.pixels_to_anchor(px(20.0)), 1);
        assert_eq!(manager.pixels_to_anchor(px(40.0)), 2);
        assert_eq!(manager.pixels_to_anchor(px(100.0)), 5);

        // Test fractional pixels (should floor)
        assert_eq!(manager.pixels_to_anchor(px(19.9)), 0);
        assert_eq!(manager.pixels_to_anchor(px(20.1)), 1);
        assert_eq!(manager.pixels_to_anchor(px(39.9)), 1);

        // Test clamping to total lines
        assert_eq!(manager.pixels_to_anchor(px(2000.0)), 99); // Should clamp to last line (99)
    }

    #[test]
    fn test_anchor_to_pixels_conversion() {
        let manager = ScrollManager::new(px(20.0));

        // Test basic conversions
        assert_eq!(manager.anchor_to_pixels(0), px(0.0));
        assert_eq!(manager.anchor_to_pixels(1), px(20.0));
        assert_eq!(manager.anchor_to_pixels(5), px(100.0));
        assert_eq!(manager.anchor_to_pixels(10), px(200.0));
    }

    #[test]
    fn test_round_trip_anchor_pixel_conversion() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(50);

        // Test round-trip conversions
        for line in [0, 1, 5, 10, 25, 49] {
            let pixels = manager.anchor_to_pixels(line);
            let recovered_line = manager.pixels_to_anchor(pixels);
            assert_eq!(recovered_line, line);
        }

        // Test pixel round-trips (may differ due to flooring)
        for pixels in [px(0.0), px(20.0), px(40.0), px(100.0), px(200.0)] {
            let line = manager.pixels_to_anchor(pixels);
            let recovered_pixels = manager.anchor_to_pixels(line);
            assert_eq!(recovered_pixels, pixels);
        }
    }

    #[test]
    fn test_max_scroll_offset_calculation() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100); // 100 lines * 20px = 2000px content height
        manager.set_viewport_size(size(px(800.0), px(400.0))); // 400px viewport height

        let max_offset = manager.max_scroll_offset();

        // Max scroll should be content_height - viewport_height = 2000 - 400 = 1600px
        assert_eq!(max_offset.height, px(1600.0));
        assert_eq!(max_offset.width, px(0.0)); // No horizontal scrolling in this test
    }

    #[test]
    fn test_max_scroll_offset_clamping_to_zero() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(10); // 10 lines * 20px = 200px content height
        manager.set_viewport_size(size(px(800.0), px(400.0))); // 400px viewport height (larger than content)

        let max_offset = manager.max_scroll_offset();

        // Max scroll should be clamped to 0 when content is smaller than viewport
        assert_eq!(max_offset.height, px(0.0));
        assert_eq!(max_offset.width, px(0.0));
    }

    #[test]
    fn test_scroll_position_clamping() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(50); // 50 lines * 20px = 1000px
        manager.set_viewport_size(size(px(800.0), px(400.0))); // Max scroll = 1000 - 400 = 600px

        // Test normal position (should not clamp)
        manager.set_scroll_position(point(px(0.0), px(300.0)));
        assert_eq!(manager.scroll_position(), point(px(0.0), px(300.0)));

        // Test negative position (should clamp to 0)
        manager.set_scroll_position(point(px(-50.0), px(-100.0)));
        assert_eq!(manager.scroll_position(), point(px(0.0), px(0.0)));

        // Test position beyond max (should clamp to max)
        manager.set_scroll_position(point(px(0.0), px(800.0)));
        assert_eq!(manager.scroll_position(), point(px(0.0), px(600.0))); // Clamped to max
    }

    #[test]
    fn test_scrollbar_changed_flag() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100); // Allow scrolling
        manager.set_viewport_size(size(px(800.0), px(400.0))); // Set viewport size

        // Initial state
        assert!(!manager.scrollbar_changed.get());

        // Setting position from scrollbar should set the flag
        manager.set_scroll_position(point(px(0.0), px(50.0)));
        assert!(manager.scrollbar_changed.get());

        // Reset flag manually
        manager.scrollbar_changed.set(false);
        assert!(!manager.scrollbar_changed.get());

        // Setting position from Helix should NOT set the flag
        manager.set_scroll_position_from_helix(point(px(0.0), px(100.0)));
        assert!(!manager.scrollbar_changed.get());

        // Setting offset from scrollbar should set the flag
        manager.set_scroll_offset(point(px(0.0), px(-150.0)));
        assert!(manager.scrollbar_changed.get());

        // Reset and test offset from Helix
        manager.scrollbar_changed.set(false);
        manager.set_scroll_offset_from_helix(point(px(0.0), px(-200.0)));
        assert!(!manager.scrollbar_changed.get());
    }

    #[test]
    fn test_visible_line_range_calculation() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(400.0))); // 400px / 20px = 20 lines visible

        // Test at top of document
        manager.set_scroll_position(point(px(0.0), px(0.0)));
        let (first, last) = manager.visible_line_range();
        assert_eq!(first, 0);
        assert_eq!(last, 21); // 20 lines + 1 for partial visibility

        // Test scrolled down
        manager.set_scroll_position(point(px(0.0), px(100.0))); // 5 lines down
        let (first, last) = manager.visible_line_range();
        assert_eq!(first, 5);
        assert_eq!(last, 26); // 5 + 20 + 1

        // Test near end of document
        manager.set_scroll_position(point(px(0.0), px(1600.0))); // 80 lines down
        let (first, last) = manager.visible_line_range();
        assert_eq!(first, 80);
        assert_eq!(last, 100); // Should clamp to total_lines (100)
    }

    #[test]
    fn test_is_line_visible() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(400.0))); // 20 lines visible

        // At top of document (lines 0-20 visible)
        manager.set_scroll_position(point(px(0.0), px(0.0)));
        assert!(manager.is_line_visible(0));
        assert!(manager.is_line_visible(10));
        assert!(manager.is_line_visible(20));
        assert!(!manager.is_line_visible(21));
        assert!(!manager.is_line_visible(50));

        // Scrolled to middle (lines 25-45 visible)
        manager.set_scroll_position(point(px(0.0), px(500.0))); // 25 lines down
        assert!(!manager.is_line_visible(24));
        assert!(manager.is_line_visible(25));
        assert!(manager.is_line_visible(35));
        assert!(manager.is_line_visible(45));
        assert!(!manager.is_line_visible(46));
    }

    #[test]
    fn test_scroll_to_line_strategies() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(400.0))); // 20 lines visible

        // Test scroll to top
        manager.scroll_to_line(25, ScrollStrategy::Top);
        assert_eq!(manager.scroll_position().y, px(500.0)); // 25 * 20px = 500px

        // Test scroll to center
        manager.scroll_to_line(25, ScrollStrategy::Center);
        // Center strategy: line - (viewport_lines / 2) = 25 - 10 = 15
        assert_eq!(manager.scroll_position().y, px(300.0)); // 15 * 20px = 300px

        // Test scroll to bottom
        manager.scroll_to_line(25, ScrollStrategy::Bottom);
        // Bottom strategy: line - (viewport_lines - 1) = 25 - 19 = 6
        assert_eq!(manager.scroll_position().y, px(120.0)); // 6 * 20px = 120px
    }

    #[test]
    fn test_scroll_to_line_edge_cases() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(400.0))); // 20 lines visible

        // Start from a non-zero position to ensure the flag gets set
        manager.set_scroll_position(point(px(0.0), px(100.0)));
        manager.scrollbar_changed.set(false); // Reset flag

        // Test scroll to line 0 with center strategy
        manager.scroll_to_line(0, ScrollStrategy::Center);
        // Should handle underflow: 0 - 10 = 0 (clamped by saturating_sub)
        assert_eq!(manager.scroll_position().y, px(0.0));

        // Test scrollbar changed flag is set (position changed from 100px to 0px)
        assert!(manager.scrollbar_changed.get());

        // Reset flag and test another edge case
        manager.scrollbar_changed.set(false);
        manager.scroll_to_line(5, ScrollStrategy::Bottom);
        // Should handle underflow: 5 - 19 = 0 (clamped by saturating_sub)
        assert_eq!(manager.scroll_position().y, px(0.0));

        // Flag should not be set since position didn't change (already at 0)
        assert!(!manager.scrollbar_changed.get());
    }

    #[test]
    fn test_basic_helix_sync_conversion() {
        // Test basic conversion between pixels and anchors without Document dependency
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(50);

        // Test pixels to anchor conversion that would be used in sync_to_helix
        let scroll_position_y = px(200.0); // 10 lines down
        let anchor_line = manager.pixels_to_anchor(scroll_position_y);
        assert_eq!(anchor_line, 10);

        // Test anchor to pixels conversion that would be used in sync_from_helix
        let pixels = manager.anchor_to_pixels(15);
        assert_eq!(pixels, px(300.0)); // 15 * 20px = 300px

        // Test that sync operations don't change scrollbar_changed flag inappropriately
        manager.set_scroll_position_from_helix(point(px(0.0), px(100.0)));
        assert!(!manager.scrollbar_changed.get());

        manager.set_scroll_offset_from_helix(point(px(0.0), px(-200.0)));
        assert!(!manager.scrollbar_changed.get());
        assert_eq!(manager.scroll_position().y, px(200.0));
    }
}
