// ABOUTME: Placeholder terminal view crate – to be implemented with GPUI

use nucleotide_events::v2::terminal::TerminalId;

#[cfg(feature = "emulator")]
use gpui::AppContext; // bring trait into scope for Context::new
#[cfg(feature = "emulator")]
use gpui::FontWeight;
#[cfg(feature = "emulator")]
use gpui::InteractiveElement;
#[cfg(feature = "emulator")]
use gpui::rgb;
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div};
#[cfg(feature = "emulator")]
use nucleotide_terminal::frame::{Cell, FramePayload, GridDiff, GridSnapshot};
use nucleotide_ui::ThemedContext;
#[cfg(feature = "emulator")]
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
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
    #[cfg(feature = "emulator")]
    cell_width: f32,
    #[cfg(feature = "emulator")]
    cell_height: f32,
    #[cfg(feature = "emulator")]
    pub history_size: usize,
    #[cfg(feature = "emulator")]
    pub display_offset: usize,
    #[cfg(feature = "emulator")]
    control_tx: Option<std::sync::mpsc::Sender<nucleotide_terminal::session::ControlMsg>>,
    #[cfg(feature = "emulator")]
    scroll_dragging: bool,
    /// Set to true when the shell process has exited
    exited: bool,
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
            #[cfg(feature = "emulator")]
            cell_width: 0.0,
            #[cfg(feature = "emulator")]
            cell_height: 0.0,
            #[cfg(feature = "emulator")]
            history_size: 0,
            #[cfg(feature = "emulator")]
            display_offset: 0,
            #[cfg(feature = "emulator")]
            control_tx: None,
            #[cfg(feature = "emulator")]
            scroll_dragging: false,
            exited: false,
        }
    }

    #[cfg(feature = "emulator")]
    pub fn set_control_sender(
        &mut self,
        tx: std::sync::mpsc::Sender<nucleotide_terminal::session::ControlMsg>,
    ) {
        self.control_tx = Some(tx);
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
    pub fn has_dirty_rows(&self) -> bool {
        self.dirty.flags.iter().any(|&f| f)
    }

    #[cfg(feature = "emulator")]
    fn set_snapshot(&mut self, snapshot: GridSnapshot) {
        self.cols = snapshot.cols;
        self.rows = snapshot.rows_len;
        self.grid = snapshot.rows;
        self.cursor_row = snapshot.cursor_row;
        self.cursor_col = snapshot.cursor_col;
        self.history_size = snapshot.history_size;
        // Don't overwrite display_offset during scrollbar drag to avoid
        // race conditions where stale frames snap the scroll position back.
        if !self.scroll_dragging {
            self.display_offset = snapshot.display_offset;
        }
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

    /// Resets scroll position to bottom (live cursor), snapping back from scrollback.
    /// Returns `true` if the offset actually changed.
    #[cfg(feature = "emulator")]
    pub fn scroll_to_bottom(&mut self) -> bool {
        if self.display_offset == 0 {
            return false;
        }
        let delta = -(self.display_offset as i32);
        self.display_offset = 0;
        if let Some(tx) = &self.control_tx {
            let _ = tx.send(nucleotide_terminal::session::ControlMsg::Scroll { delta });
        }
        true
    }

    #[cfg(feature = "emulator")]
    pub fn resize_grid(&mut self, cols: u16, rows: u16, cell_metrics: Option<(f32, f32)>) {
        let cols_usize = cols.max(1) as usize;
        let rows_usize = rows.max(1) as usize;

        if self.grid.len() > rows_usize {
            self.grid.truncate(rows_usize);
        }
        while self.grid.len() < rows_usize {
            self.grid.push(vec![blank_cell(); cols_usize]);
        }

        for row in &mut self.grid {
            if row.len() > cols_usize {
                row.truncate(cols_usize);
            } else if row.len() < cols_usize {
                row.resize(cols_usize, blank_cell());
            }
        }

        self.cols = cols;
        self.rows = rows;
        self.cursor_row = self.cursor_row.min(rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(cols.saturating_sub(1));
        self.dirty.resize_and_fill(self.grid.len(), true);
        if let Some((cw, ch)) = cell_metrics {
            self.cell_width = cw.max(1.0);
            self.cell_height = ch.max(1.0);
        }
    }

    pub fn set_exited(&mut self) {
        self.exited = true;
    }

    pub fn has_exited(&self) -> bool {
        self.exited
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

/// Bridges the terminal's line-based scrollback to the pixel-based `ScrollableHandle` trait.
#[cfg(feature = "emulator")]
#[derive(Debug)]
pub struct TerminalScrollHandle {
    model: Arc<Mutex<TerminalViewModel>>,
}

#[cfg(feature = "emulator")]
impl nucleotide_ui::scrollbar::ScrollableHandle for TerminalScrollHandle {
    fn max_offset(&self) -> gpui::Size<gpui::Pixels> {
        let guard = self.model.lock().unwrap();
        let max_y = guard.history_size as f32 * guard.cell_height.max(1.0);
        gpui::Size {
            width: gpui::px(0.0),
            height: gpui::px(max_y),
        }
    }

    fn offset(&self) -> gpui::Point<gpui::Pixels> {
        let guard = self.model.lock().unwrap();
        // display_offset=0 → at bottom (live) → most scrolled → offset = -max
        // display_offset=history_size → at top → not scrolled → offset = 0
        let scrolled_lines = (guard.history_size - guard.display_offset) as f32;
        let y = -(scrolled_lines * guard.cell_height.max(1.0));
        gpui::Point {
            x: gpui::px(0.0),
            y: gpui::px(y),
        }
    }

    fn set_offset(&self, point: gpui::Point<gpui::Pixels>) {
        let mut guard = self.model.lock().unwrap();
        let cell_h = guard.cell_height.max(1.0);
        let history = guard.history_size;
        // Convert pixel offset back to display_offset
        // offset.y is negative; abs gives pixels scrolled from top
        let scrolled_lines = (f32::from(point.y).abs() / cell_h).round() as usize;
        let new_display_offset = history.saturating_sub(scrolled_lines);
        let delta = new_display_offset as i32 - guard.display_offset as i32;
        guard.display_offset = new_display_offset;
        if delta != 0
            && let Some(tx) = &guard.control_tx
        {
            let _ = tx.send(nucleotide_terminal::session::ControlMsg::Scroll { delta });
        }
    }

    fn viewport(&self) -> gpui::Bounds<gpui::Pixels> {
        let guard = self.model.lock().unwrap();
        let h = guard.rows as f32 * guard.cell_height.max(1.0);
        let w = guard.cols as f32 * guard.cell_width.max(1.0);
        gpui::Bounds::new(
            gpui::Point {
                x: gpui::px(0.0),
                y: gpui::px(0.0),
            },
            gpui::Size {
                width: gpui::px(w),
                height: gpui::px(h),
            },
        )
    }

    fn drag_started(&self) {
        self.model.lock().unwrap().scroll_dragging = true;
    }

    fn drag_ended(&self) {
        self.model.lock().unwrap().scroll_dragging = false;
    }
}

/// A simple GPUI component that renders a TerminalViewModel as text lines.
pub struct TerminalView {
    pub model: Arc<Mutex<TerminalViewModel>>,
    #[cfg(feature = "emulator")]
    rows: Vec<gpui::Entity<TerminalRowView>>,
    #[cfg(feature = "emulator")]
    scrollbar_state: Option<ScrollbarState>,
}

impl TerminalView {
    pub fn new(
        model: Arc<Mutex<TerminalViewModel>>,
        #[allow(unused)] cx: &mut Context<Self>,
    ) -> Self {
        #[cfg(feature = "emulator")]
        let scrollbar_state = {
            let handle = TerminalScrollHandle {
                model: model.clone(),
            };
            Some(ScrollbarState::new(handle))
        };

        // Poll for background frame updates and trigger GPUI re-renders.
        // The frame consumer thread updates the view model on a background
        // thread with no access to GPUI; this task bridges the gap.
        #[cfg(feature = "emulator")]
        {
            let poll_model = model.clone();
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(32))
                        .await;
                    let has_updates = {
                        let Ok(guard) = poll_model.lock() else {
                            break;
                        };
                        guard.has_dirty_rows()
                    };
                    if has_updates && this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        Self {
            model,
            #[cfg(feature = "emulator")]
            rows: Vec::new(),
            #[cfg(feature = "emulator")]
            scrollbar_state,
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = _cx.theme();
        let tokens = &theme.tokens;
        let default_bg = tokens.editor.background;
        let default_fg = tokens.editor.text_primary;

        // Terminal content column (flex_1 so scrollbar gets space)
        let mut content = div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .flex_1()
            .overflow_hidden()
            .bg(default_bg)
            .text_color(default_fg)
            .text_size(tokens.sizes.text_md);

        // Apply editor font configuration (family/size/weight) to match the editor
        let editor_font = _cx.global::<nucleotide_types::EditorFontConfig>();
        content = content
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
                content = content.child(row_ent.clone());
            }
        }

        #[cfg(not(feature = "emulator"))]
        {
            // No emulator: show a placeholder with correct styling
            content = content.child(div().child("Terminal emulator disabled"));
        }

        // Wrap content + scrollbar in a horizontal flex container
        #[cfg(feature = "emulator")]
        let wrapper = {
            // Add scroll wheel support (requires an id for interactive element)
            let scroll_model = self.model.clone();
            let interactive_content =
                content
                    .id("terminal-content")
                    .on_scroll_wheel(move |event, _window, _cx| {
                        let mut guard = scroll_model.lock().unwrap();
                        let cell_h = guard.cell_height.max(1.0);
                        let delta_y = f32::from(event.delta.pixel_delta(_window.line_height()).y);
                        let lines = (delta_y / cell_h).round() as i32;
                        if lines != 0 {
                            let new_offset = (guard.display_offset as i32 + lines)
                                .clamp(0, guard.history_size as i32)
                                as usize;
                            let delta = new_offset as i32 - guard.display_offset as i32;
                            guard.display_offset = new_offset;
                            if delta != 0
                                && let Some(tx) = &guard.control_tx
                            {
                                let _ = tx.send(nucleotide_terminal::session::ControlMsg::Scroll {
                                    delta,
                                });
                            }
                        }
                    });
            let mut w = div()
                .flex()
                .flex_row()
                .size_full()
                .child(interactive_content);
            if let Some(state) = &self.scrollbar_state
                && let Some(scrollbar) = Scrollbar::vertical(state.clone())
            {
                w = w.child(scrollbar);
            }
            w
        };
        #[cfg(not(feature = "emulator"))]
        let wrapper = div().flex().flex_row().size_full().child(content);

        wrapper
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

        // Helper to convert emulator RGB u32 to gpui color
        #[inline]
        fn color_from_u32(c: u32) -> gpui::Hsla {
            rgb(c).into()
        }

        let (grid_row, cursor_row, cursor_col, cell_height) = {
            let guard = self.model.lock().unwrap();
            let row = if self.row_index < guard.grid.len() {
                guard.grid[self.row_index].clone()
            } else {
                Vec::new()
            };
            (
                row,
                guard.cursor_row as usize,
                guard.cursor_col as usize,
                guard.cell_height,
            )
        };

        let fallback_line_height = editor_font.size * 1.35;
        let applied_line_height = if cell_height > 0.0 {
            cell_height
        } else {
            fallback_line_height
        };
        let line_height_px = gpui::px(applied_line_height);

        let mut line = div()
            .flex()
            .flex_row()
            .flex_shrink(1.0)
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
