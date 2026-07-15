// ABOUTME: Native and polling workspace watch registration, batching, and resynchronization
// ABOUTME: Normalizes filesystem notifications into bounded protocol v5 watch events

use super::*;

#[derive(Default)]
pub(crate) struct V5WatchRegistry {
    next_watch_id: u64,
    pub(crate) subscriptions: HashMap<u64, V5WatchSubscription>,
    generations: protocol_v5::WatchGenerationTracker,
    native_events: Option<V5NativeWatchSender>,
}

pub(crate) struct V5WatchStartStatus {
    pub(crate) accepted_roots: Vec<String>,
    pub(crate) degraded_roots: Vec<String>,
    pub(crate) backend: String,
    pub(crate) degraded: bool,
}

pub(crate) struct V5WatchUpdateStatus {
    pub(crate) accepted_roots: Vec<String>,
    pub(crate) degraded_roots: Vec<String>,
}

pub(crate) struct V5WatchResyncStatus {
    pub(crate) accepted_roots: Vec<String>,
    pub(crate) unsupported_roots: Vec<String>,
}

pub(crate) struct V5WatchPendingBatch {
    event_stream_id: u64,
    watch_id: u64,
    changed_directories: Vec<String>,
    events: Vec<protocol_v5::WatchChange>,
    overflow: bool,
    resync_required: bool,
}

impl V5WatchRegistry {
    pub(crate) fn with_native_events(native_events: V5NativeWatchSender) -> Self {
        Self {
            native_events: Some(native_events),
            ..Self::default()
        }
    }

    pub(crate) fn allocate_watch_id(&mut self) -> Result<u64> {
        let watch_id = if self.next_watch_id == 0 {
            1
        } else {
            self.next_watch_id
        };
        self.next_watch_id = watch_id.checked_add(1).context("v5 watch id exhausted")?;
        Ok(watch_id)
    }

    pub(crate) fn has_active_watches(&self) -> bool {
        !self.subscriptions.is_empty()
    }

    pub(crate) fn next_poll_timeout(&self) -> Duration {
        let now = Instant::now();
        self.subscriptions
            .values()
            .filter_map(|subscription| subscription.next_due_at())
            .map(|due_at| due_at.saturating_duration_since(now))
            .min()
            .unwrap_or_else(|| Duration::from_secs(60))
    }

    pub(crate) fn start(
        &mut self,
        watch_id: u64,
        event_stream_id: u64,
        roots: Vec<String>,
        debounce_ms: u32,
        max_events_per_batch: u32,
        workspace_root: &Path,
    ) -> V5WatchStartStatus {
        let mut subscription = V5WatchSubscription::new(
            watch_id,
            event_stream_id,
            debounce_ms,
            max_events_per_batch,
            self.native_events.clone(),
        );
        for root in roots {
            subscription.add_root(root, workspace_root);
        }
        let status = V5WatchStartStatus {
            accepted_roots: subscription.accepted_roots(),
            degraded_roots: subscription.degraded_roots(),
            backend: subscription.backend_label(),
            degraded: subscription.is_degraded(),
        };
        self.subscriptions.insert(watch_id, subscription);
        status
    }

    pub(crate) fn update(
        &mut self,
        watch_id: u64,
        add_roots: Vec<String>,
        remove_roots: Vec<String>,
        workspace_root: &Path,
    ) -> std::result::Result<V5WatchUpdateStatus, RemoteError> {
        let Some(subscription) = self.subscriptions.get_mut(&watch_id) else {
            return Err(RemoteError {
                code: "not_found".to_string(),
                message: format!("unknown watch id {watch_id}"),
                diagnostic: None,
            });
        };
        for root in remove_roots {
            subscription.remove_root(&root, workspace_root);
        }
        for root in add_roots {
            subscription.add_root(root, workspace_root);
        }
        Ok(V5WatchUpdateStatus {
            accepted_roots: subscription.accepted_roots(),
            degraded_roots: subscription.degraded_roots(),
        })
    }

    pub(crate) fn stop(&mut self, watch_id: u64) -> Option<V5WatchSubscription> {
        self.subscriptions.remove(&watch_id)
    }

    pub(crate) fn resync(
        &mut self,
        watch_id: u64,
        requested_roots: Option<Vec<String>>,
    ) -> std::result::Result<V5WatchResyncStatus, RemoteError> {
        let Some(subscription) = self.subscriptions.get_mut(&watch_id) else {
            return Err(RemoteError {
                code: "not_found".to_string(),
                message: format!("unknown watch id {watch_id}"),
                diagnostic: None,
            });
        };
        Ok(subscription.force_resync(requested_roots))
    }

    pub(crate) fn record_native_event(
        &mut self,
        watch_id: u64,
        result: notify::Result<notify::Event>,
        workspace_root: &Path,
    ) -> Result<()> {
        let Some(subscription) = self.subscriptions.get_mut(&watch_id) else {
            return Ok(());
        };
        subscription.record_native_event(result, workspace_root);
        Ok(())
    }

    pub(crate) fn record_native_overflow(&mut self, watch_id: u64) {
        if let Some(subscription) = self.subscriptions.get_mut(&watch_id) {
            subscription.record_native_overflow();
        }
    }

    pub(crate) fn poll_due(
        &mut self,
        workspace_root: &Path,
    ) -> Result<Vec<(u64, protocol_v5::WatchBatch)>> {
        let now = Instant::now();
        let mut pending = Vec::new();
        for subscription in self.subscriptions.values_mut() {
            if let Some(batch) = subscription.take_due_batch(now, workspace_root) {
                pending.push(batch);
            }
        }
        let mut batches = Vec::with_capacity(pending.len());
        for batch in pending {
            let built = self.generations.build_batch(
                batch.watch_id,
                batch.changed_directories,
                batch.events,
                batch.overflow,
                batch.resync_required,
            )?;
            batches.push((batch.event_stream_id, built));
        }
        Ok(batches)
    }
}

pub(crate) struct V5NativeWatch {
    watcher: notify::RecommendedWatcher,
    roots: BTreeSet<String>,
}

impl V5NativeWatch {
    fn new(watch_id: u64, events: V5NativeWatchSender) -> notify::Result<Self> {
        let watcher = notify::recommended_watcher(move |result| {
            let _ = events.send(V5NativeWatchEvent { watch_id, result });
        })?;
        Ok(Self {
            watcher,
            roots: BTreeSet::new(),
        })
    }

    fn watch_root(&mut self, workspace_root: &Path, root: &str) -> bool {
        if self.roots.contains(root) {
            return true;
        }
        let path = v5_watch_root_path(workspace_root, root);
        match self
            .watcher
            .watch(&path, notify::RecursiveMode::NonRecursive)
        {
            Ok(()) => {
                self.roots.insert(root.to_string());
                true
            }
            Err(error) => {
                tracing::debug!(
                    root = %root,
                    path = %path.display(),
                    error = %error,
                    "Falling back to polling for v5 watch root"
                );
                false
            }
        }
    }

    fn unwatch_root(&mut self, workspace_root: &Path, root: &str) {
        if !self.roots.remove(root) {
            return;
        }
        let path = v5_watch_root_path(workspace_root, root);
        if let Err(error) = self.watcher.unwatch(&path) {
            tracing::debug!(
                root = %root,
                path = %path.display(),
                error = %error,
                "Failed to unwatch v5 native watch root"
            );
        }
    }

    fn has_roots(&self) -> bool {
        !self.roots.is_empty()
    }
}

pub(crate) struct V5WatchSubscription {
    watch_id: u64,
    pub(crate) event_stream_id: u64,
    pub(crate) roots: BTreeSet<String>,
    degraded_roots: BTreeSet<String>,
    fingerprints: HashMap<String, u64>,
    poll_interval: Duration,
    next_poll: Option<Instant>,
    pub(crate) next_emit: Option<Instant>,
    native: Option<V5NativeWatch>,
    pending_changed_directories: BTreeSet<String>,
    pub(crate) pending_events: Vec<protocol_v5::WatchChange>,
    max_events_per_batch: usize,
    pending_event_bytes: usize,
    pub(crate) pending_overflow: bool,
    pub(crate) pending_resync_required: bool,
}

impl V5WatchSubscription {
    pub(crate) fn new(
        watch_id: u64,
        event_stream_id: u64,
        debounce_ms: u32,
        max_events_per_batch: u32,
        native_events: Option<V5NativeWatchSender>,
    ) -> Self {
        let poll_interval = v5_watch_poll_interval(debounce_ms);
        let native = native_events.and_then(|events| match V5NativeWatch::new(watch_id, events) {
            Ok(watch) => Some(watch),
            Err(error) => {
                tracing::debug!(error = %error, "Native v5 file watching unavailable");
                None
            }
        });
        Self {
            watch_id,
            event_stream_id,
            roots: BTreeSet::new(),
            degraded_roots: BTreeSet::new(),
            fingerprints: HashMap::new(),
            poll_interval,
            next_poll: None,
            next_emit: None,
            native,
            pending_changed_directories: BTreeSet::new(),
            pending_events: Vec::new(),
            max_events_per_batch: v5_watch_event_limit(max_events_per_batch),
            pending_event_bytes: 0,
            pending_overflow: false,
            pending_resync_required: false,
        }
    }

    fn add_root(&mut self, root: String, workspace_root: &Path) {
        self.fingerprints.insert(
            root.clone(),
            v5_watch_root_fingerprint(workspace_root, &root),
        );
        self.roots.insert(root.clone());

        let watched_natively = self
            .native
            .as_mut()
            .is_some_and(|native| native.watch_root(workspace_root, &root));
        if watched_natively {
            self.degraded_roots.remove(&root);
        } else {
            self.degraded_roots.insert(root);
        }
        self.refresh_poll_timer();
    }

    fn remove_root(&mut self, root: &str, workspace_root: &Path) {
        self.roots.remove(root);
        self.degraded_roots.remove(root);
        self.fingerprints.remove(root);
        if let Some(native) = &mut self.native {
            native.unwatch_root(workspace_root, root);
        }
        self.pending_changed_directories.remove(root);
        self.refresh_poll_timer();
    }

    fn accepted_roots(&self) -> Vec<String> {
        self.roots.iter().cloned().collect()
    }

    fn degraded_roots(&self) -> Vec<String> {
        self.degraded_roots.iter().cloned().collect()
    }

    fn is_degraded(&self) -> bool {
        !self.degraded_roots.is_empty()
    }

    fn backend_label(&self) -> String {
        if self.native.as_ref().is_some_and(V5NativeWatch::has_roots) {
            if self.is_degraded() {
                "notify/poll"
            } else {
                "notify"
            }
        } else {
            "poll"
        }
        .to_string()
    }

    fn refresh_poll_timer(&mut self) {
        self.next_poll = if self.degraded_roots.is_empty() {
            None
        } else {
            Some(Instant::now() + self.poll_interval)
        };
    }

    fn next_due_at(&self) -> Option<Instant> {
        match (self.next_poll, self.next_emit) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(due), None) | (None, Some(due)) => Some(due),
            (None, None) => None,
        }
    }

    fn changed_degraded_roots(&mut self, workspace_root: &Path) -> Vec<String> {
        let mut changed = Vec::new();
        for root in self.degraded_roots.iter().cloned().collect::<Vec<_>>() {
            let fingerprint = v5_watch_root_fingerprint(workspace_root, &root);
            if self.fingerprints.get(&root).copied() != Some(fingerprint) {
                self.fingerprints.insert(root.clone(), fingerprint);
                changed.push(root);
            }
        }
        changed
    }

    fn force_resync(&mut self, requested_roots: Option<Vec<String>>) -> V5WatchResyncStatus {
        let requested_roots = requested_roots.unwrap_or_else(|| self.accepted_roots());
        let mut accepted_roots = BTreeSet::new();
        let mut unsupported_roots = BTreeSet::new();
        for root in requested_roots {
            if self.roots.contains(&root) {
                accepted_roots.insert(root);
            } else {
                unsupported_roots.insert(root);
            }
        }
        self.pending_changed_directories
            .extend(accepted_roots.iter().cloned());
        self.pending_resync_required = true;
        self.next_emit = Some(Instant::now());
        V5WatchResyncStatus {
            accepted_roots: accepted_roots.into_iter().collect(),
            unsupported_roots: unsupported_roots.into_iter().collect(),
        }
    }

    fn record_native_event(
        &mut self,
        result: notify::Result<notify::Event>,
        workspace_root: &Path,
    ) {
        match result {
            Ok(event) => self.record_notify_event(event, workspace_root),
            Err(error) => {
                tracing::debug!(error = %error, "Native v5 watch reported an error");
                self.degraded_roots.extend(self.roots.iter().cloned());
                self.refresh_poll_timer();
                self.pending_changed_directories
                    .extend(self.roots.iter().cloned());
                self.pending_overflow = true;
                self.pending_resync_required = true;
                self.schedule_emit();
            }
        }
    }

    pub(crate) fn record_native_overflow(&mut self) {
        tracing::debug!(
            watch_id = self.watch_id,
            "Native v5 watch event queue overflowed; requesting client reconciliation"
        );
        self.mark_overflow();
        self.schedule_emit();
    }

    fn record_notify_event(&mut self, event: notify::Event, workspace_root: &Path) {
        if self.roots.is_empty() {
            return;
        }

        if v5_notify_event_is_rename(&event)
            && event.paths.len() >= 2
            && let (Some(old_path), Some(new_path)) = (
                v5_watch_relative_path(workspace_root, &event.paths[0]),
                v5_watch_relative_path(workspace_root, &event.paths[1]),
            )
        {
            let is_dir = v5_notify_path_is_dir(&event.paths[1], &event.kind);
            self.record_changed_watch_path(&old_path);
            self.record_changed_watch_parent(&old_path);
            self.record_changed_watch_path(&new_path);
            self.record_changed_watch_parent(&new_path);
            self.record_watch_change(protocol_v5::WatchChange::renamed(
                old_path, new_path, is_dir,
            ));
            self.schedule_emit();
            return;
        }

        for path in event.paths {
            let Some(relative_path) = v5_watch_relative_path(workspace_root, &path) else {
                continue;
            };
            let is_dir = v5_notify_path_is_dir(&path, &event.kind);
            self.record_changed_watch_path(&relative_path);
            if is_dir {
                self.record_changed_watch_parent(&relative_path);
            }
            let change = match v5_notify_change_kind(&event.kind) {
                protocol_v5::WatchChangeKind::Created => {
                    protocol_v5::WatchChange::created(relative_path, is_dir)
                }
                protocol_v5::WatchChangeKind::Deleted => {
                    protocol_v5::WatchChange::deleted(relative_path, is_dir)
                }
                protocol_v5::WatchChangeKind::Renamed => {
                    protocol_v5::WatchChange::modified(relative_path, is_dir)
                }
                protocol_v5::WatchChangeKind::Modified => {
                    protocol_v5::WatchChange::modified(relative_path, is_dir)
                }
            };
            self.record_watch_change(change);
        }
        self.schedule_emit();
    }

    fn record_watch_change(&mut self, change: protocol_v5::WatchChange) {
        if self.pending_overflow {
            return;
        }
        let encoded_len = change.encoded_len().saturating_add(10);
        if self.pending_events.len() >= self.max_events_per_batch
            || self.pending_event_bytes.saturating_add(encoded_len) > V5_WATCH_BATCH_PAYLOAD_BUDGET
        {
            self.mark_overflow();
            return;
        }
        self.pending_event_bytes = self.pending_event_bytes.saturating_add(encoded_len);
        self.pending_events.push(change);
    }

    fn mark_overflow(&mut self) {
        self.pending_events.clear();
        self.pending_changed_directories.clear();
        self.pending_event_bytes = 0;
        self.pending_overflow = true;
        self.pending_resync_required = true;
    }

    fn record_changed_watch_path(&mut self, path: &str) {
        if let Some(root) = self.nearest_root_for_path(path) {
            self.pending_changed_directories.insert(root);
        }
    }

    fn record_changed_watch_parent(&mut self, path: &str) {
        if let Some(parent) = v5_watch_parent_path(path) {
            self.record_changed_watch_path(&parent);
        }
    }

    fn nearest_root_for_path(&self, path: &str) -> Option<String> {
        self.roots
            .iter()
            .filter(|root| v5_watch_root_contains(root, path))
            .max_by_key(|root| root.len())
            .cloned()
    }

    fn schedule_emit(&mut self) {
        if self.next_emit.is_none()
            && (!self.pending_events.is_empty()
                || !self.pending_changed_directories.is_empty()
                || self.pending_overflow
                || self.pending_resync_required)
        {
            self.next_emit = Some(Instant::now() + self.poll_interval);
        }
    }

    fn take_due_batch(
        &mut self,
        now: Instant,
        workspace_root: &Path,
    ) -> Option<V5WatchPendingBatch> {
        let mut changed_directories = BTreeSet::new();
        let mut events = Vec::new();
        let mut overflow = false;
        let mut resync_required = false;

        if self.next_emit.is_some_and(|due_at| due_at <= now) {
            self.next_emit = None;
            changed_directories.append(&mut self.pending_changed_directories);
            events.append(&mut self.pending_events);
            self.pending_event_bytes = 0;
            overflow = self.pending_overflow;
            resync_required = self.pending_resync_required;
            self.pending_overflow = false;
            self.pending_resync_required = false;
        }

        if self.next_poll.is_some_and(|due_at| due_at <= now) {
            self.next_poll = Some(now + self.poll_interval);
            let changed_roots = self.changed_degraded_roots(workspace_root);
            for root in changed_roots {
                changed_directories.insert(root.clone());
                events.push(protocol_v5::WatchChange::modified(root, true));
            }
        }

        if events.len() > self.max_events_per_batch
            || v5_watch_batch_payload_len(&changed_directories, &events)
                > V5_WATCH_BATCH_PAYLOAD_BUDGET
        {
            changed_directories.clear();
            events.clear();
            overflow = true;
            resync_required = true;
        }

        if changed_directories.is_empty() && events.is_empty() && !overflow && !resync_required {
            return None;
        }

        Some(V5WatchPendingBatch {
            event_stream_id: self.event_stream_id,
            watch_id: self.watch_id,
            changed_directories: changed_directories.into_iter().collect(),
            events,
            overflow,
            resync_required,
        })
    }
}

pub(crate) fn v5_watch_poll_interval(debounce_ms: u32) -> Duration {
    Duration::from_millis(u64::from(debounce_ms.clamp(50, 60_000)))
}

pub(crate) fn v5_watch_event_limit(requested: u32) -> usize {
    let requested = usize::try_from(requested).unwrap_or(V5_MAX_WATCH_EVENTS_PER_BATCH);
    if requested == 0 {
        V5_DEFAULT_WATCH_EVENTS_PER_BATCH
    } else {
        requested.min(V5_MAX_WATCH_EVENTS_PER_BATCH)
    }
}

pub(crate) fn v5_watch_batch_payload_len(
    changed_directories: &BTreeSet<String>,
    events: &[protocol_v5::WatchChange],
) -> usize {
    let generations = changed_directories
        .iter()
        .map(|path| path.len().saturating_add(32))
        .sum::<usize>();
    let events = events
        .iter()
        .map(|event| event.encoded_len().saturating_add(10))
        .sum::<usize>();
    64_usize.saturating_add(generations).saturating_add(events)
}

pub(crate) fn v5_watch_root_path(workspace_root: &Path, root: &str) -> PathBuf {
    if root == "." {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(root)
    }
}

pub(crate) fn v5_watch_relative_path(workspace_root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(workspace_root).ok()?;
    if relative.as_os_str().is_empty() {
        Some(".".to_string())
    } else {
        Some(posix_path_string(relative))
    }
}

pub(crate) fn v5_notify_event_is_rename(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
    )
}

pub(crate) fn v5_notify_change_kind(kind: &notify::EventKind) -> protocol_v5::WatchChangeKind {
    match kind {
        notify::EventKind::Create(_) => protocol_v5::WatchChangeKind::Created,
        notify::EventKind::Remove(_) => protocol_v5::WatchChangeKind::Deleted,
        notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
            protocol_v5::WatchChangeKind::Renamed
        }
        _ => protocol_v5::WatchChangeKind::Modified,
    }
}

pub(crate) fn v5_notify_path_is_dir(path: &Path, kind: &notify::EventKind) -> bool {
    path.is_dir()
        || matches!(
            kind,
            notify::EventKind::Create(notify::event::CreateKind::Folder)
                | notify::EventKind::Remove(notify::event::RemoveKind::Folder)
        )
}

pub(crate) fn v5_watch_parent_path(path: &str) -> Option<String> {
    if path == "." {
        None
    } else {
        Some(
            path.rsplit_once('/')
                .map(|(parent, _)| if parent.is_empty() { "." } else { parent })
                .unwrap_or(".")
                .to_string(),
        )
    }
}

pub(crate) fn v5_watch_root_contains(root: &str, path: &str) -> bool {
    root == "."
        || path == root
        || path
            .as_bytes()
            .get(root.len())
            .is_some_and(|separator| *separator == b'/')
            && path.starts_with(root)
}

pub(crate) fn v5_watch_root_fingerprint(workspace_root: &Path, root: &str) -> u64 {
    let path = v5_watch_root_path(workspace_root, root);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "v5-watch-root".hash(&mut hasher);
    root.hash(&mut hasher);

    let Ok(entries) = std::fs::read_dir(&path) else {
        "read_dir_error".hash(&mut hasher);
        return hasher.finish();
    };

    let mut entry_fingerprints = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                let name = entry.file_name().to_string_lossy().into_owned();
                let metadata = entry.metadata();
                let fingerprint = match metadata {
                    Ok(metadata) => {
                        let kind = if metadata.is_dir() {
                            "dir"
                        } else if metadata.is_file() {
                            "file"
                        } else {
                            "other"
                        };
                        let modified = metadata.modified().ok().and_then(|modified| {
                            modified
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
                        });
                        (name, kind.to_string(), metadata.len(), modified)
                    }
                    Err(error) => (name, format!("metadata_error:{:?}", error.kind()), 0, None),
                };
                entry_fingerprints.push(fingerprint);
            }
            Err(error) => {
                entry_fingerprints.push((
                    format!("read_entry_error:{:?}", error.kind()),
                    "error".to_string(),
                    0,
                    None,
                ));
            }
        }
    }
    entry_fingerprints.sort();
    entry_fingerprints.hash(&mut hasher);
    hasher.finish()
}
