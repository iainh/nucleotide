// ABOUTME: Placeholder terminal view crate – to be implemented with GPUI

use nucleotide_events::v2::terminal::TerminalId;

#[cfg(feature = "emulator")]
use gpui::AppContext;
use gpui::{Context, FontWeight, IntoElement, ParentElement, Render, Styled, Window, div};
#[cfg(feature = "emulator")]
use gpui::{Hsla, InteractiveElement, hsla, rgb};
#[cfg(feature = "emulator")]
use nucleotide_terminal::frame::{
    Cell, DEFAULT_BACKGROUND, DEFAULT_FOREGROUND, FramePayload, GridDiff, GridSnapshot,
    TerminalInputMode, ansi_color_index,
};
use nucleotide_ui::ThemedContext;
#[cfg(feature = "emulator")]
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
#[cfg(feature = "emulator")]
use nucleotide_ui::{ColorTheory, ContrastRatios, DesignTokens};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

const DEFAULT_TERMINAL_TITLE: &str = "Terminal";

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn sanitize_terminal_title(title: impl AsRef<str>) -> Option<String> {
    let title = title
        .as_ref()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    let title = title.trim();

    (!title.is_empty()).then(|| title.to_string())
}

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

    fn mark_all(&mut self) {
        self.flags.fill(true);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSpawnFailure {
    pub message: String,
    pub details: Vec<String>,
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
    #[cfg(feature = "emulator")]
    wheel_scroll_remainder: f32,
    #[cfg(feature = "emulator")]
    input_mode: TerminalInputMode,
    #[cfg(feature = "emulator")]
    input_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    window_title: Option<String>,
    spawn_failure: Option<TerminalSpawnFailure>,
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
            #[cfg(feature = "emulator")]
            wheel_scroll_remainder: 0.0,
            #[cfg(feature = "emulator")]
            input_mode: TerminalInputMode::default(),
            #[cfg(feature = "emulator")]
            input_tx: None,
            window_title: None,
            spawn_failure: None,
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
    pub fn set_input_sender(&mut self, tx: std::sync::mpsc::Sender<Vec<u8>>) {
        self.input_tx = Some(tx);
    }

    #[cfg(feature = "emulator")]
    pub fn clear_input_sender(&mut self) {
        self.input_tx = None;
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
    pub fn input_mode(&self) -> TerminalInputMode {
        self.input_mode
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
        self.input_mode = snapshot.input_mode;
        self.window_title = snapshot.title.and_then(sanitize_terminal_title);
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
            let visible_rows = self.grid.len();
            if delta > 0 {
                let d = (delta as usize).min(visible_rows);
                for _ in 0..d {
                    self.grid.remove(0);
                }
                for _ in 0..d {
                    self.grid.push(vec![blank_cell(); self.cols as usize]);
                }
            } else if delta < 0 {
                let d = ((-delta) as usize).min(visible_rows);
                for _ in 0..d {
                    self.grid.pop();
                }
                for _ in 0..d {
                    self.grid.insert(0, vec![blank_cell(); self.cols as usize]);
                }
            }
            self.dirty.mark_all();
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
        self.set_display_offset(0)
    }

    #[cfg(feature = "emulator")]
    fn set_display_offset(&mut self, display_offset: usize) -> bool {
        self.set_display_offset_internal(display_offset, true)
    }

    #[cfg(feature = "emulator")]
    fn set_display_offset_internal(
        &mut self,
        display_offset: usize,
        reset_wheel_remainder: bool,
    ) -> bool {
        let new_display_offset = display_offset.min(self.history_size);
        if self.display_offset == new_display_offset {
            if reset_wheel_remainder {
                self.wheel_scroll_remainder = 0.0;
            }
            return false;
        }

        let delta = new_display_offset as i32 - self.display_offset as i32;
        self.display_offset = new_display_offset;
        if reset_wheel_remainder {
            self.wheel_scroll_remainder = 0.0;
        }
        self.send_scroll_delta(delta);
        true
    }

    #[cfg(feature = "emulator")]
    fn scroll_wheel_by_pixel_delta(&mut self, pixel_delta_y: f32) -> bool {
        let cell_h = self.cell_height.max(1.0);
        let raw_lines = self.wheel_scroll_remainder + pixel_delta_y / cell_h;

        if self.should_send_alternate_scroll_input() {
            let whole_lines = raw_lines.trunc() as i32;
            self.wheel_scroll_remainder = raw_lines - whole_lines as f32;

            if whole_lines == 0 {
                return false;
            }

            return self.send_alternate_scroll_input(whole_lines);
        }

        if (raw_lines < 0.0 && self.display_offset == 0)
            || (raw_lines > 0.0 && self.display_offset == self.history_size)
        {
            self.wheel_scroll_remainder = 0.0;
            return false;
        }

        let whole_lines = raw_lines.trunc() as i32;
        self.wheel_scroll_remainder = raw_lines - whole_lines as f32;

        if whole_lines == 0 {
            return false;
        }

        let requested_offset = self.display_offset as i32 + whole_lines;
        if requested_offset < 0 || requested_offset > self.history_size as i32 {
            self.wheel_scroll_remainder = 0.0;
        }

        let new_offset = requested_offset.clamp(0, self.history_size as i32) as usize;
        self.set_display_offset_internal(new_offset, false)
    }

    #[cfg(feature = "emulator")]
    fn should_send_alternate_scroll_input(&self) -> bool {
        self.input_mode.alternate_screen
            && self.input_mode.alternate_scroll
            && !self.input_mode.mouse_mode
    }

    #[cfg(feature = "emulator")]
    fn send_alternate_scroll_input(&self, line_delta: i32) -> bool {
        let Some(tx) = &self.input_tx else {
            return false;
        };

        let sequence: &[u8] = match (line_delta > 0, self.input_mode.application_cursor) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1b[A",
            (false, true) => b"\x1bOB",
            (false, false) => b"\x1b[B",
        };
        let line_count = line_delta.unsigned_abs() as usize;
        let mut bytes = Vec::with_capacity(sequence.len() * line_count);
        for _ in 0..line_count {
            bytes.extend_from_slice(sequence);
        }

        tx.send(bytes).is_ok()
    }

    #[cfg(feature = "emulator")]
    fn send_scroll_delta(&self, delta: i32) {
        if delta == 0 {
            return;
        }

        if let Some(tx) = &self.control_tx {
            let _ = tx.send(nucleotide_terminal::session::ControlMsg::Scroll { delta });
        }
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

    pub fn set_window_title(&mut self, title: impl AsRef<str>) {
        self.window_title = sanitize_terminal_title(title);
    }

    pub fn window_title(&self) -> Option<&str> {
        self.window_title.as_deref()
    }

    pub fn display_title(&self) -> String {
        self.window_title
            .as_deref()
            .unwrap_or(DEFAULT_TERMINAL_TITLE)
            .to_string()
    }

    pub fn set_spawn_failure(&mut self, message: impl Into<String>, details: Vec<String>) {
        self.spawn_failure = Some(TerminalSpawnFailure {
            message: message.into(),
            details,
        });
        self.exited = true;
    }

    pub fn spawn_failure(&self) -> Option<TerminalSpawnFailure> {
        self.spawn_failure.clone()
    }

    pub fn has_spawn_failure(&self) -> bool {
        self.spawn_failure.is_some()
    }
}

#[cfg(feature = "emulator")]
fn blank_cell() -> Cell {
    Cell {
        ch: ' ',
        fg: DEFAULT_FOREGROUND,
        bg: DEFAULT_BACKGROUND,
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
        let guard = lock_or_recover(self.model.as_ref());
        let max_y = guard.history_size as f32 * guard.cell_height.max(1.0);
        gpui::Size {
            width: gpui::px(0.0),
            height: gpui::px(max_y),
        }
    }

    fn offset(&self) -> gpui::Point<gpui::Pixels> {
        let guard = lock_or_recover(self.model.as_ref());
        // display_offset=0 → at bottom (live) → most scrolled → offset = -max
        // display_offset=history_size → at top → not scrolled → offset = 0
        let scrolled_lines = guard.history_size.saturating_sub(guard.display_offset) as f32;
        let y = -(scrolled_lines * guard.cell_height.max(1.0));
        gpui::Point {
            x: gpui::px(0.0),
            y: gpui::px(y),
        }
    }

    fn set_offset(&self, point: gpui::Point<gpui::Pixels>) {
        let mut guard = lock_or_recover(self.model.as_ref());
        let cell_h = guard.cell_height.max(1.0);
        let history = guard.history_size;
        let max_y = history as f32 * cell_h;
        let clamped_y = f32::from(point.y).clamp(-max_y, 0.0);
        // Convert pixel offset back to display_offset
        // offset.y is negative; abs gives pixels scrolled from top.
        let scrolled_lines = (clamped_y.abs() / cell_h).round() as usize;
        let new_display_offset = history.saturating_sub(scrolled_lines);
        guard.set_display_offset(new_display_offset);
    }

    fn viewport(&self) -> gpui::Bounds<gpui::Pixels> {
        let guard = lock_or_recover(self.model.as_ref());
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
        lock_or_recover(self.model.as_ref()).scroll_dragging = true;
    }

    fn drag_ended(&self) {
        lock_or_recover(self.model.as_ref()).scroll_dragging = false;
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

        // Poll for background frame and failure updates. Runtime workers update
        // the view model on background threads with no access to GPUI.
        {
            let poll_model = model.clone();
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(32))
                        .await;
                    let has_updates = {
                        let guard = lock_or_recover(poll_model.as_ref());
                        let has_terminal_state_change =
                            guard.has_exited() || guard.has_spawn_failure();
                        #[cfg(feature = "emulator")]
                        {
                            has_terminal_state_change || guard.has_dirty_rows()
                        }
                        #[cfg(not(feature = "emulator"))]
                        {
                            has_terminal_state_change
                        }
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
        let failure = { lock_or_recover(self.model.as_ref()).spawn_failure() };

        if let Some(failure) = failure {
            let mut failure_content = div()
                .flex()
                .flex_col()
                .size_full()
                .overflow_hidden()
                .bg(default_bg)
                .text_color(default_fg)
                .p_3()
                .gap_2()
                .text_size(tokens.sizes.text_sm)
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(tokens.editor.error)
                        .child(failure.message),
                );

            for detail in failure.details {
                failure_content = failure_content.child(
                    div()
                        .font_family("monospace")
                        .text_size(tokens.sizes.text_xs)
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .child(detail),
                );
            }

            return failure_content;
        }

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
            let rows_len = { lock_or_recover(self.model.as_ref()).grid.len() };

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
            let dirty_rows = { lock_or_recover(self.model.as_ref()).take_dirty_rows() };
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
                    .on_scroll_wheel(move |event, window, cx| {
                        let mut guard = lock_or_recover(scroll_model.as_ref());
                        let delta_y = f32::from(event.delta.pixel_delta(window.line_height()).y);
                        if guard.scroll_wheel_by_pixel_delta(delta_y) {
                            window.refresh();
                        }
                        cx.stop_propagation();
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
const RGB_COLOR_MAX: u32 = 0x00ff_ffff;

#[cfg(feature = "emulator")]
#[derive(Debug, Clone, Copy)]
struct TerminalAnsiPalette {
    default_foreground: Hsla,
    default_background: Hsla,
    colors: [Hsla; 16],
}

#[cfg(feature = "emulator")]
impl TerminalAnsiPalette {
    fn from_tokens(tokens: &DesignTokens) -> Self {
        let default_background = Self::opaque(tokens.editor.background);
        let default_foreground =
            Self::ensure_readable(default_background, tokens.editor.text_primary);
        let is_dark = ColorTheory::relative_luminance(default_background) < 0.5;
        let neutral_saturation = default_background.s * 0.2;

        let black_seed = hsla(
            default_background.h,
            neutral_saturation,
            if is_dark { 0.32 } else { 0.08 },
            1.0,
        );
        let bright_black_seed = hsla(
            default_background.h,
            neutral_saturation,
            if is_dark { 0.48 } else { 0.28 },
            1.0,
        );
        let white_seed = hsla(
            default_background.h,
            neutral_saturation,
            if is_dark { 0.84 } else { 0.36 },
            1.0,
        );

        let red = Self::ensure_readable(default_background, tokens.editor.error);
        let green = Self::ensure_readable(default_background, tokens.editor.success);
        let yellow = Self::ensure_readable(default_background, tokens.editor.warning);
        let blue_seed =
            ColorTheory::mix_oklch(tokens.chrome.primary, tokens.editor.focus_ring, 0.25);
        let blue = Self::ensure_readable(default_background, blue_seed);
        let magenta_seed = ColorTheory::mix_oklch(tokens.editor.error, blue, 0.55);
        let magenta = Self::ensure_readable(default_background, magenta_seed);
        let cyan_seed = ColorTheory::mix_oklch(tokens.editor.success, blue, 0.50);
        let cyan = Self::ensure_readable(default_background, cyan_seed);

        let colors = [
            Self::ensure_readable(default_background, black_seed),
            red,
            green,
            yellow,
            blue,
            magenta,
            cyan,
            Self::ensure_readable(default_background, white_seed),
            Self::ensure_readable(default_background, bright_black_seed),
            Self::bright_variant(default_background, red, is_dark),
            Self::bright_variant(default_background, green, is_dark),
            Self::bright_variant(default_background, yellow, is_dark),
            Self::bright_variant(default_background, blue, is_dark),
            Self::bright_variant(default_background, magenta, is_dark),
            Self::bright_variant(default_background, cyan, is_dark),
            Self::ensure_readable(default_background, default_foreground),
        ];

        Self {
            default_foreground,
            default_background,
            colors,
        }
    }

    fn foreground_for_code(self, color: u32) -> Hsla {
        if color == DEFAULT_FOREGROUND {
            self.default_foreground
        } else if color == DEFAULT_BACKGROUND {
            self.default_background
        } else if let Some(index) = ansi_color_index(color) {
            self.colors[index]
        } else if color <= RGB_COLOR_MAX {
            rgb(color).into()
        } else {
            self.default_foreground
        }
    }

    fn background_for_code(self, color: u32) -> Option<Hsla> {
        if color == DEFAULT_BACKGROUND {
            None
        } else if color == DEFAULT_FOREGROUND {
            Some(self.default_foreground)
        } else if let Some(index) = ansi_color_index(color) {
            Some(self.colors[index])
        } else if color <= RGB_COLOR_MAX {
            Some(rgb(color).into())
        } else {
            None
        }
    }

    fn ensure_readable(background: Hsla, color: Hsla) -> Hsla {
        ColorTheory::ensure_contrast(background, Self::opaque(color), ContrastRatios::AA_NORMAL)
    }

    fn bright_variant(background: Hsla, color: Hsla, is_dark: bool) -> Hsla {
        let delta = if is_dark { 0.10 } else { -0.10 };
        Self::ensure_readable(
            background,
            ColorTheory::adjust_oklab_lightness(color, delta),
        )
    }

    fn opaque(color: Hsla) -> Hsla {
        hsla(color.h, color.s, color.l, 1.0)
    }
}

#[cfg(all(test, feature = "emulator"))]
mod tests {
    use super::*;
    use gpui::{point, px};
    use nucleotide_terminal::frame::ansi_color;
    use nucleotide_ui::scrollbar::ScrollableHandle;

    fn scroll_model(history_size: usize, display_offset: usize) -> Arc<Mutex<TerminalViewModel>> {
        let mut model = TerminalViewModel::new(TerminalId(1));
        model.history_size = history_size;
        model.display_offset = display_offset;
        model.cell_height = 10.0;
        model.cell_width = 8.0;
        model.rows = 24;
        model.cols = 80;
        Arc::new(Mutex::new(model))
    }

    #[test]
    fn terminal_scrollbar_maps_live_bottom_to_track_end() {
        let model = scroll_model(100, 0);
        let handle = TerminalScrollHandle {
            model: model.clone(),
        };

        assert_eq!(handle.max_offset().height, px(1000.0));
        assert_eq!(handle.offset().y, px(-1000.0));

        handle.set_offset(point(px(0.0), px(0.0)));
        assert_eq!(model.lock().unwrap().display_offset, 100);

        handle.set_offset(point(px(0.0), px(-1000.0)));
        assert_eq!(model.lock().unwrap().display_offset, 0);
    }

    #[test]
    fn terminal_scrollbar_clamps_offsets_to_scrollback_bounds() {
        let model = scroll_model(20, 10);
        let handle = TerminalScrollHandle {
            model: model.clone(),
        };

        handle.set_offset(point(px(0.0), px(50.0)));
        assert_eq!(model.lock().unwrap().display_offset, 20);

        handle.set_offset(point(px(0.0), px(-250.0)));
        assert_eq!(model.lock().unwrap().display_offset, 0);
    }

    #[test]
    fn terminal_wheel_scroll_accumulates_fractional_lines() {
        let mut model = TerminalViewModel::new(TerminalId(1));
        model.history_size = 10;
        model.cell_height = 20.0;

        assert!(!model.scroll_wheel_by_pixel_delta(8.0));
        assert_eq!(model.display_offset, 0);

        assert!(!model.scroll_wheel_by_pixel_delta(8.0));
        assert_eq!(model.display_offset, 0);

        assert!(model.scroll_wheel_by_pixel_delta(8.0));
        assert_eq!(model.display_offset, 1);
        assert!((model.wheel_scroll_remainder - 0.2).abs() < 0.0001);
    }

    #[test]
    fn terminal_wheel_scroll_sends_positive_delta_for_scrollback() {
        let mut model = TerminalViewModel::new(TerminalId(1));
        let (tx, rx) = std::sync::mpsc::channel();
        model.set_control_sender(tx);
        model.history_size = 10;
        model.cell_height = 20.0;

        assert!(model.scroll_wheel_by_pixel_delta(20.0));
        assert_eq!(model.display_offset, 1);

        let Ok(nucleotide_terminal::session::ControlMsg::Scroll { delta }) = rx.try_recv() else {
            panic!("expected scroll control message");
        };
        assert_eq!(delta, 1);
    }

    #[test]
    fn terminal_wheel_scroll_discards_remainder_at_edges() {
        let mut model = TerminalViewModel::new(TerminalId(1));
        model.history_size = 10;
        model.cell_height = 20.0;

        assert!(!model.scroll_wheel_by_pixel_delta(-12.0));
        assert_eq!(model.display_offset, 0);
        assert_eq!(model.wheel_scroll_remainder, 0.0);

        assert!(!model.scroll_wheel_by_pixel_delta(12.0));
        assert_eq!(model.display_offset, 0);

        assert!(model.scroll_wheel_by_pixel_delta(12.0));
        assert_eq!(model.display_offset, 1);
    }

    #[test]
    fn terminal_alternate_scroll_sends_arrow_input_instead_of_scrollback() {
        let mut model = TerminalViewModel::new(TerminalId(1));
        let (tx, rx) = std::sync::mpsc::channel();
        model.set_input_sender(tx);
        model.cell_height = 10.0;
        model.input_mode = TerminalInputMode {
            alternate_screen: true,
            alternate_scroll: true,
            ..TerminalInputMode::default()
        };

        assert!(model.scroll_wheel_by_pixel_delta(10.0));
        assert_eq!(model.display_offset, 0);
        assert_eq!(rx.try_recv().unwrap(), b"\x1b[A".to_vec());
    }

    #[test]
    fn terminal_clear_input_sender_closes_direct_input_channel() {
        let mut model = TerminalViewModel::new(TerminalId(1));
        let (tx, rx) = std::sync::mpsc::channel();

        model.set_input_sender(tx);
        model.clear_input_sender();

        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn terminal_diff_scroll_marks_visible_rows_dirty() {
        let mut model = TerminalViewModel::new(TerminalId(1));
        model.resize_grid(4, 3, Some((8.0, 16.0)));
        model.take_dirty_rows();

        model.apply_frame(FramePayload::Diff(GridDiff {
            lines: Vec::new(),
            scrolled: Some(1),
            cursor_row: 0,
            cursor_col: 0,
        }));

        assert_eq!(model.take_dirty_rows(), vec![0, 1, 2]);
    }

    #[test]
    fn terminal_diff_scroll_clamps_delta_to_visible_rows() {
        let mut model = TerminalViewModel::new(TerminalId(1));
        model.resize_grid(4, 3, Some((8.0, 16.0)));
        model.take_dirty_rows();

        model.apply_frame(FramePayload::Diff(GridDiff {
            lines: Vec::new(),
            scrolled: Some(8),
            cursor_row: 0,
            cursor_col: 0,
        }));

        assert_eq!(model.grid.len(), 3);
        assert_eq!(model.take_dirty_rows(), vec![0, 1, 2]);
    }

    #[test]
    fn derived_ansi_palette_meets_contrast_for_light_and_dark_themes() {
        for tokens in [DesignTokens::light(), DesignTokens::dark()] {
            let palette = TerminalAnsiPalette::from_tokens(&tokens);

            for (index, color) in palette.colors.iter().copied().enumerate() {
                let contrast = ColorTheory::contrast_ratio(palette.default_background, color);
                assert!(
                    contrast >= ContrastRatios::AA_NORMAL,
                    "ANSI colour {index} contrast {contrast:.2} is below AA"
                );
            }
        }
    }

    #[test]
    fn derived_ansi_palette_tracks_theme_semantic_hues() {
        let tokens = DesignTokens::dark();
        let palette = TerminalAnsiPalette::from_tokens(&tokens);

        assert!(hue_distance(palette.colors[1], tokens.editor.error) < 0.08);
        assert!(hue_distance(palette.colors[2], tokens.editor.success) < 0.08);
        assert!(hue_distance(palette.colors[3], tokens.editor.warning) < 0.08);
    }

    #[test]
    fn ansi_markers_map_to_palette_without_remapping_truecolor() {
        let palette = TerminalAnsiPalette::from_tokens(&DesignTokens::dark());
        let ansi_red = palette.foreground_for_code(ansi_color(1));
        let truecolor_red = palette.foreground_for_code(0xcc0000);
        let expected_truecolor_red: Hsla = rgb(0xcc0000).into();

        assert_eq!(ansi_red, palette.colors[1]);
        assert_eq!(truecolor_red, expected_truecolor_red);
        assert_ne!(ansi_red, truecolor_red);
        assert_eq!(
            palette.background_for_code(ansi_color(4)),
            Some(palette.colors[4])
        );
        assert_eq!(palette.background_for_code(DEFAULT_BACKGROUND), None);
    }

    fn hue_distance(a: Hsla, b: Hsla) -> f32 {
        let raw = (a.h - b.h).abs();
        raw.min(1.0 - raw)
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
        let ansi_palette = TerminalAnsiPalette::from_tokens(tokens);

        let (grid_row, cursor_row, cursor_col, cell_height) = {
            let guard = lock_or_recover(self.model.as_ref());
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
        let mut cur_inverse = false;
        let mut buf = String::new();

        let flush_run = |line_in: gpui::Div,
                         text: &mut String,
                         fg: u32,
                         bg: u32,
                         bold: bool,
                         italic: bool,
                         underline: bool,
                         inverse: bool| {
            if text.is_empty() {
                return line_in;
            }
            let (fg, bg) = if inverse { (bg, fg) } else { (fg, bg) };
            let mapped_bg = ansi_palette.background_for_code(bg);
            let contrast_bg = mapped_bg.unwrap_or(ansi_palette.default_background);
            let mapped_fg = ColorTheory::ensure_contrast(
                contrast_bg,
                ansi_palette.foreground_for_code(fg),
                ContrastRatios::AA_NORMAL,
            );

            let mut run = div().child(std::mem::take(text));
            run = run.text_color(mapped_fg);
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
            let (fg, bg, bold, italic, underline, inverse) = (
                cell.fg,
                cell.bg,
                cell.bold,
                cell.italic,
                cell.underline,
                cell.inverse,
            );
            if fg != cur_fg
                || bg != cur_bg
                || bold != cur_bold
                || italic != cur_italic
                || underline != cur_underline
                || inverse != cur_inverse
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
                    cur_inverse,
                );
                cur_fg = fg;
                cur_bg = bg;
                cur_bold = bold;
                cur_italic = italic;
                cur_underline = underline;
                cur_inverse = inverse;
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
                    cur_inverse,
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
            cur_inverse,
        );

        line
    }
}

// Global registry to share TerminalViewModel instances across UI and runtime
static TERMINAL_VIEW_REGISTRY: Lazy<Mutex<HashMap<TerminalId, Arc<Mutex<TerminalViewModel>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn register_view_model(id: TerminalId, model: Arc<Mutex<TerminalViewModel>>) {
    let mut map = lock_or_recover(&TERMINAL_VIEW_REGISTRY);
    map.insert(id, model);
}

pub fn get_view_model(id: TerminalId) -> Option<Arc<Mutex<TerminalViewModel>>> {
    let map = lock_or_recover(&TERMINAL_VIEW_REGISTRY);
    map.get(&id).cloned()
}

pub fn unregister_view_model(id: TerminalId) {
    let mut map = lock_or_recover(&TERMINAL_VIEW_REGISTRY);
    map.remove(&id);
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn unregister_view_model_removes_global_entry() {
        let id = TerminalId(u64::MAX);
        let model = Arc::new(Mutex::new(TerminalViewModel::new(id)));

        register_view_model(id, model);
        assert!(get_view_model(id).is_some());

        unregister_view_model(id);
        assert!(get_view_model(id).is_none());
    }

    #[test]
    fn terminal_view_model_records_spawn_failure() {
        let mut model = TerminalViewModel::new(TerminalId(9));

        model.set_spawn_failure(
            "Terminal session failed to start",
            vec!["Command: ssh devbox".to_string()],
        );

        let failure = model
            .spawn_failure()
            .expect("spawn failure should be recorded");
        assert_eq!(failure.message, "Terminal session failed to start");
        assert_eq!(failure.details, vec!["Command: ssh devbox"]);
        assert!(model.has_exited());
        assert!(model.has_spawn_failure());
    }

    #[test]
    fn terminal_view_model_display_title_uses_terminal_title_with_fallback() {
        let mut model = TerminalViewModel::new(TerminalId(10));

        assert_eq!(model.display_title(), "Terminal");

        model.set_window_title("  cargo test\u{7}  ");

        assert_eq!(model.window_title(), Some("cargo test"));
        assert_eq!(model.display_title(), "cargo test");

        model.set_window_title(" \n\t ");

        assert_eq!(model.window_title(), None);
        assert_eq!(model.display_title(), "Terminal");
    }
}
