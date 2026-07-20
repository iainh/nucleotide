// ABOUTME: Native editor line painting helpers
// ABOUTME: Paints shaped-line backgrounds shared by wrapped and unwrapped render paths

use gpui::{
    App, Bounds, Hsla, Pixels, Point, Result, ShapedLine, SharedString, TextAlign, TextRun, Window,
    fill, point, px, size,
};
use std::ops::Range;

use crate::{
    line_cache::{LineLayout, LineLayoutCache},
    line_plan::UnwrappedLinePaintPlan,
    line_text::{DisplayLineText, normalize_text_runs_for_display_text},
    soft_wrap::SoftWrapLinePaintPlan,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndentGuidePaintConfig {
    pub indent_width: u16,
    pub skip_levels: u8,
    pub color: Hsla,
}

pub fn paint_cursorline_background(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    window.paint_quad(fill(bounds, color));
}

pub fn paint_indent_guides(
    window: &mut Window,
    text_origin: Point<Pixels>,
    line_height: Pixels,
    cell_width: Pixels,
    source_text: &str,
    config: IndentGuidePaintConfig,
) {
    for bounds in indent_guide_bounds(text_origin, line_height, cell_width, source_text, config) {
        window.paint_quad(fill(bounds, config.color));
    }
}

pub fn indent_guide_bounds(
    text_origin: Point<Pixels>,
    line_height: Pixels,
    cell_width: Pixels,
    source_text: &str,
    config: IndentGuidePaintConfig,
) -> Vec<Bounds<Pixels>> {
    let indent_width = usize::from(config.indent_width.max(1));
    let indent_cols = leading_indent_columns(source_text, indent_width as u16);
    let start_level = usize::from(config.skip_levels);
    let mut bounds = Vec::new();

    for level in start_level..(indent_cols / indent_width) {
        let col = level * indent_width;
        let x = text_origin.x + cell_width * (col as f32 + 0.5) - px(0.5);
        bounds.push(Bounds::new(
            point(x, text_origin.y),
            size(px(1.0), line_height),
        ));
    }

    bounds
}

fn leading_indent_columns(text: &str, tab_width: u16) -> usize {
    let mut col = 0usize;
    for ch in text.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col += helix_core::graphemes::tab_width_at(col, tab_width),
            _ if ch.is_whitespace() => col += 1,
            _ => break,
        }
    }
    col
}

pub fn paint_line_backgrounds(
    window: &mut Window,
    shaped_line: &ShapedLine,
    runs: &[TextRun],
    origin: Point<Pixels>,
    line_height: Pixels,
) {
    for (range, bg_color) in text_run_background_ranges(runs) {
        let start_x = shaped_line.x_for_index(range.start);
        let end_x = shaped_line.x_for_index(range.end);

        let bg_bounds = Bounds {
            origin: point(origin.x + start_x, origin.y),
            size: size(end_x - start_x, line_height),
        };
        window.paint_quad(fill(bg_bounds, bg_color));
    }
}

fn text_run_background_ranges(runs: &[TextRun]) -> impl Iterator<Item = (Range<usize>, Hsla)> + '_ {
    let mut byte_offset = 0;
    runs.iter().filter_map(move |run| {
        let start = byte_offset;
        byte_offset += run.len;
        run.background_color
            .map(|background| (start..byte_offset, background))
    })
}

pub fn paint_editor_line(
    window: &mut Window,
    cx: &mut App,
    shaped_line: &ShapedLine,
    runs: &[TextRun],
    origin: Point<Pixels>,
    line_height: Pixels,
) -> Result<()> {
    paint_line_backgrounds(window, shaped_line, runs, origin, line_height);
    shaped_line.paint(origin, line_height, TextAlign::Left, None, window, cx)
}

pub struct UnwrappedEditorLinePaintParams<'a, 'b> {
    pub plan: UnwrappedLinePaintPlan<'a>,
    pub line_text: DisplayLineText,
    pub line_runs: &'b [TextRun],
    pub line_cache: &'b LineLayoutCache,
    pub font_size: Pixels,
    pub viewport_width: Pixels,
    pub line_height: Pixels,
    pub cursorline_color: Option<Hsla>,
}

pub fn paint_unwrapped_editor_line(
    window: &mut Window,
    cx: &mut App,
    params: UnwrappedEditorLinePaintParams<'_, '_>,
) -> Result<LineLayout> {
    if params.plan.is_cursor_line
        && let Some(cursorline_color) = params.cursorline_color
    {
        paint_cursorline_background(window, params.plan.cursorline_bounds, cursorline_color);
    }

    let text_system = window.text_system().clone();
    let shaped_line = if params.line_text.is_empty() {
        params.line_cache.shape_line_cached(
            text_system.as_ref(),
            SharedString::from(""),
            params.font_size,
            params.viewport_width,
            &[],
        )
    } else {
        let line_runs = normalize_text_runs_for_display_text(
            params.line_text.display.as_ref(),
            params.line_runs,
        );
        let line_runs = line_runs.as_ref();
        let shaped_line = params.line_cache.shape_line_cached(
            text_system.as_ref(),
            params.line_text.display.clone(),
            params.font_size,
            params.viewport_width,
            line_runs,
        );
        paint_editor_line(
            window,
            cx,
            &shaped_line,
            line_runs,
            params.plan.text_origin,
            params.line_height,
        )?;
        shaped_line
    };

    Ok(LineLayout::from_visible_line_with_origin_x_and_display_map(
        params.plan.line,
        shaped_line,
        params.plan.line_origin.x,
        params.line_text.map,
    ))
}

pub struct SoftWrapEditorLinePaintParams<'a, 'b> {
    pub plan: SoftWrapLinePaintPlan<'a>,
    pub line_runs: &'b [TextRun],
    pub line_cache: &'b LineLayoutCache,
    pub font_size: Pixels,
    pub viewport_width: Pixels,
    pub line_height: Pixels,
    pub cursorline_color: Option<Hsla>,
}

pub fn paint_soft_wrap_editor_line(
    window: &mut Window,
    cx: &mut App,
    params: SoftWrapEditorLinePaintParams<'_, '_>,
) -> Result<Option<LineLayout>> {
    if params.plan.is_cursor_visual_line
        && let Some(cursorline_color) = params.cursorline_color
    {
        paint_cursorline_background(window, params.plan.cursorline_bounds, cursorline_color);
    }

    if params.plan.visual.text.is_empty() {
        return Ok(None);
    }

    let text_system = window.text_system().clone();
    let line_runs =
        normalize_text_runs_for_display_text(params.plan.visual.text.as_ref(), params.line_runs);
    let line_runs = line_runs.as_ref();
    let shaped_line = params.line_cache.shape_line_cached(
        text_system.as_ref(),
        params.plan.visual.text.clone(),
        params.font_size,
        params.viewport_width,
        line_runs,
    );
    paint_editor_line(
        window,
        cx,
        &shaped_line,
        line_runs,
        params.plan.text_origin,
        params.line_height,
    )?;

    Ok(soft_wrap_layout_for_painted_line(params.plan, shaped_line))
}

fn soft_wrap_layout_for_painted_line(
    plan: SoftWrapLinePaintPlan<'_>,
    shaped_line: ShapedLine,
) -> Option<LineLayout> {
    if plan.visual.text.is_empty() || plan.visual.is_phantom_line {
        return None;
    }

    Some(LineLayout::from_soft_wrap_visual(
        plan.visual,
        shaped_line,
        plan.y_offset,
    ))
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, ShapedLine, TextRun, black, font, point, px, rgb, size};

    use super::*;
    use crate::{SoftWrapLinePaintPlan, SoftWrapVisualLine, line_text::DisplayTextMap};

    fn run(len: usize, background_color: Option<Hsla>) -> TextRun {
        TextRun {
            len,
            font: font("TestFont"),
            color: black(),
            background_color,
            underline: None,
            strikethrough: None,
        }
    }

    #[test]
    fn background_ranges_include_every_explicit_background_in_run_order() {
        let match_background = rgb(0xcc6633).into();
        let selection_background = rgb(0x3366cc).into();
        let runs = [
            run(2, None),
            run(1, Some(match_background)),
            run(2, None),
            run(1, Some(selection_background)),
        ];

        assert_eq!(
            text_run_background_ranges(&runs).collect::<Vec<_>>(),
            vec![(2..3, match_background), (5..6, selection_background)]
        );
    }

    #[test]
    fn leading_indent_columns_expands_tabs() {
        assert_eq!(super::leading_indent_columns(" \tvalue", 4), 4);
        assert_eq!(super::leading_indent_columns("    value", 4), 4);
        assert_eq!(super::leading_indent_columns("value", 4), 0);
    }

    #[test]
    fn indent_guide_bounds_center_lines_in_indent_cells() {
        let config = IndentGuidePaintConfig {
            indent_width: 4,
            skip_levels: 0,
            color: rgb(0x999999).into(),
        };
        let bounds = indent_guide_bounds(
            point(px(100.0), px(20.0)),
            px(18.0),
            px(10.0),
            "        value",
            config,
        );

        assert_eq!(bounds.len(), 2);
        assert_eq!(bounds[0].origin, point(px(104.5), px(20.0)));
        assert_eq!(bounds[0].size, size(px(1.0), px(18.0)));
        assert_eq!(bounds[1].origin, point(px(144.5), px(20.0)));
    }

    #[test]
    fn indent_guide_bounds_honor_skip_levels() {
        let config = IndentGuidePaintConfig {
            indent_width: 4,
            skip_levels: 1,
            color: rgb(0x999999).into(),
        };
        let bounds = indent_guide_bounds(
            point(px(100.0), px(20.0)),
            px(18.0),
            px(10.0),
            "        value",
            config,
        );

        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].origin, point(px(144.5), px(20.0)));
    }

    fn soft_wrap_visual(text: &str, is_phantom_line: bool) -> SoftWrapVisualLine {
        SoftWrapVisualLine {
            visual_line: 3,
            doc_line: 11,
            text: text.into(),
            line_start_col: 0,
            wrap_indicator_len: 0,
            line_start_char: Some(30),
            line_end_char: Some(30 + text.chars().count()),
            segment_char_offset: 30,
            text_start_byte_offset: 0,
            is_phantom_line,
            display_map: DisplayTextMap::identity(text.len()),
            virtual_text_ranges: Vec::new(),
            whitespace_ranges: Vec::new(),
        }
    }

    fn soft_wrap_plan(visual: &SoftWrapVisualLine) -> SoftWrapLinePaintPlan<'_> {
        SoftWrapLinePaintPlan {
            visual,
            y_offset: px(72.0),
            line_y: px(112.0),
            text_origin: point(px(132.0), px(112.0)),
            cursorline_bounds: Bounds::new(point(px(100.0), px(112.0)), size(px(500.0), px(20.0))),
            is_cursor_visual_line: true,
        }
    }

    #[test]
    fn soft_wrap_layout_tracks_painted_visual_line_metadata() {
        let visual = soft_wrap_visual("wrapped", false);
        let layout =
            soft_wrap_layout_for_painted_line(soft_wrap_plan(&visual), ShapedLine::default())
                .unwrap();

        assert_eq!(layout.line_idx, 11);
        assert_eq!(layout.origin, point(px(0.0), px(72.0)));
        assert_eq!(layout.segment_char_offset, 30);
        assert_eq!(layout.text_start_byte_offset, 0);
    }

    #[test]
    fn soft_wrap_layout_skips_phantom_visual_lines() {
        let visual = soft_wrap_visual("phantom", true);

        assert!(
            soft_wrap_layout_for_painted_line(soft_wrap_plan(&visual), ShapedLine::default())
                .is_none()
        );
    }

    #[test]
    fn soft_wrap_layout_skips_empty_visual_lines() {
        let visual = soft_wrap_visual("", false);

        assert!(
            soft_wrap_layout_for_painted_line(soft_wrap_plan(&visual), ShapedLine::default())
                .is_none()
        );
    }
}
