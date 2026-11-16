use super::*;
use crate::diff::{DiffStyle, diff_lines, render_diff_line};
use nucleotide_ui::{Checkbox, CheckboxSize};

pub(super) const MAX_VISIBLE_LINES: usize = 400;

fn button(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id, label)
        .variant(ButtonVariant::Ghost)
        .size(ButtonSize::Small)
}

fn line_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

impl Workspace {
    fn review_checkbox(
        &mut self,
        id: String,
        label: String,
        state: SelectionState,
        file: Arc<ReviewFile>,
        change: StageChange,
        cx: &mut Context<Self>,
    ) -> Checkbox {
        let workspace = cx.entity().downgrade();
        let focus = self
            .commit_review
            .control_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone();
        self.commit_review
            .controls
            .push((focus.clone(), file.clone(), change.clone()));
        Checkbox::new(SharedString::from(id), label)
            .focus_handle(focus.clone())
            .size(CheckboxSize::Small)
            .checked(state == SelectionState::Staged)
            .indeterminate(state == SelectionState::Partial)
            .disabled(self.commit_review.busy)
            .on_change(move |_, window, cx| {
                window.focus(&focus, cx);
                let _ = workspace.update(cx, |workspace, cx| {
                    workspace.stage_review_files(vec![(file.clone(), change.clone())], cx)
                });
            })
    }

    pub(in crate::workspace) fn render_commit_review(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let font = cx.global::<nucleotide_types::EditorFontConfig>();
        let style = DiffStyle {
            font_family: font.family.clone().into(),
            font_size: px(font.size),
            surface: tokens.chrome.surface,
            border: tokens.chrome.border_default,
            text: tokens.editor.text_primary,
            text_secondary: tokens.editor.text_secondary,
            added: tokens.editor.success,
            removed: tokens.editor.error,
        };
        let files = self
            .commit_review
            .snapshot
            .as_ref()
            .map(|s| s.files.clone())
            .unwrap_or_default();
        let staged_count = files
            .iter()
            .filter(|file| file.state() != SelectionState::Unstaged)
            .count();
        let branch = self
            .commit_review
            .snapshot
            .as_ref()
            .map(|s| match &s.upstream {
                Ok(state) if state.upstream.is_some() => {
                    format!("{} ↑{} ↓{}", s.branch, state.ahead, state.behind)
                }
                _ => s.branch.clone(),
            })
            .unwrap_or_default();
        let busy = self.commit_review.busy;
        let can_commit = self.can_commit(cx);
        let can_push = self.can_push();
        let amending = self.commit_amending();
        let can_amend = !busy
            && self
                .commit_review
                .snapshot
                .as_ref()
                .is_some_and(|s| s.head.is_some());
        let error = self.commit_review.error.clone().or_else(|| {
            self.commit_amend_stale().then(|| "HEAD changed since Amend was selected. Toggle Amend off and on to review the new commit.".into())
        }).or_else(|| {
            self.commit_review.snapshot.as_ref().and_then(|s| {
                s.message_context
                    .as_ref()
                    .err()
                    .map(|e| format!("Cannot load commit message defaults: {e}"))
                    .or_else(|| {
                        s.upstream
                            .as_ref()
                            .err()
                            .map(|e| format!("Cannot read upstream: {e}"))
                    })
            })
        });
        self.commit_review.controls.clear();
        let mut list = div()
            .id("commit-files")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.pane_host.commit_message_focused {
                        this.focus_commit_list(window, cx);
                    }
                }),
            )
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.commit_review.scroll)
            .p(tokens.sizes.space_3)
            .flex()
            .flex_col()
            .gap(tokens.sizes.space_3);
        for (index, file) in files.iter().enumerate() {
            list = list.child(self.render_review_file(index, file.clone(), &style, cx));
        }
        if files.is_empty() && !busy && self.commit_review.error.is_none() {
            list = list.child(
                div()
                    .p(tokens.sizes.space_8)
                    .text_color(style.text_secondary)
                    .child("No changes to commit. Your working tree is clean."),
            );
        }
        if self
            .commit_review
            .snapshot
            .as_ref()
            .is_some_and(|s| s.truncated)
        {
            list = list.child("Some files are omitted by the status limit. Stage all affects only the files listed here.");
        }
        let all_files = files.clone();
        let unstage_files = files.clone();
        div()
            .id("commit-review")
            .when_some(self.commit_review.focus.as_ref(), |view, focus| {
                view.track_focus(focus)
            })
            .capture_key_down(cx.listener(Self::commit_shortcut))
            .on_key_down(cx.listener(Self::commit_review_key))
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(tokens.editor.background)
            .text_color(style.text)
            .text_size(tokens.sizes.text_sm)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens.sizes.space_3)
                    .p(tokens.sizes.space_3)
                    .border_b_1()
                    .border_color(style.border)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Commit review"),
                    )
                    .child(div().text_color(style.text_secondary).child(branch))
                    .child(button("commit-refresh", "Refresh").disabled(busy).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.commit_review.error = None;
                            this.refresh_commit_review(cx);
                        }),
                    ))
                    .child(
                        button("commit-stage-all", "Stage all")
                            .disabled(busy || files.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stage_review_files(
                                    all_files
                                        .iter()
                                        .map(|f| (f.clone(), StageChange::File(true)))
                                        .collect(),
                                    cx,
                                );
                            })),
                    )
                    .child(
                        button("commit-unstage-all", "Unstage all")
                            .disabled(busy || staged_count == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stage_review_files(
                                    unstage_files
                                        .iter()
                                        .filter(|f| f.state() != SelectionState::Unstaged)
                                        .map(|f| (f.clone(), StageChange::File(false)))
                                        .collect(),
                                    cx,
                                );
                            })),
                    ),
            )
            .child(self.render_commit_message(cx))
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .id("commit-error")
                        .when(cfg!(test), |view| {
                            view.debug_selector(|| "commit-error".into())
                        })
                        .max_h(tokens.sizes.space_8 * 3.0)
                        .overflow_y_scroll()
                        .flex_shrink_0()
                        .p(tokens.sizes.space_3)
                        .bg(tokens.editor.error.opacity(0.12))
                        .text_color(tokens.editor.error)
                        .child(error),
                )
            })
            .when(busy, |view| {
                view.child(
                    div()
                        .px(tokens.sizes.space_3)
                        .py(tokens.sizes.space_1)
                        .text_color(style.text_secondary)
                        .child(if let Some(operation) = self.commit_review.operation {
                            operation
                        } else if self.commit_review.pending.is_some() {
                            "Saving buffers before staging…"
                        } else {
                            "Refreshing repository…"
                        }),
                )
            })
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens.sizes.space_3)
                    .p(tokens.sizes.space_3)
                    .border_t_1()
                    .border_color(style.border)
                    .flex_shrink_0()
                    .child(format!(
                        "{staged_count} of {} files staged · Staging saves affected buffers",
                        files.len()
                    ))
                    .child(
                        Checkbox::new("commit-amend", "Amend")
                            .checked(amending)
                            .disabled(!can_amend)
                            .on_change({
                                let workspace = cx.entity().downgrade();
                                move |_, _, cx| {
                                    let _ = workspace
                                        .update(cx, |this, cx| this.toggle_commit_amend(cx));
                                }
                            }),
                    )
                    .child(
                        div()
                            .when(cfg!(test), |view| {
                                view.debug_selector(|| "commit-submit".into())
                            })
                            .child(
                                Button::new(
                                    "commit-submit",
                                    if amending { "Amend Commit" } else { "Commit" },
                                )
                                .disabled(!can_commit)
                                .size(ButtonSize::Small)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.submit_commit(false, cx)),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .when(cfg!(test), |view| {
                                view.debug_selector(|| "commit-push".into())
                            })
                            .child(
                                Button::new("commit-push", "Commit and Push")
                                    .disabled(!can_commit)
                                    .size(ButtonSize::Small)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.submit_commit(true, cx)),
                                    ),
                            ),
                    )
                    .when(can_push, |footer| {
                        footer.child(
                            div()
                                .when(cfg!(test), |view| {
                                    view.debug_selector(|| "commit-push-only".into())
                                })
                                .child(
                                    button("commit-push-only", "Push").on_click(
                                        cx.listener(|this, _, _, cx| this.push_commit(cx)),
                                    ),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_review_file(
        &mut self,
        index: usize,
        file: Arc<ReviewFile>,
        style: &DiffStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let path = file.entry.relative_path.clone();
        let collapsed = self.commit_review.collapsed.contains(&path);
        let state = file.state();
        let state_label = match state {
            SelectionState::Unstaged => "Unstaged",
            SelectionState::Partial => "Partly staged",
            SelectionState::Staged => "Staged",
        };
        let unstage_file = file.clone();
        let mut section = div()
            .flex_none()
            .min_w_0()
            .border_1()
            .border_color(style.border)
            .rounded(tokens.sizes.radius_md)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens.sizes.space_2)
                    .p(tokens.sizes.space_2)
                    .bg(style.surface)
                    .child(
                        button(
                            SharedString::from(format!("commit-collapse-{index}")),
                            if collapsed { "▸" } else { "▾" },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.commit_review.collapsed.remove(&path) {
                                this.commit_review.collapsed.insert(path.clone());
                            }
                            cx.notify();
                        })),
                    )
                    .child(self.review_checkbox(
                        format!("commit-file-{index}"),
                        file.entry.relative_path.display().to_string(),
                        state,
                        file.clone(),
                        StageChange::File(state != SelectionState::Staged),
                        cx,
                    ))
                    .child(div().text_color(style.text_secondary).child(state_label))
                    .when(file.unsaved, |header| {
                        header.child(div().text_color(tokens.editor.warning).child("Unsaved"))
                    })
                    .when_some(
                        file.entry.original_relative_path.as_ref(),
                        |header, from| header.child(format!("← {}", from.display())),
                    )
                    .when(state == SelectionState::Partial, |header| {
                        header.child(
                            button(
                                SharedString::from(format!("commit-unstage-{index}")),
                                "Unstage file",
                            )
                            .disabled(self.commit_review.busy)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.stage_review_files(
                                        vec![(unstage_file.clone(), StageChange::File(false))],
                                        cx,
                                    );
                                },
                            )),
                        )
                    }),
            );
        if !collapsed {
            match &file.content {
                Err(reason) => {
                    section = section.child(
                        div()
                            .p(tokens.sizes.space_3)
                            .text_color(style.text_secondary)
                            .child(reason.clone()),
                    )
                }
                Ok(content) => {
                    if content.selection.is_none() {
                        section = section.child(div().p(tokens.sizes.space_3).text_color(tokens.editor.warning)
                            .child("Staged version differs from working tree. Stage the whole file to replace it, or unstage it."));
                    }
                    if let Some(staged) = &content.staged_diff {
                        let mut preview = div()
                            .id(SharedString::from(format!("commit-staged-{index}")))
                            .overflow_x_scroll()
                            .font_family(style.font_family.clone())
                            .text_size(style.font_size)
                            .child(
                                div()
                                    .p(tokens.sizes.space_2)
                                    .text_color(style.text_secondary)
                                    .child("Staged changes · read-only"),
                            );
                        let mut remaining = MAX_VISIBLE_LINES;
                        for hunk in staged.hunks() {
                            let removed: Vec<_> = hunk
                                .before
                                .clone()
                                .take(remaining)
                                .map(|n| line_text(staged.head_line(n)))
                                .collect();
                            remaining -= removed.len();
                            let added: Vec<_> = hunk
                                .after
                                .clone()
                                .take(remaining)
                                .map(|n| line_text(staged.worktree_line(n)))
                                .collect();
                            remaining -= added.len();
                            preview = preview.children(
                                diff_lines(&removed, &added, style)
                                    .into_iter()
                                    .map(|row| render_diff_line(row, tokens.sizes.space_6)),
                            );
                            if remaining == 0 {
                                break;
                            }
                        }
                        section = section.child(preview).child(
                            div()
                                .p(tokens.sizes.space_2)
                                .text_color(style.text_secondary)
                                .child("Working-tree changes · line staging unavailable"),
                        );
                    }
                    let mut body = div()
                        .id(SharedString::from(format!("commit-diff-{index}")))
                        .overflow_x_scroll()
                        .font_family(style.font_family.clone())
                        .text_size(style.font_size);
                    let mut visible = 0;
                    for (hunk_index, hunk) in content.changes.hunks().iter().enumerate() {
                        if visible >= MAX_VISIBLE_LINES {
                            break;
                        }
                        let state = content
                            .selection
                            .as_ref()
                            .map(|s| s.hunk_state(hunk))
                            .unwrap_or(SelectionState::Unstaged);
                        let header = self
                            .review_checkbox(
                                format!("commit-hunk-{index}-{hunk_index}"),
                                format!(
                                    "@@ −{},{} +{},{} @@",
                                    hunk.before.start + 1,
                                    hunk.before.len(),
                                    hunk.after.start + 1,
                                    hunk.after.len()
                                ),
                                state,
                                file.clone(),
                                StageChange::Hunk(hunk_index, state != SelectionState::Staged),
                                cx,
                            )
                            .disabled(self.commit_review.busy || content.selection.is_none());
                        body = body.child(
                            div()
                                .bg(style.surface)
                                .px(tokens.sizes.space_3)
                                .child(header),
                        );
                        let context_line = |line: u32, bytes: &[u8]| {
                            div()
                                .flex()
                                .text_color(style.text_secondary)
                                .child(
                                    div()
                                        .w(tokens.sizes.space_10)
                                        .text_right()
                                        .child((line + 1).to_string()),
                                )
                                .child(
                                    div()
                                        .px(tokens.sizes.space_3)
                                        .whitespace_nowrap()
                                        .child(crate::diff::expand_tabs(&line_text(bytes))),
                                )
                        };
                        for line in hunk.before.start.saturating_sub(3)..hunk.before.start {
                            body = body.child(context_line(line, content.changes.head_line(line)));
                        }
                        let removed: Vec<_> = hunk
                            .before
                            .clone()
                            .take(MAX_VISIBLE_LINES - visible)
                            .map(|line| line_text(content.changes.head_line(line)))
                            .collect();
                        let added: Vec<_> = hunk
                            .after
                            .clone()
                            .take((MAX_VISIBLE_LINES - visible).saturating_sub(removed.len()))
                            .map(|line| line_text(content.changes.worktree_line(line)))
                            .collect();
                        let locations = hunk
                            .before
                            .clone()
                            .take(removed.len())
                            .map(|line| (true, line))
                            .chain(
                                hunk.after
                                    .clone()
                                    .take(added.len())
                                    .map(|line| (false, line)),
                            );
                        for ((removal, line), row) in
                            locations.zip(diff_lines(&removed, &added, style))
                        {
                            visible += 1;
                            let staged = content.selection.as_ref().is_some_and(|s| {
                                if removal {
                                    s.is_removal_staged(line)
                                } else {
                                    s.is_addition_staged(line)
                                }
                            });
                            let checkbox = self
                                .review_checkbox(
                                    format!("commit-line-{index}-{removal}-{line}"),
                                    String::new(),
                                    if staged {
                                        SelectionState::Staged
                                    } else {
                                        SelectionState::Unstaged
                                    },
                                    file.clone(),
                                    StageChange::Line {
                                        removal,
                                        line,
                                        staged: !staged,
                                    },
                                    cx,
                                )
                                .disabled(self.commit_review.busy || content.selection.is_none());
                            body = body.child(
                                div()
                                    .when(cfg!(test), |row| {
                                        row.debug_selector(move || {
                                            format!("commit-row-{index}-{removal}-{line}")
                                        })
                                    })
                                    .flex()
                                    .items_center()
                                    .flex_none()
                                    .child(checkbox)
                                    .child(
                                        div()
                                            .w(tokens.sizes.space_9)
                                            .flex_none()
                                            .text_right()
                                            .text_color(style.text_secondary)
                                            .child((line + 1).to_string()),
                                    )
                                    .child(render_diff_line(row, tokens.sizes.space_6)),
                            );
                        }
                        let end = content
                            .changes
                            .hunks()
                            .get(hunk_index + 1)
                            .map(|next| next.after.start)
                            .unwrap_or_else(|| {
                                content.worktree.split_inclusive(|b| *b == b'\n').count() as u32
                            });
                        for line in hunk.after.end..end.min(hunk.after.end + 3) {
                            body =
                                body.child(context_line(line, content.changes.worktree_line(line)));
                        }
                    }
                    if visible == MAX_VISIBLE_LINES {
                        body = body.child(div().p(tokens.sizes.space_3).child("Preview limited to 400 changed lines. Whole-file actions include all changes."));
                    }
                    if content.changes.hunks().is_empty() {
                        body = body.child(div().p(tokens.sizes.space_3).child("No text changes against HEAD. Use whole-file actions for staged-only or mode changes."));
                    }
                    section = section.child(body);
                }
            }
        }
        section.into_any_element()
    }
}
