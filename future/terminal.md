# Embedded Terminal – Implementation Guide (LLM-Friendly)

This document specifies a full, incremental design for integrating an embedded terminal into Nucleotide that is consistent with our workspace layout, event system, and design tokens. It is written so another LLM (or human) can implement it with minimal back‑and‑forth.

Before You Start
- Create a new feature branch for this work before implementation:
  - `git switch -c feat/terminal` (or `git checkout -b feat/terminal`)
- Keep commits atomic and use Conventional Commit prefixes per the repo guidelines.

Goals
- Provide a performant terminal panel (bottom/right dock) with multiple tabs.
- Respect Nucleotide’s tokenized theming and font settings.
- Integrate cleanly with the Event System and GPUI focus/keyboard paradigms.
- macOS/Linux first; Windows (ConPTY) behind a feature flag.

Non‑Goals (initially)
- Full session persistence/restore of PTY state.
- Terminal splits and vi‑mode (will arrive in Phase 3).

Crates To Add
- `crates/nucleotide-terminal` – core session and PTY management.
- `crates/nucleotide-terminal-view` – GPUI rendering of a terminal grid.
- `crates/nucleotide-terminal-panel` – panel, tabs, and actions wiring.

Add each crate to the workspace `Cargo.toml` and keep dependencies minimal. Use the same lint/style settings as sibling crates.

Third‑Party Dependencies
- `alacritty_terminal` – terminal emulation and ANSI/OSC handling.
- `portable-pty` – PRIMARY PTY abstraction for all platforms (Unix + Windows via ConPTY). Using it from day one avoids platform-specific cfg branches.
- `tokio` – async IO for PTY read/write and coalesced frame updates.
- `crossbeam-channel` or `tokio::sync` – channels for output frames.

Event System Integration
Define a new domain: `nucleotide-events/src/v2/terminal/`.

Rust types (documentation-first, not compiled here):
```rust
// crates/nucleotide-events/src/v2/terminal/mod.rs
#[derive(Clone, Debug)]
pub struct TerminalId(pub u64);

#[derive(Clone, Debug)]
pub struct SpawnTerminal {
    pub id: TerminalId,
    pub cwd: Option<std::path::PathBuf>,
    pub shell: Option<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct TerminalResized { pub id: TerminalId, pub cols: u16, pub rows: u16 }

#[derive(Clone, Debug)]
pub struct TerminalInput { pub id: TerminalId, pub bytes: Vec<u8> }

#[derive(Clone, Debug)]
pub struct TerminalOutput { pub id: TerminalId, pub bytes: Vec<u8> }

#[derive(Clone, Debug)]
pub struct TerminalExited { pub id: TerminalId, pub code: Option<i32>, pub signal: Option<i32> }

#[derive(Clone, Debug)]
pub struct TerminalFocusChanged { pub id: TerminalId, pub focused: bool }
```

Bus APIs (as with other domains):
```rust
impl EventBus {
    pub fn dispatch_terminal_spawn(&self, e: SpawnTerminal) { /* … */ }
    pub fn dispatch_terminal_resize(&self, e: TerminalResized) { /* … */ }
    pub fn dispatch_terminal_input(&self, e: TerminalInput) { /* … */ }
    pub fn dispatch_terminal_output(&self, e: TerminalOutput) { /* … */ }
    pub fn dispatch_terminal_exited(&self, e: TerminalExited) { /* … */ }
    pub fn dispatch_terminal_focus(&self, e: TerminalFocusChanged) { /* … */ }
}
```

Handlers
- Add `TerminalEventHandler` under `nucleotide/src/application/<domain>.rs` that owns sessions, spawns/kills PTYs, and forwards output to UI.
- Keep handlers fast and offload IO to `tokio` tasks; publish `TerminalOutput` via the bus.

Core Session (nucleotide-terminal)
API sketch:
```rust
// crates/nucleotide-terminal/src/session.rs
pub struct TerminalSessionCfg {
    pub cwd: Option<std::path::PathBuf>,
    pub shell: Option<String>,
    pub env: Vec<(String, String)>,
}

pub struct TerminalSession {
    id: u64,
    // PTY and child handle types depend on platform (feature‑gated)
    pty: PtyHandle,
    grid: Arc<parking_lot::RwLock<GridSnapshot>>, // lightweight view for UI
}

impl TerminalSession {
    pub async fn spawn(id: u64, cfg: TerminalSessionCfg) -> anyhow::Result<(Self, tokio::sync::mpsc::Receiver<Vec<u8>>)>;
    pub async fn write(&self, bytes: &[u8]) -> std::io::Result<()>;
    pub async fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()>;
    pub async fn kill(&self) -> anyhow::Result<()>;
}
```

Implementation notes
- Use `alacritty_terminal::{Term, ansi}` with `portable-pty` for PTY spawn/resize. Maintain a read loop that:
  - Reads from the PTY into a reusable buffer.
  - Feeds bytes to `Term::advance_bytes` and updates an internal grid model.
  - Coalesces output frames on a 8–16ms timer; send frames through a bounded channel.
  - Keep a ring buffer for scrollback (configurable; default 10–20K lines).
  - On shutdown, send SIGHUP (Unix), or CTRL_BREAK (Windows), wait, then kill.

GridSnapshot
- Store only what the GPUI renderer needs: rows of cells with char, fg, bg, style flags (bold/italic/underline/invert).
- Expose `fn snapshot(&self) -> Arc<GridSnapshot>` for the view to read on render without locking the emulator.

Diff Frames (optimize render loop)
- Leverage alacritty’s dirty/damage tracking to build smaller payloads per frame.
- Frame protocol:
  ```rust
  pub enum FramePayload {
      Full(GridSnapshot),              // initial, resize, theme change, or large damage
      Diff(GridDiff),                  // typical frame
  }
  pub struct GridDiff { pub lines: Vec<ChangedLine>, pub scrolled: Option<i32> }
  pub struct ChangedLine { pub row: u32, pub ranges: Vec<ChangedRange> }
  pub struct ChangedRange { pub col: u16, pub cells: Vec<Cell> }
  ```
- Heuristics: send `Full` when dirty coverage is high (e.g., >45%), on palette/resize; otherwise send `Diff`.
- Coalesce diffs on a short timer (8–16ms). The UI applies diffs to its local copy and only re-batches affected lines.

Terminal View (nucleotide-terminal-view)
Components
- `TerminalView`: GPUI component implementing `Render`, holds `TerminalId`, `FocusHandle`, scroll offset, and a `UniformList`/custom scroller.
- `TerminalElement`: low‑level renderer that batches adjacent cells into `TextRun`s for performance.

Rendering
- Palette mapping from design tokens:
  - Use `theme.tokens` to derive ANSI 0–15 colors. Suggested mapping: neutrals for 0..7, brighter variants for 8..15; map info/warning/error/success where appropriate.
  - Background: `tokens.editor.text/background` for contrast; selection and cursor from `tokens.editor.selection_*`/`cursor_*`.
- Font: use `FontSettings.fixed_font` for terminal glyphs; fallback chain respected.
- Cursor: draw a block/line per config; blink using GPUI timers.
- Scrollbar: reuse `scrollbar` component if possible; otherwise simple custom bar.

Input & Focus
- Integrate with `nucleotide-ui::global_input` for focus groups, navigation, and shortcuts.
- Two modes:
  - “Terminal Insert” (default): raw key passthrough → `TerminalInput` bytes.
  - “Editor” optional vi‑style navigation (Phase 3): when enabled, arrow keys/gg/G search operate on scrollback.
- Handle copy/paste and bracketed paste mode.

Panel & Tabs (nucleotide-terminal-panel)
- Dockable panel like other UI panels (bottom/right). Persist last height.
- Tabs with `TerminalId`, title (program name or cwd), status (running/exited).
- Key actions (define in `crates/nucleotide-ui/src/actions.rs` or new `terminal/actions.rs`):
  - `NewTerminal`, `CloseTerminal`, `NextTab`, `PrevTab`, `RenameTerminal`.
- Default bindings proposal: `cmd-j` toggle terminal panel, `cmd-shift-]`/`[` cycle tabs; configurable.

Configuration (nucleotide.toml)
```toml
[terminal]
shell = "/bin/zsh"          # default: user login shell
scrollback = 20000           # ring buffer lines (memory capped internally)
cursor = { style = "block", blink = true }
bracketed_paste = true

[terminal.env]
RUST_LOG = "info"

[terminal.font]
family = "<inherit fixed_font>"   # optional override
size = 12.0                       # optional override

[terminal.windows]
shell = "pwsh"                    # only on Windows, feature-gated initially
```

Workspace Integration
- `Workspace` owns a `HashMap<TerminalId, TerminalSessionHandle>`.
- When `SpawnTerminal` is dispatched, create session, mount a `TerminalView` inside `TerminalPanel` and subscribe to its output channel; translate frames to redraws.
- Resize: on panel/layout changes, compute cols/rows from font metrics and panel size; debounce and dispatch `TerminalResized`.

Helix Integration
- Add `:terminal` command in our app command palette that dispatches `SpawnTerminal`.
- Provide actions to “Run Current File” and “Send Selection” later (Phase 3): implement as Events that write bytes to the active session.

Security & Safety
- Never execute project‑local RC files implicitly. Respect user login shell behavior only.
- Sanitize link clicks (no command injection); open files via safe workspace APIs.
- Defensive parsing of OSC/CSI; ignore unsupported/private sequences that could perturb the UI.

Testing Plan
Unit
- ANSI/OSC parser → styles grid (snapshots for a small set of sequences).
- Palette mapping functions.
- Size calculation: cols/rows from font metrics.

Integration (gpui::TestAppContext)
- Spawn echo server PTY and validate throughput, resizing, and scrollback.
- Coalescing: emit many output bursts; ensure frames are coalesced within the time window.

Manual/Golden
- Record batched `TextRun` snapshots for a handful of terminal screens.
- Link clicking opens files at line/col.

Performance Notes
- Coalesce output frames (8–16ms) to reduce UI churn.
- Prefer Diff frames (dirty lines only) to avoid full-grid copies and reduce rendering work.
- Reuse buffers; avoid per‑cell allocations.
- Cache `TextRun` styles by (font, size, weight, fg, bg, flags); invalidate on theme change.

Phased Delivery
MVP (Phase 1–2)
1) Core session spawn/IO/resize + single tab.
2) TerminalView rendering (no search, no linkification).
3) Docked panel with toggle and resize; use UI chrome tokens.

Phase 3
4) Tabs + rename + close semantics.
5) Linkification + basic search overlay (regex on ring buffer).
6) Optional vi‑mode navigation for scrollback.
7) Windows (ConPTY) behind `cfg(windows)` feature.

Implementation Checklist (ordered)
1. Add `v2::terminal` events and bus methods with unit tests.
2. Scaffold crates (terminal, terminal-view, terminal-panel) with minimal types.
3. Implement `TerminalSession::spawn` using `alacritty_terminal` on Unix; start PTY read loop and frame channel.
4. Build `TerminalView` that reads `GridSnapshot` and renders with batched `TextRun`s; wire cursor and selection.
5. Add `TerminalPanel` and a temporary action to open one session; place it in the workspace dock.
6. Add resize wiring; debounce and dispatch `TerminalResized`.
7. Bind basic keys (toggle panel, new/close tab); integrate focus with `GlobalInputDispatcher`.
8. Map palette from `DesignTokens`; switch fonts depending on terminal vs UI contexts.
9. Document config keys; load from `~/.config/helix/nucleotide.toml`.
10. Add tests (parser, coalescing, sizing) and a short “Terminal Quickstart” in README.

Design Token Mapping (suggested defaults)
- ANSI 0 (black) → `tokens.chrome.border_muted`
- ANSI 1 (red) → `tokens.colors.error`
- ANSI 2 (green) → `tokens.colors.success`
- ANSI 3 (yellow) → `tokens.colors.warning`
- ANSI 4 (blue) → `tokens.colors.info`
- ANSI 5 (magenta) → `tokens.chrome.primary`
- ANSI 6 (cyan) → lighten(tokens.chrome.primary, 0.15)
- ANSI 7 (white) → `tokens.colors.text_primary`
- Bright variants (8–15) → above with increased lightness/saturation; ensure 4.5:1 contrast on the chosen background.

Keyboard/Event Mapping
- Raw → UTF‑8 bytes; respect modifiers; translate special keys to escape sequences (e.g., arrows, Home/End, PageUp/PageDown) using alacritty key table.
- Mouse: button press/release, wheel to scroll, drag selection (future).

Open Questions (to be decided during implementation)
- Minimum viable search feature (regex vs. literal only) in Phase 3.
- Session restore semantics (just re‑spawn shell in same cwd/env).

Appendix: Example Pseudocode – Spawn Flow
```rust
let (session, mut rx) = TerminalSession::spawn(id, cfg).await?;
workspace.register_terminal(id, session.clone());

// UI task: pump frames into a view-specific buffer and request redraws
cx.spawn(async move |cx| {
    while let Some(bytes) = rx.recv().await { cx.emit(TerminalOutput { id, bytes }); }
});
```

This plan fits Nucleotide’s Event System, GPUI views, and token system, and is deliberately specified so an LLM can scaffold code with clear module boundaries, signatures, and stepwise milestones.
