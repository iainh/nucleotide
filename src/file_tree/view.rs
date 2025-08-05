// ABOUTME: File tree UI view component using GPUI's uniform_list for performance
// ABOUTME: Handles user interaction, selection, and rendering of file tree entries

use std::path::{Path, PathBuf};
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

        let mut instance = Self {
            tree,
            selected_path: None,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
        };

        // Auto-select the first entry if there are any entries
        let entries = instance.tree.visible_entries();
        println!("FileTreeView: Found {} visible entries during initialization", entries.len());
        if !entries.is_empty() {
            println!("FileTreeView: Auto-selecting first entry: {:?}", entries[0].path);
            instance.selected_path = Some(entries[0].path.clone());
        } else {
            println!("FileTreeView: No entries to auto-select");
        }

        instance
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
    
    /// Sync selection with the currently open file
    pub fn sync_selection_with_file(&mut self, file_path: Option<&Path>, cx: &mut Context<Self>) {
        if let Some(path) = file_path {
            // Only update if the path exists in the tree
            if self.tree.entry_by_path(path).is_some() {
                self.select_path(Some(path.to_path_buf()), cx);
                
                // Ensure parent directories are expanded so the file is visible
                if let Some(parent) = path.parent() {
                    self.ensure_path_visible(parent, cx);
                }
            }
        }
    }
    
    /// Ensure a path is visible by expanding parent directories
    fn ensure_path_visible(&mut self, path: &Path, cx: &mut Context<Self>) {
        // Start from the root and expand directories along the path
        let mut current = PathBuf::new();
        
        for component in path.components() {
            current.push(component);
            
            if let Some(entry) = self.tree.entry_by_path(&current) {
                if entry.is_directory() && !self.tree.is_expanded(&current) {
                    // Expand this directory using toggle_directory
                    self.toggle_directory(&current, cx);
                }
            }
        }
        
        cx.notify();
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
        println!("select_next: {} visible entries", entries.len());
        if entries.is_empty() {
            println!("select_next: No entries available");
            return;
        }

        // If no selection, start with first entry
        if self.selected_path.is_none() {
            println!("select_next: No selection, selecting first entry");
            self.select_path(Some(entries[0].path.clone()), cx);
            return;
        }

        let current_index = self.selected_path.as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
            .unwrap_or(0);

        println!("select_next: current_index={}, selected_path={:?}", current_index, self.selected_path);
        let next_index = (current_index + 1).min(entries.len() - 1);
        println!("select_next: moving from index {} to {}", current_index, next_index);
        self.select_path(Some(entries[next_index].path.clone()), cx);
    }

    /// Select previous entry
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        log::debug!("select_previous: {} visible entries", entries.len());
        if entries.is_empty() {
            log::debug!("select_previous: No entries available");
            return;
        }

        // If no selection, start with first entry
        if self.selected_path.is_none() {
            log::debug!("select_previous: No selection, selecting first entry");
            self.select_path(Some(entries[0].path.clone()), cx);
            return;
        }

        let current_index = self.selected_path.as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
            .unwrap_or(0);

        log::debug!("select_previous: current_index={}, selected_path={:?}", current_index, self.selected_path);
        let prev_index = current_index.saturating_sub(1);
        log::debug!("select_previous: moving from index {} to {}", current_index, prev_index);
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

    /// Handle left arrow key navigation
    pub fn navigate_left(&mut self, cx: &mut Context<Self>) {
        if let Some(current_path) = self.selected_path.clone() {
            if let Some(current_entry) = self.tree.entry_by_path(&current_path) {
                if current_entry.is_directory() && current_entry.is_expanded() {
                    // Collapse the current directory if it's expanded
                    self.toggle_directory(&current_path, cx);
                } else {
                    // Navigate to parent directory
                    if let Some(parent_entry) = self.tree.find_parent_entry(&current_path) {
                        self.select_path(Some(parent_entry.path), cx);
                    }
                }
            }
        }
    }

    /// Handle right arrow key navigation  
    pub fn navigate_right(&mut self, cx: &mut Context<Self>) {
        if let Some(current_path) = self.selected_path.clone() {
            if let Some(current_entry) = self.tree.entry_by_path(&current_path) {
                if current_entry.is_directory() {
                    if !current_entry.is_expanded() {
                        // Expand the current directory if it's collapsed
                        self.toggle_directory(&current_path, cx);
                    } else {
                        // Navigate to first child if already expanded
                        if let Some(first_child) = self.tree.find_first_child_entry(&current_path) {
                            self.select_path(Some(first_child.path), cx);
                        }
                    }
                }
                // For files, right arrow does nothing
            }
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
                div.bg(theme.accent).text_color(theme.background)
            })
            .hover(|style| style.bg(theme.surface_hover))
            .on_click({
                let path = entry.path.clone();
                let is_dir = entry.is_directory();
                let is_root = entry.depth == 0;
                cx.listener(move |view, _event, window, cx| {
                    // Focus the tree view when any entry is clicked
                    log::debug!("File tree entry clicked, focusing tree view");
                    view.focus_handle.focus(window);
                    view.select_path(Some(path.clone()), cx);
                    
                    // Don't toggle the root directory
                    if is_dir && !is_root {
                        view.toggle_directory(&path, cx);
                    } else if !is_dir {
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
        
        // Root directory is always shown as expanded
        let is_expanded = entry.is_expanded || entry.depth == 0;
        
        chevron_icon(if is_expanded { "down" } else { "right" })
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
        
        // For root directory, show just the directory name
        let filename = if entry.depth == 0 && entry.is_directory() {
            entry.path.file_name()
                .and_then(|name| name.to_str())
                .or_else(|| entry.path.components().last()
                    .and_then(|c| c.as_os_str().to_str()))
                .unwrap_or(".")
                .to_string()
        } else {
            entry.file_name().unwrap_or("?").to_string()
        };

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

impl Focusable for FileTreeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// FileTreeView is focusable through its focus_handle field

impl Render for FileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let entries = self.tree.visible_entries();

        div()
            .id("file-tree")
            .key_context("FileTree")
            .w_full()
            .h_full()
            .bg(theme.background)
            .border_r_1()
            .border_color(theme.border)
            .when(self.focus_handle.is_focused(_window), |style| {
                style.border_color(theme.border_focused)
            })
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .on_click(cx.listener(|view, _event, window, _cx| {
                // Focus the tree view when clicked anywhere on it
                log::debug!("File tree container clicked, focusing");
                view.focus_handle.focus(window);
            }))
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _window, cx| {
                println!("File tree received key event: {:?}", event.keystroke.key);
                match event.keystroke.key.as_str() {
                    "down" | "j" => {
                        println!("File tree: down/j pressed");
                        view.select_next(cx);
                    }
                    "up" | "k" => {
                        println!("File tree: up/k pressed");
                        view.select_previous(cx);
                    }
                    "left" | "h" => {
                        println!("File tree: left/h pressed");
                        view.navigate_left(cx);
                    }
                    "right" | "l" => {
                        println!("File tree: right/l pressed");
                        view.navigate_right(cx);
                    }
                    "enter" | " " => {
                        println!("File tree: enter/space pressed");
                        view.open_selected(cx);
                    }
                    "home" => {
                        view.select_first(cx);
                    }
                    "end" => {
                        view.select_last(cx);
                    }
                    "f5" => {
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