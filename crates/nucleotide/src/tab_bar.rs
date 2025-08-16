// ABOUTME: Tab bar component that displays all open buffers as tabs
// ABOUTME: Manages tab layout and provides callbacks for tab interactions

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window,
};
use helix_view::DocumentId;
use nucleotide_ui::{compute_component_style, StyleSize, StyleState, StyleVariant, VcsStatus};
use std::path::PathBuf;
use std::sync::Arc;

use crate::tab::Tab;
use crate::tab_overflow_dropdown::TabOverflowButton;

/// Type alias for tab event handlers
type TabEventHandler = Arc<dyn Fn(DocumentId, &mut Window, &mut App) + 'static>;
/// Type alias for dropdown toggle handlers
type DropdownToggleHandler = Arc<dyn Fn(&mut Window, &mut App) + 'static>;

/// Information about a document for tab display
#[derive(Clone)]
pub struct DocumentInfo {
    pub id: DocumentId,
    pub path: Option<PathBuf>,
    pub is_modified: bool,
    pub focused_at: std::time::Instant,
    pub order: usize, // Tracks the order documents were opened
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
    /// Callback when overflow dropdown toggle is clicked
    on_overflow_toggle: Option<DropdownToggleHandler>,
    /// Available width for tabs (None means no limit)
    available_width: Option<f32>,
    /// Whether overflow dropdown is currently open
    is_overflow_open: bool,
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
            on_overflow_toggle: None,
            available_width: None,
            is_overflow_open: false,
        }
    }

    /// Create a new TabBar with available width for overflow calculation
    pub fn with_available_width(mut self, width: f32) -> Self {
        self.available_width = Some(width);
        self
    }

    /// Set the overflow dropdown state
    pub fn with_overflow_open(mut self, is_open: bool) -> Self {
        self.is_overflow_open = is_open;
        self
    }

    /// Set the overflow dropdown toggle callback
    pub fn with_overflow_toggle(
        mut self,
        on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_overflow_toggle = Some(Arc::new(on_toggle));
        self
    }

    /// Get the overflow documents that don't fit in the tab bar
    pub fn get_overflow_documents(&self) -> Vec<DocumentInfo> {
        if let Some(available_width) = self.available_width {
            let documents = self.documents.clone();
            let (_visible_tabs, overflow_documents) =
                self.calculate_overflow(&documents, available_width);
            overflow_documents
        } else {
            Vec::new()
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

    /// Calculate which tabs fit in available width and which overflow
    fn calculate_overflow(
        &self,
        documents: &[DocumentInfo],
        available_width: f32,
    ) -> (Vec<DocumentInfo>, Vec<DocumentInfo>) {
        const OVERFLOW_BUTTON_WIDTH: f32 = 50.0; // More accurate estimate: padding(16) + text(20) + arrow(10) + border(4)
        const TAB_PADDING: f32 = 8.0; // Additional padding between tabs

        // If there's only one document, no overflow is possible
        if documents.len() <= 1 {
            return (documents.to_vec(), Vec::new());
        }

        let mut visible_tabs = Vec::new();
        let mut overflow_tabs = Vec::new();
        let mut used_width = 0.0;

        // Always reserve space for overflow button when there are multiple documents
        // This prevents the "flickering" effect where tabs switch between visible and overflow
        let effective_width = available_width - OVERFLOW_BUTTON_WIDTH;

        // Process all tabs in their natural order (from opening sequence)
        // Don't prioritize active tab - it should stay in its natural position
        for doc_info in documents {
            let tab_width = self.estimate_tab_width(doc_info);
            if used_width + tab_width <= effective_width {
                visible_tabs.push(doc_info.clone());
                used_width += tab_width + TAB_PADDING;
            } else {
                overflow_tabs.push(doc_info.clone());
            }
        }

        // Debug logging to help diagnose overflow issues
        nucleotide_logging::debug!(
            available_width = available_width,
            effective_width = effective_width,
            visible_count = visible_tabs.len(),
            overflow_count = overflow_tabs.len(),
            visible_docs = ?visible_tabs.iter().map(|d| d.path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("[scratch]")).collect::<Vec<_>>(),
            overflow_docs = ?overflow_tabs.iter().map(|d| d.path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("[scratch]")).collect::<Vec<_>>(),
            "Tab overflow calculation completed"
        );

        (visible_tabs, overflow_tabs)
    }

    /// Estimate the width a tab would take up
    fn estimate_tab_width(&self, doc_info: &DocumentInfo) -> f32 {
        const TAB_MIN_WIDTH: f32 = 120.0;
        const TAB_MAX_WIDTH: f32 = 200.0;
        const CHAR_WIDTH: f32 = 8.0; // Approximate character width
        const TAB_PADDING: f32 = 24.0; // Icon + close button + padding

        let label = self.get_document_label(doc_info);
        let text_width = label.len() as f32 * CHAR_WIDTH;
        let estimated_width = text_width + TAB_PADDING;

        // Clamp between min and max width
        estimated_width.clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
    }
}

impl RenderOnce for TabBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Use enhanced styling system with provider support
        let ui_theme =
            nucleotide_ui::providers::use_provider::<nucleotide_ui::providers::ThemeProvider>()
                .map(|provider| provider.current_theme().clone())
                .unwrap_or_else(|| cx.global::<nucleotide_ui::Theme>().clone());

        // Compute style for the tab bar container (secondary variant for surface styling)
        let container_style = compute_component_style(
            &ui_theme,
            StyleState::Default,
            StyleVariant::Secondary.as_str(),
            StyleSize::Medium.as_str(),
        );

        // Get fallback background color from Helix theme for visual continuity
        let theme_manager = cx.global::<crate::ThemeManager>();
        let helix_theme = theme_manager.helix_theme();
        let statusline_style = helix_theme.get("ui.statusline");
        let bg_color = statusline_style
            .bg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(container_style.background);

        // Keep documents in the order they were opened
        let mut documents = self.documents.clone();
        documents.sort_by(|a, b| a.order.cmp(&b.order));

        // Calculate overflow if available width is specified
        let (visible_tabs, overflow_documents) = if let Some(available_width) = self.available_width
        {
            self.calculate_overflow(&documents, available_width)
        } else {
            // No overflow calculation - show all tabs
            (documents.clone(), Vec::new())
        };

        // Create tabs for visible documents
        let mut tabs = Vec::new();
        for doc_info in &visible_tabs {
            let label = self.get_document_label(doc_info);
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
        let has_overflow = !overflow_documents.is_empty();

        // Create a container that allows the dropdown to escape the tab bar bounds
        div()
            .relative() // Important: relative positioning for absolute child
            .w_full()
            .h(px(32.0))
            .child(
                // The actual tab bar
                div()
                    .id("tab-bar")
                    .flex()
                    .flex_row() // Explicitly set horizontal layout
                    .items_center() // Vertically center tabs
                    .w_full()
                    .h(px(32.0)) // Match tab height
                    .bg(bg_color)
                    // Removed border_b_1() for seamless active tab integration
                    .when(!has_overflow, |this| this.overflow_x_scroll()) // Only scroll when no overflow dropdown
                    .when(has_tabs, |this| {
                        this.child(
                            // Container for visible tabs
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .flex_1() // Take remaining space
                                .children(tabs),
                        )
                    })
                    .when(!has_tabs, |this| {
                        // Show placeholder when no tabs using computed styling
                        let placeholder_style = compute_component_style(
                            &ui_theme,
                            StyleState::Disabled,
                            StyleVariant::Ghost.as_str(),
                            StyleSize::Small.as_str(),
                        );
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .px(placeholder_style.padding_x)
                                .text_color(placeholder_style.foreground)
                                .text_size(placeholder_style.font_size)
                                .child("No open files"),
                        )
                    }),
            )
            .when(has_overflow, |this| {
                // Add overflow button as a sibling, not child of tab-bar
                this.child(if let Some(ref on_toggle) = self.on_overflow_toggle {
                    TabOverflowButton::new(
                        overflow_documents.len(),
                        {
                            let on_toggle = on_toggle.clone();
                            move |window, cx| on_toggle(window, cx)
                        },
                        self.is_overflow_open,
                    )
                } else {
                    // Fallback without toggle functionality
                    TabOverflowButton::new(
                        overflow_documents.len(),
                        |_window, _cx| {}, // No-op
                        self.is_overflow_open,
                    )
                })
            })
    }
}
