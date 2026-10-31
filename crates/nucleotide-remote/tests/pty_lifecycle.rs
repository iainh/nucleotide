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
