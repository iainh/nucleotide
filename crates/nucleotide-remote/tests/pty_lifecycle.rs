// ABOUTME: Real-helper PTY teardown regressions with backpressured terminal input
// ABOUTME: Checks that cancellation reaps the actual child, not just the client stream

#![cfg(target_os = "linux")]

use nucleotide_remote::{
    ProjectEnvironmentSelection, PtySessionCommand, PtySessionRequest, RemotePtySessionParts,
    RemoteWorkspaceBackendOptions, connect_workspace_backend_for_location,
};
use nucleotide_workspace::classify_workspace_location;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn backpressured_teardown(drop_guard: bool, send_input: bool) {
    let temp = tempfile::tempdir().unwrap();
    let connection = connect_workspace_backend_for_location(
        classify_workspace_location(temp.path()),
        &RemoteWorkspaceBackendOptions {
            use_local_service: true,
            local_helper_path: Some(env!("CARGO_BIN_EXE_nucleotide-remote").into()),
            ..Default::default()
        },
    )
    .unwrap();
    let RemotePtySessionParts {
        mut input,
        mut output,
        completion,
        control,
        lifecycle,
    } = connection
        .pty_session_launcher
        .as_ref()
        .unwrap()
        .launch(PtySessionRequest {
            command: PtySessionCommand::Command {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "stty raw -echo; printf '%s' $$ > terminal.pid; printf READY; exec sleep 60"
                        .into(),
                ],
            },
            cwd: temp.path().into(),
            environment: ProjectEnvironmentSelection::WorkspaceOrNearest {
                anchor: temp.path().into(),
            },
            env_overrides: BTreeMap::new(),
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut ready = [0; 5];
    tokio::time::timeout(Duration::from_secs(5), output.read_exact(&mut ready))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&ready, b"READY");
    let pid: u32 = std::fs::read_to_string(temp.path().join("terminal.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let proc = PathBuf::from(format!("/proc/{pid}"));
    assert_eq!(std::fs::read_link(proc.join("cwd")).unwrap(), temp.path());
    let write = tokio::spawn(async move {
        let bytes = vec![b'x'; if send_input { 2 * 1024 * 1024 } else { 0 }];
        input.write_all(&bytes).await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(write.is_finished(), !send_input);
    if drop_guard {
        drop(lifecycle);
    } else {
        control.cancel();
    }
    let reaped = tokio::time::timeout(Duration::from_secs(3), async {
        while proc.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    if !reaped {
        // Only clean up the disposable child whose identity we established above.
        // A failing implementation may leave it a zombie until helper teardown.
        if std::fs::read_link(proc.join("cwd")).is_ok_and(|cwd| cwd == temp.path()) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
    let settled = tokio::time::timeout(Duration::from_secs(3), write).await;
    let completed = tokio::time::timeout(Duration::from_secs(3), completion).await;
    assert!(
        reaped,
        "PTY child {pid} survived teardown or was not reaped"
    );
    let input_result = settled.expect("pending input did not settle").unwrap();
    if send_input {
        assert!(
            input_result.is_err(),
            "backpressured input unexpectedly completed"
        );
    } else {
        input_result.unwrap();
    }
    assert!(
        completed
            .expect("session completion did not settle")
            .is_err()
    );
    // Keep the connection alive through the assertion: helper shutdown must not
    // conceal a session-specific cancellation failure.
    assert!(connection.backend.stat(temp.path()).await.is_ok());
}

#[tokio::test]
async fn cancellation_reaps_backpressured_pty() {
    backpressured_teardown(false, true).await;
}

#[tokio::test]
async fn guard_drop_reaps_backpressured_pty() {
    backpressured_teardown(true, true).await;
}

#[tokio::test]
async fn cancellation_reaps_pty_without_input() {
    backpressured_teardown(false, false).await;
}

#[tokio::test]
async fn input_shutdown_preserves_raw_terminal_eof_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let connection = connect_workspace_backend_for_location(
        classify_workspace_location(temp.path()),
        &RemoteWorkspaceBackendOptions {
            use_local_service: true,
            local_helper_path: Some(env!("CARGO_BIN_EXE_nucleotide-remote").into()),
            ..Default::default()
        },
    )
    .unwrap();
    let mut session = connection.pty_session_launcher.as_ref().unwrap().launch(PtySessionRequest {
        command: PtySessionCommand::Command {
            program: "/usr/bin/python3".into(),
            args: vec!["-c".into(), "import os, signal, tty\nsignal.alarm(10)\ntty.setraw(0)\nos.write(1, b'READY')\ndata = b''\nwhile len(data) < 2: data += os.read(0, 2-len(data))\nassert data == b'\\n\\x04', repr(data)\nos.write(1, b'DONE')".into()],
        },
        cwd: temp.path().into(),
        environment: ProjectEnvironmentSelection::Bare,
        env_overrides: BTreeMap::new(),
        cols: 80, rows: 24, pixel_width: 0, pixel_height: 0,
    }).unwrap();
    let mut ready = [0; 5];
    tokio::time::timeout(
        Duration::from_secs(5),
        session.output.read_exact(&mut ready),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&ready, b"READY");
    session.input.shutdown().await.unwrap();
    let mut tail = Vec::new();
    let (completion, output) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(session.completion, session.output.read_to_end(&mut tail))
    })
    .await
    .expect("raw EOF bytes were not delivered");
    assert!(completion.unwrap().success);
    output.unwrap();
    assert_eq!(tail, b"DONE");
    assert!(connection.backend.stat(temp.path()).await.is_ok());
}

#[tokio::test]
async fn duplex_progress_preserves_input_output_and_eof() {
    use sha2::{Digest, Sha256};

    const OUTPUT: usize = 4 * 1024 * 1024;
    // The child writes more than the PTY and reader queue can hold before reading
    // input. Large concurrent input must not prevent the worker forwarding output.
    for (input_size, output_first) in [(OUTPUT, true), (1024, false), (OUTPUT, false)] {
        let temp = tempfile::tempdir().unwrap();
        let connection = connect_workspace_backend_for_location(
            classify_workspace_location(temp.path()),
            &RemoteWorkspaceBackendOptions {
                use_local_service: true,
                local_helper_path: Some(env!("CARGO_BIN_EXE_nucleotide-remote").into()),
                ..Default::default()
            },
        )
        .unwrap();
        let code = format!(
            r#"import os, signal, tty, time, hashlib
signal.alarm(20)
tty.setraw(0)
os.write(1, b'READY')
while not os.path.exists('go'): time.sleep(.005)
data = b'0123456789abcdef' * {blocks}
while data:
    n = os.write(1, data)
    data = data[n:]
h = hashlib.sha256()
got = 0
while got < {input_size}:
    data = os.read(0, min(65536, {input_size} - got))
    assert data
    h.update(data)
    got += len(data)
open('input.result', 'w').write(str(got) + ' ' + h.hexdigest())
eof = b''
while len(eof) < 2: eof += os.read(0, 2 - len(eof))
assert eof == b'\n\x04', repr(eof)
"#,
            blocks = OUTPUT / 16,
        );
        let RemotePtySessionParts {
            mut input,
            mut output,
            completion,
            control: _,
            lifecycle,
        } = connection
            .pty_session_launcher
            .as_ref()
            .unwrap()
            .launch(PtySessionRequest {
                command: PtySessionCommand::Command {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-c".into(), code],
                },
                cwd: temp.path().into(),
                environment: ProjectEnvironmentSelection::Bare,
                env_overrides: BTreeMap::new(),
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut ready = [0; 5];
        tokio::time::timeout(Duration::from_secs(5), output.read_exact(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&ready, b"READY");
        let bytes = vec![b'x'; input_size];
        let expected_input = format!("{} {:x}", input_size, Sha256::digest(&bytes));
        let write = async {
            input.write_all(&bytes).await?;
            input.shutdown().await
        };
        tokio::pin!(write);
        let mut received = Vec::new();
        if output_first {
            std::fs::write(temp.path().join("go"), "go").unwrap();
            received.resize(OUTPUT, 0);
            tokio::time::timeout(Duration::from_secs(5), output.read_exact(&mut received))
                .await
                .unwrap()
                .unwrap();
        } else {
            // Poll the actual write while the child is gated, establishing the
            // large-input backpressure before allowing it to produce output.
            if input_size == OUTPUT {
                assert!(
                    tokio::time::timeout(Duration::from_millis(200), &mut write)
                        .await
                        .is_err(),
                    "large input must be backpressured before releasing the child"
                );
            }
            std::fs::write(temp.path().join("go"), "go").unwrap();
        }
        let (written, completed, read) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(write, completion, output.read_to_end(&mut received))
        })
        .await
        .unwrap_or_else(|_| {
            panic!("duplex stalled: input={input_size}, output_first={output_first}")
        });
        written.unwrap();
        let status = completed.unwrap();
        assert!(status.success, "{status:?}");
        assert_eq!(status.status_code, Some(0));
        read.unwrap();
        assert_eq!(received, b"0123456789abcdef".repeat(OUTPUT / 16));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("input.result")).unwrap(),
            expected_input
        );
        assert!(connection.backend.stat(temp.path()).await.is_ok());
        drop(lifecycle);
    }
}
