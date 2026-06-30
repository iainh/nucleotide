// ABOUTME: Path-first file tree model inspired by @pierre/trees
// ABOUTME: Derives visible rows from canonical paths, expansion, search, and flattening

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::file_tree::entry::{FileTreeEntryId, FileTreeFlattenedSegment};
use crate::file_tree::{
    FileKind, FileTreeCollisionStrategy, FileTreeConfig, FileTreeEntry, FileTreeSearchMode,
};

#[derive(Debug, Clone, PartialEq)]
pub enum FileTreeDirectoryEntryKind {
    File,
    Directory,
    Symlink {
        target: Option<PathBuf>,
        target_exists: bool,
    },
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileTreeDirectoryEntry {
    pub path: PathBuf,
    pub kind: FileTreeDirectoryEntryKind,
    pub size: u64,
    pub mtime: Option<SystemTime>,
}

impl FileTreeDirectoryEntry {
    pub fn from_metadata(path: PathBuf, metadata: std::fs::Metadata) -> Self {
        let kind = if metadata.is_dir() {
            FileTreeDirectoryEntryKind::Directory
        } else if metadata.is_file() {
            FileTreeDirectoryEntryKind::File
        } else {
            let target = fs::read_link(&path).ok();
            let target_exists = target
                .as_ref()
                .map(|target| target.exists())
                .unwrap_or(false);
            FileTreeDirectoryEntryKind::Symlink {
                target,
                target_exists,
            }
        };

        Self {
            path,
            kind,
            size: metadata.len(),
            mtime: metadata.modified().ok(),
        }
    }
}

/// Core file tree data structure.
pub struct FileTree {
    /// Root directory path.
    root_path: PathBuf,
    /// Canonical path keyed entries. Public identity is the path, not this map's storage.
    entries: HashMap<PathBuf, FileTreeEntry>,
    /// Map from canonical path to entry ID.
    path_to_id: HashMap<PathBuf, FileTreeEntryId>,
    /// Set of expanded directory paths.
    expanded_dirs: HashSet<PathBuf>,
    /// Configuration.
    config: FileTreeConfig,
    /// Next available entry ID.
    next_id: u64,
    /// Whether the tree has been initially loaded.
    is_loaded: bool,
    /// Cache of visible entries to avoid recomputing.
    visible_entries_cache: Option<Arc<[FileTreeEntry]>>,
    /// Set of directories currently being loaded.
    loading_dirs: HashSet<PathBuf>,
    /// Gitignore matcher for filtering files.
    gitignore: Option<Gitignore>,
    /// Normalized search query.
    search_query: Option<String>,
}

impl FileTree {
    /// Create a new file tree for the given root path.
    pub fn new(root_path: PathBuf, config: FileTreeConfig) -> Self {
        let root_path = normalize_tree_path(&root_path);
        let gitignore = Self::build_gitignore_matcher(&root_path);

        Self {
            root_path,
            entries: HashMap::new(),
            path_to_id: HashMap::new(),
            expanded_dirs: HashSet::new(),
            config,
            next_id: 1,
            is_loaded: false,
            visible_entries_cache: None,
            loading_dirs: HashSet::new(),
            gitignore,
            search_query: None,
        }
    }

    /// Get the root path.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Get the configuration.
    pub fn config(&self) -> &FileTreeConfig {
        &self.config
    }

    /// Update the configuration.
    pub fn set_config(&mut self, config: FileTreeConfig) {
        self.config = config;
        self.invalidate_cache();
    }

    /// Set the current search query.
    pub fn set_search_query(&mut self, query: Option<String>) {
        let normalized = query
            .as_deref()
            .map(normalize_search_query)
            .filter(|query| !query.is_empty());

        if self.search_query != normalized {
            self.search_query = normalized;
            self.invalidate_cache();
        }
    }

    /// Get the current normalized search query.
    pub fn search_query(&self) -> Option<&str> {
        self.search_query.as_deref()
    }

    /// Clear the current search query.
    pub fn clear_search_query(&mut self) {
        self.set_search_query(None);
    }

    /// Return the canonical paths that directly match the current search query.
    pub fn search_matching_paths(&self) -> Vec<PathBuf> {
        let Some(query) = self.search_query.as_deref() else {
            return Vec::new();
        };

        let mut matches: Vec<_> = self
            .entries
            .values()
            .filter(|entry| self.entry_matches_search(entry, query))
            .map(|entry| entry.path.clone())
            .collect();
        matches.sort();
        matches
    }

    /// Load the initial file tree.
    pub fn load(&mut self) -> Result<()> {
        if self.is_loaded {
            return Ok(());
        }

        let root_path = self.root_path.clone();
        let max_depth = self.config.initial_depth.max(1);
        let (mut entries, _) = self.scan_directory_recursive(&root_path, 1, max_depth)?;
        let directory_child_parents = directory_child_parent_paths(&root_path, &entries);

        self.entries.clear();
        self.path_to_id.clear();
        self.expanded_dirs.insert(root_path.clone());
        for entry in &mut entries {
            if entry.is_directory() && directory_child_parents.contains(&entry.path) {
                entry.is_expanded = true;
                self.expanded_dirs.insert(entry.path.clone());
            }
        }
        self.insert_entries(entries);
        self.is_loaded = true;
        self.invalidate_cache();

        Ok(())
    }

    pub fn load_root_only(&mut self) {
        self.entries.clear();
        self.path_to_id.clear();
        self.expanded_dirs.clear();
        self.loading_dirs.clear();
        self.next_id = 1;
        self.expanded_dirs.insert(self.root_path.clone());
        self.is_loaded = true;
        self.invalidate_cache();
    }

    /// Refresh the entire tree.
    pub fn refresh(&mut self) -> Result<()> {
        self.entries.clear();
        self.path_to_id.clear();
        self.expanded_dirs.clear();
        self.loading_dirs.clear();
        self.next_id = 1;
        self.is_loaded = false;
        self.invalidate_cache();
        self.load()
    }

    /// Get all visible entries.
    pub fn visible_entries(&mut self) -> Arc<[FileTreeEntry]> {
        if let Some(ref cache) = self.visible_entries_cache {
            return cache.clone();
        }

        let result = Arc::<[FileTreeEntry]>::from(self.collect_visible_entries());
        self.visible_entries_cache = Some(result.clone());
        result
    }

    /// Get a snapshot of visible entries without mutating the cache.
    pub fn visible_entries_snapshot(&self) -> Arc<[FileTreeEntry]> {
        if let Some(ref cache) = self.visible_entries_cache {
            return cache.clone();
        }

        Arc::<[FileTreeEntry]>::from(self.collect_visible_entries())
    }

    fn collect_visible_entries(&self) -> Vec<FileTreeEntry> {
        let entries_by_parent = self.entries_by_parent();
        let matching_paths: HashSet<PathBuf> = self.search_matching_paths().into_iter().collect();
        let search_active = self.search_query.is_some();
        let included_paths = if search_active {
            Some(self.search_projection_paths(&matching_paths))
        } else {
            None
        };

        let mut result = Vec::new();
        let mut root_entry = self.root_entry(&entries_by_parent);
        root_entry.is_search_match = self
            .search_query
            .as_deref()
            .is_some_and(|query| self.entry_matches_search(&root_entry, query));
        result.push(root_entry);

        if !self.should_descend(&self.root_path, search_active, &matching_paths) {
            return result;
        }

        self.push_visible_children(
            &self.root_path,
            1,
            Vec::new(),
            &entries_by_parent,
            included_paths.as_ref(),
            &matching_paths,
            search_active,
            &mut result,
        );

        result
    }

    fn root_entry(&self, entries_by_parent: &HashMap<PathBuf, Vec<PathBuf>>) -> FileTreeEntry {
        let root_id = FileTreeEntryId(0);
        let mut root_entry = FileTreeEntry::new_directory(root_id, self.root_path.clone(), None);
        root_entry.depth = 0;
        root_entry.level = 1;
        root_entry.pos_in_set = 1;
        root_entry.set_size = 1;
        root_entry.is_expanded = self.is_expanded(&self.root_path);
        root_entry.is_visible = true;

        if let FileKind::Directory {
            ref mut child_count,
            ref mut is_loaded,
        } = root_entry.kind
        {
            *child_count = entries_by_parent.get(&self.root_path).map_or(0, Vec::len);
            *is_loaded = true;
        }

        root_entry
    }

    #[allow(clippy::too_many_arguments)]
    fn push_visible_children(
        &self,
        parent_path: &Path,
        depth: usize,
        ancestor_paths: Vec<PathBuf>,
        entries_by_parent: &HashMap<PathBuf, Vec<PathBuf>>,
        included_paths: Option<&HashSet<PathBuf>>,
        matching_paths: &HashSet<PathBuf>,
        search_active: bool,
        result: &mut Vec<FileTreeEntry>,
    ) {
        let Some(children) = entries_by_parent.get(parent_path) else {
            return;
        };

        let visible_children: Vec<PathBuf> = children
            .iter()
            .filter(|path| included_paths.is_none_or(|included| included.contains(*path)))
            .cloned()
            .collect();
        let set_size = visible_children.len();

        for (index, child_path) in visible_children.iter().enumerate() {
            let ProjectionRow {
                path,
                flattened_segments,
            } = self.project_child_row(child_path, entries_by_parent, included_paths);

            let Some(entry) = self.entries.get(&path) else {
                continue;
            };

            let mut row = entry.clone();
            row.depth = depth;
            row.level = depth + 1;
            row.pos_in_set = index + 1;
            row.set_size = set_size;
            row.ancestor_paths = Arc::<[PathBuf]>::from(ancestor_paths.clone());
            row.flattened_segments = flattened_segments;
            row.is_expanded = self.is_expanded(&row.path);
            row.is_visible = true;
            row.is_search_match = self.row_matches_search(&row, matching_paths);

            result.push(row.clone());

            if row.is_directory() && self.should_descend(&row.path, search_active, matching_paths) {
                let mut child_ancestors = ancestor_paths.clone();
                child_ancestors.push(row.path.clone());
                self.push_visible_children(
                    &row.path,
                    depth + 1,
                    child_ancestors,
                    entries_by_parent,
                    included_paths,
                    matching_paths,
                    search_active,
                    result,
                );
            }
        }
    }

    fn project_child_row(
        &self,
        child_path: &Path,
        entries_by_parent: &HashMap<PathBuf, Vec<PathBuf>>,
        included_paths: Option<&HashSet<PathBuf>>,
    ) -> ProjectionRow {
        if !self.config.flatten_empty_directories {
            return ProjectionRow {
                path: child_path.to_path_buf(),
                flattened_segments: None,
            };
        }

        let Some(entry) = self.entries.get(child_path) else {
            return ProjectionRow {
                path: child_path.to_path_buf(),
                flattened_segments: None,
            };
        };

        if !entry.is_directory() {
            return ProjectionRow {
                path: child_path.to_path_buf(),
                flattened_segments: None,
            };
        }

        let mut current_path = child_path.to_path_buf();
        let mut segments = vec![FileTreeFlattenedSegment {
            name: display_name_for_path(child_path),
            path: current_path.clone(),
            is_terminal: true,
        }];

        while let Some(children) = entries_by_parent.get(&current_path) {
            let visible_children: Vec<&PathBuf> = children
                .iter()
                .filter(|path| included_paths.is_none_or(|included| included.contains(*path)))
                .collect();

            if visible_children.len() != 1 {
                break;
            }

            let only_child = visible_children[0];
            let Some(child_entry) = self.entries.get(only_child) else {
                break;
            };

            if !child_entry.is_directory() {
                break;
            }

            if let Some(last) = segments.last_mut() {
                last.is_terminal = false;
            }
            current_path = only_child.clone();
            segments.push(FileTreeFlattenedSegment {
                name: display_name_for_path(only_child),
                path: current_path.clone(),
                is_terminal: true,
            });
        }

        ProjectionRow {
            path: current_path,
            flattened_segments: (segments.len() > 1)
                .then(|| Arc::<[FileTreeFlattenedSegment]>::from(segments)),
        }
    }

    fn entries_by_parent(&self) -> HashMap<PathBuf, Vec<PathBuf>> {
        let mut entries_by_parent: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

        for entry in self
            .entries
            .values()
            .filter(|entry| self.entry_is_included(entry))
        {
            if let Some(parent) = entry.path.parent() {
                entries_by_parent
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(entry.path.clone());
            }
        }

        for entries in entries_by_parent.values_mut() {
            entries.sort_by(|left, right| {
                let left = self.entries.get(left);
                let right = self.entries.get(right);
                match (left, right) {
                    (Some(left), Some(right)) => Self::compare_tree_entries(left, right),
                    _ => Ordering::Equal,
                }
            });
        }

        entries_by_parent
    }

    fn compare_tree_entries(a: &FileTreeEntry, b: &FileTreeEntry) -> Ordering {
        match (a.is_directory(), b.is_directory()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => display_name_for_path(&a.path)
                .to_lowercase()
                .cmp(&display_name_for_path(&b.path).to_lowercase())
                .then_with(|| a.path.cmp(&b.path)),
        }
    }

    fn should_descend(
        &self,
        path: &Path,
        search_active: bool,
        matching_paths: &HashSet<PathBuf>,
    ) -> bool {
        if search_active {
            return path == self.root_path
                || matching_paths.contains(path)
                || matching_paths
                    .iter()
                    .any(|matching_path| matching_path.starts_with(path));
        }

        self.is_expanded(path)
    }

    fn search_projection_paths(&self, matching_paths: &HashSet<PathBuf>) -> HashSet<PathBuf> {
        let mut included = HashSet::new();

        for path in matching_paths {
            included.insert(path.clone());
            self.insert_ancestors(path, &mut included);

            if matches!(self.config.search_mode, FileTreeSearchMode::ExpandMatches)
                && self
                    .entries
                    .get(path)
                    .is_some_and(FileTreeEntry::is_directory)
            {
                for descendant in self
                    .entries
                    .keys()
                    .filter(|candidate| candidate.starts_with(path))
                {
                    included.insert(descendant.clone());
                }
            }
        }

        if matches!(
            self.config.search_mode,
            FileTreeSearchMode::CollapseNonMatches
        ) {
            for expanded in &self.expanded_dirs {
                included.insert(expanded.clone());
                self.insert_ancestors(expanded, &mut included);
            }
        }

        included
    }

    fn insert_ancestors(&self, path: &Path, included: &mut HashSet<PathBuf>) {
        let mut current = path.parent();
        while let Some(parent) = current {
            if parent < self.root_path.as_path() {
                break;
            }

            if parent != self.root_path {
                included.insert(parent.to_path_buf());
            }

            if parent == self.root_path {
                break;
            }

            current = parent.parent();
        }
    }

    fn row_matches_search(&self, row: &FileTreeEntry, matching_paths: &HashSet<PathBuf>) -> bool {
        matching_paths.contains(&row.path)
            || row.flattened_segments.as_ref().is_some_and(|segments| {
                segments
                    .iter()
                    .any(|segment| matching_paths.contains(&segment.path))
            })
    }

    fn entry_matches_search(&self, entry: &FileTreeEntry, query: &str) -> bool {
        let name = display_name_for_path(&entry.path).to_lowercase();
        if name.contains(query) {
            return true;
        }

        let relative_path = entry
            .path
            .strip_prefix(&self.root_path)
            .unwrap_or(&entry.path)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/")
            .to_lowercase();

        relative_path.contains(query)
    }

    /// Invalidate the visible entries cache.
    fn invalidate_cache(&mut self) {
        self.visible_entries_cache = None;
    }

    /// Get entry by path.
    pub fn entry_by_path(&self, path: &Path) -> Option<FileTreeEntry> {
        let path = normalize_tree_path(path);
        if path == self.root_path {
            return Some(self.root_entry(&self.entries_by_parent()));
        }

        self.entries.get(&path).cloned()
    }

    /// Get entry by ID.
    pub fn entry_by_id(&self, id: FileTreeEntryId) -> Option<FileTreeEntry> {
        self.entries.values().find(|entry| entry.id == id).cloned()
    }

    /// Toggle directory expansion.
    pub fn toggle_directory(&mut self, path: &Path) -> Result<bool> {
        let path = normalize_tree_path(path);
        let entry = self.entry_by_path(&path).context("Entry not found")?;

        if !entry.is_directory() {
            anyhow::bail!("Not a directory");
        }

        let was_expanded = self.expanded_dirs.contains(&path);
        if was_expanded {
            self.expanded_dirs.remove(&path);
        } else {
            self.expanded_dirs.insert(path.clone());
            self.load_directory(&path)?;
        }

        self.invalidate_cache();
        Ok(!was_expanded)
    }

    /// Check if a directory is expanded.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded_dirs.contains(&normalize_tree_path(path))
    }

    /// Check if a directory is currently being loaded.
    pub fn is_directory_loading(&self, path: &Path) -> bool {
        self.loading_dirs.contains(&normalize_tree_path(path))
    }

    /// Mark a directory as being loaded.
    pub fn mark_directory_loading(&mut self, path: &Path) {
        self.loading_dirs.insert(normalize_tree_path(path));
    }

    /// Unmark a directory as being loaded.
    pub fn unmark_directory_loading(&mut self, path: &Path) {
        self.loading_dirs.remove(&normalize_tree_path(path));
    }

    /// Collapse a directory.
    pub fn collapse_directory(&mut self, path: &Path) -> Result<()> {
        let path = normalize_tree_path(path);
        let entry = self.entry_by_path(&path).context("Entry not found")?;

        if !entry.is_directory() {
            anyhow::bail!("Not a directory");
        }

        self.expanded_dirs.remove(&path);
        self.invalidate_cache();

        Ok(())
    }

    /// Expand a directory with pre-loaded entries.
    pub fn expand_directory_with_entries(
        &mut self,
        path: &Path,
        entries: Vec<(PathBuf, std::fs::Metadata)>,
    ) -> Result<()> {
        let entries = entries
            .into_iter()
            .map(|(path, metadata)| FileTreeDirectoryEntry::from_metadata(path, metadata))
            .collect();
        self.expand_directory_with_listing(path, entries)
    }

    pub fn expand_directory_with_listing(
        &mut self,
        path: &Path,
        entries: Vec<FileTreeDirectoryEntry>,
    ) -> Result<()> {
        let path = normalize_tree_path(path);
        let mut parent_entry = self.entry_by_path(&path).context("Entry not found")?;

        if !parent_entry.is_directory() {
            anyhow::bail!("Not a directory");
        }

        self.expanded_dirs.insert(path.clone());
        self.remove_descendants(&path);

        let parent_depth = if path == self.root_path {
            0
        } else {
            parent_entry.depth
        };
        let mut children = Vec::new();

        for entry in entries {
            let child_path = normalize_tree_path(&entry.path);

            if !self.config.show_hidden && self.is_hidden_file(&child_path) {
                continue;
            }

            if !self.config.show_ignored && self.is_ignored_file(&child_path) {
                continue;
            }

            let mut child_entry =
                self.entry_from_directory_listing(child_path, entry, parent_depth + 1);
            child_entry.is_visible = true;
            children.push(child_entry);
        }

        if path != self.root_path {
            parent_entry.is_expanded = true;
            if let FileKind::Directory {
                ref mut child_count,
                ref mut is_loaded,
            } = parent_entry.kind
            {
                *child_count = children.len();
                *is_loaded = true;
            }
            self.upsert_entry(parent_entry);
        }

        self.insert_entries(children);
        self.loading_dirs.remove(&path);
        self.invalidate_cache();

        Ok(())
    }

    /// Get the total number of entries.
    pub fn total_count(&self) -> usize {
        self.entries.len()
    }

    /// Get the number of visible entries.
    pub fn visible_count(&self) -> usize {
        self.visible_entries_snapshot().len()
    }

    /// Get file statistics.
    pub fn stats(&self) -> FileTreeStats {
        let mut stats = FileTreeStats {
            total_entries: self.entries.len(),
            visible_entries: self.visible_entries_snapshot().len(),
            file_count: 0,
            directory_count: 0,
            total_size: 0,
            max_depth: 0,
        };

        for entry in self.entries.values() {
            if entry.is_file() {
                stats.file_count += 1;
            }
            if entry.is_directory() {
                stats.directory_count += 1;
            }
            stats.total_size += entry.size;
            stats.max_depth = stats.max_depth.max(entry.depth);
        }

        stats
    }

    /// Add or update an entry.
    pub fn upsert_entry(&mut self, entry: FileTreeEntry) {
        self.upsert_entries(vec![entry]);
    }

    /// Add or update multiple entries at once.
    pub fn upsert_entries(&mut self, new_entries: Vec<FileTreeEntry>) {
        for entry in new_entries {
            self.remove_single_entry(&entry.path);
            self.insert_entry(entry);
        }
        self.invalidate_cache();
    }

    /// Remove an entry by path, including any descendants.
    pub fn remove_entry(&mut self, path: &Path) -> Option<FileTreeEntry> {
        let path = normalize_tree_path(path);
        let removed = self.remove_single_entry(&path);
        if removed.is_some() {
            self.remove_descendants(&path);
            self.expanded_dirs.remove(&path);
            self.invalidate_cache();
        }
        removed
    }

    /// Move an entry and all loaded descendants to a new canonical path.
    ///
    /// Returns `Ok(true)` when a subtree moved and `Ok(false)` when the move
    /// was a no-op or skipped because of the selected collision strategy.
    pub fn move_entry(
        &mut self,
        from: &Path,
        to: &Path,
        collision: FileTreeCollisionStrategy,
    ) -> Result<bool> {
        let from = normalize_tree_path(from);
        let to = normalize_tree_path(to);

        if from == to {
            return Ok(false);
        }

        if from == self.root_path {
            anyhow::bail!("Cannot move the file tree root");
        }

        if to.starts_with(&from) {
            anyhow::bail!("Cannot move a directory into itself");
        }

        if !from.starts_with(&self.root_path) || !to.starts_with(&self.root_path) {
            anyhow::bail!("File tree moves must stay within the tree root");
        }

        if !self.entries.contains_key(&from) {
            anyhow::bail!("Entry not found: {}", from.display());
        }

        if self.entries.contains_key(&to) {
            match collision {
                FileTreeCollisionStrategy::Error => {
                    anyhow::bail!("Destination already exists: {}", to.display());
                }
                FileTreeCollisionStrategy::Skip => return Ok(false),
                FileTreeCollisionStrategy::Replace => {
                    self.remove_entry(&to);
                }
            }
        }

        let affected_paths: Vec<_> = self
            .entries
            .keys()
            .filter(|path| *path == &from || path.starts_with(&from))
            .cloned()
            .collect();

        let mut moved_entries = Vec::with_capacity(affected_paths.len());
        for old_path in &affected_paths {
            if let Some(mut entry) = self.remove_single_entry(old_path) {
                let new_path = rebase_path(old_path, &from, &to);
                self.retarget_entry_path(&mut entry, new_path);
                moved_entries.push(entry);
            }
        }

        self.remap_path_set_prefix(&from, &to, PathSetKind::Expanded);
        self.remap_path_set_prefix(&from, &to, PathSetKind::Loading);
        self.insert_entries(moved_entries);

        if let Some(parent) = from.parent() {
            self.refresh_known_directory_child_count(parent);
        }
        if let Some(parent) = to.parent() {
            self.refresh_known_directory_child_count(parent);
        }

        self.invalidate_cache();

        Ok(true)
    }

    /// Find the parent entry of a given path in the visible entries.
    pub fn find_parent_entry(&mut self, path: &Path) -> Option<FileTreeEntry> {
        let parent_path = normalize_tree_path(path).parent()?.to_path_buf();
        if parent_path < self.root_path {
            return None;
        }

        let target_path = if parent_path == self.root_path {
            self.root_path.clone()
        } else {
            parent_path
        };

        self.visible_entries()
            .iter()
            .find(|entry| entry.path == target_path)
            .cloned()
    }

    /// Find the first child entry of a directory in the visible entries.
    pub fn find_first_child_entry(&mut self, dir_path: &Path) -> Option<FileTreeEntry> {
        let dir_path = normalize_tree_path(dir_path);
        let entry = self.entry_by_path(&dir_path)?;
        if !entry.is_directory() {
            return None;
        }

        let visible_entries = self.visible_entries();
        let parent_index = visible_entries
            .iter()
            .position(|entry| entry.path == dir_path)?;

        visible_entries.get(parent_index + 1).and_then(|candidate| {
            candidate
                .ancestor_paths
                .iter()
                .any(|ancestor| ancestor == &dir_path)
                .then(|| candidate.clone())
        })
    }

    /// Generate next entry ID.
    pub fn next_entry_id(&mut self) -> FileTreeEntryId {
        let id = FileTreeEntryId(self.next_id);
        self.next_id += 1;
        id
    }

    fn scan_directory_recursive(
        &mut self,
        dir_path: &Path,
        current_depth: usize,
        max_depth: usize,
    ) -> Result<(Vec<FileTreeEntry>, usize)> {
        let mut entries = Vec::new();
        let read_dir = fs::read_dir(dir_path)
            .with_context(|| format!("Failed to read directory: {}", dir_path.display()))?;

        let mut immediate_count = 0;
        for entry in read_dir {
            let entry = entry?;
            let path = normalize_tree_path(&entry.path());
            let metadata = entry.metadata()?;
            let directory_entry = FileTreeDirectoryEntry::from_metadata(path, metadata);

            if !self.config.show_hidden && self.is_hidden_file(&directory_entry.path) {
                continue;
            }

            if !self.config.show_ignored && self.is_ignored_file(&directory_entry.path) {
                continue;
            }

            immediate_count += 1;
            let mut file_entry = self.entry_from_directory_listing(
                directory_entry.path.clone(),
                directory_entry,
                current_depth,
            );

            if file_entry.is_directory() && current_depth < max_depth {
                let (children, child_count) =
                    self.scan_directory_recursive(&file_entry.path, current_depth + 1, max_depth)?;
                if let FileKind::Directory {
                    child_count: ref mut entry_child_count,
                    ref mut is_loaded,
                } = file_entry.kind
                {
                    *entry_child_count = child_count;
                    *is_loaded = true;
                }
                entries.extend(children);
            }

            entries.push(file_entry);
        }

        Ok((entries, immediate_count))
    }

    fn load_directory(&mut self, dir_path: &Path) -> Result<()> {
        let path = normalize_tree_path(dir_path);
        let mut entry = self.entry_by_path(&path).context("Entry not found")?;

        if let FileKind::Directory { is_loaded, .. } = &entry.kind
            && *is_loaded
        {
            return Ok(());
        }

        let parent_depth = if path == self.root_path {
            0
        } else {
            entry.depth
        };
        let (children, child_count) =
            self.scan_directory_recursive(&path, parent_depth + 1, parent_depth + 1)?;

        if path != self.root_path {
            entry.is_expanded = true;
            if let FileKind::Directory {
                child_count: ref mut entry_child_count,
                ref mut is_loaded,
            } = entry.kind
            {
                *entry_child_count = child_count;
                *is_loaded = true;
            }
            self.upsert_entry(entry);
        }

        self.insert_entries(children);

        Ok(())
    }

    fn entry_from_directory_listing(
        &mut self,
        path: PathBuf,
        listing: FileTreeDirectoryEntry,
        depth: usize,
    ) -> FileTreeEntry {
        let id = self.next_entry_id();
        let mut entry = match listing.kind {
            FileTreeDirectoryEntryKind::Directory => {
                FileTreeEntry::new_directory(id, path.clone(), listing.mtime)
            }
            FileTreeDirectoryEntryKind::File => {
                FileTreeEntry::new_file(id, path.clone(), listing.size, listing.mtime)
            }
            FileTreeDirectoryEntryKind::Symlink {
                target,
                target_exists,
            } => FileTreeEntry::new_symlink(id, path, target, target_exists, listing.mtime),
            FileTreeDirectoryEntryKind::Other => {
                FileTreeEntry::new_file(id, path.clone(), listing.size, listing.mtime)
            }
        };

        entry.depth = depth;
        entry.is_visible = true;
        entry.is_ignored = self.is_ignored_file(&entry.path);
        entry
    }

    fn insert_entries(&mut self, entries: Vec<FileTreeEntry>) {
        for entry in entries {
            self.insert_entry(entry);
        }
    }

    fn insert_entry(&mut self, mut entry: FileTreeEntry) {
        entry.path = normalize_tree_path(&entry.path);
        self.path_to_id.insert(entry.path.clone(), entry.id);
        self.entries.insert(entry.path.clone(), entry);
    }

    fn remove_single_entry(&mut self, path: &Path) -> Option<FileTreeEntry> {
        let path = normalize_tree_path(path);
        self.path_to_id.remove(&path);
        self.entries.remove(&path)
    }

    fn remove_descendants(&mut self, path: &Path) {
        let path = normalize_tree_path(path);
        let descendants: Vec<_> = self
            .entries
            .keys()
            .filter(|candidate| candidate.starts_with(&path) && *candidate != &path)
            .cloned()
            .collect();

        for descendant in descendants {
            self.path_to_id.remove(&descendant);
            self.entries.remove(&descendant);
            self.expanded_dirs.remove(&descendant);
        }
    }

    fn retarget_entry_path(&self, entry: &mut FileTreeEntry, new_path: PathBuf) {
        entry.path = normalize_tree_path(&new_path);
        entry.depth = self.depth_for_path(&entry.path);
        entry.is_hidden = self.is_hidden_file(&entry.path);
        entry.is_ignored = self.is_ignored_file(&entry.path);
        entry.flattened_segments = None;
        entry.ancestor_paths = Arc::from([]);
        entry.is_search_match = false;

        if let FileKind::File { extension } = &mut entry.kind {
            *extension = entry
                .path
                .extension()
                .map(|extension| extension.to_string_lossy().to_string());
        }
    }

    fn depth_for_path(&self, path: &Path) -> usize {
        path.strip_prefix(&self.root_path)
            .ok()
            .map(|relative| relative.components().count())
            .unwrap_or(0)
    }

    fn remap_path_set_prefix(&mut self, from: &Path, to: &Path, kind: PathSetKind) {
        let set = match kind {
            PathSetKind::Expanded => &mut self.expanded_dirs,
            PathSetKind::Loading => &mut self.loading_dirs,
        };
        let moved_paths: Vec<_> = set
            .iter()
            .filter(|path| *path == from || path.starts_with(from))
            .cloned()
            .collect();

        for old_path in moved_paths {
            set.remove(&old_path);
            set.insert(rebase_path(&old_path, from, to));
        }
    }

    fn refresh_known_directory_child_count(&mut self, path: &Path) {
        let path = normalize_tree_path(path);
        let child_count = self
            .entries
            .values()
            .filter(|entry| entry.path.parent() == Some(path.as_path()))
            .count();

        if let Some(entry) = self.entries.get_mut(&path)
            && let FileKind::Directory {
                child_count: entry_child_count,
                is_loaded,
            } = &mut entry.kind
        {
            *entry_child_count = child_count;
            *is_loaded = true;
        }
    }

    fn entry_is_included(&self, entry: &FileTreeEntry) -> bool {
        if entry.is_hidden && !self.config.show_hidden {
            return false;
        }

        if entry.is_ignored && !self.config.show_ignored {
            return false;
        }

        true
    }

    /// Check if a file is hidden (starts with .).
    fn is_hidden_file(&self, path: &Path) -> bool {
        path.file_name()
            .map(|name| name.to_string_lossy().starts_with('.'))
            .unwrap_or(false)
    }

    /// Build gitignore matcher using the same patterns as the file picker.
    fn build_gitignore_matcher(root_path: &Path) -> Option<Gitignore> {
        let mut builder = GitignoreBuilder::new(root_path);

        if let Ok(gitignore_path) = root_path.join(".gitignore").canonicalize()
            && gitignore_path.exists()
        {
            let _ = builder.add(&gitignore_path);
        }

        if let Some(git_config_dir) = dirs::config_dir() {
            let global_gitignore = git_config_dir.join("git").join("ignore");
            if global_gitignore.exists() {
                let _ = builder.add(&global_gitignore);
            }
        }

        let git_exclude = root_path.join(".git").join("info").join("exclude");
        if git_exclude.exists() {
            let _ = builder.add(&git_exclude);
        }

        let ignore_file = root_path.join(".ignore");
        if ignore_file.exists() {
            let _ = builder.add(&ignore_file);
        }

        let helix_ignore = root_path.join(".helix").join("ignore");
        if helix_ignore.exists() {
            let _ = builder.add(&helix_ignore);
        }

        builder.build().ok()
    }

    /// Check if a file is ignored using the same patterns as the file picker.
    fn is_ignored_file(&self, path: &Path) -> bool {
        for component in path.components() {
            if let Component::Normal(name) = component
                && let Some(name_str) = name.to_str()
            {
                match name_str {
                    ".git" | ".svn" | ".hg" | ".bzr" => return true,
                    _ => {}
                }
            }
        }

        if let Some(ref gitignore) = self.gitignore
            && let Ok(relative_path) = path.strip_prefix(&self.root_path)
        {
            let matched = gitignore.matched(relative_path, path.is_dir());
            return matched.is_ignore();
        }

        false
    }
}

struct ProjectionRow {
    path: PathBuf,
    flattened_segments: Option<Arc<[FileTreeFlattenedSegment]>>,
}

enum PathSetKind {
    Expanded,
    Loading,
}

fn normalize_tree_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }

    if normalized.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        normalized
    }
}

fn normalize_search_query(value: &str) -> String {
    value.trim().to_lowercase()
}

fn rebase_path(path: &Path, from: &Path, to: &Path) -> PathBuf {
    match path.strip_prefix(from) {
        Ok(relative) if relative.as_os_str().is_empty() => to.to_path_buf(),
        Ok(relative) => to.join(relative),
        Err(_) => path.to_path_buf(),
    }
}

fn display_name_for_path(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .or_else(|| {
            path.components()
                .next_back()
                .and_then(|component| component.as_os_str().to_str())
        })
        .unwrap_or(".")
        .to_string()
}

fn directory_child_parent_paths(root_path: &Path, entries: &[FileTreeEntry]) -> HashSet<PathBuf> {
    entries
        .iter()
        .filter(|entry| entry.is_directory())
        .filter_map(|entry| entry.path.parent())
        .filter(|parent| parent.starts_with(root_path))
        .map(Path::to_path_buf)
        .collect()
}

/// Statistics about the file tree.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> FileTreeConfig {
        FileTreeConfig {
            show_hidden: true,
            show_ignored: true,
            initial_depth: 3,
            watch_filesystem: false,
            flatten_empty_directories: true,
            search_mode: FileTreeSearchMode::ExpandMatches,
            density: crate::file_tree::FileTreeDisplayDensity::Default,
            translucent_background: false,
        }
    }

    fn config_with_search_mode(search_mode: FileTreeSearchMode) -> FileTreeConfig {
        FileTreeConfig {
            search_mode,
            ..config()
        }
    }

    fn visible_paths(tree: &mut FileTree) -> Vec<PathBuf> {
        tree.visible_entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect()
    }

    #[test]
    fn initial_load_expands_root_and_exposes_child_directories() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let src = root.join("src");
        let lib = src.join("lib.rs");
        let readme = root.join("README.md");
        fs::create_dir(&src).unwrap();
        fs::write(&lib, "pub fn lib() {}\n").unwrap();
        fs::write(&readme, "# Project\n").unwrap();

        let mut tree = FileTree::new(root.clone(), config());
        tree.load().unwrap();

        let paths = visible_paths(&mut tree);
        assert!(tree.is_expanded(&root));
        assert!(paths.contains(&src));
        assert!(paths.contains(&readme));
        assert!(!paths.contains(&lib));
        assert!(!tree.is_expanded(&src));
    }

    #[test]
    fn root_only_load_can_expand_from_preloaded_listing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let src = root.join("src");
        let mut tree = FileTree::new(root.clone(), config());

        tree.load_root_only();
        tree.expand_directory_with_listing(
            &root,
            vec![FileTreeDirectoryEntry {
                path: src.clone(),
                kind: FileTreeDirectoryEntryKind::Directory,
                size: 0,
                mtime: None,
            }],
        )
        .unwrap();

        let paths = visible_paths(&mut tree);
        assert!(tree.is_expanded(&root));
        assert!(paths.contains(&src));
    }

    #[test]
    fn initial_load_expands_parents_to_expose_directory_children() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let macros_dir = root.join("mp-config-sqlx-macros");
        let macros_src = macros_dir.join("src");
        let macros_lib = macros_src.join("lib.rs");
        let macros_manifest = macros_dir.join("Cargo.toml");
        let examples = root.join("examples");
        let example = examples.join("basic.rs");
        fs::create_dir_all(&macros_src).unwrap();
        fs::create_dir_all(&examples).unwrap();
        fs::write(&macros_lib, "pub fn lib() {}\n").unwrap();
        fs::write(&macros_manifest, "[package]\n").unwrap();
        fs::write(&example, "fn main() {}\n").unwrap();

        let mut tree = FileTree::new(root.clone(), config());
        tree.load().unwrap();

        let paths = visible_paths(&mut tree);
        assert!(tree.is_expanded(&root));
        assert!(tree.is_expanded(&macros_dir));
        assert!(!tree.is_expanded(&macros_src));
        assert!(!tree.is_expanded(&examples));
        assert!(paths.contains(&macros_dir));
        assert!(paths.contains(&macros_src));
        assert!(paths.contains(&macros_manifest));
        assert!(paths.contains(&examples));
        assert!(!paths.contains(&macros_lib));
        assert!(!paths.contains(&example));
    }

    #[test]
    fn collapsed_loaded_directory_reexpands_with_children_visible() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let src = root.join("src");
        let lib = src.join("lib.rs");
        fs::create_dir(&src).unwrap();
        fs::write(&lib, "pub fn lib() {}\n").unwrap();

        let mut tree = FileTree::new(root.clone(), config());
        tree.load().unwrap();

        tree.toggle_directory(&src).unwrap();
        assert!(visible_paths(&mut tree).contains(&lib));

        tree.collapse_directory(&src).unwrap();
        assert!(!visible_paths(&mut tree).contains(&lib));

        tree.toggle_directory(&src).unwrap();
        assert!(visible_paths(&mut tree).contains(&lib));
    }

    #[test]
    fn visible_projection_flattens_single_child_directory_chains() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let leaf_dir = root.join("src").join("features").join("tree");
        fs::create_dir_all(&leaf_dir).unwrap();
        fs::write(leaf_dir.join("mod.rs"), "mod tree;\n").unwrap();

        let mut tree = FileTree::new(root.clone(), config());
        tree.load().unwrap();
        let rows = tree.visible_entries();
        let flattened = rows.iter().find(|entry| entry.path == leaf_dir).unwrap();
        let segments = flattened.flattened_segments.as_ref().unwrap();

        assert_eq!(flattened.depth, 1);
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.name.as_str())
                .collect::<Vec<_>>(),
            vec!["src", "features", "tree"]
        );
        assert!(segments.last().unwrap().is_terminal);
    }

    #[test]
    fn search_projection_expands_matching_ancestors_by_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let nested = root.join("src").join("ui");
        let matched = nested.join("button.rs");
        let other = root.join("README.md");
        fs::create_dir_all(&nested).unwrap();
        fs::write(&matched, "button\n").unwrap();
        fs::write(&other, "readme\n").unwrap();

        let mut tree = FileTree::new(root.clone(), config());
        tree.load().unwrap();
        tree.collapse_directory(&root).unwrap();
        tree.set_search_query(Some("button".to_string()));

        let paths = visible_paths(&mut tree);
        assert!(paths.contains(&matched));
        assert!(!paths.contains(&other));
        assert_eq!(tree.search_matching_paths(), vec![matched]);
    }

    #[test]
    fn search_projection_collapse_non_matches_keeps_expanded_sibling_rows() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        let alpha_child = alpha.join("keep.rs");
        let matched = beta.join("match.rs");
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        fs::write(&alpha_child, "keep\n").unwrap();
        fs::write(&matched, "match\n").unwrap();

        let mut tree = FileTree::new(
            root.clone(),
            config_with_search_mode(FileTreeSearchMode::CollapseNonMatches),
        );
        tree.load().unwrap();
        tree.toggle_directory(&alpha).unwrap();
        tree.set_search_query(Some("match".to_string()));

        let paths = visible_paths(&mut tree);
        assert!(paths.contains(&alpha));
        assert!(!paths.contains(&alpha_child));
        assert!(paths.contains(&beta));
        assert!(paths.contains(&matched));
    }

    #[test]
    fn search_projection_hide_non_matches_removes_unmatched_sibling_rows() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        let alpha_child = alpha.join("keep.rs");
        let matched = beta.join("match.rs");
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        fs::write(&alpha_child, "keep\n").unwrap();
        fs::write(&matched, "match\n").unwrap();

        let mut tree = FileTree::new(
            root.clone(),
            config_with_search_mode(FileTreeSearchMode::HideNonMatches),
        );
        tree.load().unwrap();
        tree.set_search_query(Some("match".to_string()));

        let paths = visible_paths(&mut tree);
        assert!(!paths.contains(&alpha));
        assert!(!paths.contains(&alpha_child));
        assert!(paths.contains(&beta));
        assert!(paths.contains(&matched));
    }

    #[test]
    fn visible_projection_assigns_tree_position_metadata() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let app = root.join("app");
        let docs = root.join("docs");
        let app_main = app.join("main.rs");
        let app_mod = app.join("mod.rs");
        let readme = root.join("README.md");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&docs).unwrap();
        fs::write(&app_main, "fn main() {}\n").unwrap();
        fs::write(&app_mod, "mod main;\n").unwrap();
        fs::write(&readme, "# Project\n").unwrap();

        let mut tree = FileTree::new(root.clone(), config());
        tree.load().unwrap();
        tree.toggle_directory(&app).unwrap();
        let entries = tree.visible_entries();

        let root_entry = entries.iter().find(|entry| entry.path == root).unwrap();
        assert_eq!(root_entry.level, 1);
        assert_eq!(root_entry.pos_in_set, 1);
        assert_eq!(root_entry.set_size, 1);
        assert!(root_entry.ancestor_paths.is_empty());

        let app_entry = entries.iter().find(|entry| entry.path == app).unwrap();
        assert_eq!(app_entry.level, 2);
        assert_eq!(app_entry.pos_in_set, 1);
        assert_eq!(app_entry.set_size, 3);
        assert!(app_entry.ancestor_paths.is_empty());

        let app_main_entry = entries.iter().find(|entry| entry.path == app_main).unwrap();
        assert_eq!(app_main_entry.level, 3);
        assert_eq!(app_main_entry.pos_in_set, 1);
        assert_eq!(app_main_entry.set_size, 2);
        assert_eq!(app_main_entry.ancestor_paths.as_ref(), [app]);
    }

    #[test]
    fn remove_entry_removes_descendants_from_path_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let dir = root.join("src");
        let child = dir.join("lib.rs");
        fs::create_dir(&dir).unwrap();
        fs::write(&child, "pub fn lib() {}\n").unwrap();

        let mut tree = FileTree::new(root, config());
        tree.load().unwrap();

        assert!(tree.remove_entry(&dir).is_some());
        assert!(tree.entry_by_path(&dir).is_none());
        assert!(tree.entry_by_path(&child).is_none());
    }

    #[test]
    fn move_entry_preserves_descendants_ids_and_expansion() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let src = root.join("src");
        let nested = src.join("nested");
        let child = nested.join("lib.rs");
        let moved = root.join("crates");
        let moved_nested = moved.join("nested");
        let moved_child = moved_nested.join("lib.rs");
        fs::create_dir_all(&nested).unwrap();
        fs::write(&child, "pub fn lib() {}\n").unwrap();

        let mut tree = FileTree::new(root, config());
        tree.load().unwrap();
        let child_id = tree.entry_by_path(&child).unwrap().id;
        tree.toggle_directory(&nested).unwrap();

        assert!(
            tree.move_entry(&src, &moved, FileTreeCollisionStrategy::Error)
                .unwrap()
        );

        assert!(tree.entry_by_path(&src).is_none());
        assert!(tree.entry_by_path(&nested).is_none());
        assert!(tree.entry_by_path(&child).is_none());
        assert_eq!(tree.entry_by_path(&moved_child).unwrap().id, child_id);
        assert!(tree.is_expanded(&moved));
        assert!(tree.is_expanded(&moved_nested));
    }

    #[test]
    fn move_entry_skip_collision_leaves_source_and_destination() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let src = root.join("src");
        let dest = root.join("crates");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src.join("lib.rs"), "pub fn src() {}\n").unwrap();
        fs::write(dest.join("lib.rs"), "pub fn dest() {}\n").unwrap();

        let mut tree = FileTree::new(root, config());
        tree.load().unwrap();
        let source_id = tree.entry_by_path(&src).unwrap().id;
        let destination_id = tree.entry_by_path(&dest).unwrap().id;

        assert!(
            !tree
                .move_entry(&src, &dest, FileTreeCollisionStrategy::Skip)
                .unwrap()
        );

        assert_eq!(tree.entry_by_path(&src).unwrap().id, source_id);
        assert_eq!(tree.entry_by_path(&dest).unwrap().id, destination_id);
    }

    #[test]
    fn move_entry_replace_collision_removes_destination_subtree() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let src = root.join("src");
        let src_child = src.join("lib.rs");
        let dest = root.join("crates");
        let old_dest_child = dest.join("main.rs");
        let new_dest_child = dest.join("lib.rs");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(&src_child, "pub fn src() {}\n").unwrap();
        fs::write(&old_dest_child, "pub fn dest() {}\n").unwrap();

        let mut tree = FileTree::new(root, config());
        tree.load().unwrap();
        let source_child_id = tree.entry_by_path(&src_child).unwrap().id;

        assert!(
            tree.move_entry(&src, &dest, FileTreeCollisionStrategy::Replace)
                .unwrap()
        );

        assert!(tree.entry_by_path(&src).is_none());
        assert!(tree.entry_by_path(&src_child).is_none());
        assert!(tree.entry_by_path(&old_dest_child).is_none());
        assert_eq!(
            tree.entry_by_path(&new_dest_child).unwrap().id,
            source_child_id
        );
    }
}
