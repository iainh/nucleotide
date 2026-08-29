// ABOUTME: Project type detection and status indicator components
// ABOUTME: Displays project types, LSP server status, and development environment info

use gpui::prelude::FluentBuilder;
use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, div,
};
use nucleotide_ui::ThemedContext;
use std::path::PathBuf;

use crate::ProjectType;

/// UI presentation derived from the canonical project type.
#[derive(Clone, Debug, PartialEq)]
struct ProjectTypePresentation {
    pub display_name: String,
    pub icon: String,
    pub color: Option<gpui::Hsla>,
}

/// Status of project-wide LSP servers
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub project_type: ProjectType,
    pub lsp_status: ProjectLspStatus,
    pub last_updated: std::time::Instant,
}

impl ProjectInfo {
    pub fn new(root_path: Option<PathBuf>) -> Self {
        Self {
            root_path,
            project_type: ProjectType::Unknown,
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

    pub fn set_project_type(&mut self, project_type: ProjectType) {
        self.project_type = project_type;
        self.last_updated = std::time::Instant::now();
    }

    pub fn update_lsp_status(&mut self, lsp_status: ProjectLspStatus) {
        self.lsp_status = lsp_status;
        self.last_updated = std::time::Instant::now();
    }

    fn primary_project_type(&self) -> Option<ProjectTypePresentation> {
        project_type_presentation(&self.project_type)
    }
}

fn project_type_presentation(project_type: &ProjectType) -> Option<ProjectTypePresentation> {
    let (display_name, icon) = match project_type {
        ProjectType::Rust => ("Rust", "R"),
        ProjectType::TypeScript => ("TypeScript", "TS"),
        ProjectType::JavaScript => ("JavaScript", "JS"),
        ProjectType::Python => ("Python", "Py"),
        ProjectType::Go => ("Go", "Go"),
        ProjectType::Java => ("Java", "Java"),
        ProjectType::CSharp => ("C#", "C#"),
        ProjectType::C => ("C", "C"),
        ProjectType::Cpp => ("C++", "C++"),
        ProjectType::Mixed(project_types) => {
            return project_types.iter().find_map(project_type_presentation);
        }
        ProjectType::Other(name) => {
            return Some(ProjectTypePresentation {
                display_name: name.clone(),
                icon: String::new(),
                color: None,
            });
        }
        ProjectType::Unknown => return None,
    };

    Some(ProjectTypePresentation {
        display_name: display_name.to_string(),
        icon: icon.to_string(),
        color: None,
    })
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

            let badge_bg = project_type.color.unwrap_or(tokens.chrome.surface_elevated);
            let text_color = if theme.is_dark() {
                tokens.chrome.text_on_chrome
            } else {
                tokens.chrome.text_chrome_secondary
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
                .border_color(tokens.chrome.border_muted)
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
                tokens.editor.warning
            } else {
                tokens.editor.success
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
                    .text_color(tokens.chrome.text_chrome_secondary)
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
                    .text_color(tokens.editor.error)
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
                    .text_color(tokens.editor.warning)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_type_presentation_maps_canonical_type() {
        let presentation = project_type_presentation(&ProjectType::Rust).unwrap();
        assert_eq!(presentation.display_name, "Rust");
    }

    #[test]
    fn mixed_project_presentation_uses_canonical_priority() {
        let presentation = project_type_presentation(&ProjectType::Mixed(vec![
            ProjectType::Python,
            ProjectType::Rust,
        ]))
        .unwrap();
        assert_eq!(presentation.display_name, "Python");
    }

    #[test]
    fn unknown_project_has_no_presentation() {
        assert!(project_type_presentation(&ProjectType::Unknown).is_none());
    }
}
