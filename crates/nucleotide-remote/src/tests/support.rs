// Shared blocking I/O, duplex transport, mock backend, and loopback fixtures.

fn wait_for_cancellation_observer(
    state: &Arc<(StdMutex<CancellationObservingState>, Condvar)>,
    predicate: impl Fn(&CancellationObservingState) -> bool,
    label: &str,
) {
    let started = Instant::now();
    let (state, wake) = &**state;
    let mut state = state.lock().unwrap();
    while !predicate(&state) {
        let (next, _) = wake.wait_timeout(state, Duration::from_millis(20)).unwrap();
        state = next;
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for cancellation observer {label}"
        );
    }
}

#[derive(Clone)]
enum FakeProtocolOutcome {
    Ok(RemoteResponse),
    Disconnected,
    Io(io::ErrorKind),
    RemoteError(&'static str),
}

#[derive(Clone)]
struct FakeProtocolClient {
    calls: Arc<StdMutex<Vec<RemoteRequest>>>,
    outcomes: Arc<StdMutex<VecDeque<FakeProtocolOutcome>>>,
}

impl FakeProtocolClient {
    fn new(
        calls: Arc<StdMutex<Vec<RemoteRequest>>>,
        outcomes: impl IntoIterator<Item = FakeProtocolOutcome>,
    ) -> Self {
        Self {
            calls,
            outcomes: Arc::new(StdMutex::new(outcomes.into_iter().collect())),
        }
    }
}

impl RemoteWorkspaceProtocolClient for FakeProtocolClient {
    fn request(
        &self,
        request: RemoteRequest,
        _body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        self.calls.lock().unwrap().push(request);
        match self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("fake protocol outcome")
        {
            FakeProtocolOutcome::Ok(response) => Ok((response, Vec::new())),
            FakeProtocolOutcome::Disconnected => Err(RemoteClientError::Disconnected),
            FakeProtocolOutcome::Io(kind) => Err(RemoteClientError::Io(io::Error::new(
                kind,
                "fake I/O failure",
            ))),
            FakeProtocolOutcome::RemoteError(code) => Err(RemoteClientError::Remote(RemoteError {
                code: code.to_string(),
                message: "remote final error".to_string(),
                diagnostic: None,
            })),
        }
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        Ok(())
    }
}

#[derive(Clone)]
enum ContextProtocolOutcome {
    Ok(RemoteResponse),
    Disconnected,
    Deadline,
}

struct ContextRecordingProtocolClient {
    contexts: Arc<StdMutex<Vec<RemoteRequestContext>>>,
    outcomes: StdMutex<VecDeque<ContextProtocolOutcome>>,
    closes: Arc<AtomicUsize>,
}

impl ContextRecordingProtocolClient {
    fn new(
        contexts: Arc<StdMutex<Vec<RemoteRequestContext>>>,
        outcomes: impl IntoIterator<Item = ContextProtocolOutcome>,
        closes: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            contexts,
            outcomes: StdMutex::new(outcomes.into_iter().collect()),
            closes,
        }
    }
}

impl RemoteWorkspaceProtocolClient for ContextRecordingProtocolClient {
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
        _body: Vec<u8>,
        context: RemoteRequestContext,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        self.contexts.lock().unwrap().push(context);
        match self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("context protocol outcome")
        {
            ContextProtocolOutcome::Ok(response) => Ok((response, Vec::new())),
            ContextProtocolOutcome::Disconnected => Err(RemoteClientError::Disconnected),
            ContextProtocolOutcome::Deadline => Err(RemoteClientError::RequestDeadlineExceeded {
                method: request.v5_method().to_string(),
                kind: RemoteRequestDeadlineKind::Inactivity,
            }),
        }
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        Ok(())
    }

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

struct LifecycleProtocolClient {
    closes: Arc<AtomicUsize>,
    shutdowns: Arc<AtomicUsize>,
}

impl RemoteWorkspaceProtocolClient for LifecycleProtocolClient {
    fn request(
        &self,
        _request: RemoteRequest,
        _body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        Err(RemoteClientError::Disconnected)
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(2));
        Ok(())
    }

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

struct WatchProtocolClient {
    starts: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    fail_start: bool,
}

impl RemoteWorkspaceProtocolClient for WatchProtocolClient {
    fn request(
        &self,
        _request: RemoteRequest,
        _body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        Err(RemoteClientError::Disconnected)
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        Ok(())
    }

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }

    fn start_watch(
        &self,
        _request: WorkspaceWatchRequest,
    ) -> std::result::Result<Option<WorkspaceWatch>, RemoteClientError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if self.fail_start {
            Err(RemoteClientError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "fake watch transport timeout",
            )))
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchControlFailure {
    Update,
    Stop,
}

struct WatchControlProtocolClient {
    updates: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    failed_operation: Option<WatchControlFailure>,
}

impl RemoteWorkspaceProtocolClient for WatchControlProtocolClient {
    fn request(
        &self,
        _request: RemoteRequest,
        _body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        Err(RemoteClientError::Disconnected)
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        Ok(())
    }

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }

    fn update_watch(
        &self,
        _watch_id: u64,
        _add_roots: Vec<PathBuf>,
        _remove_roots: Vec<PathBuf>,
    ) -> std::result::Result<Option<WorkspaceWatchUpdate>, RemoteClientError> {
        self.updates.fetch_add(1, Ordering::SeqCst);
        if self.failed_operation == Some(WatchControlFailure::Update) {
            Err(RemoteClientError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "fake watch.update transport timeout",
            )))
        } else {
            Ok(None)
        }
    }

    fn stop_watch(&self, _watch_id: u64) -> std::result::Result<(), RemoteClientError> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        if self.failed_operation == Some(WatchControlFailure::Stop) {
            Err(RemoteClientError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "fake watch.stop transport timeout",
            )))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Default)]
struct PausingWrite {
    state: Arc<(StdMutex<PausingWriteState>, Condvar)>,
}

#[derive(Default)]
struct PausingWriteState {
    bytes: Vec<u8>,
    pause_next_write: bool,
    paused: bool,
    released: bool,
}

impl PausingWrite {
    fn pause_next_write(&self) {
        let (lock, _) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.pause_next_write = true;
        state.paused = false;
        state.released = false;
    }

    fn wait_until_paused(&self) {
        let started = Instant::now();
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        while !state.paused {
            let (next, _) = cvar.wait_timeout(state, Duration::from_millis(20)).unwrap();
            state = next;
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for v5 writer pause"
            );
        }
    }

    fn release(&self) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.released = true;
        cvar.notify_all();
    }

    fn bytes(&self) -> Vec<u8> {
        self.state.0.lock().unwrap().bytes.clone()
    }
}

impl Write for PausingWrite {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        if state.pause_next_write {
            state.pause_next_write = false;
            state.paused = true;
            cvar.notify_all();
            while !state.released {
                state = cvar.wait(state).unwrap();
            }
        }
        state.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct ReleasingTransportAbort {
    writer: PausingWrite,
    calls: Arc<AtomicUsize>,
}

impl V5TransportAbort for ReleasingTransportAbort {
    fn abort(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.writer.release();
    }
}

struct CountingTransportAbort {
    calls: Arc<AtomicUsize>,
}

impl V5TransportAbort for CountingTransportAbort {
    fn abort(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Default)]
struct SharedWrite {
    bytes: Arc<StdMutex<Vec<u8>>>,
}

impl SharedWrite {
    fn bytes(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }
}

impl Write for SharedWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct FaultInjectingWrite {
    output: SharedWrite,
    mode: Arc<StdMutex<FaultWriteMode>>,
    successful_flushes: Arc<AtomicUsize>,
}

#[derive(Default)]
enum FaultWriteMode {
    #[default]
    Healthy,
    FailAfterBytes(usize),
    FailAfterFlushes(usize),
}

impl FaultInjectingWrite {
    fn output(&self) -> SharedWrite {
        self.output.clone()
    }

    fn fail_after_bytes(&self, bytes: usize) {
        *self.mode.lock().unwrap() = FaultWriteMode::FailAfterBytes(bytes);
    }

    fn fail_after_flushes(&self, successful_flushes: usize) {
        *self.mode.lock().unwrap() = FaultWriteMode::FailAfterFlushes(successful_flushes);
    }

    fn successful_flush_count(&self) -> usize {
        self.successful_flushes.load(Ordering::Acquire)
    }

    fn wait_for_successful_flush_after(&self, previous: usize) {
        let started = Instant::now();
        while self.successful_flush_count() <= previous {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "timed out waiting for v5 writer flush"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn injected_error() -> io::Error {
        io::Error::new(io::ErrorKind::BrokenPipe, "injected v5 writer failure")
    }
}

impl Write for FaultInjectingWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut mode = self.mode.lock().unwrap();
        if let FaultWriteMode::FailAfterBytes(remaining) = &mut *mode {
            if *remaining == 0 {
                return Err(Self::injected_error());
            }
            let written = buf.len().min(*remaining);
            self.output
                .bytes
                .lock()
                .unwrap()
                .extend_from_slice(&buf[..written]);
            *remaining -= written;
            return Ok(written);
        }

        self.output.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut mode = self.mode.lock().unwrap();
        if let FaultWriteMode::FailAfterFlushes(remaining) = &mut *mode {
            if *remaining == 0 {
                return Err(Self::injected_error());
            }
            *remaining -= 1;
        }
        self.successful_flushes.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

#[derive(Clone, Default)]
struct BlockingRead {
    state: Arc<(StdMutex<BlockingReadState>, Condvar)>,
}

struct BlockingReadState {
    bytes: VecDeque<u8>,
    closed: bool,
    next_frame_sequence: u64,
}

impl Default for BlockingReadState {
    fn default() -> Self {
        Self {
            bytes: VecDeque::new(),
            closed: false,
            next_frame_sequence: 1,
        }
    }
}

impl BlockingRead {
    fn push(&self, bytes: Vec<u8>) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        let bytes = sequence_blocking_read_frames(bytes, &mut state.next_frame_sequence);
        state.bytes.extend(bytes);
        cvar.notify_all();
    }

    fn push_raw(&self, bytes: Vec<u8>) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.bytes.extend(bytes);
        cvar.notify_all();
    }

    fn close(&self) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.closed = true;
        cvar.notify_all();
    }
}

fn sequence_blocking_read_frames(bytes: Vec<u8>, next_sequence: &mut u64) -> Vec<u8> {
    let mut cursor = Cursor::new(bytes.as_slice());
    let mut frames = Vec::new();
    loop {
        match protocol_v5::read_frame(&mut cursor) {
            Ok(Some(frame)) => frames.push(frame),
            Ok(None) => break,
            Err(_) => return bytes,
        }
    }
    if cursor.position() != bytes.len() as u64 || frames.is_empty() {
        return bytes;
    }

    let mut encoded = Vec::with_capacity(bytes.len());
    let mut candidate = *next_sequence;
    for mut frame in frames {
        frame.frame_sequence = candidate;
        candidate = candidate
            .checked_add(1)
            .expect("test v5 peer frame sequence exhausted");
        protocol_v5::write_frame(&mut encoded, &frame).unwrap();
    }
    *next_sequence = candidate;
    encoded
}

impl Read for BlockingRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        while state.bytes.is_empty() && !state.closed {
            state = cvar.wait(state).unwrap();
        }
        if state.bytes.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(state.bytes.len());
        for slot in &mut buf[..len] {
            *slot = state.bytes.pop_front().unwrap();
        }
        Ok(len)
    }
}

#[derive(Default)]
struct FragmentingPipeState {
    bytes: VecDeque<u8>,
    closed: bool,
}

#[derive(Clone)]
struct FragmentingPipeControl {
    state: Arc<(StdMutex<FragmentingPipeState>, Condvar)>,
}

impl FragmentingPipeControl {
    fn close(&self) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.closed = true;
        cvar.notify_all();
    }
}

struct FragmentingRead {
    state: Arc<(StdMutex<FragmentingPipeState>, Condvar)>,
    calls: usize,
}

impl Read for FragmentingRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.calls += 1;
        if self.calls.is_multiple_of(7) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "injected fragmented read interruption",
            ));
        }

        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        while state.bytes.is_empty() && !state.closed {
            state = cvar.wait(state).unwrap();
        }
        let Some(byte) = state.bytes.pop_front() else {
            return Ok(0);
        };
        buf[0] = byte;
        Ok(1)
    }
}

struct FragmentingWrite {
    state: Arc<(StdMutex<FragmentingPipeState>, Condvar)>,
    calls: usize,
}

impl Write for FragmentingWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.calls += 1;
        if self.calls.is_multiple_of(11) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "injected fragmented write interruption",
            ));
        }

        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        if state.closed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "fragmenting pipe is closed",
            ));
        }
        state.bytes.push_back(buf[0]);
        cvar.notify_one();
        Ok(1)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

type FragmentingEndpoint = (FragmentingRead, FragmentingWrite);

fn fragmenting_duplex_pair() -> (
    FragmentingEndpoint,
    FragmentingEndpoint,
    [FragmentingPipeControl; 2],
) {
    let client_inbound = Arc::new((
        StdMutex::new(FragmentingPipeState::default()),
        Condvar::new(),
    ));
    let server_inbound = Arc::new((
        StdMutex::new(FragmentingPipeState::default()),
        Condvar::new(),
    ));
    let client = (
        FragmentingRead {
            state: Arc::clone(&client_inbound),
            calls: 0,
        },
        FragmentingWrite {
            state: Arc::clone(&server_inbound),
            calls: 0,
        },
    );
    let server = (
        FragmentingRead {
            state: Arc::clone(&server_inbound),
            calls: 0,
        },
        FragmentingWrite {
            state: Arc::clone(&client_inbound),
            calls: 0,
        },
    );
    let controls = [
        FragmentingPipeControl {
            state: client_inbound,
        },
        FragmentingPipeControl {
            state: server_inbound,
        },
    ];
    (client, server, controls)
}

fn spawn_v5_concurrent_service<B>(
    service: WorkspaceService<B>,
    input: &BlockingRead,
    output: &SharedWrite,
    info: protocol_v5::ServerHandshakeInfo,
) -> std::thread::JoinHandle<()>
where
    B: WorkspaceBackend + 'static,
{
    let service_input = input.clone();
    let service_output = output.clone();
    std::thread::spawn(move || {
        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(service_input, service_output),
                &info,
            )
            .unwrap();
    })
}

fn wait_for_v5_stream_frame(
    output: &SharedWrite,
    stream_id: u64,
    expected_frame_type: protocol_v5::FrameType,
) {
    let started = Instant::now();
    loop {
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
            if frame.stream_id == stream_id && frame.frame_type == expected_frame_type {
                return;
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for {expected_frame_type:?} on stream {stream_id}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_v5_connection_frame_after(
    output: &SharedWrite,
    expected_frame_type: protocol_v5::FrameType,
    after_sequence: u64,
) -> protocol_v5::Frame {
    wait_for_v5_connection_frame_after_with_timeout(
        output,
        expected_frame_type,
        after_sequence,
        Duration::from_secs(2),
    )
}

fn wait_for_v5_connection_frame_after_with_timeout(
    output: &SharedWrite,
    expected_frame_type: protocol_v5::FrameType,
    after_sequence: u64,
    timeout: Duration,
) -> protocol_v5::Frame {
    let started = Instant::now();
    loop {
        let bytes = output.bytes();
        let mut cursor = Cursor::new(bytes);
        while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
            if frame.stream_id == 0
                && frame.frame_type == expected_frame_type
                && frame.frame_sequence > after_sequence
            {
                return frame;
            }
        }
        assert!(
            started.elapsed() < timeout,
            "timed out waiting for {expected_frame_type:?} after frame {after_sequence}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn trigger_v5_client_idle_ping<W>(shared: &RemoteWorkspaceV5Shared<W>) {
    let started = Instant::now();
    loop {
        let armed = {
            let mut heartbeat = shared.heartbeat.lock().unwrap();
            if heartbeat.ping.is_none() {
                heartbeat.last_peer_activity = Instant::now()
                    .checked_sub(heartbeat.idle_ping_interval)
                    .unwrap();
                true
            } else {
                false
            }
        };
        if armed {
            signal_v5_client_heartbeat(shared);
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for the previous client heartbeat to clear"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn v5_frames_bytes(frames: Vec<protocol_v5::Frame>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for frame in frames {
        protocol_v5::write_frame(&mut bytes, &frame).unwrap();
    }
    bytes
}

fn find_v5_request_stream(output: &SharedWrite, method: &str) -> Option<u64> {
    let bytes = output.bytes();
    let mut cursor = Cursor::new(bytes);
    while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
        if frame.frame_type != protocol_v5::FrameType::Headers {
            continue;
        }
        let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
        if envelope.role == protocol_v5::MessageRole::Request as i32 && envelope.method == method {
            return Some(frame.stream_id);
        }
    }
    None
}

fn wait_for_v5_request_stream(output: &SharedWrite, method: &str) -> u64 {
    let started = Instant::now();
    loop {
        if let Some(stream_id) = find_v5_request_stream(output, method) {
            return stream_id;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for v5 request {method}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_v5_outbound_request_reservation_release<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    stream_id: u64,
) {
    let started = Instant::now();
    loop {
        let retained = shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .contains_key(&stream_id);
        if !retained {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for v5 request reservation on stream {stream_id} to release"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn find_v5_request_stream_after(
    output: &SharedWrite,
    method: &str,
    after_stream_id: u64,
) -> Option<u64> {
    let bytes = output.bytes();
    let mut cursor = Cursor::new(bytes);
    while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
        if frame.stream_id <= after_stream_id || frame.frame_type != protocol_v5::FrameType::Headers
        {
            continue;
        }
        let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
        if envelope.role == protocol_v5::MessageRole::Request as i32 && envelope.method == method {
            return Some(frame.stream_id);
        }
    }
    None
}

fn wait_for_v5_request_stream_after(
    output: &SharedWrite,
    method: &str,
    after_stream_id: u64,
) -> u64 {
    let started = Instant::now();
    loop {
        if let Some(stream_id) = find_v5_request_stream_after(output, method, after_stream_id) {
            return stream_id;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for v5 request {method} after stream {after_stream_id}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn decode_v5_request_payload<T>(output: &SharedWrite, stream_id: u64) -> Option<T>
where
    T: DeserializeOwned,
{
    let bytes = output.bytes();
    let mut cursor = Cursor::new(bytes);
    let mut payload = Vec::new();
    let mut content_encoding = protocol_v5::ContentEncoding::None;
    while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
        if frame.stream_id != stream_id {
            continue;
        }
        if frame.frame_type == protocol_v5::FrameType::Headers {
            let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
            content_encoding = envelope.decode_content_encoding().ok()?;
            continue;
        }
        if frame.frame_type != protocol_v5::FrameType::Data {
            continue;
        }
        let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().ok()?;
        if protocol_v5::DataChannel::try_from(envelope.channel).ok()?
            == protocol_v5::DataChannel::Unspecified
        {
            match content_encoding {
                protocol_v5::ContentEncoding::None => payload.extend_from_slice(&frame.body),
                protocol_v5::ContentEncoding::Zstd => {
                    let len = usize::try_from(envelope.uncompressed_len).ok()?;
                    let decoded = zstd::bulk::decompress(&frame.body, len).ok()?;
                    payload.extend_from_slice(&decoded);
                }
            }
        }
    }
    serde_json::from_slice(&payload).ok()
}

fn decode_v5_protobuf_request<T>(output: &SharedWrite, stream_id: u64) -> Option<T>
where
    T: ProstMessage + Default,
{
    let bytes = output.bytes();
    let mut cursor = Cursor::new(bytes);
    let mut payload = Vec::new();
    let mut content_encoding = protocol_v5::ContentEncoding::None;
    while let Some(frame) = protocol_v5::read_frame(&mut cursor).ok()? {
        if frame.stream_id != stream_id {
            continue;
        }
        if frame.frame_type == protocol_v5::FrameType::Headers {
            let envelope = frame.decode_control::<protocol_v5::StreamEnvelope>().ok()?;
            content_encoding = envelope.decode_content_encoding().ok()?;
            continue;
        }
        if frame.frame_type != protocol_v5::FrameType::Data {
            continue;
        }
        let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().ok()?;
        if protocol_v5::DataChannel::try_from(envelope.channel).ok()?
            == protocol_v5::DataChannel::Unspecified
        {
            match content_encoding {
                protocol_v5::ContentEncoding::None => payload.extend_from_slice(&frame.body),
                protocol_v5::ContentEncoding::Zstd => {
                    let len = usize::try_from(envelope.uncompressed_len).ok()?;
                    let decoded = zstd::bulk::decompress(&frame.body, len).ok()?;
                    payload.extend_from_slice(&decoded);
                }
            }
        }
    }
    T::decode(payload.as_slice()).ok()
}

#[derive(Clone)]
struct ConcurrentV5Backend {
    state: Arc<(StdMutex<ConcurrentV5State>, Condvar)>,
}

#[derive(Default)]
struct ConcurrentV5State {
    stat_seen: bool,
    list_dir_calls: Vec<PathBuf>,
    first_list_cancellation: Option<WorkspaceCancellationToken>,
    release_first_list: bool,
    return_list_cancelled: bool,
}

impl ConcurrentV5Backend {
    fn new() -> Self {
        Self {
            state: Arc::new((StdMutex::new(ConcurrentV5State::default()), Condvar::new())),
        }
    }

    fn unsupported<T>(
        &self,
        operation: &'static str,
        path: &Path,
    ) -> nucleotide_workspace::Result<T> {
        Err(WorkspaceError::Remote {
            operation,
            path: path.to_path_buf(),
            message: "unsupported by concurrent v5 test backend".to_string(),
            diagnostic: None,
        })
    }

    fn wait_for_first_list_cancellation(&self) -> WorkspaceCancellationToken {
        let (lock, cvar) = &*self.state;
        let state = lock.lock().unwrap();
        let (state, _) = cvar
            .wait_timeout_while(state, Duration::from_secs(2), |state| {
                state.first_list_cancellation.is_none()
            })
            .unwrap();
        state
            .first_list_cancellation
            .clone()
            .expect("first list_dir call did not start")
    }

    fn release_first_list(&self) {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.release_first_list = true;
        cvar.notify_all();
    }

    fn list_dir_calls(&self) -> Vec<PathBuf> {
        self.state.0.lock().unwrap().list_dir_calls.clone()
    }

    fn return_list_cancelled(&self) {
        self.state.0.lock().unwrap().return_list_cancelled = true;
    }
}

#[async_trait]
impl WorkspaceBackend for ConcurrentV5Backend {
    fn identity(&self) -> WorkspaceIdentity {
        WorkspaceIdentity::Remote(loopback_identity())
    }

    async fn stat(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.stat_seen = true;
        cvar.notify_all();
        Ok(FileStat {
            path: path.to_path_buf(),
            kind: FileKind::File,
            size: 4,
            modified: None,
            readonly: false,
        })
    }

    async fn list_dir(&self, path: &Path) -> nucleotide_workspace::Result<DirectoryListing> {
        self.unsupported("list directory", path)
    }

    async fn list_dir_with_cancellation(
        &self,
        path: &Path,
        cancellation: &WorkspaceCancellationToken,
    ) -> nucleotide_workspace::Result<DirectoryListing> {
        let (lock, cvar) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.list_dir_calls.push(path.to_path_buf());
        if state.return_list_cancelled {
            return Err(WorkspaceError::Cancelled {
                operation: "list directory",
                path: path.to_path_buf(),
            });
        }
        if state.list_dir_calls.len() == 1 {
            state.first_list_cancellation = Some(cancellation.clone());
            cvar.notify_all();
            while !state.release_first_list {
                state = cvar.wait(state).unwrap();
            }
        }
        drop(state);
        cancellation.check_cancelled("list directory", path)?;
        Ok(DirectoryListing {
            path: path.to_path_buf(),
            entries: Vec::new(),
        })
    }

    async fn find_ancestor_file(
        &self,
        start: &Path,
        _file_name: &str,
        _limit: usize,
    ) -> nucleotide_workspace::Result<Option<PathBuf>> {
        self.unsupported("find ancestor file", start)
    }

    async fn create_file(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        self.unsupported("create file", path)
    }

    async fn create_dir(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        self.unsupported("create directory", path)
    }

    async fn rename_path(&self, from: &Path, _to: &Path) -> nucleotide_workspace::Result<FileStat> {
        self.unsupported("rename path", from)
    }

    async fn delete_path(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        self.unsupported("delete path", path)
    }

    async fn copy_path(&self, from: &Path, _to: &Path) -> nucleotide_workspace::Result<FileStat> {
        self.unsupported("copy path", from)
    }

    async fn read_file(
        &self,
        path: &Path,
        _options: ReadOptions,
    ) -> nucleotide_workspace::Result<FileRead> {
        let (lock, cvar) = &*self.state;
        let state = lock.lock().unwrap();
        let (state, _) = cvar
            .wait_timeout_while(state, Duration::from_secs(2), |state| !state.stat_seen)
            .unwrap();
        if !state.stat_seen {
            return Err(WorkspaceError::Remote {
                operation: "read file",
                path: path.to_path_buf(),
                message: "stat did not run while read was waiting".to_string(),
                diagnostic: None,
            });
        }
        drop(state);
        std::thread::sleep(Duration::from_millis(50));

        Ok(FileRead {
            path: path.to_path_buf(),
            bytes: b"slow".to_vec(),
            size: 4,
            modified: None,
            readonly: false,
            truncated: false,
        })
    }

    async fn write_file(
        &self,
        path: &Path,
        _bytes: &[u8],
        _options: WriteOptions,
    ) -> nucleotide_workspace::Result<WriteResult> {
        self.unsupported("write file", path)
    }

    async fn file_search(
        &self,
        query: FileSearchQuery,
    ) -> nucleotide_workspace::Result<FileSearchResult> {
        self.unsupported("file search", &query.root)
    }

    async fn text_search(
        &self,
        query: TextSearchQuery,
    ) -> nucleotide_workspace::Result<TextSearchResult> {
        self.unsupported("text search", &query.root)
    }

    async fn project_environment(
        &self,
        root: &Path,
    ) -> nucleotide_workspace::Result<ProjectEnvironmentSnapshot> {
        self.unsupported("project environment", root)
    }

    async fn git_head(&self, root: &Path) -> nucleotide_workspace::Result<GitHeadResult> {
        self.unsupported("git head", root)
    }

    async fn git_status(
        &self,
        root: &Path,
        _options: GitStatusOptions,
    ) -> nucleotide_workspace::Result<GitStatusResult> {
        self.unsupported("git status", root)
    }

    async fn run_process(&self, spec: ProcessSpec) -> nucleotide_workspace::Result<ProcessOutput> {
        self.unsupported("run process", &spec.cwd)
    }
}
