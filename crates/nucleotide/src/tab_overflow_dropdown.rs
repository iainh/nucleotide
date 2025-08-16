// ABOUTME: Dropdown component that displays tabs that don't fit in the tab bar
// ABOUTME: Shows as a button with dropdown menu containing overflow tab items

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, CursorStyle, InteractiveElement, IntoElement, MouseButton, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
};
use helix_view::DocumentId;
use nucleotide_ui::{compute_contextual_style, ColorContext, StyleSize, StyleState, StyleVariant};
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
        // Use enhanced styling system with provider support
        let ui_theme =
            nucleotide_ui::providers::use_provider::<nucleotide_ui::providers::ThemeProvider>()
                .map(|provider| provider.current_theme().clone())
                .unwrap_or_else(|| cx.global::<nucleotide_ui::Theme>().clone());

        // Get fallback colors from Helix theme for visual continuity
        let theme_manager = cx.global::<crate::ThemeManager>();
        let helix_theme = theme_manager.helix_theme();
        let statusline_style = helix_theme.get("ui.statusline");
        let container_bg = statusline_style
            .bg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.tokens.colors.surface);


        // Compute style for the dropdown button with the actual background context
        // Since we're on the statusline/tab bar background, we need to compute foreground accordingly
        let button_style = {
            let mut style = compute_contextual_style(
                &ui_theme,
                StyleState::Default,
                StyleVariant::Ghost.as_str(),
                StyleSize::Small.as_str(),
                ColorContext::OnSurface,
            );
            // Override foreground to work with the actual container background
            style.foreground = nucleotide_ui::ColorTheory::best_text_color(container_bg, &ui_theme.tokens);
            style
        };

        div()
            .id("tab-overflow-button")
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .flex()
            .flex_none()
            .items_center()
            .h(px(32.0))
            .bg(container_bg)
            .child(
                // Dropdown trigger button using computed styles
                div()
                    .id("tab-overflow-trigger")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(button_style.padding_x)
                    .py(button_style.padding_y)
                    .h(px(24.0))
                    .cursor(CursorStyle::PointingHand)
                    .rounded(button_style.border_radius)
                    .bg(button_style.background)
                    .text_color(button_style.foreground)
                    .border_1()
                    .border_color(button_style.border_color)
                    .hover(|style| {
                        // Compute hover style with proper context
                        let mut hover_style = compute_contextual_style(
                            &ui_theme,
                            StyleState::Hover,
                            StyleVariant::Ghost.as_str(),
                            StyleSize::Small.as_str(),
                            ColorContext::OnSurface,
                        );
                        // Override foreground to work with the actual container background
                        hover_style.foreground = nucleotide_ui::ColorTheory::best_text_color(container_bg, &ui_theme.tokens);
                        style.bg(hover_style.background).text_color(hover_style.foreground)
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
                            .text_size(button_style.font_size)
                            .child(format!("+{}", self.overflow_count)),
                    )
                    .child(
                        div()
                            .text_size(button_style.font_size)
                            .child(if self.is_open { "▲" } else { "▼" }),
                    ),
            )
    }
}

impl RenderOnce for TabOverflowMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Use enhanced styling system with provider support
        let ui_theme =
            nucleotide_ui::providers::use_provider::<nucleotide_ui::providers::ThemeProvider>()
                .map(|provider| provider.current_theme().clone())
                .unwrap_or_else(|| cx.global::<nucleotide_ui::Theme>().clone());

        // Get the same background color as the tab area for consistency
        let theme_manager = cx.global::<crate::ThemeManager>();
        let helix_theme = theme_manager.helix_theme();
        let statusline_style = helix_theme.get("ui.statusline");
        let dropdown_bg = statusline_style
            .bg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.tokens.colors.surface);

        // Compute style for the dropdown menu using the same background as tabs
        let mut menu_style = compute_contextual_style(
            &ui_theme,
            StyleState::Default,
            StyleVariant::Secondary.as_str(),
            StyleSize::Medium.as_str(),
            ColorContext::Floating,
        );
        
        // Override background to match the tab area for visual consistency
        menu_style.background = dropdown_bg;

        // Compute style for menu items with the actual dropdown background
        let item_style = {
            let mut style = compute_contextual_style(
                &ui_theme,
                StyleState::Default,
                StyleVariant::Ghost.as_str(),
                StyleSize::Small.as_str(),
                ColorContext::Floating,
            );
            // Override foreground to work with the dropdown background
            style.foreground = nucleotide_ui::ColorTheory::best_text_color(dropdown_bg, &ui_theme.tokens);
            style
        };

        // Use the computed text color for consistency
        let text_color = item_style.foreground;

        // Positioned absolutely to appear right below the overflow button
        div()
            .id("tab-overflow-menu")
            .absolute()
            .top(px(34.0)) // Right below the tab bar with small gap
            .right(px(20.0)) // More margin from right edge to prevent clipping
            .w(px(260.0)) // Slightly narrower to ensure it fits
            .max_h(px(400.0)) // More space for items
            .bg(menu_style.background) // Now using the same background as tabs
            .border_1()
            .border_color(menu_style.border_color)
            .rounded(px(8.0)) // Consistent rounded corners
            .shadow_lg()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .py(px(4.0)) // Tighter padding
            .children(
                self.overflow_documents
                    .iter()
                    .map(|doc_info| {
                        let label = self.get_document_label(doc_info);
                        let is_active = self.active_doc_id == Some(doc_info.id);
                        let doc_id = doc_info.id;
                        let on_tab_click = self.on_tab_click.clone();

                        {
                            // Compute styles for active/inactive states with proper context
                            let current_style = if is_active {
                                let mut style = compute_contextual_style(
                                    &ui_theme,
                                    StyleState::Selected,
                                    StyleVariant::Ghost.as_str(),
                                    StyleSize::Small.as_str(),
                                    ColorContext::Floating,
                                );
                                // Override foreground to work with the dropdown background
                                style.foreground = nucleotide_ui::ColorTheory::best_text_color(dropdown_bg, &ui_theme.tokens);
                                style
                            } else {
                                item_style.clone()
                            };

                            div()
                                .id(SharedString::from(format!("overflow-tab-{}", doc_id)))
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(10.0)) // More spacing between icon and text
                                .px(px(12.0)) // More horizontal padding
                                .py(px(6.0)) // Comfortable vertical padding
                                .mx(px(4.0)) // Margin from edges
                                .w_full()
                                .min_h(px(32.0)) // Taller items for better touch targets
                                .cursor(CursorStyle::PointingHand)
                                .bg(current_style.background)
                                .text_color(text_color)
                                .rounded(px(4.0)) // Rounded item corners
                                .hover(|style| {
                                    // Create a more visible hover background by darkening/lightening the menu background
                                    let hover_bg = if ui_theme.is_dark() {
                                        // For dark themes, lighten the background
                                        gpui::hsla(dropdown_bg.h, dropdown_bg.s, (dropdown_bg.l + 0.1).min(1.0), dropdown_bg.a)
                                    } else {
                                        // For light themes, darken the background more noticeably
                                        gpui::hsla(dropdown_bg.h, dropdown_bg.s, (dropdown_bg.l - 0.15).max(0.0), dropdown_bg.a)
                                    };
                                    let hover_text = nucleotide_ui::ColorTheory::best_text_color(hover_bg, &ui_theme.tokens);
                                    style.bg(hover_bg).text_color(hover_text)
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
                                        .w(px(18.0)) // Slightly larger icon container
                                        .h(px(18.0))
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_center()
                                        .child(if let Some(ref path) = doc_info.path {
                                            nucleotide_ui::FileIcon::from_path(path, false)
                                                .size(16.0) // Larger icon for better visibility
                                                .text_color(text_color)
                                        } else {
                                            nucleotide_ui::FileIcon::scratch()
                                                .size(16.0) // Larger icon for better visibility
                                                .text_color(text_color)
                                        }),
                                )
                                .child(
                                    // File name
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .text_color(text_color)
                                        .text_size(px(13.0)) // Consistent font size
                                        .line_height(px(18.0)) // Better line height
                                        .when(doc_info.is_modified, |name_div| name_div.italic()) // Italic instead of underline
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
                        }
                    })
                    .collect::<Vec<_>>(),
            )
    }
}
