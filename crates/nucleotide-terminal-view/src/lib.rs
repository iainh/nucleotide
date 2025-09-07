// ABOUTME: Placeholder terminal view crate – to be implemented with GPUI

use nucleotide_events::v2::terminal::TerminalId;

#[cfg(feature = "emulator")]
use gpui::FontWeight;
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, rgb};
#[cfg(feature = "emulator")]
use nucleotide_terminal::frame::{Cell, FramePayload, GridDiff, GridSnapshot};
use nucleotide_ui::ThemedContext;
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
        let theme = _cx.theme();
        let tokens = &theme.tokens;
        let default_bg = tokens.colors.background;
        let default_fg = tokens.colors.text_primary;

        // Base container styling: editor-like background, foreground, and monospace font
        let mut container = div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(default_bg)
            .text_color(default_fg)
            .font_family("JetBrains Mono")
            .text_size(tokens.sizes.text_md);

        // Helper to convert emulator RGB u32 to gpui color
        #[inline]
        fn color_from_u32(c: u32) -> gpui::Hsla {
            rgb(c).into()
        }

        // Render
        {
            let guard = self.model.lock().unwrap();

            #[cfg(feature = "emulator")]
            {
                // Render each row by grouping runs of same style
                for row in &guard.grid {
                    let mut line = div().flex().flex_row();

                    // Accumulate runs
                    let mut cur_fg = 0xffffffff; // sentinel to force first run
                    let mut cur_bg = 0xffffffff;
                    let mut cur_bold = false;
                    let mut cur_italic = false;
                    let mut cur_underline = false;
                    let mut buf = String::new();

                    let mut flush_run = |mut line_in: gpui::Div,
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

                    for cell in row {
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

                    container = container.child(line);
                }
            }

            #[cfg(not(feature = "emulator"))]
            {
                // No emulator: show a placeholder with correct styling
                container = container.child(div().child("Terminal emulator disabled"));
            }
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
