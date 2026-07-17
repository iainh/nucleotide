// ABOUTME: Remote workspace RPC requests, responses, codecs, and stream-facing data types
// ABOUTME: Maps typed workspace operations to protocol v5 methods and payloads

use super::*;

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_version: Option<Vec<u8>>,
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

pub(crate) type RemoteRequestCancellationCallback = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone, Default)]
pub struct RemoteRequestCancellation {
    inner: Arc<RemoteRequestCancellationInner>,
}

#[derive(Default)]
pub(crate) struct RemoteRequestCancellationInner {
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

    pub(crate) fn register(&self, callback: impl FnOnce() + Send + 'static) {
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

    pub(crate) fn check_cancelled(
        &self,
        method: &str,
    ) -> std::result::Result<(), RemoteClientError> {
        if self.is_cancelled() {
            Err(remote_request_cancelled_error(method))
        } else {
            Ok(())
        }
    }
}

pub(crate) struct RemoteRequestCancelOnDrop {
    cancellation: Option<RemoteRequestCancellation>,
}

impl RemoteRequestCancelOnDrop {
    pub(crate) fn new() -> Self {
        Self {
            cancellation: Some(RemoteRequestCancellation::new()),
        }
    }

    pub(crate) fn cancellation(&self) -> RemoteRequestCancellation {
        self.cancellation.clone().unwrap_or_default()
    }

    pub(crate) fn disarm(&mut self) {
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
pub(crate) struct RemoteRequestDeadlinePolicy {
    pub(crate) absolute_timeout: Option<Duration>,
    pub(crate) inactivity_timeout: Option<Duration>,
}

impl RemoteRequestDeadlinePolicy {
    pub(crate) const fn bounded(absolute_timeout: Duration, inactivity_timeout: Duration) -> Self {
        Self {
            absolute_timeout: Some(absolute_timeout),
            inactivity_timeout: Some(inactivity_timeout),
        }
    }

    pub(crate) const fn absolute_only(absolute_timeout: Duration) -> Self {
        Self {
            absolute_timeout: Some(absolute_timeout),
            inactivity_timeout: None,
        }
    }

    pub(crate) const fn unlimited() -> Self {
        Self {
            absolute_timeout: None,
            inactivity_timeout: None,
        }
    }
}

impl RemoteRequestContext {
    pub(crate) fn from_policy(policy: RemoteRequestDeadlinePolicy) -> Self {
        Self::from_policy_at(policy, Instant::now(), v5_now_unix_millis())
    }

    pub(crate) fn from_policy_at(
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

    pub(crate) fn expired_at(self, now: Instant) -> Option<RemoteRequestDeadlineKind> {
        self.absolute_deadline
            .filter(|deadline| now >= *deadline)
            .map(|_| RemoteRequestDeadlineKind::Absolute)
    }
}

pub(crate) fn v5_watch_control_request_context() -> RemoteRequestContext {
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

    pub(crate) fn v5_deadline_policy(&self) -> RemoteRequestDeadlinePolicy {
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

    pub(crate) fn v5_request_options_with_context(
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
                    expected_version: payload.expected_version,
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
                expected_version,
                expected_modified_unix_millis,
                expected_modified_unix_nanos,
            } => V5RequestPayloadRef::WriteFile {
                path,
                create_parent_dirs: *create_parent_dirs,
                expected_version: expected_version.as_deref(),
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
pub(crate) enum V5RequestPayloadRef<'a> {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_version: Option<&'a [u8]>,
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
pub(crate) struct V5DirectoryListPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) path: PathBuf,
    #[serde(default)]
    pub(crate) known_generation: Option<u64>,
    #[serde(default)]
    pub(crate) known_fingerprint: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5DirectoryListEntryPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) path: PathBuf,
    #[serde(default)]
    pub(crate) known_generation: Option<u64>,
    #[serde(default)]
    pub(crate) known_fingerprint: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5DirectoryListDirsPayload {
    #[serde(default, serialize_with = "serialize_posix_paths")]
    pub(crate) paths: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) entries: Vec<V5DirectoryListEntryPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5PathPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5PathsPayload {
    #[serde(serialize_with = "serialize_posix_paths")]
    pub(crate) paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5FindAncestorPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) start: PathBuf,
    pub(crate) file_name: String,
    pub(crate) limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5RenamePayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) from: PathBuf,
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5ReadFilePayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) path: PathBuf,
    pub(crate) max_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5WriteFilePayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) path: PathBuf,
    pub(crate) create_parent_dirs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expected_version: Option<Vec<u8>>,
    pub(crate) expected_modified_unix_millis: Option<i64>,
    #[serde(default)]
    pub(crate) expected_modified_unix_nanos: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5RootPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct V5GitStatusPayload {
    #[serde(serialize_with = "serialize_posix_path")]
    pub(crate) root: PathBuf,
    pub(crate) include_untracked: bool,
    pub(crate) limit: usize,
}

pub(crate) fn serialize_posix_path<S, P>(path: &P, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    P: AsRef<Path>,
{
    serializer.serialize_str(&posix_path_string(path.as_ref()))
}

pub(crate) fn serialize_posix_paths<S, P>(paths: &P, serializer: S) -> Result<S::Ok, S::Error>
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

pub(crate) fn encode_v5_payload(
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

pub(crate) fn encode_v5_json_payload<T>(
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

pub(crate) fn decode_v5_payload<T>(
    method: &str,
    payload: &[u8],
) -> std::result::Result<T, V5MethodError>
where
    T: DeserializeOwned,
{
    let payload = if payload.is_empty() { b"{}" } else { payload };
    serde_json::from_slice(payload).map_err(|error| V5MethodError::InvalidPayload {
        method: method.to_string(),
        error: error.to_string(),
    })
}

pub(crate) fn decode_empty_v5_payload(
    method: &str,
    payload: &[u8],
) -> std::result::Result<(), V5MethodError> {
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

pub(crate) fn decode_v5_protobuf_payload<T>(
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

pub(crate) fn validate_v5_watch_start(
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<Vec<u8>>,
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

pub(crate) fn default_true() -> bool {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<Vec<u8>>,
    pub modified_unix_millis: Option<i64>,
    #[serde(default)]
    pub modified_unix_nanos: Option<u32>,
    pub readonly: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFileReadEvent {
    Chunk(Vec<u8>),
    Complete(FileReadResponse),
}

pub(crate) type RemoteTerminalPredicate<E> = Box<dyn Fn(&E) -> bool + Send + Sync + 'static>;

#[must_use = "dropping a live remote event stream cancels its request"]
pub struct RemoteEventStream<E> {
    inner: Pin<Box<dyn Stream<Item = std::result::Result<E, RemoteClientError>> + Send + 'static>>,
    cancellation: Option<RemoteRequestCancellation>,
    terminal_error: Option<Box<dyn FnOnce() + Send + 'static>>,
    terminal_on_ok: Option<RemoteTerminalPredicate<E>>,
    finished: bool,
}

impl<E> RemoteEventStream<E> {
    pub(crate) fn new(
        stream: impl Stream<Item = std::result::Result<E, RemoteClientError>> + Send + 'static,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            cancellation: None,
            terminal_error: None,
            terminal_on_ok: None,
            finished: false,
        }
    }

    pub(crate) fn with_cancellation(mut self, cancellation: RemoteRequestCancellation) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub(crate) fn with_terminal_error_callback(
        mut self,
        callback: impl FnOnce() + Send + 'static,
    ) -> Self {
        self.terminal_error = Some(Box::new(callback));
        self
    }

    pub(crate) fn with_terminal_predicate(
        mut self,
        predicate: impl Fn(&E) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.terminal_on_ok = Some(Box::new(predicate));
        self
    }
}

pub type RemoteFileReadStream = RemoteEventStream<RemoteFileReadEvent>;

impl RemoteEventStream<RemoteFileReadEvent> {
    pub(crate) fn from_response(
        response: FileReadResponse,
        body: Vec<u8>,
    ) -> std::result::Result<Self, RemoteClientError> {
        validate_file_read_body(&response, body.len())?;
        let mut events = Vec::with_capacity(usize::from(!body.is_empty()) + 1);
        if !body.is_empty() {
            events.push(Ok(RemoteFileReadEvent::Chunk(body)));
        }
        events.push(Ok(RemoteFileReadEvent::Complete(response)));
        Ok(Self::new(futures::stream::iter(events))
            .with_terminal_predicate(|event| matches!(event, RemoteFileReadEvent::Complete(_))))
    }
}

impl<E> Stream for RemoteEventStream<E> {
    type Item = std::result::Result<E, RemoteClientError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }
        match self.inner.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(event))) => {
                if self
                    .terminal_on_ok
                    .as_ref()
                    .is_some_and(|predicate| predicate(&event))
                {
                    self.finished = true;
                    self.cancellation = None;
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.finished = true;
                self.cancellation = None;
                if let Some(callback) = self.terminal_error.take() {
                    callback();
                }
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.finished = true;
                self.cancellation = None;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<E> Drop for RemoteEventStream<E> {
    fn drop(&mut self) {
        if !self.finished
            && let Some(cancellation) = self.cancellation.take()
        {
            cancellation.cancel();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteResultResponse {
    pub path: PathBuf,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<Vec<u8>>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFileSearchEvent {
    Batch(Vec<PathBuf>),
    Complete { root: PathBuf, truncated: bool },
}

pub type RemoteFileSearchStream = RemoteEventStream<RemoteFileSearchEvent>;

impl RemoteEventStream<RemoteFileSearchEvent> {
    pub(crate) fn from_response(response: FileSearchResponse) -> Self {
        let FileSearchResponse {
            root,
            files,
            truncated,
        } = response;
        let mut events = Vec::with_capacity(usize::from(!files.is_empty()) + 1);
        if !files.is_empty() {
            events.push(Ok(RemoteFileSearchEvent::Batch(files)));
        }
        events.push(Ok(RemoteFileSearchEvent::Complete { root, truncated }));
        Self::new(futures::stream::iter(events)).with_terminal_predicate(|event| {
            matches!(event, RemoteFileSearchEvent::Complete { .. })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteTextSearchEvent {
    Batch(Vec<TextSearchMatchResponse>),
    Complete { root: PathBuf, truncated: bool },
}

pub type RemoteTextSearchStream = RemoteEventStream<RemoteTextSearchEvent>;

impl RemoteEventStream<RemoteTextSearchEvent> {
    pub(crate) fn from_response(response: TextSearchResponse) -> Self {
        let TextSearchResponse {
            root,
            matches,
            truncated,
        } = response;
        let mut events = Vec::with_capacity(usize::from(!matches.is_empty()) + 1);
        if !matches.is_empty() {
            events.push(Ok(RemoteTextSearchEvent::Batch(matches)));
        }
        events.push(Ok(RemoteTextSearchEvent::Complete { root, truncated }));
        Self::new(futures::stream::iter(events)).with_terminal_predicate(|event| {
            matches!(event, RemoteTextSearchEvent::Complete { .. })
        })
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteProcessEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Complete(ProcessOutputResponse),
}

pub type RemoteProcessStream = RemoteEventStream<RemoteProcessEvent>;

impl RemoteEventStream<RemoteProcessEvent> {
    pub(crate) fn from_response(
        response: ProcessOutputResponse,
        body: Vec<u8>,
    ) -> std::result::Result<Self, RemoteClientError> {
        let stdout_len = response.stdout_len;
        let stderr_len = response.stderr_len;
        let output = process_output_from_response(response, body)?;
        let ProcessOutput {
            status_code,
            success,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            timed_out,
        } = output;
        let mut events = Vec::with_capacity(
            usize::from(!stdout.is_empty()) + usize::from(!stderr.is_empty()) + 1,
        );
        if !stdout.is_empty() {
            events.push(Ok(RemoteProcessEvent::Stdout(stdout)));
        }
        if !stderr.is_empty() {
            events.push(Ok(RemoteProcessEvent::Stderr(stderr)));
        }
        events.push(Ok(RemoteProcessEvent::Complete(ProcessOutputResponse {
            status_code,
            success,
            stdout_truncated,
            stderr_truncated,
            stdout_len,
            stderr_len,
            timed_out,
        })));
        Ok(Self::new(futures::stream::iter(events))
            .with_terminal_predicate(|event| matches!(event, RemoteProcessEvent::Complete(_))))
    }
}
