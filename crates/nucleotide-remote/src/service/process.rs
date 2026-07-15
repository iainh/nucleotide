// ABOUTME: Cancellable local Git and process execution for remote workspace requests
// ABOUTME: Streams bounded output and contains process-group timeout cleanup

use super::*;

pub(crate) fn v5_local_git_head(
    root: &Path,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<GitHeadResult, WorkspaceError> {
    let mut command = Command::new("git");
    command
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(root);
    let output = v5_run_cancellable_command_collect(command, "git rev-parse", root, cancellation)?;

    if !output.status.success() {
        return Ok(GitHeadResult {
            root: root.to_path_buf(),
            head: None,
        });
    }

    let head = std::str::from_utf8(&output.stdout)
        .ok()
        .map(str::trim)
        .filter(|head| !head.is_empty())
        .map(ToOwned::to_owned);

    Ok(GitHeadResult {
        root: root.to_path_buf(),
        head,
    })
}

pub(crate) fn v5_local_git_status(
    root: &Path,
    options: GitStatusOptions,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<GitStatusResult, WorkspaceError> {
    let mut command = Command::new("git");
    command
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(root);
    if options.include_untracked {
        command.arg("--untracked-files=all");
    } else {
        command.arg("--untracked-files=no");
    }

    let output = v5_run_cancellable_command_collect(command, "git status", root, cancellation)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if v5_git_error_is_not_repository(&stderr) {
            return Ok(GitStatusResult {
                root: root.to_path_buf(),
                entries: Vec::new(),
                truncated: false,
            });
        }

        let message = if stderr.is_empty() {
            format!("git exited with status {}", output.status)
        } else {
            format!("git exited with status {}: {stderr}", output.status)
        };
        return Err(WorkspaceError::CommandFailed {
            operation: "git status",
            path: root.to_path_buf(),
            message,
        });
    }

    Ok(v5_parse_git_status_output(
        root,
        &output.stdout,
        options.limit,
    ))
}

#[derive(Debug)]
pub(crate) struct V5CollectedCommandOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub(crate) fn v5_run_cancellable_command_collect(
    mut command: Command,
    operation: &'static str,
    path: &Path,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<V5CollectedCommandOutput, WorkspaceError> {
    if v5_stream_cancelled_ref(cancellation) {
        return Err(WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: format!("{operation} cancelled"),
        });
    }

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    v5_configure_workspace_process(&mut command);

    let mut child = command.spawn().map_err(|source| WorkspaceError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: "child process stdout was not piped".to_string(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: "child process stderr was not piped".to_string(),
        })?;

    let stdout_thread = std::thread::spawn(move || v5_read_command_pipe(stdout));
    let stderr_thread = std::thread::spawn(move || v5_read_command_pipe(stderr));
    let exit = v5_wait_for_process(&mut child, None, cancellation, path)?;
    let stdout = v5_join_io_thread(stdout_thread, operation, path)?;
    let stderr = v5_join_io_thread(stderr_thread, operation, path)?;

    if exit.canceled {
        return Err(WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: format!("{operation} cancelled"),
        });
    }

    Ok(V5CollectedCommandOutput {
        status: exit.status,
        stdout,
        stderr,
    })
}

pub(crate) fn v5_read_command_pipe<R: Read>(mut reader: R) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    Ok(buffer)
}

pub(crate) fn v5_git_error_is_not_repository(message: &str) -> bool {
    message.contains("not a git repository")
}

pub(crate) fn v5_parse_git_status_output(
    root: &Path,
    output: &[u8],
    limit: usize,
) -> GitStatusResult {
    let mut entries = Vec::new();
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut truncated = false;

    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            continue;
        }

        let index = field[0];
        let worktree = field[1];
        let relative_path = v5_path_from_git_bytes(&field[3..]);
        let original_relative_path = if matches!(index, b'R' | b'C') {
            fields.next().map(v5_path_from_git_bytes)
        } else {
            None
        };

        if entries.len() >= limit {
            truncated = true;
            break;
        }

        entries.push(GitStatusEntry {
            relative_path,
            original_relative_path,
            index_status: v5_git_status_kind(index, worktree),
            working_tree_status: v5_git_status_kind(worktree, index),
        });
    }

    GitStatusResult {
        root: root.to_path_buf(),
        entries,
        truncated,
    }
}

pub(crate) fn v5_path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

pub(crate) fn v5_git_status_kind(status: u8, other: u8) -> GitStatusKind {
    if v5_git_status_is_conflict_pair(status, other) {
        return GitStatusKind::Conflicted;
    }

    match status {
        b' ' => GitStatusKind::Unmodified,
        b'M' => GitStatusKind::Modified,
        b'A' => GitStatusKind::Added,
        b'D' => GitStatusKind::Deleted,
        b'R' => GitStatusKind::Renamed,
        b'C' => GitStatusKind::Copied,
        b'T' => GitStatusKind::TypeChanged,
        b'?' => GitStatusKind::Untracked,
        b'U' => GitStatusKind::Conflicted,
        _ => GitStatusKind::Unknown,
    }
}

pub(crate) fn v5_git_status_is_conflict_pair(left: u8, right: u8) -> bool {
    matches!(
        (left, right),
        (b'D', b'D')
            | (b'A', b'U')
            | (b'U', b'D')
            | (b'U', b'A')
            | (b'D', b'U')
            | (b'A', b'A')
            | (b'U', b'U')
    )
}

#[derive(Debug)]
pub(crate) struct V5StreamedProcessOutput {
    status_code: Option<i32>,
    success: bool,
    stdout_len: usize,
    stderr_len: usize,
    stdout_truncated: bool,
    stderr_truncated: bool,
    timed_out: bool,
}

#[derive(Debug)]
pub(crate) struct V5StreamedProcessPipe {
    len: usize,
    truncated: bool,
}

pub(crate) fn v5_process_output_limit(max_output_bytes: Option<usize>) -> usize {
    max_output_bytes
        .unwrap_or((MAX_FRAME_BODY_LEN / 2) as usize)
        .min((MAX_FRAME_BODY_LEN / 2) as usize)
}

pub(crate) fn v5_run_local_streaming_process(
    spec: ProcessSpec,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<WorkspaceCancellationToken>,
) -> std::result::Result<V5StreamedProcessOutput, WorkspaceError> {
    let cwd = spec.cwd.clone();
    if v5_stream_cancelled(&cancellation) {
        return Err(WorkspaceError::CommandFailed {
            operation: "run process",
            path: cwd,
            message: "process cancelled".to_string(),
        });
    }

    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if spec.clear_env {
        command.env_clear();
    }
    v5_apply_process_environment(&mut command, &spec.env);
    v5_configure_workspace_process(&mut command);

    let mut child = command.spawn().map_err(|source| WorkspaceError::Io {
        operation: "spawn process",
        path: cwd.clone(),
        source,
    })?;

    let output_limit = spec
        .max_output_bytes
        .unwrap_or_else(|| v5_process_output_limit(None));
    let mut stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "spawn process",
            path: cwd.clone(),
            message: "child process stdout was not piped".to_string(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "spawn process",
            path: cwd.clone(),
            message: "child process stderr was not piped".to_string(),
        })?;

    let stdout_events = stream_events.clone();
    let stdout_cancellation = cancellation.clone();
    let stdout_thread = std::thread::spawn(move || {
        v5_stream_process_stdout(
            stdout,
            output_limit,
            stream_id,
            priority,
            stdout_events,
            stdout_cancellation,
        )
    });
    let stderr_cancellation = cancellation.clone();
    let stderr_thread = std::thread::spawn(move || {
        v5_stream_process_stderr(
            stderr,
            output_limit,
            stream_id,
            priority,
            stream_events,
            stderr_cancellation,
        )
    });
    let input = spec.stdin;
    let stdin_thread = stdin.take().map(|mut stdin| {
        std::thread::spawn(move || match stdin.write_all(&input) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(error),
        })
    });

    let process_exit =
        v5_wait_for_process(&mut child, spec.timeout_ms, cancellation.as_ref(), &cwd)?;

    if let Some(thread) = stdin_thread {
        v5_join_io_thread(thread, "write process stdin", &cwd)?;
    }
    let stdout = v5_join_io_thread(stdout_thread, "stream process stdout", &cwd)?;
    let stderr = v5_join_io_thread(stderr_thread, "stream process stderr", &cwd)?;

    if process_exit.canceled {
        return Err(WorkspaceError::CommandFailed {
            operation: "run process",
            path: cwd,
            message: "process cancelled".to_string(),
        });
    }

    Ok(V5StreamedProcessOutput {
        status_code: process_exit.status.code(),
        success: process_exit.status.success(),
        stdout_len: stdout.len,
        stderr_len: stderr.len,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        timed_out: process_exit.timed_out,
    })
}

pub(crate) fn v5_stream_process_stdout(
    reader: ChildStdout,
    limit: usize,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<WorkspaceCancellationToken>,
) -> io::Result<V5StreamedProcessPipe> {
    v5_read_limited_process_pipe(
        reader,
        limit,
        stream_id,
        protocol_v5::DataChannel::Stdout,
        priority,
        stream_events,
        cancellation,
    )
}

pub(crate) fn v5_stream_process_stderr(
    reader: ChildStderr,
    limit: usize,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<WorkspaceCancellationToken>,
) -> io::Result<V5StreamedProcessPipe> {
    v5_read_limited_process_pipe(
        reader,
        limit,
        stream_id,
        protocol_v5::DataChannel::Stderr,
        priority,
        stream_events,
        cancellation,
    )
}

pub(crate) fn v5_read_limited_process_pipe<R: Read>(
    mut reader: R,
    limit: usize,
    stream_id: u64,
    channel: protocol_v5::DataChannel,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<WorkspaceCancellationToken>,
) -> io::Result<V5StreamedProcessPipe> {
    let mut len = 0_usize;
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];

    loop {
        if v5_stream_cancelled(&cancellation) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "process output streaming cancelled",
            ));
        }
        let read = reader.read(&mut buffer)?;
        if v5_stream_cancelled(&cancellation) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "process output streaming cancelled",
            ));
        }
        if read == 0 {
            break;
        }

        let remaining = limit.saturating_sub(len);
        let retained = remaining.min(read);
        if retained > 0 {
            v5_send_output_event_with_optional_cancellation(
                &stream_events,
                V5ServeOutputEvent::StreamData {
                    stream_id,
                    channel,
                    body: buffer[..retained].to_vec(),
                    priority,
                },
                cancellation.as_ref(),
            )
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "v5 service event loop closed while streaming process output",
                )
            })?;
            len += retained;
        }
        if retained < read {
            truncated = true;
        }
    }

    Ok(V5StreamedProcessPipe { len, truncated })
}

pub(crate) fn v5_wait_for_process(
    child: &mut Child,
    timeout_ms: Option<u64>,
    cancellation: Option<&WorkspaceCancellationToken>,
    path: &Path,
) -> std::result::Result<V5ProcessExit, WorkspaceError> {
    if timeout_ms.is_none() && cancellation.is_none() {
        return child
            .wait()
            .map(|status| V5ProcessExit {
                status,
                timed_out: false,
                canceled: false,
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "wait for process",
                path: path.to_path_buf(),
                source,
            });
    }

    let timeout = timeout_ms.map(Duration::from_millis);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|source| WorkspaceError::Io {
            operation: "poll process",
            path: path.to_path_buf(),
            source,
        })? {
            return Ok(V5ProcessExit {
                status,
                timed_out: false,
                canceled: false,
            });
        }

        if v5_stream_cancelled_ref(cancellation) {
            v5_kill_timed_out_process(child, path)?;
            return child
                .wait()
                .map(|status| V5ProcessExit {
                    status,
                    timed_out: false,
                    canceled: true,
                })
                .map_err(|source| WorkspaceError::Io {
                    operation: "wait for cancelled process",
                    path: path.to_path_buf(),
                    source,
                });
        }

        if let Some(timeout) = timeout {
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                v5_kill_timed_out_process(child, path)?;
                return child
                    .wait()
                    .map(|status| V5ProcessExit {
                        status,
                        timed_out: true,
                        canceled: false,
                    })
                    .map_err(|source| WorkspaceError::Io {
                        operation: "wait for killed process",
                        path: path.to_path_buf(),
                        source,
                    });
            }

            let remaining = timeout.saturating_sub(elapsed);
            std::thread::sleep(remaining.min(Duration::from_millis(10)));
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[derive(Debug)]
pub(crate) struct V5ProcessExit {
    status: std::process::ExitStatus,
    timed_out: bool,
    canceled: bool,
}

pub(crate) fn v5_stream_cancelled(cancellation: &Option<WorkspaceCancellationToken>) -> bool {
    v5_stream_cancelled_ref(cancellation.as_ref())
}

pub(crate) fn v5_stream_cancelled_ref(cancellation: Option<&WorkspaceCancellationToken>) -> bool {
    cancellation.is_some_and(WorkspaceCancellationToken::is_cancelled)
}

pub(crate) fn v5_apply_process_environment(
    command: &mut Command,
    environment: &BTreeMap<String, String>,
) {
    for (key, value) in environment {
        if v5_process_environment_entry_is_valid(key, value) {
            command.env(key, value);
        }
    }
}

pub(crate) fn v5_process_environment_entry_is_valid(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains(['=', '\0']) && !value.contains('\0')
}

#[cfg(unix)]
pub(crate) fn v5_configure_workspace_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn v5_configure_workspace_process(_command: &mut Command) {}

pub(crate) fn v5_kill_timed_out_process(
    child: &mut Child,
    path: &Path,
) -> std::result::Result<(), WorkspaceError> {
    #[cfg(unix)]
    {
        if v5_kill_process_group(child.id()).is_ok() {
            return Ok(());
        }
    }

    child.kill().map_err(|source| WorkspaceError::Io {
        operation: "kill timed out process",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
pub(crate) fn v5_kill_process_group(process_id: u32) -> io::Result<()> {
    let status = Command::new("kill")
        .arg("-KILL")
        .arg(format!("-{process_id}"))
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "kill process group exited with {status}"
        )))
    }
}

pub(crate) fn v5_join_io_thread<T>(
    thread: std::thread::JoinHandle<io::Result<T>>,
    operation: &'static str,
    path: &Path,
) -> std::result::Result<T, WorkspaceError> {
    thread
        .join()
        .map_err(|_| WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: "I/O thread panicked".to_string(),
        })?
        .map_err(|source| WorkspaceError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn v5_streamed_process_output_response(
    output: &V5StreamedProcessOutput,
) -> ProcessOutputResponse {
    ProcessOutputResponse {
        status_code: output.status_code,
        success: output.success,
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
        stdout_len: output.stdout_len,
        stderr_len: output.stderr_len,
        timed_out: output.timed_out,
    }
}
