// ABOUTME: File tree UI view component using GPUI's uniform_list for performance
// ABOUTME: Handles user interaction, selection, and rendering of file tree entries

use crate::file_tree::watcher::FileTreeWatcher;
use crate::file_tree::{
    FileTree, FileTreeConfig, FileTreeEntry, FileTreeEvent, icons::chevron_icon,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use nucleotide_logging::{debug, error, warn};
use nucleotide_types::VcsStatus;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::{
    ListItem, ListItemSpacing, ListItemVariant, Theme, VcsIcon, VcsIconRenderer,
    scrollbar::{Scrollbar, ScrollbarState},
};
use nucleotide_vcs::VcsServiceHandle;
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
    _tokio_handle: Option<tokio::runtime::Handle>,
    /// File system watcher for detecting changes
    file_watcher: Option<FileTreeWatcher>,
    /// Pending file system events for debouncing
    pending_fs_events: std::collections::HashMap<PathBuf, FileTreeEvent>,
    /// Last file system event time for debouncing
    last_fs_event_time: Option<std::time::Instant>,
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
            _tokio_handle: None,
            file_watcher: None,
            pending_fs_events: std::collections::HashMap::new(),
            last_fs_event_time: None,
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
            debug!(root_path = ?root_path, "Attempting to create file system watcher");
            match FileTreeWatcher::new(root_path.clone()) {
                Ok(watcher) => {
                    debug!(root_path = ?root_path, "File system watcher created successfully");
                    Some(watcher)
                }
                Err(e) => {
                    warn!(error = %e, root_path = ?root_path, "Failed to create file system watcher");
                    None
                }
            }
        } else {
            debug!("File system watching disabled in config");
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
            _tokio_handle: tokio_handle,
            file_watcher,
            pending_fs_events: std::collections::HashMap::new(),
            last_fs_event_time: None,
        };

        // Auto-select the first entry if there are any entries
        let entries = instance.tree.visible_entries();
        if !entries.is_empty() {
            instance.selected_path = Some(entries[0].path.clone());
        }

        // VCS monitoring will be handled by the global VCS service
        // The file tree will query VCS status at render time via get_vcs_status_for_entry

        // Start file watcher if enabled
        if instance.file_watcher.is_some() {
            debug!("Starting file system watcher");
            instance.start_file_watcher(cx);
        } else {
            debug!("No file watcher available, file watching disabled");
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

            if let Some(entry) = self.tree.entry_by_path(&current)
                && entry.is_directory()
                && !self.tree.is_expanded(&current)
            {
                // Expand this directory using toggle_directory
                self.toggle_directory(&current, cx);
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

                        // VCS status is handled by the global VCS service

                        cx.notify();
                    });
                }
            })
            .detach();
        }
    }

    /// Open the selected file
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path.clone()
            && let Some(entry) = self.tree.entry_by_path(&path)
        {
            if entry.is_file() {
                cx.emit(FileTreeEvent::OpenFile { path });
            } else if entry.is_directory() {
                self.toggle_directory(&path, cx);
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
        if let Some(current_path) = self.selected_path.clone()
            && let Some(current_entry) = self.tree.entry_by_path(&current_path)
        {
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

    /// Handle right arrow key navigation  
    pub fn navigate_right(&mut self, cx: &mut Context<Self>) {
        if let Some(current_path) = self.selected_path.clone()
            && let Some(current_entry) = self.tree.entry_by_path(&current_path)
            && current_entry.is_directory()
        {
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

    /// Refresh a single directory by rescanning its entries and expanding it
    pub fn refresh_directory(&mut self, dir: &Path, cx: &mut Context<Self>) {
        match std::fs::read_dir(dir) {
            Ok(read_dir) => {
                let mut entries = Vec::new();
                for entry in read_dir.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        entries.push((entry.path(), metadata));
                    }
                }
                if let Err(e) = self.tree.expand_directory_with_entries(dir, entries) {
                    error!(path=%dir.display(), error=%e, "Failed to refresh directory entries");
                }
                cx.notify();
            }
            Err(e) => {
                error!(path=%dir.display(), error=%e, "Failed to read directory during refresh");
            }
        }
    }

    /// Handle VCS refresh request - now uses centralized VCS service
    pub fn handle_vcs_refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        debug!(force = force, "VCS refresh requested");

        // Use centralized VCS service instead of file tree's own VCS logic
        let root_path = self.tree.root_path().to_path_buf();
        let vcs_handle = cx.global::<VcsServiceHandle>().service().clone();

        vcs_handle.update(cx, |service, cx| {
            // Start monitoring if not already
            if service.root_path() != Some(&root_path) {
                service.start_monitoring(root_path.clone(), cx);
            } else if force {
                // Force refresh the VCS status
                service.force_refresh(cx);
            }
        });

        // Update file tree entries with current VCS status
        self.update_entries_with_vcs_status(cx);
    }

    /// Update file tree entries with VCS status from the centralized service
    fn update_entries_with_vcs_status(&mut self, cx: &mut Context<Self>) {
        // Now that we use the centralized VCS service, we don't need to update entries directly
        // Instead, entries will query VCS status during rendering via get_vcs_status_for_entry
        debug!("VCS status update requested - will be handled during render");
        cx.notify(); // Trigger re-render to pick up new VCS status
    }

    /// Handle file system change event (should trigger VCS refresh)
    pub fn handle_file_system_change(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        debug!(path = ?path, "File system change detected");

        // Only refresh VCS if the change is within our repository
        let root_path = self.tree.root_path().to_path_buf();
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
    #[allow(dead_code)]
    fn start_vcs_refresh(&mut self, cx: &mut Context<Self>) {
        // VCS refresh rate limiting is now handled by the global VCS service

        self.start_async_vcs_refresh_with_handle(cx);
    }

    /// Start async VCS refresh using the stored Tokio handle
    #[allow(dead_code)]
    fn start_async_vcs_refresh(&self, cx: &mut Context<Self>) {
        debug!("Starting VCS refresh");
        self.start_async_vcs_refresh_with_handle(cx);
    }

    /// Start async VCS refresh using GPUI's background executor
    #[allow(dead_code)]
    fn start_async_vcs_refresh_with_handle(&self, cx: &mut Context<Self>) {
        let root_path = self.tree.root_path().to_path_buf();

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
                this.update(cx, |_view, cx| {
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
                            FileChange::Untracked { .. } => VcsStatus::Untracked,
                            FileChange::Modified { .. } => VcsStatus::Modified,
                            FileChange::Conflict { .. } => VcsStatus::Conflicted,
                            FileChange::Deleted { .. } => VcsStatus::Deleted,
                            FileChange::Renamed { .. } => VcsStatus::Renamed,
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
                    // VCS status is now handled by the global VCS service

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
        let root_path = self.tree.root_path().to_path_buf();
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
                        status_map.insert(entry.path.clone(), VcsStatus::Modified);
                    }
                    "main.rs" => {
                        status_map.insert(entry.path.clone(), VcsStatus::Modified);
                    }
                    "view.rs" => {
                        status_map.insert(entry.path.clone(), VcsStatus::Modified);
                    }
                    "tree.rs" => {
                        status_map.insert(entry.path.clone(), VcsStatus::Modified);
                    }
                    "CLAUDE.md" => {
                        status_map.insert(entry.path.clone(), VcsStatus::Untracked);
                    }
                    name if name.ends_with(".md") => {
                        status_map.insert(entry.path.clone(), VcsStatus::Untracked);
                    }
                    name if name.ends_with(".rs") => {
                        status_map.insert(entry.path.clone(), VcsStatus::Modified);
                    }
                    _ => {}
                }
            }
        }

        // Also add test statuses for common files in subdirectories that might exist
        // These will be applied even if the directories aren't currently expanded
        let common_test_files = vec![
            ("src/main.rs", VcsStatus::Modified),
            ("src/application.rs", VcsStatus::Modified),
            ("src/workspace.rs", VcsStatus::Modified),
            ("src/file_tree/view.rs", VcsStatus::Modified),
            ("src/file_tree/tree.rs", VcsStatus::Modified),
            ("src/file_tree/entry.rs", VcsStatus::Modified),
            ("src/file_tree/mod.rs", VcsStatus::Modified),
            ("src/document.rs", VcsStatus::Modified),
            ("src/ui/mod.rs", VcsStatus::Modified),
            ("src/ui/theme.rs", VcsStatus::Modified),
            ("CLAUDE.md", VcsStatus::Untracked),
            ("AGENTS.md", VcsStatus::Untracked),
            ("PROJECT_DIRECTORY_DESIGN.md", VcsStatus::Untracked),
            ("README.md", VcsStatus::Untracked),
            ("docs/README.md", VcsStatus::Untracked),
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

        // VCS status is now handled by the global VCS service
        // Test VCS status will be visible through get_vcs_status_for_entry

        cx.notify();

        debug!("Applied test VCS statuses to demonstrate indicators");
    }

    /// Start the file system watcher background task
    fn start_file_watcher(&mut self, cx: &mut Context<Self>) {
        if let Some(mut watcher) = self.file_watcher.take() {
            debug!("Starting file system watcher background task");

            cx.spawn(async move |this, cx| {
                while let Some(event) = watcher.next_event().await {
                    if let Some(this) = this.upgrade() {
                        let _ = this.update(cx, |view, cx| {
                            view.queue_file_system_event(event, cx);
                        });
                    } else {
                        // Component was dropped, stop watching
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// Queue a file system event for debounced processing
    fn queue_file_system_event(&mut self, event: FileTreeEvent, cx: &mut Context<Self>) {
        if let FileTreeEvent::FileSystemChanged { path, kind } = &event {
            debug!(path = ?path, kind = ?kind, "Queuing file system event");

            // Immediate debug: check if root is expanded
            if let Some(parent) = path.parent() {
                let is_expanded = self.tree.is_expanded(parent);
                debug!(parent = ?parent, is_expanded = is_expanded, "Parent directory expansion status");
            }

            // Coalesce with any pending event for this path to avoid losing deletes/renames
            if let Some(prev) = self.pending_fs_events.get(path) {
                let merged = Self::merge_fs_events(prev, &event);
                debug!(prev = ?prev, new = ?event, merged = ?merged, "Coalesced FS event");
                self.pending_fs_events.insert(path.clone(), merged);
            } else {
                self.pending_fs_events.insert(path.clone(), event);
            }
            self.last_fs_event_time = Some(std::time::Instant::now());

            // Schedule a debounced processing after 300ms
            cx.spawn(async move |this, cx| {
                // Wait for debounce period
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(300))
                    .await;

                if let Some(this) = this.upgrade() {
                    let _ = this.update(cx, |view, cx| {
                        view.process_pending_events(cx);
                    });
                }
            })
            .detach();
        }
    }

    /// Process pending file system events
    fn process_pending_events(&mut self, cx: &mut Context<Self>) {
        debug!("Processing pending file system events");

        // Check if enough time has passed since the last event
        if let Some(last_time) = self.last_fs_event_time
            && last_time.elapsed() < std::time::Duration::from_millis(300)
        {
            debug!("Debounce time not elapsed yet, skipping processing");
            return;
        }

        // Collect events to process to avoid borrowing issues
        let events_to_process: Vec<_> = self.pending_fs_events.drain().collect();
        debug!(
            event_count = events_to_process.len(),
            "Processing {} pending events",
            events_to_process.len()
        );

        // Process all collected events
        for (_, event) in events_to_process {
            debug!("Processing event: {:?}", event);
            self.handle_file_system_event(event, cx);
        }

        self.last_fs_event_time = None;
    }

    /// Handle a file system event and update the tree structure
    fn handle_file_system_event(&mut self, event: FileTreeEvent, cx: &mut Context<Self>) {
        if let FileTreeEvent::FileSystemChanged { path, kind } = &event {
            debug!(path = ?path, kind = ?kind, "Handling file system event");

            use crate::file_tree::FileSystemEventKind;
            match kind {
                FileSystemEventKind::Created => {
                    self.handle_file_created(path, cx);
                }
                FileSystemEventKind::Deleted => {
                    self.handle_file_deleted(path, cx);
                }
                FileSystemEventKind::Modified => {
                    self.handle_file_modified(path, cx);
                }
                FileSystemEventKind::Renamed { from, to } => {
                    self.handle_file_renamed(from, to, cx);
                }
            }

            // Emit the event to workspace for further handling (VCS refresh, etc.)
            cx.emit(event);
        }
    }

    /// Handle file/directory creation
    fn handle_file_created(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        debug!(path = ?path, "Handling file creation");

        // Check if the parent directory is expanded and visible
        if let Some(parent) = path.parent() {
            // Only add if parent directory is expanded (visible in tree)
            if self.tree.is_expanded(parent) {
                debug!(parent = ?parent, "Parent directory is expanded, adding new entry");

                // Create the new entry
                if let Ok(metadata) = std::fs::metadata(path) {
                    let entry = self.create_tree_entry(path, &metadata, parent);

                    // Add the entry to the tree
                    self.tree.upsert_entry(entry);
                    debug!(path = ?path, "Successfully added new entry to tree");

                    // Trigger VCS refresh to get correct status indicators for the new file
                    debug!(path = ?path, "Triggering VCS refresh for newly added file");
                    self.handle_vcs_refresh(false, cx);

                    cx.notify();
                } else {
                    debug!(path = ?path, "Failed to get metadata for created file");
                }
            } else {
                debug!(parent = ?parent, "Parent directory not expanded, skipping file addition");
            }
        } else {
            debug!(path = ?path, "No parent directory found for created file");
        }
    }

    /// Create a tree entry from path and metadata  
    fn create_tree_entry(
        &mut self,
        path: &PathBuf,
        metadata: &std::fs::Metadata,
        parent: &std::path::Path,
    ) -> crate::file_tree::FileTreeEntry {
        use crate::file_tree::FileTreeEntry;

        let id = self.tree.next_entry_id();
        let mtime = metadata.modified().ok();

        // Determine depth based on parent
        let parent_depth = if let Some(parent_entry) = self.tree.entry_by_path(parent) {
            parent_entry.depth
        } else {
            0
        };

        let mut entry = if metadata.is_dir() {
            FileTreeEntry::new_directory(id, path.clone(), mtime)
        } else if metadata.is_file() {
            FileTreeEntry::new_file(id, path.clone(), metadata.len(), mtime)
        } else {
            // Symlink
            let target = std::fs::read_link(path).ok();
            let target_exists = target.as_ref().map(|t| t.exists()).unwrap_or(false);
            FileTreeEntry::new_symlink(id, path.clone(), target, target_exists, mtime)
        };

        entry.depth = parent_depth + 1;
        entry.is_visible = true;

        // VCS status will be retrieved during rendering from centralized service
        entry.git_status = None;

        entry
    }

    /// Get VCS status for a specific file path using the centralized service
    fn get_vcs_status_for_entry(&self, path: &Path, cx: &Context<Self>) -> Option<VcsStatus> {
        let vcs_service = cx.global::<VcsServiceHandle>();
        vcs_service.get_status(path, cx)
    }

    /// Handle file/directory deletion  
    fn handle_file_deleted(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        debug!(path = ?path, "Handling file deletion");

        // Remove the entry from the tree
        if self.tree.remove_entry(path).is_some() {
            // If the deleted file was selected, clear selection or select next available
            if self.selected_path.as_ref() == Some(path) {
                // Try to select the next available entry
                let entries = self.tree.visible_entries();
                let new_selection = if !entries.is_empty() {
                    Some(entries[0].path.clone())
                } else {
                    None
                };
                self.select_path(new_selection, cx);
            }
            cx.notify();
        }
    }

    /// Handle file modification
    fn handle_file_modified(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        debug!(path = ?path, "Handling file modification");

        // If it no longer exists on disk, treat as deletion
        if !path.exists() {
            debug!(path = ?path, "Modified event but path missing on disk; treating as deletion");
            self.handle_file_deleted(path, cx);
            return;
        }

        // Check if this file exists in the tree
        if let Some(_existing_entry) = self.tree.entry_by_path(path) {
            // File exists in tree - this is a genuine modification
            debug!(path = ?path, "File exists in tree, treating as modification");

            // For now, just refresh VCS status since file content changed
            // We could also update metadata if needed (size, permissions, etc.)
            self.handle_vcs_refresh(false, cx);
        } else {
            // File doesn't exist in tree - this might be a new file creation
            // that was reported as "Modified" instead of "Created" by the OS
            debug!(path = ?path, "File not in tree, checking if it's a new file");

            // Verify the file actually exists on disk
            if path.exists() {
                debug!(path = ?path, "File exists on disk but not in tree, treating as creation");
                self.handle_file_created(path, cx);
            } else {
                debug!(path = ?path, "File doesn't exist on disk or in tree, ignoring");
            }
        }
    }

    /// Coalesce two file-system events for the same path using precedence:
    /// Deleted > Renamed > Created > Modified
    fn merge_fs_events(prev: &FileTreeEvent, next: &FileTreeEvent) -> FileTreeEvent {
        use crate::file_tree::FileSystemEventKind as K;
        let (p_kind, p_path) = match prev {
            FileTreeEvent::FileSystemChanged { path, kind } => (kind, path),
            _ => return next.clone(),
        };
        let (n_kind, n_path) = match next {
            FileTreeEvent::FileSystemChanged { path, kind } => (kind, path),
            _ => return next.clone(),
        };

        if p_path != n_path {
            return next.clone();
        }

        let rank = |k: &K| match k {
            K::Deleted => 3,
            K::Renamed { .. } => 2,
            K::Created => 1,
            K::Modified => 0,
        };

        match (p_kind, n_kind) {
            (K::Created, K::Deleted) => FileTreeEvent::FileSystemChanged {
                path: n_path.clone(),
                kind: K::Deleted,
            },
            (K::Deleted, K::Created) => FileTreeEvent::FileSystemChanged {
                path: n_path.clone(),
                kind: K::Created,
            },
            (K::Deleted, K::Modified) => FileTreeEvent::FileSystemChanged {
                path: n_path.clone(),
                kind: K::Deleted,
            },
            _ if rank(n_kind) >= rank(p_kind) => next.clone(),
            _ => prev.clone(),
        }
    }

    /// Handle file/directory rename
    fn handle_file_renamed(&mut self, from: &PathBuf, to: &PathBuf, cx: &mut Context<Self>) {
        debug!(from = ?from, to = ?to, "Handling file rename");

        // Remove the old entry
        if self.tree.remove_entry(from).is_some() {
            // Update selection if the renamed file was selected
            if self.selected_path.as_ref() == Some(from) {
                self.select_path(Some(to.clone()), cx);
            }

            // Add the new entry if parent is expanded
            if let Some(parent) = to.parent()
                && self.tree.is_expanded(parent)
            {
                debug!(parent = ?parent, "Parent directory is expanded, adding renamed entry");

                if let Ok(metadata) = std::fs::metadata(to) {
                    let entry = self.create_tree_entry(to, &metadata, parent);
                    self.tree.upsert_entry(entry);
                    debug!(path = ?to, "Successfully added renamed entry to tree");

                    // Trigger VCS refresh to get correct status for the renamed file
                    debug!(from = ?from, to = ?to, "Triggering VCS refresh for renamed file");
                    self.handle_vcs_refresh(false, cx);
                } else {
                    debug!(path = ?to, "Failed to get metadata for renamed file");
                }
            }

            cx.notify();
        }
    }

    /// Render a single file tree entry using enhanced ListItem component with wrapped GPUI div
    fn render_entry(
        &self,
        entry: &FileTreeEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let is_selected = self.selected_path.as_ref() == Some(&entry.path);
        // Use provider hooks to get theme
        let theme =
            nucleotide_ui::providers::use_provider::<nucleotide_ui::providers::ThemeProvider>()
                .map(|provider| provider.current_theme().clone())
                .unwrap_or_else(|| cx.global::<Theme>().clone());

        let indentation = px(entry.depth as f32 * 16.0); // 16px per level
        let path = entry.path.clone();
        let is_dir = entry.is_directory();

        // Wrapper adds click behavior; row content built via helper to centralize styling/layout
        div()
            .w_full()
            .on_mouse_up(MouseButton::Left, {
                let path = path.clone();
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
            .on_mouse_down(MouseButton::Right, {
                let path = path.clone();
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    // Focus and select the item under cursor, then request context menu
                    view.focus_handle.focus(window);
                    view.select_path(Some(path.clone()), cx);
                    // Emit context menu request with screen coordinates
                    cx.emit(FileTreeEvent::ContextMenuRequested {
                        path: path.clone(),
                        x: event.position.x.0,
                        y: event.position.y.0,
                    });
                })
            })
            .child(self.build_file_tree_row(&theme, entry, is_selected, indentation, cx))
    }

    /// Centralized helper to build a file tree ListItem row with consistent styling and slots.
    fn build_file_tree_row(
        &self,
        theme: &Theme,
        entry: &FileTreeEntry,
        is_selected: bool,
        indentation: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let ft_tokens = theme.tokens.file_tree_tokens();
        ListItem::new(("file-tree-entry", entry.id.0))
            .variant(ListItemVariant::Ghost)
            .spacing(ListItemSpacing::Compact)
            .selected(is_selected)
            .class("file-tree-entry")
            .with_listener(move |item| {
                let mut item = item.w_full().pl(indentation).pr(px(8.0)).h(px(24.0));
                if is_selected {
                    item = item.bg(ft_tokens.item_background_selected);
                } else {
                    item = item.hover(|s| s.bg(ft_tokens.background));
                }
                item
            })
            .start_slot(
                // Reserve space for chevron to align icons
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
            .child(
                // Icon + filename content
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(self.render_icon_with_vcs_status(entry, is_selected, cx))
                    .child(self.render_filename_with_selection(entry, is_selected, cx)),
            )
            .into_any_element()
    }

    /// Render the chevron for directories using design tokens
    fn render_chevron(&self, entry: &FileTreeEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let file_tree_tokens = theme.tokens.file_tree_tokens();

        chevron_icon(if entry.is_expanded { "down" } else { "right" })
            .size_3()
            .text_color(file_tree_tokens.item_text_secondary) // Use computed chrome text color
    }

    /// Render the file/directory icon with VCS status overlay using VcsIcon component
    fn render_icon_with_vcs_status(
        &self,
        entry: &FileTreeEntry,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let selected_text = theme.tokens.editor.text_on_primary;
        let normal_text = theme.text;
        let icon_color = if is_selected {
            selected_text
        } else {
            normal_text
        };

        // Create the appropriate VcsIcon based on the entry type
        let vcs_icon = match &entry.kind {
            crate::file_tree::FileKind::Directory { .. } => VcsIcon::directory(entry.is_expanded)
                .size(16.0)
                .text_color(icon_color),
            crate::file_tree::FileKind::File { extension } => {
                VcsIcon::from_extension(extension.as_deref())
                    .size(16.0)
                    .text_color(icon_color)
            }
            crate::file_tree::FileKind::Symlink { target_exists, .. } => {
                VcsIcon::symlink(*target_exists)
                    .size(16.0)
                    .text_color(if *target_exists {
                        icon_color
                    } else {
                        theme.error
                    })
            }
        };

        // Add VCS status if available
        let vcs_icon_with_status =
            vcs_icon.vcs_status(self.get_vcs_status_for_entry(&entry.path, cx));

        // Use the VcsIconRenderer trait to render with proper theme context
        self.render_vcs_icon(vcs_icon_with_status, cx)
    }

    /// Render the filename using color theory text colors
    fn render_filename_with_selection(
        &self,
        entry: &FileTreeEntry,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let file_tree_tokens = theme.tokens.file_tree_tokens();
        let selected_text = theme.tokens.editor.text_on_primary;

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

        // Use computed chrome text colors for consistency with chrome background
        let mut node = div()
            .flex_1()
            .text_size(px(14.0)) // Use consistent text size
            .child(filename);

        if is_selected {
            node = node.text_color(selected_text);
        } else if entry.is_hidden {
            node = node.text_color(file_tree_tokens.item_text_secondary);
        } else {
            node = node.text_color(file_tree_tokens.item_text);
        }

        node
    }
}

impl EventEmitter<FileTreeEvent> for FileTreeView {}

impl Focusable for FileTreeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// FileTreeView is focusable through its focus_handle field

// FileTreeView uses nucleotide-ui design patterns without implementing traits
// The component is already well-structured with ListItem usage

impl Render for FileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Use nucleotide-ui theme access for consistent styling
        let theme = cx.theme();
        let entries = self.tree.visible_entries();

        // Use FileTreeTokens from hybrid color system for chrome background
        let file_tree_tokens = theme.tokens.file_tree_tokens();
        let bg_color = file_tree_tokens.background;

        // Create semantic file tree container with nucleotide-ui design tokens
        div()
            .id("file-tree")
            .key_context("FileTree")
            .w_full()
            .h_full()
            .bg(bg_color) // Use semantic background color from design tokens
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
                    if let Some(selected_path) = view.selected_path.clone()
                        && let Some(entry) = view.tree.entry_by_path(&selected_path)
                        && entry.is_directory()
                    {
                        view.toggle_directory(&selected_path, cx);
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
                        gpui::ParentElement::child,
                    ),
            )
    }
}
