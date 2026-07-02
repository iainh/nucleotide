// ABOUTME: Framed stdio protocol and service loop for Nucleotide remote workspaces
// ABOUTME: Keeps WSL, SSH, and local service transports on one request model

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::executor::block_on;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use nucleotide_env::{EnvironmentOrigin, ProjectEnvironment, ShellEnvironmentError};
use nucleotide_workspace::{
    DirectoryListing, FileKind, FileRead, FileSearchQuery, FileSearchResult, FileStat,
    GitHeadResult, GitStatusEntry, GitStatusKind, GitStatusOptions, GitStatusResult,
    LocalWorkspaceBackend, ProcessOutput, ProcessSpec, ProjectEnvironmentOrigin,
    ProjectEnvironmentSnapshot, ReadOptions, RemoteWorkspaceIdentity, RemoteWorkspaceKind,
    SshWorkspaceTarget, TextSearchMatch, TextSearchQuery, TextSearchResult, WorkspaceBackend,
    WorkspaceBackendHandle, WorkspaceError, WorkspaceIdentity, WorkspaceLocation, WriteOptions,
    WriteResult, local_workspace_backend, path_mapped_workspace_backend,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

pub const PROTOCOL_VERSION: u32 = 2;
pub const FRAME_VERSION: u16 = 1;
pub const FRAME_MAGIC: [u8; 4] = *b"NUCL";
pub const FRAME_HEADER_LEN: usize = 36;
pub const MAX_FRAME_HEADER_LEN: u32 = 1024 * 1024;
pub const MAX_FRAME_BODY_LEN: u64 = 128 * 1024 * 1024;
pub const DEFAULT_SSH_CONNECT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_SSH_CONTROL_PERSIST: &str = "10m";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FrameKind {
    Request = 1,
    Response = 2,
    Error = 3,
    Data = 4,
    Progress = 5,
    Cancel = 6,
    Shutdown = 7,
}

impl TryFrom<u16> for FrameKind {
    type Error = io::Error;

    fn try_from(value: u16) -> std::result::Result<Self, <Self as TryFrom<u16>>::Error> {
        match value {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            3 => Ok(Self::Error),
            4 => Ok(Self::Data),
            5 => Ok(Self::Progress),
            6 => Ok(Self::Cancel),
            7 => Ok(Self::Shutdown),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown frame kind: {value}"),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub flags: u16,
    pub request_id: u64,
    pub stream_id: u32,
    pub header: Vec<u8>,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn from_json_header<T: Serialize>(
        kind: FrameKind,
        request_id: u64,
        stream_id: u32,
        header: &T,
        body: Vec<u8>,
    ) -> serde_json::Result<Self> {
        Ok(Self {
            kind,
            flags: 0,
            request_id,
            stream_id,
            header: serde_json::to_vec(header)?,
            body,
        })
    }

    pub fn decode_json_header<T: DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.header)
    }
}

pub fn write_frame<W: Write>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    if frame.header.len() > MAX_FRAME_HEADER_LEN as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame header is too large",
        ));
    }
    if frame.body.len() as u64 > MAX_FRAME_BODY_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame body is too large",
        ));
    }

    let mut header = [0_u8; FRAME_HEADER_LEN];
    header[0..4].copy_from_slice(&FRAME_MAGIC);
    header[4..6].copy_from_slice(&FRAME_VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&(frame.kind as u16).to_le_bytes());
    header[8..10].copy_from_slice(&frame.flags.to_le_bytes());
    header[10..12].copy_from_slice(&0_u16.to_le_bytes());
    header[12..20].copy_from_slice(&frame.request_id.to_le_bytes());
    header[20..24].copy_from_slice(&frame.stream_id.to_le_bytes());
    header[24..28].copy_from_slice(&(frame.header.len() as u32).to_le_bytes());
    header[28..36].copy_from_slice(&(frame.body.len() as u64).to_le_bytes());

    writer.write_all(&header)?;
    writer.write_all(&frame.header)?;
    writer.write_all(&frame.body)?;
    writer.flush()
}

pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Frame>> {
    let mut fixed = [0_u8; FRAME_HEADER_LEN];
    match reader.read(&mut fixed[..1])? {
        0 => return Ok(None),
        1 => reader.read_exact(&mut fixed[1..])?,
        _ => unreachable!("read buffer length is one byte"),
    }

    if fixed[0..4] != FRAME_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame magic",
        ));
    }

    let version = u16::from_le_bytes([fixed[4], fixed[5]]);
    if version != FRAME_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported frame version: {version}"),
        ));
    }

    let kind = FrameKind::try_from(u16::from_le_bytes([fixed[6], fixed[7]]))?;
    let flags = u16::from_le_bytes([fixed[8], fixed[9]]);
    let request_id = u64::from_le_bytes([
        fixed[12], fixed[13], fixed[14], fixed[15], fixed[16], fixed[17], fixed[18], fixed[19],
    ]);
    let stream_id = u32::from_le_bytes([fixed[20], fixed[21], fixed[22], fixed[23]]);
    let header_len = u32::from_le_bytes([fixed[24], fixed[25], fixed[26], fixed[27]]);
    let body_len = u64::from_le_bytes([
        fixed[28], fixed[29], fixed[30], fixed[31], fixed[32], fixed[33], fixed[34], fixed[35],
    ]);

    if header_len > MAX_FRAME_HEADER_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame header exceeds maximum length",
        ));
    }
    if body_len > MAX_FRAME_BODY_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame body exceeds maximum length",
        ));
    }

    let mut header = vec![0_u8; header_len as usize];
    reader.read_exact(&mut header)?;
    let mut body = vec![0_u8; body_len as usize];
    reader.read_exact(&mut body)?;

    Ok(Some(Frame {
        kind,
        flags,
        request_id,
        stream_id,
        header,
        body,
    }))
}

pub trait RemoteTransport: Send {
    fn write_frame(&mut self, frame: &Frame) -> io::Result<()>;

    fn read_frame(&mut self) -> io::Result<Option<Frame>>;
}

pub struct FramedTransport<R, W> {
    reader: R,
    writer: W,
}

impl<R, W> FramedTransport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
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
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        command
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

    pub fn spawn(&self) -> io::Result<ChildProcessTransport> {
        ChildProcessTransport::spawn(self)
    }
}

pub fn local_service_command(
    helper_path: impl AsRef<Path>,
    workspace_root: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let helper_path = helper_path.as_ref();
    let workspace_root = workspace_root.as_ref();
    RemoteServiceCommand {
        program: helper_path.as_os_str().to_os_string(),
        args: vec![
            OsString::from("serve"),
            OsString::from("--workspace"),
            workspace_root.as_os_str().to_os_string(),
        ],
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
    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args: vec![
            OsString::from("--distribution"),
            distro.as_ref().to_os_string(),
            OsString::from("--cd"),
            linux_root.as_os_str().to_os_string(),
            OsString::from("--exec"),
            helper_path.as_os_str().to_os_string(),
            OsString::from("serve"),
            OsString::from("--workspace"),
            linux_root.as_os_str().to_os_string(),
        ],
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

pub fn ssh_service_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let remote_root = remote_root.as_ref().to_string_lossy();
    let helper_path = helper_path.as_ref().to_string_lossy();
    let remote_command = format!(
        "exec {} serve --workspace {}",
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
    let remote_root = remote_root.as_ref().to_string_lossy();
    let helper_path = helper_path.as_ref().to_string_lossy();
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
    if let Some(timeout_secs) = target.connect_timeout_secs {
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!("ConnectTimeout={timeout_secs}")));
    }

    if let Some(control_path) = target.control_path.as_ref() {
        if let Some(parent) = control_path.parent() {
            let _ = std::fs::create_dir_all(parent);
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
    let mut parts = vec![
        "exec".to_string(),
        quote_posix_shell(&helper_path.to_string_lossy()),
        "terminal-proxy".to_string(),
        "--workspace".to_string(),
        quote_posix_shell(&remote_root.to_string_lossy()),
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

pub struct ChildProcessTransport {
    child: Child,
    reader: ChildStdout,
    writer: ChildStdin,
}

impl ChildProcessTransport {
    pub fn spawn(spec: &RemoteServiceCommand) -> io::Result<Self> {
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

        Ok(Self {
            child,
            reader,
            writer,
        })
    }

    pub fn child_id(&self) -> u32 {
        self.child.id()
    }
}

impl RemoteTransport for ChildProcessTransport {
    fn write_frame(&mut self, frame: &Frame) -> io::Result<()> {
        write_frame(&mut self.writer, frame)
    }

    fn read_frame(&mut self) -> io::Result<Option<Frame>> {
        read_frame(&mut self.reader)
    }
}

impl Drop for ChildProcessTransport {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl<R, W> RemoteTransport for FramedTransport<R, W>
where
    R: Read + Send,
    W: Write + Send,
{
    fn write_frame(&mut self, frame: &Frame) -> io::Result<()> {
        write_frame(&mut self.writer, frame)
    }

    fn read_frame(&mut self) -> io::Result<Option<Frame>> {
        read_frame(&mut self.reader)
    }
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
pub struct RequestEnvelope {
    pub protocol_version: u32,
    pub request: RemoteRequest,
}

impl RequestEnvelope {
    pub fn new(request: RemoteRequest) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum RemoteRequest {
    Hello,
    Stat {
        path: PathBuf,
    },
    ListDir {
        path: PathBuf,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub protocol_version: u32,
    pub response: RemoteResponse,
}

impl ResponseEnvelope {
    pub fn new(response: RemoteResponse) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            response,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "snake_case")]
pub enum RemoteResponse {
    Hello(HelloResponse),
    Stat(FileStatResponse),
    ListDir(DirectoryListingResponse),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub protocol_version: u32,
    pub error: RemoteError,
}

impl ErrorEnvelope {
    pub fn new(error: RemoteError) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            error,
        }
    }
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

impl HelloResponse {
    fn current(workspace_root: PathBuf) -> Self {
        Self {
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            workspace_root,
            capabilities: vec![
                "stat".to_string(),
                "list_dir".to_string(),
                "find_ancestor_file".to_string(),
                "create_file".to_string(),
                "create_dir".to_string(),
                "rename_path".to_string(),
                "delete_path".to_string(),
                "copy_path".to_string(),
                "read_file".to_string(),
                "write_file".to_string(),
                "file_search".to_string(),
                "text_search".to_string(),
                "project_environment".to_string(),
                "project_environment_process_spawn".to_string(),
                "git_head".to_string(),
                "git_status".to_string(),
                "run_process".to_string(),
                "binary_body_frames".to_string(),
                "directory_entry_ignored".to_string(),
            ],
        }
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
    pub local_helper_path: Option<PathBuf>,
    pub ssh_helper_upload_path: Option<PathBuf>,
    pub ssh_helper_artifact_dir: Option<PathBuf>,
    pub ssh_helper_download_base_url: Option<String>,
    pub ssh_helper_install_policy: RemoteHelperInstallPolicy,
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
            local_helper_path: None,
            ssh_helper_upload_path: None,
            ssh_helper_artifact_dir: None,
            ssh_helper_download_base_url: None,
            ssh_helper_install_policy: RemoteHelperInstallPolicy::Auto,
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

        if let Some(policy) = env_ssh_helper_install_policy {
            options.ssh_helper_install_policy =
                RemoteHelperInstallPolicy::from_env_value(Some(policy));
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
            options.remote_helper_path = PathBuf::from(path);
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
            WorkspaceLocation::Local { .. } | WorkspaceLocation::Wsl { .. } => {
                Ok(self.options.remote_helper_path.clone())
            }
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
            WorkspaceLocation::Local { .. } | WorkspaceLocation::Wsl { .. } => Ok(None),
        }
    }

    fn resolve_ssh_helper(&self, target: &SshTarget) -> Result<PathBuf> {
        if self.options.remote_helper_path_is_override
            && self.options.ssh_helper_install_policy != RemoteHelperInstallPolicy::Upload
            && self.options.ssh_helper_install_policy != RemoteHelperInstallPolicy::RemoteDownload
        {
            return Ok(self.options.remote_helper_path.clone());
        }

        if self.options.ssh_helper_install_policy == RemoteHelperInstallPolicy::Never {
            return Ok(self.options.remote_helper_path.clone());
        }

        self.emit_progress(
            RemoteDeploymentPhase::ConnectingSshHost,
            Some(target.target_arg()),
            None,
        );
        let probe = match self.probe_ssh_platform(target) {
            Ok(probe) => probe,
            Err(_) if self.options.ssh_helper_install_policy == RemoteHelperInstallPolicy::Auto => {
                return Ok(self.options.remote_helper_path.clone());
            }
            Err(error) => return Err(error),
        };

        let helper_path = if self.options.remote_helper_path_is_override {
            self.options.remote_helper_path.clone()
        } else {
            ssh_remote_helper_path(&probe)
        };

        self.emit_progress(
            RemoteDeploymentPhase::CheckingRemoteHelper,
            Some(target.target_arg()),
            Some(helper_path.display().to_string()),
        );
        if self.remote_helper_matches(target, &helper_path, &probe.platform) {
            return Ok(helper_path);
        }

        if self.options.ssh_helper_install_policy == RemoteHelperInstallPolicy::RemoteDownload {
            self.install_ssh_helper_by_remote_download(target, &probe.platform, &helper_path)?;
            if !self.remote_helper_matches(target, &helper_path, &probe.platform) {
                bail!(
                    "downloaded nucleotide-remote on SSH target {} but version probe did not match protocol {}",
                    target.target_arg(),
                    PROTOCOL_VERSION
                );
            }
            return Ok(helper_path);
        }

        let Some(local_helper) = self.local_upload_artifact_for_platform(&probe.platform) else {
            if self.options.ssh_helper_install_policy == RemoteHelperInstallPolicy::Auto
                && self
                    .install_ssh_helper_by_remote_download(target, &probe.platform, &helper_path)
                    .is_ok()
            {
                if !self.remote_helper_matches(target, &helper_path, &probe.platform) {
                    bail!(
                        "downloaded nucleotide-remote on SSH target {} but version probe did not match protocol {}",
                        target.target_arg(),
                        PROTOCOL_VERSION
                    );
                }
                return Ok(helper_path);
            }

            if self.options.ssh_helper_install_policy == RemoteHelperInstallPolicy::Upload {
                bail!(
                    "SSH helper upload requested, but no local nucleotide-remote artifact is configured"
                );
            }
            return Ok(self.options.remote_helper_path.clone());
        };

        if !local_helper.is_file() {
            if self.options.ssh_helper_install_policy == RemoteHelperInstallPolicy::Upload {
                bail!(
                    "SSH helper upload requested, but local artifact does not exist: {}",
                    local_helper.display()
                );
            }
            return Ok(self.options.remote_helper_path.clone());
        }

        self.emit_progress(
            RemoteDeploymentPhase::InstallingRemoteHelper,
            Some(target.target_arg()),
            Some(format!("upload {}", local_helper.display())),
        );
        self.upload_ssh_helper(target, &local_helper, &helper_path)
            .with_context(|| {
                format!(
                    "failed to upload nucleotide-remote to SSH target {}",
                    target.target_arg()
                )
            })?;

        if !self.remote_helper_matches(target, &helper_path, &probe.platform) {
            bail!(
                "uploaded nucleotide-remote on SSH target {} but version probe did not match protocol {}",
                target.target_arg(),
                PROTOCOL_VERSION
            );
        }

        Ok(helper_path)
    }

    fn reinstall_ssh_helper(&self, target: &SshTarget) -> Result<PathBuf> {
        if self.options.remote_helper_path_is_override
            && self.options.ssh_helper_install_policy != RemoteHelperInstallPolicy::Upload
            && self.options.ssh_helper_install_policy != RemoteHelperInstallPolicy::RemoteDownload
        {
            bail!("NUCLEOTIDE_REMOTE_HELPER is set; automatic SSH helper reinstall is disabled");
        }

        if self.options.ssh_helper_install_policy == RemoteHelperInstallPolicy::Never {
            bail!("SSH helper auto-install is disabled");
        }

        let probe = self.probe_ssh_platform(target)?;
        let helper_path = if self.options.remote_helper_path_is_override {
            self.options.remote_helper_path.clone()
        } else {
            ssh_remote_helper_path(&probe)
        };

        if self.options.ssh_helper_install_policy == RemoteHelperInstallPolicy::RemoteDownload {
            self.install_ssh_helper_by_remote_download(target, &probe.platform, &helper_path)?;
            if !self.remote_helper_matches(target, &helper_path, &probe.platform) {
                bail!(
                    "reinstalled nucleotide-remote on SSH target {} by download but version probe did not match protocol {}",
                    target.target_arg(),
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
            Some(target.target_arg()),
            Some(format!("upload {}", local_helper.display())),
        );
        self.upload_ssh_helper(target, &local_helper, &helper_path)
            .with_context(|| {
                format!(
                    "failed to reinstall nucleotide-remote on SSH target {}",
                    target.target_arg()
                )
            })?;

        if !self.remote_helper_matches(target, &helper_path, &probe.platform) {
            bail!(
                "reinstalled nucleotide-remote on SSH target {} but version probe did not match protocol {}",
                target.target_arg(),
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

    fn probe_ssh_platform(&self, target: &SshTarget) -> Result<SshRemoteProbe> {
        self.emit_progress(
            RemoteDeploymentPhase::DetectingRemotePlatform,
            Some(target.target_arg()),
            None,
        );
        let script = concat!(
            "printf 'NUCL_PLATFORM '; uname -sm; ",
            "printf 'NUCL_CACHE %s\\n' \"${XDG_CACHE_HOME:-$HOME/.cache}\""
        );
        let output = self.run_ssh_command_output(
            target,
            "detecting remote platform",
            &format!("sh -lc {}", quote_posix_shell(script)),
        )?;
        parse_ssh_probe_output(&output)
    }

    fn remote_helper_matches(
        &self,
        target: &SshTarget,
        helper_path: &Path,
        platform: &SshRemotePlatform,
    ) -> bool {
        self.remote_helper_version(target, helper_path)
            .map(|info| helper_version_matches_current(&info, platform))
            .unwrap_or(false)
    }

    fn remote_helper_version(
        &self,
        target: &SshTarget,
        helper_path: &Path,
    ) -> Result<HelperVersionInfo> {
        let helper_path = helper_path.to_string_lossy();
        let remote_command = format!("exec {} version --json", quote_posix_shell(&helper_path));
        let output =
            self.run_ssh_command_output(target, "checking nucleotide-remote", &remote_command)?;
        parse_helper_version_output(&output)
    }

    fn install_ssh_helper_by_remote_download(
        &self,
        target: &SshTarget,
        platform: &SshRemotePlatform,
        helper_path: &Path,
    ) -> Result<()> {
        let asset_name = remote_helper_release_asset_name(platform);
        let (asset_url, checksums_url) = self.remote_helper_download_urls(platform)?;
        self.emit_progress(
            RemoteDeploymentPhase::InstallingRemoteHelper,
            Some(target.target_arg()),
            Some(format!("download {asset_name}")),
        );
        self.remote_download_ssh_helper(
            target,
            helper_path,
            &asset_url,
            &checksums_url,
            &asset_name,
        )
        .with_context(|| {
            format!(
                "failed to download nucleotide-remote on SSH target {}",
                target.target_arg()
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
            bail!("SSH helper remote-download base URL is empty");
        }

        Ok((
            format!("{base_url}/{}", remote_helper_release_asset_name(platform)),
            format!("{base_url}/{RELEASE_CHECKSUMS_ASSET}"),
        ))
    }

    fn upload_ssh_helper(
        &self,
        target: &SshTarget,
        local_helper: &Path,
        helper_path: &Path,
    ) -> Result<()> {
        let helper_path = helper_path.to_string_lossy();
        let helper_dir = posix_parent(&helper_path);
        let tmp_path = format!(
            "{helper_path}.tmp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        let script = concat!(
            "set -eu\n",
            "dir=$1\n",
            "tmp=$2\n",
            "final=$3\n",
            "mkdir -p \"$dir\"\n",
            "chmod 700 \"$dir\"\n",
            "cat > \"$tmp\"\n",
            "chmod 755 \"$tmp\"\n",
            "mv -f \"$tmp\" \"$final\"\n",
        );
        let remote_command = format!(
            "sh -lc {} sh {} {} {}",
            quote_posix_shell(script),
            quote_posix_shell(&helper_dir),
            quote_posix_shell(&tmp_path),
            quote_posix_shell(&helper_path)
        );
        let spec = ssh_non_tty_command(target.clone(), remote_command);
        let local_file = std::fs::File::open(local_helper)
            .with_context(|| format!("failed to open {}", local_helper.display()))?;
        let output = spec
            .command()
            .stdin(Stdio::from(local_file))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| {
                format!(
                    "failed to run SSH helper upload command: {}",
                    spec.display_context()
                )
            })?;

        if output.status.success() {
            Ok(())
        } else {
            bail!(
                "SSH helper upload command failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
    }

    fn remote_download_ssh_helper(
        &self,
        target: &SshTarget,
        helper_path: &Path,
        asset_url: &str,
        checksums_url: &str,
        asset_name: &str,
    ) -> Result<()> {
        let helper_path = helper_path.to_string_lossy();
        let helper_dir = posix_parent(&helper_path);
        let tmp_path = format!(
            "{helper_path}.tmp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        let remote_command = ssh_remote_helper_download_command(
            &helper_dir,
            &tmp_path,
            &helper_path,
            asset_url,
            checksums_url,
            asset_name,
        );
        let output = ssh_non_tty_command(target.clone(), remote_command)
            .command()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| {
                format!(
                    "failed to run SSH helper download command for {}",
                    target.target_arg()
                )
            })?;

        if output.status.success() {
            Ok(())
        } else {
            bail!(
                "SSH helper download command failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
    }

    fn run_ssh_command_output(
        &self,
        target: &SshTarget,
        label: &'static str,
        remote_command: &str,
    ) -> Result<String> {
        let spec = ssh_non_tty_command(target.clone(), remote_command.to_string());
        let output = spec.command().output().with_context(|| {
            format!(
                "failed to run SSH command while {label}: {}",
                spec.display_context()
            )
        })?;

        if output.status.success() {
            String::from_utf8(output.stdout)
                .with_context(|| format!("SSH command while {label} returned non-UTF-8 stdout"))
        } else {
            bail!(
                "SSH command while {label} failed with status {}: {}",
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

fn remote_lsp_proxy_command_for_location_with_options(
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
            spawn_child_process_workspace_backend(identity, &retry_command).with_context(|| {
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
    if !matches!(location, WorkspaceLocation::Ssh { .. }) {
        return false;
    }

    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("protocol version")
            || message.contains("frame version")
            || message.contains("invalid frame magic")
            || message.contains("unexpected hello response")
            || message.contains("remote service disconnected")
            || message.contains("verify the helper exists and speaks protocol version")
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

fn default_ssh_control_path() -> Option<PathBuf> {
    let control_dir = dirs::cache_dir()?.join("nucleotide").join("ssh-control");
    Some(control_dir.join("%C"))
}

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

fn parse_ssh_probe_output(output: &str) -> Result<SshRemoteProbe> {
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
        platform: platform.context("SSH platform probe did not report NUCL_PLATFORM")?,
        cache_root: cache_root.context("SSH platform probe did not report NUCL_CACHE")?,
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
        other => bail!("unsupported SSH remote platform: {other} {arch}"),
    };
    let arch = match arch {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        arch if arch.starts_with("armv8") || arch.starts_with("armv9") => "aarch64",
        other => bail!("unsupported SSH remote platform: {os} {other}"),
    };

    Ok(SshRemotePlatform {
        os: os.to_string(),
        arch: arch.to_string(),
    })
}

fn ssh_remote_helper_path(probe: &SshRemoteProbe) -> PathBuf {
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

fn ssh_remote_helper_download_command(
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
        "trap cleanup EXIT INT TERM HUP\n",
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
            "Install nucleotide-remote inside WSL distro {distro} at {} or set NUCLEOTIDE_REMOTE_HELPER to a Linux path visible in that distro.",
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
pub struct DirectoryListingResponse {
    pub path: PathBuf,
    pub entries: Vec<DirectoryEntryResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadResponse {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
    pub readonly: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteResultResponse {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_millis: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchRequest {
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
    pub excluded_relative_paths: Vec<PathBuf>,
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
    Protocol(String),
    Remote(RemoteError),
}

impl fmt::Display for RemoteClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "remote transport I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "remote protocol JSON failed: {error}"),
            Self::Disconnected => formatter.write_str("remote service disconnected"),
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
            Self::Disconnected | Self::Protocol(_) | Self::Remote(_) => None,
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

pub struct RemoteWorkspaceClient<T> {
    transport: T,
    next_request_id: u64,
}

impl<T> RemoteWorkspaceClient<T>
where
    T: RemoteTransport,
{
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_request_id: 1,
        }
    }

    pub fn request(
        &mut self,
        request: RemoteRequest,
        body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let request_id = self.next_id();
        let envelope = RequestEnvelope::new(request);
        let frame = Frame::from_json_header(FrameKind::Request, request_id, 0, &envelope, body)?;
        self.transport.write_frame(&frame)?;

        loop {
            let frame = self
                .transport
                .read_frame()?
                .ok_or(RemoteClientError::Disconnected)?;
            if frame.request_id != request_id {
                return Err(RemoteClientError::Protocol(format!(
                    "received frame for request {}, expected {}",
                    frame.request_id, request_id
                )));
            }

            match frame.kind {
                FrameKind::Response => {
                    let envelope = frame.decode_json_header::<ResponseEnvelope>()?;
                    if envelope.protocol_version != PROTOCOL_VERSION {
                        return Err(RemoteClientError::Protocol(format!(
                            "unsupported response protocol version {}; expected {}",
                            envelope.protocol_version, PROTOCOL_VERSION
                        )));
                    }
                    return Ok((envelope.response, frame.body));
                }
                FrameKind::Error => {
                    let envelope = frame.decode_json_header::<ErrorEnvelope>()?;
                    if envelope.protocol_version != PROTOCOL_VERSION {
                        return Err(RemoteClientError::Protocol(format!(
                            "unsupported error protocol version {}; expected {}",
                            envelope.protocol_version, PROTOCOL_VERSION
                        )));
                    }
                    return Err(RemoteClientError::Remote(envelope.error));
                }
                FrameKind::Progress => continue,
                other => {
                    return Err(RemoteClientError::Protocol(format!(
                        "unexpected response frame kind: {other:?}"
                    )));
                }
            }
        }
    }

    pub fn hello(&mut self) -> std::result::Result<HelloResponse, RemoteClientError> {
        let (response, _) = self.request(RemoteRequest::Hello, Vec::new())?;
        match response {
            RemoteResponse::Hello(hello) => Ok(hello),
            other => Err(RemoteClientError::Protocol(format!(
                "unexpected hello response: {other:?}"
            ))),
        }
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

    fn next_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }
}

pub struct RemoteWorkspaceBackend<T: RemoteTransport> {
    identity: RemoteWorkspaceIdentity,
    client: Mutex<RemoteWorkspaceClient<T>>,
}

impl<T> RemoteWorkspaceBackend<T>
where
    T: RemoteTransport,
{
    pub fn new(identity: RemoteWorkspaceIdentity, client: RemoteWorkspaceClient<T>) -> Self {
        Self {
            identity,
            client: Mutex::new(client),
        }
    }

    pub fn connect(
        identity: RemoteWorkspaceIdentity,
        mut client: RemoteWorkspaceClient<T>,
    ) -> std::result::Result<(Self, HelloResponse), RemoteClientError> {
        let hello = client.hello()?;
        Ok((Self::new(identity, client), hello))
    }

    fn request(
        &self,
        operation: &'static str,
        path: &Path,
        request: RemoteRequest,
        body: Vec<u8>,
    ) -> nucleotide_workspace::Result<(RemoteResponse, Vec<u8>)> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| remote_lock_error(operation, path))?;
        client
            .request(request, body)
            .map_err(|error| client_error_to_workspace(operation, path, error))
    }
}

impl<T> Drop for RemoteWorkspaceBackend<T>
where
    T: RemoteTransport,
{
    fn drop(&mut self) {
        if let Ok(mut client) = self.client.lock() {
            let _ = client.shutdown();
        }
    }
}

pub fn spawn_child_process_workspace_backend(
    identity: RemoteWorkspaceIdentity,
    command: &RemoteServiceCommand,
) -> Result<(WorkspaceBackendHandle, HelloResponse)> {
    let transport = command.spawn().with_context(|| {
        format!(
            "failed to start remote workspace service: {}",
            command.display_context()
        )
    })?;
    let client = RemoteWorkspaceClient::new(transport);
    let (backend, hello) =
        RemoteWorkspaceBackend::connect(identity, client).with_context(|| {
            format!(
                concat!(
                    "failed to connect to remote workspace service after starting {}; ",
                    "verify the helper exists and speaks protocol version {}"
                ),
                command.display_context(),
                PROTOCOL_VERSION
            )
        })?;

    Ok((Arc::new(backend), hello))
}

#[async_trait]
impl<T> WorkspaceBackend for RemoteWorkspaceBackend<T>
where
    T: RemoteTransport + Send,
{
    fn identity(&self) -> WorkspaceIdentity {
        WorkspaceIdentity::Remote(self.identity.clone())
    }

    async fn stat(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self.request(
            "stat",
            path,
            RemoteRequest::Stat {
                path: path.to_path_buf(),
            },
            Vec::new(),
        )?;
        match response {
            RemoteResponse::Stat(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("stat", path, other)),
        }
    }

    async fn list_dir(&self, path: &Path) -> nucleotide_workspace::Result<DirectoryListing> {
        let (response, _) = self.request(
            "list directory",
            path,
            RemoteRequest::ListDir {
                path: path.to_path_buf(),
            },
            Vec::new(),
        )?;
        match response {
            RemoteResponse::ListDir(listing) => Ok(directory_listing_from_response(listing)),
            other => Err(unexpected_response_error("list directory", path, other)),
        }
    }

    async fn find_ancestor_file(
        &self,
        start: &Path,
        file_name: &str,
        limit: usize,
    ) -> nucleotide_workspace::Result<Option<PathBuf>> {
        let (response, _) = self.request(
            "find ancestor file",
            start,
            RemoteRequest::FindAncestorFile {
                start: start.to_path_buf(),
                file_name: file_name.to_string(),
                limit,
            },
            Vec::new(),
        )?;
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
        let (response, _) = self.request(
            "create file",
            path,
            RemoteRequest::CreateFile {
                path: path.to_path_buf(),
            },
            Vec::new(),
        )?;
        match response {
            RemoteResponse::CreateFile(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("create file", path, other)),
        }
    }

    async fn create_dir(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self.request(
            "create directory",
            path,
            RemoteRequest::CreateDir {
                path: path.to_path_buf(),
            },
            Vec::new(),
        )?;
        match response {
            RemoteResponse::CreateDir(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("create directory", path, other)),
        }
    }

    async fn rename_path(&self, from: &Path, to: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self.request(
            "rename path",
            from,
            RemoteRequest::RenamePath {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            },
            Vec::new(),
        )?;
        match response {
            RemoteResponse::RenamePath(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("rename path", from, other)),
        }
    }

    async fn delete_path(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self.request(
            "delete path",
            path,
            RemoteRequest::DeletePath {
                path: path.to_path_buf(),
            },
            Vec::new(),
        )?;
        match response {
            RemoteResponse::DeletePath(stat) => Ok(file_stat_from_response(stat)),
            other => Err(unexpected_response_error("delete path", path, other)),
        }
    }

    async fn copy_path(&self, from: &Path, to: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self.request(
            "copy path",
            from,
            RemoteRequest::CopyPath {
                from: from.to_path_buf(),
                to: to.to_path_buf(),
            },
            Vec::new(),
        )?;
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
        let (response, body) = self.request(
            "read file",
            path,
            RemoteRequest::ReadFile {
                path: path.to_path_buf(),
                max_bytes: options.max_bytes,
            },
            Vec::new(),
        )?;
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
        let (response, _) = self.request(
            "write file",
            path,
            RemoteRequest::WriteFile {
                path: path.to_path_buf(),
                create_parent_dirs: options.create_parent_dirs,
                expected_modified_unix_millis: options
                    .expected_modified
                    .and_then(system_time_unix_millis),
            },
            bytes.to_vec(),
        )?;
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
        let (response, _) = self.request(
            "file search",
            &root,
            RemoteRequest::FileSearch(request),
            Vec::new(),
        )?;
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
        let (response, _) = self.request(
            "text search",
            &root,
            RemoteRequest::TextSearch(request),
            Vec::new(),
        )?;
        match response {
            RemoteResponse::TextSearch(result) => Ok(text_search_from_response(result)),
            other => Err(unexpected_response_error("text search", &root, other)),
        }
    }

    async fn project_environment(
        &self,
        root: &Path,
    ) -> nucleotide_workspace::Result<ProjectEnvironmentSnapshot> {
        let (response, _) = self.request(
            "project environment",
            root,
            RemoteRequest::ProjectEnvironment {
                root: root.to_path_buf(),
            },
            Vec::new(),
        )?;
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
        let (response, _) = self.request(
            "git head",
            root,
            RemoteRequest::GitHead {
                root: root.to_path_buf(),
            },
            Vec::new(),
        )?;
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
        let (response, _) = self.request(
            "git status",
            root,
            RemoteRequest::GitStatus {
                root: root.to_path_buf(),
                include_untracked: options.include_untracked,
                limit: options.limit,
            },
            Vec::new(),
        )?;
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
        let (response, body) = self.request(
            "run process",
            &cwd,
            RemoteRequest::RunProcess(request),
            spec.stdin,
        )?;
        match response {
            RemoteResponse::RunProcess(result) => process_output_from_response(result, body)
                .map_err(|error| client_error_to_workspace("run process", &cwd, error)),
            other => Err(unexpected_response_error("run process", &cwd, other)),
        }
    }
}

pub struct WorkspaceService<B> {
    backend: B,
    workspace_root: PathBuf,
    ignore_matcher: Option<Gitignore>,
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
            project_environment: ProjectEnvironment::new(Some(environment_baseline)),
            runtime,
        })
    }

    pub fn serve<R: Read, W: Write>(&self, reader: &mut R, writer: &mut W) -> Result<()> {
        while let Some(frame) = read_frame(reader).context("failed to read protocol frame")? {
            match frame.kind {
                FrameKind::Request => {
                    let should_continue = self.handle_request(frame, writer)?;
                    if !should_continue {
                        break;
                    }
                }
                FrameKind::Cancel => {
                    self.write_error(
                        writer,
                        frame.request_id,
                        "unsupported_cancel",
                        "cancellation is not available for this operation yet",
                        None,
                    )?;
                }
                FrameKind::Shutdown => break,
                other => {
                    self.write_error(
                        writer,
                        frame.request_id,
                        "unexpected_frame",
                        format!("unexpected frame kind from client: {other:?}"),
                        None,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn handle_request<W: Write>(&self, frame: Frame, writer: &mut W) -> Result<bool> {
        let request = match frame.decode_json_header::<RequestEnvelope>() {
            Ok(request) => request,
            Err(error) => {
                self.write_error(
                    writer,
                    frame.request_id,
                    "invalid_request",
                    "request header is not valid JSON",
                    Some(error.to_string()),
                )?;
                return Ok(true);
            }
        };

        if request.protocol_version != PROTOCOL_VERSION {
            self.write_error(
                writer,
                frame.request_id,
                "protocol_mismatch",
                format!(
                    "unsupported protocol version {}; expected {}",
                    request.protocol_version, PROTOCOL_VERSION
                ),
                None,
            )?;
            return Ok(true);
        }

        match self.execute(request.request, frame.body) {
            Ok(ServiceOutcome::Continue(response, body)) => {
                self.write_response(writer, frame.request_id, response, body)?;
                Ok(true)
            }
            Ok(ServiceOutcome::Shutdown) => {
                self.write_response(
                    writer,
                    frame.request_id,
                    RemoteResponse::Shutdown,
                    Vec::new(),
                )?;
                Ok(false)
            }
            Err(error) => {
                self.write_error(
                    writer,
                    frame.request_id,
                    &error.code,
                    error.message,
                    error.diagnostic,
                )?;
                Ok(true)
            }
        }
    }

    fn execute(
        &self,
        request: RemoteRequest,
        request_body: Vec<u8>,
    ) -> std::result::Result<ServiceOutcome, RemoteError> {
        match request {
            RemoteRequest::Hello => Ok(ServiceOutcome::Continue(
                RemoteResponse::Hello(HelloResponse::current(self.workspace_root.clone())),
                Vec::new(),
            )),
            RemoteRequest::Stat { path } => {
                let path = self.resolve_path(&path)?;
                let stat =
                    block_on(self.backend.stat(&path)).map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::Stat(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::ListDir { path } => {
                let path = self.resolve_path(&path)?;
                let listing =
                    block_on(self.backend.list_dir(&path)).map_err(remote_error_from_workspace)?;
                let listing = annotate_directory_listing_ignored(
                    listing,
                    &self.workspace_root,
                    self.ignore_matcher.as_ref(),
                );
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::ListDir(directory_listing_response(listing)),
                    Vec::new(),
                ))
            }
            RemoteRequest::FindAncestorFile {
                start,
                file_name,
                limit,
            } => {
                let start = self.resolve_path(&start)?;
                let path = block_on(self.backend.find_ancestor_file(
                    &start,
                    file_name.as_str(),
                    limit,
                ))
                .map_err(remote_error_from_workspace)?;
                let path = path.filter(|path| path_is_within_workspace(path, &self.workspace_root));
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::FindAncestorFile(path),
                    Vec::new(),
                ))
            }
            RemoteRequest::CreateFile { path } => {
                let path = self.resolve_path(&path)?;
                let stat = block_on(self.backend.create_file(&path))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::CreateFile(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::CreateDir { path } => {
                let path = self.resolve_path(&path)?;
                let stat = block_on(self.backend.create_dir(&path))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::CreateDir(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::RenamePath { from, to } => {
                let from = self.resolve_path(&from)?;
                let to = self.resolve_path(&to)?;
                let stat = block_on(self.backend.rename_path(&from, &to))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::RenamePath(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::DeletePath { path } => {
                let path = self.resolve_path(&path)?;
                let stat = block_on(self.backend.delete_path(&path))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::DeletePath(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::CopyPath { from, to } => {
                let from = self.resolve_path(&from)?;
                let to = self.resolve_path(&to)?;
                let stat = block_on(self.backend.copy_path(&from, &to))
                    .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::CopyPath(file_stat_response(stat)),
                    Vec::new(),
                ))
            }
            RemoteRequest::ReadFile { path, max_bytes } => {
                let path = self.resolve_path(&path)?;
                let max_bytes = Some(
                    max_bytes
                        .unwrap_or(MAX_FRAME_BODY_LEN)
                        .min(MAX_FRAME_BODY_LEN),
                );
                let read = block_on(self.backend.read_file(&path, ReadOptions { max_bytes }))
                    .map_err(remote_error_from_workspace)?;
                let response = file_read_response(&read);
                let body = read.bytes;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::ReadFile(response),
                    body,
                ))
            }
            RemoteRequest::WriteFile {
                path,
                create_parent_dirs,
                expected_modified_unix_millis,
            } => {
                let path = self.resolve_path(&path)?;
                let expected_modified =
                    expected_modified_unix_millis.and_then(system_time_from_unix_millis);
                let result = block_on(self.backend.write_file(
                    &path,
                    &request_body,
                    WriteOptions {
                        create_parent_dirs,
                        expected_modified,
                    },
                ))
                .map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
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
                Ok(ServiceOutcome::Continue(
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
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::TextSearch(text_search_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::ProjectEnvironment { root } => {
                let root = self.resolve_search_root(&root)?;
                let snapshot = self
                    .load_project_environment(&root)
                    .map_err(remote_error_from_environment)?;
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::ProjectEnvironment(project_environment_response(snapshot)),
                    Vec::new(),
                ))
            }
            RemoteRequest::GitHead { root } => {
                let root = self.resolve_search_root(&root)?;
                let result =
                    block_on(self.backend.git_head(&root)).map_err(remote_error_from_workspace)?;
                Ok(ServiceOutcome::Continue(
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
                Ok(ServiceOutcome::Continue(
                    RemoteResponse::GitStatus(git_status_response(result)),
                    Vec::new(),
                ))
            }
            RemoteRequest::RunProcess(request) => {
                let cwd = self.resolve_path(&request.cwd)?;
                let max_output_bytes = Some(
                    request
                        .max_output_bytes
                        .unwrap_or((MAX_FRAME_BODY_LEN / 2) as usize)
                        .min((MAX_FRAME_BODY_LEN / 2) as usize),
                );
                let env = if request.inherit_project_environment {
                    let environment_root = self.project_environment_root_for_process(&cwd);
                    let mut project_environment = self
                        .load_project_environment(&environment_root)
                        .map_err(remote_error_from_environment)?
                        .variables;
                    project_environment.extend(request.env);
                    project_environment
                } else {
                    request.env
                };
                let output = block_on(self.backend.run_process(ProcessSpec {
                    program: request.program,
                    args: request.args,
                    cwd,
                    env,
                    clear_env: request.clear_env,
                    inherit_project_environment: false,
                    stdin: request_body,
                    max_output_bytes,
                    timeout_ms: request.timeout_ms,
                }))
                .map_err(remote_error_from_workspace)?;
                let response = process_output_response(&output);
                let mut body = output.stdout;
                body.extend_from_slice(&output.stderr);
                Ok(ServiceOutcome::Continue(
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
        self.runtime.block_on(async {
            let mut variables = self
                .project_environment
                .get_environment_for_directory(root)
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

    fn resolve_search_root(&self, root: &Path) -> std::result::Result<PathBuf, RemoteError> {
        if root.as_os_str().is_empty() {
            Ok(self.workspace_root.clone())
        } else {
            self.resolve_path(root)
        }
    }

    fn write_response<W: Write>(
        &self,
        writer: &mut W,
        request_id: u64,
        response: RemoteResponse,
        body: Vec<u8>,
    ) -> Result<()> {
        let envelope = ResponseEnvelope::new(response);
        let frame = Frame::from_json_header(FrameKind::Response, request_id, 0, &envelope, body)
            .context("failed to encode response frame")?;
        write_frame(writer, &frame).context("failed to write response frame")
    }

    fn write_error<W: Write>(
        &self,
        writer: &mut W,
        request_id: u64,
        code: impl Into<String>,
        message: impl Into<String>,
        diagnostic: Option<String>,
    ) -> Result<()> {
        let envelope = ErrorEnvelope::new(RemoteError {
            code: code.into(),
            message: message.into(),
            diagnostic,
        });
        let frame = Frame::from_json_header(FrameKind::Error, request_id, 0, &envelope, Vec::new())
            .context("failed to encode error frame")?;
        write_frame(writer, &frame).context("failed to write error frame")
    }
}

pub fn serve_local_workspace<R: Read, W: Write>(
    workspace_root: PathBuf,
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    WorkspaceService::new(LocalWorkspaceBackend, workspace_root)?.serve(reader, writer)
}

enum ServiceOutcome {
    Continue(RemoteResponse, Vec<u8>),
    Shutdown,
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
        readonly: stat.readonly,
    }
}

fn directory_listing_response(listing: DirectoryListing) -> DirectoryListingResponse {
    DirectoryListingResponse {
        path: listing.path,
        entries: listing
            .entries
            .into_iter()
            .map(|entry| DirectoryEntryResponse {
                name: entry.name,
                path: entry.path,
                stat: file_stat_response(entry.stat),
                symlink_target: entry.symlink_target,
                target_exists: entry.target_exists,
                ignored: entry.ignored,
            })
            .collect(),
    }
}

fn file_read_response(read: &FileRead) -> FileReadResponse {
    FileReadResponse {
        path: read.path.clone(),
        size: read.size,
        modified_unix_millis: read.modified.and_then(system_time_unix_millis),
        readonly: read.readonly,
        truncated: read.truncated,
    }
}

fn write_result_response(result: WriteResult) -> WriteResultResponse {
    WriteResultResponse {
        path: result.path,
        size: result.size,
        modified_unix_millis: result.modified.and_then(system_time_unix_millis),
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

fn annotate_directory_listing_ignored(
    mut listing: DirectoryListing,
    root_path: &Path,
    matcher: Option<&Gitignore>,
) -> DirectoryListing {
    for entry in &mut listing.entries {
        entry.ignored = Some(service_path_is_ignored(
            root_path,
            matcher,
            &entry.path,
            entry.stat.kind,
        ));
    }
    listing
}

fn file_stat_from_response(stat: FileStatResponse) -> FileStat {
    FileStat {
        path: stat.path,
        kind: file_kind_from_response(stat.kind),
        size: stat.size,
        modified: stat
            .modified_unix_millis
            .and_then(system_time_from_unix_millis),
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
        modified: read
            .modified_unix_millis
            .and_then(system_time_from_unix_millis),
        readonly: read.readonly,
        truncated: read.truncated,
    })
}

fn write_result_from_response(result: WriteResultResponse) -> WriteResult {
    WriteResult {
        path: result.path,
        size: result.size,
        modified: result
            .modified_unix_millis
            .and_then(system_time_from_unix_millis),
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

fn remote_lock_error(operation: &'static str, path: &Path) -> WorkspaceError {
    WorkspaceError::Remote {
        operation,
        path: path.to_path_buf(),
        message: "remote client lock is poisoned".to_string(),
        diagnostic: None,
    }
}

fn client_error_to_workspace(
    operation: &'static str,
    path: &Path,
    error: RemoteClientError,
) -> WorkspaceError {
    match error {
        RemoteClientError::Remote(error) if error.code == "modified" => WorkspaceError::Modified {
            path: path.to_path_buf(),
        },
        RemoteClientError::Remote(error) if error.code == "not_file" => WorkspaceError::NotFile {
            path: path.to_path_buf(),
        },
        RemoteClientError::Remote(error) => WorkspaceError::Remote {
            operation,
            path: path.to_path_buf(),
            message: error.message,
            diagnostic: error.diagnostic,
        },
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

fn system_time_from_unix_millis(millis: i64) -> Option<SystemTime> {
    u64::try_from(millis)
        .ok()
        .map(|millis| UNIX_EPOCH + Duration::from_millis(millis))
}

pub fn run_from_args<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_else(|| "help".to_string());

    match command.as_str() {
        "serve" => {
            let workspace_root = parse_workspace_root(args)?;
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            serve_local_workspace(workspace_root, &mut stdin.lock(), &mut stdout.lock())
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

fn parse_workspace_root<I>(args: I) -> Result<PathBuf>
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
            other => bail!("unknown serve argument: {other}"),
        }
    }

    workspace_root
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .context("failed to resolve workspace root")
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

    let mut child = tokio::process::Command::new(&server_program)
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

    let (program, args) = terminal_proxy_process(&options, &environment);
    let program_path =
        resolve_program_from_environment_path(&program, &environment, &options.workspace_root);

    exec_terminal_proxy_process(&program_path, &args, &environment, &options.workspace_root)
        .with_context(|| {
            format!(
                "failed to run terminal command {} in {}",
                program_path.display(),
                options.workspace_root.display()
            )
        })
}

fn terminal_proxy_process(
    options: &TerminalProxyOptions,
    environment: &HashMap<String, String>,
) -> (String, Vec<String>) {
    match &options.command {
        Some((program, args)) => (program.clone(), args.clone()),
        None => {
            let shell = options
                .shell
                .as_deref()
                .filter(|shell| !shell.is_empty())
                .or_else(|| environment.get("SHELL").map(String::as_str))
                .filter(|shell| !shell.is_empty())
                .unwrap_or("/bin/sh")
                .to_string();
            (shell, Vec::new())
        }
    }
}

#[cfg(unix)]
fn exec_terminal_proxy_process(
    program: &Path,
    args: &[String],
    environment: &HashMap<String, String>,
    workspace_root: &Path,
) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    let error = Command::new(program)
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
    use nucleotide_workspace::RemoteWorkspaceKind;
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn executable_tempdir() -> tempfile::TempDir {
        let base = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("test-tmp");
        std::fs::create_dir_all(&base).unwrap();
        tempfile::Builder::new()
            .prefix("nucleotide-remote-")
            .tempdir_in(base)
            .unwrap()
    }

    fn generated_executable_is_allowed(executable: &Path) -> bool {
        match std::process::Command::new(executable)
            .arg("--probe")
            .status()
        {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
            Err(error) => panic!(
                "failed to probe generated executable {}: {error}",
                executable.display()
            ),
        }
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
    fn terminal_proxy_process_uses_environment_shell_without_extra_flags() {
        let options = TerminalProxyOptions {
            workspace_root: PathBuf::from("/workspace"),
            shell: None,
            env: Vec::new(),
            command: None,
        };
        let environment = HashMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]);

        let (program, args) = terminal_proxy_process(&options, &environment);

        assert_eq!(program, "/bin/zsh");
        assert!(args.is_empty());
    }

    #[test]
    fn lsp_proxy_resolves_server_from_project_environment_path() {
        let temp = tempfile::tempdir().unwrap();
        let server = temp.path().join("rust-analyzer");
        std::fs::write(&server, "").unwrap();
        let environment = HashMap::from([(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin", temp.path().display()),
        )]);

        assert_eq!(
            resolve_program_from_environment_path("rust-analyzer", &environment, temp.path()),
            server
        );
        assert_eq!(
            resolve_program_from_environment_path(
                "/custom/rust-analyzer",
                &environment,
                temp.path()
            ),
            PathBuf::from("/custom/rust-analyzer")
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

    struct LoopbackTransport {
        root: PathBuf,
        pending: VecDeque<Frame>,
    }

    impl LoopbackTransport {
        fn new(root: &Path) -> Self {
            Self {
                root: root.to_path_buf(),
                pending: VecDeque::new(),
            }
        }
    }

    impl RemoteTransport for LoopbackTransport {
        fn write_frame(&mut self, frame: &Frame) -> io::Result<()> {
            let mut input = Vec::new();
            write_frame(&mut input, frame)?;
            let root = self.root.clone();
            let output = std::thread::spawn(move || {
                let service = WorkspaceService::new(LocalWorkspaceBackend, root)?;
                let mut output = Vec::new();
                service
                    .serve(&mut Cursor::new(input), &mut output)
                    .map(|_| output)
            })
            .join()
            .map_err(|_| io::Error::other("loopback service thread panicked"))?
            .map_err(|error| io::Error::other(error.to_string()))?;

            let mut cursor = Cursor::new(output);
            while let Some(frame) = read_frame(&mut cursor)? {
                self.pending.push_back(frame);
            }
            Ok(())
        }

        fn read_frame(&mut self) -> io::Result<Option<Frame>> {
            Ok(self.pending.pop_front())
        }
    }

    struct ShutdownRecordingTransport {
        saw_shutdown: Arc<AtomicBool>,
        pending: VecDeque<Frame>,
    }

    impl ShutdownRecordingTransport {
        fn new(saw_shutdown: Arc<AtomicBool>) -> Self {
            Self {
                saw_shutdown,
                pending: VecDeque::new(),
            }
        }
    }

    impl RemoteTransport for ShutdownRecordingTransport {
        fn write_frame(&mut self, frame: &Frame) -> io::Result<()> {
            let envelope = frame
                .decode_json_header::<RequestEnvelope>()
                .map_err(io::Error::other)?;
            let response = match envelope.request {
                RemoteRequest::Shutdown => {
                    self.saw_shutdown.store(true, Ordering::SeqCst);
                    RemoteResponse::Shutdown
                }
                other => {
                    return Err(io::Error::other(format!("unexpected request: {other:?}")));
                }
            };

            self.pending.push_back(
                Frame::from_json_header(
                    FrameKind::Response,
                    frame.request_id,
                    0,
                    &ResponseEnvelope::new(response),
                    Vec::new(),
                )
                .map_err(io::Error::other)?,
            );
            Ok(())
        }

        fn read_frame(&mut self) -> io::Result<Option<Frame>> {
            Ok(self.pending.pop_front())
        }
    }

    struct StaticResponseTransport {
        pending: VecDeque<Frame>,
    }

    impl StaticResponseTransport {
        fn new(response: Frame) -> Self {
            Self {
                pending: VecDeque::from([response]),
            }
        }
    }

    impl RemoteTransport for StaticResponseTransport {
        fn write_frame(&mut self, _frame: &Frame) -> io::Result<()> {
            Ok(())
        }

        fn read_frame(&mut self) -> io::Result<Option<Frame>> {
            Ok(self.pending.pop_front())
        }
    }

    fn loopback_identity() -> RemoteWorkspaceIdentity {
        RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Other("loopback".to_string()),
            name: "loopback".to_string(),
        }
    }

    fn remote_backend(root: &Path) -> RemoteWorkspaceBackend<LoopbackTransport> {
        RemoteWorkspaceBackend::new(
            loopback_identity(),
            RemoteWorkspaceClient::new(LoopbackTransport::new(root)),
        )
    }

    fn static_response_backend(
        response: RemoteResponse,
        body: Vec<u8>,
    ) -> RemoteWorkspaceBackend<StaticResponseTransport> {
        let frame = Frame::from_json_header(
            FrameKind::Response,
            1,
            0,
            &ResponseEnvelope::new(response),
            body,
        )
        .unwrap();

        RemoteWorkspaceBackend::new(
            loopback_identity(),
            RemoteWorkspaceClient::new(StaticResponseTransport::new(frame)),
        )
    }

    #[test]
    fn remote_client_hello_returns_service_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = RemoteWorkspaceClient::new(LoopbackTransport::new(temp.path()));

        let hello = client.hello().unwrap();

        assert_eq!(hello.workspace_root, temp.path());
        assert_eq!(hello.helper_version, env!("CARGO_PKG_VERSION"));
        assert!(hello.capabilities.contains(&"list_dir".to_string()));
        assert!(
            hello
                .capabilities
                .contains(&"directory_entry_ignored".to_string())
        );
    }

    #[test]
    fn remote_backend_connect_performs_handshake() {
        let temp = tempfile::tempdir().unwrap();
        let client = RemoteWorkspaceClient::new(LoopbackTransport::new(temp.path()));

        let (backend, hello) =
            RemoteWorkspaceBackend::connect(loopback_identity(), client).unwrap();

        assert_eq!(hello.workspace_root, temp.path());
        assert_eq!(
            backend.identity(),
            WorkspaceIdentity::Remote(loopback_identity())
        );
    }

    #[test]
    fn remote_backend_drop_sends_shutdown() {
        let saw_shutdown = Arc::new(AtomicBool::new(false));
        let transport = ShutdownRecordingTransport::new(saw_shutdown.clone());
        let backend =
            RemoteWorkspaceBackend::new(loopback_identity(), RemoteWorkspaceClient::new(transport));

        drop(backend);

        assert!(saw_shutdown.load(Ordering::SeqCst));
    }

    #[test]
    fn remote_backend_file_operations_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let backend = remote_backend(temp.path());
        let dir = temp.path().join("src");
        let file = dir.join("main.rs");
        let renamed = dir.join("lib.rs");
        let copied = dir.join("lib-copy.rs");

        let dir_stat = block_on(backend.create_dir(&dir)).unwrap();
        let file_stat = block_on(backend.create_file(&file)).unwrap();
        std::fs::write(&file, "pub fn lib() {}\n").unwrap();
        let renamed_stat = block_on(backend.rename_path(&file, &renamed)).unwrap();
        let copied_stat = block_on(backend.copy_path(&renamed, &copied)).unwrap();
        let deleted_stat = block_on(backend.delete_path(&renamed)).unwrap();

        assert_eq!(dir_stat.path, dir);
        assert_eq!(dir_stat.kind, FileKind::Directory);
        assert_eq!(file_stat.path, file);
        assert_eq!(file_stat.kind, FileKind::File);
        assert_eq!(renamed_stat.path, renamed);
        assert_eq!(copied_stat.path, copied);
        assert_eq!(deleted_stat.path, renamed);
        assert!(!renamed.exists());
        assert_eq!(
            std::fs::read_to_string(copied).unwrap(),
            "pub fn lib() {}\n"
        );
    }

    #[test]
    fn remote_backend_find_ancestor_file_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let backend = remote_backend(temp.path());
        let manifest = temp.path().join("Cargo.toml");
        let file = temp.path().join("src").join("main.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&manifest, "[package]\n").unwrap();
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let found = block_on(backend.find_ancestor_file(&file, "Cargo.toml", 8)).unwrap();

        assert_eq!(found, Some(manifest));
    }

    #[cfg(unix)]
    #[test]
    fn remote_backend_run_process_round_trips_output() {
        let temp = tempfile::tempdir().unwrap();
        let backend = remote_backend(temp.path());

        let output = block_on(backend.run_process(ProcessSpec {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf '%s:' \"$REMOTE_FLAG\"; cat; printf err >&2".to_string(),
            ],
            cwd: PathBuf::new(),
            env: BTreeMap::from([("REMOTE_FLAG".to_string(), "remote".to_string())]),
            clear_env: false,
            inherit_project_environment: false,
            stdin: b"stdin".to_vec(),
            max_output_bytes: None,
            timeout_ms: None,
        }))
        .unwrap();

        assert!(output.success);
        assert_eq!(output.status_code, Some(0));
        assert_eq!(output.stdout, b"remote:stdin");
        assert_eq!(output.stderr, b"err");
        assert!(!output.stdout_truncated);
        assert!(!output.stderr_truncated);
        assert!(!output.timed_out);
    }

    #[cfg(unix)]
    #[test]
    fn remote_backend_run_process_reports_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let backend = remote_backend(temp.path());

        let output = block_on(backend.run_process(ProcessSpec {
            program: "tail".to_string(),
            args: vec!["-f".to_string(), "/dev/null".to_string()],
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: None,
            timeout_ms: Some(20),
        }))
        .unwrap();

        assert!(!output.success);
        assert!(output.timed_out);
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
    fn remote_backend_rejects_malformed_run_process_body() {
        let backend = static_response_backend(
            RemoteResponse::RunProcess(ProcessOutputResponse {
                status_code: Some(0),
                success: true,
                stdout_truncated: false,
                stderr_truncated: false,
                stdout_len: 3,
                stderr_len: 3,
                timed_out: false,
            }),
            b"abc".to_vec(),
        );

        let result = block_on(backend.run_process(ProcessSpec {
            program: "ignored".to_string(),
            args: Vec::new(),
            cwd: PathBuf::new(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: None,
            timeout_ms: None,
        }));

        assert!(matches!(
            result,
            Err(WorkspaceError::Remote { message, .. })
                if message.contains("malformed run_process body")
        ));
    }

    #[test]
    fn remote_backend_rejects_short_untruncated_read_body() {
        let backend = static_response_backend(
            RemoteResponse::ReadFile(FileReadResponse {
                path: PathBuf::from("main.rs"),
                size: 6,
                modified_unix_millis: None,
                readonly: false,
                truncated: false,
            }),
            b"abc".to_vec(),
        );

        let result = block_on(backend.read_file(Path::new("main.rs"), ReadOptions::default()));

        assert!(matches!(
            result,
            Err(WorkspaceError::Remote { message, .. })
                if message.contains("malformed read_file body")
        ));
    }

    fn request_frame(id: u64, request: RemoteRequest, body: Vec<u8>) -> Frame {
        Frame::from_json_header(
            FrameKind::Request,
            id,
            0,
            &RequestEnvelope::new(request),
            body,
        )
        .unwrap()
    }

    fn single_request_output(root: &Path, request: RemoteRequest, body: Vec<u8>) -> Vec<u8> {
        let mut input = Vec::new();
        write_frame(&mut input, &request_frame(7, request, body)).unwrap();
        write_frame(
            &mut input,
            &request_frame(8, RemoteRequest::Shutdown, Vec::new()),
        )
        .unwrap();

        let service = WorkspaceService::new(LocalWorkspaceBackend, root.to_path_buf()).unwrap();
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();
        service.serve(&mut reader, &mut output).unwrap();
        output
    }

    fn read_first_output_frame(output: Vec<u8>) -> Frame {
        read_frame(&mut Cursor::new(output)).unwrap().unwrap()
    }

    #[test]
    fn frame_round_trip_preserves_header_and_body() {
        let envelope = RequestEnvelope::new(RemoteRequest::ReadFile {
            path: PathBuf::from("src/main.rs"),
            max_bytes: Some(10),
        });
        let frame = Frame::from_json_header(FrameKind::Request, 42, 3, &envelope, b"body".to_vec())
            .unwrap();

        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        let decoded = read_frame(&mut Cursor::new(bytes)).unwrap().unwrap();

        assert_eq!(decoded.kind, FrameKind::Request);
        assert_eq!(decoded.request_id, 42);
        assert_eq!(decoded.stream_id, 3);
        assert_eq!(decoded.body, b"body");
        assert_eq!(
            decoded.decode_json_header::<RequestEnvelope>().unwrap(),
            envelope
        );
    }

    #[test]
    fn frame_reader_returns_none_on_clean_eof() {
        assert!(read_frame(&mut Cursor::new(Vec::new())).unwrap().is_none());
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
                OsString::from("/workspace/project")
            ]
        );
        assert_eq!(spec.current_dir, Some(PathBuf::from("/workspace/project")));
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
            format!("'/tmp/nucleotide remote' serve --workspace {quoted_workspace}")
        );
        assert_eq!(
            spec.display_context(),
            format!(
                "'/tmp/nucleotide remote' serve --workspace {quoted_workspace} (cwd {quoted_workspace})"
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
                OsString::from("/home/me/project")
            ]
        );
        assert_eq!(spec.current_dir, None);
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
        assert_eq!(spec.args[1], OsString::from("-p"));
        assert_eq!(spec.args[2], OsString::from("2222"));
        assert_eq!(spec.args[3], OsString::from("--"));
        assert_eq!(spec.args[4], OsString::from("me@devbox"));
        let command = spec.args[5].to_string_lossy();
        assert!(command.starts_with("exec "));
        assert!(command.contains("'/home/me/.cache/nucleotide remote/bin'"));
        assert!(command.contains("'/home/me/project with spaces/it'\"'\"'s'"));
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
        assert_eq!(spec.args[1], OsString::from("-p"));
        assert_eq!(spec.args[2], OsString::from("2222"));
        assert_eq!(spec.args[3], OsString::from("--"));
        assert_eq!(spec.args[4], OsString::from("me@devbox"));
        let command = spec.args[5].to_string_lossy();
        assert!(command.starts_with("exec "));
        assert!(command.contains("'/home/me/.cache/nucleotide remote/bin'"));
        assert!(command.contains(" lsp-proxy "));
        assert!(command.contains("'/home/me/project with spaces/it'\"'\"'s'"));
        assert!(command.contains("typescript-language-server"));
        assert!(command.ends_with(" --"));
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
        assert_eq!(spec.args[0], OsString::from("-p"));
        assert_eq!(spec.args[1], OsString::from("2222"));
        assert_eq!(spec.args[2], OsString::from("-tt"));
        assert_eq!(spec.args[3], OsString::from("--"));
        assert_eq!(spec.args[4], OsString::from("me@devbox"));
        let command = spec.args[5].to_string_lossy();
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

        assert_eq!(
            spec.args[..13],
            [
                OsString::from("-T"),
                OsString::from("-o"),
                OsString::from("ConnectTimeout=12"),
                OsString::from("-o"),
                OsString::from("ControlMaster=auto"),
                OsString::from("-o"),
                OsString::from("ControlPersist=10m"),
                OsString::from("-o"),
                OsString::from("ControlPath=/tmp/nucl-ssh/%C"),
                OsString::from("-J"),
                OsString::from("bastion"),
                OsString::from("-F"),
                OsString::from("/tmp/ssh config"),
            ]
        );
        assert_eq!(spec.args[13], OsString::from("--"));
        assert_eq!(spec.args[14], OsString::from("devbox"));
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
        assert_eq!(spec.args[1], OsString::from("-p"));
        assert_eq!(spec.args[2], OsString::from("2222"));
        assert_eq!(spec.args[3], OsString::from("--"));
        assert_eq!(spec.args[4], OsString::from("me@example.com"));
        let command = spec.args[5].to_string_lossy();
        assert!(command.contains("/remote/bin/nucleotide-remote"));
        assert!(command.contains("/home/me/project"));
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
        let mut options = RemoteWorkspaceBackendOptions::default();
        options.ssh_connect_timeout_secs = Some(4);
        options.ssh_control_path = None;
        options.ssh_extra_args = vec![OsString::from("-J"), OsString::from("bastion")];

        let spec = remote_service_command_for_location_with_options(
            &location,
            "/remote/bin/nucleotide-remote",
            &options,
        )
        .unwrap();

        assert_eq!(
            spec.args[..5],
            [
                OsString::from("-T"),
                OsString::from("-o"),
                OsString::from("ConnectTimeout=4"),
                OsString::from("-J"),
                OsString::from("bastion"),
            ]
        );
        assert_eq!(spec.args[5], OsString::from("--"));
        assert_eq!(spec.args[6], OsString::from("me@example.com"));
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
        assert_eq!(spec.args[1], OsString::from("-p"));
        assert_eq!(spec.args[2], OsString::from("2222"));
        assert_eq!(spec.args[3], OsString::from("--"));
        assert_eq!(spec.args[4], OsString::from("me@example.com"));
        let command = spec.args[5].to_string_lossy();
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
            "failed to connect to remote workspace service: unsupported response protocol version 1; expected 2"
        );

        assert!(remote_startup_error_can_retry_helper_install(
            &location, &error
        ));
    }

    #[test]
    fn startup_retry_is_limited_to_ssh_helper_failures() {
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
        let auth_error = anyhow::anyhow!("Permission denied (publickey)");
        let protocol_error = anyhow::anyhow!("unsupported response protocol version 1; expected 2");

        assert!(!remote_startup_error_can_retry_helper_install(
            &ssh_location,
            &auth_error
        ));
        assert!(!remote_startup_error_can_retry_helper_install(
            &local_location,
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
    fn ssh_probe_parser_accepts_shell_noise_and_platform_markers() {
        let probe = parse_ssh_probe_output(
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
    fn ssh_helper_cache_path_includes_protocol_version_and_platform() {
        let probe = SshRemoteProbe {
            platform: SshRemotePlatform {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
            cache_root: "/home/me/.cache".to_string(),
        };

        assert_eq!(
            ssh_remote_helper_path(&probe),
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
    fn remote_helper_download_urls_use_release_assets_and_checksums() {
        let mut options = RemoteWorkspaceBackendOptions::default();
        options.ssh_helper_download_base_url =
            Some("https://downloads.example/nucleotide/v1/".to_string());
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
    fn ssh_remote_helper_download_command_verifies_checksum_before_install() {
        let command = ssh_remote_helper_download_command(
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

    #[test]
    fn service_hello_returns_workspace_root_and_capabilities() {
        let temp = tempfile::tempdir().unwrap();
        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::Hello,
            Vec::new(),
        ));

        assert_eq!(frame.kind, FrameKind::Response);
        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::Hello(hello) = response.response else {
            panic!("expected hello response");
        };
        assert_eq!(hello.workspace_root, temp.path());
        assert!(
            hello
                .capabilities
                .contains(&"binary_body_frames".to_string())
        );
        assert!(hello.capabilities.contains(&"text_search".to_string()));
        assert!(
            hello
                .capabilities
                .contains(&"project_environment".to_string())
        );
        assert!(hello.capabilities.contains(&"git_head".to_string()));
        assert!(hello.capabilities.contains(&"git_status".to_string()));
    }

    #[test]
    fn service_read_file_returns_metadata_and_raw_body() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("main.rs"), "abcdef").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::ReadFile {
                path: PathBuf::from("main.rs"),
                max_bytes: Some(3),
            },
            Vec::new(),
        ));

        assert_eq!(frame.body, b"abc");
        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::ReadFile(read) = response.response else {
            panic!("expected read response");
        };
        assert_eq!(read.size, 6);
        assert!(read.truncated);
    }

    #[test]
    fn service_write_file_accepts_raw_body() {
        let temp = tempfile::tempdir().unwrap();
        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::WriteFile {
                path: PathBuf::from("src/main.rs"),
                create_parent_dirs: true,
                expected_modified_unix_millis: None,
            },
            b"fn main() {}\n".to_vec(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::WriteFile(write) = response.response else {
            panic!("expected write response");
        };
        assert_eq!(write.size, 13);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("src").join("main.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn service_rejects_absolute_paths_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside.txt");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(&outside, "secret").unwrap();

        let frame = read_first_output_frame(single_request_output(
            &workspace,
            RemoteRequest::ReadFile {
                path: outside,
                max_bytes: None,
            },
            Vec::new(),
        ));

        assert_eq!(frame.kind, FrameKind::Error);
        let error = frame.decode_json_header::<ErrorEnvelope>().unwrap();
        assert_eq!(error.error.code, "path_outside_workspace");
    }

    #[test]
    fn service_rejects_relative_parent_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside.txt");
        std::fs::create_dir(&workspace).unwrap();

        let frame = read_first_output_frame(single_request_output(
            &workspace,
            RemoteRequest::WriteFile {
                path: PathBuf::from("../outside.txt"),
                create_parent_dirs: false,
                expected_modified_unix_millis: None,
            },
            b"escaped".to_vec(),
        ));

        assert_eq!(frame.kind, FrameKind::Error);
        let error = frame.decode_json_header::<ErrorEnvelope>().unwrap();
        assert_eq!(error.error.code, "path_outside_workspace");
        assert!(!outside.exists());
    }

    #[test]
    fn service_find_ancestor_file_does_not_return_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let source = workspace.join("src").join("main.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(temp.path().join("Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(&source, "fn main() {}\n").unwrap();

        let frame = read_first_output_frame(single_request_output(
            &workspace,
            RemoteRequest::FindAncestorFile {
                start: source,
                file_name: "Cargo.toml".to_string(),
                limit: 8,
            },
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::FindAncestorFile(found) = response.response else {
            panic!("expected find ancestor response");
        };
        assert_eq!(found, None);
    }

    #[test]
    fn service_file_search_uses_workspace_root_for_empty_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src").join("main.rs"), "").unwrap();
        std::fs::write(temp.path().join("README.md"), "").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::FileSearch(FileSearchRequest {
                pattern: Some(r"\.rs$".to_string()),
                limit: 10,
                ..FileSearchRequest::default()
            }),
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::FileSearch(search) = response.response else {
            panic!("expected file search response");
        };
        assert_eq!(search.files, vec![PathBuf::from("src/main.rs")]);
        assert!(!search.truncated);
    }

    #[test]
    fn service_file_search_excludes_relative_prefixes_before_limit() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("skip")).unwrap();
        std::fs::write(temp.path().join("skip").join("a.rs"), "").unwrap();
        std::fs::write(temp.path().join("skip").join("b.rs"), "").unwrap();
        std::fs::write(temp.path().join("main.rs"), "").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::FileSearch(FileSearchRequest {
                limit: 1,
                excluded_relative_prefixes: vec![PathBuf::from("skip")],
                ..FileSearchRequest::default()
            }),
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::FileSearch(search) = response.response else {
            panic!("expected file search response");
        };
        assert_eq!(search.files, vec![PathBuf::from("main.rs")]);
        assert!(!search.truncated);
    }

    #[test]
    fn service_text_search_uses_workspace_root_for_empty_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src").join("main.rs"), "Needle\nneedle\n").unwrap();
        std::fs::write(temp.path().join("README.md"), "needle\n").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::TextSearch(TextSearchRequest {
                pattern: "needle".to_string(),
                limit: 1,
                smart_case: true,
                ..TextSearchRequest::default()
            }),
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::TextSearch(search) = response.response else {
            panic!("expected text search response");
        };
        assert_eq!(search.matches.len(), 1);
        assert!(search.truncated);
    }

    #[test]
    fn service_text_search_excludes_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src").join("main.rs"), "needle\n").unwrap();
        std::fs::write(temp.path().join("README.md"), "needle\n").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::TextSearch(TextSearchRequest {
                pattern: "needle".to_string(),
                limit: 10,
                excluded_relative_paths: vec![PathBuf::from("src/main.rs")],
                ..TextSearchRequest::default()
            }),
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::TextSearch(search) = response.response else {
            panic!("expected text search response");
        };
        assert_eq!(search.matches.len(), 1);
        assert_eq!(search.matches[0].relative_path, PathBuf::from("README.md"));
    }

    #[test]
    fn service_project_environment_returns_process_baseline() {
        let temp = tempfile::tempdir().unwrap();
        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::ProjectEnvironment {
                root: PathBuf::new(),
            },
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::ProjectEnvironment(snapshot) = response.response else {
            panic!("expected project environment response");
        };
        assert_eq!(snapshot.root, temp.path());
        assert_eq!(
            snapshot.origin,
            RemoteProjectEnvironmentOrigin::ProcessBaseline
        );
        assert_eq!(
            snapshot.variables.get("ZED_ENVIRONMENT"),
            Some(&"process-baseline".to_string())
        );
        assert!(snapshot.diagnostics.is_empty());
    }

    #[test]
    fn service_project_environment_reports_envrc_fallback_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".envrc"), "export FOO=bar\n").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::ProjectEnvironment {
                root: PathBuf::new(),
            },
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::ProjectEnvironment(snapshot) = response.response else {
            panic!("expected project environment response");
        };
        assert_eq!(
            snapshot.origin,
            RemoteProjectEnvironmentOrigin::ProcessBaseline
        );
        assert!(
            snapshot
                .diagnostics
                .iter()
                .any(|message| message.contains("Unsupported .envrc"))
        );
        assert!(!snapshot.variables.contains_key("FOO"));
    }

    #[test]
    fn service_git_status_uses_workspace_root_for_empty_root() {
        let temp = tempfile::tempdir().unwrap();
        init_git_repo(temp.path());
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);
        std::fs::write(temp.path().join("tracked.txt"), "changed\n").unwrap();
        std::fs::write(temp.path().join("notes.md"), "untracked\n").unwrap();

        let frame = read_first_output_frame(single_request_output(
            temp.path(),
            RemoteRequest::GitStatus {
                root: PathBuf::new(),
                include_untracked: true,
                limit: 10,
            },
            Vec::new(),
        ));

        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::GitStatus(status) = response.response else {
            panic!("expected git status response");
        };
        assert_eq!(status.root, temp.path());
        assert!(status.entries.iter().any(|entry| {
            entry.relative_path == PathBuf::from("tracked.txt")
                && entry.working_tree_status == RemoteGitStatusKind::Modified
        }));
        assert!(status.entries.iter().any(|entry| {
            entry.relative_path == PathBuf::from("notes.md")
                && entry.index_status == RemoteGitStatusKind::Untracked
        }));
        assert!(!status.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn service_project_environment_loads_native_flake_envrc() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".envrc"), "use flake\n").unwrap();

        let fake_bin = executable_tempdir();
        let fake_nix = fake_bin.path().join("nix");
        std::fs::write(
            &fake_nix,
            r#"#!/bin/sh
case "$*" in
  *"print-dev-env"*)
    printf '{"variables":{"PATH":{"type":"exported","value":"/nix/dev/bin"},"REMOTE_FLAG":{"type":"exported","value":"loaded"}}}\n'
    ;;
  *"profile wipe-history"*)
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_nix).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_nix, permissions).unwrap();
        if !generated_executable_is_allowed(&fake_nix) {
            return;
        }

        let baseline = HashMap::from([
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", fake_bin.path().display()),
            ),
            (
                "HOME".to_string(),
                temp.path().join("home").display().to_string(),
            ),
            (
                "NUCLEOTIDE_CACHE_DIR".to_string(),
                temp.path().join("cache").display().to_string(),
            ),
        ]);

        let request = request_frame(
            10,
            RemoteRequest::ProjectEnvironment {
                root: PathBuf::new(),
            },
            Vec::new(),
        );
        let mut input = Vec::new();
        write_frame(&mut input, &request).unwrap();

        let service = WorkspaceService::with_environment_baseline(
            LocalWorkspaceBackend,
            temp.path().to_path_buf(),
            baseline,
        )
        .unwrap();
        let mut output = Vec::new();
        service.serve(&mut Cursor::new(input), &mut output).unwrap();

        let frame = read_first_output_frame(output);
        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::ProjectEnvironment(snapshot) = response.response else {
            panic!("expected project environment response");
        };
        assert_eq!(snapshot.origin, RemoteProjectEnvironmentOrigin::NativeFlake);
        assert_eq!(
            snapshot.variables.get("REMOTE_FLAG"),
            Some(&"loaded".to_string())
        );
        assert_eq!(
            snapshot.variables.get("ZED_ENVIRONMENT"),
            Some(&"native-flake".to_string())
        );
        assert!(
            snapshot
                .variables
                .get("PATH")
                .is_some_and(|path| path.starts_with("/nix/dev/bin"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn service_run_process_can_inherit_project_environment() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".envrc"), "use flake\n").unwrap();

        let fake_bin = executable_tempdir();
        let fake_nix = fake_bin.path().join("nix");
        std::fs::write(
            &fake_nix,
            r#"#!/bin/sh
case "$*" in
  *"print-dev-env"*)
    printf '{"variables":{"PATH":{"type":"exported","value":"/nix/dev/bin"},"REMOTE_FLAG":{"type":"exported","value":"loaded"},"DEV_ONLY":{"type":"exported","value":"devshell"}}}\n'
    ;;
  *"profile wipe-history"*)
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_nix).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_nix, permissions).unwrap();
        if !generated_executable_is_allowed(&fake_nix) {
            return;
        }

        let baseline = HashMap::from([
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", fake_bin.path().display()),
            ),
            (
                "HOME".to_string(),
                temp.path().join("home").display().to_string(),
            ),
            (
                "NUCLEOTIDE_CACHE_DIR".to_string(),
                temp.path().join("cache").display().to_string(),
            ),
        ]);
        let request = request_frame(
            11,
            RemoteRequest::RunProcess(ProcessRequest {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "printf '%s:%s' \"$DEV_ONLY\" \"$REMOTE_FLAG\"".to_string(),
                ],
                cwd: PathBuf::new(),
                env: BTreeMap::from([("REMOTE_FLAG".to_string(), "override".to_string())]),
                clear_env: true,
                inherit_project_environment: true,
                max_output_bytes: None,
                timeout_ms: None,
            }),
            Vec::new(),
        );
        let mut input = Vec::new();
        write_frame(&mut input, &request).unwrap();

        let service = WorkspaceService::with_environment_baseline(
            LocalWorkspaceBackend,
            temp.path().to_path_buf(),
            baseline,
        )
        .unwrap();
        let mut output = Vec::new();
        service.serve(&mut Cursor::new(input), &mut output).unwrap();

        let frame = read_first_output_frame(output);
        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::RunProcess(process) = response.response else {
            panic!("expected run process response");
        };
        assert!(process.success);
        assert_eq!(process.stdout_len, "devshell:override".len());
        assert_eq!(&frame.body[..process.stdout_len], b"devshell:override");
    }

    #[cfg(unix)]
    #[test]
    fn service_run_process_uses_workspace_envrc_for_nested_cwd() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("crates").join("app");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(temp.path().join(".envrc"), "use flake\n").unwrap();

        let fake_bin = executable_tempdir();
        let fake_nix = fake_bin.path().join("nix");
        std::fs::write(
            &fake_nix,
            r#"#!/bin/sh
case "$*" in
  *"print-dev-env"*)
    printf '{"variables":{"PATH":{"type":"exported","value":"/nix/dev/bin"},"DEV_ONLY":{"type":"exported","value":"nested-devshell"}}}\n'
    ;;
  *"profile wipe-history"*)
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_nix).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_nix, permissions).unwrap();
        if !generated_executable_is_allowed(&fake_nix) {
            return;
        }

        let baseline = HashMap::from([
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", fake_bin.path().display()),
            ),
            (
                "HOME".to_string(),
                temp.path().join("home").display().to_string(),
            ),
            (
                "NUCLEOTIDE_CACHE_DIR".to_string(),
                temp.path().join("cache").display().to_string(),
            ),
        ]);
        let request = request_frame(
            12,
            RemoteRequest::RunProcess(ProcessRequest {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "printf '%s:%s' \"$DEV_ONLY\" \"$(pwd -P)\"".to_string(),
                ],
                cwd: nested.strip_prefix(temp.path()).unwrap().to_path_buf(),
                env: BTreeMap::new(),
                clear_env: true,
                inherit_project_environment: true,
                max_output_bytes: None,
                timeout_ms: None,
            }),
            Vec::new(),
        );
        let mut input = Vec::new();
        write_frame(&mut input, &request).unwrap();

        let service = WorkspaceService::with_environment_baseline(
            LocalWorkspaceBackend,
            temp.path().to_path_buf(),
            baseline,
        )
        .unwrap();
        let mut output = Vec::new();
        service.serve(&mut Cursor::new(input), &mut output).unwrap();

        let frame = read_first_output_frame(output);
        let response = frame.decode_json_header::<ResponseEnvelope>().unwrap();
        let RemoteResponse::RunProcess(process) = response.response else {
            panic!("expected run process response");
        };
        let expected = format!(
            "nested-devshell:{}",
            nested.canonicalize().unwrap().display()
        );
        assert!(process.success);
        assert_eq!(process.stdout_len, expected.len());
        assert_eq!(&frame.body[..process.stdout_len], expected.as_bytes());
    }

    #[test]
    fn service_reports_protocol_mismatch_as_error_frame() {
        let temp = tempfile::tempdir().unwrap();
        let request = RequestEnvelope {
            protocol_version: PROTOCOL_VERSION + 1,
            request: RemoteRequest::Hello,
        };
        let frame =
            Frame::from_json_header(FrameKind::Request, 9, 0, &request, Vec::new()).unwrap();
        let mut input = Vec::new();
        write_frame(&mut input, &frame).unwrap();

        let service =
            WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();
        service.serve(&mut reader, &mut output).unwrap();

        let frame = read_first_output_frame(output);
        assert_eq!(frame.kind, FrameKind::Error);
        let error = frame.decode_json_header::<ErrorEnvelope>().unwrap();
        assert_eq!(error.error.code, "protocol_mismatch");
    }

    #[test]
    fn remote_workspace_backend_reads_files_through_service() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("main.rs"), "abcdef").unwrap();
        let backend = remote_backend(temp.path());

        let read =
            block_on(backend.read_file(Path::new("main.rs"), ReadOptions { max_bytes: Some(4) }))
                .unwrap();

        assert_eq!(read.bytes, b"abcd");
        assert_eq!(read.size, 6);
        assert!(read.truncated);
    }

    #[test]
    fn remote_workspace_backend_writes_files_through_service() {
        let temp = tempfile::tempdir().unwrap();
        let backend = remote_backend(temp.path());

        let result = block_on(backend.write_file(
            Path::new("src/main.rs"),
            b"fn main() {}\n",
            WriteOptions {
                create_parent_dirs: true,
                expected_modified: None,
            },
        ))
        .unwrap();

        assert_eq!(result.size, 13);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("src").join("main.rs")).unwrap(),
            "fn main() {}\n"
        );
    }

    #[test]
    fn remote_workspace_backend_maps_modified_errors() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("main.rs"), "old").unwrap();
        let backend = remote_backend(temp.path());

        let result = block_on(backend.write_file(
            Path::new("main.rs"),
            b"new",
            WriteOptions {
                create_parent_dirs: false,
                expected_modified: Some(UNIX_EPOCH),
            },
        ));

        assert!(matches!(result, Err(WorkspaceError::Modified { .. })));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("main.rs")).unwrap(),
            "old"
        );
    }

    #[test]
    fn remote_workspace_backend_loads_project_environment_through_service() {
        let temp = tempfile::tempdir().unwrap();
        let backend = remote_backend(temp.path());

        let snapshot = block_on(backend.project_environment(Path::new(""))).unwrap();

        assert_eq!(snapshot.root, temp.path());
        assert_eq!(snapshot.origin, ProjectEnvironmentOrigin::ProcessBaseline);
        assert_eq!(
            snapshot.variables.get("ZED_ENVIRONMENT"),
            Some(&"process-baseline".to_string())
        );
    }

    #[test]
    fn remote_workspace_backend_marks_ignored_directory_entries() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".gitignore"), "ignored.log\n").unwrap();
        std::fs::write(temp.path().join("visible.rs"), "").unwrap();
        std::fs::write(temp.path().join("ignored.log"), "").unwrap();
        let backend = remote_backend(temp.path());

        let listing = block_on(backend.list_dir(Path::new(""))).unwrap();

        let visible = listing
            .entries
            .iter()
            .find(|entry| entry.name == "visible.rs")
            .expect("visible entry");
        let ignored = listing
            .entries
            .iter()
            .find(|entry| entry.name == "ignored.log")
            .expect("ignored entry");
        assert_eq!(visible.ignored, Some(false));
        assert_eq!(ignored.ignored, Some(true));
    }

    #[test]
    fn remote_workspace_backend_runs_text_search_through_service() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("main.rs"), "needle\n").unwrap();
        let backend = remote_backend(temp.path());

        let result = block_on(backend.text_search(TextSearchQuery {
            root: PathBuf::new(),
            pattern: "needle".to_string(),
            limit: 10,
            ..TextSearchQuery::default()
        }))
        .unwrap();

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].relative_path, PathBuf::from("main.rs"));
        assert_eq!(result.matches[0].line_number, 1);
    }

    #[test]
    fn remote_workspace_backend_reads_git_head_and_status_through_service() {
        let temp = tempfile::tempdir().unwrap();
        init_git_repo(temp.path());
        std::fs::write(temp.path().join("tracked.txt"), "initial\n").unwrap();
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);
        let expected_head = git_output(temp.path(), &["rev-parse", "--verify", "HEAD"]);
        std::fs::write(temp.path().join("tracked.txt"), "changed\n").unwrap();

        let backend = remote_backend(temp.path());
        let head = block_on(backend.git_head(Path::new(""))).unwrap();
        let status =
            block_on(backend.git_status(Path::new(""), GitStatusOptions::default())).unwrap();

        assert_eq!(head.head, Some(expected_head.trim().to_string()));
        assert!(status.entries.iter().any(|entry| {
            entry.relative_path == PathBuf::from("tracked.txt")
                && entry.working_tree_status == GitStatusKind::Modified
        }));
    }

    fn init_git_repo(root: &Path) {
        run_git(root, &["init"]);
        run_git(root, &["config", "user.email", "nucleotide@example.test"]);
        run_git(root, &["config", "user.name", "Nucleotide Tests"]);
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}
