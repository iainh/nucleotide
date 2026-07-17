// ABOUTME: Remote protocol client abstraction and reconnecting request/watch wrapper
// ABOUTME: Centralizes replay safety, transport healing, and watch restoration

use super::*;

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

    fn read_file_stream(
        &self,
        path: PathBuf,
        max_bytes: Option<u64>,
    ) -> std::result::Result<RemoteFileReadStream, RemoteClientError> {
        let request = RemoteRequest::ReadFile { path, max_bytes };
        let context = request.v5_request_context();
        self.read_file_stream_with_context_and_cancellation(
            request,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn read_file_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteFileReadStream, RemoteClientError> {
        let method = request.v5_method();
        if !matches!(&request, RemoteRequest::ReadFile { .. }) {
            return Err(RemoteClientError::Protocol(format!(
                "{method} is not a file-read request"
            )));
        }
        cancellation.check_cancelled(method)?;
        let (response, body) =
            self.request_with_context_and_cancellation(request, Vec::new(), context, cancellation)?;
        cancellation.check_cancelled(method)?;
        match response {
            RemoteResponse::ReadFile(response) => {
                RemoteFileReadStream::from_response(response, body)
                    .map(|stream| stream.with_cancellation(cancellation.clone()))
            }
            other => Err(RemoteClientError::Protocol(format!(
                "unexpected read file response: {other:?}"
            ))),
        }
    }

    fn file_search_stream(
        &self,
        request: FileSearchRequest,
    ) -> std::result::Result<RemoteFileSearchStream, RemoteClientError> {
        let request = RemoteRequest::FileSearch(request);
        let context = request.v5_request_context();
        self.file_search_stream_with_context_and_cancellation(
            request,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn file_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteFileSearchStream, RemoteClientError> {
        let method = request.v5_method();
        if !matches!(&request, RemoteRequest::FileSearch(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{method} is not a file-search request"
            )));
        }
        cancellation.check_cancelled(method)?;
        let (response, _) =
            self.request_with_context_and_cancellation(request, Vec::new(), context, cancellation)?;
        cancellation.check_cancelled(method)?;
        match response {
            RemoteResponse::FileSearch(response) => {
                Ok(RemoteFileSearchStream::from_response(response)
                    .with_cancellation(cancellation.clone()))
            }
            other => Err(RemoteClientError::Protocol(format!(
                "unexpected file-search response: {other:?}"
            ))),
        }
    }

    fn text_search_stream(
        &self,
        request: TextSearchRequest,
    ) -> std::result::Result<RemoteTextSearchStream, RemoteClientError> {
        let request = RemoteRequest::TextSearch(request);
        let context = request.v5_request_context();
        self.text_search_stream_with_context_and_cancellation(
            request,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn text_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteTextSearchStream, RemoteClientError> {
        let method = request.v5_method();
        if !matches!(&request, RemoteRequest::TextSearch(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{method} is not a text-search request"
            )));
        }
        cancellation.check_cancelled(method)?;
        let (response, _) =
            self.request_with_context_and_cancellation(request, Vec::new(), context, cancellation)?;
        cancellation.check_cancelled(method)?;
        match response {
            RemoteResponse::TextSearch(response) => {
                Ok(RemoteTextSearchStream::from_response(response)
                    .with_cancellation(cancellation.clone()))
            }
            other => Err(RemoteClientError::Protocol(format!(
                "unexpected text-search response: {other:?}"
            ))),
        }
    }

    fn run_process_stream(
        &self,
        request: ProcessRequest,
        stdin: Vec<u8>,
    ) -> std::result::Result<RemoteProcessStream, RemoteClientError> {
        let request = RemoteRequest::RunProcess(request);
        let context = request.v5_request_context();
        self.run_process_stream_with_context_and_cancellation(
            request,
            stdin,
            context,
            &RemoteRequestCancellation::new(),
        )
    }

    fn run_process_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        stdin: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteProcessStream, RemoteClientError> {
        let method = request.v5_method();
        if !matches!(&request, RemoteRequest::RunProcess(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{method} is not a process request"
            )));
        }
        cancellation.check_cancelled(method)?;
        let (response, body) =
            self.request_with_context_and_cancellation(request, stdin, context, cancellation)?;
        cancellation.check_cancelled(method)?;
        match response {
            RemoteResponse::RunProcess(response) => {
                RemoteProcessStream::from_response(response, body)
                    .map(|stream| stream.with_cancellation(cancellation.clone()))
            }
            other => Err(RemoteClientError::Protocol(format!(
                "unexpected process response: {other:?}"
            ))),
        }
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

    fn resync_watch(
        &self,
        _watch_id: u64,
        _roots: Vec<PathBuf>,
    ) -> std::result::Result<(), RemoteClientError> {
        Err(RemoteClientError::Protocol(
            "remote protocol client does not support watch resync".to_string(),
        ))
    }

    fn resync_watch_with_cancellation(
        &self,
        watch_id: u64,
        roots: Vec<PathBuf>,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        cancellation.check_cancelled("watch.resync")?;
        let result = self.resync_watch(watch_id, roots);
        cancellation.check_cancelled("watch.resync")?;
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

#[derive(Clone)]
pub(crate) struct ReconnectAttempt {
    pub(crate) method: &'static str,
    pub(crate) context: RemoteRequestContext,
    pub(crate) cancellation: RemoteRequestCancellation,
}

pub(crate) fn check_reconnect_attempt(
    attempt: Option<&ReconnectAttempt>,
) -> std::result::Result<(), RemoteClientError> {
    let Some(attempt) = attempt else {
        return Ok(());
    };
    attempt.cancellation.check_cancelled(attempt.method)?;
    if let Some(kind) = attempt.context.expired_at(Instant::now()) {
        return Err(RemoteClientError::RequestDeadlineExceeded {
            method: attempt.method.to_string(),
            kind,
        });
    }
    Ok(())
}

pub(crate) type ReconnectFactory<C> = dyn Fn(Option<ReconnectAttempt>) -> std::result::Result<C, RemoteClientError>
    + Send
    + Sync
    + 'static;

pub(crate) const RECONNECTING_WATCH_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub(crate) const RECONNECTING_WATCH_RETRY_MIN_DELAY: Duration = Duration::from_millis(100);
pub(crate) const RECONNECTING_WATCH_RETRY_MAX_DELAY: Duration = Duration::from_secs(2);

pub(crate) struct ReconnectingWatchRegistration {
    logical_watch_id: u64,
    desired: Mutex<WorkspaceWatchRequest>,
    physical_watch_id: Mutex<Option<u64>>,
    operation_gate: Mutex<()>,
    next_sequence: AtomicU64,
    stopped: AtomicBool,
    sender: mpsc::SyncSender<WorkspaceWatchBatch>,
}

#[derive(Default)]
pub(crate) struct ReconnectingWatchRegistry {
    next_watch_id: AtomicU64,
    registrations: Mutex<HashMap<u64, Arc<ReconnectingWatchRegistration>>>,
}

pub struct ReconnectingRemoteWorkspaceProtocolClient<C: RemoteWorkspaceProtocolClient> {
    client: Arc<Mutex<Option<Arc<C>>>>,
    reconnect_gate: Arc<Mutex<()>>,
    reconnect: Arc<ReconnectFactory<C>>,
    closed: Arc<AtomicBool>,
    watches: Arc<ReconnectingWatchRegistry>,
}

impl<C> ReconnectingRemoteWorkspaceProtocolClient<C>
where
    C: RemoteWorkspaceProtocolClient + 'static,
{
    pub fn new(
        client: C,
        reconnect: impl Fn() -> std::result::Result<C, RemoteClientError> + Send + Sync + 'static,
    ) -> Self {
        Self::new_with_attempt(client, move |_| reconnect())
    }

    pub(crate) fn new_with_attempt(
        client: C,
        reconnect: impl Fn(Option<ReconnectAttempt>) -> std::result::Result<C, RemoteClientError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            client: Arc::new(Mutex::new(Some(Arc::new(client)))),
            reconnect_gate: Arc::new(Mutex::new(())),
            reconnect: Arc::new(reconnect),
            closed: Arc::new(AtomicBool::new(false)),
            watches: Arc::new(ReconnectingWatchRegistry::default()),
        }
    }

    fn shared_handle(&self) -> Self {
        Self {
            client: Arc::clone(&self.client),
            reconnect_gate: Arc::clone(&self.reconnect_gate),
            reconnect: Arc::clone(&self.reconnect),
            closed: Arc::clone(&self.closed),
            watches: Arc::clone(&self.watches),
        }
    }

    fn current_client(&self) -> std::result::Result<Arc<C>, RemoteClientError> {
        self.current_client_with_attempt(None)
    }

    fn current_client_for_request(
        &self,
        method: &'static str,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Arc<C>, RemoteClientError> {
        self.current_client_with_attempt(Some(ReconnectAttempt {
            method,
            context,
            cancellation: cancellation.clone(),
        }))
    }

    fn current_client_with_attempt(
        &self,
        attempt: Option<ReconnectAttempt>,
    ) -> std::result::Result<Arc<C>, RemoteClientError> {
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

        check_reconnect_attempt(attempt.as_ref())?;
        let reconnected = Arc::new((self.reconnect)(attempt.clone())?);
        if let Err(error) = check_reconnect_attempt(attempt.as_ref()) {
            reconnected.close();
            return Err(error);
        }
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

    fn reconnect_if_current_for_request(
        &self,
        stale: &Arc<C>,
        method: &'static str,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Arc<C>, RemoteClientError> {
        self.reconnect_if_current_with_attempt(
            stale,
            Some(ReconnectAttempt {
                method,
                context,
                cancellation: cancellation.clone(),
            }),
        )
    }

    fn reconnect_if_current_with_attempt(
        &self,
        stale: &Arc<C>,
        attempt: Option<ReconnectAttempt>,
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
        check_reconnect_attempt(attempt.as_ref())?;
        let reconnected = Arc::new((self.reconnect)(attempt.clone())?);
        if let Err(error) = check_reconnect_attempt(attempt.as_ref()) {
            reconnected.close();
            return Err(error);
        }
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

    fn start_physical_watch_with_context_and_cancellation(
        &self,
        request: WorkspaceWatchRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        cancellation.check_cancelled("watch.start")?;
        let client = self.current_client_for_request("watch.start", context, cancellation)?;
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
                tracing::warn!(error = %error, "Retrying v5 watch.start after reconnect");
                let retry_client = self.reconnect_if_current_for_request(
                    &client,
                    "watch.start",
                    context,
                    cancellation,
                )?;
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

    fn update_physical_watch_with_cancellation(
        &self,
        watch_id: u64,
        add_roots: Vec<PathBuf>,
        remove_roots: Vec<PathBuf>,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
        cancellation.check_cancelled("watch.update")?;
        let context = v5_watch_control_request_context();
        let client = self.current_client_for_request("watch.update", context, cancellation)?;
        cancellation.check_cancelled("watch.update")?;
        match client.update_watch_with_cancellation(watch_id, add_roots, remove_roots, cancellation)
        {
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled("watch.update")?;
                if let Err(reconnect_error) = self.reconnect_if_current_for_request(
                    &client,
                    "watch.update",
                    context,
                    cancellation,
                ) {
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

    fn resync_physical_watch_with_cancellation(
        &self,
        watch_id: u64,
        roots: Vec<PathBuf>,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        cancellation.check_cancelled("watch.resync")?;
        let context = v5_watch_control_request_context();
        let client = self.current_client_for_request("watch.resync", context, cancellation)?;
        cancellation.check_cancelled("watch.resync")?;
        match client.resync_watch_with_cancellation(watch_id, roots, cancellation) {
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled("watch.resync")?;
                if let Err(reconnect_error) = self.reconnect_if_current_for_request(
                    &client,
                    "watch.resync",
                    context,
                    cancellation,
                ) {
                    tracing::warn!(
                        error = %reconnect_error,
                        original_error = %error,
                        "Failed to heal v5 transport after watch.resync failure"
                    );
                }
                Err(error)
            }
            result => {
                cancellation.check_cancelled("watch.resync")?;
                result
            }
        }
    }

    fn stop_physical_watch_with_cancellation(
        &self,
        watch_id: u64,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        cancellation.check_cancelled("watch.stop")?;
        let context = v5_watch_control_request_context();
        let client = self.current_client_for_request("watch.stop", context, cancellation)?;
        cancellation.check_cancelled("watch.stop")?;
        match client.stop_watch_with_cancellation(watch_id, cancellation) {
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled("watch.stop")?;
                if let Err(reconnect_error) = self.reconnect_if_current_for_request(
                    &client,
                    "watch.stop",
                    context,
                    cancellation,
                ) {
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

    fn register_reconnecting_watch(
        &self,
        request: WorkspaceWatchRequest,
        physical_watch: WorkspaceWatch,
    ) -> std::result::Result<WorkspaceWatch, RemoteClientError> {
        let physical_watch_id = physical_watch.watch_id;
        let logical_watch_id = self
            .watches
            .next_watch_id
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        if logical_watch_id == 0 {
            return Err(RemoteClientError::Protocol(
                "remote logical watch id space exhausted".to_string(),
            ));
        }
        let (sender, receiver) = mpsc::sync_channel(V5_WATCH_DELIVERY_CAPACITY);
        let registration = Arc::new(ReconnectingWatchRegistration {
            logical_watch_id,
            desired: Mutex::new(request),
            physical_watch_id: Mutex::new(Some(physical_watch.watch_id)),
            operation_gate: Mutex::new(()),
            next_sequence: AtomicU64::new(1),
            stopped: AtomicBool::new(false),
            sender,
        });
        self.watches
            .registrations
            .lock()
            .map_err(|_| {
                RemoteClientError::Protocol(
                    "remote reconnect watch registry lock is poisoned".to_string(),
                )
            })?
            .insert(logical_watch_id, Arc::clone(&registration));

        let reconnecting_client = self.shared_handle();
        if let Err(source) = std::thread::Builder::new()
            .name("nucleotide-v5-watch-reconnect".to_string())
            .spawn(move || {
                run_reconnecting_workspace_watch(reconnecting_client, registration, physical_watch);
            })
        {
            self.watches
                .registrations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&logical_watch_id);
            let _ = self.stop_physical_watch_with_cancellation(
                physical_watch_id,
                &RemoteRequestCancellation::new(),
            );
            return Err(RemoteClientError::Io(source));
        }

        Ok(WorkspaceWatch::new(
            logical_watch_id,
            logical_watch_id,
            receiver,
        ))
    }

    fn watch_registration(
        &self,
        logical_watch_id: u64,
    ) -> Option<Arc<ReconnectingWatchRegistration>> {
        self.watches
            .registrations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&logical_watch_id)
            .cloned()
    }

    fn remove_watch_registration(
        &self,
        logical_watch_id: u64,
    ) -> Option<Arc<ReconnectingWatchRegistration>> {
        self.watches
            .registrations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&logical_watch_id)
    }

    fn restore_reconnecting_watch(
        &self,
        registration: &Arc<ReconnectingWatchRegistration>,
    ) -> std::result::Result<(WorkspaceWatch, WorkspaceWatchBatch), RemoteClientError> {
        let _operation = registration.operation_gate.lock().map_err(|_| {
            RemoteClientError::Protocol("remote watch operation lock is poisoned".to_string())
        })?;
        if registration.stopped.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
            return Err(RemoteClientError::Disconnected);
        }

        let desired = registration
            .desired
            .lock()
            .map_err(|_| {
                RemoteClientError::Protocol(
                    "remote watch desired roots lock is poisoned".to_string(),
                )
            })?
            .clone();
        let cancellation = RemoteRequestCancellation::new();
        let Some(watch) = self.start_physical_watch_with_context_and_cancellation(
            desired.clone(),
            v5_watch_control_request_context(),
            &cancellation,
        )?
        else {
            return Err(RemoteClientError::Protocol(
                "remote watch capability disappeared after reconnect".to_string(),
            ));
        };

        if registration.stopped.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
            let _ = self.stop_physical_watch_with_cancellation(watch.watch_id, &cancellation);
            return Err(RemoteClientError::Disconnected);
        }
        if let Err(error) = self.resync_physical_watch_with_cancellation(
            watch.watch_id,
            desired.roots,
            &cancellation,
        ) {
            let _ = self.stop_physical_watch_with_cancellation(watch.watch_id, &cancellation);
            return Err(error);
        }

        let barrier_deadline = Instant::now() + V5_REQUEST_CONTROL_DEADLINE;
        loop {
            if registration.stopped.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
                let _ = self.stop_physical_watch_with_cancellation(watch.watch_id, &cancellation);
                return Err(RemoteClientError::Disconnected);
            }
            match watch.recv_timeout(RECONNECTING_WATCH_POLL_INTERVAL) {
                Ok(batch) if batch.resync_required => {
                    *registration.physical_watch_id.lock().map_err(|_| {
                        RemoteClientError::Protocol(
                            "remote physical watch id lock is poisoned".to_string(),
                        )
                    })? = Some(watch.watch_id);
                    return Ok((watch, batch));
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < barrier_deadline => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ =
                        self.stop_physical_watch_with_cancellation(watch.watch_id, &cancellation);
                    return Err(RemoteClientError::RequestDeadlineExceeded {
                        method: "watch.resync".to_string(),
                        kind: RemoteRequestDeadlineKind::Absolute,
                    });
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(RemoteClientError::Disconnected);
                }
            }
        }
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
        let client = self.current_client_for_request(method, context, cancellation)?;
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
                let recovery =
                    self.reconnect_if_current_for_request(&client, method, context, cancellation);
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

    fn read_file_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteFileReadStream, RemoteClientError> {
        let method = request.v5_method();
        if !matches!(&request, RemoteRequest::ReadFile { .. }) {
            return Err(RemoteClientError::Protocol(format!(
                "{method} is not a file-read request"
            )));
        }
        cancellation.check_cancelled(method)?;
        let client = self.current_client_for_request(method, context, cancellation)?;
        cancellation.check_cancelled(method)?;
        match client.read_file_stream_with_context_and_cancellation(
            request.clone(),
            context,
            cancellation,
        ) {
            Ok(stream) => {
                let reconnecting = self.shared_handle();
                let stale = Arc::clone(&client);
                Ok(stream.with_terminal_error_callback(move || {
                    if let Err(error) = reconnecting.discard_if_current(&stale) {
                        tracing::warn!(
                            error = %error,
                            "Failed to invalidate v5 transport after file stream failure"
                        );
                    }
                }))
            }
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled(method)?;
                let retry_safe = remote_client_error_allows_reconnect_retry(&error);
                let retry_client =
                    self.reconnect_if_current_for_request(&client, method, context, cancellation)?;
                cancellation.check_cancelled(method)?;
                if !retry_safe {
                    return Err(error);
                }
                if let Some(kind) = context.expired_at(Instant::now()) {
                    return Err(RemoteClientError::RequestDeadlineExceeded {
                        method: method.to_string(),
                        kind,
                    });
                }
                let stream = retry_client.read_file_stream_with_context_and_cancellation(
                    request,
                    context,
                    cancellation,
                )?;
                let reconnecting = self.shared_handle();
                let stale = Arc::clone(&retry_client);
                Ok(stream.with_terminal_error_callback(move || {
                    if let Err(error) = reconnecting.discard_if_current(&stale) {
                        tracing::warn!(
                            error = %error,
                            "Failed to invalidate replayed v5 file stream transport"
                        );
                    }
                }))
            }
            Err(error) => Err(error),
        }
    }

    fn file_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteFileSearchStream, RemoteClientError> {
        let method = request.v5_method();
        if !matches!(&request, RemoteRequest::FileSearch(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{method} is not a file-search request"
            )));
        }
        cancellation.check_cancelled(method)?;
        let client = self.current_client_for_request(method, context, cancellation)?;
        cancellation.check_cancelled(method)?;
        match client.file_search_stream_with_context_and_cancellation(
            request.clone(),
            context,
            cancellation,
        ) {
            Ok(stream) => {
                let reconnecting = self.shared_handle();
                let stale = Arc::clone(&client);
                Ok(stream.with_terminal_error_callback(move || {
                    if let Err(error) = reconnecting.discard_if_current(&stale) {
                        tracing::warn!(
                            error = %error,
                            "Failed to invalidate v5 transport after file-search stream failure"
                        );
                    }
                }))
            }
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled(method)?;
                let retry_safe = remote_client_error_allows_reconnect_retry(&error);
                let retry_client =
                    self.reconnect_if_current_for_request(&client, method, context, cancellation)?;
                cancellation.check_cancelled(method)?;
                if !retry_safe {
                    return Err(error);
                }
                if let Some(kind) = context.expired_at(Instant::now()) {
                    return Err(RemoteClientError::RequestDeadlineExceeded {
                        method: method.to_string(),
                        kind,
                    });
                }
                let stream = retry_client.file_search_stream_with_context_and_cancellation(
                    request,
                    context,
                    cancellation,
                )?;
                let reconnecting = self.shared_handle();
                let stale = Arc::clone(&retry_client);
                Ok(stream.with_terminal_error_callback(move || {
                    if let Err(error) = reconnecting.discard_if_current(&stale) {
                        tracing::warn!(
                            error = %error,
                            "Failed to invalidate replayed v5 file-search stream transport"
                        );
                    }
                }))
            }
            Err(error) => Err(error),
        }
    }

    fn text_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteTextSearchStream, RemoteClientError> {
        let method = request.v5_method();
        if !matches!(&request, RemoteRequest::TextSearch(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{method} is not a text-search request"
            )));
        }
        cancellation.check_cancelled(method)?;
        let client = self.current_client_for_request(method, context, cancellation)?;
        cancellation.check_cancelled(method)?;
        match client.text_search_stream_with_context_and_cancellation(
            request.clone(),
            context,
            cancellation,
        ) {
            Ok(stream) => {
                let reconnecting = self.shared_handle();
                let stale = Arc::clone(&client);
                Ok(stream.with_terminal_error_callback(move || {
                    if let Err(error) = reconnecting.discard_if_current(&stale) {
                        tracing::warn!(
                            error = %error,
                            "Failed to invalidate v5 transport after text-search stream failure"
                        );
                    }
                }))
            }
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled(method)?;
                let retry_safe = remote_client_error_allows_reconnect_retry(&error);
                let retry_client =
                    self.reconnect_if_current_for_request(&client, method, context, cancellation)?;
                cancellation.check_cancelled(method)?;
                if !retry_safe {
                    return Err(error);
                }
                if let Some(kind) = context.expired_at(Instant::now()) {
                    return Err(RemoteClientError::RequestDeadlineExceeded {
                        method: method.to_string(),
                        kind,
                    });
                }
                let stream = retry_client.text_search_stream_with_context_and_cancellation(
                    request,
                    context,
                    cancellation,
                )?;
                let reconnecting = self.shared_handle();
                let stale = Arc::clone(&retry_client);
                Ok(stream.with_terminal_error_callback(move || {
                    if let Err(error) = reconnecting.discard_if_current(&stale) {
                        tracing::warn!(
                            error = %error,
                            "Failed to invalidate replayed v5 text-search stream transport"
                        );
                    }
                }))
            }
            Err(error) => Err(error),
        }
    }

    fn run_process_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        stdin: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteProcessStream, RemoteClientError> {
        let method = request.v5_method();
        if !matches!(&request, RemoteRequest::RunProcess(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{method} is not a process request"
            )));
        }
        cancellation.check_cancelled(method)?;
        let client = self.current_client_for_request(method, context, cancellation)?;
        cancellation.check_cancelled(method)?;
        match client.run_process_stream_with_context_and_cancellation(
            request,
            stdin,
            context,
            cancellation,
        ) {
            Ok(stream) => {
                let reconnecting = self.shared_handle();
                let stale = Arc::clone(&client);
                Ok(stream.with_terminal_error_callback(move || {
                    if let Err(error) = reconnecting.discard_if_current(&stale) {
                        tracing::warn!(
                            error = %error,
                            "Failed to invalidate v5 transport after process stream failure"
                        );
                    }
                }))
            }
            Err(error) if remote_client_error_requires_reconnect(&error) => {
                cancellation.check_cancelled(method)?;
                if let Err(reconnect_error) =
                    self.reconnect_if_current_for_request(&client, method, context, cancellation)
                {
                    tracing::warn!(
                        error = %reconnect_error,
                        original_error = %error,
                        "Failed to heal v5 transport after process stream open failure"
                    );
                }
                Err(RemoteClientError::OutcomeUnknown {
                    method: method.to_string(),
                    cause: error.to_string(),
                })
            }
            Err(error) => Err(error),
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
        let registrations = self
            .watches
            .registrations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain()
            .map(|(_, registration)| registration)
            .collect::<Vec<_>>();
        for registration in registrations {
            registration.stopped.store(true, Ordering::Release);
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
        let physical_watch = self.start_physical_watch_with_context_and_cancellation(
            request.clone(),
            context,
            cancellation,
        )?;
        match physical_watch {
            Some(physical_watch) => self
                .register_reconnecting_watch(request, physical_watch)
                .map(Some),
            None => Ok(None),
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
        let Some(registration) = self.watch_registration(watch_id) else {
            return self.update_physical_watch_with_cancellation(
                watch_id,
                add_roots,
                remove_roots,
                cancellation,
            );
        };
        let _operation = registration.operation_gate.lock().map_err(|_| {
            RemoteClientError::Protocol("remote watch operation lock is poisoned".to_string())
        })?;
        if registration.stopped.load(Ordering::Acquire) {
            return Err(RemoteClientError::Disconnected);
        }
        {
            let mut desired = registration.desired.lock().map_err(|_| {
                RemoteClientError::Protocol(
                    "remote watch desired roots lock is poisoned".to_string(),
                )
            })?;
            desired
                .roots
                .retain(|root| !remove_roots.iter().any(|removed| removed == root));
            for root in &add_roots {
                if !desired.roots.contains(root) {
                    desired.roots.push(root.clone());
                }
            }
        }
        let physical_watch_id = registration
            .physical_watch_id
            .lock()
            .map_err(|_| {
                RemoteClientError::Protocol("remote physical watch id lock is poisoned".to_string())
            })?
            .ok_or(RemoteClientError::Disconnected)?;
        self.update_physical_watch_with_cancellation(
            physical_watch_id,
            add_roots,
            remove_roots,
            cancellation,
        )
        .map(|update| {
            update.map(|mut update| {
                update.watch_id = watch_id;
                update
            })
        })
    }

    fn resync_watch(
        &self,
        watch_id: u64,
        roots: Vec<PathBuf>,
    ) -> std::result::Result<(), RemoteClientError> {
        self.resync_watch_with_cancellation(watch_id, roots, &RemoteRequestCancellation::new())
    }

    fn resync_watch_with_cancellation(
        &self,
        watch_id: u64,
        roots: Vec<PathBuf>,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        self.resync_physical_watch_with_cancellation(watch_id, roots, cancellation)
    }

    fn stop_watch(&self, watch_id: u64) -> std::result::Result<(), RemoteClientError> {
        self.stop_watch_with_cancellation(watch_id, &RemoteRequestCancellation::new())
    }

    fn stop_watch_with_cancellation(
        &self,
        watch_id: u64,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(), RemoteClientError> {
        let Some(registration) = self.remove_watch_registration(watch_id) else {
            return self.stop_physical_watch_with_cancellation(watch_id, cancellation);
        };
        registration.stopped.store(true, Ordering::Release);
        let _operation = registration.operation_gate.lock().map_err(|_| {
            RemoteClientError::Protocol("remote watch operation lock is poisoned".to_string())
        })?;
        let physical_watch_id = registration
            .physical_watch_id
            .lock()
            .map_err(|_| {
                RemoteClientError::Protocol("remote physical watch id lock is poisoned".to_string())
            })?
            .take();
        match physical_watch_id {
            Some(physical_watch_id) => {
                self.stop_physical_watch_with_cancellation(physical_watch_id, cancellation)
            }
            None => Ok(()),
        }
    }
}

pub(crate) fn run_reconnecting_workspace_watch<C>(
    client: ReconnectingRemoteWorkspaceProtocolClient<C>,
    registration: Arc<ReconnectingWatchRegistration>,
    mut physical_watch: WorkspaceWatch,
) where
    C: RemoteWorkspaceProtocolClient + 'static,
{
    let mut retry_delay = RECONNECTING_WATCH_RETRY_MIN_DELAY;
    'watch: loop {
        if registration.stopped.load(Ordering::Acquire) || client.closed.load(Ordering::Acquire) {
            break;
        }
        match physical_watch.recv_timeout(RECONNECTING_WATCH_POLL_INTERVAL) {
            Ok(mut batch) => {
                batch.watch_id = registration.logical_watch_id;
                batch.sequence = registration.next_sequence.fetch_add(1, Ordering::AcqRel);
                if registration.sender.send(batch).is_err() {
                    registration.stopped.store(true, Ordering::Release);
                    break;
                }
                retry_delay = RECONNECTING_WATCH_RETRY_MIN_DELAY;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                *registration
                    .physical_watch_id
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                loop {
                    if registration.stopped.load(Ordering::Acquire)
                        || client.closed.load(Ordering::Acquire)
                    {
                        break 'watch;
                    }
                    match client.restore_reconnecting_watch(&registration) {
                        Ok((watch, mut resync_batch)) => {
                            resync_batch.watch_id = registration.logical_watch_id;
                            resync_batch.sequence =
                                registration.next_sequence.fetch_add(1, Ordering::AcqRel);
                            if registration.sender.send(resync_batch).is_err() {
                                registration.stopped.store(true, Ordering::Release);
                                break 'watch;
                            }
                            physical_watch = watch;
                            retry_delay = RECONNECTING_WATCH_RETRY_MIN_DELAY;
                            continue 'watch;
                        }
                        Err(error) if remote_watch_restore_error_is_retryable(&error) => {
                            tracing::warn!(
                                error = %error,
                                logical_watch_id = registration.logical_watch_id,
                                retry_ms = retry_delay.as_millis() as u64,
                                "Retrying remote watch restoration"
                            );
                            let retry_at = Instant::now() + retry_delay;
                            while Instant::now() < retry_at {
                                if registration.stopped.load(Ordering::Acquire)
                                    || client.closed.load(Ordering::Acquire)
                                {
                                    break 'watch;
                                }
                                std::thread::sleep(
                                    RECONNECTING_WATCH_POLL_INTERVAL
                                        .min(retry_at.saturating_duration_since(Instant::now())),
                                );
                            }
                            retry_delay = retry_delay
                                .saturating_mul(2)
                                .min(RECONNECTING_WATCH_RETRY_MAX_DELAY);
                        }
                        Err(error) => {
                            tracing::error!(
                                error = %error,
                                logical_watch_id = registration.logical_watch_id,
                                "Remote watch restoration failed permanently"
                            );
                            registration.stopped.store(true, Ordering::Release);
                            break 'watch;
                        }
                    }
                }
            }
        }
    }

    client.remove_watch_registration(registration.logical_watch_id);
}

pub(crate) fn remote_watch_restore_error_is_retryable(error: &RemoteClientError) -> bool {
    match error {
        RemoteClientError::Disconnected
        | RemoteClientError::TransportClosed { .. }
        | RemoteClientError::RequestDeadlineExceeded { .. }
        | RemoteClientError::OutcomeUnknown { .. }
        | RemoteClientError::ResponseIncomplete { .. } => true,
        RemoteClientError::Io(error) => remote_io_error_is_transport_failure(error),
        RemoteClientError::Json(_)
        | RemoteClientError::Protocol(_)
        | RemoteClientError::Remote(_) => false,
    }
}

pub(crate) fn remote_client_error_allows_reconnect_retry(error: &RemoteClientError) -> bool {
    match error {
        RemoteClientError::Disconnected | RemoteClientError::TransportClosed { .. } => true,
        RemoteClientError::Io(error) => remote_io_error_is_transport_failure(error),
        RemoteClientError::Json(_)
        | RemoteClientError::RequestDeadlineExceeded { .. }
        | RemoteClientError::OutcomeUnknown { .. }
        | RemoteClientError::ResponseIncomplete { .. }
        | RemoteClientError::Protocol(_)
        | RemoteClientError::Remote(_) => false,
    }
}

pub(crate) fn remote_client_error_requires_reconnect(error: &RemoteClientError) -> bool {
    remote_client_error_allows_reconnect_retry(error)
        || matches!(
            error,
            RemoteClientError::OutcomeUnknown { .. } | RemoteClientError::ResponseIncomplete { .. }
        )
}

fn remote_io_error_is_transport_failure(error: &io::Error) -> bool {
    // Protocol-session capacity and control-budget limits use OutOfMemory as a local resource
    // signal. Replacing the SSH transport cannot create capacity and instead causes every
    // queued read to replay at once, amplifying pressure into a reconnect storm.
    error.kind() != io::ErrorKind::OutOfMemory
}

pub(crate) fn transport_closed_before_final_error(error: RemoteClientError) -> RemoteClientError {
    match error {
        RemoteClientError::Json(_) | RemoteClientError::Protocol(_) => {
            RemoteClientError::TransportClosed {
                cause: error.to_string(),
            }
        }
        error => error,
    }
}

pub(crate) fn disconnect_after_final_response_error(error: RemoteClientError) -> RemoteClientError {
    match error {
        RemoteClientError::Remote(_) | RemoteClientError::ResponseIncomplete { .. } => error,
        error => RemoteClientError::ResponseIncomplete {
            cause: error.to_string(),
        },
    }
}
