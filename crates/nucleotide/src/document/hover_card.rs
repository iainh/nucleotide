//! Mouse hover cards: LSP hover results for the word under a resting pointer,
//! anchored to that word, with navigation actions in a footer.

use std::ops::Range;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, AvailableSpace, Bounds, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement, Styled, Task, Window,
    canvas, deferred, div, px, size,
};
use helix_view::DocumentId;
use nucleotide_types::scrollbar::SCROLLBAR_THICKNESS;
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
use nucleotide_ui::theme_manager::HelixThemedContext;
use nucleotide_ui::{
    Button, ButtonSize, ButtonVariant, MarkdownStyle, PopupConstraints, PopupPositioner,
    markdown_extended,
};

use super::DocumentView;
use crate::types::HoverDocEntry;

/// How long the pointer rests on a word before the card asks for it.
const HOVER_DELAY: Duration = Duration::from_millis(450);
/// Grace period for the pointer to travel from the word into the card.
const DISMISS_DELAY: Duration = Duration::from_millis(250);
const CARD_MAX_WIDTH: f32 = 560.0;
const DOCS_MAX_HEIGHT: f32 = 320.0;
const EDGE_MARGIN: f32 = 8.0;

/// The card on screen, if any, and the word the pointer rests on, if it is
/// another one. A pending word replaces the card only once its results arrive,
/// so passing over words on the way into the card does not close it.
#[derive(Default)]
pub(super) struct HoverState {
    card: Option<HoverCard>,
    pending: Option<PendingHover>,
}

struct PendingHover {
    doc_id: DocumentId,
    word: Range<usize>,
    _request: Task<()>,
}

struct HoverCard {
    doc_id: DocumentId,
    doc_version: i32,
    /// Viewport scroll when the word was hit-tested; any scroll, on either
    /// axis and by part of a row, moves the word away from the anchor.
    scroll_position: Point<Pixels>,
    char_idx: usize,
    word: Range<usize>,
    anchor: Bounds<Pixels>,
    entries: Vec<HoverDocEntry>,
    dismiss: Option<Task<()>>,
    card_hovered: bool,
    scroll_handle: gpui::ScrollHandle,
    scrollbar_state: ScrollbarState,
}

#[derive(Clone, Copy)]
enum HoverAction {
    GoToDefinition,
    FindReferences,
    OpenDocs,
}

impl DocumentView {
    /// Tracks the word under a moving pointer: a new word starts the hover delay,
    /// and leaving the word for anything but the card starts the dismissal.
    pub(super) fn hover_pointer_moved(
        &mut self,
        position: Point<Pixels>,
        button_pressed: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.native_input.bounds.get() else {
            return;
        };
        if button_pressed || !window.is_window_active() {
            self.dismiss_hover_card(cx);
            return;
        }
        let metrics = self.editor_state.surface_metrics().get();
        let event = nucleotide_editor::EditorSurfacePointerEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            bounds,
            line_height: metrics.line_height,
            cell_width: metrics.cell_width,
        };
        let (target, doc) = {
            let core = self.core.read(cx);
            let target = self
                .editor_state
                .hover_target_at_event(&core.editor, self.view_id, event);
            let doc = core
                .editor
                .tree
                .try_get(self.view_id)
                .and_then(|view| core.editor.documents.get(&view.doc))
                .map(|doc| (doc.id(), doc.version()));
            (target, doc)
        };
        let (Some(target), Some((doc_id, doc_version))) = (target, doc) else {
            self.hover_pointer_left(cx);
            return;
        };

        if let Some(card) = &mut self.hover.card
            && card.doc_id == doc_id
            && card.word == target.word
        {
            card.dismiss = None;
            self.hover.pending = None;
            return;
        }
        if self
            .hover
            .pending
            .as_ref()
            .is_some_and(|pending| pending.doc_id == doc_id && pending.word == target.word)
        {
            return;
        }

        let scroll_position = self.editor_state.viewport().scroll_position();
        let word = target.word.clone();
        let request = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let Ok(Some(requests)) = this.update(cx, |view, cx| {
                crate::workspace::hover_requests(
                    &view.core.read(cx).editor,
                    view.view_id,
                    Some(target.char_idx),
                )
            }) else {
                return;
            };
            let entries = crate::workspace::collect_hover_entries(requests).await;
            let _ = this.update(cx, |view, cx| {
                let still_wanted = view
                    .hover
                    .pending
                    .take_if(|pending| pending.doc_id == doc_id && pending.word == target.word)
                    .is_some();
                // The pointer reached the open card while this was in flight.
                let card_in_use = view
                    .hover
                    .card
                    .as_ref()
                    .is_some_and(|card| card.card_hovered);
                if !still_wanted || card_in_use || entries.is_empty() {
                    return;
                }
                let scroll_handle = gpui::ScrollHandle::new();
                view.hover.card = Some(HoverCard {
                    doc_id,
                    doc_version,
                    scroll_position,
                    char_idx: target.char_idx,
                    word: target.word,
                    anchor: target.anchor,
                    entries,
                    dismiss: None,
                    card_hovered: false,
                    scrollbar_state: ScrollbarState::new(scroll_handle.clone()),
                    scroll_handle,
                });
                view.sync_hover_card(cx);
                cx.notify();
            });
        });
        self.hover.pending = Some(PendingHover {
            doc_id,
            word,
            _request: request,
        });
        // An open card for another word gets the same grace as leaving it.
        self.schedule_hover_dismiss(cx);
    }

    /// Hides the card after a short grace period unless the pointer reaches it.
    fn schedule_hover_dismiss(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &mut self.hover.card else {
            return;
        };
        if card.dismiss.is_some() {
            return;
        }
        card.dismiss = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DISMISS_DELAY).await;
            let _ = this.update(cx, |view, cx| {
                if view
                    .hover
                    .card
                    .as_ref()
                    .is_some_and(|card| !card.card_hovered)
                {
                    view.hover.card = None;
                    cx.notify();
                }
            });
        }));
    }

    /// The pointer left the editor: drop any pending word and let the card go.
    pub(super) fn hover_pointer_left(&mut self, cx: &mut Context<Self>) {
        self.hover.pending = None;
        self.schedule_hover_dismiss(cx);
    }

    pub(super) fn dismiss_hover_card(&mut self, cx: &mut Context<Self>) {
        self.hover.pending = None;
        if self.hover.card.take().is_some() {
            cx.notify();
        }
    }

    /// Drops a card whose document changed or scrolled since it was anchored.
    pub(super) fn sync_hover_card(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &self.hover.card else {
            return;
        };
        let current = {
            let core = self.core.read(cx);
            core.editor
                .tree
                .try_get(self.view_id)
                .and_then(|view| core.editor.documents.get(&view.doc))
                .is_some_and(|doc| doc.id() == card.doc_id && doc.version() == card.doc_version)
        };
        let scrolled = self.editor_state.viewport().scroll_position() != card.scroll_position;
        if !current || scrolled {
            self.hover.card = None;
        }
    }

    fn set_hover_card_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        let Some(card) = &mut self.hover.card else {
            return;
        };
        card.card_hovered = hovered;
        if hovered {
            card.dismiss = None;
            self.hover.pending = None;
        } else {
            self.schedule_hover_dismiss(cx);
        }
    }

    fn run_hover_action(
        &mut self,
        action: HoverAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.hover.card.take() else {
            return;
        };
        self.hover.pending = None;
        cx.notify();
        let entries = card.entries;
        if let HoverAction::OpenDocs = action {
            self.core.update(cx, |_core, cx| {
                cx.emit(crate::Update::OpenHoverDocs(entries));
            });
            return;
        }

        // Navigation acts on the primary cursor, so put it on the hovered word.
        let view_id = self.view_id;
        let moved = self.core.update(cx, |core, cx| {
            if core.editor.tree.try_get(view_id).is_none() {
                return false;
            }
            let Some(doc) = core.editor.documents.get_mut(&card.doc_id) else {
                return false;
            };
            if doc.version() != card.doc_version {
                return false;
            }
            doc.set_selection(view_id, helix_core::Selection::point(card.char_idx));
            cx.notify();
            true
        });
        if !moved {
            return;
        }
        super::focus_editor_view(&self.core, view_id, cx);
        window.focus(&self.focus, cx);
        let action: Box<dyn gpui::Action> = match action {
            HoverAction::GoToDefinition => Box::new(crate::actions::workspace::GoToDefinition),
            HoverAction::FindReferences => Box::new(crate::actions::workspace::FindReferences),
            HoverAction::OpenDocs => unreachable!("handled above"),
        };
        window.dispatch_action(action, cx);
    }

    /// The card, placed over the window above everything else, or `None` while
    /// there is nothing to show.
    pub(super) fn render_hover_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = self.hover.card.as_ref()?;
        let entries = &card.entries;
        let pane = self.native_input.bounds.get()?;
        let anchor = card.anchor;
        let margin = px(EDGE_MARGIN);

        let tokens = cx.theme().tokens;
        let ui_tokens = cx.ui_theme().tokens;
        let ui_font: gpui::Font = cx
            .try_global::<nucleotide_types::FontSettings>()
            .map(|fonts| fonts.var_font.clone().into())
            .unwrap_or_else(|| gpui::font(".SystemUIFont"));
        let code_font = self.style.font_family.clone();
        let code_size = self.style.font_size.to_pixels(px(14.0));

        let mut docs_style = MarkdownStyle::from_tokens(&tokens).compact();
        docs_style.code_font_family = code_font.clone();
        docs_style.body_font_size = tokens.sizes.text_sm;
        let mut signature_style = docs_style.clone();
        signature_style.body_font_size = code_size;
        signature_style.code_background = gpui::transparent_black();
        signature_style.code_border = gpui::transparent_black();
        signature_style.code_language_labels = false;

        // The positioner puts a card that fits neither side on the larger one;
        // cap the card at that side so the docs, not the word, give way.
        let room = (anchor.top() - pane.top()).max(pane.bottom() - anchor.bottom())
            - margin
            - nucleotide_ui::completion_popup::ANCHORED_POPUP_GAP;
        let docs_max_height = px(DOCS_MAX_HEIGHT);
        let max_width = px(CARD_MAX_WIDTH).min(pane.size.width - margin * 2.0);

        let mut signatures = Vec::new();
        let mut docs = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let (signature, body) = split_hover_signature(&entry.markdown);
            if let Some(signature) = signature {
                signatures.push(
                    markdown_extended(signature, signature_style.clone())
                        .id(("hover-card-signature", index))
                        .into_any_element(),
                );
            }
            if !body.trim().is_empty() {
                docs.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens.sizes.space_1)
                        .when(entries.len() > 1, |section| {
                            section.child(
                                div()
                                    .text_size(tokens.sizes.text_xs)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui_tokens.chrome.text_chrome_secondary)
                                    .child(entry.server_name.clone()),
                            )
                        })
                        .child(
                            markdown_extended(body, docs_style.clone())
                                .id(("hover-card-docs", index)),
                        )
                        .into_any_element(),
                );
            }
        }

        let view = cx.entity();
        let action_button = |name: &'static str, label: &'static str, icon, action| {
            let view = view.clone();
            Button::new(SharedString::from(format!("hover-card-{name}")), label)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::ExtraSmall)
                .icon(icon)
                .on_click(move |_, window, cx| {
                    view.update(cx, |view, cx| view.run_hover_action(action, window, cx));
                    cx.stop_propagation();
                })
        };
        let separator = ui_tokens.chrome.border_muted;
        let has_signature = !signatures.is_empty();
        let has_docs = !docs.is_empty();

        let hover_view = view.clone();
        let mut card_element = div()
            .id("hover-card")
            .occlude()
            .on_hover(move |hovered, _window, cx| {
                hover_view.update(cx, |view, cx| view.set_hover_card_hovered(*hovered, cx));
            })
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .max_w(max_width)
            .min_w(px(240.0).min(max_width))
            .max_h(room.max(px(0.0)))
            .overflow_hidden()
            .font(ui_font)
            .text_color(ui_tokens.chrome.text_on_chrome)
            .bg(ui_tokens.chrome.popup_background)
            .border_1()
            .border_color(ui_tokens.chrome.popup_border)
            .rounded(tokens.sizes.radius_lg)
            .shadow(vec![ui_tokens.chrome.shadow_md.to_box_shadow(false)])
            .when(has_signature, |card| {
                card.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .py(tokens.sizes.space_1)
                        .when(has_docs, |header| {
                            header.border_b_1().border_color(separator)
                        })
                        .children(signatures),
                )
            })
            .when(has_docs, |this| {
                this.child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .flex_shrink(1.0)
                        .min_h(px(0.0))
                        .max_h(docs_max_height)
                        .overflow_hidden()
                        .child(
                            div()
                                .id("hover-card-docs")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h(px(0.0))
                                .gap(tokens.sizes.space_3)
                                .overflow_y_scroll()
                                .track_scroll(&card.scroll_handle)
                                .px(tokens.sizes.space_3)
                                .py(tokens.sizes.space_3)
                                .children(docs),
                        )
                        .when_some(
                            Scrollbar::vertical(card.scrollbar_state.clone()),
                            |container, scrollbar| {
                                container.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .right_0()
                                        .bottom_0()
                                        .w(SCROLLBAR_THICKNESS)
                                        .child(scrollbar),
                                )
                            },
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_shrink_0()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens.sizes.space_1)
                    .px(tokens.sizes.space_2)
                    .py(tokens.sizes.space_1)
                    .border_t_1()
                    .border_color(separator)
                    .child(action_button(
                        "definition",
                        "Go to Definition",
                        "icons/arrow-right.svg",
                        HoverAction::GoToDefinition,
                    ))
                    .child(action_button(
                        "references",
                        "Find References",
                        "icons/search.svg",
                        HoverAction::FindReferences,
                    ))
                    .child(action_button(
                        "sidebar",
                        "Open Docs in Sidebar",
                        "icons/book-text.svg",
                        HoverAction::OpenDocs,
                    )),
            )
            .into_any_element();

        let positioner = PopupPositioner::new(PopupConstraints {
            margin,
            max_width,
            max_height: pane.size.height,
            min_width: px(0.0),
            min_height: px(0.0),
            ..Default::default()
        });
        Some(
            deferred(
                canvas(
                    move |_bounds, window, cx| {
                        let content_size = card_element.layout_as_root(
                            size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                            window,
                            cx,
                        );
                        let position =
                            positioner.calculate_anchored_position(anchor, content_size, pane);
                        card_element.prepaint_at(position.bounds.origin, window, cx);
                        card_element
                    },
                    |_, mut card, window, cx| card.paint(window, cx),
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

/// Splits a hover's leading fenced code blocks, its signature, from the prose
/// after them, dropping a rule that separates the two. Consecutive blocks, such
/// as rust-analyzer's module path and declaration, merge into one block.
fn split_hover_signature(markdown: &str) -> (Option<String>, String) {
    let mut rest = markdown.trim_start();
    let mut language = None;
    let mut lines = Vec::new();
    while let Some(after_fence) = rest.strip_prefix("```") {
        let Some(info_end) = after_fence.find('\n') else {
            break;
        };
        let Some(close) = after_fence[info_end..].find("\n```") else {
            break;
        };
        // An empty block closes on the line after its info string.
        let body = after_fence
            .get(info_end + 1..info_end + close)
            .unwrap_or_default();
        language.get_or_insert_with(|| after_fence[..info_end].trim().to_string());
        lines.push(body.trim_end());
        let close_end = info_end + close + "\n```".len();
        let line_end = after_fence[close_end..]
            .find('\n')
            .map_or(after_fence.len(), |offset| close_end + offset);
        rest = after_fence[line_end..].trim_start();
    }
    let Some(language) = language else {
        return (None, markdown.to_string());
    };
    let body = rest
        .strip_prefix("---")
        .or_else(|| rest.strip_prefix("***"))
        .or_else(|| rest.strip_prefix("___"))
        .unwrap_or(rest)
        .trim_start();
    (
        Some(format!("```{language}\n{}\n```", lines.join("\n"))),
        body.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::split_hover_signature;

    #[test]
    fn leading_code_blocks_become_the_signature() {
        // rust-analyzer's shape: module path, signature, rule, docs.
        let hover = "```rust\ntokio::task\n```\n\n```rust\npub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>\n```\n\n---\n\nRuns the closure.\n\n```rust\nlet x = 1;\n```";
        let (signature, body) = split_hover_signature(hover);
        assert_eq!(
            signature.as_deref(),
            Some("```rust\ntokio::task\npub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>\n```")
        );
        assert_eq!(body, "Runs the closure.\n\n```rust\nlet x = 1;\n```");
    }

    #[test]
    fn prose_first_hovers_and_unclosed_fences_have_no_signature() {
        let prose = "Docs first.\n\n```rust\nfn f()\n```";
        assert_eq!(split_hover_signature(prose), (None, prose.to_string()));
        let unclosed = "```rust\nfn f()";
        assert_eq!(
            split_hover_signature(unclosed),
            (None, unclosed.to_string())
        );
        assert_eq!(
            split_hover_signature("```rust\n```\nDocs."),
            (Some("```rust\n\n```".to_string()), "Docs.".to_string())
        );
        let only = "```go\nfunc F()\n```";
        assert_eq!(
            split_hover_signature(only),
            (Some(only.to_string()), String::new())
        );
    }
}
