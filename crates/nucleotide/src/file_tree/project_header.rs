// ABOUTME: Enhanced file tree header with project type and LSP status indicators
// ABOUTME: Displays project information at the top of the file tree panel

use crate::project_indicator::{ProjectInfo, ProjectLspStatusIndicator, ProjectTypeBadge, ProjectBadgeSize};
use crate::project_status_service::project_status_service;
use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use nucleotide_ui::ThemedContext;
use nucleotide_ui::{Button, ButtonSize, ButtonVariant};
use std::path::PathBuf;

/// Enhanced file tree header showing project status and controls
pub struct ProjectHeader {
    project_root: Option<PathBuf>,
    project_info: Entity<ProjectInfo>,
    show_project_type: bool,
    show_lsp_status: bool,
    collapsed: bool,
}

impl ProjectHeader {
    pub fn new(
        project_root: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Get project info from global service
        let project_status = project_status_service(cx);
        let project_info = project_status.project_info(cx);

        // Observe project info changes
        cx.observe(&project_info, |_, _, cx| {
            cx.notify();
        }).detach();

        Self {
            project_root,
            project_info,
            show_project_type: true,
            show_lsp_status: true,
            collapsed: false,
        }
    }

    pub fn with_project_type_visible(mut self, visible: bool) -> Self {
        self.show_project_type = visible;
        self
    }

    pub fn with_lsp_status_visible(mut self, visible: bool) -> Self {
        self.show_lsp_status = visible;
        self
    }

    pub fn update_project_root(&mut self, project_root: Option<PathBuf>) {
        self.project_root = project_root;
    }

    fn get_project_name(&self) -> String {
        if let Some(ref root) = self.project_root {
            root.file_name()
                .and_then(|name| name.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Project".to_string())
        } else {
            "No Project".to_string()
        }
    }

    fn render_header_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let project_name = self.get_project_name();

        let header_bg = tokens.colors.surface_elevated;
        let border_color = nucleotide_ui::styling::ColorTheory::subtle_border_color(
            header_bg,
            &tokens,
        );

        div()
            .flex()
            .flex_col()
            .w_full()
            .bg(header_bg)
            .border_b_1()
            .border_color(border_color)
            .p(tokens.sizes.space_3)
            .gap(tokens.sizes.space_2)
            .child(
                // Top row: Project name and collapse toggle
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens.sizes.space_2)
                            .child(
                                div()
                                    .text_size(tokens.sizes.text_base)
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(tokens.colors.text_primary)
                                    .child(project_name)
                            )
                            .when(self.show_project_type, |div| {
                                div.child(
                                    ProjectTypeBadge::new(
                                        self.project_info.clone(),
                                        false, // No label in header
                                        ProjectBadgeSize::Small,
                                        cx,
                                    )
                                )
                            })
                    )
                    .child(
                        Button::new("collapse-project-header", if self.collapsed { "▶" } else { "▼" })
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::ExtraSmall)
                            .on_click(cx.listener(|header, _event, _window, cx| {
                                header.collapsed = !header.collapsed;
                                cx.notify();
                            }))
                    )
            )
            .when(!self.collapsed && self.show_lsp_status, |div| {
                // Bottom row: LSP status (when expanded)
                div.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .w_full()
                        .child(
                            div()
                                .text_size(tokens.sizes.text_sm)
                                .text_color(tokens.colors.text_secondary)
                                .child("Language Servers")
                        )
                        .child(
                            ProjectLspStatusIndicator::new(
                                self.project_info.clone(),
                                true, // Show details in header
                                cx,
                            )
                        )
                )
            })
            .when(!self.collapsed && self.project_root.is_none(), |div| {
                // Show project setup prompt when no project is open
                div.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens.sizes.space_2)
                        .child(
                            div()
                                .text_size(tokens.sizes.text_sm)
                                .text_color(tokens.colors.text_muted)
                                .child("No project directory selected")
                        )
                        .child(
                            Button::new("open-project-btn", "Open Project")
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(|_header, _event, _window, cx| {
                                    // Emit event to open project directory picker
                                    cx.emit(ProjectHeaderEvent::OpenProjectRequested);
                                }))
                        )
                )
            })
    }
}

#[derive(Clone, Debug)]
pub enum ProjectHeaderEvent {
    OpenProjectRequested,
    ToggleCollapsed(bool),
}

impl EventEmitter<ProjectHeaderEvent> for ProjectHeader {}

impl Render for ProjectHeader {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_header_content(cx)
    }
}

/// Compact project status indicator for use in other UI areas (like tab bar)
pub struct CompactProjectStatus {
    project_info: Entity<ProjectInfo>,
    show_label: bool,
}

impl CompactProjectStatus {
    pub fn new(
        show_label: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_status = project_status_service(cx);
        let project_info = project_status.project_info(cx);

        // Observe project info changes
        cx.observe(&project_info, |_, _, cx| {
            cx.notify();
        }).detach();

        Self {
            project_info,
            show_label,
        }
    }

    pub fn with_label(mut self, show_label: bool) -> Self {
        self.show_label = show_label;
        self
    }
}

impl EventEmitter<()> for CompactProjectStatus {}

impl Render for CompactProjectStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tokens = &theme.tokens;

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens.sizes.space_2)
            .child(
                ProjectTypeBadge::new(
                    self.project_info.clone(),
                    self.show_label,
                    ProjectBadgeSize::Small,
                    cx,
                )
            )
            .child(
                ProjectLspStatusIndicator::new(
                    self.project_info.clone(),
                    false, // Compact view - no details
                    cx,
                )
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[gpui::test]
    async fn test_project_header_creation(cx: &mut TestAppContext) {
        let temp_dir = TempDir::new().unwrap();
        let project_root = Some(temp_dir.path().to_path_buf());
        
        let header = cx.new_view(|cx| {
            crate::project_status_service::initialize_project_status_service(cx);
            ProjectHeader::new(project_root, cx)
        });

        let header_ref = header.read(cx);
        assert!(header_ref.project_root.is_some());
        assert!(header_ref.show_project_type);
        assert!(header_ref.show_lsp_status);
        assert!(!header_ref.collapsed);
    }

    #[gpui::test]
    async fn test_project_name_extraction(cx: &mut TestAppContext) {
        let temp_dir = TempDir::new().unwrap();
        let project_name = temp_dir.path().file_name().unwrap().to_str().unwrap();
        
        let header = cx.new_view(|cx| {
            crate::project_status_service::initialize_project_status_service(cx);
            ProjectHeader::new(Some(temp_dir.path().to_path_buf()), cx)
        });

        let header_ref = header.read(cx);
        assert_eq!(header_ref.get_project_name(), project_name);
    }

    #[gpui::test]
    async fn test_no_project_name(cx: &mut TestAppContext) {
        let header = cx.new_view(|cx| {
            crate::project_status_service::initialize_project_status_service(cx);
            ProjectHeader::new(None, cx)
        });

        let header_ref = header.read(cx);
        assert_eq!(header_ref.get_project_name(), "No Project");
    }
}