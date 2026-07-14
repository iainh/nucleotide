// ABOUTME: Version 5 remote protocol frame and control-message primitives
// ABOUTME: Provides multiplexed transport, flow control, and typed stream metadata

use prost::Message;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
const PRIORITY_SERVICE_WEIGHTS: [usize; PRIORITY_LEVELS] = [8, 6, 4, 3, 2, 1];
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
        if self.frame_type != FrameType::Data {
            return 0;
        }
        if self.control.is_empty() {
            return self.body.len() as u64;
        }
        DataEnvelope::decode(self.control.as_slice())
            .ok()
            .map(|envelope| envelope.uncompressed_len)
            .filter(|length| *length > 0)
            .unwrap_or(self.body.len() as u64)
    }

    pub fn control_budget_len(&self) -> u64 {
        if self.frame_type.consumes_flow_window()
            || matches!(
                self.frame_type,
                FrameType::ResetStream
                    | FrameType::WindowUpdate
                    | FrameType::Ping
                    | FrameType::Pong
            )
        {
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
        let available = self.available.checked_add(bytes).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "v5 flow-control window overflow",
            )
        })?;
        if available > MAX_FLOW_WINDOW {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "v5 flow-control window exceeds maximum",
            ));
        }
        self.available = available;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamTombstone {
    Closed,
    Reset,
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
    pub priority: Priority,
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
            options.priority,
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
            Priority::VisibleFileTree,
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
                if update.credit_bytes == 0 {
                    return Err(protocol_error(
                        "v5 WINDOW_UPDATE credit must be greater than zero",
                    ));
                }
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
            self.route_opening_headers(frame.stream_id, envelope, role, frame.priority)
        }
    }

    fn route_opening_headers(
        &mut self,
        stream_id: u64,
        envelope: StreamEnvelope,
        role: MessageRole,
        wire_priority: u8,
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
        let priority = Priority::from_wire(wire_priority)?;
        let initiator = remote_initiator(self.local_initiator);
        let actual_initiator = stream_id_initiator(stream_id)?;
        if actual_initiator != initiator {
            return Err(protocol_error(format!(
                "{actual_initiator:?} stream id {stream_id} cannot be opened by {initiator:?}"
            )));
        }
        if stream_id <= self.last_accepted_remote_stream_id {
            return Err(protocol_error(format!(
                "remote v5 stream id {stream_id} does not increase past {}",
                self.last_accepted_remote_stream_id
            )));
        }
        self.insert_stream(StreamEntry::new(
            stream_id,
            envelope.method.clone(),
            initiator,
            envelope.request_id,
            envelope.cancellation_group.clone(),
            content_encoding,
            priority,
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
        priority: Priority,
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
            priority,
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

    fn clear(&mut self) {
        self.connection_used = 0;
        self.stream_used.clear();
    }
}

#[derive(Debug, Clone)]
pub struct OutboundScheduler {
    queues: Vec<VecDeque<QueuedItem>>,
    connection_window: FlowWindow,
    stream_windows: HashMap<u64, FlowWindow>,
    default_stream_window: u64,
    control_budget: ControlTrafficBudget,
    next_enqueue_order: u64,
    next_priority_queue: usize,
    priority_quota_remaining: usize,
}

#[derive(Debug, Clone)]
struct QueuedItem {
    order: u64,
    item: OutboundItem,
}

#[derive(Debug, Clone)]
enum OutboundItem {
    Frame(Frame),
    Data(DataProducer),
}

impl OutboundItem {
    fn stream_id(&self) -> u64 {
        match self {
            Self::Frame(frame) => frame.stream_id,
            Self::Data(producer) => producer.stream_id,
        }
    }

    fn is_urgent_control(&self) -> bool {
        matches!(
            self,
            Self::Frame(frame)
                if matches!(
                    frame.frame_type,
                    FrameType::ResetStream
                        | FrameType::WindowUpdate
                        | FrameType::Ping
                        | FrameType::Pong
                )
        )
    }

    fn bypasses_stream_order(&self) -> bool {
        matches!(
            self,
            Self::Frame(frame)
                if matches!(frame.frame_type, FrameType::ResetStream | FrameType::WindowUpdate)
        )
    }
}

#[derive(Debug, Clone)]
struct DataProducer {
    stream_id: u64,
    bytes: Arc<Vec<u8>>,
    offset: usize,
    max_body_len: usize,
    options: DataFrameOptions,
    pending: Option<Frame>,
}

enum DataBuffer<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl DataBuffer<'_> {
    fn is_empty(&self) -> bool {
        match self {
            Self::Borrowed(bytes) => bytes.is_empty(),
            Self::Owned(bytes) => bytes.is_empty(),
        }
    }

    fn into_shared(self) -> Arc<Vec<u8>> {
        match self {
            Self::Borrowed(bytes) => Arc::new(bytes.to_vec()),
            Self::Owned(bytes) => Arc::new(bytes),
        }
    }
}

impl DataProducer {
    fn new(
        stream_id: u64,
        bytes: DataBuffer<'_>,
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
                "DATA producer derives uncompressed_len per frame",
            ));
        }
        Ok(Self {
            stream_id,
            bytes: bytes.into_shared(),
            offset: 0,
            max_body_len: limits.max_body_len as usize,
            options,
            pending: None,
        })
    }

    fn pending_frame(&mut self, available_credit: u64) -> io::Result<Option<&Frame>> {
        if self.pending.is_none() {
            self.pending = self.build_next_frame(available_credit)?;
        }
        Ok(self.pending.as_ref())
    }

    fn take_pending(&mut self) -> Option<Frame> {
        self.pending.take()
    }

    fn is_exhausted(&self) -> bool {
        self.pending.is_none() && self.offset >= self.bytes.len()
    }

    fn build_next_frame(&mut self, available_credit: u64) -> io::Result<Option<Frame>> {
        if self.offset >= self.bytes.len() {
            return Ok(None);
        }
        if available_credit == 0 {
            return Ok(None);
        }

        let credit_limit = usize::try_from(available_credit).unwrap_or(usize::MAX);
        let mut end = self
            .offset
            .saturating_add(self.max_body_len.min(credit_limit))
            .min(self.bytes.len());
        let frame = if self.options.content_encoding == ContentEncoding::Zstd {
            loop {
                let uncompressed = &self.bytes[self.offset..end];
                let compressed = zstd::bulk::compress(uncompressed, ZSTD_DATA_COMPRESSION_LEVEL)
                    .map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("failed to compress v5 DATA chunk with zstd: {error}"),
                        )
                    })?;
                if compressed.len() <= self.max_body_len {
                    let frame = build_data_frame(
                        self.stream_id,
                        compressed,
                        DataFrameOptions {
                            uncompressed_len: Some(uncompressed.len() as u64),
                            ..self.options
                        },
                    );
                    self.offset = end;
                    break frame;
                }
                let uncompressed_len = end.saturating_sub(self.offset);
                if uncompressed_len <= 1 {
                    return Err(protocol_error(
                        "compressed DATA chunk exceeds negotiated frame body limit",
                    ));
                }
                end = self.offset + (uncompressed_len / 2);
            }
        } else {
            let body = self.bytes[self.offset..end].to_vec();
            self.offset = end;
            build_data_frame(self.stream_id, body, self.options)
        };
        Ok(Some(frame))
    }
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
            next_priority_queue: 0,
            priority_quota_remaining: PRIORITY_SERVICE_WEIGHTS[0],
        }
    }

    /// Returns the number of queued scheduler items.
    ///
    /// A lazy DATA producer counts as one item regardless of how many frames remain.
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
        Priority::from_wire(frame.priority)?;
        self.control_budget.reserve_frame(&frame)?;
        let index = Priority::queue_index_from_wire(frame.priority);
        let order = self.next_enqueue_order;
        self.next_enqueue_order = self
            .next_enqueue_order
            .checked_add(1)
            .ok_or_else(|| io::Error::other("v5 outbound scheduler order exhausted"))?;
        self.queues[index].push_back(QueuedItem {
            order,
            item: OutboundItem::Frame(frame),
        });
        Ok(())
    }

    fn enqueue_data(&mut self, producer: DataProducer) -> io::Result<()> {
        Priority::from_wire(producer.options.priority.as_u8())?;
        let index = Priority::queue_index_from_wire(producer.options.priority.as_u8());
        let order = self.next_enqueue_order;
        self.next_enqueue_order = self
            .next_enqueue_order
            .checked_add(1)
            .ok_or_else(|| io::Error::other("v5 outbound scheduler order exhausted"))?;
        self.queues[index].push_back(QueuedItem {
            order,
            item: OutboundItem::Data(producer),
        });
        Ok(())
    }

    pub fn enqueue_batch(&mut self, frames: Vec<Frame>) -> io::Result<()> {
        let mut control_budget = self.control_budget.clone();
        let mut next_order = self.next_enqueue_order;
        for frame in &frames {
            if frame.frame_type == FrameType::Data && frame.stream_id == 0 {
                return Err(protocol_error("DATA frames require a non-zero stream id"));
            }
            Priority::from_wire(frame.priority)?;
            control_budget.reserve_frame(frame)?;
            next_order = next_order
                .checked_add(1)
                .ok_or_else(|| io::Error::other("v5 outbound scheduler order exhausted"))?;
        }

        self.control_budget = control_budget;
        for frame in frames {
            let index = Priority::queue_index_from_wire(frame.priority);
            let order = self.next_enqueue_order;
            self.next_enqueue_order += 1;
            self.queues[index].push_back(QueuedItem {
                order,
                item: OutboundItem::Frame(frame),
            });
        }
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

    /// Removes every queued item and all per-stream accounting for a terminal stream.
    ///
    /// Callers enqueue a `RESET_STREAM` only after this returns so the reset can never
    /// overtake stale data or end-of-stream frames for the same stream.
    pub fn drop_stream(&mut self, stream_id: u64) -> usize {
        let mut dropped = 0;
        for queue in &mut self.queues {
            let mut retained = VecDeque::with_capacity(queue.len());
            while let Some(queued) = queue.pop_front() {
                if queued.item.stream_id() == stream_id {
                    if let OutboundItem::Frame(frame) = &queued.item {
                        self.control_budget.release_frame(frame);
                    }
                    dropped += 1;
                } else {
                    retained.push_back(queued);
                }
            }
            *queue = retained;
        }
        self.stream_windows.remove(&stream_id);
        dropped
    }

    fn drop_stream_window(&mut self, stream_id: u64) {
        self.stream_windows.remove(&stream_id);
    }

    fn has_pending_stream(&self, stream_id: u64) -> bool {
        self.queues.iter().any(|queue| {
            queue
                .iter()
                .any(|queued| queued.item.stream_id() == stream_id)
        })
    }

    pub fn clear(&mut self) {
        for queue in &mut self.queues {
            queue.clear();
        }
        self.stream_windows.clear();
        self.control_budget.clear();
        self.next_priority_queue = 0;
        self.priority_quota_remaining = PRIORITY_SERVICE_WEIGHTS[0];
    }

    pub fn pop_next(&mut self) -> io::Result<Option<Frame>> {
        if let Some(frame) = self.pop_urgent_control() {
            return Ok(Some(frame));
        }

        for _ in 0..PRIORITY_LEVELS {
            let queue_index = self.next_priority_queue;
            if let Some(frame) = self.pop_from_priority_queue(queue_index)? {
                self.priority_quota_remaining = self.priority_quota_remaining.saturating_sub(1);
                if self.priority_quota_remaining == 0 {
                    self.advance_priority_queue();
                }
                return Ok(Some(frame));
            }
            self.advance_priority_queue();
        }
        Ok(None)
    }

    fn pop_urgent_control(&mut self) -> Option<Frame> {
        for queue_index in 0..self.queues.len() {
            let urgent_index = self.queues[queue_index]
                .iter()
                .position(|queued| queued.item.is_urgent_control());
            let Some(urgent_index) = urgent_index else {
                continue;
            };
            let queued = self.queues[queue_index]
                .remove(urgent_index)
                .expect("urgent queue index was just observed");
            let OutboundItem::Frame(frame) = queued.item else {
                unreachable!("only concrete frames are urgent control")
            };
            self.control_budget.release_frame(&frame);
            if frame.frame_type == FrameType::ResetStream {
                self.stream_windows.remove(&frame.stream_id);
            }
            return Some(frame);
        }
        None
    }

    fn pop_from_priority_queue(&mut self, queue_index: usize) -> io::Result<Option<Frame>> {
        let mut blocked_streams = HashSet::new();
        let items_to_scan = self.queues[queue_index].len();
        for _ in 0..items_to_scan {
            let mut queued = self.queues[queue_index]
                .pop_front()
                .expect("queue length was checked");
            if self.is_blocked_by_earlier_stream_item(&queued, &blocked_streams) {
                self.queues[queue_index].push_back(queued);
                continue;
            }
            if self.has_earlier_same_stream_item(&queued) && !queued.item.bypasses_stream_order() {
                self.queues[queue_index].push_back(queued);
                continue;
            }

            let available_credit = self
                .connection_window
                .available()
                .min(self.stream_window(queued.item.stream_id()));
            match &mut queued.item {
                OutboundItem::Frame(frame) => {
                    if self.can_send(frame) {
                        self.consume_credit(frame)?;
                        self.control_budget.release_frame(frame);
                        if matches!(
                            frame.frame_type,
                            FrameType::EndStream | FrameType::ResetStream
                        ) {
                            self.stream_windows.remove(&frame.stream_id);
                        }
                        let OutboundItem::Frame(frame) = queued.item else {
                            unreachable!("matched a concrete outbound frame")
                        };
                        return Ok(Some(frame));
                    }
                    if frame.frame_type == FrameType::Data {
                        blocked_streams.insert(frame.stream_id);
                    }
                }
                OutboundItem::Data(producer) => {
                    if available_credit == 0 {
                        blocked_streams.insert(producer.stream_id);
                        self.queues[queue_index].push_back(queued);
                        continue;
                    }
                    let frame = match producer.pending_frame(available_credit) {
                        Ok(Some(frame)) => frame,
                        Ok(None) => continue,
                        Err(error) => {
                            self.queues[queue_index].push_back(queued);
                            return Err(error);
                        }
                    };
                    if self.can_send(frame) {
                        self.consume_credit(frame)?;
                        let frame = producer
                            .take_pending()
                            .expect("pending DATA frame was just observed");
                        if !producer.is_exhausted() {
                            self.queues[queue_index].push_back(queued);
                        }
                        return Ok(Some(frame));
                    }
                    blocked_streams.insert(producer.stream_id);
                }
            }
            self.queues[queue_index].push_back(queued);
        }
        Ok(None)
    }

    fn advance_priority_queue(&mut self) {
        self.next_priority_queue = (self.next_priority_queue + 1) % PRIORITY_LEVELS;
        self.priority_quota_remaining = PRIORITY_SERVICE_WEIGHTS[self.next_priority_queue];
    }

    fn is_blocked_by_earlier_stream_item(
        &self,
        queued: &QueuedItem,
        blocked_streams: &HashSet<u64>,
    ) -> bool {
        let stream_id = queued.item.stream_id();
        stream_id != 0
            && blocked_streams.contains(&stream_id)
            && !queued.item.bypasses_stream_order()
    }

    fn has_earlier_same_stream_item(&self, queued: &QueuedItem) -> bool {
        let stream_id = queued.item.stream_id();
        if stream_id == 0 || queued.item.bypasses_stream_order() {
            return false;
        }
        self.queues.iter().any(|queue| {
            queue.iter().any(|earlier| {
                earlier.order < queued.order
                    && earlier.item.stream_id() == stream_id
                    && !earlier.item.bypasses_stream_order()
            })
        })
    }

    #[cfg(test)]
    fn pending_data_frames(&self, stream_id: u64) -> usize {
        self.queues
            .iter()
            .flat_map(|queue| queue.iter())
            .filter(|queued| match &queued.item {
                OutboundItem::Frame(frame) => {
                    frame.stream_id == stream_id && frame.frame_type == FrameType::Data
                }
                OutboundItem::Data(producer) => {
                    producer.stream_id == stream_id && producer.pending.is_some()
                }
            })
            .count()
    }

    #[cfg(test)]
    fn producer_buffer_ptrs(&self, stream_id: u64) -> Vec<*const u8> {
        self.queues
            .iter()
            .flat_map(|queue| queue.iter())
            .filter_map(|queued| match &queued.item {
                OutboundItem::Data(producer) if producer.stream_id == stream_id => {
                    Some(producer.bytes.as_slice().as_ptr())
                }
                _ => None,
            })
            .collect()
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

    fn refund_credit(&mut self, frame: &Frame) -> io::Result<()> {
        let flow_len = frame.flow_control_len();
        if flow_len == 0 {
            return Ok(());
        }

        let mut connection_window = self.connection_window;
        connection_window.grant(flow_len)?;
        let stream_window = self.stream_windows.get(&frame.stream_id).copied();
        let stream_window = match stream_window {
            Some(mut stream_window) => {
                stream_window.grant(flow_len)?;
                Some(stream_window)
            }
            None => None,
        };
        self.connection_window = connection_window;
        if let Some(stream_window) = stream_window {
            self.stream_windows.insert(frame.stream_id, stream_window);
        }
        Ok(())
    }

    fn stream_window_mut(&mut self, stream_id: u64) -> &mut FlowWindow {
        self.stream_windows
            .entry(stream_id)
            .or_insert_with(|| FlowWindow::new(self.default_stream_window))
    }
}

#[derive(Debug, Clone)]
struct ReceiveFlowControl {
    connection_window: FlowWindow,
    connection_limit: u64,
    stream_windows: HashMap<u64, FlowWindow>,
    default_stream_window: u64,
}

impl ReceiveFlowControl {
    fn new(settings: &ConnectionSettings) -> Self {
        let connection_limit = nonzero_or(
            settings.initial_connection_window,
            DEFAULT_CONNECTION_WINDOW,
        ) as u64;
        Self {
            connection_window: FlowWindow::new(connection_limit),
            connection_limit,
            stream_windows: HashMap::new(),
            default_stream_window: nonzero_or(settings.initial_stream_window, DEFAULT_STREAM_WINDOW)
                as u64,
        }
    }

    fn connection_window(&self) -> u64 {
        self.connection_window.available()
    }

    fn stream_window(&self, stream_id: u64) -> u64 {
        self.stream_windows
            .get(&stream_id)
            .map(FlowWindow::available)
            .unwrap_or(self.default_stream_window)
    }

    fn consume(&mut self, stream_id: u64, bytes: u64) -> io::Result<()> {
        if stream_id == 0 {
            return Err(protocol_error(
                "received v5 DATA flow credit for stream zero",
            ));
        }
        let connection_available = self.connection_window();
        let stream_available = self.stream_window(stream_id);
        if bytes > connection_available || bytes > stream_available {
            return Err(protocol_error(format!(
                "received v5 DATA exceeds flow-control credit on stream {stream_id}: \
                 need {bytes}, connection has {connection_available}, stream has {stream_available}"
            )));
        }

        self.connection_window.consume(bytes)?;
        self.stream_window_mut(stream_id).consume(bytes)
    }

    fn acknowledge(&mut self, stream_id: u64, bytes: u64) -> io::Result<()> {
        if stream_id == 0 {
            return Err(protocol_error(
                "v5 DATA acknowledgement requires non-zero stream id",
            ));
        }
        if !self.stream_windows.contains_key(&stream_id) {
            return Err(protocol_error(format!(
                "cannot acknowledge DATA for unknown v5 receive stream {stream_id}"
            )));
        }

        let connection_available = self.connection_window();
        let stream_available = self.stream_window(stream_id);
        if connection_available
            .checked_add(bytes)
            .is_none_or(|value| value > self.connection_limit)
            || stream_available
                .checked_add(bytes)
                .is_none_or(|value| value > self.default_stream_window)
        {
            return Err(protocol_error(format!(
                "v5 DATA acknowledgement overflows receive window for stream {stream_id}"
            )));
        }

        self.connection_window.grant(bytes)?;
        self.stream_window_mut(stream_id).grant(bytes)
    }

    fn drop_stream(&mut self, stream_id: u64) -> io::Result<u64> {
        let Some(window) = self.stream_windows.remove(&stream_id) else {
            return Ok(0);
        };
        let released = self.default_stream_window - window.available();
        self.connection_window.grant(released)?;
        Ok(released)
    }

    fn clear(&mut self) {
        self.connection_window = FlowWindow::new(self.connection_limit);
        self.stream_windows.clear();
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
        priority: Priority,
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
        Self::from_frame_with_content_encoding_and_limit(
            frame,
            ContentEncoding::None,
            DEFAULT_MAX_FRAME_BODY_LEN as u64,
        )
    }

    pub fn from_frame_with_content_encoding(
        frame: Frame,
        content_encoding: ContentEncoding,
    ) -> io::Result<Option<Self>> {
        Self::from_frame_with_content_encoding_and_limit(
            frame,
            content_encoding,
            DEFAULT_MAX_FRAME_BODY_LEN as u64,
        )
    }

    fn from_frame_with_content_encoding_and_limit(
        frame: Frame,
        content_encoding: ContentEncoding,
        max_decoded_len: u64,
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
                    priority: Priority::from_wire(frame.priority)?,
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
                let body = decode_data_body(
                    frame.body,
                    content_encoding,
                    envelope.uncompressed_len,
                    max_decoded_len,
                )?;
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

fn validated_data_flow_control_len(frame: &Frame, limits: FrameLimits) -> io::Result<u64> {
    validate_frame_for_write(frame, limits)?;
    if frame.frame_type != FrameType::Data {
        return Err(protocol_error(
            "v5 crossing-data credit requires a DATA frame",
        ));
    }

    let credit = if frame.control.is_empty() {
        frame.body.len() as u64
    } else {
        let envelope = decode_control::<DataEnvelope>(frame)?;
        DataChannel::try_from(envelope.channel).map_err(|_| {
            protocol_error(format!("unknown v5 data channel: {}", envelope.channel))
        })?;
        if envelope.uncompressed_len == 0 {
            frame.body.len() as u64
        } else {
            envelope.uncompressed_len
        }
    };
    if credit > u64::from(limits.max_body_len) {
        return Err(protocol_error(format!(
            "v5 crossing DATA decoded length {credit} exceeds maximum {}",
            limits.max_body_len
        )));
    }
    Ok(credit)
}

fn decode_data_body(
    body: Vec<u8>,
    content_encoding: ContentEncoding,
    uncompressed_len: u64,
    max_decoded_len: u64,
) -> io::Result<Vec<u8>> {
    if uncompressed_len > max_decoded_len {
        return Err(protocol_error(format!(
            "v5 DATA frame decoded length {uncompressed_len} exceeds maximum {max_decoded_len}"
        )));
    }
    match content_encoding {
        ContentEncoding::None => {
            if body.len() as u64 != uncompressed_len {
                return Err(protocol_error(format!(
                    "v5 uncompressed DATA frame body has {} bytes, declared {uncompressed_len}",
                    body.len()
                )));
            }
            Ok(body)
        }
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
    receive_flow: ReceiveFlowControl,
    in_flight: InFlightRequests,
    limits: FrameLimits,
    shutdown_grace_ms: u32,
    peer_goaway: Option<GoAway>,
    sent_goaway: Option<GoAway>,
    stream_tombstones: HashMap<u64, StreamTombstone>,
    closed_stream_watermarks: [u64; 2],
    crossing_receive_allowances: HashMap<u64, u64>,
    crossing_receive_allowance_order: VecDeque<u64>,
    crossing_receive_allowance_limit: usize,
    locally_ended_on_wire: HashSet<u64>,
    extracted_streams: HashMap<u64, usize>,
    receive_credit_update_threshold: u64,
    pending_receive_credit: HashMap<u64, u64>,
    pending_receive_connection_credit: u64,
    min_unsolicited_ping_interval: Duration,
    last_unsolicited_ping_at: Option<Instant>,
    terminated: bool,
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
        let stream_window = u64::from(nonzero_or(
            settings.initial_stream_window,
            DEFAULT_STREAM_WINDOW,
        ));
        let connection_window = u64::from(nonzero_or(
            settings.initial_connection_window,
            DEFAULT_CONNECTION_WINDOW,
        ));
        let receive_credit_update_threshold = (stream_window.min(connection_window) / 2).max(1);
        Self {
            streams: StreamTable::new(local_initiator, settings),
            scheduler: OutboundScheduler::new(settings),
            receive_flow: ReceiveFlowControl::new(settings),
            in_flight: InFlightRequests::default(),
            limits,
            shutdown_grace_ms: nonzero_or(settings.shutdown_grace_ms, DEFAULT_SHUTDOWN_GRACE_MS),
            peer_goaway: None,
            sent_goaway: None,
            stream_tombstones: HashMap::new(),
            closed_stream_watermarks: [0; 2],
            crossing_receive_allowances: HashMap::new(),
            crossing_receive_allowance_order: VecDeque::new(),
            crossing_receive_allowance_limit: nonzero_or(
                settings.max_concurrent_streams,
                DEFAULT_MAX_CONCURRENT_STREAMS,
            )
            .min(DEFAULT_MAX_CONCURRENT_STREAMS)
                as usize,
            locally_ended_on_wire: HashSet::new(),
            extracted_streams: HashMap::new(),
            receive_credit_update_threshold,
            pending_receive_credit: HashMap::new(),
            pending_receive_connection_credit: 0,
            min_unsolicited_ping_interval: Duration::from_millis(u64::from(nonzero_or(
                settings.min_unsolicited_ping_interval_ms,
                MIN_UNSOLICITED_PING_INTERVAL_MS,
            ))),
            last_unsolicited_ping_at: None,
            terminated: false,
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

    pub fn receive_stream_window(&self, stream_id: u64) -> u64 {
        self.receive_flow.stream_window(stream_id)
    }

    pub fn receive_connection_window(&self) -> u64 {
        self.receive_flow.connection_window()
    }

    pub fn last_accepted_remote_stream_id(&self) -> u64 {
        self.streams.last_accepted_remote_stream_id()
    }

    pub fn stream_priority(&self, stream_id: u64) -> Option<Priority> {
        self.streams.get(stream_id).map(|entry| entry.priority)
    }

    pub fn peer_goaway(&self) -> Option<&GoAway> {
        self.peer_goaway.as_ref()
    }

    pub fn sent_goaway(&self) -> Option<&GoAway> {
        self.sent_goaway.as_ref()
    }

    pub fn stream_tombstone(&self, stream_id: u64) -> Option<StreamTombstone> {
        if let Some(tombstone) = self.stream_tombstones.get(&stream_id) {
            return Some(*tombstone);
        }
        if stream_id == 0
            || self.streams.get(stream_id).is_some()
            || self.scheduler.has_pending_stream(stream_id)
            || self.extracted_streams.contains_key(&stream_id)
        {
            return None;
        }
        let index = usize::from(stream_id.is_multiple_of(2));
        (stream_id <= self.closed_stream_watermarks[index]).then_some(StreamTombstone::Closed)
    }

    /// Returns whether a previously extracted frame is still valid to write.
    ///
    /// Writers should call this immediately before each unflushed frame write. A locally
    /// generated reset remains writable after its stream is tombstoned; all other frames for
    /// reset or fully closed streams are stale.
    pub fn should_write_frame(&self, frame: &Frame) -> bool {
        if self.terminated {
            return false;
        }
        if frame.stream_id == 0 {
            return true;
        }
        match self.stream_tombstone(frame.stream_id) {
            None => true,
            Some(StreamTombstone::Reset) => frame.frame_type == FrameType::ResetStream,
            Some(StreamTombstone::Closed) => false,
        }
    }

    /// Releases send credit consumed when an extracted DATA frame is invalidated before write.
    ///
    /// A reset removes the per-stream window, so only still-live stream state is restored. The
    /// connection credit must always be restored because the peer never observed the bytes.
    pub fn discard_unwritten_frame(&mut self, frame: &Frame) -> io::Result<()> {
        if self.terminated {
            return Ok(());
        }
        self.scheduler.refund_credit(frame)?;
        self.settle_extracted_frame(frame.stream_id);
        Ok(())
    }

    /// Records successful wire delivery of a terminal stream frame.
    ///
    /// This delays normal-close tombstoning until `END_STREAM` has actually been written, so a
    /// response batch extracted before logical closure remains valid.
    pub fn observe_frame_written(&mut self, frame: &Frame) {
        self.observe_frame_parts_written(frame.stream_id, frame.frame_type);
    }

    pub(crate) fn observe_frame_parts_written(&mut self, stream_id: u64, frame_type: FrameType) {
        if self.terminated || stream_id == 0 {
            return;
        }
        self.settle_extracted_frame(stream_id);
        match frame_type {
            FrameType::ResetStream => {
                self.locally_ended_on_wire.remove(&stream_id);
                self.retire_closed_stream(stream_id);
            }
            FrameType::EndStream => {
                if self.stream_tombstone(stream_id) == Some(StreamTombstone::Reset) {
                    return;
                }
                if self.streams.get(stream_id).is_none() {
                    self.locally_ended_on_wire.remove(&stream_id);
                    self.retire_closed_stream(stream_id);
                } else {
                    self.locally_ended_on_wire.insert(stream_id);
                }
            }
            _ => {}
        }
    }

    fn settle_extracted_frame(&mut self, stream_id: u64) {
        let Some(count) = self.extracted_streams.get_mut(&stream_id) else {
            return;
        };
        if *count <= 1 {
            self.extracted_streams.remove(&stream_id);
        } else {
            *count -= 1;
        }
    }

    fn register_crossing_receive_allowance(&mut self, stream_id: u64, credit: u64) {
        if stream_id == 0
            || credit == 0
            || self.crossing_receive_allowances.contains_key(&stream_id)
        {
            return;
        }
        while self.crossing_receive_allowances.len() >= self.crossing_receive_allowance_limit {
            let Some(expired_stream_id) = self.crossing_receive_allowance_order.pop_front() else {
                break;
            };
            self.crossing_receive_allowances.remove(&expired_stream_id);
        }
        self.crossing_receive_allowances.insert(stream_id, credit);
        self.crossing_receive_allowance_order.push_back(stream_id);
    }

    fn consume_crossing_receive_allowance(
        &mut self,
        stream_id: u64,
        credit: u64,
    ) -> io::Result<()> {
        if credit == 0 {
            return Ok(());
        }
        let remaining = self
            .crossing_receive_allowances
            .get_mut(&stream_id)
            .ok_or_else(|| {
                protocol_error(format!(
                    "received DATA without residual credit on tombstoned v5 stream {stream_id}"
                ))
            })?;
        if credit > *remaining {
            return Err(protocol_error(format!(
                "crossing v5 DATA exceeds residual stream credit on tombstoned stream \
                 {stream_id}: need {credit}, have {remaining}"
            )));
        }
        *remaining -= credit;
        if *remaining == 0 {
            self.crossing_receive_allowances.remove(&stream_id);
            self.crossing_receive_allowance_order
                .retain(|queued_stream_id| *queued_stream_id != stream_id);
        }
        Ok(())
    }

    fn forget_crossing_receive_allowance(&mut self, stream_id: u64) {
        if self
            .crossing_receive_allowances
            .remove(&stream_id)
            .is_some()
        {
            self.crossing_receive_allowance_order
                .retain(|queued_stream_id| *queued_stream_id != stream_id);
        }
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
        self.open_request_with_buffers(
            method,
            options,
            DataBuffer::Borrowed(&[]),
            channel,
            DataBuffer::Borrowed(body),
        )
    }

    /// Opens a request while transferring ownership of its DATA body to the session.
    ///
    /// Unlike [`Self::open_request_with_body`], this avoids copying the complete body into
    /// the lazy outbound producer.
    pub fn open_request_with_owned_body(
        &mut self,
        method: impl Into<String>,
        options: RequestOptions,
        channel: DataChannel,
        body: Vec<u8>,
    ) -> io::Result<u64> {
        self.open_request_with_buffers(
            method,
            options,
            DataBuffer::Borrowed(&[]),
            channel,
            DataBuffer::Owned(body),
        )
    }

    pub fn open_request_with_payload_and_body(
        &mut self,
        method: impl Into<String>,
        options: RequestOptions,
        payload: &[u8],
        body_channel: DataChannel,
        body: &[u8],
    ) -> io::Result<u64> {
        self.open_request_with_buffers(
            method,
            options,
            DataBuffer::Borrowed(payload),
            body_channel,
            DataBuffer::Borrowed(body),
        )
    }

    /// Opens a request while transferring ownership of both DATA buffers to the session.
    ///
    /// The payload and body allocations are retained by the corresponding lazy producers.
    pub fn open_request_with_owned_payload_and_body(
        &mut self,
        method: impl Into<String>,
        options: RequestOptions,
        payload: Vec<u8>,
        body_channel: DataChannel,
        body: Vec<u8>,
    ) -> io::Result<u64> {
        self.open_request_with_buffers(
            method,
            options,
            DataBuffer::Owned(payload),
            body_channel,
            DataBuffer::Owned(body),
        )
    }

    fn open_request_with_buffers(
        &mut self,
        method: impl Into<String>,
        options: RequestOptions,
        payload: DataBuffer<'_>,
        body_channel: DataChannel,
        body: DataBuffer<'_>,
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
        self.send_progress_with_priority(stream_id, method, progress, Priority::Background)
    }

    pub fn send_progress_with_priority(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        progress: Progress,
        priority: Priority,
    ) -> io::Result<()> {
        self.ensure_open_stream(stream_id)?;
        self.scheduler.enqueue(
            Frame::from_control(
                FrameType::Headers,
                stream_id,
                &StreamEnvelope::progress(stream_id, method, progress),
            )
            .with_priority(priority),
        )
    }

    pub fn send_response(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        role: MessageRole,
        complete: bool,
    ) -> io::Result<()> {
        self.send_response_with_priority(stream_id, method, role, complete, Priority::Background)
    }

    pub fn send_response_with_priority(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        role: MessageRole,
        complete: bool,
        priority: Priority,
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
            .with_priority(priority),
        )
    }

    pub fn send_error(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        error: ErrorHeader,
    ) -> io::Result<()> {
        self.send_error_with_priority(stream_id, method, error, Priority::Background)
    }

    pub fn send_error_with_priority(
        &mut self,
        stream_id: u64,
        method: impl Into<String>,
        error: ErrorHeader,
        priority: Priority,
    ) -> io::Result<()> {
        self.ensure_open_stream(stream_id)?;
        self.scheduler.enqueue(
            Frame::from_control(
                FrameType::Headers,
                stream_id,
                &StreamEnvelope::error(stream_id, method, error),
            )
            .with_priority(priority),
        )
    }

    pub fn send_data(
        &mut self,
        stream_id: u64,
        channel: DataChannel,
        body: &[u8],
        priority: Priority,
    ) -> io::Result<()> {
        self.send_data_buffer(stream_id, channel, DataBuffer::Borrowed(body), priority)
    }

    /// Queues DATA while transferring ownership of its allocation to the session.
    pub fn send_owned_data(
        &mut self,
        stream_id: u64,
        channel: DataChannel,
        body: Vec<u8>,
        priority: Priority,
    ) -> io::Result<()> {
        self.send_data_buffer(stream_id, channel, DataBuffer::Owned(body), priority)
    }

    fn send_data_buffer(
        &mut self,
        stream_id: u64,
        channel: DataChannel,
        body: DataBuffer<'_>,
        priority: Priority,
    ) -> io::Result<()> {
        self.ensure_open_stream(stream_id)?;
        if body.is_empty() {
            return Ok(());
        }
        let content_encoding = self
            .streams
            .get(stream_id)
            .map(|entry| entry.content_encoding)
            .unwrap_or(ContentEncoding::None);
        let options = DataFrameOptions::new(channel)
            .with_priority(priority)
            .with_content_encoding(content_encoding);
        self.scheduler
            .enqueue_data(DataProducer::new(stream_id, body, self.limits, options)?)
    }

    pub fn finish_stream(&mut self, stream_id: u64, priority: Priority) -> io::Result<StreamState> {
        self.ensure_open_stream(stream_id)?;
        self.scheduler
            .enqueue(end_stream_frame(stream_id)?.with_priority(priority))?;
        let state = self.streams.mark_local_end(stream_id)?;
        if state == StreamState::Closed {
            self.release_receive_stream(stream_id)?;
        }
        Ok(state)
    }

    pub fn receive_frame(&mut self, frame: Frame) -> io::Result<SessionEvent> {
        self.receive_frame_at(frame, Instant::now())
    }

    fn receive_frame_at(&mut self, frame: Frame, received_at: Instant) -> io::Result<SessionEvent> {
        Priority::from_wire(frame.priority)?;
        if frame.stream_id != 0 && self.stream_tombstone(frame.stream_id).is_some() {
            if frame.frame_type == FrameType::Data {
                let credit = validated_data_flow_control_len(&frame, self.limits)?;
                let connection_available = self.receive_flow.connection_window();
                let stream_available = self.receive_flow.stream_window(frame.stream_id);
                if credit > connection_available || credit > stream_available {
                    return Err(protocol_error(format!(
                        "crossing v5 DATA exceeds flow-control credit on tombstoned stream {}: \
                         need {credit}, connection has {connection_available}, stream has \
                         {stream_available}",
                        frame.stream_id
                    )));
                }
                if credit > 0 {
                    let remaining = self
                        .crossing_receive_allowances
                        .get(&frame.stream_id)
                        .copied()
                        .ok_or_else(|| {
                            protocol_error(format!(
                                "received DATA without residual credit on tombstoned v5 stream {}",
                                frame.stream_id
                            ))
                        })?;
                    if credit > remaining {
                        return Err(protocol_error(format!(
                            "crossing v5 DATA exceeds residual stream credit on tombstoned stream \
                             {}: need {credit}, have {remaining}",
                            frame.stream_id
                        )));
                    }
                    self.scheduler.enqueue(
                        window_update_frame(0, credit)?.with_priority(Priority::UserInput),
                    )?;
                    self.consume_crossing_receive_allowance(frame.stream_id, credit)?;
                }
            } else if matches!(
                frame.frame_type,
                FrameType::EndStream | FrameType::ResetStream
            ) {
                self.forget_crossing_receive_allowance(frame.stream_id);
            }
            return Ok(SessionEvent {
                routed: RoutedFrame::RejectedStream {
                    stream_id: frame.stream_id,
                },
                stream_event: None,
            });
        }
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
                    if self.streams.get(*stream_id).is_none() {
                        if self.scheduler.has_pending_stream(*stream_id) {
                            self.scheduler.grant_stream(*stream_id, *credit_bytes)?;
                        } else if !self.extracted_streams.contains_key(stream_id) {
                            return Err(protocol_error(format!(
                                "received WINDOW_UPDATE for unknown v5 stream {stream_id}"
                            )));
                        }
                        return Ok(SessionEvent {
                            routed,
                            stream_event: None,
                        });
                    }
                    self.scheduler.grant_stream(*stream_id, *credit_bytes)?;
                }
                Ok(SessionEvent {
                    routed,
                    stream_event: None,
                })
            }
            RoutedFrame::ConnectionControl { frame_type } => {
                match frame_type {
                    FrameType::Ping => {
                        self.observe_unsolicited_ping(received_at)?;
                        self.queue_pong(frame.control.clone())?;
                    }
                    FrameType::GoAway => {
                        self.peer_goaway = Some(decode_control::<GoAway>(&frame)?);
                    }
                    FrameType::Hello | FrameType::Settings | FrameType::SettingsAck => {
                        return Err(protocol_error(format!(
                            "received handshake frame {frame_type:?} after v5 session activation"
                        )));
                    }
                    FrameType::Pong => {}
                    _ => unreachable!("stream control was routed as connection control"),
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
                if let RoutedFrame::Data {
                    stream_id,
                    flow_control_len,
                } = &routed
                {
                    self.receive_flow.consume(*stream_id, *flow_control_len)?;
                }
                let content_encoding = match &routed {
                    RoutedFrame::Data { stream_id, .. } => self
                        .streams
                        .get(*stream_id)
                        .map(|entry| entry.content_encoding)
                        .unwrap_or(ContentEncoding::None),
                    _ => ContentEncoding::None,
                };
                let event = StreamEvent::from_frame_with_content_encoding_and_limit(
                    frame,
                    content_encoding,
                    self.limits.max_body_len as u64,
                )?;
                if let Some(event) = event.as_ref() {
                    self.observe_stream_event(event)?;
                }
                match &routed {
                    RoutedFrame::ResetStream { stream_id, .. } => {
                        self.discard_stream_state(*stream_id)?;
                    }
                    RoutedFrame::EndStream {
                        stream_id,
                        state: StreamState::Closed,
                    } => {
                        self.release_receive_stream(*stream_id)?;
                        if self.locally_ended_on_wire.remove(stream_id) {
                            self.scheduler.drop_stream_window(*stream_id);
                            self.retire_closed_stream(*stream_id);
                        }
                    }
                    _ => {}
                }
                Ok(SessionEvent {
                    routed,
                    stream_event: event,
                })
            }
        }
    }

    fn observe_unsolicited_ping(&mut self, received_at: Instant) -> io::Result<()> {
        if let Some(previous) = self.last_unsolicited_ping_at {
            let elapsed = received_at.saturating_duration_since(previous);
            if elapsed < self.min_unsolicited_ping_interval {
                return Err(protocol_error(format!(
                    "received unsolicited v5 PING after {} ms; negotiated minimum is {} ms",
                    elapsed.as_millis(),
                    self.min_unsolicited_ping_interval.as_millis(),
                )));
            }
        }
        self.last_unsolicited_ping_at = Some(received_at);
        Ok(())
    }

    pub fn acknowledge_data(&mut self, stream_id: u64, credit_bytes: u64) -> io::Result<()> {
        if credit_bytes == 0 {
            return Ok(());
        }

        let stream_credit = self
            .pending_receive_credit
            .get(&stream_id)
            .copied()
            .unwrap_or(0)
            .checked_add(credit_bytes)
            .ok_or_else(|| protocol_error("v5 pending stream receive credit overflow"))?;
        let connection_credit = self
            .pending_receive_connection_credit
            .checked_add(credit_bytes)
            .ok_or_else(|| protocol_error("v5 pending connection receive credit overflow"))?;

        let mut validated_flow = self.receive_flow.clone();
        validated_flow.acknowledge(stream_id, stream_credit)?;

        if stream_credit >= self.receive_credit_update_threshold
            || connection_credit >= self.receive_credit_update_threshold
        {
            let mut pending = self.pending_receive_credit.clone();
            pending.insert(stream_id, stream_credit);
            self.flush_receive_credit(&pending, connection_credit)?;
            self.pending_receive_credit.clear();
            self.pending_receive_connection_credit = 0;
        } else {
            self.pending_receive_credit.insert(stream_id, stream_credit);
            self.pending_receive_connection_credit = connection_credit;
        }
        Ok(())
    }

    fn flush_receive_credit(
        &mut self,
        pending: &HashMap<u64, u64>,
        connection_credit: u64,
    ) -> io::Result<()> {
        let mut credits = pending
            .iter()
            .map(|(stream_id, credit)| (*stream_id, *credit))
            .collect::<Vec<_>>();
        credits.sort_unstable_by_key(|(stream_id, _)| *stream_id);

        let summed_credit = credits.iter().try_fold(0_u64, |total, (_, credit)| {
            total
                .checked_add(*credit)
                .ok_or_else(|| protocol_error("v5 pending receive credit overflow"))
        })?;
        if summed_credit != connection_credit {
            return Err(protocol_error(
                "v5 pending stream and connection receive credit diverged",
            ));
        }

        let mut receive_flow = self.receive_flow.clone();
        for (stream_id, credit) in &credits {
            receive_flow.acknowledge(*stream_id, *credit)?;
        }

        let mut frames = Vec::with_capacity(credits.len() + 1);
        frames.push(window_update_frame(0, connection_credit)?.with_priority(Priority::UserInput));
        for (stream_id, credit) in credits {
            frames.push(window_update_frame(stream_id, credit)?.with_priority(Priority::UserInput));
        }
        self.scheduler.enqueue_batch(frames)?;
        self.receive_flow = receive_flow;
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
        self.queue_terminal_reset(frame)?;
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
            self.queue_terminal_reset(reset_stream_frame(stream_id, code, diagnostic))?;
        } else if self.stream_tombstone(stream_id).is_none() {
            self.discard_stream_state(stream_id)?;
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
            self.queue_terminal_reset(frame)?;
        }
        Ok(frame_count)
    }

    pub fn expire_deadlines(&mut self, now_unix_ms: u64) -> io::Result<usize> {
        let frames = self.in_flight.expire_deadlines(now_unix_ms);
        let frame_count = frames.len();
        for frame in frames {
            self.streams.streams.remove(&frame.stream_id);
            self.queue_terminal_reset(frame)?;
        }
        Ok(frame_count)
    }

    pub fn pop_next_frame(&mut self) -> io::Result<Option<Frame>> {
        let frame = self.scheduler.pop_next()?;
        if let Some(frame) = frame.as_ref()
            && frame.stream_id != 0
        {
            let extracted = self.extracted_streams.entry(frame.stream_id).or_default();
            *extracted = extracted.saturating_add(1);
        }
        Ok(frame)
    }

    /// Discards all stream state after the underlying transport becomes unusable.
    pub fn terminate(&mut self) {
        self.terminated = true;
        self.streams.streams.clear();
        self.in_flight.requests.clear();
        self.scheduler.clear();
        self.receive_flow.clear();
        self.stream_tombstones.clear();
        self.crossing_receive_allowances.clear();
        self.crossing_receive_allowance_order.clear();
        self.pending_receive_credit.clear();
        self.pending_receive_connection_credit = 0;
        self.locally_ended_on_wire.clear();
        self.extracted_streams.clear();
        self.closed_stream_watermarks = [0; 2];
    }

    fn queue_open_request(
        &mut self,
        stream_id: u64,
        headers: Frame,
        priority: Priority,
        payload: DataBuffer<'_>,
        channel: DataChannel,
        body: DataBuffer<'_>,
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
        let content_encoding = self
            .streams
            .get(stream_id)
            .map(|entry| entry.content_encoding)
            .unwrap_or(ContentEncoding::None);

        let payload_producer = if payload.is_empty() {
            None
        } else {
            let options = DataFrameOptions::new(DataChannel::Unspecified)
                .with_priority(priority)
                .with_content_encoding(content_encoding);
            Some(DataProducer::new(stream_id, payload, self.limits, options)?)
        };

        let body_producer = if body.is_empty() {
            None
        } else {
            let options = DataFrameOptions::new(channel)
                .with_priority(priority)
                .with_content_encoding(content_encoding);
            Some(DataProducer::new(stream_id, body, self.limits, options)?)
        };

        self.scheduler.enqueue(headers)?;
        if let Some(producer) = payload_producer {
            self.scheduler.enqueue_data(producer)?;
        }
        if let Some(producer) = body_producer {
            self.scheduler.enqueue_data(producer)?;
        }
        self.scheduler
            .enqueue(end_stream_frame(stream_id)?.with_priority(priority))?;
        self.streams.mark_local_end(stream_id)?;

        if let Some(frame) = self.in_flight.cancel_superseded_by(stream_id)? {
            self.streams.streams.remove(&frame.stream_id);
            self.queue_terminal_reset(frame)?;
        }

        Ok(())
    }

    fn observe_stream_event(&mut self, event: &StreamEvent) -> io::Result<()> {
        if let StreamEvent::Headers {
            stream_id,
            role: MessageRole::Request,
            envelope,
            ..
        } = event
        {
            self.in_flight
                .register_from_envelope(*stream_id, envelope)?;
            if let Some(frame) = self.in_flight.cancel_superseded_by(*stream_id)? {
                self.streams.streams.remove(&frame.stream_id);
                self.queue_terminal_reset(frame)?;
            }
            return Ok(());
        }
        self.in_flight.observe_event(event)
    }

    fn rollback_stream(&mut self, stream_id: u64) {
        self.streams.streams.remove(&stream_id);
        self.in_flight.requests.remove(&stream_id);
        self.extracted_streams.remove(&stream_id);
        self.forget_crossing_receive_allowance(stream_id);
        self.scheduler.drop_stream(stream_id);
        let _ = self.receive_flow.drop_stream(stream_id);
        self.forget_pending_receive_credit(stream_id);
        self.retire_closed_stream(stream_id);
    }

    fn retire_closed_stream(&mut self, stream_id: u64) {
        if stream_id == 0 {
            return;
        }
        self.stream_tombstones.remove(&stream_id);
        let index = usize::from(stream_id.is_multiple_of(2));
        self.closed_stream_watermarks[index] = self.closed_stream_watermarks[index].max(stream_id);
    }

    fn queue_terminal_reset(&mut self, frame: Frame) -> io::Result<()> {
        let stream_id = frame.stream_id;
        self.register_crossing_receive_allowance(
            stream_id,
            self.receive_flow.stream_window(stream_id),
        );
        self.extracted_streams.remove(&stream_id);
        self.locally_ended_on_wire.remove(&stream_id);
        self.stream_tombstones
            .entry(stream_id)
            .or_insert(StreamTombstone::Reset);
        self.scheduler.drop_stream(stream_id);
        let mut receive_flow = self.receive_flow.clone();
        let released = receive_flow.drop_stream(stream_id)?;
        let mut frames = vec![frame];
        if released > 0 {
            frames.push(window_update_frame(0, released)?.with_priority(Priority::UserInput));
        }
        self.scheduler.enqueue_batch(frames)?;
        self.receive_flow = receive_flow;
        self.forget_pending_receive_credit(stream_id);
        Ok(())
    }

    fn discard_stream_state(&mut self, stream_id: u64) -> io::Result<()> {
        self.in_flight.requests.remove(&stream_id);
        self.extracted_streams.remove(&stream_id);
        self.locally_ended_on_wire.remove(&stream_id);
        self.forget_crossing_receive_allowance(stream_id);
        self.retire_closed_stream(stream_id);
        self.scheduler.drop_stream(stream_id);
        self.release_receive_stream(stream_id)
    }

    fn release_receive_stream(&mut self, stream_id: u64) -> io::Result<()> {
        let mut receive_flow = self.receive_flow.clone();
        let released = receive_flow.drop_stream(stream_id)?;
        if released > 0 {
            self.scheduler
                .enqueue(window_update_frame(0, released)?.with_priority(Priority::UserInput))?;
        }
        self.receive_flow = receive_flow;
        self.forget_pending_receive_credit(stream_id);
        Ok(())
    }

    fn forget_pending_receive_credit(&mut self, stream_id: u64) {
        let Some(credit) = self.pending_receive_credit.remove(&stream_id) else {
            return;
        };
        debug_assert!(self.pending_receive_connection_credit >= credit);
        self.pending_receive_connection_credit = self
            .pending_receive_connection_credit
            .saturating_sub(credit);
    }

    fn ensure_open_stream(&self, stream_id: u64) -> io::Result<()> {
        match self.streams.get(stream_id).map(|entry| entry.state) {
            Some(StreamState::Open | StreamState::HalfClosedRemote) => Ok(()),
            Some(StreamState::HalfClosedLocal | StreamState::Closed) => Err(protocol_error(
                format!("local side already closed v5 stream {stream_id}"),
            )),
            None => Err(protocol_error(format!("unknown v5 stream {stream_id}"))),
        }
    }

    fn ensure_peer_accepts_new_stream(&self) -> io::Result<()> {
        if self.terminated {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "v5 protocol session is terminated",
            ));
        }
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
        let last_accepted_stream_id = goaway.last_accepted_stream_id;
        if frame.stream_id == 0
            || self.streams.get(frame.stream_id).is_some()
            || frame.frame_type == FrameType::ResetStream
        {
            return Ok(None);
        }
        if stream_id_initiator(frame.stream_id)? != remote_initiator(self.streams.local_initiator) {
            return Ok(None);
        }

        self.stream_tombstones
            .entry(frame.stream_id)
            .or_insert(StreamTombstone::Reset);
        self.register_crossing_receive_allowance(
            frame.stream_id,
            self.receive_flow.stream_window(frame.stream_id),
        );
        self.scheduler.enqueue(
            reset_stream_frame(
                frame.stream_id,
                RESET_UNAVAILABLE,
                format!(
                    "stream rejected after GOAWAY; last accepted stream was {}",
                    last_accepted_stream_id
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
    inbound_frame_sequence: InboundFrameSequence,
    next_frame_sequence: u64,
}

pub struct FramedIoParts<R, W> {
    pub reader: R,
    pub writer: W,
    pub limits: FrameLimits,
    pub inbound_frame_sequence: InboundFrameSequence,
    pub next_frame_sequence: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InboundFrameSequence {
    next_expected: u64,
}

impl Default for InboundFrameSequence {
    fn default() -> Self {
        Self { next_expected: 1 }
    }
}

impl InboundFrameSequence {
    pub fn read_frame<R: Read>(
        &mut self,
        reader: &mut R,
        limits: FrameLimits,
    ) -> io::Result<Option<Frame>> {
        read_frame_with_limits_and_sequence(reader, limits, self)
    }

    fn validated_next(&self, actual: u64) -> io::Result<u64> {
        if actual != self.next_expected {
            return Err(protocol_error(format!(
                "invalid v5 frame sequence: expected {}, got {}",
                self.next_expected, actual
            )));
        }
        self.next_expected
            .checked_add(1)
            .ok_or_else(|| protocol_error("v5 inbound frame sequence exhausted"))
    }

    #[cfg(test)]
    fn next_expected(&self) -> u64 {
        self.next_expected
    }
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
            inbound_frame_sequence: InboundFrameSequence::default(),
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
            inbound_frame_sequence: self.inbound_frame_sequence,
            next_frame_sequence: self.next_frame_sequence,
        }
    }
}

impl<R: Read, W: Write> FramedIo<R, W> {
    pub fn read_frame(&mut self) -> io::Result<Option<Frame>> {
        self.inbound_frame_sequence
            .read_frame(&mut self.reader, self.limits)
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

    /// Assigns consecutive frame sequences and writes the batch with one final flush.
    pub fn write_frame_batch(&mut self, frames: &mut [Frame]) -> io::Result<()> {
        for frame in frames.iter() {
            validate_frame_for_write(frame, self.limits)?;
        }
        let frame_count = u64::try_from(frames.len())
            .map_err(|_| io::Error::other("v5 frame batch length exceeds u64"))?;
        let next_frame_sequence = self
            .next_frame_sequence
            .checked_add(frame_count)
            .ok_or_else(|| io::Error::other("v5 frame sequence exhausted"))?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            frame.frame_sequence = self.next_frame_sequence + offset as u64;
        }
        self.next_frame_sequence = next_frame_sequence;
        write_frame_batch_with_limits(&mut self.writer, frames, self.limits)
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

pub fn write_frame_unflushed<W: Write>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    write_frame_unflushed_with_limits(writer, frame, FrameLimits::default())
}

pub fn write_frame_batch<W: Write>(writer: &mut W, frames: &[Frame]) -> io::Result<()> {
    write_frame_batch_with_limits(writer, frames, FrameLimits::default())
}

pub fn write_frame_with_limits<W: Write>(
    writer: &mut W,
    frame: &Frame,
    limits: FrameLimits,
) -> io::Result<()> {
    write_frame_unflushed_with_limits(writer, frame, limits)?;
    writer.flush()
}

/// Validates and writes one frame without flushing the writer.
///
/// This supports writer loops that revalidate extracted frames against a `ProtocolSession`
/// immediately before each write and flush once after the surviving batch.
pub fn write_frame_unflushed_with_limits<W: Write>(
    writer: &mut W,
    frame: &Frame,
    limits: FrameLimits,
) -> io::Result<()> {
    validate_frame_for_write(frame, limits)?;
    write_frame_parts(writer, frame)
}

/// Validates the complete batch before writing and flushes once after its final frame.
pub fn write_frame_batch_with_limits<W: Write>(
    writer: &mut W,
    frames: &[Frame],
    limits: FrameLimits,
) -> io::Result<()> {
    for frame in frames {
        validate_frame_for_write(frame, limits)?;
    }
    if frames.is_empty() {
        return Ok(());
    }
    for frame in frames {
        write_frame_parts(writer, frame)?;
    }
    writer.flush()
}

fn validate_frame_for_write(frame: &Frame, limits: FrameLimits) -> io::Result<()> {
    Priority::from_wire(frame.priority)?;
    validate_lengths(frame.control.len(), frame.body.len(), limits)?;
    validate_frame_shape(
        frame.frame_type,
        frame.flags,
        frame.stream_id,
        frame.control.len(),
        frame.body.len(),
    )
}

fn write_frame_parts<W: Write>(writer: &mut W, frame: &Frame) -> io::Result<()> {
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
    writer.write_all(&frame.body)
}

pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Frame>> {
    read_frame_with_limits(reader, FrameLimits::default())
}

pub fn read_frame_with_limits<R: Read>(
    reader: &mut R,
    limits: FrameLimits,
) -> io::Result<Option<Frame>> {
    read_frame_with_limits_inner(reader, limits, None)
}

fn read_frame_with_limits_and_sequence<R: Read>(
    reader: &mut R,
    limits: FrameLimits,
    sequence: &mut InboundFrameSequence,
) -> io::Result<Option<Frame>> {
    read_frame_with_limits_inner(reader, limits, Some(sequence))
}

fn read_frame_with_limits_inner<R: Read>(
    reader: &mut R,
    limits: FrameLimits,
    mut sequence: Option<&mut InboundFrameSequence>,
) -> io::Result<Option<Frame>> {
    let mut fixed = [0_u8; FRAME_HEADER_LEN];
    loop {
        match reader.read(&mut fixed[..1]) {
            Ok(0) => return Ok(None),
            Ok(1) => {
                reader.read_exact(&mut fixed[1..])?;
                break;
            }
            Ok(_) => unreachable!("read buffer length is one byte"),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
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

    if fixed[11] != 0 || fixed[36..48].iter().any(|byte| *byte != 0) {
        return Err(protocol_error(
            "v5 frame reserved header bytes must be zero",
        ));
    }

    let frame_type = FrameType::try_from(u16::from_be_bytes([fixed[6], fixed[7]]))?;
    let flags = u16::from_be_bytes([fixed[8], fixed[9]]);
    let priority = fixed[10];
    Priority::from_wire(priority)?;
    let stream_id = u64::from_be_bytes([
        fixed[12], fixed[13], fixed[14], fixed[15], fixed[16], fixed[17], fixed[18], fixed[19],
    ]);
    let frame_sequence = u64::from_be_bytes([
        fixed[20], fixed[21], fixed[22], fixed[23], fixed[24], fixed[25], fixed[26], fixed[27],
    ]);
    let next_inbound_sequence = sequence
        .as_ref()
        .map(|sequence| sequence.validated_next(frame_sequence))
        .transpose()?;
    let control_len = u32::from_be_bytes([fixed[28], fixed[29], fixed[30], fixed[31]]);
    let body_len = u32::from_be_bytes([fixed[32], fixed[33], fixed[34], fixed[35]]);

    validate_lengths(control_len as usize, body_len as usize, limits)?;
    validate_frame_shape(
        frame_type,
        flags,
        stream_id,
        control_len as usize,
        body_len as usize,
    )?;

    let mut control = vec![0_u8; control_len as usize];
    reader.read_exact(&mut control)?;
    let mut body = vec![0_u8; body_len as usize];
    reader.read_exact(&mut body)?;

    if let (Some(sequence), Some(next_inbound_sequence)) =
        (sequence.as_mut(), next_inbound_sequence)
    {
        sequence.next_expected = next_inbound_sequence;
    }

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

fn validate_frame_shape(
    frame_type: FrameType,
    flags: u16,
    stream_id: u64,
    control_len: usize,
    body_len: usize,
) -> io::Result<()> {
    if flags != 0 {
        return Err(protocol_error(format!(
            "unknown v5 frame flags for {frame_type:?}: {flags:#06x}"
        )));
    }

    match frame_type {
        FrameType::Hello
        | FrameType::Settings
        | FrameType::SettingsAck
        | FrameType::Ping
        | FrameType::Pong
        | FrameType::GoAway => {
            if stream_id != 0 {
                return Err(protocol_error(format!(
                    "{frame_type:?} must use v5 stream 0"
                )));
            }
        }
        FrameType::Headers | FrameType::Data | FrameType::EndStream | FrameType::ResetStream => {
            if stream_id == 0 {
                return Err(protocol_error(format!(
                    "{frame_type:?} requires a non-zero v5 stream id"
                )));
            }
        }
        FrameType::WindowUpdate => {}
    }

    match frame_type {
        FrameType::SettingsAck | FrameType::EndStream if control_len != 0 || body_len != 0 => {
            Err(protocol_error(format!(
                "{frame_type:?} must not carry v5 control or body bytes"
            )))
        }
        FrameType::Data | FrameType::SettingsAck | FrameType::EndStream => Ok(()),
        _ if body_len != 0 => Err(protocol_error(format!(
            "{frame_type:?} must not carry v5 body bytes"
        ))),
        _ => Ok(()),
    }
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

    pub fn from_wire(priority: u8) -> io::Result<Self> {
        Self::try_from(i32::from(priority))
            .map_err(|_| protocol_error(format!("unknown v5 priority: {priority}")))
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
        let min_unsolicited_ping_interval_ms = nonzero_or(
            desired.min_unsolicited_ping_interval_ms,
            MIN_UNSOLICITED_PING_INTERVAL_MS,
        )
        .max(MIN_UNSOLICITED_PING_INTERVAL_MS);
        let idle_ping_interval_ms =
            nonzero_or(desired.idle_ping_interval_ms, IDLE_PING_INTERVAL_MS)
                .max(min_unsolicited_ping_interval_ms);
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
            idle_ping_interval_ms,
            ping_timeout_ms: nonzero_or(desired.ping_timeout_ms, PING_TIMEOUT_MS),
            min_unsolicited_ping_interval_ms,
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
    if server_hello.accepted_settings.as_ref() != Some(&settings) {
        return Err(protocol_error(
            "v5 SETTINGS does not match ServerHello.accepted_settings",
        ));
    }
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
    let requested = client_requested_capabilities(client);
    if let Some(capability) = server
        .capabilities
        .iter()
        .find(|capability| !requested.contains(capability))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "UNSUPPORTED_CAPABILITY: server accepted unrequested v5 capability: {capability}"
            ),
        ));
    }
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
    use std::io::{Cursor, Read, Write};

    struct InterruptedOnce<R> {
        inner: R,
        interrupted: bool,
    }

    impl<R: Read> Read for InterruptedOnce<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "retry"));
            }
            self.inner.read(buffer)
        }
    }

    #[derive(Default)]
    struct ObservedWriter {
        bytes: Vec<u8>,
        flush_count: usize,
        fail_after: Option<usize>,
    }

    impl ObservedWriter {
        fn failing_after(byte_count: usize) -> Self {
            Self {
                fail_after: Some(byte_count),
                ..Self::default()
            }
        }
    }

    impl Write for ObservedWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if let Some(fail_after) = self.fail_after {
                let remaining = fail_after.saturating_sub(self.bytes.len());
                if remaining == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "injected batch write failure",
                    ));
                }
                let written = remaining.min(buffer.len());
                self.bytes.extend_from_slice(&buffer[..written]);
                return Ok(written);
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flush_count += 1;
            Ok(())
        }
    }

    fn encode_sequenced_frames(frames: impl IntoIterator<Item = Frame>) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (index, mut frame) in frames.into_iter().enumerate() {
            frame.frame_sequence = u64::try_from(index).unwrap() + 1;
            write_frame(&mut bytes, &frame).unwrap();
        }
        bytes
    }

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
        let mut frame = Frame::new(FrameType::Data, 0x0102_0304_0506_0708);
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
            FrameType::Data as u16
        );
        assert_eq!(u16::from_be_bytes([bytes[8], bytes[9]]), 0);
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
    fn frame_batch_writer_flushes_once_and_preserves_frames() {
        let frames = vec![
            Frame::new(FrameType::Headers, 1).with_priority(Priority::ForegroundDocument),
            data_frame(1, 3).with_priority(Priority::ForegroundDocument),
            Frame::new(FrameType::EndStream, 1).with_priority(Priority::ForegroundDocument),
        ];
        let mut writer = ObservedWriter::default();

        write_frame_batch(&mut writer, &frames).unwrap();

        assert_eq!(writer.flush_count, 1);
        let mut reader = Cursor::new(writer.bytes);
        for expected in frames {
            assert_eq!(read_frame(&mut reader).unwrap(), Some(expected));
        }
        assert!(read_frame(&mut reader).unwrap().is_none());
    }

    #[test]
    fn frame_batch_writer_validates_every_frame_before_writing() {
        let limits = FrameLimits {
            max_control_len: DEFAULT_MAX_CONTROL_LEN,
            max_body_len: 2,
        };
        let frames = vec![data_frame(1, 2), data_frame(3, 3)];
        let mut writer = ObservedWriter::default();

        let error = write_frame_batch_with_limits(&mut writer, &frames, limits).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(writer.bytes.is_empty());
        assert_eq!(writer.flush_count, 0);
    }

    #[test]
    fn frame_batch_writer_propagates_partial_write_without_flushing() {
        let frames = vec![
            Frame::new(FrameType::Headers, 1).with_priority(Priority::Background),
            Frame::new(FrameType::EndStream, 1).with_priority(Priority::Background),
        ];
        let mut writer = ObservedWriter::failing_after(FRAME_HEADER_LEN + 8);

        let error = write_frame_batch(&mut writer, &frames).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(writer.bytes.len(), FRAME_HEADER_LEN + 8);
        assert_eq!(writer.flush_count, 0);
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
        bytes[0..4].copy_from_slice(b"BAD!");

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
    fn frame_reader_retries_interrupted_first_read() {
        let frame = Frame::new(FrameType::Ping, 0);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        let mut reader = InterruptedOnce {
            inner: Cursor::new(bytes),
            interrupted: false,
        };

        assert_eq!(read_frame(&mut reader).unwrap(), Some(frame));
    }

    #[test]
    fn frame_reader_rejects_nonzero_reserved_header_bytes() {
        let frame = Frame::new(FrameType::Ping, 0);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        bytes[36] = 1;

        let error = read_frame(&mut Cursor::new(bytes)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("reserved header bytes"));
    }

    #[test]
    fn frame_codec_rejects_malformed_frame_shapes() {
        struct Case {
            name: String,
            frame: Frame,
            expected_diagnostic: &'static str,
        }

        let mut cases = Vec::new();

        let mut flagged = Frame::new(FrameType::Ping, 0);
        flagged.flags = 1;
        cases.push(Case {
            name: "unknown flags".to_string(),
            frame: flagged,
            expected_diagnostic: "unknown v5 frame flags",
        });

        let mut invalid_priority = Frame::new(FrameType::Ping, 0);
        invalid_priority.priority = u8::MAX;
        cases.push(Case {
            name: "unknown priority".to_string(),
            frame: invalid_priority,
            expected_diagnostic: "unknown v5 priority",
        });

        for frame_type in [
            FrameType::Hello,
            FrameType::Settings,
            FrameType::SettingsAck,
            FrameType::Ping,
            FrameType::Pong,
            FrameType::GoAway,
        ] {
            cases.push(Case {
                name: format!("{frame_type:?} on a stream"),
                frame: Frame::new(frame_type, 1),
                expected_diagnostic: "must use v5 stream 0",
            });
        }

        for frame_type in [
            FrameType::Headers,
            FrameType::Data,
            FrameType::EndStream,
            FrameType::ResetStream,
        ] {
            cases.push(Case {
                name: format!("{frame_type:?} on stream zero"),
                frame: Frame::new(frame_type, 0),
                expected_diagnostic: "requires a non-zero v5 stream id",
            });
        }

        for (frame_type, stream_id) in [
            (FrameType::Hello, 0),
            (FrameType::Settings, 0),
            (FrameType::Headers, 1),
            (FrameType::ResetStream, 1),
            (FrameType::WindowUpdate, 0),
            (FrameType::Ping, 0),
            (FrameType::Pong, 0),
            (FrameType::GoAway, 0),
        ] {
            let mut frame = Frame::new(frame_type, stream_id);
            frame.body.push(1);
            cases.push(Case {
                name: format!("{frame_type:?} with a body"),
                frame,
                expected_diagnostic: "must not carry v5 body bytes",
            });
        }

        for (frame_type, stream_id) in [(FrameType::SettingsAck, 0), (FrameType::EndStream, 1)] {
            let mut with_control = Frame::new(frame_type, stream_id);
            with_control.control.push(1);
            cases.push(Case {
                name: format!("{frame_type:?} with control"),
                frame: with_control,
                expected_diagnostic: "must not carry v5 control or body bytes",
            });

            let mut with_body = Frame::new(frame_type, stream_id);
            with_body.body.push(1);
            cases.push(Case {
                name: format!("{frame_type:?} with a body"),
                frame: with_body,
                expected_diagnostic: "must not carry v5 control or body bytes",
            });
        }

        for case in cases {
            let write_error = match write_frame(&mut Vec::new(), &case.frame) {
                Ok(()) => panic!("writer accepted malformed case: {}", case.name),
                Err(error) => error,
            };
            assert_eq!(
                write_error.kind(),
                io::ErrorKind::InvalidData,
                "{}",
                case.name
            );
            assert!(
                write_error.to_string().contains(case.expected_diagnostic),
                "{}: {write_error}",
                case.name
            );

            let mut wire = Vec::new();
            write_frame_parts(&mut wire, &case.frame).unwrap();
            let read_error = match read_frame(&mut Cursor::new(wire)) {
                Ok(_) => panic!("reader accepted malformed case: {}", case.name),
                Err(error) => error,
            };
            assert_eq!(
                read_error.kind(),
                io::ErrorKind::InvalidData,
                "{}",
                case.name
            );
            assert!(
                read_error.to_string().contains(case.expected_diagnostic),
                "{}: {read_error}",
                case.name
            );
        }
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
    fn framed_io_validates_contiguous_incoming_sequences_across_split() {
        let bytes = encode_sequenced_frames([
            Frame::new(FrameType::Ping, 0),
            Frame::new(FrameType::Pong, 0),
            Frame::new(FrameType::Ping, 0),
        ]);
        let mut io = FramedIo::new(Cursor::new(bytes), Vec::new());

        assert_eq!(io.read_frame().unwrap().unwrap().frame_sequence, 1);
        assert_eq!(io.read_frame().unwrap().unwrap().frame_sequence, 2);
        let mut parts = io.into_parts();

        assert_eq!(parts.inbound_frame_sequence.next_expected(), 3);
        assert_eq!(
            parts
                .inbound_frame_sequence
                .read_frame(&mut parts.reader, parts.limits)
                .unwrap()
                .unwrap()
                .frame_sequence,
            3
        );
        assert_eq!(parts.inbound_frame_sequence.next_expected(), 4);
    }

    #[test]
    fn framed_io_rejects_zero_duplicate_and_gapped_incoming_sequences() {
        for (sequences, expected, actual) in
            [(vec![0], 1, 0), (vec![1, 1], 2, 1), (vec![1, 3], 2, 3)]
        {
            let mut bytes = Vec::new();
            for (index, sequence) in sequences.iter().copied().enumerate() {
                let mut frame = Frame::new(
                    if index.is_multiple_of(2) {
                        FrameType::Ping
                    } else {
                        FrameType::Pong
                    },
                    0,
                );
                frame.frame_sequence = sequence;
                write_frame(&mut bytes, &frame).unwrap();
            }
            let mut io = FramedIo::new(Cursor::new(bytes), Vec::new());

            let error = loop {
                match io.read_frame() {
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("sequence case ended without an error: {sequences:?}"),
                    Err(error) => break error,
                }
            };

            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(
                error
                    .to_string()
                    .contains(&format!("expected {expected}, got {actual}")),
                "{error}"
            );
        }
    }

    #[test]
    fn inbound_sequence_advances_only_after_a_complete_frame() {
        let mut frame = Frame::from_control(
            FrameType::Ping,
            0,
            &PingPayload {
                token: b"complete".to_vec(),
            },
        );
        frame.frame_sequence = 1;
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        let truncated = &bytes[..bytes.len() - 1];
        let mut sequence = InboundFrameSequence::default();

        let error = sequence
            .read_frame(&mut Cursor::new(truncated), FrameLimits::default())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(sequence.next_expected(), 1);

        let decoded = sequence
            .read_frame(&mut Cursor::new(bytes), FrameLimits::default())
            .unwrap()
            .unwrap();
        assert_eq!(decoded.frame_sequence, 1);
        assert_eq!(sequence.next_expected(), 2);
    }

    #[test]
    fn framed_io_batch_assigns_sequences_and_flushes_once() {
        let mut io = FramedIo::new(Cursor::new(Vec::new()), ObservedWriter::default());
        let mut frames = [
            Frame::new(FrameType::Ping, 0),
            Frame::new(FrameType::Pong, 0),
        ];

        io.write_frame_batch(&mut frames).unwrap();
        let parts = io.into_parts();

        assert_eq!(frames[0].frame_sequence, 1);
        assert_eq!(frames[1].frame_sequence, 2);
        assert_eq!(parts.next_frame_sequence, 3);
        assert_eq!(parts.writer.flush_count, 1);
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
    fn frame_flow_control_len_counts_decoded_data_bytes() {
        let mut data = Frame::from_control(
            FrameType::Data,
            3,
            &DataEnvelope {
                channel: DataChannel::FileBody as i32,
                uncompressed_len: 100,
            },
        );
        data.body = vec![0; 10];
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
    fn frame_control_budget_len_exempts_data_and_urgent_control() {
        let mut ping = Frame::new(FrameType::Ping, 0);
        ping.control = b"abc".to_vec();
        let mut headers = Frame::new(FrameType::Headers, 1);
        headers.control = b"request".to_vec();
        let mut data = data_frame(1, 100);
        data.control = b"metadata".to_vec();

        assert_eq!(ping.control_budget_len(), 0);
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
        assert_eq!(window.available(), 3);
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
    fn stream_table_preserves_opening_priority() {
        let mut table =
            StreamTable::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        let opening = headers_frame(1, StreamEnvelope::request(1, "fs.read"))
            .with_priority(Priority::ForegroundDocument);

        table.route_incoming(&opening).unwrap();

        assert_eq!(
            table.get(1).map(|entry| entry.priority),
            Some(Priority::ForegroundDocument)
        );
    }

    #[test]
    fn stream_table_rejects_unknown_opening_priority() {
        let mut table =
            StreamTable::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        let mut opening = headers_frame(1, StreamEnvelope::request(1, "fs.read"));
        opening.priority = u8::MAX;

        let error = table.route_incoming(&opening).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unknown v5 priority"));
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
    fn stream_table_rejects_non_monotonic_remote_stream_ids() {
        let mut table =
            StreamTable::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        table
            .route_incoming(&headers_frame(3, StreamEnvelope::request(3, "fs.stat")))
            .unwrap();
        table.route_incoming(&reset_frame(3)).unwrap();

        let error = table
            .route_incoming(&headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap_err();

        assert!(error.to_string().contains("does not increase past 3"));
        assert_eq!(table.last_accepted_remote_stream_id(), 3);
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
        assert_eq!(client.queued_len(), 3);
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
        let settings = ConnectionSettings {
            initial_stream_window: 10,
            initial_connection_window: 20,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
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
    fn protocol_session_aggregates_receive_credit_until_threshold() {
        let settings = ConnectionSettings {
            initial_stream_window: 8,
            initial_connection_window: 12,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();

        for _ in 0..3 {
            session.receive_frame(data_frame(1, 1)).unwrap();
            session.acknowledge_data(1, 1).unwrap();
            assert!(session.pop_next_frame().unwrap().is_none());
        }
        assert_eq!(session.receive_stream_window(1), 5);
        assert_eq!(session.receive_connection_window(), 9);

        session.receive_frame(data_frame(1, 1)).unwrap();
        session.acknowledge_data(1, 1).unwrap();

        let connection_update = session.pop_next_frame().unwrap().unwrap();
        let stream_update = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(connection_update.stream_id, 0);
        assert_eq!(stream_update.stream_id, 1);
        assert_eq!(
            connection_update
                .decode_control::<WindowUpdate>()
                .unwrap()
                .credit_bytes,
            4
        );
        assert_eq!(
            stream_update
                .decode_control::<WindowUpdate>()
                .unwrap()
                .credit_bytes,
            4
        );
        assert!(session.pop_next_frame().unwrap().is_none());
        assert_eq!(session.receive_stream_window(1), 8);
        assert_eq!(session.receive_connection_window(), 12);
    }

    #[test]
    fn protocol_session_aggregates_connection_credit_across_streams() {
        let settings = ConnectionSettings {
            initial_stream_window: 8,
            initial_connection_window: 8,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        for stream_id in [1, 3] {
            session
                .receive_frame(headers_frame(
                    stream_id,
                    StreamEnvelope::request(stream_id, "fs.write"),
                ))
                .unwrap();
            session.receive_frame(data_frame(stream_id, 2)).unwrap();
            session.acknowledge_data(stream_id, 2).unwrap();
        }

        let updates = [
            session.pop_next_frame().unwrap().unwrap(),
            session.pop_next_frame().unwrap().unwrap(),
            session.pop_next_frame().unwrap().unwrap(),
        ];
        assert_eq!(
            updates
                .iter()
                .map(|frame| frame.stream_id)
                .collect::<Vec<_>>(),
            [0, 1, 3]
        );
        assert_eq!(
            updates
                .iter()
                .map(|frame| { frame.decode_control::<WindowUpdate>().unwrap().credit_bytes })
                .collect::<Vec<_>>(),
            [4, 2, 2]
        );
        assert_eq!(session.receive_connection_window(), 8);
        assert_eq!(session.receive_stream_window(1), 8);
        assert_eq!(session.receive_stream_window(3), 8);
    }

    #[test]
    fn protocol_session_releases_residual_receive_credit_on_close_and_reset() {
        let settings = ConnectionSettings {
            initial_stream_window: 8,
            initial_connection_window: 12,
            ..ConnectionSettings::recommended()
        };

        let mut client = ProtocolSession::new(StreamInitiator::Client, &settings);
        let client_stream = client
            .open_unary_request("fs.read", RequestOptions::default())
            .unwrap();
        while client.pop_next_frame().unwrap().is_some() {}
        client
            .receive_frame(headers_frame(
                client_stream,
                StreamEnvelope::response(
                    client_stream,
                    "fs.read",
                    MessageRole::FinalResponse,
                    true,
                ),
            ))
            .unwrap();
        client.receive_frame(data_frame(client_stream, 2)).unwrap();
        client.acknowledge_data(client_stream, 2).unwrap();
        assert!(client.pop_next_frame().unwrap().is_none());

        client
            .receive_frame(Frame::new(FrameType::EndStream, client_stream))
            .unwrap();
        let close_update = client.pop_next_frame().unwrap().unwrap();
        assert_eq!(close_update.stream_id, 0);
        assert_eq!(
            close_update
                .decode_control::<WindowUpdate>()
                .unwrap()
                .credit_bytes,
            2
        );
        assert!(client.pop_next_frame().unwrap().is_none());
        assert!(client.pending_receive_credit.is_empty());
        assert_eq!(client.pending_receive_connection_credit, 0);

        let mut server = ProtocolSession::new(StreamInitiator::Server, &settings);
        server
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();
        server.receive_frame(data_frame(1, 2)).unwrap();
        server.acknowledge_data(1, 2).unwrap();
        assert!(server.pop_next_frame().unwrap().is_none());

        server.receive_frame(reset_frame(1)).unwrap();
        let reset_update = server.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset_update.stream_id, 0);
        assert_eq!(
            reset_update
                .decode_control::<WindowUpdate>()
                .unwrap()
                .credit_bytes,
            2
        );
        assert!(server.pop_next_frame().unwrap().is_none());
        assert!(server.pending_receive_credit.is_empty());
        assert_eq!(server.pending_receive_connection_credit, 0);
    }

    #[test]
    fn protocol_session_aggregated_credit_resumes_blocked_sender() {
        let settings = ConnectionSettings {
            initial_stream_window: 4,
            initial_connection_window: 6,
            max_frame_body: 2,
            ..ConnectionSettings::recommended()
        };
        let mut sender = ProtocolSession::new(StreamInitiator::Client, &settings);
        let mut receiver = ProtocolSession::new(StreamInitiator::Server, &settings);
        let stream_id = sender
            .open_request_with_body(
                "fs.write",
                RequestOptions::default(),
                DataChannel::FileBody,
                b"abcdef",
            )
            .unwrap();

        let headers = sender.pop_next_frame().unwrap().unwrap();
        receiver.receive_frame(headers).unwrap();
        let first = sender.pop_next_frame().unwrap().unwrap();
        assert_eq!(first.body, b"ab");
        let first_credit = receiver
            .receive_frame(first)
            .unwrap()
            .data_credit()
            .unwrap();
        receiver
            .acknowledge_data(first_credit.0, first_credit.1)
            .unwrap();
        let updates = [
            receiver.pop_next_frame().unwrap().unwrap(),
            receiver.pop_next_frame().unwrap().unwrap(),
        ];

        let second = sender.pop_next_frame().unwrap().unwrap();
        assert_eq!(second.body, b"cd");
        assert!(sender.pop_next_frame().unwrap().is_none());

        for update in updates {
            sender.receive_frame(update).unwrap();
        }
        let resumed = sender.pop_next_frame().unwrap().unwrap();
        assert_eq!(resumed.frame_type, FrameType::Data);
        assert_eq!(resumed.stream_id, stream_id);
        assert_eq!(resumed.body, b"ef");
    }

    #[test]
    fn protocol_session_terminate_clears_pending_receive_credit() {
        let settings = ConnectionSettings {
            initial_stream_window: 8,
            initial_connection_window: 12,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();
        session.receive_frame(data_frame(1, 2)).unwrap();
        session.acknowledge_data(1, 2).unwrap();
        assert_eq!(session.pending_receive_credit.get(&1), Some(&2));

        session.terminate();

        assert!(session.pending_receive_credit.is_empty());
        assert_eq!(session.pending_receive_connection_credit, 0);
    }

    #[test]
    fn protocol_session_enforces_receive_connection_and_stream_windows() {
        let settings = ConnectionSettings {
            initial_stream_window: 4,
            initial_connection_window: 6,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();

        session.receive_frame(data_frame(1, 4)).unwrap();
        assert_eq!(session.receive_stream_window(1), 0);
        assert_eq!(session.receive_connection_window(), 2);

        let error = session.receive_frame(data_frame(1, 1)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds flow-control credit"));
        assert_eq!(session.receive_stream_window(1), 0);
        assert_eq!(session.receive_connection_window(), 2);

        session.acknowledge_data(1, 4).unwrap();
        assert_eq!(session.receive_stream_window(1), 4);
        assert_eq!(session.receive_connection_window(), 6);
        assert!(session.acknowledge_data(1, 1).is_err());
    }

    #[test]
    fn protocol_session_rejects_data_over_connection_receive_window() {
        let settings = ConnectionSettings {
            initial_stream_window: 8,
            initial_connection_window: 3,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();

        let error = session.receive_frame(data_frame(1, 4)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("connection has 3"));
        assert_eq!(session.receive_connection_window(), 3);
        assert_eq!(session.receive_stream_window(1), 8);
    }

    #[test]
    fn protocol_session_rejects_unknown_and_zero_window_updates_without_state_growth() {
        let settings = ConnectionSettings {
            initial_stream_window: 3,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Client, &settings);

        let error = session
            .receive_frame(window_update_frame(99, 10).unwrap())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unknown v5 stream 99"));
        assert_eq!(session.stream_window(99), 3);

        let zero = Frame::from_control(
            FrameType::WindowUpdate,
            0,
            &WindowUpdate { credit_bytes: 0 },
        );
        let error = session.receive_frame(zero).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("greater than zero"));
    }

    #[test]
    fn protocol_session_cancel_purges_flow_blocked_frames_before_reset() {
        let settings = ConnectionSettings {
            initial_stream_window: 3,
            initial_connection_window: 64,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Client, &settings);
        let stream_id = session
            .open_request_with_body(
                "fs.write",
                RequestOptions::default(),
                DataChannel::FileBody,
                b"abcd",
            )
            .unwrap();

        assert_eq!(
            session.pop_next_frame().unwrap().unwrap().frame_type,
            FrameType::Headers
        );
        let extracted_data = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(extracted_data.body, b"abc");
        assert!(session.pop_next_frame().unwrap().is_none());
        assert_eq!(session.queued_len(), 2);
        assert_eq!(session.scheduler.pending_data_frames(stream_id), 0);

        assert!(
            session
                .cancel_stream(stream_id, RESET_CANCELLED, "cancelled")
                .unwrap()
        );
        assert_eq!(session.queued_len(), 1);
        assert_eq!(session.scheduler.pending_data_frames(stream_id), 0);
        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.frame_type, FrameType::ResetStream);
        assert_eq!(reset.stream_id, stream_id);
        session.discard_unwritten_frame(&extracted_data).unwrap();
        assert!(session.pop_next_frame().unwrap().is_none());
        assert_eq!(session.stream_window(stream_id), 3);

        let next_stream = session
            .open_unary_request("fs.stat", RequestOptions::default())
            .unwrap();
        assert_eq!(next_stream, 3);
        assert_eq!(
            session.pop_next_frame().unwrap().unwrap().frame_type,
            FrameType::Headers
        );
    }

    #[test]
    fn protocol_session_peer_reset_invalidates_extracted_frames_permanently() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let stream_id = session
            .open_request_with_body(
                "fs.write",
                RequestOptions::default(),
                DataChannel::FileBody,
                b"payload",
            )
            .unwrap();
        let mut extracted = Vec::new();
        while let Some(frame) = session.pop_next_frame().unwrap() {
            extracted.push(frame);
        }
        assert!(
            extracted
                .iter()
                .all(|frame| session.should_write_frame(frame))
        );

        session.receive_frame(reset_frame(stream_id)).unwrap();

        assert_eq!(
            session.stream_tombstone(stream_id),
            Some(StreamTombstone::Closed)
        );
        assert!(
            extracted
                .iter()
                .all(|frame| !session.should_write_frame(frame))
        );
        assert!(!session.should_write_frame(&reset_stream_frame(
            stream_id,
            RESET_CANCELLED,
            "acknowledge reset"
        )));
    }

    #[test]
    fn unflushed_writer_can_revalidate_extracted_frames_after_reset() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let stream_id = session
            .open_request_with_body(
                "fs.write",
                RequestOptions::default(),
                DataChannel::FileBody,
                b"payload",
            )
            .unwrap();
        let mut extracted = Vec::new();
        while let Some(frame) = session.pop_next_frame().unwrap() {
            extracted.push(frame);
        }
        let first = extracted.remove(0);
        let mut writer = ObservedWriter::default();

        assert!(session.should_write_frame(&first));
        write_frame_unflushed(&mut writer, &first).unwrap();
        session.observe_frame_written(&first);
        session.receive_frame(reset_frame(stream_id)).unwrap();
        for frame in &extracted {
            if session.should_write_frame(frame) {
                write_frame_unflushed(&mut writer, frame).unwrap();
                session.observe_frame_written(frame);
            } else {
                session.discard_unwritten_frame(frame).unwrap();
            }
        }
        writer.flush().unwrap();

        assert_eq!(writer.flush_count, 1);
        let mut reader = Cursor::new(writer.bytes);
        assert_eq!(read_frame(&mut reader).unwrap(), Some(first));
        assert!(read_frame(&mut reader).unwrap().is_none());
    }

    #[test]
    fn protocol_session_closes_only_after_end_stream_reaches_wire() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();
        session
            .receive_frame(Frame::new(FrameType::EndStream, 1))
            .unwrap();
        session
            .send_owned_data(1, DataChannel::FileBody, b"result".to_vec(), Priority::Bulk)
            .unwrap();
        assert_eq!(
            session.finish_stream(1, Priority::Background).unwrap(),
            StreamState::Closed
        );
        let data = session.pop_next_frame().unwrap().unwrap();
        let end = session.pop_next_frame().unwrap().unwrap();

        assert!(session.should_write_frame(&data));
        assert!(session.should_write_frame(&end));
        assert_eq!(session.stream_tombstone(1), None);
        session.observe_frame_written(&data);
        assert_eq!(session.stream_tombstone(1), None);
        session.observe_frame_written(&end);

        assert_eq!(session.stream_tombstone(1), Some(StreamTombstone::Closed));
        assert!(!session.should_write_frame(&data));
        assert!(!session.should_write_frame(&end));
    }

    #[test]
    fn closed_remote_request_keeps_send_window_state_until_large_response_reaches_wire() {
        let settings = ConnectionSettings {
            initial_stream_window: 4,
            initial_connection_window: 4,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::with_limits(
            StreamInitiator::Server,
            &settings,
            FrameLimits {
                max_control_len: DEFAULT_MAX_CONTROL_LEN,
                max_body_len: 8,
            },
        );
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();
        session
            .receive_frame(Frame::new(FrameType::EndStream, 1))
            .unwrap();
        session
            .send_owned_data(
                1,
                DataChannel::FileBody,
                b"abcdefgh".to_vec(),
                Priority::Bulk,
            )
            .unwrap();
        assert_eq!(
            session.finish_stream(1, Priority::Background).unwrap(),
            StreamState::Closed
        );

        let first = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(first.body, b"abcd");
        assert!(session.pop_next_frame().unwrap().is_none());
        assert_eq!(session.active_streams(), 0);

        session
            .receive_frame(window_update_frame(1, 4).unwrap())
            .unwrap();
        session
            .receive_frame(window_update_frame(0, 4).unwrap())
            .unwrap();
        let second = session.pop_next_frame().unwrap().unwrap();
        let end = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(second.body, b"efgh");
        assert_eq!(end.frame_type, FrameType::EndStream);

        session.observe_frame_written(&first);
        session.observe_frame_written(&second);
        session.observe_frame_written(&end);
        assert_eq!(session.active_streams(), 0);
        assert_eq!(session.stream_tombstone(1), Some(StreamTombstone::Closed));
    }

    #[test]
    fn discarded_extracted_data_refunds_connection_send_credit_after_reset() {
        let settings = ConnectionSettings {
            initial_stream_window: 8,
            initial_connection_window: 8,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Client, &settings);
        let stream_id = session
            .open_request_with_body(
                "fs.write",
                RequestOptions::default(),
                DataChannel::FileBody,
                b"data",
            )
            .unwrap();
        assert_eq!(
            session.pop_next_frame().unwrap().unwrap().frame_type,
            FrameType::Headers
        );
        let data = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(session.connection_window(), 4);

        assert!(
            session
                .reset_stream(stream_id, RESET_CANCELLED, "cancelled")
                .unwrap()
        );
        assert!(!session.should_write_frame(&data));
        session.discard_unwritten_frame(&data).unwrap();

        assert_eq!(session.connection_window(), 8);
        assert_eq!(session.stream_window(stream_id), 8);
    }

    #[test]
    fn tombstoned_stream_ignores_crossing_frames_and_next_stream_remains_usable() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();
        assert!(
            session
                .reset_stream(1, RESET_RESOURCE_EXHAUSTED, "request too large")
                .unwrap()
        );

        for crossing in [
            data_frame(1, 8),
            Frame::new(FrameType::EndStream, 1),
            window_update_frame(1, 8).unwrap(),
        ] {
            let event = session.receive_frame(crossing).unwrap();
            assert_eq!(event.routed, RoutedFrame::RejectedStream { stream_id: 1 });
            assert!(event.stream_event.is_none());
        }

        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.frame_type, FrameType::ResetStream);
        let connection_credit = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(connection_credit.frame_type, FrameType::WindowUpdate);
        assert_eq!(connection_credit.stream_id, 0);
        assert_eq!(
            connection_credit
                .decode_control::<WindowUpdate>()
                .unwrap()
                .credit_bytes,
            8
        );
        assert!(session.pop_next_frame().unwrap().is_none());

        let event = session
            .receive_frame(headers_frame(3, StreamEnvelope::request(3, "fs.stat")))
            .unwrap();
        assert!(matches!(
            event.stream_event,
            Some(StreamEvent::Headers { stream_id: 3, .. })
        ));
    }

    #[test]
    fn tombstoned_stream_rejects_crossing_data_beyond_residual_credit() {
        let settings = ConnectionSettings {
            initial_stream_window: 8,
            initial_connection_window: 8,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();
        assert!(
            session
                .reset_stream(1, RESET_RESOURCE_EXHAUSTED, "request too large")
                .unwrap()
        );

        session.receive_frame(data_frame(1, 5)).unwrap();
        session.receive_frame(data_frame(1, 3)).unwrap();
        let error = session.receive_frame(data_frame(1, 1)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("without residual credit"));

        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.frame_type, FrameType::ResetStream);
        let updates = [
            session.pop_next_frame().unwrap().unwrap(),
            session.pop_next_frame().unwrap().unwrap(),
        ];
        assert_eq!(
            updates
                .iter()
                .map(|frame| {
                    assert_eq!(frame.frame_type, FrameType::WindowUpdate);
                    assert_eq!(frame.stream_id, 0);
                    frame.decode_control::<WindowUpdate>().unwrap().credit_bytes
                })
                .sum::<u64>(),
            8
        );
        assert!(session.pop_next_frame().unwrap().is_none());
    }

    #[test]
    fn crossing_receive_allowance_registry_is_bounded() {
        let settings = ConnectionSettings {
            max_concurrent_streams: 2,
            initial_stream_window: 8,
            initial_connection_window: 8,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);

        for stream_id in [1, 3, 5] {
            session
                .receive_frame(headers_frame(
                    stream_id,
                    StreamEnvelope::request(stream_id, "fs.write"),
                ))
                .unwrap();
            assert!(
                session
                    .reset_stream(stream_id, RESET_CANCELLED, "cancelled")
                    .unwrap()
            );
            let reset = session.pop_next_frame().unwrap().unwrap();
            session.observe_frame_written(&reset);
        }

        assert_eq!(session.crossing_receive_allowances.len(), 2);
        assert!(!session.crossing_receive_allowances.contains_key(&1));
        assert_eq!(session.crossing_receive_allowances.get(&3), Some(&8));
        assert_eq!(session.crossing_receive_allowances.get(&5), Some(&8));
    }

    #[test]
    fn duplicate_reset_preserves_queued_reset_and_crossing_receive_allowance() {
        let settings = ConnectionSettings {
            initial_stream_window: 8,
            initial_connection_window: 8,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.write")))
            .unwrap();
        let superseding = StreamEnvelope::request_with_options(
            3,
            "fs.stat",
            &RequestOptions {
                supersedes_stream_id: 1,
                ..RequestOptions::default()
            },
        );
        session
            .receive_frame(headers_frame(3, superseding))
            .unwrap();

        assert_eq!(session.stream_tombstone(1), Some(StreamTombstone::Reset));
        assert_eq!(session.crossing_receive_allowances.get(&1), Some(&8));
        assert!(
            !session
                .reset_stream(1, RESET_CANCELLED, "duplicate service reset")
                .unwrap()
        );

        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.frame_type, FrameType::ResetStream);
        assert_eq!(reset.stream_id, 1);
        session.observe_frame_written(&reset);
        assert_eq!(session.stream_tombstone(1), Some(StreamTombstone::Closed));
        assert!(
            !session
                .reset_stream(1, RESET_CANCELLED, "duplicate after reset write")
                .unwrap()
        );

        let crossing = session.receive_frame(data_frame(1, 4)).unwrap();
        assert_eq!(
            crossing.routed,
            RoutedFrame::RejectedStream { stream_id: 1 }
        );
        assert_eq!(session.crossing_receive_allowances.get(&1), Some(&4));
        let connection_update = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(connection_update.frame_type, FrameType::WindowUpdate);
        assert_eq!(connection_update.stream_id, 0);
        assert_eq!(
            connection_update
                .decode_control::<WindowUpdate>()
                .unwrap()
                .credit_bytes,
            4
        );
    }

    #[test]
    fn written_stream_window_update_does_not_mask_closed_tombstone() {
        let settings = ConnectionSettings {
            initial_stream_window: 4,
            initial_connection_window: 4,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::new(StreamInitiator::Client, &settings);
        let stream_id = session
            .open_unary_request("fs.read", RequestOptions::default())
            .unwrap();
        while let Some(frame) = session.pop_next_frame().unwrap() {
            session.observe_frame_written(&frame);
        }
        session
            .receive_frame(headers_frame(
                stream_id,
                StreamEnvelope::response(stream_id, "fs.read", MessageRole::FinalResponse, true),
            ))
            .unwrap();
        let credit = session
            .receive_frame(data_frame(stream_id, 2))
            .unwrap()
            .data_credit()
            .unwrap();
        session.acknowledge_data(credit.0, credit.1).unwrap();

        while let Some(frame) = session.pop_next_frame().unwrap() {
            session.observe_frame_written(&frame);
        }
        assert!(session.extracted_streams.is_empty());

        session
            .receive_frame(Frame::new(FrameType::EndStream, stream_id))
            .unwrap();
        assert!(session.extracted_streams.is_empty());
        assert_eq!(
            session.stream_tombstone(stream_id),
            Some(StreamTombstone::Closed)
        );
    }

    #[test]
    fn local_send_apis_reject_frames_after_finish_without_mutating_queue() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();
        assert_eq!(
            session.finish_stream(1, Priority::Background).unwrap(),
            StreamState::HalfClosedLocal
        );
        let queued = session.queued_len();

        assert!(session.finish_stream(1, Priority::Background).is_err());
        assert!(
            session
                .send_owned_data(1, DataChannel::FileBody, b"late".to_vec(), Priority::Bulk)
                .is_err()
        );
        assert!(
            session
                .send_response_with_priority(
                    1,
                    "fs.read",
                    MessageRole::FinalResponse,
                    true,
                    Priority::Background,
                )
                .is_err()
        );
        assert_eq!(session.queued_len(), queued);
    }

    #[test]
    fn protocol_session_remote_end_closes_locally_ended_wire_stream() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let stream_id = session
            .open_unary_request("fs.stat", RequestOptions::default())
            .unwrap();
        let headers = session.pop_next_frame().unwrap().unwrap();
        let end = session.pop_next_frame().unwrap().unwrap();
        session.observe_frame_written(&headers);
        session.observe_frame_written(&end);
        assert_eq!(session.stream_tombstone(stream_id), None);

        session
            .receive_frame(headers_frame(
                stream_id,
                StreamEnvelope::response(stream_id, "fs.stat", MessageRole::FinalResponse, true),
            ))
            .unwrap();
        session
            .receive_frame(Frame::new(FrameType::EndStream, stream_id))
            .unwrap();

        assert_eq!(
            session.stream_tombstone(stream_id),
            Some(StreamTombstone::Closed)
        );
    }

    #[test]
    fn completed_stream_tombstones_compact_into_monotonic_watermarks() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let mut last_stream_id = 0;

        for _ in 0..256 {
            let stream_id = session
                .open_unary_request("fs.stat", RequestOptions::default())
                .unwrap();
            last_stream_id = stream_id;
            let headers = session.pop_next_frame().unwrap().unwrap();
            let end = session.pop_next_frame().unwrap().unwrap();
            session.observe_frame_written(&headers);
            session.observe_frame_written(&end);
            session
                .receive_frame(headers_frame(
                    stream_id,
                    StreamEnvelope::response(
                        stream_id,
                        "fs.stat",
                        MessageRole::FinalResponse,
                        true,
                    ),
                ))
                .unwrap();
            session
                .receive_frame(Frame::new(FrameType::EndStream, stream_id))
                .unwrap();
        }

        assert!(session.stream_tombstones.is_empty());
        assert_eq!(
            session.stream_tombstone(last_stream_id),
            Some(StreamTombstone::Closed)
        );
        assert_eq!(session.closed_stream_watermarks[0], last_stream_id);
    }

    #[test]
    fn protocol_session_terminate_invalidates_stream_and_control_frames() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        session
            .open_unary_request("fs.stat", RequestOptions::default())
            .unwrap();
        let extracted = session.pop_next_frame().unwrap().unwrap();
        let ping = Frame::new(FrameType::Ping, 0).with_priority(Priority::UserInput);

        session.terminate();

        assert!(!session.should_write_frame(&extracted));
        assert!(!session.should_write_frame(&ping));
        assert!(
            session
                .open_unary_request("fs.stat", RequestOptions::default())
                .is_err()
        );
    }

    #[test]
    fn protocol_session_incoming_reset_purges_queued_response_frames() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        session
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();
        session
            .send_data(1, DataChannel::FileBody, b"stale", Priority::Bulk)
            .unwrap();
        assert_eq!(session.queued_len(), 1);

        session.receive_frame(reset_frame(1)).unwrap();

        assert_eq!(session.active_streams(), 0);
        assert_eq!(session.queued_len(), 0);
        assert!(session.pop_next_frame().unwrap().is_none());
    }

    #[test]
    fn protocol_session_terminate_clears_all_queued_and_stream_state() {
        let mut session =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        session
            .open_request_with_body(
                "fs.write",
                RequestOptions::default(),
                DataChannel::FileBody,
                b"payload",
            )
            .unwrap();
        assert!(session.queued_len() > 0);
        assert_eq!(session.active_streams(), 1);

        session.terminate();

        assert_eq!(session.queued_len(), 0);
        assert_eq!(session.active_streams(), 0);
        assert_eq!(session.in_flight_len(), 0);
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
    fn protocol_session_enforces_negotiated_unsolicited_ping_interval() {
        let settings = ConnectionSettings::recommended();
        let mut session = ProtocolSession::new(StreamInitiator::Server, &settings);
        let minimum = Duration::from_millis(u64::from(settings.min_unsolicited_ping_interval_ms));
        let started_at = Instant::now();
        let ping = |token: &[u8]| {
            Frame::from_control(
                FrameType::Ping,
                0,
                &PingPayload {
                    token: token.to_vec(),
                },
            )
        };

        session
            .receive_frame_at(ping(b"first"), started_at)
            .unwrap();
        assert_eq!(
            session.pop_next_frame().unwrap().unwrap().frame_type,
            FrameType::Pong
        );

        session
            .receive_frame_at(
                Frame::new(FrameType::Pong, 0),
                started_at + minimum - Duration::from_millis(1),
            )
            .unwrap();
        session
            .receive_frame_at(ping(b"on-time"), started_at + minimum)
            .unwrap();
        assert_eq!(
            session.pop_next_frame().unwrap().unwrap().frame_type,
            FrameType::Pong
        );

        let error = session
            .receive_frame_at(
                ping(b"too-soon"),
                started_at + minimum + minimum - Duration::from_millis(1),
            )
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("unsolicited v5 PING"));
        assert!(error.to_string().contains("negotiated minimum"));
        assert!(session.pop_next_frame().unwrap().is_none());
    }

    #[test]
    fn protocol_session_rejects_handshake_frames_after_activation() {
        for frame_type in [
            FrameType::Hello,
            FrameType::Settings,
            FrameType::SettingsAck,
        ] {
            let mut session =
                ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());

            let error = session
                .receive_frame(Frame::new(frame_type, 0))
                .unwrap_err();

            assert!(error.to_string().contains("after v5 session activation"));
        }
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
            ..
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
    fn outbound_scheduler_services_priority_classes_by_weight() {
        let mut scheduler = OutboundScheduler::new(&ConnectionSettings::recommended());
        let priorities = [
            Priority::UserInput,
            Priority::ForegroundDocument,
            Priority::VisibleFileTree,
            Priority::LspSupport,
            Priority::Background,
            Priority::Bulk,
        ];
        let mut stream_id = 1_u64;
        for (priority, weight) in priorities.into_iter().zip(PRIORITY_SERVICE_WEIGHTS) {
            for _ in 0..weight {
                scheduler
                    .enqueue(Frame::new(FrameType::Headers, stream_id).with_priority(priority))
                    .unwrap();
                stream_id += 2;
            }
        }

        let observed = (0..PRIORITY_SERVICE_WEIGHTS.iter().sum())
            .map(|_| scheduler.pop_next().unwrap().unwrap().priority)
            .collect::<Vec<_>>();
        let expected = priorities
            .into_iter()
            .zip(PRIORITY_SERVICE_WEIGHTS)
            .flat_map(|(priority, weight)| std::iter::repeat_n(priority.as_u8(), weight))
            .collect::<Vec<_>>();

        assert_eq!(observed, expected);
    }

    #[test]
    fn outbound_scheduler_bulk_is_not_starved_by_continuous_user_input() {
        let mut scheduler = OutboundScheduler::new(&ConnectionSettings::recommended());
        for stream_id in (1_u64..=199).step_by(2) {
            scheduler
                .enqueue(
                    Frame::new(FrameType::Headers, stream_id).with_priority(Priority::UserInput),
                )
                .unwrap();
        }
        scheduler
            .enqueue(Frame::new(FrameType::Headers, 201).with_priority(Priority::Bulk))
            .unwrap();

        let priorities = (0..=PRIORITY_SERVICE_WEIGHTS[0])
            .map(|_| scheduler.pop_next().unwrap().unwrap().priority)
            .collect::<Vec<_>>();

        assert_eq!(priorities.last(), Some(&Priority::Bulk.as_u8()));
        assert_eq!(
            priorities
                .iter()
                .filter(|priority| **priority == Priority::UserInput.as_u8())
                .count(),
            PRIORITY_SERVICE_WEIGHTS[0]
        );
    }

    #[test]
    fn outbound_scheduler_urgent_credit_bypasses_blocked_stream_data() {
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
                window_update_frame(1, 4)
                    .unwrap()
                    .with_priority(Priority::Bulk),
            )
            .unwrap();

        let update = scheduler.pop_next().unwrap().unwrap();

        assert_eq!(update.frame_type, FrameType::WindowUpdate);
        assert_eq!(update.stream_id, 1);
        assert!(scheduler.pop_next().unwrap().is_none());
    }

    #[test]
    fn outbound_scheduler_preserves_cross_priority_stream_order() {
        let mut scheduler = OutboundScheduler::new(&ConnectionSettings::recommended());
        scheduler
            .enqueue(data_frame(1, 4).with_priority(Priority::Bulk))
            .unwrap();
        scheduler
            .enqueue(
                end_stream_frame(1)
                    .unwrap()
                    .with_priority(Priority::UserInput),
            )
            .unwrap();

        assert_eq!(
            scheduler.pop_next().unwrap().unwrap().frame_type,
            FrameType::Data
        );
        assert_eq!(
            scheduler.pop_next().unwrap().unwrap().frame_type,
            FrameType::EndStream
        );
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
            .enqueue(Frame::new(FrameType::Headers, 3).with_priority(Priority::ForegroundDocument))
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
        let mut first = Frame::new(FrameType::GoAway, 0);
        first.control = b"abc".to_vec();
        let second = Frame::new(FrameType::GoAway, 0);

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
            Some(FrameType::GoAway)
        );
        assert_eq!(scheduler.connection_control_used(), 0);

        scheduler.enqueue(second).unwrap();
        assert_eq!(
            scheduler.pop_next().unwrap().map(|frame| frame.frame_type),
            Some(FrameType::GoAway)
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
    fn outbound_scheduler_drop_stream_purges_frames_budget_and_window() {
        let settings = ConnectionSettings {
            initial_stream_window: 3,
            initial_connection_window: 64,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);
        let mut headers = Frame::new(FrameType::Headers, 1);
        headers.control = b"request".to_vec();
        scheduler.enqueue(headers).unwrap();
        scheduler.enqueue(data_frame(1, 4)).unwrap();
        scheduler.enqueue(end_stream_frame(1).unwrap()).unwrap();
        scheduler.grant_stream(1, 10).unwrap();

        assert_eq!(scheduler.queued_len(), 3);
        assert!(scheduler.connection_control_used() > 0);
        assert_eq!(scheduler.stream_window(1), 13);

        assert_eq!(scheduler.drop_stream(1), 3);
        assert!(scheduler.is_empty());
        assert_eq!(scheduler.connection_control_used(), 0);
        assert_eq!(scheduler.stream_control_used(1), 0);
        assert_eq!(scheduler.stream_window(1), 3);
    }

    #[test]
    fn outbound_scheduler_batch_enqueue_is_atomic_on_budget_failure() {
        let settings = ConnectionSettings {
            connection_control_budget: FRAME_HEADER_LEN as u32,
            stream_control_budget: FRAME_HEADER_LEN as u32,
            ..ConnectionSettings::recommended()
        };
        let mut scheduler = OutboundScheduler::new(&settings);

        let error = scheduler
            .enqueue_batch(vec![
                Frame::new(FrameType::GoAway, 0),
                Frame::new(FrameType::GoAway, 0),
            ])
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::OutOfMemory);
        assert!(scheduler.is_empty());
        assert_eq!(scheduler.connection_control_used(), 0);
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
    fn data_decoder_validates_uncompressed_length() {
        let error = decode_data_body(b"abc".to_vec(), ContentEncoding::None, 2, 3).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("body has 3 bytes, declared 2"));
    }

    #[test]
    fn data_decoder_rejects_declared_size_before_zstd_allocation() {
        let compressed = zstd::bulk::compress(&[b'a'; 128], ZSTD_DATA_COMPRESSION_LEVEL)
            .expect("test data should compress");

        let error = decode_data_body(compressed, ContentEncoding::Zstd, 128, 64).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .contains("decoded length 128 exceeds maximum 64")
        );
    }

    #[test]
    fn data_decoder_rejects_corrupt_zstd_body_within_limit() {
        let error = decode_data_body(
            b"not-zstd".to_vec(),
            ContentEncoding::Zstd,
            8,
            DEFAULT_MAX_FRAME_BODY_LEN as u64,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("failed to decompress"));
    }

    #[test]
    fn protocol_session_keeps_large_body_queue_bounded() {
        let settings = ConnectionSettings {
            initial_stream_window: 3,
            initial_connection_window: 64,
            ..ConnectionSettings::recommended()
        };
        let mut session = ProtocolSession::with_limits(
            StreamInitiator::Client,
            &settings,
            FrameLimits {
                max_control_len: DEFAULT_MAX_CONTROL_LEN,
                max_body_len: 4,
            },
        );
        let body = vec![b'x'; 4 * 1_024];

        let stream_id = session
            .open_request_with_body(
                "fs.write",
                RequestOptions::default(),
                DataChannel::FileBody,
                &body,
            )
            .unwrap();

        // HEADERS, one lazy producer, and END_STREAM are independent of chunk count.
        assert_eq!(session.queued_len(), 3);
        assert_eq!(session.scheduler.pending_data_frames(stream_id), 0);
        assert_eq!(
            session.pop_next_frame().unwrap().unwrap().frame_type,
            FrameType::Headers
        );

        // The producer sizes the first chunk to available credit and does not retain a blocked
        // encoded frame after the credit is consumed.
        assert_eq!(session.pop_next_frame().unwrap().unwrap().body.len(), 3);
        assert_eq!(session.queued_len(), 2);
        assert_eq!(session.scheduler.pending_data_frames(stream_id), 0);
        assert!(session.pop_next_frame().unwrap().is_none());
        assert_eq!(session.scheduler.pending_data_frames(stream_id), 0);
    }

    #[test]
    fn protocol_session_lazy_producers_preserve_payload_body_and_end_order() {
        let settings = ConnectionSettings::recommended();
        let mut session = ProtocolSession::with_limits(
            StreamInitiator::Client,
            &settings,
            FrameLimits {
                max_control_len: DEFAULT_MAX_CONTROL_LEN,
                max_body_len: 2,
            },
        );

        session
            .open_request_with_payload_and_body(
                "fs.write",
                RequestOptions::default(),
                b"abcd",
                DataChannel::FileBody,
                b"efgh",
            )
            .unwrap();

        assert_eq!(session.queued_len(), 4);
        assert_eq!(
            session.pop_next_frame().unwrap().unwrap().frame_type,
            FrameType::Headers
        );
        let mut chunks = Vec::new();
        let mut ended = false;
        while let Some(frame) = session.pop_next_frame().unwrap() {
            if frame.frame_type == FrameType::Data {
                assert!(!ended, "DATA must not follow END_STREAM");
                let envelope = frame.decode_control::<DataEnvelope>().unwrap();
                chunks.push((envelope.channel, frame.body));
            } else {
                assert_eq!(frame.frame_type, FrameType::EndStream);
                assert!(!ended, "stream must end exactly once");
                ended = true;
            }
        }
        assert!(ended);
        assert_eq!(
            chunks,
            vec![
                (DataChannel::Unspecified as i32, b"ab".to_vec()),
                (DataChannel::Unspecified as i32, b"cd".to_vec()),
                (DataChannel::FileBody as i32, b"ef".to_vec()),
                (DataChannel::FileBody as i32, b"gh".to_vec()),
            ]
        );
    }

    #[test]
    fn protocol_session_owned_data_apis_retain_vec_allocations() {
        let mut client =
            ProtocolSession::new(StreamInitiator::Client, &ConnectionSettings::recommended());
        let payload = vec![1_u8, 2, 3];
        let body = vec![4_u8, 5, 6, 7];
        let payload_ptr = payload.as_ptr();
        let body_ptr = body.as_ptr();

        let stream_id = client
            .open_request_with_owned_payload_and_body(
                "fs.write",
                RequestOptions::default(),
                payload,
                DataChannel::FileBody,
                body,
            )
            .unwrap();

        assert_eq!(
            client.scheduler.producer_buffer_ptrs(stream_id),
            vec![payload_ptr, body_ptr]
        );

        let mut server =
            ProtocolSession::new(StreamInitiator::Server, &ConnectionSettings::recommended());
        server
            .receive_frame(headers_frame(1, StreamEnvelope::request(1, "fs.read")))
            .unwrap();
        let response = vec![8_u8, 9, 10];
        let response_ptr = response.as_ptr();
        server
            .send_owned_data(1, DataChannel::FileBody, response, Priority::Bulk)
            .unwrap();
        assert_eq!(server.scheduler.producer_buffer_ptrs(1), vec![response_ptr]);
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
        assert_eq!(session.queued_len(), 3);

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
            FrameLimits {
                max_control_len: DEFAULT_MAX_CONTROL_LEN,
                max_body_len: 4,
            },
        );
        let stream_id = session
            .open_request_with_body(
                "fs.write",
                RequestOptions {
                    priority: Priority::UserInput,
                    ..RequestOptions::default()
                },
                DataChannel::FileBody,
                b"abcdefghij",
            )
            .unwrap();

        assert_eq!(
            session
                .pop_next_frame()
                .unwrap()
                .map(|frame| frame.frame_type),
            Some(FrameType::Headers)
        );
        assert_eq!(session.pop_next_frame().unwrap().unwrap().body, b"abc");
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

        assert_eq!(session.pop_next_frame().unwrap().unwrap().body, b"d");
        assert!(session.pop_next_frame().unwrap().is_none());

        session
            .receive_frame(window_update_frame(stream_id, 4).unwrap())
            .unwrap();
        assert_eq!(session.pop_next_frame().unwrap().unwrap().body, b"efgh");
        assert!(session.pop_next_frame().unwrap().is_none());

        session
            .receive_frame(window_update_frame(stream_id, 2).unwrap())
            .unwrap();
        assert_eq!(session.pop_next_frame().unwrap().unwrap().body, b"ij");
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
        session
            .send_data(1, DataChannel::SearchPayload, b"stale", Priority::Bulk)
            .unwrap();
        assert_eq!(session.queued_len(), 1);
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
        assert_eq!(session.queued_len(), 1);
        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.frame_type, FrameType::ResetStream);
        assert_eq!(reset.stream_id, 1);
        assert_eq!(
            reset.decode_control::<ResetStream>().unwrap().code,
            RESET_CANCELLED
        );
        assert!(session.pop_next_frame().unwrap().is_none());
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
        session
            .send_data(1, DataChannel::FileBody, b"stale", Priority::Bulk)
            .unwrap();

        assert_eq!(session.expire_deadlines(99).unwrap(), 0);
        assert!(session.is_in_flight(1));
        assert_eq!(session.queued_len(), 1);

        assert_eq!(session.expire_deadlines(100).unwrap(), 1);
        assert!(!session.is_in_flight(1));
        assert_eq!(session.active_streams(), 0);
        let reset = session.pop_next_frame().unwrap().unwrap();
        assert_eq!(reset.stream_id, 1);
        assert_eq!(
            reset.decode_control::<ResetStream>().unwrap().code,
            RESET_DEADLINE_EXCEEDED
        );
        assert!(session.pop_next_frame().unwrap().is_none());
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
            priority: Priority::Background,
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
    fn settings_negotiation_keeps_idle_ping_at_or_above_accepted_minimum() {
        let mut desired = ConnectionSettings::recommended();
        desired.idle_ping_interval_ms = 100;
        desired.min_unsolicited_ping_interval_ms = 0;

        let accepted = ConnectionSettings::accept_peer_desired(Some(&desired));

        assert_eq!(
            accepted.min_unsolicited_ping_interval_ms,
            MIN_UNSOLICITED_PING_INTERVAL_MS
        );
        assert_eq!(
            accepted.idle_ping_interval_ms,
            accepted.min_unsolicited_ping_interval_ms
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
    fn handshakes_ignore_unknown_additive_protobuf_fields() {
        // Field 63, wire type varint, value 42. These literal bytes are a golden
        // representation of an additive field unknown to the current schema.
        const UNKNOWN_FIELD: &[u8] = &[0xf8, 0x03, 0x2a];

        let client_hello = ClientHello::nucleotide("0.1.0");
        let mut client_hello_frame = Frame::from_control(FrameType::Hello, 0, &client_hello);
        client_hello_frame.control.extend_from_slice(UNKNOWN_FIELD);
        let input =
            encode_sequenced_frames([client_hello_frame, Frame::new(FrameType::SettingsAck, 0)]);
        let mut server_io = FramedIo::new(Cursor::new(input), Vec::new());

        let server_handshake =
            server_handshake(&mut server_io, &ServerHandshakeInfo::current("/workspace")).unwrap();

        assert_eq!(server_handshake.client_hello, client_hello);

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
        let mut server_hello_frame = Frame::from_control(FrameType::Hello, 0, &server_hello);
        server_hello_frame.control.extend_from_slice(UNKNOWN_FIELD);
        let mut settings_frame = Frame::from_control(FrameType::Settings, 0, &settings);
        settings_frame.control.extend_from_slice(UNKNOWN_FIELD);
        let input = encode_sequenced_frames([server_hello_frame, settings_frame]);
        let mut client_io = FramedIo::new(Cursor::new(input), Vec::new());

        let client_handshake = client_handshake(&mut client_io, client_hello).unwrap();

        assert_eq!(client_handshake.server_hello, server_hello);
        assert_eq!(client_handshake.settings, settings);
    }

    #[test]
    fn client_handshake_accepts_lower_and_higher_peer_minor_versions() {
        for (client_minor, server_minor) in [(7, 3), (3, 7)] {
            let mut client = ClientHello::nucleotide("0.1.0");
            client.protocol_minor = client_minor;
            client.required_capabilities = vec!["multiplex".to_string()];
            let settings = ConnectionSettings::recommended();
            let server_hello = ServerHello {
                protocol_major: PROTOCOL_MAJOR,
                protocol_minor: server_minor,
                helper_version: "helper".to_string(),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                workspace_root: "/workspace".to_string(),
                control_codec: "protobuf".to_string(),
                capabilities: vec!["multiplex".to_string()],
                accepted_settings: Some(settings.clone()),
            };
            let input = encode_sequenced_frames([
                Frame::from_control(FrameType::Hello, 0, &server_hello),
                Frame::from_control(FrameType::Settings, 0, &settings),
            ]);
            let mut io = FramedIo::new(Cursor::new(input), Vec::new());

            let handshake = client_handshake(&mut io, client).unwrap();

            assert_eq!(handshake.client_hello.protocol_minor, client_minor);
            assert_eq!(handshake.server_hello.protocol_minor, server_minor);
        }
    }

    #[test]
    fn handshake_rejects_peer_major_version_mismatch() {
        let mut client = ClientHello::nucleotide("0.1.0");
        client.protocol_major = PROTOCOL_MAJOR + 1;
        let input = encode_sequenced_frames([Frame::from_control(FrameType::Hello, 0, &client)]);
        let mut server_io = FramedIo::new(Cursor::new(input), Vec::new());

        let server_error =
            server_handshake(&mut server_io, &ServerHandshakeInfo::current("/workspace"))
                .err()
                .expect("server should reject a client major-version mismatch");

        assert_eq!(server_error.kind(), io::ErrorKind::InvalidData);
        assert!(
            server_error
                .to_string()
                .contains("protocol major from client")
        );

        let client = ClientHello::nucleotide("0.1.0");
        let mut server_hello =
            ServerHello::accept_client(&client, &ServerHandshakeInfo::current("/workspace"))
                .unwrap();
        server_hello.protocol_major = PROTOCOL_MAJOR + 1;
        let input =
            encode_sequenced_frames([Frame::from_control(FrameType::Hello, 0, &server_hello)]);
        let mut client_io = FramedIo::new(Cursor::new(input), Vec::new());

        let client_error = client_handshake(&mut client_io, client)
            .err()
            .expect("client should reject a server major-version mismatch");

        assert_eq!(client_error.kind(), io::ErrorKind::InvalidData);
        assert!(
            client_error
                .to_string()
                .contains("protocol major from server")
        );
    }

    #[test]
    fn server_handshake_reads_client_hello_and_writes_server_frames() {
        let client = ClientHello::nucleotide("0.1.0");
        let input = encode_sequenced_frames([
            Frame::from_control(FrameType::Hello, 0, &client),
            Frame::new(FrameType::SettingsAck, 0),
        ]);
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
    fn server_handshake_rejects_gapped_client_frame_sequence() {
        let client = ClientHello::nucleotide("0.1.0");
        let mut hello = Frame::from_control(FrameType::Hello, 0, &client);
        hello.frame_sequence = 1;
        let mut ack = Frame::new(FrameType::SettingsAck, 0);
        ack.frame_sequence = 3;
        let mut input = Vec::new();
        write_frame(&mut input, &hello).unwrap();
        write_frame(&mut input, &ack).unwrap();
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let error = server_handshake(&mut io, &ServerHandshakeInfo::current("/workspace"))
            .err()
            .expect("server handshake should reject a sequence gap");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("expected 2, got 3"));
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
        let input = encode_sequenced_frames([
            Frame::from_control(FrameType::Hello, 0, &server_hello),
            Frame::from_control(FrameType::Settings, 0, &settings),
        ]);
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
    fn client_handshake_rejects_duplicate_server_frame_sequence() {
        let client = ClientHello::nucleotide("0.1.0");
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
        let mut hello = Frame::from_control(FrameType::Hello, 0, &server_hello);
        hello.frame_sequence = 1;
        let mut settings_frame = Frame::from_control(FrameType::Settings, 0, &settings);
        settings_frame.frame_sequence = 1;
        let mut input = Vec::new();
        write_frame(&mut input, &hello).unwrap();
        write_frame(&mut input, &settings_frame).unwrap();
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let error = client_handshake(&mut io, client)
            .err()
            .expect("client handshake should reject a duplicate sequence");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("expected 2, got 1"));
    }

    #[test]
    fn client_handshake_rejects_settings_that_differ_from_server_hello() {
        let client = ClientHello::nucleotide("0.1.0");
        let accepted_settings = ConnectionSettings::recommended();
        let mut settings_frame = accepted_settings.clone();
        settings_frame.max_concurrent_streams -= 1;
        let server_hello = ServerHello {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            helper_version: "helper".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            workspace_root: "/workspace".to_string(),
            control_codec: "protobuf".to_string(),
            capabilities: vec!["multiplex".to_string()],
            accepted_settings: Some(accepted_settings),
        };
        let input = encode_sequenced_frames([
            Frame::from_control(FrameType::Hello, 0, &server_hello),
            Frame::from_control(FrameType::Settings, 0, &settings_frame),
        ]);
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let error = match client_handshake(&mut io, client) {
            Ok(_) => panic!("expected client handshake to reject inconsistent settings"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("SETTINGS does not match"));
        assert!(error.to_string().contains("accepted_settings"));
    }

    #[test]
    fn server_handshake_rejects_client_without_protobuf_codec() {
        let mut client = ClientHello::nucleotide("0.1.0");
        client.control_codecs = vec!["json".to_string()];
        let input = encode_sequenced_frames([Frame::from_control(FrameType::Hello, 0, &client)]);
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
        let input = encode_sequenced_frames([Frame::from_control(FrameType::Hello, 0, &client)]);
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
        let input =
            encode_sequenced_frames([Frame::from_control(FrameType::Hello, 0, &server_hello)]);
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
        let input = encode_sequenced_frames([
            Frame::from_control(FrameType::Hello, 0, &server_hello),
            Frame::from_control(FrameType::Settings, 0, &settings),
        ]);
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
    fn client_handshake_rejects_server_capability_not_offered_by_client() {
        let mut client = ClientHello::nucleotide("0.1.0");
        client.capabilities = vec!["multiplex".to_string()];
        let settings = ConnectionSettings::recommended();
        let server_hello = ServerHello {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            helper_version: "helper".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            workspace_root: "/workspace".to_string(),
            control_codec: "protobuf".to_string(),
            capabilities: vec!["multiplex".to_string(), "watch".to_string()],
            accepted_settings: Some(settings.clone()),
        };
        let input = encode_sequenced_frames([
            Frame::from_control(FrameType::Hello, 0, &server_hello),
            Frame::from_control(FrameType::Settings, 0, &settings),
        ]);
        let mut io = FramedIo::new(Cursor::new(input), Vec::new());

        let error = client_handshake(&mut io, client)
            .err()
            .expect("client should reject a capability it did not offer");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("UNSUPPORTED_CAPABILITY"));
        assert!(error.to_string().contains("unrequested"));
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
