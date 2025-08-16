use crate::utils::color_to_hsla;
use crate::Core;
use gpui::{
    div, px, App, Context, Entity, EventEmitter, Hsla, IntoElement, ParentElement, Render, Styled,
    Window,
};
use helix_view::{DocumentId, ViewId};
use nucleotide_ui::theme_manager::ThemedContext;
use nucleotide_ui::{
    compute_component_style, StyleSize, StyleState, StyleVariant, ThemedContext as UIThemedContext,
};

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
        // Get LSP state from core if available
        let lsp_state = core.read(cx).lsp_state.clone();

        // Observe LSP state changes if available
        if let Some(lsp) = &lsp_state {
            cx.observe(lsp, |_, _, cx| {
                cx.notify();
            })
            .detach();
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
        if self.focused {
            tokens.colors.surface
        } else {
            tokens.colors.surface_disabled
        }
    }
}

impl EventEmitter<()> for StatusLineView {}

impl Render for StatusLineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Get LSP indicator first (before any other mutable borrows)
        let lsp_indicator = if let Some(lsp_state) = &self.lsp_state {
            lsp_state.update(cx, |state, _| state.get_lsp_indicator())
        } else {
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
                let status_bg = self.get_status_color(&tokens);
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
                let status_bg = self.get_status_color(&tokens);
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
        let status_bg = self.get_status_color(&tokens);

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

        // Build the status line layout using design tokens
        let mut status_bar = div()
            .h(tokens.sizes.button_height_sm)
            .w_full()
            .bg(status_bg)
            .border_t_1()
            .border_color(tokens.colors.border_default)
            .flex()
            .flex_row()
            .items_center()
            .px(tokens.sizes.space_4)
            .gap(tokens.sizes.space_2)
            .font(font)
            .text_size(font_size)
            .text_color(tokens.colors.text_primary)
            .child(
                // Mode indicator
                div()
                    .child(mode_name)
                    .min_w(px(24.))
                    .px(tokens.sizes.space_2)
                    .text_color(tokens.colors.text_secondary),
            )
            .child(
                // Divider
                div().w(px(1.)).h(px(16.)).bg(tokens.colors.border_muted),
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
                div().w(px(1.)).h(px(16.)).bg(tokens.colors.border_muted),
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
            status_bar = status_bar
                .child(
                    // Divider before LSP
                    div().w(px(1.)).h(px(16.)).bg(tokens.colors.border_muted),
                )
                .child(
                    // LSP indicator
                    div()
                        .child(indicator)
                        .min_w(px(16.))
                        .px(tokens.sizes.space_2),
                );
        }

        status_bar
    }
}
