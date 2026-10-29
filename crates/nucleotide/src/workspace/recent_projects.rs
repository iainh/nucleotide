//! Native recent-project registration and Windows Jump List integration.

#[cfg(target_os = "windows")]
use std::collections::VecDeque;
use std::path::Path;
#[cfg(target_os = "windows")]
use std::path::PathBuf;
#[cfg(target_os = "windows")]
use std::sync::{LazyLock, Mutex};

use gpui::App;
#[cfg(target_os = "windows")]
use gpui::MenuItem;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use nucleotide_logging::debug;
#[cfg(target_os = "windows")]
use nucleotide_logging::warn;
use nucleotide_workspace::WorkspaceIdentity;
#[cfg(target_os = "windows")]
use smallvec::{SmallVec, smallvec};

pub(super) fn should_add_recent_project(backend_identity: &WorkspaceIdentity) -> bool {
    matches!(backend_identity, WorkspaceIdentity::Local)
}

#[cfg(target_os = "macos")]
pub(super) fn add_recent_project(path: &Path, cx: &mut App) {
    if path.is_dir() {
        cx.add_recent_document(path);
        debug!(project_root = %path.display(), "Added project to macOS recent documents");
    }
}

#[cfg(target_os = "windows")]
pub(super) const WINDOWS_JUMP_LIST_PROJECT_LIMIT: usize = 10;

#[cfg(target_os = "windows")]
static WINDOWS_RECENT_PROJECTS: LazyLock<Mutex<VecDeque<PathBuf>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

#[cfg(target_os = "windows")]
pub(super) fn add_recent_project(path: &Path, cx: &mut App) {
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
pub(super) fn windows_recent_project_path(path: &Path) -> Option<PathBuf> {
    if !path.is_dir() {
        return None;
    }

    Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
}

#[cfg(target_os = "windows")]
pub(super) fn windows_wide_nul_path(path: &Path) -> Vec<u16> {
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
pub(super) fn windows_update_recent_project_list(recent: &mut VecDeque<PathBuf>, path: PathBuf) {
    if let Some(index) = recent.iter().position(|entry| entry == &path) {
        recent.remove(index);
    }

    recent.push_front(path);

    while recent.len() > WINDOWS_JUMP_LIST_PROJECT_LIMIT {
        recent.pop_back();
    }
}

#[cfg(target_os = "windows")]
pub(super) fn windows_jump_list_entries(recent: &VecDeque<PathBuf>) -> Vec<SmallVec<[PathBuf; 2]>> {
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
pub(super) fn add_recent_project(_path: &Path, _cx: &mut App) {}
