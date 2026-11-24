//! The sidebar's Changes list: changed files with staging checkboxes. Clicking a
//! file shows its diff in the Commit tab.
use super::*;
use nucleotide_types::VcsStatus;
use nucleotide_ui::{
    Checkbox, CheckboxSize, ListItem, ListItemSpacing, ListItemVariant, NavigableList,
    StateViewTone, VcsIcon, VcsStatusStyle,
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

/// Keys of the Changes list, shown in its rows' tooltips.
const CHANGES_KEYS: &str = "↑↓ select · Space stages · Enter opens the file";

impl Workspace {
    /// Shows changed file `index` in the Commit tab, opening the tab when
    /// needed. Focus stays in the Changes list.
    fn select_review_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self
            .commit_review
            .snapshot
            .as_ref()
            .and_then(|s| s.files.get(index))
        else {
            return;
        };
        self.commit_review.selected = Some(file.entry.relative_path.clone());
        self.show_commit_tab(false, cx);
    }

    /// Stages changed file `index`, or unstages it when it is fully staged.
    fn toggle_review_file(&mut self, index: usize, cx: &mut Context<Self>) {
        // Clicks queue behind a stage; only a commit or push locks them.
        if self.commit_review.operation.is_some() {
            return;
        }
        let Some(file) = self
            .commit_review
            .snapshot
            .as_ref()
            .and_then(|s| s.files.get(index).cloned())
        else {
            return;
        };
        let change = StageChange::File(file.state() != SelectionState::Staged);
        self.stage_review_files(vec![(file, change)], cx);
    }

    /// Opens changed file `index` in the editor at its first change.
    fn open_review_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(snapshot) = &self.commit_review.snapshot else {
            return;
        };
        let Some(file) = snapshot.files.get(index) else {
            return;
        };
        if change_kind(&file.entry) == VcsStatus::Deleted {
            return;
        }
        let line = file
            .content
            .as_ref()
            .ok()
            .and_then(|c| c.changes.hunks().first().map(|hunk| hunk.after.start))
            .unwrap_or(0);
        let path = snapshot.repository.root().join(&file.entry.relative_path);
        self.open_file_at(&path, Position::new(line as usize, 0), cx);
    }

    /// Moves keyboard focus to the list the sidebar shows.
    pub(in crate::workspace) fn focus_sidebar_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = match self.commit_review.sidebar {
            Sidebar::Files => return,
            Sidebar::Changes => &mut self.commit_review.changes_focus,
            Sidebar::History => &mut self.commit_review.history.focus,
        };
        let focus = focus.get_or_insert_with(|| cx.focus_handle()).clone();
        // Opening the Commit tab focuses it after this effect cycle; follow it.
        cx.defer_in(window, move |_, window, cx| window.focus(&focus, cx));
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
        let badge = (count > 0).then(|| {
            if count > 99 {
                "99+".to_string()
            } else {
                count.to_string()
            }
        });
        let mut badge = Some(badge);
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
                    let badge = if sidebar == Sidebar::Changes {
                        badge.take().flatten()
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
                                .when_some(badge, |button, badge| {
                                    button
                                        .badge(badge)
                                        .aria_label(format!("{label}, {count} changed"))
                                })
                                .variant(if current == sidebar {
                                    ButtonVariant::Secondary
                                } else {
                                    ButtonVariant::Ghost
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    match sidebar {
                                        Sidebar::Changes => this.open_commit_tab(cx),
                                        Sidebar::History => this.show_history_sidebar(cx),
                                        Sidebar::Files => {
                                            this.commit_review.sidebar = Sidebar::Files;
                                            cx.notify();
                                        }
                                    }
                                    this.focus_sidebar_list(window, cx);
                                })),
                        )
                }),
            )
            .into_any_element()
    }

    pub(in crate::workspace) fn render_changes_sidebar(
        &mut self,
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
        let focus = self
            .commit_review
            .changes_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let scroll = self.commit_review.changes_scroll.clone();
        let selected = self.selected_review_file().map(|(index, _)| index);
        let list = (count > 0).then(|| {
            let rows = gpui::uniform_list(
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
            .track_scroll(&scroll.handle)
            .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::FitList)
            .size_full();
            let workspace = cx.entity().downgrade();
            let (select, toggle, open) = (workspace.clone(), workspace.clone(), workspace);
            NavigableList::new("changes-navigation", &focus, count)
                .selected(selected)
                .track_scroll(&scroll.handle)
                .on_select(move |index, _, cx| {
                    let _ = select.update(cx, |this, cx| this.select_review_file(index, cx));
                })
                .on_toggle(move |index, _, cx| {
                    let _ = toggle.update(cx, |this, cx| this.toggle_review_file(index, cx));
                })
                .on_confirm(move |index, _, cx| {
                    let _ = open.update(cx, |this, cx| this.open_review_file(index, cx));
                })
                .relative()
                .flex_1()
                .min_h_0()
                .child(rows)
                .child(Scrollbar::vertical_overlay(scroll.bar))
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
                    .aria_label(format!(
                        "{} {}",
                        if state == SelectionState::Staged {
                            "Unstage"
                        } else {
                            "Stage"
                        },
                        path.display()
                    ))
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
            .tooltip_detail(CHANGES_KEYS)
            .start_slot(checkbox)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(tokens.sizes.space_2)
                    .min_w_0()
                    .child(
                        VcsIcon::from_path(&path, false)
                            .size(nucleotide_ui::tokens::SMALL_ICON_SIZE)
                            .text_color(if is_selected {
                                file_tree.icon_color_selected
                            } else {
                                file_tree.icon_color
                            })
                            .render_with_theme(cx.global::<nucleotide_ui::Theme>())
                            .flex_none(),
                    )
                    // The name keeps its width until the directory has given up all of its own.
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink(1.0)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(name),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink(1000.0)
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
                    this.select_review_file(index, cx)
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
