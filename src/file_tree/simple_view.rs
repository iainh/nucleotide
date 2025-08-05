// ABOUTME: Simplified file tree view for initial implementation
// ABOUTME: Basic directory listing without advanced SumTree features

use std::path::PathBuf;
use gpui::*;
use crate::file_tree::{FileTreeEvent, FileTreeConfig};
use crate::ui::{Theme, spacing};

/// Simple file tree entry without SumTree complexity
#[derive(Debug, Clone)]
pub struct SimpleFileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_expanded: bool,
    pub depth: usize,
}

/// Simplified file tree view
pub struct SimpleFileTreeView {
    /// Root path
    root_path: PathBuf,
    /// List of visible entries
    entries: Vec<SimpleFileEntry>,
    /// Currently selected index
    selected_index: Option<usize>,
    /// Focus handle
    focus_handle: FocusHandle,
    /// Config
    _config: FileTreeConfig,
}

impl SimpleFileTreeView {
    pub fn new(root_path: PathBuf, config: FileTreeConfig, cx: &mut Context<Self>) -> Self {
        let entries = Self::load_entries(&root_path, 0, 1); // Load 1 level initially
        
        Self {
            root_path,
            entries,
            selected_index: None,
            focus_handle: cx.focus_handle(),
            _config: config,
        }
    }

    fn load_entries(path: &PathBuf, depth: usize, max_depth: usize) -> Vec<SimpleFileEntry> {
        let mut entries = Vec::new();
        
        if depth > max_depth {
            return entries;
        }

        if let Ok(read_dir) = std::fs::read_dir(path) {
            let mut items: Vec<_> = read_dir
                .filter_map(|entry| entry.ok())
                .collect();
            
            // Sort directories first, then files, all alphabetically
            items.sort_by(|a, b| {
                let a_is_dir = a.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                let b_is_dir = b.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                
                match (a_is_dir, b_is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.file_name().cmp(&b.file_name()),
                }
            });

            for entry in items {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                
                // Skip hidden files for now
                if name.starts_with('.') {
                    continue;
                }

                entries.push(SimpleFileEntry {
                    path: path.clone(),
                    name,
                    is_dir,
                    is_expanded: false,
                    depth,
                });
            }
        }

        entries
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }

        let next_index = match self.selected_index {
            Some(current) => (current + 1).min(self.entries.len() - 1),
            None => 0,
        };

        self.selected_index = Some(next_index);
        cx.notify();
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }

        let prev_index = match self.selected_index {
            Some(current) => current.saturating_sub(1),
            None => 0,
        };

        self.selected_index = Some(prev_index);
        cx.notify();
    }

    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.selected_index {
            if let Some(entry) = self.entries.get(index).cloned() {
                if entry.is_dir {
                    self.toggle_directory(&entry.path, cx);
                } else {
                    // Emit file open event
                    cx.emit(FileTreeEvent::OpenFile {
                        path: entry.path,
                    });
                }
            }
        }
    }

    pub fn toggle_directory(&mut self, dir_path: &PathBuf, cx: &mut Context<Self>) {
        // Find the directory entry
        if let Some(dir_index) = self.entries.iter().position(|e| &e.path == dir_path && e.is_dir) {
            let was_expanded = self.entries[dir_index].is_expanded;
            
            if was_expanded {
                // Collapse: remove all children
                self.collapse_directory(dir_index);
            } else {
                // Expand: add children
                self.expand_directory(dir_index);
            }
            
            // Toggle the expanded state
            self.entries[dir_index].is_expanded = !was_expanded;
            
            cx.emit(FileTreeEvent::DirectoryToggled {
                path: dir_path.clone(),
                expanded: !was_expanded,
            });
            
            cx.notify();
        }
    }

    fn expand_directory(&mut self, dir_index: usize) {
        let dir_entry = &self.entries[dir_index];
        let dir_path = dir_entry.path.clone();
        let dir_depth = dir_entry.depth;
        
        // Load children of this directory
        let children = Self::load_entries(&dir_path, dir_depth + 1, dir_depth + 1);
        
        // Insert children after the directory entry
        for (i, child) in children.into_iter().enumerate() {
            self.entries.insert(dir_index + 1 + i, child);
        }
    }

    fn collapse_directory(&mut self, dir_index: usize) {
        let dir_depth = self.entries[dir_index].depth;
        let mut remove_count = 0;
        
        // Count how many entries to remove (all children and their descendants)
        for entry in self.entries.iter().skip(dir_index + 1) {
            if entry.depth <= dir_depth {
                break;
            }
            remove_count += 1;
        }
        
        // Remove the entries
        for _ in 0..remove_count {
            self.entries.remove(dir_index + 1);
        }
        
        // Update selected index if it was in the collapsed area
        if let Some(selected) = self.selected_index {
            if selected > dir_index && selected <= dir_index + remove_count {
                self.selected_index = Some(dir_index);
            } else if selected > dir_index + remove_count {
                self.selected_index = Some(selected - remove_count);
            }
        }
    }

    fn render_entry(&self, entry: &SimpleFileEntry, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_index == Some(index);
        let theme = cx.global::<Theme>();
        
        let indentation = px(entry.depth as f32 * 16.0);
        
        let mut entry_div = div()
            .w_full()
            .h(px(28.0)) // Slightly taller for better click targets
            .flex()
            .items_center()
            .pl(indentation)
            .pr(px(8.0))
            .py(px(2.0)); // Add vertical padding
        
        // Better selected state styling
        if is_selected {
            entry_div = entry_div
                .bg(theme.accent)
                .text_color(white());
        } else {
            entry_div = entry_div.text_color(theme.text);
        }
        
        entry_div.hover(|style| {
            if is_selected {
                style.bg(theme.accent_hover)
            } else {
                style.bg(theme.surface_hover)
            }
        })
        .cursor_pointer()
        .on_mouse_down(gpui::MouseButton::Left, {
            let path = entry.path.clone();
            let is_dir = entry.is_dir;
            cx.listener(move |view, _event, _window, cx| {
                // Set selection
                view.selected_index = Some(index);
                
                if is_dir {
                    // Toggle directory expansion
                    view.toggle_directory(&path, cx);
                } else {
                    // Open file
                    cx.emit(FileTreeEvent::OpenFile {
                        path: path.clone(),
                    });
                }
                
                cx.notify();
            })
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0)) // Better spacing
                .child(
                    // Expand/collapse arrow for directories
                    if entry.is_dir {
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.0))
                            .child(if entry.is_expanded { "▼" } else { "▶" })
                    } else {
                        div().w(px(16.0)) // Spacer for files
                    }
                )
                .child(
                    // File/folder icon
                    div()
                        .w(px(16.0))
                        .h(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .child(if entry.is_dir { "📁" } else { "📄" })
                )
                .child(
                    // Filename with proper text styling
                    div()
                        .flex_1()
                        .text_size(px(13.0))
                        .font_weight(if entry.is_dir { 
                            gpui::FontWeight::MEDIUM 
                        } else { 
                            gpui::FontWeight::NORMAL 
                        })
                        .child(entry.name.clone())
                )
        )
    }
}

impl EventEmitter<FileTreeEvent> for SimpleFileTreeView {}

impl Render for SimpleFileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();

        div()
            .id("simple-file-tree")
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
                    _ => {}
                }
            }))
            .child(
                // Header
                div()
                    .w_full()
                    .h(px(36.0))
                    .px(spacing::MD)
                    .py(spacing::SM)
                    .bg(theme.surface)
                    .border_b_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_muted)
                            .child("FILES")
                    )
            )
            .child(
                // File list - simple list instead of uniform_list for now
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .children(
                        self.entries.iter().enumerate().map(|(index, entry)| {
                            self.render_entry(entry, index, cx)
                        })
                    )
            )
    }
}