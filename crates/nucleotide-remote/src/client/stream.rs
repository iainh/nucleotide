// ABOUTME: Streaming file, search, and process mailboxes for the multiplexed v5 client
// ABOUTME: Delivers bounded events while retaining flow-control credit until consumption

use super::*;

pub(crate) struct V5PendingFileRead {
    pub(crate) mailbox: Arc<V5FileStreamMailbox>,
    pub(crate) payload: Vec<u8>,
    pub(crate) response_reservation: V5ByteReservation,
    pub(crate) final_method: Option<String>,
    pub(crate) final_error: Option<RemoteError>,
    pub(crate) file_bytes: usize,
    pub(crate) method: &'static str,
    pub(crate) deadline: V5RequestDeadline,
}

pub(crate) struct V5PendingProcess {
    pub(crate) mailbox: Arc<V5ProcessStreamMailbox>,
    pub(crate) payload: Vec<u8>,
    pub(crate) payload_bytes: usize,
    pub(crate) payload_credit: u64,
    pub(crate) stdout_bytes: usize,
    pub(crate) stderr_bytes: usize,
    pub(crate) received_bytes: usize,
    pub(crate) final_method: Option<String>,
    pub(crate) final_error: Option<RemoteError>,
    pub(crate) method: &'static str,
    pub(crate) deadline: V5RequestDeadline,
}

pub(crate) struct V5FileStreamMailbox {
    pub(crate) state: Mutex<V5FileStreamMailboxState>,
    waker: AtomicWaker,
    byte_limit: usize,
}

pub(crate) struct V5FileStreamMailboxState {
    pub(crate) chunks: VecDeque<V5FileStreamChunk>,
    pub(crate) queued_bytes: usize,
    pub(crate) queued_credit: u64,
    reservation: V5ByteReservation,
    completion: Option<FileReadResponse>,
    pub(crate) error: Option<RemoteClientError>,
    terminal: bool,
}

pub(crate) struct V5FileStreamChunk {
    pub(crate) body: Vec<u8>,
    credit_bytes: u64,
}

pub(crate) struct V5FileStreamDelivery {
    pub(crate) event: RemoteFileReadEvent,
    pub(crate) credit_bytes: u64,
}

impl V5FileStreamMailbox {
    pub(crate) fn new(byte_limit: usize, reservation: V5ByteReservation) -> Self {
        Self {
            state: Mutex::new(V5FileStreamMailboxState {
                chunks: VecDeque::new(),
                queued_bytes: 0,
                queued_credit: 0,
                reservation,
                completion: None,
                error: None,
                terminal: false,
            }),
            waker: AtomicWaker::new(),
            byte_limit,
        }
    }

    pub(crate) fn push_chunk(
        &self,
        body: Vec<u8>,
        credit_bytes: u64,
    ) -> std::result::Result<(), RemoteClientError> {
        if usize::try_from(credit_bytes).ok() != Some(body.len()) {
            return Err(RemoteClientError::Protocol(format!(
                "v5 file DATA credit {credit_bytes} does not match decoded body length {}",
                body.len()
            )));
        }
        let mut state = self.state.lock().map_err(v5_client_lock_error)?;
        if state.terminal {
            return Err(RemoteClientError::ResponseIncomplete {
                cause: "v5 file DATA arrived after terminal delivery".to_string(),
            });
        }
        let queued_bytes = state.queued_bytes.checked_add(body.len()).ok_or_else(|| {
            RemoteClientError::Protocol("v5 file delivery byte count overflowed".to_string())
        })?;
        if queued_bytes > self.byte_limit {
            return Err(RemoteClientError::Protocol(format!(
                "v5 file delivery exceeds negotiated stream window of {} bytes",
                self.byte_limit
            )));
        }
        let coalesce = state.chunks.back().is_some_and(|chunk| {
            chunk.body.len().saturating_add(body.len()) <= V5_FILE_STREAM_CHUNK_TARGET_BYTES
        });
        if !coalesce && state.chunks.len() >= V5_FILE_STREAM_MAX_QUEUED_CHUNKS {
            return Err(RemoteClientError::Protocol(format!(
                "v5 file delivery exceeds {} queued chunks",
                V5_FILE_STREAM_MAX_QUEUED_CHUNKS
            )));
        }
        let queued_credit = state
            .queued_credit
            .checked_add(credit_bytes)
            .ok_or_else(|| {
                RemoteClientError::Protocol("v5 file delivery credit overflowed".to_string())
            })?;
        let coalesced_credit = if coalesce {
            Some(
                state
                    .chunks
                    .back()
                    .expect("coalesced file delivery chunk should exist")
                    .credit_bytes
                    .checked_add(credit_bytes)
                    .ok_or_else(|| {
                        RemoteClientError::Protocol("v5 file chunk credit overflowed".to_string())
                    })?,
            )
        } else {
            None
        };
        state.reservation.try_grow(body.len()).map_err(|error| {
            RemoteClientError::Protocol(format!(
                "v5 file delivery exceeds connection retained-byte budget: {error}"
            ))
        })?;
        state.queued_bytes = queued_bytes;
        state.queued_credit = queued_credit;
        if coalesce {
            let chunk = state
                .chunks
                .back_mut()
                .expect("coalesced file delivery chunk should exist");
            chunk.body.extend(body);
            chunk.credit_bytes =
                coalesced_credit.expect("coalesced file delivery credit should be precomputed");
        } else {
            state
                .chunks
                .push_back(V5FileStreamChunk { body, credit_bytes });
        }
        drop(state);
        self.waker.wake();
        Ok(())
    }

    pub(crate) fn has_pending_delivery(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !state.chunks.is_empty() || state.completion.is_some()
    }

    pub(crate) fn complete(
        &self,
        completion: FileReadResponse,
    ) -> std::result::Result<(), RemoteClientError> {
        let mut state = self.state.lock().map_err(v5_client_lock_error)?;
        if state.terminal {
            return Err(RemoteClientError::Protocol(
                "v5 file stream completed more than once".to_string(),
            ));
        }
        state.completion = Some(completion);
        state.terminal = true;
        drop(state);
        self.waker.wake();
        Ok(())
    }

    pub(crate) fn fail(&self, error: RemoteClientError) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.terminal
            && state.error.is_none()
            && state.chunks.is_empty()
            && state.completion.is_none()
        {
            return 0;
        }
        let credit_bytes = state.queued_credit;
        state.chunks.clear();
        state.queued_bytes = 0;
        state.queued_credit = 0;
        state.reservation.release_all();
        state.completion = None;
        state.error = Some(error);
        state.terminal = true;
        drop(state);
        self.waker.wake();
        credit_bytes
    }

    pub(crate) fn poll_delivery(
        &self,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<std::result::Result<V5FileStreamDelivery, RemoteClientError>>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(chunk) = state.chunks.pop_front() {
            state.queued_bytes = state.queued_bytes.saturating_sub(chunk.body.len());
            state.queued_credit = state.queued_credit.saturating_sub(chunk.credit_bytes);
            state.reservation.release(chunk.body.len());
            return Poll::Ready(Some(Ok(V5FileStreamDelivery {
                event: RemoteFileReadEvent::Chunk(chunk.body),
                credit_bytes: chunk.credit_bytes,
            })));
        }
        if let Some(completion) = state.completion.take() {
            return Poll::Ready(Some(Ok(V5FileStreamDelivery {
                event: RemoteFileReadEvent::Complete(completion),
                credit_bytes: 0,
            })));
        }
        if let Some(error) = state.error.take() {
            return Poll::Ready(Some(Err(error)));
        }
        if state.terminal {
            return Poll::Ready(None);
        }
        self.waker.register(context.waker());
        Poll::Pending
    }
}

pub(crate) struct V5RemoteFileReadSource<W> {
    pub(crate) shared: Weak<RemoteWorkspaceV5Shared<W>>,
    pub(crate) mailbox: Arc<V5FileStreamMailbox>,
    pub(crate) stream_id: u64,
    pub(crate) finished: bool,
}

pub(crate) enum V5SearchWireEvent {
    FileBatch(Vec<PathBuf>),
    TextBatch(Vec<TextSearchMatchResponse>),
    FileComplete { root: PathBuf, truncated: bool },
    TextComplete { root: PathBuf, truncated: bool },
}

pub(crate) struct V5PendingSearch {
    pub(crate) mailbox: Arc<V5SearchStreamMailbox>,
    pub(crate) current_method: Option<String>,
    pub(crate) current_payload: Vec<u8>,
    pub(crate) current_credit: u64,
    pub(crate) current_bytes: usize,
    pub(crate) final_method: Option<String>,
    pub(crate) final_payload: Vec<u8>,
    pub(crate) final_credit: u64,
    pub(crate) final_bytes: usize,
    pub(crate) final_error: Option<RemoteError>,
    pub(crate) received_bytes: usize,
    pub(crate) method: &'static str,
    pub(crate) deadline: V5RequestDeadline,
}

pub(crate) struct V5SearchStreamMailbox {
    state: Mutex<V5SearchStreamMailboxState>,
    waker: AtomicWaker,
    byte_limit: usize,
}

pub(crate) struct V5SearchStreamMailboxState {
    batches: VecDeque<V5SearchStreamBatch>,
    queued_bytes: usize,
    queued_credit: u64,
    reservation: V5ByteReservation,
    completion: Option<V5SearchStreamBatch>,
    error: Option<RemoteClientError>,
    terminal: bool,
}

pub(crate) struct V5SearchStreamBatch {
    event: V5SearchWireEvent,
    retained_bytes: usize,
    credit_bytes: u64,
}

pub(crate) struct V5SearchStreamDelivery {
    event: V5SearchWireEvent,
    credit_bytes: u64,
}

impl V5SearchStreamMailbox {
    pub(crate) fn new(byte_limit: usize, reservation: V5ByteReservation) -> Self {
        Self {
            state: Mutex::new(V5SearchStreamMailboxState {
                batches: VecDeque::new(),
                queued_bytes: 0,
                queued_credit: 0,
                reservation,
                completion: None,
                error: None,
                terminal: false,
            }),
            waker: AtomicWaker::new(),
            byte_limit,
        }
    }

    pub(crate) fn reserve_data(
        &self,
        retained_bytes: usize,
        credit_bytes: u64,
    ) -> std::result::Result<(), RemoteClientError> {
        if usize::try_from(credit_bytes).ok() != Some(retained_bytes) {
            return Err(RemoteClientError::Protocol(format!(
                "v5 search DATA credit {credit_bytes} does not match decoded payload length {retained_bytes}"
            )));
        }
        let mut state = self.state.lock().map_err(v5_client_lock_error)?;
        if state.terminal {
            return Err(RemoteClientError::ResponseIncomplete {
                cause: "v5 search DATA arrived after terminal delivery".to_string(),
            });
        }
        let queued_bytes = state
            .queued_bytes
            .checked_add(retained_bytes)
            .ok_or_else(|| {
                RemoteClientError::Protocol("v5 search delivery byte count overflowed".to_string())
            })?;
        if queued_bytes > self.byte_limit {
            return Err(RemoteClientError::Protocol(format!(
                "v5 search delivery exceeds negotiated stream window of {} bytes",
                self.byte_limit
            )));
        }
        let queued_credit = state
            .queued_credit
            .checked_add(credit_bytes)
            .ok_or_else(|| {
                RemoteClientError::Protocol("v5 search delivery credit overflowed".to_string())
            })?;
        state
            .reservation
            .try_grow(retained_bytes)
            .map_err(|error| {
                RemoteClientError::Protocol(format!(
                    "v5 search delivery exceeds connection retained-byte budget: {error}"
                ))
            })?;
        state.queued_bytes = queued_bytes;
        state.queued_credit = queued_credit;
        Ok(())
    }

    pub(crate) fn queued_credit(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .queued_credit
    }

    pub(crate) fn has_pending_delivery(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !state.batches.is_empty() || state.completion.is_some()
    }

    pub(crate) fn push_batch(
        &self,
        event: V5SearchWireEvent,
        retained_bytes: usize,
        credit_bytes: u64,
    ) -> std::result::Result<(), RemoteClientError> {
        let mut state = self.state.lock().map_err(v5_client_lock_error)?;
        if state.terminal {
            return Err(RemoteClientError::ResponseIncomplete {
                cause: "v5 search batch arrived after terminal delivery".to_string(),
            });
        }
        let can_coalesce = state.batches.back().is_some_and(|batch| {
            batch.retained_bytes.saturating_add(retained_bytes) <= V5_FILE_STREAM_CHUNK_TARGET_BYTES
                && matches!(
                    (&batch.event, &event),
                    (
                        V5SearchWireEvent::FileBatch(_),
                        V5SearchWireEvent::FileBatch(_)
                    ) | (
                        V5SearchWireEvent::TextBatch(_),
                        V5SearchWireEvent::TextBatch(_)
                    )
                )
        });
        if !can_coalesce && state.batches.len() >= V5_FILE_STREAM_MAX_QUEUED_CHUNKS {
            return Err(RemoteClientError::Protocol(format!(
                "v5 search delivery exceeds {} queued batches",
                V5_FILE_STREAM_MAX_QUEUED_CHUNKS
            )));
        }
        if can_coalesce {
            let batch = state
                .batches
                .back_mut()
                .expect("coalesced search batch should exist");
            match (&mut batch.event, event) {
                (
                    V5SearchWireEvent::FileBatch(existing),
                    V5SearchWireEvent::FileBatch(mut next),
                ) => {
                    existing.append(&mut next);
                }
                (
                    V5SearchWireEvent::TextBatch(existing),
                    V5SearchWireEvent::TextBatch(mut next),
                ) => {
                    existing.append(&mut next);
                }
                _ => unreachable!("search batch kinds were checked before coalescing"),
            }
            batch.retained_bytes = batch
                .retained_bytes
                .checked_add(retained_bytes)
                .ok_or_else(|| {
                    RemoteClientError::Protocol(
                        "v5 search batch retained byte count overflowed".to_string(),
                    )
                })?;
            batch.credit_bytes = batch
                .credit_bytes
                .checked_add(credit_bytes)
                .ok_or_else(|| {
                    RemoteClientError::Protocol("v5 search batch credit overflowed".to_string())
                })?;
        } else {
            state.batches.push_back(V5SearchStreamBatch {
                event,
                retained_bytes,
                credit_bytes,
            });
        }
        drop(state);
        self.waker.wake();
        Ok(())
    }

    pub(crate) fn complete(
        &self,
        event: V5SearchWireEvent,
        retained_bytes: usize,
        credit_bytes: u64,
    ) -> std::result::Result<(), RemoteClientError> {
        let mut state = self.state.lock().map_err(v5_client_lock_error)?;
        if state.terminal {
            return Err(RemoteClientError::Protocol(
                "v5 search stream completed more than once".to_string(),
            ));
        }
        state.completion = Some(V5SearchStreamBatch {
            event,
            retained_bytes,
            credit_bytes,
        });
        state.terminal = true;
        drop(state);
        self.waker.wake();
        Ok(())
    }

    pub(crate) fn fail(&self, error: RemoteClientError) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.terminal
            && state.error.is_none()
            && state.batches.is_empty()
            && state.completion.is_none()
        {
            return 0;
        }
        let credit_bytes = state.queued_credit;
        state.batches.clear();
        state.queued_bytes = 0;
        state.queued_credit = 0;
        state.reservation.release_all();
        state.completion = None;
        state.error = Some(error);
        state.terminal = true;
        drop(state);
        self.waker.wake();
        credit_bytes
    }

    fn poll_delivery(
        &self,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<std::result::Result<V5SearchStreamDelivery, RemoteClientError>>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(batch) = state.batches.pop_front() {
            state.queued_bytes = state.queued_bytes.saturating_sub(batch.retained_bytes);
            state.queued_credit = state.queued_credit.saturating_sub(batch.credit_bytes);
            state.reservation.release(batch.retained_bytes);
            return Poll::Ready(Some(Ok(V5SearchStreamDelivery {
                event: batch.event,
                credit_bytes: batch.credit_bytes,
            })));
        }
        if let Some(completion) = state.completion.take() {
            state.queued_bytes = state.queued_bytes.saturating_sub(completion.retained_bytes);
            state.queued_credit = state.queued_credit.saturating_sub(completion.credit_bytes);
            state.reservation.release(completion.retained_bytes);
            return Poll::Ready(Some(Ok(V5SearchStreamDelivery {
                event: completion.event,
                credit_bytes: completion.credit_bytes,
            })));
        }
        if let Some(error) = state.error.take() {
            return Poll::Ready(Some(Err(error)));
        }
        if state.terminal {
            return Poll::Ready(None);
        }
        self.waker.register(context.waker());
        Poll::Pending
    }
}

pub(crate) struct V5RemoteFileSearchSource<W> {
    pub(crate) shared: Weak<RemoteWorkspaceV5Shared<W>>,
    pub(crate) mailbox: Arc<V5SearchStreamMailbox>,
    pub(crate) stream_id: u64,
    pub(crate) finished: bool,
}

impl<W> Stream for V5RemoteFileSearchSource<W>
where
    W: Write,
{
    type Item = std::result::Result<RemoteFileSearchEvent, RemoteClientError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }
        match self.mailbox.poll_delivery(context) {
            Poll::Ready(Some(Ok(delivery))) => {
                if delivery.credit_bytes > 0
                    && let Some(shared) = self.shared.upgrade()
                    && let Err(error) = queue_v5_released_receive_credit(
                        &shared,
                        self.stream_id,
                        delivery.credit_bytes,
                    )
                {
                    self.finished = true;
                    fail_all_v5_waiters_for_error(&shared, &error);
                    return Poll::Ready(Some(Err(error)));
                }
                let event = match delivery.event {
                    V5SearchWireEvent::FileBatch(files) => RemoteFileSearchEvent::Batch(files),
                    V5SearchWireEvent::FileComplete { root, truncated } => {
                        self.finished = true;
                        if let Some(shared) = self.shared.upgrade() {
                            shared
                                .completed_search_streams
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .remove(&self.stream_id);
                        }
                        RemoteFileSearchEvent::Complete { root, truncated }
                    }
                    _ => {
                        self.finished = true;
                        return Poll::Ready(Some(Err(RemoteClientError::Protocol(
                            "v5 file-search stream received text-search delivery".to_string(),
                        ))));
                    }
                };
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.finished = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.finished = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pub(crate) struct V5RemoteTextSearchSource<W> {
    pub(crate) shared: Weak<RemoteWorkspaceV5Shared<W>>,
    pub(crate) mailbox: Arc<V5SearchStreamMailbox>,
    pub(crate) stream_id: u64,
    pub(crate) finished: bool,
}

impl<W> Stream for V5RemoteTextSearchSource<W>
where
    W: Write,
{
    type Item = std::result::Result<RemoteTextSearchEvent, RemoteClientError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }
        match self.mailbox.poll_delivery(context) {
            Poll::Ready(Some(Ok(delivery))) => {
                if delivery.credit_bytes > 0
                    && let Some(shared) = self.shared.upgrade()
                    && let Err(error) = queue_v5_released_receive_credit(
                        &shared,
                        self.stream_id,
                        delivery.credit_bytes,
                    )
                {
                    self.finished = true;
                    fail_all_v5_waiters_for_error(&shared, &error);
                    return Poll::Ready(Some(Err(error)));
                }
                let event = match delivery.event {
                    V5SearchWireEvent::TextBatch(matches) => RemoteTextSearchEvent::Batch(matches),
                    V5SearchWireEvent::TextComplete { root, truncated } => {
                        self.finished = true;
                        if let Some(shared) = self.shared.upgrade() {
                            shared
                                .completed_search_streams
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .remove(&self.stream_id);
                        }
                        RemoteTextSearchEvent::Complete { root, truncated }
                    }
                    _ => {
                        self.finished = true;
                        return Poll::Ready(Some(Err(RemoteClientError::Protocol(
                            "v5 text-search stream received file-search delivery".to_string(),
                        ))));
                    }
                };
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.finished = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.finished = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<W> Stream for V5RemoteFileReadSource<W>
where
    W: Write,
{
    type Item = std::result::Result<RemoteFileReadEvent, RemoteClientError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }
        match self.mailbox.poll_delivery(context) {
            Poll::Ready(Some(Ok(delivery))) => {
                if delivery.credit_bytes > 0
                    && let Some(shared) = self.shared.upgrade()
                    && let Err(error) = queue_v5_released_receive_credit(
                        &shared,
                        self.stream_id,
                        delivery.credit_bytes,
                    )
                {
                    self.finished = true;
                    fail_all_v5_waiters_for_error(&shared, &error);
                    return Poll::Ready(Some(Err(error)));
                }
                if matches!(delivery.event, RemoteFileReadEvent::Complete(_)) {
                    self.finished = true;
                    if let Some(shared) = self.shared.upgrade() {
                        shared
                            .completed_file_streams
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(&self.stream_id);
                    }
                }
                Poll::Ready(Some(Ok(delivery.event)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.finished = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.finished = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pub(crate) struct V5ProcessStreamMailbox {
    state: Mutex<V5ProcessStreamMailboxState>,
    waker: AtomicWaker,
    byte_limit: usize,
}

pub(crate) struct V5ProcessStreamMailboxState {
    chunks: VecDeque<V5ProcessStreamChunk>,
    queued_bytes: usize,
    queued_credit: u64,
    reservation: V5ByteReservation,
    completion: Option<V5ProcessStreamCompletion>,
    error: Option<RemoteClientError>,
    terminal: bool,
}

pub(crate) struct V5ProcessStreamChunk {
    channel: protocol_v5::DataChannel,
    body: Vec<u8>,
    credit_bytes: u64,
}

pub(crate) struct V5ProcessStreamCompletion {
    response: ProcessOutputResponse,
    retained_bytes: usize,
    credit_bytes: u64,
}

pub(crate) struct V5ProcessStreamDelivery {
    event: RemoteProcessEvent,
    credit_bytes: u64,
}

impl V5ProcessStreamMailbox {
    pub(crate) fn new(byte_limit: usize, reservation: V5ByteReservation) -> Self {
        Self {
            state: Mutex::new(V5ProcessStreamMailboxState {
                chunks: VecDeque::new(),
                queued_bytes: 0,
                queued_credit: 0,
                reservation,
                completion: None,
                error: None,
                terminal: false,
            }),
            waker: AtomicWaker::new(),
            byte_limit,
        }
    }

    pub(crate) fn reserve_data(
        &self,
        retained_bytes: usize,
        credit_bytes: u64,
    ) -> std::result::Result<(), RemoteClientError> {
        if usize::try_from(credit_bytes).ok() != Some(retained_bytes) {
            return Err(RemoteClientError::Protocol(format!(
                "v5 process DATA credit {credit_bytes} does not match decoded payload length {retained_bytes}"
            )));
        }
        let mut state = self.state.lock().map_err(v5_client_lock_error)?;
        if state.terminal {
            return Err(RemoteClientError::ResponseIncomplete {
                cause: "v5 process DATA arrived after terminal delivery".to_string(),
            });
        }
        let queued_bytes = state
            .queued_bytes
            .checked_add(retained_bytes)
            .ok_or_else(|| {
                RemoteClientError::Protocol("v5 process delivery byte count overflowed".to_string())
            })?;
        if queued_bytes > self.byte_limit {
            return Err(RemoteClientError::Protocol(format!(
                "v5 process delivery exceeds negotiated stream window of {} bytes",
                self.byte_limit
            )));
        }
        let queued_credit = state
            .queued_credit
            .checked_add(credit_bytes)
            .ok_or_else(|| {
                RemoteClientError::Protocol("v5 process delivery credit overflowed".to_string())
            })?;
        state
            .reservation
            .try_grow(retained_bytes)
            .map_err(|error| {
                RemoteClientError::Protocol(format!(
                    "v5 process delivery exceeds connection retained-byte budget: {error}"
                ))
            })?;
        state.queued_bytes = queued_bytes;
        state.queued_credit = queued_credit;
        Ok(())
    }

    pub(crate) fn push_chunk(
        &self,
        channel: protocol_v5::DataChannel,
        body: Vec<u8>,
        credit_bytes: u64,
    ) -> std::result::Result<(), RemoteClientError> {
        let mut state = self.state.lock().map_err(v5_client_lock_error)?;
        let coalesce = state.chunks.back().is_some_and(|chunk| {
            chunk.channel == channel
                && chunk.body.len().saturating_add(body.len()) <= V5_FILE_STREAM_CHUNK_TARGET_BYTES
        });
        if !coalesce && state.chunks.len() >= V5_FILE_STREAM_MAX_QUEUED_CHUNKS {
            return Err(RemoteClientError::Protocol(format!(
                "v5 process delivery exceeds {} queued chunks",
                V5_FILE_STREAM_MAX_QUEUED_CHUNKS
            )));
        }
        if coalesce {
            let chunk = state
                .chunks
                .back_mut()
                .expect("coalesced process chunk should exist");
            chunk.body.extend(body);
            chunk.credit_bytes = chunk
                .credit_bytes
                .checked_add(credit_bytes)
                .ok_or_else(|| {
                    RemoteClientError::Protocol("v5 process chunk credit overflowed".to_string())
                })?;
        } else {
            state.chunks.push_back(V5ProcessStreamChunk {
                channel,
                body,
                credit_bytes,
            });
        }
        drop(state);
        self.waker.wake();
        Ok(())
    }

    pub(crate) fn complete(
        &self,
        response: ProcessOutputResponse,
        retained_bytes: usize,
        credit_bytes: u64,
    ) -> std::result::Result<(), RemoteClientError> {
        let mut state = self.state.lock().map_err(v5_client_lock_error)?;
        if state.terminal {
            return Err(RemoteClientError::Protocol(
                "v5 process stream completed more than once".to_string(),
            ));
        }
        state.completion = Some(V5ProcessStreamCompletion {
            response,
            retained_bytes,
            credit_bytes,
        });
        state.terminal = true;
        drop(state);
        self.waker.wake();
        Ok(())
    }

    pub(crate) fn queued_credit(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .queued_credit
    }

    pub(crate) fn has_pending_delivery(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !state.chunks.is_empty() || state.completion.is_some()
    }

    pub(crate) fn fail(&self, error: RemoteClientError) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.terminal
            && state.error.is_none()
            && state.chunks.is_empty()
            && state.completion.is_none()
        {
            return 0;
        }
        let credit_bytes = state.queued_credit;
        state.chunks.clear();
        state.queued_bytes = 0;
        state.queued_credit = 0;
        state.reservation.release_all();
        state.completion = None;
        state.error = Some(error);
        state.terminal = true;
        drop(state);
        self.waker.wake();
        credit_bytes
    }

    fn poll_delivery(
        &self,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<std::result::Result<V5ProcessStreamDelivery, RemoteClientError>>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(chunk) = state.chunks.pop_front() {
            state.queued_bytes = state.queued_bytes.saturating_sub(chunk.body.len());
            state.queued_credit = state.queued_credit.saturating_sub(chunk.credit_bytes);
            state.reservation.release(chunk.body.len());
            let event = match chunk.channel {
                protocol_v5::DataChannel::Stdout => RemoteProcessEvent::Stdout(chunk.body),
                protocol_v5::DataChannel::Stderr => RemoteProcessEvent::Stderr(chunk.body),
                _ => {
                    return Poll::Ready(Some(Err(RemoteClientError::Protocol(
                        "v5 process mailbox contained a non-output channel".to_string(),
                    ))));
                }
            };
            return Poll::Ready(Some(Ok(V5ProcessStreamDelivery {
                event,
                credit_bytes: chunk.credit_bytes,
            })));
        }
        if let Some(completion) = state.completion.take() {
            state.queued_bytes = state.queued_bytes.saturating_sub(completion.retained_bytes);
            state.queued_credit = state.queued_credit.saturating_sub(completion.credit_bytes);
            state.reservation.release(completion.retained_bytes);
            return Poll::Ready(Some(Ok(V5ProcessStreamDelivery {
                event: RemoteProcessEvent::Complete(completion.response),
                credit_bytes: completion.credit_bytes,
            })));
        }
        if let Some(error) = state.error.take() {
            return Poll::Ready(Some(Err(error)));
        }
        if state.terminal {
            return Poll::Ready(None);
        }
        self.waker.register(context.waker());
        Poll::Pending
    }
}

pub(crate) struct V5RemoteProcessSource<W> {
    pub(crate) shared: Weak<RemoteWorkspaceV5Shared<W>>,
    pub(crate) mailbox: Arc<V5ProcessStreamMailbox>,
    pub(crate) stream_id: u64,
    pub(crate) finished: bool,
}

impl<W> Stream for V5RemoteProcessSource<W>
where
    W: Write,
{
    type Item = std::result::Result<RemoteProcessEvent, RemoteClientError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }
        match self.mailbox.poll_delivery(context) {
            Poll::Ready(Some(Ok(delivery))) => {
                if delivery.credit_bytes > 0
                    && let Some(shared) = self.shared.upgrade()
                    && let Err(error) = queue_v5_released_receive_credit(
                        &shared,
                        self.stream_id,
                        delivery.credit_bytes,
                    )
                {
                    self.finished = true;
                    fail_all_v5_waiters_for_error(&shared, &error);
                    return Poll::Ready(Some(Err(error)));
                }
                if matches!(delivery.event, RemoteProcessEvent::Complete(_)) {
                    self.finished = true;
                    if let Some(shared) = self.shared.upgrade() {
                        shared
                            .completed_process_streams
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(&self.stream_id);
                    }
                }
                Poll::Ready(Some(Ok(delivery.event)))
            }
            Poll::Ready(Some(Err(error))) => {
                self.finished = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.finished = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
