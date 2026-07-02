// ABOUTME: Native editor highlighting helpers for GPUI text rendering
// ABOUTME: Converts Helix syntax, selection, and diagnostic styles into TextRuns

use std::ops::Range;

use gpui::{Font, Hsla, TextRun};
use helix_core::{
    RopeSlice,
    doc_formatter::{DocumentFormatter, GraphemeSource, TextFormat},
    graphemes::Grapheme,
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    syntax::{self, HighlightEvent, OverlayHighlights},
};
use helix_stdx::rope::RopeSliceExt;
use helix_view::{
    Document, Theme, View,
    document::Mode,
    editor::WhitespaceRenderValue,
    graphics::{Color, CursorKind, Style},
    view::ViewPosition,
};
use nucleotide_logging::trace;

use crate::{
    line_plan::VisibleLinePlan,
    line_text::{
        DisplayLineText, DisplayLineTextBuilder, DisplayWhitespace, VirtualTextRange,
        expand_text_runs_for_display, line_text_without_trailing_newline, text_run_boundaries,
    },
    soft_wrap::{SoftWrapVisualLine, VirtualHighlightRange, decorate_soft_wrap_line_runs},
    style::{create_styled_text_run, helix_color_to_hsla},
};

pub type DiagnosticOverlaySpans = Vec<(syntax::Highlight, Vec<Range<usize>>)>;

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
    pub tab_width: u16,
    pub display_whitespace: Option<DisplayWhitespace>,
    pub whitespace_style: Style,
}

pub struct SoftWrapHighlightedLineRunsParams<'a> {
    pub context: EditorLineHighlightContext<'a>,
    pub visual: &'a SoftWrapVisualLine,
    pub wrap_indicator_color: Option<Hsla>,
}

pub struct SoftWrapHighlightedLineRunsBatchParams<'a> {
    pub context: EditorLineHighlightContext<'a>,
    pub visual_lines: &'a [SoftWrapVisualLine],
    pub wrap_indicator_color: Option<Hsla>,
}

pub struct UnwrappedHighlightedLineParams<'a> {
    pub context: EditorLineHighlightContext<'a>,
    pub text: RopeSlice<'a>,
    pub line: &'a VisibleLinePlan,
}

pub struct UnwrappedHighlightedLinesParams<'a> {
    pub context: EditorLineHighlightContext<'a>,
    pub text: RopeSlice<'a>,
    pub lines: &'a [VisibleLinePlan],
}

#[derive(Debug, Clone)]
pub struct UnwrappedHighlightedLine {
    pub line_text: DisplayLineText,
    pub line_runs: Vec<TextRun>,
}

pub fn diagnostic_overlay_spans(doc: &Document, theme: &Theme) -> Option<DiagnosticOverlaySpans> {
    use helix_core::diagnostic::{DiagnosticTag, Severity};

    let get_scope_of = |scope| {
        theme
            .find_highlight_exact(scope)
            .or_else(|| theme.find_highlight_exact("diagnostic"))
            .or_else(|| theme.find_highlight_exact("ui.cursor"))
            .or_else(|| theme.find_highlight_exact("ui.selection"))
    };

    let diagnostics = doc.diagnostics();
    if diagnostics.is_empty() {
        return None;
    }

    let unnecessary = theme.find_highlight_exact("diagnostic.unnecessary");
    let deprecated = theme.find_highlight_exact("diagnostic.deprecated");

    let mut default_vec = Vec::new();
    let mut info_vec = Vec::new();
    let mut hint_vec = Vec::new();
    let mut warning_vec = Vec::new();
    let mut error_vec = Vec::new();
    let mut unnecessary_vec = Vec::new();
    let mut deprecated_vec = Vec::new();

    let push_diagnostic = |ranges: &mut Vec<Range<usize>>, range: Range<usize>| {
        let range = if range.start >= range.end {
            range.start..range.start.saturating_add(1)
        } else {
            range
        };

        match ranges.last_mut() {
            Some(existing) if range.start <= existing.end => {
                debug_assert!(existing.start <= range.start);
                existing.end = existing.end.max(range.end);
            }
            _ => ranges.push(range),
        }
    };

    for diagnostic in diagnostics.iter() {
        let ranges = match diagnostic.severity {
            Some(Severity::Info) => &mut info_vec,
            Some(Severity::Hint) => &mut hint_vec,
            Some(Severity::Warning) => &mut warning_vec,
            Some(Severity::Error) => &mut error_vec,
            _ => &mut default_vec,
        };

        if diagnostic.tags.is_empty()
            || matches!(
                diagnostic.severity,
                Some(Severity::Warning | Severity::Error)
            )
        {
            push_diagnostic(ranges, diagnostic.range.start..diagnostic.range.end);
        }

        for tag in &diagnostic.tags {
            match tag {
                DiagnosticTag::Unnecessary => {
                    if unnecessary.is_some() {
                        push_diagnostic(
                            &mut unnecessary_vec,
                            diagnostic.range.start..diagnostic.range.end,
                        );
                    }
                }
                DiagnosticTag::Deprecated => {
                    if deprecated.is_some() {
                        push_diagnostic(
                            &mut deprecated_vec,
                            diagnostic.range.start..diagnostic.range.end,
                        );
                    }
                }
            }
        }
    }

    let mut overlays = Vec::new();
    if let Some(highlight) = get_scope_of("diagnostic") {
        push_diagnostic_overlay(&mut overlays, highlight, default_vec);
    }
    if let Some(highlight) = unnecessary {
        push_diagnostic_overlay(&mut overlays, highlight, unnecessary_vec);
    }
    if let Some(highlight) = deprecated {
        push_diagnostic_overlay(&mut overlays, highlight, deprecated_vec);
    }
    for (scope, ranges) in [
        ("diagnostic.info", info_vec),
        ("diagnostic.hint", hint_vec),
        ("diagnostic.warning", warning_vec),
        ("diagnostic.error", error_vec),
    ] {
        if let Some(highlight) = get_scope_of(scope) {
            push_diagnostic_overlay(&mut overlays, highlight, ranges);
        }
    }

    (!overlays.is_empty()).then_some(overlays)
}

fn push_diagnostic_overlay(
    overlays: &mut DiagnosticOverlaySpans,
    highlight: syntax::Highlight,
    ranges: Vec<Range<usize>>,
) {
    if !ranges.is_empty() {
        overlays.push((highlight, ranges));
    }
}

fn diagnostic_overlay_highlights(
    overlays: &DiagnosticOverlaySpans,
) -> impl Iterator<Item = OverlayHighlights> + '_ {
    overlays
        .iter()
        .map(|(highlight, ranges)| OverlayHighlights::Homogeneous {
            highlight: *highlight,
            ranges: ranges.clone(),
        })
}

pub(crate) fn display_whitespace_for_document(doc: &Document) -> Option<DisplayWhitespace> {
    let config = doc.config.load();
    let render = &config.whitespace.render;
    let chars = &config.whitespace.characters;
    let whitespace = DisplayWhitespace {
        space: (render.space() == WhitespaceRenderValue::All).then_some(chars.space),
        nbsp: (render.nbsp() == WhitespaceRenderValue::All).then_some(chars.nbsp),
        nnbsp: (render.nnbsp() == WhitespaceRenderValue::All).then_some(chars.nnbsp),
        tab: (render.tab() == WhitespaceRenderValue::All).then_some((chars.tab, chars.tabpad)),
    };

    (whitespace.space.is_some()
        || whitespace.nbsp.is_some()
        || whitespace.nnbsp.is_some()
        || whitespace.tab.is_some())
    .then_some(whitespace)
}

struct DocumentOverlayHighlightParams<'a> {
    doc: &'a Document,
    view: &'a View,
    theme: &'a Theme,
    syntax_loader: &'a helix_core::syntax::Loader,
    is_view_focused: bool,
    visible_range: Range<usize>,
    diagnostic_overlay_spans: Option<&'a DiagnosticOverlaySpans>,
}

fn document_overlay_highlights(
    params: DocumentOverlayHighlightParams<'_>,
) -> Vec<OverlayHighlights> {
    let mut overlays = Vec::new();
    let text_annotations = params.view.text_annotations(params.doc, Some(params.theme));
    overlays.push(text_annotations.collect_overlay_highlights(params.visible_range.clone()));

    if let Some(rainbow) = rainbow_overlay_highlights(
        params.doc,
        params.theme,
        params.syntax_loader,
        params.visible_range.clone(),
    ) {
        overlays.push(rainbow);
    }

    if let Some(highlights) = params.diagnostic_overlay_spans {
        overlays.extend(diagnostic_overlay_highlights(highlights));
    }

    if params.is_view_focused
        && let Some(tabstops) = tabstop_highlights(params.doc, params.theme)
    {
        overlays.push(tabstops);
    }

    overlays
}

fn rainbow_overlay_highlights(
    doc: &Document,
    theme: &Theme,
    loader: &helix_core::syntax::Loader,
    visible_range: Range<usize>,
) -> Option<OverlayHighlights> {
    let editor_config = doc.config.load();
    let enabled = doc
        .language_config()
        .and_then(|config| config.rainbow_brackets)
        .unwrap_or(editor_config.rainbow_brackets);
    if !enabled || visible_range.is_empty() {
        return None;
    }

    let syntax = doc.syntax()?;
    let text = doc.text().slice(..);
    let start_byte = text.char_to_byte(visible_range.start.min(text.len_chars()));
    let end_byte = text.char_to_byte(visible_range.end.min(text.len_chars()));
    if start_byte >= end_byte {
        return None;
    }

    let start = syntax::child_for_byte_range(
        &syntax.tree().root_node(),
        start_byte as u32..end_byte as u32,
    )
    .map_or(start_byte as u32, |node| node.start_byte());

    Some(syntax.rainbow_highlights(text, theme.rainbow_length(), loader, start..end_byte as u32))
}

fn matching_bracket_highlight(
    view: &View,
    doc: &Document,
    theme: &Theme,
) -> Option<OverlayHighlights> {
    let syntax = doc.syntax()?;
    let highlight = theme.find_highlight_exact("ui.cursor.match")?;
    let text = doc.text().slice(..);
    let pos = doc.selection(view.id).primary().cursor(text);
    let pos = helix_core::match_brackets::find_matching_bracket(syntax, text, pos)?;
    Some(OverlayHighlights::single(highlight, pos..pos + 1))
}

fn tabstop_highlights(doc: &Document, theme: &Theme) -> Option<OverlayHighlights> {
    let snippet = doc.active_snippet.as_ref()?;
    let highlight = theme.find_highlight_exact("tabstop")?;
    let mut ranges = Vec::new();
    for tabstop in snippet.tabstops() {
        ranges.extend(tabstop.ranges.iter().map(|range| range.start..range.end));
    }
    Some(OverlayHighlights::Homogeneous { highlight, ranges })
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
    let mut overlays = document_overlay_highlights(DocumentOverlayHighlightParams {
        doc: params.doc,
        view: params.view,
        theme: params.theme,
        syntax_loader: params.syntax_loader,
        is_view_focused: params.is_view_focused,
        visible_range: params.line_start..params.line_end,
        diagnostic_overlay_spans: params.diagnostic_overlay_spans,
    });
    overlays.push(selection_overlay);
    if params.is_view_focused
        && let Some(matching_bracket) =
            matching_bracket_highlight(params.view, params.doc, params.theme)
    {
        overlays.push(matching_bracket);
    }
    let mut overlay_hl = OverlayHighlighter::new(overlays, params.theme);

    highlight_line_with_state(
        text,
        &mut syntax_hl,
        &mut overlay_hl,
        params.line_start,
        params.line_end,
        params.fg_color,
        &params.font,
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
    line_runs = expand_text_runs_for_display(&line_runs, &params.visual.display_map);
    line_runs = apply_whitespace_text_runs(
        line_runs,
        params.visual.text.as_ref(),
        &params.visual.whitespace_ranges,
        &context,
    );
    line_runs = apply_virtual_text_runs(
        line_runs,
        params.visual.text.as_ref(),
        &params.visual.virtual_text_ranges,
        &context,
    );

    line_runs = decorate_soft_wrap_line_runs(
        line_runs,
        params.visual,
        &context.font,
        context.fg_color,
        params.wrap_indicator_color,
    );

    line_runs
}

pub fn soft_wrap_highlighted_line_runs_batch(
    params: SoftWrapHighlightedLineRunsBatchParams<'_>,
) -> Vec<Vec<TextRun>> {
    let context = params.context;
    let text = context.doc.text().slice(..);
    let visible_range = visible_char_range(
        params
            .visual_lines
            .iter()
            .filter_map(|visual| Some(visual.line_start_char?..visual.line_end_char?)),
    );
    let mut state = FrameHighlightState::new(&context, text, visible_range);

    params
        .visual_lines
        .iter()
        .map(|visual| {
            let mut line_runs = if let (Some(line_start), Some(line_end)) =
                (visual.line_start_char, visual.line_end_char)
            {
                state.highlight_range(line_start, line_end, &context)
            } else {
                Vec::new()
            };
            line_runs = expand_text_runs_for_display(&line_runs, &visual.display_map);
            line_runs = apply_whitespace_text_runs(
                line_runs,
                visual.text.as_ref(),
                &visual.whitespace_ranges,
                &context,
            );
            line_runs = apply_virtual_text_runs(
                line_runs,
                visual.text.as_ref(),
                &visual.virtual_text_ranges,
                &context,
            );

            line_runs = decorate_soft_wrap_line_runs(
                line_runs,
                visual,
                &context.font,
                context.fg_color,
                params.wrap_indicator_color,
            );

            line_runs
        })
        .collect()
}

pub fn unwrapped_highlighted_line(
    params: UnwrappedHighlightedLineParams<'_>,
) -> UnwrappedHighlightedLine {
    let context = params.context;
    let (line_text, virtual_text_ranges) = unwrapped_display_line_text(
        params.text,
        params.line.line_start,
        params.line.line_end,
        &context,
    );
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
        font: context.font.clone(),
        default_text_style: context.default_text_style,
        default_bg: context.default_bg,
        diagnostic_overlay_spans: context.diagnostic_overlay_spans,
    });
    let line_runs = expand_text_runs_for_display(&line_runs, &line_text.map);
    let line_runs = apply_whitespace_text_runs(
        line_runs,
        line_text.display.as_ref(),
        &line_text.whitespace_ranges,
        &context,
    );
    let line_runs = apply_virtual_text_runs(
        line_runs,
        line_text.display.as_ref(),
        &virtual_text_ranges,
        &context,
    );

    UnwrappedHighlightedLine {
        line_text,
        line_runs,
    }
}

pub fn unwrapped_highlighted_lines(
    params: UnwrappedHighlightedLinesParams<'_>,
) -> Vec<UnwrappedHighlightedLine> {
    let context = params.context;
    let visible_range = visible_char_range(
        params
            .lines
            .iter()
            .map(|line| line.line_start..line.line_end),
    );
    let mut state = FrameHighlightState::new(&context, params.text, visible_range);

    params
        .lines
        .iter()
        .map(|line| {
            let (line_text, virtual_text_ranges) =
                unwrapped_display_line_text(params.text, line.line_start, line.line_end, &context);
            let line_runs = state.highlight_range(line.line_start, line.line_end, &context);
            let line_runs = expand_text_runs_for_display(&line_runs, &line_text.map);
            let line_runs = apply_whitespace_text_runs(
                line_runs,
                line_text.display.as_ref(),
                &line_text.whitespace_ranges,
                &context,
            );
            let line_runs = apply_virtual_text_runs(
                line_runs,
                line_text.display.as_ref(),
                &virtual_text_ranges,
                &context,
            );

            UnwrappedHighlightedLine {
                line_text,
                line_runs,
            }
        })
        .collect()
}

fn unwrapped_display_line_text(
    text: RopeSlice<'_>,
    line_start: usize,
    line_end: usize,
    context: &EditorLineHighlightContext<'_>,
) -> (DisplayLineText, Vec<VirtualHighlightRange>) {
    let text_format = TextFormat {
        soft_wrap: false,
        tab_width: context.tab_width,
        viewport_width: u16::MAX,
        ..TextFormat::default()
    };
    let text_annotations = context
        .view
        .text_annotations(context.doc, Some(context.theme));
    let mut formatter = DocumentFormatter::new_at_prev_checkpoint(
        text,
        &text_format,
        &text_annotations,
        line_start,
    );
    let mut builder = DisplayLineTextBuilder::with_whitespace(0, context.display_whitespace);

    for grapheme in formatter.by_ref() {
        if !grapheme.is_virtual() && grapheme.char_idx < line_start {
            continue;
        }
        if !grapheme.is_virtual() && grapheme.char_idx >= line_end {
            break;
        }

        match (&grapheme.raw, grapheme.source) {
            (Grapheme::Other { g }, GraphemeSource::VirtualText { highlight }) => {
                builder.push_virtual(g, highlight, context.tab_width);
            }
            (Grapheme::Tab { .. }, GraphemeSource::Document { codepoints }) if codepoints > 0 => {
                builder.push_source_char('\t', context.tab_width);
            }
            (Grapheme::Other { g }, GraphemeSource::Document { codepoints }) if codepoints > 0 => {
                for ch in g.chars() {
                    builder.push_source_char(ch, context.tab_width);
                }
            }
            (Grapheme::Newline, GraphemeSource::Document { .. }) => break,
            _ => {}
        }
    }

    let (line_text, virtual_text_ranges) = builder.finish();
    if line_text.source.is_empty() && line_start < line_end {
        (
            DisplayLineText::from_source_with_whitespace(
                line_text_without_trailing_newline(text.slice(line_start..line_end)),
                context.tab_width,
                context.display_whitespace,
            ),
            Vec::new(),
        )
    } else {
        (line_text, virtual_text_ranges)
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

fn syntax_highlight_window_for_char_range(
    text: RopeSlice<'_>,
    view_position: ViewPosition,
    visible_range: Option<Range<usize>>,
) -> (usize, u16) {
    let Some(visible_range) = visible_range else {
        return syntax_highlight_window(text, view_position);
    };

    let text_len = text.len_chars();
    let anchor = view_position.anchor.min(visible_range.start).min(text_len);
    let end = visible_range.end.min(text_len);
    let start_row = text.char_to_line(anchor);
    let end_row = text
        .char_to_line(end)
        .saturating_add(2)
        .min(text.len_lines());
    let height = end_row.saturating_sub(start_row).max(1);

    (anchor, u16::try_from(height).unwrap_or(u16::MAX))
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

fn visible_char_range(ranges: impl IntoIterator<Item = Range<usize>>) -> Option<Range<usize>> {
    let mut start = usize::MAX;
    let mut end = 0;
    let mut has_range = false;

    for range in ranges {
        start = start.min(range.start);
        end = end.max(range.end);
        has_range = true;
    }

    has_range.then_some(start..end)
}

fn apply_virtual_text_runs(
    runs: Vec<TextRun>,
    display_text: &str,
    virtual_ranges: &[VirtualHighlightRange],
    context: &EditorLineHighlightContext<'_>,
) -> Vec<TextRun> {
    let display_len = display_text.len();
    if virtual_ranges.is_empty() || display_len == 0 {
        return runs;
    }

    let mut run_segments = Vec::with_capacity(runs.len());
    let mut offset = 0usize;
    for run in runs {
        let start = offset;
        offset = offset.saturating_add(run.len);
        run_segments.push((start, offset, run));
    }

    let fallback = text_run_from_style(
        0,
        context.default_text_style,
        context.fg_color,
        context.default_bg,
        &context.font,
    );
    let mut boundaries = Vec::new();
    for (start, end, _) in &run_segments {
        boundaries.push((*start).min(display_len));
        boundaries.push((*end).min(display_len));
    }
    for range in virtual_ranges {
        boundaries.push(range.display_start.min(display_len));
        boundaries.push(
            range
                .display_start
                .saturating_add(range.display_len)
                .min(display_len),
        );
    }
    let boundaries = text_run_boundaries(display_text, boundaries);

    let mut merged = Vec::new();
    for window in boundaries.windows(2) {
        let start = window[0];
        let end = window[1];
        if start >= end {
            continue;
        }

        let mut run = virtual_ranges
            .iter()
            .find(|range| {
                start >= range.display_start
                    && start
                        < range
                            .display_start
                            .saturating_add(range.display_len)
                            .min(display_len)
            })
            .map(|range| {
                virtual_text_run(
                    end - start,
                    range.metadata,
                    context.theme,
                    context.default_text_style,
                    context.fg_color,
                    context.default_bg,
                    &context.font,
                )
            })
            .unwrap_or_else(|| {
                run_segments
                    .iter()
                    .find(|(run_start, run_end, _)| start >= *run_start && start < *run_end)
                    .map(|(_, _, run)| run.clone())
                    .unwrap_or_else(|| fallback.clone())
            });
        run.len = end - start;
        push_text_run(&mut merged, run);
    }

    merged
}

fn apply_whitespace_text_runs(
    runs: Vec<TextRun>,
    display_text: &str,
    whitespace_ranges: &[VirtualTextRange<()>],
    context: &EditorLineHighlightContext<'_>,
) -> Vec<TextRun> {
    let display_len = display_text.len();
    if whitespace_ranges.is_empty() || display_len == 0 {
        return runs;
    }

    let mut run_segments = Vec::with_capacity(runs.len());
    let mut offset = 0usize;
    for run in runs {
        let start = offset;
        offset = offset.saturating_add(run.len);
        run_segments.push((start, offset, run));
    }

    let mut boundaries = Vec::new();
    for (start, end, _) in &run_segments {
        boundaries.push((*start).min(display_len));
        boundaries.push((*end).min(display_len));
    }
    for range in whitespace_ranges {
        boundaries.push(range.display_start.min(display_len));
        boundaries.push(
            range
                .display_start
                .saturating_add(range.display_len)
                .min(display_len),
        );
    }
    let boundaries = text_run_boundaries(display_text, boundaries);

    let fallback = text_run_from_style(
        0,
        context.default_text_style,
        context.fg_color,
        context.default_bg,
        &context.font,
    );
    let whitespace_style = context.default_text_style.patch(context.whitespace_style);
    let mut merged = Vec::new();
    for window in boundaries.windows(2) {
        let start = window[0];
        let end = window[1];
        if start >= end {
            continue;
        }

        let mut run = if whitespace_ranges.iter().any(|range| {
            start >= range.display_start
                && start
                    < range
                        .display_start
                        .saturating_add(range.display_len)
                        .min(display_len)
        }) {
            text_run_from_style(
                end - start,
                whitespace_style,
                context.fg_color,
                context.default_bg,
                &context.font,
            )
        } else {
            run_segments
                .iter()
                .find(|(run_start, run_end, _)| start >= *run_start && start < *run_end)
                .map(|(_, _, run)| run.clone())
                .unwrap_or_else(|| fallback.clone())
        };
        run.len = end - start;
        push_text_run(&mut merged, run);
    }

    merged
}

fn virtual_text_run(
    len: usize,
    highlight: Option<syntax::Highlight>,
    theme: &Theme,
    default_style: Style,
    fg_color: Hsla,
    default_bg: Hsla,
    font: &Font,
) -> TextRun {
    let style = highlight
        .map(|highlight| default_style.patch(safe_highlight(theme, highlight)))
        .unwrap_or(default_style);
    let fg = style.fg.and_then(helix_color_to_hsla).unwrap_or(fg_color);
    let bg = style.bg.and_then(helix_color_to_hsla);
    let underline = style.underline_color.and_then(helix_color_to_hsla);

    create_styled_text_run(len, font, &style, fg, bg, default_bg, underline)
}

fn text_run_from_style(
    len: usize,
    style: Style,
    fg_color: Hsla,
    default_bg: Hsla,
    font: &Font,
) -> TextRun {
    let fg = style.fg.and_then(helix_color_to_hsla).unwrap_or(fg_color);
    let bg = style.bg.and_then(helix_color_to_hsla);
    let underline = style.underline_color.and_then(helix_color_to_hsla);

    create_styled_text_run(len, font, &style, fg, bg, default_bg, underline)
}

fn push_text_run(runs: &mut Vec<TextRun>, run: TextRun) {
    if run.len == 0 {
        return;
    }

    if let Some(last) = runs.last_mut()
        && last.font == run.font
        && last.color == run.color
        && last.background_color == run.background_color
        && last.underline == run.underline
        && last.strikethrough == run.strikethrough
    {
        last.len += run.len;
        return;
    }

    runs.push(run);
}

struct FrameHighlightState<'a> {
    text: RopeSlice<'a>,
    syntax_hl: SyntaxHighlighter<'a, 'a, 'a>,
    overlay_hl: OverlayHighlighter<'a>,
}

impl<'a> FrameHighlightState<'a> {
    fn new(
        context: &EditorLineHighlightContext<'a>,
        text: RopeSlice<'a>,
        visible_range: Option<Range<usize>>,
    ) -> Self {
        let (anchor, height) = syntax_highlight_window_for_char_range(
            text,
            context.view_position,
            visible_range.clone(),
        );
        let syntax_highlighter =
            doc_syntax_highlights(context.doc, anchor, height, context.syntax_loader);
        let mut overlays = document_overlay_highlights(DocumentOverlayHighlightParams {
            doc: context.doc,
            view: context.view,
            theme: context.theme,
            syntax_loader: context.syntax_loader,
            is_view_focused: context.is_view_focused,
            visible_range: visible_range.clone().unwrap_or(0..0),
            diagnostic_overlay_spans: context.diagnostic_overlay_spans,
        });
        overlays.push(selection_overlay_highlights(
            context.editor_mode,
            context.doc,
            context.view,
            context.theme,
            context.cursor_shape,
        ));
        if context.is_view_focused
            && let Some(matching_bracket) =
                matching_bracket_highlight(context.view, context.doc, context.theme)
        {
            overlays.push(matching_bracket);
        }

        Self {
            text,
            syntax_hl: SyntaxHighlighter::new(
                syntax_highlighter,
                text,
                context.theme,
                context.default_text_style,
            ),
            overlay_hl: OverlayHighlighter::new(overlays, context.theme),
        }
    }

    fn highlight_range(
        &mut self,
        line_start: usize,
        line_end: usize,
        context: &EditorLineHighlightContext<'_>,
    ) -> Vec<TextRun> {
        highlight_line_with_state(
            self.text,
            &mut self.syntax_hl,
            &mut self.overlay_hl,
            line_start,
            line_end,
            context.fg_color,
            &context.font,
            context.default_bg,
        )
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
    font: &Font,
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
            byte_len, font, &style, fg, bg, default_bg, underline,
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

    #[test]
    fn syntax_window_for_visible_range_limits_height() {
        let text = "one\ntwo\nthree\nfour\nfive\n";
        let visible_start = text.find("three").unwrap();
        let visible_end = visible_start + "three".chars().count();

        let (anchor, height) = syntax_highlight_window_for_char_range(
            text.into(),
            ViewPosition {
                anchor: visible_start,
                vertical_offset: 0,
                horizontal_offset: 0,
            },
            Some(visible_start..visible_end),
        );

        assert_eq!(anchor, visible_start);
        assert!(height < 5);
    }
}
