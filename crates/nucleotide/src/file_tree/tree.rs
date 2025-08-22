// ABOUTME: Core file tree data structure using SumTree for efficient operations
// ABOUTME: Manages file system hierarchy with support for lazy loading and filtering

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use sum_tree::SumTree;

use crate::file_tree::entry::FileTreeEntryId;
use crate::file_tree::{FileKind, FileTreeConfig, FileTreeEntry};

/// Core file tree data structure
pub struct FileTree {
    /// Root directory path
    root_path: PathBuf,
    /// SumTree containing all entries
    entries: SumTree<FileTreeEntry>,
    /// Map from path to entry ID
    path_to_id: HashMap<PathBuf, FileTreeEntryId>,
    /// Set of expanded directory paths
    expanded_dirs: HashSet<PathBuf>,
    /// Configuration
    config: FileTreeConfig,
    /// Next available entry ID
    next_id: u64,
    /// Whether the tree has been initially loaded
    is_loaded: bool,
    /// Cache of visible entries to avoid recomputing
    visible_entries_cache: Option<Vec<FileTreeEntry>>,
    /// Set of directories currently being loaded
    loading_dirs: HashSet<PathBuf>,
    /// Gitignore matcher for filtering files
    gitignore: Option<Gitignore>,
}

impl FileTree {
    /// Create a new file tree for the given root path
    pub fn new(root_path: PathBuf, config: FileTreeConfig) -> Self {
        // Build gitignore matcher using the same patterns as the file picker
        let gitignore = Self::build_gitignore_matcher(&root_path);

        Self {
            root_path,
            entries: SumTree::new(&()),
            path_to_id: HashMap::new(),
            expanded_dirs: HashSet::new(),
            config,
            next_id: 1,
            is_loaded: false,
            visible_entries_cache: None,
            loading_dirs: HashSet::new(),
            gitignore,
        }
    }

    /// Get the root path
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Get the configuration
    pub fn config(&self) -> &FileTreeConfig {
        &self.config
    }

    /// Update the configuration
    pub fn set_config(&mut self, config: FileTreeConfig) {
        self.config = config;
        // Refresh visibility based on new config
        self.refresh_visibility();
    }

    /// Load the initial file tree
    pub fn load(&mut self) -> Result<()> {
        if self.is_loaded {
            return Ok(());
        }

        // Only load immediate children of root initially
        let entries = self.scan_directory(&self.root_path.clone(), 1, 1)?;

        // Mark root as expanded since we're loading its children
        self.expanded_dirs.insert(self.root_path.clone());

        self.entries = SumTree::from_iter(entries, &());
        self.is_loaded = true;

        // Don't refresh VCS status here - it needs to be done after Tokio runtime is available

        self.invalidate_cache();

        Ok(())
    }

    /// Refresh the entire tree
    pub fn refresh(&mut self) -> Result<()> {
        self.entries = SumTree::new(&());
        self.path_to_id.clear();
        self.next_id = 1;
        self.is_loaded = false;
        // VCS status is now handled by the global VCS service
        self.invalidate_cache();
        self.load()
    }

    /// Get all visible entries
    pub fn visible_entries(&mut self) -> Vec<FileTreeEntry> {
        if let Some(ref cache) = self.visible_entries_cache {
            return cache.clone();
        }

        // Get all visible entries from the tree
        let entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.is_visible)
            .cloned()
            .collect();

        // Create the root entry
        let mut result = Vec::with_capacity(entries.len() + 1);

        // Add the root directory as the first entry
        let root_id = FileTreeEntryId(0); // Special ID for root
        let mut root_entry = FileTreeEntry::new_directory(root_id, self.root_path.clone(), None);
        root_entry.depth = 0;
        root_entry.is_expanded = self.is_expanded(&self.root_path);

        // Count direct children
        let children_count = entries
            .iter()
            .filter(|e| e.path.parent() == Some(&self.root_path))
            .count();

        if let FileKind::Directory {
            ref mut child_count,
            ref mut is_loaded,
        } = root_entry.kind
        {
            *child_count = children_count;
            *is_loaded = true;
        }

        result.push(root_entry);

        // Build sorted tree starting from root only if root is expanded
        if self.is_expanded(&self.root_path) {
            // Adjust depth for all entries
            let adjusted_entries: Vec<FileTreeEntry> = entries
                .iter()
                .map(|e| {
                    let mut entry = e.clone();
                    entry.depth += 1;
                    entry
                })
                .collect();

            self.build_sorted_tree(&adjusted_entries, &self.root_path, &mut result);
        }

        self.visible_entries_cache = Some(result.clone());
        result
    }

    /// Build sorted tree with directories first at each level
    fn build_sorted_tree(
        &self,
        entries: &[FileTreeEntry],
        parent_path: &Path,
        result: &mut Vec<FileTreeEntry>,
    ) {
        // Find all immediate children of parent_path
        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in entries {
            if let Some(entry_parent) = entry.path.parent() {
                if entry_parent == parent_path {
                    if entry.is_directory() {
                        dirs.push(entry.clone());
                    } else {
                        files.push(entry.clone());
                    }
                }
            }
        }

        // Sort directories and files by name
        dirs.sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));
        files.sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));

        // Add directories first, recursively processing their children
        for dir in dirs {
            result.push(dir.clone());
            if self.is_expanded(&dir.path) {
                self.build_sorted_tree(entries, &dir.path, result);
            }
        }

        // Then add files
        result.extend(files);
    }

    /// Invalidate the visible entries cache
    fn invalidate_cache(&mut self) {
        self.visible_entries_cache = None;
    }

    /// Get entry by path
    pub fn entry_by_path(&self, path: &Path) -> Option<FileTreeEntry> {
        // Special case for root path
        if path == self.root_path {
            return Some(FileTreeEntry {
                id: FileTreeEntryId(0),
                path: self.root_path.clone(),
                kind: crate::file_tree::FileKind::Directory {
                    child_count: self
                        .entries
                        .iter()
                        .filter(|e| e.path.parent() == Some(&self.root_path))
                        .count(),
                    is_loaded: true,
                },
                size: 0,
                mtime: None,
                depth: 0,
                is_expanded: self.is_expanded(&self.root_path),
                is_visible: true,
                is_hidden: false,
                is_ignored: false,
                git_status: None,
            });
        }

        let id = self.path_to_id.get(path)?;
        self.entry_by_id(*id)
    }

    /// Get entry by ID
    pub fn entry_by_id(&self, id: FileTreeEntryId) -> Option<FileTreeEntry> {
        self.entries.iter().find(|entry| entry.id == id).cloned()
    }

    /// Toggle directory expansion
    pub fn toggle_directory(&mut self, path: &Path) -> Result<bool> {
        let entry = self.entry_by_path(path).context("Entry not found")?;

        if !entry.is_directory() {
            anyhow::bail!("Not a directory");
        }

        let is_expanded = self.expanded_dirs.contains(path);

        if is_expanded {
            // Collapse directory
            self.expanded_dirs.remove(path);

            // For root directory, we don't need to update the entry in the tree
            if path != self.root_path {
                let mut entry = entry;
                entry.is_expanded = false;
                self.upsert_entry(entry);
            }

            self.hide_children(path);
            self.invalidate_cache();
        } else {
            // Expand directory
            self.expanded_dirs.insert(path.to_path_buf());

            // For root directory, we don't need to update the entry in the tree
            if path != self.root_path {
                let mut entry = entry;
                entry.is_expanded = true;
                self.upsert_entry(entry);
            }

            self.load_directory(path)?;
            self.invalidate_cache();
        }

        Ok(!is_expanded)
    }

    /// Check if a directory is expanded
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded_dirs.contains(path)
    }

    /// Check if a directory is currently being loaded
    pub fn is_directory_loading(&self, path: &Path) -> bool {
        self.loading_dirs.contains(path)
    }

    /// Mark a directory as being loaded
    pub fn mark_directory_loading(&mut self, path: &Path) {
        self.loading_dirs.insert(path.to_path_buf());
    }

    /// Unmark a directory as being loaded
    pub fn unmark_directory_loading(&mut self, path: &Path) {
        self.loading_dirs.remove(path);
    }

    /// Collapse a directory (synchronous)
    pub fn collapse_directory(&mut self, path: &Path) -> Result<()> {
        let mut entry = self.entry_by_path(path).context("Entry not found")?;

        if !entry.is_directory() {
            anyhow::bail!("Not a directory");
        }

        self.expanded_dirs.remove(path);
        entry.is_expanded = false;
        self.upsert_entry(entry);
        self.hide_children(path);
        self.invalidate_cache();

        Ok(())
    }

    /// Expand a directory with pre-loaded entries
    pub fn expand_directory_with_entries(
        &mut self,
        path: &Path,
        entries: Vec<(PathBuf, std::fs::Metadata)>,
    ) -> Result<()> {
        let mut entry = self.entry_by_path(path).context("Entry not found")?;

        if !entry.is_directory() {
            anyhow::bail!("Not a directory");
        }

        // Mark as expanded
        self.expanded_dirs.insert(path.to_path_buf());
        entry.is_expanded = true;

        // Process the entries
        let mut children = Vec::new();
        for (child_path, metadata) in entries {
            // Skip hidden files unless configured
            if !self.config.show_hidden && self.is_hidden_file(&child_path) {
                continue;
            }

            let id = self.next_entry_id();
            let mtime = metadata.modified().ok();

            let mut child_entry = if metadata.is_dir() {
                FileTreeEntry::new_directory(id, child_path.clone(), mtime)
            } else if metadata.is_file() {
                FileTreeEntry::new_file(id, child_path.clone(), metadata.len(), mtime)
            } else {
                // Symlink
                let target = std::fs::read_link(&child_path).ok();
                let target_exists = target.as_ref().map(|t| t.exists()).unwrap_or(false);
                FileTreeEntry::new_symlink(id, child_path.clone(), target, target_exists, mtime)
            };

            child_entry.depth = entry.depth + 1;
            child_entry.is_visible = true;

            // VCS status will be queried at render time via the global VCS service

            self.path_to_id.insert(child_path, id);
            children.push(child_entry);
        }

        // Update directory entry
        if let FileKind::Directory {
            ref mut child_count,
            ref mut is_loaded,
        } = entry.kind
        {
            *child_count = children.len();
            *is_loaded = true;
        }

        // Sort children before adding
        children.sort();

        // Add all entries at once (parent + children)
        let mut all_entries = vec![entry];
        all_entries.extend(children);
        self.upsert_entries(all_entries);

        // Don't refresh VCS status here - it will be done asynchronously

        // Remove from loading set
        self.loading_dirs.remove(path);

        Ok(())
    }

    /// Get the total number of entries
    pub fn total_count(&self) -> usize {
        self.entries.summary().count
    }

    /// Get the number of visible entries
    pub fn visible_count(&self) -> usize {
        self.entries.summary().visible_count
    }

    /// Get file statistics
    pub fn stats(&self) -> FileTreeStats {
        let summary = self.entries.summary();
        FileTreeStats {
            total_entries: summary.count,
            visible_entries: summary.visible_count,
            file_count: summary.file_count,
            directory_count: summary.directory_count,
            total_size: summary.total_size,
            max_depth: summary.max_depth,
        }
    }

    /// Add or update an entry
    pub fn upsert_entry(&mut self, entry: FileTreeEntry) {
        self.upsert_entries(vec![entry]);
    }

    /// Add or update multiple entries at once
    pub fn upsert_entries(&mut self, new_entries: Vec<FileTreeEntry>) {
        // Remove existing entries if they exist
        for entry in &new_entries {
            if let Some(existing_id) = self.path_to_id.get(&entry.path) {
                self.remove_entry_by_id(*existing_id);
            }
        }

        // Collect all existing entries
        let mut entries: Vec<_> = self.entries.iter().cloned().collect();

        // Add new entries and update path_to_id map
        for entry in new_entries {
            self.path_to_id.insert(entry.path.clone(), entry.id);
            entries.push(entry);
        }

        // Sort all entries - the Ord implementation ensures parents come before children
        entries.sort();
        self.entries = SumTree::from_iter(entries, &());
        self.invalidate_cache();
    }

    /// Remove an entry by path
    pub fn remove_entry(&mut self, path: &Path) -> Option<FileTreeEntry> {
        let id = self.path_to_id.remove(path)?;
        self.remove_entry_by_id(id)
    }

    /// Find the parent entry of a given path in the visible entries
    pub fn find_parent_entry(&mut self, path: &Path) -> Option<FileTreeEntry> {
        let parent_path = path.parent()?;

        // Clone root_path to avoid borrowing issues
        let root_path = self.root_path.clone();

        // Don't go above the root directory
        if parent_path < root_path {
            return None;
        }

        // Use the root path itself if the parent would be outside the tree
        let target_path = if parent_path == root_path {
            root_path
        } else {
            parent_path.to_path_buf()
        };

        // Find the parent in visible entries
        let visible_entries = self.visible_entries();
        visible_entries
            .into_iter()
            .find(|entry| entry.path == target_path)
    }

    /// Find the first child entry of a directory in the visible entries
    pub fn find_first_child_entry(&mut self, dir_path: &Path) -> Option<FileTreeEntry> {
        // Only works for directories
        if let Some(entry) = self.entry_by_path(dir_path) {
            if !entry.is_directory() {
                return None;
            }
        } else {
            return None;
        }

        // Find the first child in visible entries
        let visible_entries = self.visible_entries();
        visible_entries.into_iter().find(|entry| {
            if let Some(parent) = entry.path.parent() {
                parent == dir_path
            } else {
                false
            }
        })
    }

    /// Generate next entry ID
    pub fn next_entry_id(&mut self) -> FileTreeEntryId {
        let id = FileTreeEntryId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Scan a directory recursively
    fn scan_directory(
        &mut self,
        dir_path: &Path,
        current_depth: usize,
        max_depth: usize,
    ) -> Result<Vec<FileTreeEntry>> {
        let mut entries = Vec::new();

        let read_dir = fs::read_dir(dir_path)
            .with_context(|| format!("Failed to read directory: {}", dir_path.display()))?;

        for entry in read_dir {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;
            let mtime = metadata.modified().ok();

            // Skip hidden files unless configured to show them
            if !self.config.show_hidden && self.is_hidden_file(&path) {
                continue;
            }

            // Skip ignored files unless configured to show them
            if !self.config.show_ignored && self.is_ignored_file(&path) {
                continue;
            }

            let id = self.next_entry_id();
            let mut file_entry = if metadata.is_dir() {
                let mut dir_entry = FileTreeEntry::new_directory(id, path.clone(), mtime);
                dir_entry.depth = current_depth;

                // Recursively scan subdirectories if within depth limit
                if current_depth < max_depth {
                    if let Ok(children) = self.scan_directory(&path, current_depth + 1, max_depth) {
                        // Update child count with actual number of children
                        if let FileKind::Directory {
                            ref mut child_count,
                            ref mut is_loaded,
                        } = dir_entry.kind
                        {
                            *child_count = children.len();
                            *is_loaded = true;
                        }
                        entries.extend(children);
                    }
                }

                dir_entry
            } else if metadata.is_file() {
                let mut file_entry =
                    FileTreeEntry::new_file(id, path.clone(), metadata.len(), mtime);
                file_entry.depth = current_depth;
                file_entry
            } else {
                // Handle symlinks
                let target = fs::read_link(&path).ok();
                let target_exists = target.as_ref().map(|t| t.exists()).unwrap_or(false);
                let mut symlink_entry =
                    FileTreeEntry::new_symlink(id, path.clone(), target, target_exists, mtime);
                symlink_entry.depth = current_depth;
                symlink_entry
            };

            // Set visibility based on current filter settings
            file_entry.is_visible = self.should_be_visible(&file_entry);

            // VCS status will be queried at render time via the global VCS service

            self.path_to_id.insert(path, id);
            entries.push(file_entry);
        }

        Ok(entries)
    }

    /// Load a specific directory (for lazy loading)
    fn load_directory(&mut self, dir_path: &Path) -> Result<()> {
        if let Some(mut entry) = self.entry_by_path(dir_path) {
            // Check if already loaded
            if let FileKind::Directory { is_loaded, .. } = &entry.kind {
                if *is_loaded {
                    // Directory already loaded, nothing to do
                    return Ok(());
                }
            }

            let children = self.scan_directory(dir_path, entry.depth + 1, entry.depth + 2)?;

            // Update the directory entry
            if let FileKind::Directory {
                ref mut child_count,
                ref mut is_loaded,
            } = entry.kind
            {
                *child_count = children.len();
                *is_loaded = true;
            }

            // Add children to tree
            for child in children {
                self.upsert_entry(child);
            }

            // Update the directory entry
            self.upsert_entry(entry);
        }

        Ok(())
    }

    /// Hide children of a directory
    fn hide_children(&mut self, dir_path: &Path) {
        let entries: Vec<_> = self.entries.iter().cloned().collect();
        let mut updated_entries = Vec::new();

        for mut entry in entries {
            if entry.path.starts_with(dir_path) && entry.path != dir_path {
                entry.is_visible = false;
            }
            updated_entries.push(entry);
        }

        self.entries = SumTree::from_iter(updated_entries, &());
        self.invalidate_cache();
    }

    /// Remove entry by ID
    fn remove_entry_by_id(&mut self, id: FileTreeEntryId) -> Option<FileTreeEntry> {
        let entries: Vec<_> = self.entries.iter().cloned().collect();
        let mut updated_entries = Vec::new();
        let mut removed_entry = None;

        for entry in entries {
            if entry.id == id {
                removed_entry = Some(entry);
            } else {
                updated_entries.push(entry);
            }
        }

        if removed_entry.is_some() {
            self.entries = SumTree::from_iter(updated_entries, &());
            self.invalidate_cache();
        }

        removed_entry
    }

    /// Refresh visibility of all entries
    fn refresh_visibility(&mut self) {
        let entries: Vec<_> = self.entries.iter().cloned().collect();
        let mut updated_entries = Vec::new();

        for mut entry in entries {
            entry.is_visible = self.should_be_visible(&entry);
            updated_entries.push(entry);
        }

        self.entries = SumTree::from_iter(updated_entries, &());
        self.invalidate_cache();
    }

    /// Check if an entry should be visible based on current configuration
    fn should_be_visible(&self, entry: &FileTreeEntry) -> bool {
        // Hidden files
        if entry.is_hidden && !self.config.show_hidden {
            return false;
        }

        // Ignored files
        if entry.is_ignored && !self.config.show_ignored {
            return false;
        }

        // Check if parent directory is expanded
        if let Some(parent) = entry.path.parent() {
            if parent != self.root_path && !self.is_expanded(parent) {
                return false;
            }
        }

        true
    }

    /// Check if a file is hidden (starts with .)
    fn is_hidden_file(&self, path: &Path) -> bool {
        path.file_name()
            .map(|name| name.to_string_lossy().starts_with('.'))
            .unwrap_or(false)
    }

    /// Build gitignore matcher using the same patterns as the file picker
    fn build_gitignore_matcher(root_path: &Path) -> Option<Gitignore> {
        let mut builder = GitignoreBuilder::new(root_path);

        // Add .gitignore files
        if let Ok(gitignore_path) = root_path.join(".gitignore").canonicalize() {
            if gitignore_path.exists() {
                let _ = builder.add(&gitignore_path);
            }
        }

        // Add global gitignore
        if let Some(git_config_dir) = dirs::config_dir() {
            let global_gitignore = git_config_dir.join("git").join("ignore");
            if global_gitignore.exists() {
                let _ = builder.add(&global_gitignore);
            }
        }

        // Add .git/info/exclude
        let git_exclude = root_path.join(".git").join("info").join("exclude");
        if git_exclude.exists() {
            let _ = builder.add(&git_exclude);
        }

        // Add .ignore files
        let ignore_file = root_path.join(".ignore");
        if ignore_file.exists() {
            let _ = builder.add(&ignore_file);
        }

        // Add Helix-specific ignore files
        let helix_ignore = root_path.join(".helix").join("ignore");
        if helix_ignore.exists() {
            let _ = builder.add(&helix_ignore);
        }

        builder.build().ok()
    }

    /// Check if a file is ignored using the same patterns as the file picker
    fn is_ignored_file(&self, path: &Path) -> bool {
        // Check if path is inside VCS directories
        for component in path.components() {
            if let std::path::Component::Normal(name) = component {
                if let Some(name_str) = name.to_str() {
                    match name_str {
                        ".git" | ".svn" | ".hg" | ".bzr" => return true,
                        _ => {}
                    }
                }
            }
        }

        // Check gitignore patterns
        if let Some(ref gitignore) = self.gitignore {
            if let Ok(relative_path) = path.strip_prefix(&self.root_path) {
                let matched = gitignore.matched(relative_path, path.is_dir());
                return matched.is_ignore();
            }
        }

        false
    }

    // VCS status is now handled by the global VCS service
    // The view layer queries VCS status at render time via get_vcs_status_for_entry
}

/// Statistics about the file tree
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileTreeStats {
    pub total_entries: usize,
    pub visible_entries: usize,
    pub file_count: usize,
    pub directory_count: usize,
    pub total_size: u64,
    pub max_depth: usize,
}
