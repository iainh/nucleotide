// ABOUTME: Tab bar component that displays all open buffers as tabs
// ABOUTME: Manages tab layout and provides callbacks for tab interactions

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window,
};
use helix_view::DocumentId;
use std::path::PathBuf;
use std::sync::Arc;

use crate::tab::Tab;
use nucleotide_ui::VcsStatus;

/// Type alias for tab event handlers
type TabEventHandler = Arc<dyn Fn(DocumentId, &mut Window, &mut App) + 'static>;

/// Information about a document for tab display
#[derive(Clone)]
pub struct DocumentInfo {
    pub id: DocumentId,
    pub path: Option<PathBuf>,
    pub is_modified: bool,
    pub focused_at: std::time::Instant,
    pub git_status: Option<VcsStatus>,
}

/// Tab bar that displays all open documents
#[derive(IntoElement)]
pub struct TabBar {
    /// Document information for all open documents
    documents: Vec<DocumentInfo>,
    /// Currently active document ID
    active_doc_id: Option<DocumentId>,
    /// Project directory for relative paths
    project_directory: Option<PathBuf>,
    /// Callback when a tab is clicked
    on_tab_click: TabEventHandler,
    /// Callback when a tab close button is clicked
    on_tab_close: TabEventHandler,
}

impl TabBar {
    pub fn new(
        documents: Vec<DocumentInfo>,
        active_doc_id: Option<DocumentId>,
        project_directory: Option<PathBuf>,
        on_tab_click: impl Fn(DocumentId, &mut Window, &mut App) + 'static,
        on_tab_close: impl Fn(DocumentId, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            documents,
            active_doc_id,
            project_directory,
            on_tab_click: Arc::new(on_tab_click),
            on_tab_close: Arc::new(on_tab_close),
        }
    }

    /// Get a display label for a document
    fn get_document_label(&self, doc_info: &DocumentInfo) -> String {
        if let Some(path) = &doc_info.path {
            // Try to get relative path if project directory is set
            if let Some(ref project_dir) = self.project_directory {
                if let Ok(relative) = path.strip_prefix(project_dir) {
                    return relative.display().to_string();
                }
            }
            // Otherwise use filename
            path.file_name()
                .and_then(|name| name.to_str())
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|| path.display().to_string())
        } else {
            // Unnamed buffer
            "[scratch]".to_string()
        }
    }
}

impl RenderOnce for TabBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui_theme = cx.global::<nucleotide_ui::Theme>();
        let theme_manager = cx.global::<crate::ThemeManager>();
        let helix_theme = theme_manager.helix_theme();

        // Get statusline background color for the tab bar
        let statusline_style = helix_theme.get("ui.statusline");
        let bg_color = statusline_style
            .bg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.surface_background);

        // Keep documents in a stable order
        // Sort by path (alphabetically) for a consistent order, with unnamed buffers at the end
        let mut documents = self.documents.clone();
        documents.sort_by(|a, b| {
            match (&a.path, &b.path) {
                (Some(path_a), Some(path_b)) => path_a.cmp(path_b),
                (Some(_), None) => std::cmp::Ordering::Less, // Named files come before unnamed
                (None, Some(_)) => std::cmp::Ordering::Greater, // Unnamed buffers go after named
                (None, None) => a.id.cmp(&b.id),             // For unnamed buffers, sort by ID
            }
        });

        // Create tabs for each document
        let mut tabs = Vec::new();
        for doc_info in documents {
            let label = self.get_document_label(&doc_info);
            let is_active = self.active_doc_id == Some(doc_info.id);

            let on_tab_click = self.on_tab_click.clone();
            let on_tab_close = self.on_tab_close.clone();
            let doc_id = doc_info.id;

            let tab = Tab::new(
                doc_id,
                label,
                doc_info.path.clone(),
                doc_info.is_modified,
                doc_info.git_status.clone(),
                is_active,
                move |_event, window, cx| {
                    on_tab_click(doc_id, window, cx);
                },
                move |_event, window, cx| {
                    on_tab_close(doc_id, window, cx);
                },
            );

            tabs.push(tab);
        }

        // Render the tab bar container
        let has_tabs = !tabs.is_empty();
        div()
            .id("tab-bar")
            .flex()
            .flex_row() // Explicitly set horizontal layout
            .items_center() // Vertically center tabs
            .w_full()
            .h(px(32.0)) // Match tab height
            .bg(bg_color)
            // Removed border_b_1() for seamless active tab integration
            .overflow_x_scroll()
            .when(has_tabs, |this| this.children(tabs))
            .when(!has_tabs, |this| {
                // Show placeholder when no tabs
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .px(px(12.0))
                        .text_color(ui_theme.text_muted)
                        .text_size(px(13.0))
                        .child("No open files"),
                )
            })
    }
}
