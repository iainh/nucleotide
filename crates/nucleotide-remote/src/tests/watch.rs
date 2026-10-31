// Service-side watch registration, delivery, expiry, and overflow tests.

#[test]
fn v5_service_watch_reset_releases_subscription_and_preserves_end_half_close() {
    v5_watch_reset_lifecycle(false);
}

#[test]
fn v5_concurrent_service_watch_reset_releases_subscription_and_preserves_end_half_close() {
    v5_watch_reset_lifecycle(true);
}

fn v5_watch_reset_lifecycle(concurrent: bool) {
    let temp = tempfile::tempdir().unwrap();
    let mut frames = Vec::new();
    for cycle in 0..4 {
        let request_id = 1 + cycle * 10;
        let watch_id = cycle + 1;
        let event_stream_id = watch_id * 2;
        frames.extend(v5_protobuf_request_frames(
            request_id,
            "watch.start",
            &protocol_v5::WatchStart::expanded_dirs(["."]),
        ));
        // END closes only the unused client sending half, not the subscription.
        frames.push(protocol_v5::Frame::new(
            protocol_v5::FrameType::EndStream,
            event_stream_id,
        ));
        frames.extend(v5_protobuf_request_frames(
            request_id + 2,
            "watch.resync",
            &protocol_v5::WatchResync {
                watch_id,
                roots: Vec::new(),
            },
        ));
        frames.push(protocol_v5::reset_stream_frame(
            event_stream_id,
            protocol_v5::RESET_CANCELLED,
            "watch receiver disposed",
        ));
        frames.extend(v5_protobuf_request_frames(
            request_id + 4,
            "watch.update",
            &protocol_v5::WatchUpdate {
                watch_id,
                add_roots: Vec::new(),
                remove_roots: Vec::new(),
            },
        ));
        // A stop after reset must still acknowledge, without closing a dead stream.
        frames.extend(v5_protobuf_request_frames(
            request_id + 6,
            "watch.stop",
            &protocol_v5::WatchStop { watch_id },
        ));
    }
    // Normal stop remains usable on the same connection after repeated resets.
    frames.extend(v5_protobuf_request_frames(
        41,
        "watch.start",
        &protocol_v5::WatchStart::expanded_dirs(["."]),
    ));
    frames.extend(v5_protobuf_request_frames(
        43,
        "watch.stop",
        &protocol_v5::WatchStop { watch_id: 5 },
    ));
    let input = BlockingRead::default();
    input.push(v5_client_input(Vec::new()));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let info = protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string());
    let service_input = input.clone();
    let service_output = output.clone();
    let worker = std::thread::spawn(move || {
        let mut io = protocol_v5::FramedIo::new(service_input, service_output);
        if concurrent {
            service.serve_v5_concurrent(io, &info)
        } else {
            service.serve_v5(&mut io, &info)
        }
    });
    let mut pending = Vec::new();
    for frame in frames {
        if frame.frame_type == protocol_v5::FrameType::ResetStream {
            input.push(encode_v5_sequenced_frames(std::mem::take(&mut pending)));
            // Observe the END control's batch before RESET can invalidate queued output.
            let started = Instant::now();
            while find_v5_watch_batch(&output, frame.stream_id).is_none()
                && !worker.is_finished()
                && started.elapsed() < Duration::from_secs(5)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        pending.push(frame);
    }
    input.push(encode_v5_sequenced_frames(pending));
    let started = Instant::now();
    loop {
        let done = read_v5_complete_frames(output.bytes()).iter().any(|frame| {
            frame.stream_id == 43 && frame.frame_type == protocol_v5::FrameType::EndStream
        });
        if done || worker.is_finished() || started.elapsed() > Duration::from_secs(5) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    input.close();
    worker
        .join()
        .unwrap()
        .expect("watch reset must not terminate the service");
    let frames = read_v5_frames(output.bytes());
    for cycle in 0..4 {
        let request_id = 1 + cycle * 10;
        let event_stream_id = (cycle + 1) * 2;
        let (_, error) = decode_v5_protobuf_service_response::<protocol_v5::WatchUpdateResponse>(
            &frames,
            request_id + 4,
        );
        assert_eq!(
            error.expect("reset must remove watch ownership").code,
            "not_found"
        );
        assert!(
            find_v5_watch_batch_in_frames(&frames, event_stream_id).is_some(),
            "END must preserve the server event-sending half"
        );
        assert!(frames.iter().any(|frame| frame.stream_id == request_id + 6
            && frame.frame_type == protocol_v5::FrameType::EndStream));
        assert!(
            !frames.iter().any(|frame| frame.stream_id == event_stream_id
                && frame.frame_type == protocol_v5::FrameType::EndStream)
        );
    }
    for stream_id in [10, 43] {
        assert!(
            frames.iter().any(|frame| frame.stream_id == stream_id
                && frame.frame_type == protocol_v5::FrameType::EndStream),
            "normal stop must finish the event stream and acknowledge on the retained connection"
        );
    }
}

#[test]
fn v5_service_watch_start_returns_degraded_poll_event_stream() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("src")).unwrap();
    let start = protocol_v5::WatchStart::expanded_dirs([".", "src", "../outside"]);
    let input = v5_client_input(v5_protobuf_request_frames(1, "watch.start", &start));
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

    service
        .serve_v5(
            &mut io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, output) = io.into_inner();
    let frames = read_v5_frames(output);
    let (response, error) =
        decode_v5_protobuf_service_response::<protocol_v5::WatchStartResponse>(&frames, 1);

    assert!(error.is_none());
    let response = response.expect("expected watch.start response");
    assert_eq!(response.watch_id, 1);
    assert_ne!(response.event_stream_id, 0);
    assert_eq!(response.event_stream_id % 2, 0);
    assert_eq!(response.backend, "poll");
    assert!(response.degraded);
    assert!(response.requires_reconciliation);
    assert_eq!(response.accepted_roots, [".", "src"]);
    assert_eq!(response.unsupported_roots, ["../outside"]);

    let event_headers = frames
        .iter()
        .find(|frame| {
            frame.stream_id == response.event_stream_id
                && frame.frame_type == protocol_v5::FrameType::Headers
        })
        .expect("expected watch event stream headers");
    let envelope = event_headers
        .decode_control::<protocol_v5::StreamEnvelope>()
        .unwrap();
    assert_eq!(envelope.role, protocol_v5::MessageRole::Event as i32);
    assert_eq!(envelope.method, "watch.batch");
}
#[test]
fn v5_service_rejects_expired_watch_start_before_registering_it() {
    let temp = tempfile::tempdir().unwrap();
    let start = protocol_v5::WatchStart::expanded_dirs(["."]);
    let options = protocol_v5::RequestOptions {
        deadline_unix_ms: v5_now_unix_millis().saturating_sub(1),
        ..protocol_v5::RequestOptions::default()
    };
    let input = v5_client_input(v5_protobuf_request_frames_with_options(
        1,
        "watch.start",
        &start,
        options,
    ));
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

    service
        .serve_v5(
            &mut io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, output) = io.into_inner();
    let frames = read_v5_frames(output);
    let reset = frames
        .iter()
        .find(|frame| {
            frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
        })
        .expect("expired watch.start should be reset")
        .decode_control::<protocol_v5::ResetStream>()
        .unwrap();

    assert_eq!(reset.code, protocol_v5::RESET_DEADLINE_EXCEEDED);
    assert!(!frames.iter().any(|frame| {
        frame.stream_id != 0 && frame.frame_type == protocol_v5::FrameType::Headers
    }));
}

#[test]
fn v5_service_watch_update_and_stop_manage_event_stream() {
    let temp = tempfile::tempdir().unwrap();
    let start = protocol_v5::WatchStart::expanded_dirs(["."]);
    let update = protocol_v5::WatchUpdate {
        watch_id: 1,
        add_roots: vec!["crates".to_string(), "../outside".to_string()],
        remove_roots: vec![".".to_string()],
    };
    let stop = protocol_v5::WatchStop { watch_id: 1 };
    let mut request_frames = v5_protobuf_request_frames(1, "watch.start", &start);
    request_frames.extend(v5_protobuf_request_frames(3, "watch.update", &update));
    request_frames.extend(v5_protobuf_request_frames(5, "watch.stop", &stop));
    let input = v5_client_input(request_frames);
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

    service
        .serve_v5(
            &mut io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, output) = io.into_inner();
    let frames = read_v5_frames(output);
    let (start_response, start_error) =
        decode_v5_protobuf_service_response::<protocol_v5::WatchStartResponse>(&frames, 1);
    let (update_response, update_error) =
        decode_v5_protobuf_service_response::<protocol_v5::WatchUpdateResponse>(&frames, 3);

    assert!(start_error.is_none());
    assert!(update_error.is_none());
    let event_stream_id = start_response.unwrap().event_stream_id;
    let update_response = update_response.expect("expected watch.update response");
    assert_eq!(update_response.watch_id, 1);
    assert_eq!(update_response.accepted_roots, ["crates"]);
    assert_eq!(update_response.unsupported_roots, ["../outside"]);
    assert!(frames.iter().any(|frame| {
        frame.stream_id == event_stream_id && frame.frame_type == protocol_v5::FrameType::EndStream
    }));
    assert!(frames.iter().any(|frame| {
        frame.stream_id == 5 && frame.frame_type == protocol_v5::FrameType::EndStream
    }));
}

#[test]
fn v5_service_watch_resync_emits_resync_batch() {
    let temp = tempfile::tempdir().unwrap();
    let start = protocol_v5::WatchStart::expanded_dirs(["."]);
    let resync = protocol_v5::WatchResync {
        watch_id: 1,
        roots: vec![
            ".".to_string(),
            "missing".to_string(),
            "../outside".to_string(),
        ],
    };
    let mut request_frames = v5_protobuf_request_frames(1, "watch.start", &start);
    request_frames.extend(v5_protobuf_request_frames(3, "watch.resync", &resync));
    let input = v5_client_input(request_frames);
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());

    service
        .serve_v5(
            &mut io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, output) = io.into_inner();
    let frames = read_v5_frames(output);
    let (start_response, start_error) =
        decode_v5_protobuf_service_response::<protocol_v5::WatchStartResponse>(&frames, 1);
    let (resync_response, resync_error) =
        decode_v5_protobuf_service_response::<protocol_v5::WatchResyncResponse>(&frames, 3);

    assert!(start_error.is_none());
    assert!(resync_error.is_none());
    let event_stream_id = start_response.unwrap().event_stream_id;
    let response = resync_response.expect("expected watch.resync response");
    assert_eq!(response.watch_id, 1);
    assert_eq!(response.accepted_roots, ["."]);
    assert_eq!(response.unsupported_roots, ["../outside", "missing"]);

    let batch = find_v5_watch_batch_in_frames(&frames, event_stream_id)
        .expect("expected watch.resync to emit a resync batch");
    assert_eq!(batch.watch_id, 1);
    assert_eq!(batch.sequence, 1);
    assert!(batch.resync_required);
    assert!(!batch.overflow);
    assert_eq!(batch.directory_generations[0].path, ".");
}

#[test]
fn v5_watch_registry_stop_and_reset_prune_sequences_but_retain_directory_generations() {
    let temp = tempfile::tempdir().unwrap();
    let mut watches = V5WatchRegistry::default();
    watches.start(2, 4, vec![".".to_string()], 50, 500, temp.path());
    for cycle in 1..=8 {
        // Reusing an ID here exposes stale sequence metadata that monotonic IDs conceal.
        watches.start(1, 2, vec![".".to_string()], 50, 500, temp.path());
        watches.resync(1, None).unwrap();
        let batches = watches.poll_due(temp.path()).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].1.sequence, 1);
        assert_eq!(batches[0].1.directory_generations[0].generation, cycle);
        if cycle % 2 == 0 {
            watches.stop(1).unwrap();
        } else {
            watches.stop_event_stream(2);
        }
        watches.stop_event_stream(2);
        assert!(!watches.subscriptions.contains_key(&1));
        assert!(watches.subscriptions.contains_key(&2));
    }
    watches.stop(2).unwrap();
    assert!(!watches.has_active_watches());
}

#[test]
fn v5_watch_registry_polling_emits_batches_for_changed_roots() {
    let temp = tempfile::tempdir().unwrap();
    let mut watches = V5WatchRegistry::default();
    let watch_id = watches.allocate_watch_id().unwrap();
    let status = watches.start(watch_id, 2, vec![".".to_string()], 50, 500, temp.path());
    assert_eq!(status.backend, "poll");
    assert!(status.degraded);

    std::thread::sleep(Duration::from_millis(60));
    assert!(watches.poll_due(temp.path()).unwrap().is_empty());

    std::fs::write(temp.path().join("new.txt"), b"changed").unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let batches = watches.poll_due(temp.path()).unwrap();

    assert_eq!(batches.len(), 1);
    let (event_stream_id, batch) = &batches[0];
    assert_eq!(*event_stream_id, 2);
    assert_eq!(batch.watch_id, watch_id);
    assert_eq!(batch.sequence, 1);
    assert_eq!(batch.directory_generations[0].path, ".");
    assert_eq!(batch.directory_generations[0].generation, 1);
    assert_eq!(
        batch.events[0].kind,
        protocol_v5::WatchChangeKind::Modified as i32
    );
    assert_eq!(batch.events[0].path, ".");
    assert!(batch.events[0].is_dir);
}

#[test]
fn v5_watch_registry_native_events_emit_batches_for_nearest_root() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("src")).unwrap();
    let (events_tx, _events_rx) = mpsc::channel();
    let (native_tx, _native_rx) = mpsc::sync_channel(V5_NATIVE_WATCH_EVENT_CAPACITY);
    let mut watches =
        V5WatchRegistry::with_native_events(V5NativeWatchSender::new(native_tx, events_tx));
    let watch_id = watches.allocate_watch_id().unwrap();
    watches.start(
        watch_id,
        2,
        vec![".".to_string(), "src".to_string()],
        50,
        500,
        temp.path(),
    );

    let event = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
        .add_path(temp.path().join("src/lib.rs"));
    watches
        .record_native_event(watch_id, Ok(event), temp.path())
        .unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let batches = watches.poll_due(temp.path()).unwrap();

    assert_eq!(batches.len(), 1);
    let (event_stream_id, batch) = &batches[0];
    assert_eq!(*event_stream_id, 2);
    assert_eq!(batch.watch_id, watch_id);
    assert_eq!(batch.sequence, 1);
    assert_eq!(batch.directory_generations[0].path, "src");
    assert_eq!(batch.events[0].path, "src/lib.rs");
    assert_eq!(
        batch.events[0].kind,
        protocol_v5::WatchChangeKind::Created as i32
    );
    assert!(!batch.events[0].is_dir);
}

#[test]
fn v5_watch_registry_collapses_event_overflow_to_resync() {
    let temp = tempfile::tempdir().unwrap();
    let mut watches = V5WatchRegistry::default();
    let watch_id = watches.allocate_watch_id().unwrap();
    watches.start(watch_id, 2, vec![".".to_string()], 50, 2, temp.path());

    let event = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
        .add_path(temp.path().join("one.txt"))
        .add_path(temp.path().join("two.txt"))
        .add_path(temp.path().join("three.txt"));
    watches
        .record_native_event(watch_id, Ok(event), temp.path())
        .unwrap();
    std::thread::sleep(Duration::from_millis(60));

    let batches = watches.poll_due(temp.path()).unwrap();
    assert_eq!(batches.len(), 1);
    let (_, batch) = &batches[0];
    assert!(batch.overflow);
    assert!(batch.resync_required);
    assert!(batch.events.is_empty());
    assert!(batch.directory_generations.is_empty());
}

#[test]
fn v5_watch_event_limit_defaults_and_has_a_hard_cap() {
    assert_eq!(v5_watch_event_limit(0), V5_DEFAULT_WATCH_EVENTS_PER_BATCH);
    assert_eq!(v5_watch_event_limit(7), 7);
    assert_eq!(
        v5_watch_event_limit(u32::MAX),
        V5_MAX_WATCH_EVENTS_PER_BATCH
    );
}

#[test]
fn v5_concurrent_service_emits_watch_batch_on_open_connection() {
    let temp = tempfile::tempdir().unwrap();
    let start = protocol_v5::WatchStart {
        debounce_ms: 50,
        ..protocol_v5::WatchStart::expanded_dirs(["missing"])
    };
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_protobuf_request_frames(
        1,
        "watch.start",
        &start,
    )));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let info = protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string());
    let service_input = input.clone();
    let service_output = output.clone();
    let service_thread = std::thread::spawn(move || {
        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(service_input, service_output),
                &info,
            )
            .unwrap();
    });

    let started = Instant::now();
    let watch = loop {
        if let Some(response) = find_v5_watch_start_response(&output, 1) {
            break response;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for watch.start response"
        );
        std::thread::sleep(Duration::from_millis(10));
    };

    std::fs::create_dir(temp.path().join("missing")).unwrap();
    let started = Instant::now();
    let batch = loop {
        if let Some(batch) = find_v5_watch_batch(&output, watch.event_stream_id) {
            break batch;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for watch.batch"
        );
        std::thread::sleep(Duration::from_millis(10));
    };

    input.close();
    service_thread.join().unwrap();

    assert_eq!(batch.watch_id, watch.watch_id);
    assert_eq!(batch.sequence, 1);
    assert_eq!(batch.directory_generations[0].path, "missing");
    assert_eq!(batch.events[0].path, "missing");
    assert_eq!(
        batch.events[0].kind,
        protocol_v5::WatchChangeKind::Modified as i32
    );
}

#[test]
fn v5_concurrent_service_rejects_expired_watch_start_before_registering_it() {
    let temp = tempfile::tempdir().unwrap();
    let start = protocol_v5::WatchStart::expanded_dirs(["."]);
    let options = protocol_v5::RequestOptions {
        deadline_unix_ms: v5_now_unix_millis().saturating_sub(1),
        ..protocol_v5::RequestOptions::default()
    };
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_protobuf_request_frames_with_options(
        1,
        "watch.start",
        &start,
        options,
    )));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );

    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::ResetStream);
    input.close();
    service_thread.join().unwrap();
    let frames = read_v5_frames(output.bytes());
    let reset = frames
        .iter()
        .find(|frame| {
            frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
        })
        .expect("expired watch.start should be reset")
        .decode_control::<protocol_v5::ResetStream>()
        .unwrap();

    assert_eq!(reset.code, protocol_v5::RESET_DEADLINE_EXCEEDED);
    assert!(!frames.iter().any(|frame| {
        frame.stream_id != 0 && frame.frame_type == protocol_v5::FrameType::Headers
    }));
}
