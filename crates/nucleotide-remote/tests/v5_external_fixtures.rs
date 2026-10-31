// These fixtures execute the host-built helper as though it were installed on a
// remote Linux machine, so they are only valid when the host helper is Linux.
#![cfg(target_os = "linux")]

use futures::executor::block_on;
use nucleotide_remote::{
    RemoteHelperInstallPolicy, RemoteWorkspaceBackendOptions,
    connect_workspace_backend_for_location,
};
use nucleotide_workspace::{
    FileSearchQuery, ProcessSpec, ReadOptions, SshWorkspaceTarget, TextSearchQuery,
    WorkspaceLocation, WorkspaceWatch, WorkspaceWatchBatch, WorkspaceWatchRequest,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

static PATH_LOCK: Mutex<()> = Mutex::new(());

struct PathOverride {
    original: Option<OsString>,
}

impl PathOverride {
    fn prepend(directory: &Path) -> Self {
        let original = std::env::var_os("PATH");
        let mut paths = vec![directory.to_path_buf()];
        if let Some(original) = original.as_ref() {
            paths.extend(std::env::split_paths(original));
        }
        let value = std::env::join_paths(paths).expect("test PATH should be joinable");
        // SAFETY: Tests that mutate PATH hold PATH_LOCK for the full guard lifetime.
        unsafe {
            std::env::set_var("PATH", value);
        }
        Self { original }
    }
}

impl Drop for PathOverride {
    fn drop(&mut self) {
        // SAFETY: Tests that mutate PATH hold PATH_LOCK for the full guard lifetime.
        unsafe {
            if let Some(original) = self.original.take() {
                std::env::set_var("PATH", original);
            } else {
                std::env::remove_var("PATH");
            }
        }
    }
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, contents).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn install_fake_ssh(directory: &Path) {
    write_executable(
        &directory.join("ssh"),
        "#!/bin/sh\n\
         last=\n\
         for arg do\n\
         \tlast=$arg\n\
         done\n\
         if [ -z \"$last\" ]; then\n\
         \techo 'fake ssh did not receive a remote command' >&2\n\
         \texit 64\n\
         fi\n\
         exec /bin/sh -c \"$last\"\n",
    );
}

#[cfg(unix)]
fn install_fake_wsl(directory: &Path) {
    write_executable(
        &directory.join("wsl.exe"),
        "#!/bin/sh\n\
         while [ \"$#\" -gt 0 ]; do\n\
         \tcase \"$1\" in\n\
         \t--distribution)\n\
         \t\tshift 2\n\
         \t\t;;\n\
         \t--cd)\n\
         \t\tcd \"$2\" || exit 1\n\
         \t\tshift 2\n\
         \t\t;;\n\
         \t--exec)\n\
         \t\tshift\n\
         \t\texec \"$@\"\n\
         \t\t;;\n\
         \t*)\n\
         \t\tshift\n\
         \t\t;;\n\
         \tesac\n\
         done\n\
         echo 'fake wsl.exe did not receive --exec' >&2\n\
         exit 64\n",
    );
}

#[cfg(unix)]
fn path_fixture(directory: &Path, installer: fn(&Path)) -> (MutexGuard<'static, ()>, PathOverride) {
    let guard = PATH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    installer(directory);
    let path = PathOverride::prepend(directory);
    (guard, path)
}

fn wait_for_watch_batch(watch: &WorkspaceWatch) -> WorkspaceWatchBatch {
    let started = Instant::now();
    loop {
        match watch.recv_timeout(Duration::from_millis(250)) {
            Ok(batch) => return batch,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                if started.elapsed() < Duration::from_secs(4) => {}
            Err(error) => panic!("watch did not deliver a batch: {error}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn ssh_v5_fixture_exercises_watch_search_process_and_path_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let launcher_dir = temp.path().join("bin");
    std::fs::create_dir(&launcher_dir).unwrap();
    let (_path_guard, _path_override) = path_fixture(&launcher_dir, install_fake_ssh);

    let helper = PathBuf::from(env!("CARGO_BIN_EXE_nucleotide-remote"));
    let deployed_helper = temp.path().join("ssh-cache").join("nucleotide-remote");
    let workspace = temp.path().join("workspace");
    let src = workspace.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("main.rs"), "fn main() { println!(\"needle\"); }\n").unwrap();

    let display_root = PathBuf::from("ssh://me@devbox/home/me/project");
    let location = WorkspaceLocation::Ssh {
        original_path: display_root.clone(),
        target: SshWorkspaceTarget {
            host: "devbox".to_string(),
            user: Some("me".to_string()),
            port: None,
        },
        path: workspace.clone(),
    };
    let options = RemoteWorkspaceBackendOptions {
        ssh_helper_path: Some(deployed_helper.clone()),
        ssh_helper_path_is_override: true,
        ssh_helper_install_policy: RemoteHelperInstallPolicy::Upload,
        ssh_helper_upload_path: Some(helper),
        ssh_connect_timeout_secs: Some(1),
        ssh_control_path: None,
        ..RemoteWorkspaceBackendOptions::default()
    };
    let connection = connect_workspace_backend_for_location(location, &options)
        .expect("ssh v5 fixture should connect through fake ssh");
    assert!(
        deployed_helper.is_file(),
        "SSH helper should be uploaded before the service starts"
    );
    assert_eq!(connection.hello.unwrap().workspace_root, workspace);
    let backend = connection.backend;

    let listing = block_on(backend.list_dir(&display_root)).unwrap();
    assert_eq!(listing.path, display_root);
    assert!(
        listing
            .entries
            .iter()
            .any(|entry| entry.path == display_root.join("src"))
    );

    let read_path = display_root.join("src").join("main.rs");
    let read = block_on(backend.read_file(&read_path, ReadOptions::default())).unwrap();
    assert_eq!(read.path, read_path);
    assert!(String::from_utf8_lossy(&read.bytes).contains("needle"));

    let search = block_on(backend.text_search(TextSearchQuery {
        root: display_root.clone(),
        pattern: "needle".to_string(),
        limit: 20,
        hidden: true,
        ..TextSearchQuery::default()
    }))
    .unwrap();
    assert_eq!(search.root, display_root);
    assert_eq!(search.matches.len(), 1);

    let output = block_on(backend.run_process(ProcessSpec {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            "printf ssh-stdout; printf ssh-stderr >&2".to_string(),
        ],
        cwd: display_root.clone(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        stdin: Vec::new(),
        max_output_bytes: None,
        timeout_ms: Some(5_000),
    }))
    .unwrap();
    assert!(output.success);
    assert_eq!(output.stdout, b"ssh-stdout");
    assert_eq!(output.stderr, b"ssh-stderr");

    let watched_root = display_root.join("missing");
    let watch = block_on(backend.start_watch(WorkspaceWatchRequest {
        roots: vec![watched_root.clone()],
        debounce_ms: 50,
        max_events_per_batch: 500,
    }))
    .unwrap()
    .expect("v5 helper should advertise watch");
    std::fs::create_dir(workspace.join("missing")).unwrap();
    let batch = wait_for_watch_batch(&watch);
    assert!(
        batch.events.iter().any(|event| event.path == watched_root),
        "watch batch should be mapped to the ssh display root: {batch:?}"
    );
    block_on(backend.stop_watch(watch.watch_id)).unwrap();
}

/// Uses real clangd, but only simulates the WSL launcher. Set
/// NUCLEOTIDE_TEST_SSH_CONFIG to an OpenSSH config containing a `devbox` host
/// to exercise real SSH too. That host must be this machine (shared temp paths).
#[test]
#[ignore = "requires clangd; optionally a loopback SSH devbox host"]
fn remote_lsp_clangd_exercises_startup_editing_navigation_and_shutdown() {
    use nucleotide_remote::RemotePipedProcessSessionRequest;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, BufReader};

    let clangd = std::env::var("NUCLEOTIDE_TEST_CLANGD").unwrap_or_else(|_| "clangd".into());
    assert!(
        std::process::Command::new(&clangd)
            .arg("--version")
            .status()
            .unwrap()
            .success()
    );
    let ssh_config = std::env::var_os("NUCLEOTIDE_TEST_SSH_CONFIG");
    for wsl in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let launcher_dir = temp.path().join("bin");
        std::fs::create_dir(&launcher_dir).unwrap();
        let (_guard, _path) = path_fixture(
            &launcher_dir,
            if wsl {
                install_fake_wsl
            } else {
                install_fake_ssh
            },
        );
        // Real SSH must not resolve the fixture's fake ssh executable.
        if !wsl && ssh_config.is_some() {
            std::fs::remove_file(launcher_dir.join("ssh")).unwrap();
        }
        let connection_root = temp.path().join("remote workspace");
        let workspace = connection_root.join("project with spaces");
        std::fs::create_dir_all(&workspace).unwrap();
        let remote_bin = workspace.join("remote-bin");
        std::fs::create_dir(&remote_bin).unwrap();
        // This command is absent from the host PATH. Both cwd and overrides must
        // reach the helper for startup to succeed; stdout must stay pure LSP.
        write_executable(
            &remote_bin.join("remote-clangd"),
            &format!(
                "#!/bin/sh\n[ \"$PWD\" = \"$EXPECTED_CWD\" ] || exit 71\n[ \"$LSP_TEST\" = remote ] || exit 72\necho $$ > server.pid\nexec '{}' --background-index=0 --clang-tidy=0 --log=error\n",
                clangd.replace('\'', "'\\''")
            ),
        );
        let display_root = if wsl {
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project")
        } else {
            PathBuf::from("ssh://devbox/home/me/project")
        };
        let location = if wsl {
            WorkspaceLocation::Wsl {
                original_path: display_root.clone(),
                distro: "Ubuntu".into(),
                linux_path: connection_root.clone(),
            }
        } else {
            WorkspaceLocation::Ssh {
                original_path: display_root.clone(),
                target: SshWorkspaceTarget {
                    host: "devbox".into(),
                    user: None,
                    port: None,
                },
                path: connection_root,
            }
        };
        let display_root = location.path_mapping().to_display_path(&workspace);
        let mut options = RemoteWorkspaceBackendOptions {
            remote_helper_path: PathBuf::from(env!("CARGO_BIN_EXE_nucleotide-remote")),
            remote_helper_path_is_override: true,
            ssh_helper_install_policy: RemoteHelperInstallPolicy::Never,
            wsl_helper_install_policy: RemoteHelperInstallPolicy::Never,
            ssh_control_path: None,
            ..RemoteWorkspaceBackendOptions::default()
        };
        if !wsl && let Some(config) = &ssh_config {
            options.ssh_extra_args = vec!["-F".into(), config.clone()];
        }
        let connection = connect_workspace_backend_for_location(location, &options).unwrap();
        let launcher = connection
            .process_session_launcher
            .as_ref()
            .expect("process_sessions_v1");
        // Model the mixed separators produced by Windows PathBuf::join. The
        // actual Linux helper must receive normalized cwd and anchor fields.
        let process_root = if wsl {
            PathBuf::from(format!(
                "{}\\project with spaces",
                connection.location.native_root().display()
            ))
        } else {
            connection
                .location
                .path_mapping()
                .to_native_path(&display_root)
        };
        let request = RemotePipedProcessSessionRequest {
            program: "remote-clangd".into(),
            args: vec![],
            cwd: process_root.clone(),
            environment_anchor: process_root,
            env_overrides: BTreeMap::from([
                (
                    "PATH".into(),
                    format!("{}:/usr/bin:/bin", remote_bin.display()),
                ),
                (
                    "EXPECTED_CWD".into(),
                    workspace.to_string_lossy().into_owned(),
                ),
                ("LSP_TEST".into(), "remote".into()),
            ]),
        };
        let mut parts = launcher.launch(request.clone()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(30), async {
                let stderr = tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    parts.stderr.read_to_end(&mut bytes).await.unwrap();
                    bytes
                });
                let mut stdout = BufReader::new(parts.stdout);
                // Encode spaces as a real file URI, rather than a remote display URI.
                let root_uri = format!("file://{}", workspace.display()).replace(' ', "%20");
                let uri = format!("{root_uri}/main.c");
                send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                    "processId":null,"rootUri":root_uri,"capabilities":{"textDocument":{"publishDiagnostics":{"versionSupport":true}}},
                    "workspaceFolders":[{"uri":root_uri,"name":"remote test"}]
                }})).await;
                let initialized = receive_lsp_until(&mut stdout, |m| m["id"] == 1).await;
                assert!(initialized["result"]["capabilities"]["definitionProvider"].as_bool().unwrap());
                send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","method":"initialized","params":{}})).await;
                let broken = "int remote_answer(void) { return 42; }\nint main(void) { return missing_value; }\n";
                std::fs::write(workspace.join("main.c"), broken).unwrap();
                send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                    "textDocument":{"uri":uri,"languageId":"c","version":1,"text":broken}
                }})).await;
                let diagnostics = receive_lsp_until(&mut stdout, |m| m["method"] == "textDocument/publishDiagnostics" && m["params"]["version"] == 1).await;
                assert_eq!(diagnostics["params"]["uri"], uri);
                assert!(diagnostics["params"]["diagnostics"].as_array().unwrap().iter().any(|d| d["message"].as_str().unwrap().contains("missing_value")));
                // File requests must still work while a full-duplex LSP is alive.
                let read = connection.backend.read_file(&display_root.join("main.c"), ReadOptions::default()).await.unwrap();
                assert_eq!(read.bytes, broken.as_bytes());
                let fixed = "int remote_answer(void) { return 42; }\nint main(void) { return remote_answer(); }\n";
                send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{
                    "textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":fixed}]
                }})).await;
                let diagnostics = receive_lsp_until(&mut stdout, |m| m["method"] == "textDocument/publishDiagnostics" && m["params"]["version"] == 2).await;
                assert_eq!(diagnostics["params"]["diagnostics"], json!([]));
                for (id, method) in [(2, "textDocument/hover"), (3, "textDocument/definition")] {
                    send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","id":id,"method":method,"params":{
                        "textDocument":{"uri":uri},"position":{"line":1,"character":28}
                    }})).await;
                    let response = receive_lsp_until(&mut stdout, |m| m["id"] == id).await;
                    if id == 2 {
                        assert!(response["result"]["contents"].to_string().contains("remote_answer"), "{response}");
                    } else {
                        assert_eq!(response["result"][0]["uri"], uri);
                        assert_eq!(response["result"][0]["range"]["start"]["line"], 0);
                    }
                }
                let partial = "int remote_answer(void) { return 42; }\nint main(void) { return remote_ans; }\n";
                send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{
                    "textDocument":{"uri":uri,"version":3},"contentChanges":[{"text":partial}]
                }})).await;
                send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","id":4,"method":"textDocument/completion","params":{
                    "textDocument":{"uri":uri},"position":{"line":1,"character":33}
                }})).await;
                let completion = receive_lsp_until(&mut stdout, |m| m["id"] == 4).await;
                assert!(completion["result"]["items"].as_array().unwrap().iter().any(|item| item["label"].as_str().unwrap().contains("remote_answer")), "{completion}");
                send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","id":5,"method":"shutdown","params":null})).await;
                let shutdown = receive_lsp_until(&mut stdout, |m| m["id"] == 5).await;
                assert_eq!(shutdown.get("result"), Some(&serde_json::Value::Null));
                send_lsp(&mut parts.stdin, json!({"jsonrpc":"2.0","method":"exit","params":null})).await;
                let completion = parts.completion.await.unwrap();
                let stderr = stderr.await.unwrap();
                assert!(completion.success, "{completion:?}: {}", String::from_utf8_lossy(&stderr));
                let pid_path = workspace.join("server.pid");
                let pid = std::fs::read_to_string(&pid_path).unwrap();
                assert!(!Path::new("/proc").join(pid.trim()).exists(), "LSP survived clean exit");

                let missing = launcher.launch(RemotePipedProcessSessionRequest {
                    program: "nucleotide-nonexistent-lsp".into(), ..request.clone()
                }).unwrap();
                let error = missing.completion.await.unwrap_err().to_string();
                assert!(error.contains("failed to spawn process.session"), "{error}");

                // Dropping an active client must terminate its server, and a
                // failed startup must not prevent the next server from starting.
                std::fs::remove_file(&pid_path).unwrap();
                let cancelled = launcher.launch(request.clone()).unwrap();
                while !std::fs::read_to_string(&pid_path).is_ok_and(|pid| !pid.trim().is_empty()) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let pid = std::fs::read_to_string(&pid_path).unwrap();
                assert!(Path::new("/proc").join(pid.trim()).exists());
                drop(cancelled.lifecycle);
                assert!(cancelled.completion.await.is_err());
                while Path::new("/proc").join(pid.trim()).exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                connection.backend.stat(&display_root).await.unwrap();
                eprintln!("{}: remote-only PATH/cwd/env, initialize, diagnostics, didChange, hover, definition, completion, concurrent file read, clean exit, missing server, restart and cancellation cleanup passed", if wsl { "simulated WSL" } else if ssh_config.is_some() { "real SSH" } else { "simulated SSH" });
            }).await.expect("remote LSP exchange exceeded 30 seconds");
        });
    }
}

async fn send_lsp(
    writer: &mut nucleotide_remote::RemoteProcessSessionWriter,
    message: serde_json::Value,
) {
    use tokio::io::AsyncWriteExt;
    let body = serde_json::to_vec(&message).unwrap();
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .unwrap();
    writer.write_all(&body).await.unwrap();
    writer.flush().await.unwrap();
}

async fn receive_lsp_until(
    reader: &mut tokio::io::BufReader<nucleotide_remote::RemoteProcessSessionReader>,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    loop {
        let mut length = None;
        loop {
            let mut header = String::new();
            assert_ne!(
                reader.read_line(&mut header).await.unwrap(),
                0,
                "LSP stdout closed before response"
            );
            if header == "\r\n" {
                break;
            }
            if let Some(value) = header.strip_prefix("Content-Length:") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; length.expect("LSP Content-Length")];
        reader.read_exact(&mut body).await.unwrap();
        let message = serde_json::from_slice(&body).unwrap();
        if predicate(&message) {
            return message;
        }
    }
}

#[cfg(unix)]
#[test]
fn wsl_v5_fixture_exercises_path_mapping_and_watch_availability() {
    let temp = tempfile::tempdir().unwrap();
    let launcher_dir = temp.path().join("bin");
    std::fs::create_dir(&launcher_dir).unwrap();
    let (_path_guard, _path_override) = path_fixture(&launcher_dir, install_fake_wsl);

    let helper = PathBuf::from(env!("CARGO_BIN_EXE_nucleotide-remote"));
    let deployed_helper = temp.path().join("wsl-cache").join("nucleotide-remote");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src").join("lib.rs"), "pub fn needle() {}\n").unwrap();

    let display_root = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project");
    let location = WorkspaceLocation::Wsl {
        original_path: display_root.clone(),
        distro: "Ubuntu".to_string(),
        linux_path: workspace.clone(),
    };
    let options = RemoteWorkspaceBackendOptions {
        wsl_helper_path: Some(deployed_helper.clone()),
        wsl_helper_path_is_override: true,
        wsl_helper_install_policy: RemoteHelperInstallPolicy::Upload,
        ssh_helper_upload_path: Some(helper),
        ssh_control_path: None,
        ..RemoteWorkspaceBackendOptions::default()
    };
    let connection = connect_workspace_backend_for_location(location, &options)
        .expect("wsl v5 fixture should connect through fake wsl.exe");
    assert!(
        deployed_helper.is_file(),
        "WSL helper should be uploaded before the service starts"
    );
    assert_eq!(connection.hello.unwrap().workspace_root, workspace);
    let backend = connection.backend;

    let listing = block_on(backend.list_dir(&display_root)).unwrap();
    assert_eq!(listing.path, display_root);
    assert!(
        listing.entries.iter().any(|entry| {
            entry.path.to_string_lossy() == r"\\wsl.localhost\Ubuntu\home\me\project\src"
        }),
        "listing should map native paths to WSL display paths: {listing:?}"
    );

    let files = block_on(backend.file_search(FileSearchQuery {
        root: display_root.clone(),
        pattern: Some("lib".to_string()),
        limit: 10,
        hidden: true,
        ..FileSearchQuery::default()
    }))
    .unwrap();
    assert_eq!(files.root, display_root);
    assert_eq!(files.files, vec![PathBuf::from("src/lib.rs")]);

    let watched_root = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project\missing");
    let watch = block_on(backend.start_watch(WorkspaceWatchRequest {
        roots: vec![watched_root],
        debounce_ms: 50,
        max_events_per_batch: 500,
    }))
    .unwrap()
    .expect("v5 helper should advertise watch");
    std::fs::create_dir(workspace.join("missing")).unwrap();
    let batch = wait_for_watch_batch(&watch);
    assert!(
        batch.events.iter().any(|event| {
            let path = event.path.to_string_lossy();
            path == r"\\wsl.localhost\Ubuntu\home\me\project\missing"
        }),
        "watch batch should be mapped to the WSL display root: {batch:?}"
    );
    block_on(backend.stop_watch(watch.watch_id)).unwrap();
}
