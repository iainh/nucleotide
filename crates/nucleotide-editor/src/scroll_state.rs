// ABOUTME: Scroll state management for the editor view
// ABOUTME: Tracks scroll position and visible range independently

use gpui::{Pixels, px};

/// Scroll state for the editor
#[derive(Debug, Clone)]
pub struct ScrollState {
    /// Current scroll position in pixels
    scroll_y: Pixels,

    /// First visible line
    first_visible_line: usize,

    /// Last visible line
    last_visible_line: usize,

    /// Total number of lines
    total_lines: usize,

    /// Viewport height
    viewport_height: Pixels,
}

impl ScrollState {
    /// Create new scroll state
    pub fn new() -> Self {
        Self {
            scroll_y: Pixels::ZERO,
            first_visible_line: 0,
            last_visible_line: 0,
            total_lines: 0,
            viewport_height: px(800.0),
        }
    }

    /// Reset scroll to top
    pub fn reset(&mut self) {
        self.scroll_y = Pixels::ZERO;
        self.first_visible_line = 0;
        self.update_visible_range();
    }

    /// Scroll by a number of lines
    pub fn scroll_by_lines(&mut self, lines: i32, line_height: Pixels) {
        let delta = line_height * (lines as f32);
        self.scroll_y = (self.scroll_y + delta).max(Pixels::ZERO);
        self.update_from_scroll_position(line_height);
    }

    /// Scroll to a specific line
    pub fn scroll_to_line(&mut self, line: usize, line_height: Pixels) {
        self.scroll_y = line_height * (line as f32);
        self.first_visible_line = line;
        self.update_visible_range();
    }

    /// Update scroll from pixel position
    pub fn set_scroll_position(&mut self, y: Pixels, line_height: Pixels) {
        self.scroll_y = y.max(Pixels::ZERO);
        self.update_from_scroll_position(line_height);
    }

    /// Get current scroll position
    pub fn scroll_position(&self) -> Pixels {
        self.scroll_y
    }

    /// Get first visible line
    pub fn first_visible_line(&self) -> usize {
        self.first_visible_line
    }

    /// Get last visible line
    pub fn last_visible_line(&self) -> usize {
        self.last_visible_line
    }

    /// Set viewport height
    pub fn set_viewport_height(&mut self, height: Pixels, line_height: Pixels) {
        self.viewport_height = height;
        self.update_visible_range_with_height(line_height);
    }

    /// Set total number of lines
    pub fn set_total_lines(&mut self, lines: usize) {
        self.total_lines = lines;
        self.update_visible_range();
    }

    /// Check if a line is visible
    pub fn is_line_visible(&self, line: usize) -> bool {
        line >= self.first_visible_line && line <= self.last_visible_line
    }

    /// Ensure a line is visible
    pub fn ensure_line_visible(&mut self, line: usize, line_height: Pixels) {
        let line_height_value: f32 = line_height.into();
        if line_height_value <= 0.0 {
            return;
        }

        if line < self.first_visible_line {
            self.scroll_to_line(line, line_height);
        } else if line > self.last_visible_line {
            let lines_in_view = (self.viewport_height / line_height) as usize;
            let new_first = line.saturating_sub(lines_in_view - 1);
            self.scroll_to_line(new_first, line_height);
        }
    }

    // Private helper methods

    fn update_from_scroll_position(&mut self, line_height: Pixels) {
        let line_height_value: f32 = line_height.into();
        if line_height_value <= 0.0 {
            return;
        }

        let scroll_y = self.scroll_y / line_height;
        self.first_visible_line = scroll_y as usize;
        self.update_visible_range_with_height(line_height);
    }

    fn update_visible_range(&mut self) {
        // Default line height of 20px if not specified
        self.update_visible_range_with_height(px(20.0));
    }

    fn update_visible_range_with_height(&mut self, line_height: Pixels) {
        let line_height_value: f32 = line_height.into();
        if line_height_value <= 0.0 {
            return;
        }

        let ratio = self.viewport_height / line_height;
        let lines_in_view = ratio.ceil() as usize;
        self.last_visible_line =
            (self.first_visible_line + lines_in_view).min(self.total_lines.saturating_sub(1));
    }
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new()
    }
}
