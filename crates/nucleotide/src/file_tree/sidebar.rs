// ABOUTME: Sidebar row model derived from file tree entries
// ABOUTME: Keeps project-tree rendering inputs separate from FileTreeView state

use std::{path::PathBuf, sync::Arc};

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, App, AppContext, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render,
    StatefulInteractiveElement, Styled, Window, anchored, div, point, px,
};
use nucleotide_types::VcsStatus;
use nucleotide_ui::VcsIcon;
use nucleotide_ui::{ListItem, ListItemSpacing, ListItemVariant, Theme};

use crate::file_tree::{
    FileKind, FileTreeDisplayDensity, FileTreeEntry, entry::FileTreeFlattenedSegment,
    icons::chevron_icon,
};

pub const PROJECT_TREE_ROW_HEIGHT_PX: f32 = 30.0;
pub const PROJECT_TREE_ROW_INDENT_PX: f32 = 16.0;
const PROJECT_TREE_ICON_SIZE_PX: f32 = 16.0;
const PROJECT_TREE_CHEVRON_SLOT_PX: f32 = 14.0;
const PROJECT_TREE_ROW_RADIUS_PX: f32 = 4.0;
const PROJECT_TREE_ROW_GAP_PX: f32 = 6.0;
const PROJECT_TREE_ROW_PADDING_RIGHT_PX: f32 = 8.0;
const PROJECT_TREE_GIT_STATUS_BADGE_PX: f32 = 22.0;
const PROJECT_TREE_GIT_STATUS_LANE_PX: f32 = PROJECT_TREE_GIT_STATUS_BADGE_PX;
const PROJECT_TREE_FILENAME_CHAR_WIDTH_PX: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct ProjectTreeDensityMetrics {
    row_height_px: f32,
    indent_px: f32,
    row_gap_px: f32,
    row_radius_px: f32,
    padding_right_px: f32,
}

impl ProjectTreeDensityMetrics {
    fn new(density: FileTreeDisplayDensity) -> Self {
        let spacing_factor = density.spacing_factor();
        Self {
            row_height_px: density.row_height_px(),
            indent_px: PROJECT_TREE_ROW_INDENT_PX * spacing_factor,
            row_gap_px: PROJECT_TREE_ROW_GAP_PX * spacing_factor,
            row_radius_px: PROJECT_TREE_ROW_RADIUS_PX * spacing_factor,
            padding_right_px: PROJECT_TREE_ROW_PADDING_RIGHT_PX * spacing_factor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectTreeRowAction {
    ToggleDirectory,
    OpenFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectTreeRowEvent {
    Activate {
        path: PathBuf,
        action: ProjectTreeRowAction,
    },
    ContextMenuRequested {
        path: PathBuf,
    },
    MoveRequested {
        from: PathBuf,
        target_dir: PathBuf,
    },
}

impl ProjectTreeRowEvent {
    pub fn context_menu_for_path(path: PathBuf) -> Self {
        Self::ContextMenuRequested { path }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectTreeContextMenuIntent {
    NewFile,
    NewFolder,
    Rename,
    Delete,
    Duplicate,
    CopyPath,
    CopyRelativePath,
    RevealInOs,
}

impl ProjectTreeContextMenuIntent {
    pub fn common_file_operations() -> &'static [Self] {
        &[
            Self::NewFile,
            Self::NewFolder,
            Self::Rename,
            Self::Delete,
            Self::Duplicate,
            Self::CopyPath,
            Self::CopyRelativePath,
            Self::RevealInOs,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NewFile => "New File",
            Self::NewFolder => "New Folder",
            Self::Rename => "Rename",
            Self::Delete => "Delete",
            Self::Duplicate => "Duplicate",
            Self::CopyPath => "Copy Path",
            Self::CopyRelativePath => "Copy Relative Path",
            Self::RevealInOs => "Reveal in OS",
        }
    }
}

pub struct ProjectTreeContextMenuState<'a> {
    pub theme: &'a Theme,
    pub position: (f32, f32),
    pub selected_index: usize,
    pub intents: &'a [ProjectTreeContextMenuIntent],
}

pub struct ProjectTreeContextMenuCallbacks<Hover, Activate, Backdrop> {
    pub on_item_hover: Hover,
    pub on_item_activate: Activate,
    pub on_backdrop_mouse_down: Backdrop,
}

pub fn render_project_tree_context_menu<T, Hover, Activate, Backdrop>(
    state: ProjectTreeContextMenuState<'_>,
    cx: &mut Context<T>,
    callbacks: ProjectTreeContextMenuCallbacks<Hover, Activate, Backdrop>,
) -> gpui::AnyElement
where
    T: 'static,
    Hover: Fn(&mut T, usize, &MouseMoveEvent, &mut Window, &mut Context<T>) + Copy + 'static,
    Activate: Fn(&mut T, ProjectTreeContextMenuIntent, &MouseDownEvent, &mut Window, &mut Context<T>)
        + Copy
        + 'static,
    Backdrop: Fn(&mut T, &MouseDownEvent, &mut Window, &mut Context<T>) + Copy + 'static,
{
    let ProjectTreeContextMenuCallbacks {
        on_item_hover,
        on_item_activate,
        on_backdrop_mouse_down,
    } = callbacks;
    let tokens = &state.theme.tokens;
    let dd_tokens = tokens.dropdown_tokens();
    let (x, y) = state.position;
    let item_count = state.intents.len();

    let popup = div()
        .bg(dd_tokens.container_background)
        .border_1()
        .border_color(dd_tokens.border)
        .rounded(tokens.sizes.radius_md)
        .shadow(vec![
            tokens.chrome.shadow_md.to_box_shadow(false),
            tokens.chrome.inset_highlight.to_box_shadow(true),
        ])
        .min_w(px(200.0))
        .py(tokens.sizes.space_1)
        .px(tokens.sizes.space_1)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .children(
            state
                .intents
                .iter()
                .copied()
                .enumerate()
                .map(|(index, intent)| {
                    let label = intent.label();
                    let text_default = dd_tokens.item_text;
                    let inner_radius = tokens.sizes.radius_md - px(0.5);
                    let is_selected = state.selected_index == index;
                    let is_first = index == 0;
                    let is_last = index + 1 == item_count;

                    div()
                        .w_full()
                        .on_mouse_move(cx.listener(move |state, event, window, cx| {
                            on_item_hover(state, index, event, window, cx);
                        }))
                        .when(is_selected, |div| {
                            div.bg(dd_tokens.item_background_selected)
                        })
                        .when(is_selected && is_first, |div| {
                            div.rounded_tl(inner_radius).rounded_tr(inner_radius)
                        })
                        .when(is_selected && is_last, |div| {
                            div.rounded_bl(inner_radius).rounded_br(inner_radius)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |state, event, window, cx| {
                                window.prevent_default();
                                on_item_activate(state, intent, event, window, cx);
                            }),
                        )
                        .child(
                            ListItem::new(("filetree-cm", index as u32))
                                .variant(ListItemVariant::Ghost)
                                .spacing(ListItemSpacing::Compact)
                                .child(
                                    div()
                                        .w_full()
                                        .text_size(tokens.sizes.text_sm)
                                        .px(tokens.sizes.space_2)
                                        .py(tokens.sizes.space_1)
                                        .text_color(if is_selected {
                                            dd_tokens.item_text_selected
                                        } else {
                                            text_default
                                        })
                                        .child(label),
                                ),
                        )
                }),
        );

    div()
        .absolute()
        .size_full()
        .top_0()
        .left_0()
        .occlude()
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Left, cx.listener(on_backdrop_mouse_down))
        .on_mouse_down(MouseButton::Right, cx.listener(on_backdrop_mouse_down))
        .child(
            anchored()
                .position(point(px(x), px(y)))
                .anchor(Anchor::TopLeft)
                .offset(point(px(8.0), px(8.0)))
                .snap_to_window_with_margin(tokens.sizes.space_2)
                .child(popup),
        )
        .into_any_element()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectTreeRowKind {
    File {
        extension: Option<String>,
    },
    Directory {
        is_loaded: bool,
        child_count: usize,
    },
    Symlink {
        target: Option<PathBuf>,
        target_exists: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTreeRow {
    pub id: u64,
    pub path: PathBuf,
    pub depth: usize,
    pub level: usize,
    pub pos_in_set: usize,
    pub set_size: usize,
    pub file_name: String,
    pub ancestor_paths: Arc<[PathBuf]>,
    pub flattened_segments: Option<Arc<[FileTreeFlattenedSegment]>>,
    pub kind: ProjectTreeRowKind,
    pub is_expanded: bool,
    pub is_selected: bool,
    pub is_hidden: bool,
    pub is_search_match: bool,
    pub vcs_status: Option<VcsStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTreeDraggedEntry {
    pub path: PathBuf,
    pub file_name: String,
    pub is_directory: bool,
}

struct ProjectTreeDragPreview {
    entry: ProjectTreeDraggedEntry,
    position: Point<Pixels>,
}

impl ProjectTreeDragPreview {
    fn new(entry: ProjectTreeDraggedEntry, position: Point<Pixels>) -> Self {
        Self { entry, position }
    }
}

impl Render for ProjectTreeDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().pl(self.position.x).pt(self.position.y).child(
            div()
                .h(px(PROJECT_TREE_ROW_HEIGHT_PX))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(PROJECT_TREE_ROW_RADIUS_PX))
                .bg(gpui::black().opacity(0.72))
                .text_color(gpui::white())
                .text_size(px(13.0))
                .child(self.entry.file_name.clone()),
        )
    }
}

impl ProjectTreeRow {
    pub fn from_entry(
        entry: &FileTreeEntry,
        is_selected: bool,
        vcs_status: Option<VcsStatus>,
    ) -> Self {
        Self {
            id: entry.id.0,
            path: entry.path.clone(),
            depth: entry.depth,
            level: entry.level,
            pos_in_set: entry.pos_in_set,
            set_size: entry.set_size,
            file_name: display_name(entry),
            ancestor_paths: entry.ancestor_paths.clone(),
            flattened_segments: entry.flattened_segments.clone(),
            kind: ProjectTreeRowKind::from(&entry.kind),
            is_expanded: entry.is_expanded,
            is_selected,
            is_hidden: entry.is_hidden,
            is_search_match: entry.is_search_match,
            vcs_status: vcs_status.or(entry.git_status),
        }
    }

    pub fn primary_action(&self) -> ProjectTreeRowAction {
        if self.is_directory() {
            ProjectTreeRowAction::ToggleDirectory
        } else {
            ProjectTreeRowAction::OpenFile
        }
    }

    pub fn primary_click_event(&self) -> ProjectTreeRowEvent {
        ProjectTreeRowEvent::Activate {
            path: self.path.clone(),
            action: self.primary_action(),
        }
    }

    pub fn context_menu_event(&self) -> ProjectTreeRowEvent {
        ProjectTreeRowEvent::ContextMenuRequested {
            path: self.path.clone(),
        }
    }

    pub fn click_event(&self, secondary: bool) -> ProjectTreeRowEvent {
        if secondary {
            self.context_menu_event()
        } else {
            self.primary_click_event()
        }
    }

    pub fn is_directory(&self) -> bool {
        matches!(self.kind, ProjectTreeRowKind::Directory { .. })
    }

    pub fn is_root(&self) -> bool {
        self.depth == 0 && self.is_directory()
    }

    pub fn can_be_dragged(&self) -> bool {
        !self.is_root()
    }

    pub fn dragged_entry(&self) -> ProjectTreeDraggedEntry {
        ProjectTreeDraggedEntry {
            path: self.path.clone(),
            file_name: self.file_name.clone(),
            is_directory: self.is_directory(),
        }
    }

    pub fn can_accept_drop(&self, dragged: &ProjectTreeDraggedEntry) -> bool {
        self.is_directory()
            && self.path != dragged.path
            && !self.path.starts_with(&dragged.path)
            && dragged.path.parent() != Some(self.path.as_path())
    }
}

impl From<&FileKind> for ProjectTreeRowKind {
    fn from(kind: &FileKind) -> Self {
        match kind {
            FileKind::File { extension } => Self::File {
                extension: extension.clone(),
            },
            FileKind::Directory {
                is_loaded,
                child_count,
            } => Self::Directory {
                is_loaded: *is_loaded,
                child_count: *child_count,
            },
            FileKind::Symlink {
                target,
                target_exists,
            } => Self::Symlink {
                target: target.clone(),
                target_exists: *target_exists,
            },
        }
    }
}

pub fn render_project_tree_row(
    row: ProjectTreeRow,
    theme: &Theme,
    density: FileTreeDisplayDensity,
    on_left_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_right_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_drop: impl Fn(&ProjectTreeDraggedEntry, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let file_tree_tokens = theme.tokens.file_tree_tokens();
    let row_foreground = if row.is_selected {
        theme.tokens.editor.text_on_primary
    } else {
        file_tree_tokens.item_text
    };
    let metrics = ProjectTreeDensityMetrics::new(density);
    let indentation = px(row.depth as f32 * metrics.indent_px);
    let min_row_width = project_tree_row_min_width(&row, density);
    let drop_target_row = row.clone();
    let drop_style_row = row.clone();
    let drop_event_row = row.clone();
    let drag_payload = row.dragged_entry();
    let can_be_dragged = row.can_be_dragged();
    let drop_background = file_tree_tokens.item_background_hover;

    div()
        .id(("file-tree-entry", row.id))
        .w_full()
        .min_w(px(min_row_width))
        .h(px(metrics.row_height_px))
        .px(px(0.0))
        .py(px(0.0))
        .rounded(px(metrics.row_radius_px))
        .can_drop(move |dragged, _, _| {
            dragged
                .downcast_ref::<ProjectTreeDraggedEntry>()
                .is_some_and(|dragged| drop_target_row.can_accept_drop(dragged))
        })
        .drag_over::<ProjectTreeDraggedEntry>(move |mut style, dragged, _, _| {
            if drop_style_row.can_accept_drop(dragged) {
                style.background = Some(drop_background.into());
            }
            style
        })
        .on_drop(move |dragged: &ProjectTreeDraggedEntry, window, cx| {
            if drop_event_row.can_accept_drop(dragged) {
                on_drop(dragged, window, cx);
            }
        })
        .when(can_be_dragged, |row| {
            row.cursor_move()
                .on_drag(drag_payload, |dragged, position, _, cx| {
                    cx.new(|_| ProjectTreeDragPreview::new(dragged.clone(), position))
                })
        })
        .on_mouse_down(MouseButton::Left, on_left_mouse_down)
        .on_mouse_down(MouseButton::Right, on_right_mouse_down)
        .on_click(on_click)
        .child(
            div()
                .w_full()
                .min_w(px(min_row_width))
                .h_full()
                .flex()
                .items_center()
                .gap(px(metrics.row_gap_px))
                .pl(indentation)
                .pr(px(metrics.padding_right_px))
                .text_color(row_foreground)
                .when(row.is_selected, |row| {
                    row.bg(file_tree_tokens.item_background_selected)
                })
                .when(!row.is_selected, |row| {
                    row.hover(move |row| {
                        row.bg(file_tree_tokens.item_background_hover)
                            .text_color(file_tree_tokens.item_text)
                    })
                })
                .child(render_chevron_slot(&row, theme))
                .child(render_icon(&row, theme))
                .child(render_filename(&row, theme))
                .child(render_git_status_lane(&row, theme)),
        )
        .into_any_element()
}

fn render_chevron_slot(row: &ProjectTreeRow, theme: &Theme) -> gpui::AnyElement {
    div()
        .w(px(PROJECT_TREE_CHEVRON_SLOT_PX))
        .h(px(PROJECT_TREE_CHEVRON_SLOT_PX))
        .flex()
        .items_center()
        .justify_center()
        .when(row.is_directory(), |div| {
            div.child(render_chevron(row, theme))
        })
        .into_any_element()
}

fn render_chevron(row: &ProjectTreeRow, theme: &Theme) -> impl IntoElement {
    let file_tree_tokens = theme.tokens.file_tree_tokens();
    let chevron_color = if row.is_selected {
        theme.tokens.editor.text_on_primary
    } else {
        file_tree_tokens.item_text_secondary
    };

    chevron_icon(if row.is_expanded { "down" } else { "right" })
        .size_3()
        .text_color(chevron_color)
}

fn render_icon(row: &ProjectTreeRow, theme: &Theme) -> impl IntoElement {
    let file_tree_tokens = theme.tokens.file_tree_tokens();
    let icon_color = if row.is_selected {
        theme.tokens.editor.text_on_primary
    } else {
        file_tree_tokens.item_text
    };

    let vcs_icon = match &row.kind {
        ProjectTreeRowKind::Directory { .. } => VcsIcon::directory(row.is_expanded)
            .size(PROJECT_TREE_ICON_SIZE_PX)
            .text_color(icon_color),
        ProjectTreeRowKind::File { extension } => VcsIcon::from_extension(extension.as_deref())
            .size(PROJECT_TREE_ICON_SIZE_PX)
            .text_color(icon_color),
        ProjectTreeRowKind::Symlink { target_exists, .. } => VcsIcon::symlink(*target_exists)
            .size(PROJECT_TREE_ICON_SIZE_PX)
            .text_color(if *target_exists {
                icon_color
            } else {
                theme.tokens.editor.error
            }),
    };

    vcs_icon.render_with_theme(theme)
}

fn render_filename(row: &ProjectTreeRow, theme: &Theme) -> impl IntoElement {
    let file_tree_tokens = theme.tokens.file_tree_tokens();
    let is_root_directory = row.depth == 0 && row.is_directory();
    let display_status = git_status_for_display(row);
    let text_color = filename_color(row, theme);

    let mut node = div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .text_size(theme.tokens.sizes.text_md)
        .child(row.file_name.clone());

    if display_status.is_some() {
        node = node.text_color(text_color);
        if is_root_directory || row.is_search_match {
            node = node.font_weight(gpui::FontWeight::MEDIUM);
        }
    } else if row.is_selected {
        node = node.text_color(text_color);
    } else if is_root_directory || row.is_search_match {
        node = node
            .text_color(text_color)
            .font_weight(gpui::FontWeight::MEDIUM);
    } else if row.is_hidden {
        node = node
            .text_color(text_color)
            .hover(move |node| node.text_color(file_tree_tokens.item_text));
    } else {
        node = node.text_color(text_color);
    }

    node
}

fn render_git_status_lane(row: &ProjectTreeRow, theme: &Theme) -> impl IntoElement {
    let Some(status) = git_status_for_display(row) else {
        return div()
            .ml_auto()
            .w(px(PROJECT_TREE_GIT_STATUS_LANE_PX))
            .flex_shrink_0()
            .into_any_element();
    };

    div()
        .ml_auto()
        .w(px(PROJECT_TREE_GIT_STATUS_LANE_PX))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .child(
            div()
                .size(px(PROJECT_TREE_GIT_STATUS_BADGE_PX))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.tokens.sizes.radius_md)
                .border_1()
                .border_color(git_status_badge_border_color(theme))
                .text_size(theme.tokens.sizes.text_xs)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(git_status_color(status, theme))
                .child(git_status_label(status)),
        )
        .into_any_element()
}

fn git_status_for_display(row: &ProjectTreeRow) -> Option<VcsStatus> {
    row.vcs_status
        .filter(|status| should_render_git_status(*status))
}

fn filename_color(row: &ProjectTreeRow, theme: &Theme) -> gpui::Hsla {
    if let Some(status) = git_status_for_display(row) {
        return git_status_color(status, theme);
    }

    if row.is_selected {
        theme.tokens.editor.text_on_primary
    } else if row.is_hidden {
        theme.tokens.file_tree_tokens().item_text_secondary
    } else {
        theme.tokens.file_tree_tokens().item_text
    }
}

fn should_render_git_status(status: VcsStatus) -> bool {
    !matches!(status, VcsStatus::Clean)
}

fn git_status_label(status: VcsStatus) -> &'static str {
    match status {
        VcsStatus::Untracked => "?",
        VcsStatus::Clean => "",
        VcsStatus::Modified => "M",
        VcsStatus::Added => "A",
        VcsStatus::Deleted => "D",
        VcsStatus::Renamed => "R",
        VcsStatus::Conflicted => "C",
        VcsStatus::Unknown => "!",
    }
}

fn git_status_color(status: VcsStatus, theme: &Theme) -> gpui::Hsla {
    match status {
        VcsStatus::Modified => theme.tokens.editor.vcs_modified,
        VcsStatus::Added => theme.tokens.editor.vcs_added,
        VcsStatus::Deleted => theme.tokens.editor.vcs_deleted,
        VcsStatus::Untracked | VcsStatus::Unknown => theme.tokens.chrome.text_chrome_secondary,
        VcsStatus::Renamed => theme.tokens.chrome.primary,
        VcsStatus::Conflicted => theme.tokens.editor.error,
        VcsStatus::Clean => theme.tokens.chrome.text_chrome_secondary,
    }
}

fn git_status_badge_border_color(theme: &Theme) -> gpui::Hsla {
    theme.tokens.chrome.border_default
}

fn project_tree_row_min_width(row: &ProjectTreeRow, density: FileTreeDisplayDensity) -> f32 {
    project_tree_row_min_width_for(row.depth, row.file_name.chars().count(), density)
}

pub(crate) fn project_tree_entry_min_width(
    entry: &FileTreeEntry,
    density: FileTreeDisplayDensity,
) -> f32 {
    project_tree_row_min_width_for(entry.depth, display_name(entry).chars().count(), density)
}

fn project_tree_row_min_width_for(
    depth: usize,
    filename_char_count: usize,
    density: FileTreeDisplayDensity,
) -> f32 {
    let metrics = ProjectTreeDensityMetrics::new(density);
    let indentation = depth as f32 * metrics.indent_px;
    let fixed_width = PROJECT_TREE_CHEVRON_SLOT_PX
        + PROJECT_TREE_ICON_SIZE_PX
        + metrics.padding_right_px
        + metrics.row_gap_px * 2.0;
    let git_status_lane_width = PROJECT_TREE_GIT_STATUS_LANE_PX + metrics.row_gap_px;
    let filename_width = filename_char_count as f32 * PROJECT_TREE_FILENAME_CHAR_WIDTH_PX;

    indentation + fixed_width + git_status_lane_width + filename_width
}

fn display_name(entry: &FileTreeEntry) -> String {
    if let Some(segments) = &entry.flattened_segments {
        return segments
            .iter()
            .map(|segment| segment.name.as_str())
            .collect::<Vec<_>>()
            .join("/");
    }

    if entry.depth == 0 && entry.is_directory() {
        return entry
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .or_else(|| {
                entry
                    .path
                    .components()
                    .next_back()
                    .and_then(|component| component.as_os_str().to_str())
            })
            .unwrap_or(".")
            .to_string();
    }

    entry.file_name().unwrap_or("?").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_tree::entry::FileTreeEntryId;

    #[test]
    fn row_from_entry_uses_root_directory_name() {
        let entry = FileTreeEntry::new_directory(
            FileTreeEntryId(1),
            PathBuf::from("/workspace/nucleotide"),
            None,
        );

        let row = ProjectTreeRow::from_entry(&entry, false, None);

        assert_eq!(row.file_name, "nucleotide");
        assert_eq!(row.depth, 0);
        assert!(row.is_directory());
    }

    #[test]
    fn row_from_entry_uses_dot_for_empty_root_path() {
        let entry = FileTreeEntry::new_directory(FileTreeEntryId(1), PathBuf::new(), None);

        let row = ProjectTreeRow::from_entry(&entry, false, None);

        assert_eq!(row.file_name, ".");
    }

    #[test]
    fn row_from_entry_preserves_depth_selection_kind_and_vcs_status() {
        let mut entry = FileTreeEntry::new_file(
            FileTreeEntryId(2),
            PathBuf::from("/workspace/src/main.rs"),
            42,
            None,
        );
        entry.depth = 2;
        entry.level = 3;
        entry.pos_in_set = 2;
        entry.set_size = 5;
        entry.ancestor_paths =
            Arc::from([PathBuf::from("/workspace"), PathBuf::from("/workspace/src")]);
        entry.is_hidden = true;
        entry.git_status = Some(VcsStatus::Clean);

        let row = ProjectTreeRow::from_entry(&entry, true, Some(VcsStatus::Modified));

        assert_eq!(row.id, 2);
        assert_eq!(row.file_name, "main.rs");
        assert_eq!(row.depth, 2);
        assert_eq!(row.level, 3);
        assert_eq!(row.pos_in_set, 2);
        assert_eq!(row.set_size, 5);
        assert_eq!(
            row.ancestor_paths.as_ref(),
            [PathBuf::from("/workspace"), PathBuf::from("/workspace/src")]
        );
        assert!(row.is_selected);
        assert!(row.is_hidden);
        assert_eq!(row.vcs_status, Some(VcsStatus::Modified));
        assert_eq!(
            row.kind,
            ProjectTreeRowKind::File {
                extension: Some("rs".to_string())
            }
        );
    }

    #[test]
    fn row_min_width_grows_with_depth_and_filename() {
        let mut shallow = ProjectTreeRow::from_entry(
            &FileTreeEntry::new_file(
                FileTreeEntryId(2),
                PathBuf::from("/workspace/a.rs"),
                42,
                None,
            ),
            false,
            None,
        );
        shallow.depth = 0;
        let mut deep = shallow.clone();
        deep.depth = 3;
        deep.file_name = "very_long_nested_file_name.rs".to_string();

        assert!(
            project_tree_row_min_width(&deep, FileTreeDisplayDensity::Default)
                > project_tree_row_min_width(&shallow, FileTreeDisplayDensity::Default)
        );
    }

    #[test]
    fn row_min_width_reserves_right_aligned_git_status_lane() {
        let width = project_tree_row_min_width_for(0, 0, FileTreeDisplayDensity::Default);
        let expected = PROJECT_TREE_CHEVRON_SLOT_PX
            + PROJECT_TREE_ICON_SIZE_PX
            + PROJECT_TREE_ROW_PADDING_RIGHT_PX
            + PROJECT_TREE_GIT_STATUS_LANE_PX
            + PROJECT_TREE_ROW_GAP_PX * 3.0;

        assert_eq!(width, expected);
    }

    #[test]
    fn density_metrics_match_tree_density_presets() {
        let compact = ProjectTreeDensityMetrics::new(FileTreeDisplayDensity::Compact);
        let default = ProjectTreeDensityMetrics::new(FileTreeDisplayDensity::Default);
        let relaxed = ProjectTreeDensityMetrics::new(FileTreeDisplayDensity::Relaxed);

        assert_eq!(compact.row_height_px, 24.0);
        assert_eq!(default.row_height_px, PROJECT_TREE_ROW_HEIGHT_PX);
        assert_eq!(relaxed.row_height_px, 36.0);
        assert_eq!(compact.row_gap_px, PROJECT_TREE_ROW_GAP_PX * 0.8);
        assert_eq!(default.row_gap_px, PROJECT_TREE_ROW_GAP_PX);
        assert_eq!(relaxed.row_gap_px, PROJECT_TREE_ROW_GAP_PX * 1.2);
    }

    #[test]
    fn git_status_labels_use_compact_tree_style_text() {
        assert!(!should_render_git_status(VcsStatus::Clean));
        assert!(should_render_git_status(VcsStatus::Modified));
        assert_eq!(git_status_label(VcsStatus::Modified), "M");
        assert_eq!(git_status_label(VcsStatus::Added), "A");
        assert_eq!(git_status_label(VcsStatus::Deleted), "D");
        assert_eq!(git_status_label(VcsStatus::Renamed), "R");
        assert_eq!(git_status_label(VcsStatus::Untracked), "?");
        assert_eq!(git_status_label(VcsStatus::Conflicted), "C");
        assert_eq!(git_status_label(VcsStatus::Unknown), "!");
    }

    #[test]
    fn git_status_colors_use_vcs_design_tokens() {
        let theme = Theme::from_tokens(nucleotide_ui::DesignTokens::dark());

        assert_eq!(
            git_status_color(VcsStatus::Modified, &theme),
            theme.tokens.editor.vcs_modified
        );
        assert_eq!(
            git_status_color(VcsStatus::Added, &theme),
            theme.tokens.editor.vcs_added
        );
        assert_eq!(
            git_status_color(VcsStatus::Deleted, &theme),
            theme.tokens.editor.vcs_deleted
        );
        assert_eq!(
            git_status_color(VcsStatus::Renamed, &theme),
            theme.tokens.chrome.primary
        );
        assert_eq!(
            git_status_color(VcsStatus::Conflicted, &theme),
            theme.tokens.editor.error
        );
        assert_eq!(
            git_status_color(VcsStatus::Untracked, &theme),
            theme.tokens.chrome.text_chrome_secondary
        );
    }

    #[test]
    fn filename_color_uses_displayable_git_status_color() {
        let theme = Theme::from_tokens(nucleotide_ui::DesignTokens::dark());
        let mut entry = FileTreeEntry::new_file(
            FileTreeEntryId(3),
            PathBuf::from("/workspace/main.rs"),
            1,
            None,
        );
        entry.git_status = Some(VcsStatus::Modified);

        let row = ProjectTreeRow::from_entry(&entry, true, None);

        assert_eq!(
            filename_color(&row, &theme),
            theme.tokens.editor.vcs_modified
        );
    }

    #[test]
    fn git_status_badge_border_uses_theme_token() {
        let theme = Theme::from_tokens(nucleotide_ui::DesignTokens::dark());

        assert_eq!(
            git_status_badge_border_color(&theme),
            theme.tokens.chrome.border_default
        );
    }

    #[test]
    fn row_from_entry_falls_back_to_entry_vcs_status() {
        let mut entry = FileTreeEntry::new_file(
            FileTreeEntryId(3),
            PathBuf::from("/workspace/a.txt"),
            1,
            None,
        );
        entry.git_status = Some(VcsStatus::Added);

        let row = ProjectTreeRow::from_entry(&entry, false, None);

        assert_eq!(row.vcs_status, Some(VcsStatus::Added));
    }

    #[test]
    fn row_primary_action_toggles_directories() {
        let entry =
            FileTreeEntry::new_directory(FileTreeEntryId(4), PathBuf::from("/workspace/src"), None);
        let row = ProjectTreeRow::from_entry(&entry, false, None);

        assert_eq!(row.primary_action(), ProjectTreeRowAction::ToggleDirectory);
    }

    #[test]
    fn row_primary_action_opens_files_and_symlinks() {
        let file = FileTreeEntry::new_file(
            FileTreeEntryId(5),
            PathBuf::from("/workspace/main.rs"),
            12,
            None,
        );
        let symlink = FileTreeEntry::new_symlink(
            FileTreeEntryId(6),
            PathBuf::from("/workspace/current"),
            Some(PathBuf::from("/workspace/releases/current")),
            true,
            None,
        );

        let file_row = ProjectTreeRow::from_entry(&file, false, None);
        let symlink_row = ProjectTreeRow::from_entry(&symlink, false, None);

        assert_eq!(file_row.primary_action(), ProjectTreeRowAction::OpenFile);
        assert_eq!(symlink_row.primary_action(), ProjectTreeRowAction::OpenFile);
    }

    #[test]
    fn row_primary_click_event_activates_primary_action() {
        let file = FileTreeEntry::new_file(
            FileTreeEntryId(7),
            PathBuf::from("/workspace/main.rs"),
            12,
            None,
        );
        let row = ProjectTreeRow::from_entry(&file, false, None);

        assert_eq!(
            row.primary_click_event(),
            ProjectTreeRowEvent::Activate {
                path: PathBuf::from("/workspace/main.rs"),
                action: ProjectTreeRowAction::OpenFile,
            }
        );
    }

    #[test]
    fn row_secondary_click_event_requests_context_menu() {
        let entry =
            FileTreeEntry::new_directory(FileTreeEntryId(8), PathBuf::from("/workspace/src"), None);
        let row = ProjectTreeRow::from_entry(&entry, false, None);

        assert_eq!(
            row.click_event(true),
            ProjectTreeRowEvent::ContextMenuRequested {
                path: PathBuf::from("/workspace/src"),
            }
        );
    }

    #[test]
    fn context_menu_event_can_target_project_root() {
        assert_eq!(
            ProjectTreeRowEvent::context_menu_for_path(PathBuf::from("/workspace")),
            ProjectTreeRowEvent::ContextMenuRequested {
                path: PathBuf::from("/workspace"),
            }
        );
    }

    #[test]
    fn row_drag_payload_preserves_move_identity() {
        let mut entry =
            FileTreeEntry::new_directory(FileTreeEntryId(9), PathBuf::from("/workspace/src"), None);
        entry.depth = 1;
        let row = ProjectTreeRow::from_entry(&entry, false, None);

        assert!(row.can_be_dragged());
        assert_eq!(
            row.dragged_entry(),
            ProjectTreeDraggedEntry {
                path: PathBuf::from("/workspace/src"),
                file_name: "src".to_string(),
                is_directory: true,
            }
        );
    }

    #[test]
    fn root_row_cannot_be_dragged_but_accepts_nested_drops() {
        let mut root = FileTreeEntry::new_directory(
            FileTreeEntryId(0),
            PathBuf::from("/workspace/project"),
            None,
        );
        root.depth = 0;
        let root_row = ProjectTreeRow::from_entry(&root, false, None);
        let dragged = ProjectTreeDraggedEntry {
            path: PathBuf::from("/workspace/project/crates/src"),
            file_name: "src".to_string(),
            is_directory: true,
        };

        assert!(!root_row.can_be_dragged());
        assert!(root_row.can_accept_drop(&dragged));
    }

    #[test]
    fn row_rejects_invalid_drop_targets() {
        let target = ProjectTreeRow::from_entry(
            &FileTreeEntry::new_directory(
                FileTreeEntryId(10),
                PathBuf::from("/workspace/crates"),
                None,
            ),
            false,
            None,
        );
        let file_target = ProjectTreeRow::from_entry(
            &FileTreeEntry::new_file(
                FileTreeEntryId(11),
                PathBuf::from("/workspace/main.rs"),
                1,
                None,
            ),
            false,
            None,
        );

        assert!(!target.can_accept_drop(&ProjectTreeDraggedEntry {
            path: PathBuf::from("/workspace/crates"),
            file_name: "crates".to_string(),
            is_directory: true,
        }));
        assert!(!target.can_accept_drop(&ProjectTreeDraggedEntry {
            path: PathBuf::from("/workspace"),
            file_name: "workspace".to_string(),
            is_directory: true,
        }));
        assert!(!target.can_accept_drop(&ProjectTreeDraggedEntry {
            path: PathBuf::from("/workspace/crates/lib.rs"),
            file_name: "lib.rs".to_string(),
            is_directory: false,
        }));
        assert!(!file_target.can_accept_drop(&ProjectTreeDraggedEntry {
            path: PathBuf::from("/workspace/src"),
            file_name: "src".to_string(),
            is_directory: true,
        }));
    }

    #[test]
    fn context_menu_intents_cover_common_file_operations() {
        let labels: Vec<_> = ProjectTreeContextMenuIntent::common_file_operations()
            .iter()
            .map(|intent| intent.label())
            .collect();

        assert_eq!(
            labels,
            vec![
                "New File",
                "New Folder",
                "Rename",
                "Delete",
                "Duplicate",
                "Copy Path",
                "Copy Relative Path",
                "Reveal in OS"
            ]
        );
    }

    #[test]
    fn row_layout_uses_stable_zed_like_dimensions() {
        assert_eq!(PROJECT_TREE_ROW_HEIGHT_PX, 30.0);
        assert_eq!(PROJECT_TREE_ROW_INDENT_PX, 16.0);
    }
}
