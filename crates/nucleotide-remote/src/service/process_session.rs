// ABOUTME: Hardened service-side implementation of the v5 full-duplex process session RPC
// ABOUTME: Validates spawn state, contains descendants, and decouples process I/O from dispatch

use super::*;

pub(crate) enum V5ProcessSessionInput {
    Data { body: Vec<u8>, credit: u64 },
    End,
    Reset,
}

pub(crate) struct V5ProcessSession {
    pub(crate) input: mpsc::SyncSender<V5ProcessSessionInput>,
    pub(crate) cancellation: WorkspaceCancellationToken,
    eof: Arc<AtomicBool>,
    pub(crate) finished: bool,
    pub(crate) peer_ended: bool,
}

pub(crate) struct V5ProcessSessionRegistry {
    sessions: HashMap<u64, V5ProcessSession>,
}

impl V5ProcessSessionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.sessions.len()
    }
    pub(crate) fn insert(&mut self, id: u64, session: V5ProcessSession) {
        self.sessions.insert(id, session);
    }
    pub(crate) fn get(&self, id: u64) -> Option<&V5ProcessSession> {
        self.sessions.get(&id)
    }
    pub(crate) fn get_mut(&mut self, id: u64) -> Option<&mut V5ProcessSession> {
        self.sessions.get_mut(&id)
    }
    pub(crate) fn install_tombstone(&mut self, id: u64) {
        let (input, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        self.sessions.insert(
            id,
            V5ProcessSession {
                input,
                cancellation: WorkspaceCancellationToken::new(),
                eof: Arc::new(AtomicBool::new(false)),
                finished: true,
                peer_ended: false,
            },
        );
    }
    pub(crate) fn mark_peer_ended(&mut self, id: u64) {
        if let Some(session) = self.sessions.get_mut(&id) {
            session.peer_ended = true;
            session.eof.store(true, Ordering::Release);
            let _ = session.input.try_send(V5ProcessSessionInput::End);
        }
    }
    pub(crate) fn remove(&mut self, id: u64) {
        if let Some(session) = self.sessions.remove(&id) {
            session.cancellation.cancel();
            let _ = session.input.try_send(V5ProcessSessionInput::Reset);
        }
    }
}

impl Drop for V5ProcessSessionRegistry {
    fn drop(&mut self) {
        for (_, session) in self.sessions.drain() {
            session.cancellation.cancel();
            let _ = session.input.try_send(V5ProcessSessionInput::Reset);
        }
    }
}

impl<B: WorkspaceBackend> WorkspaceService<B> {
    pub(crate) fn start_v5_process_session<'scope>(
        &'scope self,
        scope: &'scope std::thread::Scope<'scope, '_>,
        stream_id: u64,
        priority: protocol_v5::Priority,
        payload: Vec<u8>,
        output: V5ServeOutputSender,
    ) -> std::result::Result<V5ProcessSession, RemoteError> {
        if payload.is_empty() || payload.len() > protocol_v5::MAX_INLINE_REQUEST_PAYLOAD_LEN {
            return Err(session_error(
                "invalid_request",
                "process.session requires a bounded inline HEADERS payload",
            ));
        }
        let request: ProcessSessionRequest = serde_json::from_slice(&payload).map_err(|error| {
            session_error(
                "invalid_request",
                format!("invalid process.session payload: {error}"),
            )
        })?;
        let (input, input_rx) = mpsc::sync_channel(V5_PROCESS_SESSION_INGRESS_CAPACITY);
        let cancellation = WorkspaceCancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let eof = Arc::new(AtomicBool::new(false));
        let worker_eof = Arc::clone(&eof);
        scope.spawn(move || {
            self.run_v5_process_session(
                stream_id,
                priority,
                request,
                input_rx,
                output,
                worker_cancellation,
                worker_eof,
            )
        });
        Ok(V5ProcessSession {
            input,
            cancellation,
            eof,
            finished: false,
            peer_ended: false,
        })
    }

    fn run_v5_process_session(
        &self,
        stream_id: u64,
        priority: protocol_v5::Priority,
        request: ProcessSessionRequest,
        input: mpsc::Receiver<V5ProcessSessionInput>,
        output: V5ServeOutputSender,
        cancellation: WorkspaceCancellationToken,
        eof: Arc<AtomicBool>,
    ) {
        let result =
            self.prepare_v5_process_session(&request)
                .and_then(|(program, cwd, env)| {
                    if cancellation.is_cancelled() {
                        return Err(session_error("cancelled", "process.session was reset"));
                    }
                    let mut command = nucleotide_process::contained_command(program);
                    command
                        .args(&request.args)
                        .current_dir(&cwd)
                        .env_clear()
                        .envs(&env)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                    let mut child = nucleotide_process::ContainedChild::spawn(&mut command)
                        .map_err(|error| {
                            session_error(
                                "spawn_failed",
                                format!("failed to spawn process.session: {error}"),
                            )
                        })?;
                    let stdin = child.child_mut().stdin.take().ok_or_else(|| {
                        session_error("spawn_failed", "child stdin was not piped")
                    })?;
                    let stdout = child.child_mut().stdout.take().ok_or_else(|| {
                        session_error("spawn_failed", "child stdout was not piped")
                    })?;
                    let stderr = child.child_mut().stderr.take().ok_or_else(|| {
                        session_error("spawn_failed", "child stderr was not piped")
                    })?;

                    let (pipe_tx, pipe_rx) = mpsc::sync_channel(16);
                    spawn_session_pipe(stdout, protocol_v5::DataChannel::Stdout, pipe_tx.clone());
                    spawn_session_pipe(stderr, protocol_v5::DataChannel::Stderr, pipe_tx);
                    let (stdin_done_tx, stdin_done_rx) = mpsc::channel();
                    let stdin_stop = Arc::new(AtomicBool::new(false));
                    let worker_stop = Arc::clone(&stdin_stop);
                    let stdin_worker = std::thread::spawn(move || {
                        session_stdin_loop(stdin, input, stdin_done_tx, eof, worker_stop)
                    });
                    let mut open_pipes = 2;
                    let status = loop {
                        if cancellation.is_cancelled() {
                            let _ = child.terminate();
                            let _ = child.wait();
                            stdin_stop.store(true, Ordering::Release);
                            let _ = stdin_worker.join();
                            return Err(session_error("cancelled", "process.session was reset"));
                        }
                        while let Ok(event) = stdin_done_rx.try_recv() {
                            match event {
                                SessionStdinResult::Settled { bytes, disposition } => {
                                    let _ = output.send_with_cancellation(
                                        V5ServeOutputEvent::SessionInputSettled {
                                            stream_id,
                                            bytes,
                                            disposition,
                                            priority,
                                        },
                                        &cancellation,
                                    );
                                }
                                SessionStdinResult::Reset => {
                                    let _ = child.terminate();
                                    let _ = child.wait();
                                    stdin_stop.store(true, Ordering::Release);
                                    let _ = stdin_worker.join();
                                    return Err(session_error(
                                        "cancelled",
                                        "process.session was reset",
                                    ));
                                }
                                SessionStdinResult::Closed => {}
                            }
                        }
                        while let Ok(event) = pipe_rx.try_recv() {
                            match event {
                                SessionPipeResult::Data(channel, body) => {
                                    let _ = output.send_with_cancellation(
                                        V5ServeOutputEvent::SessionStreamData {
                                            stream_id,
                                            channel,
                                            body,
                                            priority,
                                        },
                                        &cancellation,
                                    );
                                }
                                SessionPipeResult::Closed => open_pipes -= 1,
                            }
                        }
                        match child.child_mut().try_wait() {
                            Ok(Some(status)) if open_pipes == 0 => break status,
                            Ok(Some(_)) => {
                                // Descendants must not keep inherited pipes alive.
                                let _ = child.terminate();
                            }
                            Ok(None) => {}
                            Err(error) => {
                                stdin_stop.store(true, Ordering::Release);
                                let _ = stdin_worker.join();
                                return Err(session_error("process_failed", error.to_string()));
                            }
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    };
                    stdin_stop.store(true, Ordering::Release);
                    let _ = stdin_worker.join();
                    while let Ok(event) = stdin_done_rx.try_recv() {
                        if let SessionStdinResult::Settled { bytes, disposition } = event {
                            let _ = output.send_with_cancellation(
                                V5ServeOutputEvent::SessionInputSettled {
                                    stream_id,
                                    bytes,
                                    disposition,
                                    priority,
                                },
                                &cancellation,
                            );
                        }
                    }
                    Ok(ProcessSessionCompletion {
                        status_code: status.code(),
                        success: status.success(),
                    })
                });
        send_session_terminal(&output, stream_id, priority, result, &cancellation);
    }

    fn prepare_v5_process_session(
        &self,
        request: &ProcessSessionRequest,
    ) -> std::result::Result<(PathBuf, PathBuf, BTreeMap<String, String>), RemoteError> {
        if request.program.is_empty() || request.program.contains('\0') {
            return Err(session_error("invalid_request", "invalid process program"));
        }
        let cwd = self.resolve_path(&request.cwd)?;
        if !cwd.is_dir() {
            return Err(session_error(
                "invalid_request",
                "process cwd is not a directory",
            ));
        }
        let env_root = match &request.environment {
            ProjectEnvironmentSelection::Bare => None,
            ProjectEnvironmentSelection::Exact { root } => Some(self.resolve_path(root)?),
            ProjectEnvironmentSelection::WorkspaceOrNearest { anchor } => {
                let mut root = self.resolve_path(anchor)?;
                if root.is_file() {
                    root.pop();
                }
                loop {
                    if root.join(".envrc").is_file() {
                        break;
                    }
                    if root == self.workspace_root || !root.pop() {
                        root = self.workspace_root.clone();
                        break;
                    }
                }
                Some(root)
            }
        };
        let mut env: BTreeMap<String, String> = match env_root {
            None => self.environment_baseline.clone().into_iter().collect(),
            Some(root) => {
                self.load_project_environment(&root)
                    .map_err(remote_error_from_environment)?
                    .variables
            }
        };
        for (key, value) in &request.env_overrides {
            if !v5_process_environment_entry_is_valid(key, value) {
                return Err(session_error(
                    "invalid_request",
                    format!("invalid environment entry {key:?}"),
                ));
            }
            insert_session_environment(&mut env, key.clone(), value.clone());
        }
        if env
            .iter()
            .any(|(key, value)| !v5_process_environment_entry_is_valid(key, value))
        {
            return Err(session_error(
                "invalid_request",
                "project environment contains an invalid entry",
            ));
        }
        insert_session_environment(
            &mut env,
            "PWD".to_string(),
            cwd.to_string_lossy().into_owned(),
        );
        if let Some(path) = session_environment_value_mut(&mut env, "PATH") {
            *path = resolve_session_path(path, &cwd);
        }
        let program_path = Path::new(&request.program);
        let program = if program_path.components().count() > 1 || program_path.is_absolute() {
            normalize_path_lexically(&if program_path.is_absolute() {
                program_path.to_path_buf()
            } else {
                cwd.join(program_path)
            })
        } else {
            resolve_bare_program(
                program_path,
                session_environment_value(&env, "PATH"),
                session_environment_value(&env, "PATHEXT"),
                &cwd,
            )
        };
        Ok((program, cwd, env))
    }
}

enum SessionPipeResult {
    Data(protocol_v5::DataChannel, Vec<u8>),
    Closed,
}
enum SessionStdinResult {
    Settled {
        bytes: u64,
        disposition: SessionInputDisposition,
    },
    Reset,
    Closed,
}

fn spawn_session_pipe(
    mut pipe: impl Read + Send + 'static,
    channel: protocol_v5::DataChannel,
    sender: mpsc::SyncSender<SessionPipeResult>,
) {
    std::thread::spawn(move || {
        let mut buf = [0; 8192];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if sender
                        .send(SessionPipeResult::Data(channel, buf[..n].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
        let _ = sender.send(SessionPipeResult::Closed);
    });
}

fn session_stdin_loop(
    mut stdin: ChildStdin,
    input: mpsc::Receiver<V5ProcessSessionInput>,
    done: mpsc::Sender<SessionStdinResult>,
    eof: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) {
    let mut discard = false;
    loop {
        let message = match input.recv_timeout(Duration::from_millis(2)) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout)
                if eof.load(Ordering::Acquire) || stop.load(Ordering::Acquire) =>
            {
                drop(stdin);
                let _ = done.send(SessionStdinResult::Closed);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        match message {
            V5ProcessSessionInput::Data { body, credit } => {
                let disposition = if !discard && stdin.write_all(&body).is_ok() {
                    SessionInputDisposition::Consumed
                } else {
                    discard = true;
                    SessionInputDisposition::Discarded
                };
                let _ = done.send(SessionStdinResult::Settled {
                    bytes: credit,
                    disposition,
                });
            }
            V5ProcessSessionInput::End => {
                drop(stdin);
                let _ = done.send(SessionStdinResult::Closed);
                return;
            }
            V5ProcessSessionInput::Reset => {
                let _ = done.send(SessionStdinResult::Reset);
                return;
            }
        }
    }
}

fn resolve_session_path(path: &str, cwd: &Path) -> String {
    std::env::split_paths(OsStr::new(path))
        .map(|entry| {
            if entry.is_absolute() {
                entry
            } else {
                cwd.join(entry)
            }
        })
        .collect::<Vec<_>>()
        .iter()
        .fold(OsString::new(), |mut joined, entry| {
            if !joined.is_empty() {
                joined.push(if cfg!(windows) { ";" } else { ":" });
            }
            joined.push(entry);
            joined
        })
        .to_string_lossy()
        .into_owned()
}
fn resolve_bare_program(
    program: &Path,
    path: Option<&String>,
    pathext: Option<&String>,
    cwd: &Path,
) -> PathBuf {
    path.and_then(|path| {
        std::env::split_paths(path)
            .flat_map(|dir| executable_candidates(dir, program, pathext))
            .find(|candidate| candidate.is_file())
    })
    .unwrap_or_else(|| cwd.join(program))
}

fn executable_candidates(dir: PathBuf, program: &Path, pathext: Option<&String>) -> Vec<PathBuf> {
    let direct = dir.join(program);
    #[cfg(windows)]
    {
        if program.extension().is_some() {
            return vec![direct];
        }
        let mut candidates = vec![direct];
        for extension in pathext
            .map_or(".COM;.EXE;.BAT;.CMD", String::as_str)
            .split(';')
        {
            if !extension.is_empty() {
                candidates.push(dir.join(format!("{}{}", program.to_string_lossy(), extension)));
            }
        }
        candidates
    }
    #[cfg(not(windows))]
    {
        let _ = pathext;
        vec![direct]
    }
}

fn session_environment_value<'a>(
    env: &'a BTreeMap<String, String>,
    key: &str,
) -> Option<&'a String> {
    #[cfg(windows)]
    return env
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value);
    #[cfg(not(windows))]
    env.get(key)
}

fn session_environment_value_mut<'a>(
    env: &'a mut BTreeMap<String, String>,
    key: &str,
) -> Option<&'a mut String> {
    #[cfg(windows)]
    return env
        .iter_mut()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value);
    #[cfg(not(windows))]
    env.get_mut(key)
}

fn insert_session_environment(env: &mut BTreeMap<String, String>, key: String, value: String) {
    #[cfg(windows)]
    if let Some(existing) = env
        .keys()
        .find(|candidate| candidate.eq_ignore_ascii_case(&key))
        .cloned()
    {
        env.remove(&existing);
    }
    env.insert(key, value);
}
fn session_error(code: impl Into<String>, message: impl Into<String>) -> RemoteError {
    RemoteError {
        code: code.into(),
        message: message.into(),
        diagnostic: None,
    }
}
fn send_session_terminal(
    output: &V5ServeOutputSender,
    stream_id: u64,
    priority: protocol_v5::Priority,
    result: std::result::Result<ProcessSessionCompletion, RemoteError>,
    cancellation: &WorkspaceCancellationToken,
) {
    let _ = output.send_with_cancellation(
        V5ServeOutputEvent::SessionTerminal {
            stream_id,
            priority,
            result,
        },
        cancellation,
    );
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn environment_override_and_executable_resolution_are_case_insensitive() {
        let temporary = tempfile::tempdir().unwrap();
        let bin = temporary.path().join("relative-bin");
        std::fs::create_dir(&bin).unwrap();
        let executable = bin.join("tool.EXE");
        std::fs::write(&executable, b"").unwrap();

        let mut env = BTreeMap::from([
            ("Path".to_string(), "old".to_string()),
            ("PATHEXT".to_string(), ".EXE;.CMD".to_string()),
        ]);
        insert_session_environment(&mut env, "PATH".to_string(), "relative-bin".to_string());
        assert_eq!(
            env.keys()
                .filter(|key| key.eq_ignore_ascii_case("path"))
                .count(),
            1
        );
        let path = resolve_session_path(
            session_environment_value(&env, "PATH").unwrap(),
            temporary.path(),
        );
        assert_eq!(
            resolve_bare_program(
                Path::new("tool"),
                Some(&path),
                session_environment_value(&env, "pathext"),
                temporary.path(),
            ),
            executable
        );
    }
}
