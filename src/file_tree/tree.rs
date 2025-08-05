// ABOUTME: Core file tree data structure using SumTree for efficient operations
// ABOUTME: Manages file system hierarchy with support for lazy loading and filtering

use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::SystemTime;
use anyhow::{Result, Context};
use sum_tree::SumTree;

use crate::file_tree::{
    FileTreeEntry, FileKind, FileTreeSummary, FileTreeConfig
};
use crate::file_tree::entry::FileTreeEntryId;
use crate::file_tree::summary::{Count, VisibleCount};

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
}

impl FileTree {
    /// Create a new file tree for the given root path
    pub fn new(root_path: PathBuf, config: FileTreeConfig) -> Self {
        Self {
            root_path,
            entries: SumTree::new(&()),
            path_to_id: HashMap::new(),
            expanded_dirs: HashSet::new(),
            config,
            next_id: 1,
            is_loaded: false,
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

        let entries = self.scan_directory(&self.root_path.clone(), 0, self.config.initial_depth)?;
        self.entries = SumTree::from_iter(entries, &());
        self.is_loaded = true;

        Ok(())
    }

    /// Refresh the entire tree
    pub fn refresh(&mut self) -> Result<()> {
        self.entries = SumTree::new(&());
        self.path_to_id.clear();
        self.next_id = 1;
        self.is_loaded = false;
        self.load()
    }

    /// Get all visible entries
    pub fn visible_entries(&self) -> Vec<FileTreeEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.is_visible)
            .cloned()
            .collect()
    }

    /// Get entry by path
    pub fn entry_by_path(&self, path: &Path) -> Option<FileTreeEntry> {
        let id = self.path_to_id.get(path)?;
        self.entry_by_id(*id)
    }

    /// Get entry by ID
    pub fn entry_by_id(&self, id: FileTreeEntryId) -> Option<FileTreeEntry> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
    }

    /// Toggle directory expansion
    pub fn toggle_directory(&mut self, path: &Path) -> Result<bool> {
        let mut entry = self.entry_by_path(path)
            .context("Entry not found")?;

        if !entry.is_directory() {
            anyhow::bail!("Not a directory");
        }

        let is_expanded = self.expanded_dirs.contains(path);
        
        if is_expanded {
            // Collapse directory
            self.expanded_dirs.remove(path);
            entry.is_expanded = false;
            self.upsert_entry(entry);
            self.hide_children(path);
        } else {
            // Expand directory
            self.expanded_dirs.insert(path.to_path_buf());
            entry.is_expanded = true;
            self.upsert_entry(entry);
            self.load_directory(path)?;
        }

        Ok(!is_expanded)
    }

    /// Check if a directory is expanded
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded_dirs.contains(path)
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
        // Remove existing entry if it exists
        if let Some(existing_id) = self.path_to_id.get(&entry.path) {
            self.remove_entry_by_id(*existing_id);
        }

        // Add new entry
        self.path_to_id.insert(entry.path.clone(), entry.id);
        
        // Insert into SumTree (rebuild for now - could be optimized)
        let mut entries: Vec<_> = self.entries.iter().cloned().collect();
        entries.push(entry);
        entries.sort();
        self.entries = SumTree::from_iter(entries, &());
    }

    /// Remove an entry by path
    pub fn remove_entry(&mut self, path: &Path) -> Option<FileTreeEntry> {
        let id = self.path_to_id.remove(path)?;
        self.remove_entry_by_id(id)
    }

    /// Generate next entry ID
    fn next_entry_id(&mut self) -> FileTreeEntryId {
        let id = FileTreeEntryId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Scan a directory recursively
    fn scan_directory(&mut self, dir_path: &Path, current_depth: usize, max_depth: usize) -> Result<Vec<FileTreeEntry>> {
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
                        entries.extend(children);
                        
                        // Update child count
                        if let FileKind::Directory { ref mut child_count, ref mut is_loaded } = dir_entry.kind {
                            *child_count = entries.len();
                            *is_loaded = true;
                        }
                    }
                }
                
                dir_entry
            } else if metadata.is_file() {
                let mut file_entry = FileTreeEntry::new_file(id, path.clone(), metadata.len(), mtime);
                file_entry.depth = current_depth;
                file_entry
            } else {
                // Handle symlinks
                let target = fs::read_link(&path).ok();
                let target_exists = target.as_ref()
                    .map(|t| t.exists())
                    .unwrap_or(false);
                let mut symlink_entry = FileTreeEntry::new_symlink(id, path.clone(), target, target_exists, mtime);
                symlink_entry.depth = current_depth;
                symlink_entry
            };

            // Set visibility based on current filter settings
            file_entry.is_visible = self.should_be_visible(&file_entry);
            
            self.path_to_id.insert(path, id);
            entries.push(file_entry);
        }

        entries.sort();
        Ok(entries)
    }

    /// Load a specific directory (for lazy loading)
    fn load_directory(&mut self, dir_path: &Path) -> Result<()> {
        if let Some(mut entry) = self.entry_by_path(dir_path) {
            let children = self.scan_directory(dir_path, entry.depth + 1, entry.depth + 2)?;
            
            // Update the directory entry
            if let FileKind::Directory { ref mut child_count, ref mut is_loaded } = entry.kind {
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

    /// Check if a file is ignored (simple implementation for now)
    fn is_ignored_file(&self, _path: &Path) -> bool {
        // TODO: Implement proper gitignore checking using the ignore crate
        false
    }
}

/// Statistics about the file tree
#[derive(Debug, Clone)]
pub struct FileTreeStats {
    pub total_entries: usize,
    pub visible_entries: usize,
    pub file_count: usize,
    pub directory_count: usize,
    pub total_size: u64,
    pub max_depth: usize,
}