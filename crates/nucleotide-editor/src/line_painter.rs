// ABOUTME: Native editor line painting helpers
// ABOUTME: Paints shaped-line backgrounds shared by wrapped and unwrapped render paths

use gpui::{
    App, Bounds, Hsla, Pixels, Point, Result, ShapedLine, TextRun, Window, fill, point, size,
};

#[derive(Debug, Clone, Copy)]
pub struct EditorLineBackgroundStyle {
    pub only_selection_backgrounds: bool,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
}

pub fn paint_line_backgrounds(
    window: &mut Window,
    shaped_line: &ShapedLine,
    runs: &[TextRun],
    origin: Point<Pixels>,
    line_height: Pixels,
    style: EditorLineBackgroundStyle,
) {
    let mut byte_offset = 0;
    for run in runs {
        if let Some(bg_color) = run.background_color
            && should_paint_background(bg_color, style)
        {
            let start_x = shaped_line.x_for_index(byte_offset);
            let end_x = shaped_line.x_for_index(byte_offset + run.len);

            let bg_bounds = Bounds {
                origin: point(origin.x + start_x, origin.y),
                size: size(end_x - start_x, line_height),
            };
            window.paint_quad(fill(bg_bounds, bg_color));
        }

        byte_offset += run.len;
    }
}

pub fn paint_editor_line(
    window: &mut Window,
    cx: &mut App,
    shaped_line: &ShapedLine,
    runs: &[TextRun],
    origin: Point<Pixels>,
    line_height: Pixels,
    background_style: EditorLineBackgroundStyle,
) -> Result<()> {
    paint_line_backgrounds(
        window,
        shaped_line,
        runs,
        origin,
        line_height,
        background_style,
    );
    shaped_line.paint(origin, line_height, window, cx)
}

fn should_paint_background(bg_color: Hsla, style: EditorLineBackgroundStyle) -> bool {
    !style.only_selection_backgrounds
        || approx_hsla_eq(bg_color, style.selection_primary)
        || approx_hsla_eq(bg_color, style.selection_secondary)
}

fn approx_hsla_eq(a: Hsla, b: Hsla) -> bool {
    let eh = (a.h - b.h).abs() <= 0.005;
    let es = (a.s - b.s).abs() <= 0.005;
    let el = (a.l - b.l).abs() <= 0.005;
    let ea = (a.a - b.a).abs() <= 0.005;
    eh && es && el && ea
}

#[cfg(test)]
mod tests {
    use gpui::rgb;

    use super::*;

    fn style(only_selection_backgrounds: bool) -> EditorLineBackgroundStyle {
        EditorLineBackgroundStyle {
            only_selection_backgrounds,
            selection_primary: rgb(0x3366cc).into(),
            selection_secondary: rgb(0x669933).into(),
        }
    }

    #[test]
    fn all_backgrounds_paint_off_cursor_line() {
        assert!(should_paint_background(rgb(0xcc6633).into(), style(false)));
    }

    #[test]
    fn cursor_line_paints_selection_backgrounds() {
        assert!(should_paint_background(rgb(0x3366cc).into(), style(true)));
        assert!(should_paint_background(rgb(0x669933).into(), style(true)));
    }

    #[test]
    fn cursor_line_filters_non_selection_backgrounds() {
        assert!(!should_paint_background(rgb(0xcc6633).into(), style(true)));
    }

    #[test]
    fn selection_matching_tolerates_minor_rounding() {
        let mut nearly_primary: Hsla = rgb(0x3366cc).into();
        nearly_primary.h += 0.001;
        nearly_primary.s += 0.001;
        nearly_primary.l += 0.001;

        assert!(should_paint_background(nearly_primary, style(true)));
    }
}
