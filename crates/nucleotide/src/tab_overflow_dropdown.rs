// ABOUTME: Dropdown component that displays tabs that don't fit in the tab bar
// ABOUTME: Shows as a button with dropdown menu containing overflow tab items

use gpui::prelude::FluentBuilder;
use gpui::{
    App, CursorStyle, InteractiveElement, IntoElement, MouseButton, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px,
};
use helix_view::DocumentId;
use nucleotide_ui::{
    ListItem, ListItemSpacing, ListItemVariant, ThemedContext as UIThemedContext, VcsIcon,
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
    /// Background color to match the tab bar
    container_bg: Option<gpui::Hsla>,
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
            container_bg: None,
        }
    }

    /// Set the background color to match the tab bar
    pub fn with_background(mut self, background: gpui::Hsla) -> Self {
        self.container_bg = Some(background);
        self
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
}

impl RenderOnce for TabOverflowButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Use TabBarTokens for consistent tab bar theming
        let tab_tokens = tokens.tab_bar_tokens();

        // Use provided background or fall back to tab bar background for consistency
        let container_bg = self.container_bg.unwrap_or(tab_tokens.container_background);

        // Use tab border color for consistency with actual tabs
        let border_color = tab_tokens.tab_border;

        div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .flex()
            .flex_none()
            .items_center()
            .h(tokens.sizes.button_height_md)
            .bg(container_bg)
            .border_b_1()
            .border_color(border_color)
            .child(
                // Custom button styled to match tab bar aesthetic (no borders)
                div()
                    .flex()
                    .items_center()
                    .gap(tokens.sizes.space_1)
                    .px(tokens.sizes.space_3)
                    .py(tokens.sizes.space_2)
                    .h(tokens.sizes.button_height_sm)
                    .cursor(CursorStyle::PointingHand)
                    .rounded(tokens.sizes.radius_md)
                    .bg(if self.is_open {
                        tab_tokens.tab_active_background // Use active tab color when open
                    } else {
                        tab_tokens.tab_inactive_background // Use inactive tab color when closed
                    })
                    .text_color(if self.is_open {
                        tab_tokens.tab_text_active // Active tab text color
                    } else {
                        tab_tokens.tab_text_inactive // Inactive tab text color
                    })
                    .hover(|style| {
                        style
                            .bg(tab_tokens.tab_hover_background)
                            .text_color(tab_tokens.tab_text_inactive)
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

        // Use dropdown component tokens (hybrid system)
        let dd_tokens = tokens.dropdown_tokens();
        let dropdown_bg = dd_tokens.container_background;

        // Positioned so the menu's top-right touches the overflow button's bottom-left
        // Keep this aligned with the button height and reserved width (see tab bar calc ~60px)
        div()
            .absolute()
            .top(tokens.sizes.button_height_md - px(32.0)) // move up by another 10px
            .right(px(14.0)) // move right by another 10px
            .w(px(260.0)) // Fixed width for consistency
            .max_h(px(400.0)) // Maximum height for scrolling
            .bg(dropdown_bg)
            .border_1()
            .border_color(dd_tokens.border)
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

                        // Modern ListItem component with wrapper for click handling (like file tree)
                        {
                            let item_id = SharedString::from(format!("overflow-tab-{}", doc_id));
                            div()
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
                                    ListItem::new(item_id)
                                        .variant(if is_active {
                                            ListItemVariant::Primary // Automatically handles text_on_primary
                                        } else {
                                            ListItemVariant::Ghost // Automatically handles text_primary
                                        })
                                        .spacing(ListItemSpacing::Compact)
                                        .selected(is_active)
                                        .start_slot({
                                            // VCS-aware file icon (matches tab/file-tree styling)
                                            if let Some(ref path) = doc_info.path {
                                                let icon = VcsIcon::from_path(path, false)
                                                    .size(16.0)
                                                    .text_color(theme.tokens.chrome.text_on_chrome)
                                                    .vcs_status(doc_info.git_status);
                                                icon.render_with_theme(theme).into_any_element()
                                            } else {
                                                let icon = VcsIcon::scratch()
                                                    .size(16.0)
                                                    .text_color(theme.tokens.chrome.text_on_chrome)
                                                    .vcs_status(doc_info.git_status);
                                                icon.render_with_theme(theme).into_any_element()
                                            }
                                        })
                                        .end_slot(
                                            // Modified indicator dot
                                            if doc_info.is_modified {
                                                div()
                                                    .w(px(6.0))
                                                    .h(px(6.0))
                                                    .rounded(px(3.0))
                                                    .bg(tokens.chrome.primary)
                                                    .into_any_element()
                                            } else {
                                                div().into_any_element()
                                            },
                                        )
                                        .child(
                                            // File name with automatic styling based on variant
                                            div()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .text_size(tokens.sizes.text_sm)
                                                .line_height(px(18.0))
                                                .when(doc_info.is_modified, |name_div| {
                                                    name_div.italic()
                                                })
                                                .when(is_active, |name_div| {
                                                    name_div.font_weight(gpui::FontWeight::MEDIUM)
                                                })
                                                .child(label),
                                        ),
                                )
                        }
                    })
                    .collect::<Vec<_>>(),
            )
    }
}
