// ABOUTME: Client-side remote watch delivery and directory-cache coordination
// ABOUTME: Converts v5 watch batches and invalidates cached directory state

use super::*;

pub(crate) fn apply_v5_directory_cache<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    request: &RemoteRequest,
    response: RemoteResponse,
) -> std::result::Result<RemoteResponse, RemoteClientError> {
    match (request, response) {
        (RemoteRequest::ListDir { path }, RemoteResponse::ListDir(listing)) => {
            let listing = resolve_v5_directory_listing_cache(shared, path, listing)?;
            Ok(RemoteResponse::ListDir(listing))
        }
        (RemoteRequest::ListDirs { .. }, RemoteResponse::ListDirs(mut response)) => {
            for result in &mut response.results {
                if let Some(listing) = result.listing.take() {
                    result.listing = Some(resolve_v5_directory_listing_cache(
                        shared,
                        &result.path,
                        listing,
                    )?);
                }
            }
            Ok(RemoteResponse::ListDirs(response))
        }
        (_, response) => Ok(response),
    }
}

pub(crate) fn resolve_v5_directory_listing_cache<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    cache_key: &Path,
    mut listing: DirectoryListingResponse,
) -> std::result::Result<DirectoryListingResponse, RemoteClientError> {
    let mut cache = shared
        .directory_cache
        .lock()
        .map_err(v5_client_lock_error)?;
    if listing.not_modified {
        return cache.get(cache_key).cloned().ok_or_else(|| {
            RemoteClientError::Protocol(format!(
                "v5 directory listing for {} was not_modified without a cached listing",
                cache_key.display()
            ))
        });
    }
    if let Some(delta) = listing.delta.take() {
        let base = cache.get(cache_key).cloned().ok_or_else(|| {
            RemoteClientError::Protocol(format!(
                "v5 directory listing for {} carried a delta without a cached base",
                cache_key.display()
            ))
        })?;
        listing = apply_directory_listing_delta(cache_key, base, listing, delta)?;
    }
    if listing.complete && listing.generation.is_some() {
        if !cache.contains_key(cache_key)
            && cache.len() >= V5_DIRECTORY_DELTA_CACHE_LIMIT
            && let Some(evicted) = cache.keys().next().cloned()
        {
            cache.remove(&evicted);
        }
        cache.insert(cache_key.to_path_buf(), listing.clone());
    }
    Ok(listing)
}

pub(crate) fn handle_v5_client_watch_event<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    event: protocol_v5::StreamEvent,
) {
    match event {
        protocol_v5::StreamEvent::Headers {
            stream_id,
            role: protocol_v5::MessageRole::Event,
            envelope,
            ..
        } => {
            let Some(protocol_v5::stream_envelope::Message::Event(event)) = envelope.message else {
                return;
            };
            if event.kind != "watch.batch" {
                return;
            }
            let Some(batch) = event.watch_batch else {
                return;
            };
            send_or_backlog_v5_watch_batch(shared, stream_id, batch);
        }
        protocol_v5::StreamEvent::EndStream { stream_id }
        | protocol_v5::StreamEvent::ResetStream { stream_id, .. } => {
            if let Ok(mut watch_batches) = shared.watch_batches.lock() {
                watch_batches.remove(&stream_id);
            }
            if let Ok(mut watch_backlog) = shared.watch_backlog.lock() {
                watch_backlog.remove(&stream_id);
            }
            if let Ok(mut watch_stream_by_id) = shared.watch_stream_by_id.lock() {
                watch_stream_by_id.retain(|_, event_stream_id| *event_stream_id != stream_id);
            }
        }
        _ => {}
    }
}

pub(crate) fn send_or_backlog_v5_watch_batch<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    stream_id: u64,
    batch: protocol_v5::WatchBatch,
) {
    invalidate_v5_directory_cache_after_watch_batch(shared, &batch);

    let delivery = match shared.watch_batches.lock() {
        Ok(watch_batches) => watch_batches.get(&stream_id).cloned(),
        Err(_) => return,
    };
    if let Some(delivery) = delivery {
        delivery
            .last_sequence
            .store(batch.sequence, Ordering::Release);
        if delivery.overflowed.load(Ordering::Acquire) {
            clear_v5_directory_cache(shared);
            return;
        }
        match delivery.sender.try_send(batch) {
            Ok(()) => return,
            Err(mpsc::TrySendError::Full(_)) => {
                delivery.overflowed.store(true, Ordering::Release);
                clear_v5_directory_cache(shared);
                return;
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                if let Ok(mut watch_batches) = shared.watch_batches.lock() {
                    watch_batches.remove(&stream_id);
                }
                return;
            }
        }
    }

    let Ok(mut watch_backlog) = shared.watch_backlog.lock() else {
        return;
    };
    let backlog = watch_backlog.entry(stream_id).or_default();
    if let Some(overflow) = backlog
        .back_mut()
        .filter(|batch| batch.overflow && batch.resync_required)
    {
        overflow.sequence = batch.sequence;
        return;
    }
    if backlog.len() >= V5_WATCH_BACKLOG_LIMIT {
        let mut overflow = batch;
        overflow.directory_generations.clear();
        overflow.events.clear();
        overflow.overflow = true;
        overflow.resync_required = true;
        backlog.clear();
        backlog.push_back(overflow);
        drop(watch_backlog);
        clear_v5_directory_cache(shared);
        return;
    }
    backlog.push_back(batch);
}

pub(crate) fn invalidate_v5_directory_cache_after_watch_batch<W>(
    shared: &RemoteWorkspaceV5Shared<W>,
    batch: &protocol_v5::WatchBatch,
) {
    if !batch.overflow && !batch.resync_required {
        return;
    }
    clear_v5_directory_cache(shared);
}

pub(crate) fn clear_v5_directory_cache<W>(shared: &RemoteWorkspaceV5Shared<W>) {
    if let Ok(mut directory_cache) = shared.directory_cache.lock() {
        directory_cache.clear();
    }
}

pub(crate) fn workspace_watch_from_v5(
    watch: RemoteWorkspaceV5Watch,
    workspace_root: PathBuf,
) -> WorkspaceWatch {
    let (sender, receiver) = mpsc::sync_channel(V5_WATCH_DELIVERY_CAPACITY);
    let watch_id = watch.watch_id;
    let event_stream_id = watch.event_stream_id;
    std::thread::Builder::new()
        .name("nucleotide-v5-watch-map".to_string())
        .spawn(move || {
            while let Ok(batch) = watch.recv() {
                let batch = workspace_watch_batch_from_v5(batch, &workspace_root);
                if sender.send(batch).is_err() {
                    break;
                }
            }
        })
        .ok();
    WorkspaceWatch::new(watch_id, event_stream_id, receiver)
}

pub(crate) fn workspace_watch_update_from_v5(
    response: protocol_v5::WatchUpdateResponse,
    workspace_root: &Path,
) -> WorkspaceWatchUpdate {
    WorkspaceWatchUpdate {
        watch_id: response.watch_id,
        accepted_roots: response
            .accepted_roots
            .iter()
            .map(|path| v5_watch_path_to_workspace_path(workspace_root, path))
            .collect(),
        degraded_roots: response
            .degraded_roots
            .iter()
            .map(|path| v5_watch_path_to_workspace_path(workspace_root, path))
            .collect(),
        unsupported_roots: response
            .unsupported_roots
            .iter()
            .map(|path| v5_watch_path_to_workspace_path(workspace_root, path))
            .collect(),
    }
}

pub(crate) fn workspace_watch_batch_from_v5(
    batch: protocol_v5::WatchBatch,
    workspace_root: &Path,
) -> WorkspaceWatchBatch {
    WorkspaceWatchBatch {
        watch_id: batch.watch_id,
        sequence: batch.sequence,
        directory_generations: batch
            .directory_generations
            .into_iter()
            .map(|generation| WorkspaceWatchDirectoryGeneration {
                path: v5_watch_path_to_workspace_path(workspace_root, &generation.path),
                generation: generation.generation,
            })
            .collect(),
        events: batch
            .events
            .into_iter()
            .map(|event| WorkspaceWatchChange {
                kind: workspace_watch_change_kind_from_v5(event.kind),
                path: v5_watch_path_to_workspace_path(workspace_root, &event.path),
                old_path: (!event.old_path.is_empty())
                    .then(|| v5_watch_path_to_workspace_path(workspace_root, &event.old_path)),
                is_dir: event.is_dir,
            })
            .collect(),
        overflow: batch.overflow,
        resync_required: batch.resync_required,
    }
}

pub(crate) fn workspace_watch_change_kind_from_v5(kind: i32) -> WorkspaceWatchChangeKind {
    match protocol_v5::WatchChangeKind::try_from(kind) {
        Ok(protocol_v5::WatchChangeKind::Created) => WorkspaceWatchChangeKind::Created,
        Ok(protocol_v5::WatchChangeKind::Modified) => WorkspaceWatchChangeKind::Modified,
        Ok(protocol_v5::WatchChangeKind::Deleted) => WorkspaceWatchChangeKind::Deleted,
        Ok(protocol_v5::WatchChangeKind::Renamed) => WorkspaceWatchChangeKind::Renamed,
        Err(_) => WorkspaceWatchChangeKind::Modified,
    }
}

pub(crate) fn v5_watch_path_to_workspace_path(workspace_root: &Path, path: &str) -> PathBuf {
    if path.is_empty() || path == "." {
        return workspace_root.to_path_buf();
    }
    let path = Path::new(path);
    if path.is_absolute() {
        normalize_path_lexically(path)
    } else {
        normalize_path_lexically(&workspace_root.join(path))
    }
}
