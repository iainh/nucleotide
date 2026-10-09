//! GPUI's native text protocol, backed by Helix documents and temporary transactions.
use std::{cell::Cell, ops::Range, rc::Rc, sync::Arc};

use gpui::{
    App, Bounds, Context, Entity, EntityInputHandler, KeyDownEvent, Pixels, Point, UTF16Selection,
    Window, fill, point, px, size,
};
use helix_core::{Assoc, Rope, Selection, Transaction};
use helix_view::{
    DocumentId, Editor, ViewId,
    document::{Mode, SavePoint},
};
use nucleotide_editor::{EditorSurfacePointerEvent, EditorViewState, hit_test_document_position};
use nucleotide_ui::ThemedContext;

use super::DocumentView;

#[derive(Default)]
pub(super) struct NativeInput {
    pub bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    composition: Option<Composition>,
    resetting: bool,
    reset_requested: bool,
}

struct Composition {
    doc_id: DocumentId,
    savepoint: Arc<SavePoint>,
    ranges: Vec<Range<usize>>,
    primary: usize,
    replacement: Option<Range<usize>>,
    text: String,
    selected: Range<usize>,
}

impl Composition {
    fn contains_range(&self, text: &Rope, range: Range<usize>) -> bool {
        let range = chars_from_utf16(text, range);
        let marked = &self.ranges[self.primary];
        range.start >= marked.start && range.end <= marked.end
    }

    fn replace_within_mark(
        &self,
        text: &Rope,
        range: Range<usize>,
        new_text: &str,
    ) -> (String, usize) {
        let range = chars_from_utf16(text, range);
        let marked = &self.ranges[self.primary];
        let prefix = text.slice(marked.start..range.start).to_string();
        let prefix_chars = prefix.chars().count();
        (
            prefix + new_text + text.slice(range.end..marked.end).to_string().as_str(),
            prefix_chars,
        )
    }
}

/// Snap an invalid UTF-16 offset to the scalar's start, never split a surrogate pair.
fn chars_from_utf16(text: &Rope, range: Range<usize>) -> Range<usize> {
    let len = text.len_utf16_cu();
    let start = text.utf16_cu_to_char(range.start.min(len));
    start..text.utf16_cu_to_char(range.end.min(len)).max(start)
}

fn utf16_from_chars(text: &Rope, range: Range<usize>) -> Range<usize> {
    text.char_to_utf16_cu(range.start)..text.char_to_utf16_cu(range.end)
}

impl NativeInput {
    pub fn marked_ranges(&self) -> Vec<Range<usize>> {
        self.composition
            .as_ref()
            .map(|c| c.ranges.clone())
            .unwrap_or_default()
    }

    fn cancel(&mut self, editor: &mut Editor, view_id: ViewId) -> Option<Composition> {
        let composition = self.composition.take()?;
        if editor
            .tree
            .try_get(view_id)
            .is_some_and(|view| view.doc == composition.doc_id)
            && let Some(doc) = editor.documents.get_mut(&composition.doc_id)
        {
            let view = editor.tree.get_mut(view_id);
            doc.restore(view, &composition.savepoint, false);
        }
        Some(composition)
    }

    pub fn mark(
        &mut self,
        editor: &mut Editor,
        view_id: ViewId,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        if text.is_empty() && range.is_none() {
            self.cancel(editor, view_id);
            return;
        }
        let Some(view) = editor.tree.try_get(view_id) else {
            return;
        };
        let Some(doc) = editor.documents.get_mut(&view.doc) else {
            return;
        };
        let relative = chars_from_utf16(
            &Rope::from_str(text),
            selected.unwrap_or_else(|| {
                let end = text.encode_utf16().count();
                end..end
            }),
        );
        let (text, prefix_chars) = if let Some(composition) = &self.composition
            && let Some(range) = range.clone()
        {
            composition.replace_within_mark(doc.text(), range, text)
        } else {
            (text.to_owned(), 0)
        };
        if text.is_empty() {
            self.cancel(editor, view_id);
            return;
        }
        let composition = self.composition.get_or_insert_with(|| {
            let replacement = range.map(|range| chars_from_utf16(doc.text(), range));
            let selection = doc.selection(view_id);
            let (ranges, primary) = if let Some(range) = replacement.clone() {
                (vec![range], 0)
            } else {
                (
                    selection
                        .iter()
                        .map(|r| {
                            let cursor = r.cursor(doc.text().slice(..));
                            cursor..cursor
                        })
                        .collect(),
                    selection.primary_index(),
                )
            };
            Composition {
                doc_id: doc.id(),
                savepoint: doc.savepoint(view),
                ranges,
                primary,
                replacement,
                text: String::new(),
                selected: 0..0,
            }
        });
        let transaction = Transaction::change(
            doc.text(),
            composition
                .ranges
                .iter()
                .map(|r| (r.start, r.end, Some(text.as_str().into()))),
        );
        let length = text.chars().count();
        composition.ranges = composition
            .ranges
            .iter()
            .map(|range| {
                let start = transaction.changes().map_pos(range.start, Assoc::Before);
                start..start + length
            })
            .collect();
        let start = composition.ranges[composition.primary].start;
        composition.selected =
            start + prefix_chars + relative.start..start + prefix_chars + relative.end;
        let selection = Selection::new(
            composition
                .ranges
                .iter()
                .map(|r| helix_core::Range::point(r.start + prefix_chars + relative.end))
                .collect(),
            composition.primary,
        );
        doc.apply_temporary(&transaction.with_selection(selection), view_id);
        composition.text = text;
    }
}

impl DocumentView {
    pub(crate) fn has_native_composition(&self) -> bool {
        self.native_input.composition.is_some()
    }

    pub(super) fn uses_native_text(&self, event: &KeyDownEvent, cx: &App) -> bool {
        let core = self.core.read(cx);
        !self.native_input.resetting
            && event.keystroke.key_char.is_some()
            && event
                .keystroke
                .modifiers
                .is_subset_of(&gpui::Modifiers::shift())
            && core
                .editor_input
                .uses_native_text(&core.editor, crate::utils::translate_key(&event.keystroke))
    }

    pub(crate) fn cancel_native_composition(&mut self, cx: &mut Context<Self>) {
        if self.native_input.composition.is_none() {
            return;
        }
        self.core.update(cx, |core, cx| {
            self.native_input.cancel(&mut core.editor, self.view_id);
            cx.notify();
        });
        self.native_input.resetting = true;
        self.native_input.reset_requested = true;
        cx.notify();
    }

    pub(super) fn reset_native_input_if_needed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !std::mem::take(&mut self.native_input.reset_requested) {
            return;
        }
        // GPUI exposes native input acceptance, not an OS preedit reset. Keep it
        // disabled through one backend-observed frame before re-enabling it.
        // A single next-frame callback would re-enable before Wayland observes
        // the disabled state. Printable keys use the Helix bridge during reset.
        let view = cx.entity().downgrade();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, cx| {
                if let Some(view) = view.upgrade() {
                    view.update(cx, |view, cx| {
                        view.native_input.resetting = false;
                        cx.notify();
                    });
                }
            });
            window.refresh();
        });
    }

    fn native_input_enabled(&self, cx: &App) -> bool {
        let core = self.core.read(cx);
        !self.native_input.resetting
            && core.editor.mode() == Mode::Insert
            && core.editor.tree.focus == self.view_id
            && core.editor.tree.try_get(self.view_id).is_some()
    }

    fn commit_native_text(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.native_input_enabled(cx) {
            return;
        }
        let text = if let Some(composition) = &self.native_input.composition
            && let Some(range) = range.clone()
        {
            let core = self.core.read(cx);
            let doc = &core.editor.documents[&composition.doc_id];
            if composition.contains_range(doc.text(), range.clone()) {
                composition.replace_within_mark(doc.text(), range, text).0
            } else {
                let marked_text = composition.text.clone();
                self.commit_native_text(None, &marked_text, window, cx);
                text.to_owned()
            }
        } else {
            text.to_owned()
        };
        let is_cancellation = self.native_input.composition.is_some() && text.is_empty();
        let (doc_id, replacement) = self.core.update(cx, |core, _| {
            let composition = self.native_input.cancel(&mut core.editor, self.view_id);
            let view = core.editor.tree.get(self.view_id);
            let doc_id = view.doc;
            let doc = core.editor.documents.get_mut(&doc_id).unwrap();
            let replacement = if let Some(composition) = composition {
                composition.replacement
            } else {
                range.map(|r| chars_from_utf16(doc.text(), r))
            };
            (doc_id, replacement)
        });
        let workspace = window.root::<crate::workspace::Workspace>().flatten();
        if replacement.is_none() {
            for ch in text.chars() {
                if let Some(workspace) = &workspace {
                    workspace.update(cx, |workspace, cx| {
                        workspace.handle_completion_commit_character(ch, cx);
                    });
                }
                self.core.update(cx, |core, _| {
                    core.editor_input.insert_text(
                        ch.encode_utf8(&mut [0; 4]),
                        None,
                        &mut core.editor,
                        &mut core.jobs,
                    );
                });
            }
        } else if !is_cancellation {
            self.core.update(cx, |core, _| {
                core.editor_input
                    .insert_text(&text, replacement, &mut core.editor, &mut core.jobs);
            });
        }
        self.core.update(cx, |_, cx| {
            cx.emit(crate::Update::SelectionChanged {
                doc_id,
                view_id: self.view_id,
            });
            cx.emit(crate::Update::Redraw);
            cx.notify();
        });
        self.request_cursor_reveal();
        cx.notify();
    }
}

impl EntityInputHandler for DocumentView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let core = self.core.read(cx);
        let doc = core
            .editor
            .documents
            .get(&core.editor.tree.try_get(self.view_id)?.doc)?;
        let range = chars_from_utf16(doc.text(), range);
        *actual = Some(utf16_from_chars(doc.text(), range.clone()));
        Some(doc.text().slice(range).to_string())
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if !ignore_disabled && !self.native_input_enabled(cx) {
            return None;
        }
        let core = self.core.read(cx);
        let doc = core
            .editor
            .documents
            .get(&core.editor.tree.try_get(self.view_id)?.doc)?;
        let range = if let Some(composition) = &self.native_input.composition {
            composition.selected.clone()
        } else {
            // Helix's insert cursor still has a one-grapheme block selection.
            // Advertising that as selected text would make the IME delete it.
            let cursor = doc
                .selection(self.view_id)
                .primary()
                .cursor(doc.text().slice(..));
            cursor..cursor
        };
        Some(UTF16Selection {
            range: utf16_from_chars(doc.text(), range),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>> {
        let composition = self.native_input.composition.as_ref()?;
        let doc = self
            .core
            .read(cx)
            .editor
            .documents
            .get(&composition.doc_id)?;
        Some(utf16_from_chars(
            doc.text(),
            composition.ranges[composition.primary].clone(),
        ))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(composition) = &self.native_input.composition {
            let text = composition.text.clone();
            self.commit_native_text(None, &text, window, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_native_text(range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.native_input_enabled(cx) {
            return;
        }
        if let Some(composition) = &self.native_input.composition
            && let Some(range) = range.clone()
            && !composition.contains_range(
                self.core.read(cx).editor.documents[&composition.doc_id].text(),
                range,
            )
        {
            self.unmark_text(window, cx);
        }
        self.core.update(cx, |core, cx| {
            self.native_input
                .mark(&mut core.editor, self.view_id, range, text, selected);
            cx.notify();
        });
        self.request_cursor_reveal();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let core = self.core.read(cx);
        let doc = core
            .editor
            .documents
            .get(&core.editor.tree.try_get(self.view_id)?.doc)?;
        range_bounds(
            &self.editor_state,
            doc.text(),
            chars_from_utf16(doc.text(), range),
            bounds,
        )
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.native_input.bounds.get()?;
        if !bounds.contains(&position) {
            return None;
        }
        let core = self.core.read(cx);
        let doc = core
            .editor
            .documents
            .get(&core.editor.tree.try_get(self.view_id)?.doc)?;
        let metrics = self.editor_state.layout_snapshot();
        let hit = hit_test_document_position(
            EditorSurfacePointerEvent {
                position,
                bounds,
                modifiers: gpui::Modifiers::none(),
                cell_width: metrics.cell_width,
                line_height: metrics.line_height,
            },
            (metrics.gutter_width / metrics.cell_width) as u16,
            &self.editor_state.surface_metrics().line_cache(),
            doc,
        )?;
        Some(doc.text().char_to_utf16_cu(hit.char_idx))
    }

    fn accepts_text_input(&self, _: &mut Window, cx: &mut Context<Self>) -> bool {
        self.native_input_enabled(cx)
    }
}

/// Use the painted shaped segments, not logical columns: this includes tabs,
/// inlays, proportional fallback glyphs, wraps and both scroll offsets.
fn range_bounds(
    state: &EditorViewState,
    text: &Rope,
    range: Range<usize>,
    bounds: Bounds<Pixels>,
) -> Option<Bounds<Pixels>> {
    let line_idx = text.char_to_line(range.start);
    let line = text.line(line_idx);
    let offset = range.start - text.line_to_char(line_idx);
    let layouts = state
        .surface_metrics()
        .line_cache()
        .lines_for_index(line_idx);
    let Some(layout) = layouts
        .iter()
        .rev()
        .find(|line| line.segment_char_offset <= offset)
    else {
        // Empty documents and trailing phantom lines have no shaped text.
        return (range.start == text.len_chars())
            .then(|| {
                state
                    .layout_snapshot()
                    .cursor_overlay_bounds
                    .map(|(origin, size)| Bounds::new(origin, size))
            })
            .flatten();
    };
    let segment_start = line.char_to_byte(layout.segment_char_offset);
    let source_start = line.char_to_byte(offset) - segment_start;
    if source_start > layout.source_byte_for_display_byte(layout.shaped_line.len()) {
        return None;
    }
    let start = layout.display_byte_for_source_byte(source_start);
    let end = if text.char_to_line(range.end) == line_idx {
        let end = range.end - text.line_to_char(line_idx);
        layout.display_byte_for_source_byte(line.char_to_byte(end) - segment_start)
    } else {
        layout.shaped_line.len()
    };
    let metrics = state.layout_snapshot();
    let origin = bounds.origin + point(metrics.gutter_width, px(1.0)) + layout.origin;
    let x = layout.shaped_line.x_for_index(start);
    let rect = Bounds::new(
        origin + point(x, px(0.0)),
        size(
            (layout.shaped_line.x_for_index(end) - x).max(px(1.0)),
            metrics.line_height,
        ),
    );
    let text_bounds = Bounds::from_corners(
        bounds.origin + point(metrics.gutter_width, px(1.0)),
        bounds.bottom_right(),
    );
    let clipped = rect.intersect(&text_bounds);
    (clipped.size.width > px(0.0) && clipped.size.height > px(0.0)).then_some(clipped)
}

pub(super) fn paint_marked_ranges(
    state: &EditorViewState,
    ranges: &[Range<usize>],
    bounds: Option<Bounds<Pixels>>,
    view: &Entity<DocumentView>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(bounds) = bounds else { return };
    let view = view.read(cx);
    let core = view.core.read(cx);
    let Some(doc) = core
        .editor
        .tree
        .try_get(view.view_id)
        .and_then(|v| core.editor.documents.get(&v.doc))
    else {
        return;
    };
    let color = cx.theme().tokens.editor.text_primary;
    for range in ranges {
        for start in range.clone() {
            if let Some(rect) = range_bounds(state, doc.text(), start..start + 1, bounds) {
                window.paint_quad(fill(
                    Bounds::new(
                        point(rect.left(), rect.bottom() - px(1.0)),
                        size(rect.size.width, px(1.0)),
                    ),
                    color,
                ));
            }
        }
    }
}
