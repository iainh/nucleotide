// ABOUTME: Tab bar component that displays all open buffers as tabs
// ABOUTME: Manages tab layout and provides callbacks for tab interactions

use gpui::prelude::FluentBuilder;
use gpui::{App, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px};
use helix_view::DocumentId;
use nucleotide_types::VcsStatus;
use nucleotide_ui::ThemedContext;
use std::path::PathBuf;
use std::sync::Arc;

use crate::tab::Tab;
use crate::tab_overflow_dropdown::TabOverflowButton;

// Keep overflow button width consistent across measurement and rendering
const OVERFLOW_BUTTON_WIDTH: f32 = 60.0;
// Shared label when no tabs are open
const NO_OPEN_FILES: &str = "No open files";

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
            if let Some(ref project_dir) = self.project_directory
                && let Ok(relative) = path.strip_prefix(project_dir)
            {
                return relative.display().to_string();
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

    /// Calculate which tabs fit in available width and which overflow (fallback using estimation)
    fn calculate_overflow(
        &self,
        documents: &[DocumentInfo],
        available_width: f32,
    ) -> (Vec<DocumentInfo>, Vec<DocumentInfo>) {
        self.calculate_overflow_internal(documents, available_width, None)
    }

    /// Calculate which tabs fit in available width and which overflow (with optional context for measurement)
    fn calculate_overflow_internal(
        &self,
        documents: &[DocumentInfo],
        available_width: f32,
        mut cx: Option<&mut App>,
    ) -> (Vec<DocumentInfo>, Vec<DocumentInfo>) {
        // Overflow button width calculation - refined based on actual rendering:
        // The "+X" button should be more accurately sized
        // Width defined at module scope as OVERFLOW_BUTTON_WIDTH

        // No gap between tabs since we removed .gap() from the container
        const TAB_GAP: f32 = 0.0;

        // If there's only one document, no overflow is possible
        if documents.len() <= 1 {
            return (documents.to_vec(), Vec::new());
        }

        let mut visible_tabs = Vec::new();
        let mut overflow_tabs = Vec::new();
        let mut used_width = 0.0;

        // Always reserve space for overflow button when there are multiple documents
        // This prevents the "flickering" effect where tabs switch between visible and overflow
        // Add moderate safety margin to ensure tabs never appear partially behind overflow button
        const SAFETY_MARGIN: f32 = 10.0; // Reduced from 20px to allow more tabs to be visible
        let effective_width = available_width - OVERFLOW_BUTTON_WIDTH - SAFETY_MARGIN;

        // Process all tabs in their natural order (from opening sequence)
        // Don't prioritize active tab - it should stay in its natural position
        for (index, doc_info) in documents.iter().enumerate() {
            let tab_width = if let Some(ref mut context) = cx {
                // Use accurate measurement when context is available
                self.measure_tab_width(doc_info, context)
            } else {
                // Fall back to estimation when no context (for public API compatibility)
                self.estimate_tab_width(doc_info)
            };

            // Calculate required width including gap (gaps go between tabs, not after the last one)
            let width_needed = if index == 0 {
                tab_width // First tab needs no preceding gap
            } else {
                tab_width + TAB_GAP // Subsequent tabs need gap
            };

            if used_width + width_needed <= effective_width {
                visible_tabs.push(doc_info.clone());
                used_width += width_needed;
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

    /// Estimate the width a tab would take up (fallback when no context available)
    fn estimate_tab_width(&self, doc_info: &DocumentInfo) -> f32 {
        const TAB_MIN_WIDTH: f32 = 120.0;
        const TAB_MAX_WIDTH: f32 = 280.0;
        const CHAR_WIDTH: f32 = 9.0; // Approximate character width
        const TAB_PADDING: f32 = 62.0; // Icon + close button + padding + borders - reduced to match measurement

        let label = self.get_document_label(doc_info);
        let text_width = label.len() as f32 * CHAR_WIDTH;
        let estimated_width = text_width + TAB_PADDING;

        // Clamp between min and max width
        estimated_width.clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
    }

    /// Calculate the actual width a tab would take up using text measurement
    fn measure_tab_width(&self, doc_info: &DocumentInfo, cx: &mut App) -> f32 {
        const TAB_MIN_WIDTH: f32 = 120.0;
        const TAB_MAX_WIDTH: f32 = 280.0;

        // Get the label text
        let label = self.get_document_label(doc_info);

        // Get theme and font information
        let theme = cx.theme();
        let tokens = &theme.tokens;
        let font_size = tokens.sizes.text_md;

        // Measure the actual text width using GPUI's text system
        let text_width = self.measure_text_width(&label, font_size, cx);

        // Add padding for icon, close button, and tab padding
        // Icon (16px) + gap (4px) + close button (16px) + padding left/right (24px) + border (2px)
        const TAB_PADDING: f32 = 16.0 + 4.0 + 16.0 + 24.0 + 2.0; // ~62px total - reduced from 70px

        let total_width = text_width + TAB_PADDING;

        // Clamp between min and max width
        total_width.clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
    }

    /// Measure the actual width of text using GPUI's text system
    fn measure_text_width(&self, text: &str, font_size: gpui::Pixels, cx: &mut App) -> f32 {
        // Use system UI font which matches what GPUI uses by default for UI text
        let font = gpui::Font {
            family: ".SystemUIFont".into(), // System UI font family as SharedString
            features: gpui::FontFeatures::default(),
            weight: gpui::FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
            fallbacks: None,
        };

        // Resolve the font
        let font_id = cx.text_system().resolve_font(&font);

        // For simple width measurement, we can estimate using character advances
        // This is faster than full text shaping for our use case
        let mut total_width = 0.0;

        for ch in text.chars() {
            let char_width = cx
                .text_system()
                .advance(font_id, font_size, ch)
                .map(|advance| advance.width.0)
                .unwrap_or(8.0); // fallback to 8px if measurement fails

            total_width += char_width;
        }

        total_width
    }
}

impl RenderOnce for TabBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Keep documents in the order they were opened
        let mut documents = self.documents.clone();
        documents.sort_by(|a, b| a.order.cmp(&b.order));

        // Calculate overflow if available width is specified
        let (visible_tabs, overflow_documents) = if let Some(available_width) = self.available_width
        {
            self.calculate_overflow_internal(&documents, available_width, Some(cx))
        } else {
            // No overflow calculation - show all tabs
            (documents.clone(), Vec::new())
        };

        // Use ThemedContext for consistent theme access after overflow calculation
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get tab bar tokens for hybrid color system
        let tab_bar_tokens = tokens.tab_bar_tokens();
        let tabbar_bg = tab_bar_tokens.container_background;
        let _border_color = tab_bar_tokens.tab_border;

        // For inactive tab areas, use the same container background and border
        let _inactive_tab_bg = tab_bar_tokens.container_background;
        let inactive_border_color = tab_bar_tokens.tab_border;

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
                doc_info.git_status,
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

        // Render the tab bar container using extracted helpers
        let has_tabs = !tabs.is_empty();
        let has_overflow = !overflow_documents.is_empty();

        let tab_row = self.render_tabs_row(
            tabs,
            has_tabs,
            has_overflow,
            tabbar_bg,
            inactive_border_color,
            tokens,
            &tab_bar_tokens,
        );

        let mut root = div()
            .relative()
            .w_full()
            .h(tokens.sizes.button_height_md)
            .child(tab_row);

        if has_overflow {
            root = root.child(self.render_overflow_button(overflow_documents.len(), tabbar_bg));
        }

        root
    }
}

// Helpers to improve function shape and centralize control flow
impl TabBar {
    #[allow(clippy::too_many_arguments)]
    fn render_tabs_row(
        &self,
        tabs: Vec<Tab>,
        has_tabs: bool,
        has_overflow: bool,
        tabbar_bg: gpui::Hsla,
        inactive_border_color: gpui::Hsla,
        tokens: &nucleotide_ui::tokens::DesignTokens,
        tab_bar_tokens: &nucleotide_ui::tokens::TabBarTokens,
    ) -> gpui::AnyElement {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(tokens.sizes.button_height_md)
            .bg(tabbar_bg);

        if has_tabs {
            let tabs_container = div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .overflow_x_hidden()
                .when(has_overflow, |d| d.pr(px(OVERFLOW_BUTTON_WIDTH)))
                .children(tabs);

            row = row.child(tabs_container).child(
                div()
                    .flex_1()
                    .h_full()
                    .bg(tabbar_bg)
                    .border_b_1()
                    .border_color(inactive_border_color),
            );
        } else {
            row = row
                .child(
                    div()
                        .flex()
                        .items_center()
                        .px(tokens.sizes.space_4)
                        .text_color(tab_bar_tokens.tab_text_inactive)
                        .text_size(tokens.sizes.text_sm)
                        .child(NO_OPEN_FILES),
                )
                .border_b_1()
                .border_color(inactive_border_color);
        }

        row.overflow_x_hidden().into_any_element()
    }

    fn render_overflow_button(
        &self,
        overflow_count: usize,
        background: gpui::Hsla,
    ) -> gpui::AnyElement {
        let btn = if let Some(ref on_toggle) = self.on_overflow_toggle {
            let on_toggle = on_toggle.clone();
            TabOverflowButton::new(
                overflow_count,
                move |window, cx| on_toggle(window, cx),
                self.is_overflow_open,
            )
            .with_background(background)
        } else {
            TabOverflowButton::new(overflow_count, |_w, _c| {}, self.is_overflow_open)
                .with_background(background)
        };

        btn.into_any_element()
    }
}
