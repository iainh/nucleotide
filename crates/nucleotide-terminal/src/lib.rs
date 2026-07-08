// ABOUTME: Core terminal session implementation using portable-pty
// ABOUTME: Ghostty-backed terminal emulation with raw PTY fallback

pub mod frame {
    #[cfg(feature = "emulator")]
    const ANSI_COLOR_BASE: u32 = 0x0100_0000;
    #[cfg(feature = "emulator")]
    const ANSI_COLOR_MAX: u32 = ANSI_COLOR_BASE + 15;

    pub const DEFAULT_FOREGROUND: u32 = 0x01ff_fff0;
    pub const DEFAULT_BACKGROUND: u32 = 0x01ff_fff1;

    #[cfg(feature = "emulator")]
    pub fn ansi_color(index: u8) -> u32 {
        ANSI_COLOR_BASE | u32::from(index.min(15))
    }

    #[cfg(feature = "emulator")]
    pub fn ansi_color_index(color: u32) -> Option<usize> {
        if (ANSI_COLOR_BASE..=ANSI_COLOR_MAX).contains(&color) {
            Some((color - ANSI_COLOR_BASE) as usize)
        } else {
            None
        }
    }

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
        pub history_size: usize,
        pub display_offset: usize,
        pub input_mode: TerminalInputMode,
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct TerminalInputMode {
        pub application_cursor: bool,
        pub alternate_screen: bool,
        pub alternate_scroll: bool,
        pub mouse_mode: bool,
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
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
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
        Scroll {
            delta: i32,
        },
    }

    #[derive(Debug, Clone, Default)]
    pub struct TerminalSessionCfg {
        pub cwd: Option<PathBuf>,
        pub shell: Option<String>,
        pub program: Option<String>,
        pub args: Vec<String>,
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

            let terminal_env = terminal_env_with_defaults(&cfg.env, cfg.cwd.as_deref());
            let (mut cmd, command_label) = terminal_command_builder(&cfg, &terminal_env);

            if let Some(cwd) = &cfg.cwd {
                cmd.cwd(cwd);
            }

            let child = pair
                .slave
                .spawn_command(cmd)
                .with_context(|| format!("spawn terminal command: {}", command_label))?;

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
                use crate::engine::Engine;
                use std::time::{Duration, Instant};
                let engine_writer = writer.clone();

                // Spawn a reader thread so PTY reads don't block control message
                // processing (e.g. scroll commands while the terminal is idle).
                let (data_tx, data_rx) = std::sync::mpsc::channel::<Vec<u8>>();
                std::thread::spawn(move || {
                    let mut buf = vec![0u8; 8192];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if data_tx.send(buf[..n].to_vec()).is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });

                tokio::task::spawn_blocking(move || {
                    let mut engine = Engine::new(
                        cfg.cols.unwrap_or(80),
                        cfg.rows.unwrap_or(24),
                        Some(engine_writer),
                    );
                    let mut last_emit = Instant::now();
                    let window = Duration::from_millis(16); // ~60 FPS cap
                    let mut needs_frame = false;
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
                                    needs_frame = true;
                                }
                                ControlMsg::Scroll { delta } => {
                                    engine.scroll_display(delta);
                                    needs_frame = true;
                                }
                            }
                        }
                        // Try to receive data with a short timeout
                        match data_rx.recv_timeout(Duration::from_millis(8)) {
                            Ok(data) => {
                                engine.feed_bytes(&data);
                                while let Ok(more) = data_rx.try_recv() {
                                    engine.feed_bytes(&more);
                                }
                                needs_frame = true;
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                if let Some(frame) = engine.take_frame() {
                                    let _ = tx.try_send(frame);
                                }
                                break;
                            }
                        }
                        // Rate-limited frame emission
                        if needs_frame && last_emit.elapsed() >= window {
                            if engine
                                .take_frame()
                                .is_some_and(|frame| tx.try_send(frame).is_err())
                            {
                                break;
                            }
                            last_emit = Instant::now();
                            needs_frame = false;
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
            self.write_sync(bytes)
        }

        /// Synchronous write — preferred for the input hot-path since the
        /// underlying PTY writer is blocking anyway.
        pub fn write_sync(&self, bytes: &[u8]) -> std::io::Result<()> {
            let mut guard = self
                .writer
                .lock()
                .map_err(|_| std::io::Error::other("terminal writer lock poisoned"))?;
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
            match self.writer.lock() {
                Ok(writer) => drop(writer),
                Err(poisoned) => drop(poisoned.into_inner()),
            }
            // portable-pty's Child provides kill()
            self.child.kill().ok();
            Ok(())
        }

        pub fn id(&self) -> u64 {
            self.id
        }

        pub fn wait_exit_code(&mut self) -> Option<i32> {
            self.child
                .wait()
                .ok()
                .and_then(|status| i32::try_from(status.exit_code()).ok())
        }

        pub fn try_exit_code(&mut self) -> Option<i32> {
            self.child
                .try_wait()
                .ok()
                .flatten()
                .and_then(|status| i32::try_from(status.exit_code()).ok())
        }
    }

    fn terminal_command_builder(
        cfg: &TerminalSessionCfg,
        terminal_env: &[(String, String)],
    ) -> (CommandBuilder, String) {
        if let Some(program) = cfg.program.as_deref() {
            let mut cmd = CommandBuilder::new(program);
            cmd.args(&cfg.args);
            apply_terminal_env(&mut cmd, terminal_env, ShellEnvMode::Preserve);
            return (cmd, shell_command_label(program, &cfg.args));
        }

        if let Some(shell) = cfg.shell.as_deref() {
            let mut cmd = CommandBuilder::new(shell);
            cmd.args(&cfg.args);
            apply_terminal_env(&mut cmd, terminal_env, ShellEnvMode::ExplicitShell(shell));
            return (cmd, shell_command_label(shell, &cfg.args));
        }

        #[cfg(windows)]
        {
            let (shell, args) = windows_shell::default_shell_command();
            let mut cmd = CommandBuilder::new(&shell);
            cmd.args(&args);
            apply_terminal_env(&mut cmd, terminal_env, ShellEnvMode::ExplicitShell(&shell));
            (cmd, shell_command_label(&shell, &args))
        }
        #[cfg(not(windows))]
        {
            let mut cmd = CommandBuilder::new_default_prog();
            apply_terminal_env(&mut cmd, terminal_env, ShellEnvMode::LoginShell);
            (cmd, "login shell".to_string())
        }
    }

    enum ShellEnvMode<'a> {
        Preserve,
        ExplicitShell(&'a str),
        #[allow(dead_code)]
        LoginShell,
    }

    fn apply_terminal_env(
        cmd: &mut CommandBuilder,
        terminal_env: &[(String, String)],
        shell_mode: ShellEnvMode<'_>,
    ) {
        if matches!(shell_mode, ShellEnvMode::LoginShell) {
            cmd.env_remove("SHELL");
        }

        for (key, value) in terminal_env {
            if matches!(shell_mode, ShellEnvMode::LoginShell) && key.eq_ignore_ascii_case("SHELL") {
                continue;
            }

            cmd.env(key, value);
        }

        if let ShellEnvMode::ExplicitShell(shell) = shell_mode {
            cmd.env("SHELL", shell);
        }
    }

    fn shell_command_label(program: &str, args: &[String]) -> String {
        std::iter::once(program)
            .chain(args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn terminal_env_with_defaults(
        env: &[(String, String)],
        cwd: Option<&Path>,
    ) -> Vec<(String, String)> {
        let parent_env = std::env::vars().collect::<BTreeMap<_, _>>();
        terminal_env_with_defaults_from(env, cwd, &parent_env)
    }

    fn terminal_env_with_defaults_from(
        env: &[(String, String)],
        cwd: Option<&Path>,
        parent_env: &BTreeMap<String, String>,
    ) -> Vec<(String, String)> {
        let mut merged = parent_env.clone();
        merged.extend(env.iter().cloned());

        merged.insert("NUCLEOTIDE_TERM".to_string(), "true".to_string());
        merged.insert("TERM_PROGRAM".to_string(), "nucleotide".to_string());
        merged.insert("TERM".to_string(), "xterm-256color".to_string());
        merged.insert("COLORTERM".to_string(), "truecolor".to_string());
        merged.insert(
            "TERM_PROGRAM_VERSION".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        );

        restore_parent_env(&mut merged, parent_env, "HOME");
        restore_parent_env(&mut merged, parent_env, "USER");
        restore_parent_env(&mut merged, parent_env, "LOGNAME");
        restore_parent_env(&mut merged, parent_env, "SHELL");
        if let Some(cwd) = cwd {
            merged.insert("PWD".to_string(), cwd.to_string_lossy().into_owned());
        } else {
            restore_parent_env(&mut merged, parent_env, "PWD");
        }
        restore_parent_env(&mut merged, parent_env, "OLDPWD");
        remove_interactive_shell_state(&mut merged);

        merged.into_iter().collect()
    }

    const INTERACTIVE_SHELL_STATE_ENV_VARS: &[&str] = &[
        "BASH_ENV",
        "BASHOPTS",
        "ENV",
        "POSIXLY_CORRECT",
        "PROMPT_COMMAND",
        "PS1",
        "SHELLOPTS",
    ];

    fn remove_interactive_shell_state(merged: &mut BTreeMap<String, String>) {
        for key in INTERACTIVE_SHELL_STATE_ENV_VARS {
            merged.remove(*key);
        }
    }

    fn restore_parent_env(
        merged: &mut BTreeMap<String, String>,
        parent_env: &BTreeMap<String, String>,
        key: &str,
    ) {
        if let Some(value) = parent_env.get(key).filter(|value| !value.is_empty()) {
            merged.insert(key.to_string(), value.clone());
        } else {
            merged.remove(key);
        }
    }

    #[cfg(windows)]
    mod windows_shell {
        pub(super) fn default_shell_command() -> (String, Vec<String>) {
            super::windows_default_shell_command_from_comspec(
                std::env::var("COMSPEC").ok().as_deref(),
            )
        }
    }

    #[cfg(any(test, windows))]
    fn windows_default_shell_command_from_comspec(comspec: Option<&str>) -> (String, Vec<String>) {
        let shell = comspec
            .filter(|shell| !shell.trim().is_empty())
            .unwrap_or("cmd.exe")
            .to_string();
        let args = if is_cmd_shell(&shell) {
            vec!["/D".to_string(), "/K".to_string()]
        } else {
            Vec::new()
        };
        (shell, args)
    }

    #[cfg(any(test, windows))]
    fn is_cmd_shell(shell: &str) -> bool {
        let file_name = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
        file_name.eq_ignore_ascii_case("cmd") || file_name.eq_ignore_ascii_case("cmd.exe")
    }

    #[cfg(test)]
    mod windows_shell_tests {
        use super::windows_default_shell_command_from_comspec;

        #[test]
        fn windows_default_shell_falls_back_to_cmd() {
            assert_eq!(
                windows_default_shell_command_from_comspec(None),
                (
                    "cmd.exe".to_string(),
                    vec!["/D".to_string(), "/K".to_string()]
                )
            );
            assert_eq!(
                windows_default_shell_command_from_comspec(Some("")),
                (
                    "cmd.exe".to_string(),
                    vec!["/D".to_string(), "/K".to_string()]
                )
            );
            assert_eq!(
                windows_default_shell_command_from_comspec(Some("   ")),
                (
                    "cmd.exe".to_string(),
                    vec!["/D".to_string(), "/K".to_string()]
                )
            );
        }

        #[test]
        fn windows_default_shell_uses_comspec() {
            assert_eq!(
                windows_default_shell_command_from_comspec(Some("C:\\Windows\\System32\\cmd.exe")),
                (
                    "C:\\Windows\\System32\\cmd.exe".to_string(),
                    vec!["/D".to_string(), "/K".to_string()]
                )
            );
        }

        #[test]
        fn windows_default_shell_leaves_custom_shell_args_empty() {
            assert_eq!(
                windows_default_shell_command_from_comspec(Some("C:\\Tools\\pwsh.exe")),
                ("C:\\Tools\\pwsh.exe".to_string(), Vec::<String>::new())
            );
        }
    }

    #[cfg(test)]
    mod env_tests {
        use super::*;

        fn env_map(env: Vec<(String, String)>) -> BTreeMap<String, String> {
            env.into_iter().collect()
        }

        fn pair(key: &str, value: &str) -> (String, String) {
            (key.to_string(), value.to_string())
        }

        #[test]
        fn terminal_env_sets_zed_style_terminal_defaults() {
            let env = env_map(terminal_env_with_defaults_from(&[], None, &BTreeMap::new()));

            assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
            assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
            assert_eq!(
                env.get("TERM_PROGRAM").map(String::as_str),
                Some("nucleotide")
            );
            assert_eq!(env.get("NUCLEOTIDE_TERM").map(String::as_str), Some("true"));
            assert_eq!(
                env.get("TERM_PROGRAM_VERSION").map(String::as_str),
                Some(env!("CARGO_PKG_VERSION"))
            );
        }

        #[test]
        fn terminal_env_overrides_stale_terminal_values_like_zed() {
            let env = env_map(terminal_env_with_defaults_from(
                &[
                    pair("TERM", "dumb"),
                    pair("COLORTERM", "false"),
                    pair("TERM_PROGRAM", "other"),
                ],
                None,
                &BTreeMap::new(),
            ));

            assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
            assert_eq!(env.get("COLORTERM").map(String::as_str), Some("truecolor"));
            assert_eq!(
                env.get("TERM_PROGRAM").map(String::as_str),
                Some("nucleotide")
            );
        }

        #[test]
        fn terminal_env_preserves_unrelated_overrides() {
            let env = env_map(terminal_env_with_defaults_from(
                &[pair("PATH", "/custom/bin")],
                None,
                &BTreeMap::new(),
            ));

            assert_eq!(env.get("PATH").map(String::as_str), Some("/custom/bin"));
        }

        #[test]
        fn terminal_env_preserves_parent_process_environment() {
            let parent = BTreeMap::from([
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("SystemRoot".to_string(), "C:\\Windows".to_string()),
            ]);
            let env = env_map(terminal_env_with_defaults_from(&[], None, &parent));

            assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
            assert_eq!(
                env.get("SystemRoot").map(String::as_str),
                Some("C:\\Windows")
            );
        }

        #[test]
        fn terminal_env_project_values_override_parent_environment() {
            let parent = BTreeMap::from([("PATH".to_string(), "/usr/bin".to_string())]);
            let env = env_map(terminal_env_with_defaults_from(
                &[pair("PATH", "/project/bin")],
                None,
                &parent,
            ));

            assert_eq!(env.get("PATH").map(String::as_str), Some("/project/bin"));
        }

        #[test]
        fn terminal_env_restores_session_vars_from_parent_session() {
            let parent = BTreeMap::from([
                ("HOME".to_string(), "/Users/test".to_string()),
                ("USER".to_string(), "test".to_string()),
                ("SHELL".to_string(), "/bin/zsh".to_string()),
            ]);
            let env = env_map(terminal_env_with_defaults_from(
                &[
                    pair("HOME", "/nix/dev-home"),
                    pair("USER", "nix-user"),
                    pair("SHELL", "/nix/store/bash/bin/bash"),
                ],
                None,
                &parent,
            ));

            assert_eq!(env.get("HOME").map(String::as_str), Some("/Users/test"));
            assert_eq!(env.get("USER").map(String::as_str), Some("test"));
            assert_eq!(env.get("SHELL").map(String::as_str), Some("/bin/zsh"));
        }

        #[test]
        fn terminal_env_sets_pwd_to_spawn_cwd() {
            let parent = BTreeMap::from([("PWD".to_string(), "/Users/test".to_string())]);
            let env = env_map(terminal_env_with_defaults_from(
                &[pair("PWD", "/nix/dev-pwd")],
                Some(Path::new("/project")),
                &parent,
            ));

            assert_eq!(env.get("PWD").map(String::as_str), Some("/project"));
        }

        #[test]
        fn terminal_env_removes_session_vars_absent_from_parent_session() {
            let env = env_map(terminal_env_with_defaults_from(
                &[pair("HOME", "/nix/dev-home")],
                None,
                &BTreeMap::new(),
            ));

            assert!(!env.contains_key("HOME"));
        }

        #[test]
        fn terminal_env_removes_prompt_and_shell_startup_state() {
            let env = env_map(terminal_env_with_defaults_from(
                &[
                    pair("BASH_ENV", "/tmp/bash-env"),
                    pair("BASHOPTS", "cmdhist:progcomp"),
                    pair("ENV", "/tmp/sh-env"),
                    pair("POSIXLY_CORRECT", "1"),
                    pair("PROMPT_COMMAND", "echo prompt"),
                    pair("PS1", "\\[broken\\]$ "),
                    pair("SHELLOPTS", "posix"),
                ],
                None,
                &BTreeMap::new(),
            ));

            for key in INTERACTIVE_SHELL_STATE_ENV_VARS {
                assert!(
                    !env.contains_key(*key),
                    "{key} should not leak into terminal"
                );
            }
        }

        #[cfg(not(windows))]
        #[test]
        fn default_terminal_session_uses_login_shell_lookup() {
            let env = terminal_env_with_defaults_from(
                &[],
                None,
                &BTreeMap::from([("SHELL".to_string(), "/bin/bash".to_string())]),
            );
            let cfg = TerminalSessionCfg::default();

            let (cmd, label) = terminal_command_builder(&cfg, &env);

            assert!(
                cmd.get_argv().is_empty(),
                "default terminal should use portable-pty login shell lookup"
            );
            assert_eq!(label, "login shell");
            assert!(cmd.get_env("SHELL").is_none());
        }

        #[test]
        fn explicit_shell_session_sets_shell_environment_to_override() {
            let env = terminal_env_with_defaults_from(
                &[],
                None,
                &BTreeMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]),
            );
            let cfg = TerminalSessionCfg {
                shell: Some("/bin/bash".to_string()),
                args: vec!["-l".to_string()],
                ..TerminalSessionCfg::default()
            };

            let (cmd, label) = terminal_command_builder(&cfg, &env);

            assert_eq!(label, "/bin/bash -l");
            assert_eq!(
                cmd.get_argv().first().and_then(|arg| arg.to_str()),
                Some("/bin/bash")
            );
            assert_eq!(
                cmd.get_argv().get(1).and_then(|arg| arg.to_str()),
                Some("-l")
            );
            assert_eq!(
                cmd.get_env("SHELL").and_then(|value| value.to_str()),
                Some("/bin/bash")
            );
        }

        #[cfg(windows)]
        #[test]
        fn default_windows_terminal_launches_cmd_interactively() {
            let env = terminal_env_with_defaults_from(&[], None, &BTreeMap::new());
            let cfg = TerminalSessionCfg::default();

            let (cmd, label) = terminal_command_builder(&cfg, &env);

            assert!(label.contains("/D"));
            assert!(label.contains("/K"));
            assert_eq!(
                cmd.get_argv().get(1).and_then(|arg| arg.to_str()),
                Some("/D")
            );
            assert_eq!(
                cmd.get_argv().get(2).and_then(|arg| arg.to_str()),
                Some("/K")
            );
        }
    }
}

#[cfg(feature = "emulator")]
pub mod engine {
    use crate::frame::{
        Cell, DEFAULT_BACKGROUND, DEFAULT_FOREGROUND, FramePayload, GridSnapshot,
        TerminalInputMode, ansi_color,
    };
    use libghostty_vt::render::{CellIterator, RenderState, RowIterator};
    use libghostty_vt::style::{PaletteIndex, RgbColor, Style, StyleColor, Underline};
    use libghostty_vt::{Terminal, TerminalOptions};
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    const DEFAULT_CELL_WIDTH: f32 = 8.0;
    const DEFAULT_CELL_HEIGHT: f32 = 16.0;
    const DEFAULT_SCROLLBACK_LINES: usize = 10_000;

    pub struct Engine {
        cols: u16,
        rows: u16,
        cell_width: f32,
        cell_height: f32,
        grid: Vec<Vec<Cell>>,
        terminal: Option<Terminal<'static, 'static>>,
        render_state: Option<RenderState<'static>>,
        row_iter: Option<RowIterator<'static>>,
        cell_iter: Option<CellIterator<'static>>,
        pty_writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    }

    impl Engine {
        pub fn new(
            cols: u16,
            rows: u16,
            pty_writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
        ) -> Self {
            let cols = cols.max(1);
            let rows = rows.max(1);
            let mut engine = Self {
                cols,
                rows,
                cell_width: DEFAULT_CELL_WIDTH,
                cell_height: DEFAULT_CELL_HEIGHT,
                grid: blank_grid(cols, rows),
                terminal: None,
                render_state: None,
                row_iter: None,
                cell_iter: None,
                pty_writer,
            };
            engine.rebuild_terminal();
            engine
        }

        pub fn feed_bytes(&mut self, bytes: &[u8]) {
            if self.ensure_initialized()
                && let Some(terminal) = &mut self.terminal
            {
                terminal.vt_write(bytes);
            }
        }

        pub fn take_frame(&mut self) -> Option<FramePayload> {
            if !self.ensure_initialized() {
                return None;
            }

            let terminal = self.terminal.as_mut()?;
            let render_state = self.render_state.as_mut()?;
            let row_iter_handle = self.row_iter.as_mut()?;
            let cell_iter_handle = self.cell_iter.as_mut()?;

            let snapshot = render_state.update(terminal).ok()?;
            let cols = snapshot.cols().unwrap_or(self.cols).max(1);
            let rows_len = snapshot.rows().unwrap_or(self.rows).max(1);
            let blank = blank_cell();
            let mut grid = vec![vec![blank; cols as usize]; rows_len as usize];

            let mut row_index = 0usize;
            let mut row_iter = row_iter_handle.update(&snapshot).ok()?;
            while let Some(row) = row_iter.next() {
                if row_index >= grid.len() {
                    break;
                }

                let mut col_index = 0usize;
                let mut cell_iter = cell_iter_handle.update(row).ok()?;
                while let Some(cell) = cell_iter.next() {
                    if col_index >= grid[row_index].len() {
                        break;
                    }

                    let style = cell.style().unwrap_or_default();
                    grid[row_index][col_index] = Cell {
                        ch: first_grapheme_char(cell.graphemes().ok().as_deref()),
                        fg: foreground_cell_color(&style, cell.fg_color().ok().flatten()),
                        bg: background_cell_color(&style, cell.bg_color().ok().flatten()),
                        bold: style.bold,
                        italic: style.italic,
                        underline: style.underline != Underline::None,
                        inverse: style.inverse,
                    };
                    col_index += 1;
                }

                row_index += 1;
            }

            let cursor = snapshot.cursor_viewport().ok().flatten();
            let scrollbar = terminal.scrollbar().ok();
            let history_size = scrollbar
                .as_ref()
                .map(|scrollbar| scrollbar.total.saturating_sub(scrollbar.len) as usize)
                .unwrap_or_else(|| terminal.scrollback_rows().unwrap_or(0));
            let display_offset = scrollbar
                .as_ref()
                .map(|scrollbar| history_size.saturating_sub(scrollbar.offset as usize))
                .unwrap_or(0);

            self.cols = cols;
            self.rows = rows_len;
            self.grid = grid;

            Some(FramePayload::Full(GridSnapshot {
                rows: self.grid.clone(),
                cols,
                rows_len,
                cursor_row: cursor.map(|cursor| cursor.y).unwrap_or(0),
                cursor_col: cursor.map(|cursor| cursor.x).unwrap_or(0),
                history_size,
                display_offset,
                input_mode: TerminalInputMode {
                    application_cursor: terminal
                        .mode(libghostty_vt::terminal::Mode::DECCKM)
                        .unwrap_or(false),
                    alternate_screen: terminal
                        .mode(libghostty_vt::terminal::Mode::ALT_SCREEN)
                        .unwrap_or(false)
                        || terminal
                            .mode(libghostty_vt::terminal::Mode::ALT_SCREEN_LEGACY)
                            .unwrap_or(false)
                        || terminal
                            .mode(libghostty_vt::terminal::Mode::ALT_SCREEN_SAVE)
                            .unwrap_or(false),
                    alternate_scroll: terminal
                        .mode(libghostty_vt::terminal::Mode::ALT_SCROLL)
                        .unwrap_or(false),
                    mouse_mode: terminal.is_mouse_tracking().unwrap_or(false),
                },
            }))
        }

        pub fn resize_with_metrics(
            &mut self,
            cols: u16,
            rows: u16,
            cell_width: f32,
            cell_height: f32,
        ) {
            self.cols = cols.max(1);
            self.rows = rows.max(1);
            self.cell_width = cell_width.max(1.0);
            self.cell_height = cell_height.max(1.0);
            self.grid = blank_grid(self.cols, self.rows);

            if self.ensure_initialized()
                && let Some(terminal) = &mut self.terminal
            {
                let _ = terminal.resize(
                    self.cols,
                    self.rows,
                    self.cell_width.round().clamp(1.0, u32::MAX as f32) as u32,
                    self.cell_height.round().clamp(1.0, u32::MAX as f32) as u32,
                );
            }
        }

        pub fn scroll_display(&mut self, delta: i32) {
            if self.ensure_initialized()
                && let Some(terminal) = &mut self.terminal
            {
                // Nucleotide's view model uses positive deltas for larger
                // display offsets, i.e. scrolling up into history. Ghostty's
                // viewport delta uses the opposite sign.
                terminal.scroll_viewport(libghostty_vt::terminal::ScrollViewport::Delta(
                    -(delta as isize),
                ));
            }
        }

        fn ensure_initialized(&mut self) -> bool {
            if self.terminal.is_none()
                || self.render_state.is_none()
                || self.row_iter.is_none()
                || self.cell_iter.is_none()
            {
                self.rebuild_terminal();
            }

            self.terminal.is_some()
                && self.render_state.is_some()
                && self.row_iter.is_some()
                && self.cell_iter.is_some()
        }

        fn rebuild_terminal(&mut self) {
            let mut terminal = match Terminal::new(TerminalOptions {
                cols: self.cols,
                rows: self.rows,
                max_scrollback: DEFAULT_SCROLLBACK_LINES,
            }) {
                Ok(terminal) => terminal,
                Err(_) => {
                    self.terminal = None;
                    return;
                }
            };

            if let Some(writer) = self.pty_writer.clone() {
                let _ = terminal.on_pty_write(move |_terminal, data| {
                    if let Ok(mut writer) = writer.lock() {
                        let _ = writer.write_all(data);
                        let _ = writer.flush();
                    }
                });
            }

            let _ = terminal.resize(
                self.cols,
                self.rows,
                self.cell_width.round().clamp(1.0, u32::MAX as f32) as u32,
                self.cell_height.round().clamp(1.0, u32::MAX as f32) as u32,
            );

            self.terminal = Some(terminal);
            self.render_state = RenderState::new().ok();
            self.row_iter = RowIterator::new().ok();
            self.cell_iter = CellIterator::new().ok();
        }
    }

    fn blank_grid(cols: u16, rows: u16) -> Vec<Vec<Cell>> {
        vec![vec![blank_cell(); cols as usize]; rows as usize]
    }

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

    fn first_grapheme_char(graphemes: Option<&[char]>) -> char {
        graphemes
            .and_then(|graphemes| graphemes.first().copied())
            .unwrap_or(' ')
    }

    fn foreground_cell_color(style: &Style, resolved: Option<RgbColor>) -> u32 {
        style_color_to_cell_color(style.fg_color, resolved, DEFAULT_FOREGROUND)
    }

    fn background_cell_color(style: &Style, resolved: Option<RgbColor>) -> u32 {
        style_color_to_cell_color(style.bg_color, resolved, DEFAULT_BACKGROUND)
    }

    fn style_color_to_cell_color(
        style_color: StyleColor,
        resolved: Option<RgbColor>,
        default: u32,
    ) -> u32 {
        match style_color {
            StyleColor::None => resolved.map(rgb_to_cell_color).unwrap_or(default),
            StyleColor::Palette(PaletteIndex(index)) if index < 16 => ansi_color(index),
            StyleColor::Palette(_) => resolved.map(rgb_to_cell_color).unwrap_or(default),
            StyleColor::Rgb(rgb) => rgb_to_cell_color(rgb),
        }
    }

    fn rgb_to_cell_color(rgb: RgbColor) -> u32 {
        (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn truecolor_values_remain_literal_rgb() {
            assert_eq!(
                rgb_to_cell_color(RgbColor {
                    r: 0xcc,
                    g: 0x00,
                    b: 0x00,
                }),
                0xcc0000
            );
        }

        #[test]
        fn application_cursor_mode_is_reported_in_frames() {
            let mut engine = Engine::new(5, 2, None);

            engine.feed_bytes(b"\x1b[?1h");

            let Some(FramePayload::Full(snapshot)) = engine.take_frame() else {
                panic!("expected full snapshot");
            };
            assert!(snapshot.input_mode.application_cursor);
        }

        #[test]
        fn scrollback_display_offset_uses_bottom_zero_convention() {
            let mut engine = Engine::new(5, 2, None);

            engine.feed_bytes(b"one\r\ntwo\r\nthree\r\nfour");
            let Some(FramePayload::Full(bottom)) = engine.take_frame() else {
                panic!("expected full snapshot");
            };
            assert_eq!(bottom.display_offset, 0);

            engine.scroll_display(1);
            let Some(FramePayload::Full(scrolled)) = engine.take_frame() else {
                panic!("expected full snapshot");
            };
            assert!(scrolled.display_offset > 0);
        }

        #[test]
        fn scroll_display_negative_delta_returns_toward_bottom() {
            let mut engine = Engine::new(5, 2, None);

            engine.feed_bytes(b"one\r\ntwo\r\nthree\r\nfour");
            engine.scroll_display(1);
            let Some(FramePayload::Full(scrolled)) = engine.take_frame() else {
                panic!("expected full snapshot");
            };
            assert!(scrolled.display_offset > 0);

            engine.scroll_display(-1);
            let Some(FramePayload::Full(bottom)) = engine.take_frame() else {
                panic!("expected full snapshot");
            };
            assert!(bottom.display_offset < scrolled.display_offset);
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(unix, all(windows, feature = "emulator")))]
    use super::session::{TerminalSession, TerminalSessionCfg};
    #[cfg(all(windows, feature = "emulator"))]
    use crate::frame::FramePayload;
    #[cfg(any(unix, all(windows, feature = "emulator")))]
    use std::time::Duration;

    #[cfg(unix)]
    #[tokio::test]
    async fn command_session_runs_program_args_and_reports_exit_code() {
        let cfg = TerminalSessionCfg {
            program: Some("/bin/sh".to_string()),
            args: vec![
                "-lc".to_string(),
                "printf runnable-test; exit 7".to_string(),
            ],
            ..TerminalSessionCfg::default()
        };

        let (mut session, mut rx) = TerminalSession::spawn(42, cfg).await.unwrap();
        while rx.recv().await.is_some() {}

        assert_eq!(session.wait_exit_code(), Some(7));
    }

    #[cfg(any(unix, all(windows, feature = "emulator")))]
    #[tokio::test]
    async fn command_session_try_exit_code_reports_finished_child() {
        #[cfg(windows)]
        let cfg = TerminalSessionCfg {
            program: Some("cmd.exe".to_string()),
            args: vec!["/D".to_string(), "/C".to_string(), "exit 7".to_string()],
            ..TerminalSessionCfg::default()
        };
        #[cfg(not(windows))]
        let cfg = TerminalSessionCfg {
            program: Some("/bin/sh".to_string()),
            args: vec!["-lc".to_string(), "exit 7".to_string()],
            ..TerminalSessionCfg::default()
        };

        let (mut session, _rx) = TerminalSession::spawn(43, cfg).await.unwrap();
        let mut code = None;
        for _ in 0..30 {
            code = session.try_exit_code();
            if code.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        assert_eq!(code, Some(7));
    }

    #[cfg(all(windows, feature = "emulator"))]
    #[tokio::test]
    async fn default_windows_terminal_emits_visible_shell_output() {
        let cfg = TerminalSessionCfg::default();
        let (mut session, mut rx) = TerminalSession::spawn(42, cfg).await.unwrap();

        let mut saw_visible_output = false;
        let deadline = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(frame) = rx.recv().await {
                if frame_has_visible_output(&frame) {
                    saw_visible_output = true;
                    break;
                }
            }
        });

        let _ = deadline.await;
        let _ = session.kill().await;

        assert!(
            saw_visible_output,
            "default Windows shell produced no visible terminal output"
        );
    }

    #[cfg(all(windows, feature = "emulator"))]
    fn frame_has_visible_output(frame: &FramePayload) -> bool {
        match frame {
            FramePayload::Full(snapshot) => snapshot
                .rows
                .iter()
                .flatten()
                .any(|cell| !cell.ch.is_whitespace()),
            FramePayload::Diff(diff) => diff
                .lines
                .iter()
                .flat_map(|line| line.ranges.iter())
                .flat_map(|range| range.cells.iter())
                .any(|cell| !cell.ch.is_whitespace()),
            FramePayload::Raw(bytes) => bytes.iter().any(|byte| !byte.is_ascii_whitespace()),
        }
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
                            if row == self.cursor_row as usize {
                                // Cursor row: erase from start to cursor inclusive (ECMA-48)
                                for col in 0..=self.cursor_col as usize {
                                    if col < self.grid[row].len() {
                                        self.grid[row][col] = blank_cell();
                                    }
                                }
                            } else {
                                // Rows above cursor: erase entirely
                                for col in 0..self.cols as usize {
                                    self.grid[row][col] = blank_cell();
                                }
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
