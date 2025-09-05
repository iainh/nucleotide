// ABOUTME: TypeScript/Node.js project manifest provider for package.json and tsconfig.json
// ABOUTME: Handles modern JavaScript/TypeScript projects with npm, yarn, and pnpm support

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::error::{ProjectError, Result, WithPathContext};
use crate::manifest::{
    BaseManifestProvider, ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery,
    ProjectMetadata,
};

/// Manifest provider for TypeScript/Node.js projects
pub struct TypeScriptManifestProvider {
    base: BaseManifestProvider,
}

impl TypeScriptManifestProvider {
    pub fn new() -> Self {
        Self {
            base: BaseManifestProvider::new(
                "package.json",
                vec![
                    "package.json".to_string(),
                    "tsconfig.json".to_string(),
                    "yarn.lock".to_string(),
                    "pnpm-lock.yaml".to_string(),
                    "package-lock.json".to_string(),
                ],
            )
            .with_priority(130), // High priority for JS/TS projects
        }
    }
}

#[async_trait]
impl ManifestProvider for TypeScriptManifestProvider {
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
        // Priority order for Node.js/TypeScript project detection
        let priority_patterns = [
            "package.json",  // Main Node.js manifest
            "tsconfig.json", // TypeScript config
        ];

        let mut outermost_with_indicators = None;

        for ancestor in query.path.ancestors().take(query.max_depth) {
            // Stop at filesystem roots or system directories
            if ancestor.parent().is_none()
                || ancestor == Path::new("/")
                || ancestor == Path::new("/var")
                || ancestor == Path::new("/tmp")
                || ancestor == Path::new("/System")
                || ancestor == Path::new("/Users")
            {
                break;
            }
            // First, look for high-priority files
            for pattern in &priority_patterns {
                let manifest_path = ancestor.join(pattern);

                if query.delegate.exists(&manifest_path, Some(false)).await {
                    nucleotide_logging::debug!(
                        js_manifest = %manifest_path.display(),
                        pattern = pattern,
                        "Found JavaScript/TypeScript manifest file"
                    );

                    // Validate the manifest
                    if self
                        .validate_js_manifest(&manifest_path, pattern, &*query.delegate)
                        .await?
                    {
                        nucleotide_logging::info!(
                            project_root = %ancestor.display(),
                            manifest_type = pattern,
                            "Found JavaScript/TypeScript project root"
                        );
                        return Ok(Some(ancestor.to_path_buf()));
                    }
                }
            }

            // Check for package manager lock files (indicates project root)
            let lock_files = ["yarn.lock", "pnpm-lock.yaml", "package-lock.json"];
            for lock_file in &lock_files {
                let lock_path = ancestor.join(lock_file);
                if query.delegate.exists(&lock_path, Some(false)).await {
                    // Also check for package.json in the same directory
                    let package_json = ancestor.join("package.json");
                    if query.delegate.exists(&package_json, Some(false)).await {
                        nucleotide_logging::info!(
                            project_root = %ancestor.display(),
                            lock_file = lock_file,
                            "Found JavaScript/TypeScript project root via lock file"
                        );
                        return Ok(Some(ancestor.to_path_buf()));
                    }
                }
            }

            // Check for common JS/TS project indicators (find outermost)
            if self
                .has_js_project_indicators(ancestor, &*query.delegate)
                .await?
            {
                nucleotide_logging::debug!(
                    project_root = %ancestor.display(),
                    "Found JavaScript/TypeScript project indicator"
                );
                outermost_with_indicators = Some(ancestor.to_path_buf());
            }
        }

        if let Some(root) = outermost_with_indicators {
            nucleotide_logging::info!(
                project_root = %root.display(),
                "Found JavaScript/TypeScript project root based on indicators"
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

        self.validate_js_manifest(path, file_name, delegate).await
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
            language: "javascript".to_string(),
            ..Default::default()
        };

        match file_name {
            "package.json" => {
                self.extract_package_json_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "tsconfig.json" => {
                self.extract_tsconfig_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
                metadata.language = "typescript".to_string();
            }
            _ => {}
        }

        // Add additional JavaScript/TypeScript project information
        self.add_js_environment_info(
            manifest_path.parent().unwrap_or(Path::new(".")),
            delegate,
            &mut metadata,
        )
        .await?;

        Ok(metadata)
    }
}

impl TypeScriptManifestProvider {
    async fn validate_js_manifest(
        &self,
        path: &Path,
        file_name: &str,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        match file_name {
            "package.json" => self.validate_package_json(path, delegate).await,
            "tsconfig.json" => self.validate_tsconfig_json(path, delegate).await,
            _ => Ok(false),
        }
    }

    async fn validate_package_json(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        match serde_json::from_str::<PackageJson>(&content) {
            Ok(manifest) => {
                nucleotide_logging::trace!(
                    package_json = %path.display(),
                    has_name = manifest.name.is_some(),
                    has_scripts = manifest.scripts.is_some(),
                    has_dependencies = manifest.dependencies.is_some(),
                    "Validated package.json structure"
                );

                // Valid if it has a name or scripts or dependencies
                Ok(manifest.name.is_some()
                    || manifest.scripts.is_some()
                    || manifest.dependencies.is_some())
            }
            Err(e) => {
                nucleotide_logging::warn!(
                    package_json = %path.display(),
                    error = %e,
                    "Invalid JSON in package.json"
                );
                Err(ProjectError::manifest_parse(path.to_path_buf(), e))
            }
        }
    }

    async fn validate_tsconfig_json(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        match serde_json::from_str::<TsConfig>(&content) {
            Ok(config) => {
                nucleotide_logging::trace!(
                    tsconfig_json = %path.display(),
                    has_compiler_options = config.compiler_options.is_some(),
                    has_files = config.files.is_some(),
                    has_include = config.include.is_some(),
                    "Validated tsconfig.json structure"
                );

                // Valid if it has TypeScript-specific configuration
                Ok(config.compiler_options.is_some()
                    || config.files.is_some()
                    || config.include.is_some())
            }
            Err(e) => {
                nucleotide_logging::warn!(
                    tsconfig_json = %path.display(),
                    error = %e,
                    "Invalid JSON in tsconfig.json"
                );
                Err(ProjectError::manifest_parse(path.to_path_buf(), e))
            }
        }
    }

    async fn has_js_project_indicators(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let indicators = [
            "node_modules",      // npm/yarn/pnpm installed packages
            "src",               // Common source directory
            "lib",               // Common library directory
            "dist",              // Common build output
            "build",             // Build directory
            "index.js",          // Common entry point
            "index.ts",          // TypeScript entry point
            "main.js",           // Alternative entry point
            "app.js",            // Common for apps
            "server.js",         // Common for servers
            ".eslintrc.js",      // ESLint config
            ".eslintrc.json",    // ESLint config
            "webpack.config.js", // Webpack config
            "vite.config.js",    // Vite config
            "rollup.config.js",  // Rollup config
            "jest.config.js",    // Jest config
        ];

        for indicator in &indicators {
            let indicator_path = path.join(indicator);
            if delegate.exists(&indicator_path, None).await {
                nucleotide_logging::debug!(
                    indicator = indicator,
                    path = %path.display(),
                    "Found JavaScript/TypeScript project indicator"
                );
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn extract_package_json_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;
        let manifest: PackageJson =
            serde_json::from_str(&content).with_path_context(path.to_path_buf())?;

        metadata.name = manifest.name;
        metadata.version = manifest.version;
        metadata.description = manifest.description;

        // Extract dependencies
        if let Some(deps) = manifest.dependencies {
            metadata.dependencies = deps.into_keys().collect();
        }

        if let Some(dev_deps) = manifest.dev_dependencies {
            metadata.dev_dependencies = dev_deps.into_keys().collect();
        }

        // Extract scripts
        if let Some(scripts) = manifest.scripts {
            let script_names: Vec<String> = scripts.into_keys().collect();
            metadata
                .additional_info
                .insert("scripts".to_string(), script_names.join(","));
        }

        // Detect framework/runtime
        if metadata.dependencies.iter().any(|d| d.contains("react")) {
            metadata
                .additional_info
                .insert("framework".to_string(), "react".to_string());
        } else if metadata.dependencies.iter().any(|d| d.contains("vue")) {
            metadata
                .additional_info
                .insert("framework".to_string(), "vue".to_string());
        } else if metadata.dependencies.iter().any(|d| d.contains("angular")) {
            metadata
                .additional_info
                .insert("framework".to_string(), "angular".to_string());
        } else if metadata.dependencies.iter().any(|d| d.contains("express")) {
            metadata
                .additional_info
                .insert("framework".to_string(), "express".to_string());
        } else if metadata.dependencies.iter().any(|d| d.contains("next")) {
            metadata
                .additional_info
                .insert("framework".to_string(), "next.js".to_string());
        }

        // Detect if TypeScript is used
        if metadata.dependencies.iter().any(|d| d == "typescript")
            || metadata.dev_dependencies.iter().any(|d| d == "typescript")
        {
            metadata.language = "typescript".to_string();
        }

        Ok(())
    }

    async fn extract_tsconfig_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;
        let config: TsConfig =
            serde_json::from_str(&content).with_path_context(path.to_path_buf())?;

        if let Some(compiler_options) = config.compiler_options {
            if let Some(target) = compiler_options.get("target")
                && let Some(target_str) = target.as_str()
            {
                metadata
                    .additional_info
                    .insert("typescript_target".to_string(), target_str.to_string());
            }

            if let Some(module) = compiler_options.get("module")
                && let Some(module_str) = module.as_str()
            {
                metadata
                    .additional_info
                    .insert("typescript_module".to_string(), module_str.to_string());
            }

            if let Some(strict) = compiler_options.get("strict")
                && let Some(strict_bool) = strict.as_bool()
            {
                metadata
                    .additional_info
                    .insert("typescript_strict".to_string(), strict_bool.to_string());
            }
        }

        Ok(())
    }

    async fn add_js_environment_info(
        &self,
        project_root: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        // Check for package manager lock files
        if delegate
            .exists(&project_root.join("yarn.lock"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("package_manager".to_string(), "yarn".to_string());
        } else if delegate
            .exists(&project_root.join("pnpm-lock.yaml"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("package_manager".to_string(), "pnpm".to_string());
        } else if delegate
            .exists(&project_root.join("package-lock.json"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("package_manager".to_string(), "npm".to_string());
        }

        // Check for Node.js version files
        if delegate
            .exists(&project_root.join(".nvmrc"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("has_nvmrc".to_string(), "true".to_string());
        }

        // Check for common config files
        let config_files = [
            (".eslintrc.js", "eslint"),
            (".eslintrc.json", "eslint"),
            (".prettierrc", "prettier"),
            ("webpack.config.js", "webpack"),
            ("vite.config.js", "vite"),
            ("rollup.config.js", "rollup"),
            ("jest.config.js", "jest"),
            ("babel.config.js", "babel"),
        ];

        for (file, tool) in &config_files {
            if delegate.exists(&project_root.join(file), Some(false)).await {
                metadata
                    .additional_info
                    .insert(format!("has_{}", tool), "true".to_string());
            }
        }

        Ok(())
    }
}

/// Simplified package.json structure for parsing
#[derive(serde::Deserialize)]
struct PackageJson {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    scripts: Option<std::collections::HashMap<String, String>>,
    dependencies: Option<std::collections::HashMap<String, serde_json::Value>>,
    #[serde(rename = "devDependencies")]
    dev_dependencies: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Simplified tsconfig.json structure for parsing
#[derive(serde::Deserialize)]
struct TsConfig {
    #[serde(rename = "compilerOptions")]
    compiler_options: Option<std::collections::HashMap<String, serde_json::Value>>,
    files: Option<Vec<String>>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
}

impl Default for TypeScriptManifestProvider {
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
    async fn test_package_json() {
        let temp_dir = TempDir::new().unwrap();
        let package_json = temp_dir.path().join("package.json");

        let manifest_content = r#"
{
  "name": "test-js-project",
  "version": "1.0.0",
  "description": "A test JavaScript project",
  "main": "index.js",
  "scripts": {
    "start": "node index.js",
    "test": "jest",
    "build": "webpack"
  },
  "dependencies": {
    "express": "^4.18.0",
    "lodash": "^4.17.21"
  },
  "devDependencies": {
    "jest": "^27.0.0",
    "webpack": "^5.0.0"
  }
}
"#;
        tokio::fs::write(&package_json, manifest_content)
            .await
            .unwrap();

        let provider = TypeScriptManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&package_json, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&package_json, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "test-js-project");
        assert_eq!(metadata.version.as_ref().unwrap(), "1.0.0");
        assert_eq!(metadata.language, "javascript");
        assert!(metadata.dependencies.contains(&"express".to_string()));
        assert!(metadata.dev_dependencies.contains(&"jest".to_string()));
        assert_eq!(
            metadata.additional_info.get("framework").unwrap(),
            "express"
        );
    }

    #[tokio::test]
    async fn test_typescript_project() {
        let temp_dir = TempDir::new().unwrap();

        // Create package.json with TypeScript
        let package_json = temp_dir.path().join("package.json");
        let package_content = r#"
{
  "name": "test-ts-project",
  "version": "1.0.0",
  "dependencies": {
    "react": "^18.0.0"
  },
  "devDependencies": {
    "typescript": "^4.8.0",
    "@types/react": "^18.0.0"
  }
}
"#;
        tokio::fs::write(&package_json, package_content)
            .await
            .unwrap();

        // Create tsconfig.json
        let tsconfig_json = temp_dir.path().join("tsconfig.json");
        let tsconfig_content = r#"
{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
    "strict": true,
    "jsx": "react-jsx"
  },
  "include": ["src/**/*"],
  "exclude": ["node_modules", "dist"]
}
"#;
        tokio::fs::write(&tsconfig_json, tsconfig_content)
            .await
            .unwrap();

        let provider = TypeScriptManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test package.json validation and metadata
        let metadata = provider
            .get_project_metadata(&package_json, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "typescript"); // Should detect TypeScript
        assert_eq!(metadata.additional_info.get("framework").unwrap(), "react");

        // Test tsconfig.json validation and metadata
        assert!(
            provider
                .validate_manifest(&tsconfig_json, &*delegate)
                .await
                .unwrap()
        );
        let ts_metadata = provider
            .get_project_metadata(&tsconfig_json, &*delegate)
            .await
            .unwrap();
        assert_eq!(ts_metadata.language, "typescript");
        assert_eq!(
            ts_metadata
                .additional_info
                .get("typescript_target")
                .unwrap(),
            "ES2020"
        );
        assert_eq!(
            ts_metadata
                .additional_info
                .get("typescript_module")
                .unwrap(),
            "ESNext"
        );
        assert_eq!(
            ts_metadata
                .additional_info
                .get("typescript_strict")
                .unwrap(),
            "true"
        );
    }

    #[tokio::test]
    async fn test_lock_file_detection() {
        let temp_dir = TempDir::new().unwrap();

        // Create package.json
        let package_json = temp_dir.path().join("package.json");
        tokio::fs::write(&package_json, r#"{"name": "test"}"#)
            .await
            .unwrap();

        // Create yarn.lock
        let yarn_lock = temp_dir.path().join("yarn.lock");
        tokio::fs::write(&yarn_lock, "# yarn lockfile")
            .await
            .unwrap();

        // Create a nested file
        let src_dir = temp_dir.path().join("src");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        let index_ts = src_dir.join("index.ts");
        tokio::fs::write(&index_ts, "console.log('hello');")
            .await
            .unwrap();

        let provider = TypeScriptManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&index_ts, 10, delegate);

        // Should detect project root via lock file
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_js_project_indicators() {
        let temp_dir = TempDir::new().unwrap();

        // Create JS project structure without package.json
        tokio::fs::write(temp_dir.path().join("index.js"), "console.log('hello');")
            .await
            .unwrap();
        let node_modules = temp_dir.path().join("node_modules");
        tokio::fs::create_dir_all(&node_modules).await.unwrap();

        let src_file = temp_dir.path().join("src").join("app.js");
        tokio::fs::create_dir_all(src_file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&src_file, "// js code").await.unwrap();

        let provider = TypeScriptManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&src_file, 10, delegate);

        // Should detect as JS project based on indicators
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_invalid_package_json() {
        let temp_dir = TempDir::new().unwrap();
        let package_json = temp_dir.path().join("package.json");

        // Invalid JSON content
        tokio::fs::write(&package_json, r#"{ invalid json"#)
            .await
            .unwrap();

        let provider = TypeScriptManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Should fail validation
        let result = provider.validate_manifest(&package_json, &*delegate).await;
        assert!(result.is_err());
    }
}
