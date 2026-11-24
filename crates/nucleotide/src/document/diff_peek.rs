// ABOUTME: Inline diff peek panel opened from the editor's diff gutter
// ABOUTME: Shows one changed hunk against the diff base, embedded between document lines

use std::ops::Range;

use gpui::{
    AnyElement, App, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, div,
};
use helix_core::{Tendril, Transaction};
use helix_view::{Document, DocumentId, ViewId, view::ReservedRows};
use nucleotide_ui::scrollbar::{Scrollbar, ScrollbarState};
use nucleotide_ui::{Button, ButtonSize, ButtonVariant};

use super::DocumentView;
pub(super) use crate::diff::DiffStyle as DiffPeekStyle;
use crate::diff::{diff_lines, render_diff_line};
#[cfg(test)]
use crate::diff::{expand_tabs, shared_affixes};

/// Diff lines shown before the panel body scrolls.
const MAX_BODY_LINES: usize = 20;

#[derive(Debug, Clone)]
pub(super) struct DiffPeek {
    pub doc_id: DocumentId,
    pub doc_version: i32,
    pub hunk_index: u32,
    pub hunk_count: u32,
    /// Document lines of the hunk; empty for a pure removal.
    pub after: Range<usize>,
    pub removed: Vec<String>,
    pub added: Vec<String>,
    scroll_handle: ScrollHandle,
    scrollbar_state: ScrollbarState,
}

impl PartialEq for DiffPeek {
    fn eq(&self, other: &Self) -> bool {
        self.doc_id == other.doc_id
            && self.doc_version == other.doc_version
            && self.hunk_index == other.hunk_index
            && self.hunk_count == other.hunk_count
            && self.after == other.after
            && self.removed == other.removed
            && self.added == other.added
    }
}

impl Eq for DiffPeek {}

impl DiffPeek {
    /// The change shown by the diff gutter marker on `line`.
    pub fn at_line(doc: &Document, line: usize) -> Option<Self> {
        let index = doc
            .diff_handle()?
            .load()
            .hunk_at(u32::try_from(line).ok()?, true)?;
        Self::for_hunk(doc, index)
    }

    pub fn for_hunk(doc: &Document, index: u32) -> Option<Self> {
        let diff = doc.diff_handle()?.load();
        if index >= diff.len() {
            return None;
        }
        let hunk = diff.nth_hunk(index);
        let base = diff.diff_base();
        let text = doc.text();
        let lines = |rope: &helix_core::Rope, range: Range<u32>| -> Vec<String> {
            (range.start as usize..(range.end as usize).min(rope.len_lines()))
                .map(|line| {
                    let line = rope.line(line).to_string();
                    line.trim_end_matches(['\n', '\r']).to_string()
                })
                .collect()
        };

        let scroll_handle = ScrollHandle::new();
        let scrollbar_state = ScrollbarState::new(scroll_handle.clone());
        Some(Self {
            doc_id: doc.id(),
            doc_version: doc.version(),
            hunk_index: index,
            hunk_count: diff.len(),
            after: hunk.after.start as usize..hunk.after.end as usize,
            removed: lines(base, hunk.before.clone()),
            added: lines(text, hunk.after.clone()),
            scroll_handle,
            scrollbar_state,
        })
    }

    /// The line the panel sits below: the hunk's last line, or the line above a removal.
    fn anchor_line(&self) -> usize {
        if self.after.is_empty() {
            self.after.start.saturating_sub(1)
        } else {
            self.after.end - 1
        }
    }

    fn body_lines(&self) -> usize {
        (self.removed.len() + self.added.len()).clamp(1, MAX_BODY_LINES)
    }

    /// Header row, body rows, and one row split into margins above and below.
    pub fn reservation(&self) -> ReservedRows {
        ReservedRows {
            doc: self.doc_id,
            doc_version: self.doc_version,
            line: self.anchor_line(),
            rows: self.body_lines() + 2,
        }
    }

    pub fn is_current_for(&self, doc: &Document) -> bool {
        self.doc_id == doc.id() && self.doc_version == doc.version()
    }

    /// Replaces the hunk's lines with the diff base's, like `:reset-diff-change`.
    pub fn revert_transaction(&self, doc: &Document) -> Option<Transaction> {
        if !self.is_current_for(doc) {
            return None;
        }
        let diff = doc.diff_handle()?.load();
        let hunk = diff.nth_hunk(self.hunk_index);
        if hunk.after.start as usize != self.after.start
            || hunk.after.end as usize != self.after.end
        {
            return None;
        }
        let base = diff.diff_base();
        let text = doc.text();
        let original: Tendril = base
            .slice(
                base.line_to_char(hunk.before.start as usize)
                    ..base.line_to_char(hunk.before.end as usize),
            )
            .chunks()
            .collect();
        Some(Transaction::change(
            text,
            std::iter::once((
                text.line_to_char(self.after.start),
                text.line_to_char(self.after.end),
                (!original.is_empty()).then_some(original),
            )),
        ))
    }

    pub fn original_text(&self) -> String {
        self.removed.join("\n")
    }

    fn title(&self) -> String {
        let kind = match (self.removed.is_empty(), self.added.is_empty()) {
            (true, _) => "Added",
            (false, true) => "Removed",
            (false, false) => "Modified",
        };
        let lines = if self.after.is_empty() {
            format!(
                "{} line{} above line {}",
                self.removed.len(),
                if self.removed.len() == 1 { "" } else { "s" },
                self.after.start + 1
            )
        } else if self.after.len() == 1 {
            format!("line {}", self.after.start + 1)
        } else {
            format!("lines {}–{}", self.after.start + 1, self.after.end)
        };
        format!(
            "{kind} · {lines} · change {} of {}",
            self.hunk_index + 1,
            self.hunk_count
        )
    }
}

pub(super) fn render_diff_peek(
    peek: &DiffPeek,
    style: &DiffPeekStyle,
    view: Entity<DocumentView>,
    view_id: ViewId,
    line_height: Pixels,
    cx: &App,
) -> AnyElement {
    let sizes = cx
        .try_global::<nucleotide_ui::Theme>()
        .map(|theme| theme.tokens.sizes)
        .unwrap_or_else(nucleotide_ui::tokens::SizeTokens::default);
    let ui_font: gpui::Font = cx
        .try_global::<nucleotide_types::FontSettings>()
        .map(|fonts| fonts.var_font.clone().into())
        .unwrap_or_else(|| gpui::font(".SystemUIFont"));
    let id = |name: &str| SharedString::from(format!("diff-peek-{name}-{view_id:?}"));
    let button = |name: &str, label: &'static str, icon: Option<&'static str>| {
        let button = Button::new(id(name), label)
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::ExtraSmall);
        match icon {
            Some(icon) => button.icon(icon),
            None => button,
        }
    };
    let on = |action: fn(&mut DocumentView, &mut gpui::Context<DocumentView>)| {
        let view = view.clone();
        move |_: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut App| {
            view.update(cx, action);
            cx.stop_propagation();
        }
    };

    let header = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .h(line_height)
        .px(sizes.space_3)
        .font(ui_font)
        .text_size(sizes.text_sm)
        .child(
            div()
                .text_color(style.text_secondary)
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .child(peek.title()),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(sizes.space_1)
                .child(
                    button("prev", "Previous", Some("icons/chevron-up.svg"))
                        .disabled(peek.hunk_index == 0)
                        .on_click(on(DocumentView::show_previous_diff_change)),
                )
                .child(
                    button("next", "Next", Some("icons/chevron-down.svg"))
                        .disabled(peek.hunk_index + 1 >= peek.hunk_count)
                        .on_click(on(DocumentView::show_next_diff_change)),
                )
                .child(
                    button("revert", "Revert", Some("icons/rotate-ccw.svg"))
                        .on_click(on(DocumentView::revert_diff_peek)),
                )
                .child(
                    button("copy", "Copy original", None)
                        .disabled(peek.removed.is_empty())
                        .on_click(on(DocumentView::copy_diff_peek_original)),
                )
                .child(
                    Button::icon_only(id("close"), "icons/close.svg")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::ExtraSmall)
                        .on_click(on(DocumentView::close_diff_peek)),
                ),
        );

    let body = div()
        .relative()
        .max_h(line_height * MAX_BODY_LINES as f32)
        .overflow_hidden()
        .child(
            div()
                .id(id("body"))
                .flex()
                .flex_col()
                .max_h(line_height * MAX_BODY_LINES as f32)
                .overflow_y_scroll()
                .track_scroll(&peek.scroll_handle)
                .font_family(style.font_family.clone())
                .text_size(style.font_size)
                .text_color(style.text)
                .children(
                    diff_lines(&peek.removed, &peek.added, style)
                        .into_iter()
                        .map(|line| render_diff_line(line, line_height)),
                ),
        )
        .child(Scrollbar::vertical_overlay(peek.scrollbar_state.clone()));

    div()
        .id(id("panel"))
        .size_full()
        // The reservation's spare row becomes the margin above and below the panel.
        .py(line_height * 0.5)
        .pr(sizes.space_4)
        // Clicks and hovers stay in the panel; wheel scrolling still reaches the editor.
        .block_mouse_except_scroll()
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(sizes.radius_lg)
                .border_1()
                .border_color(style.border)
                .bg(style.surface)
                .child(header)
                .child(body),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_affixes_isolate_the_changed_middle() {
        assert_eq!(
            shared_affixes("retry(3)?;", "retry(config.retries)?;"),
            (6, 3)
        );
        // The suffix never overlaps the prefix when one line extends the other.
        assert_eq!(shared_affixes("aa", "aaa"), (2, 0));
        assert_eq!(shared_affixes("é日x", "é月x"), (1, 1));
    }

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tc"), "ab  c");
    }
}
