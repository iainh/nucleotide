// ABOUTME: Placeholder terminal view crate – to be implemented with GPUI

use nucleotide_events::v2::terminal::TerminalId;

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div};
#[cfg(feature = "emulator")]
use nucleotide_terminal::frame::{Cell, FramePayload, GridDiff, GridSnapshot};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct TerminalViewModel {
    pub id: TerminalId,
    #[cfg(feature = "emulator")]
    grid: Vec<Vec<Cell>>, // current grid
    #[cfg(feature = "emulator")]
    cols: u16,
    #[cfg(feature = "emulator")]
    rows: u16,
}

impl TerminalViewModel {
    pub fn new(id: TerminalId) -> Self {
        Self {
            id,
            #[cfg(feature = "emulator")]
            grid: Vec::new(),
            #[cfg(feature = "emulator")]
            cols: 0,
            #[cfg(feature = "emulator")]
            rows: 0,
        }
    }

    #[cfg(feature = "emulator")]
    pub fn apply_frame(&mut self, frame: FramePayload) {
        match frame {
            FramePayload::Full(snapshot) => self.set_snapshot(snapshot),
            FramePayload::Diff(diff) => self.apply_diff(diff),
            FramePayload::Raw(_) => {}
        }
    }

    #[cfg(not(feature = "emulator"))]
    pub fn apply_frame(&mut self, _frame: ()) { /* no-op without emulator */
    }

    #[cfg(feature = "emulator")]
    fn set_snapshot(&mut self, snapshot: GridSnapshot) {
        self.cols = snapshot.cols;
        self.rows = snapshot.rows_len;
        self.grid = snapshot.rows;
    }

    #[cfg(feature = "emulator")]
    fn apply_diff(&mut self, diff: GridDiff) {
        if self.grid.is_empty() || self.cols == 0 || self.rows == 0 {
            return;
        }
        if let Some(delta) = diff.scrolled {
            if delta > 0 {
                let d = delta as usize;
                for _ in 0..d {
                    self.grid.remove(0);
                }
                for _ in 0..d {
                    self.grid.push(vec![blank_cell(); self.cols as usize]);
                }
            } else if delta < 0 {
                let d = (-delta) as usize;
                for _ in 0..d {
                    self.grid.pop();
                }
                for _ in 0..d {
                    self.grid.insert(0, vec![blank_cell(); self.cols as usize]);
                }
            }
        }
        for line in diff.lines {
            let row = line.row as usize;
            if row >= self.grid.len() {
                continue;
            }
            for range in line.ranges {
                let start = range.col as usize;
                let end = (start + range.cells.len()).min(self.grid[row].len());
                for (i, cell) in range.cells.into_iter().enumerate() {
                    if start + i < end {
                        self.grid[row][start + i] = cell;
                    }
                }
            }
        }
    }
}

#[cfg(feature = "emulator")]
fn blank_cell() -> Cell {
    Cell {
        ch: ' ',
        fg: 0xffffff,
        bg: 0x000000,
        bold: false,
        italic: false,
        underline: false,
        inverse: false,
    }
}

/// A simple GPUI component that renders a TerminalViewModel as text lines.
pub struct TerminalView {
    pub model: Arc<Mutex<TerminalViewModel>>,
}

impl TerminalView {
    pub fn new(model: Arc<Mutex<TerminalViewModel>>) -> Self {
        Self { model }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = {
            let guard = self.model.lock().unwrap();
            #[cfg(feature = "emulator")]
            {
                // Render grid rows into strings (ignoring per-cell color/style for now)
                let mut lines: Vec<String> = Vec::with_capacity(guard.grid.len());
                for row in &guard.grid {
                    let s: String = row.iter().map(|c| c.ch).collect();
                    lines.push(s);
                }
                lines
            }
            #[cfg(not(feature = "emulator"))]
            {
                vec![String::from("Terminal emulator disabled")] // placeholder
            }
        };

        let mut container = div().flex().flex_col().size_full().overflow_hidden();
        for line in snapshot {
            container = container.child(div().text_size(gpui::px(12.0)).child(line));
        }
        container
    }
}

// Global registry to share TerminalViewModel instances across UI and runtime
static TERMINAL_VIEW_REGISTRY: Lazy<Mutex<HashMap<TerminalId, Arc<Mutex<TerminalViewModel>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn register_view_model(id: TerminalId, model: Arc<Mutex<TerminalViewModel>>) {
    let mut map = TERMINAL_VIEW_REGISTRY.lock().unwrap();
    map.insert(id, model);
}

pub fn get_view_model(id: TerminalId) -> Option<Arc<Mutex<TerminalViewModel>>> {
    let map = TERMINAL_VIEW_REGISTRY.lock().unwrap();
    map.get(&id).cloned()
}
