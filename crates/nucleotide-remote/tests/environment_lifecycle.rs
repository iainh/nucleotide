// ABOUTME: Real-helper cancellation while process and PTY environment activation is gated
// ABOUTME: Controls prove activated variables reach commands; teardown must not need gate release

#![cfg(target_os = "linux")]

use nucleotide_remote::{
    ProcessSessionRequest, ProjectEnvironmentSelection, PtySessionCommand, PtySessionRequest,
    RemoteWorkspaceBackendOptions, RemoteWorkspaceProtocolClient,
    RemoteWorkspaceV5MultiplexedClient, connect_workspace_backend_for_location, protocol_v5,
};
use nucleotide_workspace::classify_workspace_location;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

const LIMIT: Duration = Duration::from_secs(3);

struct Fixture {
    child: Option<Child>,
    root: tempfile::TempDir,
}

fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while !condition() {
        if started.elapsed() >= LIMIT {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

impl Fixture {
    fn activation_process(&self) -> PathBuf {
        let pid = fs::read_to_string(self.root.path().join("activation.pid")).unwrap();
        PathBuf::from(format!("/proc/{}", pid.trim().parse::<u32>().unwrap()))
    }

    fn wait_exit(&mut self) -> bool {
        wait_until(|| self.child.as_mut().unwrap().try_wait().unwrap().is_some())
    }

    fn release(&self) {
        fs::write(self.root.path().join("release"), "release").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Unblock even on assertion failure, then kill only our identified activation
        // group. The shell is independently bounded in case teardown itself fails.
        let _ = fs::write(self.root.path().join("release"), "release");
        if let Ok(pid) = fs::read_to_string(self.root.path().join("activation.pid"))
            && let Ok(pid) = pid.trim().parse::<u32>()
            && fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd == self.root.path())
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        } else if let Ok(pid) = fs::read_to_string(self.root.path().join("helper.pid"))
            && let Ok(pid) = pid.trim().parse::<u32>()
            && fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd == self.root.path())
        {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn gated_root() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("devenv.nix"), "{}").unwrap();
    fs::write(
        root.path().join("devenv"),
        r#"#!/bin/sh
[ "$1" = print-dev-env ] && [ "$2" = --json ] || exit 2
printf '%s' $$ > activation.pid
i=0
while [ ! -f release ] && [ "$i" -lt 600 ]; do
  /bin/sleep 0.02
  i=$((i+1))
done
[ -f release ] || exit 3
printf finished > activation.finished
printf '{"variables":{"LIFECYCLE_ENV":{"type":"exported","value":"activated"}}}\n'
"#,
    )
    .unwrap();
    fs::set_permissions(
        root.path().join("devenv"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    root
}

async fn exercise(pty: bool, cancel: bool) {
    let root = gated_root();
    let child = Command::new(env!("CARGO_BIN_EXE_nucleotide-remote"))
        .args(["serve", "--workspace"])
        .arg(root.path())
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", root.path().display()))
        .env("HOME", root.path())
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("NUCLEOTIDE_LOG_NO_FILE", "1")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut fixture = Fixture {
        child: Some(child),
        root,
    };
    let child = fixture.child.as_mut().unwrap();
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(child.stdout.take().unwrap(), child.stdin.take().unwrap()),
        protocol_v5::ClientHello::nucleotide("environment-lifecycle-test"),
    )
    .unwrap();
    let root = fixture.root.path();
    let environment = ProjectEnvironmentSelection::Exact { root: root.into() };
    let args = vec![
        "-c".into(),
        "printf '%s' \"$LIFECYCLE_ENV\" > target.spawned".into(),
    ];
    let session = if pty {
        client
            .open_pty_session(PtySessionRequest {
                command: PtySessionCommand::Command {
                    program: "/bin/sh".into(),
                    args,
                },
                cwd: root.into(),
                environment,
                env_overrides: BTreeMap::new(),
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap()
    } else {
        client
            .open_process_session(ProcessSessionRequest {
                program: "/bin/sh".into(),
                args,
                cwd: root.into(),
                environment,
                env_overrides: BTreeMap::new(),
            })
            .unwrap()
    };
    assert!(wait_until(|| root.join("activation.pid").exists()));
    let activation = fixture.activation_process();
    assert!(
        activation.exists(),
        "activation must be gated before teardown"
    );
    assert!(!root.join("target.spawned").exists());

    if cancel {
        drop(session.lifecycle);
        // Observe the long-lived activation PID, not transient target spawns.
        // Cancellation must reap activation while the gate is STILL CLOSED.
        let activation_stopped = wait_until(|| !activation.exists());
        let gate_stayed_closed = !root.join("activation.finished").exists();
        client.shutdown().unwrap();
        let shutdown_bounded = fixture.wait_exit();
        // Releasing only after both deadlines must not resurrect a cancelled target.
        fixture.release();
        let exited = fixture.wait_exit();
        let target_spawned = fixture.root.path().join("target.spawned").exists();
        client.close();
        eprintln!(
            "pty={pty} activation_stopped_before_release={activation_stopped} gate_stayed_closed={gate_stayed_closed} shutdown_bounded={shutdown_bounded} exited={exited} target_spawned={target_spawned}"
        );
        assert!(
            activation_stopped,
            "cancelled activation survived the closed gate"
        );
        assert!(gate_stayed_closed, "activation escaped the fixture gate");
        assert!(shutdown_bounded, "helper shutdown depended on gate release");
        assert!(exited, "helper did not exit after fixture release");
        assert!(!target_spawned, "cancelled command ran after activation");
    } else {
        fixture.release();
        let completion = tokio::time::timeout(LIMIT, session.completion)
            .await
            .expect("activated command did not complete")
            .unwrap();
        assert!(completion.success);
        assert_eq!(
            fs::read_to_string(fixture.root.path().join("target.spawned")).unwrap(),
            "activated"
        );
        client.shutdown().unwrap();
        assert!(fixture.wait_exit(), "control helper did not exit");
        assert!(!activation.exists(), "control activation was not reaped");
        client.close();
    }
}

#[tokio::test]
async fn cancelling_process_stops_gated_activation() {
    exercise(false, true).await;
}

#[tokio::test]
async fn cancelling_pty_stops_gated_activation() {
    exercise(true, true).await;
}

#[tokio::test]
async fn process_receives_activated_environment() {
    exercise(false, false).await;
}

#[tokio::test]
async fn pty_receives_activated_environment() {
    exercise(true, false).await;
}

#[tokio::test]
async fn pty_eof_survives_full_resize_queue_during_activation() {
    let fixture = Fixture {
        child: None,
        root: gated_root(),
    };
    let root = fixture.root.path();
    // The public launcher exposes resize controls. Isolate its helper environment
    // without changing this test process's environment (other tests run concurrently).
    let wrapper = root.join("helper");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\ncd '{}' || exit 1\nprintf '%s' $$ > helper.pid\nexec /usr/bin/env -i PATH='{}:/usr/bin:/bin' HOME='{}' XDG_CACHE_HOME='{}/cache' NUCLEOTIDE_LOG_NO_FILE=1 '{}' \"$@\"\n",
            root.display(),
            root.display(),
            root.display(),
            root.display(),
            env!("CARGO_BIN_EXE_nucleotide-remote"),
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let connection = connect_workspace_backend_for_location(
        classify_workspace_location(root),
        &RemoteWorkspaceBackendOptions {
            use_local_service: true,
            local_helper_path: Some(wrapper),
            ..Default::default()
        },
    )
    .unwrap();
    let mut session = connection
        .pty_session_launcher
        .as_ref()
        .unwrap()
        .launch(PtySessionRequest {
            command: PtySessionCommand::Command {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "cat >/dev/null; stty size > terminal.size; printf '%s' \"$LIFECYCLE_ENV\" > terminal.finished".into(),
                ],
            },
            cwd: root.into(),
            environment: ProjectEnvironmentSelection::Exact { root: root.into() },
            env_overrides: BTreeMap::new(),
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    assert!(wait_until(|| root.join("activation.pid").exists()));
    // Preparation holds ingress: exactly sixteen controls fill its bounded queue.
    // END must survive even though it cannot enqueue an ordinary EOF sentinel.
    for cols in 81..=96 {
        session.control.resize(cols, 25, 0, 0).unwrap();
    }
    tokio::time::timeout(LIMIT, session.input.shutdown())
        .await
        .expect("PTY END did not flush")
        .unwrap();
    // Ordered request proves all preceding control frames and END reached the
    // same helper before activation can finish; no timing/ptrace interposition.
    tokio::time::timeout(LIMIT, connection.backend.stat(root))
        .await
        .expect("ordered stat barrier timed out")
        .unwrap();
    assert!(!root.join("activation.finished").exists());
    fixture.release();
    let completed = tokio::time::timeout(LIMIT, session.completion).await;
    eprintln!(
        "PTY EOF with 16 queued resizes completed={}",
        completed.is_ok()
    );
    assert!(
        completed
            .expect("PTY lost EOF behind full resize queue")
            .unwrap()
            .success
    );
    assert_eq!(
        fs::read_to_string(root.join("terminal.size"))
            .unwrap()
            .trim(),
        "25 96"
    );
    assert_eq!(
        fs::read_to_string(root.join("terminal.finished")).unwrap(),
        "activated"
    );
}
