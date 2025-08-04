// ABOUTME: Line layout cache for mouse interaction in document view
// ABOUTME: Provides thread-safe storage of line layouts without RefCell

use gpui::*;
use std::sync::{Arc, Mutex};

/// Layout information for a single line in the document
#[derive(Clone)]
pub struct LineLayout {
    pub line_idx: usize,
    pub shaped_line: ShapedLine,
    pub origin: gpui::Point<Pixels>,
}

/// Thread-safe cache for line layouts
#[derive(Clone, Default)]
pub struct LineLayoutCache {
    layouts: Arc<Mutex<Vec<LineLayout>>>,
}

impl LineLayoutCache {
    pub fn new() -> Self {
        Self {
            layouts: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    pub fn clear(&self) {
        if let Ok(mut layouts) = self.layouts.lock() {
            layouts.clear();
        }
    }
    
    pub fn push(&self, layout: LineLayout) {
        if let Ok(mut layouts) = self.layouts.lock() {
            layouts.push(layout);
        }
    }
    
    pub fn find_line_at_position(&self, position: gpui::Point<Pixels>, bounds_width: Pixels, line_height: Pixels) -> Option<LineLayout> {
        if let Ok(layouts) = self.layouts.lock() {
            layouts.iter().find(|layout| {
                let line_bounds = Bounds {
                    origin: layout.origin,
                    size: size(bounds_width, line_height),
                };
                line_bounds.contains(&position)
            }).cloned()
        } else {
            None
        }
    }
    
    pub fn find_line_by_index(&self, line_idx: usize) -> Option<LineLayout> {
        if let Ok(layouts) = self.layouts.lock() {
            layouts.iter().find(|layout| layout.line_idx == line_idx).cloned()
        } else {
            None
        }
    }
}

impl gpui::Global for LineLayoutCache {}