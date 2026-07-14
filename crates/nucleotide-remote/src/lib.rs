// ABOUTME: Framed stdio protocol and service loop for Nucleotide remote workspaces
// ABOUTME: Keeps WSL, SSH, and local service transports on one request model

pub mod protocol_v5;
mod v5_budget;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::{channel::oneshot, executor::block_on};
use ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use notify::Watcher as _;
use nucleotide_env::{EnvironmentOrigin, ProjectEnvironment, ShellEnvironmentError};
use nucleotide_workspace::{
    DirectoryListing, FileKind, FileRead, FileSearchQuery, FileSearchResult, FileStat,
    GitHeadResult, GitStatusEntry, GitStatusKind, GitStatusOptions, GitStatusResult,
    LocalWorkspaceBackend, ProcessOutput, ProcessSpec, ProjectEnvironmentOrigin,
    ProjectEnvironmentSnapshot, ReadOptions, RemoteWorkspaceIdentity, RemoteWorkspaceKind,
    SshWorkspaceTarget, TextSearchMatch, TextSearchQuery, TextSearchResult, WorkspaceBackend,
    WorkspaceBackendHandle, WorkspaceCancellationToken, WorkspaceError, WorkspaceIdentity,
    WorkspaceLocation, WorkspaceWatch, WorkspaceWatchBatch, WorkspaceWatchChange,
    WorkspaceWatchChangeKind, WorkspaceWatchDirectoryGeneration, WorkspaceWatchRequest,
    WorkspaceWatchUpdate, WriteOptions, WriteResult, local_workspace_backend,
    path_mapped_workspace_backend, posix_path_string,
};
use prost::Message as ProstMessage;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize, Serializer, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use v5_budget::{V5Budgeted, V5ByteReservation, V5ConnectionByteBudget};

pub const PROTOCOL_VERSION: u32 = protocol_v5::PROTOCOL_MAJOR;
pub const FRAME_VERSION: u16 = protocol_v5::FRAME_HEADER_VERSION;
pub const MAX_FRAME_BODY_LEN: u64 = protocol_v5::MAX_NEGOTIATED_FRAME_BODY_LEN as u64;
pub const DEFAULT_SSH_CONNECT_TIMEOUT_SECS: u64 = 30;
pub const REMOTE_STARTUP_PROBE_TIMEOUT: Duration = Duration::from_secs(30);
pub const REMOTE_STARTUP_OUTPUT_LIMIT: usize = 64 * 1024;
const REMOTE_REQUEST_SLOW_LOG_MS: u64 = 500;
const REMOTE_TRANSPORT_WAIT_SLOW_LOG_MS: u64 = 100;
const REMOTE_QUEUE_SLOW_LOG_MS: u64 = 100;
const V5_SEARCH_PARTIAL_BATCH_SIZE: usize = 100;
const V5_SEARCH_PARTIAL_INTERVAL_MS: u64 = 50;
const V5_DIRECTORY_DELTA_CACHE_LIMIT: usize = 2048;
const V5_METADATA_WORKER_LIMIT: usize = 16;
const V5_FILE_BODY_WORKER_LIMIT: usize = 8;
const V5_SEARCH_WORKER_LIMIT: usize = 2;
const V5_GIT_ENV_WORKER_LIMIT: usize = 4;
const V5_PROCESS_WORKER_LIMIT: usize = 4;
const V5_DEFAULT_WATCH_EVENTS_PER_BATCH: usize = 500;
const V5_MAX_WATCH_EVENTS_PER_BATCH: usize = 4_096;
const V5_WATCH_BATCH_PAYLOAD_BUDGET: usize = 48 * 1024;
const V5_WATCH_DELIVERY_CAPACITY: usize = 64;
const V5_WATCH_BACKLOG_LIMIT: usize = 64;
const V5_SERVE_OUTPUT_EVENT_CAPACITY: usize = 64;
const V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES: usize = 64 * 1024;
const V5_SERVE_COMPLETION_BYTE_BUDGET: usize = 64 * 1024 * 1024;
const V5_SERVE_SCHEDULER_BACKLOG_LIMIT: usize = 64;
const V5_SERVE_INBOUND_EVENT_CAPACITY: usize = 8;
const V5_NATIVE_WATCH_EVENT_CAPACITY: usize = 256;
const V5_MAX_ACCUMULATED_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const V5_MAX_RAW_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const V5_MAX_REQUEST_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const V5_MAX_REQUEST_BODY_BYTES: usize = 256 * 1024 * 1024;
const V5_REQUEST_CONNECTION_BYTE_BUDGET: usize =
    V5_MAX_REQUEST_PAYLOAD_BYTES + V5_MAX_REQUEST_BODY_BYTES;
const V5_RESPONSE_CONNECTION_BYTE_BUDGET: usize = V5_MAX_ACCUMULATED_RESPONSE_BYTES;
const V5_CLIENT_WRITE_BATCH_FRAMES: usize = 64;
const V5_SERVER_WRITE_BATCH_FRAMES: usize = 64;
const V5_CHILD_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const V5_REQUEST_METADATA_DEADLINE: Duration = Duration::from_secs(60);
const V5_REQUEST_METADATA_INACTIVITY: Duration = Duration::from_secs(30);
const V5_REQUEST_MUTATION_DEADLINE: Duration = Duration::from_secs(120);
const V5_REQUEST_MUTATION_INACTIVITY: Duration = Duration::from_secs(30);
const V5_REQUEST_FILE_DEADLINE: Duration = Duration::from_secs(5 * 60);
const V5_REQUEST_FILE_INACTIVITY: Duration = Duration::from_secs(60);
const V5_REQUEST_SEARCH_DEADLINE: Duration = Duration::from_secs(10 * 60);
const V5_REQUEST_SEARCH_INACTIVITY: Duration = Duration::from_secs(120);
const V5_REQUEST_PROCESS_CANCELLATION_GRACE: Duration = Duration::from_secs(15);
const V5_REQUEST_CONTROL_DEADLINE: Duration = Duration::from_secs(15);
const V5_REQUEST_CONTROL_INACTIVITY: Duration = Duration::from_secs(10);
const REMOTE_HELPER_TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_SSH_CONTROL_PERSIST: &str = "10m";
const DEFAULT_SSH_SERVER_ALIVE_INTERVAL_SECS: u32 = 15;
const DEFAULT_SSH_SERVER_ALIVE_COUNT_MAX: u32 = 3;
const DEFAULT_RELEASE_TAG_PREFIX: &str = "v";
const RELEASE_CHECKSUMS_ASSET: &str = "SHA256SUMS";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperVersionInfo {
    pub helper_version: String,
    pub protocol_version: u32,
    pub frame_version: u16,
    pub os: String,
    pub arch: String,
}

impl HelperVersionInfo {
    pub fn current() -> Self {
        Self {
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            frame_version: FRAME_VERSION,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteDeploymentPhase {
    ConnectingSshHost,
    StartingWslDistro,
    DetectingRemotePlatform,
    CheckingRemoteHelper,
    InstallingRemoteHelper,
    StartingRemoteWorkspaceService,
    LoadingProjectEnvironment,
}

impl RemoteDeploymentPhase {
    pub fn message(self) -> &'static str {
        match self {
            Self::ConnectingSshHost => "Connecting to SSH host",
            Self::StartingWslDistro => "Starting WSL distribution",
            Self::DetectingRemotePlatform => "Detecting remote platform",
            Self::CheckingRemoteHelper => "Checking nucleotide-remote",
            Self::InstallingRemoteHelper => "Installing nucleotide-remote",
            Self::StartingRemoteWorkspaceService => "Starting remote workspace service",
            Self::LoadingProjectEnvironment => "Loading project environment",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDeploymentProgress {
    pub phase: RemoteDeploymentPhase,
    pub target: Option<String>,
    pub detail: Option<String>,
}

impl RemoteDeploymentProgress {
    pub fn message(&self) -> String {
        let mut message = self.phase.message().to_string();
        if let Some(target) = self.target.as_deref().filter(|target| !target.is_empty()) {
            message.push_str(": ");
            message.push_str(target);
        }
        if let Some(detail) = self.detail.as_deref().filter(|detail| !detail.is_empty()) {
            message.push_str(" (");
            message.push_str(detail);
            message.push(')');
        }
        message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteServiceCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
}

impl RemoteServiceCommand {
    pub fn command(&self) -> Command {
        let program = self.resolved_program();
        let mut command = nucleotide_process::command(&program);
        command.args(&self.args);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        command
    }

    pub fn resolved_program(&self) -> OsString {
        resolve_service_program(&self.program)
    }

    pub fn display_invocation(&self) -> String {
        std::iter::once(self.program.as_os_str())
            .chain(self.args.iter().map(OsString::as_os_str))
            .map(quote_command_display_arg)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn display_context(&self) -> String {
        match &self.current_dir {
            Some(current_dir) => format!(
                "{} (cwd {})",
                self.display_invocation(),
                quote_command_display_arg(current_dir.as_os_str())
            ),
            None => self.display_invocation(),
        }
    }
}

fn resolve_service_program(program: &OsStr) -> OsString {
    #[cfg(windows)]
    {
        if let Some(path) = resolve_windows_program(program) {
            return path.into_os_string();
        }
    }

    program.to_os_string()
}

#[cfg(windows)]
fn resolve_windows_program(program: &OsStr) -> Option<PathBuf> {
    let program_text = program.to_string_lossy();
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.is_file().then(|| program_path.to_path_buf());
    }

    resolve_windows_program_from_path(&program_text).or_else(|| {
        let windir = std::env::var_os("WINDIR")?;
        let system32 = PathBuf::from(windir).join("System32");
        if program_text.eq_ignore_ascii_case("ssh") || program_text.eq_ignore_ascii_case("ssh.exe")
        {
            let ssh = system32.join("OpenSSH").join("ssh.exe");
            return ssh.is_file().then_some(ssh);
        }
        if program_text.eq_ignore_ascii_case("wsl") || program_text.eq_ignore_ascii_case("wsl.exe")
        {
            let wsl = system32.join("wsl.exe");
            return wsl.is_file().then_some(wsl);
        }
        None
    })
}

#[cfg(windows)]
fn resolve_windows_program_from_path(program: &str) -> Option<PathBuf> {
    let path_exts = std::env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()]);
    let candidates = if Path::new(program).extension().is_some() {
        vec![program.to_string()]
    } else {
        path_exts
            .iter()
            .map(|ext| format!("{program}{ext}"))
            .chain(std::iter::once(program.to_string()))
            .collect()
    };

    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|directory| {
            candidates
                .iter()
                .map(|candidate| directory.join(candidate))
                .find(|candidate| candidate.is_file())
        })
    })
}

pub fn local_service_command(
    helper_path: impl AsRef<Path>,
    workspace_root: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let helper_path = helper_path.as_ref();
    let workspace_root = workspace_root.as_ref();
    let args = vec![
        OsString::from("serve"),
        OsString::from("--workspace"),
        workspace_root.as_os_str().to_os_string(),
        OsString::from("--protocol"),
        OsString::from("v5"),
    ];
    RemoteServiceCommand {
        program: helper_path.as_os_str().to_os_string(),
        args,
        current_dir: Some(workspace_root.to_path_buf()),
    }
}

pub fn wsl_service_command(
    distro: impl AsRef<OsStr>,
    linux_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let linux_root = linux_root.as_ref();
    let helper_path = helper_path.as_ref();
    let args = vec![
        OsString::from("--distribution"),
        distro.as_ref().to_os_string(),
        OsString::from("--cd"),
        linux_root.as_os_str().to_os_string(),
        OsString::from("--exec"),
        helper_path.as_os_str().to_os_string(),
        OsString::from("serve"),
        OsString::from("--workspace"),
        linux_root.as_os_str().to_os_string(),
        OsString::from("--protocol"),
        OsString::from("v5"),
    ];
    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args,
        current_dir: None,
    }
}

pub fn wsl_lsp_proxy_command(
    distro: impl AsRef<OsStr>,
    linux_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
    server: impl AsRef<OsStr>,
) -> RemoteServiceCommand {
    let linux_root = linux_root.as_ref();
    let helper_path = helper_path.as_ref();
    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args: vec![
            OsString::from("--distribution"),
            distro.as_ref().to_os_string(),
            OsString::from("--cd"),
            linux_root.as_os_str().to_os_string(),
            OsString::from("--exec"),
            helper_path.as_os_str().to_os_string(),
            OsString::from("lsp-proxy"),
            OsString::from("--workspace"),
            linux_root.as_os_str().to_os_string(),
            OsString::from("--server"),
            server.as_ref().to_os_string(),
            OsString::from("--"),
        ],
        current_dir: None,
    }
}

pub fn wsl_interactive_terminal_command(
    distro: impl AsRef<OsStr>,
    linux_root: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let linux_root = linux_root.as_ref();
    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args: vec![
            OsString::from("--distribution"),
            distro.as_ref().to_os_string(),
            OsString::from("--cd"),
            linux_root.as_os_str().to_os_string(),
        ],
        current_dir: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub connect_timeout_secs: Option<u64>,
    pub extra_args: Vec<OsString>,
    pub control_path: Option<PathBuf>,
}

impl SshTarget {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            user: None,
            port: None,
            connect_timeout_secs: None,
            extra_args: Vec::new(),
            control_path: None,
        }
    }

    fn target_arg(&self) -> String {
        match &self.user {
            Some(user) if !user.is_empty() => format!("{user}@{}", self.host),
            _ => self.host.clone(),
        }
    }
}

pub fn ssh_interactive_terminal_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let remote_root = posix_path_string(remote_root);
    let remote_command = format!(
        "cd {} && exec \"${{SHELL:-/bin/sh}}\" -l",
        quote_posix_shell(&remote_root)
    );
    let mut args = Vec::new();
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("-tt"));
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

pub fn ssh_service_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let remote_root = posix_path_string(remote_root);
    let helper_path = posix_path_string(helper_path);
    let remote_command = format!(
        "exec {} serve --workspace {} --protocol v5",
        quote_posix_shell(&helper_path),
        quote_posix_shell(&remote_root)
    );
    let mut args = Vec::new();
    args.push(OsString::from("-T"));
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

pub fn ssh_lsp_proxy_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
    server: impl AsRef<OsStr>,
) -> RemoteServiceCommand {
    let remote_root = posix_path_string(remote_root);
    let helper_path = posix_path_string(helper_path);
    let server = server.as_ref().to_string_lossy();
    let remote_command = format!(
        "exec {} lsp-proxy --workspace {} --server {} --",
        quote_posix_shell(&helper_path),
        quote_posix_shell(&remote_root),
        quote_posix_shell(&server)
    );
    let mut args = Vec::new();
    args.push(OsString::from("-T"));
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

pub fn wsl_terminal_proxy_command(
    distro: impl AsRef<OsStr>,
    linux_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> RemoteServiceCommand {
    let linux_root = linux_root.as_ref();
    let helper_path = helper_path.as_ref();
    let mut args = vec![
        OsString::from("--distribution"),
        distro.as_ref().to_os_string(),
        OsString::from("--cd"),
        linux_root.as_os_str().to_os_string(),
        OsString::from("--exec"),
        helper_path.as_os_str().to_os_string(),
        OsString::from("terminal-proxy"),
        OsString::from("--workspace"),
        linux_root.as_os_str().to_os_string(),
    ];
    append_terminal_proxy_args(&mut args, shell, command, env);

    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args,
        current_dir: None,
    }
}

pub fn ssh_terminal_proxy_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> RemoteServiceCommand {
    let remote_command = terminal_proxy_shell_command(
        helper_path.as_ref(),
        remote_root.as_ref(),
        shell,
        command,
        env,
    );
    let mut args = Vec::new();
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("-tt"));
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

fn append_ssh_connection_args(args: &mut Vec<OsString>, target: &SshTarget) {
    args.push(OsString::from("-o"));
    args.push(OsString::from("BatchMode=yes"));
    args.push(OsString::from("-o"));
    args.push(OsString::from("NumberOfPasswordPrompts=0"));
    args.push(OsString::from("-o"));
    args.push(OsString::from("ConnectionAttempts=1"));
    args.push(OsString::from("-o"));
    args.push(OsString::from("StrictHostKeyChecking=accept-new"));
    args.push(OsString::from("-o"));
    args.push(OsString::from(format!(
        "ServerAliveInterval={DEFAULT_SSH_SERVER_ALIVE_INTERVAL_SECS}"
    )));
    args.push(OsString::from("-o"));
    args.push(OsString::from(format!(
        "ServerAliveCountMax={DEFAULT_SSH_SERVER_ALIVE_COUNT_MAX}"
    )));

    if let Some(timeout_secs) = target.connect_timeout_secs {
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!("ConnectTimeout={timeout_secs}")));
    }

    if let Some(control_path) = target.control_path.as_ref() {
        if let Some(parent) = control_path.parent() {
            let _ = std::fs::create_dir_all(parent);
            ensure_private_ssh_control_dir(parent);
        }

        args.push(OsString::from("-o"));
        args.push(OsString::from("ControlMaster=auto"));
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!(
            "ControlPersist={DEFAULT_SSH_CONTROL_PERSIST}"
        )));
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!(
            "ControlPath={}",
            control_path.display()
        )));
    }

    args.extend(target.extra_args.iter().cloned());
}

fn append_terminal_proxy_args(
    args: &mut Vec<OsString>,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) {
    if let Some(shell) = shell.filter(|shell| !shell.is_empty()) {
        args.push(OsString::from("--shell"));
        args.push(OsString::from(shell));
    }
    for (key, value) in env {
        if terminal_env_entry_is_valid(key, value) {
            args.push(OsString::from("--env"));
            args.push(OsString::from(format!("{key}={value}")));
        }
    }
    if let Some((program, program_args)) = command {
        args.push(OsString::from("--"));
        args.push(OsString::from(program));
        args.extend(program_args.iter().map(OsString::from));
    }
}

fn terminal_proxy_shell_command(
    helper_path: &Path,
    remote_root: &Path,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> String {
    let helper_path = posix_path_string(helper_path);
    let remote_root = posix_path_string(remote_root);
    let mut parts = vec![
        "exec".to_string(),
        quote_posix_shell(&helper_path),
        "terminal-proxy".to_string(),
        "--workspace".to_string(),
        quote_posix_shell(&remote_root),
    ];
    if let Some(shell) = shell.filter(|shell| !shell.is_empty()) {
        parts.push("--shell".to_string());
        parts.push(quote_posix_shell(shell));
    }
    for (key, value) in env {
        if terminal_env_entry_is_valid(key, value) {
            parts.push("--env".to_string());
            parts.push(quote_posix_shell(&format!("{key}={value}")));
        }
    }
    if let Some((program, program_args)) = command {
        parts.push("--".to_string());
        parts.push(quote_posix_shell(program));
        parts.extend(program_args.iter().map(|arg| quote_posix_shell(arg)));
    }
    parts.join(" ")
}

fn terminal_env_entry_is_valid(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains('=') && !key.contains('\0') && !value.contains('\0')
}

trait V5TransportAbort: Send + Sync {
    fn abort(&self);
}

struct ChildProcessV5Control {
    child: Mutex<Option<Child>>,
    child_id: u32,
    abort_started: AtomicBool,
    reaped: Arc<AtomicBool>,
}

impl ChildProcessV5Control {
    fn new(child: Child) -> Self {
        let child_id = child.id();
        Self {
            child: Mutex::new(Some(child)),
            child_id,
            abort_started: AtomicBool::new(false),
            reaped: Arc::new(AtomicBool::new(false)),
        }
    }

    fn child_id(&self) -> u32 {
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
        let _ = child.kill();
        let reaped = Arc::clone(&self.reaped);
        if std::thread::Builder::new()
            .name("nucleotide-v5-child-reaper".to_string())
            .spawn(move || {
                let _ = child.wait();
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
    fn was_reaped(&self) -> bool {
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

fn spawn_child_process_v5_io(
    spec: &RemoteServiceCommand,
) -> io::Result<(
    protocol_v5::FramedIo<ChildStdout, ChildProcessV5Writer>,
    Arc<ChildProcessV5Control>,
)> {
    let mut command = spec.command();
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn()?;
    let writer = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("remote service child did not expose stdin"))?;
    let reader = child
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

fn quote_posix_shell(value: &str) -> String {
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

fn quote_command_display_arg(value: &OsStr) -> String {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum RemoteRequest {
    Stat {
        path: PathBuf,
    },
    ListDir {
        path: PathBuf,
    },
    ListDirs {
        paths: Vec<PathBuf>,
    },
    FindAncestorFile {
        start: PathBuf,
        file_name: String,
        limit: usize,
    },
    CreateFile {
        path: PathBuf,
    },
    CreateDir {
        path: PathBuf,
    },
    RenamePath {
        from: PathBuf,
        to: PathBuf,
    },
    DeletePath {
        path: PathBuf,
    },
    CopyPath {
        from: PathBuf,
        to: PathBuf,
    },
    ReadFile {
        path: PathBuf,
        max_bytes: Option<u64>,
    },
    WriteFile {
        path: PathBuf,
        create_parent_dirs: bool,
        expected_modified_unix_millis: Option<i64>,
        #[serde(default)]
        expected_modified_unix_nanos: Option<u32>,
    },
    FileSearch(FileSearchRequest),
    TextSearch(TextSearchRequest),
    ProjectEnvironment {
        root: PathBuf,
    },
    GitHead {
        root: PathBuf,
    },
    GitStatus {
        root: PathBuf,
        include_untracked: bool,
        limit: usize,
    },
    RunProcess(ProcessRequest),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteRequestDeadlineKind {
    Absolute,
    Inactivity,
}

impl fmt::Display for RemoteRequestDeadlineKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute => formatter.write_str("absolute"),
            Self::Inactivity => formatter.write_str("inactivity"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteRequestContext {
    pub created_at: Instant,
    pub absolute_deadline: Option<Instant>,
    pub deadline_unix_ms: u64,
    pub inactivity_timeout: Option<Duration>,
}

type RemoteRequestCancellationCallback = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone, Default)]
pub struct RemoteRequestCancellation {
    inner: Arc<RemoteRequestCancellationInner>,
}

#[derive(Default)]
struct RemoteRequestCancellationInner {
    cancelled: AtomicBool,
    callbacks: Mutex<Vec<RemoteRequestCancellationCallback>>,
}

impl RemoteRequestCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if self.inner.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        let callbacks = std::mem::take(
            &mut *self
                .inner
                .callbacks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for callback in callbacks {
            callback();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    fn register(&self, callback: impl FnOnce() + Send + 'static) {
        let mut callback = Some(Box::new(callback) as RemoteRequestCancellationCallback);
        {
            let mut callbacks = self
                .inner
                .callbacks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !self.is_cancelled()
                && let Some(callback) = callback.take()
            {
                callbacks.push(callback);
            }
        }
        if let Some(callback) = callback {
            callback();
        }
    }

    fn check_cancelled(&self, method: &str) -> std::result::Result<(), RemoteClientError> {
        if self.is_cancelled() {
            Err(remote_request_cancelled_error(method))
        } else {
            Ok(())
        }
    }
}

struct RemoteRequestCancelOnDrop {
    cancellation: Option<RemoteRequestCancellation>,
}

impl RemoteRequestCancelOnDrop {
    fn new() -> Self {
        Self {
            cancellation: Some(RemoteRequestCancellation::new()),
        }
    }

    fn cancellation(&self) -> RemoteRequestCancellation {
        self.cancellation.clone().unwrap_or_default()
    }

    fn disarm(&mut self) {
        self.cancellation.take();
    }
}

impl Drop for RemoteRequestCancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteRequestDeadlinePolicy {
    absolute_timeout: Option<Duration>,
    inactivity_timeout: Option<Duration>,
}

impl RemoteRequestDeadlinePolicy {
    const fn bounded(absolute_timeout: Duration, inactivity_timeout: Duration) -> Self {
        Self {
            absolute_timeout: Some(absolute_timeout),
            inactivity_timeout: Some(inactivity_timeout),
        }
    }

    const fn absolute_only(absolute_timeout: Duration) -> Self {
        Self {
            absolute_timeout: Some(absolute_timeout),
            inactivity_timeout: None,
        }
    }

    const fn unlimited() -> Self {
        Self {
            absolute_timeout: None,
            inactivity_timeout: None,
        }
    }
}

impl RemoteRequestContext {
    fn from_policy(policy: RemoteRequestDeadlinePolicy) -> Self {
        Self::from_policy_at(policy, Instant::now(), v5_now_unix_millis())
    }

    fn from_policy_at(
        policy: RemoteRequestDeadlinePolicy,
        created_at: Instant,
        now_unix_ms: u64,
    ) -> Self {
        let absolute_deadline = policy
            .absolute_timeout
            .and_then(|timeout| created_at.checked_add(timeout));
        let deadline_unix_ms = absolute_deadline
            .and(policy.absolute_timeout)
            .map(|timeout| {
                let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
                now_unix_ms.saturating_add(timeout_ms)
            })
            .unwrap_or(0);
        Self {
            created_at,
            absolute_deadline,
            deadline_unix_ms,
            inactivity_timeout: policy.inactivity_timeout,
        }
    }

    fn expired_at(self, now: Instant) -> Option<RemoteRequestDeadlineKind> {
        self.absolute_deadline
            .filter(|deadline| now >= *deadline)
            .map(|_| RemoteRequestDeadlineKind::Absolute)
    }
}

fn v5_watch_control_request_context() -> RemoteRequestContext {
    RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::bounded(
        V5_REQUEST_CONTROL_DEADLINE,
        V5_REQUEST_CONTROL_INACTIVITY,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "snake_case")]
pub enum RemoteResponse {
    Stat(FileStatResponse),
    ListDir(DirectoryListingResponse),
    ListDirs(ListDirsResponse),
    FindAncestorFile(Option<PathBuf>),
    CreateFile(FileStatResponse),
    CreateDir(FileStatResponse),
    RenamePath(FileStatResponse),
    DeletePath(FileStatResponse),
    CopyPath(FileStatResponse),
    ReadFile(FileReadResponse),
    WriteFile(WriteResultResponse),
    FileSearch(FileSearchResponse),
    TextSearch(TextSearchResponse),
    ProjectEnvironment(ProjectEnvironmentResponse),
    GitHead(GitHeadResponse),
    GitStatus(GitStatusResponse),
    RunProcess(ProcessOutputResponse),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum V5MethodError {
    UnsupportedMethod(String),
    InvalidPayload { method: String, error: String },
    Encode { method: String, error: String },
}

impl fmt::Display for V5MethodError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMethod(method) => {
                write!(f, "unsupported v5 method: {method}")
            }
            Self::InvalidPayload { method, error } => {
                write!(f, "invalid v5 payload for {method}: {error}")
            }
            Self::Encode { method, error } => {
                write!(f, "failed to encode v5 payload for {method}: {error}")
            }
        }
    }
}

impl Error for V5MethodError {}

impl RemoteRequest {
    pub fn v5_method(&self) -> &'static str {
        match self {
            Self::Stat { .. } => "fs.stat",
            Self::ListDir { .. } => "fs.list_dir",
            Self::ListDirs { .. } => "fs.list_dirs",
            Self::FindAncestorFile { .. } => "fs.find_ancestor",
            Self::CreateFile { .. } => "fs.create_file",
            Self::CreateDir { .. } => "fs.create_dir",
            Self::RenamePath { .. } => "fs.rename",
            Self::DeletePath { .. } => "fs.delete",
            Self::CopyPath { .. } => "fs.copy",
            Self::ReadFile { .. } => "fs.read",
            Self::WriteFile { .. } => "fs.write",
            Self::FileSearch(_) => "search.files",
            Self::TextSearch(_) => "search.text",
            Self::ProjectEnvironment { .. } => "env.project",
            Self::GitHead { .. } => "git.head",
            Self::GitStatus { .. } => "git.status",
            Self::RunProcess(_) => "process.run",
            Self::Shutdown => "session.shutdown",
        }
    }

    pub fn v5_request_options(&self) -> protocol_v5::RequestOptions {
        let mut options = protocol_v5::RequestOptions::default();
        match self {
            Self::WriteFile { .. }
            | Self::CreateFile { .. }
            | Self::CreateDir { .. }
            | Self::RenamePath { .. }
            | Self::DeletePath { .. }
            | Self::CopyPath { .. } => {
                options.idempotency = protocol_v5::Idempotency::Mutation;
                options.priority = protocol_v5::Priority::UserInput;
            }
            Self::RunProcess(_) => {
                options.idempotency = protocol_v5::Idempotency::Process;
                options.priority = protocol_v5::Priority::LspSupport;
            }
            Self::FileSearch(_) | Self::TextSearch(_) => {
                options.priority = protocol_v5::Priority::Background;
                options.cancellation_group = self.v5_method().to_string();
            }
            Self::ListDir { .. } | Self::ListDirs { .. } => {
                options.priority = protocol_v5::Priority::VisibleFileTree;
            }
            Self::Stat { .. } | Self::ReadFile { .. } => {
                options.priority = protocol_v5::Priority::ForegroundDocument;
            }
            Self::FindAncestorFile { .. } | Self::ProjectEnvironment { .. } => {
                options.priority = protocol_v5::Priority::LspSupport;
            }
            Self::GitHead { .. } | Self::GitStatus { .. } => {
                options.priority = protocol_v5::Priority::Background;
            }
            Self::Shutdown => {
                options.priority = protocol_v5::Priority::UserInput;
            }
        }
        options
    }

    fn v5_deadline_policy(&self) -> RemoteRequestDeadlinePolicy {
        match self {
            Self::Stat { .. }
            | Self::ListDir { .. }
            | Self::ListDirs { .. }
            | Self::FindAncestorFile { .. }
            | Self::GitHead { .. }
            | Self::GitStatus { .. } => RemoteRequestDeadlinePolicy::bounded(
                V5_REQUEST_METADATA_DEADLINE,
                V5_REQUEST_METADATA_INACTIVITY,
            ),
            Self::CreateFile { .. }
            | Self::CreateDir { .. }
            | Self::RenamePath { .. }
            | Self::DeletePath { .. }
            | Self::CopyPath { .. }
            | Self::ProjectEnvironment { .. } => RemoteRequestDeadlinePolicy::bounded(
                V5_REQUEST_MUTATION_DEADLINE,
                V5_REQUEST_MUTATION_INACTIVITY,
            ),
            Self::ReadFile { .. } | Self::WriteFile { .. } => RemoteRequestDeadlinePolicy::bounded(
                V5_REQUEST_FILE_DEADLINE,
                V5_REQUEST_FILE_INACTIVITY,
            ),
            Self::FileSearch(_) | Self::TextSearch(_) => RemoteRequestDeadlinePolicy::bounded(
                V5_REQUEST_SEARCH_DEADLINE,
                V5_REQUEST_SEARCH_INACTIVITY,
            ),
            Self::RunProcess(request) => request.timeout_ms.map_or_else(
                RemoteRequestDeadlinePolicy::unlimited,
                |timeout_ms| {
                    RemoteRequestDeadlinePolicy::absolute_only(
                        Duration::from_millis(timeout_ms)
                            .checked_add(V5_REQUEST_PROCESS_CANCELLATION_GRACE)
                            .unwrap_or(Duration::MAX),
                    )
                },
            ),
            Self::Shutdown => RemoteRequestDeadlinePolicy::bounded(
                V5_REQUEST_CONTROL_DEADLINE,
                V5_REQUEST_CONTROL_INACTIVITY,
            ),
        }
    }

    pub fn v5_request_context(&self) -> RemoteRequestContext {
        RemoteRequestContext::from_policy(self.v5_deadline_policy())
    }

    fn v5_request_options_with_context(
        &self,
        context: RemoteRequestContext,
    ) -> protocol_v5::RequestOptions {
        let mut options = self.v5_request_options();
        options.deadline_unix_ms = context.deadline_unix_ms;
        options
    }

    pub fn v5_body_channel(&self) -> protocol_v5::DataChannel {
        match self {
            Self::WriteFile { .. } | Self::ReadFile { .. } => protocol_v5::DataChannel::FileBody,
            Self::RunProcess(_) => protocol_v5::DataChannel::Stdin,
            Self::FileSearch(_) | Self::TextSearch(_) => protocol_v5::DataChannel::SearchPayload,
            _ => protocol_v5::DataChannel::Unspecified,
        }
    }

    pub fn v5_prefers_zstd_compression(&self) -> bool {
        matches!(
            self,
            Self::ListDir { .. }
                | Self::ListDirs { .. }
                | Self::FileSearch(_)
                | Self::TextSearch(_)
        )
    }

    pub fn v5_retry_after_reconnect_allowed(&self) -> bool {
        !matches!(self, Self::Shutdown)
            && self.v5_request_options().idempotency == protocol_v5::Idempotency::ReadOnly
    }

    pub fn to_v5_method_payload(
        &self,
    ) -> std::result::Result<(&'static str, Vec<u8>), V5MethodError> {
        encode_v5_payload(self.v5_method(), self.v5_payload_value())
    }

    pub fn from_v5_method_payload(
        method: &str,
        payload: &[u8],
    ) -> std::result::Result<Self, V5MethodError> {
        match method {
            "session.shutdown" => {
                decode_empty_v5_payload(method, payload)?;
                Ok(Self::Shutdown)
            }
            "fs.stat" => {
                let payload: V5PathPayload = decode_v5_payload(method, payload)?;
                Ok(Self::Stat { path: payload.path })
            }
            "fs.list_dir" => {
                let payload: V5PathPayload = decode_v5_payload(method, payload)?;
                Ok(Self::ListDir { path: payload.path })
            }
            "fs.list_dirs" => {
                let payload: V5PathsPayload = decode_v5_payload(method, payload)?;
                Ok(Self::ListDirs {
                    paths: payload.paths,
                })
            }
            "fs.find_ancestor" => {
                let payload: V5FindAncestorPayload = decode_v5_payload(method, payload)?;
                Ok(Self::FindAncestorFile {
                    start: payload.start,
                    file_name: payload.file_name,
                    limit: payload.limit,
                })
            }
            "fs.create_file" => {
                let payload: V5PathPayload = decode_v5_payload(method, payload)?;
                Ok(Self::CreateFile { path: payload.path })
            }
            "fs.create_dir" => {
                let payload: V5PathPayload = decode_v5_payload(method, payload)?;
                Ok(Self::CreateDir { path: payload.path })
            }
            "fs.rename" => {
                let payload: V5RenamePayload = decode_v5_payload(method, payload)?;
                Ok(Self::RenamePath {
                    from: payload.from,
                    to: payload.to,
                })
            }
            "fs.delete" => {
                let payload: V5PathPayload = decode_v5_payload(method, payload)?;
                Ok(Self::DeletePath { path: payload.path })
            }
            "fs.copy" => {
                let payload: V5RenamePayload = decode_v5_payload(method, payload)?;
                Ok(Self::CopyPath {
                    from: payload.from,
                    to: payload.to,
                })
            }
            "fs.read" => {
                let payload: V5ReadFilePayload = decode_v5_payload(method, payload)?;
                Ok(Self::ReadFile {
                    path: payload.path,
                    max_bytes: payload.max_bytes,
                })
            }
            "fs.write" => {
                let payload: V5WriteFilePayload = decode_v5_payload(method, payload)?;
                Ok(Self::WriteFile {
                    path: payload.path,
                    create_parent_dirs: payload.create_parent_dirs,
                    expected_modified_unix_millis: payload.expected_modified_unix_millis,
                    expected_modified_unix_nanos: payload.expected_modified_unix_nanos,
                })
            }
            "search.files" => Ok(Self::FileSearch(decode_v5_payload(method, payload)?)),
            "search.text" => Ok(Self::TextSearch(decode_v5_payload(method, payload)?)),
            "env.project" => {
                let payload: V5RootPayload = decode_v5_payload(method, payload)?;
                Ok(Self::ProjectEnvironment { root: payload.root })
            }
            "git.head" => {
                let payload: V5RootPayload = decode_v5_payload(method, payload)?;
                Ok(Self::GitHead { root: payload.root })
            }
            "git.status" => {
                let payload: V5GitStatusPayload = decode_v5_payload(method, payload)?;
                Ok(Self::GitStatus {
                    root: payload.root,
                    include_untracked: payload.include_untracked,
                    limit: payload.limit,
                })
            }
            "process.run" => Ok(Self::RunProcess(decode_v5_payload(method, payload)?)),
            _ => Err(V5MethodError::UnsupportedMethod(method.to_string())),
        }
    }

    fn v5_payload_value(&self) -> V5RequestPayloadRef<'_> {
        match self {
            Self::Shutdown => V5RequestPayloadRef::Empty {},
            Self::Stat { path }
            | Self::ListDir { path }
            | Self::CreateFile { path }
            | Self::CreateDir { path }
            | Self::DeletePath { path } => V5RequestPayloadRef::Path { path },
            Self::ListDirs { paths } => V5RequestPayloadRef::Paths { paths },
            Self::FindAncestorFile {
                start,
                file_name,
                limit,
            } => V5RequestPayloadRef::FindAncestor {
                start,
                file_name,
                limit: *limit,
            },
            Self::RenamePath { from, to } | Self::CopyPath { from, to } => {
                V5RequestPayloadRef::Rename { from, to }
            }
            Self::ReadFile { path, max_bytes } => V5RequestPayloadRef::ReadFile {
                path,
                max_bytes: *max_bytes,
            },
            Self::WriteFile {
                path,
                create_parent_dirs,
                expected_modified_unix_millis,
                expected_modified_unix_nanos,
            } => V5RequestPayloadRef::WriteFile {
                path,
                create_parent_dirs: *create_parent_dirs,
                expected_modified_unix_millis: *expected_modified_unix_millis,
                expected_modified_unix_nanos: *expected_modified_unix_nanos,
            },
            Self::FileSearch(request) => V5RequestPayloadRef::FileSearch(request),
            Self::TextSearch(request) => V5RequestPayloadRef::TextSearch(request),
            Self::ProjectEnvironment { root } | Self::GitHead { root } => {
                V5RequestPayloadRef::Root { root }
            }
            Self::GitStatus {
                root,
                include_untracked,
                limit,
            } => V5RequestPayloadRef::GitStatus {
                root,
                include_untracked: *include_untracked,
                limit: *limit,
            },
            Self::RunProcess(request) => V5RequestPayloadRef::RunProcess(request),
        }
    }
}

impl RemoteResponse {
    pub fn v5_method(&self) -> &'static str {
        match self {
            Self::Stat(_) => "fs.stat",
            Self::ListDir(_) => "fs.list_dir",
            Self::ListDirs(_) => "fs.list_dirs",
            Self::FindAncestorFile(_) => "fs.find_ancestor",
            Self::CreateFile(_) => "fs.create_file",
            Self::CreateDir(_) => "fs.create_dir",
            Self::RenamePath(_) => "fs.rename",
            Self::DeletePath(_) => "fs.delete",
            Self::CopyPath(_) => "fs.copy",
            Self::ReadFile(_) => "fs.read",
            Self::WriteFile(_) => "fs.write",
            Self::FileSearch(_) => "search.files",
            Self::TextSearch(_) => "search.text",
            Self::ProjectEnvironment(_) => "env.project",
            Self::GitHead(_) => "git.head",
            Self::GitStatus(_) => "git.status",
            Self::RunProcess(_) => "process.run",
            Self::Shutdown => "session.shutdown",
        }
    }

    pub fn to_v5_payload(&self) -> std::result::Result<Vec<u8>, V5MethodError> {
        let method = self.v5_method();
        serde_json::to_vec(self).map_err(|error| V5MethodError::Encode {
            method: method.to_string(),
            error: error.to_string(),
        })
    }

    pub fn from_v5_payload(
        method: &str,
        payload: &[u8],
    ) -> std::result::Result<Self, V5MethodError> {
        let response: Self =
            serde_json::from_slice(payload).map_err(|error| V5MethodError::InvalidPayload {
                method: method.to_string(),
                error: error.to_string(),
            })?;
        if response.v5_method() == method {
            Ok(response)
        } else {
            Err(V5MethodError::InvalidPayload {
                method: method.to_string(),
                error: format!(
                    "response payload method {:?} does not match stream method",
                    response.v5_method()
                ),
            })
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum V5RequestPayloadRef<'a> {
    Empty {},
    Path {
        #[serde(serialize_with = "serialize_posix_path")]
        path: &'a PathBuf,
    },
    Paths {
        #[serde(serialize_with = "serialize_posix_paths")]
        paths: &'a Vec<PathBuf>,
    },
    FindAncestor {
        #[serde(serialize_with = "serialize_posix_path")]
        start: &'a PathBuf,
        file_name: &'a String,
        limit: usize,
    },
    Rename {
        #[serde(serialize_with = "serialize_posix_path")]
        from: &'a PathBuf,
        #[serde(serialize_with = "serialize_posix_path")]
        to: &'a PathBuf,
    },
    ReadFile {
        #[serde(serialize_with = "serialize_posix_path")]
        path: &'a PathBuf,
        max_bytes: Option<u64>,
    },
    WriteFile {
        #[serde(serialize_with = "serialize_posix_path")]
        path: &'a PathBuf,
        create_parent_dirs: bool,
        expected_modified_unix_millis: Option<i64>,
        expected_modified_unix_nanos: Option<u32>,
    },
    FileSearch(&'a FileSearchRequest),
    TextSearch(&'a TextSearchRequest),
    Root {
        #[serde(serialize_with = "serialize_posix_path")]
        root: &'a PathBuf,
    },
    GitStatus {
        #[serde(serialize_with = "serialize_posix_path")]
        root: &'a PathBuf,
        include_untracked: bool,
        limit: usize,
    },
    RunProcess(&'a ProcessRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5DirectoryListPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    path: PathBuf,
    #[serde(default)]
    known_generation: Option<u64>,
    #[serde(default)]
    known_fingerprint: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5DirectoryListEntryPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    path: PathBuf,
    #[serde(default)]
    known_generation: Option<u64>,
    #[serde(default)]
    known_fingerprint: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5DirectoryListDirsPayload {
    #[serde(default, serialize_with = "serialize_posix_paths")]
    paths: Vec<PathBuf>,
    #[serde(default)]
    entries: Vec<V5DirectoryListEntryPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5PathPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5PathsPayload {
    #[serde(serialize_with = "serialize_posix_paths")]
    paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5FindAncestorPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    start: PathBuf,
    file_name: String,
    limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5RenamePayload {
    #[serde(serialize_with = "serialize_posix_path")]
    from: PathBuf,
    #[serde(serialize_with = "serialize_posix_path")]
    to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5ReadFilePayload {
    #[serde(serialize_with = "serialize_posix_path")]
    path: PathBuf,
    max_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5WriteFilePayload {
    #[serde(serialize_with = "serialize_posix_path")]
    path: PathBuf,
    create_parent_dirs: bool,
    expected_modified_unix_millis: Option<i64>,
    #[serde(default)]
    expected_modified_unix_nanos: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5RootPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct V5GitStatusPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    root: PathBuf,
    include_untracked: bool,
    limit: usize,
}

fn serialize_posix_path<S, P>(path: &P, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    P: AsRef<Path>,
{
    serializer.serialize_str(&posix_path_string(path.as_ref()))
}

fn serialize_posix_paths<S, P>(paths: &P, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    P: AsRef<[PathBuf]>,
{
    paths
        .as_ref()
        .iter()
        .map(posix_path_string)
        .collect::<Vec<_>>()
        .serialize(serializer)
}

fn encode_v5_payload(
    method: &'static str,
    payload: V5RequestPayloadRef<'_>,
) -> std::result::Result<(&'static str, Vec<u8>), V5MethodError> {
    serde_json::to_vec(&payload)
        .map(|payload| (method, payload))
        .map_err(|error| V5MethodError::Encode {
            method: method.to_string(),
            error: error.to_string(),
        })
}

fn encode_v5_json_payload<T>(
    method: &'static str,
    payload: &T,
) -> std::result::Result<(&'static str, Vec<u8>), RemoteClientError>
where
    T: Serialize,
{
    serde_json::to_vec(payload)
        .map(|payload| (method, payload))
        .map_err(|error| {
            RemoteClientError::Protocol(format!(
                "failed to encode v5 payload for {method}: {error}"
            ))
        })
}

fn decode_v5_payload<T>(method: &str, payload: &[u8]) -> std::result::Result<T, V5MethodError>
where
    T: DeserializeOwned,
{
    let payload = if payload.is_empty() { b"{}" } else { payload };
    serde_json::from_slice(payload).map_err(|error| V5MethodError::InvalidPayload {
        method: method.to_string(),
        error: error.to_string(),
    })
}

fn decode_empty_v5_payload(method: &str, payload: &[u8]) -> std::result::Result<(), V5MethodError> {
    if payload.is_empty() {
        return Ok(());
    }
    let value: serde_json::Value = decode_v5_payload(method, payload)?;
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(V5MethodError::InvalidPayload {
            method: method.to_string(),
            error: "expected empty object".to_string(),
        })
    }
}

fn decode_v5_protobuf_payload<T>(
    method: &str,
    payload: &[u8],
) -> std::result::Result<T, V5MethodError>
where
    T: ProstMessage + Default,
{
    T::decode(payload).map_err(|error| V5MethodError::InvalidPayload {
        method: method.to_string(),
        error: error.to_string(),
    })
}

fn validate_v5_watch_start(
    start: &protocol_v5::WatchStart,
) -> std::result::Result<(), V5MethodError> {
    let method = "watch.start".to_string();
    let mode = protocol_v5::WatchMode::try_from(start.mode).map_err(|_| {
        V5MethodError::InvalidPayload {
            method: method.clone(),
            error: format!("unknown watch mode {}", start.mode),
        }
    })?;
    if mode != protocol_v5::WatchMode::ExpandedDirs {
        return Err(V5MethodError::InvalidPayload {
            method,
            error: format!("unsupported watch mode {mode:?}"),
        });
    }
    protocol_v5::WatchIgnorePolicy::try_from(start.ignore_policy).map_err(|_| {
        V5MethodError::InvalidPayload {
            method: "watch.start".to_string(),
            error: format!("unknown watch ignore policy {}", start.ignore_policy),
        }
    })?;
    if start.recursive {
        return Err(V5MethodError::InvalidPayload {
            method: "watch.start".to_string(),
            error: "recursive watch.start is not supported in v5.0".to_string(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteError {
    pub code: String,
    pub message: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResponse {
    pub helper_version: String,
    pub os: String,
    pub arch: String,
    pub workspace_root: PathBuf,
    pub capabilities: Vec<String>,
}

fn hello_response_from_v5_server_hello(hello: &protocol_v5::ServerHello) -> HelloResponse {
    HelloResponse {
        helper_version: hello.helper_version.clone(),
        os: hello.os.clone(),
        arch: hello.arch.clone(),
        workspace_root: PathBuf::from(&hello.workspace_root),
        capabilities: hello.capabilities.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteHelperInstallPolicy {
    Auto,
    Never,
    Upload,
    RemoteDownload,
}

impl RemoteHelperInstallPolicy {
    fn from_env_value(value: Option<String>) -> Self {
        match value.as_deref() {
            None | Some("") | Some("auto") | Some("AUTO") => Self::Auto,
            Some("never") | Some("NEVER") => Self::Never,
            Some("upload") | Some("UPLOAD") => Self::Upload,
            Some("remote_download") | Some("REMOTE_DOWNLOAD") => Self::RemoteDownload,
            Some(_) => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWorkspaceBackendOptions {
    pub remote_helper_path: PathBuf,
    pub remote_helper_path_is_override: bool,
    pub ssh_helper_path: Option<PathBuf>,
    pub ssh_helper_path_is_override: bool,
    pub wsl_helper_path: Option<PathBuf>,
    pub wsl_helper_path_is_override: bool,
    pub local_helper_path: Option<PathBuf>,
    pub ssh_helper_upload_path: Option<PathBuf>,
    pub ssh_helper_artifact_dir: Option<PathBuf>,
    pub ssh_helper_download_base_url: Option<String>,
    pub ssh_helper_install_policy: RemoteHelperInstallPolicy,
    pub wsl_helper_install_policy: RemoteHelperInstallPolicy,
    pub ssh_connect_timeout_secs: Option<u64>,
    pub ssh_extra_args: Vec<OsString>,
    pub ssh_control_path: Option<PathBuf>,
    pub use_local_service: bool,
}

impl Default for RemoteWorkspaceBackendOptions {
    fn default() -> Self {
        Self {
            remote_helper_path: PathBuf::from("nucleotide-remote"),
            remote_helper_path_is_override: false,
            ssh_helper_path: None,
            ssh_helper_path_is_override: false,
            wsl_helper_path: None,
            wsl_helper_path_is_override: false,
            local_helper_path: None,
            ssh_helper_upload_path: None,
            ssh_helper_artifact_dir: None,
            ssh_helper_download_base_url: None,
            ssh_helper_install_policy: RemoteHelperInstallPolicy::Auto,
            wsl_helper_install_policy: RemoteHelperInstallPolicy::Auto,
            ssh_connect_timeout_secs: Some(DEFAULT_SSH_CONNECT_TIMEOUT_SECS),
            ssh_extra_args: Vec::new(),
            ssh_control_path: default_ssh_control_master_enabled()
                .then(default_ssh_control_path)
                .flatten(),
            use_local_service: false,
        }
    }
}

#[derive(Default)]
struct RemoteWorkspaceBackendEnvironment {
    remote_helper_path: Option<OsString>,
    local_helper_path: Option<OsString>,
    ssh_helper_upload_path: Option<OsString>,
    ssh_helper_artifact_dir: Option<OsString>,
    ssh_helper_download_base_url: Option<String>,
    ssh_helper_install_policy: Option<String>,
    ssh_connect_timeout_secs: Option<String>,
    ssh_extra_args: Option<OsString>,
    ssh_control_master: Option<String>,
    ssh_control_path: Option<OsString>,
    use_local_service: bool,
    current_exe: Option<PathBuf>,
}

impl RemoteWorkspaceBackendOptions {
    pub fn from_environment() -> Self {
        Self::default().with_environment_overrides()
    }

    pub fn with_environment_overrides(self) -> Self {
        Self::from_environment_values_with_base(
            RemoteWorkspaceBackendEnvironment {
                remote_helper_path: std::env::var_os("NUCLEOTIDE_REMOTE_HELPER"),
                local_helper_path: std::env::var_os("NUCLEOTIDE_LOCAL_REMOTE_HELPER"),
                ssh_helper_upload_path: std::env::var_os("NUCLEOTIDE_REMOTE_HELPER_UPLOAD"),
                ssh_helper_artifact_dir: std::env::var_os("NUCLEOTIDE_REMOTE_HELPER_ARTIFACT_DIR"),
                ssh_helper_download_base_url: std::env::var(
                    "NUCLEOTIDE_REMOTE_HELPER_DOWNLOAD_BASE_URL",
                )
                .ok(),
                ssh_helper_install_policy: std::env::var("NUCLEOTIDE_REMOTE_HELPER_INSTALL").ok(),
                ssh_connect_timeout_secs: std::env::var("NUCLEOTIDE_SSH_CONNECT_TIMEOUT_SECS").ok(),
                ssh_extra_args: std::env::var_os("NUCLEOTIDE_SSH_EXTRA_ARGS"),
                ssh_control_master: std::env::var("NUCLEOTIDE_SSH_CONTROL_MASTER").ok(),
                ssh_control_path: std::env::var_os("NUCLEOTIDE_SSH_CONTROL_PATH"),
                use_local_service: env_flag_enabled("NUCLEOTIDE_LOCAL_REMOTE_SERVICE"),
                current_exe: std::env::current_exe().ok(),
            },
            self,
        )
    }

    #[cfg(test)]
    fn from_environment_values(values: RemoteWorkspaceBackendEnvironment) -> Self {
        Self::from_environment_values_with_base(values, Self::default())
    }

    fn from_environment_values_with_base(
        values: RemoteWorkspaceBackendEnvironment,
        mut options: Self,
    ) -> Self {
        let base_control_path = options.ssh_control_path.clone();
        let base_use_local_service = options.use_local_service;
        let base_remote_helper_path_is_override = options.remote_helper_path_is_override;
        let base_ssh_helper_path_is_override = options.ssh_helper_path_is_override;
        let base_wsl_helper_path_is_override = options.wsl_helper_path_is_override;
        let env_remote_helper_path = values.remote_helper_path;
        let env_local_helper_path = values.local_helper_path;
        let env_ssh_helper_upload_path = values.ssh_helper_upload_path;
        let env_ssh_helper_artifact_dir = values.ssh_helper_artifact_dir;
        let env_ssh_helper_download_base_url = values.ssh_helper_download_base_url;
        let env_ssh_helper_install_policy = values.ssh_helper_install_policy;
        let env_ssh_connect_timeout_secs = values.ssh_connect_timeout_secs;
        let env_ssh_extra_args = values.ssh_extra_args;
        let env_ssh_control_master = values.ssh_control_master;
        let env_ssh_control_path = values.ssh_control_path;
        let env_use_local_service = values.use_local_service;
        let current_exe = values.current_exe;
        let remote_helper_path_is_override =
            base_remote_helper_path_is_override || env_remote_helper_path.is_some();
        let ssh_helper_path_is_override =
            base_ssh_helper_path_is_override || env_remote_helper_path.is_some();
        let wsl_helper_path_is_override =
            base_wsl_helper_path_is_override || env_remote_helper_path.is_some();
        let bundled_helper = current_exe.as_deref().and_then(bundled_local_helper_path);
        let bundled_artifact_dir = current_exe
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let ssh_control_enabled = env_ssh_control_master
            .as_deref()
            .map(|value| env_flag_enabled_with_default(Some(value), base_control_path.is_some()))
            .unwrap_or_else(|| base_control_path.is_some());

        options.remote_helper_path_is_override = remote_helper_path_is_override;
        options.ssh_helper_path_is_override = ssh_helper_path_is_override;
        options.wsl_helper_path_is_override = wsl_helper_path_is_override;

        if let Some(policy) = env_ssh_helper_install_policy {
            let policy = RemoteHelperInstallPolicy::from_env_value(Some(policy));
            options.ssh_helper_install_policy = policy;
            options.wsl_helper_install_policy = policy;
        }

        if let Some(timeout) = env_ssh_connect_timeout_secs {
            options.ssh_connect_timeout_secs = ssh_connect_timeout_from_env(Some(&timeout));
        }

        if let Some(args) = env_ssh_extra_args {
            options.ssh_extra_args = ssh_extra_args_from_env(Some(args));
        }

        options.ssh_control_path = if ssh_control_enabled {
            env_ssh_control_path
                .map(PathBuf::from)
                .or(base_control_path)
                .or_else(default_ssh_control_path)
        } else {
            None
        };

        options.use_local_service = base_use_local_service || env_use_local_service;

        if let Some(path) = env_remote_helper_path {
            let path = PathBuf::from(path);
            options.remote_helper_path = path.clone();
            options.ssh_helper_path = Some(path.clone());
            options.wsl_helper_path = Some(path);
        }
        if let Some(path) = env_local_helper_path {
            options.local_helper_path = Some(PathBuf::from(path));
        } else if options.local_helper_path.is_none() {
            options.local_helper_path = bundled_helper.clone();
        }
        if let Some(path) = env_ssh_helper_upload_path {
            options.ssh_helper_upload_path = Some(PathBuf::from(path));
        }
        if let Some(path) = env_ssh_helper_artifact_dir {
            options.ssh_helper_artifact_dir = Some(PathBuf::from(path));
        } else if options.ssh_helper_artifact_dir.is_none() {
            options.ssh_helper_artifact_dir = bundled_artifact_dir;
        }
        if let Some(url) = env_ssh_helper_download_base_url
            && !url.trim().is_empty()
        {
            options.ssh_helper_download_base_url = Some(url);
        }
        options
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshRemotePlatform {
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SshRemoteProbe {
    platform: SshRemotePlatform,
    cache_root: String,
}

#[derive(Clone, Copy)]
enum LinuxHelperTransport<'a> {
    Ssh(&'a SshTarget),
    Wsl(&'a str),
}

impl LinuxHelperTransport<'_> {
    fn target_name(self) -> String {
        match self {
            Self::Ssh(target) => target.target_arg(),
            Self::Wsl(distro) => distro.to_string(),
        }
    }

    fn description(self) -> String {
        match self {
            Self::Ssh(target) => format!("SSH target {}", target.target_arg()),
            Self::Wsl(distro) => format!("WSL distro {distro}"),
        }
    }

    fn command(self, remote_command: String) -> RemoteServiceCommand {
        match self {
            Self::Ssh(target) => ssh_non_tty_command(target.clone(), remote_command),
            Self::Wsl(distro) => wsl_shell_command(distro, remote_command),
        }
    }
}

pub struct RemoteHelperManager<'a> {
    options: &'a RemoteWorkspaceBackendOptions,
    progress: Option<&'a dyn Fn(RemoteDeploymentProgress)>,
}

impl<'a> RemoteHelperManager<'a> {
    pub fn new(options: &'a RemoteWorkspaceBackendOptions) -> Self {
        Self {
            options,
            progress: None,
        }
    }

    pub fn with_progress(
        options: &'a RemoteWorkspaceBackendOptions,
        progress: &'a dyn Fn(RemoteDeploymentProgress),
    ) -> Self {
        Self {
            options,
            progress: Some(progress),
        }
    }

    pub fn resolve_helper_for_location(&self, location: &WorkspaceLocation) -> Result<PathBuf> {
        match location {
            WorkspaceLocation::Ssh { target, .. } => self.resolve_ssh_helper(
                &ssh_target_from_workspace_target_with_options(target, self.options),
            ),
            WorkspaceLocation::Wsl { distro, .. } => self.resolve_wsl_helper(distro),
            WorkspaceLocation::Local { .. } => Ok(self.options.remote_helper_path.clone()),
        }
    }

    pub fn reinstall_helper_for_location(
        &self,
        location: &WorkspaceLocation,
    ) -> Result<Option<PathBuf>> {
        match location {
            WorkspaceLocation::Ssh { target, .. } => self
                .reinstall_ssh_helper(&ssh_target_from_workspace_target_with_options(
                    target,
                    self.options,
                ))
                .map(Some),
            WorkspaceLocation::Wsl { distro, .. } => self.reinstall_wsl_helper(distro).map(Some),
            WorkspaceLocation::Local { .. } => Ok(None),
        }
    }

    fn resolve_ssh_helper(&self, target: &SshTarget) -> Result<PathBuf> {
        let helper_path = self
            .options
            .ssh_helper_path
            .as_deref()
            .unwrap_or(&self.options.remote_helper_path);
        let helper_path_is_override = self.options.ssh_helper_path_is_override
            || (self.options.ssh_helper_path.is_none()
                && self.options.remote_helper_path_is_override);
        self.resolve_linux_helper(
            LinuxHelperTransport::Ssh(target),
            self.options.ssh_helper_install_policy,
            helper_path,
            helper_path_is_override,
        )
    }

    fn resolve_wsl_helper(&self, distro: &str) -> Result<PathBuf> {
        let helper_path = self
            .options
            .wsl_helper_path
            .as_deref()
            .unwrap_or(&self.options.remote_helper_path);
        let helper_path_is_override = self.options.wsl_helper_path_is_override
            || (self.options.wsl_helper_path.is_none()
                && self.options.remote_helper_path_is_override);
        self.resolve_linux_helper(
            LinuxHelperTransport::Wsl(distro),
            self.options.wsl_helper_install_policy,
            helper_path,
            helper_path_is_override,
        )
    }

    fn resolve_linux_helper(
        &self,
        transport: LinuxHelperTransport<'_>,
        install_policy: RemoteHelperInstallPolicy,
        configured_helper_path: &Path,
        helper_path_is_override: bool,
    ) -> Result<PathBuf> {
        if helper_path_is_override
            && install_policy != RemoteHelperInstallPolicy::Upload
            && install_policy != RemoteHelperInstallPolicy::RemoteDownload
        {
            return Ok(configured_helper_path.to_path_buf());
        }

        if install_policy == RemoteHelperInstallPolicy::Never {
            return Ok(configured_helper_path.to_path_buf());
        }

        let connection_phase = match transport {
            LinuxHelperTransport::Ssh(_) => RemoteDeploymentPhase::ConnectingSshHost,
            LinuxHelperTransport::Wsl(_) => RemoteDeploymentPhase::StartingWslDistro,
        };
        self.emit_progress(connection_phase, Some(transport.target_name()), None);
        let probe = match self.probe_linux_platform(transport) {
            Ok(probe) => probe,
            Err(_) if install_policy == RemoteHelperInstallPolicy::Auto => {
                return Ok(configured_helper_path.to_path_buf());
            }
            Err(error) => return Err(error),
        };

        let helper_path = if helper_path_is_override {
            configured_helper_path.to_path_buf()
        } else {
            remote_linux_helper_path(&probe)
        };

        self.emit_progress(
            RemoteDeploymentPhase::CheckingRemoteHelper,
            Some(transport.target_name()),
            Some(helper_path.display().to_string()),
        );
        if self.remote_helper_matches(transport, &helper_path, &probe.platform) {
            return Ok(helper_path);
        }

        if install_policy == RemoteHelperInstallPolicy::RemoteDownload {
            self.install_helper_by_remote_download(transport, &probe.platform, &helper_path)?;
            if !self.remote_helper_matches(transport, &helper_path, &probe.platform) {
                bail!(
                    "downloaded nucleotide-remote on {} but version probe did not match protocol {}",
                    transport.description(),
                    PROTOCOL_VERSION
                );
            }
            return Ok(helper_path);
        }

        let Some(local_helper) = self.local_upload_artifact_for_platform(&probe.platform) else {
            if install_policy == RemoteHelperInstallPolicy::Auto
                && self
                    .install_helper_by_remote_download(transport, &probe.platform, &helper_path)
                    .is_ok()
            {
                if !self.remote_helper_matches(transport, &helper_path, &probe.platform) {
                    bail!(
                        "downloaded nucleotide-remote on {} but version probe did not match protocol {}",
                        transport.description(),
                        PROTOCOL_VERSION
                    );
                }
                return Ok(helper_path);
            }

            if install_policy == RemoteHelperInstallPolicy::Upload {
                bail!(
                    "helper upload requested for {}, but no local nucleotide-remote artifact is configured",
                    transport.description()
                );
            }
            return Ok(configured_helper_path.to_path_buf());
        };

        if !local_helper.is_file() {
            if install_policy == RemoteHelperInstallPolicy::Upload {
                bail!(
                    "helper upload requested for {}, but local artifact does not exist: {}",
                    transport.description(),
                    local_helper.display()
                );
            }
            return Ok(configured_helper_path.to_path_buf());
        }

        self.emit_progress(
            RemoteDeploymentPhase::InstallingRemoteHelper,
            Some(transport.target_name()),
            Some(format!("upload {}", local_helper.display())),
        );
        self.upload_helper(transport, &local_helper, &helper_path)
            .with_context(|| {
                format!(
                    "failed to upload nucleotide-remote to {}",
                    transport.description()
                )
            })?;

        if !self.remote_helper_matches(transport, &helper_path, &probe.platform) {
            bail!(
                "uploaded nucleotide-remote on {} but version probe did not match protocol {}",
                transport.description(),
                PROTOCOL_VERSION
            );
        }

        Ok(helper_path)
    }

    fn reinstall_ssh_helper(&self, target: &SshTarget) -> Result<PathBuf> {
        let helper_path = self
            .options
            .ssh_helper_path
            .as_deref()
            .unwrap_or(&self.options.remote_helper_path);
        let helper_path_is_override = self.options.ssh_helper_path_is_override
            || (self.options.ssh_helper_path.is_none()
                && self.options.remote_helper_path_is_override);
        self.reinstall_linux_helper(
            LinuxHelperTransport::Ssh(target),
            self.options.ssh_helper_install_policy,
            helper_path,
            helper_path_is_override,
        )
    }

    fn reinstall_wsl_helper(&self, distro: &str) -> Result<PathBuf> {
        let helper_path = self
            .options
            .wsl_helper_path
            .as_deref()
            .unwrap_or(&self.options.remote_helper_path);
        let helper_path_is_override = self.options.wsl_helper_path_is_override
            || (self.options.wsl_helper_path.is_none()
                && self.options.remote_helper_path_is_override);
        self.reinstall_linux_helper(
            LinuxHelperTransport::Wsl(distro),
            self.options.wsl_helper_install_policy,
            helper_path,
            helper_path_is_override,
        )
    }

    fn reinstall_linux_helper(
        &self,
        transport: LinuxHelperTransport<'_>,
        install_policy: RemoteHelperInstallPolicy,
        configured_helper_path: &Path,
        helper_path_is_override: bool,
    ) -> Result<PathBuf> {
        if helper_path_is_override
            && install_policy != RemoteHelperInstallPolicy::Upload
            && install_policy != RemoteHelperInstallPolicy::RemoteDownload
        {
            bail!(
                "a custom helper path is set; automatic helper reinstall is disabled for {}",
                transport.description()
            );
        }

        if install_policy == RemoteHelperInstallPolicy::Never {
            bail!(
                "helper auto-install is disabled for {}",
                transport.description()
            );
        }

        let probe = self.probe_linux_platform(transport)?;
        let helper_path = if helper_path_is_override {
            configured_helper_path.to_path_buf()
        } else {
            remote_linux_helper_path(&probe)
        };

        if install_policy == RemoteHelperInstallPolicy::RemoteDownload {
            self.install_helper_by_remote_download(transport, &probe.platform, &helper_path)?;
            if !self.remote_helper_matches(transport, &helper_path, &probe.platform) {
                bail!(
                    "reinstalled nucleotide-remote on {} by download but version probe did not match protocol {}",
                    transport.description(),
                    PROTOCOL_VERSION
                );
            }
            return Ok(helper_path);
        }

        let local_helper = self
            .local_upload_artifact_for_platform(&probe.platform)
            .with_context(|| {
                format!(
                    "no bundled helper for {}-{}",
                    probe.platform.os, probe.platform.arch
                )
            })?;

        if !local_helper.is_file() {
            bail!(
                "local helper artifact does not exist: {}",
                local_helper.display()
            );
        }

        self.emit_progress(
            RemoteDeploymentPhase::InstallingRemoteHelper,
            Some(transport.target_name()),
            Some(format!("upload {}", local_helper.display())),
        );
        self.upload_helper(transport, &local_helper, &helper_path)
            .with_context(|| {
                format!(
                    "failed to reinstall nucleotide-remote on {}",
                    transport.description()
                )
            })?;

        if !self.remote_helper_matches(transport, &helper_path, &probe.platform) {
            bail!(
                "reinstalled nucleotide-remote on {} but version probe did not match protocol {}",
                transport.description(),
                PROTOCOL_VERSION
            );
        }

        Ok(helper_path)
    }

    fn local_upload_artifact_for_platform(&self, platform: &SshRemotePlatform) -> Option<PathBuf> {
        if let Some(path) = self.options.ssh_helper_upload_path.as_ref() {
            return Some(path.clone());
        }

        let artifact_dir = self.options.ssh_helper_artifact_dir.as_ref()?;
        bundled_ssh_helper_artifact_path(artifact_dir, platform)
    }

    fn probe_linux_platform(&self, transport: LinuxHelperTransport<'_>) -> Result<SshRemoteProbe> {
        self.emit_progress(
            RemoteDeploymentPhase::DetectingRemotePlatform,
            Some(transport.target_name()),
            None,
        );
        let script = concat!(
            "printf 'NUCL_PLATFORM '; uname -sm; ",
            "printf 'NUCL_CACHE %s\\n' \"${XDG_CACHE_HOME:-$HOME/.cache}\""
        );
        let output = self.run_linux_command_output(
            transport,
            "detecting remote platform",
            &format!("sh -lc {}", quote_posix_shell(script)),
        )?;
        parse_linux_probe_output(&output)
    }

    fn remote_helper_matches(
        &self,
        transport: LinuxHelperTransport<'_>,
        helper_path: &Path,
        platform: &SshRemotePlatform,
    ) -> bool {
        self.remote_helper_version(transport, helper_path)
            .map(|info| helper_version_matches_current(&info, platform))
            .unwrap_or(false)
    }

    fn remote_helper_version(
        &self,
        transport: LinuxHelperTransport<'_>,
        helper_path: &Path,
    ) -> Result<HelperVersionInfo> {
        let helper_path = posix_path_string(helper_path);
        let remote_command = format!("exec {} version --json", quote_posix_shell(&helper_path));
        let output = self.run_linux_command_output(
            transport,
            "checking nucleotide-remote",
            &remote_command,
        )?;
        parse_helper_version_output(&output)
    }

    fn install_helper_by_remote_download(
        &self,
        transport: LinuxHelperTransport<'_>,
        platform: &SshRemotePlatform,
        helper_path: &Path,
    ) -> Result<()> {
        let asset_name = remote_helper_release_asset_name(platform);
        let (asset_url, checksums_url) = self.remote_helper_download_urls(platform)?;
        self.emit_progress(
            RemoteDeploymentPhase::InstallingRemoteHelper,
            Some(transport.target_name()),
            Some(format!("download {asset_name}")),
        );
        self.remote_download_helper(
            transport,
            helper_path,
            &asset_url,
            &checksums_url,
            &asset_name,
        )
        .with_context(|| {
            format!(
                "failed to download nucleotide-remote on {}",
                transport.description()
            )
        })
    }

    fn remote_helper_download_urls(
        &self,
        platform: &SshRemotePlatform,
    ) -> Result<(String, String)> {
        let base_url = self
            .options
            .ssh_helper_download_base_url
            .clone()
            .unwrap_or_else(default_remote_helper_download_base_url);
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            bail!("remote helper download base URL is empty");
        }

        Ok((
            format!("{base_url}/{}", remote_helper_release_asset_name(platform)),
            format!("{base_url}/{RELEASE_CHECKSUMS_ASSET}"),
        ))
    }

    fn upload_helper(
        &self,
        transport: LinuxHelperTransport<'_>,
        local_helper: &Path,
        helper_path: &Path,
    ) -> Result<()> {
        let helper_path = posix_path_string(helper_path);
        let helper_dir = posix_parent(&helper_path);
        let tmp_path = format!(
            "{helper_path}.tmp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        let mut local_file = std::fs::File::open(local_helper)
            .with_context(|| format!("failed to open {}", local_helper.display()))?;
        let expected_sha256 = sha256_reader(&mut local_file)
            .with_context(|| format!("failed to hash {}", local_helper.display()))?;
        local_file
            .rewind()
            .with_context(|| format!("failed to rewind {}", local_helper.display()))?;
        let remote_command =
            remote_helper_upload_command(&helper_dir, &tmp_path, &helper_path, &expected_sha256);
        let spec = transport.command(remote_command);
        let mut command = spec.command();
        command.stdin(Stdio::from(local_file));
        let output = nucleotide_process::output_with_limits(
            &mut command,
            nucleotide_process::OutputLimits::new(
                REMOTE_HELPER_TRANSFER_TIMEOUT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
            ),
        )
        .with_context(|| {
            format!(
                "failed to run helper upload command for {}: {}",
                transport.description(),
                spec.display_context()
            )
        })?;

        if output.status.success() {
            Ok(())
        } else {
            bail!(
                "helper upload command for {} failed with status {}: {}",
                transport.description(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
    }

    fn remote_download_helper(
        &self,
        transport: LinuxHelperTransport<'_>,
        helper_path: &Path,
        asset_url: &str,
        checksums_url: &str,
        asset_name: &str,
    ) -> Result<()> {
        let helper_path = posix_path_string(helper_path);
        let helper_dir = posix_parent(&helper_path);
        let tmp_path = format!(
            "{helper_path}.tmp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        let remote_command = remote_helper_download_command(
            &helper_dir,
            &tmp_path,
            &helper_path,
            asset_url,
            checksums_url,
            asset_name,
        );
        let spec = transport.command(remote_command);
        let mut command = spec.command();
        command.stdin(Stdio::null());
        let output = nucleotide_process::output_with_limits(
            &mut command,
            nucleotide_process::OutputLimits::new(
                REMOTE_HELPER_TRANSFER_TIMEOUT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
            ),
        )
        .with_context(|| {
            format!(
                "failed to run helper download command for {}",
                transport.description()
            )
        })?;

        if output.status.success() {
            Ok(())
        } else {
            bail!(
                "helper download command for {} failed with status {}: {}",
                transport.description(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
    }

    fn run_linux_command_output(
        &self,
        transport: LinuxHelperTransport<'_>,
        label: &'static str,
        remote_command: &str,
    ) -> Result<String> {
        let spec = transport.command(remote_command.to_string());
        let mut command = spec.command();
        command.stdin(Stdio::null());
        let output = nucleotide_process::output_with_limits(
            &mut command,
            nucleotide_process::OutputLimits::new(
                REMOTE_STARTUP_PROBE_TIMEOUT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
            ),
        )
        .with_context(|| {
            format!(
                "failed to run command on {} while {label}: {}",
                transport.description(),
                spec.display_context()
            )
        })?;

        if output.status.success() {
            String::from_utf8(output.stdout).with_context(|| {
                format!(
                    "command on {} while {label} returned non-UTF-8 stdout",
                    transport.description()
                )
            })
        } else {
            bail!(
                "command on {} while {label} failed with status {}: {}",
                transport.description(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
    }

    fn emit_progress(
        &self,
        phase: RemoteDeploymentPhase,
        target: Option<String>,
        detail: Option<String>,
    ) {
        if let Some(progress) = self.progress {
            progress(RemoteDeploymentProgress {
                phase,
                target,
                detail,
            });
        }
    }
}

pub fn resolve_remote_helper_for_location(
    location: &WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
) -> Result<PathBuf> {
    RemoteHelperManager::new(options).resolve_helper_for_location(location)
}

pub fn resolve_remote_helper_for_location_with_progress(
    location: &WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
    progress: &dyn Fn(RemoteDeploymentProgress),
) -> Result<PathBuf> {
    RemoteHelperManager::with_progress(options, progress).resolve_helper_for_location(location)
}

pub fn resolved_remote_lsp_proxy_command_for_location(
    location: &WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
    server: impl AsRef<OsStr>,
) -> Result<Option<RemoteServiceCommand>> {
    let helper_path = resolve_remote_helper_for_location(location, options)?;
    Ok(remote_lsp_proxy_command_for_location_with_options(
        location,
        helper_path,
        options,
        server,
    ))
}

pub fn resolved_remote_terminal_proxy_command_for_location(
    location: &WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> Result<Option<RemoteServiceCommand>> {
    let helper_path = resolve_remote_helper_for_location(location, options)?;
    Ok(remote_terminal_proxy_command_for_location_with_options(
        location,
        helper_path,
        options,
        shell,
        command,
        env,
    ))
}

#[derive(Clone)]
pub struct WorkspaceBackendConnection {
    pub backend: WorkspaceBackendHandle,
    pub location: WorkspaceLocation,
    pub hello: Option<HelloResponse>,
}

pub fn remote_workspace_identity_for_location(
    location: &WorkspaceLocation,
) -> Option<RemoteWorkspaceIdentity> {
    match location {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl { distro, .. } => Some(RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Wsl,
            name: distro.clone(),
        }),
        WorkspaceLocation::Ssh { target, .. } => Some(RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Ssh,
            name: ssh_target_display_name(target),
        }),
    }
}

pub fn remote_service_command_for_location(
    location: &WorkspaceLocation,
    helper_path: impl AsRef<Path>,
) -> Option<RemoteServiceCommand> {
    match location {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            distro, linux_path, ..
        } => Some(wsl_service_command(distro, linux_path, helper_path)),
        WorkspaceLocation::Ssh { target, path, .. } => Some(ssh_service_command(
            ssh_target_from_workspace_target(target),
            path,
            helper_path,
        )),
    }
}

fn remote_service_command_for_location_with_options(
    location: &WorkspaceLocation,
    helper_path: impl AsRef<Path>,
    options: &RemoteWorkspaceBackendOptions,
) -> Option<RemoteServiceCommand> {
    match location {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            distro, linux_path, ..
        } => Some(wsl_service_command(distro, linux_path, helper_path)),
        WorkspaceLocation::Ssh { target, path, .. } => Some(ssh_service_command(
            ssh_target_from_workspace_target_with_options(target, options),
            path,
            helper_path,
        )),
    }
}

pub fn remote_lsp_proxy_command_for_location(
    location: &WorkspaceLocation,
    helper_path: impl AsRef<Path>,
    server: impl AsRef<OsStr>,
) -> Option<RemoteServiceCommand> {
    match location {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            distro, linux_path, ..
        } => Some(wsl_lsp_proxy_command(
            distro,
            linux_path,
            helper_path,
            server,
        )),
        WorkspaceLocation::Ssh { target, path, .. } => Some(ssh_lsp_proxy_command(
            ssh_target_from_workspace_target(target),
            path,
            helper_path,
            server,
        )),
    }
}

pub fn remote_lsp_proxy_command_for_location_with_options(
    location: &WorkspaceLocation,
    helper_path: impl AsRef<Path>,
    options: &RemoteWorkspaceBackendOptions,
    server: impl AsRef<OsStr>,
) -> Option<RemoteServiceCommand> {
    match location {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            distro, linux_path, ..
        } => Some(wsl_lsp_proxy_command(
            distro,
            linux_path,
            helper_path,
            server,
        )),
        WorkspaceLocation::Ssh { target, path, .. } => Some(ssh_lsp_proxy_command(
            ssh_target_from_workspace_target_with_options(target, options),
            path,
            helper_path,
            server,
        )),
    }
}

pub fn remote_interactive_terminal_command_for_location_with_options(
    location: &WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
) -> Option<RemoteServiceCommand> {
    match location {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            distro, linux_path, ..
        } => Some(wsl_interactive_terminal_command(distro, linux_path)),
        WorkspaceLocation::Ssh { target, path, .. } => Some(ssh_interactive_terminal_command(
            ssh_target_from_workspace_target_with_options(target, options),
            path,
        )),
    }
}

pub fn remote_terminal_proxy_command_for_location(
    location: &WorkspaceLocation,
    helper_path: impl AsRef<Path>,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> Option<RemoteServiceCommand> {
    let helper_path = helper_path.as_ref();
    match location {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            distro, linux_path, ..
        } => Some(wsl_terminal_proxy_command(
            distro,
            linux_path,
            helper_path,
            shell,
            command,
            env,
        )),
        WorkspaceLocation::Ssh { target, path, .. } => Some(ssh_terminal_proxy_command(
            ssh_target_from_workspace_target(target),
            path,
            helper_path,
            shell,
            command,
            env,
        )),
    }
}

fn remote_terminal_proxy_command_for_location_with_options(
    location: &WorkspaceLocation,
    helper_path: impl AsRef<Path>,
    options: &RemoteWorkspaceBackendOptions,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> Option<RemoteServiceCommand> {
    let helper_path = helper_path.as_ref();
    match location {
        WorkspaceLocation::Local { .. } => None,
        WorkspaceLocation::Wsl {
            distro, linux_path, ..
        } => Some(wsl_terminal_proxy_command(
            distro,
            linux_path,
            helper_path,
            shell,
            command,
            env,
        )),
        WorkspaceLocation::Ssh { target, path, .. } => Some(ssh_terminal_proxy_command(
            ssh_target_from_workspace_target_with_options(target, options),
            path,
            helper_path,
            shell,
            command,
            env,
        )),
    }
}

pub fn connect_workspace_backend_for_location(
    location: WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
) -> Result<WorkspaceBackendConnection> {
    connect_workspace_backend_for_location_with_optional_progress(location, options, None)
}

pub fn connect_workspace_backend_for_location_with_progress(
    location: WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
    progress: &dyn Fn(RemoteDeploymentProgress),
) -> Result<WorkspaceBackendConnection> {
    connect_workspace_backend_for_location_with_optional_progress(location, options, Some(progress))
}

fn connect_workspace_backend_for_location_with_optional_progress(
    location: WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
    progress: Option<&dyn Fn(RemoteDeploymentProgress)>,
) -> Result<WorkspaceBackendConnection> {
    if let WorkspaceLocation::Local { path } = &location {
        if options.use_local_service {
            let helper_path = options
                .local_helper_path
                .as_deref()
                .unwrap_or(&options.remote_helper_path);
            let command = local_service_command(helper_path, path);
            let (backend, hello) = spawn_child_process_workspace_backend(
                RemoteWorkspaceIdentity {
                    kind: RemoteWorkspaceKind::Other("local-service".to_string()),
                    name: "local-service".to_string(),
                },
                &command,
            )
            .with_context(|| {
                format!(
                    "failed to initialize local workspace service for {}. {}",
                    path.display(),
                    local_helper_setup_hint(helper_path)
                )
            })?;

            return Ok(WorkspaceBackendConnection {
                backend,
                location,
                hello: Some(hello),
            });
        }

        return Ok(WorkspaceBackendConnection {
            backend: local_workspace_backend(),
            location,
            hello: None,
        });
    }

    let identity = remote_workspace_identity_for_location(&location)
        .context("remote workspace location is missing an identity")?;
    let helper_path = match progress {
        Some(progress) => {
            resolve_remote_helper_for_location_with_progress(&location, options, progress)?
        }
        None => resolve_remote_helper_for_location(&location, options)?,
    };
    let command =
        remote_service_command_for_location_with_options(&location, &helper_path, options)
            .context("remote workspace location is missing a service command")?;
    let mapping = location.path_mapping();
    let display_root = location.display_root().to_path_buf();
    emit_remote_deployment_progress(
        progress,
        RemoteDeploymentPhase::StartingRemoteWorkspaceService,
        &location,
        Some(display_root.display().to_string()),
    );
    let (backend, hello) = match spawn_child_process_workspace_backend(identity.clone(), &command) {
        Ok(connection) => connection,
        Err(error) if remote_startup_error_can_retry_helper_install(&location, &error) => {
            let retry_helper_path = match progress {
                Some(progress) => RemoteHelperManager::with_progress(options, progress),
                None => RemoteHelperManager::new(options),
            }
            .reinstall_helper_for_location(&location)
            .with_context(|| {
                format!(
                    "failed to reinstall remote helper after startup failure. Initial error: {error:#}"
                )
            })?
            .context("remote helper reinstall did not apply to this workspace location")?;
            let retry_command = remote_service_command_for_location_with_options(
                &location,
                &retry_helper_path,
                options,
            )
            .context("remote workspace location is missing a service command")?;
            emit_remote_deployment_progress(
                progress,
                RemoteDeploymentPhase::StartingRemoteWorkspaceService,
                &location,
                Some(display_root.display().to_string()),
            );
            spawn_child_process_workspace_backend(identity, &retry_command)
            .with_context(|| {
                format!(
                    "failed to initialize remote workspace service for {} after reinstalling helper. Initial error: {error:#}",
                    display_root.display()
                )
            })?
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to initialize remote workspace service for {}. {}",
                    display_root.display(),
                    remote_helper_setup_hint(&location, &helper_path)
                )
            });
        }
    };

    Ok(WorkspaceBackendConnection {
        backend: path_mapped_workspace_backend(backend, mapping),
        location,
        hello: Some(hello),
    })
}

fn emit_remote_deployment_progress(
    progress: Option<&dyn Fn(RemoteDeploymentProgress)>,
    phase: RemoteDeploymentPhase,
    location: &WorkspaceLocation,
    detail: Option<String>,
) {
    if let Some(progress) = progress {
        progress(RemoteDeploymentProgress {
            phase,
            target: remote_deployment_target(location),
            detail,
        });
    }
}

fn remote_deployment_target(location: &WorkspaceLocation) -> Option<String> {
    match location {
        WorkspaceLocation::Ssh { target, .. } => Some(ssh_target_display_name(target)),
        WorkspaceLocation::Wsl { distro, .. } => Some(distro.clone()),
        WorkspaceLocation::Local { .. } => None,
    }
}

fn remote_startup_error_can_retry_helper_install(
    location: &WorkspaceLocation,
    error: &anyhow::Error,
) -> bool {
    if !matches!(
        location,
        WorkspaceLocation::Ssh { .. } | WorkspaceLocation::Wsl { .. }
    ) {
        return false;
    }

    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("protocol v5")
            || message.contains("frame header version")
            || message.contains("invalid frame magic")
            || message.contains("remote service disconnected")
            || message.contains("verify the helper speaks protocol v5")
    })
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(false)
}

fn env_flag_enabled_with_default(value: Option<&str>, default: bool) -> bool {
    match value {
        None | Some("") => default,
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES") | Some("on")
        | Some("ON") => true,
        Some("0") | Some("false") | Some("FALSE") | Some("no") | Some("NO") | Some("off")
        | Some("OFF") => false,
        Some(_) => default,
    }
}

fn ssh_connect_timeout_from_env(value: Option<&str>) -> Option<u64> {
    match value {
        None | Some("") => Some(DEFAULT_SSH_CONNECT_TIMEOUT_SECS),
        Some("0") => None,
        Some(value) => value
            .parse::<u64>()
            .ok()
            .filter(|timeout| *timeout > 0)
            .or(Some(DEFAULT_SSH_CONNECT_TIMEOUT_SECS)),
    }
}

fn ssh_extra_args_from_env(value: Option<OsString>) -> Vec<OsString> {
    let Some(value) = value else {
        return Vec::new();
    };
    split_ssh_extra_args(&value.to_string_lossy())
        .into_iter()
        .map(OsString::from)
        .collect()
}

fn split_ssh_extra_args(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, ch) if ch.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            (None, '\'' | '"') => quote = Some(ch),
            (Some(active), ch) if ch == active => quote = None,
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

fn default_ssh_control_master_enabled() -> bool {
    cfg!(unix)
}

pub fn default_ssh_control_path() -> Option<PathBuf> {
    if !cfg!(unix) {
        return None;
    }

    Some(short_ssh_control_dir().join("%C"))
}

fn short_ssh_control_dir() -> PathBuf {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".to_string());
    let mut user = user
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .take(16)
        .collect::<String>();
    if user.is_empty() {
        user.push_str("user");
    }

    PathBuf::from("/tmp").join(format!("nucl-ssh-{user}"))
}

#[cfg(unix)]
fn ensure_private_ssh_control_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o700);
    let _ = std::fs::set_permissions(path, permissions);
}

#[cfg(not(unix))]
fn ensure_private_ssh_control_dir(_path: &Path) {}

fn ssh_non_tty_command(target: SshTarget, remote_command: String) -> RemoteServiceCommand {
    let mut args = Vec::new();
    args.push(OsString::from("-T"));
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

fn wsl_shell_command(distro: impl AsRef<OsStr>, remote_command: String) -> RemoteServiceCommand {
    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args: vec![
            OsString::from("--distribution"),
            distro.as_ref().to_os_string(),
            OsString::from("--exec"),
            OsString::from("sh"),
            OsString::from("-lc"),
            OsString::from(remote_command),
        ],
        current_dir: None,
    }
}

pub fn ssh_non_tty_remote_command(
    target: SshTarget,
    remote_command: impl Into<String>,
) -> RemoteServiceCommand {
    ssh_non_tty_command(target, remote_command.into())
}

fn parse_linux_probe_output(output: &str) -> Result<SshRemoteProbe> {
    let mut platform = None;
    let mut cache_root = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("NUCL_PLATFORM ") {
            platform = Some(parse_uname_platform(value)?);
        } else if let Some(value) = line.strip_prefix("NUCL_CACHE ") {
            let value = value.trim();
            if !value.is_empty() {
                cache_root = Some(value.to_string());
            }
        }
    }

    Ok(SshRemoteProbe {
        platform: platform.context("Linux platform probe did not report NUCL_PLATFORM")?,
        cache_root: cache_root.context("Linux platform probe did not report NUCL_CACHE")?,
    })
}

fn parse_uname_platform(value: &str) -> Result<SshRemotePlatform> {
    let mut parts = value.split_whitespace();
    let os = parts.next().context("remote uname output is missing OS")?;
    let arch = parts
        .next()
        .context("remote uname output is missing architecture")?;

    let os = match os {
        "Linux" => "linux",
        other => bail!("unsupported remote Linux platform: {other} {arch}"),
    };
    let arch = match arch {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        arch if arch.starts_with("armv8") || arch.starts_with("armv9") => "aarch64",
        other => bail!("unsupported remote Linux platform: {os} {other}"),
    };

    Ok(SshRemotePlatform {
        os: os.to_string(),
        arch: arch.to_string(),
    })
}

fn remote_linux_helper_path(probe: &SshRemoteProbe) -> PathBuf {
    PathBuf::from(posix_join(
        &probe.cache_root,
        &[
            "nucleotide",
            "remote",
            &format!("protocol-{PROTOCOL_VERSION}"),
            &remote_helper_file_name(&probe.platform),
        ],
    ))
}

fn remote_helper_file_name(platform: &SshRemotePlatform) -> String {
    format!(
        "nucleotide-remote-{}-{}-{}",
        env!("CARGO_PKG_VERSION"),
        platform.os,
        platform.arch
    )
}

fn remote_helper_release_asset_name(platform: &SshRemotePlatform) -> String {
    format!("nucleotide-remote-{}-{}", platform.os, platform.arch)
}

fn default_remote_helper_download_base_url() -> String {
    format!(
        "{}/releases/download/{DEFAULT_RELEASE_TAG_PREFIX}{}",
        env!("CARGO_PKG_REPOSITORY").trim_end_matches('/'),
        env!("CARGO_PKG_VERSION")
    )
}

fn helper_version_matches_current(info: &HelperVersionInfo, platform: &SshRemotePlatform) -> bool {
    info.helper_version == env!("CARGO_PKG_VERSION")
        && info.protocol_version == PROTOCOL_VERSION
        && info.frame_version == FRAME_VERSION
        && info.os == platform.os
        && info.arch == platform.arch
}

fn parse_helper_version_output(output: &str) -> Result<HelperVersionInfo> {
    let trimmed = output.trim();
    if let Ok(info) = serde_json::from_str::<HelperVersionInfo>(trimmed) {
        return Ok(info);
    }

    for line in output.lines().rev() {
        let line = line.trim();
        if line.starts_with('{') {
            return serde_json::from_str(line)
                .context("failed to parse nucleotide-remote version JSON");
        }
    }

    bail!("nucleotide-remote version output did not contain JSON")
}

fn posix_join(base: &str, parts: &[&str]) -> String {
    let mut output = base.trim_end_matches('/').to_string();
    for part in parts {
        let part = part.trim_matches('/');
        if part.is_empty() {
            continue;
        }
        if output.is_empty() || !output.ends_with('/') {
            output.push('/');
        }
        output.push_str(part);
    }
    output
}

fn posix_parent(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) if !parent.is_empty() => parent.to_string(),
        _ => ".".to_string(),
    }
}

fn sha256_reader(reader: &mut impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(format!("{:x}", hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

fn remote_helper_upload_command(
    helper_dir: &str,
    tmp_path: &str,
    helper_path: &str,
    expected_sha256: &str,
) -> String {
    let script = concat!(
        "set -eu\n",
        "dir=$1\n",
        "tmp=$2\n",
        "final=$3\n",
        "expected=$4\n",
        "hash_file() {\n",
        "  file=$1\n",
        "  if command -v sha256sum >/dev/null 2>&1; then\n",
        "    sha256sum \"$file\" | awk '{print $1}'\n",
        "  elif command -v shasum >/dev/null 2>&1; then\n",
        "    shasum -a 256 \"$file\" | awk '{print $1}'\n",
        "  else\n",
        "    echo \"neither sha256sum nor shasum is available for helper verification\" >&2\n",
        "    return 127\n",
        "  fi\n",
        "}\n",
        "cleanup() { rm -f \"$tmp\"; }\n",
        "trap cleanup EXIT\n",
        "trap \"exit 1\" INT TERM HUP\n",
        "mkdir -p \"$dir\"\n",
        "chmod 700 \"$dir\"\n",
        "cat > \"$tmp\"\n",
        "actual=$(hash_file \"$tmp\")\n",
        "if [ \"$expected\" != \"$actual\" ]; then\n",
        "  echo \"checksum mismatch for uploaded helper\" >&2\n",
        "  exit 1\n",
        "fi\n",
        "chmod 755 \"$tmp\"\n",
        "mv -f \"$tmp\" \"$final\"\n",
    );

    format!(
        "sh -lc {} sh {} {} {} {}",
        quote_posix_shell(script),
        quote_posix_shell(helper_dir),
        quote_posix_shell(tmp_path),
        quote_posix_shell(helper_path),
        quote_posix_shell(expected_sha256)
    )
}

fn remote_helper_download_command(
    helper_dir: &str,
    tmp_path: &str,
    helper_path: &str,
    asset_url: &str,
    checksums_url: &str,
    asset_name: &str,
) -> String {
    let script = concat!(
        "set -eu\n",
        "dir=$1\n",
        "tmp=$2\n",
        "final=$3\n",
        "asset_url=$4\n",
        "checksums_url=$5\n",
        "asset_name=$6\n",
        "sums=\"$tmp.sha256sums\"\n",
        "download() {\n",
        "  url=$1\n",
        "  out=$2\n",
        "  if command -v curl >/dev/null 2>&1; then\n",
        "    curl -fsSL \"$url\" -o \"$out\"\n",
        "  elif command -v wget >/dev/null 2>&1; then\n",
        "    wget -qO \"$out\" \"$url\"\n",
        "  else\n",
        "    echo \"neither curl nor wget is available for remote helper download\" >&2\n",
        "    return 127\n",
        "  fi\n",
        "}\n",
        "hash_file() {\n",
        "  file=$1\n",
        "  if command -v sha256sum >/dev/null 2>&1; then\n",
        "    sha256sum \"$file\" | awk '{print $1}'\n",
        "  elif command -v shasum >/dev/null 2>&1; then\n",
        "    shasum -a 256 \"$file\" | awk '{print $1}'\n",
        "  else\n",
        "    echo \"neither sha256sum nor shasum is available for helper verification\" >&2\n",
        "    return 127\n",
        "  fi\n",
        "}\n",
        "cleanup() { rm -f \"$tmp\" \"$sums\"; }\n",
        "trap cleanup EXIT\n",
        "trap \"exit 1\" INT TERM HUP\n",
        "mkdir -p \"$dir\"\n",
        "chmod 700 \"$dir\"\n",
        "rm -f \"$tmp\" \"$sums\"\n",
        "download \"$asset_url\" \"$tmp\"\n",
        "download \"$checksums_url\" \"$sums\"\n",
        "expected=$(awk -v name=\"$asset_name\" '$2 == name || $2 == (\"*\" name) { print $1; found=1; exit } END { if (!found) exit 1 }' \"$sums\") || {\n",
        "  echo \"checksum for $asset_name not found in SHA256SUMS\" >&2\n",
        "  cleanup\n",
        "  exit 1\n",
        "}\n",
        "actual=$(hash_file \"$tmp\")\n",
        "if [ \"$expected\" != \"$actual\" ]; then\n",
        "  echo \"checksum mismatch for $asset_name\" >&2\n",
        "  cleanup\n",
        "  exit 1\n",
        "fi\n",
        "chmod 755 \"$tmp\"\n",
        "mv -f \"$tmp\" \"$final\"\n",
    );

    format!(
        "sh -lc {} sh {} {} {} {} {} {}",
        quote_posix_shell(script),
        quote_posix_shell(helper_dir),
        quote_posix_shell(tmp_path),
        quote_posix_shell(helper_path),
        quote_posix_shell(asset_url),
        quote_posix_shell(checksums_url),
        quote_posix_shell(asset_name)
    )
}

fn bundled_local_helper_path(current_exe: &Path) -> Option<PathBuf> {
    let executable_dir = current_exe.parent()?;
    let helper_path = executable_dir.join(local_helper_binary_name());
    helper_path.is_file().then_some(helper_path)
}

fn bundled_ssh_helper_artifact_path(
    artifact_dir: &Path,
    platform: &SshRemotePlatform,
) -> Option<PathBuf> {
    for candidate in ssh_helper_artifact_candidate_names(platform) {
        let path = artifact_dir.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn ssh_helper_artifact_candidate_names(platform: &SshRemotePlatform) -> Vec<String> {
    let mut candidates = vec![
        remote_helper_file_name(platform),
        format!("nucleotide-remote-{}-{}", platform.os, platform.arch),
    ];

    if current_host_platform_matches(platform) {
        candidates.push(local_helper_binary_name().to_string());
    }

    candidates
}

fn current_host_platform_matches(platform: &SshRemotePlatform) -> bool {
    let Some(host) = current_host_remote_platform() else {
        return false;
    };

    host == *platform
}

fn current_host_remote_platform() -> Option<SshRemotePlatform> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };

    Some(SshRemotePlatform {
        os: os.to_string(),
        arch: arch.to_string(),
    })
}

fn local_helper_binary_name() -> &'static str {
    if cfg!(windows) {
        "nucleotide-remote.exe"
    } else {
        "nucleotide-remote"
    }
}

fn local_helper_setup_hint(helper_path: &Path) -> String {
    format!(
        "Local service mode needs nucleotide-remote at {}. Set NUCLEOTIDE_LOCAL_REMOTE_HELPER or place {} next to the nucl executable.",
        helper_path.display(),
        local_helper_binary_name()
    )
}

fn remote_helper_setup_hint(location: &WorkspaceLocation, helper_path: &Path) -> String {
    match location {
        WorkspaceLocation::Wsl { distro, .. } => format!(
            "Nucleotide could not install or start nucleotide-remote in WSL distro {distro} at {}. Check [remote.wsl], or set NUCLEOTIDE_REMOTE_HELPER to a Linux path visible in that distro.",
            helper_path.display()
        ),
        WorkspaceLocation::Ssh { target, .. } => format!(
            "Install nucleotide-remote on SSH target {} at {} or set NUCLEOTIDE_REMOTE_HELPER to a remote path visible after login.",
            ssh_target_display_name(target),
            helper_path.display()
        ),
        WorkspaceLocation::Local { .. } => local_helper_setup_hint(helper_path),
    }
}

fn ssh_target_from_workspace_target(target: &SshWorkspaceTarget) -> SshTarget {
    SshTarget {
        host: target.host.clone(),
        user: target.user.clone(),
        port: target.port,
        connect_timeout_secs: None,
        extra_args: Vec::new(),
        control_path: None,
    }
}

fn ssh_target_from_workspace_target_with_options(
    target: &SshWorkspaceTarget,
    options: &RemoteWorkspaceBackendOptions,
) -> SshTarget {
    SshTarget {
        host: target.host.clone(),
        user: target.user.clone(),
        port: target.port,
        connect_timeout_secs: options.ssh_connect_timeout_secs,
        extra_args: options.ssh_extra_args.clone(),
        control_path: options.ssh_control_path.clone(),
    }
}

fn ssh_target_display_name(target: &SshWorkspaceTarget) -> String {
    let host = ssh_display_host(&target.host);
    let mut name = match &target.user {
        Some(user) if !user.is_empty() => format!("{user}@{host}"),
        _ => host,
    };
    if let Some(port) = target.port {
        name.push(':');
        name.push_str(&port.to_string());
    }
    name
}

fn ssh_display_host(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteFileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStatResponse {
    pub path: PathBuf,
    pub kind: RemoteFileKind,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
    #[serde(default)]
    pub modified_unix_nanos: Option<u32>,
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntryResponse {
    pub name: String,
    pub path: PathBuf,
    pub stat: FileStatResponse,
    pub symlink_target: Option<PathBuf>,
    pub target_exists: Option<bool>,
    #[serde(default)]
    pub ignored: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryListingDeltaResponse {
    #[serde(default)]
    pub base_generation: Option<u64>,
    #[serde(default)]
    pub base_fingerprint: Option<u64>,
    #[serde(default)]
    pub added: Vec<DirectoryEntryResponse>,
    #[serde(default)]
    pub updated: Vec<DirectoryEntryResponse>,
    #[serde(default)]
    pub removed: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryListingResponse {
    pub path: PathBuf,
    #[serde(default)]
    pub generation: Option<u64>,
    #[serde(default)]
    pub fingerprint: Option<u64>,
    #[serde(default = "default_true")]
    pub complete: bool,
    #[serde(default)]
    pub not_modified: bool,
    #[serde(default)]
    pub delta: Option<DirectoryListingDeltaResponse>,
    pub entries: Vec<DirectoryEntryResponse>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryListingResultResponse {
    pub path: PathBuf,
    pub listing: Option<DirectoryListingResponse>,
    pub error: Option<RemoteError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListDirsResponse {
    pub results: Vec<DirectoryListingResultResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadResponse {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
    #[serde(default)]
    pub modified_unix_nanos: Option<u32>,
    pub readonly: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteResultResponse {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
    #[serde(default)]
    pub modified_unix_nanos: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchRequest {
    #[serde(serialize_with = "serialize_posix_path")]
    pub root: PathBuf,
    pub pattern: Option<String>,
    pub limit: usize,
    pub hidden: bool,
    pub parents: bool,
    pub ignore: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
    pub follow_links: bool,
    pub max_depth: Option<usize>,
    #[serde(serialize_with = "serialize_posix_paths")]
    pub excluded_relative_prefixes: Vec<PathBuf>,
}

impl Default for FileSearchRequest {
    fn default() -> Self {
        let query = FileSearchQuery::default();
        Self {
            root: query.root,
            pattern: query.pattern,
            limit: query.limit,
            hidden: query.hidden,
            parents: query.parents,
            ignore: query.ignore,
            git_ignore: query.git_ignore,
            git_global: query.git_global,
            git_exclude: query.git_exclude,
            follow_links: query.follow_links,
            max_depth: query.max_depth,
            excluded_relative_prefixes: query.excluded_relative_prefixes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchResponse {
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearchRequest {
    #[serde(serialize_with = "serialize_posix_path")]
    pub root: PathBuf,
    pub pattern: String,
    pub limit: usize,
    pub smart_case: bool,
    pub hidden: bool,
    pub parents: bool,
    pub ignore: bool,
    pub git_ignore: bool,
    pub git_global: bool,
    pub git_exclude: bool,
    pub follow_links: bool,
    pub max_depth: Option<usize>,
    pub max_file_bytes: u64,
    #[serde(serialize_with = "serialize_posix_paths")]
    pub excluded_relative_paths: Vec<PathBuf>,
    #[serde(serialize_with = "serialize_posix_paths")]
    pub custom_ignore_filenames: Vec<PathBuf>,
}

impl Default for TextSearchRequest {
    fn default() -> Self {
        let query = TextSearchQuery::default();
        Self {
            root: query.root,
            pattern: query.pattern,
            limit: query.limit,
            smart_case: query.smart_case,
            hidden: query.hidden,
            parents: query.parents,
            ignore: query.ignore,
            git_ignore: query.git_ignore,
            git_global: query.git_global,
            git_exclude: query.git_exclude,
            follow_links: query.follow_links,
            max_depth: query.max_depth,
            max_file_bytes: query.max_file_bytes,
            excluded_relative_paths: query.excluded_relative_paths,
            custom_ignore_filenames: query.custom_ignore_filenames,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearchMatchResponse {
    pub relative_path: PathBuf,
    pub line_number: usize,
    pub line_text: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearchResponse {
    pub root: PathBuf,
    pub matches: Vec<TextSearchMatchResponse>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProjectEnvironmentOrigin {
    NativeFlake,
    DirectoryShell,
    ProcessBaseline,
    Cli,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEnvironmentResponse {
    pub root: PathBuf,
    pub variables: BTreeMap<String, String>,
    pub origin: RemoteProjectEnvironmentOrigin,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteGitStatusKind {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatusEntryResponse {
    pub relative_path: PathBuf,
    pub original_relative_path: Option<PathBuf>,
    pub index_status: RemoteGitStatusKind,
    pub working_tree_status: RemoteGitStatusKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatusResponse {
    pub root: PathBuf,
    pub entries: Vec<GitStatusEntryResponse>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHeadResponse {
    pub root: PathBuf,
    pub head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRequest {
    pub program: String,
    pub args: Vec<String>,
    #[serde(serialize_with = "serialize_posix_path")]
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub clear_env: bool,
    #[serde(default)]
    pub inherit_project_environment: bool,
    pub max_output_bytes: Option<usize>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOutputResponse {
    pub status_code: Option<i32>,
    pub success: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_len: usize,
    pub stderr_len: usize,
    #[serde(default)]
    pub timed_out: bool,
}

#[derive(Debug)]
pub enum RemoteClientError {
    Io(io::Error),
    Json(serde_json::Error),
    Disconnected,
    TransportClosed {
        cause: String,
    },
    RequestDeadlineExceeded {
        method: String,
        kind: RemoteRequestDeadlineKind,
    },
    OutcomeUnknown {
        method: String,
        cause: String,
    },
    ResponseIncomplete {
        cause: String,
    },
    Protocol(String),
    Remote(RemoteError),
}

impl fmt::Display for RemoteClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "remote transport I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "remote protocol JSON failed: {error}"),
            Self::Disconnected => formatter.write_str("remote service disconnected"),
            Self::TransportClosed { cause } => {
                write!(formatter, "remote transport closed: {cause}")
            }
            Self::RequestDeadlineExceeded { method, kind } => {
                write!(formatter, "remote {method} {kind} deadline exceeded")
            }
            Self::OutcomeUnknown { method, cause } => write!(
                formatter,
                "remote {method} outcome is unknown after transport failure: {cause}"
            ),
            Self::ResponseIncomplete { cause } => write!(
                formatter,
                "remote response was incomplete when the transport closed: {cause}"
            ),
            Self::Protocol(message) => write!(formatter, "remote protocol error: {message}"),
            Self::Remote(error) => write!(formatter, "remote service error: {}", error.message),
        }
    }
}

impl Error for RemoteClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Disconnected
            | Self::TransportClosed { .. }
            | Self::RequestDeadlineExceeded { .. }
            | Self::OutcomeUnknown { .. }
            | Self::ResponseIncomplete { .. }
            | Self::Protocol(_)
            | Self::Remote(_) => None,
        }
    }
}

impl From<io::Error> for RemoteClientError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RemoteClientError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn remote_request_cancelled_error(method: &str) -> RemoteClientError {
    RemoteClientError::Remote(RemoteError {
        code: protocol_v5::RESET_CANCELLED.to_string(),
        message: format!("remote {method} request cancelled by caller"),
        diagnostic: None,
    })
}

pub struct RemoteWorkspaceV5Client<R, W> {
    io: protocol_v5::FramedIo<R, W>,
    session: protocol_v5::ProtocolSession,
    server_hello: protocol_v5::ServerHello,
}

impl<R: Read, W: Write> RemoteWorkspaceV5Client<R, W> {
    pub fn connect(
        mut io: protocol_v5::FramedIo<R, W>,
        client_hello: protocol_v5::ClientHello,
    ) -> std::result::Result<Self, RemoteClientError> {
        let handshake = protocol_v5::client_handshake(&mut io, client_hello)?;
        let session = protocol_v5::ProtocolSession::new(
            protocol_v5::StreamInitiator::Client,
            &handshake.settings,
        );
        Ok(Self {
            io,
            session,
            server_hello: handshake.server_hello,
        })
    }

    pub fn server_hello(&self) -> &protocol_v5::ServerHello {
        &self.server_hello
    }

    pub fn request(
        &mut self,
        request: RemoteRequest,
        body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let context = request.v5_request_context();
        self.request_with_context(request, body, context)
    }

    pub fn request_with_context(
        &mut self,
        request: RemoteRequest,
        body: Vec<u8>,
        context: RemoteRequestContext,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let (method, payload) = request
            .to_v5_method_payload()
            .map_err(v5_method_error_to_client_error)?;
        if let Some(kind) = context.expired_at(Instant::now()) {
            return Err(RemoteClientError::RequestDeadlineExceeded {
                method: method.to_string(),
                kind,
            });
        }
        let stream_id = self.session.open_request_with_owned_payload_and_body(
            method,
            request.v5_request_options_with_context(context),
            payload,
            request.v5_body_channel(),
            body,
        )?;
        self.drain_outbound()?;
        self.read_response(
            stream_id,
            method,
            request.v5_request_options().idempotency != protocol_v5::Idempotency::ReadOnly
                || matches!(request, RemoteRequest::Shutdown),
        )
    }

    pub fn shutdown(&mut self) -> std::result::Result<(), RemoteClientError> {
        let (response, _) = self.request(RemoteRequest::Shutdown, Vec::new())?;
        match response {
            RemoteResponse::Shutdown => Ok(()),
            other => Err(RemoteClientError::Protocol(format!(
                "unexpected shutdown response: {other:?}"
            ))),
        }
    }

    pub fn into_inner(self) -> (R, W) {
        self.io.into_inner()
    }

    fn drain_outbound(&mut self) -> std::result::Result<(), RemoteClientError> {
        while let Some(frame) = self.session.pop_next_frame()? {
            let stream_id = frame.stream_id;
            let frame_type = frame.frame_type;
            self.io.write_frame(frame)?;
            self.session
                .observe_frame_parts_written(stream_id, frame_type);
        }
        Ok(())
    }

    fn read_response(
        &mut self,
        stream_id: u64,
        request_method: &'static str,
        terminal_on_deadline: bool,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let mut method = None;
        let mut payload = Vec::new();
        let mut file_body = Vec::new();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut search_partials = V5SearchResponsePartials::default();
        let mut final_error = None;
        let mut received_bytes = 0_usize;

        loop {
            let frame = self
                .io
                .read_frame()?
                .ok_or(RemoteClientError::Disconnected)?;
            let event = self.session.receive_frame(frame)?;
            let data_credit = event.data_credit();
            let Some(stream_event) = event.stream_event else {
                self.drain_outbound()?;
                continue;
            };
            if stream_event.stream_id() != stream_id {
                if let Some((stream_id, credit_bytes)) = data_credit {
                    self.session.acknowledge_data(stream_id, credit_bytes)?;
                }
                self.drain_outbound()?;
                continue;
            }

            match stream_event {
                protocol_v5::StreamEvent::Headers {
                    role: protocol_v5::MessageRole::FinalResponse,
                    envelope,
                    ..
                } => {
                    search_partials.finish_current()?;
                    method = Some(envelope.method);
                }
                protocol_v5::StreamEvent::Headers {
                    role: protocol_v5::MessageRole::FinalError,
                    envelope,
                    ..
                } => {
                    search_partials.finish_current()?;
                    method = Some(envelope.method.clone());
                    final_error = Some(v5_final_error_from_envelope(envelope)?);
                }
                protocol_v5::StreamEvent::Headers {
                    role: protocol_v5::MessageRole::PartialResult,
                    envelope,
                    ..
                } => {
                    search_partials.begin_partial(envelope.method)?;
                }
                protocol_v5::StreamEvent::Data { channel, body, .. } => {
                    let Some(total) = received_bytes.checked_add(body.len()) else {
                        return Err(RemoteClientError::Protocol(
                            "v5 response decoded byte count overflowed".to_string(),
                        ));
                    };
                    if total > V5_MAX_ACCUMULATED_RESPONSE_BYTES {
                        self.session.reset_stream(
                            stream_id,
                            protocol_v5::RESET_RESOURCE_EXHAUSTED,
                            "client response decoded byte limit exceeded",
                        )?;
                        self.drain_outbound()?;
                        return Err(RemoteClientError::Protocol(format!(
                            "v5 response exceeds decoded byte limit of {V5_MAX_ACCUMULATED_RESPONSE_BYTES}"
                        )));
                    }
                    received_bytes = total;
                    match channel {
                        protocol_v5::DataChannel::Unspecified => payload.extend(body),
                        protocol_v5::DataChannel::SearchPayload => {
                            search_partials.push_search_payload(body);
                        }
                        protocol_v5::DataChannel::FileBody | protocol_v5::DataChannel::Stdin => {
                            file_body.extend(body)
                        }
                        protocol_v5::DataChannel::Stdout => stdout.extend(body),
                        protocol_v5::DataChannel::Stderr => stderr.extend(body),
                    }
                }
                protocol_v5::StreamEvent::EndStream { .. } => {
                    if let Some(error) = final_error {
                        if error.code == protocol_v5::RESET_DEADLINE_EXCEEDED {
                            if terminal_on_deadline
                                && (request_method == "session.shutdown" || method.is_none())
                            {
                                self.session.terminate();
                                return Err(RemoteClientError::OutcomeUnknown {
                                    method: request_method.to_string(),
                                    cause: RemoteClientError::Remote(error).to_string(),
                                });
                            }
                            return Err(RemoteClientError::RequestDeadlineExceeded {
                                method: request_method.to_string(),
                                kind: RemoteRequestDeadlineKind::Absolute,
                            });
                        }
                        return Err(RemoteClientError::Remote(error));
                    }
                    let method = method.ok_or_else(|| {
                        RemoteClientError::Protocol(format!(
                            "v5 stream {stream_id} ended without final response"
                        ))
                    })?;
                    let response =
                        if let Some(response) = search_partials.merge_final(&method, &payload)? {
                            response
                        } else {
                            RemoteResponse::from_v5_payload(&method, &payload)
                                .map_err(v5_method_error_to_client_error)?
                        };
                    let body = v5_client_body_for_response(&response, file_body, stdout, stderr);
                    return Ok((response, body));
                }
                protocol_v5::StreamEvent::ResetStream {
                    code, diagnostic, ..
                } => {
                    if code == protocol_v5::RESET_DEADLINE_EXCEEDED {
                        if terminal_on_deadline
                            && (request_method == "session.shutdown" || method.is_none())
                        {
                            self.session.terminate();
                            return Err(RemoteClientError::OutcomeUnknown {
                                method: request_method.to_string(),
                                cause: format!("v5 peer reset {request_method} after its deadline"),
                            });
                        }
                        return Err(RemoteClientError::RequestDeadlineExceeded {
                            method: request_method.to_string(),
                            kind: RemoteRequestDeadlineKind::Absolute,
                        });
                    }
                    return Err(RemoteClientError::Remote(RemoteError {
                        code,
                        message: "v5 stream reset".to_string(),
                        diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
                    }));
                }
                protocol_v5::StreamEvent::Headers { .. } => {}
            }
            if let Some((stream_id, credit_bytes)) = data_credit {
                self.session.acknowledge_data(stream_id, credit_bytes)?;
            }
            self.drain_outbound()?;
        }
    }
}

pub struct RemoteWorkspaceV5MultiplexedClient<R, W> {
    server_hello: protocol_v5::ServerHello,
    shared: Arc<RemoteWorkspaceV5Shared<W>>,
    _reader: std::marker::PhantomData<fn() -> R>,
}

struct RemoteWorkspaceV5Shared<W> {
    session: Mutex<protocol_v5::ProtocolSession>,
    writer_wake: mpsc::SyncSender<()>,
    heartbeat: Mutex<V5ClientHeartbeat>,
    heartbeat_wake: mpsc::SyncSender<()>,
    deadline_wake: mpsc::SyncSender<()>,
    transport_abort: Option<Arc<dyn V5TransportAbort>>,
    request_budget: V5ConnectionByteBudget,
    response_budget: V5ConnectionByteBudget,
    outbound_request_reservations: Mutex<HashMap<u64, V5ByteReservation>>,
    waiters: Mutex<HashMap<u64, V5PendingResponse>>,
    raw_waiters: Mutex<HashMap<u64, V5PendingRawResponse>>,
    pending_cancellations: Mutex<HashMap<u64, V5ClientCancellation>>,
    watch_batches: Mutex<HashMap<u64, V5WatchDelivery>>,
    watch_backlog: Mutex<HashMap<u64, VecDeque<protocol_v5::WatchBatch>>>,
    watch_stream_by_id: Mutex<HashMap<u64, u64>>,
    directory_cache: Mutex<HashMap<PathBuf, DirectoryListingResponse>>,
    closed: AtomicBool,
    _writer: std::marker::PhantomData<fn() -> W>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum V5ClientCancellationMode {
    Stream,
    Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct V5ClientCancellation {
    method: &'static str,
    mode: V5ClientCancellationMode,
}

#[derive(Clone)]
struct V5WatchDelivery {
    sender: mpsc::SyncSender<protocol_v5::WatchBatch>,
    overflowed: Arc<AtomicBool>,
    last_sequence: Arc<AtomicU64>,
}

struct RemoteWorkspaceV5Writer<W> {
    writer: W,
    limits: protocol_v5::FrameLimits,
    next_frame_sequence: u64,
}

#[derive(Debug)]
struct V5ClientHeartbeat {
    idle_ping_interval: Duration,
    ping_timeout: Duration,
    last_peer_activity: Instant,
    next_ping_id: u64,
    ping: Option<V5ClientPing>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum V5ClientPing {
    Queued {
        expected_pong_control: Vec<u8>,
        queued_at: Instant,
    },
    Outstanding {
        expected_pong_control: Vec<u8>,
        started_at: Instant,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum V5ClientHeartbeatAction {
    Wait(Duration),
    QueuePing(Vec<u8>),
    TimedOut(&'static str),
}

const V5_CLIENT_PING_WRITE_TIMEOUT: &str = "v5 client writer did not send idle PING before timeout";
const V5_CLIENT_PONG_TIMEOUT: &str = "v5 peer did not answer client idle PING before timeout";

fn v5_client_heartbeat_timeout(message: &'static str) -> RemoteClientError {
    RemoteClientError::Io(io::Error::new(io::ErrorKind::TimedOut, message))
}

impl V5ClientHeartbeat {
    fn new(settings: &protocol_v5::ConnectionSettings, now: Instant) -> Self {
        let min_unsolicited_ping_interval_ms = if settings.min_unsolicited_ping_interval_ms == 0 {
            protocol_v5::MIN_UNSOLICITED_PING_INTERVAL_MS
        } else {
            settings.min_unsolicited_ping_interval_ms
        }
        .max(protocol_v5::MIN_UNSOLICITED_PING_INTERVAL_MS);
        let idle_ping_interval_ms = if settings.idle_ping_interval_ms == 0 {
            protocol_v5::IDLE_PING_INTERVAL_MS
        } else {
            settings.idle_ping_interval_ms
        }
        .max(min_unsolicited_ping_interval_ms);
        let ping_timeout_ms = if settings.ping_timeout_ms == 0 {
            protocol_v5::PING_TIMEOUT_MS
        } else {
            settings.ping_timeout_ms
        };
        Self {
            idle_ping_interval: Duration::from_millis(u64::from(idle_ping_interval_ms)),
            ping_timeout: Duration::from_millis(u64::from(ping_timeout_ms)),
            last_peer_activity: now,
            next_ping_id: 0,
            ping: None,
        }
    }

    fn next_action(
        &mut self,
        now: Instant,
    ) -> std::result::Result<V5ClientHeartbeatAction, RemoteClientError> {
        if let Some(ping) = &self.ping {
            let (started_at, timeout_message) = match ping {
                V5ClientPing::Queued { queued_at, .. } => {
                    (*queued_at, V5_CLIENT_PING_WRITE_TIMEOUT)
                }
                V5ClientPing::Outstanding { started_at, .. } => {
                    (*started_at, V5_CLIENT_PONG_TIMEOUT)
                }
            };
            let elapsed = now.saturating_duration_since(started_at);
            return Ok(if elapsed >= self.ping_timeout {
                V5ClientHeartbeatAction::TimedOut(timeout_message)
            } else {
                V5ClientHeartbeatAction::Wait(self.ping_timeout - elapsed)
            });
        }

        let idle = now.saturating_duration_since(self.last_peer_activity);
        if idle < self.idle_ping_interval {
            return Ok(V5ClientHeartbeatAction::Wait(
                self.idle_ping_interval - idle,
            ));
        }

        self.next_ping_id = self.next_ping_id.checked_add(1).ok_or_else(|| {
            RemoteClientError::Protocol("v5 client heartbeat nonce exhausted".to_string())
        })?;
        let token = self.next_ping_id.to_be_bytes().to_vec();
        let expected_pong_control = protocol_v5::PingPayload {
            token: token.clone(),
        }
        .encode_to_vec();
        self.ping = Some(V5ClientPing::Queued {
            expected_pong_control,
            queued_at: now,
        });
        Ok(V5ClientHeartbeatAction::QueuePing(token))
    }

    fn peer_is_healthy_at(&self, now: Instant) -> bool {
        self.ping.is_none()
            && now.saturating_duration_since(self.last_peer_activity) < self.idle_ping_interval
    }

    fn mark_ping_started(
        &mut self,
        frame: &protocol_v5::Frame,
        now: Instant,
    ) -> std::result::Result<(), RemoteClientError> {
        match self.ping.take() {
            Some(V5ClientPing::Queued {
                expected_pong_control,
                queued_at,
            }) => {
                if expected_pong_control != frame.control {
                    self.ping = Some(V5ClientPing::Queued {
                        expected_pong_control,
                        queued_at,
                    });
                    return Err(RemoteClientError::Protocol(
                        "v5 writer selected an unexpected client heartbeat PING".to_string(),
                    ));
                }
                if now.saturating_duration_since(queued_at) >= self.ping_timeout {
                    self.ping = Some(V5ClientPing::Queued {
                        expected_pong_control,
                        queued_at,
                    });
                    return Err(v5_client_heartbeat_timeout(V5_CLIENT_PING_WRITE_TIMEOUT));
                }
                self.ping = Some(V5ClientPing::Outstanding {
                    expected_pong_control,
                    started_at: now,
                });
                Ok(())
            }
            Some(ping) => {
                self.ping = Some(ping);
                Err(RemoteClientError::Protocol(
                    "v5 writer selected an unexpected client heartbeat PING".to_string(),
                ))
            }
            None => Err(RemoteClientError::Protocol(
                "v5 writer selected a client heartbeat PING without queued state".to_string(),
            )),
        }
    }

    fn observe_inbound(
        &mut self,
        frame_type: protocol_v5::FrameType,
        pong_control: Option<Vec<u8>>,
        now: Instant,
    ) -> std::result::Result<Option<Duration>, RemoteClientError> {
        if frame_type != protocol_v5::FrameType::Pong {
            self.last_peer_activity = now;
            return Ok(None);
        }

        let pong_control = pong_control.ok_or_else(|| {
            RemoteClientError::Protocol("v5 PONG did not carry heartbeat control".to_string())
        })?;
        match self.ping.take() {
            Some(V5ClientPing::Outstanding {
                expected_pong_control,
                started_at,
            }) => {
                let rtt = now.saturating_duration_since(started_at);
                if rtt >= self.ping_timeout {
                    self.ping = Some(V5ClientPing::Outstanding {
                        expected_pong_control,
                        started_at,
                    });
                    return Err(v5_client_heartbeat_timeout(V5_CLIENT_PONG_TIMEOUT));
                }
                if expected_pong_control == pong_control {
                    self.last_peer_activity = now;
                    return Ok(Some(rtt));
                }
                self.ping = Some(V5ClientPing::Outstanding {
                    expected_pong_control,
                    started_at,
                });
                Err(RemoteClientError::Protocol(
                    "received v5 PONG with an unexpected heartbeat token".to_string(),
                ))
            }
            Some(ping @ V5ClientPing::Queued { .. }) => {
                self.ping = Some(ping);
                Err(RemoteClientError::Protocol(
                    "received v5 PONG before the client heartbeat PING was written".to_string(),
                ))
            }
            None => Err(RemoteClientError::Protocol(
                "received unsolicited v5 PONG".to_string(),
            )),
        }
    }
}

fn v5_client_pong_control(frame: &protocol_v5::Frame) -> Option<Vec<u8>> {
    (frame.frame_type == protocol_v5::FrameType::Pong).then(|| frame.control.clone())
}

type V5ResponseDelivery =
    std::result::Result<V5Budgeted<(RemoteResponse, Vec<u8>)>, RemoteClientError>;
type V5RawResponseDelivery = std::result::Result<V5Budgeted<Vec<u8>>, RemoteClientError>;

#[must_use = "dropping a live v5 request handle cancels its stream"]
pub struct RemoteWorkspaceV5RequestHandle<W> {
    shared: Arc<RemoteWorkspaceV5Shared<W>>,
    stream_id: u64,
    request: RemoteRequest,
    receiver: mpsc::Receiver<V5ResponseDelivery>,
    cancellation: RemoteRequestCancellation,
    finished: bool,
}

impl<W> RemoteWorkspaceV5RequestHandle<W> {
    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn wait(mut self) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let delivery = match self.receiver.recv() {
            Ok(delivery) => {
                self.finished = true;
                delivery
            }
            Err(_) => return Err(RemoteClientError::Disconnected),
        };
        let (response, body) = delivery?.into_inner();
        let response = apply_v5_directory_cache(&self.shared, &self.request, response)?;
        Ok((response, body))
    }
}

impl<W> Drop for RemoteWorkspaceV5RequestHandle<W> {
    fn drop(&mut self) {
        if !self.finished {
            self.cancellation.cancel();
        }
    }
}

struct V5NormalizedDeadlineResult<T> {
    result: std::result::Result<T, RemoteClientError>,
    peer_deadline: bool,
    terminal: bool,
}

#[derive(Debug, Clone, Copy)]
struct V5RequestDeadline {
    context: RemoteRequestContext,
    last_progress_at: Instant,
}

impl V5RequestDeadline {
    fn new(context: RemoteRequestContext, now: Instant) -> Self {
        Self {
            context,
            last_progress_at: now,
        }
    }

    fn next_expiry(self) -> Option<(Instant, RemoteRequestDeadlineKind)> {
        let absolute = self
            .context
            .absolute_deadline
            .map(|deadline| (deadline, RemoteRequestDeadlineKind::Absolute));
        let inactivity = self.context.inactivity_timeout.and_then(|timeout| {
            self.last_progress_at
                .checked_add(timeout)
                .map(|deadline| (deadline, RemoteRequestDeadlineKind::Inactivity))
        });
        match (absolute, inactivity) {
            (Some(absolute), Some(inactivity)) => {
                if absolute.0 <= inactivity.0 {
                    Some(absolute)
                } else {
                    Some(inactivity)
                }
            }
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        }
    }

    fn expired_at(self, now: Instant) -> Option<RemoteRequestDeadlineKind> {
        self.next_expiry()
            .filter(|(deadline, _)| now >= *deadline)
            .map(|(_, kind)| kind)
    }

    fn observe_progress(&mut self, now: Instant) {
        if now > self.last_progress_at {
            self.last_progress_at = now;
        }
    }
}

struct V5PendingResponse {
    sender: mpsc::Sender<V5ResponseDelivery>,
    accumulator: V5ResponseAccumulator,
    response_reservation: V5ByteReservation,
    method: &'static str,
    idempotency: protocol_v5::Idempotency,
    terminal_on_deadline: bool,
    deadline: V5RequestDeadline,
}

struct V5PendingRawResponse {
    sender: mpsc::Sender<V5RawResponseDelivery>,
    accumulator: V5RawResponseAccumulator,
    response_reservation: V5ByteReservation,
    method: &'static str,
    deadline: V5RequestDeadline,
}

impl V5PendingResponse {
    fn deadline_is_connection_terminal(&self) -> bool {
        self.terminal_on_deadline
            && (self.method == "session.shutdown" || !self.accumulator.final_message_seen())
    }

    fn failure_error(&self, error: RemoteClientError) -> RemoteClientError {
        if self.accumulator.final_message_seen() {
            disconnect_after_final_response_error(error)
        } else {
            let error = transport_closed_before_final_error(error);
            if self.idempotency != protocol_v5::Idempotency::ReadOnly
                && remote_client_error_allows_reconnect_retry(&error)
            {
                RemoteClientError::OutcomeUnknown {
                    method: self.method.to_string(),
                    cause: error.to_string(),
                }
            } else {
                error
            }
        }
    }
}

impl V5PendingRawResponse {
    fn failure_error(&self, error: RemoteClientError) -> RemoteClientError {
        if self.accumulator.final_message_seen() {
            disconnect_after_final_response_error(error)
        } else {
            transport_closed_before_final_error(error)
        }
    }
}

fn reserve_v5_client_request_bytes(
    budget: &V5ConnectionByteBudget,
    method: &str,
    payload_bytes: usize,
    body_bytes: usize,
) -> std::result::Result<V5ByteReservation, RemoteClientError> {
    if payload_bytes > V5_MAX_REQUEST_PAYLOAD_BYTES {
        return Err(RemoteClientError::Protocol(format!(
            "v5 {method} request payload exceeds decoded byte limit {V5_MAX_REQUEST_PAYLOAD_BYTES}"
        )));
    }
    if body_bytes > V5_MAX_REQUEST_BODY_BYTES {
        return Err(RemoteClientError::Protocol(format!(
            "v5 {method} request body exceeds decoded byte limit {V5_MAX_REQUEST_BODY_BYTES}"
        )));
    }
    let retained_bytes = payload_bytes.checked_add(body_bytes).ok_or_else(|| {
        RemoteClientError::Protocol(format!("v5 {method} request decoded byte count overflowed"))
    })?;
    let mut reservation = budget.reservation();
    reservation.try_grow(retained_bytes).map_err(|error| {
        RemoteClientError::Protocol(format!(
            "v5 {method} request exceeds connection retained-byte budget: {error}"
        ))
    })?;
    Ok(reservation)
}

#[derive(Default)]
struct V5ResponseAccumulator {
    method: Option<String>,
    payload: Vec<u8>,
    file_body: Vec<u8>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    search_partials: V5SearchResponsePartials,
    final_error: Option<RemoteError>,
    received_bytes: usize,
}

#[derive(Default)]
struct V5SearchResponsePartials {
    current_method: Option<String>,
    current_payload: Vec<u8>,
    file_root: Option<PathBuf>,
    file_files: Vec<PathBuf>,
    file_truncated: bool,
    text_root: Option<PathBuf>,
    text_matches: Vec<TextSearchMatchResponse>,
    text_truncated: bool,
}

impl V5SearchResponsePartials {
    fn begin_partial(&mut self, method: String) -> std::result::Result<(), RemoteClientError> {
        self.finish_current()?;
        if matches!(method.as_str(), "search.files" | "search.text") {
            self.current_method = Some(method);
            self.current_payload.clear();
        }
        Ok(())
    }

    fn push_search_payload(&mut self, body: Vec<u8>) {
        if self.current_method.is_some() {
            self.current_payload.extend(body);
        }
    }

    fn finish_current(&mut self) -> std::result::Result<(), RemoteClientError> {
        let Some(method) = self.current_method.take() else {
            return Ok(());
        };
        let payload = std::mem::take(&mut self.current_payload);
        let response = RemoteResponse::from_v5_payload(&method, &payload)
            .map_err(v5_method_error_to_client_error)?;
        match response {
            RemoteResponse::FileSearch(partial) => {
                self.file_root.get_or_insert(partial.root);
                self.file_files.extend(partial.files);
                self.file_truncated |= partial.truncated;
            }
            RemoteResponse::TextSearch(partial) => {
                self.text_root.get_or_insert(partial.root);
                self.text_matches.extend(partial.matches);
                self.text_truncated |= partial.truncated;
            }
            other => {
                return Err(RemoteClientError::Protocol(format!(
                    "unexpected v5 search partial response: {other:?}"
                )));
            }
        }
        Ok(())
    }

    fn merge_final(
        &mut self,
        method: &str,
        payload: &[u8],
    ) -> std::result::Result<Option<RemoteResponse>, RemoteClientError> {
        self.finish_current()?;
        match method {
            "search.files" if self.file_root.is_some() || !self.file_files.is_empty() => {
                let mut final_response = match RemoteResponse::from_v5_payload(method, payload)
                    .map_err(v5_method_error_to_client_error)?
                {
                    RemoteResponse::FileSearch(response) => response,
                    other => {
                        return Err(RemoteClientError::Protocol(format!(
                            "unexpected v5 file search final response: {other:?}"
                        )));
                    }
                };
                let mut files = std::mem::take(&mut self.file_files);
                files.append(&mut final_response.files);
                let root = self.file_root.take().unwrap_or(final_response.root);
                Ok(Some(RemoteResponse::FileSearch(FileSearchResponse {
                    root,
                    files,
                    truncated: self.file_truncated || final_response.truncated,
                })))
            }
            "search.text" if self.text_root.is_some() || !self.text_matches.is_empty() => {
                let mut final_response = match RemoteResponse::from_v5_payload(method, payload)
                    .map_err(v5_method_error_to_client_error)?
                {
                    RemoteResponse::TextSearch(response) => response,
                    other => {
                        return Err(RemoteClientError::Protocol(format!(
                            "unexpected v5 text search final response: {other:?}"
                        )));
                    }
                };
                let mut matches = std::mem::take(&mut self.text_matches);
                matches.append(&mut final_response.matches);
                let root = self.text_root.take().unwrap_or(final_response.root);
                Ok(Some(RemoteResponse::TextSearch(TextSearchResponse {
                    root,
                    matches,
                    truncated: self.text_truncated || final_response.truncated,
                })))
            }
            _ => Ok(None),
        }
    }
}

#[derive(Default)]
struct V5RawResponseAccumulator {
    payload: Vec<u8>,
    final_seen: bool,
    final_error: Option<RemoteError>,
    received_bytes: usize,
}

pub struct RemoteWorkspaceV5Watch {
    pub watch_id: u64,
    pub event_stream_id: u64,
    receiver: mpsc::Receiver<protocol_v5::WatchBatch>,
    overflowed: Arc<AtomicBool>,
    last_sequence: Arc<AtomicU64>,
}

impl RemoteWorkspaceV5Watch {
    pub fn recv(&self) -> std::result::Result<protocol_v5::WatchBatch, mpsc::RecvError> {
        if let Some(batch) = self.take_overflow_batch() {
            return Ok(batch);
        }
        let batch = self.receiver.recv()?;
        Ok(self.take_overflow_batch().unwrap_or(batch))
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<protocol_v5::WatchBatch, mpsc::RecvTimeoutError> {
        if let Some(batch) = self.take_overflow_batch() {
            return Ok(batch);
        }
        let batch = self.receiver.recv_timeout(timeout)?;
        Ok(self.take_overflow_batch().unwrap_or(batch))
    }

    pub fn try_recv(&self) -> std::result::Result<protocol_v5::WatchBatch, mpsc::TryRecvError> {
        if let Some(batch) = self.take_overflow_batch() {
            return Ok(batch);
        }
        let batch = self.receiver.try_recv()?;
        Ok(self.take_overflow_batch().unwrap_or(batch))
    }

    fn take_overflow_batch(&self) -> Option<protocol_v5::WatchBatch> {
        if !self.overflowed.swap(false, Ordering::AcqRel) {
            return None;
        }
        while self.receiver.try_recv().is_ok() {}
        Some(protocol_v5::WatchBatch {
            watch_id: self.watch_id,
            sequence: self.last_sequence.load(Ordering::Acquire),
            directory_generations: Vec::new(),
            events: Vec::new(),
            overflow: true,
            resync_required: true,
        })
    }
}

impl<R, W> RemoteWorkspaceV5MultiplexedClient<R, W>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn connect(
        io: protocol_v5::FramedIo<R, W>,
        client_hello: protocol_v5::ClientHello,
    ) -> std::result::Result<Self, RemoteClientError> {
        Self::connect_with_transport_abort(io, client_hello, None)
    }

    fn connect_with_transport_abort(
        mut io: protocol_v5::FramedIo<R, W>,
        client_hello: protocol_v5::ClientHello,
        transport_abort: Option<Arc<dyn V5TransportAbort>>,
    ) -> std::result::Result<Self, RemoteClientError> {
        let handshake = protocol_v5::client_handshake(&mut io, client_hello)?;
        let session = protocol_v5::ProtocolSession::new(
            protocol_v5::StreamInitiator::Client,
            &handshake.settings,
        );
        let parts = io.into_parts();
        let limits = parts.limits;
        let inbound_frame_sequence = parts.inbound_frame_sequence;
        let writer = RemoteWorkspaceV5Writer {
            writer: parts.writer,
            limits,
            next_frame_sequence: parts.next_frame_sequence,
        };
        let (writer_wake, writer_wakes) = mpsc::sync_channel(1);
        let (heartbeat_wake, heartbeat_wakes) = mpsc::sync_channel(1);
        let (deadline_wake, deadline_wakes) = mpsc::sync_channel(1);
        let shared = Arc::new(RemoteWorkspaceV5Shared {
            session: Mutex::new(session),
            writer_wake,
            heartbeat: Mutex::new(V5ClientHeartbeat::new(&handshake.settings, Instant::now())),
            heartbeat_wake,
            deadline_wake,
            transport_abort,
            request_budget: V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET),
            response_budget: V5ConnectionByteBudget::new(V5_RESPONSE_CONNECTION_BYTE_BUDGET),
            outbound_request_reservations: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            raw_waiters: Mutex::new(HashMap::new()),
            pending_cancellations: Mutex::new(HashMap::new()),
            watch_batches: Mutex::new(HashMap::new()),
            watch_backlog: Mutex::new(HashMap::new()),
            watch_stream_by_id: Mutex::new(HashMap::new()),
            directory_cache: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            _writer: std::marker::PhantomData,
        });

        let writer_shared = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("nucleotide-v5-client-writer".to_string())
            .spawn(move || run_v5_client_writer(writer, writer_wakes, writer_shared))
            .map_err(RemoteClientError::Io)?;

        let heartbeat_shared = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("nucleotide-v5-client-heartbeat".to_string())
            .spawn(move || run_v5_client_heartbeat(heartbeat_wakes, heartbeat_shared))
            .map_err(RemoteClientError::Io)?;

        let deadline_shared = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("nucleotide-v5-client-deadlines".to_string())
            .spawn(move || run_v5_client_deadlines(deadline_wakes, deadline_shared))
            .map_err(RemoteClientError::Io)?;

        let reader_shared = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("nucleotide-v5-client-reader".to_string())
            .spawn(move || {
                run_v5_client_reader(parts.reader, limits, inbound_frame_sequence, reader_shared)
            })
            .map_err(RemoteClientError::Io)?;

        Ok(Self {
            server_hello: handshake.server_hello,
            shared,
            _reader: std::marker::PhantomData,
        })
    }

    pub fn server_hello(&self) -> &protocol_v5::ServerHello {
        &self.server_hello
    }
}

fn connect_child_process_v5_client(
    io: protocol_v5::FramedIo<ChildStdout, ChildProcessV5Writer>,
    control: Arc<ChildProcessV5Control>,
    client_hello: protocol_v5::ClientHello,
) -> std::result::Result<RemoteWorkspaceV5ChildClient, RemoteClientError> {
    connect_child_process_v5_client_with_timeout(
        io,
        control,
        client_hello,
        V5_CHILD_HANDSHAKE_TIMEOUT,
    )
}

fn connect_child_process_v5_client_with_timeout(
    io: protocol_v5::FramedIo<ChildStdout, ChildProcessV5Writer>,
    control: Arc<ChildProcessV5Control>,
    client_hello: protocol_v5::ClientHello,
    timeout: Duration,
) -> std::result::Result<RemoteWorkspaceV5ChildClient, RemoteClientError> {
    let (watchdog_cancel, watchdog_receiver) = mpsc::channel();
    let watchdog_control = Arc::clone(&control);
    let watchdog = std::thread::Builder::new()
        .name("nucleotide-v5-handshake-watchdog".to_string())
        .spawn(move || {
            if matches!(
                watchdog_receiver.recv_timeout(timeout),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                tracing::warn!(
                    child_id = watchdog_control.child_id(),
                    timeout_ms = timeout.as_millis() as u64,
                    "Terminating remote service after v5 handshake timeout"
                );
                watchdog_control.abort();
            }
        })
        .map_err(RemoteClientError::Io)?;

    let abort: Arc<dyn V5TransportAbort> = control;
    let result = RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
        io,
        client_hello,
        Some(abort),
    );
    let _ = watchdog_cancel.send(());
    let _ = watchdog.join();
    result
}

impl<R, W> RemoteWorkspaceProtocolClient for RemoteWorkspaceV5MultiplexedClient<R, W>
where
    R: Send + 'static,
    W: Write + Send + 'static,
{
    fn request(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let context = request.v5_request_context();
        self.request_with_context(request, body, context)
    }

    fn request_with_context(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
        context: RemoteRequestContext,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        self.start_request_with_context(request, body, context)?
            .wait()
    }

    fn request_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        self.start_request_with_context_and_cancellation(
            request,
            body,
            context,
            cancellation.clone(),
        )?
        .wait()
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        let (response, _) = self.request(RemoteRequest::Shutdown, Vec::new())?;
        match response {
            RemoteResponse::Shutdown => Ok(()),
            other => Err(RemoteClientError::Protocol(format!(
                "unexpected shutdown response: {other:?}"
            ))),
        }
    }

    fn close(&self) {
        fail_all_v5_waiters(&self.shared, || RemoteClientError::Disconnected);
    }

    fn start_watch(
        &self,
        request: WorkspaceWatchRequest,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        let context = RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::bounded(
            V5_REQUEST_CONTROL_DEADLINE,
            V5_REQUEST_CONTROL_INACTIVITY,
        ));
        self.start_watch_with_context(request, context)
    }

    fn start_watch_with_context(
        &self,
        request: WorkspaceWatchRequest,
        context: RemoteRequestContext,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        self.start_watch_with_context_and_cancellation(
            request,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn start_watch_with_context_and_cancellation(
        &self,
        request: WorkspaceWatchRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        cancellation.check_cancelled("watch.start")?;
        if !self
            .server_hello
            .capabilities
            .iter()
            .any(|capability| capability == "watch")
        {
            return Ok(None);
        }
        let mut v5_request =
            protocol_v5::WatchStart::expanded_dirs(request.roots.iter().map(posix_path_string));
        v5_request.debounce_ms = request.debounce_ms;
        v5_request.max_events_per_batch = request.max_events_per_batch;
        let workspace_root = PathBuf::from(&self.server_hello.workspace_root);
        let watch =
            self.start_v5_watch_with_context_and_cancellation(v5_request, context, cancellation)?;
        Ok(Some(workspace_watch_from_v5(watch, workspace_root)))
    }

    fn update_watch(
        &self,
        watch_id: u64,
        add_roots: Vec<PathBuf>,
        remove_roots: Vec<PathBuf>,
    ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
        self.update_watch_with_cancellation(
            watch_id,
            add_roots,
            remove_roots,
            &RemoteRequestCancellation::new(),
        )
    }

    fn update_watch_with_cancellation(
        &self,
        watch_id: u64,
        add_roots: Vec<PathBuf>,
        remove_roots: Vec<PathBuf>,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
        let response = self.update_v5_watch_with_cancellation(
            protocol_v5::WatchUpdate {
                watch_id,
                add_roots: add_roots.iter().map(posix_path_string).collect(),
                remove_roots: remove_roots.iter().map(posix_path_string).collect(),
            },
            cancellation,
        )?;
        let workspace_root = PathBuf::from(&self.server_hello.workspace_root);
        Ok(Some(workspace_watch_update_from_v5(
            response,
            &workspace_root,
        )))
    }

    fn stop_watch(&self, watch_id: u64) -> std::result::Result<(), RemoteClientError> {
        self.stop_watch_with_cancellation(watch_id, &RemoteRequestCancellation::new())
    }

    fn stop_watch_with_cancellation(
        &self,
        watch_id: u64,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        self.stop_v5_watch_with_cancellation(watch_id, cancellation)
    }
}

impl<R, W> RemoteWorkspaceV5MultiplexedClient<R, W>
where
    W: Write + 'static,
{
    pub fn start_request(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
    ) -> std::result::Result<RemoteWorkspaceV5RequestHandle<W>, RemoteClientError> {
        let context = request.v5_request_context();
        self.start_request_with_context(request, body, context)
    }

    pub fn start_request_with_context(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
        context: RemoteRequestContext,
    ) -> std::result::Result<RemoteWorkspaceV5RequestHandle<W>, RemoteClientError> {
        self.start_request_with_context_and_cancellation(
            request,
            body,
            context,
            RemoteRequestCancellation::new(),
        )
    }

    fn start_request_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: RemoteRequestCancellation,
    ) -> std::result::Result<RemoteWorkspaceV5RequestHandle<W>, RemoteClientError> {
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }

        let (method, payload) = self.v5_method_payload_with_directory_cache(&request)?;
        cancellation.check_cancelled(method)?;
        if let Some(kind) = context.expired_at(Instant::now()) {
            return Err(RemoteClientError::RequestDeadlineExceeded {
                method: method.to_string(),
                kind,
            });
        }
        let mut options = request.v5_request_options_with_context(context);
        if request.v5_prefers_zstd_compression()
            && self
                .server_hello
                .capabilities
                .iter()
                .any(|capability| capability == "compression_zstd")
        {
            options.content_encoding = protocol_v5::ContentEncoding::Zstd;
        }
        let idempotency = options.idempotency;
        let body_channel = request.v5_body_channel();
        let terminal_on_deadline = idempotency != protocol_v5::Idempotency::ReadOnly
            || matches!(&request, RemoteRequest::Shutdown);
        let request_reservation = reserve_v5_client_request_bytes(
            &self.shared.request_budget,
            method,
            payload.len(),
            body.len(),
        )?;
        let response_reservation = self.shared.response_budget.reservation();
        let (sender, receiver) = mpsc::channel();
        let deadline = V5RequestDeadline::new(context, Instant::now());

        let stream_id = {
            let mut session = self.shared.session.lock().map_err(v5_client_lock_error)?;
            if self.shared.closed.load(Ordering::Acquire) {
                return Err(RemoteClientError::Disconnected);
            }
            cancellation.check_cancelled(method)?;
            if let Some(kind) = context.expired_at(Instant::now()) {
                return Err(RemoteClientError::RequestDeadlineExceeded {
                    method: method.to_string(),
                    kind,
                });
            }
            let stream_id = session.open_request_with_owned_payload_and_body(
                method,
                options,
                payload,
                body_channel,
                body,
            )?;
            let pending = V5PendingResponse {
                sender,
                accumulator: V5ResponseAccumulator::default(),
                response_reservation,
                method,
                idempotency,
                terminal_on_deadline,
                deadline,
            };
            self.shared
                .outbound_request_reservations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(stream_id, request_reservation);
            let mut waiters = match self.shared.waiters.lock() {
                Ok(waiters) => waiters,
                Err(error) => {
                    let error = v5_client_lock_error(error);
                    let reset_result = session.reset_stream(
                        stream_id,
                        protocol_v5::RESET_CANCELLED,
                        "client could not register response waiter",
                    );
                    release_v5_outbound_request_reservation(&self.shared, stream_id);
                    reset_result?;
                    return Err(error);
                }
            };
            waiters.insert(stream_id, pending);
            stream_id
        };

        register_v5_client_cancellation(
            &cancellation,
            &self.shared,
            stream_id,
            V5ClientCancellation {
                method,
                mode: V5ClientCancellationMode::Stream,
            },
        );
        signal_v5_client_deadlines(&self.shared);
        let handle = RemoteWorkspaceV5RequestHandle {
            shared: Arc::clone(&self.shared),
            stream_id,
            request,
            receiver,
            cancellation,
            finished: false,
        };

        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }
        self.wake_outbound()?;
        Ok(handle)
    }

    fn v5_method_payload_with_directory_cache(
        &self,
        request: &RemoteRequest,
    ) -> std::result::Result<(&'static str, Vec<u8>), RemoteClientError> {
        match request {
            RemoteRequest::ListDir { path } => {
                let (known_generation, known_fingerprint) = self.v5_known_directory_state(path)?;
                encode_v5_json_payload(
                    "fs.list_dir",
                    &V5DirectoryListPayload {
                        path: path.clone(),
                        known_generation,
                        known_fingerprint,
                    },
                )
            }
            RemoteRequest::ListDirs { paths } => {
                let entries = paths
                    .iter()
                    .map(|path| {
                        let (known_generation, known_fingerprint) =
                            self.v5_known_directory_state(path)?;
                        Ok(V5DirectoryListEntryPayload {
                            path: path.clone(),
                            known_generation,
                            known_fingerprint,
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, RemoteClientError>>()?;
                encode_v5_json_payload(
                    "fs.list_dirs",
                    &V5DirectoryListDirsPayload {
                        paths: paths.clone(),
                        entries,
                    },
                )
            }
            _ => request
                .to_v5_method_payload()
                .map_err(v5_method_error_to_client_error),
        }
    }

    fn v5_known_directory_state(
        &self,
        path: &Path,
    ) -> std::result::Result<(Option<u64>, Option<u64>), RemoteClientError> {
        let cache = self
            .shared
            .directory_cache
            .lock()
            .map_err(v5_client_lock_error)?;
        let Some(listing) = cache.get(path) else {
            return Ok((None, None));
        };
        Ok((listing.generation, listing.fingerprint))
    }

    pub fn start_v5_watch(
        &self,
        request: protocol_v5::WatchStart,
    ) -> std::result::Result<RemoteWorkspaceV5Watch, RemoteClientError> {
        let context = v5_watch_control_request_context();
        self.start_v5_watch_with_context(request, context)
    }

    fn start_v5_watch_with_context(
        &self,
        request: protocol_v5::WatchStart,
        context: RemoteRequestContext,
    ) -> std::result::Result<RemoteWorkspaceV5Watch, RemoteClientError> {
        self.start_v5_watch_with_context_and_cancellation(
            request,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn start_v5_watch_with_context_and_cancellation(
        &self,
        request: protocol_v5::WatchStart,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteWorkspaceV5Watch, RemoteClientError> {
        let payload = self.request_v5_raw_with_cancellation(
            "watch.start",
            request.encode_to_vec(),
            context,
            cancellation,
        )?;
        cancellation.check_cancelled("watch.start")?;
        let response =
            protocol_v5::WatchStartResponse::decode(payload.as_slice()).map_err(|error| {
                RemoteClientError::Protocol(format!(
                    "invalid v5 watch.start response payload: {error}"
                ))
            })?;
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(RemoteClientError::Disconnected);
        }
        let (sender, receiver) = mpsc::sync_channel(V5_WATCH_DELIVERY_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let last_sequence = Arc::new(AtomicU64::new(0));
        let delivery = V5WatchDelivery {
            sender: sender.clone(),
            overflowed: Arc::clone(&overflowed),
            last_sequence: Arc::clone(&last_sequence),
        };
        {
            self.shared
                .watch_batches
                .lock()
                .map_err(v5_client_lock_error)?
                .insert(response.event_stream_id, delivery);
            self.shared
                .watch_stream_by_id
                .lock()
                .map_err(v5_client_lock_error)?
                .insert(response.watch_id, response.event_stream_id);
        }
        if let Some(backlog) = self
            .shared
            .watch_backlog
            .lock()
            .map_err(v5_client_lock_error)?
            .remove(&response.event_stream_id)
        {
            for batch in backlog {
                last_sequence.store(batch.sequence, Ordering::Release);
                match sender.try_send(batch) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(_)) => {
                        overflowed.store(true, Ordering::Release);
                        break;
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
            }
        }
        if self.shared.closed.load(Ordering::Acquire) {
            self.remove_watch_sender(response.watch_id)?;
            return Err(RemoteClientError::Disconnected);
        }
        if let Err(error) = cancellation.check_cancelled("watch.start") {
            self.remove_watch_sender(response.watch_id)?;
            return Err(error);
        }
        Ok(RemoteWorkspaceV5Watch {
            watch_id: response.watch_id,
            event_stream_id: response.event_stream_id,
            receiver,
            overflowed,
            last_sequence,
        })
    }

    pub fn update_v5_watch(
        &self,
        request: protocol_v5::WatchUpdate,
    ) -> std::result::Result<protocol_v5::WatchUpdateResponse, RemoteClientError> {
        self.update_v5_watch_with_cancellation(request, &RemoteRequestCancellation::new())
    }

    fn update_v5_watch_with_cancellation(
        &self,
        request: protocol_v5::WatchUpdate,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<protocol_v5::WatchUpdateResponse, RemoteClientError> {
        let payload = self.request_v5_raw_with_cancellation(
            "watch.update",
            request.encode_to_vec(),
            v5_watch_control_request_context(),
            cancellation,
        )?;
        protocol_v5::WatchUpdateResponse::decode(payload.as_slice()).map_err(|error| {
            RemoteClientError::Protocol(format!(
                "invalid v5 watch.update response payload: {error}"
            ))
        })
    }

    pub fn resync_v5_watch(
        &self,
        request: protocol_v5::WatchResync,
    ) -> std::result::Result<protocol_v5::WatchResyncResponse, RemoteClientError> {
        let payload = self.request_v5_raw(
            "watch.resync",
            request.encode_to_vec(),
            v5_watch_control_request_context(),
        )?;
        protocol_v5::WatchResyncResponse::decode(payload.as_slice()).map_err(|error| {
            RemoteClientError::Protocol(format!(
                "invalid v5 watch.resync response payload: {error}"
            ))
        })
    }

    pub fn stop_v5_watch(&self, watch_id: u64) -> std::result::Result<(), RemoteClientError> {
        self.stop_v5_watch_with_cancellation(watch_id, &RemoteRequestCancellation::new())
    }

    fn stop_v5_watch_with_cancellation(
        &self,
        watch_id: u64,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        let _payload = self.request_v5_raw_with_cancellation(
            "watch.stop",
            protocol_v5::WatchStop { watch_id }.encode_to_vec(),
            v5_watch_control_request_context(),
            cancellation,
        )?;
        self.remove_watch_sender(watch_id)?;
        Ok(())
    }

    fn remove_watch_sender(&self, watch_id: u64) -> std::result::Result<(), RemoteClientError> {
        let event_stream_id = self
            .shared
            .watch_stream_by_id
            .lock()
            .map_err(v5_client_lock_error)?
            .remove(&watch_id);
        if let Some(event_stream_id) = event_stream_id {
            self.shared
                .watch_batches
                .lock()
                .map_err(v5_client_lock_error)?
                .remove(&event_stream_id);
            self.shared
                .watch_backlog
                .lock()
                .map_err(v5_client_lock_error)?
                .remove(&event_stream_id);
        }
        Ok(())
    }

    fn request_v5_raw(
        &self,
        method: &'static str,
        payload: Vec<u8>,
        context: RemoteRequestContext,
    ) -> std::result::Result<Vec<u8>, RemoteClientError> {
        self.request_v5_raw_with_cancellation(
            method,
            payload,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn request_v5_raw_with_cancellation(
        &self,
        method: &'static str,
        payload: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Vec<u8>, RemoteClientError> {
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }

        cancellation.check_cancelled(method)?;
        if let Some(kind) = context.expired_at(Instant::now()) {
            return Err(RemoteClientError::RequestDeadlineExceeded {
                method: method.to_string(),
                kind,
            });
        }

        let request_reservation =
            reserve_v5_client_request_bytes(&self.shared.request_budget, method, payload.len(), 0)?;
        let response_reservation = self.shared.response_budget.reservation();
        let (sender, receiver) = mpsc::channel();
        let stream_id = {
            let mut session = self.shared.session.lock().map_err(v5_client_lock_error)?;
            if self.shared.closed.load(Ordering::Acquire) {
                return Err(RemoteClientError::Disconnected);
            }
            cancellation.check_cancelled(method)?;
            if let Some(kind) = context.expired_at(Instant::now()) {
                return Err(RemoteClientError::RequestDeadlineExceeded {
                    method: method.to_string(),
                    kind,
                });
            }
            let stream_id = session.open_request_with_owned_payload_and_body(
                method,
                protocol_v5::RequestOptions {
                    priority: protocol_v5::Priority::VisibleFileTree,
                    deadline_unix_ms: context.deadline_unix_ms,
                    ..protocol_v5::RequestOptions::default()
                },
                payload,
                protocol_v5::DataChannel::Unspecified,
                Vec::new(),
            )?;
            let pending = V5PendingRawResponse {
                sender,
                accumulator: V5RawResponseAccumulator::default(),
                response_reservation,
                method,
                deadline: V5RequestDeadline::new(context, Instant::now()),
            };
            let mut outbound_request_reservations = self
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            outbound_request_reservations.insert(stream_id, request_reservation);
            drop(outbound_request_reservations);
            let mut waiters = match self.shared.raw_waiters.lock() {
                Ok(waiters) => waiters,
                Err(error) => {
                    let error = v5_client_lock_error(error);
                    let reset_result = session.reset_stream(
                        stream_id,
                        protocol_v5::RESET_CANCELLED,
                        "client could not register raw response waiter",
                    );
                    release_v5_outbound_request_reservation(&self.shared, stream_id);
                    reset_result?;
                    return Err(error);
                }
            };
            waiters.insert(stream_id, pending);
            stream_id
        };
        register_v5_client_cancellation(
            cancellation,
            &self.shared,
            stream_id,
            V5ClientCancellation {
                method,
                mode: V5ClientCancellationMode::Connection,
            },
        );
        signal_v5_client_deadlines(&self.shared);

        if self.shared.closed.load(Ordering::SeqCst) {
            let cancellation_error = cancellation.check_cancelled(method).err();
            self.shared
                .raw_waiters
                .lock()
                .map_err(v5_client_lock_error)?
                .remove(&stream_id);
            return Err(cancellation_error.unwrap_or(RemoteClientError::Disconnected));
        }

        if let Err(error) = self.wake_outbound() {
            return receiver
                .recv()
                .unwrap_or(Err(error))
                .map(V5Budgeted::into_inner);
        }

        let result = receiver
            .recv()
            .map_err(|_| RemoteClientError::Disconnected)?
            .map(V5Budgeted::into_inner);
        cancellation.check_cancelled(method)?;
        result
    }

    fn wake_outbound(&self) -> std::result::Result<(), RemoteClientError> {
        wake_v5_client_writer(&self.shared)
    }
}

fn apply_v5_directory_cache<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    request: &RemoteRequest,
    response: RemoteResponse,
) -> std::result::Result<RemoteResponse, RemoteClientError> {
    match (request, response) {
        (RemoteRequest::ListDir { path }, RemoteResponse::ListDir(listing)) => {
            let listing = resolve_v5_directory_listing_cache(shared, path, listing)?;
            Ok(RemoteResponse::ListDir(listing))
        }
        (RemoteRequest::ListDirs { .. }, RemoteResponse::ListDirs(mut response)) => {
            for result in &mut response.results {
                if let Some(listing) = result.listing.take() {
                    result.listing = Some(resolve_v5_directory_listing_cache(
                        shared,
                        &result.path,
                        listing,
                    )?);
                }
            }
            Ok(RemoteResponse::ListDirs(response))
        }
        (_, response) => Ok(response),
    }
}

fn resolve_v5_directory_listing_cache<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    cache_key: &Path,
    mut listing: DirectoryListingResponse,
) -> std::result::Result<DirectoryListingResponse, RemoteClientError> {
    let mut cache = shared
        .directory_cache
        .lock()
        .map_err(v5_client_lock_error)?;
    if listing.not_modified {
        return cache.get(cache_key).cloned().ok_or_else(|| {
            RemoteClientError::Protocol(format!(
                "v5 directory listing for {} was not_modified without a cached listing",
                cache_key.display()
            ))
        });
    }
    if let Some(delta) = listing.delta.take() {
        let base = cache.get(cache_key).cloned().ok_or_else(|| {
            RemoteClientError::Protocol(format!(
                "v5 directory listing for {} carried a delta without a cached base",
                cache_key.display()
            ))
        })?;
        listing = apply_directory_listing_delta(cache_key, base, listing, delta)?;
    }
    if listing.complete && listing.generation.is_some() {
        if !cache.contains_key(cache_key)
            && cache.len() >= V5_DIRECTORY_DELTA_CACHE_LIMIT
            && let Some(evicted) = cache.keys().next().cloned()
        {
            cache.remove(&evicted);
        }
        cache.insert(cache_key.to_path_buf(), listing.clone());
    }
    Ok(listing)
}

fn run_v5_client_reader<R, W>(
    mut reader: R,
    limits: protocol_v5::FrameLimits,
    mut inbound_frame_sequence: protocol_v5::InboundFrameSequence,
    shared: Weak<RemoteWorkspaceV5Shared<W>>,
) where
    R: Read,
    W: Write,
{
    loop {
        if shared.strong_count() == 0 {
            break;
        }
        match inbound_frame_sequence.read_frame(&mut reader, limits) {
            Ok(Some(frame)) => {
                let Some(shared) = shared.upgrade() else {
                    break;
                };
                let received_at = Instant::now();
                let frame_type = frame.frame_type;
                let pong_control = v5_client_pong_control(&frame);
                let event = {
                    let mut session = match shared.session.lock() {
                        Ok(session) => session,
                        Err(_) => {
                            fail_all_v5_waiters(&shared, || {
                                RemoteClientError::Protocol(
                                    "v5 session lock is poisoned".to_string(),
                                )
                            });
                            break;
                        }
                    };
                    session.receive_frame(frame)
                };
                match event {
                    Ok(event) => {
                        if let Some(stream_id) = v5_client_inbound_progress_stream(&event.routed)
                            && let Err(error) =
                                observe_v5_client_request_progress(&shared, stream_id, received_at)
                        {
                            fail_all_v5_waiters_for_error(&shared, &error);
                            break;
                        }
                        let heartbeat_result = shared
                            .heartbeat
                            .lock()
                            .map_err(v5_client_lock_error)
                            .and_then(|mut heartbeat| {
                                heartbeat.observe_inbound(frame_type, pong_control, received_at)
                            });
                        match heartbeat_result {
                            Ok(Some(rtt)) => {
                                tracing::trace!(
                                    rtt_micros = rtt.as_micros() as u64,
                                    "Received matching v5 client heartbeat PONG"
                                );
                            }
                            Ok(None) => {}
                            Err(error) => {
                                fail_all_v5_waiters_for_error(&shared, &error);
                                break;
                            }
                        }
                        signal_v5_client_heartbeat(&shared);
                        let data_credit = event.data_credit();
                        let acknowledge_data = event
                            .stream_event
                            .map(|stream_event| {
                                handle_v5_client_stream_event(&shared, stream_event)
                            })
                            .unwrap_or(true);
                        if acknowledge_data && let Some((stream_id, credit_bytes)) = data_credit {
                            let result = shared
                                .session
                                .lock()
                                .map_err(v5_client_lock_error)
                                .and_then(|mut session| {
                                    if session.stream_tombstone(stream_id).is_some() {
                                        Ok(())
                                    } else {
                                        session
                                            .acknowledge_data(stream_id, credit_bytes)
                                            .map_err(RemoteClientError::Io)
                                    }
                                });
                            if let Err(error) = result {
                                let message = error.to_string();
                                fail_all_v5_waiters(&shared, || {
                                    RemoteClientError::Protocol(format!(
                                        "failed to queue v5 flow-control update: {message}"
                                    ))
                                });
                                break;
                            }
                        }
                        if let Err(error) = wake_v5_client_writer(&shared) {
                            tracing::warn!(
                                error = %error,
                                "Closing v5 client after writer wake failed"
                            );
                            break;
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        fail_all_v5_waiters(&shared, || {
                            RemoteClientError::Protocol(format!(
                                "failed to route v5 response frame: {message}"
                            ))
                        });
                        break;
                    }
                }
            }
            Ok(None) => {
                if let Some(shared) = shared.upgrade() {
                    fail_all_v5_waiters(&shared, || RemoteClientError::Disconnected);
                }
                break;
            }
            Err(error) => {
                let kind = error.kind();
                let message = error.to_string();
                if let Some(shared) = shared.upgrade() {
                    fail_all_v5_waiters(&shared, || {
                        RemoteClientError::Io(io::Error::new(kind, message.clone()))
                    });
                }
                break;
            }
        }
    }
}

fn signal_v5_client_heartbeat<W>(shared: &RemoteWorkspaceV5Shared<W>) {
    let _ = shared.heartbeat_wake.try_send(());
}

fn register_v5_client_cancellation<W>(
    cancellation: &RemoteRequestCancellation,
    shared: &Arc<RemoteWorkspaceV5Shared<W>>,
    stream_id: u64,
    request: V5ClientCancellation,
) where
    W: 'static,
{
    let shared = Arc::downgrade(shared);
    cancellation.register(move || {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        let mut pending_cancellations = shared
            .pending_cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        pending_cancellations
            .entry(stream_id)
            .and_modify(|pending| {
                if request.mode == V5ClientCancellationMode::Connection {
                    *pending = request;
                }
            })
            .or_insert(request);
        drop(pending_cancellations);
        signal_v5_client_deadlines(&shared);
    });
}

fn signal_v5_client_deadlines<W>(shared: &RemoteWorkspaceV5Shared<W>) {
    let _ = shared.deadline_wake.try_send(());
}

fn run_v5_client_heartbeat<W>(wakes: mpsc::Receiver<()>, shared: Weak<RemoteWorkspaceV5Shared<W>>) {
    loop {
        let Some(shared) = shared.upgrade() else {
            break;
        };
        if shared.closed.load(Ordering::Acquire) {
            break;
        }

        let action = match shared
            .heartbeat
            .lock()
            .map_err(v5_client_lock_error)
            .and_then(|mut heartbeat| heartbeat.next_action(Instant::now()))
        {
            Ok(action) => action,
            Err(error) => {
                fail_all_v5_waiters_for_error(&shared, &error);
                break;
            }
        };

        match action {
            V5ClientHeartbeatAction::QueuePing(token) => {
                let queued = shared
                    .session
                    .lock()
                    .map_err(v5_client_lock_error)
                    .and_then(|mut session| {
                        session.send_ping(token).map_err(RemoteClientError::Io)
                    });
                if let Err(error) = queued {
                    fail_all_v5_waiters_for_error(&shared, &error);
                    break;
                }
                if wake_v5_client_writer(&shared).is_err() {
                    break;
                }
            }
            V5ClientHeartbeatAction::TimedOut(message) => {
                let error = RemoteClientError::Io(io::Error::new(io::ErrorKind::TimedOut, message));
                fail_all_v5_waiters_for_error(&shared, &error);
                break;
            }
            V5ClientHeartbeatAction::Wait(timeout) => {
                drop(shared);
                match wakes.recv_timeout(timeout) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }
    }
}

fn run_v5_client_deadlines<W>(wakes: mpsc::Receiver<()>, shared: Weak<RemoteWorkspaceV5Shared<W>>)
where
    W: Write,
{
    loop {
        let Some(shared) = shared.upgrade() else {
            break;
        };
        if shared.closed.load(Ordering::Acquire) {
            break;
        }

        let wait = match expire_v5_client_deadlines_at(&shared, Instant::now()) {
            Ok(wait) => wait,
            Err(error) => {
                fail_all_v5_waiters_for_error(&shared, &error);
                break;
            }
        };
        if shared.closed.load(Ordering::Acquire) {
            break;
        }
        drop(shared);

        let wake = match wait {
            Some(timeout) => wakes.recv_timeout(timeout),
            None => match wakes.recv() {
                Ok(()) => continue,
                Err(_) => break,
            },
        };
        match wake {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn process_v5_client_cancellations<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
) -> std::result::Result<bool, RemoteClientError> {
    if shared.closed.load(Ordering::Acquire) {
        return Ok(false);
    }
    let cancellations = std::mem::take(
        &mut *shared
            .pending_cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    if cancellations.is_empty() {
        return Ok(true);
    }

    let terminal_streams = cancellations
        .iter()
        .filter_map(|(stream_id, cancellation)| {
            (cancellation.mode == V5ClientCancellationMode::Connection).then_some(*stream_id)
        })
        .collect::<HashSet<_>>();
    if !terminal_streams.is_empty() {
        let response_pending = {
            let mut waiters = shared.waiters.lock().map_err(v5_client_lock_error)?;
            terminal_streams
                .iter()
                .filter_map(|stream_id| waiters.remove(stream_id))
                .collect::<Vec<_>>()
        };
        for pending in response_pending {
            let _ = pending
                .sender
                .send(Err(remote_request_cancelled_error(pending.method)));
        }
        let raw_pending = {
            let mut waiters = shared.raw_waiters.lock().map_err(v5_client_lock_error)?;
            terminal_streams
                .iter()
                .filter_map(|stream_id| waiters.remove(stream_id))
                .collect::<Vec<_>>()
        };
        for pending in raw_pending {
            let _ = pending
                .sender
                .send(Err(remote_request_cancelled_error(pending.method)));
        }
        if shared
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            finish_v5_connection_close(shared, || {
                RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "v5 watch control request cancelled by caller",
                ))
            });
        }
        return Ok(false);
    }

    let cancelled = {
        let mut waiters = shared.waiters.lock().map_err(v5_client_lock_error)?;
        cancellations
            .into_iter()
            .map(|(stream_id, cancellation)| (stream_id, cancellation, waiters.remove(&stream_id)))
            .collect::<Vec<_>>()
    };
    let mut reset_queued = false;
    for (stream_id, cancellation, pending) in cancelled {
        if let Some(pending) = pending {
            let _ = pending
                .sender
                .send(Err(remote_request_cancelled_error(pending.method)));
        }
        match shared
            .session
            .lock()
            .map_err(v5_client_lock_error)?
            .reset_stream(
                stream_id,
                protocol_v5::RESET_CANCELLED,
                format!("client dropped {} request handle", cancellation.method),
            )
            .map_err(RemoteClientError::Io)?
        {
            true => reset_queued = true,
            false => release_v5_outbound_request_reservation(shared, stream_id),
        }
    }
    if reset_queued {
        wake_v5_client_writer(shared)?;
    }
    Ok(!shared.closed.load(Ordering::Acquire))
}

fn expire_v5_client_deadlines_at<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    now: Instant,
) -> std::result::Result<Option<Duration>, RemoteClientError>
where
    W: Write,
{
    if shared.closed.load(Ordering::Acquire) {
        return Ok(None);
    }
    if !process_v5_client_cancellations(shared)? {
        return Ok(None);
    }
    let heartbeat = shared.heartbeat.lock().map_err(v5_client_lock_error)?;
    let peer_is_healthy = heartbeat.peer_is_healthy_at(now);

    let (raw_expired, raw_close_claimed) = {
        let raw_waiters = shared.raw_waiters.lock().map_err(v5_client_lock_error)?;
        let raw_expired = raw_waiters
            .values()
            .any(|pending| pending.deadline.expired_at(now).is_some());
        let close_claimed = raw_expired
            && shared
                .closed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
        (raw_expired, close_claimed)
    };
    if raw_expired {
        drop(heartbeat);
        if raw_close_claimed {
            finish_v5_connection_close(shared, || {
                RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "v5 watch control request deadline expired",
                ))
            });
        }
        return Ok(None);
    }

    let mut waiters = shared.waiters.lock().map_err(v5_client_lock_error)?;
    let connection_terminal = waiters.values().any(|pending| {
        pending.deadline.expired_at(now).is_some()
            && (!peer_is_healthy || pending.deadline_is_connection_terminal())
    });
    let close_claimed = connection_terminal
        && shared
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
    let expired = if connection_terminal {
        Vec::new()
    } else {
        let expired = waiters
            .iter()
            .filter_map(|(stream_id, pending)| {
                pending
                    .deadline
                    .expired_at(now)
                    .map(|kind| (*stream_id, kind))
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|(stream_id, kind)| {
                waiters
                    .remove(&stream_id)
                    .map(|pending| (stream_id, kind, pending))
            })
            .collect::<Vec<_>>()
    };
    drop(waiters);
    drop(heartbeat);
    if connection_terminal {
        let cause = if !peer_is_healthy {
            "v5 request deadline expired while peer health was unknown"
        } else {
            "v5 mutation request deadline expired"
        };
        if close_claimed {
            finish_v5_connection_close(shared, || {
                RemoteClientError::Io(io::Error::new(io::ErrorKind::TimedOut, cause))
            });
        }
        return Ok(None);
    }

    for (stream_id, kind, pending) in expired {
        let reset = match shared.session.lock() {
            Ok(mut session) => session
                .reset_stream(
                    stream_id,
                    protocol_v5::RESET_DEADLINE_EXCEEDED,
                    format!("client {kind} deadline expired"),
                )
                .map_err(RemoteClientError::Io),
            Err(error) => Err(v5_client_lock_error(error)),
        };
        match reset {
            Ok(true) => {
                if let Err(error) = wake_v5_client_writer(shared) {
                    let pending_error = pending.failure_error(RemoteClientError::TransportClosed {
                        cause: error.to_string(),
                    });
                    let _ = pending.sender.send(Err(pending_error));
                    fail_all_v5_waiters_for_error(shared, &error);
                    return Ok(None);
                }
            }
            Ok(false) => release_v5_outbound_request_reservation(shared, stream_id),
            Err(error) => {
                let pending_error = pending.failure_error(RemoteClientError::TransportClosed {
                    cause: error.to_string(),
                });
                let _ = pending.sender.send(Err(pending_error));
                fail_all_v5_waiters_for_error(shared, &error);
                return Ok(None);
            }
        }
        let error = RemoteClientError::RequestDeadlineExceeded {
            method: pending.method.to_string(),
            kind,
        };
        let _ = pending.sender.send(Err(error));
    }

    next_v5_client_deadline_wait(shared, Instant::now())
}

fn next_v5_client_deadline_wait<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    now: Instant,
) -> std::result::Result<Option<Duration>, RemoteClientError> {
    let response_deadline = shared
        .waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .filter_map(|pending| pending.deadline.next_expiry().map(|(deadline, _)| deadline))
        .min();
    let raw_deadline = shared
        .raw_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .filter_map(|pending| pending.deadline.next_expiry().map(|(deadline, _)| deadline))
        .min();
    Ok(match (response_deadline, raw_deadline) {
        (Some(response), Some(raw)) => Some(response.min(raw).saturating_duration_since(now)),
        (Some(deadline), None) | (None, Some(deadline)) => {
            Some(deadline.saturating_duration_since(now))
        }
        (None, None) => None,
    })
}

fn wake_v5_client_writer<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
) -> std::result::Result<(), RemoteClientError> {
    if shared.closed.load(Ordering::Acquire) {
        return Err(RemoteClientError::Disconnected);
    }
    match shared.writer_wake.try_send(()) {
        Ok(()) | Err(mpsc::TrySendError::Full(())) => Ok(()),
        Err(mpsc::TrySendError::Disconnected(())) => {
            let error = RemoteClientError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "v5 client writer stopped",
            ));
            fail_all_v5_waiters_for_error(shared, &error);
            Err(error)
        }
    }
}

fn run_v5_client_writer<W>(
    mut writer: RemoteWorkspaceV5Writer<W>,
    wakes: mpsc::Receiver<()>,
    shared: Weak<RemoteWorkspaceV5Shared<W>>,
) where
    W: Write,
{
    while wakes.recv().is_ok() {
        let Some(shared) = shared.upgrade() else {
            break;
        };
        if shared.closed.load(Ordering::Acquire) {
            break;
        }
        if let Err(error) = write_v5_client_outbound(&shared, &mut writer) {
            if !shared.closed.load(Ordering::Acquire) {
                tracing::warn!(
                    error = %error,
                    "Closing v5 client after writer pump failed"
                );
            }
            fail_all_v5_waiters_for_error(&shared, &error);
            break;
        }
    }
}

fn write_v5_client_outbound<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    writer: &mut RemoteWorkspaceV5Writer<W>,
) -> std::result::Result<(), RemoteClientError>
where
    W: Write,
{
    loop {
        if shared.closed.load(Ordering::Acquire) {
            return Err(RemoteClientError::Disconnected);
        }

        // Select one frame at a time so newly queued urgent control traffic can pre-empt the
        // remainder of this bounded flush batch.
        let mut processed_frames = 0;
        let mut wrote_frame = false;
        while processed_frames < V5_CLIENT_WRITE_BATCH_FRAMES {
            let Some(mut frame) = shared
                .session
                .lock()
                .map_err(v5_client_lock_error)?
                .pop_next_frame()?
            else {
                break;
            };
            processed_frames += 1;
            let should_write = {
                let mut session = shared.session.lock().map_err(v5_client_lock_error)?;
                let should_write = session.should_write_frame(&frame);
                if !should_write {
                    session.discard_unwritten_frame(&frame)?;
                }
                should_write
            };
            if !should_write || shared.closed.load(Ordering::Acquire) {
                continue;
            }

            if frame.frame_type == protocol_v5::FrameType::Ping {
                shared
                    .heartbeat
                    .lock()
                    .map_err(v5_client_lock_error)?
                    .mark_ping_started(&frame, Instant::now())?;
                signal_v5_client_heartbeat(shared);
            }
            let request_frame = frame.stream_id != 0
                && matches!(
                    frame.frame_type,
                    protocol_v5::FrameType::Headers
                        | protocol_v5::FrameType::Data
                        | protocol_v5::FrameType::EndStream
                );
            // Only a completed physical write advances inactivity. Mutation deadlines are
            // conservatively connection-terminal because reset and write can race at the
            // transport boundary.
            frame.frame_sequence = writer.next_frame_sequence;
            writer.next_frame_sequence =
                writer.next_frame_sequence.checked_add(1).ok_or_else(|| {
                    RemoteClientError::Protocol("v5 frame sequence exhausted".to_string())
                })?;
            let limits = writer.limits;
            protocol_v5::write_frame_unflushed_with_limits(&mut writer.writer, &frame, limits)?;
            {
                shared
                    .session
                    .lock()
                    .map_err(v5_client_lock_error)?
                    .observe_frame_written(&frame);
            }
            if request_frame {
                observe_v5_client_request_progress(shared, frame.stream_id, Instant::now())?;
            }
            if matches!(
                frame.frame_type,
                protocol_v5::FrameType::EndStream | protocol_v5::FrameType::ResetStream
            ) {
                release_v5_outbound_request_reservation(shared, frame.stream_id);
            }
            wrote_frame = true;
        }
        if wrote_frame {
            writer.writer.flush()?;
        }
        if processed_frames < V5_CLIENT_WRITE_BATCH_FRAMES {
            return Ok(());
        }
    }
}

fn v5_client_inbound_progress_stream(routed: &protocol_v5::RoutedFrame) -> Option<u64> {
    match routed {
        protocol_v5::RoutedFrame::WindowUpdate { stream_id, .. }
        | protocol_v5::RoutedFrame::Headers { stream_id, .. }
        | protocol_v5::RoutedFrame::Data { stream_id, .. }
        | protocol_v5::RoutedFrame::EndStream { stream_id, .. }
        | protocol_v5::RoutedFrame::ResetStream { stream_id, .. }
            if *stream_id != 0 =>
        {
            Some(*stream_id)
        }
        protocol_v5::RoutedFrame::ConnectionControl { .. }
        | protocol_v5::RoutedFrame::WindowUpdate { .. }
        | protocol_v5::RoutedFrame::Headers { .. }
        | protocol_v5::RoutedFrame::Data { .. }
        | protocol_v5::RoutedFrame::EndStream { .. }
        | protocol_v5::RoutedFrame::RejectedStream { .. }
        | protocol_v5::RoutedFrame::ResetStream { .. } => None,
    }
}

fn observe_v5_client_request_progress<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    stream_id: u64,
    now: Instant,
) -> std::result::Result<(), RemoteClientError> {
    if let Some(pending) = shared
        .waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .get_mut(&stream_id)
    {
        pending.deadline.observe_progress(now);
        return Ok(());
    }
    if let Some(pending) = shared
        .raw_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .get_mut(&stream_id)
    {
        pending.deadline.observe_progress(now);
    }
    Ok(())
}

fn release_v5_outbound_request_reservation<W>(shared: &RemoteWorkspaceV5Shared<W>, stream_id: u64) {
    shared
        .outbound_request_reservations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&stream_id);
}

fn handle_v5_client_stream_event<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    event: protocol_v5::StreamEvent,
) -> bool
where
    W: Write,
{
    if shared.closed.load(Ordering::Acquire) {
        return false;
    }
    let stream_id = event.stream_id();
    if matches!(&event, protocol_v5::StreamEvent::ResetStream { .. }) {
        release_v5_outbound_request_reservation(shared, stream_id);
    }
    let mut event = Some(event);
    let completed_response = {
        let mut waiters = match shared.waiters.lock() {
            Ok(waiters) => waiters,
            Err(_) => return false,
        };
        if shared.closed.load(Ordering::Acquire) {
            return false;
        }
        let result = if let Some(pending) = waiters.get_mut(&stream_id) {
            pending.accumulator.observe_with_reservation(
                event.take().expect("event should be available"),
                &mut pending.response_reservation,
            )
        } else {
            None
        };
        result.map(|result| (waiters.remove(&stream_id), result))
    };

    if let Some((Some(pending), result)) = completed_response {
        let normalized = normalize_v5_response_deadline(&pending, result);
        let result = normalized.result;
        let accepted = result.is_ok();
        if !accepted && !normalized.peer_deadline {
            reset_v5_client_stream_after_local_error(shared, stream_id);
        }
        let result = result.map(|value| V5Budgeted::new(value, pending.response_reservation));
        let _ = pending.sender.send(result);
        if normalized.terminal {
            fail_all_v5_waiters(shared, || {
                RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "v5 peer expired an ambiguous mutation request",
                ))
            });
            return false;
        }
        return accepted;
    }
    if event.is_none() {
        return true;
    }

    let completed_raw = {
        let mut raw_waiters = match shared.raw_waiters.lock() {
            Ok(waiters) => waiters,
            Err(_) => return false,
        };
        if shared.closed.load(Ordering::Acquire) {
            return false;
        }
        let result = if let Some(pending) = raw_waiters.get_mut(&stream_id) {
            pending.accumulator.observe_with_reservation(
                event.take().expect("event should be available"),
                &mut pending.response_reservation,
            )
        } else {
            None
        };
        result.map(|result| (raw_waiters.remove(&stream_id), result))
    };

    if let Some((Some(pending), result)) = completed_raw {
        let normalized = normalize_v5_raw_response_deadline(&pending, result);
        let result = normalized.result;
        let accepted = result.is_ok();
        if !accepted && !normalized.peer_deadline {
            reset_v5_client_stream_after_local_error(shared, stream_id);
        }
        let result = result.map(|value| V5Budgeted::new(value, pending.response_reservation));
        let _ = pending.sender.send(result);
        if normalized.terminal {
            fail_all_v5_waiters(shared, || {
                RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "v5 peer expired a watch control request",
                ))
            });
            return false;
        }
        return accepted;
    }
    if let Some(event) = event {
        handle_v5_client_watch_event(shared, event);
    }
    true
}

fn normalize_v5_response_deadline(
    pending: &V5PendingResponse,
    result: std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>,
) -> V5NormalizedDeadlineResult<(RemoteResponse, Vec<u8>)> {
    match result {
        Err(RemoteClientError::Remote(error))
            if error.code == protocol_v5::RESET_DEADLINE_EXCEEDED =>
        {
            if pending.deadline_is_connection_terminal() {
                let cause = RemoteClientError::Remote(error).to_string();
                V5NormalizedDeadlineResult {
                    result: Err(RemoteClientError::OutcomeUnknown {
                        method: pending.method.to_string(),
                        cause,
                    }),
                    peer_deadline: true,
                    terminal: true,
                }
            } else {
                V5NormalizedDeadlineResult {
                    result: Err(RemoteClientError::RequestDeadlineExceeded {
                        method: pending.method.to_string(),
                        kind: RemoteRequestDeadlineKind::Absolute,
                    }),
                    peer_deadline: true,
                    terminal: false,
                }
            }
        }
        result => V5NormalizedDeadlineResult {
            result,
            peer_deadline: false,
            terminal: false,
        },
    }
}

fn normalize_v5_raw_response_deadline(
    pending: &V5PendingRawResponse,
    result: std::result::Result<Vec<u8>, RemoteClientError>,
) -> V5NormalizedDeadlineResult<Vec<u8>> {
    match result {
        Err(RemoteClientError::Remote(error))
            if error.code == protocol_v5::RESET_DEADLINE_EXCEEDED =>
        {
            V5NormalizedDeadlineResult {
                result: Err(RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("v5 peer expired {}", pending.method),
                ))),
                peer_deadline: true,
                terminal: true,
            }
        }
        result => V5NormalizedDeadlineResult {
            result,
            peer_deadline: false,
            terminal: false,
        },
    }
}

fn reset_v5_client_stream_after_local_error<W>(shared: &RemoteWorkspaceV5Shared<W>, stream_id: u64)
where
    W: Write,
{
    let reset_queued = shared
        .session
        .lock()
        .map_err(v5_client_lock_error)
        .and_then(|mut session| {
            session
                .reset_stream(
                    stream_id,
                    protocol_v5::RESET_RESOURCE_EXHAUSTED,
                    "client rejected response stream",
                )
                .map_err(RemoteClientError::Io)
        });
    match reset_queued {
        Ok(true) => {
            let _ = wake_v5_client_writer(shared);
        }
        Ok(false) => {
            // A response END can close the logical stream before a flow-blocked request body
            // reaches the wire. `reset_stream` purges that non-tombstoned scheduler state but
            // cannot queue another terminal frame for the already-closed stream, so no writer
            // observation remains to release the retained request bytes.
            release_v5_outbound_request_reservation(shared, stream_id);
        }
        Err(error) => fail_all_v5_waiters_for_error(shared, &error),
    }
}

fn fail_all_v5_waiters<W, F>(shared: &RemoteWorkspaceV5Shared<W>, make_error: F)
where
    F: Fn() -> RemoteClientError,
{
    if shared
        .closed
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    finish_v5_connection_close(shared, make_error);
}

fn finish_v5_connection_close<W, F>(shared: &RemoteWorkspaceV5Shared<W>, make_error: F)
where
    F: Fn() -> RemoteClientError,
{
    let _ = shared.writer_wake.try_send(());
    let _ = shared.heartbeat_wake.try_send(());
    let _ = shared.deadline_wake.try_send(());
    shared
        .session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .terminate();
    shared
        .outbound_request_reservations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .pending_cancellations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    if let Some(abort) = &shared.transport_abort {
        abort.abort();
    }
    let waiters = match shared.waiters.lock() {
        Ok(mut waiters) => std::mem::take(&mut *waiters),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, pending) in waiters {
        let error = pending.failure_error(make_error());
        let _ = pending.sender.send(Err(error));
    }
    let raw_waiters = match shared.raw_waiters.lock() {
        Ok(mut raw_waiters) => std::mem::take(&mut *raw_waiters),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, pending) in raw_waiters {
        let error = pending.failure_error(make_error());
        let _ = pending.sender.send(Err(error));
    }
    shared
        .watch_batches
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .watch_backlog
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .watch_stream_by_id
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .directory_cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

fn fail_all_v5_waiters_for_error<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    error: &RemoteClientError,
) {
    match error {
        RemoteClientError::Io(error) => {
            let kind = error.kind();
            let message = error.to_string();
            fail_all_v5_waiters(shared, || {
                RemoteClientError::Io(io::Error::new(kind, message.clone()))
            });
        }
        RemoteClientError::Json(error) => {
            let message = error.to_string();
            fail_all_v5_waiters(shared, || {
                RemoteClientError::Protocol(format!(
                    "v5 transport closed after JSON error: {message}"
                ))
            });
        }
        RemoteClientError::Disconnected => {
            fail_all_v5_waiters(shared, || RemoteClientError::Disconnected);
        }
        RemoteClientError::TransportClosed { cause } => {
            fail_all_v5_waiters(shared, || RemoteClientError::TransportClosed {
                cause: cause.clone(),
            });
        }
        RemoteClientError::RequestDeadlineExceeded { method, kind } => {
            fail_all_v5_waiters(shared, || RemoteClientError::RequestDeadlineExceeded {
                method: method.clone(),
                kind: *kind,
            });
        }
        RemoteClientError::OutcomeUnknown { method, cause } => {
            fail_all_v5_waiters(shared, || RemoteClientError::OutcomeUnknown {
                method: method.clone(),
                cause: cause.clone(),
            });
        }
        RemoteClientError::ResponseIncomplete { cause } => {
            fail_all_v5_waiters(shared, || RemoteClientError::ResponseIncomplete {
                cause: cause.clone(),
            });
        }
        RemoteClientError::Protocol(message) => {
            fail_all_v5_waiters(shared, || RemoteClientError::Protocol(message.clone()));
        }
        RemoteClientError::Remote(error) => {
            fail_all_v5_waiters(shared, || RemoteClientError::Remote(error.clone()));
        }
    }
}

impl V5ResponseAccumulator {
    fn final_message_seen(&self) -> bool {
        self.method.is_some() || self.final_error.is_some()
    }

    #[cfg(test)]
    fn observe(
        &mut self,
        event: protocol_v5::StreamEvent,
    ) -> Option<std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>> {
        self.observe_inner(event, None)
    }

    fn observe_with_reservation(
        &mut self,
        event: protocol_v5::StreamEvent,
        reservation: &mut V5ByteReservation,
    ) -> Option<std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>> {
        self.observe_inner(event, Some(reservation))
    }

    fn observe_inner(
        &mut self,
        event: protocol_v5::StreamEvent,
        reservation: Option<&mut V5ByteReservation>,
    ) -> Option<std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>> {
        match event {
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalResponse,
                envelope,
                ..
            } => {
                if let Err(error) = self.search_partials.finish_current() {
                    return Some(Err(error));
                }
                self.method = Some(envelope.method);
                None
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalError,
                envelope,
                ..
            } => {
                if let Err(error) = self.search_partials.finish_current() {
                    return Some(Err(error));
                }
                self.method = Some(envelope.method.clone());
                self.final_error = Some(match v5_final_error_from_envelope(envelope) {
                    Ok(error) => error,
                    Err(error) => return Some(Err(error)),
                });
                None
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::PartialResult,
                envelope,
                ..
            } => match self.search_partials.begin_partial(envelope.method) {
                Ok(()) => None,
                Err(error) => Some(Err(error)),
            },
            protocol_v5::StreamEvent::Data { channel, body, .. } => {
                let Some(received_bytes) = self.received_bytes.checked_add(body.len()) else {
                    return Some(Err(RemoteClientError::Protocol(
                        "v5 response decoded byte count overflowed".to_string(),
                    )));
                };
                if received_bytes > V5_MAX_ACCUMULATED_RESPONSE_BYTES {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 response exceeds decoded byte limit of {V5_MAX_ACCUMULATED_RESPONSE_BYTES}"
                    ))));
                }
                if let Some(reservation) = reservation
                    && let Err(error) = reservation.try_grow(body.len())
                {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 response exceeds connection retained-byte budget: {error}"
                    ))));
                }
                self.received_bytes = received_bytes;
                match channel {
                    protocol_v5::DataChannel::Unspecified => self.payload.extend(body),
                    protocol_v5::DataChannel::SearchPayload => {
                        self.search_partials.push_search_payload(body);
                    }
                    protocol_v5::DataChannel::FileBody | protocol_v5::DataChannel::Stdin => {
                        self.file_body.extend(body)
                    }
                    protocol_v5::DataChannel::Stdout => self.stdout.extend(body),
                    protocol_v5::DataChannel::Stderr => self.stderr.extend(body),
                }
                None
            }
            protocol_v5::StreamEvent::EndStream { stream_id } => {
                if let Some(error) = self.final_error.take() {
                    return Some(Err(RemoteClientError::Remote(error)));
                }
                let Some(method) = self.method.take() else {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 stream {stream_id} ended without final response"
                    ))));
                };
                let response = match self.search_partials.merge_final(&method, &self.payload) {
                    Ok(Some(response)) => response,
                    Ok(None) => match RemoteResponse::from_v5_payload(&method, &self.payload) {
                        Ok(response) => response,
                        Err(error) => return Some(Err(v5_method_error_to_client_error(error))),
                    },
                    Err(error) => return Some(Err(error)),
                };
                let body = v5_client_body_for_response(
                    &response,
                    std::mem::take(&mut self.file_body),
                    std::mem::take(&mut self.stdout),
                    std::mem::take(&mut self.stderr),
                );
                Some(Ok((response, body)))
            }
            protocol_v5::StreamEvent::ResetStream {
                code, diagnostic, ..
            } => Some(Err(RemoteClientError::Remote(RemoteError {
                code,
                message: "v5 stream reset".to_string(),
                diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
            }))),
            protocol_v5::StreamEvent::Headers { .. } => None,
        }
    }
}

impl V5RawResponseAccumulator {
    fn final_message_seen(&self) -> bool {
        self.final_seen || self.final_error.is_some()
    }

    #[cfg(test)]
    fn observe(
        &mut self,
        event: protocol_v5::StreamEvent,
    ) -> Option<std::result::Result<Vec<u8>, RemoteClientError>> {
        self.observe_inner(event, None)
    }

    fn observe_with_reservation(
        &mut self,
        event: protocol_v5::StreamEvent,
        reservation: &mut V5ByteReservation,
    ) -> Option<std::result::Result<Vec<u8>, RemoteClientError>> {
        self.observe_inner(event, Some(reservation))
    }

    fn observe_inner(
        &mut self,
        event: protocol_v5::StreamEvent,
        reservation: Option<&mut V5ByteReservation>,
    ) -> Option<std::result::Result<Vec<u8>, RemoteClientError>> {
        match event {
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalResponse,
                ..
            } => {
                self.final_seen = true;
                None
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalError,
                envelope,
                ..
            } => {
                self.final_error = Some(match v5_final_error_from_envelope(envelope) {
                    Ok(error) => error,
                    Err(error) => return Some(Err(error)),
                });
                None
            }
            protocol_v5::StreamEvent::Data { channel, body, .. } => {
                let Some(received_bytes) = self.received_bytes.checked_add(body.len()) else {
                    return Some(Err(RemoteClientError::Protocol(
                        "v5 raw response decoded byte count overflowed".to_string(),
                    )));
                };
                if received_bytes > V5_MAX_RAW_RESPONSE_BYTES {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 raw response exceeds decoded byte limit of {V5_MAX_RAW_RESPONSE_BYTES}"
                    ))));
                }
                match channel {
                    protocol_v5::DataChannel::Unspecified => {}
                    protocol_v5::DataChannel::SearchPayload => {}
                    protocol_v5::DataChannel::FileBody
                    | protocol_v5::DataChannel::Stdin
                    | protocol_v5::DataChannel::Stdout
                    | protocol_v5::DataChannel::Stderr => {
                        return Some(Err(RemoteClientError::Protocol(format!(
                            "unexpected v5 raw response data channel: {channel:?}"
                        ))));
                    }
                }
                if channel == protocol_v5::DataChannel::Unspecified {
                    if let Some(reservation) = reservation
                        && let Err(error) = reservation.try_grow(body.len())
                    {
                        return Some(Err(RemoteClientError::Protocol(format!(
                            "v5 raw response exceeds connection retained-byte budget: {error}"
                        ))));
                    }
                    self.payload.extend(body);
                }
                self.received_bytes = received_bytes;
                None
            }
            protocol_v5::StreamEvent::EndStream { stream_id } => {
                if let Some(error) = self.final_error.take() {
                    return Some(Err(RemoteClientError::Remote(error)));
                }
                if !self.final_seen {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 raw stream {stream_id} ended without final response"
                    ))));
                }
                Some(Ok(std::mem::take(&mut self.payload)))
            }
            protocol_v5::StreamEvent::ResetStream {
                code, diagnostic, ..
            } => Some(Err(RemoteClientError::Remote(RemoteError {
                code,
                message: "v5 stream reset".to_string(),
                diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
            }))),
            protocol_v5::StreamEvent::Headers { .. } => None,
        }
    }
}

fn handle_v5_client_watch_event<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    event: protocol_v5::StreamEvent,
) {
    match event {
        protocol_v5::StreamEvent::Headers {
            stream_id,
            role: protocol_v5::MessageRole::Event,
            envelope,
            ..
        } => {
            let Some(protocol_v5::stream_envelope::Message::Event(event)) = envelope.message else {
                return;
            };
            if event.kind != "watch.batch" {
                return;
            }
            let Some(batch) = event.watch_batch else {
                return;
            };
            send_or_backlog_v5_watch_batch(shared, stream_id, batch);
        }
        protocol_v5::StreamEvent::EndStream { stream_id }
        | protocol_v5::StreamEvent::ResetStream { stream_id, .. } => {
            if let Ok(mut watch_batches) = shared.watch_batches.lock() {
                watch_batches.remove(&stream_id);
            }
            if let Ok(mut watch_backlog) = shared.watch_backlog.lock() {
                watch_backlog.remove(&stream_id);
            }
            if let Ok(mut watch_stream_by_id) = shared.watch_stream_by_id.lock() {
                watch_stream_by_id.retain(|_, event_stream_id| *event_stream_id != stream_id);
            }
        }
        _ => {}
    }
}

fn send_or_backlog_v5_watch_batch<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    stream_id: u64,
    batch: protocol_v5::WatchBatch,
) {
    invalidate_v5_directory_cache_after_watch_batch(shared, &batch);

    let delivery = match shared.watch_batches.lock() {
        Ok(watch_batches) => watch_batches.get(&stream_id).cloned(),
        Err(_) => return,
    };
    if let Some(delivery) = delivery {
        delivery
            .last_sequence
            .store(batch.sequence, Ordering::Release);
        if delivery.overflowed.load(Ordering::Acquire) {
            clear_v5_directory_cache(shared);
            return;
        }
        match delivery.sender.try_send(batch) {
            Ok(()) => return,
            Err(mpsc::TrySendError::Full(_)) => {
                delivery.overflowed.store(true, Ordering::Release);
                clear_v5_directory_cache(shared);
                return;
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                if let Ok(mut watch_batches) = shared.watch_batches.lock() {
                    watch_batches.remove(&stream_id);
                }
                return;
            }
        }
    }

    let Ok(mut watch_backlog) = shared.watch_backlog.lock() else {
        return;
    };
    let backlog = watch_backlog.entry(stream_id).or_default();
    if let Some(overflow) = backlog
        .back_mut()
        .filter(|batch| batch.overflow && batch.resync_required)
    {
        overflow.sequence = batch.sequence;
        return;
    }
    if backlog.len() >= V5_WATCH_BACKLOG_LIMIT {
        let mut overflow = batch;
        overflow.directory_generations.clear();
        overflow.events.clear();
        overflow.overflow = true;
        overflow.resync_required = true;
        backlog.clear();
        backlog.push_back(overflow);
        drop(watch_backlog);
        clear_v5_directory_cache(shared);
        return;
    }
    backlog.push_back(batch);
}

fn invalidate_v5_directory_cache_after_watch_batch<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    batch: &protocol_v5::WatchBatch,
) {
    if !batch.overflow && !batch.resync_required {
        return;
    }
    clear_v5_directory_cache(shared);
}

fn clear_v5_directory_cache<W>(shared: &RemoteWorkspaceV5Shared<W>) {
    if let Ok(mut directory_cache) = shared.directory_cache.lock() {
        directory_cache.clear();
    }
}

fn v5_client_lock_error<T>(_error: std::sync::PoisonError<T>) -> RemoteClientError {
    RemoteClientError::Protocol("v5 client lock is poisoned".to_string())
}

fn v5_method_error_to_client_error(error: V5MethodError) -> RemoteClientError {
    RemoteClientError::Protocol(error.to_string())
}

fn v5_method_error_to_remote_error(error: V5MethodError) -> RemoteError {
    RemoteError {
        code: "invalid_request".to_string(),
        message: error.to_string(),
        diagnostic: None,
    }
}

fn v5_final_error_from_envelope(
    envelope: protocol_v5::StreamEnvelope,
) -> std::result::Result<RemoteError, RemoteClientError> {
    match envelope.message {
        Some(protocol_v5::stream_envelope::Message::Error(error)) => Ok(RemoteError {
            code: error.code,
            message: error.message,
            diagnostic: (!error.details.is_empty()).then_some(error.details),
        }),
        _ => Err(RemoteClientError::Protocol(format!(
            "v5 final_error for {} omitted error payload",
            envelope.method
        ))),
    }
}

fn v5_client_body_for_response(
    response: &RemoteResponse,
    file_body: Vec<u8>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
) -> Vec<u8> {
    if matches!(response, RemoteResponse::RunProcess(_)) {
        let mut body = stdout;
        body.extend(stderr);
        body
    } else if !file_body.is_empty() {
        file_body
    } else {
        let mut body = stdout;
        body.extend(stderr);
        body
    }
}

fn workspace_watch_from_v5(
    watch: RemoteWorkspaceV5Watch,
    workspace_root: PathBuf,
) -> WorkspaceWatch {
    let (sender, receiver) = mpsc::sync_channel(V5_WATCH_DELIVERY_CAPACITY);
    let watch_id = watch.watch_id;
    let event_stream_id = watch.event_stream_id;
    std::thread::Builder::new()
        .name("nucleotide-v5-watch-map".to_string())
        .spawn(move || {
            while let Ok(batch) = watch.recv() {
                let batch = workspace_watch_batch_from_v5(batch, &workspace_root);
                if sender.send(batch).is_err() {
                    break;
                }
            }
        })
        .ok();
    WorkspaceWatch::new(watch_id, event_stream_id, receiver)
}

fn workspace_watch_update_from_v5(
    response: protocol_v5::WatchUpdateResponse,
    workspace_root: &Path,
) -> WorkspaceWatchUpdate {
    WorkspaceWatchUpdate {
        watch_id: response.watch_id,
        accepted_roots: response
            .accepted_roots
            .iter()
            .map(|path| v5_watch_path_to_workspace_path(workspace_root, path))
            .collect(),
        degraded_roots: response
            .degraded_roots
            .iter()
            .map(|path| v5_watch_path_to_workspace_path(workspace_root, path))
            .collect(),
        unsupported_roots: response
            .unsupported_roots
            .iter()
            .map(|path| v5_watch_path_to_workspace_path(workspace_root, path))
            .collect(),
    }
}

fn workspace_watch_batch_from_v5(
    batch: protocol_v5::WatchBatch,
    workspace_root: &Path,
) -> WorkspaceWatchBatch {
    WorkspaceWatchBatch {
        watch_id: batch.watch_id,
        sequence: batch.sequence,
        directory_generations: batch
            .directory_generations
            .into_iter()
            .map(|generation| WorkspaceWatchDirectoryGeneration {
                path: v5_watch_path_to_workspace_path(workspace_root, &generation.path),
                generation: generation.generation,
            })
            .collect(),
        events: batch
            .events
            .into_iter()
            .map(|event| WorkspaceWatchChange {
                kind: workspace_watch_change_kind_from_v5(event.kind),
                path: v5_watch_path_to_workspace_path(workspace_root, &event.path),
                old_path: (!event.old_path.is_empty())
                    .then(|| v5_watch_path_to_workspace_path(workspace_root, &event.old_path)),
                is_dir: event.is_dir,
            })
            .collect(),
        overflow: batch.overflow,
        resync_required: batch.resync_required,
    }
}

fn workspace_watch_change_kind_from_v5(kind: i32) -> WorkspaceWatchChangeKind {
    match protocol_v5::WatchChangeKind::try_from(kind) {
        Ok(protocol_v5::WatchChangeKind::Created) => WorkspaceWatchChangeKind::Created,
        Ok(protocol_v5::WatchChangeKind::Modified) => WorkspaceWatchChangeKind::Modified,
        Ok(protocol_v5::WatchChangeKind::Deleted) => WorkspaceWatchChangeKind::Deleted,
        Ok(protocol_v5::WatchChangeKind::Renamed) => WorkspaceWatchChangeKind::Renamed,
        Err(_) => WorkspaceWatchChangeKind::Modified,
    }
}

fn v5_watch_path_to_workspace_path(workspace_root: &Path, path: &str) -> PathBuf {
    if path.is_empty() || path == "." {
        return workspace_root.to_path_buf();
    }
    let path = Path::new(path);
    if path.is_absolute() {
        normalize_path_lexically(path)
    } else {
        normalize_path_lexically(&workspace_root.join(path))
    }
}

pub trait RemoteWorkspaceProtocolClient: Send + Sync {
    fn request(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>;

    fn request_with_context(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
        _context: RemoteRequestContext,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        self.request(request, body)
    }

    fn request_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let method = request.v5_method();
        cancellation.check_cancelled(method)?;
        let result = self.request_with_context(request, body, context);
        cancellation.check_cancelled(method)?;
        result
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError>;

    /// Closes the current transport without performing protocol I/O or waiting for the peer.
    fn close(&self) {}

    fn start_watch(
        &self,
        _request: WorkspaceWatchRequest,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        Ok(None)
    }

    fn start_watch_with_context(
        &self,
        request: WorkspaceWatchRequest,
        _context: RemoteRequestContext,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        self.start_watch(request)
    }

    fn start_watch_with_context_and_cancellation(
        &self,
        request: WorkspaceWatchRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        cancellation.check_cancelled("watch.start")?;
        let result = self.start_watch_with_context(request, context);
        cancellation.check_cancelled("watch.start")?;
        result
    }

    fn update_watch(
        &self,
        _watch_id: u64,
        _add_roots: Vec<PathBuf>,
        _remove_roots: Vec<PathBuf>,
    ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
        Ok(None)
    }

    fn update_watch_with_cancellation(
        &self,
        watch_id: u64,
        add_roots: Vec<PathBuf>,
        remove_roots: Vec<PathBuf>,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
        cancellation.check_cancelled("watch.update")?;
        let result = self.update_watch(watch_id, add_roots, remove_roots);
        cancellation.check_cancelled("watch.update")?;
        result
    }

    fn stop_watch(&self, _watch_id: u64) -> std::result::Result<(), RemoteClientError> {
        Ok(())
    }

    fn stop_watch_with_cancellation(
        &self,
        watch_id: u64,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        cancellation.check_cancelled("watch.stop")?;
        let result = self.stop_watch(watch_id);
        cancellation.check_cancelled("watch.stop")?;
        result
    }
}

type ReconnectFactory<C> =
    dyn Fn() -> std::result::Result<C, RemoteClientError> + Send + Sync + 'static;

pub struct ReconnectingRemoteWorkspaceProtocolClient<C: RemoteWorkspaceProtocolClient> {
    client: Mutex<Option<Arc<C>>>,
    reconnect_gate: Mutex<()>,
    reconnect: Arc<ReconnectFactory<C>>,
    closed: AtomicBool,
}

impl<C> ReconnectingRemoteWorkspaceProtocolClient<C>
where
    C: RemoteWorkspaceProtocolClient + 'static,
{
    pub fn new(
        client: C,
        reconnect: impl Fn() -> std::result::Result<C, RemoteClientError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            client: Mutex::new(Some(Arc::new(client))),
            reconnect_gate: Mutex::new(()),
            reconnect: Arc::new(reconnect),
            closed: AtomicBool::new(false),
        }
    }

    fn current_client(&self) -> std::result::Result<Arc<C>, RemoteClientError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(RemoteClientError::Disconnected);
        }
        if let Some(client) = self
            .client
            .lock()
            .map_err(|_| {
                RemoteClientError::Protocol("remote reconnect client lock is poisoned".to_string())
            })?
            .as_ref()
        {
            return Ok(Arc::clone(client));
        }

        let _gate = self.reconnect_gate.lock().map_err(|_| {
            RemoteClientError::Protocol("remote reconnect gate is poisoned".to_string())
        })?;
        if self.closed.load(Ordering::Acquire) {
            return Err(RemoteClientError::Disconnected);
        }
        let current = self.client.lock().map_err(|_| {
            RemoteClientError::Protocol("remote reconnect client lock is poisoned".to_string())
        })?;
        if let Some(client) = current.as_ref() {
            return Ok(Arc::clone(client));
        }
        drop(current);

        let reconnected = Arc::new((self.reconnect)()?);
        if self.closed.load(Ordering::Acquire) {
            reconnected.close();
            return Err(RemoteClientError::Disconnected);
        }
        let mut current = self.client.lock().map_err(|_| {
            RemoteClientError::Protocol("remote reconnect client lock is poisoned".to_string())
        })?;
        if self.closed.load(Ordering::Acquire) {
            drop(current);
            reconnected.close();
            return Err(RemoteClientError::Disconnected);
        }
        *current = Some(Arc::clone(&reconnected));
        Ok(reconnected)
    }

    fn reconnect_if_current(
        &self,
        stale: &Arc<C>,
    ) -> std::result::Result<Arc<C>, RemoteClientError> {
        let _gate = self.reconnect_gate.lock().map_err(|_| {
            RemoteClientError::Protocol("remote reconnect gate is poisoned".to_string())
        })?;
        if self.closed.load(Ordering::Acquire) {
            return Err(RemoteClientError::Disconnected);
        }
        let mut current = self.client.lock().map_err(|_| {
            RemoteClientError::Protocol("remote reconnect client lock is poisoned".to_string())
        })?;
        if let Some(client) = current.as_ref()
            && !Arc::ptr_eq(client, stale)
        {
            return Ok(Arc::clone(client));
        }

        // Clear and physically close the stale transport before reconnecting. The factory runs
        // outside the client lock so a concurrent nonblocking close never waits for startup.
        let stale_client = current.take();
        drop(current);
        if let Some(stale_client) = stale_client {
            stale_client.close();
        }
        let reconnected = Arc::new((self.reconnect)()?);
        if self.closed.load(Ordering::Acquire) {
            reconnected.close();
            return Err(RemoteClientError::Disconnected);
        }
        let mut current = self.client.lock().map_err(|_| {
            RemoteClientError::Protocol("remote reconnect client lock is poisoned".to_string())
        })?;
        if self.closed.load(Ordering::Acquire) {
            drop(current);
            reconnected.close();
            return Err(RemoteClientError::Disconnected);
        }
        *current = Some(Arc::clone(&reconnected));
        Ok(reconnected)
    }

    fn discard_if_current(&self, stale: &Arc<C>) -> std::result::Result<(), RemoteClientError> {
        let _gate = self.reconnect_gate.lock().map_err(|_| {
            RemoteClientError::Protocol("remote reconnect gate is poisoned".to_string())
        })?;
        let mut current = self.client.lock().map_err(|_| {
            RemoteClientError::Protocol("remote reconnect client lock is poisoned".to_string())
        })?;
        if current
            .as_ref()
            .is_some_and(|client| Arc::ptr_eq(client, stale))
        {
            let stale = current.take();
            drop(current);
            if let Some(stale) = stale {
                stale.close();
            }
        }
        Ok(())
    }
}

impl<C> RemoteWorkspaceProtocolClient for ReconnectingRemoteWorkspaceProtocolClient<C>
where
    C: RemoteWorkspaceProtocolClient + 'static,
{
    fn request(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let context = request.v5_request_context();
        self.request_with_context(request, body, context)
    }

    fn request_with_context(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
        context: RemoteRequestContext,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        self.request_with_context_and_cancellation(
            request,
            body,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn request_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        body: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let method = request.v5_method();
        cancellation.check_cancelled(method)?;
        let idempotency = request.v5_request_options().idempotency;
        let retry_allowed = request.v5_retry_after_reconnect_allowed();
        let retry_request = retry_allowed.then(|| request.clone());
        let retry_body = retry_allowed.then(|| body.clone());
        let client = self.current_client()?;
        cancellation.check_cancelled(method)?;

        match client.request_with_context_and_cancellation(request, body, context, cancellation) {
            Ok(response) => {
                cancellation.check_cancelled(method)?;
                Ok(response)
            }
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled(method)?;
                let retry_safe =
                    retry_allowed && remote_client_error_allows_reconnect_retry(&error);
                let recovery = self.reconnect_if_current(&client);
                cancellation.check_cancelled(method)?;
                if retry_safe {
                    let retry_request = retry_request.expect("retry request recorded");
                    let retry_body = retry_body.expect("retry body recorded");
                    tracing::warn!(
                        error = %error,
                        "Retrying read-only v5 remote request after reconnect"
                    );
                    let retry_client = recovery?;
                    if let Some(kind) = context.expired_at(Instant::now()) {
                        return Err(RemoteClientError::RequestDeadlineExceeded {
                            method: method.to_string(),
                            kind,
                        });
                    }
                    cancellation.check_cancelled(method)?;
                    let result = retry_client.request_with_context_and_cancellation(
                        retry_request,
                        retry_body,
                        context,
                        cancellation,
                    );
                    cancellation.check_cancelled(method)?;
                    if let Err(retry_error) = &result
                        && remote_client_error_requires_reconnect(retry_error)
                        && let Err(close_error) = self.discard_if_current(&retry_client)
                    {
                        tracing::warn!(
                            error = %close_error,
                            retry_error = %retry_error,
                            "Failed to invalidate v5 transport after replay failure"
                        );
                    }
                    return result;
                }

                if let Err(reconnect_error) = recovery {
                    tracing::warn!(
                        error = %reconnect_error,
                        original_error = %error,
                        "Failed to heal v5 transport after request failure"
                    );
                }
                if idempotency != protocol_v5::Idempotency::ReadOnly
                    && !matches!(error, RemoteClientError::OutcomeUnknown { .. })
                {
                    Err(RemoteClientError::OutcomeUnknown {
                        method: method.to_string(),
                        cause: error.to_string(),
                    })
                } else {
                    Err(error)
                }
            }
            Err(error) => {
                cancellation.check_cancelled(method)?;
                Err(error)
            }
        }
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        self.current_client()?.shutdown()
    }

    fn close(&self) {
        if self
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let current = self
            .client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(current) = current {
            current.close();
        }
    }

    fn start_watch(
        &self,
        request: WorkspaceWatchRequest,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        self.start_watch_with_context(request, v5_watch_control_request_context())
    }

    fn start_watch_with_context(
        &self,
        request: WorkspaceWatchRequest,
        context: RemoteRequestContext,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        self.start_watch_with_context_and_cancellation(
            request,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn start_watch_with_context_and_cancellation(
        &self,
        request: WorkspaceWatchRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        cancellation.check_cancelled("watch.start")?;
        let client = self.current_client()?;
        cancellation.check_cancelled("watch.start")?;
        match client.start_watch_with_context_and_cancellation(
            request.clone(),
            context,
            cancellation,
        ) {
            Ok(watch) => {
                cancellation.check_cancelled("watch.start")?;
                Ok(watch)
            }
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled("watch.start")?;
                tracing::warn!(
                    error = %error,
                    "Retrying v5 watch.start after reconnect"
                );
                let retry_client = self.reconnect_if_current(&client)?;
                cancellation.check_cancelled("watch.start")?;
                if let Some(kind) = context.expired_at(Instant::now()) {
                    return Err(RemoteClientError::RequestDeadlineExceeded {
                        method: "watch.start".to_string(),
                        kind,
                    });
                }
                let result = retry_client.start_watch_with_context_and_cancellation(
                    request,
                    context,
                    cancellation,
                );
                cancellation.check_cancelled("watch.start")?;
                if let Err(retry_error) = &result
                    && remote_client_error_requires_reconnect(retry_error)
                    && let Err(close_error) = self.discard_if_current(&retry_client)
                {
                    tracing::warn!(
                        error = %close_error,
                        retry_error = %retry_error,
                        "Failed to invalidate v5 transport after watch.start replay failure"
                    );
                }
                result
            }
            Err(error) => {
                cancellation.check_cancelled("watch.start")?;
                Err(error)
            }
        }
    }

    fn update_watch(
        &self,
        watch_id: u64,
        add_roots: Vec<PathBuf>,
        remove_roots: Vec<PathBuf>,
    ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
        self.update_watch_with_cancellation(
            watch_id,
            add_roots,
            remove_roots,
            &RemoteRequestCancellation::new(),
        )
    }

    fn update_watch_with_cancellation(
        &self,
        watch_id: u64,
        add_roots: Vec<PathBuf>,
        remove_roots: Vec<PathBuf>,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
        cancellation.check_cancelled("watch.update")?;
        let client = self.current_client()?;
        cancellation.check_cancelled("watch.update")?;
        match client.update_watch_with_cancellation(watch_id, add_roots, remove_roots, cancellation)
        {
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled("watch.update")?;
                if let Err(reconnect_error) = self.reconnect_if_current(&client) {
                    tracing::warn!(
                        error = %reconnect_error,
                        original_error = %error,
                        "Failed to heal v5 transport after watch.update failure"
                    );
                }
                Err(error)
            }
            result => {
                cancellation.check_cancelled("watch.update")?;
                result
            }
        }
    }

    fn stop_watch(&self, watch_id: u64) -> std::result::Result<(), RemoteClientError> {
        self.stop_watch_with_cancellation(watch_id, &RemoteRequestCancellation::new())
    }

    fn stop_watch_with_cancellation(
        &self,
        watch_id: u64,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        cancellation.check_cancelled("watch.stop")?;
        let client = self.current_client()?;
        cancellation.check_cancelled("watch.stop")?;
        match client.stop_watch_with_cancellation(watch_id, cancellation) {
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled("watch.stop")?;
                if let Err(reconnect_error) = self.reconnect_if_current(&client) {
                    tracing::warn!(
                        error = %reconnect_error,
                        original_error = %error,
                        "Failed to heal v5 transport after watch.stop failure"
                    );
                }
                Err(error)
            }
            result => {
                cancellation.check_cancelled("watch.stop")?;
                result
            }
        }
    }
}

fn remote_client_error_allows_reconnect_retry(error: &RemoteClientError) -> bool {
    match error {
        RemoteClientError::Disconnected | RemoteClientError::TransportClosed { .. } => true,
        RemoteClientError::Io(_) => true,
        RemoteClientError::Json(_)
        | RemoteClientError::RequestDeadlineExceeded { .. }
        | RemoteClientError::OutcomeUnknown { .. }
        | RemoteClientError::ResponseIncomplete { .. }
        | RemoteClientError::Protocol(_)
        | RemoteClientError::Remote(_) => false,
    }
}

fn remote_client_error_requires_reconnect(error: &RemoteClientError) -> bool {
    remote_client_error_allows_reconnect_retry(error)
        || matches!(
            error,
            RemoteClientError::Io(_)
                | RemoteClientError::OutcomeUnknown { .. }
                | RemoteClientError::ResponseIncomplete { .. }
        )
}

fn transport_closed_before_final_error(error: RemoteClientError) -> RemoteClientError {
    match error {
        RemoteClientError::Json(_) | RemoteClientError::Protocol(_) => {
            RemoteClientError::TransportClosed {
                cause: error.to_string(),
            }
        }
        error => error,
    }
}

fn disconnect_after_final_response_error(error: RemoteClientError) -> RemoteClientError {
    match error {
        RemoteClientError::Remote(_) | RemoteClientError::ResponseIncomplete { .. } => error,
        error => RemoteClientError::ResponseIncomplete {
            cause: error.to_string(),
        },
    }
}

pub struct RemoteWorkspaceBackendImpl<C: RemoteWorkspaceProtocolClient> {
    identity: RemoteWorkspaceIdentity,
    client: Arc<C>,
}

pub type RemoteWorkspaceV5Backend<R, W> =
    RemoteWorkspaceBackendImpl<RemoteWorkspaceV5MultiplexedClient<R, W>>;
type RemoteWorkspaceV5ChildClient =
    RemoteWorkspaceV5MultiplexedClient<ChildStdout, ChildProcessV5Writer>;
type RemoteWorkspaceV5ReconnectingClient =
    ReconnectingRemoteWorkspaceProtocolClient<RemoteWorkspaceV5ChildClient>;

impl<R, W> RemoteWorkspaceBackendImpl<RemoteWorkspaceV5MultiplexedClient<R, W>>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    pub fn new(
        identity: RemoteWorkspaceIdentity,
        client: RemoteWorkspaceV5MultiplexedClient<R, W>,
    ) -> Self {
        Self {
            identity,
            client: Arc::new(client),
        }
    }

    pub fn connect(
        identity: RemoteWorkspaceIdentity,
        client: RemoteWorkspaceV5MultiplexedClient<R, W>,
    ) -> std::result::Result<(Self, HelloResponse), RemoteClientError> {
        let hello = hello_response_from_v5_server_hello(client.server_hello());
        Ok((Self::new(identity, client), hello))
    }
}

impl<C> RemoteWorkspaceBackendImpl<C>
where
    C: RemoteWorkspaceProtocolClient,
{
    fn from_protocol_client(identity: RemoteWorkspaceIdentity, client: C) -> Self {
        Self {
            identity,
            client: Arc::new(client),
        }
    }

    async fn request(
        &self,
        operation: &'static str,
        path: &Path,
        request: RemoteRequest,
        body: Vec<u8>,
    ) -> nucleotide_workspace::Result<(RemoteResponse, Vec<u8>)>
    where
        C: 'static,
    {
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let client = self.client.clone();
        let identity = self.identity.clone();
        let path = path.to_path_buf();
        let worker_path = path.clone();
        let (sender, receiver) = oneshot::channel();
        let queued_at = Instant::now();

        tracing::trace!(
            operation,
            path = %path.display(),
            remote_kind = ?identity.kind,
            remote_name = %identity.name,
            "Remote workspace request queued"
        );

        std::thread::Builder::new()
            .name(format!("nucleotide-remote-{operation}"))
            .spawn(move || {
                let queued_ms = queued_at.elapsed().as_millis() as u64;
                if queued_ms >= REMOTE_QUEUE_SLOW_LOG_MS {
                    tracing::info!(
                        operation,
                        path = %worker_path.display(),
                        remote_kind = ?identity.kind,
                        remote_name = %identity.name,
                        queued_ms,
                        "Slow remote workspace request queue"
                    );
                } else {
                    tracing::debug!(
                        operation,
                        path = %worker_path.display(),
                        remote_kind = ?identity.kind,
                        remote_name = %identity.name,
                        queued_ms,
                        "Remote workspace request started"
                    );
                }
                let started_at = Instant::now();
                let result = request_with_client(
                    client.as_ref(),
                    &identity,
                    operation,
                    &worker_path,
                    request,
                    body,
                    &cancellation,
                );
                let elapsed_ms = started_at.elapsed().as_millis() as u64;
                match &result {
                    Ok(_) => {
                        if elapsed_ms >= REMOTE_REQUEST_SLOW_LOG_MS {
                            tracing::info!(
                                operation,
                                path = %worker_path.display(),
                                remote_kind = ?identity.kind,
                                remote_name = %identity.name,
                                elapsed_ms,
                                "Slow remote workspace request completed"
                            );
                        } else {
                            tracing::debug!(
                                operation,
                                path = %worker_path.display(),
                                remote_kind = ?identity.kind,
                                remote_name = %identity.name,
                                elapsed_ms,
                                "Remote workspace request completed"
                            );
                        }
                    }
                    Err(error) => tracing::warn!(
                        operation,
                        path = %worker_path.display(),
                        remote_kind = ?identity.kind,
                        remote_name = %identity.name,
                        elapsed_ms,
                        error = %error,
                        "Remote workspace request failed"
                    ),
                }
                let _ = sender.send(result);
            })
            .map_err(|source| WorkspaceError::Io {
                operation,
                path: path.clone(),
                source,
            })?;

        match receiver.await {
            Ok(result) => {
                cancel_on_drop.disarm();
                result
            }
            Err(_) => Err(WorkspaceError::Remote {
                operation,
                path,
                message: "remote request worker exited before returning a response".to_string(),
                diagnostic: None,
            }),
        }
    }
}

fn request_with_client<C>(
    client: &C,
    identity: &RemoteWorkspaceIdentity,
    operation: &'static str,
    path: &Path,
    request: RemoteRequest,
    body: Vec<u8>,
    cancellation: &RemoteRequestCancellation,
) -> nucleotide_workspace::Result<(RemoteResponse, Vec<u8>)>
where
    C: RemoteWorkspaceProtocolClient,
{
    let waiting_at = Instant::now();
    tracing::trace!(
        operation,
        path = %path.display(),
        remote_kind = ?identity.kind,
        remote_name = %identity.name,
        "Remote workspace request waiting for transport"
    );
    let transport_wait_ms = waiting_at.elapsed().as_millis() as u64;
    if transport_wait_ms >= REMOTE_TRANSPORT_WAIT_SLOW_LOG_MS {
        tracing::info!(
            operation,
            path = %path.display(),
            remote_kind = ?identity.kind,
            remote_name = %identity.name,
            transport_wait_ms,
            "Remote workspace request waited for transport"
        );
    } else {
        tracing::debug!(
            operation,
            path = %path.display(),
            remote_kind = ?identity.kind,
            remote_name = %identity.name,
            transport_wait_ms,
            "Remote workspace request acquired transport"
        );
    }
    let context = request.v5_request_context();
    client
        .request_with_context_and_cancellation(request, body, context, cancellation)
        .map_err(|error| client_error_to_workspace(operation, path, error))
}

impl<C> Drop for RemoteWorkspaceBackendImpl<C>
where
    C: RemoteWorkspaceProtocolClient,
{
    fn drop(&mut self) {
        self.client.close();
    }
}

pub fn spawn_child_process_workspace_backend(
    identity: RemoteWorkspaceIdentity,
    command: &RemoteServiceCommand,
) -> Result<(WorkspaceBackendHandle, HelloResponse)> {
    spawn_child_process_workspace_v5_backend(identity, command)
}

fn spawn_child_process_workspace_v5_backend(
    identity: RemoteWorkspaceIdentity,
    command: &RemoteServiceCommand,
) -> Result<(WorkspaceBackendHandle, HelloResponse)> {
    tracing::info!(
        remote_kind = ?identity.kind,
        remote_name = %identity.name,
        command = %command.display_context(),
        "Starting v5 remote workspace service process"
    );
    let (io, control) = spawn_child_process_v5_io(command).with_context(|| {
        format!(
            "failed to start v5 remote workspace service: {}",
            command.display_context()
        )
    })?;
    let client_hello = protocol_v5::ClientHello::nucleotide(env!("CARGO_PKG_VERSION"));
    let client = connect_child_process_v5_client(io, control, client_hello).with_context(|| {
            format!(
                "failed to connect to v5 remote workspace service after starting {}; verify the helper speaks protocol v5",
                command.display_context()
            )
        })?;
    let hello = hello_response_from_v5_server_hello(client.server_hello());
    let reconnect_command = command.clone();
    let reconnect_identity = identity.clone();
    let reconnecting_client: RemoteWorkspaceV5ReconnectingClient =
        ReconnectingRemoteWorkspaceProtocolClient::new(client, move || {
            tracing::info!(
                remote_kind = ?reconnect_identity.kind,
                remote_name = %reconnect_identity.name,
                command = %reconnect_command.display_context(),
                "Reconnecting v5 remote workspace service process"
            );
            let (io, control) = spawn_child_process_v5_io(&reconnect_command)?;
            let client_hello = protocol_v5::ClientHello::nucleotide(env!("CARGO_PKG_VERSION"));
            connect_child_process_v5_client(io, control, client_hello)
        });
    let backend =
        RemoteWorkspaceBackendImpl::from_protocol_client(identity.clone(), reconnecting_client);
    tracing::info!(
        remote_kind = ?identity.kind,
        remote_name = %identity.name,
        workspace_root = %hello.workspace_root.display(),
        helper_version = %hello.helper_version,
        helper_os = %hello.os,
        helper_arch = %hello.arch,
        "V5 remote workspace service hello completed"
    );

    Ok((Arc::new(backend), hello))
}

#[async_trait]
impl<C> WorkspaceBackend for RemoteWorkspaceBackendImpl<C>
where
    C: RemoteWorkspaceProtocolClient + 'static,
{
    fn identity(&self) -> WorkspaceIdentity {
        WorkspaceIdentity::Remote(self.identity.clone())
    }

    async fn stat(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request(
                "stat",
                path,
                RemoteRequest::Stat {
                    path: path.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::Stat(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("stat", path, other)),
        }
    }

    async fn list_dir(&self, path: &Path) -> nucleotide_workspace::Result<DirectoryListing> {
        let (response, _) = self
            .request(
                "list directory",
                path,
                RemoteRequest::ListDir {
                    path: path.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::ListDir(listing) => Ok(directory_listing_from_response(listing)),
            other => Err(unexpected_response_error("list directory", path, other)),
        }
    }

    async fn list_dirs(
        &self,
        paths: Vec<PathBuf>,
    ) -> Vec<(PathBuf, nucleotide_workspace::Result<DirectoryListing>)> {
        if paths.is_empty() {
            return Vec::new();
        }

        let representative_path = paths.first().cloned().unwrap_or_else(|| PathBuf::from("."));
        let response = self
            .request(
                "list directories",
                &representative_path,
                RemoteRequest::ListDirs {
                    paths: paths.clone(),
                },
                Vec::new(),
            )
            .await;

        match response {
            Ok((RemoteResponse::ListDirs(response), _)) => response
                .results
                .into_iter()
                .map(|result| {
                    let listing = match (result.listing, result.error) {
                        (Some(listing), None) => Ok(directory_listing_from_response(listing)),
                        (_, Some(error)) => Err(remote_error_to_workspace(
                            "list directories",
                            &result.path,
                            error,
                        )),
                        (None, None) => Err(WorkspaceError::Remote {
                            operation: "list directories",
                            path: result.path.clone(),
                            message: "remote list directories response omitted listing and error"
                                .to_string(),
                            diagnostic: None,
                        }),
                    };
                    (result.path, listing)
                })
                .collect(),
            Ok((other, _)) => paths
                .into_iter()
                .map(|path| {
                    let error = unexpected_response_error("list directories", &path, other.clone());
                    (path, Err(error))
                })
                .collect(),
            Err(error) => {
                let message = error.to_string();
                let diagnostic = Some(format!("{error:?}"));
                paths
                    .into_iter()
                    .map(|path| {
                        (
                            path.clone(),
                            Err(WorkspaceError::Remote {
                                operation: "list directories",
                                path,
                                message: message.clone(),
                                diagnostic: diagnostic.clone(),
                            }),
                        )
                    })
                    .collect()
            }
        }
    }

    async fn find_ancestor_file(
        &self,
        start: &Path,
        file_name: &str,
        limit: usize,
    ) -> nucleotide_workspace::Result<Option<PathBuf>> {
        let (response, _) = self
            .request(
                "find ancestor file",
                start,
                RemoteRequest::FindAncestorFile {
                    start: start.to_path_buf(),
                    file_name: file_name.to_string(),
                    limit,
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::FindAncestorFile(path) => Ok(path),
            other => Err(unexpected_response_error(
                "find ancestor file",
                start,
                other,
            )),
        }
    }

    async fn create_file(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request(
                "create file",
                path,
                RemoteRequest::CreateFile {
                    path: path.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::CreateFile(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("create file", path, other)),
        }
    }

    async fn create_dir(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request(
                "create directory",
                path,
                RemoteRequest::CreateDir {
                    path: path.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::CreateDir(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("create directory", path, other)),
        }
    }

    async fn rename_path(&self, from: &Path, to: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request(
                "rename path",
                from,
                RemoteRequest::RenamePath {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::RenamePath(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("rename path", from, other)),
        }
    }

    async fn delete_path(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request(
                "delete path",
                path,
                RemoteRequest::DeletePath {
                    path: path.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::DeletePath(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("delete path", path, other)),
        }
    }

    async fn copy_path(&self, from: &Path, to: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request(
                "copy path",
                from,
                RemoteRequest::CopyPath {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::CopyPath(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("copy path", from, other)),
        }
    }

    async fn read_file(
        &self,
        path: &Path,
        options: ReadOptions,
    ) -> nucleotide_workspace::Result<FileRead> {
        let (response, body) = self
            .request(
                "read file",
                path,
                RemoteRequest::ReadFile {
                    path: path.to_path_buf(),
                    max_bytes: options.max_bytes,
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::ReadFile(read) => file_read_from_response(read, body)
                .map_err(|error| client_error_to_workspace("read file", path, error)),
            other => Err(unexpected_response_error("read file", path, other)),
        }
    }

    async fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        options: WriteOptions,
    ) -> nucleotide_workspace::Result<WriteResult> {
        let (response, _) = self
            .request(
                "write file",
                path,
                RemoteRequest::WriteFile {
                    path: path.to_path_buf(),
                    create_parent_dirs: options.create_parent_dirs,
                    expected_modified_unix_millis: options
                        .expected_modified
                        .and_then(system_time_unix_millis),
                    expected_modified_unix_nanos: options
                        .expected_modified
                        .and_then(system_time_unix_nanos),
                },
                bytes.to_vec(),
            )
            .await?;
        match response {
            RemoteResponse::WriteFile(result) => Ok(write_result_from_response(result)),
            other => Err(unexpected_response_error("write file", path, other)),
        }
    }

    async fn file_search(
        &self,
        query: FileSearchQuery,
    ) -> nucleotide_workspace::Result<FileSearchResult> {
        let root = query.root.clone();
        let request = FileSearchRequest {
            root: query.root,
            pattern: query.pattern,
            limit: query.limit,
            hidden: query.hidden,
            parents: query.parents,
            ignore: query.ignore,
            git_ignore: query.git_ignore,
            git_global: query.git_global,
            git_exclude: query.git_exclude,
            follow_links: query.follow_links,
            max_depth: query.max_depth,
            excluded_relative_prefixes: query.excluded_relative_prefixes,
        };
        let (response, _) = self
            .request(
                "file search",
                &root,
                RemoteRequest::FileSearch(request),
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::FileSearch(result) => Ok(file_search_from_response(result)),
            other => Err(unexpected_response_error("file search", &root, other)),
        }
    }

    async fn text_search(
        &self,
        query: TextSearchQuery,
    ) -> nucleotide_workspace::Result<TextSearchResult> {
        let root = query.root.clone();
        let request = TextSearchRequest {
            root: query.root,
            pattern: query.pattern,
            limit: query.limit,
            smart_case: query.smart_case,
            hidden: query.hidden,
            parents: query.parents,
            ignore: query.ignore,
            git_ignore: query.git_ignore,
            git_global: query.git_global,
            git_exclude: query.git_exclude,
            follow_links: query.follow_links,
            max_depth: query.max_depth,
            max_file_bytes: query.max_file_bytes,
            excluded_relative_paths: query.excluded_relative_paths,
            custom_ignore_filenames: query.custom_ignore_filenames,
        };
        let (response, _) = self
            .request(
                "text search",
                &root,
                RemoteRequest::TextSearch(request),
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::TextSearch(result) => Ok(text_search_from_response(result)),
            other => Err(unexpected_response_error("text search", &root, other)),
        }
    }

    async fn project_environment(
        &self,
        root: &Path,
    ) -> nucleotide_workspace::Result<ProjectEnvironmentSnapshot> {
        let (response, _) = self
            .request(
                "project environment",
                root,
                RemoteRequest::ProjectEnvironment {
                    root: root.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::ProjectEnvironment(snapshot) => {
                Ok(project_environment_from_response(snapshot))
            }
            other => Err(unexpected_response_error(
                "project environment",
                root,
                other,
            )),
        }
    }

    async fn git_head(&self, root: &Path) -> nucleotide_workspace::Result<GitHeadResult> {
        let (response, _) = self
            .request(
                "git head",
                root,
                RemoteRequest::GitHead {
                    root: root.to_path_buf(),
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::GitHead(result) => Ok(git_head_from_response(result)),
            other => Err(unexpected_response_error("git head", root, other)),
        }
    }

    async fn git_status(
        &self,
        root: &Path,
        options: GitStatusOptions,
    ) -> nucleotide_workspace::Result<GitStatusResult> {
        let (response, _) = self
            .request(
                "git status",
                root,
                RemoteRequest::GitStatus {
                    root: root.to_path_buf(),
                    include_untracked: options.include_untracked,
                    limit: options.limit,
                },
                Vec::new(),
            )
            .await?;
        match response {
            RemoteResponse::GitStatus(result) => Ok(git_status_from_response(result)),
            other => Err(unexpected_response_error("git status", root, other)),
        }
    }

    async fn run_process(&self, spec: ProcessSpec) -> nucleotide_workspace::Result<ProcessOutput> {
        let cwd = spec.cwd.clone();
        let request = ProcessRequest {
            program: spec.program,
            args: spec.args,
            cwd: spec.cwd,
            env: spec.env,
            clear_env: spec.clear_env,
            inherit_project_environment: spec.inherit_project_environment,
            max_output_bytes: spec.max_output_bytes,
            timeout_ms: spec.timeout_ms,
        };
        let (response, body) = self
            .request(
                "run process",
                &cwd,
                RemoteRequest::RunProcess(request),
                spec.stdin,
            )
            .await?;
        match response {
            RemoteResponse::RunProcess(result) => process_output_from_response(result, body)
                .map_err(|error| client_error_to_workspace("run process", &cwd, error)),
            other => Err(unexpected_response_error("run process", &cwd, other)),
        }
    }

    async fn start_watch(
        &self,
        request: WorkspaceWatchRequest,
    ) -> nucleotide_workspace::Result<Option<WorkspaceWatch>> {
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let client = self.client.clone();
        let path = request
            .roots
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));
        let worker_path = path.clone();
        let (sender, receiver) = oneshot::channel();
        std::thread::Builder::new()
            .name("nucleotide-remote-start-watch".to_string())
            .spawn(move || {
                let result = client
                    .start_watch_with_context_and_cancellation(
                        request,
                        v5_watch_control_request_context(),
                        &cancellation,
                    )
                    .map_err(|error| client_error_to_workspace("start watch", &worker_path, error));
                if let Err(Ok(Some(watch))) = sender.send(result) {
                    let _ = client.stop_watch(watch.watch_id);
                }
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "start watch",
                path: path.clone(),
                source,
            })?;
        match receiver.await {
            Ok(result) => {
                cancel_on_drop.disarm();
                result
            }
            Err(_) => Err(WorkspaceError::Remote {
                operation: "start watch",
                path,
                message: "remote watch worker exited before returning a response".to_string(),
                diagnostic: None,
            }),
        }
    }

    async fn update_watch(
        &self,
        watch_id: u64,
        add_roots: Vec<PathBuf>,
        remove_roots: Vec<PathBuf>,
    ) -> nucleotide_workspace::Result<Option<WorkspaceWatchUpdate>> {
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let client = self.client.clone();
        let path = add_roots
            .first()
            .or_else(|| remove_roots.first())
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));
        let worker_path = path.clone();
        let (sender, receiver) = oneshot::channel();
        std::thread::Builder::new()
            .name("nucleotide-remote-update-watch".to_string())
            .spawn(move || {
                let _ = sender.send(
                    client
                        .update_watch_with_cancellation(
                            watch_id,
                            add_roots,
                            remove_roots,
                            &cancellation,
                        )
                        .map_err(|error| {
                            client_error_to_workspace("update watch", &worker_path, error)
                        }),
                );
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "update watch",
                path: path.clone(),
                source,
            })?;
        match receiver.await {
            Ok(result) => {
                cancel_on_drop.disarm();
                result
            }
            Err(_) => Err(WorkspaceError::Remote {
                operation: "update watch",
                path,
                message: "remote watch worker exited before returning a response".to_string(),
                diagnostic: None,
            }),
        }
    }

    async fn stop_watch(&self, watch_id: u64) -> nucleotide_workspace::Result<()> {
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let client = self.client.clone();
        let path = PathBuf::from(".");
        let worker_path = path.clone();
        let (sender, receiver) = oneshot::channel();
        std::thread::Builder::new()
            .name("nucleotide-remote-stop-watch".to_string())
            .spawn(move || {
                let _ = sender.send(
                    client
                        .stop_watch_with_cancellation(watch_id, &cancellation)
                        .map_err(|error| {
                            client_error_to_workspace("stop watch", &worker_path, error)
                        }),
                );
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "stop watch",
                path: path.clone(),
                source,
            })?;
        match receiver.await {
            Ok(result) => {
                cancel_on_drop.disarm();
                result
            }
            Err(_) => Err(WorkspaceError::Remote {
                operation: "stop watch",
                path,
                message: "remote watch worker exited before returning a response".to_string(),
                diagnostic: None,
            }),
        }
    }
}

pub struct WorkspaceService<B> {
    backend: B,
    workspace_root: PathBuf,
    ignore_matcher: Option<Gitignore>,
    directory_delta_cache: Mutex<HashMap<PathBuf, DirectoryListingResponse>>,
    project_environment: ProjectEnvironment,
    runtime: tokio::runtime::Runtime,
}

impl<B> WorkspaceService<B>
where
    B: WorkspaceBackend,
{
    pub fn new(backend: B, workspace_root: PathBuf) -> Result<Self> {
        Self::with_environment_baseline(backend, workspace_root, std::env::vars().collect())
    }

    pub fn with_environment_baseline(
        backend: B,
        workspace_root: PathBuf,
        environment_baseline: HashMap<String, String>,
    ) -> Result<Self> {
        let workspace_root = normalize_path_lexically(&workspace_root);
        let ignore_matcher = build_service_ignore_matcher(&workspace_root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to create remote workspace runtime")?;
        Ok(Self {
            backend,
            workspace_root,
            ignore_matcher,
            directory_delta_cache: Mutex::new(HashMap::new()),
            project_environment: ProjectEnvironment::new(Some(environment_baseline)),
            runtime,
        })
    }

    pub fn serve_v5<R: Read, W: Write>(
        &self,
        io: &mut protocol_v5::FramedIo<R, W>,
        info: &protocol_v5::ServerHandshakeInfo,
    ) -> Result<()> {
        let handshake = protocol_v5::server_handshake(io, info).context("v5 handshake failed")?;
        let mut session = protocol_v5::ProtocolSession::new(
            protocol_v5::StreamInitiator::Server,
            &handshake.settings,
        );
        let request_budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
        let mut requests = HashMap::<u64, V5ServiceRequest>::new();
        let mut watches = V5WatchRegistry::default();

        while let Some(frame) = io
            .read_frame()
            .context("failed to read v5 protocol frame")?
        {
            let event = session
                .receive_frame(frame)
                .context("failed to route v5 protocol frame")?;
            let data_credit = event.data_credit();
            let mut shutdown = false;
            let mut acknowledge_data = true;
            if let Some(stream_event) = event.stream_event {
                (shutdown, acknowledge_data) = self.handle_v5_stream_event(
                    &mut session,
                    &mut requests,
                    &request_budget,
                    &mut watches,
                    stream_event,
                )?;
            }
            if acknowledge_data && let Some((stream_id, credit_bytes)) = data_credit {
                session
                    .acknowledge_data(stream_id, credit_bytes)
                    .context("failed to queue v5 data window update")?;
            }
            self.poll_v5_watches(&mut session, &mut watches)?;
            self.drain_v5_session(&mut session, io)?;
            if shutdown {
                break;
            }
        }

        Ok(())
    }

    pub fn serve_v5_concurrent<R, W>(
        &self,
        mut io: protocol_v5::FramedIo<R, W>,
        info: &protocol_v5::ServerHandshakeInfo,
    ) -> Result<()>
    where
        R: Read + Send + 'static,
        W: Write,
    {
        let handshake =
            protocol_v5::server_handshake(&mut io, info).context("v5 handshake failed")?;
        let mut session = protocol_v5::ProtocolSession::new(
            protocol_v5::StreamInitiator::Server,
            &handshake.settings,
        );
        let request_budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
        let parts = io.into_parts();
        let mut writer = parts.writer;
        let limits = parts.limits;
        let mut inbound_frame_sequence = parts.inbound_frame_sequence;
        let mut next_frame_sequence = parts.next_frame_sequence;
        let mut requests = HashMap::<u64, V5ServiceRequest>::new();
        let (events_tx, events_rx) = mpsc::channel::<V5ServeEvent>();
        let (inbound_tx, inbound_rx) =
            mpsc::sync_channel::<V5InboundEvent>(V5_SERVE_INBOUND_EVENT_CAPACITY);
        let inbound_events = V5InboundSender::new(inbound_tx, events_tx.clone());
        let reader_events = inbound_events.clone();

        std::thread::Builder::new()
            .name("nucleotide-v5-reader".to_string())
            .spawn(move || {
                let mut reader = parts.reader;
                loop {
                    let result = inbound_frame_sequence.read_frame(&mut reader, limits);
                    let done = !matches!(result, Ok(Some(_)));
                    if reader_events.send(result).is_err() {
                        break;
                    }
                    if done {
                        break;
                    }
                }
            })
            .context("failed to spawn v5 service reader")?;

        std::thread::scope(|scope| -> Result<()> {
            let (output_tx, output_rx) =
                mpsc::sync_channel::<V5ServeOutputEvent>(V5_SERVE_OUTPUT_EVENT_CAPACITY);
            let output_events = V5ServeOutputSender::new(output_tx, events_tx.clone());
            let (native_watch_tx, native_watch_rx) =
                mpsc::sync_channel::<V5NativeWatchEvent>(V5_NATIVE_WATCH_EVENT_CAPACITY);
            let native_watch_events = V5NativeWatchSender::new(native_watch_tx, events_tx.clone());
            let mut inbound_closed = false;
            let mut active_workers = 0_usize;
            let mut task_pools = V5ServiceTaskPools::default();
            let mut active_streams = HashSet::<u64>::new();
            let mut active_task_classes = HashMap::<u64, V5ServiceTaskClass>::new();
            let mut active_cancellations = HashMap::<u64, WorkspaceCancellationToken>::new();
            let mut active_deadlines = HashMap::<u64, u64>::new();
            let mut canceled_streams = HashSet::<u64>::new();
            let mut watches = V5WatchRegistry::with_native_events(native_watch_events.clone());
            let mut shutdown = false;
            let shutdown_grace =
                Duration::from_millis(u64::from(handshake.settings.shutdown_grace_ms));
            let mut shutdown_started: Option<Instant> = None;
            let idle_ping_interval =
                Duration::from_millis(u64::from(handshake.settings.idle_ping_interval_ms));
            let ping_timeout = Duration::from_millis(u64::from(handshake.settings.ping_timeout_ms));
            let mut last_activity = Instant::now();
            let mut outstanding_ping: Option<(Vec<u8>, Instant)> = None;
            let mut next_ping_id = 0_u64;

            macro_rules! start_v5_service_worker {
                ($stream_id:expr, $request:expr) => {{
                    let stream_id = $stream_id;
                    let request = $request;
                    let priority = request.priority;
                    let deadline_unix_ms = request.deadline_unix_ms;
                    let task_class = task_pools.mark_started(&request.method);
                    active_workers += 1;
                    active_streams.insert(stream_id);
                    active_task_classes.insert(stream_id, task_class);
                    let cancellation = WorkspaceCancellationToken::new();
                    active_cancellations.insert(stream_id, cancellation.clone());
                    if deadline_unix_ms != 0 {
                        active_deadlines.insert(stream_id, deadline_unix_ms);
                    }
                    let worker_output_events = output_events.clone();
                    let worker_events = events_tx.clone();
                    scope.spawn(move || {
                        let completion = self.execute_v5_request(
                            stream_id,
                            request,
                            Some(worker_output_events.clone()),
                            Some(cancellation.clone()),
                        );
                        let terminal_queued = matches!(
                            self.enqueue_v5_service_completion(
                                completion,
                                priority,
                                &worker_output_events,
                                &cancellation,
                            ),
                            Ok(true)
                        );
                        let _ = worker_events.send(V5ServeEvent::WorkerFinished {
                            stream_id,
                            terminal_queued,
                        });
                    });
                }};
            }

            macro_rules! drain_v5_service_task_queue {
                () => {{
                    while let Some((stream_id, request)) = task_pools.pop_next_startable() {
                        if v5_deadline_expired(request.deadline_unix_ms) {
                            session
                                .reset_stream(
                                    stream_id,
                                    protocol_v5::RESET_DEADLINE_EXCEEDED,
                                    "request deadline expired",
                                )
                                .context("failed to reset expired v5 request stream")?;
                            continue;
                        }
                        start_v5_service_worker!(stream_id, request);
                    }
                }};
            }

            macro_rules! apply_v5_output_event {
                ($event:expr) => {{
                    let output_event = $event;
                    match output_event {
                        V5ServeOutputEvent::StreamData {
                            stream_id,
                            channel,
                            body,
                            priority,
                        } => {
                            if active_streams.contains(&stream_id)
                                && !canceled_streams.contains(&stream_id)
                            {
                                session
                                    .send_owned_data(stream_id, channel, body, priority)
                                    .context("failed to queue v5 streamed response data")?;
                            }
                        }
                        V5ServeOutputEvent::PartialResponse {
                            stream_id,
                            method,
                            payload,
                            priority,
                        } => {
                            if active_streams.contains(&stream_id)
                                && !canceled_streams.contains(&stream_id)
                            {
                                session
                                    .send_response_with_priority(
                                        stream_id,
                                        method,
                                        protocol_v5::MessageRole::PartialResult,
                                        false,
                                        priority,
                                    )
                                    .context("failed to queue v5 partial response headers")?;
                                session
                                    .send_owned_data(
                                        stream_id,
                                        protocol_v5::DataChannel::SearchPayload,
                                        payload,
                                        priority,
                                    )
                                    .context("failed to queue v5 partial response payload")?;
                            }
                        }
                        V5ServeOutputEvent::Progress {
                            stream_id,
                            method,
                            progress,
                        } => {
                            if active_streams.contains(&stream_id)
                                && !canceled_streams.contains(&stream_id)
                            {
                                let priority = session
                                    .stream_priority(stream_id)
                                    .unwrap_or(protocol_v5::Priority::Background);
                                session
                                    .send_progress_with_priority(
                                        stream_id, method, progress, priority,
                                    )
                                    .context("failed to queue v5 progress response")?;
                            }
                        }
                        V5ServeOutputEvent::Completed(completion) => {
                            active_cancellations.remove(&completion.stream_id);
                            let deadline_unix_ms =
                                active_deadlines.remove(&completion.stream_id).unwrap_or(0);
                            active_streams.remove(&completion.stream_id);
                            if canceled_streams.remove(&completion.stream_id) {
                                tracing::debug!(
                                    stream_id = completion.stream_id,
                                    method = %completion.method,
                                    "Suppressing v5 response for canceled stream"
                                );
                            } else if v5_deadline_expired(deadline_unix_ms) {
                                session
                                    .reset_stream(
                                        completion.stream_id,
                                        protocol_v5::RESET_DEADLINE_EXCEEDED,
                                        "request deadline expired before response delivery",
                                    )
                                    .context(
                                        "failed to reset v5 request that completed after deadline",
                                    )?;
                            } else {
                                shutdown |=
                                    self.apply_v5_service_terminal(&mut session, completion)?;
                            }
                        }
                    }
                    output_events.mark_delivered();
                }};
            }

            loop {
                if inbound_closed && active_workers == 0 && !task_pools.has_pending() {
                    break;
                }
                if shutdown
                    && active_workers == 0
                    && !task_pools.has_pending()
                    && !output_events.has_pending_output()
                {
                    break;
                }

                let ping_wait = v5_ping_wait_timeout(
                    last_activity,
                    outstanding_ping.as_ref().map(|(_, sent_at)| *sent_at),
                    idle_ping_interval,
                    ping_timeout,
                );
                let event = if watches.has_active_watches() {
                    let timeout =
                        if active_workers > 0 || !requests.is_empty() || task_pools.has_pending() {
                            watches.next_poll_timeout().min(Duration::from_millis(10))
                        } else {
                            watches.next_poll_timeout()
                        }
                        .min(ping_wait);
                    match events_rx.recv_timeout(timeout) {
                        Ok(event) => Some(event),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            cancel_all_v5_service_work(
                                &mut requests,
                                &mut task_pools,
                                &active_cancellations,
                                &mut active_deadlines,
                                &mut canceled_streams,
                                &mut watches,
                            );
                            return Err(anyhow::anyhow!("v5 service event channel closed"));
                        }
                    }
                } else if active_workers > 0 || !requests.is_empty() || task_pools.has_pending() {
                    match events_rx.recv_timeout(Duration::from_millis(10).min(ping_wait)) {
                        Ok(event) => Some(event),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            cancel_all_v5_service_work(
                                &mut requests,
                                &mut task_pools,
                                &active_cancellations,
                                &mut active_deadlines,
                                &mut canceled_streams,
                                &mut watches,
                            );
                            return Err(anyhow::anyhow!("v5 service event channel closed"));
                        }
                    }
                } else {
                    match events_rx.recv_timeout(ping_wait) {
                        Ok(event) => Some(event),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            cancel_all_v5_service_work(
                                &mut requests,
                                &mut task_pools,
                                &active_cancellations,
                                &mut active_deadlines,
                                &mut canceled_streams,
                                &mut watches,
                            );
                            return Err(anyhow::anyhow!("v5 service event channel closed"));
                        }
                    }
                };
                let event = event.and_then(|event| match event {
                    V5ServeEvent::Inbound => {
                        inbound_events.clear_ready();
                        match inbound_rx.try_recv() {
                            Ok(inbound) => {
                                let _ = inbound_events.signal_ready();
                                Some(V5ServeLoopEvent::Inbound(inbound))
                            }
                            Err(mpsc::TryRecvError::Empty) => None,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                Some(V5ServeLoopEvent::Inbound(Ok(None)))
                            }
                        }
                    }
                    event => Some(V5ServeLoopEvent::Wake(event)),
                });

                if let Some(event) = event {
                    match event {
                        V5ServeLoopEvent::Inbound(Ok(Some(frame))) => {
                            let frame_type = frame.frame_type;
                            let frame_control = frame.control.clone();
                            let event = match session.receive_frame(frame) {
                                Ok(event) => event,
                                Err(error) => {
                                    cancel_all_v5_service_work(
                                        &mut requests,
                                        &mut task_pools,
                                        &active_cancellations,
                                        &mut active_deadlines,
                                        &mut canceled_streams,
                                        &mut watches,
                                    );
                                    return Err(error).context("failed to route v5 protocol frame");
                                }
                            };
                            last_activity = Instant::now();
                            let data_credit = event.data_credit();
                            if frame_type == protocol_v5::FrameType::Pong
                                && outstanding_ping
                                    .as_ref()
                                    .is_some_and(|(expected, _)| *expected == frame_control)
                            {
                                outstanding_ping = None;
                            }
                            let mut acknowledge_data = true;
                            if let Some(stream_event) = event.stream_event {
                                if let protocol_v5::StreamEvent::ResetStream { stream_id, .. } =
                                    &stream_event
                                {
                                    requests.remove(stream_id);
                                    task_pools.remove_pending(*stream_id);
                                    let was_active = active_streams.contains(stream_id);
                                    if was_active {
                                        if let Some(cancellation) =
                                            active_cancellations.get(stream_id)
                                        {
                                            cancellation.cancel();
                                        }
                                        active_deadlines.remove(stream_id);
                                        canceled_streams.insert(*stream_id);
                                    }
                                } else {
                                    match self.queue_v5_stream_event(
                                        &mut requests,
                                        &request_budget,
                                        stream_event,
                                    )? {
                                        V5QueuedStreamEvent::Pending => {}
                                        V5QueuedStreamEvent::Rejected {
                                            stream_id,
                                            code,
                                            diagnostic,
                                        } => {
                                            session
                                                .reset_stream(stream_id, code, diagnostic)
                                                .context(
                                                    "failed to reset rejected v5 request stream",
                                                )?;
                                            acknowledge_data = false;
                                        }
                                        V5QueuedStreamEvent::Complete { stream_id } => {
                                            let request = requests.remove(&stream_id).with_context(
                                                || {
                                                    format!(
                                                        "completed v5 request stream {stream_id} was not queued"
                                                    )
                                                },
                                            )?;
                                            if v5_deadline_expired(request.deadline_unix_ms) {
                                                session
                                                    .reset_stream(
                                                        stream_id,
                                                        protocol_v5::RESET_DEADLINE_EXCEEDED,
                                                        "request deadline expired",
                                                    )
                                                    .context(
                                                        "failed to reset expired v5 request stream",
                                                    )?;
                                                continue;
                                            }
                                            if let Some(should_shutdown) = self
                                                .handle_v5_control_request(
                                                    &mut session,
                                                    &mut watches,
                                                    stream_id,
                                                    &request,
                                                )?
                                            {
                                                shutdown |= should_shutdown;
                                            } else {
                                                if request.supersedes_stream_id != 0 {
                                                    let mut cancellation_state =
                                                        V5ServiceCancellationState {
                                                            task_pools: &mut task_pools,
                                                            active_cancellations:
                                                                &active_cancellations,
                                                            active_deadlines: &mut active_deadlines,
                                                            canceled_streams: &mut canceled_streams,
                                                        };
                                                    reset_v5_service_stream(
                                                        &mut session,
                                                        &mut requests,
                                                        &mut cancellation_state,
                                                        request.supersedes_stream_id,
                                                        protocol_v5::RESET_CANCELLED,
                                                        format!("superseded by stream {stream_id}"),
                                                    )?;
                                                }
                                                let deadline_unix_ms = request.deadline_unix_ms;
                                                if task_pools.can_start(&request) {
                                                    start_v5_service_worker!(stream_id, request);
                                                } else {
                                                    if deadline_unix_ms != 0 {
                                                        tracing::trace!(
                                                            stream_id,
                                                            deadline_unix_ms,
                                                            method = %request.method,
                                                            "Queueing v5 request behind bounded task pool"
                                                        );
                                                    }
                                                    task_pools.enqueue(stream_id, request);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if acknowledge_data && let Some((stream_id, credit_bytes)) = data_credit
                            {
                                session
                                    .acknowledge_data(stream_id, credit_bytes)
                                    .context("failed to queue v5 data window update")?;
                            }
                        }
                        V5ServeLoopEvent::Inbound(Ok(None)) => {
                            inbound_closed = true;
                            cancel_all_v5_service_work(
                                &mut requests,
                                &mut task_pools,
                                &active_cancellations,
                                &mut active_deadlines,
                                &mut canceled_streams,
                                &mut watches,
                            );
                        }
                        V5ServeLoopEvent::Inbound(Err(error)) => {
                            cancel_all_v5_service_work(
                                &mut requests,
                                &mut task_pools,
                                &active_cancellations,
                                &mut active_deadlines,
                                &mut canceled_streams,
                                &mut watches,
                            );
                            return Err(error).context("failed to read v5 protocol frame");
                        }
                        V5ServeLoopEvent::Wake(V5ServeEvent::Output) => {
                            output_events.clear_ready();
                            if session.queued_len() < V5_SERVE_SCHEDULER_BACKLOG_LIMIT {
                                match output_rx.try_recv() {
                                    Ok(output_event) => {
                                        let _ = output_events.signal_ready();
                                        apply_v5_output_event!(output_event);
                                    }
                                    Err(mpsc::TryRecvError::Empty) => {}
                                    Err(mpsc::TryRecvError::Disconnected) => {
                                        if active_workers > 0 {
                                            cancel_all_v5_service_work(
                                                &mut requests,
                                                &mut task_pools,
                                                &active_cancellations,
                                                &mut active_deadlines,
                                                &mut canceled_streams,
                                                &mut watches,
                                            );
                                            return Err(anyhow::anyhow!(
                                                "v5 service output event channel closed"
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        V5ServeLoopEvent::Wake(V5ServeEvent::NativeWatch) => {
                            native_watch_events.clear_ready();
                            for watch_id in native_watch_events.take_overflowed_watch_ids() {
                                watches.record_native_overflow(watch_id);
                            }
                            match native_watch_rx.try_recv() {
                                Ok(event) => {
                                    let _ = native_watch_events.signal_ready();
                                    watches.record_native_event(
                                        event.watch_id,
                                        event.result,
                                        &self.workspace_root,
                                    )?;
                                }
                                Err(mpsc::TryRecvError::Empty) => {}
                                Err(mpsc::TryRecvError::Disconnected) => {
                                    tracing::debug!(
                                        "Native v5 watch event queue closed; using polling fallback"
                                    );
                                }
                            }
                        }
                        V5ServeLoopEvent::Wake(V5ServeEvent::WorkerFinished {
                            stream_id,
                            terminal_queued,
                        }) => {
                            active_workers = active_workers.saturating_sub(1);
                            if let Some(task_class) = active_task_classes.remove(&stream_id) {
                                task_pools.mark_finished(task_class);
                            }
                            if !terminal_queued {
                                active_cancellations.remove(&stream_id);
                                active_deadlines.remove(&stream_id);
                                active_streams.remove(&stream_id);
                                canceled_streams.remove(&stream_id);
                            }
                        }
                        V5ServeLoopEvent::Wake(V5ServeEvent::Inbound) => {}
                    }
                }

                if inbound_closed {
                    continue;
                }

                if shutdown && shutdown_started.is_none() {
                    shutdown_started = Some(Instant::now());
                }
                if shutdown_started.is_some_and(|started| started.elapsed() >= shutdown_grace) {
                    cancel_all_v5_service_work(
                        &mut requests,
                        &mut task_pools,
                        &active_cancellations,
                        &mut active_deadlines,
                        &mut canceled_streams,
                        &mut watches,
                    );
                }

                if let Err(error) = expire_v5_service_deadlines(
                    &mut session,
                    &mut requests,
                    &mut task_pools,
                    &active_cancellations,
                    &mut active_deadlines,
                    &mut canceled_streams,
                ) {
                    cancel_all_v5_service_work(
                        &mut requests,
                        &mut task_pools,
                        &active_cancellations,
                        &mut active_deadlines,
                        &mut canceled_streams,
                        &mut watches,
                    );
                    return Err(error);
                }
                drain_v5_service_task_queue!();
                if let Err(error) = drive_v5_idle_ping(
                    &mut session,
                    &mut last_activity,
                    &mut outstanding_ping,
                    &mut next_ping_id,
                    idle_ping_interval,
                    ping_timeout,
                ) {
                    cancel_all_v5_service_work(
                        &mut requests,
                        &mut task_pools,
                        &active_cancellations,
                        &mut active_deadlines,
                        &mut canceled_streams,
                        &mut watches,
                    );
                    return Err(error);
                }
                if let Err(error) = self.poll_v5_watches(&mut session, &mut watches) {
                    cancel_all_v5_service_work(
                        &mut requests,
                        &mut task_pools,
                        &active_cancellations,
                        &mut active_deadlines,
                        &mut canceled_streams,
                        &mut watches,
                    );
                    return Err(error);
                }
                if let Err(error) = self.drain_v5_session_writer(
                    &mut session,
                    &mut writer,
                    limits,
                    &mut next_frame_sequence,
                ) {
                    cancel_all_v5_service_work(
                        &mut requests,
                        &mut task_pools,
                        &active_cancellations,
                        &mut active_deadlines,
                        &mut canceled_streams,
                        &mut watches,
                    );
                    return Err(error);
                }
                if session.queued_len() < V5_SERVE_SCHEDULER_BACKLOG_LIMIT
                    && output_events.has_pending_output()
                {
                    let _ = output_events.signal_ready();
                }
            }

            self.drain_v5_session_writer(
                &mut session,
                &mut writer,
                limits,
                &mut next_frame_sequence,
            )
        })
    }

    fn handle_v5_stream_event(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        requests: &mut HashMap<u64, V5ServiceRequest>,
        request_budget: &V5ConnectionByteBudget,
        watches: &mut V5WatchRegistry,
        event: protocol_v5::StreamEvent,
    ) -> Result<(bool, bool)> {
        let (stream_id, request) =
            match self.queue_v5_stream_event(requests, request_budget, event)? {
                V5QueuedStreamEvent::Pending => return Ok((false, true)),
                V5QueuedStreamEvent::Rejected {
                    stream_id,
                    code,
                    diagnostic,
                } => {
                    session
                        .reset_stream(stream_id, code, diagnostic)
                        .context("failed to reset rejected v5 request stream")?;
                    return Ok((false, false));
                }
                V5QueuedStreamEvent::Complete { stream_id } => {
                    let request = requests.remove(&stream_id).with_context(|| {
                        format!("completed v5 request stream {stream_id} was not queued")
                    })?;
                    (stream_id, request)
                }
            };
        if v5_deadline_expired(request.deadline_unix_ms) {
            session
                .reset_stream(
                    stream_id,
                    protocol_v5::RESET_DEADLINE_EXCEEDED,
                    "request deadline expired",
                )
                .context("failed to reset expired v5 request stream")?;
            return Ok((false, true));
        }
        if let Some(should_shutdown) =
            self.handle_v5_control_request(session, watches, stream_id, &request)?
        {
            return Ok((should_shutdown, true));
        }
        self.complete_v5_request(session, stream_id, request)
            .map(|shutdown| (shutdown, true))
    }

    fn queue_v5_stream_event(
        &self,
        requests: &mut HashMap<u64, V5ServiceRequest>,
        request_budget: &V5ConnectionByteBudget,
        event: protocol_v5::StreamEvent,
    ) -> Result<V5QueuedStreamEvent> {
        match event {
            protocol_v5::StreamEvent::Headers {
                stream_id,
                role: protocol_v5::MessageRole::Request,
                priority,
                envelope,
            } => {
                requests.insert(
                    stream_id,
                    V5ServiceRequest::from_envelope(envelope, priority, request_budget),
                );
                Ok(V5QueuedStreamEvent::Pending)
            }
            protocol_v5::StreamEvent::Data {
                stream_id,
                channel,
                body,
                ..
            } => {
                let request = requests
                    .get_mut(&stream_id)
                    .with_context(|| format!("received v5 DATA for unknown stream {stream_id}"))?;
                if let Some(error) = self.append_v5_request_data(request, channel, body) {
                    requests.remove(&stream_id);
                    let code = if error.code == "resource_exhausted" {
                        protocol_v5::RESET_RESOURCE_EXHAUSTED
                    } else {
                        protocol_v5::RESET_UNAVAILABLE
                    };
                    return Ok(V5QueuedStreamEvent::Rejected {
                        stream_id,
                        code,
                        diagnostic: error.message,
                    });
                }
                Ok(V5QueuedStreamEvent::Pending)
            }
            protocol_v5::StreamEvent::EndStream { stream_id } => {
                if !requests.contains_key(&stream_id) {
                    return Ok(V5QueuedStreamEvent::Pending);
                }
                Ok(V5QueuedStreamEvent::Complete { stream_id })
            }
            protocol_v5::StreamEvent::ResetStream { stream_id, .. } => {
                requests.remove(&stream_id);
                Ok(V5QueuedStreamEvent::Pending)
            }
            protocol_v5::StreamEvent::Headers { .. } => Ok(V5QueuedStreamEvent::Pending),
        }
    }

    fn append_v5_request_data(
        &self,
        request: &mut V5ServiceRequest,
        channel: protocol_v5::DataChannel,
        body: Vec<u8>,
    ) -> Option<RemoteError> {
        if request.early_error.is_some() {
            return None;
        }
        let streamed_file_body = request.method == "fs.write"
            && channel == protocol_v5::DataChannel::FileBody
            && matches!(self.backend.identity(), WorkspaceIdentity::Local);
        if let Err(error) = request.reserve_data(channel, body.len(), !streamed_file_body) {
            request.streamed_write = None;
            return Some(error);
        }
        if streamed_file_body {
            if let Err(error) = self.append_v5_streaming_write_data(request, &body) {
                request.streamed_write = None;
                return Some(error);
            }
        } else {
            request.append_data(channel, body);
        }
        None
    }

    fn append_v5_streaming_write_data(
        &self,
        request: &mut V5ServiceRequest,
        body: &[u8],
    ) -> std::result::Result<(), RemoteError> {
        if request.streamed_write.is_none() {
            let payload: V5WriteFilePayload = decode_v5_payload(&request.method, &request.payload)
                .map_err(v5_method_error_to_remote_error)?;
            let path = self.resolve_path(&payload.path)?;
            let expected_modified = system_time_from_unix_millis_and_nanos(
                payload.expected_modified_unix_millis,
                payload.expected_modified_unix_nanos,
            );
            let streamed_write =
                V5StreamingWrite::create(path, payload.create_parent_dirs, expected_modified)
                    .map_err(remote_error_from_workspace)?;
            request.streamed_write = Some(streamed_write);
        }

        let Some(streamed_write) = request.streamed_write.as_mut() else {
            return Err(RemoteError {
                code: "invalid_request".to_string(),
                message: "fs.write stream did not create a temporary file".to_string(),
                diagnostic: None,
            });
        };
        streamed_write
            .write_chunk(body)
            .map_err(remote_error_from_workspace)
    }

    fn complete_v5_request(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        stream_id: u64,
        request: V5ServiceRequest,
    ) -> Result<bool> {
        let completion = self.execute_v5_request(stream_id, request, None, None);
        self.apply_v5_completion(session, completion)
    }

    fn handle_v5_control_request(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        watches: &mut V5WatchRegistry,
        stream_id: u64,
        request: &V5ServiceRequest,
    ) -> Result<Option<bool>> {
        match request.method.as_str() {
            "watch.start" => {
                self.handle_v5_watch_start(session, watches, stream_id, request)?;
                Ok(Some(false))
            }
            "watch.update" => {
                self.handle_v5_watch_update(session, watches, stream_id, request)?;
                Ok(Some(false))
            }
            "watch.stop" => {
                self.handle_v5_watch_stop(session, watches, stream_id, request)?;
                Ok(Some(false))
            }
            "watch.resync" => {
                self.handle_v5_watch_resync(session, watches, stream_id, request)?;
                Ok(Some(false))
            }
            _ => Ok(None),
        }
    }

    fn handle_v5_watch_start(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        watches: &mut V5WatchRegistry,
        stream_id: u64,
        request: &V5ServiceRequest,
    ) -> Result<()> {
        let start: protocol_v5::WatchStart =
            match decode_v5_protobuf_payload(&request.method, &request.payload) {
                Ok(start) => start,
                Err(error) => {
                    self.send_v5_remote_error(
                        session,
                        stream_id,
                        &request.method,
                        v5_method_error_to_remote_error(error),
                    )?;
                    return Ok(());
                }
            };
        if let Err(error) = validate_v5_watch_start(&start) {
            self.send_v5_remote_error(
                session,
                stream_id,
                &request.method,
                v5_method_error_to_remote_error(error),
            )?;
            return Ok(());
        }
        let roots = if start.roots.is_empty() {
            vec![".".to_string()]
        } else {
            start.roots.clone()
        };
        let (accepted_roots, unsupported_roots) = self.classify_v5_watch_roots(&roots);
        if accepted_roots.is_empty() {
            self.send_v5_remote_error(
                session,
                stream_id,
                &request.method,
                RemoteError {
                    code: "invalid_argument".to_string(),
                    message: "watch.start did not include any workspace-contained roots"
                        .to_string(),
                    diagnostic: (!unsupported_roots.is_empty())
                        .then(|| format!("unsupported roots: {}", unsupported_roots.join(", "))),
                },
            )?;
            return Ok(());
        }

        let watch_id = watches.allocate_watch_id()?;
        let event_stream_id = session
            .open_event_stream("watch.batch", watch_id)
            .context("failed to open v5 watch event stream")?;
        let watch_status = watches.start(
            watch_id,
            event_stream_id,
            accepted_roots.clone(),
            start.debounce_ms,
            start.max_events_per_batch,
            &self.workspace_root,
        );

        let response = protocol_v5::WatchStartResponse {
            watch_id,
            event_stream_id,
            backend: watch_status.backend,
            recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
            degraded: watch_status.degraded,
            requires_reconciliation: true,
            accepted_roots: watch_status.accepted_roots,
            degraded_roots: watch_status.degraded_roots,
            unsupported_roots,
        };
        self.send_v5_protobuf_response(session, stream_id, &request.method, &response)
    }

    fn handle_v5_watch_update(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        watches: &mut V5WatchRegistry,
        stream_id: u64,
        request: &V5ServiceRequest,
    ) -> Result<()> {
        let update: protocol_v5::WatchUpdate =
            match decode_v5_protobuf_payload(&request.method, &request.payload) {
                Ok(update) => update,
                Err(error) => {
                    self.send_v5_remote_error(
                        session,
                        stream_id,
                        &request.method,
                        v5_method_error_to_remote_error(error),
                    )?;
                    return Ok(());
                }
            };
        let (accepted_adds, unsupported_roots) = self.classify_v5_watch_roots(&update.add_roots);
        let removed_roots = update
            .remove_roots
            .iter()
            .filter_map(|root| self.normalize_v5_watch_root(root))
            .collect::<Vec<_>>();
        let update_status = match watches.update(
            update.watch_id,
            accepted_adds,
            removed_roots,
            &self.workspace_root,
        ) {
            Ok(accepted_roots) => accepted_roots,
            Err(error) => {
                self.send_v5_remote_error(session, stream_id, &request.method, error)?;
                return Ok(());
            }
        };
        let response = protocol_v5::WatchUpdateResponse {
            watch_id: update.watch_id,
            accepted_roots: update_status.accepted_roots,
            degraded_roots: update_status.degraded_roots,
            unsupported_roots,
        };
        self.send_v5_protobuf_response(session, stream_id, &request.method, &response)
    }

    fn handle_v5_watch_resync(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        watches: &mut V5WatchRegistry,
        stream_id: u64,
        request: &V5ServiceRequest,
    ) -> Result<()> {
        let resync: protocol_v5::WatchResync =
            match decode_v5_protobuf_payload(&request.method, &request.payload) {
                Ok(resync) => resync,
                Err(error) => {
                    self.send_v5_remote_error(
                        session,
                        stream_id,
                        &request.method,
                        v5_method_error_to_remote_error(error),
                    )?;
                    return Ok(());
                }
            };

        let (accepted_roots, mut unsupported_roots) = if resync.roots.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            self.classify_v5_watch_roots(&resync.roots)
        };
        let requested_roots = (!resync.roots.is_empty()).then_some(accepted_roots);
        let resync_status = match watches.resync(resync.watch_id, requested_roots) {
            Ok(resync_status) => resync_status,
            Err(error) => {
                self.send_v5_remote_error(session, stream_id, &request.method, error)?;
                return Ok(());
            }
        };
        unsupported_roots.extend(resync_status.unsupported_roots);
        unsupported_roots.sort();
        unsupported_roots.dedup();
        let response = protocol_v5::WatchResyncResponse {
            watch_id: resync.watch_id,
            accepted_roots: resync_status.accepted_roots,
            unsupported_roots,
        };
        self.send_v5_protobuf_response(session, stream_id, &request.method, &response)
    }

    fn handle_v5_watch_stop(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        watches: &mut V5WatchRegistry,
        stream_id: u64,
        request: &V5ServiceRequest,
    ) -> Result<()> {
        let stop: protocol_v5::WatchStop =
            match decode_v5_protobuf_payload(&request.method, &request.payload) {
                Ok(stop) => stop,
                Err(error) => {
                    self.send_v5_remote_error(
                        session,
                        stream_id,
                        &request.method,
                        v5_method_error_to_remote_error(error),
                    )?;
                    return Ok(());
                }
            };
        if let Some(subscription) = watches.stop(stop.watch_id) {
            session
                .finish_stream(
                    subscription.event_stream_id,
                    protocol_v5::Priority::VisibleFileTree,
                )
                .context("failed to close v5 watch event stream")?;
        }
        self.send_v5_raw_response(session, stream_id, &request.method, Vec::new())
    }

    fn poll_v5_watches(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        watches: &mut V5WatchRegistry,
    ) -> Result<()> {
        for (event_stream_id, batch) in watches.poll_due(&self.workspace_root)? {
            session
                .enqueue_watch_batch(event_stream_id, batch)
                .context("failed to queue v5 watch batch")?;
        }
        Ok(())
    }

    fn execute_v5_request(
        &self,
        stream_id: u64,
        mut request: V5ServiceRequest,
        stream_events: Option<V5ServeOutputSender>,
        cancellation: Option<WorkspaceCancellationToken>,
    ) -> V5ServiceCompletion {
        let method = request.method.clone();
        let cancellation = cancellation.unwrap_or_default();
        if let Some(error) = request.early_error.take() {
            return V5ServiceCompletion {
                stream_id,
                method,
                result: Err(error),
            };
        }
        if let Err(error) = cancellation
            .check_cancelled("execute remote request", &self.workspace_root)
            .map_err(remote_error_from_workspace)
        {
            return V5ServiceCompletion {
                stream_id,
                method,
                result: Err(error),
            };
        }
        if method == "fs.list_dir" {
            return V5ServiceCompletion {
                stream_id,
                method,
                result: self.execute_v5_list_dir_request(&request, &cancellation),
            };
        }
        if method == "fs.list_dirs" {
            return V5ServiceCompletion {
                stream_id,
                method,
                result: self.execute_v5_list_dirs_request(&request, &cancellation),
            };
        }
        if method == "fs.write" && matches!(self.backend.identity(), WorkspaceIdentity::Local) {
            if request.streamed_write.is_none()
                && let Err(error) = self.append_v5_streaming_write_data(&mut request, &[])
            {
                return V5ServiceCompletion {
                    stream_id,
                    method,
                    result: Err(error),
                };
            }
            return V5ServiceCompletion {
                stream_id,
                method,
                result: self.execute_v5_streaming_write_request(request, &cancellation),
            };
        }
        if let Some(stream_events) = stream_events
            && matches!(self.backend.identity(), WorkspaceIdentity::Local)
        {
            match method.as_str() {
                "fs.read" => {
                    return V5ServiceCompletion {
                        stream_id,
                        method,
                        result: self.execute_v5_streaming_read_request(
                            stream_id,
                            &request,
                            stream_events,
                            &cancellation,
                        ),
                    };
                }
                "process.run" => {
                    return V5ServiceCompletion {
                        stream_id,
                        method,
                        result: self.execute_v5_streaming_process_request(
                            stream_id,
                            &request,
                            stream_events,
                            Some(cancellation.clone()),
                        ),
                    };
                }
                "search.files" => {
                    return V5ServiceCompletion {
                        stream_id,
                        method,
                        result: self.execute_v5_streaming_file_search_request(
                            stream_id,
                            &request,
                            stream_events,
                            Some(cancellation.clone()),
                        ),
                    };
                }
                "search.text" => {
                    return V5ServiceCompletion {
                        stream_id,
                        method,
                        result: self.execute_v5_streaming_text_search_request(
                            stream_id,
                            &request,
                            stream_events,
                            Some(cancellation.clone()),
                        ),
                    };
                }
                _ => {}
            }
        }
        let remote_request = match RemoteRequest::from_v5_method_payload(&method, &request.payload)
        {
            Ok(request) => request,
            Err(error) => {
                return V5ServiceCompletion {
                    stream_id,
                    method,
                    result: Err(RemoteError {
                        code: "invalid_request".to_string(),
                        message: error.to_string(),
                        diagnostic: None,
                    }),
                };
            }
        };

        if matches!(self.backend.identity(), WorkspaceIdentity::Local) {
            match &remote_request {
                RemoteRequest::ProjectEnvironment { root } => {
                    return V5ServiceCompletion {
                        stream_id,
                        method,
                        result: self
                            .execute_v5_project_environment_request(root, Some(&cancellation)),
                    };
                }
                RemoteRequest::GitHead { root } => {
                    return V5ServiceCompletion {
                        stream_id,
                        method,
                        result: self.execute_v5_git_head_request(root, Some(cancellation.clone())),
                    };
                }
                RemoteRequest::GitStatus {
                    root,
                    include_untracked,
                    limit,
                } => {
                    return V5ServiceCompletion {
                        stream_id,
                        method,
                        result: self.execute_v5_git_status_request(
                            root,
                            GitStatusOptions {
                                include_untracked: *include_untracked,
                                limit: *limit,
                            },
                            Some(cancellation.clone()),
                        ),
                    };
                }
                _ => {}
            }
        }

        V5ServiceCompletion {
            stream_id,
            method,
            result: self.execute(remote_request, request.body, &cancellation),
        }
    }

    fn process_spec_from_request(
        &self,
        request: ProcessRequest,
        request_body: Vec<u8>,
        cancellation: Option<&WorkspaceCancellationToken>,
    ) -> std::result::Result<ProcessSpec, RemoteError> {
        let cwd = self.resolve_path(&request.cwd)?;
        let max_output_bytes = Some(v5_process_output_limit(request.max_output_bytes));
        let env = if request.inherit_project_environment {
            let environment_root = self.project_environment_root_for_process(&cwd);
            let mut project_environment = self
                .load_project_environment_with_cancellation(&environment_root, cancellation)
                .map_err(remote_error_from_environment)?
                .variables;
            project_environment.extend(request.env);
            project_environment
        } else {
            request.env
        };

        Ok(ProcessSpec {
            program: request.program,
            args: request.args,
            cwd,
            env,
            clear_env: request.clear_env,
            inherit_project_environment: false,
            stdin: request_body,
            max_output_bytes,
            timeout_ms: request.timeout_ms,
        })
    }

    fn execute_v5_list_dir_request(
        &self,
        request: &V5ServiceRequest,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let payload: V5DirectoryListPayload = decode_v5_payload(&request.method, &request.payload)
            .map_err(v5_method_error_to_remote_error)?;
        let path = self.resolve_path(&payload.path)?;
        let listing = block_on(self.backend.list_dir_with_cancellation(&path, cancellation))
            .map_err(remote_error_from_workspace)?;
        let listing = annotate_directory_listing_ignored_with_cancellation(
            listing,
            &self.workspace_root,
            self.ignore_matcher.as_ref(),
            cancellation,
        )
        .map_err(remote_error_from_workspace)?;
        cancellation
            .check_cancelled("prepare directory listing response", &path)
            .map_err(remote_error_from_workspace)?;
        let response = self.cached_directory_listing_response(
            &path,
            directory_listing_response_with_cancellation(listing, cancellation)
                .map_err(remote_error_from_workspace)?,
            payload.known_generation,
            payload.known_fingerprint,
            cancellation,
        )?;
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::ListDir(response),
            Vec::new(),
        ))
    }

    fn cached_directory_listing_response(
        &self,
        cache_key: &Path,
        response: DirectoryListingResponse,
        known_generation: Option<u64>,
        known_fingerprint: Option<u64>,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<DirectoryListingResponse, RemoteError> {
        cancellation
            .check_cancelled("cache directory listing", cache_key)
            .map_err(remote_error_from_workspace)?;
        let current = response.clone();
        let mut response = directory_listing_response_for_known_state(
            response,
            known_generation,
            known_fingerprint,
        );
        if !response.not_modified
            && response.delta.is_none()
            && let Ok(mut cache) = self.directory_delta_cache.lock()
        {
            cancellation
                .check_cancelled("cache directory listing", cache_key)
                .map_err(remote_error_from_workspace)?;
            if let Some(previous) = cache.get(cache_key) {
                response = directory_listing_delta_response_for_known_state(
                    response,
                    previous,
                    known_generation,
                    known_fingerprint,
                );
            }
            if cache.len() >= V5_DIRECTORY_DELTA_CACHE_LIMIT
                && !cache.contains_key(cache_key)
                && let Some(first_key) = cache.keys().next().cloned()
            {
                cache.remove(&first_key);
            }
            cache.insert(cache_key.to_path_buf(), current);
        }
        cancellation
            .check_cancelled("cache directory listing", cache_key)
            .map_err(remote_error_from_workspace)?;
        Ok(response)
    }

    fn execute_v5_list_dirs_request(
        &self,
        request: &V5ServiceRequest,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let payload: V5DirectoryListDirsPayload =
            decode_v5_payload(&request.method, &request.payload)
                .map_err(v5_method_error_to_remote_error)?;
        let entries = if payload.entries.is_empty() {
            payload
                .paths
                .into_iter()
                .map(|path| V5DirectoryListEntryPayload {
                    path,
                    known_generation: None,
                    known_fingerprint: None,
                })
                .collect::<Vec<_>>()
        } else {
            payload.entries
        };

        let mut results = Vec::with_capacity(entries.len());
        for entry in entries {
            let display_path = entry.path;
            cancellation
                .check_cancelled("list directories", &display_path)
                .map_err(remote_error_from_workspace)?;
            let result = match self.resolve_path(&display_path) {
                Ok(path) => {
                    match block_on(self.backend.list_dir_with_cancellation(&path, cancellation)) {
                        Ok(listing) => {
                            let listing = annotate_directory_listing_ignored_with_cancellation(
                                listing,
                                &self.workspace_root,
                                self.ignore_matcher.as_ref(),
                                cancellation,
                            )
                            .map_err(remote_error_from_workspace)?;
                            let listing = self.cached_directory_listing_response(
                                &path,
                                directory_listing_response_with_cancellation(listing, cancellation)
                                    .map_err(remote_error_from_workspace)?,
                                entry.known_generation,
                                entry.known_fingerprint,
                                cancellation,
                            )?;
                            DirectoryListingResultResponse {
                                path: display_path,
                                listing: Some(listing),
                                error: None,
                            }
                        }
                        Err(error @ WorkspaceError::Cancelled { .. }) => {
                            return Err(remote_error_from_workspace(error));
                        }
                        Err(error) => {
                            cancellation
                                .check_cancelled("list directories", &path)
                                .map_err(remote_error_from_workspace)?;
                            DirectoryListingResultResponse {
                                path: display_path,
                                listing: None,
                                error: Some(remote_error_from_workspace(error)),
                            }
                        }
                    }
                }
                Err(error) => DirectoryListingResultResponse {
                    path: display_path,
                    listing: None,
                    error: Some(error),
                },
            };
            cancellation
                .check_cancelled("list directories", &self.workspace_root)
                .map_err(remote_error_from_workspace)?;
            results.push(result);
        }

        Ok(ServiceOutcome::continue_response(
            RemoteResponse::ListDirs(ListDirsResponse { results }),
            Vec::new(),
        ))
    }

    fn execute_v5_project_environment_request(
        &self,
        root: &Path,
        cancellation: Option<&WorkspaceCancellationToken>,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let root = self.resolve_search_root(root)?;
        let snapshot = self
            .load_project_environment_with_cancellation(&root, cancellation)
            .map_err(remote_error_from_environment)?;
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::ProjectEnvironment(project_environment_response(snapshot)),
            Vec::new(),
        ))
    }

    fn execute_v5_git_head_request(
        &self,
        root: &Path,
        cancellation: Option<WorkspaceCancellationToken>,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let root = self.resolve_search_root(root)?;
        let result =
            v5_local_git_head(&root, cancellation.as_ref()).map_err(remote_error_from_workspace)?;
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::GitHead(git_head_response(result)),
            Vec::new(),
        ))
    }

    fn execute_v5_git_status_request(
        &self,
        root: &Path,
        options: GitStatusOptions,
        cancellation: Option<WorkspaceCancellationToken>,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let root = self.resolve_search_root(root)?;
        let result = v5_local_git_status(&root, options, cancellation.as_ref())
            .map_err(remote_error_from_workspace)?;
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::GitStatus(git_status_response(result)),
            Vec::new(),
        ))
    }

    fn execute_v5_streaming_read_request(
        &self,
        stream_id: u64,
        request: &V5ServiceRequest,
        stream_events: V5ServeOutputSender,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let payload: V5ReadFilePayload = decode_v5_payload(&request.method, &request.payload)
            .map_err(v5_method_error_to_remote_error)?;
        let path = self.resolve_read_path(&payload.path)?;
        cancellation
            .check_cancelled("read file", &path)
            .map_err(remote_error_from_workspace)?;
        let metadata = std::fs::metadata(&path).map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "stat file",
                path: path.clone(),
                source,
            })
        })?;
        cancellation
            .check_cancelled("read file", &path)
            .map_err(remote_error_from_workspace)?;
        if !metadata.is_file() {
            return Err(remote_error_from_workspace(WorkspaceError::NotFile {
                path: path.clone(),
            }));
        }

        let size = metadata.len();
        let read_len = payload
            .max_bytes
            .unwrap_or(MAX_FRAME_BODY_LEN)
            .min(MAX_FRAME_BODY_LEN)
            .min(size);
        let mut file = std::fs::File::open(&path).map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "open file",
                path: path.clone(),
                source,
            })
        })?;
        cancellation
            .check_cancelled("read file", &path)
            .map_err(remote_error_from_workspace)?;
        v5_stream_file_chunks(&mut file, read_len, &path, cancellation, |body| {
            stream_events
                .send_with_cancellation(
                    V5ServeOutputEvent::StreamData {
                        stream_id,
                        channel: protocol_v5::DataChannel::FileBody,
                        body,
                        priority: request.priority,
                    },
                    cancellation,
                )
                .map_err(v5_queue_error_to_remote_error)
        })?;

        let read = FileRead {
            path,
            bytes: Vec::new(),
            size,
            modified: metadata.modified().ok(),
            readonly: metadata.permissions().readonly(),
            truncated: read_len < size,
        };
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::ReadFile(file_read_response(&read)),
            Vec::new(),
        ))
    }

    fn execute_v5_streaming_write_request(
        &self,
        mut request: V5ServiceRequest,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let streamed_write = request.streamed_write.take().ok_or_else(|| RemoteError {
            code: "invalid_request".to_string(),
            message: "fs.write stream did not include a temporary file".to_string(),
            diagnostic: None,
        })?;
        let result = streamed_write
            .finish(Some(cancellation))
            .map_err(remote_error_from_workspace)?;
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::WriteFile(write_result_response(result)),
            Vec::new(),
        ))
    }

    fn execute_v5_streaming_process_request(
        &self,
        stream_id: u64,
        request: &V5ServiceRequest,
        stream_events: V5ServeOutputSender,
        cancellation: Option<WorkspaceCancellationToken>,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let payload: ProcessRequest = decode_v5_payload(&request.method, &request.payload)
            .map_err(v5_method_error_to_remote_error)?;
        let spec =
            self.process_spec_from_request(payload, request.body.clone(), cancellation.as_ref())?;
        let output = v5_run_local_streaming_process(
            spec,
            stream_id,
            request.priority,
            stream_events,
            cancellation,
        )
        .map_err(remote_error_from_workspace)?;
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::RunProcess(v5_streamed_process_output_response(&output)),
            Vec::new(),
        ))
    }

    fn execute_v5_streaming_file_search_request(
        &self,
        stream_id: u64,
        request: &V5ServiceRequest,
        stream_events: V5ServeOutputSender,
        cancellation: Option<WorkspaceCancellationToken>,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let priority = request.priority;
        let request: FileSearchRequest = decode_v5_payload(&request.method, &request.payload)
            .map_err(v5_method_error_to_remote_error)?;
        let query = FileSearchQuery {
            root: self.resolve_search_root(&request.root)?,
            pattern: request.pattern,
            limit: request.limit,
            hidden: request.hidden,
            parents: request.parents,
            ignore: request.ignore,
            git_ignore: request.git_ignore,
            git_global: request.git_global,
            git_exclude: request.git_exclude,
            follow_links: request.follow_links,
            max_depth: request.max_depth,
            excluded_relative_prefixes: request.excluded_relative_prefixes,
        };
        let result = v5_streaming_file_search(
            query,
            stream_id,
            priority,
            stream_events,
            cancellation.as_ref(),
        )?;
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::FileSearch(file_search_response(result)),
            Vec::new(),
        ))
    }

    fn execute_v5_streaming_text_search_request(
        &self,
        stream_id: u64,
        request: &V5ServiceRequest,
        stream_events: V5ServeOutputSender,
        cancellation: Option<WorkspaceCancellationToken>,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        let priority = request.priority;
        let request: TextSearchRequest = decode_v5_payload(&request.method, &request.payload)
            .map_err(v5_method_error_to_remote_error)?;
        let query = TextSearchQuery {
            root: self.resolve_search_root(&request.root)?,
            pattern: request.pattern,
            limit: request.limit,
            smart_case: request.smart_case,
            hidden: request.hidden,
            parents: request.parents,
            ignore: request.ignore,
            git_ignore: request.git_ignore,
            git_global: request.git_global,
            git_exclude: request.git_exclude,
            follow_links: request.follow_links,
            max_depth: request.max_depth,
            max_file_bytes: request.max_file_bytes,
            excluded_relative_paths: request.excluded_relative_paths,
            custom_ignore_filenames: request.custom_ignore_filenames,
        };
        let result = v5_streaming_text_search(
            query,
            stream_id,
            priority,
            stream_events,
            cancellation.as_ref(),
        )?;
        Ok(ServiceOutcome::continue_response(
            RemoteResponse::TextSearch(text_search_response(result)),
            Vec::new(),
        ))
    }

    fn apply_v5_completion(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        completion: V5ServiceCompletion,
    ) -> Result<bool> {
        match completion.result {
            Ok(ServiceOutcome::Continue { response, body }) => {
                self.send_v5_response(
                    session,
                    completion.stream_id,
                    &completion.method,
                    *response,
                    body,
                )?;
                Ok(false)
            }
            Ok(ServiceOutcome::Shutdown) => {
                self.send_v5_response(
                    session,
                    completion.stream_id,
                    &completion.method,
                    RemoteResponse::Shutdown,
                    Vec::new(),
                )?;
                session
                    .send_goaway("OK", "session shutdown")
                    .context("failed to queue v5 goaway after shutdown")?;
                Ok(true)
            }
            Err(error) => {
                self.send_v5_remote_error(
                    session,
                    completion.stream_id,
                    &completion.method,
                    error,
                )?;
                Ok(false)
            }
        }
    }

    fn enqueue_v5_service_completion(
        &self,
        completion: V5ServiceCompletion,
        priority: protocol_v5::Priority,
        output_events: &V5ServeOutputSender,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<bool, V5ServeQueueError> {
        let V5ServiceCompletion {
            stream_id,
            method,
            result,
        } = completion;
        if cancellation.is_cancelled() {
            return Ok(false);
        }
        let terminal_result = match result {
            Ok(ServiceOutcome::Continue { response, body }) => {
                match self.enqueue_v5_service_response(
                    output_events,
                    stream_id,
                    *response,
                    body,
                    priority,
                    cancellation,
                ) {
                    Ok(()) => Ok(V5ServiceTerminalOutcome::Continue),
                    Err(error) => Err(error),
                }
            }
            Ok(ServiceOutcome::Shutdown) => self
                .enqueue_v5_service_response(
                    output_events,
                    stream_id,
                    RemoteResponse::Shutdown,
                    Vec::new(),
                    priority,
                    cancellation,
                )
                .map(|()| V5ServiceTerminalOutcome::Shutdown),
            Err(error) => Err(error),
        };
        if cancellation.is_cancelled() {
            return Ok(false);
        }
        match output_events.send_with_cancellation(
            V5ServeOutputEvent::Completed(V5ServiceTerminal {
                stream_id,
                method: v5_bounded_terminal_string(method, 256),
                result: terminal_result.map_err(v5_bound_terminal_error),
            }),
            cancellation,
        ) {
            Ok(()) => Ok(true),
            Err(V5ServeQueueError::Cancelled) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn enqueue_v5_service_response(
        &self,
        output_events: &V5ServeOutputSender,
        stream_id: u64,
        response: RemoteResponse,
        body: Vec<u8>,
        priority: protocol_v5::Priority,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<(), RemoteError> {
        let payload_bytes = v5_serialized_response_len(&response, cancellation)?;
        let retained_bytes = payload_bytes
            .checked_add(body.capacity())
            .ok_or_else(v5_response_size_overflow_error)?;
        let _reservation = output_events.reserve_completion_bytes(retained_bytes)?;

        v5_serialize_response_to_output(
            output_events,
            stream_id,
            &response,
            priority,
            cancellation,
        )?;
        self.enqueue_v5_service_body(
            output_events,
            stream_id,
            &response,
            body,
            priority,
            cancellation,
        )
    }

    fn enqueue_v5_service_body(
        &self,
        output_events: &V5ServeOutputSender,
        stream_id: u64,
        response: &RemoteResponse,
        body: Vec<u8>,
        priority: protocol_v5::Priority,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<(), RemoteError> {
        if body.is_empty() {
            return Ok(());
        }

        match response {
            RemoteResponse::ReadFile(_) => self
                .enqueue_v5_service_data(
                    output_events,
                    stream_id,
                    protocol_v5::DataChannel::FileBody,
                    &body,
                    priority,
                    cancellation,
                )
                .map_err(v5_queue_error_to_remote_error),
            RemoteResponse::RunProcess(process) => {
                let total_len = process
                    .stdout_len
                    .checked_add(process.stderr_len)
                    .ok_or_else(v5_response_size_overflow_error)?;
                if total_len > body.len() {
                    return Err(RemoteError {
                        code: "invalid_response".to_string(),
                        message: "process output body is shorter than declared lengths".to_string(),
                        diagnostic: Some(format!(
                            "stdout_len={} stderr_len={} body_len={}",
                            process.stdout_len,
                            process.stderr_len,
                            body.len()
                        )),
                    });
                }
                if process.stdout_len != 0 {
                    self.enqueue_v5_service_data(
                        output_events,
                        stream_id,
                        protocol_v5::DataChannel::Stdout,
                        &body[..process.stdout_len],
                        priority,
                        cancellation,
                    )
                    .map_err(v5_queue_error_to_remote_error)?;
                }
                if process.stderr_len != 0 {
                    self.enqueue_v5_service_data(
                        output_events,
                        stream_id,
                        protocol_v5::DataChannel::Stderr,
                        &body[process.stdout_len..total_len],
                        priority,
                        cancellation,
                    )
                    .map_err(v5_queue_error_to_remote_error)?;
                }
                Ok(())
            }
            _ => self
                .enqueue_v5_service_data(
                    output_events,
                    stream_id,
                    protocol_v5::DataChannel::Unspecified,
                    &body,
                    priority,
                    cancellation,
                )
                .map_err(v5_queue_error_to_remote_error),
        }
    }

    fn enqueue_v5_service_data(
        &self,
        output_events: &V5ServeOutputSender,
        stream_id: u64,
        channel: protocol_v5::DataChannel,
        body: &[u8],
        priority: protocol_v5::Priority,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<(), V5ServeQueueError> {
        for chunk in body.chunks(V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES) {
            output_events.send_with_cancellation(
                V5ServeOutputEvent::StreamData {
                    stream_id,
                    channel,
                    body: chunk.to_vec(),
                    priority,
                },
                cancellation,
            )?;
        }
        Ok(())
    }

    fn apply_v5_service_terminal(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        completion: V5ServiceTerminal,
    ) -> Result<bool> {
        match completion.result {
            Ok(outcome) => {
                let priority = session
                    .stream_priority(completion.stream_id)
                    .unwrap_or(protocol_v5::Priority::Background);
                session
                    .send_response_with_priority(
                        completion.stream_id,
                        completion.method,
                        protocol_v5::MessageRole::FinalResponse,
                        true,
                        priority,
                    )
                    .context("failed to queue v5 final response")?;
                session
                    .finish_stream(completion.stream_id, priority)
                    .context("failed to queue v5 end stream")?;
                if matches!(outcome, V5ServiceTerminalOutcome::Shutdown) {
                    session
                        .send_goaway("OK", "session shutdown")
                        .context("failed to queue v5 goaway after shutdown")?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Err(error) => {
                self.send_v5_remote_error(
                    session,
                    completion.stream_id,
                    &completion.method,
                    error,
                )?;
                Ok(false)
            }
        }
    }

    fn send_v5_response(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        stream_id: u64,
        method: &str,
        response: RemoteResponse,
        body: Vec<u8>,
    ) -> Result<()> {
        let priority = session
            .stream_priority(stream_id)
            .unwrap_or(protocol_v5::Priority::Background);
        let payload = response
            .to_v5_payload()
            .context("failed to encode v5 response payload")?;
        session
            .send_owned_data(
                stream_id,
                protocol_v5::DataChannel::Unspecified,
                payload,
                priority,
            )
            .context("failed to queue v5 response payload")?;
        for (channel, body) in v5_response_body_chunks(&response, body).map_err(|error| {
            anyhow::anyhow!(
                "failed to split v5 response body: {}: {}{}",
                error.code,
                error.message,
                error
                    .diagnostic
                    .as_deref()
                    .map(|diagnostic| format!(" ({diagnostic})"))
                    .unwrap_or_default()
            )
        })? {
            session
                .send_owned_data(stream_id, channel, body, priority)
                .context("failed to queue v5 response body")?;
        }
        session
            .send_response_with_priority(
                stream_id,
                method,
                protocol_v5::MessageRole::FinalResponse,
                true,
                priority,
            )
            .context("failed to queue v5 final response")?;
        session
            .finish_stream(stream_id, priority)
            .context("failed to queue v5 end stream")?;
        Ok(())
    }

    fn send_v5_protobuf_response<M>(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        stream_id: u64,
        method: &str,
        response: &M,
    ) -> Result<()>
    where
        M: ProstMessage,
    {
        let payload = response.encode_to_vec();
        self.send_v5_raw_response(session, stream_id, method, payload)
    }

    fn send_v5_raw_response(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        stream_id: u64,
        method: &str,
        payload: Vec<u8>,
    ) -> Result<()> {
        let priority = session
            .stream_priority(stream_id)
            .unwrap_or(protocol_v5::Priority::VisibleFileTree);
        if !payload.is_empty() {
            session
                .send_owned_data(
                    stream_id,
                    protocol_v5::DataChannel::Unspecified,
                    payload,
                    priority,
                )
                .context("failed to queue v5 response payload")?;
        }
        session
            .send_response_with_priority(
                stream_id,
                method,
                protocol_v5::MessageRole::FinalResponse,
                true,
                priority,
            )
            .context("failed to queue v5 final response")?;
        session
            .finish_stream(stream_id, priority)
            .context("failed to queue v5 end stream")?;
        Ok(())
    }

    fn send_v5_remote_error(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        stream_id: u64,
        method: &str,
        error: RemoteError,
    ) -> Result<()> {
        let priority = session
            .stream_priority(stream_id)
            .unwrap_or(protocol_v5::Priority::Background);
        session
            .send_error_with_priority(
                stream_id,
                method,
                protocol_v5::ErrorHeader {
                    code: error.code,
                    message: error.message,
                    retryable: false,
                    details: error.diagnostic.unwrap_or_default(),
                    remote_errno: 0,
                },
                priority,
            )
            .context("failed to queue v5 error response")?;
        session
            .finish_stream(stream_id, priority)
            .context("failed to queue v5 error end stream")?;
        Ok(())
    }

    fn drain_v5_session<R: Read, W: Write>(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        io: &mut protocol_v5::FramedIo<R, W>,
    ) -> Result<()> {
        loop {
            let mut frames = Vec::with_capacity(V5_SERVER_WRITE_BATCH_FRAMES);
            for _ in 0..V5_SERVER_WRITE_BATCH_FRAMES {
                let Some(frame) = session.pop_next_frame().context("failed to pop v5 frame")?
                else {
                    break;
                };
                frames.push(frame);
            }
            if frames.is_empty() {
                return Ok(());
            }
            io.write_frame_batch(&mut frames)
                .context("failed to write v5 frame batch")?;
            for frame in &frames {
                session.observe_frame_written(frame);
            }
        }
    }

    fn drain_v5_session_writer<W: Write>(
        &self,
        session: &mut protocol_v5::ProtocolSession,
        writer: &mut W,
        limits: protocol_v5::FrameLimits,
        next_frame_sequence: &mut u64,
    ) -> Result<()> {
        loop {
            let mut frames = Vec::with_capacity(V5_SERVER_WRITE_BATCH_FRAMES);
            for _ in 0..V5_SERVER_WRITE_BATCH_FRAMES {
                let Some(mut frame) = session.pop_next_frame().context("failed to pop v5 frame")?
                else {
                    break;
                };
                frame.frame_sequence = *next_frame_sequence;
                *next_frame_sequence = next_frame_sequence
                    .checked_add(1)
                    .context("v5 frame sequence exhausted")?;
                frames.push(frame);
            }
            if frames.is_empty() {
                return Ok(());
            }
            protocol_v5::write_frame_batch_with_limits(writer, &frames, limits)
                .context("failed to write v5 frame batch")?;
            for frame in &frames {
                session.observe_frame_written(frame);
            }
        }
    }

    fn execute(
        &self,
        request: RemoteRequest,
        request_body: Vec<u8>,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        match request {
            RemoteRequest::Stat { path } => {
                let path = self.resolve_read_path(&path)?;
                let stat = block_on(self.backend.stat_with_cancellation(&path, cancellation))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::Stat(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::ListDir { path } => {
                let path = self.resolve_path(&path)?;
                let listing =
                    block_on(self.backend.list_dir_with_cancellation(&path, cancellation))
                        .map_err(remote_error_from_workspace)?;
                let listing = annotate_directory_listing_ignored_with_cancellation(
                    listing,
                    &self.workspace_root,
                    self.ignore_matcher.as_ref(),
                    cancellation,
                )
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::ListDir(
                        directory_listing_response_with_cancellation(listing, cancellation)
                            .map_err(remote_error_from_workspace)?,
                    ),
                    Vec::new(),
                ))
            }
            RemoteRequest::ListDirs { paths } => {
                let mut results = Vec::with_capacity(paths.len());
                for display_path in paths {
                    cancellation
                        .check_cancelled("list directories", &display_path)
                        .map_err(remote_error_from_workspace)?;
                    let result = match self.resolve_path(&display_path) {
                        Ok(path) => {
                            match block_on(
                                self.backend.list_dir_with_cancellation(&path, cancellation),
                            ) {
                                Ok(listing) => {
                                    let listing =
                                        annotate_directory_listing_ignored_with_cancellation(
                                            listing,
                                            &self.workspace_root,
                                            self.ignore_matcher.as_ref(),
                                            cancellation,
                                        )
                                        .map_err(remote_error_from_workspace)?;
                                    DirectoryListingResultResponse {
                                        path: display_path,
                                        listing: Some(
                                            directory_listing_response_with_cancellation(
                                                listing,
                                                cancellation,
                                            )
                                            .map_err(remote_error_from_workspace)?,
                                        ),
                                        error: None,
                                    }
                                }
                                Err(error @ WorkspaceError::Cancelled { .. }) => {
                                    return Err(remote_error_from_workspace(error));
                                }
                                Err(error) => {
                                    cancellation
                                        .check_cancelled("list directories", &path)
                                        .map_err(remote_error_from_workspace)?;
                                    DirectoryListingResultResponse {
                                        path: display_path,
                                        listing: None,
                                        error: Some(remote_error_from_workspace(error)),
                                    }
                                }
                            }
                        }
                        Err(error) => DirectoryListingResultResponse {
                            path: display_path,
                            listing: None,
                            error: Some(error),
                        },
                    };
                    cancellation
                        .check_cancelled("list directories", &self.workspace_root)
                        .map_err(remote_error_from_workspace)?;
                    results.push(result);
                }

                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::ListDirs(ListDirsResponse { results }),
                    Vec::new(),
                ))
            }
            RemoteRequest::FindAncestorFile {
                start,
                file_name,
                limit,
            } => {
                let start = self.resolve_path(&start)?;
                let path = block_on(self.backend.find_ancestor_file_with_cancellation(
                    &start,
                    file_name.as_str(),
                    limit,
                    cancellation,
                ))
                .map_err(remote_error_from_workspace)?;
                let path = path.filter(|path| path_is_within_workspace(path, &self.workspace_root));
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::FindAncestorFile(path),
                    Vec::new(),
                ))
            }
            RemoteRequest::CreateFile { path } => {
                let path = self.resolve_path(&path)?;
                let stat = block_on(
                    self.backend
                        .create_file_with_cancellation(&path, cancellation),
                )
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::CreateFile(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::CreateDir { path } => {
                let path = self.resolve_path(&path)?;
                let stat = block_on(
                    self.backend
                        .create_dir_with_cancellation(&path, cancellation),
                )
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::CreateDir(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::RenamePath { from, to } => {
                let from = self.resolve_path(&from)?;
                let to = self.resolve_path(&to)?;
                let stat = block_on(self.backend.rename_path_with_cancellation(
                    &from,
                    &to,
                    cancellation,
                ))
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::RenamePath(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::DeletePath { path } => {
                let path = self.resolve_path(&path)?;
                let stat = block_on(
                    self.backend
                        .delete_path_with_cancellation(&path, cancellation),
                )
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::DeletePath(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::CopyPath { from, to } => {
                let from = self.resolve_path(&from)?;
                let to = self.resolve_path(&to)?;
                let stat = block_on(self.backend.copy_path_with_cancellation(
                    &from,
                    &to,
                    cancellation,
                ))
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::CopyPath(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::ReadFile { path, max_bytes } => {
                let path = self.resolve_read_path(&path)?;
                let max_bytes = Some(
                    max_bytes
                        .unwrap_or(MAX_FRAME_BODY_LEN)
                        .min(MAX_FRAME_BODY_LEN),
                );
                let read = block_on(self.backend.read_file_with_cancellation(
                    &path,
                    ReadOptions { max_bytes },
                    cancellation,
                ))
                .map_err(remote_error_from_workspace)?;
                let response = file_read_response(&read);
                let body = read.bytes;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::ReadFile(response),
                    body,
                ))
            }
            RemoteRequest::WriteFile {
                path,
                create_parent_dirs,
                expected_modified_unix_millis,
                expected_modified_unix_nanos,
            } => {
                let path = self.resolve_path(&path)?;
                let expected_modified = system_time_from_unix_millis_and_nanos(
                    expected_modified_unix_millis,
                    expected_modified_unix_nanos,
                );
                let result = block_on(self.backend.write_file_with_cancellation(
                    &path,
                    &request_body,
                    WriteOptions {
                        create_parent_dirs,
                        expected_modified,
                    },
                    cancellation,
                ))
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::WriteFile(write_result_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::FileSearch(request) => {
                let query = FileSearchQuery {
                    root: self.resolve_search_root(&request.root)?,
                    pattern: request.pattern,
                    limit: request.limit,
                    hidden: request.hidden,
                    parents: request.parents,
                    ignore: request.ignore,
                    git_ignore: request.git_ignore,
                    git_global: request.git_global,
                    git_exclude: request.git_exclude,
                    follow_links: request.follow_links,
                    max_depth: request.max_depth,
                    excluded_relative_prefixes: request.excluded_relative_prefixes,
                };
                let result = block_on(self.backend.file_search(query))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::FileSearch(file_search_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::TextSearch(request) => {
                let query = TextSearchQuery {
                    root: self.resolve_search_root(&request.root)?,
                    pattern: request.pattern,
                    limit: request.limit,
                    smart_case: request.smart_case,
                    hidden: request.hidden,
                    parents: request.parents,
                    ignore: request.ignore,
                    git_ignore: request.git_ignore,
                    git_global: request.git_global,
                    git_exclude: request.git_exclude,
                    follow_links: request.follow_links,
                    max_depth: request.max_depth,
                    max_file_bytes: request.max_file_bytes,
                    excluded_relative_paths: request.excluded_relative_paths,
                    custom_ignore_filenames: request.custom_ignore_filenames,
                };
                let result = block_on(self.backend.text_search(query))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::TextSearch(text_search_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::ProjectEnvironment { root } => {
                let root = self.resolve_search_root(&root)?;
                let snapshot = self
                    .load_project_environment(&root)
                    .map_err(remote_error_from_environment)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::ProjectEnvironment(project_environment_response(snapshot)),
                    Vec::new(),
                ))
            }
            RemoteRequest::GitHead { root } => {
                let root = self.resolve_search_root(&root)?;
                let result =
                    block_on(self.backend.git_head(&root)).map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::GitHead(git_head_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::GitStatus {
                root,
                include_untracked,
                limit,
            } => {
                let root = self.resolve_search_root(&root)?;
                let result = block_on(self.backend.git_status(
                    &root,
                    GitStatusOptions {
                        include_untracked,
                        limit,
                    },
                ))
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::GitStatus(git_status_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::RunProcess(request) => {
                let spec = self.process_spec_from_request(request, request_body, None)?;
                let output = block_on(self.backend.run_process(spec))
                    .map_err(remote_error_from_workspace)?;
                let response = process_output_response(&output);
                let mut body = output.stdout;
                body.extend_from_slice(&output.stderr);
                Ok(ServiceOutcome::continue_response(
                    RemoteResponse::RunProcess(response),
                    body,
                ))
            }
            RemoteRequest::Shutdown => Ok(ServiceOutcome::Shutdown),
        }
    }

    fn load_project_environment(
        &self,
        root: &Path,
    ) -> std::result::Result<ProjectEnvironmentSnapshot, ShellEnvironmentError> {
        self.load_project_environment_with_cancellation(root, None)
    }

    fn load_project_environment_with_cancellation(
        &self,
        root: &Path,
        cancellation: Option<&WorkspaceCancellationToken>,
    ) -> std::result::Result<ProjectEnvironmentSnapshot, ShellEnvironmentError> {
        self.runtime.block_on(async {
            let mut variables = self
                .project_environment
                .get_environment_for_directory_with_cancellation(
                    root,
                    cancellation.map(WorkspaceCancellationToken::as_atomic_bool),
                )
                .await?;
            let cached_origin = self.project_environment.get_cached_origin(root).await;
            let origin = cached_origin
                .map(project_environment_origin_from_cached)
                .unwrap_or(ProjectEnvironmentOrigin::ProcessBaseline);
            let diagnostics = self
                .project_environment
                .get_environment_diagnostics(root)
                .await;

            if origin == ProjectEnvironmentOrigin::ProcessBaseline {
                variables.insert(
                    "ZED_ENVIRONMENT".to_string(),
                    "process-baseline".to_string(),
                );
            }

            Ok(ProjectEnvironmentSnapshot {
                root: root.to_path_buf(),
                variables: variables.into_iter().collect(),
                origin,
                diagnostics,
            })
        })
    }

    fn project_environment_root_for_process(&self, cwd: &Path) -> PathBuf {
        let mut candidate = normalize_path_lexically(cwd);

        loop {
            if candidate.join(".envrc").is_file() {
                return candidate;
            }

            if candidate == self.workspace_root || !candidate.starts_with(&self.workspace_root) {
                break;
            }

            if !candidate.pop() {
                break;
            }
        }

        cwd.to_path_buf()
    }

    fn resolve_path(&self, path: &Path) -> std::result::Result<PathBuf, RemoteError> {
        let resolved = if path.as_os_str().is_empty() {
            self.workspace_root.clone()
        } else if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        };
        let resolved = normalize_path_lexically(&resolved);

        if path_is_within_workspace(&resolved, &self.workspace_root) {
            Ok(resolved)
        } else {
            Err(path_outside_workspace_error(path, &self.workspace_root))
        }
    }

    fn resolve_read_path(&self, path: &Path) -> std::result::Result<PathBuf, RemoteError> {
        if path.as_os_str().is_empty() || !path.is_absolute() {
            return self.resolve_path(path);
        }

        Ok(normalize_path_lexically(path))
    }

    fn resolve_search_root(&self, root: &Path) -> std::result::Result<PathBuf, RemoteError> {
        if root.as_os_str().is_empty() {
            Ok(self.workspace_root.clone())
        } else {
            self.resolve_path(root)
        }
    }

    fn classify_v5_watch_roots(&self, roots: &[String]) -> (Vec<String>, Vec<String>) {
        let mut accepted = BTreeSet::new();
        let mut unsupported = BTreeSet::new();
        for root in roots {
            match self.normalize_v5_watch_root(root) {
                Some(root) => {
                    accepted.insert(root);
                }
                None => {
                    unsupported.insert(root.clone());
                }
            }
        }
        (
            accepted.into_iter().collect(),
            unsupported.into_iter().collect(),
        )
    }

    fn normalize_v5_watch_root(&self, root: &str) -> Option<String> {
        let path = Path::new(root);
        let resolved = if root.is_empty() || root == "." {
            self.workspace_root.clone()
        } else if path.is_absolute() {
            normalize_path_lexically(path)
        } else {
            normalize_path_lexically(&self.workspace_root.join(path))
        };
        if !path_is_within_workspace(&resolved, &self.workspace_root) {
            return None;
        }
        let relative = resolved.strip_prefix(&self.workspace_root).ok()?;
        if relative.as_os_str().is_empty() {
            Some(".".to_string())
        } else {
            Some(posix_path_string(relative))
        }
    }
}

pub fn serve_local_workspace_v5<R: Read + Send + 'static, W: Write>(
    workspace_root: PathBuf,
    reader: R,
    writer: W,
) -> Result<()> {
    let info = protocol_v5::ServerHandshakeInfo::current(workspace_root.display().to_string());
    let io = protocol_v5::FramedIo::new(reader, writer);
    WorkspaceService::new(LocalWorkspaceBackend, workspace_root)?.serve_v5_concurrent(io, &info)
}

enum ServiceOutcome {
    Continue {
        response: Box<RemoteResponse>,
        body: Vec<u8>,
    },
    Shutdown,
}

impl ServiceOutcome {
    fn continue_response(response: RemoteResponse, body: Vec<u8>) -> Self {
        Self::Continue {
            response: Box::new(response),
            body,
        }
    }
}

struct V5ServiceCompletion {
    stream_id: u64,
    method: String,
    result: std::result::Result<ServiceOutcome, RemoteError>,
}

struct V5ServiceTerminal {
    stream_id: u64,
    method: String,
    result: std::result::Result<V5ServiceTerminalOutcome, RemoteError>,
}

enum V5ServiceTerminalOutcome {
    Continue,
    Shutdown,
}

enum V5ServeEvent {
    Inbound,
    Output,
    NativeWatch,
    WorkerFinished {
        stream_id: u64,
        terminal_queued: bool,
    },
}

enum V5ServeLoopEvent {
    Inbound(V5InboundEvent),
    Wake(V5ServeEvent),
}

enum V5ServeOutputEvent {
    StreamData {
        stream_id: u64,
        channel: protocol_v5::DataChannel,
        body: Vec<u8>,
        priority: protocol_v5::Priority,
    },
    PartialResponse {
        stream_id: u64,
        method: String,
        payload: Vec<u8>,
        priority: protocol_v5::Priority,
    },
    Progress {
        stream_id: u64,
        method: String,
        progress: protocol_v5::Progress,
    },
    Completed(V5ServiceTerminal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum V5ServeQueueError {
    Closed,
    Cancelled,
    EventTooLarge { retained_bytes: usize, max: usize },
}

#[derive(Clone)]
struct V5ServeOutputSender {
    sender: mpsc::SyncSender<V5ServeOutputEvent>,
    ready_events: mpsc::Sender<V5ServeEvent>,
    ready: Arc<AtomicBool>,
    pending_count: Arc<AtomicU64>,
    completion_budget: V5ConnectionByteBudget,
}

impl V5ServeOutputSender {
    fn new(
        sender: mpsc::SyncSender<V5ServeOutputEvent>,
        ready_events: mpsc::Sender<V5ServeEvent>,
    ) -> Self {
        Self {
            sender,
            ready_events,
            ready: Arc::new(AtomicBool::new(false)),
            pending_count: Arc::new(AtomicU64::new(0)),
            completion_budget: V5ConnectionByteBudget::new(V5_SERVE_COMPLETION_BYTE_BUDGET),
        }
    }

    #[cfg(test)]
    fn with_completion_budget(
        sender: mpsc::SyncSender<V5ServeOutputEvent>,
        ready_events: mpsc::Sender<V5ServeEvent>,
        completion_budget: V5ConnectionByteBudget,
    ) -> Self {
        Self {
            sender,
            ready_events,
            ready: Arc::new(AtomicBool::new(false)),
            pending_count: Arc::new(AtomicU64::new(0)),
            completion_budget,
        }
    }

    fn reserve_completion_bytes(
        &self,
        retained_bytes: usize,
    ) -> std::result::Result<V5ByteReservation, RemoteError> {
        let mut reservation = self.completion_budget.reservation();
        reservation
            .try_grow(retained_bytes)
            .map_err(|error| RemoteError {
                code: "resource_exhausted".to_string(),
                message: "v5 service completions exceed the connection memory budget".to_string(),
                diagnostic: Some(error.to_string()),
            })?;
        Ok(reservation)
    }

    fn send(&self, event: V5ServeOutputEvent) -> std::result::Result<(), V5ServeQueueError> {
        let retained_bytes = event.retained_bytes();
        if retained_bytes > V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES {
            return Err(V5ServeQueueError::EventTooLarge {
                retained_bytes,
                max: V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES,
            });
        }
        self.pending_count.fetch_add(1, Ordering::AcqRel);
        if self.sender.send(event).is_err() {
            self.mark_delivered();
            return Err(V5ServeQueueError::Closed);
        }
        self.signal_ready()
    }

    fn send_with_cancellation(
        &self,
        mut event: V5ServeOutputEvent,
        cancellation: &WorkspaceCancellationToken,
    ) -> std::result::Result<(), V5ServeQueueError> {
        let retained_bytes = event.retained_bytes();
        if retained_bytes > V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES {
            return Err(V5ServeQueueError::EventTooLarge {
                retained_bytes,
                max: V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES,
            });
        }
        self.pending_count.fetch_add(1, Ordering::AcqRel);
        loop {
            if cancellation.is_cancelled() {
                self.mark_delivered();
                return Err(V5ServeQueueError::Cancelled);
            }
            match self.sender.try_send(event) {
                Ok(()) => return self.signal_ready(),
                Err(mpsc::TrySendError::Full(returned)) => {
                    event = returned;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.mark_delivered();
                    return Err(V5ServeQueueError::Closed);
                }
            }
        }
    }

    fn signal_ready(&self) -> std::result::Result<(), V5ServeQueueError> {
        if !self.ready.swap(true, Ordering::AcqRel)
            && self.ready_events.send(V5ServeEvent::Output).is_err()
        {
            self.ready.store(false, Ordering::Release);
            return Err(V5ServeQueueError::Closed);
        }
        Ok(())
    }

    fn clear_ready(&self) {
        self.ready.store(false, Ordering::Release);
    }

    fn has_pending_output(&self) -> bool {
        self.pending_count.load(Ordering::Acquire) != 0
    }

    fn mark_delivered(&self) {
        self.pending_count.fetch_sub(1, Ordering::AcqRel);
    }
}

struct V5SerializedByteCounter<'a> {
    bytes: usize,
    cancellation: &'a WorkspaceCancellationToken,
}

impl Write for V5SerializedByteCounter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "v5 response sizing cancelled",
            ));
        }
        self.bytes = self.bytes.checked_add(bytes.len()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "v5 serialized response length overflowed usize",
            )
        })?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn v5_serialized_response_len(
    response: &RemoteResponse,
    cancellation: &WorkspaceCancellationToken,
) -> std::result::Result<usize, RemoteError> {
    if cancellation.is_cancelled() {
        return Err(v5_cancelled_response_error());
    }
    let mut counter = V5SerializedByteCounter {
        bytes: 0,
        cancellation,
    };
    if let Err(error) = serde_json::to_writer(&mut counter, response) {
        if cancellation.is_cancelled() {
            return Err(v5_cancelled_response_error());
        }
        return Err(RemoteError {
            code: "internal".to_string(),
            message: "failed to size v5 response payload".to_string(),
            diagnostic: Some(error.to_string()),
        });
    }
    Ok(counter.bytes)
}

struct V5SerializedResponseWriter<'a> {
    output_events: &'a V5ServeOutputSender,
    stream_id: u64,
    priority: protocol_v5::Priority,
    cancellation: &'a WorkspaceCancellationToken,
    buffer: Vec<u8>,
    queue_error: Option<V5ServeQueueError>,
}

impl<'a> V5SerializedResponseWriter<'a> {
    fn new(
        output_events: &'a V5ServeOutputSender,
        stream_id: u64,
        priority: protocol_v5::Priority,
        cancellation: &'a WorkspaceCancellationToken,
    ) -> Self {
        Self {
            output_events,
            stream_id,
            priority,
            cancellation,
            buffer: Vec::with_capacity(V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES),
            queue_error: None,
        }
    }

    fn flush_chunk(&mut self) -> io::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "v5 response serialization cancelled",
            ));
        }
        if self.buffer.is_empty() {
            return Ok(());
        }
        let body = std::mem::replace(
            &mut self.buffer,
            Vec::with_capacity(V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES),
        );
        self.output_events
            .send_with_cancellation(
                V5ServeOutputEvent::StreamData {
                    stream_id: self.stream_id,
                    channel: protocol_v5::DataChannel::Unspecified,
                    body,
                    priority: self.priority,
                },
                self.cancellation,
            )
            .map_err(|error| {
                self.queue_error = Some(error);
                io::Error::other("v5 service output queue rejected serialized response data")
            })
    }
}

impl Write for V5SerializedResponseWriter<'_> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "v5 response serialization cancelled",
            ));
        }
        let written = bytes.len();
        while !bytes.is_empty() {
            let available =
                V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES.saturating_sub(self.buffer.len());
            let take = available.min(bytes.len());
            self.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.buffer.len() == V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES {
                self.flush_chunk()?;
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn v5_serialize_response_to_output(
    output_events: &V5ServeOutputSender,
    stream_id: u64,
    response: &RemoteResponse,
    priority: protocol_v5::Priority,
    cancellation: &WorkspaceCancellationToken,
) -> std::result::Result<(), RemoteError> {
    if cancellation.is_cancelled() {
        return Err(v5_cancelled_response_error());
    }
    let mut writer =
        V5SerializedResponseWriter::new(output_events, stream_id, priority, cancellation);
    if let Err(error) = serde_json::to_writer(&mut writer, response) {
        if let Some(queue_error) = writer.queue_error {
            return Err(v5_queue_error_to_remote_error(queue_error));
        }
        if cancellation.is_cancelled() {
            return Err(v5_cancelled_response_error());
        }
        return Err(RemoteError {
            code: "internal".to_string(),
            message: "failed to encode v5 response payload".to_string(),
            diagnostic: Some(error.to_string()),
        });
    }
    if let Err(error) = writer.flush_chunk() {
        if let Some(queue_error) = writer.queue_error {
            return Err(v5_queue_error_to_remote_error(queue_error));
        }
        if cancellation.is_cancelled() {
            return Err(v5_cancelled_response_error());
        }
        return Err(RemoteError {
            code: "internal".to_string(),
            message: "failed to buffer v5 response payload".to_string(),
            diagnostic: Some(error.to_string()),
        });
    }
    Ok(())
}

fn v5_cancelled_response_error() -> RemoteError {
    RemoteError {
        code: protocol_v5::RESET_CANCELLED.to_string(),
        message: "v5 response production cancelled".to_string(),
        diagnostic: None,
    }
}

fn v5_response_size_overflow_error() -> RemoteError {
    RemoteError {
        code: "resource_exhausted".to_string(),
        message: "v5 response size overflowed the server byte counter".to_string(),
        diagnostic: None,
    }
}

impl V5ServeOutputEvent {
    fn retained_bytes(&self) -> usize {
        match self {
            Self::StreamData { body, .. } => body.capacity(),
            Self::PartialResponse {
                method, payload, ..
            } => method.capacity().saturating_add(payload.capacity()),
            Self::Progress {
                method, progress, ..
            } => method
                .capacity()
                .saturating_add(progress.message.capacity()),
            Self::Completed(completion) => {
                let result_bytes = match &completion.result {
                    Ok(_) => 0,
                    Err(error) => error
                        .code
                        .capacity()
                        .saturating_add(error.message.capacity())
                        .saturating_add(
                            error
                                .diagnostic
                                .as_ref()
                                .map_or(0, |diagnostic| diagnostic.capacity()),
                        ),
                };
                completion.method.capacity().saturating_add(result_bytes)
            }
        }
    }
}

fn v5_queue_error_to_remote_error(error: V5ServeQueueError) -> RemoteError {
    match error {
        V5ServeQueueError::Closed => RemoteError {
            code: "unavailable".to_string(),
            message: "v5 service output queue closed".to_string(),
            diagnostic: None,
        },
        V5ServeQueueError::Cancelled => RemoteError {
            code: protocol_v5::RESET_CANCELLED.to_string(),
            message: "v5 response production cancelled".to_string(),
            diagnostic: None,
        },
        V5ServeQueueError::EventTooLarge {
            retained_bytes,
            max,
        } => RemoteError {
            code: "resource_exhausted".to_string(),
            message: "v5 service output event exceeds its memory budget".to_string(),
            diagnostic: Some(format!("retained_bytes={retained_bytes} max={max}")),
        },
    }
}

fn v5_bound_terminal_error(mut error: RemoteError) -> RemoteError {
    error.code = v5_bounded_terminal_string(error.code, 256);
    error.message = v5_bounded_terminal_string(error.message, 16 * 1024);
    error.diagnostic = error
        .diagnostic
        .map(|diagnostic| v5_bounded_terminal_string(diagnostic, 32 * 1024));
    error
}

fn v5_bounded_terminal_string(mut value: String, max_bytes: usize) -> String {
    if value.len() > max_bytes {
        let mut boundary = max_bytes;
        while boundary > 0 && !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value.truncate(boundary);
    }
    value.into_boxed_str().into_string()
}

type V5InboundEvent = io::Result<Option<protocol_v5::Frame>>;

#[derive(Clone)]
struct V5InboundSender {
    sender: mpsc::SyncSender<V5InboundEvent>,
    ready_events: mpsc::Sender<V5ServeEvent>,
    ready: Arc<AtomicBool>,
}

impl V5InboundSender {
    fn new(
        sender: mpsc::SyncSender<V5InboundEvent>,
        ready_events: mpsc::Sender<V5ServeEvent>,
    ) -> Self {
        Self {
            sender,
            ready_events,
            ready: Arc::new(AtomicBool::new(false)),
        }
    }

    fn send(&self, event: V5InboundEvent) -> std::result::Result<(), V5ServeQueueError> {
        self.sender
            .send(event)
            .map_err(|_| V5ServeQueueError::Closed)?;
        self.signal_ready()
    }

    fn signal_ready(&self) -> std::result::Result<(), V5ServeQueueError> {
        if !self.ready.swap(true, Ordering::AcqRel)
            && self.ready_events.send(V5ServeEvent::Inbound).is_err()
        {
            self.ready.store(false, Ordering::Release);
            return Err(V5ServeQueueError::Closed);
        }
        Ok(())
    }

    fn clear_ready(&self) {
        self.ready.store(false, Ordering::Release);
    }
}

struct V5NativeWatchEvent {
    watch_id: u64,
    result: notify::Result<notify::Event>,
}

#[derive(Clone)]
struct V5NativeWatchSender {
    sender: mpsc::SyncSender<V5NativeWatchEvent>,
    ready_events: mpsc::Sender<V5ServeEvent>,
    ready: Arc<AtomicBool>,
    overflowed_watch_ids: Arc<Mutex<HashSet<u64>>>,
}

impl V5NativeWatchSender {
    fn new(
        sender: mpsc::SyncSender<V5NativeWatchEvent>,
        ready_events: mpsc::Sender<V5ServeEvent>,
    ) -> Self {
        Self {
            sender,
            ready_events,
            ready: Arc::new(AtomicBool::new(false)),
            overflowed_watch_ids: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn send(&self, event: V5NativeWatchEvent) -> std::result::Result<(), V5ServeQueueError> {
        match self.sender.try_send(event) {
            Ok(()) => self.signal_ready(),
            Err(mpsc::TrySendError::Full(event)) => {
                self.overflowed_watch_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(event.watch_id);
                self.signal_ready()
            }
            Err(mpsc::TrySendError::Disconnected(_)) => Err(V5ServeQueueError::Closed),
        }
    }

    fn signal_ready(&self) -> std::result::Result<(), V5ServeQueueError> {
        if !self.ready.swap(true, Ordering::AcqRel)
            && self.ready_events.send(V5ServeEvent::NativeWatch).is_err()
        {
            self.ready.store(false, Ordering::Release);
            return Err(V5ServeQueueError::Closed);
        }
        Ok(())
    }

    fn clear_ready(&self) {
        self.ready.store(false, Ordering::Release);
    }

    fn take_overflowed_watch_ids(&self) -> Vec<u64> {
        let mut overflowed = self
            .overflowed_watch_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut watch_ids = overflowed.drain().collect::<Vec<_>>();
        watch_ids.sort_unstable();
        watch_ids
    }
}

#[derive(Debug)]
enum V5QueuedStreamEvent {
    Pending,
    Complete {
        stream_id: u64,
    },
    Rejected {
        stream_id: u64,
        code: &'static str,
        diagnostic: String,
    },
}

#[derive(Debug)]
struct V5ServiceRequest {
    method: String,
    priority: protocol_v5::Priority,
    payload: Vec<u8>,
    body: Vec<u8>,
    retained_bytes: V5ByteReservation,
    received_payload_bytes: usize,
    received_body_bytes: usize,
    deadline_unix_ms: u64,
    supersedes_stream_id: u64,
    streamed_write: Option<V5StreamingWrite>,
    early_error: Option<RemoteError>,
}

impl V5ServiceRequest {
    fn from_envelope(
        envelope: protocol_v5::StreamEnvelope,
        priority: protocol_v5::Priority,
        budget: &V5ConnectionByteBudget,
    ) -> Self {
        Self {
            method: envelope.method,
            priority,
            payload: Vec::new(),
            body: Vec::new(),
            retained_bytes: budget.reservation(),
            received_payload_bytes: 0,
            received_body_bytes: 0,
            deadline_unix_ms: envelope.deadline_unix_ms,
            supersedes_stream_id: envelope.supersedes_stream_id,
            streamed_write: None,
            early_error: None,
        }
    }

    fn append_data(&mut self, channel: protocol_v5::DataChannel, bytes: Vec<u8>) {
        match channel {
            protocol_v5::DataChannel::Unspecified | protocol_v5::DataChannel::SearchPayload => {
                self.payload.extend(bytes)
            }
            protocol_v5::DataChannel::FileBody
            | protocol_v5::DataChannel::Stdin
            | protocol_v5::DataChannel::Stdout
            | protocol_v5::DataChannel::Stderr => self.body.extend(bytes),
        }
    }

    fn reserve_data(
        &mut self,
        channel: protocol_v5::DataChannel,
        bytes: usize,
        retained: bool,
    ) -> std::result::Result<(), RemoteError> {
        let (received, limit, label) = match channel {
            protocol_v5::DataChannel::Unspecified | protocol_v5::DataChannel::SearchPayload => (
                &mut self.received_payload_bytes,
                V5_MAX_REQUEST_PAYLOAD_BYTES,
                "payload",
            ),
            protocol_v5::DataChannel::FileBody
            | protocol_v5::DataChannel::Stdin
            | protocol_v5::DataChannel::Stdout
            | protocol_v5::DataChannel::Stderr => (
                &mut self.received_body_bytes,
                V5_MAX_REQUEST_BODY_BYTES,
                "body",
            ),
        };
        let Some(total) = received.checked_add(bytes) else {
            return Err(v5_request_size_error(&self.method, label, limit));
        };
        if total > limit {
            return Err(v5_request_size_error(&self.method, label, limit));
        }
        if retained && let Err(error) = self.retained_bytes.try_grow(bytes) {
            return Err(RemoteError {
                code: "resource_exhausted".to_string(),
                message: format!(
                    "v5 {} request exceeds connection retained-byte budget: {error}",
                    self.method
                ),
                diagnostic: None,
            });
        }
        *received = total;
        Ok(())
    }
}

fn v5_request_size_error(method: &str, label: &str, limit: usize) -> RemoteError {
    RemoteError {
        code: "resource_exhausted".to_string(),
        message: format!("v5 {method} request {label} exceeds decoded byte limit {limit}"),
        diagnostic: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum V5ServiceTaskClass {
    Metadata,
    FileBody,
    Search,
    GitEnv,
    Process,
}

impl V5ServiceTaskClass {
    fn for_method(method: &str) -> Self {
        match method {
            "fs.read" | "fs.write" => Self::FileBody,
            "search.files" | "search.text" => Self::Search,
            "git.head" | "git.status" | "env.project" => Self::GitEnv,
            "process.run" => Self::Process,
            _ => Self::Metadata,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Metadata => 0,
            Self::FileBody => 1,
            Self::Search => 2,
            Self::GitEnv => 3,
            Self::Process => 4,
        }
    }

    fn limit(self) -> usize {
        match self {
            Self::Metadata => V5_METADATA_WORKER_LIMIT,
            Self::FileBody => V5_FILE_BODY_WORKER_LIMIT,
            Self::Search => V5_SEARCH_WORKER_LIMIT,
            Self::GitEnv => V5_GIT_ENV_WORKER_LIMIT,
            Self::Process => V5_PROCESS_WORKER_LIMIT,
        }
    }
}

#[derive(Debug, Default)]
struct V5ServiceTaskPools {
    active_by_class: [usize; 5],
    pending: VecDeque<(u64, V5ServiceRequest)>,
}

impl V5ServiceTaskPools {
    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn can_start_method(&self, method: &str) -> bool {
        let class = V5ServiceTaskClass::for_method(method);
        self.active_by_class[class.index()] < class.limit()
    }

    fn can_start(&self, request: &V5ServiceRequest) -> bool {
        self.can_start_method(&request.method)
    }

    fn mark_started(&mut self, method: &str) -> V5ServiceTaskClass {
        let class = V5ServiceTaskClass::for_method(method);
        self.active_by_class[class.index()] += 1;
        class
    }

    fn mark_finished(&mut self, class: V5ServiceTaskClass) {
        let active = &mut self.active_by_class[class.index()];
        *active = active.saturating_sub(1);
    }

    fn enqueue(&mut self, stream_id: u64, request: V5ServiceRequest) {
        self.pending.push_back((stream_id, request));
    }

    fn remove_pending(&mut self, stream_id: u64) -> bool {
        let Some(index) = self
            .pending
            .iter()
            .position(|(pending_stream_id, _)| *pending_stream_id == stream_id)
        else {
            return false;
        };
        let _ = self.pending.remove(index);
        true
    }

    fn clear_pending(&mut self) {
        self.pending.clear();
    }

    fn expired_pending_streams(&self, now_unix_ms: u64) -> Vec<u64> {
        self.pending
            .iter()
            .filter_map(|(stream_id, request)| {
                (request.deadline_unix_ms != 0 && request.deadline_unix_ms <= now_unix_ms)
                    .then_some(*stream_id)
            })
            .collect()
    }

    fn pop_next_startable(&mut self) -> Option<(u64, V5ServiceRequest)> {
        let index = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, (_, request))| self.can_start(request))
            .min_by_key(|(index, (_, request))| (request.priority.as_u8(), *index))
            .map(|(index, _)| index)?;
        self.pending.remove(index)
    }
}

fn v5_now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn v5_deadline_expired(deadline_unix_ms: u64) -> bool {
    deadline_unix_ms != 0 && deadline_unix_ms <= v5_now_unix_millis()
}

fn cancel_all_v5_service_work(
    requests: &mut HashMap<u64, V5ServiceRequest>,
    task_pools: &mut V5ServiceTaskPools,
    active_cancellations: &HashMap<u64, WorkspaceCancellationToken>,
    active_deadlines: &mut HashMap<u64, u64>,
    canceled_streams: &mut HashSet<u64>,
    watches: &mut V5WatchRegistry,
) {
    requests.clear();
    task_pools.clear_pending();
    for (stream_id, cancellation) in active_cancellations {
        cancellation.cancel();
        canceled_streams.insert(*stream_id);
    }
    active_deadlines.clear();
    watches.subscriptions.clear();
}

fn v5_ping_wait_timeout(
    last_activity: Instant,
    outstanding_ping_sent_at: Option<Instant>,
    idle_ping_interval: Duration,
    ping_timeout: Duration,
) -> Duration {
    if let Some(sent_at) = outstanding_ping_sent_at {
        return ping_timeout.saturating_sub(sent_at.elapsed());
    }
    idle_ping_interval.saturating_sub(last_activity.elapsed())
}

fn drive_v5_idle_ping(
    session: &mut protocol_v5::ProtocolSession,
    last_activity: &mut Instant,
    outstanding_ping: &mut Option<(Vec<u8>, Instant)>,
    next_ping_id: &mut u64,
    idle_ping_interval: Duration,
    ping_timeout: Duration,
) -> Result<()> {
    if let Some((_, sent_at)) = outstanding_ping.as_ref()
        && sent_at.elapsed() >= ping_timeout
    {
        return Err(anyhow::anyhow!("v5 peer did not answer idle PING"));
    }

    if outstanding_ping.is_none() && last_activity.elapsed() >= idle_ping_interval {
        *next_ping_id = next_ping_id.wrapping_add(1).max(1);
        let token = next_ping_id.to_be_bytes().to_vec();
        let expected_pong = protocol_v5::PingPayload {
            token: token.clone(),
        }
        .encode_to_vec();
        session
            .send_ping(token)
            .context("failed to queue v5 idle ping")?;
        *outstanding_ping = Some((expected_pong, Instant::now()));
        *last_activity = Instant::now();
    }

    Ok(())
}

fn expire_v5_service_deadlines(
    session: &mut protocol_v5::ProtocolSession,
    requests: &mut HashMap<u64, V5ServiceRequest>,
    task_pools: &mut V5ServiceTaskPools,
    active_cancellations: &HashMap<u64, WorkspaceCancellationToken>,
    active_deadlines: &mut HashMap<u64, u64>,
    canceled_streams: &mut HashSet<u64>,
) -> Result<()> {
    let now_unix_ms = v5_now_unix_millis();
    let expired_pending = requests
        .iter()
        .filter_map(|(stream_id, request)| {
            (request.deadline_unix_ms != 0 && request.deadline_unix_ms <= now_unix_ms)
                .then_some(*stream_id)
        })
        .collect::<Vec<_>>();
    let expired_ready = task_pools.expired_pending_streams(now_unix_ms);
    let expired_active = active_deadlines
        .iter()
        .filter_map(|(stream_id, deadline_unix_ms)| {
            (*deadline_unix_ms != 0 && *deadline_unix_ms <= now_unix_ms).then_some(*stream_id)
        })
        .collect::<Vec<_>>();

    let mut cancellation_state = V5ServiceCancellationState {
        task_pools,
        active_cancellations,
        active_deadlines,
        canceled_streams,
    };
    for stream_id in expired_pending {
        reset_v5_service_stream(
            session,
            requests,
            &mut cancellation_state,
            stream_id,
            protocol_v5::RESET_DEADLINE_EXCEEDED,
            "request deadline expired",
        )?;
    }
    for stream_id in expired_ready {
        reset_v5_service_stream(
            session,
            requests,
            &mut cancellation_state,
            stream_id,
            protocol_v5::RESET_DEADLINE_EXCEEDED,
            "request deadline expired",
        )?;
    }
    for stream_id in expired_active {
        reset_v5_service_stream(
            session,
            requests,
            &mut cancellation_state,
            stream_id,
            protocol_v5::RESET_DEADLINE_EXCEEDED,
            "request deadline expired",
        )?;
    }

    Ok(())
}

struct V5ServiceCancellationState<'a> {
    task_pools: &'a mut V5ServiceTaskPools,
    active_cancellations: &'a HashMap<u64, WorkspaceCancellationToken>,
    active_deadlines: &'a mut HashMap<u64, u64>,
    canceled_streams: &'a mut HashSet<u64>,
}

fn reset_v5_service_stream(
    session: &mut protocol_v5::ProtocolSession,
    requests: &mut HashMap<u64, V5ServiceRequest>,
    cancellation_state: &mut V5ServiceCancellationState<'_>,
    stream_id: u64,
    code: &'static str,
    diagnostic: impl Into<String>,
) -> Result<()> {
    requests.remove(&stream_id);
    cancellation_state.task_pools.remove_pending(stream_id);
    let was_active =
        if let Some(cancellation) = cancellation_state.active_cancellations.get(&stream_id) {
            cancellation.cancel();
            true
        } else {
            false
        };
    cancellation_state.active_deadlines.remove(&stream_id);
    if was_active {
        cancellation_state.canceled_streams.insert(stream_id);
    }
    session
        .reset_stream(stream_id, code, diagnostic)
        .context("failed to reset v5 service stream")?;
    Ok(())
}

#[derive(Default)]
struct V5WatchRegistry {
    next_watch_id: u64,
    subscriptions: HashMap<u64, V5WatchSubscription>,
    generations: protocol_v5::WatchGenerationTracker,
    native_events: Option<V5NativeWatchSender>,
}

struct V5WatchStartStatus {
    accepted_roots: Vec<String>,
    degraded_roots: Vec<String>,
    backend: String,
    degraded: bool,
}

struct V5WatchUpdateStatus {
    accepted_roots: Vec<String>,
    degraded_roots: Vec<String>,
}

struct V5WatchResyncStatus {
    accepted_roots: Vec<String>,
    unsupported_roots: Vec<String>,
}

struct V5WatchPendingBatch {
    event_stream_id: u64,
    watch_id: u64,
    changed_directories: Vec<String>,
    events: Vec<protocol_v5::WatchChange>,
    overflow: bool,
    resync_required: bool,
}

impl V5WatchRegistry {
    fn with_native_events(native_events: V5NativeWatchSender) -> Self {
        Self {
            native_events: Some(native_events),
            ..Self::default()
        }
    }

    fn allocate_watch_id(&mut self) -> Result<u64> {
        let watch_id = if self.next_watch_id == 0 {
            1
        } else {
            self.next_watch_id
        };
        self.next_watch_id = watch_id.checked_add(1).context("v5 watch id exhausted")?;
        Ok(watch_id)
    }

    fn has_active_watches(&self) -> bool {
        !self.subscriptions.is_empty()
    }

    fn next_poll_timeout(&self) -> Duration {
        let now = Instant::now();
        self.subscriptions
            .values()
            .filter_map(|subscription| subscription.next_due_at())
            .map(|due_at| due_at.saturating_duration_since(now))
            .min()
            .unwrap_or_else(|| Duration::from_secs(60))
    }

    fn start(
        &mut self,
        watch_id: u64,
        event_stream_id: u64,
        roots: Vec<String>,
        debounce_ms: u32,
        max_events_per_batch: u32,
        workspace_root: &Path,
    ) -> V5WatchStartStatus {
        let mut subscription = V5WatchSubscription::new(
            watch_id,
            event_stream_id,
            debounce_ms,
            max_events_per_batch,
            self.native_events.clone(),
        );
        for root in roots {
            subscription.add_root(root, workspace_root);
        }
        let status = V5WatchStartStatus {
            accepted_roots: subscription.accepted_roots(),
            degraded_roots: subscription.degraded_roots(),
            backend: subscription.backend_label(),
            degraded: subscription.is_degraded(),
        };
        self.subscriptions.insert(watch_id, subscription);
        status
    }

    fn update(
        &mut self,
        watch_id: u64,
        add_roots: Vec<String>,
        remove_roots: Vec<String>,
        workspace_root: &Path,
    ) -> std::result::Result<V5WatchUpdateStatus, RemoteError> {
        let Some(subscription) = self.subscriptions.get_mut(&watch_id) else {
            return Err(RemoteError {
                code: "not_found".to_string(),
                message: format!("unknown watch id {watch_id}"),
                diagnostic: None,
            });
        };
        for root in remove_roots {
            subscription.remove_root(&root, workspace_root);
        }
        for root in add_roots {
            subscription.add_root(root, workspace_root);
        }
        Ok(V5WatchUpdateStatus {
            accepted_roots: subscription.accepted_roots(),
            degraded_roots: subscription.degraded_roots(),
        })
    }

    fn stop(&mut self, watch_id: u64) -> Option<V5WatchSubscription> {
        self.subscriptions.remove(&watch_id)
    }

    fn resync(
        &mut self,
        watch_id: u64,
        requested_roots: Option<Vec<String>>,
    ) -> std::result::Result<V5WatchResyncStatus, RemoteError> {
        let Some(subscription) = self.subscriptions.get_mut(&watch_id) else {
            return Err(RemoteError {
                code: "not_found".to_string(),
                message: format!("unknown watch id {watch_id}"),
                diagnostic: None,
            });
        };
        Ok(subscription.force_resync(requested_roots))
    }

    fn record_native_event(
        &mut self,
        watch_id: u64,
        result: notify::Result<notify::Event>,
        workspace_root: &Path,
    ) -> Result<()> {
        let Some(subscription) = self.subscriptions.get_mut(&watch_id) else {
            return Ok(());
        };
        subscription.record_native_event(result, workspace_root);
        Ok(())
    }

    fn record_native_overflow(&mut self, watch_id: u64) {
        if let Some(subscription) = self.subscriptions.get_mut(&watch_id) {
            subscription.record_native_overflow();
        }
    }

    fn poll_due(&mut self, workspace_root: &Path) -> Result<Vec<(u64, protocol_v5::WatchBatch)>> {
        let now = Instant::now();
        let mut pending = Vec::new();
        for subscription in self.subscriptions.values_mut() {
            if let Some(batch) = subscription.take_due_batch(now, workspace_root) {
                pending.push(batch);
            }
        }
        let mut batches = Vec::with_capacity(pending.len());
        for batch in pending {
            let built = self.generations.build_batch(
                batch.watch_id,
                batch.changed_directories,
                batch.events,
                batch.overflow,
                batch.resync_required,
            )?;
            batches.push((batch.event_stream_id, built));
        }
        Ok(batches)
    }
}

struct V5NativeWatch {
    watcher: notify::RecommendedWatcher,
    roots: BTreeSet<String>,
}

impl V5NativeWatch {
    fn new(watch_id: u64, events: V5NativeWatchSender) -> notify::Result<Self> {
        let watcher = notify::recommended_watcher(move |result| {
            let _ = events.send(V5NativeWatchEvent { watch_id, result });
        })?;
        Ok(Self {
            watcher,
            roots: BTreeSet::new(),
        })
    }

    fn watch_root(&mut self, workspace_root: &Path, root: &str) -> bool {
        if self.roots.contains(root) {
            return true;
        }
        let path = v5_watch_root_path(workspace_root, root);
        match self
            .watcher
            .watch(&path, notify::RecursiveMode::NonRecursive)
        {
            Ok(()) => {
                self.roots.insert(root.to_string());
                true
            }
            Err(error) => {
                tracing::debug!(
                    root = %root,
                    path = %path.display(),
                    error = %error,
                    "Falling back to polling for v5 watch root"
                );
                false
            }
        }
    }

    fn unwatch_root(&mut self, workspace_root: &Path, root: &str) {
        if !self.roots.remove(root) {
            return;
        }
        let path = v5_watch_root_path(workspace_root, root);
        if let Err(error) = self.watcher.unwatch(&path) {
            tracing::debug!(
                root = %root,
                path = %path.display(),
                error = %error,
                "Failed to unwatch v5 native watch root"
            );
        }
    }

    fn has_roots(&self) -> bool {
        !self.roots.is_empty()
    }
}

struct V5WatchSubscription {
    watch_id: u64,
    event_stream_id: u64,
    roots: BTreeSet<String>,
    degraded_roots: BTreeSet<String>,
    fingerprints: HashMap<String, u64>,
    poll_interval: Duration,
    next_poll: Option<Instant>,
    next_emit: Option<Instant>,
    native: Option<V5NativeWatch>,
    pending_changed_directories: BTreeSet<String>,
    pending_events: Vec<protocol_v5::WatchChange>,
    max_events_per_batch: usize,
    pending_event_bytes: usize,
    pending_overflow: bool,
    pending_resync_required: bool,
}

impl V5WatchSubscription {
    fn new(
        watch_id: u64,
        event_stream_id: u64,
        debounce_ms: u32,
        max_events_per_batch: u32,
        native_events: Option<V5NativeWatchSender>,
    ) -> Self {
        let poll_interval = v5_watch_poll_interval(debounce_ms);
        let native = native_events.and_then(|events| match V5NativeWatch::new(watch_id, events) {
            Ok(watch) => Some(watch),
            Err(error) => {
                tracing::debug!(error = %error, "Native v5 file watching unavailable");
                None
            }
        });
        Self {
            watch_id,
            event_stream_id,
            roots: BTreeSet::new(),
            degraded_roots: BTreeSet::new(),
            fingerprints: HashMap::new(),
            poll_interval,
            next_poll: None,
            next_emit: None,
            native,
            pending_changed_directories: BTreeSet::new(),
            pending_events: Vec::new(),
            max_events_per_batch: v5_watch_event_limit(max_events_per_batch),
            pending_event_bytes: 0,
            pending_overflow: false,
            pending_resync_required: false,
        }
    }

    fn add_root(&mut self, root: String, workspace_root: &Path) {
        self.fingerprints.insert(
            root.clone(),
            v5_watch_root_fingerprint(workspace_root, &root),
        );
        self.roots.insert(root.clone());

        let watched_natively = self
            .native
            .as_mut()
            .is_some_and(|native| native.watch_root(workspace_root, &root));
        if watched_natively {
            self.degraded_roots.remove(&root);
        } else {
            self.degraded_roots.insert(root);
        }
        self.refresh_poll_timer();
    }

    fn remove_root(&mut self, root: &str, workspace_root: &Path) {
        self.roots.remove(root);
        self.degraded_roots.remove(root);
        self.fingerprints.remove(root);
        if let Some(native) = &mut self.native {
            native.unwatch_root(workspace_root, root);
        }
        self.pending_changed_directories.remove(root);
        self.refresh_poll_timer();
    }

    fn accepted_roots(&self) -> Vec<String> {
        self.roots.iter().cloned().collect()
    }

    fn degraded_roots(&self) -> Vec<String> {
        self.degraded_roots.iter().cloned().collect()
    }

    fn is_degraded(&self) -> bool {
        !self.degraded_roots.is_empty()
    }

    fn backend_label(&self) -> String {
        if self.native.as_ref().is_some_and(V5NativeWatch::has_roots) {
            if self.is_degraded() {
                "notify/poll"
            } else {
                "notify"
            }
        } else {
            "poll"
        }
        .to_string()
    }

    fn refresh_poll_timer(&mut self) {
        self.next_poll = if self.degraded_roots.is_empty() {
            None
        } else {
            Some(Instant::now() + self.poll_interval)
        };
    }

    fn next_due_at(&self) -> Option<Instant> {
        match (self.next_poll, self.next_emit) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(due), None) | (None, Some(due)) => Some(due),
            (None, None) => None,
        }
    }

    fn changed_degraded_roots(&mut self, workspace_root: &Path) -> Vec<String> {
        let mut changed = Vec::new();
        for root in self.degraded_roots.iter().cloned().collect::<Vec<_>>() {
            let fingerprint = v5_watch_root_fingerprint(workspace_root, &root);
            if self.fingerprints.get(&root).copied() != Some(fingerprint) {
                self.fingerprints.insert(root.clone(), fingerprint);
                changed.push(root);
            }
        }
        changed
    }

    fn force_resync(&mut self, requested_roots: Option<Vec<String>>) -> V5WatchResyncStatus {
        let requested_roots = requested_roots.unwrap_or_else(|| self.accepted_roots());
        let mut accepted_roots = BTreeSet::new();
        let mut unsupported_roots = BTreeSet::new();
        for root in requested_roots {
            if self.roots.contains(&root) {
                accepted_roots.insert(root);
            } else {
                unsupported_roots.insert(root);
            }
        }
        self.pending_changed_directories
            .extend(accepted_roots.iter().cloned());
        self.pending_resync_required = true;
        self.next_emit = Some(Instant::now());
        V5WatchResyncStatus {
            accepted_roots: accepted_roots.into_iter().collect(),
            unsupported_roots: unsupported_roots.into_iter().collect(),
        }
    }

    fn record_native_event(
        &mut self,
        result: notify::Result<notify::Event>,
        workspace_root: &Path,
    ) {
        match result {
            Ok(event) => self.record_notify_event(event, workspace_root),
            Err(error) => {
                tracing::debug!(error = %error, "Native v5 watch reported an error");
                self.degraded_roots.extend(self.roots.iter().cloned());
                self.refresh_poll_timer();
                self.pending_changed_directories
                    .extend(self.roots.iter().cloned());
                self.pending_overflow = true;
                self.pending_resync_required = true;
                self.schedule_emit();
            }
        }
    }

    fn record_native_overflow(&mut self) {
        tracing::debug!(
            watch_id = self.watch_id,
            "Native v5 watch event queue overflowed; requesting client reconciliation"
        );
        self.mark_overflow();
        self.schedule_emit();
    }

    fn record_notify_event(&mut self, event: notify::Event, workspace_root: &Path) {
        if self.roots.is_empty() {
            return;
        }

        if v5_notify_event_is_rename(&event)
            && event.paths.len() >= 2
            && let (Some(old_path), Some(new_path)) = (
                v5_watch_relative_path(workspace_root, &event.paths[0]),
                v5_watch_relative_path(workspace_root, &event.paths[1]),
            )
        {
            let is_dir = v5_notify_path_is_dir(&event.paths[1], &event.kind);
            self.record_changed_watch_path(&old_path);
            self.record_changed_watch_parent(&old_path);
            self.record_changed_watch_path(&new_path);
            self.record_changed_watch_parent(&new_path);
            self.record_watch_change(protocol_v5::WatchChange::renamed(
                old_path, new_path, is_dir,
            ));
            self.schedule_emit();
            return;
        }

        for path in event.paths {
            let Some(relative_path) = v5_watch_relative_path(workspace_root, &path) else {
                continue;
            };
            let is_dir = v5_notify_path_is_dir(&path, &event.kind);
            self.record_changed_watch_path(&relative_path);
            if is_dir {
                self.record_changed_watch_parent(&relative_path);
            }
            let change = match v5_notify_change_kind(&event.kind) {
                protocol_v5::WatchChangeKind::Created => {
                    protocol_v5::WatchChange::created(relative_path, is_dir)
                }
                protocol_v5::WatchChangeKind::Deleted => {
                    protocol_v5::WatchChange::deleted(relative_path, is_dir)
                }
                protocol_v5::WatchChangeKind::Renamed => {
                    protocol_v5::WatchChange::modified(relative_path, is_dir)
                }
                protocol_v5::WatchChangeKind::Modified => {
                    protocol_v5::WatchChange::modified(relative_path, is_dir)
                }
            };
            self.record_watch_change(change);
        }
        self.schedule_emit();
    }

    fn record_watch_change(&mut self, change: protocol_v5::WatchChange) {
        if self.pending_overflow {
            return;
        }
        let encoded_len = change.encoded_len().saturating_add(10);
        if self.pending_events.len() >= self.max_events_per_batch
            || self.pending_event_bytes.saturating_add(encoded_len) > V5_WATCH_BATCH_PAYLOAD_BUDGET
        {
            self.mark_overflow();
            return;
        }
        self.pending_event_bytes = self.pending_event_bytes.saturating_add(encoded_len);
        self.pending_events.push(change);
    }

    fn mark_overflow(&mut self) {
        self.pending_events.clear();
        self.pending_changed_directories.clear();
        self.pending_event_bytes = 0;
        self.pending_overflow = true;
        self.pending_resync_required = true;
    }

    fn record_changed_watch_path(&mut self, path: &str) {
        if let Some(root) = self.nearest_root_for_path(path) {
            self.pending_changed_directories.insert(root);
        }
    }

    fn record_changed_watch_parent(&mut self, path: &str) {
        if let Some(parent) = v5_watch_parent_path(path) {
            self.record_changed_watch_path(&parent);
        }
    }

    fn nearest_root_for_path(&self, path: &str) -> Option<String> {
        self.roots
            .iter()
            .filter(|root| v5_watch_root_contains(root, path))
            .max_by_key(|root| root.len())
            .cloned()
    }

    fn schedule_emit(&mut self) {
        if self.next_emit.is_none()
            && (!self.pending_events.is_empty()
                || !self.pending_changed_directories.is_empty()
                || self.pending_overflow
                || self.pending_resync_required)
        {
            self.next_emit = Some(Instant::now() + self.poll_interval);
        }
    }

    fn take_due_batch(
        &mut self,
        now: Instant,
        workspace_root: &Path,
    ) -> Option<V5WatchPendingBatch> {
        let mut changed_directories = BTreeSet::new();
        let mut events = Vec::new();
        let mut overflow = false;
        let mut resync_required = false;

        if self.next_emit.is_some_and(|due_at| due_at <= now) {
            self.next_emit = None;
            changed_directories.append(&mut self.pending_changed_directories);
            events.append(&mut self.pending_events);
            self.pending_event_bytes = 0;
            overflow = self.pending_overflow;
            resync_required = self.pending_resync_required;
            self.pending_overflow = false;
            self.pending_resync_required = false;
        }

        if self.next_poll.is_some_and(|due_at| due_at <= now) {
            self.next_poll = Some(now + self.poll_interval);
            let changed_roots = self.changed_degraded_roots(workspace_root);
            for root in changed_roots {
                changed_directories.insert(root.clone());
                events.push(protocol_v5::WatchChange::modified(root, true));
            }
        }

        if events.len() > self.max_events_per_batch
            || v5_watch_batch_payload_len(&changed_directories, &events)
                > V5_WATCH_BATCH_PAYLOAD_BUDGET
        {
            changed_directories.clear();
            events.clear();
            overflow = true;
            resync_required = true;
        }

        if changed_directories.is_empty() && events.is_empty() && !overflow && !resync_required {
            return None;
        }

        Some(V5WatchPendingBatch {
            event_stream_id: self.event_stream_id,
            watch_id: self.watch_id,
            changed_directories: changed_directories.into_iter().collect(),
            events,
            overflow,
            resync_required,
        })
    }
}

fn v5_watch_poll_interval(debounce_ms: u32) -> Duration {
    Duration::from_millis(u64::from(debounce_ms.clamp(50, 60_000)))
}

fn v5_watch_event_limit(requested: u32) -> usize {
    let requested = usize::try_from(requested).unwrap_or(V5_MAX_WATCH_EVENTS_PER_BATCH);
    if requested == 0 {
        V5_DEFAULT_WATCH_EVENTS_PER_BATCH
    } else {
        requested.min(V5_MAX_WATCH_EVENTS_PER_BATCH)
    }
}

fn v5_watch_batch_payload_len(
    changed_directories: &BTreeSet<String>,
    events: &[protocol_v5::WatchChange],
) -> usize {
    let generations = changed_directories
        .iter()
        .map(|path| path.len().saturating_add(32))
        .sum::<usize>();
    let events = events
        .iter()
        .map(|event| event.encoded_len().saturating_add(10))
        .sum::<usize>();
    64_usize.saturating_add(generations).saturating_add(events)
}

fn v5_watch_root_path(workspace_root: &Path, root: &str) -> PathBuf {
    if root == "." {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(root)
    }
}

fn v5_watch_relative_path(workspace_root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(workspace_root).ok()?;
    if relative.as_os_str().is_empty() {
        Some(".".to_string())
    } else {
        Some(posix_path_string(relative))
    }
}

fn v5_notify_event_is_rename(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
    )
}

fn v5_notify_change_kind(kind: &notify::EventKind) -> protocol_v5::WatchChangeKind {
    match kind {
        notify::EventKind::Create(_) => protocol_v5::WatchChangeKind::Created,
        notify::EventKind::Remove(_) => protocol_v5::WatchChangeKind::Deleted,
        notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
            protocol_v5::WatchChangeKind::Renamed
        }
        _ => protocol_v5::WatchChangeKind::Modified,
    }
}

fn v5_notify_path_is_dir(path: &Path, kind: &notify::EventKind) -> bool {
    path.is_dir()
        || matches!(
            kind,
            notify::EventKind::Create(notify::event::CreateKind::Folder)
                | notify::EventKind::Remove(notify::event::RemoveKind::Folder)
        )
}

fn v5_watch_parent_path(path: &str) -> Option<String> {
    if path == "." {
        None
    } else {
        Some(
            path.rsplit_once('/')
                .map(|(parent, _)| if parent.is_empty() { "." } else { parent })
                .unwrap_or(".")
                .to_string(),
        )
    }
}

fn v5_watch_root_contains(root: &str, path: &str) -> bool {
    root == "."
        || path == root
        || path
            .as_bytes()
            .get(root.len())
            .is_some_and(|separator| *separator == b'/')
            && path.starts_with(root)
}

fn v5_watch_root_fingerprint(workspace_root: &Path, root: &str) -> u64 {
    let path = v5_watch_root_path(workspace_root, root);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "v5-watch-root".hash(&mut hasher);
    root.hash(&mut hasher);

    let Ok(entries) = std::fs::read_dir(&path) else {
        "read_dir_error".hash(&mut hasher);
        return hasher.finish();
    };

    let mut entry_fingerprints = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                let name = entry.file_name().to_string_lossy().into_owned();
                let metadata = entry.metadata();
                let fingerprint = match metadata {
                    Ok(metadata) => {
                        let kind = if metadata.is_dir() {
                            "dir"
                        } else if metadata.is_file() {
                            "file"
                        } else {
                            "other"
                        };
                        let modified = metadata.modified().ok().and_then(|modified| {
                            modified
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
                        });
                        (name, kind.to_string(), metadata.len(), modified)
                    }
                    Err(error) => (name, format!("metadata_error:{:?}", error.kind()), 0, None),
                };
                entry_fingerprints.push(fingerprint);
            }
            Err(error) => {
                entry_fingerprints.push((
                    format!("read_entry_error:{:?}", error.kind()),
                    "error".to_string(),
                    0,
                    None,
                ));
            }
        }
    }
    entry_fingerprints.sort();
    entry_fingerprints.hash(&mut hasher);
    hasher.finish()
}

fn v5_response_body_chunks(
    response: &RemoteResponse,
    body: Vec<u8>,
) -> std::result::Result<Vec<(protocol_v5::DataChannel, Vec<u8>)>, RemoteError> {
    if body.is_empty() {
        return Ok(Vec::new());
    }

    match response {
        RemoteResponse::ReadFile(_) => Ok(vec![(protocol_v5::DataChannel::FileBody, body)]),
        RemoteResponse::RunProcess(process) => {
            let total_len = process
                .stdout_len
                .checked_add(process.stderr_len)
                .ok_or_else(|| RemoteError {
                    code: "invalid_response".to_string(),
                    message: "process output length overflow".to_string(),
                    diagnostic: None,
                })?;
            if total_len > body.len() {
                return Err(RemoteError {
                    code: "invalid_response".to_string(),
                    message: "process output body is shorter than declared lengths".to_string(),
                    diagnostic: Some(format!(
                        "stdout_len={} stderr_len={} body_len={}",
                        process.stdout_len,
                        process.stderr_len,
                        body.len()
                    )),
                });
            }
            let stdout = body[..process.stdout_len].to_vec();
            let stderr = body[process.stdout_len..total_len].to_vec();
            let mut chunks = Vec::new();
            if !stdout.is_empty() {
                chunks.push((protocol_v5::DataChannel::Stdout, stdout));
            }
            if !stderr.is_empty() {
                chunks.push((protocol_v5::DataChannel::Stderr, stderr));
            }
            Ok(chunks)
        }
        _ => Ok(vec![(protocol_v5::DataChannel::Unspecified, body)]),
    }
}

fn v5_streaming_file_search(
    query: FileSearchQuery,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<FileSearchResult, RemoteError> {
    let pattern = query
        .pattern
        .as_ref()
        .map(|pattern| RegexBuilder::new(pattern).case_insensitive(true).build())
        .transpose()
        .map_err(|error| {
            remote_error_from_workspace(WorkspaceError::InvalidSearchPattern(error))
        })?;
    let mut walker = WalkBuilder::new(&query.root);
    walker
        .hidden(!query.hidden)
        .parents(query.parents)
        .ignore(query.ignore)
        .git_ignore(query.git_ignore)
        .git_global(query.git_global)
        .git_exclude(query.git_exclude)
        .follow_links(query.follow_links)
        .add_custom_ignore_filename(".helix/ignore");
    if !query.excluded_relative_prefixes.is_empty() {
        let root = query.root.clone();
        let excluded_relative_prefixes = query.excluded_relative_prefixes.clone();
        walker.filter_entry(move |entry| {
            let relative_path = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            !excluded_relative_prefixes
                .iter()
                .any(|prefix| relative_path.starts_with(prefix))
        });
    }
    if let Some(max_depth) = query.max_depth {
        walker.max_depth(Some(max_depth));
    }

    let mut matched_count = 0_usize;
    let mut partial_files = Vec::new();
    let mut partial_flush = V5SearchPartialFlush::new();
    let mut truncated = false;
    for entry in walker.build() {
        if v5_stream_cancelled_ref(cancellation) {
            return Err(v5_cancelled_search_error(&query.root));
        }
        let entry = entry.map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "walk directory",
                path: query.root.clone(),
                source: io::Error::other(source),
            })
        })?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let relative_path = entry
            .path()
            .strip_prefix(&query.root)
            .unwrap_or(entry.path())
            .to_path_buf();
        if relative_path.as_os_str().is_empty() {
            continue;
        }
        if let Some(pattern) = &pattern
            && !pattern.is_match(&relative_path.to_string_lossy())
        {
            continue;
        }
        if matched_count >= query.limit {
            truncated = true;
            break;
        }
        matched_count += 1;
        partial_files.push(relative_path);
        if partial_flush.should_flush(partial_files.len()) {
            v5_send_file_search_partial(
                stream_id,
                &query.root,
                priority,
                &stream_events,
                std::mem::take(&mut partial_files),
                cancellation,
            )?;
            v5_send_search_progress(
                stream_id,
                "search.files",
                "file search matches",
                matched_count as u64,
                query.limit as u64,
                &stream_events,
                cancellation,
            )?;
            partial_flush.mark_flushed();
        }
    }

    Ok(FileSearchResult {
        root: query.root,
        files: partial_files,
        truncated,
    })
}

fn v5_send_file_search_partial(
    stream_id: u64,
    root: &Path,
    priority: protocol_v5::Priority,
    stream_events: &V5ServeOutputSender,
    files: Vec<PathBuf>,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<(), RemoteError> {
    if files.is_empty() {
        return Ok(());
    }
    let payload = RemoteResponse::FileSearch(FileSearchResponse {
        root: root.to_path_buf(),
        files,
        truncated: false,
    })
    .to_v5_payload()
    .map_err(v5_method_error_to_remote_error)?;
    v5_send_output_event_with_optional_cancellation(
        stream_events,
        V5ServeOutputEvent::PartialResponse {
            stream_id,
            method: "search.files".to_string(),
            payload,
            priority,
        },
        cancellation,
    )
    .map_err(v5_queue_error_to_remote_error)
}

fn v5_streaming_text_search(
    query: TextSearchQuery,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<TextSearchResult, RemoteError> {
    let case_insensitive = query.smart_case && !query.pattern.chars().any(char::is_uppercase);
    let pattern = RegexBuilder::new(&query.pattern)
        .case_insensitive(case_insensitive)
        .multi_line(true)
        .build()
        .map_err(|error| {
            remote_error_from_workspace(WorkspaceError::InvalidSearchPattern(error))
        })?;
    let mut walker = WalkBuilder::new(&query.root);
    walker
        .hidden(!query.hidden)
        .parents(query.parents)
        .ignore(query.ignore)
        .git_ignore(query.git_ignore)
        .git_global(query.git_global)
        .git_exclude(query.git_exclude)
        .follow_links(query.follow_links)
        .add_custom_ignore_filename(".helix/ignore");
    for filename in &query.custom_ignore_filenames {
        walker.add_custom_ignore_filename(filename);
    }
    if let Some(max_depth) = query.max_depth {
        walker.max_depth(Some(max_depth));
    }

    let mut matched_count = 0_usize;
    let mut partial_matches = Vec::new();
    let mut partial_flush = V5SearchPartialFlush::new();
    let mut truncated = false;
    let excluded_relative_paths = query
        .excluded_relative_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();

    'walk: for entry in walker.build() {
        if v5_stream_cancelled_ref(cancellation) {
            return Err(v5_cancelled_search_error(&query.root));
        }
        let entry = entry.map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "walk directory",
                path: query.root.clone(),
                source: io::Error::other(source),
            })
        })?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let metadata = std::fs::metadata(entry.path()).map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "stat search file",
                path: entry.path().to_path_buf(),
                source,
            })
        })?;
        if metadata.len() > query.max_file_bytes {
            continue;
        }

        let Ok(contents) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let relative_path = entry
            .path()
            .strip_prefix(&query.root)
            .unwrap_or(entry.path())
            .to_path_buf();
        if relative_path.as_os_str().is_empty() {
            continue;
        }
        if excluded_relative_paths.contains(&relative_path) {
            continue;
        }

        for (line_index, line_text) in contents.lines().enumerate() {
            if v5_stream_cancelled_ref(cancellation) {
                return Err(v5_cancelled_search_error(&query.root));
            }
            for found in pattern.find_iter(line_text) {
                if matched_count >= query.limit {
                    truncated = true;
                    break 'walk;
                }
                let search_match = TextSearchMatch {
                    relative_path: relative_path.clone(),
                    line_number: line_index + 1,
                    line_text: line_text.to_string(),
                    start: found.start(),
                    end: found.end(),
                };
                matched_count += 1;
                partial_matches.push(search_match);
                if partial_flush.should_flush(partial_matches.len()) {
                    v5_send_text_search_partial(
                        stream_id,
                        &query.root,
                        priority,
                        &stream_events,
                        std::mem::take(&mut partial_matches),
                        cancellation,
                    )?;
                    v5_send_search_progress(
                        stream_id,
                        "search.text",
                        "text search matches",
                        matched_count as u64,
                        query.limit as u64,
                        &stream_events,
                        cancellation,
                    )?;
                    partial_flush.mark_flushed();
                }
            }
        }
    }

    Ok(TextSearchResult {
        root: query.root,
        matches: partial_matches,
        truncated,
    })
}

fn v5_send_text_search_partial(
    stream_id: u64,
    root: &Path,
    priority: protocol_v5::Priority,
    stream_events: &V5ServeOutputSender,
    matches: Vec<TextSearchMatch>,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<(), RemoteError> {
    if matches.is_empty() {
        return Ok(());
    }
    let payload = RemoteResponse::TextSearch(text_search_response(TextSearchResult {
        root: root.to_path_buf(),
        matches,
        truncated: false,
    }))
    .to_v5_payload()
    .map_err(v5_method_error_to_remote_error)?;
    v5_send_output_event_with_optional_cancellation(
        stream_events,
        V5ServeOutputEvent::PartialResponse {
            stream_id,
            method: "search.text".to_string(),
            payload,
            priority,
        },
        cancellation,
    )
    .map_err(v5_queue_error_to_remote_error)
}

fn v5_send_search_progress(
    stream_id: u64,
    method: &str,
    message: &str,
    completed: u64,
    total: u64,
    stream_events: &V5ServeOutputSender,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<(), RemoteError> {
    v5_send_output_event_with_optional_cancellation(
        stream_events,
        V5ServeOutputEvent::Progress {
            stream_id,
            method: method.to_string(),
            progress: protocol_v5::Progress {
                message: message.to_string(),
                completed,
                total,
            },
        },
        cancellation,
    )
    .map_err(v5_queue_error_to_remote_error)
}

fn v5_send_output_event_with_optional_cancellation(
    output_events: &V5ServeOutputSender,
    event: V5ServeOutputEvent,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<(), V5ServeQueueError> {
    if let Some(cancellation) = cancellation {
        output_events.send_with_cancellation(event, cancellation)
    } else {
        output_events.send(event)
    }
}

#[derive(Debug)]
struct V5SearchPartialFlush {
    last_emit: Instant,
}

impl V5SearchPartialFlush {
    fn new() -> Self {
        Self {
            last_emit: Instant::now(),
        }
    }

    fn should_flush(&self, pending_len: usize) -> bool {
        pending_len >= V5_SEARCH_PARTIAL_BATCH_SIZE
            || (pending_len > 0
                && self.last_emit.elapsed() >= Duration::from_millis(V5_SEARCH_PARTIAL_INTERVAL_MS))
    }

    fn mark_flushed(&mut self) {
        self.last_emit = Instant::now();
    }
}

fn v5_cancelled_search_error(root: &Path) -> RemoteError {
    RemoteError {
        code: protocol_v5::RESET_CANCELLED.to_string(),
        message: "search cancelled".to_string(),
        diagnostic: Some(root.display().to_string()),
    }
}

fn v5_stream_file_chunks<R, F>(
    mut reader: R,
    mut remaining: u64,
    path: &Path,
    cancellation: &WorkspaceCancellationToken,
    mut emit: F,
) -> std::result::Result<(), RemoteError>
where
    R: Read,
    F: FnMut(Vec<u8>) -> std::result::Result<(), RemoteError>,
{
    let mut buffer = vec![0_u8; protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize];
    while remaining > 0 {
        cancellation
            .check_cancelled("read file", path)
            .map_err(remote_error_from_workspace)?;
        let read_limit = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..read_limit]).map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "read file",
                path: path.to_path_buf(),
                source,
            })
        })?;
        cancellation
            .check_cancelled("read file", path)
            .map_err(remote_error_from_workspace)?;
        if read == 0 {
            break;
        }
        remaining = remaining.saturating_sub(read as u64);
        emit(buffer[..read].to_vec())?;
        cancellation
            .check_cancelled("read file", path)
            .map_err(remote_error_from_workspace)?;
    }
    Ok(())
}

#[derive(Debug)]
struct V5StreamingWrite {
    original_path: PathBuf,
    target_path: PathBuf,
    expected_modified: Option<SystemTime>,
    existing_permissions: Option<std::fs::Permissions>,
    temp: tempfile::NamedTempFile,
}

impl V5StreamingWrite {
    fn create(
        path: PathBuf,
        create_parent_dirs: bool,
        expected_modified: Option<SystemTime>,
    ) -> std::result::Result<Self, WorkspaceError> {
        if let Some(parent) = path.parent()
            && create_parent_dirs
        {
            std::fs::create_dir_all(parent).map_err(|source| WorkspaceError::Io {
                operation: "create parent directories",
                path: parent.to_path_buf(),
                source,
            })?;
        }

        v5_validate_write_expected_modified(&path, expected_modified)?;
        let target_path = v5_write_target_for_path(&path)?;
        let existing_permissions = match std::fs::metadata(&target_path) {
            Ok(metadata) if metadata.is_file() => Some(metadata.permissions()),
            Ok(_) => {
                return Err(WorkspaceError::NotFile { path });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(WorkspaceError::Io {
                    operation: "stat write target",
                    path: target_path.clone(),
                    source,
                });
            }
        };
        let parent = target_path.parent().ok_or_else(|| WorkspaceError::Io {
            operation: "resolve write parent",
            path: target_path.clone(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"),
        })?;
        let temp = tempfile::Builder::new()
            .prefix(".nucleotide-write-")
            .tempfile_in(parent)
            .map_err(|source| WorkspaceError::Io {
                operation: "create temporary file",
                path: parent.to_path_buf(),
                source,
            })?;

        Ok(Self {
            original_path: path,
            target_path,
            expected_modified,
            existing_permissions,
            temp,
        })
    }

    fn write_chunk(&mut self, bytes: &[u8]) -> std::result::Result<(), WorkspaceError> {
        self.temp
            .write_all(bytes)
            .map_err(|source| WorkspaceError::Io {
                operation: "write temporary file",
                path: self.target_path.clone(),
                source,
            })
    }

    fn finish(
        mut self,
        cancellation: Option<&WorkspaceCancellationToken>,
    ) -> std::result::Result<WriteResult, WorkspaceError> {
        if v5_stream_cancelled_ref(cancellation) {
            return Err(v5_cancelled_write_error(&self.original_path));
        }
        v5_validate_write_expected_modified(&self.original_path, self.expected_modified)?;
        self.temp.flush().map_err(|source| WorkspaceError::Io {
            operation: "write temporary file",
            path: self.target_path.clone(),
            source,
        })?;
        if let Some(permissions) = self.existing_permissions {
            self.temp
                .as_file()
                .set_permissions(permissions)
                .map_err(|source| WorkspaceError::Io {
                    operation: "set temporary file permissions",
                    path: self.target_path.clone(),
                    source,
                })?;
        }
        self.temp
            .as_file()
            .sync_all()
            .map_err(|source| WorkspaceError::Io {
                operation: "sync temporary file",
                path: self.target_path.clone(),
                source,
            })?;
        if v5_stream_cancelled_ref(cancellation) {
            return Err(v5_cancelled_write_error(&self.original_path));
        }

        let temp_path = self.temp.into_temp_path();
        std::fs::rename(&temp_path, &self.target_path).map_err(|source| WorkspaceError::Io {
            operation: "replace file",
            path: self.target_path.clone(),
            source,
        })?;

        let metadata =
            std::fs::metadata(&self.target_path).map_err(|source| WorkspaceError::Io {
                operation: "stat written file",
                path: self.target_path,
                source,
            })?;

        Ok(WriteResult {
            path: self.original_path,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

fn v5_cancelled_write_error(path: &Path) -> WorkspaceError {
    WorkspaceError::Cancelled {
        operation: "write file",
        path: path.to_path_buf(),
    }
}

fn v5_validate_write_expected_modified(
    path: &Path,
    expected_modified: Option<SystemTime>,
) -> std::result::Result<(), WorkspaceError> {
    let Some(expected_modified) = expected_modified else {
        return Ok(());
    };
    let modified = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|source| WorkspaceError::Io {
            operation: "stat file before write",
            path: path.to_path_buf(),
            source,
        })?;
    if modified != expected_modified {
        return Err(WorkspaceError::Modified {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn v5_write_target_for_path(path: &Path) -> std::result::Result<PathBuf, WorkspaceError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path.to_path_buf()),
        Err(source) => {
            return Err(WorkspaceError::Io {
                operation: "stat write target",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if !metadata.file_type().is_symlink() {
        return Ok(path.to_path_buf());
    }

    let target = std::fs::read_link(path).map_err(|source| WorkspaceError::Io {
        operation: "read write symlink",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or_else(|| Path::new(".")).join(target)
    })
}

fn v5_local_git_head(
    root: &Path,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<GitHeadResult, WorkspaceError> {
    let mut command = Command::new("git");
    command
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(root);
    let output = v5_run_cancellable_command_collect(command, "git rev-parse", root, cancellation)?;

    if !output.status.success() {
        return Ok(GitHeadResult {
            root: root.to_path_buf(),
            head: None,
        });
    }

    let head = std::str::from_utf8(&output.stdout)
        .ok()
        .map(str::trim)
        .filter(|head| !head.is_empty())
        .map(ToOwned::to_owned);

    Ok(GitHeadResult {
        root: root.to_path_buf(),
        head,
    })
}

fn v5_local_git_status(
    root: &Path,
    options: GitStatusOptions,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<GitStatusResult, WorkspaceError> {
    let mut command = Command::new("git");
    command
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(root);
    if options.include_untracked {
        command.arg("--untracked-files=all");
    } else {
        command.arg("--untracked-files=no");
    }

    let output = v5_run_cancellable_command_collect(command, "git status", root, cancellation)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if v5_git_error_is_not_repository(&stderr) {
            return Ok(GitStatusResult {
                root: root.to_path_buf(),
                entries: Vec::new(),
                truncated: false,
            });
        }

        let message = if stderr.is_empty() {
            format!("git exited with status {}", output.status)
        } else {
            format!("git exited with status {}: {stderr}", output.status)
        };
        return Err(WorkspaceError::CommandFailed {
            operation: "git status",
            path: root.to_path_buf(),
            message,
        });
    }

    Ok(v5_parse_git_status_output(
        root,
        &output.stdout,
        options.limit,
    ))
}

#[derive(Debug)]
struct V5CollectedCommandOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn v5_run_cancellable_command_collect(
    mut command: Command,
    operation: &'static str,
    path: &Path,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<V5CollectedCommandOutput, WorkspaceError> {
    if v5_stream_cancelled_ref(cancellation) {
        return Err(WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: format!("{operation} cancelled"),
        });
    }

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    v5_configure_workspace_process(&mut command);

    let mut child = command.spawn().map_err(|source| WorkspaceError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: "child process stdout was not piped".to_string(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: "child process stderr was not piped".to_string(),
        })?;

    let stdout_thread = std::thread::spawn(move || v5_read_command_pipe(stdout));
    let stderr_thread = std::thread::spawn(move || v5_read_command_pipe(stderr));
    let exit = v5_wait_for_process(&mut child, None, cancellation, path)?;
    let stdout = v5_join_io_thread(stdout_thread, operation, path)?;
    let stderr = v5_join_io_thread(stderr_thread, operation, path)?;

    if exit.canceled {
        return Err(WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: format!("{operation} cancelled"),
        });
    }

    Ok(V5CollectedCommandOutput {
        status: exit.status,
        stdout,
        stderr,
    })
}

fn v5_read_command_pipe<R: Read>(mut reader: R) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    Ok(buffer)
}

fn v5_git_error_is_not_repository(message: &str) -> bool {
    message.contains("not a git repository")
}

fn v5_parse_git_status_output(root: &Path, output: &[u8], limit: usize) -> GitStatusResult {
    let mut entries = Vec::new();
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut truncated = false;

    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            continue;
        }

        let index = field[0];
        let worktree = field[1];
        let relative_path = v5_path_from_git_bytes(&field[3..]);
        let original_relative_path = if matches!(index, b'R' | b'C') {
            fields.next().map(v5_path_from_git_bytes)
        } else {
            None
        };

        if entries.len() >= limit {
            truncated = true;
            break;
        }

        entries.push(GitStatusEntry {
            relative_path,
            original_relative_path,
            index_status: v5_git_status_kind(index, worktree),
            working_tree_status: v5_git_status_kind(worktree, index),
        });
    }

    GitStatusResult {
        root: root.to_path_buf(),
        entries,
        truncated,
    }
}

fn v5_path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

fn v5_git_status_kind(status: u8, other: u8) -> GitStatusKind {
    if v5_git_status_is_conflict_pair(status, other) {
        return GitStatusKind::Conflicted;
    }

    match status {
        b' ' => GitStatusKind::Unmodified,
        b'M' => GitStatusKind::Modified,
        b'A' => GitStatusKind::Added,
        b'D' => GitStatusKind::Deleted,
        b'R' => GitStatusKind::Renamed,
        b'C' => GitStatusKind::Copied,
        b'T' => GitStatusKind::TypeChanged,
        b'?' => GitStatusKind::Untracked,
        b'U' => GitStatusKind::Conflicted,
        _ => GitStatusKind::Unknown,
    }
}

fn v5_git_status_is_conflict_pair(left: u8, right: u8) -> bool {
    matches!(
        (left, right),
        (b'D', b'D')
            | (b'A', b'U')
            | (b'U', b'D')
            | (b'U', b'A')
            | (b'D', b'U')
            | (b'A', b'A')
            | (b'U', b'U')
    )
}

#[derive(Debug)]
struct V5StreamedProcessOutput {
    status_code: Option<i32>,
    success: bool,
    stdout_len: usize,
    stderr_len: usize,
    stdout_truncated: bool,
    stderr_truncated: bool,
    timed_out: bool,
}

#[derive(Debug)]
struct V5StreamedProcessPipe {
    len: usize,
    truncated: bool,
}

fn v5_process_output_limit(max_output_bytes: Option<usize>) -> usize {
    max_output_bytes
        .unwrap_or((MAX_FRAME_BODY_LEN / 2) as usize)
        .min((MAX_FRAME_BODY_LEN / 2) as usize)
}

fn v5_run_local_streaming_process(
    spec: ProcessSpec,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<WorkspaceCancellationToken>,
) -> std::result::Result<V5StreamedProcessOutput, WorkspaceError> {
    let cwd = spec.cwd.clone();
    if v5_stream_cancelled(&cancellation) {
        return Err(WorkspaceError::CommandFailed {
            operation: "run process",
            path: cwd,
            message: "process cancelled".to_string(),
        });
    }

    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if spec.clear_env {
        command.env_clear();
    }
    v5_apply_process_environment(&mut command, &spec.env);
    v5_configure_workspace_process(&mut command);

    let mut child = command.spawn().map_err(|source| WorkspaceError::Io {
        operation: "spawn process",
        path: cwd.clone(),
        source,
    })?;

    let output_limit = spec
        .max_output_bytes
        .unwrap_or_else(|| v5_process_output_limit(None));
    let mut stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "spawn process",
            path: cwd.clone(),
            message: "child process stdout was not piped".to_string(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| WorkspaceError::CommandFailed {
            operation: "spawn process",
            path: cwd.clone(),
            message: "child process stderr was not piped".to_string(),
        })?;

    let stdout_events = stream_events.clone();
    let stdout_cancellation = cancellation.clone();
    let stdout_thread = std::thread::spawn(move || {
        v5_stream_process_stdout(
            stdout,
            output_limit,
            stream_id,
            priority,
            stdout_events,
            stdout_cancellation,
        )
    });
    let stderr_cancellation = cancellation.clone();
    let stderr_thread = std::thread::spawn(move || {
        v5_stream_process_stderr(
            stderr,
            output_limit,
            stream_id,
            priority,
            stream_events,
            stderr_cancellation,
        )
    });
    let input = spec.stdin;
    let stdin_thread = stdin.take().map(|mut stdin| {
        std::thread::spawn(move || match stdin.write_all(&input) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(error),
        })
    });

    let process_exit =
        v5_wait_for_process(&mut child, spec.timeout_ms, cancellation.as_ref(), &cwd)?;

    if let Some(thread) = stdin_thread {
        v5_join_io_thread(thread, "write process stdin", &cwd)?;
    }
    let stdout = v5_join_io_thread(stdout_thread, "stream process stdout", &cwd)?;
    let stderr = v5_join_io_thread(stderr_thread, "stream process stderr", &cwd)?;

    if process_exit.canceled {
        return Err(WorkspaceError::CommandFailed {
            operation: "run process",
            path: cwd,
            message: "process cancelled".to_string(),
        });
    }

    Ok(V5StreamedProcessOutput {
        status_code: process_exit.status.code(),
        success: process_exit.status.success(),
        stdout_len: stdout.len,
        stderr_len: stderr.len,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        timed_out: process_exit.timed_out,
    })
}

fn v5_stream_process_stdout(
    reader: ChildStdout,
    limit: usize,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<WorkspaceCancellationToken>,
) -> io::Result<V5StreamedProcessPipe> {
    v5_read_limited_process_pipe(
        reader,
        limit,
        stream_id,
        protocol_v5::DataChannel::Stdout,
        priority,
        stream_events,
        cancellation,
    )
}

fn v5_stream_process_stderr(
    reader: ChildStderr,
    limit: usize,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<WorkspaceCancellationToken>,
) -> io::Result<V5StreamedProcessPipe> {
    v5_read_limited_process_pipe(
        reader,
        limit,
        stream_id,
        protocol_v5::DataChannel::Stderr,
        priority,
        stream_events,
        cancellation,
    )
}

fn v5_read_limited_process_pipe<R: Read>(
    mut reader: R,
    limit: usize,
    stream_id: u64,
    channel: protocol_v5::DataChannel,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<WorkspaceCancellationToken>,
) -> io::Result<V5StreamedProcessPipe> {
    let mut len = 0_usize;
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];

    loop {
        if v5_stream_cancelled(&cancellation) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "process output streaming cancelled",
            ));
        }
        let read = reader.read(&mut buffer)?;
        if v5_stream_cancelled(&cancellation) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "process output streaming cancelled",
            ));
        }
        if read == 0 {
            break;
        }

        let remaining = limit.saturating_sub(len);
        let retained = remaining.min(read);
        if retained > 0 {
            v5_send_output_event_with_optional_cancellation(
                &stream_events,
                V5ServeOutputEvent::StreamData {
                    stream_id,
                    channel,
                    body: buffer[..retained].to_vec(),
                    priority,
                },
                cancellation.as_ref(),
            )
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "v5 service event loop closed while streaming process output",
                )
            })?;
            len += retained;
        }
        if retained < read {
            truncated = true;
        }
    }

    Ok(V5StreamedProcessPipe { len, truncated })
}

fn v5_wait_for_process(
    child: &mut Child,
    timeout_ms: Option<u64>,
    cancellation: Option<&WorkspaceCancellationToken>,
    path: &Path,
) -> std::result::Result<V5ProcessExit, WorkspaceError> {
    if timeout_ms.is_none() && cancellation.is_none() {
        return child
            .wait()
            .map(|status| V5ProcessExit {
                status,
                timed_out: false,
                canceled: false,
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "wait for process",
                path: path.to_path_buf(),
                source,
            });
    }

    let timeout = timeout_ms.map(Duration::from_millis);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|source| WorkspaceError::Io {
            operation: "poll process",
            path: path.to_path_buf(),
            source,
        })? {
            return Ok(V5ProcessExit {
                status,
                timed_out: false,
                canceled: false,
            });
        }

        if v5_stream_cancelled_ref(cancellation) {
            v5_kill_timed_out_process(child, path)?;
            return child
                .wait()
                .map(|status| V5ProcessExit {
                    status,
                    timed_out: false,
                    canceled: true,
                })
                .map_err(|source| WorkspaceError::Io {
                    operation: "wait for cancelled process",
                    path: path.to_path_buf(),
                    source,
                });
        }

        if let Some(timeout) = timeout {
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                v5_kill_timed_out_process(child, path)?;
                return child
                    .wait()
                    .map(|status| V5ProcessExit {
                        status,
                        timed_out: true,
                        canceled: false,
                    })
                    .map_err(|source| WorkspaceError::Io {
                        operation: "wait for killed process",
                        path: path.to_path_buf(),
                        source,
                    });
            }

            let remaining = timeout.saturating_sub(elapsed);
            std::thread::sleep(remaining.min(Duration::from_millis(10)));
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[derive(Debug)]
struct V5ProcessExit {
    status: std::process::ExitStatus,
    timed_out: bool,
    canceled: bool,
}

fn v5_stream_cancelled(cancellation: &Option<WorkspaceCancellationToken>) -> bool {
    v5_stream_cancelled_ref(cancellation.as_ref())
}

fn v5_stream_cancelled_ref(cancellation: Option<&WorkspaceCancellationToken>) -> bool {
    cancellation.is_some_and(WorkspaceCancellationToken::is_cancelled)
}

fn v5_apply_process_environment(command: &mut Command, environment: &BTreeMap<String, String>) {
    for (key, value) in environment {
        if v5_process_environment_entry_is_valid(key, value) {
            command.env(key, value);
        }
    }
}

fn v5_process_environment_entry_is_valid(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains(['=', '\0']) && !value.contains('\0')
}

#[cfg(unix)]
fn v5_configure_workspace_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn v5_configure_workspace_process(_command: &mut Command) {}

fn v5_kill_timed_out_process(
    child: &mut Child,
    path: &Path,
) -> std::result::Result<(), WorkspaceError> {
    #[cfg(unix)]
    {
        if v5_kill_process_group(child.id()).is_ok() {
            return Ok(());
        }
    }

    child.kill().map_err(|source| WorkspaceError::Io {
        operation: "kill timed out process",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn v5_kill_process_group(process_id: u32) -> io::Result<()> {
    let status = Command::new("kill")
        .arg("-KILL")
        .arg(format!("-{process_id}"))
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "kill process group exited with {status}"
        )))
    }
}

fn v5_join_io_thread<T>(
    thread: std::thread::JoinHandle<io::Result<T>>,
    operation: &'static str,
    path: &Path,
) -> std::result::Result<T, WorkspaceError> {
    thread
        .join()
        .map_err(|_| WorkspaceError::CommandFailed {
            operation,
            path: path.to_path_buf(),
            message: "I/O thread panicked".to_string(),
        })?
        .map_err(|source| WorkspaceError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })
}

fn v5_streamed_process_output_response(output: &V5StreamedProcessOutput) -> ProcessOutputResponse {
    ProcessOutputResponse {
        status_code: output.status_code,
        success: output.success,
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
        stdout_len: output.stdout_len,
        stderr_len: output.stderr_len,
        timed_out: output.timed_out,
    }
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn path_is_within_workspace(path: &Path, workspace_root: &Path) -> bool {
    let path = normalize_path_lexically(path);
    let workspace_root = normalize_path_lexically(workspace_root);
    path == workspace_root || path.starts_with(workspace_root)
}

fn path_outside_workspace_error(path: &Path, workspace_root: &Path) -> RemoteError {
    RemoteError {
        code: "path_outside_workspace".to_string(),
        message: format!(
            "path {} is outside workspace root {}",
            path.display(),
            workspace_root.display()
        ),
        diagnostic: None,
    }
}

fn file_stat_response(stat: FileStat) -> FileStatResponse {
    FileStatResponse {
        path: stat.path,
        kind: remote_file_kind(stat.kind),
        size: stat.size,
        modified_unix_millis: stat.modified.and_then(system_time_unix_millis),
        modified_unix_nanos: stat.modified.and_then(system_time_unix_nanos),
        readonly: stat.readonly,
    }
}

fn directory_listing_response_with_cancellation(
    listing: DirectoryListing,
    cancellation: &WorkspaceCancellationToken,
) -> nucleotide_workspace::Result<DirectoryListingResponse> {
    let path = listing.path;
    cancellation.check_cancelled("prepare directory listing response", &path)?;
    let mut entries = Vec::with_capacity(listing.entries.len());
    for entry in listing.entries {
        cancellation.check_cancelled("prepare directory listing response", &path)?;
        entries.push(DirectoryEntryResponse {
            name: entry.name,
            path: entry.path,
            stat: file_stat_response(entry.stat),
            symlink_target: entry.symlink_target,
            target_exists: entry.target_exists,
            ignored: entry.ignored,
        });
    }
    let mut response = DirectoryListingResponse {
        path,
        generation: None,
        fingerprint: None,
        complete: true,
        not_modified: false,
        delta: None,
        entries,
    };
    let fingerprint =
        directory_listing_response_fingerprint_with_cancellation(&response, cancellation)?;
    response.generation = Some(fingerprint);
    response.fingerprint = Some(fingerprint);
    cancellation.check_cancelled("prepare directory listing response", &response.path)?;
    Ok(response)
}

fn annotate_directory_listing_response_metadata(response: &mut DirectoryListingResponse) {
    let fingerprint = directory_listing_response_fingerprint(response);
    response.generation = Some(fingerprint);
    response.fingerprint = Some(fingerprint);
    response.complete = true;
}

fn directory_listing_not_modified_response(
    mut response: DirectoryListingResponse,
) -> DirectoryListingResponse {
    annotate_directory_listing_response_metadata(&mut response);
    response.entries.clear();
    response.not_modified = true;
    response.delta = None;
    response
}

fn directory_listing_response_for_known_state(
    response: DirectoryListingResponse,
    known_generation: Option<u64>,
    known_fingerprint: Option<u64>,
) -> DirectoryListingResponse {
    let generation = response.generation;
    let fingerprint = response.fingerprint;
    if known_generation.is_some() && known_generation == generation {
        return directory_listing_not_modified_response(response);
    }
    if known_fingerprint.is_some() && known_fingerprint == fingerprint {
        return directory_listing_not_modified_response(response);
    }
    response
}

fn directory_listing_delta_response_for_known_state(
    mut response: DirectoryListingResponse,
    previous: &DirectoryListingResponse,
    known_generation: Option<u64>,
    known_fingerprint: Option<u64>,
) -> DirectoryListingResponse {
    if !directory_listing_state_matches(previous, known_generation, known_fingerprint) {
        return response;
    }
    let delta = directory_listing_delta_response(previous, &response);
    let delta_entry_count = delta.added.len() + delta.updated.len() + delta.removed.len();
    if delta_entry_count == 0 || delta_entry_count > response.entries.len() {
        return response;
    }
    response.entries.clear();
    response.delta = Some(delta);
    response
}

fn directory_listing_state_matches(
    response: &DirectoryListingResponse,
    known_generation: Option<u64>,
    known_fingerprint: Option<u64>,
) -> bool {
    (known_generation.is_some() && response.generation == known_generation)
        || (known_fingerprint.is_some() && response.fingerprint == known_fingerprint)
}

fn directory_listing_delta_response(
    previous: &DirectoryListingResponse,
    current: &DirectoryListingResponse,
) -> DirectoryListingDeltaResponse {
    let previous_entries = previous
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let current_entries = current
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<HashMap<_, _>>();

    let mut added = Vec::new();
    let mut updated = Vec::new();
    for entry in &current.entries {
        match previous_entries.get(&entry.path) {
            Some(previous_entry) if *previous_entry == entry => {}
            Some(_) => updated.push(entry.clone()),
            None => added.push(entry.clone()),
        }
    }

    let removed = previous
        .entries
        .iter()
        .filter(|entry| !current_entries.contains_key(&entry.path))
        .map(|entry| entry.path.clone())
        .collect();

    DirectoryListingDeltaResponse {
        base_generation: previous.generation,
        base_fingerprint: previous.fingerprint,
        added,
        updated,
        removed,
    }
}

fn apply_directory_listing_delta(
    cache_key: &Path,
    base: DirectoryListingResponse,
    mut response: DirectoryListingResponse,
    delta: DirectoryListingDeltaResponse,
) -> std::result::Result<DirectoryListingResponse, RemoteClientError> {
    if !directory_listing_state_matches(&base, delta.base_generation, delta.base_fingerprint) {
        return Err(RemoteClientError::Protocol(format!(
            "v5 directory listing delta for {} did not match the cached base",
            cache_key.display()
        )));
    }

    let mut entries = base
        .entries
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for path in delta.removed {
        entries.remove(&path);
    }
    for entry in delta.added.into_iter().chain(delta.updated) {
        entries.insert(entry.path.clone(), entry);
    }

    response.entries = entries.into_values().collect();
    sort_directory_entry_responses(&mut response.entries);
    response.not_modified = false;
    response.delta = None;
    Ok(response)
}

fn sort_directory_entry_responses(entries: &mut [DirectoryEntryResponse]) {
    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn directory_listing_response_fingerprint(response: &DirectoryListingResponse) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "directory-listing-v5".hash(&mut hasher);
    response.path.hash(&mut hasher);
    response.complete.hash(&mut hasher);
    for entry in &response.entries {
        entry.name.hash(&mut hasher);
        entry.path.hash(&mut hasher);
        remote_file_kind_discriminant(&entry.stat.kind).hash(&mut hasher);
        entry.stat.path.hash(&mut hasher);
        entry.stat.size.hash(&mut hasher);
        entry.stat.modified_unix_millis.hash(&mut hasher);
        entry.stat.modified_unix_nanos.hash(&mut hasher);
        entry.stat.readonly.hash(&mut hasher);
        entry.symlink_target.hash(&mut hasher);
        entry.target_exists.hash(&mut hasher);
        entry.ignored.hash(&mut hasher);
    }
    hasher.finish()
}

fn directory_listing_response_fingerprint_with_cancellation(
    response: &DirectoryListingResponse,
    cancellation: &WorkspaceCancellationToken,
) -> nucleotide_workspace::Result<u64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "directory-listing-v5".hash(&mut hasher);
    response.path.hash(&mut hasher);
    response.complete.hash(&mut hasher);
    for entry in &response.entries {
        cancellation.check_cancelled("fingerprint directory listing", &response.path)?;
        entry.name.hash(&mut hasher);
        entry.path.hash(&mut hasher);
        remote_file_kind_discriminant(&entry.stat.kind).hash(&mut hasher);
        entry.stat.path.hash(&mut hasher);
        entry.stat.size.hash(&mut hasher);
        entry.stat.modified_unix_millis.hash(&mut hasher);
        entry.stat.modified_unix_nanos.hash(&mut hasher);
        entry.stat.readonly.hash(&mut hasher);
        entry.symlink_target.hash(&mut hasher);
        entry.target_exists.hash(&mut hasher);
        entry.ignored.hash(&mut hasher);
    }
    cancellation.check_cancelled("fingerprint directory listing", &response.path)?;
    Ok(hasher.finish())
}

fn remote_file_kind_discriminant(kind: &RemoteFileKind) -> u8 {
    match kind {
        RemoteFileKind::File => 0,
        RemoteFileKind::Directory => 1,
        RemoteFileKind::Symlink => 2,
        RemoteFileKind::Other => 3,
    }
}

fn file_read_response(read: &FileRead) -> FileReadResponse {
    FileReadResponse {
        path: read.path.clone(),
        size: read.size,
        modified_unix_millis: read.modified.and_then(system_time_unix_millis),
        modified_unix_nanos: read.modified.and_then(system_time_unix_nanos),
        readonly: read.readonly,
        truncated: read.truncated,
    }
}

fn write_result_response(result: WriteResult) -> WriteResultResponse {
    WriteResultResponse {
        path: result.path,
        size: result.size,
        modified_unix_millis: result.modified.and_then(system_time_unix_millis),
        modified_unix_nanos: result.modified.and_then(system_time_unix_nanos),
    }
}

fn file_search_response(result: FileSearchResult) -> FileSearchResponse {
    FileSearchResponse {
        root: result.root,
        files: result.files,
        truncated: result.truncated,
    }
}

fn text_search_response(result: TextSearchResult) -> TextSearchResponse {
    TextSearchResponse {
        root: result.root,
        matches: result
            .matches
            .into_iter()
            .map(|match_| TextSearchMatchResponse {
                relative_path: match_.relative_path,
                line_number: match_.line_number,
                line_text: match_.line_text,
                start: match_.start,
                end: match_.end,
            })
            .collect(),
        truncated: result.truncated,
    }
}

fn project_environment_response(
    snapshot: ProjectEnvironmentSnapshot,
) -> ProjectEnvironmentResponse {
    ProjectEnvironmentResponse {
        root: snapshot.root,
        variables: snapshot.variables,
        origin: remote_project_environment_origin(snapshot.origin),
        diagnostics: snapshot.diagnostics,
    }
}

fn git_head_response(result: GitHeadResult) -> GitHeadResponse {
    GitHeadResponse {
        root: result.root,
        head: result.head,
    }
}

fn git_status_response(result: GitStatusResult) -> GitStatusResponse {
    GitStatusResponse {
        root: result.root,
        entries: result
            .entries
            .into_iter()
            .map(|entry| GitStatusEntryResponse {
                relative_path: entry.relative_path,
                original_relative_path: entry.original_relative_path,
                index_status: remote_git_status_kind(entry.index_status),
                working_tree_status: remote_git_status_kind(entry.working_tree_status),
            })
            .collect(),
        truncated: result.truncated,
    }
}

fn process_output_response(output: &ProcessOutput) -> ProcessOutputResponse {
    ProcessOutputResponse {
        status_code: output.status_code,
        success: output.success,
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
        stdout_len: output.stdout.len(),
        stderr_len: output.stderr.len(),
        timed_out: output.timed_out,
    }
}

fn build_service_ignore_matcher(root_path: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(root_path);

    if let Ok(gitignore_path) = root_path.join(".gitignore").canonicalize()
        && gitignore_path.exists()
    {
        let _ = builder.add(&gitignore_path);
    }

    if let Some(git_config_dir) = dirs::config_dir() {
        let global_gitignore = git_config_dir.join("git").join("ignore");
        if global_gitignore.exists() {
            let _ = builder.add(&global_gitignore);
        }
    }

    let git_exclude = root_path.join(".git").join("info").join("exclude");
    if git_exclude.exists() {
        let _ = builder.add(&git_exclude);
    }

    let ignore_file = root_path.join(".ignore");
    if ignore_file.exists() {
        let _ = builder.add(&ignore_file);
    }

    let helix_ignore = root_path.join(".helix").join("ignore");
    if helix_ignore.exists() {
        let _ = builder.add(&helix_ignore);
    }

    builder.build().ok()
}

fn service_path_is_ignored(
    root_path: &Path,
    matcher: Option<&Gitignore>,
    path: &Path,
    kind: FileKind,
) -> bool {
    for component in path.components() {
        if let Component::Normal(name) = component
            && let Some(name_str) = name.to_str()
            && matches!(name_str, ".git" | ".svn" | ".hg" | ".bzr")
        {
            return true;
        }
    }

    if let Some(matcher) = matcher
        && let Ok(relative_path) = path.strip_prefix(root_path)
    {
        let matched = matcher.matched(relative_path, kind == FileKind::Directory);
        return matched.is_ignore();
    }

    false
}

fn annotate_directory_listing_ignored_with_cancellation(
    mut listing: DirectoryListing,
    root_path: &Path,
    matcher: Option<&Gitignore>,
    cancellation: &WorkspaceCancellationToken,
) -> nucleotide_workspace::Result<DirectoryListing> {
    cancellation.check_cancelled("annotate directory listing", &listing.path)?;
    for entry in &mut listing.entries {
        cancellation.check_cancelled("annotate directory listing", &listing.path)?;
        entry.ignored = Some(service_path_is_ignored(
            root_path,
            matcher,
            &entry.path,
            entry.stat.kind,
        ));
    }
    cancellation.check_cancelled("annotate directory listing", &listing.path)?;
    Ok(listing)
}

fn file_stat_from_response(stat: FileStatResponse) -> FileStat {
    FileStat {
        path: stat.path,
        kind: file_kind_from_response(stat.kind),
        size: stat.size,
        modified: system_time_from_unix_millis_and_nanos(
            stat.modified_unix_millis,
            stat.modified_unix_nanos,
        ),
        readonly: stat.readonly,
    }
}

fn directory_listing_from_response(listing: DirectoryListingResponse) -> DirectoryListing {
    DirectoryListing {
        path: listing.path,
        entries: listing
            .entries
            .into_iter()
            .map(|entry| nucleotide_workspace::DirectoryEntry {
                name: entry.name,
                path: entry.path,
                stat: file_stat_from_response(entry.stat),
                symlink_target: entry.symlink_target,
                target_exists: entry.target_exists,
                ignored: entry.ignored,
            })
            .collect(),
    }
}

fn file_read_from_response(
    read: FileReadResponse,
    bytes: Vec<u8>,
) -> std::result::Result<FileRead, RemoteClientError> {
    let body_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if body_len > read.size {
        return Err(RemoteClientError::Protocol(format!(
            "malformed read_file body: body has {} bytes but file size is {}",
            bytes.len(),
            read.size
        )));
    }
    if !read.truncated && body_len != read.size {
        return Err(RemoteClientError::Protocol(format!(
            "malformed read_file body: response is not truncated but body has {} bytes and file size is {}",
            bytes.len(),
            read.size
        )));
    }

    Ok(FileRead {
        path: read.path,
        bytes,
        size: read.size,
        modified: system_time_from_unix_millis_and_nanos(
            read.modified_unix_millis,
            read.modified_unix_nanos,
        ),
        readonly: read.readonly,
        truncated: read.truncated,
    })
}

fn write_result_from_response(result: WriteResultResponse) -> WriteResult {
    WriteResult {
        path: result.path,
        size: result.size,
        modified: system_time_from_unix_millis_and_nanos(
            result.modified_unix_millis,
            result.modified_unix_nanos,
        ),
    }
}

fn file_search_from_response(result: FileSearchResponse) -> FileSearchResult {
    FileSearchResult {
        root: result.root,
        files: result.files,
        truncated: result.truncated,
    }
}

fn text_search_from_response(result: TextSearchResponse) -> TextSearchResult {
    TextSearchResult {
        root: result.root,
        matches: result
            .matches
            .into_iter()
            .map(|match_| TextSearchMatch {
                relative_path: match_.relative_path,
                line_number: match_.line_number,
                line_text: match_.line_text,
                start: match_.start,
                end: match_.end,
            })
            .collect(),
        truncated: result.truncated,
    }
}

fn project_environment_from_response(
    snapshot: ProjectEnvironmentResponse,
) -> ProjectEnvironmentSnapshot {
    ProjectEnvironmentSnapshot {
        root: snapshot.root,
        variables: snapshot.variables,
        origin: project_environment_origin_from_response(snapshot.origin),
        diagnostics: snapshot.diagnostics,
    }
}

fn git_head_from_response(result: GitHeadResponse) -> GitHeadResult {
    GitHeadResult {
        root: result.root,
        head: result.head,
    }
}

fn git_status_from_response(result: GitStatusResponse) -> GitStatusResult {
    GitStatusResult {
        root: result.root,
        entries: result
            .entries
            .into_iter()
            .map(|entry| GitStatusEntry {
                relative_path: entry.relative_path,
                original_relative_path: entry.original_relative_path,
                index_status: git_status_kind_from_response(entry.index_status),
                working_tree_status: git_status_kind_from_response(entry.working_tree_status),
            })
            .collect(),
        truncated: result.truncated,
    }
}

fn process_output_from_response(
    response: ProcessOutputResponse,
    mut body: Vec<u8>,
) -> std::result::Result<ProcessOutput, RemoteClientError> {
    let expected_body_len = response
        .stdout_len
        .checked_add(response.stderr_len)
        .ok_or_else(|| {
            RemoteClientError::Protocol(
                "malformed run_process body: stdout and stderr lengths overflow".to_string(),
            )
        })?;
    if expected_body_len != body.len() {
        return Err(RemoteClientError::Protocol(format!(
            "malformed run_process body: header declares {expected_body_len} bytes but body has {} bytes",
            body.len()
        )));
    }

    let stdout_len = response.stdout_len;
    let stderr_start = stdout_len;
    let stderr_end = stderr_start + response.stderr_len;
    let stderr = body[stderr_start..stderr_end].to_vec();
    body.truncate(stdout_len);

    Ok(ProcessOutput {
        status_code: response.status_code,
        success: response.success,
        stdout: body,
        stderr,
        stdout_truncated: response.stdout_truncated,
        stderr_truncated: response.stderr_truncated,
        timed_out: response.timed_out,
    })
}

fn file_kind_from_response(kind: RemoteFileKind) -> FileKind {
    match kind {
        RemoteFileKind::File => FileKind::File,
        RemoteFileKind::Directory => FileKind::Directory,
        RemoteFileKind::Symlink => FileKind::Symlink,
        RemoteFileKind::Other => FileKind::Other,
    }
}

fn remote_file_kind(kind: FileKind) -> RemoteFileKind {
    match kind {
        FileKind::File => RemoteFileKind::File,
        FileKind::Directory => RemoteFileKind::Directory,
        FileKind::Symlink => RemoteFileKind::Symlink,
        FileKind::Other => RemoteFileKind::Other,
    }
}

fn remote_project_environment_origin(
    origin: ProjectEnvironmentOrigin,
) -> RemoteProjectEnvironmentOrigin {
    match origin {
        ProjectEnvironmentOrigin::NativeFlake => RemoteProjectEnvironmentOrigin::NativeFlake,
        ProjectEnvironmentOrigin::DirectoryShell => RemoteProjectEnvironmentOrigin::DirectoryShell,
        ProjectEnvironmentOrigin::ProcessBaseline => {
            RemoteProjectEnvironmentOrigin::ProcessBaseline
        }
        ProjectEnvironmentOrigin::Cli => RemoteProjectEnvironmentOrigin::Cli,
        ProjectEnvironmentOrigin::Unknown => RemoteProjectEnvironmentOrigin::Unknown,
    }
}

fn project_environment_origin_from_response(
    origin: RemoteProjectEnvironmentOrigin,
) -> ProjectEnvironmentOrigin {
    match origin {
        RemoteProjectEnvironmentOrigin::NativeFlake => ProjectEnvironmentOrigin::NativeFlake,
        RemoteProjectEnvironmentOrigin::DirectoryShell => ProjectEnvironmentOrigin::DirectoryShell,
        RemoteProjectEnvironmentOrigin::ProcessBaseline => {
            ProjectEnvironmentOrigin::ProcessBaseline
        }
        RemoteProjectEnvironmentOrigin::Cli => ProjectEnvironmentOrigin::Cli,
        RemoteProjectEnvironmentOrigin::Unknown => ProjectEnvironmentOrigin::Unknown,
    }
}

fn project_environment_origin_from_cached(origin: EnvironmentOrigin) -> ProjectEnvironmentOrigin {
    match origin {
        EnvironmentOrigin::Cli => ProjectEnvironmentOrigin::Cli,
        EnvironmentOrigin::NativeFlake => ProjectEnvironmentOrigin::NativeFlake,
        EnvironmentOrigin::DirectoryShell => ProjectEnvironmentOrigin::DirectoryShell,
        EnvironmentOrigin::Process => ProjectEnvironmentOrigin::ProcessBaseline,
    }
}

fn remote_git_status_kind(kind: GitStatusKind) -> RemoteGitStatusKind {
    match kind {
        GitStatusKind::Unmodified => RemoteGitStatusKind::Unmodified,
        GitStatusKind::Modified => RemoteGitStatusKind::Modified,
        GitStatusKind::Added => RemoteGitStatusKind::Added,
        GitStatusKind::Deleted => RemoteGitStatusKind::Deleted,
        GitStatusKind::Renamed => RemoteGitStatusKind::Renamed,
        GitStatusKind::Copied => RemoteGitStatusKind::Copied,
        GitStatusKind::TypeChanged => RemoteGitStatusKind::TypeChanged,
        GitStatusKind::Untracked => RemoteGitStatusKind::Untracked,
        GitStatusKind::Conflicted => RemoteGitStatusKind::Conflicted,
        GitStatusKind::Unknown => RemoteGitStatusKind::Unknown,
    }
}

fn git_status_kind_from_response(kind: RemoteGitStatusKind) -> GitStatusKind {
    match kind {
        RemoteGitStatusKind::Unmodified => GitStatusKind::Unmodified,
        RemoteGitStatusKind::Modified => GitStatusKind::Modified,
        RemoteGitStatusKind::Added => GitStatusKind::Added,
        RemoteGitStatusKind::Deleted => GitStatusKind::Deleted,
        RemoteGitStatusKind::Renamed => GitStatusKind::Renamed,
        RemoteGitStatusKind::Copied => GitStatusKind::Copied,
        RemoteGitStatusKind::TypeChanged => GitStatusKind::TypeChanged,
        RemoteGitStatusKind::Untracked => GitStatusKind::Untracked,
        RemoteGitStatusKind::Conflicted => GitStatusKind::Conflicted,
        RemoteGitStatusKind::Unknown => GitStatusKind::Unknown,
    }
}

fn remote_error_from_workspace(error: WorkspaceError) -> RemoteError {
    let code = match &error {
        WorkspaceError::Io { .. } => "io",
        WorkspaceError::Modified { .. } => "modified",
        WorkspaceError::NotFile { .. } => "not_file",
        WorkspaceError::InvalidSearchPattern(_) => "invalid_search_pattern",
        WorkspaceError::CommandFailed { .. } => "command_failed",
        WorkspaceError::Remote { .. } => "remote",
        WorkspaceError::Cancelled { .. } => protocol_v5::RESET_CANCELLED,
    };

    RemoteError {
        code: code.to_string(),
        message: error.to_string(),
        diagnostic: Some(format!("{error:?}")),
    }
}

fn remote_error_from_environment(error: ShellEnvironmentError) -> RemoteError {
    RemoteError {
        code: "project_environment".to_string(),
        message: error.to_string(),
        diagnostic: Some(format!("{error:?}")),
    }
}

fn client_error_to_workspace(
    operation: &'static str,
    path: &Path,
    error: RemoteClientError,
) -> WorkspaceError {
    match error {
        RemoteClientError::Remote(error) => remote_error_to_workspace(operation, path, error),
        RemoteClientError::Io(source) => WorkspaceError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        },
        other => WorkspaceError::Remote {
            operation,
            path: path.to_path_buf(),
            message: other.to_string(),
            diagnostic: Some(format!("{other:?}")),
        },
    }
}

fn remote_error_to_workspace(
    operation: &'static str,
    path: &Path,
    error: RemoteError,
) -> WorkspaceError {
    match error.code.as_str() {
        "modified" => WorkspaceError::Modified {
            path: path.to_path_buf(),
        },
        "not_file" => WorkspaceError::NotFile {
            path: path.to_path_buf(),
        },
        protocol_v5::RESET_CANCELLED => WorkspaceError::Cancelled {
            operation,
            path: path.to_path_buf(),
        },
        _ => WorkspaceError::Remote {
            operation,
            path: path.to_path_buf(),
            message: error.message,
            diagnostic: error.diagnostic,
        },
    }
}

fn unexpected_response_error(
    operation: &'static str,
    path: &Path,
    response: RemoteResponse,
) -> WorkspaceError {
    WorkspaceError::Remote {
        operation,
        path: path.to_path_buf(),
        message: format!("unexpected response: {response:?}"),
        diagnostic: None,
    }
}

fn system_time_unix_millis(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn system_time_unix_nanos(time: SystemTime) -> Option<u32> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.subsec_nanos())
}

fn system_time_from_unix_millis(millis: i64) -> Option<SystemTime> {
    u64::try_from(millis)
        .ok()
        .map(|millis| UNIX_EPOCH + Duration::from_millis(millis))
}

fn system_time_from_unix_millis_and_nanos(
    millis: Option<i64>,
    nanos: Option<u32>,
) -> Option<SystemTime> {
    if let (Some(millis), Some(nanos)) = (millis, nanos)
        && nanos < 1_000_000_000
    {
        let seconds = u64::try_from(millis.div_euclid(1_000)).ok()?;
        return Some(UNIX_EPOCH + Duration::new(seconds, nanos));
    }

    millis.and_then(system_time_from_unix_millis)
}

pub fn run_from_args<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_else(|| "help".to_string());

    match command.as_str() {
        "serve" => {
            let options = parse_serve_options(args)?;
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            serve_local_workspace_v5(options.workspace_root, stdin, stdout)
        }
        "lsp-proxy" => {
            let options = parse_lsp_proxy_options(args)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to create remote LSP proxy runtime")?;
            runtime.block_on(run_lsp_proxy(options))
        }
        "terminal-proxy" => {
            let options = parse_terminal_proxy_options(args)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to create remote terminal proxy runtime")?;
            runtime.block_on(run_terminal_proxy(options))
        }
        "version" => print_version(args, &mut std::io::stdout()).context("failed to write version"),
        "--help" | "-h" | "help" => {
            print_help(&mut std::io::stdout()).context("failed to write help")
        }
        other => bail!("unknown nucleotide-remote command: {other}"),
    }
}

fn print_version<I, W>(args: I, writer: &mut W) -> io::Result<()>
where
    I: IntoIterator<Item = String>,
    W: Write,
{
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--help" | "-h" => {
                writeln!(writer, "nucleotide-remote version [--json]")?;
                return Ok(());
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown version argument: {other}"),
                ));
            }
        }
    }

    let info = HelperVersionInfo::current();
    if json {
        serde_json::to_writer(&mut *writer, &info).map_err(io::Error::other)?;
        writeln!(writer)
    } else {
        writeln!(
            writer,
            "nucleotide-remote {} protocol {} frame {} {}-{}",
            info.helper_version, info.protocol_version, info.frame_version, info.os, info.arch
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServeOptions {
    workspace_root: PathBuf,
}

fn parse_serve_options<I>(args: I) -> Result<ServeOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut workspace_root = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let path = args
                    .next()
                    .context("--workspace requires a remote workspace path")?;
                let path = PathBuf::from(path);
                workspace_root = Some(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .context("failed to resolve current directory")?
                        .join(path)
                });
            }
            "--protocol" => {
                let value = args.next().context("--protocol requires v5")?;
                if !matches!(value.as_str(), "5" | "v5" | "V5") {
                    bail!("unsupported serve protocol: {value}");
                }
            }
            other => bail!("unknown serve argument: {other}"),
        }
    }

    let workspace_root = workspace_root
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .context("failed to resolve workspace root")?;
    Ok(ServeOptions { workspace_root })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LspProxyOptions {
    workspace_root: PathBuf,
    server: String,
    server_args: Vec<String>,
}

fn parse_lsp_proxy_options<I>(args: I) -> Result<LspProxyOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut workspace_root = None;
    let mut server = None;
    let mut server_args = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let path = args
                    .next()
                    .context("--workspace requires a remote workspace path")?;
                let path = PathBuf::from(path);
                workspace_root = Some(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .context("failed to resolve current directory")?
                        .join(path)
                });
            }
            "--server" => {
                server = Some(args.next().context("--server requires a language server")?);
            }
            "--server-arg" => {
                server_args.push(
                    args.next()
                        .context("--server-arg requires a language server argument")?,
                );
            }
            "--" => {
                server_args.extend(args);
                break;
            }
            other if server.is_none() => {
                server = Some(other.to_string());
            }
            other => {
                server_args.push(other.to_string());
            }
        }
    }

    Ok(LspProxyOptions {
        workspace_root: workspace_root
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .context("failed to resolve workspace root")?,
        server: server.context("lsp-proxy requires --server <language-server>")?,
        server_args,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalProxyOptions {
    workspace_root: PathBuf,
    shell: Option<String>,
    env: Vec<(String, String)>,
    command: Option<(String, Vec<String>)>,
}

fn parse_terminal_proxy_options<I>(args: I) -> Result<TerminalProxyOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut workspace_root = None;
    let mut shell = None;
    let mut env = Vec::new();
    let mut command = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let path = args
                    .next()
                    .context("--workspace requires a remote workspace path")?;
                let path = PathBuf::from(path);
                workspace_root = Some(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .context("failed to resolve current directory")?
                        .join(path)
                });
            }
            "--shell" => {
                shell = Some(args.next().context("--shell requires a shell path")?);
            }
            "--env" => {
                let entry = args.next().context("--env requires KEY=VALUE")?;
                let (key, value) = entry
                    .split_once('=')
                    .with_context(|| format!("terminal env entry must be KEY=VALUE: {entry}"))?;
                if !terminal_env_entry_is_valid(key, value) {
                    bail!("terminal env entry is invalid: {key}");
                }
                env.push((key.to_string(), value.to_string()));
            }
            "--" => {
                if let Some(program) = args.next() {
                    command = Some((program, args.collect()));
                }
                break;
            }
            other => bail!("unknown terminal-proxy argument: {other}"),
        }
    }

    Ok(TerminalProxyOptions {
        workspace_root: workspace_root
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .context("failed to resolve workspace root")?,
        shell,
        env,
        command,
    })
}

async fn run_lsp_proxy(options: LspProxyOptions) -> Result<()> {
    let environment = load_lsp_proxy_environment(&options.workspace_root).await?;
    let server_program = resolve_program_from_environment_path(
        &options.server,
        &environment,
        &options.workspace_root,
    );

    let mut child = nucleotide_process::tokio_command(&server_program)
        .args(&options.server_args)
        .current_dir(&options.workspace_root)
        .env_clear()
        .envs(&environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn language server {} in {}",
                server_program.display(),
                options.workspace_root.display()
            )
        })?;

    let mut server_stdin = child
        .stdin
        .take()
        .context("language server child did not expose stdin")?;
    let mut server_stdout = child
        .stdout
        .take()
        .context("language server child did not expose stdout")?;
    let mut client_stdin = tokio::io::stdin();
    let mut client_stdout = tokio::io::stdout();

    let mut stdin_task = tokio::spawn(async move {
        let copied = tokio::io::copy(&mut client_stdin, &mut server_stdin).await;
        let _ = server_stdin.shutdown().await;
        copied
    });
    let mut stdout_task =
        tokio::spawn(async move { tokio::io::copy(&mut server_stdout, &mut client_stdout).await });

    let status = tokio::select! {
        result = &mut stdin_task => {
            pipe_task_result(result, "copy LSP client stdin to server")?;
            child.wait().await.context("failed waiting for language server after stdin closed")?
        }
        result = &mut stdout_task => {
            pipe_task_result(result, "copy language server stdout to client")?;
            child.wait().await.context("failed waiting for language server after stdout closed")?
        }
        status = child.wait() => {
            status.context("failed waiting for language server")?
        }
    };

    stdin_task.abort();
    stdout_task.abort();

    if status.success() {
        Ok(())
    } else {
        bail!("language server exited with status {status}")
    }
}

async fn run_terminal_proxy(options: TerminalProxyOptions) -> Result<()> {
    let mut environment = load_proxy_environment("terminal-proxy", &options.workspace_root).await?;
    environment.extend(options.env.iter().cloned());
    remove_interactive_shell_state(&mut environment);

    let process = terminal_proxy_process(&options, &environment);
    let program_path = resolve_program_from_environment_path(
        &process.program,
        &environment,
        &options.workspace_root,
    );

    exec_terminal_proxy_process(
        &program_path,
        &process.args,
        process.login_shell,
        &environment,
        &options.workspace_root,
    )
    .with_context(|| {
        format!(
            "failed to run terminal command {} in {}",
            program_path.display(),
            options.workspace_root.display()
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalProxyProcess {
    program: String,
    args: Vec<String>,
    login_shell: bool,
}

fn terminal_proxy_process(
    options: &TerminalProxyOptions,
    environment: &HashMap<String, String>,
) -> TerminalProxyProcess {
    match &options.command {
        Some((program, args)) => TerminalProxyProcess {
            program: program.clone(),
            args: args.clone(),
            login_shell: false,
        },
        None => {
            let shell = options
                .shell
                .as_deref()
                .filter(|shell| !shell.is_empty())
                .or_else(|| environment.get("SHELL").map(String::as_str))
                .filter(|shell| !shell.is_empty())
                .unwrap_or("/bin/sh")
                .to_string();
            TerminalProxyProcess {
                program: shell,
                args: Vec::new(),
                login_shell: true,
            }
        }
    }
}

const INTERACTIVE_SHELL_STATE_ENV_VARS: &[&str] = &[
    "BASH_ENV",
    "BASHOPTS",
    "ENV",
    "POSIXLY_CORRECT",
    "PROMPT_COMMAND",
    "PS1",
    "SHELLOPTS",
];

fn remove_interactive_shell_state(environment: &mut HashMap<String, String>) {
    for key in INTERACTIVE_SHELL_STATE_ENV_VARS {
        environment.remove(*key);
    }
}

#[cfg(unix)]
fn exec_terminal_proxy_process(
    program: &Path,
    args: &[String],
    login_shell: bool,
    environment: &HashMap<String, String>,
    workspace_root: &Path,
) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(program);
    if login_shell {
        command.arg0(login_shell_arg0(program));
    }
    let error = command
        .args(args)
        .current_dir(workspace_root)
        .env_clear()
        .envs(environment)
        .exec();
    Err(error)
}

#[cfg(not(unix))]
fn exec_terminal_proxy_process(
    program: &Path,
    args: &[String],
    _login_shell: bool,
    environment: &HashMap<String, String>,
    workspace_root: &Path,
) -> io::Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(workspace_root)
        .env_clear()
        .envs(environment)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "terminal command exited with status {status}"
        )))
    }
}

#[cfg(any(unix, test))]
fn login_shell_arg0(program: &Path) -> OsString {
    let name = program.file_name().unwrap_or(program.as_os_str());
    let mut arg0 = OsString::from("-");
    arg0.push(name);
    arg0
}

fn pipe_task_result(
    result: std::result::Result<std::io::Result<u64>, tokio::task::JoinError>,
    operation: &'static str,
) -> Result<u64> {
    result
        .with_context(|| format!("{operation} task panicked"))?
        .with_context(|| format!("{operation} failed"))
}

async fn load_lsp_proxy_environment(root: &Path) -> Result<HashMap<String, String>> {
    load_proxy_environment("lsp-proxy", root).await
}

async fn load_proxy_environment(label: &str, root: &Path) -> Result<HashMap<String, String>> {
    let project_environment = ProjectEnvironment::new(Some(std::env::vars().collect()));
    let environment = project_environment
        .get_environment_for_directory(root)
        .await
        .with_context(|| format!("failed to load project environment for {}", root.display()))?;

    for diagnostic in project_environment.get_environment_diagnostics(root).await {
        eprintln!("nucleotide-remote {label} environment diagnostic: {diagnostic}");
    }

    Ok(environment)
}

fn resolve_program_from_environment_path(
    program: &str,
    environment: &HashMap<String, String>,
    workspace_root: &Path,
) -> PathBuf {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return if program_path.is_absolute() {
            program_path.to_path_buf()
        } else {
            workspace_root.join(program_path)
        };
    }

    environment
        .get("PATH")
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|directory| {
            if directory.is_absolute() {
                directory.join(program)
            } else {
                workspace_root.join(directory).join(program)
            }
        })
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| program_path.to_path_buf())
}

fn print_help<W: Write>(writer: &mut W) -> io::Result<()> {
    writeln!(writer, "nucleotide-remote serve [--workspace <path>]")?;
    writeln!(writer, "nucleotide-remote version [--json]")?;
    writeln!(
        writer,
        "nucleotide-remote lsp-proxy [--workspace <path>] --server <name> [-- <args>...]"
    )?;
    writeln!(
        writer,
        "nucleotide-remote terminal-proxy [--workspace <path>] [--shell <path>] [--env KEY=VALUE]... [-- <command> <args>...]"
    )?;
    writeln!(writer)?;
    writeln!(
        writer,
        "Protocol traffic uses framed messages on stdin/stdout."
    )?;
    writeln!(
        writer,
        "Proxy diagnostics are written to stderr so protocol and terminal streams stay clean."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg_index(args: &[OsString], needle: &str) -> usize {
        args.iter()
            .position(|arg| arg.as_os_str() == OsStr::new(needle))
            .unwrap_or_else(|| panic!("missing argument {needle:?} in {args:?}"))
    }

    fn has_arg_pair(args: &[OsString], key: &str, value: &str) -> bool {
        args.windows(2).any(|window| {
            window[0].as_os_str() == OsStr::new(key) && window[1].as_os_str() == OsStr::new(value)
        })
    }

    fn assert_arg_pair(args: &[OsString], key: &str, value: &str) {
        assert!(
            has_arg_pair(args, key, value),
            "missing argument pair {key:?} {value:?} in {args:?}"
        );
    }

    fn assert_ssh_non_interactive_defaults(args: &[OsString]) {
        assert_arg_pair(args, "-o", "BatchMode=yes");
        assert_arg_pair(args, "-o", "NumberOfPasswordPrompts=0");
        assert_arg_pair(args, "-o", "ConnectionAttempts=1");
        assert_arg_pair(args, "-o", "StrictHostKeyChecking=accept-new");
        assert_arg_pair(args, "-o", "ServerAliveInterval=15");
        assert_arg_pair(args, "-o", "ServerAliveCountMax=3");
    }

    fn ssh_target_separator_index(args: &[OsString]) -> usize {
        arg_index(args, "--")
    }
    use nucleotide_workspace::RemoteWorkspaceKind;
    use std::collections::VecDeque;
    use std::io::Cursor;
    #[cfg(unix)]
    use std::sync::atomic::AtomicBool;
    use std::sync::{
        Arc, Condvar, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn v5_client_input(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
        v5_client_input_with_settings(frames, protocol_v5::ConnectionSettings::recommended())
    }

    fn v5_client_input_with_settings(
        frames: Vec<protocol_v5::Frame>,
        settings: protocol_v5::ConnectionSettings,
    ) -> Vec<u8> {
        let mut hello = protocol_v5::ClientHello::nucleotide("test-client");
        hello.desired_settings = Some(settings);
        let mut all_frames = vec![
            protocol_v5::Frame::from_control(protocol_v5::FrameType::Hello, 0, &hello),
            protocol_v5::Frame::new(protocol_v5::FrameType::SettingsAck, 0),
        ];
        all_frames.extend(frames);
        encode_v5_sequenced_frames(all_frames)
    }

    fn v5_server_input(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
        let mut info = protocol_v5::ServerHandshakeInfo::current("/workspace");
        info.capabilities
            .retain(|capability| capability != "compression_zstd");
        v5_server_input_with_info(frames, info)
    }

    fn v5_server_input_with_compression(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
        v5_server_input_with_info(
            frames,
            protocol_v5::ServerHandshakeInfo::current("/workspace"),
        )
    }

    fn v5_server_input_with_info(
        frames: Vec<protocol_v5::Frame>,
        info: protocol_v5::ServerHandshakeInfo,
    ) -> Vec<u8> {
        let client = protocol_v5::ClientHello::nucleotide("test-client");
        v5_server_input_for_client(frames, &client, info)
    }

    fn v5_server_input_for_client(
        frames: Vec<protocol_v5::Frame>,
        client: &protocol_v5::ClientHello,
        info: protocol_v5::ServerHandshakeInfo,
    ) -> Vec<u8> {
        let hello = protocol_v5::ServerHello::accept_client(client, &info).unwrap();
        let settings = hello.accepted_settings.clone().unwrap();
        let mut all_frames = vec![
            protocol_v5::Frame::from_control(protocol_v5::FrameType::Hello, 0, &hello),
            protocol_v5::Frame::from_control(protocol_v5::FrameType::Settings, 0, &settings),
        ];
        all_frames.extend(frames);
        encode_v5_sequenced_frames(all_frames)
    }

    fn v5_heartbeat_client_hello(ping_timeout: Duration) -> protocol_v5::ClientHello {
        let mut settings = protocol_v5::ConnectionSettings::recommended();
        settings.ping_timeout_ms = u32::try_from(ping_timeout.as_millis()).unwrap();
        let mut client = protocol_v5::ClientHello::nucleotide("test-client");
        client.desired_settings = Some(settings);
        client
    }

    fn v5_test_client_heartbeat(now: Instant) -> V5ClientHeartbeat {
        let mut heartbeat =
            V5ClientHeartbeat::new(&protocol_v5::ConnectionSettings::recommended(), now);
        heartbeat.idle_ping_interval = Duration::from_millis(20);
        heartbeat.ping_timeout = Duration::from_millis(50);
        heartbeat
    }

    fn encode_v5_sequenced_frames(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
        let mut input = Vec::new();
        for (index, mut frame) in frames.into_iter().enumerate() {
            frame.frame_sequence = u64::try_from(index).unwrap() + 1;
            protocol_v5::write_frame(&mut input, &frame).unwrap();
        }
        input
    }

    fn v5_request_frames(
        stream_id: u64,
        request: &RemoteRequest,
        body: &[u8],
    ) -> Vec<protocol_v5::Frame> {
        v5_request_frames_with_options(stream_id, request, body, request.v5_request_options())
    }

    fn v5_request_frames_with_options(
        stream_id: u64,
        request: &RemoteRequest,
        body: &[u8],
        options: protocol_v5::RequestOptions,
    ) -> Vec<protocol_v5::Frame> {
        let (method, payload) = request.to_v5_method_payload().unwrap();
        let headers = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::request_with_options(stream_id, method, &options),
        );
        let payload = protocol_v5::stream_data_frame(
            stream_id,
            payload,
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap();
        let mut frames = vec![headers, payload];
        if !body.is_empty() {
            frames.push(
                protocol_v5::stream_data_frame(
                    stream_id,
                    body.to_vec(),
                    protocol_v5::DataFrameOptions::new(request.v5_body_channel()),
                )
                .unwrap(),
            );
        }
        frames.push(protocol_v5::Frame::new(
            protocol_v5::FrameType::EndStream,
            stream_id,
        ));
        frames
    }

    fn v5_protobuf_request_frames<M>(
        stream_id: u64,
        method: &str,
        payload: &M,
    ) -> Vec<protocol_v5::Frame>
    where
        M: ProstMessage,
    {
        v5_protobuf_request_frames_with_options(
            stream_id,
            method,
            payload,
            protocol_v5::RequestOptions::default(),
        )
    }

    fn v5_protobuf_request_frames_with_options<M>(
        stream_id: u64,
        method: &str,
        payload: &M,
        options: protocol_v5::RequestOptions,
    ) -> Vec<protocol_v5::Frame>
    where
        M: ProstMessage,
    {
        let headers = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::request_with_options(stream_id, method, &options),
        );
        let payload = protocol_v5::stream_data_frame(
            stream_id,
            payload.encode_to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap();
        vec![
            headers,
            payload,
            protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, stream_id),
        ]
    }

    fn v5_json_request_frames<T>(
        stream_id: u64,
        method: &str,
        payload: &T,
    ) -> Vec<protocol_v5::Frame>
    where
        T: Serialize,
    {
        let headers = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::request(stream_id, method),
        );
        let payload = protocol_v5::stream_data_frame(
            stream_id,
            serde_json::to_vec(payload).unwrap(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap();
        vec![
            headers,
            payload,
            protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, stream_id),
        ]
    }

    fn read_v5_frames(bytes: Vec<u8>) -> Vec<protocol_v5::Frame> {
        let mut cursor = Cursor::new(bytes);
        let mut frames = Vec::new();
        while let Some(frame) = protocol_v5::read_frame(&mut cursor).unwrap() {
            frames.push(frame);
        }
        frames
    }

    fn read_v5_complete_frames(bytes: Vec<u8>) -> Vec<protocol_v5::Frame> {
        let mut cursor = Cursor::new(bytes);
        let mut frames = Vec::new();
        while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
            frames.push(frame);
        }
        frames
    }

    fn assert_v5_data_channel_priority(
        frames: &[protocol_v5::Frame],
        stream_id: u64,
        channel: protocol_v5::DataChannel,
        priority: protocol_v5::Priority,
    ) {
        let matching_frames = frames
            .iter()
            .filter(|frame| {
                if frame.stream_id != stream_id || frame.frame_type != protocol_v5::FrameType::Data
                {
                    return false;
                }
                let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                protocol_v5::DataChannel::try_from(envelope.channel).unwrap() == channel
            })
            .collect::<Vec<_>>();

        assert!(
            !matching_frames.is_empty(),
            "expected {channel:?} DATA on stream {stream_id}"
        );
        assert!(
            matching_frames
                .iter()
                .all(|frame| frame.priority == priority.as_u8()),
            "{channel:?} DATA did not preserve {priority:?} priority"
        );
    }

    fn v5_response_frames(
        stream_id: u64,
        method: &str,
        response: RemoteResponse,
        body: Vec<u8>,
    ) -> Vec<protocol_v5::Frame> {
        v5_response_frames_with_content_encoding(
            stream_id,
            method,
            response,
            body,
            protocol_v5::ContentEncoding::None,
        )
    }

    fn v5_response_frames_with_content_encoding(
        stream_id: u64,
        method: &str,
        response: RemoteResponse,
        body: Vec<u8>,
        content_encoding: protocol_v5::ContentEncoding,
    ) -> Vec<protocol_v5::Frame> {
        let payload = response.to_v5_payload().unwrap();
        let mut frames = vec![
            protocol_v5::stream_data_frame(
                stream_id,
                payload,
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified)
                    .with_content_encoding(content_encoding),
            )
            .unwrap(),
        ];
        if !body.is_empty() {
            let channel = if matches!(response, RemoteResponse::ReadFile(_)) {
                protocol_v5::DataChannel::FileBody
            } else {
                protocol_v5::DataChannel::Stdout
            };
            frames.push(
                protocol_v5::stream_data_frame(
                    stream_id,
                    body,
                    protocol_v5::DataFrameOptions::new(channel)
                        .with_content_encoding(content_encoding),
                )
                .unwrap(),
            );
        }
        frames.push(protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::response(
                stream_id,
                method,
                protocol_v5::MessageRole::FinalResponse,
                true,
            ),
        ));
        frames.push(protocol_v5::Frame::new(
            protocol_v5::FrameType::EndStream,
            stream_id,
        ));
        frames
    }

    fn v5_raw_response_frames(
        stream_id: u64,
        method: &str,
        payload: Vec<u8>,
    ) -> Vec<protocol_v5::Frame> {
        let mut frames = Vec::new();
        if !payload.is_empty() {
            frames.push(
                protocol_v5::stream_data_frame(
                    stream_id,
                    payload,
                    protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
                )
                .unwrap(),
            );
        }
        frames.push(protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::response(
                stream_id,
                method,
                protocol_v5::MessageRole::FinalResponse,
                true,
            ),
        ));
        frames.push(protocol_v5::Frame::new(
            protocol_v5::FrameType::EndStream,
            stream_id,
        ));
        frames
    }

    fn v5_watch_event_open_frame(event_stream_id: u64, watch_id: u64) -> protocol_v5::Frame {
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            event_stream_id,
            &protocol_v5::StreamEnvelope::event(event_stream_id, "watch.batch", watch_id),
        )
    }

    fn decode_v5_service_response(
        frames: &[protocol_v5::Frame],
        stream_id: u64,
    ) -> (
        Option<RemoteResponse>,
        Vec<u8>,
        Option<protocol_v5::ErrorHeader>,
    ) {
        let mut method = None;
        let mut payload = Vec::new();
        let mut body = Vec::new();
        let mut error = None;

        for frame in frames.iter().filter(|frame| frame.stream_id == stream_id) {
            match frame.frame_type {
                protocol_v5::FrameType::Headers => {
                    let envelope = frame
                        .decode_control::<protocol_v5::StreamEnvelope>()
                        .unwrap();
                    match envelope.message {
                        Some(protocol_v5::stream_envelope::Message::Response(_)) => {
                            method = Some(envelope.method);
                        }
                        Some(protocol_v5::stream_envelope::Message::Error(header)) => {
                            method = Some(envelope.method);
                            error = Some(header);
                        }
                        _ => {}
                    }
                }
                protocol_v5::FrameType::Data => {
                    let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                    let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
                    match channel {
                        protocol_v5::DataChannel::Unspecified => {
                            payload.extend_from_slice(&frame.body)
                        }
                        protocol_v5::DataChannel::SearchPayload => {}
                        protocol_v5::DataChannel::FileBody
                        | protocol_v5::DataChannel::Stdout
                        | protocol_v5::DataChannel::Stderr
                        | protocol_v5::DataChannel::Stdin => body.extend_from_slice(&frame.body),
                    }
                }
                _ => {}
            }
        }

        let response = method
            .as_deref()
            .filter(|_| !payload.is_empty())
            .map(|method| RemoteResponse::from_v5_payload(method, &payload).unwrap());
        (response, body, error)
    }

    fn decode_v5_partial_file_search_responses(
        frames: &[protocol_v5::Frame],
        stream_id: u64,
    ) -> Vec<FileSearchResponse> {
        let mut partial_payload_next = false;
        let mut partials = Vec::new();

        for frame in frames.iter().filter(|frame| frame.stream_id == stream_id) {
            match frame.frame_type {
                protocol_v5::FrameType::Headers => {
                    let envelope = frame
                        .decode_control::<protocol_v5::StreamEnvelope>()
                        .unwrap();
                    partial_payload_next = envelope.role
                        == protocol_v5::MessageRole::PartialResult as i32
                        && envelope.method == "search.files";
                }
                protocol_v5::FrameType::Data if partial_payload_next => {
                    let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                    let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
                    if channel == protocol_v5::DataChannel::SearchPayload {
                        let response =
                            RemoteResponse::from_v5_payload("search.files", &frame.body).unwrap();
                        let RemoteResponse::FileSearch(search) = response else {
                            panic!("expected file search partial response");
                        };
                        partials.push(search);
                        partial_payload_next = false;
                    }
                }
                _ => {}
            }
        }

        partials
    }

    fn decode_v5_partial_text_search_responses(
        frames: &[protocol_v5::Frame],
        stream_id: u64,
    ) -> Vec<TextSearchResponse> {
        let mut partial_payload_next = false;
        let mut partials = Vec::new();

        for frame in frames.iter().filter(|frame| frame.stream_id == stream_id) {
            match frame.frame_type {
                protocol_v5::FrameType::Headers => {
                    let envelope = frame
                        .decode_control::<protocol_v5::StreamEnvelope>()
                        .unwrap();
                    partial_payload_next = envelope.role
                        == protocol_v5::MessageRole::PartialResult as i32
                        && envelope.method == "search.text";
                }
                protocol_v5::FrameType::Data if partial_payload_next => {
                    let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                    let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
                    if channel == protocol_v5::DataChannel::SearchPayload {
                        let response =
                            RemoteResponse::from_v5_payload("search.text", &frame.body).unwrap();
                        let RemoteResponse::TextSearch(search) = response else {
                            panic!("expected text search partial response");
                        };
                        partials.push(search);
                        partial_payload_next = false;
                    }
                }
                _ => {}
            }
        }

        partials
    }

    fn decode_v5_progress_headers(
        frames: &[protocol_v5::Frame],
        stream_id: u64,
        method: &str,
    ) -> Vec<protocol_v5::Progress> {
        frames
            .iter()
            .filter(|frame| {
                frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::Headers
            })
            .filter_map(|frame| {
                let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
                if envelope.role != protocol_v5::MessageRole::Progress as i32
                    || envelope.method != method
                {
                    return None;
                }
                match envelope.message {
                    Some(protocol_v5::stream_envelope::Message::Progress(progress)) => {
                        Some(progress)
                    }
                    _ => None,
                }
            })
            .collect()
    }

    #[cfg(unix)]
    fn v5_data_for_channel(
        frames: &[protocol_v5::Frame],
        stream_id: u64,
        expected_channel: protocol_v5::DataChannel,
    ) -> Vec<u8> {
        let mut data = Vec::new();
        for frame in frames.iter().filter(|frame| {
            frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::Data
        }) {
            let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
            let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
            if channel == expected_channel {
                data.extend_from_slice(&frame.body);
            }
        }
        data
    }

    #[cfg(unix)]
    fn find_v5_output_data_for_channel(
        output: &SharedWrite,
        stream_id: u64,
        expected_channel: protocol_v5::DataChannel,
    ) -> Vec<u8> {
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        let mut data = Vec::new();
        while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
            if frame.stream_id != stream_id || frame.frame_type != protocol_v5::FrameType::Data {
                continue;
            }
            let Ok(envelope) = frame.decode_control::<protocol_v5::DataEnvelope>() else {
                continue;
            };
            if protocol_v5::DataChannel::try_from(envelope.channel).ok() == Some(expected_channel) {
                data.extend_from_slice(&frame.body);
            }
        }
        data
    }

    #[cfg(unix)]
    fn v5_first_data_channel_index(
        frames: &[protocol_v5::Frame],
        stream_id: u64,
        expected_channel: protocol_v5::DataChannel,
    ) -> Option<usize> {
        frames.iter().position(|frame| {
            if frame.stream_id != stream_id || frame.frame_type != protocol_v5::FrameType::Data {
                return false;
            }
            let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
            protocol_v5::DataChannel::try_from(envelope.channel).unwrap() == expected_channel
        })
    }

    fn v5_write_temp_files(parent: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(parent)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with(".nucleotide-write-"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn decode_v5_protobuf_service_response<T>(
        frames: &[protocol_v5::Frame],
        stream_id: u64,
    ) -> (Option<T>, Option<protocol_v5::ErrorHeader>)
    where
        T: ProstMessage + Default,
    {
        let mut payload = Vec::new();
        let mut saw_response = false;
        let mut error = None;

        for frame in frames.iter().filter(|frame| frame.stream_id == stream_id) {
            match frame.frame_type {
                protocol_v5::FrameType::Headers => {
                    let envelope = frame
                        .decode_control::<protocol_v5::StreamEnvelope>()
                        .unwrap();
                    match envelope.message {
                        Some(protocol_v5::stream_envelope::Message::Response(_)) => {
                            saw_response = true;
                        }
                        Some(protocol_v5::stream_envelope::Message::Error(header)) => {
                            error = Some(header);
                        }
                        _ => {}
                    }
                }
                protocol_v5::FrameType::Data => {
                    let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                    let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
                    if channel == protocol_v5::DataChannel::Unspecified {
                        payload.extend_from_slice(&frame.body);
                    }
                }
                _ => {}
            }
        }

        let response = saw_response.then(|| T::decode(payload.as_slice()).unwrap());
        (response, error)
    }

    fn find_v5_watch_start_response(
        output: &SharedWrite,
        stream_id: u64,
    ) -> Option<protocol_v5::WatchStartResponse> {
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        let mut payload = Vec::new();
        let mut saw_response = false;
        while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
            if frame.stream_id != stream_id {
                continue;
            }
            match frame.frame_type {
                protocol_v5::FrameType::Headers => {
                    let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
                    if matches!(
                        envelope.message,
                        Some(protocol_v5::stream_envelope::Message::Response(_))
                    ) {
                        saw_response = true;
                    }
                }
                protocol_v5::FrameType::Data => {
                    let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().ok()?;
                    if protocol_v5::DataChannel::try_from(envelope.channel).ok()?
                        == protocol_v5::DataChannel::Unspecified
                    {
                        payload.extend_from_slice(&frame.body);
                    }
                }
                _ => {}
            }
        }
        (saw_response && !payload.is_empty())
            .then(|| protocol_v5::WatchStartResponse::decode(payload.as_slice()).ok())
            .flatten()
    }

    fn find_v5_watch_batch(
        output: &SharedWrite,
        event_stream_id: u64,
    ) -> Option<protocol_v5::WatchBatch> {
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
            if frame.stream_id != event_stream_id
                || frame.frame_type != protocol_v5::FrameType::Headers
            {
                continue;
            }
            let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
            let Some(protocol_v5::stream_envelope::Message::Event(event)) = envelope.message else {
                continue;
            };
            if event.kind == "watch.batch"
                && let Some(batch) = event.watch_batch
            {
                return Some(batch);
            }
        }
        None
    }

    fn find_v5_watch_batch_in_frames(
        frames: &[protocol_v5::Frame],
        event_stream_id: u64,
    ) -> Option<protocol_v5::WatchBatch> {
        for frame in frames {
            if frame.stream_id != event_stream_id
                || frame.frame_type != protocol_v5::FrameType::Headers
            {
                continue;
            }
            let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
            let Some(protocol_v5::stream_envelope::Message::Event(event)) = envelope.message else {
                continue;
            };
            if event.kind == "watch.batch"
                && let Some(batch) = event.watch_batch
            {
                return Some(batch);
            }
        }
        None
    }

    fn v5_final_response_index(frames: &[protocol_v5::Frame], stream_id: u64) -> usize {
        frames
            .iter()
            .position(|frame| {
                if frame.stream_id != stream_id
                    || frame.frame_type != protocol_v5::FrameType::Headers
                {
                    return false;
                }
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                matches!(
                    envelope.message,
                    Some(protocol_v5::stream_envelope::Message::Response(_))
                ) && envelope.role == protocol_v5::MessageRole::FinalResponse as i32
            })
            .unwrap_or_else(|| panic!("missing final response for stream {stream_id}"))
    }

    #[test]
    fn v5_method_payload_round_trips_existing_one_shot_requests() {
        let requests = vec![
            RemoteRequest::Stat {
                path: PathBuf::from("src/lib.rs"),
            },
            RemoteRequest::ListDirs {
                paths: vec![PathBuf::from("."), PathBuf::from("crates")],
            },
            RemoteRequest::FindAncestorFile {
                start: PathBuf::from("crates/nucleotide-remote/src"),
                file_name: "Cargo.toml".to_string(),
                limit: 4,
            },
            RemoteRequest::RenamePath {
                from: PathBuf::from("old.rs"),
                to: PathBuf::from("new.rs"),
            },
            RemoteRequest::ReadFile {
                path: PathBuf::from("README.md"),
                max_bytes: Some(4096),
            },
            RemoteRequest::WriteFile {
                path: PathBuf::from("src/main.rs"),
                create_parent_dirs: true,
                expected_modified_unix_millis: Some(123),
                expected_modified_unix_nanos: Some(456),
            },
            RemoteRequest::FileSearch(FileSearchRequest {
                pattern: Some("lib".to_string()),
                limit: 25,
                ..FileSearchRequest::default()
            }),
            RemoteRequest::TextSearch(TextSearchRequest {
                pattern: "needle".to_string(),
                limit: 10,
                ..TextSearchRequest::default()
            }),
            RemoteRequest::GitStatus {
                root: PathBuf::new(),
                include_untracked: true,
                limit: 99,
            },
            RemoteRequest::RunProcess(ProcessRequest {
                program: "printf".to_string(),
                args: vec!["hello".to_string()],
                cwd: PathBuf::new(),
                env: BTreeMap::from([("LANG".to_string(), "C".to_string())]),
                clear_env: true,
                inherit_project_environment: false,
                max_output_bytes: Some(1024),
                timeout_ms: Some(250),
            }),
            RemoteRequest::Shutdown,
        ];

        for request in requests {
            let (method, payload) = request.to_v5_method_payload().unwrap();
            let decoded = RemoteRequest::from_v5_method_payload(method, &payload).unwrap();
            assert_eq!(decoded, request, "{method}");
        }
    }

    #[test]
    fn v5_request_payloads_serialize_remote_paths_with_posix_separators() {
        let windows_style_path = PathBuf::from(r"\home\iheggie\projects");
        let request = RemoteRequest::ListDir {
            path: windows_style_path.clone(),
        };
        let (method, payload) = request.to_v5_method_payload().unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();

        assert_eq!(method, "fs.list_dir");
        assert_eq!(payload["path"], "/home/iheggie/projects");

        let cached_payload = V5DirectoryListPayload {
            path: windows_style_path.clone(),
            known_generation: Some(1),
            known_fingerprint: Some(2),
        };
        let payload = serde_json::to_value(cached_payload).unwrap();
        assert_eq!(payload["path"], "/home/iheggie/projects");

        let search = FileSearchRequest {
            root: windows_style_path.clone(),
            excluded_relative_prefixes: vec![PathBuf::from(r"target\generated")],
            ..FileSearchRequest::default()
        };
        let payload = serde_json::to_value(search).unwrap();
        assert_eq!(payload["root"], "/home/iheggie/projects");
        assert_eq!(payload["excluded_relative_prefixes"][0], "target/generated");

        let process = ProcessRequest {
            program: "pwd".to_string(),
            args: Vec::new(),
            cwd: windows_style_path,
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        };
        let payload = serde_json::to_value(process).unwrap();
        assert_eq!(payload["cwd"], "/home/iheggie/projects");
    }

    #[test]
    fn v5_request_options_classify_priority_idempotency_and_body_channel() {
        let write = RemoteRequest::WriteFile {
            path: PathBuf::from("src/lib.rs"),
            create_parent_dirs: false,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };
        let write_options = write.v5_request_options();
        assert_eq!(
            write_options.idempotency,
            protocol_v5::Idempotency::Mutation
        );
        assert_eq!(write_options.priority, protocol_v5::Priority::UserInput);
        assert_eq!(write.v5_body_channel(), protocol_v5::DataChannel::FileBody);
        assert!(!write.v5_retry_after_reconnect_allowed());

        let list_dirs = RemoteRequest::ListDirs {
            paths: vec![PathBuf::from(".")],
        };
        assert_eq!(
            list_dirs.v5_request_options().priority,
            protocol_v5::Priority::VisibleFileTree
        );
        assert!(list_dirs.v5_retry_after_reconnect_allowed());

        let search = RemoteRequest::TextSearch(TextSearchRequest {
            pattern: "main".to_string(),
            ..TextSearchRequest::default()
        });
        let search_options = search.v5_request_options();
        assert_eq!(search_options.priority, protocol_v5::Priority::Background);
        assert_eq!(search_options.cancellation_group, "search.text");
        assert_eq!(
            search.v5_body_channel(),
            protocol_v5::DataChannel::SearchPayload
        );
        assert!(search.v5_retry_after_reconnect_allowed());

        let read = RemoteRequest::ReadFile {
            path: PathBuf::from("src/lib.rs"),
            max_bytes: None,
        };
        assert_eq!(
            read.v5_request_options().priority,
            protocol_v5::Priority::ForegroundDocument
        );

        let environment = RemoteRequest::ProjectEnvironment {
            root: PathBuf::from("."),
        };
        assert_eq!(
            environment.v5_request_options().priority,
            protocol_v5::Priority::LspSupport
        );

        assert_eq!(
            RemoteRequest::Shutdown.v5_request_options().priority,
            protocol_v5::Priority::UserInput
        );

        let process = RemoteRequest::RunProcess(ProcessRequest {
            program: "cat".to_string(),
            args: Vec::new(),
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        });
        assert_eq!(
            process.v5_request_options().idempotency,
            protocol_v5::Idempotency::Process
        );
        assert_eq!(process.v5_body_channel(), protocol_v5::DataChannel::Stdin);
        assert!(!process.v5_retry_after_reconnect_allowed());
        assert!(!RemoteRequest::Shutdown.v5_retry_after_reconnect_allowed());
    }

    #[test]
    fn v5_request_deadline_policy_covers_every_method() {
        let metadata = (
            Some(V5_REQUEST_METADATA_DEADLINE),
            Some(V5_REQUEST_METADATA_INACTIVITY),
        );
        let mutation = (
            Some(V5_REQUEST_MUTATION_DEADLINE),
            Some(V5_REQUEST_MUTATION_INACTIVITY),
        );
        let file = (
            Some(V5_REQUEST_FILE_DEADLINE),
            Some(V5_REQUEST_FILE_INACTIVITY),
        );
        let search = (
            Some(V5_REQUEST_SEARCH_DEADLINE),
            Some(V5_REQUEST_SEARCH_INACTIVITY),
        );
        let control = (
            Some(V5_REQUEST_CONTROL_DEADLINE),
            Some(V5_REQUEST_CONTROL_INACTIVITY),
        );
        let requests = vec![
            (RemoteRequest::Stat { path: "a".into() }, metadata),
            (RemoteRequest::ListDir { path: "a".into() }, metadata),
            (
                RemoteRequest::ListDirs {
                    paths: vec!["a".into()],
                },
                metadata,
            ),
            (
                RemoteRequest::FindAncestorFile {
                    start: "a".into(),
                    file_name: "Cargo.toml".to_string(),
                    limit: 8,
                },
                metadata,
            ),
            (RemoteRequest::CreateFile { path: "a".into() }, mutation),
            (RemoteRequest::CreateDir { path: "a".into() }, mutation),
            (
                RemoteRequest::RenamePath {
                    from: "a".into(),
                    to: "b".into(),
                },
                mutation,
            ),
            (RemoteRequest::DeletePath { path: "a".into() }, mutation),
            (
                RemoteRequest::CopyPath {
                    from: "a".into(),
                    to: "b".into(),
                },
                mutation,
            ),
            (
                RemoteRequest::ReadFile {
                    path: "a".into(),
                    max_bytes: None,
                },
                file,
            ),
            (
                RemoteRequest::WriteFile {
                    path: "a".into(),
                    create_parent_dirs: false,
                    expected_modified_unix_millis: None,
                    expected_modified_unix_nanos: None,
                },
                file,
            ),
            (
                RemoteRequest::FileSearch(FileSearchRequest::default()),
                search,
            ),
            (
                RemoteRequest::TextSearch(TextSearchRequest {
                    pattern: "needle".to_string(),
                    ..TextSearchRequest::default()
                }),
                search,
            ),
            (
                RemoteRequest::ProjectEnvironment { root: "a".into() },
                mutation,
            ),
            (RemoteRequest::GitHead { root: "a".into() }, metadata),
            (
                RemoteRequest::GitStatus {
                    root: "a".into(),
                    include_untracked: true,
                    limit: 10,
                },
                metadata,
            ),
            (RemoteRequest::Shutdown, control),
        ];
        let created_at = Instant::now();
        let now_unix_ms = 1_000_000;

        for (request, (absolute_timeout, inactivity_timeout)) in requests {
            let policy = request.v5_deadline_policy();
            assert_eq!(policy.absolute_timeout, absolute_timeout, "{request:?}");
            assert_eq!(policy.inactivity_timeout, inactivity_timeout, "{request:?}");
            let context = RemoteRequestContext::from_policy_at(policy, created_at, now_unix_ms);
            assert_eq!(
                context.absolute_deadline,
                absolute_timeout.map(|timeout| created_at + timeout),
                "{request:?}"
            );
            assert_eq!(
                context.inactivity_timeout, inactivity_timeout,
                "{request:?}"
            );
            assert_eq!(
                request
                    .v5_request_options_with_context(context)
                    .deadline_unix_ms,
                now_unix_ms + u64::try_from(absolute_timeout.unwrap().as_millis()).unwrap(),
                "{request:?}"
            );
        }

        let bounded_process = RemoteRequest::RunProcess(ProcessRequest {
            program: "sleep".to_string(),
            args: Vec::new(),
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: Some(2_500),
        });
        let bounded_policy = bounded_process.v5_deadline_policy();
        assert_eq!(
            bounded_policy.absolute_timeout,
            Some(Duration::from_millis(2_500) + V5_REQUEST_PROCESS_CANCELLATION_GRACE)
        );
        assert_eq!(bounded_policy.inactivity_timeout, None);

        let unlimited_process = RemoteRequest::RunProcess(ProcessRequest {
            program: "server".to_string(),
            args: Vec::new(),
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        });
        let unlimited = RemoteRequestContext::from_policy_at(
            unlimited_process.v5_deadline_policy(),
            created_at,
            now_unix_ms,
        );
        assert_eq!(unlimited.absolute_deadline, None);
        assert_eq!(unlimited.deadline_unix_ms, 0);
        assert_eq!(unlimited.inactivity_timeout, None);

        let watch_control = v5_watch_control_request_context();
        assert_eq!(
            watch_control.absolute_deadline,
            watch_control
                .created_at
                .checked_add(V5_REQUEST_CONTROL_DEADLINE)
        );
        assert_eq!(
            watch_control.inactivity_timeout,
            Some(V5_REQUEST_CONTROL_INACTIVITY)
        );
        assert_ne!(watch_control.deadline_unix_ms, 0);
    }

    #[test]
    fn v5_request_progress_extends_only_inactivity() {
        let started = Instant::now();
        let context = RemoteRequestContext::from_policy_at(
            RemoteRequestDeadlinePolicy::bounded(Duration::from_secs(60), Duration::from_secs(30)),
            started,
            1_000,
        );
        let mut deadline = V5RequestDeadline::new(context, started);

        assert_eq!(
            deadline.next_expiry(),
            Some((
                started + Duration::from_secs(30),
                RemoteRequestDeadlineKind::Inactivity
            ))
        );
        deadline.observe_progress(started + Duration::from_secs(20));
        assert_eq!(
            deadline.next_expiry(),
            Some((
                started + Duration::from_secs(50),
                RemoteRequestDeadlineKind::Inactivity
            ))
        );
        deadline.observe_progress(started + Duration::from_secs(50));
        assert_eq!(
            deadline.next_expiry(),
            Some((
                started + Duration::from_secs(60),
                RemoteRequestDeadlineKind::Absolute
            ))
        );
        assert_eq!(
            deadline.expired_at(started + Duration::from_secs(60)),
            Some(RemoteRequestDeadlineKind::Absolute)
        );
    }

    #[test]
    fn v5_inbound_request_progress_is_stream_scoped() {
        let targeted = [
            protocol_v5::RoutedFrame::WindowUpdate {
                stream_id: 7,
                credit_bytes: 1,
            },
            protocol_v5::RoutedFrame::Headers {
                stream_id: 7,
                role: protocol_v5::MessageRole::Progress,
                method: "fs.stat".to_string(),
            },
            protocol_v5::RoutedFrame::Data {
                stream_id: 7,
                flow_control_len: 1,
            },
            protocol_v5::RoutedFrame::EndStream {
                stream_id: 7,
                state: protocol_v5::StreamState::Closed,
            },
            protocol_v5::RoutedFrame::ResetStream {
                stream_id: 7,
                known: true,
            },
        ];
        for routed in targeted {
            assert_eq!(v5_client_inbound_progress_stream(&routed), Some(7));
        }

        let unrelated = [
            protocol_v5::RoutedFrame::ConnectionControl {
                frame_type: protocol_v5::FrameType::Ping,
            },
            protocol_v5::RoutedFrame::WindowUpdate {
                stream_id: 0,
                credit_bytes: 1,
            },
            protocol_v5::RoutedFrame::RejectedStream { stream_id: 7 },
        ];
        for routed in unrelated {
            assert_eq!(v5_client_inbound_progress_stream(&routed), None);
        }
    }

    #[test]
    fn v5_client_heartbeat_queues_once_and_correlates_exact_pong() {
        let started = Instant::now();
        let mut heartbeat = v5_test_client_heartbeat(started);
        let first_token = 1_u64.to_be_bytes().to_vec();

        assert_eq!(
            heartbeat
                .next_action(started + Duration::from_millis(20))
                .unwrap(),
            V5ClientHeartbeatAction::QueuePing(first_token.clone())
        );
        assert_eq!(
            heartbeat
                .next_action(started + Duration::from_millis(21))
                .unwrap(),
            V5ClientHeartbeatAction::Wait(Duration::from_millis(49))
        );

        let ping = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Ping,
            0,
            &protocol_v5::PingPayload {
                token: first_token.clone(),
            },
        );
        heartbeat
            .mark_ping_started(&ping, started + Duration::from_millis(22))
            .unwrap();
        let pong = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Pong,
            0,
            &protocol_v5::PingPayload { token: first_token },
        );
        assert_eq!(
            heartbeat
                .observe_inbound(
                    pong.frame_type,
                    Some(pong.control.clone()),
                    started + Duration::from_millis(27),
                )
                .unwrap(),
            Some(Duration::from_millis(5))
        );
        assert_eq!(
            heartbeat
                .next_action(started + Duration::from_millis(47))
                .unwrap(),
            V5ClientHeartbeatAction::QueuePing(2_u64.to_be_bytes().to_vec())
        );

        let mut active = v5_test_client_heartbeat(started);
        active
            .observe_inbound(
                protocol_v5::FrameType::Ping,
                None,
                started + Duration::from_millis(15),
            )
            .unwrap();
        assert_eq!(
            active
                .next_action(started + Duration::from_millis(20))
                .unwrap(),
            V5ClientHeartbeatAction::Wait(Duration::from_millis(15))
        );
    }

    #[test]
    fn v5_client_heartbeat_requires_matching_pong_before_timeout() {
        let started = Instant::now();
        let mut heartbeat = v5_test_client_heartbeat(started);
        let token = match heartbeat
            .next_action(started + Duration::from_millis(20))
            .unwrap()
        {
            V5ClientHeartbeatAction::QueuePing(token) => token,
            action => panic!("expected heartbeat PING, got {action:?}"),
        };
        let ping = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Ping,
            0,
            &protocol_v5::PingPayload {
                token: token.clone(),
            },
        );
        let ping_started = started + Duration::from_millis(22);
        heartbeat.mark_ping_started(&ping, ping_started).unwrap();

        let unrelated = protocol_v5::Frame::new(protocol_v5::FrameType::GoAway, 0);
        assert_eq!(
            heartbeat
                .observe_inbound(
                    unrelated.frame_type,
                    None,
                    started + Duration::from_millis(30),
                )
                .unwrap(),
            None
        );
        let wrong_pong = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Pong,
            0,
            &protocol_v5::PingPayload {
                token: b"wrong".to_vec(),
            },
        );
        let error = heartbeat
            .observe_inbound(
                wrong_pong.frame_type,
                Some(wrong_pong.control.clone()),
                started + Duration::from_millis(31),
            )
            .unwrap_err();
        assert!(error.to_string().contains("unexpected heartbeat token"));
        let mut altered_control = protocol_v5::PingPayload { token }.encode_to_vec();
        altered_control.extend_from_slice(&[0x78, 0x01]);
        let error = heartbeat
            .observe_inbound(
                protocol_v5::FrameType::Pong,
                Some(altered_control),
                started + Duration::from_millis(32),
            )
            .unwrap_err();
        assert!(error.to_string().contains("unexpected heartbeat token"));
        assert_eq!(
            heartbeat
                .next_action(ping_started + Duration::from_millis(50))
                .unwrap(),
            V5ClientHeartbeatAction::TimedOut(
                "v5 peer did not answer client idle PING before timeout"
            )
        );

        let mut unsolicited = v5_test_client_heartbeat(started);
        assert!(
            unsolicited
                .observe_inbound(
                    wrong_pong.frame_type,
                    Some(wrong_pong.control.clone()),
                    started,
                )
                .unwrap_err()
                .to_string()
                .contains("unsolicited")
        );

        let mut queued = v5_test_client_heartbeat(started);
        let queued_token = match queued
            .next_action(started + Duration::from_millis(20))
            .unwrap()
        {
            V5ClientHeartbeatAction::QueuePing(token) => token,
            action => panic!("expected queued heartbeat PING, got {action:?}"),
        };
        let queued_control = protocol_v5::PingPayload {
            token: queued_token,
        }
        .encode_to_vec();
        assert!(
            queued
                .observe_inbound(
                    protocol_v5::FrameType::Pong,
                    Some(queued_control),
                    started + Duration::from_millis(21),
                )
                .unwrap_err()
                .to_string()
                .contains("before the client heartbeat PING was written")
        );
        assert_eq!(
            queued
                .next_action(started + Duration::from_millis(70))
                .unwrap(),
            V5ClientHeartbeatAction::TimedOut(
                "v5 client writer did not send idle PING before timeout"
            )
        );
    }

    #[test]
    fn v5_client_heartbeat_deadlines_win_transition_races() {
        let started = Instant::now();
        let mut queued = v5_test_client_heartbeat(started);
        let token = match queued
            .next_action(started + Duration::from_millis(20))
            .unwrap()
        {
            V5ClientHeartbeatAction::QueuePing(token) => token,
            action => panic!("expected queued heartbeat PING, got {action:?}"),
        };
        let ping = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Ping,
            0,
            &protocol_v5::PingPayload {
                token: token.clone(),
            },
        );
        let error = queued
            .mark_ping_started(&ping, started + Duration::from_millis(70))
            .unwrap_err();
        let RemoteClientError::Io(error) = error else {
            panic!("expected queued heartbeat timeout, got {error:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("writer did not send"));
        assert!(matches!(queued.ping, Some(V5ClientPing::Queued { .. })));

        let mut outstanding = v5_test_client_heartbeat(started);
        let _ = outstanding
            .next_action(started + Duration::from_millis(20))
            .unwrap();
        outstanding
            .mark_ping_started(&ping, started + Duration::from_millis(21))
            .unwrap();
        let pong = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Pong,
            0,
            &protocol_v5::PingPayload { token },
        );
        let error = outstanding
            .observe_inbound(
                pong.frame_type,
                Some(pong.control),
                started + Duration::from_millis(71),
            )
            .unwrap_err();
        let RemoteClientError::Io(error) = error else {
            panic!("expected outstanding heartbeat timeout, got {error:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("peer did not answer"));
        assert!(matches!(
            outstanding.ping,
            Some(V5ClientPing::Outstanding { .. })
        ));
    }

    #[test]
    fn v5_client_heartbeat_normalizes_inconsistent_peer_settings() {
        let mut settings = protocol_v5::ConnectionSettings::recommended();
        settings.idle_ping_interval_ms = 20;
        settings.ping_timeout_ms = 0;
        settings.min_unsolicited_ping_interval_ms = 1;

        let heartbeat = V5ClientHeartbeat::new(&settings, Instant::now());

        assert_eq!(heartbeat.idle_ping_interval, Duration::from_millis(5_000));
        assert_eq!(
            heartbeat.ping_timeout,
            Duration::from_millis(u64::from(protocol_v5::PING_TIMEOUT_MS))
        );
    }

    #[test]
    fn v5_service_preserves_request_priority_on_response_frames() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("visible.txt"), b"visible").unwrap();
        let request = RemoteRequest::Stat {
            path: PathBuf::from("visible.txt"),
        };
        let mut options = request.v5_request_options();
        options.priority = protocol_v5::Priority::UserInput;
        let input = v5_client_input(v5_request_frames_with_options(1, &request, &[], options));
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let response_frames = read_v5_frames(output)
            .into_iter()
            .filter(|frame| frame.stream_id == 1)
            .collect::<Vec<_>>();

        assert!(!response_frames.is_empty());
        assert!(
            response_frames
                .iter()
                .all(|frame| { frame.priority == protocol_v5::Priority::UserInput.as_u8() })
        );
    }

    #[test]
    fn reconnecting_client_retries_read_only_request_after_disconnect() {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = FakeProtocolClient::new(calls.clone(), [FakeProtocolOutcome::Disconnected]);
        let reconnect_calls = calls.clone();
        let reconnect_count = reconnects.clone();
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(FakeProtocolClient::new(
                reconnect_calls.clone(),
                [FakeProtocolOutcome::Ok(RemoteResponse::FindAncestorFile(
                    None,
                ))],
            ))
        });
        let request = RemoteRequest::Stat {
            path: PathBuf::from("src/lib.rs"),
        };

        let (response, body) = client.request(request.clone(), Vec::new()).unwrap();

        assert_eq!(response, RemoteResponse::FindAncestorFile(None));
        assert!(body.is_empty());
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[request.clone(), request]
        );
    }

    #[test]
    fn reconnecting_client_does_not_replay_cancelled_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let reconnect_count = Arc::clone(&reconnects);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(
            CancelThenDisconnectProtocolClient {
                calls: Arc::clone(&calls),
            },
            move || {
                reconnect_count.fetch_add(1, Ordering::SeqCst);
                Ok(CancelThenDisconnectProtocolClient {
                    calls: Arc::new(AtomicUsize::new(0)),
                })
            },
        );
        let cancellation = RemoteRequestCancellation::new();

        let error = client
            .request_with_context_and_cancellation(
                RemoteRequest::Stat {
                    path: PathBuf::from("cancelled.rs"),
                },
                Vec::new(),
                RemoteRequest::Stat {
                    path: PathBuf::from("cancelled.rs"),
                }
                .v5_request_context(),
                &cancellation,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            RemoteClientError::Remote(RemoteError { ref code, .. })
                if code == protocol_v5::RESET_CANCELLED
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reconnecting_client_does_not_replay_when_cancelled_during_recovery() {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let cancellation = RemoteRequestCancellation::new();
        let reconnect_cancellation = cancellation.clone();
        let reconnect_calls = Arc::clone(&calls);
        let reconnect_count = Arc::clone(&reconnects);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(
            FakeProtocolClient::new(Arc::clone(&calls), [FakeProtocolOutcome::Disconnected]),
            move || {
                reconnect_count.fetch_add(1, Ordering::SeqCst);
                reconnect_cancellation.cancel();
                Ok(FakeProtocolClient::new(
                    Arc::clone(&reconnect_calls),
                    [FakeProtocolOutcome::Ok(RemoteResponse::Stat(
                        FileStatResponse {
                            path: PathBuf::from("cancelled.rs"),
                            kind: RemoteFileKind::File,
                            size: 0,
                            modified_unix_millis: None,
                            modified_unix_nanos: None,
                            readonly: false,
                        },
                    ))],
                ))
            },
        );
        let request = RemoteRequest::Stat {
            path: PathBuf::from("cancelled.rs"),
        };

        let error = client
            .request_with_context_and_cancellation(
                request.clone(),
                Vec::new(),
                request.v5_request_context(),
                &cancellation,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            RemoteClientError::Remote(RemoteError { ref code, .. })
                if code == protocol_v5::RESET_CANCELLED
        ));
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(calls.lock().unwrap().as_slice(), &[request]);
    }

    #[test]
    fn reconnecting_client_reuses_exact_context_for_safe_replay() {
        let contexts = Arc::new(StdMutex::new(Vec::new()));
        let closes = Arc::new(AtomicUsize::new(0));
        let initial = ContextRecordingProtocolClient::new(
            Arc::clone(&contexts),
            [ContextProtocolOutcome::Disconnected],
            Arc::clone(&closes),
        );
        let reconnect_contexts = Arc::clone(&contexts);
        let reconnect_closes = Arc::clone(&closes);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            Ok(ContextRecordingProtocolClient::new(
                Arc::clone(&reconnect_contexts),
                [ContextProtocolOutcome::Ok(
                    RemoteResponse::FindAncestorFile(None),
                )],
                Arc::clone(&reconnect_closes),
            ))
        });

        let response = client
            .request(
                RemoteRequest::Stat {
                    path: PathBuf::from("src/lib.rs"),
                },
                Vec::new(),
            )
            .unwrap();

        assert_eq!(
            response,
            (RemoteResponse::FindAncestorFile(None), Vec::new())
        );
        let contexts = contexts.lock().unwrap();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0], contexts[1]);
        assert_ne!(contexts[0].deadline_unix_ms, 0);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reconnecting_client_does_not_replay_after_original_deadline_expires() {
        let contexts = Arc::new(StdMutex::new(Vec::new()));
        let closes = Arc::new(AtomicUsize::new(0));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = ContextRecordingProtocolClient::new(
            Arc::clone(&contexts),
            [ContextProtocolOutcome::Disconnected],
            Arc::clone(&closes),
        );
        let reconnect_contexts = Arc::clone(&contexts);
        let reconnect_closes = Arc::clone(&closes);
        let reconnect_count = Arc::clone(&reconnects);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(40));
            Ok(ContextRecordingProtocolClient::new(
                Arc::clone(&reconnect_contexts),
                [ContextProtocolOutcome::Ok(
                    RemoteResponse::FindAncestorFile(None),
                )],
                Arc::clone(&reconnect_closes),
            ))
        });
        let context = RemoteRequestContext::from_policy(
            RemoteRequestDeadlinePolicy::absolute_only(Duration::from_millis(20)),
        );

        let error = client
            .request_with_context(
                RemoteRequest::Stat {
                    path: PathBuf::from("src/lib.rs"),
                },
                Vec::new(),
                context,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            RemoteClientError::RequestDeadlineExceeded {
                ref method,
                kind: RemoteRequestDeadlineKind::Absolute,
            } if method == "fs.stat"
        ));
        assert_eq!(contexts.lock().unwrap().as_slice(), &[context]);
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reconnecting_client_does_not_reconnect_stream_local_deadline() {
        let contexts = Arc::new(StdMutex::new(Vec::new()));
        let closes = Arc::new(AtomicUsize::new(0));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = ContextRecordingProtocolClient::new(
            Arc::clone(&contexts),
            [ContextProtocolOutcome::Deadline],
            Arc::clone(&closes),
        );
        let reconnect_count = Arc::clone(&reconnects);
        let reconnect_contexts = Arc::clone(&contexts);
        let reconnect_closes = Arc::clone(&closes);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(ContextRecordingProtocolClient::new(
                Arc::clone(&reconnect_contexts),
                [ContextProtocolOutcome::Ok(
                    RemoteResponse::FindAncestorFile(None),
                )],
                Arc::clone(&reconnect_closes),
            ))
        });

        let error = client
            .request(
                RemoteRequest::Stat {
                    path: PathBuf::from("src/lib.rs"),
                },
                Vec::new(),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            RemoteClientError::RequestDeadlineExceeded {
                kind: RemoteRequestDeadlineKind::Inactivity,
                ..
            }
        ));
        assert_eq!(contexts.lock().unwrap().len(), 1);
        assert_eq!(reconnects.load(Ordering::SeqCst), 0);
        assert_eq!(closes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reconnecting_client_invalidates_failed_replay_without_third_attempt() {
        let contexts = Arc::new(StdMutex::new(Vec::new()));
        let closes = Arc::new(AtomicUsize::new(0));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = ContextRecordingProtocolClient::new(
            Arc::clone(&contexts),
            [ContextProtocolOutcome::Disconnected],
            Arc::clone(&closes),
        );
        let reconnect_count = Arc::clone(&reconnects);
        let reconnect_contexts = Arc::clone(&contexts);
        let reconnect_closes = Arc::clone(&closes);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(ContextRecordingProtocolClient::new(
                Arc::clone(&reconnect_contexts),
                [ContextProtocolOutcome::Disconnected],
                Arc::clone(&reconnect_closes),
            ))
        });

        let error = client
            .request(
                RemoteRequest::Stat {
                    path: PathBuf::from("src/lib.rs"),
                },
                Vec::new(),
            )
            .unwrap_err();

        assert!(matches!(error, RemoteClientError::Disconnected));
        let contexts = contexts.lock().unwrap();
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[0], contexts[1]);
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(closes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reconnecting_client_heals_but_does_not_retry_mutation_after_disconnect() {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = FakeProtocolClient::new(calls.clone(), [FakeProtocolOutcome::Disconnected]);
        let reconnect_calls = calls.clone();
        let reconnect_count = reconnects.clone();
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(FakeProtocolClient::new(
                reconnect_calls.clone(),
                [FakeProtocolOutcome::Ok(RemoteResponse::FindAncestorFile(
                    None,
                ))],
            ))
        });
        let request = RemoteRequest::WriteFile {
            path: PathBuf::from("src/lib.rs"),
            create_parent_dirs: false,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };

        let error = client
            .request(request.clone(), b"body".to_vec())
            .unwrap_err();
        let next_request = RemoteRequest::Stat {
            path: PathBuf::from("src/lib.rs"),
        };
        let (response, _) = client.request(next_request.clone(), Vec::new()).unwrap();

        let RemoteClientError::OutcomeUnknown { method, .. } = error else {
            panic!("expected unknown mutation outcome");
        };
        assert_eq!(method, "fs.write");
        assert_eq!(response, RemoteResponse::FindAncestorFile(None));
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(calls.lock().unwrap().as_slice(), &[request, next_request]);
    }

    #[test]
    fn reconnecting_client_does_not_retry_remote_final_error() {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = FakeProtocolClient::new(
            calls.clone(),
            [FakeProtocolOutcome::RemoteError("PERMISSION_DENIED")],
        );
        let reconnect_count = reconnects.clone();
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(FakeProtocolClient::new(
                Arc::new(StdMutex::new(Vec::new())),
                [FakeProtocolOutcome::Ok(RemoteResponse::FindAncestorFile(
                    None,
                ))],
            ))
        });
        let request = RemoteRequest::Stat {
            path: PathBuf::from("src/lib.rs"),
        };

        let error = client.request(request.clone(), Vec::new()).unwrap_err();

        assert!(matches!(error, RemoteClientError::Remote(_)));
        assert_eq!(reconnects.load(Ordering::SeqCst), 0);
        assert_eq!(calls.lock().unwrap().as_slice(), &[request]);
    }

    #[test]
    fn reconnecting_client_replays_safe_reads_after_any_terminal_io_kind() {
        for kind in [
            io::ErrorKind::TimedOut,
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::NotConnected,
            io::ErrorKind::WriteZero,
            io::ErrorKind::Other,
        ] {
            let calls = Arc::new(StdMutex::new(Vec::new()));
            let reconnects = Arc::new(AtomicUsize::new(0));
            let initial =
                FakeProtocolClient::new(Arc::clone(&calls), [FakeProtocolOutcome::Io(kind)]);
            let reconnect_calls = Arc::clone(&calls);
            let reconnect_count = Arc::clone(&reconnects);
            let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
                reconnect_count.fetch_add(1, Ordering::SeqCst);
                Ok(FakeProtocolClient::new(
                    Arc::clone(&reconnect_calls),
                    [FakeProtocolOutcome::Ok(RemoteResponse::FindAncestorFile(
                        None,
                    ))],
                ))
            });
            let request = RemoteRequest::Stat {
                path: PathBuf::from("src/lib.rs"),
            };

            let (response, _) = client.request(request.clone(), Vec::new()).unwrap();

            assert_eq!(response, RemoteResponse::FindAncestorFile(None), "{kind:?}");
            assert_eq!(reconnects.load(Ordering::SeqCst), 1, "{kind:?}");
            assert_eq!(
                calls.lock().unwrap().as_slice(),
                &[request.clone(), request],
                "{kind:?}"
            );
        }
    }

    #[test]
    fn reconnecting_client_retries_watch_start_after_transport_healing() {
        let starts = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = WatchProtocolClient {
            starts: Arc::clone(&starts),
            closes: Arc::clone(&closes),
            fail_start: true,
        };
        let reconnect_starts = Arc::clone(&starts);
        let reconnect_closes = Arc::clone(&closes);
        let reconnect_count = Arc::clone(&reconnects);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(WatchProtocolClient {
                starts: Arc::clone(&reconnect_starts),
                closes: Arc::clone(&reconnect_closes),
                fail_start: false,
            })
        });

        let watch = client
            .start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from("src")]))
            .unwrap();

        assert!(watch.is_none());
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reconnecting_client_does_not_replay_watch_start_after_original_deadline_expires() {
        let starts = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = WatchProtocolClient {
            starts: Arc::clone(&starts),
            closes: Arc::clone(&closes),
            fail_start: true,
        };
        let reconnect_starts = Arc::clone(&starts);
        let reconnect_closes = Arc::clone(&closes);
        let reconnect_count = Arc::clone(&reconnects);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(40));
            Ok(WatchProtocolClient {
                starts: Arc::clone(&reconnect_starts),
                closes: Arc::clone(&reconnect_closes),
                fail_start: false,
            })
        });
        let context = RemoteRequestContext::from_policy(
            RemoteRequestDeadlinePolicy::absolute_only(Duration::from_millis(20)),
        );

        let error = match client.start_watch_with_context(
            WorkspaceWatchRequest::expanded_dirs([PathBuf::from("src")]),
            context,
        ) {
            Ok(_) => panic!("watch.start replay should not outlive its original deadline"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            RemoteClientError::RequestDeadlineExceeded {
                ref method,
                kind: RemoteRequestDeadlineKind::Absolute,
            } if method == "watch.start"
        ));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reconnecting_client_heals_watch_control_failures_without_replaying_them() {
        for failed_operation in [WatchControlFailure::Update, WatchControlFailure::Stop] {
            let updates = Arc::new(AtomicUsize::new(0));
            let stops = Arc::new(AtomicUsize::new(0));
            let closes = Arc::new(AtomicUsize::new(0));
            let reconnects = Arc::new(AtomicUsize::new(0));
            let initial = WatchControlProtocolClient {
                updates: Arc::clone(&updates),
                stops: Arc::clone(&stops),
                closes: Arc::clone(&closes),
                failed_operation: Some(failed_operation),
            };
            let reconnect_updates = Arc::clone(&updates);
            let reconnect_stops = Arc::clone(&stops);
            let reconnect_closes = Arc::clone(&closes);
            let reconnect_count = Arc::clone(&reconnects);
            let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
                reconnect_count.fetch_add(1, Ordering::SeqCst);
                Ok(WatchControlProtocolClient {
                    updates: Arc::clone(&reconnect_updates),
                    stops: Arc::clone(&reconnect_stops),
                    closes: Arc::clone(&reconnect_closes),
                    failed_operation: None,
                })
            });

            let error = match failed_operation {
                WatchControlFailure::Update => client
                    .update_watch(7, vec![PathBuf::from("src")], Vec::new())
                    .unwrap_err(),
                WatchControlFailure::Stop => client.stop_watch(7).unwrap_err(),
            };
            assert!(matches!(error, RemoteClientError::Io(_)));
            assert_eq!(reconnects.load(Ordering::SeqCst), 1);
            assert_eq!(closes.load(Ordering::SeqCst), 1);

            // The failed connection-scoped mutation is not replayed, while the healed transport
            // is immediately available to the next watch control operation.
            client
                .update_watch(8, vec![PathBuf::from("tests")], Vec::new())
                .unwrap();
            client.stop_watch(8).unwrap();
            assert_eq!(
                updates.load(Ordering::SeqCst),
                usize::from(failed_operation == WatchControlFailure::Update) + 1
            );
            assert_eq!(
                stops.load(Ordering::SeqCst),
                usize::from(failed_operation == WatchControlFailure::Stop) + 1
            );
        }
    }

    #[test]
    fn backend_drop_and_reconnecting_close_are_nonblocking_and_do_not_reconnect() {
        let backend_closes = Arc::new(AtomicUsize::new(0));
        let backend_shutdowns = Arc::new(AtomicUsize::new(0));
        let backend = RemoteWorkspaceBackendImpl::from_protocol_client(
            loopback_identity(),
            LifecycleProtocolClient {
                closes: Arc::clone(&backend_closes),
                shutdowns: Arc::clone(&backend_shutdowns),
            },
        );

        let started = Instant::now();
        drop(backend);

        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(backend_closes.load(Ordering::SeqCst), 1);
        assert_eq!(backend_shutdowns.load(Ordering::SeqCst), 0);

        let reconnect_closes = Arc::new(AtomicUsize::new(0));
        let reconnect_shutdowns = Arc::new(AtomicUsize::new(0));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let reconnect_count = Arc::clone(&reconnects);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(
            LifecycleProtocolClient {
                closes: Arc::clone(&reconnect_closes),
                shutdowns: Arc::clone(&reconnect_shutdowns),
            },
            move || {
                reconnect_count.fetch_add(1, Ordering::SeqCst);
                Ok(LifecycleProtocolClient {
                    closes: Arc::new(AtomicUsize::new(0)),
                    shutdowns: Arc::new(AtomicUsize::new(0)),
                })
            },
        );

        client.close();

        assert_eq!(reconnects.load(Ordering::SeqCst), 0);
        assert_eq!(reconnect_closes.load(Ordering::SeqCst), 1);
        assert!(matches!(
            client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("src/lib.rs")
                },
                Vec::new()
            ),
            Err(RemoteClientError::Disconnected)
        ));
        assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dropping_backend_request_future_cancels_and_releases_worker() {
        let state = Arc::new((
            StdMutex::new(CancellationObservingState::default()),
            Condvar::new(),
        ));
        let backend = RemoteWorkspaceBackendImpl::from_protocol_client(
            loopback_identity(),
            CancellationObservingProtocolClient {
                state: Arc::clone(&state),
            },
        );
        let mut request = Box::pin(backend.stat(Path::new("pending.rs")));
        let waker = futures::task::noop_waker();
        let mut context = std::task::Context::from_waker(&waker);

        assert!(
            std::future::Future::poll(request.as_mut(), &mut context).is_pending(),
            "the fake protocol request should block until cancellation"
        );
        wait_for_cancellation_observer(&state, |state| state.started, "worker start");

        drop(request);

        wait_for_cancellation_observer(&state, |state| state.finished, "worker cancellation");
        let state = state.0.lock().unwrap();
        assert!(state.cancelled);
    }

    #[test]
    fn dropping_backend_watch_start_closes_ambiguous_control_connection() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();
        let shared = Arc::clone(&client.shared);
        let backend = RemoteWorkspaceBackendImpl::new(loopback_identity(), client);
        let mut request = Box::pin(
            backend.start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from("src")])),
        );
        let waker = futures::task::noop_waker();
        let mut context = std::task::Context::from_waker(&waker);

        assert!(std::future::Future::poll(request.as_mut(), &mut context).is_pending());
        let stream_id = wait_for_v5_request_stream(&output, "watch.start");
        assert!(shared.raw_waiters.lock().unwrap().contains_key(&stream_id));

        drop(request);

        let started = Instant::now();
        loop {
            let closed = shared.closed.load(Ordering::Acquire);
            let cleaned = shared.raw_waiters.lock().unwrap().is_empty()
                && shared.pending_cancellations.lock().unwrap().is_empty()
                && shared.request_budget.used() == 0
                && Arc::strong_count(&backend.client) == 1;
            if closed && cleaned {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for dropped watch.start cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        drop(backend);
        input.close();
    }

    #[cfg(unix)]
    #[test]
    fn child_handshake_watchdog_physically_aborts_and_reaps_silent_helper() {
        let command = RemoteServiceCommand {
            program: OsString::from("/bin/sleep"),
            args: vec![OsString::from("60")],
            current_dir: None,
        };
        let (io, control) = spawn_child_process_v5_io(&command).unwrap();

        let result = connect_child_process_v5_client_with_timeout(
            io,
            Arc::clone(&control),
            protocol_v5::ClientHello::nucleotide("test-client"),
            Duration::from_millis(50),
        );

        assert!(result.is_err());
        let started = Instant::now();
        while !control.was_reaped() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for silent v5 child to be reaped"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn dropping_v5_request_handle_resets_stream_and_releases_response_budget() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let handle = client
            .start_request(
                RemoteRequest::Stat {
                    path: PathBuf::from("cancelled.rs"),
                },
                Vec::new(),
            )
            .unwrap();
        let cancelled_stream = handle.stream_id();
        wait_for_v5_stream_frame(&output, cancelled_stream, protocol_v5::FrameType::EndStream);
        input.push(v5_frames_bytes(vec![
            protocol_v5::stream_data_frame(
                cancelled_stream,
                vec![0; 64],
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
            )
            .unwrap(),
        ]));
        let started = Instant::now();
        while client.shared.response_budget.used() == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for partial response budget reservation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        drop(handle);

        wait_for_v5_stream_frame(
            &output,
            cancelled_stream,
            protocol_v5::FrameType::ResetStream,
        );
        wait_for_v5_outbound_request_reservation_release(&client.shared, cancelled_stream);
        let started = Instant::now();
        loop {
            let cleaned = client.shared.waiters.lock().unwrap().is_empty()
                && client
                    .shared
                    .pending_cancellations
                    .lock()
                    .unwrap()
                    .is_empty()
                && client.shared.response_budget.used() == 0;
            if cleaned {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for dropped request cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let resets = read_v5_complete_frames(output.bytes())
            .into_iter()
            .filter(|frame| {
                frame.stream_id == cancelled_stream
                    && frame.frame_type == protocol_v5::FrameType::ResetStream
            })
            .collect::<Vec<_>>();
        assert_eq!(resets.len(), 1);
        assert_eq!(
            resets[0]
                .decode_control::<protocol_v5::ResetStream>()
                .unwrap()
                .code,
            protocol_v5::RESET_CANCELLED
        );
        assert!(!client.shared.closed.load(Ordering::Acquire));

        let healthy_client = Arc::clone(&client);
        let healthy = std::thread::spawn(move || {
            healthy_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("healthy.rs"),
                },
                Vec::new(),
            )
        });
        let healthy_stream = wait_for_v5_request_stream_after(&output, "fs.stat", cancelled_stream);
        let response = RemoteResponse::Stat(FileStatResponse {
            path: PathBuf::from("healthy.rs"),
            kind: RemoteFileKind::File,
            size: 7,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        });
        input.push(v5_frames_bytes(v5_response_frames(
            healthy_stream,
            "fs.stat",
            response.clone(),
            Vec::new(),
        )));

        assert_eq!(healthy.join().unwrap().unwrap(), (response, Vec::new()));
        assert!(!client.shared.closed.load(Ordering::Acquire));
        client.close();
        input.close();
    }

    #[test]
    fn v5_client_reader_rejects_frame_sequence_gap_after_handshake() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let request_client = Arc::clone(&client);
        let request = std::thread::spawn(move || {
            request_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("pending.rs"),
                },
                Vec::new(),
            )
        });
        wait_for_v5_request_stream(&output, "fs.stat");

        let mut ping = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Ping,
            0,
            &protocol_v5::PingPayload {
                token: b"sequence-gap".to_vec(),
            },
        );
        ping.frame_sequence = 4;
        let mut bytes = Vec::new();
        protocol_v5::write_frame(&mut bytes, &ping).unwrap();
        input.push_raw(bytes);

        let error = request.join().unwrap().unwrap_err();
        let RemoteClientError::Io(error) = error else {
            panic!("expected sequence I/O error, got {error:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("expected 3, got 4"));
        assert!(client.shared.closed.load(Ordering::Acquire));
        input.close();
    }

    #[test]
    fn v5_concurrent_service_reader_rejects_frame_sequence_gap_after_handshake() {
        let temp = tempfile::tempdir().unwrap();
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_client_input(Vec::new()));
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let info = protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string());
        let service_input = input.clone();
        let service_thread = std::thread::spawn(move || {
            service.serve_v5_concurrent(protocol_v5::FramedIo::new(service_input, output), &info)
        });

        let mut ping = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Ping,
            0,
            &protocol_v5::PingPayload {
                token: b"sequence-gap".to_vec(),
            },
        );
        ping.frame_sequence = 4;
        let mut bytes = Vec::new();
        protocol_v5::write_frame(&mut bytes, &ping).unwrap();
        input.push_raw(bytes);

        let error = service_thread.join().unwrap().unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("failed to read v5 protocol frame"));
        assert!(message.contains("expected 3, got 4"), "{message}");
        input.close();
    }

    #[test]
    fn v5_client_heartbeat_uses_negotiated_timing_and_exact_pong() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        let client_hello = v5_heartbeat_client_hello(Duration::from_millis(500));
        input.push(v5_server_input_for_client(
            Vec::new(),
            &client_hello,
            protocol_v5::ServerHandshakeInfo::current("/workspace"),
        ));
        let client = RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            client_hello,
        )
        .unwrap();

        {
            let heartbeat = client.shared.heartbeat.lock().unwrap();
            assert_eq!(
                heartbeat.idle_ping_interval,
                Duration::from_millis(u64::from(protocol_v5::IDLE_PING_INTERVAL_MS))
            );
            assert_eq!(heartbeat.ping_timeout, Duration::from_millis(500));
        }
        trigger_v5_client_idle_ping(&client.shared);

        let first_ping =
            wait_for_v5_connection_frame_after(&output, protocol_v5::FrameType::Ping, 2);
        let first_payload = first_ping
            .decode_control::<protocol_v5::PingPayload>()
            .unwrap();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(
            read_v5_frames(output.bytes())
                .into_iter()
                .filter(|frame| frame.frame_type == protocol_v5::FrameType::Ping)
                .count(),
            1,
            "only one heartbeat may be outstanding"
        );

        input.push(v5_frames_bytes(vec![protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Pong,
            0,
            &first_payload,
        )]));
        trigger_v5_client_idle_ping(&client.shared);
        let second_ping = wait_for_v5_connection_frame_after(
            &output,
            protocol_v5::FrameType::Ping,
            first_ping.frame_sequence,
        );
        let second_payload = second_ping
            .decode_control::<protocol_v5::PingPayload>()
            .unwrap();
        assert_ne!(second_payload.token, first_payload.token);
        assert!(!client.shared.closed.load(Ordering::Acquire));

        client.close();
        input.close();
    }

    #[test]
    fn v5_client_wrong_pong_is_connection_terminal() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        let abort_calls = Arc::new(AtomicUsize::new(0));
        let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
            calls: Arc::clone(&abort_calls),
        });
        let client_hello = v5_heartbeat_client_hello(Duration::from_millis(500));
        input.push(v5_server_input_for_client(
            Vec::new(),
            &client_hello,
            protocol_v5::ServerHandshakeInfo::current("/workspace"),
        ));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                client_hello,
                Some(abort),
            )
            .unwrap(),
        );
        let request_client = Arc::clone(&client);
        let (result_sender, result_receiver) = mpsc::channel();
        let request = std::thread::spawn(move || {
            let result = request_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("pending.rs"),
                },
                Vec::new(),
            );
            result_sender.send(result).unwrap();
        });
        wait_for_v5_request_stream(&output, "fs.stat");
        trigger_v5_client_idle_ping(&client.shared);
        let _ping = wait_for_v5_connection_frame_after(&output, protocol_v5::FrameType::Ping, 2);

        input.push(v5_frames_bytes(vec![protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Pong,
            0,
            &protocol_v5::PingPayload {
                token: b"wrong".to_vec(),
            },
        )]));

        let error = result_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("wrong PONG should fail the pending request")
            .unwrap_err();
        let RemoteClientError::TransportClosed { cause } = error else {
            panic!("expected terminal heartbeat protocol error, got {error:?}");
        };
        assert!(cause.contains("unexpected heartbeat token"), "{cause}");
        assert!(client.shared.closed.load(Ordering::Acquire));
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        client.close();
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        input.close();
        request.join().unwrap();
    }

    #[test]
    fn v5_client_missing_pong_times_out_waiter_and_aborts_once() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        let abort_calls = Arc::new(AtomicUsize::new(0));
        let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
            calls: Arc::clone(&abort_calls),
        });
        let client_hello = v5_heartbeat_client_hello(Duration::from_millis(80));
        input.push(v5_server_input_for_client(
            Vec::new(),
            &client_hello,
            protocol_v5::ServerHandshakeInfo::current("/workspace"),
        ));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                client_hello,
                Some(abort),
            )
            .unwrap(),
        );
        let request_client = Arc::clone(&client);
        let (result_sender, result_receiver) = mpsc::channel();
        let request = std::thread::spawn(move || {
            let result = request_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("pending.rs"),
                },
                Vec::new(),
            );
            result_sender.send(result).unwrap();
        });
        wait_for_v5_request_stream(&output, "fs.stat");
        trigger_v5_client_idle_ping(&client.shared);
        let _ping = wait_for_v5_connection_frame_after(&output, protocol_v5::FrameType::Ping, 2);

        let error = result_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("missing PONG should fail the pending request")
            .unwrap_err();
        let RemoteClientError::Io(error) = error else {
            panic!("expected heartbeat timeout, got {error:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("peer did not answer"));
        assert!(client.shared.closed.load(Ordering::Acquire));
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        client.close();
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        input.close();
        request.join().unwrap();
    }

    #[test]
    fn v5_client_heartbeat_aborts_writer_stalled_before_ping() {
        let input = BlockingRead::default();
        let writer = PausingWrite::default();
        let abort_calls = Arc::new(AtomicUsize::new(0));
        let abort: Arc<dyn V5TransportAbort> = Arc::new(ReleasingTransportAbort {
            writer: writer.clone(),
            calls: Arc::clone(&abort_calls),
        });
        let client_hello = v5_heartbeat_client_hello(Duration::from_millis(80));
        input.push(v5_server_input_for_client(
            Vec::new(),
            &client_hello,
            protocol_v5::ServerHandshakeInfo::current("/workspace"),
        ));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
                protocol_v5::FramedIo::new(input.clone(), writer.clone()),
                client_hello,
                Some(abort),
            )
            .unwrap(),
        );
        writer.pause_next_write();
        let request_client = Arc::clone(&client);
        let (result_sender, result_receiver) = mpsc::channel();
        let request = std::thread::spawn(move || {
            let result = request_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("blocked.rs"),
                },
                Vec::new(),
            );
            result_sender.send(result).unwrap();
        });
        writer.wait_until_paused();
        trigger_v5_client_idle_ping(&client.shared);

        let error = result_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("queued heartbeat should time out a stalled writer")
            .unwrap_err();
        let RemoteClientError::Io(error) = error else {
            panic!("expected stalled-writer heartbeat timeout, got {error:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("writer did not send"));
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        client.close();
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        input.close();
        request.join().unwrap();
    }

    #[test]
    fn v5_client_reader_routes_responses_while_writer_is_blocked() {
        let input = BlockingRead::default();
        let writer = PausingWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), writer.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        writer.pause_next_write();

        let request_client = Arc::clone(&client);
        let (result_sender, result_receiver) = mpsc::channel();
        let request = std::thread::spawn(move || {
            let result = request_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("responsive.rs"),
                },
                Vec::new(),
            );
            result_sender.send(result).unwrap();
        });
        writer.wait_until_paused();

        let ping = protocol_v5::PingPayload {
            token: b"reader-remains-responsive".to_vec(),
        };
        input.push(v5_frames_bytes(vec![protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Ping,
            0,
            &ping,
        )]));
        let response = RemoteResponse::Stat(FileStatResponse {
            path: PathBuf::from("responsive.rs"),
            kind: RemoteFileKind::File,
            size: 1,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        });
        input.push(v5_frames_bytes(v5_response_frames(
            1,
            "fs.stat",
            response.clone(),
            Vec::new(),
        )));

        assert_eq!(
            result_receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("client reader should not wait for the blocked writer")
                .unwrap(),
            (response, Vec::new())
        );

        writer.release();
        request.join().unwrap();
        let started = Instant::now();
        loop {
            let pong = read_v5_frames(writer.bytes())
                .into_iter()
                .find(|frame| frame.frame_type == protocol_v5::FrameType::Pong);
            if let Some(pong) = pong {
                assert_eq!(
                    pong.decode_control::<protocol_v5::PingPayload>().unwrap(),
                    ping
                );
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for writer-pump PONG"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        client.close();
        input.close();
    }

    #[test]
    fn v5_writer_failures_close_transport_and_fail_every_waiter() {
        enum Failure {
            AfterBytes(usize),
            AfterFlushes(usize),
        }

        let second_request = RemoteRequest::Stat {
            path: PathBuf::from("second.rs"),
        };
        let second_frames = v5_request_frames(3, &second_request, &[]);
        let first_frame_len = protocol_v5::FRAME_HEADER_LEN
            + second_frames[0].control.len()
            + second_frames[0].body.len();
        let partial_data_body =
            first_frame_len + protocol_v5::FRAME_HEADER_LEN + second_frames[1].control.len() + 1;
        assert!(second_frames[1].body.len() > 1);

        let failures = [
            ("before header", Failure::AfterBytes(0)),
            ("inside header", Failure::AfterBytes(7)),
            (
                "inside headers control",
                Failure::AfterBytes(protocol_v5::FRAME_HEADER_LEN + 1),
            ),
            ("inside data body", Failure::AfterBytes(partial_data_body)),
            ("data flush", Failure::AfterFlushes(0)),
        ];

        for (label, failure) in failures {
            let input = BlockingRead::default();
            let writer = FaultInjectingWrite::default();
            let output = writer.output();
            input.push(v5_server_input(Vec::new()));
            let client = Arc::new(
                RemoteWorkspaceV5MultiplexedClient::connect(
                    protocol_v5::FramedIo::new(input.clone(), writer.clone()),
                    protocol_v5::ClientHello::nucleotide("test-client"),
                )
                .unwrap(),
            );
            let handshake_flushes = writer.successful_flush_count();

            let first_client = Arc::clone(&client);
            let first = std::thread::spawn(move || {
                first_client.request(
                    RemoteRequest::Stat {
                        path: PathBuf::from("first.rs"),
                    },
                    Vec::new(),
                )
            });
            wait_for_v5_request_stream(&output, "fs.stat");
            writer.wait_for_successful_flush_after(handshake_flushes);

            let (watch_sender, watch_receiver) = mpsc::sync_channel(1);
            client.shared.watch_batches.lock().unwrap().insert(
                2,
                V5WatchDelivery {
                    sender: watch_sender,
                    overflowed: Arc::new(AtomicBool::new(false)),
                    last_sequence: Arc::new(AtomicU64::new(0)),
                },
            );
            client
                .shared
                .watch_stream_by_id
                .lock()
                .unwrap()
                .insert(1, 2);

            match failure {
                Failure::AfterBytes(bytes) => writer.fail_after_bytes(bytes),
                Failure::AfterFlushes(flushes) => writer.fail_after_flushes(flushes),
            }
            let second_error = client
                .request(second_request.clone(), Vec::new())
                .unwrap_err();
            let first_error = first.join().unwrap().unwrap_err();

            for error in [first_error, second_error] {
                let RemoteClientError::Io(error) = error else {
                    panic!("{label}: expected I/O failure, got {error:?}");
                };
                assert_eq!(error.kind(), io::ErrorKind::BrokenPipe, "{label}");
            }
            assert!(client.shared.closed.load(Ordering::Acquire), "{label}");
            assert_eq!(client.shared.request_budget.used(), 0, "{label}");
            assert!(
                client
                    .shared
                    .outbound_request_reservations
                    .lock()
                    .unwrap()
                    .is_empty(),
                "{label}"
            );
            assert!(client.shared.waiters.lock().unwrap().is_empty(), "{label}");
            assert!(
                client.shared.raw_waiters.lock().unwrap().is_empty(),
                "{label}"
            );
            assert!(
                client.shared.watch_batches.lock().unwrap().is_empty(),
                "{label}"
            );
            assert!(
                client.shared.watch_stream_by_id.lock().unwrap().is_empty(),
                "{label}"
            );
            assert!(matches!(
                watch_receiver.try_recv(),
                Err(mpsc::TryRecvError::Disconnected)
            ));

            let bytes_after_failure = output.bytes().len();
            assert!(matches!(
                client.request(
                    RemoteRequest::Stat {
                        path: PathBuf::from("third.rs"),
                    },
                    Vec::new()
                ),
                Err(RemoteClientError::Disconnected)
            ));
            assert_eq!(output.bytes().len(), bytes_after_failure, "{label}");
            input.close();
        }
    }

    #[test]
    fn v5_close_aborts_transport_without_waiting_for_blocked_writer() {
        let input = BlockingRead::default();
        let writer = PausingWrite::default();
        let abort_calls = Arc::new(AtomicUsize::new(0));
        let abort: Arc<dyn V5TransportAbort> = Arc::new(ReleasingTransportAbort {
            writer: writer.clone(),
            calls: Arc::clone(&abort_calls),
        });
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
                protocol_v5::FramedIo::new(input.clone(), writer.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
                Some(abort),
            )
            .unwrap(),
        );
        writer.pause_next_write();
        let request_client = Arc::clone(&client);
        let request = std::thread::spawn(move || {
            request_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("blocked.rs"),
                },
                Vec::new(),
            )
        });
        writer.wait_until_paused();
        assert!(client.shared.request_budget.used() > 0);
        assert!(
            !client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .is_empty()
        );

        let started = Instant::now();
        client.close();

        assert!(started.elapsed() < Duration::from_millis(250));
        client.close();
        assert_eq!(abort_calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.shared.request_budget.used(), 0);
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            request.join().unwrap(),
            Err(RemoteClientError::Disconnected)
        ));
        input.close();
    }

    #[test]
    fn v5_writer_revalidates_extracted_frames_after_local_reset() {
        let input = BlockingRead::default();
        let writer = PausingWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), writer.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        writer.pause_next_write();
        let request_client = Arc::clone(&client);
        let request = std::thread::spawn(move || {
            request_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("stale.rs"),
                },
                Vec::new(),
            )
        });
        writer.wait_until_paused();
        assert!(client.shared.request_budget.used() > 0);
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .contains_key(&1)
        );
        assert!(
            client
                .shared
                .session
                .lock()
                .unwrap()
                .reset_stream(1, protocol_v5::RESET_CANCELLED, "test reset")
                .unwrap()
        );
        writer.release();
        let started = Instant::now();
        loop {
            let bytes = writer.bytes();
            let mut cursor = Cursor::new(bytes);
            let mut reset_seen = false;
            while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
                reset_seen |=
                    frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream;
            }
            if reset_seen {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for locally reset v5 stream"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_for_v5_outbound_request_reservation_release(&client.shared, 1);
        assert_eq!(client.shared.request_budget.used(), 0);

        let stream_frames = read_v5_frames(writer.bytes())
            .into_iter()
            .filter(|frame| frame.stream_id == 1)
            .collect::<Vec<_>>();
        assert!(
            stream_frames
                .iter()
                .any(|frame| frame.frame_type == protocol_v5::FrameType::Headers)
        );
        assert!(
            stream_frames
                .iter()
                .any(|frame| frame.frame_type == protocol_v5::FrameType::ResetStream)
        );
        assert!(stream_frames.iter().all(|frame| {
            !matches!(
                frame.frame_type,
                protocol_v5::FrameType::Data | protocol_v5::FrameType::EndStream
            )
        }));

        client.close();
        assert!(matches!(
            request.join().unwrap(),
            Err(RemoteClientError::Disconnected)
        ));
        input.close();
    }

    #[test]
    fn pending_response_disconnect_before_final_response_remains_retryable() {
        let (sender, _receiver) = mpsc::channel();
        let response_budget = V5ConnectionByteBudget::new(1);
        let pending = V5PendingResponse {
            sender,
            accumulator: V5ResponseAccumulator::default(),
            response_reservation: response_budget.reservation(),
            method: "fs.stat",
            idempotency: protocol_v5::Idempotency::ReadOnly,
            terminal_on_deadline: false,
            deadline: V5RequestDeadline::new(
                RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
                Instant::now(),
            ),
        };

        let error = pending.failure_error(RemoteClientError::Disconnected);

        assert!(matches!(error, RemoteClientError::Disconnected));
        assert!(remote_client_error_allows_reconnect_retry(&error));
    }

    #[test]
    fn pending_mutation_disconnect_reports_unknown_outcome() {
        let (sender, _receiver) = mpsc::channel();
        let response_budget = V5ConnectionByteBudget::new(1);
        let pending = V5PendingResponse {
            sender,
            accumulator: V5ResponseAccumulator::default(),
            response_reservation: response_budget.reservation(),
            method: "fs.write",
            idempotency: protocol_v5::Idempotency::Mutation,
            terminal_on_deadline: true,
            deadline: V5RequestDeadline::new(
                RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
                Instant::now(),
            ),
        };

        let error = pending.failure_error(RemoteClientError::Disconnected);

        assert!(matches!(
            error,
            RemoteClientError::OutcomeUnknown { ref method, .. } if method == "fs.write"
        ));
        assert!(!remote_client_error_allows_reconnect_retry(&error));
        assert!(remote_client_error_requires_reconnect(&error));
    }

    #[test]
    fn pending_response_disconnect_after_final_response_is_not_retryable() {
        let (sender, _receiver) = mpsc::channel();
        let response_budget = V5ConnectionByteBudget::new(1);
        let mut pending = V5PendingResponse {
            sender,
            accumulator: V5ResponseAccumulator::default(),
            response_reservation: response_budget.reservation(),
            method: "fs.stat",
            idempotency: protocol_v5::Idempotency::ReadOnly,
            terminal_on_deadline: false,
            deadline: V5RequestDeadline::new(
                RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
                Instant::now(),
            ),
        };
        pending.accumulator.method = Some("fs.stat".to_string());

        let error = pending.failure_error(RemoteClientError::Disconnected);

        assert!(matches!(
            error,
            RemoteClientError::ResponseIncomplete { .. }
        ));
        assert!(!remote_client_error_allows_reconnect_retry(&error));
        assert!(remote_client_error_requires_reconnect(&error));
    }

    #[test]
    fn pending_raw_response_disconnect_after_final_error_is_not_retryable() {
        let (sender, _receiver) = mpsc::channel();
        let response_budget = V5ConnectionByteBudget::new(1);
        let mut pending = V5PendingRawResponse {
            sender,
            accumulator: V5RawResponseAccumulator::default(),
            response_reservation: response_budget.reservation(),
            method: "watch.start",
            deadline: V5RequestDeadline::new(
                RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
                Instant::now(),
            ),
        };
        pending.accumulator.final_error = Some(RemoteError {
            code: "UNAVAILABLE".to_string(),
            message: "remote closed".to_string(),
            diagnostic: None,
        });

        let error = pending.failure_error(RemoteClientError::Disconnected);

        assert!(matches!(
            error,
            RemoteClientError::ResponseIncomplete { .. }
        ));
        assert!(!remote_client_error_allows_reconnect_retry(&error));
        assert!(remote_client_error_requires_reconnect(&error));
    }

    #[test]
    fn peer_deadline_maps_reads_mutations_and_watch_controls_safely() {
        fn deadline_error() -> RemoteClientError {
            RemoteClientError::Remote(RemoteError {
                code: protocol_v5::RESET_DEADLINE_EXCEEDED.to_string(),
                message: "deadline expired".to_string(),
                diagnostic: None,
            })
        }

        let response_budget = V5ConnectionByteBudget::new(1);
        let (sender, _receiver) = mpsc::channel();
        let read = V5PendingResponse {
            sender,
            accumulator: V5ResponseAccumulator::default(),
            response_reservation: response_budget.reservation(),
            method: "fs.stat",
            idempotency: protocol_v5::Idempotency::ReadOnly,
            terminal_on_deadline: false,
            deadline: V5RequestDeadline::new(
                RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
                Instant::now(),
            ),
        };
        let normalized = normalize_v5_response_deadline(&read, Err(deadline_error()));
        assert!(matches!(
            normalized.result,
            Err(RemoteClientError::RequestDeadlineExceeded {
                kind: RemoteRequestDeadlineKind::Absolute,
                ..
            })
        ));
        assert!(normalized.peer_deadline);
        assert!(!normalized.terminal);

        let (sender, _receiver) = mpsc::channel();
        let mut mutation = V5PendingResponse {
            sender,
            accumulator: V5ResponseAccumulator::default(),
            response_reservation: response_budget.reservation(),
            method: "fs.write",
            idempotency: protocol_v5::Idempotency::Mutation,
            terminal_on_deadline: true,
            deadline: V5RequestDeadline::new(
                RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
                Instant::now(),
            ),
        };
        let normalized = normalize_v5_response_deadline(&mutation, Err(deadline_error()));
        assert!(matches!(
            normalized.result,
            Err(RemoteClientError::OutcomeUnknown { ref method, .. }) if method == "fs.write"
        ));
        assert!(normalized.peer_deadline);
        assert!(normalized.terminal);

        mutation.accumulator.method = Some("fs.write".to_string());
        let normalized = normalize_v5_response_deadline(&mutation, Err(deadline_error()));
        assert!(matches!(
            normalized.result,
            Err(RemoteClientError::RequestDeadlineExceeded {
                kind: RemoteRequestDeadlineKind::Absolute,
                ..
            })
        ));
        assert!(normalized.peer_deadline);
        assert!(!normalized.terminal);

        let (sender, _receiver) = mpsc::channel();
        let mut shutdown = V5PendingResponse {
            sender,
            accumulator: V5ResponseAccumulator::default(),
            response_reservation: response_budget.reservation(),
            method: "session.shutdown",
            idempotency: protocol_v5::Idempotency::Mutation,
            terminal_on_deadline: true,
            deadline: V5RequestDeadline::new(
                RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
                Instant::now(),
            ),
        };
        shutdown.accumulator.method = Some("session.shutdown".to_string());
        assert!(shutdown.deadline_is_connection_terminal());
        let normalized = normalize_v5_response_deadline(&shutdown, Err(deadline_error()));
        assert!(matches!(
            normalized.result,
            Err(RemoteClientError::OutcomeUnknown { ref method, .. })
                if method == "session.shutdown"
        ));
        assert!(normalized.peer_deadline);
        assert!(normalized.terminal);

        let (sender, _receiver) = mpsc::channel();
        let raw = V5PendingRawResponse {
            sender,
            accumulator: V5RawResponseAccumulator::default(),
            response_reservation: response_budget.reservation(),
            method: "watch.start",
            deadline: V5RequestDeadline::new(
                RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
                Instant::now(),
            ),
        };
        let normalized = normalize_v5_raw_response_deadline(&raw, Err(deadline_error()));
        assert!(matches!(
            normalized.result,
            Err(RemoteClientError::Io(ref error)) if error.kind() == io::ErrorKind::TimedOut
        ));
        assert!(normalized.peer_deadline);
        assert!(normalized.terminal);
    }

    #[test]
    fn v5_service_task_pools_bound_classes_and_skip_blocked_front() {
        fn request(method: &str) -> V5ServiceRequest {
            let budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
            V5ServiceRequest {
                method: method.to_string(),
                priority: protocol_v5::Priority::Background,
                payload: Vec::new(),
                body: Vec::new(),
                retained_bytes: budget.reservation(),
                received_payload_bytes: 0,
                received_body_bytes: 0,
                deadline_unix_ms: 0,
                supersedes_stream_id: 0,
                streamed_write: None,
                early_error: None,
            }
        }

        let mut pools = V5ServiceTaskPools::default();
        for _ in 0..V5_SEARCH_WORKER_LIMIT {
            assert!(pools.can_start_method("search.text"));
            assert_eq!(
                pools.mark_started("search.text"),
                V5ServiceTaskClass::Search
            );
        }
        assert!(!pools.can_start_method("search.text"));

        pools.enqueue(1, request("search.text"));
        pools.enqueue(3, request("fs.stat"));
        let (stream_id, ready) = pools.pop_next_startable().unwrap();
        assert_eq!(stream_id, 3);
        assert_eq!(ready.method, "fs.stat");

        pools.mark_finished(V5ServiceTaskClass::Search);
        let (stream_id, ready) = pools.pop_next_startable().unwrap();
        assert_eq!(stream_id, 1);
        assert_eq!(ready.method, "search.text");

        let mut expired = request("git.status");
        let now_unix_ms = v5_now_unix_millis();
        expired.deadline_unix_ms = now_unix_ms.saturating_sub(1);
        pools.enqueue(5, expired);
        assert_eq!(pools.expired_pending_streams(now_unix_ms), vec![5]);
        assert!(pools.remove_pending(5));
        assert!(!pools.has_pending());

        pools.enqueue(7, request("fs.stat"));
        let mut urgent = request("fs.stat");
        urgent.priority = protocol_v5::Priority::UserInput;
        pools.enqueue(9, urgent);
        assert_eq!(pools.pop_next_startable().unwrap().0, 9);
        assert_eq!(pools.pop_next_startable().unwrap().0, 7);
    }

    #[test]
    fn v5_service_request_rejects_aggregate_decoded_data_over_limit() {
        let budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
        let mut request = V5ServiceRequest::from_envelope(
            protocol_v5::StreamEnvelope::request(1, "search.text"),
            protocol_v5::Priority::Background,
            &budget,
        );
        request.received_payload_bytes = V5_MAX_REQUEST_PAYLOAD_BYTES;

        let error = request
            .reserve_data(protocol_v5::DataChannel::SearchPayload, 1, true)
            .unwrap_err();

        assert_eq!(error.code, "resource_exhausted");
        assert!(
            error
                .message
                .contains("request payload exceeds decoded byte limit")
        );
    }

    #[test]
    fn v5_service_requests_compete_for_connection_budget_and_release_on_drop() {
        let budget = V5ConnectionByteBudget::new(10);
        let mut first = V5ServiceRequest::from_envelope(
            protocol_v5::StreamEnvelope::request(1, "fs.stat"),
            protocol_v5::Priority::Background,
            &budget,
        );
        let mut second = V5ServiceRequest::from_envelope(
            protocol_v5::StreamEnvelope::request(3, "fs.stat"),
            protocol_v5::Priority::Background,
            &budget,
        );

        first
            .reserve_data(protocol_v5::DataChannel::Unspecified, 6, true)
            .unwrap();
        let error = second
            .reserve_data(protocol_v5::DataChannel::Unspecified, 5, true)
            .unwrap_err();
        assert_eq!(error.code, "resource_exhausted");
        assert_eq!(budget.used(), 6);

        drop(first);
        second
            .reserve_data(protocol_v5::DataChannel::Unspecified, 5, true)
            .unwrap();
        assert_eq!(budget.used(), 5);
    }

    #[test]
    fn v5_streamed_file_body_counts_stream_limit_without_retaining_connection_budget() {
        let budget = V5ConnectionByteBudget::new(1);
        let mut request = V5ServiceRequest::from_envelope(
            protocol_v5::StreamEnvelope::request(1, "fs.write"),
            protocol_v5::Priority::Background,
            &budget,
        );

        request
            .reserve_data(protocol_v5::DataChannel::FileBody, 1024, false)
            .unwrap();

        assert_eq!(request.received_body_bytes, 1024);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn v5_client_responses_compete_for_connection_budget_and_release_on_drop() {
        let budget = V5ConnectionByteBudget::new(10);
        let mut first_reservation = budget.reservation();
        let mut second_reservation = budget.reservation();
        let mut first = V5ResponseAccumulator::default();
        let mut second = V5ResponseAccumulator::default();

        assert!(
            first
                .observe_with_reservation(
                    protocol_v5::StreamEvent::Data {
                        stream_id: 1,
                        channel: protocol_v5::DataChannel::FileBody,
                        uncompressed_len: 6,
                        body: vec![0; 6],
                    },
                    &mut first_reservation,
                )
                .is_none()
        );
        let error = second
            .observe_with_reservation(
                protocol_v5::StreamEvent::Data {
                    stream_id: 3,
                    channel: protocol_v5::DataChannel::FileBody,
                    uncompressed_len: 5,
                    body: vec![0; 5],
                },
                &mut second_reservation,
            )
            .expect("connection budget should reject the second response")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("connection retained-byte budget")
        );
        assert_eq!(budget.used(), 6);

        drop(first_reservation);
        assert!(
            second
                .observe_with_reservation(
                    protocol_v5::StreamEvent::Data {
                        stream_id: 3,
                        channel: protocol_v5::DataChannel::FileBody,
                        uncompressed_len: 5,
                        body: vec![0; 5],
                    },
                    &mut second_reservation,
                )
                .is_none()
        );
        assert_eq!(budget.used(), 5);
    }

    #[test]
    fn v5_client_requests_share_budget_validate_stream_limits_and_release() {
        let budget = V5ConnectionByteBudget::new(10);
        let first = reserve_v5_client_request_bytes(&budget, "fs.write", 4, 2).unwrap();
        let error = reserve_v5_client_request_bytes(&budget, "fs.write", 3, 2).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("connection retained-byte budget")
        );
        assert_eq!(budget.used(), 6);

        drop(first);
        let second = reserve_v5_client_request_bytes(&budget, "fs.write", 3, 2).unwrap();
        assert_eq!(budget.used(), 5);
        drop(second);
        assert_eq!(budget.used(), 0);

        let normal_budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
        let payload_error = reserve_v5_client_request_bytes(
            &normal_budget,
            "fs.stat",
            V5_MAX_REQUEST_PAYLOAD_BYTES + 1,
            0,
        )
        .unwrap_err();
        assert!(payload_error.to_string().contains("request payload"));
        let body_error = reserve_v5_client_request_bytes(
            &normal_budget,
            "fs.write",
            0,
            V5_MAX_REQUEST_BODY_BYTES + 1,
        )
        .unwrap_err();
        assert!(body_error.to_string().contains("request body"));
        assert_eq!(normal_budget.used(), 0);
    }

    #[test]
    fn v5_client_does_not_open_request_after_deadline_while_waiting_for_session() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let context = RemoteRequestContext::from_policy(
            RemoteRequestDeadlinePolicy::absolute_only(Duration::from_secs(2)),
        );
        let session = client.shared.session.lock().unwrap();
        let request_client = Arc::clone(&client);
        let worker = std::thread::spawn(move || {
            request_client.request_with_context(
                RemoteRequest::Stat {
                    path: PathBuf::from("late.rs"),
                },
                Vec::new(),
                context,
            )
        });

        let started = Instant::now();
        while client.shared.request_budget.used() == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "request did not reach the session lock before its deadline"
            );
            std::thread::yield_now();
        }
        std::thread::sleep(
            context
                .absolute_deadline
                .unwrap()
                .saturating_duration_since(Instant::now())
                + Duration::from_millis(10),
        );
        drop(session);

        let error = worker.join().unwrap().unwrap_err();
        assert!(matches!(
            error,
            RemoteClientError::RequestDeadlineExceeded {
                ref method,
                kind: RemoteRequestDeadlineKind::Absolute,
            } if method == "fs.stat"
        ));
        assert!(find_v5_request_stream(&output, "fs.stat").is_none());
        assert_eq!(client.shared.request_budget.used(), 0);
        client.close();
        input.close();
    }

    #[test]
    fn v5_client_does_not_open_watch_control_after_deadline_while_waiting_for_session() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let context = RemoteRequestContext::from_policy(
            RemoteRequestDeadlinePolicy::absolute_only(Duration::from_secs(2)),
        );
        let session = client.shared.session.lock().unwrap();
        let request_client = Arc::clone(&client);
        let worker = std::thread::spawn(move || {
            request_client.request_v5_raw("watch.stop", vec![1, 2, 3], context)
        });

        let started = Instant::now();
        while client.shared.request_budget.used() == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "watch control did not reach the session lock before its deadline"
            );
            std::thread::yield_now();
        }
        std::thread::sleep(
            context
                .absolute_deadline
                .unwrap()
                .saturating_duration_since(Instant::now())
                + Duration::from_millis(10),
        );
        drop(session);

        let error = worker.join().unwrap().unwrap_err();
        assert!(matches!(
            error,
            RemoteClientError::RequestDeadlineExceeded {
                ref method,
                kind: RemoteRequestDeadlineKind::Absolute,
            } if method == "watch.stop"
        ));
        assert!(find_v5_request_stream(&output, "watch.stop").is_none());
        assert_eq!(client.shared.request_budget.used(), 0);
        client.close();
        input.close();
    }

    #[test]
    fn v5_early_response_keeps_request_body_reserved_until_outbound_end_is_written() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let request = RemoteRequest::WriteFile {
            path: PathBuf::from("large.txt"),
            create_parent_dirs: false,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };
        let body_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
            + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
        let body = vec![7; body_len];
        let (_, encoded_payload) = request.to_v5_method_payload().unwrap();
        let retained_bytes = encoded_payload.len() + body_len;
        let request_client = Arc::clone(&client);
        let request_for_thread = request.clone();
        let worker = std::thread::spawn(move || request_client.request(request_for_thread, body));

        let stream_id = wait_for_v5_request_stream(&output, "fs.write");
        assert_eq!(client.shared.request_budget.used(), retained_bytes);
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .contains_key(&stream_id)
        );

        let response = RemoteResponse::WriteFile(WriteResultResponse {
            path: PathBuf::from("large.txt"),
            size: body_len as u64,
            modified_unix_millis: None,
            modified_unix_nanos: None,
        });
        input.push(v5_frames_bytes(v5_response_frames(
            stream_id,
            "fs.write",
            response.clone(),
            Vec::new(),
        )));
        assert_eq!(worker.join().unwrap().unwrap(), (response, Vec::new()));

        let stream_frames = read_v5_frames(output.bytes())
            .into_iter()
            .filter(|frame| frame.stream_id == stream_id)
            .collect::<Vec<_>>();
        assert!(
            stream_frames
                .iter()
                .all(|frame| { frame.frame_type != protocol_v5::FrameType::EndStream })
        );
        assert_eq!(client.shared.request_budget.used(), retained_bytes);
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .contains_key(&stream_id)
        );

        input.push(v5_frames_bytes(vec![
            protocol_v5::window_update_frame(stream_id, retained_bytes as u64).unwrap(),
        ]));
        wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
        wait_for_v5_outbound_request_reservation_release(&client.shared, stream_id);
        assert_eq!(client.shared.request_budget.used(), 0);

        client.close();
        input.close();
    }

    #[test]
    fn dropping_backend_future_after_early_response_purges_flow_blocked_body() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();
        let shared = Arc::clone(&client.shared);
        let backend = RemoteWorkspaceBackendImpl::new(loopback_identity(), client);
        let body_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
            + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
        let body = vec![7; body_len];
        let mut request =
            Box::pin(backend.write_file(Path::new("large.txt"), &body, WriteOptions::default()));
        let waker = futures::task::noop_waker();
        let mut context = std::task::Context::from_waker(&waker);

        assert!(
            std::future::Future::poll(request.as_mut(), &mut context).is_pending(),
            "the early response must remain queued until the application polls again"
        );
        let stream_id = wait_for_v5_request_stream(&output, "fs.write");
        let response = RemoteResponse::WriteFile(WriteResultResponse {
            path: PathBuf::from("large.txt"),
            size: body_len as u64,
            modified_unix_millis: None,
            modified_unix_nanos: None,
        });
        input.push(v5_frames_bytes(v5_response_frames(
            stream_id,
            "fs.write",
            response,
            Vec::new(),
        )));

        let started = Instant::now();
        while Arc::strong_count(&backend.client) != 1
            || shared.waiters.lock().unwrap().contains_key(&stream_id)
        {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for the early response to reach the backend future"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .contains_key(&stream_id)
        );
        assert!(shared.request_budget.used() > 0);
        assert!(
            !read_v5_complete_frames(output.bytes()).iter().any(|frame| {
                frame.stream_id == stream_id
                    && frame.frame_type == protocol_v5::FrameType::EndStream
            })
        );
        let data_frames_before_drop = read_v5_complete_frames(output.bytes())
            .into_iter()
            .filter(|frame| {
                frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::Data
            })
            .count();

        drop(request);

        wait_for_v5_outbound_request_reservation_release(&shared, stream_id);
        assert_eq!(shared.request_budget.used(), 0);
        let stream_frames = read_v5_complete_frames(output.bytes())
            .into_iter()
            .filter(|frame| frame.stream_id == stream_id)
            .collect::<Vec<_>>();
        let resets = stream_frames
            .iter()
            .filter(|frame| frame.frame_type == protocol_v5::FrameType::ResetStream)
            .collect::<Vec<_>>();
        assert!(resets.len() <= 1);
        if let Some(reset) = resets.first() {
            assert_eq!(
                reset
                    .decode_control::<protocol_v5::ResetStream>()
                    .unwrap()
                    .code,
                protocol_v5::RESET_CANCELLED
            );
        }
        assert_eq!(
            stream_frames
                .iter()
                .filter(|frame| frame.frame_type == protocol_v5::FrameType::Data)
                .count(),
            data_frames_before_drop
        );
        assert!(
            stream_frames
                .iter()
                .all(|frame| frame.frame_type != protocol_v5::FrameType::EndStream)
        );
        assert!(!shared.closed.load(Ordering::Acquire));

        let healthy_client = Arc::clone(&backend.client);
        let healthy = std::thread::spawn(move || {
            healthy_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("healthy.rs"),
                },
                Vec::new(),
            )
        });
        let healthy_stream = wait_for_v5_request_stream_after(&output, "fs.stat", stream_id);
        let healthy_response = RemoteResponse::Stat(FileStatResponse {
            path: PathBuf::from("healthy.rs"),
            kind: RemoteFileKind::File,
            size: 7,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        });
        input.push(v5_frames_bytes(v5_response_frames(
            healthy_stream,
            "fs.stat",
            healthy_response.clone(),
            Vec::new(),
        )));
        assert_eq!(
            healthy.join().unwrap().unwrap(),
            (healthy_response, Vec::new())
        );

        drop(backend);
        input.close();
    }

    #[test]
    fn v5_early_raw_response_keeps_request_payload_reserved_until_outbound_end_is_written() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let payload_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
            + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
        let payload = vec![9; payload_len];
        let request_client = Arc::clone(&client);
        let worker = std::thread::spawn(move || {
            request_client.request_v5_raw(
                "watch.resync",
                payload,
                v5_watch_control_request_context(),
            )
        });

        let stream_id = wait_for_v5_request_stream(&output, "watch.resync");
        assert_eq!(client.shared.request_budget.used(), payload_len);
        input.push(v5_frames_bytes(v5_raw_response_frames(
            stream_id,
            "watch.resync",
            vec![1, 2, 3],
        )));
        assert_eq!(worker.join().unwrap().unwrap(), vec![1, 2, 3]);

        assert_eq!(client.shared.request_budget.used(), payload_len);
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .contains_key(&stream_id)
        );
        input.push(v5_frames_bytes(vec![
            protocol_v5::window_update_frame(stream_id, payload_len as u64).unwrap(),
        ]));
        wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
        wait_for_v5_outbound_request_reservation_release(&client.shared, stream_id);
        assert_eq!(client.shared.request_budget.used(), 0);

        client.close();
        input.close();
    }

    #[test]
    fn v5_malformed_early_end_releases_purged_request_reservation() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let request = RemoteRequest::WriteFile {
            path: PathBuf::from("malformed.txt"),
            create_parent_dirs: false,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };
        let body_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
            + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
        let request_client = Arc::clone(&client);
        let worker = std::thread::spawn(move || request_client.request(request, vec![5; body_len]));

        let stream_id = wait_for_v5_request_stream(&output, "fs.write");
        assert!(client.shared.request_budget.used() >= body_len);
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .contains_key(&stream_id)
        );

        input.push(v5_frames_bytes(vec![protocol_v5::Frame::new(
            protocol_v5::FrameType::EndStream,
            stream_id,
        )]));
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("ended without final response"));
        wait_for_v5_outbound_request_reservation_release(&client.shared, stream_id);
        assert_eq!(client.shared.request_budget.used(), 0);
        assert_eq!(client.shared.session.lock().unwrap().queued_len(), 0);

        client.close();
        input.close();
    }

    #[test]
    fn v5_incoming_reset_releases_flow_blocked_request_reservation() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let request = RemoteRequest::WriteFile {
            path: PathBuf::from("cancelled.txt"),
            create_parent_dirs: false,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };
        let body_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
            + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
        let body = vec![3; body_len];
        let request_client = Arc::clone(&client);
        let worker = std::thread::spawn(move || request_client.request(request, body));

        let stream_id = wait_for_v5_request_stream(&output, "fs.write");
        assert!(client.shared.request_budget.used() >= body_len);
        input.push(v5_frames_bytes(vec![protocol_v5::reset_stream_frame(
            stream_id,
            protocol_v5::RESET_CANCELLED,
            "peer cancelled request",
        )]));

        let error = worker.join().unwrap().unwrap_err();
        assert!(matches!(error, RemoteClientError::Remote(_)));
        wait_for_v5_outbound_request_reservation_release(&client.shared, stream_id);
        assert_eq!(client.shared.request_budget.used(), 0);

        client.close();
        input.close();
    }

    #[test]
    fn v5_waiter_registration_failure_rolls_back_request_reservation() {
        for raw in [false, true] {
            let input = BlockingRead::default();
            let output = SharedWrite::default();
            input.push(v5_server_input(Vec::new()));
            let client = RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap();

            let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if raw {
                    let _guard = client.shared.raw_waiters.lock().unwrap();
                    panic!("poison raw v5 waiters");
                } else {
                    let _guard = client.shared.waiters.lock().unwrap();
                    panic!("poison v5 waiters");
                }
            }));
            assert!(poisoned.is_err());

            let failed = if raw {
                client
                    .request_v5_raw(
                        "watch.stop",
                        vec![1, 2, 3],
                        v5_watch_control_request_context(),
                    )
                    .is_err()
            } else {
                client
                    .request(
                        RemoteRequest::Stat {
                            path: PathBuf::from("poisoned.rs"),
                        },
                        Vec::new(),
                    )
                    .is_err()
            };
            assert!(failed);
            assert_eq!(client.shared.request_budget.used(), 0);
            assert!(
                client
                    .shared
                    .outbound_request_reservations
                    .lock()
                    .unwrap()
                    .is_empty()
            );

            client.close();
            input.close();
        }
    }

    #[test]
    fn v5_client_accumulators_reject_aggregate_decoded_data_over_limit() {
        let mut response = V5ResponseAccumulator {
            received_bytes: V5_MAX_ACCUMULATED_RESPONSE_BYTES,
            ..V5ResponseAccumulator::default()
        };
        let response_error = response
            .observe(protocol_v5::StreamEvent::Data {
                stream_id: 1,
                channel: protocol_v5::DataChannel::FileBody,
                uncompressed_len: 1,
                body: vec![0],
            })
            .expect("response limit should finish with an error")
            .unwrap_err();
        assert!(response_error.to_string().contains("decoded byte limit"));

        let mut raw = V5RawResponseAccumulator {
            received_bytes: V5_MAX_RAW_RESPONSE_BYTES,
            ..V5RawResponseAccumulator::default()
        };
        let raw_error = raw
            .observe(protocol_v5::StreamEvent::Data {
                stream_id: 3,
                channel: protocol_v5::DataChannel::Unspecified,
                uncompressed_len: 1,
                body: vec![0],
            })
            .expect("raw response limit should finish with an error")
            .unwrap_err();
        assert!(raw_error.to_string().contains("decoded byte limit"));
    }

    #[test]
    fn v5_over_limit_stream_resets_without_credit_or_harming_another_stream() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let first_client = Arc::clone(&client);
        let first = std::thread::spawn(move || {
            first_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("oversized.rs"),
                },
                Vec::new(),
            )
        });
        let first_stream = wait_for_v5_request_stream(&output, "fs.stat");

        let second_client = Arc::clone(&client);
        let second = std::thread::spawn(move || {
            second_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("healthy.rs"),
                },
                Vec::new(),
            )
        });
        let second_stream = wait_for_v5_request_stream_after(&output, "fs.stat", first_stream);
        client
            .shared
            .waiters
            .lock()
            .unwrap()
            .get_mut(&first_stream)
            .unwrap()
            .accumulator
            .received_bytes = V5_MAX_ACCUMULATED_RESPONSE_BYTES;

        let mut frames = vec![
            protocol_v5::stream_data_frame(
                first_stream,
                vec![0],
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
            )
            .unwrap(),
        ];
        frames.extend(v5_response_frames(
            second_stream,
            "fs.stat",
            RemoteResponse::Stat(FileStatResponse {
                path: PathBuf::from("healthy.rs"),
                kind: RemoteFileKind::File,
                size: 1,
                modified_unix_millis: None,
                modified_unix_nanos: None,
                readonly: false,
            }),
            Vec::new(),
        ));
        input.push(v5_frames_bytes(frames));

        let first_error = first.join().unwrap().unwrap_err();
        let (second_response, _) = second.join().unwrap().unwrap();

        assert!(first_error.to_string().contains("decoded byte limit"));
        assert!(matches!(second_response, RemoteResponse::Stat(_)));
        assert!(!client.shared.closed.load(Ordering::Acquire));
        let outbound = read_v5_frames(output.bytes());
        assert!(outbound.iter().any(|frame| {
            frame.stream_id == first_stream
                && frame.frame_type == protocol_v5::FrameType::ResetStream
        }));
        assert!(!outbound.iter().any(|frame| {
            frame.stream_id == first_stream
                && frame.frame_type == protocol_v5::FrameType::WindowUpdate
        }));
        client.close();
        input.close();
    }

    #[test]
    fn v5_client_inactivity_is_stream_targeted_and_read_timeout_is_local() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let context = RemoteRequestContext::from_policy_at(
            RemoteRequestDeadlinePolicy::bounded(
                Duration::from_secs(60 * 60),
                Duration::from_secs(30),
            ),
            Instant::now(),
            v5_now_unix_millis(),
        );

        let first_client = Arc::clone(&client);
        let (first_sender, first_receiver) = mpsc::channel();
        let first = std::thread::spawn(move || {
            let result = first_client.request_with_context(
                RemoteRequest::Stat {
                    path: PathBuf::from("stalled.rs"),
                },
                Vec::new(),
                context,
            );
            first_sender.send(result).unwrap();
        });
        let first_stream = wait_for_v5_request_stream(&output, "fs.stat");
        wait_for_v5_stream_frame(&output, first_stream, protocol_v5::FrameType::EndStream);

        let second_client = Arc::clone(&client);
        let (second_sender, second_receiver) = mpsc::channel();
        let second = std::thread::spawn(move || {
            let result = second_client.request_with_context(
                RemoteRequest::Stat {
                    path: PathBuf::from("healthy.rs"),
                },
                Vec::new(),
                context,
            );
            second_sender.send(result).unwrap();
        });
        let second_stream = wait_for_v5_request_stream_after(&output, "fs.stat", first_stream);
        wait_for_v5_stream_frame(&output, second_stream, protocol_v5::FrameType::EndStream);

        let recent = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        {
            let mut waiters = client.shared.waiters.lock().unwrap();
            waiters
                .get_mut(&first_stream)
                .unwrap()
                .deadline
                .last_progress_at = recent;
            waiters
                .get_mut(&second_stream)
                .unwrap()
                .deadline
                .last_progress_at = recent;
        }
        let after_sequence = read_v5_complete_frames(output.bytes())
            .into_iter()
            .map(|frame| frame.frame_sequence)
            .max()
            .unwrap_or(0);
        let ping = protocol_v5::PingPayload {
            token: b"deadline-progress-barrier".to_vec(),
        };
        input.push(v5_frames_bytes(vec![
            protocol_v5::window_update_frame(second_stream, 1).unwrap(),
            protocol_v5::Frame::from_control(protocol_v5::FrameType::Ping, 0, &ping),
        ]));
        let _ = wait_for_v5_connection_frame_after(
            &output,
            protocol_v5::FrameType::Pong,
            after_sequence,
        );

        {
            let waiters = client.shared.waiters.lock().unwrap();
            assert_eq!(
                waiters
                    .get(&first_stream)
                    .unwrap()
                    .deadline
                    .last_progress_at,
                recent,
                "another stream and heartbeat traffic must not refresh the stalled request"
            );
            assert!(
                waiters
                    .get(&second_stream)
                    .unwrap()
                    .deadline
                    .last_progress_at
                    > recent,
                "same-stream WINDOW_UPDATE should refresh inactivity"
            );
        }

        let expired_at = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
        {
            let mut waiters = client.shared.waiters.lock().unwrap();
            let pending = waiters.get_mut(&first_stream).unwrap();
            pending.accumulator.method = Some("fs.stat".to_string());
            pending.deadline.last_progress_at = expired_at;
        }
        signal_v5_client_deadlines(&client.shared);

        let error = first_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("deadline watchdog should finish the stalled read")
            .unwrap_err();
        assert!(matches!(
            error,
            RemoteClientError::RequestDeadlineExceeded {
                ref method,
                kind: RemoteRequestDeadlineKind::Inactivity,
            } if method == "fs.stat"
        ));
        wait_for_v5_stream_frame(&output, first_stream, protocol_v5::FrameType::ResetStream);
        let resets = read_v5_complete_frames(output.bytes())
            .into_iter()
            .filter(|frame| {
                frame.stream_id == first_stream
                    && frame.frame_type == protocol_v5::FrameType::ResetStream
            })
            .collect::<Vec<_>>();
        assert_eq!(resets.len(), 1);
        assert_eq!(
            resets[0]
                .decode_control::<protocol_v5::ResetStream>()
                .unwrap()
                .code,
            protocol_v5::RESET_DEADLINE_EXCEEDED
        );
        assert!(!client.shared.closed.load(Ordering::Acquire));
        assert!(
            client
                .shared
                .waiters
                .lock()
                .unwrap()
                .contains_key(&second_stream)
        );

        let response = RemoteResponse::Stat(FileStatResponse {
            path: PathBuf::from("healthy.rs"),
            kind: RemoteFileKind::File,
            size: 7,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        });
        input.push(v5_frames_bytes(v5_response_frames(
            second_stream,
            "fs.stat",
            response.clone(),
            Vec::new(),
        )));
        assert_eq!(
            second_receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("second stream should remain usable")
                .unwrap(),
            (response, Vec::new())
        );

        first.join().unwrap();
        second.join().unwrap();
        client.close();
        input.close();
    }

    #[test]
    fn v5_client_mutation_deadline_is_terminal_and_outcome_unknown() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        let abort_calls = Arc::new(AtomicUsize::new(0));
        let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
            calls: Arc::clone(&abort_calls),
        });
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
                Some(abort),
            )
            .unwrap(),
        );
        let context = RemoteRequestContext::from_policy_at(
            RemoteRequestDeadlinePolicy::bounded(
                Duration::from_secs(60 * 60),
                Duration::from_secs(30),
            ),
            Instant::now(),
            v5_now_unix_millis(),
        );
        let request_client = Arc::clone(&client);
        let (result_sender, result_receiver) = mpsc::channel();
        let request = std::thread::spawn(move || {
            let result = request_client.request_with_context(
                RemoteRequest::CreateDir {
                    path: PathBuf::from("possibly-created"),
                },
                Vec::new(),
                context,
            );
            result_sender.send(result).unwrap();
        });
        let stream_id = wait_for_v5_request_stream(&output, "fs.create_dir");
        wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
        {
            let mut waiters = client.shared.waiters.lock().unwrap();
            let pending = waiters.get_mut(&stream_id).unwrap();
            pending.deadline.last_progress_at =
                Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
        }
        {
            let mut heartbeat = client.shared.heartbeat.lock().unwrap();
            heartbeat.last_peer_activity = Instant::now();
            heartbeat.ping = None;
        }
        signal_v5_client_deadlines(&client.shared);

        let error = result_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("mutation deadline should finish the request")
            .unwrap_err();
        assert!(matches!(
            error,
            RemoteClientError::OutcomeUnknown { ref method, .. }
                if method == "fs.create_dir"
        ));
        assert!(client.shared.closed.load(Ordering::Acquire));
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        assert!(client.shared.waiters.lock().unwrap().is_empty());
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .is_empty()
        );
        assert_eq!(client.shared.request_budget.used(), 0);
        assert_eq!(client.shared.response_budget.used(), 0);

        client.close();
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        request.join().unwrap();
        input.close();
    }

    #[test]
    fn v5_client_final_metadata_wins_race_with_mutation_deadline() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let context = RemoteRequestContext::from_policy_at(
            RemoteRequestDeadlinePolicy::bounded(
                Duration::from_secs(60 * 60),
                Duration::from_secs(30),
            ),
            Instant::now(),
            v5_now_unix_millis(),
        );
        let request_client = Arc::clone(&client);
        let (result_sender, result_receiver) = mpsc::channel();
        let request = std::thread::spawn(move || {
            let result = request_client.request_with_context(
                RemoteRequest::CreateDir {
                    path: PathBuf::from("created-before-deadline"),
                },
                Vec::new(),
                context,
            );
            result_sender.send(result).unwrap();
        });
        let stream_id = wait_for_v5_request_stream(&output, "fs.create_dir");
        wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
        {
            let mut waiters = client.shared.waiters.lock().unwrap();
            waiters
                .get_mut(&stream_id)
                .unwrap()
                .deadline
                .last_progress_at = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
        }

        let mut heartbeat = client.shared.heartbeat.lock().unwrap();
        heartbeat.last_peer_activity = Instant::now();
        heartbeat.ping = None;
        let expiry_client = Arc::clone(&client);
        let expiry = std::thread::spawn(move || {
            expire_v5_client_deadlines_at(&expiry_client.shared, Instant::now()).unwrap()
        });
        assert!(handle_v5_client_stream_event(
            &client.shared,
            protocol_v5::StreamEvent::Headers {
                stream_id,
                role: protocol_v5::MessageRole::FinalResponse,
                priority: protocol_v5::Priority::UserInput,
                envelope: protocol_v5::StreamEnvelope::response(
                    stream_id,
                    "fs.create_dir",
                    protocol_v5::MessageRole::FinalResponse,
                    true,
                ),
            },
        ));
        drop(heartbeat);
        expiry.join().unwrap();

        let error = result_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("deadline should finish the mutation after final metadata")
            .unwrap_err();
        assert!(matches!(
            error,
            RemoteClientError::RequestDeadlineExceeded {
                ref method,
                kind: RemoteRequestDeadlineKind::Inactivity,
            } if method == "fs.create_dir"
        ));
        assert!(!client.shared.closed.load(Ordering::Acquire));
        wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::ResetStream);

        client.close();
        request.join().unwrap();
        input.close();
    }

    #[test]
    fn v5_client_unknown_peer_deadline_fails_every_waiter() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        let abort_calls = Arc::new(AtomicUsize::new(0));
        let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
            calls: Arc::clone(&abort_calls),
        });
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
                Some(abort),
            )
            .unwrap(),
        );
        let context = RemoteRequestContext::from_policy_at(
            RemoteRequestDeadlinePolicy::bounded(
                Duration::from_secs(60 * 60),
                Duration::from_secs(30),
            ),
            Instant::now(),
            v5_now_unix_millis(),
        );
        let first_client = Arc::clone(&client);
        let (first_sender, first_receiver) = mpsc::channel();
        let first = std::thread::spawn(move || {
            let result = first_client.request_with_context(
                RemoteRequest::Stat {
                    path: PathBuf::from("stalled.rs"),
                },
                Vec::new(),
                context,
            );
            first_sender.send(result).unwrap();
        });
        let first_stream = wait_for_v5_request_stream(&output, "fs.stat");
        wait_for_v5_stream_frame(&output, first_stream, protocol_v5::FrameType::EndStream);

        let second_client = Arc::clone(&client);
        let (second_sender, second_receiver) = mpsc::channel();
        let second = std::thread::spawn(move || {
            let result = second_client.request_with_context(
                RemoteRequest::Stat {
                    path: PathBuf::from("also-failed.rs"),
                },
                Vec::new(),
                context,
            );
            second_sender.send(result).unwrap();
        });
        let second_stream = wait_for_v5_request_stream_after(&output, "fs.stat", first_stream);
        wait_for_v5_stream_frame(&output, second_stream, protocol_v5::FrameType::EndStream);

        client
            .shared
            .waiters
            .lock()
            .unwrap()
            .get_mut(&first_stream)
            .unwrap()
            .deadline
            .last_progress_at = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
        {
            let mut heartbeat = client.shared.heartbeat.lock().unwrap();
            heartbeat.last_peer_activity = Instant::now()
                .checked_sub(heartbeat.idle_ping_interval + Duration::from_millis(1))
                .unwrap();
            heartbeat.ping = None;
        }
        signal_v5_client_deadlines(&client.shared);

        for receiver in [&first_receiver, &second_receiver] {
            let error = receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("unknown peer expiry should fail every waiter")
                .unwrap_err();
            let RemoteClientError::Io(error) = error else {
                panic!("expected terminal peer-health timeout, got {error:?}");
            };
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        }
        assert!(client.shared.closed.load(Ordering::Acquire));
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        assert!(client.shared.waiters.lock().unwrap().is_empty());
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .is_empty()
        );
        assert_eq!(client.shared.request_budget.used(), 0);
        assert_eq!(client.shared.response_budget.used(), 0);

        first.join().unwrap();
        second.join().unwrap();
        input.close();
    }

    #[test]
    fn v5_client_watch_control_deadline_is_connection_terminal() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        let abort_calls = Arc::new(AtomicUsize::new(0));
        let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
            calls: Arc::clone(&abort_calls),
        });
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
                Some(abort),
            )
            .unwrap(),
        );
        let context = RemoteRequestContext::from_policy_at(
            RemoteRequestDeadlinePolicy::bounded(
                Duration::from_secs(60 * 60),
                Duration::from_secs(30),
            ),
            Instant::now(),
            v5_now_unix_millis(),
        );
        let request_client = Arc::clone(&client);
        let (result_sender, result_receiver) = mpsc::channel();
        let request = std::thread::spawn(move || {
            let result = request_client.request_v5_raw(
                "watch.resync",
                protocol_v5::WatchResync {
                    watch_id: 7,
                    roots: vec![".".to_string()],
                }
                .encode_to_vec(),
                context,
            );
            result_sender.send(result).unwrap();
        });
        let stream_id = wait_for_v5_request_stream(&output, "watch.resync");
        wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
        client
            .shared
            .raw_waiters
            .lock()
            .unwrap()
            .get_mut(&stream_id)
            .unwrap()
            .deadline
            .last_progress_at = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
        signal_v5_client_deadlines(&client.shared);

        let error = result_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("watch control deadline should finish the request")
            .unwrap_err();
        let RemoteClientError::Io(error) = error else {
            panic!("expected terminal watch timeout, got {error:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(client.shared.closed.load(Ordering::Acquire));
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        assert!(client.shared.raw_waiters.lock().unwrap().is_empty());
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .is_empty()
        );
        assert_eq!(client.shared.request_budget.used(), 0);
        assert_eq!(client.shared.response_budget.used(), 0);

        client.close();
        assert_eq!(abort_calls.load(Ordering::Acquire), 1);
        request.join().unwrap();
        input.close();
    }

    #[test]
    fn v5_response_accumulator_merges_search_partials_with_final_tail() {
        let mut accumulator = V5ResponseAccumulator::default();
        let partial_payload = RemoteResponse::FileSearch(FileSearchResponse {
            root: PathBuf::new(),
            files: vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")],
            truncated: false,
        })
        .to_v5_payload()
        .unwrap();
        let split_at = partial_payload.len() / 2;

        assert!(
            accumulator
                .observe(protocol_v5::StreamEvent::Headers {
                    stream_id: 1,
                    role: protocol_v5::MessageRole::PartialResult,
                    priority: protocol_v5::Priority::Background,
                    envelope: protocol_v5::StreamEnvelope::response(
                        1,
                        "search.files",
                        protocol_v5::MessageRole::PartialResult,
                        false,
                    ),
                })
                .is_none()
        );
        assert!(
            accumulator
                .observe(protocol_v5::StreamEvent::Data {
                    stream_id: 1,
                    channel: protocol_v5::DataChannel::SearchPayload,
                    uncompressed_len: split_at as u64,
                    body: partial_payload[..split_at].to_vec(),
                })
                .is_none()
        );
        assert!(
            accumulator
                .observe(protocol_v5::StreamEvent::Data {
                    stream_id: 1,
                    channel: protocol_v5::DataChannel::SearchPayload,
                    uncompressed_len: (partial_payload.len() - split_at) as u64,
                    body: partial_payload[split_at..].to_vec(),
                })
                .is_none()
        );

        let final_payload = RemoteResponse::FileSearch(FileSearchResponse {
            root: PathBuf::new(),
            files: vec![PathBuf::from("c.rs")],
            truncated: true,
        })
        .to_v5_payload()
        .unwrap();
        assert!(
            accumulator
                .observe(protocol_v5::StreamEvent::Headers {
                    stream_id: 1,
                    role: protocol_v5::MessageRole::FinalResponse,
                    priority: protocol_v5::Priority::Background,
                    envelope: protocol_v5::StreamEnvelope::response(
                        1,
                        "search.files",
                        protocol_v5::MessageRole::FinalResponse,
                        true,
                    ),
                })
                .is_none()
        );
        assert!(
            accumulator
                .observe(protocol_v5::StreamEvent::Data {
                    stream_id: 1,
                    channel: protocol_v5::DataChannel::Unspecified,
                    uncompressed_len: final_payload.len() as u64,
                    body: final_payload,
                })
                .is_none()
        );

        let result = accumulator
            .observe(protocol_v5::StreamEvent::EndStream { stream_id: 1 })
            .expect("search response should complete")
            .unwrap();
        let (RemoteResponse::FileSearch(response), body) = result else {
            panic!("expected file search response");
        };
        assert!(body.is_empty());
        assert_eq!(
            response.files,
            vec![
                PathBuf::from("a.rs"),
                PathBuf::from("b.rs"),
                PathBuf::from("c.rs")
            ]
        );
        assert!(response.truncated);
    }

    #[test]
    fn v5_method_payload_reports_unsupported_and_invalid_payloads() {
        let error = RemoteRequest::from_v5_method_payload("watch.start", b"{}").unwrap_err();
        assert_eq!(
            error,
            V5MethodError::UnsupportedMethod("watch.start".to_string())
        );

        let error = RemoteRequest::from_v5_method_payload("fs.stat", b"{").unwrap_err();
        assert!(matches!(
            error,
            V5MethodError::InvalidPayload { ref method, .. } if method == "fs.stat"
        ));

        let error = RemoteRequest::from_v5_method_payload("session.shutdown", br#"{"extra":true}"#)
            .unwrap_err();
        assert!(matches!(
            error,
            V5MethodError::InvalidPayload { ref method, .. } if method == "session.shutdown"
        ));
    }

    #[test]
    fn v5_response_method_matches_request_namespace() {
        assert_eq!(RemoteResponse::Shutdown.v5_method(), "session.shutdown");
        assert_eq!(
            RemoteResponse::ReadFile(FileReadResponse {
                path: PathBuf::from("README.md"),
                size: 0,
                modified_unix_millis: None,
                modified_unix_nanos: None,
                readonly: false,
                truncated: false,
            })
            .v5_method(),
            "fs.read"
        );
    }

    #[test]
    fn v5_client_writes_method_payload_body_and_decodes_write_response() {
        let request = RemoteRequest::WriteFile {
            path: PathBuf::from("src/lib.rs"),
            create_parent_dirs: true,
            expected_modified_unix_millis: Some(10),
            expected_modified_unix_nanos: Some(20),
        };
        let response = RemoteResponse::WriteFile(WriteResultResponse {
            path: PathBuf::from("src/lib.rs"),
            size: 7,
            modified_unix_millis: Some(11),
            modified_unix_nanos: Some(21),
        });
        let input = v5_server_input(v5_response_frames(
            1,
            "fs.write",
            response.clone(),
            Vec::new(),
        ));
        let mut client = RemoteWorkspaceV5Client::connect(
            protocol_v5::FramedIo::new(Cursor::new(input), Vec::new()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();

        let (actual_response, actual_body) = client
            .request(request.clone(), b"new body".to_vec())
            .unwrap();
        assert_eq!(
            client.session.stream_tombstone(1),
            Some(protocol_v5::StreamTombstone::Closed)
        );
        let (_, output) = client.into_inner();
        let frames = read_v5_frames(output);

        assert_eq!(actual_response, response);
        assert!(actual_body.is_empty());
        assert_eq!(frames[0].frame_type, protocol_v5::FrameType::Hello);
        assert_eq!(frames[1].frame_type, protocol_v5::FrameType::SettingsAck);
        let request_headers = frames
            .iter()
            .find(|frame| frame.frame_type == protocol_v5::FrameType::Headers)
            .unwrap();
        let envelope = request_headers
            .decode_control::<protocol_v5::StreamEnvelope>()
            .unwrap();
        assert_eq!(envelope.method, "fs.write");
        assert_ne!(envelope.deadline_unix_ms, 0);
        assert_eq!(
            envelope.request_idempotency().unwrap(),
            protocol_v5::Idempotency::Mutation
        );

        let data_frames = frames
            .iter()
            .filter(|frame| frame.frame_type == protocol_v5::FrameType::Data)
            .collect::<Vec<_>>();
        assert_eq!(data_frames.len(), 2);
        let metadata = data_frames[0]
            .decode_control::<protocol_v5::DataEnvelope>()
            .unwrap();
        assert_eq!(
            protocol_v5::DataChannel::try_from(metadata.channel).unwrap(),
            protocol_v5::DataChannel::Unspecified
        );
        assert_eq!(
            RemoteRequest::from_v5_method_payload("fs.write", &data_frames[0].body).unwrap(),
            request
        );
        let body = data_frames[1]
            .decode_control::<protocol_v5::DataEnvelope>()
            .unwrap();
        assert_eq!(
            protocol_v5::DataChannel::try_from(body.channel).unwrap(),
            protocol_v5::DataChannel::FileBody
        );
        assert_eq!(data_frames[1].body, b"new body");
    }

    #[test]
    fn v5_client_decodes_file_body_response() {
        let request = RemoteRequest::ReadFile {
            path: PathBuf::from("README.md"),
            max_bytes: None,
        };
        let response = RemoteResponse::ReadFile(FileReadResponse {
            path: PathBuf::from("README.md"),
            size: 11,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
            truncated: false,
        });
        let input = v5_server_input(v5_response_frames(
            1,
            "fs.read",
            response.clone(),
            b"hello world".to_vec(),
        ));
        let mut client = RemoteWorkspaceV5Client::connect(
            protocol_v5::FramedIo::new(Cursor::new(input), Vec::new()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();

        let (actual_response, actual_body) = client.request(request, Vec::new()).unwrap();

        assert_eq!(actual_response, response);
        assert_eq!(actual_body, b"hello world");
    }

    #[test]
    fn v5_client_returns_remote_error_after_final_error_headers() {
        let error = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            1,
            &protocol_v5::StreamEnvelope::error(
                1,
                "fs.stat",
                protocol_v5::ErrorHeader {
                    code: "NOT_FOUND".to_string(),
                    message: "missing".to_string(),
                    retryable: false,
                    details: "stat failed".to_string(),
                    remote_errno: 2,
                },
            ),
        );
        let input = v5_server_input(vec![
            error,
            protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, 1),
        ]);
        let mut client = RemoteWorkspaceV5Client::connect(
            protocol_v5::FramedIo::new(Cursor::new(input), Vec::new()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();

        let error = client
            .request(
                RemoteRequest::Stat {
                    path: PathBuf::from("missing.txt"),
                },
                Vec::new(),
            )
            .unwrap_err();

        let RemoteClientError::Remote(error) = error else {
            panic!("expected remote error");
        };
        assert_eq!(error.code, "NOT_FOUND");
        assert_eq!(error.message, "missing");
        assert_eq!(error.diagnostic.as_deref(), Some("stat failed"));
    }

    #[test]
    fn v5_sync_client_keeps_connection_after_mutation_deadline_with_final_metadata() {
        let final_metadata = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            1,
            &protocol_v5::StreamEnvelope::response(
                1,
                "fs.create_dir",
                protocol_v5::MessageRole::FinalResponse,
                true,
            ),
        );
        let reset = protocol_v5::reset_stream_frame(
            1,
            protocol_v5::RESET_DEADLINE_EXCEEDED,
            "response delivery deadline expired",
        );
        let healthy_response = RemoteResponse::Stat(FileStatResponse {
            path: PathBuf::from("healthy.rs"),
            kind: RemoteFileKind::File,
            size: 1,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        });
        let mut response_frames = vec![final_metadata, reset];
        response_frames.extend(v5_response_frames(
            3,
            "fs.stat",
            healthy_response.clone(),
            Vec::new(),
        ));
        let input = v5_server_input(response_frames);
        let mut client = RemoteWorkspaceV5Client::connect(
            protocol_v5::FramedIo::new(Cursor::new(input), Vec::new()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();

        let error = client
            .request(
                RemoteRequest::CreateDir {
                    path: PathBuf::from("possibly-created"),
                },
                Vec::new(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RemoteClientError::RequestDeadlineExceeded {
                ref method,
                kind: RemoteRequestDeadlineKind::Absolute,
            } if method == "fs.create_dir"
        ));

        let response = client
            .request(
                RemoteRequest::Stat {
                    path: PathBuf::from("healthy.rs"),
                },
                Vec::new(),
            )
            .unwrap();
        assert_eq!(response, (healthy_response, Vec::new()));
    }

    #[test]
    fn v5_backend_read_file_uses_shared_workspace_backend_impl() {
        let response = RemoteResponse::ReadFile(FileReadResponse {
            path: PathBuf::from("README.md"),
            size: 11,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
            truncated: false,
        });
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();
        let (backend, hello) = RemoteWorkspaceV5Backend::connect(loopback_identity(), client)
            .expect("v5 backend connect");
        let backend = Arc::new(backend);
        let worker_backend = Arc::clone(&backend);
        let worker = std::thread::spawn(move || {
            block_on(worker_backend.read_file(Path::new("README.md"), ReadOptions::default()))
        });

        let stream_id = wait_for_v5_request_stream(&output, "fs.read");
        input.push(v5_frames_bytes(v5_response_frames(
            stream_id,
            "fs.read",
            response,
            b"hello world".to_vec(),
        )));

        let read = worker.join().unwrap().expect("v5 read file");

        assert_eq!(hello.workspace_root, PathBuf::from("/workspace"));
        assert_eq!(read.path, PathBuf::from("README.md"));
        assert_eq!(read.bytes, b"hello world");
        input.close();
    }

    #[test]
    fn v5_backend_start_watch_exposes_workspace_watch_batches() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();
        let (backend, _) = RemoteWorkspaceV5Backend::connect(loopback_identity(), client).unwrap();
        let backend = Arc::new(backend);
        let worker_backend = Arc::clone(&backend);
        let worker = std::thread::spawn(move || {
            block_on(
                worker_backend.start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from(
                    "/workspace",
                )])),
            )
        });

        let request_stream = wait_for_v5_request_stream(&output, "watch.start");
        let response = protocol_v5::WatchStartResponse {
            watch_id: 9,
            event_stream_id: 2,
            backend: "poll".to_string(),
            recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
            degraded: true,
            requires_reconciliation: true,
            accepted_roots: vec![".".to_string()],
            degraded_roots: Vec::new(),
            unsupported_roots: Vec::new(),
        };
        let batch = protocol_v5::WatchBatch {
            watch_id: 9,
            sequence: 1,
            directory_generations: vec![protocol_v5::WatchDirectoryGeneration {
                path: ".".to_string(),
                generation: 1,
            }],
            events: vec![protocol_v5::WatchChange::modified("src", true)],
            overflow: false,
            resync_required: false,
        };
        let mut frames = vec![v5_watch_event_open_frame(2, 9)];
        frames.extend(v5_raw_response_frames(
            request_stream,
            "watch.start",
            response.encode_to_vec(),
        ));
        frames.push(protocol_v5::watch_batch_frame(2, batch).unwrap());
        input.push(v5_frames_bytes(frames));

        let watch = worker
            .join()
            .unwrap()
            .unwrap()
            .expect("v5 watch should be supported");
        let received = watch.recv_timeout(Duration::from_secs(2)).unwrap();

        assert_eq!(watch.watch_id, 9);
        assert_eq!(
            received.directory_generations[0].path,
            PathBuf::from("/workspace")
        );
        assert_eq!(received.events[0].path, PathBuf::from("/workspace/src"));
        input.close();
    }

    #[test]
    fn v5_backend_start_watch_returns_none_without_watch_capability() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        let mut info = protocol_v5::ServerHandshakeInfo::current("/workspace");
        info.capabilities.retain(|capability| capability != "watch");
        input.push(v5_server_input_with_info(Vec::new(), info));
        let client = RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();
        let (backend, _) = RemoteWorkspaceV5Backend::connect(loopback_identity(), client).unwrap();

        let watch =
            block_on(
                backend.start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from(
                    "/workspace",
                )])),
            )
            .unwrap();

        assert!(watch.is_none());
        assert!(find_v5_request_stream(&output, "watch.start").is_none());
        input.close();
    }

    #[test]
    fn v5_multiplexed_client_receives_server_watch_batches() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let watch_client = Arc::clone(&client);
        let watch_thread = std::thread::spawn(move || {
            watch_client.start_v5_watch(protocol_v5::WatchStart::expanded_dirs(["."]))
        });
        let request_stream = wait_for_v5_request_stream(&output, "watch.start");
        let response = protocol_v5::WatchStartResponse {
            watch_id: 7,
            event_stream_id: 2,
            backend: "poll".to_string(),
            recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
            degraded: true,
            requires_reconciliation: true,
            accepted_roots: vec![".".to_string()],
            degraded_roots: Vec::new(),
            unsupported_roots: Vec::new(),
        };
        let batch = protocol_v5::WatchBatch {
            watch_id: 7,
            sequence: 1,
            directory_generations: vec![protocol_v5::WatchDirectoryGeneration {
                path: ".".to_string(),
                generation: 1,
            }],
            events: vec![protocol_v5::WatchChange::modified(".", true)],
            overflow: false,
            resync_required: false,
        };
        let mut frames = vec![v5_watch_event_open_frame(2, 7)];
        frames.extend(v5_raw_response_frames(
            request_stream,
            "watch.start",
            response.encode_to_vec(),
        ));
        // Exercise the backlog path: the first batch may arrive before start_watch
        // has registered its receiver after decoding the watch.start response.
        frames.push(protocol_v5::watch_batch_frame(2, batch.clone()).unwrap());
        input.push(v5_frames_bytes(frames));

        let watch = watch_thread
            .join()
            .unwrap()
            .expect("watch.start should succeed");
        let received = watch
            .recv_timeout(Duration::from_secs(2))
            .expect("watch batch should be delivered");

        assert_eq!(watch.watch_id, 7);
        assert_eq!(watch.event_stream_id, 2);
        assert_eq!(received.watch_id, batch.watch_id);
        assert_eq!(received.sequence, batch.sequence);
        assert_eq!(received.directory_generations[0].path, ".");
        assert_eq!(received.events[0].path, ".");
        input.close();
    }

    #[test]
    fn v5_multiplexed_client_collapses_slow_watch_consumer_to_resync() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let watch_client = Arc::clone(&client);
        let watch_thread = std::thread::spawn(move || {
            watch_client.start_v5_watch(protocol_v5::WatchStart::expanded_dirs(["."]))
        });
        let request_stream = wait_for_v5_request_stream(&output, "watch.start");
        let response = protocol_v5::WatchStartResponse {
            watch_id: 7,
            event_stream_id: 2,
            backend: "poll".to_string(),
            recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
            degraded: true,
            requires_reconciliation: true,
            accepted_roots: vec![".".to_string()],
            degraded_roots: Vec::new(),
            unsupported_roots: Vec::new(),
        };
        let mut frames = vec![v5_watch_event_open_frame(2, 7)];
        frames.extend(v5_raw_response_frames(
            request_stream,
            "watch.start",
            response.encode_to_vec(),
        ));
        input.push(v5_frames_bytes(frames));
        let watch = watch_thread
            .join()
            .unwrap()
            .expect("watch.start should succeed");

        let batches = (1..=V5_WATCH_DELIVERY_CAPACITY + 1)
            .map(|sequence| protocol_v5::WatchBatch {
                watch_id: 7,
                sequence: sequence as u64,
                directory_generations: Vec::new(),
                events: vec![protocol_v5::WatchChange::modified(
                    format!("file-{sequence}"),
                    false,
                )],
                overflow: false,
                resync_required: false,
            })
            .map(|batch| protocol_v5::watch_batch_frame(2, batch).unwrap())
            .collect::<Vec<_>>();
        input.push(v5_frames_bytes(batches));

        let started = Instant::now();
        while !watch.overflowed.load(Ordering::Acquire) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for local watch overflow"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let overflow = watch
            .recv_timeout(Duration::from_secs(2))
            .expect("local overflow should produce a resync batch");

        assert_eq!(overflow.watch_id, 7);
        assert_eq!(overflow.sequence, (V5_WATCH_DELIVERY_CAPACITY + 1) as u64);
        assert!(overflow.overflow);
        assert!(overflow.resync_required);
        assert!(overflow.events.is_empty());
        input.close();
    }

    #[test]
    fn v5_multiplexed_client_updates_and_stops_watch() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let watch_client = Arc::clone(&client);
        let watch_thread = std::thread::spawn(move || {
            watch_client.start_v5_watch(protocol_v5::WatchStart::expanded_dirs(["."]))
        });
        let start_stream = wait_for_v5_request_stream(&output, "watch.start");
        let start_response = protocol_v5::WatchStartResponse {
            watch_id: 11,
            event_stream_id: 2,
            backend: "poll".to_string(),
            recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
            degraded: true,
            requires_reconciliation: true,
            accepted_roots: vec![".".to_string()],
            degraded_roots: Vec::new(),
            unsupported_roots: Vec::new(),
        };
        let mut frames = vec![v5_watch_event_open_frame(2, 11)];
        frames.extend(v5_raw_response_frames(
            start_stream,
            "watch.start",
            start_response.encode_to_vec(),
        ));
        input.push(v5_frames_bytes(frames));
        let watch = watch_thread.join().unwrap().unwrap();

        let update_client = Arc::clone(&client);
        let update_thread = std::thread::spawn(move || {
            update_client.update_v5_watch(protocol_v5::WatchUpdate {
                watch_id: 11,
                add_roots: vec!["src".to_string()],
                remove_roots: vec![".".to_string()],
            })
        });
        let update_stream = wait_for_v5_request_stream(&output, "watch.update");
        let update_response = protocol_v5::WatchUpdateResponse {
            watch_id: 11,
            accepted_roots: vec!["src".to_string()],
            degraded_roots: Vec::new(),
            unsupported_roots: Vec::new(),
        };
        input.push(v5_frames_bytes(v5_raw_response_frames(
            update_stream,
            "watch.update",
            update_response.encode_to_vec(),
        )));
        let update_response = update_thread.join().unwrap().unwrap();
        assert_eq!(update_response.accepted_roots, ["src"]);

        let resync_client = Arc::clone(&client);
        let resync_thread = std::thread::spawn(move || {
            resync_client.resync_v5_watch(protocol_v5::WatchResync {
                watch_id: 11,
                roots: vec!["src".to_string()],
            })
        });
        let resync_stream = wait_for_v5_request_stream(&output, "watch.resync");
        let resync_response = protocol_v5::WatchResyncResponse {
            watch_id: 11,
            accepted_roots: vec!["src".to_string()],
            unsupported_roots: Vec::new(),
        };
        input.push(v5_frames_bytes(v5_raw_response_frames(
            resync_stream,
            "watch.resync",
            resync_response.encode_to_vec(),
        )));
        let resync_response = resync_thread.join().unwrap().unwrap();
        assert_eq!(resync_response.accepted_roots, ["src"]);

        let stop_client = Arc::clone(&client);
        let stop_thread = std::thread::spawn(move || stop_client.stop_v5_watch(11));
        let stop_stream = wait_for_v5_request_stream(&output, "watch.stop");
        input.push(v5_frames_bytes(v5_raw_response_frames(
            stop_stream,
            "watch.stop",
            Vec::new(),
        )));
        stop_thread.join().unwrap().unwrap();

        assert!(matches!(
            watch.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        input.close();
    }

    #[test]
    fn v5_multiplexed_client_uses_known_generation_and_cached_listing() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let full_listing = DirectoryListingResponse {
            path: PathBuf::from("src"),
            generation: Some(10),
            fingerprint: Some(20),
            complete: true,
            not_modified: false,
            delta: None,
            entries: vec![DirectoryEntryResponse {
                name: "lib.rs".to_string(),
                path: PathBuf::from("src/lib.rs"),
                stat: FileStatResponse {
                    path: PathBuf::from("src/lib.rs"),
                    kind: RemoteFileKind::File,
                    size: 12,
                    modified_unix_millis: None,
                    modified_unix_nanos: None,
                    readonly: false,
                },
                symlink_target: None,
                target_exists: None,
                ignored: Some(false),
            }],
        };

        let first_client = Arc::clone(&client);
        let first_thread = std::thread::spawn(move || {
            first_client.request(
                RemoteRequest::ListDir {
                    path: PathBuf::from("src"),
                },
                Vec::new(),
            )
        });
        let first_stream = wait_for_v5_request_stream(&output, "fs.list_dir");
        let first_payload: V5DirectoryListPayload =
            decode_v5_request_payload(&output, first_stream).unwrap();
        assert_eq!(first_payload.path, PathBuf::from("src"));
        assert_eq!(first_payload.known_generation, None);
        input.push(v5_frames_bytes(v5_response_frames(
            first_stream,
            "fs.list_dir",
            RemoteResponse::ListDir(full_listing.clone()),
            Vec::new(),
        )));
        let (first_response, _) = first_thread.join().unwrap().unwrap();
        let RemoteResponse::ListDir(first_listing) = first_response else {
            panic!("expected first list_dir response");
        };
        assert_eq!(first_listing.entries.len(), 1);

        let second_client = Arc::clone(&client);
        let second_thread = std::thread::spawn(move || {
            second_client.request(
                RemoteRequest::ListDir {
                    path: PathBuf::from("src"),
                },
                Vec::new(),
            )
        });
        let second_stream = wait_for_v5_request_stream_after(&output, "fs.list_dir", first_stream);
        let second_payload: V5DirectoryListPayload =
            decode_v5_request_payload(&output, second_stream).unwrap();
        assert_eq!(second_payload.known_generation, Some(10));
        assert_eq!(second_payload.known_fingerprint, Some(20));
        input.push(v5_frames_bytes(v5_response_frames(
            second_stream,
            "fs.list_dir",
            RemoteResponse::ListDir(DirectoryListingResponse {
                path: PathBuf::from("src"),
                generation: Some(10),
                fingerprint: Some(20),
                complete: true,
                not_modified: true,
                delta: None,
                entries: Vec::new(),
            }),
            Vec::new(),
        )));
        let (second_response, _) = second_thread.join().unwrap().unwrap();
        let RemoteResponse::ListDir(second_listing) = second_response else {
            panic!("expected cached list_dir response");
        };
        assert_eq!(second_listing.entries, full_listing.entries);
        assert!(!second_listing.not_modified);
        input.close();
    }

    fn v5_test_directory_entry(path: &str, size: u64) -> DirectoryEntryResponse {
        let path = PathBuf::from(path);
        let name = path
            .file_name()
            .unwrap_or_else(|| OsStr::new(""))
            .to_string_lossy()
            .into_owned();
        DirectoryEntryResponse {
            name,
            path: path.clone(),
            stat: FileStatResponse {
                path,
                kind: RemoteFileKind::File,
                size,
                modified_unix_millis: None,
                modified_unix_nanos: None,
                readonly: false,
            },
            symlink_target: None,
            target_exists: None,
            ignored: Some(false),
        }
    }

    fn v5_test_directory_listing(
        path: &str,
        generation: u64,
        fingerprint: u64,
        entries: Vec<DirectoryEntryResponse>,
    ) -> DirectoryListingResponse {
        DirectoryListingResponse {
            path: PathBuf::from(path),
            generation: Some(generation),
            fingerprint: Some(fingerprint),
            complete: true,
            not_modified: false,
            delta: None,
            entries,
        }
    }

    #[test]
    fn v5_multiplexed_client_clears_directory_cache_after_watch_resync() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let first_client = Arc::clone(&client);
        let first_thread = std::thread::spawn(move || {
            first_client.request(
                RemoteRequest::ListDir {
                    path: PathBuf::from("src"),
                },
                Vec::new(),
            )
        });
        let first_stream = wait_for_v5_request_stream(&output, "fs.list_dir");
        input.push(v5_frames_bytes(v5_response_frames(
            first_stream,
            "fs.list_dir",
            RemoteResponse::ListDir(v5_test_directory_listing(
                "src",
                10,
                20,
                vec![v5_test_directory_entry("src/lib.rs", 12)],
            )),
            Vec::new(),
        )));
        first_thread.join().unwrap().unwrap();

        let watch_client = Arc::clone(&client);
        let watch_thread = std::thread::spawn(move || {
            watch_client.start_v5_watch(protocol_v5::WatchStart::expanded_dirs(["src"]))
        });
        let watch_stream = wait_for_v5_request_stream_after(&output, "watch.start", first_stream);
        let watch_response = protocol_v5::WatchStartResponse {
            watch_id: 7,
            event_stream_id: 2,
            backend: "poll".to_string(),
            recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
            degraded: true,
            requires_reconciliation: true,
            accepted_roots: vec!["src".to_string()],
            degraded_roots: vec!["src".to_string()],
            unsupported_roots: Vec::new(),
        };
        let mut watch_frames = vec![v5_watch_event_open_frame(2, 7)];
        watch_frames.extend(v5_raw_response_frames(
            watch_stream,
            "watch.start",
            watch_response.encode_to_vec(),
        ));
        input.push(v5_frames_bytes(watch_frames));
        let watch = watch_thread.join().unwrap().unwrap();

        let resync_batch = protocol_v5::WatchBatch {
            watch_id: 7,
            sequence: 1,
            directory_generations: vec![protocol_v5::WatchDirectoryGeneration {
                path: "src".to_string(),
                generation: 11,
            }],
            events: Vec::new(),
            overflow: true,
            resync_required: true,
        };
        input.push(v5_frames_bytes(vec![
            protocol_v5::watch_batch_frame(2, resync_batch).unwrap(),
        ]));
        watch.recv_timeout(Duration::from_secs(2)).unwrap();

        let second_client = Arc::clone(&client);
        let second_thread = std::thread::spawn(move || {
            second_client.request(
                RemoteRequest::ListDir {
                    path: PathBuf::from("src"),
                },
                Vec::new(),
            )
        });
        let second_stream = wait_for_v5_request_stream_after(&output, "fs.list_dir", watch_stream);
        let second_payload: V5DirectoryListPayload =
            decode_v5_request_payload(&output, second_stream).unwrap();
        assert_eq!(second_payload.known_generation, None);
        assert_eq!(second_payload.known_fingerprint, None);
        input.push(v5_frames_bytes(v5_response_frames(
            second_stream,
            "fs.list_dir",
            RemoteResponse::ListDir(v5_test_directory_listing(
                "src",
                11,
                21,
                vec![v5_test_directory_entry("src/lib.rs", 12)],
            )),
            Vec::new(),
        )));
        second_thread.join().unwrap().unwrap();
        input.close();
    }

    #[test]
    fn v5_multiplexed_client_enables_zstd_for_directory_requests_when_negotiated() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input_with_compression(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let request_client = Arc::clone(&client);
        let request_thread = std::thread::spawn(move || {
            request_client.request(
                RemoteRequest::ListDir {
                    path: PathBuf::from("src"),
                },
                Vec::new(),
            )
        });
        let stream_id = wait_for_v5_request_stream(&output, "fs.list_dir");
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        let mut content_encoding = protocol_v5::ContentEncoding::None;
        while let Some(frame) = protocol_v5::read_frame(&mut cursor).unwrap() {
            if frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::Headers {
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                content_encoding = envelope.decode_content_encoding().unwrap();
                break;
            }
        }
        assert_eq!(content_encoding, protocol_v5::ContentEncoding::Zstd);
        let payload: V5DirectoryListPayload =
            decode_v5_request_payload(&output, stream_id).unwrap();
        assert_eq!(payload.path, PathBuf::from("src"));

        input.push(v5_frames_bytes(v5_response_frames_with_content_encoding(
            stream_id,
            "fs.list_dir",
            RemoteResponse::ListDir(v5_test_directory_listing(
                "src",
                1,
                2,
                vec![v5_test_directory_entry("src/lib.rs", 12)],
            )),
            Vec::new(),
            protocol_v5::ContentEncoding::Zstd,
        )));
        let (response, _) = request_thread.join().unwrap().unwrap();
        let RemoteResponse::ListDir(listing) = response else {
            panic!("expected compressed list_dir response");
        };
        assert_eq!(listing.entries.len(), 1);
        input.close();
    }

    #[test]
    fn v5_multiplexed_client_writes_window_updates_after_receiving_data() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let request_client = Arc::clone(&client);
        let request_thread = std::thread::spawn(move || {
            request_client.request(
                RemoteRequest::ReadFile {
                    path: PathBuf::from("README.md"),
                    max_bytes: None,
                },
                Vec::new(),
            )
        });
        let stream_id = wait_for_v5_request_stream(&output, "fs.read");
        input.push(v5_frames_bytes(v5_response_frames(
            stream_id,
            "fs.read",
            RemoteResponse::ReadFile(FileReadResponse {
                path: PathBuf::from("README.md"),
                size: 11,
                modified_unix_millis: None,
                modified_unix_nanos: None,
                readonly: false,
                truncated: false,
            }),
            b"hello world".to_vec(),
        )));

        let (_, body) = request_thread.join().unwrap().unwrap();
        assert_eq!(body, b"hello world");

        let frames = read_v5_frames(output.bytes());
        let mut connection_credit = 0_u64;
        let mut stream_credit = 0_u64;
        for frame in frames
            .iter()
            .filter(|frame| frame.frame_type == protocol_v5::FrameType::WindowUpdate)
        {
            let update = frame.decode_control::<protocol_v5::WindowUpdate>().unwrap();
            if frame.stream_id == 0 {
                connection_credit += update.credit_bytes;
            } else if frame.stream_id == stream_id {
                stream_credit += update.credit_bytes;
            }
        }
        assert!(connection_credit >= 11);
        assert!(stream_credit <= connection_credit);
        input.close();
    }

    #[test]
    fn v5_multiplexed_client_applies_directory_delta_to_cached_listing() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let initial_listing = v5_test_directory_listing(
            "src",
            10,
            20,
            vec![
                v5_test_directory_entry("src/lib.rs", 12),
                v5_test_directory_entry("src/old.rs", 4),
            ],
        );
        let updated_lib = v5_test_directory_entry("src/lib.rs", 42);
        let added_mod = v5_test_directory_entry("src/mod.rs", 8);

        let first_client = Arc::clone(&client);
        let first_thread = std::thread::spawn(move || {
            first_client.request(
                RemoteRequest::ListDir {
                    path: PathBuf::from("src"),
                },
                Vec::new(),
            )
        });
        let first_stream = wait_for_v5_request_stream(&output, "fs.list_dir");
        input.push(v5_frames_bytes(v5_response_frames(
            first_stream,
            "fs.list_dir",
            RemoteResponse::ListDir(initial_listing),
            Vec::new(),
        )));
        first_thread.join().unwrap().unwrap();

        let second_client = Arc::clone(&client);
        let second_thread = std::thread::spawn(move || {
            second_client.request(
                RemoteRequest::ListDir {
                    path: PathBuf::from("src"),
                },
                Vec::new(),
            )
        });
        let second_stream = wait_for_v5_request_stream_after(&output, "fs.list_dir", first_stream);
        let second_payload: V5DirectoryListPayload =
            decode_v5_request_payload(&output, second_stream).unwrap();
        assert_eq!(second_payload.known_generation, Some(10));
        assert_eq!(second_payload.known_fingerprint, Some(20));
        input.push(v5_frames_bytes(v5_response_frames(
            second_stream,
            "fs.list_dir",
            RemoteResponse::ListDir(DirectoryListingResponse {
                path: PathBuf::from("src"),
                generation: Some(11),
                fingerprint: Some(21),
                complete: true,
                not_modified: false,
                delta: Some(DirectoryListingDeltaResponse {
                    base_generation: Some(10),
                    base_fingerprint: Some(20),
                    added: vec![added_mod.clone()],
                    updated: vec![updated_lib.clone()],
                    removed: vec![PathBuf::from("src/old.rs")],
                }),
                entries: Vec::new(),
            }),
            Vec::new(),
        )));
        let (second_response, _) = second_thread.join().unwrap().unwrap();
        let RemoteResponse::ListDir(second_listing) = second_response else {
            panic!("expected delta-expanded list_dir response");
        };
        assert_eq!(second_listing.generation, Some(11));
        assert_eq!(
            second_listing.entries,
            vec![updated_lib.clone(), added_mod.clone()]
        );
        assert!(second_listing.delta.is_none());

        let third_client = Arc::clone(&client);
        let third_thread = std::thread::spawn(move || {
            third_client.request(
                RemoteRequest::ListDir {
                    path: PathBuf::from("src"),
                },
                Vec::new(),
            )
        });
        let third_stream = wait_for_v5_request_stream_after(&output, "fs.list_dir", second_stream);
        let third_payload: V5DirectoryListPayload =
            decode_v5_request_payload(&output, third_stream).unwrap();
        assert_eq!(third_payload.known_generation, Some(11));
        assert_eq!(third_payload.known_fingerprint, Some(21));
        input.push(v5_frames_bytes(v5_response_frames(
            third_stream,
            "fs.list_dir",
            RemoteResponse::ListDir(DirectoryListingResponse {
                path: PathBuf::from("src"),
                generation: Some(11),
                fingerprint: Some(21),
                complete: true,
                not_modified: true,
                delta: None,
                entries: Vec::new(),
            }),
            Vec::new(),
        )));
        third_thread.join().unwrap().unwrap();
        input.close();
    }

    #[test]
    fn v5_multiplexed_client_sends_second_request_before_first_completes() {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), output.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );

        let (completion_tx, completion_rx) = mpsc::channel();
        let read_client = Arc::clone(&client);
        let read_tx = completion_tx.clone();
        let read_thread = std::thread::spawn(move || {
            let result = read_client.request(
                RemoteRequest::ReadFile {
                    path: PathBuf::from("slow.txt"),
                    max_bytes: None,
                },
                Vec::new(),
            );
            read_tx.send(("read", result)).unwrap();
        });
        let read_stream = wait_for_v5_request_stream(&output, "fs.read");

        let stat_client = Arc::clone(&client);
        let stat_tx = completion_tx.clone();
        let stat_thread = std::thread::spawn(move || {
            let result = stat_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("fast.txt"),
                },
                Vec::new(),
            );
            stat_tx.send(("stat", result)).unwrap();
        });
        let stat_stream = wait_for_v5_request_stream(&output, "fs.stat");

        assert_ne!(read_stream, stat_stream);
        input.push(v5_frames_bytes(v5_response_frames(
            stat_stream,
            "fs.stat",
            RemoteResponse::Stat(FileStatResponse {
                path: PathBuf::from("fast.txt"),
                kind: RemoteFileKind::File,
                size: 4,
                modified_unix_millis: None,
                modified_unix_nanos: None,
                readonly: false,
            }),
            Vec::new(),
        )));
        let first = completion_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first completion");
        assert_eq!(first.0, "stat");
        assert!(matches!(first.1.unwrap().0, RemoteResponse::Stat(_)));

        input.push(v5_frames_bytes(v5_response_frames(
            read_stream,
            "fs.read",
            RemoteResponse::ReadFile(FileReadResponse {
                path: PathBuf::from("slow.txt"),
                size: 4,
                modified_unix_millis: None,
                modified_unix_nanos: None,
                readonly: false,
                truncated: false,
            }),
            b"slow".to_vec(),
        )));
        let second = completion_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("second completion");
        assert_eq!(second.0, "read");
        let (response, body) = second.1.unwrap();
        assert!(matches!(response, RemoteResponse::ReadFile(_)));
        assert_eq!(body, b"slow");

        input.close();
        stat_thread.join().unwrap();
        read_thread.join().unwrap();
    }

    #[test]
    fn v5_service_reads_file_through_protocol_session() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("hello.txt"), b"hello from v5").unwrap();
        let request = RemoteRequest::ReadFile {
            path: PathBuf::from("hello.txt"),
            max_bytes: None,
        };
        let input = v5_client_input(v5_request_frames(1, &request, &[]));
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);

        assert_eq!(frames[0].frame_type, protocol_v5::FrameType::Hello);
        assert_eq!(frames[1].frame_type, protocol_v5::FrameType::Settings);
        let (response, body, error) = decode_v5_service_response(&frames, 1);
        assert!(error.is_none());
        let Some(RemoteResponse::ReadFile(read)) = response else {
            panic!("expected read_file response");
        };
        assert_eq!(read.path, temp.path().join("hello.txt"));
        assert_eq!(body, b"hello from v5");
        assert!(frames.iter().any(|frame| {
            frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::EndStream
        }));
    }

    #[test]
    fn v5_service_shutdown_sends_goaway_after_final_response() {
        let temp = tempfile::tempdir().unwrap();
        let request = RemoteRequest::Shutdown;
        let input = v5_client_input(v5_request_frames(1, &request, &[]));
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let (response, body, error) = decode_v5_service_response(&frames, 1);
        assert!(error.is_none());
        assert_eq!(response, Some(RemoteResponse::Shutdown));
        assert!(body.is_empty());

        let final_response_index = v5_final_response_index(&frames, 1);
        let goaway_index = frames
            .iter()
            .position(|frame| frame.frame_type == protocol_v5::FrameType::GoAway)
            .expect("shutdown should emit GOAWAY");
        assert!(goaway_index > final_response_index);
        let goaway = frames[goaway_index]
            .decode_control::<protocol_v5::GoAway>()
            .unwrap();
        assert_eq!(goaway.last_accepted_stream_id, 1);
        assert_eq!(goaway.code, "OK");
        assert_eq!(
            goaway.drain_grace_ms,
            protocol_v5::DEFAULT_SHUTDOWN_GRACE_MS
        );
    }

    #[test]
    fn v5_service_list_dir_returns_not_modified_for_known_generation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("lib.rs"), b"pub fn lib() {}\n").unwrap();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

        let first_request = RemoteRequest::ListDir {
            path: PathBuf::from("."),
        };
        let first_input = v5_client_input(v5_request_frames(1, &first_request, &[]));
        let mut first_io = protocol_v5::FramedIo::new(Cursor::new(first_input), Vec::new());
        service
            .serve_v5(
                &mut first_io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, first_output) = first_io.into_inner();
        let first_frames = read_v5_frames(first_output);
        let (first_response, _, first_error) = decode_v5_service_response(&first_frames, 1);
        assert!(first_error.is_none());
        let Some(RemoteResponse::ListDir(first_listing)) = first_response else {
            panic!("expected first list_dir response");
        };
        assert!(!first_listing.not_modified);
        assert_eq!(first_listing.entries.len(), 1);
        let generation = first_listing
            .generation
            .expect("list_dir should include a generation");

        let second_payload = V5DirectoryListPayload {
            path: PathBuf::from("."),
            known_generation: Some(generation),
            known_fingerprint: None,
        };
        let second_input =
            v5_client_input(v5_json_request_frames(1, "fs.list_dir", &second_payload));
        let mut second_io = protocol_v5::FramedIo::new(Cursor::new(second_input), Vec::new());
        service
            .serve_v5(
                &mut second_io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, second_output) = second_io.into_inner();
        let second_frames = read_v5_frames(second_output);
        let (second_response, _, second_error) = decode_v5_service_response(&second_frames, 1);
        assert!(second_error.is_none());
        let Some(RemoteResponse::ListDir(second_listing)) = second_response else {
            panic!("expected second list_dir response");
        };
        assert!(second_listing.not_modified);
        assert_eq!(second_listing.generation, Some(generation));
        assert!(second_listing.entries.is_empty());
    }

    #[test]
    fn v5_service_list_dir_returns_delta_for_cached_known_generation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("lib.rs"), b"pub fn lib() {}\n").unwrap();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

        let first_request = RemoteRequest::ListDir {
            path: PathBuf::from("."),
        };
        let first_input = v5_client_input(v5_request_frames(1, &first_request, &[]));
        let mut first_io = protocol_v5::FramedIo::new(Cursor::new(first_input), Vec::new());
        service
            .serve_v5(
                &mut first_io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, first_output) = first_io.into_inner();
        let first_frames = read_v5_frames(first_output);
        let (first_response, _, first_error) = decode_v5_service_response(&first_frames, 1);
        assert!(first_error.is_none());
        let Some(RemoteResponse::ListDir(first_listing)) = first_response else {
            panic!("expected first list_dir response");
        };
        let generation = first_listing
            .generation
            .expect("list_dir should include a generation");
        let fingerprint = first_listing
            .fingerprint
            .expect("list_dir should include a fingerprint");

        std::fs::write(temp.path().join("mod.rs"), b"mod child;\n").unwrap();
        let second_payload = V5DirectoryListPayload {
            path: PathBuf::from("."),
            known_generation: Some(generation),
            known_fingerprint: Some(fingerprint),
        };
        let second_input =
            v5_client_input(v5_json_request_frames(1, "fs.list_dir", &second_payload));
        let mut second_io = protocol_v5::FramedIo::new(Cursor::new(second_input), Vec::new());
        service
            .serve_v5(
                &mut second_io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, second_output) = second_io.into_inner();
        let second_frames = read_v5_frames(second_output);
        let (second_response, _, second_error) = decode_v5_service_response(&second_frames, 1);
        assert!(second_error.is_none());
        let Some(RemoteResponse::ListDir(second_listing)) = second_response else {
            panic!("expected second list_dir response");
        };
        assert!(!second_listing.not_modified);
        assert_ne!(second_listing.generation, Some(generation));
        assert!(second_listing.entries.is_empty());
        let delta = second_listing.delta.expect("expected directory delta");
        assert_eq!(delta.base_generation, Some(generation));
        assert_eq!(delta.base_fingerprint, Some(fingerprint));
        assert_eq!(delta.added.len(), 1);
        assert_eq!(delta.added[0].name, "mod.rs");
        assert!(delta.updated.is_empty());
        assert!(delta.removed.is_empty());
    }

    #[test]
    fn v5_service_list_dirs_returns_delta_for_cached_known_generation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("lib.rs"), b"pub fn lib() {}\n").unwrap();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

        let first_request = RemoteRequest::ListDirs {
            paths: vec![PathBuf::from(".")],
        };
        let first_input = v5_client_input(v5_request_frames(1, &first_request, &[]));
        let mut first_io = protocol_v5::FramedIo::new(Cursor::new(first_input), Vec::new());
        service
            .serve_v5(
                &mut first_io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, first_output) = first_io.into_inner();
        let first_frames = read_v5_frames(first_output);
        let (first_response, _, first_error) = decode_v5_service_response(&first_frames, 1);
        assert!(first_error.is_none());
        let Some(RemoteResponse::ListDirs(first_response)) = first_response else {
            panic!("expected first list_dirs response");
        };
        let first_listing = first_response.results[0]
            .listing
            .as_ref()
            .expect("first list_dirs result should include a listing");
        let generation = first_listing
            .generation
            .expect("list_dirs should include a generation");
        let fingerprint = first_listing
            .fingerprint
            .expect("list_dirs should include a fingerprint");

        std::fs::write(temp.path().join("mod.rs"), b"mod child;\n").unwrap();
        let second_payload = V5DirectoryListDirsPayload {
            paths: Vec::new(),
            entries: vec![V5DirectoryListEntryPayload {
                path: PathBuf::from("."),
                known_generation: Some(generation),
                known_fingerprint: Some(fingerprint),
            }],
        };
        let second_input =
            v5_client_input(v5_json_request_frames(1, "fs.list_dirs", &second_payload));
        let mut second_io = protocol_v5::FramedIo::new(Cursor::new(second_input), Vec::new());
        service
            .serve_v5(
                &mut second_io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, second_output) = second_io.into_inner();
        let second_frames = read_v5_frames(second_output);
        let (second_response, _, second_error) = decode_v5_service_response(&second_frames, 1);
        assert!(second_error.is_none());
        let Some(RemoteResponse::ListDirs(second_response)) = second_response else {
            panic!("expected second list_dirs response");
        };
        let second_listing = second_response.results[0]
            .listing
            .as_ref()
            .expect("second list_dirs result should include a listing");
        assert!(second_listing.entries.is_empty());
        let delta = second_listing
            .delta
            .as_ref()
            .expect("expected list_dirs delta");
        assert_eq!(delta.base_generation, Some(generation));
        assert_eq!(delta.base_fingerprint, Some(fingerprint));
        assert_eq!(delta.added.len(), 1);
        assert_eq!(delta.added[0].name, "mod.rs");
    }

    #[test]
    fn v5_service_list_dir_returns_full_listing_when_delta_base_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("lib.rs"), b"pub fn lib() {}\n").unwrap();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

        let payload = V5DirectoryListPayload {
            path: PathBuf::from("."),
            known_generation: Some(u64::MAX),
            known_fingerprint: Some(u64::MAX - 1),
        };
        let input = v5_client_input(v5_json_request_frames(1, "fs.list_dir", &payload));
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());
        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let (response, _, error) = decode_v5_service_response(&frames, 1);
        assert!(error.is_none());
        let Some(RemoteResponse::ListDir(listing)) = response else {
            panic!("expected list_dir response");
        };
        assert!(!listing.not_modified);
        assert!(listing.delta.is_none());
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "lib.rs");
    }

    #[test]
    fn v5_service_writes_file_body_through_protocol_session() {
        let temp = tempfile::tempdir().unwrap();
        let request = RemoteRequest::WriteFile {
            path: PathBuf::from("nested/out.txt"),
            create_parent_dirs: true,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };
        let input = v5_client_input(v5_request_frames(1, &request, b"written over v5"));
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let (response, body, error) = decode_v5_service_response(&frames, 1);

        assert!(error.is_none());
        assert!(body.is_empty());
        let Some(RemoteResponse::WriteFile(write)) = response else {
            panic!("expected write_file response");
        };
        assert_eq!(write.path, temp.path().join("nested/out.txt"));
        assert_eq!(
            std::fs::read(temp.path().join("nested/out.txt")).unwrap(),
            b"written over v5"
        );
    }

    #[test]
    fn v5_service_commits_zero_byte_write_through_streaming_path() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("out.txt");
        std::fs::write(&target, b"previous contents").unwrap();
        let request = RemoteRequest::WriteFile {
            path: PathBuf::from("out.txt"),
            create_parent_dirs: false,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };
        let input = v5_client_input(v5_request_frames(1, &request, b""));
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let (response, body, error) = decode_v5_service_response(&frames, 1);

        assert!(error.is_none());
        assert!(body.is_empty());
        let Some(RemoteResponse::WriteFile(write)) = response else {
            panic!("expected write_file response");
        };
        assert_eq!(write.size, 0);
        assert!(std::fs::read(target).unwrap().is_empty());
        assert!(v5_write_temp_files(temp.path()).is_empty());
    }

    #[test]
    fn v5_service_reports_unsupported_method_as_final_error() {
        let temp = tempfile::tempdir().unwrap();
        let headers = protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            1,
            &protocol_v5::StreamEnvelope::request(1, "fs.unknown"),
        );
        let payload = protocol_v5::stream_data_frame(
            1,
            b"{}".to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap();
        let input = v5_client_input(vec![
            headers,
            payload,
            protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, 1),
        ]);
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let (response, body, error) = decode_v5_service_response(&frames, 1);

        assert!(response.is_none());
        assert!(body.is_empty());
        let error = error.expect("expected final error");
        assert_eq!(error.code, "invalid_request");
        assert!(error.message.contains("unsupported v5 method"));
        assert!(frames.iter().any(|frame| {
            frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::EndStream
        }));
    }

    #[test]
    fn v5_service_watch_start_returns_degraded_poll_event_stream() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        let start = protocol_v5::WatchStart::expanded_dirs([".", "src", "../outside"]);
        let input = v5_client_input(v5_protobuf_request_frames(1, "watch.start", &start));
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let (response, error) =
            decode_v5_protobuf_service_response::<protocol_v5::WatchStartResponse>(&frames, 1);

        assert!(error.is_none());
        let response = response.expect("expected watch.start response");
        assert_eq!(response.watch_id, 1);
        assert_ne!(response.event_stream_id, 0);
        assert_eq!(response.event_stream_id % 2, 0);
        assert_eq!(response.backend, "poll");
        assert!(response.degraded);
        assert!(response.requires_reconciliation);
        assert_eq!(response.accepted_roots, [".", "src"]);
        assert_eq!(response.unsupported_roots, ["../outside"]);

        let event_headers = frames
            .iter()
            .find(|frame| {
                frame.stream_id == response.event_stream_id
                    && frame.frame_type == protocol_v5::FrameType::Headers
            })
            .expect("expected watch event stream headers");
        let envelope = event_headers
            .decode_control::<protocol_v5::StreamEnvelope>()
            .unwrap();
        assert_eq!(envelope.role, protocol_v5::MessageRole::Event as i32);
        assert_eq!(envelope.method, "watch.batch");
    }

    #[test]
    fn v5_service_rejects_expired_watch_start_before_registering_it() {
        let temp = tempfile::tempdir().unwrap();
        let start = protocol_v5::WatchStart::expanded_dirs(["."]);
        let options = protocol_v5::RequestOptions {
            deadline_unix_ms: v5_now_unix_millis().saturating_sub(1),
            ..protocol_v5::RequestOptions::default()
        };
        let input = v5_client_input(v5_protobuf_request_frames_with_options(
            1,
            "watch.start",
            &start,
            options,
        ));
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let reset = frames
            .iter()
            .find(|frame| {
                frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
            })
            .expect("expired watch.start should be reset")
            .decode_control::<protocol_v5::ResetStream>()
            .unwrap();

        assert_eq!(reset.code, protocol_v5::RESET_DEADLINE_EXCEEDED);
        assert!(!frames.iter().any(|frame| {
            frame.stream_id != 0 && frame.frame_type == protocol_v5::FrameType::Headers
        }));
    }

    #[test]
    fn v5_service_watch_update_and_stop_manage_event_stream() {
        let temp = tempfile::tempdir().unwrap();
        let start = protocol_v5::WatchStart::expanded_dirs(["."]);
        let update = protocol_v5::WatchUpdate {
            watch_id: 1,
            add_roots: vec!["crates".to_string(), "../outside".to_string()],
            remove_roots: vec![".".to_string()],
        };
        let stop = protocol_v5::WatchStop { watch_id: 1 };
        let mut request_frames = v5_protobuf_request_frames(1, "watch.start", &start);
        request_frames.extend(v5_protobuf_request_frames(3, "watch.update", &update));
        request_frames.extend(v5_protobuf_request_frames(5, "watch.stop", &stop));
        let input = v5_client_input(request_frames);
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let (start_response, start_error) =
            decode_v5_protobuf_service_response::<protocol_v5::WatchStartResponse>(&frames, 1);
        let (update_response, update_error) =
            decode_v5_protobuf_service_response::<protocol_v5::WatchUpdateResponse>(&frames, 3);

        assert!(start_error.is_none());
        assert!(update_error.is_none());
        let event_stream_id = start_response.unwrap().event_stream_id;
        let update_response = update_response.expect("expected watch.update response");
        assert_eq!(update_response.watch_id, 1);
        assert_eq!(update_response.accepted_roots, ["crates"]);
        assert_eq!(update_response.unsupported_roots, ["../outside"]);
        assert!(frames.iter().any(|frame| {
            frame.stream_id == event_stream_id
                && frame.frame_type == protocol_v5::FrameType::EndStream
        }));
        assert!(frames.iter().any(|frame| {
            frame.stream_id == 5 && frame.frame_type == protocol_v5::FrameType::EndStream
        }));
    }

    #[test]
    fn v5_service_watch_resync_emits_resync_batch() {
        let temp = tempfile::tempdir().unwrap();
        let start = protocol_v5::WatchStart::expanded_dirs(["."]);
        let resync = protocol_v5::WatchResync {
            watch_id: 1,
            roots: vec![
                ".".to_string(),
                "missing".to_string(),
                "../outside".to_string(),
            ],
        };
        let mut request_frames = v5_protobuf_request_frames(1, "watch.start", &start);
        request_frames.extend(v5_protobuf_request_frames(3, "watch.resync", &resync));
        let input = v5_client_input(request_frames);
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

        service
            .serve_v5(
                &mut io,
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
        let (_, output) = io.into_inner();
        let frames = read_v5_frames(output);
        let (start_response, start_error) =
            decode_v5_protobuf_service_response::<protocol_v5::WatchStartResponse>(&frames, 1);
        let (resync_response, resync_error) =
            decode_v5_protobuf_service_response::<protocol_v5::WatchResyncResponse>(&frames, 3);

        assert!(start_error.is_none());
        assert!(resync_error.is_none());
        let event_stream_id = start_response.unwrap().event_stream_id;
        let response = resync_response.expect("expected watch.resync response");
        assert_eq!(response.watch_id, 1);
        assert_eq!(response.accepted_roots, ["."]);
        assert_eq!(response.unsupported_roots, ["../outside", "missing"]);

        let batch = find_v5_watch_batch_in_frames(&frames, event_stream_id)
            .expect("expected watch.resync to emit a resync batch");
        assert_eq!(batch.watch_id, 1);
        assert_eq!(batch.sequence, 1);
        assert!(batch.resync_required);
        assert!(!batch.overflow);
        assert_eq!(batch.directory_generations[0].path, ".");
    }

    #[test]
    fn v5_watch_registry_polling_emits_batches_for_changed_roots() {
        let temp = tempfile::tempdir().unwrap();
        let mut watches = V5WatchRegistry::default();
        let watch_id = watches.allocate_watch_id().unwrap();
        let status = watches.start(watch_id, 2, vec![".".to_string()], 50, 500, temp.path());
        assert_eq!(status.backend, "poll");
        assert!(status.degraded);

        std::thread::sleep(Duration::from_millis(60));
        assert!(watches.poll_due(temp.path()).unwrap().is_empty());

        std::fs::write(temp.path().join("new.txt"), b"changed").unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let batches = watches.poll_due(temp.path()).unwrap();

        assert_eq!(batches.len(), 1);
        let (event_stream_id, batch) = &batches[0];
        assert_eq!(*event_stream_id, 2);
        assert_eq!(batch.watch_id, watch_id);
        assert_eq!(batch.sequence, 1);
        assert_eq!(batch.directory_generations[0].path, ".");
        assert_eq!(batch.directory_generations[0].generation, 1);
        assert_eq!(
            batch.events[0].kind,
            protocol_v5::WatchChangeKind::Modified as i32
        );
        assert_eq!(batch.events[0].path, ".");
        assert!(batch.events[0].is_dir);
    }

    #[test]
    fn v5_watch_registry_native_events_emit_batches_for_nearest_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        let (events_tx, _events_rx) = mpsc::channel();
        let (native_tx, _native_rx) = mpsc::sync_channel(V5_NATIVE_WATCH_EVENT_CAPACITY);
        let mut watches =
            V5WatchRegistry::with_native_events(V5NativeWatchSender::new(native_tx, events_tx));
        let watch_id = watches.allocate_watch_id().unwrap();
        watches.start(
            watch_id,
            2,
            vec![".".to_string(), "src".to_string()],
            50,
            500,
            temp.path(),
        );

        let event = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(temp.path().join("src/lib.rs"));
        watches
            .record_native_event(watch_id, Ok(event), temp.path())
            .unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let batches = watches.poll_due(temp.path()).unwrap();

        assert_eq!(batches.len(), 1);
        let (event_stream_id, batch) = &batches[0];
        assert_eq!(*event_stream_id, 2);
        assert_eq!(batch.watch_id, watch_id);
        assert_eq!(batch.sequence, 1);
        assert_eq!(batch.directory_generations[0].path, "src");
        assert_eq!(batch.events[0].path, "src/lib.rs");
        assert_eq!(
            batch.events[0].kind,
            protocol_v5::WatchChangeKind::Created as i32
        );
        assert!(!batch.events[0].is_dir);
    }

    #[test]
    fn v5_watch_registry_collapses_event_overflow_to_resync() {
        let temp = tempfile::tempdir().unwrap();
        let mut watches = V5WatchRegistry::default();
        let watch_id = watches.allocate_watch_id().unwrap();
        watches.start(watch_id, 2, vec![".".to_string()], 50, 2, temp.path());

        let event = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(temp.path().join("one.txt"))
            .add_path(temp.path().join("two.txt"))
            .add_path(temp.path().join("three.txt"));
        watches
            .record_native_event(watch_id, Ok(event), temp.path())
            .unwrap();
        std::thread::sleep(Duration::from_millis(60));

        let batches = watches.poll_due(temp.path()).unwrap();
        assert_eq!(batches.len(), 1);
        let (_, batch) = &batches[0];
        assert!(batch.overflow);
        assert!(batch.resync_required);
        assert!(batch.events.is_empty());
        assert!(batch.directory_generations.is_empty());
    }

    #[test]
    fn v5_watch_event_limit_defaults_and_has_a_hard_cap() {
        assert_eq!(v5_watch_event_limit(0), V5_DEFAULT_WATCH_EVENTS_PER_BATCH);
        assert_eq!(v5_watch_event_limit(7), 7);
        assert_eq!(
            v5_watch_event_limit(u32::MAX),
            V5_MAX_WATCH_EVENTS_PER_BATCH
        );
    }

    #[test]
    fn v5_concurrent_service_emits_watch_batch_on_open_connection() {
        let temp = tempfile::tempdir().unwrap();
        let start = protocol_v5::WatchStart {
            debounce_ms: 50,
            ..protocol_v5::WatchStart::expanded_dirs(["missing"])
        };
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_protobuf_request_frames(
            1,
            "watch.start",
            &start,
        )));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let info = protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string());
        let service_input = input.clone();
        let service_output = output.clone();
        let service_thread = std::thread::spawn(move || {
            service
                .serve_v5_concurrent(
                    protocol_v5::FramedIo::new(service_input, service_output),
                    &info,
                )
                .unwrap();
        });

        let started = Instant::now();
        let watch = loop {
            if let Some(response) = find_v5_watch_start_response(&output, 1) {
                break response;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for watch.start response"
            );
            std::thread::sleep(Duration::from_millis(10));
        };

        std::fs::create_dir(temp.path().join("missing")).unwrap();
        let started = Instant::now();
        let batch = loop {
            if let Some(batch) = find_v5_watch_batch(&output, watch.event_stream_id) {
                break batch;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for watch.batch"
            );
            std::thread::sleep(Duration::from_millis(10));
        };

        input.close();
        service_thread.join().unwrap();

        assert_eq!(batch.watch_id, watch.watch_id);
        assert_eq!(batch.sequence, 1);
        assert_eq!(batch.directory_generations[0].path, "missing");
        assert_eq!(batch.events[0].path, "missing");
        assert_eq!(
            batch.events[0].kind,
            protocol_v5::WatchChangeKind::Modified as i32
        );
    }

    #[test]
    fn v5_concurrent_service_rejects_expired_watch_start_before_registering_it() {
        let temp = tempfile::tempdir().unwrap();
        let start = protocol_v5::WatchStart::expanded_dirs(["."]);
        let options = protocol_v5::RequestOptions {
            deadline_unix_ms: v5_now_unix_millis().saturating_sub(1),
            ..protocol_v5::RequestOptions::default()
        };
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_protobuf_request_frames_with_options(
            1,
            "watch.start",
            &start,
            options,
        )));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );

        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::ResetStream);
        input.close();
        service_thread.join().unwrap();
        let frames = read_v5_frames(output.bytes());
        let reset = frames
            .iter()
            .find(|frame| {
                frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
            })
            .expect("expired watch.start should be reset")
            .decode_control::<protocol_v5::ResetStream>()
            .unwrap();

        assert_eq!(reset.code, protocol_v5::RESET_DEADLINE_EXCEEDED);
        assert!(!frames.iter().any(|frame| {
            frame.stream_id != 0 && frame.frame_type == protocol_v5::FrameType::Headers
        }));
    }

    #[test]
    fn v5_concurrent_service_streams_local_file_body_before_final_response() {
        let temp = tempfile::tempdir().unwrap();
        let body = vec![b'a'; protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize + 123];
        std::fs::write(temp.path().join("large.txt"), &body).unwrap();
        let read = RemoteRequest::ReadFile {
            path: PathBuf::from("large.txt"),
            max_bytes: None,
        };
        let mut options = read.v5_request_options();
        options.priority = protocol_v5::Priority::UserInput;
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames_with_options(
            1,
            &read,
            &[],
            options,
        )));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
        input.close();
        service_thread.join().unwrap();

        let frames = read_v5_frames(output.bytes());
        let (response, read_body, error) = decode_v5_service_response(&frames, 1);
        assert!(error.is_none());
        let Some(RemoteResponse::ReadFile(read_response)) = response else {
            panic!("expected streamed read response");
        };
        assert_eq!(read_response.size, body.len() as u64);
        assert!(!read_response.truncated);
        assert_eq!(read_body, body);
        assert_v5_data_channel_priority(
            &frames,
            1,
            protocol_v5::DataChannel::FileBody,
            protocol_v5::Priority::UserInput,
        );

        let first_file_body_index = frames
            .iter()
            .position(|frame| {
                if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Data {
                    return false;
                }
                let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                protocol_v5::DataChannel::try_from(envelope.channel).unwrap()
                    == protocol_v5::DataChannel::FileBody
            })
            .expect("expected streamed file body DATA frame");
        assert!(
            first_file_body_index < v5_final_response_index(&frames, 1),
            "file body DATA should be queued before final response headers"
        );
        let file_body_frames = frames
            .iter()
            .filter(|frame| {
                if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Data {
                    return false;
                }
                let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                protocol_v5::DataChannel::try_from(envelope.channel).unwrap()
                    == protocol_v5::DataChannel::FileBody
            })
            .count();
        assert!(file_body_frames >= 2);
    }

    #[test]
    fn v5_concurrent_service_streams_write_body_to_temp_file() {
        let temp = tempfile::tempdir().unwrap();
        let write = RemoteRequest::WriteFile {
            path: PathBuf::from("src/main.rs"),
            create_parent_dirs: true,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };
        let (method, payload) = write.to_v5_method_payload().unwrap();
        let frames = vec![
            protocol_v5::Frame::from_control(
                protocol_v5::FrameType::Headers,
                1,
                &protocol_v5::StreamEnvelope::request_with_options(
                    1,
                    method,
                    &write.v5_request_options(),
                ),
            ),
            protocol_v5::stream_data_frame(
                1,
                payload,
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
            )
            .unwrap(),
            protocol_v5::stream_data_frame(
                1,
                b"fn main".to_vec(),
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
            )
            .unwrap(),
            protocol_v5::stream_data_frame(
                1,
                b"() {}\n".to_vec(),
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
            )
            .unwrap(),
            protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, 1),
        ];
        let input = BlockingRead::default();
        input.push(v5_client_input(frames));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
        input.close();
        service_thread.join().unwrap();

        let frames = read_v5_frames(output.bytes());
        let (response, body, error) = decode_v5_service_response(&frames, 1);
        assert!(error.is_none());
        assert!(body.is_empty());
        let Some(RemoteResponse::WriteFile(write_response)) = response else {
            panic!("expected write response");
        };
        assert_eq!(write_response.size, "fn main() {}\n".len() as u64);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("src").join("main.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert!(v5_write_temp_files(&temp.path().join("src")).is_empty());
    }

    #[test]
    fn v5_streaming_write_cancellation_before_commit_preserves_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("main.rs");
        std::fs::write(&target, b"old").unwrap();
        let mut write = V5StreamingWrite::create(target.clone(), false, None).unwrap();
        write.write_chunk(b"new contents").unwrap();
        let cancellation = WorkspaceCancellationToken::new();
        cancellation.cancel();

        let error = write.finish(Some(&cancellation)).unwrap_err();

        assert!(matches!(error, WorkspaceError::Cancelled { .. }));
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(v5_write_temp_files(temp.path()).is_empty());
    }

    #[test]
    fn v5_concurrent_service_cleans_streaming_write_temp_file_on_reset() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("main.rs");
        std::fs::write(&target, "old").unwrap();
        let write = RemoteRequest::WriteFile {
            path: PathBuf::from("main.rs"),
            create_parent_dirs: false,
            expected_modified_unix_millis: None,
            expected_modified_unix_nanos: None,
        };
        let (method, payload) = write.to_v5_method_payload().unwrap();
        let frames = vec![
            protocol_v5::Frame::from_control(
                protocol_v5::FrameType::Headers,
                1,
                &protocol_v5::StreamEnvelope::request_with_options(
                    1,
                    method,
                    &write.v5_request_options(),
                ),
            ),
            protocol_v5::stream_data_frame(
                1,
                payload,
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
            )
            .unwrap(),
            protocol_v5::stream_data_frame(
                1,
                b"new".to_vec(),
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
            )
            .unwrap(),
        ];
        let input = BlockingRead::default();
        input.push(v5_client_input(frames));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let workspace_path = temp.path().to_path_buf();
        let service_input = input.clone();
        let service_output = output.clone();
        let service_thread = std::thread::spawn(move || {
            service
                .serve_v5_concurrent(
                    protocol_v5::FramedIo::new(service_input, service_output),
                    &protocol_v5::ServerHandshakeInfo::current(
                        workspace_path.display().to_string(),
                    ),
                )
                .unwrap();
        });

        let started = Instant::now();
        loop {
            if !v5_write_temp_files(temp.path()).is_empty() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for streaming write temp file"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut reset = Vec::new();
        protocol_v5::write_frame(
            &mut reset,
            &protocol_v5::reset_stream_frame(1, protocol_v5::RESET_CANCELLED, "write cancelled"),
        )
        .unwrap();
        input.push(reset);
        input.close();
        service_thread.join().unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert!(v5_write_temp_files(temp.path()).is_empty());
        let frames = read_v5_frames(output.bytes());
        assert!(
            !frames.iter().any(|frame| frame.stream_id == 1
                && matches!(
                    frame.frame_type,
                    protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
                )),
            "canceled write stream should not receive final headers or END_STREAM"
        );
    }

    #[test]
    fn v5_search_partial_flushes_by_count_or_elapsed_interval() {
        let mut flush = V5SearchPartialFlush::new();

        assert!(!flush.should_flush(0));
        assert!(!flush.should_flush(1));
        assert!(flush.should_flush(V5_SEARCH_PARTIAL_BATCH_SIZE));

        flush.last_emit = Instant::now() - Duration::from_millis(V5_SEARCH_PARTIAL_INTERVAL_MS);
        assert!(flush.should_flush(1));

        flush.mark_flushed();
        assert!(!flush.should_flush(1));
    }

    #[test]
    fn v5_streamed_read_drops_chunk_when_cancelled_during_read() {
        struct CancellingReader {
            cancellation: WorkspaceCancellationToken,
            reads: Arc<AtomicUsize>,
        }

        impl Read for CancellingReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                self.reads.fetch_add(1, Ordering::AcqRel);
                buffer[..4].copy_from_slice(b"data");
                self.cancellation.cancel();
                Ok(4)
            }
        }

        let cancellation = WorkspaceCancellationToken::new();
        let reads = Arc::new(AtomicUsize::new(0));
        let mut emitted = Vec::new();
        let result = v5_stream_file_chunks(
            CancellingReader {
                cancellation: cancellation.clone(),
                reads: Arc::clone(&reads),
            },
            4,
            Path::new("document.txt"),
            &cancellation,
            |body| {
                emitted.push(body);
                Ok(())
            },
        );

        assert!(matches!(
            result,
            Err(RemoteError { code, .. }) if code == protocol_v5::RESET_CANCELLED
        ));
        assert_eq!(reads.load(Ordering::Acquire), 1);
        assert!(emitted.is_empty());
    }

    #[test]
    fn v5_streamed_read_stops_after_cancelled_emission() {
        let cancellation = WorkspaceCancellationToken::new();
        let mut emitted = Vec::new();
        let body = vec![7_u8; protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize + 1];
        let result = v5_stream_file_chunks(
            Cursor::new(body),
            protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as u64 + 1,
            Path::new("document.txt"),
            &cancellation,
            |chunk| {
                emitted.push(chunk);
                cancellation.cancel();
                Ok(())
            },
        );

        assert!(matches!(
            result,
            Err(RemoteError { code, .. }) if code == protocol_v5::RESET_CANCELLED
        ));
        assert_eq!(emitted.len(), 1);
    }

    #[test]
    fn v5_server_output_queue_backpressures_without_blocking_control_events() {
        let (control_tx, control_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(1);
        let output_events = V5ServeOutputSender::new(output_tx, control_tx.clone());
        let output = |byte| V5ServeOutputEvent::StreamData {
            stream_id: 7,
            channel: protocol_v5::DataChannel::Stdout,
            body: vec![byte],
            priority: protocol_v5::Priority::LspSupport,
        };

        output_events.send(output(1)).unwrap();
        assert!(matches!(
            control_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            V5ServeEvent::Output
        ));
        output_events.clear_ready();

        let blocked_sender = output_events.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (finished_tx, finished_rx) = mpsc::sync_channel(0);
        let producer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = blocked_sender.send(output(2));
            finished_tx.send(result).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        control_tx.send(V5ServeEvent::NativeWatch).unwrap();
        assert!(matches!(
            control_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            V5ServeEvent::NativeWatch
        ));
        assert!(matches!(
            finished_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        let first = output_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            first,
            V5ServeOutputEvent::StreamData { stream_id: 7, .. }
        ));
        output_events.mark_delivered();
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        let second = output_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            second,
            V5ServeOutputEvent::StreamData { stream_id: 7, .. }
        ));
        output_events.mark_delivered();
        producer.join().unwrap();
    }

    #[test]
    fn v5_server_output_queue_rejects_events_over_the_byte_budget() {
        let (control_tx, control_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(V5_SERVE_OUTPUT_EVENT_CAPACITY);
        let output_events = V5ServeOutputSender::new(output_tx, control_tx);
        let retained_bytes = V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES + 1;

        let error = output_events
            .send(V5ServeOutputEvent::StreamData {
                stream_id: 9,
                channel: protocol_v5::DataChannel::FileBody,
                body: vec![0; retained_bytes],
                priority: protocol_v5::Priority::ForegroundDocument,
            })
            .unwrap_err();

        assert_eq!(
            error,
            V5ServeQueueError::EventTooLarge {
                retained_bytes,
                max: V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES,
            }
        );
        assert!(matches!(
            output_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(matches!(
            control_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(!output_events.has_pending_output());
    }

    #[test]
    fn v5_error_completion_discards_oversized_string_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut method = String::with_capacity(1024 * 1024);
        method.push_str("fs.list_dirs");
        let mut code = String::with_capacity(1024 * 1024);
        code.push_str("remote");
        let error = RemoteError {
            code,
            message: "m".repeat(1024 * 1024),
            diagnostic: Some("d".repeat(1024 * 1024)),
        };
        let (control_tx, _control_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(1);
        let output_events = V5ServeOutputSender::new(output_tx, control_tx);
        let cancellation = WorkspaceCancellationToken::new();

        assert!(
            service
                .enqueue_v5_service_completion(
                    V5ServiceCompletion {
                        stream_id: 7,
                        method,
                        result: Err(error),
                    },
                    protocol_v5::Priority::VisibleFileTree,
                    &output_events,
                    &cancellation,
                )
                .unwrap()
        );

        let event = output_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(event.retained_bytes() <= V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES);
        let V5ServeOutputEvent::Completed(completion) = event else {
            panic!("expected terminal error completion");
        };
        assert_eq!(completion.method.capacity(), completion.method.len());
        let Err(error) = completion.result else {
            panic!("expected terminal error result");
        };
        assert_eq!(error.code.capacity(), error.code.len());
        assert_eq!(error.message.len(), 16 * 1024);
        assert_eq!(error.message.capacity(), error.message.len());
        let diagnostic = error.diagnostic.unwrap();
        assert_eq!(diagnostic.len(), 32 * 1024);
        assert_eq!(diagnostic.capacity(), diagnostic.len());
        output_events.mark_delivered();
        assert!(!output_events.has_pending_output());
    }

    #[test]
    fn v5_cancellable_output_send_unblocks_when_full() {
        let (control_tx, _control_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(1);
        let output_events = V5ServeOutputSender::new(output_tx, control_tx);
        let output = |byte| V5ServeOutputEvent::StreamData {
            stream_id: 7,
            channel: protocol_v5::DataChannel::FileBody,
            body: vec![byte],
            priority: protocol_v5::Priority::ForegroundDocument,
        };
        output_events.send(output(1)).unwrap();
        let cancellation = WorkspaceCancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let worker_output = output_events.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker_output.send_with_cancellation(output(2), &worker_cancellation)
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let started = Instant::now();
        while output_events.pending_count.load(Ordering::Acquire) < 2 {
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "cancellable sender did not reach the full queue"
            );
            std::thread::yield_now();
        }
        cancellation.cancel();

        assert_eq!(worker.join().unwrap(), Err(V5ServeQueueError::Cancelled));
        assert_eq!(output_events.pending_count.load(Ordering::Acquire), 1);
        assert!(matches!(
            output_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            V5ServeOutputEvent::StreamData { body, .. } if body == vec![1]
        ));
        output_events.mark_delivered();
        assert!(!output_events.has_pending_output());
    }

    #[test]
    fn v5_cancelled_completion_stops_before_terminal_output() {
        let temp = tempfile::tempdir().unwrap();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let response = RemoteResponse::FileSearch(FileSearchResponse {
            root: PathBuf::from("."),
            files: (0..4_096)
                .map(|index| PathBuf::from(format!("src/nested/module_{index:04}.rs")))
                .collect(),
            truncated: false,
        });
        let (control_tx, _control_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(1);
        let output_events = V5ServeOutputSender::new(output_tx, control_tx);
        let cancellation = WorkspaceCancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let worker_output = output_events.clone();
        let worker = std::thread::spawn(move || {
            service.enqueue_v5_service_completion(
                V5ServiceCompletion {
                    stream_id: 7,
                    method: "search.files".to_string(),
                    result: Ok(ServiceOutcome::continue_response(response, Vec::new())),
                },
                protocol_v5::Priority::LspSupport,
                &worker_output,
                &worker_cancellation,
            )
        });

        let started = Instant::now();
        while output_events.pending_count.load(Ordering::Acquire) < 2 {
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "completion did not block behind its first serialized chunk"
            );
            std::thread::yield_now();
        }
        cancellation.cancel();

        assert_eq!(worker.join().unwrap(), Ok(false));
        assert_eq!(output_events.completion_budget.used(), 0);
        assert_eq!(output_events.pending_count.load(Ordering::Acquire), 1);
        assert!(matches!(
            output_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            V5ServeOutputEvent::StreamData { stream_id: 7, .. }
        ));
        output_events.mark_delivered();
        assert!(matches!(
            output_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(!output_events.has_pending_output());
    }

    #[test]
    fn v5_service_completion_serializes_large_payloads_in_bounded_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let response = RemoteResponse::FileSearch(FileSearchResponse {
            root: PathBuf::from("."),
            files: (0..4_096)
                .map(|index| PathBuf::from(format!("src/nested/module_{index:04}.rs")))
                .collect(),
            truncated: false,
        });
        let expected_payload = response.to_v5_payload().unwrap();
        assert!(expected_payload.len() > V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES);

        let (control_tx, _control_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(8);
        let output_events = V5ServeOutputSender::new(output_tx, control_tx);
        let cancellation = WorkspaceCancellationToken::new();
        assert!(
            service
                .enqueue_v5_service_completion(
                    V5ServiceCompletion {
                        stream_id: 7,
                        method: "search.files".to_string(),
                        result: Ok(ServiceOutcome::continue_response(response, Vec::new())),
                    },
                    protocol_v5::Priority::LspSupport,
                    &output_events,
                    &cancellation,
                )
                .unwrap()
        );

        let mut chunks = 0;
        let mut actual_payload = Vec::new();
        let mut completed = false;
        for event in output_rx.try_iter() {
            match event {
                V5ServeOutputEvent::StreamData {
                    stream_id,
                    channel,
                    body,
                    priority,
                } => {
                    assert_eq!(stream_id, 7);
                    assert_eq!(channel, protocol_v5::DataChannel::Unspecified);
                    assert_eq!(priority, protocol_v5::Priority::LspSupport);
                    assert!(body.capacity() <= V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES);
                    chunks += 1;
                    actual_payload.extend(body);
                }
                V5ServeOutputEvent::Completed(completion) => {
                    assert_eq!(completion.stream_id, 7);
                    assert!(matches!(
                        completion.result,
                        Ok(V5ServiceTerminalOutcome::Continue)
                    ));
                    completed = true;
                }
                other => panic!(
                    "unexpected service output event: {:?}",
                    other.retained_bytes()
                ),
            }
        }

        assert!(chunks > 1);
        assert!(completed);
        assert_eq!(actual_payload, expected_payload);
        assert_eq!(output_events.completion_budget.used(), 0);
    }

    #[test]
    fn v5_service_completion_budget_is_held_while_output_is_backpressured() {
        let temp = tempfile::tempdir().unwrap();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let response = RemoteResponse::Shutdown;
        let payload_len = response.to_v5_payload().unwrap().len();
        let budget = V5ConnectionByteBudget::new(payload_len);
        let (control_tx, _control_rx) = mpsc::channel();
        let (output_tx, output_rx) = mpsc::sync_channel(0);
        let output_events =
            V5ServeOutputSender::with_completion_budget(output_tx, control_tx, budget.clone());
        let worker_output = output_events.clone();
        let cancellation = WorkspaceCancellationToken::new();
        let worker = std::thread::spawn(move || {
            service.enqueue_v5_service_completion(
                V5ServiceCompletion {
                    stream_id: 9,
                    method: "session.shutdown".to_string(),
                    result: Ok(ServiceOutcome::Shutdown),
                },
                protocol_v5::Priority::UserInput,
                &worker_output,
                &cancellation,
            )
        });

        let started = Instant::now();
        while budget.used() == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "completion did not reserve its encoded bytes"
            );
            std::thread::yield_now();
        }
        let error = output_events.reserve_completion_bytes(1).unwrap_err();
        assert_eq!(error.code, "resource_exhausted");

        assert!(matches!(
            output_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            V5ServeOutputEvent::StreamData { stream_id: 9, .. }
        ));
        assert!(matches!(
            output_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            V5ServeOutputEvent::Completed(V5ServiceTerminal {
                stream_id: 9,
                result: Ok(V5ServiceTerminalOutcome::Shutdown),
                ..
            })
        ));
        worker.join().unwrap().unwrap();
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn v5_native_watch_queue_overflow_requests_explicit_resync() {
        let (control_tx, control_rx) = mpsc::channel();
        let (native_tx, native_rx) = mpsc::sync_channel(1);
        let native_events = V5NativeWatchSender::new(native_tx, control_tx);

        native_events
            .send(V5NativeWatchEvent {
                watch_id: 11,
                result: Ok(notify::Event::new(notify::EventKind::Any)),
            })
            .unwrap();
        assert!(matches!(
            control_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            V5ServeEvent::NativeWatch
        ));
        native_events.clear_ready();
        native_events
            .send(V5NativeWatchEvent {
                watch_id: 11,
                result: Ok(notify::Event::new(notify::EventKind::Any)),
            })
            .unwrap();
        assert!(matches!(
            control_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            V5ServeEvent::NativeWatch
        ));
        assert_eq!(native_events.take_overflowed_watch_ids(), vec![11]);
        assert_eq!(
            native_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .watch_id,
            11
        );

        let mut subscription = V5WatchSubscription::new(11, 3, 50, 100, None);
        subscription.roots.insert(".".to_string());
        subscription
            .pending_events
            .push(protocol_v5::WatchChange::modified("src/lib.rs", false));
        subscription.record_native_overflow();
        assert!(subscription.pending_events.is_empty());
        assert!(subscription.pending_overflow);
        assert!(subscription.pending_resync_required);
        assert!(subscription.next_emit.is_some());
    }

    #[test]
    fn v5_concurrent_service_streams_file_search_partial_results() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        std::fs::create_dir(&src).unwrap();
        for index in 0..105 {
            std::fs::write(src.join(format!("file-{index:03}.rs")), "").unwrap();
        }
        let search = RemoteRequest::FileSearch(FileSearchRequest {
            root: PathBuf::new(),
            pattern: Some("file-".to_string()),
            limit: 200,
            hidden: true,
            ..FileSearchRequest::default()
        });
        let mut options = search.v5_request_options();
        options.priority = protocol_v5::Priority::UserInput;
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames_with_options(
            1,
            &search,
            &[],
            options,
        )));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
        input.close();
        service_thread.join().unwrap();

        let frames = read_v5_frames(output.bytes());
        assert_v5_data_channel_priority(
            &frames,
            1,
            protocol_v5::DataChannel::SearchPayload,
            protocol_v5::Priority::UserInput,
        );
        let partials = decode_v5_partial_file_search_responses(&frames, 1);
        assert_eq!(partials.len(), 1);
        assert_eq!(partials[0].files.len(), V5_SEARCH_PARTIAL_BATCH_SIZE);
        assert!(!partials[0].truncated);
        let progress = decode_v5_progress_headers(&frames, 1, "search.files");
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].message, "file search matches");
        assert_eq!(progress[0].completed, V5_SEARCH_PARTIAL_BATCH_SIZE as u64);
        assert_eq!(progress[0].total, 200);

        let (response, body, error) = decode_v5_service_response(&frames, 1);
        assert!(error.is_none());
        assert!(body.is_empty());
        let Some(RemoteResponse::FileSearch(final_response)) = response else {
            panic!("expected file search response");
        };
        assert_eq!(final_response.files.len(), 5);
        assert!(!final_response.truncated);
        let mut aggregate_files = partials[0].files.clone();
        aggregate_files.extend(final_response.files.clone());
        aggregate_files.sort();
        assert_eq!(aggregate_files.len(), 105);
        assert_eq!(aggregate_files[0], PathBuf::from("src/file-000.rs"));
        assert_eq!(aggregate_files[104], PathBuf::from("src/file-104.rs"));

        let partial_index = frames
            .iter()
            .position(|frame| {
                if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Headers {
                    return false;
                }
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                envelope.role == protocol_v5::MessageRole::PartialResult as i32
                    && envelope.method == "search.files"
            })
            .expect("expected partial file search response");
        assert!(
            partial_index < v5_final_response_index(&frames, 1),
            "partial search response should be queued before final response"
        );
        let progress_index = frames
            .iter()
            .position(|frame| {
                if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Headers {
                    return false;
                }
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                envelope.role == protocol_v5::MessageRole::Progress as i32
                    && envelope.method == "search.files"
            })
            .expect("expected file search progress");
        assert!(
            progress_index < v5_final_response_index(&frames, 1),
            "file search progress should be queued before final response"
        );
    }

    #[test]
    fn v5_concurrent_service_streams_text_search_partial_results() {
        let temp = tempfile::tempdir().unwrap();
        let body = (0..105)
            .map(|index| format!("needle line {index}\n"))
            .collect::<String>();
        std::fs::write(temp.path().join("main.rs"), body).unwrap();
        let search = RemoteRequest::TextSearch(TextSearchRequest {
            root: PathBuf::new(),
            pattern: "needle".to_string(),
            limit: 200,
            hidden: true,
            ..TextSearchRequest::default()
        });
        let mut options = search.v5_request_options();
        options.priority = protocol_v5::Priority::UserInput;
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames_with_options(
            1,
            &search,
            &[],
            options,
        )));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
        input.close();
        service_thread.join().unwrap();

        let frames = read_v5_frames(output.bytes());
        assert_v5_data_channel_priority(
            &frames,
            1,
            protocol_v5::DataChannel::SearchPayload,
            protocol_v5::Priority::UserInput,
        );
        let partials = decode_v5_partial_text_search_responses(&frames, 1);
        assert_eq!(partials.len(), 1);
        assert_eq!(partials[0].matches.len(), V5_SEARCH_PARTIAL_BATCH_SIZE);
        assert_eq!(
            partials[0].matches[0].relative_path,
            PathBuf::from("main.rs")
        );
        assert_eq!(partials[0].matches[0].line_number, 1);
        assert!(!partials[0].truncated);
        let progress = decode_v5_progress_headers(&frames, 1, "search.text");
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].message, "text search matches");
        assert_eq!(progress[0].completed, V5_SEARCH_PARTIAL_BATCH_SIZE as u64);
        assert_eq!(progress[0].total, 200);

        let (response, body, error) = decode_v5_service_response(&frames, 1);
        assert!(error.is_none());
        assert!(body.is_empty());
        let Some(RemoteResponse::TextSearch(final_response)) = response else {
            panic!("expected text search response");
        };
        assert_eq!(final_response.matches.len(), 5);
        assert!(!final_response.truncated);
        assert_eq!(final_response.matches[0].line_number, 101);
        assert_eq!(final_response.matches[4].line_number, 105);

        let partial_index = frames
            .iter()
            .position(|frame| {
                if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Headers {
                    return false;
                }
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                envelope.role == protocol_v5::MessageRole::PartialResult as i32
                    && envelope.method == "search.text"
            })
            .expect("expected partial text search response");
        assert!(
            partial_index < v5_final_response_index(&frames, 1),
            "partial text search response should be queued before final response"
        );
        let progress_index = frames
            .iter()
            .position(|frame| {
                if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Headers {
                    return false;
                }
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                envelope.role == protocol_v5::MessageRole::Progress as i32
                    && envelope.method == "search.text"
            })
            .expect("expected text search progress");
        assert!(
            progress_index < v5_final_response_index(&frames, 1),
            "text search progress should be queued before final response"
        );
    }

    #[test]
    fn v5_concurrent_service_cancels_search_after_reset_without_results() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..200 {
            std::fs::write(temp.path().join(format!("file-{index:03}.txt")), "needle\n").unwrap();
        }
        let search = RemoteRequest::TextSearch(TextSearchRequest {
            root: PathBuf::new(),
            pattern: "needle".to_string(),
            limit: 1_000,
            hidden: true,
            ..TextSearchRequest::default()
        });
        let mut request_frames = v5_request_frames(1, &search, &[]);
        request_frames.push(protocol_v5::reset_stream_frame(
            1,
            protocol_v5::RESET_CANCELLED,
            "query superseded",
        ));
        let input = v5_client_input(request_frames);
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(Cursor::new(input), output.clone()),
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();

        let frames = read_v5_frames(output.bytes());
        assert!(
            !frames.iter().any(|frame| frame.stream_id == 1
                && matches!(
                    frame.frame_type,
                    protocol_v5::FrameType::Headers
                        | protocol_v5::FrameType::Data
                        | protocol_v5::FrameType::EndStream
                )),
            "canceled search stream should not receive partial data, final headers, or END_STREAM"
        );
    }

    #[cfg(unix)]
    #[test]
    fn v5_concurrent_service_streams_process_output_before_final_response() {
        let temp = tempfile::tempdir().unwrap();
        let process = RemoteRequest::RunProcess(ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'stdout-data'; printf 'stderr-data' >&2".to_string(),
            ],
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        });
        let mut options = process.v5_request_options();
        options.priority = protocol_v5::Priority::UserInput;
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames_with_options(
            1,
            &process,
            &[],
            options,
        )));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
        input.close();
        service_thread.join().unwrap();

        let frames = read_v5_frames(output.bytes());
        let (response, _body, error) = decode_v5_service_response(&frames, 1);
        assert!(error.is_none());
        let Some(RemoteResponse::RunProcess(process_response)) = response else {
            panic!("expected streamed process response");
        };
        assert!(process_response.success);
        assert_eq!(process_response.stdout_len, "stdout-data".len());
        assert_eq!(process_response.stderr_len, "stderr-data".len());
        assert_eq!(
            v5_data_for_channel(&frames, 1, protocol_v5::DataChannel::Stdout),
            b"stdout-data"
        );
        assert_eq!(
            v5_data_for_channel(&frames, 1, protocol_v5::DataChannel::Stderr),
            b"stderr-data"
        );
        assert_v5_data_channel_priority(
            &frames,
            1,
            protocol_v5::DataChannel::Stdout,
            protocol_v5::Priority::UserInput,
        );
        assert_v5_data_channel_priority(
            &frames,
            1,
            protocol_v5::DataChannel::Stderr,
            protocol_v5::Priority::UserInput,
        );

        let final_response_index = v5_final_response_index(&frames, 1);
        let stdout_index =
            v5_first_data_channel_index(&frames, 1, protocol_v5::DataChannel::Stdout)
                .expect("expected streamed stdout DATA frame");
        let stderr_index =
            v5_first_data_channel_index(&frames, 1, protocol_v5::DataChannel::Stderr)
                .expect("expected streamed stderr DATA frame");
        assert!(
            stdout_index < final_response_index,
            "stdout DATA should be queued before final response headers"
        );
        assert!(
            stderr_index < final_response_index,
            "stderr DATA should be queued before final response headers"
        );
    }

    #[cfg(unix)]
    #[test]
    fn v5_concurrent_service_cancels_running_process_on_reset() {
        let temp = tempfile::tempdir().unwrap();
        let process = RemoteRequest::RunProcess(ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "printf 'started'; sleep 3".to_string()],
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        });
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames(1, &process, &[])));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_input = input.clone();
        let service_output = output.clone();
        let service_thread = std::thread::spawn(move || {
            service
                .serve_v5_concurrent(
                    protocol_v5::FramedIo::new(service_input, service_output),
                    &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
                )
                .unwrap();
        });

        let started = Instant::now();
        loop {
            if find_v5_output_data_for_channel(&output, 1, protocol_v5::DataChannel::Stdout)
                == b"started"
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for process stdout"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut reset = Vec::new();
        protocol_v5::write_frame(
            &mut reset,
            &protocol_v5::reset_stream_frame(1, protocol_v5::RESET_CANCELLED, "process cancelled"),
        )
        .unwrap();
        let cancelled_at = Instant::now();
        input.push(reset);
        input.close();
        service_thread.join().unwrap();

        assert!(
            cancelled_at.elapsed() < Duration::from_secs(2),
            "service waited for the sleeping process instead of cancelling it"
        );
        let frames = read_v5_frames(output.bytes());
        assert!(
            !frames.iter().any(|frame| frame.stream_id == 1
                && matches!(
                    frame.frame_type,
                    protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
                )),
            "canceled process stream should not receive final headers or END_STREAM"
        );
    }

    #[cfg(unix)]
    #[test]
    fn v5_concurrent_service_cancels_running_process_on_peer_eof() {
        let temp = tempfile::tempdir().unwrap();
        let process = RemoteRequest::RunProcess(ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "printf 'started'; sleep 3".to_string()],
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        });
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames(1, &process, &[])));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_input = input.clone();
        let service_output = output.clone();
        let service_thread = std::thread::spawn(move || {
            service
                .serve_v5_concurrent(
                    protocol_v5::FramedIo::new(service_input, service_output),
                    &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
                )
                .unwrap();
        });

        let started = Instant::now();
        loop {
            if find_v5_output_data_for_channel(&output, 1, protocol_v5::DataChannel::Stdout)
                == b"started"
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for process stdout"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let disconnected_at = Instant::now();
        input.close();
        service_thread.join().unwrap();

        assert!(
            disconnected_at.elapsed() < Duration::from_secs(2),
            "service waited for the sleeping process after peer EOF"
        );
    }

    #[cfg(unix)]
    #[test]
    fn v5_concurrent_service_expires_running_process_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let process = RemoteRequest::RunProcess(ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "printf 'started'; sleep 10".to_string()],
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        });
        let mut options = process.v5_request_options();
        options.deadline_unix_ms = v5_now_unix_millis() + 2_000;
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames_with_options(
            1,
            &process,
            &[],
            options,
        )));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_input = input.clone();
        let service_output = output.clone();
        let service_thread = std::thread::spawn(move || {
            service
                .serve_v5_concurrent(
                    protocol_v5::FramedIo::new(service_input, service_output),
                    &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
                )
                .unwrap();
        });

        let started = Instant::now();
        loop {
            if find_v5_output_data_for_channel(&output, 1, protocol_v5::DataChannel::Stdout)
                == b"started"
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for process stdout"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let deadline_wait_started = Instant::now();
        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::ResetStream);
        input.close();
        service_thread.join().unwrap();

        assert!(
            deadline_wait_started.elapsed() < Duration::from_secs(3),
            "service waited for the sleeping process instead of expiring its deadline"
        );
        let frames = read_v5_frames(output.bytes());
        let reset = frames
            .iter()
            .find(|frame| {
                frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
            })
            .expect("deadline expiry should reset the process stream")
            .decode_control::<protocol_v5::ResetStream>()
            .unwrap();
        assert_eq!(reset.code, protocol_v5::RESET_DEADLINE_EXCEEDED);
        assert!(
            !frames.iter().any(|frame| frame.stream_id == 1
                && matches!(
                    frame.frame_type,
                    protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
                )),
            "expired process stream should not receive final headers or END_STREAM"
        );
    }

    #[cfg(unix)]
    #[test]
    fn v5_concurrent_service_cancels_superseded_running_stream() {
        let temp = tempfile::tempdir().unwrap();
        let process = RemoteRequest::RunProcess(ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "printf 'started'; sleep 3".to_string()],
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        });
        let stat = RemoteRequest::Stat {
            path: PathBuf::new(),
        };
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames(1, &process, &[])));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_input = input.clone();
        let service_output = output.clone();
        let service_thread = std::thread::spawn(move || {
            service
                .serve_v5_concurrent(
                    protocol_v5::FramedIo::new(service_input, service_output),
                    &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
                )
                .unwrap();
        });

        let started = Instant::now();
        loop {
            if find_v5_output_data_for_channel(&output, 1, protocol_v5::DataChannel::Stdout)
                == b"started"
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for process stdout"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut options = stat.v5_request_options();
        options.supersedes_stream_id = 1;
        let cancelled_at = Instant::now();
        input.push(v5_frames_bytes(v5_request_frames_with_options(
            3,
            &stat,
            &[],
            options,
        )));
        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::ResetStream);
        wait_for_v5_stream_frame(&output, 3, protocol_v5::FrameType::EndStream);
        input.close();
        service_thread.join().unwrap();

        assert!(
            cancelled_at.elapsed() < Duration::from_secs(2),
            "service waited for the superseded sleeping process instead of cancelling it"
        );
        let frames = read_v5_frames(output.bytes());
        let reset = frames
            .iter()
            .find(|frame| {
                frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
            })
            .expect("supersession should reset the old stream")
            .decode_control::<protocol_v5::ResetStream>()
            .unwrap();
        assert_eq!(reset.code, protocol_v5::RESET_CANCELLED);
        let (stat_response, _, stat_error) = decode_v5_service_response(&frames, 3);
        assert!(stat_error.is_none());
        assert!(matches!(stat_response, Some(RemoteResponse::Stat(_))));
    }

    #[cfg(unix)]
    #[test]
    fn v5_cancellable_git_command_kills_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let started_file = temp.path().join("git-started");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf started > \"$STARTED_FILE\"; sleep 3"])
            .current_dir(temp.path())
            .env("STARTED_FILE", &started_file);
        let cancellation = WorkspaceCancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let root = temp.path().to_path_buf();
        let worker = std::thread::spawn(move || {
            v5_run_cancellable_command_collect(
                command,
                "git status",
                &root,
                Some(&worker_cancellation),
            )
        });

        let started = Instant::now();
        while !started_file.exists() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for fake git process to start"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let cancelled_at = Instant::now();
        cancellation.cancel();
        let error = worker.join().unwrap().unwrap_err();

        assert!(
            cancelled_at.elapsed() < Duration::from_secs(2),
            "cancellable git command waited for the child sleep instead of killing its process group"
        );
        let WorkspaceError::CommandFailed {
            operation, message, ..
        } = error
        else {
            panic!("expected command failure after cancellation");
        };
        assert_eq!(operation, "git status");
        assert_eq!(message, "git status cancelled");
    }

    #[test]
    fn v5_list_dirs_promotes_backend_cancellation_to_request_error() {
        let temp = tempfile::tempdir().unwrap();
        let request = RemoteRequest::ListDirs {
            paths: vec![PathBuf::from("first"), PathBuf::from("second")],
        };

        let backend = ConcurrentV5Backend::new();
        backend.return_list_cancelled();
        let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
        let cancellation = WorkspaceCancellationToken::new();
        let Err(error) = service.execute(request.clone(), Vec::new(), &cancellation) else {
            panic!("generic list_dirs should promote backend cancellation");
        };
        assert_eq!(error.code, protocol_v5::RESET_CANCELLED);
        assert!(!cancellation.is_cancelled());
        assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);

        let backend = ConcurrentV5Backend::new();
        backend.return_list_cancelled();
        let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
        let (method, payload) = request.to_v5_method_payload().unwrap();
        let budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
        let service_request = V5ServiceRequest {
            method: method.to_string(),
            priority: protocol_v5::Priority::VisibleFileTree,
            payload,
            body: Vec::new(),
            retained_bytes: budget.reservation(),
            received_payload_bytes: 0,
            received_body_bytes: 0,
            deadline_unix_ms: 0,
            supersedes_stream_id: 0,
            streamed_write: None,
            early_error: None,
        };
        let cancellation = WorkspaceCancellationToken::new();
        let Err(error) = service.execute_v5_list_dirs_request(&service_request, &cancellation)
        else {
            panic!("v5 list_dirs should promote backend cancellation");
        };
        assert_eq!(error.code, protocol_v5::RESET_CANCELLED);
        assert!(!cancellation.is_cancelled());
        assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);
    }

    #[test]
    fn v5_list_dirs_reset_cancels_batch_before_next_path() {
        let temp = tempfile::tempdir().unwrap();
        let request = RemoteRequest::ListDirs {
            paths: vec![PathBuf::from("first"), PathBuf::from("second")],
        };
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames(1, &request, &[])));
        let output = SharedWrite::default();
        let backend = ConcurrentV5Backend::new();
        let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        let cancellation = backend.wait_for_first_list_cancellation();

        input.push(v5_frames_bytes(vec![protocol_v5::reset_stream_frame(
            1,
            protocol_v5::RESET_CANCELLED,
            "list superseded",
        )]));
        let started = Instant::now();
        while !cancellation.is_cancelled() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "reset did not reach the filesystem cancellation token"
            );
            std::thread::yield_now();
        }
        backend.release_first_list();
        input.close();
        service_thread.join().unwrap();

        assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);
        let frames = read_v5_frames(output.bytes());
        assert!(!frames.iter().any(|frame| {
            frame.stream_id == 1
                && matches!(
                    frame.frame_type,
                    protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
                )
        }));
    }

    #[test]
    fn v5_peer_eof_cancels_blocked_filesystem_worker() {
        let temp = tempfile::tempdir().unwrap();
        let request = RemoteRequest::ListDirs {
            paths: vec![PathBuf::from("first"), PathBuf::from("second")],
        };
        let input = BlockingRead::default();
        input.push(v5_client_input(v5_request_frames(1, &request, &[])));
        let output = SharedWrite::default();
        let backend = ConcurrentV5Backend::new();
        let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        let cancellation = backend.wait_for_first_list_cancellation();

        input.close();
        let started = Instant::now();
        while !cancellation.is_cancelled() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "peer EOF did not reach the filesystem cancellation token"
            );
            std::thread::yield_now();
        }
        backend.release_first_list();
        service_thread.join().unwrap();

        assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);
    }

    #[test]
    fn v5_concurrent_service_completes_fast_stream_while_slow_stream_waits() {
        let temp = tempfile::tempdir().unwrap();
        let read = RemoteRequest::ReadFile {
            path: PathBuf::from("slow.txt"),
            max_bytes: None,
        };
        let stat = RemoteRequest::Stat {
            path: PathBuf::from("fast.txt"),
        };
        let mut request_frames = v5_request_frames(1, &read, &[]);
        request_frames.extend(v5_request_frames(3, &stat, &[]));
        let input = BlockingRead::default();
        input.push(v5_client_input(request_frames));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(ConcurrentV5Backend::new(), temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
        input.close();
        service_thread.join().unwrap();

        let frames = read_v5_frames(output.bytes());
        let (stat_response, _, stat_error) = decode_v5_service_response(&frames, 3);
        let (read_response, read_body, read_error) = decode_v5_service_response(&frames, 1);

        assert!(stat_error.is_none());
        assert!(read_error.is_none());
        assert!(matches!(stat_response, Some(RemoteResponse::Stat(_))));
        assert!(matches!(read_response, Some(RemoteResponse::ReadFile(_))));
        assert_eq!(read_body, b"slow");
        assert!(
            v5_final_response_index(&frames, 3) < v5_final_response_index(&frames, 1),
            "fast stat stream should complete before the earlier slow read stream"
        );
    }

    #[test]
    fn v5_concurrent_service_suppresses_response_after_client_reset() {
        let temp = tempfile::tempdir().unwrap();
        let read = RemoteRequest::ReadFile {
            path: PathBuf::from("slow.txt"),
            max_bytes: None,
        };
        let stat = RemoteRequest::Stat {
            path: PathBuf::from("fast.txt"),
        };
        let mut request_frames = v5_request_frames(1, &read, &[]);
        request_frames.push(protocol_v5::reset_stream_frame(
            1,
            protocol_v5::RESET_CANCELLED,
            "client superseded read",
        ));
        request_frames.extend(v5_request_frames(3, &stat, &[]));
        let input = BlockingRead::default();
        input.push(v5_client_input(request_frames));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(ConcurrentV5Backend::new(), temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );
        wait_for_v5_stream_frame(&output, 3, protocol_v5::FrameType::EndStream);
        input.close();
        service_thread.join().unwrap();

        let frames = read_v5_frames(output.bytes());
        let (stat_response, _, stat_error) = decode_v5_service_response(&frames, 3);
        assert!(stat_error.is_none());
        assert!(matches!(stat_response, Some(RemoteResponse::Stat(_))));
        assert!(
            !frames.iter().any(|frame| frame.stream_id == 1
                && matches!(
                    frame.frame_type,
                    protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
                )),
            "canceled stream should not receive final headers or END_STREAM"
        );
    }

    #[test]
    fn v5_concurrent_service_sends_idle_ping() {
        let temp = tempfile::tempdir().unwrap();
        let mut settings = protocol_v5::ConnectionSettings::recommended();
        settings.idle_ping_interval_ms = protocol_v5::MIN_UNSOLICITED_PING_INTERVAL_MS;
        settings.ping_timeout_ms = 1_000;
        let input = BlockingRead::default();
        input.push(v5_client_input_with_settings(Vec::new(), settings));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_input = input.clone();
        let service_output = output.clone();
        let service_thread = std::thread::spawn(move || {
            service
                .serve_v5_concurrent(
                    protocol_v5::FramedIo::new(service_input, service_output),
                    &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
                )
                .unwrap();
        });

        let started = Instant::now();
        let ping = loop {
            let frames = read_v5_frames(output.bytes());
            if let Some(frame) = frames
                .into_iter()
                .find(|frame| frame.frame_type == protocol_v5::FrameType::Ping)
            {
                break frame;
            }
            assert!(
                started.elapsed() < Duration::from_secs(8),
                "timed out waiting for idle PING"
            );
            std::thread::sleep(Duration::from_millis(10));
        };

        let payload = ping.decode_control::<protocol_v5::PingPayload>().unwrap();
        assert!(!payload.token.is_empty());
        input.close();
        service_thread.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn v5_concurrent_service_outbound_progress_does_not_suppress_idle_ping() {
        let temp = tempfile::tempdir().unwrap();
        let process = RemoteRequest::RunProcess(ProcessRequest {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "i=0; while [ \"$i\" -lt 400 ]; do printf x; sleep 0.02; i=$((i + 1)); done"
                    .to_string(),
            ],
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            max_output_bytes: None,
            timeout_ms: None,
        });
        let mut settings = protocol_v5::ConnectionSettings::recommended();
        settings.idle_ping_interval_ms = protocol_v5::MIN_UNSOLICITED_PING_INTERVAL_MS;
        settings.ping_timeout_ms = 1_000;
        let input = BlockingRead::default();
        input.push(v5_client_input_with_settings(
            v5_request_frames(1, &process, &[]),
            settings,
        ));
        let output = SharedWrite::default();
        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let service_thread = spawn_v5_concurrent_service(
            service,
            &input,
            &output,
            protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        );

        let _ping = wait_for_v5_connection_frame_after_with_timeout(
            &output,
            protocol_v5::FrameType::Ping,
            2,
            Duration::from_secs(8),
        );
        assert!(
            !read_v5_frames(output.bytes()).into_iter().any(|frame| {
                frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::EndStream
            }),
            "outbound progress must not postpone the heartbeat until the process completes"
        );

        input.close();
        service_thread.join().unwrap();
    }

    #[test]
    fn serve_options_parse_v5_protocol() {
        let temp = tempfile::tempdir().unwrap();

        let options = parse_serve_options([
            "--workspace".to_string(),
            temp.path().display().to_string(),
            "--protocol".to_string(),
            "v5".to_string(),
        ])
        .unwrap();

        assert_eq!(options.workspace_root, temp.path());
    }

    #[test]
    fn serve_options_reject_unknown_protocol() {
        let error = parse_serve_options(["--protocol".to_string(), "v9".to_string()])
            .expect_err("unsupported protocol should fail");

        assert!(error.to_string().contains("unsupported serve protocol"));
    }

    #[test]
    fn lsp_proxy_options_parse_workspace_server_and_args() {
        let temp = tempfile::tempdir().unwrap();

        let options = parse_lsp_proxy_options([
            "--workspace".to_string(),
            temp.path().display().to_string(),
            "--server".to_string(),
            "rust-analyzer".to_string(),
            "--".to_string(),
            "--log-file".to_string(),
            "ra.log".to_string(),
        ])
        .unwrap();

        assert_eq!(options.workspace_root, temp.path());
        assert_eq!(options.server, "rust-analyzer");
        assert_eq!(options.server_args, ["--log-file", "ra.log"]);
    }

    #[test]
    fn terminal_proxy_options_parse_shell_env_and_command() {
        let temp = tempfile::tempdir().unwrap();

        let options = parse_terminal_proxy_options([
            "--workspace".to_string(),
            temp.path().display().to_string(),
            "--shell".to_string(),
            "/bin/zsh".to_string(),
            "--env".to_string(),
            "RUST_LOG=debug".to_string(),
            "--".to_string(),
            "cargo".to_string(),
            "test".to_string(),
            "--workspace".to_string(),
        ])
        .unwrap();

        assert_eq!(options.workspace_root, temp.path());
        assert_eq!(options.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(
            options.env,
            vec![("RUST_LOG".to_string(), "debug".to_string())]
        );
        assert_eq!(
            options.command,
            Some((
                "cargo".to_string(),
                vec!["test".to_string(), "--workspace".to_string()]
            ))
        );
    }

    #[test]
    fn terminal_proxy_options_reject_invalid_env_entry() {
        let error =
            parse_terminal_proxy_options(["--env".to_string(), "BAD".to_string()]).unwrap_err();

        assert!(error.to_string().contains("KEY=VALUE"));
    }

    #[test]
    fn terminal_proxy_process_uses_environment_shell_as_login_shell_without_extra_flags() {
        let options = TerminalProxyOptions {
            workspace_root: PathBuf::from("/workspace"),
            shell: None,
            env: Vec::new(),
            command: None,
        };
        let environment = HashMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]);

        let process = terminal_proxy_process(&options, &environment);

        assert_eq!(process.program, "/bin/zsh");
        assert!(process.args.is_empty());
        assert!(process.login_shell);
    }

    #[test]
    fn terminal_proxy_process_keeps_command_sessions_non_login() {
        let options = TerminalProxyOptions {
            workspace_root: PathBuf::from("/workspace"),
            shell: None,
            env: Vec::new(),
            command: Some((
                "cargo".to_string(),
                vec!["test".to_string(), "--workspace".to_string()],
            )),
        };
        let environment = HashMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]);

        let process = terminal_proxy_process(&options, &environment);

        assert_eq!(process.program, "cargo");
        assert_eq!(process.args, ["test", "--workspace"]);
        assert!(!process.login_shell);
    }

    #[test]
    fn terminal_proxy_environment_removes_prompt_and_shell_startup_state() {
        let mut environment = HashMap::from([
            ("BASH_ENV".to_string(), "/tmp/bash-env".to_string()),
            ("BASHOPTS".to_string(), "cmdhist:progcomp".to_string()),
            ("ENV".to_string(), "/tmp/sh-env".to_string()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("POSIXLY_CORRECT".to_string(), "1".to_string()),
            ("PROMPT_COMMAND".to_string(), "echo prompt".to_string()),
            ("PS1".to_string(), "\\[broken\\]$ ".to_string()),
            ("SHELL".to_string(), "/bin/zsh".to_string()),
            ("SHELLOPTS".to_string(), "posix".to_string()),
        ]);

        remove_interactive_shell_state(&mut environment);

        for key in INTERACTIVE_SHELL_STATE_ENV_VARS {
            assert!(
                !environment.contains_key(*key),
                "{key} should not leak into remote terminal"
            );
        }
        assert_eq!(
            environment.get("SHELL").map(String::as_str),
            Some("/bin/zsh")
        );
        assert_eq!(
            environment.get("PATH").map(String::as_str),
            Some("/usr/bin:/bin")
        );
    }

    #[test]
    fn login_shell_arg0_prefixes_program_basename() {
        assert_eq!(
            login_shell_arg0(Path::new("/bin/zsh")),
            OsString::from("-zsh")
        );
        assert_eq!(login_shell_arg0(Path::new("bash")), OsString::from("-bash"));
    }

    #[test]
    fn lsp_proxy_resolves_server_from_project_environment_path() {
        let temp = tempfile::tempdir().unwrap();
        let server = temp.path().join("rust-analyzer");
        std::fs::write(&server, "").unwrap();
        let environment = HashMap::from([(
            "PATH".to_string(),
            std::env::join_paths([
                temp.path().to_path_buf(),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ])
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        )]);

        assert_eq!(
            resolve_program_from_environment_path("rust-analyzer", &environment, temp.path()),
            server
        );
        let absolute_server = temp.path().join("custom").join("rust-analyzer");
        assert_eq!(
            resolve_program_from_environment_path(
                &absolute_server.to_string_lossy(),
                &environment,
                temp.path()
            ),
            absolute_server
        );
        assert_eq!(
            resolve_program_from_environment_path(
                "./node_modules/.bin/typescript-language-server",
                &environment,
                temp.path()
            ),
            temp.path()
                .join("node_modules")
                .join(".bin")
                .join("typescript-language-server")
        );
    }

    struct CancelThenDisconnectProtocolClient {
        calls: Arc<AtomicUsize>,
    }

    impl RemoteWorkspaceProtocolClient for CancelThenDisconnectProtocolClient {
        fn request(
            &self,
            _request: RemoteRequest,
            _body: Vec<u8>,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            Err(RemoteClientError::Disconnected)
        }

        fn request_with_context_and_cancellation(
            &self,
            _request: RemoteRequest,
            _body: Vec<u8>,
            _context: RemoteRequestContext,
            cancellation: &RemoteRequestCancellation,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            cancellation.cancel();
            Err(RemoteClientError::Disconnected)
        }

        fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CancellationObservingState {
        started: bool,
        cancelled: bool,
        finished: bool,
    }

    struct CancellationObservingProtocolClient {
        state: Arc<(StdMutex<CancellationObservingState>, Condvar)>,
    }

    impl RemoteWorkspaceProtocolClient for CancellationObservingProtocolClient {
        fn request(
            &self,
            _request: RemoteRequest,
            _body: Vec<u8>,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            Err(RemoteClientError::Disconnected)
        }

        fn request_with_context_and_cancellation(
            &self,
            request: RemoteRequest,
            _body: Vec<u8>,
            _context: RemoteRequestContext,
            cancellation: &RemoteRequestCancellation,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            let (state, wake) = &*self.state;
            {
                let mut state = state.lock().unwrap();
                state.started = true;
                wake.notify_all();
            }
            let callback_state = Arc::clone(&self.state);
            cancellation.register(move || {
                let (state, wake) = &*callback_state;
                state.lock().unwrap().cancelled = true;
                wake.notify_all();
            });
            let mut state = state.lock().unwrap();
            while !state.cancelled {
                state = wake.wait(state).unwrap();
            }
            state.finished = true;
            wake.notify_all();
            Err(remote_request_cancelled_error(request.v5_method()))
        }

        fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
            Ok(())
        }
    }

    fn wait_for_cancellation_observer(
        state: &Arc<(StdMutex<CancellationObservingState>, Condvar)>,
        predicate: impl Fn(&CancellationObservingState) -> bool,
        label: &str,
    ) {
        let started = Instant::now();
        let (state, wake) = &**state;
        let mut state = state.lock().unwrap();
        while !predicate(&state) {
            let (next, _) = wake.wait_timeout(state, Duration::from_millis(20)).unwrap();
            state = next;
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for cancellation observer {label}"
            );
        }
    }

    #[derive(Clone)]
    enum FakeProtocolOutcome {
        Ok(RemoteResponse),
        Disconnected,
        Io(io::ErrorKind),
        RemoteError(&'static str),
    }

    #[derive(Clone)]
    struct FakeProtocolClient {
        calls: Arc<StdMutex<Vec<RemoteRequest>>>,
        outcomes: Arc<StdMutex<VecDeque<FakeProtocolOutcome>>>,
    }

    impl FakeProtocolClient {
        fn new(
            calls: Arc<StdMutex<Vec<RemoteRequest>>>,
            outcomes: impl IntoIterator<Item = FakeProtocolOutcome>,
        ) -> Self {
            Self {
                calls,
                outcomes: Arc::new(StdMutex::new(outcomes.into_iter().collect())),
            }
        }
    }

    impl RemoteWorkspaceProtocolClient for FakeProtocolClient {
        fn request(
            &self,
            request: RemoteRequest,
            _body: Vec<u8>,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            self.calls.lock().unwrap().push(request);
            match self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("fake protocol outcome")
            {
                FakeProtocolOutcome::Ok(response) => Ok((response, Vec::new())),
                FakeProtocolOutcome::Disconnected => Err(RemoteClientError::Disconnected),
                FakeProtocolOutcome::Io(kind) => Err(RemoteClientError::Io(io::Error::new(
                    kind,
                    "fake I/O failure",
                ))),
                FakeProtocolOutcome::RemoteError(code) => {
                    Err(RemoteClientError::Remote(RemoteError {
                        code: code.to_string(),
                        message: "remote final error".to_string(),
                        diagnostic: None,
                    }))
                }
            }
        }

        fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    enum ContextProtocolOutcome {
        Ok(RemoteResponse),
        Disconnected,
        Deadline,
    }

    struct ContextRecordingProtocolClient {
        contexts: Arc<StdMutex<Vec<RemoteRequestContext>>>,
        outcomes: StdMutex<VecDeque<ContextProtocolOutcome>>,
        closes: Arc<AtomicUsize>,
    }

    impl ContextRecordingProtocolClient {
        fn new(
            contexts: Arc<StdMutex<Vec<RemoteRequestContext>>>,
            outcomes: impl IntoIterator<Item = ContextProtocolOutcome>,
            closes: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                contexts,
                outcomes: StdMutex::new(outcomes.into_iter().collect()),
                closes,
            }
        }
    }

    impl RemoteWorkspaceProtocolClient for ContextRecordingProtocolClient {
        fn request(
            &self,
            request: RemoteRequest,
            body: Vec<u8>,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            let context = request.v5_request_context();
            self.request_with_context(request, body, context)
        }

        fn request_with_context(
            &self,
            request: RemoteRequest,
            _body: Vec<u8>,
            context: RemoteRequestContext,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            self.contexts.lock().unwrap().push(context);
            match self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("context protocol outcome")
            {
                ContextProtocolOutcome::Ok(response) => Ok((response, Vec::new())),
                ContextProtocolOutcome::Disconnected => Err(RemoteClientError::Disconnected),
                ContextProtocolOutcome::Deadline => {
                    Err(RemoteClientError::RequestDeadlineExceeded {
                        method: request.v5_method().to_string(),
                        kind: RemoteRequestDeadlineKind::Inactivity,
                    })
                }
            }
        }

        fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
            Ok(())
        }

        fn close(&self) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct LifecycleProtocolClient {
        closes: Arc<AtomicUsize>,
        shutdowns: Arc<AtomicUsize>,
    }

    impl RemoteWorkspaceProtocolClient for LifecycleProtocolClient {
        fn request(
            &self,
            _request: RemoteRequest,
            _body: Vec<u8>,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            Err(RemoteClientError::Disconnected)
        }

        fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_secs(2));
            Ok(())
        }

        fn close(&self) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct WatchProtocolClient {
        starts: Arc<AtomicUsize>,
        closes: Arc<AtomicUsize>,
        fail_start: bool,
    }

    impl RemoteWorkspaceProtocolClient for WatchProtocolClient {
        fn request(
            &self,
            _request: RemoteRequest,
            _body: Vec<u8>,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            Err(RemoteClientError::Disconnected)
        }

        fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
            Ok(())
        }

        fn close(&self) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }

        fn start_watch(
            &self,
            _request: WorkspaceWatchRequest,
        ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if self.fail_start {
                Err(RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "fake watch transport timeout",
                )))
            } else {
                Ok(None)
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum WatchControlFailure {
        Update,
        Stop,
    }

    struct WatchControlProtocolClient {
        updates: Arc<AtomicUsize>,
        stops: Arc<AtomicUsize>,
        closes: Arc<AtomicUsize>,
        failed_operation: Option<WatchControlFailure>,
    }

    impl RemoteWorkspaceProtocolClient for WatchControlProtocolClient {
        fn request(
            &self,
            _request: RemoteRequest,
            _body: Vec<u8>,
        ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
            Err(RemoteClientError::Disconnected)
        }

        fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
            Ok(())
        }

        fn close(&self) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }

        fn update_watch(
            &self,
            _watch_id: u64,
            _add_roots: Vec<PathBuf>,
            _remove_roots: Vec<PathBuf>,
        ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
            self.updates.fetch_add(1, Ordering::SeqCst);
            if self.failed_operation == Some(WatchControlFailure::Update) {
                Err(RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "fake watch.update transport timeout",
                )))
            } else {
                Ok(None)
            }
        }

        fn stop_watch(&self, _watch_id: u64) -> std::result::Result<(), RemoteClientError> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            if self.failed_operation == Some(WatchControlFailure::Stop) {
                Err(RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "fake watch.stop transport timeout",
                )))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Clone, Default)]
    struct PausingWrite {
        state: Arc<(StdMutex<PausingWriteState>, Condvar)>,
    }

    #[derive(Default)]
    struct PausingWriteState {
        bytes: Vec<u8>,
        pause_next_write: bool,
        paused: bool,
        released: bool,
    }

    impl PausingWrite {
        fn pause_next_write(&self) {
            let (lock, _) = &*self.state;
            let mut state = lock.lock().unwrap();
            state.pause_next_write = true;
            state.paused = false;
            state.released = false;
        }

        fn wait_until_paused(&self) {
            let started = Instant::now();
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            while !state.paused {
                let (next, _) = cvar.wait_timeout(state, Duration::from_millis(20)).unwrap();
                state = next;
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "timed out waiting for v5 writer pause"
                );
            }
        }

        fn release(&self) {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            state.released = true;
            cvar.notify_all();
        }

        fn bytes(&self) -> Vec<u8> {
            self.state.0.lock().unwrap().bytes.clone()
        }
    }

    impl Write for PausingWrite {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            if state.pause_next_write {
                state.pause_next_write = false;
                state.paused = true;
                cvar.notify_all();
                while !state.released {
                    state = cvar.wait(state).unwrap();
                }
            }
            state.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct ReleasingTransportAbort {
        writer: PausingWrite,
        calls: Arc<AtomicUsize>,
    }

    impl V5TransportAbort for ReleasingTransportAbort {
        fn abort(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.writer.release();
        }
    }

    struct CountingTransportAbort {
        calls: Arc<AtomicUsize>,
    }

    impl V5TransportAbort for CountingTransportAbort {
        fn abort(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Clone, Default)]
    struct SharedWrite {
        bytes: Arc<StdMutex<Vec<u8>>>,
    }

    impl SharedWrite {
        fn bytes(&self) -> Vec<u8> {
            self.bytes.lock().unwrap().clone()
        }
    }

    impl Write for SharedWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct FaultInjectingWrite {
        output: SharedWrite,
        mode: Arc<StdMutex<FaultWriteMode>>,
        successful_flushes: Arc<AtomicUsize>,
    }

    #[derive(Default)]
    enum FaultWriteMode {
        #[default]
        Healthy,
        FailAfterBytes(usize),
        FailAfterFlushes(usize),
    }

    impl FaultInjectingWrite {
        fn output(&self) -> SharedWrite {
            self.output.clone()
        }

        fn fail_after_bytes(&self, bytes: usize) {
            *self.mode.lock().unwrap() = FaultWriteMode::FailAfterBytes(bytes);
        }

        fn fail_after_flushes(&self, successful_flushes: usize) {
            *self.mode.lock().unwrap() = FaultWriteMode::FailAfterFlushes(successful_flushes);
        }

        fn successful_flush_count(&self) -> usize {
            self.successful_flushes.load(Ordering::Acquire)
        }

        fn wait_for_successful_flush_after(&self, previous: usize) {
            let started = Instant::now();
            while self.successful_flush_count() <= previous {
                assert!(
                    started.elapsed() < Duration::from_secs(2),
                    "timed out waiting for v5 writer flush"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn injected_error() -> io::Error {
            io::Error::new(io::ErrorKind::BrokenPipe, "injected v5 writer failure")
        }
    }

    impl Write for FaultInjectingWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut mode = self.mode.lock().unwrap();
            if let FaultWriteMode::FailAfterBytes(remaining) = &mut *mode {
                if *remaining == 0 {
                    return Err(Self::injected_error());
                }
                let written = buf.len().min(*remaining);
                self.output
                    .bytes
                    .lock()
                    .unwrap()
                    .extend_from_slice(&buf[..written]);
                *remaining -= written;
                return Ok(written);
            }

            self.output.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            let mut mode = self.mode.lock().unwrap();
            if let FaultWriteMode::FailAfterFlushes(remaining) = &mut *mode {
                if *remaining == 0 {
                    return Err(Self::injected_error());
                }
                *remaining -= 1;
            }
            self.successful_flushes.fetch_add(1, Ordering::Release);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct BlockingRead {
        state: Arc<(StdMutex<BlockingReadState>, Condvar)>,
    }

    struct BlockingReadState {
        bytes: VecDeque<u8>,
        closed: bool,
        next_frame_sequence: u64,
    }

    impl Default for BlockingReadState {
        fn default() -> Self {
            Self {
                bytes: VecDeque::new(),
                closed: false,
                next_frame_sequence: 1,
            }
        }
    }

    impl BlockingRead {
        fn push(&self, bytes: Vec<u8>) {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            let bytes = sequence_blocking_read_frames(bytes, &mut state.next_frame_sequence);
            state.bytes.extend(bytes);
            cvar.notify_all();
        }

        fn push_raw(&self, bytes: Vec<u8>) {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            state.bytes.extend(bytes);
            cvar.notify_all();
        }

        fn close(&self) {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            state.closed = true;
            cvar.notify_all();
        }
    }

    fn sequence_blocking_read_frames(bytes: Vec<u8>, next_sequence: &mut u64) -> Vec<u8> {
        let mut cursor = Cursor::new(bytes.as_slice());
        let mut frames = Vec::new();
        loop {
            match protocol_v5::read_frame(&mut cursor) {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => break,
                Err(_) => return bytes,
            }
        }
        if cursor.position() != bytes.len() as u64 || frames.is_empty() {
            return bytes;
        }

        let mut encoded = Vec::with_capacity(bytes.len());
        let mut candidate = *next_sequence;
        for mut frame in frames {
            frame.frame_sequence = candidate;
            candidate = candidate
                .checked_add(1)
                .expect("test v5 peer frame sequence exhausted");
            protocol_v5::write_frame(&mut encoded, &frame).unwrap();
        }
        *next_sequence = candidate;
        encoded
    }

    impl Read for BlockingRead {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            while state.bytes.is_empty() && !state.closed {
                state = cvar.wait(state).unwrap();
            }
            if state.bytes.is_empty() {
                return Ok(0);
            }
            let len = buf.len().min(state.bytes.len());
            for slot in &mut buf[..len] {
                *slot = state.bytes.pop_front().unwrap();
            }
            Ok(len)
        }
    }

    fn spawn_v5_concurrent_service<B>(
        service: WorkspaceService<B>,
        input: &BlockingRead,
        output: &SharedWrite,
        info: protocol_v5::ServerHandshakeInfo,
    ) -> std::thread::JoinHandle<()>
    where
        B: WorkspaceBackend + 'static,
    {
        let service_input = input.clone();
        let service_output = output.clone();
        std::thread::spawn(move || {
            service
                .serve_v5_concurrent(
                    protocol_v5::FramedIo::new(service_input, service_output),
                    &info,
                )
                .unwrap();
        })
    }

    fn wait_for_v5_stream_frame(
        output: &SharedWrite,
        stream_id: u64,
        expected_frame_type: protocol_v5::FrameType,
    ) {
        let started = Instant::now();
        loop {
            let bytes = output.bytes();
            let mut cursor = Cursor::new(bytes);
            while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
                if frame.stream_id == stream_id && frame.frame_type == expected_frame_type {
                    return;
                }
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for {expected_frame_type:?} on stream {stream_id}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_v5_connection_frame_after(
        output: &SharedWrite,
        expected_frame_type: protocol_v5::FrameType,
        after_sequence: u64,
    ) -> protocol_v5::Frame {
        wait_for_v5_connection_frame_after_with_timeout(
            output,
            expected_frame_type,
            after_sequence,
            Duration::from_secs(2),
        )
    }

    fn wait_for_v5_connection_frame_after_with_timeout(
        output: &SharedWrite,
        expected_frame_type: protocol_v5::FrameType,
        after_sequence: u64,
        timeout: Duration,
    ) -> protocol_v5::Frame {
        let started = Instant::now();
        loop {
            let bytes = output.bytes();
            let mut cursor = Cursor::new(bytes);
            while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
                if frame.stream_id == 0
                    && frame.frame_type == expected_frame_type
                    && frame.frame_sequence > after_sequence
                {
                    return frame;
                }
            }
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for {expected_frame_type:?} after frame {after_sequence}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn trigger_v5_client_idle_ping<W>(shared: &RemoteWorkspaceV5Shared<W>) {
        let started = Instant::now();
        loop {
            let armed = {
                let mut heartbeat = shared.heartbeat.lock().unwrap();
                if heartbeat.ping.is_none() {
                    heartbeat.last_peer_activity = Instant::now()
                        .checked_sub(heartbeat.idle_ping_interval)
                        .unwrap();
                    true
                } else {
                    false
                }
            };
            if armed {
                signal_v5_client_heartbeat(shared);
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for the previous client heartbeat to clear"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn v5_frames_bytes(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
        let mut bytes = Vec::new();
        for frame in frames {
            protocol_v5::write_frame(&mut bytes, &frame).unwrap();
        }
        bytes
    }

    fn find_v5_request_stream(output: &SharedWrite, method: &str) -> Option<u64> {
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
            if frame.frame_type != protocol_v5::FrameType::Headers {
                continue;
            }
            let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
            if envelope.role == protocol_v5::MessageRole::Request as i32
                && envelope.method == method
            {
                return Some(frame.stream_id);
            }
        }
        None
    }

    fn wait_for_v5_request_stream(output: &SharedWrite, method: &str) -> u64 {
        let started = Instant::now();
        loop {
            if let Some(stream_id) = find_v5_request_stream(output, method) {
                return stream_id;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for v5 request {method}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_v5_outbound_request_reservation_release<W>(
        shared: &RemoteWorkspaceV5Shared<W>,
        stream_id: u64,
    ) {
        let started = Instant::now();
        loop {
            let retained = shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .contains_key(&stream_id);
            if !retained {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for v5 request reservation on stream {stream_id} to release"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn find_v5_request_stream_after(
        output: &SharedWrite,
        method: &str,
        after_stream_id: u64,
    ) -> Option<u64> {
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
            if frame.stream_id <= after_stream_id
                || frame.frame_type != protocol_v5::FrameType::Headers
            {
                continue;
            }
            let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
            if envelope.role == protocol_v5::MessageRole::Request as i32
                && envelope.method == method
            {
                return Some(frame.stream_id);
            }
        }
        None
    }

    fn wait_for_v5_request_stream_after(
        output: &SharedWrite,
        method: &str,
        after_stream_id: u64,
    ) -> u64 {
        let started = Instant::now();
        loop {
            if let Some(stream_id) = find_v5_request_stream_after(output, method, after_stream_id) {
                return stream_id;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for v5 request {method} after stream {after_stream_id}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn decode_v5_request_payload<T>(output: &SharedWrite, stream_id: u64) -> Option<T>
    where
        T: DeserializeOwned,
    {
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        let mut payload = Vec::new();
        let mut content_encoding = protocol_v5::ContentEncoding::None;
        while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
            if frame.stream_id != stream_id {
                continue;
            }
            if frame.frame_type == protocol_v5::FrameType::Headers {
                let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
                content_encoding = envelope.decode_content_encoding().ok()?;
                continue;
            }
            if frame.frame_type != protocol_v5::FrameType::Data {
                continue;
            }
            let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().ok()?;
            if protocol_v5::DataChannel::try_from(envelope.channel).ok()?
                == protocol_v5::DataChannel::Unspecified
            {
                match content_encoding {
                    protocol_v5::ContentEncoding::None => payload.extend_from_slice(&frame.body),
                    protocol_v5::ContentEncoding::Zstd => {
                        let len = usize::try_from(envelope.uncompressed_len).ok()?;
                        let decoded = zstd::bulk::decompress(&frame.body, len).ok()?;
                        payload.extend_from_slice(&decoded);
                    }
                }
            }
        }
        serde_json::from_slice(&payload).ok()
    }

    #[derive(Clone)]
    struct ConcurrentV5Backend {
        state: Arc<(StdMutex<ConcurrentV5State>, Condvar)>,
    }

    #[derive(Default)]
    struct ConcurrentV5State {
        stat_seen: bool,
        list_dir_calls: Vec<PathBuf>,
        first_list_cancellation: Option<WorkspaceCancellationToken>,
        release_first_list: bool,
        return_list_cancelled: bool,
    }

    impl ConcurrentV5Backend {
        fn new() -> Self {
            Self {
                state: Arc::new((StdMutex::new(ConcurrentV5State::default()), Condvar::new())),
            }
        }

        fn unsupported<T>(
            &self,
            operation: &'static str,
            path: &Path,
        ) -> nucleotide_workspace::Result<T> {
            Err(WorkspaceError::Remote {
                operation,
                path: path.to_path_buf(),
                message: "unsupported by concurrent v5 test backend".to_string(),
                diagnostic: None,
            })
        }

        fn wait_for_first_list_cancellation(&self) -> WorkspaceCancellationToken {
            let (lock, cvar) = &*self.state;
            let state = lock.lock().unwrap();
            let (state, _) = cvar
                .wait_timeout_while(state, Duration::from_secs(2), |state| {
                    state.first_list_cancellation.is_none()
                })
                .unwrap();
            state
                .first_list_cancellation
                .clone()
                .expect("first list_dir call did not start")
        }

        fn release_first_list(&self) {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            state.release_first_list = true;
            cvar.notify_all();
        }

        fn list_dir_calls(&self) -> Vec<PathBuf> {
            self.state.0.lock().unwrap().list_dir_calls.clone()
        }

        fn return_list_cancelled(&self) {
            self.state.0.lock().unwrap().return_list_cancelled = true;
        }
    }

    #[async_trait]
    impl WorkspaceBackend for ConcurrentV5Backend {
        fn identity(&self) -> WorkspaceIdentity {
            WorkspaceIdentity::Remote(loopback_identity())
        }

        async fn stat(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            state.stat_seen = true;
            cvar.notify_all();
            Ok(FileStat {
                path: path.to_path_buf(),
                kind: FileKind::File,
                size: 4,
                modified: None,
                readonly: false,
            })
        }

        async fn list_dir(&self, path: &Path) -> nucleotide_workspace::Result<DirectoryListing> {
            self.unsupported("list directory", path)
        }

        async fn list_dir_with_cancellation(
            &self,
            path: &Path,
            cancellation: &WorkspaceCancellationToken,
        ) -> nucleotide_workspace::Result<DirectoryListing> {
            let (lock, cvar) = &*self.state;
            let mut state = lock.lock().unwrap();
            state.list_dir_calls.push(path.to_path_buf());
            if state.return_list_cancelled {
                return Err(WorkspaceError::Cancelled {
                    operation: "list directory",
                    path: path.to_path_buf(),
                });
            }
            if state.list_dir_calls.len() == 1 {
                state.first_list_cancellation = Some(cancellation.clone());
                cvar.notify_all();
                while !state.release_first_list {
                    state = cvar.wait(state).unwrap();
                }
            }
            drop(state);
            cancellation.check_cancelled("list directory", path)?;
            Ok(DirectoryListing {
                path: path.to_path_buf(),
                entries: Vec::new(),
            })
        }

        async fn find_ancestor_file(
            &self,
            start: &Path,
            _file_name: &str,
            _limit: usize,
        ) -> nucleotide_workspace::Result<Option<PathBuf>> {
            self.unsupported("find ancestor file", start)
        }

        async fn create_file(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("create file", path)
        }

        async fn create_dir(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("create directory", path)
        }

        async fn rename_path(
            &self,
            from: &Path,
            _to: &Path,
        ) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("rename path", from)
        }

        async fn delete_path(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("delete path", path)
        }

        async fn copy_path(
            &self,
            from: &Path,
            _to: &Path,
        ) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("copy path", from)
        }

        async fn read_file(
            &self,
            path: &Path,
            _options: ReadOptions,
        ) -> nucleotide_workspace::Result<FileRead> {
            let (lock, cvar) = &*self.state;
            let state = lock.lock().unwrap();
            let (state, _) = cvar
                .wait_timeout_while(state, Duration::from_secs(2), |state| !state.stat_seen)
                .unwrap();
            if !state.stat_seen {
                return Err(WorkspaceError::Remote {
                    operation: "read file",
                    path: path.to_path_buf(),
                    message: "stat did not run while read was waiting".to_string(),
                    diagnostic: None,
                });
            }
            drop(state);
            std::thread::sleep(Duration::from_millis(50));

            Ok(FileRead {
                path: path.to_path_buf(),
                bytes: b"slow".to_vec(),
                size: 4,
                modified: None,
                readonly: false,
                truncated: false,
            })
        }

        async fn write_file(
            &self,
            path: &Path,
            _bytes: &[u8],
            _options: WriteOptions,
        ) -> nucleotide_workspace::Result<WriteResult> {
            self.unsupported("write file", path)
        }

        async fn file_search(
            &self,
            query: FileSearchQuery,
        ) -> nucleotide_workspace::Result<FileSearchResult> {
            self.unsupported("file search", &query.root)
        }

        async fn text_search(
            &self,
            query: TextSearchQuery,
        ) -> nucleotide_workspace::Result<TextSearchResult> {
            self.unsupported("text search", &query.root)
        }

        async fn project_environment(
            &self,
            root: &Path,
        ) -> nucleotide_workspace::Result<ProjectEnvironmentSnapshot> {
            self.unsupported("project environment", root)
        }

        async fn git_head(&self, root: &Path) -> nucleotide_workspace::Result<GitHeadResult> {
            self.unsupported("git head", root)
        }

        async fn git_status(
            &self,
            root: &Path,
            _options: GitStatusOptions,
        ) -> nucleotide_workspace::Result<GitStatusResult> {
            self.unsupported("git status", root)
        }

        async fn run_process(
            &self,
            spec: ProcessSpec,
        ) -> nucleotide_workspace::Result<ProcessOutput> {
            self.unsupported("run process", &spec.cwd)
        }
    }

    fn loopback_identity() -> RemoteWorkspaceIdentity {
        RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Other("loopback".to_string()),
            name: "loopback".to_string(),
        }
    }

    #[test]
    fn process_output_response_defaults_missing_timeout_flag() {
        let response: ProcessOutputResponse = serde_json::from_value(serde_json::json!({
            "status_code": 0,
            "success": true,
            "stdout_truncated": false,
            "stderr_truncated": false,
            "stdout_len": 0,
            "stderr_len": 0
        }))
        .unwrap();

        assert!(!response.timed_out);
    }

    #[test]
    fn remote_time_conversion_preserves_sub_millisecond_precision() {
        let time = UNIX_EPOCH + Duration::new(42, 123_456_789);
        let millis = system_time_unix_millis(time);
        let nanos = system_time_unix_nanos(time);

        assert_eq!(
            system_time_from_unix_millis_and_nanos(millis, nanos),
            Some(time)
        );
        assert_ne!(millis.and_then(system_time_from_unix_millis), Some(time));
    }

    #[test]
    fn local_service_command_runs_helper_directly() {
        let spec = local_service_command("/tmp/nucleotide-remote", "/workspace/project");

        assert_eq!(spec.program, OsString::from("/tmp/nucleotide-remote"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("serve"),
                OsString::from("--workspace"),
                OsString::from("/workspace/project"),
                OsString::from("--protocol"),
                OsString::from("v5")
            ]
        );
        assert_eq!(spec.current_dir, Some(PathBuf::from("/workspace/project")));
        assert_arg_pair(&spec.args, "--protocol", "v5");
    }

    #[test]
    fn service_command_display_quotes_arguments_and_cwd() {
        let spec = local_service_command(
            "/tmp/nucleotide remote",
            "/workspace/project with spaces/it's",
        );
        let quoted_workspace = "'/workspace/project with spaces/it'\"'\"'s'";

        assert_eq!(
            spec.display_invocation(),
            format!("'/tmp/nucleotide remote' serve --workspace {quoted_workspace} --protocol v5")
        );
        assert_eq!(
            spec.display_context(),
            format!(
                "'/tmp/nucleotide remote' serve --workspace {quoted_workspace} --protocol v5 (cwd {quoted_workspace})"
            )
        );
    }

    #[test]
    fn wsl_service_command_uses_exec_without_shell() {
        let spec = wsl_service_command("Ubuntu", "/home/me/project", "/home/me/.cache/nucl/remote");

        assert_eq!(spec.program, OsString::from("wsl.exe"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--distribution"),
                OsString::from("Ubuntu"),
                OsString::from("--cd"),
                OsString::from("/home/me/project"),
                OsString::from("--exec"),
                OsString::from("/home/me/.cache/nucl/remote"),
                OsString::from("serve"),
                OsString::from("--workspace"),
                OsString::from("/home/me/project"),
                OsString::from("--protocol"),
                OsString::from("v5")
            ]
        );
        assert_eq!(spec.current_dir, None);
        assert_arg_pair(&spec.args, "--protocol", "v5");
    }

    #[test]
    fn wsl_shell_command_passes_deployment_script_as_one_argument() {
        let spec = wsl_shell_command(
            "Ubuntu Preview",
            "printf 'NUCL_PLATFORM '; uname -sm".to_string(),
        );

        assert_eq!(spec.program, OsString::from("wsl.exe"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--distribution"),
                OsString::from("Ubuntu Preview"),
                OsString::from("--exec"),
                OsString::from("sh"),
                OsString::from("-lc"),
                OsString::from("printf 'NUCL_PLATFORM '; uname -sm"),
            ]
        );
        assert_eq!(spec.current_dir, None);
    }

    #[test]
    fn wsl_custom_helper_path_bypasses_auto_install() {
        let helper_path = PathBuf::from("/opt/nucleotide/nucleotide-remote");
        let options = RemoteWorkspaceBackendOptions {
            wsl_helper_path: Some(helper_path.clone()),
            wsl_helper_path_is_override: true,
            ..RemoteWorkspaceBackendOptions::default()
        };
        let location = WorkspaceLocation::Wsl {
            original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
            distro: "Ubuntu".to_string(),
            linux_path: PathBuf::from("/home/me/project"),
        };

        assert_eq!(
            RemoteHelperManager::new(&options)
                .resolve_helper_for_location(&location)
                .unwrap(),
            helper_path
        );
    }

    #[test]
    fn transport_specific_helper_paths_do_not_leak_between_ssh_and_wsl() {
        let ssh_helper = PathBuf::from("/opt/ssh/nucleotide-remote");
        let wsl_helper = PathBuf::from("/opt/wsl/nucleotide-remote");
        let options = RemoteWorkspaceBackendOptions {
            ssh_helper_path: Some(ssh_helper.clone()),
            ssh_helper_path_is_override: true,
            wsl_helper_path: Some(wsl_helper.clone()),
            wsl_helper_path_is_override: true,
            ..RemoteWorkspaceBackendOptions::default()
        };
        let ssh_location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
            target: SshWorkspaceTarget {
                host: "example.com".to_string(),
                user: Some("me".to_string()),
                port: None,
            },
            path: PathBuf::from("/home/me/project"),
        };
        let wsl_location = WorkspaceLocation::Wsl {
            original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
            distro: "Ubuntu".to_string(),
            linux_path: PathBuf::from("/home/me/project"),
        };
        let manager = RemoteHelperManager::new(&options);

        assert_eq!(
            manager.resolve_helper_for_location(&ssh_location).unwrap(),
            ssh_helper
        );
        assert_eq!(
            manager.resolve_helper_for_location(&wsl_location).unwrap(),
            wsl_helper
        );
    }

    #[test]
    fn wsl_lsp_proxy_command_uses_remote_helper() {
        let spec = wsl_lsp_proxy_command(
            "Ubuntu",
            "/home/me/project",
            "/home/me/.cache/nucl/remote",
            "rust-analyzer",
        );

        assert_eq!(spec.program, OsString::from("wsl.exe"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--distribution"),
                OsString::from("Ubuntu"),
                OsString::from("--cd"),
                OsString::from("/home/me/project"),
                OsString::from("--exec"),
                OsString::from("/home/me/.cache/nucl/remote"),
                OsString::from("lsp-proxy"),
                OsString::from("--workspace"),
                OsString::from("/home/me/project"),
                OsString::from("--server"),
                OsString::from("rust-analyzer"),
                OsString::from("--"),
            ]
        );
        assert_eq!(spec.current_dir, None);
    }

    #[test]
    fn wsl_terminal_proxy_command_uses_remote_helper() {
        let command_args = vec!["test".to_string()];
        let spec = wsl_terminal_proxy_command(
            "Ubuntu",
            "/home/me/project",
            "/home/me/.cache/nucl/remote",
            Some("/bin/zsh"),
            Some(("cargo", &command_args)),
            &[("RUST_LOG".to_string(), "debug".to_string())],
        );

        assert_eq!(spec.program, OsString::from("wsl.exe"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--distribution"),
                OsString::from("Ubuntu"),
                OsString::from("--cd"),
                OsString::from("/home/me/project"),
                OsString::from("--exec"),
                OsString::from("/home/me/.cache/nucl/remote"),
                OsString::from("terminal-proxy"),
                OsString::from("--workspace"),
                OsString::from("/home/me/project"),
                OsString::from("--shell"),
                OsString::from("/bin/zsh"),
                OsString::from("--env"),
                OsString::from("RUST_LOG=debug"),
                OsString::from("--"),
                OsString::from("cargo"),
                OsString::from("test"),
            ]
        );
        assert_eq!(spec.current_dir, None);
    }

    #[test]
    fn wsl_interactive_terminal_command_uses_distro_and_directory_without_helper() {
        let spec = wsl_interactive_terminal_command("Ubuntu", "/home/me/project");

        assert_eq!(spec.program, OsString::from("wsl.exe"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--distribution"),
                OsString::from("Ubuntu"),
                OsString::from("--cd"),
                OsString::from("/home/me/project"),
            ]
        );
        assert_eq!(spec.current_dir, None);
    }

    #[test]
    fn ssh_service_command_quotes_remote_paths() {
        let mut target = SshTarget::new("devbox");
        target.user = Some("me".to_string());
        target.port = Some(2222);

        let spec = ssh_service_command(
            target,
            "/home/me/project with spaces/it's",
            "/home/me/.cache/nucleotide remote/bin",
        );

        assert_eq!(spec.program, OsString::from("ssh"));
        assert_eq!(spec.args[0], OsString::from("-T"));
        assert_ssh_non_interactive_defaults(&spec.args);
        assert_arg_pair(&spec.args, "-p", "2222");
        let separator = ssh_target_separator_index(&spec.args);
        assert_eq!(spec.args[separator + 1], OsString::from("me@devbox"));
        let command = spec.args[separator + 2].to_string_lossy();
        assert!(command.starts_with("exec "));
        assert!(command.contains("'/home/me/.cache/nucleotide remote/bin'"));
        assert!(command.contains("'/home/me/project with spaces/it'\"'\"'s'"));
        assert!(command.contains("--protocol v5"));
    }

    #[test]
    fn ssh_commands_normalize_remote_paths_to_posix() {
        let spec = ssh_service_command(
            SshTarget::new("devbox"),
            r"\home\me\project",
            r"\home\me\.cache\nucl\remote",
        );
        let separator = ssh_target_separator_index(&spec.args);
        let command = spec.args[separator + 2].to_string_lossy();

        assert!(command.contains("'/home/me/.cache/nucl/remote'"));
        assert!(command.contains("'/home/me/project'"));
        assert!(command.contains("--protocol v5"));

        let spec = ssh_terminal_proxy_command(
            SshTarget::new("devbox"),
            r"\home\me\project",
            r"\home\me\.cache\nucl\remote",
            None,
            None,
            &[],
        );
        let separator = ssh_target_separator_index(&spec.args);
        let command = spec.args[separator + 2].to_string_lossy();

        assert!(command.contains("'/home/me/.cache/nucl/remote'"));
        assert!(command.contains("'/home/me/project'"));
    }

    #[cfg(windows)]
    #[test]
    fn ssh_service_command_resolves_system_openssh_on_windows() {
        let Some(windir) = std::env::var_os("WINDIR") else {
            return;
        };
        let system_ssh = PathBuf::from(windir)
            .join("System32")
            .join("OpenSSH")
            .join("ssh.exe");
        if !system_ssh.is_file() {
            return;
        }

        let spec = ssh_service_command(
            SshTarget::new("devbox"),
            "/home/me/project",
            "/home/me/.cache/nucl/remote",
        );
        let command = spec.command();

        assert_eq!(
            command.get_program().to_string_lossy().to_ascii_lowercase(),
            system_ssh.to_string_lossy().to_ascii_lowercase()
        );
    }

    #[test]
    fn ssh_lsp_proxy_command_quotes_remote_paths_and_server() {
        let mut target = SshTarget::new("devbox");
        target.user = Some("me".to_string());
        target.port = Some(2222);

        let spec = ssh_lsp_proxy_command(
            target,
            "/home/me/project with spaces/it's",
            "/home/me/.cache/nucleotide remote/bin",
            "typescript-language-server",
        );

        assert_eq!(spec.program, OsString::from("ssh"));
        assert_eq!(spec.args[0], OsString::from("-T"));
        assert_ssh_non_interactive_defaults(&spec.args);
        assert_arg_pair(&spec.args, "-p", "2222");
        let separator = ssh_target_separator_index(&spec.args);
        assert_eq!(spec.args[separator + 1], OsString::from("me@devbox"));
        let command = spec.args[separator + 2].to_string_lossy();
        assert!(command.starts_with("exec "));
        assert!(command.contains("'/home/me/.cache/nucleotide remote/bin'"));
        assert!(command.contains(" lsp-proxy "));
        assert!(command.contains("'/home/me/project with spaces/it'\"'\"'s'"));
        assert!(command.contains("typescript-language-server"));
        assert!(command.ends_with(" --"));
    }

    #[test]
    fn ssh_interactive_terminal_command_reuses_ssh_options_and_starts_login_shell() {
        let mut target = SshTarget::new("devbox");
        target.user = Some("me".to_string());
        target.port = Some(2222);
        target.control_path = Some(PathBuf::from("/tmp/nucl-ssh/%C"));

        let spec = ssh_interactive_terminal_command(target, "/home/me/project with spaces");

        assert_eq!(spec.program, OsString::from("ssh"));
        assert_ssh_non_interactive_defaults(&spec.args);
        assert_arg_pair(&spec.args, "-p", "2222");
        assert_arg_pair(&spec.args, "-o", "ControlMaster=auto");
        let tty = arg_index(&spec.args, "-tt");
        let separator = ssh_target_separator_index(&spec.args);
        assert!(tty < separator);
        assert_eq!(spec.args[separator + 1], OsString::from("me@devbox"));
        let command = spec.args[separator + 2].to_string_lossy();
        assert!(command.starts_with("cd "));
        assert!(command.contains("'/home/me/project with spaces'"));
        assert!(command.contains("exec \"${SHELL:-/bin/sh}\" -l"));
    }

    #[test]
    fn ssh_terminal_proxy_command_quotes_remote_command_and_forces_tty() {
        let mut target = SshTarget::new("devbox");
        target.user = Some("me".to_string());
        target.port = Some(2222);
        let command_args = vec!["test".to_string(), "--workspace".to_string()];

        let spec = ssh_terminal_proxy_command(
            target,
            "/home/me/project with spaces/it's",
            "/home/me/.cache/nucleotide remote/bin",
            None,
            Some(("cargo", &command_args)),
            &[("RUST_LOG".to_string(), "debug".to_string())],
        );

        assert_eq!(spec.program, OsString::from("ssh"));
        assert_ssh_non_interactive_defaults(&spec.args);
        assert_arg_pair(&spec.args, "-p", "2222");
        let tty = arg_index(&spec.args, "-tt");
        let separator = ssh_target_separator_index(&spec.args);
        assert!(tty < separator);
        assert_eq!(spec.args[separator + 1], OsString::from("me@devbox"));
        let command = spec.args[separator + 2].to_string_lossy();
        assert!(command.starts_with("exec "));
        assert!(command.contains("'/home/me/.cache/nucleotide remote/bin'"));
        assert!(command.contains(" terminal-proxy "));
        assert!(command.contains("'/home/me/project with spaces/it'\"'\"'s'"));
        assert!(command.contains("--env 'RUST_LOG=debug'"));
        assert!(command.contains(" -- 'cargo' 'test' "));
        assert!(command.ends_with("'--workspace'"));
    }

    #[test]
    fn ssh_service_command_applies_connection_options_before_target() {
        let mut target = SshTarget::new("devbox");
        target.connect_timeout_secs = Some(12);
        target.control_path = Some(PathBuf::from("/tmp/nucl-ssh/%C"));
        target.extra_args = vec![
            OsString::from("-J"),
            OsString::from("bastion"),
            OsString::from("-F"),
            OsString::from("/tmp/ssh config"),
        ];

        let spec = ssh_service_command(target, "/home/me/project", "/remote/bin/nucleotide-remote");

        let separator = ssh_target_separator_index(&spec.args);
        assert_eq!(spec.args[0], OsString::from("-T"));
        assert_ssh_non_interactive_defaults(&spec.args);
        assert_arg_pair(&spec.args, "-o", "ConnectTimeout=12");
        assert_arg_pair(&spec.args, "-o", "ControlMaster=auto");
        assert_arg_pair(&spec.args, "-o", "ControlPersist=10m");
        assert_arg_pair(&spec.args, "-o", "ControlPath=/tmp/nucl-ssh/%C");
        assert_arg_pair(&spec.args, "-J", "bastion");
        assert_arg_pair(&spec.args, "-F", "/tmp/ssh config");
        assert!(arg_index(&spec.args, "ConnectTimeout=12") < separator);
        assert!(arg_index(&spec.args, "ControlPath=/tmp/nucl-ssh/%C") < separator);
        assert!(arg_index(&spec.args, "bastion") < separator);
        assert!(arg_index(&spec.args, "/tmp/ssh config") < separator);
        assert_eq!(spec.args[separator + 1], OsString::from("devbox"));
        assert!(
            spec.args[separator + 2]
                .to_string_lossy()
                .contains("--protocol v5")
        );
    }

    #[test]
    fn remote_workspace_identity_uses_wsl_distro() {
        let location = WorkspaceLocation::Wsl {
            original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
            distro: "Ubuntu".to_string(),
            linux_path: PathBuf::from("/home/me/project"),
        };

        let identity = remote_workspace_identity_for_location(&location).unwrap();

        assert_eq!(identity.kind, RemoteWorkspaceKind::Wsl);
        assert_eq!(identity.name, "Ubuntu");
    }

    #[test]
    fn remote_workspace_identity_formats_ssh_target() {
        let location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@example.com:2222/home/me/project"),
            target: SshWorkspaceTarget {
                host: "example.com".to_string(),
                user: Some("me".to_string()),
                port: Some(2222),
            },
            path: PathBuf::from("/home/me/project"),
        };

        let identity = remote_workspace_identity_for_location(&location).unwrap();

        assert_eq!(identity.kind, RemoteWorkspaceKind::Ssh);
        assert_eq!(identity.name, "me@example.com:2222");
    }

    #[test]
    fn remote_workspace_identity_formats_ssh_ipv6_target() {
        let location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@[2001:db8::1]:2222/home/me/project"),
            target: SshWorkspaceTarget {
                host: "2001:db8::1".to_string(),
                user: Some("me".to_string()),
                port: Some(2222),
            },
            path: PathBuf::from("/home/me/project"),
        };

        let identity = remote_workspace_identity_for_location(&location).unwrap();

        assert_eq!(identity.kind, RemoteWorkspaceKind::Ssh);
        assert_eq!(identity.name, "me@[2001:db8::1]:2222");
    }

    #[test]
    fn ssh_display_host_brackets_ipv6_hosts() {
        assert_eq!(ssh_display_host("example.com"), "example.com");
        assert_eq!(ssh_display_host("2001:db8::1"), "[2001:db8::1]");
        assert_eq!(ssh_display_host("[2001:db8::1]"), "[2001:db8::1]");
    }

    #[test]
    fn remote_service_command_for_wsl_uses_native_root() {
        let location = WorkspaceLocation::Wsl {
            original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
            distro: "Ubuntu".to_string(),
            linux_path: PathBuf::from("/home/me/project"),
        };

        let spec = remote_service_command_for_location(&location, "/remote/bin/nucleotide-remote")
            .unwrap();

        assert_eq!(spec.program, OsString::from("wsl.exe"));
        assert_eq!(spec.args[3], OsString::from("/home/me/project"));
        assert_eq!(
            spec.args[5],
            OsString::from("/remote/bin/nucleotide-remote")
        );
        assert_eq!(spec.args[8], OsString::from("/home/me/project"));
        assert_arg_pair(&spec.args, "--protocol", "v5");
    }

    #[test]
    fn remote_lsp_proxy_command_for_wsl_uses_native_root() {
        let location = WorkspaceLocation::Wsl {
            original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
            distro: "Ubuntu".to_string(),
            linux_path: PathBuf::from("/home/me/project"),
        };

        let spec = remote_lsp_proxy_command_for_location(
            &location,
            "/remote/bin/nucleotide-remote",
            "rust-analyzer",
        )
        .unwrap();

        assert_eq!(spec.program, OsString::from("wsl.exe"));
        assert_eq!(spec.args[3], OsString::from("/home/me/project"));
        assert_eq!(
            spec.args[5],
            OsString::from("/remote/bin/nucleotide-remote")
        );
        assert_eq!(spec.args[6], OsString::from("lsp-proxy"));
        assert_eq!(spec.args[8], OsString::from("/home/me/project"));
        assert_eq!(spec.args[10], OsString::from("rust-analyzer"));
    }

    #[test]
    fn remote_service_command_for_ssh_uses_target_and_native_root() {
        let location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@example.com:2222/home/me/project"),
            target: SshWorkspaceTarget {
                host: "example.com".to_string(),
                user: Some("me".to_string()),
                port: Some(2222),
            },
            path: PathBuf::from("/home/me/project"),
        };

        let spec = remote_service_command_for_location(&location, "/remote/bin/nucleotide-remote")
            .unwrap();

        assert_eq!(spec.program, OsString::from("ssh"));
        assert_eq!(spec.args[0], OsString::from("-T"));
        assert_ssh_non_interactive_defaults(&spec.args);
        assert_arg_pair(&spec.args, "-p", "2222");
        let separator = ssh_target_separator_index(&spec.args);
        assert_eq!(spec.args[separator + 1], OsString::from("me@example.com"));
        let command = spec.args[separator + 2].to_string_lossy();
        assert!(command.contains("/remote/bin/nucleotide-remote"));
        assert!(command.contains("/home/me/project"));
        assert!(command.contains("--protocol v5"));
    }

    #[test]
    fn remote_service_command_with_options_applies_ssh_settings() {
        let location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
            target: SshWorkspaceTarget {
                host: "example.com".to_string(),
                user: Some("me".to_string()),
                port: None,
            },
            path: PathBuf::from("/home/me/project"),
        };
        let options = RemoteWorkspaceBackendOptions {
            ssh_connect_timeout_secs: Some(4),
            ssh_control_path: None,
            ssh_extra_args: vec![OsString::from("-J"), OsString::from("bastion")],
            ..RemoteWorkspaceBackendOptions::default()
        };

        let spec = remote_service_command_for_location_with_options(
            &location,
            "/remote/bin/nucleotide-remote",
            &options,
        )
        .unwrap();

        let separator = ssh_target_separator_index(&spec.args);
        assert_eq!(spec.args[0], OsString::from("-T"));
        assert_ssh_non_interactive_defaults(&spec.args);
        assert_arg_pair(&spec.args, "-o", "ConnectTimeout=4");
        assert_arg_pair(&spec.args, "-J", "bastion");
        assert!(arg_index(&spec.args, "ConnectTimeout=4") < separator);
        assert!(arg_index(&spec.args, "bastion") < separator);
        assert_eq!(spec.args[separator + 1], OsString::from("me@example.com"));
        assert!(
            spec.args[separator + 2]
                .to_string_lossy()
                .contains("--protocol v5")
        );
    }

    #[test]
    fn remote_lsp_proxy_command_for_ssh_uses_target_and_native_root() {
        let location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@example.com:2222/home/me/project"),
            target: SshWorkspaceTarget {
                host: "example.com".to_string(),
                user: Some("me".to_string()),
                port: Some(2222),
            },
            path: PathBuf::from("/home/me/project"),
        };

        let spec = remote_lsp_proxy_command_for_location(
            &location,
            "/remote/bin/nucleotide-remote",
            "rust-analyzer",
        )
        .unwrap();

        assert_eq!(spec.program, OsString::from("ssh"));
        assert_eq!(spec.args[0], OsString::from("-T"));
        assert_ssh_non_interactive_defaults(&spec.args);
        assert_arg_pair(&spec.args, "-p", "2222");
        let separator = ssh_target_separator_index(&spec.args);
        assert_eq!(spec.args[separator + 1], OsString::from("me@example.com"));
        let command = spec.args[separator + 2].to_string_lossy();
        assert!(command.contains("/remote/bin/nucleotide-remote"));
        assert!(command.contains("lsp-proxy"));
        assert!(command.contains("/home/me/project"));
        assert!(command.contains("rust-analyzer"));
    }

    #[test]
    fn ssh_startup_protocol_error_allows_helper_reinstall_retry() {
        let location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
            target: SshWorkspaceTarget {
                host: "example.com".to_string(),
                user: Some("me".to_string()),
                port: None,
            },
            path: PathBuf::from("/home/me/project"),
        };
        let error = anyhow::anyhow!(
            "failed to connect to v5 remote workspace service after starting ssh helper; verify the helper speaks protocol v5"
        );

        assert!(remote_startup_error_can_retry_helper_install(
            &location, &error
        ));
    }

    #[test]
    fn startup_retry_is_limited_to_remote_linux_helper_failures() {
        let ssh_location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
            target: SshWorkspaceTarget {
                host: "example.com".to_string(),
                user: Some("me".to_string()),
                port: None,
            },
            path: PathBuf::from("/home/me/project"),
        };
        let local_location = WorkspaceLocation::Local {
            path: PathBuf::from("/home/me/project"),
        };
        let wsl_location = WorkspaceLocation::Wsl {
            original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
            distro: "Ubuntu".to_string(),
            linux_path: PathBuf::from("/home/me/project"),
        };
        let auth_error = anyhow::anyhow!("Permission denied (publickey)");
        let protocol_error = anyhow::anyhow!("invalid frame magic; expected NUC2");

        assert!(!remote_startup_error_can_retry_helper_install(
            &ssh_location,
            &auth_error
        ));
        assert!(!remote_startup_error_can_retry_helper_install(
            &local_location,
            &protocol_error
        ));
        assert!(remote_startup_error_can_retry_helper_install(
            &wsl_location,
            &protocol_error
        ));
    }

    #[test]
    fn workspace_backend_factory_keeps_local_backend_in_process_by_default() {
        let temp = tempfile::tempdir().unwrap();
        let location = WorkspaceLocation::Local {
            path: temp.path().to_path_buf(),
        };

        let connection = connect_workspace_backend_for_location(
            location,
            &RemoteWorkspaceBackendOptions::default(),
        )
        .unwrap();

        assert_eq!(connection.backend.identity(), WorkspaceIdentity::Local);
        assert_eq!(connection.hello, None);
    }

    #[test]
    fn backend_options_discover_bundled_local_helper() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("nucl");
        let helper = temp.path().join(local_helper_binary_name());
        std::fs::write(&executable, "").unwrap();
        std::fs::write(&helper, "").unwrap();

        let options = RemoteWorkspaceBackendOptions::from_environment_values(
            RemoteWorkspaceBackendEnvironment {
                use_local_service: true,
                current_exe: Some(executable),
                ssh_control_master: Some("false".to_string()),
                ..RemoteWorkspaceBackendEnvironment::default()
            },
        );

        assert_eq!(options.local_helper_path.as_deref(), Some(helper.as_path()));
        assert!(options.use_local_service);
    }

    #[test]
    fn backend_options_prefer_local_helper_env_over_bundled_helper() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("nucl");
        let bundled_helper = temp.path().join(local_helper_binary_name());
        let env_helper = temp.path().join("custom-helper");
        std::fs::write(&executable, "").unwrap();
        std::fs::write(&bundled_helper, "").unwrap();

        let options = RemoteWorkspaceBackendOptions::from_environment_values(
            RemoteWorkspaceBackendEnvironment {
                local_helper_path: Some(env_helper.clone().into_os_string()),
                use_local_service: true,
                current_exe: Some(executable),
                ssh_control_master: Some("false".to_string()),
                ..RemoteWorkspaceBackendEnvironment::default()
            },
        );

        assert_eq!(
            options.local_helper_path.as_deref(),
            Some(env_helper.as_path())
        );
    }

    #[test]
    fn backend_options_discover_ssh_helper_upload_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("nucl");
        let upload_helper = temp.path().join("nucleotide-remote-linux-x86_64");
        std::fs::write(&executable, "").unwrap();

        let options = RemoteWorkspaceBackendOptions::from_environment_values(
            RemoteWorkspaceBackendEnvironment {
                ssh_helper_upload_path: Some(upload_helper.clone().into_os_string()),
                ssh_helper_install_policy: Some("upload".to_string()),
                current_exe: Some(executable),
                ssh_control_master: Some("false".to_string()),
                ..RemoteWorkspaceBackendEnvironment::default()
            },
        );

        assert_eq!(
            options.ssh_helper_upload_path.as_deref(),
            Some(upload_helper.as_path())
        );
        assert_eq!(
            options.ssh_helper_install_policy,
            RemoteHelperInstallPolicy::Upload
        );
        assert_eq!(
            options.wsl_helper_install_policy,
            RemoteHelperInstallPolicy::Upload
        );
    }

    #[test]
    fn generic_remote_helper_environment_overrides_ssh_and_wsl() {
        let helper = PathBuf::from("/opt/nucleotide/nucleotide-remote");
        let options = RemoteWorkspaceBackendOptions::from_environment_values(
            RemoteWorkspaceBackendEnvironment {
                remote_helper_path: Some(helper.clone().into_os_string()),
                ssh_helper_install_policy: Some("never".to_string()),
                ssh_control_master: Some("false".to_string()),
                ..RemoteWorkspaceBackendEnvironment::default()
            },
        );

        assert_eq!(options.remote_helper_path, helper);
        assert_eq!(options.ssh_helper_path.as_deref(), Some(helper.as_path()));
        assert_eq!(options.wsl_helper_path.as_deref(), Some(helper.as_path()));
        assert!(options.remote_helper_path_is_override);
        assert!(options.ssh_helper_path_is_override);
        assert!(options.wsl_helper_path_is_override);
        assert_eq!(
            options.ssh_helper_install_policy,
            RemoteHelperInstallPolicy::Never
        );
        assert_eq!(
            options.wsl_helper_install_policy,
            RemoteHelperInstallPolicy::Never
        );
    }

    #[test]
    fn backend_options_parse_ssh_connection_environment_values() {
        let control_path = PathBuf::from("/tmp/nucl-control/%C");

        let options = RemoteWorkspaceBackendOptions::from_environment_values(
            RemoteWorkspaceBackendEnvironment {
                ssh_connect_timeout_secs: Some("9".to_string()),
                ssh_extra_args: Some(OsString::from("-J bastion -F '/tmp/ssh config'")),
                ssh_control_master: Some("true".to_string()),
                ssh_control_path: Some(control_path.clone().into_os_string()),
                ssh_helper_download_base_url: Some(
                    "https://mirror.example/releases/v1".to_string(),
                ),
                ..RemoteWorkspaceBackendEnvironment::default()
            },
        );

        assert_eq!(options.ssh_connect_timeout_secs, Some(9));
        assert_eq!(
            options.ssh_extra_args,
            [
                OsString::from("-J"),
                OsString::from("bastion"),
                OsString::from("-F"),
                OsString::from("/tmp/ssh config"),
            ]
        );
        assert_eq!(
            options.ssh_control_path.as_deref(),
            Some(control_path.as_path())
        );
        assert_eq!(
            options.ssh_helper_download_base_url.as_deref(),
            Some("https://mirror.example/releases/v1")
        );
    }

    #[test]
    fn default_ssh_control_path_leaves_room_for_openssh_suffix() {
        let Some(control_path) = default_ssh_control_path() else {
            return;
        };

        let expanded_hash = "0123456789abcdef0123456789abcdef01234567";
        let openssh_bind_suffix = ".abcdefghijklmnop";
        let expanded_path = control_path
            .display()
            .to_string()
            .replace("%C", expanded_hash);
        let bind_path = format!("{expanded_path}{openssh_bind_suffix}");

        assert!(
            bind_path.len() < 104,
            "OpenSSH ControlPath is too long for macOS Unix sockets: {bind_path}"
        );
    }

    #[test]
    fn backend_options_discover_platform_named_ssh_helper_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("nucl");
        let artifact = temp.path().join("nucleotide-remote-linux-x86_64");
        std::fs::write(&executable, "").unwrap();
        std::fs::write(&artifact, "").unwrap();

        let options = RemoteWorkspaceBackendOptions::from_environment_values(
            RemoteWorkspaceBackendEnvironment {
                current_exe: Some(executable),
                ssh_control_master: Some("false".to_string()),
                ..RemoteWorkspaceBackendEnvironment::default()
            },
        );
        let platform = SshRemotePlatform {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        };

        assert_eq!(
            RemoteHelperManager::new(&options).local_upload_artifact_for_platform(&platform),
            Some(artifact)
        );
    }

    #[test]
    fn helper_version_command_writes_json_probe_payload() {
        let mut output = Vec::new();

        print_version(["--json".to_string()], &mut output).unwrap();

        let info: HelperVersionInfo = serde_json::from_slice(&output).unwrap();
        assert_eq!(info.helper_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.protocol_version, PROTOCOL_VERSION);
        assert_eq!(info.frame_version, FRAME_VERSION);
        assert_eq!(info.os, std::env::consts::OS);
        assert_eq!(info.arch, std::env::consts::ARCH);
    }

    #[test]
    fn linux_probe_parser_accepts_shell_noise_and_platform_markers() {
        let probe = parse_linux_probe_output(
            "profile says hi\nNUCL_PLATFORM Linux aarch64\nNUCL_CACHE /home/me/.cache\n",
        )
        .unwrap();

        assert_eq!(
            probe.platform,
            SshRemotePlatform {
                os: "linux".to_string(),
                arch: "aarch64".to_string(),
            }
        );
        assert_eq!(probe.cache_root, "/home/me/.cache");
    }

    #[test]
    fn linux_helper_cache_path_includes_protocol_version_and_platform() {
        let probe = SshRemoteProbe {
            platform: SshRemotePlatform {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
            cache_root: "/home/me/.cache".to_string(),
        };

        assert_eq!(
            remote_linux_helper_path(&probe),
            PathBuf::from(format!(
                "/home/me/.cache/nucleotide/remote/protocol-{PROTOCOL_VERSION}/nucleotide-remote-{}-linux-x86_64",
                env!("CARGO_PKG_VERSION")
            ))
        );
    }

    #[test]
    fn helper_version_match_checks_protocol_version_and_platform() {
        let platform = SshRemotePlatform {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        };
        let mut info = HelperVersionInfo {
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            frame_version: FRAME_VERSION,
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        };

        assert!(helper_version_matches_current(&info, &platform));

        info.protocol_version += 1;
        assert!(!helper_version_matches_current(&info, &platform));
    }

    #[test]
    fn remote_deployment_progress_formats_status_message() {
        let progress = RemoteDeploymentProgress {
            phase: RemoteDeploymentPhase::InstallingRemoteHelper,
            target: Some("me@example.com".to_string()),
            detail: Some("download nucleotide-remote-linux-x86_64".to_string()),
        };

        assert_eq!(
            progress.message(),
            "Installing nucleotide-remote: me@example.com (download nucleotide-remote-linux-x86_64)"
        );
    }

    #[test]
    fn wsl_deployment_progress_names_distribution_startup() {
        let progress = RemoteDeploymentProgress {
            phase: RemoteDeploymentPhase::StartingWslDistro,
            target: Some("Ubuntu".to_string()),
            detail: None,
        };

        assert_eq!(progress.message(), "Starting WSL distribution: Ubuntu");
    }

    #[test]
    fn remote_helper_download_urls_use_release_assets_and_checksums() {
        let options = RemoteWorkspaceBackendOptions {
            ssh_helper_download_base_url: Some(
                "https://downloads.example/nucleotide/v1/".to_string(),
            ),
            ..RemoteWorkspaceBackendOptions::default()
        };
        let manager = RemoteHelperManager::new(&options);
        let platform = SshRemotePlatform {
            os: "linux".to_string(),
            arch: "aarch64".to_string(),
        };

        let (asset_url, checksums_url) = manager.remote_helper_download_urls(&platform).unwrap();

        assert_eq!(
            asset_url,
            "https://downloads.example/nucleotide/v1/nucleotide-remote-linux-aarch64"
        );
        assert_eq!(
            checksums_url,
            "https://downloads.example/nucleotide/v1/SHA256SUMS"
        );
    }

    #[test]
    fn remote_helper_upload_command_registers_temporary_file_cleanup() {
        let expected_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let command = remote_helper_upload_command(
            "/home/me/.cache/nucleotide",
            "/home/me/.cache/nucleotide/helper tmp",
            "/home/me/.cache/nucleotide/helper",
            expected_sha256,
        );

        assert!(command.starts_with("sh -lc "));
        assert!(command.contains("sha256sum"));
        assert!(command.contains("shasum -a 256"));
        assert!(command.contains("checksum mismatch for uploaded helper"));
        assert!(command.contains("cleanup() { rm -f \"$tmp\"; }"));
        assert!(command.contains("trap cleanup EXIT"));
        assert!(command.contains("trap \"exit 1\" INT TERM HUP"));
        assert!(command.contains("cat > \"$tmp\""));
        assert!(command.contains("mv -f \"$tmp\" \"$final\""));
        assert!(command.contains("'/home/me/.cache/nucleotide/helper tmp'"));
        assert!(command.contains(expected_sha256));
    }

    #[cfg(unix)]
    #[test]
    fn remote_helper_upload_command_rejects_truncated_input_without_replacing_helper() {
        let temp = tempfile::tempdir().unwrap();
        let helper_dir = temp.path().join("helper dir");
        let tmp_path = helper_dir.join("helper tmp");
        let helper_path = helper_dir.join("helper");
        let input_path = temp.path().join("input helper");
        let complete_helper = b"complete helper bytes that must arrive intact";
        let expected_sha256 = sha256_reader(&mut complete_helper.as_slice()).unwrap();
        std::fs::create_dir_all(&helper_dir).unwrap();
        std::fs::write(&helper_path, b"existing working helper").unwrap();
        std::fs::write(&input_path, b"partial helper bytes").unwrap();
        let command = remote_helper_upload_command(
            helper_dir.to_str().unwrap(),
            tmp_path.to_str().unwrap(),
            helper_path.to_str().unwrap(),
            &expected_sha256,
        );

        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &command])
            .stdin(Stdio::from(std::fs::File::open(input_path).unwrap()))
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("checksum mismatch"));
        assert!(!tmp_path.exists(), "failed upload left its temporary file");
        assert_eq!(
            std::fs::read(&helper_path).unwrap(),
            b"existing working helper"
        );
    }

    #[cfg(unix)]
    #[test]
    fn remote_helper_upload_command_installs_complete_verified_input() {
        let temp = tempfile::tempdir().unwrap();
        let helper_dir = temp.path().join("helper dir");
        let tmp_path = helper_dir.join("helper tmp");
        let helper_path = helper_dir.join("helper");
        let input_path = temp.path().join("input helper");
        let helper = b"complete verified helper bytes";
        let expected_sha256 = sha256_reader(&mut helper.as_slice()).unwrap();
        std::fs::write(&input_path, helper).unwrap();
        let command = remote_helper_upload_command(
            helper_dir.to_str().unwrap(),
            tmp_path.to_str().unwrap(),
            helper_path.to_str().unwrap(),
            &expected_sha256,
        );

        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &command])
            .stdin(Stdio::from(std::fs::File::open(input_path).unwrap()))
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "verified upload failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!tmp_path.exists());
        assert_eq!(std::fs::read(&helper_path).unwrap(), helper);
    }

    #[test]
    fn remote_helper_download_command_verifies_checksum_before_install() {
        let command = remote_helper_download_command(
            "/home/me/.cache/nucleotide/remote",
            "/home/me/.cache/nucleotide/remote/helper tmp",
            "/home/me/.cache/nucleotide/remote/helper",
            "https://downloads.example/nucleotide-remote-linux-x86_64",
            "https://downloads.example/SHA256SUMS",
            "nucleotide-remote-linux-x86_64",
        );

        assert!(command.starts_with("sh -lc "));
        assert!(command.contains("curl -fsSL"));
        assert!(command.contains("wget -qO"));
        assert!(command.contains("sha256sum"));
        assert!(command.contains("shasum -a 256"));
        assert!(command.contains("checksum mismatch"));
        assert!(command.contains("trap cleanup EXIT"));
        assert!(command.contains("trap \"exit 1\" INT TERM HUP"));
        assert!(command.contains("mv -f"));
        assert!(command.contains("'/home/me/.cache/nucleotide/remote/helper tmp'"));
        assert!(command.contains("nucleotide-remote-linux-x86_64"));
        assert!(command.contains("SHA256SUMS"));
    }

    #[test]
    fn remote_helper_hints_name_transport_and_env_var() {
        let wsl_location = WorkspaceLocation::Wsl {
            original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
            distro: "Ubuntu".to_string(),
            linux_path: PathBuf::from("/home/me/project"),
        };
        let ssh_location = WorkspaceLocation::Ssh {
            original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
            target: SshWorkspaceTarget {
                host: "example.com".to_string(),
                user: Some("me".to_string()),
                port: None,
            },
            path: PathBuf::from("/home/me/project"),
        };

        let wsl_hint = remote_helper_setup_hint(&wsl_location, Path::new("/remote/nucl"));
        let ssh_hint = remote_helper_setup_hint(&ssh_location, Path::new("/remote/nucl"));

        assert!(wsl_hint.contains("WSL distro Ubuntu"));
        assert!(wsl_hint.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(ssh_hint.contains("SSH target me@example.com"));
        assert!(ssh_hint.contains("NUCLEOTIDE_REMOTE_HELPER"));
    }
}
