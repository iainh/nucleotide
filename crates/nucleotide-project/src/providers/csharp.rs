// ABOUTME: C# project manifest provider for .NET projects and solutions
// ABOUTME: Handles modern .NET projects (.csproj), solutions (.sln), and MSBuild configurations

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::error::{ProjectError, Result};
use crate::manifest::{
    BaseManifestProvider, ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery,
    ProjectMetadata,
};

/// Manifest provider for C# / .NET projects
pub struct CSharpManifestProvider {
    base: BaseManifestProvider,
}

impl CSharpManifestProvider {
    pub fn new() -> Self {
        Self {
            base: BaseManifestProvider::new(
                "*.csproj",
                vec![
                    "*.csproj".to_string(),
                    "*.sln".to_string(),
                    "Directory.Build.props".to_string(),
                    "Directory.Build.targets".to_string(),
                    "global.json".to_string(),
                    "nuget.config".to_string(),
                    "*.fsproj".to_string(),
                    "*.vbproj".to_string(),
                ],
            )
            .with_priority(115), // High priority for .NET projects
        }
    }
}

#[async_trait]
impl ManifestProvider for CSharpManifestProvider {
    fn name(&self) -> ManifestName {
        // Return a more generic name since we handle multiple file types
        ManifestName::new("project.csproj")
    }

    fn priority(&self) -> u32 {
        self.base.priority
    }

    fn file_patterns(&self) -> Vec<String> {
        self.base.file_patterns.clone()
    }

    async fn search(&self, query: ManifestQuery) -> Result<Option<PathBuf>> {
        // For .NET, we want to find the outermost solution or the most appropriate project
        let mut outermost_project = None;

        for ancestor in query.path.ancestors().take(query.max_depth) {
            // Stop at filesystem roots
            if ancestor.parent().is_none() {
                break;
            }
            // First priority: Solution files (indicate multi-project)
            if let Some(sln_file) = self.find_solution_file(ancestor, &*query.delegate).await? {
                nucleotide_logging::info!(
                    solution_file = %sln_file.display(),
                    project_root = %ancestor.display(),
                    "Found .NET solution"
                );
                return Ok(Some(ancestor.to_path_buf()));
            }

            // Second priority: global.json (indicates .NET workspace)
            let global_json = ancestor.join("global.json");
            if query.delegate.exists(&global_json, Some(false)).await
                && self
                    .validate_global_json(&global_json, &*query.delegate)
                    .await?
            {
                nucleotide_logging::info!(
                    global_json = %global_json.display(),
                    project_root = %ancestor.display(),
                    "Found .NET workspace via global.json"
                );
                return Ok(Some(ancestor.to_path_buf()));
            }

            // Third priority: Directory.Build.props (MSBuild directory-level configuration)
            let directory_build_props = ancestor.join("Directory.Build.props");
            if query
                .delegate
                .exists(&directory_build_props, Some(false))
                .await
            {
                nucleotide_logging::info!(
                    directory_build_props = %directory_build_props.display(),
                    project_root = %ancestor.display(),
                    "Found .NET project root via Directory.Build.props"
                );
                return Ok(Some(ancestor.to_path_buf()));
            }

            // Fourth priority: Project files
            if let Some(project_file) = self.find_project_file(ancestor, &*query.delegate).await? {
                nucleotide_logging::debug!(
                    project_file = %project_file.display(),
                    "Found .NET project file"
                );

                if self
                    .validate_project_file(&project_file, &*query.delegate)
                    .await?
                {
                    outermost_project = Some(ancestor.to_path_buf());
                }
            }

            // Check for .NET project indicators (fallback) - continue to find outermost
            if self
                .has_dotnet_project_indicators(ancestor, &*query.delegate)
                .await?
            {
                nucleotide_logging::debug!(
                    project_root = %ancestor.display(),
                    "Found .NET project root based on indicators"
                );
                outermost_project = Some(ancestor.to_path_buf());
            }
        }

        if let Some(root) = &outermost_project {
            nucleotide_logging::info!(
                project_root = %root.display(),
                "Found .NET project root"
            );
        }

        Ok(outermost_project)
    }

    async fn validate_manifest(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if file_name.ends_with(".sln") {
            self.validate_solution_file(path, delegate).await
        } else if file_name.ends_with(".csproj")
            || file_name.ends_with(".fsproj")
            || file_name.ends_with(".vbproj")
        {
            self.validate_project_file(path, delegate).await
        } else if file_name == "global.json" {
            self.validate_global_json(path, delegate).await
        } else if file_name == "Directory.Build.props" || file_name == "Directory.Build.targets" {
            self.validate_directory_build_file(path, delegate).await
        } else {
            Ok(false)
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
            language: "csharp".to_string(),
            ..Default::default()
        };

        if file_name.ends_with(".sln") {
            self.extract_solution_metadata(manifest_path, delegate, &mut metadata)
                .await?;
        } else if file_name.ends_with(".csproj") {
            self.extract_csproj_metadata(manifest_path, delegate, &mut metadata)
                .await?;
        } else if file_name.ends_with(".fsproj") {
            self.extract_fsproj_metadata(manifest_path, delegate, &mut metadata)
                .await?;
            metadata.language = "fsharp".to_string();
        } else if file_name.ends_with(".vbproj") {
            self.extract_vbproj_metadata(manifest_path, delegate, &mut metadata)
                .await?;
            metadata.language = "vb.net".to_string();
        } else if file_name == "global.json" {
            self.extract_global_json_metadata(manifest_path, delegate, &mut metadata)
                .await?;
        }

        // Add additional .NET project information
        self.add_dotnet_environment_info(
            manifest_path.parent().unwrap_or(Path::new(".")),
            delegate,
            &mut metadata,
        )
        .await?;

        Ok(metadata)
    }
}

impl CSharpManifestProvider {
    async fn find_solution_file(
        &self,
        dir: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<Option<PathBuf>> {
        // In a real implementation, we'd scan the directory for .sln files
        // For simplicity, we'll check for common solution file patterns
        let common_sln_names = [
            "solution.sln",
            "Solution.sln",
            "app.sln",
            "App.sln",
            "main.sln",
            "Main.sln",
            "project.sln",
            "Project.sln",
        ];

        for sln_name in &common_sln_names {
            let sln_path = dir.join(sln_name);
            if delegate.exists(&sln_path, Some(false)).await {
                return Ok(Some(sln_path));
            }
        }

        // Check for any .sln file (this is a simplified approach)
        // In a real implementation, we'd use directory listing
        let _potential_paths = [
            "*.sln", // This would require directory scanning in a real implementation
        ];

        // For now, we'll return None and rely on other detection methods
        Ok(None)
    }

    async fn find_project_file(
        &self,
        dir: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<Option<PathBuf>> {
        let project_extensions = ["csproj", "fsproj", "vbproj"];
        let common_project_names = [
            "project", "Project", "app", "App", "main", "Main", "web", "Web", "api", "Api",
            "service", "Service",
        ];

        for extension in &project_extensions {
            for name in &common_project_names {
                let project_path = dir.join(format!("{}.{}", name, extension));
                if delegate.exists(&project_path, Some(false)).await {
                    return Ok(Some(project_path));
                }
            }
        }

        Ok(None)
    }

    async fn validate_solution_file(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for solution file markers
        let has_vs_version = content.contains("Microsoft Visual Studio Solution File");
        let has_project_section = content.contains("Project(") && content.contains("EndProject");
        let has_global_section = content.contains("Global") && content.contains("EndGlobal");

        nucleotide_logging::trace!(
            solution_file = %path.display(),
            has_vs_version = has_vs_version,
            has_project_section = has_project_section,
            has_global_section = has_global_section,
            "Validated solution file structure"
        );

        Ok(has_vs_version || has_project_section || has_global_section)
    }

    async fn validate_project_file(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for MSBuild project file markers
        let has_project_element = content.contains("<Project");
        let has_sdk_attribute = content.contains("Sdk=") || content.contains("ToolsVersion=");
        let has_target_framework =
            content.contains("<TargetFramework") || content.contains("<TargetFrameworks");
        let has_property_group = content.contains("<PropertyGroup");
        let has_item_group = content.contains("<ItemGroup");

        nucleotide_logging::trace!(
            project_file = %path.display(),
            has_project_element = has_project_element,
            has_sdk_attribute = has_sdk_attribute,
            has_target_framework = has_target_framework,
            has_property_group = has_property_group,
            has_item_group = has_item_group,
            "Validated project file structure"
        );

        // Valid if it has project element and typical MSBuild content
        Ok(has_project_element
            && (has_sdk_attribute || has_target_framework || has_property_group || has_item_group))
    }

    async fn validate_global_json(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        match serde_json::from_str::<GlobalJson>(&content) {
            Ok(global_json) => {
                nucleotide_logging::trace!(
                    global_json = %path.display(),
                    has_sdk = global_json.sdk.is_some(),
                    has_msbuild_sdks = global_json.msbuild_sdks.is_some(),
                    "Validated global.json structure"
                );

                Ok(global_json.sdk.is_some() || global_json.msbuild_sdks.is_some())
            }
            Err(e) => {
                nucleotide_logging::warn!(
                    global_json = %path.display(),
                    error = %e,
                    "Invalid JSON in global.json"
                );
                Err(ProjectError::manifest_parse(path.to_path_buf(), e))
            }
        }
    }

    async fn validate_directory_build_file(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for MSBuild content
        let has_project_element = content.contains("<Project");
        let has_property_group = content.contains("<PropertyGroup");
        let has_item_group = content.contains("<ItemGroup");

        Ok(has_project_element && (has_property_group || has_item_group))
    }

    async fn has_dotnet_project_indicators(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let indicators = [
            "bin",                 // Build output
            "obj",                 // Build intermediate
            "packages",            // NuGet packages (legacy)
            "packages.config",     // NuGet packages config
            "app.config",          // Application config
            "web.config",          // Web application config
            "App.config",          // Application config (capitalized)
            "Web.config",          // Web application config (capitalized)
            "AssemblyInfo.cs",     // Assembly info
            "Program.cs",          // Common entry point
            "Startup.cs",          // ASP.NET Core startup
            "appsettings.json",    // Configuration file
            "launchSettings.json", // Debug launch settings
            ".vs",                 // Visual Studio directory
            "Properties",          // Properties directory
        ];

        for indicator in &indicators {
            let indicator_path = path.join(indicator);
            if delegate.exists(&indicator_path, None).await {
                nucleotide_logging::debug!(
                    indicator = indicator,
                    path = %path.display(),
                    "Found .NET project indicator"
                );
                return Ok(true);
            }
        }

        // Check for C# source files
        if self.directory_contains_csharp_files(path, delegate).await? {
            return Ok(true);
        }

        Ok(false)
    }

    async fn directory_contains_csharp_files(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let csharp_files = [
            "Program.cs",
            "App.cs",
            "Application.cs",
            "Main.cs",
            "Startup.cs",
            "Controller.cs",
            "Service.cs",
            "Model.cs",
        ];

        for cs_file in &csharp_files {
            let file_path = path.join(cs_file);
            if delegate.exists(&file_path, Some(false)).await {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn extract_solution_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Extract solution name from file name
        if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
            metadata.name = Some(file_stem.to_string());
        }

        // Parse project references from solution
        let projects = self.extract_solution_projects(&content);
        if !projects.is_empty() {
            metadata
                .additional_info
                .insert("solution_projects".to_string(), projects.join(","));
        }

        metadata
            .additional_info
            .insert("project_type".to_string(), "solution".to_string());

        Ok(())
    }

    async fn extract_csproj_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Extract project name from file name
        if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
            metadata.name = Some(file_stem.to_string());
        }

        // Extract MSBuild properties
        if let Some(target_framework) = self.extract_xml_element(&content, "TargetFramework") {
            metadata
                .additional_info
                .insert("target_framework".to_string(), target_framework);
        }

        if let Some(target_frameworks) = self.extract_xml_element(&content, "TargetFrameworks") {
            metadata
                .additional_info
                .insert("target_frameworks".to_string(), target_frameworks);
        }

        if let Some(version) = self.extract_xml_element(&content, "Version") {
            metadata.version = Some(version);
        }

        if let Some(description) = self.extract_xml_element(&content, "Description") {
            metadata.description = Some(description);
        }

        if let Some(sdk) = self.extract_project_sdk(&content) {
            metadata.additional_info.insert("sdk".to_string(), sdk);
        }

        // Extract package references
        let package_references = self.extract_package_references(&content);
        metadata.dependencies = package_references;

        metadata
            .additional_info
            .insert("project_type".to_string(), "csproj".to_string());

        Ok(())
    }

    async fn extract_fsproj_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        // F# projects use similar structure to C# projects
        self.extract_csproj_metadata(path, delegate, metadata)
            .await?;
        metadata
            .additional_info
            .insert("project_type".to_string(), "fsproj".to_string());
        Ok(())
    }

    async fn extract_vbproj_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        // VB.NET projects use similar structure to C# projects
        self.extract_csproj_metadata(path, delegate, metadata)
            .await?;
        metadata
            .additional_info
            .insert("project_type".to_string(), "vbproj".to_string());
        Ok(())
    }

    async fn extract_global_json_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;
        let global_json: GlobalJson = serde_json::from_str(&content)
            .map_err(|e| ProjectError::manifest_parse(path.to_path_buf(), e))?;

        if let Some(sdk) = global_json.sdk {
            if let Some(version) = sdk.version {
                metadata
                    .additional_info
                    .insert("dotnet_sdk_version".to_string(), version);
            }
        }

        metadata
            .additional_info
            .insert("project_type".to_string(), "workspace".to_string());

        Ok(())
    }

    async fn add_dotnet_environment_info(
        &self,
        project_root: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        // Check for build outputs
        if delegate.exists(&project_root.join("bin"), Some(true)).await {
            metadata
                .additional_info
                .insert("has_bin_dir".to_string(), "true".to_string());
        }

        if delegate.exists(&project_root.join("obj"), Some(true)).await {
            metadata
                .additional_info
                .insert("has_obj_dir".to_string(), "true".to_string());
        }

        // Check for package management
        if delegate
            .exists(&project_root.join("packages.config"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("package_manager".to_string(), "packages.config".to_string());
        } else {
            metadata.additional_info.insert(
                "package_manager".to_string(),
                "packagereference".to_string(),
            );
        }

        // Check for IDE files
        if delegate.exists(&project_root.join(".vs"), Some(true)).await {
            metadata
                .additional_info
                .insert("has_visual_studio_config".to_string(), "true".to_string());
        }

        // Check for configuration files
        let config_files = [
            ("appsettings.json", "appsettings"),
            ("appsettings.Development.json", "appsettings_dev"),
            ("web.config", "web_config"),
            ("app.config", "app_config"),
            ("launchSettings.json", "launch_settings"),
            ("nuget.config", "nuget_config"),
        ];

        for (file, flag) in &config_files {
            if delegate.exists(&project_root.join(file), Some(false)).await {
                metadata
                    .additional_info
                    .insert(format!("has_{}", flag), "true".to_string());
            }
        }

        // Check for common .NET project directories
        let project_dirs = [
            ("Controllers", "has_controllers"),
            ("Models", "has_models"),
            ("Views", "has_views"),
            ("Services", "has_services"),
            ("Data", "has_data"),
            ("wwwroot", "has_wwwroot"),
            ("Pages", "has_pages"),
            ("Components", "has_components"),
        ];

        for (dir, flag) in &project_dirs {
            if delegate.exists(&project_root.join(dir), Some(true)).await {
                metadata
                    .additional_info
                    .insert(flag.to_string(), "true".to_string());
            }
        }

        Ok(())
    }

    fn extract_xml_element(&self, content: &str, element: &str) -> Option<String> {
        let start_tag = format!("<{}>", element);
        let end_tag = format!("</{}>", element);

        if let Some(start) = content.find(&start_tag) {
            if let Some(end) = content[start..].find(&end_tag) {
                let value_start = start + start_tag.len();
                let value_end = start + end;
                return Some(content[value_start..value_end].trim().to_string());
            }
        }

        None
    }

    fn extract_project_sdk(&self, content: &str) -> Option<String> {
        // Extract SDK from <Project Sdk="...">
        if let Some(start) = content.find("<Project") {
            if let Some(sdk_start) = content[start..].find("Sdk=\"") {
                let sdk_pos = start + sdk_start + 5; // 5 = len("Sdk=\"")
                if let Some(end) = content[sdk_pos..].find('"') {
                    return Some(content[sdk_pos..sdk_pos + end].to_string());
                }
            }
        }
        None
    }

    fn extract_solution_projects(&self, content: &str) -> Vec<String> {
        let mut projects = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Project(") && trimmed.contains("=") {
                // Extract project name from: Project("{...}") = "ProjectName", "path", "{...}"
                if let Some(quote_start) = trimmed.find("= \"") {
                    let name_start = quote_start + 3;
                    if let Some(quote_end) = trimmed[name_start..].find('"') {
                        let project_name = &trimmed[name_start..name_start + quote_end];
                        projects.push(project_name.to_string());
                    }
                }
            }
        }

        projects
    }

    fn extract_package_references(&self, content: &str) -> Vec<String> {
        let mut packages = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.contains("<PackageReference") && trimmed.contains("Include=\"") {
                // Extract package name from: <PackageReference Include="PackageName" Version="..." />
                if let Some(include_start) = trimmed.find("Include=\"") {
                    let name_start = include_start + 9; // 9 = len("Include=\"")
                    if let Some(quote_end) = trimmed[name_start..].find('"') {
                        let package_name = &trimmed[name_start..name_start + quote_end];
                        packages.push(package_name.to_string());
                    }
                }
            }
        }

        packages
    }
}

/// Simplified global.json structure for parsing
#[derive(serde::Deserialize)]
struct GlobalJson {
    sdk: Option<DotNetSdk>,
    #[serde(rename = "msbuild-sdks")]
    msbuild_sdks: Option<std::collections::HashMap<String, String>>,
}

#[derive(serde::Deserialize)]
struct DotNetSdk {
    version: Option<String>,
    #[serde(rename = "rollForward")]
    roll_forward: Option<String>,
}

impl Default for CSharpManifestProvider {
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
    async fn test_csproj_modern() {
        let temp_dir = TempDir::new().unwrap();
        let csproj = temp_dir.path().join("TestProject.csproj");

        let manifest_content = r#"
<Project Sdk="Microsoft.NET.Sdk">

  <PropertyGroup>
    <TargetFramework>net6.0</TargetFramework>
    <Version>1.0.0</Version>
    <Description>A test .NET project</Description>
    <OutputType>Exe</OutputType>
  </PropertyGroup>

  <ItemGroup>
    <PackageReference Include="Newtonsoft.Json" Version="13.0.1" />
    <PackageReference Include="Microsoft.Extensions.Hosting" Version="6.0.1" />
  </ItemGroup>

</Project>
"#;
        tokio::fs::write(&csproj, manifest_content).await.unwrap();

        let provider = CSharpManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&csproj, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&csproj, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "TestProject");
        assert_eq!(metadata.version.as_ref().unwrap(), "1.0.0");
        assert_eq!(metadata.language, "csharp");
        assert_eq!(
            metadata.additional_info.get("target_framework").unwrap(),
            "net6.0"
        );
        assert_eq!(
            metadata.additional_info.get("sdk").unwrap(),
            "Microsoft.NET.Sdk"
        );
        assert!(
            metadata
                .dependencies
                .contains(&"Newtonsoft.Json".to_string())
        );
        assert!(
            metadata
                .dependencies
                .contains(&"Microsoft.Extensions.Hosting".to_string())
        );
    }

    #[tokio::test]
    async fn test_solution_file() {
        let temp_dir = TempDir::new().unwrap();
        let sln = temp_dir.path().join("TestSolution.sln");

        let solution_content = r#"
Microsoft Visual Studio Solution File, Format Version 12.00
# Visual Studio Version 17
VisualStudioVersion = 17.0.31903.59
MinimumVisualStudioVersion = 10.0.40219.1
Project("{9A19103F-16F7-4668-BE54-9A1E7A4F7556}") = "WebApp", "WebApp\WebApp.csproj", "{12345678-1234-1234-1234-123456789012}"
EndProject
Project("{9A19103F-16F7-4668-BE54-9A1E7A4F7556}") = "ClassLibrary", "ClassLibrary\ClassLibrary.csproj", "{87654321-4321-4321-4321-210987654321}"
EndProject
Global
	GlobalSection(SolutionConfigurationPlatforms) = preSolution
		Debug|Any CPU = Debug|Any CPU
		Release|Any CPU = Release|Any CPU
	EndGlobalSection
	GlobalSection(ProjectConfigurationPlatforms) = postSolution
		{12345678-1234-1234-1234-123456789012}.Debug|Any CPU.ActiveCfg = Debug|Any CPU
		{12345678-1234-1234-1234-123456789012}.Debug|Any CPU.Build.0 = Debug|Any CPU
	EndGlobalSection
EndGlobal
"#;
        tokio::fs::write(&sln, solution_content).await.unwrap();

        let provider = CSharpManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider.validate_manifest(&sln, &*delegate).await.unwrap());

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&sln, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "TestSolution");
        assert_eq!(metadata.language, "csharp");
        assert_eq!(
            metadata.additional_info.get("project_type").unwrap(),
            "solution"
        );
        assert!(
            metadata
                .additional_info
                .get("solution_projects")
                .unwrap()
                .contains("WebApp")
        );
        assert!(
            metadata
                .additional_info
                .get("solution_projects")
                .unwrap()
                .contains("ClassLibrary")
        );
    }

    #[tokio::test]
    async fn test_global_json() {
        let temp_dir = TempDir::new().unwrap();
        let global_json = temp_dir.path().join("global.json");

        let json_content = r#"
{
  "sdk": {
    "version": "6.0.100",
    "rollForward": "latestFeature"
  },
  "msbuild-sdks": {
    "Microsoft.Build.Traversal": "3.0.23"
  }
}
"#;
        tokio::fs::write(&global_json, json_content).await.unwrap();

        let provider = CSharpManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&global_json, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&global_json, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "csharp");
        assert_eq!(
            metadata.additional_info.get("dotnet_sdk_version").unwrap(),
            "6.0.100"
        );
        assert_eq!(
            metadata.additional_info.get("project_type").unwrap(),
            "workspace"
        );
    }

    #[tokio::test]
    async fn test_fsharp_project() {
        let temp_dir = TempDir::new().unwrap();
        let fsproj = temp_dir.path().join("FSharpProject.fsproj");

        let manifest_content = r#"
<Project Sdk="Microsoft.NET.Sdk">

  <PropertyGroup>
    <TargetFramework>net6.0</TargetFramework>
    <GenerateDocumentationFile>true</GenerateDocumentationFile>
  </PropertyGroup>

  <ItemGroup>
    <Compile Include="Program.fs" />
  </ItemGroup>

  <ItemGroup>
    <PackageReference Include="FSharp.Core" Version="6.0.1" />
  </ItemGroup>

</Project>
"#;
        tokio::fs::write(&fsproj, manifest_content).await.unwrap();

        let provider = CSharpManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&fsproj, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&fsproj, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "FSharpProject");
        assert_eq!(metadata.language, "fsharp");
        assert_eq!(
            metadata.additional_info.get("project_type").unwrap(),
            "fsproj"
        );
        assert!(metadata.dependencies.contains(&"FSharp.Core".to_string()));
    }

    #[tokio::test]
    async fn test_directory_build_props() {
        let temp_dir = TempDir::new().unwrap();
        let directory_build = temp_dir.path().join("Directory.Build.props");

        let build_content = r#"
<Project>

  <PropertyGroup>
    <TargetFramework>net6.0</TargetFramework>
    <TreatWarningsAsErrors>true</TreatWarningsAsErrors>
    <Company>Example Corp</Company>
  </PropertyGroup>

  <ItemGroup>
    <PackageReference Include="StyleCop.Analyzers" Version="1.1.118" PrivateAssets="All" />
  </ItemGroup>

</Project>
"#;
        tokio::fs::write(&directory_build, build_content)
            .await
            .unwrap();

        let provider = CSharpManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&directory_build, &*delegate)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_dotnet_project_indicators() {
        let temp_dir = TempDir::new().unwrap();

        // Create .NET project structure
        tokio::fs::write(
            temp_dir.path().join("Program.cs"),
            "Console.WriteLine(\"Hello\");",
        )
        .await
        .unwrap();
        let bin_dir = temp_dir.path().join("bin");
        tokio::fs::create_dir_all(&bin_dir).await.unwrap();

        let nested_file = temp_dir
            .path()
            .join("Controllers")
            .join("HomeController.cs");
        tokio::fs::create_dir_all(nested_file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&nested_file, "public class HomeController {}")
            .await
            .unwrap();

        let provider = CSharpManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&nested_file, 10, delegate);

        // Should detect as .NET project based on indicators
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_xml_element_extraction() {
        let provider = CSharpManifestProvider::new();

        let xml_content = r#"
        <TargetFramework>net6.0</TargetFramework>
        <Version>1.2.3</Version>
        <Description>Test project</Description>
        "#;

        assert_eq!(
            provider.extract_xml_element(xml_content, "TargetFramework"),
            Some("net6.0".to_string())
        );
        assert_eq!(
            provider.extract_xml_element(xml_content, "Version"),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            provider.extract_xml_element(xml_content, "Description"),
            Some("Test project".to_string())
        );
        assert_eq!(
            provider.extract_xml_element(xml_content, "NonExistent"),
            None
        );
    }

    #[tokio::test]
    async fn test_package_reference_extraction() {
        let provider = CSharpManifestProvider::new();

        let csproj_content = r#"
        <PackageReference Include="Newtonsoft.Json" Version="13.0.1" />
        <PackageReference Include="Microsoft.Extensions.Hosting" Version="6.0.1" />
        <PackageReference Include="Serilog" Version="2.10.0" />
        "#;

        let packages = provider.extract_package_references(csproj_content);
        assert_eq!(packages.len(), 3);
        assert!(packages.contains(&"Newtonsoft.Json".to_string()));
        assert!(packages.contains(&"Microsoft.Extensions.Hosting".to_string()));
        assert!(packages.contains(&"Serilog".to_string()));
    }

    #[tokio::test]
    async fn test_invalid_manifests() {
        let temp_dir = TempDir::new().unwrap();
        let provider = CSharpManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Invalid project file (no Project element)
        let invalid_csproj = temp_dir.path().join("invalid.csproj");
        tokio::fs::write(&invalid_csproj, "<root><element>content</element></root>")
            .await
            .unwrap();

        let result = provider
            .validate_manifest(&invalid_csproj, &*delegate)
            .await;
        assert!(result.is_ok());
        assert!(!result.unwrap());

        // Invalid global.json
        let invalid_global = temp_dir.path().join("global.json");
        tokio::fs::write(&invalid_global, "{ invalid json")
            .await
            .unwrap();

        let result = provider
            .validate_manifest(&invalid_global, &*delegate)
            .await;
        assert!(result.is_err());
    }
}
