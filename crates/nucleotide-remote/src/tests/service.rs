// Service dispatch, scheduling, streaming operation, and shutdown tests.

#[test]
fn v5_service_reads_file_through_protocol_session() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("hello.txt"), b"hello from v5").unwrap();
    let request = RemoteRequest::ReadFile {
        path: PathBuf::from("hello.txt"),
        max_bytes: None,
    };
    let input = v5_client_input(v5_request_frames(1, &request, &[]));
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

    assert_eq!(frames[0].frame_type, protocol_v5::FrameType::Hello);
    assert_eq!(frames[1].frame_type, protocol_v5::FrameType::Settings);
    let (response, body, error) = decode_v5_service_response(&frames, 1);
    assert!(error.is_none());
    let Some(RemoteResponse::ReadFile(read)) = response else {
        panic!("expected read_file response");
    };
    assert_eq!(read.path, temp.path().join("hello.txt"));
    assert_eq!(read.version.as_deref().map(|version| version.len()), Some(32));
    assert_eq!(body, b"hello from v5");
    assert!(frames.iter().any(|frame| {
        frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::EndStream
    }));
}

// Service dispatch, watch, streaming worker, and shutdown tests.

#[test]
fn v5_fragmented_duplex_retries_interruptions_end_to_end() {
    let temp = tempfile::tempdir().unwrap();
    let expected = (0_u8..=255).cycle().take(8 * 1024).collect::<Vec<_>>();
    std::fs::write(temp.path().join("fragmented.bin"), &expected).unwrap();

    let (client_endpoint, server_endpoint, controls) = fragmenting_duplex_pair();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let info = protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string());
    let server_thread = std::thread::spawn(move || {
        service.serve_v5_concurrent(
            protocol_v5::FramedIo::new(server_endpoint.0, server_endpoint.1),
            &info,
        )
    });

    let mut hello = protocol_v5::ClientHello::nucleotide("fragmenting-test-client");
    let mut settings = protocol_v5::ConnectionSettings::recommended();
    settings.max_frame_body = 64;
    hello.desired_settings = Some(settings);
    let client = RemoteWorkspaceV5MultiplexedClient::connect(
        protocol_v5::FramedIo::new(client_endpoint.0, client_endpoint.1),
        hello,
    )
    .unwrap();

    let read = client
        .start_request(
            RemoteRequest::ReadFile {
                path: PathBuf::from("fragmented.bin"),
                max_bytes: None,
            },
            Vec::new(),
        )
        .unwrap();
    let stat = client
        .start_request(
            RemoteRequest::Stat {
                path: PathBuf::from("fragmented.bin"),
            },
            Vec::new(),
        )
        .unwrap();

    let (stat_response, stat_body) = stat.wait().unwrap();
    let RemoteResponse::Stat(stat) = stat_response else {
        panic!("expected fragmented duplex stat response");
    };
    assert_eq!(stat.size, expected.len() as u64);
    assert!(stat_body.is_empty());

    let (read_response, read_body) = read.wait().unwrap();
    let RemoteResponse::ReadFile(read) = read_response else {
        panic!("expected fragmented duplex read response");
    };
    assert_eq!(read.size, expected.len() as u64);
    assert_eq!(read_body, expected);

    client.shutdown().unwrap();
    server_thread.join().unwrap().unwrap();
    for control in controls {
        control.close();
    }
    client.close();
}
#[test]
fn v5_service_shutdown_sends_goaway_after_final_response() {
    let temp = tempfile::tempdir().unwrap();
    let request = RemoteRequest::Shutdown;
    let input = v5_client_input(v5_request_frames(1, &request, &[]));
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
    let (response, body, error) = decode_v5_service_response(&frames, 1);
    assert!(error.is_none());
    assert_eq!(response, Some(RemoteResponse::Shutdown));
    assert!(body.is_empty());

    let final_response_index = v5_final_response_index(&frames, 1);
    let goaway_index = frames
        .iter()
        .position(|frame| frame.frame_type == protocol_v5::FrameType::GoAway)
        .expect("shutdown should emit GOAWAY");
    assert!(goaway_index > final_response_index);
    let goaway = frames[goaway_index]
        .decode_control::<protocol_v5::GoAway>()
        .unwrap();
    assert_eq!(goaway.last_accepted_stream_id, 1);
    assert_eq!(goaway.code, "OK");
    assert_eq!(
        goaway.drain_grace_ms,
        protocol_v5::DEFAULT_SHUTDOWN_GRACE_MS
    );
}

#[test]
fn v5_service_list_dir_returns_not_modified_for_known_generation() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("lib.rs"), b"pub fn lib() {}\n").unwrap();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

    let first_request = RemoteRequest::ListDir {
        path: PathBuf::from("."),
    };
    let first_input = v5_client_input(v5_request_frames(1, &first_request, &[]));
    let mut first_io = protocol_v5::FramedIo::new(Cursor::new(first_input), Vec::new());
    service
        .serve_v5(
            &mut first_io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, first_output) = first_io.into_inner();
    let first_frames = read_v5_frames(first_output);
    let (first_response, _, first_error) = decode_v5_service_response(&first_frames, 1);
    assert!(first_error.is_none());
    let Some(RemoteResponse::ListDir(first_listing)) = first_response else {
        panic!("expected first list_dir response");
    };
    assert!(!first_listing.not_modified);
    assert_eq!(first_listing.entries.len(), 1);
    let generation = first_listing
        .generation
        .expect("list_dir should include a generation");

    let second_payload = V5DirectoryListPayload {
        path: PathBuf::from("."),
        known_generation: Some(generation),
        known_fingerprint: None,
    };
    let second_input = v5_client_input(v5_json_request_frames(1, "fs.list_dir", &second_payload));
    let mut second_io = protocol_v5::FramedIo::new(Cursor::new(second_input), Vec::new());
    service
        .serve_v5(
            &mut second_io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, second_output) = second_io.into_inner();
    let second_frames = read_v5_frames(second_output);
    let (second_response, _, second_error) = decode_v5_service_response(&second_frames, 1);
    assert!(second_error.is_none());
    let Some(RemoteResponse::ListDir(second_listing)) = second_response else {
        panic!("expected second list_dir response");
    };
    assert!(second_listing.not_modified);
    assert_eq!(second_listing.generation, Some(generation));
    assert!(second_listing.entries.is_empty());
}

#[test]
fn v5_service_list_dir_returns_delta_for_cached_known_generation() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("lib.rs"), b"pub fn lib() {}\n").unwrap();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

    let first_request = RemoteRequest::ListDir {
        path: PathBuf::from("."),
    };
    let first_input = v5_client_input(v5_request_frames(1, &first_request, &[]));
    let mut first_io = protocol_v5::FramedIo::new(Cursor::new(first_input), Vec::new());
    service
        .serve_v5(
            &mut first_io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, first_output) = first_io.into_inner();
    let first_frames = read_v5_frames(first_output);
    let (first_response, _, first_error) = decode_v5_service_response(&first_frames, 1);
    assert!(first_error.is_none());
    let Some(RemoteResponse::ListDir(first_listing)) = first_response else {
        panic!("expected first list_dir response");
    };
    let generation = first_listing
        .generation
        .expect("list_dir should include a generation");
    let fingerprint = first_listing
        .fingerprint
        .expect("list_dir should include a fingerprint");

    std::fs::write(temp.path().join("mod.rs"), b"mod child;\n").unwrap();
    let second_payload = V5DirectoryListPayload {
        path: PathBuf::from("."),
        known_generation: Some(generation),
        known_fingerprint: Some(fingerprint),
    };
    let second_input = v5_client_input(v5_json_request_frames(1, "fs.list_dir", &second_payload));
    let mut second_io = protocol_v5::FramedIo::new(Cursor::new(second_input), Vec::new());
    service
        .serve_v5(
            &mut second_io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, second_output) = second_io.into_inner();
    let second_frames = read_v5_frames(second_output);
    let (second_response, _, second_error) = decode_v5_service_response(&second_frames, 1);
    assert!(second_error.is_none());
    let Some(RemoteResponse::ListDir(second_listing)) = second_response else {
        panic!("expected second list_dir response");
    };
    assert!(!second_listing.not_modified);
    assert_ne!(second_listing.generation, Some(generation));
    assert!(second_listing.entries.is_empty());
    let delta = second_listing.delta.expect("expected directory delta");
    assert_eq!(delta.base_generation, Some(generation));
    assert_eq!(delta.base_fingerprint, Some(fingerprint));
    assert_eq!(delta.added.len(), 1);
    assert_eq!(delta.added[0].name, "mod.rs");
    assert!(delta.updated.is_empty());
    assert!(delta.removed.is_empty());
}

#[test]
fn v5_service_list_dirs_returns_delta_for_cached_known_generation() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("lib.rs"), b"pub fn lib() {}\n").unwrap();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

    let first_request = RemoteRequest::ListDirs {
        paths: vec![PathBuf::from(".")],
    };
    let first_input = v5_client_input(v5_request_frames(1, &first_request, &[]));
    let mut first_io = protocol_v5::FramedIo::new(Cursor::new(first_input), Vec::new());
    service
        .serve_v5(
            &mut first_io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, first_output) = first_io.into_inner();
    let first_frames = read_v5_frames(first_output);
    let (first_response, _, first_error) = decode_v5_service_response(&first_frames, 1);
    assert!(first_error.is_none());
    let Some(RemoteResponse::ListDirs(first_response)) = first_response else {
        panic!("expected first list_dirs response");
    };
    let first_listing = first_response.results[0]
        .listing
        .as_ref()
        .expect("first list_dirs result should include a listing");
    let generation = first_listing
        .generation
        .expect("list_dirs should include a generation");
    let fingerprint = first_listing
        .fingerprint
        .expect("list_dirs should include a fingerprint");

    std::fs::write(temp.path().join("mod.rs"), b"mod child;\n").unwrap();
    let second_payload = V5DirectoryListDirsPayload {
        paths: Vec::new(),
        entries: vec![V5DirectoryListEntryPayload {
            path: PathBuf::from("."),
            known_generation: Some(generation),
            known_fingerprint: Some(fingerprint),
        }],
    };
    let second_input = v5_client_input(v5_json_request_frames(1, "fs.list_dirs", &second_payload));
    let mut second_io = protocol_v5::FramedIo::new(Cursor::new(second_input), Vec::new());
    service
        .serve_v5(
            &mut second_io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, second_output) = second_io.into_inner();
    let second_frames = read_v5_frames(second_output);
    let (second_response, _, second_error) = decode_v5_service_response(&second_frames, 1);
    assert!(second_error.is_none());
    let Some(RemoteResponse::ListDirs(second_response)) = second_response else {
        panic!("expected second list_dirs response");
    };
    let second_listing = second_response.results[0]
        .listing
        .as_ref()
        .expect("second list_dirs result should include a listing");
    assert!(second_listing.entries.is_empty());
    let delta = second_listing
        .delta
        .as_ref()
        .expect("expected list_dirs delta");
    assert_eq!(delta.base_generation, Some(generation));
    assert_eq!(delta.base_fingerprint, Some(fingerprint));
    assert_eq!(delta.added.len(), 1);
    assert_eq!(delta.added[0].name, "mod.rs");
}

#[test]
fn v5_service_list_dir_returns_full_listing_when_delta_base_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("lib.rs"), b"pub fn lib() {}\n").unwrap();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

    let payload = V5DirectoryListPayload {
        path: PathBuf::from("."),
        known_generation: Some(u64::MAX),
        known_fingerprint: Some(u64::MAX - 1),
    };
    let input = v5_client_input(v5_json_request_frames(1, "fs.list_dir", &payload));
    let mut io = protocol_v5::FramedIo::new(Cursor::new(input), Vec::new());
    service
        .serve_v5(
            &mut io,
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();
    let (_, output) = io.into_inner();
    let frames = read_v5_frames(output);
    let (response, _, error) = decode_v5_service_response(&frames, 1);
    assert!(error.is_none());
    let Some(RemoteResponse::ListDir(listing)) = response else {
        panic!("expected list_dir response");
    };
    assert!(!listing.not_modified);
    assert!(listing.delta.is_none());
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].name, "lib.rs");
}

#[test]
fn v5_service_writes_file_body_through_protocol_session() {
    let temp = tempfile::tempdir().unwrap();
    let request = RemoteRequest::WriteFile {
        path: PathBuf::from("nested/out.txt"),
        create_parent_dirs: true,
        expected_version: None,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };
    let input = v5_client_input(v5_request_frames(1, &request, b"written over v5"));
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
    let (response, body, error) = decode_v5_service_response(&frames, 1);

    assert!(error.is_none());
    assert!(body.is_empty());
    let Some(RemoteResponse::WriteFile(write)) = response else {
        panic!("expected write_file response");
    };
    assert_eq!(write.path, temp.path().join("nested/out.txt"));
    assert_eq!(
        std::fs::read(temp.path().join("nested/out.txt")).unwrap(),
        b"written over v5"
    );
}

#[test]
fn v5_service_write_uses_opaque_version_from_payload() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("out.txt");
    std::fs::write(&target, b"old").unwrap();
    let expected_version = file_version_for_path(&target).unwrap();
    let request = RemoteRequest::WriteFile {
        path: PathBuf::from("out.txt"),
        create_parent_dirs: false,
        expected_version: Some(expected_version.as_bytes().to_vec()),
        // Deliberately wrong: the version token must take precedence.
        expected_modified_unix_millis: Some(0),
        expected_modified_unix_nanos: Some(0),
    };
    let input = v5_client_input(v5_request_frames(1, &request, b"new"));
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
    let (response, body, error) = decode_v5_service_response(&frames, 1);

    assert!(error.is_none());
    assert!(body.is_empty());
    let Some(RemoteResponse::WriteFile(write)) = response else {
        panic!("expected write_file response");
    };
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert_ne!(write.version.as_deref(), Some(expected_version.as_bytes()));
}

#[test]
fn v5_service_commits_zero_byte_write_through_streaming_path() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("out.txt");
    std::fs::write(&target, b"previous contents").unwrap();
    let request = RemoteRequest::WriteFile {
        path: PathBuf::from("out.txt"),
        create_parent_dirs: false,
        expected_version: None,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };
    let input = v5_client_input(v5_request_frames(1, &request, b""));
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
    let (response, body, error) = decode_v5_service_response(&frames, 1);

    assert!(error.is_none());
    assert!(body.is_empty());
    let Some(RemoteResponse::WriteFile(write)) = response else {
        panic!("expected write_file response");
    };
    assert_eq!(write.size, 0);
    assert!(std::fs::read(target).unwrap().is_empty());
    assert!(v5_write_temp_files(temp.path()).is_empty());
}

#[test]
fn v5_service_reports_unsupported_method_as_final_error() {
    let temp = tempfile::tempdir().unwrap();
    let headers = protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Headers,
        1,
        &protocol_v5::StreamEnvelope::request(1, "fs.unknown"),
    );
    let payload = protocol_v5::stream_data_frame(
        1,
        b"{}".to_vec(),
        protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
    )
    .unwrap();
    let input = v5_client_input(vec![
        headers,
        payload,
        protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, 1),
    ]);
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
    let (response, body, error) = decode_v5_service_response(&frames, 1);

    assert!(response.is_none());
    assert!(body.is_empty());
    let error = error.expect("expected final error");
    assert_eq!(error.code, "invalid_request");
    assert!(error.message.contains("unsupported v5 method"));
    assert!(frames.iter().any(|frame| {
        frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::EndStream
    }));
}

#[test]
fn v5_concurrent_service_streams_local_file_body_before_final_response() {
    let temp = tempfile::tempdir().unwrap();
    let body = vec![b'a'; protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize + 123];
    std::fs::write(temp.path().join("large.txt"), &body).unwrap();
    let read = RemoteRequest::ReadFile {
        path: PathBuf::from("large.txt"),
        max_bytes: None,
    };
    let mut options = read.v5_request_options();
    options.priority = protocol_v5::Priority::UserInput;
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames_with_options(
        1,
        &read,
        &[],
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
    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    let frames = read_v5_frames(output.bytes());
    let (response, read_body, error) = decode_v5_service_response(&frames, 1);
    assert!(error.is_none());
    let Some(RemoteResponse::ReadFile(read_response)) = response else {
        panic!("expected streamed read response");
    };
    assert_eq!(read_response.size, body.len() as u64);
    assert!(!read_response.truncated);
    assert_eq!(read_body, body);
    assert_v5_data_channel_priority(
        &frames,
        1,
        protocol_v5::DataChannel::FileBody,
        protocol_v5::Priority::UserInput,
    );

    let first_file_body_index = frames
        .iter()
        .position(|frame| {
            if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Data {
                return false;
            }
            let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
            protocol_v5::DataChannel::try_from(envelope.channel).unwrap()
                == protocol_v5::DataChannel::FileBody
        })
        .expect("expected streamed file body DATA frame");
    assert!(
        first_file_body_index < v5_final_response_index(&frames, 1),
        "file body DATA should be queued before final response headers"
    );
    let file_body_frames = frames
        .iter()
        .filter(|frame| {
            if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Data {
                return false;
            }
            let envelope = frame.decode_control::<protocol_v5::DataEnvelope>().unwrap();
            protocol_v5::DataChannel::try_from(envelope.channel).unwrap()
                == protocol_v5::DataChannel::FileBody
        })
        .count();
    assert!(file_body_frames >= 2);
}

#[test]
fn v5_file_read_limit_is_independent_of_frame_body_limit() {
    let above_frame_limit = MAX_FRAME_BODY_LEN + 123;

    assert_eq!(
        v5_streamed_file_read_limit(Some(above_frame_limit)),
        above_frame_limit
    );
    assert_eq!(
        v5_streamed_file_read_limit(None),
        V5_MAX_STREAMED_FILE_READ_BYTES
    );
    assert_eq!(
        v5_streamed_file_read_limit(Some(V5_MAX_STREAMED_FILE_READ_BYTES + 1)),
        V5_MAX_STREAMED_FILE_READ_BYTES
    );
}

#[test]
fn v5_concurrent_service_streams_write_body_to_temp_file() {
    let temp = tempfile::tempdir().unwrap();
    let write = RemoteRequest::WriteFile {
        path: PathBuf::from("src/main.rs"),
        create_parent_dirs: true,
        expected_version: None,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };
    let (method, payload) = write.to_v5_method_payload().unwrap();
    let frames = vec![
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            1,
            &protocol_v5::StreamEnvelope::request_with_options(
                1,
                method,
                &write.v5_request_options(),
            ),
        ),
        protocol_v5::stream_data_frame(
            1,
            payload,
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap(),
        protocol_v5::stream_data_frame(
            1,
            b"fn main".to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
        )
        .unwrap(),
        protocol_v5::stream_data_frame(
            1,
            b"() {}\n".to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
        )
        .unwrap(),
        protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, 1),
    ];
    let input = BlockingRead::default();
    input.push(v5_client_input(frames));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );
    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    let frames = read_v5_frames(output.bytes());
    let (response, body, error) = decode_v5_service_response(&frames, 1);
    assert!(error.is_none());
    assert!(body.is_empty());
    let Some(RemoteResponse::WriteFile(write_response)) = response else {
        panic!("expected write response");
    };
    assert_eq!(write_response.size, "fn main() {}\n".len() as u64);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("src").join("main.rs")).unwrap(),
        "fn main() {}\n"
    );
    assert!(v5_write_temp_files(&temp.path().join("src")).is_empty());
}

#[test]
fn v5_concurrent_service_drains_streaming_write_error_and_keeps_connection_usable() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("main.rs");
    std::fs::write(&target, b"old").unwrap();
    let write = RemoteRequest::WriteFile {
        path: PathBuf::from("main.rs"),
        create_parent_dirs: false,
        expected_version: None,
        expected_modified_unix_millis: Some(0),
        expected_modified_unix_nanos: Some(0),
    };
    let (method, payload) = write.to_v5_method_payload().unwrap();
    let mut frames = vec![
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            1,
            &protocol_v5::StreamEnvelope::request_with_options(
                1,
                method,
                &write.v5_request_options(),
            ),
        ),
        protocol_v5::stream_data_frame(
            1,
            payload,
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap(),
        protocol_v5::stream_data_frame(
            1,
            b"new ".to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
        )
        .unwrap(),
        protocol_v5::stream_data_frame(
            1,
            b"contents".to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
        )
        .unwrap(),
        protocol_v5::Frame::new(protocol_v5::FrameType::EndStream, 1),
    ];
    frames.extend(v5_request_frames(
        3,
        &RemoteRequest::Stat {
            path: PathBuf::from("main.rs"),
        },
        &[],
    ));
    let input = BlockingRead::default();
    input.push(v5_client_input(frames));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );
    wait_for_v5_stream_frame(&output, 3, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    let frames = read_v5_frames(output.bytes());
    assert!(
        !frames.iter().any(|frame| {
            frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
        }),
        "ordinary write failures should be returned as typed final errors"
    );
    let (write_response, write_body, write_error) = decode_v5_service_response(&frames, 1);
    assert!(write_response.is_none());
    assert!(write_body.is_empty());
    let write_error = write_error.expect("write should return a typed final error");
    assert_eq!(write_error.code, "modified");
    assert!(write_error.message.contains("modified externally"));
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
    assert!(v5_write_temp_files(temp.path()).is_empty());

    let (stat_response, stat_body, stat_error) = decode_v5_service_response(&frames, 3);
    assert!(stat_error.is_none());
    assert!(stat_body.is_empty());
    assert!(matches!(stat_response, Some(RemoteResponse::Stat(_))));
}

#[test]
fn v5_streaming_write_cancellation_before_commit_preserves_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("main.rs");
    std::fs::write(&target, b"old").unwrap();
    let mut write = V5StreamingWrite::create(target.clone(), false, None, None).unwrap();
    write.write_chunk(b"new contents").unwrap();
    let cancellation = WorkspaceCancellationToken::new();
    cancellation.cancel();

    let error = write.finish(Some(&cancellation)).unwrap_err();

    assert!(matches!(error, WorkspaceError::Cancelled { .. }));
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
    assert!(v5_write_temp_files(temp.path()).is_empty());
}

#[test]
fn v5_streaming_write_prefers_opaque_version_over_legacy_timestamp() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("main.rs");
    std::fs::write(&target, b"old").unwrap();
    let expected_version = file_version_for_path(&target).unwrap();

    let mut write = V5StreamingWrite::create(
        target.clone(),
        false,
        Some(expected_version.clone()),
        Some(SystemTime::UNIX_EPOCH),
    )
    .unwrap();
    write.write_chunk(b"new").unwrap();
    let result = write.finish(None).unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert_ne!(result.version.as_ref(), Some(&expected_version));
}

#[test]
fn v5_concurrent_service_cleans_streaming_write_temp_file_on_reset() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("main.rs");
    std::fs::write(&target, "old").unwrap();
    let write = RemoteRequest::WriteFile {
        path: PathBuf::from("main.rs"),
        create_parent_dirs: false,
        expected_version: None,
        expected_modified_unix_millis: None,
        expected_modified_unix_nanos: None,
    };
    let (method, payload) = write.to_v5_method_payload().unwrap();
    let frames = vec![
        protocol_v5::Frame::from_control(
            protocol_v5::FrameType::Headers,
            1,
            &protocol_v5::StreamEnvelope::request_with_options(
                1,
                method,
                &write.v5_request_options(),
            ),
        ),
        protocol_v5::stream_data_frame(
            1,
            payload,
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::Unspecified),
        )
        .unwrap(),
        protocol_v5::stream_data_frame(
            1,
            b"new".to_vec(),
            protocol_v5::DataFrameOptions::new(protocol_v5::DataChannel::FileBody),
        )
        .unwrap(),
    ];
    let input = BlockingRead::default();
    input.push(v5_client_input(frames));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let workspace_path = temp.path().to_path_buf();
    let service_input = input.clone();
    let service_output = output.clone();
    let service_thread = std::thread::spawn(move || {
        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(service_input, service_output),
                &protocol_v5::ServerHandshakeInfo::current(workspace_path.display().to_string()),
            )
            .unwrap();
    });

    let started = Instant::now();
    loop {
        if !v5_write_temp_files(temp.path()).is_empty() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for streaming write temp file"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut reset = Vec::new();
    protocol_v5::write_frame(
        &mut reset,
        &protocol_v5::reset_stream_frame(1, protocol_v5::RESET_CANCELLED, "write cancelled"),
    )
    .unwrap();
    input.push(reset);
    input.close();
    service_thread.join().unwrap();

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
    assert!(v5_write_temp_files(temp.path()).is_empty());
    let frames = read_v5_frames(output.bytes());
    assert!(
        !frames.iter().any(|frame| frame.stream_id == 1
            && matches!(
                frame.frame_type,
                protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
            )),
        "canceled write stream should not receive final headers or END_STREAM"
    );
}

#[test]
fn v5_search_partial_flushes_by_count_or_elapsed_interval() {
    let mut flush = V5SearchPartialFlush::new();

    assert!(!flush.should_flush(0));
    assert!(!flush.should_flush(1));
    assert!(flush.should_flush(V5_SEARCH_PARTIAL_BATCH_SIZE));

    flush.last_emit = Instant::now() - Duration::from_millis(V5_SEARCH_PARTIAL_INTERVAL_MS);
    assert!(flush.should_flush(1));

    flush.mark_flushed();
    assert!(!flush.should_flush(1));
}

#[test]
fn v5_streamed_read_drops_chunk_when_cancelled_during_read() {
    struct CancellingReader {
        cancellation: WorkspaceCancellationToken,
        reads: Arc<AtomicUsize>,
    }

    impl Read for CancellingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.reads.fetch_add(1, Ordering::AcqRel);
            buffer[..4].copy_from_slice(b"data");
            self.cancellation.cancel();
            Ok(4)
        }
    }

    let cancellation = WorkspaceCancellationToken::new();
    let reads = Arc::new(AtomicUsize::new(0));
    let mut emitted = Vec::new();
    let result = v5_stream_file_chunks(
        CancellingReader {
            cancellation: cancellation.clone(),
            reads: Arc::clone(&reads),
        },
        4,
        Path::new("document.txt"),
        &cancellation,
        |body| {
            emitted.push(body);
            Ok(())
        },
    );

    assert!(matches!(
        result,
        Err(RemoteError { code, .. }) if code == protocol_v5::RESET_CANCELLED
    ));
    assert_eq!(reads.load(Ordering::Acquire), 1);
    assert!(emitted.is_empty());
}

#[test]
fn v5_streamed_read_stops_after_cancelled_emission() {
    let cancellation = WorkspaceCancellationToken::new();
    let mut emitted = Vec::new();
    let body = vec![7_u8; protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as usize + 1];
    let result = v5_stream_file_chunks(
        Cursor::new(body),
        protocol_v5::DEFAULT_MAX_FRAME_BODY_LEN as u64 + 1,
        Path::new("document.txt"),
        &cancellation,
        |chunk| {
            emitted.push(chunk);
            cancellation.cancel();
            Ok(())
        },
    );

    assert!(matches!(
        result,
        Err(RemoteError { code, .. }) if code == protocol_v5::RESET_CANCELLED
    ));
    assert_eq!(emitted.len(), 1);
}

#[test]
fn v5_server_output_queue_backpressures_without_blocking_control_events() {
    let (control_tx, control_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let output_events = V5ServeOutputSender::new(output_tx, control_tx.clone());
    let output = |byte| V5ServeOutputEvent::StreamData {
        stream_id: 7,
        channel: protocol_v5::DataChannel::Stdout,
        body: vec![byte],
        priority: protocol_v5::Priority::LspSupport,
    };

    output_events.send(output(1)).unwrap();
    assert!(matches!(
        control_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        V5ServeEvent::Output
    ));
    output_events.clear_ready();

    let blocked_sender = output_events.clone();
    let (started_tx, started_rx) = mpsc::sync_channel(0);
    let (finished_tx, finished_rx) = mpsc::sync_channel(0);
    let producer = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = blocked_sender.send(output(2));
        finished_tx.send(result).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    control_tx.send(V5ServeEvent::NativeWatch).unwrap();
    assert!(matches!(
        control_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        V5ServeEvent::NativeWatch
    ));
    assert!(matches!(
        finished_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    let first = output_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        first,
        V5ServeOutputEvent::StreamData { stream_id: 7, .. }
    ));
    output_events.mark_delivered();
    finished_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    let second = output_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        second,
        V5ServeOutputEvent::StreamData { stream_id: 7, .. }
    ));
    output_events.mark_delivered();
    producer.join().unwrap();
}

#[test]
fn v5_server_output_queue_rejects_events_over_the_byte_budget() {
    let (control_tx, control_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::sync_channel(V5_SERVE_OUTPUT_EVENT_CAPACITY);
    let output_events = V5ServeOutputSender::new(output_tx, control_tx);
    let retained_bytes = V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES + 1;

    let error = output_events
        .send(V5ServeOutputEvent::StreamData {
            stream_id: 9,
            channel: protocol_v5::DataChannel::FileBody,
            body: vec![0; retained_bytes],
            priority: protocol_v5::Priority::ForegroundDocument,
        })
        .unwrap_err();

    assert_eq!(
        error,
        V5ServeQueueError::EventTooLarge {
            retained_bytes,
            max: V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES,
        }
    );
    assert!(matches!(
        output_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        control_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(!output_events.has_pending_output());
}

#[test]
fn v5_error_completion_discards_oversized_string_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let mut method = String::with_capacity(1024 * 1024);
    method.push_str("fs.list_dirs");
    let mut code = String::with_capacity(1024 * 1024);
    code.push_str("remote");
    let error = RemoteError {
        code,
        message: "m".repeat(1024 * 1024),
        diagnostic: Some("d".repeat(1024 * 1024)),
    };
    let (control_tx, _control_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let output_events = V5ServeOutputSender::new(output_tx, control_tx);
    let cancellation = WorkspaceCancellationToken::new();

    assert!(
        service
            .enqueue_v5_service_completion(
                V5ServiceCompletion {
                    stream_id: 7,
                    method,
                    result: Err(error),
                },
                protocol_v5::Priority::VisibleFileTree,
                &output_events,
                &cancellation,
            )
            .unwrap()
    );

    let event = output_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(event.retained_bytes() <= V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES);
    let V5ServeOutputEvent::Completed(completion) = event else {
        panic!("expected terminal error completion");
    };
    assert_eq!(completion.method.capacity(), completion.method.len());
    let Err(error) = completion.result else {
        panic!("expected terminal error result");
    };
    assert_eq!(error.code.capacity(), error.code.len());
    assert_eq!(error.message.len(), 16 * 1024);
    assert_eq!(error.message.capacity(), error.message.len());
    let diagnostic = error.diagnostic.unwrap();
    assert_eq!(diagnostic.len(), 32 * 1024);
    assert_eq!(diagnostic.capacity(), diagnostic.len());
    output_events.mark_delivered();
    assert!(!output_events.has_pending_output());
}

#[test]
fn v5_cancellable_output_send_unblocks_when_full() {
    let (control_tx, _control_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let output_events = V5ServeOutputSender::new(output_tx, control_tx);
    let output = |byte| V5ServeOutputEvent::StreamData {
        stream_id: 7,
        channel: protocol_v5::DataChannel::FileBody,
        body: vec![byte],
        priority: protocol_v5::Priority::ForegroundDocument,
    };
    output_events.send(output(1)).unwrap();
    let cancellation = WorkspaceCancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let worker_output = output_events.clone();
    let (started_tx, started_rx) = mpsc::sync_channel(0);
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        worker_output.send_with_cancellation(output(2), &worker_cancellation)
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let started = Instant::now();
    while output_events.pending_count.load(Ordering::Acquire) < 2 {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "cancellable sender did not reach the full queue"
        );
        std::thread::yield_now();
    }
    cancellation.cancel();

    assert_eq!(worker.join().unwrap(), Err(V5ServeQueueError::Cancelled));
    assert_eq!(output_events.pending_count.load(Ordering::Acquire), 1);
    assert!(matches!(
        output_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        V5ServeOutputEvent::StreamData { body, .. } if body == vec![1]
    ));
    output_events.mark_delivered();
    assert!(!output_events.has_pending_output());
}

#[test]
fn v5_cancelled_completion_stops_before_terminal_output() {
    let temp = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let response = RemoteResponse::FileSearch(FileSearchResponse {
        root: PathBuf::from("."),
        files: (0..4_096)
            .map(|index| PathBuf::from(format!("src/nested/module_{index:04}.rs")))
            .collect(),
        truncated: false,
    });
    let (control_tx, _control_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    let output_events = V5ServeOutputSender::new(output_tx, control_tx);
    let cancellation = WorkspaceCancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let worker_output = output_events.clone();
    let worker = std::thread::spawn(move || {
        service.enqueue_v5_service_completion(
            V5ServiceCompletion {
                stream_id: 7,
                method: "search.files".to_string(),
                result: Ok(ServiceOutcome::continue_response(response, Vec::new())),
            },
            protocol_v5::Priority::LspSupport,
            &worker_output,
            &worker_cancellation,
        )
    });

    let started = Instant::now();
    while output_events.pending_count.load(Ordering::Acquire) < 2 {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "completion did not block behind its first serialized chunk"
        );
        std::thread::yield_now();
    }
    cancellation.cancel();

    assert_eq!(worker.join().unwrap(), Ok(false));
    assert_eq!(output_events.completion_budget.used(), 0);
    assert_eq!(output_events.pending_count.load(Ordering::Acquire), 1);
    assert!(matches!(
        output_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        V5ServeOutputEvent::StreamData { stream_id: 7, .. }
    ));
    output_events.mark_delivered();
    assert!(matches!(
        output_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(!output_events.has_pending_output());
}

#[test]
fn v5_service_completion_serializes_large_payloads_in_bounded_chunks() {
    let temp = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let response = RemoteResponse::FileSearch(FileSearchResponse {
        root: PathBuf::from("."),
        files: (0..4_096)
            .map(|index| PathBuf::from(format!("src/nested/module_{index:04}.rs")))
            .collect(),
        truncated: false,
    });
    let expected_payload = response.to_v5_payload().unwrap();
    assert!(expected_payload.len() > V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES);

    let (control_tx, _control_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::sync_channel(8);
    let output_events = V5ServeOutputSender::new(output_tx, control_tx);
    let cancellation = WorkspaceCancellationToken::new();
    assert!(
        service
            .enqueue_v5_service_completion(
                V5ServiceCompletion {
                    stream_id: 7,
                    method: "search.files".to_string(),
                    result: Ok(ServiceOutcome::continue_response(response, Vec::new())),
                },
                protocol_v5::Priority::LspSupport,
                &output_events,
                &cancellation,
            )
            .unwrap()
    );

    let mut chunks = 0;
    let mut actual_payload = Vec::new();
    let mut completed = false;
    for event in output_rx.try_iter() {
        match event {
            V5ServeOutputEvent::StreamData {
                stream_id,
                channel,
                body,
                priority,
            } => {
                assert_eq!(stream_id, 7);
                assert_eq!(channel, protocol_v5::DataChannel::Unspecified);
                assert_eq!(priority, protocol_v5::Priority::LspSupport);
                assert!(body.capacity() <= V5_SERVE_OUTPUT_EVENT_MAX_RETAINED_BYTES);
                chunks += 1;
                actual_payload.extend(body);
            }
            V5ServeOutputEvent::Completed(completion) => {
                assert_eq!(completion.stream_id, 7);
                assert!(matches!(
                    completion.result,
                    Ok(V5ServiceTerminalOutcome::Continue)
                ));
                completed = true;
            }
            other => panic!(
                "unexpected service output event: {:?}",
                other.retained_bytes()
            ),
        }
    }

    assert!(chunks > 1);
    assert!(completed);
    assert_eq!(actual_payload, expected_payload);
    assert_eq!(output_events.completion_budget.used(), 0);
}

#[test]
fn v5_service_completion_budget_is_held_while_output_is_backpressured() {
    let temp = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let response = RemoteResponse::Shutdown;
    let payload_len = response.to_v5_payload().unwrap().len();
    let budget = V5ConnectionByteBudget::new(payload_len);
    let (control_tx, _control_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::sync_channel(0);
    let output_events =
        V5ServeOutputSender::with_completion_budget(output_tx, control_tx, budget.clone());
    let worker_output = output_events.clone();
    let cancellation = WorkspaceCancellationToken::new();
    let worker = std::thread::spawn(move || {
        service.enqueue_v5_service_completion(
            V5ServiceCompletion {
                stream_id: 9,
                method: "session.shutdown".to_string(),
                result: Ok(ServiceOutcome::Shutdown),
            },
            protocol_v5::Priority::UserInput,
            &worker_output,
            &cancellation,
        )
    });

    let started = Instant::now();
    while budget.used() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "completion did not reserve its encoded bytes"
        );
        std::thread::yield_now();
    }
    let error = output_events.reserve_completion_bytes(1).unwrap_err();
    assert_eq!(error.code, "resource_exhausted");

    assert!(matches!(
        output_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        V5ServeOutputEvent::StreamData { stream_id: 9, .. }
    ));
    assert!(matches!(
        output_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        V5ServeOutputEvent::Completed(V5ServiceTerminal {
            stream_id: 9,
            result: Ok(V5ServiceTerminalOutcome::Shutdown),
            ..
        })
    ));
    worker.join().unwrap().unwrap();
    assert_eq!(budget.used(), 0);
}

#[test]
fn v5_native_watch_queue_overflow_requests_explicit_resync() {
    let (control_tx, control_rx) = mpsc::channel();
    let (native_tx, native_rx) = mpsc::sync_channel(1);
    let native_events = V5NativeWatchSender::new(native_tx, control_tx);

    native_events
        .send(V5NativeWatchEvent {
            watch_id: 11,
            result: Ok(notify::Event::new(notify::EventKind::Any)),
        })
        .unwrap();
    assert!(matches!(
        control_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        V5ServeEvent::NativeWatch
    ));
    native_events.clear_ready();
    native_events
        .send(V5NativeWatchEvent {
            watch_id: 11,
            result: Ok(notify::Event::new(notify::EventKind::Any)),
        })
        .unwrap();
    assert!(matches!(
        control_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        V5ServeEvent::NativeWatch
    ));
    assert_eq!(native_events.take_overflowed_watch_ids(), vec![11]);
    assert_eq!(
        native_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .watch_id,
        11
    );

    let mut subscription = V5WatchSubscription::new(11, 3, 50, 100, None);
    subscription.roots.insert(".".to_string());
    subscription
        .pending_events
        .push(protocol_v5::WatchChange::modified("src/lib.rs", false));
    subscription.record_native_overflow();
    assert!(subscription.pending_events.is_empty());
    assert!(subscription.pending_overflow);
    assert!(subscription.pending_resync_required);
    assert!(subscription.next_emit.is_some());
}

#[test]
fn v5_concurrent_service_streams_file_search_partial_results() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    std::fs::create_dir(&src).unwrap();
    for index in 0..105 {
        std::fs::write(src.join(format!("file-{index:03}.rs")), "").unwrap();
    }
    let search = RemoteRequest::FileSearch(FileSearchRequest {
        root: PathBuf::new(),
        pattern: Some("file-".to_string()),
        limit: 200,
        hidden: true,
        ..FileSearchRequest::default()
    });
    let mut options = search.v5_request_options();
    options.priority = protocol_v5::Priority::UserInput;
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames_with_options(
        1,
        &search,
        &[],
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
    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    let frames = read_v5_frames(output.bytes());
    assert_v5_data_channel_priority(
        &frames,
        1,
        protocol_v5::DataChannel::SearchPayload,
        protocol_v5::Priority::UserInput,
    );
    let partials = decode_v5_partial_file_search_responses(&frames, 1);
    assert_eq!(partials.len(), 1);
    assert_eq!(partials[0].files.len(), V5_SEARCH_PARTIAL_BATCH_SIZE);
    assert!(!partials[0].truncated);
    let progress = decode_v5_progress_headers(&frames, 1, "search.files");
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].message, "file search matches");
    assert_eq!(progress[0].completed, V5_SEARCH_PARTIAL_BATCH_SIZE as u64);
    assert_eq!(progress[0].total, 200);

    let (response, body, error) = decode_v5_service_response(&frames, 1);
    assert!(error.is_none());
    assert!(body.is_empty());
    let Some(RemoteResponse::FileSearch(final_response)) = response else {
        panic!("expected file search response");
    };
    assert_eq!(final_response.files.len(), 5);
    assert!(!final_response.truncated);
    let mut aggregate_files = partials[0].files.clone();
    aggregate_files.extend(final_response.files.clone());
    aggregate_files.sort();
    assert_eq!(aggregate_files.len(), 105);
    assert_eq!(aggregate_files[0], PathBuf::from("src/file-000.rs"));
    assert_eq!(aggregate_files[104], PathBuf::from("src/file-104.rs"));

    let partial_index = frames
        .iter()
        .position(|frame| {
            if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Headers {
                return false;
            }
            let envelope = frame
                .decode_control::<protocol_v5::StreamEnvelope>()
                .unwrap();
            envelope.role == protocol_v5::MessageRole::PartialResult as i32
                && envelope.method == "search.files"
        })
        .expect("expected partial file search response");
    assert!(
        partial_index < v5_final_response_index(&frames, 1),
        "partial search response should be queued before final response"
    );
    let progress_index = frames
        .iter()
        .position(|frame| {
            if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Headers {
                return false;
            }
            let envelope = frame
                .decode_control::<protocol_v5::StreamEnvelope>()
                .unwrap();
            envelope.role == protocol_v5::MessageRole::Progress as i32
                && envelope.method == "search.files"
        })
        .expect("expected file search progress");
    assert!(
        progress_index < v5_final_response_index(&frames, 1),
        "file search progress should be queued before final response"
    );
}

#[test]
fn v5_concurrent_service_streams_text_search_partial_results() {
    let temp = tempfile::tempdir().unwrap();
    let body = (0..105)
        .map(|index| format!("needle line {index}\n"))
        .collect::<String>();
    std::fs::write(temp.path().join("main.rs"), body).unwrap();
    let search = RemoteRequest::TextSearch(TextSearchRequest {
        root: PathBuf::new(),
        pattern: "needle".to_string(),
        limit: 200,
        hidden: true,
        ..TextSearchRequest::default()
    });
    let mut options = search.v5_request_options();
    options.priority = protocol_v5::Priority::UserInput;
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames_with_options(
        1,
        &search,
        &[],
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
    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    let frames = read_v5_frames(output.bytes());
    assert_v5_data_channel_priority(
        &frames,
        1,
        protocol_v5::DataChannel::SearchPayload,
        protocol_v5::Priority::UserInput,
    );
    let partials = decode_v5_partial_text_search_responses(&frames, 1);
    assert_eq!(partials.len(), 1);
    assert_eq!(partials[0].matches.len(), V5_SEARCH_PARTIAL_BATCH_SIZE);
    assert_eq!(
        partials[0].matches[0].relative_path,
        PathBuf::from("main.rs")
    );
    assert_eq!(partials[0].matches[0].line_number, 1);
    assert!(!partials[0].truncated);
    let progress = decode_v5_progress_headers(&frames, 1, "search.text");
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].message, "text search matches");
    assert_eq!(progress[0].completed, V5_SEARCH_PARTIAL_BATCH_SIZE as u64);
    assert_eq!(progress[0].total, 200);

    let (response, body, error) = decode_v5_service_response(&frames, 1);
    assert!(error.is_none());
    assert!(body.is_empty());
    let Some(RemoteResponse::TextSearch(final_response)) = response else {
        panic!("expected text search response");
    };
    assert_eq!(final_response.matches.len(), 5);
    assert!(!final_response.truncated);
    assert_eq!(final_response.matches[0].line_number, 101);
    assert_eq!(final_response.matches[4].line_number, 105);

    let partial_index = frames
        .iter()
        .position(|frame| {
            if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Headers {
                return false;
            }
            let envelope = frame
                .decode_control::<protocol_v5::StreamEnvelope>()
                .unwrap();
            envelope.role == protocol_v5::MessageRole::PartialResult as i32
                && envelope.method == "search.text"
        })
        .expect("expected partial text search response");
    assert!(
        partial_index < v5_final_response_index(&frames, 1),
        "partial text search response should be queued before final response"
    );
    let progress_index = frames
        .iter()
        .position(|frame| {
            if frame.stream_id != 1 || frame.frame_type != protocol_v5::FrameType::Headers {
                return false;
            }
            let envelope = frame
                .decode_control::<protocol_v5::StreamEnvelope>()
                .unwrap();
            envelope.role == protocol_v5::MessageRole::Progress as i32
                && envelope.method == "search.text"
        })
        .expect("expected text search progress");
    assert!(
        progress_index < v5_final_response_index(&frames, 1),
        "text search progress should be queued before final response"
    );
}

#[test]
fn v5_concurrent_service_cancels_search_after_reset_without_results() {
    let temp = tempfile::tempdir().unwrap();
    for index in 0..200 {
        std::fs::write(temp.path().join(format!("file-{index:03}.txt")), "needle\n").unwrap();
    }
    let search = RemoteRequest::TextSearch(TextSearchRequest {
        root: PathBuf::new(),
        pattern: "needle".to_string(),
        limit: 1_000,
        hidden: true,
        ..TextSearchRequest::default()
    });
    let mut request_frames = v5_request_frames(1, &search, &[]);
    request_frames.push(protocol_v5::reset_stream_frame(
        1,
        protocol_v5::RESET_CANCELLED,
        "query superseded",
    ));
    let input = v5_client_input(request_frames);
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();

    service
        .serve_v5_concurrent(
            protocol_v5::FramedIo::new(Cursor::new(input), output.clone()),
            &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
        )
        .unwrap();

    let frames = read_v5_frames(output.bytes());
    assert!(
        !frames.iter().any(|frame| frame.stream_id == 1
            && matches!(
                frame.frame_type,
                protocol_v5::FrameType::Headers
                    | protocol_v5::FrameType::Data
                    | protocol_v5::FrameType::EndStream
            )),
        "canceled search stream should not receive partial data, final headers, or END_STREAM"
    );
}

#[cfg(unix)]
#[test]
fn v5_concurrent_service_streams_process_output_before_final_response() {
    let temp = tempfile::tempdir().unwrap();
    let process = RemoteRequest::RunProcess(ProcessRequest {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            "printf 'stdout-data'; printf 'stderr-data' >&2".to_string(),
        ],
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    });
    let mut options = process.v5_request_options();
    options.priority = protocol_v5::Priority::UserInput;
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames_with_options(
        1,
        &process,
        &[],
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
    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    let frames = read_v5_frames(output.bytes());
    let (response, _body, error) = decode_v5_service_response(&frames, 1);
    assert!(error.is_none());
    let Some(RemoteResponse::RunProcess(process_response)) = response else {
        panic!("expected streamed process response");
    };
    assert!(process_response.success);
    assert_eq!(process_response.stdout_len, "stdout-data".len());
    assert_eq!(process_response.stderr_len, "stderr-data".len());
    assert_eq!(
        v5_data_for_channel(&frames, 1, protocol_v5::DataChannel::Stdout),
        b"stdout-data"
    );
    assert_eq!(
        v5_data_for_channel(&frames, 1, protocol_v5::DataChannel::Stderr),
        b"stderr-data"
    );
    assert_v5_data_channel_priority(
        &frames,
        1,
        protocol_v5::DataChannel::Stdout,
        protocol_v5::Priority::UserInput,
    );
    assert_v5_data_channel_priority(
        &frames,
        1,
        protocol_v5::DataChannel::Stderr,
        protocol_v5::Priority::UserInput,
    );

    let final_response_index = v5_final_response_index(&frames, 1);
    let stdout_index = v5_first_data_channel_index(&frames, 1, protocol_v5::DataChannel::Stdout)
        .expect("expected streamed stdout DATA frame");
    let stderr_index = v5_first_data_channel_index(&frames, 1, protocol_v5::DataChannel::Stderr)
        .expect("expected streamed stderr DATA frame");
    assert!(
        stdout_index < final_response_index,
        "stdout DATA should be queued before final response headers"
    );
    assert!(
        stderr_index < final_response_index,
        "stderr DATA should be queued before final response headers"
    );
}

#[cfg(unix)]
#[test]
fn v5_concurrent_service_cancels_running_process_on_reset() {
    let temp = tempfile::tempdir().unwrap();
    let process = RemoteRequest::RunProcess(ProcessRequest {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "printf 'started'; sleep 3".to_string()],
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    });
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames(1, &process, &[])));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_input = input.clone();
    let service_output = output.clone();
    let service_thread = std::thread::spawn(move || {
        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(service_input, service_output),
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
    });

    let started = Instant::now();
    loop {
        if find_v5_output_data_for_channel(&output, 1, protocol_v5::DataChannel::Stdout)
            == b"started"
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for process stdout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut reset = Vec::new();
    protocol_v5::write_frame(
        &mut reset,
        &protocol_v5::reset_stream_frame(1, protocol_v5::RESET_CANCELLED, "process cancelled"),
    )
    .unwrap();
    let cancelled_at = Instant::now();
    input.push(reset);
    input.close();
    service_thread.join().unwrap();

    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "service waited for the sleeping process instead of cancelling it"
    );
    let frames = read_v5_frames(output.bytes());
    assert!(
        !frames.iter().any(|frame| frame.stream_id == 1
            && matches!(
                frame.frame_type,
                protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
            )),
        "canceled process stream should not receive final headers or END_STREAM"
    );
}

#[cfg(unix)]
#[test]
fn v5_concurrent_service_cancels_running_process_on_peer_eof() {
    let temp = tempfile::tempdir().unwrap();
    let process = RemoteRequest::RunProcess(ProcessRequest {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "printf 'started'; sleep 3".to_string()],
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    });
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames(1, &process, &[])));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_input = input.clone();
    let service_output = output.clone();
    let service_thread = std::thread::spawn(move || {
        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(service_input, service_output),
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
    });

    let started = Instant::now();
    loop {
        if find_v5_output_data_for_channel(&output, 1, protocol_v5::DataChannel::Stdout)
            == b"started"
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for process stdout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let disconnected_at = Instant::now();
    input.close();
    service_thread.join().unwrap();

    assert!(
        disconnected_at.elapsed() < Duration::from_secs(2),
        "service waited for the sleeping process after peer EOF"
    );
}

#[cfg(unix)]
#[test]
fn v5_concurrent_service_expires_running_process_deadline() {
    let temp = tempfile::tempdir().unwrap();
    let process = RemoteRequest::RunProcess(ProcessRequest {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "printf 'started'; sleep 10".to_string()],
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    });
    let mut options = process.v5_request_options();
    options.deadline_unix_ms = v5_now_unix_millis() + 2_000;
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames_with_options(
        1,
        &process,
        &[],
        options,
    )));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_input = input.clone();
    let service_output = output.clone();
    let service_thread = std::thread::spawn(move || {
        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(service_input, service_output),
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
    });

    let started = Instant::now();
    loop {
        if find_v5_output_data_for_channel(&output, 1, protocol_v5::DataChannel::Stdout)
            == b"started"
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for process stdout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let deadline_wait_started = Instant::now();
    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::ResetStream);
    input.close();
    service_thread.join().unwrap();

    assert!(
        deadline_wait_started.elapsed() < Duration::from_secs(3),
        "service waited for the sleeping process instead of expiring its deadline"
    );
    let frames = read_v5_frames(output.bytes());
    let reset = frames
        .iter()
        .find(|frame| {
            frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
        })
        .expect("deadline expiry should reset the process stream")
        .decode_control::<protocol_v5::ResetStream>()
        .unwrap();
    assert_eq!(reset.code, protocol_v5::RESET_DEADLINE_EXCEEDED);
    assert!(
        !frames.iter().any(|frame| frame.stream_id == 1
            && matches!(
                frame.frame_type,
                protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
            )),
        "expired process stream should not receive final headers or END_STREAM"
    );
}

#[cfg(unix)]
#[test]
fn v5_concurrent_service_cancels_superseded_running_stream() {
    let temp = tempfile::tempdir().unwrap();
    let process = RemoteRequest::RunProcess(ProcessRequest {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "printf 'started'; sleep 3".to_string()],
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    });
    let stat = RemoteRequest::Stat {
        path: PathBuf::new(),
    };
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames(1, &process, &[])));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_input = input.clone();
    let service_output = output.clone();
    let service_thread = std::thread::spawn(move || {
        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(service_input, service_output),
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
    });

    let started = Instant::now();
    loop {
        if find_v5_output_data_for_channel(&output, 1, protocol_v5::DataChannel::Stdout)
            == b"started"
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for process stdout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut options = stat.v5_request_options();
    options.supersedes_stream_id = 1;
    let cancelled_at = Instant::now();
    input.push(v5_frames_bytes(v5_request_frames_with_options(
        3,
        &stat,
        &[],
        options,
    )));
    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::ResetStream);
    wait_for_v5_stream_frame(&output, 3, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "service waited for the superseded sleeping process instead of cancelling it"
    );
    let frames = read_v5_frames(output.bytes());
    let reset = frames
        .iter()
        .find(|frame| {
            frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::ResetStream
        })
        .expect("supersession should reset the old stream")
        .decode_control::<protocol_v5::ResetStream>()
        .unwrap();
    assert_eq!(reset.code, protocol_v5::RESET_CANCELLED);
    let (stat_response, _, stat_error) = decode_v5_service_response(&frames, 3);
    assert!(stat_error.is_none());
    assert!(matches!(stat_response, Some(RemoteResponse::Stat(_))));
}

#[cfg(unix)]
#[test]
fn v5_cancellable_git_command_kills_process_group() {
    let temp = tempfile::tempdir().unwrap();
    let started_file = temp.path().join("git-started");
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "printf started > \"$STARTED_FILE\"; sleep 3"])
        .current_dir(temp.path())
        .env("STARTED_FILE", &started_file);
    let cancellation = WorkspaceCancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let root = temp.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        v5_run_cancellable_command_collect(command, "git status", &root, Some(&worker_cancellation))
    });

    let started = Instant::now();
    while !started_file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for fake git process to start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let cancelled_at = Instant::now();
    cancellation.cancel();
    let error = worker.join().unwrap().unwrap_err();

    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "cancellable git command waited for the child sleep instead of killing its process group"
    );
    let WorkspaceError::CommandFailed {
        operation, message, ..
    } = error
    else {
        panic!("expected command failure after cancellation");
    };
    assert_eq!(operation, "git status");
    assert_eq!(message, "git status cancelled");
}

#[test]
fn v5_list_dirs_promotes_backend_cancellation_to_request_error() {
    let temp = tempfile::tempdir().unwrap();
    let request = RemoteRequest::ListDirs {
        paths: vec![PathBuf::from("first"), PathBuf::from("second")],
    };

    let backend = ConcurrentV5Backend::new();
    backend.return_list_cancelled();
    let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
    let cancellation = WorkspaceCancellationToken::new();
    let Err(error) = service.execute(request.clone(), Vec::new(), &cancellation) else {
        panic!("generic list_dirs should promote backend cancellation");
    };
    assert_eq!(error.code, protocol_v5::RESET_CANCELLED);
    assert!(!cancellation.is_cancelled());
    assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);

    let backend = ConcurrentV5Backend::new();
    backend.return_list_cancelled();
    let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
    let (method, payload) = request.to_v5_method_payload().unwrap();
    let budget = V5ConnectionByteBudget::new(V5_REQUEST_CONNECTION_BYTE_BUDGET);
    let service_request = V5ServiceRequest {
        method: method.to_string(),
        priority: protocol_v5::Priority::VisibleFileTree,
        payload,
        body: Vec::new(),
        retained_bytes: budget.reservation(),
        received_payload_bytes: 0,
        received_body_bytes: 0,
        deadline_unix_ms: 0,
        supersedes_stream_id: 0,
        streamed_write: None,
        early_error: None,
    };
    let cancellation = WorkspaceCancellationToken::new();
    let Err(error) = service.execute_v5_list_dirs_request(&service_request, &cancellation) else {
        panic!("v5 list_dirs should promote backend cancellation");
    };
    assert_eq!(error.code, protocol_v5::RESET_CANCELLED);
    assert!(!cancellation.is_cancelled());
    assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);
}

#[test]
fn v5_list_dirs_reset_cancels_batch_before_next_path() {
    let temp = tempfile::tempdir().unwrap();
    let request = RemoteRequest::ListDirs {
        paths: vec![PathBuf::from("first"), PathBuf::from("second")],
    };
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames(1, &request, &[])));
    let output = SharedWrite::default();
    let backend = ConcurrentV5Backend::new();
    let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );
    let cancellation = backend.wait_for_first_list_cancellation();

    input.push(v5_frames_bytes(vec![protocol_v5::reset_stream_frame(
        1,
        protocol_v5::RESET_CANCELLED,
        "list superseded",
    )]));
    let started = Instant::now();
    while !cancellation.is_cancelled() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "reset did not reach the filesystem cancellation token"
        );
        std::thread::yield_now();
    }
    backend.release_first_list();
    input.close();
    service_thread.join().unwrap();

    assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);
    let frames = read_v5_frames(output.bytes());
    assert!(!frames.iter().any(|frame| {
        frame.stream_id == 1
            && matches!(
                frame.frame_type,
                protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
            )
    }));
}

#[test]
fn v5_shutdown_grace_cancels_blocked_filesystem_worker() {
    let temp = tempfile::tempdir().unwrap();
    let list = RemoteRequest::ListDirs {
        paths: vec![PathBuf::from("first"), PathBuf::from("second")],
    };
    let shutdown = RemoteRequest::Shutdown;
    let mut request_frames = v5_request_frames(1, &list, &[]);
    request_frames.extend(v5_request_frames(3, &shutdown, &[]));

    let mut settings = protocol_v5::ConnectionSettings::recommended();
    settings.shutdown_grace_ms = 200;
    let input = BlockingRead::default();
    input.push(v5_client_input_with_settings(request_frames, settings));
    let output = SharedWrite::default();
    let backend = ConcurrentV5Backend::new();
    let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );
    let cancellation = backend.wait_for_first_list_cancellation();

    wait_for_v5_stream_frame(&output, 3, protocol_v5::FrameType::EndStream);
    assert!(
        !cancellation.is_cancelled(),
        "shutdown should allow active work to drain during the negotiated grace"
    );

    let started = Instant::now();
    while !cancellation.is_cancelled() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown grace expiry did not reach the filesystem cancellation token"
        );
        std::thread::yield_now();
    }

    backend.release_first_list();
    input.close();
    service_thread.join().unwrap();

    assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);
    let frames = read_v5_frames(output.bytes());
    let (response, body, error) = decode_v5_service_response(&frames, 3);
    assert_eq!(response, Some(RemoteResponse::Shutdown));
    assert!(body.is_empty());
    assert!(error.is_none());

    let shutdown_response_index = v5_final_response_index(&frames, 3);
    let goaway_index = frames
        .iter()
        .position(|frame| frame.frame_type == protocol_v5::FrameType::GoAway)
        .expect("shutdown should emit GOAWAY");
    assert!(goaway_index > shutdown_response_index);
    assert!(!frames.iter().any(|frame| {
        frame.stream_id == 1
            && matches!(
                frame.frame_type,
                protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
            )
    }));
}

#[test]
fn v5_shutdown_grace_remains_live_while_server_writer_is_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let list = RemoteRequest::ListDirs {
        paths: vec![PathBuf::from("first"), PathBuf::from("second")],
    };
    let mut settings = protocol_v5::ConnectionSettings::recommended();
    settings.shutdown_grace_ms = 200;
    let input = BlockingRead::default();
    input.push(v5_client_input_with_settings(Vec::new(), settings));
    let output = PausingWrite::default();
    let backend = ConcurrentV5Backend::new();
    let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
    let service_input = input.clone();
    let service_output = output.clone();
    let info = protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string());
    let service_thread = std::thread::spawn(move || {
        service.serve_v5_concurrent(
            protocol_v5::FramedIo::new(service_input, service_output),
            &info,
        )
    });

    let handshake_started = Instant::now();
    while !read_v5_complete_frames(output.bytes())
        .iter()
        .any(|frame| frame.frame_type == protocol_v5::FrameType::Settings)
    {
        assert!(
            handshake_started.elapsed() < Duration::from_secs(2),
            "timed out waiting for the v5 server handshake"
        );
        std::thread::yield_now();
    }

    output.pause_next_write();
    input.push(v5_frames_bytes(vec![protocol_v5::Frame::from_control(
        protocol_v5::FrameType::Ping,
        0,
        &protocol_v5::PingPayload {
            token: b"block-server-writer".to_vec(),
        },
    )]));
    output.wait_until_paused();
    input.push(v5_frames_bytes(v5_request_frames(1, &list, &[])));
    let cancellation = backend.wait_for_first_list_cancellation();
    input.push(v5_frames_bytes(v5_request_frames(
        3,
        &RemoteRequest::Shutdown,
        &[],
    )));

    let cancellation_started = Instant::now();
    while !cancellation.is_cancelled() && cancellation_started.elapsed() < Duration::from_secs(2) {
        std::thread::yield_now();
    }
    let cancelled_while_writer_stalled = cancellation.is_cancelled();

    backend.release_first_list();
    let service_exit_started = Instant::now();
    while !service_thread.is_finished() && service_exit_started.elapsed() < Duration::from_secs(2) {
        std::thread::yield_now();
    }
    let exited_while_writer_stalled = service_thread.is_finished();

    output.release();
    input.close();
    service_thread.join().unwrap().unwrap();

    assert!(
        cancelled_while_writer_stalled,
        "shutdown grace expiry was blocked behind the physical server write"
    );
    assert!(
        exited_while_writer_stalled,
        "server waited for a blocked physical writer after shutdown grace expiry"
    );
    assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);
}

#[test]
fn v5_peer_eof_cancels_blocked_filesystem_worker() {
    let temp = tempfile::tempdir().unwrap();
    let request = RemoteRequest::ListDirs {
        paths: vec![PathBuf::from("first"), PathBuf::from("second")],
    };
    let input = BlockingRead::default();
    input.push(v5_client_input(v5_request_frames(1, &request, &[])));
    let output = SharedWrite::default();
    let backend = ConcurrentV5Backend::new();
    let service = WorkspaceService::new(backend.clone(), temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );
    let cancellation = backend.wait_for_first_list_cancellation();

    input.close();
    let started = Instant::now();
    while !cancellation.is_cancelled() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "peer EOF did not reach the filesystem cancellation token"
        );
        std::thread::yield_now();
    }
    backend.release_first_list();
    service_thread.join().unwrap();

    assert_eq!(backend.list_dir_calls(), vec![temp.path().join("first")]);
}

#[test]
fn v5_concurrent_service_completes_fast_stream_while_slow_stream_waits() {
    let temp = tempfile::tempdir().unwrap();
    let read = RemoteRequest::ReadFile {
        path: PathBuf::from("slow.txt"),
        max_bytes: None,
    };
    let stat = RemoteRequest::Stat {
        path: PathBuf::from("fast.txt"),
    };
    let mut request_frames = v5_request_frames(1, &read, &[]);
    request_frames.extend(v5_request_frames(3, &stat, &[]));
    let input = BlockingRead::default();
    input.push(v5_client_input(request_frames));
    let output = SharedWrite::default();
    let service =
        WorkspaceService::new(ConcurrentV5Backend::new(), temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );
    wait_for_v5_stream_frame(&output, 1, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    let frames = read_v5_frames(output.bytes());
    let (stat_response, _, stat_error) = decode_v5_service_response(&frames, 3);
    let (read_response, read_body, read_error) = decode_v5_service_response(&frames, 1);

    assert!(stat_error.is_none());
    assert!(read_error.is_none());
    assert!(matches!(stat_response, Some(RemoteResponse::Stat(_))));
    assert!(matches!(read_response, Some(RemoteResponse::ReadFile(_))));
    assert_eq!(read_body, b"slow");
    assert!(
        v5_final_response_index(&frames, 3) < v5_final_response_index(&frames, 1),
        "fast stat stream should complete before the earlier slow read stream"
    );
}

#[test]
fn v5_concurrent_service_suppresses_response_after_client_reset() {
    let temp = tempfile::tempdir().unwrap();
    let read = RemoteRequest::ReadFile {
        path: PathBuf::from("slow.txt"),
        max_bytes: None,
    };
    let stat = RemoteRequest::Stat {
        path: PathBuf::from("fast.txt"),
    };
    let mut request_frames = v5_request_frames(1, &read, &[]);
    request_frames.push(protocol_v5::reset_stream_frame(
        1,
        protocol_v5::RESET_CANCELLED,
        "client superseded read",
    ));
    request_frames.extend(v5_request_frames(3, &stat, &[]));
    let input = BlockingRead::default();
    input.push(v5_client_input(request_frames));
    let output = SharedWrite::default();
    let service =
        WorkspaceService::new(ConcurrentV5Backend::new(), temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );
    wait_for_v5_stream_frame(&output, 3, protocol_v5::FrameType::EndStream);
    input.close();
    service_thread.join().unwrap();

    let frames = read_v5_frames(output.bytes());
    let (stat_response, _, stat_error) = decode_v5_service_response(&frames, 3);
    assert!(stat_error.is_none());
    assert!(matches!(stat_response, Some(RemoteResponse::Stat(_))));
    assert!(
        !frames.iter().any(|frame| frame.stream_id == 1
            && matches!(
                frame.frame_type,
                protocol_v5::FrameType::Headers | protocol_v5::FrameType::EndStream
            )),
        "canceled stream should not receive final headers or END_STREAM"
    );
}

#[test]
fn v5_concurrent_service_sends_idle_ping() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = protocol_v5::ConnectionSettings::recommended();
    settings.idle_ping_interval_ms = protocol_v5::MIN_UNSOLICITED_PING_INTERVAL_MS;
    settings.ping_timeout_ms = 1_000;
    let input = BlockingRead::default();
    input.push(v5_client_input_with_settings(Vec::new(), settings));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_input = input.clone();
    let service_output = output.clone();
    let service_thread = std::thread::spawn(move || {
        service
            .serve_v5_concurrent(
                protocol_v5::FramedIo::new(service_input, service_output),
                &protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
            )
            .unwrap();
    });

    let started = Instant::now();
    let ping = loop {
        let frames = read_v5_frames(output.bytes());
        if let Some(frame) = frames
            .into_iter()
            .find(|frame| frame.frame_type == protocol_v5::FrameType::Ping)
        {
            break frame;
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "timed out waiting for idle PING"
        );
        std::thread::sleep(Duration::from_millis(10));
    };

    let payload = ping.decode_control::<protocol_v5::PingPayload>().unwrap();
    assert!(!payload.token.is_empty());
    input.close();
    service_thread.join().unwrap();
}

#[cfg(unix)]
#[test]
fn v5_concurrent_service_outbound_progress_does_not_suppress_idle_ping() {
    let temp = tempfile::tempdir().unwrap();
    let process = RemoteRequest::RunProcess(ProcessRequest {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            "i=0; while [ \"$i\" -lt 400 ]; do printf x; sleep 0.02; i=$((i + 1)); done"
                .to_string(),
        ],
        cwd: PathBuf::new(),
        env: BTreeMap::new(),
        clear_env: false,
        inherit_project_environment: false,
        max_output_bytes: None,
        timeout_ms: None,
    });
    let mut settings = protocol_v5::ConnectionSettings::recommended();
    settings.idle_ping_interval_ms = protocol_v5::MIN_UNSOLICITED_PING_INTERVAL_MS;
    settings.ping_timeout_ms = 1_000;
    let input = BlockingRead::default();
    input.push(v5_client_input_with_settings(
        v5_request_frames(1, &process, &[]),
        settings,
    ));
    let output = SharedWrite::default();
    let service = WorkspaceService::new(LocalWorkspaceBackend, temp.path().to_path_buf()).unwrap();
    let service_thread = spawn_v5_concurrent_service(
        service,
        &input,
        &output,
        protocol_v5::ServerHandshakeInfo::current(temp.path().display().to_string()),
    );

    let _ping = wait_for_v5_connection_frame_after_with_timeout(
        &output,
        protocol_v5::FrameType::Ping,
        2,
        Duration::from_secs(8),
    );
    assert!(
        !read_v5_frames(output.bytes()).into_iter().any(|frame| {
            frame.stream_id == 1 && frame.frame_type == protocol_v5::FrameType::EndStream
        }),
        "outbound progress must not postpone the heartbeat until the process completes"
    );

    input.close();
    service_thread.join().unwrap();
}
