// ABOUTME: Integration tests for end-to-end project detection and configuration workflows
// ABOUTME: Tests complete project setup scenarios, error recovery, and real-world project structures

#[cfg(test)]
#[allow(clippy::assertions_on_constants)]
mod tests {
    use crate::application::find_workspace_root_from;
    use crate::config::Config;
    use nucleotide_logging::debug;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    /// Integration test builder for creating realistic project scenarios
    struct IntegrationTestProject {
        temp_dir: TempDir,
        #[allow(dead_code)]
        name: String,
    }

    impl IntegrationTestProject {
        fn new(name: &str) -> Self {
            Self {
                temp_dir: TempDir::new().expect("Failed to create temp directory"),
                name: name.to_string(),
            }
        }

        fn create_rust_workspace(&self) -> std::io::Result<()> {
            // Create workspace Cargo.toml
            fs::write(
                self.path().join("Cargo.toml"),
                r#"[workspace]
resolver = "2"
members = [
    "crates/core",
    "crates/ui", 
    "crates/cli"
]

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["Test Author <test@example.com>"]

[workspace.dependencies]
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
"#,
            )?;

            // Create workspace members
            self.create_rust_crate("crates/core", "nucleotide-core")?;
            self.create_rust_crate("crates/ui", "nucleotide-ui")?;
            self.create_rust_crate("crates/cli", "nucleotide-cli")?;

            // Create additional files
            fs::write(self.path().join("README.md"), "# Test Workspace")?;
            fs::write(self.path().join(".gitignore"), "/target/\n/Cargo.lock\n")?;

            Ok(())
        }

        fn create_rust_crate(&self, path: &str, name: &str) -> std::io::Result<()> {
            let crate_path = self.path().join(path);
            fs::create_dir_all(&crate_path)?;
            fs::create_dir_all(crate_path.join("src"))?;

            fs::write(
                crate_path.join("Cargo.toml"),
                format!(
                    r#"[package]
name = "{}"
version.workspace = true
edition.workspace = true
authors.workspace = true

[dependencies]
tokio.workspace = true
serde.workspace = true
"#,
                    name
                ),
            )?;

            fs::write(crate_path.join("src").join("lib.rs"), "// Library crate")?;
            Ok(())
        }

        fn add_vcs(&self, vcs_type: &str) -> std::io::Result<()> {
            let vcs_dir = match vcs_type {
                "git" => ".git",
                "svn" => ".svn",
                "hg" => ".hg",
                "jj" => ".jj",
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Unknown VCS type",
                    ));
                }
            };

            fs::create_dir_all(self.path().join(vcs_dir))?;

            // Add some VCS-like files
            if vcs_type == "git" {
                fs::write(
                    self.path().join(".git").join("HEAD"),
                    "ref: refs/heads/main\n",
                )?;
                fs::create_dir_all(self.path().join(".git").join("refs").join("heads"))?;
                fs::write(
                    self.path()
                        .join(".git")
                        .join("refs")
                        .join("heads")
                        .join("main"),
                    "0000000000000000000000000000000000000000\n",
                )?;
            }

            Ok(())
        }

        fn add_nucleotide_config(&self, config_content: &str) -> std::io::Result<()> {
            fs::write(self.path().join("nucleotide.toml"), config_content)
        }

        fn path(&self) -> &Path {
            self.temp_dir.path()
        }

        fn get_subdirectory(&self, subpath: &str) -> PathBuf {
            self.path().join(subpath)
        }
    }

    #[test]
    fn test_rust_workspace_detection() {
        let project = IntegrationTestProject::new("rust-workspace");
        project.create_rust_workspace().unwrap();
        project.add_vcs("git").unwrap();

        // Test workspace root detection from various locations
        let test_locations = [
            "",                // Root
            "crates",          // Crates directory
            "crates/core",     // Individual crate
            "crates/core/src", // Deep in crate
            "crates/ui/src",   // Different crate
        ];

        for location in &test_locations {
            let test_path = if location.is_empty() {
                project.path().to_path_buf()
            } else {
                project.get_subdirectory(location)
            };

            let workspace_root = find_workspace_root_from(&test_path);

            assert_eq!(
                workspace_root,
                project.path(),
                "Failed to find workspace root from location: {}",
                location
            );
        }
    }

    #[test]
    #[ignore = "Requires Helix runtime files to be available"]
    fn test_end_to_end_configuration_loading() {
        let project = IntegrationTestProject::new("config-test");
        project.add_vcs("git").unwrap();

        // Add nucleotide configuration
        let nucleotide_config = r#"
[ui.font]
family = "JetBrains Mono"
size = 13.0
weight = "medium"

[editor.font]
family = "Fira Code"
size = 14.0
weight = "normal"

[ui]
look = "system"

[theme]
mode = "dark"
dark_theme = "tokyo_night"
light_theme = "github_light"

[window]
blur_dark_themes = true
appearance_follows_theme = true
"#;
        project.add_nucleotide_config(nucleotide_config).unwrap();

        // Load configuration from project directory
        let config = Config::load_from_dir(project.path()).expect("Failed to load config");

        // Verify GUI configuration was loaded correctly
        let ui_font = config.gui.ui.font.expect("UI font should be loaded");
        assert_eq!(ui_font.family, "JetBrains Mono");
        assert_eq!(ui_font.size, 13.0);

        let editor_font = config
            .gui
            .editor
            .font
            .expect("Editor font should be loaded");
        assert_eq!(editor_font.family, "Fira Code");
        assert_eq!(editor_font.size, 14.0);

        assert_eq!(config.gui.theme.mode, crate::config::ThemeMode::Dark);
        assert_eq!(config.gui.theme.get_dark_theme(), "tokyo_night");
        assert_eq!(config.gui.theme.get_light_theme(), "github_light");

        assert!(config.gui.window.blur_dark_themes);
        assert!(config.gui.window.appearance_follows_theme);
        assert_eq!(config.gui.ui.look, crate::config::UiLook::System);
    }

    #[test]
    fn test_nested_vcs_priority() {
        let project = IntegrationTestProject::new("nested-vcs");

        // Create outer git repository
        project.add_vcs("git").unwrap();

        // Create inner project with its own git repo
        let inner_path = project.get_subdirectory("vendor/external-lib");
        fs::create_dir_all(&inner_path).unwrap();
        fs::create_dir_all(inner_path.join(".git")).unwrap();

        // Test from inner directory - should find closest VCS root
        let workspace_root = find_workspace_root_from(&inner_path);
        assert_eq!(workspace_root, inner_path);

        // Test from a subdirectory of inner project
        let inner_src = inner_path.join("src");
        fs::create_dir_all(&inner_src).unwrap();
        let workspace_root = find_workspace_root_from(&inner_src);
        assert_eq!(workspace_root, inner_path);
    }

    #[test]
    fn test_project_without_vcs() {
        let project = IntegrationTestProject::new("no-vcs");
        project.create_rust_crate(".", "standalone-crate").unwrap();

        // No VCS directory added

        let test_path = project.get_subdirectory("src");
        let workspace_root = find_workspace_root_from(&test_path);

        // Should fall back to the test path itself
        assert_eq!(workspace_root, test_path);
    }

    #[test]
    fn test_different_vcs_types() {
        let vcs_types = ["git", "svn", "hg", "jj"];

        for vcs_type in &vcs_types {
            let project = IntegrationTestProject::new(&format!("{}-project", vcs_type));
            project.create_rust_crate(".", "test-crate").unwrap();
            project.add_vcs(vcs_type).unwrap();

            let test_path = project.get_subdirectory("src");
            let workspace_root = find_workspace_root_from(&test_path);

            assert_eq!(
                workspace_root,
                project.path(),
                "Failed to detect {} repository",
                vcs_type
            );
        }
    }

    /// Test error conditions and recovery mechanisms  
    mod error_handling {
        use super::*;

        #[test]
        fn test_inaccessible_directory() {
            // Test behavior when trying to detect workspace root from a non-existent directory
            let project = IntegrationTestProject::new("access-test");
            project.add_vcs("git").unwrap();

            let deep_path = project.get_subdirectory("this/path/does/not/exist");

            // Should not panic, even if the path doesn't exist
            let workspace_root = find_workspace_root_from(&deep_path);
            // The function should climb up and find the VCS root (project directory)
            assert_eq!(workspace_root, project.path());
        }
    }

    /// Test performance characteristics
    mod performance_tests {
        use super::*;
        use std::time::Instant;

        #[test]
        fn test_workspace_detection_performance() {
            let project = IntegrationTestProject::new("performance-test");
            project.add_vcs("git").unwrap();

            // Create a very deep directory structure
            let deep_path = (0..50).fold(project.path().to_path_buf(), |path, i| {
                let new_path = path.join(format!("level_{}", i));
                fs::create_dir_all(&new_path).unwrap();
                new_path
            });

            // Time the workspace detection
            let start = Instant::now();
            let workspace_root = find_workspace_root_from(&deep_path);
            let duration = start.elapsed();

            assert_eq!(workspace_root, project.path());

            // Should complete quickly (under 100ms for 50 levels)
            assert!(
                duration.as_millis() < 100,
                "Workspace detection took too long: {:?}",
                duration
            );

            debug!("Workspace detection took: {:?}", duration);
        }

        #[test]
        #[ignore = "Requires Helix runtime files to be available"]
        fn test_config_loading_performance() {
            let project = IntegrationTestProject::new("config-performance");

            // Create a large configuration file
            let mut large_config = String::from(
                r#"
[ui.font]
family = "Test Font"
size = 14.0

[editor.font] 
family = "Editor Font"
size = 16.0

[theme]
mode = "dark"
"#,
            );

            // Add many theme entries to make config larger
            for i in 0..1000 {
                large_config.push_str(&format!(
                    "\n# Theme option {}\n# This is a comment for theme {}\n",
                    i, i
                ));
            }

            project.add_nucleotide_config(&large_config).unwrap();

            // Time the configuration loading
            let start = Instant::now();
            let config = Config::load_from_dir(project.path()).unwrap();
            let duration = start.elapsed();

            // Verify config was loaded correctly
            assert_eq!(config.gui.ui.font.as_ref().unwrap().family, "Test Font");

            // Should complete quickly (under 50ms for large config)
            assert!(
                duration.as_millis() < 50,
                "Config loading took too long: {:?}",
                duration
            );

            debug!("Config loading took: {:?}", duration);
        }
    }

    /// Test real-world project scenarios
    mod realistic_scenarios {
        use super::*;

        #[test]
        #[ignore = "Requires Helix runtime files to be available"]
        fn test_project_with_documentation_and_tools() {
            let project = IntegrationTestProject::new("full-project");
            project.add_vcs("git").unwrap();
            project.create_rust_workspace().unwrap();

            // Add documentation
            fs::create_dir_all(project.get_subdirectory("docs")).unwrap();
            fs::write(
                project.path().join("docs").join("README.md"),
                "# Documentation\n",
            )
            .unwrap();

            // Add tools directory
            let tools_path = project.get_subdirectory("tools");
            fs::create_dir_all(&tools_path).unwrap();
            fs::write(
                tools_path.join("package.json"),
                r#"{"name": "build-tools", "version": "1.0.0"}"#,
            )
            .unwrap();

            // Add configuration
            project
                .add_nucleotide_config(
                    r#"
[ui.font]
family = "SF Pro"
size = 13.0

[theme] 
mode = "system"
"#,
                )
                .unwrap();

            // Test comprehensive setup
            let config = Config::load_from_dir(project.path()).unwrap();
            assert_eq!(config.gui.ui.font.as_ref().unwrap().family, "SF Pro");

            let workspace_root = find_workspace_root_from(&project.get_subdirectory("docs"));
            assert_eq!(workspace_root, project.path());

            let workspace_root = find_workspace_root_from(&tools_path);
            assert_eq!(workspace_root, project.path());
        }

        #[test]
        #[ignore = "Requires Helix runtime files to be available"]
        fn test_project_migration_scenario() {
            let project = IntegrationTestProject::new("migration-test");

            // Start as a simple project
            project.create_rust_crate(".", "simple-project").unwrap();

            // Add VCS later
            project.add_vcs("git").unwrap();

            // Convert to workspace
            fs::write(
                project.path().join("Cargo.toml"),
                r#"[workspace]
members = ["core", "cli"]

[workspace.package]
version = "0.2.0"
edition = "2021"
"#,
            )
            .unwrap();

            project.create_rust_crate("core", "project-core").unwrap();
            project.create_rust_crate("cli", "project-cli").unwrap();

            // Add configuration
            project
                .add_nucleotide_config(
                    r#"
[theme]
mode = "dark"
dark_theme = "custom_theme"
"#,
                )
                .unwrap();

            // Verify everything works after migration
            let config = Config::load_from_dir(project.path()).unwrap();
            assert_eq!(config.gui.theme.get_dark_theme(), "custom_theme");

            let workspace_root = find_workspace_root_from(&project.get_subdirectory("cli/src"));
            assert_eq!(workspace_root, project.path());
        }
    }
}
