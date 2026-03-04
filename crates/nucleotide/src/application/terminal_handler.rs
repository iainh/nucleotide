// ABOUTME: Terminal runtime handler; consumes terminal events and updates view state

use nucleotide_core::{self as core, EventAggregatorHandle};
use nucleotide_events::EventBus;
use nucleotide_events::v2::terminal::{Event as TerminalEvent, TerminalId};
use nucleotide_logging::{error, info};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[cfg(feature = "terminal-emulator")]
use nucleotide_terminal::TerminalBounds;
#[cfg(feature = "terminal-emulator")]
use nucleotide_terminal::session::ControlMsg;
use nucleotide_terminal::session::{TerminalSession, TerminalSessionCfg};
use nucleotide_terminal_view::{TerminalViewModel, register_view_model};

/// Shared map of terminal input senders, allowing the UI thread to bypass the
/// event queue and write keystrokes directly to the PTY background writer.
pub type TerminalInputSenders = Arc<Mutex<HashMap<TerminalId, std::sync::mpsc::Sender<Vec<u8>>>>>;

/// Manages terminal sessions and translates frames into UI view state updates
pub struct TerminalRuntimeHandler {
    sessions: HashMap<TerminalId, SessionEntry>,
    /// Shared sender map so callers outside the event loop can write input directly
    input_senders: TerminalInputSenders,
    /// Event bus handle so consumer threads can dispatch Exited events
    event_bus: Option<EventAggregatorHandle>,
}

struct SessionEntry {
    // Protect session so we can use it from background IO workers without blocking the UI thread
    session: Arc<Mutex<TerminalSession>>,
    #[allow(dead_code)]
    rx_task: std::thread::JoinHandle<()>,
    // Background input writer to avoid blocking on each key press
    input_tx: std::sync::mpsc::Sender<Vec<u8>>,
    input_task: std::thread::JoinHandle<()>,
    view: Arc<Mutex<TerminalViewModel>>,
    last_size: Option<(u16, u16)>,
    #[cfg(feature = "terminal-emulator")]
    last_bounds: Option<TerminalBounds>,
}

impl TerminalRuntimeHandler {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
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
        let (session, mut rx) =
            match futures_executor::block_on(TerminalSession::spawn(id.0, cfg.clone())) {
                Ok(pair) => pair,
                Err(e) => {
                    error!(terminal_id=?id, error=%e, "Failed to spawn terminal session");
                    return;
                }
            };

        let view = Arc::new(Mutex::new(TerminalViewModel::new(id)));
        #[cfg(feature = "terminal-emulator")]
        {
            view.lock()
                .unwrap()
                .set_control_sender(session.control_sender());
        }
        let view_clone = Arc::clone(&view);
        // Register globally so UI panels can fetch by TerminalId
        register_view_model(id, view.clone());

        // Spawn a blocking thread to consume frames, coalescing bursts to the latest
        let exit_bus = self.event_bus.clone();
        let handle = std::thread::spawn(move || {
            loop {
                // Wait for at least one frame
                let Some(mut frame) = futures_executor::block_on(rx.recv()) else {
                    break;
                };
                // Drain any queued frames to coalesce updates
                while let Ok(next) = rx.try_recv() {
                    frame = next;
                }
                let mut guard = view_clone.lock().unwrap();
                guard.apply_frame(frame);
            }
            // Channel closed – shell process exited; mark view model and notify the event bus
            view_clone.lock().unwrap().set_exited();
            if let Some(bus) = exit_bus {
                bus.dispatch_terminal(TerminalEvent::Exited {
                    id,
                    code: None,
                    signal: None,
                });
            }
        });

        // Wrap session for cross-thread access and create a non-blocking input queue
        let session_arc = Arc::new(Mutex::new(session));
        let (tx, rx_input) = std::sync::mpsc::channel::<Vec<u8>>();
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
                input_tx: tx,
                input_task,
                view,
                last_size: None,
                #[cfg(feature = "terminal-emulator")]
                last_bounds: None,
            },
        );
        info!(terminal_id=?id, "Terminal session spawned and consumer started");
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
                    env: env.clone(),
                    cols: Some(80),
                    rows: Some(24),
                };
                // No bus needed here yet; placeholder for future reporting
                self.handle_spawn(*id, &cfg);
            }
            TerminalEvent::Resized { id, cols, rows } => {
                if let Some(entry) = self.sessions.get_mut(id) {
                    let size_changed = entry
                        .last_size
                        .map(|(prev_cols, prev_rows)| prev_cols != *cols || prev_rows != *rows)
                        .unwrap_or(true);

                    if size_changed {
                        if let Ok(session) = entry.session.lock() {
                            let _ = futures_executor::block_on(session.resize(*cols, *rows));
                        }
                        #[cfg(feature = "terminal-emulator")]
                        if let Ok(mut view) = entry.view.lock() {
                            view.resize_grid(*cols, *rows, None);
                        }
                        entry.last_size = Some((*cols, *rows));
                        #[cfg(feature = "terminal-emulator")]
                        if let Some(existing) = entry.last_bounds.as_mut() {
                            *existing = existing.with_cells(*cols, *rows);
                        }
                    }
                }
            }
            TerminalEvent::ResizedWithMetrics {
                id,
                cols,
                rows,
                cell_width,
                cell_height,
            } => {
                if let Some(entry) = self.sessions.get_mut(id) {
                    #[cfg(feature = "terminal-emulator")]
                    {
                        let new_bounds =
                            TerminalBounds::from_cells(*cell_width, *cell_height, *cols, *rows);
                        let bounds_changed = entry
                            .last_bounds
                            .as_ref()
                            .map(|prev| !prev.approx_eq(&new_bounds))
                            .unwrap_or(true);

                        if bounds_changed {
                            if let Ok(session) = entry.session.lock() {
                                // Push control message to engine so emulator redraws with new metrics
                                let _ = session.control_sender().send(ControlMsg::Resize {
                                    cols: *cols,
                                    rows: *rows,
                                    cell_width: *cell_width,
                                    cell_height: *cell_height,
                                });
                                // Also resize PTY to maintain app expectations
                                let _ = futures_executor::block_on(session.resize(*cols, *rows));
                            }
                            if let Ok(mut view) = entry.view.lock() {
                                view.resize_grid(*cols, *rows, Some(new_bounds.cell_size()));
                            }
                            entry.last_bounds = Some(new_bounds);
                            entry.last_size = Some((*cols, *rows));
                        }
                    }
                    #[cfg(not(feature = "terminal-emulator"))]
                    {
                        let _ = (cell_width, cell_height);
                        let size_changed = entry
                            .last_size
                            .map(|(prev_cols, prev_rows)| prev_cols != *cols || prev_rows != *rows)
                            .unwrap_or(true);

                        if size_changed {
                            if let Ok(session) = entry.session.lock() {
                                let _ = futures_executor::block_on(session.resize(*cols, *rows));
                            }
                            entry.last_size = Some((*cols, *rows));
                        }
                    }
                }
            }
            TerminalEvent::Input { id, bytes } => {
                if let Some(entry) = self.sessions.get(id) {
                    // Send bytes to background writer; drop if receiver is gone
                    let _ = entry.input_tx.send(bytes.clone());
                }
            }
            TerminalEvent::Output { .. } => {
                // Output is produced by session read loop; nothing to do
            }
            TerminalEvent::Exited { id, .. } => {
                // Remove from shared sender map first
                if let Ok(mut senders) = self.input_senders.lock() {
                    senders.remove(id);
                }
                if let Some(entry) = self.sessions.remove(id) {
                    // Close input channel to stop input task
                    drop(entry.input_tx);
                    // Best-effort: kill session and join workers
                    if let Ok(mut session) = entry.session.lock() {
                        let _ = futures_executor::block_on(session.kill());
                    }
                    let _ = entry.rx_task.join();
                    let _ = entry.input_task.join();
                }
            }
            TerminalEvent::FocusChanged { .. } => {}
        }
    }
}
