// ABOUTME: Remote helper discovery, installation, startup, and connection orchestration
// ABOUTME: Owns backend options, progress reporting, and transport-specific bootstrap policy

use super::*;

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

pub(crate) fn hello_response_from_v5_server_hello(
    hello: &protocol_v5::ServerHello,
) -> HelloResponse {
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
pub(crate) struct RemoteWorkspaceBackendEnvironment {
    pub(crate) remote_helper_path: Option<OsString>,
    pub(crate) local_helper_path: Option<OsString>,
    pub(crate) ssh_helper_upload_path: Option<OsString>,
    pub(crate) ssh_helper_artifact_dir: Option<OsString>,
    pub(crate) ssh_helper_download_base_url: Option<String>,
    pub(crate) ssh_helper_install_policy: Option<String>,
    pub(crate) ssh_connect_timeout_secs: Option<String>,
    pub(crate) ssh_extra_args: Option<OsString>,
    pub(crate) ssh_control_master: Option<String>,
    pub(crate) ssh_control_path: Option<OsString>,
    pub(crate) use_local_service: bool,
    pub(crate) current_exe: Option<PathBuf>,
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
    pub(crate) fn from_environment_values(values: RemoteWorkspaceBackendEnvironment) -> Self {
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
pub(crate) struct SshRemoteProbe {
    pub(crate) platform: SshRemotePlatform,
    pub(crate) cache_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RemoteBootstrapTransportKey {
    Ssh {
        host: String,
        user: Option<String>,
        port: Option<u16>,
        connect_timeout_secs: Option<u64>,
        extra_args: Vec<OsString>,
        control_path: Option<PathBuf>,
    },
    Wsl {
        distro: String,
    },
}

impl RemoteBootstrapTransportKey {
    pub(crate) fn from_location(
        location: &WorkspaceLocation,
        options: &RemoteWorkspaceBackendOptions,
    ) -> Option<Self> {
        match location {
            WorkspaceLocation::Ssh { target, .. } => {
                Some(Self::from_transport(LinuxHelperTransport::Ssh(
                    &ssh_target_from_workspace_target_with_options(target, options),
                )))
            }
            WorkspaceLocation::Wsl { distro, .. } => Some(Self::Wsl {
                distro: distro.clone(),
            }),
            WorkspaceLocation::Local { .. } => None,
        }
    }

    fn from_transport(transport: LinuxHelperTransport<'_>) -> Self {
        match transport {
            LinuxHelperTransport::Ssh(target) => Self::Ssh {
                host: target.host.clone(),
                user: target.user.clone(),
                port: target.port,
                connect_timeout_secs: target.connect_timeout_secs,
                extra_args: target.extra_args.clone(),
                control_path: target.control_path.clone(),
            },
            LinuxHelperTransport::Wsl(distro) => Self::Wsl {
                distro: distro.to_string(),
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum LinuxHelperTransport<'a> {
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

#[derive(Debug)]
pub struct RemoteStartupCancelled;

impl fmt::Display for RemoteStartupCancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("remote workspace startup cancelled")
    }
}

impl Error for RemoteStartupCancelled {}

#[derive(Debug)]
pub struct RemoteStartupDeadlineExceeded;

impl fmt::Display for RemoteStartupDeadlineExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("remote workspace startup deadline exceeded")
    }
}

impl Error for RemoteStartupDeadlineExceeded {}

pub fn remote_startup_was_cancelled(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<RemoteStartupCancelled>().is_some())
}

pub fn remote_startup_deadline_was_exceeded(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<RemoteStartupDeadlineExceeded>()
            .is_some()
    })
}

/// Shared cancellation and monotonic deadline for one complete remote startup attempt.
#[derive(Clone, Debug)]
pub struct RemoteStartupContext {
    cancellation: WorkspaceCancellationToken,
    started: Instant,
    timeout: Duration,
}

impl RemoteStartupContext {
    pub fn new(timeout: Duration) -> Self {
        Self::with_cancellation(WorkspaceCancellationToken::new(), timeout)
    }

    pub fn with_cancellation(cancellation: WorkspaceCancellationToken, timeout: Duration) -> Self {
        Self {
            cancellation,
            started: Instant::now(),
            timeout,
        }
    }

    pub fn cancellation(&self) -> &WorkspaceCancellationToken {
        &self.cancellation
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn check(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            Err(RemoteStartupCancelled.into())
        } else if self.started.elapsed() >= self.timeout {
            Err(RemoteStartupDeadlineExceeded.into())
        } else {
            Ok(())
        }
    }

    pub fn remaining(&self) -> Result<Duration> {
        self.check()?;
        Ok(self.timeout.saturating_sub(self.started.elapsed()))
    }

    pub fn cap_timeout(&self, stage_timeout: Duration) -> Result<Duration> {
        Ok(stage_timeout.min(self.remaining()?))
    }
}

/// Owns a startup context and cancels it unless the accepted attempt is explicitly disarmed.
#[derive(Debug)]
pub struct RemoteStartupAttempt {
    context: RemoteStartupContext,
    armed: bool,
}

impl RemoteStartupAttempt {
    pub fn new(timeout: Duration) -> Self {
        Self {
            context: RemoteStartupContext::new(timeout),
            armed: true,
        }
    }

    pub fn context(&self) -> RemoteStartupContext {
        self.context.clone()
    }

    pub fn cancel(&mut self) {
        self.context.cancel();
        self.armed = false;
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RemoteStartupAttempt {
    fn drop(&mut self) {
        if self.armed {
            self.context.cancel();
        }
    }
}

pub(crate) const REMOTE_BOOTSTRAP_CACHE_CAPACITY: usize = 32;
pub(crate) const REMOTE_BOOTSTRAP_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
pub(crate) const REMOTE_BOOTSTRAP_WAIT_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteSingleFlightKind {
    Load,
    Refresh,
}

pub(crate) enum RemoteSingleFlightEntry<V> {
    Resolving {
        flight_id: u64,
        kind: RemoteSingleFlightKind,
    },
    Ready {
        value: V,
        generation: u64,
        kind: RemoteSingleFlightKind,
        expires_at: Instant,
    },
}

pub(crate) enum RemoteSingleFlightLoad<V> {
    Cache(V),
    Uncached(V),
}

impl<V> RemoteSingleFlightLoad<V> {
    fn into_value(self) -> V {
        match self {
            Self::Cache(value) | Self::Uncached(value) => value,
        }
    }
}

pub(crate) struct RemoteSingleFlightLookup<V> {
    pub(crate) value: V,
    pub(crate) generation: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum RemoteSingleFlightRequest {
    Load,
    Refresh { observed_generation: u64 },
}

impl RemoteSingleFlightRequest {
    fn kind(self) -> RemoteSingleFlightKind {
        match self {
            Self::Load => RemoteSingleFlightKind::Load,
            Self::Refresh { .. } => RemoteSingleFlightKind::Refresh,
        }
    }

    fn can_use_ready(self, kind: RemoteSingleFlightKind, generation: u64) -> bool {
        match self {
            Self::Load => true,
            Self::Refresh {
                observed_generation,
            } => kind == RemoteSingleFlightKind::Refresh && generation != observed_generation,
        }
    }
}

pub(crate) struct RemoteSingleFlightState<K, V> {
    pub(crate) entries: HashMap<K, RemoteSingleFlightEntry<V>>,
    ready_order: VecDeque<K>,
    next_flight_id: u64,
}

impl<K, V> RemoteSingleFlightState<K, V>
where
    K: Clone + Eq + Hash,
{
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            ready_order: VecDeque::new(),
            next_flight_id: 1,
        }
    }

    fn remove_expired(&mut self, now: Instant) {
        let expired = self
            .ready_order
            .iter()
            .filter(|key| {
                matches!(
                    self.entries.get(*key),
                    Some(RemoteSingleFlightEntry::Ready { expires_at, .. })
                        if *expires_at <= now
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in expired {
            self.entries.remove(&key);
            self.ready_order.retain(|cached| cached != &key);
        }
    }

    fn evict_oldest_ready(&mut self) -> bool {
        while let Some(key) = self.ready_order.pop_front() {
            if matches!(
                self.entries.get(&key),
                Some(RemoteSingleFlightEntry::Ready { .. })
            ) {
                self.entries.remove(&key);
                return true;
            }
        }
        false
    }

    fn allocate_flight_id(&mut self) -> u64 {
        let flight_id = self.next_flight_id;
        self.next_flight_id = self.next_flight_id.wrapping_add(1).max(1);
        flight_id
    }
}

pub(crate) struct RemoteSingleFlightCache<K, V> {
    state: Mutex<RemoteSingleFlightState<K, V>>,
    wake: Condvar,
    capacity: usize,
    ttl: Duration,
}

impl<K, V> RemoteSingleFlightCache<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    pub(crate) fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            state: Mutex::new(RemoteSingleFlightState::new()),
            wake: Condvar::new(),
            capacity: capacity.max(1),
            ttl,
        }
    }

    pub(crate) fn get_or_try_init(
        &self,
        key: K,
        startup: &RemoteStartupContext,
        initialize: impl FnOnce() -> Result<V>,
    ) -> Result<V> {
        self.get_or_try_init_controlled(key, startup, || {
            initialize().map(RemoteSingleFlightLoad::Cache)
        })
        .map(|lookup| lookup.value)
    }

    pub(crate) fn get_or_try_init_controlled(
        &self,
        key: K,
        startup: &RemoteStartupContext,
        initialize: impl FnOnce() -> Result<RemoteSingleFlightLoad<V>>,
    ) -> Result<RemoteSingleFlightLookup<V>> {
        self.get_or_try_init_for_request(key, startup, RemoteSingleFlightRequest::Load, initialize)
    }

    pub(crate) fn refresh_after(
        &self,
        key: K,
        observed_generation: u64,
        startup: &RemoteStartupContext,
        initialize: impl FnOnce() -> Result<V>,
    ) -> Result<RemoteSingleFlightLookup<V>> {
        self.get_or_try_init_for_request(
            key,
            startup,
            RemoteSingleFlightRequest::Refresh {
                observed_generation,
            },
            || initialize().map(RemoteSingleFlightLoad::Cache),
        )
    }

    fn get_or_try_init_for_request(
        &self,
        key: K,
        startup: &RemoteStartupContext,
        request: RemoteSingleFlightRequest,
        initialize: impl FnOnce() -> Result<RemoteSingleFlightLoad<V>>,
    ) -> Result<RemoteSingleFlightLookup<V>> {
        let mut initialize = Some(initialize);
        loop {
            startup.check()?;
            let mut state = self.lock_state();
            let now = Instant::now();
            state.remove_expired(now);

            if let Some(RemoteSingleFlightEntry::Ready {
                value,
                generation,
                kind,
                ..
            }) = state.entries.get(&key)
            {
                if request.can_use_ready(*kind, *generation) {
                    let value = value.clone();
                    let generation = *generation;
                    state.ready_order.retain(|cached| cached != &key);
                    state.ready_order.push_back(key.clone());
                    drop(state);
                    startup.check()?;
                    return Ok(RemoteSingleFlightLookup { value, generation });
                }
                state.entries.remove(&key);
                state.ready_order.retain(|cached| cached != &key);
            }

            if matches!(
                state.entries.get(&key),
                Some(RemoteSingleFlightEntry::Resolving { .. })
            ) {
                let wait = startup.cap_timeout(REMOTE_BOOTSTRAP_WAIT_POLL)?;
                let (state, _) = self
                    .wake
                    .wait_timeout(state, wait)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                drop(state);
                continue;
            }

            while state.entries.len() >= self.capacity && state.evict_oldest_ready() {}
            if state.entries.len() >= self.capacity {
                let wait = startup.cap_timeout(REMOTE_BOOTSTRAP_WAIT_POLL)?;
                let (state, _) = self
                    .wake
                    .wait_timeout(state, wait)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                drop(state);
                continue;
            }

            let flight_id = state.allocate_flight_id();
            state.entries.insert(
                key.clone(),
                RemoteSingleFlightEntry::Resolving {
                    flight_id,
                    kind: request.kind(),
                },
            );
            drop(state);

            let mut leader = RemoteSingleFlightLeader::new(self, key, flight_id, request.kind());
            let initialize = initialize
                .take()
                .expect("bootstrap initializer must run at most once");
            return match initialize() {
                Ok(RemoteSingleFlightLoad::Cache(value)) => {
                    startup.check()?;
                    leader.complete(value.clone());
                    Ok(RemoteSingleFlightLookup {
                        value,
                        generation: flight_id,
                    })
                }
                Ok(RemoteSingleFlightLoad::Uncached(value)) => {
                    startup.check()?;
                    leader.finish_without_value();
                    Ok(RemoteSingleFlightLookup {
                        value,
                        generation: flight_id,
                    })
                }
                Err(error) => Err(error),
            };
        }
    }

    pub(crate) fn lock_state(&self) -> std::sync::MutexGuard<'_, RemoteSingleFlightState<K, V>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) struct RemoteSingleFlightLeader<'a, K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    cache: &'a RemoteSingleFlightCache<K, V>,
    key: K,
    flight_id: u64,
    kind: RemoteSingleFlightKind,
    completed: bool,
}

impl<'a, K, V> RemoteSingleFlightLeader<'a, K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    fn new(
        cache: &'a RemoteSingleFlightCache<K, V>,
        key: K,
        flight_id: u64,
        kind: RemoteSingleFlightKind,
    ) -> Self {
        Self {
            cache,
            key,
            flight_id,
            kind,
            completed: false,
        }
    }

    fn complete(&mut self, value: V) {
        let mut state = self.cache.lock_state();
        if !matches!(
            state.entries.get(&self.key),
            Some(RemoteSingleFlightEntry::Resolving { flight_id, kind })
                if *flight_id == self.flight_id && *kind == self.kind
        ) {
            self.completed = true;
            return;
        }
        state.entries.insert(
            self.key.clone(),
            RemoteSingleFlightEntry::Ready {
                value,
                generation: self.flight_id,
                kind: self.kind,
                expires_at: Instant::now() + self.cache.ttl,
            },
        );
        state.ready_order.retain(|cached| cached != &self.key);
        state.ready_order.push_back(self.key.clone());
        while state.ready_order.len() > self.cache.capacity {
            if let Some(evicted) = state.ready_order.pop_front() {
                state.entries.remove(&evicted);
            }
        }
        self.completed = true;
        self.cache.wake.notify_all();
    }

    fn finish_without_value(&mut self) {
        let mut state = self.cache.lock_state();
        if matches!(
            state.entries.get(&self.key),
            Some(RemoteSingleFlightEntry::Resolving { flight_id, kind })
                if *flight_id == self.flight_id && *kind == self.kind
        ) {
            state.entries.remove(&self.key);
        }
        self.completed = true;
        self.cache.wake.notify_all();
    }
}

impl<K, V> Drop for RemoteSingleFlightLeader<'_, K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.finish_without_value();
    }
}

pub(crate) struct RemoteBootstrapCache {
    probes: RemoteSingleFlightCache<RemoteBootstrapTransportKey, SshRemoteProbe>,
    pub(crate) helpers: RemoteSingleFlightCache<RemoteBootstrapTransportKey, PathBuf>,
}

impl RemoteBootstrapCache {
    fn new() -> Self {
        Self {
            probes: RemoteSingleFlightCache::new(
                REMOTE_BOOTSTRAP_CACHE_CAPACITY,
                REMOTE_BOOTSTRAP_CACHE_TTL,
            ),
            helpers: RemoteSingleFlightCache::new(
                REMOTE_BOOTSTRAP_CACHE_CAPACITY,
                REMOTE_BOOTSTRAP_CACHE_TTL,
            ),
        }
    }
}

/// Immutable remote connection options plus bounded, pathless bootstrap facts shared by opens.
#[derive(Clone)]
pub struct RemoteWorkspaceBootstrap {
    options: Arc<RemoteWorkspaceBackendOptions>,
    pub(crate) cache: Arc<RemoteBootstrapCache>,
}

impl RemoteWorkspaceBootstrap {
    pub fn new(options: RemoteWorkspaceBackendOptions) -> Self {
        Self {
            options: Arc::new(options),
            cache: Arc::new(RemoteBootstrapCache::new()),
        }
    }

    pub fn options(&self) -> &RemoteWorkspaceBackendOptions {
        self.options.as_ref()
    }
}

impl fmt::Debug for RemoteWorkspaceBootstrap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteWorkspaceBootstrap")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

pub(crate) struct RemoteHelperResolution {
    path: PathBuf,
    cache_generation: Option<u64>,
}

pub struct RemoteHelperManager<'a> {
    options: &'a RemoteWorkspaceBackendOptions,
    progress: Option<&'a dyn Fn(RemoteDeploymentProgress)>,
    startup: RemoteStartupContext,
    bootstrap: Option<&'a RemoteWorkspaceBootstrap>,
}

impl<'a> RemoteHelperManager<'a> {
    pub fn new(options: &'a RemoteWorkspaceBackendOptions) -> Self {
        Self {
            options,
            progress: None,
            startup: RemoteStartupContext::new(DEFAULT_REMOTE_STARTUP_TIMEOUT),
            bootstrap: None,
        }
    }

    pub fn with_progress(
        options: &'a RemoteWorkspaceBackendOptions,
        progress: &'a dyn Fn(RemoteDeploymentProgress),
    ) -> Self {
        Self {
            options,
            progress: Some(progress),
            startup: RemoteStartupContext::new(DEFAULT_REMOTE_STARTUP_TIMEOUT),
            bootstrap: None,
        }
    }

    pub fn with_progress_and_startup_context(
        options: &'a RemoteWorkspaceBackendOptions,
        progress: Option<&'a dyn Fn(RemoteDeploymentProgress)>,
        startup: &RemoteStartupContext,
    ) -> Self {
        Self {
            options,
            progress,
            startup: startup.clone(),
            bootstrap: None,
        }
    }

    pub fn with_bootstrap_and_startup_context(
        bootstrap: &'a RemoteWorkspaceBootstrap,
        progress: Option<&'a dyn Fn(RemoteDeploymentProgress)>,
        startup: &RemoteStartupContext,
    ) -> Self {
        Self {
            options: bootstrap.options(),
            progress,
            startup: startup.clone(),
            bootstrap: Some(bootstrap),
        }
    }

    pub fn resolve_helper_for_location(&self, location: &WorkspaceLocation) -> Result<PathBuf> {
        self.resolve_helper_for_startup(location)
            .map(|helper| helper.path)
    }

    fn resolve_helper_for_startup(
        &self,
        location: &WorkspaceLocation,
    ) -> Result<RemoteHelperResolution> {
        self.check_cancelled()?;
        let helper =
            if let (Some(bootstrap), Some(key)) = (
                self.bootstrap,
                RemoteBootstrapTransportKey::from_location(location, self.options),
            ) {
                let lookup = bootstrap.cache.helpers.get_or_try_init_controlled(
                    key,
                    &self.startup,
                    || self.resolve_helper_for_location_uncached(location),
                )?;
                RemoteHelperResolution {
                    path: lookup.value,
                    cache_generation: Some(lookup.generation),
                }
            } else {
                RemoteHelperResolution {
                    path: self
                        .resolve_helper_for_location_uncached(location)?
                        .into_value(),
                    cache_generation: None,
                }
            };
        self.check_cancelled()?;
        Ok(helper)
    }

    fn resolve_helper_for_location_uncached(
        &self,
        location: &WorkspaceLocation,
    ) -> Result<RemoteSingleFlightLoad<PathBuf>> {
        match location {
            WorkspaceLocation::Ssh { target, .. } => self.resolve_ssh_helper(
                &ssh_target_from_workspace_target_with_options(target, self.options),
            ),
            WorkspaceLocation::Wsl { distro, .. } => self.resolve_wsl_helper(distro),
            WorkspaceLocation::Local { .. } => Ok(RemoteSingleFlightLoad::Uncached(
                self.options.remote_helper_path.clone(),
            )),
        }
    }

    pub fn reinstall_helper_for_location(
        &self,
        location: &WorkspaceLocation,
    ) -> Result<Option<PathBuf>> {
        self.check_cancelled()?;
        let helper = self.reinstall_helper_for_location_uncached(location)?;
        self.check_cancelled()?;
        Ok(helper)
    }

    fn refresh_helper_for_location(
        &self,
        location: &WorkspaceLocation,
        previous: &RemoteHelperResolution,
    ) -> Result<Option<RemoteHelperResolution>> {
        self.check_cancelled()?;
        let helper = if let (Some(bootstrap), Some(key), Some(observed_generation)) = (
            self.bootstrap,
            RemoteBootstrapTransportKey::from_location(location, self.options),
            previous.cache_generation,
        ) {
            let lookup = bootstrap.cache.helpers.refresh_after(
                key,
                observed_generation,
                &self.startup,
                || {
                    self.reinstall_helper_for_location_uncached(location)?
                        .context("remote helper reinstall did not return a helper path")
                },
            )?;
            Some(RemoteHelperResolution {
                path: lookup.value,
                cache_generation: Some(lookup.generation),
            })
        } else {
            self.reinstall_helper_for_location_uncached(location)?
                .map(|path| RemoteHelperResolution {
                    path,
                    cache_generation: None,
                })
        };
        self.check_cancelled()?;
        Ok(helper)
    }

    fn reinstall_helper_for_location_uncached(
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

    fn resolve_ssh_helper(&self, target: &SshTarget) -> Result<RemoteSingleFlightLoad<PathBuf>> {
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

    fn resolve_wsl_helper(&self, distro: &str) -> Result<RemoteSingleFlightLoad<PathBuf>> {
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
    ) -> Result<RemoteSingleFlightLoad<PathBuf>> {
        self.check_cancelled()?;
        if helper_path_is_override
            && install_policy != RemoteHelperInstallPolicy::Upload
            && install_policy != RemoteHelperInstallPolicy::RemoteDownload
        {
            return Ok(RemoteSingleFlightLoad::Uncached(
                configured_helper_path.to_path_buf(),
            ));
        }

        if install_policy == RemoteHelperInstallPolicy::Never {
            return Ok(RemoteSingleFlightLoad::Uncached(
                configured_helper_path.to_path_buf(),
            ));
        }

        let connection_phase = match transport {
            LinuxHelperTransport::Ssh(_) => RemoteDeploymentPhase::ConnectingSshHost,
            LinuxHelperTransport::Wsl(_) => RemoteDeploymentPhase::StartingWslDistro,
        };
        self.emit_progress(connection_phase, Some(transport.target_name()), None);
        let probe = match self.probe_linux_platform(transport) {
            Ok(probe) => probe,
            Err(error) if install_policy == RemoteHelperInstallPolicy::Auto => {
                self.check_cancelled()?;
                tracing::debug!(
                    error = %error,
                    target = %transport.target_name(),
                    "Falling back to configured helper after platform probe failure"
                );
                return Ok(RemoteSingleFlightLoad::Uncached(
                    configured_helper_path.to_path_buf(),
                ));
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
        if self.remote_helper_matches(transport, &helper_path, &probe.platform)? {
            return Ok(RemoteSingleFlightLoad::Cache(helper_path));
        }

        if install_policy == RemoteHelperInstallPolicy::RemoteDownload {
            self.install_helper_by_remote_download(transport, &probe.platform, &helper_path)?;
            if !self.remote_helper_matches(transport, &helper_path, &probe.platform)? {
                bail!(
                    "downloaded nucleotide-remote on {} but version probe did not match protocol {}",
                    transport.description(),
                    PROTOCOL_VERSION
                );
            }
            return Ok(RemoteSingleFlightLoad::Cache(helper_path));
        }

        let Some(local_helper) = self.local_upload_artifact_for_platform(&probe.platform) else {
            if install_policy == RemoteHelperInstallPolicy::Auto {
                match self.install_helper_by_remote_download(
                    transport,
                    &probe.platform,
                    &helper_path,
                ) {
                    Ok(()) => {
                        if !self.remote_helper_matches(transport, &helper_path, &probe.platform)? {
                            bail!(
                                "downloaded nucleotide-remote on {} but version probe did not match protocol {}",
                                transport.description(),
                                PROTOCOL_VERSION
                            );
                        }
                        return Ok(RemoteSingleFlightLoad::Cache(helper_path));
                    }
                    Err(error) => {
                        self.check_cancelled()?;
                        tracing::debug!(
                            error = %error,
                            target = %transport.target_name(),
                            "Automatic remote helper download was unavailable"
                        );
                    }
                }
            }

            if install_policy == RemoteHelperInstallPolicy::Upload {
                bail!(
                    "helper upload requested for {}, but no local nucleotide-remote artifact is configured",
                    transport.description()
                );
            }
            return Ok(RemoteSingleFlightLoad::Uncached(
                configured_helper_path.to_path_buf(),
            ));
        };

        if !local_helper.is_file() {
            if install_policy == RemoteHelperInstallPolicy::Upload {
                bail!(
                    "helper upload requested for {}, but local artifact does not exist: {}",
                    transport.description(),
                    local_helper.display()
                );
            }
            return Ok(RemoteSingleFlightLoad::Uncached(
                configured_helper_path.to_path_buf(),
            ));
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

        if !self.remote_helper_matches(transport, &helper_path, &probe.platform)? {
            bail!(
                "uploaded nucleotide-remote on {} but version probe did not match protocol {}",
                transport.description(),
                PROTOCOL_VERSION
            );
        }

        Ok(RemoteSingleFlightLoad::Cache(helper_path))
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
        self.check_cancelled()?;
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
            if !self.remote_helper_matches(transport, &helper_path, &probe.platform)? {
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

        if !self.remote_helper_matches(transport, &helper_path, &probe.platform)? {
            bail!(
                "reinstalled nucleotide-remote on {} but version probe did not match protocol {}",
                transport.description(),
                PROTOCOL_VERSION
            );
        }

        Ok(helper_path)
    }

    pub(crate) fn local_upload_artifact_for_platform(
        &self,
        platform: &SshRemotePlatform,
    ) -> Option<PathBuf> {
        if let Some(path) = self.options.ssh_helper_upload_path.as_ref() {
            return Some(path.clone());
        }

        let artifact_dir = self.options.ssh_helper_artifact_dir.as_ref()?;
        bundled_ssh_helper_artifact_path(artifact_dir, platform)
    }

    fn probe_linux_platform(&self, transport: LinuxHelperTransport<'_>) -> Result<SshRemoteProbe> {
        self.check_cancelled()?;
        let probe = if let Some(bootstrap) = self.bootstrap {
            bootstrap.cache.probes.get_or_try_init(
                RemoteBootstrapTransportKey::from_transport(transport),
                &self.startup,
                || self.probe_linux_platform_uncached(transport),
            )?
        } else {
            self.probe_linux_platform_uncached(transport)?
        };
        self.check_cancelled()?;
        Ok(probe)
    }

    fn probe_linux_platform_uncached(
        &self,
        transport: LinuxHelperTransport<'_>,
    ) -> Result<SshRemoteProbe> {
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
    ) -> Result<bool> {
        match self.remote_helper_version(transport, helper_path) {
            Ok(info) => Ok(helper_version_matches_current(&info, platform)),
            Err(error) => {
                self.check_cancelled()?;
                tracing::debug!(
                    error = %error,
                    target = %transport.target_name(),
                    helper = %helper_path.display(),
                    "Remote helper version probe did not match"
                );
                Ok(false)
            }
        }
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

    pub(crate) fn remote_helper_download_urls(
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
        let expected_sha256 = sha256_reader_with_startup_context(&mut local_file, &self.startup)
            .with_context(|| format!("failed to hash {}", local_helper.display()))?;
        self.check_cancelled()?;
        local_file
            .rewind()
            .with_context(|| format!("failed to rewind {}", local_helper.display()))?;
        let remote_command =
            remote_helper_upload_command(&helper_dir, &tmp_path, &helper_path, &expected_sha256);
        let spec = transport.command(remote_command);
        let mut command = spec.contained_command();
        command.stdin(Stdio::from(local_file));
        let output = self
            .run_bounded_command(
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
        let mut command = spec.contained_command();
        command.stdin(Stdio::null());
        let output = self
            .run_bounded_command(
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
        let mut command = spec.contained_command();
        command.stdin(Stdio::null());
        let output = self
            .run_bounded_command(
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
        if self.check_cancelled().is_err() {
            return;
        }
        if let Some(progress) = self.progress {
            progress(RemoteDeploymentProgress {
                phase,
                target,
                detail,
            });
        }
    }

    pub(crate) fn run_bounded_command(
        &self,
        command: &mut nucleotide_process::ContainedCommand,
        mut limits: nucleotide_process::OutputLimits,
    ) -> Result<std::process::Output> {
        self.check_cancelled()?;
        limits.timeout = self.startup.cap_timeout(limits.timeout)?;
        let output = nucleotide_process::output_with_limits_contained_and_cancellation(
            command,
            limits,
            self.startup.cancellation().as_atomic_bool(),
        );
        self.check_cancelled()?;
        output.map_err(Into::into)
    }

    fn check_cancelled(&self) -> Result<()> {
        self.startup.check()
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

pub(crate) fn remote_service_command_for_location_with_options(
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

pub(crate) fn remote_terminal_proxy_command_for_location_with_options(
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
    let startup = RemoteStartupContext::new(DEFAULT_REMOTE_STARTUP_TIMEOUT);
    let bootstrap = RemoteWorkspaceBootstrap::new(options.clone());
    connect_workspace_backend_for_location_with_optional_progress(
        location, &bootstrap, None, &startup,
    )
}

pub fn connect_workspace_backend_for_location_with_progress(
    location: WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
    progress: &dyn Fn(RemoteDeploymentProgress),
) -> Result<WorkspaceBackendConnection> {
    let startup = RemoteStartupContext::new(DEFAULT_REMOTE_STARTUP_TIMEOUT);
    let bootstrap = RemoteWorkspaceBootstrap::new(options.clone());
    connect_workspace_backend_for_location_with_optional_progress(
        location,
        &bootstrap,
        Some(progress),
        &startup,
    )
}

pub fn connect_workspace_backend_for_location_with_progress_and_cancellation(
    location: WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
    progress: &dyn Fn(RemoteDeploymentProgress),
    cancellation: &WorkspaceCancellationToken,
) -> Result<WorkspaceBackendConnection> {
    let startup = RemoteStartupContext::with_cancellation(
        cancellation.clone(),
        DEFAULT_REMOTE_STARTUP_TIMEOUT,
    );
    connect_workspace_backend_for_location_with_progress_and_startup_context(
        location, options, progress, &startup,
    )
}

pub fn connect_workspace_backend_for_location_with_progress_and_startup_context(
    location: WorkspaceLocation,
    options: &RemoteWorkspaceBackendOptions,
    progress: &dyn Fn(RemoteDeploymentProgress),
    startup: &RemoteStartupContext,
) -> Result<WorkspaceBackendConnection> {
    let bootstrap = RemoteWorkspaceBootstrap::new(options.clone());
    connect_workspace_backend_for_location_with_bootstrap_progress_and_startup_context(
        location, &bootstrap, progress, startup,
    )
}

pub fn connect_workspace_backend_for_location_with_bootstrap_progress_and_startup_context(
    location: WorkspaceLocation,
    bootstrap: &RemoteWorkspaceBootstrap,
    progress: &dyn Fn(RemoteDeploymentProgress),
    startup: &RemoteStartupContext,
) -> Result<WorkspaceBackendConnection> {
    connect_workspace_backend_for_location_with_optional_progress(
        location,
        bootstrap,
        Some(progress),
        startup,
    )
}

pub(crate) fn connect_workspace_backend_for_location_with_optional_progress(
    location: WorkspaceLocation,
    bootstrap: &RemoteWorkspaceBootstrap,
    progress: Option<&dyn Fn(RemoteDeploymentProgress)>,
    startup: &RemoteStartupContext,
) -> Result<WorkspaceBackendConnection> {
    startup.check()?;
    let options = bootstrap.options();
    if let WorkspaceLocation::Local { path } = &location {
        if options.use_local_service {
            let helper_path = options
                .local_helper_path
                .as_deref()
                .unwrap_or(&options.remote_helper_path);
            let command = local_service_command(helper_path, path);
            let (backend, hello) = spawn_child_process_workspace_backend_with_startup_context(
                RemoteWorkspaceIdentity {
                    kind: RemoteWorkspaceKind::Other("local-service".to_string()),
                    name: "local-service".to_string(),
                },
                &command,
                startup,
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
    let helper_manager =
        RemoteHelperManager::with_bootstrap_and_startup_context(bootstrap, progress, startup);
    let helper = helper_manager.resolve_helper_for_startup(&location)?;
    startup.check()?;
    let command =
        remote_service_command_for_location_with_options(&location, &helper.path, options)
            .context("remote workspace location is missing a service command")?;
    let mapping = location.path_mapping();
    let display_root = location.display_root().to_path_buf();
    startup.check()?;
    emit_remote_deployment_progress(
        progress,
        RemoteDeploymentPhase::StartingRemoteWorkspaceService,
        &location,
        Some(display_root.display().to_string()),
    );
    let (backend, hello) = match spawn_child_process_workspace_backend_with_startup_context(
        identity.clone(),
        &command,
        startup,
    ) {
        Ok(connection) => connection,
        Err(error) if remote_startup_error_can_retry_helper_install(&location, &error) => {
            startup.check()?;
            let retry_helper = helper_manager
                .refresh_helper_for_location(&location, &helper)
                .with_context(|| {
                    format!(
                        "failed to reinstall remote helper after startup failure. Initial error: {error:#}"
                    )
                })?
                .context("remote helper reinstall did not apply to this workspace location")?;
            let retry_command = remote_service_command_for_location_with_options(
                &location,
                &retry_helper.path,
                options,
            )
            .context("remote workspace location is missing a service command")?;
            startup.check()?;
            emit_remote_deployment_progress(
                progress,
                RemoteDeploymentPhase::StartingRemoteWorkspaceService,
                &location,
                Some(display_root.display().to_string()),
            );
            spawn_child_process_workspace_backend_with_startup_context(
                identity,
                &retry_command,
                startup,
            )
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
                    remote_helper_setup_hint(&location, &helper.path)
                )
            });
        }
    };

    startup.check()?;

    Ok(WorkspaceBackendConnection {
        backend: path_mapped_workspace_backend(backend, mapping),
        location,
        hello: Some(hello),
    })
}

pub(crate) fn emit_remote_deployment_progress(
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

pub(crate) fn remote_deployment_target(location: &WorkspaceLocation) -> Option<String> {
    match location {
        WorkspaceLocation::Ssh { target, .. } => Some(ssh_target_display_name(target)),
        WorkspaceLocation::Wsl { distro, .. } => Some(distro.clone()),
        WorkspaceLocation::Local { .. } => None,
    }
}

pub(crate) fn remote_startup_error_can_retry_helper_install(
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

pub(crate) fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(false)
}

pub(crate) fn env_flag_enabled_with_default(value: Option<&str>, default: bool) -> bool {
    match value {
        None | Some("") => default,
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES") | Some("on")
        | Some("ON") => true,
        Some("0") | Some("false") | Some("FALSE") | Some("no") | Some("NO") | Some("off")
        | Some("OFF") => false,
        Some(_) => default,
    }
}

pub(crate) fn ssh_connect_timeout_from_env(value: Option<&str>) -> Option<u64> {
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

pub(crate) fn ssh_extra_args_from_env(value: Option<OsString>) -> Vec<OsString> {
    let Some(value) = value else {
        return Vec::new();
    };
    split_ssh_extra_args(&value.to_string_lossy())
        .into_iter()
        .map(OsString::from)
        .collect()
}

pub(crate) fn split_ssh_extra_args(value: &str) -> Vec<String> {
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

pub(crate) fn default_ssh_control_master_enabled() -> bool {
    cfg!(unix)
}

pub fn default_ssh_control_path() -> Option<PathBuf> {
    if !cfg!(unix) {
        return None;
    }

    Some(short_ssh_control_dir().join("%C"))
}

pub(crate) fn short_ssh_control_dir() -> PathBuf {
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
pub(crate) fn ensure_private_ssh_control_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o700);
    let _ = std::fs::set_permissions(path, permissions);
}

#[cfg(not(unix))]
pub(crate) fn ensure_private_ssh_control_dir(_path: &Path) {}

pub(crate) fn ssh_non_tty_command(
    target: SshTarget,
    remote_command: String,
) -> RemoteServiceCommand {
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

pub(crate) fn wsl_shell_command(
    distro: impl AsRef<OsStr>,
    remote_command: String,
) -> RemoteServiceCommand {
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

pub(crate) fn parse_linux_probe_output(output: &str) -> Result<SshRemoteProbe> {
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

pub(crate) fn parse_uname_platform(value: &str) -> Result<SshRemotePlatform> {
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

pub(crate) fn remote_linux_helper_path(probe: &SshRemoteProbe) -> PathBuf {
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

pub(crate) fn remote_helper_file_name(platform: &SshRemotePlatform) -> String {
    format!(
        "nucleotide-remote-{}-{}-{}",
        env!("CARGO_PKG_VERSION"),
        platform.os,
        platform.arch
    )
}

pub(crate) fn remote_helper_release_asset_name(platform: &SshRemotePlatform) -> String {
    format!("nucleotide-remote-{}-{}", platform.os, platform.arch)
}

pub(crate) fn default_remote_helper_download_base_url() -> String {
    format!(
        "{}/releases/download/{DEFAULT_RELEASE_TAG_PREFIX}{}",
        env!("CARGO_PKG_REPOSITORY").trim_end_matches('/'),
        env!("CARGO_PKG_VERSION")
    )
}

pub(crate) fn helper_version_matches_current(
    info: &HelperVersionInfo,
    platform: &SshRemotePlatform,
) -> bool {
    info.helper_version == env!("CARGO_PKG_VERSION")
        && info.protocol_version == PROTOCOL_VERSION
        && info.frame_version == FRAME_VERSION
        && info.os == platform.os
        && info.arch == platform.arch
}

pub(crate) fn parse_helper_version_output(output: &str) -> Result<HelperVersionInfo> {
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

pub(crate) fn posix_join(base: &str, parts: &[&str]) -> String {
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

pub(crate) fn posix_parent(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((parent, _)) if !parent.is_empty() => parent.to_string(),
        _ => ".".to_string(),
    }
}

#[cfg(test)]
pub(crate) fn sha256_reader(reader: &mut impl Read) -> io::Result<String> {
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

pub(crate) fn sha256_reader_with_startup_context(
    reader: &mut impl Read,
    startup: &RemoteStartupContext,
) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        startup.check()?;
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                startup.check()?;
                return Err(error.into());
            }
        };
        if read == 0 {
            startup.check()?;
            return Ok(format!("{:x}", hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

pub(crate) fn remote_helper_upload_command(
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

pub(crate) fn remote_helper_download_command(
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

pub(crate) fn bundled_local_helper_path(current_exe: &Path) -> Option<PathBuf> {
    let executable_dir = current_exe.parent()?;
    let helper_path = executable_dir.join(local_helper_binary_name());
    helper_path.is_file().then_some(helper_path)
}

pub(crate) fn bundled_ssh_helper_artifact_path(
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

pub(crate) fn ssh_helper_artifact_candidate_names(platform: &SshRemotePlatform) -> Vec<String> {
    let mut candidates = vec![
        remote_helper_file_name(platform),
        format!("nucleotide-remote-{}-{}", platform.os, platform.arch),
    ];

    if current_host_platform_matches(platform) {
        candidates.push(local_helper_binary_name().to_string());
    }

    candidates
}

pub(crate) fn current_host_platform_matches(platform: &SshRemotePlatform) -> bool {
    let Some(host) = current_host_remote_platform() else {
        return false;
    };

    host == *platform
}

pub(crate) fn current_host_remote_platform() -> Option<SshRemotePlatform> {
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

pub(crate) fn local_helper_binary_name() -> &'static str {
    if cfg!(windows) {
        "nucleotide-remote.exe"
    } else {
        "nucleotide-remote"
    }
}

pub(crate) fn local_helper_setup_hint(helper_path: &Path) -> String {
    format!(
        "Local service mode needs nucleotide-remote at {}. Set NUCLEOTIDE_LOCAL_REMOTE_HELPER or place {} next to the nucl executable.",
        helper_path.display(),
        local_helper_binary_name()
    )
}

pub(crate) fn remote_helper_setup_hint(location: &WorkspaceLocation, helper_path: &Path) -> String {
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

pub(crate) fn ssh_target_from_workspace_target(target: &SshWorkspaceTarget) -> SshTarget {
    SshTarget {
        host: target.host.clone(),
        user: target.user.clone(),
        port: target.port,
        connect_timeout_secs: None,
        extra_args: Vec::new(),
        control_path: None,
    }
}

pub(crate) fn ssh_target_from_workspace_target_with_options(
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

pub(crate) fn ssh_target_display_name(target: &SshWorkspaceTarget) -> String {
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

pub(crate) fn ssh_display_host(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}
