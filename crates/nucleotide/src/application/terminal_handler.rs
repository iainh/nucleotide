// ABOUTME: Terminal runtime handler; consumes terminal events and updates view state

use nucleotide_core::{self as core, EventAggregatorHandle};
use nucleotide_events::EventBus;
use nucleotide_events::v2::terminal::{Event as TerminalEvent, TerminalId};
use nucleotide_logging::{error, info};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[cfg(feature = "terminal-emulator-core")]
use nucleotide_terminal::TerminalBounds;
#[cfg(feature = "terminal-emulator-core")]
use nucleotide_terminal::session::ControlMsg;
use nucleotide_terminal::session::{TerminalSession, TerminalSessionCfg};
use nucleotide_terminal_view::{TerminalViewModel, register_view_model};

/// Shared map of terminal input senders, allowing the UI thread to bypass the
/// event queue and write keystrokes directly to the PTY background writer.
pub type TerminalInputSenders = Arc<Mutex<HashMap<TerminalId, std::sync::mpsc::Sender<Vec<u8>>>>>;

/// Manages terminal sessions and translates frames into UI view state updates
pub struct TerminalRuntimeHandler {
    sessions: HashMap<TerminalId, SessionEntry>,
    pending_resizes: HashMap<TerminalId, PendingTerminalResize>,
    /// Shared sender map so callers outside the event loop can write input directly
    input_senders: TerminalInputSenders,
    /// Event bus handle so consumer threads can dispatch Exited events
    event_bus: Option<EventAggregatorHandle>,
}

#[derive(Clone, Copy, Debug)]
struct PendingTerminalResize {
    cols: u16,
    rows: u16,
    cell_width: f32,
    cell_height: f32,
}

struct SessionEntry {
    // Protect session so we can use it from background IO workers without blocking the UI thread
    session: Arc<Mutex<TerminalSession>>,
    #[allow(dead_code)]
    rx_task: std::thread::JoinHandle<()>,
    #[allow(dead_code)]
    exit_task: std::thread::JoinHandle<()>,
    exit_reported: Arc<AtomicBool>,
    // Background input writer to avoid blocking on each key press
    input_tx: std::sync::mpsc::Sender<Vec<u8>>,
    #[allow(dead_code)]
    input_task: std::thread::JoinHandle<()>,
    view: Arc<Mutex<TerminalViewModel>>,
    last_size: Option<(u16, u16)>,
    #[cfg(feature = "terminal-emulator-core")]
    last_bounds: Option<TerminalBounds>,
}

#[cfg(feature = "terminal-emulator-core")]
fn metrics_resize_bounds(
    last_bounds: Option<TerminalBounds>,
    new_bounds: TerminalBounds,
) -> Option<TerminalBounds> {
    last_bounds
        .is_none_or(|prev| !prev.approx_eq(&new_bounds))
        .then_some(new_bounds)
}

fn lock_view_model<'a>(
    view: &'a Mutex<TerminalViewModel>,
    id: TerminalId,
    operation: &'static str,
) -> MutexGuard<'a, TerminalViewModel> {
    match view.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            error!(
                terminal_id = ?id,
                operation,
                "Terminal view model lock poisoned; recovering"
            );
            poisoned.into_inner()
        }
    }
}

fn quote_command_part(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '/' | '\\' | '.' | '_' | '-' | '=' | ':' | '@' | ',' | '+'
            )
    }) {
        return value.to_string();
    }

    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\"'\"'");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

fn terminal_command_summary(cfg: &TerminalSessionCfg) -> String {
    let Some(program) = cfg.program.as_deref().or(cfg.shell.as_deref()) else {
        return "default login shell".to_string();
    };

    std::iter::once(program)
        .chain(cfg.args.iter().map(String::as_str))
        .map(quote_command_part)
        .collect::<Vec<_>>()
        .join(" ")
}

fn terminal_program_is_ssh(cfg: &TerminalSessionCfg) -> bool {
    let Some(program) = cfg.program.as_deref() else {
        return false;
    };
    let file_name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let lower_file_name = file_name.to_ascii_lowercase();
    lower_file_name
        .strip_suffix(".exe")
        .unwrap_or(&lower_file_name)
        == "ssh"
}

fn terminal_spawn_failure_details(cfg: &TerminalSessionCfg, error: &anyhow::Error) -> Vec<String> {
    let mut details = vec![
        format!("Command: {}", terminal_command_summary(cfg)),
        format!("Spawn error: {error:#}"),
    ];

    if let Some(cwd) = cfg.cwd.as_ref() {
        details.insert(1, format!("Working directory: {}", cwd.display()));
    }

    if terminal_program_is_ssh(cfg) {
        details.push(
            "SSH remote terminals start a local PTY running OpenSSH with -tt before the remote shell starts."
                .to_string(),
        );
        #[cfg(windows)]
        details.push(
            "Windows: verify the OpenSSH Client optional feature is installed and System32\\OpenSSH\\ssh.exe is accessible to Nucleotide."
                .to_string(),
        );
        details.push(
            "Try the command above from PowerShell or a local terminal to check SSH options, host keys, ControlMaster, and remote shell startup."
                .to_string(),
        );
    }

    details
}

impl TerminalRuntimeHandler {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            pending_resizes: HashMap::new(),
            input_senders: Arc::new(Mutex::new(HashMap::new())),
            event_bus: None,
        }
    }

    /// Set the event bus handle so exit events can be dispatched from consumer threads
    pub fn set_event_bus(&mut self, bus: EventAggregatorHandle) {
        self.event_bus = Some(bus);
    }

    /// Get a clone of the shared input senders map.
    /// Store this on the Application so the UI thread can send input directly.
    pub fn input_senders(&self) -> TerminalInputSenders {
        self.input_senders.clone()
    }

    pub fn get_view_model(
        &self,
        id: TerminalId,
    ) -> Option<std::sync::Arc<std::sync::Mutex<TerminalViewModel>>> {
        self.sessions.get(&id).map(|e| e.view.clone())
    }

    #[allow(clippy::await_holding_lock)]
    fn handle_spawn(&mut self, id: TerminalId, cfg: &TerminalSessionCfg) {
        let cfg = cfg.clone();
        let view = Arc::new(Mutex::new(TerminalViewModel::new(id)));
        register_view_model(id, view.clone());

        let (session, mut rx) =
            match futures_executor::block_on(TerminalSession::spawn(id.0, cfg.clone())) {
                Ok(pair) => pair,
                Err(e) => {
                    let details = terminal_spawn_failure_details(&cfg, &e);
                    lock_view_model(view.as_ref(), id, "set_spawn_failure")
                        .set_spawn_failure("Terminal session failed to start", details);
                    error!(terminal_id=?id, error=%e, "Failed to spawn terminal session");
                    return;
                }
            };

        #[cfg(feature = "terminal-emulator-core")]
        {
            lock_view_model(view.as_ref(), id, "set_control_sender")
                .set_control_sender(session.control_sender());
        }
        let view_clone = Arc::clone(&view);

        // Wrap session for cross-thread access and create a non-blocking input queue
        let session_arc = Arc::new(Mutex::new(session));

        // Spawn a blocking thread to consume frames, coalescing bursts to the latest
        let exit_bus = self.event_bus.clone();
        let session_for_exit = Arc::clone(&session_arc);
        let exit_reported = Arc::new(AtomicBool::new(false));
        let rx_exit_reported = Arc::clone(&exit_reported);
        let handle = std::thread::spawn(move || {
            while let Some(mut frame) = futures_executor::block_on(rx.recv()) {
                // Drain any queued frames to coalesce updates
                while let Ok(next) = rx.try_recv() {
                    frame = next;
                }
                let mut guard = lock_view_model(view_clone.as_ref(), id, "apply_frame");
                guard.apply_frame(frame);
            }
            // Channel closed – shell process exited; mark view model and notify the event bus
            lock_view_model(view_clone.as_ref(), id, "set_exited").set_exited();
            let code = session_for_exit
                .lock()
                .ok()
                .and_then(|mut session| session.wait_exit_code());
            if !rx_exit_reported.swap(true, Ordering::SeqCst)
                && let Some(bus) = exit_bus
            {
                bus.dispatch_terminal(TerminalEvent::Exited {
                    id,
                    code,
                    signal: None,
                });
            }
        });

        let exit_bus = self.event_bus.clone();
        let exit_view = Arc::clone(&view);
        let session_for_monitor = Arc::clone(&session_arc);
        let monitor_exit_reported = Arc::clone(&exit_reported);
        let exit_task = std::thread::spawn(move || {
            while !monitor_exit_reported.load(Ordering::SeqCst) {
                let code = session_for_monitor
                    .lock()
                    .ok()
                    .and_then(|mut session| session.try_exit_code());

                if let Some(code) = code {
                    lock_view_model(exit_view.as_ref(), id, "set_exited").set_exited();
                    if !monitor_exit_reported.swap(true, Ordering::SeqCst)
                        && let Some(bus) = exit_bus
                    {
                        bus.dispatch_terminal(TerminalEvent::Exited {
                            id,
                            code: Some(code),
                            signal: None,
                        });
                    }
                    break;
                }

                std::thread::sleep(Duration::from_millis(100));
            }
        });

        let (tx, rx_input) = std::sync::mpsc::channel::<Vec<u8>>();
        #[cfg(feature = "terminal-emulator-core")]
        if let Ok(mut guard) = view.lock() {
            guard.set_input_sender(tx.clone());
        }
        let session_for_input = session_arc.clone();
        let input_task = std::thread::spawn(move || {
            while let Ok(bytes) = rx_input.recv() {
                // Best-effort synchronous write; no block_on overhead
                if let Ok(guard) = session_for_input.lock() {
                    let _ = guard.write_sync(&bytes);
                }
            }
        });

        // Register sender in shared map so UI thread can bypass the event queue
        if let Ok(mut senders) = self.input_senders.lock() {
            senders.insert(id, tx.clone());
        }

        self.sessions.insert(
            id,
            SessionEntry {
                session: session_arc,
                rx_task: handle,
                exit_task,
                exit_reported,
                input_tx: tx,
                input_task,
                view,
                last_size: None,
                #[cfg(feature = "terminal-emulator-core")]
                last_bounds: None,
            },
        );
        self.apply_pending_resize(id);
        info!(terminal_id=?id, "Terminal session spawned and consumer started");
    }

    fn handle_resize(
        &mut self,
        id: TerminalId,
        cols: u16,
        rows: u16,
        cell_width: f32,
        cell_height: f32,
    ) {
        if self.sessions.contains_key(&id) {
            self.apply_resize(id, cols, rows, cell_width, cell_height);
        } else {
            self.pending_resizes.insert(
                id,
                PendingTerminalResize {
                    cols,
                    rows,
                    cell_width,
                    cell_height,
                },
            );
        }
    }

    fn apply_pending_resize(&mut self, id: TerminalId) {
        let Some(resize) = self.pending_resizes.remove(&id) else {
            return;
        };

        self.apply_resize(
            id,
            resize.cols,
            resize.rows,
            resize.cell_width,
            resize.cell_height,
        );
    }

    fn apply_resize(
        &mut self,
        id: TerminalId,
        cols: u16,
        rows: u16,
        cell_width: f32,
        cell_height: f32,
    ) {
        let Some(entry) = self.sessions.get_mut(&id) else {
            return;
        };

        #[cfg(feature = "terminal-emulator-core")]
        {
            let new_bounds = TerminalBounds::from_cells(cell_width, cell_height, cols, rows);

            if let Some(new_bounds) = metrics_resize_bounds(entry.last_bounds, new_bounds) {
                if let Ok(session) = entry.session.lock() {
                    // Push control message to engine so emulator redraws with new metrics
                    let _ = session.control_sender().send(ControlMsg::Resize {
                        cols,
                        rows,
                        cell_width,
                        cell_height,
                    });
                    // Also resize PTY to maintain app expectations
                    let _ = futures_executor::block_on(session.resize(cols, rows));
                }
                if let Ok(mut view) = entry.view.lock() {
                    view.resize_grid(cols, rows, Some(new_bounds.cell_size()));
                }
                entry.last_bounds = Some(new_bounds);
                entry.last_size = Some((cols, rows));
            }
        }
        #[cfg(not(feature = "terminal-emulator-core"))]
        {
            let _ = (cell_width, cell_height);
            let size_changed = entry
                .last_size
                .map(|(prev_cols, prev_rows)| prev_cols != cols || prev_rows != rows)
                .unwrap_or(true);

            if size_changed {
                if let Ok(session) = entry.session.lock() {
                    let _ = futures_executor::block_on(session.resize(cols, rows));
                }
                entry.last_size = Some((cols, rows));
            }
        }
    }
}
impl Default for TerminalRuntimeHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl core::EventHandler for TerminalRuntimeHandler {
    fn handle_terminal(&mut self, event: &TerminalEvent) {
        match event {
            TerminalEvent::SpawnRequested {
                id,
                cwd,
                shell,
                env,
            } => {
                let cfg = TerminalSessionCfg {
                    cwd: cwd.clone(),
                    shell: shell.clone(),
                    program: None,
                    args: Vec::new(),
                    env: env.clone(),
                    cols: Some(80),
                    rows: Some(24),
                };
                self.handle_spawn(*id, &cfg);
            }
            TerminalEvent::CommandSpawnRequested {
                id,
                cwd,
                program,
                args,
                env,
            } => {
                let cfg = TerminalSessionCfg {
                    cwd: cwd.clone(),
                    shell: None,
                    program: Some(program.clone()),
                    args: args.clone(),
                    env: env.clone(),
                    cols: Some(80),
                    rows: Some(24),
                };
                self.handle_spawn(*id, &cfg);
            }
            TerminalEvent::Resized {
                id,
                cols,
                rows,
                cell_width,
                cell_height,
            } => {
                self.handle_resize(*id, *cols, *rows, *cell_width, *cell_height);
            }
            TerminalEvent::Input { id, bytes } => {
                if let Some(entry) = self.sessions.get(id) {
                    // Send bytes to background writer; drop if receiver is gone
                    let _ = entry.input_tx.send(bytes.clone());
                }
            }
            TerminalEvent::Exited { id, .. } => {
                // Remove from shared sender map first
                if let Ok(mut senders) = self.input_senders.lock() {
                    senders.remove(id);
                }
                self.pending_resizes.remove(id);
                if let Some(entry) = self.sessions.remove(id) {
                    entry.exit_reported.store(true, Ordering::SeqCst);
                    #[cfg(feature = "terminal-emulator-core")]
                    if let Ok(mut view) = entry.view.lock() {
                        view.clear_input_sender();
                    }
                    // Close input channel to stop input task
                    drop(entry.input_tx);
                    // Best-effort: kill the PTY. Do not join worker threads here:
                    // on Windows, ConPTY can keep the reader blocked after the
                    // child exits, and blocking event processing prevents the
                    // workspace from observing Exited and closing the panel.
                    if let Ok(mut session) = entry.session.lock() {
                        let _ = futures_executor::block_on(session.kill());
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "terminal-emulator-core"))]
mod tests {
    use super::*;

    #[test]
    fn metrics_resize_still_applies_when_cell_metrics_change() {
        let initial = TerminalBounds::from_cells(8.0, 16.0, 80, 24);
        let updated_metrics = TerminalBounds::from_cells(9.0, 18.0, 100, 30);

        let metrics_resize = metrics_resize_bounds(Some(initial), updated_metrics)
            .expect("changed cell metrics should force an emulator resize");

        assert_eq!(metrics_resize.cols(), 100);
        assert_eq!(metrics_resize.rows(), 30);
        assert_eq!(metrics_resize.cell_size(), (9.0, 18.0));
    }

    #[test]
    fn resize_before_spawn_is_kept_until_session_exists() {
        let mut handler = TerminalRuntimeHandler::new();
        let id = TerminalId(1);

        handler.handle_resize(id, 120, 32, 9.0, 18.0);

        match handler.pending_resizes.get(&id) {
            Some(PendingTerminalResize {
                cols,
                rows,
                cell_width,
                cell_height,
            }) => {
                assert_eq!((*cols, *rows), (120, 32));
                assert_eq!((*cell_width, *cell_height), (9.0, 18.0));
            }
            resize => panic!("unexpected pending resize: {resize:?}"),
        }
    }

    #[test]
    fn terminal_spawn_failure_details_include_ssh_context() {
        let cfg = TerminalSessionCfg {
            program: Some(r"C:\Windows\System32\OpenSSH\ssh.exe".to_string()),
            args: vec!["-tt".to_string(), "--".to_string(), "devbox".to_string()],
            ..TerminalSessionCfg::default()
        };
        let error = anyhow::anyhow!("program not found");
        let details = terminal_spawn_failure_details(&cfg, &error);

        assert!(terminal_program_is_ssh(&cfg));
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("local PTY running OpenSSH"))
        );
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("program not found"))
        );
    }
}
