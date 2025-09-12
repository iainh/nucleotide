// ABOUTME: Placeholder terminal view crate – to be implemented with GPUI

use nucleotide_events::v2::terminal::TerminalId;

use gpui::AppContext; // bring trait into scope for Context::new
#[cfg(feature = "emulator")]
use gpui::FontWeight;
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, rgb};
#[cfg(feature = "emulator")]
use nucleotide_terminal::frame::{Cell, FramePayload, GridDiff, GridSnapshot};
use nucleotide_ui::ThemedContext;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[cfg(feature = "emulator")]
#[derive(Debug, Default, Clone)]
struct DirtyFlags {
    flags: Vec<bool>,
}

#[cfg(feature = "emulator")]
impl DirtyFlags {
    fn resize_and_fill(&mut self, len: usize, value: bool) {
        self.flags.clear();
        self.flags.resize(len, value);
    }

    fn mark(&mut self, idx: usize) {
        if idx < self.flags.len() {
            self.flags[idx] = true;
        }
    }

    fn take(&mut self) -> Vec<usize> {
        let mut dirty = Vec::new();
        for (i, f) in self.flags.iter_mut().enumerate() {
            if *f {
                dirty.push(i);
                *f = false;
            }
        }
        dirty
    }
}

#[derive(Debug)]
pub struct TerminalViewModel {
    pub id: TerminalId,
    #[cfg(feature = "emulator")]
    grid: Vec<Vec<Cell>>, // current grid
    #[cfg(feature = "emulator")]
    cols: u16,
    #[cfg(feature = "emulator")]
    rows: u16,
    #[cfg(feature = "emulator")]
    cursor_row: u16,
    #[cfg(feature = "emulator")]
    cursor_col: u16,
    #[cfg(feature = "emulator")]
    dirty: DirtyFlags,
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
            #[cfg(feature = "emulator")]
            cursor_row: 0,
            #[cfg(feature = "emulator")]
            cursor_col: 0,
            #[cfg(feature = "emulator")]
            dirty: DirtyFlags::default(),
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
        self.cursor_row = snapshot.cursor_row;
        self.cursor_col = snapshot.cursor_col;
        self.dirty.resize_and_fill(self.grid.len(), true);
    }

    #[cfg(feature = "emulator")]
    fn apply_diff(&mut self, diff: GridDiff) {
        if self.grid.is_empty() || self.cols == 0 || self.rows == 0 {
            return;
        }
        // Update cursor position first
        self.cursor_row = diff.cursor_row.min(self.rows.saturating_sub(1));
        self.cursor_col = diff.cursor_col.min(self.cols.saturating_sub(1));
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
            self.dirty.mark(row);
        }
    }

    #[cfg(feature = "emulator")]
    pub fn take_dirty_rows(&mut self) -> Vec<usize> {
        self.dirty.take()
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
    #[cfg(feature = "emulator")]
    rows: Vec<gpui::Entity<TerminalRowView>>, // row entities for dirty rendering
}

impl TerminalView {
    pub fn new(model: Arc<Mutex<TerminalViewModel>>) -> Self {
        Self {
            model,
            #[cfg(feature = "emulator")]
            rows: Vec::new(),
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = _cx.theme();
        let tokens = &theme.tokens;
        let default_bg = tokens.editor.background;
        let default_fg = tokens.editor.text_primary;

        // Base container styling: editor-like background, foreground, and configured monospace font
        let mut container = div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(default_bg)
            .text_color(default_fg)
            .text_size(tokens.sizes.text_md);

        // Apply editor font configuration (family/size/weight) to match the editor
        let editor_font = _cx.global::<nucleotide_types::EditorFontConfig>();
        container = container
            .font_family(editor_font.family.clone())
            .text_size(gpui::px(editor_font.size))
            .font_weight(editor_font.weight.into());

        // Render
        #[cfg(feature = "emulator")]
        {
            let rows_len = { self.model.lock().unwrap().grid.len() };

            // Ensure we have the right number of row entities
            if self.rows.len() != rows_len {
                // Recreate the full set if length changes materially (e.g., resize)
                self.rows.clear();
                for row_index in 0..rows_len {
                    let model = self.model.clone();
                    let row_ent =
                        _cx.new(move |_cx| TerminalRowView::new(model.clone(), row_index));
                    self.rows.push(row_ent);
                }
            }

            // Take dirty rows and update only those
            let dirty_rows = { self.model.lock().unwrap().take_dirty_rows() };
            for idx in dirty_rows {
                if let Some(ent) = self.rows.get(idx) {
                    ent.update(_cx, |row, cx| row.mark_dirty(cx));
                }
            }

            // Mount all row entities
            for row_ent in &self.rows {
                container = container.child(row_ent.clone());
            }
        }

        #[cfg(not(feature = "emulator"))]
        {
            // No emulator: show a placeholder with correct styling
            container = container.child(div().child("Terminal emulator disabled"));
        }

        container
    }
}

#[cfg(feature = "emulator")]
pub struct TerminalRowView {
    model: Arc<Mutex<TerminalViewModel>>,
    row_index: usize,
    // simple version to force rerender on change
    version: u64,
}

#[cfg(feature = "emulator")]
impl TerminalRowView {
    pub fn new(model: Arc<Mutex<TerminalViewModel>>, row_index: usize) -> Self {
        Self {
            model,
            row_index,
            version: 0,
        }
    }

    pub fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.version = self.version.wrapping_add(1);
        cx.notify();
    }
}

#[cfg(feature = "emulator")]
impl Render for TerminalRowView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let editor_font = cx.global::<nucleotide_types::EditorFontConfig>();
        let line_height_px = gpui::px(editor_font.size * 1.35);

        // Helper to convert emulator RGB u32 to gpui color
        #[inline]
        fn color_from_u32(c: u32) -> gpui::Hsla {
            rgb(c).into()
        }

        let (grid_row, cursor_row, cursor_col) = {
            let guard = self.model.lock().unwrap();
            let row = if self.row_index < guard.grid.len() {
                guard.grid[self.row_index].clone()
            } else {
                Vec::new()
            };
            (row, guard.cursor_row as usize, guard.cursor_col as usize)
        };

        let mut line = div()
            .flex()
            .flex_row()
            .whitespace_nowrap()
            .line_height(line_height_px)
            .text_size(gpui::px(editor_font.size));
        // Accumulate runs
        let mut cur_fg = 0xffffffff; // sentinel to force first run
        let mut cur_bg = 0xffffffff;
        let mut cur_bold = false;
        let mut cur_italic = false;
        let mut cur_underline = false;
        let mut buf = String::new();

        let flush_run = |line_in: gpui::Div,
                         text: &mut String,
                         fg: u32,
                         bg: u32,
                         bold: bool,
                         italic: bool,
                         underline: bool| {
            if text.is_empty() {
                return line_in;
            }
            // Map emulator defaults (fg=0xffffff, bg=0x000000) to theme defaults
            let mapped_fg = if fg == 0xffffff {
                None
            } else {
                Some(color_from_u32(fg))
            };
            let mapped_bg = if bg == 0x000000 {
                None
            } else {
                Some(color_from_u32(bg))
            };

            let mut run = div().child(std::mem::take(text));
            if let Some(c) = mapped_fg {
                run = run.text_color(c);
            }
            if let Some(c) = mapped_bg {
                run = run.bg(c);
            }
            if bold {
                run = run.font_weight(FontWeight::BOLD);
            }
            if italic {
                run = run.italic();
            }
            if underline {
                run = run.underline();
            }
            line_in.child(run)
        };

        for (i, cell) in grid_row.iter().enumerate() {
            let (fg, bg, bold, italic, underline) =
                (cell.fg, cell.bg, cell.bold, cell.italic, cell.underline);
            if fg != cur_fg
                || bg != cur_bg
                || bold != cur_bold
                || italic != cur_italic
                || underline != cur_underline
            {
                // flush previous run
                line = flush_run(
                    line,
                    &mut buf,
                    cur_fg,
                    cur_bg,
                    cur_bold,
                    cur_italic,
                    cur_underline,
                );
                cur_fg = fg;
                cur_bg = bg;
                cur_bold = bold;
                cur_italic = italic;
                cur_underline = underline;
            }
            // Cursor rendering: render a block cursor at (cursor_row, cursor_col)
            if self.row_index == cursor_row && i == cursor_col {
                // Flush any accumulated text
                line = flush_run(
                    line,
                    &mut buf,
                    cur_fg,
                    cur_bg,
                    cur_bold,
                    cur_italic,
                    cur_underline,
                );
                // Render the cursor cell as a block using theme tokens
                let mut run = div().child(cell.ch.to_string());
                run = run.bg(tokens.editor.cursor_normal);
                run = run.text_color(tokens.editor.text_on_primary);
                // Make cursor more prominent
                run = run.font_weight(FontWeight::BOLD);
                line = line.child(run);
                // Do not push this char into the normal buffer
                continue;
            }
            buf.push(cell.ch);
        }
        // flush last
        line = flush_run(
            line,
            &mut buf,
            cur_fg,
            cur_bg,
            cur_bold,
            cur_italic,
            cur_underline,
        );

        line
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
