// ABOUTME: Project detection and manifest provider system for Nucleotide
// ABOUTME: Provides language-specific project root detection with efficient ancestor traversal

use std::path::Path;

pub mod error;
pub mod manifest;
pub mod providers;
pub mod registry;
pub mod utils;

pub use error::{ProjectError, Result};
pub use manifest::{ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery};
pub use registry::ManifestProviders;

/// Main entry point for project detection
///
/// This function attempts to detect the project root for a given file path
/// by querying all registered manifest providers.
pub async fn detect_project_root(
    file_path: &Path,
    max_depth: Option<usize>,
) -> Result<Option<std::path::PathBuf>> {
    let providers = ManifestProviders::global();
    providers.detect_project_root(file_path, max_depth).await
}

/// Convenience function to detect project type
///
/// Returns the manifest name (e.g., "Cargo.toml", "package.json") if a project is detected
pub async fn detect_project_type(
    file_path: &Path,
    max_depth: Option<usize>,
) -> Result<Option<ManifestName>> {
    let providers = ManifestProviders::global();
    providers.detect_project_type(file_path, max_depth).await
}

/// Register all built-in providers
///
/// This function should be called during application initialization
/// to register all the standard language providers.
pub fn register_builtin_providers() {
    let providers = ManifestProviders::global();

    providers.register(Box::new(providers::RustManifestProvider::new()));
    providers.register(Box::new(providers::PythonManifestProvider::new()));
    providers.register(Box::new(providers::TypeScriptManifestProvider::new()));
    providers.register(Box::new(providers::GoManifestProvider::new()));
    providers.register(Box::new(providers::JavaManifestProvider::new()));
    providers.register(Box::new(providers::CSharpManifestProvider::new()));
    providers.register(Box::new(providers::CppManifestProvider::new()));

    nucleotide_logging::info!("Registered built-in manifest providers");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn test_project_detection_integration() {
        let temp_dir = TempDir::new().unwrap();
        let rust_project = temp_dir.path().join("rust_project");
        fs::create_dir_all(&rust_project).await.unwrap();

        // Create a Cargo.toml file
        let cargo_toml = rust_project.join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"test\"\nversion = \"0.1.0\"",
        )
        .await
        .unwrap();

        // Create a nested file
        let src_dir = rust_project.join("src");
        fs::create_dir_all(&src_dir).await.unwrap();
        let main_rs = src_dir.join("main.rs");
        fs::write(&main_rs, "fn main() {}").await.unwrap();

        register_builtin_providers();

        // Test detection from nested file
        let detected_root = detect_project_root(&main_rs, None).await.unwrap();
        assert!(detected_root.is_some());
        assert_eq!(detected_root.unwrap(), rust_project);

        // Test project type detection
        let project_type = detect_project_type(&main_rs, None).await.unwrap();
        assert!(project_type.is_some());
        assert_eq!(project_type.unwrap().as_str(), "Cargo.toml");
    }
}
