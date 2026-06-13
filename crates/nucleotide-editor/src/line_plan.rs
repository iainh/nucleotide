// ABOUTME: Pure visible-line planning for native editor rendering
// ABOUTME: Converts Helix row ranges into renderable document line segments

use gpui::Pixels;
use helix_core::RopeSlice;

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

pub fn unwrapped_visible_line_plans(
    text: RopeSlice<'_>,
    viewport: LineViewportPlan,
    line_height: Pixels,
    scroll_line_offset: Pixels,
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
    }

    plans
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
    use super::{line_viewport_plan, unwrapped_visible_line_plans};
    use gpui::px;

    #[test]
    fn plans_unwrapped_visible_lines() {
        let text = "alpha\nbeta\ngamma";
        let viewport = line_viewport_plan(text.into(), 0, 3, 0);
        let plans = unwrapped_visible_line_plans(text.into(), viewport, px(14.0), px(0.0));

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
        let plans = unwrapped_visible_line_plans(text.into(), viewport, px(14.0), px(4.0));

        assert_eq!(plans[0].y_offset, px(-4.0));
        assert_eq!(plans[1].y_offset, px(10.0));
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
        let plans = unwrapped_visible_line_plans(text.into(), viewport, px(14.0), px(0.0));

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].line_idx, 0);
        assert_eq!((plans[0].line_start, plans[0].line_end), (0, 5));
        assert_eq!(plans[0].y_offset, px(0.0));
    }
}
