// ABOUTME: Line layout cache for mouse interaction in document view
// ABOUTME: Provides thread-safe storage of line layouts without RefCell

use gpui::{size, Bounds, Pixels, ShapedLine};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Layout information for a single line in the document
#[derive(Clone)]
pub struct LineLayout {
    pub line_idx: usize,
    pub shaped_line: ShapedLine,
    pub origin: gpui::Point<Pixels>,
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

    pub fn find_line_at_position(
        &self,
        position: gpui::Point<Pixels>,
        bounds_width: Pixels,
        line_height: Pixels,
    ) -> Option<LineLayout> {
        if let Ok(layouts) = self.layouts.lock() {
            layouts
                .iter()
                .find(|layout| {
                    let line_bounds = Bounds {
                        origin: layout.origin,
                        size: size(bounds_width, line_height),
                    };
                    line_bounds.contains(&position)
                })
                .cloned()
        } else {
            None
        }
    }

    /// Find line at position accounting for scroll offset
    /// The scroll_offset represents how much the content has been shifted (GPUI convention: negative when scrolled down)
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
