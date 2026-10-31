// ABOUTME: Linux service-owned pseudo-terminal sessions for protocol v5
// ABOUTME: Spawns login shells or commands under portable-pty and bridges ordered input/control

use super::*;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use prost::Message;
use std::os::fd::{AsRawFd, BorrowedFd};

impl<B: WorkspaceBackend> WorkspaceService<B> {
    pub(crate) fn start_v5_pty_session<'scope>(
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
                "pty.session requires a bounded inline HEADERS payload",
            ));
        }
        let request: PtySessionRequest = serde_json::from_slice(&payload).map_err(|e| {
            session_error(
                "invalid_request",
                format!("invalid pty.session payload: {e}"),
            )
        })?;
        if request.cols == 0 || request.rows == 0 {
            return Err(session_error(
                "invalid_request",
                "PTY rows and columns must be non-zero",
            ));
        }
        let (input, input_rx) = mpsc::sync_channel(V5_PROCESS_SESSION_INGRESS_CAPACITY);
        let cancellation = WorkspaceCancellationToken::new();
        let worker_cancel = cancellation.clone();
        let eof = Arc::new(AtomicBool::new(false));
        let worker_eof = Arc::clone(&eof);
        scope.spawn(move || {
            self.run_v5_pty_session(
                stream_id,
                priority,
                request,
                input_rx,
                output,
                worker_cancel,
                worker_eof,
            )
        });
        Ok(V5ProcessSession {
            input,
            cancellation,
            eof,
            method: "pty.session",
            finished: false,
            peer_ended: false,
        })
    }

    fn run_v5_pty_session(
        &self,
        stream_id: u64,
        priority: protocol_v5::Priority,
        request: PtySessionRequest,
        input: mpsc::Receiver<V5ProcessSessionInput>,
        output: V5ServeOutputSender,
        cancellation: WorkspaceCancellationToken,
        eof: Arc<AtomicBool>,
    ) {
        let prepared = self.prepare_pty(&request, &cancellation);
        let result = prepared.and_then(|(mut command, cwd)| {
            if cancellation.is_cancelled() {
                return Err(session_error("cancelled", "pty.session was reset"));
            }
            command.cwd(cwd);
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: request.rows,
                    cols: request.cols,
                    pixel_width: request.pixel_width,
                    pixel_height: request.pixel_height,
                })
                .map_err(|e| session_error("spawn_failed", e.to_string()))?;
            let master_fd = pair.master.as_raw_fd().expect("Linux PTY has a master fd");
            // SAFETY: pair owns this descriptor until after the duplicate is created.
            let master = unsafe { BorrowedFd::borrow_raw(master_fd) };
            // Reader and writer share these flags and both explicitly handle WouldBlock.
            // Killing every slave holder does not reliably interrupt a blocked master write.
            let flags = unsafe { libc::fcntl(master_fd, libc::F_GETFL) };
            if flags < 0
                || unsafe { libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(session_error(
                    "spawn_failed",
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            let reader = master
                .try_clone_to_owned()
                .map(std::fs::File::from)
                .map_err(|e| session_error("spawn_failed", e.to_string()))?;
            let mut writer = Some(
                master
                    .try_clone_to_owned()
                    .map(std::fs::File::from)
                    .map_err(|e| session_error("spawn_failed", e.to_string()))?,
            );
            let mut child = pair
                .slave
                .spawn_command(command)
                .map_err(|e| session_error("spawn_failed", e.to_string()))?;
            let pid = child.process_id().expect("Linux PTY child has a PID") as libc::pid_t;
            drop(pair.slave);
            let (tx, rx) = mpsc::sync_channel(16);
            std::thread::scope(|scope| {
                // The sender's lifetime stops the reader on every return/unwind before
                // scope joins it, even when a different job group retains the slave.
                let (_reader_stop, reader_stopped) = mpsc::channel::<()>();
                scope.spawn(move || read_pty_output(reader, tx, reader_stopped));
                // Keep input and resize operations ordered here, with cancellation-aware
                // nonblocking I/O and independent child supervision.
                let (_stop, stopped) = mpsc::channel::<()>();
                let (exit_tx, child_exit) = mpsc::channel();
                let worker_cancellation = &cancellation;
                let input_stop = WorkspaceCancellationToken::new();
                let child_input_stop = input_stop.clone();
                scope.spawn(move || {
                    loop {
                        if worker_cancellation.is_cancelled()
                            || matches!(
                                stopped.recv_timeout(Duration::from_millis(2)),
                                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)
                            )
                        {
                            let _ = terminate_pty_group(pid);
                            let _ = child.kill();
                            let _ = exit_tx.send(child.wait());
                            break;
                        }
                        match pty_child_exited(pid) {
                            Ok(true) => {
                                // portable-pty starts a new session/group with PGID=PID.
                                // Kill original-group descendants BEFORE reaping the leader:
                                // its reserved PID prevents signalling a reused unrelated PGID.
                                // Preserve its exit status, and drain already-produced output.
                                let terminated = terminate_pty_group(pid);
                                let status = child.wait();
                                let _ = exit_tx.send(terminated.and(status));
                                break;
                            }
                            Ok(false) => {}
                            Err(error) => {
                                let _ = terminate_pty_group(pid);
                                let _ = child.kill();
                                let _ = child.wait();
                                let _ = exit_tx.send(Err(error));
                                break;
                            }
                        }
                    }
                    // A completed leader must also release pending input when a
                    // different job group still holds a non-reading slave.
                    child_input_stop.cancel();
                });
                // Every return drops _stop, waking the supervisor before the
                // scope joins it. No writer or child-wait thread is detached.
                let mut reader_open = true;
                let mut exit = None;
                let mut drain_deadline = None;
                let forward_output = || {
                    let mut closed = false;
                    // Bound each output turn so continuously readable output cannot
                    // starve input. The write loop also pumps this queue while blocked:
                    // a child may need its output drained before it can consume input.
                    for _ in 0..16 {
                        match rx.try_recv() {
                            Ok(SessionPipeResult::Data(_, body)) => {
                                let _ = output.send_with_cancellation(
                                    V5ServeOutputEvent::SessionStreamData {
                                        stream_id,
                                        channel: protocol_v5::DataChannel::Stdout,
                                        body,
                                        priority,
                                    },
                                    &cancellation,
                                );
                            }
                            Ok(SessionPipeResult::Closed) => closed = true,
                            Err(_) => break,
                        }
                    }
                    closed
                };
                loop {
                    if cancellation.is_cancelled() {
                        return Err(session_error("cancelled", "pty.session was reset"));
                    }
                    match input.recv_timeout(Duration::from_millis(2)) {
                        Ok(V5ProcessSessionInput::Data { body, credit }) => {
                            let disposition = if writer.as_mut().is_some_and(|writer| {
                                write_pty_input(
                                    writer,
                                    &body,
                                    &cancellation,
                                    &input_stop,
                                    &mut || reader_open &= !forward_output(),
                                )
                                .is_ok()
                            }) {
                                SessionInputDisposition::Consumed
                            } else {
                                SessionInputDisposition::Discarded
                            };
                            let _ = output.send_with_cancellation(
                                V5ServeOutputEvent::SessionInputSettled {
                                    stream_id,
                                    bytes: credit,
                                    disposition,
                                    priority,
                                },
                                &cancellation,
                            );
                        }
                        Ok(V5ProcessSessionInput::PtyControl { body, credit }) => {
                            let applied = protocol_v5::PtyControl::decode(body.as_slice())
                                .ok()
                                .and_then(|c| c.resize)
                                .filter(|r| {
                                    r.cols > 0
                                        && r.rows > 0
                                        && r.cols <= u16::MAX.into()
                                        && r.rows <= u16::MAX.into()
                                        && r.pixel_width <= u16::MAX.into()
                                        && r.pixel_height <= u16::MAX.into()
                                })
                                .and_then(|r| {
                                    pair.master
                                        .resize(PtySize {
                                            rows: r.rows as u16,
                                            cols: r.cols as u16,
                                            pixel_width: r.pixel_width as u16,
                                            pixel_height: r.pixel_height as u16,
                                        })
                                        .ok()
                                });
                            let disposition = if applied.is_some() {
                                SessionInputDisposition::Consumed
                            } else {
                                SessionInputDisposition::Discarded
                            };
                            let _ = output.send_with_cancellation(
                                V5ServeOutputEvent::SessionInputSettled {
                                    stream_id,
                                    bytes: credit,
                                    disposition,
                                    priority,
                                },
                                &cancellation,
                            );
                            if applied.is_none() {
                                return Err(session_error(
                                    "invalid_request",
                                    "invalid PTY resize control",
                                ));
                            }
                        }
                        Ok(V5ProcessSessionInput::Reset) => {
                            return Err(session_error("cancelled", "pty.session was reset"));
                        }
                        Ok(V5ProcessSessionInput::End)
                        | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            eof.store(true, Ordering::Release);
                            if let Some(mut writer) = writer.take() {
                                send_pty_eof(&mut writer, &cancellation, &input_stop, &mut || {
                                    reader_open &= !forward_output()
                                });
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // END may not fit in the bounded ingress queue. Honour its atomic
                            // fallback only after queued input and resizes have drained.
                            if eof.load(Ordering::Acquire) {
                                if let Some(mut writer) = writer.take() {
                                    send_pty_eof(
                                        &mut writer,
                                        &cancellation,
                                        &input_stop,
                                        &mut || reader_open &= !forward_output(),
                                    );
                                }
                            }
                        }
                    }
                    reader_open &= !forward_output();
                    if exit.is_none() {
                        if let Ok(status) = child_exit.try_recv() {
                            exit = Some(
                                status
                                    .map_err(|e| session_error("process_failed", e.to_string()))?,
                            );
                        }
                        if exit.is_some() {
                            writer.take();
                            // Other job groups may retain the slave. Bound final output drain,
                            // then stop and join our reader without waiting for slave closure.
                            drain_deadline = Some(Instant::now() + Duration::from_millis(250));
                        }
                    }
                    if exit.is_some()
                        && (!reader_open || drain_deadline.is_some_and(|d| Instant::now() >= d))
                    {
                        // Drain everything already queued before completion/END is emitted.
                        while let Ok(event) = rx.try_recv() {
                            if let SessionPipeResult::Data(_, body) = event {
                                let _ = output.send_with_cancellation(
                                    V5ServeOutputEvent::SessionStreamData {
                                        stream_id,
                                        channel: protocol_v5::DataChannel::Stdout,
                                        body,
                                        priority,
                                    },
                                    &cancellation,
                                );
                            }
                        }
                        let status = exit.expect("exit status checked above");
                        return Ok(ProcessSessionCompletion {
                            status_code: status.exit_code().try_into().ok(),
                            success: status.success(),
                        });
                    }
                }
            })
        });
        send_session_terminal(&output, stream_id, priority, result, &cancellation);
    }

    fn prepare_pty(
        &self,
        request: &PtySessionRequest,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<(CommandBuilder, PathBuf), RemoteError> {
        let (program, args) = match &request.command {
            PtySessionCommand::Command { program, args } => (program.clone(), args.clone()),
            PtySessionCommand::LoginShell { shell } => (
                shell.clone().unwrap_or_else(|| "/bin/sh".into()),
                Vec::new(),
            ),
        };
        let process = ProcessSessionRequest {
            program,
            args,
            cwd: request.cwd.clone(),
            environment: request.environment.clone(),
            env_overrides: request.env_overrides.clone(),
        };
        let (program, cwd, mut env) = self.prepare_v5_process_session(&process, cancellation)?;
        for key in crate::proxy::INTERACTIVE_SHELL_STATE_ENV_VARS {
            env.remove(*key);
        }
        configure_pty_shell_environment(&request.command, &program, &mut env);
        env.extend([
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("TERM_PROGRAM".into(), "nucleotide".into()),
            ("NUCLEOTIDE_TERM".into(), "true".into()),
        ]);
        let mut command = match &request.command {
            PtySessionCommand::LoginShell { shell: None } => CommandBuilder::new_default_prog(),
            PtySessionCommand::LoginShell { shell: Some(_) } => {
                let mut command = CommandBuilder::new(&program);
                command.arg("-l");
                command
            }
            PtySessionCommand::Command { .. } => {
                let mut command = CommandBuilder::new(&program);
                command.args(process.args);
                command
            }
        };
        command.env_clear();
        for (key, value) in env {
            command.env(key, value);
        }
        Ok((command, cwd))
    }
}

fn pty_child_exited(pid: libc::pid_t) -> std::io::Result<bool> {
    loop {
        // SAFETY: waitid writes a siginfo_t for our owned child. WNOWAIT retains
        // the leader's PID until group cleanup completes and Child::wait reaps it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn terminate_pty_group(pid: libc::pid_t) -> std::io::Result<()> {
    // SAFETY: portable-pty creates this group; its direct leader is still owned
    // and unreaped. Never call this after Child::wait/try_wait/kill can reap it.
    if unsafe { libc::kill(-pid, libc::SIGKILL) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn read_pty_output(
    mut reader: std::fs::File,
    sender: mpsc::SyncSender<SessionPipeResult>,
    stopped: mpsc::Receiver<()>,
) {
    let mut buffer = [0; 8192];
    loop {
        if !matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            return;
        }
        let mut fd = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd describes our live nonblocking reader.
        let ready = unsafe { libc::poll(&mut fd, 1, 10) };
        if ready == 0 {
            continue;
        }
        if ready < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        let event = if ready < 0 || fd.revents & libc::POLLNVAL != 0 {
            SessionPipeResult::Closed
        } else {
            // Sole reader: HUP may accompany final buffered data; read it before
            // treating EOF/Linux PTY EIO as closure.
            match reader.read(&mut buffer) {
                Ok(0) => SessionPipeResult::Closed,
                Ok(n) => {
                    SessionPipeResult::Data(protocol_v5::DataChannel::Stdout, buffer[..n].to_vec())
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                Err(_) => SessionPipeResult::Closed,
            }
        };
        let closed = matches!(event, SessionPipeResult::Closed);
        let mut pending = event;
        loop {
            if !matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                return;
            }
            match sender.try_send(pending) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => return,
                Err(mpsc::TrySendError::Full(event)) => {
                    pending = event;
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        if closed {
            return;
        }
    }
}

fn write_pty_input(
    writer: &mut std::fs::File,
    mut body: &[u8],
    cancellation: &WorkspaceCancellationToken,
    input_stop: &WorkspaceCancellationToken,
    forward_output: &mut impl FnMut(),
) -> std::io::Result<()> {
    while !body.is_empty() {
        if cancellation.is_cancelled() || input_stop.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "PTY input cancelled",
            ));
        }
        forward_output();
        // A bounded write turn keeps both directions moving, including when
        // every write succeeds but the child is continuously producing output.
        match writer.write(&body[..body.len().min(8192)]) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => body = &body[n..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let mut fd = libc::pollfd {
                    fd: writer.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: writer owns this live descriptor; the timeout bounds cancellation polling.
                if unsafe { libc::poll(&mut fd, 1, 10) } < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                } else if fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn send_pty_eof(
    writer: &mut std::fs::File,
    cancellation: &WorkspaceCancellationToken,
    input_stop: &WorkspaceCancellationToken,
    forward_output: &mut impl FnMut(),
) {
    // Match portable-pty's writer-drop convention without a blocking Drop: newline
    // then VEOF. Canonical readers interpret EOF; raw readers receive literal bytes.
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: termios is writable and writer owns the live master descriptor.
    if unsafe { libc::tcgetattr(writer.as_raw_fd(), &mut termios) } == 0 {
        let eof = termios.c_cc[libc::VEOF];
        if eof != 0 {
            let _ = write_pty_input(
                writer,
                &[b'\n', eof],
                cancellation,
                input_stop,
                forward_output,
            );
        }
    }
}

fn configure_pty_shell_environment(
    command: &PtySessionCommand,
    program: &Path,
    env: &mut BTreeMap<String, String>,
) {
    match command {
        // `CommandBuilder::new_default_prog` consults its environment before the password
        // database. Do not let the service's Nix launch environment replace the account's
        // configured login shell.
        PtySessionCommand::LoginShell { shell: None } => {
            env.remove("SHELL");
        }
        PtySessionCommand::LoginShell { shell: Some(_) } => {
            env.insert("SHELL".into(), program.to_string_lossy().into_owned());
        }
        PtySessionCommand::Command { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_login_shell_does_not_inherit_service_shell() {
        let mut env = BTreeMap::from([(
            "SHELL".to_string(),
            "/nix/store/helper-bash/bin/bash".to_string(),
        )]);

        configure_pty_shell_environment(
            &PtySessionCommand::LoginShell { shell: None },
            Path::new("/bin/sh"),
            &mut env,
        );

        assert!(!env.contains_key("SHELL"));
    }

    #[test]
    fn explicit_login_shell_replaces_service_shell() {
        let mut env = BTreeMap::from([(
            "SHELL".to_string(),
            "/nix/store/helper-bash/bin/bash".to_string(),
        )]);

        configure_pty_shell_environment(
            &PtySessionCommand::LoginShell {
                shell: Some("/bin/zsh".to_string()),
            },
            Path::new("/bin/zsh"),
            &mut env,
        );

        assert_eq!(env.get("SHELL").map(String::as_str), Some("/bin/zsh"));
    }
}
