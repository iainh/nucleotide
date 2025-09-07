// ABOUTME: Terminal runtime handler; consumes terminal events and updates view state
#![cfg(feature = "terminal-emulator")]

use nucleotide_core as core;
use nucleotide_events::v2::terminal::{Event as TerminalEvent, TerminalId};
use nucleotide_logging::{error, info, instrument, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nucleotide_terminal::frame::FramePayload;
use nucleotide_terminal::session::{TerminalSession, TerminalSessionCfg};
use nucleotide_terminal_view::{TerminalViewModel, register_view_model};

/// Manages terminal sessions and translates frames into UI view state updates
pub struct TerminalRuntimeHandler {
    sessions: HashMap<TerminalId, SessionEntry>,
}

struct SessionEntry {
    session: TerminalSession,
    #[allow(dead_code)]
    rx_task: std::thread::JoinHandle<()>,
    view: Arc<Mutex<TerminalViewModel>>,
}

impl TerminalRuntimeHandler {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn get_view_model(
        &self,
        id: TerminalId,
    ) -> Option<std::sync::Arc<std::sync::Mutex<TerminalViewModel>>> {
        self.sessions.get(&id).map(|e| e.view.clone())
    }

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
        let view_clone = Arc::clone(&view);
        // Register globally so UI panels can fetch by TerminalId
        register_view_model(id, view.clone());

        // Spawn a blocking thread to consume frames, coalescing bursts to the latest
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
        });

        self.sessions.insert(
            id,
            SessionEntry {
                session,
                rx_task: handle,
                view,
            },
        );
        info!(terminal_id=?id, "Terminal session spawned and consumer started");
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
                if let Some(entry) = self.sessions.get(id) {
                    let _ = futures_executor::block_on(entry.session.resize(*cols, *rows));
                }
            }
            TerminalEvent::Input { id, bytes } => {
                if let Some(entry) = self.sessions.get(id) {
                    let _ = futures_executor::block_on(entry.session.write(bytes));
                }
            }
            TerminalEvent::Output { .. } => {
                // Output is produced by session read loop; nothing to do
            }
            TerminalEvent::Exited { id, .. } => {
                if let Some(mut entry) = self.sessions.remove(id) {
                    let _ = futures_executor::block_on(entry.session.kill());
                    // Best-effort: join the rx task
                    let _ = entry.rx_task.join();
                }
            }
            TerminalEvent::FocusChanged { .. } => {}
        }
    }
}
