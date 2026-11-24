//! User-triggered repository mutations. A push failure never retries the commit.
use super::*;

impl Workspace {
    fn commit_draft(&self) -> Option<&CommitDraft> {
        let snapshot = self.commit_review.snapshot.as_ref()?;
        self.commit_review.messages.get(&(
            snapshot.backend.identity(),
            snapshot.repository.root().to_path_buf(),
        ))
    }

    pub(super) fn commit_amending(&self) -> bool {
        self.commit_draft()
            .is_some_and(|draft| draft.before_amend.is_some())
    }

    pub(super) fn commit_amend_stale(&self) -> bool {
        self.commit_draft().is_some_and(|draft| {
            draft.amend_head.is_some()
                && self
                    .commit_review
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.head != draft.amend_head)
        })
    }

    pub(super) fn can_commit(&self, cx: &Context<Self>) -> bool {
        let Some(snapshot) = &self.commit_review.snapshot else {
            return false;
        };
        let Some(draft) = self.commit_draft() else {
            return false;
        };
        let Ok(context) = &snapshot.message_context else {
            return false;
        };
        !self.commit_review.busy
            && !self.commit_amend_stale()
            && (self.commit_amending() || !snapshot.index_signature.is_empty())
            && !snapshot
                .files
                .iter()
                .any(|file| file.entry.index_status == Status::Conflicted)
            && self
                .core
                .read(cx)
                .editor
                .document(draft.document)
                .is_some_and(|doc| {
                    doc.text().to_string().lines().any(|line| {
                        !line.starts_with(&context.comment_prefix) && !line.trim().is_empty()
                    })
                })
    }

    pub(super) fn can_push(&self) -> bool {
        !self.commit_review.busy
            && self
                .commit_review
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| {
                    snapshot.head.is_some()
                        && snapshot.upstream.as_ref().is_ok_and(|state| {
                            state.branch.is_some() && (state.ahead > 0 || state.upstream.is_none())
                        })
                })
    }

    pub(super) fn toggle_commit_amend(&mut self, cx: &mut Context<Self>) {
        if self.commit_review.busy {
            return;
        }
        let Some(snapshot) = &self.commit_review.snapshot else {
            return;
        };
        let Some(last_message) = snapshot
            .message_context
            .as_ref()
            .ok()
            .and_then(|c| c.last_message.clone())
        else {
            return;
        };
        let Some((document, slot)) = self.pane_host.commit_message else {
            return;
        };
        let key = (
            snapshot.backend.identity(),
            snapshot.repository.root().to_path_buf(),
        );
        let Some(draft) = self.commit_review.messages.get_mut(&key) else {
            return;
        };
        let text = if let Some(previous) = draft.before_amend.take() {
            draft.amend_head = None;
            previous
        } else {
            draft.amend_head = snapshot.head.clone();
            draft.before_amend = self
                .core
                .read(cx)
                .editor
                .document(document)
                .map(|doc| doc.text().to_string());
            last_message
        };
        self.replace_commit_message(document, slot, &text, cx);
    }

    pub(super) fn commit_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let primary = if cfg!(target_os = "macos") {
            modifiers.platform && !modifiers.control
        } else {
            modifiers.control && !modifiers.platform
        };
        if event.keystroke.key == "enter" && primary && !modifiers.shift {
            cx.stop_propagation();
            if !event.is_held {
                self.submit_commit(modifiers.alt, cx);
            }
        } else if event.keystroke.key.eq_ignore_ascii_case("a")
            && modifiers.alt
            && !modifiers.control
            && !modifiers.platform
            && !modifiers.shift
        {
            cx.stop_propagation();
            if !event.is_held {
                self.toggle_commit_amend(cx);
            }
        } else {
            self.commit_message_escape(event, window, cx);
        }
    }

    pub(super) fn submit_commit(&mut self, push: bool, cx: &mut Context<Self>) {
        if !self.can_commit(cx) {
            return;
        }
        self.run_commit_action(Some(push), cx);
    }

    pub(super) fn push_commit(&mut self, cx: &mut Context<Self>) {
        if !self.can_push() {
            return;
        }
        self.run_commit_action(None, cx);
    }

    fn run_commit_action(&mut self, commit: Option<bool>, cx: &mut Context<Self>) {
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
        let branch = snapshot.branch.clone();
        let signature = snapshot.index_signature.clone();
        let identity = backend.identity();
        let key = (identity.clone(), repository.root().to_path_buf());
        let message = self.pane_host.commit_message.and_then(|(document, slot)| {
            core.editor
                .document(document)
                .map(|doc| (document, slot, doc.text().to_string()))
        });
        let amend = self.commit_amending();
        let documents: Vec<_> = core
            .editor
            .documents()
            .filter_map(|doc| {
                let path = doc.path()?;
                Some((
                    doc.id(),
                    path.to_path_buf(),
                    path.strip_prefix(repository.root()).ok()?.to_path_buf(),
                ))
            })
            .collect();
        self.commit_review.busy = true;
        self.commit_review.error = None;
        self.commit_review.operation = Some(if commit.is_some() {
            "Committing…"
        } else {
            "Pushing…"
        });
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            let repo = repository.clone();
            let message_text = message.as_ref().map(|(_, _, text)| text.clone());
            let identity_for_check = identity.clone();
            let backend_for_check = backend.clone();
            let result = cx.background_executor().spawn(async move {
                handle.block_on(async {
                    let current = backend_for_check.git_head(repo.root()).await.map_err(|e| e.to_string())?;
                    if current.head != head || current.display_ref.as_deref().unwrap_or("No commits yet") != branch
                        || (commit.is_some() && repo.index_signature().await? != signature) {
                        return Err("HEAD or index changed since review. Refresh before continuing.".to_string());
                    }
                    if commit.is_some()
                        && let Err(error) = repo.commit(message_text.as_deref().unwrap_or_default(), amend).await {
                            let after = backend_for_check.git_head(repo.root()).await;
                            return Err(if after.as_ref().is_ok_and(|after| after.head == current.head) {
                                error
                            } else {
                                format!("{error}\nCommit outcome is uncertain. Inspect Git history before retrying.")
                            });
                    }
                    backend_for_check.git_head(repo.root()).await.map_err(|error| format!("Cannot confirm repository state: {error}. Inspect Git history before retrying."))
                })
            }).await;
            let expected_head = match result {
                Ok(head) => head,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| this.finish_commit_action(Err(error), cx));
                    return;
                }
            };
            if commit.is_some() {
                // Clear only the submitted text. Editing during hooks belongs to
                // the next draft and must never be discarded by this completion.
                let _ = this.update(cx, |this, cx| {
                    if let Some(draft) = this.commit_review.messages.get_mut(&key) {
                        draft.before_amend = None;
                        draft.amend_head = None;
                    }
                    if let Some((document, slot, text)) = &message
                        && this.core.read(cx).editor.document(*document).is_some_and(|doc| doc.text() == text.as_str()) {
                        this.replace_commit_message(*document, *slot, "", cx);
                    }
                    this.commit_review.operation = Some(if commit == Some(true) { "Commit created. Pushing…" } else { "Commit created. Refreshing…" });
                    cx.notify();
                });
                let repo = repository.clone();
                let handle = this.update(cx, |this, _| this.handle.clone()).ok();
                if let Some(handle) = handle {
                    let bases = cx.background_executor().spawn(async move {
                        handle.block_on(async {
                            let mut bases = Vec::new();
                            for (id, path, relative) in documents {
                                bases.push((id, path, repo.read_head(&relative).await));
                            }
                            bases
                        })
                    }).await;
                    let _ = this.update(cx, |this, cx| {
                        let handle = this.handle.clone();
                        this.core.update(cx, |core, _| {
                            let _guard = handle.enter();
                            if core.workspace_backend.identity() != identity_for_check { return; }
                            for (id, path, base) in bases {
                                if let Ok(base) = base && let Some(doc) = core.editor.document_mut(id)
                                    && doc.path() == Some(&path) {
                                    doc.set_diff_base(base.unwrap_or_default());
                                }
                            }
                        });
                        this.update_document_views(cx);
                    });
                }
            }
            let result = if commit != Some(false) {
                let handle = this.update(cx, |this, _| this.handle.clone()).ok();
                if let Some(handle) = handle {
                    cx.background_executor().spawn(async move { handle.block_on(async {
                        let current = backend.git_head(repository.root()).await.map_err(|e| e.to_string())?;
                        if current.head != expected_head.head || current.display_ref != expected_head.display_ref {
                            return Err("HEAD changed before push. Inspect the branch before retrying.".into());
                        }
                        repository.push().await
                    }) }).await
                        .map_err(|error| if commit.is_some() {
                            format!("Commit created, but push failed. The local commit is kept. Use Push to retry; do not commit again.\n{error}")
                        } else { error })
                } else { return; }
            } else { Ok(()) };
            let _ = this.update(cx, |this, cx| this.finish_commit_action(result.map(|_| {
                if commit == Some(false) { "Commit created" } else { "Push completed" }
            }), cx));
        }).detach();
        cx.notify();
    }

    fn finish_commit_action(
        &mut self,
        result: Result<&'static str, String>,
        cx: &mut Context<Self>,
    ) {
        self.commit_review.operation = None;
        let status = match &result {
            Ok(message) => {
                // A push moves remote branches without moving HEAD.
                self.history_changed(cx);
                // Replace the previous failure in Helix too, or the next
                // editor event republishes it after a successful retry.
                self.core
                    .update(cx, |core, _| core.editor.set_status(*message));
                EditorStatus {
                    status: (*message).into(),
                    severity: Severity::Info,
                }
            }
            Err(error) => EditorStatus {
                status: error.clone(),
                severity: Severity::Error,
            },
        };
        self.push_editor_status_notification(status, cx);
        self.finish_staging(result.map(|_| ()), cx);
    }
}
