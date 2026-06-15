// ABOUTME: Sidebar row model derived from file tree entries
// ABOUTME: Keeps project-tree rendering inputs separate from FileTreeView state

use std::path::PathBuf;

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, App, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, StatefulInteractiveElement, Styled, Window,
    anchored, div, point, px,
};
use nucleotide_types::VcsStatus;
use nucleotide_ui::VcsIcon;
use nucleotide_ui::{ListItem, ListItemSpacing, ListItemVariant, Theme};

use crate::file_tree::{FileKind, FileTreeEntry, icons::chevron_icon};

pub const PROJECT_TREE_ROW_HEIGHT_PX: f32 = 30.0;
pub const PROJECT_TREE_ROW_INDENT_PX: f32 = 16.0;
const PROJECT_TREE_ICON_SIZE_PX: f32 = 16.0;
const PROJECT_TREE_CHEVRON_SLOT_PX: f32 = 14.0;
const PROJECT_TREE_ROW_RADIUS_PX: f32 = 4.0;
const PROJECT_TREE_ROW_GAP_PX: f32 = 6.0;
const PROJECT_TREE_ROW_PADDING_RIGHT_PX: f32 = 8.0;

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
    Activate: Fn(&mut T, ProjectTreeContextMenuIntent, &MouseUpEvent, &mut Window, &mut Context<T>)
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
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |state, event, window, cx| {
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
    pub file_name: String,
    pub kind: ProjectTreeRowKind,
    pub is_expanded: bool,
    pub is_selected: bool,
    pub is_hidden: bool,
    pub vcs_status: Option<VcsStatus>,
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
            file_name: display_name(entry),
            kind: ProjectTreeRowKind::from(&entry.kind),
            is_expanded: entry.is_expanded,
            is_selected,
            is_hidden: entry.is_hidden,
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
    on_left_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_right_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let file_tree_tokens = theme.tokens.file_tree_tokens();
    let row_foreground = if row.is_selected {
        theme.tokens.editor.text_on_primary
    } else {
        file_tree_tokens.item_text
    };
    let indentation = px(row.depth as f32 * PROJECT_TREE_ROW_INDENT_PX);

    div()
        .id(("file-tree-entry", row.id))
        .w_full()
        .h(px(PROJECT_TREE_ROW_HEIGHT_PX))
        .px(px(0.0))
        .py(px(0.0))
        .rounded(px(PROJECT_TREE_ROW_RADIUS_PX))
        .on_mouse_down(MouseButton::Left, on_left_mouse_down)
        .on_mouse_down(MouseButton::Right, on_right_mouse_down)
        .on_click(on_click)
        .child(
            div()
                .w_full()
                .h_full()
                .flex()
                .items_center()
                .gap(px(PROJECT_TREE_ROW_GAP_PX))
                .pl(indentation)
                .pr(px(PROJECT_TREE_ROW_PADDING_RIGHT_PX))
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
                .child(render_icon_with_vcs_status(&row, theme))
                .child(render_filename(&row, theme)),
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

fn render_icon_with_vcs_status(row: &ProjectTreeRow, theme: &Theme) -> impl IntoElement {
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

    vcs_icon.vcs_status(row.vcs_status).render_with_theme(theme)
}

fn render_filename(row: &ProjectTreeRow, theme: &Theme) -> impl IntoElement {
    let file_tree_tokens = theme.tokens.file_tree_tokens();
    let is_root_directory = row.depth == 0 && row.is_directory();

    let mut node = div()
        .flex_1()
        .min_w(px(0.0))
        .overflow_hidden()
        .text_size(theme.tokens.sizes.text_md)
        .child(row.file_name.clone());

    if row.is_selected {
        node = node.text_color(theme.tokens.editor.text_on_primary);
    } else if is_root_directory {
        node = node
            .text_color(file_tree_tokens.item_text)
            .font_weight(gpui::FontWeight::MEDIUM);
    } else if row.is_hidden {
        node = node
            .text_color(file_tree_tokens.item_text_secondary)
            .hover(move |node| node.text_color(file_tree_tokens.item_text));
    } else {
        node = node.text_color(file_tree_tokens.item_text);
    }

    node
}

fn display_name(entry: &FileTreeEntry) -> String {
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
        entry.is_hidden = true;
        entry.git_status = Some(VcsStatus::Clean);

        let row = ProjectTreeRow::from_entry(&entry, true, Some(VcsStatus::Modified));

        assert_eq!(row.id, 2);
        assert_eq!(row.file_name, "main.rs");
        assert_eq!(row.depth, 2);
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
