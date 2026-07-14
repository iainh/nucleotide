use std::{
    ffi::OsStr,
    io::{self, Read},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    hide_window(&mut command);
    command
}

pub fn hide_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLimits {
    pub timeout: Duration,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
}

impl OutputLimits {
    pub const fn new(timeout: Duration, max_stdout_bytes: usize, max_stderr_bytes: usize) -> Self {
        Self {
            timeout,
            max_stdout_bytes,
            max_stderr_bytes,
        }
    }
}

/// Runs a child with bounded output and kills and reaps it when the deadline expires.
///
/// On Unix the child starts a new process group so descendants that retain an output pipe are
/// terminated with the child. Other platforms fall back to terminating the direct child; a
/// platform-specific job object is required to extend that guarantee to descendants on Windows.
/// The caller remains responsible for configuring stdin, including setting it to [`Stdio::null`]
/// for commands that should receive immediate EOF.
pub fn output_with_limits(command: &mut Command, limits: OutputLimits) -> io::Result<Output> {
    output_with_limits_inner(command, limits, None)
}

/// Runs a child with bounded output and stops its process tree when cancellation is requested.
///
/// Cancellation is cooperative at a polling interval of at most 10 ms. The same containment and
/// reaping guarantees as [`output_with_limits`] apply.
pub fn output_with_limits_and_cancellation(
    command: &mut Command,
    limits: OutputLimits,
    cancellation: &AtomicBool,
) -> io::Result<Output> {
    output_with_limits_inner(command, limits, Some(cancellation))
}

fn output_with_limits_inner(
    command: &mut Command,
    limits: OutputLimits,
    cancellation: Option<&AtomicBool>,
) -> io::Result<Output> {
    if cancellation.is_some_and(|cancellation| cancellation.load(Ordering::Acquire)) {
        return Err(cancelled_output_error());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    configure_output_child(command);
    let mut child = ChildGuard::spawn(command)?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("child stdout was not piped"))?;
    let stderr = child
        .child_mut()
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("child stderr was not piped"))?;
    let output_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_bounded_reader(
        "nucleotide-child-stdout",
        stdout,
        limits.max_stdout_bytes,
        Arc::clone(&output_exceeded),
    )?;
    let stderr_reader = spawn_bounded_reader(
        "nucleotide-child-stderr",
        stderr,
        limits.max_stderr_bytes,
        Arc::clone(&output_exceeded),
    )?;

    let started = Instant::now();
    let mut status = None;
    loop {
        if cancellation.is_some_and(|cancellation| cancellation.load(Ordering::Acquire)) {
            return Err(cancelled_output_error());
        }
        if output_exceeded.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "child output exceeded configured byte limit",
            ));
        }

        if status.is_none() {
            match child.child_mut().try_wait() {
                Ok(Some(child_status)) => status = Some(child_status),
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }

        if status.is_some() && stdout_reader.is_finished() && stderr_reader.is_finished() {
            break;
        }

        if started.elapsed() >= limits.timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "child and output pipes did not finish within {} ms",
                    limits.timeout.as_millis()
                ),
            ));
        }

        thread::sleep(Duration::from_millis(10));
    }

    let stdout = join_bounded_reader(stdout_reader)?;
    let stderr = join_bounded_reader(stderr_reader)?;
    if output_exceeded.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "child output exceeded configured byte limit",
        ));
    }
    child.disarm();
    Ok(Output {
        status: status.expect("child status was observed before joining output readers"),
        stdout,
        stderr,
    })
}

fn cancelled_output_error() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "child execution cancelled")
}

#[cfg(unix)]
fn configure_output_child(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_output_child(_command: &mut Command) {}

struct ChildGuard {
    child: Option<Child>,
    #[cfg(unix)]
    process_group_id: u32,
}

impl ChildGuard {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        let child = command.spawn()?;
        #[cfg(unix)]
        let process_group_id = child.id();

        Ok(Self {
            child: Some(child),
            #[cfg(unix)]
            process_group_id,
        })
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("child guard was used after being disarmed")
    }

    fn disarm(&mut self) {
        self.child.take();
    }

    fn terminate_and_reap(&mut self) -> io::Result<ExitStatus> {
        let Some(mut child) = self.child.take() else {
            return Err(io::Error::other("child guard was already disarmed"));
        };

        #[cfg(unix)]
        let termination = terminate_process_group(&mut child, self.process_group_id);
        #[cfg(not(unix))]
        let termination = terminate_direct_child(&mut child);

        let wait = child.wait();
        match (termination, wait) {
            (Ok(()), Ok(status)) => Ok(status),
            (Err(error), Ok(_)) | (Ok(()), Err(error)) => Err(error),
            (Err(termination_error), Err(wait_error)) => Err(io::Error::new(
                wait_error.kind(),
                format!(
                    "failed to terminate child: {termination_error}; failed to reap child: {wait_error}"
                ),
            )),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = self.terminate_and_reap();
        }
    }
}

#[cfg(unix)]
fn terminate_process_group(child: &mut Child, process_group_id: u32) -> io::Result<()> {
    let process_group_id = match libc::pid_t::try_from(process_group_id) {
        Ok(process_group_id) => process_group_id,
        Err(_) => {
            let error = io::Error::other("child process ID did not fit in pid_t");
            return terminate_direct_child(child).and(Err(error));
        }
    };
    // SAFETY: `process_group_id` belongs to the child that this module just spawned. Negating it
    // asks POSIX `kill` to signal that process group; SIGKILL requires no shared-memory access.
    let result = unsafe { libc::kill(-process_group_id, libc::SIGKILL) };
    let group_error = (result != 0)
        .then(io::Error::last_os_error)
        .filter(|error| error.raw_os_error() != Some(libc::ESRCH));
    let direct_result = terminate_direct_child(child);

    match (group_error, direct_result) {
        (None, Ok(())) => Ok(()),
        (Some(error), Ok(())) | (None, Err(error)) => Err(error),
        (Some(group_error), Err(child_error)) => Err(io::Error::new(
            child_error.kind(),
            format!(
                "failed to kill child process group: {group_error}; direct-child fallback also failed: {child_error}"
            ),
        )),
    }
}

fn terminate_direct_child(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    Ok(())
}

fn spawn_bounded_reader<R>(
    name: &str,
    reader: R,
    limit: usize,
    output_exceeded: Arc<AtomicBool>,
) -> io::Result<thread::JoinHandle<io::Result<Vec<u8>>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || read_bounded(reader, limit, &output_exceeded))
}

fn read_bounded(
    mut reader: impl Read,
    limit: usize,
    output_exceeded: &AtomicBool,
) -> io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(limit.min(8 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(output);
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..read.min(remaining)]);
        if read > remaining {
            output_exceeded.store(true, Ordering::Release);
        }
    }
}

fn join_bounded_reader(reader: thread::JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| io::Error::other("child output reader panicked"))?
}

#[cfg(feature = "tokio")]
pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    hide_tokio_window(&mut command);
    command
}

#[cfg(feature = "tokio")]
pub fn hide_tokio_window(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_reader_retains_only_the_configured_prefix() {
        let exceeded = AtomicBool::new(false);

        let output = read_bounded(Cursor::new(vec![7_u8; 32]), 10, &exceeded).unwrap();

        assert_eq!(output, vec![7_u8; 10]);
        assert!(exceeded.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[test]
    fn output_with_limits_captures_small_successful_output() {
        let mut child = Command::new("printf");
        child.arg("ready");

        let output = output_with_limits(
            &mut child,
            OutputLimits::new(Duration::from_secs(1), 64, 64),
        )
        .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"ready");
    }

    #[cfg(unix)]
    #[test]
    fn output_with_limits_preserves_explicit_stdin() {
        let temp = tempfile::tempdir().unwrap();
        let input_path = temp.path().join("input");
        std::fs::write(&input_path, b"configured input").unwrap();
        let input = std::fs::File::open(&input_path).unwrap();
        let mut child = Command::new("cat");
        child.stdin(Stdio::from(input));

        let output = output_with_limits(
            &mut child,
            OutputLimits::new(Duration::from_secs(1), 64, 64),
        )
        .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"configured input");
    }

    #[cfg(unix)]
    #[test]
    fn output_with_limits_kills_and_reaps_timed_out_child() {
        let mut child = Command::new("sleep");
        child.arg("5");
        let started = Instant::now();

        let error = output_with_limits(
            &mut child,
            OutputLimits::new(Duration::from_millis(100), 64, 64),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn output_with_limits_cancellation_kills_and_reaps_child() {
        let cancellation = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancellation);
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel.store(true, Ordering::Release);
        });
        let mut child = Command::new("sleep");
        child.arg("5");
        let started = Instant::now();

        let error = output_with_limits_and_cancellation(
            &mut child,
            OutputLimits::new(Duration::from_secs(10), 64, 64),
            cancellation.as_ref(),
        )
        .unwrap_err();
        canceller.join().unwrap();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn output_with_limits_kills_descendant_holding_pipe_after_leader_exits() {
        let temp = tempfile::tempdir().unwrap();
        let started_file = temp.path().join("descendant-started");
        let survived_file = temp.path().join("descendant-survived");
        let mut child = Command::new("/bin/sh");
        child
            .args([
                "-c",
                concat!(
                    "printf started > \"$NUCLEOTIDE_TEST_STARTED_FILE\"; ",
                    "(sleep 2; printf survived > \"$NUCLEOTIDE_TEST_SURVIVED_FILE\") &",
                ),
            ])
            .env("NUCLEOTIDE_TEST_STARTED_FILE", &started_file)
            .env("NUCLEOTIDE_TEST_SURVIVED_FILE", &survived_file);
        let started = Instant::now();

        let error = output_with_limits(
            &mut child,
            OutputLimits::new(Duration::from_millis(750), 64, 64),
        )
        .unwrap_err();
        let elapsed = started.elapsed();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(
            started_file.exists(),
            "shell leader did not start the background descendant"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "background descendant kept output pipes open for {elapsed:?}"
        );

        thread::sleep(Duration::from_millis(2_100));
        assert!(
            !survived_file.exists(),
            "background descendant survived process-group cleanup"
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_cancellation_kills_descendant_holding_pipe_after_leader_exits() {
        let temp = tempfile::tempdir().unwrap();
        let started_file = temp.path().join("descendant-started");
        let survived_file = temp.path().join("descendant-survived");
        let cancellation = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancellation);
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            cancel.store(true, Ordering::Release);
        });
        let mut child = Command::new("/bin/sh");
        child
            .args([
                "-c",
                concat!(
                    "printf started > \"$NUCLEOTIDE_TEST_STARTED_FILE\"; ",
                    "(sleep 2; printf survived > \"$NUCLEOTIDE_TEST_SURVIVED_FILE\") &",
                ),
            ])
            .env("NUCLEOTIDE_TEST_STARTED_FILE", &started_file)
            .env("NUCLEOTIDE_TEST_SURVIVED_FILE", &survived_file);
        let started = Instant::now();

        let error = output_with_limits_and_cancellation(
            &mut child,
            OutputLimits::new(Duration::from_secs(10), 64, 64),
            cancellation.as_ref(),
        )
        .unwrap_err();
        canceller.join().unwrap();
        let elapsed = started.elapsed();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(
            started_file.exists(),
            "shell leader did not start descendant"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "background descendant kept output pipes open for {elapsed:?}"
        );

        thread::sleep(Duration::from_millis(2_100));
        assert!(
            !survived_file.exists(),
            "background descendant survived cancellation cleanup"
        );
    }
}
