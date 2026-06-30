// ABOUTME: Core ManifestProvider trait and related types for project detection
// ABOUTME: Provides the foundation for language-specific project root detection

use async_trait::async_trait;
use nucleotide_env::WslWorkspace;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{ProjectError, Result};

/// Represents a manifest name (e.g., "Cargo.toml", "package.json")
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ManifestName(String);

impl ManifestName {
    /// Create a new manifest name
    pub fn new<S: Into<String>>(name: S) -> Self {
        Self(name.into())
    }

    /// Get the manifest name as a string slice
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ManifestName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ManifestName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<str> for ManifestName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ManifestName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Query for manifest search operations
#[derive(Clone)]
pub struct ManifestQuery {
    /// Path to search from (typically a file path)
    pub path: Arc<Path>,
    /// Maximum depth to search upwards
    pub max_depth: usize,
    /// Delegate for file system operations
    pub delegate: Arc<dyn ManifestDelegate>,
}

impl ManifestQuery {
    pub fn new(
        path: impl AsRef<Path>,
        max_depth: usize,
        delegate: Arc<dyn ManifestDelegate>,
    ) -> Self {
        Self {
            path: path.as_ref().into(),
            max_depth,
            delegate,
        }
    }
}

/// Delegate trait for file system operations during manifest search
#[async_trait]
pub trait ManifestDelegate: Send + Sync {
    /// Check if a file or directory exists
    async fn exists(&self, path: &Path, is_dir: Option<bool>) -> bool;

    /// Read file contents as a string
    async fn read_to_string(&self, path: &Path) -> Result<String>;

    /// Get metadata for a file
    async fn metadata(&self, path: &Path) -> Result<std::fs::Metadata>;

    /// Check if a path is accessible
    async fn is_accessible(&self, path: &Path) -> bool;
}

/// Main trait for manifest providers
#[async_trait]
pub trait ManifestProvider: Send + Sync {
    /// Get the manifest name this provider handles
    fn name(&self) -> ManifestName;

    /// Get the priority of this provider (higher = checked first)
    fn priority(&self) -> u32 {
        100
    }

    /// Get the file patterns this provider recognizes
    fn file_patterns(&self) -> Vec<String>;

    /// Search for a manifest starting from the given path
    async fn search(&self, query: ManifestQuery) -> Result<Option<PathBuf>>;

    /// Validate that a found manifest is actually valid for this provider
    async fn validate_manifest(&self, path: &Path, delegate: &dyn ManifestDelegate)
    -> Result<bool>;

    /// Get additional metadata about the detected project
    async fn get_project_metadata(
        &self,
        manifest_path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<ProjectMetadata>;
}

/// Metadata about a detected project
#[derive(Debug, Clone)]
pub struct ProjectMetadata {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub language: String,
    pub dependencies: Vec<String>,
    pub dev_dependencies: Vec<String>,
    pub additional_info: std::collections::HashMap<String, String>,
}

impl Default for ProjectMetadata {
    fn default() -> Self {
        Self {
            name: None,
            version: None,
            description: None,
            language: "unknown".to_string(),
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            additional_info: std::collections::HashMap::new(),
        }
    }
}

/// Default file system delegate using tokio::fs
pub struct FsDelegate;

#[async_trait]
impl ManifestDelegate for FsDelegate {
    async fn exists(&self, path: &Path, is_dir: Option<bool>) -> bool {
        match tokio::fs::metadata(path).await {
            Ok(metadata) => match is_dir {
                Some(true) => metadata.is_dir(),
                Some(false) => metadata.is_file(),
                None => true,
            },
            Err(_) => false,
        }
    }

    async fn read_to_string(&self, path: &Path) -> Result<String> {
        tokio::fs::read_to_string(path)
            .await
            .map_err(ProjectError::from)
    }

    async fn metadata(&self, path: &Path) -> Result<std::fs::Metadata> {
        tokio::fs::metadata(path).await.map_err(ProjectError::from)
    }

    async fn is_accessible(&self, path: &Path) -> bool {
        tokio::fs::metadata(path).await.is_ok()
    }
}

/// WSL-aware delegate that runs cheap manifest probes inside the owning distro.
#[derive(Default)]
pub struct WslManifestDelegate {
    exists_cache: Mutex<HashMap<(PathBuf, Option<bool>), bool>>,
}

impl WslManifestDelegate {
    pub fn supports(path: &Path) -> bool {
        WslWorkspace::from_unc_path(path).is_some()
    }

    fn cached_exists(&self, path: &Path, is_dir: Option<bool>) -> Option<bool> {
        self.exists_cache
            .lock()
            .ok()?
            .get(&(path.to_path_buf(), is_dir))
            .copied()
    }

    fn store_exists(&self, path: &Path, is_dir: Option<bool>, exists: bool) {
        if let Ok(mut cache) = self.exists_cache.lock() {
            cache.insert((path.to_path_buf(), is_dir), exists);
        }
    }
}

#[async_trait]
impl ManifestDelegate for WslManifestDelegate {
    async fn exists(&self, path: &Path, is_dir: Option<bool>) -> bool {
        if let Some(exists) = self.cached_exists(path, is_dir) {
            return exists;
        }

        let Some(mut command) = wsl_manifest_test_command(path, is_dir) else {
            return false;
        };

        let exists = matches!(command.output().await, Ok(output) if output.status.success());
        self.store_exists(path, is_dir, exists);
        exists
    }

    async fn read_to_string(&self, path: &Path) -> Result<String> {
        let Some(mut command) = wsl_manifest_read_command(path) else {
            return Err(ProjectError::invalid_path(path.to_path_buf()));
        };

        let output = command.output().await.map_err(ProjectError::from)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ProjectError::Io(std::io::Error::other(format!(
                "failed to read WSL manifest {}: {}",
                path.display(),
                stderr.trim()
            ))));
        }

        String::from_utf8(output.stdout)
            .map_err(|error| ProjectError::manifest_parse(path.to_path_buf(), error))
    }

    async fn metadata(&self, path: &Path) -> Result<std::fs::Metadata> {
        tokio::fs::metadata(path).await.map_err(ProjectError::from)
    }

    async fn is_accessible(&self, path: &Path) -> bool {
        self.exists(path, None).await
    }
}

fn wsl_manifest_test_command(path: &Path, is_dir: Option<bool>) -> Option<tokio::process::Command> {
    let test_flag = match is_dir {
        Some(true) => "-d",
        Some(false) => "-f",
        None => "-e",
    };
    wsl_manifest_command(path, &format!("test {test_flag} {{path}}"))
}

fn wsl_manifest_read_command(path: &Path) -> Option<tokio::process::Command> {
    wsl_manifest_command(path, "cat -- {path}")
}

fn wsl_manifest_command(path: &Path, script_template: &str) -> Option<tokio::process::Command> {
    let workspace = WslWorkspace::from_unc_path(path)?;
    let linux_path = quote_posix_single(workspace.linux_path());
    let script = script_template.replace("{path}", &linux_path);

    let mut command = nucleotide_process::tokio_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(workspace.distro())
        .arg("--")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script);

    Some(command)
}

fn quote_posix_single(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Base implementation for common manifest provider patterns
pub struct BaseManifestProvider {
    pub name: ManifestName,
    pub priority: u32,
    pub file_patterns: Vec<String>,
}

impl BaseManifestProvider {
    pub fn new(name: impl Into<ManifestName>, patterns: Vec<String>) -> Self {
        Self {
            name: name.into(),
            priority: 100,
            file_patterns: patterns,
        }
    }

    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Common search implementation that looks for any of the file patterns
    pub async fn search_patterns(&self, query: &ManifestQuery) -> Result<Option<PathBuf>> {
        let mut visited_paths = std::collections::HashSet::new();

        for ancestor in query.path.ancestors().take(query.max_depth) {
            // Prevent circular dependencies
            if !visited_paths.insert(ancestor.to_path_buf()) {
                return Err(ProjectError::CircularDependency);
            }

            // Check if this directory is accessible
            if !query.delegate.is_accessible(ancestor).await {
                continue;
            }

            // Check each pattern in this directory
            for pattern in &self.file_patterns {
                let manifest_path = ancestor.join(pattern);

                if query.delegate.exists(&manifest_path, Some(false)).await {
                    nucleotide_logging::debug!(
                        provider = %self.name,
                        manifest_path = %manifest_path.display(),
                        "Found potential manifest file"
                    );

                    // Validate the manifest
                    if self
                        .validate_manifest_internal(&manifest_path, &*query.delegate)
                        .await?
                    {
                        return Ok(Some(ancestor.to_path_buf()));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Internal validation method that can be overridden
    async fn validate_manifest_internal(
        &self,
        _path: &Path,
        _delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        // Default implementation just checks file exists
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_manifest_name() {
        let name = ManifestName::new("Cargo.toml");
        assert_eq!(name.as_str(), "Cargo.toml");
        assert_eq!(name.to_string(), "Cargo.toml");

        let name2: ManifestName = "package.json".into();
        assert_eq!(name2.as_str(), "package.json");
    }

    #[tokio::test]
    async fn test_fs_delegate() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "test content").await.unwrap();

        let delegate = FsDelegate;

        // Test file exists
        assert!(delegate.exists(&file_path, Some(false)).await);
        assert!(!delegate.exists(&file_path, Some(true)).await);

        // Test directory exists
        assert!(delegate.exists(temp_dir.path(), Some(true)).await);
        assert!(!delegate.exists(temp_dir.path(), Some(false)).await);

        // Test read content
        let content = delegate.read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "test content");

        // Test metadata
        let metadata = delegate.metadata(&file_path).await.unwrap();
        assert!(metadata.is_file());

        // Test accessibility
        assert!(delegate.is_accessible(&file_path).await);
        assert!(
            !delegate
                .is_accessible(&temp_dir.path().join("nonexistent"))
                .await
        );
    }

    #[test]
    fn wsl_manifest_delegate_supports_wsl_unc_paths() {
        assert!(WslManifestDelegate::supports(Path::new(
            r"\\wsl.localhost\Ubuntu\home\iain\repo\Cargo.toml"
        )));
        assert!(!WslManifestDelegate::supports(Path::new(
            r"C:\Users\iain\repo\Cargo.toml"
        )));
    }

    #[test]
    fn wsl_manifest_test_command_uses_distribution_and_linux_path() {
        let command = wsl_manifest_test_command(
            Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo\Cargo.toml"),
            Some(false),
        )
        .expect("build WSL manifest test command");
        let command_debug = format!("{command:?}");

        assert!(command_debug.contains("wsl.exe"));
        assert!(command_debug.contains("Ubuntu"));
        assert!(command_debug.contains("test -f"));
        assert!(command_debug.contains("/home/iain/repo/Cargo.toml"));
    }

    #[test]
    fn wsl_manifest_read_command_quotes_linux_path() {
        let command = wsl_manifest_read_command(Path::new(
            r"\\wsl.localhost\Ubuntu\home\iain\repo with spaces\Cargo.toml",
        ))
        .expect("build WSL manifest read command");
        let command_debug = format!("{command:?}");

        assert!(command_debug.contains("cat --"));
        assert!(command_debug.contains("'/home/iain/repo with spaces/Cargo.toml'"));
    }

    #[test]
    fn wsl_manifest_delegate_caches_existence_results() {
        let delegate = WslManifestDelegate::default();
        let path = Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo");

        assert_eq!(delegate.cached_exists(path, Some(true)), None);
        delegate.store_exists(path, Some(true), true);
        assert_eq!(delegate.cached_exists(path, Some(true)), Some(true));
        assert_eq!(delegate.cached_exists(path, Some(false)), None);
    }

    #[tokio::test]
    async fn test_base_manifest_provider() {
        let temp_dir = TempDir::new().unwrap();
        let manifest_path = temp_dir.path().join("test.manifest");
        tokio::fs::write(&manifest_path, "test").await.unwrap();

        let provider =
            BaseManifestProvider::new("test.manifest", vec!["test.manifest".to_string()]);

        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(manifest_path.clone(), 10, delegate);

        let result = provider.search_patterns(&query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_project_metadata() {
        let metadata = ProjectMetadata {
            name: Some("test-project".to_string()),
            version: Some("1.0.0".to_string()),
            description: Some("A test project".to_string()),
            language: "rust".to_string(),
            dependencies: vec!["serde".to_string()],
            dev_dependencies: vec!["tokio-test".to_string()],
            additional_info: {
                let mut map = HashMap::new();
                map.insert("edition".to_string(), "2021".to_string());
                map
            },
        };

        assert_eq!(metadata.name.as_ref().unwrap(), "test-project");
        assert_eq!(metadata.language, "rust");
        assert_eq!(metadata.dependencies.len(), 1);
        assert_eq!(metadata.additional_info.get("edition").unwrap(), "2021");
    }
}
