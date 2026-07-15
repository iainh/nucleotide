// ABOUTME: Multiplexed v5 client reader, writer, cancellation, and response-routing runtime
// ABOUTME: Coordinates connection flow control, deadlines, and terminal failure

use super::*;

pub(crate) fn run_v5_client_reader<R, W>(
    mut reader: R,
    limits: protocol_v5::FrameLimits,
    mut inbound_frame_sequence: protocol_v5::InboundFrameSequence,
    shared: Weak<RemoteWorkspaceV5Shared<W>>,
) where
    R: Read,
    W: Write,
{
    loop {
        if shared.strong_count() == 0 {
            break;
        }
        match inbound_frame_sequence.read_frame(&mut reader, limits) {
            Ok(Some(frame)) => {
                let Some(shared) = shared.upgrade() else {
                    break;
                };
                let received_at = Instant::now();
                let frame_type = frame.frame_type;
                let pong_control = v5_client_pong_control(&frame);
                let event = {
                    let mut session = match shared.session.lock() {
                        Ok(session) => session,
                        Err(_) => {
                            fail_all_v5_waiters(&shared, || {
                                RemoteClientError::Protocol(
                                    "v5 session lock is poisoned".to_string(),
                                )
                            });
                            break;
                        }
                    };
                    session.receive_frame(frame)
                };
                match event {
                    Ok(event) => {
                        if let Some(stream_id) = v5_client_inbound_progress_stream(&event.routed)
                            && let Err(error) =
                                observe_v5_client_request_progress(&shared, stream_id, received_at)
                        {
                            fail_all_v5_waiters_for_error(&shared, &error);
                            break;
                        }
                        let heartbeat_result = shared
                            .heartbeat
                            .lock()
                            .map_err(v5_client_lock_error)
                            .and_then(|mut heartbeat| {
                                heartbeat.observe_inbound(frame_type, pong_control, received_at)
                            });
                        match heartbeat_result {
                            Ok(Some(rtt)) => {
                                tracing::trace!(
                                    rtt_micros = rtt.as_micros() as u64,
                                    "Received matching v5 client heartbeat PONG"
                                );
                            }
                            Ok(None) => {}
                            Err(error) => {
                                fail_all_v5_waiters_for_error(&shared, &error);
                                break;
                            }
                        }
                        signal_v5_client_heartbeat(&shared);
                        let data_credit = event.data_credit();
                        let acknowledge_data = event
                            .stream_event
                            .map(|stream_event| {
                                handle_v5_client_stream_event(&shared, stream_event, data_credit)
                            })
                            .unwrap_or(true);
                        if acknowledge_data && let Some((stream_id, credit_bytes)) = data_credit {
                            let result = shared
                                .session
                                .lock()
                                .map_err(v5_client_lock_error)
                                .and_then(|mut session| {
                                    if session.stream_tombstone(stream_id).is_some() {
                                        Ok(())
                                    } else {
                                        session
                                            .acknowledge_data(stream_id, credit_bytes)
                                            .map_err(RemoteClientError::Io)
                                    }
                                });
                            if let Err(error) = result {
                                let message = error.to_string();
                                fail_all_v5_waiters(&shared, || {
                                    RemoteClientError::Protocol(format!(
                                        "failed to queue v5 flow-control update: {message}"
                                    ))
                                });
                                break;
                            }
                        }
                        if let Err(error) = wake_v5_client_writer(&shared) {
                            tracing::warn!(
                                error = %error,
                                "Closing v5 client after writer wake failed"
                            );
                            break;
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        fail_all_v5_waiters(&shared, || {
                            RemoteClientError::Protocol(format!(
                                "failed to route v5 response frame: {message}"
                            ))
                        });
                        break;
                    }
                }
            }
            Ok(None) => {
                if let Some(shared) = shared.upgrade() {
                    fail_all_v5_waiters(&shared, || RemoteClientError::Disconnected);
                }
                break;
            }
            Err(error) => {
                let kind = error.kind();
                let message = error.to_string();
                if let Some(shared) = shared.upgrade() {
                    fail_all_v5_waiters(&shared, || {
                        RemoteClientError::Io(io::Error::new(kind, message.clone()))
                    });
                }
                break;
            }
        }
    }
}

pub(crate) fn signal_v5_client_heartbeat<W>(shared: &RemoteWorkspaceV5Shared<W>) {
    let _ = shared.heartbeat_wake.try_send(());
}

pub(crate) fn register_v5_client_cancellation<W>(
    cancellation: &RemoteRequestCancellation,
    shared: &Arc<RemoteWorkspaceV5Shared<W>>,
    stream_id: u64,
    request: V5ClientCancellation,
) where
    W: 'static,
{
    let shared = Arc::downgrade(shared);
    cancellation.register(move || {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        let mut pending_cancellations = shared
            .pending_cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        pending_cancellations
            .entry(stream_id)
            .and_modify(|pending| {
                if request.mode == V5ClientCancellationMode::Connection {
                    *pending = request;
                }
            })
            .or_insert(request);
        drop(pending_cancellations);
        signal_v5_client_deadlines(&shared);
    });
}

pub(crate) fn signal_v5_client_deadlines<W>(shared: &RemoteWorkspaceV5Shared<W>) {
    let _ = shared.deadline_wake.try_send(());
}

pub(crate) fn run_v5_client_heartbeat<W>(
    wakes: mpsc::Receiver<()>,
    shared: Weak<RemoteWorkspaceV5Shared<W>>,
) {
    loop {
        let Some(shared) = shared.upgrade() else {
            break;
        };
        if shared.closed.load(Ordering::Acquire) {
            break;
        }

        let action = match shared
            .heartbeat
            .lock()
            .map_err(v5_client_lock_error)
            .and_then(|mut heartbeat| heartbeat.next_action(Instant::now()))
        {
            Ok(action) => action,
            Err(error) => {
                fail_all_v5_waiters_for_error(&shared, &error);
                break;
            }
        };

        match action {
            V5ClientHeartbeatAction::QueuePing(token) => {
                let queued = shared
                    .session
                    .lock()
                    .map_err(v5_client_lock_error)
                    .and_then(|mut session| {
                        session.send_ping(token).map_err(RemoteClientError::Io)
                    });
                if let Err(error) = queued {
                    fail_all_v5_waiters_for_error(&shared, &error);
                    break;
                }
                if wake_v5_client_writer(&shared).is_err() {
                    break;
                }
            }
            V5ClientHeartbeatAction::TimedOut(message) => {
                let error = RemoteClientError::Io(io::Error::new(io::ErrorKind::TimedOut, message));
                fail_all_v5_waiters_for_error(&shared, &error);
                break;
            }
            V5ClientHeartbeatAction::Wait(timeout) => {
                drop(shared);
                match wakes.recv_timeout(timeout) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }
    }
}

pub(crate) fn run_v5_client_deadlines<W>(
    wakes: mpsc::Receiver<()>,
    shared: Weak<RemoteWorkspaceV5Shared<W>>,
) where
    W: Write,
{
    loop {
        let Some(shared) = shared.upgrade() else {
            break;
        };
        if shared.closed.load(Ordering::Acquire) {
            break;
        }

        let wait = match expire_v5_client_deadlines_at(&shared, Instant::now()) {
            Ok(wait) => wait,
            Err(error) => {
                fail_all_v5_waiters_for_error(&shared, &error);
                break;
            }
        };
        if shared.closed.load(Ordering::Acquire) {
            break;
        }
        drop(shared);

        let wake = match wait {
            Some(timeout) => wakes.recv_timeout(timeout),
            None => match wakes.recv() {
                Ok(()) => continue,
                Err(_) => break,
            },
        };
        match wake {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

pub(crate) fn process_v5_client_cancellations<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
) -> std::result::Result<bool, RemoteClientError> {
    if shared.closed.load(Ordering::Acquire) {
        return Ok(false);
    }
    let cancellations = std::mem::take(
        &mut *shared
            .pending_cancellations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    if cancellations.is_empty() {
        return Ok(true);
    }

    let terminal_streams = cancellations
        .iter()
        .filter_map(|(stream_id, cancellation)| {
            (cancellation.mode == V5ClientCancellationMode::Connection).then_some(*stream_id)
        })
        .collect::<HashSet<_>>();
    if !terminal_streams.is_empty() {
        let response_pending = {
            let mut waiters = shared.waiters.lock().map_err(v5_client_lock_error)?;
            terminal_streams
                .iter()
                .filter_map(|stream_id| waiters.remove(stream_id))
                .collect::<Vec<_>>()
        };
        for pending in response_pending {
            let _ = pending
                .sender
                .send(Err(remote_request_cancelled_error(pending.method)));
        }
        let raw_pending = {
            let mut waiters = shared.raw_waiters.lock().map_err(v5_client_lock_error)?;
            terminal_streams
                .iter()
                .filter_map(|stream_id| waiters.remove(stream_id))
                .collect::<Vec<_>>()
        };
        for pending in raw_pending {
            let _ = pending
                .sender
                .send(Err(remote_request_cancelled_error(pending.method)));
        }
        if shared
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            finish_v5_connection_close(shared, || {
                RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "v5 watch control request cancelled by caller",
                ))
            });
        }
        return Ok(false);
    }

    let cancelled = {
        let mut waiters = shared.waiters.lock().map_err(v5_client_lock_error)?;
        cancellations
            .into_iter()
            .map(|(stream_id, cancellation)| (stream_id, cancellation, waiters.remove(&stream_id)))
            .collect::<Vec<_>>()
    };
    let mut reset_queued = false;
    for (stream_id, cancellation, pending) in cancelled {
        if let Some(pending) = pending {
            let _ = pending
                .sender
                .send(Err(remote_request_cancelled_error(pending.method)));
        }
        let file_pending = shared
            .file_waiters
            .lock()
            .map_err(v5_client_lock_error)?
            .remove(&stream_id);
        let search_pending = shared
            .search_waiters
            .lock()
            .map_err(v5_client_lock_error)?
            .remove(&stream_id);
        let process_pending = shared
            .process_waiters
            .lock()
            .map_err(v5_client_lock_error)?
            .remove(&stream_id);
        let completed_file = shared
            .completed_file_streams
            .lock()
            .map_err(v5_client_lock_error)?
            .remove(&stream_id);
        let completed_search = shared
            .completed_search_streams
            .lock()
            .map_err(v5_client_lock_error)?
            .remove(&stream_id);
        let completed_process = shared
            .completed_process_streams
            .lock()
            .map_err(v5_client_lock_error)?
            .remove(&stream_id);
        let reset = shared
            .session
            .lock()
            .map_err(v5_client_lock_error)?
            .reset_stream(
                stream_id,
                protocol_v5::RESET_CANCELLED,
                format!("client dropped {} request handle", cancellation.method),
            )
            .map_err(RemoteClientError::Io);
        if let Some(file_pending) = file_pending {
            let credit_bytes = file_pending
                .mailbox
                .fail(remote_request_cancelled_error(file_pending.method));
            queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        }
        if let Some(search_pending) = search_pending {
            let credit_bytes = search_pending
                .mailbox
                .fail(remote_request_cancelled_error(search_pending.method));
            queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        }
        if let Some(process_pending) = process_pending {
            let credit_bytes = process_pending
                .mailbox
                .fail(remote_request_cancelled_error(process_pending.method));
            queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        }
        if let Some(mailbox) = completed_file {
            let credit_bytes = mailbox.fail(remote_request_cancelled_error(cancellation.method));
            queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        }
        if let Some(mailbox) = completed_search {
            let credit_bytes = mailbox.fail(remote_request_cancelled_error(cancellation.method));
            queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        }
        if let Some(mailbox) = completed_process {
            let credit_bytes = mailbox.fail(remote_request_cancelled_error(cancellation.method));
            queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        }
        match reset? {
            true => reset_queued = true,
            false => release_v5_outbound_request_reservation(shared, stream_id),
        }
    }
    if reset_queued {
        wake_v5_client_writer(shared)?;
    }
    Ok(!shared.closed.load(Ordering::Acquire))
}

pub(crate) fn expire_v5_client_deadlines_at<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    now: Instant,
) -> std::result::Result<Option<Duration>, RemoteClientError>
where
    W: Write,
{
    if shared.closed.load(Ordering::Acquire) {
        return Ok(None);
    }
    if !process_v5_client_cancellations(shared)? {
        return Ok(None);
    }
    let heartbeat = shared.heartbeat.lock().map_err(v5_client_lock_error)?;
    let peer_is_healthy = heartbeat.peer_is_healthy_at(now);

    let (raw_expired, raw_close_claimed) = {
        let raw_waiters = shared.raw_waiters.lock().map_err(v5_client_lock_error)?;
        let raw_expired = raw_waiters
            .values()
            .any(|pending| pending.deadline.expired_at(now).is_some());
        let close_claimed = raw_expired
            && shared
                .closed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
        (raw_expired, close_claimed)
    };
    if raw_expired {
        drop(heartbeat);
        if raw_close_claimed {
            finish_v5_connection_close(shared, || {
                RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "v5 watch control request deadline expired",
                ))
            });
        }
        return Ok(None);
    }

    let mut waiters = shared.waiters.lock().map_err(v5_client_lock_error)?;
    let response_connection_terminal = waiters.values().any(|pending| {
        pending.deadline.expired_at(now).is_some()
            && (!peer_is_healthy || pending.deadline_is_connection_terminal())
    });
    let file_connection_terminal = shared
        .file_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .any(|pending| pending.deadline.expired_at(now).is_some() && !peer_is_healthy);
    let search_connection_terminal = shared
        .search_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .any(|pending| pending.deadline.expired_at(now).is_some() && !peer_is_healthy);
    let process_connection_terminal = shared
        .process_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .any(|pending| {
            pending.deadline.expired_at(now).is_some()
                && (!peer_is_healthy || pending.final_method.is_none())
        });
    let connection_terminal = response_connection_terminal
        || file_connection_terminal
        || search_connection_terminal
        || process_connection_terminal;
    let close_claimed = connection_terminal
        && shared
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
    let expired = if connection_terminal {
        Vec::new()
    } else {
        let expired = waiters
            .iter()
            .filter_map(|(stream_id, pending)| {
                pending
                    .deadline
                    .expired_at(now)
                    .map(|kind| (*stream_id, kind))
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|(stream_id, kind)| {
                waiters
                    .remove(&stream_id)
                    .map(|pending| (stream_id, kind, pending))
            })
            .collect::<Vec<_>>()
    };
    drop(waiters);
    let file_expired = if connection_terminal {
        Vec::new()
    } else {
        let mut file_waiters = shared.file_waiters.lock().map_err(v5_client_lock_error)?;
        let expired = file_waiters
            .iter()
            .filter_map(|(stream_id, pending)| {
                pending
                    .deadline
                    .expired_at(now)
                    .map(|kind| (*stream_id, kind))
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|(stream_id, kind)| {
                file_waiters
                    .remove(&stream_id)
                    .map(|pending| (stream_id, kind, pending))
            })
            .collect::<Vec<_>>()
    };
    let search_expired = if connection_terminal {
        Vec::new()
    } else {
        let mut search_waiters = shared.search_waiters.lock().map_err(v5_client_lock_error)?;
        let expired = search_waiters
            .iter()
            .filter_map(|(stream_id, pending)| {
                pending
                    .deadline
                    .expired_at(now)
                    .map(|kind| (*stream_id, kind))
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|(stream_id, kind)| {
                search_waiters
                    .remove(&stream_id)
                    .map(|pending| (stream_id, kind, pending))
            })
            .collect::<Vec<_>>()
    };
    let process_expired = if connection_terminal {
        Vec::new()
    } else {
        let mut process_waiters = shared
            .process_waiters
            .lock()
            .map_err(v5_client_lock_error)?;
        let expired = process_waiters
            .iter()
            .filter_map(|(stream_id, pending)| {
                pending
                    .deadline
                    .expired_at(now)
                    .map(|kind| (*stream_id, kind))
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|(stream_id, kind)| {
                process_waiters
                    .remove(&stream_id)
                    .map(|pending| (stream_id, kind, pending))
            })
            .collect::<Vec<_>>()
    };
    drop(heartbeat);
    if connection_terminal {
        let cause = if !peer_is_healthy {
            "v5 request deadline expired while peer health was unknown"
        } else {
            "v5 mutation request deadline expired"
        };
        if close_claimed {
            finish_v5_connection_close(shared, || {
                RemoteClientError::Io(io::Error::new(io::ErrorKind::TimedOut, cause))
            });
        }
        return Ok(None);
    }

    for (stream_id, kind, pending) in expired {
        let reset = match shared.session.lock() {
            Ok(mut session) => session
                .reset_stream(
                    stream_id,
                    protocol_v5::RESET_DEADLINE_EXCEEDED,
                    format!("client {kind} deadline expired"),
                )
                .map_err(RemoteClientError::Io),
            Err(error) => Err(v5_client_lock_error(error)),
        };
        match reset {
            Ok(true) => {
                if let Err(error) = wake_v5_client_writer(shared) {
                    let pending_error = pending.failure_error(RemoteClientError::TransportClosed {
                        cause: error.to_string(),
                    });
                    let _ = pending.sender.send(Err(pending_error));
                    fail_all_v5_waiters_for_error(shared, &error);
                    return Ok(None);
                }
            }
            Ok(false) => release_v5_outbound_request_reservation(shared, stream_id),
            Err(error) => {
                let pending_error = pending.failure_error(RemoteClientError::TransportClosed {
                    cause: error.to_string(),
                });
                let _ = pending.sender.send(Err(pending_error));
                fail_all_v5_waiters_for_error(shared, &error);
                return Ok(None);
            }
        }
        let error = RemoteClientError::RequestDeadlineExceeded {
            method: pending.method.to_string(),
            kind,
        };
        let _ = pending.sender.send(Err(error));
    }

    for (stream_id, kind, pending) in file_expired {
        let reset = shared
            .session
            .lock()
            .map_err(v5_client_lock_error)?
            .reset_stream(
                stream_id,
                protocol_v5::RESET_DEADLINE_EXCEEDED,
                format!("client {kind} deadline expired"),
            )
            .map_err(RemoteClientError::Io);
        let deadline_error = RemoteClientError::RequestDeadlineExceeded {
            method: pending.method.to_string(),
            kind,
        };
        let credit_bytes = pending.mailbox.fail(deadline_error);
        queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        match reset? {
            true => wake_v5_client_writer(shared)?,
            false => release_v5_outbound_request_reservation(shared, stream_id),
        }
    }

    for (stream_id, kind, pending) in search_expired {
        let reset = shared
            .session
            .lock()
            .map_err(v5_client_lock_error)?
            .reset_stream(
                stream_id,
                protocol_v5::RESET_DEADLINE_EXCEEDED,
                format!("client {kind} deadline expired"),
            )
            .map_err(RemoteClientError::Io);
        let deadline_error = RemoteClientError::RequestDeadlineExceeded {
            method: pending.method.to_string(),
            kind,
        };
        let credit_bytes = pending.mailbox.fail(deadline_error);
        queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        match reset? {
            true => wake_v5_client_writer(shared)?,
            false => release_v5_outbound_request_reservation(shared, stream_id),
        }
    }

    for (stream_id, kind, pending) in process_expired {
        let reset = shared
            .session
            .lock()
            .map_err(v5_client_lock_error)?
            .reset_stream(
                stream_id,
                protocol_v5::RESET_DEADLINE_EXCEEDED,
                format!("client {kind} deadline expired"),
            )
            .map_err(RemoteClientError::Io);
        let deadline_error = RemoteClientError::RequestDeadlineExceeded {
            method: pending.method.to_string(),
            kind,
        };
        let credit_bytes = pending.mailbox.fail(deadline_error);
        queue_v5_released_receive_credit(shared, stream_id, credit_bytes)?;
        match reset? {
            true => wake_v5_client_writer(shared)?,
            false => release_v5_outbound_request_reservation(shared, stream_id),
        }
    }

    next_v5_client_deadline_wait(shared, Instant::now())
}

pub(crate) fn next_v5_client_deadline_wait<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    now: Instant,
) -> std::result::Result<Option<Duration>, RemoteClientError> {
    let response_deadline = shared
        .waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .filter_map(|pending| pending.deadline.next_expiry().map(|(deadline, _)| deadline))
        .min();
    let raw_deadline = shared
        .raw_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .filter_map(|pending| pending.deadline.next_expiry().map(|(deadline, _)| deadline))
        .min();
    let file_deadline = shared
        .file_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .filter_map(|pending| pending.deadline.next_expiry().map(|(deadline, _)| deadline))
        .min();
    let search_deadline = shared
        .search_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .filter_map(|pending| pending.deadline.next_expiry().map(|(deadline, _)| deadline))
        .min();
    let process_deadline = shared
        .process_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .values()
        .filter_map(|pending| pending.deadline.next_expiry().map(|(deadline, _)| deadline))
        .min();
    Ok([
        response_deadline,
        raw_deadline,
        file_deadline,
        search_deadline,
        process_deadline,
    ]
    .into_iter()
    .flatten()
    .min()
    .map(|deadline| deadline.saturating_duration_since(now)))
}

pub(crate) fn wake_v5_client_writer<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
) -> std::result::Result<(), RemoteClientError> {
    if shared.closed.load(Ordering::Acquire) {
        return Err(RemoteClientError::Disconnected);
    }
    match shared.writer_wake.try_send(()) {
        Ok(()) | Err(mpsc::TrySendError::Full(())) => Ok(()),
        Err(mpsc::TrySendError::Disconnected(())) => {
            let error = RemoteClientError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "v5 client writer stopped",
            ));
            fail_all_v5_waiters_for_error(shared, &error);
            Err(error)
        }
    }
}

pub(crate) fn queue_v5_released_receive_credit<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    stream_id: u64,
    credit_bytes: u64,
) -> std::result::Result<(), RemoteClientError> {
    if credit_bytes == 0 || shared.closed.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut credits = shared
        .pending_receive_credits
        .lock()
        .map_err(v5_client_lock_error)?;
    if shared.closed.load(Ordering::Acquire) {
        return Ok(());
    }
    let credit = credits.entry(stream_id).or_default();
    *credit = credit.checked_add(credit_bytes).ok_or_else(|| {
        RemoteClientError::Protocol("v5 released receive credit overflowed".to_string())
    })?;
    drop(credits);
    wake_v5_client_writer(shared)
}

pub(crate) fn apply_v5_released_receive_credit<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
) -> std::result::Result<(), RemoteClientError> {
    let credits = std::mem::take(
        &mut *shared
            .pending_receive_credits
            .lock()
            .map_err(v5_client_lock_error)?,
    );
    if credits.is_empty() {
        return Ok(());
    }
    let mut credits = credits.into_iter().collect::<Vec<_>>();
    credits.sort_unstable_by_key(|(stream_id, _)| *stream_id);
    let mut session = shared.session.lock().map_err(v5_client_lock_error)?;
    for (stream_id, credit_bytes) in credits {
        session
            .acknowledge_data(stream_id, credit_bytes)
            .map_err(RemoteClientError::Io)?;
    }
    Ok(())
}

pub(crate) fn run_v5_client_writer<W>(
    mut writer: RemoteWorkspaceV5Writer<W>,
    wakes: mpsc::Receiver<()>,
    shared: Weak<RemoteWorkspaceV5Shared<W>>,
) where
    W: Write,
{
    while wakes.recv().is_ok() {
        let Some(shared) = shared.upgrade() else {
            break;
        };
        if shared.closed.load(Ordering::Acquire) {
            break;
        }
        if let Err(error) = write_v5_client_outbound(&shared, &mut writer) {
            if !shared.closed.load(Ordering::Acquire) {
                tracing::warn!(
                    error = %error,
                    "Closing v5 client after writer pump failed"
                );
            }
            fail_all_v5_waiters_for_error(&shared, &error);
            break;
        }
    }
}

pub(crate) fn write_v5_client_outbound<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    writer: &mut RemoteWorkspaceV5Writer<W>,
) -> std::result::Result<(), RemoteClientError>
where
    W: Write,
{
    loop {
        if shared.closed.load(Ordering::Acquire) {
            return Err(RemoteClientError::Disconnected);
        }
        apply_v5_released_receive_credit(shared)?;

        // Select one frame at a time so newly queued urgent control traffic can pre-empt the
        // remainder of this bounded flush batch.
        let mut processed_frames = 0;
        let mut wrote_frame = false;
        while processed_frames < V5_CLIENT_WRITE_BATCH_FRAMES {
            let Some(mut frame) = shared
                .session
                .lock()
                .map_err(v5_client_lock_error)?
                .pop_next_frame()?
            else {
                break;
            };
            processed_frames += 1;
            let should_write = {
                let mut session = shared.session.lock().map_err(v5_client_lock_error)?;
                let should_write = session.should_write_frame(&frame);
                if !should_write {
                    session.discard_unwritten_frame(&frame)?;
                }
                should_write
            };
            if !should_write || shared.closed.load(Ordering::Acquire) {
                continue;
            }

            if frame.frame_type == protocol_v5::FrameType::Ping {
                shared
                    .heartbeat
                    .lock()
                    .map_err(v5_client_lock_error)?
                    .mark_ping_started(&frame, Instant::now())?;
                signal_v5_client_heartbeat(shared);
            }
            let request_frame = frame.stream_id != 0
                && matches!(
                    frame.frame_type,
                    protocol_v5::FrameType::Headers
                        | protocol_v5::FrameType::Data
                        | protocol_v5::FrameType::EndStream
                );
            // Only a completed physical write advances inactivity. Mutation deadlines are
            // conservatively connection-terminal because reset and write can race at the
            // transport boundary.
            frame.frame_sequence = writer.next_frame_sequence;
            writer.next_frame_sequence =
                writer.next_frame_sequence.checked_add(1).ok_or_else(|| {
                    RemoteClientError::Protocol("v5 frame sequence exhausted".to_string())
                })?;
            let limits = writer.limits;
            protocol_v5::write_frame_unflushed_with_limits(&mut writer.writer, &frame, limits)?;
            {
                shared
                    .session
                    .lock()
                    .map_err(v5_client_lock_error)?
                    .observe_frame_written(&frame);
            }
            if request_frame {
                observe_v5_client_request_progress(shared, frame.stream_id, Instant::now())?;
            }
            if matches!(
                frame.frame_type,
                protocol_v5::FrameType::EndStream | protocol_v5::FrameType::ResetStream
            ) {
                release_v5_outbound_request_reservation(shared, frame.stream_id);
            }
            wrote_frame = true;
        }
        if wrote_frame {
            writer.writer.flush()?;
        }
        if processed_frames < V5_CLIENT_WRITE_BATCH_FRAMES {
            return Ok(());
        }
    }
}

pub(crate) fn v5_client_inbound_progress_stream(routed: &protocol_v5::RoutedFrame) -> Option<u64> {
    match routed {
        protocol_v5::RoutedFrame::WindowUpdate { stream_id, .. }
        | protocol_v5::RoutedFrame::Headers { stream_id, .. }
        | protocol_v5::RoutedFrame::Data { stream_id, .. }
        | protocol_v5::RoutedFrame::EndStream { stream_id, .. }
        | protocol_v5::RoutedFrame::ResetStream { stream_id, .. }
            if *stream_id != 0 =>
        {
            Some(*stream_id)
        }
        protocol_v5::RoutedFrame::ConnectionControl { .. }
        | protocol_v5::RoutedFrame::WindowUpdate { .. }
        | protocol_v5::RoutedFrame::Headers { .. }
        | protocol_v5::RoutedFrame::Data { .. }
        | protocol_v5::RoutedFrame::EndStream { .. }
        | protocol_v5::RoutedFrame::RejectedStream { .. }
        | protocol_v5::RoutedFrame::ResetStream { .. } => None,
    }
}

pub(crate) fn observe_v5_client_request_progress<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    stream_id: u64,
    now: Instant,
) -> std::result::Result<(), RemoteClientError> {
    if let Some(pending) = shared
        .waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .get_mut(&stream_id)
    {
        pending.deadline.observe_progress(now);
        return Ok(());
    }
    if let Some(pending) = shared
        .raw_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .get_mut(&stream_id)
    {
        pending.deadline.observe_progress(now);
        return Ok(());
    }
    if let Some(pending) = shared
        .file_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .get_mut(&stream_id)
    {
        pending.deadline.observe_progress(now);
        return Ok(());
    }
    if let Some(pending) = shared
        .search_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .get_mut(&stream_id)
    {
        pending.deadline.observe_progress(now);
        return Ok(());
    }
    if let Some(pending) = shared
        .process_waiters
        .lock()
        .map_err(v5_client_lock_error)?
        .get_mut(&stream_id)
    {
        pending.deadline.observe_progress(now);
    }
    Ok(())
}

pub(crate) fn release_v5_outbound_request_reservation<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    stream_id: u64,
) {
    shared
        .outbound_request_reservations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&stream_id);
}

pub(crate) fn handle_v5_client_file_stream_event<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    event: protocol_v5::StreamEvent,
    data_credit: Option<(u64, u64)>,
) -> Option<bool>
where
    W: Write,
{
    let stream_id = event.stream_id();
    let peer_reset = matches!(&event, protocol_v5::StreamEvent::ResetStream { .. });
    let (pending, observation) = {
        let mut waiters = match shared.file_waiters.lock() {
            Ok(waiters) => waiters,
            Err(_) => return Some(false),
        };
        let pending = waiters.get_mut(&stream_id)?;
        let observation = pending.observe(event, data_credit);
        match &observation {
            Ok(V5FileStreamObservation::Continue { .. }) => (None, observation),
            Ok(V5FileStreamObservation::Complete(_)) => {
                let pending = waiters.remove(&stream_id);
                let Some(pending_ref) = pending.as_ref() else {
                    return Some(false);
                };
                match shared.completed_file_streams.lock() {
                    Ok(mut completed) => {
                        completed.insert(stream_id, Arc::clone(&pending_ref.mailbox));
                        (pending, observation)
                    }
                    Err(_) => (
                        pending,
                        Err(RemoteClientError::Protocol(
                            "v5 completed file stream registry lock is poisoned".to_string(),
                        )),
                    ),
                }
            }
            Err(_) => (waiters.remove(&stream_id), observation),
        }
    };

    match observation {
        Ok(V5FileStreamObservation::Continue { acknowledge_data }) => Some(acknowledge_data),
        Ok(V5FileStreamObservation::Complete(read)) => {
            let pending = pending.expect("completed v5 file waiter should be removed");
            if let Err(error) = pending.mailbox.complete(read) {
                shared
                    .completed_file_streams
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&stream_id);
                reset_v5_client_stream_after_local_error(shared, stream_id);
                let credit_bytes = pending.mailbox.fail(error);
                if let Err(error) =
                    queue_v5_released_receive_credit(shared, stream_id, credit_bytes)
                {
                    fail_all_v5_waiters_for_error(shared, &error);
                }
                return Some(false);
            }
            if let Ok(mut completed) = shared.completed_file_streams.lock()
                && !pending.mailbox.has_pending_delivery()
            {
                completed.remove(&stream_id);
            }
            Some(true)
        }
        Err(error) => {
            let pending = pending.expect("failed v5 file waiter should be removed");
            if !peer_reset {
                reset_v5_client_stream_after_local_error(shared, stream_id);
            }
            let queued_credit = pending.mailbox.fail(error);
            let current_credit = data_credit.map(|(_, credit)| credit).unwrap_or(0);
            let Some(released_credit) = queued_credit.checked_add(current_credit) else {
                let error = RemoteClientError::Protocol(
                    "v5 abandoned file stream credit overflowed".to_string(),
                );
                fail_all_v5_waiters_for_error(shared, &error);
                return Some(false);
            };
            if let Err(error) = queue_v5_released_receive_credit(shared, stream_id, released_credit)
            {
                fail_all_v5_waiters_for_error(shared, &error);
            }
            Some(false)
        }
    }
}

pub(crate) fn handle_v5_client_search_stream_event<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    event: protocol_v5::StreamEvent,
    data_credit: Option<(u64, u64)>,
) -> Option<bool>
where
    W: Write,
{
    let stream_id = event.stream_id();
    let peer_reset = matches!(&event, protocol_v5::StreamEvent::ResetStream { .. });
    let (pending, observation, credit_was_reserved) = {
        let mut waiters = match shared.search_waiters.lock() {
            Ok(waiters) => waiters,
            Err(_) => return Some(false),
        };
        let pending = waiters.get_mut(&stream_id)?;
        let credit_before = pending.mailbox.queued_credit();
        let observation = pending.observe(event, data_credit);
        let credit_after = pending.mailbox.queued_credit();
        let current_credit = data_credit.map(|(_, credit)| credit).unwrap_or(0);
        let credit_was_reserved = credit_after >= credit_before.saturating_add(current_credit);
        match &observation {
            Ok(V5SearchStreamObservation::Continue { .. }) => {
                (None, observation, credit_was_reserved)
            }
            Ok(V5SearchStreamObservation::Complete) => {
                let pending = waiters.remove(&stream_id);
                let Some(pending_ref) = pending.as_ref() else {
                    return Some(false);
                };
                match shared.completed_search_streams.lock() {
                    Ok(mut completed) => {
                        completed.insert(stream_id, Arc::clone(&pending_ref.mailbox));
                        (pending, observation, credit_was_reserved)
                    }
                    Err(_) => (
                        pending,
                        Err(RemoteClientError::Protocol(
                            "v5 completed search stream registry lock is poisoned".to_string(),
                        )),
                        credit_was_reserved,
                    ),
                }
            }
            Err(_) => (waiters.remove(&stream_id), observation, credit_was_reserved),
        }
    };

    match observation {
        Ok(V5SearchStreamObservation::Continue { acknowledge_data }) => Some(acknowledge_data),
        Ok(V5SearchStreamObservation::Complete) => {
            let pending = pending.expect("completed v5 search waiter should be removed");
            if let Ok(mut completed) = shared.completed_search_streams.lock() {
                if !pending.mailbox.has_pending_delivery() {
                    completed.remove(&stream_id);
                }
                Some(true)
            } else {
                Some(false)
            }
        }
        Err(error) => {
            let pending = pending.expect("failed v5 search waiter should be removed");
            if !peer_reset {
                reset_v5_client_stream_after_local_error(shared, stream_id);
            }
            let queued_credit = pending.mailbox.fail(error);
            let unreserved_current_credit = if credit_was_reserved {
                0
            } else {
                data_credit.map(|(_, credit)| credit).unwrap_or(0)
            };
            let Some(released_credit) = queued_credit.checked_add(unreserved_current_credit) else {
                let error = RemoteClientError::Protocol(
                    "v5 abandoned search stream credit overflowed".to_string(),
                );
                fail_all_v5_waiters_for_error(shared, &error);
                return Some(false);
            };
            if let Err(error) = queue_v5_released_receive_credit(shared, stream_id, released_credit)
            {
                fail_all_v5_waiters_for_error(shared, &error);
            }
            Some(false)
        }
    }
}

pub(crate) fn handle_v5_client_process_stream_event<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    event: protocol_v5::StreamEvent,
    data_credit: Option<(u64, u64)>,
) -> Option<bool>
where
    W: Write,
{
    let stream_id = event.stream_id();
    let peer_reset = matches!(&event, protocol_v5::StreamEvent::ResetStream { .. });
    let peer_deadline = matches!(
        &event,
        protocol_v5::StreamEvent::ResetStream { code, .. }
            if code == protocol_v5::RESET_DEADLINE_EXCEEDED
    );
    let (pending, observation, credit_was_reserved, terminal_peer_deadline) = {
        let mut waiters = match shared.process_waiters.lock() {
            Ok(waiters) => waiters,
            Err(_) => return Some(false),
        };
        let pending = waiters.get_mut(&stream_id)?;
        let terminal_peer_deadline = peer_deadline && pending.final_method.is_none();
        let credit_before = pending.mailbox.queued_credit();
        let observation = pending.observe(event, data_credit);
        let credit_after = pending.mailbox.queued_credit();
        let current_credit = data_credit.map(|(_, credit)| credit).unwrap_or(0);
        let credit_was_reserved = credit_after >= credit_before.saturating_add(current_credit);
        match &observation {
            Ok(V5ProcessStreamObservation::Continue { .. }) => (
                None,
                observation,
                credit_was_reserved,
                terminal_peer_deadline,
            ),
            Ok(V5ProcessStreamObservation::Complete) => {
                let pending = waiters.remove(&stream_id);
                let Some(pending_ref) = pending.as_ref() else {
                    return Some(false);
                };
                match shared.completed_process_streams.lock() {
                    Ok(mut completed) => {
                        completed.insert(stream_id, Arc::clone(&pending_ref.mailbox));
                        (
                            pending,
                            observation,
                            credit_was_reserved,
                            terminal_peer_deadline,
                        )
                    }
                    Err(_) => (
                        pending,
                        Err(RemoteClientError::Protocol(
                            "v5 completed process stream registry lock is poisoned".to_string(),
                        )),
                        credit_was_reserved,
                        terminal_peer_deadline,
                    ),
                }
            }
            Err(_) => (
                waiters.remove(&stream_id),
                observation,
                credit_was_reserved,
                terminal_peer_deadline,
            ),
        }
    };

    match observation {
        Ok(V5ProcessStreamObservation::Continue { acknowledge_data }) => Some(acknowledge_data),
        Ok(V5ProcessStreamObservation::Complete) => {
            let pending = pending.expect("completed v5 process waiter should be removed");
            if let Ok(mut completed) = shared.completed_process_streams.lock() {
                if !pending.mailbox.has_pending_delivery() {
                    completed.remove(&stream_id);
                }
                Some(true)
            } else {
                Some(false)
            }
        }
        Err(error) => {
            let pending = pending.expect("failed v5 process waiter should be removed");
            if !peer_reset {
                reset_v5_client_stream_after_local_error(shared, stream_id);
            }
            let queued_credit = pending.mailbox.fail(error);
            let unreserved_current_credit = if credit_was_reserved {
                0
            } else {
                data_credit.map(|(_, credit)| credit).unwrap_or(0)
            };
            let Some(released_credit) = queued_credit.checked_add(unreserved_current_credit) else {
                let error = RemoteClientError::Protocol(
                    "v5 abandoned process stream credit overflowed".to_string(),
                );
                fail_all_v5_waiters_for_error(shared, &error);
                return Some(false);
            };
            if let Err(error) = queue_v5_released_receive_credit(shared, stream_id, released_credit)
            {
                fail_all_v5_waiters_for_error(shared, &error);
            }
            if terminal_peer_deadline {
                fail_all_v5_waiters(shared, || {
                    RemoteClientError::Io(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "v5 peer expired an ambiguous process request",
                    ))
                });
            }
            Some(false)
        }
    }
}

pub(crate) fn handle_v5_client_stream_event<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    event: protocol_v5::StreamEvent,
    data_credit: Option<(u64, u64)>,
) -> bool
where
    W: Write,
{
    if shared.closed.load(Ordering::Acquire) {
        return false;
    }
    let stream_id = event.stream_id();
    if matches!(&event, protocol_v5::StreamEvent::ResetStream { .. }) {
        release_v5_outbound_request_reservation(shared, stream_id);
    }
    let is_file_stream = shared
        .file_waiters
        .lock()
        .map(|waiters| waiters.contains_key(&stream_id))
        .unwrap_or(false);
    if is_file_stream {
        return handle_v5_client_file_stream_event(shared, event, data_credit).unwrap_or(false);
    }
    let is_search_stream = shared
        .search_waiters
        .lock()
        .map(|waiters| waiters.contains_key(&stream_id))
        .unwrap_or(false);
    if is_search_stream {
        return handle_v5_client_search_stream_event(shared, event, data_credit).unwrap_or(false);
    }
    let is_process_stream = shared
        .process_waiters
        .lock()
        .map(|waiters| waiters.contains_key(&stream_id))
        .unwrap_or(false);
    if is_process_stream {
        return handle_v5_client_process_stream_event(shared, event, data_credit).unwrap_or(false);
    }
    let mut event = Some(event);
    let completed_response = {
        let mut waiters = match shared.waiters.lock() {
            Ok(waiters) => waiters,
            Err(_) => return false,
        };
        if shared.closed.load(Ordering::Acquire) {
            return false;
        }
        let result = if let Some(pending) = waiters.get_mut(&stream_id) {
            pending.accumulator.observe_with_reservation(
                event.take().expect("event should be available"),
                &mut pending.response_reservation,
            )
        } else {
            None
        };
        result.map(|result| (waiters.remove(&stream_id), result))
    };

    if let Some((Some(pending), result)) = completed_response {
        let normalized = normalize_v5_response_deadline(&pending, result);
        let result = normalized.result;
        let accepted = result.is_ok();
        if !accepted && !normalized.peer_deadline {
            reset_v5_client_stream_after_local_error(shared, stream_id);
        }
        let result = result.map(|value| V5Budgeted::new(value, pending.response_reservation));
        let _ = pending.sender.send(result);
        if normalized.terminal {
            fail_all_v5_waiters(shared, || {
                RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "v5 peer expired an ambiguous mutation request",
                ))
            });
            return false;
        }
        return accepted;
    }
    if event.is_none() {
        return true;
    }

    let completed_raw = {
        let mut raw_waiters = match shared.raw_waiters.lock() {
            Ok(waiters) => waiters,
            Err(_) => return false,
        };
        if shared.closed.load(Ordering::Acquire) {
            return false;
        }
        let result = if let Some(pending) = raw_waiters.get_mut(&stream_id) {
            pending.accumulator.observe_with_reservation(
                event.take().expect("event should be available"),
                &mut pending.response_reservation,
            )
        } else {
            None
        };
        result.map(|result| (raw_waiters.remove(&stream_id), result))
    };

    if let Some((Some(pending), result)) = completed_raw {
        let normalized = normalize_v5_raw_response_deadline(&pending, result);
        let result = normalized.result;
        let accepted = result.is_ok();
        if !accepted && !normalized.peer_deadline {
            reset_v5_client_stream_after_local_error(shared, stream_id);
        }
        let result = result.map(|value| V5Budgeted::new(value, pending.response_reservation));
        let _ = pending.sender.send(result);
        if normalized.terminal {
            fail_all_v5_waiters(shared, || {
                RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "v5 peer expired a watch control request",
                ))
            });
            return false;
        }
        return accepted;
    }
    if let Some(event) = event {
        handle_v5_client_watch_event(shared, event);
    }
    true
}

pub(crate) fn normalize_v5_response_deadline(
    pending: &V5PendingResponse,
    result: std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>,
) -> V5NormalizedDeadlineResult<(RemoteResponse, Vec<u8>)> {
    match result {
        Err(RemoteClientError::Remote(error))
            if error.code == protocol_v5::RESET_DEADLINE_EXCEEDED =>
        {
            if pending.deadline_is_connection_terminal() {
                let cause = RemoteClientError::Remote(error).to_string();
                V5NormalizedDeadlineResult {
                    result: Err(RemoteClientError::OutcomeUnknown {
                        method: pending.method.to_string(),
                        cause,
                    }),
                    peer_deadline: true,
                    terminal: true,
                }
            } else {
                V5NormalizedDeadlineResult {
                    result: Err(RemoteClientError::RequestDeadlineExceeded {
                        method: pending.method.to_string(),
                        kind: RemoteRequestDeadlineKind::Absolute,
                    }),
                    peer_deadline: true,
                    terminal: false,
                }
            }
        }
        result => V5NormalizedDeadlineResult {
            result,
            peer_deadline: false,
            terminal: false,
        },
    }
}

pub(crate) fn normalize_v5_raw_response_deadline(
    pending: &V5PendingRawResponse,
    result: std::result::Result<Vec<u8>, RemoteClientError>,
) -> V5NormalizedDeadlineResult<Vec<u8>> {
    match result {
        Err(RemoteClientError::Remote(error))
            if error.code == protocol_v5::RESET_DEADLINE_EXCEEDED =>
        {
            V5NormalizedDeadlineResult {
                result: Err(RemoteClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("v5 peer expired {}", pending.method),
                ))),
                peer_deadline: true,
                terminal: true,
            }
        }
        result => V5NormalizedDeadlineResult {
            result,
            peer_deadline: false,
            terminal: false,
        },
    }
}

pub(crate) fn reset_v5_client_stream_after_local_error<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    stream_id: u64,
) where
    W: Write,
{
    let reset_queued = shared
        .session
        .lock()
        .map_err(v5_client_lock_error)
        .and_then(|mut session| {
            session
                .reset_stream(
                    stream_id,
                    protocol_v5::RESET_RESOURCE_EXHAUSTED,
                    "client rejected response stream",
                )
                .map_err(RemoteClientError::Io)
        });
    match reset_queued {
        Ok(true) => {
            let _ = wake_v5_client_writer(shared);
        }
        Ok(false) => {
            // A response END can close the logical stream before a flow-blocked request body
            // reaches the wire. `reset_stream` purges that non-tombstoned scheduler state but
            // cannot queue another terminal frame for the already-closed stream, so no writer
            // observation remains to release the retained request bytes.
            release_v5_outbound_request_reservation(shared, stream_id);
        }
        Err(error) => fail_all_v5_waiters_for_error(shared, &error),
    }
}

pub(crate) fn fail_all_v5_waiters<W, F>(shared: &RemoteWorkspaceV5Shared<W>, make_error: F)
where
    F: Fn() -> RemoteClientError,
{
    if shared
        .closed
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    finish_v5_connection_close(shared, make_error);
}

pub(crate) fn finish_v5_connection_close<W, F>(shared: &RemoteWorkspaceV5Shared<W>, make_error: F)
where
    F: Fn() -> RemoteClientError,
{
    let _ = shared.writer_wake.try_send(());
    let _ = shared.heartbeat_wake.try_send(());
    let _ = shared.deadline_wake.try_send(());
    shared
        .session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .terminate();
    shared
        .outbound_request_reservations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .pending_cancellations
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .pending_receive_credits
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    if let Some(abort) = &shared.transport_abort {
        abort.abort();
    }
    let waiters = match shared.waiters.lock() {
        Ok(mut waiters) => std::mem::take(&mut *waiters),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, pending) in waiters {
        let error = pending.failure_error(make_error());
        let _ = pending.sender.send(Err(error));
    }
    let raw_waiters = match shared.raw_waiters.lock() {
        Ok(mut raw_waiters) => std::mem::take(&mut *raw_waiters),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, pending) in raw_waiters {
        let error = pending.failure_error(make_error());
        let _ = pending.sender.send(Err(error));
    }
    let file_waiters = match shared.file_waiters.lock() {
        Ok(mut file_waiters) => std::mem::take(&mut *file_waiters),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, pending) in file_waiters {
        let error = transport_closed_before_final_error(make_error());
        pending.mailbox.fail(error);
    }
    let search_waiters = match shared.search_waiters.lock() {
        Ok(mut search_waiters) => std::mem::take(&mut *search_waiters),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, pending) in search_waiters {
        let error = transport_closed_before_final_error(make_error());
        pending.mailbox.fail(error);
    }
    let process_waiters = match shared.process_waiters.lock() {
        Ok(mut process_waiters) => std::mem::take(&mut *process_waiters),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, pending) in process_waiters {
        let error = transport_closed_before_final_error(make_error());
        pending.mailbox.fail(error);
    }
    let completed_file_streams = match shared.completed_file_streams.lock() {
        Ok(mut completed) => std::mem::take(&mut *completed),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, mailbox) in completed_file_streams {
        mailbox.fail(transport_closed_before_final_error(make_error()));
    }
    let completed_search_streams = match shared.completed_search_streams.lock() {
        Ok(mut completed) => std::mem::take(&mut *completed),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, mailbox) in completed_search_streams {
        mailbox.fail(transport_closed_before_final_error(make_error()));
    }
    let completed_process_streams = match shared.completed_process_streams.lock() {
        Ok(mut completed) => std::mem::take(&mut *completed),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    for (_, mailbox) in completed_process_streams {
        mailbox.fail(transport_closed_before_final_error(make_error()));
    }
    shared
        .watch_batches
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .watch_backlog
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .watch_stream_by_id
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    shared
        .directory_cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

pub(crate) fn fail_all_v5_waiters_for_error<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    error: &RemoteClientError,
) {
    match error {
        RemoteClientError::Io(error) => {
            let kind = error.kind();
            let message = error.to_string();
            fail_all_v5_waiters(shared, || {
                RemoteClientError::Io(io::Error::new(kind, message.clone()))
            });
        }
        RemoteClientError::Json(error) => {
            let message = error.to_string();
            fail_all_v5_waiters(shared, || {
                RemoteClientError::Protocol(format!(
                    "v5 transport closed after JSON error: {message}"
                ))
            });
        }
        RemoteClientError::Disconnected => {
            fail_all_v5_waiters(shared, || RemoteClientError::Disconnected);
        }
        RemoteClientError::TransportClosed { cause } => {
            fail_all_v5_waiters(shared, || RemoteClientError::TransportClosed {
                cause: cause.clone(),
            });
        }
        RemoteClientError::RequestDeadlineExceeded { method, kind } => {
            fail_all_v5_waiters(shared, || RemoteClientError::RequestDeadlineExceeded {
                method: method.clone(),
                kind: *kind,
            });
        }
        RemoteClientError::OutcomeUnknown { method, cause } => {
            fail_all_v5_waiters(shared, || RemoteClientError::OutcomeUnknown {
                method: method.clone(),
                cause: cause.clone(),
            });
        }
        RemoteClientError::ResponseIncomplete { cause } => {
            fail_all_v5_waiters(shared, || RemoteClientError::ResponseIncomplete {
                cause: cause.clone(),
            });
        }
        RemoteClientError::Protocol(message) => {
            fail_all_v5_waiters(shared, || RemoteClientError::Protocol(message.clone()));
        }
        RemoteClientError::Remote(error) => {
            fail_all_v5_waiters(shared, || RemoteClientError::Remote(error.clone()));
        }
    }
}

impl V5ResponseAccumulator {
    pub(crate) fn final_message_seen(&self) -> bool {
        self.method.is_some() || self.final_error.is_some()
    }

    #[cfg(test)]
    pub(crate) fn observe(
        &mut self,
        event: protocol_v5::StreamEvent,
    ) -> Option<std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>> {
        self.observe_inner(event, None)
    }

    pub(crate) fn observe_with_reservation(
        &mut self,
        event: protocol_v5::StreamEvent,
        reservation: &mut V5ByteReservation,
    ) -> Option<std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>> {
        self.observe_inner(event, Some(reservation))
    }

    fn observe_inner(
        &mut self,
        event: protocol_v5::StreamEvent,
        reservation: Option<&mut V5ByteReservation>,
    ) -> Option<std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError>> {
        match event {
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalResponse,
                envelope,
                ..
            } => {
                if let Err(error) = self.search_partials.finish_current() {
                    return Some(Err(error));
                }
                self.method = Some(envelope.method);
                None
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalError,
                envelope,
                ..
            } => {
                if let Err(error) = self.search_partials.finish_current() {
                    return Some(Err(error));
                }
                self.method = Some(envelope.method.clone());
                self.final_error = Some(match v5_final_error_from_envelope(envelope) {
                    Ok(error) => error,
                    Err(error) => return Some(Err(error)),
                });
                None
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::PartialResult,
                envelope,
                ..
            } => match self.search_partials.begin_partial(envelope.method) {
                Ok(()) => None,
                Err(error) => Some(Err(error)),
            },
            protocol_v5::StreamEvent::Data { channel, body, .. } => {
                let Some(received_bytes) = self.received_bytes.checked_add(body.len()) else {
                    return Some(Err(RemoteClientError::Protocol(
                        "v5 response decoded byte count overflowed".to_string(),
                    )));
                };
                if received_bytes > V5_MAX_ACCUMULATED_RESPONSE_BYTES {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 response exceeds decoded byte limit of {V5_MAX_ACCUMULATED_RESPONSE_BYTES}"
                    ))));
                }
                if let Some(reservation) = reservation
                    && let Err(error) = reservation.try_grow(body.len())
                {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 response exceeds connection retained-byte budget: {error}"
                    ))));
                }
                self.received_bytes = received_bytes;
                match channel {
                    protocol_v5::DataChannel::Unspecified => self.payload.extend(body),
                    protocol_v5::DataChannel::SearchPayload => {
                        self.search_partials.push_search_payload(body);
                    }
                    protocol_v5::DataChannel::FileBody | protocol_v5::DataChannel::Stdin => {
                        self.file_body.extend(body)
                    }
                    protocol_v5::DataChannel::Stdout => self.stdout.extend(body),
                    protocol_v5::DataChannel::Stderr => self.stderr.extend(body),
                }
                None
            }
            protocol_v5::StreamEvent::EndStream { stream_id } => {
                if let Some(error) = self.final_error.take() {
                    return Some(Err(RemoteClientError::Remote(error)));
                }
                let Some(method) = self.method.take() else {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 stream {stream_id} ended without final response"
                    ))));
                };
                let response = match self.search_partials.merge_final(&method, &self.payload) {
                    Ok(Some(response)) => response,
                    Ok(None) => match RemoteResponse::from_v5_payload(&method, &self.payload) {
                        Ok(response) => response,
                        Err(error) => return Some(Err(v5_method_error_to_client_error(error))),
                    },
                    Err(error) => return Some(Err(error)),
                };
                let body = v5_client_body_for_response(
                    &response,
                    std::mem::take(&mut self.file_body),
                    std::mem::take(&mut self.stdout),
                    std::mem::take(&mut self.stderr),
                );
                Some(Ok((response, body)))
            }
            protocol_v5::StreamEvent::ResetStream {
                code, diagnostic, ..
            } => Some(Err(RemoteClientError::Remote(RemoteError {
                code,
                message: "v5 stream reset".to_string(),
                diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
            }))),
            protocol_v5::StreamEvent::Headers { .. } => None,
        }
    }
}

impl V5RawResponseAccumulator {
    pub(crate) fn final_message_seen(&self) -> bool {
        self.final_seen || self.final_error.is_some()
    }

    #[cfg(test)]
    pub(crate) fn observe(
        &mut self,
        event: protocol_v5::StreamEvent,
    ) -> Option<std::result::Result<Vec<u8>, RemoteClientError>> {
        self.observe_inner(event, None)
    }

    fn observe_with_reservation(
        &mut self,
        event: protocol_v5::StreamEvent,
        reservation: &mut V5ByteReservation,
    ) -> Option<std::result::Result<Vec<u8>, RemoteClientError>> {
        self.observe_inner(event, Some(reservation))
    }

    fn observe_inner(
        &mut self,
        event: protocol_v5::StreamEvent,
        reservation: Option<&mut V5ByteReservation>,
    ) -> Option<std::result::Result<Vec<u8>, RemoteClientError>> {
        match event {
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalResponse,
                ..
            } => {
                self.final_seen = true;
                None
            }
            protocol_v5::StreamEvent::Headers {
                role: protocol_v5::MessageRole::FinalError,
                envelope,
                ..
            } => {
                self.final_error = Some(match v5_final_error_from_envelope(envelope) {
                    Ok(error) => error,
                    Err(error) => return Some(Err(error)),
                });
                None
            }
            protocol_v5::StreamEvent::Data { channel, body, .. } => {
                let Some(received_bytes) = self.received_bytes.checked_add(body.len()) else {
                    return Some(Err(RemoteClientError::Protocol(
                        "v5 raw response decoded byte count overflowed".to_string(),
                    )));
                };
                if received_bytes > V5_MAX_RAW_RESPONSE_BYTES {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 raw response exceeds decoded byte limit of {V5_MAX_RAW_RESPONSE_BYTES}"
                    ))));
                }
                match channel {
                    protocol_v5::DataChannel::Unspecified => {}
                    protocol_v5::DataChannel::SearchPayload => {}
                    protocol_v5::DataChannel::FileBody
                    | protocol_v5::DataChannel::Stdin
                    | protocol_v5::DataChannel::Stdout
                    | protocol_v5::DataChannel::Stderr => {
                        return Some(Err(RemoteClientError::Protocol(format!(
                            "unexpected v5 raw response data channel: {channel:?}"
                        ))));
                    }
                }
                if channel == protocol_v5::DataChannel::Unspecified {
                    if let Some(reservation) = reservation
                        && let Err(error) = reservation.try_grow(body.len())
                    {
                        return Some(Err(RemoteClientError::Protocol(format!(
                            "v5 raw response exceeds connection retained-byte budget: {error}"
                        ))));
                    }
                    self.payload.extend(body);
                }
                self.received_bytes = received_bytes;
                None
            }
            protocol_v5::StreamEvent::EndStream { stream_id } => {
                if let Some(error) = self.final_error.take() {
                    return Some(Err(RemoteClientError::Remote(error)));
                }
                if !self.final_seen {
                    return Some(Err(RemoteClientError::Protocol(format!(
                        "v5 raw stream {stream_id} ended without final response"
                    ))));
                }
                Some(Ok(std::mem::take(&mut self.payload)))
            }
            protocol_v5::StreamEvent::ResetStream {
                code, diagnostic, ..
            } => Some(Err(RemoteClientError::Remote(RemoteError {
                code,
                message: "v5 stream reset".to_string(),
                diagnostic: (!diagnostic.is_empty()).then_some(diagnostic),
            }))),
            protocol_v5::StreamEvent::Headers { .. } => None,
        }
    }
}

pub(crate) fn v5_client_lock_error<T>(_error: std::sync::PoisonError<T>) -> RemoteClientError {
    RemoteClientError::Protocol("v5 client lock is poisoned".to_string())
}

pub(crate) fn v5_method_error_to_client_error(error: V5MethodError) -> RemoteClientError {
    RemoteClientError::Protocol(error.to_string())
}

pub(crate) fn v5_method_error_to_remote_error(error: V5MethodError) -> RemoteError {
    RemoteError {
        code: "invalid_request".to_string(),
        message: error.to_string(),
        diagnostic: None,
    }
}

pub(crate) fn v5_final_error_from_envelope(
    envelope: protocol_v5::StreamEnvelope,
) -> std::result::Result<RemoteError, RemoteClientError> {
    match envelope.message {
        Some(protocol_v5::stream_envelope::Message::Error(error)) => Ok(RemoteError {
            code: error.code,
            message: error.message,
            diagnostic: (!error.details.is_empty()).then_some(error.details),
        }),
        _ => Err(RemoteClientError::Protocol(format!(
            "v5 final_error for {} omitted error payload",
            envelope.method
        ))),
    }
}

pub(crate) fn v5_client_body_for_response(
    response: &RemoteResponse,
    file_body: Vec<u8>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
) -> Vec<u8> {
    if matches!(response, RemoteResponse::RunProcess(_)) {
        let mut body = stdout;
        body.extend(stderr);
        body
    } else if !file_body.is_empty() {
        file_body
    } else {
        let mut body = stdout;
        body.extend(stderr);
        body
    }
}
