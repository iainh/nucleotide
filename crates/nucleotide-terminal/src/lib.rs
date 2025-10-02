// ABOUTME: Core terminal session implementation using portable-pty
// ABOUTME: Incremental: raw PTY IO now; emulator/grid later

pub mod frame {
    #[derive(Debug, Clone)]
    pub enum FramePayload {
        Raw(Vec<u8>),
        #[cfg(feature = "emulator")]
        Full(GridSnapshot),
        #[cfg(feature = "emulator")]
        Diff(GridDiff),
    }

    #[cfg(feature = "emulator")]
    #[derive(Debug, Clone)]
    pub struct GridSnapshot {
        pub rows: Vec<Vec<Cell>>, // row-major
        pub cols: u16,
        pub rows_len: u16,
        pub cursor_row: u16,
        pub cursor_col: u16,
    }

    #[cfg(feature = "emulator")]
    #[derive(Debug, Clone)]
    pub struct GridDiff {
        pub lines: Vec<ChangedLine>,
        pub scrolled: Option<i32>,
        pub cursor_row: u16,
        pub cursor_col: u16,
    }

    #[cfg(feature = "emulator")]
    #[derive(Debug, Clone)]
    pub struct ChangedLine {
        pub row: u32,
        pub ranges: Vec<ChangedRange>,
    }

    #[cfg(feature = "emulator")]
    #[derive(Debug, Clone)]
    pub struct ChangedRange {
        pub col: u16,
        pub cells: Vec<Cell>,
    }

    #[cfg(feature = "emulator")]
    #[derive(Debug, Clone, Copy)]
    pub struct Cell {
        pub ch: char,
        pub fg: u32,
        pub bg: u32,
        pub bold: bool,
        pub italic: bool,
        pub underline: bool,
        pub inverse: bool,
    }
}

pub mod session {
    use anyhow::{Context, Result};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{Read, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc::{self, Receiver};

    use crate::frame::FramePayload;

    /// Control messages for the emulator engine (only when emulator feature is enabled)
    #[cfg(feature = "emulator")]
    pub enum ControlMsg {
        Resize {
            cols: u16,
            rows: u16,
            cell_width: f32,
            cell_height: f32,
        },
    }

    #[derive(Debug, Clone, Default)]
    pub struct TerminalSessionCfg {
        pub cwd: Option<PathBuf>,
        pub shell: Option<String>,
        pub env: Vec<(String, String)>,
        pub cols: Option<u16>,
        pub rows: Option<u16>,
    }

    pub struct TerminalSession {
        id: u64,
        master: Box<dyn portable_pty::MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        #[cfg(feature = "emulator")]
        control_tx: std::sync::mpsc::Sender<ControlMsg>,
    }

    impl std::fmt::Debug for TerminalSession {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TerminalSession")
                .field("id", &self.id)
                .finish()
        }
    }

    impl TerminalSession {
        pub async fn spawn(
            id: u64,
            cfg: TerminalSessionCfg,
        ) -> Result<(Self, Receiver<FramePayload>)> {
            let pty_system = native_pty_system();
            let size = PtySize {
                rows: cfg.rows.unwrap_or(24),
                cols: cfg.cols.unwrap_or(80),
                pixel_width: 0,
                pixel_height: 0,
            };
            let pair = pty_system.openpty(size).context("open PTY")?;

            let shell = default_shell(cfg.shell.as_deref());
            let mut cmd = CommandBuilder::new(&shell);

            if let Some(cwd) = &cfg.cwd {
                cmd.cwd(cwd);
            }
            for (k, v) in &cfg.env {
                cmd.env(k, v);
            }

            let child = pair
                .slave
                .spawn_command(cmd)
                .with_context(|| format!("spawn shell: {}", shell))?;

            // IO endpoints
            let mut reader = pair.master.try_clone_reader().context("clone PTY reader")?;
            let writer = pair.master.take_writer().context("take PTY writer")?;

            let writer = Arc::new(Mutex::new(writer));

            // Create output channel and blocking read loop
            let (tx, rx) = mpsc::channel::<FramePayload>(1024);

            // Control channel for emulator (resize with metrics)
            #[cfg(feature = "emulator")]
            let (control_tx, control_rx) = std::sync::mpsc::channel::<ControlMsg>();

            #[cfg(feature = "emulator")]
            {
                use crate::engine::AlacrittyEngine as Engine;
                use std::time::{Duration, Instant};
                tokio::task::spawn_blocking(move || {
                    let mut engine = Engine::new(cfg.cols.unwrap_or(80), cfg.rows.unwrap_or(24));
                    let mut buf = vec![0u8; 8192];
                    let mut last_emit = Instant::now();
                    let window = Duration::from_millis(16); // ~60 FPS cap
                    loop {
                        // Handle any pending control messages
                        while let Ok(msg) = control_rx.try_recv() {
                            match msg {
                                ControlMsg::Resize {
                                    cols,
                                    rows,
                                    cell_width: cw,
                                    cell_height: ch,
                                } => {
                                    engine.resize_with_metrics(cols, rows, cw, ch);
                                }
                            }
                        }
                        match reader.read(&mut buf) {
                            Ok(0) => {
                                if let Some(frame) = engine.take_frame() {
                                    let _ = tx.try_send(frame);
                                }
                                break;
                            }
                            Ok(n) => {
                                engine.feed_bytes(&buf[..n]);
                                if last_emit.elapsed() >= window {
                                    if engine
                                        .take_frame()
                                        .is_some_and(|frame| tx.try_send(frame).is_err())
                                    {
                                        break;
                                    }
                                    last_emit = Instant::now();
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });
            }

            #[cfg(not(feature = "emulator"))]
            {
                tokio::task::spawn_blocking(move || {
                    let mut buf = vec![0u8; 8192];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) => break, // EOF
                            Ok(n) => {
                                if tx.try_send(FramePayload::Raw(buf[..n].to_vec())).is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });
            }

            let session = Self {
                id,
                master: pair.master,
                child,
                writer,
                #[cfg(feature = "emulator")]
                control_tx,
            };

            Ok((session, rx))
        }

        pub async fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
            let mut guard = self.writer.lock().unwrap();
            guard.write_all(bytes)?;
            guard.flush()
        }

        pub async fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
            let size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            self.master.resize(size).map_err(std::io::Error::other)
        }

        /// Get a clone of the control channel sender (emulator feature only)
        #[cfg(feature = "emulator")]
        pub fn control_sender(&self) -> std::sync::mpsc::Sender<ControlMsg> {
            self.control_tx.clone()
        }

        pub async fn kill(&mut self) -> Result<()> {
            // Attempt graceful termination, then force kill
            // Drop writer to send HUP on Unix; then kill if still alive
            drop(self.writer.lock().unwrap());
            // portable-pty's Child provides kill()
            self.child.kill().ok();
            Ok(())
        }

        pub fn id(&self) -> u64 {
            self.id
        }
    }

    fn default_shell(override_shell: Option<&str>) -> String {
        if let Some(s) = override_shell {
            return s.to_string();
        }
        #[cfg(windows)]
        {
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
        }
        #[cfg(not(windows))]
        {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        }
    }
}

// Alacritty-based terminal engine scaffold (emulator feature)
#[cfg(feature = "emulator")]
pub mod engine {
    use crate::frame::{Cell, FramePayload, GridSnapshot};
    use alacritty_terminal::event::{Event as TermEvent, EventListener};
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::{self, RenderableContent, Term};
    use alacritty_terminal::vte::ansi::{
        self, Color as VteColor, NamedColor as VteNamedColor, Rgb as VteRgb,
    };

    /// Placeholder engine producing full snapshots; next iteration will integrate alacritty_terminal
    pub struct AlacrittyEngine {
        cols: u16,
        rows: u16,
        cell_width: f32,
        cell_height: f32,
        grid: Vec<Vec<Cell>>,
        term: Option<Term<NoopListener>>,
        parser: ansi::Processor,
    }

    impl AlacrittyEngine {
        pub fn new(cols: u16, rows: u16) -> Self {
            let cell_width = 8.0f32;
            let cell_height = 16.0f32;
            let mut grid = Vec::with_capacity(rows as usize);
            for _ in 0..rows {
                grid.push(vec![
                    Cell {
                        ch: ' ',
                        fg: 0xffffff,
                        bg: 0x000000,
                        bold: false,
                        italic: false,
                        underline: false,
                        inverse: false
                    };
                    cols as usize
                ]);
            }
            let mut engine = Self {
                cols,
                rows,
                cell_width,
                cell_height,
                grid,
                term: None,
                parser: ansi::Processor::new(),
            };
            engine.rebuild_term();
            engine
        }

        pub fn feed_bytes(&mut self, bytes: &[u8]) {
            if self.term.is_none() {
                self.rebuild_term();
            }
            if let Some(term) = &mut self.term {
                for &b in bytes {
                    self.parser.advance(term, b);
                }
            }
        }

        pub fn take_frame(&mut self) -> Option<FramePayload> {
            if self.term.is_none() {
                self.rebuild_term();
            }
            if let Some(term) = &mut self.term {
                // Renderable content for current viewport
                let content: RenderableContent<'_> = term.renderable_content();
                // Prepare grid buffer
                if self.grid.len() as u16 != self.rows
                    || self.grid.first().map(|r| r.len()).unwrap_or(0) as u16 != self.cols
                {
                    self.grid = vec![
                        vec![
                            Cell {
                                ch: ' ',
                                fg: 0xffffff,
                                bg: 0x000000,
                                bold: false,
                                italic: false,
                                underline: false,
                                inverse: false
                            };
                            self.cols as usize
                        ];
                        self.rows as usize
                    ];
                }
                for indexed in content.display_iter {
                    let pos = indexed.point;
                    let cell = indexed.cell;
                    let row = pos.line.0 as usize;
                    let col = pos.column.0;
                    if row < self.grid.len() && col < self.grid[row].len() {
                        let ch = cell.c;
                        let fg = Self::color_to_rgb_u32(cell.fg);
                        let bg = Self::color_to_rgb_u32(cell.bg);
                        self.grid[row][col] = Cell {
                            ch,
                            fg,
                            bg,
                            bold: cell.flags.contains(term::cell::Flags::BOLD),
                            italic: cell.flags.contains(term::cell::Flags::ITALIC),
                            underline: cell.flags.contains(term::cell::Flags::UNDERLINE),
                            inverse: cell.flags.contains(term::cell::Flags::INVERSE),
                        };
                    }
                }

                let cursor = content.cursor.point;
                return Some(FramePayload::Full(GridSnapshot {
                    rows: self.grid.clone(),
                    cols: self.cols,
                    rows_len: self.rows,
                    cursor_row: cursor.line.0.max(0) as u16,
                    cursor_col: (cursor.column.0 as u16),
                }));
            }
            None
        }

        pub fn resize(&mut self, cols: u16, rows: u16) {
            if cols != self.cols || rows != self.rows {
                self.cols = cols;
                self.rows = rows;
                self.grid.clear();
                for _ in 0..rows {
                    self.grid.push(vec![
                        Cell {
                            ch: ' ',
                            fg: 0xffffff,
                            bg: 0x000000,
                            bold: false,
                            italic: false,
                            underline: false,
                            inverse: false
                        };
                        cols as usize
                    ]);
                }
            }
        }

        pub fn resize_with_metrics(
            &mut self,
            cols: u16,
            rows: u16,
            cell_width: f32,
            cell_height: f32,
        ) {
            self.cell_width = cell_width;
            self.cell_height = cell_height;
            self.resize(cols, rows);
            self.rebuild_term();
        }

        fn rebuild_term(&mut self) {
            let config = term::Config::default();
            let listener = NoopListener;
            let dims = SimpleSize {
                cols: self.cols as usize,
                rows: self.rows as usize,
            };
            let term = Term::new(config, &dims, listener);
            self.term = Some(term);
        }

        #[inline]
        fn ansi_named_color_to_rgb(nc: VteNamedColor) -> VteRgb {
            // Basic 16-color palette mapping using common defaults
            match nc {
                VteNamedColor::Background => VteRgb {
                    r: 0x00,
                    g: 0x00,
                    b: 0x00,
                },
                VteNamedColor::Foreground => VteRgb {
                    r: 0xff,
                    g: 0xff,
                    b: 0xff,
                },
                VteNamedColor::Black => VteRgb {
                    r: 0x00,
                    g: 0x00,
                    b: 0x00,
                },
                VteNamedColor::Red => VteRgb {
                    r: 0xcc,
                    g: 0x00,
                    b: 0x00,
                },
                VteNamedColor::Green => VteRgb {
                    r: 0x00,
                    g: 0xa6,
                    b: 0x00,
                },
                VteNamedColor::Yellow => VteRgb {
                    r: 0x99,
                    g: 0x99,
                    b: 0x00,
                },
                VteNamedColor::Blue => VteRgb {
                    r: 0x00,
                    g: 0x00,
                    b: 0xcc,
                },
                VteNamedColor::Magenta => VteRgb {
                    r: 0xcc,
                    g: 0x00,
                    b: 0xcc,
                },
                VteNamedColor::Cyan => VteRgb {
                    r: 0x00,
                    g: 0xa6,
                    b: 0xb2,
                },
                VteNamedColor::White => VteRgb {
                    r: 0xcc,
                    g: 0xcc,
                    b: 0xcc,
                },
                VteNamedColor::BrightBlack => VteRgb {
                    r: 0x4d,
                    g: 0x4d,
                    b: 0x4d,
                },
                VteNamedColor::BrightRed => VteRgb {
                    r: 0xff,
                    g: 0x00,
                    b: 0x00,
                },
                VteNamedColor::BrightGreen => VteRgb {
                    r: 0x00,
                    g: 0xff,
                    b: 0x00,
                },
                VteNamedColor::BrightYellow => VteRgb {
                    r: 0xff,
                    g: 0xff,
                    b: 0x00,
                },
                VteNamedColor::BrightBlue => VteRgb {
                    r: 0x00,
                    g: 0x00,
                    b: 0xff,
                },
                VteNamedColor::BrightMagenta => VteRgb {
                    r: 0xff,
                    g: 0x00,
                    b: 0xff,
                },
                VteNamedColor::BrightCyan => VteRgb {
                    r: 0x00,
                    g: 0xff,
                    b: 0xff,
                },
                VteNamedColor::BrightWhite => VteRgb {
                    r: 0xff,
                    g: 0xff,
                    b: 0xff,
                },
                _ => VteRgb {
                    r: 0xff,
                    g: 0xff,
                    b: 0xff,
                },
            }
        }

        #[inline]
        fn xterm_256_to_rgb(idx: u8) -> VteRgb {
            if idx < 16 {
                // Map to bright 8 defaults above
                return match idx {
                    0 => VteRgb {
                        r: 0x00,
                        g: 0x00,
                        b: 0x00,
                    },
                    1 => VteRgb {
                        r: 0xcc,
                        g: 0x00,
                        b: 0x00,
                    },
                    2 => VteRgb {
                        r: 0x00,
                        g: 0xa6,
                        b: 0x00,
                    },
                    3 => VteRgb {
                        r: 0x99,
                        g: 0x99,
                        b: 0x00,
                    },
                    4 => VteRgb {
                        r: 0x00,
                        g: 0x00,
                        b: 0xcc,
                    },
                    5 => VteRgb {
                        r: 0xcc,
                        g: 0x00,
                        b: 0xcc,
                    },
                    6 => VteRgb {
                        r: 0x00,
                        g: 0xa6,
                        b: 0xb2,
                    },
                    7 => VteRgb {
                        r: 0xcc,
                        g: 0xcc,
                        b: 0xcc,
                    },
                    8 => VteRgb {
                        r: 0x4d,
                        g: 0x4d,
                        b: 0x4d,
                    },
                    9 => VteRgb {
                        r: 0xff,
                        g: 0x00,
                        b: 0x00,
                    },
                    10 => VteRgb {
                        r: 0x00,
                        g: 0xff,
                        b: 0x00,
                    },
                    11 => VteRgb {
                        r: 0xff,
                        g: 0xff,
                        b: 0x00,
                    },
                    12 => VteRgb {
                        r: 0x00,
                        g: 0x00,
                        b: 0xff,
                    },
                    13 => VteRgb {
                        r: 0xff,
                        g: 0x00,
                        b: 0xff,
                    },
                    14 => VteRgb {
                        r: 0x00,
                        g: 0xff,
                        b: 0xff,
                    },
                    _ => VteRgb {
                        r: 0xff,
                        g: 0xff,
                        b: 0xff,
                    },
                };
            }
            if (16..=231).contains(&idx) {
                let i = (idx - 16) as u32;
                let r = (i / 36) % 6;
                let g = (i / 6) % 6;
                let b = i % 6;
                let comp = |v: u32| if v == 0 { 0 } else { 55 + 40 * v } as u8;
                return VteRgb {
                    r: comp(r),
                    g: comp(g),
                    b: comp(b),
                };
            }
            let gray = 8 + 10 * (idx as u32 - 232);
            VteRgb {
                r: gray as u8,
                g: gray as u8,
                b: gray as u8,
            }
        }

        #[inline]
        fn color_to_rgb_u32(color: VteColor) -> u32 {
            let rgb = match color {
                VteColor::Spec(rgb) => rgb,
                VteColor::Named(nc) => Self::ansi_named_color_to_rgb(nc),
                VteColor::Indexed(idx) => Self::xterm_256_to_rgb(idx),
            };
            ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | (rgb.b as u32)
        }
    }

    #[derive(Clone, Copy)]
    struct SimpleSize {
        cols: usize,
        rows: usize,
    }

    impl Dimensions for SimpleSize {
        fn total_lines(&self) -> usize {
            self.rows
        }
        fn screen_lines(&self) -> usize {
            self.rows
        }
        fn columns(&self) -> usize {
            self.cols
        }
    }

    #[derive(Clone, Copy, Default)]
    pub struct NoopListener;

    impl EventListener for NoopListener {
        fn send_event(&self, _event: TermEvent) {}
    }
}

// Legacy VTE emulator (removed)
/*
mod emulator {
    use crate::frame::{Cell, ChangedLine, ChangedRange, FramePayload, GridDiff, GridSnapshot};
    use unicode_width::UnicodeWidthChar;
    use vte::{Params, Parser, Perform};

    type Grid = Vec<Vec<Cell>>;

    pub struct Emulator {
        cols: u16,
        rows: u16,
        cursor_col: u16,
        cursor_row: u16,
        last_cursor_col: u16,
        last_cursor_row: u16,
        scroll_top: u16,
        scroll_bottom: u16,
        // Current attributes
        cur_fg: u32,
        cur_bg: u32,
        cur_bold: bool,
        cur_italic: bool,
        cur_underline: bool,
        cur_inverse: bool,
        // Grid + diff cache
        grid: Grid,
        last_grid: Option<Grid>,
        threshold: f32,
        // vte parser + scroll tracking
        parser: Parser,
        scrolled_delta: i32,
    }

    impl Emulator {
        pub fn new(cols: u16, rows: u16) -> Self {
            let mut grid: Grid = Vec::with_capacity(rows as usize);
            for _ in 0..rows {
                grid.push(vec![blank_cell(); cols as usize]);
            }
            Self {
                cols,
                rows,
                cursor_col: 0,
                cursor_row: 0,
                last_cursor_col: 0,
                last_cursor_row: 0,
                scroll_top: 0,
                scroll_bottom: rows.saturating_sub(1),
                cur_fg: 0xffffff,
                cur_bg: 0x000000,
                cur_bold: false,
                cur_italic: false,
                cur_underline: false,
                cur_inverse: false,
                grid,
                last_grid: None,
                threshold: 0.45,
                parser: Parser::new(),
                scrolled_delta: 0,
            }
        }

        pub fn feed_bytes(&mut self, bytes: &[u8]) {
            // Temporarily take the parser to appease the borrow checker
            let mut parser = std::mem::take(&mut self.parser);
            for &b in bytes {
                parser.advance(self, b);
            }
            self.parser = parser;
        }

        pub fn take_frame(&mut self) -> Option<FramePayload> {
            let rows = self.rows;
            let cols = self.cols;
            let current = self.grid.clone();
            match self.last_grid.take() {
                None => {
                    self.last_grid = Some(current.clone());
                    self.last_cursor_row = self.cursor_row;
                    self.last_cursor_col = self.cursor_col;
                    Some(FramePayload::Full(GridSnapshot {
                        rows: current,
                        cols,
                        rows_len: rows,
                        cursor_row: self.cursor_row,
                        cursor_col: self.cursor_col,
                    }))
                }
                Some(prev) => {
                    let (mut diff, changed) = build_diff(&prev, &current);
                    if self.scrolled_delta != 0 {
                        diff.scrolled = Some(self.scrolled_delta);
                        self.scrolled_delta = 0;
                    }
                    diff.cursor_row = self.cursor_row;
                    diff.cursor_col = self.cursor_col;
                    let total = (rows as usize) * (cols as usize);
                    let coverage = if total == 0 {
                        0.0
                    } else {
                        (changed as f32) / (total as f32)
                    };
                    let cursor_changed = self.last_cursor_row != self.cursor_row
                        || self.last_cursor_col != self.cursor_col;
                    self.last_grid = Some(current.clone());
                    self.last_cursor_row = self.cursor_row;
                    self.last_cursor_col = self.cursor_col;
                    if coverage > self.threshold {
                        Some(FramePayload::Full(GridSnapshot {
                            rows: current,
                            cols,
                            rows_len: rows,
                            cursor_row: self.cursor_row,
                            cursor_col: self.cursor_col,
                        }))
                    } else if changed > 0 || cursor_changed {
                        Some(FramePayload::Diff(diff))
                    } else {
                        None
                    }
                }
            }
        }
    }

    impl Emulator {
        fn apply_sgr(&mut self, code: u16) {
            match code {
                0 => {
                    self.cur_fg = 0xffffff;
                    self.cur_bg = 0x000000;
                    self.cur_bold = false;
                    self.cur_italic = false;
                    self.cur_underline = false;
                    self.cur_inverse = false;
                }
                1 => self.cur_bold = true,
                3 => self.cur_italic = true,
                4 => self.cur_underline = true,
                7 => self.cur_inverse = true,
                21 | 22 => self.cur_bold = false,
                23 => self.cur_italic = false,
                24 => self.cur_underline = false,
                27 => self.cur_inverse = false,
                30..=37 => {
                    self.cur_fg = ansi_8_color(code - 30);
                }
                40..=47 => {
                    self.cur_bg = ansi_8_color(code - 40);
                }
                90..=97 => {
                    self.cur_fg = ansi_bright_8_color(code - 90);
                }
                100..=107 => {
                    self.cur_bg = ansi_bright_8_color(code - 100);
                }
                _ => {}
            }
        }
    }

    fn ansi_8_color(idx: u16) -> u32 {
        match idx {
            0 => 0x000000,
            1 => 0xcc0000,
            2 => 0x00a600,
            3 => 0x999900,
            4 => 0x0000cc,
            5 => 0xcc00cc,
            6 => 0x00a6b2,
            _ => 0xcccccc,
        }
    }
    fn ansi_bright_8_color(idx: u16) -> u32 {
        match idx {
            0 => 0x4d4d4d,
            1 => 0xff0000,
            2 => 0x00ff00,
            3 => 0xffff00,
            4 => 0x0000ff,
            5 => 0xff00ff,
            6 => 0x00ffff,
            _ => 0xffffff,
        }
    }
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

    fn build_diff(prev: &Grid, curr: &Grid) -> (GridDiff, usize) {
        let mut lines: Vec<ChangedLine> = Vec::new();
        let mut changed_cells = 0usize;
        let row_count = prev.len().min(curr.len());
        for row in 0..row_count {
            let (p, c) = (&prev[row], &curr[row]);
            let mut ranges: Vec<ChangedRange> = Vec::new();
            let mut col = 0usize;
            while col < p.len().min(c.len()) {
                if p[col].ch != c[col].ch
                    || p[col].fg != c[col].fg
                    || p[col].bg != c[col].bg
                    || p[col].bold != c[col].bold
                    || p[col].italic != c[col].italic
                    || p[col].underline != c[col].underline
                    || p[col].inverse != c[col].inverse
                {
                    let start = col as u16;
                    let mut cells: Vec<Cell> = Vec::new();
                    while col < p.len().min(c.len()) {
                        if p[col].ch != c[col].ch
                            || p[col].fg != c[col].fg
                            || p[col].bg != c[col].bg
                            || p[col].bold != c[col].bold
                            || p[col].italic != c[col].italic
                            || p[col].underline != c[col].underline
                            || p[col].inverse != c[col].inverse
                        {
                            cells.push(c[col]);
                            changed_cells += 1;
                            col += 1;
                        } else {
                            break;
                        }
                    }
                    ranges.push(ChangedRange { col: start, cells });
                } else {
                    col += 1;
                }
            }
            if !ranges.is_empty() {
                lines.push(ChangedLine {
                    row: row as u32,
                    ranges,
                });
            }
        }
        (
            GridDiff {
                lines,
                scrolled: None,
                cursor_row: 0,
                cursor_col: 0,
            },
            changed_cells,
        )
    }

    // vte-based performer implementation
    impl Emulator {
        fn clamp_cursor(&mut self) {
            if self.cursor_row >= self.rows {
                self.cursor_row = self.rows.saturating_sub(1);
            }
            if self.cursor_col >= self.cols {
                self.cursor_col = self.cols.saturating_sub(1);
            }
        }
        fn set_cell(&mut self, r: u16, c: u16, ch: char) {
            let r_us = r as usize;
            let c_us = c as usize;
            if r_us < self.grid.len() && c_us < self.grid[r_us].len() {
                let mut cell = self.grid[r_us][c_us];
                cell.ch = ch;
                cell.fg = self.cur_fg;
                cell.bg = self.cur_bg;
                cell.bold = self.cur_bold;
                cell.italic = self.cur_italic;
                cell.underline = self.cur_underline;
                cell.inverse = self.cur_inverse;
                if cell.inverse {
                    std::mem::swap(&mut cell.fg, &mut cell.bg);
                }
                self.grid[r_us][c_us] = cell;
            }
        }
        fn index(&mut self) {
            if self.cursor_row >= self.scroll_bottom {
                let top = self.scroll_top as usize;
                let bottom = self.scroll_bottom as usize;
                let region_len = bottom - top + 1;
                for i in 0..(region_len - 1) {
                    self.grid[top + i] = self.grid[top + i + 1].clone();
                }
                self.grid[bottom] = vec![blank_cell(); self.cols as usize];
                self.scrolled_delta += 1;
            } else {
                self.cursor_row = (self.cursor_row + 1).min(self.rows.saturating_sub(1));
            }
        }
        fn reverse_index(&mut self) {
            if self.cursor_row <= self.scroll_top {
                let top = self.scroll_top as usize;
                let bottom = self.scroll_bottom as usize;
                for i in (1..(bottom - top + 1)).rev() {
                    self.grid[top + i] = self.grid[top + i - 1].clone();
                }
                self.grid[top] = vec![blank_cell(); self.cols as usize];
                self.scrolled_delta -= 1;
            } else {
                self.cursor_row = self.cursor_row.saturating_sub(1);
            }
        }
        fn erase_in_display(&mut self, mode: u16) {
            match mode {
                0 => {
                    let r = self.cursor_row as usize;
                    let c = self.cursor_col as usize;
                    for col in c..self.cols as usize {
                        self.grid[r][col] = blank_cell();
                    }
                    for row in (r + 1)..self.rows as usize {
                        self.grid[row].fill(blank_cell());
                    }
                }
                1 => {
                    for row in 0..=self.cursor_row as usize {
                        if row < self.grid.len() {
                            let end = if row == self.cursor_row as usize {
                                self.cursor_col as usize
                            } else {
                                self.cols as usize
                            };
                            for col in 0..end {
                                self.grid[row][col] = blank_cell();
                            }
                        }
                    }
                }
                2 => {
                    for row in 0..self.rows as usize {
                        self.grid[row].fill(blank_cell());
                    }
                }
                _ => {}
            }
        }
        fn erase_in_line(&mut self, mode: u16) {
            let r = self.cursor_row as usize;
            match mode {
                0 => {
                    for col in self.cursor_col as usize..self.cols as usize {
                        self.grid[r][col] = blank_cell();
                    }
                }
                1 => {
                    for col in 0..=self.cursor_col as usize {
                        self.grid[r][col] = blank_cell();
                    }
                }
                2 => self.grid[r].fill(blank_cell()),
                _ => {}
            }
        }
        fn move_to(&mut self, row1: u16, col1: u16) {
            self.cursor_row = row1.saturating_sub(1).min(self.rows.saturating_sub(1));
            self.cursor_col = col1.saturating_sub(1).min(self.cols.saturating_sub(1));
            self.clamp_cursor();
        }
        fn apply_sgr_params(&mut self, params: &[u16]) {
            let mut it = params.iter().copied().peekable();
            while let Some(p) = it.next() {
                match p {
                    0
                    | 1
                    | 3
                    | 4
                    | 7
                    | 21
                    | 22
                    | 23
                    | 24
                    | 27
                    | 30..=37
                    | 40..=47
                    | 90..=97
                    | 100..=107 => {
                        self.apply_sgr(p);
                    }
                    38 | 48 => {
                        let is_fg = p == 38;
                        match it.peek().copied() {
                            Some(2) => {
                                it.next();
                                let r = it.next().unwrap_or(0);
                                let g = it.next().unwrap_or(0);
                                let b = it.next().unwrap_or(0);
                                let rgb = ((r as u32 & 0xff) << 16)
                                    | ((g as u32 & 0xff) << 8)
                                    | (b as u32 & 0xff);
                                if is_fg {
                                    self.cur_fg = rgb;
                                } else {
                                    self.cur_bg = rgb;
                                }
                            }
                            Some(5) => {
                                it.next();
                                let idx = it.next().unwrap_or(0);
                                // 256-color approximation
                                let c = if idx < 16 {
                                    ansi_bright_8_color(idx)
                                } else if (16..=231).contains(&idx) {
                                    let i = idx as u32 - 16;
                                    let r = (i / 36) % 6;
                                    let g = (i / 6) % 6;
                                    let b = i % 6;
                                    let comp = |v: u32| if v == 0 { 0 } else { 55 + 40 * v } as u32;
                                    (comp(r) << 16) | (comp(g) << 8) | comp(b)
                                } else {
                                    let gray = 8 + 10 * (idx as u32 - 232);
                                    (gray << 16) | (gray << 8) | gray
                                };
                                if is_fg {
                                    self.cur_fg = c;
                                } else {
                                    self.cur_bg = c;
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    impl Perform for Emulator {
        fn print(&mut self, c: char) {
            let w = UnicodeWidthChar::width(c).unwrap_or(1) as u16;
            self.set_cell(self.cursor_row, self.cursor_col, c);
            if w == 2 && self.cursor_col + 1 < self.cols {
                self.set_cell(self.cursor_row, self.cursor_col + 1, ' ');
                self.cursor_col = (self.cursor_col + 2).min(self.cols.saturating_sub(1));
            } else if self.cursor_col + 1 >= self.cols {
                self.cursor_col = 0;
                self.index();
            } else {
                self.cursor_col += 1;
            }
        }

        fn execute(&mut self, byte: u8) {
            match byte {
                b'\n' => {
                    self.cursor_col = 0;
                    self.index();
                }
                b'\r' => {
                    self.cursor_col = 0;
                }
                0x08 => {
                    if self.cursor_col > 0 {
                        self.cursor_col -= 1;
                        self.set_cell(self.cursor_row, self.cursor_col, ' ');
                    }
                }
                b'\t' => {
                    let next = ((self.cursor_col / 8) + 1) * 8;
                    self.cursor_col = next.min(self.cols.saturating_sub(1));
                }
                _ => {}
            }
        }

        fn csi_dispatch(
            &mut self,
            params: &Params,
            _intermediates: &[u8],
            _ignore: bool,
            action: char,
        ) {
            let num = |i: usize, default: u16| -> u16 {
                params
                    .iter()
                    .nth(i)
                    .and_then(|p| p.iter().next().copied())
                    .unwrap_or(default)
            };
            match action {
                'H' | 'f' => {
                    let row = num(0, 1);
                    let col = num(1, 1);
                    self.move_to(row, col);
                }
                'A' => {
                    let n = num(0, 1);
                    self.cursor_row = self.cursor_row.saturating_sub(n);
                }
                'B' => {
                    let n = num(0, 1);
                    self.cursor_row = (self.cursor_row + n).min(self.rows.saturating_sub(1));
                }
                'C' => {
                    let n = num(0, 1);
                    self.cursor_col = (self.cursor_col + n).min(self.cols.saturating_sub(1));
                }
                'D' => {
                    let n = num(0, 1);
                    self.cursor_col = self.cursor_col.saturating_sub(n);
                }
                'G' => {
                    let col = num(0, 1);
                    self.cursor_col = col.saturating_sub(1).min(self.cols.saturating_sub(1));
                }
                'J' => {
                    let m = num(0, 0);
                    self.erase_in_display(m);
                }
                'K' => {
                    let m = num(0, 0);
                    self.erase_in_line(m);
                }
                'm' => {
                    let flat: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
                    let vals = if flat.is_empty() { vec![0u16] } else { flat };
                    self.apply_sgr_params(&vals);
                }
                'r' => {
                    let top = num(0, 1).saturating_sub(1).min(self.rows.saturating_sub(1));
                    let bot = num(1, self.rows)
                        .saturating_sub(1)
                        .min(self.rows.saturating_sub(1));
                    self.scroll_top = top;
                    self.scroll_bottom = bot.max(top);
                    self.move_to(1, 1);
                }
                _ => {}
            }
        }

        fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
            match byte {
                b'M' => self.reverse_index(), // RI
                b'D' => self.index(),         // IND
                _ => {}
            }
        }
        fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
        fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
        fn put(&mut self, _byte: u8) {}
        fn unhook(&mut self) {}
    }
}
*/
pub mod bounds {
    use portable_pty::PtySize;

    /// Represents the visible terminal viewport in both cell and pixel metrics.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct TerminalBounds {
        cell_width: f32,
        cell_height: f32,
        pixel_width: f32,
        pixel_height: f32,
    }

    impl TerminalBounds {
        /// Create bounds from raw pixel dimensions, quantizing to full cells.
        pub fn from_pixels(cell_width: f32, cell_height: f32, width: f32, height: f32) -> Self {
            let cell_width = cell_width.max(1.0);
            let cell_height = cell_height.max(1.0);
            let cols = (width / cell_width).floor().max(1.0);
            let rows = (height / cell_height).floor().max(1.0);
            Self {
                cell_width,
                cell_height,
                pixel_width: cols * cell_width,
                pixel_height: rows * cell_height,
            }
        }

        /// Create bounds from explicit cell counts and cell metrics.
        pub fn from_cells(cell_width: f32, cell_height: f32, cols: u16, rows: u16) -> Self {
            let cell_width = cell_width.max(1.0);
            let cell_height = cell_height.max(1.0);
            let cols = cols.max(1) as f32;
            let rows = rows.max(1) as f32;
            Self {
                cell_width,
                cell_height,
                pixel_width: cols * cell_width,
                pixel_height: rows * cell_height,
            }
        }

        /// Return bounds with updated cell counts while preserving pixel metrics for each cell.
        pub fn with_cells(&self, cols: u16, rows: u16) -> Self {
            Self::from_cells(self.cell_width, self.cell_height, cols, rows)
        }

        #[inline]
        pub fn cols(&self) -> u16 {
            (self.pixel_width / self.cell_width).round().max(1.0) as u16
        }

        #[inline]
        pub fn rows(&self) -> u16 {
            (self.pixel_height / self.cell_height).round().max(1.0) as u16
        }

        #[inline]
        pub fn cell_size(&self) -> (f32, f32) {
            (self.cell_width, self.cell_height)
        }

        #[inline]
        pub fn pixel_size(&self) -> (f32, f32) {
            (self.pixel_width, self.pixel_height)
        }

        #[inline]
        pub fn approx_eq(&self, other: &Self) -> bool {
            (self.cols() == other.cols())
                && (self.rows() == other.rows())
                && (self.cell_width - other.cell_width).abs() < 0.1
                && (self.cell_height - other.cell_height).abs() < 0.1
        }

        #[inline]
        pub fn to_pty_size(&self) -> PtySize {
            let (px_w, px_h) = (self.pixel_width, self.pixel_height);
            PtySize {
                cols: self.cols(),
                rows: self.rows(),
                pixel_width: px_w.round().clamp(0.0, u16::MAX as f32) as u16,
                pixel_height: px_h.round().clamp(0.0, u16::MAX as f32) as u16,
            }
        }
    }
}

pub use bounds::TerminalBounds;
