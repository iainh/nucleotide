use crate::utils::color_to_hsla;
use crate::Core;
use gpui::*;
use helix_view::{DocumentId, ViewId};

/// StatusLineView is a proper GPUI View that can observe model changes
pub struct StatusLineView {
    core: Entity<Core>,
    doc_id: DocumentId,
    view_id: ViewId,
    focused: bool,
    lsp_state: Option<Entity<crate::core::lsp_state::LspState>>,
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
            }).detach();
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

    fn style(&self, cx: &mut App) -> (Hsla, Hsla) {
        let theme = cx.global::<crate::theme_manager::ThemeManager>().helix_theme();
        let base_style = if self.focused {
            theme.get("ui.statusline")
        } else {
            theme.get("ui.statusline.inactive")
        };
        let foreground = base_style
            .fg
            .and_then(color_to_hsla)
            .unwrap_or(hsla(0.5, 0.5, 0.5, 1.));
        let background = base_style
            .bg
            .and_then(color_to_hsla)
            .unwrap_or(hsla(0.5, 0.5, 0.5, 1.));
        (foreground, background)
    }
}

impl EventEmitter<()> for StatusLineView {}

impl Render for StatusLineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Get UI font configuration
        let ui_font_config = cx.global::<crate::UiFontConfig>();
        let font = gpui::font(&ui_font_config.family);
        let font_size = gpui::px(ui_font_config.size);
        
        // Get theme colors
        let (fg_color, bg_color) = self.style(cx);
        
        // Create divider color with reduced opacity
        let divider_color = Hsla {
            h: fg_color.h,
            s: fg_color.s,
            l: fg_color.l,
            a: 0.3,
        };
        
        // Collect all data we need
        let core = self.core.read(cx);
        let editor = &core.editor;
        let doc = match editor.document(self.doc_id) {
            Some(doc) => doc,
            None => return div().h(px(24.)).w_full().bg(bg_color),
        };
        let view = match editor.tree.try_get(self.view_id) {
            Some(view) => view,
            None => return div().h(px(24.)).w_full().bg(bg_color),
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

        let file_name = doc.path()
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
                                format!(".../{}", file_name_str)
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
            .bg(bg_color)
            .flex()
            .flex_row()
            .items_center()
            .px_2()
            .gap_2()
            .font(font)
            .text_size(font_size)
            .text_color(fg_color)
            .child(
                // Mode indicator
                div()
                    .child(mode_name)
                    .min_w(px(24.))
            )
            .child(
                // Divider
                div()
                    .w(px(1.))
                    .h(px(16.))
                    .bg(divider_color)
            )
            .child(
                // File name - takes up available space
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(file_name)
            )
            .child(
                // Divider
                div()
                    .w(px(1.))
                    .h(px(16.))
                    .bg(divider_color)
            )
            .child(
                // Position
                div()
                    .child(position_text)
                    .min_w(px(80.))
            );
            
        // Add LSP indicator if available
        if let Some(indicator) = lsp_indicator {
            status_bar = status_bar
                .child(
                    // Divider before LSP
                    div()
                        .w(px(1.))
                        .h(px(16.))
                        .bg(divider_color)
                )
                .child(
                    // LSP indicator
                    div()
                        .child(indicator)
                        .min_w(px(16.))
                );
        }
        
        status_bar
    }
}

