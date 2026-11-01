// ABOUTME: Terminal runtime handler; consumes terminal events and updates view state

use nucleotide_events::terminal::{Event as TerminalEvent, TerminalId};
use nucleotide_logging::{error, info};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[cfg(feature = "terminal-emulator-core")]
use nucleotide_terminal::TerminalBounds;
#[cfg(feature = "terminal-emulator-core")]
use nucleotide_terminal::session::ControlMsg;
use nucleotide_terminal::session::{
    ExternalTerminalCompletion, ExternalTerminalControl, ExternalTerminalParts,
};
use nucleotide_terminal::session::{TerminalSession, TerminalSessionCfg};
use nucleotide_terminal_view::{TerminalViewModel, register_view_model};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct RemoteTerminalSessionProvider {
    display_root: PathBuf,
    native_root: PathBuf,
    launcher: Arc<dyn nucleotide_remote::RemotePtySessionLauncher>,
}

impl RemoteTerminalSessionProvider {
    pub fn from_connection(
        connection: &nucleotide_remote::WorkspaceBackendConnection,
    ) -> Option<Self> {
        connection
            .pty_session_launcher
            .as_ref()
            .map(|launcher| Self {
                display_root: connection.location.display_root().to_path_buf(),
                native_root: connection.location.native_root().to_path_buf(),
                launcher: Arc::clone(launcher),
            })
    }

    pub fn handles(&self, workspace: &Path, cwd: &Path) -> bool {
        workspace.starts_with(&self.display_root) && cwd.starts_with(&self.display_root)
    }

    fn native_path(&self, path: &Path) -> anyhow::Result<PathBuf> {
        if !path.starts_with(&self.display_root) {
            anyhow::bail!("terminal cwd is outside the connected remote workspace");
        }
        Ok(
            nucleotide_workspace::WorkspacePathMapping::new(&self.display_root, &self.native_root)
                .to_native_path(path),
        )
    }

    fn launch(
        &self,
        id: TerminalId,
        cwd: &Path,
        command: nucleotide_remote::PtySessionCommand,
        env: &[(String, String)],
    ) -> anyhow::Result<(
        TerminalSession,
        tokio::sync::mpsc::Receiver<nucleotide_terminal::frame::FramePayload>,
    )> {
        let parts = self.launcher.launch(nucleotide_remote::PtySessionRequest {
            command,
            cwd: self.native_path(cwd)?,
            environment: nucleotide_remote::ProjectEnvironmentSelection::WorkspaceOrNearest {
                anchor: self.native_root.clone(),
            },
            env_overrides: env.iter().cloned().collect(),
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let completion = BlockingCompletion::new(parts.completion);
        let external = ExternalTerminalParts {
            reader: Box::new(BlockingReader(parts.output)),
            writer: Box::new(BlockingWriter(parts.input)),
            control: Box::new(RemoteControl(parts.control)),
            completion: Box::new(completion),
            lifecycle: Some(parts.lifecycle),
        };
        Ok(TerminalSession::from_external(id.0, external, 80, 24))
    }
}

struct BlockingReader(nucleotide_remote::RemoteProcessSessionReader);
impl Read for BlockingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        futures_executor::block_on(self.0.read(buffer))
    }
}

struct BlockingWriter(nucleotide_remote::RemoteProcessSessionWriter);
impl Write for BlockingWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        futures_executor::block_on(self.0.write(buffer))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        futures_executor::block_on(self.0.flush())
    }
}

struct RemoteControl(Box<dyn nucleotide_remote::RemotePtySessionControl>);
impl ExternalTerminalControl for RemoteControl {
    fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        self.0
            .resize(cols, rows, 0, 0)
            .map_err(std::io::Error::other)
    }
    fn cancel(&self) -> std::io::Result<()> {
        self.0.cancel();
        Ok(())
    }
}

type CompletionResult = Result<Option<i32>, String>;
struct BlockingCompletion {
    result: Arc<(Mutex<Option<CompletionResult>>, Condvar)>,
}
impl BlockingCompletion {
    fn new(completion: nucleotide_remote::RemoteProcessSessionCompletionFuture) -> Self {
        let result = Arc::new((Mutex::new(None), Condvar::new()));
        let worker_result = Arc::clone(&result);
        std::thread::spawn(move || {
            let completed = futures_executor::block_on(completion)
                .map(|completion| completion.status_code)
                .map_err(|error| error.to_string());
            let (lock, wake) = &*worker_result;
            *lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(completed);
            wake.notify_all();
        });
        Self { result }
    }
}
impl ExternalTerminalCompletion for BlockingCompletion {
    fn poll_exit(&mut self) -> std::io::Result<Option<Option<i32>>> {
        self.result
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|result| result.clone().map_err(std::io::Error::other))
            .transpose()
    }

    fn try_exit_code(&mut self) -> std::io::Result<Option<i32>> {
        let result = self
            .result
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match result.as_ref() {
            None => Ok(None),
            Some(Ok(code)) => Ok(*code),
            Some(Err(error)) => Err(std::io::Error::other(error.clone())),
        }
    }
    fn wait_exit_code(&mut self) -> std::io::Result<Option<i32>> {
        let (lock, wake) = &*self.result;
        let mut result = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while result.is_none() {
            result = wake
                .wait(result)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        match result.as_ref().unwrap() {
            Ok(code) => Ok(*code),
            Err(error) => Err(std::io::Error::other(error.clone())),
        }
    }
}

/// Shared map of terminal input senders, allowing the UI thread to bypass the
/// event queue and write keystrokes directly to the PTY background writer.
pub type TerminalInputSenders = Arc<Mutex<HashMap<TerminalId, std::sync::mpsc::Sender<Vec<u8>>>>>;

/// Connect this to the application's typed Update::Terminal queue and maintenance wake.
/// Called from workers, never with the runtime mutex held. Must not access GPUI directly.
pub type TerminalEventSink = Arc<dyn Fn(TerminalEvent) + Send + Sync>;
type SharedEventSink = Arc<Mutex<Option<TerminalEventSink>>>;

fn publish(sink: &SharedEventSink, event: TerminalEvent) {
    let callback = sink
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(callback) = callback {
        callback(event);
    }
}

#[derive(Clone)]
pub struct TerminalRuntimeHandle {
    inner: Arc<Mutex<TerminalRuntimeHandler>>,
    input_senders: TerminalInputSenders,
    remote_provider: Arc<Mutex<Option<RemoteTerminalSessionProvider>>>,
}

impl TerminalRuntimeHandle {
    pub fn new() -> Self {
        let handler = TerminalRuntimeHandler::new();
        let input_senders = handler.input_senders();
        Self {
            inner: Arc::new(Mutex::new(handler)),
            input_senders,
            remote_provider: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_event_sink(&self, sink: TerminalEventSink) {
        let handler = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *handler
            .event_sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
    }

    /// Reserve the model before resolving project environment or remote commands.
    /// IDs must be unique for the lifetime of this runtime, including across workspaces.
    pub fn prepare(&self, id: TerminalId) -> Option<Arc<Mutex<TerminalViewModel>>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prepare(id)
    }

    pub fn dispatch(&self, event: &TerminalEvent) {
        let spawn = match event {
            TerminalEvent::SpawnRequested {
                id,
                cwd,
                shell,
                env,
            } => Some((
                *id,
                TerminalSessionCfg {
                    cwd: cwd.clone(),
                    shell: shell.clone(),
                    env: env.clone(),
                    cols: Some(80),
                    rows: Some(24),
                    ..Default::default()
                },
                None,
            )),
            TerminalEvent::CommandSpawnRequested {
                id,
                cwd,
                program,
                args,
                env,
            } => Some((
                *id,
                TerminalSessionCfg {
                    cwd: cwd.clone(),
                    program: Some(program.clone()),
                    args: args.clone(),
                    env: env.clone(),
                    cols: Some(80),
                    rows: Some(24),
                    ..Default::default()
                },
                None,
            )),
            TerminalEvent::ServiceSpawnRequested {
                id,
                cwd,
                shell,
                env,
            } => Some((
                *id,
                TerminalSessionCfg {
                    cwd: Some(cwd.clone()),
                    shell: shell.clone(),
                    env: env.clone(),
                    ..Default::default()
                },
                Some(nucleotide_remote::PtySessionCommand::LoginShell {
                    shell: shell.clone(),
                }),
            )),
            TerminalEvent::ServiceCommandSpawnRequested {
                id,
                cwd,
                program,
                args,
                env,
            } => Some((
                *id,
                TerminalSessionCfg {
                    cwd: Some(cwd.clone()),
                    program: Some(program.clone()),
                    args: args.clone(),
                    env: env.clone(),
                    ..Default::default()
                },
                Some(nucleotide_remote::PtySessionCommand::Command {
                    program: program.clone(),
                    args: args.clone(),
                }),
            )),
            _ => None,
        };
        if let Some((id, cfg, service_command)) = spawn {
            {
                let mut handler = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if handler.prepare(id).is_none() || !handler.started.insert(id) {
                    return;
                }
            }
            let provider = self
                .remote_provider
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let runtime = self.clone();
            let executor = tokio::runtime::Handle::try_current();
            std::thread::spawn(move || {
                // Launch without holding the runtime lock: close and other terminals stay responsive.
                let result = if let Ok(executor) = executor {
                    let _entered = executor.enter();
                    if let Some(command) = service_command {
                        provider
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "The connected workspace does not provide service-owned PTYs"
                                )
                            })
                            .and_then(|provider| {
                                provider.launch(id, cfg.cwd.as_deref().unwrap(), command, &cfg.env)
                            })
                    } else {
                        futures_executor::block_on(TerminalSession::spawn(id.0, cfg.clone()))
                    }
                } else {
                    Err(anyhow::anyhow!(
                        "Terminal launch requires the application's Tokio executor"
                    ))
                };
                runtime.finish_spawn(id, cfg, result);
            });
            return;
        }
        match self.inner.lock() {
            Ok(mut handler) => handler.handle_event(event),
            Err(poisoned) => {
                error!("Terminal runtime lock poisoned; recovering");
                poisoned.into_inner().handle_event(event);
            }
        }
    }

    fn finish_spawn(
        &self,
        id: TerminalId,
        cfg: TerminalSessionCfg,
        result: anyhow::Result<(
            TerminalSession,
            tokio::sync::mpsc::Receiver<nucleotide_terminal::frame::FramePayload>,
        )>,
    ) {
        let mut handler = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if handler.closed.contains(&id) {
            drop(handler);
            if let Ok((mut session, _rx)) = result {
                let _ = futures_executor::block_on(session.kill());
                let _ = session.wait_exit_code();
            }
            return;
        }
        let sink = handler.event_sink.clone();
        let (event, ready) = match result {
            Ok((session, rx)) => {
                let ready = handler.register_session(id, cfg, session, rx);
                (TerminalEvent::Spawned { id }, Some(ready))
            }
            Err(error) => {
                let message = format!("{error:#}");
                handler.pending_input.remove(&id);
                handler.register_spawn_failure(id, &cfg, error);
                (TerminalEvent::SpawnFailed { id, error: message }, None)
            }
        };
        drop(handler);
        publish(&sink, event);
        if let Some(ready) = ready {
            ready.wait();
        }
    }

    pub fn send_input(&self, id: TerminalId, bytes: Vec<u8>) -> bool {
        let sent = self
            .input_senders
            .lock()
            .ok()
            .and_then(|senders| senders.get(&id).cloned())
            .is_some_and(|sender| sender.send(bytes.clone()).is_ok());
        if sent {
            return true;
        }
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .queue_input(id, bytes)
    }

    pub fn set_remote_provider(&self, provider: Option<RemoteTerminalSessionProvider>) {
        *self
            .remote_provider
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = provider;
    }

    pub fn handles_remote_terminal(&self, workspace: &Path, cwd: &Path) -> bool {
        self.remote_provider
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|provider| provider.handles(workspace, cwd))
    }

    pub fn has_remote_terminal_provider_for(&self, workspace: &Path) -> bool {
        self.remote_provider
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|provider| workspace.starts_with(&provider.display_root))
    }
}

impl Default for TerminalRuntimeHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages terminal sessions and translates frames into UI view state updates
pub struct TerminalRuntimeHandler {
    sessions: HashMap<TerminalId, SessionEntry>,
    pending_resizes: HashMap<TerminalId, PendingTerminalResize>,
    models: HashMap<TerminalId, Arc<Mutex<TerminalViewModel>>>,
    started: HashSet<TerminalId>,
    closed: HashSet<TerminalId>,
    pending_input: HashMap<TerminalId, Vec<Vec<u8>>>,
    event_sink: SharedEventSink,
    /// Shared sender map so callers outside the event loop can write input directly
    input_senders: TerminalInputSenders,
}

#[derive(Clone, Copy, Debug)]
struct PendingTerminalResize {
    cols: u16,
    rows: u16,
    cell_width: f32,
    cell_height: f32,
}

struct SessionEntry {
    #[allow(dead_code)]
    rx_task: std::thread::JoinHandle<()>,
    #[allow(dead_code)]
    exit_task: std::thread::JoinHandle<()>,
    exit_reported: Arc<AtomicBool>,
    // Background input writer to avoid blocking on each key press
    input_tx: std::sync::mpsc::Sender<Vec<u8>>,
    #[allow(dead_code)]
    input_task: std::thread::JoinHandle<()>,
    control_tx: std::sync::mpsc::Sender<SessionControl>,
    view: Arc<Mutex<TerminalViewModel>>,
    last_size: Option<(u16, u16)>,
    #[cfg(feature = "terminal-emulator-core")]
    last_bounds: Option<TerminalBounds>,
}

enum SessionControl {
    Resize(PendingTerminalResize),
    Terminate,
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
            models: HashMap::new(),
            started: HashSet::new(),
            closed: HashSet::new(),
            pending_input: HashMap::new(),
            event_sink: Arc::new(Mutex::new(None)),
            input_senders: Arc::new(Mutex::new(HashMap::new())),
        }
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
        self.models.get(&id).cloned()
    }

    fn prepare(&mut self, id: TerminalId) -> Option<Arc<Mutex<TerminalViewModel>>> {
        if self.closed.contains(&id) {
            return None;
        }
        Some(
            self.models
                .entry(id)
                .or_insert_with(|| {
                    let view = Arc::new(Mutex::new(TerminalViewModel::new(id)));
                    register_view_model(id, view.clone());
                    view
                })
                .clone(),
        )
    }

    fn queue_input(&mut self, id: TerminalId, bytes: Vec<u8>) -> bool {
        if self.closed.contains(&id) {
            return false;
        }
        if let Some(entry) = self.sessions.get(&id) {
            return !entry.exit_reported.load(Ordering::SeqCst)
                && entry.input_tx.send(bytes).is_ok();
        }
        if let Some(model) = self.models.get(&id)
            && !lock_view_model(model, id, "queue_input").has_exited()
        {
            self.pending_input.entry(id).or_default().push(bytes);
            return true;
        }
        false
    }

    fn register_spawn_failure(
        &self,
        id: TerminalId,
        cfg: &TerminalSessionCfg,
        error: anyhow::Error,
    ) {
        let view = self.models.get(&id).expect("spawn reserves a model");
        let details = terminal_spawn_failure_details(cfg, &error);
        lock_view_model(view.as_ref(), id, "set_spawn_failure")
            .set_spawn_failure("Terminal session failed to start", details);
        error!(terminal_id=?id, error=%error, "Failed to spawn terminal session");
    }

    fn register_session(
        &mut self,
        id: TerminalId,
        _cfg: TerminalSessionCfg,
        session: TerminalSession,
        mut rx: tokio::sync::mpsc::Receiver<nucleotide_terminal::frame::FramePayload>,
    ) -> Arc<std::sync::Barrier> {
        let view = self
            .models
            .get(&id)
            .expect("spawn reserves a model")
            .clone();

        #[cfg(feature = "terminal-emulator-core")]
        {
            lock_view_model(view.as_ref(), id, "set_control_sender")
                .set_control_sender(session.control_sender());
        }
        let view_clone = Arc::clone(&view);

        // Wrap session for cross-thread access and create a non-blocking input queue
        let session_arc = Arc::new(Mutex::new(session));

        // Apply every incremental frame, even while no surface is mounted.
        let exit_reported = Arc::new(AtomicBool::new(false));
        let frames_drained = Arc::new(AtomicBool::new(false));
        let rx_drained = frames_drained.clone();
        let handle = std::thread::spawn(move || {
            while let Some(frame) = futures_executor::block_on(rx.recv()) {
                let mut guard = lock_view_model(view_clone.as_ref(), id, "apply_frame");
                guard.apply_frame(frame);
            }
            rx_drained.store(true, Ordering::SeqCst);
        });

        let exit_view = Arc::clone(&view);
        let session_for_monitor = Arc::clone(&session_arc);
        let monitor_exit_reported = Arc::clone(&exit_reported);
        let event_sink = self.event_sink.clone();
        let input_senders = self.input_senders.clone();
        let ready = Arc::new(std::sync::Barrier::new(2));
        let monitor_ready = ready.clone();
        let exit_task = std::thread::spawn(move || {
            monitor_ready.wait(); // Spawned must precede Exited, even for a very short command.
            while !monitor_exit_reported.load(Ordering::SeqCst) {
                let completion = session_for_monitor
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .poll_exit();

                let completion = match completion {
                    Ok(completion) => completion,
                    Err(error) => {
                        error!(terminal_id=?id, %error, "Terminal completion failed; status unknown");
                        Some(None)
                    }
                };
                if let Some(code) = completion.filter(|_| frames_drained.load(Ordering::SeqCst)) {
                    let mut view = lock_view_model(exit_view.as_ref(), id, "set_exited");
                    view.set_exited();
                    #[cfg(feature = "terminal-emulator-core")]
                    view.clear_input_sender();
                    drop(view);
                    input_senders
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&id);
                    if !monitor_exit_reported.swap(true, Ordering::SeqCst) {
                        publish(
                            &event_sink,
                            TerminalEvent::Exited {
                                id,
                                code,
                                signal: None,
                            },
                        );
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
        let input_handle = session_arc
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .input_handle();
        let input_exited = exit_reported.clone();
        let input_task = std::thread::spawn(move || {
            while !input_exited.load(Ordering::SeqCst) {
                match rx_input.recv_timeout(Duration::from_millis(100)) {
                    Ok(bytes) => {
                        let _ = input_handle.write_sync(&bytes);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        // Serialize PTY resize/cancel away from GPUI. Hidden hosts send no resize.
        let (control_tx, control_rx) = std::sync::mpsc::channel();
        let control_session = session_arc.clone();
        let control_exited = exit_reported.clone();
        std::thread::spawn(move || {
            while !control_exited.load(Ordering::SeqCst) {
                let command = match control_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(command) => command,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let mut session = control_session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match command {
                    SessionControl::Resize(size) => {
                        #[cfg(feature = "terminal-emulator-core")]
                        let _ = session.control_sender().send(ControlMsg::Resize {
                            cols: size.cols,
                            rows: size.rows,
                            cell_width: size.cell_width,
                            cell_height: size.cell_height,
                        });
                        let _ = futures_executor::block_on(session.resize(size.cols, size.rows));
                    }
                    SessionControl::Terminate => {
                        let _ = futures_executor::block_on(session.kill());
                        break;
                    }
                }
            }
        });
        for bytes in self.pending_input.remove(&id).unwrap_or_default() {
            let _ = tx.send(bytes);
        }

        // Register sender in shared map so UI thread can bypass the event queue
        if let Ok(mut senders) = self.input_senders.lock() {
            senders.insert(id, tx.clone());
        }

        self.sessions.insert(
            id,
            SessionEntry {
                rx_task: handle,
                exit_task,
                exit_reported,
                input_tx: tx,
                input_task,
                control_tx,
                view,
                last_size: None,
                #[cfg(feature = "terminal-emulator-core")]
                last_bounds: None,
            },
        );
        self.apply_pending_resize(id);
        info!(terminal_id=?id, "Terminal session spawned and consumer started");
        ready
    }

    fn handle_resize(
        &mut self,
        id: TerminalId,
        cols: u16,
        rows: u16,
        cell_width: f32,
        cell_height: f32,
    ) {
        if self.closed.contains(&id)
            || cols == 0
            || rows == 0
            || !cell_width.is_finite()
            || cell_width <= 0.0
            || !cell_height.is_finite()
            || cell_height <= 0.0
        {
            return;
        }
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
                let _ = entry
                    .control_tx
                    .send(SessionControl::Resize(PendingTerminalResize {
                        cols,
                        rows,
                        cell_width,
                        cell_height,
                    }));
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
                let _ = entry
                    .control_tx
                    .send(SessionControl::Resize(PendingTerminalResize {
                        cols,
                        rows,
                        cell_width,
                        cell_height,
                    }));
                entry.last_size = Some((cols, rows));
            }
        }
    }

    fn handle_event(&mut self, event: &TerminalEvent) {
        match event {
            TerminalEvent::SpawnRequested { .. }
            | TerminalEvent::CommandSpawnRequested { .. }
            | TerminalEvent::ServiceSpawnRequested { .. }
            | TerminalEvent::ServiceCommandSpawnRequested { .. } => {
                unreachable!("service terminal events are intercepted by TerminalRuntimeHandle")
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
                self.queue_input(*id, bytes.clone());
            }
            TerminalEvent::TerminateRequested { id } => {
                self.closed.insert(*id);
                if let Ok(mut senders) = self.input_senders.lock() {
                    senders.remove(id);
                }
                self.pending_resizes.remove(id);
                self.pending_input.remove(id);
                self.models.remove(id);
                nucleotide_terminal_view::unregister_view_model(*id);
                if let Some(entry) = self.sessions.remove(id) {
                    #[cfg(feature = "terminal-emulator-core")]
                    if let Ok(mut view) = entry.view.lock() {
                        view.clear_input_sender();
                    }
                    drop(entry.input_tx);
                    if !entry.exit_reported.load(Ordering::SeqCst) {
                        let _ = entry.control_tx.send(SessionControl::Terminate);
                    }
                }
            }
            TerminalEvent::Spawned { .. }
            | TerminalEvent::SpawnFailed { .. }
            | TerminalEvent::Exited { .. } => {}
        }
    }
}
impl Default for TerminalRuntimeHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TerminalRuntimeHandler {
    fn drop(&mut self) {
        for entry in self.sessions.values() {
            if !entry.exit_reported.load(Ordering::SeqCst) {
                let _ = entry.control_tx.send(SessionControl::Terminate);
            }
        }
        for id in self.models.keys() {
            nucleotide_terminal_view::unregister_view_model(*id);
        }
    }
}

#[cfg(all(test, feature = "terminal-emulator-core"))]
#[path = "terminal_handler_tests.rs"]
mod lifecycle_tests;

#[cfg(all(test, feature = "terminal-emulator-core"))]
mod tests {
    use super::*;

    struct UnusedLauncher;

    impl nucleotide_remote::RemotePtySessionLauncher for UnusedLauncher {
        fn launch(
            &self,
            _request: nucleotide_remote::RemotePtySessionRequest,
        ) -> Result<nucleotide_remote::RemotePtySessionParts, nucleotide_remote::RemoteClientError>
        {
            panic!("capability tests must not launch a session")
        }
    }

    fn test_remote_provider() -> RemoteTerminalSessionProvider {
        RemoteTerminalSessionProvider {
            display_root: PathBuf::from("remote-workspace"),
            native_root: PathBuf::from("/workspace"),
            launcher: Arc::new(UnusedLauncher),
        }
    }

    #[test]
    fn remote_provider_requires_workspace_and_cwd_mapping() {
        let provider = test_remote_provider();

        assert!(provider.handles(
            Path::new("remote-workspace"),
            Path::new("remote-workspace/src")
        ));
        assert!(!provider.handles(Path::new("remote-workspace"), Path::new("other-workspace")));
        assert_eq!(
            provider
                .native_path(Path::new("remote-workspace/src"))
                .unwrap(),
            PathBuf::from("/workspace/src")
        );
        assert!(provider.native_path(Path::new("other-workspace")).is_err());
    }

    #[test]
    fn runtime_provider_can_be_installed_and_cleared() {
        let runtime = TerminalRuntimeHandle::new();
        runtime.set_remote_provider(Some(test_remote_provider()));
        assert!(runtime.has_remote_terminal_provider_for(Path::new("remote-workspace")));
        assert!(runtime.handles_remote_terminal(
            Path::new("remote-workspace"),
            Path::new("remote-workspace/src")
        ));

        runtime.set_remote_provider(None);
        assert!(!runtime.has_remote_terminal_provider_for(Path::new("remote-workspace")));
    }

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
