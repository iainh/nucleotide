use futures::executor::block_on;
use nucleotide_remote::{local_service_command, spawn_child_process_workspace_backend};
use nucleotide_workspace::{
    ProcessSpec, ReadOptions, RemoteWorkspaceIdentity, RemoteWorkspaceKind,
};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const PROCESS_HELPER_MODE: &str = "NUCLEOTIDE_V5_LOOPBACK_PROCESS_HELPER";
const PROCESS_HELPER_STARTED: &str = "NUCLEOTIDE_V5_LOOPBACK_PROCESS_STARTED";
const PROCESS_HELPER_RELEASE: &str = "NUCLEOTIDE_V5_LOOPBACK_PROCESS_RELEASE";
const PROCESS_READY_OUTPUT: &str = "nucleotide-v5-loopback-process-ready";
const PROCESS_RELEASED_OUTPUT: &str = "nucleotide-v5-loopback-process-released";

#[test]
fn remote_helper_persists_command_failures_on_the_helper_host() {
    let temp = tempfile::tempdir().unwrap();
    let log_directory = temp.path().join("logs");
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_nucleotide-remote"));

    let output = Command::new(helper)
        .arg("invalid-test-command")
        .env("NUCLEOTIDE_LOG_DIR", &log_directory)
        .env("NUCLEOTIDE_LOG", "info")
        .env_remove("NUCLEOTIDE_LOG_NO_FILE")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "protocol stdout must remain clean"
    );

    let log_path = std::fs::read_dir(&log_directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("nucleotide-remote.log."))
        })
        .expect("nucleotide-remote should create a dated host log");
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(log.contains("Nucleotide remote host logging initialized"));
    assert!(log.contains("nucleotide-remote command failed"));
    assert!(log.contains("unknown nucleotide-remote command"));
}

#[test]
fn v5_loopback_process_helper() {
    if std::env::var_os(PROCESS_HELPER_MODE).is_none() {
        return;
    }

    let started = required_helper_path(PROCESS_HELPER_STARTED);
    let release = required_helper_path(PROCESS_HELPER_RELEASE);

    println!("{PROCESS_READY_OUTPUT}");
    std::io::stdout().flush().unwrap();
    std::fs::write(&started, b"ready").unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while !release.is_file() {
        assert!(
            Instant::now() < deadline,
            "loopback process helper was not released"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    println!("{PROCESS_RELEASED_OUTPUT}");
    std::io::stdout().flush().unwrap();
}

#[test]
fn real_v5_helper_multiplexes_file_requests_while_process_is_running() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let file = workspace.join("probe.txt");
    std::fs::write(&file, b"loopback fixture contents").unwrap();

    let helper = PathBuf::from(env!("CARGO_BIN_EXE_nucleotide-remote"));
    let command = local_service_command(&helper, &workspace);
    let (backend, hello) = spawn_child_process_workspace_backend(
        RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Other("loopback-fixture".to_string()),
            name: "loopback-fixture".to_string(),
        },
        &command,
    )
    .expect("real v5 helper should start over local stdio");
    assert_eq!(hello.workspace_root, workspace);

    let started = temp.path().join("process-started");
    let release = temp.path().join("process-release");
    let process_executable = std::env::current_exe().unwrap();
    let mut process_env = BTreeMap::new();
    process_env.insert(PROCESS_HELPER_MODE.to_string(), "1".to_string());
    process_env.insert(
        PROCESS_HELPER_STARTED.to_string(),
        path_for_process_env(&started),
    );
    process_env.insert(
        PROCESS_HELPER_RELEASE.to_string(),
        path_for_process_env(&release),
    );

    let process_backend = backend.clone();
    let process_workspace = workspace.clone();
    let (process_sender, process_receiver) = mpsc::channel();
    let process_thread = std::thread::spawn(move || {
        let result = block_on(process_backend.run_process(ProcessSpec {
            program: process_executable.to_string_lossy().into_owned(),
            args: vec![
                "--exact".to_string(),
                "v5_loopback_process_helper".to_string(),
                "--nocapture".to_string(),
            ],
            cwd: process_workspace,
            env: process_env,
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: Some(1024 * 1024),
            timeout_ms: Some(20_000),
        }))
        .map_err(|error| error.to_string());
        process_sender.send(result).unwrap();
    });

    wait_for_process_marker(&started, &process_receiver);

    let metadata_backend = backend.clone();
    let metadata_file = file.clone();
    let (metadata_sender, metadata_receiver) = mpsc::channel();
    let metadata_started = Instant::now();
    let metadata_thread = std::thread::spawn(move || {
        let result = block_on(async {
            let stat = metadata_backend.stat(&metadata_file).await?;
            let read = metadata_backend
                .read_file(&metadata_file, ReadOptions::default())
                .await?;
            Ok::<_, nucleotide_workspace::WorkspaceError>((stat, read))
        })
        .map_err(|error| error.to_string());
        metadata_sender.send(result).unwrap();
    });

    let metadata = metadata_receiver.recv_timeout(Duration::from_secs(5));
    std::fs::write(&release, b"release").unwrap();
    let (stat, read) = metadata
        .expect("stat and read should complete while process.run remains active")
        .expect("stat and read should succeed through the real v5 helper");
    assert!(
        metadata_started.elapsed() < Duration::from_secs(5),
        "multiplexed stat and read were not prompt"
    );
    assert_eq!(stat.path, file);
    assert_eq!(read.path, file);
    assert_eq!(read.bytes, b"loopback fixture contents");

    let output = process_receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("released process request should complete")
        .expect("remote process should succeed");
    assert!(output.success, "remote test process failed: {output:?}");
    assert!(!output.timed_out);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(PROCESS_READY_OUTPUT), "stdout: {stdout}");
    assert!(stdout.contains(PROCESS_RELEASED_OUTPUT), "stdout: {stdout}");

    metadata_thread.join().unwrap();
    process_thread.join().unwrap();
}

fn required_helper_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} must be set in process-helper mode"))
}

fn path_for_process_env(path: &Path) -> String {
    path.to_str()
        .expect("temporary fixture paths must be valid Unicode")
        .to_string()
}

fn wait_for_process_marker(
    started: &Path,
    process_receiver: &mpsc::Receiver<Result<nucleotide_workspace::ProcessOutput, String>>,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !started.is_file() {
        if let Ok(result) = process_receiver.try_recv() {
            panic!("remote process completed before its start marker: {result:?}");
        }
        assert!(
            Instant::now() < deadline,
            "remote process did not publish its start marker"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
