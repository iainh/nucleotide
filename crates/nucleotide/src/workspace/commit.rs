//! Commit review state and save-before-stage coordination. Git work never runs on the UI thread.
mod changes;
mod history;
mod menu;
mod message;
mod split;
mod submit;
#[cfg(test)]
mod tests;
mod view;

use super::*;
pub(crate) use menu::ReviewMenuIntent;
use nucleotide_vcs::repository::{
    CommitMessageContext, FileVersions, GitRepository, UpstreamState,
};
use nucleotide_vcs::staging::{
    FileChanges, MAX_LINE_STAGING_BYTES, Selection, SelectionState, supports_line_staging,
};
use nucleotide_workspace::{
    GitStatusEntry, GitStatusKind as Status, GitStatusOptions, ReadOptions,
};

/// What the sidebar lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Sidebar {
    /// The project tree.
    #[default]
    Files,
    /// Changed files, whose diffs the Commit tab shows for staging.
    Changes,
    /// Commits drawn as a graph; the Commit tab shows the selected one.
    History,
}

#[derive(Default)]
pub(super) struct CommitReview {
    snapshot: Option<ReviewSnapshot>,
    busy: bool,
    pending: Option<PendingStage>,
    error: Option<String>,
    collapsed: HashSet<PathBuf>,
    scroll: Option<gpui::ListState>,
    refresh_again: bool,
    focus: Option<FocusHandle>,
    control_focus: HashMap<String, FocusHandle>,
    controls: Vec<(FocusHandle, Arc<ReviewFile>, StageChange)>,
    collapse_prefix: bool,
    messages: HashMap<(WorkspaceIdentity, PathBuf), CommitDraft>,
    operation: Option<&'static str>,
    /// Stack removals above additions instead of showing HEAD and the working tree side by side.
    unified: bool,
    /// Unfolded unchanged runs of side-by-side diffs: file, staged preview, first HEAD line.
    expanded: HashSet<(PathBuf, bool, u32)>,
    /// The HEAD column's share of side-by-side diff width; half when unset.
    split: Option<f32>,
    /// Height of the commit message editor; five rows when unset.
    message_height: Option<Pixels>,
    /// Pointer y and editor height when the message divider drag began.
    message_resize: Option<(Pixels, Pixels)>,
    /// The file whose diff is shown. The first file when unset or no longer changed.
    selected: Option<PathBuf>,
    /// The file the diff list was last laid out for, to reset its scroll on a new selection.
    shown: Option<PathBuf>,
    /// What the sidebar lists.
    pub(super) sidebar: Sidebar,
    /// The commit graph of the History sidebar, and the commit the Commit tab shows from it.
    pub(super) history: history::History,
    /// Changes shown at once but not yet written to the index, by file. They
    /// are staged when the operation in progress finishes.
    queued: HashMap<PathBuf, StageTarget>,
    /// The next reload follows a stage, which leaves HEAD and the message
    /// defaults as they were.
    reuse_context: bool,
    /// Git status changed while a reload ran, so it may already be stale.
    status_changed: bool,
    /// The last reload raced another Git process and is being read again.
    raced: bool,
    /// The review is not on screen and reloads when it next shows.
    stale: bool,
    /// Waits for edits to pause before reloading.
    debounce: Option<gpui::Task<()>>,
    /// Horizontal scroll of each diff's columns, shared by all its list items.
    scrolls: HashMap<String, [gpui::ScrollHandle; 2]>,
    /// The list item holding each working-tree hunk of the shown file.
    hunk_items: Vec<usize>,
    /// The items and row height the list last measured.
    laid_out: (Vec<view::ReviewItem>, Pixels),
    /// The right-click menu of a changed file, or of one of its hunks.
    menu: ContextMenuController,
    menu_target: Option<(Arc<ReviewFile>, Option<usize>)>,
    menu_popup: Option<(Entity<PopupMenu>, Subscription)>,
    /// Changes to discard once confirmed and no other Git work runs.
    discard: Option<menu::Discard>,
}

impl CommitReview {
    /// Shows queued changes over a freshly read review. A change is dropped
    /// once the index holds it, or when the HEAD or working-tree text it was
    /// chosen against has changed.
    fn keep_queued(&mut self, snapshot: &mut ReviewSnapshot) {
        let shown = self.snapshot.as_ref();
        self.queued.retain(|path, target| {
            let Some(file) = snapshot
                .files
                .iter_mut()
                .find(|file| &file.entry.relative_path == path)
            else {
                return false;
            };
            let before = shown
                .and_then(|s| s.files.iter().find(|f| &f.entry.relative_path == path))
                .map(|f| &f.content);
            let unchanged = match (before, &file.content) {
                (Some(Ok(before)), Ok(now)) => {
                    before.head == now.head && before.worktree == now.worktree
                }
                // Whole-file actions do not depend on line numbers.
                (Some(Err(_)), Err(_)) => matches!(target, StageTarget::File(_)),
                _ => false,
            };
            let keep = unchanged
                && match target {
                    StageTarget::Selection(selection) => file
                        .content
                        .as_ref()
                        .ok()
                        .and_then(|c| c.selection.as_ref())
                        .is_some_and(|now| now != selection),
                    StageTarget::File(staged) => file.state() != target_state(*staged),
                };
            if keep {
                *file = file.with_target(target);
            }
            keep
        });
    }
}

fn target_state(staged: bool) -> SelectionState {
    if staged {
        SelectionState::Staged
    } else {
        SelectionState::Unstaged
    }
}

/// Whether the review lists exactly the files git status reports. The review
/// also lists files changed only in unsaved buffers. The review's files are
/// sorted by path; git status is not, as it lists untracked files last and
/// orders paths by bytes, so its entries are sorted here and both are merged.
fn files_match_status(files: &[Arc<ReviewFile>], entries: &[GitStatusEntry]) -> bool {
    let mut entries: Vec<_> = entries.iter().collect();
    entries.sort_unstable_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let mut entries = entries.into_iter().peekable();
    for file in files {
        match entries.peek() {
            Some(entry) if entry.relative_path < file.entry.relative_path => return false,
            Some(entry) if entry.relative_path == file.entry.relative_path => {
                if **entry != file.entry {
                    return false;
                }
                entries.next();
            }
            _ => {
                let unsaved_only = file.unsaved
                    && file.entry.index_status == Status::Unmodified
                    && file.entry.working_tree_status == Status::Modified;
                if !unsaved_only {
                    return false;
                }
            }
        }
    }
    entries.next().is_none()
}

struct CommitDraft {
    document: DocumentId,
    before_amend: Option<String>,
    amend_head: Option<String>,
}

struct ReviewSnapshot {
    repository: GitRepository,
    backend: WorkspaceBackendHandle,
    directory: PathBuf,
    head: Option<String>,
    branch: String,
    files: Vec<Arc<ReviewFile>>,
    truncated: bool,
    index_signature: Vec<u8>,
    message_context: Result<CommitMessageContext, String>,
    upstream: Result<UpstreamState, String>,
}

struct ReviewFile {
    entry: GitStatusEntry,
    unsaved: bool,
    content: Result<FileContent, String>,
    /// Read-only diff of a file staged only as a whole, such as a deletion or rename.
    preview: Option<Preview>,
    /// The state of a queued whole-file change, shown until the index holds it.
    shown: Option<SelectionState>,
}

struct Preview {
    changes: FileChanges,
    old_len: u32,
    highlights: [Option<split::LineHighlights>; 2],
}

/// HEAD against what a deletion or rename leaves, when both are text.
async fn load_preview(
    repository: &GitRepository,
    backend: &WorkspaceBackendHandle,
    entry: &GitStatusEntry,
    loader: Option<&helix_core::syntax::Loader>,
) -> Option<Preview> {
    let path = &entry.relative_path;
    let deleted = [entry.index_status, entry.working_tree_status].contains(&Status::Deleted);
    let renamed_from = entry.original_relative_path.as_deref();
    if !deleted && renamed_from.is_none() {
        return None;
    }
    let old = repository
        .read_head(renamed_from.unwrap_or(path))
        .await
        .ok()??;
    let new = if deleted {
        Vec::new()
    } else {
        match read_worktree(backend, &repository.root().join(path)).await {
            Ok(bytes) => bytes,
            Err(_) => repository.read_index(path).await.ok()??,
        }
    };
    if !supports_line_staging(&[&old, &new])
        || std::str::from_utf8(&old).is_err()
        || std::str::from_utf8(&new).is_err()
    {
        return None;
    }
    let highlight = |text: &[u8]| split::line_highlights(text, path, loader?);
    Some(Preview {
        changes: FileChanges::new(&old, &new),
        old_len: old.split_inclusive(|b| *b == b'\n').count() as u32,
        highlights: [highlight(&old), highlight(&new)],
    })
}

#[derive(Clone)]
struct FileContent {
    head: Option<Vec<u8>>,
    index: Option<Vec<u8>>,
    worktree: Vec<u8>,
    changes: FileChanges,
    selection: Option<Selection>,
    staged_diff: Option<FileChanges>,
    /// Syntax highlights of the HEAD, index (with `staged_diff`) and working-tree lines.
    highlights: [Option<Arc<split::LineHighlights>>; 3],
}

impl ReviewFile {
    fn state(&self) -> SelectionState {
        if let Some(state) = self.shown {
            return state;
        }
        if let Ok(content) = &self.content
            && let Some(selection) = &content.selection
            && !content.changes.hunks().is_empty()
        {
            return content.changes.selection_state(selection);
        }
        if matches!(
            self.entry.index_status,
            Status::Unmodified | Status::Untracked
        ) {
            SelectionState::Unstaged
        } else if self.entry.working_tree_status == Status::Unmodified && !self.unsaved {
            SelectionState::Staged
        } else {
            SelectionState::Partial
        }
    }

    /// This file as shown once `target` is staged, before the index holds it.
    fn with_target(&self, target: &StageTarget) -> Arc<Self> {
        let mut content = self.content.clone();
        let mut shown = None;
        if let Ok(content) = &mut content
            && content.selection.is_some()
        {
            content.selection = Some(match target {
                StageTarget::Selection(selection) => selection.clone(),
                StageTarget::File(true) => content.changes.select_all(),
                StageTarget::File(false) => Selection::default(),
            });
        } else if let StageTarget::File(staged) = target {
            shown = Some(target_state(*staged));
        }
        Arc::new(Self {
            entry: self.entry.clone(),
            unsaved: self.unsaved,
            content,
            preview: None,
            shown,
        })
    }

    fn paths(&self) -> Vec<&Path> {
        let mut paths = vec![self.entry.relative_path.as_path()];
        if let Some(original) = &self.entry.original_relative_path {
            paths.push(original);
        }
        paths
    }
}

#[derive(Clone)]
enum StageChange {
    File(bool),
    Hunk(usize, bool),
    Line {
        removal: bool,
        line: u32,
        staged: bool,
    },
}

impl StageChange {
    /// The selection after this line or hunk change, or `None` for a file
    /// change or a file that cannot be staged line by line.
    fn select(&self, content: &FileContent) -> Option<Selection> {
        let mut selection = content.selection.clone()?;
        match *self {
            Self::File(_) => return None,
            Self::Hunk(index, staged) => {
                selection.set_hunk(&content.changes.hunks()[index], staged)
            }
            Self::Line {
                removal: true,
                line,
                staged,
            } => selection.set_removal(line, staged),
            Self::Line {
                removal: false,
                line,
                staged,
            } => selection.set_addition(line, staged),
        }
        Some(selection)
    }

    fn control_id(&self, file: usize) -> String {
        match self {
            Self::File(_) => format!("commit-file-{file}"),
            Self::Hunk(hunk, _) => format!("commit-hunk-{file}-{hunk}"),
            Self::Line { removal, line, .. } => format!("commit-line-{file}-{removal}-{line}"),
        }
    }
}

/// What a stage writes for one file.
#[derive(Clone, PartialEq)]
enum StageTarget {
    /// Stage or unstage the whole file.
    File(bool),
    /// Make the index hold exactly these changed lines.
    Selection(Selection),
}

struct PendingStage {
    repository: GitRepository,
    backend: WorkspaceBackendHandle,
    directory: PathBuf,
    head: Option<String>,
    files: Vec<(Arc<ReviewFile>, StageTarget)>,
    saves: HashMap<DocumentId, u64>,
}

async fn read_worktree(backend: &WorkspaceBackendHandle, path: &Path) -> Result<Vec<u8>, String> {
    let stat = backend.stat(path).await.map_err(|e| e.to_string())?;
    if stat.kind != FileKind::File || stat.size > MAX_LINE_STAGING_BYTES as u64 {
        return Err("Whole-file staging only (non-regular or large file)".into());
    }
    let read = backend
        .read_file(
            path,
            ReadOptions {
                max_bytes: Some(MAX_LINE_STAGING_BYTES as u64),
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    if read.truncated {
        return Err("Whole-file staging only (large file)".into());
    }
    Ok(read.bytes)
}

/// Line-staging contents of each entry, reading Git with a fixed number of
/// processes. Highlights are reused from `previous` where the text is unchanged.
async fn load_contents(
    repository: &GitRepository,
    backend: &WorkspaceBackendHandle,
    entries: &[&GitStatusEntry],
    buffers: &HashMap<PathBuf, Vec<u8>>,
    loader: Option<&helix_core::syntax::Loader>,
    previous: &[Arc<ReviewFile>],
) -> Vec<Result<FileContent, String>> {
    let whole_file = |entry: &GitStatusEntry| {
        [entry.index_status, entry.working_tree_status]
            .iter()
            .any(|s| {
                matches!(
                    s,
                    Status::Deleted
                        | Status::Renamed
                        | Status::Copied
                        | Status::TypeChanged
                        | Status::Conflicted
                )
            })
    };
    let paths: Vec<_> = entries
        .iter()
        .filter(|entry| !whole_file(entry))
        .map(|entry| entry.relative_path.as_path())
        .collect();
    let mut versions = match repository.read_versions(&paths).await {
        Ok(versions) => versions.into_iter(),
        Err(error) => vec![Err(error); paths.len()].into_iter(),
    };
    let mut contents = Vec::new();
    for entry in entries {
        if whole_file(entry) {
            contents.push(Err(
                "Whole-file staging only (deletion, rename, type change or conflict)".into(),
            ));
            continue;
        }
        let versions = versions.next().expect("one version per path");
        let previous = previous
            .iter()
            .find(|file| file.entry.relative_path == entry.relative_path)
            .and_then(|file| file.content.as_ref().ok());
        let buffer = buffers.get(&repository.root().join(&entry.relative_path));
        contents.push(
            load_content(
                repository, backend, entry, versions, buffer, loader, previous,
            )
            .await,
        );
    }
    contents
}

async fn load_content(
    repository: &GitRepository,
    backend: &WorkspaceBackendHandle,
    entry: &GitStatusEntry,
    versions: Result<FileVersions, String>,
    buffer: Option<&Vec<u8>>,
    loader: Option<&helix_core::syntax::Loader>,
    previous: Option<&FileContent>,
) -> Result<FileContent, String> {
    let path = &entry.relative_path;
    let stat = backend
        .stat(&repository.root().join(path))
        .await
        .map_err(|e| e.to_string())?;
    let FileVersions {
        head,
        index,
        mode_changed,
    } = versions?;
    if stat.kind != FileKind::File || mode_changed {
        return Err("Whole-file staging only (file mode change or non-regular file)".into());
    }
    let worktree = match buffer {
        Some(bytes) => bytes.clone(),
        None => read_worktree(backend, &repository.root().join(path)).await?,
    };
    if !supports_line_staging(&[
        head.as_deref().unwrap_or_default(),
        index.as_deref().unwrap_or_default(),
        &worktree,
    ]) || [
        &worktree,
        head.as_ref().unwrap_or(&worktree),
        index.as_ref().unwrap_or(&worktree),
    ]
    .iter()
    .any(|bytes| std::str::from_utf8(bytes).is_err())
    {
        return Err("Whole-file staging only (binary, non-UTF-8 or large file)".into());
    }
    let changes = FileChanges::new(head.as_deref().unwrap_or_default(), &worktree);
    let selection = changes.selection_from_index(index.as_deref().unwrap_or_default());
    let staged_diff = selection.is_none().then(|| {
        FileChanges::new(
            head.as_deref().unwrap_or_default(),
            index.as_deref().unwrap_or_default(),
        )
    });
    // Staging changes only the index, so the HEAD and working-tree highlights
    // usually carry over from the previous review.
    let highlight = |side: usize, text: Option<&Vec<u8>>| {
        if let Some(previous) = previous {
            let old = [
                previous.head.as_ref(),
                previous.index.as_ref(),
                Some(&previous.worktree),
            ];
            if old[side] == text && (side != 1 || previous.highlights[1].is_some()) {
                return previous.highlights[side].clone();
            }
        }
        split::line_highlights(text.map_or(&[][..], Vec::as_slice), path, loader?).map(Arc::new)
    };
    let highlights = [
        highlight(0, head.as_ref()),
        staged_diff
            .as_ref()
            .and_then(|_| highlight(1, index.as_ref())),
        highlight(2, Some(&worktree)),
    ];
    Ok(FileContent {
        head,
        index,
        worktree,
        changes,
        selection,
        staged_diff,
        highlights,
    })
}

/// What a reload can reuse from the review it replaces.
#[derive(Default)]
struct Previous {
    files: Vec<Arc<ReviewFile>>,
    /// HEAD with its message defaults and upstream, reused while HEAD is unchanged.
    context: Option<ReviewContext>,
}

/// Another Git process moved HEAD or the index while a review was read.
const RACED: &str = "HEAD or index changed while loading. Refresh to review the new changes.";

type ReviewContext = (
    Option<String>,
    Result<CommitMessageContext, String>,
    Result<UpstreamState, String>,
);

async fn load_review(
    backend: WorkspaceBackendHandle,
    directory: PathBuf,
    buffers: HashMap<PathBuf, Vec<u8>>,
    loader: Option<Arc<helix_core::syntax::Loader>>,
    previous: Previous,
) -> Result<ReviewSnapshot, String> {
    let repository = GitRepository::discover(backend.clone(), &directory)
        .await?
        .ok_or("This workspace is not a Git repository")?;
    let head = backend
        .git_head(repository.root())
        .await
        .map_err(|e| e.to_string())?;
    let index_signature = repository.index_signature().await?;
    let (message_context, upstream) = match previous.context {
        Some((reused, message_context, upstream)) if reused == head.head => {
            (message_context, upstream)
        }
        _ => (
            repository.message_context().await,
            repository.upstream_state().await,
        ),
    };
    let mut status = backend
        .git_status(repository.root(), GitStatusOptions::default())
        .await
        .map_err(|e| e.to_string())?;
    // A tracked file can be clean on disk but modified only in an open buffer.
    for path in buffers.keys() {
        if let Ok(relative) = path.strip_prefix(repository.root())
            && !status.entries.iter().any(|e| e.relative_path == relative)
            && repository.read_head(relative).await?.is_some()
        {
            status.entries.push(GitStatusEntry {
                relative_path: relative.into(),
                original_relative_path: None,
                index_status: Status::Unmodified,
                working_tree_status: Status::Modified,
            });
        }
    }
    status
        .entries
        .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let entries: Vec<_> = status.entries.iter().collect();
    let contents = load_contents(
        &repository,
        &backend,
        &entries,
        &buffers,
        loader.as_deref(),
        &previous.files,
    )
    .await;
    let mut files = Vec::new();
    for (entry, content) in status.entries.into_iter().zip(contents) {
        let unsaved = buffers.contains_key(&repository.root().join(&entry.relative_path));
        let preview = match content {
            Ok(_) => None,
            Err(_) => load_preview(&repository, &backend, &entry, loader.as_deref()).await,
        };
        files.push(Arc::new(ReviewFile {
            entry,
            unsaved,
            content,
            preview,
            shown: None,
        }));
    }
    if repository.index_signature().await? != index_signature
        || backend
            .git_head(repository.root())
            .await
            .map_err(|e| e.to_string())?
            .head
            != head.head
    {
        return Err(RACED.into());
    }
    Ok(ReviewSnapshot {
        repository,
        backend,
        directory,
        head: head.head,
        branch: head.display_ref.unwrap_or_else(|| "No commits yet".into()),
        files,
        truncated: status.truncated,
        index_signature,
        message_context,
        upstream,
    })
}

impl PendingStage {
    async fn apply(self) -> Result<(), String> {
        if self
            .backend
            .git_head(self.repository.root())
            .await
            .map_err(|e| e.to_string())?
            .head
            != self.head
        {
            return Err("HEAD changed. Review the refreshed changes before staging.".into());
        }
        // Validate all partial selections before making any index writes.
        let entries: Vec<_> = self.files.iter().map(|(file, _)| &file.entry).collect();
        let currents = load_contents(
            &self.repository,
            &self.backend,
            &entries,
            &HashMap::new(),
            None,
            &[],
        )
        .await;
        let mut replacements = Vec::new();
        for ((file, target), current) in self.files.iter().zip(currents) {
            let path = &file.entry.relative_path;
            let StageTarget::Selection(selection) = target else {
                if let Ok(content) = &file.content {
                    let current = current?;
                    if content.head != current.head
                        || content.index != current.index
                        || (file.unsaved && content.worktree != current.worktree)
                    {
                        return Err(format!(
                            "{} changed since review. Nothing staged; review the refreshed changes.",
                            path.display()
                        ));
                    }
                }
                replacements.push(None);
                continue;
            };
            let content = file.content.as_ref().map_err(Clone::clone)?;
            let current = current?;
            if content.head != current.head
                || content.index != current.index
                || content.worktree != current.worktree
            {
                return Err(format!(
                    "{} changed since review. Nothing staged; review the refreshed changes.",
                    path.display()
                ));
            }
            replacements.push(Some((
                content.changes.index_content(selection),
                content.changes.selection_state(selection),
            )));
        }
        for ((file, target), replacement) in self.files.iter().zip(replacements) {
            let path = &file.entry.relative_path;
            match (target, replacement) {
                (StageTarget::File(true), _) => self.repository.stage(&file.paths()).await?,
                (StageTarget::File(false), _) => self.repository.unstage(&file.paths()).await?,
                (_, Some((_, SelectionState::Unstaged)))
                    if file.content.as_ref().is_ok_and(|c| c.head.is_none()) =>
                {
                    self.repository.unstage(&[path]).await?;
                }
                (_, Some((bytes, _))) => self.repository.write_index(path, bytes).await?,
                _ => unreachable!(),
            }
        }
        Ok(())
    }
}

impl Workspace {
    pub(super) fn active_commit_focus(&self) -> Option<FocusHandle> {
        (self
            .pane_host
            .layout
            .pane(self.focused_pane_id())?
            .active_tab()
            == Some(TabId::Commit))
        .then(|| {
            self.commit_review
                .focus
                .clone()
                .filter(|_| !self.pane_host.commit_message_focused)
        })
        .flatten()
    }

    fn commit_review_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_manager.is_document_view_focused(cx, window) {
            return;
        }
        if event.keystroke.modifiers != gpui::Modifiers::none() {
            return;
        }
        let key = event.keystroke.key.as_str();
        if !matches!(
            key,
            "j" | "k" | "down" | "up" | "[" | "]" | "space" | "enter" | "z" | "a" | "r" | "c"
        ) {
            return;
        }
        cx.stop_propagation();
        if key == "c" {
            self.focus_commit_message(window, cx);
            return;
        }
        if key == "r" {
            self.commit_review.error = None;
            self.refresh_commit_review(cx);
            return;
        }
        let index = self
            .commit_review
            .controls
            .iter()
            .position(|(focus, _, _)| focus.is_focused(window));
        let collapse = std::mem::take(&mut self.commit_review.collapse_prefix);
        if key == "z" {
            self.commit_review.collapse_prefix = true;
            return;
        }
        let count = self.commit_review.controls.len();
        if count == 0 {
            return;
        }
        let current = index.unwrap_or(0);
        if matches!(key, "[" | "]") {
            // Files are shown one at a time; move the selection and focus its checkbox.
            if let Some((selected, _)) = self.selected_review_file() {
                let next = if key == "]" {
                    selected + 1
                } else {
                    selected.wrapping_sub(1)
                };
                if let Some(file) = self
                    .commit_review
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.files.get(next))
                {
                    self.commit_review.selected = Some(file.entry.relative_path.clone());
                    let focus = self
                        .commit_review
                        .control_focus
                        .entry(StageChange::File(true).control_id(next))
                        .or_insert_with(|| cx.focus_handle())
                        .clone();
                    window.focus(&focus, cx);
                }
            }
        } else if matches!(key, "j" | "k" | "down" | "up") {
            let next = match key {
                "k" | "up" => current.saturating_sub(1),
                _ => index.map_or(0, |i| (i + 1).min(count - 1)),
            };
            // Its list item may be off screen and not yet rendered.
            let (focus, file, change) = &self.commit_review.controls[next];
            if let (Some(item), Some(scroll)) = (
                self.review_item_of(file, change),
                &self.commit_review.scroll,
            ) {
                scroll.scroll_to_reveal_item(item);
            }
            window.focus(&focus.clone(), cx);
        } else {
            let (_, file, change) = self.commit_review.controls[current].clone();
            match key {
                "space" if !event.is_held => {
                    if matches!(change, StageChange::File(_))
                        || file.content.as_ref().is_ok_and(|c| c.selection.is_some())
                    {
                        self.stage_review_files(vec![(file, change)], cx);
                    }
                }
                "enter" => {
                    if let Some(snapshot) = &self.commit_review.snapshot {
                        let line = match change {
                            StageChange::Line { line, .. } => line,
                            StageChange::Hunk(index, _) => file
                                .content
                                .as_ref()
                                .map(|c| c.changes.hunks()[index].after.start)
                                .unwrap_or(0),
                            StageChange::File(_) => 0,
                        };
                        let path = snapshot.repository.root().join(&file.entry.relative_path);
                        self.open_file_at(&path, Position::new(line as usize, 0), cx);
                    }
                }
                "a" if collapse
                    && !self
                        .commit_review
                        .collapsed
                        .remove(&file.entry.relative_path) =>
                {
                    self.commit_review
                        .collapsed
                        .insert(file.entry.relative_path.clone());
                }
                _ => {}
            }
        }
        cx.notify();
    }

    /// The list item that shows a control of the shown file.
    fn review_item_of(&self, file: &ReviewFile, change: &StageChange) -> Option<usize> {
        let hunks = file.content.as_ref().ok()?.changes.hunks();
        let hunk = match *change {
            StageChange::File(_) => return Some(0),
            StageChange::Hunk(hunk, _) => hunk,
            StageChange::Line { removal, line, .. } => hunks.iter().position(|hunk| {
                if removal {
                    hunk.before.contains(&line)
                } else {
                    hunk.after.contains(&line)
                }
            })?,
        };
        self.commit_review.hunk_items.get(hunk).copied()
    }

    pub(super) fn commit_tab_pane(&self) -> Option<PaneId> {
        self.pane_host.layout.panes().into_iter().find(|id| {
            self.pane_host
                .layout
                .pane(*id)
                .is_some_and(|pane| pane.tabs().contains(&TabId::Commit))
        })
    }

    /// The changed file whose diff the Commit tab shows, with its index in the review.
    fn selected_review_file(&self) -> Option<(usize, Arc<ReviewFile>)> {
        let files = &self.commit_review.snapshot.as_ref()?.files;
        let index = self
            .commit_review
            .selected
            .as_ref()
            .and_then(|path| files.iter().position(|f| &f.entry.relative_path == path))
            .unwrap_or(0);
        Some((index, files.get(index)?.clone()))
    }

    pub(super) fn open_commit_tab(&mut self, cx: &mut Context<Self>) {
        self.show_commit_tab(cx);
        self.refresh_commit_review(cx);
    }

    /// Activates the Commit tab, opening it when needed, without reloading the review.
    fn show_commit_tab(&mut self, cx: &mut Context<Self>) {
        self.commit_review.sidebar = Sidebar::Changes;
        self.commit_review.history.in_tab = false;
        self.activate_commit_tab(cx);
    }

    /// Activates the Commit tab, opening it when needed, for whichever of the
    /// review or a history commit it shows.
    fn activate_commit_tab(&mut self, cx: &mut Context<Self>) {
        self.commit_review
            .focus
            .get_or_insert_with(|| cx.focus_handle());
        let pane = self
            .commit_tab_pane()
            .unwrap_or_else(|| self.focused_pane_id());
        let _ = self.pane_host.layout.activate_pane(pane);
        let _ = self.pane_host.layout.open_tab(pane, TabId::Commit, true);
        self.hide_completions(cx);
        self.project_panes(cx);
        self.update_document_views(cx);
        self.invalidate_tab_bar_documents();
        self.defer_focus_editor(cx);
        cx.notify();
    }

    pub(super) fn close_commit_tab(&mut self, cx: &mut Context<Self>) {
        // The History list stays useful without a commit on screen.
        if self.commit_review.sidebar == Sidebar::Changes {
            self.commit_review.sidebar = Sidebar::Files;
        }
        self.commit_review.history.in_tab = false;
        if let Some(pane) = self.commit_tab_pane() {
            let _ = self.pane_host.layout.close_tab(pane, TabId::Commit);
        }
        self.project_panes(cx);
        self.update_document_views(cx);
        self.invalidate_tab_bar_documents();
        self.defer_focus_editor(cx);
        cx.notify();
    }

    pub(super) fn refresh_commit_review(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.commit_tab_pane() else {
            return;
        };
        self.commit_review.debounce = None;
        // A hidden review reloads when it next shows instead of on every change.
        let in_tab = !self.commit_review.history.in_tab
            && self
                .pane_host
                .layout
                .pane(pane)
                .and_then(|p| p.active_tab())
                == Some(TabId::Commit);
        if self.commit_review.sidebar != Sidebar::Changes && !in_tab {
            self.commit_review.stale = true;
            return;
        }
        self.commit_review.stale = false;
        if self.commit_review.busy {
            self.commit_review.refresh_again = true;
            return;
        }
        let core = self.core.read(cx);
        let Some(directory) = core.project_directory.clone() else {
            return;
        };
        let backend = core.workspace_backend.clone();
        let loader = Some(core.editor.syn_loader.load_full());
        let buffers = core
            .editor
            .documents
            .values()
            .filter(|doc| doc.is_modified())
            .filter_map(|doc| {
                Some((
                    doc.path()?.to_path_buf(),
                    doc.text().to_string().into_bytes(),
                ))
            })
            .collect();
        if self
            .commit_review
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.directory != directory || snapshot.backend.identity() != backend.identity()
            })
        {
            self.commit_review.snapshot = None;
            self.pane_host.commit_message_focused = false;
            self.project_panes(cx);
            self.update_document_views(cx);
        }
        self.commit_review.busy = true;
        let reuse_context = std::mem::take(&mut self.commit_review.reuse_context);
        let previous = self
            .commit_review
            .snapshot
            .as_ref()
            .map(|snapshot| Previous {
                files: snapshot.files.clone(),
                context: reuse_context.then(|| {
                    (
                        snapshot.head.clone(),
                        snapshot.message_context.clone(),
                        snapshot.upstream.clone(),
                    )
                }),
            })
            .unwrap_or_default();
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    handle.block_on(load_review(backend, directory, buffers, loader, previous))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.commit_review.busy = false;
                let raced = std::mem::take(&mut this.commit_review.raced);
                match result {
                    // Keep showing the review read before and read once more.
                    Err(error) if error == RACED && !raced => {
                        this.commit_review.raced = true;
                        this.commit_review.refresh_again = true;
                    }
                    Err(error) if error == RACED && this.commit_review.snapshot.is_some() => {
                        this.commit_review.error = Some(error);
                    }
                    Ok(snapshot) => {
                        if this.core.read(cx).project_directory.as_ref()
                            != Some(&snapshot.directory)
                            || this.core.read(cx).workspace_backend.identity()
                                != snapshot.backend.identity()
                        {
                            this.commit_review.refresh_again = true;
                        } else {
                            // Collapse large files when they first appear; after
                            // that, the user's expand or collapse stands.
                            let shown: HashSet<_> = this
                                .commit_review
                                .snapshot
                                .iter()
                                .flat_map(|s| &s.files)
                                .map(|f| &f.entry.relative_path)
                                .collect();
                            for file in &snapshot.files {
                                if shown.contains(&file.entry.relative_path) {
                                    continue;
                                }
                                if file.content.as_ref().is_ok_and(|c| {
                                    c.changes
                                        .hunks()
                                        .iter()
                                        .map(|h| h.before.len() + h.after.len())
                                        .sum::<usize>()
                                        > view::MAX_VISIBLE_LINES
                                }) {
                                    this.commit_review
                                        .collapsed
                                        .insert(file.entry.relative_path.clone());
                                }
                            }
                            this.ensure_commit_message(&snapshot, cx);
                            let mut snapshot = snapshot;
                            this.commit_review.keep_queued(&mut snapshot);
                            this.commit_review.snapshot = Some(snapshot);
                        }
                    }
                    Err(error) => {
                        this.commit_review.snapshot = None;
                        this.commit_review.queued.clear();
                        this.commit_review.error = Some(error);
                        this.pane_host.commit_message_focused = false;
                        this.project_panes(cx);
                        this.update_document_views(cx);
                    }
                }
                if std::mem::take(&mut this.commit_review.status_changed)
                    && !this.review_matches_status(cx)
                {
                    this.commit_review.refresh_again = true;
                }
                if std::mem::take(&mut this.commit_review.refresh_again) {
                    this.refresh_commit_review(cx);
                }
                this.stage_queued(cx);
                this.start_discard(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Reloads after edits once they pause, rather than on every keystroke.
    pub(super) fn refresh_commit_review_soon(&mut self, cx: &mut Context<Self>) {
        if self.commit_tab_pane().is_none() {
            return;
        }
        // Replacing the task cancels the previous wait.
        self.commit_review.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| this.refresh_commit_review(cx));
        }));
    }

    /// Reloads when git status no longer matches the review. Our own stage
    /// changes the status too, and its reload has usually already read it.
    pub(super) fn commit_status_changed(&mut self, cx: &mut Context<Self>) {
        // A commit changes the status as well as the history.
        self.history_changed(cx);
        if self.commit_review.busy {
            self.commit_review.status_changed = true;
        } else if !self.review_matches_status(cx) {
            self.refresh_commit_review(cx);
        }
    }

    fn review_matches_status(&self, cx: &App) -> bool {
        let Some(snapshot) = &self.commit_review.snapshot else {
            return false;
        };
        let service = cx.global::<VcsServiceHandle>().service().read(cx);
        service.repository_head() == snapshot.head.as_deref()
            && service.repository_ref().unwrap_or("No commits yet") == snapshot.branch
            && files_match_status(&snapshot.files, service.status_entries())
    }

    /// Shows each change at once and stages it, after the operation in
    /// progress when there is one. Clicks made meanwhile fold into one write.
    fn stage_review_files(
        &mut self,
        files: Vec<(Arc<ReviewFile>, StageChange)>,
        cx: &mut Context<Self>,
    ) {
        if self.commit_review.operation.is_some() {
            return;
        }
        let Some(snapshot) = &mut self.commit_review.snapshot else {
            return;
        };
        for (file, change) in files {
            let path = &file.entry.relative_path;
            let Some(shown) = snapshot
                .files
                .iter_mut()
                .find(|f| &f.entry.relative_path == path)
            else {
                continue;
            };
            let target = match change {
                StageChange::File(staged) => StageTarget::File(staged),
                // Build on the shown selection, which includes clicks not yet staged.
                change => match shown.content.as_ref().ok().and_then(|c| change.select(c)) {
                    Some(selection) => StageTarget::Selection(selection),
                    None => continue,
                },
            };
            *shown = shown.with_target(&target);
            self.commit_review.queued.insert(path.clone(), target);
        }
        self.stage_queued(cx);
        cx.notify();
    }

    fn stage_queued(&mut self, cx: &mut Context<Self>) {
        if self.commit_review.busy || self.commit_review.queued.is_empty() {
            return;
        }
        let Some(snapshot) = &self.commit_review.snapshot else {
            return;
        };
        let files = std::mem::take(&mut self.commit_review.queued)
            .into_iter()
            .filter_map(|(path, target)| {
                let file = snapshot
                    .files
                    .iter()
                    .find(|f| f.entry.relative_path == path)?;
                Some((file.clone(), target))
            })
            .collect();
        self.start_stage(files, cx);
    }

    fn start_stage(&mut self, files: Vec<(Arc<ReviewFile>, StageTarget)>, cx: &mut Context<Self>) {
        if self.commit_review.busy || files.is_empty() {
            return;
        }
        let Some(snapshot) = &self.commit_review.snapshot else {
            return;
        };
        if self.core.read(cx).project_directory.as_ref() != Some(&snapshot.directory) {
            self.refresh_commit_review(cx);
            return;
        }
        let paths: HashSet<_> = files
            .iter()
            .flat_map(|(file, _)| file.paths())
            .map(|path| snapshot.repository.root().join(path))
            .collect();
        let mut pending = PendingStage {
            repository: snapshot.repository.clone(),
            backend: snapshot.backend.clone(),
            directory: snapshot.directory.clone(),
            head: snapshot.head.clone(),
            files,
            saves: HashMap::new(),
        };
        // Use the editor's ordinary save queue, including the remote save handler.
        let handle = self.handle.clone();
        let result = self.core.update(cx, |core, _| -> Result<(), String> {
            let _guard = handle.enter();
            let documents: Vec<_> = core
                .editor
                .documents
                .values_mut()
                .filter(|doc| doc.is_modified() && doc.path().is_some_and(|p| paths.contains(p)))
                .map(|doc| (doc.id(), doc.get_current_revision() as u64))
                .collect();
            for (id, revision) in documents {
                core.editor
                    .save::<PathBuf>(id, None, false)
                    .map_err(|e| e.to_string())?;
                pending.saves.insert(id, revision);
            }
            core.request_event_driven_maintenance();
            Ok(())
        });
        self.commit_review.error = None;
        if let Err(error) = result {
            self.finish_staging(Err(format!("Save failed; nothing staged: {error}")), cx);
            return;
        }
        self.commit_review.busy = true;
        self.commit_review.pending = Some(pending);
        self.apply_pending_stage(cx);
        cx.notify();
    }

    pub(super) fn commit_save_failed(&mut self, error: &str, cx: &mut Context<Self>) {
        // Helix errors carry no document ID. While waiting for saves, fail closed
        // even when an unrelated queued save might have produced this error.
        if self.commit_review.pending.is_some() {
            self.commit_review.pending = None;
            self.finish_staging(Err(format!("Save failed; nothing staged: {error}")), cx);
        }
    }

    pub(super) fn commit_document_saved(
        &mut self,
        id: DocumentId,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = &mut self.commit_review.pending
            && pending.saves.get(&id) == Some(&revision)
        {
            if self
                .core
                .read(cx)
                .editor
                .document(id)
                .is_none_or(|doc| doc.is_modified())
            {
                self.commit_review.pending = None;
                self.finish_staging(
                    Err("Buffer changed while saving. Nothing staged; review again.".into()),
                    cx,
                );
                return;
            }
            pending.saves.remove(&id);
            self.apply_pending_stage(cx);
        } else {
            self.refresh_commit_review(cx);
        }
    }

    fn apply_pending_stage(&mut self, cx: &mut Context<Self>) {
        if self
            .commit_review
            .pending
            .as_ref()
            .is_none_or(|p| !p.saves.is_empty())
        {
            return;
        }
        let Some(pending) = self.commit_review.pending.take() else {
            return;
        };
        if self.core.read(cx).project_directory.as_ref() != Some(&pending.directory) {
            self.finish_staging(Err("Workspace changed; nothing staged.".into()), cx);
            return;
        }
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { handle.block_on(pending.apply()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.commit_review.reuse_context = true;
                this.finish_staging(result, cx)
            });
        })
        .detach();
    }

    fn finish_staging(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.commit_review.busy = false;
        self.commit_review.refresh_again = false;
        self.commit_review.error = result.err();
        if let Some(error) = &self.commit_review.error {
            // Show what the index holds rather than clicks that may no longer apply.
            self.commit_review.queued.clear();
            self.core
                .update(cx, |core, _| core.editor.set_error(error.clone()));
        }
        let service = cx.global::<VcsServiceHandle>().service().clone();
        service.update(cx, |service, cx| service.refresh_now(cx));
        self.refresh_commit_review(cx);
        cx.notify();
    }
}
