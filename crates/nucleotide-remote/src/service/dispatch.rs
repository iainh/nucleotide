// ABOUTME: WorkspaceService request dispatch and backend operation orchestration
// ABOUTME: Validates workspace paths and maps v5 requests onto workspace capabilities

use super::*;

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
        W: Write + Send + 'static,
    {
        let handshake =
            protocol_v5::server_handshake(&mut io, info).context("v5 handshake failed")?;
        let shared_session = Arc::new(Mutex::new(protocol_v5::ProtocolSession::new(
            protocol_v5::StreamInitiator::Server,
            &handshake.settings,
        )));
        let request_budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
        let parts = io.into_parts();
        let mut inbound_frame_sequence = parts.inbound_frame_sequence;
        let mut requests = HashMap::<u64, V5ServiceRequest>::new();
        let (events_tx, events_rx) = mpsc::channel::<V5ServeEvent>();
        let (inbound_tx, inbound_rx) =
            mpsc::sync_channel::<V5InboundEvent>(V5_SERVE_INBOUND_EVENT_CAPACITY);
        let inbound_events = V5InboundSender::new(inbound_tx, events_tx.clone());
        let reader_events = inbound_events.clone();
        let server_writer = spawn_v5_server_writer(
            parts.writer,
            parts.limits,
            parts.next_frame_sequence,
            Arc::clone(&shared_session),
            events_tx.clone(),
        )?;
        let limits = parts.limits;

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
            let mut shutdown_grace_expired = false;
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
                ($session:expr) => {{
                    while let Some((stream_id, request)) = task_pools.pop_next_startable() {
                        if v5_deadline_expired(request.deadline_unix_ms) {
                            $session
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
                ($session:expr, $event:expr) => {{
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
                                $session
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
                                $session
                                    .send_response_with_priority(
                                        stream_id,
                                        method,
                                        protocol_v5::MessageRole::PartialResult,
                                        false,
                                        priority,
                                    )
                                    .context("failed to queue v5 partial response headers")?;
                                $session
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
                                let priority = $session
                                    .stream_priority(stream_id)
                                    .unwrap_or(protocol_v5::Priority::Background);
                                $session
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
                                $session
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
                                    self.apply_v5_service_terminal($session, completion)?;
                            }
                        }
                    }
                    output_events.mark_delivered();
                }};
            }

            loop {
                if let Err(error) = server_writer.check_failure() {
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
                let writer_drained = server_writer.is_drained();
                if inbound_closed && active_workers == 0 && !task_pools.has_pending() {
                    break;
                }
                if shutdown
                    && active_workers == 0
                    && !task_pools.has_pending()
                    && !output_events.has_pending_output()
                    && writer_drained
                {
                    break;
                }

                let ping_wait = v5_ping_wait_timeout(
                    last_activity,
                    outstanding_ping.as_ref().map(|(_, sent_at)| *sent_at),
                    idle_ping_interval,
                    ping_timeout,
                );
                let service_wait = if inbound_closed || shutdown {
                    ping_wait.min(Duration::from_millis(10))
                } else {
                    ping_wait
                };
                let event = if watches.has_active_watches() {
                    let timeout =
                        if active_workers > 0 || !requests.is_empty() || task_pools.has_pending() {
                            watches.next_poll_timeout().min(Duration::from_millis(10))
                        } else {
                            watches.next_poll_timeout()
                        }
                        .min(service_wait);
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
                    match events_rx.recv_timeout(Duration::from_millis(10).min(service_wait)) {
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
                    match events_rx.recv_timeout(service_wait) {
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

                let mut session = v5_server_session_lock(&shared_session)
                    .context("failed to lock v5 server protocol session")?;
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
                                            } else if let Some(should_shutdown) = self
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
                            session.terminate();
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
                            session.terminate();
                            return Err(error).context("failed to read v5 protocol frame");
                        }
                        V5ServeLoopEvent::Wake(V5ServeEvent::Output) => {
                            output_events.clear_ready();
                            if session.queued_len() < V5_SERVE_SCHEDULER_BACKLOG_LIMIT {
                                match output_rx.try_recv() {
                                    Ok(output_event) => {
                                        let _ = output_events.signal_ready();
                                        apply_v5_output_event!(&mut *session, output_event);
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
                        V5ServeLoopEvent::Wake(V5ServeEvent::Writer) => {
                            if let Err(error) = server_writer.check_failure() {
                                cancel_all_v5_service_work(
                                    &mut requests,
                                    &mut task_pools,
                                    &active_cancellations,
                                    &mut active_deadlines,
                                    &mut canceled_streams,
                                    &mut watches,
                                );
                                session.terminate();
                                return Err(error);
                            }
                        }
                        V5ServeLoopEvent::Wake(V5ServeEvent::Inbound) => {}
                    }
                }

                if inbound_closed {
                    drop(session);
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
                    session.terminate();
                    shutdown_grace_expired = true;
                    break;
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
                    session.terminate();
                    return Err(error);
                }
                drain_v5_service_task_queue!(&mut *session);
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
                    session.terminate();
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
                    session.terminate();
                    return Err(error);
                }
                if session.queued_len() < V5_SERVE_SCHEDULER_BACKLOG_LIMIT
                    && output_events.has_pending_output()
                {
                    let _ = output_events.signal_ready();
                }
                let has_queued_frames = session.queued_len() != 0;
                drop(session);
                if has_queued_frames {
                    server_writer.wake()?;
                }
            }

            if !shutdown_grace_expired {
                server_writer.check_failure()?;
            }
            Ok(())
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

    pub(crate) fn execute_v5_list_dirs_request(
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
        let read_len = v5_streamed_file_read_limit(payload.max_bytes).min(size);
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

    pub(crate) fn enqueue_v5_service_completion(
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

    pub(crate) fn execute(
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
                let max_bytes = Some(v5_streamed_file_read_limit(max_bytes));
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
