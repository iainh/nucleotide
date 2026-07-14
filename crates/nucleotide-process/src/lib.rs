use std::{
    ffi::OsStr,
    io::{self, Read},
    path::Path,
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
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;

pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    hide_window(&mut command);
    command
}

/// Builds a command whose Windows creation flags can be preserved during contained startup.
///
/// Use this builder with [`ContainedChild::spawn`], [`output_with_limits_contained`], or
/// [`output_with_limits_contained_and_cancellation`]. The tracked flag mask lets the Windows path
/// add `CREATE_SUSPENDED` for Job Object assignment and restore the exact caller-selected mask even
/// when spawning fails.
pub struct ContainedCommand {
    command: Command,
    #[cfg(windows)]
    creation_flags: u32,
}

pub fn contained_command(program: impl AsRef<OsStr>) -> ContainedCommand {
    let mut command = Command::new(program);
    hide_window(&mut command);
    ContainedCommand {
        command,
        #[cfg(windows)]
        creation_flags: CREATE_NO_WINDOW,
    }
}

impl ContainedCommand {
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.command.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command.args(args);
        self
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.command.env(key, value);
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.command.envs(vars);
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.command.env_remove(key);
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.command.env_clear();
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.command.current_dir(dir);
        self
    }

    pub fn stdin(&mut self, stdin: impl Into<Stdio>) -> &mut Self {
        self.command.stdin(stdin);
        self
    }

    pub fn stdout(&mut self, stdout: impl Into<Stdio>) -> &mut Self {
        self.command.stdout(stdout);
        self
    }

    pub fn stderr(&mut self, stderr: impl Into<Stdio>) -> &mut Self {
        self.command.stderr(stderr);
        self
    }

    #[cfg(windows)]
    /// Sets the Windows creation-flag mask preserved by contained startup.
    ///
    /// `CREATE_SUSPENDED` is reserved because contained startup adds and later resumes that state.
    #[track_caller]
    pub fn creation_flags(&mut self, flags: u32) -> &mut Self {
        use std::os::windows::process::CommandExt as _;

        assert_eq!(
            flags & CREATE_SUSPENDED,
            0,
            "ContainedCommand reserves CREATE_SUSPENDED"
        );
        self.command.creation_flags(flags);
        self.creation_flags = flags;
        self
    }

    fn as_std_mut(&mut self) -> &mut Command {
        &mut self.command
    }

    #[cfg(windows)]
    fn spawn_suspended(&mut self) -> io::Result<Child> {
        use std::os::windows::process::CommandExt as _;

        self.command
            .creation_flags(self.creation_flags | CREATE_SUSPENDED);
        let child = self.command.spawn();
        self.command.creation_flags(self.creation_flags);
        child
    }
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
/// On Unix the child starts a new process group. Other platforms terminate the direct child. Use
/// [`output_with_limits_contained`] when Windows-side descendants must be contained as well. The
/// caller remains responsible for configuring stdin, including setting it to [`Stdio::null`] for
/// commands that should receive immediate EOF.
pub fn output_with_limits(command: &mut Command, limits: OutputLimits) -> io::Result<Output> {
    output_with_limits_inner(command, limits, None)
}

/// Runs a tracked command with bounded output and descendant containment.
///
/// On Unix the child starts a new process group. On Windows it starts suspended, enters an unnamed
/// Job Object, and only then resumes. Successful completion releases containment so intentional
/// detached descendants such as OpenSSH `ControlPersist` can remain alive.
pub fn output_with_limits_contained(
    command: &mut ContainedCommand,
    limits: OutputLimits,
) -> io::Result<Output> {
    output_with_limits_inner(command, limits, None)
}

/// Runs a child with bounded output and stops it when cancellation is requested.
///
/// Cancellation is cooperative at a polling interval of at most 10 ms. The same reaping guarantees
/// as [`output_with_limits`] apply.
pub fn output_with_limits_and_cancellation(
    command: &mut Command,
    limits: OutputLimits,
    cancellation: &AtomicBool,
) -> io::Result<Output> {
    output_with_limits_inner(command, limits, Some(cancellation))
}

/// Runs a tracked command with bounded output, cancellation, and descendant containment.
///
/// Cancellation is cooperative at a polling interval of at most 10 ms. Timeout, cancellation and
/// output-limit failures terminate the contained process tree and reap the direct child.
pub fn output_with_limits_contained_and_cancellation(
    command: &mut ContainedCommand,
    limits: OutputLimits,
    cancellation: &AtomicBool,
) -> io::Result<Output> {
    output_with_limits_inner(command, limits, Some(cancellation))
}

trait OutputCommand {
    fn as_std_mut(&mut self) -> &mut Command;
    fn spawn_output_child(&mut self) -> io::Result<OutputChildGuard>;
}

impl OutputCommand for Command {
    fn as_std_mut(&mut self) -> &mut Command {
        self
    }

    fn spawn_output_child(&mut self) -> io::Result<OutputChildGuard> {
        OutputChildGuard::spawn_standard(self)
    }
}

impl OutputCommand for ContainedCommand {
    fn as_std_mut(&mut self) -> &mut Command {
        self.as_std_mut()
    }

    fn spawn_output_child(&mut self) -> io::Result<OutputChildGuard> {
        OutputChildGuard::spawn_contained(self)
    }
}

fn output_with_limits_inner<C: OutputCommand>(
    command: &mut C,
    limits: OutputLimits,
    cancellation: Option<&AtomicBool>,
) -> io::Result<Output> {
    if cancellation.is_some_and(|cancellation| cancellation.load(Ordering::Acquire)) {
        return Err(cancelled_output_error());
    }
    command
        .as_std_mut()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn_output_child()?;
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
    child.disarm()?;
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
fn configure_contained_child(command: &mut ContainedCommand) {
    use std::os::unix::process::CommandExt as _;

    command.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
fn configure_contained_child(_command: &mut ContainedCommand) {}

/// A child process whose local descendants are terminated with it.
///
/// Unix containment uses a process group. Windows containment uses a Job Object and starts the
/// process suspended so user code cannot create descendants before assignment. Dropping this
/// value requests termination but does not wait; callers that need reaping must call [`Self::wait`].
pub struct ContainedChild {
    child: Child,
    #[cfg(unix)]
    process_group_id: u32,
    #[cfg(windows)]
    job: WindowsJob,
    containment_released: bool,
    termination_requested: bool,
}

impl ContainedChild {
    /// Spawns a child inside the platform's descendant-containment primitive.
    pub fn spawn(command: &mut ContainedCommand) -> io::Result<Self> {
        configure_contained_child(command);

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle as _;

            let job = WindowsJob::new()?;
            let child = command.spawn_suspended()?;
            let child_id = child.id();
            let process_handle = child.as_raw_handle();
            let mut contained = Self {
                child,
                job,
                containment_released: false,
                termination_requested: false,
            };
            let setup = contained
                .job
                .assign(process_handle)
                .and_then(|()| resume_only_suspended_thread(child_id));
            if let Err(setup_error) = setup {
                let termination = contained.terminate();
                let wait = wait_after_termination(contained.child_mut(), &termination);
                return Err(combine_setup_cleanup_error(setup_error, termination, wait));
            }
            Ok(contained)
        }

        #[cfg(not(windows))]
        {
            let child = command.as_std_mut().spawn()?;
            #[cfg(unix)]
            let process_group_id = child.id();

            Ok(Self {
                child,
                #[cfg(unix)]
                process_group_id,
                containment_released: false,
                termination_requested: false,
            })
        }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Requests termination of the child and its local descendants without waiting for exit.
    pub fn terminate(&mut self) -> io::Result<()> {
        if self.containment_released || self.termination_requested {
            return Ok(());
        }

        #[cfg(unix)]
        let termination = terminate_process_group(&mut self.child, self.process_group_id);
        #[cfg(windows)]
        let termination = if self.job.is_assigned() {
            self.job
                .terminate()
                .or_else(|job_error| terminate_direct_child(&mut self.child).and(Err(job_error)))
        } else {
            terminate_direct_child(&mut self.child)
        };
        #[cfg(not(any(unix, windows)))]
        let termination = terminate_direct_child(&mut self.child);

        if termination.is_ok() {
            self.termination_requested = true;
        }
        termination
    }

    /// Waits for and reaps the direct child.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    fn release_containment(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        self.job.release()?;

        self.containment_released = true;
        Ok(())
    }
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        if !self.containment_released && !self.termination_requested {
            let _ = self.terminate();
        }
    }
}

struct OutputChildGuard {
    child: Option<OutputChild>,
}

enum OutputChild {
    Standard {
        child: Child,
        #[cfg(unix)]
        process_group_id: u32,
    },
    Contained(ContainedChild),
}

impl OutputChildGuard {
    fn spawn_standard(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        let child = command.spawn()?;
        #[cfg(unix)]
        let process_group_id = child.id();

        Ok(Self {
            child: Some(OutputChild::Standard {
                child,
                #[cfg(unix)]
                process_group_id,
            }),
        })
    }

    fn spawn_contained(command: &mut ContainedCommand) -> io::Result<Self> {
        Ok(Self {
            child: Some(OutputChild::Contained(ContainedChild::spawn(command)?)),
        })
    }

    fn child_mut(&mut self) -> &mut Child {
        match self
            .child
            .as_mut()
            .expect("child guard was used after being disarmed")
        {
            OutputChild::Standard { child, .. } => child,
            OutputChild::Contained(child) => child.child_mut(),
        }
    }

    fn disarm(&mut self) -> io::Result<()> {
        if let Some(OutputChild::Contained(child)) = self.child.as_mut() {
            child.release_containment()?;
        }
        self.child.take();
        Ok(())
    }

    fn terminate_and_reap(&mut self) -> io::Result<ExitStatus> {
        let Some(child) = self.child.take() else {
            return Err(io::Error::other("child guard was already disarmed"));
        };

        let (termination, wait) = match child {
            OutputChild::Standard {
                mut child,
                #[cfg(unix)]
                process_group_id,
            } => {
                #[cfg(unix)]
                let termination = terminate_process_group(&mut child, process_group_id);
                #[cfg(not(unix))]
                let termination = terminate_direct_child(&mut child);
                let wait = wait_after_termination(&mut child, &termination);
                (termination, wait)
            }
            OutputChild::Contained(mut child) => {
                let termination = child.terminate();
                let wait = wait_after_termination(child.child_mut(), &termination);
                (termination, wait)
            }
        };
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

impl Drop for OutputChildGuard {
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

fn wait_after_termination(
    child: &mut Child,
    termination: &io::Result<()>,
) -> io::Result<ExitStatus> {
    if termination.is_ok() {
        child.wait()
    } else {
        child.try_wait()?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "skipped blocking reap after child termination failed",
            )
        })
    }
}

#[cfg(windows)]
struct WindowsJob {
    handle: std::os::windows::io::OwnedHandle,
    assigned: bool,
}

#[cfg(windows)]
impl WindowsJob {
    fn new() -> io::Result<Self> {
        use std::os::windows::io::FromRawHandle as _;
        use windows_sys::Win32::System::JobObjects::CreateJobObjectW;

        // SAFETY: Null security attributes and name request a private, non-inheritable Job Object.
        let raw_handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw_handle.is_null() {
            return Err(windows_last_error("failed to create child Job Object"));
        }
        // SAFETY: CreateJobObjectW returned a new owned handle and null was rejected above.
        let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw_handle) };
        let job = Self {
            handle,
            assigned: false,
        };
        job.set_kill_on_close(true)?;
        Ok(job)
    }

    fn assign(&mut self, process_handle: std::os::windows::io::RawHandle) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        // SAFETY: Both handles remain owned and live for the duration of this call. The process is
        // still suspended, so it cannot create an escaping descendant before assignment finishes.
        let assigned =
            unsafe { AssignProcessToJobObject(self.handle.as_raw_handle(), process_handle) };
        if assigned == 0 {
            return Err(windows_last_error(
                "failed to assign suspended child to Job Object",
            ));
        }
        self.assigned = true;
        Ok(())
    }

    fn is_assigned(&self) -> bool {
        self.assigned
    }

    fn terminate(&self) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: The Job Object handle is valid and remains owned by self.
        if unsafe { TerminateJobObject(self.handle.as_raw_handle(), 1) } == 0 {
            Err(windows_last_error("failed to terminate child Job Object"))
        } else {
            Ok(())
        }
    }

    fn release(&self) -> io::Result<()> {
        self.set_kill_on_close(false)
    }

    fn set_kill_on_close(&self, enabled: bool) -> io::Result<()> {
        use std::mem::size_of_val;
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::JobObjects::{
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        if enabled {
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        }
        let limits_len = u32::try_from(size_of_val(&limits)).map_err(|_| {
            io::Error::other(format!(
                "Job Object limit structure exceeded u32: {} bytes",
                size_of_val(&limits)
            ))
        })?;
        // SAFETY: limits has the layout required by JobObjectExtendedLimitInformation and remains
        // live for the call. The handle is a live Job Object owned by self.
        let configured = unsafe {
            SetInformationJobObject(
                self.handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                limits_len,
            )
        };
        if configured == 0 {
            Err(windows_last_error(if enabled {
                "failed to enable kill-on-close for child Job Object"
            } else {
                "failed to release child Job Object after successful completion"
            }))
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
fn resume_only_suspended_thread(process_id: u32) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    const SNAPSHOT_ATTEMPTS: usize = 10;
    const INVALID_RESUME_COUNT: u32 = u32::MAX;

    let mut thread_id = None;
    for attempt in 0..SNAPSHOT_ATTEMPTS {
        thread_id = only_process_thread_id(process_id)?;
        if thread_id.is_some() {
            break;
        }
        if attempt + 1 < SNAPSHOT_ATTEMPTS {
            thread::sleep(Duration::from_millis(1));
        }
    }
    let thread_id = thread_id.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("suspended child {process_id} did not expose its primary thread"),
        )
    })?;

    // SAFETY: The discovered thread belongs to the child we just created and remains suspended.
    let raw_thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if raw_thread.is_null() {
        return Err(windows_last_error(
            "failed to open suspended child primary thread",
        ));
    }
    // SAFETY: OpenThread returned a new owned handle and null was rejected above.
    let thread_handle = unsafe { OwnedHandle::from_raw_handle(raw_thread) };
    // SAFETY: The handle grants THREAD_SUSPEND_RESUME and is live for this call.
    let previous_count = unsafe { ResumeThread(thread_handle.as_raw_handle()) };
    match previous_count {
        1 => Ok(()),
        INVALID_RESUME_COUNT => Err(windows_last_error(
            "failed to resume suspended child primary thread",
        )),
        count => Err(io::Error::other(format!(
            "suspended child primary thread had unexpected suspend count {count}"
        ))),
    }
}

#[cfg(windows)]
fn only_process_thread_id(process_id: u32) -> io::Result<Option<u32>> {
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
    };

    // SAFETY: The snapshot flags and process identifier are plain values. A thread snapshot
    // ignores the process identifier and enumerates threads system-wide.
    let raw_snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw_snapshot == INVALID_HANDLE_VALUE {
        return Err(windows_last_error(
            "failed to snapshot suspended child threads",
        ));
    }
    // SAFETY: CreateToolhelp32Snapshot returned a new owned handle and the invalid sentinel was
    // rejected above.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw_snapshot) };
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(size_of::<THREADENTRY32>()).expect("THREADENTRY32 size fits in u32"),
        ..THREADENTRY32::default()
    };
    // SAFETY: entry has the documented size and remains writable for the call.
    if unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } == 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
            Ok(None)
        } else {
            Err(io::Error::new(
                error.kind(),
                format!("failed to enumerate suspended child threads: {error}"),
            ))
        };
    }

    let mut matching_thread = None;
    loop {
        if entry.th32OwnerProcessID == process_id
            && matching_thread.replace(entry.th32ThreadID).is_some()
        {
            return Err(io::Error::other(format!(
                "suspended child {process_id} exposed more than one thread before resume"
            )));
        }

        // SAFETY: entry and snapshot satisfy the same invariants as Thread32First above.
        if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                Ok(matching_thread)
            } else {
                Err(io::Error::new(
                    error.kind(),
                    format!("failed to continue suspended child thread enumeration: {error}"),
                ))
            };
        }
    }
}

#[cfg(windows)]
fn windows_last_error(context: &str) -> io::Error {
    let error = io::Error::last_os_error();
    io::Error::new(error.kind(), format!("{context}: {error}"))
}

#[cfg(windows)]
fn combine_setup_cleanup_error(
    setup_error: io::Error,
    termination: io::Result<()>,
    wait: io::Result<ExitStatus>,
) -> io::Error {
    match (termination, wait) {
        (Ok(()), Ok(_)) => setup_error,
        (termination, wait) => {
            let mut message = format!("{setup_error}");
            if let Err(error) = termination {
                message.push_str(&format!("; failed to terminate setup child: {error}"));
            }
            if let Err(error) = wait {
                message.push_str(&format!("; failed to reap setup child: {error}"));
            }
            io::Error::new(setup_error.kind(), message)
        }
    }
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

    #[cfg(windows)]
    const WINDOWS_HELPER_ROLE: &str = "NUCLEOTIDE_PROCESS_TEST_ROLE";
    #[cfg(windows)]
    const WINDOWS_HELPER_STARTED: &str = "NUCLEOTIDE_PROCESS_TEST_STARTED";
    #[cfg(windows)]
    const WINDOWS_HELPER_SURVIVED: &str = "NUCLEOTIDE_PROCESS_TEST_SURVIVED";
    #[cfg(windows)]
    const WINDOWS_HELPER_RELEASE: &str = "NUCLEOTIDE_PROCESS_TEST_RELEASE";

    #[cfg(windows)]
    #[test]
    #[allow(clippy::zombie_processes)] // The helper deliberately leaves a Job-owned descendant.
    fn windows_descendant_helper() {
        let Ok(role) = std::env::var(WINDOWS_HELPER_ROLE) else {
            return;
        };
        let started = std::env::var_os(WINDOWS_HELPER_STARTED)
            .map(std::path::PathBuf::from)
            .expect("Windows descendant helper requires a started marker");
        let survived = std::env::var_os(WINDOWS_HELPER_SURVIVED)
            .map(std::path::PathBuf::from)
            .expect("Windows descendant helper requires a survived marker");
        let release = std::env::var_os(WINDOWS_HELPER_RELEASE)
            .map(std::path::PathBuf::from)
            .expect("Windows descendant helper requires a release marker");

        match role.as_str() {
            "leader-piped" | "leader-detached" => {
                let mut descendant =
                    windows_raw_helper_command("descendant", &started, &survived, &release);
                descendant.stdin(Stdio::null());
                if role == "leader-detached" {
                    descendant.stdout(Stdio::null()).stderr(Stdio::null());
                } else {
                    descendant.stdout(Stdio::inherit()).stderr(Stdio::inherit());
                }
                descendant.spawn().unwrap();
            }
            "descendant" => {
                std::fs::write(&started, b"started").unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                while !release.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                if release.exists() {
                    std::fs::write(&survived, b"survived").unwrap();
                }
            }
            other => panic!("unknown Windows descendant helper role {other}"),
        }
    }

    #[cfg(windows)]
    fn windows_raw_helper_command(
        role: &str,
        started: &std::path::Path,
        survived: &std::path::Path,
        release: &std::path::Path,
    ) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "tests::windows_descendant_helper", "--nocapture"])
            .env(WINDOWS_HELPER_ROLE, role)
            .env(WINDOWS_HELPER_STARTED, started)
            .env(WINDOWS_HELPER_SURVIVED, survived)
            .env(WINDOWS_HELPER_RELEASE, release);
        command
    }

    #[cfg(windows)]
    fn windows_contained_helper_command(
        role: &str,
        started: &std::path::Path,
        survived: &std::path::Path,
        release: &std::path::Path,
    ) -> ContainedCommand {
        let mut command = contained_command(std::env::current_exe().unwrap());
        command
            .args(["--exact", "tests::windows_descendant_helper", "--nocapture"])
            .env(WINDOWS_HELPER_ROLE, role)
            .env(WINDOWS_HELPER_STARTED, started)
            .env(WINDOWS_HELPER_SURVIVED, survived)
            .env(WINDOWS_HELPER_RELEASE, release);
        command
    }

    #[cfg(windows)]
    fn release_windows_descendant_and_assert_killed(
        release: &std::path::Path,
        survived: &std::path::Path,
    ) {
        std::fs::write(release, b"release").unwrap();
        thread::sleep(Duration::from_secs(2));
        assert!(
            !survived.exists(),
            "Windows descendant survived Job Object cleanup"
        );
    }

    #[cfg(windows)]
    #[test]
    fn output_with_limits_kills_windows_descendant_holding_pipe() {
        let temp = tempfile::tempdir().unwrap();
        let started_file = temp.path().join("descendant-started");
        let survived_file = temp.path().join("descendant-survived");
        let release_file = temp.path().join("release-descendant");
        let mut child = windows_contained_helper_command(
            "leader-piped",
            &started_file,
            &survived_file,
            &release_file,
        );

        let error = output_with_limits_contained(
            &mut child,
            OutputLimits::new(Duration::from_secs(3), 4 * 1024, 4 * 1024),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(
            started_file.exists(),
            "Windows descendant did not start before the output deadline"
        );
        release_windows_descendant_and_assert_killed(&release_file, &survived_file);
    }

    #[cfg(windows)]
    #[test]
    fn output_cancellation_kills_windows_descendant_holding_pipe() {
        let temp = tempfile::tempdir().unwrap();
        let started_file = temp.path().join("descendant-started");
        let survived_file = temp.path().join("descendant-survived");
        let release_file = temp.path().join("release-descendant");
        let cancellation = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancellation);
        let observed_started = started_file.clone();
        let canceller = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !observed_started.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            cancel.store(true, Ordering::Release);
        });
        let mut child = windows_contained_helper_command(
            "leader-piped",
            &started_file,
            &survived_file,
            &release_file,
        );

        let error = output_with_limits_contained_and_cancellation(
            &mut child,
            OutputLimits::new(Duration::from_secs(10), 4 * 1024, 4 * 1024),
            cancellation.as_ref(),
        )
        .unwrap_err();
        canceller.join().unwrap();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(
            started_file.exists(),
            "Windows descendant did not start before cancellation"
        );
        release_windows_descendant_and_assert_killed(&release_file, &survived_file);
    }

    #[cfg(windows)]
    #[test]
    fn successful_output_releases_windows_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let started_file = temp.path().join("descendant-started");
        let survived_file = temp.path().join("descendant-survived");
        let release_file = temp.path().join("release-descendant");
        let mut child = windows_contained_helper_command(
            "leader-detached",
            &started_file,
            &survived_file,
            &release_file,
        );

        let output = output_with_limits_contained(
            &mut child,
            OutputLimits::new(Duration::from_secs(5), 4 * 1024, 4 * 1024),
        )
        .unwrap();

        assert!(output.status.success());
        std::fs::write(&release_file, b"release").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !survived_file.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            started_file.exists(),
            "detached Windows descendant did not start"
        );
        assert!(
            survived_file.exists(),
            "successful output cleanup killed a released Windows descendant"
        );
    }

    #[cfg(windows)]
    #[test]
    fn contained_spawn_restores_command_creation_flags() {
        let mut command = contained_command("cmd.exe");
        command.args(["/D", "/Q", "/C", "exit 0"]);

        let mut contained = ContainedChild::spawn(&mut command).unwrap();
        assert!(contained.wait().unwrap().success());
        drop(contained);

        let mut reused = command.command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = reused.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                reused.kill().unwrap();
                reused.wait().unwrap();
                panic!("reused contained command remained suspended");
            }
            thread::sleep(Duration::from_millis(10));
        }
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
