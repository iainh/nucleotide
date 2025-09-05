// ABOUTME: Utility functions for efficient path traversal and file system operations
// ABOUTME: Provides optimized ancestor scanning and validation helpers for manifest detection

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{ProjectError, Result};

/// Configuration for path traversal operations
#[derive(Debug, Clone)]
pub struct TraversalConfig {
    /// Maximum depth to traverse upward from the starting path
    pub max_depth: usize,
    /// Paths to exclude from traversal (e.g., system directories)
    pub excluded_paths: HashSet<PathBuf>,
    /// Whether to follow symlinks during traversal
    pub follow_symlinks: bool,
    /// Whether to stop at filesystem boundaries
    pub stop_at_fs_boundary: bool,
}

impl Default for TraversalConfig {
    fn default() -> Self {
        let mut excluded_paths = HashSet::new();

        // Common system paths to exclude
        #[cfg(unix)]
        {
            excluded_paths.insert(PathBuf::from("/"));
            excluded_paths.insert(PathBuf::from("/usr"));
            excluded_paths.insert(PathBuf::from("/var"));
            excluded_paths.insert(PathBuf::from("/etc"));
            excluded_paths.insert(PathBuf::from("/tmp"));
            excluded_paths.insert(PathBuf::from("/sys"));
            excluded_paths.insert(PathBuf::from("/proc"));
        }

        #[cfg(windows)]
        {
            excluded_paths.insert(PathBuf::from("C:\\Windows"));
            excluded_paths.insert(PathBuf::from("C:\\Program Files"));
            excluded_paths.insert(PathBuf::from("C:\\Program Files (x86)"));
        }

        Self {
            max_depth: 20,
            excluded_paths,
            follow_symlinks: false,
            stop_at_fs_boundary: true,
        }
    }
}

impl TraversalConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    pub fn with_excluded_path(mut self, path: PathBuf) -> Self {
        self.excluded_paths.insert(path);
        self
    }

    pub fn with_follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    pub fn with_stop_at_fs_boundary(mut self, stop: bool) -> Self {
        self.stop_at_fs_boundary = stop;
        self
    }
}

/// Iterator for traversing ancestor directories with configuration
pub struct AncestorIterator {
    current: Option<PathBuf>,
    config: TraversalConfig,
    visited: HashSet<PathBuf>,
    depth: usize,
    #[cfg(unix)]
    starting_device: Option<u64>,
}

impl AncestorIterator {
    pub fn new<P: AsRef<Path>>(path: P, config: TraversalConfig) -> Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(ProjectError::invalid_path(path.to_path_buf()));
        }

        let canonical = path
            .canonicalize()
            .map_err(|e| ProjectError::manifest_parse(path.to_path_buf(), e))?;

        #[cfg(unix)]
        let starting_device = if config.stop_at_fs_boundary {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&canonical)
                .map(|m| Some(m.dev()))
                .unwrap_or(None)
        } else {
            None
        };

        Ok(Self {
            current: Some(canonical),
            config,
            visited: HashSet::new(),
            depth: 0,
            #[cfg(unix)]
            starting_device,
        })
    }

    /// Create iterator with default configuration
    pub fn with_defaults<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::new(path, TraversalConfig::default())
    }

    /// Create iterator with custom max depth
    pub fn with_max_depth<P: AsRef<Path>>(path: P, max_depth: usize) -> Result<Self> {
        Self::new(path, TraversalConfig::default().with_max_depth(max_depth))
    }

    /// Check if we should stop traversal at this path
    fn should_stop(&self, path: &Path) -> bool {
        // Check depth limit
        if self.depth >= self.config.max_depth {
            return true;
        }

        // Check excluded paths
        if self.config.excluded_paths.contains(path) {
            return true;
        }

        // Check filesystem boundary
        #[cfg(unix)]
        if self.config.stop_at_fs_boundary {
            if let Some(starting_device) = self.starting_device {
                if let Ok(metadata) = std::fs::metadata(path) {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.dev() != starting_device {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Check if path is a symlink and handle according to config
    fn handle_symlink(&self, path: &Path) -> Option<PathBuf> {
        if !self.config.follow_symlinks && path.is_symlink() {
            return None;
        }

        Some(path.to_path_buf())
    }
}

impl Iterator for AncestorIterator {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(current) = self.current.take() {
            // Check if we should stop here
            if self.should_stop(&current) {
                return None;
            }

            // Check for circular references
            if !self.visited.insert(current.clone()) {
                nucleotide_logging::warn!(
                    path = %current.display(),
                    "Circular reference detected in path traversal"
                );
                return None;
            }

            // Handle symlinks
            let path_to_return = self.handle_symlink(&current)?;

            // Set up next iteration
            self.current = current.parent().map(|p| p.to_path_buf());
            self.depth += 1;

            return Some(path_to_return);
        }

        None
    }
}

/// Find all files matching patterns in ancestor directories
pub async fn find_files_in_ancestors<P: AsRef<Path>>(
    start_path: P,
    patterns: &[String],
    config: TraversalConfig,
) -> Result<Vec<PathBuf>> {
    let iterator = AncestorIterator::new(start_path, config)?;
    let mut found_files = Vec::new();

    for ancestor in iterator {
        for pattern in patterns {
            let candidate = ancestor.join(pattern);
            if tokio::fs::metadata(&candidate).await.is_ok() {
                found_files.push(candidate);
            }
        }
    }

    Ok(found_files)
}

/// Check if a path is likely a project root based on common indicators
pub async fn is_likely_project_root(path: &Path) -> bool {
    let common_indicators = [
        // Version control
        ".git",
        ".hg",
        ".svn",
        ".bzr",
        // Build tools
        "Makefile",
        "CMakeLists.txt",
        "build.xml",
        "build.gradle",
        // Package managers
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "requirements.txt",
        "pom.xml",
        "go.mod",
        "composer.json",
        "Gemfile",
        // IDE/Editor files
        ".vscode",
        ".idea",
        ".project",
        // Documentation
        "README.md",
        "README.rst",
        "README.txt",
        "README",
        // Configuration
        ".editorconfig",
        ".gitignore",
        ".env",
    ];

    let mut indicator_count = 0;
    for indicator in &common_indicators {
        let indicator_path = path.join(indicator);
        if tokio::fs::metadata(&indicator_path).await.is_ok() {
            indicator_count += 1;
        }
    }

    // Consider it a project root if we have 2 or more indicators
    indicator_count >= 2
}

/// Validate that a path is suitable for project detection
pub async fn validate_path_for_detection(path: &Path) -> Result<()> {
    // Check if path exists
    if tokio::fs::metadata(path).await.is_err() {
        return Err(ProjectError::invalid_path(path.to_path_buf()));
    }

    // Check if path is accessible
    match tokio::fs::metadata(path).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(ProjectError::access_denied(path.to_path_buf()));
        }
        Err(e) => {
            return Err(ProjectError::manifest_parse(path.to_path_buf(), e));
        }
    }

    Ok(())
}

/// Get the canonical form of a path, handling errors gracefully
pub fn canonicalize_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .map_err(|e| ProjectError::manifest_parse(path.to_path_buf(), e))
}

/// Calculate the depth between two paths
pub fn path_depth_between(from: &Path, to: &Path) -> Option<usize> {
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();

    if to_components.len() <= from_components.len() {
        return None;
    }

    // Check if 'from' is actually an ancestor of 'to'
    for (i, component) in from_components.iter().enumerate() {
        if to_components.get(i) != Some(component) {
            return None;
        }
    }

    Some(to_components.len() - from_components.len())
}

/// Check if a directory contains any files (not just directories)
pub async fn directory_contains_files(path: &Path) -> Result<bool> {
    let mut entries = tokio::fs::read_dir(path)
        .await
        .map_err(|e| ProjectError::manifest_parse(path.to_path_buf(), e))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| ProjectError::manifest_parse(path.to_path_buf(), e))?
    {
        if entry
            .file_type()
            .await
            .map_err(|e| ProjectError::manifest_parse(path.to_path_buf(), e))?
            .is_file()
        {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_ancestor_iterator() {
        let temp_dir = TempDir::new().unwrap();
        let nested_path = temp_dir.path().join("a").join("b").join("c");
        tokio::fs::create_dir_all(&nested_path).await.unwrap();

        let config = TraversalConfig::default().with_max_depth(5);
        let iterator = AncestorIterator::new(&nested_path, config).unwrap();

        let ancestors: Vec<_> = iterator.collect();
        assert!(!ancestors.is_empty());

        // Should include the nested path itself and its ancestors
        assert!(ancestors.iter().any(|p| p.ends_with("c")));
        assert!(ancestors.iter().any(|p| p.ends_with("b")));
        assert!(ancestors.iter().any(|p| p.ends_with("a")));
    }

    #[tokio::test]
    async fn test_ancestor_iterator_max_depth() {
        let temp_dir = TempDir::new().unwrap();
        let nested_path = temp_dir.path().join("a").join("b").join("c");
        tokio::fs::create_dir_all(&nested_path).await.unwrap();

        let config = TraversalConfig::default().with_max_depth(2);
        let iterator = AncestorIterator::new(&nested_path, config).unwrap();

        let ancestors: Vec<_> = iterator.collect();
        assert!(ancestors.len() <= 2);
    }

    #[tokio::test]
    async fn test_find_files_in_ancestors() {
        let temp_dir = TempDir::new().unwrap();
        let nested_path = temp_dir.path().join("src");
        tokio::fs::create_dir_all(&nested_path).await.unwrap();

        // Create a Cargo.toml in the root
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        tokio::fs::write(&cargo_toml, "[package]\nname = \"test\"")
            .await
            .unwrap();

        let config = TraversalConfig::default();
        let found = find_files_in_ancestors(&nested_path, &["Cargo.toml".to_string()], config)
            .await
            .unwrap();

        assert_eq!(found.len(), 1);
        // Use canonicalized paths to handle symlinks on macOS
        let expected = cargo_toml.canonicalize().unwrap_or(cargo_toml);
        let actual = found[0].canonicalize().unwrap_or(found[0].clone());
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_is_likely_project_root() {
        let temp_dir = TempDir::new().unwrap();

        // Empty directory shouldn't be considered a project root
        assert!(!is_likely_project_root(temp_dir.path()).await);

        // Add some project indicators
        tokio::fs::write(temp_dir.path().join("Cargo.toml"), "[package]")
            .await
            .unwrap();
        tokio::fs::write(temp_dir.path().join("README.md"), "# Test")
            .await
            .unwrap();

        // Now it should be considered a project root
        assert!(is_likely_project_root(temp_dir.path()).await);
    }

    #[tokio::test]
    async fn test_validate_path_for_detection() {
        let temp_dir = TempDir::new().unwrap();

        // Valid path should pass
        assert!(validate_path_for_detection(temp_dir.path()).await.is_ok());

        // Non-existent path should fail
        let nonexistent = temp_dir.path().join("nonexistent");
        assert!(validate_path_for_detection(&nonexistent).await.is_err());
    }

    #[tokio::test]
    async fn test_path_depth_between() {
        let parent = Path::new("/a/b");
        let child = Path::new("/a/b/c/d");
        let unrelated = Path::new("/x/y");

        assert_eq!(path_depth_between(parent, child), Some(2));
        assert_eq!(path_depth_between(child, parent), None);
        assert_eq!(path_depth_between(parent, unrelated), None);
    }

    #[tokio::test]
    async fn test_directory_contains_files() {
        let temp_dir = TempDir::new().unwrap();

        // Empty directory
        assert!(!directory_contains_files(temp_dir.path()).await.unwrap());

        // Directory with only subdirectories
        let subdir = temp_dir.path().join("subdir");
        tokio::fs::create_dir(&subdir).await.unwrap();
        assert!(!directory_contains_files(temp_dir.path()).await.unwrap());

        // Directory with files
        tokio::fs::write(temp_dir.path().join("file.txt"), "content")
            .await
            .unwrap();
        assert!(directory_contains_files(temp_dir.path()).await.unwrap());
    }

    #[test]
    fn test_traversal_config() {
        let config = TraversalConfig::new()
            .with_max_depth(10)
            .with_excluded_path(PathBuf::from("/tmp"))
            .with_follow_symlinks(true)
            .with_stop_at_fs_boundary(false);

        assert_eq!(config.max_depth, 10);
        assert!(config.excluded_paths.contains(&PathBuf::from("/tmp")));
        assert!(config.follow_symlinks);
        assert!(!config.stop_at_fs_boundary);
    }
}
