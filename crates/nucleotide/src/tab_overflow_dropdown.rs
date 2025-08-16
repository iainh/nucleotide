// ABOUTME: Dropdown component that displays tabs that don't fit in the tab bar
// ABOUTME: Shows as a button with dropdown menu containing overflow tab items

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, CursorStyle, InteractiveElement, IntoElement, MouseButton, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};
use helix_view::DocumentId;
use std::path::PathBuf;
use std::sync::Arc;

use crate::tab_bar::DocumentInfo;

/// Type alias for tab event handlers
type TabEventHandler = Arc<dyn Fn(DocumentId, &mut Window, &mut App) + 'static>;
/// Type alias for dropdown toggle handlers
type DropdownToggleHandler = Arc<dyn Fn(&mut Window, &mut App) + 'static>;

/// Dropdown button component for overflow tabs - just the trigger button
#[derive(IntoElement)]
pub struct TabOverflowButton {
    /// Number of overflow tabs
    overflow_count: usize,
    /// Callback when dropdown toggle is clicked
    on_dropdown_toggle: DropdownToggleHandler,
    /// Whether the dropdown is currently open
    is_open: bool,
}

/// Dropdown menu that shows overflow tabs
#[derive(IntoElement)]
pub struct TabOverflowMenu {
    /// Documents that don't fit in the tab bar
    overflow_documents: Vec<DocumentInfo>,
    /// Currently active document ID
    active_doc_id: Option<DocumentId>,
    /// Project directory for relative paths
    project_directory: Option<PathBuf>,
    /// Callback when a tab is clicked
    on_tab_click: TabEventHandler,
    /// Callback when dropdown should close
    on_close: DropdownToggleHandler,
}

impl TabOverflowButton {
    pub fn new(
        overflow_count: usize,
        on_dropdown_toggle: impl Fn(&mut Window, &mut App) + 'static,
        is_open: bool,
    ) -> Self {
        Self {
            overflow_count,
            on_dropdown_toggle: Arc::new(on_dropdown_toggle),
            is_open,
        }
    }
}

impl TabOverflowMenu {
    pub fn new(
        overflow_documents: Vec<DocumentInfo>,
        active_doc_id: Option<DocumentId>,
        project_directory: Option<PathBuf>,
        on_tab_click: TabEventHandler,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            overflow_documents,
            active_doc_id,
            project_directory,
            on_tab_click,
            on_close: Arc::new(on_close),
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

impl RenderOnce for TabOverflowButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme_manager = cx.global::<crate::ThemeManager>();
        let helix_theme = theme_manager.helix_theme();
        let ui_theme = cx.global::<nucleotide_ui::Theme>();

        // Get theme colors for visual continuity with editor
        let text_color = helix_theme
            .get("ui.text")
            .fg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.tokens.colors.text_primary);

        let border_color = ui_theme.tokens.colors.border_default;

        // Get statusline background color to match tab bar
        let statusline_style = helix_theme.get("ui.statusline");
        let bg_color = statusline_style
            .bg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.tokens.colors.surface);

        div()
            .id("tab-overflow-button")
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .flex()
            .flex_none()
            .items_center()
            .h(px(32.0))
            .bg(bg_color)
            .child(
                // Dropdown trigger button
                div()
                    .id("tab-overflow-trigger")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .h(px(24.0))
                    .cursor(CursorStyle::PointingHand)
                    .rounded(px(4.0))
                    .bg(ui_theme.tokens.colors.surface)
                    .hover(|style| style.bg(ui_theme.tokens.colors.surface_hover))
                    .border_1()
                    .border_color(border_color)
                    .on_mouse_up(MouseButton::Left, {
                        let on_dropdown_toggle = self.on_dropdown_toggle.clone();
                        move |_event, window, cx| {
                            on_dropdown_toggle(window, cx);
                            cx.stop_propagation();
                        }
                    })
                    .child(
                        div()
                            .text_color(text_color)
                            .text_size(px(12.0))
                            .child(format!("+{}", self.overflow_count)),
                    )
                    .child(
                        div()
                            .text_color(text_color)
                            .text_size(px(12.0))
                            .child(if self.is_open { "▲" } else { "▼" }),
                    ),
            )
    }
}

impl RenderOnce for TabOverflowMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme_manager = cx.global::<crate::ThemeManager>();
        let helix_theme = theme_manager.helix_theme();
        let ui_theme = cx.global::<nucleotide_ui::Theme>();

        // Get theme colors
        let text_color = helix_theme
            .get("ui.text")
            .fg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.tokens.colors.text_primary);

        let dropdown_bg = ui_theme.tokens.colors.surface;
        let border_color = ui_theme.tokens.colors.border_default;

        // Positioned absolutely to appear right below the overflow button
        div()
            .id("tab-overflow-menu")
            .absolute()
            .top(px(32.0)) // Right below the tab bar (32px height)
            .right(px(4.0)) // Align with button position
            .w(px(240.0)) // Slightly smaller width
            .max_h(px(300.0)) // Reasonable max height
            .bg(dropdown_bg)
            .border_1()
            .border_color(border_color)
            .rounded(px(6.0))
            .shadow_lg()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .py(px(2.0)) // Tighter padding
            .children(
                self.overflow_documents
                    .iter()
                    .map(|doc_info| {
                        let label = self.get_document_label(doc_info);
                        let is_active = self.active_doc_id == Some(doc_info.id);
                        let doc_id = doc_info.id;
                        let on_tab_click = self.on_tab_click.clone();

                        div()
                            .id(SharedString::from(format!("overflow-tab-{}", doc_id)))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(12.0))
                            .py(px(6.0)) // Tighter vertical padding
                            .w_full()
                            .min_h(px(28.0)) // Smaller minimum height
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(ui_theme.tokens.colors.surface_hover))
                            .when(is_active, |item_div| {
                                item_div.bg(ui_theme.tokens.colors.surface_active)
                            })
                            .on_mouse_down(MouseButton::Left, {
                                move |_event, _window, cx| {
                                    // Stop propagation immediately to prevent workspace click-away handler
                                    cx.stop_propagation();
                                }
                            })
                            .on_mouse_up(MouseButton::Left, {
                                let on_tab_click = on_tab_click.clone();
                                let on_close = self.on_close.clone();
                                move |_event, window, cx| {
                                    // Stop propagation first to prevent workspace handlers
                                    cx.stop_propagation();
                                    on_tab_click(doc_id, window, cx);
                                    on_close(window, cx);
                                }
                            })
                            .child(
                                // File icon
                                div()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .child(if let Some(ref path) = doc_info.path {
                                        nucleotide_ui::FileIcon::from_path(path, false)
                                            .size(14.0) // Slightly smaller icon
                                            .text_color(text_color)
                                    } else {
                                        nucleotide_ui::FileIcon::scratch()
                                            .size(14.0) // Slightly smaller icon
                                            .text_color(text_color)
                                    }),
                            )
                            .child(
                                // File name
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_color(text_color)
                                    .text_size(px(12.0)) // Slightly smaller text
                                    .line_height(px(16.0)) // Tighter line height
                                    .when(doc_info.is_modified, |name_div| name_div.underline())
                                    .when(is_active, |name_div| {
                                        name_div.font_weight(gpui::FontWeight::MEDIUM)
                                    })
                                    .child(label),
                            )
                            .when(doc_info.is_modified, |modified_div| {
                                modified_div.child(
                                    div()
                                        .flex_none()
                                        .w(px(6.0))
                                        .h(px(6.0))
                                        .rounded(px(3.0))
                                        .bg(ui_theme.tokens.colors.primary)
                                        .ml(px(4.0)),
                                )
                            })
                    })
                    .collect::<Vec<_>>(),
            )
    }
}
