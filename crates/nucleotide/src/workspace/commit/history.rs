//! The sidebar's History list: commits drawn as a graph, loaded a page at a
//! time in the background. Selecting a commit shows its diff in the Commit tab
//! with the review's side-by-side diff.
use super::split::{DiffPart, LineHighlights, SplitDiff};
use super::view::MAX_VISIBLE_LINES;
use super::*;
use crate::diff::DiffStyle;
use gpui::{Hsla, PathBuilder, canvas, size};
use nucleotide_vcs::history::{
    CommitChanges, CommitGraph, CommitRef, CommitSummary, FileChangeKind, GraphRow, Half, RefKind,
};

/// Commits read per page. Later pages load as the list nears its end.
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
    commits: Vec<CommitSummary>,
    rows: Vec<GraphRow>,
    graph: CommitGraph,
    /// The most lanes any loaded row uses.
    lanes: usize,
    loading: bool,
    complete: bool,
    error: Option<String>,
    /// Discards pages and reloads requested before the last reload.
    generation: u64,
    /// The repository changed while the list was hidden.
    stale: bool,
    scroll: gpui::UniformListScrollHandle,
    selected: Option<String>,
    shown: Option<Result<Arc<ShownCommit>, String>>,
    collapsed: HashSet<PathBuf>,
    list: Option<gpui::ListState>,
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
    pub(super) fn history_changed(&mut self, cx: &mut Context<Self>) {
        if self.commit_review.sidebar == Sidebar::History {
            self.reload_history(false, cx);
        } else {
            self.commit_review.history.stale = true;
        }
    }

    /// Reads the refs and, when they moved or `force` is set, the first page.
    fn reload_history(&mut self, force: bool, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let Some(directory) = core.project_directory.clone() else {
            return;
        };
        let backend = core.workspace_backend.clone();
        let history = &mut self.commit_review.history;
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
                        let page = repository.log(&tips, 0, PAGE).await?;
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
                        history.commits.clear();
                        history.rows.clear();
                        history.graph = CommitGraph::default();
                        history.lanes = 0;
                        history.complete = false;
                        history.add_page(page);
                    }
                    Err(error) => {
                        history.tips = Some(Vec::new());
                        history.commits.clear();
                        history.rows.clear();
                        history.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Loads the page after the commits already listed.
    fn load_history_page(&mut self, cx: &mut Context<Self>) {
        let history = &mut self.commit_review.history;
        let Some(tips) = history.tips.clone() else {
            return;
        };
        if history.loading || history.complete {
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
        let skip = history.commits.len();
        let handle = self.handle.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    handle.block_on(async move {
                        let repository = GitRepository::discover(backend, &directory)
                            .await?
                            .ok_or("This workspace is not a Git repository")?;
                        repository.log(&tips, skip, PAGE).await
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

    /// Shows commit `index` of the list in the Commit tab.
    fn select_history_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(commit) = self.commit_review.history.commits.get(index).cloned() else {
            return;
        };
        let history = &mut self.commit_review.history;
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
        self.activate_commit_tab(cx);
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
        let selected = history.in_tab && history.selected.as_ref() == Some(&commit.id);
        let colors = Self::lane_colors(cx);
        let lane_width = tokens.sizes.space_5;
        let graph_width = lane_width * history.lanes.clamp(1, MAX_LANES) as f32;
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
        let hover = if selected {
            file_tree.item_background_selected_hover
        } else {
            file_tree.item_background_hover
        };
        div()
            .id(("history-row", index))
            .when(cfg!(test), |view| {
                view.debug_selector(move || format!("history-row-{index}"))
            })
            .h(Self::history_row_height(cx))
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .px(tokens.sizes.space_2)
            .gap(tokens.sizes.space_2)
            .rounded(tokens.sizes.radius_md)
            .when(selected, |row| row.bg(file_tree.item_background_selected))
            .hover(move |row| row.bg(hover))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                this.select_history_commit(index, cx)
            }))
            .child(
                div()
                    .w(graph_width)
                    .h_full()
                    .flex_none()
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
                            .text_color(text)
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
                            .child(
                                div()
                                    .flex_none()
                                    .whitespace_nowrap()
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
            .into_any_element()
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
        let body = if count == 0 {
            let (title, loading) = match &history.error {
                Some(error) => (error.clone(), false),
                None if history.loading || history.tips.is_none() => {
                    ("Loading history…".to_string(), true)
                }
                None => ("No commits yet".to_string(), false),
            };
            StateView::new("history-sidebar-empty", title)
                .loading(loading)
                .compact(true)
                .into_any_element()
        } else {
            let now = now();
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
            .track_scroll(&history.scroll)
            .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::FitList)
            .w_full()
            .flex_1()
            .min_h_0()
            .px(tokens.sizes.space_1);
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(list)
                .when_some(history.error.clone(), |view, error| {
                    view.child(
                        div()
                            .p(tokens.sizes.space_2)
                            .text_size(tokens.sizes.text_xs)
                            .text_color(tokens.editor.error)
                            .child(error),
                    )
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
                    "No text changes (mode change or empty file)".into(),
                )),
                Ok(_) if unified => items.push(HistoryItem::Unified(index)),
                Ok(diff) => items.extend(
                    self.split_parts(&Self::history_split(index, file, diff))
                        .into_iter()
                        .map(|part| HistoryItem::Split(index, part)),
                ),
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
        let (letter, color) = match file.kind {
            FileChangeKind::Added => ("A", tokens.editor.vcs_added),
            FileChangeKind::Deleted => ("D", tokens.editor.vcs_deleted),
            FileChangeKind::Renamed => ("R", tokens.editor.vcs_modified),
            FileChangeKind::Copied => ("C", tokens.editor.vcs_added),
            FileChangeKind::TypeChanged => ("T", tokens.editor.vcs_modified),
            FileChangeKind::Modified | FileChangeKind::Other => ("M", tokens.editor.vcs_modified),
        };
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
                    .text_color(color)
                    .child(letter),
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
                    Ok(diff) => self.render_split_part(
                        Self::history_split(*index, file, diff),
                        *part,
                        style,
                        cx,
                    ),
                    Err(_) => div().into_any_element(),
                }
            }
            HistoryItem::Unified(index) => match &shown.files[*index].diff {
                Ok(diff) => self.render_unified_staged(*index, &diff.changes, style, cx),
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
        let history = &mut self.commit_review.history;
        let list = history
            .list
            .get_or_insert_with(|| gpui::ListState::new(0, gpui::ListAlignment::Top, px(1000.0)))
            .clone();
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
                .child(
                    div()
                        .text_size(tokens.sizes.text_lg)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(commit.subject.clone()),
                )
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
                            .id("history-commit-body")
                            .max_h(tokens.sizes.space_10 * 2.0)
                            .overflow_y_scroll()
                            .font_family(style.font_family.clone())
                            .text_color(style.text_secondary)
                            .whitespace_normal()
                            .child(shown.body.clone()),
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
                .when(!shown.files.is_empty(), |view| view.child(diff_list))
                .when(shown.files.is_empty(), |view| {
                    view.child(StateView::new("history-commit-empty", "No file changes"))
                }),
        )
        .into_any_element()
    }
}

impl History {
    fn add_page(&mut self, page: Vec<CommitSummary>) {
        self.complete = page.len() < PAGE;
        for commit in &page {
            let row = self.graph.add(commit);
            self.lanes = self.lanes.max(row.lanes);
            self.rows.push(row);
        }
        self.commits.extend(page);
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
