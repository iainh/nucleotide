// ABOUTME: Native editor highlighting helpers for GPUI text rendering
// ABOUTME: Converts Helix syntax, selection, and diagnostic styles into TextRuns

use std::ops::Range;

use gpui::{Font, Hsla, SharedString, TextRun};
use helix_core::{
    RopeSlice,
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    syntax::{self, HighlightEvent, OverlayHighlights},
};
use helix_stdx::rope::RopeSliceExt;
use helix_view::{
    Document, Theme, View,
    document::Mode,
    graphics::{Color, CursorKind, Style},
    view::ViewPosition,
};
use nucleotide_logging::trace;

use crate::{
    line_plan::VisibleLinePlan,
    line_text::shared_line_text_without_trailing_newline,
    soft_wrap::{SoftWrapVisualLine, decorate_soft_wrap_line_runs},
    style::{create_styled_text_run, helix_color_to_hsla},
};

pub type DiagnosticOverlaySpans = Vec<(syntax::Highlight, Range<usize>)>;

pub struct HighlightLineParams<'a> {
    pub doc: &'a Document,
    pub view: &'a View,
    pub theme: &'a Theme,
    pub syntax_loader: &'a helix_core::syntax::Loader,
    pub editor_mode: Mode,
    pub cursor_shape: &'a helix_view::editor::CursorShapeConfig,
    pub is_view_focused: bool,
    pub view_position: ViewPosition,
    pub line_start: usize,
    pub line_end: usize,
    pub fg_color: Hsla,
    pub font: Font,
    pub default_text_style: Style,
    pub default_bg: Hsla,
    pub diagnostic_overlay_spans: Option<&'a DiagnosticOverlaySpans>,
}

pub struct EditorLineHighlightContext<'a> {
    pub doc: &'a Document,
    pub view: &'a View,
    pub theme: &'a Theme,
    pub syntax_loader: &'a helix_core::syntax::Loader,
    pub editor_mode: Mode,
    pub cursor_shape: &'a helix_view::editor::CursorShapeConfig,
    pub is_view_focused: bool,
    pub view_position: ViewPosition,
    pub fg_color: Hsla,
    pub font: Font,
    pub default_text_style: Style,
    pub default_bg: Hsla,
    pub diagnostic_overlay_spans: Option<&'a DiagnosticOverlaySpans>,
}

pub struct SoftWrapHighlightedLineRunsParams<'a> {
    pub context: EditorLineHighlightContext<'a>,
    pub visual: &'a SoftWrapVisualLine,
    pub wrap_indicator_color: Option<Hsla>,
}

pub struct UnwrappedHighlightedLineParams<'a> {
    pub context: EditorLineHighlightContext<'a>,
    pub text: RopeSlice<'a>,
    pub line: &'a VisibleLinePlan,
}

#[derive(Debug, Clone)]
pub struct UnwrappedHighlightedLine {
    pub line_text: SharedString,
    pub line_runs: Vec<TextRun>,
}

pub fn diagnostic_overlay_spans(doc: &Document, theme: &Theme) -> Option<DiagnosticOverlaySpans> {
    let mut spans = DiagnosticOverlaySpans::new();
    let error_h = theme.find_highlight_exact("diagnostic.error");
    let warn_h = theme.find_highlight_exact("diagnostic.warning");
    let info_h = theme.find_highlight_exact("diagnostic.info");
    let hint_h = theme.find_highlight_exact("diagnostic.hint");

    let diagnostics = doc.diagnostics();
    if diagnostics.is_empty() {
        return None;
    }

    for diagnostic in diagnostics.iter() {
        let (start, end) = (diagnostic.range.start, diagnostic.range.end);
        let highlight = match diagnostic.severity {
            Some(helix_core::diagnostic::Severity::Error) => error_h,
            Some(helix_core::diagnostic::Severity::Warning) => warn_h,
            Some(helix_core::diagnostic::Severity::Info) => info_h,
            Some(helix_core::diagnostic::Severity::Hint) | None => hint_h,
        };

        let Some(highlight) = highlight else {
            continue;
        };

        if start >= end {
            spans.push((highlight, start..start.saturating_add(1)));
        } else {
            spans.push((highlight, start..end));
        }
    }

    (!spans.is_empty()).then_some(spans)
}

pub fn text_style_at_position(
    doc: &Document,
    view_position: ViewPosition,
    theme: &Theme,
    syntax_loader: &helix_core::syntax::Loader,
    position: usize,
) -> Style {
    let text = doc.text().slice(..);
    let (anchor, height) = syntax_highlight_window(text, view_position);

    let syntax_highlighter = doc_syntax_highlights(doc, anchor, height, syntax_loader);

    let default_style = theme.get("ui.text");
    let text_style = Style {
        fg: default_style.fg,
        bg: None,
        ..Default::default()
    };

    let mut syntax_hl = SyntaxHighlighter::new(syntax_highlighter, text, theme, text_style);

    while position >= syntax_hl.pos {
        syntax_hl.advance();
    }

    syntax_hl.style
}

pub fn highlight_line(params: HighlightLineParams<'_>) -> Vec<TextRun> {
    let text = params.doc.text().slice(..);
    let (anchor, height) = syntax_highlight_window(text, params.view_position);
    let syntax_highlighter =
        doc_syntax_highlights(params.doc, anchor, height, params.syntax_loader);

    let selection_overlay = selection_overlay_highlights(
        params.editor_mode,
        params.doc,
        params.view,
        params.theme,
        params.cursor_shape,
    );

    let mut syntax_hl = SyntaxHighlighter::new(
        syntax_highlighter,
        text,
        params.theme,
        params.default_text_style,
    );
    let mut overlays = Vec::new();
    overlays.push(selection_overlay);
    if let Some(highlights) = params.diagnostic_overlay_spans {
        overlays.push(OverlayHighlights::Heterogenous {
            highlights: highlights.clone(),
        });
    }
    let mut overlay_hl = OverlayHighlighter::new(overlays, params.theme);

    highlight_line_with_state(
        text,
        &mut syntax_hl,
        &mut overlay_hl,
        params.line_start,
        params.line_end,
        params.fg_color,
        params.font,
        params.default_bg,
    )
}

pub fn soft_wrap_highlighted_line_runs(
    params: SoftWrapHighlightedLineRunsParams<'_>,
) -> Vec<TextRun> {
    let context = params.context;
    let mut line_runs = if let (Some(line_start), Some(line_end)) =
        (params.visual.line_start_char, params.visual.line_end_char)
    {
        highlight_line(HighlightLineParams {
            doc: context.doc,
            view: context.view,
            theme: context.theme,
            syntax_loader: context.syntax_loader,
            editor_mode: context.editor_mode,
            cursor_shape: context.cursor_shape,
            is_view_focused: context.is_view_focused,
            view_position: context.view_position,
            line_start,
            line_end,
            fg_color: context.fg_color,
            font: context.font.clone(),
            default_text_style: context.default_text_style,
            default_bg: context.default_bg,
            diagnostic_overlay_spans: context.diagnostic_overlay_spans,
        })
    } else {
        Vec::new()
    };

    line_runs = decorate_soft_wrap_line_runs(
        line_runs,
        params.visual,
        &context.font,
        context.fg_color,
        params.wrap_indicator_color,
    );

    line_runs
}

pub fn unwrapped_highlighted_line(
    params: UnwrappedHighlightedLineParams<'_>,
) -> UnwrappedHighlightedLine {
    let line_slice = params
        .text
        .slice(params.line.line_start..params.line.line_end);
    let line_text = shared_line_text_without_trailing_newline(line_slice);
    let context = params.context;
    let line_runs = highlight_line(HighlightLineParams {
        doc: context.doc,
        view: context.view,
        theme: context.theme,
        syntax_loader: context.syntax_loader,
        editor_mode: context.editor_mode,
        cursor_shape: context.cursor_shape,
        is_view_focused: context.is_view_focused,
        view_position: context.view_position,
        line_start: params.line.line_start,
        line_end: params.line.line_end,
        fg_color: context.fg_color,
        font: context.font,
        default_text_style: context.default_text_style,
        default_bg: context.default_bg,
        diagnostic_overlay_spans: context.diagnostic_overlay_spans,
    });

    UnwrappedHighlightedLine {
        line_text,
        line_runs,
    }
}

pub fn gpui_hsla_to_helix_color(c: Hsla) -> Option<Color> {
    let c_chroma = (1.0 - (2.0 * c.l - 1.0).abs()) * c.s;
    let x = c_chroma * (1.0 - (((c.h * 360.0) / 60.0) % 2.0 - 1.0).abs());
    let hue = c.h * 360.0;
    let (r1, g1, b1) = if hue < 60.0 {
        (c_chroma, x, 0.0)
    } else if hue < 120.0 {
        (x, c_chroma, 0.0)
    } else if hue < 180.0 {
        (0.0, c_chroma, x)
    } else if hue < 240.0 {
        (0.0, x, c_chroma)
    } else if hue < 300.0 {
        (x, 0.0, c_chroma)
    } else {
        (c_chroma, 0.0, x)
    };
    let m = c.l - c_chroma / 2.0;
    let (r, g, b) = (r1 + m, g1 + m, b1 + m);
    Some(Color::Rgb(
        (r * 255.0) as u8,
        (g * 255.0) as u8,
        (b * 255.0) as u8,
    ))
}

fn syntax_highlight_window(text: RopeSlice<'_>, view_position: ViewPosition) -> (usize, u16) {
    let anchor = view_position.anchor.min(text.len_chars());
    let lines_from_anchor = text.len_lines() - text.char_to_line(anchor);
    let height = u16::try_from(lines_from_anchor).unwrap_or(u16::MAX);

    (anchor, height)
}

fn doc_syntax_highlights<'d>(
    doc: &'d Document,
    anchor: usize,
    height: u16,
    syntax_loader: &'d helix_core::syntax::Loader,
) -> Option<syntax::Highlighter<'d>> {
    let syntax = doc.syntax()?;
    trace!(language = ?doc.language_name(), "Document has syntax support");

    let text = doc.text().slice(..);
    let anchor = anchor.min(text.len_chars().saturating_sub(1));
    let row = text.char_to_line(anchor);
    let range = viewport_byte_range(text, row, height);
    let start = (range.start as u32).min(text.len_bytes() as u32);
    let end = (range.end as u32).min(text.len_bytes() as u32);

    if start >= end {
        return None;
    }

    Some(syntax.highlighter(text, syntax_loader, start..end))
}

fn viewport_byte_range(text: RopeSlice<'_>, row: usize, height: u16) -> Range<usize> {
    let row = row.min(text.len_lines().saturating_sub(1));
    let start = text.line_to_byte(row);
    let end_row = (row + height as usize).min(text.len_lines());
    let end = text.line_to_byte(end_row);

    if start >= end {
        0..text.len_bytes().min(1)
    } else {
        start..end
    }
}

fn selection_overlay_highlights(
    mode: Mode,
    doc: &Document,
    view: &View,
    theme: &Theme,
    cursor_shape_config: &helix_view::editor::CursorShapeConfig,
) -> OverlayHighlights {
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id);
    let primary_idx = selection.primary_index();

    let cursorkind = cursor_shape_config.from_mode(mode);
    let cursor_is_block = cursorkind == CursorKind::Block;

    let selection_scope = theme
        .find_highlight_exact("ui.selection")
        .expect("could not find `ui.selection` scope in the theme!");
    let primary_selection_scope = theme
        .find_highlight_exact("ui.selection.primary")
        .unwrap_or(selection_scope);

    let mut spans = Vec::new();
    for (i, range) in selection.iter().enumerate() {
        let selection_is_primary = i == primary_idx;
        let selection_scope = if selection_is_primary {
            primary_selection_scope
        } else {
            selection_scope
        };

        if range.head == range.anchor && range.head == text.len_chars() {
            continue;
        }

        let range = range.min_width_1(text);

        if range.head > range.anchor {
            let cursor_start = prev_grapheme_boundary(text, range.head);
            let selection_end = if selection_is_primary && cursor_is_block && mode != Mode::Insert {
                cursor_start
            } else {
                range.head
            };

            if range.anchor < selection_end {
                spans.push((selection_scope, range.anchor..selection_end));
            }
        } else {
            let cursor_end = next_grapheme_boundary(text, range.head);
            let selection_start = if selection_is_primary && cursor_is_block && mode != Mode::Insert
            {
                cursor_end
            } else {
                range.head
            };

            if selection_start < range.anchor {
                spans.push((selection_scope, selection_start..range.anchor));
            }
        }
    }

    if spans.is_empty() {
        OverlayHighlights::Homogeneous {
            highlight: syntax::Highlight::new(0),
            ranges: Vec::new(),
        }
    } else {
        OverlayHighlights::Heterogenous { highlights: spans }
    }
}

#[allow(clippy::too_many_arguments)]
fn highlight_line_with_state(
    text: RopeSlice<'_>,
    syntax_hl: &mut SyntaxHighlighter<'_, '_, '_>,
    overlay_hl: &mut OverlayHighlighter<'_>,
    line_start: usize,
    line_end: usize,
    fg_color: Hsla,
    font: Font,
    default_bg: Hsla,
) -> Vec<TextRun> {
    let mut runs = vec![];
    let line_slice = text.slice(line_start..line_end);

    let mut position = line_start;
    while position < line_end {
        while position >= syntax_hl.pos {
            syntax_hl.advance();
        }
        while position >= overlay_hl.pos {
            overlay_hl.advance();
        }

        let next_pos = std::cmp::min(std::cmp::min(syntax_hl.pos, overlay_hl.pos), line_end);
        let char_len = next_pos - position;
        if char_len == 0 {
            break;
        }

        let run_start_in_line = position - line_start;
        let run_end_in_line = next_pos - line_start;
        let run_slice = line_slice.slice(run_start_in_line..run_end_in_line);
        let byte_len = run_slice.len_bytes();

        let style = syntax_hl.style.patch(overlay_hl.style);
        let fg = style.fg.and_then(helix_color_to_hsla).unwrap_or(fg_color);
        let bg = style.bg.and_then(helix_color_to_hsla);
        let underline = style.underline_color.and_then(helix_color_to_hsla);

        runs.push(create_styled_text_run(
            byte_len, &font, &style, fg, bg, default_bg, underline,
        ));
        position = next_pos;
    }

    runs
}

fn safe_highlight(theme: &Theme, highlight: syntax::Highlight) -> Style {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    catch_unwind(AssertUnwindSafe(|| theme.highlight(highlight))).unwrap_or_default()
}

struct SyntaxHighlighter<'h, 'r, 't> {
    inner: Option<syntax::Highlighter<'h>>,
    text: RopeSlice<'r>,
    pos: usize,
    theme: &'t Theme,
    text_style: Style,
    style: Style,
}

impl<'h, 'r, 't> SyntaxHighlighter<'h, 'r, 't> {
    fn new(
        inner: Option<syntax::Highlighter<'h>>,
        text: RopeSlice<'r>,
        theme: &'t Theme,
        text_style: Style,
    ) -> Self {
        let mut highlighter = Self {
            inner,
            text,
            pos: 0,
            theme,
            style: text_style,
            text_style,
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self
            .inner
            .as_ref()
            .and_then(|highlighter| {
                let next_byte_idx = highlighter.next_event_offset();
                (next_byte_idx != u32::MAX).then(|| {
                    self.text
                        .byte_to_char(self.text.ceil_char_boundary(next_byte_idx as usize))
                })
            })
            .unwrap_or(usize::MAX);
    }

    fn advance(&mut self) {
        let Some(highlighter) = self.inner.as_mut() else {
            return;
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (event, highlights) = highlighter.advance();
            let mut collected = Vec::new();
            for highlight in highlights {
                collected.push(highlight);
            }
            (event, collected)
        }));

        if let Ok((event, collected)) = result {
            let base = match event {
                HighlightEvent::Refresh => self.text_style,
                HighlightEvent::Push => self.style,
            };

            self.style = collected.into_iter().fold(base, |acc, highlight| {
                let highlight_style = safe_highlight(self.theme, highlight);
                let patched = acc.patch(highlight_style);
                if patched != acc {
                    trace!(
                        "Applying highlight: {:?} -> style: {:?}",
                        highlight, patched.fg
                    );
                }
                patched
            });
            self.update_pos();
        } else {
            self.inner = None;
            self.pos = usize::MAX;
        }
    }
}

struct OverlayHighlighter<'t> {
    inner: syntax::OverlayHighlighter,
    pos: usize,
    theme: &'t Theme,
    style: Style,
}

impl<'t> OverlayHighlighter<'t> {
    fn new(overlays: Vec<OverlayHighlights>, theme: &'t Theme) -> Self {
        let inner = syntax::OverlayHighlighter::new(overlays);
        let mut highlighter = Self {
            inner,
            pos: 0,
            theme,
            style: Style::default(),
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self.inner.next_event_offset();
    }

    fn advance(&mut self) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (event, highlights) = self.inner.advance();
            let mut collected = Vec::new();
            for highlight in highlights {
                collected.push(highlight);
            }
            (event, collected)
        }));

        if let Ok((event, collected)) = result {
            let base = match event {
                HighlightEvent::Refresh => Style::default(),
                HighlightEvent::Push => self.style,
            };

            self.style = collected.into_iter().fold(base, |acc, highlight| {
                let highlight_style = safe_highlight(self.theme, highlight);
                acc.patch(highlight_style)
            });
            self.update_pos();
        } else {
            self.pos = usize::MAX;
        }
    }
}

#[cfg(test)]
mod tests {
    use helix_view::view::ViewPosition;

    use super::*;

    #[test]
    fn syntax_window_uses_supplied_view_position_anchor() {
        let text = "one\ntwo\nthree\n";
        let (anchor, height) = syntax_highlight_window(
            text.into(),
            ViewPosition {
                anchor: text.find("three").unwrap(),
                vertical_offset: 0,
                horizontal_offset: 0,
            },
        );

        assert_eq!(anchor, text.find("three").unwrap());
        assert_eq!(height, 2);
    }

    #[test]
    fn syntax_window_clamps_stale_view_position_anchor() {
        let text = "one\ntwo";
        let (anchor, height) = syntax_highlight_window(
            text.into(),
            ViewPosition {
                anchor: 1_000,
                vertical_offset: 0,
                horizontal_offset: 0,
            },
        );

        assert_eq!(anchor, text.chars().count());
        assert_eq!(height, 1);
    }
}
