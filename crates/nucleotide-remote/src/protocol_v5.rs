// ABOUTME: Version 5 remote protocol frame and control-message primitives
// ABOUTME: Provides multiplexed transport foundations without changing v4 service behaviour

use prost::Message;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};

pub const PROTOCOL_MAJOR: u32 = 5;
pub const PROTOCOL_MINOR: u32 = 0;
pub const FRAME_MAGIC: [u8; 4] = *b"NUC2";
pub const FRAME_HEADER_VERSION: u16 = 2;
pub const FRAME_HEADER_LEN: usize = 48;
pub const DEFAULT_MAX_FRAME_BODY_LEN: u32 = 64 * 1024;
pub const MAX_NEGOTIATED_FRAME_BODY_LEN: u32 = 1024 * 1024;
pub const DEFAULT_MAX_CONTROL_LEN: u32 = 64 * 1024;
pub const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024;
pub const DEFAULT_CONNECTION_WINDOW: u32 = 4 * 1024 * 1024;
pub const MAX_FLOW_WINDOW: u64 = u32::MAX as u64;
pub const DEFAULT_MAX_CONCURRENT_STREAMS: u32 = 128;
pub const DEFAULT_CONNECTION_CONTROL_BUDGET: u32 = 1024 * 1024;
pub const DEFAULT_STREAM_CONTROL_BUDGET: u32 = 256 * 1024;
pub const DEFAULT_SHUTDOWN_GRACE_MS: u32 = 5_000;
pub const IDLE_PING_INTERVAL_MS: u32 = 30_000;
pub const PING_TIMEOUT_MS: u32 = 90_000;
pub const MIN_UNSOLICITED_PING_INTERVAL_MS: u32 = 5_000;
pub const RESET_CANCELLED: &str = "CANCELLED";
pub const RESET_DEADLINE_EXCEEDED: &str = "DEADLINE_EXCEEDED";
pub const RESET_RESOURCE_EXHAUSTED: &str = "RESOURCE_EXHAUSTED";
pub const RESET_UNAVAILABLE: &str = "UNAVAILABLE";
const STREAM_IDS_EXHAUSTED_MESSAGE: &str = "v5 stream ids exhausted";
const PRIORITY_LEVELS: usize = 6;
const ZSTD_DATA_COMPRESSION_LEVEL: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimits {
    pub max_control_len: u32,
    pub max_body_len: u32,
}

impl Default for FrameLimits {
    fn default() -> Self {
        Self {
            max_control_len: DEFAULT_MAX_CONTROL_LEN,
            max_body_len: DEFAULT_MAX_FRAME_BODY_LEN,
        }
    }
}

impl FrameLimits {
    pub fn negotiated(max_control_len: u32, max_body_len: u32) -> Self {
        Self {
            max_control_len: nonzero_or(max_control_len, DEFAULT_MAX_CONTROL_LEN)
                .min(DEFAULT_MAX_CONTROL_LEN),
            max_body_len: nonzero_or(max_body_len, DEFAULT_MAX_FRAME_BODY_LEN)
                .min(MAX_NEGOTIATED_FRAME_BODY_LEN),
        }
    }

    pub fn from_settings(settings: &ConnectionSettings) -> Self {
        Self::negotiated(settings.max_control_len, settings.max_frame_body)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamInitiator {
    Client,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamIdAllocator {
    next: u64,
}

impl StreamIdAllocator {
    pub fn new(initiator: StreamInitiator) -> Self {
        let next = match initiator {
            StreamInitiator::Client => 1,
            StreamInitiator::Server => 2,
        };
        Self { next }
    }

    pub fn next_id(&mut self) -> Option<u64> {
        let id = self.next;
        if id == 0 {
            return None;
        }
        self.next = id.checked_add(2).unwrap_or(0);
        Some(id)
    }

    pub fn peek(&self) -> Option<u64> {
        (self.next != 0).then_some(self.next)
    }

    #[cfg(test)]
    fn with_next_for_test(next: u64) -> Self {
        Self { next }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FrameType {
    Hello = 1,
    Settings = 2,
    SettingsAck = 3,
    Headers = 4,
    Data = 5,
    EndStream = 6,
    ResetStream = 7,
    WindowUpdate = 8,
    Ping = 9,
    Pong = 10,
    GoAway = 11,
}

impl TryFrom<u16> for FrameType {
    type Error = io::Error;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::Settings),
            3 => Ok(Self::SettingsAck),
            4 => Ok(Self::Headers),
            5 => Ok(Self::Data),
            6 => Ok(Self::EndStream),
            7 => Ok(Self::ResetStream),
            8 => Ok(Self::WindowUpdate),
            9 => Ok(Self::Ping),
            10 => Ok(Self::Pong),
            11 => Ok(Self::GoAway),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown v5 frame type: {value}"),
            )),
        }
    }
}

impl FrameType {
    pub fn consumes_flow_window(self) -> bool {
        matches!(self, Self::Data)
    }

    pub fn is_connection_control(self) -> bool {
        matches!(
            self,
            Self::Hello
                | Self::Settings
                | Self::SettingsAck
                | Self::WindowUpdate
                | Self::Ping
                | Self::Pong
                | Self::GoAway
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: FrameType,
    pub flags: u16,
    pub priority: u8,
    pub stream_id: u64,
    pub frame_sequence: u64,
    pub control: Vec<u8>,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn new(frame_type: FrameType, stream_id: u64) -> Self {
        Self {
            frame_type,
            flags: 0,
            priority: 0,
            stream_id,
            frame_sequence: 0,
            control: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn from_control<T: Message>(frame_type: FrameType, stream_id: u64, control: &T) -> Self {
        Self {
            frame_type,
            flags: 0,
            priority: 0,
            stream_id,
            frame_sequence: 0,
            control: control.encode_to_vec(),
            body: Vec::new(),
        }
    }

    pub fn decode_control<T: Message + Default>(&self) -> Result<T, prost::DecodeError> {
        T::decode(self.control.as_slice())
    }

    pub fn flow_control_len(&self) -> u64 {
        if self.frame_type.consumes_flow_window() {
            self.body.len() as u64
        } else {
            0
        }
    }

    pub fn control_budget_len(&self) -> u64 {
        if self.frame_type.consumes_flow_window() {
            0
        } else {
            FRAME_HEADER_LEN as u64 + self.control.len() as u64 + self.body.len() as u64
        }
    }

    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority.as_u8();
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowWindow {
    available: u64,
}

impl FlowWindow {
    pub fn new(initial_credit: u64) -> Self {
        Self {
            available: initial_credit.min(MAX_FLOW_WINDOW),
        }
    }

    pub fn available(&self) -> u64 {
        self.available
    }

    pub fn consume(&mut self, bytes: u64) -> io::Result<()> {
        if bytes > self.available {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "insufficient v5 flow-control credit: need {bytes}, have {}",
                    self.available
                ),
            ));
        }
        self.available -= bytes;
        Ok(())
    }

    pub fn consume_frame(&mut self, frame: &Frame) -> io::Result<()> {
        self.consume(frame.flow_control_len())
    }

    pub fn grant(&mut self, bytes: u64) -> io::Result<()> {
        self.available = self.available.checked_add(bytes).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "v5 flow-control window overflow",
            )
        })?;
        if self.available > MAX_FLOW_WINDOW {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "v5 flow-control window exceeds maximum",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Open,
    HalfClosedLocal,
    HalfClosedRemote,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEntry {
    pub stream_id: u64,
    pub method: String,
    pub initiator: StreamInitiator,
    pub state: StreamState,
    pub final_seen: bool,
    pub request_id: u64,
    pub cancellation_group: String,
    pub content_encoding: ContentEncoding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOptions {
    pub priority: Priority,
    pub cancellation_group: String,
    pub deadline_unix_ms: u64,
    pub supersedes_stream_id: u64,
    pub content_encoding: ContentEncoding,
    pub idempotency: Idempotency,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            priority: Priority::Background,
            cancellation_group: String::new(),
            deadline_unix_ms: 0,
            supersedes_stream_id: 0,
            content_encoding: ContentEncoding::None,
            idempotency: Idempotency::ReadOnly,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutedFrame {
    ConnectionControl {
        frame_type: FrameType,
    },
    WindowUpdate {
        stream_id: u64,
        credit_bytes: u64,
    },
    Headers {
        stream_id: u64,
        role: MessageRole,
        method: String,
    },
    Data {
        stream_id: u64,
        flow_control_len: u64,
    },
    EndStream {
        stream_id: u64,
        state: StreamState,
    },
    RejectedStream {
        stream_id: u64,
    },
    ResetStream {
        stream_id: u64,
        known: bool,
    },
}

#[derive(Debug, Clone)]
pub struct StreamTable {
    local_initiator: StreamInitiator,
    allocator: StreamIdAllocator,
    max_concurrent_streams: usize,
    streams: HashMap<u64, StreamEntry>,
    last_accepted_remote_stream_id: u64,
}

impl StreamTable {
    pub fn new(local_initiator: StreamInitiator, settings: &ConnectionSettings) -> Self {
        Self {
            local_initiator,
            allocator: StreamIdAllocator::new(local_initiator),
            max_concurrent_streams: nonzero_or(
                settings.max_concurrent_streams,
                DEFAULT_MAX_CONCURRENT_STREAMS,
            ) as usize,
            streams: HashMap::new(),
            last_accepted_remote_stream_id: 0,
        }
    }

    pub fn active_streams(&self) -> usize {
        self.streams.len()
    }

    pub fn get(&self, stream_id: u64) -> Option<&StreamEntry> {
        self.streams.get(&stream_id)
    }

    pub fn last_accepted_remote_stream_id(&self) -> u64 {
        self.last_accepted_remote_stream_id
    }

    pub fn open_request(&mut self, method: impl Into<String>) -> io::Result<(u64, Frame)> {
        self.open_request_with_options(method, RequestOptions::default())
    }

    pub fn open_request_with_options(
        &mut self,
        method: impl Into<String>,
        options: RequestOptions,
    ) -> io::Result<(u64, Frame)> {
        let stream_id = self
            .allocator
            .next_id()
            .ok_or_else(|| io::Error::other(STREAM_IDS_EXHAUSTED_MESSAGE))?;
        let method = method.into();
        let envelope = StreamEnvelope::request_with_options(stream_id, method.clone(), &options);
        self.insert_stream(StreamEntry::new(
            stream_id,
            method,
            self.local_initiator,
            stream_id,
            options.cancellation_group.clone(),
            options.content_encoding,
        ))?;
        Ok((
            stream_id,
            Frame::from_control(FrameType::Headers, stream_id, &envelope)
                .with_priority(options.priority),
        ))
    }

    pub fn open_event_stream(
        &mut self,
        method: impl Into<String>,
        watch_id: u64,
    ) -> io::Result<(u64, Frame)> {
        let stream_id = self
            .allocator
            .next_id()
            .ok_or_else(|| io::Error::other(STREAM_IDS_EXHAUSTED_MESSAGE))?;
        let method = method.into();
        let envelope = StreamEnvelope::event(stream_id, method.clone(), watch_id);
        self.insert_stream(StreamEntry::new(
            stream_id,
            method,
            self.local_initiator,
            0,
            String::new(),
            ContentEncoding::None,
        ))?;
        Ok((
            stream_id,
            Frame::from_control(FrameType::Headers, stream_id, &envelope)
                .with_priority(Priority::VisibleFileTree),
        ))
    }

    pub fn mark_local_end(&mut self, stream_id: u64) -> io::Result<StreamState> {
        let entry = self.stream_entry_mut(stream_id)?;
        entry.state = match entry.state {
            StreamState::Open => StreamState::HalfClosedLocal,
            StreamState::HalfClosedRemote => StreamState::Closed,
            StreamState::HalfClosedLocal | StreamState::Closed => {
                return Err(protocol_error(format!(
                    "local side already closed v5 stream {stream_id}"
                )));
            }
        };
        let state = entry.state;
        if state == StreamState::Closed {
            self.streams.remove(&stream_id);
        }
        Ok(state)
    }

    pub fn route_incoming(&mut self, frame: &Frame) -> io::Result<RoutedFrame> {
        match frame.frame_type {
            FrameType::Hello
            | FrameType::Settings
            | FrameType::SettingsAck
            | FrameType::Ping
            | FrameType::Pong
            | FrameType::GoAway => {
                if frame.stream_id != 0 {
                    return Err(protocol_error(format!(
                        "{:?} must use stream 0",
                        frame.frame_type
                    )));
                }
                Ok(RoutedFrame::ConnectionControl {
                    frame_type: frame.frame_type,
                })
            }
            FrameType::WindowUpdate => {
                let update = decode_control::<WindowUpdate>(frame)?;
                Ok(RoutedFrame::WindowUpdate {
                    stream_id: frame.stream_id,
                    credit_bytes: update.credit_bytes,
                })
            }
            FrameType::Headers => self.route_headers(frame),
            FrameType::Data => self.route_data(frame),
            FrameType::EndStream => self.route_end_stream(frame),
            FrameType::ResetStream => self.route_reset_stream(frame),
        }
    }

    fn route_headers(&mut self, frame: &Frame) -> io::Result<RoutedFrame> {
        require_nonzero_stream(frame)?;
        let envelope = decode_control::<StreamEnvelope>(frame)?;
        let role = envelope.message_role()?;

        if self.streams.contains_key(&frame.stream_id) {
            self.route_existing_headers(frame.stream_id, &envelope, role)
        } else {
            self.route_opening_headers(frame.stream_id, envelope, role)
        }
    }

    fn route_opening_headers(
        &mut self,
        stream_id: u64,
        envelope: StreamEnvelope,
        role: MessageRole,
    ) -> io::Result<RoutedFrame> {
        match role {
            MessageRole::Request => {
                if envelope.request_id != stream_id {
                    return Err(protocol_error(format!(
                        "request_id {} does not match opening stream {stream_id}",
                        envelope.request_id
                    )));
                }
            }
            MessageRole::Event => {
                if envelope.request_id != 0 && envelope.request_id != stream_id {
                    return Err(protocol_error(format!(
                        "event request_id {} does not match stream {stream_id}",
                        envelope.request_id
                    )));
                }
            }
            MessageRole::PartialResult
            | MessageRole::FinalResponse
            | MessageRole::FinalError
            | MessageRole::Progress => {
                return Err(protocol_error(format!(
                    "{role:?} cannot open v5 stream {stream_id}"
                )));
            }
        }

        let content_encoding = envelope.decode_content_encoding()?;
        let initiator = remote_initiator(self.local_initiator);
        let actual_initiator = stream_id_initiator(stream_id)?;
        if actual_initiator != initiator {
            return Err(protocol_error(format!(
                "{actual_initiator:?} stream id {stream_id} cannot be opened by {initiator:?}"
            )));
        }
        self.insert_stream(StreamEntry::new(
            stream_id,
            envelope.method.clone(),
            initiator,
            envelope.request_id,
            envelope.cancellation_group.clone(),
            content_encoding,
        ))?;
        self.last_accepted_remote_stream_id = self.last_accepted_remote_stream_id.max(stream_id);
        Ok(RoutedFrame::Headers {
            stream_id,
            role,
            method: envelope.method,
        })
    }

    fn route_existing_headers(
        &mut self,
        stream_id: u64,
        envelope: &StreamEnvelope,
        role: MessageRole,
    ) -> io::Result<RoutedFrame> {
        let entry = self.stream_entry_mut(stream_id)?;
        if matches!(
            entry.state,
            StreamState::HalfClosedRemote | StreamState::Closed
        ) {
            return Err(protocol_error(format!(
                "received headers after remote side closed v5 stream {stream_id}"
            )));
        }
        if entry.final_seen {
            return Err(protocol_error(format!(
                "received headers after final response on v5 stream {stream_id}"
            )));
        }
        if role == MessageRole::Request {
            return Err(protocol_error(format!(
                "received duplicate request headers on v5 stream {stream_id}"
            )));
        }
        if envelope.decode_content_encoding()? != ContentEncoding::None {
            return Err(protocol_error(format!(
                "content_encoding can only be set on opening headers for v5 stream {stream_id}"
            )));
        }
        if role.is_final() {
            entry.final_seen = true;
        }
        Ok(RoutedFrame::Headers {
            stream_id,
            role,
            method: envelope.method.clone(),
        })
    }

    fn route_data(&mut self, frame: &Frame) -> io::Result<RoutedFrame> {
        require_nonzero_stream(frame)?;
        let entry = self.stream_entry_mut(frame.stream_id)?;
        if matches!(
            entry.state,
            StreamState::HalfClosedRemote | StreamState::Closed
        ) {
            return Err(protocol_error(format!(
                "received DATA after remote side closed v5 stream {}",
                frame.stream_id
            )));
        }
        Ok(RoutedFrame::Data {
            stream_id: frame.stream_id,
            flow_control_len: frame.flow_control_len(),
        })
    }

    fn route_end_stream(&mut self, frame: &Frame) -> io::Result<RoutedFrame> {
        require_nonzero_stream(frame)?;
        let entry = self.stream_entry_mut(frame.stream_id)?;
        entry.state = match entry.state {
            StreamState::Open => StreamState::HalfClosedRemote,
            StreamState::HalfClosedLocal => StreamState::Closed,
            StreamState::HalfClosedRemote | StreamState::Closed => {
                return Err(protocol_error(format!(
                    "remote side already closed v5 stream {}",
                    frame.stream_id
                )));
            }
        };
        let state = entry.state;
        if state == StreamState::Closed {
            self.streams.remove(&frame.stream_id);
        }
        Ok(RoutedFrame::EndStream {
            stream_id: frame.stream_id,
            state,
        })
    }

    fn route_reset_stream(&mut self, frame: &Frame) -> io::Result<RoutedFrame> {
        require_nonzero_stream(frame)?;
        let known = self.streams.remove(&frame.stream_id).is_some();
        Ok(RoutedFrame::ResetStream {
            stream_id: frame.stream_id,
            known,
        })
    }

    fn insert_stream(&mut self, entry: StreamEntry) -> io::Result<()> {
        if self.streams.len() >= self.max_concurrent_streams {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "v5 max concurrent streams exceeded",
            ));
        }
        if self.streams.insert(entry.stream_id, entry).is_some() {
            return Err(protocol_error("v5 stream already exists"));
        }
        Ok(())
    }

    fn stream_entry_mut(&mut self, stream_id: u64) -> io::Result<&mut StreamEntry> {
        self.streams
            .get_mut(&stream_id)
            .ok_or_else(|| protocol_error(format!("unknown v5 stream {stream_id}")))
    }
}

impl StreamEntry {
    fn new(
        stream_id: u64,
        method: String,
        initiator: StreamInitiator,
        request_id: u64,
        cancellation_group: String,
        content_encoding: ContentEncoding,
    ) -> Self {
        Self {
            stream_id,
            method,
            initiator,
            state: StreamState::Open,
            final_seen: false,
            request_id,
            cancellation_group,
            content_encoding,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlTrafficBudget {
    connection_limit: u64,
    stream_limit: u64,
    connection_used: u64,
    stream_used: HashMap<u64, u64>,
}

impl ControlTrafficBudget {
    pub fn new(settings: &ConnectionSettings) -> Self {
        Self {
            connection_limit: nonzero_or(
                settings.connection_control_budget,
                DEFAULT_CONNECTION_CONTROL_BUDGET,
            ) as u64,
            stream_limit: nonzero_or(
                settings.stream_control_budget,
                DEFAULT_STREAM_CONTROL_BUDGET,
            ) as u64,
            connection_used: 0,
            stream_used: HashMap::new(),
        }
    }

    pub fn connection_used(&self) -> u64 {
        self.connection_used
    }

    pub fn stream_used(&self, stream_id: u64) -> u64 {
        self.stream_used.get(&stream_id).copied().unwrap_or(0)
    }

    pub fn reserve_frame(&mut self, frame: &Frame) -> io::Result<()> {
        let bytes = frame.control_budget_len();
        if bytes == 0 {
            return Ok(());
        }

        let new_connection_used = self
            .connection_used
            .checked_add(bytes)
            .ok_or_else(|| control_budget_error("connection control queue size overflowed u64"))?;
        if new_connection_used > self.connection_limit {
            return Err(control_budget_error(format!(
                "connection control queue would use {new_connection_used} bytes, limit {}",
                self.connection_limit
            )));
        }

        let new_stream_used = if frame.stream_id == 0 {
            None
        } else {
            let current = self.stream_used(frame.stream_id);
            let new_used = current.checked_add(bytes).ok_or_else(|| {
                control_budget_error(format!(
                    "stream {} control queue size overflowed u64",
                    frame.stream_id
                ))
            })?;
            if new_used > self.stream_limit {
                return Err(control_budget_error(format!(
                    "stream {} control queue would use {new_used} bytes, limit {}",
                    frame.stream_id, self.stream_limit
                )));
            }
            Some(new_used)
        };

        self.connection_used = new_connection_used;
        if let Some(new_stream_used) = new_stream_used {
            self.stream_used.insert(frame.stream_id, new_stream_used);
        }
        Ok(())
    }

    pub fn release_frame(&mut self, frame: &Frame) {
        let bytes = frame.control_budget_len();
        if bytes == 0 {
            return;
        }

        self.connection_used = self.connection_used.saturating_sub(bytes);
        if frame.stream_id == 0 {
            return;
        }

        if let Some(stream_used) = self.stream_used.get_mut(&frame.stream_id) {
            *stream_used = stream_used.saturating_sub(bytes);
            if *stream_used == 0 {
                self.stream_used.remove(&frame.stream_id);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutboundScheduler {
    queues: Vec<VecDeque<QueuedFrame>>,
    connection_window: FlowWindow,
    stream_windows: HashMap<u64, FlowWindow>,
    default_stream_window: u64,
    control_budget: ControlTrafficBudget,
    next_enqueue_order: u64,
}

#[derive(Debug, Clone)]
struct QueuedFrame {
    order: u64,
    frame: Frame,
}

impl OutboundScheduler {
    pub fn new(settings: &ConnectionSettings) -> Self {
        Self {
            queues: (0..PRIORITY_LEVELS).map(|_| VecDeque::new()).collect(),
            connection_window: FlowWindow::new(nonzero_or(
                settings.initial_connection_window,
                DEFAULT_CONNECTION_WINDOW,
            ) as u64),
            stream_windows: HashMap::new(),
            default_stream_window: nonzero_or(settings.initial_stream_window, DEFAULT_STREAM_WINDOW)
                as u64,
            control_budget: ControlTrafficBudget::new(settings),
            next_enqueue_order: 0,
        }
    }

    pub fn queued_len(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.queued_len() == 0
    }

    pub fn connection_window(&self) -> u64 {
        self.connection_window.available()
    }

    pub fn stream_window(&self, stream_id: u64) -> u64 {
        self.stream_windows
            .get(&stream_id)
            .map(FlowWindow::available)
            .unwrap_or(self.default_stream_window)
    }

    pub fn connection_control_used(&self) -> u64 {
        self.control_budget.connection_used()
    }

    pub fn stream_control_used(&self, stream_id: u64) -> u64 {
        self.control_budget.stream_used(stream_id)
    }

    pub fn enqueue(&mut self, frame: Frame) -> io::Result<()> {
        if frame.frame_type == FrameType::Data && frame.stream_id == 0 {
            return Err(protocol_error("DATA frames require a non-zero stream id"));
        }
        self.control_budget.reserve_frame(&frame)?;
        let index = Priority::queue_index_from_wire(frame.priority);
        let order = self.next_enqueue_order;
        self.next_enqueue_order = self
            .next_enqueue_order
            .checked_add(1)
            .ok_or_else(|| io::Error::other("v5 outbound scheduler order exhausted"))?;
        self.queues[index].push_back(QueuedFrame { order, frame });
        Ok(())
    }

    pub fn grant_connection(&mut self, credit_bytes: u64) -> io::Result<()> {
        self.connection_window.grant(credit_bytes)
    }

    pub fn grant_stream(&mut self, stream_id: u64, credit_bytes: u64) -> io::Result<()> {
        if stream_id == 0 {
            return Err(protocol_error(
                "stream WINDOW_UPDATE requires non-zero stream id",
            ));
        }
        self.stream_window_mut(stream_id).grant(credit_bytes)
    }

    pub fn pop_next(&mut self) -> io::Result<Option<Frame>> {
        let mut blocked_streams = HashSet::new();
        for queue_index in 0..self.queues.len() {
            let frames_to_scan = self.queues[queue_index].len();
            for _ in 0..frames_to_scan {
                let frame = self.queues[queue_index]
                    .pop_front()
                    .expect("queue length was checked");
                if self.is_blocked_by_earlier_stream_frame(&frame, &blocked_streams) {
                    self.queues[queue_index].push_back(frame);
                    continue;
                }
                if self.has_earlier_same_stream_frame(&frame)
                    && frame.frame.frame_type != FrameType::ResetStream
                {
                    self.queues[queue_index].push_back(frame);
                    continue;
                }
                if self.can_send(&frame.frame) {
                    self.consume_credit(&frame.frame)?;
                    self.control_budget.release_frame(&frame.frame);
                    return Ok(Some(frame.frame));
                }
                if frame.frame.frame_type == FrameType::Data {
                    blocked_streams.insert(frame.frame.stream_id);
                }
                self.queues[queue_index].push_back(frame);
            }
        }
        Ok(None)
    }

    fn is_blocked_by_earlier_stream_frame(
        &self,
        frame: &QueuedFrame,
        blocked_streams: &HashSet<u64>,
    ) -> bool {
        frame.frame.stream_id != 0
            && blocked_streams.contains(&frame.frame.stream_id)
            && frame.frame.frame_type != FrameType::ResetStream
    }

    fn has_earlier_same_stream_frame(&self, frame: &QueuedFrame) -> bool {
        if frame.frame.stream_id == 0 || frame.frame.frame_type == FrameType::ResetStream {
            return false;
        }
        self.queues.iter().any(|queue| {
            queue.iter().any(|queued| {
                queued.order < frame.order
                    && queued.frame.stream_id == frame.frame.stream_id
                    && queued.frame.frame_type != FrameType::ResetStream
            })
        })
    }

    fn can_send(&self, frame: &Frame) -> bool {
        let flow_len = frame.flow_control_len();
        if flow_len == 0 {
            return true;
        }
        self.connection_window.available() >= flow_len
            && self.stream_window(frame.stream_id) >= flow_len
    }

    fn consume_credit(&mut self, frame: &Frame) -> io::Result<()> {
        let flow_len = frame.flow_control_len();
        if flow_len == 0 {
            return Ok(());
        }
        self.connection_window.consume(flow_len)?;
        self.stream_window_mut(frame.stream_id).consume(flow_len)
    }

    fn stream_window_mut(&mut self, stream_id: u64) -> &mut FlowWindow {
        self.stream_windows
            .entry(stream_id)
            .or_insert_with(|| FlowWindow::new(self.default_stream_window))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Headers {
        stream_id: u64,
        role: MessageRole,
        envelope: StreamEnvelope,
    },
    Data {
        stream_id: u64,
        channel: DataChannel,
        uncompressed_len: u64,
        body: Vec<u8>,
    },
    EndStream {
        stream_id: u64,
    },
    ResetStream {
        stream_id: u64,
        code: String,
        diagnostic: String,
    },
}

impl StreamEvent {
    pub fn from_frame(frame: Frame) -> io::Result<Option<Self>> {
        Self::from_frame_with_content_encoding(frame, ContentEncoding::None)
    }

    pub fn from_frame_with_content_encoding(
        frame: Frame,
        content_encoding: ContentEncoding,
    ) -> io::Result<Option<Self>> {
        match frame.frame_type {
            FrameType::Headers => {
                let envelope = frame
                    .decode_control::<StreamEnvelope>()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                let role = envelope.message_role()?;
                envelope.decode_content_encoding()?;
                Ok(Some(Self::Headers {
                    stream_id: frame.stream_id,
                    role,
                    envelope,
                }))
            }
            FrameType::Data => {
                let envelope = if frame.control.is_empty() {
                    DataEnvelope {
                        channel: DataChannel::Unspecified as i32,
                        uncompressed_len: frame.body.len() as u64,
                    }
                } else {
                    frame
                        .decode_control::<DataEnvelope>()
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                };
                let channel = DataChannel::try_from(envelope.channel).map_err(|_| {
                    protocol_error(format!("unknown v5 data channel: {}", envelope.channel))
                })?;
                let body =
                    decode_data_body(frame.body, content_encoding, envelope.uncompressed_len)?;
                Ok(Some(Self::Data {
                    stream_id: frame.stream_id,
                    channel,
                    uncompressed_len: envelope.uncompressed_len,
                    body,
                }))
            }
            FrameType::EndStream => Ok(Some(Self::EndStream {
                stream_id: frame.stream_id,
            })),
            FrameType::ResetStream => {
                let reset = if frame.control.is_empty() {
                    ResetStream {
                        code: "reset".to_string(),
                        diagnostic: String::new(),
                    }
                } else {
                    frame
                        .decode_control::<ResetStream>()
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                };
                Ok(Some(Self::ResetStream {
                    stream_id: frame.stream_id,
                    code: reset.code,
                    diagnostic: reset.diagnostic,
                }))
            }
            FrameType::Hello
            | FrameType::Settings
            | FrameType::SettingsAck
            | FrameType::WindowUpdate
            | FrameType::Ping
            | FrameType::Pong
            | FrameType::GoAway => Ok(None),
        }
    }

    pub fn stream_id(&self) -> u64 {
        match self {
            Self::Headers { stream_id, .. }
            | Self::Data { stream_id, .. }
            | Self::EndStream { stream_id }
            | Self::ResetStream { stream_id, .. } => *stream_id,
        }
    }
}

fn decode_data_body(
    body: Vec<u8>,
    content_encoding: ContentEncoding,
    uncompressed_len: u64,
) -> io::Result<Vec<u8>> {
    match content_encoding {
        ContentEncoding::None => Ok(body),
        ContentEncoding::Zstd => {
            let expected_len = usize::try_from(uncompressed_len).map_err(|_| {
                protocol_error("v5 zstd DATA frame uncompressed length exceeds usize")
            })?;
            let decompressed = zstd::bulk::decompress(&body, expected_len).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to decompress v5 DATA frame with zstd: {error}"),
                )
            })?;
            if decompressed.len() != expected_len {
                return Err(protocol_error(format!(
                    "v5 zstd DATA frame decompressed to {} bytes, expected {expected_len}",
                    decompressed.len()
                )));
            }
            Ok(decompressed)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    pub stream_id: u64,
    pub method: String,
    pub cancellation_group: String,
    pub deadline_unix_ms: u64,
    pub supersedes_stream_id: u64,
    pub idempotency: Idempotency,
    pub final_seen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestMetadata {
    pub method: String,
    pub cancellation_group: String,
    pub deadline_unix_ms: u64,
    pub supersedes_stream_id: u64,
    pub idempotency: Idempotency,
}

impl RequestMetadata {
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            cancellation_group: String::new(),
            deadline_unix_ms: 0,
            supersedes_stream_id: 0,
            idempotency: Idempotency::ReadOnly,
        }
    }

    pub fn with_cancellation_group(mut self, cancellation_group: impl Into<String>) -> Self {
        self.cancellation_group = cancellation_group.into();
        self
    }

    pub fn with_deadline_unix_ms(mut self, deadline_unix_ms: u64) -> Self {
        self.deadline_unix_ms = deadline_unix_ms;
        self
    }

    pub fn with_supersedes_stream_id(mut self, supersedes_stream_id: u64) -> Self {
        self.supersedes_stream_id = supersedes_stream_id;
        self
    }

    pub fn with_idempotency(mut self, idempotency: Idempotency) -> Self {
        self.idempotency = idempotency;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct InFlightRequests {
    requests: HashMap<u64, PendingRequest>,
}

impl InFlightRequests {
    pub fn len(&self) -> usize {
        self.requests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    pub fn contains(&self, stream_id: u64) -> bool {
        self.requests.contains_key(&stream_id)
    }

    pub fn get(&self, stream_id: u64) -> Option<&PendingRequest> {
        self.requests.get(&stream_id)
    }

    pub fn register(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        cancellation_group: impl Into<String>,
    ) -> io::Result<()> {
        let metadata = RequestMetadata::new(method).with_cancellation_group(cancellation_group);
        self.register_with_metadata(stream_id, metadata)
    }

    pub fn register_with_metadata(
        &mut self,
        stream_id: u64,
        metadata: RequestMetadata,
    ) -> io::Result<()> {
        if stream_id == 0 {
            return Err(protocol_error(
                "in-flight request stream id must be non-zero",
            ));
        }
        let previous = self.requests.insert(
            stream_id,
            PendingRequest {
                stream_id,
                method: metadata.method,
                cancellation_group: metadata.cancellation_group,
                deadline_unix_ms: metadata.deadline_unix_ms,
                supersedes_stream_id: metadata.supersedes_stream_id,
                idempotency: metadata.idempotency,
                final_seen: false,
            },
        );
        if previous.is_some() {
            return Err(protocol_error(format!(
                "in-flight request already exists for stream {stream_id}"
            )));
        }
        Ok(())
    }

    pub fn register_from_envelope(
        &mut self,
        stream_id: u64,
        envelope: &StreamEnvelope,
    ) -> io::Result<()> {
        if envelope.message_role()? != MessageRole::Request {
            return Err(protocol_error(
                "only request headers can register in-flight work",
            ));
        }
        self.register_with_metadata(stream_id, envelope.request_metadata()?)
    }

    pub fn observe_event(&mut self, event: &StreamEvent) -> io::Result<()> {
        match event {
            StreamEvent::Headers {
                stream_id, role, ..
            } if role.is_final() => {
                let request = self.request_mut(*stream_id)?;
                request.final_seen = true;
            }
            StreamEvent::EndStream { stream_id } | StreamEvent::ResetStream { stream_id, .. } => {
                self.requests.remove(stream_id);
            }
            StreamEvent::Headers { .. } | StreamEvent::Data { .. } => {}
        }
        Ok(())
    }

    pub fn cancel_stream(
        &mut self,
        stream_id: u64,
        code: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> Option<Frame> {
        self.requests
            .remove(&stream_id)
            .map(|_| reset_stream_frame(stream_id, code, diagnostic))
    }

    pub fn cancel_group(
        &mut self,
        cancellation_group: &str,
        code: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> Vec<Frame> {
        if cancellation_group.is_empty() {
            return Vec::new();
        }
        let code = code.into();
        let diagnostic = diagnostic.into();
        let mut stream_ids = self
            .requests
            .values()
            .filter(|request| request.cancellation_group == cancellation_group)
            .map(|request| request.stream_id)
            .collect::<Vec<_>>();
        stream_ids.sort_unstable();
        stream_ids
            .into_iter()
            .filter_map(|stream_id| {
                self.requests.remove(&stream_id)?;
                Some(reset_stream_frame(
                    stream_id,
                    code.clone(),
                    diagnostic.clone(),
                ))
            })
            .collect()
    }

    pub fn cancel_superseded_by(
        &mut self,
        superseding_stream_id: u64,
    ) -> io::Result<Option<Frame>> {
        let superseded_stream_id = self
            .requests
            .get(&superseding_stream_id)
            .ok_or_else(|| {
                protocol_error(format!("unknown in-flight stream {superseding_stream_id}"))
            })?
            .supersedes_stream_id;
        if superseded_stream_id == 0 {
            return Ok(None);
        }
        if superseded_stream_id == superseding_stream_id {
            return Err(protocol_error(format!(
                "stream {superseding_stream_id} cannot supersede itself"
            )));
        }
        Ok(self.cancel_stream(
            superseded_stream_id,
            RESET_CANCELLED,
            format!("superseded by stream {superseding_stream_id}"),
        ))
    }

    pub fn expire_deadlines(&mut self, now_unix_ms: u64) -> Vec<Frame> {
        let mut stream_ids = self
            .requests
            .values()
            .filter(|request| {
                request.deadline_unix_ms != 0 && request.deadline_unix_ms <= now_unix_ms
            })
            .map(|request| request.stream_id)
            .collect::<Vec<_>>();
        stream_ids.sort_unstable();
        stream_ids
            .into_iter()
            .filter_map(|stream_id| {
                self.requests.remove(&stream_id)?;
                Some(reset_stream_frame(
                    stream_id,
                    RESET_DEADLINE_EXCEEDED,
                    "request deadline expired",
                ))
            })
            .collect()
    }

    fn request_mut(&mut self, stream_id: u64) -> io::Result<&mut PendingRequest> {
        self.requests
            .get_mut(&stream_id)
            .ok_or_else(|| protocol_error(format!("unknown in-flight stream {stream_id}")))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionEvent {
    pub routed: RoutedFrame,
    pub stream_event: Option<StreamEvent>,
}

impl SessionEvent {
    pub fn data_credit(&self) -> Option<(u64, u64)> {
        match &self.routed {
            RoutedFrame::Data {
                stream_id,
                flow_control_len,
            } if *flow_control_len > 0 => Some((*stream_id, *flow_control_len)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProtocolSession {
    streams: StreamTable,
    scheduler: OutboundScheduler,
    in_flight: InFlightRequests,
    limits: FrameLimits,
    shutdown_grace_ms: u32,
    peer_goaway: Option<GoAway>,
    sent_goaway: Option<GoAway>,
}

impl ProtocolSession {
    pub fn new(local_initiator: StreamInitiator, settings: &ConnectionSettings) -> Self {
        Self::with_limits(
            local_initiator,
            settings,
            FrameLimits::from_settings(settings),
        )
    }

    pub fn with_limits(
        local_initiator: StreamInitiator,
        settings: &ConnectionSettings,
        limits: FrameLimits,
    ) -> Self {
        Self {
            streams: StreamTable::new(local_initiator, settings),
            scheduler: OutboundScheduler::new(settings),
            in_flight: InFlightRequests::default(),
            limits,
            shutdown_grace_ms: nonzero_or(settings.shutdown_grace_ms, DEFAULT_SHUTDOWN_GRACE_MS),
            peer_goaway: None,
            sent_goaway: None,
        }
    }

    pub fn active_streams(&self) -> usize {
        self.streams.active_streams()
    }

    pub fn get_stream(&self, stream_id: u64) -> Option<&StreamEntry> {
        self.streams.get(stream_id)
    }

    pub fn queued_len(&self) -> usize {
        self.scheduler.queued_len()
    }

    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_in_flight(&self, stream_id: u64) -> bool {
        self.in_flight.contains(stream_id)
    }

    pub fn stream_window(&self, stream_id: u64) -> u64 {
        self.scheduler.stream_window(stream_id)
    }

    pub fn connection_window(&self) -> u64 {
        self.scheduler.connection_window()
    }

    pub fn last_accepted_remote_stream_id(&self) -> u64 {
        self.streams.last_accepted_remote_stream_id()
    }

    pub fn peer_goaway(&self) -> Option<&GoAway> {
        self.peer_goaway.as_ref()
    }

    pub fn sent_goaway(&self) -> Option<&GoAway> {
        self.sent_goaway.as_ref()
    }

    #[cfg(test)]
    fn set_next_stream_id_for_test(&mut self, next: u64) {
        self.streams.allocator = StreamIdAllocator::with_next_for_test(next);
    }

    pub fn open_unary_request(
        &mut self,
        method: impl Into<String>,
        options: RequestOptions,
    ) -> io::Result<u64> {
        self.open_request_with_body(method, options, DataChannel::Unspecified, &[])
    }

    pub fn open_request_with_body(
        &mut self,
        method: impl Into<String>,
        options: RequestOptions,
        channel: DataChannel,
        body: &[u8],
    ) -> io::Result<u64> {
        self.open_request_with_payload_and_body(method, options, &[], channel, body)
    }

    pub fn open_request_with_payload_and_body(
        &mut self,
        method: impl Into<String>,
        options: RequestOptions,
        payload: &[u8],
        body_channel: DataChannel,
        body: &[u8],
    ) -> io::Result<u64> {
        self.ensure_peer_accepts_new_stream()?;
        let priority = options.priority;
        let (stream_id, headers) = match self.streams.open_request_with_options(method, options) {
            Ok(opened) => opened,
            Err(error) if is_stream_ids_exhausted_error(&error) => {
                self.send_stream_ids_exhausted_goaway()?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let result =
            self.queue_open_request(stream_id, headers, priority, payload, body_channel, body);
        if result.is_err() {
            self.rollback_stream(stream_id);
        }
        result.map(|()| stream_id)
    }

    pub fn open_event_stream(
        &mut self,
        method: impl Into<String>,
        watch_id: u64,
    ) -> io::Result<u64> {
        self.ensure_peer_accepts_new_stream()?;
        let (stream_id, headers) = match self.streams.open_event_stream(method, watch_id) {
            Ok(opened) => opened,
            Err(error) if is_stream_ids_exhausted_error(&error) => {
                self.send_stream_ids_exhausted_goaway()?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let result = self.scheduler.enqueue(headers);
        if result.is_err() {
            self.rollback_stream(stream_id);
        }
        result.map(|()| stream_id)
    }

    pub fn enqueue_watch_batch(
        &mut self,
        event_stream_id: u64,
        batch: WatchBatch,
    ) -> io::Result<()> {
        self.scheduler
            .enqueue(watch_batch_frame(event_stream_id, batch)?)
    }

    pub fn send_goaway(
        &mut self,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> io::Result<()> {
        let goaway = GoAway {
            last_accepted_stream_id: self.last_accepted_remote_stream_id(),
            code: code.into(),
            message: message.into(),
            drain_grace_ms: self.shutdown_grace_ms,
        };
        self.scheduler.enqueue(
            Frame::from_control(FrameType::GoAway, 0, &goaway).with_priority(Priority::Background),
        )?;
        self.sent_goaway = Some(goaway);
        Ok(())
    }

    fn send_stream_ids_exhausted_goaway(&mut self) -> io::Result<()> {
        self.send_goaway(RESET_RESOURCE_EXHAUSTED, STREAM_IDS_EXHAUSTED_MESSAGE)
    }

    pub fn send_progress(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        progress: Progress,
    ) -> io::Result<()> {
        self.ensure_open_stream(stream_id)?;
        self.scheduler.enqueue(
            Frame::from_control(
                FrameType::Headers,
                stream_id,
                &StreamEnvelope::progress(stream_id, method, progress),
            )
            .with_priority(Priority::Background),
        )
    }

    pub fn send_response(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        role: MessageRole,
        complete: bool,
    ) -> io::Result<()> {
        if !matches!(
            role,
            MessageRole::PartialResult | MessageRole::FinalResponse
        ) {
            return Err(protocol_error(format!(
                "response headers require partial_result or final_response role, got {role:?}"
            )));
        }
        self.ensure_open_stream(stream_id)?;
        self.scheduler.enqueue(
            Frame::from_control(
                FrameType::Headers,
                stream_id,
                &StreamEnvelope::response(stream_id, method, role, complete),
            )
            .with_priority(Priority::Background),
        )
    }

    pub fn send_error(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        error: ErrorHeader,
    ) -> io::Result<()> {
        self.ensure_open_stream(stream_id)?;
        self.scheduler.enqueue(
            Frame::from_control(
                FrameType::Headers,
                stream_id,
                &StreamEnvelope::error(stream_id, method, error),
            )
            .with_priority(Priority::Background),
        )
    }

    pub fn send_data(
        &mut self,
        stream_id: u64,
        channel: DataChannel,
        body: &[u8],
        priority: Priority,
    ) -> io::Result<()> {
        self.ensure_open_stream(stream_id)?;
        let content_encoding = self
            .streams
            .get(stream_id)
            .map(|entry| entry.content_encoding)
            .unwrap_or(ContentEncoding::None);
        let options = DataFrameOptions::new(channel)
            .with_priority(priority)
            .with_content_encoding(content_encoding);
        for frame in DataChunker::with_options(stream_id, body, self.limits, options)? {
            self.scheduler.enqueue(frame)?;
        }
        Ok(())
    }

    pub fn finish_stream(&mut self, stream_id: u64, priority: Priority) -> io::Result<StreamState> {
        self.ensure_open_stream(stream_id)?;
        self.scheduler
            .enqueue(end_stream_frame(stream_id)?.with_priority(priority))?;
        self.streams.mark_local_end(stream_id)
    }

    pub fn receive_frame(&mut self, frame: Frame) -> io::Result<SessionEvent> {
        if let Some(event) = self.reject_unknown_stream_after_local_goaway(&frame)? {
            return Ok(event);
        }
        let routed = self.streams.route_incoming(&frame)?;
        match &routed {
            RoutedFrame::WindowUpdate {
                stream_id,
                credit_bytes,
            } => {
                if *stream_id == 0 {
                    self.scheduler.grant_connection(*credit_bytes)?;
                } else {
                    self.scheduler.grant_stream(*stream_id, *credit_bytes)?;
                }
                Ok(SessionEvent {
                    routed,
                    stream_event: None,
                })
            }
            RoutedFrame::ConnectionControl { frame_type } => {
                if *frame_type == FrameType::Ping {
                    self.queue_pong(frame.control.clone())?;
                } else if *frame_type == FrameType::GoAway {
                    self.peer_goaway = Some(decode_control::<GoAway>(&frame)?);
                }
                Ok(SessionEvent {
                    routed,
                    stream_event: None,
                })
            }
            RoutedFrame::RejectedStream { .. } => Ok(SessionEvent {
                routed,
                stream_event: None,
            }),
            RoutedFrame::Headers { .. }
            | RoutedFrame::Data { .. }
            | RoutedFrame::EndStream { .. }
            | RoutedFrame::ResetStream { .. } => {
                let content_encoding = match &routed {
                    RoutedFrame::Data { stream_id, .. } => self
                        .streams
                        .get(*stream_id)
                        .map(|entry| entry.content_encoding)
                        .unwrap_or(ContentEncoding::None),
                    _ => ContentEncoding::None,
                };
                let event = StreamEvent::from_frame_with_content_encoding(frame, content_encoding)?;
                if let Some(event) = event.as_ref() {
                    self.observe_stream_event(event)?;
                }
                Ok(SessionEvent {
                    routed,
                    stream_event: event,
                })
            }
        }
    }

    pub fn acknowledge_data(&mut self, stream_id: u64, credit_bytes: u64) -> io::Result<()> {
        if credit_bytes == 0 {
            return Ok(());
        }
        self.queue_data_window_updates(stream_id, credit_bytes)
    }

    fn queue_data_window_updates(&mut self, stream_id: u64, credit_bytes: u64) -> io::Result<()> {
        self.scheduler
            .enqueue(window_update_frame(0, credit_bytes)?.with_priority(Priority::UserInput))?;
        self.scheduler.enqueue(
            window_update_frame(stream_id, credit_bytes)?.with_priority(Priority::UserInput),
        )?;
        Ok(())
    }

    fn queue_pong(&mut self, token: Vec<u8>) -> io::Result<()> {
        let mut pong = Frame::new(FrameType::Pong, 0).with_priority(Priority::UserInput);
        pong.control = token;
        self.scheduler.enqueue(pong)
    }

    pub fn send_ping(&mut self, token: Vec<u8>) -> io::Result<()> {
        let mut ping = Frame::from_control(FrameType::Ping, 0, &PingPayload { token })
            .with_priority(Priority::UserInput);
        ping.body.clear();
        self.scheduler.enqueue(ping)
    }

    pub fn cancel_stream(
        &mut self,
        stream_id: u64,
        code: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> io::Result<bool> {
        let Some(frame) = self.in_flight.cancel_stream(stream_id, code, diagnostic) else {
            return Ok(false);
        };
        self.streams.streams.remove(&stream_id);
        self.scheduler.enqueue(frame)?;
        Ok(true)
    }

    pub fn reset_stream(
        &mut self,
        stream_id: u64,
        code: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> io::Result<bool> {
        self.in_flight.requests.remove(&stream_id);
        let known = self.streams.streams.remove(&stream_id).is_some();
        if known {
            self.scheduler
                .enqueue(reset_stream_frame(stream_id, code, diagnostic))?;
        }
        Ok(known)
    }

    pub fn cancel_group(
        &mut self,
        cancellation_group: &str,
        code: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> io::Result<usize> {
        let frames = self
            .in_flight
            .cancel_group(cancellation_group, code, diagnostic);
        let frame_count = frames.len();
        for frame in frames {
            self.streams.streams.remove(&frame.stream_id);
            self.scheduler.enqueue(frame)?;
        }
        Ok(frame_count)
    }

    pub fn expire_deadlines(&mut self, now_unix_ms: u64) -> io::Result<usize> {
        let frames = self.in_flight.expire_deadlines(now_unix_ms);
        let frame_count = frames.len();
        for frame in frames {
            self.streams.streams.remove(&frame.stream_id);
            self.scheduler.enqueue(frame)?;
        }
        Ok(frame_count)
    }

    pub fn pop_next_frame(&mut self) -> io::Result<Option<Frame>> {
        self.scheduler.pop_next()
    }

    fn queue_open_request(
        &mut self,
        stream_id: u64,
        headers: Frame,
        priority: Priority,
        payload: &[u8],
        channel: DataChannel,
        body: &[u8],
    ) -> io::Result<()> {
        let envelope = headers
            .decode_control::<StreamEnvelope>()
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid request headers: {error}"),
                )
            })?;
        self.in_flight
            .register_with_metadata(stream_id, envelope.request_metadata()?)?;
        self.scheduler.enqueue(headers)?;
        let content_encoding = self
            .streams
            .get(stream_id)
            .map(|entry| entry.content_encoding)
            .unwrap_or(ContentEncoding::None);

        if !payload.is_empty() {
            let options = DataFrameOptions::new(DataChannel::Unspecified)
                .with_priority(priority)
                .with_content_encoding(content_encoding);
            for frame in DataChunker::with_options(stream_id, payload, self.limits, options)? {
                self.scheduler.enqueue(frame)?;
            }
        }

        if !body.is_empty() {
            let options = DataFrameOptions::new(channel)
                .with_priority(priority)
                .with_content_encoding(content_encoding);
            for frame in DataChunker::with_options(stream_id, body, self.limits, options)? {
                self.scheduler.enqueue(frame)?;
            }
        }

        self.scheduler
            .enqueue(end_stream_frame(stream_id)?.with_priority(priority))?;
        self.streams.mark_local_end(stream_id)?;

        if let Some(frame) = self.in_flight.cancel_superseded_by(stream_id)? {
            self.streams.streams.remove(&frame.stream_id);
            self.scheduler.enqueue(frame)?;
        }

        Ok(())
    }

    fn observe_stream_event(&mut self, event: &StreamEvent) -> io::Result<()> {
        if let StreamEvent::Headers {
            stream_id,
            role: MessageRole::Request,
            envelope,
        } = event
        {
            self.in_flight
                .register_from_envelope(*stream_id, envelope)?;
            if let Some(frame) = self.in_flight.cancel_superseded_by(*stream_id)? {
                self.streams.streams.remove(&frame.stream_id);
                self.scheduler.enqueue(frame)?;
            }
            return Ok(());
        }
        self.in_flight.observe_event(event)
    }

    fn rollback_stream(&mut self, stream_id: u64) {
        self.streams.streams.remove(&stream_id);
        self.in_flight.requests.remove(&stream_id);
    }

    fn ensure_open_stream(&self, stream_id: u64) -> io::Result<()> {
        if self.streams.get(stream_id).is_none() {
            return Err(protocol_error(format!("unknown v5 stream {stream_id}")));
        }
        Ok(())
    }

    fn ensure_peer_accepts_new_stream(&self) -> io::Result<()> {
        if let Some(goaway) = &self.peer_goaway {
            return Err(protocol_error(format!(
                "peer sent GOAWAY ({}) and will not accept new v5 streams; last accepted stream was {}",
                goaway.code, goaway.last_accepted_stream_id
            )));
        }
        Ok(())
    }

    fn reject_unknown_stream_after_local_goaway(
        &mut self,
        frame: &Frame,
    ) -> io::Result<Option<SessionEvent>> {
        let Some(goaway) = &self.sent_goaway else {
            return Ok(None);
        };
        if frame.stream_id == 0
            || self.streams.get(frame.stream_id).is_some()
            || frame.frame_type == FrameType::ResetStream
        {
            return Ok(None);
        }
        if stream_id_initiator(frame.stream_id)? != remote_initiator(self.streams.local_initiator) {
            return Ok(None);
        }

        self.scheduler.enqueue(
            reset_stream_frame(
                frame.stream_id,
                RESET_UNAVAILABLE,
                format!(
                    "stream rejected after GOAWAY; last accepted stream was {}",
                    goaway.last_accepted_stream_id
                ),
            )
            .with_priority(Priority::UserInput),
        )?;
        Ok(Some(SessionEvent {
            routed: RoutedFrame::RejectedStream {
                stream_id: frame.stream_id,
            },
            stream_event: None,
        }))
    }
}

pub fn reset_stream_frame(
    stream_id: u64,
    code: impl Into<String>,
    diagnostic: impl Into<String>,
) -> Frame {
    Frame::from_control(
        FrameType::ResetStream,
        stream_id,
        &ResetStream {
            code: code.into(),
            diagnostic: diagnostic.into(),
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataFrameOptions {
    pub channel: DataChannel,
    pub priority: Priority,
    pub uncompressed_len: Option<u64>,
    pub content_encoding: ContentEncoding,
}

impl DataFrameOptions {
    pub fn new(channel: DataChannel) -> Self {
        Self {
            channel,
            priority: Priority::Background,
            uncompressed_len: None,
            content_encoding: ContentEncoding::None,
        }
    }

    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_uncompressed_len(mut self, uncompressed_len: u64) -> Self {
        self.uncompressed_len = Some(uncompressed_len);
        self
    }

    pub fn with_content_encoding(mut self, content_encoding: ContentEncoding) -> Self {
        self.content_encoding = content_encoding;
        self
    }
}

pub fn stream_data_frame(
    stream_id: u64,
    body: impl Into<Vec<u8>>,
    options: DataFrameOptions,
) -> io::Result<Frame> {
    if stream_id == 0 {
        return Err(protocol_error("DATA frames require a non-zero stream id"));
    }
    build_encoded_data_frame(stream_id, body.into(), options)
}

pub fn end_stream_frame(stream_id: u64) -> io::Result<Frame> {
    if stream_id == 0 {
        return Err(protocol_error("END_STREAM requires a non-zero stream id"));
    }
    Ok(Frame::new(FrameType::EndStream, stream_id))
}

pub fn window_update_frame(stream_id: u64, credit_bytes: u64) -> io::Result<Frame> {
    if credit_bytes == 0 {
        return Err(protocol_error("WINDOW_UPDATE credit must be non-zero"));
    }
    Ok(Frame::from_control(
        FrameType::WindowUpdate,
        stream_id,
        &WindowUpdate { credit_bytes },
    ))
}

pub fn watch_batch_frame(event_stream_id: u64, batch: WatchBatch) -> io::Result<Frame> {
    if event_stream_id == 0 {
        return Err(protocol_error(
            "watch.batch requires a non-zero event stream id",
        ));
    }
    Ok(Frame::from_control(
        FrameType::Headers,
        event_stream_id,
        &StreamEnvelope::watch_batch(batch),
    )
    .with_priority(Priority::VisibleFileTree))
}

#[derive(Debug, Clone, Default)]
pub struct WatchGenerationTracker {
    next_sequence_by_watch: HashMap<u64, u64>,
    directory_generations: HashMap<String, u64>,
}

impl WatchGenerationTracker {
    pub fn generation(&self, path: &str) -> u64 {
        self.directory_generations.get(path).copied().unwrap_or(0)
    }

    pub fn build_batch<I, P>(
        &mut self,
        watch_id: u64,
        changed_directories: I,
        events: Vec<WatchChange>,
        overflow: bool,
        resync_required: bool,
    ) -> io::Result<WatchBatch>
    where
        I: IntoIterator<Item = P>,
        P: Into<String>,
    {
        if watch_id == 0 {
            return Err(protocol_error("watch_id must be non-zero"));
        }

        let sequence = self.next_sequence(watch_id)?;
        let mut changed_directories = changed_directories
            .into_iter()
            .map(Into::into)
            .collect::<Vec<String>>();
        changed_directories.sort();
        changed_directories.dedup();

        let mut directory_generations = Vec::with_capacity(changed_directories.len());
        for path in changed_directories {
            let generation = self.bump_generation(&path)?;
            directory_generations.push(WatchDirectoryGeneration { path, generation });
        }

        Ok(WatchBatch {
            watch_id,
            sequence,
            directory_generations,
            events,
            overflow,
            resync_required,
        })
    }

    fn next_sequence(&mut self, watch_id: u64) -> io::Result<u64> {
        let next = self.next_sequence_by_watch.entry(watch_id).or_insert(1);
        let sequence = *next;
        *next = next
            .checked_add(1)
            .ok_or_else(|| protocol_error(format!("watch {watch_id} sequence exhausted")))?;
        Ok(sequence)
    }

    fn bump_generation(&mut self, path: &str) -> io::Result<u64> {
        let entry = self
            .directory_generations
            .entry(path.to_string())
            .or_insert(0);
        *entry = entry
            .checked_add(1)
            .ok_or_else(|| protocol_error(format!("directory generation exhausted for {path}")))?;
        Ok(*entry)
    }
}

#[derive(Debug, Clone)]
pub struct DataChunker<'a> {
    stream_id: u64,
    bytes: &'a [u8],
    offset: usize,
    max_body_len: usize,
    options: DataFrameOptions,
    prepared_frames: Option<VecDeque<Frame>>,
}

impl<'a> DataChunker<'a> {
    pub fn new(
        stream_id: u64,
        channel: DataChannel,
        bytes: &'a [u8],
        limits: FrameLimits,
    ) -> io::Result<Self> {
        Self::with_options(stream_id, bytes, limits, DataFrameOptions::new(channel))
    }

    pub fn with_options(
        stream_id: u64,
        bytes: &'a [u8],
        limits: FrameLimits,
        options: DataFrameOptions,
    ) -> io::Result<Self> {
        if stream_id == 0 {
            return Err(protocol_error("DATA chunks require a non-zero stream id"));
        }
        if limits.max_body_len == 0 {
            return Err(protocol_error("DATA chunk body limit must be non-zero"));
        }
        if options.uncompressed_len.is_some() {
            return Err(protocol_error(
                "DATA chunker derives uncompressed_len per frame",
            ));
        }
        let prepared_frames = if options.content_encoding == ContentEncoding::Zstd {
            Some(build_compressed_data_frames(
                stream_id,
                bytes,
                limits.max_body_len as usize,
                options,
            )?)
        } else {
            None
        };
        Ok(Self {
            stream_id,
            bytes,
            offset: 0,
            max_body_len: limits.max_body_len as usize,
            options,
            prepared_frames,
        })
    }

    pub fn remaining_bytes(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
}

impl Iterator for DataChunker<'_> {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(prepared_frames) = self.prepared_frames.as_mut() {
            return prepared_frames.pop_front();
        }
        if self.offset >= self.bytes.len() {
            return None;
        }

        let end = self
            .offset
            .saturating_add(self.max_body_len)
            .min(self.bytes.len());
        let body = self.bytes[self.offset..end].to_vec();
        self.offset = end;

        Some(build_data_frame(self.stream_id, body, self.options))
    }
}

fn build_data_frame(stream_id: u64, body: Vec<u8>, options: DataFrameOptions) -> Frame {
    let uncompressed_len = options.uncompressed_len.unwrap_or(body.len() as u64);
    let mut frame = Frame::from_control(
        FrameType::Data,
        stream_id,
        &DataEnvelope {
            channel: options.channel as i32,
            uncompressed_len,
        },
    );
    frame.body = body;
    frame.with_priority(options.priority)
}

fn build_encoded_data_frame(
    stream_id: u64,
    body: Vec<u8>,
    options: DataFrameOptions,
) -> io::Result<Frame> {
    if options.content_encoding == ContentEncoding::Zstd && options.uncompressed_len.is_none() {
        let uncompressed_len = body.len() as u64;
        let compressed =
            zstd::bulk::compress(&body, ZSTD_DATA_COMPRESSION_LEVEL).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to compress v5 DATA frame with zstd: {error}"),
                )
            })?;
        Ok(build_data_frame(
            stream_id,
            compressed,
            DataFrameOptions {
                uncompressed_len: Some(uncompressed_len),
                ..options
            },
        ))
    } else {
        Ok(build_data_frame(stream_id, body, options))
    }
}

fn build_compressed_data_frames(
    stream_id: u64,
    bytes: &[u8],
    max_body_len: usize,
    options: DataFrameOptions,
) -> io::Result<VecDeque<Frame>> {
    let mut frames = VecDeque::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let mut end = offset.saturating_add(max_body_len).min(bytes.len());
        loop {
            let uncompressed = &bytes[offset..end];
            let compressed = zstd::bulk::compress(uncompressed, ZSTD_DATA_COMPRESSION_LEVEL)
                .map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("failed to compress v5 DATA chunk with zstd: {error}"),
                    )
                })?;
            if compressed.len() <= max_body_len {
                frames.push_back(build_data_frame(
                    stream_id,
                    compressed,
                    DataFrameOptions {
                        uncompressed_len: Some(uncompressed.len() as u64),
                        ..options
                    },
                ));
                offset = end;
                break;
            }
            let uncompressed_len = end.saturating_sub(offset);
            if uncompressed_len <= 1 {
                return Err(protocol_error(
                    "compressed DATA chunk exceeds negotiated frame body limit",
                ));
            }
            end = offset + (uncompressed_len / 2);
        }
    }
    Ok(frames)
}

pub struct FramedIo<R, W> {
    reader: R,
    writer: W,
    limits: FrameLimits,
    next_frame_sequence: u64,
}

pub struct FramedIoParts<R, W> {
    pub reader: R,
    pub writer: W,
    pub limits: FrameLimits,
    pub next_frame_sequence: u64,
}

impl<R, W> FramedIo<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self::with_limits(reader, writer, FrameLimits::default())
    }

    pub fn with_limits(reader: R, writer: W, limits: FrameLimits) -> Self {
        Self {
            reader,
            writer,
            limits,
            next_frame_sequence: 1,
        }
    }

    pub fn limits(&self) -> FrameLimits {
        self.limits
    }

    pub fn set_limits(&mut self, limits: FrameLimits) {
        self.limits = limits;
    }

    pub fn into_inner(self) -> (R, W) {
        (self.reader, self.writer)
    }

    pub fn into_parts(self) -> FramedIoParts<R, W> {
        FramedIoParts {
            reader: self.reader,
            writer: self.writer,
            limits: self.limits,
            next_frame_sequence: self.next_frame_sequence,
        }
    }
}

impl<R: Read, W: Write> FramedIo<R, W> {
    pub fn read_frame(&mut self) -> io::Result<Option<Frame>> {
        read_frame_with_limits(&mut self.reader, self.limits)
    }

    pub fn read_required_frame(
        &mut self,
        expected_type: FrameType,
        expected_stream_id: u64,
    ) -> io::Result<Frame> {
        let frame = self
            .read_frame()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "expected v5 frame"))?;
        if frame.frame_type != expected_type {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "expected v5 frame type {expected_type:?}, got {:?}",
                    frame.frame_type
                ),
            ));
        }
        if frame.stream_id != expected_stream_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "expected v5 stream id {expected_stream_id}, got {}",
                    frame.stream_id
                ),
            ));
        }
        Ok(frame)
    }

    pub fn read_control<T: Message + Default>(
        &mut self,
        expected_type: FrameType,
        expected_stream_id: u64,
    ) -> io::Result<T> {
        let frame = self.read_required_frame(expected_type, expected_stream_id)?;
        decode_control(&frame)
    }

    pub fn write_frame(&mut self, mut frame: Frame) -> io::Result<()> {
        frame.frame_sequence = self.next_frame_sequence;
        self.next_frame_sequence = self
            .next_frame_sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("v5 frame sequence exhausted"))?;
        write_frame_with_limits(&mut self.writer, &frame, self.limits)
    }

    pub fn write_control<T: Message>(
        &mut self,
        frame_type: FrameType,
        stream_id: u64,
        control: &T,
    ) -> io::Result<()> {
        self.write_frame(Frame::from_control(frame_type, stream_id, control))
    }
}

pub fn write_frame<W: Write>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    write_frame_with_limits(writer, frame, FrameLimits::default())
}

pub fn write_frame_with_limits<W: Write>(
    writer: &mut W,
    frame: &Frame,
    limits: FrameLimits,
) -> io::Result<()> {
    validate_lengths(frame.control.len(), frame.body.len(), limits)?;

    let mut fixed = [0_u8; FRAME_HEADER_LEN];
    fixed[0..4].copy_from_slice(&FRAME_MAGIC);
    fixed[4..6].copy_from_slice(&FRAME_HEADER_VERSION.to_be_bytes());
    fixed[6..8].copy_from_slice(&(frame.frame_type as u16).to_be_bytes());
    fixed[8..10].copy_from_slice(&frame.flags.to_be_bytes());
    fixed[10] = frame.priority;
    fixed[12..20].copy_from_slice(&frame.stream_id.to_be_bytes());
    fixed[20..28].copy_from_slice(&frame.frame_sequence.to_be_bytes());
    fixed[28..32].copy_from_slice(&(frame.control.len() as u32).to_be_bytes());
    fixed[32..36].copy_from_slice(&(frame.body.len() as u32).to_be_bytes());

    writer.write_all(&fixed)?;
    writer.write_all(&frame.control)?;
    writer.write_all(&frame.body)?;
    writer.flush()
}

pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Frame>> {
    read_frame_with_limits(reader, FrameLimits::default())
}

pub fn read_frame_with_limits<R: Read>(
    reader: &mut R,
    limits: FrameLimits,
) -> io::Result<Option<Frame>> {
    let mut fixed = [0_u8; FRAME_HEADER_LEN];
    match reader.read(&mut fixed[..1])? {
        0 => return Ok(None),
        1 => reader.read_exact(&mut fixed[1..])?,
        _ => unreachable!("read buffer length is one byte"),
    }

    if fixed[0..4] != FRAME_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid v5 frame magic",
        ));
    }

    let frame_header_version = u16::from_be_bytes([fixed[4], fixed[5]]);
    if frame_header_version != FRAME_HEADER_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported v5 frame header version: {frame_header_version}"),
        ));
    }

    let frame_type = FrameType::try_from(u16::from_be_bytes([fixed[6], fixed[7]]))?;
    let flags = u16::from_be_bytes([fixed[8], fixed[9]]);
    let priority = fixed[10];
    let stream_id = u64::from_be_bytes([
        fixed[12], fixed[13], fixed[14], fixed[15], fixed[16], fixed[17], fixed[18], fixed[19],
    ]);
    let frame_sequence = u64::from_be_bytes([
        fixed[20], fixed[21], fixed[22], fixed[23], fixed[24], fixed[25], fixed[26], fixed[27],
    ]);
    let control_len = u32::from_be_bytes([fixed[28], fixed[29], fixed[30], fixed[31]]);
    let body_len = u32::from_be_bytes([fixed[32], fixed[33], fixed[34], fixed[35]]);

    validate_lengths(control_len as usize, body_len as usize, limits)?;

    let mut control = vec![0_u8; control_len as usize];
    reader.read_exact(&mut control)?;
    let mut body = vec![0_u8; body_len as usize];
    reader.read_exact(&mut body)?;

    Ok(Some(Frame {
        frame_type,
        flags,
        priority,
        stream_id,
        frame_sequence,
        control,
        body,
    }))
}

fn validate_lengths(control_len: usize, body_len: usize, limits: FrameLimits) -> io::Result<()> {
    if control_len > limits.max_control_len as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "v5 frame control exceeds maximum length",
        ));
    }
    if body_len > limits.max_body_len as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "v5 frame body exceeds maximum length",
        ));
    }
    Ok(())
}

fn decode_control<T: Message + Default>(frame: &Frame) -> io::Result<T> {
    frame
        .decode_control()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn require_nonzero_stream(frame: &Frame) -> io::Result<()> {
    if frame.stream_id == 0 {
        return Err(protocol_error(format!(
            "{:?} requires a non-zero stream id",
            frame.frame_type
        )));
    }
    Ok(())
}

fn remote_initiator(local: StreamInitiator) -> StreamInitiator {
    match local {
        StreamInitiator::Client => StreamInitiator::Server,
        StreamInitiator::Server => StreamInitiator::Client,
    }
}

fn stream_id_initiator(stream_id: u64) -> io::Result<StreamInitiator> {
    if stream_id == 0 {
        return Err(protocol_error("stream 0 has no initiator"));
    }
    if stream_id.is_multiple_of(2) {
        Ok(StreamInitiator::Server)
    } else {
        Ok(StreamInitiator::Client)
    }
}

fn protocol_error(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn control_budget_error(message: impl Into<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::OutOfMemory,
        format!("v5 control queue RESOURCE_EXHAUSTED: {}", message.into()),
    )
}

fn is_stream_ids_exhausted_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Other && error.to_string() == STREAM_IDS_EXHAUSTED_MESSAGE
}

fn nonzero_or(value: u32, default: u32) -> u32 {
    if value == 0 { default } else { value }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum Priority {
    UserInput = 0,
    ForegroundDocument = 1,
    VisibleFileTree = 2,
    LspSupport = 3,
    Background = 4,
    Bulk = 5,
}

impl Priority {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    fn queue_index_from_wire(priority: u8) -> usize {
        usize::from(priority).min(PRIORITY_LEVELS - 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum MessageRole {
    Request = 0,
    PartialResult = 1,
    FinalResponse = 2,
    FinalError = 3,
    Progress = 4,
    Event = 5,
}

impl MessageRole {
    pub fn is_final(self) -> bool {
        matches!(self, Self::FinalResponse | Self::FinalError)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum ContentEncoding {
    None = 0,
    Zstd = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum DataChannel {
    Unspecified = 0,
    Stdin = 1,
    Stdout = 2,
    Stderr = 3,
    FileBody = 4,
    SearchPayload = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum Idempotency {
    ReadOnly = 0,
    Mutation = 1,
    Process = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum WatchMode {
    ExpandedDirs = 0,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum WatchIgnorePolicy {
    Workspace = 0,
    None = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum RecursiveCoverage {
    None = 0,
    Partial = 1,
    Full = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum WatchChangeKind {
    Created = 0,
    Modified = 1,
    Deleted = 2,
    Renamed = 3,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ConnectionSettings {
    #[prost(uint32, tag = "1")]
    pub max_concurrent_streams: u32,
    #[prost(uint32, tag = "2")]
    pub initial_stream_window: u32,
    #[prost(uint32, tag = "3")]
    pub initial_connection_window: u32,
    #[prost(uint32, tag = "4")]
    pub max_frame_body: u32,
    #[prost(uint32, tag = "5")]
    pub max_control_len: u32,
    #[prost(uint32, tag = "6")]
    pub connection_control_budget: u32,
    #[prost(uint32, tag = "7")]
    pub stream_control_budget: u32,
    #[prost(uint32, tag = "8")]
    pub shutdown_grace_ms: u32,
    #[prost(uint32, tag = "9")]
    pub idle_ping_interval_ms: u32,
    #[prost(uint32, tag = "10")]
    pub ping_timeout_ms: u32,
    #[prost(uint32, tag = "11")]
    pub min_unsolicited_ping_interval_ms: u32,
}

impl ConnectionSettings {
    pub fn recommended() -> Self {
        Self {
            max_concurrent_streams: DEFAULT_MAX_CONCURRENT_STREAMS,
            initial_stream_window: DEFAULT_STREAM_WINDOW,
            initial_connection_window: DEFAULT_CONNECTION_WINDOW,
            max_frame_body: DEFAULT_MAX_FRAME_BODY_LEN,
            max_control_len: DEFAULT_MAX_CONTROL_LEN,
            connection_control_budget: DEFAULT_CONNECTION_CONTROL_BUDGET,
            stream_control_budget: DEFAULT_STREAM_CONTROL_BUDGET,
            shutdown_grace_ms: DEFAULT_SHUTDOWN_GRACE_MS,
            idle_ping_interval_ms: IDLE_PING_INTERVAL_MS,
            ping_timeout_ms: PING_TIMEOUT_MS,
            min_unsolicited_ping_interval_ms: MIN_UNSOLICITED_PING_INTERVAL_MS,
        }
    }

    pub fn accept_peer_desired(desired: Option<&Self>) -> Self {
        let desired = desired.cloned().unwrap_or_else(Self::recommended);
        Self {
            max_concurrent_streams: nonzero_or(
                desired.max_concurrent_streams,
                DEFAULT_MAX_CONCURRENT_STREAMS,
            )
            .min(DEFAULT_MAX_CONCURRENT_STREAMS),
            initial_stream_window: nonzero_or(desired.initial_stream_window, DEFAULT_STREAM_WINDOW)
                .min(DEFAULT_STREAM_WINDOW),
            initial_connection_window: nonzero_or(
                desired.initial_connection_window,
                DEFAULT_CONNECTION_WINDOW,
            )
            .min(DEFAULT_CONNECTION_WINDOW),
            max_frame_body: nonzero_or(desired.max_frame_body, DEFAULT_MAX_FRAME_BODY_LEN)
                .min(MAX_NEGOTIATED_FRAME_BODY_LEN),
            max_control_len: nonzero_or(desired.max_control_len, DEFAULT_MAX_CONTROL_LEN)
                .min(DEFAULT_MAX_CONTROL_LEN),
            connection_control_budget: nonzero_or(
                desired.connection_control_budget,
                DEFAULT_CONNECTION_CONTROL_BUDGET,
            )
            .min(DEFAULT_CONNECTION_CONTROL_BUDGET),
            stream_control_budget: nonzero_or(
                desired.stream_control_budget,
                DEFAULT_STREAM_CONTROL_BUDGET,
            )
            .min(DEFAULT_STREAM_CONTROL_BUDGET),
            shutdown_grace_ms: nonzero_or(desired.shutdown_grace_ms, DEFAULT_SHUTDOWN_GRACE_MS),
            idle_ping_interval_ms: nonzero_or(desired.idle_ping_interval_ms, IDLE_PING_INTERVAL_MS),
            ping_timeout_ms: nonzero_or(desired.ping_timeout_ms, PING_TIMEOUT_MS),
            min_unsolicited_ping_interval_ms: nonzero_or(
                desired.min_unsolicited_ping_interval_ms,
                MIN_UNSOLICITED_PING_INTERVAL_MS,
            )
            .max(MIN_UNSOLICITED_PING_INTERVAL_MS),
        }
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ClientHello {
    #[prost(uint32, tag = "1")]
    pub protocol_major: u32,
    #[prost(uint32, tag = "2")]
    pub protocol_minor: u32,
    #[prost(string, tag = "3")]
    pub client_name: String,
    #[prost(string, tag = "4")]
    pub client_version: String,
    #[prost(string, repeated, tag = "5")]
    pub control_codecs: Vec<String>,
    #[prost(string, repeated, tag = "6")]
    pub capabilities: Vec<String>,
    #[prost(message, optional, tag = "7")]
    pub desired_settings: Option<ConnectionSettings>,
    #[prost(string, repeated, tag = "8")]
    pub required_capabilities: Vec<String>,
}

impl ClientHello {
    pub fn nucleotide(client_version: impl Into<String>) -> Self {
        Self {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            client_name: "nucleotide".to_string(),
            client_version: client_version.into(),
            control_codecs: vec!["protobuf".to_string()],
            capabilities: default_client_capabilities(),
            desired_settings: Some(ConnectionSettings::recommended()),
            required_capabilities: Vec::new(),
        }
    }

    pub fn supports_control_codec(&self, codec: &str) -> bool {
        self.control_codecs
            .iter()
            .any(|candidate| candidate == codec)
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ServerHello {
    #[prost(uint32, tag = "1")]
    pub protocol_major: u32,
    #[prost(uint32, tag = "2")]
    pub protocol_minor: u32,
    #[prost(string, tag = "3")]
    pub helper_version: String,
    #[prost(string, tag = "4")]
    pub os: String,
    #[prost(string, tag = "5")]
    pub arch: String,
    #[prost(string, tag = "6")]
    pub workspace_root: String,
    #[prost(string, tag = "7")]
    pub control_codec: String,
    #[prost(string, repeated, tag = "8")]
    pub capabilities: Vec<String>,
    #[prost(message, optional, tag = "9")]
    pub accepted_settings: Option<ConnectionSettings>,
}

impl ServerHello {
    pub fn accept_client(client: &ClientHello, info: &ServerHandshakeInfo) -> io::Result<Self> {
        validate_client_hello(client)?;
        validate_required_capabilities(client, info)?;
        let accepted_settings =
            ConnectionSettings::accept_peer_desired(client.desired_settings.as_ref());
        Ok(Self {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            helper_version: info.helper_version.clone(),
            os: info.os.clone(),
            arch: info.arch.clone(),
            workspace_root: info.workspace_root.clone(),
            control_codec: "protobuf".to_string(),
            capabilities: intersect_capabilities(client, &info.capabilities),
            accepted_settings: Some(accepted_settings),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHandshakeInfo {
    pub helper_version: String,
    pub os: String,
    pub arch: String,
    pub workspace_root: String,
    pub capabilities: Vec<String>,
}

impl ServerHandshakeInfo {
    pub fn current(workspace_root: impl Into<String>) -> Self {
        Self {
            helper_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            workspace_root: workspace_root.into(),
            capabilities: default_client_capabilities(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct ClientHandshake {
    pub client_hello: ClientHello,
    pub server_hello: ServerHello,
    pub settings: ConnectionSettings,
}

#[derive(Clone, PartialEq)]
pub struct ServerHandshake {
    pub client_hello: ClientHello,
    pub server_hello: ServerHello,
    pub settings: ConnectionSettings,
}

pub fn client_handshake<R: Read, W: Write>(
    io: &mut FramedIo<R, W>,
    client_hello: ClientHello,
) -> io::Result<ClientHandshake> {
    validate_client_hello(&client_hello)?;
    io.write_control(FrameType::Hello, 0, &client_hello)?;

    let server_hello = io.read_control::<ServerHello>(FrameType::Hello, 0)?;
    validate_server_hello(&server_hello)?;
    validate_server_capabilities(&client_hello, &server_hello)?;
    let settings = io.read_control::<ConnectionSettings>(FrameType::Settings, 0)?;
    let accepted_limits = FrameLimits::from_settings(&settings);
    io.write_frame(Frame::new(FrameType::SettingsAck, 0))?;
    io.set_limits(accepted_limits);

    Ok(ClientHandshake {
        client_hello,
        server_hello,
        settings,
    })
}

pub fn server_handshake<R: Read, W: Write>(
    io: &mut FramedIo<R, W>,
    info: &ServerHandshakeInfo,
) -> io::Result<ServerHandshake> {
    let client_hello = io.read_control::<ClientHello>(FrameType::Hello, 0)?;
    let server_hello = ServerHello::accept_client(&client_hello, info)?;
    let settings = server_hello
        .accepted_settings
        .clone()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing accepted settings"))?;

    io.write_control(FrameType::Hello, 0, &server_hello)?;
    io.write_control(FrameType::Settings, 0, &settings)?;
    let ack = io.read_required_frame(FrameType::SettingsAck, 0)?;
    if !ack.control.is_empty() || !ack.body.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SETTINGS_ACK must not carry control or body bytes",
        ));
    }
    io.set_limits(FrameLimits::from_settings(&settings));

    Ok(ServerHandshake {
        client_hello,
        server_hello,
        settings,
    })
}

fn validate_client_hello(client: &ClientHello) -> io::Result<()> {
    if client.protocol_major != PROTOCOL_MAJOR {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported v5 protocol major from client: {}",
                client.protocol_major
            ),
        ));
    }
    if !client.supports_control_codec("protobuf") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "client does not support protobuf control codec",
        ));
    }
    Ok(())
}

fn validate_server_hello(server: &ServerHello) -> io::Result<()> {
    if server.protocol_major != PROTOCOL_MAJOR {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported v5 protocol major from server: {}",
                server.protocol_major
            ),
        ));
    }
    if server.control_codec != "protobuf" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported v5 control codec from server: {}",
                server.control_codec
            ),
        ));
    }
    if server.accepted_settings.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "server hello missing accepted settings",
        ));
    }
    Ok(())
}

fn validate_required_capabilities(
    client: &ClientHello,
    info: &ServerHandshakeInfo,
) -> io::Result<()> {
    if let Some(capability) = client
        .required_capabilities
        .iter()
        .find(|capability| !info.capabilities.contains(capability))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("UNSUPPORTED_CAPABILITY: required v5 capability is unavailable: {capability}"),
        ));
    }
    Ok(())
}

fn validate_server_capabilities(client: &ClientHello, server: &ServerHello) -> io::Result<()> {
    if let Some(capability) = client
        .required_capabilities
        .iter()
        .find(|capability| !server.capabilities.contains(capability))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "UNSUPPORTED_CAPABILITY: server did not accept required v5 capability: {capability}"
            ),
        ));
    }
    Ok(())
}

fn client_requested_capabilities(client: &ClientHello) -> Vec<String> {
    let mut requested = client.capabilities.clone();
    for capability in &client.required_capabilities {
        if !requested.contains(capability) {
            requested.push(capability.clone());
        }
    }
    requested
}

fn intersect_capabilities(client: &ClientHello, server: &[String]) -> Vec<String> {
    let requested = client_requested_capabilities(client);
    server
        .iter()
        .filter(|capability| requested.contains(capability))
        .cloned()
        .collect()
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct StreamEnvelope {
    #[prost(uint64, tag = "1")]
    pub request_id: u64,
    #[prost(string, tag = "2")]
    pub method: String,
    #[prost(uint64, tag = "3")]
    pub correlation_id: u64,
    #[prost(uint64, tag = "4")]
    pub deadline_unix_ms: u64,
    #[prost(enumeration = "Priority", tag = "5")]
    pub priority: i32,
    #[prost(enumeration = "MessageRole", tag = "6")]
    pub role: i32,
    #[prost(string, tag = "7")]
    pub cancellation_group: String,
    #[prost(uint64, tag = "8")]
    pub supersedes_stream_id: u64,
    #[prost(enumeration = "ContentEncoding", tag = "9")]
    pub content_encoding: i32,
    #[prost(oneof = "stream_envelope::Message", tags = "10, 11, 12, 13, 14")]
    pub message: Option<stream_envelope::Message>,
}

impl StreamEnvelope {
    pub fn request(stream_id: u64, method: impl Into<String>) -> Self {
        Self::request_with_options(stream_id, method, &RequestOptions::default())
    }

    pub fn request_with_options(
        stream_id: u64,
        method: impl Into<String>,
        options: &RequestOptions,
    ) -> Self {
        Self {
            request_id: stream_id,
            method: method.into(),
            correlation_id: 0,
            deadline_unix_ms: options.deadline_unix_ms,
            priority: options.priority as i32,
            role: MessageRole::Request as i32,
            cancellation_group: options.cancellation_group.clone(),
            supersedes_stream_id: options.supersedes_stream_id,
            content_encoding: options.content_encoding as i32,
            message: Some(stream_envelope::Message::Request(RequestHeader {
                idempotency: options.idempotency as i32,
            })),
        }
    }

    pub fn response(
        stream_id: u64,
        method: impl Into<String>,
        role: MessageRole,
        complete: bool,
    ) -> Self {
        Self {
            request_id: stream_id,
            method: method.into(),
            correlation_id: 0,
            deadline_unix_ms: 0,
            priority: Priority::Background as i32,
            role: role as i32,
            cancellation_group: String::new(),
            supersedes_stream_id: 0,
            content_encoding: ContentEncoding::None as i32,
            message: Some(stream_envelope::Message::Response(ResponseHeader {
                complete,
                generation: 0,
            })),
        }
    }

    pub fn progress(stream_id: u64, method: impl Into<String>, progress: Progress) -> Self {
        Self {
            request_id: stream_id,
            method: method.into(),
            correlation_id: 0,
            deadline_unix_ms: 0,
            priority: Priority::Background as i32,
            role: MessageRole::Progress as i32,
            cancellation_group: String::new(),
            supersedes_stream_id: 0,
            content_encoding: ContentEncoding::None as i32,
            message: Some(stream_envelope::Message::Progress(progress)),
        }
    }

    pub fn error(stream_id: u64, method: impl Into<String>, error: ErrorHeader) -> Self {
        Self {
            request_id: stream_id,
            method: method.into(),
            correlation_id: 0,
            deadline_unix_ms: 0,
            priority: Priority::Background as i32,
            role: MessageRole::FinalError as i32,
            cancellation_group: String::new(),
            supersedes_stream_id: 0,
            content_encoding: ContentEncoding::None as i32,
            message: Some(stream_envelope::Message::Error(error)),
        }
    }

    pub fn event(_stream_id: u64, method: impl Into<String>, watch_id: u64) -> Self {
        Self {
            request_id: 0,
            method: method.into(),
            correlation_id: 0,
            deadline_unix_ms: 0,
            priority: Priority::VisibleFileTree as i32,
            role: MessageRole::Event as i32,
            cancellation_group: String::new(),
            supersedes_stream_id: 0,
            content_encoding: ContentEncoding::None as i32,
            message: Some(stream_envelope::Message::Event(Event {
                kind: "watch.batch".to_string(),
                watch_id,
                watch_batch: None,
            })),
        }
    }

    pub fn watch_batch(batch: WatchBatch) -> Self {
        let watch_id = batch.watch_id;
        Self {
            request_id: 0,
            method: "watch.batch".to_string(),
            correlation_id: 0,
            deadline_unix_ms: 0,
            priority: Priority::VisibleFileTree as i32,
            role: MessageRole::Event as i32,
            cancellation_group: String::new(),
            supersedes_stream_id: 0,
            content_encoding: ContentEncoding::None as i32,
            message: Some(stream_envelope::Message::Event(Event {
                kind: "watch.batch".to_string(),
                watch_id,
                watch_batch: Some(batch),
            })),
        }
    }

    pub fn message_role(&self) -> io::Result<MessageRole> {
        MessageRole::try_from(self.role)
            .map_err(|_| protocol_error(format!("unknown v5 message role: {}", self.role)))
    }

    pub fn decode_content_encoding(&self) -> io::Result<ContentEncoding> {
        ContentEncoding::try_from(self.content_encoding).map_err(|_| {
            protocol_error(format!(
                "unknown v5 content encoding: {}",
                self.content_encoding
            ))
        })
    }

    pub fn request_metadata(&self) -> io::Result<RequestMetadata> {
        if self.message_role()? != MessageRole::Request {
            return Err(protocol_error("request metadata requires request role"));
        }
        Ok(RequestMetadata {
            method: self.method.clone(),
            cancellation_group: self.cancellation_group.clone(),
            deadline_unix_ms: self.deadline_unix_ms,
            supersedes_stream_id: self.supersedes_stream_id,
            idempotency: self.request_idempotency()?,
        })
    }

    pub fn request_idempotency(&self) -> io::Result<Idempotency> {
        match self.message.as_ref() {
            Some(stream_envelope::Message::Request(request)) => {
                Idempotency::try_from(request.idempotency).map_err(|_| {
                    protocol_error(format!(
                        "unknown v5 request idempotency: {}",
                        request.idempotency
                    ))
                })
            }
            _ => Err(protocol_error("request headers missing RequestHeader")),
        }
    }
}

pub mod stream_envelope {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Message {
        #[prost(message, tag = "10")]
        Request(super::RequestHeader),
        #[prost(message, tag = "11")]
        Response(super::ResponseHeader),
        #[prost(message, tag = "12")]
        Error(super::ErrorHeader),
        #[prost(message, tag = "13")]
        Progress(super::Progress),
        #[prost(message, tag = "14")]
        Event(super::Event),
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RequestHeader {
    #[prost(enumeration = "Idempotency", tag = "1")]
    pub idempotency: i32,
}

impl RequestHeader {
    pub fn read_only() -> Self {
        Self {
            idempotency: Idempotency::ReadOnly as i32,
        }
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ResponseHeader {
    #[prost(bool, tag = "1")]
    pub complete: bool,
    #[prost(uint64, tag = "2")]
    pub generation: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ErrorHeader {
    #[prost(string, tag = "1")]
    pub code: String,
    #[prost(string, tag = "2")]
    pub message: String,
    #[prost(bool, tag = "3")]
    pub retryable: bool,
    #[prost(string, tag = "4")]
    pub details: String,
    #[prost(int32, tag = "5")]
    pub remote_errno: i32,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Progress {
    #[prost(string, tag = "1")]
    pub message: String,
    #[prost(uint64, tag = "2")]
    pub completed: u64,
    #[prost(uint64, tag = "3")]
    pub total: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Event {
    #[prost(string, tag = "1")]
    pub kind: String,
    #[prost(uint64, tag = "2")]
    pub watch_id: u64,
    #[prost(message, optional, tag = "3")]
    pub watch_batch: Option<WatchBatch>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchStart {
    #[prost(string, repeated, tag = "1")]
    pub roots: Vec<String>,
    #[prost(enumeration = "WatchMode", tag = "2")]
    pub mode: i32,
    #[prost(bool, tag = "3")]
    pub recursive: bool,
    #[prost(uint32, tag = "4")]
    pub debounce_ms: u32,
    #[prost(uint32, tag = "5")]
    pub max_events_per_batch: u32,
    #[prost(enumeration = "WatchIgnorePolicy", tag = "6")]
    pub ignore_policy: i32,
    #[prost(bool, tag = "7")]
    pub include_hidden: bool,
    #[prost(bool, tag = "8")]
    pub send_initial_snapshot: bool,
}

impl WatchStart {
    pub fn expanded_dirs(roots: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            roots: roots.into_iter().map(Into::into).collect(),
            mode: WatchMode::ExpandedDirs as i32,
            recursive: false,
            debounce_ms: 200,
            max_events_per_batch: 500,
            ignore_policy: WatchIgnorePolicy::Workspace as i32,
            include_hidden: true,
            send_initial_snapshot: false,
        }
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchStartResponse {
    #[prost(uint64, tag = "1")]
    pub watch_id: u64,
    #[prost(uint64, tag = "2")]
    pub event_stream_id: u64,
    #[prost(string, tag = "3")]
    pub backend: String,
    #[prost(enumeration = "RecursiveCoverage", tag = "4")]
    pub recursive_coverage: i32,
    #[prost(bool, tag = "5")]
    pub degraded: bool,
    #[prost(bool, tag = "6")]
    pub requires_reconciliation: bool,
    #[prost(string, repeated, tag = "7")]
    pub accepted_roots: Vec<String>,
    #[prost(string, repeated, tag = "8")]
    pub degraded_roots: Vec<String>,
    #[prost(string, repeated, tag = "9")]
    pub unsupported_roots: Vec<String>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchUpdate {
    #[prost(uint64, tag = "1")]
    pub watch_id: u64,
    #[prost(string, repeated, tag = "2")]
    pub add_roots: Vec<String>,
    #[prost(string, repeated, tag = "3")]
    pub remove_roots: Vec<String>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchUpdateResponse {
    #[prost(uint64, tag = "1")]
    pub watch_id: u64,
    #[prost(string, repeated, tag = "2")]
    pub accepted_roots: Vec<String>,
    #[prost(string, repeated, tag = "3")]
    pub degraded_roots: Vec<String>,
    #[prost(string, repeated, tag = "4")]
    pub unsupported_roots: Vec<String>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchStop {
    #[prost(uint64, tag = "1")]
    pub watch_id: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchResync {
    #[prost(uint64, tag = "1")]
    pub watch_id: u64,
    #[prost(string, repeated, tag = "2")]
    pub roots: Vec<String>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchResyncResponse {
    #[prost(uint64, tag = "1")]
    pub watch_id: u64,
    #[prost(string, repeated, tag = "2")]
    pub accepted_roots: Vec<String>,
    #[prost(string, repeated, tag = "3")]
    pub unsupported_roots: Vec<String>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchDirectoryGeneration {
    #[prost(string, tag = "1")]
    pub path: String,
    #[prost(uint64, tag = "2")]
    pub generation: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchChange {
    #[prost(enumeration = "WatchChangeKind", tag = "1")]
    pub kind: i32,
    #[prost(string, tag = "2")]
    pub path: String,
    #[prost(string, tag = "3")]
    pub old_path: String,
    #[prost(bool, tag = "4")]
    pub is_dir: bool,
}

impl WatchChange {
    pub fn created(path: impl Into<String>, is_dir: bool) -> Self {
        Self::new(WatchChangeKind::Created, path, "", is_dir)
    }

    pub fn modified(path: impl Into<String>, is_dir: bool) -> Self {
        Self::new(WatchChangeKind::Modified, path, "", is_dir)
    }

    pub fn deleted(path: impl Into<String>, is_dir: bool) -> Self {
        Self::new(WatchChangeKind::Deleted, path, "", is_dir)
    }

    pub fn renamed(old_path: impl Into<String>, path: impl Into<String>, is_dir: bool) -> Self {
        Self::new(WatchChangeKind::Renamed, path, old_path, is_dir)
    }

    fn new(
        kind: WatchChangeKind,
        path: impl Into<String>,
        old_path: impl Into<String>,
        is_dir: bool,
    ) -> Self {
        Self {
            kind: kind as i32,
            path: path.into(),
            old_path: old_path.into(),
            is_dir,
        }
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WatchBatch {
    #[prost(uint64, tag = "1")]
    pub watch_id: u64,
    #[prost(uint64, tag = "2")]
    pub sequence: u64,
    #[prost(message, repeated, tag = "3")]
    pub directory_generations: Vec<WatchDirectoryGeneration>,
    #[prost(message, repeated, tag = "4")]
    pub events: Vec<WatchChange>,
    #[prost(bool, tag = "5")]
    pub overflow: bool,
    #[prost(bool, tag = "6")]
    pub resync_required: bool,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DataEnvelope {
    #[prost(enumeration = "DataChannel", tag = "1")]
    pub channel: i32,
    #[prost(uint64, tag = "2")]
    pub uncompressed_len: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WindowUpdate {
    #[prost(uint64, tag = "1")]
    pub credit_bytes: u64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ResetStream {
    #[prost(string, tag = "1")]
    pub code: String,
    #[prost(string, tag = "2")]
    pub diagnostic: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PingPayload {
    #[prost(bytes = "vec", tag = "1")]
    pub token: Vec<u8>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GoAway {
    #[prost(uint64, tag = "1")]
    pub last_accepted_stream_id: u64,
    #[prost(string, tag = "2")]
    pub code: String,
    #[prost(string, tag = "3")]
    pub message: String,
    #[prost(uint32, tag = "4")]
    pub drain_grace_ms: u32,
}

pub fn default_client_capabilities() -> Vec<String> {
    [
        "multiplex",
        "cancel",
        "progress",
        "partial_results",
        "streaming_read",
        "streaming_write",
        "process_streams",
        "watch",
        "watch_overflow",
        "directory_not_modified",
        "compression_zstd",
        "external_read_only",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn headers_frame(stream_id: u64, envelope: StreamEnvelope) -> Frame {
        Frame::from_control(FrameType::Headers, stream_id, &envelope)
    }

    fn data_frame(stream_id: u64, body_len: usize) -> Frame {
        let mut frame = Frame::from_control(
            FrameType::Data,
            stream_id,
            &DataEnvelope {
                channel: DataChannel::FileBody as i32,
                uncompressed_len: body_len as u64,
            },
        );
        frame.body = vec![0; body_len];
        frame
    }

    fn reset_frame(stream_id: u64) -> Frame {
        Frame::from_control(
            FrameType::ResetStream,
            stream_id,
            &ResetStream {
                code: "cancelled".to_string(),
                diagnostic: String::new(),
            },
        )
    }

    #[test]
    fn frame_round_trip_preserves_header_control_and_body() {
        let mut frame = Frame::new(FrameType::Headers, 0x0102_0304_0506_0708);
        frame.flags = 0x1234;
        frame.priority = Priority::VisibleFileTree as u8;
        frame.frame_sequence = 0x1112_1314_1516_1718;
        frame.control = vec![1, 2, 3];
        frame.body = b"payload".to_vec();

        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();

        assert_eq!(&bytes[0..4], b"NUC2");
        assert_eq!(
            u16::from_be_bytes([bytes[4], bytes[5]]),
            FRAME_HEADER_VERSION
        );
        assert_eq!(
            u16::from_be_bytes([bytes[6], bytes[7]]),
            FrameType::Headers as u16
        );
        assert_eq!(
            u64::from_be_bytes(bytes[12..20].try_into().unwrap()),
            frame.stream_id
        );
        assert_eq!(
            u64::from_be_bytes(bytes[20..28].try_into().unwrap()),
            frame.frame_sequence
        );
        assert_eq!(u32::from_be_bytes(bytes[28..32].try_into().unwrap()), 3);
        assert_eq!(u32::from_be_bytes(bytes[32..36].try_into().unwrap()), 7);

        let decoded = read_frame(&mut Cursor::new(bytes)).unwrap().unwrap();

        assert_eq!(decoded, frame);
    }

    #[test]
    fn frame_reader_returns_none_on_clean_eof() {
        assert!(read_frame(&mut Cursor::new(Vec::new())).unwrap().is_none());
    }

    #[test]
    fn frame_reader_rejects_invalid_magic() {
        let mut frame = Frame::new(FrameType::Ping, 0);
        frame.control = PingPayload {
            token: b"ping".to_vec(),
        }
        .encode_to_vec();
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        bytes[0..4].copy_from_slice(b"NUCL");

        let error = read_frame(&mut Cursor::new(bytes)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("invalid v5 frame magic"));
    }

    #[test]
    fn frame_reader_rejects_unsupported_header_version() {
        let frame = Frame::new(FrameType::Ping, 0);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        bytes[4..6].copy_from_slice(&99_u16.to_be_bytes());

        let error = read_frame(&mut Cursor::new(bytes)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .contains("unsupported v5 frame header version")
        );
    }

    #[test]
    fn framed_io_assigns_monotonic_outgoing_sequences() {
        let mut io = FramedIo::new(Cursor::new(Vec::new()), Vec::new());

        io.write_frame(Frame::new(FrameType::Ping, 0)).unwrap();
        io.write_frame(Frame::new(FrameType::Pong, 0)).unwrap();
        let (_, bytes) = io.into_inner();

        let mut reader = Cursor::new(bytes);
        let first = read_frame(&mut reader).unwrap().unwrap();
        let second = read_frame(&mut reader).unwrap().unwrap();

        assert_eq!(first.frame_sequence, 1);
        assert_eq!(second.frame_sequence, 2);
        assert_eq!(first.frame_type, FrameType::Ping);
        assert_eq!(second.frame_type, FrameType::Pong);
    }

    #[test]
    fn frame_type_flow_control_classification_matches_spec() {
        let frame_types = [
            FrameType::Hello,
            FrameType::Settings,
            FrameType::SettingsAck,
            FrameType::Headers,
            FrameType::Data,
            FrameType::EndStream,
            FrameType::ResetStream,
            FrameType::WindowUpdate,
            FrameType::Ping,
            FrameType::Pong,
            FrameType::GoAway,
        ];

        for frame_type in frame_types {
            assert_eq!(
                frame_type.consumes_flow_window(),
                matches!(frame_type, FrameType::Data),
                "{frame_type:?}"
            );
        }

        assert!(FrameType::Hello.is_connection_control());
        assert!(FrameType::WindowUpdate.is_connection_control());
        assert!(FrameType::GoAway.is_connection_control());
        assert!(!FrameType::Headers.is_connection_control());
        assert!(!FrameType::Data.is_connection_control());
    }

    #[test]
    fn frame_flow_control_len_counts_only_data_body_bytes() {
        let mut data = Frame::from_control(
            FrameType::Data,
            3,
            &DataEnvelope {
                channel: DataChannel::FileBody as i32,
                uncompressed_len: 100,
            },
        );
        data.body = vec![0; 100];
        let mut headers = Frame::from_control(
            FrameType::Headers,
            3,
            &StreamEnvelope::request(3, "fs.read"),
        );
        headers.body = vec![0; 100];

        assert_eq!(data.flow_control_len(), 100);
        assert_eq!(headers.flow_control_len(), 0);
    }

    #[test]
    fn frame_control_budget_len_counts_only_non_data_frames() {
        let mut ping = Frame::new(FrameType::Ping, 0);
        ping.control = b"abc".to_vec();
        let mut headers = Frame::new(FrameType::Headers, 1);
        headers.control = b"request".to_vec();
        let mut data = data_frame(1, 100);
        data.control = b"metadata".to_vec();

        assert_eq!(ping.control_budget_len(), FRAME_HEADER_LEN as u64 + 3);
        assert_eq!(headers.control_budget_len(), FRAME_HEADER_LEN as u64 + 7);
        assert_eq!(data.control_budget_len(), 0);
    }

    #[test]
    fn flow_window_consumes_data_frames_and_rejects_insufficient_credit() {
        let mut window = FlowWindow::new(4);
        let mut frame = Frame::new(FrameType::Data, 1);
        frame.body = vec![0; 3];

        window.consume_frame(&frame).unwrap();
        assert_eq!(window.available(), 1);

        let error = window.consume(2).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(window.available(), 1);
    }

    #[test]
    fn flow_window_grants_credit_and_rejects_overflow() {
        let mut window = FlowWindow::new(1);

        window.grant(2).unwrap();
        assert_eq!(window.available(), 3);

        let error = window.grant(MAX_FLOW_WINDOW).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn stream_table_open_request_allocates_local_stream_and_tracks_local_end() {
        let mut table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());

        let (stream_id, frame) = table.open_request("fs.stat").unwrap();

        assert_eq!(stream_id, 1);
        assert_eq!(frame.frame_type, FrameType::Headers);
        assert_eq!(frame.stream_id, 1);
        assert_eq!(table.active_streams(), 1);
        let envelope = frame.decode_control::<StreamEnvelope>().unwrap();
        assert_eq!(envelope.message_role().unwrap(), MessageRole::Request);
        assert_eq!(envelope.request_id, 1);
        assert_eq!(envelope.method, "fs.stat");

        assert_eq!(
            table.mark_local_end(stream_id).unwrap(),
            StreamState::HalfClosedLocal
        );
        assert_eq!(
            table.get(stream_id).map(|entry| entry.state),
            Some(StreamState::HalfClosedLocal)
        );
    }

    #[test]
    fn stream_table_open_request_with_options_encodes_cancellation_metadata() {
        let mut table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let options = RequestOptions {
            priority: Priority::ForegroundDocument,
            cancellation_group: "search:rust".to_string(),
            deadline_unix_ms: 1234,
            supersedes_stream_id: 7,
            content_encoding: ContentEncoding::Zstd,
            idempotency: Idempotency::Process,
        };

        let (stream_id, frame) = table
            .open_request_with_options("search.text", options.clone())
            .unwrap();
        let envelope = frame.decode_control::<StreamEnvelope>().unwrap();

        assert_eq!(stream_id, 1);
        assert_eq!(frame.priority, Priority::ForegroundDocument.as_u8());
        assert_eq!(envelope.method, "search.text");
        assert_eq!(envelope.cancellation_group, options.cancellation_group);
        assert_eq!(envelope.deadline_unix_ms, 1234);
        assert_eq!(envelope.supersedes_stream_id, 7);
        assert_eq!(envelope.content_encoding, ContentEncoding::Zstd as i32);
        assert!(matches!(
            envelope.message,
            Some(stream_envelope::Message::Request(RequestHeader { idempotency }))
                if idempotency == Idempotency::Process as i32
        ));
        assert_eq!(
            table
                .get(stream_id)
                .map(|entry| entry.cancellation_group.as_str()),
            Some("search:rust")
        );
        assert_eq!(
            table.get(stream_id).map(|entry| entry.content_encoding),
            Some(ContentEncoding::Zstd)
        );
    }

    #[test]
    fn stream_table_open_event_stream_uses_local_server_stream_id() {
        let mut server_table =
            StreamTable::new(StreamInitiator::Server, &ConnectionSettings::recommended());

        let (stream_id, frame) = server_table.open_event_stream("watch.batch", 42).unwrap();

        assert_eq!(stream_id, 2);
        assert_eq!(frame.frame_type, FrameType::Headers);
        assert_eq!(frame.priority, Priority::VisibleFileTree.as_u8());
        let envelope = frame.decode_control::<StreamEnvelope>().unwrap();
        assert_eq!(envelope.request_id, 0);
        assert_eq!(envelope.method, "watch.batch");
        assert_eq!(envelope.message_role().unwrap(), MessageRole::Event);
        assert!(matches!(
            envelope.message,
            Some(stream_envelope::Message::Event(Event {
                watch_id: 42,
                watch_batch: None,
                ..
            }))
        ));
        assert_eq!(
            server_table.get(stream_id).map(|entry| entry.initiator),
            Some(StreamInitiator::Server)
        );

        let mut client_table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        assert_eq!(
            client_table.route_incoming(&frame).unwrap(),
            RoutedFrame::Headers {
                stream_id,
                role: MessageRole::Event,
                method: "watch.batch".to_string()
            }
        );
    }

    #[test]
    fn stream_table_tracks_opening_content_encoding_and_rejects_midstream_changes() {
        let mut table =
            StreamTable::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        let request = StreamEnvelope::request_with_options(
            1,
            "fs.read",
            &RequestOptions {
                content_encoding: ContentEncoding::Zstd,
                ..RequestOptions::default()
            },
        );

        table.route_incoming(&headers_frame(1, request)).unwrap();
        assert_eq!(
            table.get(1).map(|entry| entry.content_encoding),
            Some(ContentEncoding::Zstd)
        );

        let mut partial = StreamEnvelope::response(1, "fs.read", MessageRole::PartialResult, false);
        partial.content_encoding = ContentEncoding::Zstd as i32;
        let error = table
            .route_incoming(&headers_frame(1, partial))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("opening headers"));
    }

    #[test]
    fn stream_table_routes_inbound_request_data_and_end() {
        let mut table =
            StreamTable::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        let request = headers_frame(1, StreamEnvelope::request(1, "fs.read"));

        let routed = table.route_incoming(&request).unwrap();

        assert_eq!(
            routed,
            RoutedFrame::Headers {
                stream_id: 1,
                role: MessageRole::Request,
                method: "fs.read".to_string()
            }
        );
        assert_eq!(table.active_streams(), 1);
        assert_eq!(
            table.get(1).map(|entry| entry.initiator),
            Some(StreamInitiator::Client)
        );

        let routed = table.route_incoming(&data_frame(1, 12)).unwrap();
        assert_eq!(
            routed,
            RoutedFrame::Data {
                stream_id: 1,
                flow_control_len: 12
            }
        );

        let routed = table
            .route_incoming(&Frame::new(FrameType::EndStream, 1))
            .unwrap();
        assert_eq!(
            routed,
            RoutedFrame::EndStream {
                stream_id: 1,
                state: StreamState::HalfClosedRemote
            }
        );
        assert_eq!(
            table.get(1).map(|entry| entry.state),
            Some(StreamState::HalfClosedRemote)
        );

        let error = table.route_incoming(&data_frame(1, 1)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("after remote side closed"));
    }

    #[test]
    fn stream_table_rejects_opening_headers_with_wrong_stream_parity() {
        let mut table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let request = headers_frame(1, StreamEnvelope::request(1, "fs.read"));

        let error = table.route_incoming(&request).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("cannot be opened"));
    }

    #[test]
    fn stream_table_routes_partial_final_and_rejects_headers_after_final() {
        let mut table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let (stream_id, _) = table.open_request("search.text").unwrap();
        table.mark_local_end(stream_id).unwrap();

        let partial = headers_frame(
            stream_id,
            StreamEnvelope::response(stream_id, "search.text", MessageRole::PartialResult, false),
        );
        let final_response = headers_frame(
            stream_id,
            StreamEnvelope::response(stream_id, "search.text", MessageRole::FinalResponse, true),
        );

        assert_eq!(
            table.route_incoming(&partial).unwrap(),
            RoutedFrame::Headers {
                stream_id,
                role: MessageRole::PartialResult,
                method: "search.text".to_string()
            }
        );
        assert!(!table.get(stream_id).unwrap().final_seen);
        assert_eq!(
            table.route_incoming(&final_response).unwrap(),
            RoutedFrame::Headers {
                stream_id,
                role: MessageRole::FinalResponse,
                method: "search.text".to_string()
            }
        );
        assert!(table.get(stream_id).unwrap().final_seen);

        let error = table.route_incoming(&partial).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("after final response"));
    }

    #[test]
    fn stream_table_closes_and_removes_stream_after_both_sides_end() {
        let mut table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let (stream_id, _) = table.open_request("fs.read").unwrap();
        table.mark_local_end(stream_id).unwrap();

        let routed = table
            .route_incoming(&Frame::new(FrameType::EndStream, stream_id))
            .unwrap();

        assert_eq!(
            routed,
            RoutedFrame::EndStream {
                stream_id,
                state: StreamState::Closed
            }
        );
        assert_eq!(table.active_streams(), 0);
        assert!(table.get(stream_id).is_none());
    }

    #[test]
    fn stream_table_reset_removes_known_stream_and_tolerates_unknown_reset() {
        let mut table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let (stream_id, _) = table.open_request("fs.read").unwrap();

        assert_eq!(
            table.route_incoming(&reset_frame(stream_id)).unwrap(),
            RoutedFrame::ResetStream {
                stream_id,
                known: true
            }
        );
        assert_eq!(table.active_streams(), 0);
        assert_eq!(
            table.route_incoming(&reset_frame(stream_id)).unwrap(),
            RoutedFrame::ResetStream {
                stream_id,
                known: false
            }
        );
    }

    #[test]
    fn stream_table_rejects_data_on_unknown_stream() {
        let mut table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());

        let error = table.route_incoming(&data_frame(2, 1)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unknown v5 stream"));
    }

    #[test]
    fn stream_table_enforces_max_concurrent_streams() {
        let settings = ConnectionSettings {
            max_concurrent_streams: 1,
            ..ConnectionSettings::recommended()
        };
        let mut table = StreamTable::new(StreamInitiator::Server, &settings);

        table
            .route_incoming(&headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();
        let error = table
            .route_incoming(&headers_frame(3, StreamEnvelope::request(3, "fs.stat")))
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::OutOfMemory);
        assert!(error.to_string().contains("max concurrent streams"));
    }

    #[test]
    fn stream_table_routes_connection_control_and_window_update() {
        let mut table =
            StreamTable::new(StreamInitiator::Client, &ConnectionSettings::recommended());

        assert_eq!(
            table
                .route_incoming(&Frame::new(FrameType::Ping, 0))
                .unwrap(),
            RoutedFrame::ConnectionControl {
                frame_type: FrameType::Ping
            }
        );

        let mut ping_on_stream = Frame::new(FrameType::Ping, 1);
        ping_on_stream.stream_id = 1;
        let error = table.route_incoming(&ping_on_stream).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let update = Frame::from_control(
            FrameType::WindowUpdate,
            9,
            &WindowUpdate { credit_bytes: 42 },
        );
        assert_eq!(
            table.route_incoming(&update).unwrap(),
            RoutedFrame::WindowUpdate {
                stream_id: 9,
                credit_bytes: 42
            }
        );
    }

    #[test]
    fn stream_data_frame_encodes_channel_priority_and_uncompressed_len() {
        let frame = stream_data_frame(
            7,
            b"compressed".to_vec(),
            DataFrameOptions::new(DataChannel::Stdout)
                .with_priority(Priority::ForegroundDocument)
                .with_uncompressed_len(128),
        )
        .unwrap();

        assert_eq!(frame.frame_type, FrameType::Data);
        assert_eq!(frame.stream_id, 7);
        assert_eq!(frame.priority, Priority::ForegroundDocument.as_u8());
        assert_eq!(frame.body, b"compressed");
        let envelope = frame.decode_control::<DataEnvelope>().unwrap();
        assert_eq!(envelope.channel, DataChannel::Stdout as i32);
        assert_eq!(envelope.uncompressed_len, 128);

        let error = stream_data_frame(0, Vec::new(), DataFrameOptions::new(DataChannel::FileBody))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn data_chunker_splits_body_by_negotiated_body_limit() {
        let limits = FrameLimits {
            max_control_len: DEFAULT_MAX_CONTROL_LEN,
            max_body_len: 4,
        };
        let frames = DataChunker::with_options(
            9,
            b"abcdefghij",
            limits,
            DataFrameOptions::new(DataChannel::FileBody).with_priority(Priority::Bulk),
        )
        .unwrap()
        .collect::<Vec<_>>();

        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].body, b"abcd");
        assert_eq!(frames[1].body, b"efgh");
        assert_eq!(frames[2].body, b"ij");
        for frame in &frames {
            assert_eq!(frame.frame_type, FrameType::Data);
            assert_eq!(frame.stream_id, 9);
            assert_eq!(frame.priority, Priority::Bulk.as_u8());
            let envelope = frame.decode_control::<DataEnvelope>().unwrap();
            assert_eq!(envelope.channel, DataChannel::FileBody as i32);
            assert_eq!(envelope.uncompressed_len, frame.body.len() as u64);
            assert!(frame.body.len() <= limits.max_body_len as usize);
        }
    }

    #[test]
    fn data_chunker_rejects_invalid_stream_limit_and_static_uncompressed_len() {
        let limits = FrameLimits {
            max_control_len: DEFAULT_MAX_CONTROL_LEN,
            max_body_len: 0,
        };

        assert!(
            DataChunker::new(0, DataChannel::FileBody, b"bytes", FrameLimits::default())
                .unwrap_err()
                .to_string()
                .contains("non-zero stream")
        );
        assert!(
            DataChunker::new(1, DataChannel::FileBody, b"bytes", limits)
                .unwrap_err()
                .to_string()
                .contains("non-zero")
        );
        assert!(
            DataChunker::with_options(
                1,
                b"bytes",
                FrameLimits::default(),
                DataFrameOptions::new(DataChannel::FileBody).with_uncompressed_len(42),
            )
            .unwrap_err()
            .to_string()
            .contains("per frame")
        );
    }

    #[test]
    fn protocol_session_zstd_stream_compresses_and_decompresses_data_frames() {
        let mut settings = ConnectionSettings::recommended();
        settings.max_frame_body = 64;
        let mut client = ProtocolSession::new(StreamInitiator::Client, &settings);
        let mut server = ProtocolSession::new(StreamInitiator::Server, &settings);
        let options = RequestOptions {
            content_encoding: ContentEncoding::Zstd,
            ..RequestOptions::default()
        };
        let body = vec![b'a'; 256];

        let stream_id = client
            .open_request_with_body("search.text", options, DataChannel::SearchPayload, &body)
            .unwrap();
        let mut received = Vec::new();
        let mut saw_compressed_data = false;

        while let Some(frame) = client.pop_next_frame().unwrap() {
            let wire_body = frame.body.clone();
            let is_data = frame.frame_type == FrameType::Data;
            if is_data {
                assert!(wire_body.len() <= settings.max_frame_body as usize);
                let envelope = frame.decode_control::<DataEnvelope>().unwrap();
                assert!(envelope.uncompressed_len <= settings.max_frame_body as u64);
            }
            let event = server.receive_frame(frame).unwrap().stream_event;
            if let Some(StreamEvent::Data { body, .. }) = event {
                if wire_body != body {
                    saw_compressed_data = true;
                }
                received.extend(body);
            }
        }

        assert_eq!(stream_id, 1);
        assert!(saw_compressed_data);
        assert_eq!(received, body);
    }

    #[test]
    fn protocol_session_queues_window_updates_after_data_acknowledgement() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();

        let event = session.receive_frame(data_frame(1, 5)).unwrap();
        let data_credit = event.data_credit();

        assert_eq!(
            event.routed,
            RoutedFrame::Data {
                stream_id: 1,
                flow_control_len: 5
            }
        );
        assert!(matches!(
            event.stream_event,
            Some(StreamEvent::Data {
                stream_id: 1,
                uncompressed_len: 5,
                ..
            })
        ));
        assert_eq!(data_credit, Some((1, 5)));
        assert!(session.pop_next_frame().unwrap().is_none());

        session.acknowledge_data(1, 5).unwrap();

        let connection_update = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(connection_update.frame_type, FrameType::WindowUpdate);
        assert_eq!(connection_update.stream_id, 0);
        assert_eq!(
            connection_update
                .decode_control::<WindowUpdate>()
                .unwrap()
                .credit_bytes,
            5
        );

        let stream_update = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(stream_update.frame_type, FrameType::WindowUpdate);
        assert_eq!(stream_update.stream_id, 1);
        assert_eq!(
            stream_update
                .decode_control::<WindowUpdate>()
                .unwrap()
                .credit_bytes,
            5
        );
    }

    #[test]
    fn protocol_session_replies_to_ping_with_matching_pong() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        let mut ping = Frame::new(FrameType::Ping, 0);
        ping.control = PingPayload {
            token: b"health-check".to_vec(),
        }
        .encode_to_vec();

        let event = session.receive_frame(ping.clone()).unwrap();

        assert_eq!(
            event.routed,
            RoutedFrame::ConnectionControl {
                frame_type: FrameType::Ping
            }
        );
        assert!(event.stream_event.is_none());

        let pong = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(pong.frame_type, FrameType::Pong);
        assert_eq!(pong.stream_id, 0);
        assert_eq!(pong.control, ping.control);
    }

    #[test]
    fn protocol_session_goaway_reports_last_accepted_remote_stream() {
        let settings = ConnectionSettings {
            shutdown_grace_ms: 1_234,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.stat")))
            .unwrap();
        session
            .receive_frame(headers_frame(3, StreamEnvelope::request(3, "fs.read")))
            .unwrap();

        session.send_goaway("OK", "shutdown").unwrap();

        let frame = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::GoAway);
        assert_eq!(frame.stream_id, 0);
        let goaway = frame.decode_control::<GoAway>().unwrap();
        assert_eq!(goaway.last_accepted_stream_id, 3);
        assert_eq!(goaway.code, "OK");
        assert_eq!(goaway.message, "shutdown");
        assert_eq!(goaway.drain_grace_ms, 1_234);
    }

    #[test]
    fn protocol_session_records_peer_goaway_and_rejects_new_requests() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let goaway = GoAway {
            last_accepted_stream_id: 1,
            code: "OK".to_string(),
            message: "shutdown".to_string(),
            drain_grace_ms: 500,
        };

        let event = session
            .receive_frame(Frame::from_control(FrameType::GoAway, 0, &goaway))
            .unwrap();

        assert_eq!(
            event.routed,
            RoutedFrame::ConnectionControl {
                frame_type: FrameType::GoAway
            }
        );
        assert!(event.stream_event.is_none());
        assert_eq!(session.peer_goaway(), Some(&goaway));

        let error = session
            .open_unary_request("fs.stat", RequestOptions::default())
            .unwrap_err();
        assert!(error.to_string().contains("peer sent GOAWAY"));
        assert!(error.to_string().contains("last accepted stream was 1"));
    }

    #[test]
    fn protocol_session_rejects_new_event_stream_after_peer_goaway() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        session
            .receive_frame(Frame::from_control(
                FrameType::GoAway,
                0,
                &GoAway {
                    last_accepted_stream_id: 2,
                    code: "OK".to_string(),
                    message: "client draining".to_string(),
                    drain_grace_ms: 250,
                },
            ))
            .unwrap();

        let error = session.open_event_stream("watch.batch", 7).unwrap_err();

        assert!(error.to_string().contains("peer sent GOAWAY"));
        assert!(error.to_string().contains("last accepted stream was 2"));
    }

    #[test]
    fn protocol_session_resets_unknown_remote_stream_after_local_goaway() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.stat")))
            .unwrap();
        session.send_goaway("OK", "shutdown").unwrap();
        let sent = session.sent_goaway().cloned().unwrap();
        assert_eq!(sent.last_accepted_stream_id, 1);

        let goaway_frame = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(goaway_frame.frame_type, FrameType::GoAway);
        session
            .receive_frame(Frame::new(FrameType::EndStream, 1))
            .unwrap();

        let event = session
            .receive_frame(headers_frame(3, StreamEnvelope::request(3, "fs.read")))
            .unwrap();

        assert_eq!(event.routed, RoutedFrame::RejectedStream { stream_id: 3 });
        assert!(event.stream_event.is_none());
        assert!(session.get_stream(3).is_none());

        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.frame_type, FrameType::ResetStream);
        assert_eq!(reset.stream_id, 3);
        let reset_payload = reset.decode_control::<ResetStream>().unwrap();
        assert_eq!(reset_payload.code, RESET_UNAVAILABLE);
        assert!(
            reset_payload
                .diagnostic
                .contains("last accepted stream was 1")
        );
    }

    #[test]
    fn protocol_session_sends_goaway_when_stream_ids_are_exhausted() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        session.set_next_stream_id_for_test(0);

        let error = session
            .open_unary_request("fs.stat", RequestOptions::default())
            .unwrap_err();

        assert_eq!(error.to_string(), STREAM_IDS_EXHAUSTED_MESSAGE);
        let frame = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::GoAway);
        assert_eq!(frame.stream_id, 0);
        let goaway = frame.decode_control::<GoAway>().unwrap();
        assert_eq!(goaway.code, RESET_RESOURCE_EXHAUSTED);
        assert_eq!(goaway.message, STREAM_IDS_EXHAUSTED_MESSAGE);
        assert_eq!(session.sent_goaway(), Some(&goaway));
    }

    #[test]
    fn window_update_frame_encodes_credit_and_rejects_zero_credit() {
        let frame = window_update_frame(5, 4096).unwrap();

        assert_eq!(frame.frame_type, FrameType::WindowUpdate);
        assert_eq!(frame.stream_id, 5);
        assert_eq!(
            frame.decode_control::<WindowUpdate>().unwrap().credit_bytes,
            4096
        );

        let error = window_update_frame(0, 0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("non-zero"));
    }

    #[test]
    fn watch_start_expanded_dirs_sets_design_defaults() {
        let request = WatchStart::expanded_dirs([".", "src"]);

        assert_eq!(request.roots, [".", "src"]);
        assert_eq!(request.mode, WatchMode::ExpandedDirs as i32);
        assert!(!request.recursive);
        assert_eq!(request.debounce_ms, 200);
        assert_eq!(request.max_events_per_batch, 500);
        assert_eq!(request.ignore_policy, WatchIgnorePolicy::Workspace as i32);
        assert!(request.include_hidden);
        assert!(!request.send_initial_snapshot);

        let decoded = WatchStart::decode(request.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.roots, [".", "src"]);
    }

    #[test]
    fn watch_generation_tracker_bumps_generations_before_batch_emit() {
        let mut tracker = WatchGenerationTracker::default();
        let batch = tracker
            .build_batch(
                7,
                ["src", "src", "."],
                vec![
                    WatchChange::created("src/new.rs", false),
                    WatchChange::renamed("src/a.rs", "src/b.rs", false),
                ],
                false,
                false,
            )
            .unwrap();

        assert_eq!(batch.watch_id, 7);
        assert_eq!(batch.sequence, 1);
        assert_eq!(batch.directory_generations.len(), 2);
        assert_eq!(batch.directory_generations[0].path, ".");
        assert_eq!(batch.directory_generations[0].generation, 1);
        assert_eq!(batch.directory_generations[1].path, "src");
        assert_eq!(batch.directory_generations[1].generation, 1);
        assert_eq!(tracker.generation("src"), 1);
        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[0].kind, WatchChangeKind::Created as i32);
        assert_eq!(batch.events[1].kind, WatchChangeKind::Renamed as i32);
        assert_eq!(batch.events[1].old_path, "src/a.rs");

        let next = tracker
            .build_batch(7, ["src"], Vec::new(), true, true)
            .unwrap();
        assert_eq!(next.sequence, 2);
        assert_eq!(next.directory_generations[0].generation, 2);
        assert!(next.overflow);
        assert!(next.resync_required);
    }

    #[test]
    fn watch_batch_frame_encodes_batch_as_event_headers() {
        let mut tracker = WatchGenerationTracker::default();
        let batch = tracker
            .build_batch(
                9,
                ["crates/nucleotide-remote"],
                vec![WatchChange::modified(
                    "crates/nucleotide-remote/src/lib.rs",
                    false,
                )],
                false,
                false,
            )
            .unwrap();

        let frame = watch_batch_frame(2, batch).unwrap();

        assert_eq!(frame.frame_type, FrameType::Headers);
        assert_eq!(frame.stream_id, 2);
        assert_eq!(frame.priority, Priority::VisibleFileTree.as_u8());
        let event = StreamEvent::from_frame(frame).unwrap().unwrap();
        let StreamEvent::Headers {
            role,
            envelope,
            stream_id,
        } = event
        else {
            panic!("expected watch batch headers");
        };
        assert_eq!(stream_id, 2);
        assert_eq!(role, MessageRole::Event);
        assert_eq!(envelope.method, "watch.batch");
        let Some(stream_envelope::Message::Event(event)) = envelope.message else {
            panic!("expected event envelope");
        };
        assert_eq!(event.kind, "watch.batch");
        assert_eq!(event.watch_id, 9);
        let batch = event.watch_batch.expect("watch batch payload");
        assert_eq!(batch.sequence, 1);
        assert_eq!(
            batch.directory_generations[0].path,
            "crates/nucleotide-remote"
        );
        assert_eq!(batch.events[0].kind, WatchChangeKind::Modified as i32);

        let error = watch_batch_frame(0, batch).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn outbound_scheduler_sends_high_priority_before_background() {
        let mut scheduler = OutboundScheduler::new(&ConnectionSettings::recommended());
        let bulk = data_frame(1, 8).with_priority(Priority::Bulk);
        let foreground = Frame::new(FrameType::Ping, 0).with_priority(Priority::UserInput);

        scheduler.enqueue(bulk).unwrap();
        scheduler.enqueue(foreground).unwrap();

        assert_eq!(
            scheduler.pop_next().unwrap().map(|frame| frame.frame_type),
            Some(FrameType::Ping)
        );
        let next = scheduler.pop_next().unwrap().unwrap();
        assert_eq!(next.frame_type, FrameType::Data);
        assert_eq!(next.stream_id, 1);
        assert!(scheduler.is_empty());
    }

    #[test]
    fn outbound_scheduler_skips_blocked_data_and_sends_unblocked_frame() {
        let settings = ConnectionSettings {
            initial_stream_window: 3,
            initial_connection_window: 64,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);
        scheduler
            .enqueue(data_frame(1, 4).with_priority(Priority::UserInput))
            .unwrap();
        scheduler
            .enqueue(Frame::new(FrameType::Ping, 0).with_priority(Priority::Bulk))
            .unwrap();

        let first = scheduler.pop_next().unwrap().unwrap();
        assert_eq!(first.frame_type, FrameType::Ping);
        assert_eq!(scheduler.queued_len(), 1);

        assert!(scheduler.pop_next().unwrap().is_none());
        scheduler.grant_stream(1, 2).unwrap();
        let second = scheduler.pop_next().unwrap().unwrap();
        assert_eq!(second.frame_type, FrameType::Data);
        assert_eq!(second.stream_id, 1);
    }

    #[test]
    fn outbound_scheduler_blocks_data_on_connection_window_until_granted() {
        let settings = ConnectionSettings {
            initial_stream_window: 64,
            initial_connection_window: 3,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);
        scheduler
            .enqueue(data_frame(1, 4).with_priority(Priority::UserInput))
            .unwrap();

        assert!(scheduler.pop_next().unwrap().is_none());
        scheduler.grant_connection(2).unwrap();

        let frame = scheduler.pop_next().unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::Data);
        assert_eq!(scheduler.connection_window(), 1);
        assert_eq!(scheduler.stream_window(1), 60);
    }

    #[test]
    fn outbound_scheduler_consumes_connection_and_stream_credit_for_data_only() {
        let settings = ConnectionSettings {
            initial_stream_window: 10,
            initial_connection_window: 10,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);

        scheduler
            .enqueue(Frame::new(FrameType::Ping, 0).with_priority(Priority::UserInput))
            .unwrap();
        scheduler
            .enqueue(data_frame(1, 4).with_priority(Priority::UserInput))
            .unwrap();

        assert_eq!(
            scheduler.pop_next().unwrap().map(|frame| frame.frame_type),
            Some(FrameType::Ping)
        );
        assert_eq!(scheduler.connection_window(), 10);
        assert_eq!(scheduler.stream_window(1), 10);

        assert_eq!(
            scheduler.pop_next().unwrap().map(|frame| frame.frame_type),
            Some(FrameType::Data)
        );
        assert_eq!(scheduler.connection_window(), 6);
        assert_eq!(scheduler.stream_window(1), 6);
    }

    #[test]
    fn outbound_scheduler_preserves_fifo_within_priority() {
        let mut scheduler = OutboundScheduler::new(&ConnectionSettings::recommended());
        scheduler
            .enqueue(Frame::new(FrameType::Headers, 1).with_priority(Priority::ForegroundDocument))
            .unwrap();
        scheduler
            .enqueue(
                Frame::new(FrameType::ResetStream, 3).with_priority(Priority::ForegroundDocument),
            )
            .unwrap();

        assert_eq!(scheduler.pop_next().unwrap().unwrap().stream_id, 1);
        assert_eq!(scheduler.pop_next().unwrap().unwrap().stream_id, 3);
    }

    #[test]
    fn outbound_scheduler_rejects_data_on_stream_zero() {
        let mut scheduler = OutboundScheduler::new(&ConnectionSettings::recommended());
        let frame = data_frame(0, 1);

        let error = scheduler.enqueue(frame).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("non-zero stream id"));
    }

    #[test]
    fn outbound_scheduler_preserves_same_stream_order_when_data_is_blocked() {
        let settings = ConnectionSettings {
            initial_stream_window: 3,
            initial_connection_window: 64,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);
        scheduler
            .enqueue(data_frame(1, 4).with_priority(Priority::UserInput))
            .unwrap();
        scheduler
            .enqueue(
                end_stream_frame(1)
                    .unwrap()
                    .with_priority(Priority::UserInput),
            )
            .unwrap();
        scheduler
            .enqueue(Frame::new(FrameType::Ping, 0).with_priority(Priority::Bulk))
            .unwrap();

        let first = scheduler.pop_next().unwrap().unwrap();
        assert_eq!(first.frame_type, FrameType::Ping);
        assert_eq!(scheduler.queued_len(), 2);

        assert!(scheduler.pop_next().unwrap().is_none());
        scheduler.grant_stream(1, 1).unwrap();

        let data = scheduler.pop_next().unwrap().unwrap();
        assert_eq!(data.frame_type, FrameType::Data);
        let end = scheduler.pop_next().unwrap().unwrap();
        assert_eq!(end.frame_type, FrameType::EndStream);
    }

    #[test]
    fn outbound_scheduler_enforces_connection_control_budget_and_releases_on_send() {
        let settings = ConnectionSettings {
            connection_control_budget: FRAME_HEADER_LEN as u32 + 3,
            stream_control_budget: 1024,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);
        let mut first = Frame::new(FrameType::Ping, 0);
        first.control = b"abc".to_vec();
        let second = Frame::new(FrameType::Pong, 0);

        scheduler.enqueue(first).unwrap();
        assert_eq!(
            scheduler.connection_control_used(),
            FRAME_HEADER_LEN as u64 + 3
        );

        let error = scheduler.enqueue(second.clone()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::OutOfMemory);
        assert!(error.to_string().contains("RESOURCE_EXHAUSTED"));

        assert_eq!(
            scheduler.pop_next().unwrap().map(|frame| frame.frame_type),
            Some(FrameType::Ping)
        );
        assert_eq!(scheduler.connection_control_used(), 0);

        scheduler.enqueue(second).unwrap();
        assert_eq!(
            scheduler.pop_next().unwrap().map(|frame| frame.frame_type),
            Some(FrameType::Pong)
        );
    }

    #[test]
    fn outbound_scheduler_enforces_per_stream_control_budget() {
        let settings = ConnectionSettings {
            connection_control_budget: 1024,
            stream_control_budget: FRAME_HEADER_LEN as u32 + 4,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);
        let mut first = Frame::new(FrameType::Headers, 1);
        first.control = b"abcd".to_vec();
        let same_stream = Frame::new(FrameType::EndStream, 1);
        let other_stream = Frame::new(FrameType::EndStream, 3);

        scheduler.enqueue(first).unwrap();
        assert_eq!(
            scheduler.stream_control_used(1),
            FRAME_HEADER_LEN as u64 + 4
        );

        let error = scheduler.enqueue(same_stream).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::OutOfMemory);
        assert!(error.to_string().contains("stream 1"));

        scheduler.enqueue(other_stream).unwrap();
        assert_eq!(scheduler.stream_control_used(3), FRAME_HEADER_LEN as u64);
    }

    #[test]
    fn outbound_scheduler_does_not_charge_data_against_control_budget() {
        let settings = ConnectionSettings {
            connection_control_budget: 1,
            stream_control_budget: 1,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);

        scheduler.enqueue(data_frame(1, 128)).unwrap();

        assert_eq!(scheduler.connection_control_used(), 0);
        assert_eq!(scheduler.stream_control_used(1), 0);
        assert_eq!(
            scheduler.pop_next().unwrap().map(|frame| frame.frame_type),
            Some(FrameType::Data)
        );
    }

    #[test]
    fn stream_event_decodes_headers_data_end_and_reset_frames() {
        let headers = headers_frame(
            1,
            StreamEnvelope::response(1, "fs.read", MessageRole::PartialResult, false),
        );
        let header_event = StreamEvent::from_frame(headers).unwrap().unwrap();
        assert!(matches!(
            header_event,
            StreamEvent::Headers {
                stream_id: 1,
                role: MessageRole::PartialResult,
                ..
            }
        ));

        let data_event = StreamEvent::from_frame(data_frame(1, 5)).unwrap().unwrap();
        assert!(matches!(
            data_event,
            StreamEvent::Data {
                stream_id: 1,
                channel: DataChannel::FileBody,
                uncompressed_len: 5,
                ref body
            } if body.len() == 5
        ));

        assert_eq!(
            StreamEvent::from_frame(Frame::new(FrameType::EndStream, 1))
                .unwrap()
                .unwrap(),
            StreamEvent::EndStream { stream_id: 1 }
        );

        assert_eq!(
            StreamEvent::from_frame(reset_stream_frame(1, "cancelled", "new query"))
                .unwrap()
                .unwrap(),
            StreamEvent::ResetStream {
                stream_id: 1,
                code: "cancelled".to_string(),
                diagnostic: "new query".to_string()
            }
        );

        assert!(
            StreamEvent::from_frame(Frame::new(FrameType::Ping, 0))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn protocol_session_opens_body_request_and_cleans_up_after_final_response() {
        let settings = ConnectionSettings::recommended();
        let mut session = ProtocolSession::with_limits(
            StreamInitiator::Client,
            &settings,
            FrameLimits {
                max_control_len: DEFAULT_MAX_CONTROL_LEN,
                max_body_len: 4,
            },
        );

        let stream_id = session
            .open_request_with_body(
                "fs.write",
                RequestOptions {
                    priority: Priority::ForegroundDocument,
                    idempotency: Idempotency::Mutation,
                    ..RequestOptions::default()
                },
                DataChannel::FileBody,
                b"abcdef",
            )
            .unwrap();

        assert_eq!(stream_id, 1);
        assert_eq!(session.in_flight_len(), 1);
        assert_eq!(session.active_streams(), 1);
        assert_eq!(session.queued_len(), 4);

        let headers = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(headers.frame_type, FrameType::Headers);
        assert_eq!(headers.priority, Priority::ForegroundDocument.as_u8());
        let envelope = headers.decode_control::<StreamEnvelope>().unwrap();
        assert_eq!(
            envelope.request_idempotency().unwrap(),
            Idempotency::Mutation
        );

        let first = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(first.frame_type, FrameType::Data);
        assert_eq!(first.body, b"abcd");
        let second = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(second.frame_type, FrameType::Data);
        assert_eq!(second.body, b"ef");
        assert_eq!(
            session
                .pop_next_frame()
                .unwrap()
                .map(|frame| frame.frame_type),
            Some(FrameType::EndStream)
        );

        let final_headers = headers_frame(
            stream_id,
            StreamEnvelope::response(stream_id, "fs.write", MessageRole::FinalResponse, true),
        );
        let event = session.receive_frame(final_headers).unwrap();
        assert!(matches!(
            event.stream_event,
            Some(StreamEvent::Headers {
                role: MessageRole::FinalResponse,
                ..
            })
        ));
        assert!(session.in_flight.get(stream_id).unwrap().final_seen);

        session
            .receive_frame(Frame::new(FrameType::EndStream, stream_id))
            .unwrap();
        assert_eq!(session.in_flight_len(), 0);
        assert_eq!(session.active_streams(), 0);
    }

    #[test]
    fn protocol_session_window_update_unblocks_queued_data_without_reordering_end_stream() {
        let settings = ConnectionSettings {
            initial_stream_window: 3,
            initial_connection_window: 64,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::with_limits(
            StreamInitiator::Client,
            &settings,
            FrameLimits::default(),
        );
        let stream_id = session
            .open_request_with_body(
                "fs.write",
                RequestOptions {
                    priority: Priority::UserInput,
                    ..RequestOptions::default()
                },
                DataChannel::FileBody,
                b"abcd",
            )
            .unwrap();

        assert_eq!(
            session
                .pop_next_frame()
                .unwrap()
                .map(|frame| frame.frame_type),
            Some(FrameType::Headers)
        );
        assert!(session.pop_next_frame().unwrap().is_none());

        let event = session
            .receive_frame(window_update_frame(stream_id, 1).unwrap())
            .unwrap();
        assert_eq!(
            event.routed,
            RoutedFrame::WindowUpdate {
                stream_id,
                credit_bytes: 1
            }
        );

        assert_eq!(
            session
                .pop_next_frame()
                .unwrap()
                .map(|frame| frame.frame_type),
            Some(FrameType::Data)
        );
        assert_eq!(
            session
                .pop_next_frame()
                .unwrap()
                .map(|frame| frame.frame_type),
            Some(FrameType::EndStream)
        );
    }

    #[test]
    fn protocol_session_incoming_superseded_request_queues_reset_for_old_stream() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());

        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "search.text")))
            .unwrap();
        let superseding = StreamEnvelope::request_with_options(
            3,
            "search.text",
            &RequestOptions {
                supersedes_stream_id: 1,
                ..RequestOptions::default()
            },
        );
        session
            .receive_frame(headers_frame(3, superseding))
            .unwrap();

        assert!(!session.is_in_flight(1));
        assert!(session.is_in_flight(3));
        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.frame_type, FrameType::ResetStream);
        assert_eq!(reset.stream_id, 1);
        assert_eq!(
            reset.decode_control::<ResetStream>().unwrap().code,
            RESET_CANCELLED
        );
    }

    #[test]
    fn protocol_session_expire_deadlines_removes_streams_and_queues_resets() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        let request = StreamEnvelope::request_with_options(
            1,
            "fs.read",
            &RequestOptions {
                deadline_unix_ms: 100,
                ..RequestOptions::default()
            },
        );
        session.receive_frame(headers_frame(1, request)).unwrap();

        assert_eq!(session.expire_deadlines(99).unwrap(), 0);
        assert!(session.is_in_flight(1));

        assert_eq!(session.expire_deadlines(100).unwrap(), 1);
        assert!(!session.is_in_flight(1));
        assert_eq!(session.active_streams(), 0);
        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.stream_id, 1);
        assert_eq!(
            reset.decode_control::<ResetStream>().unwrap().code,
            RESET_DEADLINE_EXCEEDED
        );
    }

    #[test]
    fn protocol_session_server_emits_progress_data_final_response_and_end_stream() {
        let settings = ConnectionSettings::recommended();
        let mut server = ProtocolSession::with_limits(
            StreamInitiator::Server,
            &settings,
            FrameLimits {
                max_control_len: DEFAULT_MAX_CONTROL_LEN,
                max_body_len: 4,
            },
        );
        server
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();
        server
            .receive_frame(Frame::new(FrameType::EndStream, 1))
            .unwrap();

        server
            .send_progress(
                1,
                "fs.read",
                Progress {
                    message: "reading".to_string(),
                    completed: 1,
                    total: 2,
                },
            )
            .unwrap();
        server
            .send_data(
                1,
                DataChannel::FileBody,
                b"abcdef",
                Priority::ForegroundDocument,
            )
            .unwrap();
        server
            .send_response(1, "fs.read", MessageRole::FinalResponse, true)
            .unwrap();
        assert_eq!(
            server
                .finish_stream(1, Priority::ForegroundDocument)
                .unwrap(),
            StreamState::Closed
        );
        assert_eq!(server.active_streams(), 0);

        let progress = server.pop_next_frame().unwrap().unwrap();
        assert_eq!(progress.frame_type, FrameType::Headers);
        let progress = progress.decode_control::<StreamEnvelope>().unwrap();
        assert_eq!(progress.message_role().unwrap(), MessageRole::Progress);
        assert!(matches!(
            progress.message,
            Some(stream_envelope::Message::Progress(Progress {
                completed: 1,
                total: 2,
                ..
            }))
        ));

        let first = server.pop_next_frame().unwrap().unwrap();
        assert_eq!(first.frame_type, FrameType::Data);
        assert_eq!(first.body, b"abcd");
        let second = server.pop_next_frame().unwrap().unwrap();
        assert_eq!(second.frame_type, FrameType::Data);
        assert_eq!(second.body, b"ef");

        let final_response = server.pop_next_frame().unwrap().unwrap();
        assert_eq!(final_response.frame_type, FrameType::Headers);
        let final_response = final_response.decode_control::<StreamEnvelope>().unwrap();
        assert_eq!(
            final_response.message_role().unwrap(),
            MessageRole::FinalResponse
        );

        let end = server.pop_next_frame().unwrap().unwrap();
        assert_eq!(end.frame_type, FrameType::EndStream);
    }

    #[test]
    fn protocol_session_client_observes_server_final_response_and_stream_end() {
        let mut client =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let stream_id = client
            .open_unary_request("fs.stat", RequestOptions::default())
            .unwrap();
        while client.pop_next_frame().unwrap().is_some() {}

        let final_headers = headers_frame(
            stream_id,
            StreamEnvelope::response(stream_id, "fs.stat", MessageRole::FinalResponse, true),
        );
        let event = client.receive_frame(final_headers).unwrap();

        assert!(matches!(
            event.stream_event,
            Some(StreamEvent::Headers {
                role: MessageRole::FinalResponse,
                ..
            })
        ));
        assert!(client.in_flight.get(stream_id).unwrap().final_seen);

        client
            .receive_frame(Frame::new(FrameType::EndStream, stream_id))
            .unwrap();
        assert_eq!(client.in_flight_len(), 0);
        assert_eq!(client.active_streams(), 0);
    }

    #[test]
    fn protocol_session_send_error_uses_final_error_headers() {
        let mut server =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        server
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();

        server
            .send_error(
                1,
                "fs.read",
                ErrorHeader {
                    code: "NOT_FOUND".to_string(),
                    message: "missing".to_string(),
                    retryable: false,
                    details: String::new(),
                    remote_errno: 2,
                },
            )
            .unwrap();

        let frame = server.pop_next_frame().unwrap().unwrap();
        let envelope = frame.decode_control::<StreamEnvelope>().unwrap();
        assert_eq!(envelope.message_role().unwrap(), MessageRole::FinalError);
        assert!(matches!(
            envelope.message,
            Some(stream_envelope::Message::Error(ErrorHeader {
                ref code,
                remote_errno: 2,
                ..
            })) if code == "NOT_FOUND"
        ));
    }

    #[test]
    fn protocol_session_rejects_invalid_response_role() {
        let mut server =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        server
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();

        let error = server
            .send_response(1, "fs.read", MessageRole::Request, false)
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("final_response"));
    }

    #[test]
    fn in_flight_requests_track_final_and_remove_on_end() {
        let mut in_flight = InFlightRequests::default();
        let request = StreamEnvelope::request_with_options(
            1,
            "search.text",
            &RequestOptions {
                cancellation_group: "search:rust".to_string(),
                ..RequestOptions::default()
            },
        );
        in_flight.register_from_envelope(1, &request).unwrap();

        assert_eq!(in_flight.len(), 1);
        assert_eq!(
            in_flight
                .get(1)
                .map(|request| request.cancellation_group.as_str()),
            Some("search:rust")
        );
        assert_eq!(
            in_flight.get(1).map(|request| request.idempotency),
            Some(Idempotency::ReadOnly)
        );

        let final_event = StreamEvent::Headers {
            stream_id: 1,
            role: MessageRole::FinalResponse,
            envelope: StreamEnvelope::response(1, "search.text", MessageRole::FinalResponse, true),
        };
        in_flight.observe_event(&final_event).unwrap();
        assert!(in_flight.get(1).unwrap().final_seen);

        in_flight
            .observe_event(&StreamEvent::EndStream { stream_id: 1 })
            .unwrap();
        assert!(in_flight.is_empty());
    }

    #[test]
    fn request_metadata_extracts_deadline_supersession_and_idempotency() {
        let envelope = StreamEnvelope::request_with_options(
            9,
            "fs.write",
            &RequestOptions {
                cancellation_group: "write:src/lib.rs".to_string(),
                deadline_unix_ms: 123_456,
                supersedes_stream_id: 7,
                idempotency: Idempotency::Mutation,
                ..RequestOptions::default()
            },
        );

        let metadata = envelope.request_metadata().unwrap();

        assert_eq!(metadata.method, "fs.write");
        assert_eq!(metadata.cancellation_group, "write:src/lib.rs");
        assert_eq!(metadata.deadline_unix_ms, 123_456);
        assert_eq!(metadata.supersedes_stream_id, 7);
        assert_eq!(metadata.idempotency, Idempotency::Mutation);
    }

    #[test]
    fn request_metadata_rejects_missing_or_unknown_request_header() {
        let mut envelope = StreamEnvelope::request(1, "fs.read");
        envelope.message = None;

        let error = envelope.request_metadata().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("missing RequestHeader"));

        envelope.message = Some(stream_envelope::Message::Request(RequestHeader {
            idempotency: 99,
        }));
        let error = envelope.request_metadata().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unknown v5 request idempotency"));
    }

    #[test]
    fn in_flight_cancel_superseded_by_registered_request() {
        let mut in_flight = InFlightRequests::default();
        in_flight.register(1, "search.text", "search:old").unwrap();
        let request = StreamEnvelope::request_with_options(
            3,
            "search.text",
            &RequestOptions {
                cancellation_group: "search:new".to_string(),
                supersedes_stream_id: 1,
                ..RequestOptions::default()
            },
        );
        in_flight.register_from_envelope(3, &request).unwrap();

        let reset = in_flight
            .cancel_superseded_by(3)
            .unwrap()
            .expect("old stream should be cancelled");

        assert_eq!(reset.stream_id, 1);
        let payload = reset.decode_control::<ResetStream>().unwrap();
        assert_eq!(payload.code, RESET_CANCELLED);
        assert!(payload.diagnostic.contains("stream 3"));
        assert!(!in_flight.contains(1));
        assert!(in_flight.contains(3));
    }

    #[test]
    fn in_flight_deadline_expiry_resets_expired_streams_in_order() {
        let mut in_flight = InFlightRequests::default();
        in_flight
            .register_with_metadata(
                5,
                RequestMetadata::new("fs.read").with_deadline_unix_ms(200),
            )
            .unwrap();
        in_flight
            .register_with_metadata(
                1,
                RequestMetadata::new("search.text").with_deadline_unix_ms(100),
            )
            .unwrap();
        in_flight
            .register_with_metadata(3, RequestMetadata::new("fs.stat").with_deadline_unix_ms(0))
            .unwrap();

        let frames = in_flight.expire_deadlines(150);

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].stream_id, 1);
        assert_eq!(
            frames[0].decode_control::<ResetStream>().unwrap().code,
            RESET_DEADLINE_EXCEEDED
        );
        assert!(!in_flight.contains(1));
        assert!(in_flight.contains(3));
        assert!(in_flight.contains(5));

        let frames = in_flight.expire_deadlines(250);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].stream_id, 5);
    }

    #[test]
    fn in_flight_cancel_group_returns_reset_frames_and_removes_members() {
        let mut in_flight = InFlightRequests::default();
        in_flight.register(3, "search.text", "search:rust").unwrap();
        in_flight
            .register(1, "search.files", "search:rust")
            .unwrap();
        in_flight.register(5, "fs.stat", "metadata").unwrap();

        let frames = in_flight.cancel_group("search:rust", "cancelled", "query changed");

        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].stream_id, 1);
        assert_eq!(frames[1].stream_id, 3);
        assert!(!in_flight.contains(1));
        assert!(!in_flight.contains(3));
        assert!(in_flight.contains(5));
        let reset = frames[0].decode_control::<ResetStream>().unwrap();
        assert_eq!(reset.code, "cancelled");
        assert_eq!(reset.diagnostic, "query changed");
    }

    #[test]
    fn in_flight_cancel_stream_removes_known_request_only() {
        let mut in_flight = InFlightRequests::default();
        in_flight.register(1, "fs.read", "").unwrap();

        let frame = in_flight
            .cancel_stream(1, "cancelled", "closed")
            .expect("known stream should produce reset");
        assert_eq!(frame.stream_id, 1);
        assert!(!in_flight.contains(1));
        assert!(in_flight.cancel_stream(1, "cancelled", "closed").is_none());
    }

    #[test]
    fn stream_id_allocator_uses_odd_client_and_even_server_ids() {
        let mut client = StreamIdAllocator::new(StreamInitiator::Client);
        let mut server = StreamIdAllocator::new(StreamInitiator::Server);

        assert_eq!(client.peek(), Some(1));
        assert_eq!(client.next_id(), Some(1));
        assert_eq!(client.next_id(), Some(3));
        assert_eq!(server.next_id(), Some(2));
        assert_eq!(server.next_id(), Some(4));
    }

    #[test]
    fn stream_id_allocator_stops_at_wraparound() {
        let mut allocator = StreamIdAllocator::with_next_for_test(u64::MAX - 1);

        assert_eq!(allocator.next_id(), Some(u64::MAX - 1));
        assert_eq!(allocator.peek(), None);
        assert_eq!(allocator.next_id(), None);
    }

    #[test]
    fn frame_writer_rejects_oversized_control() {
        let mut frame = Frame::new(FrameType::Headers, 1);
        frame.control = vec![0; 4];
        let limits = FrameLimits {
            max_control_len: 3,
            max_body_len: 64,
        };

        let error = write_frame_with_limits(&mut Vec::new(), &frame, limits).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("control exceeds maximum"));
    }

    #[test]
    fn frame_reader_rejects_oversized_body_before_allocating() {
        let limits = FrameLimits {
            max_control_len: 64,
            max_body_len: 3,
        };
        let mut fixed = [0_u8; FRAME_HEADER_LEN];
        fixed[0..4].copy_from_slice(&FRAME_MAGIC);
        fixed[4..6].copy_from_slice(&FRAME_HEADER_VERSION.to_be_bytes());
        fixed[6..8].copy_from_slice(&(FrameType::Data as u16).to_be_bytes());
        fixed[32..36].copy_from_slice(&4_u32.to_be_bytes());

        let error = read_frame_with_limits(&mut Cursor::new(fixed), limits).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("body exceeds maximum"));
    }

    #[test]
    fn settings_negotiation_accepts_lower_limits_and_clamps_oversized_body() {
        let desired = ConnectionSettings {
            max_concurrent_streams: 8,
            initial_stream_window: 32 * 1024,
            initial_connection_window: 64 * 1024,
            max_frame_body: 2 * 1024 * 1024,
            max_control_len: 256 * 1024,
            connection_control_budget: 64 * 1024,
            stream_control_budget: 32 * 1024,
            shutdown_grace_ms: 10_000,
            idle_ping_interval_ms: 15_000,
            ping_timeout_ms: 45_000,
            min_unsolicited_ping_interval_ms: 100,
        };

        let accepted = ConnectionSettings::accept_peer_desired(Some(&desired));

        assert_eq!(accepted.max_concurrent_streams, 8);
        assert_eq!(accepted.initial_stream_window, 32 * 1024);
        assert_eq!(accepted.initial_connection_window, 64 * 1024);
        assert_eq!(accepted.max_frame_body, MAX_NEGOTIATED_FRAME_BODY_LEN);
        assert_eq!(accepted.max_control_len, DEFAULT_MAX_CONTROL_LEN);
        assert_eq!(accepted.connection_control_budget, 64 * 1024);
        assert_eq!(accepted.stream_control_budget, 32 * 1024);
        assert_eq!(accepted.shutdown_grace_ms, 10_000);
        assert_eq!(accepted.idle_ping_interval_ms, 15_000);
        assert_eq!(accepted.ping_timeout_ms, 45_000);
        assert_eq!(
            accepted.min_unsolicited_ping_interval_ms,
            MIN_UNSOLICITED_PING_INTERVAL_MS
        );
        assert_eq!(
            FrameLimits::from_settings(&accepted).max_body_len,
            MAX_NEGOTIATED_FRAME_BODY_LEN
        );
    }

    #[test]
    fn server_hello_accepts_client_and_intersects_capabilities() {
        let mut client = ClientHello::nucleotide("0.1.0");
        client.capabilities = vec!["multiplex".to_string(), "watch".to_string()];
        client.required_capabilities = vec!["directory_not_modified".to_string()];
        let info = ServerHandshakeInfo {
            helper_version: "helper".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            workspace_root: "/workspace".to_string(),
            capabilities: vec![
                "multiplex".to_string(),
                "watch".to_string(),
                "directory_not_modified".to_string(),
                "server_only".to_string(),
            ],
        };

        let hello = ServerHello::accept_client(&client, &info).unwrap();

        assert_eq!(hello.protocol_major, PROTOCOL_MAJOR);
        assert_eq!(hello.protocol_minor, PROTOCOL_MINOR);
        assert_eq!(hello.control_codec, "protobuf");
        assert_eq!(
            hello.capabilities,
            ["multiplex", "watch", "directory_not_modified"]
        );
        assert!(hello.accepted_settings.is_some());
    }

    #[test]
    fn server_handshake_reads_client_hello_and_writes_server_frames() {
        let client = ClientHello::nucleotide("0.1.0");
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &Frame::from_control(FrameType::Hello, 0, &client),
        )
        .unwrap();
        write_frame(&mut input, &Frame::new(FrameType::SettingsAck, 0)).unwrap();
        let info = ServerHandshakeInfo {
            helper_version: "helper".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            workspace_root: "/workspace".to_string(),
            capabilities: vec!["multiplex".to_string(), "watch".to_string()],
        };
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let handshake = server_handshake(&mut io, &info).unwrap();
        let limits = io.limits();
        let (_, output) = io.into_inner();

        assert_eq!(handshake.client_hello.client_name, "nucleotide");
        assert_eq!(handshake.server_hello.capabilities, ["multiplex", "watch"]);
        assert_eq!(
            limits.max_body_len,
            handshake
                .settings
                .max_frame_body
                .min(MAX_NEGOTIATED_FRAME_BODY_LEN)
        );

        let mut output = Cursor::new(output);
        let server_hello_frame = read_frame(&mut output).unwrap().unwrap();
        let settings_frame = read_frame(&mut output).unwrap().unwrap();

        assert_eq!(server_hello_frame.frame_type, FrameType::Hello);
        assert_eq!(server_hello_frame.frame_sequence, 1);
        assert_eq!(settings_frame.frame_type, FrameType::Settings);
        assert_eq!(settings_frame.frame_sequence, 2);
        assert_eq!(
            server_hello_frame
                .decode_control::<ServerHello>()
                .unwrap()
                .workspace_root,
            "/workspace"
        );
        assert_eq!(
            settings_frame
                .decode_control::<ConnectionSettings>()
                .unwrap()
                .max_concurrent_streams,
            DEFAULT_MAX_CONCURRENT_STREAMS
        );
    }

    #[test]
    fn client_handshake_writes_hello_ack_and_applies_settings() {
        let client = ClientHello::nucleotide("0.1.0");
        let settings = ConnectionSettings {
            max_concurrent_streams: 4,
            initial_stream_window: 16 * 1024,
            initial_connection_window: 64 * 1024,
            max_frame_body: 128 * 1024,
            max_control_len: 8 * 1024,
            connection_control_budget: 32 * 1024,
            stream_control_budget: 16 * 1024,
            shutdown_grace_ms: DEFAULT_SHUTDOWN_GRACE_MS,
            idle_ping_interval_ms: IDLE_PING_INTERVAL_MS,
            ping_timeout_ms: PING_TIMEOUT_MS,
            min_unsolicited_ping_interval_ms: MIN_UNSOLICITED_PING_INTERVAL_MS,
        };
        let server_hello = ServerHello {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            helper_version: "helper".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            workspace_root: "/workspace".to_string(),
            control_codec: "protobuf".to_string(),
            capabilities: vec!["multiplex".to_string()],
            accepted_settings: Some(settings.clone()),
        };
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &Frame::from_control(FrameType::Hello, 0, &server_hello),
        )
        .unwrap();
        write_frame(
            &mut input,
            &Frame::from_control(FrameType::Settings, 0, &settings),
        )
        .unwrap();
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let handshake = client_handshake(&mut io, client).unwrap();
        let limits = io.limits();
        let (_, output) = io.into_inner();

        assert_eq!(handshake.server_hello.workspace_root, "/workspace");
        assert_eq!(handshake.settings.max_concurrent_streams, 4);
        assert_eq!(limits.max_body_len, 128 * 1024);

        let mut output = Cursor::new(output);
        let client_hello_frame = read_frame(&mut output).unwrap().unwrap();
        let ack_frame = read_frame(&mut output).unwrap().unwrap();

        assert_eq!(client_hello_frame.frame_type, FrameType::Hello);
        assert_eq!(client_hello_frame.frame_sequence, 1);
        assert_eq!(ack_frame.frame_type, FrameType::SettingsAck);
        assert_eq!(ack_frame.frame_sequence, 2);
        assert!(ack_frame.control.is_empty());
        assert!(ack_frame.body.is_empty());
    }

    #[test]
    fn server_handshake_rejects_client_without_protobuf_codec() {
        let mut client = ClientHello::nucleotide("0.1.0");
        client.control_codecs = vec!["json".to_string()];
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &Frame::from_control(FrameType::Hello, 0, &client),
        )
        .unwrap();
        let info = ServerHandshakeInfo::current("/workspace");
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let error = match server_handshake(&mut io, &info) {
            Ok(_) => panic!("expected server handshake to reject missing protobuf codec"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("protobuf"));
    }

    #[test]
    fn server_handshake_rejects_unavailable_required_capability() {
        let mut client = ClientHello::nucleotide("0.1.0");
        client.required_capabilities = vec!["watch".to_string(), "future_capability".to_string()];
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &Frame::from_control(FrameType::Hello, 0, &client),
        )
        .unwrap();
        let mut info = ServerHandshakeInfo::current("/workspace");
        info.capabilities = vec!["watch".to_string()];
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let error = match server_handshake(&mut io, &info) {
            Ok(_) => panic!("expected server handshake to reject required capability"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("UNSUPPORTED_CAPABILITY"));
        assert!(error.to_string().contains("future_capability"));
    }

    #[test]
    fn client_handshake_rejects_server_without_protobuf_codec() {
        let client = ClientHello::nucleotide("0.1.0");
        let server_hello = ServerHello {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            helper_version: "helper".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            workspace_root: "/workspace".to_string(),
            control_codec: "json".to_string(),
            capabilities: Vec::new(),
            accepted_settings: Some(ConnectionSettings::recommended()),
        };
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &Frame::from_control(FrameType::Hello, 0, &server_hello),
        )
        .unwrap();
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let error = match client_handshake(&mut io, client) {
            Ok(_) => panic!("expected client handshake to reject server codec"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("control codec"));
    }

    #[test]
    fn client_handshake_rejects_server_missing_required_capability() {
        let mut client = ClientHello::nucleotide("0.1.0");
        client.required_capabilities = vec!["watch".to_string()];
        let settings = ConnectionSettings::recommended();
        let server_hello = ServerHello {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            helper_version: "helper".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            workspace_root: "/workspace".to_string(),
            control_codec: "protobuf".to_string(),
            capabilities: vec!["multiplex".to_string()],
            accepted_settings: Some(settings.clone()),
        };
        let mut input = Vec::new();
        write_frame(
            &mut input,
            &Frame::from_control(FrameType::Hello, 0, &server_hello),
        )
        .unwrap();
        write_frame(
            &mut input,
            &Frame::from_control(FrameType::Settings, 0, &settings),
        )
        .unwrap();
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let error = match client_handshake(&mut io, client) {
            Ok(_) => panic!("expected client handshake to reject missing required capability"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("UNSUPPORTED_CAPABILITY"));
        assert!(error.to_string().contains("watch"));
    }

    #[test]
    fn client_hello_round_trips_as_protobuf_control() {
        let hello = ClientHello::nucleotide("0.1.0");
        let frame = Frame::from_control(FrameType::Hello, 0, &hello);
        let decoded = frame.decode_control::<ClientHello>().unwrap();

        assert_eq!(decoded.protocol_major, PROTOCOL_MAJOR);
        assert_eq!(decoded.protocol_minor, PROTOCOL_MINOR);
        assert_eq!(decoded.client_name, "nucleotide");
        assert_eq!(decoded.control_codecs, ["protobuf"]);
        assert!(
            decoded
                .capabilities
                .contains(&"directory_not_modified".to_string())
        );
        assert_eq!(
            decoded.desired_settings.unwrap().max_concurrent_streams,
            DEFAULT_MAX_CONCURRENT_STREAMS
        );
    }

    #[test]
    fn stream_envelope_distinguishes_partial_and_final_messages() {
        let envelope = StreamEnvelope {
            request_id: 9,
            method: "search.text".to_string(),
            correlation_id: 44,
            deadline_unix_ms: 123,
            priority: Priority::Background as i32,
            role: MessageRole::PartialResult as i32,
            cancellation_group: "search:main".to_string(),
            supersedes_stream_id: 7,
            content_encoding: ContentEncoding::Zstd as i32,
            message: Some(stream_envelope::Message::Response(ResponseHeader {
                complete: false,
                generation: 0,
            })),
        };
        let frame = Frame::from_control(FrameType::Headers, 9, &envelope);

        let decoded = frame.decode_control::<StreamEnvelope>().unwrap();

        assert_eq!(decoded.request_id, 9);
        assert_eq!(decoded.method, "search.text");
        assert_eq!(decoded.role, MessageRole::PartialResult as i32);
        assert_eq!(decoded.content_encoding, ContentEncoding::Zstd as i32);
        assert_eq!(decoded.supersedes_stream_id, 7);
        assert!(matches!(
            decoded.message,
            Some(stream_envelope::Message::Response(ResponseHeader {
                complete: false,
                ..
            }))
        ));
    }

    #[test]
    fn process_data_envelope_identifies_channels() {
        let stdout = DataEnvelope {
            channel: DataChannel::Stdout as i32,
            uncompressed_len: 128,
        };
        let frame = Frame::from_control(FrameType::Data, 11, &stdout);

        let decoded = frame.decode_control::<DataEnvelope>().unwrap();

        assert_eq!(decoded.channel, DataChannel::Stdout as i32);
        assert_eq!(decoded.uncompressed_len, 128);
    }

    #[test]
    fn goaway_carries_shutdown_grace() {
        let goaway = GoAway {
            last_accepted_stream_id: 99,
            code: "shutdown".to_string(),
            message: "helper shutting down".to_string(),
            drain_grace_ms: DEFAULT_SHUTDOWN_GRACE_MS,
        };
        let frame = Frame::from_control(FrameType::GoAway, 0, &goaway);

        let decoded = frame.decode_control::<GoAway>().unwrap();

        assert_eq!(decoded.last_accepted_stream_id, 99);
        assert_eq!(decoded.drain_grace_ms, DEFAULT_SHUTDOWN_GRACE_MS);
    }
}
