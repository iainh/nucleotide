// ABOUTME: File tree UI view component using GPUI's uniform_list for performance
// ABOUTME: Handles user interaction, selection, and rendering of file tree entries

use crate::file_tree::watcher::FileTreeWatcher;
use crate::file_tree::{
    FILE_TREE_CONTEXT, FileSystemEventKind, FileTree, FileTreeCollisionStrategy, FileTreeConfig,
    FileTreeDisplayDensity, FileTreeEntry, FileTreeEvent,
    sidebar::{
        ProjectTreeContextMenuIntent, ProjectTreeDraggedEntry, ProjectTreeRow,
        ProjectTreeRowAction, ProjectTreeRowEvent, ProjectTreeRowStyle,
        project_tree_entry_min_width, project_tree_entry_min_width_with_vcs,
        render_project_tree_row,
    },
};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, Render, ScrollHandle, ScrollStrategy,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, div, px, uniform_list,
};
use nucleotide_logging::{debug, error, warn};
use nucleotide_types::{VcsStatus, scrollbar::SCROLLBAR_THICKNESS};
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
use nucleotide_ui::{Button, ButtonSize, ButtonVariant, Tooltipped};
use nucleotide_vcs::VcsServiceHandle;
use nucleotide_workspace::{
    DirectoryListing, FileKind as WorkspaceFileKind, WorkspaceBackendHandle, WorkspaceIdentity,
    WorkspaceWatchBatch, WorkspaceWatchChangeKind, WorkspaceWatchRequest,
    absolutize_workspace_path, local_workspace_backend,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

const REMOTE_FILE_TREE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const REMOTE_FILE_TREE_POLL_MAX_INTERVAL: std::time::Duration = std::time::Duration::from_secs(16);
const REMOTE_FILE_TREE_RECONCILIATION_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(60);
const REMOTE_FILE_TREE_IDLE_BACKOFF_AFTER_POLLS: u32 = 3;
const REMOTE_FILE_TREE_RESYNC_RETRY_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryListingFingerprint {
    entries: Vec<DirectoryEntryFingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DirectoryEntryFingerprintKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DirectoryEntryFingerprint {
    path: PathBuf,
    kind: DirectoryEntryFingerprintKind,
    size: u64,
    modified: Option<std::time::SystemTime>,
    symlink_target: Option<PathBuf>,
    target_exists: Option<bool>,
    ignored: Option<bool>,
}

#[derive(Clone)]
struct FileTreePresentationCache {
    tree_revision: u64,
    density: FileTreeDisplayDensity,
    vcs_revision: u64,
    entry_count: usize,
    vcs_statuses: Arc<[Option<VcsStatus>]>,
    width_measure_item_index: Option<usize>,
    content_width: f32,
}

fn cached_file_tree_presentation(
    cache: Option<&FileTreePresentationCache>,
    tree_revision: u64,
    density: FileTreeDisplayDensity,
    vcs_revision: u64,
    entry_count: usize,
) -> Option<FileTreePresentationCache> {
    cache
        .filter(|cache| {
            cache.tree_revision == tree_revision
                && cache.density == density
                && cache.vcs_revision == vcs_revision
                && cache.entry_count == entry_count
        })
        .cloned()
}

struct RemoteDirectoryPollResult {
    path: PathBuf,
    listing: Result<DirectoryListing, String>,
}

struct RemoteWatchRefreshPlan {
    workspace_backend: WorkspaceBackendHandle,
    directories: Vec<PathBuf>,
    watch_id: u64,
    sequence: u64,
    epoch: u64,
    requires_resync: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteDirectoryPollApplication {
    Stale,
    Complete,
    Incomplete(Vec<PathBuf>),
}

#[derive(Debug, Clone, PartialEq)]
struct RemoteFileSystemChange {
    path: PathBuf,
    kind: FileSystemEventKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileTreeScrollOffset {
    Top,
    Center,
    #[default]
    Nearest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileTreeScrollToPathOptions {
    pub focus: bool,
    pub offset: FileTreeScrollOffset,
}

impl Default for FileTreeScrollToPathOptions {
    fn default() -> Self {
        Self {
            focus: true,
            offset: FileTreeScrollOffset::Nearest,
        }
    }
}

fn should_focus_editor_for_project_tree_open(click_count: usize) -> bool {
    click_count > 1
}

fn scroll_strategy_for_file_tree_offset(offset: FileTreeScrollOffset) -> ScrollStrategy {
    match offset {
        FileTreeScrollOffset::Top => ScrollStrategy::Top,
        FileTreeScrollOffset::Center => ScrollStrategy::Center,
        FileTreeScrollOffset::Nearest => ScrollStrategy::Nearest,
    }
}

fn scroll_file_tree_index(
    scroll_handle: &UniformListScrollHandle,
    index: usize,
    offset: FileTreeScrollOffset,
) {
    let strategy = scroll_strategy_for_file_tree_offset(offset);
    match offset {
        FileTreeScrollOffset::Top | FileTreeScrollOffset::Center => {
            scroll_handle.scroll_to_item_strict(index, strategy);
        }
        FileTreeScrollOffset::Nearest => {
            scroll_handle.scroll_to_item(index, strategy);
        }
    }
}

fn widest_project_tree_entry_index(
    entries: &[FileTreeEntry],
    density: FileTreeDisplayDensity,
) -> Option<usize> {
    widest_project_tree_entry_index_in_range(entries, 0..entries.len(), density)
}

fn widest_project_tree_entry_index_in_range(
    entries: &[FileTreeEntry],
    range: std::ops::Range<usize>,
    density: FileTreeDisplayDensity,
) -> Option<usize> {
    let start = range.start;
    entries
        .get(range)?
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| {
            project_tree_entry_min_width(left, density)
                .total_cmp(&project_tree_entry_min_width(right, density))
        })
        .map(|(range_index, _)| start + range_index)
}

fn project_tree_content_width(
    entries: &[FileTreeEntry],
    density: FileTreeDisplayDensity,
    mut vcs_status: impl FnMut(&FileTreeEntry) -> Option<VcsStatus>,
) -> f32 {
    entries
        .iter()
        .map(|entry| project_tree_entry_min_width_with_vcs(entry, density, vcs_status(entry)))
        .fold(0.0_f32, f32::max)
}

fn rebase_file_tree_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    path.strip_prefix(from).ok().map(|relative| {
        if relative.as_os_str().is_empty() {
            to.to_path_buf()
        } else {
            to.join(relative)
        }
    })
}

fn file_tree_drop_destination(from: &Path, target_dir: &Path) -> Option<PathBuf> {
    from.file_name().map(|file_name| target_dir.join(file_name))
}

fn listing_fingerprint(listing: &DirectoryListing) -> DirectoryListingFingerprint {
    let mut entries = listing
        .entries
        .iter()
        .map(|entry| DirectoryEntryFingerprint {
            path: entry.path.clone(),
            kind: workspace_file_kind_fingerprint(entry.stat.kind),
            size: entry.stat.size,
            modified: entry.stat.modified,
            symlink_target: entry.symlink_target.clone(),
            target_exists: entry.target_exists,
            ignored: entry.ignored,
        })
        .collect::<Vec<_>>();
    entries.sort();
    DirectoryListingFingerprint { entries }
}

fn tree_entries_fingerprint(entries: Vec<FileTreeEntry>) -> DirectoryListingFingerprint {
    let mut entries = entries
        .into_iter()
        .map(|entry| DirectoryEntryFingerprint {
            path: entry.path.clone(),
            kind: file_tree_entry_kind_fingerprint(&entry),
            size: entry.size,
            modified: entry.mtime,
            symlink_target: match &entry.kind {
                crate::file_tree::FileKind::Symlink { target, .. } => target.clone(),
                _ => None,
            },
            target_exists: match &entry.kind {
                crate::file_tree::FileKind::Symlink { target_exists, .. } => Some(*target_exists),
                _ => None,
            },
            ignored: Some(entry.is_ignored),
        })
        .collect::<Vec<_>>();
    entries.sort();
    DirectoryListingFingerprint { entries }
}

fn changed_paths_between_fingerprints(
    previous: Option<&DirectoryListingFingerprint>,
    current: &DirectoryListingFingerprint,
) -> Vec<PathBuf> {
    let mut changed = BTreeSet::new();
    let Some(previous) = previous else {
        return current
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
    };

    let previous_entries = previous
        .entries
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect::<BTreeMap<_, _>>();
    let current_entries = current
        .entries
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect::<BTreeMap<_, _>>();

    for (path, current_entry) in &current_entries {
        if previous_entries.get(path).copied() != Some(*current_entry) {
            changed.insert((*path).clone());
        }
    }

    for path in previous_entries.keys() {
        if !current_entries.contains_key(path) {
            changed.insert((*path).clone());
        }
    }

    changed.into_iter().collect()
}

fn file_system_changes_between_fingerprints(
    previous: Option<&DirectoryListingFingerprint>,
    current: &DirectoryListingFingerprint,
) -> Vec<RemoteFileSystemChange> {
    let Some(previous) = previous else {
        return Vec::new();
    };

    let previous_entries = previous
        .entries
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect::<BTreeMap<_, _>>();
    let current_entries = current
        .entries
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect::<BTreeMap<_, _>>();

    let mut changes = Vec::new();
    for (path, current_entry) in &current_entries {
        match previous_entries.get(path).copied() {
            Some(previous_entry) if previous_entry == *current_entry => {}
            Some(_) => changes.push(RemoteFileSystemChange {
                path: (*path).clone(),
                kind: FileSystemEventKind::Modified,
            }),
            None => changes.push(RemoteFileSystemChange {
                path: (*path).clone(),
                kind: FileSystemEventKind::Created,
            }),
        }
    }

    for path in previous_entries.keys() {
        if !current_entries.contains_key(path) {
            changes.push(RemoteFileSystemChange {
                path: (*path).clone(),
                kind: FileSystemEventKind::Deleted,
            });
        }
    }

    changes.sort_by(|left, right| left.path.cmp(&right.path));
    changes
}

fn workspace_file_kind_fingerprint(kind: WorkspaceFileKind) -> DirectoryEntryFingerprintKind {
    match kind {
        WorkspaceFileKind::File => DirectoryEntryFingerprintKind::File,
        WorkspaceFileKind::Directory => DirectoryEntryFingerprintKind::Directory,
        WorkspaceFileKind::Symlink => DirectoryEntryFingerprintKind::Symlink,
        WorkspaceFileKind::Other => DirectoryEntryFingerprintKind::Other,
    }
}

fn file_tree_entry_kind_fingerprint(entry: &FileTreeEntry) -> DirectoryEntryFingerprintKind {
    match &entry.kind {
        crate::file_tree::FileKind::File { .. } => DirectoryEntryFingerprintKind::File,
        crate::file_tree::FileKind::Directory { .. } => DirectoryEntryFingerprintKind::Directory,
        crate::file_tree::FileKind::Symlink { .. } => DirectoryEntryFingerprintKind::Symlink,
    }
}

async fn poll_remote_directory_listings(
    workspace_backend: WorkspaceBackendHandle,
    directories: Vec<PathBuf>,
) -> Vec<RemoteDirectoryPollResult> {
    workspace_backend
        .list_dirs(directories)
        .await
        .into_iter()
        .map(|(path, listing)| RemoteDirectoryPollResult {
            path,
            listing: listing.map_err(|error| error.to_string()),
        })
        .collect()
}

fn insert_nearest_expanded_remote_watch_directory(
    directories: &mut BTreeSet<PathBuf>,
    expanded_set: &BTreeSet<PathBuf>,
    path: &Path,
) {
    if let Some(directory) = path
        .ancestors()
        .find(|ancestor| expanded_set.contains(*ancestor))
    {
        directories.insert(directory.to_path_buf());
    }
}

fn remote_file_tree_poll_interval(idle_polls: u32) -> std::time::Duration {
    if idle_polls < REMOTE_FILE_TREE_IDLE_BACKOFF_AFTER_POLLS {
        return REMOTE_FILE_TREE_POLL_INTERVAL;
    }

    let backoff_steps = idle_polls
        .saturating_sub(REMOTE_FILE_TREE_IDLE_BACKOFF_AFTER_POLLS)
        .min(3);
    let seconds = REMOTE_FILE_TREE_POLL_INTERVAL
        .as_secs()
        .saturating_mul(1_u64 << backoff_steps)
        .min(REMOTE_FILE_TREE_POLL_MAX_INTERVAL.as_secs());
    std::time::Duration::from_secs(seconds)
}

/// File tree view component
pub struct FileTreeView {
    /// The underlying file tree data
    tree: FileTree,
    /// Currently selected entry path
    selected_path: Option<PathBuf>,
    /// Full set of selected entry paths, ordered for stable events.
    selected_paths: BTreeSet<PathBuf>,
    /// Focus handle for keyboard navigation
    focus_handle: FocusHandle,
    /// Scroll handle for the list
    scroll_handle: UniformListScrollHandle,
    /// Horizontal scroll state for content wider than the sidebar.
    horizontal_scroll_handle: ScrollHandle,
    /// Vertical scrollbar state for managing token-aware scrollbar UI
    vertical_scrollbar_state: ScrollbarState,
    /// Tokio runtime handle for async VCS operations
    _tokio_handle: Option<tokio::runtime::Handle>,
    /// File system watcher for detecting changes
    file_watcher: Option<FileTreeWatcher>,
    /// Workspace backend used for directory listing and initial tree load.
    workspace_backend: WorkspaceBackendHandle,
    /// Pending file system events for debouncing
    pending_fs_events: std::collections::HashMap<PathBuf, FileTreeEvent>,
    /// Last file system event time for debouncing
    last_fs_event_time: Option<std::time::Instant>,
    /// Whether remote directory polling is currently active.
    remote_file_polling_active: bool,
    /// Generation of the active remote polling task.
    remote_file_polling_generation: u64,
    /// Consecutive remote poll iterations that found no file tree changes.
    remote_file_poll_idle_ticks: u32,
    /// Last seen fingerprints for expanded remote directories.
    remote_directory_fingerprints: std::collections::HashMap<PathBuf, DirectoryListingFingerprint>,
    /// Whether a remote workspace watch task is active or currently starting.
    remote_file_watch_active: bool,
    /// Active remote watch identifier, once the helper accepts watch.start.
    remote_file_watch_id: Option<u64>,
    /// Directories currently registered with the remote watch.
    remote_file_watch_roots: BTreeSet<PathBuf>,
    /// Last applied remote watch sequence, used to discard stale batches.
    remote_file_watch_last_sequence: u64,
    /// Sequence currently being refreshed before it can be marked as applied.
    remote_file_watch_pending_sequence: Option<u64>,
    /// Whether the pending watch refresh is a mandatory full resync barrier.
    remote_file_watch_resync_pending: bool,
    /// Invalidates directory poll results captured before a watch resync or replacement.
    remote_file_watch_epoch: u64,
    /// Whether the initial tree load is running in the background.
    initial_load_in_flight: bool,
    /// Monotonic revision for structural tree changes.
    tree_revision: u64,
    presentation_cache: Option<FileTreePresentationCache>,
    presentation_cache_hits: u64,
    presentation_cache_misses: u64,
}

impl FileTreeView {
    /// Create a new file tree view
    pub fn new(root_path: PathBuf, config: FileTreeConfig, cx: &mut Context<Self>) -> Self {
        let workspace_backend = local_workspace_backend();
        let mut tree = FileTree::new_for_backend(root_path, config, workspace_backend.identity());

        // Load initial tree structure
        if let Err(e) = tree.load_with_backend(workspace_backend.as_ref()) {
            error!(error = %e, "Failed to load file tree");
        }

        let scroll_handle = UniformListScrollHandle::new();
        let horizontal_scroll_handle = ScrollHandle::new();
        let vertical_scrollbar_state = ScrollbarState::new(scroll_handle.clone());
        let focus_handle = cx.focus_handle();
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
            coord.set_file_tree_focus(focus_handle.clone());
        }

        let mut instance = Self {
            tree,
            selected_path: None,
            selected_paths: BTreeSet::new(),
            focus_handle,
            scroll_handle,
            horizontal_scroll_handle,
            vertical_scrollbar_state,
            _tokio_handle: None,
            file_watcher: None,
            workspace_backend,
            pending_fs_events: std::collections::HashMap::new(),
            last_fs_event_time: None,
            remote_file_polling_active: false,
            remote_file_polling_generation: 0,
            remote_file_poll_idle_ticks: 0,
            remote_directory_fingerprints: std::collections::HashMap::new(),
            remote_file_watch_active: false,
            remote_file_watch_id: None,
            remote_file_watch_roots: BTreeSet::new(),
            remote_file_watch_last_sequence: 0,
            remote_file_watch_pending_sequence: None,
            remote_file_watch_resync_pending: false,
            remote_file_watch_epoch: 0,
            initial_load_in_flight: false,
            tree_revision: 0,
            presentation_cache: None,
            presentation_cache_hits: 0,
            presentation_cache_misses: 0,
        };

        // Auto-select the first entry if there are any entries
        instance.select_first_visible_entry();

        instance
    }

    /// Create a new file tree view with Tokio runtime handle for VCS operations
    pub fn new_with_runtime(
        root_path: PathBuf,
        config: FileTreeConfig,
        tokio_handle: Option<tokio::runtime::Handle>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_runtime_and_backend(
            root_path,
            config,
            tokio_handle,
            local_workspace_backend(),
            cx,
        )
    }

    /// Create a new file tree view with a workspace backend.
    pub fn new_with_runtime_and_backend(
        root_path: PathBuf,
        config: FileTreeConfig,
        tokio_handle: Option<tokio::runtime::Handle>,
        workspace_backend: WorkspaceBackendHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree =
            FileTree::new_for_backend(root_path.clone(), config, workspace_backend.identity());

        let scroll_handle = UniformListScrollHandle::new();
        let horizontal_scroll_handle = ScrollHandle::new();
        let vertical_scrollbar_state = ScrollbarState::new(scroll_handle.clone());
        let focus_handle = cx.focus_handle();
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
            coord.set_file_tree_focus(focus_handle.clone());
        }

        let mut instance = Self {
            tree,
            selected_path: None,
            selected_paths: BTreeSet::new(),
            focus_handle,
            scroll_handle,
            horizontal_scroll_handle,
            vertical_scrollbar_state,
            _tokio_handle: tokio_handle,
            file_watcher: None,
            workspace_backend,
            pending_fs_events: std::collections::HashMap::new(),
            last_fs_event_time: None,
            remote_file_polling_active: false,
            remote_file_polling_generation: 0,
            remote_file_poll_idle_ticks: 0,
            remote_directory_fingerprints: std::collections::HashMap::new(),
            remote_file_watch_active: false,
            remote_file_watch_id: None,
            remote_file_watch_roots: BTreeSet::new(),
            remote_file_watch_last_sequence: 0,
            remote_file_watch_pending_sequence: None,
            remote_file_watch_resync_pending: false,
            remote_file_watch_epoch: 0,
            initial_load_in_flight: false,
            tree_revision: 0,
            presentation_cache: None,
            presentation_cache_hits: 0,
            presentation_cache_misses: 0,
        };

        instance.start_initial_load(cx);

        // VCS monitoring will be handled by the global VCS service
        // The file tree will query VCS status at render time via get_vcs_status_for_entry

        instance
    }

    fn select_first_visible_entry(&mut self) {
        let entries = self.tree.visible_entries();
        if let Some(entry) = entries.first() {
            let path = entry.path.clone();
            self.selected_path = Some(path.clone());
            self.selected_paths.insert(path);
        }
    }

    fn start_initial_load(&mut self, cx: &mut Context<Self>) {
        if self.initial_load_in_flight {
            return;
        }

        self.initial_load_in_flight = true;
        self.tree_revision = self.tree_revision.wrapping_add(1);
        let load_revision = self.tree_revision;
        let root_path = self.tree.root_path().to_path_buf();
        let config = self.tree.config().clone();
        let workspace_backend = self.workspace_backend.clone();
        let runtime_handle = self._tokio_handle.clone();

        cx.spawn(async move |this, cx| {
            let load_result = if let Some(runtime_handle) = runtime_handle {
                match runtime_handle
                    .spawn(async move {
                        let mut tree = FileTree::new_for_backend(
                            root_path,
                            config,
                            workspace_backend.identity(),
                        );
                        tree.load_with_backend_async(workspace_backend)
                            .await
                            .map(|_| tree)
                    })
                    .await
                {
                    Ok(result) => result,
                    Err(error) => Err(anyhow::anyhow!("file tree load task failed: {error}")),
                }
            } else {
                cx.background_executor()
                    .spawn(async move {
                        let mut tree = FileTree::new_for_backend(
                            root_path,
                            config,
                            workspace_backend.identity(),
                        );
                        tree.load_with_backend(workspace_backend.as_ref())
                            .map(|_| tree)
                    })
                    .await
            };

            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    view.initial_load_in_flight = false;

                    if view.tree_revision != load_revision {
                        debug!(
                            current_revision = view.tree_revision,
                            load_revision,
                            "Ignoring stale initial file tree load"
                        );
                        return;
                    }

                    match load_result {
                        Ok(tree) => {
                            let root_path = tree.root_path().to_path_buf();
                            let watch_filesystem = tree.config().watch_filesystem;
                            let previous_selected_path = view.selected_path.clone();
                            let previous_selected_paths = view.selected_paths.clone();

                            view.tree = tree;
                            view.restore_selection_after_tree_replace(
                                previous_selected_path,
                                previous_selected_paths,
                            );

                            if watch_filesystem
                                && matches!(view.workspace_backend.identity(), WorkspaceIdentity::Local)
                            {
                                debug!(root_path = ?root_path, "Attempting to create file system watcher");
                                match FileTreeWatcher::new(root_path.clone()) {
                                    Ok(watcher) => {
                                        debug!(root_path = ?root_path, "File system watcher created successfully");
                                        view.file_watcher = Some(watcher);
                                        view.start_file_watcher(cx);
                                    }
                                    Err(error) => {
                                        warn!(
                                            error = %error,
                                            root_path = ?root_path,
                                            "Failed to create file system watcher"
                                        );
                                    }
                                }
                            } else {
                                debug!(
                                    backend = ?view.workspace_backend.identity(),
                                    watch_filesystem,
                                    "File system watching disabled for file tree"
                                );
                            }

                            if view.should_poll_remote_filesystem() {
                                view.start_remote_file_watching(cx);
                            }
                        }
                        Err(error) => {
                            error!(error = %error, "Failed to load file tree");
                        }
                    }

                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn should_poll_remote_filesystem(&self) -> bool {
        self.tree.config().watch_filesystem
            && matches!(
                self.workspace_backend.identity(),
                WorkspaceIdentity::Remote(_)
            )
    }

    fn should_watch_remote_filesystem(&self) -> bool {
        self.should_poll_remote_filesystem()
    }

    fn remote_poll_interval(&self) -> std::time::Duration {
        if self.remote_file_watch_id.is_some() {
            REMOTE_FILE_TREE_RECONCILIATION_INTERVAL
        } else {
            remote_file_tree_poll_interval(self.remote_file_poll_idle_ticks)
        }
    }

    fn start_remote_file_watching(&mut self, cx: &mut Context<Self>) {
        if self.remote_file_watch_active || !self.should_watch_remote_filesystem() {
            return;
        }

        let roots = self.tree.expanded_directory_paths();
        if roots.is_empty() {
            return;
        }

        self.remote_file_watch_active = true;
        self.remote_file_watch_roots = roots.iter().cloned().collect();
        self.remote_file_watch_last_sequence = 0;
        self.invalidate_remote_file_watch_refreshes();
        let workspace_backend = self.workspace_backend.clone();
        debug!(
            root_path = %self.tree.root_path().display(),
            roots = roots.len(),
            "Starting remote file tree watch"
        );

        cx.spawn(async move |this, cx| {
            let watch_result = cx
                .background_executor()
                .spawn(async move {
                    workspace_backend
                        .start_watch(WorkspaceWatchRequest::expanded_dirs(roots))
                        .await
                })
                .await;

            let Some(entity) = this.upgrade() else {
                return;
            };

            let watch = match watch_result {
                Ok(Some(watch)) => {
                    let watch_id = watch.watch_id;
                    entity.update(cx, |view, cx| {
                        view.invalidate_remote_file_watch_refreshes();
                        view.remote_file_watch_id = Some(watch_id);
                        view.seed_remote_directory_fingerprints();
                        view.start_remote_file_polling(cx);
                    });
                    watch
                }
                Ok(None) => {
                    entity.update(cx, |view, cx| {
                        debug!("Remote workspace backend does not support file watching; using polling");
                        view.invalidate_remote_file_watch_refreshes();
                        view.remote_file_watch_active = false;
                        view.remote_file_watch_id = None;
                        view.remote_file_watch_roots.clear();
                        view.seed_remote_directory_fingerprints();
                        view.start_remote_file_polling(cx);
                    });
                    return;
                }
                Err(error) => {
                    entity.update(cx, |view, cx| {
                        warn!(error = %error, "Failed to start remote file tree watch; using polling");
                        view.invalidate_remote_file_watch_refreshes();
                        view.remote_file_watch_active = false;
                        view.remote_file_watch_id = None;
                        view.remote_file_watch_roots.clear();
                        view.seed_remote_directory_fingerprints();
                        view.start_remote_file_polling(cx);
                    });
                    return;
                }
            };

            loop {
                let watch_for_recv = watch.clone();
                let batch_result = cx
                    .background_executor()
                    .spawn(async move {
                        watch_for_recv.recv_timeout(std::time::Duration::from_secs(30))
                    })
                    .await;

                match batch_result {
                    Ok(batch) => {
                        let Some(entity) = this.upgrade() else {
                            break;
                        };
                        let plan = entity.update(cx, |view, _cx| {
                            view.remote_watch_refresh_plan(&batch)
                        });
                        if let Some(plan) = plan {
                            let mut directories = plan.directories.clone();
                            loop {
                                let requested_directories = directories;
                                let results = cx
                                    .background_executor()
                                    .spawn(poll_remote_directory_listings(
                                        plan.workspace_backend.clone(),
                                        requested_directories.clone(),
                                    ))
                                    .await;
                                let Some(entity) = this.upgrade() else {
                                    return;
                                };
                                let application = entity.update(cx, |view, cx| {
                                    view.apply_remote_directory_poll_results(
                                        results,
                                        plan.epoch,
                                        &requested_directories,
                                        plan.requires_resync,
                                        cx,
                                    )
                                });

                                match application {
                                    RemoteDirectoryPollApplication::Stale => break,
                                    RemoteDirectoryPollApplication::Complete => {
                                        entity.update(cx, |view, _cx| {
                                            view.finish_remote_watch_refresh(&plan, true);
                                        });
                                        break;
                                    }
                                    RemoteDirectoryPollApplication::Incomplete(retry_directories)
                                        if plan.requires_resync =>
                                    {
                                        directories = retry_directories;
                                        cx.background_executor()
                                            .timer(REMOTE_FILE_TREE_RESYNC_RETRY_INTERVAL)
                                            .await;
                                    }
                                    RemoteDirectoryPollApplication::Incomplete(_) => {
                                        entity.update(cx, |view, _cx| {
                                            view.finish_remote_watch_refresh(&plan, false);
                                        });
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        if let Some(entity) = this.upgrade() {
                            entity.update(cx, |view, cx| {
                                view.restart_remote_file_polling_after_watch_disconnect(cx);
                            });
                        }
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn sync_remote_file_watch_roots(&mut self, cx: &mut Context<Self>) {
        if !self.should_watch_remote_filesystem() {
            self.invalidate_remote_file_watch_refreshes();
            self.remote_file_watch_active = false;
            self.remote_file_watch_id = None;
            self.remote_file_watch_roots.clear();
            return;
        }

        let Some(watch_id) = self.remote_file_watch_id else {
            self.start_remote_file_watching(cx);
            return;
        };

        let current_roots = self
            .tree
            .expanded_directory_paths()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let add_roots = current_roots
            .difference(&self.remote_file_watch_roots)
            .cloned()
            .collect::<Vec<_>>();
        let remove_roots = self
            .remote_file_watch_roots
            .difference(&current_roots)
            .cloned()
            .collect::<Vec<_>>();
        if add_roots.is_empty() && remove_roots.is_empty() {
            return;
        }

        self.remote_file_watch_roots = current_roots;
        let workspace_backend = self.workspace_backend.clone();
        cx.spawn(async move |_this, cx| {
            let update_result = cx
                .background_executor()
                .spawn(async move {
                    workspace_backend
                        .update_watch(watch_id, add_roots, remove_roots)
                        .await
                })
                .await;
            if let Err(error) = update_result {
                warn!(error = %error, watch_id, "Failed to update remote file tree watch roots");
            }
        })
        .detach();
    }

    fn remote_watch_refresh_plan(
        &mut self,
        batch: &WorkspaceWatchBatch,
    ) -> Option<RemoteWatchRefreshPlan> {
        if !self.should_watch_remote_filesystem() {
            self.invalidate_remote_file_watch_refreshes();
            self.remote_file_watch_active = false;
            self.remote_file_watch_id = None;
            self.remote_file_watch_roots.clear();
            return None;
        }
        if self.remote_file_watch_id != Some(batch.watch_id) {
            return None;
        }
        if batch.sequence <= self.remote_file_watch_last_sequence {
            return None;
        }
        if self.remote_file_watch_pending_sequence.is_some() {
            return None;
        }

        let requires_resync = batch.overflow || batch.resync_required;
        if requires_resync {
            self.remote_file_watch_epoch = self.remote_file_watch_epoch.wrapping_add(1);
            self.remote_directory_fingerprints.clear();
        }
        self.remote_file_watch_pending_sequence = Some(batch.sequence);
        self.remote_file_watch_resync_pending = requires_resync;

        let expanded = self.tree.expanded_directory_paths();
        let expanded_set = expanded.iter().cloned().collect::<BTreeSet<_>>();
        let mut directories = BTreeSet::new();
        if requires_resync {
            directories.extend(expanded);
        } else {
            for generation in &batch.directory_generations {
                if expanded_set.contains(&generation.path) {
                    directories.insert(generation.path.clone());
                }
            }
            for event in &batch.events {
                insert_nearest_expanded_remote_watch_directory(
                    &mut directories,
                    &expanded_set,
                    &event.path,
                );
                if event.is_dir
                    && matches!(
                        event.kind,
                        WorkspaceWatchChangeKind::Created
                            | WorkspaceWatchChangeKind::Deleted
                            | WorkspaceWatchChangeKind::Renamed
                    )
                    && let Some(parent) = event.path.parent()
                {
                    insert_nearest_expanded_remote_watch_directory(
                        &mut directories,
                        &expanded_set,
                        parent,
                    );
                }
                if let Some(old_path) = &event.old_path {
                    insert_nearest_expanded_remote_watch_directory(
                        &mut directories,
                        &expanded_set,
                        old_path,
                    );
                    if event.is_dir
                        && let Some(parent) = old_path.parent()
                    {
                        insert_nearest_expanded_remote_watch_directory(
                            &mut directories,
                            &expanded_set,
                            parent,
                        );
                    }
                }
            }
        }

        if directories.is_empty() {
            self.remote_file_watch_last_sequence = batch.sequence;
            self.remote_file_watch_pending_sequence = None;
            self.remote_file_watch_resync_pending = false;
            None
        } else {
            self.reset_remote_file_poll_backoff();
            Some(RemoteWatchRefreshPlan {
                workspace_backend: self.workspace_backend.clone(),
                directories: directories.into_iter().collect(),
                watch_id: batch.watch_id,
                sequence: batch.sequence,
                epoch: self.remote_file_watch_epoch,
                requires_resync,
            })
        }
    }

    fn finish_remote_watch_refresh(&mut self, plan: &RemoteWatchRefreshPlan, applied: bool) {
        if self.remote_file_watch_id != Some(plan.watch_id)
            || self.remote_file_watch_epoch != plan.epoch
            || self.remote_file_watch_pending_sequence != Some(plan.sequence)
        {
            return;
        }

        if applied {
            self.remote_file_watch_last_sequence = plan.sequence;
        }
        self.remote_file_watch_pending_sequence = None;
        self.remote_file_watch_resync_pending = false;
    }

    fn invalidate_remote_file_watch_refreshes(&mut self) {
        self.remote_file_watch_epoch = self.remote_file_watch_epoch.wrapping_add(1);
        self.remote_file_watch_pending_sequence = None;
        self.remote_file_watch_resync_pending = false;
    }

    fn seed_remote_directory_fingerprints(&mut self) {
        self.reset_remote_file_poll_backoff();
        let expanded = self.tree.expanded_directory_paths();
        self.remote_directory_fingerprints
            .retain(|path, _| expanded.contains(path));

        for directory in expanded {
            let children = self.tree.immediate_child_entries(&directory);
            self.remote_directory_fingerprints
                .insert(directory, tree_entries_fingerprint(children));
        }
    }

    fn seed_remote_directory_fingerprint(&mut self, directory: &Path) {
        self.reset_remote_file_poll_backoff();
        let children = self.tree.immediate_child_entries(directory);
        self.remote_directory_fingerprints
            .insert(directory.to_path_buf(), tree_entries_fingerprint(children));
    }

    fn reset_remote_file_poll_backoff(&mut self) {
        self.remote_file_poll_idle_ticks = 0;
    }

    fn restart_remote_file_polling_after_watch_disconnect(&mut self, cx: &mut Context<Self>) {
        self.invalidate_remote_file_watch_refreshes();
        self.remote_file_watch_active = false;
        self.remote_file_watch_id = None;
        self.remote_file_watch_roots.clear();
        self.remote_file_polling_active = false;
        self.reset_remote_file_poll_backoff();
        self.start_remote_file_polling(cx);
    }

    fn start_remote_file_polling(&mut self, cx: &mut Context<Self>) {
        if self.remote_file_polling_active || !self.should_poll_remote_filesystem() {
            return;
        }

        self.remote_file_polling_active = true;
        self.remote_file_polling_generation = self.remote_file_polling_generation.wrapping_add(1);
        let polling_generation = self.remote_file_polling_generation;
        debug!(
            root_path = %self.tree.root_path().display(),
            "Starting remote file tree polling"
        );

        cx.spawn(async move |this, cx| {
            loop {
                let Some(entity) = this.upgrade() else {
                    break;
                };

                let interval = entity.update(cx, |view, _cx| {
                    if view.remote_file_polling_generation != polling_generation {
                        return None;
                    }
                    if !view.should_poll_remote_filesystem() {
                        view.remote_file_polling_active = false;
                        view.remote_file_poll_idle_ticks = 0;
                        view.remote_directory_fingerprints.clear();
                        return None;
                    }

                    Some(view.remote_poll_interval())
                });

                let Some(interval) = interval else {
                    break;
                };

                cx.background_executor().timer(interval).await;

                let Some(entity) = this.upgrade() else {
                    break;
                };

                let poll_plan = entity.update(cx, |view, _cx| {
                    if view.remote_file_polling_generation != polling_generation {
                        return None;
                    }
                    if !view.should_poll_remote_filesystem() {
                        view.remote_file_polling_active = false;
                        view.remote_file_poll_idle_ticks = 0;
                        view.remote_directory_fingerprints.clear();
                        return None;
                    }

                    Some((
                        view.workspace_backend.clone(),
                        if view.remote_file_watch_resync_pending {
                            Vec::new()
                        } else {
                            view.tree.expanded_directory_paths()
                        },
                        view.remote_file_watch_epoch,
                    ))
                });

                let Some((workspace_backend, directories, watch_epoch)) = poll_plan else {
                    break;
                };

                if directories.is_empty() {
                    continue;
                }

                let requested_directories = directories;
                let results = cx
                    .background_executor()
                    .spawn(poll_remote_directory_listings(
                        workspace_backend,
                        requested_directories.clone(),
                    ))
                    .await;

                if let Some(entity) = this.upgrade() {
                    entity.update(cx, |view, cx| {
                        view.apply_remote_directory_poll_results(
                            results,
                            watch_epoch,
                            &requested_directories,
                            false,
                            cx,
                        );
                    });
                } else {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_remote_directory_poll_results(
        &mut self,
        results: Vec<RemoteDirectoryPollResult>,
        expected_watch_epoch: u64,
        requested_directories: &[PathBuf],
        require_all_expanded: bool,
        cx: &mut Context<Self>,
    ) -> RemoteDirectoryPollApplication {
        if !self.should_poll_remote_filesystem() {
            self.remote_file_polling_active = false;
            self.remote_file_poll_idle_ticks = 0;
            self.remote_directory_fingerprints.clear();
            return RemoteDirectoryPollApplication::Stale;
        }
        if self.remote_file_watch_epoch != expected_watch_epoch {
            return RemoteDirectoryPollApplication::Stale;
        }

        let expanded = self.tree.expanded_directory_paths();
        let expanded_set = expanded.iter().cloned().collect::<BTreeSet<_>>();
        self.remote_directory_fingerprints
            .retain(|path, _| expanded.contains(path));
        let mut incomplete_directories = if require_all_expanded {
            expanded_set.clone()
        } else {
            requested_directories
                .iter()
                .filter(|path| expanded_set.contains(*path))
                .cloned()
                .collect::<BTreeSet<_>>()
        };

        let mut changed_paths = Vec::new();
        for result in results {
            if !expanded_set.contains(&result.path) {
                continue;
            }

            let listing = match result.listing {
                Ok(listing) => listing,
                Err(error) => {
                    warn!(
                        path = %result.path.display(),
                        error = %error,
                        "Failed to poll remote file tree directory"
                    );
                    continue;
                }
            };

            let fingerprint = listing_fingerprint(&listing);
            let previous_fingerprint = self.remote_directory_fingerprints.get(&result.path);
            if previous_fingerprint == Some(&fingerprint) {
                incomplete_directories.remove(&result.path);
                continue;
            }
            let changed_child_paths =
                changed_paths_between_fingerprints(previous_fingerprint, &fingerprint);
            let file_system_changes =
                file_system_changes_between_fingerprints(previous_fingerprint, &fingerprint);

            match self
                .tree
                .refresh_directory_with_listing(&result.path, listing)
            {
                Ok(()) => {
                    self.remote_directory_fingerprints
                        .insert(result.path.clone(), fingerprint);
                    incomplete_directories.remove(&result.path);
                    if changed_child_paths.is_empty() {
                        changed_paths.push(result.path);
                    } else {
                        changed_paths.extend(changed_child_paths);
                    }
                    for change in file_system_changes {
                        cx.emit(FileTreeEvent::FileSystemChanged {
                            path: change.path,
                            kind: change.kind,
                        });
                    }
                }
                Err(error) => {
                    warn!(
                        path = %result.path.display(),
                        error = %error,
                        "Failed to apply remote file tree directory refresh"
                    );
                }
            }
        }

        if !changed_paths.is_empty() {
            self.reset_remote_file_poll_backoff();
            self.tree_revision = self.tree_revision.wrapping_add(1);
            self.refresh_vcs_for_file_system_changes(&changed_paths, cx);
            cx.notify();
        } else {
            self.remote_file_poll_idle_ticks = self.remote_file_poll_idle_ticks.saturating_add(1);
        }

        if incomplete_directories.is_empty() {
            RemoteDirectoryPollApplication::Complete
        } else {
            RemoteDirectoryPollApplication::Incomplete(if require_all_expanded {
                expanded
            } else {
                incomplete_directories.into_iter().collect()
            })
        }
    }

    fn restore_selection_after_tree_replace(
        &mut self,
        previous_selected_path: Option<PathBuf>,
        previous_selected_paths: BTreeSet<PathBuf>,
    ) {
        self.selected_paths = previous_selected_paths
            .into_iter()
            .filter(|path| self.tree.entry_by_path(path).is_some())
            .collect();
        self.selected_path = previous_selected_path
            .filter(|path| self.tree.entry_by_path(path).is_some())
            .or_else(|| self.selected_paths.iter().next().cloned());

        if self.selected_path.is_none() {
            self.select_first_visible_entry();
        } else if let Some(selected_path) = &self.selected_path {
            self.selected_paths.insert(selected_path.clone());
        }
    }

    /// Get the current selection
    pub fn selected_path(&self) -> Option<&PathBuf> {
        self.selected_path.as_ref()
    }

    /// Return whether the tree's current primary selection is a directory.
    pub fn selected_path_is_directory(&self) -> bool {
        self.selected_path
            .as_deref()
            .and_then(|path| self.tree.entry_by_path(path))
            .is_some_and(|entry| entry.is_directory())
    }

    /// Get all currently selected paths.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_paths.iter().cloned().collect()
    }

    /// Get the current file-tree search query.
    pub fn search_query(&self) -> Option<&str> {
        self.tree.search_query()
    }

    /// Update file-tree configuration and redraw with the new rendering settings.
    pub fn set_config(&mut self, config: FileTreeConfig, cx: &mut Context<Self>) {
        self.tree_revision = self.tree_revision.wrapping_add(1);
        self.tree.set_config(config);
        if self.should_poll_remote_filesystem() {
            self.start_remote_file_watching(cx);
        } else {
            self.invalidate_remote_file_watch_refreshes();
            self.remote_file_polling_active = false;
            self.remote_file_poll_idle_ticks = 0;
            self.remote_directory_fingerprints.clear();
            self.remote_file_watch_active = false;
            self.remote_file_watch_id = None;
            self.remote_file_watch_roots.clear();
        }
        cx.notify();
    }

    /// Return whether the tree knows about this path.
    pub fn contains_path(&self, path: &Path) -> bool {
        self.tree.entry_by_path(path).is_some()
    }

    /// Set the selection
    pub fn select_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let path = path.map(|path| self.canonical_selection_path(path));
        let mut selected_paths = BTreeSet::new();
        if let Some(path) = &path {
            selected_paths.insert(path.clone());
        }
        self.apply_selection(path, selected_paths, cx);
    }

    /// Select a single path and clear any other selected paths.
    pub fn select_only_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.select_path(Some(path), cx);
    }

    /// Add a path to the selected set and make it the primary selection.
    pub fn select_additional_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let path = self.canonical_selection_path(path);
        let mut selected_paths = self.selected_paths.clone();
        selected_paths.insert(path.clone());
        self.apply_selection(Some(path), selected_paths, cx);
    }

    /// Remove a path from the selected set.
    pub fn deselect_path(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let path = self.canonical_selection_path(path.to_path_buf());
        if !self.selected_paths.contains(&path) {
            return false;
        }

        let mut selected_paths = self.selected_paths.clone();
        selected_paths.remove(&path);
        let selected_path = if self.selected_path.as_ref() == Some(&path) {
            selected_paths.iter().next().cloned()
        } else {
            self.selected_path.clone()
        };

        self.apply_selection(selected_path, selected_paths, cx);
        true
    }

    /// Toggle whether a path is selected.
    pub fn toggle_path_selection(&mut self, path: PathBuf, cx: &mut Context<Self>) -> bool {
        let path = self.canonical_selection_path(path);
        let mut selected_paths = self.selected_paths.clone();

        let selected_path = if selected_paths.contains(&path) {
            selected_paths.remove(&path);
            if self.selected_path.as_ref() == Some(&path) {
                selected_paths.iter().next().cloned()
            } else {
                self.selected_path.clone()
            }
        } else {
            selected_paths.insert(path.clone());
            Some(path)
        };

        self.apply_selection(selected_path, selected_paths, cx);
        true
    }

    /// Select the inclusive range between two currently visible paths.
    pub fn select_path_range(
        &mut self,
        anchor: &Path,
        target: &Path,
        cx: &mut Context<Self>,
    ) -> bool {
        self.select_visible_path_range(anchor, target, false, cx)
    }

    /// Add the inclusive range between two currently visible paths to the selection set.
    pub fn add_path_range_to_selection(
        &mut self,
        anchor: &Path,
        target: &Path,
        cx: &mut Context<Self>,
    ) -> bool {
        self.select_visible_path_range(anchor, target, true, cx)
    }

    fn canonical_selection_path(&self, path: PathBuf) -> PathBuf {
        self.tree
            .entry_by_path(&path)
            .map(|entry| entry.path)
            .unwrap_or(path)
    }

    fn apply_selection(
        &mut self,
        selected_path: Option<PathBuf>,
        selected_paths: BTreeSet<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let primary_changed = self.selected_path != selected_path;
        let set_changed = self.selected_paths != selected_paths;

        if !primary_changed && !set_changed {
            return;
        }

        self.selected_path = selected_path.clone();
        self.selected_paths = selected_paths;

        if primary_changed {
            cx.emit(FileTreeEvent::SelectionChanged {
                path: selected_path,
            });
        }

        if set_changed {
            cx.emit(FileTreeEvent::SelectionSetChanged {
                paths: self.selected_paths(),
            });
        }

        cx.notify();
    }

    fn select_visible_path_range(
        &mut self,
        anchor: &Path,
        target: &Path,
        union: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let entries = self.tree.visible_entries();
        let Some(anchor_index) = entries.iter().position(|entry| entry.path == anchor) else {
            return false;
        };
        let Some(target_index) = entries.iter().position(|entry| entry.path == target) else {
            return false;
        };

        let mut selected_paths = if union {
            self.selected_paths.clone()
        } else {
            BTreeSet::new()
        };
        let range_start = anchor_index.min(target_index);
        let range_end = anchor_index.max(target_index);
        for entry in entries[range_start..=range_end].iter() {
            selected_paths.insert(entry.path.clone());
        }

        self.apply_selection(Some(target.to_path_buf()), selected_paths, cx);
        true
    }

    /// Open the workspace-owned search prompt for the file tree.
    pub fn request_search(&mut self, cx: &mut Context<Self>) {
        cx.emit(FileTreeEvent::SearchRequested {
            initial_query: self.search_query().map(ToOwned::to_owned),
        });
    }

    /// Set the search query and keep selection on a visible row.
    pub fn set_search_query(&mut self, query: Option<String>, cx: &mut Context<Self>) {
        self.tree.set_search_query(query);
        self.tree_revision = self.tree_revision.wrapping_add(1);
        self.select_valid_search_row(cx);
        cx.notify();
    }

    /// Clear the search query.
    pub fn clear_search_query(&mut self, cx: &mut Context<Self>) {
        self.tree.clear_search_query();
        self.tree_revision = self.tree_revision.wrapping_add(1);
        self.select_valid_search_row(cx);
        cx.notify();
    }

    /// Collapse every expanded directory below the workspace root.
    pub fn collapse_all_directories(&mut self, cx: &mut Context<Self>) {
        let root_path = self.tree.root_path().to_path_buf();
        let expanded_paths: Vec<_> = self
            .tree
            .visible_entries_snapshot()
            .iter()
            .filter(|entry| {
                entry.path != root_path
                    && entry.is_directory()
                    && self.tree.is_expanded(&entry.path)
            })
            .map(|entry| entry.path.clone())
            .collect();

        for path in expanded_paths.into_iter().rev() {
            let _ = self.tree.collapse_directory(&path);
        }
        self.tree_revision = self.tree_revision.wrapping_add(1);
        self.sync_remote_file_watch_roots(cx);
        cx.notify();
    }

    /// Select the next visible search match.
    pub fn select_next_search_match(&mut self, cx: &mut Context<Self>) {
        self.select_relative_search_match(1, cx);
    }

    /// Select the previous visible search match.
    pub fn select_previous_search_match(&mut self, cx: &mut Context<Self>) {
        self.select_relative_search_match(-1, cx);
    }

    /// Scroll a currently visible path into view.
    pub fn scroll_to_path(
        &mut self,
        path: &Path,
        options: FileTreeScrollToPathOptions,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target_entry) = self.tree.entry_by_path(path) else {
            return false;
        };

        let entries = self.tree.visible_entries();
        let Some(index) = entries
            .iter()
            .position(|entry| entry.path == target_entry.path)
        else {
            return false;
        };

        if options.focus {
            self.select_path(Some(target_entry.path), cx);
        }

        scroll_file_tree_index(&self.scroll_handle, index, options.offset);
        cx.notify();

        true
    }

    fn select_valid_search_row(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        let search_active = self.tree.search_query().is_some();
        let current_entry = self.selected_path.as_ref().and_then(|selected| {
            entries
                .iter()
                .find(|entry| &entry.path == selected)
                .cloned()
        });

        if current_entry
            .as_ref()
            .is_some_and(|entry| !search_active || entry.is_search_match)
        {
            return;
        }

        let next_selection = entries
            .iter()
            .find(|entry| entry.is_search_match)
            .or_else(|| entries.first())
            .map(|entry| entry.path.clone());

        self.select_path(next_selection, cx);
    }

    fn select_relative_search_match(&mut self, direction: isize, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.is_search_match)
            .map(|entry| entry.path.clone())
            .collect();

        if matches.is_empty() {
            return;
        }

        let current_index = self
            .selected_path
            .as_ref()
            .and_then(|selected| matches.iter().position(|path| path == selected));
        let next_index = match (current_index, direction.is_negative()) {
            (Some(index), false) => (index + 1) % matches.len(),
            (Some(0), true) => matches.len() - 1,
            (Some(index), true) => index - 1,
            (None, false) => 0,
            (None, true) => matches.len() - 1,
        };

        self.select_path(Some(matches[next_index].clone()), cx);
    }

    /// Sync selection with the currently open file
    pub fn sync_selection_with_file(&mut self, file_path: Option<&Path>, cx: &mut Context<Self>) {
        if let Some(path) = file_path {
            // Only update if the path exists in the tree
            if self.tree.entry_by_path(path).is_some() {
                self.select_path(Some(path.to_path_buf()), cx);

                // Ensure parent directories are expanded so the file is visible
                if let Some(parent) = path.parent() {
                    self.ensure_path_visible(parent, cx);
                }
            }
        }
    }

    /// Ensure a path is visible by expanding parent directories
    fn ensure_path_visible(&mut self, path: &Path, cx: &mut Context<Self>) {
        // Start from the root and expand directories along the path
        let mut current = PathBuf::new();

        for component in path.components() {
            current.push(component);

            if let Some(entry) = self.tree.entry_by_path(&current)
                && entry.is_directory()
                && !self.tree.is_expanded(&current)
            {
                // Expand this directory using toggle_directory
                self.toggle_directory(&current, cx);
            }
        }

        cx.notify();
    }

    /// Toggle directory expansion
    pub fn toggle_directory(&mut self, path: &Path, cx: &mut Context<Self>) {
        // Check if we're already loading this directory
        if self.tree.is_directory_loading(path) {
            return;
        }

        let path_buf = path.to_path_buf();
        let is_expanded = self.tree.is_expanded(path);

        if is_expanded {
            // Collapse is synchronous
            if let Err(e) = self.tree.collapse_directory(path) {
                error!(path = %path.display(), error = %e, "Failed to collapse directory");
            } else {
                self.tree_revision = self.tree_revision.wrapping_add(1);
                self.remote_directory_fingerprints.remove(path);
                self.reset_remote_file_poll_backoff();
                cx.emit(FileTreeEvent::DirectoryToggled {
                    path: path.to_path_buf(),
                    expanded: false,
                });
                self.sync_remote_file_watch_roots(cx);
                cx.notify();
            }
        } else {
            // Mark directory as loading to prevent double-clicks
            self.tree.mark_directory_loading(path);
            cx.notify();

            // Expand is asynchronous - spawn background task
            let path_for_io = path_buf.clone();
            let workspace_backend = self.workspace_backend.clone();
            cx.spawn(async move |this, cx| {
                let listing = cx
                    .background_executor()
                    .spawn(async move { workspace_backend.list_dir(&path_for_io).await })
                    .await;

                // Update the UI on the main thread
                if let Some(this) = this.upgrade() {
                    this.update(cx, |view, cx| {
                        match listing {
                            Ok(listing) => {
                                if let Err(e) =
                                    view.tree.expand_directory_with_listing(&path_buf, listing)
                                {
                                    error!(
                                        directory = %path_buf.display(),
                                        error = %e,
                                        "Failed to expand directory"
                                    );
                                } else {
                                    view.tree_revision = view.tree_revision.wrapping_add(1);
                                    if view.should_poll_remote_filesystem() {
                                        view.seed_remote_directory_fingerprint(&path_buf);
                                        view.sync_remote_file_watch_roots(cx);
                                        view.start_remote_file_polling(cx);
                                    }
                                    cx.emit(FileTreeEvent::DirectoryToggled {
                                        path: path_buf.clone(),
                                        expanded: true,
                                    });
                                }
                            }
                            Err(e) => {
                                error!(
                                    directory = %path_buf.display(),
                                    error = %e,
                                    "Failed to read directory"
                                );
                                view.tree.unmark_directory_loading(&path_buf);
                            }
                        }

                        // VCS status is handled by the global VCS service

                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    /// Open the selected file
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path.clone()
            && let Some(entry) = self.tree.entry_by_path(&path)
        {
            if entry.is_file() {
                cx.emit(FileTreeEvent::OpenFile {
                    path,
                    focus_editor: false,
                });
            } else if entry.is_directory() {
                self.toggle_directory(&path, cx);
            }
        }
    }

    /// Request a common project-tree operation for the selected entry.
    pub fn request_selected_operation(
        &mut self,
        intent: ProjectTreeContextMenuIntent,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.selected_path.clone() else {
            return;
        };

        let Some(entry) = self.tree.entry_by_path(&path) else {
            return;
        };

        cx.emit(FileTreeEvent::OperationRequested {
            intent,
            path,
            is_directory: entry.is_directory(),
        });
    }

    /// Request deletion for the selected entry. Workspace owns confirmation and execution.
    pub fn request_delete_selected(&mut self, cx: &mut Context<Self>) {
        self.request_selected_operation(ProjectTreeContextMenuIntent::Delete, cx);
    }

    /// Select next entry
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        if entries.is_empty() {
            return;
        }

        // If no selection, start with first entry
        if self.selected_path.is_none() {
            self.select_path(Some(entries[0].path.clone()), cx);
            return;
        }

        let current_index = self
            .selected_path
            .as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
            .unwrap_or(0);

        let next_index = (current_index + 1).min(entries.len() - 1);
        self.select_path(Some(entries[next_index].path.clone()), cx);
    }

    /// Select previous entry
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        debug!(
            entry_count = entries.len(),
            "select_previous: visible entries"
        );
        if entries.is_empty() {
            debug!("select_previous: No entries available");
            return;
        }

        // If no selection, start with first entry
        if self.selected_path.is_none() {
            debug!("select_previous: No selection, selecting first entry");
            self.select_path(Some(entries[0].path.clone()), cx);
            return;
        }

        let current_index = self
            .selected_path
            .as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
            .unwrap_or(0);

        debug!(
            current_index = current_index,
            selected_path = ?self.selected_path,
            "select_previous: current state"
        );
        let prev_index = current_index.saturating_sub(1);
        debug!(
            from_index = current_index,
            to_index = prev_index,
            "select_previous: moving selection"
        );
        self.select_path(Some(entries[prev_index].path.clone()), cx);
    }

    /// Select first entry
    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        if let Some(first) = entries.first() {
            self.select_path(Some(first.path.clone()), cx);
        }
    }

    /// Select last entry
    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        if let Some(last) = entries.last() {
            self.select_path(Some(last.path.clone()), cx);
        }
    }

    /// Handle left arrow key navigation
    pub fn navigate_left(&mut self, cx: &mut Context<Self>) {
        if let Some(current_path) = self.selected_path.clone()
            && let Some(current_entry) = self.tree.entry_by_path(&current_path)
        {
            if current_entry.is_directory() && current_entry.is_expanded() {
                // Collapse the current directory if it's expanded
                self.toggle_directory(&current_path, cx);
            } else {
                // Navigate to parent directory
                if let Some(parent_entry) = self.tree.find_parent_entry(&current_path) {
                    self.select_path(Some(parent_entry.path), cx);
                }
            }
        }
    }

    /// Handle right arrow key navigation  
    pub fn navigate_right(&mut self, cx: &mut Context<Self>) {
        if let Some(current_path) = self.selected_path.clone()
            && let Some(current_entry) = self.tree.entry_by_path(&current_path)
            && current_entry.is_directory()
        {
            if !current_entry.is_expanded() {
                // Expand the current directory if it's collapsed
                self.toggle_directory(&current_path, cx);
            } else {
                // Navigate to first child if already expanded
                if let Some(first_child) = self.tree.find_first_child_entry(&current_path) {
                    self.select_path(Some(first_child.path), cx);
                }
            }
        }
        // For files, right arrow does nothing
    }

    /// Refresh the tree
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.start_initial_load(cx);
    }

    /// Refresh a single directory by rescanning its entries and expanding it
    pub fn refresh_directory(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let dir = dir.to_path_buf();
        let workspace_backend = self.workspace_backend.clone();

        cx.spawn(async move |this, cx| {
            let dir_for_io = dir.clone();
            let listing = cx
                .background_executor()
                .spawn(async move { workspace_backend.list_dir(&dir_for_io).await })
                .await;

            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    match listing {
                        Ok(listing) => {
                            if let Err(e) = view.tree.refresh_directory_with_listing(&dir, listing)
                            {
                                error!(path=%dir.display(), error=%e, "Failed to refresh directory entries");
                            } else {
                                view.tree_revision = view.tree_revision.wrapping_add(1);
                                if view.should_poll_remote_filesystem() {
                                    view.seed_remote_directory_fingerprint(&dir);
                                    view.sync_remote_file_watch_roots(cx);
                                    view.start_remote_file_polling(cx);
                                }
                            }
                        }
                        Err(e) => {
                            error!(path=%dir.display(), error=%e, "Failed to read directory during refresh");
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Handle VCS refresh request - now uses centralized VCS service
    pub fn handle_vcs_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        debug!(force = force, "VCS refresh requested");

        // Use centralized VCS service instead of file tree's own VCS logic
        let root_path = self.tree.root_path().to_path_buf();
        let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();

        vcs_handle.update(cx, |service, cx| {
            // Start monitoring if not already
            if service.root_path() != Some(&root_path) {
                service.start_monitoring(root_path.clone(), cx);
            } else if force {
                // Force refresh the VCS status
                service.force_refresh(cx);
            }
        });

        // Update file tree entries with current VCS status
        self.update_entries_with_vcs_status(cx);
    }

    /// Update file tree entries with VCS status from the centralized service
    fn update_entries_with_vcs_status(&mut self, cx: &mut Context<Self>) {
        // Now that we use the centralized VCS service, we don't need to update entries directly
        // Instead, entries will query VCS status during rendering via get_vcs_status_for_entry
        debug!("VCS status update requested - will be handled during render");
        cx.notify(); // Trigger re-render to pick up new VCS status
    }

    /// Handle file system change event (should trigger VCS refresh)
    pub fn handle_file_system_change(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        debug!(path = ?path, "File system change detected");

        // Only refresh VCS if the change is within our repository
        let root_path = self.tree.root_path().to_path_buf();
        if path.starts_with(&root_path) {
            debug!("File system change is within repository, triggering VCS refresh");
            self.refresh_vcs_for_file_system_changes(&[path.to_path_buf()], cx);
        }
    }

    /// Trigger manual VCS refresh by emitting RefreshVcs event
    pub fn request_vcs_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        debug!(force = force, "Manual VCS refresh requested");
        cx.emit(FileTreeEvent::RefreshVcs { force });
    }

    /// Get tree statistics
    pub fn stats(&self) -> crate::file_tree::tree::FileTreeStats {
        self.tree.stats()
    }

    /// Start the file system watcher background task
    fn start_file_watcher(&mut self, cx: &mut Context<Self>) {
        if let Some(mut watcher) = self.file_watcher.take() {
            debug!("Starting file system watcher background task");

            cx.spawn(async move |this, cx| {
                while let Some(event) = watcher.next_event().await {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |view, cx| {
                            view.queue_file_system_event(event, cx);
                        });
                    } else {
                        // Component was dropped, stop watching
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// Queue a file system event for debounced processing
    fn queue_file_system_event(&mut self, event: FileTreeEvent, cx: &mut Context<Self>) {
        if let FileTreeEvent::FileSystemChanged { path, kind } = &event {
            debug!(path = ?path, kind = ?kind, "Queuing file system event");

            // Immediate debug: check if root is expanded
            if let Some(parent) = path.parent() {
                let is_expanded = self.tree.is_expanded(parent);
                debug!(parent = ?parent, is_expanded = is_expanded, "Parent directory expansion status");
            }

            // Coalesce with any pending event for this path to avoid losing deletes/renames
            if let Some(prev) = self.pending_fs_events.get(path) {
                let merged = Self::merge_fs_events(prev, &event);
                debug!(prev = ?prev, new = ?event, merged = ?merged, "Coalesced FS event");
                self.pending_fs_events.insert(path.clone(), merged);
            } else {
                self.pending_fs_events.insert(path.clone(), event);
            }
            self.last_fs_event_time = Some(std::time::Instant::now());

            // Schedule a debounced processing after 300ms
            cx.spawn(async move |this, cx| {
                // Wait for debounce period
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(300))
                    .await;

                if let Some(this) = this.upgrade() {
                    this.update(cx, |view, cx| {
                        view.process_pending_events(cx);
                    });
                }
            })
            .detach();
        }
    }

    /// Process pending file system events
    fn process_pending_events(&mut self, cx: &mut Context<Self>) {
        debug!("Processing pending file system events");

        // Check if enough time has passed since the last event
        if let Some(last_time) = self.last_fs_event_time
            && last_time.elapsed() < std::time::Duration::from_millis(300)
        {
            debug!("Debounce time not elapsed yet, skipping processing");
            return;
        }

        // Collect events to process to avoid borrowing issues
        let events_to_process: Vec<_> = self.pending_fs_events.drain().collect();
        debug!(
            event_count = events_to_process.len(),
            "Processing {} pending events",
            events_to_process.len()
        );

        // Process all collected events
        let mut vcs_changed_paths = Vec::new();
        for (_, event) in events_to_process {
            debug!("Processing event: {:?}", event);
            vcs_changed_paths.extend(Self::vcs_paths_for_file_system_event(&event));
            self.handle_file_system_event(event, cx);
        }

        self.refresh_vcs_for_file_system_changes(&vcs_changed_paths, cx);
        self.last_fs_event_time = None;
    }

    fn vcs_paths_for_file_system_event(event: &FileTreeEvent) -> Vec<PathBuf> {
        match event {
            FileTreeEvent::FileSystemChanged { path, kind } => match kind {
                crate::file_tree::FileSystemEventKind::Renamed { from, to } => {
                    vec![from.clone(), to.clone()]
                }
                _ => vec![path.clone()],
            },
            _ => Vec::new(),
        }
    }

    fn refresh_vcs_for_file_system_changes(
        &mut self,
        changed_paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        if changed_paths.is_empty() {
            return;
        }

        let root_path = self.tree.root_path().to_path_buf();
        let changed_paths = Self::vcs_changed_paths_for_root(&root_path, changed_paths);

        if changed_paths.is_empty() {
            return;
        }

        debug!(
            change_count = changed_paths.len(),
            "Triggering VCS refresh for filesystem changes"
        );

        let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();
        vcs_handle.update(cx, |service, cx| {
            if service.root_path() != Some(root_path.as_path()) {
                service.start_monitoring(root_path, cx);
            } else {
                service.refresh_after_file_system_changes(&changed_paths, cx);
            }
        });

        self.update_entries_with_vcs_status(cx);
    }

    fn vcs_changed_paths_for_root(root_path: &Path, changed_paths: &[PathBuf]) -> Vec<PathBuf> {
        changed_paths
            .iter()
            .map(|path| absolutize_workspace_path(root_path, path))
            .filter(|path| path.starts_with(root_path))
            .collect()
    }

    /// Handle a file system event and update the tree structure
    fn handle_file_system_event(&mut self, event: FileTreeEvent, cx: &mut Context<Self>) {
        if let FileTreeEvent::FileSystemChanged { path, kind } = &event {
            debug!(path = ?path, kind = ?kind, "Handling file system event");

            use crate::file_tree::FileSystemEventKind;
            match kind {
                FileSystemEventKind::Created => {
                    self.handle_file_created(path, cx);
                }
                FileSystemEventKind::Deleted => {
                    self.handle_file_deleted(path, cx);
                }
                FileSystemEventKind::Modified => {
                    self.handle_file_modified(path, cx);
                }
                FileSystemEventKind::Renamed { from, to } => {
                    self.handle_file_renamed(from, to, cx);
                }
            }

            // Emit the event to workspace for further handling (VCS refresh, etc.)
            cx.emit(event);
        }
    }

    /// Handle file/directory creation
    fn handle_file_created(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        debug!(path = ?path, "Handling file creation");

        // Check if the parent directory is expanded and visible
        if let Some(parent) = path.parent() {
            // Only add if parent directory is expanded (visible in tree)
            if self.tree.is_expanded(parent) {
                debug!(parent = ?parent, "Parent directory is expanded, adding new entry");

                // Create the new entry
                if let Ok(metadata) = std::fs::metadata(path) {
                    let entry = self.create_tree_entry(path, &metadata, parent);

                    // Add the entry to the tree
                    self.tree.upsert_entry(entry);
                    debug!(path = ?path, "Successfully added new entry to tree");

                    cx.notify();
                } else {
                    debug!(path = ?path, "Failed to get metadata for created file");
                }
            } else {
                debug!(parent = ?parent, "Parent directory not expanded, skipping file addition");
            }
        } else {
            debug!(path = ?path, "No parent directory found for created file");
        }
    }

    /// Create a tree entry from path and metadata  
    fn create_tree_entry(
        &mut self,
        path: &PathBuf,
        metadata: &std::fs::Metadata,
        parent: &std::path::Path,
    ) -> crate::file_tree::FileTreeEntry {
        use crate::file_tree::FileTreeEntry;

        let id = self.tree.next_entry_id();
        let mtime = metadata.modified().ok();

        // Determine depth based on parent
        let parent_depth = if let Some(parent_entry) = self.tree.entry_by_path(parent) {
            parent_entry.depth
        } else {
            0
        };

        let mut entry = if metadata.is_dir() {
            FileTreeEntry::new_directory(id, path.clone(), mtime)
        } else if metadata.is_file() {
            FileTreeEntry::new_file(id, path.clone(), metadata.len(), mtime)
        } else {
            // Symlink
            let target = std::fs::read_link(path).ok();
            let target_exists = target.as_ref().map(|t| t.exists()).unwrap_or(false);
            FileTreeEntry::new_symlink(id, path.clone(), target, target_exists, mtime)
        };

        entry.depth = parent_depth + 1;
        entry.is_visible = true;

        // VCS status will be retrieved during rendering from centralized service
        entry.git_status = None;

        entry
    }

    /// Get VCS status for a specific file path using the centralized service
    fn get_vcs_status_for_entry(&self, path: &Path, cx: &Context<Self>) -> Option<VcsStatus> {
        let vcs_service = cx.global::<VcsServiceHandle>();
        vcs_service.get_status(path, cx)
    }

    fn resolve_presentation_cache(
        &mut self,
        entries: &[FileTreeEntry],
        density: FileTreeDisplayDensity,
        vcs_revision: u64,
        cx: &Context<Self>,
    ) -> FileTreePresentationCache {
        if let Some(cache) = cached_file_tree_presentation(
            self.presentation_cache.as_ref(),
            self.tree_revision,
            density,
            vcs_revision,
            entries.len(),
        ) {
            self.presentation_cache_hits += 1;
            return cache;
        }

        self.presentation_cache_misses += 1;
        let vcs_statuses: Arc<[Option<VcsStatus>]> = entries
            .iter()
            .map(|entry| self.get_vcs_status_for_entry(&entry.path, cx))
            .collect::<Vec<_>>()
            .into();
        let width_measure_item_index = widest_project_tree_entry_index(entries, density);
        let mut statuses = vcs_statuses.iter().copied();
        let content_width =
            project_tree_content_width(entries, density, |_| statuses.next().flatten());
        let cache = FileTreePresentationCache {
            tree_revision: self.tree_revision,
            density,
            vcs_revision,
            entry_count: entries.len(),
            vcs_statuses,
            width_measure_item_index,
            content_width,
        };
        self.presentation_cache = Some(cache.clone());
        cache
    }

    /// Handle file/directory deletion  
    fn handle_file_deleted(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        debug!(path = ?path, "Handling file deletion");

        // Remove the entry from the tree
        if self.tree.remove_entry(path).is_some() {
            let fallback_selection = self
                .tree
                .visible_entries()
                .first()
                .map(|entry| entry.path.clone());
            self.remove_selection_under_path(path, fallback_selection, cx);
            cx.notify();
        }
    }

    /// Handle file modification
    fn handle_file_modified(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        debug!(path = ?path, "Handling file modification");

        // If it no longer exists on disk, treat as deletion
        if !path.exists() {
            debug!(path = ?path, "Modified event but path missing on disk; treating as deletion");
            self.handle_file_deleted(path, cx);
            return;
        }

        // Check if this file exists in the tree
        if let Some(_existing_entry) = self.tree.entry_by_path(path) {
            // File exists in tree - this is a genuine modification
            debug!(path = ?path, "File exists in tree, treating as modification");
            cx.notify();
        } else {
            // File doesn't exist in tree - this might be a new file creation
            // that was reported as "Modified" instead of "Created" by the OS
            debug!(path = ?path, "File not in tree, checking if it's a new file");

            // Verify the file actually exists on disk
            if path.exists() {
                debug!(path = ?path, "File exists on disk but not in tree, treating as creation");
                self.handle_file_created(path, cx);
            } else {
                debug!(path = ?path, "File doesn't exist on disk or in tree, ignoring");
            }
        }
    }

    /// Coalesce two file-system events for the same path using precedence:
    /// Deleted > Renamed > Created > Modified
    fn merge_fs_events(prev: &FileTreeEvent, next: &FileTreeEvent) -> FileTreeEvent {
        use crate::file_tree::FileSystemEventKind as K;
        let (p_kind, p_path) = match prev {
            FileTreeEvent::FileSystemChanged { path, kind } => (kind, path),
            _ => return next.clone(),
        };
        let (n_kind, n_path) = match next {
            FileTreeEvent::FileSystemChanged { path, kind } => (kind, path),
            _ => return next.clone(),
        };

        if p_path != n_path {
            return next.clone();
        }

        let rank = |k: &K| match k {
            K::Deleted => 3,
            K::Renamed { .. } => 2,
            K::Created => 1,
            K::Modified => 0,
        };

        match (p_kind, n_kind) {
            (K::Created, K::Deleted) => FileTreeEvent::FileSystemChanged {
                path: n_path.clone(),
                kind: K::Deleted,
            },
            (K::Deleted, K::Created) => FileTreeEvent::FileSystemChanged {
                path: n_path.clone(),
                kind: K::Created,
            },
            (K::Deleted, K::Modified) => FileTreeEvent::FileSystemChanged {
                path: n_path.clone(),
                kind: K::Deleted,
            },
            _ if rank(n_kind) >= rank(p_kind) => next.clone(),
            _ => prev.clone(),
        }
    }

    fn selection_contains_path_under(&self, path: &Path) -> bool {
        self.selected_path
            .as_ref()
            .is_some_and(|selected| selected.starts_with(path))
            || self
                .selected_paths
                .iter()
                .any(|selected| selected.starts_with(path))
    }

    fn remove_selection_under_path(
        &mut self,
        path: &Path,
        fallback_selection: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.selection_contains_path_under(path) {
            return false;
        }

        let mut selected_paths = BTreeSet::new();
        for selected in &self.selected_paths {
            if !selected.starts_with(path) {
                selected_paths.insert(selected.clone());
            }
        }

        let selected_path = if self
            .selected_path
            .as_ref()
            .is_some_and(|selected| selected.starts_with(path))
        {
            selected_paths.iter().next().cloned().or(fallback_selection)
        } else {
            self.selected_path.clone()
        };

        if selected_paths.is_empty()
            && let Some(selected_path) = &selected_path
        {
            selected_paths.insert(selected_path.clone());
        }

        self.apply_selection(selected_path, selected_paths, cx);
        true
    }

    fn rebase_selection_for_move(
        &mut self,
        from: &Path,
        to: &Path,
        fallback_selection: Option<PathBuf>,
        require_known_paths: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.selection_contains_path_under(from) {
            return false;
        }

        let mut selected_paths = BTreeSet::new();
        for selected in &self.selected_paths {
            if let Some(rebased) = rebase_file_tree_path(selected, from, to) {
                if !require_known_paths || self.tree.entry_by_path(&rebased).is_some() {
                    selected_paths.insert(rebased);
                }
            } else {
                selected_paths.insert(selected.clone());
            }
        }

        let selected_path = if let Some(selected) = self.selected_path.as_deref() {
            if let Some(rebased) = rebase_file_tree_path(selected, from, to) {
                if !require_known_paths || self.tree.entry_by_path(&rebased).is_some() {
                    Some(rebased)
                } else {
                    selected_paths.iter().next().cloned().or(fallback_selection)
                }
            } else {
                Some(selected.to_path_buf())
            }
        } else {
            None
        };

        if selected_paths.is_empty()
            && let Some(selected_path) = &selected_path
        {
            selected_paths.insert(selected_path.clone());
        }

        self.apply_selection(selected_path, selected_paths, cx);
        true
    }

    /// Handle file/directory rename
    fn handle_file_renamed(&mut self, from: &PathBuf, to: &PathBuf, cx: &mut Context<Self>) {
        debug!(from = ?from, to = ?to, "Handling file rename");

        if to.starts_with(self.tree.root_path()) {
            match self
                .tree
                .move_entry(from, to, FileTreeCollisionStrategy::Replace)
            {
                Ok(true) => {
                    self.rebase_selection_for_move(from, to, None, false, cx);
                    cx.notify();
                    return;
                }
                Ok(false) => return,
                Err(error) => {
                    debug!(error = %error, "Unable to move known file tree entry, falling back to remove/add");
                }
            }
        } else if self.tree.remove_entry(from).is_some() {
            let fallback_selection = self
                .tree
                .visible_entries()
                .first()
                .map(|entry| entry.path.clone());
            self.remove_selection_under_path(from, fallback_selection, cx);
            cx.notify();
            return;
        }

        let removed = self.tree.remove_entry(from).is_some();
        let mut changed = removed;

        if to.starts_with(self.tree.root_path())
            && let Some(parent) = to.parent()
            && self.tree.is_expanded(parent)
        {
            debug!(parent = ?parent, "Parent directory is expanded, adding renamed entry");

            if let Ok(metadata) = std::fs::metadata(to) {
                let entry = self.create_tree_entry(to, &metadata, parent);
                self.tree.upsert_entry(entry);
                changed = true;
                debug!(path = ?to, "Successfully added renamed entry to tree");
            } else {
                debug!(path = ?to, "Failed to get metadata for renamed file");
            }
        }

        if changed {
            let fallback_selection = if removed {
                self.tree
                    .visible_entries()
                    .first()
                    .map(|entry| entry.path.clone())
            } else {
                None
            };
            self.rebase_selection_for_move(from, to, fallback_selection, true, cx);
        }

        if changed {
            cx.notify();
        }
    }

    fn activate_entry(
        &mut self,
        path: PathBuf,
        action: ProjectTreeRowAction,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        self.select_path(Some(path.clone()), cx);

        match action {
            ProjectTreeRowAction::ToggleDirectory => self.toggle_directory(&path, cx),
            ProjectTreeRowAction::OpenFile => cx.emit(FileTreeEvent::OpenFile {
                path,
                focus_editor: should_focus_editor_for_project_tree_open(click_count),
            }),
        }
    }

    fn handle_project_tree_row_event(
        &mut self,
        row_event: ProjectTreeRowEvent,
        position: Option<gpui::Point<gpui::Pixels>>,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match row_event {
            ProjectTreeRowEvent::Activate { path, action } => {
                self.focus_handle.focus(window, cx);
                debug!("File tree entry clicked");
                self.activate_entry(path, action, click_count, cx);
            }
            ProjectTreeRowEvent::ContextMenuRequested { path, is_directory } => {
                let Some(position) = position else {
                    return;
                };
                self.request_entry_context_menu(path, is_directory, position, window, cx);
                window.prevent_default();
            }
            ProjectTreeRowEvent::MoveRequested { from, target_dir } => {
                self.focus_handle.focus(window, cx);
                self.handle_entry_move_requested(&from, &target_dir, cx);
                window.prevent_default();
            }
        }
    }

    fn move_destination_for_drop(&self, from: &Path, target_dir: &Path) -> Option<PathBuf> {
        let root_path = self.tree.root_path();

        if from == root_path
            || !from.starts_with(root_path)
            || !target_dir.starts_with(root_path)
            || from.parent() == Some(target_dir)
            || target_dir == from
            || target_dir.starts_with(from)
        {
            return None;
        }

        self.tree.entry_by_path(from)?;

        if target_dir != root_path {
            let target_entry = self.tree.entry_by_path(target_dir)?;
            if !target_entry.is_directory() {
                return None;
            }
        }

        let destination = file_tree_drop_destination(from, target_dir)?;
        if destination == from || self.tree.entry_by_path(&destination).is_some() {
            return None;
        }

        Some(destination)
    }

    fn handle_entry_move_requested(
        &mut self,
        from: &Path,
        target_dir: &Path,
        cx: &mut Context<Self>,
    ) {
        let Some(destination) = self.move_destination_for_drop(from, target_dir) else {
            debug!(
                from = ?from,
                target_dir = ?target_dir,
                "Ignoring unsupported file tree drop"
            );
            return;
        };
        let from_path = from.to_path_buf();
        let destination_for_io = destination.clone();
        let workspace_backend = self.workspace_backend.clone();

        cx.spawn(async move |this, cx| {
            let from_for_io = from_path.clone();
            let rename_result = cx
                .background_executor()
                .spawn(async move {
                    workspace_backend
                        .rename_path(&from_for_io, &destination_for_io)
                        .await
                })
                .await;

            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| match rename_result {
                    Ok(stat) => {
                        view.handle_entry_move_completed(from_path, stat.path, cx);
                    }
                    Err(error) => {
                        warn!(
                            from = ?from_path,
                            to = ?destination,
                            error = %error,
                            "Failed to move file tree entry through workspace backend"
                        );
                    }
                });
            }
        })
        .detach();
    }

    fn handle_entry_move_completed(
        &mut self,
        from_path: PathBuf,
        destination: PathBuf,
        cx: &mut Context<Self>,
    ) {
        match self
            .tree
            .move_entry(&from_path, &destination, FileTreeCollisionStrategy::Error)
        {
            Ok(true) => {
                self.rebase_selection_for_move(&from_path, &destination, None, false, cx);
                self.emit_file_tree_move_event(from_path, destination, cx);
                cx.notify();
            }
            Ok(false) => {}
            Err(error) => {
                warn!(
                    from = ?from_path,
                    to = ?destination,
                    error = %error,
                    "Workspace backend move succeeded but file tree model move failed; refreshing affected directories"
                );
                if let Some(parent) = from_path.parent() {
                    self.refresh_directory(parent, cx);
                }
                if let Some(parent) = destination.parent()
                    && Some(parent) != from_path.parent()
                {
                    self.refresh_directory(parent, cx);
                }
                self.emit_file_tree_move_event(from_path, destination, cx);
            }
        }
    }

    fn emit_file_tree_move_event(&mut self, from: PathBuf, to: PathBuf, cx: &mut Context<Self>) {
        if cx.has_global::<VcsServiceHandle>() {
            self.refresh_vcs_for_file_system_changes(&[from.clone(), to.clone()], cx);
        }

        cx.emit(FileTreeEvent::FileSystemChanged {
            path: to.clone(),
            kind: FileSystemEventKind::Renamed { from, to },
        });
    }

    fn request_entry_context_menu(
        &mut self,
        path: PathBuf,
        is_directory: bool,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        self.select_path(Some(path.clone()), cx);
        cx.emit(FileTreeEvent::ContextMenuRequested {
            path,
            is_directory,
            x: f32::from(position.x),
            y: f32::from(position.y),
        });
    }

    /// Render a single file tree entry with Zed-style row interactions.
    fn render_entry(
        &self,
        entry: &FileTreeEntry,
        vcs_status: Option<VcsStatus>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let is_selected = self.selected_paths.contains(&entry.path);
        let row = ProjectTreeRow::from_entry(entry, is_selected, vcs_status);
        let theme = cx.theme().clone();
        let file_tree_tokens = if self.tree.config().translucent_background {
            theme.tokens.file_tree_tokens().translucent_sidebar()
        } else {
            theme.tokens.file_tree_tokens()
        };
        let left_click_row = row.clone();
        let drop_target_path = row.path.clone();
        let density = self.tree.config().density;
        let search_active = self.search_query().is_some();
        let root_actions = (row.depth == 0).then(|| {
            let root_path = row.path.clone();
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(theme.tokens.sizes.space_2)
                .flex()
                .flex_none()
                .items_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.tokens.sizes.space_2)
                        .rounded(theme.tokens.sizes.radius_sm)
                        .bg(file_tree_tokens.background)
                        .child(
                            Button::icon_only(
                                "file-tree-search",
                                if search_active {
                                    "icons/circle-x.svg"
                                } else {
                                    "icons/search.svg"
                                },
                            )
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::ExtraSmall)
                            .tooltip(if search_active {
                                "Clear File Filter"
                            } else {
                                "Filter Files"
                            })
                            .activate_on_mouse_down()
                            .on_click(cx.listener(
                                move |view, _event, _window, cx| {
                                    if search_active {
                                        view.clear_search_query(cx);
                                    } else {
                                        view.request_search(cx);
                                    }
                                    cx.stop_propagation();
                                },
                            )),
                        )
                        .child(
                            Button::icon_only("file-tree-collapse-all", "icons/chevron-up.svg")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::ExtraSmall)
                                .tooltip("Collapse All")
                                .activate_on_mouse_down()
                                .on_click(cx.listener(|view, _event, _window, cx| {
                                    view.collapse_all_directories(cx);
                                    cx.stop_propagation();
                                })),
                        )
                        .child(
                            Button::icon_only("file-tree-more", "icons/menu.svg")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::ExtraSmall)
                                .tooltip("More Actions")
                                .activate_on_mouse_down()
                                .on_click(cx.listener(
                                    move |view, event: &ClickEvent, window, cx| {
                                        view.request_entry_context_menu(
                                            root_path.clone(),
                                            true,
                                            event.position(),
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    },
                                )),
                        ),
                )
                .into_any_element()
        });

        render_project_tree_row(
            row,
            ProjectTreeRowStyle::new(&theme, file_tree_tokens),
            density,
            root_actions,
            Arc::new({
                let left_click_row = left_click_row.clone();
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    let row_event = if event.button == MouseButton::Right {
                        left_click_row.context_menu_event()
                    } else {
                        left_click_row.click_event(event.modifiers.secondary())
                    };
                    view.handle_project_tree_row_event(
                        row_event,
                        Some(event.position),
                        event.click_count,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                })
            }),
            |_, _, cx| cx.stop_propagation(),
            {
                let drop_target_path = drop_target_path.clone();
                cx.listener(move |view, dragged: &ProjectTreeDraggedEntry, window, cx| {
                    view.handle_project_tree_row_event(
                        ProjectTreeRowEvent::MoveRequested {
                            from: dragged.path.clone(),
                            target_dir: drop_target_path.clone(),
                        },
                        None,
                        1,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                })
            },
        )
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::file_tree::entry::FileTreeEntryId;
    use gpui::{AppContext, TestAppContext};
    use nucleotide_workspace::{
        FileRead, FileSearchQuery, FileSearchResult, FileStat, GitHeadResult, GitStatusOptions,
        GitStatusResult, ProcessOutput, ProcessSpec, ProjectEnvironmentSnapshot, ReadOptions,
        TextSearchQuery, TextSearchResult, WorkspaceBackend, WorkspaceError, WorkspacePathMapping,
        WriteOptions, WriteResult, path_mapped_workspace_backend,
    };
    use std::{cell::RefCell, rc::Rc};

    fn test_config() -> FileTreeConfig {
        FileTreeConfig {
            show_hidden: true,
            show_ignored: true,
            initial_depth: 3,
            watch_filesystem: false,
            flatten_empty_directories: true,
            search_mode: crate::file_tree::FileTreeSearchMode::ExpandMatches,
            density: FileTreeDisplayDensity::Default,
            translucent_background: false,
        }
    }

    #[derive(Debug)]
    struct RemoteWatchTestBackend;

    impl RemoteWatchTestBackend {
        fn unsupported<T>(
            &self,
            operation: &'static str,
            path: &Path,
        ) -> nucleotide_workspace::Result<T> {
            Err(WorkspaceError::Remote {
                operation,
                path: path.to_path_buf(),
                message: "unsupported by file tree watch test backend".to_string(),
                diagnostic: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl WorkspaceBackend for RemoteWatchTestBackend {
        fn identity(&self) -> WorkspaceIdentity {
            WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
                kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
                name: "test".to_string(),
            })
        }

        async fn stat(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("stat", path)
        }

        async fn list_dir(&self, path: &Path) -> nucleotide_workspace::Result<DirectoryListing> {
            self.unsupported("list directory", path)
        }

        async fn find_ancestor_file(
            &self,
            start: &Path,
            _file_name: &str,
            _limit: usize,
        ) -> nucleotide_workspace::Result<Option<PathBuf>> {
            self.unsupported("find ancestor file", start)
        }

        async fn create_file(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("create file", path)
        }

        async fn create_dir(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("create directory", path)
        }

        async fn rename_path(
            &self,
            from: &Path,
            _to: &Path,
        ) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("rename path", from)
        }

        async fn delete_path(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("delete path", path)
        }

        async fn copy_path(
            &self,
            from: &Path,
            _to: &Path,
        ) -> nucleotide_workspace::Result<FileStat> {
            self.unsupported("copy path", from)
        }

        async fn read_file(
            &self,
            path: &Path,
            _options: ReadOptions,
        ) -> nucleotide_workspace::Result<FileRead> {
            self.unsupported("read file", path)
        }

        async fn write_file(
            &self,
            path: &Path,
            _bytes: &[u8],
            _options: WriteOptions,
        ) -> nucleotide_workspace::Result<WriteResult> {
            self.unsupported("write file", path)
        }

        async fn file_search(
            &self,
            query: FileSearchQuery,
        ) -> nucleotide_workspace::Result<FileSearchResult> {
            self.unsupported("file search", &query.root)
        }

        async fn text_search(
            &self,
            query: TextSearchQuery,
        ) -> nucleotide_workspace::Result<TextSearchResult> {
            self.unsupported("text search", &query.root)
        }

        async fn project_environment(
            &self,
            root: &Path,
        ) -> nucleotide_workspace::Result<ProjectEnvironmentSnapshot> {
            self.unsupported("project environment", root)
        }

        async fn git_head(&self, root: &Path) -> nucleotide_workspace::Result<GitHeadResult> {
            self.unsupported("git head", root)
        }

        async fn git_status(
            &self,
            root: &Path,
            _options: GitStatusOptions,
        ) -> nucleotide_workspace::Result<GitStatusResult> {
            self.unsupported("git status", root)
        }

        async fn run_process(
            &self,
            spec: ProcessSpec,
        ) -> nucleotide_workspace::Result<ProcessOutput> {
            self.unsupported("run process", &spec.cwd)
        }
    }

    fn subscribe_file_tree_events(
        cx: &mut TestAppContext,
        view: &gpui::Entity<FileTreeView>,
    ) -> Rc<RefCell<Vec<FileTreeEvent>>> {
        let events = Rc::new(RefCell::new(Vec::new()));
        let events_for_subscription = events.clone();

        cx.update(|cx| {
            cx.subscribe(view, move |_view, event: &FileTreeEvent, _cx| {
                events_for_subscription.borrow_mut().push(event.clone());
            })
            .detach();
        });
        cx.run_until_parked();

        events
    }

    fn file_fingerprint(path: &str, size: u64) -> DirectoryEntryFingerprint {
        DirectoryEntryFingerprint {
            path: PathBuf::from(path),
            kind: DirectoryEntryFingerprintKind::File,
            size,
            modified: None,
            symlink_target: None,
            target_exists: None,
            ignored: Some(false),
        }
    }

    #[test]
    fn listing_and_tree_entry_fingerprints_match_common_metadata() {
        let path = PathBuf::from("/workspace/src/main.rs");
        let modified = Some(std::time::SystemTime::UNIX_EPOCH);
        let listing = DirectoryListing {
            path: PathBuf::from("/workspace/src"),
            entries: vec![nucleotide_workspace::DirectoryEntry {
                name: "main.rs".to_string(),
                path: path.clone(),
                stat: nucleotide_workspace::FileStat {
                    path: path.clone(),
                    kind: WorkspaceFileKind::File,
                    size: 12,
                    modified,
                    version: None,
                    readonly: false,
                },
                symlink_target: None,
                target_exists: None,
                ignored: Some(false),
            }],
        };
        let mut entry = FileTreeEntry::new_file(
            crate::file_tree::entry::FileTreeEntryId(1),
            path,
            12,
            modified,
        );
        entry.is_ignored = false;

        assert_eq!(
            listing_fingerprint(&listing),
            tree_entries_fingerprint(vec![entry])
        );
    }

    #[test]
    fn changed_paths_between_fingerprints_reports_modified_added_and_deleted_children() {
        let previous = DirectoryListingFingerprint {
            entries: vec![
                file_fingerprint("/workspace/src/deleted.rs", 1),
                file_fingerprint("/workspace/src/main.rs", 1),
            ],
        };
        let current = DirectoryListingFingerprint {
            entries: vec![
                file_fingerprint("/workspace/src/added.rs", 1),
                file_fingerprint("/workspace/src/main.rs", 2),
            ],
        };

        let paths = changed_paths_between_fingerprints(Some(&previous), &current);

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/workspace/src/added.rs"),
                PathBuf::from("/workspace/src/deleted.rs"),
                PathBuf::from("/workspace/src/main.rs"),
            ]
        );
    }

    #[test]
    fn changed_paths_between_fingerprints_reports_current_entries_without_previous_cache() {
        let current = DirectoryListingFingerprint {
            entries: vec![
                file_fingerprint("/workspace/src/a.rs", 1),
                file_fingerprint("/workspace/src/b.rs", 1),
            ],
        };

        let paths = changed_paths_between_fingerprints(None, &current);

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/workspace/src/a.rs"),
                PathBuf::from("/workspace/src/b.rs"),
            ]
        );
    }

    #[test]
    fn file_system_changes_between_fingerprints_reports_typed_changes() {
        let previous = DirectoryListingFingerprint {
            entries: vec![
                file_fingerprint("/workspace/src/deleted.rs", 1),
                file_fingerprint("/workspace/src/main.rs", 1),
            ],
        };
        let current = DirectoryListingFingerprint {
            entries: vec![
                file_fingerprint("/workspace/src/added.rs", 1),
                file_fingerprint("/workspace/src/main.rs", 2),
            ],
        };

        let changes = file_system_changes_between_fingerprints(Some(&previous), &current);

        assert_eq!(
            changes,
            vec![
                RemoteFileSystemChange {
                    path: PathBuf::from("/workspace/src/added.rs"),
                    kind: FileSystemEventKind::Created,
                },
                RemoteFileSystemChange {
                    path: PathBuf::from("/workspace/src/deleted.rs"),
                    kind: FileSystemEventKind::Deleted,
                },
                RemoteFileSystemChange {
                    path: PathBuf::from("/workspace/src/main.rs"),
                    kind: FileSystemEventKind::Modified,
                },
            ]
        );
    }

    #[test]
    fn file_system_changes_between_fingerprints_skips_initial_cache_seed() {
        let current = DirectoryListingFingerprint {
            entries: vec![file_fingerprint("/workspace/src/main.rs", 1)],
        };

        assert!(file_system_changes_between_fingerprints(None, &current).is_empty());
    }

    #[test]
    fn remote_file_tree_poll_interval_stays_fast_initially() {
        assert_eq!(
            remote_file_tree_poll_interval(0),
            REMOTE_FILE_TREE_POLL_INTERVAL
        );
        assert_eq!(
            remote_file_tree_poll_interval(REMOTE_FILE_TREE_IDLE_BACKOFF_AFTER_POLLS - 1),
            REMOTE_FILE_TREE_POLL_INTERVAL
        );
    }

    #[test]
    fn remote_file_tree_poll_interval_backs_off_while_idle() {
        assert_eq!(
            remote_file_tree_poll_interval(REMOTE_FILE_TREE_IDLE_BACKOFF_AFTER_POLLS + 1),
            std::time::Duration::from_secs(4)
        );
        assert_eq!(
            remote_file_tree_poll_interval(REMOTE_FILE_TREE_IDLE_BACKOFF_AFTER_POLLS + 3),
            REMOTE_FILE_TREE_POLL_MAX_INTERVAL
        );
        assert_eq!(
            remote_file_tree_poll_interval(u32::MAX),
            REMOTE_FILE_TREE_POLL_MAX_INTERVAL
        );
    }

    #[gpui::test]
    async fn remote_watch_disconnect_restarts_polling_at_fast_interval(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        std::fs::write(root_path.join("main.rs"), "fn main() {}\n").unwrap();
        let mut config = test_config();
        config.watch_filesystem = true;
        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), config, cx));

        view.update(cx, |view, cx| {
            view.workspace_backend = std::sync::Arc::new(RemoteWatchTestBackend);
            view.start_remote_file_polling(cx);
            let initial_generation = view.remote_file_polling_generation;
            view.remote_file_poll_idle_ticks = REMOTE_FILE_TREE_IDLE_BACKOFF_AFTER_POLLS + 3;
            view.remote_file_watch_active = true;
            view.remote_file_watch_id = Some(7);
            view.remote_file_watch_roots.insert(root_path.clone());

            view.restart_remote_file_polling_after_watch_disconnect(cx);

            assert!(!view.remote_file_watch_active);
            assert_eq!(view.remote_file_watch_id, None);
            assert!(view.remote_file_watch_roots.is_empty());
            assert!(view.remote_file_polling_active);
            assert_eq!(view.remote_file_poll_idle_ticks, 0);
            assert_eq!(
                view.remote_file_polling_generation,
                initial_generation.wrapping_add(1)
            );
            assert_eq!(view.remote_poll_interval(), REMOTE_FILE_TREE_POLL_INTERVAL);
        });
    }

    #[gpui::test]
    async fn initial_selection_is_exposed_as_path_set(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        std::fs::write(root_path.join("main.rs"), "fn main() {}\n").unwrap();

        let mut config = test_config();
        config.watch_filesystem = true;
        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), config, cx));

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), Some(&root_path));
            assert_eq!(view.selected_paths(), vec![root_path]);
        });
    }

    #[gpui::test]
    async fn runtime_constructor_defers_initial_load(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        std::fs::write(root_path.join("main.rs"), "fn main() {}\n").unwrap();

        let view =
            cx.new(|cx| FileTreeView::new_with_runtime(root_path.clone(), test_config(), None, cx));

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), None);
            assert!(view.selected_paths().is_empty());
        });

        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), Some(&root_path));
            assert_eq!(view.selected_paths(), vec![root_path]);
        });
    }

    #[gpui::test]
    async fn deferred_initial_load_preserves_existing_selection(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let file_path = root_path.join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();

        let view =
            cx.new(|cx| FileTreeView::new_with_runtime(root_path.clone(), test_config(), None, cx));

        view.update(cx, |view, cx| {
            view.select_path(Some(file_path.clone()), cx);
        });

        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), Some(&file_path));
            assert_eq!(view.selected_paths(), vec![file_path]);
        });
    }

    #[gpui::test]
    async fn selected_path_is_directory_reflects_tree_entry_kind(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let file_path = root_path.join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), test_config(), cx));

        view.update(cx, |view, cx| {
            assert_eq!(view.selected_path(), Some(&root_path));
            assert!(view.selected_path_is_directory());

            view.select_path(Some(file_path), cx);
            assert!(!view.selected_path_is_directory());
        });
    }

    #[gpui::test]
    async fn delete_selected_emits_operation_request_for_selected_file(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let file_path = root_path.join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));
        let events = subscribe_file_tree_events(cx, &view);

        view.update(cx, |view, cx| {
            view.select_path(Some(file_path.clone()), cx);
            view.request_delete_selected(cx);
        });
        cx.run_until_parked();

        assert!(
            events
                .borrow()
                .contains(&FileTreeEvent::OperationRequested {
                    intent: ProjectTreeContextMenuIntent::Delete,
                    path: file_path,
                    is_directory: false,
                })
        );
    }

    #[gpui::test]
    async fn delete_selected_emits_operation_request_for_selected_directory(
        cx: &mut TestAppContext,
    ) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        std::fs::create_dir(root_path.join("src")).unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), test_config(), cx));
        let events = subscribe_file_tree_events(cx, &view);

        view.update(cx, |view, cx| {
            assert_eq!(view.selected_path(), Some(&root_path));
            view.request_delete_selected(cx);
        });
        cx.run_until_parked();

        assert!(
            events
                .borrow()
                .contains(&FileTreeEvent::OperationRequested {
                    intent: ProjectTreeContextMenuIntent::Delete,
                    path: root_path,
                    is_directory: true,
                })
        );
    }

    #[gpui::test]
    async fn selected_operation_emits_request_with_selected_entry(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let file_path = root_path.join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));
        let events = subscribe_file_tree_events(cx, &view);

        view.update(cx, |view, cx| {
            view.select_path(Some(file_path.clone()), cx);
            view.request_selected_operation(ProjectTreeContextMenuIntent::Rename, cx);
        });
        cx.run_until_parked();

        assert!(
            events
                .borrow()
                .contains(&FileTreeEvent::OperationRequested {
                    intent: ProjectTreeContextMenuIntent::Rename,
                    path: file_path,
                    is_directory: false,
                })
        );
    }

    #[gpui::test]
    async fn selection_set_supports_add_toggle_deselect_and_range(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let a_path = root_path.join("a.rs");
        let b_path = root_path.join("b.rs");
        let c_path = root_path.join("c.rs");
        std::fs::write(&a_path, "a\n").unwrap();
        std::fs::write(&b_path, "b\n").unwrap();
        std::fs::write(&c_path, "c\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), test_config(), cx));
        let events = subscribe_file_tree_events(cx, &view);

        view.update(cx, |view, cx| {
            view.select_only_path(a_path.clone(), cx);
            assert_eq!(view.selected_path(), Some(&a_path));
            assert_eq!(view.selected_paths(), vec![a_path.clone()]);

            view.select_additional_path(b_path.clone(), cx);
            assert_eq!(view.selected_path(), Some(&b_path));
            assert_eq!(view.selected_paths(), vec![a_path.clone(), b_path.clone()]);

            assert!(view.toggle_path_selection(a_path.clone(), cx));
            assert_eq!(view.selected_path(), Some(&b_path));
            assert_eq!(view.selected_paths(), vec![b_path.clone()]);

            assert!(view.deselect_path(&b_path, cx));
            assert_eq!(view.selected_path(), None);
            assert!(view.selected_paths().is_empty());

            assert!(view.select_path_range(&a_path, &c_path, cx));
            assert_eq!(view.selected_path(), Some(&c_path));
            assert_eq!(
                view.selected_paths(),
                vec![a_path.clone(), b_path.clone(), c_path.clone()]
            );

            view.select_only_path(root_path.clone(), cx);
            assert!(view.add_path_range_to_selection(&a_path, &b_path, cx));
            assert_eq!(view.selected_path(), Some(&b_path));
            assert_eq!(
                view.selected_paths(),
                vec![root_path.clone(), a_path.clone(), b_path.clone()]
            );
        });
        cx.run_until_parked();

        assert!(
            events
                .borrow()
                .contains(&FileTreeEvent::SelectionSetChanged {
                    paths: vec![a_path, b_path, c_path],
                })
        );
    }

    #[gpui::test]
    async fn deleted_directory_removes_descendant_paths_from_selection_set(
        cx: &mut TestAppContext,
    ) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let src_path = root_path.join("src");
        let file_path = src_path.join("lib.rs");
        let readme_path = root_path.join("README.md");
        std::fs::create_dir_all(&src_path).unwrap();
        std::fs::write(&file_path, "pub fn lib() {}\n").unwrap();
        std::fs::write(&readme_path, "readme\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));

        view.update(cx, |view, cx| {
            view.select_path(Some(file_path.clone()), cx);
            view.select_additional_path(readme_path.clone(), cx);
            assert_eq!(
                view.selected_paths(),
                vec![readme_path.clone(), file_path.clone()]
            );

            view.handle_file_deleted(&src_path, cx);
            assert_eq!(view.selected_path(), Some(&readme_path));
            assert_eq!(view.selected_paths(), vec![readme_path.clone()]);
            assert!(!view.contains_path(&file_path));
        });
    }

    #[gpui::test]
    async fn file_activation_selects_entry_and_emits_open_file(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("main.rs");
        std::fs::write(&file_path, "fn main() {}\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(temp_dir.path().to_path_buf(), test_config(), cx));
        let events = subscribe_file_tree_events(cx, &view);

        view.update(cx, |view, cx| {
            view.activate_entry(file_path.clone(), ProjectTreeRowAction::OpenFile, 1, cx);
        });
        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), Some(&file_path));
            assert_eq!(view.selected_paths(), vec![file_path.clone()]);
        });

        let events = events.borrow();
        assert!(events.contains(&FileTreeEvent::SelectionChanged {
            path: Some(file_path.clone()),
        }));
        assert!(events.contains(&FileTreeEvent::SelectionSetChanged {
            paths: vec![file_path.clone()],
        }));
        assert!(events.contains(&FileTreeEvent::OpenFile {
            path: file_path,
            focus_editor: false,
        }));
    }

    #[test]
    fn project_tree_file_open_focuses_editor_on_double_click_only() {
        assert!(!should_focus_editor_for_project_tree_open(0));
        assert!(!should_focus_editor_for_project_tree_open(1));
        assert!(should_focus_editor_for_project_tree_open(2));
        assert!(should_focus_editor_for_project_tree_open(3));
    }

    #[test]
    fn renamed_file_system_event_refreshes_old_and_new_vcs_paths() {
        let from = PathBuf::from("/workspace/old.rs");
        let to = PathBuf::from("/workspace/new.rs");
        let event = FileTreeEvent::FileSystemChanged {
            path: to.clone(),
            kind: crate::file_tree::FileSystemEventKind::Renamed {
                from: from.clone(),
                to: to.clone(),
            },
        };

        let paths = FileTreeView::vcs_paths_for_file_system_event(&event);

        assert_eq!(paths, vec![from, to]);
    }

    #[test]
    fn vcs_changed_paths_keep_remote_display_paths_rooted() {
        let root = PathBuf::from("ssh://devbox/home/me/project");
        let rooted = PathBuf::from("ssh://devbox/home/me/project/src/lib.rs");
        let relative = PathBuf::from("src/main.rs");

        let paths = FileTreeView::vcs_changed_paths_for_root(&root, &[rooted.clone(), relative]);

        assert_eq!(
            paths,
            vec![
                rooted,
                PathBuf::from("ssh://devbox/home/me/project/src/main.rs")
            ]
        );
    }

    #[test]
    fn file_tree_preserves_remote_display_root_spelling() {
        let root = PathBuf::from("ssh://devbox/home/me/project");
        let identity = WorkspaceIdentity::Remote(nucleotide_workspace::RemoteWorkspaceIdentity {
            kind: nucleotide_workspace::RemoteWorkspaceKind::Ssh,
            name: "devbox".to_string(),
        });

        let tree = FileTree::new_for_backend(root.clone(), test_config(), identity);

        assert_eq!(tree.root_path(), root.as_path());
    }

    #[test]
    fn presentation_cache_reuses_only_matching_tree_and_vcs_revisions() {
        let statuses: Arc<[Option<VcsStatus>]> = Arc::from(vec![Some(VcsStatus::Modified)]);
        let cache = FileTreePresentationCache {
            tree_revision: 3,
            density: FileTreeDisplayDensity::Default,
            vcs_revision: 5,
            entry_count: 1,
            vcs_statuses: statuses.clone(),
            width_measure_item_index: Some(0),
            content_width: 120.0,
        };

        let reused =
            cached_file_tree_presentation(Some(&cache), 3, FileTreeDisplayDensity::Default, 5, 1)
                .expect("cache hit");
        assert!(Arc::ptr_eq(&reused.vcs_statuses, &statuses));
        assert!(
            cached_file_tree_presentation(Some(&cache), 4, FileTreeDisplayDensity::Default, 5, 1,)
                .is_none()
        );
        assert!(
            cached_file_tree_presentation(Some(&cache), 3, FileTreeDisplayDensity::Default, 6, 1,)
                .is_none()
        );
    }

    #[gpui::test]
    async fn remote_watch_batch_refresh_plan_targets_expanded_directories(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        std::fs::create_dir(root_path.join("src")).unwrap();
        std::fs::write(root_path.join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut config = test_config();
        config.watch_filesystem = true;
        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), config, cx));

        view.update(cx, |view, _cx| {
            view.workspace_backend = std::sync::Arc::new(RemoteWatchTestBackend);
            view.remote_file_watch_active = true;
            view.remote_file_watch_id = Some(1);
            let expanded_root = view.tree.expanded_directory_paths()[0].clone();

            let batch = WorkspaceWatchBatch {
                watch_id: 1,
                sequence: 1,
                directory_generations: vec![
                    nucleotide_workspace::WorkspaceWatchDirectoryGeneration {
                        path: expanded_root.clone(),
                        generation: 2,
                    },
                ],
                events: Vec::new(),
                overflow: false,
                resync_required: false,
            };
            let foreign_batch = WorkspaceWatchBatch {
                watch_id: 2,
                sequence: 1,
                directory_generations: batch.directory_generations.clone(),
                events: Vec::new(),
                overflow: false,
                resync_required: false,
            };
            assert!(view.remote_watch_refresh_plan(&foreign_batch).is_none());
            let plan = view
                .remote_watch_refresh_plan(&batch)
                .expect("batch should refresh expanded root");
            assert_eq!(plan.directories, vec![expanded_root.clone()]);
            assert_eq!(view.remote_file_watch_last_sequence, 0);
            assert!(view.remote_watch_refresh_plan(&batch).is_none());
            view.finish_remote_watch_refresh(&plan, true);
            assert_eq!(view.remote_file_watch_last_sequence, 1);

            let resync = WorkspaceWatchBatch {
                watch_id: 1,
                sequence: 2,
                directory_generations: Vec::new(),
                events: Vec::new(),
                overflow: true,
                resync_required: true,
            };
            let plan = view
                .remote_watch_refresh_plan(&resync)
                .expect("resync should refresh expanded directories");
            assert!(plan.directories.contains(&expanded_root));
            assert!(plan.requires_resync);
            assert!(view.remote_file_watch_resync_pending);
        });
    }

    #[gpui::test]
    async fn remote_watch_resync_rejects_old_results_until_refresh_is_complete(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            let vcs_service = VcsServiceHandle::new(nucleotide_vcs::VcsConfig::default(), cx);
            cx.set_global(vcs_service);
        });
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let main_path = root_path.join("main.rs");
        std::fs::write(&main_path, "fn main() {}\n").unwrap();
        let mut config = test_config();
        config.watch_filesystem = true;
        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), config, cx));

        view.update(cx, |view, cx| {
            view.workspace_backend = std::sync::Arc::new(RemoteWatchTestBackend);
            view.remote_file_watch_active = true;
            view.remote_file_watch_id = Some(7);
            let old_epoch = view.remote_file_watch_epoch;

            let resync = WorkspaceWatchBatch {
                watch_id: 7,
                sequence: 4,
                directory_generations: Vec::new(),
                events: Vec::new(),
                overflow: false,
                resync_required: true,
            };
            let plan = view
                .remote_watch_refresh_plan(&resync)
                .expect("resync should create a full refresh plan");
            assert_ne!(plan.epoch, old_epoch);
            assert_eq!(view.remote_file_watch_last_sequence, 0);

            let stale = view.apply_remote_directory_poll_results(
                vec![RemoteDirectoryPollResult {
                    path: root_path.clone(),
                    listing: Ok(DirectoryListing {
                        path: root_path.clone(),
                        entries: Vec::new(),
                    }),
                }],
                old_epoch,
                &plan.directories,
                false,
                cx,
            );
            assert_eq!(stale, RemoteDirectoryPollApplication::Stale);
            assert!(view.tree.entry_by_path(&main_path).is_some());

            let incomplete = view.apply_remote_directory_poll_results(
                vec![RemoteDirectoryPollResult {
                    path: root_path.clone(),
                    listing: Err("temporary list failure".to_string()),
                }],
                plan.epoch,
                &plan.directories,
                true,
                cx,
            );
            assert_eq!(
                incomplete,
                RemoteDirectoryPollApplication::Incomplete(vec![root_path.clone()])
            );
            assert_eq!(view.remote_file_watch_last_sequence, 0);
            assert!(view.remote_file_watch_resync_pending);

            let next_batch = WorkspaceWatchBatch {
                watch_id: 7,
                sequence: 5,
                directory_generations: Vec::new(),
                events: Vec::new(),
                overflow: false,
                resync_required: false,
            };
            assert!(view.remote_watch_refresh_plan(&next_batch).is_none());

            let complete = view.apply_remote_directory_poll_results(
                vec![RemoteDirectoryPollResult {
                    path: root_path.clone(),
                    listing: Ok(DirectoryListing {
                        path: root_path,
                        entries: Vec::new(),
                    }),
                }],
                plan.epoch,
                &plan.directories,
                true,
                cx,
            );
            assert_eq!(complete, RemoteDirectoryPollApplication::Complete);
            view.finish_remote_watch_refresh(&plan, true);
            assert_eq!(view.remote_file_watch_last_sequence, 4);
            assert_eq!(view.remote_file_watch_pending_sequence, None);
            assert!(!view.remote_file_watch_resync_pending);
        });
    }

    #[test]
    fn widest_project_tree_entry_index_uses_depth_and_filename_width() {
        let mut shallow = FileTreeEntry::new_file(
            FileTreeEntryId(1),
            PathBuf::from("/workspace/main.rs"),
            0,
            None,
        );
        shallow.depth = 0;
        let mut deep = FileTreeEntry::new_file(
            FileTreeEntryId(2),
            PathBuf::from("/workspace/src/nested/very_long_component_name.rs"),
            0,
            None,
        );
        deep.depth = 4;
        let mut medium = FileTreeEntry::new_file(
            FileTreeEntryId(3),
            PathBuf::from("/workspace/src/lib.rs"),
            0,
            None,
        );
        medium.depth = 2;

        assert_eq!(
            widest_project_tree_entry_index(
                &[shallow, deep, medium],
                FileTreeDisplayDensity::Default
            ),
            Some(1)
        );
    }

    #[test]
    fn project_tree_content_width_uses_longest_visible_entry() {
        let mut shallow = FileTreeEntry::new_file(
            FileTreeEntryId(1),
            PathBuf::from("/workspace/main.rs"),
            0,
            None,
        );
        shallow.depth = 0;
        let mut deep = FileTreeEntry::new_file(
            FileTreeEntryId(2),
            PathBuf::from("/workspace/src/nested/very_long_component_name.rs"),
            0,
            None,
        );
        deep.depth = 4;

        let expected = project_tree_entry_min_width(&deep, FileTreeDisplayDensity::Default);

        assert_eq!(
            project_tree_content_width(&[shallow, deep], FileTreeDisplayDensity::Default, |_| None),
            expected
        );
    }

    #[test]
    fn project_tree_content_width_scales_spacing_with_density() {
        let mut entry = FileTreeEntry::new_file(
            FileTreeEntryId(1),
            PathBuf::from("/workspace/src/nested/main.rs"),
            0,
            None,
        );
        entry.depth = 4;

        let compact =
            project_tree_content_width(&[entry.clone()], FileTreeDisplayDensity::Compact, |_| None);
        let default =
            project_tree_content_width(&[entry.clone()], FileTreeDisplayDensity::Default, |_| None);
        let relaxed =
            project_tree_content_width(&[entry], FileTreeDisplayDensity::Relaxed, |_| None);

        assert!(compact < default);
        assert!(default < relaxed);
    }

    #[test]
    fn file_tree_scroll_offset_maps_to_gpui_scroll_strategy() {
        assert_eq!(
            scroll_strategy_for_file_tree_offset(FileTreeScrollOffset::Top),
            ScrollStrategy::Top
        );
        assert_eq!(
            scroll_strategy_for_file_tree_offset(FileTreeScrollOffset::Center),
            ScrollStrategy::Center
        );
        assert_eq!(
            scroll_strategy_for_file_tree_offset(FileTreeScrollOffset::Nearest),
            ScrollStrategy::Nearest
        );
    }

    #[test]
    fn project_tree_content_width_uses_longest_entry_across_visible_tree() {
        let mut long = FileTreeEntry::new_file(
            FileTreeEntryId(1),
            PathBuf::from("/workspace/extremely_long_name_that_can_scroll.rs"),
            0,
            None,
        );
        long.depth = 1;
        let mut short = FileTreeEntry::new_file(
            FileTreeEntryId(2),
            PathBuf::from("/workspace/lib.rs"),
            0,
            None,
        );
        short.depth = 1;

        let entries = [long.clone(), short];
        let expected = project_tree_entry_min_width(&long, FileTreeDisplayDensity::Default);

        assert_eq!(
            project_tree_content_width(&entries, FileTreeDisplayDensity::Default, |_| None),
            expected
        );
        assert_eq!(
            widest_project_tree_entry_index(&entries, FileTreeDisplayDensity::Default),
            Some(0)
        );
    }

    #[gpui::test]
    async fn scroll_to_path_selects_visible_path_and_schedules_offset(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let target_path = root_path.join("target.rs");
        std::fs::write(root_path.join("a.rs"), "a\n").unwrap();
        std::fs::write(&target_path, "target\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));

        view.update(cx, |view, cx| {
            assert!(view.scroll_to_path(
                &target_path,
                FileTreeScrollToPathOptions {
                    focus: true,
                    offset: FileTreeScrollOffset::Center,
                },
                cx,
            ));
            assert_eq!(view.selected_path(), Some(&target_path));
            assert_eq!(view.selected_paths(), vec![target_path.clone()]);

            let scroll_state = view.scroll_handle.0.borrow();
            let deferred = scroll_state.deferred_scroll_to_item.as_ref().unwrap();
            assert_eq!(deferred.item_index, 2);
            assert_eq!(deferred.strategy, ScrollStrategy::Center);
            assert!(deferred.scroll_strict);
        });
    }

    #[gpui::test]
    async fn scroll_to_path_can_preserve_selection_with_nearest_offset(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let target_path = root_path.join("target.rs");
        std::fs::write(root_path.join("a.rs"), "a\n").unwrap();
        std::fs::write(&target_path, "target\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), test_config(), cx));

        view.update(cx, |view, cx| {
            assert_eq!(view.selected_path(), Some(&root_path));
            assert!(view.scroll_to_path(
                &target_path,
                FileTreeScrollToPathOptions {
                    focus: false,
                    offset: FileTreeScrollOffset::Nearest,
                },
                cx,
            ));
            assert_eq!(view.selected_path(), Some(&root_path));
            assert_eq!(view.selected_paths(), vec![root_path.clone()]);

            let scroll_state = view.scroll_handle.0.borrow();
            let deferred = scroll_state.deferred_scroll_to_item.as_ref().unwrap();
            assert_eq!(deferred.item_index, 2);
            assert_eq!(deferred.strategy, ScrollStrategy::Nearest);
            assert!(!deferred.scroll_strict);
        });
    }

    #[gpui::test]
    async fn scroll_to_path_ignores_known_paths_that_are_not_visible(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let src_path = root_path.join("src");
        let target_path = src_path.join("lib.rs");
        std::fs::create_dir(&src_path).unwrap();
        std::fs::write(&target_path, "pub fn lib() {}\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));

        view.update(cx, |view, cx| {
            view.tree.collapse_directory(&src_path).unwrap();

            assert!(
                !view.scroll_to_path(&target_path, FileTreeScrollToPathOptions::default(), cx,)
            );
            assert!(
                view.scroll_handle
                    .0
                    .borrow()
                    .deferred_scroll_to_item
                    .is_none()
            );
        });
    }

    #[gpui::test]
    async fn directory_activation_selects_entry_and_emits_toggle(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        std::fs::create_dir(root_path.join("src")).unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path.clone(), test_config(), cx));
        let events = subscribe_file_tree_events(cx, &view);

        view.update(cx, |view, cx| {
            assert!(view.tree.is_expanded(&root_path));
            view.activate_entry(
                root_path.clone(),
                ProjectTreeRowAction::ToggleDirectory,
                1,
                cx,
            );
        });
        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), Some(&root_path));
            assert_eq!(view.selected_paths(), vec![root_path.clone()]);
            assert!(!view.tree.is_expanded(&root_path));
        });

        let events = events.borrow();
        assert!(events.contains(&FileTreeEvent::DirectoryToggled {
            path: root_path,
            expanded: false,
        }));
    }

    #[gpui::test]
    async fn search_query_selects_first_visible_match(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let button_path = root_path.join("button.rs");
        let readme_path = root_path.join("README.md");
        std::fs::write(&button_path, "button\n").unwrap();
        std::fs::write(&readme_path, "readme\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));
        let initial_revision = view.read_with(cx, |view, _cx| view.tree_revision);

        view.update(cx, |view, cx| {
            view.set_search_query(Some("button".to_string()), cx);
        });
        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.search_query(), Some("button"));
            assert_eq!(view.selected_path(), Some(&button_path));
            assert_eq!(view.selected_paths(), vec![button_path.clone()]);
            assert!(view.tree_revision > initial_revision);
        });

        let search_revision = view.read_with(cx, |view, _cx| view.tree_revision);

        view.update(cx, |view, cx| {
            view.clear_search_query(cx);
        });
        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.search_query(), None);
            assert!(view.tree_revision > search_revision);
        });
    }

    #[gpui::test]
    async fn search_request_emits_current_query(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        std::fs::write(root_path.join("button.rs"), "button\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));
        let events = subscribe_file_tree_events(cx, &view);

        view.update(cx, |view, cx| {
            view.set_search_query(Some("button".to_string()), cx);
            view.request_search(cx);
        });
        cx.run_until_parked();

        assert!(events.borrow().contains(&FileTreeEvent::SearchRequested {
            initial_query: Some("button".to_string()),
        }));
    }

    #[gpui::test]
    async fn directory_rename_rebases_loaded_descendant_selection(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let src_path = root_path.join("src");
        let nested_path = src_path.join("nested");
        let file_path = nested_path.join("lib.rs");
        let renamed_path = root_path.join("crates");
        let renamed_file_path = renamed_path.join("nested").join("lib.rs");
        std::fs::create_dir_all(&nested_path).unwrap();
        std::fs::write(&file_path, "pub fn lib() {}\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));
        std::fs::rename(&src_path, &renamed_path).unwrap();

        view.update(cx, |view, cx| {
            view.select_path(Some(file_path.clone()), cx);
            view.handle_file_renamed(&src_path, &renamed_path, cx);
        });
        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), Some(&renamed_file_path));
            assert_eq!(view.selected_paths(), vec![renamed_file_path.clone()]);
            assert!(view.contains_path(&renamed_file_path));
            assert!(!view.contains_path(&file_path));
        });
    }

    #[gpui::test]
    async fn entry_drop_moves_file_into_target_directory(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let root_path = temp_dir.path().to_path_buf();
        let source_dir = root_path.join("src");
        let target_dir = root_path.join("crates");
        let file_path = source_dir.join("lib.rs");
        let moved_file_path = target_dir.join("lib.rs");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(&file_path, "pub fn lib() {}\n").unwrap();

        let view = cx.new(|cx| FileTreeView::new(root_path, test_config(), cx));
        let events = subscribe_file_tree_events(cx, &view);

        view.update(cx, |view, cx| {
            view.select_path(Some(file_path.clone()), cx);
            view.handle_entry_move_requested(&file_path, &target_dir, cx);
        });
        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), Some(&moved_file_path));
            assert_eq!(view.selected_paths(), vec![moved_file_path.clone()]);
            assert!(view.contains_path(&moved_file_path));
            assert!(!view.contains_path(&file_path));
        });
        assert!(!file_path.exists());
        assert!(moved_file_path.exists());

        assert!(events.borrow().contains(&FileTreeEvent::FileSystemChanged {
            path: moved_file_path.clone(),
            kind: FileSystemEventKind::Renamed {
                from: file_path,
                to: moved_file_path,
            },
        }));
    }

    #[gpui::test]
    async fn entry_drop_moves_file_through_mapped_backend(cx: &mut TestAppContext) {
        let temp_dir = tempfile::tempdir().unwrap();
        let display_root = PathBuf::from("/remote/project");
        let native_source_dir = temp_dir.path().join("src");
        let native_target_dir = temp_dir.path().join("crates");
        let native_file_path = native_source_dir.join("lib.rs");
        let native_moved_file_path = native_target_dir.join("lib.rs");
        std::fs::create_dir_all(&native_source_dir).unwrap();
        std::fs::create_dir_all(&native_target_dir).unwrap();
        std::fs::write(&native_file_path, "pub fn lib() {}\n").unwrap();

        let display_source_dir = display_root.join("src");
        let display_target_dir = display_root.join("crates");
        let display_file_path = display_source_dir.join("lib.rs");
        let display_moved_file_path = display_target_dir.join("lib.rs");
        let backend = path_mapped_workspace_backend(
            local_workspace_backend(),
            WorkspacePathMapping::new(display_root.clone(), temp_dir.path()),
        );
        let view = cx.new(|cx| {
            FileTreeView::new_with_runtime_and_backend(
                display_root.clone(),
                test_config(),
                None,
                backend,
                cx,
            )
        });

        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.select_path(Some(display_file_path.clone()), cx);
            view.handle_entry_move_requested(&display_file_path, &display_target_dir, cx);
        });
        cx.run_until_parked();

        view.read_with(cx, |view, _cx| {
            assert_eq!(view.selected_path(), Some(&display_moved_file_path));
            assert_eq!(view.selected_paths(), vec![display_moved_file_path.clone()]);
            assert!(view.contains_path(&display_moved_file_path));
            assert!(!view.contains_path(&display_file_path));
        });
        assert!(!native_file_path.exists());
        assert!(native_moved_file_path.exists());
    }
}

impl EventEmitter<FileTreeEvent> for FileTreeView {}

impl Focusable for FileTreeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// FileTreeView is focusable through its focus_handle field

// FileTreeView uses nucleotide-ui design patterns without implementing traits
// The component is already well-structured with ListItem usage

impl Render for FileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Use nucleotide-ui theme access for consistent styling
        let theme = cx.theme().clone();
        let entries = self.tree.visible_entries();
        let density = self.tree.config().density;
        let vcs_revision = cx.global::<VcsServiceHandle>().status_revision(cx);
        let presentation = self.resolve_presentation_cache(&entries, density, vcs_revision, cx);
        let width_measure_item_index = presentation.width_measure_item_index;
        let content_width = presentation.content_width;
        let vcs_statuses = presentation.vcs_statuses;

        // (debug logging removed)

        // Use FileTreeTokens from hybrid color system for chrome background
        let file_tree_tokens = if self.tree.config().translucent_background {
            theme.tokens.file_tree_tokens().translucent_sidebar()
        } else {
            theme.tokens.file_tree_tokens()
        };
        let bg_color = file_tree_tokens.background;

        // Create semantic file tree container with nucleotide-ui design tokens
        div()
            .id("file-tree")
            .key_context(FILE_TREE_CONTEXT)
            .w_full()
            .h_full()
            .min_h(px(0.0))
            .bg(bg_color) // Use semantic background color from design tokens
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            // Header removed; render list below
            .on_click(cx.listener(|view, _event, window, cx| {
                // Focus the tree view when clicked anywhere on it
                debug!("File tree container clicked, focusing");
                view.focus_handle.focus(window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    let root_path = view.tree.root_path().to_path_buf();
                    view.handle_project_tree_row_event(
                        ProjectTreeRowEvent::context_menu_for_path(root_path, true),
                        Some(event.position),
                        event.click_count,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            // Handle FileTree actions
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::SelectNext, _window, cx| {
                    view.select_next(cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::SelectPrev, _window, cx| {
                    view.select_previous(cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::StartSearch, _window, cx| {
                    view.request_search(cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::ClearSearch, _window, cx| {
                    view.clear_search_query(cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::SelectNextSearchMatch, _window, cx| {
                    view.select_next_search_match(cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::SelectPrevSearchMatch, _window, cx| {
                    view.select_previous_search_match(cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::ToggleExpanded, _window, cx| {
                    // For left/right arrow keys, handle expand/collapse
                    if let Some(selected_path) = view.selected_path.clone()
                        && let Some(entry) = view.tree.entry_by_path(&selected_path)
                        && entry.is_directory()
                    {
                        view.toggle_directory(&selected_path, cx);
                    }
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::OpenFile, _window, cx| {
                    view.open_selected(cx);
                },
            ))
            .on_action(
                cx.listener(|view, _: &crate::actions::file_tree::Delete, _window, cx| {
                    view.request_delete_selected(cx);
                }),
            )
            .on_action(
                cx.listener(|view, _: &crate::actions::file_tree::Rename, _window, cx| {
                    view.request_selected_operation(ProjectTreeContextMenuIntent::Rename, cx);
                }),
            )
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::NewFile, _window, cx| {
                    view.request_selected_operation(ProjectTreeContextMenuIntent::NewFile, cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::NewFolder, _window, cx| {
                    view.request_selected_operation(ProjectTreeContextMenuIntent::NewFolder, cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::Duplicate, _window, cx| {
                    view.request_selected_operation(ProjectTreeContextMenuIntent::Duplicate, cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::CopyPath, _window, cx| {
                    view.request_selected_operation(ProjectTreeContextMenuIntent::CopyPath, cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::CopyRelativePath, _window, cx| {
                    view.request_selected_operation(
                        ProjectTreeContextMenuIntent::CopyRelativePath,
                        cx,
                    );
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::RevealInOs, _window, cx| {
                    view.request_selected_operation(ProjectTreeContextMenuIntent::RevealInOs, cx);
                },
            ))
            .child(
                // Zed-style: wrap the list row in a flex_1 container with min_h(0)
                div()
                    .flex_1()
                    .w_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child({
                        let list = uniform_list("file-tree-list", entries.len(), {
                            let entries = entries.clone();
                            let vcs_statuses = vcs_statuses.clone();
                            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                                let mut items = Vec::with_capacity(range.end - range.start);
                                for index in range {
                                    if let Some(entry) = entries.get(index) {
                                        let vcs_status = vcs_statuses.get(index).copied().flatten();
                                        items.push(this.render_entry(entry, vcs_status, cx));
                                    }
                                }
                                items
                            })
                        })
                        .with_sizing_behavior(gpui::ListSizingBehavior::Infer)
                        .with_width_from_item(width_measure_item_index)
                        .track_scroll(&self.scroll_handle)
                        .w_full()
                        .h_full();

                        div()
                            .relative()
                            .w_full()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("file-tree-horizontal-scroll")
                                    .size_full()
                                    .min_w(px(0.0))
                                    .min_h(px(0.0))
                                    .overflow_x_scroll()
                                    .map(|mut scroll_container| {
                                        scroll_container.style().restrict_scroll_to_axis =
                                            Some(true);
                                        scroll_container
                                    })
                                    .track_scroll(&self.horizontal_scroll_handle)
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w(px(content_width))
                                            .h_full()
                                            .min_h(px(0.0))
                                            .child(list),
                                    ),
                            )
                            .when_some(
                                Scrollbar::vertical(self.vertical_scrollbar_state.clone()),
                                |container, scrollbar| {
                                    container.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .right_0()
                                            .bottom_0()
                                            .w(SCROLLBAR_THICKNESS)
                                            .child(scrollbar),
                                    )
                                },
                            )
                    }),
            )
    }
}
