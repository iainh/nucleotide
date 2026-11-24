//! The sidebar's History list: commits drawn as a graph, loaded a page at a
//! time in the background. Selecting a commit shows its diff in the Commit tab
//! with the review's side-by-side diff.
use super::split::{DiffPart, LineHighlights, SplitDiff, split_parts};
use super::view::MAX_VISIBLE_LINES;
use super::*;
use crate::diff::DiffStyle;
use gpui::{Hsla, PathBuilder, canvas, size};
use nucleotide_types::VcsStatus;
use nucleotide_ui::VcsStatusStyle;
use nucleotide_ui::scrollbar::{ListScrollHandle, Scrollbar};
use nucleotide_ui::{ListItem, ListItemSpacing, ListItemVariant, NavigableList, StateViewTone};
use nucleotide_vcs::VcsServiceHandle;
use nucleotide_vcs::history::{
    CommitChanges, CommitRef, CommitSummary, FileChangeKind, GraphRow, Half, HistoryPages, RefKind,
};

/// The status a commit gave a file, in the vocabulary the other VCS panels use.
fn commit_file_status(kind: FileChangeKind) -> VcsStatus {
    match kind {
        FileChangeKind::Added => VcsStatus::Added,
        FileChangeKind::Deleted => VcsStatus::Deleted,
        FileChangeKind::Renamed => VcsStatus::Renamed,
        FileChangeKind::Copied => VcsStatus::Copied,
        FileChangeKind::Modified | FileChangeKind::TypeChanged => VcsStatus::Modified,
        FileChangeKind::Other => VcsStatus::Unknown,
    }
}

/// Commits read first, enough to fill the list without waiting for more.
const FIRST_PAGE: usize = 250;
/// Commits read per later page, which load as the list nears its end.
const PAGE: usize = 2000;
/// Rows left below the viewport when the next page starts loading.
const PREFETCH: usize = 300;
/// Lanes shown before the graph column stops growing; further lanes are clipped.
const MAX_LANES: usize = 10;

#[derive(Default)]
pub(in crate::workspace) struct History {
    /// The Commit tab shows the selected commit rather than the working-tree review.
    pub(in crate::workspace) in_tab: bool,
    /// The commits paging starts from; `None` until the first page loads.
    tips: Option<Vec<String>>,
    /// HEAD and its branch when the list last reloaded.
    head: Option<(Option<String>, Option<String>)>,
    commits: Vec<CommitSummary>,
    rows: Vec<GraphRow>,
    pages: HistoryPages,
    /// The most lanes any loaded row uses.
    lanes: usize,
    loading: bool,
    complete: bool,
    /// Why the last load failed. Pages stop loading until Retry.
    error: Option<String>,
    /// Discards pages and reloads requested before the last reload.
    generation: u64,
    /// The repository changed while the list was hidden.
    stale: bool,
    scroll: Scrolled<gpui::UniformListScrollHandle>,
    /// The list, which holds focus while it is navigated with keys.
    pub(in crate::workspace) focus: Option<FocusHandle>,
    /// The row the keyboard or a click selected last.
    cursor: Option<usize>,
    selected: Option<String>,
    shown: Option<Result<Arc<ShownCommit>, String>>,
    collapsed: HashSet<PathBuf>,
    list: Option<Scrolled<ListScrollHandle>>,
    /// The commit message body, which scrolls when it is long.
    body_scroll: Scrolled<gpui::ScrollHandle>,
    /// The commit and items the diff list was last laid out for.
    laid_out: Option<(String, Vec<HistoryItem>)>,
}

struct ShownCommit {
    commit: CommitSummary,
    body: String,
    files: Vec<Arc<HistoryFile>>,
}

struct HistoryFile {
    path: PathBuf,
    old_path: Option<PathBuf>,
    kind: FileChangeKind,
    diff: Result<FileDiff, String>,
}

struct FileDiff {
    changes: FileChanges,
    old_len: u32,
    highlights: [Option<LineHighlights>; 2],
    added: usize,
    removed: usize,
}

#[derive(Clone, PartialEq)]
enum HistoryItem {
    File(usize),
    Note(usize, SharedString),
    Split(usize, DiffPart),
    Unified(usize),
}

/// Line diffs and highlights of each changed file, computed off the UI thread.
fn shown_commit(
    commit: CommitSummary,
    changes: CommitChanges,
    loader: Option<&helix_core::syntax::Loader>,
) -> ShownCommit {
    let files = changes
        .files
        .into_iter()
        .map(|file| {
            let diff = file.contents.and_then(|[old, new]| {
                let (old, new) = (old.unwrap_or_default(), new.unwrap_or_default());
                if !supports_line_staging(&[&old, &new])
                    || std::str::from_utf8(&old).is_err()
                    || std::str::from_utf8(&new).is_err()
                {
                    return Err("Binary or non-UTF-8 file".to_string());
                }
                let changes = FileChanges::new(&old, &new);
                let (added, removed) = changes.hunks().iter().fold((0, 0), |(a, r), hunk| {
                    (a + hunk.after.len(), r + hunk.before.len())
                });
                let highlight = |text: &[u8]| split::line_highlights(text, &file.path, loader?);
                Ok(FileDiff {
                    old_len: old.split_inclusive(|b| *b == b'\n').count() as u32,
                    highlights: [highlight(&old), highlight(&new)],
                    changes,
                    added,
                    removed,
                })
            });
            Arc::new(HistoryFile {
                path: file.path,
                old_path: file.old_path,
                kind: file.kind,
                diff,
            })
        })
        .collect();
    ShownCommit {
        commit,
        body: changes.body,
        files,
    }
}

/// How long ago `time` was, in the largest whole unit.
fn relative_time(now: i64, time: i64) -> String {
    let seconds = (now - time).max(0);
    let (count, unit) = match seconds {
        0..60 => return "just now".into(),
        60..3_600 => (seconds / 60, "minute"),
        3_600..86_400 => (seconds / 3_600, "hour"),
        86_400..604_800 => (seconds / 86_400, "day"),
        604_800..2_629_746 => (seconds / 604_800, "week"),
        2_629_746..31_556_952 => (seconds / 2_629_746, "month"),
        _ => (seconds / 31_556_952, "year"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(7)]
}

/// A commit's local date and time, such as "2026-11-24 14:05".
fn commit_date(time: i64) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// Keys of the History list, shown in its rows' tooltips.
const HISTORY_KEYS: &str = "↑↓ select · Enter shows the commit";

impl Workspace {
    /// Shows the History list, loading it when needed.
    pub(in crate::workspace) fn show_history_sidebar(&mut self, cx: &mut Context<Self>) {
        self.commit_review.sidebar = Sidebar::History;
        let history = &mut self.commit_review.history;
        if history.tips.is_none() || std::mem::take(&mut history.stale) {
            self.reload_history(false, cx);
        }
        cx.notify();
    }

    /// The repository may have new commits or moved refs.
    pub(in crate::workspace) fn history_changed(&mut self, cx: &mut Context<Self>) {
        if self.commit_review.sidebar == Sidebar::History {
            self.reload_history(false, cx);
        } else {
            self.commit_review.history.stale = true;
        }
    }

    /// Git status changed or HEAD moved. Status changes with most edits, so
    /// the list reloads only when HEAD or its branch moved since it last did.
    pub(in crate::workspace) fn history_head_changed(&mut self, cx: &mut Context<Self>) {
        if self.commit_review.history.head != Some(Self::vcs_head(cx)) {
            self.history_changed(cx);
        }
    }

    fn vcs_head(cx: &App) -> (Option<String>, Option<String>) {
        let service = cx.global::<VcsServiceHandle>().service().read(cx);
        (
            service.repository_head().map(str::to_owned),
            service.repository_ref().map(str::to_owned),
        )
    }

    /// Reads the refs and, when they moved or `force` is set, the first page.
    fn reload_history(&mut self, force: bool, cx: &mut Context<Self>) {
        let head = Self::vcs_head(cx);
        let core = self.core.read(cx);
        let Some(directory) = core.project_directory.clone() else {
            return;
        };
        let backend = core.workspace_backend.clone();
        let history = &mut self.commit_review.history;
        history.head = Some(head);
        history.generation += 1;
        history.loading = true;
        let generation = history.generation;
        let previous = if force { None } else { history.tips.clone() };
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    handle.block_on(async move {
                        let repository = GitRepository::discover(backend, &directory)
                            .await?
                            .ok_or("This workspace is not a Git repository")?;
                        let tips = repository.history_tips().await?;
                        if previous.as_ref() == Some(&tips) {
                            return Ok(None);
                        }
                        let pages = HistoryPages::new(tips.clone());
                        let page = LoadedPage::read(&repository, pages, FIRST_PAGE).await?;
                        Ok::<_, String>(Some((tips, page)))
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let history = &mut this.commit_review.history;
                if history.generation != generation {
                    return;
                }
                history.loading = false;
                match result {
                    Ok(None) => {}
                    Ok(Some((tips, page))) => {
                        history.error = None;
                        history.tips = Some(tips);
                        history.clear();
                        history.add_page(page);
                    }
                    Err(error) => {
                        history.tips = Some(Vec::new());
                        history.clear();
                        history.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Loads the page after the commits already listed. After a failure it
    /// waits for Retry, since the list asks for the page on every layout.
    fn load_history_page(&mut self, cx: &mut Context<Self>) {
        let history = &self.commit_review.history;
        if history.tips.is_none() || history.loading || history.complete || history.error.is_some()
        {
            return;
        }
        let core = self.core.read(cx);
        let Some(directory) = core.project_directory.clone() else {
            return;
        };
        let backend = core.workspace_backend.clone();
        let history = &mut self.commit_review.history;
        history.loading = true;
        let generation = history.generation;
        let pages = history.pages.clone();
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    handle.block_on(async move {
                        let repository = GitRepository::discover(backend, &directory)
                            .await?
                            .ok_or("This workspace is not a Git repository")?;
                        LoadedPage::read(&repository, pages, PAGE).await
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let history = &mut this.commit_review.history;
                if history.generation != generation {
                    return;
                }
                history.loading = false;
                match result {
                    Ok(page) => history.add_page(page),
                    Err(error) => history.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Repeats the load that failed.
    fn retry_history(&mut self, cx: &mut Context<Self>) {
        let history = &mut self.commit_review.history;
        history.error = None;
        if history.commits.is_empty() {
            self.reload_history(true, cx);
        } else {
            self.load_history_page(cx);
        }
        cx.notify();
    }

    /// Moves the list's selection to row `index` without loading its commit.
    fn move_history_cursor(&mut self, index: usize, cx: &mut Context<Self>) {
        self.commit_review.history.cursor = Some(index);
        cx.notify();
    }

    /// Shows commit `index` of the list in the Commit tab. Focus stays in the list.
    fn select_history_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(commit) = self.commit_review.history.commits.get(index).cloned() else {
            return;
        };
        let history = &mut self.commit_review.history;
        history.cursor = Some(index);
        history.in_tab = true;
        if history.selected.as_ref() != Some(&commit.id) {
            history.selected = Some(commit.id.clone());
            history.shown = None;
            history.collapsed.clear();
            self.commit_review
                .scrolls
                .retain(|id, _| !id.starts_with("history-"));
            let core = self.core.read(cx);
            if let Some(directory) = core.project_directory.clone() {
                let backend = core.workspace_backend.clone();
                let loader = core.editor.syn_loader.load_full();
                let handle = self.handle.clone();
                cx.spawn(async move |this, cx| {
                    let id = commit.id.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            handle.block_on(async move {
                                let repository = GitRepository::discover(backend, &directory)
                                    .await?
                                    .ok_or("This workspace is not a Git repository")?;
                                let changes = repository.commit_changes(&commit.id).await?;
                                Ok::<_, String>(Arc::new(shown_commit(
                                    commit,
                                    changes,
                                    Some(&loader),
                                )))
                            })
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        let history = &mut this.commit_review.history;
                        if history.selected.as_ref() == Some(&id) {
                            history.shown = Some(result);
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        // The history view has no message editor to type into.
        self.pane_host.commit_message_focused = false;
        self.activate_commit_tab(false, cx);
    }

    fn lane_colors(cx: &App) -> [Hsla; 6] {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        [
            tokens.editor.info,
            tokens.editor.vcs_added,
            tokens.editor.warning,
            tokens.editor.diagnostic_hint,
            tokens.editor.vcs_deleted,
            tokens.chrome.primary,
        ]
    }

    fn render_ref_pill(commit_ref: &CommitRef, cx: &App) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let (color, icon) = match commit_ref.kind {
            RefKind::Tag => (tokens.editor.warning, "icons/tag.svg"),
            RefKind::Remote => (tokens.editor.text_secondary, "icons/git-branch.svg"),
            RefKind::Head | RefKind::Branch => (
                if commit_ref.current {
                    tokens.chrome.primary
                } else {
                    tokens.editor.vcs_added
                },
                "icons/git-branch.svg",
            ),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(tokens.sizes.space_1)
            .px(tokens.sizes.space_2)
            .rounded(tokens.sizes.radius_full)
            .border_1()
            .border_color(color.opacity(0.5))
            .bg(color.opacity(0.14))
            .text_color(color)
            .text_size(tokens.sizes.text_xs)
            .line_height(relative(1.4))
            .when(commit_ref.current, |pill| {
                pill.font_weight(FontWeight::SEMIBOLD)
            })
            .child(
                nucleotide_ui::icon()
                    .path(icon)
                    .size(tokens.sizes.text_xs)
                    .flex_none(),
            )
            .child(commit_ref.name.clone())
            .into_any_element()
    }

    fn render_history_row(
        &self,
        index: usize,
        now: i64,
        file_tree: &nucleotide_ui::tokens::FileTreeTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let history = &self.commit_review.history;
        let (commit, row) = (&history.commits[index], history.rows[index].clone());
        let selected = history.cursor == Some(index);
        let colors = Self::lane_colors(cx);
        let lane_width = tokens.sizes.space_5;
        let graph_width = lane_width * history.lanes.clamp(1, MAX_LANES) as f32;
        let row_height = Self::history_row_height(cx);
        let stroke = tokens.sizes.space_1;
        let dot = tokens.sizes.space_3;
        let ring = tokens.chrome.file_tree_background;
        let merge = commit.parents.len() > 1;
        let graph = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let color = |index: usize| colors[index % colors.len()];
                let x = |lane: usize| bounds.origin.x + lane_width * (lane as f32 + 0.5);
                let top = bounds.origin.y;
                let middle = top + bounds.size.height * 0.5;
                let bottom = top + bounds.size.height;
                for line in &row.lines {
                    let (start, end) = match line.half {
                        Half::Upper => (top, middle),
                        Half::Lower => (middle, bottom),
                    };
                    let mut path = PathBuilder::stroke(stroke);
                    path.move_to(point(x(line.from), start));
                    if line.from == line.to {
                        path.line_to(point(x(line.to), end));
                    } else {
                        let bend = (start + end) * 0.5;
                        path.cubic_bezier_to(
                            point(x(line.to), end),
                            point(x(line.from), bend),
                            point(x(line.to), bend),
                        );
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color(line.color));
                    }
                }
                let center = point(x(row.lane), middle);
                let radius = dot * 0.5 + stroke;
                window.paint_quad(gpui::quad(
                    gpui::Bounds::new(
                        point(center.x - radius, center.y - radius),
                        size(radius * 2.0, radius * 2.0),
                    ),
                    radius,
                    // A merge is drawn hollow, like a junction.
                    if merge { ring } else { color(row.color) },
                    stroke,
                    color(row.color),
                    gpui::BorderStyle::Solid,
                ));
            },
        )
        .size_full();
        let (text, secondary) = if selected {
            (file_tree.item_text_selected, file_tree.item_text_selected)
        } else {
            (file_tree.item_text, file_tree.item_text_secondary)
        };
        let item = ListItem::new(("history-row", index))
            .variant(ListItemVariant::Ghost)
            .spacing(ListItemSpacing::Compact)
            // The graph runs from the top of the row to the bottom to join its neighbours.
            .padding_y(tokens.sizes.space_0)
            .stretch_start_slot()
            .background(if selected {
                file_tree.item_background_selected
            } else {
                gpui::transparent_black()
            })
            .hover_background(if selected {
                file_tree.item_background_selected_hover
            } else {
                file_tree.item_background_hover
            })
            .text_color(text)
            .tooltip(commit.subject.clone())
            .tooltip_detail(format!(
                "{} · {} · {}\n{HISTORY_KEYS}",
                short_id(&commit.id),
                commit_date(commit.time),
                commit.author
            ))
            .start_slot(
                div()
                    .w(graph_width)
                    .flex_1()
                    .min_h(row_height)
                    .overflow_hidden()
                    .child(graph),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(tokens.sizes.space_1)
                    .overflow_hidden()
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(commit.subject.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(tokens.sizes.space_2)
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(tokens.sizes.text_xs)
                            .text_color(secondary)
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(tokens.sizes.space_10 * 2.5)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(commit.author.clone()),
                            )
                            // The full date is in the tooltip, so a narrow sidebar truncates this.
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(relative_time(now, commit.time)),
                            )
                            // Pills clip at the edge rather than hiding the author.
                            .when(!commit.refs.is_empty(), |line| {
                                line.child(
                                    div()
                                        .flex()
                                        .flex_shrink(1.0)
                                        .min_w_0()
                                        .overflow_hidden()
                                        .gap(tokens.sizes.space_1)
                                        .children(
                                            commit
                                                .refs
                                                .iter()
                                                .map(|r| Self::render_ref_pill(r, cx)),
                                        ),
                                )
                            }),
                    ),
            )
            .with_listener({
                let listener = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.select_history_commit(index, cx)
                });
                move |row| row.on_click(listener)
            });
        div()
            .when(cfg!(test), |view| {
                view.debug_selector(move || format!("history-row-{index}"))
            })
            .child(item)
            .into_any_element()
    }

    /// Why the first page, or with `more` a later page, failed to load.
    fn render_history_error(error: String, more: bool, cx: &mut Context<Self>) -> StateView {
        let title = if more {
            "Cannot load more history"
        } else {
            "Cannot load history"
        };
        StateView::new("history-sidebar-error", title)
            .detail(error)
            .icon("icons/triangle-alert.svg")
            .tone(StateViewTone::Error)
            .compact(true)
            .action(
                Button::new("history-retry", "Retry")
                    .icon("icons/refresh.svg")
                    .variant(ButtonVariant::Secondary)
                    .size(ButtonSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.retry_history(cx))),
            )
    }

    fn history_row_height(cx: &App) -> Pixels {
        let sizes = cx.global::<nucleotide_ui::Theme>().tokens.sizes;
        sizes.space_9 + sizes.space_2
    }

    pub(in crate::workspace) fn render_history_sidebar(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let file_tree = if window.is_window_active() {
            tokens.file_tree_tokens()
        } else {
            tokens.file_tree_tokens().inactive_window(&tokens.chrome)
        };
        self.commit_review
            .history
            .focus
            .get_or_insert_with(|| cx.focus_handle());
        let history = &self.commit_review.history;
        let count = history.commits.len();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(tokens.sizes.space_8)
            .px(tokens.sizes.space_3)
            .gap(tokens.sizes.space_2)
            .text_size(tokens.sizes.text_xs)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(file_tree.item_text_secondary)
            .child(div().flex_1().child("COMMIT GRAPH"))
            .when(history.loading, |header| {
                header.child(
                    nucleotide_ui::icon()
                        .path("icons/loader-circle.svg")
                        .size(tokens.sizes.text_md),
                )
            })
            .child(
                Button::icon_only("history-refresh", "icons/refresh.svg")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .tooltip("Reload History")
                    .aria_label("Reload history")
                    .on_click(cx.listener(|this, _, _, cx| this.reload_history(true, cx))),
            );
        let body = if let (0, Some(error)) = (count, &history.error) {
            Self::render_history_error(error.clone(), false, cx).into_any_element()
        } else if count == 0 {
            let loading = history.loading || history.tips.is_none();
            let title = if loading {
                "Loading history…"
            } else {
                "No commits yet"
            };
            StateView::new("history-sidebar-empty", title)
                .loading(loading)
                .compact(true)
                .into_any_element()
        } else {
            let now = now();
            let focus = history.focus.clone().unwrap_or_else(|| cx.focus_handle());
            let scroll = history.scroll.clone();
            let cursor = history.cursor;
            let list = gpui::uniform_list(
                "history-list",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let history = &this.commit_review.history;
                    if range.end + PREFETCH >= history.commits.len() {
                        this.load_history_page(cx);
                    }
                    let file_tree = if window.is_window_active() {
                        tokens.file_tree_tokens()
                    } else {
                        tokens.file_tree_tokens().inactive_window(&tokens.chrome)
                    };
                    range
                        .filter(|index| *index < this.commit_review.history.commits.len())
                        .map(|index| this.render_history_row(index, now, &file_tree, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&scroll.handle)
            .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::FitList)
            .size_full()
            .px(tokens.sizes.space_1);
            let workspace = cx.entity().downgrade();
            let (select, open) = (workspace.clone(), workspace);
            let list = NavigableList::new("history-navigation", &focus, count)
                .selected(cursor)
                .track_scroll(&scroll.handle)
                .on_select(move |index, _, cx| {
                    let _ = select.update(cx, |this, cx| this.move_history_cursor(index, cx));
                })
                .on_confirm(move |index, _, cx| {
                    let _ = open.update(cx, |this, cx| this.select_history_commit(index, cx));
                })
                .relative()
                .flex_1()
                .min_h_0()
                .child(list)
                .child(Scrollbar::vertical_overlay(scroll.bar));
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(list)
                .when_some(history.error.clone(), |view, error| {
                    view.child(Self::render_history_error(error, true, cx))
                })
                .into_any_element()
        };
        div()
            .id("history-sidebar")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(tokens.sizes.text_md)
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn history_items(&self, shown: &ShownCommit) -> Vec<HistoryItem> {
        let mut items = Vec::new();
        let unified = self.commit_review.unified;
        for (index, file) in shown.files.iter().enumerate() {
            items.push(HistoryItem::File(index));
            if self.commit_review.history.collapsed.contains(&file.path) {
                continue;
            }
            match &file.diff {
                Err(reason) => items.push(HistoryItem::Note(index, reason.clone().into())),
                Ok(diff) if diff.changes.hunks().is_empty() => items.push(HistoryItem::Note(
                    index,
                    match file.kind {
                        FileChangeKind::Copied => "Copied without text changes",
                        FileChangeKind::Renamed => "Renamed without text changes",
                        _ => "No text changes (mode change or empty file)",
                    }
                    .into(),
                )),
                Ok(_) if unified => items.push(HistoryItem::Unified(index)),
                Ok(diff) => {
                    let split = Self::history_split(index, file, diff);
                    items.extend(
                        split_parts(&split, &self.split_regions(&split))
                            .into_iter()
                            .map(|part| HistoryItem::Split(index, part)),
                    )
                }
            }
        }
        items
    }

    fn history_split<'a>(index: usize, file: &'a HistoryFile, diff: &'a FileDiff) -> SplitDiff<'a> {
        SplitDiff {
            id: format!("history-{index}"),
            path: &file.path,
            staged: true,
            titles: ["Before", "After"],
            changes: &diff.changes,
            old_len: diff.old_len,
            highlights: [diff.highlights[0].as_ref(), diff.highlights[1].as_ref()],
            staging: None,
        }
    }

    fn render_history_file_header(
        &mut self,
        file: &HistoryFile,
        style: &DiffStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let path = file.path.clone();
        let collapsed = self.commit_review.history.collapsed.contains(&path);
        let status = commit_file_status(file.kind);
        div()
            .flex()
            .items_center()
            .gap(tokens.sizes.space_2)
            .p(tokens.sizes.space_2)
            .bg(style.surface)
            .child(
                Button::icon_only(
                    SharedString::from(format!("history-collapse-{}", path.display())),
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
                    let collapsed = &mut this.commit_review.history.collapsed;
                    if !collapsed.remove(&path) {
                        collapsed.insert(path.clone());
                    }
                    cx.notify();
                })),
            )
            .child(
                div()
                    .w(tokens.sizes.space_4)
                    .text_color(status.color(&tokens))
                    .child(status.label()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(match &file.old_path {
                        Some(old) => format!("{} → {}", old.display(), file.path.display()),
                        None => file.path.display().to_string(),
                    }),
            )
            .when_some(file.diff.as_ref().ok(), |header, diff| {
                header
                    .child(
                        div()
                            .text_color(tokens.editor.vcs_added)
                            .child(format!("+{}", diff.added)),
                    )
                    .child(
                        div()
                            .text_color(tokens.editor.vcs_deleted)
                            .child(format!("−{}", diff.removed)),
                    )
            })
            .into_any_element()
    }

    fn render_history_item(
        &mut self,
        shown: &ShownCommit,
        item: &HistoryItem,
        style: &DiffStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let note = |text: SharedString| {
            div()
                .p(tokens.sizes.space_3)
                .text_color(style.text_secondary)
                .child(text)
                .into_any_element()
        };
        match item {
            HistoryItem::File(index) => {
                self.render_history_file_header(&shown.files[*index], style, cx)
            }
            HistoryItem::Note(_, text) => note(text.clone()),
            HistoryItem::Split(_, DiffPart::Limited) => {
                note(format!("Diff limited to {MAX_VISIBLE_LINES} changed lines.").into())
            }
            HistoryItem::Split(index, part) => {
                let file = &shown.files[*index];
                match &file.diff {
                    Ok(diff) => {
                        let split = Self::history_split(*index, file, diff);
                        let layout = self.split_layout(&split);
                        self.render_split_part(split, &layout, *part, style, cx)
                    }
                    Err(_) => div().into_any_element(),
                }
            }
            HistoryItem::Unified(index) => match &shown.files[*index].diff {
                Ok(diff) => self.render_unified_staged(
                    format!("history-unified-{index}"),
                    &diff.changes,
                    style,
                    cx,
                ),
                Err(_) => div().into_any_element(),
            },
        }
    }

    /// The selected commit in the Commit tab: its message and side-by-side diff.
    pub(in crate::workspace) fn render_history_commit(
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
            sign_width: tokens.sizes.space_7,
        };
        let root = div()
            .id("history-commit")
            .when_some(self.commit_review.focus.as_ref(), |view, focus| {
                view.track_focus(focus)
            })
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(tokens.editor.background)
            .text_color(style.text)
            .text_size(tokens.sizes.text_sm);
        let shown = match self.commit_review.history.shown.clone() {
            Some(Ok(shown)) => shown,
            Some(Err(error)) => {
                return root
                    .child(
                        StateView::new("history-commit-error", "Cannot load this commit")
                            .detail(error)
                            .icon("icons/triangle-alert.svg"),
                    )
                    .into_any_element();
            }
            None => {
                return root
                    .child(
                        StateView::new("history-commit-loading", "Loading commit…").loading(true),
                    )
                    .into_any_element();
            }
        };
        let commit = &shown.commit;
        let (added, removed) = shown
            .files
            .iter()
            .filter_map(|f| f.diff.as_ref().ok())
            .fold((0, 0), |(a, r), d| (a + d.added, r + d.removed));
        let items = self.history_items(&shown);
        // Long lines scroll sideways under a bar for the diff at the top of the view.
        let top = self
            .commit_review
            .history
            .list
            .as_ref()
            .map_or(0, |list| list.handle.0.logical_scroll_top().item_ix);
        let sideways = items.iter().skip(top).find_map(|item| match item {
            HistoryItem::Split(index, _) => Some((format!("history-{index}"), true)),
            HistoryItem::Unified(index) => Some((format!("history-unified-{index}"), false)),
            _ => None,
        });
        let sideways = sideways.map(|(id, split)| self.diff_scrollbars(&id, split, cx));
        let history = &mut self.commit_review.history;
        let scrolled = history.list.get_or_insert_with(Scrolled::list).clone();
        let body_scroll = history.body_scroll.clone();
        let list = scrolled.handle.0.clone();
        let layout = Some((commit.id.clone(), items.clone()));
        if history.laid_out != layout {
            if history
                .laid_out
                .as_ref()
                .is_some_and(|(id, _)| *id == commit.id)
            {
                // Folding keeps the scroll position of the items above.
                let count = list.item_count();
                list.splice(0..count, items.len());
            } else {
                list.reset(items.len());
            }
            history.laid_out = layout;
        }
        let last = items.len().saturating_sub(1);
        let workspace = cx.entity();
        let list_style = style.clone();
        let list_shown = shown.clone();
        let diff_list = gpui::list(list.clone(), move |item, _, cx| {
            workspace.update(cx, |workspace, cx| {
                let body =
                    workspace.render_history_item(&list_shown, &items[item], &list_style, cx);
                let first = matches!(items[item], HistoryItem::File(_));
                let end = items
                    .get(item + 1)
                    .is_none_or(|next| matches!(next, HistoryItem::File(_)));
                // Each file's items together draw one bordered box.
                div()
                    .when(end && item != last, |item| item.pb(tokens.sizes.space_3))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .border_l_1()
                            .border_r_1()
                            .border_color(list_style.border)
                            .when(first, |item| {
                                item.border_t_1().rounded_t(tokens.sizes.radius_md)
                            })
                            .when(end, |item| {
                                item.border_b_1().rounded_b(tokens.sizes.radius_md)
                            })
                            .child(body),
                    )
                    .into_any_element()
            })
        })
        .flex_1()
        .min_h_0();
        let unified = self.commit_review.unified;
        root.child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(tokens.sizes.space_2)
                .p(tokens.sizes.space_3)
                .border_b_1()
                .border_color(style.border)
                .child({
                    let shown = format!("history-shown-{}", short_id(&commit.id));
                    div()
                        .when(cfg!(test), |view| view.debug_selector(move || shown))
                        .text_size(tokens.sizes.text_lg)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(commit.subject.clone())
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(tokens.sizes.space_3)
                        .text_color(style.text_secondary)
                        .child(commit.author.clone())
                        .child(relative_time(now(), commit.time))
                        .child(
                            div()
                                .font_family(style.font_family.clone())
                                .child(short_id(&commit.id).to_string()),
                        )
                        .children(commit.refs.iter().map(|r| Self::render_ref_pill(r, cx))),
                )
                .when(!shown.body.is_empty(), |header| {
                    header.child(
                        div()
                            .relative()
                            .child(
                                div()
                                    .id("history-commit-body")
                                    .max_h(tokens.sizes.space_10 * 2.0)
                                    .overflow_y_scroll()
                                    .track_scroll(&body_scroll.handle)
                                    .font_family(style.font_family.clone())
                                    .text_color(style.text_secondary)
                                    .whitespace_normal()
                                    .child(shown.body.clone()),
                            )
                            .child(Scrollbar::vertical_overlay(body_scroll.bar)),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(tokens.sizes.space_3)
                        .child(format!(
                            "{} file{} changed",
                            shown.files.len(),
                            if shown.files.len() == 1 { "" } else { "s" }
                        ))
                        .child(
                            div()
                                .text_color(tokens.editor.vcs_added)
                                .child(format!("+{added}")),
                        )
                        .child(
                            div()
                                .text_color(tokens.editor.vcs_deleted)
                                .child(format!("−{removed}")),
                        )
                        .child(div().flex_1())
                        .children(
                            [
                                (
                                    "history-layout-split",
                                    "Side by side",
                                    "icons/diff-split.svg",
                                    false,
                                ),
                                (
                                    "history-layout-unified",
                                    "Unified",
                                    "icons/diff-unified.svg",
                                    true,
                                ),
                            ]
                            .map(|(id, label, icon, layout)| {
                                Button::new(id, label)
                                    .icon(icon)
                                    .size(ButtonSize::Small)
                                    .variant(if unified == layout {
                                        ButtonVariant::Secondary
                                    } else {
                                        ButtonVariant::Ghost
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.commit_review.unified = layout;
                                        cx.notify();
                                    }))
                            }),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .p(tokens.sizes.space_3)
                .when(!shown.files.is_empty(), |view| {
                    view.child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(diff_list)
                            .child(Scrollbar::vertical_overlay(scrolled.bar))
                            .children(sideways),
                    )
                })
                .when(shown.files.is_empty(), |view| {
                    view.child(StateView::new("history-commit-empty", "No file changes"))
                }),
        )
        .into_any_element()
    }
}

/// A page of commits, read and laid out off the UI thread.
struct LoadedPage {
    /// The history listed so far, including this page.
    pages: HistoryPages,
    commits: Vec<CommitSummary>,
    rows: Vec<GraphRow>,
    complete: bool,
}

impl LoadedPage {
    async fn read(
        repository: &GitRepository,
        mut pages: HistoryPages,
        limit: usize,
    ) -> Result<Self, String> {
        let commits = repository.log(&pages.starts(), limit).await?;
        let rows = pages.add(&commits);
        // A short page lists everything that was left.
        let complete = commits.len() < limit || pages.starts().is_empty();
        Ok(Self {
            pages,
            commits,
            rows,
            complete,
        })
    }
}

impl History {
    fn clear(&mut self) {
        self.cursor = None;
        self.commits.clear();
        self.rows.clear();
        self.pages = HistoryPages::default();
        self.lanes = 0;
        self.complete = false;
    }

    fn add_page(&mut self, page: LoadedPage) {
        self.pages = page.pages;
        self.complete = page.complete;
        self.lanes = page
            .rows
            .iter()
            .map(|row| row.lanes)
            .fold(self.lanes, usize::max);
        self.rows.extend(page.rows);
        self.commits.extend(page.commits);
    }
}

#[cfg(test)]
mod history_tests {
    use super::relative_time;

    #[test]
    fn relative_time_uses_the_largest_whole_unit() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now, now - 30), "just now");
        assert_eq!(relative_time(now, now - 60), "1 minute ago");
        assert_eq!(relative_time(now, now - 3 * 3_600 - 59), "3 hours ago");
        assert_eq!(relative_time(now, now - 86_400 * 13), "1 week ago");
        assert_eq!(relative_time(now, now - 86_400 * 400), "1 year ago");
        // Clock skew never shows a future time.
        assert_eq!(relative_time(now, now + 500), "just now");
    }
}
