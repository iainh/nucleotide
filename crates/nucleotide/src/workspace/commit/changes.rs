//! The sidebar's Changes list: changed files with staging checkboxes. Clicking a
//! file shows its diff in the Commit tab.
use super::*;
use nucleotide_types::VcsStatus;
use nucleotide_ui::{Checkbox, CheckboxSize, ListItem, ListItemSpacing, ListItemVariant, VcsIcon};

/// How a file changed, preferring what the index records over the working tree.
fn change_kind(entry: &GitStatusEntry) -> (VcsStatus, &'static str) {
    let (index, worktree) = (entry.index_status, entry.working_tree_status);
    if index == Status::Conflicted || worktree == Status::Conflicted {
        return (VcsStatus::Conflicted, "C");
    }
    let status = match index {
        Status::Added | Status::Deleted | Status::Renamed | Status::Copied => index,
        _ if worktree != Status::Unmodified => worktree,
        _ => index,
    };
    match status {
        Status::Untracked => (VcsStatus::Untracked, "U"),
        Status::Added | Status::Copied => (VcsStatus::Added, "A"),
        Status::Deleted => (VcsStatus::Deleted, "D"),
        Status::Renamed => (VcsStatus::Renamed, "R"),
        Status::Modified | Status::TypeChanged => (VcsStatus::Modified, "M"),
        _ => (VcsStatus::Unknown, "?"),
    }
}

impl Workspace {
    /// Shows a changed file's diff in the Commit tab, opening the tab when needed.
    fn select_review_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.commit_review.selected = Some(path);
        self.show_commit_tab(cx);
    }

    /// The Files | Changes switch, shown in the status bar row under the sidebar.
    pub(in crate::workspace) fn render_sidebar_switch(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let changes = self.commit_review.sidebar;
        // The VCS service watches the repository, so the count stays current
        // even while the Commit tab, and with it the review, is closed.
        let count = cx
            .global::<nucleotide_vcs::VcsServiceHandle>()
            .service()
            .read(cx)
            .status_entries()
            .len();
        let (pill_bg, pill_fg) = if let Some(c) = tokens.chrome.contrast {
            (c.highlight, c.highlight_text)
        } else {
            (tokens.editor.vcs_modified, tokens.editor.background)
        };
        // A pill after the Changes label, hidden when nothing has changed.
        let pill = (count > 0).then(|| {
            div()
                .when(cfg!(test), |view| {
                    view.debug_selector(|| "sidebar-changes-count".into())
                })
                .flex_none()
                .min_w(tokens.sizes.space_6)
                .px(tokens.sizes.space_3)
                .py(px(1.0))
                .flex()
                .justify_center()
                .rounded(tokens.sizes.radius_full)
                .bg(pill_bg)
                .text_color(pill_fg)
                .text_size(tokens.sizes.text_xs)
                .font_weight(FontWeight::SEMIBOLD)
                .line_height(relative(1.2))
                .child(if count > 99 {
                    "99+".to_string()
                } else {
                    count.to_string()
                })
        });
        let mut pill = Some(pill);
        div()
            .flex()
            .h_full()
            .items_center()
            .gap(tokens.sizes.space_1)
            .px(tokens.sizes.space_2)
            .children(
                [
                    ("sidebar-files", "Files", "icons/folder-tree.svg", false),
                    ("sidebar-changes", "Changes", "icons/git-branch.svg", true),
                ]
                .map(|(id, label, icon, show_changes)| {
                    let pill = if show_changes {
                        pill.take().flatten()
                    } else {
                        None
                    };
                    div()
                        .when(cfg!(test), |view| view.debug_selector(|| id.into()))
                        .child(
                            Button::new(id, label)
                                .icon(icon)
                                .size(ButtonSize::Small)
                                .when_some(pill, |button, pill| {
                                    // Custom content replaces the icon slot, so repeat it here.
                                    button.content(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(tokens.sizes.space_2)
                                            .child(
                                                nucleotide_ui::icon()
                                                    .path(icon)
                                                    .size(px(
                                                        nucleotide_ui::tokens::STANDARD_ICON_SIZE,
                                                    ))
                                                    .flex_none(),
                                            )
                                            .child(label)
                                            .child(pill),
                                    )
                                })
                                .variant(if changes == show_changes {
                                    ButtonVariant::Secondary
                                } else {
                                    ButtonVariant::Ghost
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if show_changes {
                                        this.open_commit_tab(cx);
                                    } else {
                                        this.commit_review.sidebar = false;
                                        cx.notify();
                                    }
                                })),
                        )
                }),
            )
            .into_any_element()
    }

    pub(in crate::workspace) fn render_changes_sidebar(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if self.commit_review.stale {
            self.refresh_commit_review(cx);
        }
        let theme = cx.global::<nucleotide_ui::Theme>();
        let tokens = theme.tokens;
        let file_tree = if window.is_window_active() {
            tokens.file_tree_tokens()
        } else {
            tokens.file_tree_tokens().inactive_window(&tokens.chrome)
        };
        let files = self
            .commit_review
            .snapshot
            .as_ref()
            .map(|s| s.files.clone())
            .unwrap_or_default();
        let selected = self.selected_review_file().map(|(index, _)| index);
        // Clicks queue behind a stage; only a commit or push locks them.
        let locked = self.commit_review.operation.is_some();
        let mut rows = Vec::with_capacity(files.len());
        for (index, file) in files.iter().enumerate() {
            let path = file.entry.relative_path.clone();
            let (status, letter) = change_kind(&file.entry);
            let is_selected = selected == Some(index);
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into(),
            );
            let directory = path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let state = file.state();
            let stage_file = file.clone();
            let workspace = cx.entity().downgrade();
            let checkbox = div()
                .when(cfg!(test), |view| {
                    view.debug_selector(move || format!("changes-stage-{index}"))
                })
                .child(
                    Checkbox::new(SharedString::from(format!("changes-stage-{index}")), "")
                        .size(CheckboxSize::Small)
                        .checked(state == SelectionState::Staged)
                        .indeterminate(state == SelectionState::Partial)
                        .disabled(locked)
                        .on_change(move |_, _, cx| {
                            let change =
                                StageChange::File(stage_file.state() != SelectionState::Staged);
                            let _ = workspace.update(cx, |workspace, cx| {
                                workspace.stage_review_files(vec![(stage_file.clone(), change)], cx)
                            });
                        }),
                );
            let (text, secondary) = if is_selected {
                (file_tree.item_text_selected, file_tree.item_text_selected)
            } else {
                (file_tree.item_text, file_tree.item_text_secondary)
            };
            let status_color = match status {
                VcsStatus::Added | VcsStatus::Untracked => tokens.editor.vcs_added,
                VcsStatus::Deleted => tokens.editor.vcs_deleted,
                VcsStatus::Modified | VcsStatus::Renamed => tokens.editor.vcs_modified,
                _ => tokens.editor.error,
            };
            let row = ListItem::new(SharedString::from(format!("changes-row-{index}")))
                .variant(ListItemVariant::Ghost)
                .spacing(ListItemSpacing::Compact)
                .background(if is_selected {
                    file_tree.item_background_selected
                } else {
                    gpui::transparent_black()
                })
                .hover_background(if is_selected {
                    file_tree.item_background_selected_hover
                } else {
                    file_tree.item_background_hover
                })
                .text_color(text)
                .tooltip(path.display().to_string())
                .start_slot(checkbox)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(tokens.sizes.space_2)
                        .min_w_0()
                        .child(
                            VcsIcon::from_path(&path, false)
                                .size(16.0)
                                .text_color(if is_selected {
                                    file_tree.icon_color_selected
                                } else {
                                    file_tree.icon_color
                                })
                                .render_with_theme(theme)
                                .flex_none(),
                        )
                        .child(div().flex_none().whitespace_nowrap().child(name))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(secondary)
                                .child(directory),
                        ),
                )
                .end_slot(
                    div()
                        .w(tokens.sizes.space_5)
                        .text_center()
                        .text_color(if is_selected { text } else { status_color })
                        .child(letter),
                )
                .with_listener({
                    let listener = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        this.select_review_file(path.clone(), cx)
                    });
                    move |row| row.on_click(listener)
                });
            rows.push(
                div()
                    .when(cfg!(test), |view| {
                        view.debug_selector(move || format!("changes-row-{index}"))
                    })
                    .child(row),
            );
        }
        let empty = files.is_empty().then(|| {
            let loading = self.commit_review.snapshot.is_none();
            StateView::new(
                "changes-sidebar-empty",
                if loading {
                    "Loading changes…"
                } else {
                    "No changes"
                },
            )
            .loading(loading)
            .compact(true)
        });
        div()
            .id("changes-sidebar")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .text_size(tokens.sizes.text_md)
            .px(tokens.sizes.space_1)
            .children(rows)
            .children(empty)
            .into_any_element()
    }
}

#[cfg(test)]
mod change_tests {
    use super::*;

    fn entry(index_status: Status, working_tree_status: Status) -> GitStatusEntry {
        GitStatusEntry {
            relative_path: "file".into(),
            original_relative_path: None,
            index_status,
            working_tree_status,
        }
    }

    #[test]
    fn change_kind_prefers_what_the_index_records() {
        // A file added to the index and edited since is still a new file.
        assert_eq!(change_kind(&entry(Status::Added, Status::Modified)).1, "A");
        assert_eq!(
            change_kind(&entry(Status::Unmodified, Status::Deleted)).1,
            "D"
        );
        assert_eq!(
            change_kind(&entry(Status::Modified, Status::Deleted)).1,
            "D"
        );
        assert_eq!(
            change_kind(&entry(Status::Untracked, Status::Untracked)).1,
            "U"
        );
        assert_eq!(
            change_kind(&entry(Status::Modified, Status::Unmodified)).1,
            "M"
        );
        assert_eq!(
            change_kind(&entry(Status::Renamed, Status::Modified)).1,
            "R"
        );
    }
}
