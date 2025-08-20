// ABOUTME: Rust project manifest provider for Cargo.toml and workspace detection
// ABOUTME: Handles Rust projects with workspace support and proper manifest validation

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::error::{ProjectError, Result, WithPathContext};
use crate::manifest::{
    BaseManifestProvider, ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery,
    ProjectMetadata,
};

/// Manifest provider for Rust projects
pub struct RustManifestProvider {
    base: BaseManifestProvider,
}

impl RustManifestProvider {
    pub fn new() -> Self {
        Self {
            base: BaseManifestProvider::new("Cargo.toml", vec!["Cargo.toml".to_string()])
                .with_priority(150), // Higher priority for Rust projects
        }
    }
}

#[async_trait]
impl ManifestProvider for RustManifestProvider {
    fn name(&self) -> ManifestName {
        self.base.name.clone()
    }

    fn priority(&self) -> u32 {
        self.base.priority
    }

    fn file_patterns(&self) -> Vec<String> {
        self.base.file_patterns.clone()
    }

    async fn search(&self, query: ManifestQuery) -> Result<Option<PathBuf>> {
        // For Rust, we want to find the outermost Cargo.toml in a workspace
        let mut outermost_cargo_toml = None;

        for ancestor in query.path.ancestors().take(query.max_depth) {
            let cargo_toml_path = ancestor.join("Cargo.toml");

            if query.delegate.exists(&cargo_toml_path, Some(false)).await {
                nucleotide_logging::debug!(
                    cargo_toml = %cargo_toml_path.display(),
                    "Found Cargo.toml"
                );

                // Validate the manifest
                if self
                    .validate_manifest(&cargo_toml_path, &*query.delegate)
                    .await?
                {
                    outermost_cargo_toml = Some(ancestor.to_path_buf());

                    // Check if this is a workspace root by looking for workspace members
                    if is_workspace_root(&cargo_toml_path, &*query.delegate).await? {
                        nucleotide_logging::info!(
                            workspace_root = %ancestor.display(),
                            "Found Rust workspace root"
                        );
                        return Ok(outermost_cargo_toml);
                    }
                }
            }
        }

        if let Some(root) = &outermost_cargo_toml {
            nucleotide_logging::info!(
                project_root = %root.display(),
                "Found Rust project root"
            );
        }

        Ok(outermost_cargo_toml)
    }

    async fn validate_manifest(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        match toml::from_str::<CargoManifest>(&content) {
            Ok(manifest) => {
                nucleotide_logging::trace!(
                    cargo_toml = %path.display(),
                    has_package = manifest.package.is_some(),
                    has_workspace = manifest.workspace.is_some(),
                    "Validated Cargo.toml structure"
                );

                // Valid if it has either package or workspace section
                Ok(manifest.package.is_some() || manifest.workspace.is_some())
            }
            Err(e) => {
                nucleotide_logging::warn!(
                    cargo_toml = %path.display(),
                    error = %e,
                    "Invalid TOML in Cargo.toml"
                );
                Err(ProjectError::manifest_parse(path.to_path_buf(), e))
            }
        }
    }

    async fn get_project_metadata(
        &self,
        manifest_path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<ProjectMetadata> {
        let content = delegate.read_to_string(manifest_path).await?;
        let manifest: CargoManifest =
            toml::from_str(&content).with_path_context(manifest_path.to_path_buf())?;

        let mut metadata = ProjectMetadata {
            language: "rust".to_string(),
            ..Default::default()
        };

        // Extract package information
        if let Some(package) = manifest.package {
            metadata.name = Some(package.name);
            metadata.version = Some(package.version);
            metadata.description = package.description;

            if let Some(edition) = package.edition {
                metadata
                    .additional_info
                    .insert("edition".to_string(), edition);
            }
        }

        // Extract dependencies
        if let Some(deps) = manifest.dependencies {
            metadata.dependencies = deps.into_keys().collect();
        }

        if let Some(dev_deps) = manifest.dev_dependencies {
            metadata.dev_dependencies = dev_deps.into_keys().collect();
        }

        // Add workspace information
        if let Some(workspace) = manifest.workspace {
            if let Some(members) = workspace.members {
                metadata
                    .additional_info
                    .insert("workspace_members".to_string(), members.join(","));
            }
        }

        // Check for additional Cargo features
        let cargo_lock_path = manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("Cargo.lock");

        if delegate.exists(&cargo_lock_path, Some(false)).await {
            metadata
                .additional_info
                .insert("has_lockfile".to_string(), "true".to_string());
        }

        Ok(metadata)
    }
}

/// Check if a Cargo.toml represents a workspace root
async fn is_workspace_root(
    cargo_toml_path: &Path,
    delegate: &dyn ManifestDelegate,
) -> Result<bool> {
    let content = delegate.read_to_string(cargo_toml_path).await?;

    let manifest: CargoManifest =
        toml::from_str(&content).with_path_context(cargo_toml_path.to_path_buf())?;

    Ok(manifest.workspace.is_some())
}

/// Simplified Cargo.toml structure for parsing
#[derive(serde::Deserialize)]
struct CargoManifest {
    package: Option<CargoPackage>,
    workspace: Option<CargoWorkspace>,
    dependencies: Option<std::collections::HashMap<String, toml::Value>>,
    #[serde(rename = "dev-dependencies")]
    dev_dependencies: Option<std::collections::HashMap<String, toml::Value>>,
}

#[derive(serde::Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    description: Option<String>,
    edition: Option<String>,
}

#[derive(serde::Deserialize)]
struct CargoWorkspace {
    members: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
}

impl Default for RustManifestProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::FsDelegate;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_rust_provider_basic() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        let manifest_content = r#"
[package]
name = "test-project"
version = "0.1.0"
edition = "2021"
description = "A test project"

[dependencies]
serde = "1.0"
tokio = { version = "1.0", features = ["full"] }

[dev-dependencies]
tokio-test = "0.4"
"#;
        tokio::fs::write(&cargo_toml, manifest_content)
            .await
            .unwrap();

        let provider = RustManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider
            .validate_manifest(&cargo_toml, &*delegate)
            .await
            .unwrap());

        // Test project metadata extraction
        let metadata = provider
            .get_project_metadata(&cargo_toml, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "test-project");
        assert_eq!(metadata.version.as_ref().unwrap(), "0.1.0");
        assert_eq!(metadata.language, "rust");
        assert!(metadata.dependencies.contains(&"serde".to_string()));
        assert!(metadata
            .dev_dependencies
            .contains(&"tokio-test".to_string()));
        assert_eq!(metadata.additional_info.get("edition").unwrap(), "2021");
    }

    #[tokio::test]
    async fn test_rust_workspace_detection() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        let workspace_content = r#"
[workspace]
members = ["crate1", "crate2"]
exclude = ["target"]
"#;
        tokio::fs::write(&cargo_toml, workspace_content)
            .await
            .unwrap();

        let provider = RustManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation for workspace
        assert!(provider
            .validate_manifest(&cargo_toml, &*delegate)
            .await
            .unwrap());

        // Test workspace detection
        assert!(is_workspace_root(&cargo_toml, &*delegate).await.unwrap());

        // Test metadata for workspace
        let metadata = provider
            .get_project_metadata(&cargo_toml, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "rust");
        assert_eq!(
            metadata.additional_info.get("workspace_members").unwrap(),
            "crate1,crate2"
        );
    }

    #[tokio::test]
    async fn test_rust_search_finds_outermost() {
        let temp_dir = TempDir::new().unwrap();

        // Create workspace structure
        let workspace_cargo = temp_dir.path().join("Cargo.toml");
        let workspace_content = r#"
[workspace]
members = ["member1"]
"#;
        tokio::fs::write(&workspace_cargo, workspace_content)
            .await
            .unwrap();

        // Create member crate
        let member_dir = temp_dir.path().join("member1");
        tokio::fs::create_dir_all(&member_dir).await.unwrap();
        let member_cargo = member_dir.join("Cargo.toml");
        let member_content = r#"
[package]
name = "member1"
version = "0.1.0"
"#;
        tokio::fs::write(&member_cargo, member_content)
            .await
            .unwrap();

        // Create a file deep in the member
        let src_dir = member_dir.join("src");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        let lib_rs = src_dir.join("lib.rs");
        tokio::fs::write(&lib_rs, "// lib").await.unwrap();

        let provider = RustManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&lib_rs, 10, delegate);

        // Should find the workspace root, not the member crate
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_invalid_cargo_toml() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        // Invalid TOML content
        tokio::fs::write(&cargo_toml, "invalid toml content [")
            .await
            .unwrap();

        let provider = RustManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Should fail validation
        let result = provider.validate_manifest(&cargo_toml, &*delegate).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_empty_cargo_toml() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");

        // Empty but valid TOML
        tokio::fs::write(&cargo_toml, "").await.unwrap();

        let provider = RustManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Should fail validation (no package or workspace section)
        let result = provider.validate_manifest(&cargo_toml, &*delegate).await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should be false
    }
}
