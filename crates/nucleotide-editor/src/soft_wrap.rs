// ABOUTME: Soft-wrap viewport collection for native editor rendering
// ABOUTME: Converts Helix formatter graphemes into owned visual-line records

use gpui::{Bounds, Font, Hsla, Pixels, Point, TextRun, point, px, size};
use helix_core::{
    RopeSlice,
    doc_formatter::{DocumentFormatter, FormattedGrapheme, TextFormat},
    graphemes::Grapheme,
    text_annotations::TextAnnotations,
};
use helix_view::{Document, Theme, ViewId, view::ViewPosition};

use crate::{EditorSurfaceGeometry, document_text_format_for_surface};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftWrapVisualLine {
    pub visual_line: usize,
    pub doc_line: usize,
    pub text: String,
    pub line_start_col: usize,
    pub wrap_indicator_len: usize,
    pub line_start_char: Option<usize>,
    pub line_end_char: Option<usize>,
    pub segment_char_offset: usize,
    pub text_start_byte_offset: usize,
    pub is_phantom_line: bool,
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
    pub view_offset: ViewPosition,
    pub bounds: Bounds<Pixels>,
    pub gutter_columns: u16,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
}

pub struct DocumentSoftWrapRenderPlanParams<'a> {
    pub document: &'a Document,
    pub theme: Option<&'a Theme>,
    pub view_id: ViewId,
    pub bounds: Bounds<Pixels>,
    pub gutter_columns: u16,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
    pub minimum_columns: u16,
}

#[derive(Debug, Clone)]
pub struct SoftWrapRenderPlan {
    pub text_format: TextFormat,
    pub view_offset: ViewPosition,
    pub gutter_columns: u16,
    pub geometry: EditorSurfaceGeometry,
    pub viewport_height: usize,
    pub visual_lines: Vec<SoftWrapVisualLine>,
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
        view_offset: params.document.view_offset(params.view_id),
        bounds: params.bounds,
        gutter_columns: params.gutter_columns,
        cell_width: params.cell_width,
        line_height: params.line_height,
        scroll_line_offset: params.scroll_line_offset,
    })
}

pub fn soft_wrap_render_plan(params: SoftWrapRenderPlanParams<'_>) -> SoftWrapRenderPlan {
    let geometry =
        EditorSurfaceGeometry::new(params.bounds, params.gutter_columns, params.cell_width);
    let viewport_height =
        soft_wrap_viewport_height(params.bounds, params.line_height, params.scroll_line_offset);
    let visual_lines = soft_wrap_visual_lines(
        params.text,
        &params.text_format,
        params.view_offset.anchor,
        params.view_offset.vertical_offset,
        viewport_height,
    );

    SoftWrapRenderPlan {
        text_format: params.text_format,
        view_offset: params.view_offset,
        gutter_columns: params.gutter_columns,
        geometry,
        viewport_height,
        visual_lines,
    }
}

pub fn soft_wrap_visual_lines(
    text: RopeSlice<'_>,
    text_format: &TextFormat,
    anchor: usize,
    vertical_offset: usize,
    viewport_height: usize,
) -> Vec<SoftWrapVisualLine> {
    let annotations = TextAnnotations::default();
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_format, &annotations, anchor);
    let mut visual_line = 0;
    let mut current_doc_line = text.char_to_line(anchor);
    let mut pending_grapheme = None;

    while visual_line < vertical_offset {
        for grapheme in formatter.by_ref() {
            if grapheme.visual_pos.row > visual_line {
                visual_line = grapheme.visual_pos.row;
                if visual_line >= vertical_offset {
                    pending_grapheme = Some(grapheme);
                    break;
                }
            }
        }

        if visual_line < vertical_offset {
            visual_line += 1;
        }
    }

    let mut lines = Vec::new();
    let end_visual_line = vertical_offset.saturating_add(viewport_height);
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

        let visual = build_visual_line(text, visual_line, current_doc_line, &line_graphemes);
        current_doc_line = visual.doc_line;
        lines.push(visual);

        visual_line += 1;
    }

    lines
}

pub fn soft_wrap_visual_position(
    text: RopeSlice<'_>,
    text_format: &TextFormat,
    anchor: usize,
    char_idx: usize,
) -> Option<SoftWrapVisualPosition> {
    let annotations = TextAnnotations::default();
    let formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_format, &annotations, anchor);
    let mut last_visual_line = 0;

    for grapheme in formatter {
        let next_char_pos = grapheme.char_idx + grapheme.doc_chars();
        if next_char_pos > char_idx {
            return Some(SoftWrapVisualPosition {
                visual_line: grapheme.visual_pos.row,
                visual_col: grapheme.visual_pos.col,
            });
        }
        last_visual_line = grapheme.visual_pos.row;
    }

    (char_idx >= text.len_chars()).then_some(SoftWrapVisualPosition {
        visual_line: last_visual_line,
        visual_col: 0,
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

    if visual.line_start_col > 0 {
        line_runs.insert(
            0,
            TextRun {
                len: visual.line_start_col,
                font: font.clone(),
                color: fg_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            },
        );
    }

    if visual.wrap_indicator_len > 0 {
        line_runs.insert(
            if visual.line_start_col > 0 { 1 } else { 0 },
            TextRun {
                len: visual.wrap_indicator_len,
                font: font.clone(),
                color: wrap_indicator_color.unwrap_or(fg_color),
                background_color: None,
                underline: None,
                strikethrough: None,
            },
        );
    }

    line_runs
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
    geometry: EditorSurfaceGeometry,
    line_height: Pixels,
    scroll_line_offset: Pixels,
    vertical_offset: usize,
    cursor_line: usize,
) -> Vec<SoftWrapLinePaintPlan<'a>> {
    let text_origin_x = geometry.text_origin_x();

    visual_lines
        .iter()
        .map(|visual| {
            let y_offset =
                -scroll_line_offset + line_height * visual.relative_row(vertical_offset) as f32;
            let line_y = geometry.bounds.origin.y + geometry.top_padding() + y_offset;

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
    visual_line: usize,
    fallback_doc_line: usize,
    line_graphemes: &[FormattedGrapheme<'_>],
) -> SoftWrapVisualLine {
    let doc_line = line_graphemes
        .first()
        .map_or(fallback_doc_line, |grapheme| grapheme.line_idx);
    let line_start_col = line_graphemes
        .first()
        .map_or(0, |grapheme| grapheme.visual_pos.col);
    let mut line_text = String::new();
    let mut wrap_indicator_len = 0;

    line_text.extend(std::iter::repeat_n(' ', line_start_col));

    for grapheme in line_graphemes {
        if grapheme.is_virtual() {
            if let Grapheme::Other { g } = &grapheme.raw {
                wrap_indicator_len += g.len();
                line_text.push_str(g);
            }
        } else {
            match &grapheme.raw {
                Grapheme::Tab { .. } => line_text.push('\t'),
                Grapheme::Other { g } => line_text.push_str(g),
                Grapheme::Newline => {}
            }
        }
    }

    let first_real = line_graphemes
        .iter()
        .find(|grapheme| !grapheme.is_virtual());
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

    SoftWrapVisualLine {
        visual_line,
        doc_line,
        text: line_text,
        line_start_col,
        wrap_indicator_len,
        line_start_char,
        line_end_char,
        segment_char_offset,
        text_start_byte_offset: line_start_col + wrap_indicator_len,
        is_phantom_line: line_start >= line_end,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, Font, Hsla, TextRun, black, blue, font, point, px, size, white};
    use helix_core::doc_formatter::TextFormat;
    use helix_view::view::ViewPosition;

    use super::{
        SoftWrapRenderPlanParams, SoftWrapVisualLine, decorate_soft_wrap_line_runs,
        soft_wrap_line_paint_plans, soft_wrap_render_plan, soft_wrap_viewport_height,
        soft_wrap_visual_lines, soft_wrap_visual_position,
    };
    use crate::EditorSurfaceGeometry;

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
            text: "    .wrapped".to_string(),
            line_start_col,
            wrap_indicator_len,
            line_start_char: Some(4),
            line_end_char: Some(11),
            segment_char_offset: 4,
            text_start_byte_offset: line_start_col + wrap_indicator_len,
            is_phantom_line: false,
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
        let lines = soft_wrap_visual_lines(text.as_str().into(), &text_format(), 0, 0, 3);

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
    fn starts_at_viewport_vertical_offset() {
        let text = "foo ".repeat(10);
        let lines = soft_wrap_visual_lines(text.as_str().into(), &text_format(), 0, 1, 1);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].visual_line, 1);
        assert_eq!(lines[0].relative_row(1), 0);
        assert_eq!(lines[0].text, ".foo foo foo foo ");
    }

    #[test]
    fn finds_visual_position_after_wrap_indicator() {
        let text = "foo ".repeat(10);
        let position = soft_wrap_visual_position(text.as_str().into(), &text_format(), 0, 16)
            .expect("cursor position");

        assert_eq!(position.visual_line, 1);
        assert_eq!(position.visual_col, 1);
    }

    #[test]
    fn decorated_runs_prefix_indent_and_wrap_indicator() {
        let runs = decorate_soft_wrap_line_runs(
            vec![run(7, white())],
            &visual_line(4, 1),
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
    }

    #[test]
    fn decorated_runs_prefix_wrap_indicator_without_indent() {
        let runs = decorate_soft_wrap_line_runs(
            vec![run(7, white())],
            &visual_line(0, 2),
            &test_font(),
            black(),
            Some(blue()),
        );

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, 2);
        assert_eq!(runs[0].color, blue());
        assert_eq!(runs[1].len, 7);
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
                text: "next".to_string(),
                line_start_col: 0,
                wrap_indicator_len: 0,
                line_start_char: Some(12),
                line_end_char: Some(16),
                segment_char_offset: 12,
                text_start_byte_offset: 0,
                is_phantom_line: false,
            },
        ];

        let plans = soft_wrap_line_paint_plans(&lines, geometry(), px(20.0), px(5.0), 1, 4);

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
            view_offset,
            bounds,
            gutter_columns: 4,
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(5.0),
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
}
