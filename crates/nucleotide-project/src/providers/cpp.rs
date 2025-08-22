// ABOUTME: C++ project manifest provider for CMake, Make, and language server configurations
// ABOUTME: Handles modern C++ projects with CMake, traditional Makefiles, and clangd configurations

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::error::{ProjectError, Result, WithPathContext};
use crate::manifest::{
    BaseManifestProvider, ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery,
    ProjectMetadata,
};

/// Manifest provider for C++ projects
pub struct CppManifestProvider {
    base: BaseManifestProvider,
}

impl CppManifestProvider {
    pub fn new() -> Self {
        Self {
            base: BaseManifestProvider::new(
                "CMakeLists.txt",
                vec![
                    "CMakeLists.txt".to_string(),
                    "Makefile".to_string(),
                    "makefile".to_string(),
                    "GNUmakefile".to_string(),
                    ".clangd".to_string(),
                    "compile_commands.json".to_string(),
                    "conanfile.txt".to_string(),
                    "conanfile.py".to_string(),
                    "vcpkg.json".to_string(),
                    "CMakePresets.json".to_string(),
                    "meson.build".to_string(),
                ],
            )
            .with_priority(105), // Standard priority for C++ projects
        }
    }
}

#[async_trait]
impl ManifestProvider for CppManifestProvider {
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
        // Priority order for C++ project detection
        let priority_patterns = [
            "CMakeLists.txt",        // CMake (modern standard)
            "CMakePresets.json",     // CMake presets
            "meson.build",           // Meson build system
            "Makefile",              // Traditional Make
            "makefile",              // Traditional Make (lowercase)
            "GNUmakefile",           // GNU Make specific
            "conanfile.txt",         // Conan package manager
            "conanfile.py",          // Conan package manager (Python)
            "vcpkg.json",            // vcpkg package manager
            ".clangd",               // Language server config
            "compile_commands.json", // Compilation database
        ];

        // For C++, we want to find the outermost build configuration
        let mut outermost_project = None;

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
            // Check for CMake workspace (multiple CMakeLists.txt indicating workspace)
            let cmake_file = ancestor.join("CMakeLists.txt");
            if query.delegate.exists(&cmake_file, Some(false)).await
                && self
                    .validate_manifest(&cmake_file, &*query.delegate)
                    .await?
            {
                outermost_project = Some(ancestor.to_path_buf());

                // Check if this is a CMake workspace (contains subdirectories with CMakeLists.txt)
                if self
                    .is_cmake_workspace(&cmake_file, ancestor, &*query.delegate)
                    .await?
                {
                    nucleotide_logging::info!(
                        cmake_workspace = %ancestor.display(),
                        "Found CMake workspace root"
                    );
                    return Ok(outermost_project);
                }
            }

            // Check for other high-priority build files
            for pattern in &priority_patterns {
                if *pattern == "CMakeLists.txt" {
                    continue; // Already handled above
                }

                let manifest_path = ancestor.join(pattern);

                if query.delegate.exists(&manifest_path, Some(false)).await {
                    nucleotide_logging::debug!(
                        cpp_manifest = %manifest_path.display(),
                        pattern = pattern,
                        "Found C++ manifest file"
                    );

                    if self
                        .validate_cpp_manifest(&manifest_path, pattern, &*query.delegate)
                        .await?
                    {
                        // For package managers and presets, check if they're at workspace level
                        if matches!(
                            *pattern,
                            "CMakePresets.json" | "conanfile.txt" | "conanfile.py" | "vcpkg.json"
                        ) {
                            nucleotide_logging::info!(
                                workspace_root = %ancestor.display(),
                                manifest_type = pattern,
                                "Found C++ workspace root via package manager"
                            );
                            return Ok(Some(ancestor.to_path_buf()));
                        }

                        outermost_project = Some(ancestor.to_path_buf());
                    }
                }
            }

            // Check for C++ project indicators (fallback) - continue to find outermost
            if self
                .has_cpp_project_indicators(ancestor, &*query.delegate)
                .await?
            {
                nucleotide_logging::debug!(
                    project_root = %ancestor.display(),
                    "Found C++ project root based on indicators"
                );
                outermost_project = Some(ancestor.to_path_buf());
            }
        }

        if let Some(root) = &outermost_project {
            nucleotide_logging::info!(
                project_root = %root.display(),
                "Found C++ project root"
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

        self.validate_cpp_manifest(path, file_name, delegate).await
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
            language: "cpp".to_string(),
            ..Default::default()
        };

        match file_name {
            "CMakeLists.txt" => {
                self.extract_cmake_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "Makefile" | "makefile" | "GNUmakefile" => {
                self.extract_makefile_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "meson.build" => {
                self.extract_meson_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "conanfile.txt" | "conanfile.py" => {
                self.extract_conan_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "vcpkg.json" => {
                self.extract_vcpkg_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "CMakePresets.json" => {
                self.extract_cmake_presets_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            ".clangd" => {
                self.extract_clangd_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "compile_commands.json" => {
                self.extract_compile_commands_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            _ => {}
        }

        // Add additional C++ project information
        self.add_cpp_environment_info(
            manifest_path.parent().unwrap_or(Path::new(".")),
            delegate,
            &mut metadata,
        )
        .await?;

        Ok(metadata)
    }
}

impl CppManifestProvider {
    async fn validate_cpp_manifest(
        &self,
        path: &Path,
        file_name: &str,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        match file_name {
            "CMakeLists.txt" => self.validate_cmake_file(path, delegate).await,
            "Makefile" | "makefile" | "GNUmakefile" => self.validate_makefile(path, delegate).await,
            "meson.build" => self.validate_meson_file(path, delegate).await,
            "conanfile.txt" => self.validate_conanfile_txt(path, delegate).await,
            "conanfile.py" => self.validate_conanfile_py(path, delegate).await,
            "vcpkg.json" => self.validate_vcpkg_json(path, delegate).await,
            "CMakePresets.json" => self.validate_cmake_presets(path, delegate).await,
            ".clangd" => self.validate_clangd_config(path, delegate).await,
            "compile_commands.json" => self.validate_compile_commands(path, delegate).await,
            _ => Ok(false),
        }
    }

    async fn validate_cmake_file(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for CMake-specific commands
        let cmake_commands = [
            "cmake_minimum_required",
            "project(",
            "add_executable(",
            "add_library(",
            "target_link_libraries(",
            "find_package(",
            "set(",
            "option(",
            "include(",
        ];

        let has_cmake_content = cmake_commands.iter().any(|cmd| content.contains(cmd));

        nucleotide_logging::trace!(
            cmake_file = %path.display(),
            has_cmake_content = has_cmake_content,
            "Validated CMakeLists.txt structure"
        );

        Ok(has_cmake_content)
    }

    async fn validate_makefile(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for Makefile patterns
        let makefile_patterns = [
            "CC=",
            "CXX=",
            "CFLAGS=",
            "CXXFLAGS=",
            "LDFLAGS=",
            ".PHONY:",
            "all:",
            "clean:",
            "install:",
            "\t",
            "$(",
            "${", // Typical Makefile syntax
        ];

        let has_makefile_content = makefile_patterns
            .iter()
            .any(|pattern| content.contains(pattern));

        nucleotide_logging::trace!(
            makefile = %path.display(),
            has_makefile_content = has_makefile_content,
            "Validated Makefile structure"
        );

        Ok(has_makefile_content)
    }

    async fn validate_meson_file(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for Meson-specific functions
        let meson_functions = [
            "project(",
            "executable(",
            "library(",
            "dependency(",
            "subdir(",
            "configure_file(",
            "meson.version",
        ];

        let has_meson_content = meson_functions.iter().any(|func| content.contains(func));

        nucleotide_logging::trace!(
            meson_build = %path.display(),
            has_meson_content = has_meson_content,
            "Validated meson.build structure"
        );

        Ok(has_meson_content)
    }

    async fn validate_conanfile_txt(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for Conan sections
        let has_requires = content.contains("[requires]");
        let has_generators = content.contains("[generators]");
        let has_options = content.contains("[options]");

        Ok(has_requires || has_generators || has_options)
    }

    async fn validate_conanfile_py(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for Conan Python API
        let conan_indicators = [
            "from conan import",
            "ConanFile",
            "def requirements(self)",
            "def configure(self)",
            "def build(self)",
            "self.requires(",
            "self.tool_requires(",
        ];

        let has_conan_content = conan_indicators
            .iter()
            .any(|indicator| content.contains(indicator));

        Ok(has_conan_content)
    }

    async fn validate_vcpkg_json(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        match serde_json::from_str::<VcpkgManifest>(&content) {
            Ok(manifest) => {
                nucleotide_logging::trace!(
                    vcpkg_json = %path.display(),
                    has_name = manifest.name.is_some(),
                    has_dependencies = manifest.dependencies.is_some(),
                    "Validated vcpkg.json structure"
                );

                Ok(manifest.name.is_some() || manifest.dependencies.is_some())
            }
            Err(e) => {
                nucleotide_logging::warn!(
                    vcpkg_json = %path.display(),
                    error = %e,
                    "Invalid JSON in vcpkg.json"
                );
                Err(ProjectError::manifest_parse(path.to_path_buf(), e))
            }
        }
    }

    async fn validate_cmake_presets(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        match serde_json::from_str::<CMakePresets>(&content) {
            Ok(presets) => Ok(presets.version.is_some()
                || presets.configure_presets.is_some()
                || presets.build_presets.is_some()),
            Err(e) => {
                nucleotide_logging::warn!(
                    cmake_presets = %path.display(),
                    error = %e,
                    "Invalid JSON in CMakePresets.json"
                );
                Err(ProjectError::manifest_parse(path.to_path_buf(), e))
            }
        }
    }

    async fn validate_clangd_config(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for clangd configuration options
        let clangd_options = [
            "CompileFlags:",
            "Index:",
            "Diagnostics:",
            "Hover:",
            "Add:",
            "Remove:",
            "Compiler:",
            "Background:",
        ];

        let has_clangd_content = clangd_options.iter().any(|option| content.contains(option));

        Ok(has_clangd_content)
    }

    async fn validate_compile_commands(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for compilation database structure
        let has_json_array = content.trim().starts_with('[') && content.trim().ends_with(']');
        let has_compile_fields = content.contains("\"command\"")
            || content.contains("\"file\"")
            || content.contains("\"directory\"");

        Ok(has_json_array && has_compile_fields)
    }

    async fn is_cmake_workspace(
        &self,
        cmake_file: &Path,
        project_root: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(cmake_file).await?;

        // Check for add_subdirectory commands that might indicate workspace structure
        let has_subdirectories = content.contains("add_subdirectory(");

        if !has_subdirectories {
            return Ok(false);
        }

        // Look for actual subdirectories with CMakeLists.txt
        let common_subdirs = [
            "src",
            "lib",
            "libs",
            "modules",
            "components",
            "apps",
            "examples",
            "tests",
        ];

        for subdir in &common_subdirs {
            let subdir_cmake = project_root.join(subdir).join("CMakeLists.txt");
            if delegate.exists(&subdir_cmake, Some(false)).await {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn has_cpp_project_indicators(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let indicators = [
            "src",               // Source directory
            "include",           // Headers directory
            "inc",               // Headers directory (alternative)
            "headers",           // Headers directory
            "lib",               // Library directory
            "libs",              // Libraries directory
            "bin",               // Binary output
            "build",             // Build directory
            "cmake",             // CMake modules
            "CMakeModules",      // CMake modules
            "third_party",       // Third party libraries
            "vendor",            // Vendor libraries
            "external",          // External dependencies
            ".vscode",           // VS Code C++ config
            ".clang-format",     // Clang formatter config
            ".clang-tidy",       // Clang tidy config
            "compile_flags.txt", // Compilation flags
        ];

        for indicator in &indicators {
            let indicator_path = path.join(indicator);
            if delegate.exists(&indicator_path, None).await {
                nucleotide_logging::debug!(
                    indicator = indicator,
                    path = %path.display(),
                    "Found C++ project indicator"
                );
                return Ok(true);
            }
        }

        // Check for C++ source files
        if self.directory_contains_cpp_files(path, delegate).await? {
            return Ok(true);
        }

        Ok(false)
    }

    async fn directory_contains_cpp_files(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let cpp_files = [
            "main.cpp",
            "main.cc",
            "main.cxx",
            "main.c",
            "app.cpp",
            "app.cc",
            "application.cpp",
            "test.cpp",
            "test.cc",
            "tests.cpp",
            "example.cpp",
            "example.cc",
            "demo.cpp",
        ];

        for cpp_file in &cpp_files {
            let file_path = path.join(cpp_file);
            if delegate.exists(&file_path, Some(false)).await {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn extract_cmake_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Extract project name and version from CMake
        if let Some((project_name, version)) = self.extract_cmake_project(&content) {
            metadata.name = Some(project_name);
            if let Some(v) = version {
                metadata.version = Some(v);
            }
        }

        // Extract CMake minimum version
        if let Some(min_version) = self.extract_cmake_minimum_required(&content) {
            metadata
                .additional_info
                .insert("cmake_minimum_required".to_string(), min_version);
        }

        // Extract C++ standard
        if let Some(cxx_standard) = self.extract_cmake_cxx_standard(&content) {
            metadata
                .additional_info
                .insert("cpp_standard".to_string(), cxx_standard);
        }

        metadata
            .additional_info
            .insert("build_system".to_string(), "cmake".to_string());

        // Extract dependencies (find_package calls)
        let dependencies = self.extract_cmake_dependencies(&content);
        metadata.dependencies = dependencies;

        Ok(())
    }

    async fn extract_makefile_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Extract project name from directory or common variables
        if let Some(file_parent) = path.parent() {
            if let Some(dir_name) = file_parent.file_name().and_then(|n| n.to_str()) {
                metadata.name = Some(dir_name.to_string());
            }
        }

        // Extract compiler information
        if let Some(compiler) = self.extract_makefile_variable(&content, "CXX") {
            metadata
                .additional_info
                .insert("compiler".to_string(), compiler);
        } else if content.contains("g++") {
            metadata
                .additional_info
                .insert("compiler".to_string(), "g++".to_string());
        } else if content.contains("clang++") {
            metadata
                .additional_info
                .insert("compiler".to_string(), "clang++".to_string());
        }

        // Extract C++ standard
        if let Some(cxx_standard) = self.extract_makefile_cxx_standard(&content) {
            metadata
                .additional_info
                .insert("cpp_standard".to_string(), cxx_standard);
        }

        metadata
            .additional_info
            .insert("build_system".to_string(), "make".to_string());

        Ok(())
    }

    async fn extract_meson_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Extract project information from Meson
        if let Some((project_name, version)) = self.extract_meson_project(&content) {
            metadata.name = Some(project_name);
            if let Some(v) = version {
                metadata.version = Some(v);
            }
        }

        metadata
            .additional_info
            .insert("build_system".to_string(), "meson".to_string());

        // Extract dependencies
        let dependencies = self.extract_meson_dependencies(&content);
        metadata.dependencies = dependencies;

        Ok(())
    }

    async fn extract_conan_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Extract dependencies from Conan
        let dependencies = if path.file_name().and_then(|n| n.to_str()) == Some("conanfile.txt") {
            self.extract_conanfile_txt_dependencies(&content)
        } else {
            self.extract_conanfile_py_dependencies(&content)
        };

        metadata.dependencies = dependencies;
        metadata
            .additional_info
            .insert("package_manager".to_string(), "conan".to_string());

        Ok(())
    }

    async fn extract_vcpkg_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;
        let vcpkg_manifest: VcpkgManifest =
            serde_json::from_str(&content).with_path_context(path.to_path_buf())?;

        if let Some(name) = vcpkg_manifest.name {
            metadata.name = Some(name);
        }

        if let Some(version) = vcpkg_manifest.version {
            metadata.version = Some(version);
        }

        if let Some(dependencies) = vcpkg_manifest.dependencies {
            metadata.dependencies = dependencies
                .into_iter()
                .map(|dep| match dep {
                    VcpkgDependency::String(s) => s,
                    VcpkgDependency::Object(obj) => obj.name,
                })
                .collect();
        }

        metadata
            .additional_info
            .insert("package_manager".to_string(), "vcpkg".to_string());

        Ok(())
    }

    async fn extract_cmake_presets_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;
        let presets: CMakePresets =
            serde_json::from_str(&content).with_path_context(path.to_path_buf())?;

        if let Some(version) = presets.version {
            metadata
                .additional_info
                .insert("cmake_presets_version".to_string(), version.to_string());
        }

        metadata
            .additional_info
            .insert("build_system".to_string(), "cmake".to_string());
        metadata
            .additional_info
            .insert("has_cmake_presets".to_string(), "true".to_string());

        Ok(())
    }

    async fn extract_clangd_metadata(
        &self,
        _path: &Path,
        _delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        metadata
            .additional_info
            .insert("has_clangd_config".to_string(), "true".to_string());
        Ok(())
    }

    async fn extract_compile_commands_metadata(
        &self,
        _path: &Path,
        _delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        metadata
            .additional_info
            .insert("has_compile_commands".to_string(), "true".to_string());
        Ok(())
    }

    async fn add_cpp_environment_info(
        &self,
        project_root: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        // Check for build directories
        let build_dirs = ["build", "cmake-build-debug", "cmake-build-release", "out"];
        for build_dir in &build_dirs {
            if delegate
                .exists(&project_root.join(build_dir), Some(true))
                .await
            {
                metadata
                    .additional_info
                    .insert("has_build_dir".to_string(), build_dir.to_string());
                break;
            }
        }

        // Check for common C++ project structure
        let structure_indicators = [
            ("src", "has_src_dir"),
            ("include", "has_include_dir"),
            ("lib", "has_lib_dir"),
            ("libs", "has_libs_dir"),
            ("test", "has_test_dir"),
            ("tests", "has_tests_dir"),
            ("examples", "has_examples_dir"),
            ("docs", "has_docs_dir"),
            ("third_party", "has_third_party"),
            ("vendor", "has_vendor"),
            ("external", "has_external"),
        ];

        for (dir, flag) in &structure_indicators {
            if delegate.exists(&project_root.join(dir), Some(true)).await {
                metadata
                    .additional_info
                    .insert(flag.to_string(), "true".to_string());
            }
        }

        // Check for configuration files
        let config_files = [
            (".clang-format", "clang_format"),
            (".clang-tidy", "clang_tidy"),
            ("compile_flags.txt", "compile_flags"),
            (".gitignore", "gitignore"),
            ("README.md", "readme"),
            ("LICENSE", "license"),
            ("Dockerfile", "docker"),
            (".github", "github_actions"),
        ];

        for (file, tool) in &config_files {
            if delegate.exists(&project_root.join(file), None).await {
                metadata
                    .additional_info
                    .insert(format!("has_{}", tool), "true".to_string());
            }
        }

        // Check for IDE configurations
        let ide_configs = [
            (".vscode", "vscode"),
            (".vs", "visual_studio"),
            (".idea", "clion"),
        ];

        for (dir, ide) in &ide_configs {
            if delegate.exists(&project_root.join(dir), Some(true)).await {
                metadata
                    .additional_info
                    .insert(format!("has_{}_config", ide), "true".to_string());
            }
        }

        Ok(())
    }

    // Helper functions for parsing various file formats

    fn extract_cmake_project(&self, content: &str) -> Option<(String, Option<String>)> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("project(") {
                // Extract project name and version from project() command
                if let Some(paren_end) = trimmed.find(')') {
                    let project_content = &trimmed[8..paren_end]; // Skip "project("
                    let parts: Vec<&str> = project_content.split_whitespace().collect();

                    if !parts.is_empty() {
                        let name = parts[0].to_string();

                        // Look for VERSION keyword
                        for i in 1..parts.len() {
                            if parts[i] == "VERSION" && i + 1 < parts.len() {
                                return Some((name, Some(parts[i + 1].to_string())));
                            }
                        }

                        return Some((name, None));
                    }
                }
            }
        }
        None
    }

    fn extract_cmake_minimum_required(&self, content: &str) -> Option<String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("cmake_minimum_required(") {
                if let Some(version_start) = trimmed.find("VERSION") {
                    let version_part = &trimmed[version_start + 7..]; // Skip "VERSION "
                    if let Some(paren_pos) = version_part.find(')') {
                        return Some(version_part[..paren_pos].trim().to_string());
                    }
                }
            }
        }
        None
    }

    fn extract_cmake_cxx_standard(&self, content: &str) -> Option<String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.contains("CMAKE_CXX_STANDARD")
                && (trimmed.contains("set(") || trimmed.contains("="))
            {
                // Extract C++ standard version from set(CMAKE_CXX_STANDARD 20) or similar
                if let Some(value_start) = trimmed.find("CMAKE_CXX_STANDARD") {
                    let rest = &trimmed[value_start + "CMAKE_CXX_STANDARD".len()..];
                    // Look for the number after the variable
                    for part in rest.split_whitespace() {
                        // Remove trailing parentheses and other characters
                        let clean_part = part.trim_end_matches(')').trim_end_matches(',');
                        if clean_part.chars().all(|c| c.is_ascii_digit()) && !clean_part.is_empty()
                        {
                            return Some(format!("C++{}", clean_part));
                        }
                    }
                }
            }
        }
        None
    }

    fn extract_cmake_dependencies(&self, content: &str) -> Vec<String> {
        let mut dependencies = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("find_package(") {
                if let Some(paren_end) = trimmed.find(')') {
                    let package_content = &trimmed[13..paren_end]; // Skip "find_package("
                    let parts: Vec<&str> = package_content.split_whitespace().collect();
                    if !parts.is_empty() {
                        dependencies.push(parts[0].to_string());
                    }
                }
            }
        }

        dependencies
    }

    fn extract_makefile_variable(&self, content: &str, var_name: &str) -> Option<String> {
        let pattern = format!("{} =", var_name);
        let pattern_no_space = format!("{}=", var_name);

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with(&pattern) {
                return Some(trimmed[pattern.len()..].trim().to_string());
            } else if trimmed.starts_with(&pattern_no_space) {
                return Some(trimmed[pattern_no_space.len()..].trim().to_string());
            }
        }
        None
    }

    fn extract_makefile_cxx_standard(&self, content: &str) -> Option<String> {
        // Look for -std=c++XX in CXXFLAGS
        if let Some(cxxflags) = self.extract_makefile_variable(content, "CXXFLAGS") {
            if let Some(std_start) = cxxflags.find("-std=c++") {
                let std_part = &cxxflags[std_start + 8..]; // Skip "-std=c++"
                if let Some(end) = std_part.find(' ') {
                    return Some(format!("C++{}", &std_part[..end]));
                } else {
                    // Take only the numeric part
                    let numeric_part: String = std_part
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if !numeric_part.is_empty() {
                        return Some(format!("C++{}", numeric_part));
                    }
                }
            }
        }
        None
    }

    fn extract_meson_project(&self, content: &str) -> Option<(String, Option<String>)> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("project(") {
                // Basic Meson project parsing
                if let Some(quote_start) = trimmed.find('\'') {
                    if let Some(quote_end) = trimmed[quote_start + 1..].find('\'') {
                        let name = &trimmed[quote_start + 1..quote_start + 1 + quote_end];
                        return Some((name.to_string(), None)); // Version parsing would be more complex
                    }
                }
            }
        }
        None
    }

    fn extract_meson_dependencies(&self, content: &str) -> Vec<String> {
        let mut dependencies = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.contains("dependency(") {
                // Extract dependency name from dependency('name')
                if let Some(quote_start) = trimmed.find('\'') {
                    if let Some(quote_end) = trimmed[quote_start + 1..].find('\'') {
                        let dep_name = &trimmed[quote_start + 1..quote_start + 1 + quote_end];
                        dependencies.push(dep_name.to_string());
                    }
                }
            }
        }

        dependencies
    }

    fn extract_conanfile_txt_dependencies(&self, content: &str) -> Vec<String> {
        let mut dependencies = Vec::new();
        let mut in_requires_section = false;

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed == "[requires]" {
                in_requires_section = true;
            } else if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_requires_section = false;
            } else if in_requires_section && !trimmed.is_empty() && !trimmed.starts_with('#') {
                // Extract package name (before /)
                if let Some(slash_pos) = trimmed.find('/') {
                    dependencies.push(trimmed[..slash_pos].to_string());
                } else {
                    dependencies.push(trimmed.to_string());
                }
            }
        }

        dependencies
    }

    fn extract_conanfile_py_dependencies(&self, content: &str) -> Vec<String> {
        let mut dependencies = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.contains("self.requires(") {
                // Extract dependency from self.requires("package/version")
                if let Some(quote_start) = trimmed.find('"') {
                    if let Some(quote_end) = trimmed[quote_start + 1..].find('"') {
                        let dep_spec = &trimmed[quote_start + 1..quote_start + 1 + quote_end];
                        if let Some(slash_pos) = dep_spec.find('/') {
                            dependencies.push(dep_spec[..slash_pos].to_string());
                        } else {
                            dependencies.push(dep_spec.to_string());
                        }
                    }
                }
            }
        }

        dependencies
    }
}

/// Simplified vcpkg.json structure for parsing
#[derive(serde::Deserialize)]
struct VcpkgManifest {
    name: Option<String>,
    version: Option<String>,
    dependencies: Option<Vec<VcpkgDependency>>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum VcpkgDependency {
    String(String),
    Object(VcpkgDependencyObject),
}

#[derive(serde::Deserialize)]
struct VcpkgDependencyObject {
    name: String,
    version: Option<String>,
}

/// Simplified CMakePresets.json structure
#[derive(serde::Deserialize)]
struct CMakePresets {
    version: Option<u32>,
    #[serde(rename = "configurePresets")]
    configure_presets: Option<Vec<serde_json::Value>>,
    #[serde(rename = "buildPresets")]
    build_presets: Option<Vec<serde_json::Value>>,
}

impl Default for CppManifestProvider {
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
    async fn test_cmake_project() {
        let temp_dir = TempDir::new().unwrap();
        let cmake_file = temp_dir.path().join("CMakeLists.txt");

        let cmake_content = r#"
cmake_minimum_required(VERSION 3.20)

project(TestCppProject VERSION 1.2.3 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Boost REQUIRED COMPONENTS system filesystem)
find_package(OpenSSL REQUIRED)

add_executable(app main.cpp)

target_link_libraries(app 
    PRIVATE 
    Boost::system 
    Boost::filesystem
    OpenSSL::SSL
)
"#;
        tokio::fs::write(&cmake_file, cmake_content).await.unwrap();

        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider
            .validate_manifest(&cmake_file, &*delegate)
            .await
            .unwrap());

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&cmake_file, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "TestCppProject");
        assert_eq!(metadata.version.as_ref().unwrap(), "1.2.3");
        assert_eq!(metadata.language, "cpp");
        assert_eq!(
            metadata.additional_info.get("build_system").unwrap(),
            "cmake"
        );
        assert_eq!(
            metadata
                .additional_info
                .get("cmake_minimum_required")
                .unwrap(),
            "3.20"
        );
        assert_eq!(
            metadata.additional_info.get("cpp_standard").unwrap(),
            "C++20"
        );
        assert!(metadata.dependencies.contains(&"Boost".to_string()));
        assert!(metadata.dependencies.contains(&"OpenSSL".to_string()));
    }

    #[tokio::test]
    async fn test_makefile() {
        let temp_dir = TempDir::new().unwrap();
        let makefile = temp_dir.path().join("Makefile");

        let makefile_content = r#"
CC = gcc
CXX = g++
CXXFLAGS = -std=c++17 -Wall -O2
LDFLAGS = -lpthread -lssl

SRCDIR = src
OBJDIR = obj
SOURCES = $(wildcard $(SRCDIR)/*.cpp)
OBJECTS = $(SOURCES:$(SRCDIR)/%.cpp=$(OBJDIR)/%.o)
TARGET = app

.PHONY: all clean install

all: $(TARGET)

$(TARGET): $(OBJECTS)
	$(CXX) $(OBJECTS) -o $@ $(LDFLAGS)

$(OBJDIR)/%.o: $(SRCDIR)/%.cpp
	@mkdir -p $(OBJDIR)
	$(CXX) $(CXXFLAGS) -c $< -o $@

clean:
	rm -rf $(OBJDIR) $(TARGET)

install: $(TARGET)
	cp $(TARGET) /usr/local/bin/
"#;
        tokio::fs::write(&makefile, makefile_content).await.unwrap();

        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider
            .validate_manifest(&makefile, &*delegate)
            .await
            .unwrap());

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&makefile, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "cpp");
        assert_eq!(
            metadata.additional_info.get("build_system").unwrap(),
            "make"
        );
        assert_eq!(metadata.additional_info.get("compiler").unwrap(), "g++");
        assert_eq!(
            metadata.additional_info.get("cpp_standard").unwrap(),
            "C++17"
        );
    }

    #[tokio::test]
    async fn test_vcpkg_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let vcpkg_json = temp_dir.path().join("vcpkg.json");

        let vcpkg_content = r#"
{
  "name": "test-cpp-app",
  "version": "1.0.0",
  "dependencies": [
    "boost-system",
    "boost-filesystem",
    {
      "name": "openssl",
      "version": "1.1.1"
    }
  ]
}
"#;
        tokio::fs::write(&vcpkg_json, vcpkg_content).await.unwrap();

        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider
            .validate_manifest(&vcpkg_json, &*delegate)
            .await
            .unwrap());

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&vcpkg_json, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "test-cpp-app");
        assert_eq!(metadata.version.as_ref().unwrap(), "1.0.0");
        assert_eq!(metadata.language, "cpp");
        assert_eq!(
            metadata.additional_info.get("package_manager").unwrap(),
            "vcpkg"
        );
        assert!(metadata.dependencies.contains(&"boost-system".to_string()));
        assert!(metadata
            .dependencies
            .contains(&"boost-filesystem".to_string()));
        assert!(metadata.dependencies.contains(&"openssl".to_string()));
    }

    #[tokio::test]
    async fn test_conanfile_txt() {
        let temp_dir = TempDir::new().unwrap();
        let conanfile = temp_dir.path().join("conanfile.txt");

        let conan_content = r#"
[requires]
boost/1.82.0
openssl/1.1.1s
fmt/9.1.0

[generators]
CMakeToolchain
CMakeDeps

[options]
boost:shared=True
openssl:shared=False
"#;
        tokio::fs::write(&conanfile, conan_content).await.unwrap();

        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider
            .validate_manifest(&conanfile, &*delegate)
            .await
            .unwrap());

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&conanfile, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "cpp");
        assert_eq!(
            metadata.additional_info.get("package_manager").unwrap(),
            "conan"
        );
        assert!(metadata.dependencies.contains(&"boost".to_string()));
        assert!(metadata.dependencies.contains(&"openssl".to_string()));
        assert!(metadata.dependencies.contains(&"fmt".to_string()));
    }

    #[tokio::test]
    async fn test_meson_build() {
        let temp_dir = TempDir::new().unwrap();
        let meson_file = temp_dir.path().join("meson.build");

        let meson_content = r#"
project('cpp-app', 'cpp',
  version : '0.1',
  default_options : ['warning_level=3',
                     'cpp_std=c++17'])

boost_dep = dependency('boost', modules : ['system', 'filesystem'])
openssl_dep = dependency('openssl')

executable('app',
           'src/main.cpp',
           dependencies : [boost_dep, openssl_dep],
           install : true)

subdir('tests')
"#;
        tokio::fs::write(&meson_file, meson_content).await.unwrap();

        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider
            .validate_manifest(&meson_file, &*delegate)
            .await
            .unwrap());

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&meson_file, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "cpp-app");
        assert_eq!(metadata.language, "cpp");
        assert_eq!(
            metadata.additional_info.get("build_system").unwrap(),
            "meson"
        );
        assert!(metadata.dependencies.contains(&"boost".to_string()));
        assert!(metadata.dependencies.contains(&"openssl".to_string()));
    }

    #[tokio::test]
    async fn test_cmake_presets() {
        let temp_dir = TempDir::new().unwrap();
        let presets_file = temp_dir.path().join("CMakePresets.json");

        let presets_content = r#"
{
  "version": 3,
  "configurePresets": [
    {
      "name": "default",
      "displayName": "Default Config",
      "description": "Default build using Ninja generator",
      "generator": "Ninja",
      "binaryDir": "${sourceDir}/out/build/${presetName}",
      "cacheVariables": {
        "CMAKE_BUILD_TYPE": "Debug"
      }
    }
  ],
  "buildPresets": [
    {
      "name": "default",
      "configurePreset": "default"
    }
  ]
}
"#;
        tokio::fs::write(&presets_file, presets_content)
            .await
            .unwrap();

        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider
            .validate_manifest(&presets_file, &*delegate)
            .await
            .unwrap());

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&presets_file, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "cpp");
        assert_eq!(
            metadata.additional_info.get("build_system").unwrap(),
            "cmake"
        );
        assert_eq!(
            metadata
                .additional_info
                .get("cmake_presets_version")
                .unwrap(),
            "3"
        );
        assert_eq!(
            metadata.additional_info.get("has_cmake_presets").unwrap(),
            "true"
        );
    }

    #[tokio::test]
    async fn test_cpp_project_indicators() {
        let temp_dir = TempDir::new().unwrap();

        // Create C++ project structure
        let src_dir = temp_dir.path().join("src");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::write(
            src_dir.join("main.cpp"),
            "#include <iostream>\nint main() { return 0; }",
        )
        .await
        .unwrap();

        let include_dir = temp_dir.path().join("include");
        tokio::fs::create_dir_all(&include_dir).await.unwrap();

        let nested_file = temp_dir.path().join("tests").join("test_main.cpp");
        tokio::fs::create_dir_all(nested_file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&nested_file, "#include <gtest/gtest.h>")
            .await
            .unwrap();

        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&nested_file, 10, delegate);

        // Should detect as C++ project based on indicators
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_clangd_config() {
        let temp_dir = TempDir::new().unwrap();
        let clangd_file = temp_dir.path().join(".clangd");

        let clangd_content = r#"
CompileFlags:
  Add: [-std=c++20, -Wall, -Wextra]
  Remove: [-W#warnings]
  Compiler: clang++

Index:
  Background: Build

Diagnostics:
  ClangTidy:
    Add: [readability-*, performance-*]
    Remove: [readability-braces-around-statements]
"#;
        tokio::fs::write(&clangd_file, clangd_content)
            .await
            .unwrap();

        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(provider
            .validate_manifest(&clangd_file, &*delegate)
            .await
            .unwrap());

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&clangd_file, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.language, "cpp");
        assert_eq!(
            metadata.additional_info.get("has_clangd_config").unwrap(),
            "true"
        );
    }

    #[tokio::test]
    async fn test_cmake_parsing() {
        let provider = CppManifestProvider::new();

        let cmake_content = r#"
cmake_minimum_required(VERSION 3.16)
project(MyApp VERSION 2.1.0 LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 17)
find_package(Boost REQUIRED)
find_package(OpenSSL 1.1 REQUIRED)
"#;

        let (name, version) = provider.extract_cmake_project(cmake_content).unwrap();
        assert_eq!(name, "MyApp");
        assert_eq!(version.unwrap(), "2.1.0");

        let min_version = provider
            .extract_cmake_minimum_required(cmake_content)
            .unwrap();
        assert_eq!(min_version, "3.16");

        let deps = provider.extract_cmake_dependencies(cmake_content);
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"Boost".to_string()));
        assert!(deps.contains(&"OpenSSL".to_string()));
    }

    #[tokio::test]
    async fn test_invalid_manifests() {
        let temp_dir = TempDir::new().unwrap();
        let provider = CppManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Invalid CMakeLists.txt (no CMake content)
        let invalid_cmake = temp_dir.path().join("CMakeLists.txt");
        tokio::fs::write(&invalid_cmake, "# Just a comment\nsome random content")
            .await
            .unwrap();

        let result = provider.validate_manifest(&invalid_cmake, &*delegate).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());

        // Invalid vcpkg.json
        let invalid_vcpkg = temp_dir.path().join("vcpkg.json");
        tokio::fs::write(&invalid_vcpkg, "{ invalid json")
            .await
            .unwrap();

        let result = provider.validate_manifest(&invalid_vcpkg, &*delegate).await;
        assert!(result.is_err());
    }
}
