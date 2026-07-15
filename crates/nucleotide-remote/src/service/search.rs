// ABOUTME: Streaming file and text search execution for protocol v5 services
// ABOUTME: Emits bounded partial batches, progress, and terminal search metadata

use super::*;

pub(crate) fn v5_response_body_chunks(
    response: &RemoteResponse,
    body: Vec<u8>,
) -> std::result::Result<Vec<(protocol_v5::DataChannel, Vec<u8>)>, RemoteError> {
    if body.is_empty() {
        return Ok(Vec::new());
    }

    match response {
        RemoteResponse::ReadFile(_) => Ok(vec![(protocol_v5::DataChannel::FileBody, body)]),
        RemoteResponse::RunProcess(process) => {
            let total_len = process
                .stdout_len
                .checked_add(process.stderr_len)
                .ok_or_else(|| RemoteError {
                    code: "invalid_response".to_string(),
                    message: "process output length overflow".to_string(),
                    diagnostic: None,
                })?;
            if total_len > body.len() {
                return Err(RemoteError {
                    code: "invalid_response".to_string(),
                    message: "process output body is shorter than declared lengths".to_string(),
                    diagnostic: Some(format!(
                        "stdout_len={} stderr_len={} body_len={}",
                        process.stdout_len,
                        process.stderr_len,
                        body.len()
                    )),
                });
            }
            let stdout = body[..process.stdout_len].to_vec();
            let stderr = body[process.stdout_len..total_len].to_vec();
            let mut chunks = Vec::new();
            if !stdout.is_empty() {
                chunks.push((protocol_v5::DataChannel::Stdout, stdout));
            }
            if !stderr.is_empty() {
                chunks.push((protocol_v5::DataChannel::Stderr, stderr));
            }
            Ok(chunks)
        }
        _ => Ok(vec![(protocol_v5::DataChannel::Unspecified, body)]),
    }
}

pub(crate) fn v5_streaming_file_search(
    query: FileSearchQuery,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<FileSearchResult, RemoteError> {
    let pattern = query
        .pattern
        .as_ref()
        .map(|pattern| RegexBuilder::new(pattern).case_insensitive(true).build())
        .transpose()
        .map_err(|error| {
            remote_error_from_workspace(WorkspaceError::InvalidSearchPattern(error))
        })?;
    let mut walker = WalkBuilder::new(&query.root);
    walker
        .hidden(!query.hidden)
        .parents(query.parents)
        .ignore(query.ignore)
        .git_ignore(query.git_ignore)
        .git_global(query.git_global)
        .git_exclude(query.git_exclude)
        .follow_links(query.follow_links)
        .add_custom_ignore_filename(".helix/ignore");
    if !query.excluded_relative_prefixes.is_empty() {
        let root = query.root.clone();
        let excluded_relative_prefixes = query.excluded_relative_prefixes.clone();
        walker.filter_entry(move |entry| {
            let relative_path = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            !excluded_relative_prefixes
                .iter()
                .any(|prefix| relative_path.starts_with(prefix))
        });
    }
    if let Some(max_depth) = query.max_depth {
        walker.max_depth(Some(max_depth));
    }

    let mut matched_count = 0_usize;
    let mut partial_files = Vec::new();
    let mut partial_flush = V5SearchPartialFlush::new();
    let mut truncated = false;
    for entry in walker.build() {
        if v5_stream_cancelled_ref(cancellation) {
            return Err(v5_cancelled_search_error(&query.root));
        }
        let entry = entry.map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "walk directory",
                path: query.root.clone(),
                source: io::Error::other(source),
            })
        })?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let relative_path = entry
            .path()
            .strip_prefix(&query.root)
            .unwrap_or(entry.path())
            .to_path_buf();
        if relative_path.as_os_str().is_empty() {
            continue;
        }
        if let Some(pattern) = &pattern
            && !pattern.is_match(&relative_path.to_string_lossy())
        {
            continue;
        }
        if matched_count >= query.limit {
            truncated = true;
            break;
        }
        matched_count += 1;
        partial_files.push(relative_path);
        if partial_flush.should_flush(partial_files.len()) {
            v5_send_file_search_partial(
                stream_id,
                &query.root,
                priority,
                &stream_events,
                std::mem::take(&mut partial_files),
                cancellation,
            )?;
            v5_send_search_progress(
                stream_id,
                "search.files",
                "file search matches",
                matched_count as u64,
                query.limit as u64,
                &stream_events,
                cancellation,
            )?;
            partial_flush.mark_flushed();
        }
    }

    Ok(FileSearchResult {
        root: query.root,
        files: partial_files,
        truncated,
    })
}

pub(crate) fn v5_send_file_search_partial(
    stream_id: u64,
    root: &Path,
    priority: protocol_v5::Priority,
    stream_events: &V5ServeOutputSender,
    files: Vec<PathBuf>,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<(), RemoteError> {
    if files.is_empty() {
        return Ok(());
    }
    let payload = RemoteResponse::FileSearch(FileSearchResponse {
        root: root.to_path_buf(),
        files,
        truncated: false,
    })
    .to_v5_payload()
    .map_err(v5_method_error_to_remote_error)?;
    v5_send_output_event_with_optional_cancellation(
        stream_events,
        V5ServeOutputEvent::PartialResponse {
            stream_id,
            method: "search.files".to_string(),
            payload,
            priority,
        },
        cancellation,
    )
    .map_err(v5_queue_error_to_remote_error)
}

pub(crate) fn v5_streaming_text_search(
    query: TextSearchQuery,
    stream_id: u64,
    priority: protocol_v5::Priority,
    stream_events: V5ServeOutputSender,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<TextSearchResult, RemoteError> {
    let case_insensitive = query.smart_case && !query.pattern.chars().any(char::is_uppercase);
    let pattern = RegexBuilder::new(&query.pattern)
        .case_insensitive(case_insensitive)
        .multi_line(true)
        .build()
        .map_err(|error| {
            remote_error_from_workspace(WorkspaceError::InvalidSearchPattern(error))
        })?;
    let mut walker = WalkBuilder::new(&query.root);
    walker
        .hidden(!query.hidden)
        .parents(query.parents)
        .ignore(query.ignore)
        .git_ignore(query.git_ignore)
        .git_global(query.git_global)
        .git_exclude(query.git_exclude)
        .follow_links(query.follow_links)
        .add_custom_ignore_filename(".helix/ignore");
    for filename in &query.custom_ignore_filenames {
        walker.add_custom_ignore_filename(filename);
    }
    if let Some(max_depth) = query.max_depth {
        walker.max_depth(Some(max_depth));
    }

    let mut matched_count = 0_usize;
    let mut partial_matches = Vec::new();
    let mut partial_flush = V5SearchPartialFlush::new();
    let mut truncated = false;
    let excluded_relative_paths = query
        .excluded_relative_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();

    'walk: for entry in walker.build() {
        if v5_stream_cancelled_ref(cancellation) {
            return Err(v5_cancelled_search_error(&query.root));
        }
        let entry = entry.map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "walk directory",
                path: query.root.clone(),
                source: io::Error::other(source),
            })
        })?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let metadata = std::fs::metadata(entry.path()).map_err(|source| {
            remote_error_from_workspace(WorkspaceError::Io {
                operation: "stat search file",
                path: entry.path().to_path_buf(),
                source,
            })
        })?;
        if metadata.len() > query.max_file_bytes {
            continue;
        }

        let Ok(contents) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let relative_path = entry
            .path()
            .strip_prefix(&query.root)
            .unwrap_or(entry.path())
            .to_path_buf();
        if relative_path.as_os_str().is_empty() {
            continue;
        }
        if excluded_relative_paths.contains(&relative_path) {
            continue;
        }

        for (line_index, line_text) in contents.lines().enumerate() {
            if v5_stream_cancelled_ref(cancellation) {
                return Err(v5_cancelled_search_error(&query.root));
            }
            for found in pattern.find_iter(line_text) {
                if matched_count >= query.limit {
                    truncated = true;
                    break 'walk;
                }
                let search_match = TextSearchMatch {
                    relative_path: relative_path.clone(),
                    line_number: line_index + 1,
                    line_text: line_text.to_string(),
                    start: found.start(),
                    end: found.end(),
                };
                matched_count += 1;
                partial_matches.push(search_match);
                if partial_flush.should_flush(partial_matches.len()) {
                    v5_send_text_search_partial(
                        stream_id,
                        &query.root,
                        priority,
                        &stream_events,
                        std::mem::take(&mut partial_matches),
                        cancellation,
                    )?;
                    v5_send_search_progress(
                        stream_id,
                        "search.text",
                        "text search matches",
                        matched_count as u64,
                        query.limit as u64,
                        &stream_events,
                        cancellation,
                    )?;
                    partial_flush.mark_flushed();
                }
            }
        }
    }

    Ok(TextSearchResult {
        root: query.root,
        matches: partial_matches,
        truncated,
    })
}

pub(crate) fn v5_send_text_search_partial(
    stream_id: u64,
    root: &Path,
    priority: protocol_v5::Priority,
    stream_events: &V5ServeOutputSender,
    matches: Vec<TextSearchMatch>,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<(), RemoteError> {
    if matches.is_empty() {
        return Ok(());
    }
    let payload = RemoteResponse::TextSearch(text_search_response(TextSearchResult {
        root: root.to_path_buf(),
        matches,
        truncated: false,
    }))
    .to_v5_payload()
    .map_err(v5_method_error_to_remote_error)?;
    v5_send_output_event_with_optional_cancellation(
        stream_events,
        V5ServeOutputEvent::PartialResponse {
            stream_id,
            method: "search.text".to_string(),
            payload,
            priority,
        },
        cancellation,
    )
    .map_err(v5_queue_error_to_remote_error)
}

pub(crate) fn v5_send_search_progress(
    stream_id: u64,
    method: &str,
    message: &str,
    completed: u64,
    total: u64,
    stream_events: &V5ServeOutputSender,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<(), RemoteError> {
    v5_send_output_event_with_optional_cancellation(
        stream_events,
        V5ServeOutputEvent::Progress {
            stream_id,
            method: method.to_string(),
            progress: protocol_v5::Progress {
                message: message.to_string(),
                completed,
                total,
            },
        },
        cancellation,
    )
    .map_err(v5_queue_error_to_remote_error)
}

pub(crate) fn v5_send_output_event_with_optional_cancellation(
    output_events: &V5ServeOutputSender,
    event: V5ServeOutputEvent,
    cancellation: Option<&WorkspaceCancellationToken>,
) -> std::result::Result<(), V5ServeQueueError> {
    if let Some(cancellation) = cancellation {
        output_events.send_with_cancellation(event, cancellation)
    } else {
        output_events.send(event)
    }
}

#[derive(Debug)]
pub(crate) struct V5SearchPartialFlush {
    pub(crate) last_emit: Instant,
}

impl V5SearchPartialFlush {
    pub(crate) fn new() -> Self {
        Self {
            last_emit: Instant::now(),
        }
    }

    pub(crate) fn should_flush(&self, pending_len: usize) -> bool {
        pending_len >= V5_SEARCH_PARTIAL_BATCH_SIZE
            || (pending_len > 0
                && self.last_emit.elapsed() >= Duration::from_millis(V5_SEARCH_PARTIAL_INTERVAL_MS))
    }

    pub(crate) fn mark_flushed(&mut self) {
        self.last_emit = Instant::now();
    }
}

pub(crate) fn v5_cancelled_search_error(root: &Path) -> RemoteError {
    RemoteError {
        code: protocol_v5::RESET_CANCELLED.to_string(),
        message: "search cancelled".to_string(),
        diagnostic: Some(root.display().to_string()),
    }
}
