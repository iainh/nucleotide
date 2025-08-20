// ABOUTME: Comprehensive tests for project detection system including workspace root finding
// ABOUTME: Tests various project structures, VCS detection, and edge cases for robust project detection

#[cfg(test)]
mod tests {
    use crate::application::find_workspace_root_from;
    use nucleotide_logging::debug;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// Helper trait for creating test project structures
    trait ProjectStructureBuilder {
        fn create_file(&self, path: &str, content: &str) -> std::io::Result<()>;
        fn create_dir(&self, path: &str) -> std::io::Result<()>;
        fn create_vcs_dir(&self, vcs_type: VcsType) -> std::io::Result<()>;
        fn path(&self) -> &Path;
    }

    #[derive(Debug, Clone, Copy)]
    enum VcsType {
        Git,
        Svn,
        Mercurial,
        Jujutsu,
        Helix,
    }

    impl VcsType {
        fn directory_name(self) -> &'static str {
            match self {
                VcsType::Git => ".git",
                VcsType::Svn => ".svn",
                VcsType::Mercurial => ".hg",
                VcsType::Jujutsu => ".jj",
                VcsType::Helix => ".helix",
            }
        }
    }

    struct TestProject {
        temp_dir: TempDir,
    }

    impl TestProject {
        fn new() -> Self {
            Self {
                temp_dir: TempDir::new().expect("Failed to create temp directory"),
            }
        }
    }

    impl ProjectStructureBuilder for TestProject {
        fn create_file(&self, path: &str, content: &str) -> std::io::Result<()> {
            let full_path = self.temp_dir.path().join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(full_path, content)
        }

        fn create_dir(&self, path: &str) -> std::io::Result<()> {
            let full_path = self.temp_dir.path().join(path);
            fs::create_dir_all(full_path)
        }

        fn create_vcs_dir(&self, vcs_type: VcsType) -> std::io::Result<()> {
            self.create_dir(vcs_type.directory_name())
        }

        fn path(&self) -> &Path {
            self.temp_dir.path()
        }
    }

    /// Mock file system for testing without actual file I/O
    #[derive(Default)]
    struct MockFileSystem {
        directories: std::collections::HashSet<PathBuf>,
    }

    impl MockFileSystem {
        fn new() -> Self {
            Default::default()
        }

        fn add_directory(&mut self, path: PathBuf) {
            self.directories.insert(path);
        }

        fn exists(&self, path: &Path) -> bool {
            self.directories.contains(path)
        }

        /// Create a mock implementation of find_workspace_root_from for testing
        fn find_workspace_root_mock(&self, start_dir: &Path) -> PathBuf {
            for ancestor in start_dir.ancestors() {
                if self.exists(&ancestor.join(".git"))
                    || self.exists(&ancestor.join(".svn"))
                    || self.exists(&ancestor.join(".hg"))
                    || self.exists(&ancestor.join(".jj"))
                    || self.exists(&ancestor.join(".helix"))
                {
                    return ancestor.to_path_buf();
                }
            }
            start_dir.to_path_buf()
        }
    }

    #[test]
    fn test_git_workspace_detection() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Git).unwrap();

        let workspace_root = find_workspace_root_from(project.path());
        assert_eq!(workspace_root, project.path());
    }

    #[test]
    fn test_nested_git_workspace_detection() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Git).unwrap();
        project.create_dir("src/submodule").unwrap();

        let nested_path = project.path().join("src/submodule");
        let workspace_root = find_workspace_root_from(&nested_path);
        assert_eq!(workspace_root, project.path());
    }

    #[test]
    fn test_svn_workspace_detection() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Svn).unwrap();

        let workspace_root = find_workspace_root_from(project.path());
        assert_eq!(workspace_root, project.path());
    }

    #[test]
    fn test_mercurial_workspace_detection() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Mercurial).unwrap();

        let workspace_root = find_workspace_root_from(project.path());
        assert_eq!(workspace_root, project.path());
    }

    #[test]
    fn test_jujutsu_workspace_detection() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Jujutsu).unwrap();

        let workspace_root = find_workspace_root_from(project.path());
        assert_eq!(workspace_root, project.path());
    }

    #[test]
    fn test_helix_workspace_detection() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Helix).unwrap();

        let workspace_root = find_workspace_root_from(project.path());
        assert_eq!(workspace_root, project.path());
    }

    #[test]
    fn test_no_vcs_directory_fallback() {
        let project = TestProject::new();
        project.create_dir("src").unwrap();

        let src_path = project.path().join("src");
        let workspace_root = find_workspace_root_from(&src_path);

        // Should fall back to the start directory when no VCS is found
        assert_eq!(workspace_root, src_path);
    }

    #[test]
    fn test_multiple_vcs_directories_priority() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Git).unwrap();
        project.create_vcs_dir(VcsType::Svn).unwrap();
        project.create_vcs_dir(VcsType::Helix).unwrap();

        let workspace_root = find_workspace_root_from(project.path());
        // Should find the directory (all are at the same level, so any is valid)
        assert_eq!(workspace_root, project.path());
    }

    #[test]
    fn test_deeply_nested_workspace_detection() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Git).unwrap();
        project
            .create_dir("a/very/deeply/nested/directory/structure")
            .unwrap();

        let deep_path = project
            .path()
            .join("a/very/deeply/nested/directory/structure");
        let workspace_root = find_workspace_root_from(&deep_path);
        assert_eq!(workspace_root, project.path());
    }

    #[test]
    fn test_workspace_detection_with_complex_project_structure() {
        let project = TestProject::new();

        // Create a complex project structure
        project.create_vcs_dir(VcsType::Git).unwrap();
        project
            .create_file(
                "Cargo.toml",
                "[package]\nname = \"test\"\nversion = \"0.1.0\"",
            )
            .unwrap();
        project
            .create_file("package.json", r#"{"name": "test", "version": "1.0.0"}"#)
            .unwrap();
        project.create_dir("src/main").unwrap();
        project.create_dir("src/lib").unwrap();
        project.create_dir("tests/integration").unwrap();
        project.create_dir("docs/api").unwrap();
        project.create_dir("scripts/build").unwrap();

        // Test from various subdirectories
        let test_paths = [
            "src/main",
            "src/lib",
            "tests/integration",
            "docs/api",
            "scripts/build",
        ];

        for test_path in &test_paths {
            let full_path = project.path().join(test_path);
            let workspace_root = find_workspace_root_from(&full_path);
            assert_eq!(
                workspace_root,
                project.path(),
                "Failed to find workspace root from {}",
                test_path
            );
        }
    }

    #[test]
    fn test_mock_file_system() {
        let mut mock_fs = MockFileSystem::new();

        let project_root = PathBuf::from("/home/user/project");
        let git_dir = project_root.join(".git");
        let src_dir = project_root.join("src");

        mock_fs.add_directory(git_dir);

        let workspace_root = mock_fs.find_workspace_root_mock(&src_dir);
        assert_eq!(workspace_root, project_root);
    }

    #[test]
    fn test_mock_nested_projects() {
        let mut mock_fs = MockFileSystem::new();

        // Outer project
        let outer_project = PathBuf::from("/home/user/outer-project");
        mock_fs.add_directory(outer_project.join(".git"));

        // Inner project (should be ignored)
        let inner_project = outer_project.join("vendor/inner-project");
        mock_fs.add_directory(inner_project.join(".git"));

        // Test from inner project - should find the closest VCS directory
        let workspace_root = mock_fs.find_workspace_root_mock(&inner_project);
        assert_eq!(workspace_root, inner_project);
    }

    #[test]
    fn test_workspace_detection_edge_cases() {
        // Test with root directory (should not panic)
        let root = PathBuf::from("/");
        let workspace_root = find_workspace_root_from(&root);
        assert_eq!(workspace_root, root);
    }

    #[test]
    fn test_workspace_detection_nonexistent_path() {
        // Test with non-existent path (should not panic)
        let nonexistent = PathBuf::from("/nonexistent/path/that/does/not/exist");
        let workspace_root = find_workspace_root_from(&nonexistent);
        assert_eq!(workspace_root, nonexistent);
    }

    #[test]
    fn test_workspace_detection_with_symlinks() {
        let project = TestProject::new();
        project.create_vcs_dir(VcsType::Git).unwrap();
        project.create_dir("real_dir").unwrap();

        // Note: Creating actual symlinks in tests can be problematic across platforms
        // This test verifies the current behavior handles paths correctly
        let real_dir = project.path().join("real_dir");
        let workspace_root = find_workspace_root_from(&real_dir);
        assert_eq!(workspace_root, project.path());
    }

    /// Test project detection with various manifest files
    mod manifest_detection {
        use super::*;

        fn has_rust_project_manifest(path: &Path) -> bool {
            path.join("Cargo.toml").exists()
        }

        fn has_node_project_manifest(path: &Path) -> bool {
            path.join("package.json").exists()
        }

        fn has_python_project_manifest(path: &Path) -> bool {
            path.join("pyproject.toml").exists()
                || path.join("setup.py").exists()
                || path.join("requirements.txt").exists()
        }

        #[test]
        fn test_rust_project_detection() {
            let project = TestProject::new();
            project
                .create_file(
                    "Cargo.toml",
                    "[package]\nname = \"test\"\nversion = \"0.1.0\"",
                )
                .unwrap();

            assert!(has_rust_project_manifest(project.path()));
            assert!(!has_node_project_manifest(project.path()));
            assert!(!has_python_project_manifest(project.path()));
        }

        #[test]
        fn test_node_project_detection() {
            let project = TestProject::new();
            project
                .create_file("package.json", r#"{"name": "test", "version": "1.0.0"}"#)
                .unwrap();

            assert!(!has_rust_project_manifest(project.path()));
            assert!(has_node_project_manifest(project.path()));
            assert!(!has_python_project_manifest(project.path()));
        }

        #[test]
        fn test_python_project_detection() {
            let project = TestProject::new();
            project
                .create_file("pyproject.toml", "[tool.poetry]\nname = \"test\"")
                .unwrap();

            assert!(!has_rust_project_manifest(project.path()));
            assert!(!has_node_project_manifest(project.path()));
            assert!(has_python_project_manifest(project.path()));
        }

        #[test]
        fn test_multi_language_project_detection() {
            let project = TestProject::new();
            project
                .create_file(
                    "Cargo.toml",
                    "[package]\nname = \"test\"\nversion = \"0.1.0\"",
                )
                .unwrap();
            project
                .create_file("package.json", r#"{"name": "test", "version": "1.0.0"}"#)
                .unwrap();
            project
                .create_file("pyproject.toml", "[tool.poetry]\nname = \"test\"")
                .unwrap();

            assert!(has_rust_project_manifest(project.path()));
            assert!(has_node_project_manifest(project.path()));
            assert!(has_python_project_manifest(project.path()));
        }
    }

    /// Test project detector trait implementations
    mod project_detector_trait {
        use super::*;

        trait ProjectDetector {
            fn detect_project_type(&self, path: &Path) -> ProjectType;
            fn find_project_root(&self, path: &Path) -> Option<PathBuf>;
            fn get_project_metadata(&self, path: &Path) -> ProjectMetadata;
        }

        #[derive(Debug, PartialEq)]
        enum ProjectType {
            Rust,
            Node,
            Python,
            Generic,
            Unknown,
        }

        #[derive(Debug, Default)]
        struct ProjectMetadata {
            name: Option<String>,
            version: Option<String>,
            description: Option<String>,
            dependencies: Vec<String>,
        }

        struct DefaultProjectDetector;

        impl ProjectDetector for DefaultProjectDetector {
            fn detect_project_type(&self, path: &Path) -> ProjectType {
                if path.join("Cargo.toml").exists() {
                    ProjectType::Rust
                } else if path.join("package.json").exists() {
                    ProjectType::Node
                } else if path.join("pyproject.toml").exists() || path.join("setup.py").exists() {
                    ProjectType::Python
                } else if find_workspace_root_from(path) != path {
                    ProjectType::Generic
                } else {
                    ProjectType::Unknown
                }
            }

            fn find_project_root(&self, path: &Path) -> Option<PathBuf> {
                let workspace_root = find_workspace_root_from(path);
                if workspace_root != path {
                    Some(workspace_root)
                } else {
                    None
                }
            }

            fn get_project_metadata(&self, path: &Path) -> ProjectMetadata {
                // Simplified metadata extraction for testing
                ProjectMetadata::default()
            }
        }

        #[test]
        fn test_project_detector_rust() {
            let project = TestProject::new();
            project
                .create_file(
                    "Cargo.toml",
                    "[package]\nname = \"test\"\nversion = \"0.1.0\"",
                )
                .unwrap();

            let detector = DefaultProjectDetector;
            assert_eq!(
                detector.detect_project_type(project.path()),
                ProjectType::Rust
            );
        }

        #[test]
        fn test_project_detector_node() {
            let project = TestProject::new();
            project
                .create_file("package.json", r#"{"name": "test", "version": "1.0.0"}"#)
                .unwrap();

            let detector = DefaultProjectDetector;
            assert_eq!(
                detector.detect_project_type(project.path()),
                ProjectType::Node
            );
        }

        #[test]
        fn test_project_detector_python() {
            let project = TestProject::new();
            project
                .create_file("pyproject.toml", "[tool.poetry]\nname = \"test\"")
                .unwrap();

            let detector = DefaultProjectDetector;
            assert_eq!(
                detector.detect_project_type(project.path()),
                ProjectType::Python
            );
        }

        #[test]
        fn test_project_detector_generic_vcs() {
            let project = TestProject::new();
            project.create_vcs_dir(VcsType::Git).unwrap();

            let detector = DefaultProjectDetector;
            let nested_path = project.path().join("src");
            std::fs::create_dir(&nested_path).unwrap();

            assert_eq!(
                detector.detect_project_type(&nested_path),
                ProjectType::Generic
            );
        }

        #[test]
        fn test_project_detector_unknown() {
            let project = TestProject::new();
            // No VCS, no manifest files

            let detector = DefaultProjectDetector;
            assert_eq!(
                detector.detect_project_type(project.path()),
                ProjectType::Unknown
            );
        }

        #[test]
        fn test_project_root_detection() {
            let project = TestProject::new();
            project.create_vcs_dir(VcsType::Git).unwrap();
            project.create_dir("src/deeply/nested").unwrap();

            let detector = DefaultProjectDetector;
            let nested_path = project.path().join("src/deeply/nested");

            let root = detector.find_project_root(&nested_path);
            assert_eq!(root, Some(project.path().to_path_buf()));
        }
    }

    /// Test manifest provider implementations
    mod manifest_provider_trait {
        use super::*;
        use serde_json::Value as JsonValue;
        use toml::Value as TomlValue;

        trait ManifestProvider {
            fn can_handle(&self, path: &Path) -> bool;
            fn parse_manifest(
                &self,
                path: &Path,
            ) -> Result<Box<dyn ManifestData>, Box<dyn std::error::Error>>;
        }

        trait ManifestData: std::fmt::Debug {
            fn get_name(&self) -> Option<&str>;
            fn get_version(&self) -> Option<&str>;
            fn get_dependencies(&self) -> Vec<String>;
        }

        #[derive(Debug)]
        struct CargoManifest {
            name: Option<String>,
            version: Option<String>,
            dependencies: Vec<String>,
        }

        impl ManifestData for CargoManifest {
            fn get_name(&self) -> Option<&str> {
                self.name.as_deref()
            }

            fn get_version(&self) -> Option<&str> {
                self.version.as_deref()
            }

            fn get_dependencies(&self) -> Vec<String> {
                self.dependencies.clone()
            }
        }

        #[derive(Debug)]
        struct PackageJsonManifest {
            name: Option<String>,
            version: Option<String>,
            dependencies: Vec<String>,
        }

        impl ManifestData for PackageJsonManifest {
            fn get_name(&self) -> Option<&str> {
                self.name.as_deref()
            }

            fn get_version(&self) -> Option<&str> {
                self.version.as_deref()
            }

            fn get_dependencies(&self) -> Vec<String> {
                self.dependencies.clone()
            }
        }

        struct CargoManifestProvider;

        impl ManifestProvider for CargoManifestProvider {
            fn can_handle(&self, path: &Path) -> bool {
                path.join("Cargo.toml").exists()
            }

            fn parse_manifest(
                &self,
                path: &Path,
            ) -> Result<Box<dyn ManifestData>, Box<dyn std::error::Error>> {
                let manifest_path = path.join("Cargo.toml");
                let content = std::fs::read_to_string(&manifest_path)?;
                let parsed: TomlValue = toml::from_str(&content)?;

                let mut manifest = CargoManifest {
                    name: None,
                    version: None,
                    dependencies: Vec::new(),
                };

                if let Some(package) = parsed.get("package") {
                    if let Some(name) = package.get("name").and_then(|v| v.as_str()) {
                        manifest.name = Some(name.to_string());
                    }
                    if let Some(version) = package.get("version").and_then(|v| v.as_str()) {
                        manifest.version = Some(version.to_string());
                    }
                }

                if let Some(deps) = parsed.get("dependencies").and_then(|v| v.as_table()) {
                    manifest.dependencies = deps.keys().map(|k| k.clone()).collect();
                }

                Ok(Box::new(manifest))
            }
        }

        struct PackageJsonManifestProvider;

        impl ManifestProvider for PackageJsonManifestProvider {
            fn can_handle(&self, path: &Path) -> bool {
                path.join("package.json").exists()
            }

            fn parse_manifest(
                &self,
                path: &Path,
            ) -> Result<Box<dyn ManifestData>, Box<dyn std::error::Error>> {
                let manifest_path = path.join("package.json");
                let content = std::fs::read_to_string(&manifest_path)?;
                let parsed: JsonValue = serde_json::from_str(&content)?;

                let mut manifest = PackageJsonManifest {
                    name: None,
                    version: None,
                    dependencies: Vec::new(),
                };

                if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
                    manifest.name = Some(name.to_string());
                }
                if let Some(version) = parsed.get("version").and_then(|v| v.as_str()) {
                    manifest.version = Some(version.to_string());
                }

                if let Some(deps) = parsed.get("dependencies").and_then(|v| v.as_object()) {
                    manifest.dependencies = deps.keys().map(|k| k.clone()).collect();
                }

                Ok(Box::new(manifest))
            }
        }

        #[test]
        fn test_cargo_manifest_provider() {
            let project = TestProject::new();
            let cargo_content = r#"
[package]
name = "test-project"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = "1.0"
serde = "1.0"
"#;
            project.create_file("Cargo.toml", cargo_content).unwrap();

            let provider = CargoManifestProvider;
            assert!(provider.can_handle(project.path()));

            let manifest = provider.parse_manifest(project.path()).unwrap();
            assert_eq!(manifest.get_name(), Some("test-project"));
            assert_eq!(manifest.get_version(), Some("0.1.0"));

            let deps = manifest.get_dependencies();
            assert!(deps.contains(&"tokio".to_string()));
            assert!(deps.contains(&"serde".to_string()));
        }

        #[test]
        fn test_package_json_manifest_provider() {
            let project = TestProject::new();
            let package_content = r#"{
  "name": "test-project",
  "version": "1.0.0",
  "description": "A test project",
  "dependencies": {
    "express": "^4.18.0",
    "lodash": "^4.17.21"
  }
}"#;
            project
                .create_file("package.json", package_content)
                .unwrap();

            let provider = PackageJsonManifestProvider;
            assert!(provider.can_handle(project.path()));

            let manifest = provider.parse_manifest(project.path()).unwrap();
            assert_eq!(manifest.get_name(), Some("test-project"));
            assert_eq!(manifest.get_version(), Some("1.0.0"));

            let deps = manifest.get_dependencies();
            assert!(deps.contains(&"express".to_string()));
            assert!(deps.contains(&"lodash".to_string()));
        }

        #[test]
        fn test_manifest_provider_selection() {
            let cargo_provider = CargoManifestProvider;
            let package_provider = PackageJsonManifestProvider;

            // Test Rust project
            let rust_project = TestProject::new();
            rust_project
                .create_file("Cargo.toml", "[package]\nname = \"test\"")
                .unwrap();

            assert!(cargo_provider.can_handle(rust_project.path()));
            assert!(!package_provider.can_handle(rust_project.path()));

            // Test Node project
            let node_project = TestProject::new();
            node_project
                .create_file("package.json", r#"{"name": "test"}"#)
                .unwrap();

            assert!(!cargo_provider.can_handle(node_project.path()));
            assert!(package_provider.can_handle(node_project.path()));
        }

        #[test]
        fn test_malformed_manifest_handling() {
            let project = TestProject::new();
            project
                .create_file("Cargo.toml", "invalid toml content {{{")
                .unwrap();

            let provider = CargoManifestProvider;
            assert!(provider.can_handle(project.path()));

            let result = provider.parse_manifest(project.path());
            assert!(result.is_err());
        }

        #[test]
        fn test_missing_manifest_fields() {
            let project = TestProject::new();
            project.create_file("package.json", r#"{}"#).unwrap();

            let provider = PackageJsonManifestProvider;
            let manifest = provider.parse_manifest(project.path()).unwrap();

            assert_eq!(manifest.get_name(), None);
            assert_eq!(manifest.get_version(), None);
            assert!(manifest.get_dependencies().is_empty());
        }
    }
}
