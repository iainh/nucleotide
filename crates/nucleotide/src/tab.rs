// ABOUTME: Individual tab component for the tab bar with close button
// ABOUTME: Displays buffer name, modified indicator, and handles click events

use gpui::prelude::FluentBuilder;
use gpui::Hsla;
use gpui::{
    div, px, App, CursorStyle, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, RenderOnce, SharedString, Styled, Window,
};
use helix_view::DocumentId;
use nucleotide_ui::{Button, ButtonSize, ButtonVariant, ColorTheory, VcsIndicator, VcsStatus};

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
    /// Computed colors for consistent theming
    pub active_tab_bg: Option<Hsla>,
    pub inactive_tab_bg: Option<Hsla>,
    pub border_color: Option<Hsla>,
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
            active_tab_bg: None,
            inactive_tab_bg: None,
            border_color: None,
        }
    }

    /// Set computed colors for consistent theming
    pub fn with_computed_colors(
        mut self,
        active_bg: Hsla,
        inactive_bg: Hsla,
        border: Hsla,
    ) -> Self {
        self.active_tab_bg = Some(active_bg);
        self.inactive_tab_bg = Some(inactive_bg);
        self.border_color = Some(border);
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme_manager = cx.global::<crate::ThemeManager>();
        let helix_theme = theme_manager.helix_theme();

        // Use provider hooks to get theme - fallback to global if provider not available
        let ui_theme =
            nucleotide_ui::providers::use_provider::<nucleotide_ui::providers::ThemeProvider>()
                .map(|provider| provider.current_theme().clone())
                .unwrap_or_else(|| cx.global::<nucleotide_ui::Theme>().clone());

        // Use provider hooks to get configuration for animations
        let enable_animations = nucleotide_ui::providers::use_provider::<
            nucleotide_ui::providers::ConfigurationProvider,
        >()
        .map(|config| config.ui_config.animation_config.enable_animations)
        .unwrap_or(true); // Default to enabled if provider not available

        // Extract values we need before moving self
        let git_status = self.git_status.clone();

        // Use computed colors if provided, otherwise fall back to theme colors
        let (bg_color, text_color, hover_bg, border_color) = if self.is_active {
            let bg_color = self.active_tab_bg.unwrap_or_else(|| {
                let editor_bg_style = helix_theme.get("ui.background");
                editor_bg_style
                    .bg
                    .and_then(crate::utils::color_to_hsla)
                    .unwrap_or(ui_theme.tokens.colors.background)
            });

            let editor_text_style = helix_theme.get("ui.text");
            let text_color = editor_text_style
                .fg
                .and_then(crate::utils::color_to_hsla)
                .unwrap_or(ui_theme.tokens.colors.text_primary);

            let border_color = self
                .border_color
                .unwrap_or(ui_theme.tokens.colors.border_default);

            (bg_color, text_color, bg_color, border_color)
        } else {
            let bg_color = self.inactive_tab_bg.unwrap_or_else(|| {
                let statusline_style = helix_theme.get("ui.statusline");
                statusline_style
                    .bg
                    .and_then(crate::utils::color_to_hsla)
                    .unwrap_or(ui_theme.tokens.colors.surface)
            });

            let editor_text_style = helix_theme.get("ui.text");
            let text_color = editor_text_style
                .fg
                .and_then(crate::utils::color_to_hsla)
                .unwrap_or(ui_theme.tokens.colors.text_primary);

            let border_color = self
                .border_color
                .unwrap_or(ui_theme.tokens.colors.border_default);

            // Compute hover color relative to the tab's background
            let hover_bg = if ui_theme.is_dark() {
                // Dark theme: lighten the inactive tab background
                ColorTheory::lighten(bg_color, 0.1)
            } else {
                // Light theme: darken the inactive tab background
                ColorTheory::darken(bg_color, 0.1)
            };

            (bg_color, text_color, hover_bg, border_color)
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
            .when(enable_animations, |tab| {
                tab.hover(|style| style.bg(hover_bg))
            })
            .cursor(CursorStyle::PointingHand)
            .border_r_1()
            .border_color(border_color)
            .when(self.is_active, |this| {
                // Active tabs: no bottom border for seamless integration with editor
                // but keep right border for tab separation
                this
            })
            .when(!self.is_active, |this| {
                // Inactive tabs get bottom border to separate from editor/active content
                this.border_b_1().border_color(border_color)
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
                                .class("tab-close-button") // Add CSS class for styling
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
