#![cfg(unix)]

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
