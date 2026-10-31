// ABOUTME: Real-helper PTY descendant teardown and reader ownership regressions
// ABOUTME: Keeps the connection alive while checking process and master/thread cleanup

#![cfg(target_os = "linux")]

use nucleotide_remote::{
    ProjectEnvironmentSelection, PtySessionCommand, PtySessionRequest, RemotePtySessionParts,
    RemoteWorkspaceBackendOptions, WorkspaceBackendConnection,
    connect_workspace_backend_for_location,
};
use nucleotide_workspace::classify_workspace_location;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// Subreaper state is process-wide. Every test in this binary takes this lock,
// and restores the previous state only after its owned processes are cleaned up.
static SERIAL: Mutex<()> = Mutex::new(());
unsafe extern "C" {
    fn prctl(option: i32, ...) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
}

struct Subreaper(i32);
impl Subreaper {
    fn new() -> Self {
        let mut previous = 0i32;
        // SAFETY: PR_GET_CHILD_SUBREAPER writes an int to a valid pointer.
        assert_eq!(unsafe { prctl(37, &mut previous as *mut i32) }, 0);
        // SAFETY: PR_SET_CHILD_SUBREAPER takes an integer boolean.
        assert_eq!(unsafe { prctl(36, 1usize) }, 0);
        Self(previous)
    }
}
impl Drop for Subreaper {
    fn drop(&mut self) {
        // SAFETY: restore the process-wide setting while still holding SERIAL.
        let result = unsafe { prctl(36, self.0 as usize) };
        assert_eq!(result, 0);
    }
}

fn children() -> Vec<i32> {
    fs::read_dir("/proc/self/task")
        .unwrap()
        .flatten()
        .flat_map(|task| {
            fs::read_to_string(task.path().join("children"))
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|pid| pid.parse().ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn exists(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn resources(pid: i32) -> (usize, usize) {
    let masters = fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .flatten()
        .filter(|fd| {
            fs::read_link(fd.path()).is_ok_and(|target| target.to_string_lossy().contains("ptmx"))
        })
        .count();
    let threads = fs::read_dir(format!("/proc/{pid}/task")).unwrap().count();
    (masters, threads)
}

async fn eventually(label: &str, mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("deadline waiting for {label}"));
}

async fn marker(root: &Path, name: &str) -> i32 {
    let path = root.join(name);
    eventually(name, || {
        fs::read_to_string(&path).is_ok_and(|s| s.trim().parse::<i32>().is_ok())
    })
    .await;
    fs::read_to_string(path).unwrap().trim().parse().unwrap()
}

struct Fixture {
    connection: WorkspaceBackendConnection,
    helper: i32,
    baseline: (usize, usize),
    root: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        assert!(Path::new("/usr/bin/python3").is_file());
        let root = tempfile::tempdir().unwrap();
        let before = children();
        let connection = connect_workspace_backend_for_location(
            classify_workspace_location(root.path()),
            &RemoteWorkspaceBackendOptions {
                use_local_service: true,
                local_helper_path: Some(env!("CARGO_BIN_EXE_nucleotide-remote").into()),
                ..Default::default()
            },
        )
        .unwrap();
        let fresh: Vec<_> = children()
            .into_iter()
            .filter(|pid| !before.contains(pid))
            .collect();
        assert_eq!(fresh.len(), 1, "must identify exactly our new helper child");
        let helper = fresh[0];
        let baseline = resources(helper);
        let mut fixture = Self {
            connection,
            helper,
            baseline,
            root,
        };
        // Connection construction can return before the helper starts its I/O
        // threads. A completed request establishes the idle resource baseline.
        fixture.responsive().await;
        fixture.baseline = resources(helper);
        fixture
    }

    async fn responsive(&self) {
        assert!(
            tokio::time::timeout(
                Duration::from_secs(3),
                self.connection.backend.stat(self.root.path())
            )
            .await
            .expect("retained helper stopped responding")
            .is_ok()
        );
    }

    fn launch(&self, name: &str, different_group: bool, status: i32) -> RemotePtySessionParts {
        let code = format!(
            r#"import os, signal, time, tty
signal.alarm(20)
tty.setraw(0)
signal.signal(signal.SIGHUP, signal.SIG_IGN)
open('{name}.parent.pid', 'w').write(str(os.getpid()))
pid = os.fork()
if pid == 0:
    signal.alarm(20)
    if {different_group}: os.setpgid(0, 0)
    open('{name}.holder.pid', 'w').write(str(os.getpid()))
    while True: time.sleep(1)
while not os.path.exists('{name}.go'): time.sleep(.01)
data = b'0123456789abcdef' * 2048
while data:
    n = os.write(1, data)
    data = data[n:]
os._exit({status})
"#,
            different_group = if different_group { "True" } else { "False" }
        );
        self.connection
            .pty_session_launcher
            .as_ref()
            .unwrap()
            .launch(PtySessionRequest {
                command: PtySessionCommand::Command {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-c".into(), code],
                },
                cwd: self.root.path().into(),
                environment: ProjectEnvironmentSelection::Bare,
                env_overrides: BTreeMap::new(),
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap()
    }

    async fn pids(&self, name: &str, different_group: bool) -> (i32, i32) {
        let parent = marker(self.root.path(), &format!("{name}.parent.pid")).await;
        let holder = marker(self.root.path(), &format!("{name}.holder.pid")).await;
        // Python's comm field contains no spaces: fields 5/6 are pgrp/session.
        for (pid, group) in [
            (parent, parent),
            (holder, if different_group { holder } else { parent }),
        ] {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
            let fields: Vec<_> = stat.split_whitespace().collect();
            assert_eq!(fields[4].parse::<i32>().unwrap(), group);
            assert_eq!(fields[5].parse::<i32>().unwrap(), parent);
            assert_eq!(
                fs::read_link(format!("/proc/{pid}/cwd")).unwrap(),
                self.root.path()
            );
        }
        (parent, holder)
    }

    async fn recovered(&self) {
        eprintln!(
            "helper={}: resources after completion {:?}, baseline {:?}",
            self.helper,
            resources(self.helper),
            self.baseline
        );
        eventually("helper master FD and thread baseline", || {
            resources(self.helper) == self.baseline
        })
        .await;
        self.responsive().await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let mut owned = Vec::new();
        for entry in fs::read_dir(self.root.path()).unwrap().flatten() {
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "pid")
            {
                if let Ok(pid) = fs::read_to_string(entry.path())
                    .unwrap_or_default()
                    .trim()
                    .parse::<i32>()
                {
                    // Kill only live disposable processes whose cwd proves ownership.
                    if fs::read_link(format!("/proc/{pid}/cwd"))
                        .is_ok_and(|cwd| cwd == self.root.path())
                    {
                        // SAFETY: positive PID identified using this unique temporary directory.
                        unsafe {
                            kill(pid, 9);
                        }
                    }
                    owned.push(pid);
                }
            }
        }
        // The retained connection owns this helper; no process-wide PID scan.
        // SAFETY: helper was identified as our freshly spawned direct child.
        unsafe {
            kill(self.helper, 9);
        }
        owned.push(self.helper);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            owned.retain(|pid| {
                // SAFETY: nonblocking reap of an explicitly owned PID, never waitpid(-1).
                unsafe {
                    waitpid(*pid, std::ptr::null_mut(), 1);
                }
                exists(*pid)
            });
            if owned.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(owned.is_empty(), "owned PIDs survived cleanup: {owned:?}");
    }
}

async fn reap_holder(holder: i32) {
    eventually("owned orphan holder termination and reap", || {
        // SAFETY: reap only the adopted PID created by this test's Python child.
        unsafe { waitpid(holder, std::ptr::null_mut(), 1) == holder }
    })
    .await;
    assert!(!exists(holder));
}

#[tokio::test(flavor = "current_thread")]
async fn natural_exit_cleans_same_group_holder_and_preserves_status_and_output() {
    let _serial = SERIAL.lock().unwrap_or_else(|poison| poison.into_inner());
    let _subreaper = Subreaper::new();
    let fixture = Fixture::new().await;
    for status in [0, 7] {
        let name = format!("exit{status}");
        let mut session = fixture.launch(&name, false, status);
        let (parent, holder) = fixture.pids(&name, false).await;
        fs::write(fixture.root.path().join(format!("{name}.go")), "go").unwrap();
        let mut bytes = Vec::new();
        let (completion, output) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(session.completion, session.output.read_to_end(&mut bytes))
        })
        .await
        .expect("natural PTY completion/output deadline");
        output.unwrap();
        let completion = completion.unwrap();
        assert_eq!(completion.status_code, Some(status));
        assert_eq!(completion.success, status == 0);
        assert_eq!(bytes, b"0123456789abcdef".repeat(2048));
        assert!(!exists(parent), "direct child was not reaped");
        eprintln!("exit={status}: original status and 32768 output bytes preserved; parent reaped");
        reap_holder(holder).await;
        fixture.recovered().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn different_job_group_survives_but_does_not_retain_reader() {
    let _serial = SERIAL.lock().unwrap_or_else(|poison| poison.into_inner());
    let _subreaper = Subreaper::new();
    let fixture = Fixture::new().await;
    let mut session = fixture.launch("job", true, 0);
    let (parent, holder) = fixture.pids("job", true).await;
    fs::write(fixture.root.path().join("job.go"), "go").unwrap();
    let mut bytes = Vec::new();
    let (completion, output) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(session.completion, session.output.read_to_end(&mut bytes))
    })
    .await
    .expect("different-group completion/output deadline");
    assert!(completion.unwrap().success);
    output.unwrap();
    assert_eq!(bytes, b"0123456789abcdef".repeat(2048));
    assert!(!exists(parent));
    fixture.recovered().await;
    assert!(
        exists(holder),
        "original-PGID policy must not kill another job group"
    );
    assert_eq!(
        fs::read_link(format!("/proc/{holder}/cwd")).unwrap(),
        fixture.root.path()
    );
    // SAFETY: the still-live owned holder was just identity-checked above.
    assert_eq!(unsafe { kill(holder, 9) }, 0);
    reap_holder(holder).await;
}

#[tokio::test(flavor = "current_thread")]
async fn natural_exit_interrupts_input_with_another_job_group() {
    let _serial = SERIAL.lock().unwrap_or_else(|poison| poison.into_inner());
    let _subreaper = Subreaper::new();
    let fixture = Fixture::new().await;
    let mut session = fixture.launch("pending", true, 7);
    let (parent, holder) = fixture.pids("pending", true).await;
    let mut write =
        tokio::spawn(async move { session.input.write_all(&vec![b'x'; 2 * 1024 * 1024]).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut write)
            .await
            .is_err()
    );
    fs::write(fixture.root.path().join("pending.go"), "go").unwrap();
    let mut bytes = Vec::new();
    let (completion, output) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(session.completion, session.output.read_to_end(&mut bytes))
    })
    .await
    .expect("natural exit must interrupt backpressured input");
    assert_eq!(completion.unwrap().status_code, Some(7));
    output.unwrap();
    assert_eq!(bytes, b"0123456789abcdef".repeat(2048));
    // AsyncWrite acknowledges transport acceptance, not slave consumption. Input
    // may finish before the successful terminal response or observe its closure.
    let input_result = tokio::time::timeout(Duration::from_secs(3), write)
        .await
        .expect("pending input must settle after natural exit")
        .unwrap();
    eprintln!("pending input after natural exit: {input_result:?}");
    assert!(!exists(parent));
    fixture.recovered().await;
    assert!(exists(holder));
    assert_eq!(
        fs::read_link(format!("/proc/{holder}/cwd")).unwrap(),
        fixture.root.path()
    );
    // SAFETY: the owned surviving different-PGID holder was just identity-checked.
    assert_eq!(unsafe { kill(holder, 9) }, 0);
    reap_holder(holder).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_and_guard_drop_clean_backpressured_same_group_holders() {
    let _serial = SERIAL.lock().unwrap_or_else(|poison| poison.into_inner());
    let _subreaper = Subreaper::new();
    let fixture = Fixture::new().await;
    for drop_guard in [false, true] {
        let name = if drop_guard { "drop" } else { "cancel" };
        let RemotePtySessionParts {
            mut input,
            output: _output,
            completion,
            control,
            lifecycle,
        } = fixture.launch(name, false, 0);
        let (parent, holder) = fixture.pids(name, false).await;
        let mut write =
            tokio::spawn(async move { input.write_all(&vec![b'x'; 2 * 1024 * 1024]).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut write)
                .await
                .is_err(),
            "input must be backpressured before teardown"
        );
        if drop_guard {
            drop(lifecycle);
        } else {
            control.cancel();
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(3), completion)
                .await
                .expect("cancel completion deadline")
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(3), write)
                .await
                .expect("blocked write did not settle")
                .unwrap()
                .is_err()
        );
        eventually("cancelled direct child reap", || !exists(parent)).await;
        reap_holder(holder).await;
        fixture.recovered().await;
    }
}
