use crate::utils::color_to_hsla;
use crate::Core;
use gpui::{
    div, px, App, Context, Entity, EventEmitter, Hsla, IntoElement, ParentElement, Render, Styled,
    Window,
};
use helix_view::{DocumentId, ViewId};
use nucleotide_ui::theme_manager::ThemedContext;
use nucleotide_ui::{compute_component_style, StyleSize, StyleState, StyleVariant};

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

    fn get_computed_style(&self, cx: &mut App) -> nucleotide_ui::ComputedStyle {
        let ui_theme =
            nucleotide_ui::providers::use_provider::<nucleotide_ui::providers::ThemeProvider>()
                .map(|provider| provider.current_theme().clone())
                .unwrap_or_else(|| cx.global::<nucleotide_ui::Theme>().clone());

        // Use different style states based on focus
        let style_state = if self.focused {
            StyleState::Default
        } else {
            StyleState::Disabled // Inactive statusline uses disabled styling
        };

        // Compute style using the enhanced style system
        let computed_style = compute_component_style(
            &ui_theme,
            style_state,
            StyleVariant::Secondary.as_str(), // Statusline uses secondary variant
            StyleSize::Medium.as_str(),
        );

        // If computed style doesn't provide good colors, fall back to Helix theme
        let base_style = if self.focused {
            cx.theme_style("ui.statusline")
        } else {
            cx.theme_style("ui.statusline.inactive")
        };

        let background = base_style
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(computed_style.background);

        let foreground = base_style
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(computed_style.foreground);

        nucleotide_ui::ComputedStyle {
            background,
            foreground,
            border_color: computed_style.border_color,
            border_width: computed_style.border_width,
            border_radius: computed_style.border_radius,
            padding_x: computed_style.padding_x,
            padding_y: computed_style.padding_y,
            font_size: computed_style.font_size,
            font_weight: computed_style.font_weight,
            opacity: computed_style.opacity,
            shadow: computed_style.shadow,
            transition: computed_style.transition,
        }
    }
}

impl EventEmitter<()> for StatusLineView {}

impl Render for StatusLineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Get UI font configuration
        let ui_font_config = cx.global::<crate::types::UiFontConfig>();
        let font = gpui::font(&ui_font_config.family);
        let font_size = gpui::px(ui_font_config.size);

        // Get computed theme style using enhanced styling system
        let computed_style = self.get_computed_style(cx);

        // Create divider color with reduced opacity
        let divider_color = Hsla {
            h: computed_style.foreground.h,
            s: computed_style.foreground.s,
            l: computed_style.foreground.l,
            a: 0.3,
        };

        // Collect all data we need
        let core = self.core.read(cx);
        let editor = &core.editor;
        let doc = match editor.document(self.doc_id) {
            Some(doc) => doc,
            None => return div().h(px(24.)).w_full().bg(computed_style.background),
        };
        let view = match editor.tree.try_get(self.view_id) {
            Some(view) => view,
            None => return div().h(px(24.)).w_full().bg(computed_style.background),
        };

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

        // Get LSP indicator if available
        let lsp_indicator = if let Some(lsp_state) = &self.lsp_state {
            lsp_state.update(cx, |state, _| state.get_lsp_indicator())
        } else {
            None
        };

        // Build the status line layout
        let mut status_bar = div()
            .h(px(24.))
            .w_full()
            .bg(computed_style.background)
            .flex()
            .flex_row()
            .items_center()
            .px(computed_style.padding_x)
            .gap_2()
            .font(font)
            .text_size(font_size)
            .text_color(computed_style.foreground)
            .child(
                // Mode indicator
                div().child(mode_name).min_w(px(24.)),
            )
            .child(
                // Divider
                div().w(px(1.)).h(px(16.)).bg(divider_color),
            )
            .child(
                // File name - takes up available space
                div().flex_1().overflow_hidden().child(file_name),
            )
            .child(
                // Divider
                div().w(px(1.)).h(px(16.)).bg(divider_color),
            )
            .child(
                // Position
                div().child(position_text).min_w(px(80.)),
            );

        // Add LSP indicator if available
        if let Some(indicator) = lsp_indicator {
            status_bar = status_bar
                .child(
                    // Divider before LSP
                    div().w(px(1.)).h(px(16.)).bg(divider_color),
                )
                .child(
                    // LSP indicator
                    div().child(indicator).min_w(px(16.)),
                );
        }

        status_bar
    }
}
