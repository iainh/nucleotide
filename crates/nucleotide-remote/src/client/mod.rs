// ABOUTME: Protocol v5 remote workspace clients and multiplexed connection machinery
// ABOUTME: Owns request streams, response routing, flow control, heartbeat, and deadlines

use super::*;

mod connection;
mod response;
mod stream;
mod watch;

pub(crate) use connection::*;
pub use response::*;
pub(crate) use stream::*;
pub(crate) use watch::*;

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

pub(crate) fn remote_request_cancelled_error(method: &str) -> RemoteClientError {
    RemoteClientError::Remote(RemoteError {
        code: protocol_v5::RESET_CANCELLED.to_string(),
        message: format!("remote {method} request cancelled by caller"),
        diagnostic: None,
    })
}

pub struct RemoteWorkspaceV5Client<R, W> {
    io: protocol_v5::FramedIo<R, W>,
    pub(crate) session: protocol_v5::ProtocolSession,
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
    pub(crate) shared: Arc<RemoteWorkspaceV5Shared<W>>,
    _reader: std::marker::PhantomData<fn() -> R>,
}

pub(crate) struct RemoteWorkspaceV5Shared<W> {
    pub(crate) session: Mutex<protocol_v5::ProtocolSession>,
    writer_wake: mpsc::SyncSender<()>,
    pub(crate) heartbeat: Mutex<V5ClientHeartbeat>,
    heartbeat_wake: mpsc::SyncSender<()>,
    deadline_wake: mpsc::SyncSender<()>,
    transport_abort: Option<Arc<dyn V5TransportAbort>>,
    pub(crate) request_budget: V5ConnectionByteBudget,
    pub(crate) response_budget: V5ConnectionByteBudget,
    pub(crate) outbound_request_reservations: Mutex<HashMap<u64, V5ByteReservation>>,
    pub(crate) waiters: Mutex<HashMap<u64, V5PendingResponse>>,
    pub(crate) file_waiters: Mutex<HashMap<u64, V5PendingFileRead>>,
    pub(crate) search_waiters: Mutex<HashMap<u64, V5PendingSearch>>,
    pub(crate) process_waiters: Mutex<HashMap<u64, V5PendingProcess>>,
    pub(crate) completed_file_streams: Mutex<HashMap<u64, Arc<V5FileStreamMailbox>>>,
    pub(crate) completed_search_streams: Mutex<HashMap<u64, Arc<V5SearchStreamMailbox>>>,
    pub(crate) completed_process_streams: Mutex<HashMap<u64, Arc<V5ProcessStreamMailbox>>>,
    pub(crate) raw_waiters: Mutex<HashMap<u64, V5PendingRawResponse>>,
    pub(crate) pending_cancellations: Mutex<HashMap<u64, V5ClientCancellation>>,
    pub(crate) pending_receive_credits: Mutex<HashMap<u64, u64>>,
    file_stream_byte_limit: usize,
    pub(crate) watch_batches: Mutex<HashMap<u64, V5WatchDelivery>>,
    watch_backlog: Mutex<HashMap<u64, VecDeque<protocol_v5::WatchBatch>>>,
    pub(crate) watch_stream_by_id: Mutex<HashMap<u64, u64>>,
    directory_cache: Mutex<HashMap<PathBuf, DirectoryListingResponse>>,
    pub(crate) closed: AtomicBool,
    _writer: std::marker::PhantomData<fn() -> W>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V5ClientCancellationMode {
    Stream,
    Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct V5ClientCancellation {
    method: &'static str,
    mode: V5ClientCancellationMode,
}

#[derive(Clone)]
pub(crate) struct V5WatchDelivery {
    pub(crate) sender: mpsc::SyncSender<protocol_v5::WatchBatch>,
    pub(crate) overflowed: Arc<AtomicBool>,
    pub(crate) last_sequence: Arc<AtomicU64>,
}

pub(crate) struct RemoteWorkspaceV5Writer<W> {
    writer: W,
    limits: protocol_v5::FrameLimits,
    next_frame_sequence: u64,
}

#[derive(Debug)]
pub(crate) struct V5ClientHeartbeat {
    pub(crate) idle_ping_interval: Duration,
    pub(crate) ping_timeout: Duration,
    pub(crate) last_peer_activity: Instant,
    next_ping_id: u64,
    pub(crate) ping: Option<V5ClientPing>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum V5ClientPing {
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
pub(crate) enum V5ClientHeartbeatAction {
    Wait(Duration),
    QueuePing(Vec<u8>),
    TimedOut(&'static str),
}

pub(crate) const V5_CLIENT_PING_WRITE_TIMEOUT: &str =
    "v5 client writer did not send idle PING before timeout";
pub(crate) const V5_CLIENT_PONG_TIMEOUT: &str =
    "v5 peer did not answer client idle PING before timeout";

pub(crate) fn v5_client_heartbeat_timeout(message: &'static str) -> RemoteClientError {
    RemoteClientError::Io(io::Error::new(io::ErrorKind::TimedOut, message))
}

impl V5ClientHeartbeat {
    pub(crate) fn new(settings: &protocol_v5::ConnectionSettings, now: Instant) -> Self {
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

    pub(crate) fn next_action(
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

    pub(crate) fn mark_ping_started(
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

    pub(crate) fn observe_inbound(
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

pub(crate) fn v5_client_pong_control(frame: &protocol_v5::Frame) -> Option<Vec<u8>> {
    (frame.frame_type == protocol_v5::FrameType::Pong).then(|| frame.control.clone())
}

pub struct RemoteWorkspaceV5Watch {
    pub watch_id: u64,
    pub event_stream_id: u64,
    receiver: mpsc::Receiver<protocol_v5::WatchBatch>,
    pub(crate) overflowed: Arc<AtomicBool>,
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

    pub(crate) fn connect_with_transport_abort(
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
            file_waiters: Mutex::new(HashMap::new()),
            search_waiters: Mutex::new(HashMap::new()),
            process_waiters: Mutex::new(HashMap::new()),
            completed_file_streams: Mutex::new(HashMap::new()),
            completed_search_streams: Mutex::new(HashMap::new()),
            completed_process_streams: Mutex::new(HashMap::new()),
            raw_waiters: Mutex::new(HashMap::new()),
            pending_cancellations: Mutex::new(HashMap::new()),
            pending_receive_credits: Mutex::new(HashMap::new()),
            file_stream_byte_limit: usize::try_from(
                if handshake.settings.initial_stream_window == 0 {
                    protocol_v5::DEFAULT_STREAM_WINDOW
                } else {
                    handshake.settings.initial_stream_window
                },
            )
            .unwrap_or(usize::MAX),
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

pub(crate) fn connect_child_process_v5_client(
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

pub(crate) fn connect_child_process_v5_client_with_timeout(
    io: protocol_v5::FramedIo<ChildStdout, ChildProcessV5Writer>,
    control: Arc<ChildProcessV5Control>,
    client_hello: protocol_v5::ClientHello,
    timeout: Duration,
) -> std::result::Result<RemoteWorkspaceV5ChildClient, RemoteClientError> {
    connect_child_process_v5_client_with_timeout_and_cancellation(
        io,
        control,
        client_hello,
        timeout,
        None,
    )
}

pub(crate) fn connect_child_process_v5_client_with_timeout_and_cancellation(
    io: protocol_v5::FramedIo<ChildStdout, ChildProcessV5Writer>,
    control: Arc<ChildProcessV5Control>,
    client_hello: protocol_v5::ClientHello,
    timeout: Duration,
    cancellation: Option<WorkspaceCancellationToken>,
) -> std::result::Result<RemoteWorkspaceV5ChildClient, RemoteClientError> {
    let (watchdog_cancel, watchdog_receiver) = mpsc::channel();
    let watchdog_control = Arc::clone(&control);
    let watchdog = std::thread::Builder::new()
        .name("nucleotide-v5-handshake-watchdog".to_string())
        .spawn(move || {
            let started = Instant::now();
            loop {
                if cancellation
                    .as_ref()
                    .is_some_and(WorkspaceCancellationToken::is_cancelled)
                {
                    tracing::info!(
                        child_id = watchdog_control.child_id(),
                        "Terminating remote service after startup cancellation"
                    );
                    watchdog_control.abort();
                    break;
                }
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    tracing::warn!(
                        child_id = watchdog_control.child_id(),
                        timeout_ms = timeout.as_millis() as u64,
                        "Terminating remote service after v5 handshake timeout"
                    );
                    watchdog_control.abort();
                    break;
                }
                match watchdog_receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
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

    fn read_file_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteFileReadStream, RemoteClientError> {
        self.start_file_read_stream_with_context_and_cancellation(
            request,
            context,
            cancellation.clone(),
        )
    }

    fn file_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteFileSearchStream, RemoteClientError> {
        self.start_file_search_stream_with_context_and_cancellation(
            request,
            context,
            cancellation.clone(),
        )
    }

    fn text_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteTextSearchStream, RemoteClientError> {
        self.start_text_search_stream_with_context_and_cancellation(
            request,
            context,
            cancellation.clone(),
        )
    }

    fn run_process_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        stdin: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<RemoteProcessStream, RemoteClientError> {
        self.start_process_stream_with_context_and_cancellation(
            request,
            stdin,
            context,
            cancellation.clone(),
        )
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
        self.resync_v5_watch_with_cancellation(
            protocol_v5::WatchResync {
                watch_id,
                roots: roots.iter().map(posix_path_string).collect(),
            },
            cancellation,
        )?;
        Ok(())
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
    fn start_process_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        stdin: Vec<u8>,
        context: RemoteRequestContext,
        cancellation: RemoteRequestCancellation,
    ) -> std::result::Result<RemoteProcessStream, RemoteClientError> {
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }
        if !matches!(&request, RemoteRequest::RunProcess(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{} is not a process request",
                request.v5_method()
            )));
        }
        let (method, payload) = self.v5_method_payload_with_directory_cache(&request)?;
        cancellation.check_cancelled(method)?;
        if let Some(kind) = context.expired_at(Instant::now()) {
            return Err(RemoteClientError::RequestDeadlineExceeded {
                method: method.to_string(),
                kind,
            });
        }
        let options = request.v5_request_options_with_context(context);
        let request_reservation = reserve_v5_client_request_bytes(
            &self.shared.request_budget,
            method,
            payload.len(),
            stdin.len(),
        )?;
        let mailbox = Arc::new(V5ProcessStreamMailbox::new(
            self.shared.file_stream_byte_limit,
            self.shared.response_budget.reservation(),
        ));
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
                protocol_v5::DataChannel::Stdin,
                stdin,
            )?;
            let pending = V5PendingProcess {
                mailbox: Arc::clone(&mailbox),
                payload: Vec::new(),
                payload_bytes: 0,
                payload_credit: 0,
                stdout_bytes: 0,
                stderr_bytes: 0,
                received_bytes: 0,
                final_method: None,
                final_error: None,
                method,
                deadline,
            };
            self.shared
                .outbound_request_reservations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(stream_id, request_reservation);
            let mut waiters = match self.shared.process_waiters.lock() {
                Ok(waiters) => waiters,
                Err(error) => {
                    let error = v5_client_lock_error(error);
                    let reset_result = session.reset_stream(
                        stream_id,
                        protocol_v5::RESET_CANCELLED,
                        "client could not register process stream waiter",
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
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }
        self.wake_outbound()?;
        Ok(RemoteProcessStream::new(V5RemoteProcessSource {
            shared: Arc::downgrade(&self.shared),
            mailbox,
            stream_id,
            finished: false,
        })
        .with_terminal_predicate(|event| matches!(event, RemoteProcessEvent::Complete(_)))
        .with_cancellation(cancellation))
    }

    fn start_file_read_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: RemoteRequestCancellation,
    ) -> std::result::Result<RemoteFileReadStream, RemoteClientError> {
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }
        if !matches!(&request, RemoteRequest::ReadFile { .. }) {
            return Err(RemoteClientError::Protocol(format!(
                "{} is not a file-read request",
                request.v5_method()
            )));
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
        let request_reservation =
            reserve_v5_client_request_bytes(&self.shared.request_budget, method, payload.len(), 0)?;
        let mailbox = Arc::new(V5FileStreamMailbox::new(
            self.shared.file_stream_byte_limit,
            self.shared.response_budget.reservation(),
        ));
        let response_reservation = self.shared.response_budget.reservation();
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
                protocol_v5::DataChannel::FileBody,
                Vec::new(),
            )?;
            let pending = V5PendingFileRead {
                mailbox: Arc::clone(&mailbox),
                payload: Vec::new(),
                response_reservation,
                final_method: None,
                final_error: None,
                file_bytes: 0,
                method,
                deadline,
            };
            self.shared
                .outbound_request_reservations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(stream_id, request_reservation);
            let mut waiters = match self.shared.file_waiters.lock() {
                Ok(waiters) => waiters,
                Err(error) => {
                    let error = v5_client_lock_error(error);
                    let reset_result = session.reset_stream(
                        stream_id,
                        protocol_v5::RESET_CANCELLED,
                        "client could not register file stream waiter",
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
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }
        self.wake_outbound()?;
        Ok(RemoteFileReadStream::new(V5RemoteFileReadSource {
            shared: Arc::downgrade(&self.shared),
            mailbox,
            stream_id,
            finished: false,
        })
        .with_terminal_predicate(|event| matches!(event, RemoteFileReadEvent::Complete(_)))
        .with_cancellation(cancellation))
    }

    fn start_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: RemoteRequestCancellation,
    ) -> std::result::Result<(Arc<V5SearchStreamMailbox>, u64), RemoteClientError> {
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }
        if !matches!(
            &request,
            RemoteRequest::FileSearch(_) | RemoteRequest::TextSearch(_)
        ) {
            return Err(RemoteClientError::Protocol(format!(
                "{} is not a search request",
                request.v5_method()
            )));
        }
        let (method, payload) = self.v5_method_payload_with_directory_cache(&request)?;
        cancellation.check_cancelled(method)?;
        if let Some(kind) = context.expired_at(Instant::now()) {
            return Err(RemoteClientError::RequestDeadlineExceeded {
                method: method.to_string(),
                kind,
            });
        }
        let options = request.v5_request_options_with_context(context);
        let request_reservation =
            reserve_v5_client_request_bytes(&self.shared.request_budget, method, payload.len(), 0)?;
        let mailbox = Arc::new(V5SearchStreamMailbox::new(
            self.shared.file_stream_byte_limit,
            self.shared.response_budget.reservation(),
        ));
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
                protocol_v5::DataChannel::SearchPayload,
                Vec::new(),
            )?;
            let pending = V5PendingSearch {
                mailbox: Arc::clone(&mailbox),
                current_method: None,
                current_payload: Vec::new(),
                current_credit: 0,
                current_bytes: 0,
                final_method: None,
                final_payload: Vec::new(),
                final_credit: 0,
                final_bytes: 0,
                final_error: None,
                received_bytes: 0,
                method,
                deadline,
            };
            self.shared
                .outbound_request_reservations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(stream_id, request_reservation);
            let mut waiters = match self.shared.search_waiters.lock() {
                Ok(waiters) => waiters,
                Err(error) => {
                    let error = v5_client_lock_error(error);
                    let reset_result = session.reset_stream(
                        stream_id,
                        protocol_v5::RESET_CANCELLED,
                        "client could not register search stream waiter",
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
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(RemoteClientError::Disconnected);
        }
        self.wake_outbound()?;
        Ok((mailbox, stream_id))
    }

    fn start_file_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: RemoteRequestCancellation,
    ) -> std::result::Result<RemoteFileSearchStream, RemoteClientError> {
        if !matches!(&request, RemoteRequest::FileSearch(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{} is not a file-search request",
                request.v5_method()
            )));
        }
        let (mailbox, stream_id) = self.start_search_stream_with_context_and_cancellation(
            request,
            context,
            cancellation.clone(),
        )?;
        Ok(RemoteFileSearchStream::new(V5RemoteFileSearchSource {
            shared: Arc::downgrade(&self.shared),
            mailbox,
            stream_id,
            finished: false,
        })
        .with_terminal_predicate(|event| matches!(event, RemoteFileSearchEvent::Complete { .. }))
        .with_cancellation(cancellation))
    }

    fn start_text_search_stream_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        context: RemoteRequestContext,
        cancellation: RemoteRequestCancellation,
    ) -> std::result::Result<RemoteTextSearchStream, RemoteClientError> {
        if !matches!(&request, RemoteRequest::TextSearch(_)) {
            return Err(RemoteClientError::Protocol(format!(
                "{} is not a text-search request",
                request.v5_method()
            )));
        }
        let (mailbox, stream_id) = self.start_search_stream_with_context_and_cancellation(
            request,
            context,
            cancellation.clone(),
        )?;
        Ok(RemoteTextSearchStream::new(V5RemoteTextSearchSource {
            shared: Arc::downgrade(&self.shared),
            mailbox,
            stream_id,
            finished: false,
        })
        .with_terminal_predicate(|event| matches!(event, RemoteTextSearchEvent::Complete { .. }))
        .with_cancellation(cancellation))
    }

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
        self.resync_v5_watch_with_cancellation(request, &RemoteRequestCancellation::new())
    }

    fn resync_v5_watch_with_cancellation(
        &self,
        request: protocol_v5::WatchResync,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<protocol_v5::WatchResyncResponse, RemoteClientError> {
        let payload = self.request_v5_raw_with_cancellation(
            "watch.resync",
            request.encode_to_vec(),
            v5_watch_control_request_context(),
            cancellation,
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

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn request_v5_raw(
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
