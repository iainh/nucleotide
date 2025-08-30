// ABOUTME: Java project manifest provider for Maven and Gradle projects
// ABOUTME: Handles both Maven (pom.xml) and Gradle (build.gradle, build.gradle.kts) with multi-module support

use async_trait::async_trait;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::manifest::{
    BaseManifestProvider, ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery,
    ProjectMetadata,
};

/// Manifest provider for Java projects
pub struct JavaManifestProvider {
    base: BaseManifestProvider,
}

impl JavaManifestProvider {
    pub fn new() -> Self {
        Self {
            base: BaseManifestProvider::new(
                "pom.xml",
                vec![
                    "pom.xml".to_string(),
                    "build.gradle".to_string(),
                    "build.gradle.kts".to_string(),
                    "gradlew".to_string(),
                    "gradle.properties".to_string(),
                    "settings.gradle".to_string(),
                    "settings.gradle.kts".to_string(),
                ],
            )
            .with_priority(110), // Standard priority for Java projects
        }
    }
}

#[async_trait]
impl ManifestProvider for JavaManifestProvider {
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
        // Priority order for Java project detection
        let priority_patterns = [
            "pom.xml",             // Maven
            "build.gradle",        // Gradle
            "build.gradle.kts",    // Gradle Kotlin DSL
            "settings.gradle",     // Gradle multi-project
            "settings.gradle.kts", // Gradle multi-project Kotlin DSL
        ];

        // For Java, we want to find the outermost build file for multi-module projects
        let mut outermost_project = None;

        for ancestor in query.path.ancestors().take(query.max_depth) {
            // Check for Maven multi-module (parent pom)
            let pom_path = ancestor.join("pom.xml");
            if query.delegate.exists(&pom_path, Some(false)).await
                && self.validate_manifest(&pom_path, &*query.delegate).await?
            {
                outermost_project = Some(ancestor.to_path_buf());

                // Check if this is a parent POM (multi-module)
                if self
                    .is_maven_parent_pom(&pom_path, &*query.delegate)
                    .await?
                {
                    nucleotide_logging::info!(
                        maven_parent = %ancestor.display(),
                        "Found Maven parent POM (multi-module project)"
                    );
                    return Ok(outermost_project);
                }
            }

            // Check for Gradle multi-project (settings.gradle)
            for settings_file in &["settings.gradle", "settings.gradle.kts"] {
                let settings_path = ancestor.join(settings_file);
                if query.delegate.exists(&settings_path, Some(false)).await {
                    nucleotide_logging::info!(
                        gradle_settings = %ancestor.display(),
                        settings_file = settings_file,
                        "Found Gradle multi-project root"
                    );
                    return Ok(Some(ancestor.to_path_buf()));
                }
            }

            // Check for single-module projects
            for pattern in &priority_patterns[..3] {
                // Only build files, not settings
                let manifest_path = ancestor.join(pattern);

                if query.delegate.exists(&manifest_path, Some(false)).await {
                    nucleotide_logging::debug!(
                        java_manifest = %manifest_path.display(),
                        pattern = pattern,
                        "Found Java manifest file"
                    );

                    if self
                        .validate_java_manifest(&manifest_path, pattern, &*query.delegate)
                        .await?
                    {
                        outermost_project = Some(ancestor.to_path_buf());
                    }
                }
            }

            // Check for Java project indicators (fallback)
            if self
                .has_java_project_indicators(ancestor, &*query.delegate)
                .await?
            {
                nucleotide_logging::debug!(
                    project_root = %ancestor.display(),
                    "Found Java project root based on indicators"
                );
                outermost_project = Some(ancestor.to_path_buf());
            }
        }

        if let Some(root) = &outermost_project {
            nucleotide_logging::info!(
                project_root = %root.display(),
                "Found Java project root"
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

        self.validate_java_manifest(path, file_name, delegate).await
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
            language: "java".to_string(),
            ..Default::default()
        };

        match file_name {
            "pom.xml" => {
                self.extract_maven_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            "build.gradle" | "build.gradle.kts" => {
                self.extract_gradle_metadata(manifest_path, delegate, &mut metadata)
                    .await?;
            }
            _ => {}
        }

        // Add additional Java project information
        self.add_java_environment_info(
            manifest_path.parent().unwrap_or(Path::new(".")),
            delegate,
            &mut metadata,
        )
        .await?;

        Ok(metadata)
    }
}

impl JavaManifestProvider {
    async fn validate_java_manifest(
        &self,
        path: &Path,
        file_name: &str,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        match file_name {
            "pom.xml" => self.validate_maven_pom(path, delegate).await,
            "build.gradle" | "build.gradle.kts" => self.validate_gradle_build(path, delegate).await,
            "settings.gradle" | "settings.gradle.kts" => {
                self.validate_gradle_settings(path, delegate).await
            }
            _ => Ok(false),
        }
    }

    async fn validate_maven_pom(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Basic XML validation - check for Maven-specific elements
        let has_project_element = content.contains("<project");
        let has_maven_namespace = content.contains("http://maven.apache.org/POM/")
            || content.contains("xmlns=\"http://maven.apache.org/POM/");
        let has_group_id = content.contains("<groupId>");
        let has_artifact_id = content.contains("<artifactId>");

        nucleotide_logging::trace!(
            pom_xml = %path.display(),
            has_project_element = has_project_element,
            has_maven_namespace = has_maven_namespace,
            has_group_id = has_group_id,
            has_artifact_id = has_artifact_id,
            "Validated pom.xml structure"
        );

        // Valid if it has project element and either namespace or standard Maven elements
        Ok(has_project_element && (has_maven_namespace || (has_group_id && has_artifact_id)))
    }

    async fn validate_gradle_build(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for Gradle-specific keywords
        let gradle_indicators = [
            "plugins {",
            "apply plugin:",
            "dependencies {",
            "repositories {",
            "gradle.properties",
            "implementation ",
            "testImplementation ",
            "api ",
            "compileOnly ",
            "runtimeOnly ",
            "sourceSets {",
            "java {",
            "kotlin {",
        ];

        let has_gradle_content = gradle_indicators
            .iter()
            .any(|indicator| content.contains(indicator));

        nucleotide_logging::trace!(
            build_gradle = %path.display(),
            has_gradle_content = has_gradle_content,
            "Validated build.gradle structure"
        );

        Ok(has_gradle_content)
    }

    async fn validate_gradle_settings(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for settings.gradle specific content
        let has_include = content.contains("include");
        let has_root_project = content.contains("rootProject.name");
        let has_settings_content = has_include || has_root_project;

        nucleotide_logging::trace!(
            settings_gradle = %path.display(),
            has_include = has_include,
            has_root_project = has_root_project,
            "Validated settings.gradle structure"
        );

        Ok(has_settings_content)
    }

    async fn is_maven_parent_pom(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let content = delegate.read_to_string(path).await?;

        // Check for parent POM indicators
        let has_modules = content.contains("<modules>") && content.contains("<module>");
        let has_packaging_pom = content.contains("<packaging>pom</packaging>");

        Ok(has_modules || has_packaging_pom)
    }

    async fn has_java_project_indicators(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let indicators = [
            "src/main/java",      // Maven/Gradle standard layout
            "src/test/java",      // Maven/Gradle test layout
            "src/main/kotlin",    // Kotlin on JVM
            "src/main/scala",     // Scala on JVM
            "src/main/resources", // Resources directory
            "target",             // Maven build output
            "build",              // Gradle build output
            ".gradle",            // Gradle cache
            "gradlew",            // Gradle wrapper
            "gradlew.bat",        // Gradle wrapper (Windows)
            "mvnw",               // Maven wrapper
            "mvnw.cmd",           // Maven wrapper (Windows)
            ".mvn",               // Maven wrapper directory
        ];

        for indicator in &indicators {
            let indicator_path = path.join(indicator);
            if delegate.exists(&indicator_path, None).await {
                nucleotide_logging::debug!(
                    indicator = indicator,
                    path = %path.display(),
                    "Found Java project indicator"
                );
                return Ok(true);
            }
        }

        // Check for common Java files
        if self.directory_contains_java_files(path, delegate).await? {
            return Ok(true);
        }

        Ok(false)
    }

    async fn directory_contains_java_files(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
    ) -> Result<bool> {
        let java_files = [
            "Main.java",
            "App.java",
            "Application.java",
            "Server.java",
            "Client.java",
            "Service.java",
        ];

        for java_file in &java_files {
            let file_path = path.join(java_file);
            if delegate.exists(&file_path, Some(false)).await {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn extract_maven_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Basic XML parsing using regex (limited but functional)
        if let Some(artifact_id) = self.extract_xml_element(&content, "artifactId") {
            metadata.name = Some(artifact_id);
        }

        if let Some(version) = self.extract_xml_element(&content, "version") {
            metadata.version = Some(version);
        }

        if let Some(description) = self.extract_xml_element(&content, "description") {
            metadata.description = Some(description);
        }

        if let Some(group_id) = self.extract_xml_element(&content, "groupId") {
            metadata
                .additional_info
                .insert("group_id".to_string(), group_id);
        }

        if let Some(packaging) = self.extract_xml_element(&content, "packaging") {
            metadata
                .additional_info
                .insert("packaging".to_string(), packaging);
        }

        metadata
            .additional_info
            .insert("build_tool".to_string(), "maven".to_string());

        // Extract dependencies (basic approach)
        let dependencies = self.extract_maven_dependencies(&content);
        metadata.dependencies = dependencies;

        Ok(())
    }

    async fn extract_gradle_metadata(
        &self,
        path: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        let content = delegate.read_to_string(path).await?;

        // Extract project name from settings.gradle if available
        let project_root = path.parent().unwrap_or(Path::new("."));
        for settings_file in &["settings.gradle", "settings.gradle.kts"] {
            let settings_path = project_root.join(settings_file);
            if delegate.exists(&settings_path, Some(false)).await {
                let settings_content = delegate.read_to_string(&settings_path).await?;
                if let Some(name) = self.extract_gradle_root_project_name(&settings_content) {
                    metadata.name = Some(name);
                    break;
                }
            }
        }

        // Extract version from build.gradle
        if let Some(version) = self.extract_gradle_version(&content) {
            metadata.version = Some(version);
        }

        // Detect build tool variant
        let build_tool = if path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .ends_with(".kts")
        {
            "gradle-kotlin"
        } else {
            "gradle"
        };
        metadata
            .additional_info
            .insert("build_tool".to_string(), build_tool.to_string());

        // Extract basic dependencies (simplified approach)
        let dependencies = self.extract_gradle_dependencies(&content);
        metadata.dependencies = dependencies;

        Ok(())
    }

    async fn add_java_environment_info(
        &self,
        project_root: &Path,
        delegate: &dyn ManifestDelegate,
        metadata: &mut ProjectMetadata,
    ) -> Result<()> {
        // Check for build outputs
        if delegate
            .exists(&project_root.join("target"), Some(true))
            .await
        {
            metadata
                .additional_info
                .insert("has_maven_target".to_string(), "true".to_string());
        }

        if delegate
            .exists(&project_root.join("build"), Some(true))
            .await
        {
            metadata
                .additional_info
                .insert("has_gradle_build".to_string(), "true".to_string());
        }

        // Check for IDE files
        if delegate
            .exists(&project_root.join(".idea"), Some(true))
            .await
        {
            metadata
                .additional_info
                .insert("has_intellij_config".to_string(), "true".to_string());
        }

        if delegate
            .exists(&project_root.join(".eclipse"), Some(true))
            .await
        {
            metadata
                .additional_info
                .insert("has_eclipse_config".to_string(), "true".to_string());
        }

        // Check for wrapper scripts
        if delegate
            .exists(&project_root.join("gradlew"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("has_gradle_wrapper".to_string(), "true".to_string());
        }

        if delegate
            .exists(&project_root.join("mvnw"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("has_maven_wrapper".to_string(), "true".to_string());
        }

        // Check for Java version files
        if delegate
            .exists(&project_root.join(".java-version"), Some(false))
            .await
        {
            metadata
                .additional_info
                .insert("has_java_version_file".to_string(), "true".to_string());
        }

        // Check for common config files
        let config_files = [
            ("application.properties", "spring_boot"),
            ("application.yml", "spring_boot"),
            ("logback.xml", "logback"),
            ("log4j2.xml", "log4j2"),
            ("checkstyle.xml", "checkstyle"),
            ("spotbugs.xml", "spotbugs"),
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

    fn extract_maven_dependencies(&self, content: &str) -> Vec<String> {
        let mut dependencies = Vec::new();

        // Simple regex-like extraction for <artifactId> within <dependencies>
        let lines: Vec<&str> = content.lines().collect();
        let mut in_dependencies = false;
        let mut in_dependency = false;

        for line in lines {
            let trimmed = line.trim();

            if trimmed.contains("<dependencies>") {
                in_dependencies = true;
            } else if trimmed.contains("</dependencies>") {
                in_dependencies = false;
            } else if in_dependencies && trimmed.contains("<dependency>") {
                in_dependency = true;
            } else if in_dependencies && trimmed.contains("</dependency>") {
                in_dependency = false;
            } else if in_dependency && trimmed.contains("<artifactId>") {
                if let Some(artifact_id) = self.extract_xml_element(trimmed, "artifactId") {
                    dependencies.push(artifact_id);
                }
            }
        }

        dependencies
    }

    fn extract_gradle_dependencies(&self, content: &str) -> Vec<String> {
        let mut dependencies = Vec::new();
        let dependency_types = [
            "implementation",
            "api",
            "compileOnly",
            "runtimeOnly",
            "testImplementation",
            "testApi",
            "testCompileOnly",
            "testRuntimeOnly",
        ];

        for line in content.lines() {
            let trimmed = line.trim();

            for dep_type in &dependency_types {
                if trimmed.starts_with(dep_type) {
                    // Extract dependency name (simplified)
                    if let Some(quote_start) = trimmed.find('\'') {
                        if let Some(quote_end) = trimmed[quote_start + 1..].find('\'') {
                            let dep_string = &trimmed[quote_start + 1..quote_start + 1 + quote_end];
                            // Extract group:artifact part
                            if let Some(colon_pos) = dep_string.find(':') {
                                if let Some(second_colon) = dep_string[colon_pos + 1..].find(':') {
                                    let artifact =
                                        &dep_string[colon_pos + 1..colon_pos + 1 + second_colon];
                                    dependencies.push(artifact.to_string());
                                } else {
                                    // Just group:artifact without version
                                    let artifact = &dep_string[colon_pos + 1..];
                                    dependencies.push(artifact.to_string());
                                }
                            }
                        }
                    } else if let Some(quote_start) = trimmed.find('"') {
                        if let Some(quote_end) = trimmed[quote_start + 1..].find('"') {
                            let dep_string = &trimmed[quote_start + 1..quote_start + 1 + quote_end];
                            // Extract group:artifact part
                            if let Some(colon_pos) = dep_string.find(':') {
                                if let Some(second_colon) = dep_string[colon_pos + 1..].find(':') {
                                    let artifact =
                                        &dep_string[colon_pos + 1..colon_pos + 1 + second_colon];
                                    dependencies.push(artifact.to_string());
                                } else {
                                    // Just group:artifact without version
                                    let artifact = &dep_string[colon_pos + 1..];
                                    dependencies.push(artifact.to_string());
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }

        dependencies
    }

    fn extract_gradle_root_project_name(&self, content: &str) -> Option<String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("rootProject.name") {
                // Extract name from: rootProject.name = "project-name"
                if let Some(equals_pos) = trimmed.find('=') {
                    let name_part = trimmed[equals_pos + 1..].trim();
                    let name = name_part.trim_matches('"').trim_matches('\'').trim();
                    return Some(name.to_string());
                }
            }
        }
        None
    }

    fn extract_gradle_version(&self, content: &str) -> Option<String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("version") && trimmed.contains('=') {
                // Extract version from: version = "1.0.0"
                if let Some(equals_pos) = trimmed.find('=') {
                    let version_part = trimmed[equals_pos + 1..].trim();
                    let version = version_part.trim_matches('"').trim_matches('\'').trim();
                    return Some(version.to_string());
                }
            }
        }
        None
    }
}

impl Default for JavaManifestProvider {
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
    async fn test_maven_pom() {
        let temp_dir = TempDir::new().unwrap();
        let pom_xml = temp_dir.path().join("pom.xml");

        let manifest_content = r#"
<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0"
         xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
         xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 http://maven.apache.org/xsd/maven-4.0.0.xsd">
    <modelVersion>4.0.0</modelVersion>
    
    <groupId>com.example</groupId>
    <artifactId>test-java-project</artifactId>
    <version>1.0.0</version>
    <packaging>jar</packaging>
    
    <description>A test Java project</description>
    
    <dependencies>
        <dependency>
            <groupId>org.springframework</groupId>
            <artifactId>spring-core</artifactId>
            <version>5.3.21</version>
        </dependency>
        <dependency>
            <groupId>junit</groupId>
            <artifactId>junit</artifactId>
            <version>4.13.2</version>
            <scope>test</scope>
        </dependency>
    </dependencies>
</project>
"#;
        tokio::fs::write(&pom_xml, manifest_content).await.unwrap();

        let provider = JavaManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&pom_xml, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&pom_xml, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "test-java-project");
        assert_eq!(metadata.version.as_ref().unwrap(), "1.0.0");
        assert_eq!(metadata.language, "java");
        assert_eq!(metadata.additional_info.get("build_tool").unwrap(), "maven");
        assert_eq!(
            metadata.additional_info.get("group_id").unwrap(),
            "com.example"
        );
        assert_eq!(metadata.additional_info.get("packaging").unwrap(), "jar");
        assert!(metadata.dependencies.contains(&"spring-core".to_string()));
        assert!(metadata.dependencies.contains(&"junit".to_string()));
    }

    #[tokio::test]
    async fn test_gradle_build() {
        let temp_dir = TempDir::new().unwrap();
        let build_gradle = temp_dir.path().join("build.gradle");

        let manifest_content = r#"
plugins {
    id 'java'
    id 'org.springframework.boot' version '2.7.0'
}

group = 'com.example'
version = '1.0.0'

repositories {
    mavenCentral()
}

dependencies {
    implementation 'org.springframework.boot:spring-boot-starter:2.7.0'
    implementation 'com.google.guava:guava:31.1-jre'
    testImplementation 'org.springframework.boot:spring-boot-starter-test:2.7.0'
    testImplementation 'junit:junit:4.13.2'
}
"#;
        tokio::fs::write(&build_gradle, manifest_content)
            .await
            .unwrap();

        // Create settings.gradle
        let settings_gradle = temp_dir.path().join("settings.gradle");
        tokio::fs::write(
            &settings_gradle,
            r#"rootProject.name = 'gradle-test-project'"#,
        )
        .await
        .unwrap();

        let provider = JavaManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&build_gradle, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&build_gradle, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.name.as_ref().unwrap(), "gradle-test-project");
        assert_eq!(metadata.version.as_ref().unwrap(), "1.0.0");
        assert_eq!(metadata.language, "java");
        assert_eq!(
            metadata.additional_info.get("build_tool").unwrap(),
            "gradle"
        );
        assert!(
            metadata
                .dependencies
                .contains(&"spring-boot-starter".to_string())
        );
        assert!(metadata.dependencies.contains(&"guava".to_string()));
    }

    #[tokio::test]
    async fn test_gradle_kotlin_dsl() {
        let temp_dir = TempDir::new().unwrap();
        let build_gradle_kts = temp_dir.path().join("build.gradle.kts");

        let manifest_content = r#"
plugins {
    kotlin("jvm") version "1.7.10"
    java
}

version = "2.0.0"

repositories {
    mavenCentral()
}

dependencies {
    implementation("org.jetbrains.kotlin:kotlin-stdlib:1.7.10")
    testImplementation("org.junit.jupiter:junit-jupiter:5.8.2")
}
"#;
        tokio::fs::write(&build_gradle_kts, manifest_content)
            .await
            .unwrap();

        let provider = JavaManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Test validation
        assert!(
            provider
                .validate_manifest(&build_gradle_kts, &*delegate)
                .await
                .unwrap()
        );

        // Test metadata extraction
        let metadata = provider
            .get_project_metadata(&build_gradle_kts, &*delegate)
            .await
            .unwrap();
        assert_eq!(metadata.version.as_ref().unwrap(), "2.0.0");
        assert_eq!(metadata.language, "java");
        assert_eq!(
            metadata.additional_info.get("build_tool").unwrap(),
            "gradle-kotlin"
        );
    }

    #[tokio::test]
    async fn test_maven_multi_module() {
        let temp_dir = TempDir::new().unwrap();
        let parent_pom = temp_dir.path().join("pom.xml");

        let parent_content = r#"
<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    
    <groupId>com.example</groupId>
    <artifactId>parent-project</artifactId>
    <version>1.0.0</version>
    <packaging>pom</packaging>
    
    <modules>
        <module>module1</module>
        <module>module2</module>
    </modules>
</project>
"#;
        tokio::fs::write(&parent_pom, parent_content).await.unwrap();

        // Create child module
        let module1_dir = temp_dir.path().join("module1");
        tokio::fs::create_dir_all(&module1_dir).await.unwrap();
        let module1_pom = module1_dir.join("pom.xml");
        let module1_content = r#"
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    
    <parent>
        <groupId>com.example</groupId>
        <artifactId>parent-project</artifactId>
        <version>1.0.0</version>
    </parent>
    
    <artifactId>module1</artifactId>
</project>
"#;
        tokio::fs::write(&module1_pom, module1_content)
            .await
            .unwrap();

        // Create a file deep in the module
        let src_dir = module1_dir.join("src").join("main").join("java");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        let java_file = src_dir.join("App.java");
        tokio::fs::write(&java_file, "public class App {}")
            .await
            .unwrap();

        let provider = JavaManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&java_file, 10, delegate);

        // Should find the parent project root
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_java_project_indicators() {
        let temp_dir = TempDir::new().unwrap();

        // Create Java project structure
        let src_dir = temp_dir.path().join("src").join("main").join("java");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::write(src_dir.join("Main.java"), "public class Main {}")
            .await
            .unwrap();

        let test_dir = temp_dir.path().join("src").join("test").join("java");
        tokio::fs::create_dir_all(&test_dir).await.unwrap();

        let nested_file = test_dir.join("TestMain.java");
        tokio::fs::write(&nested_file, "public class TestMain {}")
            .await
            .unwrap();

        let provider = JavaManifestProvider::new();
        let delegate = Arc::new(FsDelegate);
        let query = ManifestQuery::new(&nested_file, 10, delegate);

        // Should detect as Java project based on indicators
        let result = provider.search(query).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), temp_dir.path());
    }

    #[tokio::test]
    async fn test_xml_element_extraction() {
        let provider = JavaManifestProvider::new();

        let xml_content = r#"
        <groupId>com.example</groupId>
        <artifactId>test-project</artifactId>
        <version>1.0.0</version>
        "#;

        assert_eq!(
            provider.extract_xml_element(xml_content, "groupId"),
            Some("com.example".to_string())
        );
        assert_eq!(
            provider.extract_xml_element(xml_content, "artifactId"),
            Some("test-project".to_string())
        );
        assert_eq!(
            provider.extract_xml_element(xml_content, "version"),
            Some("1.0.0".to_string())
        );
        assert_eq!(
            provider.extract_xml_element(xml_content, "nonexistent"),
            None
        );
    }

    #[tokio::test]
    async fn test_gradle_name_extraction() {
        let provider = JavaManifestProvider::new();

        let settings_content = r#"
        rootProject.name = 'my-gradle-project'
        include 'module1', 'module2'
        "#;

        assert_eq!(
            provider.extract_gradle_root_project_name(settings_content),
            Some("my-gradle-project".to_string())
        );

        let settings_content2 = r#"
        rootProject.name = "another-project"
        "#;

        assert_eq!(
            provider.extract_gradle_root_project_name(settings_content2),
            Some("another-project".to_string())
        );
    }

    #[tokio::test]
    async fn test_invalid_manifests() {
        let temp_dir = TempDir::new().unwrap();
        let provider = JavaManifestProvider::new();
        let delegate = Arc::new(FsDelegate);

        // Invalid pom.xml (no project element)
        let invalid_pom = temp_dir.path().join("invalid_pom.xml");
        tokio::fs::write(&invalid_pom, "<root><element>content</element></root>")
            .await
            .unwrap();

        let result = provider.validate_manifest(&invalid_pom, &*delegate).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());

        // Invalid build.gradle (no Gradle content)
        let invalid_gradle = temp_dir.path().join("invalid_build.gradle");
        tokio::fs::write(&invalid_gradle, "# Just a comment\nsome random content")
            .await
            .unwrap();

        let result = provider
            .validate_manifest(&invalid_gradle, &*delegate)
            .await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }
}
