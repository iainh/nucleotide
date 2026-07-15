// Multiplexed client, connection, streaming, and backend adapter tests.

#[test]
fn dropping_v5_request_handle_resets_stream_and_releases_response_budget() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let handle = client
        .start_request(
            RemoteRequest::Stat {
                path: PathBuf::from("cancelled.rs"),
            },
            Vec::new(),
        )
        .unwrap();
    let cancelled_stream = handle.stream_id();
    wait_for_v5_stream_frame(&output, cancelled_stream, protocol_v5::FrameType::EndStream);
    input.push(v5_frames_bytes(vec![
        protocol_v5::stream_data_frame(
            cancelled_stream,
            vec![0; 64],
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap(),
    ]));
    let started = Instant::now();
    while client.shared.response_budget.used() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for partial response budget reservation"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(handle);

    wait_for_v5_stream_frame(
        &output,
        cancelled_stream,
        protocol_v5::FrameType::ResetStream,
    );
    wait_for_v5_outbound_request_reservation_release(&client.shared, cancelled_stream);
    let started = Instant::now();
    loop {
        let cleaned = client.shared.waiters.lock().unwrap().is_empty()
            && client
                .shared
                .pending_cancellations
                .lock()
                .unwrap()
                .is_empty()
            && client.shared.response_budget.used() == 0;
        if cleaned {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for dropped request cleanup"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let resets = read_v5_complete_frames(output.bytes())
        .into_iter()
        .filter(|frame| {
            frame.stream_id == cancelled_stream
                && frame.frame_type == protocol_v5::FrameType::ResetStream
        })
        .collect::<Vec<_>>();
    assert_eq!(resets.len(), 1);
    assert_eq!(
        resets[0]
            .decode_control::<protocol_v5::ResetStream>()
            .unwrap()
            .code,
        protocol_v5::RESET_CANCELLED
    );
    assert!(!client.shared.closed.load(Ordering::Acquire));

    let healthy_client = Arc::clone(&client);
    let healthy = std::thread::spawn(move || {
        healthy_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("healthy.rs"),
            },
            Vec::new(),
        )
    });
    let healthy_stream = wait_for_v5_request_stream_after(&output, "fs.stat", cancelled_stream);
    let response = RemoteResponse::Stat(FileStatResponse {
        path: PathBuf::from("healthy.rs"),
        kind: RemoteFileKind::File,
        size: 7,
        modified_unix_millis: None,
        modified_unix_nanos: None,
        readonly: false,
    });
    input.push(v5_frames_bytes(v5_response_frames(
        healthy_stream,
        "fs.stat",
        response.clone(),
        Vec::new(),
    )));

    assert_eq!(healthy.join().unwrap().unwrap(), (response, Vec::new()));
    assert!(!client.shared.closed.load(Ordering::Acquire));
    client.close();
    input.close();
}

// Multiplexed client transport, flow-control, streaming, and cache tests.

#[test]
fn v5_client_reader_rejects_frame_sequence_gap_after_handshake() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let request_client = Arc::clone(&client);
    let request = std::thread::spawn(move || {
        request_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("pending.rs"),
            },
            Vec::new(),
        )
    });
    wait_for_v5_request_stream(&output, "fs.stat");

    let mut ping = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Ping,
        0,
        &protocol_v5::PingPayload {
            token: b"sequence-gap".to_vec(),
        },
    );
    ping.frame_sequence = 4;
    let mut bytes = Vec::new();
    protocol_v5::write_frame(&mut bytes, &ping).unwrap();
    input.push_raw(bytes);

    let error = request.join().unwrap().unwrap_err();
    let RemoteClientError::Io(error) = error else {
        panic!("expected sequence I/O error, got {error:?}");
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("expected 3, got 4"));
    assert!(client.shared.closed.load(Ordering::Acquire));
    input.close();
}
#[test]
fn v5_concurrent_service_reader_rejects_frame_sequence_gap_after_handshake() {
    let temp = tempfile::tempdir().unwrap();
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_client_input(Vec::new()));
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let info = protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string());
    let service_input = input.clone();
    let service_thread = std::thread::spawn(move || {
        service.serve_v5_concurrent(protocol_v5::FramedIo::new(service_input, output), &info)
    });

    let mut ping = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Ping,
        0,
        &protocol_v5::PingPayload {
            token: b"sequence-gap".to_vec(),
        },
    );
    ping.frame_sequence = 4;
    let mut bytes = Vec::new();
    protocol_v5::write_frame(&mut bytes, &ping).unwrap();
    input.push_raw(bytes);

    let error = service_thread.join().unwrap().unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("failed to read v5 protocol frame"));
    assert!(message.contains("expected 3, got 4"), "{message}");
    input.close();
}

#[test]
fn v5_client_heartbeat_uses_negotiated_timing_and_exact_pong() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    let client_hello = v5_heartbeat_client_hello(Duration::from_millis(500));
    input.push(v5_server_input_for_client(
        Vec::new(),
        &client_hello,
        protocol_v5::ServerHandshakeInfo::current("/workspace"),
    ));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        client_hello,
    )
    .unwrap();

    {
        let heartbeat = client.shared.heartbeat.lock().unwrap();
        assert_eq!(
            heartbeat.idle_ping_interval,
            Duration::from_millis(u64::from(protocol_v5::IDLE_PING_INTERVAL_MS))
        );
        assert_eq!(heartbeat.ping_timeout, Duration::from_millis(500));
    }
    trigger_v5_client_idle_ping(&client.shared);

    let first_ping = wait_for_v5_connection_frame_after(&output, protocol_v5::FrameType::Ping, 2);
    let first_payload = first_ping
        .decode_control::<protocol_v5::PingPayload>()
        .unwrap();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        read_v5_frames(output.bytes())
            .into_iter()
            .filter(|frame| frame.frame_type == protocol_v5::FrameType::Ping)
            .count(),
        1,
        "only one heartbeat may be outstanding"
    );

    input.push(v5_frames_bytes(vec![protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Pong,
        0,
        &first_payload,
    )]));
    trigger_v5_client_idle_ping(&client.shared);
    let second_ping = wait_for_v5_connection_frame_after(
        &output,
        protocol_v5::FrameType::Ping,
        first_ping.frame_sequence,
    );
    let second_payload = second_ping
        .decode_control::<protocol_v5::PingPayload>()
        .unwrap();
    assert_ne!(second_payload.token, first_payload.token);
    assert!(!client.shared.closed.load(Ordering::Acquire));

    client.close();
    input.close();
}

#[test]
fn v5_client_wrong_pong_is_connection_terminal() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    let abort_calls = Arc::new(AtomicUsize::new(0));
    let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
        calls: Arc::clone(&abort_calls),
    });
    let client_hello = v5_heartbeat_client_hello(Duration::from_millis(500));
    input.push(v5_server_input_for_client(
        Vec::new(),
        &client_hello,
        protocol_v5::ServerHandshakeInfo::current("/workspace"),
    ));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            client_hello,
            Some(abort),
        )
        .unwrap(),
    );
    let request_client = Arc::clone(&client);
    let (result_sender, result_receiver) = mpsc::channel();
    let request = std::thread::spawn(move || {
        let result = request_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("pending.rs"),
            },
            Vec::new(),
        );
        result_sender.send(result).unwrap();
    });
    wait_for_v5_request_stream(&output, "fs.stat");
    trigger_v5_client_idle_ping(&client.shared);
    let _ping = wait_for_v5_connection_frame_after(&output, protocol_v5::FrameType::Ping, 2);

    input.push(v5_frames_bytes(vec![protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Pong,
        0,
        &protocol_v5::PingPayload {
            token: b"wrong".to_vec(),
        },
    )]));

    let error = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("wrong PONG should fail the pending request")
        .unwrap_err();
    let RemoteClientError::TransportClosed { cause } = error else {
        panic!("expected terminal heartbeat protocol error, got {error:?}");
    };
    assert!(cause.contains("unexpected heartbeat token"), "{cause}");
    assert!(client.shared.closed.load(Ordering::Acquire));
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    client.close();
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    input.close();
    request.join().unwrap();
}

#[test]
fn v5_client_missing_pong_times_out_waiter_and_aborts_once() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    let abort_calls = Arc::new(AtomicUsize::new(0));
    let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
        calls: Arc::clone(&abort_calls),
    });
    let client_hello = v5_heartbeat_client_hello(Duration::from_millis(80));
    input.push(v5_server_input_for_client(
        Vec::new(),
        &client_hello,
        protocol_v5::ServerHandshakeInfo::current("/workspace"),
    ));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            client_hello,
            Some(abort),
        )
        .unwrap(),
    );
    let request_client = Arc::clone(&client);
    let (result_sender, result_receiver) = mpsc::channel();
    let request = std::thread::spawn(move || {
        let result = request_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("pending.rs"),
            },
            Vec::new(),
        );
        result_sender.send(result).unwrap();
    });
    wait_for_v5_request_stream(&output, "fs.stat");
    trigger_v5_client_idle_ping(&client.shared);
    let _ping = wait_for_v5_connection_frame_after(&output, protocol_v5::FrameType::Ping, 2);

    let error = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("missing PONG should fail the pending request")
        .unwrap_err();
    let RemoteClientError::Io(error) = error else {
        panic!("expected heartbeat timeout, got {error:?}");
    };
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("peer did not answer"));
    assert!(client.shared.closed.load(Ordering::Acquire));
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    client.close();
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    input.close();
    request.join().unwrap();
}

#[test]
fn v5_client_heartbeat_aborts_writer_stalled_before_ping() {
    let input = BlockingRead::default();
    let writer = PausingWrite::default();
    let abort_calls = Arc::new(AtomicUsize::new(0));
    let abort: Arc<dyn V5TransportAbort> = Arc::new(ReleasingTransportAbort {
        writer: writer.clone(),
        calls: Arc::clone(&abort_calls),
    });
    let client_hello = v5_heartbeat_client_hello(Duration::from_millis(80));
    input.push(v5_server_input_for_client(
        Vec::new(),
        &client_hello,
        protocol_v5::ServerHandshakeInfo::current("/workspace"),
    ));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
            protocol_v5::FramedIo::new(input.clone(), writer.clone()),
            client_hello,
            Some(abort),
        )
        .unwrap(),
    );
    writer.pause_next_write();
    let request_client = Arc::clone(&client);
    let (result_sender, result_receiver) = mpsc::channel();
    let request = std::thread::spawn(move || {
        let result = request_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("blocked.rs"),
            },
            Vec::new(),
        );
        result_sender.send(result).unwrap();
    });
    writer.wait_until_paused();
    trigger_v5_client_idle_ping(&client.shared);

    let error = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("queued heartbeat should time out a stalled writer")
        .unwrap_err();
    let RemoteClientError::Io(error) = error else {
        panic!("expected stalled-writer heartbeat timeout, got {error:?}");
    };
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("writer did not send"));
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    client.close();
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    input.close();
    request.join().unwrap();
}

#[test]
fn v5_client_reader_routes_responses_while_writer_is_blocked() {
    let input = BlockingRead::default();
    let writer = PausingWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), writer.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    writer.pause_next_write();

    let request_client = Arc::clone(&client);
    let (result_sender, result_receiver) = mpsc::channel();
    let request = std::thread::spawn(move || {
        let result = request_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("responsive.rs"),
            },
            Vec::new(),
        );
        result_sender.send(result).unwrap();
    });
    writer.wait_until_paused();

    let ping = protocol_v5::PingPayload {
        token: b"reader-remains-responsive".to_vec(),
    };
    input.push(v5_frames_bytes(vec![protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Ping,
        0,
        &ping,
    )]));
    let response = RemoteResponse::Stat(FileStatResponse {
        path: PathBuf::from("responsive.rs"),
        kind: RemoteFileKind::File,
        size: 1,
        modified_unix_millis: None,
        modified_unix_nanos: None,
        readonly: false,
    });
    input.push(v5_frames_bytes(v5_response_frames(
        1,
        "fs.stat",
        response.clone(),
        Vec::new(),
    )));

    assert_eq!(
        result_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("client reader should not wait for the blocked writer")
            .unwrap(),
        (response, Vec::new())
    );

    writer.release();
    request.join().unwrap();
    let started = Instant::now();
    loop {
        let pong = read_v5_complete_frames(writer.bytes())
            .into_iter()
            .find(|frame| frame.frame_type == protocol_v5::FrameType::Pong);
        if let Some(pong) = pong {
            assert_eq!(
                pong.decode_control::<protocol_v5::PingPayload>().unwrap(),
                ping
            );
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for writer-pump PONG"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    client.close();
    input.close();
}

#[test]
fn v5_writer_failures_close_transport_and_fail_every_waiter() {
    enum Failure {
        AfterBytes(usize),
        AfterFlushes(usize),
    }

    let second_request = RemoteRequest::Stat {
        path: PathBuf::from("second.rs"),
    };
    let second_frames = v5_request_frames(3, &second_request, &[]);
    let first_frame_len = protocol_v5::FRAME_HEADER_LEN
        + second_frames[0].control.len()
        + second_frames[0].body.len();
    let partial_data_body =
        first_frame_len + protocol_v5::FRAME_HEADER_LEN + second_frames[1].control.len() + 1;
    assert!(second_frames[1].body.len() > 1);

    let failures = [
        ("before header", Failure::AfterBytes(0)),
        ("inside header", Failure::AfterBytes(7)),
        (
            "inside headers control",
            Failure::AfterBytes(protocol_v5::FRAME_HEADER_LEN + 1),
        ),
        ("inside data body", Failure::AfterBytes(partial_data_body)),
        ("data flush", Failure::AfterFlushes(0)),
    ];

    for (label, failure) in failures {
        let input = BlockingRead::default();
        let writer = FaultInjectingWrite::default();
        let output = writer.output();
        input.push(v5_server_input(Vec::new()));
        let client = Arc::new(
            RemoteWorkspaceV5MultiplexedClient::connect(
                protocol_v5::FramedIo::new(input.clone(), writer.clone()),
                protocol_v5::ClientHello::nucleotide("test-client"),
            )
            .unwrap(),
        );
        let handshake_flushes = writer.successful_flush_count();

        let first_client = Arc::clone(&client);
        let first = std::thread::spawn(move || {
            first_client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("first.rs"),
                },
                Vec::new(),
            )
        });
        wait_for_v5_request_stream(&output, "fs.stat");
        writer.wait_for_successful_flush_after(handshake_flushes);

        let (watch_sender, watch_receiver) = mpsc::sync_channel(1);
        client.shared.watch_batches.lock().unwrap().insert(
            2,
            V5WatchDelivery {
                sender: watch_sender,
                overflowed: Arc::new(AtomicBool::new(false)),
                last_sequence: Arc::new(AtomicU64::new(0)),
            },
        );
        client
            .shared
            .watch_stream_by_id
            .lock()
            .unwrap()
            .insert(1, 2);

        match failure {
            Failure::AfterBytes(bytes) => writer.fail_after_bytes(bytes),
            Failure::AfterFlushes(flushes) => writer.fail_after_flushes(flushes),
        }
        let second_error = client
            .request(second_request.clone(), Vec::new())
            .unwrap_err();
        let first_error = first.join().unwrap().unwrap_err();

        for error in [first_error, second_error] {
            let RemoteClientError::Io(error) = error else {
                panic!("{label}: expected I/O failure, got {error:?}");
            };
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe, "{label}");
        }
        assert!(client.shared.closed.load(Ordering::Acquire), "{label}");
        let cleanup_started = Instant::now();
        loop {
            let cleaned = client.shared.waiters.lock().unwrap().is_empty()
                && client.shared.file_waiters.lock().unwrap().is_empty()
                && client.shared.search_waiters.lock().unwrap().is_empty()
                && client.shared.process_waiters.lock().unwrap().is_empty()
                && client
                    .shared
                    .completed_file_streams
                    .lock()
                    .unwrap()
                    .is_empty()
                && client
                    .shared
                    .completed_search_streams
                    .lock()
                    .unwrap()
                    .is_empty()
                && client
                    .shared
                    .completed_process_streams
                    .lock()
                    .unwrap()
                    .is_empty()
                && client.shared.raw_waiters.lock().unwrap().is_empty()
                && client.shared.watch_batches.lock().unwrap().is_empty()
                && client.shared.watch_stream_by_id.lock().unwrap().is_empty();
            if cleaned {
                break;
            }
            assert!(
                cleanup_started.elapsed() < Duration::from_secs(2),
                "{label}: timed out waiting for terminal writer cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(client.shared.request_budget.used(), 0, "{label}");
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .is_empty(),
            "{label}"
        );
        assert!(client.shared.waiters.lock().unwrap().is_empty(), "{label}");
        assert!(
            client.shared.file_waiters.lock().unwrap().is_empty(),
            "{label}"
        );
        assert!(
            client.shared.search_waiters.lock().unwrap().is_empty(),
            "{label}"
        );
        assert!(
            client.shared.process_waiters.lock().unwrap().is_empty(),
            "{label}"
        );
        assert!(
            client
                .shared
                .completed_file_streams
                .lock()
                .unwrap()
                .is_empty(),
            "{label}"
        );
        assert!(
            client
                .shared
                .completed_process_streams
                .lock()
                .unwrap()
                .is_empty(),
            "{label}"
        );
        assert!(
            client
                .shared
                .completed_search_streams
                .lock()
                .unwrap()
                .is_empty(),
            "{label}"
        );
        assert!(
            client.shared.raw_waiters.lock().unwrap().is_empty(),
            "{label}"
        );
        assert!(
            client.shared.watch_batches.lock().unwrap().is_empty(),
            "{label}"
        );
        assert!(
            client.shared.watch_stream_by_id.lock().unwrap().is_empty(),
            "{label}"
        );
        assert!(matches!(
            watch_receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));

        let bytes_after_failure = output.bytes().len();
        assert!(matches!(
            client.request(
                RemoteRequest::Stat {
                    path: PathBuf::from("third.rs"),
                },
                Vec::new()
            ),
            Err(RemoteClientError::Disconnected)
        ));
        assert_eq!(output.bytes().len(), bytes_after_failure, "{label}");
        input.close();
    }
}

#[test]
fn v5_close_aborts_transport_without_waiting_for_blocked_writer() {
    let input = BlockingRead::default();
    let writer = PausingWrite::default();
    let abort_calls = Arc::new(AtomicUsize::new(0));
    let abort: Arc<dyn V5TransportAbort> = Arc::new(ReleasingTransportAbort {
        writer: writer.clone(),
        calls: Arc::clone(&abort_calls),
    });
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
            protocol_v5::FramedIo::new(input.clone(), writer.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
            Some(abort),
        )
        .unwrap(),
    );
    writer.pause_next_write();
    let request_client = Arc::clone(&client);
    let request = std::thread::spawn(move || {
        request_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("blocked.rs"),
            },
            Vec::new(),
        )
    });
    writer.wait_until_paused();
    assert!(client.shared.request_budget.used() > 0);
    assert!(
        !client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .is_empty()
    );

    let started = Instant::now();
    client.close();

    assert!(started.elapsed() < Duration::from_millis(250));
    client.close();
    assert_eq!(abort_calls.load(Ordering::SeqCst), 1);
    assert_eq!(client.shared.request_budget.used(), 0);
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        request.join().unwrap(),
        Err(RemoteClientError::Disconnected)
    ));
    input.close();
}

#[test]
fn v5_writer_revalidates_extracted_frames_after_local_reset() {
    let input = BlockingRead::default();
    let writer = PausingWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), writer.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    writer.pause_next_write();
    let request_client = Arc::clone(&client);
    let request = std::thread::spawn(move || {
        request_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("stale.rs"),
            },
            Vec::new(),
        )
    });
    writer.wait_until_paused();
    assert!(client.shared.request_budget.used() > 0);
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .contains_key(&1)
    );
    assert!(
        client
            .shared
            .session
            .lock()
            .unwrap()
            .reset_stream(1, protocol_v5::RESET_CANCELLED, "test reset")
            .unwrap()
    );
    writer.release();
    let started = Instant::now();
    loop {
        let bytes = writer.bytes();
        let mut cursor = Cursor::new(bytes);
        let mut reset_seen = false;
        while let Ok(Some(frame)) = protocol_v5::read_frame(&mut cursor) {
            reset_seen |=
                frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream;
        }
        if reset_seen {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for locally reset v5 stream"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    wait_for_v5_outbound_request_reservation_release(&client.shared, 1);
    assert_eq!(client.shared.request_budget.used(), 0);

    let stream_frames = read_v5_frames(writer.bytes())
        .into_iter()
        .filter(|frame| frame.stream_id == 1)
        .collect::<Vec<_>>();
    assert!(
        stream_frames
            .iter()
            .any(|frame| frame.frame_type == protocol_v5::FrameType::Headers)
    );
    assert!(
        stream_frames
            .iter()
            .any(|frame| frame.frame_type == protocol_v5::FrameType::ResetStream)
    );
    assert!(stream_frames.iter().all(|frame| {
        !matches!(
            frame.frame_type,
            protocol_v5::FrameType::Data | protocol_v5::FrameType::EndStream
        )
    }));

    client.close();
    assert!(matches!(
        request.join().unwrap(),
        Err(RemoteClientError::Disconnected)
    ));
    input.close();
}

#[test]
fn pending_response_disconnect_before_final_response_remains_retryable() {
    let (sender, _receiver) = mpsc::channel();
    let response_budget = V5ConnectionByteBudget::new(1);
    let pending = V5PendingResponse {
        sender,
        accumulator: V5ResponseAccumulator::default(),
        response_reservation: response_budget.reservation(),
        method: "fs.stat",
        idempotency: protocol_v5::Idempotency::ReadOnly,
        terminal_on_deadline: false,
        deadline: V5RequestDeadline::new(
            RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
            Instant::now(),
        ),
    };

    let error = pending.failure_error(RemoteClientError::Disconnected);

    assert!(matches!(error, RemoteClientError::Disconnected));
    assert!(remote_client_error_allows_reconnect_retry(&error));
}

#[test]
fn pending_mutation_disconnect_reports_unknown_outcome() {
    let (sender, _receiver) = mpsc::channel();
    let response_budget = V5ConnectionByteBudget::new(1);
    let pending = V5PendingResponse {
        sender,
        accumulator: V5ResponseAccumulator::default(),
        response_reservation: response_budget.reservation(),
        method: "fs.write",
        idempotency: protocol_v5::Idempotency::Mutation,
        terminal_on_deadline: true,
        deadline: V5RequestDeadline::new(
            RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
            Instant::now(),
        ),
    };

    let error = pending.failure_error(RemoteClientError::Disconnected);

    assert!(matches!(
        error,
        RemoteClientError::OutcomeUnknown { ref method, .. } if method == "fs.write"
    ));
    assert!(!remote_client_error_allows_reconnect_retry(&error));
    assert!(remote_client_error_requires_reconnect(&error));
}

#[test]
fn pending_response_disconnect_after_final_response_is_not_retryable() {
    let (sender, _receiver) = mpsc::channel();
    let response_budget = V5ConnectionByteBudget::new(1);
    let mut pending = V5PendingResponse {
        sender,
        accumulator: V5ResponseAccumulator::default(),
        response_reservation: response_budget.reservation(),
        method: "fs.stat",
        idempotency: protocol_v5::Idempotency::ReadOnly,
        terminal_on_deadline: false,
        deadline: V5RequestDeadline::new(
            RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
            Instant::now(),
        ),
    };
    pending.accumulator.method = Some("fs.stat".to_string());

    let error = pending.failure_error(RemoteClientError::Disconnected);

    assert!(matches!(
        error,
        RemoteClientError::ResponseIncomplete { .. }
    ));
    assert!(!remote_client_error_allows_reconnect_retry(&error));
    assert!(remote_client_error_requires_reconnect(&error));
}

#[test]
fn pending_raw_response_disconnect_after_final_error_is_not_retryable() {
    let (sender, _receiver) = mpsc::channel();
    let response_budget = V5ConnectionByteBudget::new(1);
    let mut pending = V5PendingRawResponse {
        sender,
        accumulator: V5RawResponseAccumulator::default(),
        response_reservation: response_budget.reservation(),
        method: "watch.start",
        deadline: V5RequestDeadline::new(
            RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
            Instant::now(),
        ),
    };
    pending.accumulator.final_error = Some(RemoteError {
        code: "UNAVAILABLE".to_string(),
        message: "remote closed".to_string(),
        diagnostic: None,
    });

    let error = pending.failure_error(RemoteClientError::Disconnected);

    assert!(matches!(
        error,
        RemoteClientError::ResponseIncomplete { .. }
    ));
    assert!(!remote_client_error_allows_reconnect_retry(&error));
    assert!(remote_client_error_requires_reconnect(&error));
}

#[test]
fn peer_deadline_maps_reads_mutations_and_watch_controls_safely() {
    fn deadline_error() -> RemoteClientError {
        RemoteClientError::Remote(RemoteError {
            code: protocol_v5::RESET_DEADLINE_EXCEEDED.to_string(),
            message: "deadline expired".to_string(),
            diagnostic: None,
        })
    }

    let response_budget = V5ConnectionByteBudget::new(1);
    let (sender, _receiver) = mpsc::channel();
    let read = V5PendingResponse {
        sender,
        accumulator: V5ResponseAccumulator::default(),
        response_reservation: response_budget.reservation(),
        method: "fs.stat",
        idempotency: protocol_v5::Idempotency::ReadOnly,
        terminal_on_deadline: false,
        deadline: V5RequestDeadline::new(
            RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
            Instant::now(),
        ),
    };
    let normalized = normalize_v5_response_deadline(&read, Err(deadline_error()));
    assert!(matches!(
        normalized.result,
        Err(RemoteClientError::RequestDeadlineExceeded {
            kind: RemoteRequestDeadlineKind::Absolute,
            ..
        })
    ));
    assert!(normalized.peer_deadline);
    assert!(!normalized.terminal);

    let (sender, _receiver) = mpsc::channel();
    let mut mutation = V5PendingResponse {
        sender,
        accumulator: V5ResponseAccumulator::default(),
        response_reservation: response_budget.reservation(),
        method: "fs.write",
        idempotency: protocol_v5::Idempotency::Mutation,
        terminal_on_deadline: true,
        deadline: V5RequestDeadline::new(
            RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
            Instant::now(),
        ),
    };
    let normalized = normalize_v5_response_deadline(&mutation, Err(deadline_error()));
    assert!(matches!(
        normalized.result,
        Err(RemoteClientError::OutcomeUnknown { ref method, .. }) if method == "fs.write"
    ));
    assert!(normalized.peer_deadline);
    assert!(normalized.terminal);

    mutation.accumulator.method = Some("fs.write".to_string());
    let normalized = normalize_v5_response_deadline(&mutation, Err(deadline_error()));
    assert!(matches!(
        normalized.result,
        Err(RemoteClientError::RequestDeadlineExceeded {
            kind: RemoteRequestDeadlineKind::Absolute,
            ..
        })
    ));
    assert!(normalized.peer_deadline);
    assert!(!normalized.terminal);

    let (sender, _receiver) = mpsc::channel();
    let mut shutdown = V5PendingResponse {
        sender,
        accumulator: V5ResponseAccumulator::default(),
        response_reservation: response_budget.reservation(),
        method: "session.shutdown",
        idempotency: protocol_v5::Idempotency::Mutation,
        terminal_on_deadline: true,
        deadline: V5RequestDeadline::new(
            RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
            Instant::now(),
        ),
    };
    shutdown.accumulator.method = Some("session.shutdown".to_string());
    assert!(shutdown.deadline_is_connection_terminal());
    let normalized = normalize_v5_response_deadline(&shutdown, Err(deadline_error()));
    assert!(matches!(
        normalized.result,
        Err(RemoteClientError::OutcomeUnknown { ref method, .. })
            if method == "session.shutdown"
    ));
    assert!(normalized.peer_deadline);
    assert!(normalized.terminal);

    let (sender, _receiver) = mpsc::channel();
    let raw = V5PendingRawResponse {
        sender,
        accumulator: V5RawResponseAccumulator::default(),
        response_reservation: response_budget.reservation(),
        method: "watch.start",
        deadline: V5RequestDeadline::new(
            RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::unlimited()),
            Instant::now(),
        ),
    };
    let normalized = normalize_v5_raw_response_deadline(&raw, Err(deadline_error()));
    assert!(matches!(
        normalized.result,
        Err(RemoteClientError::Io(ref error)) if error.kind() == io::ErrorKind::TimedOut
    ));
    assert!(normalized.peer_deadline);
    assert!(normalized.terminal);
}

#[test]
fn v5_service_task_pools_bound_classes_and_skip_blocked_front() {
    fn request(method: &str) -> V5ServiceRequest {
        let budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
        V5ServiceRequest {
            method: method.to_string(),
            priority: protocol_v5::Priority::Background,
            payload: Vec::new(),
            body: Vec::new(),
            retained_bytes: budget.reservation(),
            received_payload_bytes: 0,
            received_body_bytes: 0,
            deadline_unix_ms: 0,
            supersedes_stream_id: 0,
            streamed_write: None,
            early_error: None,
        }
    }

    let mut pools = V5ServiceTaskPools::default();
    for _ in 0..V5_SEARCH_WORKER_LIMIT {
        assert!(pools.can_start_method("search.text"));
        assert_eq!(
            pools.mark_started("search.text"),
            V5ServiceTaskClass::Search
        );
    }
    assert!(!pools.can_start_method("search.text"));

    pools.enqueue(1, request("search.text"));
    pools.enqueue(3, request("fs.stat"));
    let (stream_id, ready) = pools.pop_next_startable().unwrap();
    assert_eq!(stream_id, 3);
    assert_eq!(ready.method, "fs.stat");

    pools.mark_finished(V5ServiceTaskClass::Search);
    let (stream_id, ready) = pools.pop_next_startable().unwrap();
    assert_eq!(stream_id, 1);
    assert_eq!(ready.method, "search.text");

    let mut expired = request("git.status");
    let now_unix_ms = v5_now_unix_millis();
    expired.deadline_unix_ms = now_unix_ms.saturating_sub(1);
    pools.enqueue(5, expired);
    assert_eq!(pools.expired_pending_streams(now_unix_ms), vec![5]);
    assert!(pools.remove_pending(5));
    assert!(!pools.has_pending());

    pools.enqueue(7, request("fs.stat"));
    let mut urgent = request("fs.stat");
    urgent.priority = protocol_v5::Priority::UserInput;
    pools.enqueue(9, urgent);
    assert_eq!(pools.pop_next_startable().unwrap().0, 9);
    assert_eq!(pools.pop_next_startable().unwrap().0, 7);
}

#[test]
fn v5_service_request_rejects_aggregate_decoded_data_over_limit() {
    let budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
    let mut request = V5ServiceRequest::from_envelope(
        protocol_v5::StreamEnvelope::request(1, "search.text"),
        protocol_v5::Priority::Background,
        &budget,
    );
    request.received_payload_bytes = V5_MAX_REQUEST_PAYLOAD_BYTES;

    let error = request
        .reserve_data(protocol_v5::DataChannel::SearchPayload, 1, true)
        .unwrap_err();

    assert_eq!(error.code, "resource_exhausted");
    assert!(
        error
            .message
            .contains("request payload exceeds decoded byte limit")
    );
}

#[test]
fn v5_service_requests_compete_for_connection_budget_and_release_on_drop() {
    let budget = V5ConnectionByteBudget::new(10);
    let mut first = V5ServiceRequest::from_envelope(
        protocol_v5::StreamEnvelope::request(1, "fs.stat"),
        protocol_v5::Priority::Background,
        &budget,
    );
    let mut second = V5ServiceRequest::from_envelope(
        protocol_v5::StreamEnvelope::request(3, "fs.stat"),
        protocol_v5::Priority::Background,
        &budget,
    );

    first
        .reserve_data(protocol_v5::DataChannel::Unspecified, 6, true)
        .unwrap();
    let error = second
        .reserve_data(protocol_v5::DataChannel::Unspecified, 5, true)
        .unwrap_err();
    assert_eq!(error.code, "resource_exhausted");
    assert_eq!(budget.used(), 6);

    drop(first);
    second
        .reserve_data(protocol_v5::DataChannel::Unspecified, 5, true)
        .unwrap();
    assert_eq!(budget.used(), 5);
}

#[test]
fn v5_streamed_file_body_counts_stream_limit_without_retaining_connection_budget() {
    let budget = V5ConnectionByteBudget::new(1);
    let mut request = V5ServiceRequest::from_envelope(
        protocol_v5::StreamEnvelope::request(1, "fs.write"),
        protocol_v5::Priority::Background,
        &budget,
    );

    request
        .reserve_data(protocol_v5::DataChannel::FileBody, 1024, false)
        .unwrap();

    assert_eq!(request.received_body_bytes, 1024);
    assert_eq!(budget.used(), 0);
}

#[test]
fn v5_client_responses_compete_for_connection_budget_and_release_on_drop() {
    let budget = V5ConnectionByteBudget::new(10);
    let mut first_reservation = budget.reservation();
    let mut second_reservation = budget.reservation();
    let mut first = V5ResponseAccumulator::default();
    let mut second = V5ResponseAccumulator::default();

    assert!(
        first
            .observe_with_reservation(
                protocol_v5::StreamEvent::Data {
                    stream_id: 1,
                    channel: protocol_v5::DataChannel::FileBody,
                    uncompressed_len: 6,
                    body: vec![0; 6],
                },
                &mut first_reservation,
            )
            .is_none()
    );
    let error = second
        .observe_with_reservation(
            protocol_v5::StreamEvent::Data {
                stream_id: 3,
                channel: protocol_v5::DataChannel::FileBody,
                uncompressed_len: 5,
                body: vec![0; 5],
            },
            &mut second_reservation,
        )
        .expect("connection budget should reject the second response")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("connection retained-byte budget")
    );
    assert_eq!(budget.used(), 6);

    drop(first_reservation);
    assert!(
        second
            .observe_with_reservation(
                protocol_v5::StreamEvent::Data {
                    stream_id: 3,
                    channel: protocol_v5::DataChannel::FileBody,
                    uncompressed_len: 5,
                    body: vec![0; 5],
                },
                &mut second_reservation,
            )
            .is_none()
    );
    assert_eq!(budget.used(), 5);
}

#[test]
fn v5_client_requests_share_budget_validate_stream_limits_and_release() {
    let budget = V5ConnectionByteBudget::new(10);
    let first = reserve_v5_client_request_bytes(&budget, "fs.write", 4, 2).unwrap();
    let error = reserve_v5_client_request_bytes(&budget, "fs.write", 3, 2).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("connection retained-byte budget")
    );
    assert_eq!(budget.used(), 6);

    drop(first);
    let second = reserve_v5_client_request_bytes(&budget, "fs.write", 3, 2).unwrap();
    assert_eq!(budget.used(), 5);
    drop(second);
    assert_eq!(budget.used(), 0);

    let normal_budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
    let payload_error = reserve_v5_client_request_bytes(
        &normal_budget,
        "fs.stat",
        V5_MAX_REQUEST_PAYLOAD_BYTES + 1,
        0,
    )
    .unwrap_err();
    assert!(payload_error.to_string().contains("request payload"));
    let body_error = reserve_v5_client_request_bytes(
        &normal_budget,
        "fs.write",
        0,
        V5_MAX_REQUEST_BODY_BYTES + 1,
    )
    .unwrap_err();
    assert!(body_error.to_string().contains("request body"));
    assert_eq!(normal_budget.used(), 0);
}

#[test]
fn v5_client_does_not_open_request_after_deadline_while_waiting_for_session() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let context = RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::absolute_only(
        Duration::from_secs(2),
    ));
    let session = client.shared.session.lock().unwrap();
    let request_client = Arc::clone(&client);
    let worker = std::thread::spawn(move || {
        request_client.request_with_context(
            RemoteRequest::Stat {
                path: PathBuf::from("late.rs"),
            },
            Vec::new(),
            context,
        )
    });

    let started = Instant::now();
    while client.shared.request_budget.used() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "request did not reach the session lock before its deadline"
        );
        std::thread::yield_now();
    }
    std::thread::sleep(
        context
            .absolute_deadline
            .unwrap()
            .saturating_duration_since(Instant::now())
            + Duration::from_millis(10),
    );
    drop(session);

    let error = worker.join().unwrap().unwrap_err();
    assert!(matches!(
        error,
        RemoteClientError::RequestDeadlineExceeded {
            ref method,
            kind: RemoteRequestDeadlineKind::Absolute,
        } if method == "fs.stat"
    ));
    assert!(find_v5_request_stream(&output, "fs.stat").is_none());
    assert_eq!(client.shared.request_budget.used(), 0);
    client.close();
    input.close();
}

#[test]
fn v5_client_does_not_open_watch_control_after_deadline_while_waiting_for_session() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let context = RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::absolute_only(
        Duration::from_secs(2),
    ));
    let session = client.shared.session.lock().unwrap();
    let request_client = Arc::clone(&client);
    let worker = std::thread::spawn(move || {
        request_client.request_v5_raw("watch.stop", vec![1, 2, 3], context)
    });

    let started = Instant::now();
    while client.shared.request_budget.used() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "watch control did not reach the session lock before its deadline"
        );
        std::thread::yield_now();
    }
    std::thread::sleep(
        context
            .absolute_deadline
            .unwrap()
            .saturating_duration_since(Instant::now())
            + Duration::from_millis(10),
    );
    drop(session);

    let error = worker.join().unwrap().unwrap_err();
    assert!(matches!(
        error,
        RemoteClientError::RequestDeadlineExceeded {
            ref method,
            kind: RemoteRequestDeadlineKind::Absolute,
        } if method == "watch.stop"
    ));
    assert!(find_v5_request_stream(&output, "watch.stop").is_none());
    assert_eq!(client.shared.request_budget.used(), 0);
    client.close();
    input.close();
}

#[test]
fn v5_early_response_keeps_request_body_reserved_until_outbound_end_is_written() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let request = RemoteRequest::WriteFile {
        path: PathBuf::from("large.txt"),
        create_parent_dirs: false,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };
    let body_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
        + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
    let body = vec![7; body_len];
    let (_, encoded_payload) = request.to_v5_method_payload().unwrap();
    let retained_bytes = encoded_payload.len() + body_len;
    let request_client = Arc::clone(&client);
    let request_for_thread = request.clone();
    let worker = std::thread::spawn(move || request_client.request(request_for_thread, body));

    let stream_id = wait_for_v5_request_stream(&output, "fs.write");
    assert_eq!(client.shared.request_budget.used(), retained_bytes);
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .contains_key(&stream_id)
    );

    let response = RemoteResponse::WriteFile(WriteResultResponse {
        path: PathBuf::from("large.txt"),
        size: body_len as u64,
        modified_unix_millis: None,
        modified_unix_nanos: None,
    });
    input.push(v5_frames_bytes(v5_response_frames(
        stream_id,
        "fs.write",
        response.clone(),
        Vec::new(),
    )));
    assert_eq!(worker.join().unwrap().unwrap(), (response, Vec::new()));

    let stream_frames = read_v5_frames(output.bytes())
        .into_iter()
        .filter(|frame| frame.stream_id == stream_id)
        .collect::<Vec<_>>();
    assert!(
        stream_frames
            .iter()
            .all(|frame| { frame.frame_type != protocol_v5::FrameType::EndStream })
    );
    assert_eq!(client.shared.request_budget.used(), retained_bytes);
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .contains_key(&stream_id)
    );

    input.push(v5_frames_bytes(vec![
        protocol_v5::window_update_frame(stream_id, retained_bytes as u64).unwrap(),
    ]));
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
    wait_for_v5_outbound_request_reservation_release(&client.shared, stream_id);
    assert_eq!(client.shared.request_budget.used(), 0);

    client.close();
    input.close();
}

#[test]
fn dropping_backend_future_after_early_response_purges_flow_blocked_body() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let shared = Arc::clone(&client.shared);
    let backend = RemoteWorkspaceBackendImpl::new(loopback_identity(), client);
    let body_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
        + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
    let body = vec![7; body_len];
    let mut request =
        Box::pin(backend.write_file(Path::new("large.txt"), &body, WriteOptions::default()));
    let waker = futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);

    assert!(
        std::future::Future::poll(request.as_mut(), &mut context).is_pending(),
        "the early response must remain queued until the application polls again"
    );
    let stream_id = wait_for_v5_request_stream(&output, "fs.write");
    let response = RemoteResponse::WriteFile(WriteResultResponse {
        path: PathBuf::from("large.txt"),
        size: body_len as u64,
        modified_unix_millis: None,
        modified_unix_nanos: None,
    });
    input.push(v5_frames_bytes(v5_response_frames(
        stream_id,
        "fs.write",
        response,
        Vec::new(),
    )));

    let started = Instant::now();
    while Arc::strong_count(&backend.client) != 1
        || shared.waiters.lock().unwrap().contains_key(&stream_id)
    {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for the early response to reach the backend future"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .contains_key(&stream_id)
    );
    assert!(shared.request_budget.used() > 0);
    assert!(
        !read_v5_complete_frames(output.bytes()).iter().any(|frame| {
            frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::EndStream
        })
    );
    let data_frames_before_drop = read_v5_complete_frames(output.bytes())
        .into_iter()
        .filter(|frame| {
            frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::Data
        })
        .count();

    drop(request);

    wait_for_v5_outbound_request_reservation_release(&shared, stream_id);
    assert_eq!(shared.request_budget.used(), 0);
    let stream_frames = read_v5_complete_frames(output.bytes())
        .into_iter()
        .filter(|frame| frame.stream_id == stream_id)
        .collect::<Vec<_>>();
    let resets = stream_frames
        .iter()
        .filter(|frame| frame.frame_type == protocol_v5::FrameType::ResetStream)
        .collect::<Vec<_>>();
    assert!(resets.len() <= 1);
    if let Some(reset) = resets.first() {
        assert_eq!(
            reset
                .decode_control::<protocol_v5::ResetStream>()
                .unwrap()
                .code,
            protocol_v5::RESET_CANCELLED
        );
    }
    assert_eq!(
        stream_frames
            .iter()
            .filter(|frame| frame.frame_type == protocol_v5::FrameType::Data)
            .count(),
        data_frames_before_drop
    );
    assert!(
        stream_frames
            .iter()
            .all(|frame| frame.frame_type != protocol_v5::FrameType::EndStream)
    );
    assert!(!shared.closed.load(Ordering::Acquire));

    let healthy_client = Arc::clone(&backend.client);
    let healthy = std::thread::spawn(move || {
        healthy_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("healthy.rs"),
            },
            Vec::new(),
        )
    });
    let healthy_stream = wait_for_v5_request_stream_after(&output, "fs.stat", stream_id);
    let healthy_response = RemoteResponse::Stat(FileStatResponse {
        path: PathBuf::from("healthy.rs"),
        kind: RemoteFileKind::File,
        size: 7,
        modified_unix_millis: None,
        modified_unix_nanos: None,
        readonly: false,
    });
    input.push(v5_frames_bytes(v5_response_frames(
        healthy_stream,
        "fs.stat",
        healthy_response.clone(),
        Vec::new(),
    )));
    assert_eq!(
        healthy.join().unwrap().unwrap(),
        (healthy_response, Vec::new())
    );

    drop(backend);
    input.close();
}

#[test]
fn v5_early_raw_response_keeps_request_payload_reserved_until_outbound_end_is_written() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let payload_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
        + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
    let payload = vec![9; payload_len];
    let request_client = Arc::clone(&client);
    let worker = std::thread::spawn(move || {
        request_client.request_v5_raw("watch.resync", payload, v5_watch_control_request_context())
    });

    let stream_id = wait_for_v5_request_stream(&output, "watch.resync");
    assert_eq!(client.shared.request_budget.used(), payload_len);
    input.push(v5_frames_bytes(v5_raw_response_frames(
        stream_id,
        "watch.resync",
        vec![1, 2, 3],
    )));
    assert_eq!(worker.join().unwrap().unwrap(), vec![1, 2, 3]);

    assert_eq!(client.shared.request_budget.used(), payload_len);
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .contains_key(&stream_id)
    );
    input.push(v5_frames_bytes(vec![
        protocol_v5::window_update_frame(stream_id, payload_len as u64).unwrap(),
    ]));
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
    wait_for_v5_outbound_request_reservation_release(&client.shared, stream_id);
    assert_eq!(client.shared.request_budget.used(), 0);

    client.close();
    input.close();
}

#[test]
fn v5_malformed_early_end_releases_purged_request_reservation() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let request = RemoteRequest::WriteFile {
        path: PathBuf::from("malformed.txt"),
        create_parent_dirs: false,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };
    let body_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
        + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
    let request_client = Arc::clone(&client);
    let worker = std::thread::spawn(move || request_client.request(request, vec![5; body_len]));

    let stream_id = wait_for_v5_request_stream(&output, "fs.write");
    assert!(client.shared.request_budget.used() >= body_len);
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .contains_key(&stream_id)
    );

    input.push(v5_frames_bytes(vec![protocol_v5::Frame::new(
        protocol_v5::FrameType::EndStream,
        stream_id,
    )]));
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.to_string().contains("ended without final response"));
    wait_for_v5_outbound_request_reservation_release(&client.shared, stream_id);
    assert_eq!(client.shared.request_budget.used(), 0);
    assert_eq!(client.shared.session.lock().unwrap().queued_len(), 0);

    client.close();
    input.close();
}

#[test]
fn v5_incoming_reset_releases_flow_blocked_request_reservation() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let request = RemoteRequest::WriteFile {
        path: PathBuf::from("cancelled.txt"),
        create_parent_dirs: false,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };
    let body_len = protocol_v5::DEFAULT_STREAM_WINDOW as usize
        + protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize;
    let body = vec![3; body_len];
    let request_client = Arc::clone(&client);
    let worker = std::thread::spawn(move || request_client.request(request, body));

    let stream_id = wait_for_v5_request_stream(&output, "fs.write");
    assert!(client.shared.request_budget.used() >= body_len);
    input.push(v5_frames_bytes(vec![protocol_v5::reset_stream_frame(
        stream_id,
        protocol_v5::RESET_CANCELLED,
        "peer cancelled request",
    )]));

    let error = worker.join().unwrap().unwrap_err();
    assert!(matches!(error, RemoteClientError::Remote(_)));
    wait_for_v5_outbound_request_reservation_release(&client.shared, stream_id);
    assert_eq!(client.shared.request_budget.used(), 0);

    client.close();
    input.close();
}

#[test]
fn v5_waiter_registration_failure_rolls_back_request_reservation() {
    for raw in [false, true] {
        let input = BlockingRead::default();
        let output = SharedWrite::default();
        input.push(v5_server_input(Vec::new()));
        let client = RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap();

        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if raw {
                let _guard = client.shared.raw_waiters.lock().unwrap();
                panic!("poison raw v5 waiters");
            } else {
                let _guard = client.shared.waiters.lock().unwrap();
                panic!("poison v5 waiters");
            }
        }));
        assert!(poisoned.is_err());

        let failed = if raw {
            client
                .request_v5_raw(
                    "watch.stop",
                    vec![1, 2, 3],
                    v5_watch_control_request_context(),
                )
                .is_err()
        } else {
            client
                .request(
                    RemoteRequest::Stat {
                        path: PathBuf::from("poisoned.rs"),
                    },
                    Vec::new(),
                )
                .is_err()
        };
        assert!(failed);
        assert_eq!(client.shared.request_budget.used(), 0);
        assert!(
            client
                .shared
                .outbound_request_reservations
                .lock()
                .unwrap()
                .is_empty()
        );

        client.close();
        input.close();
    }
}

#[test]
fn v5_client_accumulators_reject_aggregate_decoded_data_over_limit() {
    let mut response = V5ResponseAccumulator {
        received_bytes: V5_MAX_ACCUMULATED_RESPONSE_BYTES,
        ..V5ResponseAccumulator::default()
    };
    let response_error = response
        .observe(protocol_v5::StreamEvent::Data {
            stream_id: 1,
            channel: protocol_v5::DataChannel::FileBody,
            uncompressed_len: 1,
            body: vec![0],
        })
        .expect("response limit should finish with an error")
        .unwrap_err();
    assert!(response_error.to_string().contains("decoded byte limit"));

    let mut raw = V5RawResponseAccumulator {
        received_bytes: V5_MAX_RAW_RESPONSE_BYTES,
        ..V5RawResponseAccumulator::default()
    };
    let raw_error = raw
        .observe(protocol_v5::StreamEvent::Data {
            stream_id: 3,
            channel: protocol_v5::DataChannel::Unspecified,
            uncompressed_len: 1,
            body: vec![0],
        })
        .expect("raw response limit should finish with an error")
        .unwrap_err();
    assert!(raw_error.to_string().contains("decoded byte limit"));
}

#[test]
fn v5_over_limit_stream_resets_without_credit_or_harming_another_stream() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let first_client = Arc::clone(&client);
    let first = std::thread::spawn(move || {
        first_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("oversized.rs"),
            },
            Vec::new(),
        )
    });
    let first_stream = wait_for_v5_request_stream(&output, "fs.stat");

    let second_client = Arc::clone(&client);
    let second = std::thread::spawn(move || {
        second_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("healthy.rs"),
            },
            Vec::new(),
        )
    });
    let second_stream = wait_for_v5_request_stream_after(&output, "fs.stat", first_stream);
    client
        .shared
        .waiters
        .lock()
        .unwrap()
        .get_mut(&first_stream)
        .unwrap()
        .accumulator
        .received_bytes = V5_MAX_ACCUMULATED_RESPONSE_BYTES;

    let mut frames = vec![
        protocol_v5::stream_data_frame(
            first_stream,
            vec![0],
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
        )
        .unwrap(),
    ];
    frames.extend(v5_response_frames(
        second_stream,
        "fs.stat",
        RemoteResponse::Stat(FileStatResponse {
            path: PathBuf::from("healthy.rs"),
            kind: RemoteFileKind::File,
            size: 1,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        }),
        Vec::new(),
    ));
    input.push(v5_frames_bytes(frames));

    let first_error = first.join().unwrap().unwrap_err();
    let (second_response, _) = second.join().unwrap().unwrap();

    assert!(first_error.to_string().contains("decoded byte limit"));
    assert!(matches!(second_response, RemoteResponse::Stat(_)));
    assert!(!client.shared.closed.load(Ordering::Acquire));
    let outbound = read_v5_frames(output.bytes());
    assert!(outbound.iter().any(|frame| {
        frame.stream_id == first_stream && frame.frame_type == protocol_v5::FrameType::ResetStream
    }));
    assert!(!outbound.iter().any(|frame| {
        frame.stream_id == first_stream && frame.frame_type == protocol_v5::FrameType::WindowUpdate
    }));
    client.close();
    input.close();
}

#[test]
fn v5_client_inactivity_is_stream_targeted_and_read_timeout_is_local() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let context = RemoteRequestContext::from_policy_at(
        RemoteRequestDeadlinePolicy::bounded(Duration::from_secs(60 * 60), Duration::from_secs(30)),
        Instant::now(),
        v5_now_unix_millis(),
    );

    let first_client = Arc::clone(&client);
    let (first_sender, first_receiver) = mpsc::channel();
    let first = std::thread::spawn(move || {
        let result = first_client.request_with_context(
            RemoteRequest::Stat {
                path: PathBuf::from("stalled.rs"),
            },
            Vec::new(),
            context,
        );
        first_sender.send(result).unwrap();
    });
    let first_stream = wait_for_v5_request_stream(&output, "fs.stat");
    wait_for_v5_stream_frame(&output, first_stream, protocol_v5::FrameType::EndStream);

    let second_client = Arc::clone(&client);
    let (second_sender, second_receiver) = mpsc::channel();
    let second = std::thread::spawn(move || {
        let result = second_client.request_with_context(
            RemoteRequest::Stat {
                path: PathBuf::from("healthy.rs"),
            },
            Vec::new(),
            context,
        );
        second_sender.send(result).unwrap();
    });
    let second_stream = wait_for_v5_request_stream_after(&output, "fs.stat", first_stream);
    wait_for_v5_stream_frame(&output, second_stream, protocol_v5::FrameType::EndStream);

    let recent = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    {
        let mut waiters = client.shared.waiters.lock().unwrap();
        waiters
            .get_mut(&first_stream)
            .unwrap()
            .deadline
            .last_progress_at = recent;
        waiters
            .get_mut(&second_stream)
            .unwrap()
            .deadline
            .last_progress_at = recent;
    }
    let after_sequence = read_v5_complete_frames(output.bytes())
        .into_iter()
        .map(|frame| frame.frame_sequence)
        .max()
        .unwrap_or(0);
    let ping = protocol_v5::PingPayload {
        token: b"deadline-progress-barrier".to_vec(),
    };
    input.push(v5_frames_bytes(vec![
        protocol_v5::window_update_frame(second_stream, 1).unwrap(),
        protocol_v5::Frame::from_control(protocol_v5::FrameType::Ping, 0, &ping),
    ]));
    let _ =
        wait_for_v5_connection_frame_after(&output, protocol_v5::FrameType::Pong, after_sequence);

    {
        let waiters = client.shared.waiters.lock().unwrap();
        assert_eq!(
            waiters
                .get(&first_stream)
                .unwrap()
                .deadline
                .last_progress_at,
            recent,
            "another stream and heartbeat traffic must not refresh the stalled request"
        );
        assert!(
            waiters
                .get(&second_stream)
                .unwrap()
                .deadline
                .last_progress_at
                > recent,
            "same-stream WINDOW_UPDATE should refresh inactivity"
        );
    }

    let expired_at = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
    {
        let mut waiters = client.shared.waiters.lock().unwrap();
        let pending = waiters.get_mut(&first_stream).unwrap();
        pending.accumulator.method = Some("fs.stat".to_string());
        pending.deadline.last_progress_at = expired_at;
    }
    signal_v5_client_deadlines(&client.shared);

    let error = first_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("deadline watchdog should finish the stalled read")
        .unwrap_err();
    assert!(matches!(
        error,
        RemoteClientError::RequestDeadlineExceeded {
            ref method,
            kind: RemoteRequestDeadlineKind::Inactivity,
        } if method == "fs.stat"
    ));
    wait_for_v5_stream_frame(&output, first_stream, protocol_v5::FrameType::ResetStream);
    let resets = read_v5_complete_frames(output.bytes())
        .into_iter()
        .filter(|frame| {
            frame.stream_id == first_stream
                && frame.frame_type == protocol_v5::FrameType::ResetStream
        })
        .collect::<Vec<_>>();
    assert_eq!(resets.len(), 1);
    assert_eq!(
        resets[0]
            .decode_control::<protocol_v5::ResetStream>()
            .unwrap()
            .code,
        protocol_v5::RESET_DEADLINE_EXCEEDED
    );
    assert!(!client.shared.closed.load(Ordering::Acquire));
    assert!(
        client
            .shared
            .waiters
            .lock()
            .unwrap()
            .contains_key(&second_stream)
    );

    let response = RemoteResponse::Stat(FileStatResponse {
        path: PathBuf::from("healthy.rs"),
        kind: RemoteFileKind::File,
        size: 7,
        modified_unix_millis: None,
        modified_unix_nanos: None,
        readonly: false,
    });
    input.push(v5_frames_bytes(v5_response_frames(
        second_stream,
        "fs.stat",
        response.clone(),
        Vec::new(),
    )));
    assert_eq!(
        second_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("second stream should remain usable")
            .unwrap(),
        (response, Vec::new())
    );

    first.join().unwrap();
    second.join().unwrap();
    client.close();
    input.close();
}

#[test]
fn v5_client_mutation_deadline_is_terminal_and_outcome_unknown() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    let abort_calls = Arc::new(AtomicUsize::new(0));
    let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
        calls: Arc::clone(&abort_calls),
    });
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
            Some(abort),
        )
        .unwrap(),
    );
    let context = RemoteRequestContext::from_policy_at(
        RemoteRequestDeadlinePolicy::bounded(Duration::from_secs(60 * 60), Duration::from_secs(30)),
        Instant::now(),
        v5_now_unix_millis(),
    );
    let request_client = Arc::clone(&client);
    let (result_sender, result_receiver) = mpsc::channel();
    let request = std::thread::spawn(move || {
        let result = request_client.request_with_context(
            RemoteRequest::CreateDir {
                path: PathBuf::from("possibly-created"),
            },
            Vec::new(),
            context,
        );
        result_sender.send(result).unwrap();
    });
    let stream_id = wait_for_v5_request_stream(&output, "fs.create_dir");
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
    {
        let mut waiters = client.shared.waiters.lock().unwrap();
        let pending = waiters.get_mut(&stream_id).unwrap();
        pending.deadline.last_progress_at =
            Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
    }
    {
        let mut heartbeat = client.shared.heartbeat.lock().unwrap();
        heartbeat.last_peer_activity = Instant::now();
        heartbeat.ping = None;
    }
    signal_v5_client_deadlines(&client.shared);

    let error = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("mutation deadline should finish the request")
        .unwrap_err();
    assert!(matches!(
        error,
        RemoteClientError::OutcomeUnknown { ref method, .. }
            if method == "fs.create_dir"
    ));
    assert!(client.shared.closed.load(Ordering::Acquire));
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    assert!(client.shared.waiters.lock().unwrap().is_empty());
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(client.shared.request_budget.used(), 0);
    assert_eq!(client.shared.response_budget.used(), 0);

    client.close();
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    request.join().unwrap();
    input.close();
}

#[test]
fn v5_client_final_metadata_wins_race_with_mutation_deadline() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let context = RemoteRequestContext::from_policy_at(
        RemoteRequestDeadlinePolicy::bounded(Duration::from_secs(60 * 60), Duration::from_secs(30)),
        Instant::now(),
        v5_now_unix_millis(),
    );
    let request_client = Arc::clone(&client);
    let (result_sender, result_receiver) = mpsc::channel();
    let request = std::thread::spawn(move || {
        let result = request_client.request_with_context(
            RemoteRequest::CreateDir {
                path: PathBuf::from("created-before-deadline"),
            },
            Vec::new(),
            context,
        );
        result_sender.send(result).unwrap();
    });
    let stream_id = wait_for_v5_request_stream(&output, "fs.create_dir");
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
    {
        let mut waiters = client.shared.waiters.lock().unwrap();
        waiters
            .get_mut(&stream_id)
            .unwrap()
            .deadline
            .last_progress_at = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
    }

    let mut heartbeat = client.shared.heartbeat.lock().unwrap();
    heartbeat.last_peer_activity = Instant::now();
    heartbeat.ping = None;
    let expiry_client = Arc::clone(&client);
    let expiry = std::thread::spawn(move || {
        expire_v5_client_deadlines_at(&expiry_client.shared, Instant::now()).unwrap()
    });
    assert!(handle_v5_client_stream_event(
        &client.shared,
        protocol_v5::StreamEvent::Headers {
            stream_id,
            role: protocol_v5::MessageRole::FinalResponse,
            priority: protocol_v5::Priority::UserInput,
            envelope: protocol_v5::StreamEnvelope::response(
                stream_id,
                "fs.create_dir",
                protocol_v5::MessageRole::FinalResponse,
                true,
            ),
        },
        None,
    ));
    drop(heartbeat);
    expiry.join().unwrap();

    let error = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("deadline should finish the mutation after final metadata")
        .unwrap_err();
    assert!(matches!(
        error,
        RemoteClientError::RequestDeadlineExceeded {
            ref method,
            kind: RemoteRequestDeadlineKind::Inactivity,
        } if method == "fs.create_dir"
    ));
    assert!(!client.shared.closed.load(Ordering::Acquire));
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::ResetStream);

    client.close();
    request.join().unwrap();
    input.close();
}

#[test]
fn v5_client_unknown_peer_deadline_fails_every_waiter() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    let abort_calls = Arc::new(AtomicUsize::new(0));
    let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
        calls: Arc::clone(&abort_calls),
    });
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
            Some(abort),
        )
        .unwrap(),
    );
    let context = RemoteRequestContext::from_policy_at(
        RemoteRequestDeadlinePolicy::bounded(Duration::from_secs(60 * 60), Duration::from_secs(30)),
        Instant::now(),
        v5_now_unix_millis(),
    );
    let first_client = Arc::clone(&client);
    let (first_sender, first_receiver) = mpsc::channel();
    let first = std::thread::spawn(move || {
        let result = first_client.request_with_context(
            RemoteRequest::Stat {
                path: PathBuf::from("stalled.rs"),
            },
            Vec::new(),
            context,
        );
        first_sender.send(result).unwrap();
    });
    let first_stream = wait_for_v5_request_stream(&output, "fs.stat");
    wait_for_v5_stream_frame(&output, first_stream, protocol_v5::FrameType::EndStream);

    let second_client = Arc::clone(&client);
    let (second_sender, second_receiver) = mpsc::channel();
    let second = std::thread::spawn(move || {
        let result = second_client.request_with_context(
            RemoteRequest::Stat {
                path: PathBuf::from("also-failed.rs"),
            },
            Vec::new(),
            context,
        );
        second_sender.send(result).unwrap();
    });
    let second_stream = wait_for_v5_request_stream_after(&output, "fs.stat", first_stream);
    wait_for_v5_stream_frame(&output, second_stream, protocol_v5::FrameType::EndStream);

    client
        .shared
        .waiters
        .lock()
        .unwrap()
        .get_mut(&first_stream)
        .unwrap()
        .deadline
        .last_progress_at = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
    {
        let mut heartbeat = client.shared.heartbeat.lock().unwrap();
        heartbeat.last_peer_activity = Instant::now()
            .checked_sub(heartbeat.idle_ping_interval + Duration::from_millis(1))
            .unwrap();
        heartbeat.ping = None;
    }
    signal_v5_client_deadlines(&client.shared);

    for receiver in [&first_receiver, &second_receiver] {
        let error = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("unknown peer expiry should fail every waiter")
            .unwrap_err();
        let RemoteClientError::Io(error) = error else {
            panic!("expected terminal peer-health timeout, got {error:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
    assert!(client.shared.closed.load(Ordering::Acquire));
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    assert!(client.shared.waiters.lock().unwrap().is_empty());
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(client.shared.request_budget.used(), 0);
    assert_eq!(client.shared.response_budget.used(), 0);

    first.join().unwrap();
    second.join().unwrap();
    input.close();
}

#[test]
fn v5_client_watch_control_deadline_is_connection_terminal() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    let abort_calls = Arc::new(AtomicUsize::new(0));
    let abort: Arc<dyn V5TransportAbort> = Arc::new(CountingTransportAbort {
        calls: Arc::clone(&abort_calls),
    });
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect_with_transport_abort(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
            Some(abort),
        )
        .unwrap(),
    );
    let context = RemoteRequestContext::from_policy_at(
        RemoteRequestDeadlinePolicy::bounded(Duration::from_secs(60 * 60), Duration::from_secs(30)),
        Instant::now(),
        v5_now_unix_millis(),
    );
    let request_client = Arc::clone(&client);
    let (result_sender, result_receiver) = mpsc::channel();
    let request = std::thread::spawn(move || {
        let result = request_client.request_v5_raw(
            "watch.resync",
            protocol_v5::WatchResync {
                watch_id: 7,
                roots: vec![".".to_string()],
            }
            .encode_to_vec(),
            context,
        );
        result_sender.send(result).unwrap();
    });
    let stream_id = wait_for_v5_request_stream(&output, "watch.resync");
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::EndStream);
    client
        .shared
        .raw_waiters
        .lock()
        .unwrap()
        .get_mut(&stream_id)
        .unwrap()
        .deadline
        .last_progress_at = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
    signal_v5_client_deadlines(&client.shared);

    let error = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("watch control deadline should finish the request")
        .unwrap_err();
    let RemoteClientError::Io(error) = error else {
        panic!("expected terminal watch timeout, got {error:?}");
    };
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(client.shared.closed.load(Ordering::Acquire));
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    assert!(client.shared.raw_waiters.lock().unwrap().is_empty());
    assert!(
        client
            .shared
            .outbound_request_reservations
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(client.shared.request_budget.used(), 0);
    assert_eq!(client.shared.response_budget.used(), 0);

    client.close();
    assert_eq!(abort_calls.load(Ordering::Acquire), 1);
    request.join().unwrap();
    input.close();
}

#[test]
fn v5_response_accumulator_merges_search_partials_with_final_tail() {
    let mut accumulator = V5ResponseAccumulator::default();
    let partial_payload = RemoteResponse::FileSearch(FileSearchResponse {
        root: PathBuf::new(),
        files: vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")],
        truncated: false,
    })
    .to_v5_payload()
    .unwrap();
    let split_at = partial_payload.len() / 2;

    assert!(
        accumulator
            .observe(protocol_v5::StreamEvent::Headers {
                stream_id: 1,
                role: protocol_v5::MessageRole::PartialResult,
                priority: protocol_v5::Priority::Background,
                envelope: protocol_v5::StreamEnvelope::response(
                    1,
                    "search.files",
                    protocol_v5::MessageRole::PartialResult,
                    false,
                ),
            })
            .is_none()
    );
    assert!(
        accumulator
            .observe(protocol_v5::StreamEvent::Data {
                stream_id: 1,
                channel: protocol_v5::DataChannel::SearchPayload,
                uncompressed_len: split_at as u64,
                body: partial_payload[..split_at].to_vec(),
            })
            .is_none()
    );
    assert!(
        accumulator
            .observe(protocol_v5::StreamEvent::Data {
                stream_id: 1,
                channel: protocol_v5::DataChannel::SearchPayload,
                uncompressed_len: (partial_payload.len() - split_at) as u64,
                body: partial_payload[split_at..].to_vec(),
            })
            .is_none()
    );

    let final_payload = RemoteResponse::FileSearch(FileSearchResponse {
        root: PathBuf::new(),
        files: vec![PathBuf::from("c.rs")],
        truncated: true,
    })
    .to_v5_payload()
    .unwrap();
    assert!(
        accumulator
            .observe(protocol_v5::StreamEvent::Headers {
                stream_id: 1,
                role: protocol_v5::MessageRole::FinalResponse,
                priority: protocol_v5::Priority::Background,
                envelope: protocol_v5::StreamEnvelope::response(
                    1,
                    "search.files",
                    protocol_v5::MessageRole::FinalResponse,
                    true,
                ),
            })
            .is_none()
    );
    assert!(
        accumulator
            .observe(protocol_v5::StreamEvent::Data {
                stream_id: 1,
                channel: protocol_v5::DataChannel::Unspecified,
                uncompressed_len: final_payload.len() as u64,
                body: final_payload,
            })
            .is_none()
    );

    let result = accumulator
        .observe(protocol_v5::StreamEvent::EndStream { stream_id: 1 })
        .expect("search response should complete")
        .unwrap();
    let (RemoteResponse::FileSearch(response), body) = result else {
        panic!("expected file search response");
    };
    assert!(body.is_empty());
    assert_eq!(
        response.files,
        vec![
            PathBuf::from("a.rs"),
            PathBuf::from("b.rs"),
            PathBuf::from("c.rs")
        ]
    );
    assert!(response.truncated);
}

#[test]
fn v5_method_payload_reports_unsupported_and_invalid_payloads() {
    let error = RemoteRequest::from_v5_method_payload("watch.start", b"{}").unwrap_err();
    assert_eq!(
        error,
        V5MethodError::UnsupportedMethod("watch.start".to_string())
    );

    let error = RemoteRequest::from_v5_method_payload("fs.stat", b"{").unwrap_err();
    assert!(matches!(
        error,
        V5MethodError::InvalidPayload { ref method, .. } if method == "fs.stat"
    ));

    let error = RemoteRequest::from_v5_method_payload("session.shutdown", br#"{"extra":true}"#)
        .unwrap_err();
    assert!(matches!(
        error,
        V5MethodError::InvalidPayload { ref method, .. } if method == "session.shutdown"
    ));
}

#[test]
fn v5_response_method_matches_request_namespace() {
    assert_eq!(RemoteResponse::Shutdown.v5_method(), "session.shutdown");
    assert_eq!(
        RemoteResponse::ReadFile(FileReadResponse {
            path: PathBuf::from("README.md"),
            size: 0,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
            truncated: false,
        })
        .v5_method(),
        "fs.read"
    );
}

#[test]
fn v5_client_writes_method_payload_body_and_decodes_write_response() {
    let request = RemoteRequest::WriteFile {
        path: PathBuf::from("src/lib.rs"),
        create_parent_dirs: true,
        expected_modified_unix_millis: Some(10),
        expected_modified_unix_nanos: Some(20),
    };
    let response = RemoteResponse::WriteFile(WriteResultResponse {
        path: PathBuf::from("src/lib.rs"),
        size: 7,
        modified_unix_millis: Some(11),
        modified_unix_nanos: Some(21),
    });
    let input = v5_server_input(v5_response_frames(
        1,
        "fs.write",
        response.clone(),
        Vec::new(),
    ));
    let mut client = RemoteWorkspaceV5Client::connect(
        protocol_v5::FramedIo::new(Cursor::new(input), Vec::new()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let (actual_response, actual_body) = client
        .request(request.clone(), b"new body".to_vec())
        .unwrap();
    assert_eq!(
        client.session.stream_tombstone(1),
        Some(protocol_v5::StreamTombstone::Closed)
    );
    let (_, output) = client.into_inner();
    let frames = read_v5_frames(output);

    assert_eq!(actual_response, response);
    assert!(actual_body.is_empty());
    assert_eq!(frames[0].frame_type, protocol_v5::FrameType::Hello);
    assert_eq!(frames[1].frame_type, protocol_v5::FrameType::SettingsAck);
    let request_headers = frames
        .iter()
        .find(|frame| frame.frame_type == protocol_v5::FrameType::Headers)
        .unwrap();
    let envelope = request_headers
        .decode_control::<protocol_v5::StreamEnvelope>()
        .unwrap();
    assert_eq!(envelope.method, "fs.write");
    assert_ne!(envelope.deadline_unix_ms, 0);
    assert_eq!(
        envelope.request_idempotency().unwrap(),
        protocol_v5::Idempotency::Mutation
    );

    let data_frames = frames
        .iter()
        .filter(|frame| frame.frame_type == protocol_v5::FrameType::Data)
        .collect::<Vec<_>>();
    assert_eq!(data_frames.len(), 2);
    let metadata = data_frames[0]
        .decode_control::<protocol_v5::DataEnvelope>()
        .unwrap();
    assert_eq!(
        protocol_v5::DataChannel::try_from(metadata.channel).unwrap(),
        protocol_v5::DataChannel::Unspecified
    );
    assert_eq!(
        RemoteRequest::from_v5_method_payload("fs.write", &data_frames[0].body).unwrap(),
        request
    );
    let body = data_frames[1]
        .decode_control::<protocol_v5::DataEnvelope>()
        .unwrap();
    assert_eq!(
        protocol_v5::DataChannel::try_from(body.channel).unwrap(),
        protocol_v5::DataChannel::FileBody
    );
    assert_eq!(data_frames[1].body, b"new body");
}

#[test]
fn v5_client_decodes_file_body_response() {
    let request = RemoteRequest::ReadFile {
        path: PathBuf::from("README.md"),
        max_bytes: None,
    };
    let response = RemoteResponse::ReadFile(FileReadResponse {
        path: PathBuf::from("README.md"),
        size: 11,
        modified_unix_millis: None,
        modified_unix_nanos: None,
        readonly: false,
        truncated: false,
    });
    let input = v5_server_input(v5_response_frames(
        1,
        "fs.read",
        response.clone(),
        b"hello world".to_vec(),
    ));
    let mut client = RemoteWorkspaceV5Client::connect(
        protocol_v5::FramedIo::new(Cursor::new(input), Vec::new()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let (actual_response, actual_body) = client.request(request, Vec::new()).unwrap();

    assert_eq!(actual_response, response);
    assert_eq!(actual_body, b"hello world");
}

#[test]
fn v5_client_returns_remote_error_after_final_error_headers() {
    let error = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        1,
        &protocol_v5::StreamEnvelope::error(
            1,
            "fs.stat",
            protocol_v5::ErrorHeader {
                code: "NOT_FOUND".to_string(),
                message: "missing".to_string(),
                retryable: false,
                details: "stat failed".to_string(),
                remote_errno: 2,
            },
        ),
    );
    let input = v5_server_input(vec![
        error,
        protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, 1),
    ]);
    let mut client = RemoteWorkspaceV5Client::connect(
        protocol_v5::FramedIo::new(Cursor::new(input), Vec::new()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let error = client
        .request(
            RemoteRequest::Stat {
                path: PathBuf::from("missing.txt"),
            },
            Vec::new(),
        )
        .unwrap_err();

    let RemoteClientError::Remote(error) = error else {
        panic!("expected remote error");
    };
    assert_eq!(error.code, "NOT_FOUND");
    assert_eq!(error.message, "missing");
    assert_eq!(error.diagnostic.as_deref(), Some("stat failed"));
}

#[test]
fn v5_sync_client_keeps_connection_after_mutation_deadline_with_final_metadata() {
    let final_metadata = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        1,
        &protocol_v5::StreamEnvelope::response(
            1,
            "fs.create_dir",
            protocol_v5::MessageRole::FinalResponse,
            true,
        ),
    );
    let reset = protocol_v5::reset_stream_frame(
        1,
        protocol_v5::RESET_DEADLINE_EXCEEDED,
        "response delivery deadline expired",
    );
    let healthy_response = RemoteResponse::Stat(FileStatResponse {
        path: PathBuf::from("healthy.rs"),
        kind: RemoteFileKind::File,
        size: 1,
        modified_unix_millis: None,
        modified_unix_nanos: None,
        readonly: false,
    });
    let mut response_frames = vec![final_metadata, reset];
    response_frames.extend(v5_response_frames(
        3,
        "fs.stat",
        healthy_response.clone(),
        Vec::new(),
    ));
    let input = v5_server_input(response_frames);
    let mut client = RemoteWorkspaceV5Client::connect(
        protocol_v5::FramedIo::new(Cursor::new(input), Vec::new()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let error = client
        .request(
            RemoteRequest::CreateDir {
                path: PathBuf::from("possibly-created"),
            },
            Vec::new(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        RemoteClientError::RequestDeadlineExceeded {
            ref method,
            kind: RemoteRequestDeadlineKind::Absolute,
        } if method == "fs.create_dir"
    ));

    let response = client
        .request(
            RemoteRequest::Stat {
                path: PathBuf::from("healthy.rs"),
            },
            Vec::new(),
        )
        .unwrap();
    assert_eq!(response, (healthy_response, Vec::new()));
}

#[test]
fn v5_backend_read_file_uses_shared_workspace_backend_impl() {
    let response = RemoteResponse::ReadFile(FileReadResponse {
        path: PathBuf::from("README.md"),
        size: 11,
        modified_unix_millis: None,
        modified_unix_nanos: None,
        readonly: false,
        truncated: false,
    });
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let (backend, hello) =
        RemoteWorkspaceV5Backend::connect(loopback_identity(), client).expect("v5 backend connect");
    let backend = Arc::new(backend);
    let worker_backend = Arc::clone(&backend);
    let worker = std::thread::spawn(move || {
        block_on(worker_backend.read_file(Path::new("README.md"), ReadOptions::default()))
    });

    let stream_id = wait_for_v5_request_stream(&output, "fs.read");
    input.push(v5_frames_bytes(v5_response_frames(
        stream_id,
        "fs.read",
        response,
        b"hello world".to_vec(),
    )));

    let read = worker.join().unwrap().expect("v5 read file");

    assert_eq!(hello.workspace_root, PathBuf::from("/workspace"));
    assert_eq!(read.path, PathBuf::from("README.md"));
    assert_eq!(read.bytes, b"hello world");
    input.close();
}

#[test]
fn v5_backend_start_watch_exposes_workspace_watch_batches() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let (backend, _) = RemoteWorkspaceV5Backend::connect(loopback_identity(), client).unwrap();
    let backend = Arc::new(backend);
    let worker_backend = Arc::clone(&backend);
    let worker = std::thread::spawn(move || {
        block_on(
            worker_backend.start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from(
                "/workspace",
            )])),
        )
    });

    let request_stream = wait_for_v5_request_stream(&output, "watch.start");
    let response = protocol_v5::WatchStartResponse {
        watch_id: 9,
        event_stream_id: 2,
        backend: "poll".to_string(),
        recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
        degraded: true,
        requires_reconciliation: true,
        accepted_roots: vec![".".to_string()],
        degraded_roots: Vec::new(),
        unsupported_roots: Vec::new(),
    };
    let batch = protocol_v5::WatchBatch {
        watch_id: 9,
        sequence: 1,
        directory_generations: vec![protocol_v5::WatchDirectoryGeneration {
            path: ".".to_string(),
            generation: 1,
        }],
        events: vec![protocol_v5::WatchChange::modified("src", true)],
        overflow: false,
        resync_required: false,
    };
    let mut frames = vec![v5_watch_event_open_frame(2, 9)];
    frames.extend(v5_raw_response_frames(
        request_stream,
        "watch.start",
        response.encode_to_vec(),
    ));
    frames.push(protocol_v5::watch_batch_frame(2, batch).unwrap());
    input.push(v5_frames_bytes(frames));

    let watch = worker
        .join()
        .unwrap()
        .unwrap()
        .expect("v5 watch should be supported");
    let received = watch.recv_timeout(Duration::from_secs(2)).unwrap();

    assert_eq!(watch.watch_id, 9);
    assert_eq!(
        received.directory_generations[0].path,
        PathBuf::from("/workspace")
    );
    assert_eq!(received.events[0].path, PathBuf::from("/workspace/src"));
    input.close();
}

#[test]
fn v5_backend_start_watch_returns_none_without_watch_capability() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    let mut info = protocol_v5::ServerHandshakeInfo::current("/workspace");
    info.capabilities.retain(|capability| capability != "watch");
    input.push(v5_server_input_with_info(Vec::new(), info));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let (backend, _) = RemoteWorkspaceV5Backend::connect(loopback_identity(), client).unwrap();

    let watch =
        block_on(
            backend.start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from(
                "/workspace",
            )])),
        )
        .unwrap();

    assert!(watch.is_none());
    assert!(find_v5_request_stream(&output, "watch.start").is_none());
    input.close();
}

#[test]
fn v5_multiplexed_client_receives_server_watch_batches() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let watch_client = Arc::clone(&client);
    let watch_thread = std::thread::spawn(move || {
        watch_client.start_v5_watch(protocol_v5::WatchStart::expanded_dirs(["."]))
    });
    let request_stream = wait_for_v5_request_stream(&output, "watch.start");
    let response = protocol_v5::WatchStartResponse {
        watch_id: 7,
        event_stream_id: 2,
        backend: "poll".to_string(),
        recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
        degraded: true,
        requires_reconciliation: true,
        accepted_roots: vec![".".to_string()],
        degraded_roots: Vec::new(),
        unsupported_roots: Vec::new(),
    };
    let batch = protocol_v5::WatchBatch {
        watch_id: 7,
        sequence: 1,
        directory_generations: vec![protocol_v5::WatchDirectoryGeneration {
            path: ".".to_string(),
            generation: 1,
        }],
        events: vec![protocol_v5::WatchChange::modified(".", true)],
        overflow: false,
        resync_required: false,
    };
    let mut frames = vec![v5_watch_event_open_frame(2, 7)];
    frames.extend(v5_raw_response_frames(
        request_stream,
        "watch.start",
        response.encode_to_vec(),
    ));
    // Exercise the backlog path: the first batch may arrive before start_watch
    // has registered its receiver after decoding the watch.start response.
    frames.push(protocol_v5::watch_batch_frame(2, batch.clone()).unwrap());
    input.push(v5_frames_bytes(frames));

    let watch = watch_thread
        .join()
        .unwrap()
        .expect("watch.start should succeed");
    let received = watch
        .recv_timeout(Duration::from_secs(2))
        .expect("watch batch should be delivered");

    assert_eq!(watch.watch_id, 7);
    assert_eq!(watch.event_stream_id, 2);
    assert_eq!(received.watch_id, batch.watch_id);
    assert_eq!(received.sequence, batch.sequence);
    assert_eq!(received.directory_generations[0].path, ".");
    assert_eq!(received.events[0].path, ".");
    input.close();
}

#[test]
fn v5_multiplexed_client_collapses_slow_watch_consumer_to_resync() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let watch_client = Arc::clone(&client);
    let watch_thread = std::thread::spawn(move || {
        watch_client.start_v5_watch(protocol_v5::WatchStart::expanded_dirs(["."]))
    });
    let request_stream = wait_for_v5_request_stream(&output, "watch.start");
    let response = protocol_v5::WatchStartResponse {
        watch_id: 7,
        event_stream_id: 2,
        backend: "poll".to_string(),
        recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
        degraded: true,
        requires_reconciliation: true,
        accepted_roots: vec![".".to_string()],
        degraded_roots: Vec::new(),
        unsupported_roots: Vec::new(),
    };
    let mut frames = vec![v5_watch_event_open_frame(2, 7)];
    frames.extend(v5_raw_response_frames(
        request_stream,
        "watch.start",
        response.encode_to_vec(),
    ));
    input.push(v5_frames_bytes(frames));
    let watch = watch_thread
        .join()
        .unwrap()
        .expect("watch.start should succeed");

    let batches = (1..=V5_WATCH_DELIVERY_CAPACITY + 1)
        .map(|sequence| protocol_v5::WatchBatch {
            watch_id: 7,
            sequence: sequence as u64,
            directory_generations: Vec::new(),
            events: vec![protocol_v5::WatchChange::modified(
                format!("file-{sequence}"),
                false,
            )],
            overflow: false,
            resync_required: false,
        })
        .map(|batch| protocol_v5::watch_batch_frame(2, batch).unwrap())
        .collect::<Vec<_>>();
    input.push(v5_frames_bytes(batches));

    let started = Instant::now();
    while !watch.overflowed.load(Ordering::Acquire) {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for local watch overflow"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let overflow = watch
        .recv_timeout(Duration::from_secs(2))
        .expect("local overflow should produce a resync batch");

    assert_eq!(overflow.watch_id, 7);
    assert_eq!(overflow.sequence, (V5_WATCH_DELIVERY_CAPACITY + 1) as u64);
    assert!(overflow.overflow);
    assert!(overflow.resync_required);
    assert!(overflow.events.is_empty());
    input.close();
}

#[test]
fn v5_multiplexed_client_updates_and_stops_watch() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let watch_client = Arc::clone(&client);
    let watch_thread = std::thread::spawn(move || {
        watch_client.start_v5_watch(protocol_v5::WatchStart::expanded_dirs(["."]))
    });
    let start_stream = wait_for_v5_request_stream(&output, "watch.start");
    let start_response = protocol_v5::WatchStartResponse {
        watch_id: 11,
        event_stream_id: 2,
        backend: "poll".to_string(),
        recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
        degraded: true,
        requires_reconciliation: true,
        accepted_roots: vec![".".to_string()],
        degraded_roots: Vec::new(),
        unsupported_roots: Vec::new(),
    };
    let mut frames = vec![v5_watch_event_open_frame(2, 11)];
    frames.extend(v5_raw_response_frames(
        start_stream,
        "watch.start",
        start_response.encode_to_vec(),
    ));
    input.push(v5_frames_bytes(frames));
    let watch = watch_thread.join().unwrap().unwrap();

    let update_client = Arc::clone(&client);
    let update_thread = std::thread::spawn(move || {
        update_client.update_v5_watch(protocol_v5::WatchUpdate {
            watch_id: 11,
            add_roots: vec!["src".to_string()],
            remove_roots: vec![".".to_string()],
        })
    });
    let update_stream = wait_for_v5_request_stream(&output, "watch.update");
    let update_response = protocol_v5::WatchUpdateResponse {
        watch_id: 11,
        accepted_roots: vec!["src".to_string()],
        degraded_roots: Vec::new(),
        unsupported_roots: Vec::new(),
    };
    input.push(v5_frames_bytes(v5_raw_response_frames(
        update_stream,
        "watch.update",
        update_response.encode_to_vec(),
    )));
    let update_response = update_thread.join().unwrap().unwrap();
    assert_eq!(update_response.accepted_roots, ["src"]);

    let resync_client = Arc::clone(&client);
    let resync_thread = std::thread::spawn(move || {
        resync_client.resync_v5_watch(protocol_v5::WatchResync {
            watch_id: 11,
            roots: vec!["src".to_string()],
        })
    });
    let resync_stream = wait_for_v5_request_stream(&output, "watch.resync");
    let resync_response = protocol_v5::WatchResyncResponse {
        watch_id: 11,
        accepted_roots: vec!["src".to_string()],
        unsupported_roots: Vec::new(),
    };
    input.push(v5_frames_bytes(v5_raw_response_frames(
        resync_stream,
        "watch.resync",
        resync_response.encode_to_vec(),
    )));
    let resync_response = resync_thread.join().unwrap().unwrap();
    assert_eq!(resync_response.accepted_roots, ["src"]);

    let stop_client = Arc::clone(&client);
    let stop_thread = std::thread::spawn(move || stop_client.stop_v5_watch(11));
    let stop_stream = wait_for_v5_request_stream(&output, "watch.stop");
    input.push(v5_frames_bytes(v5_raw_response_frames(
        stop_stream,
        "watch.stop",
        Vec::new(),
    )));
    stop_thread.join().unwrap().unwrap();

    assert!(matches!(
        watch.recv_timeout(Duration::from_millis(20)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    input.close();
}

#[test]
fn v5_multiplexed_client_uses_known_generation_and_cached_listing() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let full_listing = DirectoryListingResponse {
        path: PathBuf::from("src"),
        generation: Some(10),
        fingerprint: Some(20),
        complete: true,
        not_modified: false,
        delta: None,
        entries: vec![DirectoryEntryResponse {
            name: "lib.rs".to_string(),
            path: PathBuf::from("src/lib.rs"),
            stat: FileStatResponse {
                path: PathBuf::from("src/lib.rs"),
                kind: RemoteFileKind::File,
                size: 12,
                modified_unix_millis: None,
                modified_unix_nanos: None,
                readonly: false,
            },
            symlink_target: None,
            target_exists: None,
            ignored: Some(false),
        }],
    };

    let first_client = Arc::clone(&client);
    let first_thread = std::thread::spawn(move || {
        first_client.request(
            RemoteRequest::ListDir {
                path: PathBuf::from("src"),
            },
            Vec::new(),
        )
    });
    let first_stream = wait_for_v5_request_stream(&output, "fs.list_dir");
    let first_payload: V5DirectoryListPayload =
        decode_v5_request_payload(&output, first_stream).unwrap();
    assert_eq!(first_payload.path, PathBuf::from("src"));
    assert_eq!(first_payload.known_generation, None);
    input.push(v5_frames_bytes(v5_response_frames(
        first_stream,
        "fs.list_dir",
        RemoteResponse::ListDir(full_listing.clone()),
        Vec::new(),
    )));
    let (first_response, _) = first_thread.join().unwrap().unwrap();
    let RemoteResponse::ListDir(first_listing) = first_response else {
        panic!("expected first list_dir response");
    };
    assert_eq!(first_listing.entries.len(), 1);

    let second_client = Arc::clone(&client);
    let second_thread = std::thread::spawn(move || {
        second_client.request(
            RemoteRequest::ListDir {
                path: PathBuf::from("src"),
            },
            Vec::new(),
        )
    });
    let second_stream = wait_for_v5_request_stream_after(&output, "fs.list_dir", first_stream);
    let second_payload: V5DirectoryListPayload =
        decode_v5_request_payload(&output, second_stream).unwrap();
    assert_eq!(second_payload.known_generation, Some(10));
    assert_eq!(second_payload.known_fingerprint, Some(20));
    input.push(v5_frames_bytes(v5_response_frames(
        second_stream,
        "fs.list_dir",
        RemoteResponse::ListDir(DirectoryListingResponse {
            path: PathBuf::from("src"),
            generation: Some(10),
            fingerprint: Some(20),
            complete: true,
            not_modified: true,
            delta: None,
            entries: Vec::new(),
        }),
        Vec::new(),
    )));
    let (second_response, _) = second_thread.join().unwrap().unwrap();
    let RemoteResponse::ListDir(second_listing) = second_response else {
        panic!("expected cached list_dir response");
    };
    assert_eq!(second_listing.entries, full_listing.entries);
    assert!(!second_listing.not_modified);
    input.close();
}

fn v5_test_directory_entry(path: &str, size: u64) -> DirectoryEntryResponse {
    let path = PathBuf::from(path);
    let name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new(""))
        .to_string_lossy()
        .into_owned();
    DirectoryEntryResponse {
        name,
        path: path.clone(),
        stat: FileStatResponse {
            path,
            kind: RemoteFileKind::File,
            size,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        },
        symlink_target: None,
        target_exists: None,
        ignored: Some(false),
    }
}

fn v5_test_directory_listing(
    path: &str,
    generation: u64,
    fingerprint: u64,
    entries: Vec<DirectoryEntryResponse>,
) -> DirectoryListingResponse {
    DirectoryListingResponse {
        path: PathBuf::from(path),
        generation: Some(generation),
        fingerprint: Some(fingerprint),
        complete: true,
        not_modified: false,
        delta: None,
        entries,
    }
}

#[test]
fn v5_multiplexed_client_clears_directory_cache_after_watch_resync() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let first_client = Arc::clone(&client);
    let first_thread = std::thread::spawn(move || {
        first_client.request(
            RemoteRequest::ListDir {
                path: PathBuf::from("src"),
            },
            Vec::new(),
        )
    });
    let first_stream = wait_for_v5_request_stream(&output, "fs.list_dir");
    input.push(v5_frames_bytes(v5_response_frames(
        first_stream,
        "fs.list_dir",
        RemoteResponse::ListDir(v5_test_directory_listing(
            "src",
            10,
            20,
            vec![v5_test_directory_entry("src/lib.rs", 12)],
        )),
        Vec::new(),
    )));
    first_thread.join().unwrap().unwrap();

    let watch_client = Arc::clone(&client);
    let watch_thread = std::thread::spawn(move || {
        watch_client.start_v5_watch(protocol_v5::WatchStart::expanded_dirs(["src"]))
    });
    let watch_stream = wait_for_v5_request_stream_after(&output, "watch.start", first_stream);
    let watch_response = protocol_v5::WatchStartResponse {
        watch_id: 7,
        event_stream_id: 2,
        backend: "poll".to_string(),
        recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
        degraded: true,
        requires_reconciliation: true,
        accepted_roots: vec!["src".to_string()],
        degraded_roots: vec!["src".to_string()],
        unsupported_roots: Vec::new(),
    };
    let mut watch_frames = vec![v5_watch_event_open_frame(2, 7)];
    watch_frames.extend(v5_raw_response_frames(
        watch_stream,
        "watch.start",
        watch_response.encode_to_vec(),
    ));
    input.push(v5_frames_bytes(watch_frames));
    let watch = watch_thread.join().unwrap().unwrap();

    let resync_batch = protocol_v5::WatchBatch {
        watch_id: 7,
        sequence: 1,
        directory_generations: vec![protocol_v5::WatchDirectoryGeneration {
            path: "src".to_string(),
            generation: 11,
        }],
        events: Vec::new(),
        overflow: true,
        resync_required: true,
    };
    input.push(v5_frames_bytes(vec![
        protocol_v5::watch_batch_frame(2, resync_batch).unwrap(),
    ]));
    watch.recv_timeout(Duration::from_secs(2)).unwrap();

    let second_client = Arc::clone(&client);
    let second_thread = std::thread::spawn(move || {
        second_client.request(
            RemoteRequest::ListDir {
                path: PathBuf::from("src"),
            },
            Vec::new(),
        )
    });
    let second_stream = wait_for_v5_request_stream_after(&output, "fs.list_dir", watch_stream);
    let second_payload: V5DirectoryListPayload =
        decode_v5_request_payload(&output, second_stream).unwrap();
    assert_eq!(second_payload.known_generation, None);
    assert_eq!(second_payload.known_fingerprint, None);
    input.push(v5_frames_bytes(v5_response_frames(
        second_stream,
        "fs.list_dir",
        RemoteResponse::ListDir(v5_test_directory_listing(
            "src",
            11,
            21,
            vec![v5_test_directory_entry("src/lib.rs", 12)],
        )),
        Vec::new(),
    )));
    second_thread.join().unwrap().unwrap();
    input.close();
}

#[test]
fn v5_multiplexed_client_enables_zstd_for_directory_requests_when_negotiated() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input_with_compression(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let request_client = Arc::clone(&client);
    let request_thread = std::thread::spawn(move || {
        request_client.request(
            RemoteRequest::ListDir {
                path: PathBuf::from("src"),
            },
            Vec::new(),
        )
    });
    let stream_id = wait_for_v5_request_stream(&output, "fs.list_dir");
    let bytes = output.bytes();
    let mut cursor = Cursor::new(bytes);
    let mut content_encoding = protocol_v5::ContentEncoding::None;
    while let Some(frame) = protocol_v5::read_frame(&mut cursor).unwrap() {
        if frame.stream_id == stream_id && frame.frame_type == protocol_v5::FrameType::Headers {
            let envelope = frame
                .decode_control::<protocol_v5::StreamEnvelope>()
                .unwrap();
            content_encoding = envelope.decode_content_encoding().unwrap();
            break;
        }
    }
    assert_eq!(content_encoding, protocol_v5::ContentEncoding::Zstd);
    let payload: V5DirectoryListPayload = decode_v5_request_payload(&output, stream_id).unwrap();
    assert_eq!(payload.path, PathBuf::from("src"));

    input.push(v5_frames_bytes(v5_response_frames_with_content_encoding(
        stream_id,
        "fs.list_dir",
        RemoteResponse::ListDir(v5_test_directory_listing(
            "src",
            1,
            2,
            vec![v5_test_directory_entry("src/lib.rs", 12)],
        )),
        Vec::new(),
        protocol_v5::ContentEncoding::Zstd,
    )));
    let (response, _) = request_thread.join().unwrap().unwrap();
    let RemoteResponse::ListDir(listing) = response else {
        panic!("expected compressed list_dir response");
    };
    assert_eq!(listing.entries.len(), 1);
    input.close();
}

#[test]
fn v5_multiplexed_client_writes_window_updates_after_receiving_data() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let request_client = Arc::clone(&client);
    let request_thread = std::thread::spawn(move || {
        request_client.request(
            RemoteRequest::ReadFile {
                path: PathBuf::from("README.md"),
                max_bytes: None,
            },
            Vec::new(),
        )
    });
    let stream_id = wait_for_v5_request_stream(&output, "fs.read");
    input.push(v5_frames_bytes(v5_response_frames(
        stream_id,
        "fs.read",
        RemoteResponse::ReadFile(FileReadResponse {
            path: PathBuf::from("README.md"),
            size: 11,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
            truncated: false,
        }),
        b"hello world".to_vec(),
    )));

    let (_, body) = request_thread.join().unwrap().unwrap();
    assert_eq!(body, b"hello world");

    let frames = read_v5_frames(output.bytes());
    let mut connection_credit = 0_u64;
    let mut stream_credit = 0_u64;
    for frame in frames
        .iter()
        .filter(|frame| frame.frame_type == protocol_v5::FrameType::WindowUpdate)
    {
        let update = frame.decode_control::<protocol_v5::WindowUpdate>().unwrap();
        if frame.stream_id == 0 {
            connection_credit += update.credit_bytes;
        } else if frame.stream_id == stream_id {
            stream_credit += update.credit_bytes;
        }
    }
    assert!(connection_credit >= 11);
    assert!(stream_credit <= connection_credit);
    input.close();
}

#[test]
fn v5_file_stream_releases_body_credit_only_after_chunk_delivery() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let mut stream = client
        .read_file_stream(PathBuf::from("README.md"), None)
        .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "fs.read");
    let response = FileReadResponse {
        path: PathBuf::from("README.md"),
        size: 11,
        modified_unix_millis: None,
        modified_unix_nanos: None,
        readonly: false,
        truncated: false,
    };
    let frames = v5_response_frames(
        stream_id,
        "fs.read",
        RemoteResponse::ReadFile(response.clone()),
        b"hello world".to_vec(),
    );
    input.push(v5_frames_bytes(frames.clone()));

    let started = Instant::now();
    while client.shared.response_budget.used() != 11 {
        if started.elapsed() >= Duration::from_secs(2) {
            panic!(
                "timed out waiting for retained file chunk: budget={}, file_waiters={}, ordinary_waiters={}, closed={}",
                client.shared.response_budget.used(),
                client.shared.file_waiters.lock().unwrap().len(),
                client.shared.waiters.lock().unwrap().len(),
                client.shared.closed.load(Ordering::Acquire),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let retained_before_poll = client.shared.response_budget.used();
    let credits = |bytes: Vec<u8>| {
        let mut connection = 0_u64;
        let mut file_stream = 0_u64;
        for frame in read_v5_complete_frames(bytes)
            .into_iter()
            .filter(|frame| frame.frame_type == protocol_v5::FrameType::WindowUpdate)
        {
            let update = frame.decode_control::<protocol_v5::WindowUpdate>().unwrap();
            if frame.stream_id == 0 {
                connection += update.credit_bytes;
            } else if frame.stream_id == stream_id {
                file_stream += update.credit_bytes;
            }
        }
        (connection, file_stream)
    };
    let before_poll = credits(output.bytes());

    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteFileReadEvent::Chunk(b"hello world".to_vec())
    );
    assert_eq!(
        client.shared.response_budget.used(),
        retained_before_poll - 11
    );
    let started = Instant::now();
    let after_poll = loop {
        let current = credits(output.bytes());
        if current.0 >= before_poll.0 + 11 {
            break current;
        }
        if started.elapsed() >= Duration::from_secs(2) {
            panic!(
                "timed out waiting for consumption-based file credit: before={before_poll:?}, current={current:?}, pending={:?}, closed={}",
                client.shared.pending_receive_credits.lock().unwrap(),
                client.shared.closed.load(Ordering::Acquire),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(after_poll.0 - before_poll.0, 11);
    assert_eq!(after_poll.1 - before_poll.1, 0);

    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteFileReadEvent::Complete(response)
    );
    assert!(futures::executor::block_on(stream.next()).is_none());
    input.close();
}

#[test]
fn v5_process_stream_delivers_channels_and_credits_consumed_output() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let mut stream = client
        .run_process_stream(
            ProcessRequest {
                program: "command".to_string(),
                args: Vec::new(),
                cwd: PathBuf::from("."),
                env: BTreeMap::new(),
                clear_env: false,
                inherit_project_environment: false,
                max_output_bytes: None,
                timeout_ms: None,
            },
            Vec::new(),
        )
        .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "process.run");
    let response = ProcessOutputResponse {
        status_code: Some(0),
        success: true,
        stdout_truncated: false,
        stderr_truncated: false,
        stdout_len: 6,
        stderr_len: 6,
        timed_out: false,
    };
    let payload = RemoteResponse::RunProcess(response.clone())
        .to_v5_payload()
        .unwrap();
    let frames = vec![
        protocol_v5::stream_data_frame(
            stream_id,
            b"stdout".to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Stdout),
        )
        .unwrap(),
        protocol_v5::stream_data_frame(
            stream_id,
            b"stderr".to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Stderr),
        )
        .unwrap(),
        protocol_v5::stream_data_frame(
            stream_id,
            payload.clone(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap(),
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::response(
                stream_id,
                "process.run",
                protocol_v5::MessageRole::FinalResponse,
                true,
            ),
        ),
        protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, stream_id),
    ];
    input.push(v5_frames_bytes(frames));

    let retained = 12 + payload.len();
    let started = Instant::now();
    while client.shared.response_budget.used() != retained {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for buffered process output"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let connection_credit = || {
        read_v5_complete_frames(output.bytes())
            .into_iter()
            .filter(|frame| {
                frame.stream_id == 0 && frame.frame_type == protocol_v5::FrameType::WindowUpdate
            })
            .map(|frame| {
                frame
                    .decode_control::<protocol_v5::WindowUpdate>()
                    .unwrap()
                    .credit_bytes
            })
            .sum::<u64>()
    };
    let before_poll = connection_credit();

    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteProcessEvent::Stdout(b"stdout".to_vec())
    );
    assert_eq!(client.shared.response_budget.used(), retained - 6);
    let started = Instant::now();
    while connection_credit() < before_poll + 6 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for stdout consumption credit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteProcessEvent::Stderr(b"stderr".to_vec())
    );
    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteProcessEvent::Complete(response)
    );
    assert!(futures::executor::block_on(stream.next()).is_none());
    assert_eq!(client.shared.response_budget.used(), 0);
    input.close();
}

#[test]
fn v5_process_stream_peer_deadline_before_final_is_outcome_unknown() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let mut stream = client
        .run_process_stream(
            ProcessRequest {
                program: "command".to_string(),
                args: Vec::new(),
                cwd: PathBuf::from("."),
                env: BTreeMap::new(),
                clear_env: false,
                inherit_project_environment: false,
                max_output_bytes: None,
                timeout_ms: Some(1),
            },
            Vec::new(),
        )
        .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "process.run");
    input.push(v5_frames_bytes(vec![protocol_v5::reset_stream_frame(
        stream_id,
        protocol_v5::RESET_DEADLINE_EXCEEDED,
        "process deadline",
    )]));

    assert!(matches!(
        futures::executor::block_on(stream.next()),
        Some(Err(RemoteClientError::OutcomeUnknown { method, .. }))
            if method == "process.run"
    ));
    let started = Instant::now();
    while !client.shared.closed.load(Ordering::Acquire) {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for ambiguous process deadline to close transport"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    input.close();
}

#[test]
fn v5_file_search_stream_delivers_partials_and_credits_consumed_batches() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let mut stream = client
        .file_search_stream(FileSearchRequest {
            root: PathBuf::from("src"),
            pattern: Some("rs".to_string()),
            ..FileSearchRequest::default()
        })
        .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "search.files");
    let partial_payload = RemoteResponse::FileSearch(FileSearchResponse {
        root: PathBuf::from("src"),
        files: vec![PathBuf::from("lib.rs")],
        truncated: false,
    })
    .to_v5_payload()
    .unwrap();
    let final_payload = RemoteResponse::FileSearch(FileSearchResponse {
        root: PathBuf::from("src"),
        files: vec![PathBuf::from("main.rs")],
        truncated: true,
    })
    .to_v5_payload()
    .unwrap();
    let frames = [
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::response(
                stream_id,
                "search.files",
                protocol_v5::MessageRole::PartialResult,
                false,
            ),
        ),
        protocol_v5::stream_data_frame(
            stream_id,
            partial_payload.clone(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::SearchPayload),
        )
        .unwrap(),
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::progress(
                stream_id,
                "search.files",
                protocol_v5::Progress {
                    message: "file search matches".to_string(),
                    completed: 1,
                    total: 2,
                },
            ),
        ),
        protocol_v5::stream_data_frame(
            stream_id,
            final_payload.clone(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap(),
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::response(
                stream_id,
                "search.files",
                protocol_v5::MessageRole::FinalResponse,
                true,
            ),
        ),
        protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, stream_id),
    ];
    input.push(v5_frames_bytes(frames[..3].to_vec()));

    let started = Instant::now();
    while client.shared.response_budget.used() != partial_payload.len() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for buffered partial search results"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let connection_credit = || {
        read_v5_complete_frames(output.bytes())
            .into_iter()
            .filter(|frame| {
                frame.stream_id == 0 && frame.frame_type == protocol_v5::FrameType::WindowUpdate
            })
            .map(|frame| {
                frame
                    .decode_control::<protocol_v5::WindowUpdate>()
                    .unwrap()
                    .credit_bytes
            })
            .sum::<u64>()
    };
    let before_poll = connection_credit();

    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteFileSearchEvent::Batch(vec![PathBuf::from("lib.rs")])
    );
    assert_eq!(client.shared.response_budget.used(), 0);
    assert_eq!(connection_credit(), before_poll);

    input.push(v5_frames_bytes(frames[3..].to_vec()));
    let started = Instant::now();
    while client.shared.response_budget.used() != final_payload.len()
        || !client.shared.search_waiters.lock().unwrap().is_empty()
    {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for buffered final search results"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    while connection_credit() < before_poll + partial_payload.len() as u64 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for partial-search consumption credit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let before_final_poll = connection_credit();

    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteFileSearchEvent::Batch(vec![PathBuf::from("main.rs")])
    );
    assert_eq!(client.shared.response_budget.used(), 0);
    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteFileSearchEvent::Complete {
            root: PathBuf::from("src"),
            truncated: true,
        }
    );
    assert!(futures::executor::block_on(stream.next()).is_none());
    let started = Instant::now();
    while connection_credit() < before_final_poll + final_payload.len() as u64 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for final-search consumption credit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        connection_credit() - before_final_poll,
        final_payload.len() as u64
    );
    input.close();
}

#[test]
fn v5_text_search_stream_decodes_partial_matches_before_completion() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let mut stream = client
        .text_search_stream(TextSearchRequest {
            root: PathBuf::from("src"),
            pattern: "needle".to_string(),
            ..TextSearchRequest::default()
        })
        .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "search.text");
    let expected = TextSearchMatchResponse {
        relative_path: PathBuf::from("lib.rs"),
        line_number: 7,
        line_text: "a needle here".to_string(),
        start: 2,
        end: 8,
    };
    let partial_payload = RemoteResponse::TextSearch(TextSearchResponse {
        root: PathBuf::from("src"),
        matches: vec![expected.clone()],
        truncated: false,
    })
    .to_v5_payload()
    .unwrap();
    let mut frames = vec![
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::response(
                stream_id,
                "search.text",
                protocol_v5::MessageRole::PartialResult,
                false,
            ),
        ),
        protocol_v5::stream_data_frame(
            stream_id,
            partial_payload,
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::SearchPayload),
        )
        .unwrap(),
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            stream_id,
            &protocol_v5::StreamEnvelope::progress(
                stream_id,
                "search.text",
                protocol_v5::Progress {
                    message: "text search matches".to_string(),
                    completed: 1,
                    total: 1,
                },
            ),
        ),
    ];
    frames.extend(v5_response_frames(
        stream_id,
        "search.text",
        RemoteResponse::TextSearch(TextSearchResponse {
            root: PathBuf::from("src"),
            matches: Vec::new(),
            truncated: false,
        }),
        Vec::new(),
    ));
    input.push(v5_frames_bytes(frames));

    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteTextSearchEvent::Batch(vec![expected])
    );
    assert_eq!(
        futures::executor::block_on(stream.next()).unwrap().unwrap(),
        RemoteTextSearchEvent::Complete {
            root: PathBuf::from("src"),
            truncated: false,
        }
    );
    assert!(futures::executor::block_on(stream.next()).is_none());
    input.close();
}

#[test]
fn dropping_completed_search_stream_releases_sealed_receive_debt() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let stream = client
        .file_search_stream(FileSearchRequest::default())
        .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "search.files");
    let response = RemoteResponse::FileSearch(FileSearchResponse {
        root: PathBuf::new(),
        files: vec![PathBuf::from("src/lib.rs")],
        truncated: false,
    });
    let payload_len = response.to_v5_payload().unwrap().len();
    input.push(v5_frames_bytes(v5_response_frames(
        stream_id,
        "search.files",
        response,
        Vec::new(),
    )));
    let started = Instant::now();
    while !client
        .shared
        .completed_search_streams
        .lock()
        .unwrap()
        .contains_key(&stream_id)
    {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for completed buffered search"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(client.shared.response_budget.used(), payload_len);

    drop(stream);

    let started = Instant::now();
    loop {
        let released = client.shared.response_budget.used() == 0
            && client
                .shared
                .completed_search_streams
                .lock()
                .unwrap()
                .is_empty();
        let credited = read_v5_complete_frames(output.bytes())
            .into_iter()
            .filter(|frame| {
                frame.stream_id == 0 && frame.frame_type == protocol_v5::FrameType::WindowUpdate
            })
            .map(|frame| {
                frame
                    .decode_control::<protocol_v5::WindowUpdate>()
                    .unwrap()
                    .credit_bytes
            })
            .sum::<u64>()
            >= payload_len as u64;
        if released && credited {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out releasing completed search debt"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!client.shared.closed.load(Ordering::Acquire));
    input.close();
}

#[test]
fn dropping_completed_process_stream_releases_sealed_receive_debt() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let stream = client
        .run_process_stream(
            ProcessRequest {
                program: "command".to_string(),
                args: Vec::new(),
                cwd: PathBuf::from("."),
                env: BTreeMap::new(),
                clear_env: false,
                inherit_project_environment: false,
                max_output_bytes: None,
                timeout_ms: None,
            },
            Vec::new(),
        )
        .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "process.run");
    let response = RemoteResponse::RunProcess(ProcessOutputResponse {
        status_code: Some(0),
        success: true,
        stdout_truncated: false,
        stderr_truncated: false,
        stdout_len: 3,
        stderr_len: 0,
        timed_out: false,
    });
    let payload_len = response.to_v5_payload().unwrap().len();
    let retained = payload_len + 3;
    input.push(v5_frames_bytes(v5_response_frames(
        stream_id,
        "process.run",
        response,
        b"out".to_vec(),
    )));
    let started = Instant::now();
    while !client
        .shared
        .completed_process_streams
        .lock()
        .unwrap()
        .contains_key(&stream_id)
    {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for completed buffered process"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(client.shared.response_budget.used(), retained);

    drop(stream);

    let started = Instant::now();
    loop {
        let released = client.shared.response_budget.used() == 0
            && client
                .shared
                .completed_process_streams
                .lock()
                .unwrap()
                .is_empty();
        let credited = read_v5_complete_frames(output.bytes())
            .into_iter()
            .filter(|frame| {
                frame.stream_id == 0 && frame.frame_type == protocol_v5::FrameType::WindowUpdate
            })
            .map(|frame| {
                frame
                    .decode_control::<protocol_v5::WindowUpdate>()
                    .unwrap()
                    .credit_bytes
            })
            .sum::<u64>()
            >= retained as u64;
        if released && credited {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out releasing completed process debt"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!client.shared.closed.load(Ordering::Acquire));
    input.close();
}

#[test]
fn dropping_v5_file_stream_resets_once_and_keeps_connection_usable() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let stream = client
        .read_file_stream(PathBuf::from("large.bin"), None)
        .unwrap();
    let cancelled_stream = wait_for_v5_request_stream(&output, "fs.read");
    wait_for_v5_stream_frame(&output, cancelled_stream, protocol_v5::FrameType::EndStream);
    input.push(v5_frames_bytes(vec![
        protocol_v5::stream_data_frame(
            cancelled_stream,
            vec![5; 64],
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
        )
        .unwrap(),
    ]));
    let started = Instant::now();
    while client.shared.response_budget.used() != 64 {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for queued file bytes"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(stream);

    wait_for_v5_stream_frame(
        &output,
        cancelled_stream,
        protocol_v5::FrameType::ResetStream,
    );
    let started = Instant::now();
    while client.shared.response_budget.used() != 0
        || !client.shared.file_waiters.lock().unwrap().is_empty()
    {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for cancelled file stream cleanup"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let resets = read_v5_complete_frames(output.bytes())
        .into_iter()
        .filter(|frame| {
            frame.stream_id == cancelled_stream
                && frame.frame_type == protocol_v5::FrameType::ResetStream
        })
        .count();
    assert_eq!(resets, 1);
    assert!(!client.shared.closed.load(Ordering::Acquire));

    let healthy_client = Arc::clone(&client);
    let healthy = std::thread::spawn(move || {
        healthy_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("healthy.rs"),
            },
            Vec::new(),
        )
    });
    let healthy_stream = wait_for_v5_request_stream_after(&output, "fs.stat", cancelled_stream);
    input.push(v5_frames_bytes(v5_response_frames(
        healthy_stream,
        "fs.stat",
        RemoteResponse::Stat(FileStatResponse {
            path: PathBuf::from("healthy.rs"),
            kind: RemoteFileKind::File,
            size: 1,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        }),
        Vec::new(),
    )));
    assert!(matches!(
        healthy.join().unwrap().unwrap().0,
        RemoteResponse::Stat(_)
    ));
    input.close();
}

#[test]
fn v5_multiplexed_client_applies_directory_delta_to_cached_listing() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );
    let initial_listing = v5_test_directory_listing(
        "src",
        10,
        20,
        vec![
            v5_test_directory_entry("src/lib.rs", 12),
            v5_test_directory_entry("src/old.rs", 4),
        ],
    );
    let updated_lib = v5_test_directory_entry("src/lib.rs", 42);
    let added_mod = v5_test_directory_entry("src/mod.rs", 8);

    let first_client = Arc::clone(&client);
    let first_thread = std::thread::spawn(move || {
        first_client.request(
            RemoteRequest::ListDir {
                path: PathBuf::from("src"),
            },
            Vec::new(),
        )
    });
    let first_stream = wait_for_v5_request_stream(&output, "fs.list_dir");
    input.push(v5_frames_bytes(v5_response_frames(
        first_stream,
        "fs.list_dir",
        RemoteResponse::ListDir(initial_listing),
        Vec::new(),
    )));
    first_thread.join().unwrap().unwrap();

    let second_client = Arc::clone(&client);
    let second_thread = std::thread::spawn(move || {
        second_client.request(
            RemoteRequest::ListDir {
                path: PathBuf::from("src"),
            },
            Vec::new(),
        )
    });
    let second_stream = wait_for_v5_request_stream_after(&output, "fs.list_dir", first_stream);
    let second_payload: V5DirectoryListPayload =
        decode_v5_request_payload(&output, second_stream).unwrap();
    assert_eq!(second_payload.known_generation, Some(10));
    assert_eq!(second_payload.known_fingerprint, Some(20));
    input.push(v5_frames_bytes(v5_response_frames(
        second_stream,
        "fs.list_dir",
        RemoteResponse::ListDir(DirectoryListingResponse {
            path: PathBuf::from("src"),
            generation: Some(11),
            fingerprint: Some(21),
            complete: true,
            not_modified: false,
            delta: Some(DirectoryListingDeltaResponse {
                base_generation: Some(10),
                base_fingerprint: Some(20),
                added: vec![added_mod.clone()],
                updated: vec![updated_lib.clone()],
                removed: vec![PathBuf::from("src/old.rs")],
            }),
            entries: Vec::new(),
        }),
        Vec::new(),
    )));
    let (second_response, _) = second_thread.join().unwrap().unwrap();
    let RemoteResponse::ListDir(second_listing) = second_response else {
        panic!("expected delta-expanded list_dir response");
    };
    assert_eq!(second_listing.generation, Some(11));
    assert_eq!(
        second_listing.entries,
        vec![updated_lib.clone(), added_mod.clone()]
    );
    assert!(second_listing.delta.is_none());

    let third_client = Arc::clone(&client);
    let third_thread = std::thread::spawn(move || {
        third_client.request(
            RemoteRequest::ListDir {
                path: PathBuf::from("src"),
            },
            Vec::new(),
        )
    });
    let third_stream = wait_for_v5_request_stream_after(&output, "fs.list_dir", second_stream);
    let third_payload: V5DirectoryListPayload =
        decode_v5_request_payload(&output, third_stream).unwrap();
    assert_eq!(third_payload.known_generation, Some(11));
    assert_eq!(third_payload.known_fingerprint, Some(21));
    input.push(v5_frames_bytes(v5_response_frames(
        third_stream,
        "fs.list_dir",
        RemoteResponse::ListDir(DirectoryListingResponse {
            path: PathBuf::from("src"),
            generation: Some(11),
            fingerprint: Some(21),
            complete: true,
            not_modified: true,
            delta: None,
            entries: Vec::new(),
        }),
        Vec::new(),
    )));
    third_thread.join().unwrap().unwrap();
    input.close();
}

#[test]
fn v5_multiplexed_client_sends_second_request_before_first_completes() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = Arc::new(
        RemoteWorkspaceV5MultiplexedClient::connect(
            protocol_v5::FramedIo::new(input.clone(), output.clone()),
            protocol_v5::ClientHello::nucleotide("test-client"),
        )
        .unwrap(),
    );

    let (completion_tx, completion_rx) = mpsc::channel();
    let read_client = Arc::clone(&client);
    let read_tx = completion_tx.clone();
    let read_thread = std::thread::spawn(move || {
        let result = read_client.request(
            RemoteRequest::ReadFile {
                path: PathBuf::from("slow.txt"),
                max_bytes: None,
            },
            Vec::new(),
        );
        read_tx.send(("read", result)).unwrap();
    });
    let read_stream = wait_for_v5_request_stream(&output, "fs.read");

    let stat_client = Arc::clone(&client);
    let stat_tx = completion_tx.clone();
    let stat_thread = std::thread::spawn(move || {
        let result = stat_client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("fast.txt"),
            },
            Vec::new(),
        );
        stat_tx.send(("stat", result)).unwrap();
    });
    let stat_stream = wait_for_v5_request_stream(&output, "fs.stat");

    assert_ne!(read_stream, stat_stream);
    input.push(v5_frames_bytes(v5_response_frames(
        stat_stream,
        "fs.stat",
        RemoteResponse::Stat(FileStatResponse {
            path: PathBuf::from("fast.txt"),
            kind: RemoteFileKind::File,
            size: 4,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
        }),
        Vec::new(),
    )));
    let first = completion_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first completion");
    assert_eq!(first.0, "stat");
    assert!(matches!(first.1.unwrap().0, RemoteResponse::Stat(_)));

    input.push(v5_frames_bytes(v5_response_frames(
        read_stream,
        "fs.read",
        RemoteResponse::ReadFile(FileReadResponse {
            path: PathBuf::from("slow.txt"),
            size: 4,
            modified_unix_millis: None,
            modified_unix_nanos: None,
            readonly: false,
            truncated: false,
        }),
        b"slow".to_vec(),
    )));
    let second = completion_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second completion");
    assert_eq!(second.0, "read");
    let (response, body) = second.1.unwrap();
    assert!(matches!(response, RemoteResponse::ReadFile(_)));
    assert_eq!(body, b"slow");

    input.close();
    stat_thread.join().unwrap();
    read_thread.join().unwrap();
}
