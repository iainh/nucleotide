// ABOUTME: File tree UI view component using GPUI's uniform_list for performance
// ABOUTME: Handles user interaction, selection, and rendering of file tree entries

use crate::file_tree::{
    get_file_icon, get_symlink_icon, icons::chevron_icon, DebouncedFileTreeWatcher, FileTree,
    FileTreeConfig, FileTreeEntry, FileTreeEvent, GitStatus,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use nucleotide_logging::{debug, error, warn};
use nucleotide_ui::theme_utils::color_to_hsla;
use nucleotide_ui::{
    scrollbar::{Scrollbar, ScrollbarState},
    Theme,
};
use std::path::{Path, PathBuf};

/// File tree view component
pub struct FileTreeView {
    /// The underlying file tree data
    tree: FileTree,
    /// Currently selected entry path
    selected_path: Option<PathBuf>,
    /// Focus handle for keyboard navigation
    focus_handle: FocusHandle,
    /// Scroll handle for the list
    scroll_handle: UniformListScrollHandle,
    /// Scrollbar state for managing scrollbar UI
    scrollbar_state: ScrollbarState,
    /// Tokio runtime handle for async VCS operations
    tokio_handle: Option<tokio::runtime::Handle>,
    /// File system watcher for detecting changes
    _file_watcher: Option<DebouncedFileTreeWatcher>,
}

impl FileTreeView {
    /// Create a new file tree view
    pub fn new(root_path: PathBuf, config: FileTreeConfig, cx: &mut Context<Self>) -> Self {
        let mut tree = FileTree::new(root_path, config);

        // Load initial tree structure
        if let Err(e) = tree.load() {
            error!(error = %e, "Failed to load file tree");
        }

        let scroll_handle = UniformListScrollHandle::new();
        let scrollbar_state = ScrollbarState::new(scroll_handle.clone());

        let mut instance = Self {
            tree,
            selected_path: None,
            focus_handle: cx.focus_handle(),
            scroll_handle,
            scrollbar_state,
            tokio_handle: None,
            _file_watcher: None,
        };

        // Auto-select the first entry if there are any entries
        let entries = instance.tree.visible_entries();
        if !entries.is_empty() {
            instance.selected_path = Some(entries[0].path.clone());
        }

        // Apply test VCS statuses for demonstration
        instance.apply_test_statuses(cx);

        instance
    }

    /// Create a new file tree view with Tokio runtime handle for VCS operations
    pub fn new_with_runtime(
        root_path: PathBuf,
        config: FileTreeConfig,
        tokio_handle: Option<tokio::runtime::Handle>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut tree = FileTree::new(root_path.clone(), config.clone());

        // Load initial tree structure
        if let Err(e) = tree.load() {
            error!(error = %e, "Failed to load file tree");
        }

        // Try to create file watcher if filesystem watching is enabled
        let file_watcher = if config.watch_filesystem {
            match DebouncedFileTreeWatcher::with_defaults(root_path.clone()) {
                Ok(watcher) => {
                    debug!(root_path = ?root_path, "File system watcher created");
                    Some(watcher)
                }
                Err(e) => {
                    warn!(error = %e, "Failed to create file system watcher");
                    None
                }
            }
        } else {
            None
        };

        let scroll_handle = UniformListScrollHandle::new();
        let scrollbar_state = ScrollbarState::new(scroll_handle.clone());

        let mut instance = Self {
            tree,
            selected_path: None,
            focus_handle: cx.focus_handle(),
            scroll_handle,
            scrollbar_state,
            tokio_handle,
            _file_watcher: file_watcher,
        };

        // Auto-select the first entry if there are any entries
        let entries = instance.tree.visible_entries();
        if !entries.is_empty() {
            instance.selected_path = Some(entries[0].path.clone());
        }

        // Start async VCS loading if we have a runtime handle
        if instance.tokio_handle.is_some() {
            debug!("Starting async VCS refresh with Tokio handle");
            instance.start_async_vcs_refresh(cx);
        } else {
            // Fallback to test statuses for demonstration
            debug!("No Tokio handle available, using test statuses");
            instance.apply_test_statuses(cx);
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
    pub fn toggle_directory(&mut self, path: &Path, cx: &mut Context<Self>) {
        // Check if we're already loading this directory
        if self.tree.is_directory_loading(path) {
            return;
        }

        let path_buf = path.to_path_buf();
        let is_expanded = self.tree.is_expanded(path);

        if is_expanded {
            // Collapse is synchronous
            if let Err(e) = self.tree.collapse_directory(path) {
                error!(path = %path.display(), error = %e, "Failed to collapse directory");
            } else {
                cx.emit(FileTreeEvent::DirectoryToggled {
                    path: path.to_path_buf(),
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
            cx.spawn(async move |this, cx| {
                // Do the file I/O in a blocking task to avoid blocking the executor
                let entries = cx
                    .background_executor()
                    .spawn(async move {
                        match std::fs::read_dir(&path_for_io) {
                            Ok(read_dir) => {
                                let mut entries = Vec::new();
                                for entry in read_dir.flatten() {
                                    if let Ok(metadata) = entry.metadata() {
                                        entries.push((entry.path(), metadata));
                                    }
                                }
                                Ok(entries)
                            }
                            Err(e) => Err(e),
                        }
                    })
                    .await;

                // Update the UI on the main thread
                if let Some(this) = this.upgrade() {
                    let _ = this.update(cx, |view, cx| {
                        match entries {
                            Ok(entries) => {
                                if let Err(e) =
                                    view.tree.expand_directory_with_entries(&path_buf, entries)
                                {
                                    error!(
                                        directory = %path_buf.display(),
                                        error = %e,
                                        "Failed to expand directory"
                                    );
                                } else {
                                    cx.emit(FileTreeEvent::DirectoryToggled {
                                        path: path_buf.clone(),
                                        expanded: true,
                                    });
                                }
                            }
                            Err(e) => {
                                error!(
                                    directory = %path_buf.display(),
                                    error = %e,
                                    "Failed to read directory"
                                );
                                view.tree.unmark_directory_loading(&path_buf);
                            }
                        }

                        // Refresh VCS status after expanding directory
                        if view.tree.needs_vcs_refresh() {
                            view.start_vcs_refresh(cx);
                        }

                        cx.notify();
                    });
                }
            })
            .detach();
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

        // If no selection, start with first entry
        if self.selected_path.is_none() {
            self.select_path(Some(entries[0].path.clone()), cx);
            return;
        }

        let current_index = self
            .selected_path
            .as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
            .unwrap_or(0);

        let next_index = (current_index + 1).min(entries.len() - 1);
        self.select_path(Some(entries[next_index].path.clone()), cx);
    }

    /// Select previous entry
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let entries = self.tree.visible_entries();
        debug!(
            entry_count = entries.len(),
            "select_previous: visible entries"
        );
        if entries.is_empty() {
            debug!("select_previous: No entries available");
            return;
        }

        // If no selection, start with first entry
        if self.selected_path.is_none() {
            debug!("select_previous: No selection, selecting first entry");
            self.select_path(Some(entries[0].path.clone()), cx);
            return;
        }

        let current_index = self
            .selected_path
            .as_ref()
            .and_then(|path| entries.iter().position(|e| &e.path == path))
            .unwrap_or(0);

        debug!(
            current_index = current_index,
            selected_path = ?self.selected_path,
            "select_previous: current state"
        );
        let prev_index = current_index.saturating_sub(1);
        debug!(
            from_index = current_index,
            to_index = prev_index,
            "select_previous: moving selection"
        );
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
            error!(error = %e, "Failed to refresh file tree");
        } else {
            // Apply test VCS statuses for demonstration
            self.apply_test_statuses(cx);
            cx.notify();
        }
    }

    /// Handle VCS refresh request
    pub fn handle_vcs_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        if force || self.tree.needs_vcs_refresh() {
            if let Some(ref handle) = self.tokio_handle {
                debug!(force = force, "VCS refresh requested");
                self.start_async_vcs_refresh_with_handle(handle.clone(), cx);
            } else {
                warn!("VCS refresh requested but no Tokio handle available");
                let (_, root_path) = self.tree.get_vcs_info();
                cx.emit(FileTreeEvent::VcsRefreshFailed {
                    repository_root: root_path,
                    error: "No Tokio runtime handle available".to_string(),
                });
            }
        } else {
            debug!("VCS refresh skipped - not needed");
        }
    }

    /// Handle file system change event (should trigger VCS refresh)
    pub fn handle_file_system_change(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        debug!(path = ?path, "File system change detected");

        // Only refresh VCS if the change is within our repository
        let (_, root_path) = self.tree.get_vcs_info();
        if path.starts_with(&root_path) {
            debug!("File system change is within repository, triggering VCS refresh");
            // Trigger a VCS refresh after a file system change
            self.handle_vcs_refresh(false, cx);
        }
    }

    /// Trigger manual VCS refresh by emitting RefreshVcs event
    pub fn request_vcs_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        debug!(force = force, "Manual VCS refresh requested");
        cx.emit(FileTreeEvent::RefreshVcs { force });
    }

    /// Get tree statistics
    pub fn stats(&self) -> crate::file_tree::tree::FileTreeStats {
        self.tree.stats()
    }

    /// Start async VCS refresh
    fn start_vcs_refresh(&self, cx: &mut Context<Self>) {
        if let Some(ref handle) = self.tokio_handle {
            self.start_async_vcs_refresh_with_handle(handle.clone(), cx);
        } else {
            debug!("VCS refresh requested but no Tokio handle available");
        }
    }

    /// Start async VCS refresh using the stored Tokio handle
    fn start_async_vcs_refresh(&self, cx: &mut Context<Self>) {
        if let Some(ref handle) = self.tokio_handle {
            debug!("Starting VCS refresh with handle");
            self.start_async_vcs_refresh_with_handle(handle.clone(), cx);
        } else {
            debug!("VCS refresh requested but no Tokio handle available");
        }
    }

    /// Start async VCS refresh using GPUI's background executor
    fn start_async_vcs_refresh_with_handle(
        &self,
        _handle: tokio::runtime::Handle,
        cx: &mut Context<Self>,
    ) {
        let (_, root_path) = self.tree.get_vcs_info();

        debug!(root_path = ?root_path, "VCS refresh starting");

        // Emit VCS refresh started event
        cx.emit(FileTreeEvent::VcsRefreshStarted {
            repository_root: root_path.clone(),
        });

        let root_path_for_event = root_path.clone();
        cx.spawn(async move |this, cx| {
            // Use GPUI's background executor instead of Tokio spawn_blocking
            let vcs_result = cx
                .background_executor()
                .spawn(async move {
                    // Directly call the git status implementation instead of using for_each_changed_file

                    debug!(root_path = ?root_path, "VCS: Background task started");

                    // Check if this is actually a git repository
                    let git_dir = root_path.join(".git");

                    if !git_dir.exists() {
                        return Vec::new();
                    }

                    // Call git status to get changes
                    let mut changes = Vec::new();

                    // Try to use helix-vcs git functions directly
                    // Import the git status function
                    let result = std::panic::catch_unwind(|| {
                        use helix_vcs::FileChange;

                        // We can't use for_each_changed_file as it uses tokio::spawn_blocking
                        // Instead, let's manually call git status
                        match std::process::Command::new("git")
                            .arg("status")
                            .arg("--porcelain")
                            .current_dir(&root_path)
                            .output()
                        {
                            Ok(output) => {
                                let git_status = String::from_utf8_lossy(&output.stdout);

                                for line in git_status.lines() {
                                    if line.len() >= 3 {
                                        let status_chars = &line[0..2];
                                        let file_path = line[3..].trim();
                                        let full_path = root_path.join(file_path);

                                        let change = match status_chars {
                                            "??" => FileChange::Untracked { path: full_path },
                                            " M" | "M " | "MM" => {
                                                FileChange::Modified { path: full_path }
                                            }
                                            " D" | "D " => FileChange::Deleted { path: full_path },
                                            "UU" | "AA" | "DD" => {
                                                FileChange::Conflict { path: full_path }
                                            }
                                            _ => {
                                                // For any other status, treat as modified
                                                FileChange::Modified { path: full_path }
                                            }
                                        };
                                        changes.push(change);
                                    }
                                }

                                changes
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to run git status");
                                Vec::new()
                            }
                        }
                    });

                    match result {
                        Ok(changes) => changes,
                        Err(_) => {
                            error!("VCS: Panic occurred during git status parsing");
                            Vec::new()
                        }
                    }
                })
                .await;

            // Apply VCS status results to the UI
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    let mut status_map = std::collections::HashMap::new();

                    for change in vcs_result {
                        use helix_vcs::FileChange;

                        let path = match &change {
                            FileChange::Untracked { path } => path.clone(),
                            FileChange::Modified { path } => path.clone(),
                            FileChange::Conflict { path } => path.clone(),
                            FileChange::Deleted { path } => path.clone(),
                            FileChange::Renamed { to_path, .. } => to_path.clone(),
                        };

                        let status = match &change {
                            FileChange::Untracked { .. } => GitStatus::Untracked,
                            FileChange::Modified { .. } => GitStatus::Modified,
                            FileChange::Conflict { .. } => GitStatus::Conflicted,
                            FileChange::Deleted { .. } => GitStatus::Deleted,
                            FileChange::Renamed { .. } => GitStatus::Renamed,
                        };

                        status_map.insert(path, status);
                    }

                    debug!(
                        status_count = status_map.len(),
                        "Successfully loaded VCS status entries"
                    );
                    for (path, status) in &status_map {
                        debug!(file_name = ?path.file_name(), status = ?status, "VCS status");
                    }

                    let affected_files: Vec<PathBuf> = status_map.keys().cloned().collect();
                    view.tree.apply_vcs_status(status_map);

                    // Emit VCS status changed event
                    cx.emit(FileTreeEvent::VcsStatusChanged {
                        repository_root: root_path_for_event.clone(),
                        affected_files,
                    });

                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Apply test VCS statuses for demonstration
    pub fn apply_test_statuses(&mut self, cx: &mut Context<Self>) {
        // Create some test VCS statuses for demonstration
        let mut status_map = std::collections::HashMap::new();

        // Get the root path to create test paths
        let (_, root_path) = self.tree.get_vcs_info();
        debug!(root_path = ?root_path, "Root path for VCS test");

        // Get current visible entries to see what files actually exist
        let entries = self.tree.visible_entries();
        debug!("Current visible entries:");
        for entry in &entries {
            debug!(
                path = ?entry.path,
                entry_type = if entry.is_directory() { "dir" } else { "file" },
                "Visible entry"
            );
        }

        // Add test statuses for files that actually exist in the tree
        for entry in &entries {
            if !entry.is_directory() {
                let filename = entry.path.file_name().unwrap_or_default().to_string_lossy();
                match filename.as_ref() {
                    "Cargo.toml" => {
                        status_map.insert(entry.path.clone(), GitStatus::Modified);
                    }
                    "main.rs" => {
                        status_map.insert(entry.path.clone(), GitStatus::Modified);
                    }
                    "view.rs" => {
                        status_map.insert(entry.path.clone(), GitStatus::Modified);
                    }
                    "tree.rs" => {
                        status_map.insert(entry.path.clone(), GitStatus::Modified);
                    }
                    "CLAUDE.md" => {
                        status_map.insert(entry.path.clone(), GitStatus::Untracked);
                    }
                    name if name.ends_with(".md") => {
                        status_map.insert(entry.path.clone(), GitStatus::Untracked);
                    }
                    name if name.ends_with(".rs") => {
                        status_map.insert(entry.path.clone(), GitStatus::Modified);
                    }
                    _ => {}
                }
            }
        }

        // Also add test statuses for common files in subdirectories that might exist
        // These will be applied even if the directories aren't currently expanded
        let common_test_files = vec![
            ("src/main.rs", GitStatus::Modified),
            ("src/application.rs", GitStatus::Modified),
            ("src/workspace.rs", GitStatus::Modified),
            ("src/file_tree/view.rs", GitStatus::Modified),
            ("src/file_tree/tree.rs", GitStatus::Modified),
            ("src/file_tree/entry.rs", GitStatus::Modified),
            ("src/file_tree/mod.rs", GitStatus::Modified),
            ("src/document.rs", GitStatus::Modified),
            ("src/ui/mod.rs", GitStatus::Modified),
            ("src/ui/theme.rs", GitStatus::Modified),
            ("CLAUDE.md", GitStatus::Untracked),
            ("AGENTS.md", GitStatus::Untracked),
            ("PROJECT_DIRECTORY_DESIGN.md", GitStatus::Untracked),
            ("README.md", GitStatus::Untracked),
            ("docs/README.md", GitStatus::Untracked),
        ];

        for (relative_path, status) in common_test_files {
            let full_path = root_path.join(relative_path);
            if full_path.exists() {
                status_map.insert(full_path, status);
            }
        }

        // Apply the test statuses immediately
        debug!(
            status_count = status_map.len(),
            "Applying test VCS statuses to actual files"
        );
        for (path, status) in &status_map {
            debug!(file_name = ?path.file_name(), status = ?status, "Test VCS status");
        }

        self.tree.apply_vcs_status(status_map);

        // Debug the VCS status after applying
        self.tree.debug_vcs_status();

        cx.notify();

        debug!("Applied test VCS statuses to demonstrate indicators");
    }

    /// Render a single file tree entry
    fn render_entry(&self, entry: &FileTreeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_path.as_ref() == Some(&entry.path);
        let theme = cx.global::<Theme>();

        // Get ui.selection background color from Helix theme
        let selection_bg = {
            let helix_theme = cx.global::<crate::ThemeManager>().helix_theme();
            helix_theme
                .get("ui.selection")
                .bg
                .and_then(color_to_hsla)
                .unwrap_or(theme.accent)
        };

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
                div.bg(selection_bg).text_color(theme.background)
            })
            .hover(|style| style.bg(theme.surface_hover))
            .on_click({
                let path = entry.path.clone();
                let is_dir = entry.is_directory();
                cx.listener(move |view, _event, window, cx| {
                    // Focus the tree view when any entry is clicked
                    debug!("File tree entry clicked, focusing tree view");
                    view.focus_handle.focus(window);
                    view.select_path(Some(path.clone()), cx);

                    if is_dir {
                        view.toggle_directory(&path, cx);
                    } else {
                        // Open file when clicked
                        cx.emit(FileTreeEvent::OpenFile { path: path.clone() });
                    }
                })
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        // Always reserve space for chevron to align icons
                        div()
                            .w_3()
                            .h_3()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(entry.is_directory(), |div| {
                                div.child(self.render_chevron(entry, cx))
                            }),
                    )
                    .child(self.render_icon_with_vcs_status(entry, cx))
                    .child(self.render_filename(entry, cx)),
            )
    }

    /// Render the chevron for directories
    fn render_chevron(&self, entry: &FileTreeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();

        chevron_icon(if entry.is_expanded { "down" } else { "right" })
            .size_3()
            .text_color(theme.text_muted)
    }

    /// Render the file/directory icon with VCS status overlay
    fn render_icon_with_vcs_status(
        &self,
        entry: &FileTreeEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.global::<Theme>();

        let icon = match &entry.kind {
            crate::file_tree::FileKind::Directory { .. } => {
                get_file_icon(None, true, entry.is_expanded)
                    .size_4()
                    .text_color(theme.text)
            }
            crate::file_tree::FileKind::File { extension } => {
                get_file_icon(extension.as_deref(), false, false)
                    .size_4()
                    .text_color(theme.text)
            }
            crate::file_tree::FileKind::Symlink { target_exists, .. } => {
                get_symlink_icon(*target_exists)
                    .size_4()
                    .text_color(if *target_exists {
                        theme.accent
                    } else {
                        theme.error
                    })
            }
        };

        // Container with relative positioning for the icon and overlay
        div()
            .w_4()
            .h_4()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .child(icon)
            .when_some(entry.git_status.as_ref(), |div, status| {
                div.child(self.render_vcs_status_overlay(status, cx))
            })
    }

    /// Render the filename
    fn render_filename(&self, entry: &FileTreeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();

        // For root directory, show just the directory name
        let filename = if entry.depth == 0 && entry.is_directory() {
            entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .or_else(|| {
                    entry
                        .path
                        .components()
                        .next_back()
                        .and_then(|c| c.as_os_str().to_str())
                })
                .unwrap_or(".")
                .to_string()
        } else {
            entry.file_name().unwrap_or("?").to_string()
        };

        div()
            .flex_1()
            .text_size(px(14.0))
            .text_color(theme.text)
            .when(entry.is_hidden, |div| div.text_color(theme.text_muted))
            .child(filename)
    }

    /// Render VCS status as an overlay dot positioned at bottom left of icon
    fn render_vcs_status_overlay(
        &self,
        status: &GitStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.global::<Theme>();

        let color = match status {
            GitStatus::Modified => theme.warning,
            GitStatus::Added => theme.success,
            GitStatus::Deleted => theme.error,
            GitStatus::Untracked => theme.text_muted,
            GitStatus::Renamed => theme.accent,
            GitStatus::Conflicted => theme.error,
            GitStatus::UpToDate => return div(), // Don't show anything for up-to-date files
        };

        // Position at bottom left of the icon (absolute positioning)
        // Slightly offset so it doesn't completely cover the corner
        div()
            .absolute()
            .bottom(px(-2.0)) // Slightly extend below the icon
            .left(px(-2.0)) // Slightly extend to the left of the icon
            .w(px(8.0)) // 8px diameter
            .h(px(8.0))
            .rounded_full()
            .bg(color)
            .border_1()
            .border_color(theme.background) // Add a small border to separate from icon
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

        // Get prompt background color for consistency
        let prompt_bg = {
            let helix_theme = cx.global::<crate::ThemeManager>().helix_theme();
            let popup_style = helix_theme.get("ui.popup");
            popup_style
                .bg
                .and_then(color_to_hsla)
                .or_else(|| helix_theme.get("ui.background").bg.and_then(color_to_hsla))
                .unwrap_or(theme.background)
        };

        div()
            .id("file-tree")
            .key_context("FileTree")
            .w_full()
            .h_full()
            .bg(prompt_bg)
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .on_click(cx.listener(|view, _event, window, _cx| {
                // Focus the tree view when clicked anywhere on it
                debug!("File tree container clicked, focusing");
                view.focus_handle.focus(window);
            }))
            // Handle FileTree actions
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::SelectNext, _window, cx| {
                    view.select_next(cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::SelectPrev, _window, cx| {
                    view.select_previous(cx);
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::ToggleExpanded, _window, cx| {
                    // For left/right arrow keys, handle expand/collapse
                    if let Some(selected_path) = view.selected_path.clone() {
                        if let Some(entry) = view.tree.entry_by_path(&selected_path) {
                            if entry.is_directory() {
                                view.toggle_directory(&selected_path, cx);
                            }
                        }
                    }
                },
            ))
            .on_action(cx.listener(
                |view, _: &crate::actions::file_tree::OpenFile, _window, cx| {
                    view.open_selected(cx);
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .w_full()
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
                        .track_scroll(self.scroll_handle.clone())
                        .flex_1(),
                    )
                    .when_some(
                        Scrollbar::vertical(self.scrollbar_state.clone()),
                        |div, scrollbar| div.child(scrollbar),
                    ),
            )
    }
}
