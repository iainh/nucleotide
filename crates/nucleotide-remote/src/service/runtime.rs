// ABOUTME: Concurrent protocol v5 server runtime, queues, scheduling, and serialization
// ABOUTME: Coordinates service workers, transport writes, cancellation, and shutdown

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct V5ServerWriterFailure {
    kind: io::ErrorKind,
    message: String,
}

impl V5ServerWriterFailure {
    fn from_io(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }

    fn to_io(&self) -> io::Error {
        io::Error::new(self.kind, self.message.clone())
    }
}

pub(crate) struct V5ServerWriterHandle {
    wakes: mpsc::SyncSender<u64>,
    requested_generation: Arc<AtomicU64>,
    completed_generation: Arc<AtomicU64>,
    in_flight_frames: Arc<AtomicU64>,
    failure: Arc<Mutex<Option<V5ServerWriterFailure>>>,
}

impl V5ServerWriterHandle {
    pub(crate) fn wake(&self) -> Result<()> {
        let requested = self.requested_generation.load(Ordering::Acquire);
        let generation = requested
            .checked_add(1)
            .context("v5 server writer wake generation exhausted")?;
        match self.wakes.try_send(generation) {
            Ok(()) => {
                self.requested_generation
                    .store(generation, Ordering::Release);
                Ok(())
            }
            Err(mpsc::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.check_failure()?;
                Err(anyhow::anyhow!("v5 server writer stopped unexpectedly"))
            }
        }
    }

    pub(crate) fn is_drained(&self) -> bool {
        self.in_flight_frames.load(Ordering::Acquire) == 0
            && self.completed_generation.load(Ordering::Acquire)
                >= self.requested_generation.load(Ordering::Acquire)
    }

    pub(crate) fn check_failure(&self) -> Result<()> {
        let failure = self
            .failure
            .lock()
            .map_err(|_| anyhow::anyhow!("v5 server writer failure lock poisoned"))?
            .clone();
        match failure {
            Some(failure) => Err(failure.to_io()).context("v5 server writer failed"),
            None => Ok(()),
        }
    }
}

pub(crate) fn v5_server_session_lock(
    session: &Arc<Mutex<protocol_v5::ProtocolSession>>,
) -> io::Result<std::sync::MutexGuard<'_, protocol_v5::ProtocolSession>> {
    session
        .lock()
        .map_err(|_| io::Error::other("v5 server session lock poisoned"))
}

pub(crate) fn spawn_v5_server_writer<W>(
    writer: W,
    limits: protocol_v5::FrameLimits,
    next_frame_sequence: u64,
    session: Arc<Mutex<protocol_v5::ProtocolSession>>,
    events: mpsc::Sender<V5ServeEvent>,
) -> Result<V5ServerWriterHandle>
where
    W: Write + Send + 'static,
{
    let (wake_tx, wake_rx) = mpsc::sync_channel(1);
    let requested_generation = Arc::new(AtomicU64::new(0));
    let completed_generation = Arc::new(AtomicU64::new(0));
    let in_flight_frames = Arc::new(AtomicU64::new(0));
    let failure = Arc::new(Mutex::new(None));

    let writer_completed_generation = Arc::clone(&completed_generation);
    let writer_in_flight_frames = Arc::clone(&in_flight_frames);
    let writer_failure = Arc::clone(&failure);
    std::thread::Builder::new()
        .name("nucleotide-v5-server-writer".to_string())
        .spawn(move || {
            run_v5_server_writer(
                writer,
                limits,
                next_frame_sequence,
                wake_rx,
                &session,
                &writer_completed_generation,
                &writer_in_flight_frames,
                &writer_failure,
                &events,
            );
        })
        .context("failed to spawn v5 server writer")?;

    Ok(V5ServerWriterHandle {
        wakes: wake_tx,
        requested_generation,
        completed_generation,
        in_flight_frames,
        failure,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_v5_server_writer<W>(
    mut writer: W,
    limits: protocol_v5::FrameLimits,
    mut next_frame_sequence: u64,
    wakes: mpsc::Receiver<u64>,
    session: &Arc<Mutex<protocol_v5::ProtocolSession>>,
    completed_generation: &AtomicU64,
    in_flight_frames: &AtomicU64,
    failure: &Mutex<Option<V5ServerWriterFailure>>,
    events: &mpsc::Sender<V5ServeEvent>,
) where
    W: Write,
{
    while let Ok(generation) = wakes.recv() {
        match write_v5_server_outbound(
            &mut writer,
            limits,
            &mut next_frame_sequence,
            session,
            in_flight_frames,
        ) {
            Ok(()) => completed_generation.store(generation, Ordering::Release),
            Err(error) => {
                if let Ok(mut stored) = failure.lock() {
                    *stored = Some(V5ServerWriterFailure::from_io(&error));
                }
                if let Ok(mut session) = v5_server_session_lock(session) {
                    session.terminate();
                }
                let _ = events.send(V5ServeEvent::Writer);
                break;
            }
        }
    }
}

pub(crate) fn write_v5_server_outbound<W>(
    writer: &mut W,
    limits: protocol_v5::FrameLimits,
    next_frame_sequence: &mut u64,
    session: &Arc<Mutex<protocol_v5::ProtocolSession>>,
    in_flight_frames: &AtomicU64,
) -> io::Result<()>
where
    W: Write,
{
    loop {
        let mut processed_frames = 0_usize;
        let mut written_frames = Vec::with_capacity(V5_SERVER_WRITE_BATCH_FRAMES);
        while processed_frames < V5_SERVER_WRITE_BATCH_FRAMES {
            let frame = {
                let mut session = v5_server_session_lock(session)?;
                let Some(frame) = session.pop_next_frame()? else {
                    break;
                };
                processed_frames += 1;
                if !session.should_write_frame(&frame) {
                    session.discard_unwritten_frame(&frame)?;
                    continue;
                }
                in_flight_frames.fetch_add(1, Ordering::AcqRel);
                frame
            };

            let mut frame = frame;
            frame.frame_sequence = *next_frame_sequence;
            *next_frame_sequence = next_frame_sequence
                .checked_add(1)
                .ok_or_else(|| io::Error::other("v5 frame sequence exhausted"))?;
            protocol_v5::write_frame_unflushed_with_limits(writer, &frame, limits)?;
            written_frames.push((frame.stream_id, frame.frame_type));
        }

        if !written_frames.is_empty() {
            writer.flush()?;
            let mut session = v5_server_session_lock(session)?;
            for (stream_id, frame_type) in &written_frames {
                session.observe_frame_parts_written(*stream_id, *frame_type);
            }
            in_flight_frames.fetch_sub(written_frames.len() as u64, Ordering::AcqRel);
        }
        if processed_frames < V5_SERVER_WRITE_BATCH_FRAMES {
            return Ok(());
        }
    }
}

pub fn serve_local_workspace_v5<R, W>(workspace_root: PathBuf, reader: R, writer: W) -> Result<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let info = protocol_v5::ServerHandshakeInfo::current(workspace_root.display().to_string());
    let io = protocol_v5::FramedIo::new(reader, writer);
    WorkspaceService::new(LocalWorkspaceBackend, workspace_root)?.serve_v5_concurrent(io, &info)
}

pub(crate) enum ServiceOutcome {
    Continue {
        response: Box<RemoteResponse>,
        body: Vec<u8>,
    },
    Shutdown,
}

impl ServiceOutcome {
    pub(crate) fn continue_response(response: RemoteResponse, body: Vec<u8>) -> Self {
        Self::Continue {
            response: Box::new(response),
            body,
        }
    }
}

pub(crate) struct V5ServiceCompletion {
    pub(crate) stream_id: u64,
    pub(crate) method: String,
    pub(crate) result: std::result::Result<ServiceOutcome, RemoteError>,
}

pub(crate) struct V5ServiceTerminal {
    pub(crate) stream_id: u64,
    pub(crate) method: String,
    pub(crate) result: std::result::Result<V5ServiceTerminalOutcome, RemoteError>,
}

pub(crate) enum V5ServiceTerminalOutcome {
    Continue,
    Shutdown,
}

pub(crate) enum V5ServeEvent {
    Inbound,
    Output,
    NativeWatch,
    Writer,
    WorkerFinished {
        stream_id: u64,
        terminal_queued: bool,
    },
}

pub(crate) enum V5ServeLoopEvent {
    Inbound(V5InboundEvent),
    Wake(V5ServeEvent),
}

pub(crate) enum V5ServeOutputEvent {
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
pub(crate) enum V5ServeQueueError {
    Closed,
    Cancelled,
    EventTooLarge { retained_bytes: usize, max: usize },
}

#[derive(Clone)]
pub(crate) struct V5ServeOutputSender {
    sender: mpsc::SyncSender<V5ServeOutputEvent>,
    ready_events: mpsc::Sender<V5ServeEvent>,
    ready: Arc<AtomicBool>,
    pub(crate) pending_count: Arc<AtomicU64>,
    pub(crate) completion_budget: V5ConnectionByteBudget,
}

impl V5ServeOutputSender {
    pub(crate) fn new(
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
    pub(crate) fn with_completion_budget(
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

    pub(crate) fn reserve_completion_bytes(
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

    pub(crate) fn send(
        &self,
        event: V5ServeOutputEvent,
    ) -> std::result::Result<(), V5ServeQueueError> {
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

    pub(crate) fn send_with_cancellation(
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

    pub(crate) fn signal_ready(&self) -> std::result::Result<(), V5ServeQueueError> {
        if !self.ready.swap(true, Ordering::AcqRel)
            && self.ready_events.send(V5ServeEvent::Output).is_err()
        {
            self.ready.store(false, Ordering::Release);
            return Err(V5ServeQueueError::Closed);
        }
        Ok(())
    }

    pub(crate) fn clear_ready(&self) {
        self.ready.store(false, Ordering::Release);
    }

    pub(crate) fn has_pending_output(&self) -> bool {
        self.pending_count.load(Ordering::Acquire) != 0
    }

    pub(crate) fn mark_delivered(&self) {
        self.pending_count.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct V5SerializedByteCounter<'a> {
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

pub(crate) fn v5_serialized_response_len(
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

pub(crate) struct V5SerializedResponseWriter<'a> {
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

pub(crate) fn v5_serialize_response_to_output(
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

pub(crate) fn v5_cancelled_response_error() -> RemoteError {
    RemoteError {
        code: protocol_v5::RESET_CANCELLED.to_string(),
        message: "v5 response production cancelled".to_string(),
        diagnostic: None,
    }
}

pub(crate) fn v5_response_size_overflow_error() -> RemoteError {
    RemoteError {
        code: "resource_exhausted".to_string(),
        message: "v5 response size overflowed the server byte counter".to_string(),
        diagnostic: None,
    }
}

impl V5ServeOutputEvent {
    pub(crate) fn retained_bytes(&self) -> usize {
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

pub(crate) fn v5_queue_error_to_remote_error(error: V5ServeQueueError) -> RemoteError {
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

pub(crate) fn v5_bound_terminal_error(mut error: RemoteError) -> RemoteError {
    error.code = v5_bounded_terminal_string(error.code, 256);
    error.message = v5_bounded_terminal_string(error.message, 16 * 1024);
    error.diagnostic = error
        .diagnostic
        .map(|diagnostic| v5_bounded_terminal_string(diagnostic, 32 * 1024));
    error
}

pub(crate) fn v5_bounded_terminal_string(mut value: String, max_bytes: usize) -> String {
    if value.len() > max_bytes {
        let mut boundary = max_bytes;
        while boundary > 0 && !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value.truncate(boundary);
    }
    value.into_boxed_str().into_string()
}

pub(crate) type V5InboundEvent = io::Result<Option<protocol_v5::Frame>>;

#[derive(Clone)]
pub(crate) struct V5InboundSender {
    sender: mpsc::SyncSender<V5InboundEvent>,
    ready_events: mpsc::Sender<V5ServeEvent>,
    ready: Arc<AtomicBool>,
}

impl V5InboundSender {
    pub(crate) fn new(
        sender: mpsc::SyncSender<V5InboundEvent>,
        ready_events: mpsc::Sender<V5ServeEvent>,
    ) -> Self {
        Self {
            sender,
            ready_events,
            ready: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn send(&self, event: V5InboundEvent) -> std::result::Result<(), V5ServeQueueError> {
        self.sender
            .send(event)
            .map_err(|_| V5ServeQueueError::Closed)?;
        self.signal_ready()
    }

    pub(crate) fn signal_ready(&self) -> std::result::Result<(), V5ServeQueueError> {
        if !self.ready.swap(true, Ordering::AcqRel)
            && self.ready_events.send(V5ServeEvent::Inbound).is_err()
        {
            self.ready.store(false, Ordering::Release);
            return Err(V5ServeQueueError::Closed);
        }
        Ok(())
    }

    pub(crate) fn clear_ready(&self) {
        self.ready.store(false, Ordering::Release);
    }
}

pub(crate) struct V5NativeWatchEvent {
    pub(crate) watch_id: u64,
    pub(crate) result: notify::Result<notify::Event>,
}

#[derive(Clone)]
pub(crate) struct V5NativeWatchSender {
    sender: mpsc::SyncSender<V5NativeWatchEvent>,
    ready_events: mpsc::Sender<V5ServeEvent>,
    ready: Arc<AtomicBool>,
    overflowed_watch_ids: Arc<Mutex<HashSet<u64>>>,
}

impl V5NativeWatchSender {
    pub(crate) fn new(
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

    pub(crate) fn send(
        &self,
        event: V5NativeWatchEvent,
    ) -> std::result::Result<(), V5ServeQueueError> {
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

    pub(crate) fn signal_ready(&self) -> std::result::Result<(), V5ServeQueueError> {
        if !self.ready.swap(true, Ordering::AcqRel)
            && self.ready_events.send(V5ServeEvent::NativeWatch).is_err()
        {
            self.ready.store(false, Ordering::Release);
            return Err(V5ServeQueueError::Closed);
        }
        Ok(())
    }

    pub(crate) fn clear_ready(&self) {
        self.ready.store(false, Ordering::Release);
    }

    pub(crate) fn take_overflowed_watch_ids(&self) -> Vec<u64> {
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
pub(crate) enum V5QueuedStreamEvent {
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
pub(crate) struct V5ServiceRequest {
    pub(crate) method: String,
    pub(crate) priority: protocol_v5::Priority,
    pub(crate) payload: Vec<u8>,
    pub(crate) body: Vec<u8>,
    pub(crate) retained_bytes: V5ByteReservation,
    pub(crate) received_payload_bytes: usize,
    pub(crate) received_body_bytes: usize,
    pub(crate) deadline_unix_ms: u64,
    pub(crate) supersedes_stream_id: u64,
    pub(crate) streamed_write: Option<V5StreamingWrite>,
    pub(crate) early_error: Option<RemoteError>,
}

impl V5ServiceRequest {
    pub(crate) fn from_envelope(
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

    pub(crate) fn append_data(&mut self, channel: protocol_v5::DataChannel, bytes: Vec<u8>) {
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

    pub(crate) fn reserve_data(
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

pub(crate) fn v5_request_size_error(method: &str, label: &str, limit: usize) -> RemoteError {
    RemoteError {
        code: "resource_exhausted".to_string(),
        message: format!("v5 {method} request {label} exceeds decoded byte limit {limit}"),
        diagnostic: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V5ServiceTaskClass {
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
pub(crate) struct V5ServiceTaskPools {
    active_by_class: [usize; 5],
    pending: VecDeque<(u64, V5ServiceRequest)>,
}

impl V5ServiceTaskPools {
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn can_start_method(&self, method: &str) -> bool {
        let class = V5ServiceTaskClass::for_method(method);
        self.active_by_class[class.index()] < class.limit()
    }

    pub(crate) fn can_start(&self, request: &V5ServiceRequest) -> bool {
        self.can_start_method(&request.method)
    }

    pub(crate) fn mark_started(&mut self, method: &str) -> V5ServiceTaskClass {
        let class = V5ServiceTaskClass::for_method(method);
        self.active_by_class[class.index()] += 1;
        class
    }

    pub(crate) fn mark_finished(&mut self, class: V5ServiceTaskClass) {
        let active = &mut self.active_by_class[class.index()];
        *active = active.saturating_sub(1);
    }

    pub(crate) fn enqueue(&mut self, stream_id: u64, request: V5ServiceRequest) {
        self.pending.push_back((stream_id, request));
    }

    pub(crate) fn remove_pending(&mut self, stream_id: u64) -> bool {
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

    pub(crate) fn expired_pending_streams(&self, now_unix_ms: u64) -> Vec<u64> {
        self.pending
            .iter()
            .filter_map(|(stream_id, request)| {
                (request.deadline_unix_ms != 0 && request.deadline_unix_ms <= now_unix_ms)
                    .then_some(*stream_id)
            })
            .collect()
    }

    pub(crate) fn pop_next_startable(&mut self) -> Option<(u64, V5ServiceRequest)> {
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

pub(crate) fn v5_now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

pub(crate) fn v5_deadline_expired(deadline_unix_ms: u64) -> bool {
    deadline_unix_ms != 0 && deadline_unix_ms <= v5_now_unix_millis()
}

pub(crate) fn cancel_all_v5_service_work(
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

pub(crate) fn v5_ping_wait_timeout(
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

pub(crate) fn drive_v5_idle_ping(
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

pub(crate) fn expire_v5_service_deadlines(
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

pub(crate) struct V5ServiceCancellationState<'a> {
    pub(crate) task_pools: &'a mut V5ServiceTaskPools,
    pub(crate) active_cancellations: &'a HashMap<u64, WorkspaceCancellationToken>,
    pub(crate) active_deadlines: &'a mut HashMap<u64, u64>,
    pub(crate) canceled_streams: &'a mut HashSet<u64>,
}

pub(crate) fn reset_v5_service_stream(
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
