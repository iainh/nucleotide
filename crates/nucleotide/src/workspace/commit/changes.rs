//! The sidebar's Changes list: changed files with staging checkboxes. Clicking a
//! file shows its diff in the Commit tab.
use super::*;
use nucleotide_types::VcsStatus;
use nucleotide_ui::{
    Checkbox, CheckboxSize, ListItem, ListItemSpacing, ListItemVariant, StateViewTone, VcsIcon,
    VcsStatusStyle,
};

/// How a file changed, preferring what the index records over the working tree.
fn change_kind(entry: &GitStatusEntry) -> VcsStatus {
    let (index, worktree) = (entry.index_status, entry.working_tree_status);
    if index == Status::Conflicted || worktree == Status::Conflicted {
        return VcsStatus::Conflicted;
    }
    let status = match index {
        Status::Added | Status::Deleted | Status::Renamed | Status::Copied => index,
        _ if worktree != Status::Unmodified => worktree,
        _ => index,
    };
    match status {
        Status::Untracked => VcsStatus::Untracked,
        Status::Added => VcsStatus::Added,
        Status::Copied => VcsStatus::Copied,
        Status::Deleted => VcsStatus::Deleted,
        Status::Renamed => VcsStatus::Renamed,
        Status::Modified | Status::TypeChanged => VcsStatus::Modified,
        _ => VcsStatus::Unknown,
    }
}

/// Sidebar width from which every list in the switch shows its label.
const SIDEBAR_SWITCH_LABELS_WIDTH: f32 = 300.0;

impl Workspace {
    /// Shows a changed file's diff in the Commit tab, opening the tab when needed.
    fn select_review_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.commit_review.selected = Some(path);
        self.show_commit_tab(cx);
    }

    /// The Files | Changes | History switch, shown in the status bar row under the sidebar.
    pub(in crate::workspace) fn render_sidebar_switch(
        &self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let current = self.commit_review.sidebar;
        // A narrow sidebar labels only the list it shows; the others keep tooltips.
        let compact = self.file_tree_width < SIDEBAR_SWITCH_LABELS_WIDTH;
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
                    (
                        "sidebar-files",
                        "Files",
                        "icons/folder-tree.svg",
                        Sidebar::Files,
                    ),
                    (
                        "sidebar-changes",
                        "Changes",
                        "icons/git-branch.svg",
                        Sidebar::Changes,
                    ),
                    (
                        "sidebar-history",
                        "History",
                        "icons/history.svg",
                        Sidebar::History,
                    ),
                ]
                .map(|(id, label, icon, sidebar)| {
                    let pill = if sidebar == Sidebar::Changes {
                        pill.take().flatten()
                    } else {
                        None
                    };
                    let shown_label = if compact && current != sidebar {
                        ""
                    } else {
                        label
                    };
                    div()
                        .when(cfg!(test), |view| view.debug_selector(|| id.into()))
                        .child(
                            Button::new(id, shown_label)
                                .icon(icon)
                                .size(ButtonSize::Small)
                                .tooltip(label)
                                .aria_label(label)
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
                                            .when(!shown_label.is_empty(), |content| {
                                                content.child(shown_label)
                                            })
                                            .child(pill),
                                    )
                                })
                                .variant(if current == sidebar {
                                    ButtonVariant::Secondary
                                } else {
                                    ButtonVariant::Ghost
                                })
                                .on_click(cx.listener(move |this, _, _, cx| match sidebar {
                                    Sidebar::Changes => this.open_commit_tab(cx),
                                    Sidebar::History => this.show_history_sidebar(cx),
                                    Sidebar::Files => {
                                        this.commit_review.sidebar = Sidebar::Files;
                                        cx.notify();
                                    }
                                })),
                        )
                }),
            )
            .into_any_element()
    }

    pub(in crate::workspace) fn render_changes_sidebar(
        &self,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let count = self
            .commit_review
            .snapshot
            .as_ref()
            .map_or(0, |s| s.files.len());
        let failed = self
            .commit_review
            .error
            .clone()
            .filter(|_| self.commit_review.snapshot.is_none());
        let empty = (count == 0).then(|| {
            let loading = self.commit_review.snapshot.is_none();
            if let Some(error) = failed {
                return StateView::new("changes-sidebar-error", "Cannot load changes")
                    .detail(error)
                    .icon("icons/triangle-alert.svg")
                    .tone(StateViewTone::Error)
                    .action(
                        div()
                            .when(cfg!(test), |view| {
                                view.debug_selector(|| "changes-sidebar-retry".into())
                            })
                            .child(
                                Button::new("changes-sidebar-retry", "Retry")
                                    .icon("icons/refresh.svg")
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.commit_review.error = None;
                                        this.refresh_commit_review(cx);
                                        // File badges and the branch read the same failing Git.
                                        let service =
                                            cx.global::<VcsServiceHandle>().service().clone();
                                        service.update(cx, |service, cx| service.refresh_now(cx));
                                    })),
                            ),
                    )
                    .compact(true);
            }
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
        // Only the rows in view are built, so thousands of changes cost no more than a screenful.
        let list = (count > 0).then(|| {
            gpui::uniform_list(
                "changes-list",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let file_tree = if window.is_window_active() {
                        tokens.file_tree_tokens()
                    } else {
                        tokens.file_tree_tokens().inactive_window(&tokens.chrome)
                    };
                    let Some(snapshot) = &this.commit_review.snapshot else {
                        return Vec::new();
                    };
                    let files = snapshot
                        .files
                        .get(range.clone())
                        .unwrap_or_default()
                        .to_vec();
                    let selected = this.selected_review_file().map(|(index, _)| index);
                    files
                        .iter()
                        .zip(range)
                        .map(|(file, index)| {
                            this.render_changes_row(
                                index,
                                file,
                                selected == Some(index),
                                &file_tree,
                                cx,
                            )
                        })
                        .collect()
                }),
            )
            .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::FitList)
            .size_full()
        });
        div()
            .id("changes-sidebar")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(tokens.sizes.text_md)
            .px(tokens.sizes.space_1)
            .children(list)
            .children(empty)
            .into_any_element()
    }

    fn render_changes_row(
        &self,
        index: usize,
        file: &Arc<ReviewFile>,
        is_selected: bool,
        file_tree: &nucleotide_ui::tokens::FileTreeTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        // Clicks queue behind a stage; only a commit or push locks them.
        let locked = self.commit_review.operation.is_some();
        let path = file.entry.relative_path.clone();
        let status = change_kind(&file.entry);
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
                            .render_with_theme(cx.global::<nucleotide_ui::Theme>())
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
                    .text_color(if is_selected {
                        text
                    } else {
                        status.color(&tokens)
                    })
                    .child(status.label()),
            )
            .with_listener({
                let listener = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.select_review_file(path.clone(), cx)
                });
                move |row| row.on_click(listener)
            });
        div()
            .when(cfg!(test), |view| {
                view.debug_selector(move || format!("changes-row-{index}"))
            })
            .on_mouse_down(
                MouseButton::Right,
                self.review_menu_listener(file, None, cx),
            )
            .child(row)
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
        let kind = |index, worktree| change_kind(&entry(index, worktree));
        // A file added to the index and edited since is still a new file.
        assert_eq!(kind(Status::Added, Status::Modified), VcsStatus::Added);
        assert_eq!(
            kind(Status::Unmodified, Status::Deleted),
            VcsStatus::Deleted
        );
        assert_eq!(kind(Status::Modified, Status::Deleted), VcsStatus::Deleted);
        assert_eq!(
            kind(Status::Untracked, Status::Untracked),
            VcsStatus::Untracked
        );
        assert_eq!(
            kind(Status::Modified, Status::Unmodified),
            VcsStatus::Modified
        );
        assert_eq!(kind(Status::Renamed, Status::Modified), VcsStatus::Renamed);
        assert_eq!(kind(Status::Copied, Status::Unmodified), VcsStatus::Copied);
        assert_eq!(
            kind(Status::Modified, Status::Conflicted),
            VcsStatus::Conflicted
        );
    }
}
