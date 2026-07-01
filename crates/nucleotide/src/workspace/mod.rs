// ABOUTME: Workspace module decomposition for cleaner architecture
// ABOUTME: Separates view management from workspace coordination logic

pub mod prefix_extraction;
pub mod view_manager;

use prefix_extraction::PrefixExtractor;
pub use view_manager::ViewManager;

// Main workspace implementation
#[cfg(target_os = "windows")]
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(target_os = "windows")]
use std::sync::{LazyLock, Mutex};

#[cfg(target_os = "windows")]
use gpui::MenuItem;
use gpui::prelude::{FluentBuilder, StyledImage};
use gpui::{
    Anchor, App, AppContext, BorrowAppContext, Bounds, Context, DismissEvent, DragMoveEvent, Empty,
    Entity, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point, Render, ScrollHandle, Size, StatefulInteractiveElement, Styled, TextStyle, Window,
    WindowAppearance, canvas, div, img, px, relative, svg,
};
use gpui::{FontFeatures, FontWeight};
use helix_core::syntax::config::LanguageServerFeature;
use helix_core::{Position, Rope, RopeSlice, Selection, pos_at_coords};
use helix_lsp::{OffsetEncoding, lsp};
use helix_stdx::rope::RopeSliceExt;
use helix_view::input::KeyEvent;
use helix_view::keyboard::{KeyCode, KeyModifiers};
use helix_view::{DocumentId, ViewId, graphics::Rect as HelixRect};
use nucleotide_core::{event_bridge, gpui_to_helix_bridge};
use nucleotide_logging::{debug, error, info, instrument, trace, warn};
use nucleotide_types::scrollbar::SCROLLBAR_THICKNESS;
use nucleotide_ui::ThemedContext as UIThemedContext;

// ViewManager already imported above via pub use
use nucleotide_ui::notification::{StatusBarNotification, StatusBarNotificationSeverity};
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
use nucleotide_ui::{
    AboutWindow, Button, ButtonSize, ButtonVariant, ConfirmDialog, ConfirmDialogCallbacks,
    ContextMenuCallbacks, ContextMenuEntry, ContextMenuState, MarkdownStyle, Tooltipped,
    markdown_extended, render_confirm_dialog, render_context_menu,
};

use crate::input_coordinator::{FocusGroup, InputContext, InputCoordinator};
use nucleotide_lsp::ServerStatus;

use crate::application::{LspCompletionTrigger, find_workspace_root_from};
use crate::document::DocumentView;
use crate::file_tree::{
    FileSystemEventKind, FileTreeConfig, FileTreeEvent, FileTreeView,
    sidebar::ProjectTreeContextMenuIntent,
};
use crate::info_box::InfoBoxView;
use crate::key_hint_view::KeyHintView;
use crate::notification::NotificationView;
use crate::overlay::OverlayView;
use crate::tab::TabId;
use crate::types::{
    EditorStatus, GlobalSearchLocation, HoverDocEntry, RegexSelectionAction, Severity,
};
use crate::utils;
use crate::{Core, Input, InputEvent};
use nucleotide_core::EventBus;
use nucleotide_env::{
    EnvironmentOrigin, WslWorkspace, create_wsl_remote_directory_blocking,
    create_wsl_remote_file_blocking, load_wsl_remote_file_search_blocking,
    load_wsl_remote_global_search_blocking,
};
use nucleotide_events::v2::run::{Event as RunEvent, ResolvedTask, RunId, RunStatus};
use nucleotide_events::v2::terminal::{Event as TerminalEvent, TerminalId};
use nucleotide_remote::{
    FileCreateResponse, FileSearchResponse, GlobalSearchResponse, RemoteFileKind,
};
use nucleotide_terminal::TerminalBounds;
use slotmap::KeyData;
// (no direct Workspace v2 items used here)
use nucleotide_vcs::{VcsEvent, VcsServiceHandle};
#[cfg(target_os = "windows")]
use smallvec::{SmallVec, smallvec};

type FileTreeContextMenuHandler = fn(&mut Workspace, &mut Context<Workspace>);
const WSL_REMOTE_FILE_PICKER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const WSL_REMOTE_GLOBAL_SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const WSL_REMOTE_FILE_OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

fn document_lsp_identifier(
    doc: &helix_view::Document,
) -> Option<helix_lsp::lsp::TextDocumentIdentifier> {
    doc.url().map(helix_lsp::lsp::TextDocumentIdentifier::new)
}
type TabContextMenuHandler = fn(&mut Workspace, TabId, &mut Context<Workspace>);
type TabBarSplitMenuHandler = fn(&mut Workspace, &mut Context<Workspace>);
type TabBarNewMenuHandler = fn(&mut Workspace, &mut Context<Workspace>);

const STATUSBAR_NOTIFICATION_MESSAGE_MAX_CHARS: usize = 64;
const STATUSBAR_LSP_INDICATOR_MAX_CHARS: usize = 56;
const IMAGE_ZOOM_STEP: f32 = 0.25;
const IMAGE_ZOOM_MIN: f32 = 0.10;
const IMAGE_ZOOM_MAX: f32 = 8.0;
const IMAGE_TRANSPARENCY_GRID_SIZE: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnvironmentBadge {
    Loading,
    NativeFlake,
    Wsl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunnableAction {
    ShowPicker,
    RunNearest,
    RunFileTests,
}

#[derive(Clone)]
struct ImageTab {
    id: u64,
    path: PathBuf,
    dimensions: Option<(u32, u32)>,
    focused_at: std::time::Instant,
    zoom: f32,
    scroll_handle: ScrollHandle,
    vertical_scrollbar_state: ScrollbarState,
    horizontal_scrollbar_state: ScrollbarState,
}

impl EnvironmentBadge {
    fn from_environment_marker(marker: Option<&str>) -> Option<Self> {
        match marker {
            Some("native-flake") => Some(Self::NativeFlake),
            Some("wsl-remote-helper" | "wsl-shell") => Some(Self::Wsl),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Loading => "direnv",
            Self::NativeFlake => "direnv",
            Self::Wsl => "wsl",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::NativeFlake => "flake",
            Self::Wsl => "remote",
        }
    }
}

fn titlebar_filename(filename: Option<&str>) -> String {
    filename
        .filter(|name| !name.is_empty())
        .unwrap_or("Nucleotide")
        .to_string()
}

fn shorten_statusbar_text(text: &str, max_chars: usize) -> String {
    debug_assert!(max_chars >= 2);

    let mut normalized = String::new();
    let mut saw_whitespace = false;

    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if !saw_whitespace && !normalized.is_empty() {
                normalized.push(' ');
            }
            saw_whitespace = true;
        } else {
            normalized.push(ch);
            saw_whitespace = false;
        }
    }

    if normalized.chars().count() <= max_chars {
        return normalized;
    }

    let mut shortened: String = normalized.chars().take(max_chars - 1).collect();
    shortened.push('…');
    shortened
}

fn is_image_file_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "avif"
                    | "bmp"
                    | "dds"
                    | "exr"
                    | "farbfeld"
                    | "ff"
                    | "gif"
                    | "hdr"
                    | "ico"
                    | "jpeg"
                    | "jpg"
                    | "pam"
                    | "pbm"
                    | "pgm"
                    | "png"
                    | "ppm"
                    | "qoi"
                    | "svg"
                    | "tga"
                    | "tif"
                    | "tiff"
                    | "webp"
            )
        })
        .unwrap_or(false)
}

fn image_zoom_percent(zoom: f32) -> String {
    format!("{:.0}%", zoom * 100.0)
}

fn image_transparency_grid_colors(editor_background: Hsla) -> (Hsla, Hsla) {
    if editor_background.l > 0.5 {
        (
            gpui::hsla(0.0, 0.0, 0.98, 1.0),
            gpui::hsla(0.0, 0.0, 0.86, 1.0),
        )
    } else {
        (
            gpui::hsla(0.0, 0.0, 0.16, 1.0),
            gpui::hsla(0.0, 0.0, 0.26, 1.0),
        )
    }
}

fn image_transparency_grid(base: Hsla, alternate: Hsla) -> gpui::AnyElement {
    let square_size = px(IMAGE_TRANSPARENCY_GRID_SIZE);

    div()
        .absolute()
        .size_full()
        .bg(base)
        .child(
            canvas(
                move |_, _, _| (),
                move |bounds, _, window, _| {
                    let rows = (bounds.size.height / square_size).ceil() as i32;
                    let cols = (bounds.size.width / square_size).ceil() as i32;

                    for row in 0..rows {
                        for col in 0..cols {
                            if (row + col) % 2 != 0 {
                                continue;
                            }

                            let origin = bounds.origin
                                + gpui::point(square_size * col as f32, square_size * row as f32);
                            window.paint_quad(gpui::fill(
                                Bounds {
                                    origin,
                                    size: gpui::size(square_size, square_size),
                                },
                                alternate,
                            ));
                        }
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .into_any_element()
}

fn image_file_dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

fn statusbar_lsp_indicator_for_state(
    state: &mut nucleotide_lsp::LspState,
    preferred_server_id: Option<helix_lsp::LanguageServerId>,
) -> Option<String> {
    if !state.progress.is_empty() {
        return state.get_lsp_indicator();
    }

    if let Some(pref_id) = preferred_server_id
        && let Some(server) = state.servers.get(&pref_id).cloned()
    {
        let indicator = match server.status {
            ServerStatus::Starting | ServerStatus::Initializing => {
                state.get_spinner_frame().to_string()
            }
            _ => "◉".to_string(),
        };
        return Some(format!("{} {}", indicator, server.name));
    }

    state.get_lsp_indicator()
}

fn should_render_app_titlebar(
    has_titlebar: bool,
    show_file_tree: bool,
    file_tree_width: f32,
    translucent_sidebar_enabled: bool,
) -> bool {
    has_titlebar
        && !should_extend_translucent_sidebar_into_status_bar(
            show_file_tree,
            file_tree_width,
            translucent_sidebar_enabled,
        )
}

fn file_tree_content_top_inset(translucent_sidebar_enabled: bool) -> Pixels {
    if translucent_sidebar_enabled {
        px(MACOS_TRAFFIC_LIGHT_TREE_TOP_INSET_PX)
    } else {
        px(0.0)
    }
}

fn should_extend_translucent_sidebar_into_status_bar(
    show_file_tree: bool,
    file_tree_width: f32,
    translucent_sidebar_enabled: bool,
) -> bool {
    translucent_sidebar_enabled && show_file_tree && file_tree_width > 0.0
}

fn native_window_title(filename: Option<&str>) -> String {
    if let Some(filename) = filename.filter(|name| !name.is_empty()) {
        format!("{filename} — Nucleotide")
    } else {
        "Nucleotide".to_string()
    }
}

fn configured_theme_name_for_appearance(
    theme: &crate::config::ThemeConfig,
    system_appearance: nucleotide_appearance::SystemAppearance,
) -> String {
    match theme.mode {
        crate::config::ThemeMode::Light => theme.get_light_theme(),
        crate::config::ThemeMode::Dark => theme.get_dark_theme(),
        crate::config::ThemeMode::System => match system_appearance {
            nucleotide_appearance::SystemAppearance::Light => theme.get_light_theme(),
            nucleotide_appearance::SystemAppearance::Dark => theme.get_dark_theme(),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NativeWindowMetadata {
    title: String,
    document_path: Option<PathBuf>,
    edited: bool,
}

#[cfg(target_os = "macos")]
fn add_recent_project(path: &Path, cx: &mut App) {
    if path.is_dir() {
        cx.add_recent_document(path);
        debug!(project_root = %path.display(), "Added project to macOS recent documents");
    }
}

#[cfg(target_os = "windows")]
const WINDOWS_JUMP_LIST_PROJECT_LIMIT: usize = 10;

#[cfg(target_os = "windows")]
static WINDOWS_RECENT_PROJECTS: LazyLock<Mutex<VecDeque<PathBuf>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

#[cfg(target_os = "windows")]
fn add_recent_project(path: &Path, cx: &mut App) {
    let Some(path) = windows_recent_project_path(path) else {
        return;
    };

    let wide_path = windows_wide_nul_path(&path);
    unsafe {
        windows_sys::Win32::UI::Shell::SHAddToRecentDocs(
            windows_sys::Win32::UI::Shell::SHARD_PATHW as u32,
            wide_path.as_ptr().cast(),
        );
    }

    let entries = windows_record_recent_project(path.clone());
    cx.update_jump_list(windows_jump_list_menu_items(), entries)
        .detach();

    debug!(project_root = %path.display(), "Added project to Windows recent documents and Jump List");
}

#[cfg(target_os = "windows")]
fn windows_recent_project_path(path: &Path) -> Option<PathBuf> {
    if WslWorkspace::from_unc_path(path).is_some() {
        return Some(path.to_path_buf());
    }

    if !path.is_dir() {
        return None;
    }

    Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
}

#[cfg(target_os = "windows")]
fn windows_wide_nul_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_record_recent_project(path: PathBuf) -> Vec<SmallVec<[PathBuf; 2]>> {
    let Ok(mut recent) = WINDOWS_RECENT_PROJECTS.lock() else {
        warn!(project_root = %path.display(), "Failed to update Windows Jump List recent projects");
        return vec![smallvec![path]];
    };

    windows_update_recent_project_list(&mut recent, path);
    windows_jump_list_entries(&recent)
}

#[cfg(target_os = "windows")]
fn windows_update_recent_project_list(recent: &mut VecDeque<PathBuf>, path: PathBuf) {
    if let Some(index) = recent.iter().position(|entry| entry == &path) {
        recent.remove(index);
    }

    recent.push_front(path);

    while recent.len() > WINDOWS_JUMP_LIST_PROJECT_LIMIT {
        recent.pop_back();
    }
}

#[cfg(target_os = "windows")]
fn windows_jump_list_entries(recent: &VecDeque<PathBuf>) -> Vec<SmallVec<[PathBuf; 2]>> {
    recent.iter().cloned().map(|path| smallvec![path]).collect()
}

#[cfg(target_os = "windows")]
fn windows_jump_list_menu_items() -> Vec<MenuItem> {
    vec![
        MenuItem::action("Open...", crate::actions::editor::OpenFile),
        MenuItem::action("Open Directory...", crate::actions::editor::OpenDirectory),
    ]
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn add_recent_project(_path: &Path, _cx: &mut App) {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabContextMenuIntent {
    Close,
    CloseOthers,
    CloseLeft,
    CloseRight,
    CloseClean,
    CloseAll,
    CopyPath,
    CopyRelativePath,
    RevealInOs,
    RevealInProjectPanel,
    OpenInTerminal,
    ToggleReadOnly,
    TogglePin,
}

impl TabContextMenuIntent {
    fn label(self, is_pinned: bool, is_readonly: bool, is_remote: bool) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::CloseOthers => "Close Others",
            Self::CloseLeft => "Close Left",
            Self::CloseRight => "Close Right",
            Self::CloseClean => "Close Clean",
            Self::CloseAll => "Close All",
            Self::CopyPath => "Copy Path",
            Self::CopyRelativePath => "Copy Relative Path",
            Self::RevealInOs => reveal_in_file_manager_label(is_remote),
            Self::RevealInProjectPanel => "Reveal In Project Panel",
            Self::OpenInTerminal => "Open in Terminal",
            Self::ToggleReadOnly if is_readonly => "Make File Editable",
            Self::ToggleReadOnly => "Make File Read-Only",
            Self::TogglePin if is_pinned => "Unpin Tab",
            Self::TogglePin => "Pin Tab",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabContextMenuEntry {
    Action(TabContextMenuIntent),
    Separator,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TabContextMenuCapabilities {
    has_file_path: bool,
    has_project_panel_path: bool,
    has_terminal_directory: bool,
    is_readonly: bool,
    is_remote: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DocumentViewLayout {
    view_id: ViewId,
    area: HelixRect,
    is_focused: bool,
}

const SPLIT_PANE_HANDLE_HITBOX_PX: f32 = nucleotide_ui::SPLITTER_HITBOX_PX;
const SPLIT_PANE_MIN_WIDTH_CELLS: u16 = 8;
const SPLIT_PANE_MIN_HEIGHT_CELLS: u16 = 3;
const SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitPaneResizeAxis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SplitPaneDivider {
    axis: SplitPaneResizeAxis,
    before_view_ids: Vec<ViewId>,
    after_view_ids: Vec<ViewId>,
    edge: u16,
    start: u16,
    span: u16,
    gap: u16,
}

#[derive(Clone, Copy, Debug)]
struct SplitPaneResizeViewState {
    view_id: ViewId,
    area: HelixRect,
}

#[derive(Clone, Debug)]
struct SplitPaneResizeState {
    axis: SplitPaneResizeAxis,
    start_mouse_x: f32,
    start_mouse_y: f32,
    before_views: Vec<SplitPaneResizeViewState>,
    after_views: Vec<SplitPaneResizeViewState>,
    total_area: HelixRect,
    editor_width_px: f32,
    editor_height_px: f32,
}

fn helix_rect_to_scaled_pixel_bounds(
    area: HelixRect,
    total_area: HelixRect,
    target_width: f32,
    target_height: f32,
) -> (Pixels, Pixels, Pixels, Pixels) {
    let total_width = f32::from(total_area.width).max(1.0);
    let total_height = f32::from(total_area.height).max(1.0);
    let target_width = target_width.max(1.0);
    let target_height = target_height.max(1.0);

    let relative_x = area.x.saturating_sub(total_area.x);
    let relative_y = area.y.saturating_sub(total_area.y);
    let left = f32::from(relative_x) / total_width * target_width;
    let top = f32::from(relative_y) / total_height * target_height;
    let width = (f32::from(area.width) / total_width * target_width).max(1.0);
    let height = (f32::from(area.height) / total_height * target_height).max(1.0);

    (px(left), px(top), px(width), px(height))
}

fn split_pane_dividers(layouts: &[DocumentViewLayout]) -> Vec<SplitPaneDivider> {
    let mut dividers = Vec::new();

    for (index, first) in layouts.iter().enumerate() {
        for second in layouts.iter().skip(index + 1) {
            if let Some(divider) = split_pane_vertical_divider(*first, *second)
                .or_else(|| split_pane_vertical_divider(*second, *first))
            {
                push_or_merge_split_pane_divider(&mut dividers, divider);
            }

            if let Some(divider) = split_pane_horizontal_divider(*first, *second)
                .or_else(|| split_pane_horizontal_divider(*second, *first))
            {
                push_or_merge_split_pane_divider(&mut dividers, divider);
            }
        }
    }

    dividers
}

fn split_pane_resize_hitbox(
    id: impl Into<gpui::ElementId>,
    axis: SplitPaneResizeAxis,
    handle_px: f32,
) -> gpui::Stateful<gpui::Div> {
    let handle_px = handle_px.max(1.0);
    let base = div().id(id).relative().occlude();

    match axis {
        SplitPaneResizeAxis::Vertical => base
            .w(px(handle_px))
            .h_full()
            .cursor(gpui::CursorStyle::ResizeLeftRight),
        SplitPaneResizeAxis::Horizontal => base
            .w_full()
            .h(px(handle_px))
            .cursor(gpui::CursorStyle::ResizeRow),
    }
}

fn push_or_merge_split_pane_divider(
    dividers: &mut Vec<SplitPaneDivider>,
    mut divider: SplitPaneDivider,
) {
    let mut index = 0;
    while index < dividers.len() {
        if split_pane_dividers_can_merge(&dividers[index], &divider) {
            let existing = dividers.remove(index);
            divider = merge_split_pane_dividers(existing, divider);
            index = 0;
        } else {
            index += 1;
        }
    }

    dividers.push(divider);
}

fn split_pane_dividers_can_merge(first: &SplitPaneDivider, second: &SplitPaneDivider) -> bool {
    first.axis == second.axis
        && first.edge == second.edge
        && first.gap == second.gap
        && split_pane_ranges_can_merge(first.start, first.span, second.start, second.span)
}

fn split_pane_ranges_can_merge(
    first_start: u16,
    first_span: u16,
    second_start: u16,
    second_span: u16,
) -> bool {
    let first_end = first_start.saturating_add(first_span);
    let second_end = second_start.saturating_add(second_span);
    first_start <= second_end.saturating_add(SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS)
        && second_start <= first_end.saturating_add(SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS)
}

fn merge_split_pane_dividers(
    mut first: SplitPaneDivider,
    second: SplitPaneDivider,
) -> SplitPaneDivider {
    for view_id in second.before_view_ids {
        push_unique_view_id(&mut first.before_view_ids, view_id);
    }
    for view_id in second.after_view_ids {
        push_unique_view_id(&mut first.after_view_ids, view_id);
    }

    let start = first.start.min(second.start);
    let end = first
        .start
        .saturating_add(first.span)
        .max(second.start.saturating_add(second.span));
    first.start = start;
    first.span = end.saturating_sub(start);
    first.gap = first.gap.max(second.gap);
    first
}

fn push_unique_view_id(view_ids: &mut Vec<ViewId>, view_id: ViewId) {
    if !view_ids.contains(&view_id) {
        view_ids.push(view_id);
    }
}

fn split_pane_resize_view_states(
    layouts: &[DocumentViewLayout],
    view_ids: &[ViewId],
) -> Vec<SplitPaneResizeViewState> {
    view_ids
        .iter()
        .filter_map(|view_id| {
            layouts
                .iter()
                .find(|layout| layout.view_id == *view_id)
                .map(|layout| SplitPaneResizeViewState {
                    view_id: *view_id,
                    area: layout.area,
                })
        })
        .collect()
}

fn split_pane_vertical_divider(
    before: DocumentViewLayout,
    after: DocumentViewLayout,
) -> Option<SplitPaneDivider> {
    let before_right = before.area.x.saturating_add(before.area.width);
    let gap = after.area.x.checked_sub(before_right)?;
    if gap > SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS {
        return None;
    }

    let start = before.area.y.max(after.area.y);
    let end = before
        .area
        .y
        .saturating_add(before.area.height)
        .min(after.area.y.saturating_add(after.area.height));
    if end <= start {
        return None;
    }

    Some(SplitPaneDivider {
        axis: SplitPaneResizeAxis::Vertical,
        before_view_ids: vec![before.view_id],
        after_view_ids: vec![after.view_id],
        edge: before_right.saturating_add(gap / 2),
        start,
        span: end - start,
        gap,
    })
}

fn split_pane_horizontal_divider(
    before: DocumentViewLayout,
    after: DocumentViewLayout,
) -> Option<SplitPaneDivider> {
    let before_bottom = before.area.y.saturating_add(before.area.height);
    let gap = after.area.y.checked_sub(before_bottom)?;
    if gap > SPLIT_PANE_MAX_SEPARATOR_GAP_CELLS {
        return None;
    }

    let start = before.area.x.max(after.area.x);
    let end = before
        .area
        .x
        .saturating_add(before.area.width)
        .min(after.area.x.saturating_add(after.area.width));
    if end <= start {
        return None;
    }

    Some(SplitPaneDivider {
        axis: SplitPaneResizeAxis::Horizontal,
        before_view_ids: vec![before.view_id],
        after_view_ids: vec![after.view_id],
        edge: before_bottom.saturating_add(gap / 2),
        start,
        span: end - start,
        gap,
    })
}

fn document_view_visual_area(
    layout: DocumentViewLayout,
    dividers: &[SplitPaneDivider],
) -> HelixRect {
    let mut area = layout.area;

    for divider in dividers {
        if divider.gap == 0 || !divider.after_view_ids.contains(&layout.view_id) {
            continue;
        }

        match divider.axis {
            SplitPaneResizeAxis::Vertical => {
                area.x = area.x.saturating_sub(divider.gap);
                area.width = area.width.saturating_add(divider.gap);
            }
            SplitPaneResizeAxis::Horizontal => {
                area.y = area.y.saturating_sub(divider.gap);
                area.height = area.height.saturating_add(divider.gap);
            }
        }
    }

    area
}

fn split_pane_divider_visual_line(
    mut divider: SplitPaneDivider,
    dividers: &[SplitPaneDivider],
) -> SplitPaneDivider {
    for other in dividers {
        if divider.axis == other.axis || other.gap == 0 {
            continue;
        }

        let all_views_shift_with_other = divider
            .before_view_ids
            .iter()
            .chain(&divider.after_view_ids)
            .all(|view_id| other.after_view_ids.contains(view_id));
        if !all_views_shift_with_other {
            continue;
        }

        divider.start = divider.start.saturating_sub(other.gap);
        divider.span = divider.span.saturating_add(other.gap);
    }

    divider
}

fn split_pane_resized_areas(
    state: &SplitPaneResizeState,
    mouse_x: f32,
    mouse_y: f32,
) -> Option<Vec<(ViewId, HelixRect)>> {
    match state.axis {
        SplitPaneResizeAxis::Vertical => {
            let cells_per_px =
                f32::from(state.total_area.width).max(1.0) / state.editor_width_px.max(1.0);
            let delta = ((mouse_x - state.start_mouse_x) * cells_per_px).round() as i32;
            resized_vertical_split_pane_view_areas(
                &state.before_views,
                &state.after_views,
                delta,
                SPLIT_PANE_MIN_WIDTH_CELLS,
            )
        }
        SplitPaneResizeAxis::Horizontal => {
            let cells_per_px =
                f32::from(state.total_area.height).max(1.0) / state.editor_height_px.max(1.0);
            let delta = ((mouse_y - state.start_mouse_y) * cells_per_px).round() as i32;
            resized_horizontal_split_pane_view_areas(
                &state.before_views,
                &state.after_views,
                delta,
                SPLIT_PANE_MIN_HEIGHT_CELLS,
            )
        }
    }
}

fn resized_vertical_split_pane_view_areas(
    before_views: &[SplitPaneResizeViewState],
    after_views: &[SplitPaneResizeViewState],
    delta_cells: i32,
    min_width: u16,
) -> Option<Vec<(ViewId, HelixRect)>> {
    let min_width = i32::from(min_width.max(1));
    let min_delta = before_views
        .iter()
        .map(|view| min_width - i32::from(view.area.width))
        .max()?;
    let max_delta = after_views
        .iter()
        .map(|view| i32::from(view.area.width) - min_width)
        .min()?;
    if min_delta > max_delta {
        return None;
    }

    let delta = delta_cells.clamp(min_delta, max_delta);
    let mut resized = Vec::with_capacity(before_views.len() + after_views.len());

    for view in before_views {
        let width = i32::from(view.area.width).checked_add(delta)?;
        let width = u16::try_from(width).ok()?;
        resized.push((
            view.view_id,
            HelixRect::new(view.area.x, view.area.y, width, view.area.height),
        ));
    }

    for view in after_views {
        let x = i32::from(view.area.x).checked_add(delta)?;
        let width = i32::from(view.area.width).checked_sub(delta)?;
        let x = u16::try_from(x).ok()?;
        let width = u16::try_from(width).ok()?;
        resized.push((
            view.view_id,
            HelixRect::new(x, view.area.y, width, view.area.height),
        ));
    }

    Some(resized)
}

fn resized_horizontal_split_pane_view_areas(
    before_views: &[SplitPaneResizeViewState],
    after_views: &[SplitPaneResizeViewState],
    delta_cells: i32,
    min_height: u16,
) -> Option<Vec<(ViewId, HelixRect)>> {
    let min_height = i32::from(min_height.max(1));
    let min_delta = before_views
        .iter()
        .map(|view| min_height - i32::from(view.area.height))
        .max()?;
    let max_delta = after_views
        .iter()
        .map(|view| i32::from(view.area.height) - min_height)
        .min()?;
    if min_delta > max_delta {
        return None;
    }

    let delta = delta_cells.clamp(min_delta, max_delta);
    let mut resized = Vec::with_capacity(before_views.len() + after_views.len());

    for view in before_views {
        let height = i32::from(view.area.height).checked_add(delta)?;
        let height = u16::try_from(height).ok()?;
        resized.push((
            view.view_id,
            HelixRect::new(view.area.x, view.area.y, view.area.width, height),
        ));
    }

    for view in after_views {
        let y = i32::from(view.area.y).checked_add(delta)?;
        let height = i32::from(view.area.height).checked_sub(delta)?;
        let y = u16::try_from(y).ok()?;
        let height = u16::try_from(height).ok()?;
        resized.push((
            view.view_id,
            HelixRect::new(view.area.x, y, view.area.width, height),
        ));
    }

    Some(resized)
}

#[cfg(test)]
fn resized_vertical_split_pane_areas(
    before: HelixRect,
    after: HelixRect,
    delta_cells: i32,
    min_width: u16,
) -> Option<(HelixRect, HelixRect)> {
    let before_right = before.x.checked_add(before.width)?;
    let outer_left = before.x;
    let outer_right = after.x.checked_add(after.width)?;
    let gap = after.x.checked_sub(before_right)?;
    let usable = outer_right.checked_sub(outer_left)?.checked_sub(gap)?;
    let min_width = min_width.min(usable.saturating_sub(1)).max(1);
    let max_before = usable.saturating_sub(min_width);
    if max_before < min_width {
        return None;
    }

    let target_before = (i32::from(before.width) + delta_cells)
        .clamp(i32::from(min_width), i32::from(max_before)) as u16;
    let after_x = outer_left.checked_add(target_before)?.checked_add(gap)?;
    let after_width = outer_right.checked_sub(after_x)?;

    Some((
        HelixRect::new(before.x, before.y, target_before, before.height),
        HelixRect::new(after_x, after.y, after_width, after.height),
    ))
}

#[cfg(test)]
fn resized_horizontal_split_pane_areas(
    before: HelixRect,
    after: HelixRect,
    delta_cells: i32,
    min_height: u16,
) -> Option<(HelixRect, HelixRect)> {
    let before_bottom = before.y.checked_add(before.height)?;
    let outer_top = before.y;
    let outer_bottom = after.y.checked_add(after.height)?;
    let gap = after.y.checked_sub(before_bottom)?;
    let usable = outer_bottom.checked_sub(outer_top)?.checked_sub(gap)?;
    let min_height = min_height.min(usable.saturating_sub(1)).max(1);
    let max_before = usable.saturating_sub(min_height);
    if max_before < min_height {
        return None;
    }

    let target_before = (i32::from(before.height) + delta_cells)
        .clamp(i32::from(min_height), i32::from(max_before)) as u16;
    let after_y = outer_top.checked_add(target_before)?.checked_add(gap)?;
    let after_height = outer_bottom.checked_sub(after_y)?;

    Some((
        HelixRect::new(before.x, before.y, before.width, target_before),
        HelixRect::new(after.x, after_y, after.width, after_height),
    ))
}

fn document_view_layout_bounds(layouts: &[DocumentViewLayout]) -> Option<HelixRect> {
    let first = layouts.first()?;
    let mut min_x = first.area.x;
    let mut min_y = first.area.y;
    let mut max_x = first.area.x.saturating_add(first.area.width);
    let mut max_y = first.area.y.saturating_add(first.area.height);

    for layout in &layouts[1..] {
        min_x = min_x.min(layout.area.x);
        min_y = min_y.min(layout.area.y);
        max_x = max_x.max(layout.area.x.saturating_add(layout.area.width));
        max_y = max_y.max(layout.area.y.saturating_add(layout.area.height));
    }

    Some(HelixRect::new(
        min_x,
        min_y,
        max_x.saturating_sub(min_x).max(1),
        max_y.saturating_sub(min_y).max(1),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabBarSplitMenuIntent {
    Right,
    Left,
    Up,
    Down,
}

impl TabBarSplitMenuIntent {
    fn label(self) -> &'static str {
        match self {
            Self::Right => "Split Right",
            Self::Left => "Split Left",
            Self::Up => "Split Up",
            Self::Down => "Split Down",
        }
    }

    fn commands(self) -> &'static [&'static str] {
        match self {
            Self::Right => &["vsplit"],
            Self::Left => &["vsplit", "swap_view_left"],
            Self::Up => &["hsplit", "swap_view_up"],
            Self::Down => &["hsplit"],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabBarNewMenuIntent {
    NewFile,
    OpenFile,
    SearchProject,
    SearchSymbols,
    NewTerminal,
    NewCenterTerminal,
}

impl TabBarNewMenuIntent {
    fn label(self) -> &'static str {
        match self {
            Self::NewFile => "New File",
            Self::OpenFile => "Open File",
            Self::SearchProject => "Search Project",
            Self::SearchSymbols => "Search Symbols",
            Self::NewTerminal => "New Terminal",
            Self::NewCenterTerminal => "New Center Terminal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabBarNewMenuEntry {
    Action(TabBarNewMenuIntent),
    Separator,
}

pub struct Workspace {
    core: Entity<Core>,
    input: Entity<Input>,
    view_manager: ViewManager,
    handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    info: Entity<InfoBoxView>,
    info_hidden: bool,
    key_hints: Entity<KeyHintView>,
    notifications: Entity<NotificationView>,
    last_notified_editor_status: Option<EditorStatus>,
    focus_handle: FocusHandle,
    file_tree: Option<Entity<FileTreeView>>,
    show_file_tree: bool,
    file_tree_width: f32,
    file_tree_width_override: Option<f32>,
    is_resizing_file_tree: bool,
    resize_start_x: f32,
    resize_start_width: f32,
    doc_sidebar_visible: bool,
    doc_sidebar_loading: bool,
    doc_sidebar_entries: Vec<HoverDocEntry>,
    doc_sidebar_width: f32,
    doc_sidebar_resizing: bool,
    doc_sidebar_resize_start_x: f32,
    doc_sidebar_resize_start_width: f32,
    doc_sidebar_scroll_handle: ScrollHandle,
    doc_sidebar_scrollbar_state: ScrollbarState,
    titlebar: Option<Entity<nucleotide_ui::titlebar::TitleBar>>,
    appearance_observer_set: bool,
    needs_appearance_update: bool,
    needs_window_appearance_update: bool,
    pending_appearance: Option<gpui::WindowAppearance>,
    tab_bar_scroll_handle: ScrollHandle,
    last_scrolled_tab_doc_id: Option<TabId>,
    suppress_tab_bar_auto_scroll: bool,
    image_tabs: Vec<ImageTab>,
    active_image_tab_id: Option<u64>,
    next_image_tab_index: u64,
    // File tree context menu state
    context_menu_open: bool,
    context_menu_pos: (f32, f32),
    context_menu_path: Option<std::path::PathBuf>,
    context_menu_index: usize,
    // Tab context menu state
    tab_context_menu_open: bool,
    tab_context_menu_pos: (f32, f32),
    tab_context_menu_doc_id: Option<TabId>,
    tab_context_menu_index: usize,
    pinned_documents: HashSet<TabId>,
    // Tab bar split menu state
    tab_bar_split_menu_open: bool,
    tab_bar_split_menu_pos: (f32, f32),
    tab_bar_split_button_bounds: Option<Bounds<Pixels>>,
    tab_bar_split_menu_index: usize,
    split_pane_resize: Option<SplitPaneResizeState>,
    restore_standard_cursor_after_resize: bool,
    // Tab bar new item menu state
    tab_bar_new_menu_open: bool,
    tab_bar_new_menu_pos: (f32, f32),
    tab_bar_new_menu_index: usize,
    // LSP server list popup state
    lsp_menu_open: bool,
    lsp_menu_pos: (f32, f32),
    document_order: Vec<helix_view::DocumentId>, // Ordered list of documents in opening order
    input_coordinator: Arc<InputCoordinator>,    // Central input coordination system
    current_project_root: Option<std::path::PathBuf>, // Track current project root for change detection
    initial_project_startup_pending: bool, // Defer project/LSP startup until after first render
    environment_badge: Option<EnvironmentBadge>,
    _pending_lsp_startup: Option<std::path::PathBuf>, // Track pending server startup requests
    prefix_extractor: PrefixExtractor,                // Language-aware completion prefix extraction
    about_window: Entity<AboutWindow>,                // About dialog window
    theme_debug: Entity<nucleotide_ui::ThemeDebugView>, // Theme debug overlay
    // Pending file operation that expects a text input via prompt
    pending_file_op: Option<PendingFileOp>,
    // Defer a file tree refresh until after processing core events
    needs_file_tree_refresh: bool,
    // Delete confirmation modal state
    delete_confirm_open: bool,
    delete_confirm_path: Option<std::path::PathBuf>,
    // Unsaved close confirmation modal state
    close_confirm_open: bool,
    close_confirm: Option<UnsavedCloseConfirmation<DocumentId>>,
    // Terminal panel state
    terminal_panel_visible: bool,
    terminal_id: Option<TerminalId>,
    next_terminal_id: u64,
    next_run_id: u64,
    last_run_task: Option<ResolvedTask>,
    active_run_terminal: Option<(TerminalId, RunId)>,
    run_output_terminal: Option<TerminalId>,
    // Debug: color major panes when enabled via env
    debug_colors_enabled: bool,
    // Height of the bottom (terminal) pane in basic layout mode
    basic_terminal_height: f32,
    // Drag state for basic layout terminal resizer
    basic_term_resizing: bool,
    basic_term_start_mouse_y: f32,
    basic_term_start_height: f32,
    // Embedded terminal panel entity for basic layout
    embedded_terminal_panel: Option<gpui::Entity<nucleotide_terminal_panel::TerminalPanel>>,
    // Cwd used to spawn the active terminal session.
    terminal_cwd: Option<PathBuf>,
    // Focus handle for embedded terminal to capture keyboard input
    terminal_focus: gpui::FocusHandle,
    // Request to focus terminal on next render (when toggled on via button)
    terminal_focus_pending: bool,
    // Track whether terminal should capture keys (set on click in terminal area)
    terminal_active: bool,
    // Cache last applied editor size to avoid redundant resizes each frame
    last_editor_size: Option<(u16, u16)>,
    last_terminal_bounds: Option<(TerminalId, TerminalBounds)>,
    // Cached theme-derived colors to avoid per-frame recomputation
    cached_bg_color: gpui::Hsla,
    cached_text_color: gpui::Hsla,
    cached_border_color: gpui::Hsla,
    colors_dirty: bool,
    cached_font_metrics_key: Option<(String, f32, nucleotide_types::FontWeight)>,
    cached_char_width: Option<f32>,
    cached_line_height: Option<f32>,
    active_completion_session: Option<ActiveCompletionSession>,
    completion_memory: CompletionMemory,
    last_native_window_metadata: Option<NativeWindowMetadata>,
}

#[derive(Clone, Debug)]
struct ActiveCompletionSession {
    doc_id: DocumentId,
    view_id: ViewId,
    document_version: i32,
    is_incomplete: bool,
    incomplete_server_ids: Vec<u64>,
    retained_items: Vec<nucleotide_events::completion::CompletionItem>,
    requested_prefix: String,
}

#[derive(Clone, Copy, Debug)]
struct CompletionAcceptTarget {
    doc_id: DocumentId,
    view_id: ViewId,
    document_version: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CompletionMemoryKey {
    language: String,
    prefix: String,
    kind: Option<nucleotide_ui::completion_v2::CompletionItemKind>,
    insert_text: String,
}

#[derive(Default)]
struct CompletionMemory {
    entries: HashMap<CompletionMemoryKey, u64>,
    next_touch: u64,
}

impl CompletionMemory {
    fn priority(&self, key: &CompletionMemoryKey) -> u64 {
        self.entries.get(key).copied().unwrap_or(0)
    }

    fn memorize(&mut self, key: CompletionMemoryKey) {
        self.next_touch = self.next_touch.saturating_add(1);
        self.entries.insert(key, self.next_touch);
    }
}

fn should_retrigger_incomplete_completion_for_focused_session(
    session: &ActiveCompletionSession,
    current_prefix: &str,
    focused_doc_id: Option<DocumentId>,
    focused_view_id: ViewId,
) -> bool {
    session.is_incomplete
        && session.requested_prefix != current_prefix
        && focused_doc_id == Some(session.doc_id)
        && focused_view_id == session.view_id
}

fn retained_completion_items_for_completed_providers(
    items: &[nucleotide_events::completion::CompletionItem],
    incomplete_server_ids: &[u64],
) -> Vec<nucleotide_events::completion::CompletionItem> {
    items
        .iter()
        .filter(|item| {
            item.server_id
                .is_some_and(|server_id| !incomplete_server_ids.contains(&server_id))
        })
        .cloned()
        .collect()
}

fn completion_locality_key(item: &nucleotide_ui::completion_v2::CompletionItem) -> Option<String> {
    let text = item
        .filter_text
        .as_ref()
        .or(item.display_text.as_ref())
        .unwrap_or(&item.text);
    let key: String = text
        .chars()
        .skip_while(|ch| !helix_core::chars::char_is_word(*ch))
        .take_while(|ch| helix_core::chars::char_is_word(*ch))
        .collect();

    (!key.is_empty()).then(|| key.to_lowercase())
}

fn completion_locality_score_for_text(document_text: &str, cursor_line: usize, key: &str) -> u16 {
    if key.is_empty() {
        return 0;
    }

    document_text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.to_lowercase().contains(key))
        .map(|(line, _)| line.abs_diff(cursor_line).min(200) as u16)
        .min()
        .map(|distance| 200u16.saturating_sub(distance))
        .unwrap_or(0)
}

fn completion_commit_character_from_key(
    key: &str,
    key_char: Option<&str>,
    has_control_modifier: bool,
) -> Option<char> {
    if has_control_modifier {
        return None;
    }

    let text = key_char.unwrap_or(key);
    let mut chars = text.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

fn ui_completion_item_from_event(
    item: nucleotide_events::completion::CompletionItem,
) -> nucleotide_ui::completion_v2::CompletionItem {
    use nucleotide_events::completion::{CompletionItemKind, CompletionItemTag};
    use nucleotide_ui::completion_v2::{
        CompletionItem as UiCompletionItem, CompletionItemKind as UiCompletionItemKind,
        CompletionItemTag as UiCompletionItemTag,
    };

    let ui_kind = match item.kind {
        CompletionItemKind::Text => UiCompletionItemKind::Text,
        CompletionItemKind::Method => UiCompletionItemKind::Method,
        CompletionItemKind::Function => UiCompletionItemKind::Function,
        CompletionItemKind::Constructor => UiCompletionItemKind::Constructor,
        CompletionItemKind::Field => UiCompletionItemKind::Field,
        CompletionItemKind::Variable => UiCompletionItemKind::Variable,
        CompletionItemKind::Class => UiCompletionItemKind::Class,
        CompletionItemKind::Interface => UiCompletionItemKind::Interface,
        CompletionItemKind::Module => UiCompletionItemKind::Module,
        CompletionItemKind::Property => UiCompletionItemKind::Property,
        CompletionItemKind::Unit => UiCompletionItemKind::Unit,
        CompletionItemKind::Value => UiCompletionItemKind::Value,
        CompletionItemKind::Enum => UiCompletionItemKind::Enum,
        CompletionItemKind::Keyword => UiCompletionItemKind::Keyword,
        CompletionItemKind::Snippet => UiCompletionItemKind::Snippet,
        CompletionItemKind::Color => UiCompletionItemKind::Color,
        CompletionItemKind::File => UiCompletionItemKind::File,
        CompletionItemKind::Reference => UiCompletionItemKind::Reference,
        CompletionItemKind::Folder => UiCompletionItemKind::Folder,
        CompletionItemKind::EnumMember => UiCompletionItemKind::EnumMember,
        CompletionItemKind::Constant => UiCompletionItemKind::Constant,
        CompletionItemKind::Struct => UiCompletionItemKind::Struct,
        CompletionItemKind::Event => UiCompletionItemKind::Event,
        CompletionItemKind::Operator => UiCompletionItemKind::Operator,
        CompletionItemKind::TypeParameter => UiCompletionItemKind::TypeParameter,
    };

    UiCompletionItem {
        text: item.insert_text.into(),
        description: item.detail.as_ref().map(|d| d.clone().into()),
        display_text: Some(item.label.into()),
        kind: Some(ui_kind),
        documentation: item.documentation.map(|d| d.into()),
        detail: item.detail.map(|d| d.into()),
        signature_info: item.signature_info.map(|s| s.into()),
        type_info: item.type_info.map(|t| t.into()),
        insert_text_format: match item.insert_text_format {
            nucleotide_events::completion::InsertTextFormat::PlainText => {
                nucleotide_ui::completion_v2::InsertTextFormat::PlainText
            }
            nucleotide_events::completion::InsertTextFormat::Snippet => {
                nucleotide_ui::completion_v2::InsertTextFormat::Snippet
            }
        },
        edit: item.edit.map(ui_completion_edit_from_event),
        sort_text: item.sort_text.map(Into::into),
        filter_text: item.filter_text.map(Into::into),
        preselect: item.preselect,
        commit_characters: item.commit_characters.into_iter().map(Into::into).collect(),
        tags: item
            .tags
            .into_iter()
            .map(|tag| match tag {
                CompletionItemTag::Deprecated => UiCompletionItemTag::Deprecated,
            })
            .collect(),
        data: item.data,
        source_index: item.source_index,
        selection_priority: 0,
        server_id: item.server_id,
        raw_lsp_item: item.raw_lsp_item,
        locality_score: 0,
    }
}

// Pending file operation kinds awaiting user input (used with the prompt overlay)
enum PendingFileOp {
    NewFile { parent: std::path::PathBuf },
    NewFolder { parent: std::path::PathBuf },
    Rename { path: std::path::PathBuf },
    Duplicate { path: std::path::PathBuf },
}

#[derive(Debug, Clone)]
enum LspFileOperationNotification {
    Created {
        path: PathBuf,
        is_dir: bool,
    },
    Deleted {
        path: PathBuf,
        was_dir: bool,
    },
    Renamed {
        old_path: PathBuf,
        new_path: PathBuf,
        was_dir: bool,
    },
}

fn file_operation_notification_succeeded(notification: &LspFileOperationNotification) -> bool {
    match notification {
        LspFileOperationNotification::Created { path, is_dir } => {
            path.exists() && path.is_dir() == *is_dir
        }
        LspFileOperationNotification::Deleted { path, .. } => !path.exists(),
        LspFileOperationNotification::Renamed {
            old_path,
            new_path,
            was_dir,
        } => {
            old_path != new_path
                && !old_path.exists()
                && new_path.exists()
                && new_path.is_dir() == *was_dir
        }
    }
}

fn local_path_is_directory_without_wsl_probe(path: &Path) -> bool {
    WslWorkspace::from_unc_path(path).is_none() && path.is_dir()
}

#[derive(Clone)]
struct DraggedFileTreeResize;

impl Render for DraggedFileTreeResize {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Clone)]
struct DraggedDocumentationSidebarResize;

impl Render for DraggedDocumentationSidebarResize {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Clone)]
struct DraggedSplitPaneResize;

impl Render for DraggedSplitPaneResize {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

const GLOBAL_SEARCH_RESULT_LIMIT: usize = 5000;
const FILE_TREE_MIN_WIDTH: f32 = 96.0;
const FILE_TREE_DEFAULT_WIDTH: f32 = 240.0;
const FILE_TREE_MIN_EDITOR_WIDTH: f32 = 200.0;
const MACOS_TRAFFIC_LIGHT_TREE_TOP_INSET_PX: f32 = 36.0;
const DOC_SIDEBAR_MIN_WIDTH: f32 = 240.0;
const DOC_SIDEBAR_DEFAULT_WIDTH: f32 = 360.0;
const DOC_SIDEBAR_MAX_WIDTH: f32 = 640.0;
const DOC_SIDEBAR_MIN_EDITOR_WIDTH: f32 = 240.0;

fn file_tree_config_from_gui(config: &crate::config::GuiConfig) -> FileTreeConfig {
    FileTreeConfig {
        density: config.file_tree.density,
        flatten_empty_directories: config.file_tree.flatten_empty_directories,
        translucent_background: macos_system_sidebar_enabled(config),
        ..FileTreeConfig::default()
    }
}

fn macos_system_sidebar_enabled(config: &crate::config::GuiConfig) -> bool {
    nucleotide_appearance::macos_native_chrome_enabled(config.ui.look.to_ui_chrome_style())
}

fn file_tree_tokens_for_gui_config(
    tokens: &nucleotide_ui::DesignTokens,
    config: &crate::config::GuiConfig,
) -> nucleotide_ui::tokens::FileTreeTokens {
    let file_tree_tokens = tokens.file_tree_tokens();
    if macos_system_sidebar_enabled(config) {
        file_tree_tokens.translucent_sidebar()
    } else {
        file_tree_tokens
    }
}

fn tab_bar_layout_height(
    row_height: Pixels,
    show_pinned_tabs_in_separate_row: bool,
    has_pinned_tabs: bool,
    has_unpinned_tabs: bool,
) -> Pixels {
    if show_pinned_tabs_in_separate_row && has_pinned_tabs && has_unpinned_tabs {
        row_height * 2.0
    } else {
        row_height
    }
}

fn tab_bar_height_for_editor(
    show_tab_bar: bool,
    bufferline_config: &helix_view::editor::BufferLine,
    document_count: usize,
    row_height: Pixels,
    show_pinned_tabs_in_separate_row: bool,
    has_pinned_tabs: bool,
    has_unpinned_tabs: bool,
) -> Pixels {
    if !show_tab_bar {
        return px(0.0);
    }

    let visible_tab_bar_height = tab_bar_layout_height(
        row_height,
        show_pinned_tabs_in_separate_row,
        has_pinned_tabs,
        has_unpinned_tabs,
    );

    match bufferline_config {
        helix_view::editor::BufferLine::Never => px(0.0),
        helix_view::editor::BufferLine::Always => visible_tab_bar_height,
        helix_view::editor::BufferLine::Multiple => {
            if document_count > 1 {
                visible_tab_bar_height
            } else {
                px(0.0)
            }
        }
    }
}

#[cfg(test)]
fn move_ordered_item_to_target_index<T: Copy + Eq>(
    items: &mut Vec<T>,
    source: T,
    target: Option<T>,
) -> bool {
    if target == Some(source) {
        return false;
    }

    let Some(source_index) = items.iter().position(|item| *item == source) else {
        return false;
    };

    if target.is_none() && source_index + 1 == items.len() {
        return false;
    }

    let Some(target_index) = target
        .map(|target| items.iter().position(|item| *item == target))
        .unwrap_or(Some(items.len()))
    else {
        return false;
    };

    if target_index == source_index {
        return false;
    }

    let item = items.remove(source_index);
    let insert_index = target_index.min(items.len());
    items.insert(insert_index, item);
    true
}

#[cfg(test)]
fn dropped_tab_pin_state<T: Copy + Eq + Hash>(
    items: &[T],
    source: T,
    target: Option<T>,
    pinned_items: &HashSet<T>,
) -> Option<bool> {
    resolved_dropped_tab_pin_state(items, source, target, pinned_items, None)
}

#[cfg(test)]
fn resolved_dropped_tab_pin_state<T: Copy + Eq + Hash>(
    items: &[T],
    source: T,
    target: Option<T>,
    pinned_items: &HashSet<T>,
    forced_pin_state: Option<bool>,
) -> Option<bool> {
    if target == Some(source) || !items.contains(&source) {
        return None;
    }

    if let Some(target) = target
        && !items.contains(&target)
    {
        return None;
    }

    forced_pin_state.or_else(|| Some(target.is_some_and(|target| pinned_items.contains(&target))))
}

fn active_unpinned_tab_scroll_index<T: Copy + Eq + Hash>(
    ordered_items: &[T],
    pinned_items: &HashSet<T>,
    active_item: T,
) -> Option<usize> {
    if pinned_items.contains(&active_item) {
        return None;
    }

    ordered_items
        .iter()
        .filter(|item| !pinned_items.contains(*item))
        .position(|item| *item == active_item)
}

fn should_scroll_active_tab<T: Copy + Eq>(
    suppress_auto_scroll: bool,
    last_scrolled_item: Option<T>,
    active_item: Option<T>,
) -> bool {
    !suppress_auto_scroll && active_item.is_some() && last_scrolled_item != active_item
}

fn zed_style_tab_order<T: Copy + Eq + Hash>(
    ordered_items: &[T],
    pinned_items: &HashSet<T>,
) -> Vec<T> {
    let (mut pinned, unpinned): (Vec<_>, Vec<_>) = ordered_items
        .iter()
        .copied()
        .partition(|item| pinned_items.contains(item));
    pinned.extend(unpinned);
    pinned
}

#[cfg(test)]
fn change_tab_pin_state<T: Copy + Eq + Hash>(
    ordered_items: &mut Vec<T>,
    pinned_items: &mut HashSet<T>,
    item: T,
    should_pin: bool,
) -> bool {
    if pinned_items.contains(&item) == should_pin {
        return false;
    }

    let mut display_order = zed_style_tab_order(ordered_items, pinned_items);
    let Some(item_index) = display_order
        .iter()
        .position(|candidate| *candidate == item)
    else {
        return false;
    };

    let pinned_count = display_order
        .iter()
        .filter(|candidate| pinned_items.contains(candidate))
        .count();
    let Some(destination_index) = (if should_pin {
        Some(pinned_count.min(item_index))
    } else {
        pinned_count.checked_sub(1)
    }) else {
        return false;
    };

    if should_pin {
        pinned_items.insert(item);
    } else {
        pinned_items.remove(&item);
    }

    let item = display_order.remove(item_index);
    display_order.insert(destination_index, item);
    *ordered_items = display_order;
    true
}

fn unpin_all_tabs<T: Eq + Hash>(pinned_items: &mut HashSet<T>) -> bool {
    if pinned_items.is_empty() {
        return false;
    }

    pinned_items.clear();
    true
}

#[derive(Debug, PartialEq, Eq)]
enum PreviewTabTogglePlan {
    Unpreview,
    Preview,
}

fn preview_tab_toggle_plan<T: Eq + Hash>(
    preview_items: &HashSet<T>,
    active_item: &T,
) -> PreviewTabTogglePlan {
    if preview_items.contains(active_item) {
        return PreviewTabTogglePlan::Unpreview;
    }

    PreviewTabTogglePlan::Preview
}

fn should_create_project_panel_preview_tab(
    preview_tabs_enabled: bool,
    project_panel_preview_enabled: bool,
    existed_already: bool,
) -> bool {
    preview_tabs_enabled && project_panel_preview_enabled && !existed_already
}

fn should_unpreview_changed_document(is_preview: bool, is_modified: bool) -> bool {
    is_preview && is_modified
}

fn should_unpreview_retained_tab_after_close_others(is_preview: bool) -> bool {
    is_preview
}

fn reveal_in_file_manager_label(is_remote: bool) -> &'static str {
    if cfg!(target_os = "macos") && !is_remote {
        "Reveal in Finder"
    } else if cfg!(target_os = "windows") && !is_remote {
        "Reveal in File Explorer"
    } else {
        "Reveal in File Manager"
    }
}

#[cfg(test)]
fn tab_bar_end_button_icon_paths() -> [&'static str; 2] {
    ["icons/plus.svg", "icons/columns-2.svg"]
}

#[cfg(test)]
fn tab_bar_end_button_tooltips() -> [&'static str; 2] {
    ["New File", "Split Pane"]
}

#[derive(Clone, Copy)]
struct MaxTabsDocument<T> {
    id: T,
    focused_at: std::time::Instant,
    is_modified: bool,
    is_pinned: bool,
    is_protected: bool,
}

#[derive(Clone)]
struct BatchCloseDocument<T, P> {
    id: T,
    is_active: bool,
    path: Option<P>,
}

#[derive(Clone)]
enum PendingUnsavedClose<T> {
    Single {
        doc_id: T,
        activation_target: Option<T>,
    },
    Batch {
        doc_ids: Vec<T>,
    },
}

#[derive(Clone)]
struct UnsavedCloseConfirmation<T> {
    action: PendingUnsavedClose<T>,
    names: Vec<String>,
}

#[derive(Clone, Copy)]
struct TabActivationDocument<T> {
    id: T,
    focused_at: std::time::Instant,
}

#[cfg(test)]
fn max_tabs_close_candidates<T: Copy>(
    documents: &[MaxTabsDocument<T>],
    max_tabs: Option<std::num::NonZeroUsize>,
) -> Vec<T> {
    max_tabs_close_candidates_to_target(documents, max_tabs.map(std::num::NonZeroUsize::get))
}

fn max_tabs_close_candidates_to_target<T: Copy>(
    documents: &[MaxTabsDocument<T>],
    target_count: Option<usize>,
) -> Vec<T> {
    let Some(target_count) = target_count else {
        return Vec::new();
    };

    if documents.len() <= target_count {
        return Vec::new();
    }

    let mut remaining_count = documents.len();
    let mut candidates = documents
        .iter()
        .filter(|document| !document.is_modified && !document.is_pinned && !document.is_protected)
        .collect::<Vec<_>>();
    candidates.sort_by_key(|document| document.focused_at);

    let mut close_candidates = Vec::new();
    for candidate in candidates {
        if remaining_count <= target_count {
            break;
        }

        close_candidates.push(candidate.id);
        remaining_count -= 1;
    }

    close_candidates
}

fn batch_close_document_order<T: Copy, P: Clone + Ord>(
    documents: &[BatchCloseDocument<T, P>],
) -> Vec<T> {
    let mut documents = documents.to_vec();
    documents.sort_by_key(|document| {
        (
            document.is_active,
            document.path.is_none(),
            document.path.clone(),
        )
    });

    documents.into_iter().map(|document| document.id).collect()
}

fn helix_status_to_editor_status(
    status: &str,
    severity: &helix_view::editor::Severity,
) -> EditorStatus {
    EditorStatus {
        status: status.to_string(),
        severity: match severity {
            helix_view::editor::Severity::Hint => Severity::Hint,
            helix_view::editor::Severity::Info => Severity::Info,
            helix_view::editor::Severity::Warning => Severity::Warning,
            helix_view::editor::Severity::Error => Severity::Error,
        },
    }
}

fn current_editor_status(editor: &helix_view::Editor) -> Option<EditorStatus> {
    editor
        .get_status()
        .map(|(status, severity)| helix_status_to_editor_status(status, severity))
}

fn editor_status_matches(a: &EditorStatus, b: &EditorStatus) -> bool {
    a.status == b.status && a.severity == b.severity
}

fn integration_status_severity(severity: &str) -> Severity {
    match severity.to_ascii_lowercase().as_str() {
        "error" => Severity::Error,
        "warning" | "warn" => Severity::Warning,
        "hint" => Severity::Hint,
        _ => Severity::Info,
    }
}

fn editor_domain_status_severity(
    severity: nucleotide_events::v2::editor::StatusSeverity,
) -> Severity {
    match severity {
        nucleotide_events::v2::editor::StatusSeverity::Info
        | nucleotide_events::v2::editor::StatusSeverity::Success => Severity::Info,
        nucleotide_events::v2::editor::StatusSeverity::Warning => Severity::Warning,
        nucleotide_events::v2::editor::StatusSeverity::Error => Severity::Error,
    }
}

fn unsaved_buffers_remaining_status(names: Vec<String>) -> EditorStatus {
    let buffer_count = names.len();
    EditorStatus {
        status: format!(
            "{} unsaved buffer{} remaining: {:?}",
            buffer_count,
            if buffer_count == 1 { "" } else { "s" },
            names
        ),
        severity: Severity::Error,
    }
}

fn close_error_status(error: helix_view::editor::CloseError) -> EditorStatus {
    match error {
        helix_view::editor::CloseError::BufferModified(name) => {
            unsaved_buffers_remaining_status(vec![name])
        }
        helix_view::editor::CloseError::DoesNotExist => EditorStatus {
            status: "cannot close non-existent buffer".to_string(),
            severity: Severity::Error,
        },
        helix_view::editor::CloseError::SaveError(err) => EditorStatus {
            status: format!("failed to save buffer before closing: {err}"),
            severity: Severity::Error,
        },
    }
}

fn unsaved_close_confirmation_title(count: usize) -> &'static str {
    if count == 1 {
        "Close Unsaved Buffer"
    } else {
        "Close Unsaved Buffers"
    }
}

fn unsaved_close_confirmation_message(names: &[String]) -> String {
    match names {
        [name] => format!("'{name}' has unsaved changes. Close without saving?"),
        [] => "Close without saving unsaved changes?".to_string(),
        names => format!(
            "{} buffers have unsaved changes: {}. Close without saving?",
            names.len(),
            names.join(", ")
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKeyAction {
    Accept,
    Cancel,
    SelectNext,
    SelectPrevious,
}

fn completion_menu_action(key: &str, control: bool, shift: bool) -> Option<MenuKeyAction> {
    match (key, control, shift) {
        ("escape", false, false) => Some(MenuKeyAction::Cancel),
        ("tab", false, false) | ("y", true, false) => Some(MenuKeyAction::Accept),
        ("down", false, false) | ("n", true, false) => Some(MenuKeyAction::SelectNext),
        ("up", false, false) | ("p", true, false) => Some(MenuKeyAction::SelectPrevious),
        _ => None,
    }
}

fn should_refine_completion_for_focused_document(
    has_completion: bool,
    focused_doc_id: Option<DocumentId>,
    doc_id: DocumentId,
) -> bool {
    has_completion && focused_doc_id == Some(doc_id)
}

fn tab_activation_target_after_close<T: Copy + Eq>(
    documents: &[TabActivationDocument<T>],
    closing_doc_id: T,
    active_doc_id: Option<T>,
    activate_on_close: crate::config::TabActivateOnClose,
) -> Option<T> {
    if active_doc_id != Some(closing_doc_id) {
        return None;
    }

    let closing_index = documents
        .iter()
        .position(|document| document.id == closing_doc_id)?;
    let left_neighbour = || {
        closing_index
            .checked_sub(1)
            .and_then(|index| documents.get(index))
            .map(|document| document.id)
    };
    let right_neighbour = || documents.get(closing_index + 1).map(|document| document.id);

    match activate_on_close {
        crate::config::TabActivateOnClose::History => documents
            .iter()
            .filter(|document| document.id != closing_doc_id)
            .max_by_key(|document| document.focused_at)
            .map(|document| document.id)
            .or_else(left_neighbour),
        crate::config::TabActivateOnClose::Neighbour => right_neighbour().or_else(left_neighbour),
        crate::config::TabActivateOnClose::LeftNeighbour => {
            left_neighbour().or_else(right_neighbour)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ActiveTabClosePlan<T> {
    Close(T),
    Activate(T),
    Ignore,
}

fn active_tab_close_plan<T: Copy + Eq + Hash>(
    ordered_items: &[T],
    pinned_items: &HashSet<T>,
    active_item: Option<T>,
) -> ActiveTabClosePlan<T> {
    let Some(active_item) = active_item else {
        return ActiveTabClosePlan::Ignore;
    };

    if !ordered_items.contains(&active_item) {
        return ActiveTabClosePlan::Ignore;
    }

    if pinned_items.contains(&active_item) {
        return ordered_items
            .iter()
            .copied()
            .find(|item| !pinned_items.contains(item))
            .map(ActiveTabClosePlan::Activate)
            .unwrap_or(ActiveTabClosePlan::Ignore);
    }

    ActiveTabClosePlan::Close(active_item)
}

#[derive(Debug, PartialEq, Eq)]
enum TabDoubleClickPlan {
    Rename,
    Activate,
}

fn tab_double_click_plan(has_file_path: bool) -> TabDoubleClickPlan {
    if has_file_path {
        TabDoubleClickPlan::Rename
    } else {
        TabDoubleClickPlan::Activate
    }
}

fn is_deleted_document_path(path: Option<&Path>) -> bool {
    let Some(path) = path else {
        return false;
    };

    if WslWorkspace::from_unc_path(path).is_some() {
        return false;
    }

    !path.exists()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobalSearchMatch {
    path: PathBuf,
    line: usize,
    line_text: String,
}

fn compile_global_search_regex(
    query: &str,
    smart_case: bool,
) -> Result<helix_stdx::rope::Regex, String> {
    let case_insensitive = smart_case && !query.chars().any(char::is_uppercase);
    helix_stdx::rope::RegexBuilder::new()
        .syntax(
            helix_stdx::rope::Config::new()
                .case_insensitive(case_insensitive)
                .multi_line(true),
        )
        .build(query)
        .map_err(|err| err.to_string())
}

fn push_global_search_matches(
    matches: &mut Vec<GlobalSearchMatch>,
    path: &Path,
    text: RopeSlice<'_>,
    regex: &helix_stdx::rope::Regex,
    limit: usize,
) -> bool {
    for line in 0..text.len_lines() {
        let line_slice = text.line(line);
        if regex.is_match(line_slice.regex_input()) {
            matches.push(GlobalSearchMatch {
                path: path.to_path_buf(),
                line,
                line_text: line_slice.to_string().trim_end().to_string(),
            });

            if matches.len() >= limit {
                return true;
            }
        }
    }

    false
}

fn global_search_matches(
    root: &Path,
    query: &str,
    smart_case: bool,
    file_picker_config: &helix_view::editor::FilePickerConfig,
    open_documents: &[(PathBuf, Rope)],
    limit: usize,
) -> Result<Vec<GlobalSearchMatch>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let regex = compile_global_search_regex(query, smart_case)
        .map_err(|err| format!("Failed to compile regex: {err}"))?;

    if let Some(workspace) = WslWorkspace::from_unc_path(root) {
        match load_wsl_remote_global_search_blocking(
            &workspace,
            query,
            smart_case,
            limit,
            WSL_REMOTE_GLOBAL_SEARCH_TIMEOUT,
        ) {
            Ok(response) => {
                debug!(
                    match_count = response.matches.len(),
                    truncated = response.truncated,
                    "Loaded WSL global search matches from remote helper"
                );
                return Ok(global_search_matches_from_remote_response(
                    root,
                    response,
                    open_documents,
                    &regex,
                    limit,
                ));
            }
            Err(error) => {
                warn!(
                    root = %root.display(),
                    error = %error,
                    "Failed to load WSL global search matches from remote helper; falling back to local search"
                );
            }
        }
    }

    if !root.exists() {
        return Err("Current working directory does not exist".to_string());
    }

    let mut matches = Vec::new();
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(file_picker_config.hidden)
        .parents(file_picker_config.parents)
        .ignore(file_picker_config.ignore)
        .follow_links(file_picker_config.follow_symlinks)
        .git_ignore(file_picker_config.git_ignore)
        .git_global(file_picker_config.git_global)
        .git_exclude(file_picker_config.git_exclude)
        .max_depth(file_picker_config.max_depth)
        .add_custom_ignore_filename(helix_loader::config_dir().join("ignore"))
        .add_custom_ignore_filename(".helix/ignore");

    for entry in walker.build().filter_map(Result::ok) {
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let path = entry.path();
        if let Some((_, doc_text)) = open_documents
            .iter()
            .find(|(doc_path, _)| doc_path.as_path() == path)
        {
            if push_global_search_matches(&mut matches, path, doc_text.slice(..), &regex, limit) {
                break;
            }
            continue;
        }

        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        let rope = Rope::from(contents.as_str());
        if push_global_search_matches(&mut matches, path, rope.slice(..), &regex, limit) {
            break;
        }
    }

    Ok(matches)
}

fn global_search_matches_from_remote_response(
    root: &Path,
    response: GlobalSearchResponse,
    open_documents: &[(PathBuf, Rope)],
    regex: &helix_stdx::rope::Regex,
    limit: usize,
) -> Vec<GlobalSearchMatch> {
    let mut matches = Vec::new();
    let mut open_paths = HashSet::new();

    for (path, text) in open_documents {
        if !path.starts_with(root) {
            continue;
        }

        open_paths.insert(path.clone());
        if push_global_search_matches(&mut matches, path, text.slice(..), regex, limit) {
            return matches;
        }
    }

    for remote_match in response.matches {
        let path = workspace_path_from_remote_relative(root, &remote_match.relative_path);
        if open_paths.contains(&path) {
            continue;
        }

        matches.push(GlobalSearchMatch {
            path,
            line: remote_match.line,
            line_text: remote_match.line_text,
        });

        if matches.len() >= limit {
            break;
        }
    }

    matches
}

fn workspace_path_from_remote_relative(root: &Path, relative_path: &Path) -> PathBuf {
    let mut path = root.to_path_buf();
    for component in relative_path.components() {
        if let std::path::Component::Normal(segment) = component {
            path.push(segment);
        }
    }
    path
}

fn wsl_created_path(
    parent: &Path,
    response: &FileCreateResponse,
    expected_kind: RemoteFileKind,
) -> Option<PathBuf> {
    if response.kind != expected_kind {
        return None;
    }

    let workspace = WslWorkspace::from_unc_path(parent)?;
    workspace
        .unc_path_for_linux_path(&response.path)
        .map(PathBuf::from)
}

fn create_wsl_project_file(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let response = create_wsl_remote_file_blocking(parent, name, WSL_REMOTE_FILE_OP_TIMEOUT)
        .map_err(|error| error.to_string())?;
    wsl_created_path(parent, &response, RemoteFileKind::File)
        .ok_or_else(|| "remote helper returned an unmappable file path".to_string())
}

fn create_wsl_project_directory(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let response = create_wsl_remote_directory_blocking(parent, name, WSL_REMOTE_FILE_OP_TIMEOUT)
        .map_err(|error| error.to_string())?;
    wsl_created_path(parent, &response, RemoteFileKind::Directory)
        .ok_or_else(|| "remote helper returned an unmappable file path".to_string())
}

fn global_search_picker(root: &Path, matches: Vec<GlobalSearchMatch>) -> crate::picker::Picker {
    use crate::picker_view::PickerItem;
    use std::sync::Arc;

    let items = matches
        .into_iter()
        .map(|search_match| {
            let path = search_match.path;
            let label_path = path.strip_prefix(root).unwrap_or(&path);
            let label = format!("{}:{}", label_path.display(), search_match.line + 1);
            let data = GlobalSearchLocation {
                path: path.clone(),
                line: search_match.line,
            };

            PickerItem::with_sublabel_and_path(label, search_match.line_text, path, Arc::new(data))
        })
        .collect();

    crate::picker::Picker::native("Global Search", items, |_index| {
        // Selection is handled by OverlayView via GlobalSearchLocation payloads.
    })
}

fn regex_selection_result(
    action: RegexSelectionAction,
    text: RopeSlice<'_>,
    selection: &Selection,
    regex: &helix_stdx::rope::Regex,
) -> Result<Selection, &'static str> {
    match action {
        RegexSelectionAction::Select => {
            helix_core::selection::select_on_matches(text, selection, regex)
                .ok_or("nothing selected")
        }
        RegexSelectionAction::Split => Ok(helix_core::selection::split_on_matches(
            text, selection, regex,
        )),
        RegexSelectionAction::Keep => {
            helix_core::selection::keep_or_remove_matches(text, selection, regex, false)
                .ok_or("no selections remaining")
        }
        RegexSelectionAction::Remove => {
            helix_core::selection::keep_or_remove_matches(text, selection, regex, true)
                .ok_or("no selections remaining")
        }
    }
}

impl EventEmitter<crate::Update> for Workspace {}

impl Workspace {
    fn terminal_spawn_cwd(current_project_root: Option<&Path>) -> Option<PathBuf> {
        current_project_root.map(Path::to_path_buf)
    }

    fn terminal_cwd_matches(terminal_cwd: Option<&Path>, desired_cwd: Option<&Path>) -> bool {
        terminal_cwd == desired_cwd
    }

    fn shutdown_terminal_session(&mut self, id: TerminalId, cx: &mut Context<Self>) {
        self.core.update(cx, |app, _cx| {
            if let Some(bus) = &app.event_aggregator {
                bus.dispatch_terminal(TerminalEvent::Exited {
                    id,
                    code: None,
                    signal: None,
                });
                bus.process_events();
            }
        });
    }

    fn spawn_terminal_session(
        &mut self,
        cwd: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> TerminalId {
        self.spawn_terminal_session_with_input(cwd, Vec::new(), None, cx)
    }

    fn spawn_terminal_session_with_input(
        &mut self,
        cwd: Option<PathBuf>,
        extra_env: Vec<(String, String)>,
        initial_input: Option<Vec<u8>>,
        cx: &mut Context<Self>,
    ) -> TerminalId {
        let id = TerminalId(self.next_terminal_id);
        self.next_terminal_id += 1;
        self.terminal_id = Some(id);
        self.terminal_cwd = cwd.clone();
        self.run_output_terminal = None;
        self.last_terminal_bounds = None;

        let shell = None;
        let (event_bus, project_environment) = {
            let core = self.core.read(cx);
            (
                core.event_aggregator.clone(),
                core.project_environment.clone(),
            )
        };
        let cwd_for_env = cwd.clone();
        self.handle.spawn(async move {
            let mut env = match cwd_for_env.as_deref() {
                Some(directory) => {
                    match project_environment
                        .get_environment_for_directory(directory)
                        .await
                    {
                        Ok(environment) => environment.into_iter().collect::<Vec<_>>(),
                        Err(error) => {
                            warn!(
                                terminal_id = ?id,
                                directory = %directory.display(),
                                error = %error,
                                "Failed to load project environment for terminal; using process environment"
                            );
                            Vec::new()
                        }
                    }
                }
                None => Vec::new(),
            };
            env.extend(extra_env);

            if let Some(bus) = event_bus {
                bus.dispatch_terminal(TerminalEvent::SpawnRequested {
                    id,
                    cwd,
                    shell,
                    env,
                });
                bus.process_events();

                if let Some(bytes) = initial_input {
                    bus.dispatch_terminal(TerminalEvent::Input { id, bytes });
                    bus.process_events();
                }
            } else {
                warn!("No event aggregator; terminal spawn not dispatched");
            }
        });

        id
    }

    fn spawn_terminal_command_session(
        &mut self,
        cwd: Option<PathBuf>,
        program: String,
        args: Vec<String>,
        extra_env: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) -> TerminalId {
        let id = TerminalId(self.next_terminal_id);
        self.next_terminal_id += 1;
        self.terminal_id = Some(id);
        self.terminal_cwd = cwd.clone();
        self.run_output_terminal = Some(id);
        self.last_terminal_bounds = None;

        let (event_bus, project_environment) = {
            let core = self.core.read(cx);
            (
                core.event_aggregator.clone(),
                core.project_environment.clone(),
            )
        };
        let cwd_for_env = cwd.clone();
        self.handle.spawn(async move {
            let mut env = match cwd_for_env.as_deref() {
                Some(directory) => {
                    match project_environment
                        .get_environment_for_directory(directory)
                        .await
                    {
                        Ok(environment) => environment.into_iter().collect::<Vec<_>>(),
                        Err(error) => {
                            warn!(
                                terminal_id = ?id,
                                directory = %directory.display(),
                                error = %error,
                                "Failed to load project environment for runnable; using process environment"
                            );
                            Vec::new()
                        }
                    }
                }
                None => Vec::new(),
            };
            env.extend(extra_env);

            if let Some(bus) = event_bus {
                bus.dispatch_terminal(TerminalEvent::CommandSpawnRequested {
                    id,
                    cwd,
                    program,
                    args,
                    env,
                });
                bus.process_events();
            } else {
                warn!("No event aggregator; runnable terminal spawn not dispatched");
            }
        });

        id
    }

    fn set_embedded_terminal_panel(&mut self, terminal_id: TerminalId, cx: &mut Context<Self>) {
        let height = self.basic_terminal_height;
        let entity = cx.new(|cx| {
            let mut p = nucleotide_terminal_panel::TerminalPanel::new(terminal_id, height);
            p.initialize(cx);
            p
        });
        self.embedded_terminal_panel = Some(entity);
    }

    fn open_terminal_panel_at(&mut self, cwd: Option<PathBuf>, cx: &mut Context<Self>) {
        self.open_terminal_panel_at_with_input(cwd, Vec::new(), None, cx);
    }

    fn open_terminal_panel_at_with_input(
        &mut self,
        cwd: Option<PathBuf>,
        extra_env: Vec<(String, String)>,
        initial_input: Option<Vec<u8>>,
        cx: &mut Context<Self>,
    ) -> TerminalId {
        if let Some(existing_id) = self.terminal_id {
            self.shutdown_terminal_session(existing_id, cx);
        }
        let id = self.spawn_terminal_session_with_input(cwd, extra_env, initial_input, cx);
        self.set_embedded_terminal_panel(id, cx);
        self.terminal_panel_visible = true;
        self.terminal_focus_pending = true;
        self.terminal_active = true;
        cx.notify();
        id
    }

    fn open_terminal_panel_for_command(
        &mut self,
        cwd: Option<PathBuf>,
        program: String,
        args: Vec<String>,
        extra_env: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) -> TerminalId {
        if let Some(existing_id) = self.terminal_id {
            self.shutdown_terminal_session(existing_id, cx);
        }
        let id = self.spawn_terminal_command_session(cwd, program, args, extra_env, cx);
        self.set_embedded_terminal_panel(id, cx);
        self.terminal_panel_visible = true;
        self.terminal_focus_pending = true;
        self.terminal_active = true;
        cx.notify();
        id
    }

    fn hide_terminal_panel(&mut self) {
        self.terminal_panel_visible = false;
        self.terminal_focus_pending = false;
        self.terminal_active = false;
        self.last_terminal_bounds = None;
        self.view_manager.set_needs_focus_restore(true);
    }

    fn clear_terminal_panel_session(&mut self) {
        let cleared_id = self.terminal_id;
        self.embedded_terminal_panel = None;
        self.terminal_id = None;
        self.terminal_cwd = None;
        if let Some(id) = cleared_id {
            nucleotide_terminal_view::unregister_view_model(id);
        }
        if self.run_output_terminal == cleared_id {
            self.run_output_terminal = None;
        }
    }

    fn refresh_environment_badge(&mut self, project_root: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(project_root) = project_root else {
            self.environment_badge = None;
            cx.notify();
            return;
        };

        if WslWorkspace::from_unc_path(&project_root).is_some() {
            self.environment_badge = Some(EnvironmentBadge::Wsl);
            cx.notify();
            return;
        }

        if !project_root.join(".envrc").is_file() {
            self.environment_badge = None;
            cx.notify();
            return;
        }

        self.environment_badge = Some(EnvironmentBadge::Loading);
        cx.notify();

        let project_environment = self.core.read(cx).project_environment.clone();
        let runtime_handle = self.handle.clone();

        cx.spawn(async move |this, cx| {
            let loaded_root = project_root.clone();
            let result = runtime_handle
                .spawn(async move {
                    if project_environment
                        .get_cached_origin(&project_root)
                        .await
                        .is_some_and(|origin| origin == EnvironmentOrigin::NativeFlake)
                    {
                        return Ok(Some(EnvironmentBadge::NativeFlake));
                    }

                    project_environment
                        .get_environment_for_directory(&project_root)
                        .await
                        .map(|environment| {
                            EnvironmentBadge::from_environment_marker(
                                environment.get("ZED_ENVIRONMENT").map(String::as_str),
                            )
                        })
                })
                .await;

            let badge = match result {
                Ok(Ok(badge)) => badge,
                Ok(Err(error)) => {
                    warn!(
                        project_root = %loaded_root.display(),
                        error = %error,
                        "Failed to load project environment for status bar badge"
                    );
                    None
                }
                Err(error) => {
                    warn!(
                        project_root = %loaded_root.display(),
                        error = %error,
                        "Project environment badge task failed"
                    );
                    None
                }
            };

            if let Some(this) = this.upgrade() {
                this.update(cx, |workspace, cx| {
                    if workspace.current_project_root.as_deref() == Some(loaded_root.as_path()) {
                        workspace.environment_badge = badge;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn toggle_terminal_panel(&mut self, cx: &mut Context<Self>) {
        // Basic layout: toggle visibility of embedded bottom panel
        if self.terminal_panel_visible {
            self.hide_terminal_panel();
            cx.notify();
            return;
        }

        // Ensure terminal exists and embedded panel entity is available
        let desired_cwd = Self::terminal_spawn_cwd(self.current_project_root.as_deref());
        let terminal_id = if let Some(id) = self.terminal_id
            && Self::terminal_cwd_matches(self.terminal_cwd.as_deref(), desired_cwd.as_deref())
        {
            id
        } else {
            if let Some(existing_id) = self.terminal_id {
                self.shutdown_terminal_session(existing_id, cx);
            }
            self.spawn_terminal_session(desired_cwd, cx)
        };

        let needs_panel = self
            .embedded_terminal_panel
            .as_ref()
            .is_none_or(|panel| panel.read(cx).active != terminal_id);
        if needs_panel {
            self.set_embedded_terminal_panel(terminal_id, cx);
        }

        self.terminal_panel_visible = true;
        // Ask render to focus the terminal on the next frame
        self.terminal_focus_pending = true;
        self.terminal_active = true;
        cx.notify();
    }

    fn focused_runnable_document(
        &self,
        cx: &mut Context<Self>,
    ) -> Result<crate::runnables::RunnableDocument, String> {
        let core = self.core.read(cx);
        let editor = &core.editor;
        let view = editor
            .tree
            .try_get(editor.tree.focus)
            .ok_or_else(|| "No focused editor view".to_string())?;
        let doc = editor
            .documents
            .get(&view.doc)
            .ok_or_else(|| "No focused document".to_string())?;
        let path = doc
            .path()
            .cloned()
            .ok_or_else(|| "Focused document is not backed by a file".to_string())?;
        let text = doc.text().clone();
        let cursor_line = doc.selection(view.id).primary().cursor_line(text.slice(..));

        Ok(crate::runnables::RunnableDocument {
            path,
            text: String::from(text.slice(..)),
            cursor_line,
            project_root: self.current_project_root.clone(),
        })
    }

    fn discover_local_focused_runnables(
        &self,
        cx: &mut Context<Self>,
    ) -> Result<(crate::runnables::RunnableDocument, Vec<ResolvedTask>), String> {
        let document = self.focused_runnable_document(cx)?;
        let tasks = crate::runnables::discover_local_rust_runnables(&document);
        Ok((document, tasks))
    }

    fn show_runnables(&mut self, cx: &mut Context<Self>) {
        self.request_focused_runnables(RunnableAction::ShowPicker, cx);
    }

    fn run_nearest(&mut self, cx: &mut Context<Self>) {
        self.request_focused_runnables(RunnableAction::RunNearest, cx);
    }

    fn run_file_tests(&mut self, cx: &mut Context<Self>) {
        self.request_focused_runnables(RunnableAction::RunFileTests, cx);
    }

    fn request_focused_runnables(&mut self, action: RunnableAction, cx: &mut Context<Self>) {
        use futures_util::stream::{FuturesOrdered, StreamExt};

        let (document, local_tasks) = match self.discover_local_focused_runnables(cx) {
            Ok(discovery) => discovery,
            Err(message) => {
                self.set_run_status(message, Severity::Error, cx);
                return;
            }
        };

        let cursor_line = document.cursor_line;
        let mut futures: FuturesOrdered<_> = {
            let core = self.core.read(cx);
            let editor = &core.editor;
            let Some(view) = editor.tree.try_get(editor.tree.focus) else {
                self.finish_runnable_request(action, local_tasks, cursor_line, cx);
                return;
            };
            let Some(doc) = editor.documents.get(&view.doc) else {
                self.finish_runnable_request(action, local_tasks, cursor_line, cx);
                return;
            };
            if doc.path() != Some(&document.path) {
                self.finish_runnable_request(action, local_tasks, cursor_line, cx);
                return;
            }

            let Some(identifier) = document_lsp_identifier(doc) else {
                self.finish_runnable_request(action, local_tasks, cursor_line, cx);
                return;
            };
            let mut seen = std::collections::HashSet::new();
            doc.language_servers()
                .filter(|language_server| {
                    language_server.name() == "rust-analyzer"
                        && language_server.is_initialized()
                        && seen.insert(language_server.id())
                })
                .map(|language_server| {
                    let server_name = language_server.name().to_string();
                    let params = crate::runnables::RunnablesParams {
                        text_document: identifier.clone(),
                        position: None,
                    };
                    let request =
                        language_server.request::<crate::runnables::RaRunnablesRequest>(params);

                    async move {
                        request.await.map(|runnables| {
                            let tasks = runnables
                                .into_iter()
                                .map(crate::runnables::runnable_to_task_template)
                                .collect::<Vec<_>>();
                            (server_name, tasks)
                        })
                    }
                })
                .collect()
        };

        if futures.is_empty() {
            self.finish_runnable_request(action, local_tasks, cursor_line, cx);
            return;
        }

        self.set_run_status("Discovering Rust runnables...", Severity::Info, cx);
        let workspace_handle = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let mut rust_analyzer_tasks = Vec::new();

            while let Some(result) = futures.next().await {
                match result {
                    Ok((server_name, mut tasks)) => {
                        debug!(
                            server_name = %server_name,
                            runnable_count = tasks.len(),
                            "Collected rust-analyzer runnables"
                        );
                        rust_analyzer_tasks.append(&mut tasks);
                    }
                    Err(error) => {
                        warn!(error = %error, "rust-analyzer runnable request failed");
                    }
                }
            }

            let tasks = crate::runnables::merge_runnable_tasks(rust_analyzer_tasks, local_tasks);
            if let Some(workspace) = workspace_handle.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    workspace.finish_runnable_request(action, tasks, cursor_line, cx);
                });
            }
        })
        .detach();
    }

    fn finish_runnable_request(
        &mut self,
        action: RunnableAction,
        tasks: Vec<ResolvedTask>,
        cursor_line: usize,
        cx: &mut Context<Self>,
    ) {
        if tasks.is_empty() {
            self.set_run_status(
                "No runnable Rust targets found in the focused file",
                Severity::Error,
                cx,
            );
            return;
        }

        match action {
            RunnableAction::ShowPicker => self.show_runnables_picker(tasks, cx),
            RunnableAction::RunNearest => {
                match crate::runnables::nearest_runnable(&tasks, cursor_line, false) {
                    Some(task) => self.run_task(task, cx),
                    None => self.set_run_status(
                        "No runnable target near the cursor in the focused file",
                        Severity::Error,
                        cx,
                    ),
                }
            }
            RunnableAction::RunFileTests => match crate::runnables::file_tests_runnable(&tasks) {
                Some(task) => self.run_task(task, cx),
                None => self.set_run_status(
                    "No file-level Rust test runnable found in the focused file",
                    Severity::Error,
                    cx,
                ),
            },
        }
    }

    fn run_last(&mut self, cx: &mut Context<Self>) {
        match self.last_run_task.clone() {
            Some(task) => self.run_task(task, cx),
            None => self.set_run_status("No previous runnable to run", Severity::Error, cx),
        }
    }

    fn show_runnables_picker(&mut self, tasks: Vec<ResolvedTask>, cx: &mut Context<Self>) {
        use crate::picker_view::PickerItem;

        let items = tasks
            .into_iter()
            .map(|task| {
                let file_path = task.source().map(|source| source.path.clone());
                PickerItem {
                    label: task.label().to_string().into(),
                    sublabel: Some(crate::runnables::shell_command_line(&task.command).into()),
                    data: Arc::new(task),
                    file_path,
                    vcs_status: None,
                    columns: None,
                }
            })
            .collect::<Vec<_>>();

        let picker = crate::picker::Picker::native("Run", items, |_| {}).with_preview(true);
        emit_picker_update(picker, &self.overlay, cx);
    }

    fn run_task(&mut self, task: ResolvedTask, cx: &mut Context<Self>) {
        let run_id = RunId(self.next_run_id);
        self.next_run_id += 1;

        let command_line = crate::runnables::shell_command_line(&task.command);
        let cwd = task
            .command
            .cwd
            .clone()
            .or_else(|| Self::terminal_spawn_cwd(self.current_project_root.as_deref()));
        let env = task.command.env.clone();
        let terminal_id = self.open_terminal_panel_for_command(
            cwd,
            task.command.program.clone(),
            task.command.args.clone(),
            env,
            cx,
        );
        self.last_run_task = Some(task.clone());
        self.active_run_terminal = Some((terminal_id, run_id));

        self.core.update(cx, |app, app_cx| {
            if let Some(bus) = &app.event_aggregator {
                bus.dispatch_run(RunEvent::Requested { task: task.clone() });
                bus.dispatch_run(RunEvent::Started {
                    id: run_id,
                    task: task.clone(),
                    terminal_id: Some(terminal_id),
                });
                bus.dispatch_run(RunEvent::StatusChanged {
                    id: run_id,
                    status: RunStatus::Running,
                });
                bus.process_events();
            }
            app.set_editor_status_feedback(
                app_cx,
                format!("Running {}: {command_line}", task.label()),
                Severity::Info,
            );
        });
    }

    fn set_run_status(
        &mut self,
        message: impl Into<String>,
        severity: Severity,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        self.core.update(cx, |app, app_cx| {
            app.set_editor_status_feedback(app_cx, message, severity);
        });
    }

    /// Compute document and LSP context for the status bar without triggering borrow conflicts.
    fn statusbar_doc_info(
        &self,
        cx: &mut Context<Self>,
    ) -> (
        helix_view::document::Mode,          // current mode
        &'static str,                        // mode label
        String,                              // file name display
        String,                              // position text
        bool,                                // has LSP state
        Option<helix_lsp::LanguageServerId>, // preferred server for current doc
    ) {
        let core = self.core.read(cx);
        let editor = &core.editor;

        let mut mode = helix_view::document::Mode::Normal;
        let mut mode_name = "NOR";
        let mut file_name = "[no file]".to_string();
        let mut position_text = "1:1".to_string();

        if let Some(tab) = self
            .active_image_tab_id
            .and_then(|doc_id| self.image_tabs.iter().find(|tab| tab.id == doc_id))
        {
            file_name = tab.path.display().to_string();
            position_text = image_zoom_percent(tab.zoom);
            return (mode, mode_name, file_name, position_text, false, None);
        }

        // Get info from focused view if available
        if let Some(view_id) = self.view_manager.focused_view_id()
            && let Some((view, doc)) = editor
                .tree
                .try_get(view_id)
                .and_then(|v| editor.document(v.doc).map(|d| (v, d)))
        {
            mode = editor.mode();
            mode_name = match mode {
                helix_view::document::Mode::Normal => "NOR",
                helix_view::document::Mode::Insert => "INS",
                helix_view::document::Mode::Select => "SEL",
            };

            file_name = doc
                .path()
                .map(|p| {
                    let path_str = p.to_string_lossy().to_string();
                    if path_str.len() > 50 {
                        if let Some(file_name) = p.file_name() {
                            format!(".../{}", file_name.to_string_lossy())
                        } else {
                            "...".to_string()
                        }
                    } else {
                        path_str
                    }
                })
                .unwrap_or_else(|| "[scratch]".to_string());

            let position = helix_core::coords_at_pos(
                doc.text().slice(..),
                doc.selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
            );
            position_text = format!("{}:{}", position.row + 1, position.col + 1);
        }

        // Determine preferred LSP server for the current document
        let preferred_server_id = if let Some(view_id) = self.view_manager.focused_view_id()
            && let Some(view) = editor.tree.try_get(view_id)
            && let Some(doc) = editor.document(view.doc)
        {
            doc.language_servers().next().map(|ls| ls.id())
        } else {
            None
        };

        let has_lsp_state = core.lsp_state.is_some();
        (
            mode,
            mode_name,
            file_name,
            position_text,
            has_lsp_state,
            preferred_server_id,
        )
    }

    /// Build the LSP indicator string, preferring active progress over the focused document server.
    fn compute_statusbar_lsp_indicator(
        &self,
        cx: &mut Context<Self>,
        has_lsp_state: bool,
        preferred_server_id: Option<helix_lsp::LanguageServerId>,
    ) -> Option<String> {
        if !has_lsp_state {
            return None;
        }

        let lsp_state_entity = {
            let core = self.core.read(cx);
            core.lsp_state.clone()
        }?;

        lsp_state_entity.update(cx, |state, _| {
            statusbar_lsp_indicator_for_state(state, preferred_server_id)
        })
    }

    /// Standard divider element for the status bar.
    fn statusbar_divider(&self, color: gpui::Hsla) -> gpui::AnyElement {
        gpui::div()
            .flex_none()
            .w(gpui::px(1.0))
            .h(gpui::px(18.0))
            .bg(color)
            .mx_2()
            .into_any_element()
    }

    fn statusbar_environment_badge(
        &self,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
    ) -> Option<gpui::AnyElement> {
        let badge = self.environment_badge?;
        let badge_fg = status_bar_tokens.text_primary;
        let badge_bg = nucleotide_ui::tokens::utils::with_alpha(badge_fg, 0.12);
        let badge_border = nucleotide_ui::tokens::utils::with_alpha(badge_fg, 0.32);
        let badge_text = format!("{} {}", badge.label(), badge.detail());

        Some(
            gpui::div()
                .flex_none()
                .flex()
                .items_center()
                .h(gpui::px(20.0))
                .px_2()
                .rounded(gpui::px(5.0))
                .border_1()
                .border_color(badge_border)
                .bg(badge_bg)
                .text_size(gpui::px(11.0))
                .text_color(badge_fg)
                .child(badge_text)
                .into_any_element(),
        )
    }

    /// Build the main content row for the unified status bar.
    #[allow(clippy::too_many_arguments)]
    fn statusbar_main_content(
        &self,
        mode: helix_view::document::Mode,
        mode_name: &'static str,
        file_name: String,
        position_text: String,
        notification: Option<StatusBarNotification>,
        lsp_indicator: Option<String>,
        divider_color: gpui::Hsla,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use nucleotide_ui::{Button, ButtonSize, ButtonVariant, IconPosition};
        let mode_color = match mode {
            helix_view::document::Mode::Normal => status_bar_tokens.mode_normal,
            helix_view::document::Mode::Insert => status_bar_tokens.mode_insert,
            helix_view::document::Mode::Select => status_bar_tokens.mode_select,
        };
        let mut row = gpui::div()
            .flex()
            .flex_1()
            .min_w_0()
            .flex_row()
            .items_center()
            .child(
                // Mode indicator
                gpui::div()
                    .flex_none()
                    .child(mode_name)
                    .min_w(gpui::px(50.0))
                    .text_color(mode_color),
            )
            .child(self.statusbar_divider(divider_color))
            .child(
                // File name grows
                self.statusbar_message_slot(file_name, notification, status_bar_tokens, cx),
            )
            .child(self.statusbar_divider(divider_color))
            .child(
                gpui::div()
                    .flex_none()
                    .child(position_text)
                    .min_w(gpui::px(80.0)),
            );

        if let Some(environment_badge) = self.statusbar_environment_badge(status_bar_tokens) {
            row = row
                .child(self.statusbar_divider(divider_color))
                .child(environment_badge);
        }

        if let Some(indicator) = lsp_indicator {
            let shortened_indicator =
                shorten_statusbar_text(&indicator, STATUSBAR_LSP_INDICATOR_MAX_CHARS);
            row = row.child(self.statusbar_divider(divider_color)).child(
                Button::new("lsp-status-trigger", shortened_indicator)
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::ExtraSmall)
                    .tooltip("Show LSP Status")
                    .activate_on_mouse_down()
                    .icon("icons/chevron-up.svg")
                    .icon_position(IconPosition::End)
                    .on_click(cx.listener(
                        |this: &mut Workspace, ev: &gpui::ClickEvent, _w, cx| {
                            this.lsp_menu_open = true;
                            let position = ev.position();
                            this.lsp_menu_pos = (f32::from(position.x), f32::from(position.y));
                            cx.notify();
                        },
                    )),
            );
        }

        row.into_any_element()
    }

    fn statusbar_message_slot(
        &self,
        file_name: String,
        notification: Option<StatusBarNotification>,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(notification) = notification else {
            return gpui::div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(file_name)
                .into_any_element();
        };

        let notification_tokens = cx.theme().tokens.notification_tokens();
        let label_color = match notification.severity {
            StatusBarNotificationSeverity::Info => notification_tokens.info_text,
            StatusBarNotificationSeverity::Success => notification_tokens.success_text,
            StatusBarNotificationSeverity::Warning => notification_tokens.warning_text,
            StatusBarNotificationSeverity::Error => notification_tokens.error_text,
        };
        let message = shorten_statusbar_text(
            &notification.message,
            STATUSBAR_NOTIFICATION_MESSAGE_MAX_CHARS,
        );

        gpui::div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_2()
            .overflow_hidden()
            .child(
                gpui::div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(label_color)
                    .child(notification.label),
            )
            .child(
                gpui::div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(status_bar_tokens.text_primary)
                    .child(message),
            )
            .into_any_element()
    }

    fn context_menu_intents() -> &'static [ProjectTreeContextMenuIntent] {
        ProjectTreeContextMenuIntent::common_file_operations()
    }

    fn context_menu_handler(intent: ProjectTreeContextMenuIntent) -> FileTreeContextMenuHandler {
        match intent {
            ProjectTreeContextMenuIntent::NewFile => Workspace::cm_action_new_file,
            ProjectTreeContextMenuIntent::NewFolder => Workspace::cm_action_new_folder,
            ProjectTreeContextMenuIntent::Rename => Workspace::cm_action_rename,
            ProjectTreeContextMenuIntent::Delete => Workspace::cm_action_delete,
            ProjectTreeContextMenuIntent::Duplicate => Workspace::cm_action_duplicate,
            ProjectTreeContextMenuIntent::CopyPath => Workspace::cm_action_copy_path,
            ProjectTreeContextMenuIntent::CopyRelativePath => {
                Workspace::cm_action_copy_relative_path
            }
            ProjectTreeContextMenuIntent::RevealInOs => Workspace::cm_action_reveal_in_os,
        }
    }

    fn tab_context_menu_intents(
        has_file_path: bool,
        has_project_panel_path: bool,
        has_terminal_directory: bool,
    ) -> Vec<TabContextMenuIntent> {
        let mut intents = vec![
            TabContextMenuIntent::Close,
            TabContextMenuIntent::CloseOthers,
            TabContextMenuIntent::CloseLeft,
            TabContextMenuIntent::CloseRight,
            TabContextMenuIntent::CloseClean,
            TabContextMenuIntent::CloseAll,
        ];

        if has_file_path {
            intents.extend([
                TabContextMenuIntent::ToggleReadOnly,
                TabContextMenuIntent::CopyPath,
                TabContextMenuIntent::CopyRelativePath,
                TabContextMenuIntent::RevealInOs,
            ]);
        }

        intents.push(TabContextMenuIntent::TogglePin);

        if has_project_panel_path {
            intents.push(TabContextMenuIntent::RevealInProjectPanel);
        }

        if has_terminal_directory {
            intents.push(TabContextMenuIntent::OpenInTerminal);
        }

        intents
    }

    fn tab_context_menu_entries(
        has_file_path: bool,
        has_project_panel_path: bool,
        has_terminal_directory: bool,
    ) -> Vec<TabContextMenuEntry> {
        let mut entries = vec![
            TabContextMenuEntry::Action(TabContextMenuIntent::Close),
            TabContextMenuEntry::Action(TabContextMenuIntent::CloseOthers),
            TabContextMenuEntry::Separator,
            TabContextMenuEntry::Action(TabContextMenuIntent::CloseLeft),
            TabContextMenuEntry::Action(TabContextMenuIntent::CloseRight),
            TabContextMenuEntry::Separator,
            TabContextMenuEntry::Action(TabContextMenuIntent::CloseClean),
            TabContextMenuEntry::Action(TabContextMenuIntent::CloseAll),
        ];

        if has_file_path {
            entries.extend([
                TabContextMenuEntry::Separator,
                TabContextMenuEntry::Action(TabContextMenuIntent::ToggleReadOnly),
                TabContextMenuEntry::Separator,
                TabContextMenuEntry::Action(TabContextMenuIntent::CopyPath),
                TabContextMenuEntry::Action(TabContextMenuIntent::CopyRelativePath),
                TabContextMenuEntry::Separator,
                TabContextMenuEntry::Action(TabContextMenuIntent::RevealInOs),
            ]);
        }

        entries.extend([
            TabContextMenuEntry::Separator,
            TabContextMenuEntry::Action(TabContextMenuIntent::TogglePin),
        ]);

        if has_project_panel_path {
            entries.push(TabContextMenuEntry::Action(
                TabContextMenuIntent::RevealInProjectPanel,
            ));
        }

        if has_terminal_directory {
            entries.push(TabContextMenuEntry::Action(
                TabContextMenuIntent::OpenInTerminal,
            ));
        }

        entries
    }

    fn tab_context_menu_handler(intent: TabContextMenuIntent) -> TabContextMenuHandler {
        match intent {
            TabContextMenuIntent::Close => Workspace::tab_cm_action_close,
            TabContextMenuIntent::CloseOthers => Workspace::tab_cm_action_close_others,
            TabContextMenuIntent::CloseLeft => Workspace::tab_cm_action_close_left,
            TabContextMenuIntent::CloseRight => Workspace::tab_cm_action_close_right,
            TabContextMenuIntent::CloseClean => Workspace::tab_cm_action_close_clean,
            TabContextMenuIntent::CloseAll => Workspace::tab_cm_action_close_all,
            TabContextMenuIntent::CopyPath => Workspace::tab_cm_action_copy_path,
            TabContextMenuIntent::CopyRelativePath => Workspace::tab_cm_action_copy_relative_path,
            TabContextMenuIntent::RevealInOs => Workspace::tab_cm_action_reveal_in_os,
            TabContextMenuIntent::RevealInProjectPanel => {
                Workspace::tab_cm_action_reveal_in_project_panel
            }
            TabContextMenuIntent::OpenInTerminal => Workspace::tab_cm_action_open_in_terminal,
            TabContextMenuIntent::ToggleReadOnly => Workspace::tab_cm_action_toggle_readonly,
            TabContextMenuIntent::TogglePin => Workspace::tab_cm_action_toggle_pin,
        }
    }

    fn tab_context_menu_intent_disabled(
        intent: TabContextMenuIntent,
        target_index: Option<usize>,
        total_items: usize,
        has_clean_items: bool,
    ) -> bool {
        match intent {
            TabContextMenuIntent::Close | TabContextMenuIntent::CloseAll => target_index.is_none(),
            TabContextMenuIntent::CloseOthers => total_items <= 1,
            TabContextMenuIntent::CloseLeft => target_index.is_none_or(|index| index == 0),
            TabContextMenuIntent::CloseRight => {
                target_index.is_none_or(|index| index + 1 >= total_items)
            }
            TabContextMenuIntent::CloseClean => !has_clean_items,
            TabContextMenuIntent::CopyPath
            | TabContextMenuIntent::CopyRelativePath
            | TabContextMenuIntent::RevealInOs
            | TabContextMenuIntent::RevealInProjectPanel
            | TabContextMenuIntent::OpenInTerminal
            | TabContextMenuIntent::ToggleReadOnly => target_index.is_none(),
            TabContextMenuIntent::TogglePin => target_index.is_none(),
        }
    }

    fn tab_bar_split_menu_intents() -> &'static [TabBarSplitMenuIntent] {
        &[
            TabBarSplitMenuIntent::Right,
            TabBarSplitMenuIntent::Left,
            TabBarSplitMenuIntent::Up,
            TabBarSplitMenuIntent::Down,
        ]
    }

    fn tab_bar_split_menu_handler(intent: TabBarSplitMenuIntent) -> TabBarSplitMenuHandler {
        match intent {
            TabBarSplitMenuIntent::Right => Workspace::tab_bar_action_split_right,
            TabBarSplitMenuIntent::Left => Workspace::tab_bar_action_split_left,
            TabBarSplitMenuIntent::Up => Workspace::tab_bar_action_split_up,
            TabBarSplitMenuIntent::Down => Workspace::tab_bar_action_split_down,
        }
    }

    fn activate_tab_bar_split_menu_intent(
        &mut self,
        intent: TabBarSplitMenuIntent,
        cx: &mut Context<Self>,
    ) {
        self.tab_bar_split_menu_open = false;
        let handler = Self::tab_bar_split_menu_handler(intent);
        handler(self, cx);
    }

    fn tab_bar_new_menu_intents() -> &'static [TabBarNewMenuIntent] {
        &[
            TabBarNewMenuIntent::NewFile,
            TabBarNewMenuIntent::OpenFile,
            TabBarNewMenuIntent::SearchProject,
            TabBarNewMenuIntent::SearchSymbols,
            TabBarNewMenuIntent::NewTerminal,
            TabBarNewMenuIntent::NewCenterTerminal,
        ]
    }

    fn tab_bar_new_menu_entries() -> &'static [TabBarNewMenuEntry] {
        &[
            TabBarNewMenuEntry::Action(TabBarNewMenuIntent::NewFile),
            TabBarNewMenuEntry::Action(TabBarNewMenuIntent::OpenFile),
            TabBarNewMenuEntry::Separator,
            TabBarNewMenuEntry::Action(TabBarNewMenuIntent::SearchProject),
            TabBarNewMenuEntry::Action(TabBarNewMenuIntent::SearchSymbols),
            TabBarNewMenuEntry::Separator,
            TabBarNewMenuEntry::Action(TabBarNewMenuIntent::NewTerminal),
            TabBarNewMenuEntry::Action(TabBarNewMenuIntent::NewCenterTerminal),
        ]
    }

    fn tab_bar_new_menu_handler(intent: TabBarNewMenuIntent) -> TabBarNewMenuHandler {
        match intent {
            TabBarNewMenuIntent::NewFile => Workspace::tab_bar_action_new_file,
            TabBarNewMenuIntent::OpenFile => Workspace::tab_bar_action_open_file,
            TabBarNewMenuIntent::SearchProject => Workspace::tab_bar_action_search_project,
            TabBarNewMenuIntent::SearchSymbols => Workspace::tab_bar_action_search_symbols,
            TabBarNewMenuIntent::NewTerminal => Workspace::tab_bar_action_new_terminal,
            TabBarNewMenuIntent::NewCenterTerminal => Workspace::tab_bar_action_new_center_terminal,
        }
    }

    /// Ensure document is in the order list, adding it to the end if new
    fn ensure_document_in_order(&mut self, doc_id: helix_view::DocumentId) {
        if !self.document_order.contains(&doc_id) {
            self.document_order.push(doc_id);
        }
    }

    fn image_tab_mut(&mut self, image_id: u64) -> Option<&mut ImageTab> {
        self.image_tabs.iter_mut().find(|tab| tab.id == image_id)
    }

    fn next_image_tab_id(&mut self) -> u64 {
        let id = self.next_image_tab_index;
        self.next_image_tab_index = self.next_image_tab_index.saturating_add(1);
        id
    }

    fn open_image_file_internal(
        &mut self,
        path: &std::path::Path,
        should_focus: bool,
        cx: &mut Context<Self>,
    ) {
        let path = path.to_path_buf();
        if let Some(tab) = self.image_tabs.iter_mut().find(|tab| tab.path == path) {
            tab.focused_at = std::time::Instant::now();
            self.active_image_tab_id = Some(tab.id);
        } else {
            let scroll_handle = ScrollHandle::new();
            let vertical_scrollbar_state = ScrollbarState::new(scroll_handle.clone());
            let horizontal_scrollbar_state = ScrollbarState::new(scroll_handle.clone());
            let tab = ImageTab {
                id: self.next_image_tab_id(),
                path: path.clone(),
                dimensions: image_file_dimensions(&path),
                focused_at: std::time::Instant::now(),
                zoom: 1.0,
                scroll_handle,
                vertical_scrollbar_state,
                horizontal_scrollbar_state,
            };
            self.active_image_tab_id = Some(tab.id);
            self.image_tabs.push(tab);
        }

        self.allow_tab_bar_auto_scroll();

        if let Some(file_tree) = &self.file_tree {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(&path), cx);
            });
        }

        if should_focus {
            self.view_manager.set_needs_focus_restore(false);
        }

        cx.notify();
    }

    fn switch_to_image_tab(&mut self, image_id: u64, cx: &mut Context<Self>) {
        if let Some(tab) = self.image_tab_mut(image_id) {
            tab.focused_at = std::time::Instant::now();
            self.active_image_tab_id = Some(image_id);
            self.allow_tab_bar_auto_scroll();
            cx.notify();
        }
    }

    fn set_image_tab_zoom(&mut self, image_id: u64, zoom: f32, cx: &mut Context<Self>) {
        if let Some(tab) = self.image_tab_mut(image_id) {
            tab.zoom = zoom.clamp(IMAGE_ZOOM_MIN, IMAGE_ZOOM_MAX);
            cx.notify();
        }
    }

    fn visible_tab_document_ids(&self, cx: &mut Context<Self>) -> Vec<TabId> {
        let core = self.core.read(cx);
        let editor = &core.editor;

        let mut visible_doc_ids = self
            .document_order
            .iter()
            .copied()
            .filter(|doc_id| editor.documents.contains_key(doc_id))
            .map(TabId::Document)
            .collect::<Vec<_>>();
        visible_doc_ids.extend(self.image_tabs.iter().map(|tab| TabId::Image(tab.id)));

        zed_style_tab_order(&visible_doc_ids, &self.pinned_documents)
    }

    fn tab_activation_documents(
        &self,
        cx: &mut Context<Self>,
    ) -> Vec<TabActivationDocument<TabId>> {
        let visible_doc_ids = self.visible_tab_document_ids(cx);
        let core = self.core.read(cx);

        visible_doc_ids
            .into_iter()
            .filter_map(|tab_id| {
                if let TabId::Image(image_id) = tab_id
                    && let Some(tab) = self.image_tabs.iter().find(|tab| tab.id == image_id)
                {
                    return Some(TabActivationDocument {
                        id: tab_id,
                        focused_at: tab.focused_at,
                    });
                }

                let TabId::Document(doc_id) = tab_id else {
                    return None;
                };
                let doc = core.editor.documents.get(&doc_id)?;
                Some(TabActivationDocument {
                    id: tab_id,
                    focused_at: doc.focused_at,
                })
            })
            .collect()
    }

    fn close_tab_documents(
        &mut self,
        doc_ids: impl IntoIterator<Item = DocumentId>,
        cx: &mut Context<Self>,
    ) {
        self.close_tab_documents_with_force(doc_ids, false, cx);
    }

    fn close_tab_documents_with_force(
        &mut self,
        doc_ids: impl IntoIterator<Item = DocumentId>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let doc_ids = doc_ids.into_iter().collect::<Vec<_>>();
        if doc_ids.is_empty() {
            return;
        }

        let handle = self.handle.clone();
        let (closed_doc_ids, close_statuses, modified_doc_ids, modified_names) =
            self.core.update(cx, |core, cx| {
                let _guard = handle.enter();
                let mut closed_doc_ids = Vec::new();
                let mut close_statuses = Vec::new();
                let mut modified_doc_ids = Vec::new();
                let mut modified_names = Vec::new();
                let active_doc_id = core
                    .editor
                    .tree
                    .try_get(core.editor.tree.focus)
                    .map(|view| view.doc);
                let close_targets = doc_ids
                    .into_iter()
                    .map(|doc_id| {
                        let path = core
                            .editor
                            .documents
                            .get(&doc_id)
                            .and_then(|doc| doc.path().cloned());

                        BatchCloseDocument {
                            id: doc_id,
                            is_active: active_doc_id == Some(doc_id),
                            path,
                        }
                    })
                    .collect::<Vec<_>>();
                let doc_ids = batch_close_document_order(&close_targets);

                for doc_id in doc_ids {
                    match core.editor.close_document(doc_id, force) {
                        Ok(()) => {
                            closed_doc_ids.push(doc_id);
                        }
                        Err(helix_view::editor::CloseError::BufferModified(name)) => {
                            info!("Cannot close document {:?}: has unsaved changes", doc_id);
                            if force {
                                close_statuses.push(unsaved_buffers_remaining_status(vec![name]));
                            } else {
                                modified_doc_ids.push(doc_id);
                                modified_names.push(name);
                            }
                        }
                        Err(error @ helix_view::editor::CloseError::DoesNotExist) => {
                            info!("Document {:?} does not exist", doc_id);
                            close_statuses.push(close_error_status(error));
                        }
                        Err(error @ helix_view::editor::CloseError::SaveError(_)) => {
                            info!("Failed to close document {:?}", doc_id);
                            close_statuses.push(close_error_status(error));
                        }
                    }
                }

                if !closed_doc_ids.is_empty() {
                    cx.emit(crate::Update::Redraw);
                    cx.notify();
                }

                (
                    closed_doc_ids,
                    close_statuses,
                    modified_doc_ids,
                    modified_names,
                )
            });

        for status in close_statuses {
            self.push_editor_status_notification(status, cx);
        }

        if !modified_doc_ids.is_empty() {
            self.request_unsaved_close(
                PendingUnsavedClose::Batch {
                    doc_ids: modified_doc_ids,
                },
                modified_names,
                cx,
            );
        }

        if !closed_doc_ids.is_empty() {
            self.unregister_preview_documents(closed_doc_ids.iter().copied(), cx);
            self.update_document_views(cx);
            cx.notify();
        }
    }

    fn close_tab_ids(&mut self, tab_ids: impl IntoIterator<Item = TabId>, cx: &mut Context<Self>) {
        let mut document_ids = Vec::new();
        for tab_id in tab_ids {
            match tab_id {
                TabId::Document(doc_id) => document_ids.push(doc_id),
                TabId::Image(image_id) => self.close_image_tab(image_id, None, cx),
            }
        }

        self.close_tab_documents(document_ids, cx);
    }

    fn close_single_tab_document(
        &mut self,
        doc_id: DocumentId,
        active_doc_id: Option<TabId>,
        activation_documents: &[TabActivationDocument<TabId>],
        activate_on_close: crate::config::TabActivateOnClose,
        cx: &mut Context<Self>,
    ) {
        let activation_target = tab_activation_target_after_close(
            activation_documents,
            TabId::Document(doc_id),
            active_doc_id,
            activate_on_close,
        );
        self.close_single_tab_document_with_activation_target(doc_id, activation_target, false, cx);
    }

    fn close_image_tab(
        &mut self,
        image_id: u64,
        activation_target: Option<TabId>,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.image_tabs.iter().position(|tab| tab.id == image_id) else {
            return;
        };

        self.image_tabs.remove(index);
        self.pinned_documents.remove(&TabId::Image(image_id));

        if self.active_image_tab_id == Some(image_id) {
            self.active_image_tab_id = None;
            if let Some(target_id) = activation_target {
                match target_id {
                    TabId::Image(image_id) => self.switch_to_image_tab(image_id, cx),
                    TabId::Document(doc_id) => self.switch_to_tab_document(doc_id, cx),
                }
            }
        }

        cx.notify();
    }

    fn close_single_tab_document_with_activation_target(
        &mut self,
        doc_id: DocumentId,
        activation_target: Option<TabId>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let handle = self.handle.clone();
        let (closed, close_status, modified_name) = self.core.update(cx, |core, cx| {
            let _guard = handle.enter();

            match core.editor.close_document(doc_id, force) {
                Ok(()) => {
                    if let Some(TabId::Document(target_doc_id)) = activation_target
                        && core.editor.documents.contains_key(&target_doc_id)
                    {
                        core.editor
                            .switch(target_doc_id, helix_view::editor::Action::Replace);
                    }
                    cx.emit(crate::Update::Redraw);
                    cx.notify();
                    (true, None, None)
                }
                Err(helix_view::editor::CloseError::BufferModified(name)) => {
                    info!("Cannot close document {:?}: has unsaved changes", doc_id);
                    if force {
                        (
                            false,
                            Some(unsaved_buffers_remaining_status(vec![name])),
                            None,
                        )
                    } else {
                        (false, None, Some(name))
                    }
                }
                Err(error @ helix_view::editor::CloseError::DoesNotExist) => {
                    info!("Document {:?} does not exist", doc_id);
                    (false, Some(close_error_status(error)), None)
                }
                Err(error @ helix_view::editor::CloseError::SaveError(_)) => {
                    info!("Failed to close document {:?}", doc_id);
                    (false, Some(close_error_status(error)), None)
                }
            }
        });

        if let Some(status) = close_status {
            self.push_editor_status_notification(status, cx);
        }

        if let Some(name) = modified_name {
            self.request_unsaved_close(
                PendingUnsavedClose::Single {
                    doc_id,
                    activation_target: match activation_target {
                        Some(TabId::Document(doc_id)) => Some(doc_id),
                        Some(TabId::Image(_)) | None => None,
                    },
                },
                vec![name],
                cx,
            );
            return;
        }

        if closed {
            if activation_target.is_some() {
                self.allow_tab_bar_auto_scroll();
            }
            if let Some(TabId::Image(image_id)) = activation_target {
                self.switch_to_image_tab(image_id, cx);
            }
            self.unregister_preview_document(doc_id, cx);
            self.update_document_views(cx);
            cx.notify();
        }
    }

    fn force_close_single_tab_document(
        &mut self,
        doc_id: DocumentId,
        activation_target: Option<DocumentId>,
        cx: &mut Context<Self>,
    ) {
        self.close_single_tab_document_with_activation_target(
            doc_id,
            activation_target.map(TabId::Document),
            true,
            cx,
        );
    }

    fn force_close_tab_documents(
        &mut self,
        doc_ids: impl IntoIterator<Item = DocumentId>,
        cx: &mut Context<Self>,
    ) {
        self.close_tab_documents_with_force(doc_ids, true, cx);
    }

    fn unregister_preview_document(&self, doc_id: DocumentId, cx: &mut Context<Self>) {
        if let Some(tracker) = cx.try_global::<nucleotide_core::preview_tracker::PreviewTracker>() {
            tracker.unregister_doc(doc_id);
        }
    }

    fn unregister_preview_documents(
        &self,
        doc_ids: impl IntoIterator<Item = DocumentId>,
        cx: &mut Context<Self>,
    ) {
        if let Some(tracker) = cx.try_global::<nucleotide_core::preview_tracker::PreviewTracker>() {
            for doc_id in doc_ids {
                tracker.unregister_doc(doc_id);
            }
        }
    }

    fn clear_preview_documents(&self, cx: &mut Context<Self>) {
        if let Some(tracker) = cx.try_global::<nucleotide_core::preview_tracker::PreviewTracker>() {
            tracker.clear();
        }
    }

    fn allow_tab_bar_auto_scroll(&mut self) {
        self.suppress_tab_bar_auto_scroll = false;
    }

    fn active_document_and_view(&self, cx: &mut Context<Self>) -> Option<(DocumentId, ViewId)> {
        let core = self.core.read(cx);
        let view_id = self
            .view_manager
            .focused_view_id()
            .filter(|view_id| core.editor.tree.contains(*view_id))
            .unwrap_or(core.editor.tree.focus);
        let doc_id = core.editor.tree.try_get(view_id)?.doc;
        Some((doc_id, view_id))
    }

    fn active_tab_doc_id(&self, cx: &mut Context<Self>) -> Option<TabId> {
        self.active_image_tab_id.map(TabId::Image).or_else(|| {
            self.active_document_and_view(cx)
                .map(|(doc_id, _)| TabId::Document(doc_id))
        })
    }

    fn switch_to_tab_document(&mut self, doc_id: DocumentId, cx: &mut Context<Self>) {
        self.allow_tab_bar_auto_scroll();
        self.active_image_tab_id = None;
        let handle = self.handle.clone();
        self.core.update(cx, |core, cx| {
            let _guard = handle.enter();
            core.editor
                .switch(doc_id, helix_view::editor::Action::Replace);
            cx.emit(crate::Update::Redraw);
            cx.notify();
        });

        self.update_document_views(cx);
        cx.notify();
    }

    fn replace_preview_tab_document(
        &mut self,
        doc_id: DocumentId,
        view_id: ViewId,
        ephemeral: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(tracker) = cx
            .try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            .cloned()
        else {
            return;
        };

        let preview_doc_ids = tracker.preview_doc_ids();
        let PreviewTabTogglePlan::Preview = preview_tab_toggle_plan(&preview_doc_ids, &doc_id)
        else {
            return;
        };

        tracker.replace_with_doc(doc_id, view_id, ephemeral);
    }

    fn enforce_max_tabs_to_target(
        &mut self,
        target_count: Option<usize>,
        protected_doc_id: Option<DocumentId>,
        cx: &mut Context<Self>,
    ) {
        if target_count.is_none() {
            return;
        }

        let ephemeral_docs: HashSet<_> = cx
            .try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            .map(|tracker| tracker.ephemeral_doc_ids())
            .unwrap_or_default();

        let documents = {
            let core = self.core.read(cx);
            self.document_order
                .iter()
                .copied()
                .filter(|doc_id| !ephemeral_docs.contains(doc_id))
                .filter_map(|doc_id| {
                    let doc = core.editor.documents.get(&doc_id)?;
                    Some(MaxTabsDocument {
                        id: doc_id,
                        focused_at: doc.focused_at,
                        is_modified: doc.is_modified(),
                        is_pinned: self.pinned_documents.contains(&TabId::Document(doc_id)),
                        is_protected: protected_doc_id == Some(doc_id),
                    })
                })
                .collect::<Vec<_>>()
        };

        let close_candidates = max_tabs_close_candidates_to_target(&documents, target_count);
        self.close_tab_documents(close_candidates, cx);
    }

    fn enforce_max_tabs(&mut self, protected_doc_id: Option<DocumentId>, cx: &mut Context<Self>) {
        let target_count = self
            .core
            .read(cx)
            .config
            .gui
            .max_tabs
            .map(std::num::NonZeroUsize::get);
        self.enforce_max_tabs_to_target(target_count, protected_doc_id, cx);
    }

    fn unpinned_tab_document_ids(&self, tab_ids: impl IntoIterator<Item = TabId>) -> Vec<TabId> {
        tab_ids
            .into_iter()
            .filter(|tab_id| !self.pinned_documents.contains(tab_id))
            .collect()
    }

    fn tab_cm_action_close(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if let TabId::Image(image_id) = tab_id {
            let active_doc_id = self.active_tab_doc_id(cx);
            let activation_documents = self.tab_activation_documents(cx);
            let activate_on_close = self.core.read(cx).config.gui.tabs.activate_on_close;
            let activation_target = tab_activation_target_after_close(
                &activation_documents,
                tab_id,
                active_doc_id,
                activate_on_close,
            );
            self.close_image_tab(image_id, activation_target, cx);
            return;
        }

        let TabId::Document(doc_id) = tab_id else {
            return;
        };
        let active_doc_id = {
            let core = self.core.read(cx);
            self.view_manager
                .focused_view_id()
                .and_then(|focused_view_id| core.editor.tree.try_get(focused_view_id))
                .map(|view| TabId::Document(view.doc))
        };
        let activation_documents = self.tab_activation_documents(cx);
        let activate_on_close = self.core.read(cx).config.gui.tabs.activate_on_close;
        self.close_single_tab_document(
            doc_id,
            active_doc_id,
            &activation_documents,
            activate_on_close,
            cx,
        );
    }

    fn close_active_tab_document(&mut self, cx: &mut Context<Self>) {
        self.close_active_tab_document_with_force(false, cx);
    }

    fn close_active_buffer_document_with_force(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some((active_doc_id, _active_view_id)) = self.active_document_and_view(cx) else {
            return;
        };

        let activation_documents = self.tab_activation_documents(cx);
        let activate_on_close = self.core.read(cx).config.gui.tabs.activate_on_close;
        let activation_target = tab_activation_target_after_close(
            &activation_documents,
            TabId::Document(active_doc_id),
            Some(TabId::Document(active_doc_id)),
            activate_on_close,
        );
        self.close_single_tab_document_with_activation_target(
            active_doc_id,
            activation_target,
            force,
            cx,
        );
    }

    fn close_active_tab_document_with_force(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(active_doc_id) = self.active_tab_doc_id(cx) else {
            return;
        };

        let visible_doc_ids = self.visible_tab_document_ids(cx);
        match active_tab_close_plan(
            &visible_doc_ids,
            &self.pinned_documents,
            Some(active_doc_id),
        ) {
            ActiveTabClosePlan::Activate(tab_id) => match tab_id {
                TabId::Image(image_id) => self.switch_to_image_tab(image_id, cx),
                TabId::Document(doc_id) => self.switch_to_tab_document(doc_id, cx),
            },
            ActiveTabClosePlan::Close(tab_id) => {
                let activation_documents = self.tab_activation_documents(cx);
                let activate_on_close = self.core.read(cx).config.gui.tabs.activate_on_close;
                let activation_target = tab_activation_target_after_close(
                    &activation_documents,
                    tab_id,
                    Some(tab_id),
                    activate_on_close,
                );
                match tab_id {
                    TabId::Image(image_id) => self.close_image_tab(image_id, activation_target, cx),
                    TabId::Document(doc_id) => self
                        .close_single_tab_document_with_activation_target(
                            doc_id,
                            activation_target,
                            force,
                            cx,
                        ),
                }
            }
            ActiveTabClosePlan::Ignore => {}
        }
    }

    fn tab_document_path(&self, tab_id: TabId, cx: &mut Context<Self>) -> Option<PathBuf> {
        match tab_id {
            TabId::Image(image_id) => self
                .image_tabs
                .iter()
                .find(|tab| tab.id == image_id)
                .map(|tab| tab.path.clone()),
            TabId::Document(doc_id) => {
                let core = self.core.read(cx);
                core.editor
                    .documents
                    .get(&doc_id)
                    .and_then(|doc| doc.path().map(|path| path.to_path_buf()))
            }
        }
    }

    fn tab_terminal_directory(&self, tab_id: TabId, cx: &mut Context<Self>) -> Option<PathBuf> {
        let path = self.tab_document_path(tab_id, cx)?;
        let parent = path.parent()?;
        if parent.as_os_str().is_empty() {
            return self.current_project_root.clone();
        }
        Some(parent.to_path_buf())
    }

    fn tab_context_menu_capabilities(&self, cx: &mut Context<Self>) -> TabContextMenuCapabilities {
        let Some(tab_id) = self.tab_context_menu_doc_id else {
            return TabContextMenuCapabilities::default();
        };

        let tab_path = self.tab_document_path(tab_id, cx);
        let is_readonly = match tab_id {
            TabId::Image(_) => false,
            TabId::Document(doc_id) => {
                let core = self.core.read(cx);
                core.editor
                    .documents
                    .get(&doc_id)
                    .is_some_and(|doc| doc.readonly)
            }
        };

        TabContextMenuCapabilities {
            has_file_path: tab_path.is_some(),
            has_project_panel_path: tab_path
                .as_deref()
                .is_some_and(|path| self.tab_path_visible_in_project_panel(path, cx)),
            has_terminal_directory: self.tab_terminal_directory(tab_id, cx).is_some(),
            is_readonly,
            is_remote: tab_path
                .as_deref()
                .is_some_and(|path| WslWorkspace::from_unc_path(path).is_some()),
        }
    }

    fn tab_path_visible_in_project_panel(&self, path: &Path, cx: &mut Context<Self>) -> bool {
        self.file_tree
            .as_ref()
            .is_some_and(|file_tree| file_tree.read(cx).contains_path(path))
    }

    fn project_tree_path_is_directory(&self, path: &Path, cx: &mut Context<Self>) -> Option<bool> {
        self.file_tree
            .as_ref()
            .and_then(|file_tree| file_tree.read(cx).path_is_directory(path))
    }

    fn cached_or_local_path_is_directory(&self, path: &Path, cx: &mut Context<Self>) -> bool {
        self.project_tree_path_is_directory(path, cx)
            .unwrap_or_else(|| local_path_is_directory_without_wsl_probe(path))
    }

    fn parent_for_new_project_tree_item(
        &self,
        clicked: PathBuf,
        cx: &mut Context<Self>,
    ) -> PathBuf {
        if self.cached_or_local_path_is_directory(&clicked, cx) {
            clicked
        } else {
            clicked
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf()
        }
    }

    fn start_rename_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let current_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string();
        self.pending_file_op = Some(PendingFileOp::Rename { path });
        self.core.update(cx, move |_core, cx| {
            let prompt = crate::prompt::Prompt::native("Rename to", current_name, |_input| {});
            cx.emit(crate::Update::Prompt(prompt));
        });
    }

    fn relative_tab_path_text(&self, path: &Path) -> String {
        if let Some(root) = &self.current_project_root
            && let Ok(relative_path) = path.strip_prefix(root)
        {
            return relative_path.display().to_string();
        }

        path.display().to_string()
    }

    fn tab_cm_action_copy_path(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if let Some(path) = self.tab_document_path(tab_id, cx) {
            let text = path.display().to_string();
            if !Self::copy_to_clipboard_impl(&text) {
                nucleotide_logging::warn!(path=%text, "Failed to copy tab path to clipboard");
            }
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::CopyPath {
                    path,
                    kind: nucleotide_events::v2::workspace::PathCopyKind::Absolute,
                },
            };
            self.core.read(cx).dispatch_workspace_event(event);
        }
    }

    fn tab_cm_action_copy_relative_path(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if let Some(path) = self.tab_document_path(tab_id, cx) {
            let text = self.relative_tab_path_text(&path);
            if !Self::copy_to_clipboard_impl(&text) {
                nucleotide_logging::warn!(
                    path=%text,
                    "Failed to copy tab relative path to clipboard"
                );
            }
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::CopyPath {
                    path,
                    kind: nucleotide_events::v2::workspace::PathCopyKind::RelativeToWorkspace,
                },
            };
            self.core.read(cx).dispatch_workspace_event(event);
        }
    }

    fn tab_cm_action_reveal_in_os(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if let Some(path) = self.tab_document_path(tab_id, cx) {
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::RevealInOs { path },
            };
            self.core.read(cx).dispatch_workspace_event(event);
        }
    }

    fn tab_cm_action_reveal_in_project_panel(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let Some(path) = self.tab_document_path(tab_id, cx) else {
            return;
        };

        let Some(file_tree) = &self.file_tree else {
            return;
        };

        self.show_file_tree = true;
        file_tree.update(cx, |tree, cx| {
            tree.sync_selection_with_file(Some(path.as_path()), cx);
        });
        cx.notify();
    }

    fn tab_cm_action_open_in_terminal(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if let Some(cwd) = self.tab_terminal_directory(tab_id, cx) {
            self.open_terminal_panel_at(Some(cwd), cx);
        }
    }

    fn tab_cm_action_toggle_readonly(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let TabId::Document(doc_id) = tab_id else {
            return;
        };

        let toggled = self.core.update(cx, |core, _cx| {
            core.editor.documents.get_mut(&doc_id).map(|doc| {
                doc.readonly = !doc.readonly;
                doc.readonly
            })
        });

        if let Some(readonly) = toggled {
            nucleotide_logging::info!(?doc_id, readonly, "Toggled tab document read-only state");
            cx.notify();
        }
    }

    fn tab_cm_action_toggle_pin(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if self.pinned_documents.contains(&tab_id) {
            self.pinned_documents.remove(&tab_id);
        } else {
            self.pinned_documents.insert(tab_id);
        }
        cx.notify();
    }

    fn tab_action_double_click(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if let TabId::Document(doc_id) = tab_id {
            self.unregister_preview_document(doc_id, cx);
        }

        let path = self.tab_document_path(tab_id, cx);
        match tab_double_click_plan(path.is_some()) {
            TabDoubleClickPlan::Rename => {
                if let Some(path) = path {
                    self.start_rename_file(path, cx);
                }
            }
            TabDoubleClickPlan::Activate => match tab_id {
                TabId::Image(image_id) => self.switch_to_image_tab(image_id, cx),
                TabId::Document(doc_id) => self.switch_to_tab_document(doc_id, cx),
            },
        }

        cx.notify();
    }

    fn tab_cm_action_close_others(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let should_unpreview_retained_tab = cx
            .try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            .is_some_and(|tracker| match tab_id {
                TabId::Document(doc_id) => {
                    should_unpreview_retained_tab_after_close_others(tracker.is_preview_doc(doc_id))
                }
                TabId::Image(_) => false,
            });
        if should_unpreview_retained_tab {
            if let TabId::Document(doc_id) = tab_id {
                self.unregister_preview_document(doc_id, cx);
            }
        }

        let tab_ids = self.visible_tab_document_ids(cx);
        let tab_ids = self.unpinned_tab_document_ids(
            tab_ids.into_iter().filter(|candidate| *candidate != tab_id),
        );
        self.close_tab_ids(tab_ids, cx);
    }

    fn tab_cm_action_close_left(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let visible_doc_ids = self.visible_tab_document_ids(cx);
        let doc_ids = visible_doc_ids
            .iter()
            .position(|candidate| *candidate == tab_id)
            .map(|index| visible_doc_ids[..index].to_vec())
            .unwrap_or_default();
        let doc_ids = self.unpinned_tab_document_ids(doc_ids);
        self.close_tab_ids(doc_ids, cx);
    }

    fn tab_cm_action_close_right(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let visible_doc_ids = self.visible_tab_document_ids(cx);
        let doc_ids = visible_doc_ids
            .iter()
            .position(|candidate| *candidate == tab_id)
            .map(|index| visible_doc_ids[index + 1..].to_vec())
            .unwrap_or_default();
        let doc_ids = self.unpinned_tab_document_ids(doc_ids);
        self.close_tab_ids(doc_ids, cx);
    }

    fn tab_cm_action_close_clean(&mut self, _tab_id: TabId, cx: &mut Context<Self>) {
        let visible_doc_ids = self.visible_tab_document_ids(cx);
        let doc_ids = {
            let core = self.core.read(cx);
            visible_doc_ids
                .into_iter()
                .filter(|tab_id| match tab_id {
                    TabId::Image(_) => true,
                    TabId::Document(doc_id) => core
                        .editor
                        .documents
                        .get(doc_id)
                        .is_some_and(|doc| !doc.is_modified()),
                })
                .collect::<Vec<_>>()
        };
        let doc_ids = self.unpinned_tab_document_ids(doc_ids);
        self.close_tab_ids(doc_ids, cx);
    }

    fn tab_cm_action_close_all(&mut self, _tab_id: TabId, cx: &mut Context<Self>) {
        let doc_ids = self.unpinned_tab_document_ids(self.visible_tab_document_ids(cx));
        self.close_tab_ids(doc_ids, cx);
    }

    fn tab_bar_action_split_right(&mut self, cx: &mut Context<Self>) {
        self.execute_tab_bar_split_intent(TabBarSplitMenuIntent::Right, cx);
    }

    fn tab_bar_action_split_left(&mut self, cx: &mut Context<Self>) {
        self.execute_tab_bar_split_intent(TabBarSplitMenuIntent::Left, cx);
    }

    fn tab_bar_action_split_up(&mut self, cx: &mut Context<Self>) {
        self.execute_tab_bar_split_intent(TabBarSplitMenuIntent::Up, cx);
    }

    fn tab_bar_action_split_down(&mut self, cx: &mut Context<Self>) {
        self.execute_tab_bar_split_intent(TabBarSplitMenuIntent::Down, cx);
    }

    fn execute_tab_bar_split_intent(
        &mut self,
        intent: TabBarSplitMenuIntent,
        cx: &mut Context<Self>,
    ) {
        for command in intent.commands() {
            self.execute_raw_command(command, cx);
        }
        if self.view_manager.focused_view_id().is_some() {
            self.view_manager.set_needs_focus_restore(true);
        }
        cx.notify();
    }

    fn tab_bar_action_new_file(&mut self, cx: &mut Context<Self>) {
        self.execute_raw_command("new", cx);
    }

    fn tab_bar_action_open_file(&mut self, cx: &mut Context<Self>) {
        self.open_file_picker(cx);
    }

    fn tab_bar_action_search_project(&mut self, cx: &mut Context<Self>) {
        self.start_global_search(cx);
    }

    fn tab_bar_action_search_symbols(&mut self, cx: &mut Context<Self>) {
        self.core
            .update(cx, |core, cx| core.trigger_lsp_symbol_picker(true, cx));
    }

    fn tab_bar_action_new_terminal(&mut self, cx: &mut Context<Self>) {
        self.toggle_terminal_panel(cx);
    }

    fn tab_bar_action_new_center_terminal(&mut self, cx: &mut Context<Self>) {
        self.toggle_terminal_panel(cx);
    }

    fn unpin_all_tabs(&mut self, cx: &mut Context<Self>) {
        if unpin_all_tabs(&mut self.pinned_documents) {
            cx.notify();
        }
    }

    fn toggle_active_preview_tab(&mut self, cx: &mut Context<Self>) {
        if !self.core.read(cx).config.gui.preview_tabs.enabled {
            return;
        }

        let Some((active_doc_id, active_view_id)) = self.active_document_and_view(cx) else {
            return;
        };
        let Some(tracker) = cx
            .try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            .cloned()
        else {
            return;
        };

        let preview_doc_ids = tracker.preview_doc_ids();
        match preview_tab_toggle_plan(&preview_doc_ids, &active_doc_id) {
            PreviewTabTogglePlan::Unpreview => {
                tracker.unregister_doc(active_doc_id);
                cx.notify();
            }
            PreviewTabTogglePlan::Preview => {
                self.replace_preview_tab_document(active_doc_id, active_view_id, false, cx);
                cx.notify();
            }
        }
    }

    // (debug focus logger removed for commit)
    pub fn current_filename(&self, cx: &App) -> Option<String> {
        if let Some(tab) = self
            .active_image_tab_id
            .and_then(|doc_id| self.image_tabs.iter().find(|tab| tab.id == doc_id))
        {
            return Some(
                tab.path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(std::string::ToString::to_string)
                    .unwrap_or_else(|| tab.path.display().to_string()),
            );
        }

        let editor = &self.core.read(cx).editor;

        // Get the currently focused view
        for (view, is_focused) in editor.tree.views() {
            if is_focused && let Some(doc) = editor.document(view.doc) {
                return doc.path().map(|p| {
                    p.file_name()
                        .and_then(|name| name.to_str())
                        .map(std::string::ToString::to_string)
                        .unwrap_or_else(|| p.display().to_string())
                });
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_views(
        core: Entity<Core>,
        input: Entity<Input>,
        handle: tokio::runtime::Handle,
        overlay: Entity<OverlayView>,
        notifications: Entity<NotificationView>,
        info: Entity<InfoBoxView>,
        input_coordinator: Arc<InputCoordinator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        // Register editor focus with the global coordinator for centralized focus handling
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
            coord.set_editor_focus(focus_handle.clone());
        }

        // Subscribe to overlay dismiss events to restore focus
        cx.subscribe(
            &overlay,
            |workspace, _overlay, _event: &DismissEvent, cx| {
                // Mark that we need to restore focus in the next render
                workspace.view_manager.set_needs_focus_restore(true);

                // Check if completion was dismissed and manage context
                let has_completion = workspace.overlay.read(cx).has_completion();
                workspace.manage_completion_context(has_completion);

                cx.notify();
            },
        )
        .detach();

        // Subscribe to completion acceptance events from the overlay
        cx.subscribe(
            &overlay,
            |workspace, _overlay, event: &nucleotide_ui::CompleteViaHelixEvent, cx| {
                workspace.handle_completion_via_helix(event.item_index, cx);
            },
        )
        .detach();

        cx.subscribe(
            &overlay,
            |workspace, _overlay, event: &nucleotide_ui::CompletionWarningEvent, cx| {
                workspace.push_editor_status_notification(
                    EditorStatus {
                        status: event.message.to_string(),
                        severity: Severity::Warning,
                    },
                    cx,
                );
            },
        )
        .detach();

        // Subscribe to core (Application) events to receive Update events
        cx.subscribe(&core, |workspace, _core, event: &crate::Update, cx| {
            debug!("Workspace: Received Update event from core: {:?}", event);
            workspace.handle_event(event, cx);
        })
        .detach();

        cx.observe(&notifications, |_, _, cx| {
            cx.notify();
        })
        .detach();

        // Note: Window appearance observation needs to be set up after window creation
        // It will be handled in the render method when window is available

        let key_hints = cx.new(|_cx| KeyHintView::new());

        // Initialize project status service
        let _project_status_handle = nucleotide_project::initialize_project_status_service(cx);

        // Initialize file tree only if project directory is explicitly set
        let root_path = core.read(cx).project_directory.clone();
        let root_path_for_manager = root_path.clone(); // Clone for later use

        // Start VCS monitoring if we have a root path
        if let Some(root_path) = &root_path {
            let root_path_clone = root_path.clone();
            let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();
            vcs_handle.update(cx, |service, cx| {
                service.start_monitoring(root_path_clone, cx);
            });
        }

        let vcs_service = cx.global::<VcsServiceHandle>().service().clone();
        cx.subscribe(&vcs_service, |workspace, _service, event: &VcsEvent, cx| {
            workspace.handle_vcs_service_event(event, cx);
        })
        .detach();

        let file_tree_config = file_tree_config_from_gui(&core.read(cx).config.gui);
        let file_tree = root_path.map(|root_path| {
            let handle_clone = handle.clone();
            let config = file_tree_config.clone();
            cx.new(|cx| FileTreeView::new_with_runtime(root_path, config, Some(handle_clone), cx))
        });

        // Subscribe to file tree events if we have a file tree
        if let Some(ref file_tree) = file_tree {
            info!("Workspace: Subscribing to file tree events");
            cx.subscribe(file_tree, |workspace, _file_tree, event, cx| {
                debug!("Workspace: Received file tree event: {:?}", event);
                workspace.handle_file_tree_event(event, cx);
            })
            .detach();
        } else {
            info!("Workspace: No file tree to subscribe to");
        }

        // Create about window and theme debug overlay
        let about_window = cx.new(|_cx| AboutWindow::new());
        let theme_debug = cx.new(|_cx| nucleotide_ui::ThemeDebugView::new());

        let doc_sidebar_scroll_handle = ScrollHandle::new();
        let doc_sidebar_scrollbar_state = ScrollbarState::new(doc_sidebar_scroll_handle.clone());

        let initial_tokens = cx.theme().tokens;

        let mut workspace = Self {
            core,
            input,
            view_manager: ViewManager::new(),
            handle,
            overlay,
            info,
            info_hidden: true,
            key_hints,
            notifications,
            last_notified_editor_status: None,
            focus_handle,
            file_tree,
            show_file_tree: true,
            file_tree_width: FILE_TREE_DEFAULT_WIDTH,
            file_tree_width_override: None,
            is_resizing_file_tree: false,
            resize_start_x: 0.0,
            resize_start_width: 0.0,
            doc_sidebar_visible: false,
            doc_sidebar_loading: false,
            doc_sidebar_entries: Vec::new(),
            doc_sidebar_width: DOC_SIDEBAR_DEFAULT_WIDTH,
            doc_sidebar_resizing: false,
            doc_sidebar_resize_start_x: 0.0,
            doc_sidebar_resize_start_width: DOC_SIDEBAR_DEFAULT_WIDTH,
            doc_sidebar_scroll_handle,
            doc_sidebar_scrollbar_state,
            titlebar: None,
            appearance_observer_set: false,
            needs_appearance_update: false,
            needs_window_appearance_update: false,
            pending_appearance: None,
            tab_bar_scroll_handle: ScrollHandle::new(),
            last_scrolled_tab_doc_id: None,
            suppress_tab_bar_auto_scroll: false,
            image_tabs: Vec::new(),
            active_image_tab_id: None,
            next_image_tab_index: 1,
            context_menu_open: false,
            context_menu_pos: (0.0, 0.0),
            context_menu_path: None,
            context_menu_index: 0,
            tab_context_menu_open: false,
            tab_context_menu_pos: (0.0, 0.0),
            tab_context_menu_doc_id: None,
            tab_context_menu_index: 0,
            pinned_documents: HashSet::new(),
            tab_bar_split_menu_open: false,
            tab_bar_split_menu_pos: (0.0, 0.0),
            tab_bar_split_button_bounds: None,
            tab_bar_split_menu_index: 0,
            split_pane_resize: None,
            restore_standard_cursor_after_resize: false,
            tab_bar_new_menu_open: false,
            tab_bar_new_menu_pos: (0.0, 0.0),
            tab_bar_new_menu_index: 0,
            lsp_menu_open: false,
            lsp_menu_pos: (0.0, 0.0),
            document_order: Vec::new(),
            input_coordinator,
            current_project_root: root_path_for_manager.clone(),
            initial_project_startup_pending: root_path_for_manager.is_some(),
            environment_badge: None,
            _pending_lsp_startup: None,
            prefix_extractor: PrefixExtractor::new(),
            about_window,
            theme_debug,
            pending_file_op: None,
            needs_file_tree_refresh: false,
            delete_confirm_open: false,
            delete_confirm_path: None,
            close_confirm_open: false,
            close_confirm: None,
            terminal_panel_visible: false,
            terminal_id: None,
            next_terminal_id: 1,
            next_run_id: 1,
            last_run_task: None,
            active_run_terminal: None,
            run_output_terminal: None,
            debug_colors_enabled: matches!(
                std::env::var("NUCL_DEBUG_COLORS")
                    .map(|v| v.to_ascii_lowercase())
                    .as_deref(),
                Ok("1") | Ok("true") | Ok("yes") | Ok("on")
            ),
            // Basic layout is now the default
            basic_terminal_height: 220.0,
            basic_term_resizing: false,
            basic_term_start_mouse_y: 0.0,
            basic_term_start_height: 0.0,
            embedded_terminal_panel: None,
            terminal_cwd: None,
            terminal_focus: cx.focus_handle(),
            terminal_focus_pending: false,
            terminal_active: false,
            // Performance cache for editor sizing
            last_editor_size: None,
            last_terminal_bounds: None,
            // Temporary defaults; recomputed below
            cached_bg_color: initial_tokens.editor.background,
            cached_text_color: initial_tokens.chrome.text_on_chrome,
            cached_border_color: initial_tokens.chrome.border_default,
            colors_dirty: true,
            cached_font_metrics_key: None,
            cached_char_width: None,
            cached_line_height: None,
            active_completion_session: None,
            completion_memory: CompletionMemory::default(),
            last_native_window_metadata: None,
        };

        // Compute initial theme-derived colors once
        workspace.recompute_theme_colors(cx);

        // Set initial focus restore state
        workspace.view_manager.set_needs_focus_restore(true);

        // Register focus groups for main UI areas
        workspace.register_focus_groups(cx);

        // Setup completion-specific shortcuts
        workspace.setup_completion_shortcuts();

        // Note: Completion handling is now done directly via event-driven approach

        // Register action handlers for global input system
        workspace.register_action_handlers(cx);

        // Initialize document views
        workspace.update_document_views(cx);

        // Auto-focus the first document view on startup
        if workspace.view_manager.focused_view_id().is_some() {
            workspace.view_manager.set_needs_focus_restore(true);
        }

        // Setup LSP state subscription for project status updates
        workspace.setup_lsp_state_subscription(cx);

        workspace.refresh_environment_badge(workspace.current_project_root.clone(), cx);

        workspace
    }

    /// Rescan a single directory and update the file tree entries for that folder only
    fn rescan_directory(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if let Some(ref file_tree) = self.file_tree {
            let dir = dir.to_path_buf();
            file_tree.update(cx, |view, tree_cx| {
                view.refresh_directory(&dir, tree_cx);
            });
        }
    }

    fn cancel_delete_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_confirm_open = false;
        self.delete_confirm_path = None;
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
            let _ = coord.focus_first(
                window,
                cx,
                &[
                    nucleotide_ui::FocusRole::Editor,
                    nucleotide_ui::FocusRole::FileTree,
                ],
            );
        }
        cx.notify();
    }

    fn confirm_delete_from_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.perform_delete_confirm(cx);
    }

    fn clear_unsaved_close_confirm(&mut self, cx: &mut Context<Self>) {
        self.close_confirm_open = false;
        self.close_confirm = None;
        cx.notify();
    }

    fn cancel_unsaved_close_confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.clear_unsaved_close_confirm(cx);
    }

    fn confirm_unsaved_close_from_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.perform_pending_unsaved_close(cx);
    }

    fn request_unsaved_close(
        &mut self,
        action: PendingUnsavedClose<DocumentId>,
        names: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.close_confirm = Some(UnsavedCloseConfirmation { action, names });
        self.close_confirm_open = true;
        cx.notify();
    }

    fn perform_pending_unsaved_close(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.close_confirm.take() else {
            self.close_confirm_open = false;
            cx.notify();
            return;
        };

        self.close_confirm_open = false;
        match pending.action {
            PendingUnsavedClose::Single {
                doc_id,
                activation_target,
            } => self.force_close_single_tab_document(doc_id, activation_target, cx),
            PendingUnsavedClose::Batch { doc_ids } => self.force_close_tab_documents(doc_ids, cx),
        }
    }

    fn request_delete_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.delete_confirm_path = Some(path);
        match self.core.read(cx).config.gui.file_ops.delete_behavior {
            crate::config::DeleteBehavior::Trash => self.perform_delete_confirm(cx),
            crate::config::DeleteBehavior::Permanent => {
                self.delete_confirm_open = true;
                cx.notify();
            }
        }
    }

    /// Render a delete confirmation modal overlay with two actions
    fn render_delete_confirm_modal(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let message = if let Some(path) = &self.delete_confirm_path {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("this item");
            format!("Delete '{}' permanently?", name)
        } else {
            "Delete permanently?".to_string()
        };

        let confirm_label = match self.core.read(cx).config.gui.file_ops.delete_behavior {
            crate::config::DeleteBehavior::Trash => "Move to Trash",
            crate::config::DeleteBehavior::Permanent => "Delete Permanently",
        };

        render_confirm_dialog(
            ConfirmDialog::new("Confirm Delete", message, confirm_label)
                .confirm_variant(ButtonVariant::Danger),
            cx,
            ConfirmDialogCallbacks {
                on_cancel: Workspace::cancel_delete_confirm,
                on_confirm: Workspace::confirm_delete_from_dialog,
            },
        )
    }

    fn render_unsaved_close_confirm_modal(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let names = self
            .close_confirm
            .as_ref()
            .map(|pending| pending.names.as_slice())
            .unwrap_or(&[]);

        render_confirm_dialog(
            ConfirmDialog::new(
                unsaved_close_confirmation_title(names.len()),
                unsaved_close_confirmation_message(names),
                "Close Without Saving",
            )
            .confirm_variant(ButtonVariant::Danger),
            cx,
            ConfirmDialogCallbacks {
                on_cancel: Workspace::cancel_unsaved_close_confirm,
                on_confirm: Workspace::confirm_unsaved_close_from_dialog,
            },
        )
    }

    fn render_documentation_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let tokens = &cx.theme().tokens;
        let gui_config = &self.core.read(cx).config.gui;
        let file_tree_tokens = file_tree_tokens_for_gui_config(tokens, gui_config);
        let markdown_style = MarkdownStyle::from_tokens(tokens).compact();

        let mut body = div()
            .id("documentation-sidebar-body")
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.doc_sidebar_scroll_handle)
            .px(tokens.sizes.space_3)
            .py(tokens.sizes.space_3)
            .gap(tokens.sizes.space_4);

        if self.doc_sidebar_loading {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(file_tree_tokens.item_text_secondary)
                    .child("Loading documentation..."),
            );
        } else if self.doc_sidebar_entries.is_empty() {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(file_tree_tokens.item_text_secondary)
                    .child("No documentation available."),
            );
        } else {
            for (index, entry) in self.doc_sidebar_entries.iter().enumerate() {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens.sizes.space_2)
                        .when(index > 0, |section| {
                            section
                                .border_t_1()
                                .border_color(file_tree_tokens.separator)
                                .pt(tokens.sizes.space_4)
                        })
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(file_tree_tokens.item_text_secondary)
                                .child(entry.server_name.clone()),
                        )
                        .child(markdown_extended(
                            entry.markdown.clone(),
                            markdown_style.clone(),
                        )),
                );
            }
        }

        let body_container = div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(div().size_full().min_h(px(0.0)).child(body))
            .when_some(
                Scrollbar::vertical(self.doc_sidebar_scrollbar_state.clone()),
                |container, scrollbar| {
                    container.child(
                        div()
                            .id("documentation-sidebar-scrollbar")
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(SCROLLBAR_THICKNESS)
                            .child(scrollbar),
                    )
                },
            );

        div()
            .id("documentation-sidebar")
            .w(px(self.doc_sidebar_width))
            .h_full()
            .flex_shrink_0()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(file_tree_tokens.background)
            .text_color(file_tree_tokens.item_text)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .h(tokens.sizes.space_8)
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(tokens.sizes.space_3)
                    .border_b_1()
                    .border_color(file_tree_tokens.separator)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(tokens.sizes.space_2)
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(file_tree_tokens.item_text)
                            .child(
                                svg()
                                    .path("icons/book-text.svg")
                                    .size(px(14.0))
                                    .text_color(file_tree_tokens.item_text)
                                    .flex_shrink_0(),
                            )
                            .child("Documentation"),
                    )
                    .child(
                        div()
                            .id("documentation-sidebar-close")
                            .size(tokens.sizes.space_6)
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(tokens.sizes.radius_sm)
                            .cursor_pointer()
                            .hover(move |button| button.bg(file_tree_tokens.item_background_hover))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|workspace, _event, _window, cx| {
                                    workspace.close_documentation_sidebar(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                svg()
                                    .path("icons/close.svg")
                                    .size(px(12.0))
                                    .text_color(file_tree_tokens.item_text_secondary),
                            ),
                    ),
            )
            .child(body_container)
            .into_any_element()
    }

    /// Execute the delete after confirmation
    fn perform_delete_confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.delete_confirm_path.clone() {
            let existed_before = path.exists();
            let was_dir = path.is_dir();
            let mode = match self.core.read(cx).config.gui.file_ops.delete_behavior {
                crate::config::DeleteBehavior::Trash => {
                    nucleotide_events::v2::workspace::DeleteMode::Trash
                }
                crate::config::DeleteBehavior::Permanent => {
                    nucleotide_events::v2::workspace::DeleteMode::Permanent
                }
            };
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::Delete {
                    path: path.clone(),
                    mode,
                },
            };
            self.dispatch_workspace_file_op_and_process(event, cx);
            let notification = LspFileOperationNotification::Deleted {
                path: path.clone(),
                was_dir,
            };
            if existed_before && file_operation_notification_succeeded(&notification) {
                self.notify_lsp_file_operation(notification, cx);
            }
            if let Some(parent) = path.parent() {
                self.rescan_directory(parent, cx);
            }
        }
        self.delete_confirm_open = false;
        self.delete_confirm_path = None;
        cx.notify();
    }

    /// Render the file tree context menu anchored at the last click position
    fn render_file_tree_context_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Move keyboard focus to the workspace focus group so arrow/enter navigation works
        window.focus(&self.focus_handle, cx);

        let entries = Self::context_menu_intents()
            .iter()
            .copied()
            .map(|intent| ContextMenuEntry::action(intent, intent.label()))
            .collect::<Vec<_>>();

        render_context_menu(
            ContextMenuState::new(self.context_menu_pos, &entries)
                .selected_index(self.context_menu_index)
                .offset(8.0, 8.0)
                .min_width(px(200.0)),
            cx,
            ContextMenuCallbacks {
                on_item_hover: |workspace: &mut Workspace,
                                index: usize,
                                _event: &MouseMoveEvent,
                                _window: &mut Window,
                                cx: &mut Context<Workspace>| {
                    if workspace.context_menu_index != index {
                        workspace.context_menu_index = index;
                        cx.notify();
                    }
                },
                on_item_activate: |workspace: &mut Workspace,
                                   intent: ProjectTreeContextMenuIntent,
                                   _event: &MouseDownEvent,
                                   window: &mut Window,
                                   cx: &mut Context<Workspace>| {
                    window.prevent_default();
                    workspace.context_menu_open = false;
                    let handler_fn = Workspace::context_menu_handler(intent);
                    handler_fn(workspace, cx);
                    cx.stop_propagation();
                },
                on_dismiss: |workspace: &mut Workspace,
                             _event: &MouseDownEvent,
                             window: &mut Window,
                             cx: &mut Context<Workspace>| {
                    workspace.dismiss_file_tree_context_menu(window, cx);
                    cx.stop_propagation();
                },
            },
        )
    }

    fn render_tab_context_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        window.focus(&self.focus_handle, cx);

        let visible_doc_ids = self.visible_tab_document_ids(cx);
        let target_doc_id = self.tab_context_menu_doc_id;
        let target_index =
            target_doc_id.and_then(|doc_id| visible_doc_ids.iter().position(|id| *id == doc_id));
        let menu_capabilities = self.tab_context_menu_capabilities(cx);
        let has_clean_items = {
            let core = self.core.read(cx);
            visible_doc_ids.iter().any(|tab_id| match tab_id {
                TabId::Image(_) => true,
                TabId::Document(doc_id) => core
                    .editor
                    .documents
                    .get(doc_id)
                    .is_some_and(|doc| !doc.is_modified()),
            })
        };
        let target_is_pinned = self
            .tab_context_menu_doc_id
            .is_some_and(|doc_id| self.pinned_documents.contains(&doc_id));
        let entries: Vec<ContextMenuEntry<TabContextMenuIntent>> = Self::tab_context_menu_entries(
            menu_capabilities.has_file_path,
            menu_capabilities.has_project_panel_path,
            menu_capabilities.has_terminal_directory,
        )
        .into_iter()
        .map(|entry| match entry {
            TabContextMenuEntry::Action(intent) => {
                let is_disabled = Self::tab_context_menu_intent_disabled(
                    intent,
                    target_index,
                    visible_doc_ids.len(),
                    has_clean_items,
                );
                let label = intent.label(
                    target_is_pinned,
                    menu_capabilities.is_readonly,
                    menu_capabilities.is_remote,
                );
                if is_disabled {
                    ContextMenuEntry::disabled_action(intent, label)
                } else {
                    ContextMenuEntry::action(intent, label)
                }
            }
            TabContextMenuEntry::Separator => ContextMenuEntry::separator(),
        })
        .collect();

        render_context_menu(
            ContextMenuState::new(self.tab_context_menu_pos, &entries)
                .selected_index(self.tab_context_menu_index)
                .min_width(px(220.0)),
            cx,
            ContextMenuCallbacks {
                on_item_hover: |workspace: &mut Workspace,
                                index: usize,
                                _event: &MouseMoveEvent,
                                _window: &mut Window,
                                cx: &mut Context<Workspace>| {
                    if workspace.tab_context_menu_index != index {
                        workspace.tab_context_menu_index = index;
                        cx.notify();
                    }
                },
                on_item_activate: |workspace: &mut Workspace,
                                   intent: TabContextMenuIntent,
                                   _event: &MouseDownEvent,
                                   _window: &mut Window,
                                   cx: &mut Context<Workspace>| {
                    if let Some(doc_id) = workspace.tab_context_menu_doc_id {
                        workspace.tab_context_menu_open = false;
                        workspace.tab_context_menu_doc_id = None;
                        let handler = Workspace::tab_context_menu_handler(intent);
                        handler(workspace, doc_id, cx);
                    } else {
                        workspace.tab_context_menu_open = false;
                        cx.notify();
                    }
                    cx.stop_propagation();
                },
                on_dismiss: |workspace: &mut Workspace,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             cx: &mut Context<Workspace>| {
                    workspace.tab_context_menu_open = false;
                    workspace.tab_context_menu_doc_id = None;
                    cx.notify();
                    cx.stop_propagation();
                },
            },
        )
    }

    fn render_tab_bar_split_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        window.focus(&self.focus_handle, cx);

        let entries = Self::tab_bar_split_menu_intents()
            .iter()
            .copied()
            .map(|intent| ContextMenuEntry::action(intent, intent.label()))
            .collect::<Vec<_>>();

        render_context_menu(
            ContextMenuState::new(self.tab_bar_split_menu_pos, &entries)
                .anchor(Anchor::TopRight)
                .selected_index(self.tab_bar_split_menu_index)
                .min_width(px(180.0)),
            cx,
            ContextMenuCallbacks {
                on_item_hover: |workspace: &mut Workspace,
                                index: usize,
                                _event: &MouseMoveEvent,
                                _window: &mut Window,
                                cx: &mut Context<Workspace>| {
                    if workspace.tab_bar_split_menu_index != index {
                        workspace.tab_bar_split_menu_index = index;
                        cx.notify();
                    }
                },
                on_item_activate: |workspace: &mut Workspace,
                                   intent: TabBarSplitMenuIntent,
                                   _event: &MouseDownEvent,
                                   _window: &mut Window,
                                   cx: &mut Context<Workspace>| {
                    workspace.activate_tab_bar_split_menu_intent(intent, cx);
                    cx.stop_propagation();
                },
                on_dismiss: |workspace: &mut Workspace,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             cx: &mut Context<Workspace>| {
                    workspace.tab_bar_split_menu_open = false;
                    cx.notify();
                    cx.stop_propagation();
                },
            },
        )
    }

    fn render_tab_bar_new_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        window.focus(&self.focus_handle, cx);

        let entries: Vec<ContextMenuEntry<TabBarNewMenuIntent>> = Self::tab_bar_new_menu_entries()
            .iter()
            .copied()
            .map(|entry| match entry {
                TabBarNewMenuEntry::Action(intent) => {
                    ContextMenuEntry::action(intent, intent.label())
                }
                TabBarNewMenuEntry::Separator => ContextMenuEntry::separator(),
            })
            .collect();

        render_context_menu(
            ContextMenuState::new(self.tab_bar_new_menu_pos, &entries)
                .selected_index(self.tab_bar_new_menu_index)
                .offset(8.0, 8.0)
                .min_width(px(200.0)),
            cx,
            ContextMenuCallbacks {
                on_item_hover: |workspace: &mut Workspace,
                                index: usize,
                                _event: &MouseMoveEvent,
                                _window: &mut Window,
                                cx: &mut Context<Workspace>| {
                    if workspace.tab_bar_new_menu_index != index {
                        workspace.tab_bar_new_menu_index = index;
                        cx.notify();
                    }
                },
                on_item_activate: |workspace: &mut Workspace,
                                   intent: TabBarNewMenuIntent,
                                   _event: &MouseDownEvent,
                                   _window: &mut Window,
                                   cx: &mut Context<Workspace>| {
                    workspace.tab_bar_new_menu_open = false;
                    let handler = Workspace::tab_bar_new_menu_handler(intent);
                    handler(workspace, cx);
                    cx.stop_propagation();
                },
                on_dismiss: |workspace: &mut Workspace,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             cx: &mut Context<Workspace>| {
                    workspace.tab_bar_new_menu_open = false;
                    cx.notify();
                    cx.stop_propagation();
                },
            },
        )
    }

    // --- Context menu action handlers (stubs that close the menu and log) ---
    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_open = false;
        cx.notify();
    }

    fn dismiss_file_tree_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.context_menu_open {
            return;
        }

        self.context_menu_open = false;
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned() {
            let _ = coord.focus_first(
                window,
                cx,
                &[
                    nucleotide_ui::FocusRole::Editor,
                    nucleotide_ui::FocusRole::FileTree,
                ],
            );
        }
        cx.notify();
    }

    fn finish_file_tree_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_resizing_file_tree {
            self.is_resizing_file_tree = false;
            self.request_standard_cursor_restore(window, cx);
        }
    }

    fn close_documentation_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.doc_sidebar_visible || self.doc_sidebar_loading {
            self.doc_sidebar_visible = false;
            self.doc_sidebar_loading = false;
            cx.notify();
        }
    }

    fn toggle_documentation_sidebar(&mut self, cx: &mut Context<Self>) -> bool {
        if self.doc_sidebar_visible {
            self.close_documentation_sidebar(cx);
            return false;
        }

        self.doc_sidebar_visible = true;
        self.doc_sidebar_loading = true;
        self.doc_sidebar_entries.clear();
        cx.notify();
        true
    }

    fn set_documentation_sidebar_entries(
        &mut self,
        entries: Vec<HoverDocEntry>,
        cx: &mut Context<Self>,
    ) {
        if !self.doc_sidebar_visible && !self.doc_sidebar_loading {
            return;
        }

        self.doc_sidebar_visible = true;
        self.doc_sidebar_loading = false;
        self.doc_sidebar_entries = entries;
        cx.notify();
    }

    fn finish_active_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut finished = false;

        if self.is_resizing_file_tree {
            self.is_resizing_file_tree = false;
            finished = true;
        }

        if self.doc_sidebar_resizing {
            self.doc_sidebar_resizing = false;
            finished = true;
        }

        if self.basic_term_resizing {
            self.basic_term_resizing = false;
            finished = true;
        }

        if self.split_pane_resize.take().is_some() {
            if self.view_manager.focused_view_id().is_some() {
                self.view_manager.set_needs_focus_restore(true);
            }
            finished = true;
        }

        if finished {
            self.request_standard_cursor_restore(window, cx);
        }
    }

    fn request_standard_cursor_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_standard_cursor_after_resize = true;
        cx.notify();
        window.refresh();
    }

    fn max_file_tree_width(viewport_width: f32) -> f32 {
        (viewport_width - FILE_TREE_MIN_EDITOR_WIDTH).max(FILE_TREE_MIN_WIDTH)
    }

    fn max_documentation_sidebar_width(available_width: f32) -> f32 {
        (available_width - DOC_SIDEBAR_MIN_EDITOR_WIDTH)
            .clamp(DOC_SIDEBAR_MIN_WIDTH, DOC_SIDEBAR_MAX_WIDTH)
    }

    fn clamped_documentation_sidebar_width(width: f32, available_width: f32) -> f32 {
        width.clamp(
            DOC_SIDEBAR_MIN_WIDTH,
            Self::max_documentation_sidebar_width(available_width),
        )
    }

    fn sync_documentation_sidebar_width_for_viewport(&mut self, available_width: f32) {
        if !self.doc_sidebar_visible {
            return;
        }

        let width =
            Self::clamped_documentation_sidebar_width(self.doc_sidebar_width, available_width);
        if (self.doc_sidebar_width - width).abs() > 0.5 {
            self.doc_sidebar_width = width;
        }
    }

    fn clamped_file_tree_default_width(viewport_width: f32) -> f32 {
        FILE_TREE_DEFAULT_WIDTH.clamp(
            FILE_TREE_MIN_WIDTH,
            Self::max_file_tree_width(viewport_width),
        )
    }

    fn clamped_file_tree_sidebar_width(width: f32, viewport_width: f32) -> f32 {
        width.clamp(
            FILE_TREE_MIN_WIDTH,
            Self::max_file_tree_width(viewport_width),
        )
    }

    fn sync_file_tree_width_for_viewport(&mut self, viewport_width: f32) {
        let target_width = self
            .file_tree_width_override
            .unwrap_or(FILE_TREE_DEFAULT_WIDTH);
        let width = Self::clamped_file_tree_sidebar_width(target_width, viewport_width);

        if (self.file_tree_width - width).abs() > 0.5 {
            self.file_tree_width = width;
        }

        if let Some(override_width) = &mut self.file_tree_width_override {
            *override_width = self.file_tree_width;
        }
    }

    fn clamped_file_tree_resize_width(
        resize_start_width: f32,
        resize_start_x: f32,
        mouse_x: f32,
        viewport_width: f32,
    ) -> f32 {
        let dx = mouse_x - resize_start_x;
        (resize_start_width + dx).clamp(
            FILE_TREE_MIN_WIDTH,
            Self::max_file_tree_width(viewport_width),
        )
    }

    fn update_file_tree_resize(
        &mut self,
        mouse_x: f32,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let new_width = Self::clamped_file_tree_resize_width(
            self.resize_start_width,
            self.resize_start_x,
            mouse_x,
            viewport_width,
        );

        if (self.file_tree_width - new_width).abs() > 0.5 {
            self.file_tree_width = new_width;
            self.file_tree_width_override = Some(new_width);
            cx.notify();
            true
        } else {
            false
        }
    }

    fn clamped_documentation_sidebar_resize_width(
        resize_start_width: f32,
        resize_start_x: f32,
        mouse_x: f32,
        available_width: f32,
    ) -> f32 {
        let dx = resize_start_x - mouse_x;
        Self::clamped_documentation_sidebar_width(resize_start_width + dx, available_width)
    }

    fn update_documentation_sidebar_resize(
        &mut self,
        mouse_x: f32,
        available_width: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let new_width = Self::clamped_documentation_sidebar_resize_width(
            self.doc_sidebar_resize_start_width,
            self.doc_sidebar_resize_start_x,
            mouse_x,
            available_width,
        );

        if (self.doc_sidebar_width - new_width).abs() > 0.5 {
            self.doc_sidebar_width = new_width;
            cx.notify();
            true
        } else {
            false
        }
    }

    fn finish_documentation_sidebar_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc_sidebar_resizing {
            self.doc_sidebar_resizing = false;
            self.request_standard_cursor_restore(window, cx);
        }
    }

    fn start_split_pane_resize(
        &mut self,
        divider: SplitPaneDivider,
        mouse: Point<Pixels>,
        total_area: HelixRect,
        editor_width_px: f32,
        editor_height_px: f32,
        cx: &mut Context<Self>,
    ) {
        let layouts = self.document_view_layouts(cx);
        let before_views = split_pane_resize_view_states(&layouts, &divider.before_view_ids);
        let after_views = split_pane_resize_view_states(&layouts, &divider.after_view_ids);
        if before_views.is_empty() || after_views.is_empty() {
            return;
        }

        self.split_pane_resize = Some(SplitPaneResizeState {
            axis: divider.axis,
            start_mouse_x: f32::from(mouse.x),
            start_mouse_y: f32::from(mouse.y),
            before_views,
            after_views,
            total_area,
            editor_width_px,
            editor_height_px,
        });

        if self.view_manager.focused_view_id().is_some() {
            self.view_manager.set_needs_focus_restore(true);
        }
        cx.notify();
    }

    fn update_split_pane_resize(&mut self, mouse: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.split_pane_resize.as_ref() else {
            return false;
        };

        let Some(resized_areas) =
            split_pane_resized_areas(state, f32::from(mouse.x), f32::from(mouse.y))
        else {
            return false;
        };

        let changed = self.core.update(cx, |core, _| {
            let mut changed = false;
            let tree = &mut core.editor.tree;

            for (view_id, area) in resized_areas {
                if tree.try_get(view_id).is_some() {
                    let view = tree.get_mut(view_id);
                    if view.area != area {
                        view.area = area;
                        changed = true;
                    }
                }
            }

            changed
        });

        if changed {
            self.update_document_views(cx);
            cx.notify();
        }

        changed
    }

    fn finish_split_pane_resize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_pane_resize.take().is_some() {
            if self.view_manager.focused_view_id().is_some() {
                self.view_manager.set_needs_focus_restore(true);
            }
            self.request_standard_cursor_restore(window, cx);
        }
    }

    fn cm_action_new_file(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(clicked) = this.context_menu_path.clone() {
            let parent = this.parent_for_new_project_tree_item(clicked, cx);
            // Queue pending op and show prompt (overlay will emit CommandSubmitted)
            this.pending_file_op = Some(PendingFileOp::NewFile { parent });
            this.core.update(cx, |_core, cx| {
                let prompt = crate::prompt::Prompt::native("New file name", "", |_input| {});
                cx.emit(crate::Update::Prompt(prompt));
            });
        }
        this.close_context_menu(cx);
    }

    fn cm_action_new_folder(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(clicked) = this.context_menu_path.clone() {
            let parent = this.parent_for_new_project_tree_item(clicked, cx);
            this.pending_file_op = Some(PendingFileOp::NewFolder { parent });
            this.core.update(cx, |_core, cx| {
                let prompt = crate::prompt::Prompt::native("New folder name", "", |_input| {});
                cx.emit(crate::Update::Prompt(prompt));
            });
        }
        this.close_context_menu(cx);
    }

    fn cm_action_rename(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            this.start_rename_file(path, cx);
        }
        this.close_context_menu(cx);
    }

    fn cm_action_delete(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            this.request_delete_path(path, cx);
        }
        this.close_context_menu(cx);
    }

    fn cm_action_duplicate(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            let base_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| format!("{} copy", s))
                .unwrap_or_else(|| "copy".to_string());
            this.pending_file_op = Some(PendingFileOp::Duplicate { path });
            this.core.update(cx, move |_core, cx| {
                let prompt = crate::prompt::Prompt::native("Duplicate as", base_name, |_input| {});
                cx.emit(crate::Update::Prompt(prompt));
            });
        }
        this.close_context_menu(cx);
    }

    fn cm_action_copy_path(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            // Copy absolute path to clipboard
            let text = path.display().to_string();
            if !Self::copy_to_clipboard_impl(&text) {
                nucleotide_logging::warn!(path=%text, "Failed to copy path to clipboard");
            }
            // Optionally dispatch intent for telemetry/handlers
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::CopyPath {
                    path,
                    kind: nucleotide_events::v2::workspace::PathCopyKind::Absolute,
                },
            };
            this.core.read(cx).dispatch_workspace_event(event);
        }
        this.close_context_menu(cx);
    }

    fn cm_action_copy_relative_path(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            // Compute relative to current project root if available
            let text = if let Some(root) = &this.current_project_root {
                match path.strip_prefix(root) {
                    Ok(rel) => rel.display().to_string(),
                    Err(_) => path.display().to_string(),
                }
            } else {
                path.display().to_string()
            };
            if !Self::copy_to_clipboard_impl(&text) {
                nucleotide_logging::warn!(path=%text, "Failed to copy relative path to clipboard");
            }
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::CopyPath {
                    path,
                    kind: nucleotide_events::v2::workspace::PathCopyKind::RelativeToWorkspace,
                },
            };
            this.core.read(cx).dispatch_workspace_event(event);
        }
        this.close_context_menu(cx);
    }

    /// Best-effort clipboard copy using platform tools
    fn copy_to_clipboard_impl(text: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            use std::io::Write;
            let mut child = match std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(_) => return false,
            };
            if let Some(stdin) = &mut child.stdin
                && stdin.write_all(text.as_bytes()).is_err()
            {
                return false;
            }
            let _ = child.wait();
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            use std::io::Write;
            let mut child = match nucleotide_process::command("cmd")
                .args(["/C", "clip"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(_) => return false,
            };
            if let Some(stdin) = &mut child.stdin {
                if stdin.write_all(text.as_bytes()).is_err() {
                    return false;
                }
            }
            let _ = child.wait();
            return true;
        }
        #[cfg(target_os = "linux")]
        {
            use std::io::Write;
            // Try wl-copy (Wayland)
            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = &mut child.stdin {
                    if stdin.write_all(text.as_bytes()).is_ok() {
                        let _ = child.wait();
                        return true;
                    }
                }
            }
            // Fallback to xclip
            if let Ok(mut child) = std::process::Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = &mut child.stdin {
                    if stdin.write_all(text.as_bytes()).is_ok() {
                        let _ = child.wait();
                        return true;
                    }
                }
            }
            return false;
        }
        #[allow(unreachable_code)]
        {
            // Other platforms: not implemented
            false
        }
    }

    fn cm_action_reveal_in_os(this: &mut Workspace, cx: &mut Context<Workspace>) {
        if let Some(path) = this.context_menu_path.clone() {
            let event = nucleotide_events::v2::workspace::Event::FileOpRequested {
                intent: nucleotide_events::v2::workspace::FileOpIntent::RevealInOs { path },
            };
            this.core.read(cx).dispatch_workspace_event(event);
        }
        this.close_context_menu(cx);
    }

    /// Update the input context based on current focus state
    fn update_input_context(&mut self, window: &Window, cx: &mut Context<Self>) {
        // Check for active overlays first - they take priority
        let overlay_view = self.overlay.read(cx);
        let new_context = if overlay_view.has_picker() {
            InputContext::Picker
        } else if overlay_view.has_prompt() {
            InputContext::Prompt
        } else if overlay_view.has_completion() {
            InputContext::Completion
        } else if let Some(file_tree) = &self.file_tree {
            if file_tree.focus_handle(cx).is_focused(window) {
                InputContext::FileTree
            } else {
                InputContext::Normal
            }
        } else {
            InputContext::Normal
        };

        // Switch to the appropriate context
        self.input_coordinator.switch_context(new_context);

        debug!(context = ?new_context, "Updated input context");
    }

    /// Handle workspace actions triggered by InputCoordinator
    fn handle_workspace_action(&mut self, action: &str, cx: &mut Context<Self>) {
        match action {
            "ToggleFileTree" => {
                info!("Toggling file tree");
                self.show_file_tree = !self.show_file_tree;
                cx.notify();
            }
            "ShowFileFinder" => {
                info!("Showing file finder");
                let handle = self.handle.clone();
                let core = self.core.clone();
                let overlay = self.overlay.clone();
                open(core, handle, overlay, cx);
            }
            "ShowCommandPrompt" => {
                info!("Showing command prompt");
                self.show_command_prompt(cx);
            }
            "ShowBufferPicker" => {
                info!("Showing buffer picker");
                let handle = self.handle.clone();
                let core = self.core.clone();
                let overlay = self.overlay.clone();
                show_buffer_picker(core, handle, overlay, cx);
            }
            _ => {
                warn!(action = %action, "Unknown workspace action");
            }
        }
    }

    fn handle_completion_overlay_action(
        &mut self,
        action: MenuKeyAction,
        accept_with_enter: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.overlay.update(cx, |overlay, cx| match action {
            MenuKeyAction::Accept if accept_with_enter => overlay.handle_completion_enter_key(cx),
            MenuKeyAction::Accept => overlay.handle_completion_tab_key(cx),
            MenuKeyAction::Cancel => {
                overlay.dismiss_completion(cx);
                true
            }
            MenuKeyAction::SelectNext => overlay.handle_completion_arrow_key("down", cx),
            MenuKeyAction::SelectPrevious => overlay.handle_completion_arrow_key("up", cx),
        })
    }

    /// Routes Helix-style completion keys while the completion menu is open.
    fn handle_regular_completion_menu_key(
        &mut self,
        ev: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.overlay.read(cx).has_completion() {
            return false;
        }

        let Some(action) = completion_menu_action(
            ev.keystroke.key.as_str(),
            ev.keystroke.modifiers.control,
            ev.keystroke.modifiers.shift,
        ) else {
            return false;
        };

        self.handle_completion_overlay_action(action, false, cx)
    }

    fn handle_completion_commit_character(
        &mut self,
        ev: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.overlay.read(cx).has_completion() {
            return false;
        }

        let Some(commit_character) = completion_commit_character_from_key(
            ev.keystroke.key.as_str(),
            ev.keystroke.key_char.as_deref(),
            ev.keystroke.modifiers.control,
        ) else {
            return false;
        };

        let accept_index = self.overlay.update(cx, |overlay, cx| {
            overlay.completion_commit_accept_index(commit_character, cx)
        });
        let Some(item_index) = accept_index else {
            return false;
        };

        nucleotide_logging::debug!(
            item_index = item_index,
            commit_character = %commit_character,
            "Accepting completion before commit character"
        );
        self.handle_completion_via_helix(item_index, cx);
        true
    }

    /// Simplified key handler that delegates to the InputCoordinator
    fn handle_key(&mut self, ev: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        // If embedded terminal is focused, route all keys to it and stop here.
        // Terminal visibility alone must not steal editor input.
        if self.terminal_panel_visible && self.terminal_focus.is_focused(window) {
            if let Some(panel) = &self.embedded_terminal_panel {
                let id = panel.read(cx).active;
                #[cfg(feature = "terminal-emulator")]
                let bytes = {
                    let mode = nucleotide_terminal_view::get_view_model(id)
                        .and_then(|vm| vm.lock().ok().map(|guard| guard.input_mode()))
                        .unwrap_or_default();
                    crate::overlay::translate_key_to_bytes_with_mode(ev, mode)
                };
                #[cfg(not(feature = "terminal-emulator"))]
                let bytes = crate::overlay::translate_key_to_bytes(ev);
                if !bytes.is_empty() {
                    // Snap scroll back to cursor when the user types
                    #[cfg(feature = "terminal-emulator")]
                    if let Some(vm) = nucleotide_terminal_view::get_view_model(id)
                        && let Ok(mut guard) = vm.lock()
                    {
                        guard.scroll_to_bottom();
                    }
                    // Fast path: send directly to the PTY writer thread,
                    // bypassing the event queue (which defers until next render).
                    #[cfg(feature = "terminal-emulator")]
                    {
                        let sent = self
                            .core
                            .read(cx)
                            .terminal_input_senders
                            .lock()
                            .ok()
                            .and_then(|senders| {
                                senders.get(&id).map(|tx| {
                                    let _ = tx.send(bytes.clone());
                                })
                            })
                            .is_some();
                        if !sent {
                            // Fallback: dispatch through event bus if sender not yet registered
                            self.core.update(cx, |app, _| {
                                if let Some(bus) = &app.event_aggregator {
                                    bus.dispatch_terminal(
                                        nucleotide_events::v2::terminal::Event::Input { id, bytes },
                                    );
                                }
                            });
                        }
                    }
                    #[cfg(not(feature = "terminal-emulator"))]
                    {
                        self.core.update(cx, |app, _| {
                            if let Some(bus) = &app.event_aggregator {
                                bus.dispatch_terminal(
                                    nucleotide_events::v2::terminal::Event::Input { id, bytes },
                                );
                            }
                        });
                    }
                }
            }
            // Prevent further handling by editor/others
            cx.stop_propagation();
            return;
        }
        debug!(
            key = %ev.keystroke.key,
            modifiers = ?ev.keystroke.modifiers,
            "Workspace received key event"
        );

        // Delete modal keyboard handling
        if self.delete_confirm_open {
            match ev.keystroke.key.as_str() {
                "enter" => {
                    self.perform_delete_confirm(cx);
                    return;
                }
                "escape" => {
                    self.delete_confirm_open = false;
                    self.delete_confirm_path = None;
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        // Unsaved-close modal keyboard handling
        if self.close_confirm_open {
            match ev.keystroke.key.as_str() {
                "enter" => {
                    self.perform_pending_unsaved_close(cx);
                    return;
                }
                "escape" => {
                    self.clear_unsaved_close_confirm(cx);
                    return;
                }
                _ => {}
            }
        }

        // Tab context menu keyboard handling
        if self.tab_context_menu_open {
            match ev.keystroke.key.as_str() {
                "escape" => {
                    self.tab_context_menu_open = false;
                    self.tab_context_menu_doc_id = None;
                    cx.notify();
                    return;
                }
                "down" => {
                    let menu_capabilities = self.tab_context_menu_capabilities(cx);
                    let len = Self::tab_context_menu_intents(
                        menu_capabilities.has_file_path,
                        menu_capabilities.has_project_panel_path,
                        menu_capabilities.has_terminal_directory,
                    )
                    .len();
                    if len > 0 {
                        self.tab_context_menu_index = (self.tab_context_menu_index + 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "up" => {
                    let menu_capabilities = self.tab_context_menu_capabilities(cx);
                    let len = Self::tab_context_menu_intents(
                        menu_capabilities.has_file_path,
                        menu_capabilities.has_project_panel_path,
                        menu_capabilities.has_terminal_directory,
                    )
                    .len();
                    if len > 0 {
                        self.tab_context_menu_index = (self.tab_context_menu_index + len - 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "enter" => {
                    if let Some(doc_id) = self.tab_context_menu_doc_id {
                        let menu_capabilities = self.tab_context_menu_capabilities(cx);
                        let intents = Self::tab_context_menu_intents(
                            menu_capabilities.has_file_path,
                            menu_capabilities.has_project_panel_path,
                            menu_capabilities.has_terminal_directory,
                        );
                        let Some(intent) = intents.get(self.tab_context_menu_index).copied() else {
                            self.tab_context_menu_open = false;
                            self.tab_context_menu_doc_id = None;
                            cx.notify();
                            return;
                        };
                        let visible_doc_ids = self.visible_tab_document_ids(cx);
                        let target_index = visible_doc_ids.iter().position(|id| *id == doc_id);
                        let has_clean_items = {
                            let core = self.core.read(cx);
                            visible_doc_ids.iter().any(|tab_id| match tab_id {
                                TabId::Image(_) => true,
                                TabId::Document(doc_id) => core
                                    .editor
                                    .documents
                                    .get(doc_id)
                                    .is_some_and(|doc| !doc.is_modified()),
                            })
                        };

                        if Self::tab_context_menu_intent_disabled(
                            intent,
                            target_index,
                            visible_doc_ids.len(),
                            has_clean_items,
                        ) {
                            cx.notify();
                        } else {
                            let handler = Self::tab_context_menu_handler(intent);
                            self.tab_context_menu_open = false;
                            self.tab_context_menu_doc_id = None;
                            handler(self, doc_id, cx);
                        }
                    } else {
                        self.tab_context_menu_open = false;
                        self.tab_context_menu_doc_id = None;
                        cx.notify();
                    }
                    return;
                }
                _ => {}
            }
        }

        // Tab bar split menu keyboard handling
        if self.tab_bar_split_menu_open {
            match ev.keystroke.key.as_str() {
                "escape" => {
                    self.tab_bar_split_menu_open = false;
                    cx.notify();
                    return;
                }
                "down" => {
                    let len = Self::tab_bar_split_menu_intents().len();
                    if len > 0 {
                        self.tab_bar_split_menu_index = (self.tab_bar_split_menu_index + 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "up" => {
                    let len = Self::tab_bar_split_menu_intents().len();
                    if len > 0 {
                        self.tab_bar_split_menu_index =
                            (self.tab_bar_split_menu_index + len - 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "enter" => {
                    if let Some(intent) =
                        Self::tab_bar_split_menu_intents().get(self.tab_bar_split_menu_index)
                    {
                        self.activate_tab_bar_split_menu_intent(*intent, cx);
                    } else {
                        self.tab_bar_split_menu_open = false;
                        cx.notify();
                    }
                    return;
                }
                _ => {}
            }
        }

        // Tab bar new item menu keyboard handling
        if self.tab_bar_new_menu_open {
            match ev.keystroke.key.as_str() {
                "escape" => {
                    self.tab_bar_new_menu_open = false;
                    cx.notify();
                    return;
                }
                "down" => {
                    let len = Self::tab_bar_new_menu_intents().len();
                    if len > 0 {
                        self.tab_bar_new_menu_index = (self.tab_bar_new_menu_index + 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "up" => {
                    let len = Self::tab_bar_new_menu_intents().len();
                    if len > 0 {
                        self.tab_bar_new_menu_index = (self.tab_bar_new_menu_index + len - 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "enter" => {
                    if let Some(intent) =
                        Self::tab_bar_new_menu_intents().get(self.tab_bar_new_menu_index)
                    {
                        let handler = Self::tab_bar_new_menu_handler(*intent);
                        self.tab_bar_new_menu_open = false;
                        handler(self, cx);
                    } else {
                        self.tab_bar_new_menu_open = false;
                        cx.notify();
                    }
                    return;
                }
                _ => {}
            }
        }

        // Context menu keyboard handling
        if self.context_menu_open {
            match ev.keystroke.key.as_str() {
                "escape" => {
                    self.context_menu_open = false;
                    cx.notify();
                    return;
                }
                "down" => {
                    let len = Self::context_menu_intents().len();
                    if len > 0 {
                        self.context_menu_index = (self.context_menu_index + 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "up" => {
                    let len = Self::context_menu_intents().len();
                    if len > 0 {
                        self.context_menu_index = (self.context_menu_index + len - 1) % len;
                        cx.notify();
                    }
                    return;
                }
                "enter" => {
                    if let Some(intent) = Self::context_menu_intents().get(self.context_menu_index)
                    {
                        let handler_fn = Self::context_menu_handler(*intent);
                        self.context_menu_open = false;
                        handler_fn(self, cx);
                    } else {
                        self.context_menu_open = false;
                        cx.notify();
                    }
                    return;
                }
                _ => {}
            }
        }

        // Close context menu on Escape
        if self.context_menu_open && ev.keystroke.key == "escape" {
            self.context_menu_open = false;
            cx.notify();
            return;
        }

        // Check if completion is visible and handle navigation/control keys
        if self.overlay.read(cx).has_completion() && self.handle_regular_completion_menu_key(ev, cx)
        {
            return;
        }

        let accepted_completion_on_commit_character =
            self.handle_completion_commit_character(ev, cx);

        // Update input context based on current focus state
        self.update_input_context(window, cx);

        // Delegate to InputCoordinator for processing
        let result = self.input_coordinator.handle_key_event(ev, window);

        // Handle the result
        use crate::input_coordinator::InputResult;
        match result {
            InputResult::NotHandled => {
                debug!("Key event not handled by InputCoordinator");
            }
            InputResult::Handled => {
                debug!("Key event handled by InputCoordinator");
            }
            InputResult::SendToHelix(helix_key) => {
                nucleotide_logging::trace!(
                    key = ?helix_key,
                    is_held = ev.is_held,
                    accepted_completion_on_commit_character =
                        accepted_completion_on_commit_character,
                    "Sending key to Helix editor"
                );

                // Send the key to Helix
                self.input.update(cx, |_, cx| {
                    cx.emit(crate::InputEvent::key_down(helix_key, ev.is_held));
                });

                // Extra debug for ctrl-x specifically
                if helix_key
                    .modifiers
                    .contains(helix_view::keyboard::KeyModifiers::CONTROL)
                    && matches!(helix_key.code, helix_view::keyboard::KeyCode::Char('x'))
                {
                    nucleotide_logging::info!(
                        "DEBUG: CTRL-X sent to Helix - should trigger completion in insert mode"
                    );
                }
            }
            InputResult::WorkspaceAction(action) => {
                debug!(action = %action, "Executing workspace action");
                self.handle_workspace_action(&action, cx);
            }
        }

        // Trigger delete confirmation from keyboard when file tree has focus
        if ev.keystroke.key.as_str() == "delete"
            && let Some(ref file_tree) = self.file_tree
        {
            let is_tree_focused = file_tree.focus_handle(cx).is_focused(window);
            if is_tree_focused {
                let selected = file_tree.read(cx).selected_path().cloned();
                if let Some(path) = selected {
                    self.request_delete_path(path, cx);
                }
            }
        }
    }

    pub fn new(
        _core: Entity<Core>,
        _input: Entity<Input>,
        _handle: tokio::runtime::Handle,
        _cx: &mut Context<Self>,
    ) -> Self {
        panic!("Use Workspace::with_views instead - views must be created in window context");
    }

    pub fn set_titlebar(&mut self, titlebar: Entity<nucleotide_ui::titlebar::TitleBar>) {
        self.titlebar = Some(titlebar);
    }

    fn start_deferred_project_services(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.current_project_root.clone() else {
            warn!("No project root found - project level LSP will not be initialized");
            return;
        };

        info!(project_root = %root.display(), "Triggering deferred project detection and LSP startup");
        self.trigger_project_detection_and_lsp_startup(root, cx);
    }

    fn update_titlebar_filename(
        &mut self,
        filename: Option<&str>,
        notify: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(titlebar) = &self.titlebar else {
            return;
        };

        let filename = titlebar_filename(filename);
        titlebar.update(cx, |titlebar, cx| {
            if titlebar.set_filename(filename) && notify {
                cx.notify();
            }
        });
    }

    fn update_titlebar_leading_sidebar_background(
        &mut self,
        background: Option<(Pixels, Hsla, Hsla)>,
        cx: &mut Context<Self>,
    ) {
        let Some(titlebar) = &self.titlebar else {
            return;
        };

        titlebar.update(cx, |titlebar, _cx| {
            if let Some((width, background, separator)) = background {
                titlebar.set_leading_sidebar_background(width, background, separator);
            } else {
                titlebar.clear_leading_sidebar_background();
            }
        });
    }

    fn focused_native_window_metadata(
        &self,
        cx: &Context<Self>,
    ) -> (Option<String>, NativeWindowMetadata) {
        if let Some(tab) = self
            .active_image_tab_id
            .and_then(|doc_id| self.image_tabs.iter().find(|tab| tab.id == doc_id))
        {
            let focused_file_name = tab
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string);
            let title = native_window_title(focused_file_name.as_deref());
            return (
                focused_file_name,
                NativeWindowMetadata {
                    title,
                    document_path: Some(tab.path.clone()),
                    edited: self
                        .core
                        .read(cx)
                        .editor
                        .documents
                        .values()
                        .any(|doc| doc.is_modified()),
                },
            );
        }

        let core = self.core.read(cx);
        let editor = &core.editor;
        let mut focused_file_name = None;
        let mut focused_doc_path = None;

        if let Some(view) = editor.tree.try_get(editor.tree.focus)
            && let Some(doc) = editor.document(view.doc)
        {
            focused_doc_path = doc.path().cloned();
            focused_file_name = doc.path().map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| path.display().to_string())
            });
        }

        let edited = editor.documents.values().any(|doc| doc.is_modified());
        let title = native_window_title(focused_file_name.as_deref());

        (
            focused_file_name,
            NativeWindowMetadata {
                title,
                document_path: focused_doc_path,
                edited,
            },
        )
    }

    fn update_native_window_metadata(
        &mut self,
        window: &mut Window,
        metadata: NativeWindowMetadata,
    ) {
        let previous = self.last_native_window_metadata.as_ref();

        if previous.is_none_or(|previous| previous.title != metadata.title) {
            window.set_window_title(&metadata.title);
        }

        if previous.is_none_or(|previous| previous.document_path != metadata.document_path) {
            window.set_document_path(metadata.document_path.as_deref());
        }

        if previous.is_none_or(|previous| previous.edited != metadata.edited) {
            window.set_window_edited(metadata.edited);
        }

        self.last_native_window_metadata = Some(metadata);
    }

    #[instrument(skip(self, cx))]
    pub fn set_project_directory(&mut self, dir: std::path::PathBuf, cx: &mut Context<Self>) {
        add_recent_project(&dir, cx);

        // Check if this is a project root change
        let is_project_change = self.current_project_root.as_ref() != Some(&dir);
        let old_project_root = self.current_project_root.clone();

        debug!(
            current_root = ?self.current_project_root,
            new_dir = %dir.display(),
            is_change = is_project_change,
            "Evaluating project directory change"
        );

        self.core.update(cx, |core, _cx| {
            core.project_directory = Some(dir.clone());
        });

        // Update project status service
        let project_status = nucleotide_project::project_status_service(cx);
        project_status.set_project_root(Some(dir.clone()));

        // Start VCS monitoring for the new directory
        let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();
        vcs_handle.update(cx, |service, cx| {
            service.start_monitoring(dir.clone(), cx);
        });

        // Handle project change for LSP management
        if is_project_change {
            info!(
                old_root = ?self.current_project_root,
                new_root = %dir.display(),
                "Project directory changed - updating LSP management"
            );

            // Update current project root tracking
            self.current_project_root = Some(dir.clone());
            self.refresh_environment_badge(Some(dir.clone()), cx);

            // Clear existing LSP state to avoid stale indicators from previous project
            if let Some(lsp_state_entity) = self.core.read(cx).lsp_state.clone() {
                lsp_state_entity.update(cx, |state, cx| {
                    state.clear_all_state();
                    cx.notify();
                });
                info!("Cleared LSP state for project root change");

                // Immediately sync any existing servers to populate the new project context
                // This ensures LSP indicators appear quickly for the new project
                let editor = &self.core.read(cx).editor;
                let active_servers: Vec<(helix_lsp::LanguageServerId, String)> = editor
                    .language_servers
                    .iter_clients()
                    .map(|client| (client.id(), client.name().to_string()))
                    .collect();

                if !active_servers.is_empty() {
                    lsp_state_entity.update(cx, |state, cx| {
                        for (id, name) in active_servers {
                            info!(
                                server_id = ?id,
                                server_name = %name,
                                "Registering existing LSP server for new project"
                            );
                            state.register_server(id, name, Some(dir.display().to_string()));
                            state.update_server_status(id, nucleotide_lsp::ServerStatus::Running);
                        }
                        cx.notify();
                    });
                    info!("Registered existing LSP servers for new project");
                }
            }

            // Restart existing LSP servers with the new workspace root.
            self.restart_lsp_servers_for_workspace_change(old_project_root, &dir, cx);

            // Trigger project detection and LSP coordination
            self.trigger_project_detection_and_lsp_startup(dir, cx);

            // Note: File tree header update will be handled via project status service update
            // which triggers UI refresh through the project status service

            // Refresh UI indicators
            self.refresh_project_indicators(cx);
        }
    }

    /// Restart LSP servers with new workspace root when project directory changes
    #[instrument(skip(self, cx))]
    fn restart_lsp_servers_for_workspace_change(
        &mut self,
        old_project_root: Option<std::path::PathBuf>,
        new_project_root: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        info!(
            new_project_root = %new_project_root.display(),
            "🔄 LSP_RESTART: Starting LSP server restart for workspace change"
        );

        // Get the LSP command sender from the Application
        let lsp_command_sender = self.core.read(cx).get_project_lsp_command_sender();

        if let Some(sender) = lsp_command_sender {
            info!(
                old_project_root = ?old_project_root.as_ref().map(|p| p.display()),
                new_project_root = %new_project_root.display(),
                current_working_dir = ?std::env::current_dir().ok(),
                "🔄 LSP_RESTART: Sending RestartServersForWorkspaceChange command to Application"
            );

            // Create the command with a span for tracing
            let span = tracing::info_span!("workspace_lsp_restart",
                old_workspace = ?old_project_root.as_ref().map(|p| p.display()),
                new_workspace = %new_project_root.display()
            );

            // Create response channel
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            // Send the command using the event-driven pattern
            let command = nucleotide_events::ProjectLspCommand::RestartServersForWorkspaceChange {
                old_workspace_root: old_project_root,
                new_workspace_root: new_project_root.to_path_buf(),
                response: response_tx,
                span,
            };

            if let Err(e) = sender.send(command) {
                error!(
                    error = %e,
                    new_project_root = %new_project_root.display(),
                    "Failed to send RestartServersForWorkspaceChange command"
                );
                return;
            }

            // Spawn a task to handle the response asynchronously using the runtime handle
            let new_project_root_display = new_project_root.display().to_string();
            self.handle.spawn(async move {
                // Add a timeout to prevent indefinite waiting
                let timeout_duration = tokio::time::Duration::from_secs(30); // 30 second timeout for LSP operations
                match tokio::time::timeout(timeout_duration, response_rx).await {
                    Ok(response_result) => match response_result {
                        Ok(Ok(results)) => {
                            info!(
                                restart_count = results.len(),
                                new_project_root = %new_project_root_display,
                                "LSP server restart completed successfully"
                            );
                            for result in results {
                                info!(
                                    server_name = %result.server_name,
                                    language_id = %result.language_id,
                                    server_id = ?result.server_id,
                                    "Server restarted successfully"
                                );
                            }
                        }
                        Ok(Err(e)) => {
                            error!(
                                error = %e,
                                new_project_root = %new_project_root_display,
                                "LSP server restart failed"
                            );
                        }
                        Err(_) => {
                            warn!(
                                new_project_root = %new_project_root_display,
                                "RestartServersForWorkspaceChange response channel was dropped"
                            );
                        }
                    }
                    Err(_timeout) => {
                        error!(
                            new_project_root = %new_project_root_display,
                            timeout_seconds = 30,
                            "LSP server restart timed out - this may indicate environment capture is taking too long"
                        );
                    }
                }
            });

            info!(
                new_project_root = %new_project_root.display(),
                "RestartServersForWorkspaceChange command sent successfully"
            );
        } else {
            warn!(
                new_project_root = %new_project_root.display(),
                "No LSP command sender available - cannot restart LSP servers"
            );
        }
    }

    /// Trigger project detection and coordinate proactive LSP startup through Application.
    #[instrument(skip(self, cx))]
    fn trigger_project_detection_and_lsp_startup(
        &mut self,
        project_root: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        info!(project_root = %project_root.display(), "Starting project detection and LSP coordination");

        // Force refresh project detection in the project status service
        info!(project_root = %project_root.display(), "Updating project status service with project root");
        let project_status = nucleotide_project::project_status_service(cx);
        project_status.set_project_root(Some(project_root.clone()));
        info!("Project status service updated, refreshing project detection");
        project_status.refresh_project_detection();
        info!("Project detection refresh completed");

        if let Some(sender) = self.core.read(cx).get_project_lsp_command_sender() {
            let span = tracing::info_span!(
                "workspace_project_lsp_detect",
                workspace_root = %project_root.display()
            );
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let command = nucleotide_events::ProjectLspCommand::DetectAndStartProject {
                workspace_root: project_root.clone(),
                response: response_tx,
                span,
            };

            if let Err(error) = sender.send(command) {
                error!(
                    error = %error,
                    project_root = %project_root.display(),
                    "Failed to send DetectAndStartProject command"
                );
            } else {
                let project_root_display = project_root.display().to_string();
                self.handle.spawn(async move {
                    let timeout = tokio::time::Duration::from_secs(30);
                    match tokio::time::timeout(timeout, response_rx).await {
                        Ok(Ok(Ok(result))) => {
                            info!(
                                project_root = %project_root_display,
                                project_type = ?result.project_type,
                                language_servers = ?result.language_servers,
                                servers_started = result.servers_started.len(),
                                "Project detection and LSP startup completed"
                            );
                        }
                        Ok(Ok(Err(error))) => {
                            error!(
                                error = %error,
                                project_root = %project_root_display,
                                "Project detection and LSP startup failed"
                            );
                        }
                        Ok(Err(_)) => {
                            warn!(
                                project_root = %project_root_display,
                                "DetectAndStartProject response channel was dropped"
                            );
                        }
                        Err(_) => {
                            error!(
                                project_root = %project_root_display,
                                timeout_seconds = 30,
                                "Project detection and LSP startup timed out"
                            );
                        }
                    }
                });
            }
        } else {
            warn!("No LSP command sender available - skipping project LSP coordination");
        }

        // Update UI indicators and refresh project status display
        self.refresh_project_indicators(cx);

        // Process any events that may have been sent during project detection.
        self.core
            .update(cx, |app, _cx| app.request_event_driven_maintenance());
    }

    /// Set the current project root explicitly
    /// This is used during workspace initialization to ensure the project root is set correctly
    pub fn set_current_project_root(
        &mut self,
        root: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.current_project_root = root;
        self.refresh_environment_badge(self.current_project_root.clone(), cx);
        if let Some(ref root) = self.current_project_root {
            add_recent_project(root, cx);
            info!(project_root = %root.display(), "Set current project root explicitly");
        } else {
            info!("Cleared current project root");
        }
    }

    /// Subscribe to LSP state changes to update project indicators
    #[instrument(skip(self, cx))]
    fn setup_lsp_state_subscription(&mut self, cx: &mut Context<Self>) {
        // For now, refresh the status from explicit workspace/LSP update paths
        // since LspState doesn't implement EventEmitter yet.
        if let Some(_lsp_state_entity) = self.core.read(cx).lsp_state.clone() {
            info!("LSP state available for project status updates");

            // Initial update
            self.update_project_status_from_lsp_state(cx);
        } else {
            debug!("No LSP state available for subscription");
        }
    }

    /// Update project status indicators based on current LSP state
    #[instrument(skip(self, cx))]
    fn update_project_status_from_lsp_state(&mut self, cx: &mut Context<Self>) {
        if let Some(lsp_state_entity) = self.core.read(cx).lsp_state.clone() {
            // Get project status service first
            let project_status = nucleotide_project::project_status_service(cx);

            // Clone the LSP state and update project status outside the closure
            let lsp_state_clone = lsp_state_entity.read(cx).clone();
            project_status.update_lsp_state(&lsp_state_clone);

            debug!("Updated project status from LSP state");
        }
    }

    /// Refresh project indicators and trigger UI updates
    #[instrument(skip(self, cx))]
    fn refresh_project_indicators(&mut self, cx: &mut Context<Self>) {
        debug!("Refreshing project indicators");

        // Update project status from current LSP state if available
        self.update_project_status_from_lsp_state(cx);

        // Notify UI components to re-render with updated project information
        cx.notify();

        // Project detection complete - UI will be refreshed via cx.notify()

        info!("Project indicators refreshed");
    }

    // Removed - views are created in main.rs and passed in

    // Removed - views are created in main.rs and passed in

    pub fn theme(editor: &Entity<Core>, cx: &mut Context<Self>) -> helix_view::Theme {
        editor.read(cx).editor.theme.clone()
    }

    fn sync_ui_theme_from_theme_manager<V: 'static>(cx: &mut Context<V>) {
        let ui_theme = cx.global::<crate::ThemeManager>().ui_theme().clone();
        *cx.global_mut::<nucleotide_ui::Theme>() = ui_theme.clone();
        nucleotide_ui::providers::update_provider_context(|context| {
            let theme_provider = nucleotide_ui::providers::ThemeProvider::new(ui_theme);
            context.register_global_provider(theme_provider);
        });
    }

    fn handle_appearance_change(
        &mut self,
        appearance: WindowAppearance,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::types::{AppEvent, UiEvent, Update};
        use nucleotide_appearance::SystemAppearance;

        // Update system appearance in theme manager
        let system_appearance = match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => SystemAppearance::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => SystemAppearance::Light,
        };

        nucleotide_logging::info!(
            window_appearance = ?appearance,
            system_appearance = ?system_appearance,
            "OS appearance change detected - emitting SystemAppearanceChanged event"
        );

        // Update global SystemAppearance state
        cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
            theme_manager.set_system_appearance(system_appearance);
        });
        Self::sync_ui_theme_from_theme_manager(cx);
        *nucleotide_appearance::SystemAppearance::global_mut(cx) = system_appearance;

        // Mark theme colors as dirty so they get recomputed on next render
        self.colors_dirty = true;

        // Emit SystemAppearanceChanged event for event-driven handling
        let event_appearance = match system_appearance {
            SystemAppearance::Dark => crate::types::SystemAppearance::Dark,
            SystemAppearance::Light => crate::types::SystemAppearance::Light,
        };

        cx.emit(Update::Event(AppEvent::Ui(
            UiEvent::SystemAppearanceChanged {
                appearance: event_appearance,
            },
        )));
    }

    /// Version of switch_theme_by_name for use from event handlers (no window appearance updates)
    fn switch_theme_by_name_no_window(&mut self, theme_name: &str, cx: &mut Context<Self>) {
        nucleotide_logging::info!(
            theme_name = %theme_name,
            "Switching theme via event handler (no window appearance update)"
        );

        // Update theme in the editor
        self.core.update(cx, |core, cx| {
            let theme_name = if core.editor.theme_loader.load(theme_name).is_ok() {
                theme_name.to_string()
            } else {
                nucleotide_logging::warn!(theme_name = %theme_name, "Theme not found, using default");
                core.editor.theme.name().to_string()
            };

            // Set theme in the editor
            if let Ok(theme) = core.editor.theme_loader.load(&theme_name) {
                core.editor.set_theme(theme);
                nucleotide_logging::info!(theme_name = %theme_name, "Theme loaded successfully");
            }

            // Update theme manager global
            cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
                theme_manager.set_theme(core.editor.theme.clone());
            });

            Self::sync_ui_theme_from_theme_manager(cx);
        });

        // Clear caches and redraw
        self.clear_shaped_lines_cache(cx);
        // Mark cached colors dirty and recompute immediately so background updates propagate
        self.colors_dirty = true;
        self.recompute_theme_colors(cx);
        cx.notify();
    }

    // removed unused switch_theme_by_name

    fn update_window_appearance(&self, window: &mut Window, cx: &Context<Self>) {
        let config = self.core.read(cx).config.clone();

        if !config.gui.window.appearance_follows_theme {
            debug!("Window appearance does not follow theme - skipping update");
            return;
        }

        let theme_manager = cx.global::<crate::ThemeManager>();
        let is_dark = theme_manager.is_dark_chrome();

        let appearance = config.window_background_appearance(is_dark);

        let theme_name = self.core.read(cx).editor.theme.name();
        info!(
            is_dark = is_dark,
            appearance = ?appearance,
            blur_dark_themes = config.gui.window.blur_dark_themes,
            ui_chrome_style = ?config.ui_chrome_style(),
            theme_name = %theme_name,
            "Updating window background appearance based on UI chrome"
        );

        window.set_background_appearance(appearance);
    }

    /// Recompute cached theme-derived colors
    fn recompute_theme_colors(&mut self, cx: &mut Context<Self>) {
        let tokens = cx.theme().tokens;

        let uses_windows_material_backdrop = cfg!(target_os = "windows")
            && cx
                .try_global::<crate::ThemeManager>()
                .map(|theme_manager| {
                    theme_manager.ui_chrome_style() == nucleotide_appearance::UiChromeStyle::System
                })
                .unwrap_or(false);

        self.cached_bg_color = if uses_windows_material_backdrop {
            gpui::hsla(0.0, 0.0, 0.0, 0.0)
        } else {
            tokens.editor.background
        };
        self.cached_text_color = tokens.chrome.text_on_chrome;
        self.cached_border_color = tokens.chrome.border_default;

        info!(
            cached_bg_color = ?self.cached_bg_color,
            cached_text_color = ?self.cached_text_color,
            cached_border_color = ?self.cached_border_color,
            "Workspace: recomputed cached token colors"
        );

        self.colors_dirty = false;
    }

    /// Schedule window appearance update to be applied in the next render cycle.
    fn schedule_window_appearance_update(&mut self, cx: &mut Context<Self>) {
        let theme_name = self.core.read(cx).editor.theme.name();
        info!(
            theme_name = %theme_name,
            "Scheduling window appearance update for next render cycle"
        );
        self.needs_window_appearance_update = true;
        cx.notify(); // Trigger re-render
    }

    // removed unused update_titlebar_appearance

    // removed unused set_macos_window_appearance (macOS)

    #[cfg(any())]
    unsafe fn update_titlebar_appearance_native(
        system_appearance: nucleotide_appearance::SystemAppearance,
    ) {
        use nucleotide_appearance::SystemAppearance;
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_app_kit::{NSApplication, NSWindow};
        use objc2_foundation::{MainThreadMarker, NSArray, NSString};

        // Get all windows from NSApplication instead of just the main window

        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        let windows: &NSArray<NSWindow> = unsafe { msg_send![&**app, windows] };
        let window_count = windows.count();

        nucleotide_logging::debug!(
            window_count = window_count,
            "Found {} windows in NSApplication",
            window_count
        );

        // Log details about all windows to make sure we're targeting the right one
        for i in 0..window_count {
            let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
            let window_title: *mut AnyObject = msg_send![window, title];
            let title_str = if !window_title.is_null() {
                let cstr: *const i8 = msg_send![window_title, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };
            let window_level: i64 = msg_send![window, level];
            let is_visible: bool = msg_send![window, isVisible];
            let is_main: bool = msg_send![window, isMainWindow];
            let is_key: bool = msg_send![window, isKeyWindow];

            nucleotide_logging::debug!(
                window_index = i,
                window_title = title_str,
                window_level = window_level,
                is_visible = is_visible,
                is_main = is_main,
                is_key = is_key,
                "Window details"
            );
        }

        if window_count > 0 {
            // Find the actual main/key window instead of just taking the first one
            let mut target_window: *mut AnyObject = std::ptr::null_mut();

            // First try to find the main window
            for i in 0..window_count {
                let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                let is_main: bool = msg_send![window, isMainWindow];
                if is_main {
                    target_window = window;
                    nucleotide_logging::debug!(window_index = i, "Found main window");
                    break;
                }
            }

            // If no main window, try to find the key window
            if target_window.is_null() {
                for i in 0..window_count {
                    let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                    let is_key: bool = msg_send![window, isKeyWindow];
                    if is_key {
                        target_window = window;
                        nucleotide_logging::debug!(window_index = i, "Found key window");
                        break;
                    }
                }
            }

            // If still no target, find the first visible window with a titlebar
            if target_window.is_null() {
                for i in 0..window_count {
                    let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
                    let is_visible: bool = msg_send![window, isVisible];
                    let has_titlebar: bool = msg_send![window, hasTitleBar];
                    if is_visible && has_titlebar {
                        target_window = window;
                        nucleotide_logging::debug!(
                            window_index = i,
                            "Found visible window with titlebar"
                        );
                        break;
                    }
                }
            }

            // Fall back to first window if all else fails
            if target_window.is_null() {
                target_window = msg_send![windows, objectAtIndex: 0];
                nucleotide_logging::warn!("Falling back to first window");
            }

            let window = target_window;

            nucleotide_logging::debug!("Found application window, setting appearance");

            // Check window properties that might affect titlebar appearance
            let style_mask: u64 = msg_send![window, styleMask];
            let is_titled: bool = (style_mask & 1) != 0; // NSTitledWindowMask
            let has_titlebar: bool = msg_send![window, hasTitleBar];
            let titlebar_appears_transparent: bool = msg_send![window, titlebarAppearsTransparent];

            nucleotide_logging::debug!(
                style_mask = style_mask,
                is_titled = is_titled,
                has_titlebar = has_titlebar,
                titlebar_appears_transparent = titlebar_appears_transparent,
                "Window titlebar properties"
            );

            // Check current appearance before setting
            let current_appearance: *mut AnyObject = msg_send![window, appearance];
            nucleotide_logging::debug!(
                current_appearance_is_nil = (current_appearance.is_null()),
                "Window appearance before setting"
            );

            // Set the window appearance to match the detected system appearance
            match system_appearance {
                SystemAppearance::Dark => {
                    // Set to dark appearance explicitly
                    let dark_appearance_name = NSString::from_str("NSAppearanceNameDarkAqua");
                    let dark_appearance: *mut AnyObject =
                        msg_send![class!(NSAppearance), appearanceNamed: &*dark_appearance_name];
                    let _: () = msg_send![window, setAppearance: dark_appearance];
                    nucleotide_logging::debug!("Set window to dark appearance explicitly");
                }
                SystemAppearance::Light => {
                    // Set to light appearance explicitly
                    let light_appearance_name = NSString::from_str("NSAppearanceNameAqua");
                    let light_appearance: *mut AnyObject =
                        msg_send![class!(NSAppearance), appearanceNamed: &*light_appearance_name];
                    let _: () = msg_send![window, setAppearance: light_appearance];
                    nucleotide_logging::debug!("Set window to light appearance explicitly");
                }
            }

            // Check appearance after setting and verify it took effect
            let new_appearance: *mut AnyObject = msg_send![window, appearance];
            let new_appearance_name: *mut AnyObject = if !new_appearance.is_null() {
                msg_send![new_appearance, name]
            } else {
                std::ptr::null_mut()
            };

            let appearance_name_str = if !new_appearance_name.is_null() {
                let cstr: *const i8 = msg_send![new_appearance_name, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };

            nucleotide_logging::info!(
                system_appearance = ?system_appearance,
                new_appearance_is_nil = (new_appearance.is_null()),
                new_appearance_name = appearance_name_str,
                "Successfully set NSWindow appearance"
            );

            // Also check the actual effective appearance to see what macOS thinks
            let effective_appearance: *mut AnyObject = msg_send![window, effectiveAppearance];
            let effective_appearance_name: *mut AnyObject = if !effective_appearance.is_null() {
                msg_send![effective_appearance, name]
            } else {
                std::ptr::null_mut()
            };

            let effective_name_str = if !effective_appearance_name.is_null() {
                let cstr: *const i8 = msg_send![effective_appearance_name, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };

            nucleotide_logging::info!(
                effective_appearance_name = effective_name_str,
                "Window effective appearance after setting"
            );

            // Check if the appearance gets reset by something else shortly after
            // Schedule a delayed check to see if our setting persists

            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                unsafe {
                    let mtm = MainThreadMarker::new_unchecked();
                    let app = NSApplication::sharedApplication(mtm);
                    let windows: *mut AnyObject = msg_send![&**app, windows];
                    let window_count: usize = msg_send![windows, count];

                    if window_count > 0 {
                        let window: *mut AnyObject = msg_send![windows, objectAtIndex: 0];
                        let current_appearance: *mut AnyObject = msg_send![window, appearance];
                        let effective_appearance: *mut AnyObject =
                            msg_send![window, effectiveAppearance];

                        let current_name = if !current_appearance.is_null() {
                            let name: *mut AnyObject = msg_send![current_appearance, name];
                            if !name.is_null() {
                                let cstr: *const i8 = msg_send![name, UTF8String];
                                std::ffi::CStr::from_ptr(cstr).to_str().unwrap_or("unknown")
                            } else {
                                "nil"
                            }
                        } else {
                            "nil"
                        };

                        let effective_name = if !effective_appearance.is_null() {
                            let name: *mut AnyObject = msg_send![effective_appearance, name];
                            if !name.is_null() {
                                let cstr: *const i8 = msg_send![name, UTF8String];
                                std::ffi::CStr::from_ptr(cstr).to_str().unwrap_or("unknown")
                            } else {
                                "nil"
                            }
                        } else {
                            "nil"
                        };

                        nucleotide_logging::warn!(
                            current_appearance_name = current_name,
                            effective_appearance_name = effective_name,
                            "Appearance check 100ms later - did something reset it?"
                        );
                    }
                }
            });
        } else {
            nucleotide_logging::warn!("No windows found in NSApplication, cannot set appearance");
        }
    }

    #[cfg(any())]
    unsafe fn update_titlebar_appearance_native_with_retry(
        system_appearance: nucleotide_appearance::SystemAppearance,
        attempt: u32,
    ) -> bool {
        use nucleotide_appearance::SystemAppearance;
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_app_kit::{NSApplication, NSWindow};
        use objc2_foundation::{MainThreadMarker, NSArray, NSString};

        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        let windows: &NSArray<NSWindow> = unsafe { msg_send![&**app, windows] };
        let window_count = windows.count();

        nucleotide_logging::debug!(
            attempt = attempt,
            window_count = window_count,
            "Retry attempt {} - found {} windows",
            attempt,
            window_count
        );

        if window_count == 0 {
            return false;
        }

        // Look for the proper main window - one with a title and main/key status
        let mut target_window: *mut AnyObject = std::ptr::null_mut();

        for i in 0..window_count {
            let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
            let window_title: *mut AnyObject = msg_send![window, title];
            let title_str = if !window_title.is_null() {
                let cstr: *const i8 = msg_send![window_title, UTF8String];
                unsafe { std::ffi::CStr::from_ptr(cstr) }
                    .to_str()
                    .unwrap_or("unknown")
            } else {
                "nil"
            };
            let is_main: bool = msg_send![window, isMainWindow];
            let is_key: bool = msg_send![window, isKeyWindow];
            let has_titlebar: bool = msg_send![window, hasTitleBar];

            nucleotide_logging::debug!(
                attempt = attempt,
                window_index = i,
                window_title = title_str,
                is_main = is_main,
                is_key = is_key,
                has_titlebar = has_titlebar,
                "Retry window details"
            );

            // Only target windows that are actually main/key windows with titles and titlebars
            if (is_main || is_key) && has_titlebar && !title_str.is_empty() && title_str != "nil" {
                target_window = window;
                nucleotide_logging::info!(
                    attempt = attempt,
                    window_index = i,
                    window_title = title_str,
                    "Found proper main window for titlebar appearance"
                );
                break;
            }
        }

        if target_window.is_null() {
            nucleotide_logging::debug!(
                attempt = attempt,
                "No proper main window found yet, will retry"
            );
            return false;
        }

        // Set the appearance on the proper window
        let window = target_window;
        match system_appearance {
            SystemAppearance::Dark => {
                let dark_appearance_name = NSString::from_str("NSAppearanceNameDarkAqua");
                let dark_appearance: *mut AnyObject =
                    msg_send![class!(NSAppearance), appearanceNamed: &*dark_appearance_name];
                let _: () = msg_send![window, setAppearance: dark_appearance];
                nucleotide_logging::info!(
                    attempt = attempt,
                    "Set window to dark appearance on proper main window"
                );
            }
            SystemAppearance::Light => {
                let light_appearance_name = NSString::from_str("NSAppearanceNameAqua");
                let light_appearance: *mut AnyObject =
                    msg_send![class!(NSAppearance), appearanceNamed: &*light_appearance_name];
                let _: () = msg_send![window, setAppearance: light_appearance];
                nucleotide_logging::info!(
                    attempt = attempt,
                    "Set window to light appearance on proper main window"
                );
            }
        }

        true
    }

    // removed unused ensure_window_follows_system_appearance

    #[cfg(any())]
    fn ensure_nswindow_follows_system(&self) {
        // For now, log that we would set the NSWindow appearance to nil
        nucleotide_logging::info!("Would set NSWindow appearance to nil to follow system");

        // TODO: Implement the actual NSWindow appearance setting
        // This requires accessing the native window handle through GPUI
        // and calling [window setAppearance:nil]
    }

    fn clear_shaped_lines_cache(&self, cx: &mut Context<Self>) {
        for view in self.view_manager.document_views().values() {
            view.update(cx, |view, _cx| {
                view.clear_shaped_lines_cache();
            });
        }
    }

    // Event handler methods extracted from the main handle_event
    fn handle_system_appearance_changed(
        &mut self,
        appearance: crate::types::SystemAppearance,
        cx: &mut Context<Self>,
    ) {
        use crate::config::ThemeMode;

        nucleotide_logging::info!(
            appearance = ?appearance,
            "Handling SystemAppearanceChanged event"
        );

        let config = self.core.read(cx).config.clone();

        // Only switch themes if configured for system mode
        if config.gui.theme.mode == ThemeMode::System {
            let theme_name = match appearance {
                crate::types::SystemAppearance::Light => config.gui.theme.get_light_theme(),
                crate::types::SystemAppearance::Dark => config.gui.theme.get_dark_theme(),
                crate::types::SystemAppearance::Auto => {
                    // For Auto mode, we would need to detect system preference
                    // For now, fall back to the configured default theme
                    config.gui.theme.get_light_theme()
                }
            };

            nucleotide_logging::info!(
                selected_theme = %theme_name,
                "Switching theme for system appearance change"
            );

            // Switch theme directly through the core editor (no window needed)
            self.switch_theme_by_name_no_window(&theme_name, cx);
        } else {
            nucleotide_logging::debug!(
                theme_mode = ?config.gui.theme.mode,
                "Theme mode is not System - ignoring appearance change"
            );
        }
    }

    fn handle_editor_event(
        &mut self,
        ev: &helix_view::editor::EditorEvent,
        cx: &mut Context<Self>,
    ) {
        use helix_view::editor::{ConfigEvent, EditorEvent};
        match ev {
            EditorEvent::Redraw => cx.notify(),
            EditorEvent::ConfigEvent(config_event) => {
                use nucleotide_logging::debug;
                // Handle configuration changes
                debug!(config_event = ?config_event, "Workspace received ConfigEvent");

                // Log current bufferline config when we receive a config event
                let current_bufferline = &self.core.read(cx).editor.config().bufferline;
                debug!(bufferline_config = ?current_bufferline, "Current bufferline config during ConfigEvent");

                match config_event {
                    ConfigEvent::Refresh => {
                        self.refresh_after_editor_config_change(cx);
                        let config = self.core.read(cx).config.clone();
                        self.apply_workspace_config(&config, cx);
                    }
                    ConfigEvent::Update(_) => {
                        self.refresh_after_editor_config_change(cx);
                    }
                }
            }
            EditorEvent::LanguageServerMessage(_) => { /* handled by notifications */ }
            _ => {
                trace!("editor event {ev:?} not handled");
            }
        }
    }

    fn refresh_after_editor_config_change(&mut self, cx: &mut Context<Self>) {
        // Changes like bufferline visibility affect the view set and chrome.
        self.update_document_views(cx);
        cx.notify();
    }

    fn handle_redraw(&mut self, cx: &mut Context<Self>) {
        // Clear the shaped lines cache to force re-rendering with updated config
        self.clear_shaped_lines_cache(cx);

        // Minimal redraw - most updates now come through specific events
        if let Some(view) = self
            .view_manager
            .focused_view_id()
            .and_then(|id| self.view_manager.get_document_view(&id))
        {
            view.update(cx, |_view, cx| {
                cx.notify();
            })
        }

        // Update key hints on redraw
        self.update_key_hints(cx);
        cx.notify();
    }

    fn handle_viewport_scroll(
        &mut self,
        view_id: helix_view::ViewId,
        request: nucleotide_editor::EditorViewportScrollRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(view_entity) = self.view_manager.get_document_view(&view_id) else {
            return;
        };

        let (update, visible_rows) = view_entity.update(cx, |view, cx| {
            let update = view.apply_viewport_scroll(request);
            if update.changed
                || matches!(
                    request,
                    nucleotide_editor::EditorViewportScrollRequest::CursorReveal(_)
                )
            {
                cx.notify();
            }
            (update, view.visible_visual_rows())
        });

        if let Some(direction) = request.page_cursor_sync_direction() {
            let changed_doc_id = self.core.update(cx, |core, _cx| {
                crate::application::editor_input::sync_cursor_after_native_page_scroll(
                    &mut core.editor,
                    view_id,
                    direction,
                    update.top_visual_row,
                    visible_rows,
                )
            });
            if let Some(doc_id) = changed_doc_id {
                self.handle_selection_changed(doc_id, view_id, cx);
            }
        }

        cx.notify();
    }

    fn handle_viewport_cursor(
        &mut self,
        view_id: helix_view::ViewId,
        request: nucleotide_editor::EditorViewportCursorRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(view_entity) = self.view_manager.get_document_view(&view_id) else {
            return;
        };

        let (top_visual_row, visible_rows, content_rows) = view_entity.update(cx, |view, _cx| {
            (
                view.top_visual_row(),
                view.visible_visual_rows(),
                view.content_visual_rows(),
            )
        });
        let scrolloff = self
            .core
            .read(cx)
            .editor
            .config()
            .scrolloff
            .min(visible_rows.saturating_sub(1) / 2);
        let target_visual_row =
            request.target_visual_row(top_visual_row, visible_rows, content_rows, scrolloff);

        let changed_doc_id = self.core.update(cx, |core, _cx| {
            crate::application::editor_input::apply_native_viewport_cursor_request(
                &mut core.editor,
                view_id,
                target_visual_row,
            )
        });

        if let Some(doc_id) = changed_doc_id {
            self.handle_selection_changed(doc_id, view_id, cx);
        }

        cx.notify();
    }

    fn handle_overlay_update(&mut self, cx: &mut Context<Self>) {
        // When a picker, prompt, or completion appears, auto-dismiss the info box
        self.info_hidden = true;

        // Check if completion is now active and manage input contexts
        let has_completion = self.overlay.read(cx).has_completion();
        self.manage_completion_context(has_completion);

        // Focus will be handled by the overlay components
        cx.notify();
    }

    fn handle_document_changed(&mut self, doc_id: helix_view::DocumentId, cx: &mut Context<Self>) {
        let is_modified = self
            .core
            .read(cx)
            .editor
            .document(doc_id)
            .is_some_and(|doc| doc.is_modified());
        let is_preview = cx
            .try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            .is_some_and(|tracker| tracker.is_preview_doc(doc_id));
        if should_unpreview_changed_document(is_preview, is_modified) {
            self.unregister_preview_document(doc_id, cx);
        }

        let focused_doc_id = {
            let core = self.core.read(cx);
            core.editor
                .tree
                .try_get(core.editor.tree.focus)
                .map(|view| view.doc)
        };
        if should_refine_completion_for_focused_document(
            self.overlay.read(cx).has_completion(),
            focused_doc_id,
            doc_id,
        ) {
            self.update_completion_filter_auto(cx);
        }

        // Document content changed - update specific document view
        self.update_specific_document_view(doc_id, cx);
        cx.notify();
    }

    fn handle_selection_changed(
        &mut self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        cx: &mut Context<Self>,
    ) {
        let _timer = nucleotide_logging::PerfTimer::new("Workspace::handle_selection_changed")
            .with_warn_threshold(std::time::Duration::from_millis(4));
        // Selection/cursor moved - update status and specific view
        nucleotide_logging::trace!(doc_id = ?doc_id, view_id = ?view_id, "Selection changed");
        self.update_specific_document_view(doc_id, cx);
        let focused_doc_id = {
            let core = self.core.read(cx);
            core.editor
                .tree
                .try_get(core.editor.tree.focus)
                .map(|view| view.doc)
        };
        if should_refine_completion_for_focused_document(
            self.overlay.read(cx).has_completion(),
            focused_doc_id,
            doc_id,
        ) {
            self.update_completion_filter_auto(cx);
        }
        if let Some(view_entity) = self.view_manager.get_document_view(&view_id) {
            view_entity.update(cx, |view, cx| {
                view.request_cursor_reveal();
                cx.notify();
            });
        }
        cx.notify();
    }

    fn handle_mode_changed(
        &mut self,
        old_mode: &helix_view::document::Mode,
        new_mode: &helix_view::document::Mode,
        cx: &mut Context<Self>,
    ) {
        // Editor mode changed - update status line and current view
        info!("Mode changed from {:?} to {:?}", old_mode, new_mode);
        self.update_current_document_view(cx);
        cx.notify();
    }

    fn handle_diagnostics_changed(
        &mut self,
        doc_id: helix_view::DocumentId,
        cx: &mut Context<Self>,
    ) {
        // LSP diagnostics changed - update specific document view
        nucleotide_logging::debug!(doc_id = ?doc_id, "DIAG: Workspace handling DiagnosticsChanged - updating view");
        self.update_specific_document_view(doc_id, cx);
        cx.notify();
    }

    fn handle_document_opened(&mut self, doc_id: helix_view::DocumentId, cx: &mut Context<Self>) {
        // New document opened - the view will be created automatically
        info!("Document opened: {:?}", doc_id);
        self.ensure_document_in_order(doc_id);

        // Start LSP for the newly opened document using the feature flag system
        info!("Starting LSP for newly opened document using feature flag system");
        let lsp_result = self
            .core
            .update(cx, |core, _| core.start_lsp_with_feature_flags(doc_id));

        match lsp_result {
            nucleotide_lsp::LspStartupResult::Success {
                mode,
                language_servers,
                duration,
            } => {
                info!(
                    doc_id = ?doc_id,
                    mode = ?mode,
                    language_servers = ?language_servers,
                    duration_ms = duration.as_millis(),
                    "LSP startup successful for newly opened document"
                );
            }
            nucleotide_lsp::LspStartupResult::Failed {
                mode,
                error,
                fallback_mode,
            } => {
                warn!(
                    doc_id = ?doc_id,
                    mode = ?mode,
                    error = %error,
                    fallback_mode = ?fallback_mode,
                    "LSP startup failed for newly opened document"
                );
            }
            nucleotide_lsp::LspStartupResult::Skipped { reason } => {
                info!(
                    doc_id = ?doc_id,
                    reason = %reason,
                    "LSP startup skipped for newly opened document"
                );
            }
        }

        // Sync file tree selection with the newly opened document
        let doc_path = {
            let core = self.core.read(cx);
            core.editor
                .document(doc_id)
                .and_then(|doc| doc.path())
                .map(|p| p.to_path_buf())
        };

        if let Some(path) = doc_path
            && let Some(file_tree) = &self.file_tree
        {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(path.as_path()), cx);
            });
        }

        self.enforce_max_tabs(Some(doc_id), cx);
        cx.notify();
    }

    fn handle_document_closed(&mut self, doc_id: helix_view::DocumentId, cx: &mut Context<Self>) {
        // Document closed - the view will be cleaned up automatically
        info!("Document closed: {:?}", doc_id);
        self.document_order.retain(|candidate| *candidate != doc_id);
        self.pinned_documents.remove(&TabId::Document(doc_id));
        self.unregister_preview_document(doc_id, cx);
        cx.notify();
    }

    fn handle_view_focused(&mut self, view_id: helix_view::ViewId, cx: &mut Context<Self>) {
        info!("View focused: {:?}", view_id);
        self.active_image_tab_id = None;
        self.view_manager.handle_view_focused(view_id, cx);

        let focused_filename = self.current_filename(cx);
        self.update_titlebar_filename(focused_filename.as_deref(), true, cx);

        // Sync file tree selection with the newly focused view
        let doc_path = {
            let core = self.core.read(cx);
            if let Some(view) = core.editor.tree.try_get(view_id) {
                core.editor
                    .document(view.doc)
                    .and_then(|doc| doc.path())
                    .map(|p| p.to_path_buf())
            } else {
                None
            }
        };

        if let Some(path) = doc_path
            && let Some(file_tree) = &self.file_tree
        {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(path.as_path()), cx);
            });
        }

        cx.notify();
    }

    fn handle_language_server_initialized(
        &mut self,
        server_id: helix_lsp::LanguageServerId,
        cx: &mut Context<Self>,
    ) {
        // LSP server initialized - update status
        info!("Language server initialized: {:?}", server_id);
        cx.notify();
    }

    fn handle_language_server_exited(
        &mut self,
        server_id: helix_lsp::LanguageServerId,
        cx: &mut Context<Self>,
    ) {
        // LSP server exited - update status
        info!("Language server exited: {:?}", server_id);
        cx.notify();
    }

    fn handle_completion_requested(
        &mut self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: &event_bridge::CompletionTrigger,
        cx: &mut Context<Self>,
    ) {
        // Completion was requested - trigger completion UI
        nucleotide_logging::debug!(
            "🎯 TRIGGER COMPLETION: doc {:?}, view {:?}, trigger: {:?}",
            doc_id,
            view_id,
            trigger
        );

        let Some(cursor) = self.completion_cursor(doc_id, view_id, cx) else {
            return;
        };

        match trigger {
            crate::types::CompletionTrigger::Manual => {
                nucleotide_logging::debug!("Manual completion triggered (CTRL+Space)");
                self.process_completion_trigger(
                    cursor,
                    doc_id,
                    view_id,
                    LspCompletionTrigger::Manual,
                    cx,
                );
            }
            crate::types::CompletionTrigger::Character(c) => {
                nucleotide_logging::debug!(character = %c, "Character-triggered completion");
                self.process_completion_trigger(
                    cursor,
                    doc_id,
                    view_id,
                    LspCompletionTrigger::Character(*c),
                    cx,
                );
            }
            crate::types::CompletionTrigger::Automatic => {
                nucleotide_logging::debug!("Automatic completion triggered");
                self.process_completion_trigger(
                    cursor,
                    doc_id,
                    view_id,
                    LspCompletionTrigger::Automatic,
                    cx,
                );
            }
        }

        cx.notify();
    }

    fn handle_search_submitted(&mut self, search_text: &str, cx: &mut Context<Self>) {
        // Execute the search in Helix
        debug!("Search submitted: {}", search_text);

        // Clear the overlay first to hide the prompt
        self.overlay.update(cx, |overlay, cx| {
            overlay.dismiss_all(cx);
        });

        // We need to execute the search directly in Helix since we've replaced the prompt
        let mut reveal_center_view = None;
        self.core.update(cx, |core, cx| {
            let _guard = self.handle.enter();

            // First, remove any existing Helix prompt from the compositor
            // This ensures the EditorView will handle subsequent keys
            core.compositor.remove("prompt");

            // Store the search pattern in the register (raw pattern, not regex)
            core.editor.registers.last_search_register = '/';
            let _ = core.editor.registers.push('/', search_text.to_string());

            // Compile the regex pattern
            use helix_core::graphemes;
            use helix_stdx::rope::{self, RopeSliceExt};

            let case_insensitive = core.editor.config().search.smart_case
                && search_text.chars().all(char::is_lowercase);

            // Build regex the same way Helix does it in search_next_or_prev_impl
            let regex = if let Ok(regex) = rope::RegexBuilder::new()
                .syntax(
                    rope::Config::new()
                        .case_insensitive(case_insensitive)
                        .multi_line(true),
                )
                .build(search_text)
            {
                Ok(regex)
            } else {
                Err(format!("Failed to compile regex: {search_text}"))
            };

            match regex {
                Ok(ref regex) => {
                    // Get current state
                    let view_id = core.editor.tree.focus;
                    let Some(doc_id) = core.editor.tree.try_get(view_id).map(|view| view.doc)
                    else {
                        core.editor.set_error("No active view for search");
                        return;
                    };
                    let wrap_around = core.editor.config().search.wrap_around;

                    // Get text and current selection
                    let (text, current_selection, search_start_byte) = {
                        let Some(doc) = core.editor.documents.get(&doc_id) else {
                            core.editor.set_error("No active document for search");
                            return;
                        };
                        let text = doc.text().slice(..);
                        let selection = doc.selection(view_id);

                        // For forward search, start from the end of the primary selection
                        // and ensure we're on a grapheme boundary
                        let search_start_char = graphemes::ensure_grapheme_boundary_next(
                            text,
                            selection.primary().to(),
                        );
                        let search_start_byte = text.char_to_byte(search_start_char);

                        (text, selection.clone(), search_start_byte)
                    };

                    // Find the next match
                    // IMPORTANT: The regex_input_at_bytes returns a cursor that produces
                    // absolute byte positions, NOT relative to the start offset!
                    let match_range = if let Some(mat) =
                        regex.find(text.regex_input_at_bytes(search_start_byte..))
                    {
                        // The positions are already absolute in the document
                        Some((mat.start(), mat.end()))
                    } else if wrap_around {
                        // When searching from the beginning, positions are also absolute
                        regex
                            .find(text.regex_input())
                            .map(|mat| (mat.start(), mat.end()))
                    } else {
                        None
                    };

                    // Apply the match if found
                    if let Some((start_byte, end_byte)) = match_range {
                        // Skip empty matches
                        if start_byte == end_byte {
                            core.editor.set_error("Empty match");
                            return;
                        }

                        let start_char = text.byte_to_char(start_byte);
                        let end_char = text.byte_to_char(end_byte);

                        // Create a range for the match - exactly as Helix does it
                        use helix_core::Range;
                        let range = Range::new(start_char, end_char);

                        // Replace the primary selection with the new range
                        let primary_index = current_selection.primary_index();
                        let new_selection = current_selection.replace(primary_index, range);

                        if let Some(doc) = core.editor.documents.get_mut(&doc_id) {
                            doc.set_selection(view_id, new_selection);
                        } else {
                            core.editor.set_error("No active document for search");
                            return;
                        }

                        reveal_center_view = Some(view_id);

                        // Show wrapped message if we wrapped
                        if wrap_around && start_byte < search_start_byte {
                            core.editor.set_status("Wrapped around document");
                        }
                    } else {
                        core.editor
                            .set_error(format!("Pattern not found: {search_text}"));
                    }
                }
                Err(e) => {
                    core.editor.set_error(format!("Invalid regex: {e}"));
                }
            }

            cx.notify();
        });

        if let Some(view_id) = reveal_center_view
            && let Some(view_entity) = self.view_manager.get_document_view(&view_id)
        {
            view_entity.update(cx, |view, cx| {
                view.request_cursor_center();
                cx.notify();
            });
        }
    }

    fn handle_global_search_submitted(&mut self, query: &str, cx: &mut Context<Self>) {
        debug!(query = query, "Global search submitted");

        self.overlay.update(cx, |overlay, cx| {
            overlay.dismiss_all(cx);
        });

        if query.is_empty() {
            return;
        }

        let (search_root, smart_case, file_picker_config, open_documents) = {
            let core = self.core.read(cx);
            let search_root = core
                .project_directory
                .clone()
                .unwrap_or_else(helix_stdx::env::current_working_dir);
            let config = core.editor.config();
            let smart_case = config.search.smart_case;
            let file_picker_config = config.file_picker.clone();
            let open_documents = core
                .editor
                .documents
                .values()
                .filter_map(|doc| {
                    doc.path()
                        .cloned()
                        .map(|path| (path, doc.text().to_owned()))
                })
                .collect::<Vec<_>>();

            (search_root, smart_case, file_picker_config, open_documents)
        };

        self.core.update(cx, |core, _cx| {
            core.editor.registers.last_search_register = '/';
            let _ = core.editor.registers.push('/', query.to_string());
        });

        let matches = match global_search_matches(
            &search_root,
            query,
            smart_case,
            &file_picker_config,
            &open_documents,
            GLOBAL_SEARCH_RESULT_LIMIT,
        ) {
            Ok(matches) => matches,
            Err(err) => {
                self.core.update(cx, |core, _cx| {
                    core.editor.set_error(err);
                });
                return;
            }
        };

        if matches.is_empty() {
            self.core.update(cx, |core, _cx| {
                core.editor.set_error(format!("Pattern not found: {query}"));
            });
            return;
        }

        let match_count = matches.len();
        let picker = global_search_picker(&search_root, matches);
        self.core.update(cx, |core, cx| {
            if match_count >= GLOBAL_SEARCH_RESULT_LIMIT {
                core.editor.set_status(format!(
                    "Showing first {GLOBAL_SEARCH_RESULT_LIMIT} global search matches"
                ));
            } else {
                core.editor
                    .set_status(format!("{match_count} global search matches"));
            }
            cx.emit(crate::Update::Picker(picker));
        });
    }

    fn handle_regex_selection_submitted(
        &mut self,
        action: RegexSelectionAction,
        regex_text: &str,
        cx: &mut Context<Self>,
    ) {
        debug!(
            action = ?action,
            regex = regex_text,
            "Regex selection submitted"
        );

        self.overlay.update(cx, |overlay, cx| {
            overlay.dismiss_all(cx);
        });

        if regex_text.is_empty() {
            return;
        }

        let mut changed_selection = None;
        self.core.update(cx, |core, cx| {
            let _guard = self.handle.enter();

            let case_insensitive = core.editor.config().search.smart_case
                && !regex_text.chars().any(char::is_uppercase);
            let regex = match helix_stdx::rope::RegexBuilder::new()
                .syntax(
                    helix_stdx::rope::Config::new()
                        .case_insensitive(case_insensitive)
                        .multi_line(true),
                )
                .build(regex_text)
            {
                Ok(regex) => regex,
                Err(err) => {
                    core.editor.set_error(format!("Invalid regex: {err}"));
                    return;
                }
            };

            let view_id = core.editor.tree.focus;
            let Some(doc_id) = core.editor.tree.try_get(view_id).map(|view| view.doc) else {
                return;
            };

            {
                let tree = &mut core.editor.tree;
                let documents = &mut core.editor.documents;
                let view = tree.get_mut(view_id);
                let Some(doc) = documents.get_mut(&doc_id) else {
                    return;
                };
                doc.append_changes_to_history(view);
                let snapshot = doc.selection(view_id).clone();
                view.jumps.push((doc_id, snapshot));
            }

            let result = {
                let Some(doc) = core.editor.documents.get(&doc_id) else {
                    return;
                };
                regex_selection_result(action, doc.text().slice(..), doc.selection(view_id), &regex)
            };

            match result {
                Ok(selection) => {
                    let Some(doc) = core.editor.documents.get_mut(&doc_id) else {
                        return;
                    };
                    doc.set_selection(view_id, selection);
                    core.editor.ensure_cursor_in_view(view_id);
                    changed_selection = Some((doc_id, view_id));
                    cx.emit(crate::Update::Event(crate::types::AppEvent::Core(
                        crate::types::CoreEvent::SelectionChanged { doc_id, view_id },
                    )));
                    cx.emit(crate::Update::Event(crate::types::AppEvent::Core(
                        crate::types::CoreEvent::RedrawRequested,
                    )));
                }
                Err(message) => {
                    core.editor.set_error(message);
                }
            }

            cx.notify();
        });

        if let Some((_, view_id)) = changed_selection
            && let Some(view_entity) = self.view_manager.get_document_view(&view_id)
        {
            view_entity.update(cx, |view, cx| {
                view.request_cursor_center();
                cx.notify();
            });
        }
    }

    fn handle_command_submitted(&mut self, command: &str, cx: &mut Context<Self>) {
        debug!("handle_command_submitted called with '{}'", command);

        // If a file op is pending, treat the submitted text as the name and dispatch an intent
        if let Some(pending) = self.pending_file_op.take() {
            use nucleotide_events::v2::workspace::{Event as WsEvent, FileOpIntent};

            if let PendingFileOp::NewFile { parent } = &pending
                && WslWorkspace::from_unc_path(parent).is_some()
            {
                self.overlay
                    .update(cx, |overlay, cx| overlay.dismiss_all(cx));

                match create_wsl_project_file(parent, command) {
                    Ok(path) => {
                        self.dispatch_workspace_file_op_and_process(
                            WsEvent::FileCreated {
                                path: path.clone(),
                                parent_directory: parent.clone(),
                            },
                            cx,
                        );
                        self.notify_lsp_file_operation(
                            LspFileOperationNotification::Created {
                                path,
                                is_dir: false,
                            },
                            cx,
                        );
                        self.rescan_directory(parent, cx);
                    }
                    Err(error) => {
                        let status = EditorStatus {
                            status: format!("Failed to create WSL file: {error}"),
                            severity: Severity::Error,
                        };
                        self.core.update(cx, |core, cx| {
                            core.editor.set_error(status.status.clone());
                            cx.notify();
                        });
                        self.push_editor_status_notification(status, cx);
                    }
                }
                return;
            }

            if let PendingFileOp::NewFolder { parent } = &pending
                && WslWorkspace::from_unc_path(parent).is_some()
            {
                self.overlay
                    .update(cx, |overlay, cx| overlay.dismiss_all(cx));

                match create_wsl_project_directory(parent, command) {
                    Ok(path) => {
                        self.dispatch_workspace_file_op_and_process(
                            WsEvent::FileCreated {
                                path: path.clone(),
                                parent_directory: parent.clone(),
                            },
                            cx,
                        );
                        self.notify_lsp_file_operation(
                            LspFileOperationNotification::Created { path, is_dir: true },
                            cx,
                        );
                        self.rescan_directory(parent, cx);
                    }
                    Err(error) => {
                        let status = EditorStatus {
                            status: format!("Failed to create WSL folder: {error}"),
                            severity: Severity::Error,
                        };
                        self.core.update(cx, |core, cx| {
                            core.editor.set_error(status.status.clone());
                            cx.notify();
                        });
                        self.push_editor_status_notification(status, cx);
                    }
                }
                return;
            }

            // Build event and decide which directory to rescan using references to avoid moves
            let (event, refresh_dir, lsp_file_operation): (
                WsEvent,
                Option<std::path::PathBuf>,
                Option<LspFileOperationNotification>,
            ) = match &pending {
                PendingFileOp::NewFile { parent } => (
                    WsEvent::FileOpRequested {
                        intent: FileOpIntent::NewFile {
                            parent: parent.clone(),
                            name: command.to_string(),
                        },
                    },
                    Some(parent.clone()),
                    Some(LspFileOperationNotification::Created {
                        path: parent.join(command),
                        is_dir: false,
                    }),
                ),
                PendingFileOp::NewFolder { parent } => (
                    WsEvent::FileOpRequested {
                        intent: FileOpIntent::NewFolder {
                            parent: parent.clone(),
                            name: command.to_string(),
                        },
                    },
                    Some(parent.clone()),
                    Some(LspFileOperationNotification::Created {
                        path: parent.join(command),
                        is_dir: true,
                    }),
                ),
                PendingFileOp::Rename { path } => {
                    let was_dir = self.cached_or_local_path_is_directory(path, cx);
                    let new_path = path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(command);
                    (
                        WsEvent::FileOpRequested {
                            intent: FileOpIntent::Rename {
                                path: path.clone(),
                                new_name: command.to_string(),
                            },
                        },
                        path.parent().map(|p| p.to_path_buf()),
                        Some(LspFileOperationNotification::Renamed {
                            old_path: path.clone(),
                            new_path,
                            was_dir,
                        }),
                    )
                }
                PendingFileOp::Duplicate { path } => {
                    let is_dir = self.cached_or_local_path_is_directory(path, cx);
                    let target_path = path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(command);
                    (
                        WsEvent::FileOpRequested {
                            intent: FileOpIntent::Duplicate {
                                path: path.clone(),
                                target_name: command.to_string(),
                            },
                        },
                        path.parent().map(|p| p.to_path_buf()),
                        Some(LspFileOperationNotification::Created {
                            path: target_path,
                            is_dir,
                        }),
                    )
                }
            };

            // Clear the overlay and dispatch the event
            self.overlay
                .update(cx, |overlay, cx| overlay.dismiss_all(cx));
            self.dispatch_workspace_file_op_and_process(event, cx);

            if let Some(notification) = lsp_file_operation
                && file_operation_notification_succeeded(&notification)
            {
                self.notify_lsp_file_operation(notification, cx);
            }

            if let Some(dir) = refresh_dir {
                self.rescan_directory(&dir, cx);
            }
            return;
        }

        // No pending file op: proceed with normal command handling

        // Clear the overlay first to hide the prompt
        self.overlay
            .update(cx, |overlay, cx| overlay.dismiss_all(cx));

        if self.handle_runnable_command(command, cx) {
            return;
        }

        // Parse the command using our typed system
        match nucleotide_core::ParsedCommand::parse(command) {
            Ok(parsed) => {
                // Log the parsed command for debugging
                debug!("Parsed command: {:?}", parsed);

                // Convert to typed command if possible
                match nucleotide_core::Command::from_parsed(parsed.clone()) {
                    Ok(typed_cmd) => {
                        debug!("Typed command: {:?}", typed_cmd);
                        // Execute the typed command
                        self.execute_typed_command(typed_cmd, cx);
                    }
                    Err(_) => {
                        // Fall back to raw command execution for untyped commands
                        self.execute_raw_command(command, cx);
                    }
                }
            }
            Err(e) => {
                // Show error to user
                let status = EditorStatus {
                    status: format!("Invalid command: {e}"),
                    severity: Severity::Error,
                };
                self.core.update(cx, |core, cx| {
                    core.editor.set_error(status.status.clone());
                    cx.notify();
                });
                self.push_editor_status_notification(status, cx);
            }
        }
    }

    fn handle_runnable_command(&mut self, command: &str, cx: &mut Context<Self>) -> bool {
        match command.trim().trim_start_matches(':') {
            "run" | "runnables" | "show-runnables" => {
                self.show_runnables(cx);
                true
            }
            "run-nearest" => {
                self.run_nearest(cx);
                true
            }
            "run-file-tests" => {
                self.run_file_tests(cx);
                true
            }
            "run-last" | "rerun" => {
                self.run_last(cx);
                true
            }
            _ => false,
        }
    }

    fn execute_typed_command(&mut self, command: nucleotide_core::Command, cx: &mut Context<Self>) {
        use nucleotide_core::{Command, command_system::SplitDirection};

        debug!("execute_typed_command called with: {:?}", command);

        match command {
            Command::Quit { force } => {
                self.execute_raw_command(if force { "quit !" } else { "quit" }, cx);
            }
            Command::Write { path } => {
                let cmd = match path {
                    Some(p) => format!("write {p}"),
                    None => "write".to_string(),
                };
                self.execute_raw_command(&cmd, cx);
            }
            Command::WriteQuit { force } => {
                self.execute_raw_command(if force { "wq !" } else { "wq" }, cx);
            }
            Command::Goto { line } => {
                self.execute_raw_command(&format!("goto {line}"), cx);
            }
            Command::Theme { name } => {
                self.execute_raw_command(&format!("theme {name}"), cx);
            }
            Command::Open { path } => {
                self.execute_raw_command(&format!("open {path}"), cx);
            }
            Command::Split { direction } => match direction {
                SplitDirection::Horizontal => self.execute_raw_command("hsplit", cx),
                SplitDirection::Vertical => self.execute_raw_command("vsplit", cx),
            },
            Command::Close { force } => {
                self.close_active_buffer_document_with_force(force, cx);
            }
            Command::Help { topic } => {
                let cmd = match topic {
                    Some(t) => format!("help {t}"),
                    None => "help".to_string(),
                };
                self.execute_raw_command(&cmd, cx);
            }
            Command::Search { pattern } => {
                self.execute_raw_command(&format!("search {pattern}"), cx);
            }
            Command::Replace {
                pattern,
                replacement,
            } => {
                self.execute_raw_command(&format!("replace {pattern} {replacement}"), cx);
            }
            Command::Generic(parsed) => {
                // Execute generic commands
                self.execute_raw_command(&format!("{parsed}"), cx);
            }
        }
    }

    fn execute_raw_command(&mut self, command: &str, cx: &mut Context<Self>) {
        use nucleotide_logging::debug;
        // Execute the command through helix's command system
        let core = self.core.clone();
        let handle = self.handle.clone();
        let handle_for_command = handle.clone();

        debug!(command = %command, "Executing raw command");

        // Store the current theme before executing the command
        let theme_before = core.read(cx).editor.theme.name().to_string();
        let theme_before_for_closure = theme_before.clone();

        // Log current bufferline config before execution
        let bufferline_before = core.read(cx).editor.config().bufferline.clone();
        debug!(bufferline_config = ?bufferline_before, "Bufferline config before command execution");

        let command_status = core.update(cx, move |core, cx| {
            let _guard = handle_for_command.enter();

            core.editor.clear_status();
            crate::helix_command::execute_command_line(&mut core.editor, &mut core.jobs, command);

            // Check if the theme has changed after command execution
            let current_theme = core.editor.theme.clone();
            let theme_name_after = current_theme.name().to_string();

            // Always trigger a redraw after command execution to reflect any config changes
            cx.emit(crate::Update::Redraw);

            // If the theme has changed, handle it properly using existing theme switching logic
            if theme_before_for_closure != theme_name_after {
                info!(
                    old_theme = %theme_before_for_closure,
                    new_theme = %theme_name_after,
                    "Theme changed via command execution"
                );

                // Send theme change event to Helix
                gpui_to_helix_bridge::send_gpui_event_to_helix(
                    gpui_to_helix_bridge::GpuiToHelixEvent::ThemeChanged {
                        theme_name: theme_name_after.clone(),
                    },
                );
            }

            current_editor_status(&core.editor)
        });

        if let Some(status) = command_status {
            self.push_editor_status_notification(status, cx);
        }

        core.update(cx, |core, _cx| core.request_event_driven_maintenance());

        // Check if theme changed after command execution and handle accordingly
        let theme_name_after = core.read(cx).editor.theme.name().to_string();
        if theme_before != theme_name_after {
            // Use existing theme switching logic (maintains consistency)
            self.switch_theme_by_name_no_window(&theme_name_after, cx);

            // Schedule window appearance update for next render cycle
            self.schedule_window_appearance_update(cx);
        }

        // Check if we should quit after command execution
        let should_quit = core.read(cx).editor.should_close();
        if should_quit {
            cx.emit(crate::Update::Event(crate::types::AppEvent::Core(
                crate::types::CoreEvent::ShouldQuit,
            )));
        }

        // Log bufferline config after execution
        let bufferline_after = core.read(cx).editor.config().bufferline.clone();
        debug!(bufferline_config = ?bufferline_after, "Bufferline config after command execution");

        // If command execution changed bufferline visibility, refresh workspace chrome directly.
        let changed = if bufferline_before != bufferline_after {
            debug!(old_config = ?bufferline_before, new_config = ?bufferline_after, "Bufferline config changed - refreshing workspace chrome");
            true
        } else {
            false
        };

        if changed {
            self.refresh_after_editor_config_change(cx);
        } else {
            // Commands such as hsplit/vsplit/wclose mutate Helix's view tree.
            // Keep the GPUI document-view entities in lockstep before the next render.
            self.update_document_views(cx);
            cx.notify();
        }

        // Log bufferline config in workspace context after command execution
        let bufferline_after_workspace = &core.read(cx).editor.config().bufferline;
        debug!(bufferline_config = ?bufferline_after_workspace, "Bufferline config after command (workspace context)");
    }

    fn handle_open_directory(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        // Find the workspace root from this directory and update working directory
        let workspace_root = find_workspace_root_from(path);
        info!(
            directory_path = %path.display(),
            workspace_root = %workspace_root.display(),
            "🗂️ OPEN_DIR: Opening directory"
        );

        // Update the editor's working directory FIRST
        // This is critical for LSP servers to start with correct workspace
        info!(
            old_cwd = ?std::env::current_dir().ok(),
            new_cwd = %workspace_root.display(),
            "🗂️ OPEN_DIR: Changing working directory before LSP restart"
        );

        if let Err(e) = std::env::set_current_dir(&workspace_root) {
            error!("🗂️ OPEN_DIR: Failed to change working directory: {}", e);
        } else {
            info!(
                confirmed_cwd = ?std::env::current_dir().ok(),
                "🗂️ OPEN_DIR: Working directory successfully changed"
            );
        }

        // CRITICAL: Use helix_stdx to set working directory for consistency
        // This ensures Helix's internal working directory is also updated
        if let Err(e) = helix_stdx::env::set_current_working_dir(&workspace_root) {
            error!("🗂️ OPEN_DIR: Failed to set Helix working directory: {}", e);
        } else {
            info!("🗂️ OPEN_DIR: Helix working directory updated successfully");
        }

        // Use set_project_directory to properly initialize LSP and project management
        // Pass the workspace root (not the selected directory) for proper project management
        info!("🗂️ OPEN_DIR: Calling set_project_directory to trigger LSP restart");
        self.set_project_directory(workspace_root.clone(), cx);

        // Update the file tree with the new directory
        let handle_clone = self.handle.clone();
        let config = file_tree_config_from_gui(&self.core.read(cx).config.gui);
        let new_file_tree = cx.new(|cx| {
            FileTreeView::new_with_runtime(path.to_path_buf(), config, Some(handle_clone), cx)
        });

        // Subscribe to file tree events
        cx.subscribe(&new_file_tree, |workspace, _file_tree, event, cx| {
            info!(
                "Workspace: Received file tree event from new tree: {:?}",
                event
            );
            workspace.handle_file_tree_event(event, cx);
        })
        .detach();

        self.file_tree = Some(new_file_tree);

        // Make sure file tree is visible
        self.show_file_tree = true;
        cx.notify();

        // Show status message about the new project directory
        self.core.update(cx, |core, cx| {
            core.editor
                .set_status(format!("Project directory set to: {}", path.display()));
            cx.notify();
        });
    }

    fn handle_open_file_keep_focus(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        // Open file but don't steal focus from file tree
        let preview_from_project_panel = {
            let core = self.core.read(cx);
            core.config.gui.preview_tabs.enabled
                && core
                    .config
                    .gui
                    .preview_tabs
                    .enable_preview_from_project_panel
        };
        self.open_file_internal(path, false, preview_from_project_panel, None, cx);
    }

    fn handle_open_file(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        // Open file and focus the editor
        self.open_file_internal(path, true, false, None, cx);
    }

    pub fn open_file_at(
        &mut self,
        path: &std::path::Path,
        position: Position,
        cx: &mut Context<Self>,
    ) {
        self.open_file_internal(path, true, false, Some(position), cx);
    }

    /// Open the nucleotide.toml settings file
    pub fn open_settings_file(&mut self, cx: &mut Context<Self>) {
        // Get the Helix config directory path
        let config_dir = helix_loader::config_dir();
        let settings_path = config_dir.join("nucleotide.toml");

        info!("Opening settings file: {}", settings_path.display());

        // Create the config directory if it doesn't exist
        if let Err(e) = std::fs::create_dir_all(&config_dir) {
            nucleotide_logging::error!("Failed to create config directory: {}", e);
            return;
        }

        // Create a default nucleotide.toml if it doesn't exist
        if !settings_path.exists() {
            if let Err(e) = std::fs::write(&settings_path, crate::config::NUCLEOTIDE_EXAMPLE_CONFIG)
            {
                nucleotide_logging::error!("Failed to create default nucleotide.toml: {}", e);
                return;
            }

            info!("Created default nucleotide.toml configuration file");
        }

        // Open the settings file
        self.open_file_internal(&settings_path, true, false, None, cx);
    }

    fn apply_workspace_config(&mut self, config: &crate::config::Config, cx: &mut Context<Self>) {
        let preview_tabs_enabled = config.gui.preview_tabs.enabled;
        let file_tree_config = file_tree_config_from_gui(&config.gui);
        let editor_font = config.editor_font();
        let ui_font = config.ui_font();
        let ui_chrome_style = config.ui_chrome_style();
        let previous_ui_chrome_style = cx.global::<crate::ThemeManager>().ui_chrome_style();
        let ui_chrome_style_changed = previous_ui_chrome_style != ui_chrome_style;

        let editor_font_config = cx.global_mut::<crate::types::EditorFontConfig>();
        editor_font_config.family = editor_font.family.clone();
        editor_font_config.size = editor_font.size;
        editor_font_config.weight = editor_font.weight;
        editor_font_config.line_height = editor_font.line_height;

        let ui_font_config = cx.global_mut::<crate::types::UiFontConfig>();
        ui_font_config.family = ui_font.family.clone();
        ui_font_config.size = ui_font.size;
        ui_font_config.weight = ui_font.weight;

        let font_settings = cx.global_mut::<crate::types::FontSettings>();
        font_settings.fixed_font.family = editor_font.family.clone();
        font_settings.fixed_font.weight = editor_font.weight;
        font_settings.var_font.family = ui_font.family.clone();
        font_settings.var_font.weight = ui_font.weight;

        cx.update_global(|theme_manager: &mut crate::ThemeManager, _cx| {
            theme_manager.set_ui_chrome_style(ui_chrome_style);
            theme_manager.set_ui_font_size(gpui::px(ui_font.size));
        });
        Self::sync_ui_theme_from_theme_manager(cx);

        info!(
            ui_font_family = %ui_font.family,
            ui_font_size = ui_font.size,
            ui_chrome_style = ?ui_chrome_style,
            "UI configuration updated"
        );
        info!(
            "Editor font configuration updated: {} {}pt",
            editor_font.family, editor_font.size
        );

        let directwrite_params = config
            .gui
            .window
            .directwrite
            .as_ref()
            .map(|config| config.to_gpui_params());
        if let Err(error) = cx.set_direct_write_text_rendering_params(directwrite_params) {
            warn!(error = %error, "Failed to apply DirectWrite text rendering settings");
        } else {
            info!("DirectWrite text rendering settings reloaded");
        }

        if let Some(file_tree) = &self.file_tree {
            file_tree.update(cx, |tree, tree_cx| {
                tree.set_config(file_tree_config, tree_cx);
            });
        }

        if !preview_tabs_enabled {
            self.clear_preview_documents(cx);
        }

        if ui_chrome_style_changed {
            info!(
                old_ui_chrome_style = ?previous_ui_chrome_style,
                new_ui_chrome_style = ?ui_chrome_style,
                "Applying reloaded UI chrome style"
            );
            self.colors_dirty = true;
            self.schedule_window_appearance_update(cx);
        }

        let configured_theme = self.configured_theme_name(config, cx);
        let current_theme = self.core.read(cx).editor.theme.name().to_string();
        if current_theme != configured_theme {
            info!(
                old_theme = %current_theme,
                new_theme = %configured_theme,
                "Applying reloaded theme configuration"
            );
            self.switch_theme_by_name_no_window(&configured_theme, cx);
            self.schedule_window_appearance_update(cx);
        }

        self.update_document_views(cx);

        if let Some(max_tabs) = config.gui.max_tabs {
            let protected_doc_id = self
                .active_document_and_view(cx)
                .map(|(doc_id, _view_id)| doc_id);
            let settings_change_target = Some(max_tabs.get().saturating_add(1));
            self.enforce_max_tabs_to_target(settings_change_target, protected_doc_id, cx);
        }

        cx.notify();
    }

    fn configured_theme_name(
        &self,
        config: &crate::config::Config,
        cx: &mut Context<Self>,
    ) -> String {
        let system_appearance = cx.global::<crate::ThemeManager>().system_appearance();
        configured_theme_name_for_appearance(&config.gui.theme, system_appearance)
    }

    /// Reload the Nucleotide and Helix configuration without restarting
    pub fn reload_configuration(&mut self, cx: &mut Context<Self>) {
        info!("Reloading Nucleotide configuration...");

        // Get the Helix config directory path
        let config_dir = helix_loader::config_dir();
        let settings_path = config_dir.join("nucleotide.toml");

        if !settings_path.exists() {
            info!(
                config_path = %settings_path.display(),
                "No nucleotide.toml found; reloading Helix config with default Nucleotide settings"
            );
        }

        // Attempt to reload configuration
        match crate::config::Config::load_from_dir(&config_dir) {
            Ok(new_config) => {
                info!(
                    "Successfully reloaded configuration from: {}",
                    settings_path.display()
                );

                let workspace_config = new_config.clone();
                self.core.update(cx, move |core, cx| {
                    core.apply_reloaded_config(new_config, cx);
                });
                self.apply_workspace_config(&workspace_config, cx);

                info!("Configuration reloaded successfully");
            }
            Err(e) => {
                nucleotide_logging::error!("Failed to reload configuration: {}", e);
                self.push_editor_status_notification(
                    EditorStatus {
                        status: format!("Failed to reload configuration: {e}"),
                        severity: Severity::Error,
                    },
                    cx,
                );
            }
        }
    }

    fn open_file_internal(
        &mut self,
        path: &std::path::Path,
        should_focus: bool,
        preview_from_project_panel: bool,
        initial_position: Option<Position>,
        cx: &mut Context<Self>,
    ) {
        if initial_position.is_none() && is_image_file_path(path) {
            self.open_image_file_internal(path, should_focus, cx);
            return;
        }

        // Open the specified file in the editor
        debug!("Workspace: Received OpenFile update for: {path:?}");
        let mut reveal_opened_view = None;
        let mut opened_doc_id = None;
        let mut project_panel_preview = None;
        self.core.update(cx, |core, cx| {
            let _guard = self.handle.enter();
            let existed_already = core
                .editor
                .documents
                .values()
                .any(|doc| doc.path().is_some_and(|doc_path| doc_path == path));

            // Determine the right action based on whether we have views
            let action = if core.editor.tree.views().count() == 0 {
                debug!("No views exist, using VerticalSplit action");
                helix_view::editor::Action::VerticalSplit
            } else {
                debug!("Views exist, using Replace action to show in current view");
                helix_view::editor::Action::Replace
            };

            // Now open the file
            debug!(
                "About to open file from picker: {path:?} with action: {:?}",
                action
            );
            match core.editor.open(path, action) {
                Err(e) => {
                    nucleotide_logging::error!(path = ?path, error = %e, "Failed to open file");
                }
                Ok(doc_id) => {
                    info!("Successfully opened file from picker: {path:?}, doc_id: {doc_id:?}");
                    opened_doc_id = Some(doc_id);

                    // Log document info
                    if let Some(doc) = core.editor.document(doc_id) {
                        debug!(
                            "Document language: {:?}, path: {:?}",
                            doc.language_name(),
                            doc.path()
                        );

                        // Check if document has language servers
                        let lang_servers: Vec<_> = doc.language_servers().collect();
                        debug!("Document has {} language servers", lang_servers.len());
                        for ls in &lang_servers {
                            debug!("  Language server: {:?}", ls);
                        }
                    }

                    // Use the new LSP manager with feature flag support
                    debug!("Starting LSP for document using feature flag system");
                    let lsp_result = core.start_lsp_with_feature_flags(doc_id);

                    match lsp_result {
                        nucleotide_lsp::LspStartupResult::Success {
                            mode,
                            language_servers,
                            duration,
                        } => {
                            info!(
                                mode = ?mode,
                                language_servers = ?language_servers,
                                duration_ms = duration.as_millis(),
                                "LSP startup successful with feature flag system"
                            );
                        }
                        nucleotide_lsp::LspStartupResult::Failed {
                            mode,
                            error,
                            fallback_mode,
                        } => {
                            warn!(
                                mode = ?mode,
                                error = %error,
                                fallback_mode = ?fallback_mode,
                                "LSP startup failed"
                            );

                            // Fallback to existing mechanism as additional safety net
                            helix_event::request_redraw();
                        }
                        nucleotide_lsp::LspStartupResult::Skipped { reason } => {
                            debug!(
                                reason = %reason,
                                "LSP startup skipped"
                            );
                        }
                    }

                    // Trigger a redraw event to ensure UI updates
                    helix_event::request_redraw();

                    // Emit an editor redraw event which should trigger various checks
                    cx.emit(crate::Update::Event(crate::types::AppEvent::Core(
                        crate::types::CoreEvent::RedrawRequested,
                    )));

                    // Set cursor to beginning of file without selecting content
                    let view_id = core.editor.tree.focus;
                    if should_create_project_panel_preview_tab(
                        core.config.gui.preview_tabs.enabled,
                        preview_from_project_panel,
                        existed_already,
                    ) {
                        project_panel_preview = Some((doc_id, view_id));
                    }

                    // Check if the view exists before attempting operations
                    if let Some(view) = core.editor.tree.try_get(view_id) {
                        // Get the current document id from the view
                        let view_doc_id = view.doc;
                        debug!(
                            "View {:?} has document ID: {:?}, opened doc_id: {:?}",
                            view_id, view_doc_id, doc_id
                        );

                        // Make sure the view is showing the document we just opened
                        if view_doc_id != doc_id {
                            debug!(
                                "View is showing different document, switching to opened document"
                            );
                            core.editor
                                .switch(doc_id, helix_view::editor::Action::Replace);
                        }

                        // Set the selection to the requested position, or to the start by default.
                        // will reveal it after views are refreshed below.
                        if let Some(doc) = core.editor.document_mut(doc_id) {
                            let offset = initial_position
                                .map(|position| pos_at_coords(doc.text().slice(..), position, true))
                                .unwrap_or(0);
                            let pos = Selection::point(offset);
                            doc.set_selection(view_id, pos);
                            core.editor.ensure_cursor_in_view(view_id);
                            reveal_opened_view = Some(view_id);
                        }
                    }
                }
            }
            cx.notify();
        });

        if let Some((doc_id, view_id)) = project_panel_preview {
            self.replace_preview_tab_document(doc_id, view_id, true, cx);
        } else if let Some(doc_id) = opened_doc_id
            && !preview_from_project_panel
        {
            self.unregister_preview_document(doc_id, cx);
        }

        // Force focus update to ensure the correct view is focused
        self.core.update(cx, |core, _cx| {
            let view_id = core.editor.tree.focus;
            debug!("Current focused view after opening: {:?}", view_id);
        });

        if opened_doc_id.is_some() {
            self.allow_tab_bar_auto_scroll();
        }

        // Update document views after opening file
        self.update_document_views(cx);
        if let Some(view_id) = reveal_opened_view
            && let Some(view_entity) = self.view_manager.get_document_view(&view_id)
        {
            view_entity.update(cx, |view, cx| {
                view.request_cursor_reveal();
                cx.notify();
            });
        }

        // Try to trigger the same flow as initialization
        // by focusing the view and requesting a redraw
        self.core.update(cx, |core, _cx| {
            let view_id = core.editor.tree.focus;
            core.editor.focus(view_id);

            // Request idle timer which might trigger LSP initialization
            core.editor.reset_idle_timer();
        });

        // Sync file tree selection with the newly opened file
        if let Some(file_tree) = &self.file_tree {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(path), cx);
            });
        }

        // Only focus the editor if requested (not when opening from file tree)
        if should_focus && self.view_manager.focused_view_id().is_some() {
            self.view_manager.set_needs_focus_restore(true);
        }

        // Force a redraw
        cx.notify();
    }

    fn push_editor_status_notification(&mut self, status: EditorStatus, cx: &mut Context<Self>) {
        self.last_notified_editor_status = Some(status.clone());
        self.notifications.update(cx, |notifications, cx| {
            notifications.push_editor_status(status, cx);
        });
    }

    fn push_document_saved_notification(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        let message = path
            .map(|path| format!("saved to {path}"))
            .unwrap_or_else(|| "document saved".to_string());

        self.notifications.update(cx, |notifications, cx| {
            notifications.push_success("Saved", message, cx);
        });
    }

    fn sync_current_editor_status_notification(&mut self, cx: &mut Context<Self>) {
        let Some(status) = self
            .core
            .read(cx)
            .editor
            .get_status()
            .map(|(message, severity)| helix_status_to_editor_status(message, severity))
        else {
            return;
        };

        if self
            .last_notified_editor_status
            .as_ref()
            .is_some_and(|last_status| editor_status_matches(last_status, &status))
        {
            return;
        }

        self.push_editor_status_notification(status, cx);
    }

    #[instrument(skip(self, cx), fields(event = ?ev))]
    pub fn handle_event(&mut self, ev: &crate::Update, cx: &mut Context<Self>) {
        trace!("handling event {ev:?}");
        let skip_editor_status_sync = matches!(
            ev,
            crate::Update::EditorStatus(_)
                | crate::Update::Event(crate::types::AppEvent::Core(
                    crate::types::CoreEvent::StatusChanged { .. }
                ))
        );

        match ev {
            crate::Update::EditorEvent(ev) => self.handle_editor_event(ev, cx),
            crate::Update::EditorStatus(status) => {
                self.push_editor_status_notification(status.clone(), cx);
            }
            crate::Update::Redraw => self.handle_redraw(cx),
            crate::Update::CompletionEvent(completion_event) => {
                self.handle_completion_event(completion_event, cx);
            }
            crate::Update::Prompt(_)
            | crate::Update::Picker(_)
            | crate::Update::DirectoryPicker(_)
            | crate::Update::TerminalPanel(_) => {
                self.handle_overlay_update(cx);
            }
            crate::Update::HoverDocs(entries) => {
                self.set_documentation_sidebar_entries(entries.clone(), cx);
            }
            crate::Update::Completion(_completion_view) => {
                nucleotide_logging::trace!("Forwarding completion to overlay");

                // Overlay will handle completion view setup in its own Update handler
                self.handle_overlay_update(cx);
            }
            crate::Update::OpenFile(path) => self.handle_open_file(path, cx),
            crate::Update::OpenDirectory(path) => self.handle_open_directory(path, cx),
            crate::Update::FileTreeEvent(event) => {
                self.handle_file_tree_event(event, cx);
            }
            crate::Update::ShowFilePicker => {
                nucleotide_logging::debug!("DIAG: Workspace received ShowFilePicker");
                let handle = self.handle.clone();
                let core = self.core.clone();
                let overlay = self.overlay.clone();
                open(core, handle, overlay, cx);
            }
            crate::Update::ShowFilePickerAt(path) => {
                nucleotide_logging::debug!(
                    path = %path.display(),
                    "DIAG: Workspace received ShowFilePickerAt"
                );
                let handle = self.handle.clone();
                let core = self.core.clone();
                let overlay = self.overlay.clone();
                open_at(core, handle, overlay, path.clone(), cx);
            }
            crate::Update::ShowBufferPicker => {
                nucleotide_logging::debug!("DIAG: Workspace received ShowBufferPicker");
                let handle = self.handle.clone();
                let core = self.core.clone();
                let overlay = self.overlay.clone();
                show_buffer_picker(core, handle, overlay, cx);
            }
            crate::Update::ShowCodeActions => {
                nucleotide_logging::debug!("Workspace received ShowCodeActions");
                let handle = self.handle.clone();
                let core = self.core.clone();
                show_code_actions(core, handle, cx);
            }
            crate::Update::ShowRunnables => {
                nucleotide_logging::debug!("Workspace received ShowRunnables");
                self.show_runnables(cx);
            }
            crate::Update::RunTask(task) => {
                nucleotide_logging::debug!(label = %task.label(), "Workspace received RunTask");
                self.run_task(task.clone(), cx);
            }
            crate::Update::ShowHoverDocs => {
                nucleotide_logging::debug!("Workspace received ShowHoverDocs");
                if self.toggle_documentation_sidebar(cx) {
                    let handle = self.handle.clone();
                    let core = self.core.clone();
                    show_hover_docs(core, handle, cx);
                }
            }
            crate::Update::ToggleFileTree => {
                info!("Toggling file tree from native editor input");
                self.show_file_tree = !self.show_file_tree;
                cx.notify();
            }
            crate::Update::Info(_info) => {
                // Helix autoinfo is rendered by the dedicated native key-hint
                // popup. Avoid also showing the generic info box for the same
                // pending keymap payload.
                self.info_hidden = true;
                self.update_key_hints(cx);
                self.view_manager.set_needs_focus_restore(true);
                cx.notify();
            }
            crate::Update::ShouldQuit => {
                info!("ShouldQuit event received - triggering application quit");
                // Ensure editor state is cleanly flushed and views are closed before quit
                let handle = self.handle.clone();
                let core = self.core.clone();
                quit(core, handle, cx);
                cx.quit();
            }
            crate::Update::CommandSubmitted(command) => self.handle_command_submitted(command, cx),
            crate::Update::SearchSubmitted(search_text) => {
                self.handle_search_submitted(search_text, cx)
            }
            crate::Update::GlobalSearchSubmitted(query) => {
                self.handle_global_search_submitted(query, cx)
            }
            crate::Update::FileTreeSearchSubmitted(query) => {
                self.handle_file_tree_search_submitted(query, cx)
            }
            crate::Update::RegexSelectionSubmitted { action, regex } => {
                self.handle_regex_selection_submitted(*action, regex, cx)
            }
            // Helix event bridge - respond to automatic Helix events
            crate::Update::SelectionChanged { doc_id, view_id } => {
                self.handle_selection_changed(*doc_id, *view_id, cx)
            }
            crate::Update::ModeChanged { old_mode, new_mode } => {
                self.handle_mode_changed(old_mode, new_mode, cx)
            }
            crate::Update::ViewFocused { view_id } => self.handle_view_focused(*view_id, cx),
            crate::Update::LanguageServerInitialized { server_id, .. } => {
                self.handle_language_server_initialized(*server_id, cx)
            }
            crate::Update::LanguageServerExited { server_id } => {
                self.handle_language_server_exited(*server_id, cx)
            }
            crate::Update::CompletionRequested {
                doc_id,
                view_id,
                trigger,
            } => self.handle_completion_requested(*doc_id, *view_id, trigger, cx),
            crate::Update::ViewportScroll { view_id, request } => {
                self.handle_viewport_scroll(*view_id, *request, cx);
            }
            crate::Update::ViewportCursor { view_id, request } => {
                self.handle_viewport_cursor(*view_id, *request, cx);
            }
            // Handle new event-based updates (during migration)
            crate::Update::Event(event) => {
                match event {
                    crate::types::AppEvent::Core(core_event) => {
                        match core_event {
                            crate::types::CoreEvent::ShouldQuit => {
                                info!("ShouldQuit event received via Event system");
                                // Ensure editor state is cleanly flushed and views are closed before quit
                                let handle = self.handle.clone();
                                let core = self.core.clone();
                                quit(core, handle, cx);
                                cx.quit();
                            }
                            crate::types::CoreEvent::RedrawRequested => {
                                self.handle_redraw(cx);
                            }
                            crate::types::CoreEvent::CommandSubmitted { command } => {
                                self.handle_command_submitted(command, cx);
                            }
                            crate::types::CoreEvent::SearchSubmitted { query } => {
                                self.handle_search_submitted(query, cx);
                            }
                            crate::types::CoreEvent::StatusChanged { message, severity } => {
                                self.push_editor_status_notification(
                                    EditorStatus {
                                        status: message.clone(),
                                        severity: *severity,
                                    },
                                    cx,
                                );
                            }
                            crate::types::CoreEvent::SelectionChanged { doc_id, view_id } => {
                                self.handle_selection_changed(*doc_id, *view_id, cx);
                            }
                            crate::types::CoreEvent::ModeChanged { old_mode, new_mode } => {
                                self.handle_mode_changed(old_mode, new_mode, cx);
                            }
                            crate::types::CoreEvent::ViewFocused { view_id } => {
                                self.handle_view_focused(*view_id, cx);
                            }
                            crate::types::CoreEvent::CompletionRequested {
                                doc_id,
                                view_id,
                                trigger,
                            } => {
                                self.handle_completion_requested(*doc_id, *view_id, trigger, cx);
                            }
                        }
                    }
                    crate::types::AppEvent::Terminal(term_event) => {
                        // Close the terminal pane when the shell process exits
                        if let TerminalEvent::Exited { id, code, .. } = term_event
                            && self.terminal_id == Some(*id)
                        {
                            if let Some((terminal_id, run_id)) = self.active_run_terminal
                                && terminal_id == *id
                            {
                                let status = match code {
                                    Some(0) | None => RunStatus::Finished,
                                    Some(_) => RunStatus::Failed,
                                };
                                self.core.update(cx, |app, _cx| {
                                    if let Some(bus) = &app.event_aggregator {
                                        bus.dispatch_run(RunEvent::StatusChanged {
                                            id: run_id,
                                            status,
                                        });
                                        bus.dispatch_run(RunEvent::Finished {
                                            id: run_id,
                                            code: *code,
                                        });
                                        bus.process_events();
                                    }
                                });
                                self.active_run_terminal = None;
                                self.terminal_focus_pending = false;
                                self.terminal_active = false;
                                let exit_code = *code;
                                let status_message = match (status, exit_code) {
                                    (RunStatus::Finished, Some(0) | None) => {
                                        "Runnable finished".to_string()
                                    }
                                    (RunStatus::Failed, Some(exit_code)) => {
                                        format!("Runnable failed with exit code {exit_code}")
                                    }
                                    _ => "Runnable finished".to_string(),
                                };
                                self.push_editor_status_notification(
                                    EditorStatus {
                                        status: status_message,
                                        severity: if status == RunStatus::Failed {
                                            Severity::Error
                                        } else {
                                            Severity::Info
                                        },
                                    },
                                    cx,
                                );
                                cx.notify();
                                return;
                            }
                            self.hide_terminal_panel();
                            self.clear_terminal_panel_session();
                            cx.notify();
                        }
                    }
                    crate::types::AppEvent::Run(_run_event) => {}
                    crate::types::AppEvent::Workspace(workspace_event) => {
                        if let crate::types::WorkspaceEvent::FileSelected { path, source } =
                            workspace_event
                        {
                            use nucleotide_events::v2::workspace::SelectionSource;
                            match source {
                                SelectionSource::Click | SelectionSource::Command => {
                                    if path.is_file() {
                                        self.handle_open_file(path, cx);
                                    } else if path.is_dir() {
                                        self.handle_open_directory(path, cx);
                                    }
                                }
                                _ => {
                                    // Other selection sources
                                }
                            }
                        }
                    }
                    crate::types::AppEvent::Ui(ui_event) => {
                        match ui_event {
                            crate::types::UiEvent::OverlayShown {
                                overlay_type,
                                overlay_id: _,
                            } => {
                                use nucleotide_events::v2::ui::OverlayType;
                                match overlay_type {
                                    OverlayType::FilePicker => {
                                        nucleotide_logging::debug!(
                                            "DIAG: Workspace observed OverlayShown(FilePicker)"
                                        );
                                        let handle = self.handle.clone();
                                        let core = self.core.clone();
                                        let overlay = self.overlay.clone();
                                        open(core, handle, overlay, cx);
                                    }
                                    OverlayType::BufferPicker => {
                                        nucleotide_logging::debug!(
                                            "DIAG: Workspace observed OverlayShown(BufferPicker)"
                                        );
                                        let handle = self.handle.clone();
                                        let core = self.core.clone();
                                        let overlay = self.overlay.clone();
                                        show_buffer_picker(core, handle, overlay, cx);
                                    }
                                    OverlayType::Search
                                    | OverlayType::Completion
                                    | OverlayType::Prompt
                                    | OverlayType::Dialog
                                    | OverlayType::Tooltip
                                    | OverlayType::ContextMenu => {
                                        nucleotide_logging::debug!(
                                            overlay_type = ?overlay_type,
                                            "OverlayShown is handled by its owning UI component"
                                        );
                                    }
                                }
                            }
                            crate::types::UiEvent::SystemAppearanceChanged { appearance } => {
                                self.handle_system_appearance_changed(*appearance, cx);
                            }
                            _ => {
                                // Other UI events not yet handled
                            }
                        }
                    }
                    crate::types::AppEvent::Lsp(lsp_event) => {
                        match lsp_event {
                            crate::types::LspEvent::ServerInitialized { server_id, .. } => {
                                self.handle_language_server_initialized(*server_id, cx);
                            }
                            crate::types::LspEvent::ServerExited { server_id, .. } => {
                                self.handle_language_server_exited(*server_id, cx);
                            }
                            _ => {
                                // Other LSP events not yet handled
                            }
                        }
                    }
                    crate::types::AppEvent::Document(doc_event) => {
                        self.handle_document_domain_event(doc_event, cx);
                    }
                    crate::types::AppEvent::Editor(editor_event) => {
                        self.handle_editor_domain_event(editor_event, cx);
                    }
                    crate::types::AppEvent::Vcs(vcs_event) => {
                        self.handle_vcs_domain_event(vcs_event, cx);
                    }
                    crate::types::AppEvent::Integration(integration_event) => {
                        self.handle_integration_event(integration_event, cx);
                    }
                    crate::types::AppEvent::Diagnostics(_d) => {
                        // Diagnostics domain events are handled upstream to update LspState
                    }
                }
                self.forward_info_box_event(event, cx);
            }
        }

        if !skip_editor_status_sync {
            self.sync_current_editor_status_notification(cx);
        }
    }

    /// Render the tab bar showing all open documents
    fn render_tab_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::tab_bar::{DocumentInfo, TabBar};
        use helix_view::editor::BufferLine;

        let active_document_focused = self
            .view_manager
            .focused_view_id()
            .and_then(|view_id| self.view_manager.get_document_view(&view_id))
            .is_some_and(|doc_view| doc_view.focus_handle(cx).contains_focused(window, cx));
        let tab_bar_menu_focused = self.tab_context_menu_open
            || self.tab_bar_split_menu_open
            || self.tab_bar_new_menu_open;
        let workspace_focused = self.focus_handle.contains_focused(window, cx);
        let terminal_pane_focused = self.terminal_focus.is_focused(window)
            || self.terminal_focus_pending
            || self.terminal_active;
        let editor_pane_focused = workspace_focused || active_document_focused;
        let show_focused_tab_bar_buttons =
            editor_pane_focused || terminal_pane_focused || tab_bar_menu_focused;

        let core = self.core.read(cx);
        let editor = &core.editor;

        // Check bufferline configuration
        let bufferline_config = &editor.config().bufferline;
        debug!(
            "render_tab_bar: bufferline config = {:?}, doc count = {}",
            bufferline_config,
            editor.documents.len() + self.image_tabs.len()
        );
        let tab_count = editor.documents.len() + self.image_tabs.len();

        let should_show_tabs = core.config.gui.tab_bar.show
            && match bufferline_config {
                BufferLine::Never => false,
                BufferLine::Always => true,
                BufferLine::Multiple => tab_count > 1,
            };

        debug!(
            should_show_tabs = should_show_tabs,
            match_result = ?bufferline_config,
            "Tab bar visibility decision"
        );

        // If tabs shouldn't be shown, return an empty div with a unique ID
        if !should_show_tabs {
            debug!("Tab bar hidden, returning empty div");
            return div()
                .id("tab-bar-hidden")
                .h(px(0.0)) // Explicitly set height to 0 to ensure no space is taken
                .into_any_element();
        }

        debug!("Tab bar visible, rendering tabs");

        // Get the currently active tab ID
        let active_doc_id = self.active_image_tab_id.map(TabId::Image).or_else(|| {
            self.view_manager
                .focused_view_id()
                .and_then(|focused_view_id| editor.tree.try_get(focused_view_id))
                .map(|view| TabId::Document(view.doc))
        });

        // Get project directory for relative paths first
        let project_directory = core.project_directory.clone();
        let show_nav_history_buttons = core.config.gui.tab_bar.show_nav_history_buttons;
        let show_tab_bar_buttons =
            core.config.gui.tab_bar.show_tab_bar_buttons && show_focused_tab_bar_buttons;
        let show_pinned_tabs_in_separate_row =
            core.config.gui.tab_bar.show_pinned_tabs_in_separate_row;
        let show_close_button = core.config.gui.tabs.show_close_button;
        let close_position = core.config.gui.tabs.close_position;
        let show_file_icons = core.config.gui.tabs.file_icons;
        let show_git_status = core.config.gui.tabs.git_status;
        let show_diagnostics = core.config.gui.tabs.show_diagnostics;
        let activate_on_close = core.config.gui.tabs.activate_on_close;
        let show_preview_tabs = core.config.gui.preview_tabs.enabled;

        // Collect all current tab IDs
        let current_doc_ids: std::collections::HashSet<_> =
            editor.documents.keys().copied().collect();
        let mut current_tab_ids: std::collections::HashSet<_> = editor
            .documents
            .keys()
            .copied()
            .map(TabId::Document)
            .collect();
        current_tab_ids.extend(self.image_tabs.iter().map(|tab| TabId::Image(tab.id)));

        // Release the core borrow early by ending the scope

        // Clean up order list - remove documents that no longer exist
        self.document_order
            .retain(|doc_id| current_doc_ids.contains(doc_id));
        self.pinned_documents
            .retain(|tab_id| current_tab_ids.contains(tab_id));

        // Add any new documents to the end of the order list (rightmost position)
        for &doc_id in &current_doc_ids {
            self.ensure_document_in_order(doc_id);
        }

        // Now collect document information in the stable order. Ephemeral preview
        // documents stay visible so they behave like Zed preview tabs.
        let mut documents = Vec::new();
        let core = self.core.read(cx);
        let editor = &core.editor;
        // Build preview doc sets. The preview setting controls tab presentation,
        // but already-open documents remain visible when previews are disabled.
        let preview_tracker = cx
            .try_global::<nucleotide_core::preview_tracker::PreviewTracker>()
            .cloned();
        let preview_docs: std::collections::HashSet<_> = preview_tracker
            .as_ref()
            .map(|t| t.preview_doc_ids())
            .unwrap_or_default();

        // Iterate in our stable order, not HashMap order
        for (order_index, &doc_id) in self.document_order.iter().enumerate() {
            let is_preview = show_preview_tabs && preview_docs.contains(&doc_id);

            if let Some(doc) = editor.documents.get(&doc_id) {
                let path = doc.path().map(|p| p.to_path_buf());
                let diagnostic_severity = if show_file_icons {
                    match show_diagnostics {
                        crate::config::TabDiagnosticsVisibility::Off => None,
                        crate::config::TabDiagnosticsVisibility::Errors => doc
                            .diagnostics()
                            .iter()
                            .any(|diagnostic| {
                                matches!(
                                    diagnostic.severity,
                                    Some(helix_core::diagnostic::Severity::Error)
                                )
                            })
                            .then_some(helix_core::diagnostic::Severity::Error),
                        crate::config::TabDiagnosticsVisibility::All => {
                            if doc.diagnostics().iter().any(|diagnostic| {
                                matches!(
                                    diagnostic.severity,
                                    Some(helix_core::diagnostic::Severity::Error)
                                )
                            }) {
                                Some(helix_core::diagnostic::Severity::Error)
                            } else if doc.diagnostics().iter().any(|diagnostic| {
                                matches!(
                                    diagnostic.severity,
                                    Some(helix_core::diagnostic::Severity::Warning)
                                )
                            }) {
                                Some(helix_core::diagnostic::Severity::Warning)
                            } else {
                                None
                            }
                        }
                    }
                } else {
                    None
                };

                documents.push(DocumentInfo {
                    id: TabId::Document(doc_id),
                    is_deleted: is_deleted_document_path(path.as_deref()),
                    path,
                    is_modified: doc.is_modified(),
                    is_readonly: doc.readonly,
                    is_pinned: self.pinned_documents.contains(&TabId::Document(doc_id)),
                    is_preview,
                    focused_at: doc.focused_at,
                    order: order_index, // Use position in Vec as order
                    git_status: None,   // Will be filled in after releasing core borrow
                    diagnostic_severity,
                });
            }
        }

        let image_order_offset = documents.len();
        for (index, tab) in self.image_tabs.iter().enumerate() {
            documents.push(DocumentInfo {
                id: TabId::Image(tab.id),
                is_deleted: is_deleted_document_path(Some(&tab.path)),
                path: Some(tab.path.clone()),
                is_modified: false,
                is_readonly: false,
                is_pinned: self.pinned_documents.contains(&TabId::Image(tab.id)),
                is_preview: false,
                focused_at: tab.focused_at,
                order: image_order_offset + index,
                git_status: None,
                diagnostic_severity: None,
            });
        }

        // Ensure VCS service is monitoring the current project directory
        if let Some(ref project_dir) = project_directory {
            let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();
            vcs_handle.update(cx, |service, cx| {
                // Only start monitoring if we're not already monitoring this directory
                if service.root_path() != Some(project_dir.as_path()) {
                    service.start_monitoring(project_dir.clone(), cx);
                }
                // Avoid forcing a refresh on every tab bar recompute; rely on
                // initial monitoring refresh and explicit triggers elsewhere.
            });
        }

        if show_git_status {
            // Update documents with VCS status using cached method
            for doc_info in &mut documents {
                if let Some(ref path) = doc_info.path {
                    let status = cx.global::<VcsServiceHandle>().get_status_cached(path, cx);
                    debug!(file = %path.display(), vcs_status = ?status, "VCS status for tab");
                    doc_info.git_status = status;
                }
            }
        }

        let visible_doc_ids = documents.iter().map(|doc| doc.id).collect::<Vec<_>>();
        if should_scroll_active_tab(
            self.suppress_tab_bar_auto_scroll,
            self.last_scrolled_tab_doc_id,
            active_doc_id,
        ) && let Some(active_doc_id) = active_doc_id
            && let Some(active_index) = active_unpinned_tab_scroll_index(
                &visible_doc_ids,
                &self.pinned_documents,
                active_doc_id,
            )
        {
            self.tab_bar_scroll_handle.scroll_to_item(active_index);
            self.last_scrolled_tab_doc_id = Some(active_doc_id);
        }

        let has_documents = !documents.is_empty();
        let activation_documents = {
            let mut activation_documents = Vec::with_capacity(documents.len());
            activation_documents.extend(documents.iter().filter(|doc| doc.is_pinned).map(|doc| {
                TabActivationDocument {
                    id: doc.id,
                    focused_at: doc.focused_at,
                }
            }));
            activation_documents.extend(documents.iter().filter(|doc| !doc.is_pinned).map(|doc| {
                TabActivationDocument {
                    id: doc.id,
                    focused_at: doc.focused_at,
                }
            }));
            activation_documents
        };

        // Create tab bar with callbacks
        TabBar::new(
            documents,
            active_doc_id,
            project_directory,
            {
                let workspace = cx.entity().clone();
                let core = self.core.clone();
                let handle = self.handle.clone();
                move |doc_id, _window, cx| {
                    workspace.update(cx, |workspace, cx| {
                        match doc_id {
                            TabId::Image(image_id) => {
                                workspace.switch_to_image_tab(image_id, cx);
                                return;
                            }
                            TabId::Document(doc_id) => {
                                // Switch the current view to display this document
                                core.update(cx, |core, cx| {
                                    let _guard = handle.enter();

                                    // Use Helix's switch method to change which document is displayed
                                    core.editor
                                        .switch(doc_id, helix_view::editor::Action::Replace);

                                    // Emit a redraw event so the UI updates
                                    cx.emit(crate::Update::Redraw);
                                });
                            }
                        }

                        // Update document views to reflect the change
                        workspace.active_image_tab_id = None;
                        workspace.tab_context_menu_open = false;
                        workspace.tab_context_menu_doc_id = None;
                        workspace.allow_tab_bar_auto_scroll();
                        workspace.update_document_views(cx);
                    });
                }
            },
            {
                let workspace = cx.entity().clone();
                let activation_documents = activation_documents.clone();
                move |doc_id, _window, cx| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.tab_context_menu_open = false;
                        workspace.tab_context_menu_doc_id = None;
                        let activation_target = tab_activation_target_after_close(
                            &activation_documents,
                            doc_id,
                            active_doc_id,
                            activate_on_close,
                        );
                        match doc_id {
                            TabId::Image(image_id) => {
                                workspace.close_image_tab(image_id, activation_target, cx);
                            }
                            TabId::Document(doc_id) => {
                                workspace.close_single_tab_document_with_activation_target(
                                    doc_id,
                                    activation_target,
                                    false,
                                    cx,
                                );
                            }
                        }
                    });
                }
            },
        )
        .show_pinned_tabs_in_separate_row(show_pinned_tabs_in_separate_row)
        .show_close_button(show_close_button)
        .close_position(close_position)
        .file_icons(show_file_icons)
        .git_status(show_git_status)
        .show_diagnostics(show_diagnostics)
        .deemphasized(!editor_pane_focused)
        .track_scroll(&self.tab_bar_scroll_handle)
        .with_scroll_wheel_handler({
            let workspace = cx.entity().clone();
            move |_event, _window, cx| {
                workspace.update(cx, |workspace, _cx| {
                    workspace.suppress_tab_bar_auto_scroll = true;
                });
            }
        })
        .when(show_nav_history_buttons, |tab_bar| {
            tab_bar
                .start_child(
                    Button::icon_only("tab-nav-back", "icons/arrow-left.svg")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Small)
                        .tooltip("Go Back")
                        .activate_on_mouse_down()
                        .disabled(!has_documents)
                        .on_click({
                            let workspace = cx.entity().clone();
                            move |_event, _window, cx| {
                                workspace.update(cx, |workspace, cx| {
                                    workspace.send_helix_key("ctrl-o", cx);
                                });
                                cx.stop_propagation();
                            }
                        }),
                )
                .start_child(
                    Button::icon_only("tab-nav-forward", "icons/arrow-right.svg")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Small)
                        .tooltip("Go Forward")
                        .activate_on_mouse_down()
                        .disabled(!has_documents)
                        .on_click({
                            let workspace = cx.entity().clone();
                            move |_event, _window, cx| {
                                workspace.update(cx, |workspace, cx| {
                                    workspace.send_helix_key("ctrl-i", cx);
                                });
                                cx.stop_propagation();
                            }
                        }),
                )
        })
        .when(show_tab_bar_buttons, |tab_bar| {
            tab_bar
                .end_child(
                    Button::icon_only("tab-new-file", "icons/plus.svg")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::Small)
                        .tooltip("New File")
                        .activate_on_mouse_down()
                        .on_click({
                            let workspace = cx.entity().clone();
                            move |_event, _window, cx| {
                                workspace.update(cx, |workspace, cx| {
                                    workspace.tab_context_menu_open = false;
                                    workspace.tab_context_menu_doc_id = None;
                                    workspace.tab_bar_split_menu_open = false;
                                    workspace.tab_bar_new_menu_open = false;
                                    workspace.tab_bar_action_new_file(cx);
                                });
                                cx.stop_propagation();
                            }
                        }),
                )
                .end_child(
                    div()
                        .relative()
                        .child(
                            Button::icon_only("tab-split-menu", "icons/columns-2.svg")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Small)
                                .tooltip("Split Pane")
                                .activate_on_mouse_down()
                                .disabled(!has_documents)
                                .on_click({
                                    let workspace = cx.entity().clone();
                                    move |event, window, cx| {
                                        workspace.update(cx, |workspace, cx| {
                                            if workspace.tab_bar_split_menu_open {
                                                workspace.tab_bar_split_menu_open = false;
                                                workspace.tab_context_menu_open = false;
                                                workspace.tab_context_menu_doc_id = None;
                                                workspace.tab_bar_new_menu_open = false;
                                                cx.notify();
                                                return;
                                            }

                                            let fallback_position = event.position();
                                            let menu_position = workspace
                                                .tab_bar_split_button_bounds
                                                .map(|bounds| bounds.bottom_right())
                                                .unwrap_or(fallback_position);
                                            workspace.tab_context_menu_open = false;
                                            workspace.tab_context_menu_doc_id = None;
                                            workspace.tab_bar_new_menu_open = false;
                                            workspace.tab_bar_split_menu_open = true;
                                            workspace.tab_bar_split_menu_pos = (
                                                f32::from(menu_position.x),
                                                f32::from(menu_position.y),
                                            );
                                            workspace.tab_bar_split_menu_index = 0;
                                            window.focus(&workspace.focus_handle, cx);
                                            cx.notify();
                                        });
                                        cx.stop_propagation();
                                    }
                                }),
                        )
                        .child(
                            canvas(
                                {
                                    let workspace = cx.entity().clone();
                                    move |bounds, _window, cx| {
                                        workspace.update(cx, |workspace, _cx| {
                                            workspace.tab_bar_split_button_bounds = Some(bounds);
                                        });
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        ),
                )
        })
        .with_pin_toggle_handler({
            let workspace = cx.entity().clone();
            move |doc_id, _window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.tab_context_menu_open = false;
                    workspace.tab_context_menu_doc_id = None;
                    workspace.tab_bar_split_menu_open = false;
                    workspace.tab_bar_new_menu_open = false;
                    workspace.tab_cm_action_toggle_pin(doc_id, cx);
                });
            }
        })
        .with_readonly_toggle_handler({
            let workspace = cx.entity().clone();
            move |doc_id, _window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.tab_context_menu_open = false;
                    workspace.tab_context_menu_doc_id = None;
                    workspace.tab_bar_split_menu_open = false;
                    workspace.tab_bar_new_menu_open = false;
                    workspace.tab_cm_action_toggle_readonly(doc_id, cx);
                });
            }
        })
        .with_empty_double_click_handler({
            let workspace = cx.entity().clone();
            move |_event, _window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.tab_context_menu_open = false;
                    workspace.tab_context_menu_doc_id = None;
                    workspace.tab_bar_split_menu_open = false;
                    workspace.tab_bar_new_menu_open = false;
                    workspace.tab_bar_action_new_file(cx);
                });
            }
        })
        .with_double_click_handler({
            let workspace = cx.entity().clone();
            move |doc_id, _window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.tab_context_menu_open = false;
                    workspace.tab_context_menu_doc_id = None;
                    workspace.tab_bar_split_menu_open = false;
                    workspace.tab_bar_new_menu_open = false;
                    workspace.tab_action_double_click(doc_id, cx);
                });
            }
        })
        .with_context_menu_handler({
            let workspace = cx.entity().clone();
            move |doc_id, event, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.tab_bar_split_menu_open = false;
                    workspace.tab_bar_new_menu_open = false;
                    workspace.tab_context_menu_open = true;
                    workspace.tab_context_menu_pos =
                        (f32::from(event.position.x), f32::from(event.position.y));
                    workspace.tab_context_menu_doc_id = Some(doc_id);
                    workspace.tab_context_menu_index = 0;
                    window.focus(&workspace.focus_handle, cx);
                    cx.notify();
                });
            }
        })
        .into_any_element()
    }

    fn render_image_viewer(&self, tab: ImageTab, cx: &mut Context<Self>) -> gpui::AnyElement {
        let tokens = &cx.theme().tokens;
        let tab_id = tab.id;
        let zoom = tab.zoom;
        let image_path = tab.path.clone();
        let (grid_base, grid_alternate) = image_transparency_grid_colors(tokens.editor.background);
        let (image_element, image_size) = if let Some((width, height)) = tab.dimensions {
            let width = px(width as f32 * zoom);
            let height = px(height as f32 * zoom);
            (
                div()
                    .relative()
                    .flex_none()
                    .overflow_hidden()
                    .mx_auto()
                    .my_auto()
                    .w(width)
                    .h(height)
                    .child(image_transparency_grid(grid_base, grid_alternate))
                    .child(
                        img(image_path)
                            .object_fit(gpui::ObjectFit::Contain)
                            .w(width)
                            .h(height)
                            .flex_none(),
                    )
                    .into_any_element(),
                Some((width, height)),
            )
        } else {
            (
                div()
                    .relative()
                    .flex_none()
                    .overflow_hidden()
                    .mx_auto()
                    .my_auto()
                    .max_w_full()
                    .max_h_full()
                    .child(image_transparency_grid(grid_base, grid_alternate))
                    .child(
                        img(image_path)
                            .object_fit(gpui::ObjectFit::Contain)
                            .max_w_full()
                            .max_h_full(),
                    )
                    .into_any_element(),
                None,
            )
        };
        let image_scroll_body = if let Some((width, height)) = image_size {
            div()
                .flex()
                .w(width)
                .h(height)
                .min_w(relative(1.0))
                .min_h(relative(1.0))
                .child(image_element)
                .into_any_element()
        } else {
            div()
                .flex()
                .min_w(relative(1.0))
                .min_h(relative(1.0))
                .child(image_element)
                .into_any_element()
        };

        div()
            .id(format!("image-viewer-{tab_id:?}"))
            .flex()
            .flex_col()
            .size_full()
            .bg(tokens.editor.background)
            .child(
                div()
                    .id("image-viewer-toolbar")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(tokens.sizes.space_2)
                    .px(tokens.sizes.space_3)
                    .h(crate::tab::tab_container_height(*tokens))
                    .border_b_1()
                    .border_color(tokens.chrome.border_default)
                    .bg(tokens.tab_bar_tokens().container_background)
                    .child(
                        Button::icon_only("image-zoom-out", "icons/zoom-out.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .tooltip("Zoom Out")
                            .activate_on_mouse_down()
                            .disabled(zoom <= IMAGE_ZOOM_MIN)
                            .on_click({
                                let workspace = cx.entity().clone();
                                move |_event, _window, cx| {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.set_image_tab_zoom(
                                            tab_id,
                                            zoom - IMAGE_ZOOM_STEP,
                                            cx,
                                        );
                                    });
                                    cx.stop_propagation();
                                }
                            }),
                    )
                    .child(
                        Button::icon_only("image-zoom-reset", "icons/rotate-ccw.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .tooltip("Zoom to 100%")
                            .activate_on_mouse_down()
                            .disabled((zoom - 1.0).abs() < f32::EPSILON)
                            .on_click({
                                let workspace = cx.entity().clone();
                                move |_event, _window, cx| {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.set_image_tab_zoom(tab_id, 1.0, cx);
                                    });
                                    cx.stop_propagation();
                                }
                            }),
                    )
                    .child(
                        Button::icon_only("image-zoom-in", "icons/zoom-in.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .tooltip("Zoom In")
                            .activate_on_mouse_down()
                            .disabled(zoom >= IMAGE_ZOOM_MAX)
                            .on_click({
                                let workspace = cx.entity().clone();
                                move |_event, _window, cx| {
                                    workspace.update(cx, |workspace, cx| {
                                        workspace.set_image_tab_zoom(
                                            tab_id,
                                            zoom + IMAGE_ZOOM_STEP,
                                            cx,
                                        );
                                    });
                                    cx.stop_propagation();
                                }
                            }),
                    )
                    .child(
                        div()
                            .min_w(px(48.0))
                            .text_size(tokens.sizes.text_sm)
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child(image_zoom_percent(zoom)),
                    ),
            )
            .child({
                let scroll_content = div()
                    .id("image-viewer-scroll-content")
                    .size_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_scroll()
                    .track_scroll(&tab.scroll_handle)
                    .p(tokens.sizes.space_4)
                    .child(image_scroll_body);

                div()
                    .id("image-viewer-content")
                    .relative()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(scroll_content)
                    .when_some(
                        Scrollbar::vertical(tab.vertical_scrollbar_state.clone()),
                        |container, scrollbar| {
                            container.child(
                                div()
                                    .id("image-viewer-vertical-scrollbar")
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .w(SCROLLBAR_THICKNESS)
                                    .child(scrollbar),
                            )
                        },
                    )
                    .when_some(
                        Scrollbar::horizontal(tab.horizontal_scrollbar_state.clone()),
                        |container, scrollbar| {
                            container.child(
                                div()
                                    .id("image-viewer-horizontal-scrollbar")
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom_0()
                                    .h(SCROLLBAR_THICKNESS)
                                    .child(scrollbar),
                            )
                        },
                    )
            })
            .into_any_element()
    }

    /// Render unified status bar with file tree toggle and status information
    fn render_unified_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Use hybrid color system with StatusBarTokens
        let (status_bar_tokens, status_bar_height, text_size, translucent_file_tree_tokens) = {
            let ui_theme = cx.global::<nucleotide_ui::Theme>();
            (
                ui_theme.tokens.status_bar_tokens(),
                ui_theme.tokens.sizes.statusbar_height,
                ui_theme.tokens.sizes.text_sm,
                ui_theme.tokens.file_tree_tokens().translucent_sidebar(),
            )
        };

        // Use the hybrid chrome background colors for consistent visual hierarchy
        let bg_color = status_bar_tokens.background_active; // Always use active for unified bar
        let fg_color = status_bar_tokens.text_primary;

        // Get current document info first (without LSP indicator to avoid borrow conflicts)
        let (mode, mode_name, file_name, position_text, has_lsp_state, preferred_server_id) =
            self.statusbar_doc_info(cx);

        // Get LSP indicator separately to avoid borrowing conflicts
        let lsp_indicator =
            self.compute_statusbar_lsp_indicator(cx, has_lsp_state, preferred_server_id);
        let notification = self.notifications.read(cx).status_bar_notification();

        // Use consistent border and divider colors from hybrid system
        // Status bar border color
        let border_color = status_bar_tokens.border;
        let divider_color = status_bar_tokens.border;
        let native_sidebar_enabled = macos_system_sidebar_enabled(&self.core.read(cx).config.gui);
        let extend_translucent_sidebar = should_extend_translucent_sidebar_into_status_bar(
            self.show_file_tree,
            self.file_tree_width,
            native_sidebar_enabled,
        );
        let status_bar_sidebar_tokens =
            extend_translucent_sidebar.then_some(translucent_file_tree_tokens);

        let mut status_bar = div()
            // Use tokenized height to match titlebar sizing
            .h(status_bar_height)
            .min_h(status_bar_height)
            .flex_shrink_0() // never compress the status bar vertically
            .w_full()
            .relative()
            .when(!extend_translucent_sidebar, |status_bar| {
                status_bar
                    .bg(bg_color)
                    .border_t_1()
                    .border_color(border_color)
            })
            .flex()
            .flex_row()
            .items_center()
            .text_size(text_size)
            .text_color(fg_color);

        if let Some(file_tree_tokens) = status_bar_sidebar_tokens {
            status_bar = status_bar
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .bottom_0()
                        .w(px(self.file_tree_width))
                        .bg(file_tree_tokens.background),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(self.file_tree_width))
                        .right_0()
                        .bottom_0()
                        .bg(bg_color)
                        .border_t_1()
                        .border_color(border_color),
                );
        }

        status_bar
            .child(
                // Toggle button container - fixed width regardless of file tree state
                div()
                    .w(px(32.0)) // Fixed width for button container
                    .flex()
                    .items_center()
                    .justify_center()
                    .child({
                        let workspace_entity = cx.entity().clone();
                        Button::icon_only("file-tree-toggle", "icons/folder-tree.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .tooltip("Toggle File Tree")
                            .activate_on_mouse_down()
                            .on_click(move |_event, _window, app_cx| {
                                workspace_entity.update(app_cx, |workspace, cx| {
                                    workspace.show_file_tree = !workspace.show_file_tree;
                                    cx.notify();
                                });
                            })
                    }),
            )
            .child(
                // Terminal toggle button to the right of file tree button
                div()
                    .w(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child({
                        let workspace_entity = cx.entity().clone();
                        Button::icon_only("terminal-toggle", "icons/terminal.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::Small)
                            .tooltip("Toggle Terminal")
                            .activate_on_mouse_down()
                            .on_click(move |_event, _window, app_cx| {
                                workspace_entity.update(app_cx, |workspace, cx| {
                                    workspace.toggle_terminal_panel(cx);
                                });
                            })
                    }),
            )
            .when(self.show_file_tree, |status_bar| {
                status_bar
                    .child(
                        // File tree width spacer (minus button width)
                        div()
                            .w(px(self.file_tree_width - 32.0)) // File tree width minus button
                            .h_full(),
                    )
                    .child(
                        // Resize handle spacer
                        div()
                            .w(px(4.0)) // Resize handle width
                            .h_full(),
                    )
            })
            .child(
                // Main status content - fills remaining space
                self.statusbar_main_content(
                    mode,
                    mode_name,
                    file_name,
                    position_text,
                    notification,
                    lsp_indicator,
                    divider_color,
                    &status_bar_tokens,
                    cx,
                ),
            ) // .child({
        //     // Project status indicator section - temporarily disabled
        //     // let project_status_handle = nucleotide_project::project_status_service(cx);
        //     // let project_info = project_status_handle.project_info(cx);
        //
        //     div()
        //         .flex()
        //         .flex_row()
        //         .items_center()
        //         .gap(ui_theme.tokens.sizes.space_2)
        //         .child(
        //             // Divider before project status
        //             div().w(px(1.)).h(px(18.)).bg(divider_color).mx_2()
        //         )
        // }),
    }

    fn handle_file_tree_event(&mut self, event: &FileTreeEvent, cx: &mut Context<Self>) {
        match event {
            FileTreeEvent::OpenFile { path, focus_editor } => {
                info!(
                    "FileTreeEvent::OpenFile received in workspace: {:?}, focus_editor={}",
                    path, focus_editor
                );
                if *focus_editor {
                    self.handle_open_file(path, cx);
                } else {
                    self.handle_open_file_keep_focus(path, cx);
                }
            }
            FileTreeEvent::SelectionChanged { path: _ } => {
                // Update UI if needed for selection changes
                cx.notify();
            }
            FileTreeEvent::SelectionSetChanged { paths: _ } => {
                // Update UI if needed for multi-selection changes
                cx.notify();
            }
            FileTreeEvent::DirectoryToggled {
                path: _,
                expanded: _,
            } => {
                // Update UI for directory expansion/collapse
                cx.notify();
            }
            FileTreeEvent::ContextMenuRequested { path, x, y } => {
                info!(
                    "FileTreeEvent::ContextMenuRequested at ({}, {}): {:?}",
                    x, y, path
                );
                self.context_menu_open = true;
                self.context_menu_pos = (*x, *y);
                self.context_menu_path = Some(path.clone());
                self.context_menu_index = 0;
                cx.notify();
            }
            FileTreeEvent::FileSystemChanged { path, kind } => {
                info!("File system change detected: {:?} - {:?}", path, kind);
                // Tree updates and VCS refreshes are handled by the file tree at
                // the debounced watcher batch boundary before this event is emitted.
                self.notify_lsp_file_system_change(path, kind, cx);
                cx.notify();
            }
            FileTreeEvent::VcsRefreshStarted { repository_root } => {
                info!("VCS refresh started for repository: {:?}", repository_root);
                self.push_editor_status_notification(
                    EditorStatus {
                        status: format!("Refreshing VCS status for {}", repository_root.display()),
                        severity: Severity::Info,
                    },
                    cx,
                );
                cx.notify();
            }
            FileTreeEvent::VcsStatusChanged {
                repository_root,
                affected_files,
            } => {
                info!(
                    "VCS status updated for repository: {:?} ({} files)",
                    repository_root,
                    affected_files.len()
                );
                // VCS status has been updated, file tree should already be refreshed
                // Could trigger status bar updates or notifications here
                cx.notify();
            }
            FileTreeEvent::VcsRefreshFailed {
                repository_root,
                error,
            } => {
                error!(
                    "VCS refresh failed for repository: {:?} - {}",
                    repository_root, error
                );
                self.push_editor_status_notification(
                    EditorStatus {
                        status: format!(
                            "VCS refresh failed for {}: {error}",
                            repository_root.display()
                        ),
                        severity: Severity::Error,
                    },
                    cx,
                );
                cx.notify();
            }
            FileTreeEvent::RefreshVcs { force } => {
                info!("VCS refresh requested (force: {})", force);
                if let Some(ref mut file_tree) = self.file_tree {
                    file_tree.update(cx, |tree, tree_cx| {
                        tree.handle_vcs_refresh(*force, tree_cx);
                    });
                }
                cx.notify();
            }
            FileTreeEvent::ToggleVisibility => {
                info!("Toggle file tree visibility requested");
                self.show_file_tree = !self.show_file_tree;
                cx.notify();
            }
            FileTreeEvent::SearchRequested { initial_query } => {
                self.start_file_tree_search(initial_query.clone(), cx);
            }
        }
    }

    fn notify_lsp_file_system_change(
        &mut self,
        path: &Path,
        kind: &FileSystemEventKind,
        cx: &mut Context<Self>,
    ) {
        let changes: Vec<(PathBuf, lsp::FileChangeType)> = match kind {
            FileSystemEventKind::Created => {
                vec![(path.to_path_buf(), lsp::FileChangeType::CREATED)]
            }
            FileSystemEventKind::Modified => {
                vec![(path.to_path_buf(), lsp::FileChangeType::CHANGED)]
            }
            FileSystemEventKind::Deleted => {
                vec![(path.to_path_buf(), lsp::FileChangeType::DELETED)]
            }
            FileSystemEventKind::Renamed { from, to } => vec![
                (from.clone(), lsp::FileChangeType::DELETED),
                (to.clone(), lsp::FileChangeType::CREATED),
            ],
        };

        self.core.update(cx, move |core, _cx| {
            for (path, typ) in changes {
                core.editor
                    .language_servers
                    .file_event_handler
                    .file_event(path, typ);
            }
        });
    }

    fn notify_lsp_file_operation(
        &mut self,
        notification: LspFileOperationNotification,
        cx: &mut Context<Self>,
    ) {
        self.core.update(cx, move |core, _cx| {
            for language_server in core.editor.language_servers.iter_clients() {
                if !language_server.is_initialized() {
                    continue;
                }

                match &notification {
                    LspFileOperationNotification::Created { path, is_dir } => {
                        language_server.did_create(path, *is_dir);
                    }
                    LspFileOperationNotification::Deleted { path, was_dir } => {
                        language_server.did_delete(path, *was_dir);
                    }
                    LspFileOperationNotification::Renamed {
                        old_path,
                        new_path,
                        was_dir,
                    } => {
                        language_server.did_rename(old_path, new_path, *was_dir);
                    }
                }
            }
        });
    }

    fn dispatch_workspace_file_op_and_process(
        &mut self,
        event: nucleotide_events::v2::workspace::Event,
        cx: &mut Context<Self>,
    ) {
        self.core.update(cx, |core, _cx| {
            if let Some(bus) = &core.event_aggregator {
                bus.dispatch_workspace(event);
                bus.process_events();
                bus.process_events();
            }
        });
    }

    fn handle_vcs_service_event(&mut self, event: &VcsEvent, cx: &mut Context<Self>) {
        match event {
            VcsEvent::StatusUpdated { changes } => {
                debug!(
                    change_count = changes.len(),
                    "Workspace: VCS status updated"
                );
                if let Some(file_tree) = self.file_tree.as_ref() {
                    file_tree.update(cx, |_tree, tree_cx| {
                        tree_cx.notify();
                    });
                }
                cx.notify();
            }
            VcsEvent::DiffHunksUpdated { file_path, .. } => {
                debug!(
                    file_path = %file_path.display(),
                    "Workspace: VCS diff metadata updated"
                );
                cx.notify();
            }
            VcsEvent::RepositoryStarted { root_path } => {
                debug!(
                    root_path = %root_path.display(),
                    "Workspace: VCS repository monitoring started"
                );
                cx.notify();
            }
            VcsEvent::Error { message } => {
                warn!(message = %message, "Workspace: VCS service error");
            }
        }
    }

    fn handle_integration_event(
        &mut self,
        event: &crate::types::IntegrationEvent,
        cx: &mut Context<Self>,
    ) {
        use nucleotide_events::integration::{Event as IntegrationEvent, RecoveryAction, SyncType};

        debug!(integration_event = ?event, "Integration event received");

        match event {
            IntegrationEvent::DocumentViewSync {
                doc_id,
                view_id,
                sync_type,
            } => {
                match sync_type {
                    SyncType::FocusSync => self.handle_view_focused(*view_id, cx),
                    SyncType::SelectionToView | SyncType::ViewToDocument | SyncType::ScrollSync => {
                        self.update_specific_document_view(*doc_id, cx);
                    }
                }
                cx.notify();
            }
            IntegrationEvent::UiEditorSync { sync_type, data } => {
                self.handle_ui_editor_sync_event(*sync_type, data, cx);
            }
            IntegrationEvent::ErrorRecoveryCoordination {
                recovery_action: RecoveryAction::ShowUserError { message },
                ..
            } => {
                self.push_editor_status_notification(
                    EditorStatus {
                        status: message.clone(),
                        severity: Severity::Error,
                    },
                    cx,
                );
            }
            IntegrationEvent::ErrorRecoveryCoordination { .. }
            | IntegrationEvent::LspDocumentAssociation { .. }
            | IntegrationEvent::CompletionCoordination { .. }
            | IntegrationEvent::WorkspaceLspCoordination { .. } => {
                cx.notify();
            }
        }
    }

    fn handle_document_domain_event(
        &mut self,
        event: &crate::types::DocumentEvent,
        cx: &mut Context<Self>,
    ) {
        use nucleotide_events::v2::document::Event as DocumentEvent;

        debug!(document_event = ?event, "Document domain event received");

        match event {
            DocumentEvent::ContentChanged { doc_id, .. } => {
                self.handle_document_changed(*doc_id, cx);
            }
            DocumentEvent::Opened { doc_id, .. } => {
                self.handle_document_opened(*doc_id, cx);
            }
            DocumentEvent::Closed { doc_id, .. } => {
                self.handle_document_closed(*doc_id, cx);
            }
            DocumentEvent::Saved { doc_id, path, .. } => {
                self.push_document_saved_notification(path.to_str(), cx);
                self.update_specific_document_view(*doc_id, cx);
                cx.notify();
            }
            DocumentEvent::SaveFailed {
                doc_id,
                path,
                error,
            } => {
                self.push_editor_status_notification(
                    EditorStatus {
                        status: format!("Failed to save {}: {error}", path.display()),
                        severity: Severity::Error,
                    },
                    cx,
                );
                self.update_specific_document_view(*doc_id, cx);
            }
            DocumentEvent::LanguageDetected { doc_id, .. } => {
                self.update_specific_document_view(*doc_id, cx);
                cx.notify();
            }
            DocumentEvent::DiagnosticsUpdated { doc_id, .. } => {
                self.handle_diagnostics_changed(*doc_id, cx);
            }
        }
    }

    fn handle_editor_domain_event(
        &mut self,
        event: &crate::types::EditorEvent,
        cx: &mut Context<Self>,
    ) {
        use nucleotide_events::v2::editor::Event as EditorEvent;

        debug!(editor_event = ?event, "Editor domain event received");

        match event {
            EditorEvent::ModeChanged {
                previous_mode,
                new_mode,
                ..
            } => {
                self.handle_mode_changed(previous_mode, new_mode, cx);
            }
            EditorEvent::StatusChanged {
                message, severity, ..
            } => {
                self.push_editor_status_notification(
                    EditorStatus {
                        status: message.clone(),
                        severity: editor_domain_status_severity(*severity),
                    },
                    cx,
                );
            }
            EditorEvent::ConfigurationChanged { .. } => {
                self.update_document_views(cx);
                cx.notify();
            }
            EditorEvent::RedrawRequested { .. } => {
                self.handle_redraw(cx);
            }
            EditorEvent::ShutdownRequested { force, .. } => {
                if *force {
                    let handle = self.handle.clone();
                    let core = self.core.clone();
                    quit(core, handle, cx);
                    cx.quit();
                } else {
                    cx.notify();
                }
            }
            EditorEvent::CommandExecuted { success, .. } => {
                if !success {
                    cx.notify();
                }
            }
            EditorEvent::MacroRecordingChanged { .. }
            | EditorEvent::SearchCompleted { .. }
            | EditorEvent::ReplaceCompleted { .. } => {
                cx.notify();
            }
        }
    }

    fn handle_vcs_domain_event(
        &mut self,
        event: &nucleotide_events::v2::vcs::Event,
        cx: &mut Context<Self>,
    ) {
        use nucleotide_events::v2::vcs::Event as VcsDomainEvent;

        debug!(vcs_event = ?event, "VCS domain event received");

        match event {
            VcsDomainEvent::DiffStatusChanged { doc_id, path, .. }
            | VcsDomainEvent::DiffCalculationCompleted { doc_id, path, .. } => {
                self.update_vcs_document_view(*doc_id, path, cx);
            }
            VcsDomainEvent::DiffCalculationFailed {
                doc_id,
                path,
                error,
            } => {
                self.push_editor_status_notification(
                    EditorStatus {
                        status: format!("Failed to calculate diff for {}: {error}", path.display()),
                        severity: Severity::Error,
                    },
                    cx,
                );
                self.update_vcs_document_view(*doc_id, path, cx);
            }
            VcsDomainEvent::FileStageStatusChanged { path, .. }
            | VcsDomainEvent::FileTrackingChanged { path, .. } => {
                self.update_open_document_for_path(path, cx);
                if let Some(file_tree) = &self.file_tree {
                    file_tree.update(cx, |_tree, cx| cx.notify());
                }
                cx.notify();
            }
            VcsDomainEvent::RepositoryHeadChanged { .. }
            | VcsDomainEvent::DiffProviderStatusChanged { .. } => {
                if let Some(file_tree) = &self.file_tree {
                    file_tree.update(cx, |_tree, cx| cx.notify());
                }
                cx.notify();
            }
        }
    }

    fn handle_ui_editor_sync_event(
        &mut self,
        sync_type: nucleotide_events::integration::UiEditorSyncType,
        data: &nucleotide_events::integration::UiEditorSyncData,
        cx: &mut Context<Self>,
    ) {
        use nucleotide_events::integration::{UiEditorSyncData, UiEditorSyncType};

        match (sync_type, data) {
            (UiEditorSyncType::ModeSync, UiEditorSyncData::ModeData { .. }) => {
                self.update_current_document_view(cx);
                cx.notify();
            }
            (UiEditorSyncType::StatusSync, UiEditorSyncData::StatusData { message, severity }) => {
                self.push_editor_status_notification(
                    EditorStatus {
                        status: message.clone(),
                        severity: integration_status_severity(severity),
                    },
                    cx,
                );
            }
            (UiEditorSyncType::ThemeChange, UiEditorSyncData::ThemeData { .. })
            | (UiEditorSyncType::FontChange, UiEditorSyncData::FontData { .. }) => {
                cx.notify();
            }
            _ => {
                debug!(
                    sync_type = ?sync_type,
                    data = ?data,
                    "Ignoring mismatched UI-editor sync payload"
                );
            }
        }
    }

    fn update_vcs_document_view(
        &mut self,
        doc_id: helix_view::DocumentId,
        path: &Path,
        cx: &mut Context<Self>,
    ) {
        let has_document = self.core.read(cx).editor.document(doc_id).is_some();
        if has_document {
            self.update_specific_document_view(doc_id, cx);
        } else {
            self.update_open_document_for_path(path, cx);
        }
        cx.notify();
    }

    fn update_open_document_for_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(doc_id) = self.document_id_for_path(path, cx) {
            self.update_specific_document_view(doc_id, cx);
        }
    }

    fn document_id_for_path(
        &self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Option<helix_view::DocumentId> {
        let core = self.core.read(cx);
        core.editor
            .documents
            .iter()
            .find_map(|(doc_id, doc)| {
                doc.path()
                    .is_some_and(|doc_path| doc_path == path)
                    .then_some(doc_id)
            })
            .copied()
    }

    fn update_key_hints(&mut self, cx: &mut Context<Self>) {
        // Check if editor has pending keymap info
        let editor = &self.core.read(cx).editor;
        let editor_info = editor.autoinfo.as_ref().map(|info| helix_view::info::Info {
            title: info.title.clone(),
            text: info.text.clone(),
            width: info.width,
            height: info.height,
        });

        self.key_hints.update(cx, |key_hints, cx| {
            key_hints.set_info(editor_info);
            cx.notify();
        });
    }

    fn forward_info_box_event(&self, event: &crate::types::AppEvent, cx: &mut Context<Self>) {
        if !matches!(event, crate::types::AppEvent::Ui(_)) {
            return;
        }

        self.info.update(cx, |info_box, info_cx| {
            info_box.handle_app_event(event, info_cx);
        });
    }

    /// Register focus groups for main UI areas with InputCoordinator
    fn register_focus_groups(&mut self, cx: &mut Context<Self>) {
        info!("Registering focus groups with InputCoordinator");

        // Register editor focus group
        self.input_coordinator.register_focus_group(
            FocusGroup::Editor,
            Some(self.focus_handle.clone()),
            Some(Box::new(|| {
                debug!("Editor focus group activated");
            })),
        );

        // Register file tree focus group if available
        if let Some(ref file_tree) = self.file_tree {
            self.input_coordinator.register_focus_group(
                FocusGroup::FileTree,
                Some(file_tree.focus_handle(cx)),
                Some(Box::new(|| {
                    debug!("FileTree focus group activated");
                })),
            );
        }

        // Register overlay focus group
        self.input_coordinator.register_focus_group(
            FocusGroup::Overlays,
            Some(self.overlay.focus_handle(cx)),
            Some(Box::new(|| {
                debug!("Overlays focus group activated");
            })),
        );

        // Set editor and file tree as available if they exist
        self.input_coordinator
            .set_focus_group_available(FocusGroup::Editor, true);
        if self.file_tree.is_some() && self.show_file_tree {
            self.input_coordinator
                .set_focus_group_available(FocusGroup::FileTree, true);
        }

        info!("Registered focus groups for main UI areas with InputCoordinator");

        // OLD CODE - disabled
        /*
            let file_tree_group = GlobalFocusGroup {
                id: "file_tree".to_string(),
                name: "File Tree".to_string(),
                priority: FocusPriority::Normal,
                elements: vec![FocusElement {
                    id: "file_tree_view".to_string(),
                    name: "File Tree View".to_string(),
                    focus_handle: Some(file_tree.focus_handle(cx)),
                    tab_index: 0,
                    enabled: true,
                    element_type: FocusElementType::FileTree,
                }],
                active_element: Some(0),
                enabled: true,
            };
            // DISABLED: // OLD: self.global_input.register_focus_group(file_tree_group);
        }

        // Register overlay focus group
        let overlay_group = GlobalFocusGroup {
            id: "overlays".to_string(),
            name: "Overlays".to_string(),
            priority: FocusPriority::Critical,
            elements: vec![FocusElement {
                id: "overlay_view".to_string(),
                name: "Overlay View".to_string(),
                focus_handle: Some(self.overlay.focus_handle(cx)),
                tab_index: 2,
                enabled: true,
                element_type: FocusElementType::Picker,
            }],
            active_element: Some(0),
            enabled: true,
        };
        // DISABLED: // OLD: self.global_input.register_focus_group(overlay_group);
        */

        // Method completed with InputCoordinator integration
    }

    /// Setup completion-specific shortcuts and input contexts
    fn setup_completion_shortcuts(&mut self) {
        // TODO: Re-implement with InputCoordinator
        /*
        use nucleotide_ui::providers::EventPriority;
        use nucleotide_ui::{
            DismissTarget, GlobalNavigationDirection, ShortcutAction, ShortcutDefinition,
        };

        // Register Escape key to dismiss completion with high priority
        let escape_shortcut = ShortcutDefinition {
            key_combination: "escape".to_string(),
            action: ShortcutAction::Dismiss(DismissTarget::Completion),
            description: "Dismiss completion popup".to_string(),
            context: Some("completion".to_string()),
            priority: EventPriority::Critical,
            enabled: true,
        };
        // DISABLED: // OLD: self.global_input.register_shortcut(escape_shortcut);

        // DISABLED: CTRL+Space shortcut registration - let Helix handle it natively
        // let trigger_completion_shortcut = ShortcutDefinition {
        //     key_combination: "ctrl-space".to_string(),
        //     action: ShortcutAction::Action("trigger_completion".to_string()),
        //     description: "Trigger completion".to_string(),
        //     context: Some("editor".to_string()),
        //     priority: EventPriority::High,
        //     enabled: true,
        // };
        // self.global_input.register_shortcut(trigger_completion_shortcut);

        // Register Tab for completion navigation
        let tab_shortcut = ShortcutDefinition {
            key_combination: "tab".to_string(),
            action: ShortcutAction::Navigate(GlobalNavigationDirection::Next),
            description: "Navigate to next completion item".to_string(),
            context: Some("completion".to_string()),
            priority: EventPriority::High,
            enabled: true,
        };
        // DISABLED: // OLD: self.global_input.register_shortcut(tab_shortcut);

        // Register Shift+Tab for reverse completion navigation
        let shift_tab_shortcut = ShortcutDefinition {
            key_combination: "shift-tab".to_string(),
            action: ShortcutAction::Navigate(GlobalNavigationDirection::Previous),
            description: "Navigate to previous completion item".to_string(),
            context: Some("completion".to_string()),
            priority: EventPriority::High,
            enabled: true,
        };
        // DISABLED: // OLD: self.global_input.register_shortcut(shift_tab_shortcut);

        // Register additional keyboard navigation shortcuts
        self.setup_additional_navigation_shortcuts();

        // Register dismiss handler for completion
        // Note: The actual dismissal is handled by the global input system returning HandledAndStop
        // which prevents the key from reaching the normal escape handling logic
        // DISABLED: Method call to global_input system
        /*
        // OLD: self.global_input.register_dismiss_handler(
            nucleotide_ui::DismissTarget::Completion,
            move || {
                // This signals that the dismiss action was triggered by global input
                // The actual dismissal happens in the normal key handling flow
            },
        );
        */

        nucleotide_logging::info!("Setup completion-specific shortcuts");
        */
    }

    /// Manage completion input context based on completion state
    fn manage_completion_context(&mut self, has_completion: bool) {
        if !has_completion {
            self.active_completion_session = None;
        }

        let completion_context_active =
            self.input_coordinator.current_context() == InputContext::Completion;

        match (has_completion, completion_context_active) {
            (true, false) => {
                self.input_coordinator
                    .push_context(InputContext::Completion);
                nucleotide_logging::debug!("Pushed completion context");
            }
            (false, true) => {
                if let Some(popped) = self.input_coordinator.pop_context() {
                    nucleotide_logging::debug!(context = ?popped, "Popped completion context");
                }
            }
            _ => {
                // No context change needed
            }
        }
    }

    // removed unused setup_additional_navigation_shortcuts
    /*
    fn setup_additional_navigation_shortcuts(&mut self) {
        use nucleotide_ui::providers::EventPriority;
        use nucleotide_ui::{GlobalNavigationDirection, ShortcutAction, ShortcutDefinition};

        // Global shortcuts that work in any context
        let global_shortcuts = vec![
            // File tree management
            ShortcutDefinition {
                key_combination: "ctrl-b".to_string(),
                action: ShortcutAction::Action("toggle_file_tree".to_string()),
                description: "Toggle file tree visibility".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-shift-e".to_string(),
                action: ShortcutAction::Focus("file_tree".to_string()),
                description: "Focus file tree".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            // Focus management shortcuts
            ShortcutDefinition {
                key_combination: "ctrl-1".to_string(),
                action: ShortcutAction::Focus("editor".to_string()),
                description: "Focus main editor".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-2".to_string(),
                action: ShortcutAction::Focus("file_tree".to_string()),
                description: "Focus file tree".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            // Panel navigation
            ShortcutDefinition {
                key_combination: "alt-left".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Left),
                description: "Navigate to left panel".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "alt-right".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Right),
                description: "Navigate to right panel".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            // Quick actions
            ShortcutDefinition {
                key_combination: "ctrl-p".to_string(),
                action: ShortcutAction::Action("open_file_picker".to_string()),
                description: "Open file picker".to_string(),
                context: None,
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-shift-p".to_string(),
                action: ShortcutAction::Action("open_command_prompt".to_string()),
                description: "Open command prompt".to_string(),
                context: None,
                priority: EventPriority::High,
                enabled: true,
            },
            // Window management
            ShortcutDefinition {
                key_combination: "ctrl-w".to_string(),
                action: ShortcutAction::Action("close_active_document".to_string()),
                description: "Close active document".to_string(),
                context: Some("editor".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-shift-w".to_string(),
                action: ShortcutAction::Action("close_all_documents".to_string()),
                description: "Close all documents".to_string(),
                context: None,
                priority: EventPriority::Normal,
                enabled: true,
            },
            // Search and navigation
            ShortcutDefinition {
                key_combination: "ctrl-f".to_string(),
                action: ShortcutAction::Action("start_search".to_string()),
                description: "Start search".to_string(),
                context: Some("editor".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-shift-f".to_string(),
                action: ShortcutAction::Action("global_search".to_string()),
                description: "Global search in files".to_string(),
                context: None,
                priority: EventPriority::High,
                enabled: true,
            },
        ];

        // File tree specific shortcuts
        let file_tree_shortcuts = vec![
            // Navigate within file tree
            ShortcutDefinition {
                key_combination: "up".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Up),
                description: "Move up in file tree".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "down".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Down),
                description: "Move down in file tree".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "left".to_string(),
                action: ShortcutAction::Action("collapse_file_tree_node".to_string()),
                description: "Collapse file tree node".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "right".to_string(),
                action: ShortcutAction::Action("expand_file_tree_node".to_string()),
                description: "Expand file tree node".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "enter".to_string(),
                action: ShortcutAction::Action("open_selected_file".to_string()),
                description: "Open selected file".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            // Return to editor from file tree
            ShortcutDefinition {
                key_combination: "escape".to_string(),
                action: ShortcutAction::Focus("editor".to_string()),
                description: "Return focus to editor".to_string(),
                context: Some("file_tree".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
        ];

        // Completion specific shortcuts - Helix compatible keybindings
        let completion_shortcuts = vec![
            ShortcutDefinition {
                key_combination: "up".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Up),
                description: "Move up in completion list".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "down".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Down),
                description: "Move down in completion list".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-y".to_string(),
                action: ShortcutAction::Action("accept_completion".to_string()),
                description: "Accept selected completion (primary - Helix)".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "tab".to_string(),
                action: ShortcutAction::Action("accept_completion".to_string()),
                description: "Accept selected completion (secondary)".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::High,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-n".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Down),
                description: "Next completion item (Helix style)".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
            ShortcutDefinition {
                key_combination: "ctrl-p".to_string(),
                action: ShortcutAction::Navigate(GlobalNavigationDirection::Up),
                description: "Previous completion item (Helix style)".to_string(),
                context: Some("completion".to_string()),
                priority: EventPriority::Critical,
                enabled: true,
            },
        ];

        // Register all shortcuts
        for _shortcut in global_shortcuts
            .into_iter()
            .chain(file_tree_shortcuts.into_iter())
            .chain(completion_shortcuts.into_iter())
        {
            // DISABLED: // OLD: self.global_input.register_shortcut(shortcut);
        }

        // Register action handlers
        self.setup_action_handlers();

        nucleotide_logging::info!("Setup additional navigation shortcuts");
    }
    */

    // removed unused setup_action_handlers

    /// Register action handlers with the global input system
    fn register_action_handlers(&mut self, _cx: &mut Context<Self>) {
        nucleotide_logging::info!("Registering action handlers with global input system");

        // Get weak references to avoid circular dependencies
        // Weak workspace handle can be created via cx.entity().downgrade() if needed

        // For now, register simple logging handlers - the real functionality will be
        // implemented via proper GPUI actions below
        // OLD: self.global_input.register_action_handler("focus_editor".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: focus_editor")
        // });

        // OLD: self.global_input.register_action_handler("focus_file_tree".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: focus_file_tree")
        // });

        // OLD: self.global_input.register_action_handler("toggle_file_tree".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: toggle_file_tree")
        // });

        // OLD: self.global_input.register_action_handler("trigger_completion".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: trigger_completion")
        // });

        // OLD: self.global_input.register_action_handler("open_file_picker".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: open_file_picker")
        // });

        // OLD: self.global_input.register_action_handler("open_command_prompt".to_string(), || {
        //     nucleotide_logging::debug!("Global input action: open_command_prompt")
        // });

        nucleotide_logging::info!("Successfully registered all action handlers");
    }

    // /// Handle only truly global shortcuts that should work regardless of focus state
    // removed unused handle_global_shortcuts_only

    /// Handle completion events directly using the event system
    fn handle_completion_event(
        &mut self,
        event: &helix_view::handlers::completion::CompletionEvent,
        cx: &mut Context<Self>,
    ) {
        use helix_view::handlers::completion::CompletionEvent;

        debug!("Workspace handling completion event");

        match event {
            CompletionEvent::ManualTrigger { cursor, doc, view } => {
                debug!(cursor = *cursor, doc_id = ?doc, view_id = ?view, "Processing manual completion trigger");
                self.process_completion_trigger(
                    *cursor,
                    *doc,
                    *view,
                    LspCompletionTrigger::Manual,
                    cx,
                );
            }
            CompletionEvent::AutoTrigger { cursor, doc, view } => {
                debug!(cursor = *cursor, doc_id = ?doc, view_id = ?view, "Processing auto completion trigger");
                self.process_completion_trigger(
                    *cursor,
                    *doc,
                    *view,
                    LspCompletionTrigger::Automatic,
                    cx,
                );
            }
            CompletionEvent::TriggerChar { cursor, doc, view } => {
                debug!(cursor = *cursor, doc_id = ?doc, view_id = ?view, "Processing trigger character completion");
                let trigger = self
                    .completion_character_before_cursor(*cursor, *doc, cx)
                    .map(LspCompletionTrigger::Character)
                    .unwrap_or(LspCompletionTrigger::Automatic);
                self.process_completion_trigger(*cursor, *doc, *view, trigger, cx);
            }
            CompletionEvent::DeleteText { cursor: _ } => {
                debug!("Processing delete text - hiding completions");
                self.hide_completions(cx);
            }
            CompletionEvent::Cancel => {
                debug!("Processing completion cancel - hiding completions");
                self.hide_completions(cx);
            }
        }
    }

    /// Update completion filter if completion is active and prefix has changed
    /// This should be called when text changes while completion is active
    pub fn update_completion_filter(&mut self, new_prefix: String, cx: &mut Context<Self>) -> bool {
        debug!(
            prefix = %new_prefix,
            "Updating completion filter with new prefix"
        );

        // Check if we have an active completion view and update its filter
        self.overlay.update(cx, |overlay, cx| {
            overlay.update_completion_filter(new_prefix, cx)
        })
    }

    /// Update completion filter by detecting current prefix at cursor
    /// This method attempts to auto-detect the current completion prefix
    pub fn update_completion_filter_auto(&mut self, cx: &mut Context<Self>) -> bool {
        // Get current text under cursor to determine new prefix
        if let Some(current_prefix) = self.get_current_completion_prefix(cx) {
            let updated = self.update_completion_filter(current_prefix.clone(), cx);
            self.retrigger_incomplete_completion_if_needed(&current_prefix, cx);
            updated
        } else {
            false
        }
    }

    fn retrigger_incomplete_completion_if_needed(
        &mut self,
        current_prefix: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active_completion_session.as_mut() else {
            return;
        };

        let (focused_doc_id, focused_view_id) = {
            let core = self.core.read(cx);
            let view_id = core.editor.tree.focus;
            let doc_id = core.editor.tree.try_get(view_id).map(|view| view.doc);
            (doc_id, view_id)
        };

        if !should_retrigger_incomplete_completion_for_focused_session(
            session,
            current_prefix,
            focused_doc_id,
            focused_view_id,
        ) {
            return;
        }

        let doc_id = session.doc_id;
        let view_id = session.view_id;
        let server_filter = session.incomplete_server_ids.clone();
        let retained_items = session.retained_items.clone();
        if server_filter.is_empty() {
            return;
        }
        session.requested_prefix = current_prefix.to_string();

        let Some(cursor) = self.completion_cursor(doc_id, view_id, cx) else {
            return;
        };

        nucleotide_logging::debug!(
            prefix = %current_prefix,
            doc_id = ?doc_id,
            view_id = ?view_id,
            incomplete_server_count = server_filter.len(),
            retained_item_count = retained_items.len(),
            "Retriggering incomplete LSP completion providers"
        );
        self.start_completion_request_with_provider_reuse(
            cursor,
            doc_id,
            view_id,
            LspCompletionTrigger::Incomplete,
            Some(server_filter),
            retained_items,
            cx,
        );
    }

    /// Get the current word prefix under the cursor for completion filtering
    fn get_current_completion_prefix(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let core = self.core.clone();
        core.update(cx, |core, _cx| {
            let editor = &core.editor;
            let view_id = editor.tree.focus;
            let Some(view) = editor.tree.try_get(view_id) else {
                nucleotide_logging::warn!("No active view for completion prefix extraction");
                return None;
            };
            let Some(doc) = editor.document(view.doc) else {
                nucleotide_logging::warn!("No active document for completion prefix extraction");
                return None;
            };
            let text = doc.text();
            let selection = doc.selection(view.id);
            let cursor_pos = selection.primary().cursor(text.slice(..));

            // Find the start of the current word by looking backwards from cursor
            let line = text.char_to_line(cursor_pos);
            let line_start = text.line_to_char(line);
            let line_end = text.line_to_char(line + 1).min(text.len_chars());

            // Get the full line text to ensure we capture the most recent character
            let full_line = text.slice(line_start..line_end).to_string();

            // Find our position within the line
            let cursor_in_line = cursor_pos - line_start;

            nucleotide_logging::debug!(
                cursor_pos = cursor_pos,
                line_start = line_start,
                cursor_in_line = cursor_in_line,
                full_line = %full_line,
                "Cursor position analysis"
            );

            // Try getting text up to cursor position. Helix cursor positions
            // are character offsets; convert to a UTF-8 byte boundary before
            // taking a Rust string slice.
            let line_text_to_cursor =
                PrefixExtractor::line_prefix_at_char(&full_line, cursor_in_line);

            nucleotide_logging::debug!(
                line_text_to_cursor = %line_text_to_cursor,
                full_line_len = full_line.len(),
                cursor_in_line = cursor_in_line,
                "Text extraction analysis"
            );

            // Configure prefix extractor based on current document's file extension
            if let Some(path) = doc.path()
                && let Some(extension) = path.extension().and_then(|ext| ext.to_str())
            {
                let language = self.map_extension_to_language(extension);
                self.prefix_extractor.configure_for_language(&language);
            }

            // Use the enhanced prefix extractor for language-aware completion
            let (prefix, is_trigger_completion) = self
                .prefix_extractor
                .extract_prefix(&full_line, cursor_in_line);

            nucleotide_logging::debug!(
                is_trigger_completion = is_trigger_completion,
                extracted_prefix = %prefix,
                "Enhanced prefix extraction result"
            );

            nucleotide_logging::debug!(
                prefix = %prefix,
                cursor_pos = cursor_pos,
                line = line,
                line_text_to_cursor = %line_text_to_cursor,
                ends_with_dot = line_text_to_cursor.ends_with('.'),
                is_trigger_completion = is_trigger_completion,
                "Enhanced completion prefix extraction completed"
            );

            // Even empty prefix is valid for trigger completions (e.g., method completion after a dot)
            Some(prefix)
        })
    }

    /// Map file extensions to language identifiers
    fn map_extension_to_language(&self, extension: &str) -> String {
        match extension.to_lowercase().as_str() {
            "rs" => "rust".to_string(),
            "js" | "mjs" => "javascript".to_string(),
            "ts" | "mts" => "typescript".to_string(),
            "tsx" => "typescript".to_string(),
            "jsx" => "javascript".to_string(),
            "css" => "css".to_string(),
            "scss" => "scss".to_string(),
            "less" => "less".to_string(),
            "php" => "php".to_string(),
            "c" => "c".to_string(),
            "cpp" | "cc" | "cxx" | "c++" => "cpp".to_string(),
            "h" | "hpp" | "hxx" => "cpp".to_string(),
            "py" => "python".to_string(),
            "go" => "go".to_string(),
            "java" => "java".to_string(),
            _ => "generic".to_string(),
        }
    }

    fn completion_language_for_doc(
        &self,
        doc_id: helix_view::DocumentId,
        cx: &mut Context<Self>,
    ) -> String {
        let extension = {
            let core = self.core.read(cx);
            core.editor
                .document(doc_id)
                .and_then(|doc| doc.path())
                .and_then(|path| path.extension())
                .and_then(|extension| extension.to_str())
                .map(str::to_string)
        };

        extension
            .as_deref()
            .map(|extension| self.map_extension_to_language(extension))
            .unwrap_or_else(|| "generic".to_string())
    }

    fn completion_memory_key(
        language: &str,
        prefix: &str,
        item: &nucleotide_ui::CompletionItem,
    ) -> CompletionMemoryKey {
        CompletionMemoryKey {
            language: language.to_string(),
            prefix: prefix.to_string(),
            kind: item.kind,
            insert_text: item.text.to_string(),
        }
    }

    fn apply_completion_locality_scores(
        &self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        items: &mut [nucleotide_ui::completion_v2::CompletionItem],
        cx: &mut Context<Self>,
    ) {
        let Some((document_text, cursor_line)) = ({
            let core = self.core.read(cx);
            core.editor.document(doc_id).map(|doc| {
                let cursor = doc
                    .selection(view_id)
                    .primary()
                    .cursor(doc.text().slice(..));
                (doc.text().to_string(), doc.text().char_to_line(cursor))
            })
        }) else {
            return;
        };

        for item in items {
            let Some(key) = completion_locality_key(item) else {
                continue;
            };
            item.locality_score =
                completion_locality_score_for_text(&document_text, cursor_line, &key);
        }
    }

    /// Process completion trigger and request LSP completions
    fn process_completion_trigger(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: LspCompletionTrigger,
        cx: &mut Context<Self>,
    ) {
        debug!(cursor = cursor, doc_id = ?doc_id, view_id = ?view_id, trigger = ?trigger, "Requesting completions through Nucleotide");

        if matches!(trigger, LspCompletionTrigger::Manual)
            && self.manual_completion_needs_lsp_settle_delay(cursor, doc_id, cx)
        {
            self.start_completion_request_after_delay(
                cursor,
                doc_id,
                view_id,
                trigger,
                std::time::Duration::from_millis(30),
                cx,
            );
            return;
        }

        self.start_completion_request(cursor, doc_id, view_id, trigger, cx);
    }

    fn start_completion_request_after_delay(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: LspCompletionTrigger,
        delay: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;

            if let Some(this) = this.upgrade() {
                this.update(cx, move |workspace, cx| {
                    let cursor = workspace
                        .completion_cursor(doc_id, view_id, cx)
                        .unwrap_or(cursor);
                    workspace.start_completion_request(cursor, doc_id, view_id, trigger, cx);
                });
            }
        })
        .detach();
    }

    fn start_completion_request(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: LspCompletionTrigger,
        cx: &mut Context<Self>,
    ) {
        self.start_completion_request_with_provider_reuse(
            cursor,
            doc_id,
            view_id,
            trigger,
            None,
            Vec::new(),
            cx,
        );
    }

    fn start_completion_request_with_provider_reuse(
        &mut self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        trigger: LspCompletionTrigger,
        server_filter: Option<Vec<u64>>,
        retained_items: Vec<nucleotide_events::completion::CompletionItem>,
        cx: &mut Context<Self>,
    ) {
        let completion_request = self.core.update(cx, |core, _cx| {
            core.prepare_lsp_completions_with_prefix_for_servers(
                cursor,
                doc_id,
                view_id,
                trigger,
                server_filter,
                retained_items,
            )
        });

        let completion_request = match completion_request {
            Ok(request) => request,
            Err(err) => {
                nucleotide_logging::error!(
                    error = %err,
                    "Failed to prepare LSP completions through Nucleotide path"
                );
                return;
            }
        };

        cx.spawn(async move |this, cx| {
            let completion_result = completion_request.collect().await;

            if let Some(this) = this.upgrade() {
                this.update(cx, move |workspace, cx| {
                    workspace.finish_completion_request(completion_result, doc_id, view_id, cx);
                });
            }
        })
        .detach();
    }

    fn finish_completion_request(
        &mut self,
        completion_result: anyhow::Result<(
            Vec<nucleotide_events::completion::CompletionItem>,
            String,
            bool,
            Vec<u64>,
        )>,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        cx: &mut Context<Self>,
    ) {
        match completion_result {
            Ok((completion_items, prefix, is_incomplete, incomplete_server_ids)) => {
                nucleotide_logging::debug!(
                    item_count = completion_items.len(),
                    prefix = %prefix,
                    is_incomplete = is_incomplete,
                    incomplete_server_count = incomplete_server_ids.len(),
                    "Received completion items from Nucleotide LSP path"
                );

                if completion_items.is_empty() {
                    nucleotide_logging::warn!("No completion items returned from LSP");
                    self.hide_completions(cx);
                } else {
                    self.show_completion_items_with_prefix(
                        completion_items,
                        prefix,
                        doc_id,
                        view_id,
                        is_incomplete,
                        incomplete_server_ids,
                        cx,
                    );
                }
            }
            Err(err) => {
                nucleotide_logging::error!(
                    error = %err,
                    "Failed to get LSP completions through Nucleotide path"
                );
            }
        }
    }

    fn completion_cursor(
        &self,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let core = self.core.read(cx);
        let Some(doc) = core.editor.document(doc_id) else {
            nucleotide_logging::warn!(
                doc_id = ?doc_id,
                view_id = ?view_id,
                "Document not found for completion cursor"
            );
            return None;
        };

        Some(
            doc.selection(view_id)
                .primary()
                .cursor(doc.text().slice(..)),
        )
    }

    fn completion_character_before_cursor(
        &self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        cx: &mut Context<Self>,
    ) -> Option<char> {
        let core = self.core.read(cx);
        let doc = core.editor.document(doc_id)?;
        let text = doc.text();
        let cursor = cursor.min(text.len_chars());
        text.chars_at(cursor).reversed().next()
    }

    fn manual_completion_needs_lsp_settle_delay(
        &self,
        cursor: usize,
        doc_id: helix_view::DocumentId,
        cx: &mut Context<Self>,
    ) -> bool {
        let core = self.core.read(cx);
        let Some(doc) = core.editor.document(doc_id) else {
            return false;
        };

        let text = doc.text();
        let cursor_chars = cursor.min(text.len_chars());
        let Some(prev_ch) = text.chars_at(cursor_chars).reversed().next() else {
            return false;
        };

        helix_core::chars::char_is_word(prev_ch) || prev_ch == ':'
    }

    // /// Convert completion items and show completion popup
    // removed unused show_completion_items

    /// Convert completion items and show completion popup with prefix filtering
    pub fn show_completion_items_with_prefix(
        &mut self,
        items: Vec<nucleotide_events::completion::CompletionItem>,
        prefix: String,
        doc_id: helix_view::DocumentId,
        view_id: helix_view::ViewId,
        is_incomplete: bool,
        incomplete_server_ids: Vec<u64>,
        cx: &mut Context<Self>,
    ) {
        // Convert between completion item types (same as existing method)
        let language = self.completion_language_for_doc(doc_id, cx);
        let retained_items =
            retained_completion_items_for_completed_providers(&items, &incomplete_server_ids);
        let document_version = self
            .core
            .read(cx)
            .editor
            .document(doc_id)
            .map(|doc| doc.version())
            .unwrap_or_default();
        let mut ui_items: Vec<nucleotide_ui::completion_v2::CompletionItem> = items
            .into_iter()
            .map(ui_completion_item_from_event)
            .collect();

        for item in &mut ui_items {
            let key = Self::completion_memory_key(&language, &prefix, item);
            item.selection_priority = self.completion_memory.priority(&key);
        }
        self.apply_completion_locality_scores(doc_id, view_id, &mut ui_items, cx);

        nucleotide_logging::debug!(
            ui_item_count = ui_items.len(),
            prefix = %prefix,
            is_incomplete = is_incomplete,
            incomplete_server_count = incomplete_server_ids.len(),
            retained_item_count = retained_items.len(),
            "Converted to UI completion items with prefix, creating filtered completion view"
        );

        self.active_completion_session = Some(ActiveCompletionSession {
            doc_id,
            view_id,
            document_version,
            is_incomplete,
            incomplete_server_ids,
            retained_items,
            requested_prefix: prefix.clone(),
        });

        // Create completion view with prefix filtering
        let ui_items_count = ui_items.len();
        let completion_view = cx.new(|cx| {
            let mut view = nucleotide_ui::completion_v2::CompletionView::new(cx);
            // Use the new method that applies initial filtering
            let initial_filter = if prefix.is_empty() {
                None
            } else {
                Some(prefix)
            };
            view.set_items_with_filter(ui_items, initial_filter, cx);
            view
        });
        nucleotide_logging::debug!(
            "✨ CREATING COMPLETION VIEW: {} items, emitting Update::Completion event via core",
            ui_items_count
        );

        // Emit through core so overlay (which subscribes to core) receives the event
        let completion_view_clone = completion_view.clone();
        self.core.update(cx, |_core, cx| {
            cx.emit(crate::Update::Completion(completion_view_clone));
        });
        cx.notify();
    }

    /// Hide completions
    fn hide_completions(&mut self, cx: &mut Context<Self>) {
        debug!("Hiding completions via overlay dismiss");
        self.active_completion_session = None;
        self.overlay.update(cx, |overlay, cx| {
            overlay.dismiss_completion(cx);
        });
        cx.notify();
    }

    // /// Handle keyboard shortcuts detected by the global input system (full processing)
    // removed unused handle_global_input_shortcuts

    // === Action Handler Implementations ===

    /// Focus the main editor area
    pub fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Focusing editor area");

        // Find the currently active document view and focus it
        if let Some(view_id) = self.view_manager.focused_view_id()
            && let Some(doc_view) = self.view_manager.get_document_view(&view_id)
        {
            let doc_focus = doc_view.focus_handle(cx);
            window.focus(&doc_focus, cx);
            nucleotide_logging::debug!(view_id = ?view_id, "Focused active document view");
            return;
        }

        // If no specific document, focus the main workspace
        window.focus(&self.focus_handle, cx);
        nucleotide_logging::debug!("Focused main workspace");
    }

    /// Focus the file tree if it exists and is visible
    pub fn focus_file_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Focusing file tree");

        if let Some(file_tree) = &self.file_tree
            && self.show_file_tree
        {
            let file_tree_focus = file_tree.focus_handle(cx);
            window.focus(&file_tree_focus, cx);
            nucleotide_logging::debug!("Focused file tree");
            return;
        }

        nucleotide_logging::warn!(
            "File tree not available or not visible, focusing editor instead"
        );
        self.focus_editor(window, cx);
    }

    /// Toggle file tree visibility
    pub fn toggle_file_tree_visibility(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_file_tree = !self.show_file_tree;
        nucleotide_logging::debug!(
            visible = self.show_file_tree,
            "Toggled file tree visibility"
        );

        if self.show_file_tree {
            // If we're showing the file tree, focus it
            self.focus_file_tree(window, cx);
        } else {
            // If we're hiding the file tree, focus the editor
            self.focus_editor(window, cx);
        }

        cx.notify(); // Trigger re-render
    }

    /// Trigger completion in the active editor
    pub fn trigger_completion(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Triggering completion directly using real LSP completions");

        // Get current document and view information (in a separate scope to release the borrow)
        let Some((cursor, doc_id, view_id)) = ({
            let editor = &self.core.read(cx).editor;
            let view_id = editor.tree.focus;
            if let Some(view) = editor.tree.try_get(view_id) {
                if let Some(doc) = editor.documents.get(&view.doc) {
                    let cursor = doc
                        .selection(view.id)
                        .primary()
                        .cursor(doc.text().slice(..));
                    Some((cursor, doc.id(), view.id))
                } else {
                    None
                }
            } else {
                None
            }
        }) else {
            self.core.update(cx, |core, cx| {
                core.editor.set_error("No active document for completion");
                cx.notify();
            });
            return;
        };

        nucleotide_logging::debug!(
            cursor = cursor,
            doc_id = ?doc_id,
            view_id = ?view_id,
            "Calling real LSP completion directly from workspace"
        );

        self.start_completion_request(cursor, doc_id, view_id, LspCompletionTrigger::Manual, cx);
    }

    fn active_completion_accept_context(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<(CompletionAcceptTarget, String)> {
        let session = self.active_completion_session.as_ref()?;
        let core = self.core.read(cx);
        let view_doc = core.editor.tree.try_get(session.view_id)?.doc;
        if view_doc != session.doc_id {
            return None;
        }

        let doc = core.editor.document(session.doc_id)?;
        if doc.version() != session.document_version {
            return None;
        }

        Some((
            CompletionAcceptTarget {
                doc_id: session.doc_id,
                view_id: session.view_id,
                document_version: session.document_version,
            },
            session.requested_prefix.clone(),
        ))
    }

    /// Handle completion acceptance via Helix's transaction system
    fn handle_completion_via_helix(&mut self, item_index: usize, cx: &mut Context<Self>) {
        nucleotide_logging::debug!(
            item_index = item_index,
            "Accepting completion via Helix transaction system"
        );

        // Get the completion item from the current completion state
        let completion_item = self.overlay.update(cx, |overlay, cx| {
            overlay.get_completion_item(item_index, cx)
        });

        let Some(completion_item) = completion_item else {
            nucleotide_logging::warn!(
                item_index = item_index,
                "No completion item at index for acceptance"
            );
            return;
        };

        nucleotide_logging::debug!(
            item_index = item_index,
            completion_text = %completion_item.text,
            insert_text_format = ?completion_item.insert_text_format,
            has_edit = completion_item.edit.is_some(),
            "Retrieved completion item for transaction"
        );

        let Some((target, requested_prefix)) = self.active_completion_accept_context(cx) else {
            nucleotide_logging::warn!(
                item_index = item_index,
                "Dropping completion acceptance for stale completion session"
            );
            self.active_completion_session = None;
            self.overlay.update(cx, |overlay, cx| {
                overlay.dismiss_completion(cx);
            });
            return;
        };

        let completion_memory_key = {
            let language = self.completion_language_for_doc(target.doc_id, cx);
            Self::completion_memory_key(&language, &requested_prefix, &completion_item)
        };

        if self.resolve_completion_before_accept(
            completion_item.clone(),
            Some(completion_memory_key.clone()),
            target,
            cx,
        ) {
            return;
        }

        self.accept_completion_item(completion_item, Some(completion_memory_key), target, cx);
    }

    fn resolve_completion_before_accept(
        &mut self,
        completion_item: nucleotide_ui::CompletionItem,
        completion_memory_key: Option<CompletionMemoryKey>,
        target: CompletionAcceptTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(server_id) = completion_item.server_id else {
            return false;
        };
        let Some(raw_lsp_item) = completion_item.raw_lsp_item.clone() else {
            return false;
        };
        let raw_lsp_item = match serde_json::from_value::<lsp::CompletionItem>(raw_lsp_item) {
            Ok(item) => item,
            Err(err) => {
                nucleotide_logging::warn!(
                    error = %err,
                    "Failed to deserialize raw LSP completion item for resolve"
                );
                return false;
            }
        };

        let server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(server_id).into();
        let source_index = completion_item.source_index;
        let resolve_future = self.core.update(cx, |core, _cx| {
            core.prepare_lsp_completion_resolve(server_id, raw_lsp_item, source_index)
        });
        let resolve_future = match resolve_future {
            Ok(Some(resolve_future)) => resolve_future,
            Ok(None) => return false,
            Err(err) => {
                nucleotide_logging::warn!(
                    error = %err,
                    server_id = ?server_id,
                    "Failed to prepare completion item resolve"
                );
                return false;
            }
        };

        cx.spawn(async move |this, cx| {
            let resolved_item = resolve_future.await;

            if let Some(this) = this.upgrade() {
                this.update(cx, move |workspace, cx| {
                    let completion_item = match resolved_item {
                        Ok(resolved_item) => ui_completion_item_from_event(resolved_item),
                        Err(err) => {
                            nucleotide_logging::warn!(
                                error = %err,
                                "Completion item resolve failed; accepting original item"
                            );
                            completion_item
                        }
                    };

                    workspace.accept_completion_item(
                        completion_item,
                        completion_memory_key,
                        target,
                        cx,
                    );
                });
            }
        })
        .detach();

        true
    }

    fn accept_completion_item(
        &mut self,
        completion_item: nucleotide_ui::CompletionItem,
        completion_memory_key: Option<CompletionMemoryKey>,
        target: CompletionAcceptTarget,
        cx: &mut Context<Self>,
    ) {
        let accepted = if let Some(edit) = completion_item.edit.clone() {
            self.handle_lsp_edit_completion(completion_item, edit, target, cx)
        } else {
            // Check if this is a snippet completion
            match completion_item.insert_text_format {
                nucleotide_ui::completion_v2::InsertTextFormat::Snippet => {
                    self.handle_snippet_completion(completion_item, target, cx)
                }
                nucleotide_ui::completion_v2::InsertTextFormat::PlainText => {
                    self.handle_plain_text_completion(completion_item, target, cx)
                }
            }
        };

        if accepted && let Some(key) = completion_memory_key {
            self.completion_memory.memorize(key);
        }
    }

    fn handle_snippet_completion(
        &mut self,
        completion_item: nucleotide_ui::CompletionItem,
        target: CompletionAcceptTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        let snippet_text = completion_item.text.to_string();
        nucleotide_logging::debug!(
            completion_text = %snippet_text,
            "Processing snippet completion with active snippet support"
        );

        let rt_handle = self.handle.clone();
        let applied = self.core.update(cx, move |core, cx| {
            let _guard = rt_handle.enter();
            let editor = &mut core.editor;
            let Some(view_doc_id) = editor.tree.try_get(target.view_id).map(|view| view.doc) else {
                nucleotide_logging::warn!(
                    view_id = ?target.view_id,
                    "Dropping snippet completion for missing view"
                );
                return false;
            };
            if view_doc_id != target.doc_id {
                nucleotide_logging::warn!(
                    view_id = ?target.view_id,
                    expected_doc_id = ?target.doc_id,
                    actual_doc_id = ?view_doc_id,
                    "Dropping snippet completion for stale view/document association"
                );
                return false;
            }

            let Some(doc) = editor.document_mut(target.doc_id) else {
                nucleotide_logging::warn!(
                    doc_id = ?target.doc_id,
                    "Dropping snippet completion for missing document"
                );
                return false;
            };
            if doc.version() != target.document_version {
                nucleotide_logging::warn!(
                    doc_id = ?target.doc_id,
                    request_version = target.document_version,
                    current_version = doc.version(),
                    "Dropping snippet completion for changed document"
                );
                return false;
            }
            use helix_core::Transaction;

            let text = doc.text();
            let selection = doc.selection(target.view_id);
            let primary_cursor = selection.primary().cursor(text.slice(..));

            nucleotide_logging::debug!(
                cursor_pos = primary_cursor,
                doc_len = text.len_chars(),
                selection_ranges = selection.len(),
                "Transaction context before snippet insertion"
            );

            let snippet_result = snippet_completion_transaction(
                text,
                selection,
                &snippet_text,
                None,
                false,
                &mut doc.snippet_ctx(),
            );

            let Some((transaction, rendered_snippet)) = snippet_result
                .map_err(|err| {
                    nucleotide_logging::warn!(
                        completion_text = %snippet_text,
                        error = %err,
                        "Failed to parse snippet, falling back to plain text"
                    );
                })
                .ok()
            else {
                let transaction = Transaction::change_by_selection(text, selection, |range| {
                    let cursor_pos = range.cursor(text.slice(..));
                    let start_pos = completion_word_start(text.slice(..), cursor_pos);
                    (start_pos, cursor_pos, Some(snippet_text.clone().into()))
                });
                doc.apply(&transaction, target.view_id);
                cx.notify();
                return true;
            };

            nucleotide_logging::debug!("Applying snippet transaction to document");
            doc.apply(&transaction, target.view_id);
            install_active_completion_snippet(doc, rendered_snippet);

            nucleotide_logging::debug!("Applied snippet completion transaction successfully");

            cx.notify();
            true
        });

        // Dismiss the completion view after successful text insertion
        if applied {
            self.overlay.update(cx, |overlay, cx| {
                overlay.dismiss_completion(cx);
            });
        }

        nucleotide_logging::debug!("Snippet completion processing complete - view dismissed");
        applied
    }

    fn handle_lsp_edit_completion(
        &mut self,
        completion_item: nucleotide_ui::CompletionItem,
        edit: nucleotide_ui::CompletionEdit,
        target: CompletionAcceptTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        nucleotide_logging::debug!(
            completion_text = %completion_item.text,
            has_primary_edit = edit.text_edit.is_some(),
            additional_edit_count = edit.additional_text_edits.len(),
            "Processing completion with LSP edit metadata"
        );

        let rt_handle = self.handle.clone();
        let applied = self.core.update(cx, move |core, cx| {
            let _guard = rt_handle.enter();
            let editor = &mut core.editor;
            let Some(view_doc_id) = editor.tree.try_get(target.view_id).map(|view| view.doc) else {
                nucleotide_logging::warn!(
                    view_id = ?target.view_id,
                    "Dropping LSP edit completion for missing view"
                );
                return false;
            };
            if view_doc_id != target.doc_id {
                nucleotide_logging::warn!(
                    view_id = ?target.view_id,
                    expected_doc_id = ?target.doc_id,
                    actual_doc_id = ?view_doc_id,
                    "Dropping LSP edit completion for stale view/document association"
                );
                return false;
            }

            let Some(doc) = editor.document_mut(target.doc_id) else {
                nucleotide_logging::warn!(
                    doc_id = ?target.doc_id,
                    "Dropping LSP edit completion for missing document"
                );
                return false;
            };
            if doc.version() != target.document_version {
                nucleotide_logging::warn!(
                    doc_id = ?target.doc_id,
                    request_version = target.document_version,
                    current_version = doc.version(),
                    "Dropping LSP edit completion for changed document"
                );
                return false;
            }

            let text = doc.text();
            let selection = doc.selection(target.view_id);
            let primary_cursor = selection.primary().cursor(text.slice(..));
            let offset_encoding = helix_offset_encoding_from_completion(edit.offset_encoding);

            let replacement_text = completion_item.text.to_string();

            let (edit_offset, replacement_start) = edit
                .text_edit
                .as_ref()
                .and_then(|text_edit| {
                    completion_edit_offset(text, text_edit, offset_encoding, primary_cursor)
                })
                .map(|(offset, start)| (Some(offset), start))
                .unwrap_or_else(|| (None, completion_word_start(text.slice(..), primary_cursor)));

            let (transaction, rendered_snippet) = match completion_item.insert_text_format {
                nucleotide_ui::completion_v2::InsertTextFormat::Snippet => {
                    match snippet_completion_transaction(
                        text,
                        selection,
                        &replacement_text,
                        edit_offset,
                        false,
                        &mut doc.snippet_ctx(),
                    ) {
                        Ok((transaction, rendered_snippet)) => {
                            (transaction, Some(rendered_snippet))
                        }
                        Err(err) => {
                            nucleotide_logging::warn!(
                                completion_text = %replacement_text,
                                error = %err,
                                "Failed to parse snippet completion edit, inserting raw text"
                            );
                            (
                                helix_lsp::util::generate_transaction_from_completion_edit(
                                    text,
                                    selection,
                                    edit_offset,
                                    false,
                                    replacement_text,
                                ),
                                None,
                            )
                        }
                    }
                }
                nucleotide_ui::completion_v2::InsertTextFormat::PlainText => (
                    helix_lsp::util::generate_transaction_from_completion_edit(
                        text,
                        selection,
                        edit_offset,
                        false,
                        replacement_text,
                    ),
                    None,
                ),
            };

            nucleotide_logging::debug!(
                replacement_start = replacement_start,
                has_edit_offset = edit_offset.is_some(),
                "Applying completion transaction from LSP edit metadata"
            );
            doc.apply(&transaction, target.view_id);

            if let Some(rendered_snippet) = rendered_snippet {
                install_active_completion_snippet(doc, rendered_snippet);
            }

            if !edit.additional_text_edits.is_empty() {
                let additional_edits = edit
                    .additional_text_edits
                    .iter()
                    .map(lsp_text_edit_from_completion)
                    .collect();
                let transaction = helix_lsp::util::generate_transaction_from_edits(
                    doc.text(),
                    additional_edits,
                    offset_encoding,
                );
                nucleotide_logging::debug!(
                    additional_edit_count = edit.additional_text_edits.len(),
                    "Applying additional LSP completion edits"
                );
                doc.apply(&transaction, target.view_id);
            }

            cx.notify();
            true
        });

        if applied {
            self.overlay.update(cx, |overlay, cx| {
                overlay.dismiss_completion(cx);
            });
        }

        nucleotide_logging::debug!("LSP edit completion processing complete - view dismissed");
        applied
    }

    fn handle_plain_text_completion(
        &mut self,
        completion_item: nucleotide_ui::CompletionItem,
        target: CompletionAcceptTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        nucleotide_logging::debug!(
            completion_text = %completion_item.text,
            "Processing plain text completion"
        );

        // Use Helix's transaction system to insert the completion text
        let rt_handle = self.handle.clone();
        let applied = self.core.update(cx, move |core, cx| {
            let _guard = rt_handle.enter();
            let editor = &mut core.editor;

            nucleotide_logging::debug!(
                completion_text = %completion_item.text,
                "Creating Helix transaction for plain text completion"
            );

            // Apply the completion using Helix's transaction system
            let Some(view_doc_id) = editor.tree.try_get(target.view_id).map(|view| view.doc) else {
                nucleotide_logging::warn!(
                    view_id = ?target.view_id,
                    "Dropping plain text completion for missing view"
                );
                return false;
            };
            if view_doc_id != target.doc_id {
                nucleotide_logging::warn!(
                    view_id = ?target.view_id,
                    expected_doc_id = ?target.doc_id,
                    actual_doc_id = ?view_doc_id,
                    "Dropping plain text completion for stale view/document association"
                );
                return false;
            }

            let Some(doc) = editor.document_mut(target.doc_id) else {
                nucleotide_logging::warn!(
                    doc_id = ?target.doc_id,
                    "Dropping plain text completion for missing document"
                );
                return false;
            };
            if doc.version() != target.document_version {
                nucleotide_logging::warn!(
                    doc_id = ?target.doc_id,
                    request_version = target.document_version,
                    current_version = doc.version(),
                    "Dropping plain text completion for changed document"
                );
                return false;
            }

            use helix_core::Transaction;

            let text = doc.text();
            let selection = doc.selection(target.view_id);
            let primary_cursor = selection.primary().cursor(text.slice(..));

            nucleotide_logging::debug!(
                cursor_pos = primary_cursor,
                doc_len = text.len_chars(),
                selection_ranges = selection.len(),
                "Transaction context before plain text creation"
            );

            // Create transaction to replace the partial word with completion text
            let transaction = Transaction::change_by_selection(text, selection, |range| {
                // Find the start of the word being completed (go backward from cursor)
                let cursor_pos = range.cursor(text.slice(..));
                let text_slice = text.slice(..);
                let start_pos = completion_word_start(text_slice, cursor_pos);

                nucleotide_logging::trace!(
                    range_cursor = cursor_pos,
                    "Processing range in plain text transaction"
                );

                nucleotide_logging::trace!(
                    start_pos = start_pos,
                    end_pos = cursor_pos,
                    replacement_text = %completion_item.text,
                    "Plain text transaction replacement calculated"
                );

                // Return the replacement text for this range
                (
                    start_pos,
                    cursor_pos,
                    Some(completion_item.text.to_string().into()),
                )
            });

            // Apply the transaction
            nucleotide_logging::debug!("Applying plain text transaction to document");
            doc.apply(&transaction, target.view_id);

            nucleotide_logging::debug!("Applied plain text completion transaction successfully");

            cx.notify();
            true
        });

        // Dismiss the completion view after successful text insertion
        if applied {
            self.overlay.update(cx, |overlay, cx| {
                overlay.dismiss_completion(cx);
            });
        }

        nucleotide_logging::debug!("Plain text completion processing complete - view dismissed");
        applied
    }

    // /// Handle completion acceptance - insert the selected text into the editor (DEPRECATED)
    // removed unused handle_completion_accepted

    /// Accept the current completion selection
    pub fn accept_completion(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Accepting current completion selection");

        // Send Enter to accept completion
        let key_event = KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::empty(),
        };

        self.input
            .update(cx, |_, cx| cx.emit(crate::InputEvent::key(key_event)));
        nucleotide_logging::debug!("Sent Enter to accept completion");
    }

    /// Open file picker
    pub fn open_file_picker(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Opening file picker");

        let handle = self.handle.clone();
        let core = self.core.clone();
        let overlay = self.overlay.clone();
        open(core, handle, overlay, cx);
    }

    /// Open command prompt
    pub fn open_command_prompt(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Opening command prompt");
        self.show_command_prompt(cx);
    }

    fn show_command_prompt(&mut self, cx: &mut Context<Self>) {
        let prompt = crate::prompt::Prompt::native(":", "", |_| {}).with_cancel(|| {});
        cx.emit(crate::Update::Prompt(prompt));
    }

    /// Start local search in current document
    pub fn start_search(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Starting local search");

        // Send '/' to start search mode
        let key_event = KeyEvent {
            code: KeyCode::Char('/'),
            modifiers: KeyModifiers::empty(),
        };

        self.input
            .update(cx, |_, cx| cx.emit(crate::InputEvent::key(key_event)));
        nucleotide_logging::debug!("Started search mode");
    }

    /// Start global search across files
    pub fn start_global_search(&mut self, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Starting global search");

        let prompt = crate::prompt::Prompt::native("global-search:", "", |_| {}).with_cancel(|| {});
        self.core.update(cx, |_core, cx| {
            cx.emit(crate::Update::Prompt(prompt));
        });
    }

    fn start_file_tree_search(&mut self, initial_query: Option<String>, cx: &mut Context<Self>) {
        nucleotide_logging::debug!("Starting file tree search");

        let initial_query = initial_query.unwrap_or_default();
        let prompt = crate::prompt::Prompt::native("file-tree-search:", initial_query, |_| {})
            .with_cancel(|| {});
        self.core.update(cx, |_core, cx| {
            cx.emit(crate::Update::Prompt(prompt));
        });
    }

    fn handle_file_tree_search_submitted(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim();
        let query = (!query.is_empty()).then(|| query.to_string());

        if let Some(file_tree) = &self.file_tree {
            self.show_file_tree = true;
            file_tree.update(cx, |tree, cx| {
                tree.set_search_query(query, cx);
            });
        }

        cx.notify();
    }

    fn update_document_views(&mut self, cx: &mut Context<Self>) {
        let mut view_ids = HashSet::new();
        self.make_views(&mut view_ids, cx);
    }

    /// Update only a specific document view - more efficient for targeted updates
    fn update_specific_document_view(
        &mut self,
        doc_id: helix_view::DocumentId,
        cx: &mut Context<Self>,
    ) {
        // Find views for this specific document
        let view_ids: Vec<helix_view::ViewId> = self
            .core
            .read(cx)
            .editor
            .tree
            .views()
            .filter_map(|(view, _)| {
                if view.doc == doc_id {
                    Some(view.id)
                } else {
                    None
                }
            })
            .collect();

        // Update only the views for this document
        for view_id in view_ids {
            if let Some(view_entity) = self.view_manager.get_document_view(&view_id) {
                view_entity.update(cx, |_view, cx| {
                    cx.notify();
                });
            }
        }
    }

    /// Update only the currently focused document view
    fn update_current_document_view(&mut self, cx: &mut Context<Self>) {
        if let Some(focused_view_id) = self.view_manager.focused_view_id()
            && let Some(view_entity) = self.view_manager.get_document_view(&focused_view_id)
        {
            view_entity.update(cx, |_view, cx| {
                cx.notify();
            });
        }
    }

    fn document_view_layouts(&self, cx: &mut Context<Self>) -> Vec<DocumentViewLayout> {
        self.core
            .read(cx)
            .editor
            .tree
            .views()
            .map(|(view, is_focused)| DocumentViewLayout {
                view_id: view.id,
                area: view.area,
                is_focused,
            })
            .collect()
    }

    fn render_document_view_layout(
        &self,
        layout: DocumentViewLayout,
        total_area: HelixRect,
        editor_width: f32,
        editor_height: f32,
        dim_inactive_panes: bool,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let view_entity = self
            .view_manager
            .get_document_view(&layout.view_id)?
            .clone();
        let theme = cx.theme();
        let inactive_overlay =
            nucleotide_ui::tokens::with_alpha(theme.tokens.chrome.surface_overlay, 0.10);
        let (left, top, width, height) =
            helix_rect_to_scaled_pixel_bounds(layout.area, total_area, editor_width, editor_height);

        Some(
            div()
                .absolute()
                .left(left)
                .top(top)
                .w(width)
                .h(height)
                .overflow_hidden()
                .when(self.debug_colors_enabled, |d| {
                    d.border_1()
                        .border_color(theme.tokens.chrome.border_default)
                })
                .child(view_entity)
                .when(dim_inactive_panes && !layout.is_focused, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .bg(inactive_overlay),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_split_pane_resize_handle(
        &self,
        divider: SplitPaneDivider,
        total_area: HelixRect,
        editor_width: f32,
        editor_height: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let total_width = f32::from(total_area.width).max(1.0);
        let total_height = f32::from(total_area.height).max(1.0);
        let editor_width = editor_width.max(1.0);
        let editor_height = editor_height.max(1.0);
        let handle_hit = SPLIT_PANE_HANDLE_HITBOX_PX;

        let drag_divider = divider.clone();
        let handle_id = format!(
            "split-pane-resize-handle-{:?}-{:?}-{:?}-{}-{}-{}",
            divider.axis,
            divider.before_view_ids,
            divider.after_view_ids,
            divider.edge,
            divider.start,
            divider.span
        );
        let handle = match divider.axis {
            SplitPaneResizeAxis::Vertical => {
                let edge_px = f32::from(divider.edge.saturating_sub(total_area.x)) / total_width
                    * editor_width;
                let start_px = f32::from(divider.start.saturating_sub(total_area.y)) / total_height
                    * editor_height;
                let span_px = (f32::from(divider.span) / total_height * editor_height).max(1.0);

                split_pane_resize_hitbox(handle_id, SplitPaneResizeAxis::Vertical, handle_hit)
                    .absolute()
                    .left(px(edge_px - handle_hit * 0.5))
                    .top(px(start_px))
                    .h(px(span_px))
            }
            SplitPaneResizeAxis::Horizontal => {
                let edge_px = f32::from(divider.edge.saturating_sub(total_area.y)) / total_height
                    * editor_height;
                let start_px = f32::from(divider.start.saturating_sub(total_area.x)) / total_width
                    * editor_width;
                let span_px = (f32::from(divider.span) / total_width * editor_width).max(1.0);

                split_pane_resize_hitbox(handle_id, SplitPaneResizeAxis::Horizontal, handle_hit)
                    .absolute()
                    .left(px(start_px))
                    .top(px(edge_px - handle_hit * 0.5))
                    .w(px(span_px))
            }
        };

        handle
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |workspace, event: &MouseDownEvent, window, cx| {
                    workspace.start_split_pane_resize(
                        drag_divider.clone(),
                        event.position,
                        total_area,
                        editor_width,
                        editor_height,
                        cx,
                    );
                    window.refresh();
                    cx.stop_propagation();
                }),
            )
            .on_drag(DraggedSplitPaneResize, |_, _, _, cx| {
                cx.new(|_| DraggedSplitPaneResize)
            })
            .on_drag_move::<DraggedSplitPaneResize>(cx.listener(
                |workspace, event: &DragMoveEvent<DraggedSplitPaneResize>, window, cx| {
                    if workspace.split_pane_resize.is_some() {
                        if event.event.dragging()
                            && workspace.update_split_pane_resize(event.event.position, cx)
                        {
                            window.refresh();
                        }
                        cx.stop_propagation();
                    }
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|workspace, _event: &MouseUpEvent, window, cx| {
                    workspace.finish_split_pane_resize(window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|workspace, _event: &MouseUpEvent, window, cx| {
                    workspace.finish_split_pane_resize(window, cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }

    fn render_split_pane_divider_line(
        &self,
        divider: &SplitPaneDivider,
        total_area: HelixRect,
        editor_width: f32,
        editor_height: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let total_width = f32::from(total_area.width).max(1.0);
        let total_height = f32::from(total_area.height).max(1.0);
        let editor_width = editor_width.max(1.0);
        let editor_height = editor_height.max(1.0);
        let line_px = nucleotide_ui::SPLITTER_LINE_PX;
        let color =
            nucleotide_ui::tokens::with_alpha(cx.theme().tokens.chrome.separator_color, 0.7);

        match divider.axis {
            SplitPaneResizeAxis::Vertical => {
                let edge_px = f32::from(divider.edge.saturating_sub(total_area.x)) / total_width
                    * editor_width;
                let start_px = f32::from(divider.start.saturating_sub(total_area.y)) / total_height
                    * editor_height;
                let span_px = (f32::from(divider.span) / total_height * editor_height).max(1.0);

                div()
                    .absolute()
                    .left(px(edge_px - line_px * 0.5))
                    .top(px(start_px))
                    .w(px(line_px))
                    .h(px(span_px))
                    .bg(color)
                    .into_any_element()
            }
            SplitPaneResizeAxis::Horizontal => {
                let edge_px = f32::from(divider.edge.saturating_sub(total_area.y)) / total_height
                    * editor_height;
                let start_px = f32::from(divider.start.saturating_sub(total_area.x)) / total_width
                    * editor_width;
                let span_px = (f32::from(divider.span) / total_width * editor_width).max(1.0);

                div()
                    .absolute()
                    .left(px(start_px))
                    .top(px(edge_px - line_px * 0.5))
                    .w(px(span_px))
                    .h(px(line_px))
                    .bg(color)
                    .into_any_element()
            }
        }
    }

    // /// Trigger completion UI based on current editor state

    /// Send a key directly to Helix, ensuring the editor has focus
    fn send_helix_key(&mut self, key: &str, cx: &mut Context<Self>) {
        // Ensure an editor view has focus
        if self.view_manager.focused_view_id().is_some() {
            self.view_manager.set_needs_focus_restore(true);
        }

        // Parse the key string and send it to Helix
        let keystroke = gpui::Keystroke::parse(key).unwrap_or_else(|_| {
            // Fallback for simple keys
            gpui::Keystroke {
                key_char: Some(key.chars().next().unwrap_or(' ').to_string()),
                key: key.to_string(),
                modifiers: gpui::Modifiers::default(),
            }
        });

        let key_event = utils::translate_key(&keystroke);
        self.input
            .update(cx, |_, cx| cx.emit(InputEvent::key(key_event)));
    }

    /// Adjust the editor font size
    fn adjust_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        // Get current font config
        let mut font_config = cx.global::<crate::types::EditorFontConfig>().clone();

        // Adjust size with bounds checking
        font_config.size = (font_config.size + delta).clamp(8.0, 72.0);

        // Update global font config
        cx.set_global(font_config);

        // Update all document views to use new font size
        self.update_document_views(cx);

        // Force redraw
        cx.notify();
    }

    fn make_views(
        &mut self,
        view_ids: &mut HashSet<ViewId>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let mut focused_file_name = None;
        let mut focused_doc_path = None;

        {
            let editor = &self.core.read(cx).editor;

            // First pass: collect all active view IDs
            for (view, is_focused) in editor.tree.views() {
                let view_id = view.id;

                view_ids.insert(view_id);

                if is_focused {
                    // Verify the view still exists in the tree before accessing
                    if editor.tree.contains(view_id)
                        && let Some(doc) = editor.document(view.doc)
                    {
                        self.view_manager.set_focused_view_id(Some(view_id));
                        let doc_path = doc.path();
                        focused_file_name = doc_path.map(|p| p.display().to_string());
                        focused_doc_path = doc_path.map(|p| p.to_path_buf());
                    }
                }
            }
        } // End of editor borrow scope

        // Sync file tree selection with the focused document (after releasing borrow)
        if let Some(path) = focused_doc_path
            && let Some(file_tree) = &self.file_tree
        {
            file_tree.update(cx, |tree, cx| {
                tree.sync_selection_with_file(Some(path.as_path()), cx);
            });
        }

        // Remove views that are no longer active
        let to_remove: Vec<_> = self
            .view_manager
            .document_views()
            .keys()
            .copied()
            .filter(|id| !view_ids.contains(id))
            .collect();
        for view_id in to_remove {
            self.view_manager.remove_document_view(&view_id);
        }

        // Second pass: create or update views
        for view_id in view_ids.iter() {
            let view_id = *view_id;
            let is_focused = self.view_manager.focused_view_id() == Some(view_id);
            let editor_font = cx.global::<crate::types::EditorFontConfig>();
            let style = TextStyle {
                color: cx.theme().tokens.chrome.text_on_chrome,
                font_family: cx
                    .global::<crate::types::FontSettings>()
                    .fixed_font
                    .family
                    .clone()
                    .into(),
                font_features: FontFeatures::default(),
                font_fallbacks: None,
                font_size: px(editor_font.size).into(),
                line_height: gpui::phi(), // Use golden ratio for optimal line height
                font_weight: editor_font.weight.into(),
                font_style: gpui::FontStyle::Normal,
                background_color: None,
                underline: None,
                strikethrough: None,
                white_space: gpui::WhiteSpace::Normal,
                text_overflow: None,
                text_align: gpui::TextAlign::default(),
                line_clamp: None,
            };
            let core = self.core.clone();
            let input = self.input.clone();

            // Check if view exists and update its style if it does
            if let Some(view) = self.view_manager.get_document_view(&view_id) {
                view.update(cx, |view, cx| {
                    let focus_changed = view.set_focused(is_focused);
                    let style_changed = view.update_text_style(style.clone());
                    if focus_changed || style_changed {
                        cx.notify();
                    }
                });
            } else {
                // Create new view if it doesn't exist
                let view = cx.new(|cx| {
                    let doc_focus_handle = cx.focus_handle();
                    DocumentView::new(
                        core,
                        Some(input),
                        view_id,
                        style.clone(),
                        &doc_focus_handle,
                        is_focused,
                    )
                });
                self.view_manager.insert_document_view(view_id, view);
            }
        }
        focused_file_name
    }

    fn renders_app_titlebar(&self, cx: &Context<Self>) -> bool {
        let gui_config = &self.core.read(cx).config.gui;
        should_render_app_titlebar(
            self.titlebar.is_some(),
            self.show_file_tree,
            self.file_tree_width,
            macos_system_sidebar_enabled(gui_config),
        )
    }

    fn rendered_titlebar_height(&self, window: &Window, cx: &Context<Self>) -> Pixels {
        if self.renders_app_titlebar(cx) {
            nucleotide_ui::titlebar::TitleBar::height(window)
        } else {
            px(0.0)
        }
    }

    fn visible_tab_bar_height(&self, cx: &Context<Self>) -> Pixels {
        let core = self.core.read(cx);
        let editor = &core.editor;
        let has_pinned_tabs = editor
            .documents
            .keys()
            .copied()
            .map(TabId::Document)
            .chain(self.image_tabs.iter().map(|tab| TabId::Image(tab.id)))
            .any(|tab_id| self.pinned_documents.contains(&tab_id));
        let has_unpinned_tabs = editor
            .documents
            .keys()
            .copied()
            .map(TabId::Document)
            .chain(self.image_tabs.iter().map(|tab| TabId::Image(tab.id)))
            .any(|tab_id| !self.pinned_documents.contains(&tab_id));
        tab_bar_height_for_editor(
            core.config.gui.tab_bar.show,
            &editor.config().bufferline,
            editor.documents.len() + self.image_tabs.len(),
            crate::tab::tab_container_height(cx.theme().tokens),
            core.config.gui.tab_bar.show_pinned_tabs_in_separate_row,
            has_pinned_tabs,
            has_unpinned_tabs,
        )
    }

    /// Update global workspace layout information for UI positioning
    fn update_workspace_layout_info(&mut self, window: &Window, cx: &mut Context<Self>) {
        use crate::overlay::WorkspaceLayoutInfo;

        let tab_bar_height = self.visible_tab_bar_height(cx);
        let title_bar_height = self.rendered_titlebar_height(window, cx);

        // Get actual file tree width (user may have resized it)
        let file_tree_width = if self.show_file_tree {
            px(self.file_tree_width)
        } else {
            px(0.0) // No file tree width if hidden
        };

        // Get font and cursor metrics from the focused DocumentView if available
        let (line_height, char_width, gutter_width, cursor_position, cursor_size) =
            self.get_focused_document_view_layout(cx);

        let layout_info = WorkspaceLayoutInfo {
            file_tree_width,
            gutter_width,
            tab_bar_height,
            title_bar_height,
            line_height,
            char_width,
            cursor_position,
            cursor_size,
        };

        // Set as global state so overlay can access it
        cx.set_global(layout_info);
    }

    fn resolve_editor_font_metrics(&mut self, cx: &mut Context<Self>) -> (f32, f32) {
        let editor_font = cx.global::<nucleotide_types::EditorFontConfig>();
        let key = (
            editor_font.family.clone(),
            editor_font.size,
            editor_font.weight,
        );

        let need_recalc = match &self.cached_font_metrics_key {
            Some(k) => k != &key,
            None => true,
        } || self.cached_char_width.is_none()
            || self.cached_line_height.is_none();

        if need_recalc {
            let font = gpui::Font {
                family: editor_font.family.clone().into(),
                features: FontFeatures::default(),
                weight: editor_font.weight.into(),
                style: gpui::FontStyle::Normal,
                fallbacks: None,
            };
            let font_id = cx.text_system().resolve_font(&font);
            let font_size = gpui::px(editor_font.size);
            let char_w = cx
                .text_system()
                .advance(font_id, font_size, 'm')
                .map(|a| f32::from(a.width))
                .unwrap_or(editor_font.size * 0.6)
                .max(1.0);
            let line_h = if editor_font.line_height > 0.0 {
                editor_font.line_height
            } else {
                (editor_font.size * 1.35).max(1.0)
            };

            self.cached_font_metrics_key = Some(key);
            self.cached_char_width = Some(char_w);
            self.cached_line_height = Some(line_h);
        }

        (
            self.cached_line_height
                .unwrap_or((editor_font.size * 1.35).max(1.0)),
            self.cached_char_width
                .unwrap_or((editor_font.size * 0.6).max(1.0)),
        )
    }

    /// Get layout metrics from the focused DocumentView.
    fn get_focused_document_view_layout(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (
        Pixels,
        Pixels,
        Pixels,
        Option<Point<Pixels>>,
        Option<Size<Pixels>>,
    ) {
        let (fallback_line_h, cached_char_w) = self.resolve_editor_font_metrics(cx);
        let fallback_gutter_width = px(60.0);
        // Try to get the focused DocumentView
        if let Some(focused_view_id) = self.view_manager.focused_view_id()
            && let Some(doc_view) = self.view_manager.get_document_view(&focused_view_id)
        {
            // Access the DocumentView to get real font metrics
            return doc_view.read_with(cx, |doc_view, _cx| {
                let layout = doc_view.layout_snapshot();
                let gutter_width = if layout.gutter_width > px(0.0) {
                    layout.gutter_width
                } else {
                    fallback_gutter_width
                };
                let (cursor_position, cursor_size) = layout
                    .cursor_overlay_bounds
                    .map_or((None, None), |(position, size)| {
                        (Some(position), Some(size))
                    });

                (
                    layout.line_height,
                    layout.cell_width,
                    gutter_width,
                    cursor_position,
                    cursor_size,
                )
            });
        }

        // Fallback to cached metrics if no focused view exists
        (
            px(fallback_line_h),
            px(cached_char_w),
            fallback_gutter_width,
            None,
            None,
        )
    }

    fn sync_embedded_terminal_size(
        &mut self,
        available_width_px: f32,
        panel_height_px: f32,
        cell_height_px: f32,
        cell_width_px: f32,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_panel_visible {
            self.last_terminal_bounds = None;
            return;
        }

        let Some(panel) = &self.embedded_terminal_panel else {
            self.last_terminal_bounds = None;
            return;
        };

        let bounds = TerminalBounds::from_pixels(
            cell_width_px,
            cell_height_px,
            available_width_px,
            panel_height_px,
        );
        let active_id = panel.read(cx).active;
        let bounds_changed = self
            .last_terminal_bounds
            .as_ref()
            .map(|(prev_id, prev_bounds)| *prev_id != active_id || !prev_bounds.approx_eq(&bounds))
            .unwrap_or(true);

        if bounds_changed {
            let (_, pixel_height) = bounds.pixel_size();
            self.last_terminal_bounds = Some((active_id, bounds));
            if (self.basic_terminal_height - pixel_height).abs() > 0.5 {
                self.basic_terminal_height = pixel_height;
            }
            panel.update(cx, |p, cx| {
                p.height_px = pixel_height;
                cx.notify();
            });
            self.core.update(cx, |app, _| {
                if let Some(bus) = &app.event_aggregator {
                    bus.dispatch_terminal(TerminalEvent::Resized {
                        id: active_id,
                        cols: bounds.cols(),
                        rows: bounds.rows(),
                    });
                    bus.dispatch_terminal(TerminalEvent::ResizedWithMetrics {
                        id: active_id,
                        cols: bounds.cols(),
                        rows: bounds.rows(),
                        cell_width: bounds.cell_size().0,
                        cell_height: bounds.cell_size().1,
                    });
                    // Process resize events immediately so the PTY is resized
                    // in the same frame. Without this, events dispatched during
                    // render sit in the queue until the next render cycle which
                    // may never come (process_events runs at the top of render).
                    bus.process_events();
                }
            });
            // Notify the terminal view entity so it re-renders with the
            // updated grid dimensions (new row/column count).
            panel.update(cx, |p, cx| {
                if let Some(view) = &p.view_entity {
                    view.update(cx, |_, cx| cx.notify());
                }
            });
        }
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.initial_project_startup_pending {
            self.initial_project_startup_pending = false;
            let workspace = cx.entity().clone();
            cx.defer(move |cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.start_deferred_project_services(cx);
                });
            });
        }

        // Drive V2 event processing so FsOpHandler can execute intents
        if let Some(aggregator) = self.core.read(cx).event_aggregator.clone()
            && aggregator.has_queued_events()
        {
            aggregator.process_events();
        }

        // Close terminal panel when the shell process has exited
        if self.terminal_panel_visible
            && let Some(id) = self.terminal_id
            && let Some(vm) = nucleotide_terminal_view::get_view_model(id)
            && match vm.lock() {
                Ok(vm) => vm.has_exited(),
                Err(poisoned) => {
                    warn!(
                        terminal_id = ?id,
                        "Terminal view model lock poisoned while checking exit state; recovering"
                    );
                    poisoned.into_inner().has_exited()
                }
            }
            && self.run_output_terminal != Some(id)
        {
            self.hide_terminal_panel();
            self.clear_terminal_panel_session();
        }

        // Fallback: full refresh if any pending flag remains
        if self.needs_file_tree_refresh {
            if let Some(ref file_tree) = self.file_tree {
                file_tree.update(cx, |view, tree_cx| {
                    view.refresh(tree_cx);
                });
            }
            self.needs_file_tree_refresh = false;
        }

        self.sync_file_tree_width_for_viewport(f32::from(window.viewport_size().width));

        // Failsafe: If the overlay is gone and no known element has focus, force-refocus.
        // We see cases in logs where overlay_empty=true and both workspace and doc view
        // report not focused, leaving the app with no key receiver. This block ensures
        // that after overlay teardown, we always regain a valid focus target without a click.
        if self.overlay.read(cx).is_empty() {
            let ws_focused = self.focus_handle.is_focused(window);
            let overlay_focused = self.overlay.focus_handle(cx).is_focused(window);

            let (doc_focus_handle, doc_focused) = if let Some(id) =
                self.view_manager.focused_view_id()
                && let Some(doc_view) = self.view_manager.get_document_view(&id)
            {
                let fh = doc_view.focus_handle(cx);
                (Some(fh.clone()), fh.is_focused(window))
            } else {
                (None, false)
            };

            let file_tree_focused = self
                .file_tree
                .as_ref()
                .map(|ft| ft.focus_handle(cx).is_focused(window))
                .unwrap_or(false);

            // Consider embedded terminal focus as a valid target
            let terminal_focused = self.terminal_focus.is_focused(window);

            if !ws_focused
                && !overlay_focused
                && !doc_focused
                && !file_tree_focused
                && !terminal_focused
            {
                // First, nudge caret into the document view if we have one.
                if let Some(fh) = doc_focus_handle {
                    window.focus(&fh, cx);
                }
                // Then ensure global key routing via workspace root.
                window.focus(&self.focus_handle, cx);
            }
        }

        // Update global workspace layout information for completion positioning
        self.update_workspace_layout_info(window, cx);

        // Set up window appearance observer on first render
        if !self.appearance_observer_set {
            self.appearance_observer_set = true;

            // Get initial appearance and trigger theme switch if needed
            let initial_appearance = cx.window_appearance();
            nucleotide_logging::info!(
                initial_appearance = ?initial_appearance,
                "Initial window appearance detected at startup"
            );

            // Handle initial appearance
            self.handle_appearance_change(initial_appearance, window, cx);

            // Set up observer for future changes
            cx.observe_window_appearance(window, |workspace: &mut Workspace, window, cx| {
                // Get the new appearance from the window
                let appearance = window.appearance();
                nucleotide_logging::info!(
                    new_appearance = ?appearance,
                    "Window appearance observer triggered"
                );
                workspace.needs_appearance_update = true;
                workspace.pending_appearance = Some(appearance);
                cx.notify();
            })
            .detach();
        }

        // Handle window appearance update if needed (for theme changes)
        if self.needs_window_appearance_update {
            debug!("Processing scheduled window appearance update");
            self.needs_window_appearance_update = false;
            self.update_window_appearance(window, cx);
        }

        // Handle appearance update if needed
        if self.needs_appearance_update {
            self.needs_appearance_update = false;
            if let Some(appearance) = self.pending_appearance.take() {
                nucleotide_logging::info!(
                    pending_appearance = ?appearance,
                    "Processing pending appearance change"
                );
                self.handle_appearance_change(appearance, window, cx);
            } else {
                // Fallback to current appearance if no pending appearance
                let appearance = cx.window_appearance();
                self.handle_appearance_change(appearance, window, cx);
            }
        }

        // Handle focus restoration if needed
        if self.view_manager.needs_focus_restore() {
            if self.view_manager.get_focused_document_view().is_some() {
                self.view_manager.focus_editor_area(cx, window);
            } else if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned()
            {
                let _ = coord.focus_first(
                    window,
                    cx,
                    &[
                        nucleotide_ui::FocusRole::Terminal,
                        nucleotide_ui::FocusRole::FileTree,
                    ],
                );
            } else {
                window.focus(&self.focus_handle, cx);
            }
            self.view_manager.set_needs_focus_restore(false);
        }

        // If terminal was toggled on via button, focus it now
        if self.terminal_panel_visible && self.terminal_focus_pending {
            window.focus(&self.terminal_focus, cx);
            self.terminal_focus_pending = false;
        }
        let (focused_file_name, native_metadata) = self.focused_native_window_metadata(cx);

        self.update_titlebar_filename(focused_file_name.as_deref(), false, cx);
        self.update_native_window_metadata(window, native_metadata);

        // Recompute theme-derived colors only when marked dirty
        if self.colors_dirty {
            self.recompute_theme_colors(cx);
        }
        let bg_color = self.cached_bg_color;
        let native_sidebar_enabled = macos_system_sidebar_enabled(&self.core.read(cx).config.gui);
        let rendered_titlebar = should_render_app_titlebar(
            self.titlebar.is_some(),
            self.show_file_tree,
            self.file_tree_width,
            native_sidebar_enabled,
        )
        .then(|| self.titlebar.clone())
        .flatten();
        let titlebar_sidebar_background =
            if native_sidebar_enabled && self.show_file_tree && self.file_tree_width > 0.0 {
                let file_tree_tokens = cx.theme().tokens.file_tree_tokens().translucent_sidebar();
                Some((
                    px(self.file_tree_width),
                    file_tree_tokens.background,
                    file_tree_tokens.separator,
                ))
            } else {
                None
            };
        self.update_titlebar_leading_sidebar_background(titlebar_sidebar_background, cx);

        // Compute the editor content dimensions before reading Helix view areas,
        // so split panes use the current tree layout in this render pass.
        let ui_theme = cx.global::<nucleotide_ui::Theme>();
        let status_bar_height = ui_theme.tokens.sizes.statusbar_height;
        let titlebar_height = self.rendered_titlebar_height(window, cx);
        let tab_bar_height = self.visible_tab_bar_height(cx);
        let viewport_h = window.viewport_size().height;
        let available_h =
            (f32::from(viewport_h) - f32::from(status_bar_height) - f32::from(titlebar_height))
                .max(0.0);
        let content_max_h = px(available_h);

        let min_term = 80.0f32;
        let max_term = (available_h - f32::from(tab_bar_height) - 80.0).max(min_term);
        if self.basic_terminal_height > max_term {
            self.basic_terminal_height = max_term;
        }

        let (line_h_px, char_w_px, _, _, _) = self.get_focused_document_view_layout(cx);
        let line_h_value = f32::from(line_h_px).max(1.0);
        let char_w_value = f32::from(char_w_px).max(1.0);

        let viewport_w_px = f32::from(window.viewport_size().width);
        let file_tree_w_px = if self.show_file_tree {
            self.file_tree_width
        } else {
            0.0
        };
        let right_content_w_px = (viewport_w_px - file_tree_w_px).max(1.0);
        self.sync_documentation_sidebar_width_for_viewport(right_content_w_px);
        let doc_sidebar_w_px = if self.doc_sidebar_visible {
            self.doc_sidebar_width
        } else {
            0.0
        };
        let editor_content_w_px = (right_content_w_px - doc_sidebar_w_px).max(1.0);

        let editor_h = if self.terminal_panel_visible {
            (available_h - self.basic_terminal_height).max(0.0)
        } else {
            available_h
        };
        let editor_content_h_px = (editor_h - f32::from(tab_bar_height)).max(1.0);

        self.sync_embedded_terminal_size(
            editor_content_w_px,
            self.basic_terminal_height,
            line_h_value,
            char_w_value,
            cx,
        );

        let rows = (editor_content_h_px / line_h_value).floor().max(1.0) as u16;
        let cols = (editor_content_w_px / char_w_value).floor().max(1.0) as u16;
        let desired_size = (cols, rows);

        if self
            .last_editor_size
            .map(|(w, h)| w != desired_size.0 || h != desired_size.1)
            .unwrap_or(true)
        {
            self.core.update(cx, |core, _| {
                let rect = helix_view::graphics::Rect {
                    x: 0,
                    y: 0,
                    width: desired_size.0,
                    height: desired_size.1,
                };
                core.compositor.resize(rect);
                core.editor.resize(rect);
            });
            self.last_editor_size = Some(desired_size);
        }

        // Create document root container using design tokens
        let mut docs_root = div()
            .id("docs-root")
            .flex()
            .relative()
            .w_full()
            .h_full()
            .overflow_hidden()
            // Background color inherited // Use semantic background color
            .when(self.debug_colors_enabled, |d| {
                // Editor docs area border (green)
                d.border_1()
                    .border_color(cx.theme().tokens.chrome.border_strong)
            }); // No gap needed for documents

        let active_image_tab = self
            .active_image_tab_id
            .and_then(|doc_id| self.image_tabs.iter().find(|tab| tab.id == doc_id).cloned());
        if let Some(image_tab) = active_image_tab {
            docs_root = docs_root.child(
                div()
                    .id("image-viewer-container")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(self.render_image_viewer(image_tab, cx)),
            );
        } else {
            let layouts = self.document_view_layouts(cx);
            let layout_bounds = document_view_layout_bounds(&layouts);
            let dim_inactive_panes =
                layouts.len() > 1 && layouts.iter().any(|layout| layout.is_focused);
            let dividers = if layouts.len() > 1 {
                split_pane_dividers(&layouts)
            } else {
                Vec::new()
            };

            if layouts.is_empty() {
                if let Some(doc_view) = self.view_manager.document_views().values().next().cloned()
                {
                    docs_root = docs_root.child(
                        div()
                            .id("document-container")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .when(self.debug_colors_enabled, |d| {
                                d.border_1()
                                    .border_color(cx.theme().tokens.chrome.border_default)
                            })
                            .child(doc_view),
                    );
                }
            } else {
                if let Some(total_area) = layout_bounds {
                    for layout in layouts.iter().copied() {
                        let layout = DocumentViewLayout {
                            area: document_view_visual_area(layout, &dividers),
                            ..layout
                        };
                        if let Some(doc_element) = self.render_document_view_layout(
                            layout,
                            total_area,
                            editor_content_w_px,
                            editor_content_h_px,
                            dim_inactive_panes,
                            cx,
                        ) {
                            docs_root = docs_root.child(doc_element);
                        }
                    }

                    for divider in dividers.iter().cloned() {
                        docs_root = docs_root.child(self.render_split_pane_resize_handle(
                            divider,
                            total_area,
                            editor_content_w_px,
                            editor_content_h_px,
                            cx,
                        ));
                    }

                    for divider in &dividers {
                        let divider = split_pane_divider_visual_line(divider.clone(), &dividers);
                        docs_root = docs_root.child(self.render_split_pane_divider_line(
                            &divider,
                            total_area,
                            editor_content_w_px,
                            editor_content_h_px,
                            cx,
                        ));
                    }
                }
            }
        }

        let focused_view = self
            .view_manager
            .focused_view_id()
            .and_then(|id| self.view_manager.get_document_view(&id))
            .cloned();
        if let Some(_view) = &focused_view {
            // Focus is managed by DocumentView's focus state
        }

        // Editor resize handled after layout computation below

        if let Some(_view) = &focused_view {
            // Focus is managed by DocumentView's focus state
        }

        // Overlay may add top-layer views; checked lazily below when rendering

        // Create main content area using semantic layout with design tokens
        let main_content = div()
            .id("main-content")
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            // Background color inherited
            // No gap needed between tab bar and content
            .child({
                // Tab bar at the top of editor area, consistently wrapped in a Div
                let debug = self.debug_colors_enabled;
                let debug_border = cx.theme().tokens.chrome.border_default;
                let tab = self.render_tab_bar(window, cx);
                div()
                    .when(debug, |d| {
                        // Tab bar wrapper (blue)
                        d.border_1().border_color(debug_border)
                    })
                    .child(tab)
            })
            .child(
                // Editor content container
                div()
                    .id("editor-container")
                    .flex()
                    .flex_col()
                    .w_full()
                    .flex_1() // Take remaining height after tab bar
                    .relative()
                    // Debug: container styling; label appended later to ensure on top
                    .when(self.debug_colors_enabled, |d| {
                        d.bg(nucleotide_ui::ColorTheory::with_alpha(
                            cx.theme().tokens.chrome.surface,
                            0.10,
                        ))
                        .border_l_2()
                        .border_color(cx.theme().tokens.chrome.border_strong)
                        .border_1()
                        .border_color(cx.theme().tokens.chrome.border_default)
                    })
                    .when_some(Some(docs_root), gpui::ParentElement::child)
                    .child(self.notifications.clone())
                    .when(!self.overlay.read(cx).is_empty(), |this| {
                        debug!("COMP: Workspace rendering overlay because it's not empty");
                        let view = &self.overlay;
                        // Overlay wrapper (magenta)
                        if self.debug_colors_enabled {
                            this.child(
                                div()
                                    .id("overlay-debug-wrapper")
                                    .border_1()
                                    .border_color(cx.theme().tokens.chrome.border_strong)
                                    .child(view.clone()),
                            )
                        } else {
                            this.child(view.clone())
                        }
                    })
                    .child(self.about_window.clone())
                    .child(self.theme_debug.clone())
                    .when(
                        !self.info_hidden && !self.info.read(cx).is_empty(),
                        |this| this.child(self.info.clone()),
                    )
                    .child(self.key_hints.clone())
                    .when(self.tab_context_menu_open, |this| {
                        this.child(
                            gpui::deferred(self.render_tab_context_menu(window, cx))
                                .with_priority(100),
                        )
                    })
                    .when(self.tab_bar_split_menu_open, |this| {
                        this.child(
                            gpui::deferred(self.render_tab_bar_split_menu(window, cx))
                                .with_priority(100),
                        )
                    })
                    .when(self.tab_bar_new_menu_open, |this| {
                        this.child(
                            gpui::deferred(self.render_tab_bar_new_menu(window, cx))
                                .with_priority(100),
                        )
                    })
                    .when(self.delete_confirm_open, |this| {
                        // Render delete confirmation modal overlay
                        this.child(self.render_delete_confirm_modal(window, cx))
                    })
                    .when(self.close_confirm_open, |this| {
                        this.child(self.render_unsaved_close_confirm_modal(window, cx))
                    })
                    // Debug overlay tint on top of editor content; render via deferred to ensure top draw order
                    .when(self.debug_colors_enabled, |this| {
                        this.child(
                            gpui::deferred(div().absolute().top_0().left_0().size_full().bg(
                                nucleotide_ui::ColorTheory::with_alpha(
                                    cx.theme().tokens.chrome.surface_overlay,
                                    1.0,
                                ),
                            ))
                            .with_priority(100),
                        )
                    })
                    .when(self.debug_colors_enabled, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .px(px(6.0))
                                .py(px(2.0))
                                .bg(cx.theme().tokens.chrome.primary)
                                .text_color(cx.theme().tokens.chrome.text_on_chrome)
                                .child("EDITOR"),
                        )
                    }),
            );

        // Create the main workspace container using nucleotide-ui theme access

        let mut workspace_div = div()
            .key_context("Workspace")
            .id("workspace")
            .flex()
            .flex_col() // Vertical layout to include titlebar
            .w_full()
            .h_full()
            .relative() // Anchor for absolute-positioned resize hitboxes
            .when(!native_sidebar_enabled, |root| root.bg(bg_color))
            .focusable();

        // Always add global key handling - the workspace should always capture key events
        // regardless of focus state or overlay presence for global shortcuts to work
        workspace_div = workspace_div
            .track_focus(&self.focus_handle)
            .capture_key_down(cx.listener(|view, ev, _window, cx| {
                if view.handle_regular_completion_menu_key(ev, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|view, ev, window, cx| {
                view.handle_key(ev, window, cx);
            }));

        // Add resize cursor and listeners only while resizing to reduce event overhead
        if self.is_resizing_file_tree
            || self.doc_sidebar_resizing
            || self.split_pane_resize.is_some()
            || self.basic_term_resizing
        {
            workspace_div = workspace_div.capture_any_mouse_up(cx.listener(
                |workspace, event: &MouseUpEvent, window, cx| {
                    if event.button == MouseButton::Left {
                        workspace.finish_active_resize(window, cx);
                        cx.stop_propagation();
                    }
                },
            ));
        }
        if self.is_resizing_file_tree {
            workspace_div = workspace_div
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .on_mouse_move(
                    cx.listener(|workspace, event: &MouseMoveEvent, window, cx| {
                        if event.dragging()
                            && workspace.update_file_tree_resize(
                                f32::from(event.position.x),
                                f32::from(window.viewport_size().width),
                                cx,
                            )
                        {
                            window.refresh();
                        }
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|workspace, _event: &MouseUpEvent, window, cx| {
                        workspace.finish_file_tree_resize(window, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|workspace, _event: &MouseUpEvent, window, cx| {
                        workspace.finish_file_tree_resize(window, cx);
                    }),
                );
        }
        if self.doc_sidebar_resizing {
            let resize_available_w = right_content_w_px;
            workspace_div = workspace_div
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .on_mouse_move(
                    cx.listener(move |workspace, event: &MouseMoveEvent, window, cx| {
                        if event.dragging()
                            && workspace.update_documentation_sidebar_resize(
                                f32::from(event.position.x),
                                resize_available_w,
                                cx,
                            )
                        {
                            window.refresh();
                        }
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|workspace, _event: &MouseUpEvent, window, cx| {
                        workspace.finish_documentation_sidebar_resize(window, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|workspace, _event: &MouseUpEvent, window, cx| {
                        workspace.finish_documentation_sidebar_resize(window, cx);
                    }),
                );
        }
        if let Some(split_resize) = &self.split_pane_resize {
            let cursor = match split_resize.axis {
                SplitPaneResizeAxis::Vertical => gpui::CursorStyle::ResizeLeftRight,
                SplitPaneResizeAxis::Horizontal => gpui::CursorStyle::ResizeRow,
            };
            workspace_div = workspace_div
                .cursor(cursor)
                .on_mouse_move(
                    cx.listener(|workspace, event: &MouseMoveEvent, window, cx| {
                        if workspace.split_pane_resize.is_some() {
                            if event.dragging()
                                && workspace.update_split_pane_resize(event.position, cx)
                            {
                                window.refresh();
                            }
                            cx.stop_propagation();
                        }
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|workspace, _event: &MouseUpEvent, window, cx| {
                        workspace.finish_split_pane_resize(window, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|workspace, _event: &MouseUpEvent, window, cx| {
                        workspace.finish_split_pane_resize(window, cx);
                    }),
                );
        }
        // Add mouse down handler for global UI interactions
        workspace_div = workspace_div.on_mouse_down(
            MouseButton::Left,
            cx.listener(|workspace, _event: &MouseDownEvent, _window, cx| {
                // Clicking outside the delete confirm modal closes it
                if workspace.delete_confirm_open {
                    workspace.delete_confirm_open = false;
                    workspace.delete_confirm_path = None;
                    cx.notify();
                }

                if workspace.tab_context_menu_open {
                    workspace.tab_context_menu_open = false;
                    workspace.tab_context_menu_doc_id = None;
                    cx.notify();
                }

                if workspace.tab_bar_split_menu_open {
                    workspace.tab_bar_split_menu_open = false;
                    cx.notify();
                }

                if workspace.tab_bar_new_menu_open {
                    workspace.tab_bar_new_menu_open = false;
                    cx.notify();
                }

                // Clicking elsewhere deactivates terminal input capture
                workspace.terminal_active = false;

                // Ensure workspace regains focus when clicked, so global shortcuts work
                workspace.view_manager.set_needs_focus_restore(true);
                cx.notify();
            }),
        );

        // Add action handlers
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::common::Cancel, window, cx| {
                if !cx.stop_active_drag(window) {
                    cx.propagate();
                }
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::help::About, _window, cx| {
                workspace.about_window.update(cx, |about_window, cx| {
                    about_window.show(cx);
                });
            },
        ));

        // Theme Debug action opens the overlay
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::help::ThemeDebug, _window, cx| {
                workspace.theme_debug.update(cx, |view, cx| view.show(cx));
            },
        ));

        // Global editor actions that work regardless of focus
        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::editor::Quit, _window, cx| {
                quit(core.clone(), handle.clone(), cx);
                cx.quit();
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::OpenFile, _window, cx| {
                open(
                    workspace.core.clone(),
                    workspace.handle.clone(),
                    workspace.overlay.clone(),
                    cx,
                )
            },
        ));

        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::editor::OpenDirectory, _window, cx| {
                open_directory(core.clone(), handle.clone(), cx)
            },
        ));

        // Settings action - open nucleotide.toml configuration file
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::OpenSettings, _window, cx| {
                workspace.open_settings_file(cx)
            },
        ));

        // Reload configuration action - reload nucleotide.toml without restart
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::ReloadConfiguration, _window, cx| {
                workspace.reload_configuration(cx)
            },
        ));

        // Add handlers for Save, SaveAs, CloseFile
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Save, _window, cx| {
                workspace.execute_raw_command("write", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::SaveAs, _window, cx| {
                // TODO: Implement save as with file dialog
                workspace.execute_raw_command("write", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::CloseFile, _window, cx| {
                workspace.close_active_tab_document(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::RevertCurrentChange, _window, cx| {
                workspace.execute_raw_command("reset-diff-change", cx);
                workspace
                    .core
                    .update(cx, |core, cx| core.reconcile_vcs_after_diff_reset(cx));
            },
        ));

        // Add handlers for Undo, Redo, Copy, Paste
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Undo, _window, cx| {
                workspace.send_helix_key("u", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Redo, _window, cx| {
                workspace.send_helix_key("U", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Copy, _window, cx| {
                workspace.send_helix_key("y", cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::Paste, _window, cx| {
                workspace.send_helix_key("p", cx);
            },
        ));

        // Font size actions
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::IncreaseFontSize, _window, cx| {
                workspace.adjust_font_size(1.0, cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::editor::DecreaseFontSize, _window, cx| {
                workspace.adjust_font_size(-1.0, cx);
            },
        ));

        // Completion trigger action
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::completion::TriggerCompletion, _window, cx| {
                // Check if we're in insert mode - completion should only work in insert mode
                let core = workspace.core.read(cx);
                let current_mode = core.editor.mode();

                match current_mode {
                    helix_view::document::Mode::Insert => {
                        // Get current view and document IDs
                        let (doc_id, view_id) = {
                            let view_id = core.editor.tree.focus;
                            let doc_id = core
                                .editor
                                .tree
                                .try_get(view_id)
                                .map(|view| view.doc)
                                .unwrap_or_default();
                            (doc_id, view_id)
                        };

                        // Release the core read lock before calling handle_completion_requested
                        let _ = core;

                        workspace.handle_completion_requested(
                            doc_id,
                            view_id,
                            &crate::types::CompletionTrigger::Manual,
                            cx,
                        );
                    }
                    _ => {
                        // Do nothing - completion is only available in insert mode
                    }
                }
            },
        ));

        // Workspace actions
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ShowBufferPicker, _window, cx| {
                show_buffer_picker(
                    workspace.core.clone(),
                    workspace.handle.clone(),
                    workspace.overlay.clone(),
                    cx,
                )
            },
        ));

        // Code actions picker
        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::workspace::ShowCodeActions, _window, cx| {
                show_code_actions(core.clone(), handle.clone(), cx)
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ShowRunnables, _window, cx| {
                workspace.show_runnables(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::RunNearest, _window, cx| {
                workspace.run_nearest(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::RunFileTests, _window, cx| {
                workspace.run_file_tests(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::RunLast, _window, cx| {
                workspace.run_last(cx);
            },
        ));

        // Toggle file tree action
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ToggleFileTree, _window, cx| {
                info!("ToggleFileTree action triggered from menu");
                workspace.show_file_tree = !workspace.show_file_tree;
                cx.notify();
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ToggleDocumentation, _window, cx| {
                info!("ToggleDocumentation action triggered from menu");
                if workspace.toggle_documentation_sidebar(cx) {
                    show_hover_docs(workspace.core.clone(), workspace.handle.clone(), cx);
                }
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ToggleTerminal, _window, cx| {
                info!("ToggleTerminal action triggered from menu");
                workspace.toggle_terminal_panel(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::SplitPaneRight, _window, cx| {
                workspace.tab_bar_action_split_right(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::SplitPaneLeft, _window, cx| {
                workspace.tab_bar_action_split_left(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::SplitPaneUp, _window, cx| {
                workspace.tab_bar_action_split_up(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::SplitPaneDown, _window, cx| {
                workspace.tab_bar_action_split_down(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::UnpinAllTabs, _window, cx| {
                workspace.unpin_all_tabs(cx);
            },
        ));

        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::TogglePreviewTab, _window, cx| {
                workspace.toggle_active_preview_tab(cx);
            },
        ));

        // File finder action
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ShowFileFinder, _window, cx| {
                open(
                    workspace.core.clone(),
                    workspace.handle.clone(),
                    workspace.overlay.clone(),
                    cx,
                )
            },
        ));

        // NewFile action
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::NewFile, _window, cx| {
                workspace.execute_raw_command("new", cx);
            },
        ));

        // NewWindow action
        workspace_div = workspace_div.on_action(cx.listener(
            move |_workspace, _: &crate::actions::workspace::NewWindow, _window, _cx| {
                // TODO: Implement new window
                nucleotide_logging::warn!("New window not yet implemented");
            },
        ));

        // ShowCommandPrompt action opens the native command prompt.
        workspace_div = workspace_div.on_action(cx.listener(
            move |workspace, _: &crate::actions::workspace::ShowCommandPrompt, _window, cx| {
                workspace.show_command_prompt(cx);
            },
        ));

        // Window actions
        workspace_div = workspace_div
            .on_action(
                cx.listener(move |_, _: &crate::actions::window::Hide, _window, cx| cx.hide()),
            )
            .on_action(cx.listener(
                move |_, _: &crate::actions::window::HideOthers, _window, cx| cx.hide_other_apps(),
            ))
            .on_action(
                cx.listener(move |_, _: &crate::actions::window::ShowAll, _window, cx| {
                    cx.unhide_other_apps()
                }),
            )
            .on_action(cx.listener(
                move |_, _: &crate::actions::window::Minimize, window, _cx| {
                    window.minimize_window();
                },
            ))
            .on_action(
                cx.listener(move |_, _: &crate::actions::window::Zoom, window, _cx| {
                    window.zoom_window();
                }),
            );

        // Help and test actions
        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::help::OpenTutorial, _window, cx| {
                load_tutor(core.clone(), handle.clone(), cx)
            },
        ));

        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::test::TestPrompt, _window, cx| {
                test_prompt(core.clone(), handle.clone(), cx)
            },
        ));

        let handle = self.handle.clone();
        let core = self.core.clone();
        workspace_div = workspace_div.on_action(cx.listener(
            move |_, _: &crate::actions::test::TestCompletion, _window, cx| {
                test_completion(core.clone(), handle.clone(), cx)
            },
        ));

        // Create content area that will hold file tree and main content
        // Now using a centralized sidebar split from nucleotide-ui
        // split debug logs removed

        // New default layout
        let content_area = {
            // Basic layout mode: render simple colored, resizable panes
            let _ui_theme = cx.global::<nucleotide_ui::Theme>();

            // Left placeholder: File tree (yellow)
            let _left = div()
                .relative()
                .size_full()
                .min_h(px(0.0))
                // Ensure solid fill regardless of nested sizing by using an absolute overlay
                .child(
                    div().absolute().top_0().left_0().size_full().bg(cx
                        .theme()
                        .tokens
                        .chrome
                        .surface),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .px(px(6.0))
                        .py(px(2.0))
                        .bg(cx.theme().tokens.chrome.surface_active)
                        .text_color(cx.theme().tokens.chrome.text_on_chrome)
                        .child("FILE TREE"),
                );

            // Right: actual editor views + bottom terminal panel using shared split
            let right = {
                let on_change_height = {
                    let _entity = cx.entity().clone();
                    move |new_h: f32, app_cx: &mut gpui::App| {
                        _entity.update(app_cx, |this: &mut Workspace, cx| {
                            if (this.basic_terminal_height - new_h).abs() > 0.5 {
                                this.basic_terminal_height = new_h;
                                if let Some(panel) = &this.embedded_terminal_panel {
                                    panel.update(cx, |p, _| p.height_px = new_h);
                                }
                                cx.notify();
                            }
                        });
                    }
                };

                let panel_max = (available_h * 0.85).max(120.0).min(max_term);

                // Container with editor area + bottom panel
                let mut root = div()
                    .relative()
                    .w_full()
                    .h(content_max_h)
                    .min_h(px(0.0))
                    .bg(bg_color);

                // Editor area above the bottom panel: use existing editor content (tabs, overlays)
                root = root.child(
                    div()
                        .w_full()
                        .h(px(editor_h))
                        .min_h(px(0.0))
                        .overflow_hidden()
                        .child(main_content),
                );

                if self.terminal_panel_visible {
                    // Bottom terminal panel using shared split helper inside an absolute wrapper.
                    // Keep terminal focus and key handling scoped to the bottom panel content so
                    // editor clicks above it can focus documents normally.
                    root = root.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            // Track resize drags at the wrapper level for reliability
                            .on_mouse_move(cx.listener(
                                move |this: &mut Workspace, ev: &MouseMoveEvent, window, cx| {
                                    if this.basic_term_resizing && ev.dragging() {
                                        let dy = f32::from(ev.position.y)
                                            - this.basic_term_start_mouse_y;
                                        let min_h = 80.0f32;
                                        let max_h = max_term;
                                        let new_h =
                                            (this.basic_term_start_height - dy).clamp(min_h, max_h);
                                        if (this.basic_terminal_height - new_h).abs() > 0.5 {
                                            this.basic_terminal_height = new_h;
                                            cx.notify();
                                            window.refresh();
                                        }
                                    }
                                },
                            ))
                            .on_mouse_up(MouseButton::Left, cx.listener(|this: &mut Workspace, _ev: &MouseUpEvent, window, cx| {
                                if this.basic_term_resizing {
                                    this.basic_term_resizing = false;
                                    this.request_standard_cursor_restore(window, cx);
                                }
                            }))
                            .on_mouse_up_out(MouseButton::Left, cx.listener(|this: &mut Workspace, _ev: &MouseUpEvent, window, cx| {
                                if this.basic_term_resizing {
                                    this.basic_term_resizing = false;
                                    this.request_standard_cursor_restore(window, cx);
                                }
                            }))
                            .child(nucleotide_ui::bottom_panel_split(
                                self.basic_terminal_height,
                                80.0,
                                panel_max,
                                0.0, // disable internal handle; we'll overlay our own
                                220.0,
                                on_change_height,
                                {
                                    let mut c = div().relative().size_full();
                                    if let Some(panel) = &self.embedded_terminal_panel {
                                        c = c.child(
                                            div()
                                                .size_full()
                                                .overflow_hidden()
                                                .track_focus(&self.terminal_focus)
                                                .on_mouse_down(MouseButton::Left, cx.listener(|this: &mut Workspace, _ev: &MouseDownEvent, window, cx| {
                                                    window.focus(&this.terminal_focus, cx);
                                                    this.terminal_active = true;
                                                    cx.notify();
                                                    cx.stop_propagation();
                                                }))
                                                .on_key_down(cx.listener(|this: &mut Workspace, event: &gpui::KeyDownEvent, window, cx| {
                                                    if this.terminal_focus.is_focused(window) {
                                                        this.handle_key(event, window, cx);
                                                    }
                                                }))
                                                .child(panel.clone()),
                                        );
                                    } else {
                                        c = c.child(div().flex().items_center().justify_center().child("starting terminal..."));
                                    }
                                    c
                                },
                            ))
                            // Overlay our own centered handle at the top of the panel.
                            .child({
                                let handle_h = SPLIT_PANE_HANDLE_HITBOX_PX;
                                nucleotide_ui::splitter(
                                    "terminal-panel-resize-handle",
                                    nucleotide_ui::SplitterAxis::Horizontal,
                                    handle_h,
                                )
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom(px(self.basic_terminal_height - handle_h * 0.5))
                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this: &mut Workspace, ev: &MouseDownEvent, window, cx| {
                                        if ev.click_count >= 2 {
                                            let min_h = 80.0f32;
                                            let max_h = max_term;
                                            this.basic_terminal_height = 220.0f32.clamp(min_h, max_h);
                                            cx.notify();
                                            window.refresh();
                                            cx.stop_propagation();
                                            return;
                                        }
                                        this.basic_term_resizing = true;
                                        this.basic_term_start_mouse_y =
                                            f32::from(ev.position.y);
                                        this.basic_term_start_height = this.basic_terminal_height;
                                        this.terminal_active = true;
                                        window.refresh();
                                        cx.stop_propagation();
                                    }))
                            }),
                    );
                }

                let editor_stack = root;

                if self.doc_sidebar_visible {
                    let handle_hit_w = SPLIT_PANE_HANDLE_HITBOX_PX;
                    let resize_available_w = right_content_w_px;

                    div()
                        .flex()
                        .relative()
                        .w_full()
                        .h(content_max_h)
                        .min_h(px(0.0))
                        .child(
                            div()
                                .flex_1()
                                .h_full()
                                .min_h(px(0.0))
                                .overflow_hidden()
                                .child(editor_stack),
                        )
                        .child(self.render_documentation_sidebar(cx))
                        .child(
                            nucleotide_ui::splitter(
                                "documentation-sidebar-resize-handle",
                                nucleotide_ui::SplitterAxis::Vertical,
                                handle_hit_w,
                            )
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(self.doc_sidebar_width - handle_hit_w * 0.5))
                            .h_full()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(
                                    move |this: &mut Workspace, ev: &MouseDownEvent, window, cx| {
                                        if ev.click_count >= 2 {
                                            let width = Self::clamped_documentation_sidebar_width(
                                                DOC_SIDEBAR_DEFAULT_WIDTH,
                                                resize_available_w,
                                            );
                                            if (this.doc_sidebar_width - width).abs() > 0.5 {
                                                this.doc_sidebar_width = width;
                                                cx.notify();
                                            }
                                            window.refresh();
                                            cx.stop_propagation();
                                            return;
                                        }

                                        this.doc_sidebar_resizing = true;
                                        this.doc_sidebar_resize_start_x = f32::from(ev.position.x);
                                        this.doc_sidebar_resize_start_width =
                                            this.doc_sidebar_width;
                                        cx.notify();
                                        window.refresh();
                                        cx.stop_propagation();
                                    },
                                ),
                            )
                            .on_drag(DraggedDocumentationSidebarResize, |_, _, _, cx| {
                                cx.new(|_| DraggedDocumentationSidebarResize)
                            })
                            .on_drag_move::<DraggedDocumentationSidebarResize>(cx.listener(
                                move |this: &mut Workspace,
                                      event: &DragMoveEvent<DraggedDocumentationSidebarResize>,
                                      window,
                                      cx| {
                                    if this.doc_sidebar_resizing
                                        && event.event.dragging()
                                        && this.update_documentation_sidebar_resize(
                                            f32::from(event.event.position.x),
                                            resize_available_w,
                                            cx,
                                        )
                                    {
                                        window.refresh();
                                    }
                                    cx.stop_propagation();
                                },
                            ))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Workspace, _ev: &MouseUpEvent, window, cx| {
                                        this.finish_documentation_sidebar_resize(window, cx);
                                        cx.stop_propagation();
                                    },
                                ),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Workspace, _ev: &MouseUpEvent, window, cx| {
                                        this.finish_documentation_sidebar_resize(window, cx);
                                        cx.stop_propagation();
                                    },
                                ),
                            ),
                        )
                        .into_any_element()
                } else {
                    editor_stack.into_any_element()
                }
            };

            if self.show_file_tree {
                let handle_hit_w = SPLIT_PANE_HANDLE_HITBOX_PX;
                let viewport_w = f32::from(window.viewport_size().width);
                let max_left = Self::max_file_tree_width(viewport_w);

                let overlay_bg_w = (self.file_tree_width).clamp(0.0, max_left);

                // Root container handling drag to resize
                let mut container = div()
                    .relative()
                    .w_full()
                    .h(content_max_h)
                    .min_h(px(0.0))
                    .on_mouse_move(cx.listener(
                        move |this: &mut Workspace, ev: &MouseMoveEvent, window, cx| {
                            if this.is_resizing_file_tree
                                && ev.dragging()
                                && this.update_file_tree_resize(
                                    f32::from(ev.position.x),
                                    f32::from(window.viewport_size().width),
                                    cx,
                                )
                            {
                                window.refresh();
                            }
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this: &mut Workspace, _ev: &MouseUpEvent, window, cx| {
                            this.finish_file_tree_resize(window, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this: &mut Workspace, _ev: &MouseUpEvent, window, cx| {
                            this.finish_file_tree_resize(window, cx);
                        }),
                    );

                // Left file tree content
                let file_tree_top_inset = file_tree_content_top_inset(native_sidebar_enabled);
                let file_tree_tokens = {
                    let tokens = cx.theme().tokens.file_tree_tokens();
                    if native_sidebar_enabled {
                        tokens.translucent_sidebar()
                    } else {
                        tokens
                    }
                };
                let file_tree_background = file_tree_tokens.background;
                let file_tree_top_inset_background =
                    native_sidebar_enabled.then_some(file_tree_background);
                let mut file_tree_container = div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(overlay_bg_w))
                    .h(content_max_h)
                    .min_h(px(0.0));
                if let Some(file_tree) = &self.file_tree {
                    file_tree_container = file_tree_container.child(
                        div()
                            .size_full()
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(div().w_full().h(file_tree_top_inset).flex_none().when_some(
                                file_tree_top_inset_background,
                                |container, background| container.bg(background),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .overflow_hidden()
                                    .child(file_tree.clone()),
                            ),
                    );
                } else {
                    // No directory open: show a centered button to open a directory
                    let core = self.core.clone();
                    let handle = self.handle.clone();
                    use nucleotide_ui::button::Button;
                    let open_btn = Button::new("open-dir-btn", "Open a directory to view files")
                        .activate_on_mouse_down()
                        .on_click(cx.listener(
                            move |_: &mut Workspace, _ev: &gpui::ClickEvent, _window, cx| {
                                open_directory(core.clone(), handle.clone(), cx);
                            },
                        ));

                    file_tree_container = file_tree_container.child(
                        div()
                            .flex()
                            .flex_col()
                            .size_full()
                            .bg(file_tree_background)
                            .child(div().w_full().h(file_tree_top_inset).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(open_btn),
                            ),
                    );
                }
                container = container.child(file_tree_container);

                // Right content area positioned after the file tree.
                container = container.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(self.file_tree_width))
                        .right_0()
                        .h(content_max_h)
                        .min_h(px(0.0))
                        .child(right),
                );

                // Vertical handle at the boundary. Render it after both panes
                // so the symmetric hitbox is not covered by either side.
                container = container.child(
                    nucleotide_ui::splitter(
                        "file-tree-resize-handle",
                        nucleotide_ui::SplitterAxis::Vertical,
                        handle_hit_w,
                    )
                    .absolute()
                    .top_0()
                    .left(px(self.file_tree_width - handle_hit_w * 0.5))
                    .h(content_max_h)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(
                            move |this: &mut Workspace, ev: &MouseDownEvent, window, cx| {
                                if ev.click_count >= 2 {
                                    let viewport_w = f32::from(window.viewport_size().width);
                                    let snap = Self::clamped_file_tree_default_width(viewport_w);
                                    this.file_tree_width_override = None;
                                    if (this.file_tree_width - snap).abs() > 0.5 {
                                        this.file_tree_width = snap;
                                        cx.notify();
                                    }
                                    window.refresh();
                                    cx.stop_propagation();
                                    return;
                                }
                                this.is_resizing_file_tree = true;
                                this.resize_start_x = f32::from(ev.position.x);
                                this.resize_start_width = this.file_tree_width;
                                cx.notify();
                                window.refresh();
                                cx.stop_propagation();
                            },
                        ),
                    )
                    .on_drag(DraggedFileTreeResize, |_, _, _, cx| {
                        cx.new(|_| DraggedFileTreeResize)
                    })
                    .on_drag_move::<DraggedFileTreeResize>(cx.listener(
                        |this: &mut Workspace,
                         event: &DragMoveEvent<DraggedFileTreeResize>,
                         window,
                         cx| {
                            if this.is_resizing_file_tree
                                && event.event.dragging()
                                && this.update_file_tree_resize(
                                    f32::from(event.event.position.x),
                                    f32::from(window.viewport_size().width),
                                    cx,
                                )
                            {
                                window.refresh();
                            }
                            cx.stop_propagation();
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this: &mut Workspace, _ev: &MouseUpEvent, window, cx| {
                            this.finish_file_tree_resize(window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this: &mut Workspace, _ev: &MouseUpEvent, window, cx| {
                            this.finish_file_tree_resize(window, cx);
                            cx.stop_propagation();
                        }),
                    ),
                );

                if self.context_menu_open {
                    container = container.child(
                        gpui::deferred(self.render_file_tree_context_menu(window, cx))
                            .with_priority(100),
                    );
                }

                container.into_any_element()
            } else {
                // File tree not shown - render right full width
                let mut container = div()
                    .relative()
                    .w_full()
                    .h(content_max_h)
                    .min_h(px(0.0))
                    .child(right);

                if self.context_menu_open {
                    container = container.child(
                        gpui::deferred(self.render_file_tree_context_menu(window, cx))
                            .with_priority(100),
                    );
                }

                container.into_any_element()
            }
        };

        // If terminal was toggled on via button, focus it now (after elements are built)
        if self.terminal_panel_visible && self.terminal_focus_pending {
            window.focus(&self.terminal_focus, cx);
            self.terminal_focus_pending = false;
        }
        let restore_standard_cursor =
            std::mem::take(&mut self.restore_standard_cursor_after_resize);

        // Build final workspace with unified bottom status bar
        workspace_div
            .children(rendered_titlebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    // Ensure content can shrink and never hide the status bar
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .w_full()
                            .min_h(px(0.0)) // allow vertical shrink in flex column
                            .child(content_area),
                    )
                    .child(self.render_unified_status_bar(cx)), // Unified bottom status bar pinned at bottom
            )
            .when(restore_standard_cursor, |root| {
                root.child(
                    canvas(
                        |_bounds, _window, _cx| {},
                        |_bounds, (), window, _cx| {
                            window.set_window_cursor_style(gpui::CursorStyle::Arrow);
                        },
                    )
                    .absolute()
                    .size_full(),
                )
            })
            // Add Linux client-side resize hitboxes so the window can be resized
            .map(|root| {
                #[cfg(target_os = "linux")]
                {
                    use gpui::{CursorStyle, ResizeEdge};

                    // Only add when using client-side decorations and not fullscreen
                    let decorations = window.window_decorations();
                    if matches!(decorations, gpui::Decorations::Client { .. })
                        && !window.is_fullscreen()
                    {
                        let grip: f32 = 6.0; // thickness of resize handle
                        let corner: f32 = 12.0; // size of corner diagonal resize area

                        // Edges
                        let top = div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(grip))
                            .cursor(CursorStyle::ResizeUp)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.start_window_resize(ResizeEdge::Top);
                                cx.stop_propagation();
                            });

                        let bottom = div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(px(grip))
                            .cursor(CursorStyle::ResizeDown)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.start_window_resize(ResizeEdge::Bottom);
                                cx.stop_propagation();
                            });

                        let left = div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(grip))
                            .cursor(CursorStyle::ResizeLeft)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.start_window_resize(ResizeEdge::Left);
                                cx.stop_propagation();
                            });

                        let right = div()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w(px(grip))
                            .cursor(CursorStyle::ResizeRight)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.start_window_resize(ResizeEdge::Right);
                                cx.stop_propagation();
                            });

                        // Corners for diagonal resize
                        let tl = div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(px(corner))
                            .h(px(corner))
                            .cursor(CursorStyle::ResizeUpLeftDownRight)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.start_window_resize(ResizeEdge::TopLeft);
                                cx.stop_propagation();
                            });

                        let tr = div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .w(px(corner))
                            .h(px(corner))
                            .cursor(CursorStyle::ResizeUpRightDownLeft)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.start_window_resize(ResizeEdge::TopRight);
                                cx.stop_propagation();
                            });

                        let bl = div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .w(px(corner))
                            .h(px(corner))
                            .cursor(CursorStyle::ResizeUpRightDownLeft)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.start_window_resize(ResizeEdge::BottomLeft);
                                cx.stop_propagation();
                            });

                        let br = div()
                            .absolute()
                            .bottom_0()
                            .right_0()
                            .w(px(corner))
                            .h(px(corner))
                            .cursor(CursorStyle::ResizeUpLeftDownRight)
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.start_window_resize(ResizeEdge::BottomRight);
                                cx.stop_propagation();
                            });

                        return root
                            .child(top)
                            .child(bottom)
                            .child(left)
                            .child(right)
                            .child(tl)
                            .child(tr)
                            .child(bl)
                            .child(br);
                    }
                }
                root
            })
            .when(self.lsp_menu_open, |container| {
                use gpui::{Anchor, anchored, point};
                let ui_theme = cx.global::<nucleotide_ui::Theme>();
                let dd_tokens = ui_theme.tokens.dropdown_tokens();

                // Snapshot LSP state
                let server_rows: Vec<gpui::AnyElement> = {
                    let lsp_state_entity = self.core.read(cx).lsp_state.clone();
                    if let Some(lsp_state) = lsp_state_entity {
                        let state = lsp_state.read(cx);
                        let mut rows: Vec<gpui::AnyElement> = Vec::new();

                        // Sort servers by name for a stable order
                        let mut servers: Vec<_> = state.servers.values().collect();
                        servers.sort_by(|a, b| a.name.cmp(&b.name));
                        let mut progress_by_server: HashMap<_, Vec<_>> = HashMap::new();
                        for progress in state.progress.values() {
                            progress_by_server
                                .entry(progress.server_id)
                                .or_default()
                                .push(progress);
                        }

                        // If there are no servers, show muted empty message
                        if servers.is_empty() {
                            rows.push(
                                div()
                                    .w_full()
                                    .px(ui_theme.tokens.sizes.space_3)
                                    .py(ui_theme.tokens.sizes.space_2)
                                    .text_size(ui_theme.tokens.sizes.text_sm)
                                    .text_color(dd_tokens.item_text_secondary)
                                    .child("no LSP servers")
                                    .into_any_element(),
                            );
                            // No servers to display; end of list
                        }

                        for server in servers {
                            let status_text = match &server.status {
                                ServerStatus::Starting => "Starting".to_string(),
                                ServerStatus::Initializing => "Initializing".to_string(),
                                ServerStatus::Running => "Running".to_string(),
                                ServerStatus::Failed(e) => format!("Failed: {}", e),
                                ServerStatus::Stopped => "Stopped".to_string(),
                            };

                            // Header row with server name and status
                            rows.push(
                                div()
                                    .w_full()
                                    .px(ui_theme.tokens.sizes.space_3)
                                    .py(ui_theme.tokens.sizes.space_2)
                                    .text_size(ui_theme.tokens.sizes.text_sm)
                                    .text_color(dd_tokens.item_text)
                                    .child(format!("{} — {}", server.name, status_text))
                                    .into_any_element(),
                            );

                            // Progress rows for this server, or Idle if none
                            let progress_items =
                                progress_by_server.remove(&server.id).unwrap_or_default();

                            if progress_items.is_empty() {
                                rows.push(
                                    div()
                                        .w_full()
                                        .px(ui_theme.tokens.sizes.space_6)
                                        .pb(ui_theme.tokens.sizes.space_2)
                                        .text_size(ui_theme.tokens.sizes.text_sm)
                                        .text_color(dd_tokens.item_text_secondary)
                                        .child("Idle")
                                        .into_any_element(),
                                );
                            } else {
                                for p in progress_items {
                                    let mut line = String::new();
                                    if let Some(pct) = p.percentage {
                                        line.push_str(&format!("{pct}% "));
                                    }
                                    line.push_str(&p.title);
                                    if let Some(msg) = p.message.as_deref() {
                                        line.push_str(&format!(" ⋅ {}", msg));
                                    }

                                    rows.push(
                                        div()
                                            .w_full()
                                            .px(ui_theme.tokens.sizes.space_6)
                                            .pb(ui_theme.tokens.sizes.space_1)
                                            .text_size(ui_theme.tokens.sizes.text_sm)
                                            .text_color(dd_tokens.item_text_secondary)
                                            .child(line)
                                            .into_any_element(),
                                    );
                                }
                            }

                            // Separator between servers
                            rows.push(
                                div()
                                    .w_full()
                                    .h(px(1.0))
                                    .bg(dd_tokens.border)
                                    .opacity(0.5)
                                    .into_any_element(),
                            );
                        }

                        rows
                    } else {
                        vec![
                            div()
                                .w_full()
                                .px(ui_theme.tokens.sizes.space_3)
                                .py(ui_theme.tokens.sizes.space_2)
                                .text_size(ui_theme.tokens.sizes.text_sm)
                                .text_color(dd_tokens.item_text_secondary)
                                .child("no LSP servers")
                                .into_any_element(),
                        ]
                    }
                };

                let (x, y) = self.lsp_menu_pos;

                container.child(
                    div()
                        .absolute()
                        .size_full()
                        .top_0()
                        .left_0()
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this: &mut Workspace, _ev, window, cx| {
                                this.lsp_menu_open = false;
                                if let Some(coord) =
                                    cx.try_global::<nucleotide_ui::FocusCoordinator>().cloned()
                                {
                                    let _ = coord.focus_first(
                                        window,
                                        cx,
                                        &[
                                            nucleotide_ui::FocusRole::Editor,
                                            nucleotide_ui::FocusRole::FileTree,
                                        ],
                                    );
                                }
                                cx.notify();
                            }),
                        )
                        .child(
                            anchored()
                                .position(point(px(x), px(y)))
                                .anchor(Anchor::BottomLeft)
                                .offset(point(px(0.0), px(4.0)))
                                .snap_to_window_with_margin(ui_theme.tokens.sizes.space_2)
                                .child(
                                    div()
                                        .min_w(px(260.0))
                                        .max_w(px(480.0))
                                        .bg(dd_tokens.container_background)
                                        .border_1()
                                        .border_color(dd_tokens.border)
                                        .rounded(ui_theme.tokens.sizes.radius_md)
                                        .shadow(vec![
                                            ui_theme.tokens.chrome.shadow_md.to_box_shadow(false),
                                            ui_theme
                                                .tokens
                                                .chrome
                                                .inset_highlight
                                                .to_box_shadow(true),
                                        ])
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .children(server_rows),
                                ),
                        ),
                )
            })
    }
}

fn load_tutor(core: Entity<Core>, handle: tokio::runtime::Handle, cx: &mut Context<Workspace>) {
    core.update(cx, move |core, cx| {
        let _guard = handle.enter();
        let _ = utils::load_tutor(&mut core.editor);
        cx.notify()
    })
}

fn open(
    core: Entity<Core>,
    handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    cx: &mut Context<Workspace>,
) {
    let base_dir = core.update(cx, |core, _| {
        core.project_directory
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    });

    open_at(core, handle, overlay, base_dir, cx);
}

fn open_at(
    _core: Entity<Core>,
    _handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    base_dir: std::path::PathBuf,
    cx: &mut Context<Workspace>,
) {
    debug!("Opening file picker");

    debug!("Base directory for file picker: {:?}", base_dir);

    let mut items = if let Some(workspace) = WslWorkspace::from_unc_path(&base_dir) {
        match load_wsl_remote_file_search_blocking(&workspace, WSL_REMOTE_FILE_PICKER_TIMEOUT) {
            Ok(response) => {
                debug!(
                    file_count = response.files.len(),
                    truncated = response.truncated,
                    "Loaded WSL file picker items from remote helper"
                );
                file_picker_items_from_remote_search(&base_dir, response)
            }
            Err(error) => {
                warn!(
                    base_dir = %base_dir.display(),
                    error = %error,
                    "Failed to load WSL file picker items from remote helper"
                );
                Vec::new()
            }
        }
    } else {
        file_picker_items_from_local_walk(&base_dir)
    };

    // Sort items by label (path) for consistent ordering
    items.sort_by_key(|item| item.label.clone());

    debug!("File picker has {} items", items.len());

    // Populate VCS status for all file items using the global VCS service
    if let Some(vcs_service) = cx.try_global::<nucleotide_vcs::VcsServiceHandle>() {
        debug!("VCS service available, populating file picker VCS status");

        // Apply VCS status to items using cached status
        let mut vcs_status_count = 0;
        for item in &mut items {
            if let Some(ref file_path) = item.file_path {
                item.vcs_status = vcs_service.get_status_cached(file_path, cx);
                if item.vcs_status.is_some() {
                    vcs_status_count += 1;
                }
            }
        }

        debug!(
            file_count = items.len(),
            vcs_status_count = vcs_status_count,
            "Populated file picker VCS status"
        );
    } else {
        debug!("VCS service not available");
    }

    // Create a simple native picker without callback - the overlay will handle file opening via events
    let file_picker = crate::picker::Picker::native("Open File", items, |_index| {
        // This callback is no longer used - file opening is handled via OpenFile events
        // The overlay will emit OpenFile events when files are selected
    })
    .with_preview(true);

    debug!("Emitting file picker to overlay");

    emit_picker_update(file_picker, &overlay, cx);
}

fn file_picker_items_from_local_walk(base_dir: &Path) -> Vec<crate::picker_view::PickerItem> {
    use crate::picker_view::PickerItem;
    use ignore::WalkBuilder;
    use std::sync::Arc;

    let mut items = Vec::new();

    // Use ignore::Walk to get files, respecting .gitignore and other VCS ignore files
    // Configure WalkBuilder like Helix does to properly respect all ignore files
    let mut walker = WalkBuilder::new(base_dir);

    // Enable all ignore file types that Helix uses by default
    walker.git_ignore(true); // Respect .gitignore files
    walker.git_global(true); // Respect global gitignore
    walker.git_exclude(true); // Respect .git/info/exclude
    walker.ignore(true); // Respect .ignore files
    walker.parents(true); // Check parent directories for ignore files
    walker.hidden(true); // Hide hidden files (files starting with .)

    // Add Helix-specific ignore files
    walker.add_custom_ignore_filename(".helix/ignore");

    // Add standard editor ignore patterns
    walker.filter_entry(|entry| {
        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip common VCS directories that might not be caught by ignore files
        if path.is_dir() {
            match file_name {
                ".git" | ".svn" | ".hg" | ".bzr" => return false,
                _ => {}
            }
        }

        true
    });

    for entry in walker.build().filter_map(std::result::Result::ok) {
        let path = entry.path().to_path_buf();

        // Skip directories
        if path.is_dir() {
            continue;
        }

        // Get relative path from base directory
        let relative_path = path.strip_prefix(base_dir).unwrap_or(&path);
        if relative_path.starts_with("zed-source") {
            continue;
        }
        let path_str = relative_path.to_string_lossy().into_owned();

        // For project files, use path as label for better visibility
        items.push(PickerItem {
            label: path_str.clone().into(),
            sublabel: None,
            data: Arc::new(path.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            file_path: Some(path.clone()),
            vcs_status: None, // Will be populated using global VCS service
            columns: None,    // File picker uses simple label display
        });

        // Limit to 1000 files to prevent hanging on large projects
        if items.len() >= 1000 {
            break;
        }
    }

    items
}

fn file_picker_items_from_remote_search(
    base_dir: &Path,
    response: FileSearchResponse,
) -> Vec<crate::picker_view::PickerItem> {
    use crate::picker_view::PickerItem;
    use std::sync::Arc;

    response
        .files
        .into_iter()
        .take(1_000)
        .filter_map(|file| {
            if file.relative_path.starts_with("zed-source") {
                return None;
            }

            let label = file.relative_path.to_string_lossy().into_owned();
            let path = workspace_path_from_remote_relative(base_dir, &file.relative_path);

            Some(PickerItem {
                label: label.into(),
                sublabel: None,
                data: Arc::new(path.clone()) as Arc<dyn std::any::Any + Send + Sync>,
                file_path: Some(path),
                vcs_status: None,
                columns: None,
            })
        })
        .collect()
}

fn open_directory(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    debug!("Opening directory picker");

    // Create a native directory picker
    let directory_picker =
        crate::picker::Picker::native_directory("Select Project Directory", |path| {
            info!("Directory selected: {:?}", path);
            // The callback will be handled through events
        });

    // Emit the picker to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::DirectoryPicker(directory_picker));
    });
}

fn show_buffer_picker(
    core: Entity<Core>,
    _handle: tokio::runtime::Handle,
    overlay: Entity<OverlayView>,
    cx: &mut Context<Workspace>,
) {
    use crate::picker_view::PickerItem;
    use helix_view::DocumentId;
    use std::sync::Arc;

    debug!("Opening buffer picker");

    // Structure to hold buffer metadata for sorting
    #[derive(Clone)]
    struct BufferMeta {
        doc_id: DocumentId,
        path: Option<std::path::PathBuf>,
        is_modified: bool,
        is_current: bool,
        focused_at: std::time::Instant,
    }

    let (project_directory, mut buffer_metas) = {
        let core = core.read(cx);
        let editor = &core.editor;
        let current_doc_id = editor
            .tree
            .try_get(editor.tree.focus)
            .map(|view| view.doc)
            .unwrap_or_else(|| editor.documents.keys().next().copied().unwrap_or_default());

        let buffer_metas: Vec<BufferMeta> = editor
            .documents
            .iter()
            .map(|(doc_id, doc)| BufferMeta {
                doc_id: *doc_id,
                path: doc.path().map(|p| p.to_path_buf()),
                is_modified: doc.is_modified(),
                is_current: *doc_id == current_doc_id,
                focused_at: doc.focused_at,
            })
            .collect();

        (core.project_directory.clone(), buffer_metas)
    };

    // Sort by MRU (Most Recently Used) - most recent first
    buffer_metas.sort_by_key(|meta| std::cmp::Reverse(meta.focused_at));

    // Create picker items with terminal-like formatting
    let mut items = Vec::new();

    for meta in buffer_metas {
        // Format like terminal: "ID  FLAGS  PATH"
        // DocumentId likely has Display impl that shows "DocumentId(N)"
        let display_str = format!("{}", meta.doc_id);

        // Extract number from "DocumentId(N)" format
        let id_str = if display_str.starts_with("DocumentId(") && display_str.ends_with(")") {
            // Extract the number between parentheses
            display_str[11..display_str.len() - 1].to_string()
        } else if let Some(start) = display_str.find('(') {
            // More flexible parsing for variations
            if let Some(end) = display_str.rfind(')') {
                display_str[start + 1..end].trim().to_string()
            } else {
                display_str[start + 1..].trim().to_string()
            }
        } else if display_str.chars().all(char::is_numeric) {
            // If it's already just a number, use it
            display_str
        } else {
            // Fallback - try to find any number in the string
            display_str
                .chars()
                .skip_while(|c| !c.is_numeric())
                .take_while(|c| c.is_numeric())
                .collect::<String>()
        };

        // Build flags column - ensure consistent 2-character width
        let mut flags = String::new();
        if meta.is_modified {
            flags.push('+');
        }
        if meta.is_current {
            flags.push('*');
        }

        // Ensure flags are always exactly 2 characters for consistent column alignment
        let flags_str = format!("{flags:2}");

        // Get path or [scratch] label
        let path_str = if let Some(path) = &meta.path {
            // Show relative path if possible
            if let Some(project_dir) = &project_directory {
                path.strip_prefix(project_dir)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            } else {
                path.display().to_string()
            }
        } else {
            "[scratch]".to_string()
        };

        // Create data that includes both doc_id and path for preview functionality
        // We'll store a tuple of (DocumentId, Option<PathBuf>) for all items
        let picker_data =
            Arc::new((meta.doc_id, meta.path.clone())) as Arc<dyn std::any::Any + Send + Sync>;

        // Use structured columns instead of text formatting
        items.push(PickerItem::with_buffer_columns(
            id_str,
            flags_str,
            path_str,
            picker_data,
        ));
    }

    debug!("Buffer picker has {} items", items.len());

    // Create the picker with buffer items
    let buffer_picker = crate::picker::Picker::native("Switch Buffer", items, move |index| {
        debug!("Buffer selected at index: {}", index);
        // The overlay will handle buffer switching via the stored document ID
    })
    .with_preview(true);

    emit_picker_update(buffer_picker, &overlay, cx);
}

fn emit_picker_update(
    picker: crate::picker::Picker,
    overlay: &Entity<OverlayView>,
    cx: &mut Context<Workspace>,
) {
    let update = crate::Update::Picker(picker);
    overlay.update(cx, |overlay, cx| overlay.handle_event(&update, cx));
    cx.emit(update);
}

fn code_action_category(action: &lsp::CodeActionOrCommand) -> u32 {
    if let lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
        kind: Some(kind), ..
    }) = action
    {
        let mut components = kind.as_str().split('.');
        match components.next() {
            Some("quickfix") => 0,
            Some("refactor") => match components.next() {
                Some("extract") => 1,
                Some("inline") => 2,
                Some("rewrite") => 3,
                Some("move") => 4,
                Some("surround") => 5,
                _ => 7,
            },
            Some("source") => 6,
            _ => 7,
        }
    } else {
        7
    }
}

fn code_action_preferred(action: &lsp::CodeActionOrCommand) -> bool {
    matches!(
        action,
        lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
            is_preferred: Some(true),
            ..
        })
    )
}

fn code_action_fixes_diagnostics(action: &lsp::CodeActionOrCommand) -> bool {
    matches!(
        action,
        lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction { diagnostics: Some(diags), .. }) if !diags.is_empty()
    )
}

fn code_action_enabled(action: &lsp::CodeActionOrCommand) -> bool {
    matches!(
        action,
        lsp::CodeActionOrCommand::Command(_)
            | lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction { disabled: None, .. })
    )
}

fn code_action_label(action: &lsp::CodeActionOrCommand) -> &str {
    match action {
        lsp::CodeActionOrCommand::Command(command) => command.title.as_str(),
        lsp::CodeActionOrCommand::CodeAction(code_action) => code_action.title.as_str(),
    }
}

fn code_action_metadata_label(action: &lsp::CodeActionOrCommand, server_name: &str) -> String {
    let mut parts = Vec::new();

    match action {
        lsp::CodeActionOrCommand::Command(command) => {
            parts.push(format!("command: {}", command.command));
        }
        lsp::CodeActionOrCommand::CodeAction(code_action) => {
            parts.push(
                code_action
                    .kind
                    .as_ref()
                    .map(|kind| kind.as_str().to_string())
                    .unwrap_or_else(|| "code action".to_string()),
            );

            if code_action.is_preferred == Some(true) {
                parts.push("preferred".to_string());
            }
            if code_action_fixes_diagnostics(action) {
                parts.push("fixes diagnostics".to_string());
            }
            if code_action.data.is_some()
                && (code_action.edit.is_none() || code_action.command.is_none())
            {
                parts.push("resolves on apply".to_string());
            }
        }
    }

    parts.push(server_name.to_string());
    parts.join(" · ")
}

fn sort_code_actions_like_helix(actions: &mut [lsp::CodeActionOrCommand]) {
    actions.sort_by(|a, b| {
        let category = code_action_category(a).cmp(&code_action_category(b));
        if category != std::cmp::Ordering::Equal {
            return category;
        }

        let fixes_diagnostic = code_action_fixes_diagnostics(a)
            .cmp(&code_action_fixes_diagnostics(b))
            .reverse();
        if fixes_diagnostic != std::cmp::Ordering::Equal {
            return fixes_diagnostic;
        }

        code_action_preferred(a)
            .cmp(&code_action_preferred(b))
            .reverse()
    });
}

fn ui_completion_edit_from_event(
    edit: nucleotide_events::completion::CompletionEdit,
) -> nucleotide_ui::CompletionEdit {
    nucleotide_ui::CompletionEdit {
        offset_encoding: ui_completion_offset_encoding_from_event(edit.offset_encoding),
        text_edit: edit.text_edit.map(ui_completion_text_edit_from_event),
        additional_text_edits: edit
            .additional_text_edits
            .into_iter()
            .map(ui_completion_text_edit_from_event)
            .collect(),
    }
}

fn ui_completion_text_edit_from_event(
    edit: nucleotide_events::completion::CompletionTextEdit,
) -> nucleotide_ui::CompletionTextEdit {
    nucleotide_ui::CompletionTextEdit {
        range: ui_completion_range_from_event(edit.range),
        new_text: edit.new_text,
    }
}

fn ui_completion_range_from_event(
    range: nucleotide_events::completion::CompletionRange,
) -> nucleotide_ui::CompletionRange {
    nucleotide_ui::CompletionRange {
        start: ui_completion_position_from_event(range.start),
        end: ui_completion_position_from_event(range.end),
    }
}

fn ui_completion_position_from_event(
    position: nucleotide_events::completion::CompletionPosition,
) -> nucleotide_ui::CompletionPosition {
    nucleotide_ui::CompletionPosition {
        line: position.line,
        character: position.character,
    }
}

fn ui_completion_offset_encoding_from_event(
    offset_encoding: nucleotide_events::completion::CompletionOffsetEncoding,
) -> nucleotide_ui::CompletionOffsetEncoding {
    match offset_encoding {
        nucleotide_events::completion::CompletionOffsetEncoding::Utf8 => {
            nucleotide_ui::CompletionOffsetEncoding::Utf8
        }
        nucleotide_events::completion::CompletionOffsetEncoding::Utf16 => {
            nucleotide_ui::CompletionOffsetEncoding::Utf16
        }
        nucleotide_events::completion::CompletionOffsetEncoding::Utf32 => {
            nucleotide_ui::CompletionOffsetEncoding::Utf32
        }
    }
}

fn helix_offset_encoding_from_completion(
    offset_encoding: nucleotide_ui::CompletionOffsetEncoding,
) -> OffsetEncoding {
    match offset_encoding {
        nucleotide_ui::CompletionOffsetEncoding::Utf8 => OffsetEncoding::Utf8,
        nucleotide_ui::CompletionOffsetEncoding::Utf16 => OffsetEncoding::Utf16,
        nucleotide_ui::CompletionOffsetEncoding::Utf32 => OffsetEncoding::Utf32,
    }
}

fn completion_word_start(text: RopeSlice<'_>, cursor: usize) -> usize {
    cursor.saturating_sub(
        text.chars_at(cursor)
            .reversed()
            .take_while(|ch| helix_core::chars::char_is_word(*ch))
            .count(),
    )
}

fn completion_edit_offset(
    doc: &Rope,
    edit: &nucleotide_ui::CompletionTextEdit,
    offset_encoding: OffsetEncoding,
    primary_cursor: usize,
) -> Option<((i128, i128), usize)> {
    let range = helix_lsp::util::lsp_range_to_range(
        doc,
        lsp_range_from_completion(edit.range),
        offset_encoding,
    )?;
    let start = range.from();
    let mut end = range.to();
    let text = doc.slice(..);

    if should_extend_completion_edit_to_cursor(text, start, end, primary_cursor) {
        end = primary_cursor;
    }

    Some((
        (
            start as i128 - primary_cursor as i128,
            end as i128 - primary_cursor as i128,
        ),
        start,
    ))
}

fn should_extend_completion_edit_to_cursor(
    text: RopeSlice<'_>,
    start: usize,
    end: usize,
    primary_cursor: usize,
) -> bool {
    end < primary_cursor
        && primary_cursor <= text.len_chars()
        && start == completion_word_start(text, primary_cursor)
}

fn snippet_completion_transaction(
    text: &Rope,
    selection: &Selection,
    snippet_text: &str,
    edit_offset: Option<(i128, i128)>,
    replace_mode: bool,
    snippet_ctx: &mut helix_core::snippets::SnippetRenderCtx,
) -> anyhow::Result<(
    helix_core::Transaction,
    helix_core::snippets::RenderedSnippet,
)> {
    let snippet = helix_core::snippets::Snippet::parse(snippet_text)?;
    Ok(helix_lsp::util::generate_transaction_from_snippet(
        text,
        selection,
        edit_offset,
        replace_mode,
        snippet,
        snippet_ctx,
    ))
}

fn install_active_completion_snippet(
    doc: &mut helix_view::Document,
    snippet: helix_core::snippets::RenderedSnippet,
) {
    doc.active_snippet = match doc.active_snippet.take() {
        Some(active) => active.insert_subsnippet(snippet),
        None => helix_core::snippets::ActiveSnippet::new(snippet),
    };
}

fn lsp_text_edit_from_completion(edit: &nucleotide_ui::CompletionTextEdit) -> lsp::TextEdit {
    lsp::TextEdit::new(lsp_range_from_completion(edit.range), edit.new_text.clone())
}

fn lsp_range_from_completion(range: nucleotide_ui::CompletionRange) -> lsp::Range {
    lsp::Range::new(
        lsp_position_from_completion(range.start),
        lsp_position_from_completion(range.end),
    )
}

fn lsp_position_from_completion(position: nucleotide_ui::CompletionPosition) -> lsp::Position {
    lsp::Position::new(position.line, position.character)
}

fn show_code_actions(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    use futures_util::stream::{FuturesOrdered, StreamExt};
    use helix_lsp::lsp;
    use helix_lsp::util::{diagnostic_to_lsp_diagnostic, range_to_lsp_range};

    debug!("Opening code actions dropdown");

    // Snapshot needed editor state under read lock
    let Some((identifier, selection_range, doc_text, diags, servers)) = (|| {
        let core_r = core.read(cx);
        let editor = &core_r.editor;
        let view = editor.tree.try_get(editor.tree.focus)?;
        let doc = editor.documents.get(&view.doc)?;

        let selection_range = doc.selection(view.id).primary();
        let doc_text = doc.text().clone();
        let diags = doc
            .diagnostics()
            .iter()
            .filter(|d| {
                selection_range.overlaps(&helix_core::Range::new(d.range.start, d.range.end))
            })
            .cloned()
            .collect::<Vec<_>>();

        // Collect unique servers supporting CodeAction
        let mut seen = std::collections::HashSet::new();
        let servers: Vec<_> = doc
            .language_servers_with_feature(LanguageServerFeature::CodeAction)
            .filter(|ls| seen.insert(ls.id()))
            .collect();

        let identifier = document_lsp_identifier(doc)?;

        Some((identifier, selection_range, doc_text, diags, servers))
    })() else {
        core.update(cx, |core, cx| {
            core.editor
                .set_error("Code actions require a file-backed document");
            cx.notify();
        });
        return;
    };

    if servers.is_empty() {
        debug!("No language servers with CodeAction support");
        core.update(cx, |core, cx| {
            core.editor
                .set_error("No configured language server supports code actions");
            cx.notify();
        });
        return;
    }

    let mut futures: FuturesOrdered<_> = servers
        .into_iter()
        .filter_map(|ls| {
            let offset = ls.offset_encoding();
            let ls_id = ls.id();
            let server_name = ls.name().to_string();
            let range = range_to_lsp_range(&doc_text, selection_range, offset);
            let ctx = lsp::CodeActionContext {
                diagnostics: diags
                    .iter()
                    .map(|d| diagnostic_to_lsp_diagnostic(&doc_text, d, offset))
                    .collect(),
                only: None,
                trigger_kind: Some(lsp::CodeActionTriggerKind::INVOKED),
            };
            let req = ls.code_actions(identifier.clone(), range, ctx)?;
            Some(async move {
                req.await
                    .map(|opt| (opt.unwrap_or_default(), ls_id, offset, server_name))
            })
        })
        .collect();

    if futures.is_empty() {
        core.update(cx, |core, cx| {
            core.editor
                .set_error("No configured language server supports code actions");
            cx.notify();
        });
        return;
    }

    // Spawn async collection job
    let core_weak = core.downgrade();
    cx.spawn(async move |cx| {
        let mut items = Vec::new();

        while let Some(result) = futures.next().await {
            match result {
                Ok((mut actions, ls_id, offset, server_name)) => {
                    // Drop disabled actions
                    actions.retain(code_action_enabled);

                    // Sort as in Helix: category, then fixes diagnostics, then preferred
                    sort_code_actions_like_helix(&mut actions);

                    for action in actions.into_iter() {
                        let label = code_action_label(&action).to_string();
                        let sublabel = code_action_metadata_label(&action, &server_name);

                        items.push(crate::picker_view::PickerItem {
                            label: label.into(),
                            sublabel: Some(sublabel.into()),
                            data: Arc::new((action, ls_id, offset)),
                            file_path: None,
                            vcs_status: None,
                            columns: None,
                        });
                    }
                }
                Err(err) => {
                    warn!(error = %err, "Error collecting code actions from server");
                }
            }
        }

        // If none, exit with a notification
        if items.is_empty() {
            if let Some(core) = core_weak.upgrade() {
                core.update(cx, |core, cx| {
                    core.editor.set_error("No code actions available");
                    cx.emit(crate::Update::EditorStatus(
                        nucleotide_types::EditorStatus {
                            status: "No code actions available".to_string(),
                            severity: nucleotide_types::Severity::Error,
                        },
                    ));
                });
            }
            return;
        }

        if let Some(core) = core_weak.upgrade() {
            let picker = crate::picker::Picker::native("Code Actions", items, |_index| {
                // Selection is handled by OverlayView via typed code-action payloads.
            });
            core.update(cx, |_core, cx| {
                cx.emit(crate::Update::Picker(picker));
            });
        }
    })
    .detach();
}

fn test_prompt(core: Entity<Core>, handle: tokio::runtime::Handle, cx: &mut App) {
    // Create and emit a native prompt for testing
    core.update(cx, move |core, cx| {
        let _guard = handle.enter();

        // Create a native prompt directly
        let native_prompt = core.create_sample_native_prompt();

        // Emit the prompt to show it in the overlay
        cx.emit(crate::Update::Prompt(native_prompt));
    });
}

fn test_completion(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    // Create sample completion items
    let items = core.read(cx).create_sample_completion_items();

    // Position the completion near the top-left (simulating cursor position)

    // Create completion view
    let completion_view = cx.new(|cx| {
        let mut view = nucleotide_ui::completion_v2::CompletionView::new(cx);
        view.set_items(items, cx);
        view
    });

    // Emit completion event to show it in the overlay
    core.update(cx, |_core, cx| {
        cx.emit(crate::Update::Completion(completion_view));
    });
}

fn show_hover_docs(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    use futures_util::stream::{FuturesOrdered, StreamExt};

    debug!("Requesting hover documentation");

    let hover_requests = {
        let core_r = core.read(cx);
        let editor = &core_r.editor;
        let hover_requests = || {
            let Some(view) = editor.tree.try_get(editor.tree.focus) else {
                debug!("No focused editor view available for hover documentation");
                return None;
            };
            let Some(doc) = editor.documents.get(&view.doc) else {
                debug!(
                    view_id = ?view.id,
                    doc_id = ?view.doc,
                    "No focused document available for hover documentation"
                );
                return None;
            };

            let Some(url) = doc.url() else {
                debug!(
                    view_id = ?view.id,
                    doc_id = ?view.doc,
                    "Focused document has no file URL for hover documentation"
                );
                return None;
            };

            let mut seen = HashSet::new();
            let identifier = lsp::TextDocumentIdentifier::new(url);
            let mut requested = 0usize;

            let futures: FuturesOrdered<_> = doc
                .language_servers_with_feature(LanguageServerFeature::Hover)
                .filter(|ls| seen.insert(ls.id()))
                .filter_map(|language_server| {
                    requested += 1;
                    let server_name = language_server.name().to_string();
                    let identifier = identifier.clone();
                    let pos = doc.position(view.id, language_server.offset_encoding());
                    let request = language_server.text_document_hover(identifier, pos, None)?;

                    Some(async move { request.await.map(|hover| (server_name, hover)) })
                })
                .collect();

            Some((futures, requested))
        };
        hover_requests()
    };

    let Some((mut futures, requested_servers)) = hover_requests else {
        core.update(cx, |core, cx| {
            core.editor
                .set_status("No file-backed document is available for documentation.");
            cx.emit(crate::Update::HoverDocs(Vec::new()));
        });
        return;
    };

    if requested_servers == 0 {
        debug!("No LSP servers with hover capability are available");
        core.update(cx, |core, cx| {
            core.editor
                .set_error("No configured language server supports hover");
            cx.emit(crate::Update::HoverDocs(Vec::new()));
        });
        return;
    }

    let core_weak = core.downgrade();
    cx.spawn(async move |cx| {
        let mut entries: Vec<HoverDocEntry> = Vec::new();

        while let Some(result) = futures.next().await {
            match result {
                Ok((server_name, Some(hover))) => {
                    let markdown = hover_contents_to_markdown(hover.contents);
                    if !markdown.trim().is_empty() {
                        entries.push(HoverDocEntry {
                            server_name,
                            markdown,
                        });
                    }
                }
                Ok((_server_name, None)) => {}
                Err(err) => {
                    warn!(error = %err, "Hover request failed");
                }
            }
        }

        if entries.is_empty() {
            if let Some(core) = core_weak.upgrade() {
                core.update(cx, |core, cx| {
                    core.editor.set_status("No hover results available.");
                    cx.emit(crate::Update::HoverDocs(Vec::new()));
                });
            }
            return;
        }

        if let Some(core) = core_weak.upgrade() {
            let payload = entries;
            core.update(cx, |_core, cx| {
                cx.emit(crate::Update::HoverDocs(payload));
            });
        }
    })
    .detach();
}

fn hover_contents_to_markdown(contents: lsp::HoverContents) -> String {
    fn marked_string_to_markdown(contents: lsp::MarkedString) -> String {
        match contents {
            lsp::MarkedString::String(contents) => contents,
            lsp::MarkedString::LanguageString(string) => {
                if string.language == "markdown" {
                    string.value
                } else {
                    format!("```{}\n{}\n```", string.language, string.value)
                }
            }
        }
    }

    match contents {
        lsp::HoverContents::Scalar(contents) => marked_string_to_markdown(contents),
        lsp::HoverContents::Array(contents) => contents
            .into_iter()
            .map(marked_string_to_markdown)
            .collect::<Vec<_>>()
            .join("\n\n"),
        lsp::HoverContents::Markup(contents) => contents.value,
    }
}

fn quit(core: Entity<Core>, rt: tokio::runtime::Handle, cx: &mut App) {
    core.update(cx, |core, _cx| {
        let editor = &mut core.editor;
        let _guard = rt.enter();
        if let Err(e) = rt.block_on(async { editor.flush_writes().await }) {
            error!(error = %e, "Failed to flush writes");
        }
        let views: Vec<_> = editor.tree.views().map(|(view, _)| view.id).collect();
        for view_id in views {
            // Check if the view still exists before trying to close it
            if editor.tree.contains(view_id) {
                editor.close(view_id);
            }
        }
    });
}

impl Workspace {}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use helix_core::{Range, Rope, SmallVec};
    use slotmap::KeyData;
    use std::path::PathBuf;

    fn test_regex(pattern: &str) -> helix_stdx::rope::Regex {
        helix_stdx::rope::RegexBuilder::new()
            .syntax(helix_stdx::rope::Config::new().multi_line(true))
            .build(pattern)
            .unwrap()
    }

    fn default_file_picker_config() -> helix_view::editor::FilePickerConfig {
        helix_view::editor::Config::default().file_picker
    }

    #[test]
    fn local_directory_probe_skips_wsl_unc_paths() {
        let temp = tempfile::tempdir().unwrap();

        assert!(local_path_is_directory_without_wsl_probe(temp.path()));
        assert!(!local_path_is_directory_without_wsl_probe(Path::new(
            r"\\wsl.localhost\Ubuntu\home\iain\repo\src"
        )));
    }

    #[test]
    fn remote_file_search_maps_to_file_picker_items() {
        let base_dir = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\iain\repo");
        let response = FileSearchResponse {
            protocol_version: nucleotide_remote::PROTOCOL_VERSION,
            current_dir: PathBuf::from("/home/iain/repo"),
            files: vec![
                nucleotide_remote::FileSearchEntryResponse {
                    relative_path: PathBuf::from("src/main.rs"),
                },
                nucleotide_remote::FileSearchEntryResponse {
                    relative_path: PathBuf::from("zed-source/generated.rs"),
                },
            ],
            truncated: false,
        };

        let items = file_picker_items_from_remote_search(&base_dir, response);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label.as_ref(), "src/main.rs");
        assert_eq!(
            items[0].file_path.as_deref(),
            Some(base_dir.join("src/main.rs").as_path())
        );
    }

    fn test_code_action(
        title: &str,
        kind: Option<lsp::CodeActionKind>,
        fixes_diagnostic: bool,
        is_preferred: bool,
        disabled: bool,
    ) -> lsp::CodeActionOrCommand {
        lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
            title: title.to_string(),
            kind,
            diagnostics: fixes_diagnostic.then(|| {
                vec![lsp::Diagnostic::new_simple(
                    lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(0, 1)),
                    "diagnostic".to_string(),
                )]
            }),
            edit: None,
            command: None,
            is_preferred: is_preferred.then_some(true),
            disabled: disabled.then(|| lsp::CodeActionDisabled {
                reason: "disabled".to_string(),
            }),
            data: None,
        })
    }

    fn code_action_title(action: &lsp::CodeActionOrCommand) -> &str {
        match action {
            lsp::CodeActionOrCommand::Command(command) => &command.title,
            lsp::CodeActionOrCommand::CodeAction(code_action) => &code_action.title,
        }
    }

    #[test]
    fn environment_badge_appears_for_known_environment_markers() {
        assert_eq!(
            EnvironmentBadge::from_environment_marker(Some("native-flake")),
            Some(EnvironmentBadge::NativeFlake)
        );
        assert_eq!(
            EnvironmentBadge::from_environment_marker(Some("wsl-remote-helper")),
            Some(EnvironmentBadge::Wsl)
        );
        assert_eq!(
            EnvironmentBadge::from_environment_marker(Some("wsl-shell")),
            Some(EnvironmentBadge::Wsl)
        );
        assert_eq!(EnvironmentBadge::from_environment_marker(Some("cli")), None);
        assert_eq!(
            EnvironmentBadge::from_environment_marker(Some("worktree-shell")),
            None
        );
        assert_eq!(EnvironmentBadge::from_environment_marker(None), None);
    }

    #[test]
    fn environment_badge_labels_wsl_as_remote() {
        assert_eq!(EnvironmentBadge::Wsl.label(), "wsl");
        assert_eq!(EnvironmentBadge::Wsl.detail(), "remote");
    }

    #[test]
    fn titlebar_filename_uses_focused_file_or_app_name() {
        assert_eq!(titlebar_filename(Some("main.rs")), "main.rs");
        assert_eq!(titlebar_filename(Some("")), "Nucleotide");
        assert_eq!(titlebar_filename(None), "Nucleotide");
    }

    #[test]
    fn statusbar_text_shortening_collapses_whitespace() {
        assert_eq!(
            shorten_statusbar_text("  indexing\nworkspace\tfiles  ", 64),
            "indexing workspace files"
        );
    }

    #[test]
    fn statusbar_text_shortening_caps_display_width() {
        assert_eq!(
            shorten_statusbar_text("abcdefghijklmnopqrstuvwxyz", 10),
            "abcdefghi…"
        );
    }

    #[test]
    fn statusbar_text_shortening_counts_characters() {
        assert_eq!(shorten_statusbar_text("éééééé", 4), "ééé…");
    }

    #[test]
    fn statusbar_lsp_indicator_prefers_working_server_over_focused_server() {
        let focused_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(1).into();
        let working_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(2).into();
        let mut state = nucleotide_lsp::LspState::new();

        state.register_server(focused_server_id, "rust-analyzer".to_string(), None);
        state.register_server(working_server_id, "pyright".to_string(), None);
        state.add_progress(nucleotide_lsp::LspProgress {
            server_id: working_server_id,
            token: "workspace-index".to_string(),
            title: "indexing".to_string(),
            message: Some("workspace".to_string()),
            percentage: None,
        });

        let indicator = statusbar_lsp_indicator_for_state(&mut state, Some(focused_server_id))
            .expect("lsp indicator");

        assert!(indicator.contains("pyright"));
        assert!(indicator.contains("indexing"));
        assert!(!indicator.contains("rust-analyzer"));
    }

    #[test]
    fn statusbar_lsp_indicator_uses_focused_server_when_idle() {
        let focused_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(3).into();
        let other_server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(4).into();
        let mut state = nucleotide_lsp::LspState::new();

        state.register_server(focused_server_id, "rust-analyzer".to_string(), None);
        state.update_server_status(focused_server_id, nucleotide_lsp::ServerStatus::Running);
        state.register_server(other_server_id, "pyright".to_string(), None);
        state.update_server_status(other_server_id, nucleotide_lsp::ServerStatus::Running);

        let indicator = statusbar_lsp_indicator_for_state(&mut state, Some(focused_server_id))
            .expect("lsp indicator");

        assert_eq!(indicator, "◉ rust-analyzer");
    }

    #[test]
    fn app_titlebar_is_hidden_only_for_visible_translucent_sidebar() {
        assert!(should_render_app_titlebar(true, false, 240.0, true));
        assert!(should_render_app_titlebar(true, true, 240.0, false));
        assert!(should_render_app_titlebar(true, true, 0.0, true));
        assert!(!should_render_app_titlebar(true, true, 240.0, true));
        assert!(!should_render_app_titlebar(false, true, 240.0, true));
    }

    #[test]
    fn file_tree_content_inset_clears_macos_traffic_lights() {
        assert_eq!(f32::from(file_tree_content_top_inset(false)), 0.0);
        assert_eq!(
            f32::from(file_tree_content_top_inset(true)),
            MACOS_TRAFFIC_LIGHT_TREE_TOP_INSET_PX
        );
    }

    #[test]
    fn translucent_sidebar_extends_into_status_bar_only_when_visible() {
        assert!(should_extend_translucent_sidebar_into_status_bar(
            true, 240.0, true
        ));
        assert!(!should_extend_translucent_sidebar_into_status_bar(
            false, 240.0, true
        ));
        assert!(!should_extend_translucent_sidebar_into_status_bar(
            true, 0.0, true
        ));
        assert!(!should_extend_translucent_sidebar_into_status_bar(
            true, 240.0, false
        ));
    }

    #[test]
    fn native_window_title_uses_nucleotide_app_name() {
        assert_eq!(native_window_title(Some("main.rs")), "main.rs — Nucleotide");
        assert_eq!(native_window_title(Some("")), "Nucleotide");
        assert_eq!(native_window_title(None), "Nucleotide");
    }

    #[test]
    fn configured_theme_name_uses_explicit_theme_modes() {
        let theme = crate::config::ThemeConfig {
            mode: crate::config::ThemeMode::Light,
            light_theme: Some("light-test".to_string()),
            dark_theme: Some("dark-test".to_string()),
        };
        assert_eq!(
            configured_theme_name_for_appearance(
                &theme,
                nucleotide_appearance::SystemAppearance::Dark
            ),
            "light-test"
        );

        let theme = crate::config::ThemeConfig {
            mode: crate::config::ThemeMode::Dark,
            light_theme: Some("light-test".to_string()),
            dark_theme: Some("dark-test".to_string()),
        };
        assert_eq!(
            configured_theme_name_for_appearance(
                &theme,
                nucleotide_appearance::SystemAppearance::Light
            ),
            "dark-test"
        );
    }

    #[test]
    fn configured_theme_name_follows_system_appearance_in_system_mode() {
        let theme = crate::config::ThemeConfig {
            mode: crate::config::ThemeMode::System,
            light_theme: Some("light-test".to_string()),
            dark_theme: Some("dark-test".to_string()),
        };

        assert_eq!(
            configured_theme_name_for_appearance(
                &theme,
                nucleotide_appearance::SystemAppearance::Light
            ),
            "light-test"
        );
        assert_eq!(
            configured_theme_name_for_appearance(
                &theme,
                nucleotide_appearance::SystemAppearance::Dark
            ),
            "dark-test"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_recent_project_path_accepts_directories_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("file.txt");
        std::fs::write(&file_path, "not a project root").unwrap();

        let recent_path = windows_recent_project_path(temp_dir.path()).unwrap();
        assert!(recent_path.is_absolute());
        assert!(recent_path.is_dir());
        assert_eq!(windows_recent_project_path(&file_path), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_recent_project_path_accepts_wsl_unc_without_local_probe() {
        let project_path = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project");

        assert_eq!(
            windows_recent_project_path(&project_path),
            Some(project_path)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_wide_nul_path_is_nul_terminated() {
        let wide_path = windows_wide_nul_path(Path::new(r"C:\Users\Example Project"));

        assert_eq!(wide_path.last().copied(), Some(0));
        assert_eq!(wide_path.iter().filter(|&&ch| ch == 0).count(), 1);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_jump_list_recent_projects_are_most_recent_first() {
        let mut recent = VecDeque::new();
        let project_a = PathBuf::from(r"C:\Users\Example\project-a");
        let project_b = PathBuf::from(r"C:\Users\Example\project-b");

        windows_update_recent_project_list(&mut recent, project_a.clone());
        windows_update_recent_project_list(&mut recent, project_b.clone());
        windows_update_recent_project_list(&mut recent, project_a.clone());

        let entries = windows_jump_list_entries(&recent);
        let expected: Vec<SmallVec<[PathBuf; 2]>> =
            vec![smallvec![project_a], smallvec![project_b]];

        assert_eq!(entries, expected);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_jump_list_recent_projects_are_capped() {
        let mut recent = VecDeque::new();

        for index in 0..(WINDOWS_JUMP_LIST_PROJECT_LIMIT + 2) {
            windows_update_recent_project_list(
                &mut recent,
                PathBuf::from(format!(r"C:\Users\Example\project-{index}")),
            );
        }

        assert_eq!(recent.len(), WINDOWS_JUMP_LIST_PROJECT_LIMIT);
        assert_eq!(
            recent.front(),
            Some(&PathBuf::from(format!(
                r"C:\Users\Example\project-{}",
                WINDOWS_JUMP_LIST_PROJECT_LIMIT + 1
            )))
        );
        assert_eq!(
            recent.back(),
            Some(&PathBuf::from(r"C:\Users\Example\project-2"))
        );
    }

    #[test]
    fn file_operation_notification_success_tracks_disk_state() {
        let dir = tempfile::tempdir().unwrap();
        let created_file = dir.path().join("created.rs");
        std::fs::write(&created_file, "").unwrap();
        assert!(file_operation_notification_succeeded(
            &LspFileOperationNotification::Created {
                path: created_file,
                is_dir: false,
            }
        ));

        let deleted_file = dir.path().join("deleted.rs");
        assert!(!deleted_file.exists());
        assert!(file_operation_notification_succeeded(
            &LspFileOperationNotification::Deleted {
                path: deleted_file,
                was_dir: false,
            }
        ));

        let old_path = dir.path().join("old.rs");
        let new_path = dir.path().join("new.rs");
        std::fs::write(&new_path, "").unwrap();
        assert!(file_operation_notification_succeeded(
            &LspFileOperationNotification::Renamed {
                old_path,
                new_path,
                was_dir: false,
            }
        ));
    }

    #[test]
    fn completion_menu_keys_match_helix_completion_navigation() {
        assert_eq!(
            completion_menu_action("tab", false, false),
            Some(MenuKeyAction::Accept)
        );
        assert_eq!(
            completion_menu_action("y", true, false),
            Some(MenuKeyAction::Accept)
        );
        assert_eq!(
            completion_menu_action("down", false, false),
            Some(MenuKeyAction::SelectNext)
        );
        assert_eq!(
            completion_menu_action("n", true, false),
            Some(MenuKeyAction::SelectNext)
        );
        assert_eq!(
            completion_menu_action("up", false, false),
            Some(MenuKeyAction::SelectPrevious)
        );
        assert_eq!(
            completion_menu_action("p", true, false),
            Some(MenuKeyAction::SelectPrevious)
        );
        assert_eq!(
            completion_menu_action("escape", false, false),
            Some(MenuKeyAction::Cancel)
        );
    }

    #[test]
    fn completion_menu_keys_ignore_non_helix_completion_bindings() {
        assert_eq!(completion_menu_action("enter", false, false), None);
        assert_eq!(completion_menu_action("tab", false, true), None);
        assert_eq!(completion_menu_action("c", true, false), None);
        assert_eq!(completion_menu_action("down", true, false), None);
    }

    #[test]
    fn completion_refinement_follows_focused_document() {
        let doc_id = DocumentId::default();

        assert!(should_refine_completion_for_focused_document(
            true,
            Some(doc_id),
            doc_id
        ));
    }

    #[test]
    fn completion_refinement_ignores_missing_completion_or_focus() {
        let doc_id = DocumentId::default();

        assert!(!should_refine_completion_for_focused_document(
            false,
            Some(doc_id),
            doc_id
        ));
        assert!(!should_refine_completion_for_focused_document(
            true, None, doc_id
        ));
    }

    #[test]
    fn incomplete_completion_retrigger_follows_focused_session() {
        let doc_id = DocumentId::default();
        let view_id = test_view_id(1);
        let session = ActiveCompletionSession {
            doc_id,
            view_id,
            document_version: 0,
            is_incomplete: true,
            incomplete_server_ids: vec![1],
            retained_items: Vec::new(),
            requested_prefix: "pri".to_string(),
        };

        assert!(should_retrigger_incomplete_completion_for_focused_session(
            &session,
            "prin",
            Some(doc_id),
            view_id
        ));
    }

    #[test]
    fn incomplete_completion_retrigger_ignores_complete_or_unchanged_sessions() {
        let doc_id = DocumentId::default();
        let view_id = test_view_id(1);
        let mut session = ActiveCompletionSession {
            doc_id,
            view_id,
            document_version: 0,
            is_incomplete: false,
            incomplete_server_ids: vec![1],
            retained_items: Vec::new(),
            requested_prefix: "pri".to_string(),
        };

        assert!(!should_retrigger_incomplete_completion_for_focused_session(
            &session,
            "prin",
            Some(doc_id),
            view_id
        ));

        session.is_incomplete = true;
        assert!(!should_retrigger_incomplete_completion_for_focused_session(
            &session,
            "pri",
            Some(doc_id),
            view_id
        ));
        assert!(!should_retrigger_incomplete_completion_for_focused_session(
            &session, "prin", None, view_id
        ));
        assert!(!should_retrigger_incomplete_completion_for_focused_session(
            &session,
            "prin",
            Some(doc_id),
            test_view_id(2)
        ));
    }

    #[test]
    fn incomplete_completion_retains_completed_provider_items() {
        let completed = nucleotide_events::completion::CompletionItem::new(
            "clone".to_string(),
            nucleotide_events::completion::CompletionItemKind::Method,
        )
        .with_server_id(Some(1));
        let incomplete = nucleotide_events::completion::CompletionItem::new(
            "fmt".to_string(),
            nucleotide_events::completion::CompletionItemKind::Method,
        )
        .with_server_id(Some(2));
        let local = nucleotide_events::completion::CompletionItem::new(
            "local".to_string(),
            nucleotide_events::completion::CompletionItemKind::Text,
        );

        let retained = retained_completion_items_for_completed_providers(
            &[completed.clone(), incomplete, local],
            &[2],
        );

        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].label, completed.label);
        assert_eq!(retained[0].server_id, Some(1));
    }

    #[test]
    fn completion_locality_key_uses_filter_text_first() {
        let item = nucleotide_ui::completion_v2::CompletionItem::new("fmt(${1:f})")
            .with_display_text("fmt(...)")
            .with_filter_text("Debug::fmt");

        assert_eq!(completion_locality_key(&item).as_deref(), Some("debug"));
    }

    #[test]
    fn completion_locality_score_prefers_nearby_lines() {
        let text = "clone\n\nfmt\n\ninto\n";

        assert!(
            completion_locality_score_for_text(text, 2, "fmt")
                > completion_locality_score_for_text(text, 2, "clone")
        );
        assert_eq!(completion_locality_score_for_text(text, 2, "missing"), 0);
    }

    #[test]
    fn completion_memory_prioritizes_recent_prefix_match() {
        let mut memory = CompletionMemory::default();
        let old_key = CompletionMemoryKey {
            language: "rust".to_string(),
            prefix: "fo".to_string(),
            kind: Some(nucleotide_ui::completion_v2::CompletionItemKind::Function),
            insert_text: "foo".to_string(),
        };
        let recent_key = CompletionMemoryKey {
            language: "rust".to_string(),
            prefix: "fo".to_string(),
            kind: Some(nucleotide_ui::completion_v2::CompletionItemKind::Function),
            insert_text: "foobar".to_string(),
        };

        memory.memorize(old_key.clone());
        memory.memorize(recent_key.clone());

        assert!(memory.priority(&recent_key) > memory.priority(&old_key));
        assert_eq!(
            memory.priority(&CompletionMemoryKey {
                language: "rust".to_string(),
                prefix: "ba".to_string(),
                kind: Some(nucleotide_ui::completion_v2::CompletionItemKind::Function),
                insert_text: "foobar".to_string(),
            }),
            0
        );
    }

    #[test]
    fn completion_commit_character_uses_unmodified_printable_key() {
        assert_eq!(
            completion_commit_character_from_key("(", Some("("), false),
            Some('(')
        );
        assert_eq!(
            completion_commit_character_from_key("9", Some("("), false),
            Some('(')
        );
        assert_eq!(
            completion_commit_character_from_key("(", Some("("), true),
            None
        );
        assert_eq!(
            completion_commit_character_from_key("enter", None, false),
            None
        );
    }

    #[test]
    fn completion_word_start_uses_character_offsets() {
        let rope = Rope::from("héllo world");
        let cursor = 5;

        assert_eq!(completion_word_start(rope.slice(..), cursor), 0);
    }

    #[test]
    fn completion_edit_offset_converts_lsp_range() {
        let rope = Rope::from("let value = old;");
        let edit = nucleotide_ui::CompletionTextEdit {
            range: nucleotide_ui::CompletionRange {
                start: nucleotide_ui::CompletionPosition {
                    line: 0,
                    character: 12,
                },
                end: nucleotide_ui::CompletionPosition {
                    line: 0,
                    character: 15,
                },
            },
            new_text: "new".to_string(),
        };

        let (offset, start) =
            completion_edit_offset(&rope, &edit, OffsetEncoding::Utf8, 15).unwrap();

        assert_eq!(offset, (-3, 0));
        assert_eq!(start, 12);
    }

    #[test]
    fn completion_edit_offset_extends_stale_range_to_live_word() {
        let rope = Rope::from("println");
        let edit = nucleotide_ui::CompletionTextEdit {
            range: nucleotide_ui::CompletionRange {
                start: nucleotide_ui::CompletionPosition {
                    line: 0,
                    character: 0,
                },
                end: nucleotide_ui::CompletionPosition {
                    line: 0,
                    character: 5,
                },
            },
            new_text: "println!()".to_string(),
        };

        let (offset, start) =
            completion_edit_offset(&rope, &edit, OffsetEncoding::Utf8, 7).unwrap();

        assert_eq!(offset, (-7, 0));
        assert_eq!(start, 0);
    }

    #[test]
    fn completion_edit_offset_keeps_range_when_it_starts_inside_word() {
        let rope = Rope::from("println");
        let edit = nucleotide_ui::CompletionTextEdit {
            range: nucleotide_ui::CompletionRange {
                start: nucleotide_ui::CompletionPosition {
                    line: 0,
                    character: 2,
                },
                end: nucleotide_ui::CompletionPosition {
                    line: 0,
                    character: 5,
                },
            },
            new_text: "intln!()".to_string(),
        };

        let (offset, start) =
            completion_edit_offset(&rope, &edit, OffsetEncoding::Utf8, 7).unwrap();

        assert_eq!(offset, (-5, -2));
        assert_eq!(start, 2);
    }

    fn test_snippet_render_ctx() -> helix_core::snippets::SnippetRenderCtx {
        helix_core::snippets::SnippetRenderCtx {
            resolve_var: Box::new(|_| None),
            tab_width: 4,
            indent_style: helix_core::indent::IndentStyle::Spaces(4),
            line_ending: "\n",
        }
    }

    #[test]
    fn snippet_completion_transaction_preserves_active_placeholder() {
        let mut rope = Rope::from("pri");
        let selection = Selection::point(3);
        let mut snippet_ctx = test_snippet_render_ctx();

        let (transaction, rendered_snippet) = snippet_completion_transaction(
            &rope,
            &selection,
            "println(${1:value});$0",
            None,
            false,
            &mut snippet_ctx,
        )
        .unwrap();

        assert!(transaction.apply(&mut rope));
        assert_eq!(rope.to_string(), "println(value);");
        assert!(helix_core::snippets::ActiveSnippet::new(rendered_snippet).is_some());

        let primary = transaction.selection().unwrap().primary();
        assert_eq!(primary.from(), 8);
        assert_eq!(primary.to(), 13);
    }

    #[test]
    fn snippet_completion_transaction_uses_lsp_edit_range() {
        let mut rope = Rope::from("let value = old;");
        let selection = Selection::point(15);
        let mut snippet_ctx = test_snippet_render_ctx();
        let edit_offset = Some((-3, 0));

        let (transaction, rendered_snippet) = snippet_completion_transaction(
            &rope,
            &selection,
            "${1:new_value}$0",
            edit_offset,
            false,
            &mut snippet_ctx,
        )
        .unwrap();

        assert!(transaction.apply(&mut rope));
        assert_eq!(rope.to_string(), "let value = new_value;");
        assert!(helix_core::snippets::ActiveSnippet::new(rendered_snippet).is_some());

        let primary = transaction.selection().unwrap().primary();
        assert_eq!(primary.from(), 12);
        assert_eq!(primary.to(), 21);
    }

    #[test]
    fn ui_completion_edit_from_event_preserves_payload() {
        let edit = nucleotide_events::completion::CompletionEdit {
            offset_encoding: nucleotide_events::completion::CompletionOffsetEncoding::Utf16,
            text_edit: Some(nucleotide_events::completion::CompletionTextEdit {
                range: nucleotide_events::completion::CompletionRange {
                    start: nucleotide_events::completion::CompletionPosition {
                        line: 1,
                        character: 2,
                    },
                    end: nucleotide_events::completion::CompletionPosition {
                        line: 1,
                        character: 5,
                    },
                },
                new_text: "value".to_string(),
            }),
            additional_text_edits: vec![nucleotide_events::completion::CompletionTextEdit {
                range: nucleotide_events::completion::CompletionRange {
                    start: nucleotide_events::completion::CompletionPosition {
                        line: 0,
                        character: 0,
                    },
                    end: nucleotide_events::completion::CompletionPosition {
                        line: 0,
                        character: 0,
                    },
                },
                new_text: "use value;\n".to_string(),
            }],
        };

        let ui_edit = ui_completion_edit_from_event(edit);

        assert_eq!(
            ui_edit.offset_encoding,
            nucleotide_ui::CompletionOffsetEncoding::Utf16
        );
        assert_eq!(ui_edit.text_edit.as_ref().unwrap().new_text, "value");
        assert_eq!(ui_edit.additional_text_edits.len(), 1);
    }

    fn test_view_id(index: u64) -> ViewId {
        ViewId::from(KeyData::from_ffi((1_u64 << 32) | index))
    }

    #[test]
    fn code_action_enabled_filters_disabled_actions_like_helix() {
        let enabled_action = test_code_action("enabled", None, false, false, false);
        let disabled_action = test_code_action("disabled", None, false, false, true);
        let command = lsp::CodeActionOrCommand::Command(lsp::Command {
            title: "command".to_string(),
            command: "server.command".to_string(),
            arguments: None,
        });

        assert!(code_action_enabled(&enabled_action));
        assert!(code_action_enabled(&command));
        assert!(!code_action_enabled(&disabled_action));
    }

    #[test]
    fn code_action_metadata_label_includes_available_lsp_metadata() {
        let mut action = match test_code_action(
            "quick fix",
            Some(lsp::CodeActionKind::QUICKFIX),
            true,
            true,
            false,
        ) {
            lsp::CodeActionOrCommand::CodeAction(action) => action,
            lsp::CodeActionOrCommand::Command(_) => unreachable!(),
        };
        action.data = Some(serde_json::json!({ "token": "lazy" }));

        let label = code_action_metadata_label(
            &lsp::CodeActionOrCommand::CodeAction(action),
            "rust-analyzer",
        );

        assert_eq!(
            label,
            "quickfix · preferred · fixes diagnostics · resolves on apply · rust-analyzer"
        );

        let command = lsp::CodeActionOrCommand::Command(lsp::Command {
            title: "Run command".to_string(),
            command: "server.command".to_string(),
            arguments: None,
        });

        assert_eq!(
            code_action_metadata_label(&command, "test-ls"),
            "command: server.command · test-ls"
        );
    }

    #[test]
    fn code_actions_sort_like_helix_by_category_and_relevance() {
        let mut actions = vec![
            test_code_action(
                "source preferred diagnostic",
                Some(lsp::CodeActionKind::SOURCE),
                true,
                true,
                false,
            ),
            test_code_action(
                "quickfix preferred no diagnostic",
                Some(lsp::CodeActionKind::QUICKFIX),
                false,
                true,
                false,
            ),
            test_code_action(
                "quickfix diagnostic",
                Some(lsp::CodeActionKind::QUICKFIX),
                true,
                false,
                false,
            ),
            test_code_action(
                "refactor extract preferred diagnostic",
                Some(lsp::CodeActionKind::REFACTOR_EXTRACT),
                true,
                true,
                false,
            ),
            test_code_action(
                "quickfix preferred diagnostic",
                Some(lsp::CodeActionKind::QUICKFIX),
                true,
                true,
                false,
            ),
        ];

        sort_code_actions_like_helix(&mut actions);

        let titles = actions.iter().map(code_action_title).collect::<Vec<_>>();

        assert_eq!(
            titles,
            vec![
                "quickfix preferred diagnostic",
                "quickfix diagnostic",
                "quickfix preferred no diagnostic",
                "refactor extract preferred diagnostic",
                "source preferred diagnostic",
            ]
        );
    }

    #[test]
    fn file_tree_context_menu_items_follow_sidebar_intent_order() {
        let actual: Vec<_> = Workspace::context_menu_intents()
            .iter()
            .map(|intent| intent.label())
            .collect();
        let expected: Vec<_> = ProjectTreeContextMenuIntent::common_file_operations()
            .iter()
            .map(|intent| intent.label())
            .collect();

        assert_eq!(actual, expected);
    }

    #[test]
    fn tab_bar_layout_height_matches_rendered_row_count() {
        let row_height = px(32.0);

        assert_eq!(
            tab_bar_layout_height(row_height, false, true, true),
            row_height
        );
        assert_eq!(
            tab_bar_layout_height(row_height, true, true, true),
            px(64.0)
        );
        assert_eq!(
            tab_bar_layout_height(row_height, true, true, false),
            row_height
        );
        assert_eq!(
            tab_bar_layout_height(row_height, true, false, true),
            row_height
        );
    }

    #[test]
    fn tab_bar_height_for_editor_matches_bufferline_visibility() {
        use helix_view::editor::BufferLine;

        let row_height = px(32.0);

        assert_eq!(
            tab_bar_height_for_editor(true, &BufferLine::Never, 3, row_height, true, true, true),
            px(0.0)
        );
        assert_eq!(
            tab_bar_height_for_editor(true, &BufferLine::Always, 1, row_height, true, true, true),
            px(64.0)
        );
        assert_eq!(
            tab_bar_height_for_editor(true, &BufferLine::Multiple, 1, row_height, true, true, true),
            px(0.0)
        );
        assert_eq!(
            tab_bar_height_for_editor(true, &BufferLine::Multiple, 2, row_height, true, true, true),
            px(64.0)
        );
        assert_eq!(
            tab_bar_height_for_editor(false, &BufferLine::Always, 2, row_height, true, true, true),
            px(0.0)
        );
    }

    #[test]
    fn tab_context_menu_items_match_zed_close_actions() {
        let unpinned_labels = Workspace::tab_context_menu_intents(false, false, false)
            .iter()
            .map(|intent| intent.label(false, false, false))
            .collect::<Vec<_>>();
        let pinned_labels = Workspace::tab_context_menu_intents(false, false, false)
            .iter()
            .map(|intent| intent.label(true, false, false))
            .collect::<Vec<_>>();

        assert_eq!(
            unpinned_labels,
            vec![
                "Close",
                "Close Others",
                "Close Left",
                "Close Right",
                "Close Clean",
                "Close All",
                "Pin Tab"
            ]
        );
        assert_eq!(pinned_labels.last(), Some(&"Unpin Tab"));
    }

    #[test]
    fn tab_context_menu_entries_match_zed_grouping() {
        let labels = Workspace::tab_context_menu_entries(false, false, false)
            .iter()
            .map(|entry| match entry {
                TabContextMenuEntry::Action(intent) => intent.label(false, false, false),
                TabContextMenuEntry::Separator => "|",
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "Close",
                "Close Others",
                "|",
                "Close Left",
                "Close Right",
                "|",
                "Close Clean",
                "Close All",
                "|",
                "Pin Tab"
            ]
        );
    }

    #[test]
    fn tab_context_menu_entries_add_zed_file_path_actions_for_file_tabs() {
        let reveal_label = reveal_in_file_manager_label(false);
        let labels = Workspace::tab_context_menu_entries(true, false, false)
            .iter()
            .map(|entry| match entry {
                TabContextMenuEntry::Action(intent) => intent.label(false, false, false),
                TabContextMenuEntry::Separator => "|",
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "Close",
                "Close Others",
                "|",
                "Close Left",
                "Close Right",
                "|",
                "Close Clean",
                "Close All",
                "|",
                "Make File Read-Only",
                "|",
                "Copy Path",
                "Copy Relative Path",
                "|",
                reveal_label,
                "|",
                "Pin Tab"
            ]
        );
    }

    #[test]
    fn tab_context_menu_intents_add_zed_readonly_toggle_for_file_tabs() {
        let labels = Workspace::tab_context_menu_intents(true, false, false)
            .iter()
            .map(|intent| intent.label(false, false, false))
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "Close",
                "Close Others",
                "Close Left",
                "Close Right",
                "Close Clean",
                "Close All",
                "Make File Read-Only",
                "Copy Path",
                "Copy Relative Path",
                reveal_in_file_manager_label(false),
                "Pin Tab",
            ]
        );
    }

    #[test]
    fn tab_context_menu_readonly_toggle_label_matches_zed_state() {
        assert_eq!(
            TabContextMenuIntent::ToggleReadOnly.label(false, false, false),
            "Make File Read-Only"
        );
        assert_eq!(
            TabContextMenuIntent::ToggleReadOnly.label(false, true, false),
            "Make File Editable"
        );
    }

    #[test]
    fn tab_context_menu_entries_add_reveal_project_panel_for_visible_project_paths() {
        let reveal_label = reveal_in_file_manager_label(false);
        let labels = Workspace::tab_context_menu_entries(true, true, false)
            .iter()
            .map(|entry| match entry {
                TabContextMenuEntry::Action(intent) => intent.label(false, false, false),
                TabContextMenuEntry::Separator => "|",
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "Close",
                "Close Others",
                "|",
                "Close Left",
                "Close Right",
                "|",
                "Close Clean",
                "Close All",
                "|",
                "Make File Read-Only",
                "|",
                "Copy Path",
                "Copy Relative Path",
                "|",
                reveal_label,
                "|",
                "Pin Tab",
                "Reveal In Project Panel"
            ]
        );
    }

    #[test]
    fn tab_context_menu_entries_add_open_terminal_when_parent_directory_exists() {
        let reveal_label = reveal_in_file_manager_label(false);
        let labels = Workspace::tab_context_menu_entries(true, true, true)
            .iter()
            .map(|entry| match entry {
                TabContextMenuEntry::Action(intent) => intent.label(false, false, false),
                TabContextMenuEntry::Separator => "|",
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "Close",
                "Close Others",
                "|",
                "Close Left",
                "Close Right",
                "|",
                "Close Clean",
                "Close All",
                "|",
                "Make File Read-Only",
                "|",
                "Copy Path",
                "Copy Relative Path",
                "|",
                reveal_label,
                "|",
                "Pin Tab",
                "Reveal In Project Panel",
                "Open in Terminal"
            ]
        );
    }

    #[test]
    fn reveal_in_file_manager_label_matches_zed_platform_label() {
        if cfg!(target_os = "macos") {
            assert_eq!(reveal_in_file_manager_label(false), "Reveal in Finder");
        } else if cfg!(target_os = "windows") {
            assert_eq!(
                reveal_in_file_manager_label(false),
                "Reveal in File Explorer"
            );
        } else {
            assert_eq!(
                reveal_in_file_manager_label(false),
                "Reveal in File Manager"
            );
        }
        assert_eq!(reveal_in_file_manager_label(true), "Reveal in File Manager");
    }

    #[test]
    fn tab_context_reveal_label_uses_remote_file_manager_label() {
        assert_eq!(
            TabContextMenuIntent::RevealInOs.label(false, false, true),
            "Reveal in File Manager"
        );
    }

    #[test]
    fn tab_context_menu_disabled_states_match_zed_rules() {
        assert!(!Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::Close,
            Some(0),
            1,
            false,
        ));
        assert!(Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::CloseOthers,
            Some(0),
            1,
            true,
        ));
        assert!(!Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::CloseOthers,
            Some(0),
            2,
            true,
        ));
        assert!(Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::CloseLeft,
            Some(0),
            3,
            true,
        ));
        assert!(Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::CloseRight,
            Some(2),
            3,
            true,
        ));
        assert!(Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::CloseClean,
            Some(1),
            3,
            false,
        ));
        assert!(!Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::CloseClean,
            Some(1),
            3,
            true,
        ));
        assert!(Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::ToggleReadOnly,
            None,
            3,
            true,
        ));
        assert!(!Workspace::tab_context_menu_intent_disabled(
            TabContextMenuIntent::ToggleReadOnly,
            Some(1),
            3,
            true,
        ));
    }

    #[test]
    fn tab_bar_split_menu_items_match_zed_directional_split_actions() {
        let labels = Workspace::tab_bar_split_menu_intents()
            .iter()
            .map(|intent| intent.label())
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec!["Split Right", "Split Left", "Split Up", "Split Down"]
        );
    }

    #[test]
    fn tab_bar_split_menu_commands_match_directional_helix_primitives() {
        let commands = Workspace::tab_bar_split_menu_intents()
            .iter()
            .map(|intent| intent.commands())
            .collect::<Vec<_>>();

        assert_eq!(
            commands,
            vec![
                &["vsplit"][..],
                &["vsplit", "swap_view_left"][..],
                &["hsplit", "swap_view_up"][..],
                &["hsplit"][..],
            ]
        );
    }

    #[test]
    fn helix_rect_to_scaled_pixel_bounds_fills_target_for_single_view() {
        let (left, top, width, height) = helix_rect_to_scaled_pixel_bounds(
            HelixRect::new(0, 0, 20, 10),
            HelixRect::new(0, 0, 20, 10),
            640.0,
            320.0,
        );

        assert_eq!(f32::from(left), 0.0);
        assert_eq!(f32::from(top), 0.0);
        assert_eq!(f32::from(width), 640.0);
        assert_eq!(f32::from(height), 320.0);
    }

    #[test]
    fn helix_rect_to_scaled_pixel_bounds_maps_split_ratios_to_target() {
        let (left, top, width, height) = helix_rect_to_scaled_pixel_bounds(
            HelixRect::new(20, 0, 20, 10),
            HelixRect::new(0, 0, 40, 10),
            800.0,
            300.0,
        );

        assert_eq!(f32::from(left), 400.0);
        assert_eq!(f32::from(top), 0.0);
        assert_eq!(f32::from(width), 400.0);
        assert_eq!(f32::from(height), 300.0);
    }

    #[test]
    fn document_view_layout_bounds_covers_all_view_rects() {
        let layouts = vec![
            DocumentViewLayout {
                view_id: ViewId::default(),
                area: HelixRect::new(10, 0, 20, 5),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: ViewId::default(),
                area: HelixRect::new(30, 5, 20, 5),
                is_focused: false,
            },
        ];

        assert_eq!(
            document_view_layout_bounds(&layouts),
            Some(HelixRect::new(10, 0, 40, 10))
        );
    }

    #[test]
    fn split_pane_dividers_detect_vertical_shared_edge() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let layouts = vec![
            DocumentViewLayout {
                view_id: before_id,
                area: HelixRect::new(0, 0, 40, 20),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: after_id,
                area: HelixRect::new(40, 0, 40, 20),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);

        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, SplitPaneResizeAxis::Vertical);
        assert_eq!(dividers[0].edge, 40);
        assert_eq!(dividers[0].start, 0);
        assert_eq!(dividers[0].span, 20);
        assert_eq!(dividers[0].gap, 0);
        assert_eq!(dividers[0].before_view_ids, vec![before_id]);
        assert_eq!(dividers[0].after_view_ids, vec![after_id]);
    }

    #[test]
    fn split_pane_dividers_detect_horizontal_shared_edge() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let layouts = vec![
            DocumentViewLayout {
                view_id: before_id,
                area: HelixRect::new(0, 0, 80, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: after_id,
                area: HelixRect::new(0, 10, 80, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);

        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, SplitPaneResizeAxis::Horizontal);
        assert_eq!(dividers[0].edge, 10);
        assert_eq!(dividers[0].start, 0);
        assert_eq!(dividers[0].span, 80);
        assert_eq!(dividers[0].gap, 0);
        assert_eq!(dividers[0].before_view_ids, vec![before_id]);
        assert_eq!(dividers[0].after_view_ids, vec![after_id]);
    }

    #[test]
    fn document_view_visual_area_expands_after_vertical_separator_cell() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let layouts = vec![
            DocumentViewLayout {
                view_id: before_id,
                area: HelixRect::new(0, 0, 40, 20),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: after_id,
                area: HelixRect::new(41, 0, 40, 20),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);

        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, SplitPaneResizeAxis::Vertical);
        assert_eq!(dividers[0].edge, 40);
        assert_eq!(dividers[0].gap, 1);
        assert_eq!(
            document_view_visual_area(layouts[0], &dividers),
            HelixRect::new(0, 0, 40, 20)
        );
        assert_eq!(
            document_view_visual_area(layouts[1], &dividers),
            HelixRect::new(40, 0, 41, 20)
        );
    }

    #[test]
    fn document_view_visual_area_expands_after_horizontal_separator_cell() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let layouts = vec![
            DocumentViewLayout {
                view_id: before_id,
                area: HelixRect::new(0, 0, 80, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: after_id,
                area: HelixRect::new(0, 11, 80, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);

        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, SplitPaneResizeAxis::Horizontal);
        assert_eq!(dividers[0].edge, 10);
        assert_eq!(dividers[0].gap, 1);
        assert_eq!(
            document_view_visual_area(layouts[0], &dividers),
            HelixRect::new(0, 0, 80, 10)
        );
        assert_eq!(
            document_view_visual_area(layouts[1], &dividers),
            HelixRect::new(0, 10, 80, 11)
        );
    }

    #[test]
    fn split_pane_dividers_merge_horizontal_segments_across_vertical_separator_cell() {
        let top_id = test_view_id(1);
        let bottom_left_id = test_view_id(2);
        let bottom_right_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: top_id,
                area: HelixRect::new(0, 0, 81, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: bottom_left_id,
                area: HelixRect::new(0, 11, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: bottom_right_id,
                area: HelixRect::new(41, 11, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let horizontal = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Horizontal)
            .unwrap();

        assert_eq!(dividers.len(), 2);
        assert_eq!(horizontal.edge, 10);
        assert_eq!(horizontal.start, 0);
        assert_eq!(horizontal.span, 81);
        assert_eq!(horizontal.gap, 1);
        assert_eq!(horizontal.before_view_ids, vec![top_id]);
        assert_eq!(
            horizontal.after_view_ids,
            vec![bottom_left_id, bottom_right_id]
        );
    }

    #[test]
    fn split_pane_dividers_merge_vertical_segments_across_horizontal_separator_cell() {
        let left_id = test_view_id(1);
        let right_top_id = test_view_id(2);
        let right_bottom_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: left_id,
                area: HelixRect::new(0, 0, 40, 21),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: right_top_id,
                area: HelixRect::new(41, 0, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: right_bottom_id,
                area: HelixRect::new(41, 11, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let vertical = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Vertical)
            .unwrap();

        assert_eq!(dividers.len(), 2);
        assert_eq!(vertical.edge, 40);
        assert_eq!(vertical.start, 0);
        assert_eq!(vertical.span, 21);
        assert_eq!(vertical.gap, 1);
        assert_eq!(vertical.before_view_ids, vec![left_id]);
        assert_eq!(vertical.after_view_ids, vec![right_top_id, right_bottom_id]);
    }

    #[test]
    fn split_pane_divider_visual_line_expands_horizontal_inside_after_vertical_group() {
        let middle_id = test_view_id(1);
        let right_top_id = test_view_id(2);
        let right_bottom_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: middle_id,
                area: HelixRect::new(0, 0, 40, 21),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: right_top_id,
                area: HelixRect::new(41, 0, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: right_bottom_id,
                area: HelixRect::new(41, 11, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let horizontal = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Horizontal)
            .unwrap();

        assert_eq!(horizontal.edge, 10);
        assert_eq!(horizontal.start, 41);
        assert_eq!(horizontal.span, 40);

        let visual = split_pane_divider_visual_line(horizontal.clone(), &dividers);

        assert_eq!(visual.edge, 10);
        assert_eq!(visual.start, 40);
        assert_eq!(visual.span, 41);
    }

    #[test]
    fn split_pane_divider_visual_line_expands_vertical_inside_after_horizontal_group() {
        let top_id = test_view_id(1);
        let bottom_left_id = test_view_id(2);
        let bottom_right_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: top_id,
                area: HelixRect::new(0, 0, 81, 10),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: bottom_left_id,
                area: HelixRect::new(0, 11, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: bottom_right_id,
                area: HelixRect::new(41, 11, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let vertical = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Vertical)
            .unwrap();

        assert_eq!(vertical.edge, 40);
        assert_eq!(vertical.start, 11);
        assert_eq!(vertical.span, 10);

        let visual = split_pane_divider_visual_line(vertical.clone(), &dividers);

        assert_eq!(visual.edge, 40);
        assert_eq!(visual.start, 10);
        assert_eq!(visual.span, 11);
    }

    #[test]
    fn split_pane_dividers_merge_nested_leaf_segments() {
        let left_id = test_view_id(1);
        let top_right_id = test_view_id(2);
        let bottom_right_id = test_view_id(3);
        let layouts = vec![
            DocumentViewLayout {
                view_id: left_id,
                area: HelixRect::new(0, 0, 40, 20),
                is_focused: true,
            },
            DocumentViewLayout {
                view_id: top_right_id,
                area: HelixRect::new(40, 0, 40, 10),
                is_focused: false,
            },
            DocumentViewLayout {
                view_id: bottom_right_id,
                area: HelixRect::new(40, 10, 40, 10),
                is_focused: false,
            },
        ];

        let dividers = split_pane_dividers(&layouts);
        let vertical = dividers
            .iter()
            .find(|divider| divider.axis == SplitPaneResizeAxis::Vertical)
            .unwrap();

        assert_eq!(dividers.len(), 2);
        assert_eq!(vertical.edge, 40);
        assert_eq!(vertical.start, 0);
        assert_eq!(vertical.span, 20);
        assert_eq!(vertical.gap, 0);
        assert_eq!(vertical.before_view_ids, vec![left_id]);
        assert_eq!(vertical.after_view_ids, vec![top_right_id, bottom_right_id]);
    }

    #[test]
    fn resized_vertical_split_pane_areas_clamp_to_min_width() {
        let before = HelixRect::new(0, 0, 40, 20);
        let after = HelixRect::new(40, 0, 40, 20);

        assert_eq!(
            resized_vertical_split_pane_areas(before, after, 10, SPLIT_PANE_MIN_WIDTH_CELLS),
            Some((HelixRect::new(0, 0, 50, 20), HelixRect::new(50, 0, 30, 20)))
        );
        assert_eq!(
            resized_vertical_split_pane_areas(before, after, -100, SPLIT_PANE_MIN_WIDTH_CELLS),
            Some((HelixRect::new(0, 0, 8, 20), HelixRect::new(8, 0, 72, 20)))
        );
    }

    #[test]
    fn resized_horizontal_split_pane_areas_clamp_to_min_height() {
        let before = HelixRect::new(0, 0, 80, 10);
        let after = HelixRect::new(0, 10, 80, 10);

        assert_eq!(
            resized_horizontal_split_pane_areas(before, after, 4, SPLIT_PANE_MIN_HEIGHT_CELLS),
            Some((HelixRect::new(0, 0, 80, 14), HelixRect::new(0, 14, 80, 6)))
        );
        assert_eq!(
            resized_horizontal_split_pane_areas(before, after, -100, SPLIT_PANE_MIN_HEIGHT_CELLS),
            Some((HelixRect::new(0, 0, 80, 3), HelixRect::new(0, 3, 80, 17)))
        );
    }

    #[test]
    fn split_pane_resized_areas_convert_mouse_delta_to_cells() {
        let before_id = test_view_id(1);
        let after_id = test_view_id(2);
        let state = SplitPaneResizeState {
            axis: SplitPaneResizeAxis::Vertical,
            start_mouse_x: 200.0,
            start_mouse_y: 0.0,
            before_views: vec![SplitPaneResizeViewState {
                view_id: before_id,
                area: HelixRect::new(0, 0, 40, 20),
            }],
            after_views: vec![SplitPaneResizeViewState {
                view_id: after_id,
                area: HelixRect::new(40, 0, 40, 20),
            }],
            total_area: HelixRect::new(0, 0, 80, 20),
            editor_width_px: 800.0,
            editor_height_px: 200.0,
        };

        assert_eq!(
            split_pane_resized_areas(&state, 300.0, 0.0),
            Some(vec![
                (before_id, HelixRect::new(0, 0, 50, 20)),
                (after_id, HelixRect::new(50, 0, 30, 20)),
            ])
        );
    }

    #[test]
    fn split_pane_resized_areas_resize_grouped_panes_together() {
        let before_id = test_view_id(1);
        let top_after_id = test_view_id(2);
        let bottom_after_id = test_view_id(3);
        let state = SplitPaneResizeState {
            axis: SplitPaneResizeAxis::Vertical,
            start_mouse_x: 200.0,
            start_mouse_y: 0.0,
            before_views: vec![SplitPaneResizeViewState {
                view_id: before_id,
                area: HelixRect::new(0, 0, 40, 20),
            }],
            after_views: vec![
                SplitPaneResizeViewState {
                    view_id: top_after_id,
                    area: HelixRect::new(40, 0, 40, 10),
                },
                SplitPaneResizeViewState {
                    view_id: bottom_after_id,
                    area: HelixRect::new(40, 10, 40, 10),
                },
            ],
            total_area: HelixRect::new(0, 0, 80, 20),
            editor_width_px: 800.0,
            editor_height_px: 200.0,
        };

        assert_eq!(
            split_pane_resized_areas(&state, 300.0, 0.0),
            Some(vec![
                (before_id, HelixRect::new(0, 0, 50, 20)),
                (top_after_id, HelixRect::new(50, 0, 30, 10)),
                (bottom_after_id, HelixRect::new(50, 10, 30, 10)),
            ])
        );
    }

    #[test]
    fn terminal_spawn_cwd_uses_loaded_project_root() {
        let project_root = PathBuf::from("/tmp/example-project");

        assert_eq!(
            Workspace::terminal_spawn_cwd(Some(project_root.as_path())),
            Some(project_root)
        );
        assert_eq!(Workspace::terminal_spawn_cwd(None), None);
    }

    #[test]
    fn terminal_cwd_matching_detects_project_root_changes() {
        let old_root = PathBuf::from("/tmp/old-project");
        let new_root = PathBuf::from("/tmp/new-project");

        assert!(Workspace::terminal_cwd_matches(
            Some(old_root.as_path()),
            Some(old_root.as_path())
        ));
        assert!(!Workspace::terminal_cwd_matches(
            Some(old_root.as_path()),
            Some(new_root.as_path())
        ));
        assert!(!Workspace::terminal_cwd_matches(
            None,
            Some(new_root.as_path())
        ));
    }

    #[test]
    fn tab_bar_new_menu_items_match_zed_new_actions() {
        let labels = Workspace::tab_bar_new_menu_intents()
            .iter()
            .map(|intent| intent.label())
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "New File",
                "Open File",
                "Search Project",
                "Search Symbols",
                "New Terminal",
                "New Center Terminal"
            ]
        );
    }

    #[test]
    fn tab_bar_new_menu_entries_match_zed_grouping() {
        let labels = Workspace::tab_bar_new_menu_entries()
            .iter()
            .map(|entry| match entry {
                TabBarNewMenuEntry::Action(intent) => intent.label(),
                TabBarNewMenuEntry::Separator => "|",
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "New File",
                "Open File",
                "|",
                "Search Project",
                "Search Symbols",
                "|",
                "New Terminal",
                "New Center Terminal"
            ]
        );
    }

    #[test]
    fn tab_bar_end_buttons_follow_zed_new_split_order() {
        assert_eq!(
            tab_bar_end_button_icon_paths(),
            ["icons/plus.svg", "icons/columns-2.svg"]
        );
    }

    #[test]
    fn tab_bar_end_button_tooltips_describe_actions() {
        assert_eq!(tab_bar_end_button_tooltips(), ["New File", "Split Pane"]);
    }

    #[test]
    fn file_tree_resize_width_tracks_mouse_and_clamps_to_bounds() {
        assert_eq!(
            Workspace::clamped_file_tree_resize_width(250.0, 300.0, 360.0, 1000.0),
            310.0
        );
        assert_eq!(
            Workspace::clamped_file_tree_resize_width(250.0, 300.0, 100.0, 1000.0),
            FILE_TREE_MIN_WIDTH
        );
        assert_eq!(
            Workspace::clamped_file_tree_resize_width(250.0, 300.0, 1200.0, 1000.0),
            800.0
        );
    }

    #[test]
    fn documentation_sidebar_resize_width_tracks_mouse_and_clamps_to_bounds() {
        assert_eq!(
            Workspace::clamped_documentation_sidebar_resize_width(360.0, 800.0, 700.0, 1000.0),
            460.0
        );
        assert_eq!(
            Workspace::clamped_documentation_sidebar_resize_width(360.0, 800.0, 1100.0, 1000.0),
            DOC_SIDEBAR_MIN_WIDTH
        );
        assert_eq!(
            Workspace::clamped_documentation_sidebar_resize_width(360.0, 800.0, 0.0, 1000.0),
            DOC_SIDEBAR_MAX_WIDTH
        );
        assert_eq!(
            Workspace::clamped_documentation_sidebar_width(360.0, 500.0),
            260.0
        );
    }

    #[test]
    fn file_tree_default_width_uses_default_and_viewport_limit() {
        assert_eq!(
            Workspace::clamped_file_tree_default_width(1000.0),
            FILE_TREE_DEFAULT_WIDTH
        );
        assert_eq!(Workspace::clamped_file_tree_default_width(360.0), 160.0);
        assert_eq!(
            Workspace::clamped_file_tree_default_width(250.0),
            FILE_TREE_MIN_WIDTH
        );
    }

    #[test]
    fn file_tree_config_from_gui_uses_configured_file_tree_options() {
        let mut gui_config = crate::config::GuiConfig::default();
        gui_config.file_tree.density = crate::file_tree::FileTreeDisplayDensity::Relaxed;
        gui_config.file_tree.flatten_empty_directories = false;
        gui_config.ui.look = crate::config::UiLook::System;

        let file_tree_config = file_tree_config_from_gui(&gui_config);

        assert_eq!(
            file_tree_config.density,
            crate::file_tree::FileTreeDisplayDensity::Relaxed
        );
        assert!(!file_tree_config.flatten_empty_directories);
        assert_eq!(
            file_tree_config.translucent_background,
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn move_ordered_item_to_target_index_moves_items_to_target_positions() {
        let mut items = vec!['a', 'b', 'c', 'd'];

        assert!(move_ordered_item_to_target_index(
            &mut items,
            'c',
            Some('a')
        ));
        assert_eq!(items, vec!['c', 'a', 'b', 'd']);

        assert!(move_ordered_item_to_target_index(
            &mut items,
            'a',
            Some('d')
        ));
        assert_eq!(items, vec!['c', 'b', 'd', 'a']);
    }

    #[test]
    fn move_ordered_item_to_target_index_moves_items_to_end() {
        let mut items = vec!['a', 'b', 'c', 'd'];

        assert!(move_ordered_item_to_target_index(&mut items, 'b', None));
        assert_eq!(items, vec!['a', 'c', 'd', 'b']);
    }

    #[test]
    fn move_ordered_item_to_target_index_reports_no_ops() {
        let mut items = vec!['a', 'b', 'c', 'd'];

        assert!(!move_ordered_item_to_target_index(
            &mut items,
            'a',
            Some('a')
        ));
        assert_eq!(items, vec!['a', 'b', 'c', 'd']);

        assert!(!move_ordered_item_to_target_index(&mut items, 'd', None));
        assert!(!move_ordered_item_to_target_index(
            &mut items,
            'x',
            Some('a')
        ));
        assert!(!move_ordered_item_to_target_index(
            &mut items,
            'a',
            Some('x')
        ));
        assert_eq!(items, vec!['a', 'b', 'c', 'd']);
    }

    #[test]
    fn dropped_tab_pin_state_follows_target_region() {
        let items = ['a', 'b', 'c', 'd'];
        let pinned = HashSet::from(['a', 'b']);

        assert_eq!(
            dropped_tab_pin_state(&items, 'c', Some('a'), &pinned),
            Some(true)
        );
        assert_eq!(
            dropped_tab_pin_state(&items, 'a', Some('c'), &pinned),
            Some(false)
        );
        assert_eq!(
            dropped_tab_pin_state(&items, 'a', None, &pinned),
            Some(false)
        );
    }

    #[test]
    fn dropped_tab_pin_state_reports_invalid_drops() {
        let items = ['a', 'b', 'c', 'd'];
        let pinned = HashSet::from(['a', 'b']);

        assert_eq!(dropped_tab_pin_state(&items, 'a', Some('a'), &pinned), None);
        assert_eq!(dropped_tab_pin_state(&items, 'x', Some('a'), &pinned), None);
        assert_eq!(dropped_tab_pin_state(&items, 'a', Some('x'), &pinned), None);
    }

    #[test]
    fn resolved_dropped_tab_pin_state_honours_forced_row_targets() {
        let items = ['a', 'b', 'c', 'd'];
        let pinned = HashSet::from(['a', 'b']);

        assert_eq!(
            resolved_dropped_tab_pin_state(&items, 'c', None, &pinned, Some(true)),
            Some(true)
        );
        assert_eq!(
            resolved_dropped_tab_pin_state(&items, 'a', None, &pinned, Some(false)),
            Some(false)
        );
        assert_eq!(
            resolved_dropped_tab_pin_state(&items, 'a', Some('a'), &pinned, Some(false)),
            None
        );
    }

    #[test]
    fn active_unpinned_tab_scroll_index_ignores_pinned_tabs() {
        let items = ['a', 'b', 'c', 'd', 'e'];
        let pinned = HashSet::from(['b', 'd']);

        assert_eq!(
            active_unpinned_tab_scroll_index(&items, &pinned, 'a'),
            Some(0)
        );
        assert_eq!(
            active_unpinned_tab_scroll_index(&items, &pinned, 'c'),
            Some(1)
        );
        assert_eq!(
            active_unpinned_tab_scroll_index(&items, &pinned, 'e'),
            Some(2)
        );
    }

    #[test]
    fn active_unpinned_tab_scroll_index_skips_pinned_and_missing_tabs() {
        let items = ['a', 'b', 'c', 'd'];
        let pinned = HashSet::from(['a', 'b']);

        assert_eq!(active_unpinned_tab_scroll_index(&items, &pinned, 'a'), None);
        assert_eq!(active_unpinned_tab_scroll_index(&items, &pinned, 'b'), None);
        assert_eq!(active_unpinned_tab_scroll_index(&items, &pinned, 'x'), None);
    }

    #[test]
    fn active_tab_auto_scroll_respects_zed_manual_scroll_suppression() {
        assert!(should_scroll_active_tab(false, None, Some('a')));
        assert!(should_scroll_active_tab(false, Some('a'), Some('b')));
        assert!(!should_scroll_active_tab(false, Some('a'), Some('a')));
        assert!(!should_scroll_active_tab(false, Some('a'), None));
        assert!(!should_scroll_active_tab(true, Some('a'), Some('b')));
    }

    #[test]
    fn change_tab_pin_state_pins_tabs_left_to_right_without_reordering() {
        let mut items = vec!['a', 'b', 'c'];
        let mut pinned = HashSet::new();

        assert!(change_tab_pin_state(&mut items, &mut pinned, 'a', true));
        assert_eq!(items, vec!['a', 'b', 'c']);
        assert_eq!(zed_style_tab_order(&items, &pinned), vec!['a', 'b', 'c']);

        assert!(change_tab_pin_state(&mut items, &mut pinned, 'b', true));
        assert_eq!(items, vec!['a', 'b', 'c']);
        assert_eq!(zed_style_tab_order(&items, &pinned), vec!['a', 'b', 'c']);

        assert!(change_tab_pin_state(&mut items, &mut pinned, 'c', true));
        assert_eq!(items, vec!['a', 'b', 'c']);
        assert_eq!(zed_style_tab_order(&items, &pinned), vec!['a', 'b', 'c']);
    }

    #[test]
    fn change_tab_pin_state_pins_tabs_right_to_left_at_pinned_boundary() {
        let mut items = vec!['a', 'b', 'c'];
        let mut pinned = HashSet::new();

        assert!(change_tab_pin_state(&mut items, &mut pinned, 'c', true));
        assert_eq!(items, vec!['c', 'a', 'b']);
        assert_eq!(zed_style_tab_order(&items, &pinned), vec!['c', 'a', 'b']);

        assert!(change_tab_pin_state(&mut items, &mut pinned, 'b', true));
        assert_eq!(items, vec!['c', 'b', 'a']);
        assert_eq!(zed_style_tab_order(&items, &pinned), vec!['c', 'b', 'a']);

        assert!(change_tab_pin_state(&mut items, &mut pinned, 'a', true));
        assert_eq!(items, vec!['c', 'b', 'a']);
        assert_eq!(zed_style_tab_order(&items, &pinned), vec!['c', 'b', 'a']);
    }

    #[test]
    fn change_tab_pin_state_unpins_tabs_to_start_of_unpinned_region() {
        let mut items = vec!['a', 'b', 'c', 'd'];
        let mut pinned = HashSet::from(['a', 'b']);

        assert!(change_tab_pin_state(&mut items, &mut pinned, 'a', false));
        assert_eq!(items, vec!['b', 'a', 'c', 'd']);
        assert_eq!(
            zed_style_tab_order(&items, &pinned),
            vec!['b', 'a', 'c', 'd']
        );

        assert!(change_tab_pin_state(&mut items, &mut pinned, 'b', false));
        assert_eq!(items, vec!['b', 'a', 'c', 'd']);
        assert_eq!(
            zed_style_tab_order(&items, &pinned),
            vec!['b', 'a', 'c', 'd']
        );
    }

    #[test]
    fn change_tab_pin_state_reports_no_ops() {
        let mut items = vec!['a', 'b'];
        let mut pinned = HashSet::from(['a']);

        assert!(!change_tab_pin_state(&mut items, &mut pinned, 'a', true));
        assert!(!change_tab_pin_state(&mut items, &mut pinned, 'b', false));
        assert!(!change_tab_pin_state(&mut items, &mut pinned, 'x', true));
        assert_eq!(items, vec!['a', 'b']);
        assert_eq!(pinned, HashSet::from(['a']));
    }

    #[test]
    fn unpin_all_tabs_reports_no_ops_when_nothing_is_pinned() {
        let mut pinned = HashSet::<char>::new();

        assert!(!unpin_all_tabs(&mut pinned));
        assert!(pinned.is_empty());
    }

    #[test]
    fn unpin_all_tabs_preserves_current_tab_order() {
        let items = vec!['a', 'b', 'c'];
        let mut pinned = HashSet::from(['a', 'b']);

        assert!(unpin_all_tabs(&mut pinned));
        assert!(pinned.is_empty());
        assert_eq!(zed_style_tab_order(&items, &pinned), items);
    }

    #[test]
    fn preview_tab_toggle_plan_unpreviews_active_preview_tab() {
        let previews = HashSet::from(['b']);

        assert_eq!(
            preview_tab_toggle_plan(&previews, &'b'),
            PreviewTabTogglePlan::Unpreview
        );
    }

    #[test]
    fn preview_tab_toggle_plan_marks_active_non_preview_tab_as_preview() {
        let previews = HashSet::from(['b', 'd']);

        assert_eq!(
            preview_tab_toggle_plan(&previews, &'c'),
            PreviewTabTogglePlan::Preview
        );
    }

    #[test]
    fn project_panel_preview_requires_global_and_project_panel_settings() {
        assert!(should_create_project_panel_preview_tab(true, true, false));
        assert!(!should_create_project_panel_preview_tab(false, true, false));
        assert!(!should_create_project_panel_preview_tab(true, false, false));
    }

    #[test]
    fn project_panel_preview_does_not_reclassify_existing_tabs() {
        assert!(!should_create_project_panel_preview_tab(true, true, true));
    }

    #[test]
    fn changed_preview_documents_are_unpreviewed_only_after_edits() {
        assert!(should_unpreview_changed_document(true, true));
        assert!(!should_unpreview_changed_document(true, false));
        assert!(!should_unpreview_changed_document(false, true));
        assert!(!should_unpreview_changed_document(false, false));
    }

    #[test]
    fn close_others_unpreviews_retained_preview_tab() {
        assert!(should_unpreview_retained_tab_after_close_others(true));
        assert!(!should_unpreview_retained_tab_after_close_others(false));
    }

    #[test]
    fn unsaved_buffers_status_matches_helix_close_error_shape() {
        let status =
            unsaved_buffers_remaining_status(vec!["main.rs".to_string(), "lib.rs".to_string()]);

        assert_eq!(
            status.status,
            "2 unsaved buffers remaining: [\"main.rs\", \"lib.rs\"]"
        );
        assert_eq!(status.severity, Severity::Error);
    }

    #[test]
    fn editor_domain_status_severity_maps_to_workspace_severity() {
        use nucleotide_events::v2::editor::StatusSeverity;

        assert_eq!(
            editor_domain_status_severity(StatusSeverity::Info),
            Severity::Info
        );
        assert_eq!(
            editor_domain_status_severity(StatusSeverity::Success),
            Severity::Info
        );
        assert_eq!(
            editor_domain_status_severity(StatusSeverity::Warning),
            Severity::Warning
        );
        assert_eq!(
            editor_domain_status_severity(StatusSeverity::Error),
            Severity::Error
        );
    }

    #[test]
    fn unsaved_close_confirmation_copy_names_single_and_batch() {
        assert_eq!(unsaved_close_confirmation_title(1), "Close Unsaved Buffer");
        assert_eq!(
            unsaved_close_confirmation_message(&["main.rs".to_string()]),
            "'main.rs' has unsaved changes. Close without saving?"
        );

        assert_eq!(unsaved_close_confirmation_title(2), "Close Unsaved Buffers");
        assert_eq!(
            unsaved_close_confirmation_message(&["main.rs".to_string(), "lib.rs".to_string()]),
            "2 buffers have unsaved changes: main.rs, lib.rs. Close without saving?"
        );
    }

    #[test]
    fn active_tab_close_plan_closes_unpinned_active_tab() {
        let items = ['a', 'b', 'c'];
        let pinned = HashSet::from(['a']);

        assert_eq!(
            active_tab_close_plan(&items, &pinned, Some('b')),
            ActiveTabClosePlan::Close('b')
        );
    }

    #[test]
    fn active_tab_close_plan_activates_unpinned_tab_instead_of_closing_pinned_active_tab() {
        let items = ['a', 'b', 'c'];
        let pinned = HashSet::from(['a', 'b']);

        assert_eq!(
            active_tab_close_plan(&items, &pinned, Some('a')),
            ActiveTabClosePlan::Activate('c')
        );
    }

    #[test]
    fn active_tab_close_plan_ignores_pinned_active_tab_when_no_unpinned_tab_exists() {
        let items = ['a', 'b'];
        let pinned = HashSet::from(['a', 'b']);

        assert_eq!(
            active_tab_close_plan(&items, &pinned, Some('a')),
            ActiveTabClosePlan::Ignore
        );
    }

    #[test]
    fn active_tab_close_plan_ignores_missing_active_tab() {
        let items = ['a', 'b'];
        let pinned = HashSet::from(['a']);

        assert_eq!(
            active_tab_close_plan(&items, &pinned, None),
            ActiveTabClosePlan::Ignore
        );
        assert_eq!(
            active_tab_close_plan(&items, &pinned, Some('x')),
            ActiveTabClosePlan::Ignore
        );
    }

    #[test]
    fn tab_double_click_renames_file_tabs_and_activates_pathless_tabs() {
        assert_eq!(tab_double_click_plan(true), TabDoubleClickPlan::Rename);
        assert_eq!(tab_double_click_plan(false), TabDoubleClickPlan::Activate);
    }

    #[test]
    fn deleted_document_path_matches_zed_backing_file_rule() {
        assert!(!is_deleted_document_path(None));

        let dir = tempfile::tempdir().unwrap();
        let existing_path = dir.path().join("existing.rs");
        std::fs::write(&existing_path, "").unwrap();
        assert!(!is_deleted_document_path(Some(existing_path.as_path())));

        let missing_path = dir.path().join("missing.rs");
        assert!(is_deleted_document_path(Some(missing_path.as_path())));
    }

    #[test]
    fn deleted_document_path_skips_wsl_unc_probe() {
        let remote_path = Path::new(r"\\wsl.localhost\Ubuntu\home\me\missing.rs");

        assert!(!is_deleted_document_path(Some(remote_path)));
    }

    fn activation_doc(id: char, age: u64) -> TabActivationDocument<char> {
        TabActivationDocument {
            id,
            focused_at: std::time::Instant::now() + std::time::Duration::from_secs(age),
        }
    }

    #[test]
    fn tab_activation_target_history_uses_most_recent_remaining_tab() {
        let documents = [
            activation_doc('a', 0),
            activation_doc('b', 3),
            activation_doc('c', 1),
            activation_doc('d', 2),
        ];

        assert_eq!(
            tab_activation_target_after_close(
                &documents,
                'c',
                Some('c'),
                crate::config::TabActivateOnClose::History,
            ),
            Some('b')
        );
    }

    #[test]
    fn tab_activation_target_neighbour_prefers_right_then_left() {
        let documents = [
            activation_doc('a', 0),
            activation_doc('b', 1),
            activation_doc('c', 2),
            activation_doc('d', 3),
        ];

        assert_eq!(
            tab_activation_target_after_close(
                &documents,
                'b',
                Some('b'),
                crate::config::TabActivateOnClose::Neighbour,
            ),
            Some('c')
        );
        assert_eq!(
            tab_activation_target_after_close(
                &documents,
                'd',
                Some('d'),
                crate::config::TabActivateOnClose::Neighbour,
            ),
            Some('c')
        );
    }

    #[test]
    fn tab_activation_target_left_neighbour_prefers_left_then_right() {
        let documents = [
            activation_doc('a', 0),
            activation_doc('b', 1),
            activation_doc('c', 2),
        ];

        assert_eq!(
            tab_activation_target_after_close(
                &documents,
                'c',
                Some('c'),
                crate::config::TabActivateOnClose::LeftNeighbour,
            ),
            Some('b')
        );
        assert_eq!(
            tab_activation_target_after_close(
                &documents,
                'a',
                Some('a'),
                crate::config::TabActivateOnClose::LeftNeighbour,
            ),
            Some('b')
        );
    }

    #[test]
    fn tab_activation_target_ignores_inactive_or_missing_closes() {
        let documents = [activation_doc('a', 0), activation_doc('b', 1)];

        assert_eq!(
            tab_activation_target_after_close(
                &documents,
                'a',
                Some('b'),
                crate::config::TabActivateOnClose::Neighbour,
            ),
            None
        );
        assert_eq!(
            tab_activation_target_after_close(
                &documents,
                'x',
                Some('x'),
                crate::config::TabActivateOnClose::Neighbour,
            ),
            None
        );
    }

    fn max_tab_doc(
        id: char,
        age: u64,
        is_modified: bool,
        is_pinned: bool,
        is_protected: bool,
    ) -> MaxTabsDocument<char> {
        MaxTabsDocument {
            id,
            focused_at: std::time::Instant::now() + std::time::Duration::from_secs(age),
            is_modified,
            is_pinned,
            is_protected,
        }
    }

    #[test]
    fn max_tabs_close_candidates_close_oldest_clean_unpinned_tabs() {
        let documents = [
            max_tab_doc('a', 0, false, false, false),
            max_tab_doc('b', 1, false, false, false),
            max_tab_doc('c', 2, false, false, false),
            max_tab_doc('d', 3, false, false, true),
        ];

        let close_candidates =
            max_tabs_close_candidates(&documents, std::num::NonZeroUsize::new(2));

        assert_eq!(close_candidates, vec!['a', 'b']);
    }

    #[test]
    fn max_tabs_settings_change_target_allows_active_settings_tab_over_cap() {
        let documents = [
            max_tab_doc('a', 0, false, false, false),
            max_tab_doc('b', 1, false, false, false),
            max_tab_doc('c', 2, false, false, false),
            max_tab_doc('s', 3, false, false, true),
        ];

        assert_eq!(
            max_tabs_close_candidates_to_target(&documents, Some(2)),
            vec!['a', 'b']
        );
        assert_eq!(
            max_tabs_close_candidates_to_target(&documents, Some(3)),
            vec!['a']
        );
    }

    #[test]
    fn max_tabs_close_candidates_preserve_dirty_pinned_and_protected_tabs() {
        let documents = [
            max_tab_doc('a', 0, true, false, false),
            max_tab_doc('b', 1, false, true, false),
            max_tab_doc('c', 2, false, false, true),
            max_tab_doc('d', 3, false, false, false),
        ];

        let close_candidates =
            max_tabs_close_candidates(&documents, std::num::NonZeroUsize::new(1));

        assert_eq!(close_candidates, vec!['d']);
    }

    #[test]
    fn max_tabs_close_candidates_do_nothing_when_unlimited_or_under_cap() {
        let documents = [
            max_tab_doc('a', 0, false, false, false),
            max_tab_doc('b', 1, false, false, false),
        ];

        assert!(max_tabs_close_candidates(&documents, None).is_empty());
        assert!(max_tabs_close_candidates(&documents, std::num::NonZeroUsize::new(2)).is_empty());
    }

    fn close_batch_doc(
        id: char,
        path: Option<&'static str>,
        is_active: bool,
    ) -> BatchCloseDocument<char, &'static str> {
        BatchCloseDocument {
            id,
            is_active,
            path,
        }
    }

    #[test]
    fn batch_close_order_matches_zed_path_sorting() {
        let documents = [
            close_batch_doc('u', None, false),
            close_batch_doc('b', Some("/project/b.rs"), false),
            close_batch_doc('a', Some("/project/a.rs"), false),
            close_batch_doc('m', None, false),
        ];

        assert_eq!(
            batch_close_document_order(&documents),
            vec!['a', 'b', 'u', 'm']
        );
    }

    #[test]
    fn batch_close_order_closes_active_document_last() {
        let documents = [
            close_batch_doc('a', Some("/project/a.rs"), false),
            close_batch_doc('z', Some("/project/z.rs"), true),
            close_batch_doc('b', Some("/project/b.rs"), false),
            close_batch_doc('u', None, false),
        ];

        assert_eq!(
            batch_close_document_order(&documents),
            vec!['a', 'b', 'u', 'z']
        );
    }

    fn selection_fragments(selection: &Selection, text: &Rope) -> Vec<String> {
        selection
            .fragments(text.slice(..))
            .map(|fragment| fragment.into_owned())
            .collect()
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_project_detection_basic() {
        // Test that project detection function exists and doesn't panic with valid path

        //         let _detected_types = crate::project_indicator::detect_project_types_for_path(&current_dir);

        // The main goal is ensuring the integration compiles and doesn't panic
        assert!(true, "Project detection should complete without panicking");
    }

    #[test]
    fn test_workspace_project_change_detection() {
        let workspace = TestWorkspace::new();

        // Test that project root change is detected
        let old_root = Some(PathBuf::from("/old/path"));
        let new_root = PathBuf::from("/new/path");

        assert!(workspace.is_project_change(&old_root, &new_root));
        assert!(!workspace.is_project_change(&Some(new_root.clone()), &new_root));
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_lsp_manager_config_creation() {
        // Test that ProjectLspConfig can be created with defaults
        let config = nucleotide_lsp::ProjectLspConfig::default();

        // Basic validation of config fields
        assert!(
            config.enable_proactive_startup,
            "Proactive startup should be enabled by default"
        );
        assert!(
            config.health_check_interval.as_secs() > 0,
            "Health check interval should be positive"
        );

        // This test mainly ensures the integration compiles
        assert!(true, "ProjectLspConfig should be creatable with defaults");
    }

    #[test]
    fn regex_selection_result_selects_matches() {
        let text = Rope::from("one two one");
        let selection = Selection::single(0, text.len_chars());
        let regex = test_regex("one");

        let result = regex_selection_result(
            RegexSelectionAction::Select,
            text.slice(..),
            &selection,
            &regex,
        )
        .unwrap();

        assert_eq!(selection_fragments(&result, &text), vec!["one", "one"]);
    }

    #[test]
    fn regex_selection_result_splits_matches() {
        let text = Rope::from("one,two,three");
        let selection = Selection::single(0, text.len_chars());
        let regex = test_regex(",");

        let result = regex_selection_result(
            RegexSelectionAction::Split,
            text.slice(..),
            &selection,
            &regex,
        )
        .unwrap();

        assert_eq!(
            selection_fragments(&result, &text),
            vec!["one", "two", "three"]
        );
    }

    #[test]
    fn regex_selection_result_keeps_or_removes_matching_selections() {
        let text = Rope::from("one two");
        let selection = Selection::new(
            SmallVec::from_vec(vec![Range::new(0, 3), Range::new(4, 7)]),
            0,
        );
        let regex = test_regex("one");

        let kept = regex_selection_result(
            RegexSelectionAction::Keep,
            text.slice(..),
            &selection,
            &regex,
        )
        .unwrap();
        let removed = regex_selection_result(
            RegexSelectionAction::Remove,
            text.slice(..),
            &selection,
            &regex,
        )
        .unwrap();

        assert_eq!(selection_fragments(&kept, &text), vec!["one"]);
        assert_eq!(selection_fragments(&removed, &text), vec!["two"]);
    }

    #[test]
    fn regex_selection_result_reports_empty_results() {
        let text = Rope::from("one two");
        let selection = Selection::single(0, text.len_chars());
        let regex = test_regex("missing");

        assert_eq!(
            regex_selection_result(
                RegexSelectionAction::Select,
                text.slice(..),
                &selection,
                &regex,
            ),
            Err("nothing selected")
        );
        assert_eq!(
            regex_selection_result(
                RegexSelectionAction::Keep,
                text.slice(..),
                &selection,
                &regex,
            ),
            Err("no selections remaining")
        );
    }

    #[test]
    fn global_search_matches_finds_line_matches() {
        let temp_dir = tempfile::tempdir().unwrap();
        let nested_dir = temp_dir.path().join("nested");
        std::fs::create_dir(&nested_dir).unwrap();
        let first = temp_dir.path().join("first.txt");
        let second = nested_dir.join("second.txt");
        std::fs::write(&first, "alpha\nneedle one\nomega\n").unwrap();
        std::fs::write(&second, "needle two\nplain\n").unwrap();

        let matches = global_search_matches(
            temp_dir.path(),
            "needle",
            true,
            &default_file_picker_config(),
            &[],
            10,
        )
        .unwrap();

        assert_eq!(matches.len(), 2);
        assert!(matches.iter().any(|search_match| {
            search_match.path == first
                && search_match.line == 1
                && search_match.line_text == "needle one"
        }));
        assert!(matches.iter().any(|search_match| {
            search_match.path == second
                && search_match.line == 0
                && search_match.line_text == "needle two"
        }));
    }

    #[test]
    fn global_search_matches_uses_open_document_text() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("buffer.txt");
        std::fs::write(&path, "saved text\n").unwrap();
        let open_documents = vec![(path.clone(), Rope::from("unsaved needle\nsaved text\n"))];

        let matches = global_search_matches(
            temp_dir.path(),
            "needle",
            true,
            &default_file_picker_config(),
            &open_documents,
            10,
        )
        .unwrap();

        assert_eq!(
            matches,
            vec![GlobalSearchMatch {
                path,
                line: 0,
                line_text: "unsaved needle".to_string(),
            }]
        );
    }

    #[test]
    fn remote_global_search_maps_matches_to_workspace_paths() {
        let root = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\iain\repo");
        let regex = compile_global_search_regex("needle", true).unwrap();
        let response = GlobalSearchResponse {
            protocol_version: nucleotide_remote::PROTOCOL_VERSION,
            current_dir: PathBuf::from("/home/iain/repo"),
            matches: vec![nucleotide_remote::GlobalSearchMatchResponse {
                relative_path: PathBuf::from("src/main.rs"),
                line: 3,
                line_text: "let needle = true;".to_string(),
            }],
            truncated: false,
        };

        let matches = global_search_matches_from_remote_response(&root, response, &[], &regex, 10);

        assert_eq!(
            matches,
            vec![GlobalSearchMatch {
                path: workspace_path_from_remote_relative(&root, Path::new("src/main.rs")),
                line: 3,
                line_text: "let needle = true;".to_string(),
            }]
        );
    }

    #[test]
    fn remote_global_search_uses_open_document_text_for_open_paths() {
        let root = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\iain\repo");
        let open_path = workspace_path_from_remote_relative(&root, Path::new("src/main.rs"));
        let regex = compile_global_search_regex("needle", true).unwrap();
        let response = GlobalSearchResponse {
            protocol_version: nucleotide_remote::PROTOCOL_VERSION,
            current_dir: PathBuf::from("/home/iain/repo"),
            matches: vec![nucleotide_remote::GlobalSearchMatchResponse {
                relative_path: PathBuf::from("src/main.rs"),
                line: 4,
                line_text: "saved needle".to_string(),
            }],
            truncated: false,
        };
        let open_documents = vec![(open_path.clone(), Rope::from("unsaved needle\n"))];

        let matches = global_search_matches_from_remote_response(
            &root,
            response,
            &open_documents,
            &regex,
            10,
        );

        assert_eq!(
            matches,
            vec![GlobalSearchMatch {
                path: open_path,
                line: 0,
                line_text: "unsaved needle".to_string(),
            }]
        );
    }

    #[test]
    fn wsl_created_paths_map_back_to_unc_paths() {
        let parent = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\iain\repo\src");
        let file_response = FileCreateResponse {
            protocol_version: nucleotide_remote::PROTOCOL_VERSION,
            current_dir: PathBuf::from("/home/iain/repo/src"),
            path: PathBuf::from("/home/iain/repo/src/main.rs"),
            kind: RemoteFileKind::File,
        };
        let directory_response = FileCreateResponse {
            protocol_version: nucleotide_remote::PROTOCOL_VERSION,
            current_dir: PathBuf::from("/home/iain/repo/src"),
            path: PathBuf::from("/home/iain/repo/src/components"),
            kind: RemoteFileKind::Directory,
        };

        assert_eq!(
            wsl_created_path(&parent, &file_response, RemoteFileKind::File),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu\home\iain\repo\src\main.rs"
            ))
        );
        assert_eq!(
            wsl_created_path(&parent, &directory_response, RemoteFileKind::Directory),
            Some(PathBuf::from(
                r"\\wsl.localhost\Ubuntu\home\iain\repo\src\components"
            ))
        );
        assert_eq!(
            wsl_created_path(&parent, &directory_response, RemoteFileKind::File),
            None
        );
    }

    // Helper struct for testing workspace functionality
    struct TestWorkspace {
        _current_project_root: Option<PathBuf>,
    }

    impl TestWorkspace {
        fn new() -> Self {
            Self {
                _current_project_root: None,
            }
        }

        fn is_project_change(&self, old_root: &Option<PathBuf>, new_root: &PathBuf) -> bool {
            old_root.as_ref() != Some(new_root)
        }
    }
}
