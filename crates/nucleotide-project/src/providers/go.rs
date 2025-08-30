// ABOUTME: Go project manifest provider for go.mod and module detection
// ABOUTME: Handles Go modules with workspace support and proper validation

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::manifest::{
    BaseManifestProvider, ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery,
    ProjectMetadata,
};

/// Manifest provider for Go projects
pub struct GoManifestProvider {
    base: BaseManifestProvider,
}

impl GoManifestProvider {
    pub fn new() -> Self {
        Self {
            base: BaseManifestProvider::new(
                "go.mod",
                vec!["go.mod".to_string(), "go.sum".to_string()],
            )
            .with_priority(120), // High priority for Go projects
        }
    }
}

#[async_trait]
impl ManifestProvider for GoManifestProvider {
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
        // For Go, we look for go.mod files first (highest priority)
        let mut outermost_with_indicators = None;

        for ancestor in query.path.ancestors().take(query.max_depth) {
            // Stop at filesystem roots
            if ancestor.parent().is_none() {
                break;
            }
            let go_mod_path = ancestor.join("go.mod");

            if query.delegate.exists(&go_mod_path, Some(false)).await {
                nucleotide_logging::debug!(
                    go_mod = %go_mod_path.display(),
                    "Found go.mod"
                );

                // Validate the go.mod file
                if self
                    .validate_manifest(&go_mod_path, &*query.delegate)
                    .await?
                {
                    nucleotide_logging::info!(
                        project_root = %ancestor.display(),
                        "Found Go project root"
                    );
                    return Ok(Some(ancestor.to_path_buf()));
                }
            }

            // Check for Go workspace (go.work)
            let go_work_path = ancestor.join("go.work");
            if query.delegate.exists(&go_work_path, Some(false)).await {
                nucleotide_logging::info!(
                    workspace_root = %ancestor.display(),
                    "Found Go workspace root"
                );
                return Ok(Some(ancestor.to_path_buf()));
            }

            // Check for legacy GOPATH structure (fallback) - find outermost
            if self
                .has_go_project_indicators(ancestor, &*query.delegate)
                .await?
            {
                outermost_with_indicators = Some(ancestor.to_path_buf());
            }
        }

        if let Some(root) = outermost_with_indicators {
            nucleotide_logging::info!(
                project_root = %root.display(),
                "Found Go project root based on indicators"
            );
            Ok(Some(root))
        } else {
            Ok(None)
        }
    }

    async fn validate_manifest(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        match file_name {
            "go.mod" => self.validate_go_mod(path, delegate).await,
            "go.work" => self.validate_go_work(path, delegate).await,
            _ => Ok(false),
        }
    }

    async fn get_project_metadata(
        &self,
        manifest_path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<ProjectMetadata> {
        let file_name = manifest_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        let mut metadata = ProjectMetadata {
            language: "go".to_string(),
            ..Default::default()
        };

        match file_name {
            "go.mod" => {
                self.extract_go_mod_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "go.work" => {
                self.extract_go_work_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            _ => {}
        }

        // Add additional Go project information
        self.add_go_environment_info(
            manifest_path.parent().unwrap_or(Path::new(".")),
            delegate,
            &mut metadata,
        )
        .await?;

        Ok(metadata)
    }
}

impl GoManifestProvider {
    async fn validate_go_mod(&self, path: &Path, delegate: &dyn ManifestDelegate) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Parse go.mod content
        let go_mod = self.parse_go_mod(&content)?;

        nucleotide_logging::trace!(
            go_mod = %path.display(),
            module_name = %go_mod.module,
            go_version = go_mod.go_version.as_deref().unwrap_or("unknown"),
            require_count = go_mod.require.len(),
            "Validated go.mod structure"
        );

        // Valid if it has a module declaration
        Ok(!go_mod.module.is_empty())
    }

    async fn validate_go_work(&self, path: &Path, delegate: &dyn ManifestDelegate) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Basic validation for go.work
        let has_go_directive = content.contains("go ");
        let has_use_directive = content.contains("use ");

        nucleotide_logging::trace!(
            go_work = %path.display(),
            has_go_directive = has_go_directive,
            has_use_directive = has_use_directive,
            "Validated go.work structure"
        );

        Ok(has_go_directive)
    }

    async fn has_go_project_indicators(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let indicators = [
            "main.go", "cmd",      // Common Go project layout
            "pkg",      // Common Go project layout
            "internal", // Common Go project layout
            "vendor",   // Vendored dependencies
            "Makefile", // Often used with Go projects
            ".go",      // Any .go file (check if directory contains Go files)
        ];

        for indicator in &indicators {
            let indicator_path = path.join(indicator);
            if delegate.exists(&indicator_path, None).await {
                nucleotide_logging::debug!(
                    indicator = indicator,
                    path = %path.display(),
                    "Found Go project indicator"
                );
                return Ok(true);
            }
        }

        // Check for any .go files in the directory
        if self.directory_contains_go_files(path, delegate).await? {
            return Ok(true);
        }

        Ok(false)
    }

    async fn directory_contains_go_files(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        // Simple check: look for common Go file names
        let common_go_files = [
            "main.go",
            "app.go",
            "server.go",
            "client.go",
            "handler.go",
            "service.go",
            "model.go",
            "config.go",
        ];

        for go_file in &common_go_files {
            let file_path = path.join(go_file);
            if delegate.exists(&file_path, Some(false)).await {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn extract_go_mod_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;
        let go_mod = self.parse_go_mod(&content)?;

        // Extract module name (often serves as project name)
        let module_parts: Vec<&str> = go_mod.module.split('/').collect();
        if let Some(last_part) = module_parts.last() {
            metadata.name = Some(last_part.to_string());
        }

        // Extract Go version
        if let Some(version) = go_mod.go_version {
            metadata
                .additional_info
                .insert("go_version".to_string(), version);
        }

        // Extract dependencies
        metadata.dependencies = go_mod.require.into_keys().collect();

        // Module path
        metadata
            .additional_info
            .insert("module_path".to_string(), go_mod.module);

        Ok(())
    }

    async fn extract_go_work_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Extract go version from workspace
        if let Some(go_version) = self.extract_go_version(&content) {
            metadata
                .additional_info
                .insert("go_version".to_string(), go_version);
        }

        // Extract workspace modules
        let modules = self.extract_workspace_modules(&content);
        if !modules.is_empty() {
            metadata
                .additional_info
                .insert("workspace_modules".to_string(), modules.join(","));
        }

        metadata
            .additional_info
            .insert("is_workspace".to_string(), "true".to_string());

        Ok(())
    }

    async fn add_go_environment_info(
        &self,
        project_root: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        // Check for go.sum (dependency checksums)
        if delegate
            .exists(&project_root.join("go.sum"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("has_go_sum".to_string(), "true".to_string());
        }

        // Check for vendor directory
        if delegate
            .exists(&project_root.join("vendor"), Some(true))
            .await
        {
            metadata
                .additional_info
                .insert("has_vendor".to_string(), "true".to_string());
        }

        // Check for common Go project structure
        let structure_indicators = [
            ("cmd", "has_cmd_dir"),
            ("pkg", "has_pkg_dir"),
            ("internal", "has_internal_dir"),
            ("api", "has_api_dir"),
            ("web", "has_web_dir"),
            ("scripts", "has_scripts_dir"),
            ("build", "has_build_dir"),
            ("deployments", "has_deployments_dir"),
        ];

        for (dir, flag) in &structure_indicators {
            if delegate.exists(&project_root.join(dir), Some(true)).await {
                metadata
                    .additional_info
                    .insert(flag.to_string(), "true".to_string());
            }
        }

        // Check for common Go tools config
        let config_files = [
            (".golangci.yml", "golangci_lint"),
            ("Dockerfile", "docker"),
            ("docker-compose.yml", "docker_compose"),
            ("Makefile", "make"),
            (".github", "github_actions"),
        ];

        for (file, tool) in &config_files {
            if delegate.exists(&project_root.join(file), None).await {
                metadata
                    .additional_info
                    .insert(format!("has_{}", tool), "true".to_string());
            }
        }

        Ok(())
    }

    fn parse_go_mod(&self, content: &str) -> Result<GoMod> {
        let mut go_mod = GoMod::default();
        let mut in_require_block = false;

        for line in content.lines() {
            let line = line.trim();

            if line.is_empty() || line.starts_with("//") {
                continue;
            }

            if line.starts_with("module ") {
                go_mod.module = line[7..].trim().to_string();
            } else if line.starts_with("go ") {
                go_mod.go_version = Some(line[3..].trim().to_string());
            } else if line == "require (" {
                in_require_block = true;
            } else if line == ")" && in_require_block {
                in_require_block = false;
            } else if line.starts_with("require ") && !in_require_block {
                // Single line require
                if let Some((name, version)) = self.parse_require_line(&line[8..]) {
                    go_mod.require.insert(name, version);
                }
            } else if in_require_block {
                // Multi-line require block
                if let Some((name, version)) = self.parse_require_line(line) {
                    go_mod.require.insert(name, version);
                }
            }
        }

        Ok(go_mod)
    }

    fn parse_require_line(&self, line: &str) -> Option<(String, String)> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    }

    fn extract_go_version(&self, content: &str) -> Option<String> {
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("go ") {
                return Some(line[3..].trim().to_string());
            }
        }
        None
    }

    fn extract_workspace_modules(&self, content: &str) -> Vec<String> {
        let mut modules = Vec::new();
        let mut in_use_block = false;

        for line in content.lines() {
            let line = line.trim();

            if line == "use (" {
                in_use_block = true;
            } else if line == ")" && in_use_block {
                in_use_block = false;
            } else if line.starts_with("use ") && !in_use_block {
                // Single line use
                let module = line[4..].trim().trim_matches('"').trim_matches('\'');
                modules.push(module.to_string());
            } else if in_use_block && !line.is_empty() {
                // Multi-line use block
                let module = line.trim_matches('"').trim_matches('\'');
                modules.push(module.to_string());
            }
        }

        modules
    }
}

/// Parsed go.mod structure
#[derive(Default)]
struct GoMod {
    module: String,
    go_version: Option<String>,
    require: std::collections::HashMap<String, String>,
}

impl Default for GoManifestProvider {
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
    async fn test_go_mod() {
        let temp_dir = TempDir::new().unwrap();
        let go_mod = temp_dir.path().join("go.mod");

        let manifest_content = r#"
module github.com/example/myproject

go 1.21

require (
    github.com/gin-gonic/gin v1.9.1
    github.com/stretchr/testify v1.8.4
)

require (
    github.com/davecgh/go-spew v1.1.1 // indirect
    github.com/pmezard/go-difflib v1.0.0 // indirect
)
"#;
        tokio::fs::write(&go_mod, manifest_content).await.unwrap();

        let provider = GoManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&go_mod, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&go_mod, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "myproject");
        assert_eq!(metadata.language, "go");
        assert_eq!(metadata.additional_info.get("go_version").unwrap(), "1.21");
        assert_eq!(
            metadata.additional_info.get("module_path").unwrap(),
            "github.com/example/myproject"
        );
        assert!(
            metadata
                .dependencies
                .contains(&"github.com/gin-gonic/gin".to_string())
        );
    }

    #[tokio::test]
    async fn test_go_work() {
        let temp_dir = TempDir::new().unwrap();
        let go_work = temp_dir.path().join("go.work");

        let workspace_content = r#"
go 1.21

use (
    ./api
    ./web
    ./worker
)
"#;
        tokio::fs::write(&go_work, workspace_content).await.unwrap();

        let provider = GoManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&go_work, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&go_work, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "go");
        assert_eq!(metadata.additional_info.get("go_version").unwrap(), "1.21");
        assert_eq!(
            metadata.additional_info.get("is_workspace").unwrap(),
            "true"
        );
        assert_eq!(
            metadata.additional_info.get("workspace_modules").unwrap(),
            "./api,./web,./worker"
        );
    }

    #[tokio::test]
    async fn test_go_project_search() {
        let temp_dir = TempDir::new().unwrap();

        // Create go.mod
        let go_mod = temp_dir.path().join("go.mod");
        tokio::fs::write(&go_mod, "module test\ngo 1.21")
            .await
            .unwrap();

        // Create go.sum
        let go_sum = temp_dir.path().join("go.sum");
        tokio::fs::write(&go_sum, "").await.unwrap();

        // Create nested Go file
        let cmd_dir = temp_dir.path().join("cmd").join("main");
        tokio::fs::create_dir_all(&cmd_dir).await.unwrap();
        let main_go = cmd_dir.join("main.go");
        tokio::fs::write(&main_go, "package main\nfunc main() {}")
            .await
            .unwrap();

        let provider = GoManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&main_go, 10, delegate);

        // Should find the project root
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_go_project_indicators() {
        let temp_dir = TempDir::new().unwrap();

        // Create Go project structure without go.mod
        tokio::fs::write(temp_dir.path().join("main.go"), "package main")
            .await
            .unwrap();
        let cmd_dir = temp_dir.path().join("cmd");
        tokio::fs::create_dir_all(&cmd_dir).await.unwrap();

        let src_file = temp_dir.path().join("pkg").join("service.go");
        tokio::fs::create_dir_all(src_file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&src_file, "package service")
            .await
            .unwrap();

        let provider = GoManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&src_file, 10, delegate);

        // Should detect as Go project based on indicators
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_go_mod_parsing() {
        let provider = GoManifestProvider::new();

        let content = r#"
module github.com/example/test

go 1.21

require (
    github.com/gin-gonic/gin v1.9.1
    github.com/lib/pq v1.10.9
)
"#;

        let go_mod = provider.parse_go_mod(content).unwrap();
        assert_eq!(go_mod.module, "github.com/example/test");
        assert_eq!(go_mod.go_version.unwrap(), "1.21");
        assert_eq!(go_mod.require.len(), 2);
        assert_eq!(
            go_mod.require.get("github.com/gin-gonic/gin").unwrap(),
            "v1.9.1"
        );
    }

    #[tokio::test]
    async fn test_workspace_module_extraction() {
        let provider = GoManifestProvider::new();

        let content = r#"
go 1.21

use (
    ./api
    ./web
    ./worker
)
"#;

        let modules = provider.extract_workspace_modules(content);
        assert_eq!(modules.len(), 3);
        assert!(modules.contains(&"./api".to_string()));
        assert!(modules.contains(&"./web".to_string()));
        assert!(modules.contains(&"./worker".to_string()));
    }

    #[tokio::test]
    async fn test_invalid_go_mod() {
        let temp_dir = TempDir::new().unwrap();
        let go_mod = temp_dir.path().join("go.mod");

        // go.mod without module declaration
        tokio::fs::write(&go_mod, "go 1.21\n").await.unwrap();

        let provider = GoManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Should fail validation (no module)
        let result = provider.validate_manifest(&go_mod, &*delegate).await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should be false
    }
}
