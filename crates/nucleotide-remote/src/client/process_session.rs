// ABOUTME: Owned asynchronous process-session adapters for the multiplexed v5 client
// ABOUTME: Splits stdin and output while retaining protocol flow credit until reads consume it

use super::*;
use prost::Message;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const PROCESS_SESSION_METHOD: &str = "process.session";
const PTY_SESSION_METHOD: &str = "pty.session";
const PROCESS_SESSION_STDIN_LIMIT: usize = 256 * 1024;

/// Protocol-independent input used by application code to start a piped process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePipedProcessSessionRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Used only to select the workspace/project environment; it need not equal `cwd`.
    pub environment_anchor: PathBuf,
    /// Command-specific additions or replacements. The caller must not copy the full environment.
    pub env_overrides: BTreeMap<String, String>,
}

pub type RemoteProcessSessionReader = Pin<Box<dyn AsyncRead + Send>>;
pub type RemoteProcessSessionWriter = Pin<Box<dyn AsyncWrite + Send>>;
pub type RemoteProcessSessionCompletionFuture = Pin<
    Box<
        dyn Future<Output = std::result::Result<ProcessSessionCompletion, RemoteClientError>>
            + Send,
    >,
>;

/// Owned pieces of a session. Keeping `lifecycle` alive keeps the process alive; dropping it
/// cancels the process and its descendants. Completion and the two readers may be consumed
/// independently.
pub struct RemotePipedProcessSessionParts {
    pub stdin: RemoteProcessSessionWriter,
    pub stdout: RemoteProcessSessionReader,
    pub stderr: RemoteProcessSessionReader,
    pub completion: RemoteProcessSessionCompletionFuture,
    pub lifecycle: Box<dyn Send>,
}

/// Object-safe entry point exposed by workspace bootstrap when the peer negotiated
/// `process_sessions_v1`.
pub trait RemotePipedProcessSessionLauncher: Send + Sync {
    fn launch(
        &self,
        request: RemotePipedProcessSessionRequest,
    ) -> std::result::Result<RemotePipedProcessSessionParts, RemoteClientError>;
}

/// Protocol-independent PTY launch request. Unlike a process session, output is a single
/// terminal byte stream and the service owns the pseudo-terminal and child lifecycle.
pub type RemotePtySessionRequest = PtySessionRequest;

pub struct RemotePtySessionParts {
    pub input: RemoteProcessSessionWriter,
    pub output: RemoteProcessSessionReader,
    pub completion: RemoteProcessSessionCompletionFuture,
    pub control: Box<dyn RemotePtySessionControl>,
    /// Keeps both the protocol stream guard and the exact reconnecting transport alive until
    /// terminal teardown. Dropping this guard cancels the remote PTY.
    pub lifecycle: Box<dyn Send>,
}

/// Synchronous, object-safe controls which can be called by a terminal UI thread. `wait` reports
/// child exit, but PTY output may still be buffered; callers must read `output` to EOF before
/// treating all output as delivered. Cancellation kills and reaps the portable-pty child/process
/// group where supported. A fully daemonized descendant which creates a new process group/session
/// can escape that containment; teardown remains bounded and never waits for its inherited slave.
pub trait RemotePtySessionControl: Send + Sync {
    fn resize(
        &self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> std::result::Result<(), RemoteClientError>;
    fn cancel(&self);
}

pub trait RemotePtySessionLauncher: Send + Sync {
    fn launch(
        &self,
        request: RemotePtySessionRequest,
    ) -> std::result::Result<RemotePtySessionParts, RemoteClientError>;
}

pub(crate) struct RemoteV5ProcessSessionLauncher {
    client: Arc<RemoteWorkspaceV5ReconnectingClient>,
}

struct RemoteV5ProcessSessionLifecycle {
    _guard: RemoteProcessSessionGuard<ChildProcessV5Writer>,
    // RemoteProcessSession uses a weak transport reference. Retain the exact client shared with
    // WorkspaceBackendConnection so extracting session parts cannot close the backend early.
    _client: Arc<RemoteWorkspaceV5ReconnectingClient>,
}

struct RemoteV5PtyControl {
    core: Arc<V5ProcessSessionCore<ChildProcessV5Writer>>,
    _client: Arc<RemoteWorkspaceV5ReconnectingClient>,
}

impl RemotePtySessionControl for RemoteV5PtyControl {
    fn resize(
        &self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> std::result::Result<(), RemoteClientError> {
        self.core
            .send_pty_resize(cols, rows, pixel_width, pixel_height)
    }
    fn cancel(&self) {
        self.core.cancel("PTY session cancelled");
    }
}

impl RemoteV5ProcessSessionLauncher {
    pub(crate) fn new(client: Arc<RemoteWorkspaceV5ReconnectingClient>) -> Self {
        Self { client }
    }
}

impl RemotePipedProcessSessionLauncher for RemoteV5ProcessSessionLauncher {
    fn launch(
        &self,
        request: RemotePipedProcessSessionRequest,
    ) -> std::result::Result<RemotePipedProcessSessionParts, RemoteClientError> {
        let session = self.client.open_process_session(ProcessSessionRequest {
            program: request.program,
            args: request.args,
            cwd: request.cwd,
            environment: ProjectEnvironmentSelection::WorkspaceOrNearest {
                anchor: request.environment_anchor,
            },
            env_overrides: request.env_overrides,
        })?;
        Ok(RemotePipedProcessSessionParts {
            stdin: Box::pin(session.stdin),
            stdout: Box::pin(session.stdout),
            stderr: Box::pin(session.stderr),
            completion: Box::pin(session.completion),
            lifecycle: Box::new(RemoteV5ProcessSessionLifecycle {
                _guard: session.lifecycle,
                _client: Arc::clone(&self.client),
            }),
        })
    }
}

impl RemotePtySessionLauncher for RemoteV5ProcessSessionLauncher {
    fn launch(
        &self,
        request: RemotePtySessionRequest,
    ) -> std::result::Result<RemotePtySessionParts, RemoteClientError> {
        let session = self.client.open_pty_session(request)?;
        let control = RemoteV5PtyControl {
            core: Arc::clone(&session.lifecycle.core),
            _client: Arc::clone(&self.client),
        };
        Ok(RemotePtySessionParts {
            input: Box::pin(session.stdin),
            output: Box::pin(session.stdout),
            completion: Box::pin(session.completion),
            control: Box::new(control),
            lifecycle: Box::new(RemoteV5ProcessSessionLifecycle {
                _guard: session.lifecycle,
                _client: Arc::clone(&self.client),
            }),
        })
    }
}

pub struct RemoteProcessSession<W> {
    pub stdin: RemoteProcessSessionStdin<W>,
    pub stdout: RemoteProcessSessionOutput<W>,
    pub stderr: RemoteProcessSessionOutput<W>,
    pub completion: RemoteProcessSessionCompletion,
    pub lifecycle: RemoteProcessSessionGuard<W>,
}

pub struct RemoteProcessSessionStdin<W> {
    core: Arc<V5ProcessSessionCore<W>>,
}
pub struct RemoteProcessSessionOutput<W> {
    core: Arc<V5ProcessSessionCore<W>>,
    stderr: bool,
}
pub struct RemoteProcessSessionCompletion {
    mailbox: Arc<V5ProcessSessionMailbox>,
}
pub struct RemoteProcessSessionGuard<W> {
    core: Arc<V5ProcessSessionCore<W>>,
}

pub(crate) struct V5PendingProcessSession {
    pub(crate) mailbox: Arc<V5ProcessSessionMailbox>,
    pub(crate) payload: Vec<u8>,
    pub(crate) final_error: Option<RemoteError>,
}
pub(crate) struct V5ProcessSessionCore<W> {
    shared: Weak<RemoteWorkspaceV5Shared<W>>,
    mailbox: Arc<V5ProcessSessionMailbox>,
    stream_id: u64,
    ended: AtomicBool,
    reset: AtomicBool,
}

impl<W: Write> V5ProcessSessionCore<W> {
    fn send_pty_resize(
        &self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> std::result::Result<(), RemoteClientError> {
        let body = protocol_v5::PtyControl {
            resize: Some(protocol_v5::PtyResize {
                cols: cols.into(),
                rows: rows.into(),
                pixel_width: pixel_width.into(),
                pixel_height: pixel_height.into(),
            }),
        }
        .encode_to_vec();
        let shared = self
            .shared
            .upgrade()
            .ok_or(RemoteClientError::Disconnected)?;
        shared
            .session
            .lock()
            .map_err(v5_client_lock_error)?
            .send_owned_data(
                self.stream_id,
                protocol_v5::DataChannel::PtyControl,
                body,
                protocol_v5::Priority::UserInput,
            )?;
        wake_v5_client_writer(&shared)
    }
    fn cancel(&self, reason: &str) {
        if self.reset.swap(true, Ordering::AcqRel) {
            return;
        }
        // RESET has no response guarantee (especially after connection loss). Settle all local
        // readers and completion immediately rather than depending on the peer.
        self.mailbox.fail(RemoteClientError::TransportClosed {
            cause: reason.to_string(),
        });
        if let Some(shared) = self.shared.upgrade() {
            if let Ok(mut session) = shared.session.lock() {
                let _ = session.reset_stream(
                    self.stream_id,
                    protocol_v5::RESET_CANCELLED,
                    reason.to_string(),
                );
            }
            let _ = wake_v5_client_writer(&shared);
        }
    }
}
pub(crate) struct V5ProcessSessionMailbox {
    state: Mutex<V5ProcessSessionState>,
    stdout_waker: AtomicWaker,
    stderr_waker: AtomicWaker,
    completion_waker: AtomicWaker,
    stdin_waker: AtomicWaker,
}
struct V5ProcessSessionState {
    stdout: VecDeque<V5SessionChunk>,
    stderr: VecDeque<V5SessionChunk>,
    completion: Option<std::result::Result<ProcessSessionCompletion, String>>,
    terminal: bool,
    error: Option<String>,
    pending_stdin_bytes: usize,
    next_stdin_generation: u64,
    flushed_stdin_generation: u64,
    pending_stdin: VecDeque<(u64, usize)>,
}
struct V5SessionChunk {
    bytes: Vec<u8>,
    offset: usize,
}

impl V5ProcessSessionMailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(V5ProcessSessionState {
                stdout: VecDeque::new(),
                stderr: VecDeque::new(),
                completion: None,
                terminal: false,
                error: None,
                pending_stdin_bytes: 0,
                next_stdin_generation: 0,
                flushed_stdin_generation: 0,
                pending_stdin: VecDeque::new(),
            }),
            stdout_waker: AtomicWaker::new(),
            stderr_waker: AtomicWaker::new(),
            completion_waker: AtomicWaker::new(),
            stdin_waker: AtomicWaker::new(),
        }
    }
    pub(crate) fn output(&self, stderr: bool, body: Vec<u8>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let queue = if stderr {
            &mut state.stderr
        } else {
            &mut state.stdout
        };
        queue.push_back(V5SessionChunk {
            bytes: body,
            offset: 0,
        });
        drop(state);
        if stderr {
            self.stderr_waker.wake();
        } else {
            self.stdout_waker.wake();
        }
    }
    pub(crate) fn complete(
        &self,
        result: std::result::Result<ProcessSessionCompletion, RemoteClientError>,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.completion = Some(result.map_err(|e| e.to_string()));
        state.terminal = true;
        drop(state);
        self.wake_all();
    }
    pub(crate) fn fail(&self, error: RemoteClientError) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.error = Some(error.to_string());
        state.terminal = true;
        drop(state);
        self.wake_all();
    }
    pub(crate) fn terminal(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .terminal
    }

    pub(crate) fn wake(&self) {
        self.stdin_waker.wake();
    }
    fn wake_all(&self) {
        self.stdout_waker.wake();
        self.stderr_waker.wake();
        self.completion_waker.wake();
        self.stdin_waker.wake();
    }
    pub(crate) fn stdin_physically_flushed(&self, mut bytes: usize) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pending_stdin_bytes = state.pending_stdin_bytes.saturating_sub(bytes);
        while bytes != 0 {
            let Some((generation, remaining)) = state.pending_stdin.front_mut() else {
                break;
            };
            let consumed = bytes.min(*remaining);
            *remaining -= consumed;
            bytes -= consumed;
            if *remaining == 0 {
                let generation = *generation;
                state.pending_stdin.pop_front();
                state.flushed_stdin_generation = generation;
            }
        }
        drop(state);
        self.stdin_waker.wake();
    }
}

fn session_io(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, message)
}

impl<W: Write> AsyncWrite for RemoteProcessSessionStdin<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.core.ended.load(Ordering::Acquire) || self.core.mailbox.terminal() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "process session stdin is closed",
            )));
        }
        let Some(shared) = self.core.shared.upgrade() else {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "remote transport disconnected",
            )));
        };
        let accepted = {
            let state = self
                .core
                .mailbox
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if state.pending_stdin_bytes >= PROCESS_SESSION_STDIN_LIMIT {
                self.core.mailbox.stdin_waker.register(cx.waker());
                return Poll::Pending;
            }
            buf.len()
                .min(PROCESS_SESSION_STDIN_LIMIT - state.pending_stdin_bytes)
        };
        let result = shared
            .session
            .lock()
            .map_err(v5_client_lock_error)
            .and_then(|mut session| {
                session
                    .send_owned_data(
                        self.core.stream_id,
                        protocol_v5::DataChannel::Stdin,
                        buf[..accepted].to_vec(),
                        protocol_v5::Priority::UserInput,
                    )
                    .map_err(RemoteClientError::Io)?;

                // Account for the accepted frame before releasing the protocol-session lock.
                // The writer must acquire that lock before it can pop the frame, so a physical
                // flush can never overtake generation registration.
                let mut state = self
                    .core
                    .mailbox
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                state.next_stdin_generation += 1;
                let generation = state.next_stdin_generation;
                state.pending_stdin_bytes += accepted;
                state.pending_stdin.push_back((generation, accepted));
                Ok(())
            });
        match result {
            Ok(()) => match wake_v5_client_writer(&shared) {
                Ok(()) => Poll::Ready(Ok(accepted)),
                Err(e) => Poll::Ready(Err(session_io(e.to_string()))),
            },
            Err(RemoteClientError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {
                self.core.mailbox.stdin_waker.register(cx.waker());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(session_io(e.to_string()))),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        let state = self
            .core
            .mailbox
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if state.flushed_stdin_generation >= state.next_stdin_generation {
            return Poll::Ready(Ok(()));
        }
        if let Some(error) = state.error.clone() {
            return Poll::Ready(Err(session_io(error)));
        }
        if state.terminal {
            return Poll::Ready(Err(session_io(
                "process session ended before stdin was flushed".to_string(),
            )));
        }
        let transport_gone = self
            .core
            .shared
            .upgrade()
            .is_none_or(|shared| shared.closed.load(Ordering::Acquire));
        if transport_gone {
            return Poll::Ready(Err(session_io("remote transport disconnected".to_string())));
        }
        self.core.mailbox.stdin_waker.register(cx.waker());
        Poll::Pending
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        if self.core.mailbox.terminal() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "process session already completed",
            )));
        }
        if self.as_mut().poll_flush(cx).is_pending() {
            return Poll::Pending;
        }
        if self.core.ended.load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }
        let Some(shared) = self.core.shared.upgrade() else {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "remote transport disconnected",
            )));
        };
        let result = shared
            .session
            .lock()
            .map_err(v5_client_lock_error)
            .and_then(|mut session| {
                session
                    .finish_stream(self.core.stream_id, protocol_v5::Priority::UserInput)
                    .map(|_| ())
                    .map_err(RemoteClientError::Io)
            })
            .and_then(|_| wake_v5_client_writer(&shared));
        if result.is_ok() {
            self.core.ended.store(true, Ordering::Release);
        }
        Poll::Ready(result.map_err(|e| session_io(e.to_string())))
    }
}

impl<W> AsyncRead for RemoteProcessSessionOutput<W> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = self
            .core
            .mailbox
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let queue = if self.stderr {
            &mut state.stderr
        } else {
            &mut state.stdout
        };
        if let Some(chunk) = queue.front_mut() {
            let count = output.remaining().min(chunk.bytes.len() - chunk.offset);
            output.put_slice(&chunk.bytes[chunk.offset..chunk.offset + count]);
            chunk.offset += count;
            if chunk.offset == chunk.bytes.len() {
                queue.pop_front();
            }
            drop(state);
            if let Some(shared) = self.core.shared.upgrade() {
                if let Err(e) =
                    queue_v5_released_receive_credit(&shared, self.core.stream_id, count as u64)
                {
                    return Poll::Ready(Err(session_io(e.to_string())));
                }
            }
            return Poll::Ready(Ok(()));
        }
        if let Some(error) = state.error.clone() {
            return Poll::Ready(Err(session_io(error)));
        }
        if state.terminal {
            return Poll::Ready(Ok(()));
        }
        if self.stderr {
            self.core.mailbox.stderr_waker.register(cx.waker());
        } else {
            self.core.mailbox.stdout_waker.register(cx.waker());
        }
        Poll::Pending
    }
}

impl Future for RemoteProcessSessionCompletion {
    type Output = std::result::Result<ProcessSessionCompletion, RemoteClientError>;
    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let mut state = self.mailbox.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(result) = state.completion.take() {
            return Poll::Ready(
                result.map_err(|cause| RemoteClientError::TransportClosed { cause }),
            );
        }
        if let Some(error) = state.error.clone() {
            return Poll::Ready(Err(RemoteClientError::TransportClosed { cause: error }));
        }
        self.mailbox.completion_waker.register(cx.waker());
        Poll::Pending
    }
}

impl<W> Drop for RemoteProcessSessionGuard<W> {
    fn drop(&mut self) {
        if self.core.mailbox.terminal() {
            return;
        }
        if self.core.reset.swap(true, Ordering::AcqRel) {
            return;
        }
        self.core.mailbox.fail(RemoteClientError::TransportClosed {
            cause: "process session handle dropped".to_string(),
        });
        let Some(shared) = self.core.shared.upgrade() else {
            return;
        };
        if let Ok(mut session) = shared.session.lock() {
            let _ = session.reset_stream(
                self.core.stream_id,
                protocol_v5::RESET_CANCELLED,
                "process session handle dropped".to_string(),
            );
        }
        let _ = wake_v5_client_writer(&shared);
    }
}

impl<R, W> RemoteWorkspaceV5MultiplexedClient<R, W>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn open_process_session(
        &self,
        request: ProcessSessionRequest,
    ) -> std::result::Result<RemoteProcessSession<W>, RemoteClientError> {
        if !self
            .server_hello
            .capabilities
            .iter()
            .any(|capability| capability == "process_sessions_v1")
        {
            return Err(RemoteClientError::Protocol(
                "remote helper does not support process_sessions_v1".to_string(),
            ));
        }
        self.open_session(PROCESS_SESSION_METHOD, serde_json::to_vec(&request)?)
    }

    pub fn open_pty_session(
        &self,
        request: PtySessionRequest,
    ) -> std::result::Result<RemoteProcessSession<W>, RemoteClientError> {
        if !self
            .server_hello
            .capabilities
            .iter()
            .any(|capability| capability == "pty_sessions_v1")
        {
            return Err(RemoteClientError::Protocol(
                "remote helper does not support pty_sessions_v1".to_string(),
            ));
        }
        self.open_session(PTY_SESSION_METHOD, serde_json::to_vec(&request)?)
    }

    fn open_session(
        &self,
        method: &'static str,
        payload: Vec<u8>,
    ) -> std::result::Result<RemoteProcessSession<W>, RemoteClientError> {
        let mailbox = Arc::new(V5ProcessSessionMailbox::new());
        let stream_id = {
            let mut session = self.shared.session.lock().map_err(v5_client_lock_error)?;
            let stream_id = session.open_full_duplex_request(
                method,
                protocol_v5::RequestOptions::default(),
                payload,
            )?;
            let mut waiters = match self.shared.process_session_waiters.lock() {
                Ok(waiters) => waiters,
                Err(error) => {
                    let error = v5_client_lock_error(error);
                    let _ = session.reset_stream(
                        stream_id,
                        protocol_v5::RESET_CANCELLED,
                        "client could not register process session waiter",
                    );
                    return Err(error);
                }
            };
            waiters.insert(
                stream_id,
                V5PendingProcessSession {
                    mailbox: Arc::clone(&mailbox),
                    payload: Vec::new(),
                    final_error: None,
                },
            );
            stream_id
        };
        let core = Arc::new(V5ProcessSessionCore {
            shared: Arc::downgrade(&self.shared),
            mailbox: Arc::clone(&mailbox),
            stream_id,
            ended: AtomicBool::new(false),
            reset: AtomicBool::new(false),
        });
        wake_v5_client_writer(&self.shared)?;
        Ok(RemoteProcessSession {
            stdin: RemoteProcessSessionStdin {
                core: Arc::clone(&core),
            },
            stdout: RemoteProcessSessionOutput {
                core: Arc::clone(&core),
                stderr: false,
            },
            stderr: RemoteProcessSessionOutput {
                core: Arc::clone(&core),
                stderr: true,
            },
            completion: RemoteProcessSessionCompletion { mailbox },
            lifecycle: RemoteProcessSessionGuard { core },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Waker;

    fn stdin_with_outstanding_flush() -> RemoteProcessSessionStdin<Vec<u8>> {
        let mailbox = Arc::new(V5ProcessSessionMailbox::new());
        {
            let mut state = mailbox.state.lock().unwrap();
            state.pending_stdin_bytes = 4;
            state.next_stdin_generation = 1;
            state.pending_stdin.push_back((1, 4));
        }
        RemoteProcessSessionStdin {
            core: Arc::new(V5ProcessSessionCore {
                shared: Weak::new(),
                mailbox,
                stream_id: 1,
                ended: AtomicBool::new(false),
                reset: AtomicBool::new(false),
            }),
        }
    }

    #[test]
    fn process_session_flush_fails_after_accounted_frame_transport_failure() {
        let mut stdin = stdin_with_outstanding_flush();
        stdin.core.mailbox.fail(RemoteClientError::Disconnected);

        let mut cx = TaskContext::from_waker(Waker::noop());
        let Poll::Ready(Err(error)) = Pin::new(&mut stdin).poll_flush(&mut cx) else {
            panic!("outstanding flush must finish when writer transport fails");
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn process_session_flush_fails_when_transport_is_gone_with_accounted_frame() {
        let mut stdin = stdin_with_outstanding_flush();

        let mut cx = TaskContext::from_waker(Waker::noop());
        let Poll::Ready(Err(error)) = Pin::new(&mut stdin).poll_flush(&mut cx) else {
            panic!("outstanding flush must not wait after transport disappears");
        };
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
