// ABOUTME: In-flight v5 request state, deadline tracking, and response accumulation
// ABOUTME: Normalizes terminal outcomes for requests and streaming operations

use super::*;

pub(crate) type V5ResponseDelivery =
    std::result::Result<V5Budgeted<(RemoteResponse, Vec<u8>)>, RemoteClientError>;
pub(crate) type V5RawResponseDelivery = std::result::Result<V5Budgeted<Vec<u8>>, RemoteClientError>;

#[must_use = "dropping a live v5 request handle cancels its stream"]
pub struct RemoteWorkspaceV5RequestHandle<W> {
    pub(crate) shared: Arc<RemoteWorkspaceV5Shared<W>>,
    pub(crate) stream_id: u64,
    pub(crate) request: RemoteRequest,
    pub(crate) receiver: mpsc::Receiver<V5ResponseDelivery>,
    pub(crate) cancellation: RemoteRequestCancellation,
    pub(crate) finished: bool,
}

impl<W> RemoteWorkspaceV5RequestHandle<W> {
    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn wait(mut self) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let delivery = match self.receiver.recv() {
            Ok(delivery) => {
                self.finished = true;
                delivery
            }
            Err(_) => return Err(RemoteClientError::Disconnected),
        };
        let (response, body) = delivery?.into_inner();
        let response = apply_v5_directory_cache(&self.shared, &self.request, response)?;
        Ok((response, body))
    }
}

impl<W> Drop for RemoteWorkspaceV5RequestHandle<W> {
    fn drop(&mut self) {
        if !self.finished {
            self.cancellation.cancel();
        }
    }
}

pub(crate) struct V5NormalizedDeadlineResult<T> {
    pub(crate) result: std::result::Result<T, RemoteClientError>,
    pub(crate) peer_deadline: bool,
    pub(crate) terminal: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct V5RequestDeadline {
    context: RemoteRequestContext,
    pub(crate) last_progress_at: Instant,
}

impl V5RequestDeadline {
    pub(crate) fn new(context: RemoteRequestContext, now: Instant) -> Self {
        Self {
            context,
            last_progress_at: now,
        }
    }

    pub(crate) fn next_expiry(self) -> Option<(Instant, RemoteRequestDeadlineKind)> {
        let absolute = self
            .context
            .absolute_deadline
            .map(|deadline| (deadline, RemoteRequestDeadlineKind::Absolute));
        let inactivity = self.context.inactivity_timeout.and_then(|timeout| {
            self.last_progress_at
                .checked_add(timeout)
                .map(|deadline| (deadline, RemoteRequestDeadlineKind::Inactivity))
        });
        match (absolute, inactivity) {
            (Some(absolute), Some(inactivity)) => {
                if absolute.0 <= inactivity.0 {
                    Some(absolute)
                } else {
                    Some(inactivity)
                }
            }
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        }
    }

    pub(crate) fn expired_at(self, now: Instant) -> Option<RemoteRequestDeadlineKind> {
        self.next_expiry()
            .filter(|(deadline, _)| now >= *deadline)
            .map(|(_, kind)| kind)
    }

    pub(crate) fn observe_progress(&mut self, now: Instant) {
        if now > self.last_progress_at {
            self.last_progress_at = now;
        }
    }
}

pub(crate) struct V5PendingResponse {
    pub(crate) sender: mpsc::Sender<V5ResponseDelivery>,
    pub(crate) accumulator: V5ResponseAccumulator,
    pub(crate) response_reservation: V5ByteReservation,
    pub(crate) method: &'static str,
    pub(crate) idempotency: protocol_v5::Idempotency,
    pub(crate) terminal_on_deadline: bool,
    pub(crate) deadline: V5RequestDeadline,
}

pub(crate) struct V5PendingRawResponse {
    pub(crate) sender: mpsc::Sender<V5RawResponseDelivery>,
    pub(crate) accumulator: V5RawResponseAccumulator,
    pub(crate) response_reservation: V5ByteReservation,
    pub(crate) method: &'static str,
    pub(crate) deadline: V5RequestDeadline,
}

impl V5PendingResponse {
    pub(crate) fn deadline_is_connection_terminal(&self) -> bool {
        self.terminal_on_deadline
            && (self.method == "session.shutdown" || !self.accumulator.final_message_seen())
    }

    pub(crate) fn failure_error(&self, error: RemoteClientError) -> RemoteClientError {
        if self.accumulator.final_message_seen() {
            disconnect_after_final_response_error(error)
        } else {
            let error = transport_closed_before_final_error(error);
            if self.idempotency != protocol_v5::Idempotency::ReadOnly
                && remote_client_error_allows_reconnect_retry(&error)
            {
                RemoteClientError::OutcomeUnknown {
                    method: self.method.to_string(),
                    cause: error.to_string(),
                }
            } else {
                error
            }
        }
    }
}

impl V5PendingRawResponse {
    pub(crate) fn failure_error(&self, error: RemoteClientError) -> RemoteClientError {
        if self.accumulator.final_message_seen() {
            disconnect_after_final_response_error(error)
        } else {
            transport_closed_before_final_error(error)
        }
    }
}

pub(crate) enum V5FileStreamObservation {
    Continue { acknowledge_data: bool },
    Complete(FileReadResponse),
}

impl V5PendingFileRead {
    pub(crate) fn observe(
        &mut self,
        event: protocol_v5::StreamEvent,
        data_credit: Option<(u64, u64)>,
    ) -> std::result::Result<V5FileStreamObservation, RemoteClientError> {
        match event {
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalResponse,
                envelope,
                ..
            } => {
                if self.final_method.is_some() || self.final_error.is_some() {
                    return Err(RemoteClientError::Protocol(
                        "v5 file stream received duplicate final headers".to_string(),
                    ));
                }
                if envelope.method != self.method {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 file stream expected {} response, received {}",
                        self.method, envelope.method
                    )));
                }
                self.final_method = Some(envelope.method);
                Ok(V5FileStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalError,
                envelope,
                ..
            } => {
                if self.final_method.is_some() || self.final_error.is_some() {
                    return Err(RemoteClientError::Protocol(
                        "v5 file stream received duplicate final headers".to_string(),
                    ));
                }
                if envelope.method != self.method {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 file stream expected {} error, received {}",
                        self.method, envelope.method
                    )));
                }
                self.final_method = Some(envelope.method.clone());
                self.final_error = Some(v5_final_error_from_envelope(envelope)?);
                Ok(V5FileStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
            protocol_v5::StreamEvent::Data {
                channel: protocol_v5::DataChannel::FileBody,
                body,
                ..
            } => {
                let (_, credit_bytes) = data_credit.ok_or_else(|| {
                    RemoteClientError::Protocol(
                        "v5 file DATA did not carry receive credit".to_string(),
                    )
                })?;
                let file_bytes = self.file_bytes.checked_add(body.len()).ok_or_else(|| {
                    RemoteClientError::Protocol("v5 file byte count overflowed".to_string())
                })?;
                if u64::try_from(file_bytes).unwrap_or(u64::MAX) > V5_MAX_STREAMED_FILE_READ_BYTES {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 file stream exceeds decoded byte limit of \
                         {V5_MAX_STREAMED_FILE_READ_BYTES}"
                    )));
                }
                self.mailbox.push_chunk(body, credit_bytes)?;
                self.file_bytes = file_bytes;
                Ok(V5FileStreamObservation::Continue {
                    acknowledge_data: false,
                })
            }
            protocol_v5::StreamEvent::Data {
                channel: protocol_v5::DataChannel::Unspecified,
                body,
                ..
            } => {
                let payload_len = self.payload.len().checked_add(body.len()).ok_or_else(|| {
                    RemoteClientError::Protocol(
                        "v5 file response payload length overflowed".to_string(),
                    )
                })?;
                if payload_len > V5_MAX_REQUEST_PAYLOAD_BYTES {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 file response payload exceeds decoded byte limit of \
                         {V5_MAX_REQUEST_PAYLOAD_BYTES}"
                    )));
                }
                self.response_reservation
                    .try_grow(body.len())
                    .map_err(|error| {
                        RemoteClientError::Protocol(format!(
                            "v5 file response metadata exceeds connection retained-byte budget: \
                             {error}"
                        ))
                    })?;
                self.payload.extend(body);
                Ok(V5FileStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
            protocol_v5::StreamEvent::EndStream { stream_id } => {
                if let Some(error) = self.final_error.take() {
                    return Err(RemoteClientError::Remote(error));
                }
                let Some(method) = self.final_method.take() else {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 file stream {stream_id} ended without final response"
                    )));
                };
                let response = RemoteResponse::from_v5_payload(&method, &self.payload)
                    .map_err(v5_method_error_to_client_error)?;
                let RemoteResponse::ReadFile(read) = response else {
                    return Err(RemoteClientError::Protocol(format!(
                        "unexpected v5 file stream final response: {response:?}"
                    )));
                };
                validate_file_read_body(&read, self.file_bytes)?;
                Ok(V5FileStreamObservation::Complete(read))
            }
            protocol_v5::StreamEvent::ResetStream {
                code, diagnostic, ..
            } => Err(RemoteClientError::Remote(RemoteError {
                code,
                message: "v5 file stream reset".to_string(),
                diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
            })),
            protocol_v5::StreamEvent::Data { channel, .. } => Err(RemoteClientError::Protocol(
                format!("unexpected {channel:?} DATA on v5 file stream"),
            )),
            protocol_v5::StreamEvent::Headers { role, .. } => Err(RemoteClientError::Protocol(
                format!("unexpected {role:?} headers on v5 file stream"),
            )),
        }
    }
}

pub(crate) enum V5ProcessStreamObservation {
    Continue { acknowledge_data: bool },
    Complete,
}

impl V5PendingProcess {
    fn reserve_data(
        &mut self,
        body_len: usize,
        data_credit: Option<(u64, u64)>,
    ) -> std::result::Result<u64, RemoteClientError> {
        let (_, credit_bytes) = data_credit.ok_or_else(|| {
            RemoteClientError::Protocol("v5 process DATA did not carry receive credit".to_string())
        })?;
        let received_bytes = self.received_bytes.checked_add(body_len).ok_or_else(|| {
            RemoteClientError::Protocol("v5 process decoded byte count overflowed".to_string())
        })?;
        if received_bytes > V5_MAX_ACCUMULATED_RESPONSE_BYTES {
            return Err(RemoteClientError::Protocol(format!(
                "v5 process response exceeds decoded byte limit of {V5_MAX_ACCUMULATED_RESPONSE_BYTES}"
            )));
        }
        self.mailbox.reserve_data(body_len, credit_bytes)?;
        self.received_bytes = received_bytes;
        Ok(credit_bytes)
    }

    pub(crate) fn observe(
        &mut self,
        event: protocol_v5::StreamEvent,
        data_credit: Option<(u64, u64)>,
    ) -> std::result::Result<V5ProcessStreamObservation, RemoteClientError> {
        match event {
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalResponse,
                envelope,
                ..
            } => {
                if self.final_method.is_some() || self.final_error.is_some() {
                    return Err(RemoteClientError::Protocol(
                        "v5 process stream received duplicate final headers".to_string(),
                    ));
                }
                if envelope.method != self.method {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 process stream expected {} response, received {}",
                        self.method, envelope.method
                    )));
                }
                self.final_method = Some(envelope.method);
                Ok(V5ProcessStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalError,
                envelope,
                ..
            } => {
                if self.final_method.is_some() || self.final_error.is_some() {
                    return Err(RemoteClientError::Protocol(
                        "v5 process stream received duplicate final headers".to_string(),
                    ));
                }
                if envelope.method != self.method {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 process stream expected {} error, received {}",
                        self.method, envelope.method
                    )));
                }
                self.final_method = Some(envelope.method.clone());
                self.final_error = Some(v5_final_error_from_envelope(envelope)?);
                Ok(V5ProcessStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
            protocol_v5::StreamEvent::Data {
                channel:
                    channel @ (protocol_v5::DataChannel::Stdout | protocol_v5::DataChannel::Stderr),
                body,
                ..
            } => {
                let body_len = body.len();
                let credit_bytes = self.reserve_data(body_len, data_credit)?;
                let total = match channel {
                    protocol_v5::DataChannel::Stdout => &mut self.stdout_bytes,
                    protocol_v5::DataChannel::Stderr => &mut self.stderr_bytes,
                    _ => unreachable!(),
                };
                *total = total.checked_add(body_len).ok_or_else(|| {
                    RemoteClientError::Protocol(
                        "v5 process output byte count overflowed".to_string(),
                    )
                })?;
                self.mailbox.push_chunk(channel, body, credit_bytes)?;
                Ok(V5ProcessStreamObservation::Continue {
                    acknowledge_data: false,
                })
            }
            protocol_v5::StreamEvent::Data {
                channel: protocol_v5::DataChannel::Unspecified,
                body,
                ..
            } => {
                let body_len = body.len();
                let credit_bytes = self.reserve_data(body_len, data_credit)?;
                self.payload_bytes = self.payload_bytes.checked_add(body_len).ok_or_else(|| {
                    RemoteClientError::Protocol(
                        "v5 process metadata byte count overflowed".to_string(),
                    )
                })?;
                self.payload_credit =
                    self.payload_credit
                        .checked_add(credit_bytes)
                        .ok_or_else(|| {
                            RemoteClientError::Protocol(
                                "v5 process metadata credit overflowed".to_string(),
                            )
                        })?;
                self.payload.extend(body);
                Ok(V5ProcessStreamObservation::Continue {
                    acknowledge_data: false,
                })
            }
            protocol_v5::StreamEvent::EndStream { stream_id } => {
                if let Some(error) = self.final_error.take() {
                    return Err(RemoteClientError::Remote(error));
                }
                let Some(method) = self.final_method.take() else {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 process stream {stream_id} ended without final response"
                    )));
                };
                let response = RemoteResponse::from_v5_payload(&method, &self.payload)
                    .map_err(v5_method_error_to_client_error)?;
                let RemoteResponse::RunProcess(response) = response else {
                    return Err(RemoteClientError::Protocol(format!(
                        "unexpected v5 process stream final response: {response:?}"
                    )));
                };
                if response.stdout_len != self.stdout_bytes
                    || response.stderr_len != self.stderr_bytes
                {
                    return Err(RemoteClientError::Protocol(format!(
                        "malformed run_process stream: header declares stdout_len={} stderr_len={} but received stdout={} stderr={}",
                        response.stdout_len,
                        response.stderr_len,
                        self.stdout_bytes,
                        self.stderr_bytes
                    )));
                }
                self.mailbox.complete(
                    response,
                    std::mem::take(&mut self.payload_bytes),
                    std::mem::take(&mut self.payload_credit),
                )?;
                self.payload.clear();
                Ok(V5ProcessStreamObservation::Complete)
            }
            protocol_v5::StreamEvent::ResetStream {
                code, diagnostic, ..
            } => {
                let error = RemoteError {
                    code,
                    message: "v5 process stream reset".to_string(),
                    diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
                };
                if error.code == protocol_v5::RESET_DEADLINE_EXCEEDED {
                    if self.final_method.is_none() {
                        Err(RemoteClientError::OutcomeUnknown {
                            method: self.method.to_string(),
                            cause: RemoteClientError::Remote(error).to_string(),
                        })
                    } else {
                        Err(RemoteClientError::RequestDeadlineExceeded {
                            method: self.method.to_string(),
                            kind: RemoteRequestDeadlineKind::Absolute,
                        })
                    }
                } else {
                    Err(RemoteClientError::Remote(error))
                }
            }
            protocol_v5::StreamEvent::Data { channel, .. } => Err(RemoteClientError::Protocol(
                format!("unexpected {channel:?} DATA on v5 process stream"),
            )),
            protocol_v5::StreamEvent::Headers { role, .. } => Err(RemoteClientError::Protocol(
                format!("unexpected {role:?} headers on v5 process stream"),
            )),
        }
    }
}

pub(crate) enum V5SearchStreamObservation {
    Continue { acknowledge_data: bool },
    Complete,
}

impl V5PendingSearch {
    fn reserve_payload_data(
        &mut self,
        body_len: usize,
        data_credit: Option<(u64, u64)>,
    ) -> std::result::Result<u64, RemoteClientError> {
        let (_, credit_bytes) = data_credit.ok_or_else(|| {
            RemoteClientError::Protocol("v5 search DATA did not carry receive credit".to_string())
        })?;
        let received_bytes = self.received_bytes.checked_add(body_len).ok_or_else(|| {
            RemoteClientError::Protocol("v5 search decoded byte count overflowed".to_string())
        })?;
        if received_bytes > V5_MAX_ACCUMULATED_RESPONSE_BYTES {
            return Err(RemoteClientError::Protocol(format!(
                "v5 search response exceeds decoded byte limit of {V5_MAX_ACCUMULATED_RESPONSE_BYTES}"
            )));
        }
        self.mailbox.reserve_data(body_len, credit_bytes)?;
        self.received_bytes = received_bytes;
        Ok(credit_bytes)
    }

    fn finish_current(&mut self) -> std::result::Result<(), RemoteClientError> {
        let Some(method) = self.current_method.take() else {
            return Ok(());
        };
        let payload = std::mem::take(&mut self.current_payload);
        let retained_bytes = std::mem::take(&mut self.current_bytes);
        let credit_bytes = std::mem::take(&mut self.current_credit);
        let response = RemoteResponse::from_v5_payload(&method, &payload)
            .map_err(v5_method_error_to_client_error)?;
        let event = match (self.method, response) {
            ("search.files", RemoteResponse::FileSearch(response)) => {
                V5SearchWireEvent::FileBatch(response.files)
            }
            ("search.text", RemoteResponse::TextSearch(response)) => {
                V5SearchWireEvent::TextBatch(response.matches)
            }
            (_, other) => {
                return Err(RemoteClientError::Protocol(format!(
                    "unexpected v5 search partial response: {other:?}"
                )));
            }
        };
        self.mailbox.push_batch(event, retained_bytes, credit_bytes)
    }

    fn finish_final(&mut self) -> std::result::Result<(), RemoteClientError> {
        self.finish_current()?;
        if let Some(error) = self.final_error.take() {
            return Err(RemoteClientError::Remote(error));
        }
        let Some(method) = self.final_method.take() else {
            return Err(RemoteClientError::Protocol(
                "v5 search stream ended without final response".to_string(),
            ));
        };
        if method != self.method {
            return Err(RemoteClientError::Protocol(format!(
                "v5 search stream expected {} response, received {method}",
                self.method
            )));
        }
        let response = RemoteResponse::from_v5_payload(&method, &self.final_payload)
            .map_err(v5_method_error_to_client_error)?;
        let retained_bytes = std::mem::take(&mut self.final_bytes);
        let credit_bytes = std::mem::take(&mut self.final_credit);
        match response {
            RemoteResponse::FileSearch(response) if self.method == "search.files" => {
                let FileSearchResponse {
                    root,
                    files,
                    truncated,
                } = response;
                if files.is_empty() {
                    self.mailbox.complete(
                        V5SearchWireEvent::FileComplete { root, truncated },
                        retained_bytes,
                        credit_bytes,
                    )
                } else {
                    self.mailbox.push_batch(
                        V5SearchWireEvent::FileBatch(files),
                        retained_bytes,
                        credit_bytes,
                    )?;
                    self.mailbox
                        .complete(V5SearchWireEvent::FileComplete { root, truncated }, 0, 0)
                }
            }
            RemoteResponse::TextSearch(response) if self.method == "search.text" => {
                let TextSearchResponse {
                    root,
                    matches,
                    truncated,
                } = response;
                if matches.is_empty() {
                    self.mailbox.complete(
                        V5SearchWireEvent::TextComplete { root, truncated },
                        retained_bytes,
                        credit_bytes,
                    )
                } else {
                    self.mailbox.push_batch(
                        V5SearchWireEvent::TextBatch(matches),
                        retained_bytes,
                        credit_bytes,
                    )?;
                    self.mailbox
                        .complete(V5SearchWireEvent::TextComplete { root, truncated }, 0, 0)
                }
            }
            other => Err(RemoteClientError::Protocol(format!(
                "unexpected v5 search final response: {other:?}"
            ))),
        }
    }

    pub(crate) fn observe(
        &mut self,
        event: protocol_v5::StreamEvent,
        data_credit: Option<(u64, u64)>,
    ) -> std::result::Result<V5SearchStreamObservation, RemoteClientError> {
        match event {
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::PartialResult,
                envelope,
                ..
            } => {
                self.finish_current()?;
                if envelope.method != self.method {
                    return Err(RemoteClientError::Protocol(format!(
                        "v5 search stream expected {}, received partial {}",
                        self.method, envelope.method
                    )));
                }
                self.current_method = Some(envelope.method);
                Ok(V5SearchStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalResponse,
                envelope,
                ..
            } => {
                self.finish_current()?;
                if self.final_method.is_some() || self.final_error.is_some() {
                    return Err(RemoteClientError::Protocol(
                        "v5 search stream received duplicate final headers".to_string(),
                    ));
                }
                self.final_method = Some(envelope.method);
                Ok(V5SearchStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalError,
                envelope,
                ..
            } => {
                self.finish_current()?;
                if self.final_method.is_some() || self.final_error.is_some() {
                    return Err(RemoteClientError::Protocol(
                        "v5 search stream received duplicate final headers".to_string(),
                    ));
                }
                self.final_method = Some(envelope.method.clone());
                self.final_error = Some(v5_final_error_from_envelope(envelope)?);
                Ok(V5SearchStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
            protocol_v5::StreamEvent::Data {
                channel: protocol_v5::DataChannel::SearchPayload,
                body,
                ..
            } => {
                if self.current_method.is_none() {
                    return Err(RemoteClientError::Protocol(
                        "v5 search payload arrived without partial-result headers".to_string(),
                    ));
                }
                let credit_bytes = self.reserve_payload_data(body.len(), data_credit)?;
                self.current_bytes =
                    self.current_bytes.checked_add(body.len()).ok_or_else(|| {
                        RemoteClientError::Protocol(
                            "v5 partial search payload length overflowed".to_string(),
                        )
                    })?;
                self.current_credit =
                    self.current_credit
                        .checked_add(credit_bytes)
                        .ok_or_else(|| {
                            RemoteClientError::Protocol(
                                "v5 partial search payload credit overflowed".to_string(),
                            )
                        })?;
                self.current_payload.extend(body);
                Ok(V5SearchStreamObservation::Continue {
                    acknowledge_data: false,
                })
            }
            protocol_v5::StreamEvent::Data {
                channel: protocol_v5::DataChannel::Unspecified,
                body,
                ..
            } => {
                let credit_bytes = self.reserve_payload_data(body.len(), data_credit)?;
                self.final_bytes = self.final_bytes.checked_add(body.len()).ok_or_else(|| {
                    RemoteClientError::Protocol(
                        "v5 final search payload length overflowed".to_string(),
                    )
                })?;
                self.final_credit =
                    self.final_credit.checked_add(credit_bytes).ok_or_else(|| {
                        RemoteClientError::Protocol(
                            "v5 final search payload credit overflowed".to_string(),
                        )
                    })?;
                self.final_payload.extend(body);
                Ok(V5SearchStreamObservation::Continue {
                    acknowledge_data: false,
                })
            }
            protocol_v5::StreamEvent::EndStream { .. } => {
                self.finish_final()?;
                Ok(V5SearchStreamObservation::Complete)
            }
            protocol_v5::StreamEvent::ResetStream {
                code, diagnostic, ..
            } => Err(RemoteClientError::Remote(RemoteError {
                code,
                message: "v5 search stream reset".to_string(),
                diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
            })),
            protocol_v5::StreamEvent::Data { channel, .. } => Err(RemoteClientError::Protocol(
                format!("unexpected {channel:?} DATA on v5 search stream"),
            )),
            protocol_v5::StreamEvent::Headers { .. } => {
                self.finish_current()?;
                Ok(V5SearchStreamObservation::Continue {
                    acknowledge_data: true,
                })
            }
        }
    }
}

pub(crate) fn reserve_v5_client_request_bytes(
    budget: &V5ConnectionByteBudget,
    method: &str,
    payload_bytes: usize,
    body_bytes: usize,
) -> std::result::Result<V5ByteReservation, RemoteClientError> {
    if payload_bytes > V5_MAX_REQUEST_PAYLOAD_BYTES {
        return Err(RemoteClientError::Protocol(format!(
            "v5 {method} request payload exceeds decoded byte limit {V5_MAX_REQUEST_PAYLOAD_BYTES}"
        )));
    }
    if body_bytes > V5_MAX_REQUEST_BODY_BYTES {
        return Err(RemoteClientError::Protocol(format!(
            "v5 {method} request body exceeds decoded byte limit {V5_MAX_REQUEST_BODY_BYTES}"
        )));
    }
    let retained_bytes = payload_bytes.checked_add(body_bytes).ok_or_else(|| {
        RemoteClientError::Protocol(format!("v5 {method} request decoded byte count overflowed"))
    })?;
    let mut reservation = budget.reservation();
    reservation.try_grow(retained_bytes).map_err(|error| {
        RemoteClientError::Protocol(format!(
            "v5 {method} request exceeds connection retained-byte budget: {error}"
        ))
    })?;
    Ok(reservation)
}

#[derive(Default)]
pub(crate) struct V5ResponseAccumulator {
    pub(crate) method: Option<String>,
    pub(crate) payload: Vec<u8>,
    pub(crate) file_body: Vec<u8>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) search_partials: V5SearchResponsePartials,
    pub(crate) final_error: Option<RemoteError>,
    pub(crate) received_bytes: usize,
}

#[derive(Default)]
pub(crate) struct V5SearchResponsePartials {
    current_method: Option<String>,
    current_payload: Vec<u8>,
    file_root: Option<PathBuf>,
    file_files: Vec<PathBuf>,
    file_truncated: bool,
    text_root: Option<PathBuf>,
    text_matches: Vec<TextSearchMatchResponse>,
    text_truncated: bool,
}

impl V5SearchResponsePartials {
    pub(crate) fn begin_partial(
        &mut self,
        method: String,
    ) -> std::result::Result<(), RemoteClientError> {
        self.finish_current()?;
        if matches!(method.as_str(), "search.files" | "search.text") {
            self.current_method = Some(method);
            self.current_payload.clear();
        }
        Ok(())
    }

    pub(crate) fn push_search_payload(&mut self, body: Vec<u8>) {
        if self.current_method.is_some() {
            self.current_payload.extend(body);
        }
    }

    pub(crate) fn finish_current(&mut self) -> std::result::Result<(), RemoteClientError> {
        let Some(method) = self.current_method.take() else {
            return Ok(());
        };
        let payload = std::mem::take(&mut self.current_payload);
        let response = RemoteResponse::from_v5_payload(&method, &payload)
            .map_err(v5_method_error_to_client_error)?;
        match response {
            RemoteResponse::FileSearch(partial) => {
                self.file_root.get_or_insert(partial.root);
                self.file_files.extend(partial.files);
                self.file_truncated |= partial.truncated;
            }
            RemoteResponse::TextSearch(partial) => {
                self.text_root.get_or_insert(partial.root);
                self.text_matches.extend(partial.matches);
                self.text_truncated |= partial.truncated;
            }
            other => {
                return Err(RemoteClientError::Protocol(format!(
                    "unexpected v5 search partial response: {other:?}"
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn merge_final(
        &mut self,
        method: &str,
        payload: &[u8],
    ) -> std::result::Result<Option<RemoteResponse>, RemoteClientError> {
        self.finish_current()?;
        match method {
            "search.files" if self.file_root.is_some() || !self.file_files.is_empty() => {
                let mut final_response = match RemoteResponse::from_v5_payload(method, payload)
                    .map_err(v5_method_error_to_client_error)?
                {
                    RemoteResponse::FileSearch(response) => response,
                    other => {
                        return Err(RemoteClientError::Protocol(format!(
                            "unexpected v5 file search final response: {other:?}"
                        )));
                    }
                };
                let mut files = std::mem::take(&mut self.file_files);
                files.append(&mut final_response.files);
                let root = self.file_root.take().unwrap_or(final_response.root);
                Ok(Some(RemoteResponse::FileSearch(FileSearchResponse {
                    root,
                    files,
                    truncated: self.file_truncated || final_response.truncated,
                })))
            }
            "search.text" if self.text_root.is_some() || !self.text_matches.is_empty() => {
                let mut final_response = match RemoteResponse::from_v5_payload(method, payload)
                    .map_err(v5_method_error_to_client_error)?
                {
                    RemoteResponse::TextSearch(response) => response,
                    other => {
                        return Err(RemoteClientError::Protocol(format!(
                            "unexpected v5 text search final response: {other:?}"
                        )));
                    }
                };
                let mut matches = std::mem::take(&mut self.text_matches);
                matches.append(&mut final_response.matches);
                let root = self.text_root.take().unwrap_or(final_response.root);
                Ok(Some(RemoteResponse::TextSearch(TextSearchResponse {
                    root,
                    matches,
                    truncated: self.text_truncated || final_response.truncated,
                })))
            }
            _ => Ok(None),
        }
    }
}

#[derive(Default)]
pub(crate) struct V5RawResponseAccumulator {
    pub(crate) payload: Vec<u8>,
    pub(crate) final_seen: bool,
    pub(crate) final_error: Option<RemoteError>,
    pub(crate) received_bytes: usize,
}
