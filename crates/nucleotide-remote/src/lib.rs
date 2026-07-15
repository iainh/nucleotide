// ABOUTME: Framed stdio protocol and service loop for Nucleotide remote workspaces
// ABOUTME: Keeps WSL, SSH, and local service transports on one request model

mod backend;
mod bootstrap;
mod cli;
mod client;
mod command;
mod connection;
pub mod protocol_v5;
mod proxy;
mod reconnect;
mod rpc;
mod service;
mod v5_budget;

pub use backend::*;
pub use bootstrap::*;
pub use cli::*;
pub use client::*;
pub(crate) use command::*;
pub use command::{
    RemoteServiceCommand, SshTarget, local_service_command, ssh_interactive_terminal_command,
    ssh_lsp_proxy_command, ssh_service_command, ssh_terminal_proxy_command,
    wsl_interactive_terminal_command, wsl_lsp_proxy_command, wsl_service_command,
    wsl_terminal_proxy_command,
};
pub use connection::*;
pub(crate) use proxy::*;
pub use reconnect::*;
pub use rpc::*;
pub use service::*;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::{Stream, StreamExt, channel::oneshot, executor::block_on, task::AtomicWaker};
use ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use notify::Watcher as _;
use nucleotide_env::{EnvironmentOrigin, ProjectEnvironment, ShellEnvironmentError};
use nucleotide_workspace::{
    DirectoryListing, FileKind, FileRead, FileReadEvent, FileReadMetadata, FileReadStream,
    FileSearchEvent, FileSearchQuery, FileSearchResult, FileSearchStream, FileStat, GitHeadResult,
    GitStatusEntry, GitStatusKind, GitStatusOptions, GitStatusResult, LocalWorkspaceBackend,
    ProcessCompletion, ProcessEvent, ProcessOutput, ProcessSpec, ProcessStream,
    ProjectEnvironmentOrigin, ProjectEnvironmentSnapshot, ReadOptions, RemoteWorkspaceIdentity,
    RemoteWorkspaceKind, SshWorkspaceTarget, TextSearchEvent, TextSearchMatch, TextSearchQuery,
    TextSearchResult, TextSearchStream, WorkspaceBackend, WorkspaceBackendHandle,
    WorkspaceCancellationToken, WorkspaceError, WorkspaceIdentity, WorkspaceLocation,
    WorkspaceWatch, WorkspaceWatchBatch, WorkspaceWatchChange, WorkspaceWatchChangeKind,
    WorkspaceWatchDirectoryGeneration, WorkspaceWatchRequest, WorkspaceWatchUpdate, WriteOptions,
    WriteResult, local_workspace_backend, path_mapped_workspace_backend, posix_path_string,
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
use std::pin::Pin;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{
    Arc, Condvar, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use v5_budget::{V5Budgeted, V5ByteReservation, V5ConnectionByteBudget};

pub const PROTOCOL_VERSION: u32 = protocol_v5::PROTOCOL_MAJOR;
pub const FRAME_VERSION: u16 = protocol_v5::FRAME_HEADER_VERSION;
pub const MAX_FRAME_BODY_LEN: u64 = protocol_v5::MAX_NEGOTIATED_FRAME_BODY_LEN as u64;
pub const DEFAULT_SSH_CONNECT_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_REMOTE_STARTUP_TIMEOUT: Duration = Duration::from_secs(5 * 60);
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
const V5_FILE_STREAM_CHUNK_TARGET_BYTES: usize = 64 * 1024;
const V5_FILE_STREAM_MAX_QUEUED_CHUNKS: usize = 256;
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
const V5_MAX_STREAMED_FILE_READ_BYTES: u64 = 256 * 1024 * 1024;
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

#[cfg(test)]
mod tests;
