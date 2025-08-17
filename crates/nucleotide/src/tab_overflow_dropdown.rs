// ABOUTME: Dropdown component that displays tabs that don't fit in the tab bar
// ABOUTME: Shows as a button with dropdown menu containing overflow tab items

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, CursorStyle, InteractiveElement, IntoElement, MouseButton, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};
use helix_view::DocumentId;
use nucleotide_ui::theme_manager::HelixThemedContext;
use nucleotide_ui::{
    compute_contextual_style, ColorContext, StyleSize, StyleState, StyleVariant,
    ThemedContext as UIThemedContext,
};
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
        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Use design tokens for consistent colors
        let container_bg = tokens.colors.surface;
        let button_bg = if self.is_open {
            tokens.colors.surface_selected
        } else {
            tokens.colors.surface_hover
        };

        div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .flex()
            .flex_none()
            .items_center()
            .h(tokens.sizes.button_height_md)
            .bg(container_bg)
            .child(
                // Dropdown trigger button using design tokens
                div()
                    .flex()
                    .items_center()
                    .gap(tokens.sizes.space_1)
                    .px(tokens.sizes.space_3)
                    .py(tokens.sizes.space_2)
                    .h(tokens.sizes.button_height_sm)
                    .cursor(CursorStyle::PointingHand)
                    .rounded(tokens.sizes.radius_md)
                    .bg(button_bg)
                    .text_color(tokens.colors.text_primary)
                    .border_1()
                    .border_color(tokens.colors.border_default)
                    .hover(|style| {
                        style
                            .bg(tokens.colors.surface_selected)
                            .border_color(tokens.colors.border_focus)
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let on_dropdown_toggle = self.on_dropdown_toggle.clone();
                        move |_event, window, cx| {
                            on_dropdown_toggle(window, cx);
                            cx.stop_propagation();
                        }
                    })
                    .child(
                        div()
                            .text_size(tokens.sizes.text_sm)
                            .child(format!("+{}", self.overflow_count)),
                    )
                    .child(
                        div()
                            .text_size(tokens.sizes.text_xs)
                            .child(if self.is_open { "▲" } else { "▼" }),
                    ),
            )
    }
}

impl RenderOnce for TabOverflowMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Use design tokens for consistent colors
        let dropdown_bg = tokens.colors.surface_elevated;
        let text_color = tokens.colors.text_primary;

        // Positioned absolutely to appear right below the overflow button
        div()
            .absolute()
            .top(px(34.0)) // Right below the tab bar with small gap
            .right(px(20.0)) // Margin from right edge to prevent clipping
            .w(px(260.0)) // Fixed width for consistency
            .max_h(px(400.0)) // Maximum height for scrolling
            .bg(dropdown_bg)
            .border_1()
            .border_color(tokens.colors.border_default)
            .rounded(tokens.sizes.radius_lg)
            .shadow_lg()
            .overflow_y_hidden()
            .flex()
            .flex_col()
            .py(tokens.sizes.space_2)
            .children(
                self.overflow_documents
                    .iter()
                    .map(|doc_info| {
                        let label = self.get_document_label(doc_info);
                        let is_active = self.active_doc_id == Some(doc_info.id);
                        let doc_id = doc_info.id;
                        let on_tab_click = self.on_tab_click.clone();

                        {
                            // Use design tokens for active/inactive states
                            let item_bg = if is_active {
                                tokens.colors.surface_selected
                            } else {
                                gpui::transparent_black()
                            };

                            div()
                                .id(SharedString::from(format!("overflow-tab-{}", doc_id)))
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(tokens.sizes.space_3)
                                .px(tokens.sizes.space_4)
                                .py(tokens.sizes.space_2)
                                .mx(tokens.sizes.space_1)
                                .w_full()
                                .min_h(tokens.sizes.button_height_sm)
                                .cursor(CursorStyle::PointingHand)
                                .bg(item_bg)
                                .text_color(text_color)
                                .rounded(tokens.sizes.radius_md)
                                .hover(|style| style.bg(tokens.colors.surface_hover))
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
                                    // File icon using design tokens
                                    div()
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_center()
                                        .child(if let Some(ref path) = doc_info.path {
                                            nucleotide_ui::FileIcon::from_path(path, false)
                                                .size(16.0)
                                                .text_color(text_color)
                                        } else {
                                            nucleotide_ui::FileIcon::scratch()
                                                .size(16.0)
                                                .text_color(text_color)
                                        }),
                                )
                                .child(
                                    // File name using design tokens
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .text_color(text_color)
                                        .text_size(tokens.sizes.text_sm)
                                        .line_height(px(18.0))
                                        .when(doc_info.is_modified, |name_div| name_div.italic())
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
                                            .bg(tokens.colors.primary)
                                            .ml(tokens.sizes.space_1),
                                    )
                                })
                        }
                    })
                    .collect::<Vec<_>>(),
            )
    }
}
