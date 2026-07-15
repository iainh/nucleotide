// Protocol codec, request policy, deadline, and heartbeat tests.

use super::*;

#[test]
fn v5_file_mailbox_retains_bytes_and_credit_until_consumer_poll() {
    let budget = V5ConnectionByteBudget::new(16);
    let mailbox = V5FileStreamMailbox::new(16, budget.reservation());
    mailbox.push_chunk(b"hello".to_vec(), 5).unwrap();

    assert_eq!(budget.used(), 5);
    let state = mailbox.state.lock().unwrap();
    assert_eq!(state.queued_credit, 5);
    drop(state);

    let waker = futures::task::noop_waker();
    let mut context = TaskContext::from_waker(&waker);
    let Poll::Ready(Some(Ok(delivery))) = mailbox.poll_delivery(&mut context) else {
        panic!("queued file chunk should be ready");
    };
    assert_eq!(delivery.credit_bytes, 5);
    assert_eq!(
        delivery.event,
        RemoteFileReadEvent::Chunk(b"hello".to_vec())
    );
    assert_eq!(budget.used(), 0);
    assert_eq!(mailbox.state.lock().unwrap().queued_credit, 0);
}
#[test]
fn v5_file_mailbox_coalesces_tiny_frames_into_bounded_deliveries() {
    let frame_count = V5_FILE_STREAM_MAX_QUEUED_CHUNKS * 4;
    let budget = V5ConnectionByteBudget::new(frame_count);
    let mailbox = V5FileStreamMailbox::new(frame_count, budget.reservation());

    for _ in 0..frame_count {
        mailbox.push_chunk(vec![7], 1).unwrap();
    }

    let state = mailbox.state.lock().unwrap();
    assert_eq!(state.queued_bytes, frame_count);
    assert_eq!(state.queued_credit, frame_count as u64);
    assert!(state.chunks.len() < V5_FILE_STREAM_MAX_QUEUED_CHUNKS);
    assert!(
        state
            .chunks
            .iter()
            .all(|chunk| chunk.body.len() <= V5_FILE_STREAM_CHUNK_TARGET_BYTES)
    );
}

#[test]
fn v5_file_mailbox_failure_releases_all_retained_bytes_and_credit() {
    let budget = V5ConnectionByteBudget::new(16);
    let mailbox = V5FileStreamMailbox::new(16, budget.reservation());
    mailbox.push_chunk(b"hello".to_vec(), 5).unwrap();
    mailbox.push_chunk(b" world".to_vec(), 6).unwrap();

    let released = mailbox.fail(RemoteClientError::Disconnected);

    assert_eq!(released, 11);
    assert_eq!(budget.used(), 0);
    let state = mailbox.state.lock().unwrap();
    assert!(state.chunks.is_empty());
    assert_eq!(state.queued_bytes, 0);
    assert_eq!(state.queued_credit, 0);
    assert!(matches!(state.error, Some(RemoteClientError::Disconnected)));
}

fn arg_index(args: &[OsString], needle: &str) -> usize {
    args.iter()
        .position(|arg| arg.as_os_str() == OsStr::new(needle))
        .unwrap_or_else(|| panic!("missing argument {needle:?} in {args:?}"))
}

fn has_arg_pair(args: &[OsString], key: &str, value: &str) -> bool {
    args.windows(2).any(|window| {
        window[0].as_os_str() == OsStr::new(key) && window[1].as_os_str() == OsStr::new(value)
    })
}

fn assert_arg_pair(args: &[OsString], key: &str, value: &str) {
    assert!(
        has_arg_pair(args, key, value),
        "missing argument pair {key:?} {value:?} in {args:?}"
    );
}

fn assert_ssh_non_interactive_defaults(args: &[OsString]) {
    assert_arg_pair(args, "-o", "BatchMode=yes");
    assert_arg_pair(args, "-o", "NumberOfPasswordPrompts=0");
    assert_arg_pair(args, "-o", "ConnectionAttempts=1");
    assert_arg_pair(args, "-o", "StrictHostKeyChecking=accept-new");
    assert_arg_pair(args, "-o", "ServerAliveInterval=15");
    assert_arg_pair(args, "-o", "ServerAliveCountMax=3");
}

fn ssh_target_separator_index(args: &[OsString]) -> usize {
    arg_index(args, "--")
}
use nucleotide_workspace::RemoteWorkspaceKind;
use std::collections::VecDeque;
use std::io::Cursor;
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
use std::sync::{
    Arc, Barrier, Condvar, Mutex as StdMutex,
    atomic::{AtomicUsize, Ordering},
};

fn v5_client_input(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
    v5_client_input_with_settings(frames, protocol_v5::ConnectionSettings::recommended())
}

fn v5_client_input_with_settings(
    frames: Vec<protocol_v5::Frame>,
    settings: protocol_v5::ConnectionSettings,
) -> Vec<u8> {
    let mut hello = protocol_v5::ClientHello::nucleotide("test-client");
    hello.desired_settings = Some(settings);
    let mut all_frames = vec![
        protocol_v5::Frame::from_control(protocol_v5::FrameType::Hello, 0, &hello),
        protocol_v5::Frame::new(protocol_v5::FrameType::SettingsAck, 0),
    ];
    all_frames.extend(frames);
    encode_v5_sequenced_frames(all_frames)
}

fn v5_server_input(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
    let mut info = protocol_v5::ServerHandshakeInfo::current("/workspace");
    info.capabilities
        .retain(|capability| capability != "compression_zstd");
    v5_server_input_with_info(frames, info)
}

fn v5_server_input_with_compression(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
    v5_server_input_with_info(
        frames,
        protocol_v5::ServerHandshakeInfo::current("/workspace"),
    )
}

fn v5_server_input_with_info(
    frames: Vec<protocol_v5::Frame>,
    info: protocol_v5::ServerHandshakeInfo,
) -> Vec<u8> {
    let client = protocol_v5::ClientHello::nucleotide("test-client");
    v5_server_input_for_client(frames, &client, info)
}

fn v5_server_input_for_client(
    frames: Vec<protocol_v5::Frame>,
    client: &protocol_v5::ClientHello,
    info: protocol_v5::ServerHandshakeInfo,
) -> Vec<u8> {
    let hello = protocol_v5::ServerHello::accept_client(client, &info).unwrap();
    let settings = hello.accepted_settings.clone().unwrap();
    let mut all_frames = vec![
        protocol_v5::Frame::from_control(protocol_v5::FrameType::Hello, 0, &hello),
        protocol_v5::Frame::from_control(protocol_v5::FrameType::Settings, 0, &settings),
    ];
    all_frames.extend(frames);
    encode_v5_sequenced_frames(all_frames)
}

fn v5_heartbeat_client_hello(ping_timeout: Duration) -> protocol_v5::ClientHello {
    let mut settings = protocol_v5::ConnectionSettings::recommended();
    settings.ping_timeout_ms = u32::try_from(ping_timeout.as_millis()).unwrap();
    let mut client = protocol_v5::ClientHello::nucleotide("test-client");
    client.desired_settings = Some(settings);
    client
}

fn v5_test_client_heartbeat(now: Instant) -> V5ClientHeartbeat {
    let mut heartbeat =
        V5ClientHeartbeat::new(&protocol_v5::ConnectionSettings::recommended(), now);
    heartbeat.idle_ping_interval = Duration::from_millis(20);
    heartbeat.ping_timeout = Duration::from_millis(50);
    heartbeat
}

fn encode_v5_sequenced_frames(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
    let mut input = Vec::new();
    for (index, mut frame) in frames.into_iter().enumerate() {
        frame.frame_sequence = u64::try_from(index).unwrap() + 1;
        protocol_v5::write_frame(&mut input, &frame).unwrap();
    }
    input
}

fn v5_request_frames(
    stream_id: u64,
    request: &RemoteRequest,
    body: &[u8],
) -> Vec<protocol_v5::Frame> {
    v5_request_frames_with_options(stream_id, request, body, request.v5_request_options())
}

fn v5_request_frames_with_options(
    stream_id: u64,
    request: &RemoteRequest,
    body: &[u8],
    options: protocol_v5::RequestOptions,
) -> Vec<protocol_v5::Frame> {
    let (method, payload) = request.to_v5_method_payload().unwrap();
    let headers = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        stream_id,
        &protocol_v5::StreamEnvelope::request_with_options(stream_id, method, &options),
    );
    let payload = protocol_v5::stream_data_frame(
        stream_id,
        payload,
        protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
    )
    .unwrap();
    let mut frames = vec![headers, payload];
    if !body.is_empty() {
        frames.push(
            protocol_v5::stream_data_frame(
                stream_id,
                body.to_vec(),
                protocol_v5::DataFrameOptions::new(request.v5_body_channel()),
            )
            .unwrap(),
        );
    }
    frames.push(protocol_v5::Frame::new(
        protocol_v5::FrameType::EndStream,
        stream_id,
    ));
    frames
}

fn v5_protobuf_request_frames<M>(
    stream_id: u64,
    method: &str,
    payload: &M,
) -> Vec<protocol_v5::Frame>
where
    M: ProstMessage,
{
    v5_protobuf_request_frames_with_options(
        stream_id,
        method,
        payload,
        protocol_v5::RequestOptions::default(),
    )
}

fn v5_protobuf_request_frames_with_options<M>(
    stream_id: u64,
    method: &str,
    payload: &M,
    options: protocol_v5::RequestOptions,
) -> Vec<protocol_v5::Frame>
where
    M: ProstMessage,
{
    let headers = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        stream_id,
        &protocol_v5::StreamEnvelope::request_with_options(stream_id, method, &options),
    );
    let payload = protocol_v5::stream_data_frame(
        stream_id,
        payload.encode_to_vec(),
        protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
    )
    .unwrap();
    vec![
        headers,
        payload,
        protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, stream_id),
    ]
}

fn v5_json_request_frames<T>(stream_id: u64, method: &str, payload: &T) -> Vec<protocol_v5::Frame>
where
    T: Serialize,
{
    let headers = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        stream_id,
        &protocol_v5::StreamEnvelope::request(stream_id, method),
    );
    let payload = protocol_v5::stream_data_frame(
        stream_id,
        serde_json::to_vec(payload).unwrap(),
        protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
    )
    .unwrap();
    vec![
        headers,
        payload,
        protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, stream_id),
    ]
}

fn read_v5_frames(bytes: Vec<u8>) -> Vec<protocol_v5::Frame> {
    let mut cursor = Cursor::new(bytes);
    let mut frames = Vec::new();
    while let Some(frame) = protocol_v5::read_frame(&mut cursor).unwrap() {
        frames.push(frame);
    }
    frames
}

fn read_v5_complete_frames(bytes: Vec<u8>) -> Vec<protocol_v5::Frame> {
    let mut cursor = Cursor::new(bytes);
    let mut frames = Vec::new();
    while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
        frames.push(frame);
    }
    frames
}

fn assert_v5_data_channel_priority(
    frames: &[protocol_v5::Frame],
    stream_id: u64,
    channel: protocol_v5::DataChannel,
    priority: protocol_v5::Priority,
) {
    let matching_frames = frames
        .iter()
        .filter(|frame| {
            if frame.stream_id != stream_id || frame.frame_type != protocol_v5::FrameType::Data {
                return false;
            }
            let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
            protocol_v5::DataChannel::try_from(envelope.channel).unwrap() == channel
        })
        .collect::<Vec<_>>();

    assert!(
        !matching_frames.is_empty(),
        "expected {channel:?} DATA on stream {stream_id}"
    );
    assert!(
        matching_frames
            .iter()
            .all(|frame| frame.priority == priority.as_u8()),
        "{channel:?} DATA did not preserve {priority:?} priority"
    );
}

fn v5_response_frames(
    stream_id: u64,
    method: &str,
    response: RemoteResponse,
    body: Vec<u8>,
) -> Vec<protocol_v5::Frame> {
    v5_response_frames_with_content_encoding(
        stream_id,
        method,
        response,
        body,
        protocol_v5::ContentEncoding::None,
    )
}

fn v5_response_frames_with_content_encoding(
    stream_id: u64,
    method: &str,
    response: RemoteResponse,
    body: Vec<u8>,
    content_encoding: protocol_v5::ContentEncoding,
) -> Vec<protocol_v5::Frame> {
    let payload = response.to_v5_payload().unwrap();
    let mut frames = vec![
        protocol_v5::stream_data_frame(
            stream_id,
            payload,
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified)
                .with_content_encoding(content_encoding),
        )
        .unwrap(),
    ];
    if !body.is_empty() {
        let channel = if matches!(response, RemoteResponse::ReadFile(_)) {
            protocol_v5::DataChannel::FileBody
        } else {
            protocol_v5::DataChannel::Stdout
        };
        frames.push(
            protocol_v5::stream_data_frame(
                stream_id,
                body,
                protocol_v5::DataFrameOptions::new(channel).with_content_encoding(content_encoding),
            )
            .unwrap(),
        );
    }
    frames.push(protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        stream_id,
        &protocol_v5::StreamEnvelope::response(
            stream_id,
            method,
            protocol_v5::MessageRole::FinalResponse,
            true,
        ),
    ));
    frames.push(protocol_v5::Frame::new(
        protocol_v5::FrameType::EndStream,
        stream_id,
    ));
    frames
}

fn v5_raw_response_frames(
    stream_id: u64,
    method: &str,
    payload: Vec<u8>,
) -> Vec<protocol_v5::Frame> {
    let mut frames = Vec::new();
    if !payload.is_empty() {
        frames.push(
            protocol_v5::stream_data_frame(
                stream_id,
                payload,
                protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
            )
            .unwrap(),
        );
    }
    frames.push(protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        stream_id,
        &protocol_v5::StreamEnvelope::response(
            stream_id,
            method,
            protocol_v5::MessageRole::FinalResponse,
            true,
        ),
    ));
    frames.push(protocol_v5::Frame::new(
        protocol_v5::FrameType::EndStream,
        stream_id,
    ));
    frames
}

fn v5_watch_event_open_frame(event_stream_id: u64, watch_id: u64) -> protocol_v5::Frame {
    protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        event_stream_id,
        &protocol_v5::StreamEnvelope::event(event_stream_id, "watch.batch", watch_id),
    )
}

fn decode_v5_service_response(
    frames: &[protocol_v5::Frame],
    stream_id: u64,
) -> (
    Option<RemoteResponse>,
    Vec<u8>,
    Option<protocol_v5::ErrorHeader>,
) {
    let mut method = None;
    let mut payload = Vec::new();
    let mut body = Vec::new();
    let mut error = None;

    for frame in frames.iter().filter(|frame| frame.stream_id == stream_id) {
        match frame.frame_type {
            protocol_v5::FrameType::Headers => {
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                match envelope.message {
                    Some(protocol_v5::stream_envelope::Message::Response(_)) => {
                        method = Some(envelope.method);
                    }
                    Some(protocol_v5::stream_envelope::Message::Error(header)) => {
                        method = Some(envelope.method);
                        error = Some(header);
                    }
                    _ => {}
                }
            }
            protocol_v5::FrameType::Data => {
                let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
                match channel {
                    protocol_v5::DataChannel::Unspecified => payload.extend_from_slice(&frame.body),
                    protocol_v5::DataChannel::SearchPayload => {}
                    protocol_v5::DataChannel::FileBody
                    | protocol_v5::DataChannel::Stdout
                    | protocol_v5::DataChannel::Stderr
                    | protocol_v5::DataChannel::Stdin => body.extend_from_slice(&frame.body),
                }
            }
            _ => {}
        }
    }

    let response = method
        .as_deref()
        .filter(|_| !payload.is_empty())
        .map(|method| RemoteResponse::from_v5_payload(method, &payload).unwrap());
    (response, body, error)
}

fn decode_v5_partial_file_search_responses(
    frames: &[protocol_v5::Frame],
    stream_id: u64,
) -> Vec<FileSearchResponse> {
    let mut partial_payload_next = false;
    let mut partials = Vec::new();

    for frame in frames.iter().filter(|frame| frame.stream_id == stream_id) {
        match frame.frame_type {
            protocol_v5::FrameType::Headers => {
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                partial_payload_next = envelope.role
                    == protocol_v5::MessageRole::PartialResult as i32
                    && envelope.method == "search.files";
            }
            protocol_v5::FrameType::Data if partial_payload_next => {
                let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
                if channel == protocol_v5::DataChannel::SearchPayload {
                    let response =
                        RemoteResponse::from_v5_payload("search.files", &frame.body).unwrap();
                    let RemoteResponse::FileSearch(search) = response else {
                        panic!("expected file search partial response");
                    };
                    partials.push(search);
                    partial_payload_next = false;
                }
            }
            _ => {}
        }
    }

    partials
}

fn decode_v5_partial_text_search_responses(
    frames: &[protocol_v5::Frame],
    stream_id: u64,
) -> Vec<TextSearchResponse> {
    let mut partial_payload_next = false;
    let mut partials = Vec::new();

    for frame in frames.iter().filter(|frame| frame.stream_id == stream_id) {
        match frame.frame_type {
            protocol_v5::FrameType::Headers => {
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                partial_payload_next = envelope.role
                    == protocol_v5::MessageRole::PartialResult as i32
                    && envelope.method == "search.text";
            }
            protocol_v5::FrameType::Data if partial_payload_next => {
                let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
                if channel == protocol_v5::DataChannel::SearchPayload {
                    let response =
                        RemoteResponse::from_v5_payload("search.text", &frame.body).unwrap();
                    let RemoteResponse::TextSearch(search) = response else {
                        panic!("expected text search partial response");
                    };
                    partials.push(search);
                    partial_payload_next = false;
                }
            }
            _ => {}
        }
    }

    partials
}

fn decode_v5_progress_headers(
    frames: &[protocol_v5::Frame],
    stream_id: u64,
    method: &str,
) -> Vec<protocol_v5::Progress> {
    frames
        .iter()
        .filter(|frame| {
            frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::Headers
        })
        .filter_map(|frame| {
            let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
            if envelope.role != protocol_v5::MessageRole::Progress as i32
                || envelope.method != method
            {
                return None;
            }
            match envelope.message {
                Some(protocol_v5::stream_envelope::Message::Progress(progress)) => Some(progress),
                _ => None,
            }
        })
        .collect()
}

#[cfg(unix)]
fn v5_data_for_channel(
    frames: &[protocol_v5::Frame],
    stream_id: u64,
    expected_channel: protocol_v5::DataChannel,
) -> Vec<u8> {
    let mut data = Vec::new();
    for frame in frames.iter().filter(|frame| {
        frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::Data
    }) {
        let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
        let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
        if channel == expected_channel {
            data.extend_from_slice(&frame.body);
        }
    }
    data
}

#[cfg(unix)]
fn find_v5_output_data_for_channel(
    output: &SharedWrite,
    stream_id: u64,
    expected_channel: protocol_v5::DataChannel,
) -> Vec<u8> {
    let bytes = output.bytes();
    let mut cursor = Cursor::new(bytes);
    let mut data = Vec::new();
    while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
        if frame.stream_id != stream_id || frame.frame_type != protocol_v5::FrameType::Data {
            continue;
        }
        let Ok(envelope) = frame.decode_control::<protocol_v5::DataEnvelope>() else {
            continue;
        };
        if protocol_v5::DataChannel::try_from(envelope.channel).ok() == Some(expected_channel) {
            data.extend_from_slice(&frame.body);
        }
    }
    data
}

#[cfg(unix)]
fn v5_first_data_channel_index(
    frames: &[protocol_v5::Frame],
    stream_id: u64,
    expected_channel: protocol_v5::DataChannel,
) -> Option<usize> {
    frames.iter().position(|frame| {
        if frame.stream_id != stream_id || frame.frame_type != protocol_v5::FrameType::Data {
            return false;
        }
        let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
        protocol_v5::DataChannel::try_from(envelope.channel).unwrap() == expected_channel
    })
}

fn v5_write_temp_files(parent: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(parent)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(".nucleotide-write-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn decode_v5_protobuf_service_response<T>(
    frames: &[protocol_v5::Frame],
    stream_id: u64,
) -> (Option<T>, Option<protocol_v5::ErrorHeader>)
where
    T: ProstMessage + Default,
{
    let mut payload = Vec::new();
    let mut saw_response = false;
    let mut error = None;

    for frame in frames.iter().filter(|frame| frame.stream_id == stream_id) {
        match frame.frame_type {
            protocol_v5::FrameType::Headers => {
                let envelope = frame
                    .decode_control::<protocol_v5::StreamEnvelope>()
                    .unwrap();
                match envelope.message {
                    Some(protocol_v5::stream_envelope::Message::Response(_)) => {
                        saw_response = true;
                    }
                    Some(protocol_v5::stream_envelope::Message::Error(header)) => {
                        error = Some(header);
                    }
                    _ => {}
                }
            }
            protocol_v5::FrameType::Data => {
                let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
                let channel = protocol_v5::DataChannel::try_from(envelope.channel).unwrap();
                if channel == protocol_v5::DataChannel::Unspecified {
                    payload.extend_from_slice(&frame.body);
                }
            }
            _ => {}
        }
    }

    let response = saw_response.then(|| T::decode(payload.as_slice()).unwrap());
    (response, error)
}

fn find_v5_watch_start_response(
    output: &SharedWrite,
    stream_id: u64,
) -> Option<protocol_v5::WatchStartResponse> {
    let bytes = output.bytes();
    let mut cursor = Cursor::new(bytes);
    let mut payload = Vec::new();
    let mut saw_response = false;
    while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
        if frame.stream_id != stream_id {
            continue;
        }
        match frame.frame_type {
            protocol_v5::FrameType::Headers => {
                let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
                if matches!(
                    envelope.message,
                    Some(protocol_v5::stream_envelope::Message::Response(_))
                ) {
                    saw_response = true;
                }
            }
            protocol_v5::FrameType::Data => {
                let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().ok()?;
                if protocol_v5::DataChannel::try_from(envelope.channel).ok()?
                    == protocol_v5::DataChannel::Unspecified
                {
                    payload.extend_from_slice(&frame.body);
                }
            }
            _ => {}
        }
    }
    (saw_response && !payload.is_empty())
        .then(|| protocol_v5::WatchStartResponse::decode(payload.as_slice()).ok())
        .flatten()
}

fn find_v5_watch_batch(
    output: &SharedWrite,
    event_stream_id: u64,
) -> Option<protocol_v5::WatchBatch> {
    let bytes = output.bytes();
    let mut cursor = Cursor::new(bytes);
    while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
        if frame.stream_id != event_stream_id || frame.frame_type != protocol_v5::FrameType::Headers
        {
            continue;
        }
        let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
        let Some(protocol_v5::stream_envelope::Message::Event(event)) = envelope.message else {
            continue;
        };
        if event.kind == "watch.batch"
            && let Some(batch) = event.watch_batch
        {
            return Some(batch);
        }
    }
    None
}

fn find_v5_watch_batch_in_frames(
    frames: &[protocol_v5::Frame],
    event_stream_id: u64,
) -> Option<protocol_v5::WatchBatch> {
    for frame in frames {
        if frame.stream_id != event_stream_id || frame.frame_type != protocol_v5::FrameType::Headers
        {
            continue;
        }
        let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
        let Some(protocol_v5::stream_envelope::Message::Event(event)) = envelope.message else {
            continue;
        };
        if event.kind == "watch.batch"
            && let Some(batch) = event.watch_batch
        {
            return Some(batch);
        }
    }
    None
}

fn v5_final_response_index(frames: &[protocol_v5::Frame], stream_id: u64) -> usize {
    frames
        .iter()
        .position(|frame| {
            if frame.stream_id != stream_id || frame.frame_type != protocol_v5::FrameType::Headers {
                return false;
            }
            let envelope = frame
                .decode_control::<protocol_v5::StreamEnvelope>()
                .unwrap();
            matches!(
                envelope.message,
                Some(protocol_v5::stream_envelope::Message::Response(_))
            ) && envelope.role == protocol_v5::MessageRole::FinalResponse as i32
        })
        .unwrap_or_else(|| panic!("missing final response for stream {stream_id}"))
}

#[test]
fn v5_method_payload_round_trips_existing_one_shot_requests() {
    let requests = vec![
        RemoteRequest::Stat {
            path: PathBuf::from("src/lib.rs"),
        },
        RemoteRequest::ListDirs {
            paths: vec![PathBuf::from("."), PathBuf::from("crates")],
        },
        RemoteRequest::FindAncestorFile {
            start: PathBuf::from("crates/nucleotide-remote/src"),
            file_name: "Cargo.toml".to_string(),
            limit: 4,
        },
        RemoteRequest::RenamePath {
            from: PathBuf::from("old.rs"),
            to: PathBuf::from("new.rs"),
        },
        RemoteRequest::ReadFile {
            path: PathBuf::from("README.md"),
            max_bytes: Some(4096),
        },
        RemoteRequest::WriteFile {
            path: PathBuf::from("src/main.rs"),
            create_parent_dirs: true,
            expected_modified_unix_millis: Some(123),
            expected_modified_unix_nanos: Some(456),
        },
        RemoteRequest::FileSearch(FileSearchRequest {
            pattern: Some("lib".to_string()),
            limit: 25,
            ..FileSearchRequest::default()
        }),
        RemoteRequest::TextSearch(TextSearchRequest {
            pattern: "needle".to_string(),
            limit: 10,
            ..TextSearchRequest::default()
        }),
        RemoteRequest::GitStatus {
            root: PathBuf::new(),
            include_untracked: true,
            limit: 99,
        },
        RemoteRequest::RunProcess(ProcessRequest {
            program: "printf".to_string(),
            args: vec!["hello".to_string()],
            cwd: PathBuf::new(),
            env: BTreeMap::from([("LANG".to_string(), "C".to_string())]),
            clear_env: true,
            inherit_project_environment: false,
            max_output_bytes: Some(1024),
            timeout_ms: Some(250),
        }),
        RemoteRequest::Shutdown,
    ];

    for request in requests {
        let (method, payload) = request.to_v5_method_payload().unwrap();
        let decoded = RemoteRequest::from_v5_method_payload(method, &payload).unwrap();
        assert_eq!(decoded, request, "{method}");
    }
}

#[test]
fn v5_request_payloads_serialize_remote_paths_with_posix_separators() {
    let windows_style_path = PathBuf::from(r"\home\iheggie\projects");
    let request = RemoteRequest::ListDir {
        path: windows_style_path.clone(),
    };
    let (method, payload) = request.to_v5_method_payload().unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();

    assert_eq!(method, "fs.list_dir");
    assert_eq!(payload["path"], "/home/iheggie/projects");

    let cached_payload = V5DirectoryListPayload {
        path: windows_style_path.clone(),
        known_generation: Some(1),
        known_fingerprint: Some(2),
    };
    let payload = serde_json::to_value(cached_payload).unwrap();
    assert_eq!(payload["path"], "/home/iheggie/projects");

    let search = FileSearchRequest {
        root: windows_style_path.clone(),
        excluded_relative_prefixes: vec![PathBuf::from(r"target\generated")],
        ..FileSearchRequest::default()
    };
    let payload = serde_json::to_value(search).unwrap();
    assert_eq!(payload["root"], "/home/iheggie/projects");
    assert_eq!(payload["excluded_relative_prefixes"][0], "target/generated");

    let process = ProcessRequest {
        program: "pwd".to_string(),
        args: Vec::new(),
        cwd: windows_style_path,
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    };
    let payload = serde_json::to_value(process).unwrap();
    assert_eq!(payload["cwd"], "/home/iheggie/projects");
}

#[test]
fn v5_request_options_classify_priority_idempotency_and_body_channel() {
    let write = RemoteRequest::WriteFile {
        path: PathBuf::from("src/lib.rs"),
        create_parent_dirs: false,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };
    let write_options = write.v5_request_options();
    assert_eq!(
        write_options.idempotency,
        protocol_v5::Idempotency::Mutation
    );
    assert_eq!(write_options.priority, protocol_v5::Priority::UserInput);
    assert_eq!(write.v5_body_channel(), protocol_v5::DataChannel::FileBody);
    assert!(!write.v5_retry_after_reconnect_allowed());

    let list_dirs = RemoteRequest::ListDirs {
        paths: vec![PathBuf::from(".")],
    };
    assert_eq!(
        list_dirs.v5_request_options().priority,
        protocol_v5::Priority::VisibleFileTree
    );
    assert!(list_dirs.v5_retry_after_reconnect_allowed());

    let search = RemoteRequest::TextSearch(TextSearchRequest {
        pattern: "main".to_string(),
        ..TextSearchRequest::default()
    });
    let search_options = search.v5_request_options();
    assert_eq!(search_options.priority, protocol_v5::Priority::Background);
    assert_eq!(search_options.cancellation_group, "search.text");
    assert_eq!(
        search.v5_body_channel(),
        protocol_v5::DataChannel::SearchPayload
    );
    assert!(search.v5_retry_after_reconnect_allowed());

    let read = RemoteRequest::ReadFile {
        path: PathBuf::from("src/lib.rs"),
        max_bytes: None,
    };
    assert_eq!(
        read.v5_request_options().priority,
        protocol_v5::Priority::ForegroundDocument
    );

    let environment = RemoteRequest::ProjectEnvironment {
        root: PathBuf::from("."),
    };
    assert_eq!(
        environment.v5_request_options().priority,
        protocol_v5::Priority::LspSupport
    );

    assert_eq!(
        RemoteRequest::Shutdown.v5_request_options().priority,
        protocol_v5::Priority::UserInput
    );

    let process = RemoteRequest::RunProcess(ProcessRequest {
        program: "cat".to_string(),
        args: Vec::new(),
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    });
    assert_eq!(
        process.v5_request_options().idempotency,
        protocol_v5::Idempotency::Process
    );
    assert_eq!(process.v5_body_channel(), protocol_v5::DataChannel::Stdin);
    assert!(!process.v5_retry_after_reconnect_allowed());
    assert!(!RemoteRequest::Shutdown.v5_retry_after_reconnect_allowed());
}

#[test]
fn v5_request_deadline_policy_covers_every_method() {
    let metadata = (
        Some(V5_REQUEST_METADATA_DEADLINE),
        Some(V5_REQUEST_METADATA_INACTIVITY),
    );
    let mutation = (
        Some(V5_REQUEST_MUTATION_DEADLINE),
        Some(V5_REQUEST_MUTATION_INACTIVITY),
    );
    let file = (
        Some(V5_REQUEST_FILE_DEADLINE),
        Some(V5_REQUEST_FILE_INACTIVITY),
    );
    let search = (
        Some(V5_REQUEST_SEARCH_DEADLINE),
        Some(V5_REQUEST_SEARCH_INACTIVITY),
    );
    let control = (
        Some(V5_REQUEST_CONTROL_DEADLINE),
        Some(V5_REQUEST_CONTROL_INACTIVITY),
    );
    let requests = vec![
        (RemoteRequest::Stat { path: "a".into() }, metadata),
        (RemoteRequest::ListDir { path: "a".into() }, metadata),
        (
            RemoteRequest::ListDirs {
                paths: vec!["a".into()],
            },
            metadata,
        ),
        (
            RemoteRequest::FindAncestorFile {
                start: "a".into(),
                file_name: "Cargo.toml".to_string(),
                limit: 8,
            },
            metadata,
        ),
        (RemoteRequest::CreateFile { path: "a".into() }, mutation),
        (RemoteRequest::CreateDir { path: "a".into() }, mutation),
        (
            RemoteRequest::RenamePath {
                from: "a".into(),
                to: "b".into(),
            },
            mutation,
        ),
        (RemoteRequest::DeletePath { path: "a".into() }, mutation),
        (
            RemoteRequest::CopyPath {
                from: "a".into(),
                to: "b".into(),
            },
            mutation,
        ),
        (
            RemoteRequest::ReadFile {
                path: "a".into(),
                max_bytes: None,
            },
            file,
        ),
        (
            RemoteRequest::WriteFile {
                path: "a".into(),
                create_parent_dirs: false,
                expected_modified_unix_millis: None,
                expected_modified_unix_nanos: None,
            },
            file,
        ),
        (
            RemoteRequest::FileSearch(FileSearchRequest::default()),
            search,
        ),
        (
            RemoteRequest::TextSearch(TextSearchRequest {
                pattern: "needle".to_string(),
                ..TextSearchRequest::default()
            }),
            search,
        ),
        (
            RemoteRequest::ProjectEnvironment { root: "a".into() },
            mutation,
        ),
        (RemoteRequest::GitHead { root: "a".into() }, metadata),
        (
            RemoteRequest::GitStatus {
                root: "a".into(),
                include_untracked: true,
                limit: 10,
            },
            metadata,
        ),
        (RemoteRequest::Shutdown, control),
    ];
    let created_at = Instant::now();
    let now_unix_ms = 1_000_000;

    for (request, (absolute_timeout, inactivity_timeout)) in requests {
        let policy = request.v5_deadline_policy();
        assert_eq!(policy.absolute_timeout, absolute_timeout, "{request:?}");
        assert_eq!(policy.inactivity_timeout, inactivity_timeout, "{request:?}");
        let context = RemoteRequestContext::from_policy_at(policy, created_at, now_unix_ms);
        assert_eq!(
            context.absolute_deadline,
            absolute_timeout.map(|timeout| created_at + timeout),
            "{request:?}"
        );
        assert_eq!(
            context.inactivity_timeout, inactivity_timeout,
            "{request:?}"
        );
        assert_eq!(
            request
                .v5_request_options_with_context(context)
                .deadline_unix_ms,
            now_unix_ms + u64::try_from(absolute_timeout.unwrap().as_millis()).unwrap(),
            "{request:?}"
        );
    }

    let bounded_process = RemoteRequest::RunProcess(ProcessRequest {
        program: "sleep".to_string(),
        args: Vec::new(),
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: Some(2_500),
    });
    let bounded_policy = bounded_process.v5_deadline_policy();
    assert_eq!(
        bounded_policy.absolute_timeout,
        Some(Duration::from_millis(2_500) + V5_REQUEST_PROCESS_CANCELLATION_GRACE)
    );
    assert_eq!(bounded_policy.inactivity_timeout, None);

    let unlimited_process = RemoteRequest::RunProcess(ProcessRequest {
        program: "server".to_string(),
        args: Vec::new(),
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    });
    let unlimited = RemoteRequestContext::from_policy_at(
        unlimited_process.v5_deadline_policy(),
        created_at,
        now_unix_ms,
    );
    assert_eq!(unlimited.absolute_deadline, None);
    assert_eq!(unlimited.deadline_unix_ms, 0);
    assert_eq!(unlimited.inactivity_timeout, None);

    let watch_control = v5_watch_control_request_context();
    assert_eq!(
        watch_control.absolute_deadline,
        watch_control
            .created_at
            .checked_add(V5_REQUEST_CONTROL_DEADLINE)
    );
    assert_eq!(
        watch_control.inactivity_timeout,
        Some(V5_REQUEST_CONTROL_INACTIVITY)
    );
    assert_ne!(watch_control.deadline_unix_ms, 0);
}

#[test]
fn v5_request_progress_extends_only_inactivity() {
    let started = Instant::now();
    let context = RemoteRequestContext::from_policy_at(
        RemoteRequestDeadlinePolicy::bounded(Duration::from_secs(60), Duration::from_secs(30)),
        started,
        1_000,
    );
    let mut deadline = V5RequestDeadline::new(context, started);

    assert_eq!(
        deadline.next_expiry(),
        Some((
            started + Duration::from_secs(30),
            RemoteRequestDeadlineKind::Inactivity
        ))
    );
    deadline.observe_progress(started + Duration::from_secs(20));
    assert_eq!(
        deadline.next_expiry(),
        Some((
            started + Duration::from_secs(50),
            RemoteRequestDeadlineKind::Inactivity
        ))
    );
    deadline.observe_progress(started + Duration::from_secs(50));
    assert_eq!(
        deadline.next_expiry(),
        Some((
            started + Duration::from_secs(60),
            RemoteRequestDeadlineKind::Absolute
        ))
    );
    assert_eq!(
        deadline.expired_at(started + Duration::from_secs(60)),
        Some(RemoteRequestDeadlineKind::Absolute)
    );
}

#[test]
fn v5_inbound_request_progress_is_stream_scoped() {
    let targeted = [
        protocol_v5::RoutedFrame::WindowUpdate {
            stream_id: 7,
            credit_bytes: 1,
        },
        protocol_v5::RoutedFrame::Headers {
            stream_id: 7,
            role: protocol_v5::MessageRole::Progress,
            method: "fs.stat".to_string(),
        },
        protocol_v5::RoutedFrame::Data {
            stream_id: 7,
            flow_control_len: 1,
        },
        protocol_v5::RoutedFrame::EndStream {
            stream_id: 7,
            state: protocol_v5::StreamState::Closed,
        },
        protocol_v5::RoutedFrame::ResetStream {
            stream_id: 7,
            known: true,
        },
    ];
    for routed in targeted {
        assert_eq!(v5_client_inbound_progress_stream(&routed), Some(7));
    }

    let unrelated = [
        protocol_v5::RoutedFrame::ConnectionControl {
            frame_type: protocol_v5::FrameType::Ping,
        },
        protocol_v5::RoutedFrame::WindowUpdate {
            stream_id: 0,
            credit_bytes: 1,
        },
        protocol_v5::RoutedFrame::RejectedStream { stream_id: 7 },
    ];
    for routed in unrelated {
        assert_eq!(v5_client_inbound_progress_stream(&routed), None);
    }
}

#[test]
fn v5_client_heartbeat_queues_once_and_correlates_exact_pong() {
    let started = Instant::now();
    let mut heartbeat = v5_test_client_heartbeat(started);
    let first_token = 1_u64.to_be_bytes().to_vec();

    assert_eq!(
        heartbeat
            .next_action(started + Duration::from_millis(20))
            .unwrap(),
        V5ClientHeartbeatAction::QueuePing(first_token.clone())
    );
    assert_eq!(
        heartbeat
            .next_action(started + Duration::from_millis(21))
            .unwrap(),
        V5ClientHeartbeatAction::Wait(Duration::from_millis(49))
    );

    let ping = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Ping,
        0,
        &protocol_v5::PingPayload {
            token: first_token.clone(),
        },
    );
    heartbeat
        .mark_ping_started(&ping, started + Duration::from_millis(22))
        .unwrap();
    let pong = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Pong,
        0,
        &protocol_v5::PingPayload { token: first_token },
    );
    assert_eq!(
        heartbeat
            .observe_inbound(
                pong.frame_type,
                Some(pong.control.clone()),
                started + Duration::from_millis(27),
            )
            .unwrap(),
        Some(Duration::from_millis(5))
    );
    assert_eq!(
        heartbeat
            .next_action(started + Duration::from_millis(47))
            .unwrap(),
        V5ClientHeartbeatAction::QueuePing(2_u64.to_be_bytes().to_vec())
    );

    let mut active = v5_test_client_heartbeat(started);
    active
        .observe_inbound(
            protocol_v5::FrameType::Ping,
            None,
            started + Duration::from_millis(15),
        )
        .unwrap();
    assert_eq!(
        active
            .next_action(started + Duration::from_millis(20))
            .unwrap(),
        V5ClientHeartbeatAction::Wait(Duration::from_millis(15))
    );
}

#[test]
fn v5_client_heartbeat_requires_matching_pong_before_timeout() {
    let started = Instant::now();
    let mut heartbeat = v5_test_client_heartbeat(started);
    let token = match heartbeat
        .next_action(started + Duration::from_millis(20))
        .unwrap()
    {
        V5ClientHeartbeatAction::QueuePing(token) => token,
        action => panic!("expected heartbeat PING, got {action:?}"),
    };
    let ping = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Ping,
        0,
        &protocol_v5::PingPayload {
            token: token.clone(),
        },
    );
    let ping_started = started + Duration::from_millis(22);
    heartbeat.mark_ping_started(&ping, ping_started).unwrap();

    let unrelated = protocol_v5::Frame::new(protocol_v5::FrameType::GoAway, 0);
    assert_eq!(
        heartbeat
            .observe_inbound(
                unrelated.frame_type,
                None,
                started + Duration::from_millis(30),
            )
            .unwrap(),
        None
    );
    let wrong_pong = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Pong,
        0,
        &protocol_v5::PingPayload {
            token: b"wrong".to_vec(),
        },
    );
    let error = heartbeat
        .observe_inbound(
            wrong_pong.frame_type,
            Some(wrong_pong.control.clone()),
            started + Duration::from_millis(31),
        )
        .unwrap_err();
    assert!(error.to_string().contains("unexpected heartbeat token"));
    let mut altered_control = protocol_v5::PingPayload { token }.encode_to_vec();
    altered_control.extend_from_slice(&[0x78, 0x01]);
    let error = heartbeat
        .observe_inbound(
            protocol_v5::FrameType::Pong,
            Some(altered_control),
            started + Duration::from_millis(32),
        )
        .unwrap_err();
    assert!(error.to_string().contains("unexpected heartbeat token"));
    assert_eq!(
        heartbeat
            .next_action(ping_started + Duration::from_millis(50))
            .unwrap(),
        V5ClientHeartbeatAction::TimedOut("v5 peer did not answer client idle PING before timeout")
    );

    let mut unsolicited = v5_test_client_heartbeat(started);
    assert!(
        unsolicited
            .observe_inbound(
                wrong_pong.frame_type,
                Some(wrong_pong.control.clone()),
                started,
            )
            .unwrap_err()
            .to_string()
            .contains("unsolicited")
    );

    let mut queued = v5_test_client_heartbeat(started);
    let queued_token = match queued
        .next_action(started + Duration::from_millis(20))
        .unwrap()
    {
        V5ClientHeartbeatAction::QueuePing(token) => token,
        action => panic!("expected queued heartbeat PING, got {action:?}"),
    };
    let queued_control = protocol_v5::PingPayload {
        token: queued_token,
    }
    .encode_to_vec();
    assert!(
        queued
            .observe_inbound(
                protocol_v5::FrameType::Pong,
                Some(queued_control),
                started + Duration::from_millis(21),
            )
            .unwrap_err()
            .to_string()
            .contains("before the client heartbeat PING was written")
    );
    assert_eq!(
        queued
            .next_action(started + Duration::from_millis(70))
            .unwrap(),
        V5ClientHeartbeatAction::TimedOut("v5 client writer did not send idle PING before timeout")
    );
}

#[test]
fn v5_client_heartbeat_deadlines_win_transition_races() {
    let started = Instant::now();
    let mut queued = v5_test_client_heartbeat(started);
    let token = match queued
        .next_action(started + Duration::from_millis(20))
        .unwrap()
    {
        V5ClientHeartbeatAction::QueuePing(token) => token,
        action => panic!("expected queued heartbeat PING, got {action:?}"),
    };
    let ping = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Ping,
        0,
        &protocol_v5::PingPayload {
            token: token.clone(),
        },
    );
    let error = queued
        .mark_ping_started(&ping, started + Duration::from_millis(70))
        .unwrap_err();
    let RemoteClientError::Io(error) = error else {
        panic!("expected queued heartbeat timeout, got {error:?}");
    };
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("writer did not send"));
    assert!(matches!(queued.ping, Some(V5ClientPing::Queued { .. })));

    let mut outstanding = v5_test_client_heartbeat(started);
    let _ = outstanding
        .next_action(started + Duration::from_millis(20))
        .unwrap();
    outstanding
        .mark_ping_started(&ping, started + Duration::from_millis(21))
        .unwrap();
    let pong = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Pong,
        0,
        &protocol_v5::PingPayload { token },
    );
    let error = outstanding
        .observe_inbound(
            pong.frame_type,
            Some(pong.control),
            started + Duration::from_millis(71),
        )
        .unwrap_err();
    let RemoteClientError::Io(error) = error else {
        panic!("expected outstanding heartbeat timeout, got {error:?}");
    };
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("peer did not answer"));
    assert!(matches!(
        outstanding.ping,
        Some(V5ClientPing::Outstanding { .. })
    ));
}

#[test]
fn v5_client_heartbeat_normalizes_inconsistent_peer_settings() {
    let mut settings = protocol_v5::ConnectionSettings::recommended();
    settings.idle_ping_interval_ms = 20;
    settings.ping_timeout_ms = 0;
    settings.min_unsolicited_ping_interval_ms = 1;

    let heartbeat = V5ClientHeartbeat::new(&settings, Instant::now());

    assert_eq!(heartbeat.idle_ping_interval, Duration::from_millis(5_000));
    assert_eq!(
        heartbeat.ping_timeout,
        Duration::from_millis(u64::from(protocol_v5::PING_TIMEOUT_MS))
    );
}

#[test]
fn v5_service_preserves_request_priority_on_response_frames() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("visible.txt"), b"visible").unwrap();
    let request = RemoteRequest::Stat {
        path: PathBuf::from("visible.txt"),
    };
    let mut options = request.v5_request_options();
    options.priority = protocol_v5::Priority::UserInput;
    let input = v5_client_input(v5_request_frames_with_options(1, &request, &[], options));
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

    service
        .serve_v5(
            &mut io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, output) = io.into_inner();
    let response_frames = read_v5_frames(output)
        .into_iter()
        .filter(|frame| frame.stream_id == 1)
        .collect::<Vec<_>>();

    assert!(!response_frames.is_empty());
    assert!(
        response_frames
            .iter()
            .all(|frame| { frame.priority == protocol_v5::Priority::UserInput.as_u8() })
    );
}
