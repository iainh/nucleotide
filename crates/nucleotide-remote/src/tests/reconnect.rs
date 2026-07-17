// Reconnecting request, stream, watch, and backend lifecycle tests.

#[test]
fn reconnecting_client_retries_read_only_request_after_disconnect() {
    let calls = Arc::new(StdMutex::new(Vec::new()));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = FakeProtocolClient::new(calls.clone(), [FakeProtocolOutcome::Disconnected]);
    let reconnect_calls = calls.clone();
    let reconnect_count = reconnects.clone();
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(FakeProtocolClient::new(
            reconnect_calls.clone(),
            [FakeProtocolOutcome::Ok(RemoteResponse::FindAncestorFile(
                None,
            ))],
        ))
    });
    let request = RemoteRequest::Stat {
        path: PathBuf::from("src/lib.rs"),
    };

    let (response, body) = client.request(request.clone(), Vec::new()).unwrap();

    assert_eq!(response, RemoteResponse::FindAncestorFile(None));
    assert!(body.is_empty());
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[request.clone(), request]
    );
}

#[test]
fn reconnecting_client_does_not_reconnect_local_stream_capacity_error() {
    let calls = Arc::new(StdMutex::new(Vec::new()));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = FakeProtocolClient::new(
        Arc::clone(&calls),
        [FakeProtocolOutcome::Io(io::ErrorKind::OutOfMemory)],
    );
    let reconnect_count = Arc::clone(&reconnects);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(FakeProtocolClient::new(
            Arc::new(StdMutex::new(Vec::new())),
            [FakeProtocolOutcome::Ok(RemoteResponse::Stat(
                FileStatResponse {
                    path: PathBuf::from("src/lib.rs"),
                    kind: RemoteFileKind::File,
                    size: 0,
                    version: None,
                    modified_unix_millis: None,
                    modified_unix_nanos: None,
                    readonly: false,
                },
            ))],
        ))
    });
    let request = RemoteRequest::Stat {
        path: PathBuf::from("src/lib.rs"),
    };

    let error = client.request(request.clone(), Vec::new()).unwrap_err();

    assert!(matches!(
        error,
        RemoteClientError::Io(ref error) if error.kind() == io::ErrorKind::OutOfMemory
    ));
    assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    assert_eq!(calls.lock().unwrap().as_slice(), &[request]);
}

#[test]
fn local_capacity_errors_are_not_classified_as_transport_failures() {
    let error = RemoteClientError::Io(io::Error::new(
        io::ErrorKind::OutOfMemory,
        "v5 max concurrent streams exceeded",
    ));

    assert!(!remote_client_error_allows_reconnect_retry(&error));
    assert!(!remote_client_error_requires_reconnect(&error));
    assert!(!remote_watch_restore_error_is_retryable(&error));
}

// Reconnection, cancellation, replay, and watch restoration tests.

#[test]
fn reconnecting_client_does_not_replay_cancelled_request() {
    let calls = Arc::new(AtomicUsize::new(0));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let reconnect_count = Arc::clone(&reconnects);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(
        CancelThenDisconnectProtocolClient {
            calls: Arc::clone(&calls),
        },
        move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(CancelThenDisconnectProtocolClient {
                calls: Arc::new(AtomicUsize::new(0)),
            })
        },
    );
    let cancellation = RemoteRequestCancellation::new();

    let error = client
        .request_with_context_and_cancellation(
            RemoteRequest::Stat {
                path: PathBuf::from("cancelled.rs"),
            },
            Vec::new(),
            RemoteRequest::Stat {
                path: PathBuf::from("cancelled.rs"),
            }
            .v5_request_context(),
            &cancellation,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RemoteClientError::Remote(RemoteError { ref code, .. })
            if code == protocol_v5::RESET_CANCELLED
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(reconnects.load(Ordering::SeqCst), 0);
}

#[test]
fn reconnecting_client_does_not_replay_when_cancelled_during_recovery() {
    let calls = Arc::new(StdMutex::new(Vec::new()));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let cancellation = RemoteRequestCancellation::new();
    let reconnect_cancellation = cancellation.clone();
    let reconnect_calls = Arc::clone(&calls);
    let reconnect_count = Arc::clone(&reconnects);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(
        FakeProtocolClient::new(Arc::clone(&calls), [FakeProtocolOutcome::Disconnected]),
        move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            reconnect_cancellation.cancel();
            Ok(FakeProtocolClient::new(
                Arc::clone(&reconnect_calls),
                [FakeProtocolOutcome::Ok(RemoteResponse::Stat(
                    FileStatResponse {
                        path: PathBuf::from("cancelled.rs"),
                        kind: RemoteFileKind::File,
                        size: 0,
                        version: None,
                        modified_unix_millis: None,
                        modified_unix_nanos: None,
                        readonly: false,
                    },
                ))],
            ))
        },
    );
    let request = RemoteRequest::Stat {
        path: PathBuf::from("cancelled.rs"),
    };

    let error = client
        .request_with_context_and_cancellation(
            request.clone(),
            Vec::new(),
            request.v5_request_context(),
            &cancellation,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RemoteClientError::Remote(RemoteError { ref code, .. })
            if code == protocol_v5::RESET_CANCELLED
    ));
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(calls.lock().unwrap().as_slice(), &[request]);
}

#[test]
fn reconnect_factory_observes_request_context_and_cancellation() {
    let calls = Arc::new(StdMutex::new(Vec::new()));
    let initial = FakeProtocolClient::new(Arc::clone(&calls), [FakeProtocolOutcome::Disconnected]);
    let (started_sender, started_receiver) = mpsc::sync_channel(1);
    let client = Arc::new(ReconnectingRemoteWorkspaceProtocolClient::new_with_attempt(
        initial,
        move |attempt| {
            let attempt = attempt.expect("request reconnect should carry its attempt context");
            started_sender
                .send((attempt.method, attempt.context))
                .unwrap();
            while !attempt.cancellation.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(remote_request_cancelled_error(attempt.method))
        },
    ));
    let request = RemoteRequest::Stat {
        path: PathBuf::from("cancel-reconnect.rs"),
    };
    let context = request.v5_request_context();
    let cancellation = RemoteRequestCancellation::new();
    let request_client = Arc::clone(&client);
    let request_cancellation = cancellation.clone();
    let request_thread = std::thread::spawn(move || {
        request_client.request_with_context_and_cancellation(
            request,
            Vec::new(),
            context,
            &request_cancellation,
        )
    });

    let (method, factory_context) = started_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("reconnect factory did not start");
    assert_eq!(method, "fs.stat");
    assert_eq!(factory_context, context);
    cancellation.cancel();

    assert!(matches!(
        request_thread.join().unwrap(),
        Err(RemoteClientError::Remote(RemoteError { code, .. }))
            if code == protocol_v5::RESET_CANCELLED
    ));
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn reconnecting_client_reuses_exact_context_for_safe_replay() {
    let contexts = Arc::new(StdMutex::new(Vec::new()));
    let closes = Arc::new(AtomicUsize::new(0));
    let initial = ContextRecordingProtocolClient::new(
        Arc::clone(&contexts),
        [ContextProtocolOutcome::Disconnected],
        Arc::clone(&closes),
    );
    let reconnect_contexts = Arc::clone(&contexts);
    let reconnect_closes = Arc::clone(&closes);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        Ok(ContextRecordingProtocolClient::new(
            Arc::clone(&reconnect_contexts),
            [ContextProtocolOutcome::Ok(
                RemoteResponse::FindAncestorFile(None),
            )],
            Arc::clone(&reconnect_closes),
        ))
    });

    let response = client
        .request(
            RemoteRequest::Stat {
                path: PathBuf::from("src/lib.rs"),
            },
            Vec::new(),
        )
        .unwrap();

    assert_eq!(
        response,
        (RemoteResponse::FindAncestorFile(None), Vec::new())
    );
    let contexts = contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0], contexts[1]);
    assert_ne!(contexts[0].deadline_unix_ms, 0);
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}

#[test]
fn reconnecting_client_does_not_replay_after_original_deadline_expires() {
    let contexts = Arc::new(StdMutex::new(Vec::new()));
    let closes = Arc::new(AtomicUsize::new(0));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = ContextRecordingProtocolClient::new(
        Arc::clone(&contexts),
        [ContextProtocolOutcome::Disconnected],
        Arc::clone(&closes),
    );
    let reconnect_contexts = Arc::clone(&contexts);
    let reconnect_closes = Arc::clone(&closes);
    let reconnect_count = Arc::clone(&reconnects);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        Ok(ContextRecordingProtocolClient::new(
            Arc::clone(&reconnect_contexts),
            [ContextProtocolOutcome::Ok(
                RemoteResponse::FindAncestorFile(None),
            )],
            Arc::clone(&reconnect_closes),
        ))
    });
    let context = RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::absolute_only(
        Duration::from_millis(20),
    ));

    let error = client
        .request_with_context(
            RemoteRequest::Stat {
                path: PathBuf::from("src/lib.rs"),
            },
            Vec::new(),
            context,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RemoteClientError::RequestDeadlineExceeded {
            ref method,
            kind: RemoteRequestDeadlineKind::Absolute,
        } if method == "fs.stat"
    ));
    assert_eq!(contexts.lock().unwrap().as_slice(), &[context]);
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(
        closes.load(Ordering::SeqCst),
        2,
        "both the stale and post-deadline replacement transports should close"
    );
}

#[test]
fn reconnecting_client_does_not_reconnect_stream_local_deadline() {
    let contexts = Arc::new(StdMutex::new(Vec::new()));
    let closes = Arc::new(AtomicUsize::new(0));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = ContextRecordingProtocolClient::new(
        Arc::clone(&contexts),
        [ContextProtocolOutcome::Deadline],
        Arc::clone(&closes),
    );
    let reconnect_count = Arc::clone(&reconnects);
    let reconnect_contexts = Arc::clone(&contexts);
    let reconnect_closes = Arc::clone(&closes);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(ContextRecordingProtocolClient::new(
            Arc::clone(&reconnect_contexts),
            [ContextProtocolOutcome::Ok(
                RemoteResponse::FindAncestorFile(None),
            )],
            Arc::clone(&reconnect_closes),
        ))
    });

    let error = client
        .request(
            RemoteRequest::Stat {
                path: PathBuf::from("src/lib.rs"),
            },
            Vec::new(),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RemoteClientError::RequestDeadlineExceeded {
            kind: RemoteRequestDeadlineKind::Inactivity,
            ..
        }
    ));
    assert_eq!(contexts.lock().unwrap().len(), 1);
    assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    assert_eq!(closes.load(Ordering::SeqCst), 0);
}

#[test]
fn reconnecting_client_invalidates_failed_replay_without_third_attempt() {
    let contexts = Arc::new(StdMutex::new(Vec::new()));
    let closes = Arc::new(AtomicUsize::new(0));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = ContextRecordingProtocolClient::new(
        Arc::clone(&contexts),
        [ContextProtocolOutcome::Disconnected],
        Arc::clone(&closes),
    );
    let reconnect_count = Arc::clone(&reconnects);
    let reconnect_contexts = Arc::clone(&contexts);
    let reconnect_closes = Arc::clone(&closes);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(ContextRecordingProtocolClient::new(
            Arc::clone(&reconnect_contexts),
            [ContextProtocolOutcome::Disconnected],
            Arc::clone(&reconnect_closes),
        ))
    });

    let error = client
        .request(
            RemoteRequest::Stat {
                path: PathBuf::from("src/lib.rs"),
            },
            Vec::new(),
        )
        .unwrap_err();

    assert!(matches!(error, RemoteClientError::Disconnected));
    let contexts = contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0], contexts[1]);
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(closes.load(Ordering::SeqCst), 2);
}

#[test]
fn reconnecting_client_heals_but_does_not_retry_mutation_after_disconnect() {
    let calls = Arc::new(StdMutex::new(Vec::new()));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = FakeProtocolClient::new(calls.clone(), [FakeProtocolOutcome::Disconnected]);
    let reconnect_calls = calls.clone();
    let reconnect_count = reconnects.clone();
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(FakeProtocolClient::new(
            reconnect_calls.clone(),
            [FakeProtocolOutcome::Ok(RemoteResponse::FindAncestorFile(
                None,
            ))],
        ))
    });
    let request = RemoteRequest::WriteFile {
        path: PathBuf::from("src/lib.rs"),
        create_parent_dirs: false,
        expected_version: None,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };

    let error = client
        .request(request.clone(), b"body".to_vec())
        .unwrap_err();
    let next_request = RemoteRequest::Stat {
        path: PathBuf::from("src/lib.rs"),
    };
    let (response, _) = client.request(next_request.clone(), Vec::new()).unwrap();

    let RemoteClientError::OutcomeUnknown { method, .. } = error else {
        panic!("expected unknown mutation outcome");
    };
    assert_eq!(method, "fs.write");
    assert_eq!(response, RemoteResponse::FindAncestorFile(None));
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(calls.lock().unwrap().as_slice(), &[request, next_request]);
}

#[test]
fn reconnecting_client_does_not_retry_remote_final_error() {
    let calls = Arc::new(StdMutex::new(Vec::new()));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = FakeProtocolClient::new(
        calls.clone(),
        [FakeProtocolOutcome::RemoteError("PERMISSION_DENIED")],
    );
    let reconnect_count = reconnects.clone();
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(FakeProtocolClient::new(
            Arc::new(StdMutex::new(Vec::new())),
            [FakeProtocolOutcome::Ok(RemoteResponse::FindAncestorFile(
                None,
            ))],
        ))
    });
    let request = RemoteRequest::Stat {
        path: PathBuf::from("src/lib.rs"),
    };

    let error = client.request(request.clone(), Vec::new()).unwrap_err();

    assert!(matches!(error, RemoteClientError::Remote(_)));
    assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    assert_eq!(calls.lock().unwrap().as_slice(), &[request]);
}

#[test]
fn reconnecting_client_replays_safe_reads_after_any_terminal_io_kind() {
    for kind in [
        io::ErrorKind::TimedOut,
        io::ErrorKind::ConnectionRefused,
        io::ErrorKind::NotConnected,
        io::ErrorKind::WriteZero,
        io::ErrorKind::Other,
    ] {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = FakeProtocolClient::new(Arc::clone(&calls), [FakeProtocolOutcome::Io(kind)]);
        let reconnect_calls = Arc::clone(&calls);
        let reconnect_count = Arc::clone(&reconnects);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(FakeProtocolClient::new(
                Arc::clone(&reconnect_calls),
                [FakeProtocolOutcome::Ok(RemoteResponse::FindAncestorFile(
                    None,
                ))],
            ))
        });
        let request = RemoteRequest::Stat {
            path: PathBuf::from("src/lib.rs"),
        };

        let (response, _) = client.request(request.clone(), Vec::new()).unwrap();

        assert_eq!(response, RemoteResponse::FindAncestorFile(None), "{kind:?}");
        assert_eq!(reconnects.load(Ordering::SeqCst), 1, "{kind:?}");
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[request.clone(), request],
            "{kind:?}"
        );
    }
}

#[test]
fn reconnecting_client_retries_watch_start_after_transport_healing() {
    let starts = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = WatchProtocolClient {
        starts: Arc::clone(&starts),
        closes: Arc::clone(&closes),
        fail_start: true,
    };
    let reconnect_starts = Arc::clone(&starts);
    let reconnect_closes = Arc::clone(&closes);
    let reconnect_count = Arc::clone(&reconnects);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        Ok(WatchProtocolClient {
            starts: Arc::clone(&reconnect_starts),
            closes: Arc::clone(&reconnect_closes),
            fail_start: false,
        })
    });

    let watch = client
        .start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from("src")]))
        .unwrap();

    assert!(watch.is_none());
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(closes.load(Ordering::SeqCst), 1);
}

#[test]
fn reconnecting_client_does_not_replay_watch_start_after_original_deadline_expires() {
    let starts = Arc::new(AtomicUsize::new(0));
    let closes = Arc::new(AtomicUsize::new(0));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let initial = WatchProtocolClient {
        starts: Arc::clone(&starts),
        closes: Arc::clone(&closes),
        fail_start: true,
    };
    let reconnect_starts = Arc::clone(&starts);
    let reconnect_closes = Arc::clone(&closes);
    let reconnect_count = Arc::clone(&reconnects);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
        reconnect_count.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        Ok(WatchProtocolClient {
            starts: Arc::clone(&reconnect_starts),
            closes: Arc::clone(&reconnect_closes),
            fail_start: false,
        })
    });
    let context = RemoteRequestContext::from_policy(RemoteRequestDeadlinePolicy::absolute_only(
        Duration::from_millis(20),
    ));

    let error = match client.start_watch_with_context(
        WorkspaceWatchRequest::expanded_dirs([PathBuf::from("src")]),
        context,
    ) {
        Ok(_) => panic!("watch.start replay should not outlive its original deadline"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        RemoteClientError::RequestDeadlineExceeded {
            ref method,
            kind: RemoteRequestDeadlineKind::Absolute,
        } if method == "watch.start"
    ));
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(
        closes.load(Ordering::SeqCst),
        2,
        "both the stale and post-deadline replacement transports should close"
    );
}

#[test]
fn reconnecting_v5_watch_restores_desired_roots_behind_resync_barrier() {
    let initial_input = BlockingRead::default();
    let initial_output = SharedWrite::default();
    initial_input.push(v5_server_input(Vec::new()));
    let initial = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(initial_input.clone(), initial_output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();

    let replacement_input = BlockingRead::default();
    let replacement_output = SharedWrite::default();
    replacement_input.push(v5_server_input(Vec::new()));
    let replacement = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(replacement_input.clone(), replacement_output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let replacement = Arc::new(StdMutex::new(Some(replacement)));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let reconnect_replacement = Arc::clone(&replacement);
    let reconnect_count = Arc::clone(&reconnects);
    let client = Arc::new(ReconnectingRemoteWorkspaceProtocolClient::new(
        initial,
        move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(reconnect_replacement
                .lock()
                .unwrap()
                .take()
                .expect("replacement v5 client should only be consumed once"))
        },
    ));

    let start_client = Arc::clone(&client);
    let start = std::thread::spawn(move || {
        start_client.start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from("src")]))
    });
    let initial_start_stream = wait_for_v5_request_stream(&initial_output, "watch.start");
    let initial_start: protocol_v5::WatchStart =
        decode_v5_protobuf_request(&initial_output, initial_start_stream).unwrap();
    assert_eq!(initial_start.roots, vec!["src"]);
    let initial_response = protocol_v5::WatchStartResponse {
        watch_id: 11,
        event_stream_id: 2,
        backend: "poll".to_string(),
        recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
        degraded: true,
        requires_reconciliation: true,
        accepted_roots: vec!["src".to_string()],
        degraded_roots: Vec::new(),
        unsupported_roots: Vec::new(),
    };
    let mut initial_frames = vec![v5_watch_event_open_frame(2, 11)];
    initial_frames.extend(v5_raw_response_frames(
        initial_start_stream,
        "watch.start",
        initial_response.encode_to_vec(),
    ));
    initial_input.push(v5_frames_bytes(initial_frames));
    let watch = start.join().unwrap().unwrap().unwrap();
    assert_ne!(watch.watch_id, 11, "physical watch id leaked to caller");

    initial_input.push(v5_frames_bytes(vec![
        protocol_v5::watch_batch_frame(
            2,
            protocol_v5::WatchBatch {
                watch_id: 11,
                sequence: 9,
                directory_generations: Vec::new(),
                events: vec![protocol_v5::WatchChange::modified("src", true)],
                overflow: false,
                resync_required: false,
            },
        )
        .unwrap(),
    ]));
    let initial_batch = watch.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(initial_batch.watch_id, watch.watch_id);
    assert_eq!(initial_batch.sequence, 1);

    let logical_watch_id = watch.watch_id;
    let update_client = Arc::clone(&client);
    let update = std::thread::spawn(move || {
        update_client.update_watch(
            logical_watch_id,
            vec![PathBuf::from("tests")],
            vec![PathBuf::from("src")],
        )
    });
    let update_stream =
        wait_for_v5_request_stream_after(&initial_output, "watch.update", initial_start_stream);
    let update_request: protocol_v5::WatchUpdate =
        decode_v5_protobuf_request(&initial_output, update_stream).unwrap();
    assert_eq!(update_request.watch_id, 11);
    initial_input.push(v5_frames_bytes(v5_raw_response_frames(
        update_stream,
        "watch.update",
        protocol_v5::WatchUpdateResponse {
            watch_id: 11,
            accepted_roots: vec!["tests".to_string()],
            degraded_roots: Vec::new(),
            unsupported_roots: Vec::new(),
        }
        .encode_to_vec(),
    )));
    let update = update.join().unwrap().unwrap().unwrap();
    assert_eq!(update.watch_id, watch.watch_id);

    initial_input.close();
    let replacement_start_stream = wait_for_v5_request_stream(&replacement_output, "watch.start");
    let replacement_start: protocol_v5::WatchStart =
        decode_v5_protobuf_request(&replacement_output, replacement_start_stream).unwrap();
    assert_eq!(replacement_start.roots, vec!["tests"]);
    let replacement_response = protocol_v5::WatchStartResponse {
        watch_id: 41,
        event_stream_id: 4,
        backend: "poll".to_string(),
        recursive_coverage: protocol_v5::RecursiveCoverage::None as i32,
        degraded: true,
        requires_reconciliation: true,
        accepted_roots: vec!["tests".to_string()],
        degraded_roots: Vec::new(),
        unsupported_roots: Vec::new(),
    };
    let mut replacement_frames = vec![v5_watch_event_open_frame(4, 41)];
    replacement_frames.extend(v5_raw_response_frames(
        replacement_start_stream,
        "watch.start",
        replacement_response.encode_to_vec(),
    ));
    replacement_input.push(v5_frames_bytes(replacement_frames));

    let resync_stream = wait_for_v5_request_stream_after(
        &replacement_output,
        "watch.resync",
        replacement_start_stream,
    );
    let resync: protocol_v5::WatchResync =
        decode_v5_protobuf_request(&replacement_output, resync_stream).unwrap();
    assert_eq!(resync.watch_id, 41);
    assert_eq!(resync.roots, vec!["tests"]);
    let pre_barrier = protocol_v5::WatchBatch {
        watch_id: 41,
        sequence: 1,
        directory_generations: Vec::new(),
        events: vec![protocol_v5::WatchChange::modified("pre-barrier", false)],
        overflow: false,
        resync_required: false,
    };
    let barrier = protocol_v5::WatchBatch {
        watch_id: 41,
        sequence: 2,
        directory_generations: Vec::new(),
        events: Vec::new(),
        overflow: false,
        resync_required: true,
    };
    let after_barrier = protocol_v5::WatchBatch {
        watch_id: 41,
        sequence: 3,
        directory_generations: Vec::new(),
        events: vec![protocol_v5::WatchChange::modified("after-barrier", false)],
        overflow: false,
        resync_required: false,
    };
    let mut resync_frames = v5_raw_response_frames(
        resync_stream,
        "watch.resync",
        protocol_v5::WatchResyncResponse {
            watch_id: 41,
            accepted_roots: vec!["tests".to_string()],
            unsupported_roots: Vec::new(),
        }
        .encode_to_vec(),
    );
    resync_frames.push(protocol_v5::watch_batch_frame(4, pre_barrier).unwrap());
    resync_frames.push(protocol_v5::watch_batch_frame(4, barrier).unwrap());
    resync_frames.push(protocol_v5::watch_batch_frame(4, after_barrier).unwrap());
    replacement_input.push(v5_frames_bytes(resync_frames));

    let resync_batch = watch.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(resync_batch.watch_id, watch.watch_id);
    assert_eq!(resync_batch.sequence, 2);
    assert!(resync_batch.resync_required);
    let next_batch = watch.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(next_batch.watch_id, watch.watch_id);
    assert_eq!(next_batch.sequence, 3);
    assert_eq!(
        next_batch.events[0].path,
        PathBuf::from("/workspace/after-barrier")
    );

    let stop_client = Arc::clone(&client);
    let stop_watch_id = watch.watch_id;
    let stop = std::thread::spawn(move || stop_client.stop_watch(stop_watch_id));
    let stop_stream =
        wait_for_v5_request_stream_after(&replacement_output, "watch.stop", resync_stream);
    let stop_request: protocol_v5::WatchStop =
        decode_v5_protobuf_request(&replacement_output, stop_stream).unwrap();
    assert_eq!(stop_request.watch_id, 41);
    replacement_input.push(v5_frames_bytes(v5_raw_response_frames(
        stop_stream,
        "watch.stop",
        Vec::new(),
    )));
    stop.join().unwrap().unwrap();

    replacement_input.close();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(reconnects.load(Ordering::SeqCst), 1);
    client.close();
}

#[test]
fn reconnecting_client_heals_watch_control_failures_without_replaying_them() {
    for failed_operation in [WatchControlFailure::Update, WatchControlFailure::Stop] {
        let updates = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let reconnects = Arc::new(AtomicUsize::new(0));
        let initial = WatchControlProtocolClient {
            updates: Arc::clone(&updates),
            stops: Arc::clone(&stops),
            closes: Arc::clone(&closes),
            failed_operation: Some(failed_operation),
        };
        let reconnect_updates = Arc::clone(&updates);
        let reconnect_stops = Arc::clone(&stops);
        let reconnect_closes = Arc::clone(&closes);
        let reconnect_count = Arc::clone(&reconnects);
        let client = ReconnectingRemoteWorkspaceProtocolClient::new(initial, move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(WatchControlProtocolClient {
                updates: Arc::clone(&reconnect_updates),
                stops: Arc::clone(&reconnect_stops),
                closes: Arc::clone(&reconnect_closes),
                failed_operation: None,
            })
        });

        let error = match failed_operation {
            WatchControlFailure::Update => client
                .update_watch(7, vec![PathBuf::from("src")], Vec::new())
                .unwrap_err(),
            WatchControlFailure::Stop => client.stop_watch(7).unwrap_err(),
        };
        assert!(matches!(error, RemoteClientError::Io(_)));
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert_eq!(closes.load(Ordering::SeqCst), 1);

        // The failed connection-scoped mutation is not replayed, while the healed transport
        // is immediately available to the next watch control operation.
        client
            .update_watch(8, vec![PathBuf::from("tests")], Vec::new())
            .unwrap();
        client.stop_watch(8).unwrap();
        assert_eq!(
            updates.load(Ordering::SeqCst),
            usize::from(failed_operation == WatchControlFailure::Update) + 1
        );
        assert_eq!(
            stops.load(Ordering::SeqCst),
            usize::from(failed_operation == WatchControlFailure::Stop) + 1
        );
    }
}

#[test]
fn backend_drop_and_reconnecting_close_are_nonblocking_and_do_not_reconnect() {
    let backend_closes = Arc::new(AtomicUsize::new(0));
    let backend_shutdowns = Arc::new(AtomicUsize::new(0));
    let backend = RemoteWorkspaceBackendImpl::from_protocol_client(
        loopback_identity(),
        LifecycleProtocolClient {
            closes: Arc::clone(&backend_closes),
            shutdowns: Arc::clone(&backend_shutdowns),
        },
    );

    let started = Instant::now();
    drop(backend);

    assert!(started.elapsed() < Duration::from_millis(250));
    assert_eq!(backend_closes.load(Ordering::SeqCst), 1);
    assert_eq!(backend_shutdowns.load(Ordering::SeqCst), 0);

    let reconnect_closes = Arc::new(AtomicUsize::new(0));
    let reconnect_shutdowns = Arc::new(AtomicUsize::new(0));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let reconnect_count = Arc::clone(&reconnects);
    let client = ReconnectingRemoteWorkspaceProtocolClient::new(
        LifecycleProtocolClient {
            closes: Arc::clone(&reconnect_closes),
            shutdowns: Arc::clone(&reconnect_shutdowns),
        },
        move || {
            reconnect_count.fetch_add(1, Ordering::SeqCst);
            Ok(LifecycleProtocolClient {
                closes: Arc::new(AtomicUsize::new(0)),
                shutdowns: Arc::new(AtomicUsize::new(0)),
            })
        },
    );

    client.close();

    assert_eq!(reconnects.load(Ordering::SeqCst), 0);
    assert_eq!(reconnect_closes.load(Ordering::SeqCst), 1);
    assert!(matches!(
        client.request(
            RemoteRequest::Stat {
                path: PathBuf::from("src/lib.rs")
            },
            Vec::new()
        ),
        Err(RemoteClientError::Disconnected)
    ));
    assert_eq!(reconnects.load(Ordering::SeqCst), 0);
}

#[test]
fn dropping_backend_request_future_cancels_and_releases_worker() {
    let state = Arc::new((
        StdMutex::new(CancellationObservingState::default()),
        Condvar::new(),
    ));
    let backend = RemoteWorkspaceBackendImpl::from_protocol_client(
        loopback_identity(),
        CancellationObservingProtocolClient {
            state: Arc::clone(&state),
        },
    );
    let mut request = Box::pin(backend.stat(Path::new("pending.rs")));
    let waker = futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);

    assert!(
        std::future::Future::poll(request.as_mut(), &mut context).is_pending(),
        "the fake protocol request should block until cancellation"
    );
    wait_for_cancellation_observer(&state, |state| state.started, "worker start");

    drop(request);

    wait_for_cancellation_observer(&state, |state| state.finished, "worker cancellation");
    let state = state.0.lock().unwrap();
    assert!(state.cancelled);
}

#[test]
fn explicit_workspace_cancellation_wakes_live_remote_request() {
    let input = BlockingRead::default();
    let output = SharedWrite::default();
    input.push(v5_server_input(Vec::new()));
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(input.clone(), output.clone()),
        protocol_v5::ClientHello::nucleotide("test-client"),
    )
    .unwrap();
    let shared = Arc::clone(&client.shared);
    let backend = Arc::new(RemoteWorkspaceBackendImpl::new(loopback_identity(), client));
    let cancellation = WorkspaceCancellationToken::new();
    let request_backend = Arc::clone(&backend);
    let request_cancellation = cancellation.clone();
    let (result_sender, result_receiver) = mpsc::channel();
    let request = std::thread::spawn(move || {
        let result = futures::executor::block_on(
            request_backend.stat_with_cancellation(Path::new("pending.rs"), &request_cancellation),
        );
        result_sender.send(result).unwrap();
    });
    let stream_id = wait_for_v5_request_stream(&output, "fs.stat");

    cancellation.cancel();

    let error = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("explicit workspace cancellation did not wake the request")
        .unwrap_err();
    assert!(matches!(error, WorkspaceError::Cancelled { .. }));
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::ResetStream);
    assert!(shared.waiters.lock().unwrap().is_empty());

    request.join().unwrap();
    input.close();
    drop(backend);
}

#[test]
fn explicit_workspace_cancellation_stays_attached_to_search_stream() {
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
    let cancellation = WorkspaceCancellationToken::new();
    let root = PathBuf::from("src");
    let mut stream = futures::executor::block_on(backend.file_search_stream_with_cancellation(
        FileSearchQuery {
            root: root.clone(),
            ..FileSearchQuery::default()
        },
        &cancellation,
    ))
    .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "search.files");

    cancellation.cancel();

    let error = futures::executor::block_on(stream.next())
        .expect("cancelled search stream should return an error")
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::Cancelled {
            operation: "file search",
            path,
        } if path == root
    ));
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::ResetStream);
    assert!(shared.search_waiters.lock().unwrap().is_empty());
    assert!(shared.completed_search_streams.lock().unwrap().is_empty());
    assert!(!shared.closed.load(Ordering::Acquire));
    input.close();
}

#[test]
fn explicit_workspace_cancellation_stays_attached_to_process_stream() {
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
    let cancellation = WorkspaceCancellationToken::new();
    let cwd = PathBuf::from(".");
    let mut stream = futures::executor::block_on(backend.run_process_stream_with_cancellation(
        ProcessSpec {
            program: "command".to_string(),
            args: Vec::new(),
            cwd: cwd.clone(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: None,
            timeout_ms: None,
        },
        &cancellation,
    ))
    .unwrap();
    let stream_id = wait_for_v5_request_stream(&output, "process.run");

    cancellation.cancel();

    let error = futures::executor::block_on(stream.next())
        .expect("cancelled process stream should return an error")
        .unwrap_err();
    assert!(matches!(
        error,
        WorkspaceError::Cancelled {
            operation: "run process",
            path,
        } if path == cwd
    ));
    wait_for_v5_stream_frame(&output, stream_id, protocol_v5::FrameType::ResetStream);
    assert!(shared.process_waiters.lock().unwrap().is_empty());
    assert!(shared.completed_process_streams.lock().unwrap().is_empty());
    assert!(!shared.closed.load(Ordering::Acquire));
    input.close();
}

#[test]
fn dropping_backend_watch_start_closes_ambiguous_control_connection() {
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
    let mut request =
        Box::pin(backend.start_watch(WorkspaceWatchRequest::expanded_dirs([PathBuf::from("src")])));
    let waker = futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);

    assert!(std::future::Future::poll(request.as_mut(), &mut context).is_pending());
    let stream_id = wait_for_v5_request_stream(&output, "watch.start");
    assert!(shared.raw_waiters.lock().unwrap().contains_key(&stream_id));

    drop(request);

    let started = Instant::now();
    loop {
        let closed = shared.closed.load(Ordering::Acquire);
        let cleaned = shared.raw_waiters.lock().unwrap().is_empty()
            && shared.pending_cancellations.lock().unwrap().is_empty()
            && shared.request_budget.used() == 0
            && Arc::strong_count(&backend.client) == 1;
        if closed && cleaned {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for dropped watch.start cleanup"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(backend);
    input.close();
}
