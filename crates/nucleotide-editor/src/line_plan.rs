// ABOUTME: Pure visible-line planning for native editor rendering
// ABOUTME: Converts Helix row ranges into renderable document line segments

use std::collections::BTreeMap;

use gpui::{Bounds, Pixels, Point, point, size};
use helix_core::RopeSlice;

use crate::EditorSurfaceGeometry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineViewportPlan {
    pub total_lines: usize,
    pub first_row: usize,
    pub last_row: usize,
    pub end_char: usize,
    pub cursor_at_end: bool,
    pub file_ends_with_newline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleLinePlan {
    pub line_idx: usize,
    pub line_start: usize,
    pub line_end: usize,
    pub y_offset: Pixels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnwrappedLinePaintPlan<'a> {
    pub line: &'a VisibleLinePlan,
    pub text_origin: Point<Pixels>,
    pub line_origin: Point<Pixels>,
    pub cursorline_bounds: Bounds<Pixels>,
    pub is_cursor_line: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnwrappedRenderPlanParams<'a> {
    pub text: RopeSlice<'a>,
    pub line_viewport: LineViewportPlan,
    pub bounds: Bounds<Pixels>,
    pub gutter_columns: u16,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
    pub horizontal_offset: usize,
    pub cursor_line: usize,
    pub inline_diagnostic_virtual_rows: Option<&'a BTreeMap<usize, usize>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnwrappedRenderPlan {
    pub line_viewport: LineViewportPlan,
    pub geometry: EditorSurfaceGeometry,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
    pub horizontal_offset: usize,
    pub cursor_line: usize,
    pub visible_lines: Vec<VisibleLinePlan>,
    pub next_line_y_offset: Pixels,
}

impl UnwrappedRenderPlan {
    pub fn line_paint_plans(&self) -> Vec<UnwrappedLinePaintPlan<'_>> {
        unwrapped_line_paint_plans(
            &self.visible_lines,
            self.geometry,
            self.line_height,
            self.horizontal_offset,
            self.cursor_line,
        )
    }
}

pub fn line_viewport_plan(
    text: RopeSlice<'_>,
    first_row: usize,
    last_row_from_scroll: usize,
    cursor_char_idx: usize,
) -> LineViewportPlan {
    let total_lines = text.len_lines();
    let cursor_at_end = cursor_char_idx == text.len_chars();
    let file_ends_with_newline = text.len_chars() > 0 && text.char(text.len_chars() - 1) == '\n';

    let mut last_row = last_row_from_scroll;
    if cursor_at_end && file_ends_with_newline {
        last_row = last_row.max(total_lines);
    }

    let end_char = if last_row > total_lines {
        text.len_chars() + 1
    } else {
        text.line_to_char(last_row.min(total_lines))
    };

    LineViewportPlan {
        total_lines,
        first_row,
        last_row,
        end_char,
        cursor_at_end,
        file_ends_with_newline,
    }
}

pub fn unwrapped_render_plan(params: UnwrappedRenderPlanParams<'_>) -> UnwrappedRenderPlan {
    let visible_lines = unwrapped_visible_line_plans(
        params.text,
        params.line_viewport,
        params.line_height,
        params.scroll_line_offset,
        params.inline_diagnostic_virtual_rows,
    );
    let next_line_y_offset = visible_lines
        .last()
        .map_or(-params.scroll_line_offset, |line| {
            line.y_offset + params.line_height
        });

    UnwrappedRenderPlan {
        line_viewport: params.line_viewport,
        geometry: EditorSurfaceGeometry::new(
            params.bounds,
            params.gutter_columns,
            params.cell_width,
        ),
        line_height: params.line_height,
        scroll_line_offset: params.scroll_line_offset,
        horizontal_offset: params.horizontal_offset,
        cursor_line: params.cursor_line,
        visible_lines,
        next_line_y_offset,
    }
}

pub fn unwrapped_visible_line_plans(
    text: RopeSlice<'_>,
    viewport: LineViewportPlan,
    line_height: Pixels,
    scroll_line_offset: Pixels,
    inline_diagnostic_virtual_rows: Option<&BTreeMap<usize, usize>>,
) -> Vec<VisibleLinePlan> {
    let anchor_char = text.line_to_char(viewport.first_row.min(viewport.total_lines));
    let mut y_offset = -scroll_line_offset;
    let mut plans = Vec::new();

    for line_idx in viewport.first_row..viewport.last_row {
        let (line_start, line_end) = line_bounds(text, line_idx, viewport.total_lines);
        if is_phantom_line(text, line_idx, viewport) {
            continue;
        }

        if line_start >= viewport.end_char || line_end < anchor_char {
            y_offset += line_height;
            continue;
        }

        let line_start = line_start.max(anchor_char);
        let line_end = line_end.min(viewport.end_char);
        if line_start > line_end {
            y_offset += line_height;
            continue;
        }

        plans.push(VisibleLinePlan {
            line_idx,
            line_start,
            line_end,
            y_offset,
        });
        y_offset += line_height;
        y_offset += line_height
            * inline_diagnostic_virtual_rows
                .and_then(|rows| rows.get(&line_idx))
                .copied()
                .unwrap_or(0) as f32;
    }

    plans
}

pub fn unwrapped_line_paint_plans<'a>(
    lines: &'a [VisibleLinePlan],
    geometry: EditorSurfaceGeometry,
    line_height: Pixels,
    horizontal_offset: usize,
    cursor_line: usize,
) -> Vec<UnwrappedLinePaintPlan<'a>> {
    let horizontal_offset_x = geometry.cell_width * horizontal_offset as f32;
    let text_origin_x = geometry.text_origin_x() - horizontal_offset_x;

    lines
        .iter()
        .map(|line| {
            let line_y = geometry.bounds.origin.y + geometry.top_padding() + line.y_offset;

            UnwrappedLinePaintPlan {
                line,
                text_origin: point(text_origin_x, line_y),
                line_origin: point(-horizontal_offset_x, line.y_offset),
                cursorline_bounds: Bounds::new(
                    point(geometry.bounds.origin.x, line_y),
                    size(geometry.bounds.size.width, line_height),
                ),
                is_cursor_line: line.line_idx == cursor_line,
            }
        })
        .collect()
}

fn line_bounds(text: RopeSlice<'_>, line_idx: usize, total_lines: usize) -> (usize, usize) {
    let line_start = if line_idx < total_lines {
        text.line_to_char(line_idx)
    } else {
        text.len_chars()
    };
    let line_end = if line_idx + 1 < total_lines {
        text.line_to_char(line_idx + 1).saturating_sub(1)
    } else {
        text.len_chars()
    };

    (line_start, line_end)
}

fn is_phantom_line(text: RopeSlice<'_>, line_idx: usize, viewport: LineViewportPlan) -> bool {
    let (line_start, line_end) = line_bounds(text, line_idx, viewport.total_lines);
    let line_is_empty = line_start >= line_end;

    (viewport.cursor_at_end
        && viewport.file_ends_with_newline
        && line_idx == viewport.total_lines - 1)
        || line_idx >= viewport.total_lines
        || (line_idx == viewport.total_lines - 1 && line_is_empty && viewport.total_lines > 1)
}

#[cfg(test)]
mod tests {
    use super::{
        UnwrappedRenderPlanParams, VisibleLinePlan, line_viewport_plan, unwrapped_line_paint_plans,
        unwrapped_render_plan, unwrapped_visible_line_plans,
    };
    use crate::EditorSurfaceGeometry;
    use gpui::{Bounds, point, px, size};

    #[test]
    fn plans_unwrapped_visible_lines() {
        let text = "alpha\nbeta\ngamma";
        let viewport = line_viewport_plan(text.into(), 0, 3, 0);
        let plans = unwrapped_visible_line_plans(text.into(), viewport, px(14.0), px(0.0), None);

        assert_eq!(plans.len(), 3);
        assert_eq!(plans[0].line_idx, 0);
        assert_eq!((plans[0].line_start, plans[0].line_end), (0, 5));
        assert_eq!(plans[0].y_offset, px(0.0));
        assert_eq!(plans[1].line_idx, 1);
        assert_eq!((plans[1].line_start, plans[1].line_end), (6, 10));
        assert_eq!(plans[1].y_offset, px(14.0));
        assert_eq!(plans[2].line_idx, 2);
        assert_eq!((plans[2].line_start, plans[2].line_end), (11, 16));
        assert_eq!(plans[2].y_offset, px(28.0));
    }

    #[test]
    fn starts_at_negative_scroll_offset() {
        let text = "alpha\nbeta";
        let viewport = line_viewport_plan(text.into(), 0, 2, 0);
        let plans = unwrapped_visible_line_plans(text.into(), viewport, px(14.0), px(4.0), None);

        assert_eq!(plans[0].y_offset, px(-4.0));
        assert_eq!(plans[1].y_offset, px(10.0));
    }

    #[test]
    fn unwrapped_lines_reserve_inline_diagnostic_rows() {
        let text = "alpha\nbeta\ngamma";
        let viewport = line_viewport_plan(text.into(), 0, 3, 0);
        let mut virtual_rows = std::collections::BTreeMap::new();
        virtual_rows.insert(0, 2);

        let plans = unwrapped_visible_line_plans(
            text.into(),
            viewport,
            px(14.0),
            px(0.0),
            Some(&virtual_rows),
        );

        assert_eq!(plans[0].y_offset, px(0.0));
        assert_eq!(plans[1].y_offset, px(42.0));
        assert_eq!(plans[2].y_offset, px(56.0));
    }

    #[test]
    fn extends_last_row_for_cursor_at_trailing_newline() {
        let text = "alpha\n";
        let viewport = line_viewport_plan(text.into(), 0, 1, text.chars().count());

        assert_eq!(viewport.total_lines, 2);
        assert_eq!(viewport.last_row, 2);
        assert_eq!(viewport.end_char, text.chars().count());
        assert!(viewport.cursor_at_end);
        assert!(viewport.file_ends_with_newline);
    }

    #[test]
    fn skips_trailing_phantom_line_without_consuming_vertical_space() {
        let text = "alpha\n";
        let viewport = line_viewport_plan(text.into(), 0, 2, text.chars().count());
        let plans = unwrapped_visible_line_plans(text.into(), viewport, px(14.0), px(0.0), None);

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].line_idx, 0);
        assert_eq!((plans[0].line_start, plans[0].line_end), (0, 5));
        assert_eq!(plans[0].y_offset, px(0.0));
    }

    #[test]
    fn plans_unwrapped_line_paint_geometry() {
        let lines = vec![
            VisibleLinePlan {
                line_idx: 4,
                line_start: 20,
                line_end: 25,
                y_offset: px(0.0),
            },
            VisibleLinePlan {
                line_idx: 5,
                line_start: 26,
                line_end: 31,
                y_offset: px(20.0),
            },
        ];
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );

        let plans = unwrapped_line_paint_plans(&lines, geometry, px(20.0), 0, 5);

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].line, &lines[0]);
        assert_eq!(plans[0].text_origin, point(px(132.0), px(41.0)));
        assert_eq!(plans[0].line_origin, point(px(0.0), px(0.0)));
        assert_eq!(
            plans[0].cursorline_bounds,
            Bounds::new(point(px(100.0), px(41.0)), size(px(500.0), px(20.0)))
        );
        assert!(!plans[0].is_cursor_line);
        assert_eq!(plans[1].line, &lines[1]);
        assert_eq!(plans[1].text_origin, point(px(132.0), px(61.0)));
        assert_eq!(plans[1].line_origin, point(px(0.0), px(20.0)));
        assert_eq!(
            plans[1].cursorline_bounds,
            Bounds::new(point(px(100.0), px(61.0)), size(px(500.0), px(20.0)))
        );
        assert!(plans[1].is_cursor_line);
    }

    #[test]
    fn plans_unwrapped_line_paint_geometry_with_horizontal_offset() {
        let lines = vec![VisibleLinePlan {
            line_idx: 4,
            line_start: 20,
            line_end: 25,
            y_offset: px(0.0),
        }];
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );

        let plans = unwrapped_line_paint_plans(&lines, geometry, px(20.0), 3, 4);

        assert_eq!(plans[0].text_origin, point(px(108.0), px(41.0)));
        assert_eq!(plans[0].line_origin, point(px(-24.0), px(0.0)));
    }

    #[test]
    fn render_plan_collects_unwrapped_lines_and_paint_geometry() {
        let text = "alpha\nbeta\ngamma";
        let line_viewport = line_viewport_plan(text.into(), 0, 3, 6);
        let bounds = Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0)));

        let plan = unwrapped_render_plan(UnwrappedRenderPlanParams {
            text: text.into(),
            line_viewport,
            bounds,
            gutter_columns: 4,
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(5.0),
            horizontal_offset: 0,
            cursor_line: 1,
            inline_diagnostic_virtual_rows: None,
        });

        assert_eq!(plan.line_viewport, line_viewport);
        assert_eq!(
            plan.geometry,
            EditorSurfaceGeometry::new(bounds, 4, px(8.0))
        );
        assert_eq!(plan.visible_lines.len(), 3);
        assert_eq!(plan.visible_lines[0].line_idx, 0);
        assert_eq!(plan.visible_lines[0].y_offset, px(-5.0));
        assert_eq!(plan.next_line_y_offset, px(55.0));

        let paint_plans = plan.line_paint_plans();

        assert_eq!(paint_plans.len(), 3);
        assert_eq!(paint_plans[0].text_origin, point(px(132.0), px(36.0)));
        assert!(!paint_plans[0].is_cursor_line);
        assert!(paint_plans[1].is_cursor_line);
    }

    #[test]
    fn render_plan_fallback_y_offset_uses_scroll_offset() {
        let text = "alpha\n";
        let line_viewport = line_viewport_plan(text.into(), 1, 1, 0);
        let bounds = Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0)));

        let plan = unwrapped_render_plan(UnwrappedRenderPlanParams {
            text: text.into(),
            line_viewport,
            bounds,
            gutter_columns: 4,
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(7.0),
            horizontal_offset: 0,
            cursor_line: 0,
            inline_diagnostic_virtual_rows: None,
        });

        assert!(plan.visible_lines.is_empty());
        assert_eq!(plan.next_line_y_offset, px(-7.0));
    }
}
