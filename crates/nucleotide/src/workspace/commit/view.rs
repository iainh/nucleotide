use super::split::{DiffPart, SplitDiff, SplitLayout, split_parts, unified_parts, widest};
use super::*;
use crate::diff::{DiffStyle, diff_lines, render_diff_line};
use gpui::WeakEntity;
use nucleotide_ui::{Checkbox, CheckboxSize};

pub(super) const MAX_VISIBLE_LINES: usize = 400;

fn button(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id, label)
        .variant(ButtonVariant::Ghost)
        .size(ButtonSize::Small)
}

/// Diff rows use the editor's line height, which is also set as the text line
/// height so that word highlights fill exactly one row. Rows are never shorter
/// than a small checkbox, so staging controls fit with small editor fonts.
pub(super) fn row_height(cx: &App) -> Pixels {
    let font = cx.global::<nucleotide_types::EditorFontConfig>();
    let checkbox = cx.global::<nucleotide_ui::Theme>().tokens.sizes.space_6;
    px((font.size * font.line_height).round()).max(checkbox)
}

pub(super) fn line_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

/// Which diff of a file a list item belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiffKind {
    /// HEAD against what a deletion or rename leaves.
    Preview,
    /// HEAD against an index that line staging cannot express.
    Staged,
    /// HEAD against the working tree, with staging controls.
    Working,
}

/// One item of the shown file's list. Each is laid out only when on screen,
/// so a long diff costs no more per frame than a short one.
#[derive(Clone, PartialEq)]
pub(super) enum ReviewItem {
    Header,
    Note {
        text: SharedString,
        tone: NoteTone,
        small: bool,
    },
    Split(DiffKind, DiffPart),
    Unified(DiffPart),
    /// A read-only unified diff, short enough to show as one item.
    UnifiedStaged(DiffKind),
}

/// The text colour of a note, resolved from the theme when drawn.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum NoteTone {
    Primary,
    Secondary,
    Warning,
}

/// The shown file laid out as list items, with what the parts of its diffs
/// share. It is kept until the file or how it is shown changes, so a frame
/// such as one per keystroke in the commit message does not recompute it.
pub(super) struct ReviewLayout {
    file: Arc<ReviewFile>,
    index: usize,
    unified: bool,
    collapsed: bool,
    /// The file's unfolded runs: staged diff, first HEAD line.
    expanded: Vec<(bool, u32)>,
    items: Vec<ReviewItem>,
    /// The item holding each working-tree hunk.
    hunk_items: Vec<usize>,
    /// The side-by-side layout of each `DiffKind`.
    splits: [Option<SplitLayout>; 3],
    /// Columns of the widest line of the unified working-tree diff.
    unified_widest: u32,
}

/// The colours and font of the review's diffs.
fn diff_style(cx: &App) -> DiffStyle {
    let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
    let font = cx.global::<nucleotide_types::EditorFontConfig>();
    DiffStyle {
        font_family: font.family.clone().into(),
        font_size: px(font.size),
        surface: tokens.chrome.surface,
        border: tokens.chrome.border_default,
        text: tokens.editor.text_primary,
        text_secondary: tokens.editor.text_secondary,
        added: tokens.editor.success,
        removed: tokens.editor.error,
    }
}

/// The shown file's diff list as its own view. The Workspace renders on every
/// keystroke, in an editor or the commit message; this view is drawn from
/// GPUI's cache unless what it shows changed.
pub(in crate::workspace) struct ReviewList {
    workspace: WeakEntity<Workspace>,
    shows: Option<ReviewListKey>,
    _observe: Subscription,
}

/// What the diff list shows. Its rows also depend on focus and hover, which
/// redraw it on their own.
struct ReviewListKey {
    file: Option<(usize, Arc<ReviewFile>)>,
    unified: bool,
    collapsed: bool,
    expanded: Vec<(bool, u32)>,
    locked: bool,
    split: Option<f32>,
    row: Pixels,
    style: DiffStyle,
}

impl PartialEq for ReviewListKey {
    fn eq(&self, other: &Self) -> bool {
        let same_file = match (&self.file, &other.file) {
            (Some((a, file)), Some((b, other))) => a == b && Arc::ptr_eq(file, other),
            (a, b) => a.is_none() && b.is_none(),
        };
        same_file
            && self.unified == other.unified
            && self.collapsed == other.collapsed
            && self.expanded == other.expanded
            && self.locked == other.locked
            && self.split == other.split
            && self.row == other.row
            && self.style == other.style
    }
}

impl ReviewList {
    fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&workspace, |this, workspace, cx| {
            let shows = workspace.read(cx).review_list_key(cx);
            if this.shows.as_ref() != Some(&shows) {
                this.shows = Some(shows);
                cx.notify();
            }
        });
        Self {
            workspace: workspace.downgrade(),
            shows: None,
            _observe: observe,
        }
    }
}

impl Render for ReviewList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.workspace
            .update(cx, |workspace, cx| workspace.render_review_list(cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

impl Workspace {
    fn split_diff<'a>(
        index: usize,
        file: &'a Arc<ReviewFile>,
        kind: DiffKind,
    ) -> Option<SplitDiff<'a>> {
        let path = &file.entry.relative_path;
        Some(match (kind, &file.content) {
            (DiffKind::Preview, _) => {
                let preview = file.preview.as_ref()?;
                SplitDiff {
                    id: format!("commit-preview-{index}"),
                    path,
                    staged: false,
                    titles: ["HEAD", "Working tree"],
                    changes: &preview.changes,
                    old_len: preview.old_len,
                    highlights: [
                        preview.highlights[0].as_ref(),
                        preview.highlights[1].as_ref(),
                    ],
                    staging: None,
                }
            }
            (DiffKind::Staged, Ok(content)) => SplitDiff {
                id: format!("commit-staged-{index}"),
                path,
                staged: true,
                titles: ["HEAD", "Staged"],
                changes: content.staged_diff.as_ref()?,
                old_len: content.changes.head_len(),
                highlights: [
                    content.highlights[0].as_deref(),
                    content.highlights[1].as_deref(),
                ],
                staging: None,
            },
            (DiffKind::Working, Ok(content)) => SplitDiff {
                id: format!("commit-diff-{index}"),
                path,
                staged: false,
                titles: ["HEAD", "Working tree"],
                changes: &content.changes,
                old_len: content.changes.head_len(),
                highlights: [
                    content.highlights[0].as_deref(),
                    content.highlights[2].as_deref(),
                ],
                staging: Some((index, file, content)),
            },
            _ => return None,
        })
    }

    fn review_list_key(&self, cx: &App) -> ReviewListKey {
        let file = self.selected_review_file();
        let path = file
            .as_ref()
            .map(|(_, file)| file.entry.relative_path.as_path());
        ReviewListKey {
            unified: self.commit_review.unified,
            collapsed: path.is_some_and(|path| self.commit_review.collapsed.contains(path)),
            expanded: path
                .map(|path| self.expanded_runs(path))
                .unwrap_or_default(),
            locked: self.commit_review.operation.is_some(),
            split: self.commit_review.split,
            row: row_height(cx),
            style: diff_style(cx),
            file,
        }
    }

    /// The unfolded runs of `path`'s diffs, which its layout depends on.
    fn expanded_runs(&self, path: &Path) -> Vec<(bool, u32)> {
        let mut runs: Vec<_> = self
            .commit_review
            .expanded
            .iter()
            .filter(|(expanded, _, _)| expanded == path)
            .map(|(_, staged, line)| (*staged, *line))
            .collect();
        runs.sort_unstable();
        runs
    }

    /// Lays out the list items of the shown file, and the item holding each
    /// working-tree hunk, so keyboard navigation can scroll a control into view.
    fn review_layout(
        &self,
        index: usize,
        file: &Arc<ReviewFile>,
        expanded: Vec<(bool, u32)>,
    ) -> ReviewLayout {
        let unified = self.commit_review.unified;
        let collapsed = self
            .commit_review
            .collapsed
            .contains(&file.entry.relative_path);
        let mut items = vec![ReviewItem::Header];
        let mut hunk_items = Vec::new();
        let mut unified_widest = 0;
        let splits = if unified || collapsed {
            Default::default()
        } else {
            [DiffKind::Preview, DiffKind::Staged, DiffKind::Working].map(|kind| {
                Self::split_diff(index, file, kind).map(|diff| self.split_layout(&diff))
            })
        };
        let split_items =
            |kind: DiffKind| match (Self::split_diff(index, file, kind), &splits[kind as usize]) {
                (Some(diff), Some(split)) => split_parts(&diff, &split.regions),
                _ => Vec::new(),
            };
        let note = |text: &str, tone: NoteTone, small: bool| ReviewItem::Note {
            text: text.to_string().into(),
            tone,
            small,
        };
        let diff = |items: &mut Vec<ReviewItem>, kind: DiffKind| {
            if unified {
                items.push(ReviewItem::UnifiedStaged(kind));
            } else {
                items.extend(
                    split_items(kind)
                        .into_iter()
                        .map(|part| ReviewItem::Split(kind, part)),
                );
            }
        };
        match &file.content {
            _ if collapsed => {}
            Err(reason) => {
                items.push(note(reason, NoteTone::Secondary, false));
                if file.preview.is_some() {
                    diff(&mut items, DiffKind::Preview);
                }
            }
            Ok(content) => {
                if content.selection.is_none() {
                    items.push(note(
                        "Staged version differs from working tree. Stage the whole file to replace it, or unstage it.",
                        NoteTone::Warning,
                        false,
                    ));
                }
                if content.staged_diff.is_some() {
                    items.push(note(
                        "Staged changes · read-only",
                        NoteTone::Secondary,
                        true,
                    ));
                    diff(&mut items, DiffKind::Staged);
                    items.push(note(
                        "Working-tree changes · line staging unavailable",
                        NoteTone::Secondary,
                        true,
                    ));
                }
                let hunks = content.changes.hunks();
                if hunks.is_empty() {
                    items.push(note(
                        "No text changes against HEAD. Use whole-file actions for staged-only or mode changes.",
                        NoteTone::Primary,
                        false,
                    ));
                } else {
                    let parts = if unified {
                        // Every hunk is as wide as the widest line, so they scroll sideways together.
                        let worktree_len = content.changes.worktree_len();
                        let [old, new] = widest(
                            &content.changes,
                            hunks.iter().map(|hunk| {
                                (
                                    hunk.before.start.saturating_sub(3)..hunk.before.end,
                                    hunk.after.start..(hunk.after.end + 3).min(worktree_len),
                                )
                            }),
                        );
                        unified_widest = old.max(new);
                        unified_parts(&content.changes)
                    } else {
                        split_items(DiffKind::Working)
                    };
                    let regions = splits[DiffKind::Working as usize]
                        .as_ref()
                        .map_or(&[][..], |split| &split.regions);
                    hunk_items = vec![0; hunks.len()];
                    for part in parts {
                        let item = items.len();
                        match part {
                            DiffPart::Hunk { index, .. } => hunk_items[index] = item,
                            DiffPart::Region { index, .. } => {
                                for hunk in regions[index].hunks.clone() {
                                    hunk_items[hunk] = item;
                                }
                            }
                            _ => {}
                        }
                        items.push(if unified {
                            ReviewItem::Unified(part)
                        } else {
                            ReviewItem::Split(DiffKind::Working, part)
                        });
                    }
                }
            }
        }
        ReviewLayout {
            file: file.clone(),
            index,
            unified,
            collapsed,
            expanded,
            items,
            hunk_items,
            splits,
            unified_widest,
        }
    }

    // Keyboard navigation includes offscreen controls without building their UI.
    fn review_controls(
        &mut self,
        files: &[(usize, Arc<ReviewFile>)],
        parts: usize,
        cx: &mut Context<Self>,
    ) {
        self.commit_review.controls.clear();
        let mut live = HashSet::new();
        for (index, file) in files.iter().map(|(index, file)| (*index, file)) {
            let mut changes = vec![StageChange::File(file.state() != SelectionState::Staged)];
            if !self
                .commit_review
                .collapsed
                .contains(&file.entry.relative_path)
                && let Ok(content) = &file.content
            {
                let mut remaining = MAX_VISIBLE_LINES;
                for (hunk_index, hunk) in content.changes.hunks().iter().enumerate() {
                    if remaining == 0 {
                        break;
                    }
                    let selection = content.selection.as_ref();
                    changes.push(StageChange::Hunk(
                        hunk_index,
                        selection.is_none_or(|s| s.hunk_state(hunk) != SelectionState::Staged),
                    ));
                    for (removal, line) in hunk
                        .before
                        .clone()
                        .map(|line| (true, line))
                        .chain(hunk.after.clone().map(|line| (false, line)))
                        .take(remaining)
                    {
                        changes.push(StageChange::Line {
                            removal,
                            line,
                            staged: !selection.is_some_and(|s| {
                                if removal {
                                    s.is_removal_staged(line)
                                } else {
                                    s.is_addition_staged(line)
                                }
                            }),
                        });
                    }
                    remaining = remaining.saturating_sub(hunk.before.len() + hunk.after.len());
                }
            }
            live.insert(format!("commit-section-{index}"));
            live.extend((1..parts).map(|part| format!("commit-part-{index}-{part}")));
            for change in changes {
                let id = change.control_id(index);
                let focus = self
                    .commit_review
                    .control_focus
                    .entry(id.clone())
                    .or_insert_with(|| cx.focus_handle())
                    .clone();
                live.insert(id);
                self.commit_review
                    .controls
                    .push((focus, file.clone(), change));
            }
        }
        // Forget controls no longer shown, which would otherwise accumulate
        // one handle for every line ever reviewed.
        self.commit_review
            .control_focus
            .retain(|id, _| live.contains(id));
    }

    pub(super) fn review_checkbox(
        &mut self,
        index: usize,
        label: String,
        state: SelectionState,
        file: Arc<ReviewFile>,
        change: StageChange,
        cx: &mut Context<Self>,
    ) -> Checkbox {
        let workspace = cx.entity().downgrade();
        let id = change.control_id(index);
        let focus = self.commit_review.control_focus[&id].clone();
        Checkbox::new(SharedString::from(id), label)
            .focus_handle(focus.clone())
            .size(CheckboxSize::Small)
            .checked(state == SelectionState::Staged)
            .indeterminate(state == SelectionState::Partial)
            // Changes queue behind a stage, but not behind a commit or push.
            .disabled(self.commit_review.operation.is_some())
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
        if self.commit_review.stale {
            self.refresh_commit_review(cx);
        }
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let style = diff_style(cx);
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
        // Staging controls queue behind a stage; only a commit or push locks them.
        let locked = self.commit_review.operation.is_some();
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
        // The list isn't drawn without files, so forget the last file's controls here.
        if files.is_empty() && self.commit_review.laid_out.take().is_some() {
            self.commit_review.shown = None;
            self.commit_review.hunk_items.clear();
            self.review_controls(&[], 0, cx);
        }
        let review_list = self
            .commit_review
            .list
            .get_or_insert_with(|| {
                let workspace = cx.entity();
                cx.new(|cx| ReviewList::new(workspace, cx))
            })
            .clone();
        let review_list = gpui::AnyView::from(review_list);
        // Tests read debug selectors, which GPUI drops from views it draws from cache.
        let review_list = if cfg!(test) {
            div()
                .flex_1()
                .min_h_0()
                .child(review_list)
                .into_any_element()
        } else {
            review_list
                .cached(gpui::StyleRefinement::default().flex_1().min_h_0())
                .into_any_element()
        };
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
            .overflow_hidden()
            .p(tokens.sizes.space_3)
            .flex()
            .flex_col()
            .when(!files.is_empty(), |list| list.child(review_list));
        if files.is_empty() && !busy && self.commit_review.error.is_none() {
            list = list.child(
                StateView::new("commit-review-empty", "No changes to commit")
                    .detail("Your working tree is clean.")
                    .icon("icons/git-branch.svg"),
            );
        }
        if self
            .commit_review
            .snapshot
            .as_ref()
            .is_some_and(|s| s.truncated)
        {
            list = list.child(
                div()
                    .px(tokens.sizes.space_3)
                    .py(tokens.sizes.space_2)
                    .text_size(tokens.sizes.text_xs)
                    .text_color(style.text_secondary)
                    .child("Some files are omitted by the status limit. Stage all affects only the files listed here."),
            );
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
            .on_drag_move(cx.listener(Self::resize_commit_message))
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
                    .child(
                        button("commit-refresh", "Refresh")
                            .icon("icons/refresh.svg")
                            .disabled(locked)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.commit_review.error = None;
                                this.refresh_commit_review(cx);
                            })),
                    )
                    .child(
                        button("commit-stage-all", "Stage all")
                            .icon("icons/plus.svg")
                            .disabled(locked || files.is_empty())
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
                            .icon("icons/minus.svg")
                            .disabled(locked || staged_count == 0)
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
                    )
                    .children(
                        [
                            (
                                "commit-layout-split",
                                "Side by side",
                                "icons/diff-split.svg",
                                false,
                            ),
                            (
                                "commit-layout-unified",
                                "Unified",
                                "icons/diff-unified.svg",
                                true,
                            ),
                        ]
                        .map(|(id, label, icon, unified)| {
                            div()
                                .when(cfg!(test), |view| view.debug_selector(|| id.into()))
                                .child(
                                    button(id, label)
                                        .icon(icon)
                                        .variant(if self.commit_review.unified == unified {
                                            ButtonVariant::Secondary
                                        } else {
                                            ButtonVariant::Ghost
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.commit_review.unified = unified;
                                            cx.notify();
                                        })),
                                )
                        }),
                    ),
            )
            .child(self.render_commit_message(cx))
            .child(self.render_commit_message_divider(cx))
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
            // A stage or the refresh after it would otherwise shift the diff on every click.
            .when(
                busy && (self.commit_review.operation.is_some()
                    || self.commit_review.pending.is_some()
                    || self.commit_review.snapshot.is_none()),
                |view| {
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
                },
            )
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
                                .icon("icons/git-commit.svg")
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
                                    .icon("icons/git-push.svg")
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
                                    button("commit-push-only", "Push")
                                        .icon("icons/git-push.svg")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.push_commit(cx)),
                                        ),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The shown file's diff as a list that lays out only the items on screen.
    fn render_review_list(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let style = diff_style(cx);
        // One file is shown at a time; the sidebar's Changes list picks which.
        let shown = self.selected_review_file();
        let previous = self.commit_review.laid_out.take();
        let layout = shown.as_ref().map(|(index, file)| {
            let path = &file.entry.relative_path;
            let expanded = self.expanded_runs(path);
            match &previous {
                Some((layout, _))
                    if Arc::ptr_eq(&layout.file, file)
                        && layout.index == *index
                        && layout.unified == self.commit_review.unified
                        && layout.collapsed == self.commit_review.collapsed.contains(path)
                        && layout.expanded == expanded =>
                {
                    layout.clone()
                }
                _ => std::rc::Rc::new(self.review_layout(*index, file, expanded)),
            }
        });
        let rebuilt = match (&previous, &layout) {
            (Some((old, _)), Some(new)) => !std::rc::Rc::ptr_eq(old, new),
            (old, new) => old.is_some() || new.is_some(),
        };
        let items: &[ReviewItem] = layout.as_ref().map_or(&[], |layout| &layout.items);
        let mut item_focus = Vec::new();
        if rebuilt {
            self.commit_review.hunk_items = layout
                .as_ref()
                .map(|layout| layout.hunk_items.clone())
                .unwrap_or_default();
            self.review_controls(&Vec::from_iter(shown.clone()), items.len(), cx);
            // Each item keeps focus handle, so a focused control stays in the
            // dispatch tree after it scrolls out of view.
            item_focus = shown
                .iter()
                .flat_map(|(index, _)| {
                    (0..items.len()).map(move |part| match part {
                        0 => format!("commit-section-{index}"),
                        _ => format!("commit-part-{index}-{part}"),
                    })
                })
                .map(|id| {
                    self.commit_review
                        .control_focus
                        .entry(id)
                        .or_insert_with(|| cx.focus_handle())
                        .clone()
                })
                .collect();
        }
        let shown_path = shown.as_ref().map(|(_, f)| f.entry.relative_path.clone());
        let new_file = self.commit_review.shown != shown_path;
        self.commit_review.shown = shown_path;
        let scroll = self
            .commit_review
            .scroll
            // The list can only scroll as far as it has measured, so measure
            // well past the viewport for each wheel step to land on laid-out items.
            .get_or_insert_with(|| gpui::ListState::new(0, gpui::ListAlignment::Top, px(1000.0)));
        let count = scroll.item_count();
        let row = row_height(cx);
        let measured = previous
            .as_ref()
            .map_or((&[][..], row), |(layout, row)| (&layout.items[..], *row));
        // A new layout is built only when the file or how it shows changed.
        if new_file {
            scroll.reset(0);
            scroll.splice_focusable(0..0, item_focus.into_iter().map(Some));
        } else if measured != (items, row) {
            if count != item_focus.len() {
                // Keep the items before the change, and the scroll position with them.
                let common = count.min(item_focus.len());
                scroll
                    .splice_focusable(common..count, item_focus.into_iter().skip(common).map(Some));
            }
            // Folding, layout and font metrics change item heights. Visible
            // items are laid out every frame anyway; this refreshes the rest.
            scroll.remeasure();
        }
        let last = items.len().saturating_sub(1);
        self.commit_review.laid_out = layout.clone().map(|layout| (layout, row));
        let workspace = cx.entity();
        let list_style = style.clone();
        gpui::list(scroll.clone(), move |item, _, cx| {
            let (Some((index, file)), Some(layout)) = (shown.clone(), layout.clone()) else {
                return div().into_any_element();
            };
            workspace.update(cx, |workspace, cx| {
                let body = workspace.render_review_item(
                    index,
                    &file,
                    &layout,
                    &layout.items[item],
                    &list_style,
                    cx,
                );
                // A unified hunk is one item; split rows open their hunk's menu themselves.
                let hunk = match layout.items[item] {
                    ReviewItem::Unified(DiffPart::Hunk { index, .. }) => Some(index),
                    _ => None,
                };
                let menu = workspace.review_menu_listener(&file, hunk, cx);
                let focus = workspace.commit_review.control_focus[&if item == 0 {
                    format!("commit-section-{index}")
                } else {
                    format!("commit-part-{index}-{item}")
                }]
                    .clone();
                // The items together draw one bordered box around the file.
                div()
                    .when(item == last, |item| item.pb(tokens.sizes.space_3))
                    .on_mouse_down(MouseButton::Right, menu)
                    .child(
                        div()
                            .track_focus(&focus)
                            .min_w_0()
                            .overflow_hidden()
                            .border_l_1()
                            .border_r_1()
                            .border_color(list_style.border)
                            .when(item == 0, |item| {
                                item.border_t_1().rounded_t(tokens.sizes.radius_md)
                            })
                            .when(item == last, |item| {
                                item.border_b_1().rounded_b(tokens.sizes.radius_md)
                            })
                            .child(body),
                    )
                    .into_any_element()
            })
        })
        .size_full()
        .into_any_element()
    }

    fn render_review_header(
        &mut self,
        index: usize,
        file: &Arc<ReviewFile>,
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
        div()
            .when(cfg!(test), |section| {
                section.debug_selector(move || format!("commit-shown-{index}"))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens.sizes.space_2)
                    .p(tokens.sizes.space_2)
                    .bg(style.surface)
                    .child(
                        Button::icon_only(
                            SharedString::from(format!("commit-collapse-{index}")),
                            if collapsed {
                                "icons/chevron-right.svg"
                            } else {
                                "icons/chevron-down.svg"
                            },
                        )
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::ExtraSmall)
                        .aria_label(if collapsed {
                            "Expand file diff"
                        } else {
                            "Collapse file diff"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.commit_review.collapsed.remove(&path) {
                                this.commit_review.collapsed.insert(path.clone());
                            }
                            cx.notify();
                        })),
                    )
                    .child(self.review_checkbox(
                        index,
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
                            .icon("icons/minus.svg")
                            .disabled(self.commit_review.operation.is_some())
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
            )
            .into_any_element()
    }

    fn render_review_item(
        &mut self,
        index: usize,
        file: &Arc<ReviewFile>,
        layout: &ReviewLayout,
        item: &ReviewItem,
        style: &DiffStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        match item {
            ReviewItem::Header => self.render_review_header(index, file, style, cx),
            ReviewItem::Note { text, tone, small } => div()
                .p(if *small {
                    tokens.sizes.space_2
                } else {
                    tokens.sizes.space_3
                })
                .text_color(match tone {
                    NoteTone::Primary => style.text,
                    NoteTone::Secondary => style.text_secondary,
                    NoteTone::Warning => tokens.editor.warning,
                })
                .child(text.clone())
                .into_any_element(),
            ReviewItem::Split(kind, part) => match (
                Self::split_diff(index, file, *kind),
                &layout.splits[*kind as usize],
            ) {
                (Some(diff), Some(split)) => self.render_split_part(diff, split, *part, style, cx),
                _ => div().into_any_element(),
            },
            ReviewItem::Unified(part) => {
                self.render_unified_part(index, file, layout.unified_widest, *part, style, cx)
            }
            ReviewItem::UnifiedStaged(kind) => {
                let changes = match (kind, &file.content) {
                    (DiffKind::Preview, _) => file.preview.as_ref().map(|p| &p.changes),
                    (_, Ok(content)) => content.staged_diff.as_ref(),
                    _ => None,
                };
                match changes {
                    Some(changes) => self.render_unified_staged(index, changes, style, cx),
                    None => div().into_any_element(),
                }
            }
        }
    }
}

impl Workspace {
    pub(super) fn render_unified_staged(
        &mut self,
        index: usize,
        staged: &FileChanges,
        style: &DiffStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let row_height = row_height(cx);
        let mut preview = div()
            .id(SharedString::from(format!("commit-staged-{index}")))
            .overflow_x_scroll()
            .font_family(style.font_family.clone())
            .text_size(style.font_size)
            .line_height(row_height);
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
                    .map(|row| render_diff_line(row, row_height)),
            );
            if remaining == 0 {
                break;
            }
        }
        preview.into_any_element()
    }

    /// One hunk of the unified working-tree diff, with its context.
    fn render_unified_part(
        &mut self,
        index: usize,
        file: &Arc<ReviewFile>,
        widest: u32,
        part: DiffPart,
        style: &DiffStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Ok(content) = &file.content else {
            return div().into_any_element();
        };
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let row_height = row_height(cx);
        // Lines and hunks stay clickable while a stage runs; clicks queue behind it.
        let locked = self.commit_review.operation.is_some();
        let (shown_hunk, mut visible) = match part {
            DiffPart::Hunk { index, visible } => (index, visible),
            DiffPart::Limited => {
                return div()
                    .p(tokens.sizes.space_3)
                    .child(format!(
                        "Preview limited to {MAX_VISIBLE_LINES} changed lines. Whole-file actions include all changes."
                    ))
                    .into_any_element();
            }
            _ => return div().into_any_element(),
        };
        let hunks = content.changes.hunks();
        let worktree_len = content.changes.worktree_len();
        let [width, _] = Self::columns_width([widest, 0], style, cx);
        let scroll = self.diff_scroll(&format!("commit-diff-{index}"));
        let body = div()
            .id(SharedString::from(format!(
                "commit-diff-{index}-{shown_hunk}"
            )))
            .track_scroll(&scroll[0])
            .font_family(style.font_family.clone())
            .text_size(style.font_size)
            .line_height(row_height);
        // Fixed gutters keep code aligned between context and changed rows.
        let checkbox_width = tokens.sizes.space_7;
        let mut body =
            body.child(div().w(checkbox_width + tokens.sizes.space_9 + px(24.0) + width));
        body.style().restrict_scroll_to_axis = Some(true);
        let line_number = |line: u32| {
            div()
                .w(tokens.sizes.space_9)
                .flex_none()
                .text_right()
                .text_color(style.text_secondary)
                .child((line + 1).to_string())
        };
        let context_line = |line: u32, bytes: &[u8]| {
            div()
                .flex()
                .items_center()
                .h(row_height)
                .text_color(style.text_secondary)
                .child(div().w(checkbox_width).flex_none())
                .child(line_number(line))
                // Matches the sign column of changed rows.
                .child(div().w(px(24.0)).flex_none())
                .child(
                    div()
                        .whitespace_nowrap()
                        .child(crate::diff::expand_tabs(&line_text(bytes))),
                )
        };
        for (hunk_index, hunk) in hunks.iter().enumerate().skip(shown_hunk).take(1) {
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
                    index,
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
                .disabled(locked || content.selection.is_none());
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .h(row_height)
                    .bg(style.surface)
                    .px(tokens.sizes.space_3)
                    .child(header),
            );
            // Skip lines the previous hunk already shows as trailing context.
            let context_start = hunk_index
                .checked_sub(1)
                .map(|prev| {
                    let end = hunks[prev].before.end;
                    end + (hunk.before.start - end).min(3)
                })
                .unwrap_or(0)
                .max(hunk.before.start.saturating_sub(3));
            for old in context_start..hunk.before.start {
                body = body.child(context_line(old, content.changes.head_line(old)));
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
            for ((removal, line), row) in locations.zip(diff_lines(&removed, &added, style)) {
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
                        index,
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
                    .disabled(locked || content.selection.is_none());
                body = body.child(
                    div()
                        .when(cfg!(test), |cell| {
                            cell.debug_selector(move || {
                                format!("commit-row-{index}-{removal}-{line}")
                            })
                        })
                        .flex()
                        .items_center()
                        .flex_none()
                        .h(row_height)
                        .child(div().w(checkbox_width).flex_none().child(checkbox))
                        .child(line_number(line))
                        .child(render_diff_line(row, row_height)),
                );
            }
            let end = hunks
                .get(hunk_index + 1)
                .map(|next| next.after.start)
                .unwrap_or(worktree_len);
            for new in hunk.after.end..end.min(hunk.after.end + 3) {
                body = body.child(context_line(new, content.changes.worktree_line(new)));
            }
        }
        body.into_any_element()
    }
}
