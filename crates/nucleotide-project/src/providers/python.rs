// ABOUTME: Python project manifest provider for pyproject.toml, setup.py, and requirements.txt
// ABOUTME: Handles modern Python projects with priority for pyproject.toml and poetry support

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::error::{ProjectError, Result, WithPathContext};
use crate::manifest::{
    BaseManifestProvider, ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery,
    ProjectMetadata,
};

/// Manifest provider for Python projects
pub struct PythonManifestProvider {
    base: BaseManifestProvider,
}

impl PythonManifestProvider {
    pub fn new() -> Self {
        Self {
            base: BaseManifestProvider::new(
                "pyproject.toml",
                vec![
                    "pyproject.toml".to_string(),
                    "setup.py".to_string(),
                    "requirements.txt".to_string(),
                    "Pipfile".to_string(),
                    "poetry.lock".to_string(),
                ],
            )
            .with_priority(140), // High priority for Python projects
        }
    }
}

#[async_trait]
impl ManifestProvider for PythonManifestProvider {
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
        // Priority order for Python project detection
        let priority_patterns = [
            "pyproject.toml",   // Modern Python standard
            "setup.py",         // Traditional setup
            "Pipfile",          // Pipenv
            "requirements.txt", // Pip requirements
        ];

        for ancestor in query.path.ancestors().take(query.max_depth) {
            // First, look for high-priority files
            for pattern in &priority_patterns {
                let manifest_path = ancestor.join(pattern);

                if query.delegate.exists(&manifest_path, Some(false)).await {
                    nucleotide_logging::debug!(
                        python_manifest = %manifest_path.display(),
                        pattern = pattern,
                        "Found Python manifest file"
                    );

                    // Validate the manifest
                    if self
                        .validate_python_manifest(&manifest_path, pattern, &*query.delegate)
                        .await?
                    {
                        nucleotide_logging::info!(
                            project_root = %ancestor.display(),
                            manifest_type = pattern,
                            "Found Python project root"
                        );
                        return Ok(Some(ancestor.to_path_buf()));
                    }
                }
            }

            // Also check for common Python project indicators
            if self
                .has_python_project_indicators(ancestor, &*query.delegate)
                .await?
            {
                nucleotide_logging::info!(
                    project_root = %ancestor.display(),
                    "Found Python project root based on indicators"
                );
                return Ok(Some(ancestor.to_path_buf()));
            }
        }

        Ok(None)
    }

    async fn validate_manifest(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        self.validate_python_manifest(path, file_name, delegate)
            .await
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
            language: "python".to_string(),
            ..Default::default()
        };

        match file_name {
            "pyproject.toml" => {
                self.extract_pyproject_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "setup.py" => {
                self.extract_setup_py_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "requirements.txt" => {
                self.extract_requirements_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "Pipfile" => {
                self.extract_pipfile_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            _ => {}
        }

        // Add additional Python project information
        self.add_python_environment_info(
            manifest_path.parent().unwrap_or(Path::new(".")),
            delegate,
            &mut metadata,
        )
        .await?;

        Ok(metadata)
    }
}

impl PythonManifestProvider {
    async fn validate_python_manifest(
        &self,
        path: &Path,
        file_name: &str,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        match file_name {
            "pyproject.toml" => self.validate_pyproject_toml(path, delegate).await,
            "setup.py" => self.validate_setup_py(path, delegate).await,
            "requirements.txt" => self.validate_requirements_txt(path, delegate).await,
            "Pipfile" => self.validate_pipfile(path, delegate).await,
            _ => Ok(false),
        }
    }

    async fn validate_pyproject_toml(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        match toml::from_str::<PyProjectToml>(&content) {
            Ok(manifest) => {
                nucleotide_logging::trace!(
                    pyproject_toml = %path.display(),
                    has_project = manifest.project.is_some(),
                    has_tool = manifest.tool.is_some(),
                    has_build_system = manifest.build_system.is_some(),
                    "Validated pyproject.toml structure"
                );

                // Valid if it has project metadata or common Python tools
                let has_python_content = manifest.project.is_some()
                    || manifest.tool.is_some_and(|t| {
                        t.contains_key("poetry")
                            || t.contains_key("setuptools")
                            || t.contains_key("black")
                            || t.contains_key("pytest")
                            || t.contains_key("mypy")
                    });

                Ok(has_python_content)
            }
            Err(e) => {
                nucleotide_logging::warn!(
                    pyproject_toml = %path.display(),
                    error = %e,
                    "Invalid TOML in pyproject.toml"
                );
                Err(ProjectError::manifest_parse(path.to_path_buf(), e))
            }
        }
    }

    async fn validate_setup_py(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Basic validation - check for setup() call
        let has_setup_call = content.contains("setup(") || content.contains("setuptools.setup");

        nucleotide_logging::trace!(
            setup_py = %path.display(),
            has_setup_call = has_setup_call,
            "Validated setup.py structure"
        );

        Ok(has_setup_call)
    }

    async fn validate_requirements_txt(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Basic validation - check for Python package references
        let lines: Vec<&str> = content
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();

        let has_packages = !lines.is_empty();

        nucleotide_logging::trace!(
            requirements_txt = %path.display(),
            package_count = lines.len(),
            "Validated requirements.txt structure"
        );

        Ok(has_packages)
    }

    async fn validate_pipfile(&self, path: &Path, delegate: &dyn ManifestDelegate) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        match toml::from_str::<PipFile>(&content) {
            Ok(pipfile) => {
                let is_valid = pipfile.packages.is_some() || pipfile.dev_packages.is_some();

                nucleotide_logging::trace!(
                    pipfile = %path.display(),
                    has_packages = pipfile.packages.is_some(),
                    has_dev_packages = pipfile.dev_packages.is_some(),
                    "Validated Pipfile structure"
                );

                Ok(is_valid)
            }
            Err(e) => {
                nucleotide_logging::warn!(
                    pipfile = %path.display(),
                    error = %e,
                    "Invalid TOML in Pipfile"
                );
                Err(ProjectError::manifest_parse(path.to_path_buf(), e))
            }
        }
    }

    async fn has_python_project_indicators(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let indicators = [
            "__init__.py",
            "main.py",
            "app.py",
            "manage.py", // Django
            "setup.cfg",
            "tox.ini",
            ".python-version",
            "requirements", // Directory
            "src",          // Common Python src layout
        ];

        for indicator in &indicators {
            let indicator_path = path.join(indicator);
            if delegate.exists(&indicator_path, None).await {
                nucleotide_logging::debug!(
                    indicator = indicator,
                    path = %path.display(),
                    "Found Python project indicator"
                );
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn extract_pyproject_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;
        let manifest: PyProjectToml =
            toml::from_str(&content).with_path_context(path.to_path_buf())?;

        if let Some(project) = manifest.project {
            metadata.name = Some(project.name);
            metadata.version = project.version;
            metadata.description = project.description;

            if let Some(deps) = project.dependencies {
                metadata.dependencies = deps;
            }
        }

        // Extract tool-specific information
        if let Some(tool) = manifest.tool {
            if tool.contains_key("poetry") {
                metadata
                    .additional_info
                    .insert("build_tool".to_string(), "poetry".to_string());
            } else if tool.contains_key("setuptools") {
                metadata
                    .additional_info
                    .insert("build_tool".to_string(), "setuptools".to_string());
            }
        }

        Ok(())
    }

    async fn extract_setup_py_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Basic regex-based extraction (limited but functional)
        if let Some(name) = extract_setup_field(&content, "name") {
            metadata.name = Some(name);
        }

        if let Some(version) = extract_setup_field(&content, "version") {
            metadata.version = Some(version);
        }

        metadata
            .additional_info
            .insert("build_tool".to_string(), "setuptools".to_string());

        Ok(())
    }

    async fn extract_requirements_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        let packages: Vec<String> = content
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                // Extract package name (before any version specifiers)
                line.split_whitespace()
                    .next()
                    .unwrap_or(line)
                    .split(&['=', '>', '<', '!', '~', ';'][..])
                    .next()
                    .unwrap_or(line)
                    .to_string()
            })
            .collect();

        metadata.dependencies = packages;
        metadata
            .additional_info
            .insert("build_tool".to_string(), "pip".to_string());

        Ok(())
    }

    async fn extract_pipfile_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;
        let pipfile: PipFile = toml::from_str(&content).with_path_context(path.to_path_buf())?;

        if let Some(packages) = pipfile.packages {
            metadata.dependencies = packages.into_keys().collect();
        }

        if let Some(dev_packages) = pipfile.dev_packages {
            metadata.dev_dependencies = dev_packages.into_keys().collect();
        }

        metadata
            .additional_info
            .insert("build_tool".to_string(), "pipenv".to_string());

        Ok(())
    }

    async fn add_python_environment_info(
        &self,
        project_root: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        // Check for virtual environment indicators
        let venv_indicators = ["venv", ".venv", "env", ".env", "virtualenv"];

        for indicator in &venv_indicators {
            let venv_path = project_root.join(indicator);
            if delegate.exists(&venv_path, Some(true)).await {
                metadata
                    .additional_info
                    .insert("has_venv".to_string(), "true".to_string());
                break;
            }
        }

        // Check for specific Python files
        if delegate
            .exists(&project_root.join("__init__.py"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("is_package".to_string(), "true".to_string());
        }

        // Check for testing frameworks
        if delegate
            .exists(&project_root.join("pytest.ini"), Some(false))
            .await
            || delegate
                .exists(&project_root.join("setup.cfg"), Some(false))
                .await
        {
            metadata
                .additional_info
                .insert("has_pytest".to_string(), "true".to_string());
        }

        Ok(())
    }
}

/// Extract field value from setup.py content using basic regex
fn extract_setup_field(content: &str, field: &str) -> Option<String> {
    let patterns = [
        format!(r#"{}=['""]([^'""]+)['""]"#, field),
        format!(r#"{}=['"]([^']+)['"]"#, field),
        format!(r#"{}=([^,\)]+)"#, field),
    ];

    for pattern in &patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            if let Some(captures) = re.captures(content) {
                if let Some(value) = captures.get(1) {
                    return Some(value.as_str().trim().to_string());
                }
            }
        }
    }

    None
}

/// Simplified pyproject.toml structure for parsing
#[derive(serde::Deserialize)]
struct PyProjectToml {
    project: Option<PyProjectProject>,
    tool: Option<std::collections::HashMap<String, toml::Value>>,
    build_system: Option<toml::Value>,
}

#[derive(serde::Deserialize)]
struct PyProjectProject {
    name: String,
    version: Option<String>,
    description: Option<String>,
    dependencies: Option<Vec<String>>,
}

/// Simplified Pipfile structure for parsing
#[derive(serde::Deserialize)]
struct PipFile {
    #[serde(rename = "packages")]
    packages: Option<std::collections::HashMap<String, toml::Value>>,
    #[serde(rename = "dev-packages")]
    dev_packages: Option<std::collections::HashMap<String, toml::Value>>,
}

impl Default for PythonManifestProvider {
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
    async fn test_pyproject_toml() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject_toml = temp_dir.path().join("pyproject.toml");

        let manifest_content = r#"
[project]
name = "test-python-project"
version = "0.1.0"
description = "A test Python project"
dependencies = [
    "requests>=2.25.0",
    "click>=8.0.0"
]

[tool.black]
line-length = 88

[build-system]
requires = ["setuptools>=45", "wheel"]
build-backend = "setuptools.build_meta"
"#;
        tokio::fs::write(&pyproject_toml, manifest_content)
            .await
            .unwrap();

        let provider = PythonManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&pyproject_toml, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&pyproject_toml, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "test-python-project");
        assert_eq!(metadata.version.as_ref().unwrap(), "0.1.0");
        assert_eq!(metadata.language, "python");
        assert!(
            metadata
                .dependencies
                .iter()
                .any(|d| d.starts_with("requests"))
        );
    }

    #[tokio::test]
    async fn test_setup_py() {
        let temp_dir = TempDir::new().unwrap();
        let setup_py = temp_dir.path().join("setup.py");

        let setup_content = r#"
from setuptools import setup, find_packages

setup(
    name="test-project",
    version="1.0.0",
    packages=find_packages(),
    install_requires=[
        "numpy>=1.20.0",
        "pandas>=1.3.0"
    ],
)
"#;
        tokio::fs::write(&setup_py, setup_content).await.unwrap();

        let provider = PythonManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&setup_py, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&setup_py, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "python");
        assert_eq!(
            metadata.additional_info.get("build_tool").unwrap(),
            "setuptools"
        );
    }

    #[tokio::test]
    async fn test_requirements_txt() {
        let temp_dir = TempDir::new().unwrap();
        let requirements_txt = temp_dir.path().join("requirements.txt");

        let requirements_content = r#"
# Production dependencies
requests>=2.25.0
click>=8.0.0
fastapi==0.68.0

# Optional dependencies
redis>=3.5.0  # For caching
"#;
        tokio::fs::write(&requirements_txt, requirements_content)
            .await
            .unwrap();

        let provider = PythonManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&requirements_txt, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&requirements_txt, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "python");
        assert!(metadata.dependencies.contains(&"requests".to_string()));
        assert!(metadata.dependencies.contains(&"click".to_string()));
        assert!(metadata.dependencies.contains(&"fastapi".to_string()));
        assert_eq!(metadata.additional_info.get("build_tool").unwrap(), "pip");
    }

    #[tokio::test]
    async fn test_pipfile() {
        let temp_dir = TempDir::new().unwrap();
        let pipfile = temp_dir.path().join("Pipfile");

        let pipfile_content = r#"
[packages]
requests = "*"
flask = ">=2.0.0"

[dev-packages]
pytest = "*"
black = "*"

[requires]
python_version = "3.9"
"#;
        tokio::fs::write(&pipfile, pipfile_content).await.unwrap();

        let provider = PythonManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&pipfile, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&pipfile, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "python");
        assert!(metadata.dependencies.contains(&"requests".to_string()));
        assert!(metadata.dev_dependencies.contains(&"pytest".to_string()));
        assert_eq!(
            metadata.additional_info.get("build_tool").unwrap(),
            "pipenv"
        );
    }

    #[tokio::test]
    async fn test_python_project_indicators() {
        let temp_dir = TempDir::new().unwrap();

        // Create Python project structure
        tokio::fs::write(temp_dir.path().join("__init__.py"), "")
            .await
            .unwrap();
        tokio::fs::write(temp_dir.path().join("main.py"), "print('hello')")
            .await
            .unwrap();

        let src_file = temp_dir.path().join("module").join("file.py");
        tokio::fs::create_dir_all(src_file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&src_file, "# python code").await.unwrap();

        let provider = PythonManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&src_file, 10, delegate);

        // Should detect as Python project based on indicators
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_extract_setup_field() {
        let setup_content = r#"
setup(
    name="my-package",
    version='1.2.3',
    description="A test package",
)
"#;

        assert_eq!(
            extract_setup_field(setup_content, "name"),
            Some("my-package".to_string())
        );
        assert_eq!(
            extract_setup_field(setup_content, "version"),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            extract_setup_field(setup_content, "description"),
            Some("A test package".to_string())
        );
        assert_eq!(extract_setup_field(setup_content, "nonexistent"), None);
    }
}
