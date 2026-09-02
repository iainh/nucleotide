// ABOUTME: Linux service-owned pseudo-terminal sessions for protocol v5
// ABOUTME: Spawns login shells or commands under portable-pty and bridges ordered input/control

use super::*;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use prost::Message;

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
        let result = self.prepare_pty(&request).and_then(|(mut command, cwd)| {
            command.cwd(cwd);
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: request.rows,
                    cols: request.cols,
                    pixel_width: request.pixel_width,
                    pixel_height: request.pixel_height,
                })
                .map_err(|e| session_error("spawn_failed", e.to_string()))?;
            let mut child = pair
                .slave
                .spawn_command(command)
                .map_err(|e| session_error("spawn_failed", e.to_string()))?;
            drop(pair.slave);
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(|e| session_error("spawn_failed", e.to_string()))?;
            let mut writer = Some(
                pair.master
                    .take_writer()
                    .map_err(|e| session_error("spawn_failed", e.to_string()))?,
            );
            let (tx, rx) = mpsc::sync_channel(16);
            spawn_session_pipe(reader, protocol_v5::DataChannel::Stdout, tx);
            let mut reader_open = true;
            let mut exit = None;
            let mut drain_deadline = None;
            loop {
                if cancellation.is_cancelled() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(session_error("cancelled", "pty.session was reset"));
                }
                match input.recv_timeout(Duration::from_millis(2)) {
                    Ok(V5ProcessSessionInput::Data { body, credit }) => {
                        let disposition = if writer
                            .as_mut()
                            .is_some_and(|writer| writer.write_all(&body).is_ok())
                        {
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
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(session_error(
                                "invalid_request",
                                "invalid PTY resize control",
                            ));
                        }
                    }
                    Ok(V5ProcessSessionInput::Reset) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(session_error("cancelled", "pty.session was reset"));
                    }
                    Ok(V5ProcessSessionInput::End) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        eof.store(true, Ordering::Release);
                        // Closing the master writer is the PTY equivalent of stdin EOF. Merely
                        // remembering END leaves readers such as `cat` blocked forever.
                        writer.take();
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                while let Ok(event) = rx.try_recv() {
                    match event {
                        SessionPipeResult::Data(_, body) => {
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
                        SessionPipeResult::Closed => reader_open = false,
                    }
                }
                if exit.is_none() {
                    exit = child
                        .try_wait()
                        .map_err(|e| session_error("process_failed", e.to_string()))?;
                    if exit.is_some() {
                        writer.take();
                        // A descendant may retain the slave. Give final bytes a bounded window,
                        // then finish without ever joining the portable-pty blocking reader.
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
        });
        send_session_terminal(&output, stream_id, priority, result, &cancellation);
    }

    fn prepare_pty(
        &self,
        request: &PtySessionRequest,
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
        let (program, cwd, mut env) = self.prepare_v5_process_session(&process)?;
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
