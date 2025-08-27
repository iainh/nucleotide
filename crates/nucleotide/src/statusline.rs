use crate::Core;
use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use helix_view::{DocumentId, ViewId};
use nucleotide_ui::ThemedContext;

/// StatusLineView is a proper GPUI View that can observe model changes
pub struct StatusLineView {
    core: Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    focused: bool,
    lsp_state: Option<Entity<nucleotide_lsp::LspState>>,
}

impl StatusLineView {
    pub fn new(
        core: Entity<Core>,
        doc_id: DocumentId,
        view_id: ViewId,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        nucleotide_logging::info!(
            doc_id = ?doc_id,
            view_id = ?view_id,
            focused = focused,
            "STATUSLINE: Creating new StatusLineView"
        );

        // Get LSP state from core if available
        let lsp_state = core.read(cx).lsp_state.clone();

        nucleotide_logging::info!(
            lsp_state_available = lsp_state.is_some(),
            doc_id = ?doc_id,
            view_id = ?view_id,
            "STATUSLINE: Retrieved LspState from core"
        );

        // Observe LSP state changes if available
        if let Some(lsp) = &lsp_state {
            nucleotide_logging::info!(
                doc_id = ?doc_id,
                view_id = ?view_id,
                "STATUSLINE: Setting up LspState observation"
            );
            cx.observe(lsp, |_, _, cx| {
                nucleotide_logging::debug!(
                    "STATUSLINE: LspState changed, notifying StatusLineView"
                );
                cx.notify();
            })
            .detach();
        } else {
            nucleotide_logging::warn!(
                doc_id = ?doc_id,
                view_id = ?view_id,
                "STATUSLINE: No LspState available for observation"
            );
        }

        Self {
            core,
            doc_id,
            view_id,
            focused,
            lsp_state,
        }
    }

    pub fn update_doc(&mut self, doc_id: DocumentId, view_id: ViewId, focused: bool) {
        self.doc_id = doc_id;
        self.view_id = view_id;
        self.focused = focused;
    }

    fn get_status_color(&self, tokens: &nucleotide_ui::DesignTokens) -> gpui::Hsla {
        // Use StatusBarTokens from hybrid color system for chrome backgrounds
        let status_bar_tokens = tokens.status_bar_tokens();

        // Debug logging to understand what's happening
        let titlebar_tokens = tokens.titlebar_tokens();

        // CRITICAL CHECK: Assert that status bar and titlebar use same colors
        if status_bar_tokens.background_active != titlebar_tokens.background {
            nucleotide_logging::error!(
                status_active = ?status_bar_tokens.background_active,
                status_inactive = ?status_bar_tokens.background_inactive,
                titlebar_bg = ?titlebar_tokens.background,
                chrome_footer = ?tokens.chrome.footer_background,
                chrome_titlebar = ?tokens.chrome.titlebar_background,
                colors_should_match = (tokens.chrome.footer_background == tokens.chrome.titlebar_background),
                "CRITICAL ERROR: Status bar and titlebar colors don't match!"
            );
        }

        let selected_color = if self.focused {
            status_bar_tokens.background_active
        } else {
            status_bar_tokens.background_inactive
        };

        nucleotide_logging::debug!(
            focused = self.focused,
            active_bg = ?status_bar_tokens.background_active,
            inactive_bg = ?status_bar_tokens.background_inactive,
            titlebar_bg = ?titlebar_tokens.background,
            selected_color = ?selected_color,
            colors_match_active = (status_bar_tokens.background_active == titlebar_tokens.background),
            colors_match_inactive = (status_bar_tokens.background_inactive == titlebar_tokens.background),
            doc_id = ?self.doc_id,
            view_id = ?self.view_id,
            "GET_STATUS_COLOR: Color selection details"
        );

        selected_color
    }
}

impl EventEmitter<()> for StatusLineView {}

impl Render for StatusLineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Get LSP indicator first (before any other mutable borrows)
        let lsp_indicator = if let Some(lsp_state) = &self.lsp_state {
            nucleotide_logging::debug!(
                doc_id = ?self.doc_id,
                view_id = ?self.view_id,
                "STATUSLINE: Getting LSP indicator from LspState"
            );
            let indicator = lsp_state.update(cx, |state, _| {
                let server_count = state.servers.len();
                let indicator = state.get_lsp_indicator();
                nucleotide_logging::info!(
                    server_count = server_count,
                    indicator_available = indicator.is_some(),
                    doc_id = ?self.doc_id,
                    view_id = ?self.view_id,
                    "STATUSLINE: LspState has servers and indicator"
                );
                indicator
            });
            indicator
        } else {
            nucleotide_logging::warn!(
                doc_id = ?self.doc_id,
                view_id = ?self.view_id,
                "STATUSLINE: No LspState available for indicator"
            );
            None
        };

        // Collect all data we need
        let core = self.core.read(cx);
        let editor = &core.editor;
        let doc = match editor.document(self.doc_id) {
            Some(doc) => doc,
            None => {
                // Use ThemedContext for theme access
                let theme = cx.theme();
                let tokens = &theme.tokens;
                let status_bg = self.get_status_color(tokens);
                return div()
                    .h(tokens.sizes.button_height_sm)
                    .w_full()
                    .bg(status_bg);
            }
        };
        let view = match editor.tree.try_get(self.view_id) {
            Some(view) => view,
            None => {
                // Use ThemedContext for theme access
                let theme = cx.theme();
                let tokens = &theme.tokens;
                let status_bg = self.get_status_color(tokens);
                return div()
                    .h(tokens.sizes.button_height_sm)
                    .w_full()
                    .bg(status_bg);
            }
        };

        // Use ThemedContext for consistent theme access
        let theme = cx.theme();
        let tokens = &theme.tokens;

        // Get UI font configuration
        let ui_font_config = cx.global::<crate::types::UiFontConfig>();
        let font = gpui::font(&ui_font_config.family);
        let font_size = gpui::px(ui_font_config.size);

        // Get status bar background color based on focus state
        let status_bg = self.get_status_color(tokens);

        // Debug log the actual color being applied in render
        nucleotide_logging::debug!(
            actual_status_bg = ?status_bg,
            focused = self.focused,
            doc_id = ?self.doc_id,
            view_id = ?self.view_id,
            chrome_footer = ?tokens.chrome.footer_background,
            chrome_titlebar = ?tokens.chrome.titlebar_background,
            "RENDER: Applying status bar background color"
        );

        // Build status components
        let position = helix_core::coords_at_pos(
            doc.text().slice(..),
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
        );

        let mode_name = match editor.mode() {
            helix_view::document::Mode::Normal => "NOR",
            helix_view::document::Mode::Insert => "INS",
            helix_view::document::Mode::Select => "SEL",
        };

        let file_name = doc
            .path()
            .map(|p| {
                let path_str = p.to_string_lossy().to_string();
                // Truncate long paths - keep filename and some parent directories
                if path_str.len() > 50 {
                    if let Some(file_name) = p.file_name() {
                        let file_name_str = file_name.to_string_lossy();
                        if let Some(parent) = p.parent() {
                            if let Some(parent_name) = parent.file_name() {
                                format!(".../{}/{}", parent_name.to_string_lossy(), file_name_str)
                            } else {
                                format!(".../{file_name_str}")
                            }
                        } else {
                            file_name_str.to_string()
                        }
                    } else {
                        "...".to_string()
                    }
                } else {
                    path_str
                }
            })
            .unwrap_or_else(|| "[scratch]".to_string());

        let position_text = format!("{}:{}", position.row + 1, position.col + 1);

        // Build the status line layout using StatusBarTokens for chrome colors
        let status_bar_tokens = tokens.status_bar_tokens();
        let mut status_bar = div()
            .h(tokens.sizes.button_height_sm)
            .w_full()
            .bg(status_bg)
            .border_t_1()
            .border_color(status_bar_tokens.border)
            .flex()
            .flex_row()
            .items_center()
            .px(tokens.sizes.space_4)
            .gap(tokens.sizes.space_2)
            .font(font)
            .text_size(font_size)
            .text_color(status_bar_tokens.text_primary)
            .child(
                // Mode indicator - use standard text color
                div()
                    .child(mode_name)
                    .min_w(px(24.))
                    .px(tokens.sizes.space_2)
                    .text_color(status_bar_tokens.text_primary), // Use standard text color
            )
            .child(
                // Divider
                div().w(px(1.)).h(px(16.)).bg(status_bar_tokens.border),
            )
            .child(
                // File name - takes up available space
                div()
                    .flex_1()
                    .overflow_hidden()
                    .px(tokens.sizes.space_2)
                    .child(file_name),
            )
            .child(
                // Divider
                div().w(px(1.)).h(px(16.)).bg(status_bar_tokens.border),
            )
            .child(
                // Position
                div()
                    .child(position_text)
                    .min_w(px(80.))
                    .px(tokens.sizes.space_2),
            );

        // Add LSP indicator if available
        if let Some(indicator) = lsp_indicator {
            nucleotide_logging::info!(
                doc_id = ?self.doc_id,
                view_id = ?self.view_id,
                indicator = %indicator,
                "STATUSLINE: Adding LSP indicator to status bar"
            );
            status_bar = status_bar
                .child(
                    // Divider before LSP
                    div().w(px(1.)).h(px(16.)).bg(status_bar_tokens.border),
                )
                .child(
                    // LSP indicator - dynamic width using design tokens
                    div()
                        .child(indicator)
                        .flex_shrink() // Allow shrinking when space is limited
                        .max_w(px(400.)) // Max width prevents taking over the entire status bar
                        .min_w(px(16.)) // Minimum for icon-only display
                        .overflow_hidden()
                        .text_ellipsis() // Graceful text truncation
                        .px(tokens.sizes.space_3) // Use design token spacing
                        .text_size(tokens.sizes.text_sm) // Use design token text sizing
                        .whitespace_nowrap(), // Prevent text wrapping
                );
        } else {
            nucleotide_logging::debug!(
                doc_id = ?self.doc_id,
                view_id = ?self.view_id,
                "STATUSLINE: No LSP indicator available for display"
            );
        }

        status_bar
    }
}
