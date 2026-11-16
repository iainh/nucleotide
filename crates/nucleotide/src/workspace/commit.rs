//! Commit review state and save-before-stage coordination. Git work never runs on the UI thread.
mod message;
mod submit;
#[cfg(test)]
mod tests;
mod view;

use super::*;
use nucleotide_vcs::repository::{CommitMessageContext, GitRepository, UpstreamState};
use nucleotide_vcs::staging::{
    FileChanges, MAX_LINE_STAGING_BYTES, Selection, SelectionState, supports_line_staging,
};
use nucleotide_workspace::{
    GitStatusEntry, GitStatusKind as Status, GitStatusOptions, ReadOptions,
};

#[derive(Default)]
pub(super) struct CommitReview {
    snapshot: Option<ReviewSnapshot>,
    busy: bool,
    pending: Option<PendingStage>,
    error: Option<String>,
    collapsed: HashSet<PathBuf>,
    scroll: ScrollHandle,
    refresh_again: bool,
    focus: Option<FocusHandle>,
    control_focus: HashMap<String, FocusHandle>,
    controls: Vec<(FocusHandle, Arc<ReviewFile>, StageChange)>,
    collapse_prefix: bool,
    messages: HashMap<(WorkspaceIdentity, PathBuf), CommitDraft>,
    operation: Option<&'static str>,
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
}

struct FileContent {
    head: Option<Vec<u8>>,
    index: Option<Vec<u8>>,
    worktree: Vec<u8>,
    changes: FileChanges,
    selection: Option<Selection>,
    staged_diff: Option<FileChanges>,
}

impl ReviewFile {
    fn state(&self) -> SelectionState {
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

struct PendingStage {
    repository: GitRepository,
    backend: WorkspaceBackendHandle,
    directory: PathBuf,
    head: Option<String>,
    files: Vec<(Arc<ReviewFile>, StageChange)>,
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

async fn load_content(
    repository: &GitRepository,
    backend: &WorkspaceBackendHandle,
    entry: &GitStatusEntry,
    buffer: Option<&Vec<u8>>,
) -> Result<FileContent, String> {
    if [entry.index_status, entry.working_tree_status]
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
    {
        return Err("Whole-file staging only (deletion, rename, type change or conflict)".into());
    }
    let path = &entry.relative_path;
    let stat = backend
        .stat(&repository.root().join(path))
        .await
        .map_err(|e| e.to_string())?;
    if stat.kind != FileKind::File || repository.has_mode_change(path).await? {
        return Err("Whole-file staging only (file mode change or non-regular file)".into());
    }
    let head = repository.read_head(path).await?;
    let index = repository.read_index(path).await?;
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
    Ok(FileContent {
        head,
        index,
        worktree,
        changes,
        selection,
        staged_diff,
    })
}

async fn load_review(
    backend: WorkspaceBackendHandle,
    directory: PathBuf,
    buffers: HashMap<PathBuf, Vec<u8>>,
) -> Result<ReviewSnapshot, String> {
    let repository = GitRepository::discover(backend.clone(), &directory)
        .await?
        .ok_or("This workspace is not a Git repository")?;
    let head = backend
        .git_head(repository.root())
        .await
        .map_err(|e| e.to_string())?;
    let index_signature = repository.index_signature().await?;
    let message_context = repository.message_context().await;
    let upstream = repository.upstream_state().await;
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
    let mut files = Vec::new();
    for entry in status.entries {
        let buffer = buffers.get(&repository.root().join(&entry.relative_path));
        let content = load_content(&repository, &backend, &entry, buffer).await;
        files.push(Arc::new(ReviewFile {
            entry,
            unsaved: buffer.is_some(),
            content,
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
        return Err(
            "HEAD or index changed while loading. Refresh to review the new changes.".into(),
        );
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
        let mut replacements = Vec::new();
        for (file, change) in &self.files {
            let path = &file.entry.relative_path;
            if let StageChange::File(_) = change {
                if let Ok(content) = &file.content {
                    let current =
                        load_content(&self.repository, &self.backend, &file.entry, None).await?;
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
            }
            let content = file.content.as_ref().map_err(Clone::clone)?;
            let current = load_content(&self.repository, &self.backend, &file.entry, None).await?;
            if content.head != current.head
                || content.index != current.index
                || content.worktree != current.worktree
            {
                return Err(format!(
                    "{} changed since review. Nothing staged; review the refreshed changes.",
                    path.display()
                ));
            }
            let mut selection = content
                .selection
                .clone()
                .ok_or("Staged version differs from working tree")?;
            match change {
                StageChange::Hunk(index, staged) => {
                    selection.set_hunk(&content.changes.hunks()[*index], *staged)
                }
                StageChange::Line {
                    removal: true,
                    line,
                    staged,
                } => selection.set_removal(*line, *staged),
                StageChange::Line {
                    removal: false,
                    line,
                    staged,
                } => selection.set_addition(*line, *staged),
                StageChange::File(_) => unreachable!(),
            }
            replacements.push(Some((
                content.changes.index_content(&selection),
                content.changes.selection_state(&selection),
            )));
        }
        for ((file, change), replacement) in self.files.iter().zip(replacements) {
            let path = &file.entry.relative_path;
            match (change, replacement) {
                (StageChange::File(true), _) => self.repository.stage(&file.paths()).await?,
                (StageChange::File(false), _) => self.repository.unstage(&file.paths()).await?,
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
        if self.commit_review.busy {
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
        if matches!(key, "j" | "k" | "down" | "up" | "[" | "]") {
            let next = match key {
                "k" | "up" => current.saturating_sub(1),
                "j" | "down" => index.map_or(0, |i| (i + 1).min(count - 1)),
                "]" => ((current + 1)..count)
                    .find(|i| matches!(self.commit_review.controls[*i].2, StageChange::File(_)))
                    .unwrap_or(current),
                _ => (0..current)
                    .rev()
                    .find(|i| matches!(self.commit_review.controls[*i].2, StageChange::File(_)))
                    .unwrap_or(current),
            };
            let (focus, file, _) = &self.commit_review.controls[next];
            window.focus(focus, cx);
            if let Some(snapshot) = &self.commit_review.snapshot
                && let Some(index) = snapshot
                    .files
                    .iter()
                    .position(|f| f.entry.relative_path == file.entry.relative_path)
            {
                self.commit_review.scroll.scroll_to_item(index);
            }
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

    pub(super) fn commit_tab_pane(&self) -> Option<PaneId> {
        self.pane_host.layout.panes().into_iter().find(|id| {
            self.pane_host
                .layout
                .pane(*id)
                .is_some_and(|pane| pane.tabs().contains(&TabId::Commit))
        })
    }

    pub(super) fn open_commit_tab(&mut self, cx: &mut Context<Self>) {
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
        self.refresh_commit_review(cx);
        cx.notify();
    }

    pub(super) fn close_commit_tab(&mut self, cx: &mut Context<Self>) {
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
        if self.commit_tab_pane().is_none() {
            return;
        }
        if self.commit_review.busy {
            self.commit_review.refresh_again = true;
            return;
        }
        let core = self.core.read(cx);
        let Some(directory) = core.project_directory.clone() else {
            return;
        };
        let backend = core.workspace_backend.clone();
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
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { handle.block_on(load_review(backend, directory, buffers)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.commit_review.busy = false;
                match result {
                    Ok(snapshot) => {
                        if this.core.read(cx).project_directory.as_ref()
                            != Some(&snapshot.directory)
                            || this.core.read(cx).workspace_backend.identity()
                                != snapshot.backend.identity()
                        {
                            this.commit_review.refresh_again = true;
                        } else {
                            for file in &snapshot.files {
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
                            this.commit_review.snapshot = Some(snapshot);
                        }
                    }
                    Err(error) => {
                        this.commit_review.snapshot = None;
                        this.commit_review.error = Some(error);
                        this.pane_host.commit_message_focused = false;
                        this.project_panes(cx);
                        this.update_document_views(cx);
                    }
                }
                if std::mem::take(&mut this.commit_review.refresh_again) {
                    this.refresh_commit_review(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn stage_review_files(
        &mut self,
        files: Vec<(Arc<ReviewFile>, StageChange)>,
        cx: &mut Context<Self>,
    ) {
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
            let _ = this.update(cx, |this, cx| this.finish_staging(result, cx));
        })
        .detach();
    }

    fn finish_staging(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.commit_review.busy = false;
        self.commit_review.refresh_again = false;
        self.commit_review.error = result.err();
        if let Some(error) = &self.commit_review.error {
            self.core
                .update(cx, |core, _| core.editor.set_error(error.clone()));
        }
        let service = cx.global::<VcsServiceHandle>().service().clone();
        service.update(cx, |service, cx| service.refresh_now(cx));
        self.refresh_commit_review(cx);
        cx.notify();
    }
}
