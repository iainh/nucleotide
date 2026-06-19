// ABOUTME: Geometry helpers and hit targets for native run buttons in the editor gutter
// ABOUTME: Keeps run-button paint and input math in one place

use gpui::{Bounds, Pixels, Point, Size, point, px, size};

const RUN_GUTTER_BUTTON_MIN_SIZE_PX: f32 = 12.0;
const RUN_GUTTER_BUTTON_MAX_SIZE_PX: f32 = 14.0;
const RUN_GUTTER_BUTTON_VERTICAL_INSET_PX: f32 = 6.0;
const RUN_GUTTER_BUTTON_HORIZONTAL_PADDING_PX: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GutterRunButtonHit {
    pub doc_line: usize,
    pub bounds: Bounds<Pixels>,
}

pub fn run_gutter_button_size(line_height: Pixels) -> Pixels {
    (line_height - px(RUN_GUTTER_BUTTON_VERTICAL_INSET_PX))
        .max(px(RUN_GUTTER_BUTTON_MIN_SIZE_PX))
        .min(px(RUN_GUTTER_BUTTON_MAX_SIZE_PX))
}

pub fn run_gutter_required_width(line_height: Pixels) -> Pixels {
    run_gutter_button_size(line_height) + px(RUN_GUTTER_BUTTON_HORIZONTAL_PADDING_PX * 2.0)
}

pub fn run_gutter_extra_columns(line_height: Pixels, cell_width: Pixels) -> u16 {
    let required_width = f32::from(run_gutter_required_width(line_height));
    let cell_width = f32::from(cell_width.max(px(1.0)));

    (required_width / cell_width)
        .ceil()
        .clamp(1.0, f32::from(u16::MAX)) as u16
}

pub fn run_gutter_button_left(
    gutter_width: Pixels,
    reserved_width: Pixels,
    button_size: Pixels,
) -> Pixels {
    let reserved_left = gutter_width - reserved_width;
    reserved_left + ((reserved_width - button_size) * 0.5).max(px(0.0))
}

pub fn run_gutter_button_bounds(
    gutter_left: Pixels,
    line_top: Pixels,
    gutter_width: Pixels,
    reserved_width: Pixels,
    line_height: Pixels,
) -> Bounds<Pixels> {
    let button_size = run_gutter_button_size(line_height);
    Bounds::new(
        point(
            gutter_left + run_gutter_button_left(gutter_width, reserved_width, button_size),
            line_top + ((line_height - button_size) * 0.5),
        ),
        size(button_size, button_size),
    )
}

pub fn run_gutter_icon_bounds(button_bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let icon_size = run_gutter_icon_size(button_bounds.size);
    let inset_x = ((button_bounds.size.width - icon_size) * 0.5).max(px(0.0));
    let inset_y = ((button_bounds.size.height - icon_size) * 0.5).max(px(0.0));

    Bounds::new(
        Point {
            x: button_bounds.origin.x + inset_x,
            y: button_bounds.origin.y + inset_y,
        },
        size(icon_size, icon_size),
    )
}

fn run_gutter_icon_size(button_size: Size<Pixels>) -> Pixels {
    (button_size.width.min(button_size.height) - px(2.0)).max(px(10.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_gutter_extra_columns_tracks_cell_width() {
        assert_eq!(run_gutter_extra_columns(px(20.0), px(12.0)), 2);
        assert_eq!(run_gutter_extra_columns(px(20.0), px(8.0)), 3);
    }

    #[test]
    fn run_gutter_button_is_centered_in_reserved_width() {
        assert_eq!(
            run_gutter_button_left(px(100.0), px(24.0), px(14.0)),
            px(81.0)
        );
    }

    #[test]
    fn run_gutter_button_bounds_include_gutter_origin() {
        assert_eq!(
            run_gutter_button_bounds(px(10.0), px(20.0), px(100.0), px(24.0), px(20.0)),
            Bounds::new(point(px(91.0), px(23.0)), size(px(14.0), px(14.0)))
        );
    }
}
