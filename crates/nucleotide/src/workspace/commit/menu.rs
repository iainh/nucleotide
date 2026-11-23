//! The right-click menu of a changed file or one of its hunks, and discarding
//! changes after the user confirms.
use super::*;
use nucleotide_workspace::WriteOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewMenuIntent {
    Stage,
    Unstage,
    Discard,
    StageHunk,
    UnstageHunk,
    DiscardHunk,
    OpenFile,
    CopyPath,
    CopyRelativePath,
    RevealInFileTree,
}

impl ReviewMenuIntent {
    fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Unstage => "Unstage",
            Self::Discard => "Discard Changes…",
            Self::StageHunk => "Stage Hunk",
            Self::UnstageHunk => "Unstage Hunk",
            Self::DiscardHunk => "Discard Hunk…",
            Self::OpenFile => "Open File",
            Self::CopyPath => "Copy Path",
            Self::CopyRelativePath => "Copy Relative Path",
            Self::RevealInFileTree => "Reveal in File Tree",
        }
    }
}

/// The menu items, `None` for a separator. A hunk menu replaces the
/// whole-file Git actions with ones for the hunk.
fn review_menu_entries(hunk: bool) -> Vec<Option<ReviewMenuIntent>> {
    use ReviewMenuIntent::*;
    let git = if hunk {
        [StageHunk, UnstageHunk, DiscardHunk]
    } else {
        [Stage, Unstage, Discard]
    };
    let mut entries = vec![Some(git[0]), Some(git[1]), None, Some(git[2]), None];
    entries.extend(
        [OpenFile, CopyPath, CopyRelativePath, RevealInFileTree]
            .into_iter()
            .map(Some),
    );
    entries
}

/// Changes the user asked to discard, run once confirmed and Git is idle.
pub(super) struct Discard {
    file: Arc<ReviewFile>,
    hunk: Option<usize>,
    confirmed: bool,
}

impl ReviewFile {
    fn deleted(&self) -> bool {
        [self.entry.index_status, self.entry.working_tree_status].contains(&Status::Deleted)
    }

    fn hunk(&self, hunk: usize) -> Option<&nucleotide_vcs::staging::StagingHunk> {
        self.content.as_ref().ok()?.changes.hunks().get(hunk)
    }

    fn hunk_state(&self, hunk: usize) -> Option<SelectionState> {
        let content = self.content.as_ref().ok()?;
        Some(content.selection.as_ref()?.hunk_state(self.hunk(hunk)?))
    }
}

/// Discards one hunk from the working tree and the index, failing without
/// writing anything when the file changed since `file` was read.
async fn discard_hunk(
    repository: &GitRepository,
    backend: &WorkspaceBackendHandle,
    file: &ReviewFile,
    hunk: usize,
) -> Result<(), String> {
    let path = &file.entry.relative_path;
    let content = file.content.as_ref().map_err(Clone::clone)?;
    let (Some(selection), Some(hunk)) = (&content.selection, file.hunk(hunk)) else {
        return Err("This hunk can no longer be discarded. Review the refreshed changes.".into());
    };
    let current = load_contents(
        repository,
        backend,
        &[&file.entry],
        &HashMap::new(),
        None,
        &[],
    )
    .await
    .pop()
    .expect("one content per entry")?;
    if content.head != current.head
        || content.index != current.index
        || content.worktree != current.worktree
    {
        return Err(format!(
            "{} changed since review. Nothing discarded; review the refreshed changes.",
            path.display()
        ));
    }
    let mut kept = content.changes.select_all();
    kept.set_hunk(hunk, false);
    backend
        .write_file(
            &repository.root().join(path),
            &content.changes.index_content(&kept),
            WriteOptions::default(),
        )
        .await
        .map_err(|e| e.to_string())?;
    if selection.hunk_state(hunk) == SelectionState::Unstaged {
        return Ok(());
    }
    let mut staged = selection.clone();
    staged.set_hunk(hunk, false);
    if content.head.is_none()
        && content.changes.selection_state(&staged) == SelectionState::Unstaged
    {
        repository.unstage(&[path]).await
    } else {
        repository
            .write_index(path, content.changes.index_content(&staged))
            .await
    }
}

impl Workspace {
    fn open_review_menu(
        &mut self,
        file: Arc<ReviewFile>,
        hunk: Option<usize>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.close_tab_bar_menus();
        self.close_file_tree_context_menu(cx);
        self.close_editor_context_menu();
        self.close_review_menu();
        self.commit_review
            .menu
            .open_at((f32::from(position.x), f32::from(position.y)));
        self.commit_review.menu_target = Some((file, hunk));
        cx.notify();
    }

    /// Opens the menu of `file`, or of its hunk, on a right click.
    pub(super) fn review_menu_listener(
        &self,
        file: &Arc<ReviewFile>,
        hunk: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        let file = file.clone();
        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            this.open_review_menu(file.clone(), hunk, event.position, cx);
            cx.stop_propagation();
        })
    }

    /// A changed row of the working-tree diff that opens its hunk's menu.
    pub(super) fn with_hunk_menu(
        &self,
        row: gpui::AnyElement,
        diff: &split::SplitDiff,
        hunk: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match diff.staging {
            Some((_, file, _)) => div()
                .on_mouse_down(
                    MouseButton::Right,
                    self.review_menu_listener(file, Some(hunk), cx),
                )
                .child(row)
                .into_any_element(),
            None => row,
        }
    }

    fn close_review_menu(&mut self) -> bool {
        let closed = self.commit_review.menu.close();
        let had_menu = self.commit_review.menu_popup.take().is_some();
        let had_target = self.commit_review.menu_target.take().is_some();
        closed || had_menu || had_target
    }

    /// Hands focus back to the review, where the keyboard controls live.
    fn focus_review(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self
            .commit_review
            .focus
            .clone()
            .filter(|_| self.commit_tab_pane().is_some())
            .unwrap_or_else(|| self.focus_handle.clone());
        window.focus(&focus, cx);
    }

    fn dismiss_review_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_review_menu() {
            self.focus_review(window, cx);
            cx.notify();
        }
    }

    fn intent_disabled(&self, intent: ReviewMenuIntent, file: &ReviewFile, hunk: usize) -> bool {
        use ReviewMenuIntent::*;
        let locked = self.commit_review.operation.is_some() || self.commit_review.discard.is_some();
        let state = file.state();
        match intent {
            Stage => locked || state == SelectionState::Staged,
            Unstage => locked || state == SelectionState::Unstaged,
            Discard => locked,
            StageHunk => {
                locked
                    || file
                        .hunk_state(hunk)
                        .is_none_or(|s| s == SelectionState::Staged)
            }
            UnstageHunk => {
                locked
                    || file
                        .hunk_state(hunk)
                        .is_none_or(|s| s == SelectionState::Unstaged)
            }
            // Discarding writes the file, which would also save unrelated unsaved edits.
            DiscardHunk => locked || file.unsaved || file.hunk_state(hunk).is_none(),
            OpenFile => file.deleted(),
            RevealInFileTree => file.deleted() || self.file_tree.is_none(),
            CopyPath | CopyRelativePath => false,
        }
    }

    /// The menu at the last right click, built once per opening.
    pub(in crate::workspace) fn render_review_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.commit_review.menu.is_open() {
            return None;
        }
        let (file, hunk) = self.commit_review.menu_target.clone()?;
        let menu = match &self.commit_review.menu_popup {
            Some((menu, _)) => menu.clone(),
            None => {
                let items: Vec<_> = review_menu_entries(hunk.is_some())
                    .into_iter()
                    .map(|entry| {
                        entry.map(|intent| {
                            (
                                intent,
                                self.intent_disabled(intent, &file, hunk.unwrap_or(0)),
                            )
                        })
                    })
                    .collect();
                let action_context = self.focus_handle.clone();
                let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
                    menu = menu.action_context(action_context);
                    for item in items {
                        menu = match item {
                            Some((intent, disabled)) => menu.menu_with_check_and_disabled(
                                intent.label(),
                                false,
                                Box::new(crate::actions::commit_menu::Operation { intent }),
                                disabled,
                            ),
                            None => menu.separator(),
                        };
                    }
                    menu
                });
                let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
                    if this.close_review_menu() {
                        cx.notify();
                    }
                });
                self.commit_review.menu_popup = Some((menu.clone(), subscription));
                menu
            }
        };
        let menu_focus = menu.read(cx).focus_handle(cx);
        if !menu_focus.contains_focused(window, cx) {
            menu_focus.focus(window, cx);
        }
        Some(Self::render_popup_menu_backdrop(
            menu,
            self.commit_review.menu.position(),
            Anchor::TopLeft,
            (0.0, 0.0),
            cx,
            Workspace::dismiss_review_menu,
        ))
    }

    pub(in crate::workspace) fn review_menu_operation(
        &mut self,
        intent: ReviewMenuIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.commit_review.menu_target.clone();
        self.close_review_menu();
        cx.notify();
        let Some((file, hunk)) = target else {
            return;
        };
        let Some(root) = self
            .commit_review
            .snapshot
            .as_ref()
            .map(|s| s.repository.root().to_path_buf())
        else {
            return;
        };
        let path = root.join(&file.entry.relative_path);
        // The review may have reloaded since the menu opened. Act on a hunk
        // only while the shown file still has the same one.
        let current = self.commit_review.snapshot.as_ref().and_then(|s| {
            s.files
                .iter()
                .find(|f| f.entry.relative_path == file.entry.relative_path)
                .cloned()
        });
        let same_hunk = |hunk: usize| {
            current.as_ref().is_some_and(|current| {
                current.hunk(hunk).is_some() && current.hunk(hunk) == file.hunk(hunk)
            })
        };
        use ReviewMenuIntent::*;
        match (intent, hunk) {
            (Stage | Unstage, _) => {
                if let Some(current) = current {
                    let change = StageChange::File(intent == Stage);
                    self.stage_review_files(vec![(current, change)], cx);
                }
            }
            (StageHunk | UnstageHunk, Some(hunk)) if same_hunk(hunk) => {
                let change = StageChange::Hunk(hunk, intent == StageHunk);
                self.stage_review_files(vec![(current.expect("checked"), change)], cx);
            }
            (Discard, _) => self.request_discard(file, None, cx),
            (DiscardHunk, Some(hunk)) if same_hunk(hunk) => {
                self.request_discard(file, Some(hunk), cx)
            }
            (OpenFile, _) => {
                let line = hunk.and_then(|h| file.hunk(h)).map_or(0, |h| h.after.start);
                self.open_file_at(&path, Position::new(line as usize, 0), cx);
                return;
            }
            (CopyPath, _) => self.handle_project_tree_operation(
                ProjectTreeContextMenuIntent::CopyPath,
                path,
                false,
                cx,
            ),
            (CopyRelativePath, _) => self.handle_project_tree_operation(
                ProjectTreeContextMenuIntent::CopyRelativePath,
                path,
                false,
                cx,
            ),
            (RevealInFileTree, _) => {
                if let Some(file_tree) = self.file_tree.clone() {
                    self.commit_review.sidebar = false;
                    self.show_file_tree = true;
                    file_tree.update(cx, |tree, cx| {
                        tree.sync_selection_with_file(Some(path.as_path()), cx);
                    });
                }
            }
            _ => {}
        }
        self.focus_review(window, cx);
    }

    fn request_discard(
        &mut self,
        file: Arc<ReviewFile>,
        hunk: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.commit_review.discard.is_some() {
            return;
        }
        self.commit_review.discard = Some(Discard {
            file,
            hunk,
            confirmed: false,
        });
        self.defer_confirmation_modal(cx);
    }

    /// The confirmation for a requested discard, while it is unanswered.
    pub(in crate::workspace) fn discard_confirm_dialog(&self) -> Option<ConfirmDialog> {
        let discard = self
            .commit_review
            .discard
            .as_ref()
            .filter(|d| !d.confirmed)?;
        let entry = &discard.file.entry;
        let path = entry.relative_path.display();
        let (title, message, confirm) = if let Some(hunk) =
            discard.hunk.and_then(|h| discard.file.hunk(h))
        {
            (
                "Discard hunk?".to_string(),
                format!(
                    "Discard the change at @@ −{},{} +{},{} @@ in {path}? It is removed from the working tree and the index. This cannot be undone.",
                    hunk.before.start + 1,
                    hunk.before.len(),
                    hunk.after.start + 1,
                    hunk.after.len()
                ),
                "Discard Hunk",
            )
        } else if entry.index_status == Status::Untracked {
            (
                "Delete untracked file?".to_string(),
                format!("{path} is not tracked by Git. Deleting it cannot be undone."),
                "Delete",
            )
        } else {
            let unsaved = if discard.file.unsaved {
                ", including unsaved edits in its open buffer,"
            } else {
                ""
            };
            let restore = if entry.index_status == Status::Added {
                format!(
                    "{path} is new. Discarding its changes removes it from the index and deletes it."
                )
            } else {
                format!("Restore {path} to its last committed version?")
            };
            (
                "Discard changes?".to_string(),
                format!(
                    "{restore} Staged and unstaged changes{unsaved} will be lost. This cannot be undone."
                ),
                "Discard Changes",
            )
        };
        Some(ConfirmDialog::new(title, message, confirm).confirm_variant(ButtonVariant::Danger))
    }

    pub(in crate::workspace) fn handle_discard_confirmation(
        &mut self,
        event: ConfirmDialogEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ConfirmDialogEvent::Confirmed => {
                if let Some(discard) = &mut self.commit_review.discard {
                    discard.confirmed = true;
                }
                self.start_discard(cx);
            }
            ConfirmDialogEvent::Cancelled => self.commit_review.discard = None,
        }
        cx.notify();
    }

    /// Runs a confirmed discard once no other Git work is in progress.
    pub(super) fn start_discard(&mut self, cx: &mut Context<Self>) {
        if self.commit_review.busy
            || !self
                .commit_review
                .discard
                .as_ref()
                .is_some_and(|d| d.confirmed)
        {
            return;
        }
        let Some(Discard { file, hunk, .. }) = self.commit_review.discard.take() else {
            return;
        };
        let Some(snapshot) = &self.commit_review.snapshot else {
            return;
        };
        let core = self.core.read(cx);
        if core.project_directory.as_ref() != Some(&snapshot.directory)
            || core.workspace_backend.identity() != snapshot.backend.identity()
        {
            self.refresh_commit_review(cx);
            return;
        }
        let repository = snapshot.repository.clone();
        let backend = snapshot.backend.clone();
        let head = snapshot.head.clone();
        let paths: Vec<PathBuf> = file
            .paths()
            .into_iter()
            .map(|p| repository.root().join(p))
            .collect();
        // Buffers to reload, at the revision whose unsaved edits are discarded.
        // Discarding a hunk keeps edits made since, as does a later edit.
        let buffers: Vec<_> = core
            .editor
            .documents()
            .filter(|doc| doc.path().is_some_and(|p| paths.contains(&p.to_path_buf())))
            .filter(|doc| hunk.is_none() || !doc.is_modified())
            .map(|doc| (doc.id(), doc.version()))
            .collect();
        self.commit_review.busy = true;
        self.commit_review.error = None;
        self.commit_review.operation = Some("Discarding changes…");
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            let reload_backend = backend.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    handle.block_on(async {
                        let current = backend
                            .git_head(repository.root())
                            .await
                            .map_err(|e| e.to_string())?;
                        if current.head != head {
                            return Err(
                                "HEAD changed. Nothing discarded; review the refreshed changes."
                                    .to_string(),
                            );
                        }
                        match hunk {
                            Some(hunk) => discard_hunk(&repository, &backend, &file, hunk).await,
                            None if file.entry.index_status == Status::Untracked => {
                                repository.discard(&[], &file.paths()).await
                            }
                            None => repository.discard(&file.paths(), &[]).await,
                        }
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if result.is_ok() {
                    this.reload_discarded_buffers(reload_backend, buffers, cx);
                }
                this.commit_review.operation = None;
                this.finish_staging(result, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Replaces buffers with what the discard left on disk, unless they were
    /// edited meanwhile. A deleted file's buffer keeps its text.
    fn reload_discarded_buffers(
        &mut self,
        backend: WorkspaceBackendHandle,
        buffers: Vec<(DocumentId, i32)>,
        cx: &mut Context<Self>,
    ) {
        for (doc_id, revision) in buffers {
            let Some(path) = self
                .core
                .read(cx)
                .editor
                .document(doc_id)
                .and_then(|doc| doc.path().map(Path::to_path_buf))
            else {
                continue;
            };
            let backend = backend.clone();
            let runtime = self.handle.clone();
            cx.spawn(async move |this, cx| {
                let read = cx
                    .background_executor()
                    .spawn(async move {
                        runtime.block_on(crate::application::read_workspace_document(backend, path))
                    })
                    .await;
                let Ok(read) = read else {
                    return;
                };
                let _ = this.update(cx, |this, cx| {
                    let handle = this.handle.clone();
                    let reloaded = this.core.update(cx, |core, _| {
                        let _guard = handle.enter();
                        let editor = &mut core.editor;
                        let doc = editor.document(doc_id)?;
                        if doc.version() != revision {
                            return None;
                        }
                        let view_id = editor
                            .tree
                            .views()
                            .find(|(view, _)| view.doc == doc_id)
                            .map(|(view, _)| view.id)
                            .or_else(|| {
                                doc.selections()
                                    .keys()
                                    .copied()
                                    .find(|id| editor.tree.contains(*id))
                            })?;
                        crate::application::reload_workspace_document_from_read(
                            editor, doc_id, view_id, read,
                        )
                        .ok()
                    });
                    if reloaded.is_some() {
                        this.update_document_views(cx);
                        this.refresh_commit_review(cx);
                    }
                });
            })
            .detach();
        }
    }
}
