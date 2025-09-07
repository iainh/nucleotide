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
    }

    #[cfg(feature = "emulator")]
    #[derive(Debug, Clone)]
    pub struct GridDiff {
        pub lines: Vec<ChangedLine>,
        pub scrolled: Option<i32>,
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

            #[cfg(feature = "emulator")]
            {
                use crate::emulator::Emulator;
                use std::time::{Duration, Instant};
                // Spawn a blocking loop that feeds the emulator and coalesces frames
                tokio::task::spawn_blocking(move || {
                    let mut emulator =
                        Emulator::new(cfg.cols.unwrap_or(80), cfg.rows.unwrap_or(24));
                    let mut buf = vec![0u8; 8192];
                    let mut last_emit = Instant::now();
                    let window = Duration::from_millis(12);
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) => {
                                // EOF: try to flush any pending changes
                                if let Some(frame) = emulator.take_frame() {
                                    let _ = tx.blocking_send(frame);
                                }
                                break;
                            }
                            Ok(n) => {
                                emulator.feed_bytes(&buf[..n]);
                                if last_emit.elapsed() >= window {
                                    if let Some(frame) = emulator.take_frame() {
                                        if tx.blocking_send(frame).is_err() {
                                            break;
                                        }
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
                                if tx
                                    .blocking_send(FramePayload::Raw(buf[..n].to_vec()))
                                    .is_err()
                                {
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
            self.master
                .resize(size)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
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

#[cfg(feature = "emulator")]
mod emulator {
    use crate::frame::{Cell, ChangedLine, ChangedRange, FramePayload, GridDiff, GridSnapshot};

    type Grid = Vec<Vec<Cell>>;

    pub struct Emulator {
        cols: u16,
        rows: u16,
        cursor_col: u16,
        cursor_row: u16,
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
        // CSI parsing state
        esc_active: bool,
        esc_buf: Vec<u8>,
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
                cur_fg: 0xffffff,
                cur_bg: 0x000000,
                cur_bold: false,
                cur_italic: false,
                cur_underline: false,
                cur_inverse: false,
                grid,
                last_grid: None,
                threshold: 0.45,
                esc_active: false,
                esc_buf: Vec::with_capacity(32),
            }
        }

        pub fn feed_bytes(&mut self, bytes: &[u8]) {
            for &b in bytes {
                // ESC handling
                if self.esc_active {
                    self.esc_buf.push(b);
                    if Self::is_csi_final(b) {
                        self.handle_csi();
                        self.esc_active = false;
                        self.esc_buf.clear();
                    }
                    continue;
                }
                if b == 0x1B {
                    // ESC
                    self.esc_active = true;
                    self.esc_buf.clear();
                    continue;
                }

                match b {
                    b'\n' => {
                        self.cursor_col = 0;
                        if self.cursor_row + 1 >= self.rows {
                            self.grid.remove(0);
                            self.grid.push(vec![blank_cell(); self.cols as usize]);
                        } else {
                            self.cursor_row += 1;
                        }
                    }
                    b'\r' => self.cursor_col = 0,
                    0x08 => {
                        if self.cursor_col > 0 {
                            self.cursor_col -= 1;
                            let (r, c) = (self.cursor_row as usize, self.cursor_col as usize);
                            if r < self.grid.len() && c < self.grid[r].len() {
                                self.grid[r][c] = blank_cell();
                            }
                        }
                    }
                    b if b.is_ascii_graphic() || b == b' ' => {
                        let (r, c) = (self.cursor_row as usize, self.cursor_col as usize);
                        if r < self.grid.len() && c < self.grid[r].len() {
                            let mut cell = blank_cell();
                            cell.ch = b as char;
                            cell.fg = self.cur_fg;
                            cell.bg = self.cur_bg;
                            cell.bold = self.cur_bold;
                            cell.italic = self.cur_italic;
                            cell.underline = self.cur_underline;
                            cell.inverse = self.cur_inverse;
                            if cell.inverse {
                                std::mem::swap(&mut cell.fg, &mut cell.bg);
                            }
                            self.grid[r][c] = cell;
                        }
                        if self.cursor_col + 1 >= self.cols {
                            self.cursor_col = 0;
                            if self.cursor_row + 1 >= self.rows {
                                self.grid.remove(0);
                                self.grid.push(vec![blank_cell(); self.cols as usize]);
                            } else {
                                self.cursor_row += 1;
                            }
                        } else {
                            self.cursor_col += 1;
                        }
                    }
                    _ => {}
                }
            }
        }

        pub fn take_frame(&mut self) -> Option<FramePayload> {
            let rows = self.rows;
            let cols = self.cols;
            let current = self.grid.clone();
            match self.last_grid.take() {
                None => {
                    self.last_grid = Some(current.clone());
                    Some(FramePayload::Full(GridSnapshot {
                        rows: current,
                        cols,
                        rows_len: rows,
                    }))
                }
                Some(prev) => {
                    let (diff, changed) = build_diff(&prev, &current);
                    let total = (rows as usize) * (cols as usize);
                    let coverage = if total == 0 {
                        0.0
                    } else {
                        (changed as f32) / (total as f32)
                    };
                    self.last_grid = Some(current.clone());
                    if coverage > self.threshold {
                        Some(FramePayload::Full(GridSnapshot {
                            rows: current,
                            cols,
                            rows_len: rows,
                        }))
                    } else if changed > 0 {
                        Some(FramePayload::Diff(diff))
                    } else {
                        None
                    }
                }
            }
        }
    }

    impl Emulator {
        fn is_csi_final(b: u8) -> bool {
            (0x40..=0x7E).contains(&b)
        }

        fn handle_csi(&mut self) {
            if self.esc_buf.first().copied() != Some(b'[') {
                return;
            }
            let (final_b, rest) = match self.esc_buf.split_last() {
                Some((f, r)) => (*f, r),
                None => return,
            };
            // strip '[' prefix
            let params = if rest.len() > 1 { &rest[1..] } else { &[][..] };
            let s = std::str::from_utf8(params).unwrap_or("");
            let parts: Vec<&str> = if s.is_empty() {
                Vec::new()
            } else {
                s.split(';').collect()
            };

            match final_b as char {
                'H' | 'f' => {
                    let row = parts
                        .get(0)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(1);
                    let col = parts
                        .get(1)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(1);
                    self.cursor_row = row.saturating_sub(1).min(self.rows.saturating_sub(1));
                    self.cursor_col = col.saturating_sub(1).min(self.cols.saturating_sub(1));
                }
                'A' => {
                    let n = parts
                        .get(0)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(1);
                    self.cursor_row = self.cursor_row.saturating_sub(n);
                }
                'B' => {
                    let n = parts
                        .get(0)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(1);
                    self.cursor_row = (self.cursor_row + n).min(self.rows.saturating_sub(1));
                }
                'C' => {
                    let n = parts
                        .get(0)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(1);
                    self.cursor_col = (self.cursor_col + n).min(self.cols.saturating_sub(1));
                }
                'D' => {
                    let n = parts
                        .get(0)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(1);
                    self.cursor_col = self.cursor_col.saturating_sub(n);
                }
                'G' => {
                    let col = parts
                        .get(0)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(1);
                    self.cursor_col = col.saturating_sub(1).min(self.cols.saturating_sub(1));
                }
                'K' => {
                    let mode = parts
                        .get(0)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(0);
                    let r = self.cursor_row as usize;
                    match mode {
                        0 => {
                            for c in self.cursor_col as usize..self.cols as usize {
                                self.grid[r][c] = blank_cell();
                            }
                        }
                        1 => {
                            for c in 0..=self.cursor_col as usize {
                                self.grid[r][c] = blank_cell();
                            }
                        }
                        2 => {
                            for c in 0..self.cols as usize {
                                self.grid[r][c] = blank_cell();
                            }
                        }
                        _ => {}
                    }
                }
                'J' => {
                    let mode = parts
                        .get(0)
                        .and_then(|x| x.parse::<u16>().ok())
                        .unwrap_or(0);
                    if mode == 2 {
                        for r in 0..self.rows as usize {
                            for c in 0..self.cols as usize {
                                self.grid[r][c] = blank_cell();
                            }
                        }
                        self.cursor_row = 0;
                        self.cursor_col = 0;
                    }
                }
                'm' => {
                    // Avoid holding an immutable borrow of `parts` while mutably borrowing `self`
                    let mut codes: Vec<u16> = Vec::new();
                    if parts.is_empty() {
                        codes.push(0);
                    } else {
                        for p in parts {
                            if let Ok(code) = p.parse::<u16>() {
                                codes.push(code);
                            }
                        }
                    }
                    for code in codes {
                        self.apply_sgr(code);
                    }
                }
                _ => {}
            }
        }

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
            },
            changed_cells,
        )
    }
}
