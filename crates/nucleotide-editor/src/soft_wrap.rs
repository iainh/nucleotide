// ABOUTME: Soft-wrap viewport collection for native editor rendering
// ABOUTME: Converts Helix formatter graphemes into owned visual-line records

use std::collections::BTreeMap;

use gpui::{Bounds, Font, Hsla, Pixels, Point, SharedString, TextRun, point, px, size};
use helix_core::{
    RopeSlice,
    doc_formatter::{DocumentFormatter, FormattedGrapheme, GraphemeSource, TextFormat},
    graphemes::Grapheme,
    syntax,
    text_annotations::TextAnnotations,
    visual_offset_from_block,
};
use helix_view::{Document, Theme, view::ViewPosition};

use crate::{
    EditorSurfaceGeometry, document_text_format_for_surface,
    line_text::{
        DisplayLineTextBuilder, DisplayTextMap, DisplayWhitespace, VirtualTextRange,
        text_run_boundaries,
    },
};

pub type VirtualHighlightRange = VirtualTextRange<Option<syntax::Highlight>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftWrapVisualLine {
    pub visual_line: usize,
    pub doc_line: usize,
    pub text: SharedString,
    pub line_start_col: usize,
    pub wrap_indicator_len: usize,
    pub line_start_char: Option<usize>,
    pub line_end_char: Option<usize>,
    pub segment_char_offset: usize,
    pub text_start_byte_offset: usize,
    pub is_phantom_line: bool,
    pub display_map: DisplayTextMap,
    pub virtual_text_ranges: Vec<VirtualHighlightRange>,
    pub whitespace_ranges: Vec<VirtualTextRange<()>>,
}

impl SoftWrapVisualLine {
    pub fn relative_row(&self, vertical_offset: usize) -> usize {
        self.visual_line.saturating_sub(vertical_offset)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftWrapVisualPosition {
    pub visual_line: usize,
    pub visual_col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoftWrapLinePaintPlan<'a> {
    pub visual: &'a SoftWrapVisualLine,
    pub y_offset: Pixels,
    pub line_y: Pixels,
    pub text_origin: Point<Pixels>,
    pub cursorline_bounds: Bounds<Pixels>,
    pub is_cursor_visual_line: bool,
}

pub struct SoftWrapRenderPlanParams<'a> {
    pub text: RopeSlice<'a>,
    pub text_format: TextFormat,
    pub text_annotations: Option<TextAnnotations<'a>>,
    pub view_offset: ViewPosition,
    pub bounds: Bounds<Pixels>,
    pub gutter_columns: u16,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
    pub inline_diagnostic_virtual_rows: Option<&'a BTreeMap<usize, usize>>,
    pub whitespace: Option<DisplayWhitespace>,
}

pub struct DocumentSoftWrapRenderPlanParams<'a> {
    pub document: &'a Document,
    pub view: &'a helix_view::View,
    pub theme: Option<&'a Theme>,
    pub view_position: ViewPosition,
    pub bounds: Bounds<Pixels>,
    pub gutter_columns: u16,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
    pub minimum_columns: u16,
    pub inline_diagnostic_virtual_rows: Option<&'a BTreeMap<usize, usize>>,
}

#[derive(Debug, Clone)]
pub struct SoftWrapRenderPlan {
    pub text_format: TextFormat,
    pub view_offset: ViewPosition,
    pub gutter_columns: u16,
    pub geometry: EditorSurfaceGeometry,
    pub viewport_height: usize,
    pub visual_lines: Vec<SoftWrapVisualLine>,
    pub inline_diagnostic_virtual_rows: BTreeMap<usize, usize>,
}

impl SoftWrapRenderPlan {
    pub fn line_paint_plans(
        &self,
        line_height: Pixels,
        scroll_line_offset: Pixels,
        cursor_line: usize,
    ) -> Vec<SoftWrapLinePaintPlan<'_>> {
        soft_wrap_line_paint_plans(
            &self.visual_lines,
            &self.inline_diagnostic_virtual_rows,
            self.geometry,
            line_height,
            scroll_line_offset,
            self.view_offset.vertical_offset,
            cursor_line,
        )
    }
}

pub fn document_soft_wrap_render_plan(
    params: DocumentSoftWrapRenderPlanParams<'_>,
) -> SoftWrapRenderPlan {
    let (_, text_format) = document_text_format_for_surface(
        params.document,
        params.theme,
        params.bounds,
        params.gutter_columns,
        params.cell_width,
        params.minimum_columns,
    );

    soft_wrap_render_plan(SoftWrapRenderPlanParams {
        text: params.document.text().slice(..),
        text_format,
        text_annotations: Some(params.view.text_annotations(params.document, params.theme)),
        view_offset: params.view_position,
        bounds: params.bounds,
        gutter_columns: params.gutter_columns,
        cell_width: params.cell_width,
        line_height: params.line_height,
        scroll_line_offset: params.scroll_line_offset,
        inline_diagnostic_virtual_rows: params.inline_diagnostic_virtual_rows,
        whitespace: crate::highlight::display_whitespace_for_document(params.document),
    })
}

pub fn soft_wrap_render_plan(mut params: SoftWrapRenderPlanParams<'_>) -> SoftWrapRenderPlan {
    let geometry =
        EditorSurfaceGeometry::new(params.bounds, params.gutter_columns, params.cell_width);
    let viewport_height =
        soft_wrap_viewport_height(params.bounds, params.line_height, params.scroll_line_offset);
    if params.inline_diagnostic_virtual_rows.is_some()
        && let Some(annotations) = params.text_annotations.as_mut()
    {
        // Native diagnostic rows are planned and painted separately. Retaining Helix's
        // diagnostic line annotations here would reserve the same rows a second time.
        annotations.clear_line_annotations();
    }
    let visual_lines = soft_wrap_visual_lines(
        params.text,
        &params.text_format,
        params.text_annotations.as_ref(),
        params.view_offset.anchor,
        params.view_offset.vertical_offset,
        viewport_height,
        params.whitespace,
    );

    SoftWrapRenderPlan {
        text_format: params.text_format,
        view_offset: params.view_offset,
        gutter_columns: params.gutter_columns,
        geometry,
        viewport_height,
        visual_lines,
        inline_diagnostic_virtual_rows: params
            .inline_diagnostic_virtual_rows
            .cloned()
            .unwrap_or_default(),
    }
}

pub fn soft_wrap_visual_lines(
    text: RopeSlice<'_>,
    text_format: &TextFormat,
    text_annotations: Option<&TextAnnotations<'_>>,
    anchor: usize,
    vertical_offset: usize,
    viewport_height: usize,
    whitespace: Option<DisplayWhitespace>,
) -> Vec<SoftWrapVisualLine> {
    let default_annotations = TextAnnotations::default();
    let annotations = text_annotations.unwrap_or(&default_annotations);
    let anchor_visual_row =
        visual_offset_from_block(text, anchor, anchor, text_format, annotations)
            .0
            .row;
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_format, annotations, anchor);
    let mut visual_line = 0;
    let start_visual_line = anchor_visual_row.saturating_add(vertical_offset);
    let mut current_doc_line = text.char_to_line(anchor);
    let mut pending_grapheme = None;

    while visual_line < start_visual_line {
        for grapheme in formatter.by_ref() {
            if grapheme.visual_pos.row > visual_line {
                visual_line = grapheme.visual_pos.row;
                if visual_line >= start_visual_line {
                    pending_grapheme = Some(grapheme);
                    break;
                }
            }
        }

        if visual_line < start_visual_line {
            visual_line += 1;
        }
    }

    let mut lines = Vec::new();
    let end_visual_line = start_visual_line.saturating_add(viewport_height);
    while visual_line < end_visual_line {
        let mut line_graphemes = Vec::new();

        if let Some(grapheme) = pending_grapheme.take() {
            if grapheme.visual_pos.row == visual_line {
                line_graphemes.push(grapheme);
            } else if grapheme.visual_pos.row > visual_line {
                pending_grapheme = Some(grapheme);
            }
        }

        let mut has_content = !line_graphemes.is_empty();
        for grapheme in formatter.by_ref() {
            if grapheme.visual_pos.row > visual_line {
                pending_grapheme = Some(grapheme);
                break;
            } else if grapheme.visual_pos.row == visual_line {
                line_graphemes.push(grapheme);
                has_content = true;
            }
        }

        if !has_content && line_graphemes.is_empty() && pending_grapheme.is_none() {
            break;
        }

        let visual = build_visual_line(
            text,
            text_format.tab_width,
            visual_line.saturating_sub(anchor_visual_row),
            current_doc_line,
            &line_graphemes,
            whitespace,
        );
        current_doc_line = visual.doc_line;
        lines.push(visual);

        visual_line += 1;
    }

    lines
}

pub fn soft_wrap_visual_position(
    text: RopeSlice<'_>,
    text_format: &TextFormat,
    text_annotations: Option<&TextAnnotations<'_>>,
    anchor: usize,
    char_idx: usize,
) -> Option<SoftWrapVisualPosition> {
    let default_annotations = TextAnnotations::default();
    let annotations = text_annotations.unwrap_or(&default_annotations);
    let (anchor_position, block_start) =
        visual_offset_from_block(text, anchor, anchor, text_format, annotations);
    if char_idx < block_start {
        return None;
    }

    let (position, _) = visual_offset_from_block(text, anchor, char_idx, text_format, annotations);
    if position.row < anchor_position.row {
        return None;
    }

    Some(SoftWrapVisualPosition {
        visual_line: position.row.saturating_sub(anchor_position.row),
        visual_col: position.col,
    })
}

pub fn decorate_soft_wrap_line_runs(
    mut line_runs: Vec<TextRun>,
    visual: &SoftWrapVisualLine,
    font: &Font,
    fg_color: Hsla,
    wrap_indicator_color: Option<Hsla>,
) -> Vec<TextRun> {
    if line_runs.is_empty() {
        return line_runs;
    }

    let text_len = visual.text.len();
    if text_len == 0 {
        return line_runs;
    }

    let prefix_end = visual.line_start_col.min(text_len);
    let wrap_start = prefix_end;
    let wrap_end = wrap_start
        .saturating_add(visual.wrap_indicator_len)
        .min(text_len);
    if prefix_end == 0 && wrap_start == wrap_end {
        return line_runs;
    }

    let mut run_segments = Vec::with_capacity(line_runs.len());
    let mut offset = 0usize;
    for run in line_runs.drain(..) {
        let start = offset.min(text_len);
        offset = offset.saturating_add(run.len);
        let end = offset.min(text_len);
        if start < end {
            run_segments.push((start, end, run));
        }
    }

    let mut boundaries = vec![prefix_end, wrap_start, wrap_end];
    for (start, end, _) in &run_segments {
        boundaries.push(*start);
        boundaries.push(*end);
    }
    let boundaries = text_run_boundaries(visual.text.as_ref(), boundaries);

    let fallback = TextRun {
        len: 0,
        font: font.clone(),
        color: fg_color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut decorated = Vec::with_capacity(boundaries.len().saturating_sub(1));
    for window in boundaries.windows(2) {
        let start = window[0];
        let end = window[1];
        if start >= end {
            continue;
        }

        let mut run = run_segments
            .iter()
            .find(|(run_start, run_end, _)| start >= *run_start && start < *run_end)
            .map(|(_, _, run)| run.clone())
            .unwrap_or_else(|| fallback.clone());

        if start < prefix_end {
            run.color = fg_color;
            run.background_color = None;
            run.underline = None;
            run.strikethrough = None;
        } else if start >= wrap_start && start < wrap_end {
            run.color = wrap_indicator_color.unwrap_or(fg_color);
            run.background_color = None;
            run.underline = None;
            run.strikethrough = None;
        }
        run.len = end - start;
        decorated.push(run);
    }

    decorated
}

pub fn soft_wrap_viewport_height(
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    scroll_line_offset: Pixels,
) -> usize {
    if line_height <= px(0.0) {
        return 0;
    }

    let effective_height = (bounds.size.height - px(2.0)).max(px(0.0));
    let calculated_height = (effective_height / line_height) as usize;
    calculated_height + usize::from(f32::from(scroll_line_offset) > 0.0)
}

pub fn soft_wrap_line_paint_plans<'a>(
    visual_lines: &'a [SoftWrapVisualLine],
    inline_diagnostic_virtual_rows: &BTreeMap<usize, usize>,
    geometry: EditorSurfaceGeometry,
    line_height: Pixels,
    scroll_line_offset: Pixels,
    vertical_offset: usize,
    cursor_line: usize,
) -> Vec<SoftWrapLinePaintPlan<'a>> {
    let text_origin_x = geometry.text_origin_x();
    let mut extra_rows_before = 0usize;

    visual_lines
        .iter()
        .enumerate()
        .map(|(index, visual)| {
            let y_offset = -scroll_line_offset
                + line_height * (visual.relative_row(vertical_offset) + extra_rows_before) as f32;
            let line_y = geometry.bounds.origin.y + geometry.top_padding() + y_offset;
            let is_last_visual_for_doc_line = visual_lines
                .get(index + 1)
                .is_none_or(|next| next.doc_line != visual.doc_line);
            let diagnostic_rows_after = is_last_visual_for_doc_line
                .then(|| {
                    inline_diagnostic_virtual_rows
                        .get(&visual.doc_line)
                        .copied()
                })
                .flatten()
                .unwrap_or(0);
            extra_rows_before += diagnostic_rows_after;

            SoftWrapLinePaintPlan {
                visual,
                y_offset,
                line_y,
                text_origin: point(text_origin_x, line_y),
                cursorline_bounds: Bounds::new(
                    point(geometry.bounds.origin.x, line_y),
                    size(geometry.bounds.size.width, line_height),
                ),
                is_cursor_visual_line: visual.doc_line == cursor_line,
            }
        })
        .collect()
}

fn build_visual_line(
    text: RopeSlice<'_>,
    tab_width: u16,
    visual_line: usize,
    fallback_doc_line: usize,
    line_graphemes: &[FormattedGrapheme<'_>],
    whitespace: Option<DisplayWhitespace>,
) -> SoftWrapVisualLine {
    let doc_line = line_graphemes
        .first()
        .map_or(fallback_doc_line, |grapheme| grapheme.line_idx);
    let line_start_col = line_graphemes
        .first()
        .map_or(0, |grapheme| grapheme.visual_pos.col);
    let mut prefix_text = String::new();
    let mut wrap_indicator_len = 0;
    let mut prefix_visual_col = line_start_col;

    prefix_text.extend(std::iter::repeat_n(' ', line_start_col));
    let first_real = line_graphemes
        .iter()
        .find(|grapheme| !grapheme.is_virtual());

    for grapheme in line_graphemes {
        if grapheme.is_virtual()
            && let Grapheme::Other { g } = &grapheme.raw
            && first_real.is_none_or(|real| grapheme.char_idx <= real.char_idx)
            && grapheme.visual_pos.col <= prefix_visual_col
        {
            wrap_indicator_len += g.len();
            prefix_visual_col += helix_core::graphemes::grapheme_width(g);
            prefix_text.push_str(g);
        }
    }

    let line_start_char = first_real.map(|grapheme| grapheme.char_idx);
    let line_end_char = first_real.map(|first_grapheme| {
        let last_real = line_graphemes
            .iter()
            .rev()
            .find(|grapheme| !grapheme.is_virtual());
        let mut line_end = last_real
            .map(|grapheme| grapheme.char_idx + grapheme.doc_chars())
            .unwrap_or(first_grapheme.char_idx);

        if let Some(last_grapheme) = last_real
            && matches!(last_grapheme.raw, Grapheme::Newline)
        {
            line_end = last_grapheme.char_idx;
        }

        line_end
    });

    let line_start = text.line_to_char(doc_line);
    let line_end = if doc_line + 1 < text.len_lines() {
        text.line_to_char(doc_line + 1)
    } else {
        text.len_chars()
    };
    let segment_char_offset =
        first_real.map_or(0, |grapheme| grapheme.char_idx.saturating_sub(line_start));
    let mut builder = DisplayLineTextBuilder::with_whitespace(0, whitespace);
    builder.push_prefix(&prefix_text, tab_width);
    for grapheme in line_graphemes {
        match (&grapheme.raw, grapheme.source) {
            (Grapheme::Other { g }, GraphemeSource::VirtualText { highlight })
                if first_real.is_some_and(|real| {
                    grapheme.char_idx > real.char_idx || grapheme.visual_pos.col > prefix_visual_col
                }) =>
            {
                builder.push_virtual(g, highlight, tab_width);
            }
            (Grapheme::Tab { .. }, GraphemeSource::Document { .. }) => {
                builder.push_source_char('\t', tab_width);
            }
            (Grapheme::Other { g }, GraphemeSource::Document { .. }) => {
                for ch in g.chars() {
                    builder.push_source_char(ch, tab_width);
                }
            }
            (Grapheme::Newline, GraphemeSource::Document { .. }) => {}
            _ => {}
        }
    }
    let (display_line_text, virtual_text_ranges) = builder.finish();
    let whitespace_ranges = display_line_text.whitespace_ranges.clone();

    SoftWrapVisualLine {
        visual_line,
        doc_line,
        text: display_line_text.display,
        line_start_col,
        wrap_indicator_len,
        line_start_char,
        line_end_char,
        segment_char_offset,
        text_start_byte_offset: line_start_col + wrap_indicator_len,
        is_phantom_line: line_start >= line_end,
        display_map: display_line_text.map,
        virtual_text_ranges,
        whitespace_ranges,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, Font, Hsla, TextRun, black, blue, font, point, px, size, white};
    use helix_core::{
        Position,
        doc_formatter::{FormattedGrapheme, TextFormat},
        text_annotations::{InlineAnnotation, LineAnnotation, TextAnnotations},
    };
    use helix_view::view::ViewPosition;

    use super::{
        SoftWrapRenderPlanParams, SoftWrapVisualLine, SoftWrapVisualPosition,
        decorate_soft_wrap_line_runs, soft_wrap_line_paint_plans, soft_wrap_render_plan,
        soft_wrap_viewport_height, soft_wrap_visual_lines, soft_wrap_visual_position,
    };
    use crate::{EditorSurfaceGeometry, line_text::DisplayTextMap};

    struct TwoVirtualRows;

    impl LineAnnotation for TwoVirtualRows {
        fn process_anchor(&mut self, _grapheme: &FormattedGrapheme) -> usize {
            usize::MAX
        }

        fn insert_virtual_lines(
            &mut self,
            _line_end_char_idx: usize,
            _line_end_visual_pos: Position,
            _doc_line: usize,
        ) -> Position {
            Position::new(2, 0)
        }
    }

    fn text_format() -> TextFormat {
        TextFormat {
            soft_wrap: true,
            tab_width: 2,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: ".".into(),
            wrap_indicator_highlight: None,
            viewport_width: 17,
            soft_wrap_at_text_width: false,
        }
    }

    fn visual_line(line_start_col: usize, wrap_indicator_len: usize) -> SoftWrapVisualLine {
        SoftWrapVisualLine {
            visual_line: 1,
            doc_line: 0,
            text: "    .wrapped".into(),
            line_start_col,
            wrap_indicator_len,
            line_start_char: Some(4),
            line_end_char: Some(11),
            segment_char_offset: 4,
            text_start_byte_offset: line_start_col + wrap_indicator_len,
            is_phantom_line: false,
            display_map: DisplayTextMap::identity("    .wrapped".len()),
            virtual_text_ranges: Vec::new(),
            whitespace_ranges: Vec::new(),
        }
    }

    fn run(len: usize, color: Hsla) -> TextRun {
        TextRun {
            len,
            font: test_font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    fn run_boundaries_are_utf8_safe(text: &str, runs: &[TextRun]) -> bool {
        let mut offset = 0usize;
        for run in runs {
            offset += run.len;
            if !text.is_char_boundary(offset) {
                return false;
            }
        }
        offset == text.len()
    }

    fn test_font() -> Font {
        font("TestFont")
    }

    fn geometry() -> EditorSurfaceGeometry {
        EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        )
    }

    #[test]
    fn collects_wrapped_visual_lines_with_metadata() {
        let text = "foo ".repeat(10);
        let lines =
            soft_wrap_visual_lines(text.as_str().into(), &text_format(), None, 0, 0, 3, None);

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].visual_line, 0);
        assert_eq!(lines[0].doc_line, 0);
        assert_eq!(lines[0].text, "foo foo foo foo ");
        assert_eq!(lines[0].line_start_char, Some(0));
        assert_eq!(lines[0].line_end_char, Some(16));
        assert_eq!(lines[0].wrap_indicator_len, 0);
        assert_eq!(lines[0].segment_char_offset, 0);
        assert_eq!(lines[0].text_start_byte_offset, 0);

        assert_eq!(lines[1].visual_line, 1);
        assert_eq!(lines[1].doc_line, 0);
        assert_eq!(lines[1].text, ".foo foo foo foo ");
        assert_eq!(lines[1].line_start_char, Some(16));
        assert_eq!(lines[1].line_end_char, Some(32));
        assert_eq!(lines[1].wrap_indicator_len, 1);
        assert_eq!(lines[1].segment_char_offset, 16);
        assert_eq!(lines[1].text_start_byte_offset, 1);

        assert_eq!(lines[2].visual_line, 2);
        assert_eq!(lines[2].text, ".foo foo  ");
        assert_eq!(lines[2].line_start_char, Some(32));
        assert_eq!(lines[2].line_end_char, Some(40));
        assert_eq!(lines[2].segment_char_offset, 32);
    }

    #[test]
    fn visual_lines_preserve_inline_annotations() {
        let text = "ab";
        let annotations = [InlineAnnotation::new(1, ": hint")];
        let mut text_annotations = TextAnnotations::default();
        text_annotations.add_inline_annotations(&annotations, None);

        let lines = soft_wrap_visual_lines(
            text.into(),
            &text_format(),
            Some(&text_annotations),
            0,
            0,
            1,
            None,
        );

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text.to_string(), "a: hintb ");
        assert_eq!(lines[0].line_start_char, Some(0));
        assert_eq!(lines[0].line_end_char, Some(2));
        assert_eq!(lines[0].virtual_text_ranges.len(), 1);
        assert_eq!(lines[0].virtual_text_ranges[0].display_start, 1);
        assert_eq!(lines[0].virtual_text_ranges[0].display_len, ": hint".len());
    }

    #[test]
    fn starts_at_viewport_vertical_offset() {
        let text = "foo ".repeat(10);
        let lines =
            soft_wrap_visual_lines(text.as_str().into(), &text_format(), None, 0, 1, 1, None);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].visual_line, 1);
        assert_eq!(lines[0].relative_row(1), 0);
        assert_eq!(lines[0].text, ".foo foo foo foo ");
    }

    #[test]
    fn starts_at_anchor_visual_row() {
        let text = "foo ".repeat(10);
        let lines =
            soft_wrap_visual_lines(text.as_str().into(), &text_format(), None, 16, 0, 1, None);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].visual_line, 0);
        assert_eq!(lines[0].relative_row(0), 0);
        assert_eq!(lines[0].text, ".foo foo foo foo ");
        assert_eq!(lines[0].line_start_char, Some(16));
    }

    #[test]
    fn expands_tabs_in_visual_line_text() {
        let lines = soft_wrap_visual_lines(
            "a\tb".into(),
            &TextFormat {
                soft_wrap: true,
                tab_width: 4,
                viewport_width: 20,
                ..TextFormat::default()
            },
            None,
            0,
            0,
            1,
            None,
        );

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "a   b ");
        assert_eq!(lines[0].display_map.display_byte_for_source_byte(2), 4);
        assert_eq!(lines[0].display_map.source_byte_for_display_byte(3), 1);
    }

    #[test]
    fn finds_visual_position_after_wrap_indicator() {
        let text = "foo ".repeat(10);
        let position = soft_wrap_visual_position(text.as_str().into(), &text_format(), None, 0, 16)
            .expect("cursor position");

        assert_eq!(position.visual_line, 1);
        assert_eq!(position.visual_col, 1);
    }

    #[test]
    fn visual_position_is_relative_to_anchor_row() {
        let text = "foo ".repeat(10);
        let position =
            soft_wrap_visual_position(text.as_str().into(), &text_format(), None, 16, 16)
                .expect("cursor position");

        assert_eq!(position.visual_line, 0);
        assert_eq!(position.visual_col, 1);
    }

    #[test]
    fn visual_position_rejects_position_before_anchor_row() {
        let text = "foo ".repeat(10);

        assert!(
            soft_wrap_visual_position(text.as_str().into(), &text_format(), None, 16, 0).is_none()
        );
    }

    #[test]
    fn visual_position_counts_inline_annotations_before_cursor() {
        let text = "ab";
        let annotations = [InlineAnnotation::new(1, ": hint")];
        let mut text_annotations = TextAnnotations::default();
        text_annotations.add_inline_annotations(&annotations, None);

        let position =
            soft_wrap_visual_position(text.into(), &text_format(), Some(&text_annotations), 0, 2)
                .expect("cursor position");

        assert_eq!(
            position,
            SoftWrapVisualPosition {
                visual_line: 0,
                visual_col: "a: hintb".len(),
            }
        );
    }

    #[test]
    fn visual_position_uses_final_empty_line_at_trailing_newline_eof() {
        let text = "one\n";
        let position = soft_wrap_visual_position(
            text.into(),
            &TextFormat {
                soft_wrap: true,
                viewport_width: 20,
                ..TextFormat::default()
            },
            None,
            0,
            text.chars().count(),
        )
        .expect("cursor position");

        assert_eq!(
            position,
            SoftWrapVisualPosition {
                visual_line: 1,
                visual_col: 0,
            }
        );
    }

    #[test]
    fn visual_position_uses_end_column_at_eof_without_trailing_newline() {
        let text = "one";
        let position = soft_wrap_visual_position(
            text.into(),
            &TextFormat {
                soft_wrap: true,
                viewport_width: 20,
                ..TextFormat::default()
            },
            None,
            0,
            text.chars().count(),
        )
        .expect("cursor position");

        assert_eq!(
            position,
            SoftWrapVisualPosition {
                visual_line: 0,
                visual_col: 3,
            }
        );
    }

    #[test]
    fn decorated_runs_prefix_indent_and_wrap_indicator() {
        let visual = visual_line(4, 1);
        let runs = decorate_soft_wrap_line_runs(
            vec![run(visual.text.len(), white())],
            &visual,
            &test_font(),
            black(),
            Some(blue()),
        );

        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].len, 4);
        assert_eq!(runs[0].color, black());
        assert_eq!(runs[1].len, 1);
        assert_eq!(runs[1].color, blue());
        assert_eq!(runs[2].len, 7);
        assert_eq!(runs[2].color, white());
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            visual.text.len()
        );
    }

    #[test]
    fn decorated_runs_prefix_wrap_indicator_without_indent() {
        let visual = visual_line(0, 2);
        let runs = decorate_soft_wrap_line_runs(
            vec![run(visual.text.len(), white())],
            &visual,
            &test_font(),
            black(),
            Some(blue()),
        );

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, 2);
        assert_eq!(runs[0].color, blue());
        assert_eq!(runs[1].len, visual.text.len() - 2);
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            visual.text.len()
        );
    }

    #[test]
    fn decorated_runs_do_not_split_multibyte_wrap_indicator() {
        let text = "↪wrapped";
        let visual = SoftWrapVisualLine {
            visual_line: 1,
            doc_line: 0,
            text: text.into(),
            line_start_col: 0,
            wrap_indicator_len: "↪".len(),
            line_start_char: Some(0),
            line_end_char: Some(7),
            segment_char_offset: 0,
            text_start_byte_offset: "↪".len(),
            is_phantom_line: false,
            display_map: DisplayTextMap::identity(text.len()),
            virtual_text_ranges: Vec::new(),
            whitespace_ranges: Vec::new(),
        };
        let decorated = decorate_soft_wrap_line_runs(
            vec![run(2, white()), run(text.len() - 2, black())],
            &visual,
            &test_font(),
            black(),
            Some(blue()),
        );

        assert!(run_boundaries_are_utf8_safe(text, &decorated));
    }

    #[test]
    fn decorated_runs_do_not_add_prefix_bytes_twice() {
        let visual = SoftWrapVisualLine {
            visual_line: 1,
            doc_line: 0,
            text: "                 wrapped".into(),
            line_start_col: 17,
            wrap_indicator_len: 0,
            line_start_char: Some(17),
            line_end_char: Some(24),
            segment_char_offset: 17,
            text_start_byte_offset: 17,
            is_phantom_line: false,
            display_map: DisplayTextMap::identity("                 wrapped".len()),
            virtual_text_ranges: Vec::new(),
            whitespace_ranges: Vec::new(),
        };
        let runs = decorate_soft_wrap_line_runs(
            vec![run(visual.text.len(), white())],
            &visual,
            &test_font(),
            black(),
            Some(blue()),
        );

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, 17);
        assert_eq!(runs[0].color, black());
        assert_eq!(runs[1].len, "wrapped".len());
        assert_eq!(runs[1].color, white());
        assert_eq!(
            runs.iter().map(|run| run.len).sum::<usize>(),
            visual.text.len()
        );
    }

    #[test]
    fn decorated_runs_leave_empty_highlights_unchanged() {
        let runs = decorate_soft_wrap_line_runs(
            Vec::new(),
            &visual_line(4, 1),
            &test_font(),
            black(),
            Some(blue()),
        );

        assert!(runs.is_empty());
    }

    #[test]
    fn viewport_height_accounts_for_padding_and_partial_scroll() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(102.0)));

        assert_eq!(soft_wrap_viewport_height(bounds, px(20.0), px(0.0)), 5);
        assert_eq!(soft_wrap_viewport_height(bounds, px(20.0), px(3.0)), 6);
        assert_eq!(soft_wrap_viewport_height(bounds, px(0.0), px(3.0)), 0);
    }

    #[test]
    fn line_paint_plans_use_text_origin_and_row_offsets() {
        let lines = vec![
            visual_line(0, 0),
            SoftWrapVisualLine {
                visual_line: 2,
                doc_line: 4,
                text: "next".into(),
                line_start_col: 0,
                wrap_indicator_len: 0,
                line_start_char: Some(12),
                line_end_char: Some(16),
                segment_char_offset: 12,
                text_start_byte_offset: 0,
                is_phantom_line: false,
                display_map: DisplayTextMap::identity("next".len()),
                virtual_text_ranges: Vec::new(),
                whitespace_ranges: Vec::new(),
            },
        ];

        let virtual_rows = std::collections::BTreeMap::new();
        let plans =
            soft_wrap_line_paint_plans(&lines, &virtual_rows, geometry(), px(20.0), px(5.0), 1, 4);

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].y_offset, px(-5.0));
        assert_eq!(plans[0].line_y, px(36.0));
        assert_eq!(plans[0].text_origin, point(px(132.0), px(36.0)));
        assert_eq!(
            plans[0].cursorline_bounds,
            Bounds::new(point(px(100.0), px(36.0)), size(px(500.0), px(20.0)))
        );
        assert!(!plans[0].is_cursor_visual_line);

        assert_eq!(plans[1].y_offset, px(15.0));
        assert_eq!(plans[1].line_y, px(56.0));
        assert_eq!(plans[1].text_origin, point(px(132.0), px(56.0)));
        assert!(plans[1].is_cursor_visual_line);
    }

    #[test]
    fn line_paint_plans_reserve_diagnostics_after_last_visual_line_for_document_line() {
        let lines = vec![
            SoftWrapVisualLine {
                visual_line: 0,
                doc_line: 0,
                text: "first".into(),
                line_start_col: 0,
                wrap_indicator_len: 0,
                line_start_char: Some(0),
                line_end_char: Some(5),
                segment_char_offset: 0,
                text_start_byte_offset: 0,
                is_phantom_line: false,
                display_map: DisplayTextMap::identity("first".len()),
                virtual_text_ranges: Vec::new(),
                whitespace_ranges: Vec::new(),
            },
            SoftWrapVisualLine {
                visual_line: 1,
                doc_line: 0,
                text: ".wrap".into(),
                line_start_col: 0,
                wrap_indicator_len: 1,
                line_start_char: Some(5),
                line_end_char: Some(9),
                segment_char_offset: 5,
                text_start_byte_offset: 1,
                is_phantom_line: false,
                display_map: DisplayTextMap::identity(".wrap".len()),
                virtual_text_ranges: Vec::new(),
                whitespace_ranges: Vec::new(),
            },
            SoftWrapVisualLine {
                visual_line: 2,
                doc_line: 1,
                text: "next".into(),
                line_start_col: 0,
                wrap_indicator_len: 0,
                line_start_char: Some(10),
                line_end_char: Some(14),
                segment_char_offset: 0,
                text_start_byte_offset: 0,
                is_phantom_line: false,
                display_map: DisplayTextMap::identity("next".len()),
                virtual_text_ranges: Vec::new(),
                whitespace_ranges: Vec::new(),
            },
        ];
        let mut virtual_rows = std::collections::BTreeMap::new();
        virtual_rows.insert(0, 2);

        let plans =
            soft_wrap_line_paint_plans(&lines, &virtual_rows, geometry(), px(20.0), px(0.0), 0, 0);

        assert_eq!(plans[0].y_offset, px(0.0));
        assert_eq!(plans[1].y_offset, px(20.0));
        assert_eq!(plans[2].y_offset, px(80.0));
    }

    #[test]
    fn render_plan_collects_viewport_geometry_and_lines() {
        let text = "foo ".repeat(10);
        let bounds = Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(102.0)));
        let view_offset = ViewPosition {
            anchor: 0,
            vertical_offset: 1,
            horizontal_offset: 3,
        };

        let plan = soft_wrap_render_plan(SoftWrapRenderPlanParams {
            text: text.as_str().into(),
            text_format: text_format(),
            text_annotations: None,
            view_offset,
            bounds,
            gutter_columns: 4,
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(5.0),
            inline_diagnostic_virtual_rows: None,
            whitespace: None,
        });

        assert_eq!(plan.view_offset, view_offset);
        assert_eq!(plan.gutter_columns, 4);
        assert_eq!(
            plan.geometry,
            EditorSurfaceGeometry::new(bounds, 4, px(8.0))
        );
        assert_eq!(plan.viewport_height, 6);
        assert_eq!(plan.visual_lines.len(), 2);
        assert_eq!(plan.visual_lines[0].visual_line, 1);

        let paint_plans = plan.line_paint_plans(px(20.0), px(5.0), 0);

        assert_eq!(paint_plans.len(), 2);
        assert_eq!(paint_plans[0].visual.visual_line, 1);
        assert_eq!(paint_plans[0].y_offset, px(-5.0));
        assert_eq!(paint_plans[0].text_origin, point(px(132.0), px(36.0)));
        assert!(paint_plans[0].is_cursor_visual_line);
    }

    #[test]
    fn render_plan_does_not_double_reserve_native_diagnostic_rows() {
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(Box::new(TwoVirtualRows));
        let native_diagnostic_rows = std::collections::BTreeMap::new();

        let plan = soft_wrap_render_plan(SoftWrapRenderPlanParams {
            text: "one\ntwo".into(),
            text_format: TextFormat {
                soft_wrap: true,
                viewport_width: 20,
                ..TextFormat::default()
            },
            text_annotations: Some(annotations),
            view_offset: ViewPosition::default(),
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(100.0))),
            gutter_columns: 0,
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(0.0),
            inline_diagnostic_virtual_rows: Some(&native_diagnostic_rows),
            whitespace: None,
        });

        assert_eq!(
            plan.visual_lines
                .iter()
                .map(|line| line.doc_line)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }
}
