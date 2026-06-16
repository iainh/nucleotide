// ABOUTME: Native pixel scroll state for the GPUI editor viewport
// ABOUTME: Tracks visual-row positions and sub-row offsets for smooth scrolling

use gpui::{Pixels, Point, Size, point, px, size};
use nucleotide_logging::trace;
use std::cell::Cell;
use std::rc::Rc;

/// Manages native scroll state for a document viewport.
#[derive(Clone, Debug)]
pub struct ScrollManager {
    /// Unique ID for debugging
    id: usize,
    /// Cached line height in pixels
    line_height: Rc<Cell<Pixels>>,
    /// Total number of lines in the document
    total_lines: Rc<Cell<usize>>,
    /// Current scroll position in pixels (positive when scrolled down/right)
    scroll_position: Rc<Cell<Point<Pixels>>>,
    /// Viewport size in pixels
    viewport_size: Rc<Cell<Size<Pixels>>>,
    /// Track if native viewport scroll changed and needs sync to Helix
    pending_view_sync: Rc<Cell<bool>>,
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
            pending_view_sync: Rc::new(Cell::new(false)),
        }
    }

    pub(crate) fn line_height(&self) -> Pixels {
        self.line_height.get()
    }

    pub(crate) fn total_lines(&self) -> usize {
        self.total_lines.get()
    }

    pub(crate) fn viewport_size(&self) -> Size<Pixels> {
        self.viewport_size.get()
    }

    pub(crate) fn has_pending_view_sync(&self) -> bool {
        self.pending_view_sync.get()
    }

    pub(crate) fn clear_pending_view_sync(&self) {
        self.pending_view_sync.set(false);
    }

    /// Update the total number of lines in the document
    pub(crate) fn set_total_lines(&mut self, total_lines: usize) {
        self.total_lines.set(total_lines);
        self.clamp_scroll_position_after_extent_change();
    }

    /// Update the viewport size
    pub(crate) fn set_viewport_size(&mut self, size: Size<Pixels>) {
        self.viewport_size.set(size);
        self.clamp_scroll_position_after_extent_change();
    }

    /// Update the line height
    pub(crate) fn set_line_height(&mut self, line_height: Pixels) {
        let previous_line_height = self.line_height.get();
        let previous_position = self.scroll_position.get();
        let previous_top_line = self.pixels_to_anchor(previous_position.y);
        let previous_offset = if previous_line_height > px(0.0) {
            (previous_position.y - previous_line_height * previous_top_line as f32).max(px(0.0))
        } else {
            px(0.0)
        };

        self.line_height.set(line_height);

        if previous_line_height > px(0.0) && line_height > px(0.0) {
            let offset_fraction = (previous_offset / previous_line_height).clamp(0.0, 1.0);
            let scaled_position = point(
                previous_position.x,
                self.anchor_to_pixels(previous_top_line) + (line_height * offset_fraction),
            );
            self.set_scroll_position_internal(scaled_position, false);
            if self.pixels_to_anchor(self.scroll_position.get().y) != previous_top_line {
                self.pending_view_sync.set(true);
            }
        } else {
            self.clamp_scroll_position_after_extent_change();
        }
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

    /// Set the scroll position in pixels from native interaction (positive when scrolled down/right)
    /// This marks the position as needing sync back to Helix
    pub(crate) fn set_scroll_position(&self, position: Point<Pixels>) {
        self.set_scroll_position_internal(position, true);
    }

    /// Set the scroll offset in pixels from native interaction (negative when scrolled down/right)
    /// This marks the position as needing sync back to Helix
    pub(crate) fn set_scroll_offset(&self, offset: Point<Pixels>) {
        self.set_scroll_offset_internal(offset, true);
    }

    fn set_scroll_offset_internal(&self, offset: Point<Pixels>, from_native_view: bool) {
        let position = point(-offset.x, -offset.y);
        self.set_scroll_position_internal(position, from_native_view);
    }

    /// Apply a GPUI-style pixel scroll delta to the current offset.
    ///
    /// Returns whether the offset changed and how many whole document lines the
    /// scroll position crossed. Wheel scrolling uses this to keep fractional
    /// pixel movement local while letting the GUI viewport decide when to sync
    /// the visible visual row back to Helix.
    pub(crate) fn scroll_by_delta(&self, delta: Point<Pixels>) -> (bool, isize) {
        let old_position = self.scroll_position.get();
        let old_line = self.pixels_to_anchor(old_position.y);
        let next_offset = self.scroll_offset() + delta;

        self.set_scroll_offset_internal(next_offset, false);

        let new_position = self.scroll_position.get();
        let new_line = self.pixels_to_anchor(new_position.y);
        let crossed_lines = new_line as isize - old_line as isize;
        if crossed_lines != 0 {
            self.pending_view_sync.set(true);
        }

        (old_position != new_position, crossed_lines)
    }

    /// Set the scroll position from an external view sync while retaining a
    /// local sub-row pixel offset if both positions point at the same top row.
    pub(crate) fn set_scroll_position_from_view_sync_preserving_subrow_offset(
        &self,
        position: Point<Pixels>,
    ) {
        let current = self.scroll_position.get();
        let current_line = self.pixels_to_anchor(current.y);
        let incoming_line = self.pixels_to_anchor(position.y);
        let y = if current_line == incoming_line {
            current.y
        } else {
            position.y
        };

        self.set_scroll_position_internal(point(position.x, y), false);
    }

    /// Pixel distance scrolled within the current top line.
    pub(crate) fn vertical_offset_within_line(&self) -> Pixels {
        let position_y = self.scroll_position.get().y;
        let top_line = self.pixels_to_anchor(position_y);
        (position_y - self.anchor_to_pixels(top_line)).max(px(0.0))
    }

    /// Internal method to set scroll position with control over native sync tracking
    fn set_scroll_position_internal(&self, position: Point<Pixels>, from_native_view: bool) {
        let max_offset = self.max_scroll_offset();
        // Zed convention: positions are positive when scrolled, clamped between 0 and max
        let clamped_position = point(
            position.x.max(px(0.0)).min(max_offset.width),
            position.y.max(px(0.0)).min(max_offset.height),
        );
        let old_position = self.scroll_position.get();
        self.scroll_position.set(clamped_position);

        if old_position != clamped_position {
            trace!(
                scroll_manager_id = self.id,
                old_position = ?old_position,
                new_position = ?clamped_position,
                from_native_view = from_native_view,
                "ScrollManager position changed"
            );
            if from_native_view {
                self.pending_view_sync.set(true);
            }
        }
    }

    fn clamp_scroll_position_after_extent_change(&self) {
        let old_position = self.scroll_position.get();
        let old_top_line = self.pixels_to_anchor(old_position.y);

        self.set_scroll_position_internal(old_position, false);

        let new_position = self.scroll_position.get();
        if old_position != new_position {
            let new_top_line = self.pixels_to_anchor(new_position.y);
            if old_top_line != new_top_line {
                self.pending_view_sync.set(true);
            }
        }
    }

    /// Convert a pixel scroll offset to a Helix viewport anchor (line number)
    pub(crate) fn pixels_to_anchor(&self, y: Pixels) -> usize {
        let line_height = self.line_height.get();
        let total_lines = self.total_lines.get();
        let line = (y / line_height).floor() as usize;
        line.min(total_lines.saturating_sub(1))
    }

    /// Convert a Helix viewport anchor (line number) to pixel scroll offset
    pub(crate) fn anchor_to_pixels(&self, anchor: usize) -> Pixels {
        let line_height = self.line_height.get();
        line_height * (anchor as f32)
    }
}

#[cfg(test)]
mod scroll_manager_tests {
    use super::*;

    #[test]
    fn test_scroll_manager_creation() {
        let line_height = px(20.0);
        let manager = ScrollManager::new(line_height);

        assert_eq!(manager.line_height(), line_height);
        assert_eq!(manager.total_lines(), 1);
        assert_eq!(manager.scroll_position(), point(px(0.0), px(0.0)));
        assert!(!manager.has_pending_view_sync());
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
    fn test_scroll_by_delta_accumulates_subline_wheel_motion() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(400.0)));

        let (changed, crossed_lines) = manager.scroll_by_delta(point(px(0.0), px(-5.0)));
        assert!(changed);
        assert_eq!(crossed_lines, 0);
        assert_eq!(manager.scroll_position(), point(px(0.0), px(5.0)));
        assert_eq!(manager.vertical_offset_within_line(), px(5.0));
        assert!(!manager.has_pending_view_sync());

        let (_, crossed_lines) = manager.scroll_by_delta(point(px(0.0), px(-15.0)));
        assert_eq!(crossed_lines, 1);
        assert_eq!(manager.scroll_position(), point(px(0.0), px(20.0)));
        assert_eq!(manager.vertical_offset_within_line(), px(0.0));
        assert!(manager.has_pending_view_sync());
    }

    #[test]
    fn test_view_sync_preserves_subrow_offset_for_same_top_row() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(400.0)));

        manager.scroll_by_delta(point(px(0.0), px(-25.0)));
        assert_eq!(manager.scroll_position(), point(px(0.0), px(25.0)));
        assert_eq!(manager.vertical_offset_within_line(), px(5.0));

        manager
            .set_scroll_position_from_view_sync_preserving_subrow_offset(point(px(0.0), px(20.0)));
        assert_eq!(manager.scroll_position(), point(px(0.0), px(25.0)));
        assert_eq!(manager.vertical_offset_within_line(), px(5.0));

        manager
            .set_scroll_position_from_view_sync_preserving_subrow_offset(point(px(0.0), px(40.0)));
        assert_eq!(manager.scroll_position(), point(px(0.0), px(40.0)));
        assert_eq!(manager.vertical_offset_within_line(), px(0.0));
    }

    #[test]
    fn test_viewport_resize_clamps_bottom_scroll_position() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(100.0)));
        manager.set_scroll_position(point(px(0.0), px(1900.0)));
        manager.clear_pending_view_sync();

        manager.set_viewport_size(size(px(800.0), px(200.0)));

        assert_eq!(manager.scroll_position(), point(px(0.0), px(1800.0)));
        assert_eq!(manager.pixels_to_anchor(manager.scroll_position().y), 90);
        assert_eq!(manager.vertical_offset_within_line(), px(0.0));
        assert!(manager.has_pending_view_sync());
    }

    #[test]
    fn test_content_resize_clamps_bottom_scroll_position() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(100.0)));
        manager.set_scroll_position(point(px(0.0), px(1900.0)));
        manager.clear_pending_view_sync();

        manager.set_total_lines(20);

        assert_eq!(manager.scroll_position(), point(px(0.0), px(300.0)));
        assert_eq!(manager.pixels_to_anchor(manager.scroll_position().y), 15);
        assert_eq!(manager.vertical_offset_within_line(), px(0.0));
        assert!(manager.has_pending_view_sync());
    }

    #[test]
    fn test_line_height_change_preserves_visual_row_and_subrow_offset() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100);
        manager.set_viewport_size(size(px(800.0), px(100.0)));
        manager.set_scroll_position(point(px(0.0), px(105.0)));
        manager.clear_pending_view_sync();

        manager.set_line_height(px(30.0));

        assert_eq!(manager.scroll_position(), point(px(0.0), px(157.5)));
        assert_eq!(manager.pixels_to_anchor(manager.scroll_position().y), 5);
        assert_eq!(manager.vertical_offset_within_line(), px(7.5));
        assert!(!manager.has_pending_view_sync());
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
    fn test_pending_view_sync_flag() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(100); // Allow scrolling
        manager.set_viewport_size(size(px(800.0), px(400.0))); // Set viewport size

        // Initial state
        assert!(!manager.has_pending_view_sync());

        // Setting position from a native interaction should set the flag
        manager.set_scroll_position(point(px(0.0), px(50.0)));
        assert!(manager.has_pending_view_sync());

        // Reset flag manually
        manager.clear_pending_view_sync();
        assert!(!manager.has_pending_view_sync());

        // Setting position from view sync should NOT set the flag
        manager
            .set_scroll_position_from_view_sync_preserving_subrow_offset(point(px(0.0), px(100.0)));
        assert!(!manager.has_pending_view_sync());

        // Setting offset from a native interaction should set the flag
        manager.set_scroll_offset(point(px(0.0), px(-150.0)));
        assert!(manager.has_pending_view_sync());
    }

    #[test]
    fn test_basic_visual_row_conversion() {
        let mut manager = ScrollManager::new(px(20.0));
        manager.set_total_lines(50);

        let scroll_position_y = px(200.0); // 10 lines down
        let anchor_line = manager.pixels_to_anchor(scroll_position_y);
        assert_eq!(anchor_line, 10);

        let pixels = manager.anchor_to_pixels(15);
        assert_eq!(pixels, px(300.0)); // 15 * 20px = 300px
    }
}
