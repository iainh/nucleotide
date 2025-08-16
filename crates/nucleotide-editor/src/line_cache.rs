// ABOUTME: Line layout cache for mouse interaction in document view
// ABOUTME: Stores line layouts in element-local coordinates (text-area relative) for fast mouse hit testing

use gpui::{size, Bounds, Pixels, ShapedLine};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Layout information for a single line in the document
///
/// COORDINATE SYSTEM: All positions stored in element-local coordinates
/// - origin: Position relative to text area (0,0 = top-left of text area, after gutters)
/// - This matches the coordinate system expected by mouse event handlers
#[derive(Clone)]
pub struct LineLayout {
    pub line_idx: usize,
    pub shaped_line: ShapedLine,
    /// Position of line's top-left corner in element-local coordinates (text-area relative)
    pub origin: gpui::Point<Pixels>,
    /// For wrapped lines: character offset where this segment starts within the full document line.
    /// For non-wrapped lines: always 0.
    pub segment_char_offset: usize,
    /// For wrapped lines: byte offset where real text starts within the shaped line (after wrap indicators).
    /// For non-wrapped lines: always 0.
    pub text_start_byte_offset: usize,
}

/// Key for caching shaped lines
#[derive(Hash, Eq, PartialEq, Clone)]
pub struct ShapedLineKey {
    pub line_text: String,
    pub font_size: u32,      // Store as integer to avoid float comparison issues
    pub viewport_width: u32, // Store as integer pixels
}

/// Thread-safe cache for line layouts
#[derive(Clone, Default)]
pub struct LineLayoutCache {
    layouts: Arc<Mutex<Vec<LineLayout>>>,
    shaped_lines: Arc<Mutex<HashMap<ShapedLineKey, ShapedLine>>>,
}

impl LineLayoutCache {
    pub fn new() -> Self {
        Self {
            layouts: Arc::new(Mutex::new(Vec::new())),
            shaped_lines: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn clear(&self) {
        if let Ok(mut layouts) = self.layouts.lock() {
            layouts.clear();
        }
        // Don't clear shaped_lines - keep them cached across frames
    }

    /// Clear the shaped lines cache (use when font or theme changes)
    pub fn clear_shaped_lines(&self) {
        if let Ok(mut shaped) = self.shaped_lines.lock() {
            shaped.clear();
        }
    }

    pub fn push(&self, layout: LineLayout) {
        if let Ok(mut layouts) = self.layouts.lock() {
            layouts.push(layout);
        }
    }

    /// Find line at given position using element-local coordinates
    ///
    /// # Arguments
    /// * `position` - Mouse position in element-local coordinates (text-area relative)
    /// * `bounds_width` - Width of text area in pixels  
    /// * `line_height` - Height of each line in pixels
    ///
    /// # Coordinate System
    /// Expects `position` to be in element-local coordinates (after converting from window coordinates)
    /// Use this method when you have already transformed coordinates: Window → TextArea
    ///
    /// # Performance
    /// Uses binary search for O(log n) performance since line layouts are stored in Y-order
    pub fn find_line_at_position(
        &self,
        position: gpui::Point<Pixels>,
        bounds_width: Pixels,
        line_height: Pixels,
    ) -> Option<LineLayout> {
        if let Ok(layouts) = self.layouts.lock() {
            if layouts.is_empty() {
                return None;
            }

            // Binary search for the line containing the Y position
            // Since line layouts are stored in Y-order (monotonically increasing origin.y),
            // we can use binary search for O(log n) performance
            let target_y = position.y;

            let mut left = 0;
            let mut right = layouts.len();

            while left < right {
                let mid = left + (right - left) / 2;
                let layout = &layouts[mid];
                let line_bounds = Bounds {
                    origin: layout.origin,
                    size: size(bounds_width, line_height),
                };

                if line_bounds.origin.y <= target_y
                    && target_y < line_bounds.origin.y + line_bounds.size.height
                {
                    // Found the line - also check X bounds
                    return if line_bounds.contains(&position) {
                        Some(layout.clone())
                    } else {
                        None // X position is outside line bounds
                    };
                } else if target_y < line_bounds.origin.y {
                    right = mid;
                } else {
                    left = mid + 1;
                }
            }

            None // Position not found in any line
        } else {
            None
        }
    }

    /// Find line at position with scroll offset adjustment (DEPRECATED - use find_line_at_position)
    ///
    /// # Deprecation Notice
    /// This method is deprecated in favor of proper coordinate transformation.
    /// New code should use the coordinate transformation chain: Window → TextArea → Content
    /// and then use find_line_at_position() with properly transformed coordinates.
    ///
    /// # Arguments
    /// * `position` - Mouse position in element-local coordinates
    /// * `scroll_offset` - Current scroll offset (negative when scrolled down)
    ///
    /// # Note
    /// The scroll_offset uses GPUI's negative convention (negative when scrolled down)
    pub fn find_line_at_position_with_scroll(
        &self,
        position: gpui::Point<Pixels>,
        bounds_width: Pixels,
        line_height: Pixels,
        scroll_offset: gpui::Point<Pixels>,
    ) -> Option<LineLayout> {
        if let Ok(layouts) = self.layouts.lock() {
            layouts
                .iter()
                .find(|layout| {
                    // Adjust the line origin by the scroll offset
                    // GPUI applies scroll transformations, so we need to account for them
                    let adjusted_origin = gpui::point(
                        layout.origin.x + scroll_offset.x,
                        layout.origin.y + scroll_offset.y,
                    );
                    let line_bounds = Bounds {
                        origin: adjusted_origin,
                        size: size(bounds_width, line_height),
                    };
                    line_bounds.contains(&position)
                })
                .map(|layout| {
                    // Return a copy with the adjusted origin for consistency
                    LineLayout {
                        line_idx: layout.line_idx,
                        shaped_line: layout.shaped_line.clone(),
                        origin: gpui::point(
                            layout.origin.x + scroll_offset.x,
                            layout.origin.y + scroll_offset.y,
                        ),
                        segment_char_offset: layout.segment_char_offset,
                        text_start_byte_offset: layout.text_start_byte_offset,
                    }
                })
        } else {
            None
        }
    }

    pub fn find_line_by_index(&self, line_idx: usize) -> Option<LineLayout> {
        if let Ok(layouts) = self.layouts.lock() {
            layouts
                .iter()
                .find(|layout| layout.line_idx == line_idx)
                .cloned()
        } else {
            None
        }
    }

    /// Get a cached shaped line or None if not cached
    pub fn get_shaped_line(&self, key: &ShapedLineKey) -> Option<ShapedLine> {
        if let Ok(shaped) = self.shaped_lines.lock() {
            shaped.get(key).cloned()
        } else {
            None
        }
    }

    /// Store a shaped line in the cache
    pub fn store_shaped_line(&self, key: ShapedLineKey, shaped_line: ShapedLine) {
        if let Ok(mut shaped) = self.shaped_lines.lock() {
            // Limit cache size to prevent unbounded growth
            if shaped.len() > 1000 {
                shaped.clear();
            }
            shaped.insert(key, shaped_line);
        }
    }
}

impl gpui::Global for LineLayoutCache {}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    fn create_test_shaped_line() -> ShapedLine {
        // Create a mock shaped line - in real usage this would be from the font system
        // Use the default implementation since the struct has private fields
        <ShapedLine as std::default::Default>::default()
    }

    #[test]
    fn test_line_lookup_by_index() {
        let cache = LineLayoutCache::new();

        // Add some test lines
        for line_idx in 0..5 {
            let layout = LineLayout {
                line_idx,
                shaped_line: create_test_shaped_line(),
                origin: point(px(0.0), px(line_idx as f32 * 24.0)),
                segment_char_offset: 0,
                text_start_byte_offset: 0,
            };
            cache.push(layout);
        }

        // Test finding existing lines
        for line_idx in 0..5 {
            let found = cache.find_line_by_index(line_idx);
            assert!(found.is_some(), "Should find line {}", line_idx);
            let layout = found.unwrap();
            assert_eq!(layout.line_idx, line_idx);
            assert_eq!(layout.origin.y, px(line_idx as f32 * 24.0));
        }

        // Test finding non-existent line
        let not_found = cache.find_line_by_index(10);
        assert!(not_found.is_none(), "Should not find non-existent line 10");
    }

    #[test]
    fn test_line_lookup_at_position() {
        let cache = LineLayoutCache::new();
        let line_height = px(24.0);
        let bounds_width = px(800.0);

        // Add test lines at different Y positions
        for line_idx in 0..3 {
            let y_position = px(line_idx as f32 * line_height.0);
            let layout = LineLayout {
                line_idx,
                shaped_line: create_test_shaped_line(),
                origin: point(px(0.0), y_position),
                segment_char_offset: 0,
                text_start_byte_offset: 0,
            };
            cache.push(layout);
        }

        // Test finding lines by position
        let test_cases = [
            (point(px(50.0), px(12.0)), Some(0)), // Middle of first line
            (point(px(50.0), px(36.0)), Some(1)), // Middle of second line
            (point(px(50.0), px(60.0)), Some(2)), // Middle of third line
            (point(px(50.0), px(100.0)), None),   // Below all lines
        ];

        for (position, expected_line) in test_cases {
            let found = cache.find_line_at_position(position, bounds_width, line_height);
            match expected_line {
                Some(expected_idx) => {
                    assert!(
                        found.is_some(),
                        "Should find line at position {:?}",
                        position
                    );
                    assert_eq!(found.unwrap().line_idx, expected_idx);
                }
                None => {
                    assert!(
                        found.is_none(),
                        "Should not find line at position {:?}",
                        position
                    );
                }
            }
        }
    }

    #[test]
    fn test_line_lookup_with_scroll() {
        let cache = LineLayoutCache::new();
        let line_height = px(24.0);
        let bounds_width = px(800.0);
        let scroll_offset = point(px(0.0), px(-48.0)); // Scrolled down 2 lines

        // Add test lines at their unscrolled positions
        for line_idx in 0..5 {
            let y_position = px(line_idx as f32 * line_height.0);
            let layout = LineLayout {
                line_idx,
                shaped_line: create_test_shaped_line(),
                origin: point(px(0.0), y_position),
                segment_char_offset: 0,
                text_start_byte_offset: 0,
            };
            cache.push(layout);
        }

        // Test finding lines considering scroll offset
        let test_cases = [
            // Position (50, 12) after scroll adjustment should hit line 2
            (point(px(50.0), px(12.0)), Some(2)),
            // Position (50, 36) after scroll adjustment should hit line 3
            (point(px(50.0), px(36.0)), Some(3)),
        ];

        for (position, expected_line) in test_cases {
            let found = cache.find_line_at_position_with_scroll(
                position,
                bounds_width,
                line_height,
                scroll_offset,
            );
            match expected_line {
                Some(expected_idx) => {
                    assert!(
                        found.is_some(),
                        "Should find line at position {:?} with scroll",
                        position
                    );
                    assert_eq!(found.unwrap().line_idx, expected_idx);
                }
                None => {
                    assert!(
                        found.is_none(),
                        "Should not find line at position {:?} with scroll",
                        position
                    );
                }
            }
        }
    }

    #[test]
    fn test_cursor_line_lookup_scenario() {
        // Test the specific scenario from cursor rendering:
        // Cursor is on line 9, should find that line in the cache
        let cache = LineLayoutCache::new();

        // Simulate line layouts for lines 0-15 (typical visible range)
        for line_idx in 0..16 {
            let layout = LineLayout {
                line_idx,
                shaped_line: create_test_shaped_line(),
                origin: point(px(0.0), px(line_idx as f32 * 24.0)),
                segment_char_offset: 0,
                text_start_byte_offset: 0,
            };
            cache.push(layout);
        }

        // Test finding cursor line 9 (from debug logs)
        let cursor_line = 9;
        let found = cache.find_line_by_index(cursor_line);
        assert!(found.is_some(), "Should find cursor line {}", cursor_line);

        let layout = found.unwrap();
        assert_eq!(layout.line_idx, cursor_line);
        assert_eq!(layout.origin.y, px(216.0)); // 9 * 24.0 = 216
    }

    #[test]
    fn test_empty_cache_lookups() {
        let cache = LineLayoutCache::new();

        // Test lookups on empty cache
        assert!(cache.find_line_by_index(0).is_none());
        assert!(cache
            .find_line_at_position(point(px(50.0), px(12.0)), px(800.0), px(24.0))
            .is_none());
    }

    #[test]
    fn test_cache_clear_functionality() {
        let cache = LineLayoutCache::new();

        // Add a test line
        let layout = LineLayout {
            line_idx: 0,
            shaped_line: create_test_shaped_line(),
            origin: point(px(0.0), px(0.0)),
            segment_char_offset: 0,
            text_start_byte_offset: 0,
        };
        cache.push(layout);

        // Verify it exists
        assert!(cache.find_line_by_index(0).is_some());

        // Clear cache
        cache.clear();

        // Verify it's gone
        assert!(cache.find_line_by_index(0).is_none());
    }

    #[test]
    fn test_binary_search_performance_correctness() {
        let cache = LineLayoutCache::new();
        let line_height = px(24.0);
        let bounds_width = px(800.0);

        // Add many test lines to verify binary search works with larger datasets
        let num_lines = 100;
        for line_idx in 0..num_lines {
            let y_position = px(line_idx as f32 * line_height.0);
            let layout = LineLayout {
                line_idx,
                shaped_line: create_test_shaped_line(),
                origin: point(px(0.0), y_position),
                segment_char_offset: 0,
                text_start_byte_offset: 0,
            };
            cache.push(layout);
        }

        // Test various positions to ensure binary search finds correct lines
        let test_cases = [
            // First line
            (point(px(50.0), px(5.0)), Some(0)),
            // Middle lines
            (point(px(50.0), px(25.0)), Some(1)),
            (point(px(50.0), px(49.0)), Some(2)),
            (point(px(50.0), px(120.0)), Some(5)),
            (point(px(50.0), px(480.0)), Some(20)),
            // Near end
            (point(px(50.0), px(2350.0)), Some(97)),
            (point(px(50.0), px(2374.0)), Some(98)),
            (point(px(50.0), px(2398.0)), Some(99)),
            // Beyond all lines
            (point(px(50.0), px(3000.0)), None),
            // Before all lines (negative Y)
            (point(px(50.0), px(-10.0)), None),
            // X position outside bounds (should return None)
            (point(px(1000.0), px(50.0)), None),
        ];

        for (position, expected_line) in test_cases {
            let found = cache.find_line_at_position(position, bounds_width, line_height);
            match expected_line {
                Some(expected_idx) => {
                    assert!(
                        found.is_some(),
                        "Binary search should find line at position {:?}, expected line {}",
                        position,
                        expected_idx
                    );
                    let found_layout = found.unwrap();
                    assert_eq!(
                        found_layout.line_idx, expected_idx,
                        "Binary search found wrong line at position {:?}, expected {}, got {}",
                        position, expected_idx, found_layout.line_idx
                    );
                }
                None => {
                    assert!(
                        found.is_none(),
                        "Binary search should not find line at position {:?}, but found line {}",
                        position,
                        found.map(|l| l.line_idx).unwrap_or(999)
                    );
                }
            }
        }
    }

    #[test]
    fn test_binary_search_edge_cases() {
        let cache = LineLayoutCache::new();
        let line_height = px(24.0);
        let bounds_width = px(800.0);

        // Test empty cache
        assert!(cache
            .find_line_at_position(point(px(50.0), px(12.0)), bounds_width, line_height)
            .is_none());

        // Test single line
        let layout = LineLayout {
            line_idx: 0,
            shaped_line: create_test_shaped_line(),
            origin: point(px(0.0), px(0.0)),
            segment_char_offset: 0,
            text_start_byte_offset: 0,
        };
        cache.push(layout);

        // Should find the single line
        assert!(cache
            .find_line_at_position(point(px(50.0), px(12.0)), bounds_width, line_height)
            .is_some());
        // Should not find outside the line bounds
        assert!(cache
            .find_line_at_position(point(px(50.0), px(30.0)), bounds_width, line_height)
            .is_none());

        // Test two lines (boundary case for binary search)
        let layout2 = LineLayout {
            line_idx: 1,
            shaped_line: create_test_shaped_line(),
            origin: point(px(0.0), px(24.0)),
            segment_char_offset: 0,
            text_start_byte_offset: 0,
        };
        cache.push(layout2);

        // Test positions at the boundaries between lines
        assert_eq!(
            cache
                .find_line_at_position(point(px(50.0), px(23.9)), bounds_width, line_height)
                .unwrap()
                .line_idx,
            0
        );
        assert_eq!(
            cache
                .find_line_at_position(point(px(50.0), px(24.0)), bounds_width, line_height)
                .unwrap()
                .line_idx,
            1
        );
        assert_eq!(
            cache
                .find_line_at_position(point(px(50.0), px(47.9)), bounds_width, line_height)
                .unwrap()
                .line_idx,
            1
        );
    }
}
