// ABOUTME: WorkspaceBackend adapter backed by the remote protocol client abstraction
// ABOUTME: Converts async workspace operations into cancellable remote requests and streams

use super::*;

pub struct RemoteWorkspaceBackendImpl<C: RemoteWorkspaceProtocolClient> {
    identity: RemoteWorkspaceIdentity,
    pub(crate) client: Arc<C>,
}

pub type RemoteWorkspaceV5Backend<R, W> =
    RemoteWorkspaceBackendImpl<RemoteWorkspaceV5MultiplexedClient<R, W>>;
pub(crate) type RemoteWorkspaceV5ChildClient =
    RemoteWorkspaceV5MultiplexedClient<ChildStdout, ChildProcessV5Writer>;
pub(crate) type RemoteWorkspaceV5ReconnectingClient =
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
    pub(crate) fn from_protocol_client(identity: RemoteWorkspaceIdentity, client: C) -> Self {
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
        self.request_with_optional_workspace_cancellation(operation, path, request, body, None)
            .await
    }

    async fn request_with_workspace_cancellation(
        &self,
        operation: &'static str,
        path: &Path,
        request: RemoteRequest,
        body: Vec<u8>,
        workspace_cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<(RemoteResponse, Vec<u8>)>
    where
        C: 'static,
    {
        self.request_with_optional_workspace_cancellation(
            operation,
            path,
            request,
            body,
            Some(workspace_cancellation),
        )
        .await
    }

    async fn request_with_optional_workspace_cancellation(
        &self,
        operation: &'static str,
        path: &Path,
        request: RemoteRequest,
        body: Vec<u8>,
        workspace_cancellation: Option<&WorkspaceCancellationToken>,
    ) -> nucleotide_workspace::Result<(RemoteResponse, Vec<u8>)>
    where
        C: 'static,
    {
        if let Some(cancellation) = workspace_cancellation {
            cancellation.check_cancelled(operation, path)?;
        }
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let workspace_cancellation_registration = workspace_cancellation.map(|workspace| {
            let cancellation = cancellation.clone();
            workspace.on_cancel(move || cancellation.cancel())
        });
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

        let received = receiver.await;
        drop(workspace_cancellation_registration);
        match received {
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

    async fn read_file_stream_with_optional_workspace_cancellation(
        &self,
        path: &Path,
        options: ReadOptions,
        workspace_cancellation: Option<&WorkspaceCancellationToken>,
    ) -> nucleotide_workspace::Result<FileReadStream>
    where
        C: 'static,
    {
        if let Some(cancellation) = workspace_cancellation {
            cancellation.check_cancelled("read file", path)?;
        }
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let workspace_cancellation_registration = workspace_cancellation.map(|workspace| {
            let cancellation = cancellation.clone();
            workspace.on_cancel(move || cancellation.cancel())
        });
        let request = RemoteRequest::ReadFile {
            path: path.to_path_buf(),
            max_bytes: options.max_bytes,
        };
        let context = request.v5_request_context();
        let client = Arc::clone(&self.client);
        let request_path = path.to_path_buf();
        let worker_path = request_path.clone();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = oneshot::channel();
        std::thread::Builder::new()
            .name("nucleotide-remote-read-file-stream".to_string())
            .spawn(move || {
                let result = client
                    .read_file_stream_with_context_and_cancellation(
                        request,
                        context,
                        &worker_cancellation,
                    )
                    .map_err(|error| client_error_to_workspace("read file", &worker_path, error));
                let _ = sender.send(result);
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "read file",
                path: request_path.clone(),
                source,
            })?;

        let remote_stream = match receiver.await {
            Ok(result) => {
                cancel_on_drop.disarm();
                result?
            }
            Err(_) => {
                return Err(WorkspaceError::Remote {
                    operation: "read file",
                    path: request_path,
                    message: "remote file stream worker exited before returning a stream"
                        .to_string(),
                    diagnostic: None,
                });
            }
        };
        let error_path = path.to_path_buf();
        Ok(FileReadStream::new(StreamExt::map(
            remote_stream,
            move |event| {
                let _registration = &workspace_cancellation_registration;
                event
                    .map(|event| match event {
                        RemoteFileReadEvent::Chunk(bytes) => FileReadEvent::Chunk(bytes),
                        RemoteFileReadEvent::Complete(read) => {
                            FileReadEvent::Complete(FileReadMetadata {
                                path: read.path,
                                size: read.size,
                                version: read.version.map(FileVersion::from_bytes),
                                modified: system_time_from_unix_millis_and_nanos(
                                    read.modified_unix_millis,
                                    read.modified_unix_nanos,
                                ),
                                readonly: read.readonly,
                                truncated: read.truncated,
                            })
                        }
                    })
                    .map_err(|error| client_error_to_workspace("read file", &error_path, error))
            },
        )))
    }

    async fn file_search_stream_with_optional_workspace_cancellation(
        &self,
        query: FileSearchQuery,
        workspace_cancellation: Option<&WorkspaceCancellationToken>,
    ) -> nucleotide_workspace::Result<FileSearchStream>
    where
        C: 'static,
    {
        let root = query.root.clone();
        if let Some(cancellation) = workspace_cancellation {
            cancellation.check_cancelled("file search", &root)?;
        }
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let workspace_cancellation_registration = workspace_cancellation.map(|workspace| {
            let cancellation = cancellation.clone();
            workspace.on_cancel(move || cancellation.cancel())
        });
        let request = RemoteRequest::FileSearch(FileSearchRequest {
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
        });
        let context = request.v5_request_context();
        let client = Arc::clone(&self.client);
        let worker_root = root.clone();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = oneshot::channel();
        std::thread::Builder::new()
            .name("nucleotide-remote-file-search-stream".to_string())
            .spawn(move || {
                let result = client
                    .file_search_stream_with_context_and_cancellation(
                        request,
                        context,
                        &worker_cancellation,
                    )
                    .map_err(|error| client_error_to_workspace("file search", &worker_root, error));
                let _ = sender.send(result);
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "file search",
                path: root.clone(),
                source,
            })?;

        let remote_stream = match receiver.await {
            Ok(result) => {
                cancel_on_drop.disarm();
                result?
            }
            Err(_) => {
                return Err(WorkspaceError::Remote {
                    operation: "file search",
                    path: root.clone(),
                    message: "remote file-search worker exited before returning a stream"
                        .to_string(),
                    diagnostic: None,
                });
            }
        };
        let error_root = root;
        Ok(FileSearchStream::new(StreamExt::map(
            remote_stream,
            move |event| {
                let _registration = &workspace_cancellation_registration;
                event
                    .map(|event| match event {
                        RemoteFileSearchEvent::Batch(files) => FileSearchEvent::Batch(files),
                        RemoteFileSearchEvent::Complete { root, truncated } => {
                            FileSearchEvent::Complete { root, truncated }
                        }
                    })
                    .map_err(|error| client_error_to_workspace("file search", &error_root, error))
            },
        )))
    }

    async fn text_search_stream_with_optional_workspace_cancellation(
        &self,
        query: TextSearchQuery,
        workspace_cancellation: Option<&WorkspaceCancellationToken>,
    ) -> nucleotide_workspace::Result<TextSearchStream>
    where
        C: 'static,
    {
        let root = query.root.clone();
        if let Some(cancellation) = workspace_cancellation {
            cancellation.check_cancelled("text search", &root)?;
        }
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let workspace_cancellation_registration = workspace_cancellation.map(|workspace| {
            let cancellation = cancellation.clone();
            workspace.on_cancel(move || cancellation.cancel())
        });
        let request = RemoteRequest::TextSearch(TextSearchRequest {
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
        });
        let context = request.v5_request_context();
        let client = Arc::clone(&self.client);
        let worker_root = root.clone();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = oneshot::channel();
        std::thread::Builder::new()
            .name("nucleotide-remote-text-search-stream".to_string())
            .spawn(move || {
                let result = client
                    .text_search_stream_with_context_and_cancellation(
                        request,
                        context,
                        &worker_cancellation,
                    )
                    .map_err(|error| client_error_to_workspace("text search", &worker_root, error));
                let _ = sender.send(result);
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "text search",
                path: root.clone(),
                source,
            })?;

        let remote_stream = match receiver.await {
            Ok(result) => {
                cancel_on_drop.disarm();
                result?
            }
            Err(_) => {
                return Err(WorkspaceError::Remote {
                    operation: "text search",
                    path: root.clone(),
                    message: "remote text-search worker exited before returning a stream"
                        .to_string(),
                    diagnostic: None,
                });
            }
        };
        let error_root = root;
        Ok(TextSearchStream::new(StreamExt::map(
            remote_stream,
            move |event| {
                let _registration = &workspace_cancellation_registration;
                event
                    .map(|event| match event {
                        RemoteTextSearchEvent::Batch(matches) => TextSearchEvent::Batch(
                            matches
                                .into_iter()
                                .map(text_search_match_from_response)
                                .collect(),
                        ),
                        RemoteTextSearchEvent::Complete { root, truncated } => {
                            TextSearchEvent::Complete { root, truncated }
                        }
                    })
                    .map_err(|error| client_error_to_workspace("text search", &error_root, error))
            },
        )))
    }

    async fn run_process_stream_with_optional_workspace_cancellation(
        &self,
        spec: ProcessSpec,
        workspace_cancellation: Option<&WorkspaceCancellationToken>,
    ) -> nucleotide_workspace::Result<ProcessStream>
    where
        C: 'static,
    {
        let cwd = spec.cwd.clone();
        if let Some(cancellation) = workspace_cancellation {
            cancellation.check_cancelled("run process", &cwd)?;
        }
        let mut cancel_on_drop = RemoteRequestCancelOnDrop::new();
        let cancellation = cancel_on_drop.cancellation();
        let workspace_cancellation_registration = workspace_cancellation.map(|workspace| {
            let cancellation = cancellation.clone();
            workspace.on_cancel(move || cancellation.cancel())
        });
        let ProcessSpec {
            program,
            args,
            cwd: request_cwd,
            env,
            clear_env,
            inherit_project_environment,
            stdin,
            max_output_bytes,
            timeout_ms,
        } = spec;
        let request = RemoteRequest::RunProcess(ProcessRequest {
            program,
            args,
            cwd: request_cwd,
            env,
            clear_env,
            inherit_project_environment,
            max_output_bytes,
            timeout_ms,
        });
        let context = request.v5_request_context();
        let client = Arc::clone(&self.client);
        let worker_cwd = cwd.clone();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = oneshot::channel();
        std::thread::Builder::new()
            .name("nucleotide-remote-process-stream".to_string())
            .spawn(move || {
                let result = client
                    .run_process_stream_with_context_and_cancellation(
                        request,
                        stdin,
                        context,
                        &worker_cancellation,
                    )
                    .map_err(|error| client_error_to_workspace("run process", &worker_cwd, error));
                let _ = sender.send(result);
            })
            .map_err(|source| WorkspaceError::Io {
                operation: "run process",
                path: cwd.clone(),
                source,
            })?;

        let remote_stream = match receiver.await {
            Ok(result) => {
                cancel_on_drop.disarm();
                result?
            }
            Err(_) => {
                return Err(WorkspaceError::Remote {
                    operation: "run process",
                    path: cwd,
                    message: "remote process worker exited before returning a stream".to_string(),
                    diagnostic: None,
                });
            }
        };
        let error_cwd = cwd;
        Ok(ProcessStream::new(StreamExt::map(
            remote_stream,
            move |event| {
                let _registration = &workspace_cancellation_registration;
                event
                    .map(|event| match event {
                        RemoteProcessEvent::Stdout(bytes) => ProcessEvent::Stdout(bytes),
                        RemoteProcessEvent::Stderr(bytes) => ProcessEvent::Stderr(bytes),
                        RemoteProcessEvent::Complete(response) => {
                            ProcessEvent::Complete(ProcessCompletion {
                                status_code: response.status_code,
                                success: response.success,
                                stdout_truncated: response.stdout_truncated,
                                stderr_truncated: response.stderr_truncated,
                                timed_out: response.timed_out,
                            })
                        }
                    })
                    .map_err(|error| client_error_to_workspace("run process", &error_cwd, error))
            },
        )))
    }
}

pub(crate) fn request_with_client<C>(
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
    let startup = RemoteStartupContext::new(DEFAULT_REMOTE_STARTUP_TIMEOUT);
    spawn_child_process_workspace_backend_with_startup_context(identity, command, &startup)
}

pub(crate) fn spawn_child_process_workspace_backend_with_startup_context(
    identity: RemoteWorkspaceIdentity,
    command: &RemoteServiceCommand,
    startup: &RemoteStartupContext,
) -> Result<(WorkspaceBackendHandle, HelloResponse)> {
    startup.check()?;
    tracing::info!(
        remote_kind = ?identity.kind,
        remote_name = %identity.name,
        command = %command.display_context(),
        "Starting v5 remote workspace service process"
    );
    let (io, control) = match spawn_child_process_v5_io(command) {
        Ok(connection) => connection,
        Err(error) => {
            startup.check()?;
            return Err(error).with_context(|| {
                format!(
                    "failed to start v5 remote workspace service: {}",
                    command.display_context()
                )
            });
        }
    };
    if let Err(error) = startup.check() {
        control.abort();
        return Err(error);
    }
    let client_hello = protocol_v5::ClientHello::nucleotide(env!("CARGO_PKG_VERSION"));
    let handshake_timeout = match startup.cap_timeout(V5_CHILD_HANDSHAKE_TIMEOUT) {
        Ok(timeout) => timeout,
        Err(error) => {
            control.abort();
            return Err(error);
        }
    };
    let client_result = connect_child_process_v5_client_with_timeout_and_cancellation(
        io,
        Arc::clone(&control),
        client_hello,
        handshake_timeout,
        Some(startup.cancellation().clone()),
    );
    if let Err(error) = startup.check() {
        control.abort();
        return Err(error);
    }
    let client = client_result.with_context(|| {
            format!(
                "failed to connect to v5 remote workspace service after starting {}; verify the helper speaks protocol v5",
                command.display_context()
            )
        })?;
    let hello = hello_response_from_v5_server_hello(client.server_hello());
    if let Err(error) = startup.check() {
        control.abort();
        return Err(error);
    }
    let reconnect_command = command.clone();
    let reconnect_identity = identity.clone();
    let reconnecting_client: RemoteWorkspaceV5ReconnectingClient =
        ReconnectingRemoteWorkspaceProtocolClient::new_with_attempt(client, move |attempt| {
            tracing::info!(
                remote_kind = ?reconnect_identity.kind,
                remote_name = %reconnect_identity.name,
                command = %reconnect_command.display_context(),
                "Reconnecting v5 remote workspace service process"
            );
            if let Some(attempt) = attempt.as_ref() {
                attempt.cancellation.check_cancelled(attempt.method)?;
                if let Some(kind) = attempt.context.expired_at(Instant::now()) {
                    return Err(RemoteClientError::RequestDeadlineExceeded {
                        method: attempt.method.to_string(),
                        kind,
                    });
                }
            }
            let (io, control) = spawn_child_process_v5_io(&reconnect_command)?;
            let client_hello = protocol_v5::ClientHello::nucleotide(env!("CARGO_PKG_VERSION"));
            let Some(attempt) = attempt else {
                return connect_child_process_v5_client(io, control, client_hello);
            };
            let startup_cancellation = WorkspaceCancellationToken::new();
            let cancel_startup = startup_cancellation.clone();
            attempt
                .cancellation
                .register(move || cancel_startup.cancel());
            let timeout = attempt
                .context
                .absolute_deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(V5_CHILD_HANDSHAKE_TIMEOUT)
                .min(V5_CHILD_HANDSHAKE_TIMEOUT);
            if timeout.is_zero() {
                control.abort();
                return Err(RemoteClientError::RequestDeadlineExceeded {
                    method: attempt.method.to_string(),
                    kind: RemoteRequestDeadlineKind::Absolute,
                });
            }
            let result = connect_child_process_v5_client_with_timeout_and_cancellation(
                io,
                Arc::clone(&control),
                client_hello,
                timeout,
                Some(startup_cancellation),
            );
            if attempt.cancellation.is_cancelled() {
                control.abort();
                return Err(remote_request_cancelled_error(attempt.method));
            }
            if let Some(kind) = attempt.context.expired_at(Instant::now()) {
                control.abort();
                return Err(RemoteClientError::RequestDeadlineExceeded {
                    method: attempt.method.to_string(),
                    kind,
                });
            }
            result
        });
    let backend =
        RemoteWorkspaceBackendImpl::from_protocol_client(identity.clone(), reconnecting_client);
    if let Err(error) = startup.check() {
        drop(backend);
        return Err(error);
    }
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

    async fn stat_with_cancellation(
        &self,
        path: &Path,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "stat",
                path,
                RemoteRequest::Stat {
                    path: path.to_path_buf(),
                },
                Vec::new(),
                cancellation,
            )
            .await?;
        cancellation.check_cancelled("stat", path)?;
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

    async fn list_dir_with_cancellation(
        &self,
        path: &Path,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<DirectoryListing> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "list directory",
                path,
                RemoteRequest::ListDir {
                    path: path.to_path_buf(),
                },
                Vec::new(),
                cancellation,
            )
            .await?;
        cancellation.check_cancelled("list directory", path)?;
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

    async fn find_ancestor_file_with_cancellation(
        &self,
        start: &Path,
        file_name: &str,
        limit: usize,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<Option<PathBuf>> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "find ancestor file",
                start,
                RemoteRequest::FindAncestorFile {
                    start: start.to_path_buf(),
                    file_name: file_name.to_string(),
                    limit,
                },
                Vec::new(),
                cancellation,
            )
            .await?;
        cancellation.check_cancelled("find ancestor file", start)?;
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

    async fn create_file_with_cancellation(
        &self,
        path: &Path,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "create file",
                path,
                RemoteRequest::CreateFile {
                    path: path.to_path_buf(),
                },
                Vec::new(),
                cancellation,
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

    async fn create_dir_with_cancellation(
        &self,
        path: &Path,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "create directory",
                path,
                RemoteRequest::CreateDir {
                    path: path.to_path_buf(),
                },
                Vec::new(),
                cancellation,
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

    async fn rename_path_with_cancellation(
        &self,
        from: &Path,
        to: &Path,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "rename path",
                from,
                RemoteRequest::RenamePath {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                },
                Vec::new(),
                cancellation,
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

    async fn delete_path_with_cancellation(
        &self,
        path: &Path,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "delete path",
                path,
                RemoteRequest::DeletePath {
                    path: path.to_path_buf(),
                },
                Vec::new(),
                cancellation,
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

    async fn copy_path_with_cancellation(
        &self,
        from: &Path,
        to: &Path,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileStat> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "copy path",
                from,
                RemoteRequest::CopyPath {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                },
                Vec::new(),
                cancellation,
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
        self.read_file_stream(path, options)
            .await?
            .collect_file(path)
            .await
    }

    async fn read_file_stream(
        &self,
        path: &Path,
        options: ReadOptions,
    ) -> nucleotide_workspace::Result<FileReadStream> {
        self.read_file_stream_with_optional_workspace_cancellation(path, options, None)
            .await
    }

    async fn read_file_with_cancellation(
        &self,
        path: &Path,
        options: ReadOptions,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileRead> {
        self.read_file_stream_with_cancellation(path, options, cancellation)
            .await?
            .collect_file(path)
            .await
    }

    async fn read_file_stream_with_cancellation(
        &self,
        path: &Path,
        options: ReadOptions,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileReadStream> {
        self.read_file_stream_with_optional_workspace_cancellation(
            path,
            options,
            Some(cancellation),
        )
        .await
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
                    expected_version: options
                        .expected_version
                        .as_ref()
                        .map(|version| version.as_bytes().to_vec()),
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

    async fn write_file_with_cancellation(
        &self,
        path: &Path,
        bytes: &[u8],
        options: WriteOptions,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<WriteResult> {
        let (response, _) = self
            .request_with_workspace_cancellation(
                "write file",
                path,
                RemoteRequest::WriteFile {
                    path: path.to_path_buf(),
                    create_parent_dirs: options.create_parent_dirs,
                    expected_version: options
                        .expected_version
                        .as_ref()
                        .map(|version| version.as_bytes().to_vec()),
                    expected_modified_unix_millis: options
                        .expected_modified
                        .and_then(system_time_unix_millis),
                    expected_modified_unix_nanos: options
                        .expected_modified
                        .and_then(system_time_unix_nanos),
                },
                bytes.to_vec(),
                cancellation,
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
        self.file_search_stream(query)
            .await?
            .collect_search(&root)
            .await
    }

    async fn file_search_stream(
        &self,
        query: FileSearchQuery,
    ) -> nucleotide_workspace::Result<FileSearchStream> {
        self.file_search_stream_with_optional_workspace_cancellation(query, None)
            .await
    }

    async fn file_search_stream_with_cancellation(
        &self,
        query: FileSearchQuery,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<FileSearchStream> {
        self.file_search_stream_with_optional_workspace_cancellation(query, Some(cancellation))
            .await
    }

    async fn text_search(
        &self,
        query: TextSearchQuery,
    ) -> nucleotide_workspace::Result<TextSearchResult> {
        let root = query.root.clone();
        self.text_search_stream(query)
            .await?
            .collect_search(&root)
            .await
    }

    async fn text_search_stream(
        &self,
        query: TextSearchQuery,
    ) -> nucleotide_workspace::Result<TextSearchStream> {
        self.text_search_stream_with_optional_workspace_cancellation(query, None)
            .await
    }

    async fn text_search_stream_with_cancellation(
        &self,
        query: TextSearchQuery,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<TextSearchStream> {
        self.text_search_stream_with_optional_workspace_cancellation(query, Some(cancellation))
            .await
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
        self.run_process_stream(spec)
            .await?
            .collect_output(&cwd)
            .await
    }

    async fn run_process_stream(
        &self,
        spec: ProcessSpec,
    ) -> nucleotide_workspace::Result<ProcessStream> {
        self.run_process_stream_with_optional_workspace_cancellation(spec, None)
            .await
    }

    async fn run_process_stream_with_cancellation(
        &self,
        spec: ProcessSpec,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<ProcessStream> {
        self.run_process_stream_with_optional_workspace_cancellation(spec, Some(cancellation))
            .await
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
