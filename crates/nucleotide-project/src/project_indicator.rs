// ABOUTME: Project type detection and status indicator components
// ABOUTME: Displays project types, LSP server status, and development environment info

use gpui::prelude::FluentBuilder;
use gpui::{
    div, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Window,
};
use nucleotide_ui::ThemedContext;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Information about detected project types
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectType {
    pub name: String,
    pub display_name: String,
    pub icon: String,
    pub color: Option<gpui::Hsla>,
    pub confidence: f32, // 0.0-1.0, higher means more confident
}

/// Status of project-wide LSP servers
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectLspStatus {
    pub total_servers: usize,
    pub running_servers: usize,
    pub failed_servers: usize,
    pub initializing_servers: usize,
    pub has_diagnostics: bool,
    pub diagnostic_count: usize,
}

/// Project information state that can be observed by UI
#[derive(Clone)]
pub struct ProjectInfo {
    pub root_path: Option<PathBuf>,
    pub detected_types: Vec<ProjectType>,
    pub lsp_status: ProjectLspStatus,
    pub last_updated: std::time::Instant,
}

impl ProjectInfo {
    pub fn new(root_path: Option<PathBuf>) -> Self {
        Self {
            root_path,
            detected_types: Vec::new(),
            lsp_status: ProjectLspStatus {
                total_servers: 0,
                running_servers: 0,
                failed_servers: 0,
                initializing_servers: 0,
                has_diagnostics: false,
                diagnostic_count: 0,
            },
            last_updated: std::time::Instant::now(),
        }
    }

    /// Detect project types based on files in the project directory
    pub fn detect_project_types(&mut self) {
        self.detected_types.clear();

        if let Some(ref root) = self.root_path {
            nucleotide_logging::debug!(
                project_root = %root.display(),
                "Starting project type detection in directory"
            );
            self.detected_types = detect_project_types_for_path(root);
            nucleotide_logging::info!(
                project_root = %root.display(),
                detected_count = self.detected_types.len(),
                detected_types = ?self.detected_types.iter().map(|t| &t.name).collect::<Vec<_>>(),
                "Project type detection completed"
            );
        } else {
            nucleotide_logging::warn!("No project root set for project type detection");
        }

        self.last_updated = std::time::Instant::now();
    }

    /// Update LSP status from LSP state
    pub fn update_lsp_status(&mut self, lsp_state: &nucleotide_lsp::LspState) {
        let running = lsp_state
            .servers
            .values()
            .filter(|s| s.status == nucleotide_lsp::ServerStatus::Running)
            .count();
        let failed = lsp_state
            .servers
            .values()
            .filter(|s| matches!(s.status, nucleotide_lsp::ServerStatus::Failed(_)))
            .count();
        let initializing = lsp_state
            .servers
            .values()
            .filter(|s| {
                matches!(
                    s.status,
                    nucleotide_lsp::ServerStatus::Initializing
                        | nucleotide_lsp::ServerStatus::Starting
                )
            })
            .count();

        let diagnostic_count: usize = lsp_state
            .diagnostics
            .values()
            .map(|diags| diags.len())
            .sum();

        self.lsp_status = ProjectLspStatus {
            total_servers: lsp_state.servers.len(),
            running_servers: running,
            failed_servers: failed,
            initializing_servers: initializing,
            has_diagnostics: diagnostic_count > 0,
            diagnostic_count,
        };

        self.last_updated = std::time::Instant::now();
    }

    /// Get the primary project type (highest confidence)
    pub fn primary_project_type(&self) -> Option<&ProjectType> {
        self.detected_types.iter().max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}

impl EventEmitter<()> for ProjectInfo {}

/// Project type badge component for display in file tree or header
pub struct ProjectTypeBadge {
    project_info: Entity<ProjectInfo>,
    show_label: bool,
    size: ProjectBadgeSize,
}

#[derive(Clone, Copy, Debug)]
pub enum ProjectBadgeSize {
    Small,
    Medium,
    Large,
}

impl ProjectTypeBadge {
    pub fn new(
        project_info: Entity<ProjectInfo>,
        show_label: bool,
        size: ProjectBadgeSize,
        cx: &mut Context<Self>,
    ) -> Self {
        // Observe project info changes
        cx.observe(&project_info, |_, _, cx| {
            cx.notify();
        })
        .detach();

        Self {
            project_info,
            show_label,
            size,
        }
    }

    pub fn with_label(mut self, show_label: bool) -> Self {
        self.show_label = show_label;
        self
    }

    pub fn with_size(mut self, size: ProjectBadgeSize) -> Self {
        self.size = size;
        self
    }
}

impl EventEmitter<()> for ProjectTypeBadge {}

impl Render for ProjectTypeBadge {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project_info = self.project_info.read(cx);
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get primary project type
        let primary_type = project_info.primary_project_type();

        if let Some(project_type) = primary_type {
            let (icon_size, text_size, padding) = match self.size {
                ProjectBadgeSize::Small => (
                    tokens.sizes.text_xs,
                    tokens.sizes.text_xs,
                    tokens.sizes.space_1,
                ),
                ProjectBadgeSize::Medium => (
                    tokens.sizes.text_sm,
                    tokens.sizes.text_sm,
                    tokens.sizes.space_2,
                ),
                ProjectBadgeSize::Large => (
                    tokens.sizes.text_md,
                    tokens.sizes.text_md,
                    tokens.sizes.space_3,
                ),
            };

            let badge_bg = project_type.color.unwrap_or(tokens.colors.surface_elevated);
            let text_color = if theme.is_dark() {
                tokens.colors.text_primary
            } else {
                tokens.colors.text_secondary
            };

            let mut badge = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens.sizes.space_1)
                .px(padding)
                .py(tokens.sizes.space_1)
                .bg(badge_bg)
                .border_1()
                .border_color(tokens.colors.border_muted)
                .rounded(tokens.sizes.radius_sm)
                .text_color(text_color);

            // Add icon if available
            if !project_type.icon.is_empty() {
                badge = badge.child(
                    div()
                        .w(icon_size)
                        .h(icon_size)
                        .child(project_type.icon.clone()), // This would need proper icon rendering
                );
            }

            // Add label if requested
            if self.show_label {
                badge = badge.child(
                    div()
                        .text_size(text_size)
                        .child(project_type.display_name.clone()),
                );
            }

            badge.into_any_element()
        } else {
            // No project type detected
            div().size_0().into_any_element()
        }
    }
}

/// Enhanced LSP status indicator for project-wide servers
pub struct ProjectLspStatusIndicator {
    project_info: Entity<ProjectInfo>,
    show_details: bool,
}

impl ProjectLspStatusIndicator {
    pub fn new(
        project_info: Entity<ProjectInfo>,
        show_details: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        // Observe project info changes
        cx.observe(&project_info, |_, _, cx| {
            cx.notify();
        })
        .detach();

        Self {
            project_info,
            show_details,
        }
    }

    pub fn with_details(mut self, show_details: bool) -> Self {
        self.show_details = show_details;
        self
    }
}

impl EventEmitter<()> for ProjectLspStatusIndicator {}

impl Render for ProjectLspStatusIndicator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project_info = self.project_info.read(cx);
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let lsp_status = &project_info.lsp_status;

        if lsp_status.total_servers == 0 {
            return div().size_0().into_any_element();
        }

        let mut status_parts = Vec::new();

        // Server count and status
        if lsp_status.running_servers > 0 {
            let status_color = if lsp_status.failed_servers > 0 {
                tokens.colors.warning
            } else {
                tokens.colors.success
            };

            status_parts.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens.sizes.space_1)
                    .text_color(status_color)
                    .child("●") // Bullet point for status
                    .when(self.show_details, |div| {
                        div.child(format!(
                            "{}/{}",
                            lsp_status.running_servers, lsp_status.total_servers
                        ))
                    }),
            );
        }

        // Initialization status
        if lsp_status.initializing_servers > 0 {
            status_parts.push(
                div()
                    .text_color(tokens.colors.text_secondary)
                    .child("⟳") // Spinning indicator
                    .when(self.show_details, |div| {
                        div.child(format!(" {}", lsp_status.initializing_servers))
                    }),
            );
        }

        // Error status
        if lsp_status.failed_servers > 0 {
            status_parts.push(
                div()
                    .text_color(tokens.colors.error)
                    .child("⚠")
                    .when(self.show_details, |div| {
                        div.child(format!(" {}", lsp_status.failed_servers))
                    }),
            );
        }

        // Diagnostic count
        if lsp_status.has_diagnostics {
            status_parts.push(
                div()
                    .text_color(tokens.colors.warning)
                    .child("▲")
                    .when(self.show_details, |div| {
                        div.child(format!(" {}", lsp_status.diagnostic_count))
                    }),
            );
        }

        if status_parts.is_empty() {
            return div().size_0().into_any_element();
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens.sizes.space_2)
            .text_size(tokens.sizes.text_sm)
            .children(status_parts)
            .into_any_element()
    }
}

/// Detect project types based on files and structure in the given path
pub fn detect_project_types_for_path(path: &Path) -> Vec<ProjectType> {
    nucleotide_logging::debug!(
        path = %path.display(),
        "Starting project type detection in path"
    );

    if !path.exists() {
        nucleotide_logging::warn!(
            path = %path.display(),
            "Project path does not exist"
        );
        return Vec::new();
    }

    if !path.is_dir() {
        nucleotide_logging::warn!(
            path = %path.display(),
            "Project path is not a directory"
        );
        return Vec::new();
    }

    let mut detected_types: Vec<ProjectType> = Vec::new();
    let mut confidence_map: HashMap<String, f32> = HashMap::new();

    // Define project type detection rules
    let detection_rules = [
        // Rust
        (
            "Cargo.toml",
            ProjectType {
                name: "rust".to_string(),
                display_name: "Rust".to_string(),
                icon: "🦀".to_string(),
                color: Some(gpui::hsla(0.0, 0.8, 0.6, 1.0)), // Orange-ish
                confidence: 0.95,
            },
        ),
        // Node.js/JavaScript
        (
            "package.json",
            ProjectType {
                name: "nodejs".to_string(),
                display_name: "Node.js".to_string(),
                icon: "📦".to_string(),
                color: Some(gpui::hsla(0.25, 0.8, 0.6, 1.0)), // Green-ish
                confidence: 0.9,
            },
        ),
        // Python
        (
            "requirements.txt",
            ProjectType {
                name: "python".to_string(),
                display_name: "Python".to_string(),
                icon: "🐍".to_string(),
                color: Some(gpui::hsla(0.6, 0.8, 0.6, 1.0)), // Blue-ish
                confidence: 0.8,
            },
        ),
        (
            "pyproject.toml",
            ProjectType {
                name: "python".to_string(),
                display_name: "Python".to_string(),
                icon: "🐍".to_string(),
                color: Some(gpui::hsla(0.6, 0.8, 0.6, 1.0)),
                confidence: 0.9,
            },
        ),
        // Go
        (
            "go.mod",
            ProjectType {
                name: "go".to_string(),
                display_name: "Go".to_string(),
                icon: "🐹".to_string(),
                color: Some(gpui::hsla(0.5, 0.8, 0.6, 1.0)), // Cyan-ish
                confidence: 0.95,
            },
        ),
        // Java
        (
            "pom.xml",
            ProjectType {
                name: "java".to_string(),
                display_name: "Java (Maven)".to_string(),
                icon: "☕".to_string(),
                color: Some(gpui::hsla(0.1, 0.8, 0.6, 1.0)), // Red-ish
                confidence: 0.9,
            },
        ),
        (
            "build.gradle",
            ProjectType {
                name: "java".to_string(),
                display_name: "Java (Gradle)".to_string(),
                icon: "☕".to_string(),
                color: Some(gpui::hsla(0.1, 0.8, 0.6, 1.0)),
                confidence: 0.9,
            },
        ),
    ];

    // Check for project files
    nucleotide_logging::debug!(
        path = %path.display(),
        rules_count = detection_rules.len(),
        "Checking project detection rules"
    );

    for (file_name, project_type) in &detection_rules {
        let file_path = path.join(file_name);
        nucleotide_logging::debug!(
            file_path = %file_path.display(),
            file_name = file_name,
            project_type = %project_type.name,
            "Checking for project marker file"
        );

        if file_path.exists() {
            nucleotide_logging::info!(
                file_path = %file_path.display(),
                project_type = %project_type.name,
                confidence = project_type.confidence,
                "Found project marker file"
            );

            let current_confidence = confidence_map
                .get(&project_type.name)
                .map(|&c: &f32| c.max(project_type.confidence))
                .unwrap_or(project_type.confidence);
            confidence_map.insert(project_type.name.clone(), current_confidence);

            // Update or add project type with highest confidence
            if let Some(existing) = detected_types
                .iter_mut()
                .find(|t| t.name == project_type.name)
            {
                if project_type.confidence > existing.confidence {
                    nucleotide_logging::debug!(
                        project_type = %project_type.name,
                        old_confidence = existing.confidence,
                        new_confidence = project_type.confidence,
                        "Updating project type with higher confidence"
                    );
                    *existing = project_type.clone();
                }
            } else {
                nucleotide_logging::debug!(
                    project_type = %project_type.name,
                    confidence = project_type.confidence,
                    "Adding new project type"
                );
                detected_types.push(project_type.clone());
            }
        }
    }

    // Sort by confidence (highest first)
    detected_types.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    nucleotide_logging::info!(
        path = %path.display(),
        detected_count = detected_types.len(),
        detected_types = ?detected_types.iter().map(|t| format!("{}({})", t.name, t.confidence)).collect::<Vec<_>>(),
        "Project type detection completed"
    );

    detected_types
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_rust_project_detection() {
        let temp_dir = TempDir::new().unwrap();
        let cargo_toml = temp_dir.path().join("Cargo.toml");
        File::create(&cargo_toml)
            .unwrap()
            .write_all(b"[package]\nname = \"test\"")
            .unwrap();

        let detected = detect_project_types_for_path(temp_dir.path());
        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].name, "rust");
        assert_eq!(detected[0].display_name, "Rust");
    }

    #[test]
    fn test_multiple_project_types() {
        let temp_dir = TempDir::new().unwrap();
        File::create(temp_dir.path().join("package.json")).unwrap();
        File::create(temp_dir.path().join("requirements.txt")).unwrap();

        let detected = detect_project_types_for_path(temp_dir.path());
        assert_eq!(detected.len(), 2);

        // Should be sorted by confidence (Node.js > Python in this case)
        assert_eq!(detected[0].name, "nodejs");
        assert_eq!(detected[1].name, "python");
    }

    #[test]
    fn test_no_project_files() {
        let temp_dir = TempDir::new().unwrap();
        let detected = detect_project_types_for_path(temp_dir.path());
        assert_eq!(detected.len(), 0);
    }
}
