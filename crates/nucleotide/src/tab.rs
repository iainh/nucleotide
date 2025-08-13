// ABOUTME: Individual tab component for the tab bar with close button
// ABOUTME: Displays buffer name, modified indicator, and handles click events

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, CursorStyle, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, RenderOnce, SharedString, Styled, Window,
};
use helix_view::DocumentId;

/// Type alias for mouse event handlers in tabs
type MouseEventHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

/// A single tab in the tab bar
#[derive(IntoElement)]
pub struct Tab {
    /// Document ID this tab represents
    pub doc_id: DocumentId,
    /// Display label for the tab
    pub label: String,
    /// Whether the document has unsaved changes
    pub is_modified: bool,
    /// Whether this tab is currently active
    pub is_active: bool,
    /// Callback when tab is clicked
    pub on_click: MouseEventHandler,
    /// Callback when close button is clicked
    pub on_close: MouseEventHandler,
}

impl Tab {
    pub fn new(
        doc_id: DocumentId,
        label: String,
        is_modified: bool,
        is_active: bool,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
        on_close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            doc_id,
            label,
            is_modified,
            is_active,
            on_click: Box::new(on_click),
            on_close: Box::new(on_close),
        }
    }
}

impl RenderOnce for Tab {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme_manager = cx.global::<crate::ThemeManager>();
        let helix_theme = theme_manager.helix_theme();
        let ui_theme = cx.global::<nucleotide_ui::Theme>();

        // Get theme colors
        let statusline_style = if self.is_active {
            helix_theme.get("ui.statusline.active")
        } else {
            helix_theme.get("ui.statusline")
        };

        let bg_color = statusline_style
            .bg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.surface);

        let text_color = statusline_style
            .fg
            .and_then(crate::utils::color_to_hsla)
            .unwrap_or(ui_theme.text);

        let hover_bg = if self.is_active {
            bg_color
        } else {
            ui_theme.surface_hover
        };

        // Build the tab
        let tab_id = SharedString::from(format!("tab-{}", self.doc_id));
        div()
            .id(tab_id)
            .flex()
            .flex_none() // Don't grow or shrink
            .items_center()
            .px(px(16.0)) // Horizontal padding for the tab
            .h(px(32.0)) // Slightly taller for better click targets
            .min_w(px(120.0)) // Minimum width to ensure readability
            // No max width - let it size to content
            .bg(bg_color)
            .hover(|style| style.bg(hover_bg))
            .cursor(CursorStyle::PointingHand)
            .border_r_1()
            .border_color(ui_theme.border)
            .when(self.is_active, |this| {
                this.border_b_2().border_color(ui_theme.accent)
            })
            .on_mouse_down(MouseButton::Left, {
                let on_click = self.on_click;
                move |event, window, cx| {
                    on_click(event, window, cx);
                    cx.stop_propagation();
                }
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0)) // Better spacing between elements
                    .child(
                        // Modified indicator
                        div()
                            .when(self.is_modified, |this| {
                                this.child(
                                    div()
                                        .text_color(text_color)
                                        .text_size(px(16.0)) // Larger bullet
                                        .child("•"),
                                )
                            })
                            .when(!self.is_modified, |this| {
                                this.w(px(10.0)) // Space placeholder
                            }),
                    )
                    .child(
                        // Tab label
                        div()
                            .text_color(text_color)
                            .text_size(px(14.0)) // Slightly larger text
                            .child(self.label.clone()),
                    )
                    .child(
                        // Close button
                        div()
                            .ml(px(4.0)) // Less margin since we have gap
                            .px(px(4.0)) // Horizontal padding for better click target
                            .py(px(2.0)) // Vertical padding
                            .rounded_sm()
                            .hover(|style| style.bg(ui_theme.surface_active))
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let on_close = self.on_close;
                                move |event, window, cx| {
                                    on_close(event, window, cx);
                                    cx.stop_propagation();
                                }
                            })
                            .child(
                                // Simple X icon for close button
                                div()
                                    .text_color(text_color.opacity(0.6))
                                    .text_size(px(16.0)) // Larger X
                                    .hover(|style| style.text_color(text_color))
                                    .child("×"),
                            ),
                    ),
            )
    }
}
