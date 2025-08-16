// ABOUTME: Individual tab component for the tab bar with close button
// ABOUTME: Displays buffer name, modified indicator, and handles click events

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, App, CursorStyle, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, RenderOnce, SharedString, Styled, Window,
};
use helix_view::DocumentId;
use nucleotide_ui::{Button, ButtonSize, ButtonVariant, VcsIndicator, VcsStatus};

/// Type alias for mouse event handlers in tabs
type MouseEventHandler = Box<dyn Fn(&MouseUpEvent, &mut Window, &mut App) + 'static>;

/// A single tab in the tab bar
#[derive(IntoElement)]
pub struct Tab {
    /// Document ID this tab represents
    pub doc_id: DocumentId,
    /// Display label for the tab
    pub label: String,
    /// File path for determining icon
    pub file_path: Option<std::path::PathBuf>,
    /// Whether the document has unsaved changes
    pub is_modified: bool,
    /// Git status for VCS indicator
    pub git_status: Option<VcsStatus>,
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
        file_path: Option<std::path::PathBuf>,
        is_modified: bool,
        git_status: Option<VcsStatus>,
        is_active: bool,
        on_click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
        on_close: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            doc_id,
            label,
            file_path,
            is_modified,
            git_status,
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

        // Extract values we need before moving self
        let git_status = self.git_status.clone();

        // Get theme colors for visual continuity with editor
        let (bg_color, text_color, hover_bg) = if self.is_active {
            // Active tab should match editor background for visual continuity
            let editor_bg_style = helix_theme.get("ui.background");
            let editor_text_style = helix_theme.get("ui.text");

            let bg_color = editor_bg_style
                .bg
                .and_then(crate::utils::color_to_hsla)
                .unwrap_or(ui_theme.tokens.colors.background);

            let text_color = editor_text_style
                .fg
                .and_then(crate::utils::color_to_hsla)
                .unwrap_or(ui_theme.tokens.colors.text_primary);

            (bg_color, text_color, bg_color)
        } else {
            // Inactive tabs use statusline styling for background but same text color as active
            let statusline_style = helix_theme.get("ui.statusline");
            let editor_text_style = helix_theme.get("ui.text");

            let bg_color = statusline_style
                .bg
                .and_then(crate::utils::color_to_hsla)
                .unwrap_or(ui_theme.tokens.colors.surface);

            // Use same text color as active tabs for consistency
            let text_color = editor_text_style
                .fg
                .and_then(crate::utils::color_to_hsla)
                .unwrap_or(ui_theme.tokens.colors.text_primary);

            (bg_color, text_color, ui_theme.tokens.colors.surface_hover)
        };

        // Build the tab
        let tab_id = SharedString::from(format!("tab-{}", self.doc_id));
        div()
            .id(tab_id)
            .flex()
            .flex_none() // Don't grow or shrink
            .items_center()
            .pl(px(16.0)) // Left padding for the tab
            .pr(px(4.0)) // Minimal right padding so close button sits at edge
            .h(px(32.0)) // Slightly taller for better click targets
            .min_w(px(120.0)) // Minimum width to ensure readability
            // No max width - let it size to content
            .bg(bg_color)
            .hover(|style| style.bg(hover_bg))
            .cursor(CursorStyle::PointingHand)
            .border_r_1()
            .border_color(ui_theme.tokens.colors.border_default)
            .when(self.is_active, |this| {
                // Active tabs: no borders to create complete visual continuity with editor
                this
            })
            .when(!self.is_active, |this| {
                // Inactive tabs get bottom border to separate from editor
                this.border_b_1()
                    .border_color(ui_theme.tokens.colors.border_default)
            })
            .on_mouse_up(MouseButton::Left, {
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
                    .gap(px(4.0)) // Match file tree spacing (gap_1 ≈ 4px)
                    .child(
                        // File icon with VCS overlay
                        div()
                            .relative() // Needed for absolute positioning of the overlay
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if let Some(ref path) = self.file_path {
                                nucleotide_ui::FileIcon::from_path(path, false)
                                    .size(16.0)
                                    .text_color(text_color)
                            } else {
                                nucleotide_ui::FileIcon::scratch()
                                    .size(16.0)
                                    .text_color(text_color)
                            })
                            .when_some(git_status.as_ref(), |div, status| {
                                let indicator =
                                    VcsIndicator::new(status.clone()).size(8.0).overlay();
                                div.child(indicator)
                            }),
                    )
                    .child(
                        // Tab label
                        div()
                            .text_color(text_color)
                            .text_size(px(14.0)) // Slightly larger text
                            .when(self.is_active, |this| {
                                // Active tab labels are slightly bolder/more prominent
                                this.font_weight(gpui::FontWeight::MEDIUM)
                            })
                            .when(self.is_modified, |this| {
                                // Modified files show with underline
                                this.underline()
                            })
                            .child(self.label.clone()),
                    )
                    .child(
                        // Close button
                        div().ml(px(4.0)).child(
                            Button::icon_only("tab-close", "icons/close.svg")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Small)
                                .on_click({
                                    let on_close = self.on_close;
                                    move |event, window, cx| {
                                        on_close(event, window, cx);
                                        cx.stop_propagation();
                                    }
                                }),
                        ),
                    ),
            )
    }
}
