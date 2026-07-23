// ABOUTME: Comprehensive tests for project detection system including workspace root finding
// ABOUTME: Tests various project structures, VCS detection, and edge cases for robust project detection

#[cfg(test)]
#[allow(clippy::map_clone)]
mod tests {
    use crate::application::find_workspace_root_from;

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
}
