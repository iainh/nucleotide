// ABOUTME: File tree UI view component using GPUI's uniform_list for performance
// ABOUTME: Handles user interaction, selection, and rendering of file tree entries

use std::path::PathBuf;
use gpui::*;
use gpui::prelude::FluentBuilder;
use crate::file_tree::{FileTree, FileTreeEntry, FileTreeEvent, FileTreeConfig, GitStatus, get_file_icon, get_symlink_icon, icons::chevron_icon};
use crate::ui::Theme;

/// File tree view component
pub struct FileTreeView {
    /// The underlying file tree data
    tree: FileTree,
    /// Currently selected entry path
    selected_path: Option<PathBuf>,
    /// Focus handle for keyboard navigation
    focus_handle: FocusHandle,
    /// Scroll handle for the list
    scroll_handle: ScrollHandle,
}

impl FileTreeView {
    /// Create a new file tree view
    pub fn new(root_path: PathBuf, config: FileTreeConfig, cx: &mut Context<Self>) -> Self {
        let mut tree = FileTree::new(root_path, config);
        
        // Load initial tree structure
        if let Err(e) = tree.load() {
            log::error!("Failed to load file tree: {}", e);
        }

        Self {
            tree,
            selected_path: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
        }
    }

    /// Get the current selection
    pub fn selected_path(&self) -> Option<&PathBuf> {
        self.selected_path.as_ref()
    }

    /// Set the selection
    pub fn select_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.selected_path != path {
            self.selected_path = path.clone();
            cx.emit(FileTreeEvent::SelectionChanged { path });
            cx.notify();
        }
    }

    /// Toggle directory expansion
    pub fn toggle_directory(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        // Check if we're already loading this directory
        if self.tree.is_directory_loading(path) {
            return;
        }
        
        let path_buf = path.clone();
        let is_expanded = self.tree.is_expanded(path);
        
        if is_expanded {
            // Collapse is synchronous
            if let Err(e) = self.tree.collapse_directory(path) {
                log::error!("Failed to collapse directory {}: {}", path.display(), e);
            } else {
                cx.emit(FileTreeEvent::DirectoryToggled {
                    path: path.clone(),
                    expanded: false,
                });
                cx.notify();
            }
        } else {
            // Mark directory as loading to prevent double-clicks
            self.tree.mark_directory_loading(path);
            cx.notify();
            
            // Expand is asynchronous - spawn background task
            let path_for_io = path_buf.clone();
            cx.spawn(async move |this, mut cx| {
                // Do the file I/O in a blocking task to avoid blocking the executor
                let entries = cx.background_executor().spawn(async move {
                    match std::fs::read_dir(&path_for_io) {
                        Ok(read_dir) => {
                            let mut entries = Vec::new();
                            for entry in read_dir {
                                if let Ok(entry) = entry {
                                    if let Ok(metadata) = entry.metadata() {
                                        entries.push((entry.path(), metadata));
                                    }
                                }
                            }
                            Ok(entries)
                        }
                        Err(e) => Err(e),
                    }
                }).await;
                
                // Update the UI on the main thread
                if let Some(this) = this.upgrade() {
                    this.update(cx, |view, cx| {
                            match entries {
                                Ok(entries) => {
                                    if let Err(e) = view.tree.expand_directory_with_entries(&path_buf, entries) {
                                        log::error!("Failed to expand directory {}: {}", path_buf.display(), e);
                                    } else {
                                        cx.emit(FileTreeEvent::DirectoryToggled {
                                            path: path_buf.clone(),
                                            expanded: true,
                                        });
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to read directory {}: {}", path_buf.display(), e);
                                    view.tree.unmark_directory_loading(&path_buf);
                                }
                            }
                            cx.notify();
                    });
                }
            }).detach();
        }
    }

    /// Open the selected file
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path.clone() {
            if let Some(entry) = self.tree.entry_by_path(&path) {
                if entry.is_file() {
                    cx.emit(FileTreeEvent::OpenFile { path });
                } else if entry.is_directory() {
                    self.toggle_directory(&path, cx);
                }
            }
        }
    }

    /// Select next entry
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        if entries.is_empty() {
            return;
        }

        let current_index = self.selected_path.as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
            .unwrap_or(0);

        let next_index = (current_index + 1).min(entries.len() - 1);
        self.select_path(Some(entries[next_index].path.clone()), cx);
    }

    /// Select previous entry
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        if entries.is_empty() {
            return;
        }

        let current_index = self.selected_path.as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
            .unwrap_or(0);

        let prev_index = current_index.saturating_sub(1);
        self.select_path(Some(entries[prev_index].path.clone()), cx);
    }

    /// Select first entry
    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        if let Some(first) = entries.first() {
            self.select_path(Some(first.path.clone()), cx);
        }
    }

    /// Select last entry
    pub fn select_last(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        if let Some(last) = entries.last() {
            self.select_path(Some(last.path.clone()), cx);
        }
    }

    /// Refresh the tree
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.tree.refresh() {
            log::error!("Failed to refresh file tree: {}", e);
        } else {
            cx.notify();
        }
    }

    /// Get tree statistics
    pub fn stats(&self) -> crate::file_tree::tree::FileTreeStats {
        self.tree.stats()
    }

    /// Render a single file tree entry
    fn render_entry(&self, entry: &FileTreeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_path.as_ref() == Some(&entry.path);
        let theme = cx.global::<Theme>();
        
        let indentation = px(entry.depth as f32 * 16.0); // 16px per level
        
        div()
            .id(("file-tree-entry", entry.id.0))
            .w_full()
            .h(px(24.0))
            .flex()
            .items_center()
            .pl(indentation)
            .pr(px(8.0))
            .when(is_selected, |div| {
                div.bg(theme.accent)
            })
            .hover(|style| style.bg(theme.surface_hover))
            .on_click({
                let path = entry.path.clone();
                let is_dir = entry.is_directory();
                cx.listener(move |view, _event, _window, cx| {
                    view.select_path(Some(path.clone()), cx);
                    if is_dir {
                        view.toggle_directory(&path, cx);
                    } else {
                        // Open file when clicked
                        cx.emit(FileTreeEvent::OpenFile { 
                            path: path.clone() 
                        });
                    }
                })
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(entry.is_directory(), |div| {
                        div.child(self.render_chevron(entry, cx))
                    })
                    .child(self.render_icon(entry, cx))
                    .child(self.render_filename(entry, cx))
                    .when_some(entry.git_status.as_ref(), |div, status| {
                        div.child(self.render_git_status(status, cx))
                    })
            )
    }

    /// Render the chevron for directories
    fn render_chevron(&self, entry: &FileTreeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        
        chevron_icon(if entry.is_expanded { "down" } else { "right" })
            .size_3()
            .text_color(theme.text_muted)
    }

    /// Render the file/directory icon
    fn render_icon(&self, entry: &FileTreeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        
        let icon = match &entry.kind {
            crate::file_tree::FileKind::Directory { .. } => {
                get_file_icon(None, true, entry.is_expanded)
                    .size_4()
                    .text_color(theme.accent)
            }
            crate::file_tree::FileKind::File { extension } => {
                get_file_icon(extension.as_deref(), false, false)
                    .size_4()
                    .text_color(theme.text)
            }
            crate::file_tree::FileKind::Symlink { target_exists, .. } => {
                get_symlink_icon(*target_exists)
                    .size_4()
                    .text_color(if *target_exists { theme.accent } else { theme.error })
            }
        };

        div()
            .w_4()
            .h_4()
            .flex()
            .items_center()
            .justify_center()
            .child(icon)
    }

    /// Render the filename
    fn render_filename(&self, entry: &FileTreeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let filename = entry.file_name().unwrap_or("?").to_string();

        div()
            .flex_1()
            .text_size(px(14.0))
            .text_color(theme.text)
            .when(entry.is_hidden, |div| {
                div.text_color(theme.text_muted)
            })
            .child(filename)
    }

    /// Render git status indicator
    fn render_git_status(&self, status: &GitStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        
        let (symbol, color) = match status {
            GitStatus::Modified => ("M", theme.warning),
            GitStatus::Added => ("A", theme.success),
            GitStatus::Deleted => ("D", theme.error),
            GitStatus::Untracked => ("?", theme.text_muted),
            GitStatus::Renamed => ("R", theme.accent),
            GitStatus::Conflicted => ("!", theme.error),
            GitStatus::UpToDate => return div(), // Don't show anything for up-to-date files
        };

        div()
            .w(px(16.0))
            .h(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.0))
            .text_color(color)
            .child(symbol)
    }
}

impl EventEmitter<FileTreeEvent> for FileTreeView {}

// FileTreeView is focusable through its focus_handle field

impl Render for FileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let entries = self.tree.visible_entries();

        div()
            .id("file-tree")
            .w_full()
            .h_full()
            .bg(theme.background)
            .border_r_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "ArrowDown" | "j" => {
                        view.select_next(cx);
                    }
                    "ArrowUp" | "k" => {
                        view.select_previous(cx);
                    }
                    "Enter" | " " => {
                        view.open_selected(cx);
                    }
                    "Home" => {
                        view.select_first(cx);
                    }
                    "End" => {
                        view.select_last(cx);
                    }
                    "F5" => {
                        view.refresh(cx);
                    }
                    _ => {}
                }
            }))
            .child(
                // Header
                div()
                    .w_full()
                    .h(px(32.0))
                    .px_3()
                    .py_2()
                    .bg(theme.surface)
                    .border_b_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(theme.text)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Files")
                    )
            )
            .child(
                // File list using uniform_list for performance
                uniform_list("file-tree-list", entries.len(), {
                    let entries = entries.clone(); // Clone once outside the processor
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        let mut items = Vec::with_capacity(range.end - range.start);
                        
                        for index in range {
                            if let Some(entry) = entries.get(index) {
                                items.push(this.render_entry(entry, cx));
                            }
                        }
                        
                        items
                    })
                })
                .flex_1()
                .w_full()
            )
    }
}