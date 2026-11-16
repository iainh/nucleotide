//! A session-only Helix document per repository, rendered through the ordinary
//! DocumentView so editing, IME, undo, rulers and wrapping keep their contracts.
use super::*;

impl Workspace {
    pub(super) fn ensure_commit_message(
        &mut self,
        snapshot: &ReviewSnapshot,
        cx: &mut Context<Self>,
    ) {
        let key = (
            snapshot.backend.identity(),
            snapshot.repository.root().to_path_buf(),
        );
        let existing = self
            .commit_review
            .messages
            .get(&key)
            .map(|draft| draft.document);
        let slot = self.pane_host.commit_message.map(|(_, slot)| slot);
        let handle = self.handle.clone();
        let result = self.core.update(cx, |core, _| -> Result<_, String> {
            let _guard = handle.enter();
            let document = if let Some(document) = existing {
                document
            } else {
                let loader = core.editor.syn_loader.load();
                if loader.language_for_name("git-commit").is_none() {
                    return Err("git-commit language configuration is unavailable".into());
                }
                let document = core.editor.new_file(helix_view::editor::Action::Load);
                core.editor
                    .register_hosted_auxiliary(document)
                    .map_err(|e| format!("{e:?}"))?;
                core.editor
                    .document_mut(document)
                    .unwrap()
                    .set_language_by_language_id("git-commit", &loader)
                    .map_err(|e| e.to_string())?;
                document
            };
            let slot = if let Some(slot) = slot {
                if core.editor.tree.get(slot).doc != document {
                    core.editor
                        .project_hosted_replace(slot, document)
                        .map_err(|e| format!("{e:?}"))?;
                }
                slot
            } else {
                core.editor
                    .project_hosted_slot(document)
                    .map_err(|e| format!("{e:?}"))?
            };
            Ok((document, slot))
        });
        match result {
            Ok((document, slot)) => {
                self.commit_review
                    .messages
                    .entry(key)
                    .or_insert(CommitDraft {
                        document,
                        before_amend: None,
                        amend_head: None,
                    });
                self.pane_host.commit_message = Some((document, slot));
                if existing.is_none()
                    && let Ok(context) = &snapshot.message_context
                {
                    self.replace_commit_message(document, slot, &context.seed, cx);
                }
                self.project_panes(cx);
                self.update_document_views(cx);
            }
            Err(error) => {
                self.commit_review.error = Some(format!("Cannot open message editor: {error}"));
                self.pane_host.commit_message = None;
                self.pane_host.commit_message_focused = false;
                self.project_panes(cx);
                self.update_document_views(cx);
            }
        }
    }

    pub(super) fn replace_commit_message(
        &mut self,
        document: DocumentId,
        slot: ViewId,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let handle = self.handle.clone();
        self.core.update(cx, |core, _| {
            let _guard = handle.enter();
            let Some(doc) = core.editor.document_mut(document) else {
                return;
            };
            let text = format!("{}\n", text.trim_end_matches('\n'));
            let transaction = helix_core::Transaction::change(
                doc.text(),
                [(0, doc.text().len_chars(), Some(text.into()))].into_iter(),
            )
            .with_selection(helix_core::Selection::point(0));
            doc.apply(&transaction, slot);
        });
        self.update_document_views(cx);
        cx.notify();
    }

    pub(in crate::workspace) fn focus_commit_message(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_review.snapshot.is_none() {
            return;
        }
        let Some((_, slot)) = self.pane_host.commit_message else {
            return;
        };
        self.handle_view_focused(slot, cx);
        if let Some(view) = self.view_manager.get_document_view(&slot) {
            window.focus(&view.focus_handle(cx), cx);
        }
    }

    pub(in crate::workspace) fn focus_commit_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_host.commit_message_focused = false;
        self.project_panes(cx);
        self.update_document_views(cx);
        if let Some(focus) = &self.commit_review.focus {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    pub(in crate::workspace) fn commit_message_escape(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape"
            && !event.keystroke.modifiers.modified()
            && self.pane_host.commit_message_focused
            && self.view_manager.is_document_view_focused(cx, window)
            && self.core.read(cx).editor.mode() == helix_view::document::Mode::Normal
            && !self.overlay.read(cx).has_focusable_overlay()
        {
            self.focus_commit_list(window, cx);
            cx.stop_propagation();
        }
    }

    pub(super) fn render_commit_message(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let message = self.pane_host.commit_message.and_then(|(document, slot)| {
            self.commit_review.snapshot.as_ref()?;
            let core = self.core.read(cx);
            let doc = core.editor.document(document)?;
            let view = self.view_manager.get_document_view(&slot)?.clone();
            let text = doc.text();
            let subject = text
                .line(0)
                .to_string()
                .trim_end_matches(['\r', '\n'])
                .chars()
                .count();
            let long_body = text.lines().skip(1).any(|line| {
                line.to_string()
                    .trim_end_matches(['\r', '\n'])
                    .chars()
                    .count()
                    > 72
            });
            let position = helix_core::coords_at_pos(
                text.slice(..),
                doc.selection(slot).primary().cursor(text.slice(..)),
            );
            Some((view, subject, long_body, position))
        });
        let mut panel = div()
            .id("commit-message-panel")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .border_b_1()
            .border_color(tokens.chrome.border_default);
        if let Some((view, subject, long_body, position)) = message {
            panel = panel
                .child(
                    div()
                        .px(tokens.sizes.space_3)
                        .py(tokens.sizes.space_1)
                        .text_color(tokens.editor.text_secondary)
                        .child(
                            "Commit message · c to edit · Esc in normal mode returns to changes",
                        ),
                )
                .child(
                    div()
                        .id("commit-message-editor")
                        .when(cfg!(test), |view| {
                            view.debug_selector(|| "commit-message-editor".into())
                        })
                        .h(tokens.sizes.space_8 * 5.0)
                        .min_h_0()
                        .overflow_hidden()
                        .child(view),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(tokens.sizes.space_3)
                        .px(tokens.sizes.space_3)
                        .py(tokens.sizes.space_1)
                        .text_color(tokens.editor.text_secondary)
                        .child(
                            div()
                                .when(subject > 50, |row| row.text_color(tokens.editor.warning))
                                .child(format!("Subject {subject}/50")),
                        )
                        .child(format!("Ln {}, Col {}", position.row + 1, position.col + 1))
                        .child("Helix :reflow · 72 columns")
                        .when(long_body, |row| {
                            row.child(
                                div()
                                    .text_color(tokens.editor.warning)
                                    .child("Body line exceeds 72 columns"),
                            )
                        }),
                );
        } else {
            panel = panel.child(
                div()
                    .p(tokens.sizes.space_3)
                    .child("Loading message editor…"),
            );
        }
        panel.into_any_element()
    }
}
