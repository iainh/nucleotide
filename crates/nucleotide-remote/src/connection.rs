// ABOUTME: Child-process transport control for protocol v5 remote connections
// ABOUTME: Owns helper process lifecycle, stdio wiring, and command-display quoting

use super::*;

pub(crate) trait V5TransportAbort: Send + Sync {
    fn abort(&self);
}

pub(crate) struct ChildProcessV5Control {
    child: Mutex<Option<nucleotide_process::ContainedChild>>,
    child_id: u32,
    abort_started: AtomicBool,
    reaped: Arc<AtomicBool>,
}

impl ChildProcessV5Control {
    fn new(child: nucleotide_process::ContainedChild) -> Self {
        let child_id = child.id();
        Self {
            child: Mutex::new(Some(child)),
            child_id,
            abort_started: AtomicBool::new(false),
            reaped: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn child_id(&self) -> u32 {
        self.child_id
    }

    fn abort_child(&self) {
        if self
            .abort_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let mut child = self
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(mut child) = child.take() else {
            self.reaped.store(true, Ordering::Release);
            return;
        };

        // Killing is a prompt, non-waiting syscall. Reaping can block, so leave that to a
        // detached thread and never make transport close wait for child teardown.
        let termination_succeeded = match child.terminate() {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    child_id = self.child_id,
                    %error,
                    "Failed to terminate contained v5 child; closing containment without a blocking reap"
                );
                false
            }
        };
        let reaped = Arc::clone(&self.reaped);
        if std::thread::Builder::new()
            .name("nucleotide-v5-child-reaper".to_string())
            .spawn(move || {
                if termination_succeeded {
                    let _ = child.wait();
                } else {
                    let _ = child.child_mut().try_wait();
                }
                drop(child);
                reaped.store(true, Ordering::Release);
            })
            .is_err()
        {
            tracing::warn!(
                child_id = self.child_id,
                "Failed to start v5 child reaper after terminating remote service"
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn was_reaped(&self) -> bool {
        self.reaped.load(Ordering::Acquire)
    }
}

impl V5TransportAbort for ChildProcessV5Control {
    fn abort(&self) {
        self.abort_child();
    }
}

impl Drop for ChildProcessV5Control {
    fn drop(&mut self) {
        self.abort_child();
    }
}

pub struct ChildProcessV5Writer {
    writer: ChildStdin,
    control: Arc<ChildProcessV5Control>,
}

impl ChildProcessV5Writer {
    pub fn child_id(&self) -> u32 {
        self.control.child_id()
    }
}

impl Write for ChildProcessV5Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl Drop for ChildProcessV5Writer {
    fn drop(&mut self) {
        self.control.abort();
    }
}

pub(crate) fn spawn_child_process_v5_io(
    spec: &RemoteServiceCommand,
) -> io::Result<(
    protocol_v5::FramedIo<ChildStdout, ChildProcessV5Writer>,
    Arc<ChildProcessV5Control>,
)> {
    let mut command = spec.contained_command();
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = nucleotide_process::ContainedChild::spawn(&mut command)?;
    let writer = child
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("remote service child did not expose stdin"))?;
    let reader = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("remote service child did not expose stdout"))?;
    let control = Arc::new(ChildProcessV5Control::new(child));

    Ok((
        protocol_v5::FramedIo::new(
            reader,
            ChildProcessV5Writer {
                writer,
                control: Arc::clone(&control),
            },
        ),
        control,
    ))
}

pub(crate) fn quote_posix_shell(value: &str) -> String {
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

pub(crate) fn quote_command_display_arg(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    if value.is_empty() {
        return "''".to_string();
    }

    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(ch, '/' | '.' | '_' | '-' | '=' | ':' | '@' | ',' | '+')
    }) {
        value.into_owned()
    } else {
        quote_posix_shell(&value)
    }
}
