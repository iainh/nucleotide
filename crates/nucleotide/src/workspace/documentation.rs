//! Hover documentation requests, sidebar state, sizing, and rendering.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, Entity, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, px, svg,
};
use helix_core::syntax::config::LanguageServerFeature;
use helix_lsp::lsp;
use nucleotide_logging::{debug, warn};
use nucleotide_types::scrollbar::SCROLLBAR_THICKNESS;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::scrollbar::Scrollbar;
use nucleotide_ui::{
    Button, ButtonSize, ButtonVariant, MarkdownStyle, StateView, Toolbar, Tooltipped,
    markdown_extended,
};

use super::{Workspace, file_tree_tokens_for_gui_config};
use crate::Core;
use crate::types::HoverDocEntry;

pub(super) const DOC_SIDEBAR_MIN_WIDTH: f32 = 240.0;

pub(super) const DOC_SIDEBAR_DEFAULT_WIDTH: f32 = 360.0;

pub(super) const DOC_SIDEBAR_MAX_WIDTH: f32 = 640.0;

const DOC_SIDEBAR_MIN_EDITOR_WIDTH: f32 = 240.0;

pub(super) fn show_hover_docs(core: Entity<Core>, _handle: tokio::runtime::Handle, cx: &mut App) {
    use futures_util::stream::{FuturesOrdered, StreamExt};

    debug!("Requesting hover documentation");

    let hover_requests = {
        let core_r = core.read(cx);
        let editor = &core_r.editor;
        let hover_requests = || {
            let Some(view) = editor.tree.try_get(editor.tree.focus) else {
                debug!("No focused editor view available for hover documentation");
                return None;
            };
            let Some(doc) = editor.documents.get(&view.doc) else {
                debug!(
                    view_id = ?view.id,
                    doc_id = ?view.doc,
                    "No focused document available for hover documentation"
                );
                return None;
            };

            let Some(url) = doc.url() else {
                debug!(
                    view_id = ?view.id,
                    doc_id = ?view.doc,
                    "Focused document has no file URL for hover documentation"
                );
                return None;
            };

            let mut seen = HashSet::new();
            let identifier = lsp::TextDocumentIdentifier::new(url);
            let mut requested = 0usize;

            let futures: FuturesOrdered<_> = doc
                .language_servers_with_feature(LanguageServerFeature::Hover)
                .filter(|ls| seen.insert(ls.id()))
                .filter_map(|language_server| {
                    requested += 1;
                    let server_name = language_server.name().to_string();
                    let identifier = identifier.clone();
                    let pos = doc.position(view.id, language_server.offset_encoding());
                    let request = language_server.text_document_hover(identifier, pos, None)?;

                    Some(async move { request.await.map(|hover| (server_name, hover)) })
                })
                .collect();

            Some((futures, requested))
        };
        hover_requests()
    };

    let Some((mut futures, requested_servers)) = hover_requests else {
        core.update(cx, |core, cx| {
            core.editor
                .set_status("No file-backed document is available for documentation.");
            cx.emit(crate::Update::HoverDocs(Vec::new()));
        });
        return;
    };

    if requested_servers == 0 {
        debug!("No LSP servers with hover capability are available");
        core.update(cx, |core, cx| {
            core.editor
                .set_error("No configured language server supports hover");
            cx.emit(crate::Update::HoverDocs(Vec::new()));
        });
        return;
    }

    let core_weak = core.downgrade();
    cx.spawn(async move |cx| {
        let mut entries: Vec<HoverDocEntry> = Vec::new();

        while let Some(result) = futures.next().await {
            match result {
                Ok((server_name, Some(hover))) => {
                    let markdown = hover_contents_to_markdown(hover.contents);
                    if !markdown.trim().is_empty() {
                        entries.push(HoverDocEntry {
                            server_name,
                            markdown,
                        });
                    }
                }
                Ok((_server_name, None)) => {}
                Err(err) => {
                    warn!(error = %err, "Hover request failed");
                }
            }
        }

        if entries.is_empty() {
            if let Some(core) = core_weak.upgrade() {
                core.update(cx, |core, cx| {
                    core.editor.set_status("No hover results available.");
                    cx.emit(crate::Update::HoverDocs(Vec::new()));
                });
            }
            return;
        }

        if let Some(core) = core_weak.upgrade() {
            let payload = entries;
            core.update(cx, |_core, cx| {
                cx.emit(crate::Update::HoverDocs(payload));
            });
        }
    })
    .detach();
}

fn hover_contents_to_markdown(contents: lsp::HoverContents) -> String {
    fn marked_string_to_markdown(contents: lsp::MarkedString) -> String {
        match contents {
            lsp::MarkedString::String(contents) => contents,
            lsp::MarkedString::LanguageString(string) => {
                if string.language == "markdown" {
                    string.value
                } else {
                    format!("```{}\n{}\n```", string.language, string.value)
                }
            }
        }
    }

    match contents {
        lsp::HoverContents::Scalar(contents) => marked_string_to_markdown(contents),
        lsp::HoverContents::Array(contents) => contents
            .into_iter()
            .map(marked_string_to_markdown)
            .collect::<Vec<_>>()
            .join("\n\n"),
        lsp::HoverContents::Markup(contents) => contents.value,
    }
}

impl Workspace {
    pub(super) fn render_documentation_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let tokens = &cx.theme().tokens;
        let gui_config = &self.core.read(cx).config.gui;
        let file_tree_tokens = file_tree_tokens_for_gui_config(tokens, gui_config);
        let mut markdown_style = MarkdownStyle::from_tokens(tokens);
        markdown_style.body_font_size = tokens.sizes.text_base;

        let mut body = div()
            .id("documentation-sidebar-body")
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.doc_sidebar_scroll_handle)
            .px(tokens.sizes.space_4)
            .py(tokens.sizes.space_4)
            .gap(tokens.sizes.space_4);

        if self.doc_sidebar_loading {
            body = body.child(
                StateView::new("documentation-sidebar-loading", "Loading documentation")
                    .loading(true)
                    .compact(true),
            );
        } else if self.doc_sidebar_entries.is_empty() {
            body = body.child(
                StateView::new("documentation-sidebar-empty", "No documentation available")
                    .detail("Move the cursor to a symbol with hover documentation.")
                    .icon("icons/book-text.svg")
                    .compact(true),
            );
        } else {
            for (index, entry) in self.doc_sidebar_entries.iter().enumerate() {
                body = body.child(
                    div()
                        .id(SharedString::from(format!(
                            "documentation-sidebar-entry-{index}"
                        )))
                        .flex()
                        .flex_col()
                        .gap(tokens.sizes.space_2)
                        .when(index > 0, |section| {
                            section
                                .border_t_1()
                                .border_color(file_tree_tokens.separator)
                                .pt(tokens.sizes.space_4)
                        })
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(file_tree_tokens.item_text_secondary)
                                .child(entry.server_name.clone()),
                        )
                        .child(
                            markdown_extended(entry.markdown.clone(), markdown_style.clone())
                                .id(("documentation-sidebar-markdown", index)),
                        ),
                );
            }
        }

        let body_container = div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(div().size_full().min_h(px(0.0)).child(body))
            .when_some(
                Scrollbar::vertical(self.doc_sidebar_scrollbar_state.clone()),
                |container, scrollbar| {
                    container.child(
                        div()
                            .id("documentation-sidebar-scrollbar")
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(SCROLLBAR_THICKNESS)
                            .child(scrollbar),
                    )
                },
            );

        div()
            .id("documentation-sidebar")
            .w(px(self.doc_sidebar_width))
            .h_full()
            .flex_shrink_0()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(file_tree_tokens.background)
            .border_l_1()
            .border_color(file_tree_tokens.border)
            .text_color(file_tree_tokens.item_text)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Toolbar::new("documentation-sidebar-header")
                    .height(crate::tab::tab_container_height(*tokens))
                    .border_color(file_tree_tokens.separator)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .gap(tokens.sizes.space_2)
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(file_tree_tokens.item_text)
                            .child(
                                svg()
                                    .path("icons/book-text.svg")
                                    .size(tokens.sizes.space_4)
                                    .text_color(file_tree_tokens.item_text)
                                    .flex_shrink_0(),
                            )
                            .child("Documentation"),
                    )
                    .child(
                        Button::icon_only("documentation-sidebar-close", "icons/close.svg")
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::ExtraSmall)
                            .tooltip("Close Documentation")
                            .aria_label("Close documentation sidebar")
                            .activate_on_mouse_down()
                            .on_click(cx.listener(|workspace, _event, _window, cx| {
                                workspace.close_documentation_sidebar(cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
            .child(body_container)
            .into_any_element()
    }

    fn close_documentation_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.doc_sidebar_visible || self.doc_sidebar_loading {
            self.doc_sidebar_visible = false;
            self.doc_sidebar_loading = false;
            cx.notify();
        }
    }

    pub(super) fn toggle_documentation_sidebar(&mut self, cx: &mut Context<Self>) -> bool {
        if self.doc_sidebar_visible {
            self.close_documentation_sidebar(cx);
            return false;
        }

        self.doc_sidebar_visible = true;
        self.doc_sidebar_loading = true;
        self.doc_sidebar_entries.clear();
        cx.notify();
        true
    }

    pub(super) fn set_documentation_sidebar_entries(
        &mut self,
        entries: Vec<HoverDocEntry>,
        cx: &mut Context<Self>,
    ) {
        if !self.doc_sidebar_visible && !self.doc_sidebar_loading {
            return;
        }

        self.doc_sidebar_visible = true;
        self.doc_sidebar_loading = false;
        self.doc_sidebar_entries = entries;
        cx.notify();
    }

    pub(super) fn max_documentation_sidebar_width(available_width: f32) -> f32 {
        (available_width - DOC_SIDEBAR_MIN_EDITOR_WIDTH)
            .clamp(DOC_SIDEBAR_MIN_WIDTH, DOC_SIDEBAR_MAX_WIDTH)
    }

    pub(super) fn clamped_documentation_sidebar_width(width: f32, available_width: f32) -> f32 {
        width.clamp(
            DOC_SIDEBAR_MIN_WIDTH,
            Self::max_documentation_sidebar_width(available_width),
        )
    }

    pub(super) fn sync_documentation_sidebar_width_for_viewport(&mut self, available_width: f32) {
        if !self.doc_sidebar_visible {
            return;
        }

        let width =
            Self::clamped_documentation_sidebar_width(self.doc_sidebar_width, available_width);
        if (self.doc_sidebar_width - width).abs() > 0.5 {
            self.doc_sidebar_width = width;
        }
    }
}
