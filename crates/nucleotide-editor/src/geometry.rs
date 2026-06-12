// ABOUTME: Native editor surface geometry and coordinate transforms
// ABOUTME: Centralizes text-area bounds used by rendering and input handling

use gpui::{Bounds, Hitbox, Pixels, Point, point, px, size};

const RIGHT_PADDING_COLUMNS: f32 = 2.0;
const TOP_PADDING: f32 = 1.0;

#[derive(Debug)]
pub struct EditorLayout {
    pub rows: usize,
    pub columns: usize,
    pub line_height: Pixels,
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub hitbox: Option<Hitbox>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorSurfaceGeometry {
    pub bounds: Bounds<Pixels>,
    pub gutter_columns: u16,
    pub cell_width: Pixels,
}

impl EditorSurfaceGeometry {
    pub fn new(bounds: Bounds<Pixels>, gutter_columns: u16, cell_width: Pixels) -> Self {
        Self {
            bounds,
            gutter_columns,
            cell_width,
        }
    }

    pub fn gutter_width(&self) -> Pixels {
        self.cell_width * f32::from(self.gutter_columns)
    }

    pub fn right_padding(&self) -> Pixels {
        self.cell_width * RIGHT_PADDING_COLUMNS
    }

    pub fn top_padding(&self) -> Pixels {
        px(TOP_PADDING)
    }

    pub fn text_bounds(&self) -> Bounds<Pixels> {
        let gutter_width = self.gutter_width();
        let right_padding = self.right_padding();
        let top_padding = self.top_padding();

        Bounds {
            origin: point(
                self.bounds.origin.x + gutter_width,
                self.bounds.origin.y + top_padding,
            ),
            size: size(
                self.bounds.size.width - gutter_width - right_padding,
                self.bounds.size.height - top_padding,
            ),
        }
    }

    pub fn text_origin_x(&self) -> Pixels {
        self.text_bounds().origin.x
    }

    pub fn viewport_columns(&self, minimum: u16) -> u16 {
        let minimum = minimum.max(1);
        let cell_width = f32::from(self.cell_width);
        if cell_width <= 0.0 {
            return minimum;
        }

        let text_width = f32::from(self.text_bounds().size.width.max(px(0.0)));
        ((text_width / cell_width).floor() as u16).max(minimum)
    }

    pub fn window_to_text_area(&self, window_pos: Point<Pixels>) -> Point<Pixels> {
        let text_bounds = self.text_bounds();
        point(
            window_pos.x - text_bounds.origin.x,
            window_pos.y - text_bounds.origin.y,
        )
    }

    pub fn clamp_text_area_position(
        &self,
        text_area_pos: Point<Pixels>,
        clamp_y_to_viewport: bool,
    ) -> Point<Pixels> {
        let text_bounds = self.text_bounds();
        let y = if clamp_y_to_viewport {
            text_area_pos.y.max(px(0.0)).min(text_bounds.size.height)
        } else {
            text_area_pos.y.max(px(0.0))
        };

        point(text_area_pos.x.max(px(0.0)).min(text_bounds.size.width), y)
    }

    pub fn text_area_to_content(
        &self,
        text_area_pos: Point<Pixels>,
        scroll_position: Point<Pixels>,
    ) -> Point<Pixels> {
        point(
            text_area_pos.x + scroll_position.x,
            text_area_pos.y + scroll_position.y,
        )
    }

    pub fn window_to_content(
        &self,
        window_pos: Point<Pixels>,
        scroll_position: Point<Pixels>,
        clamp_y_to_viewport: bool,
    ) -> Point<Pixels> {
        let text_area_pos = self.window_to_text_area(window_pos);
        let clamped_text_area_pos =
            self.clamp_text_area_position(text_area_pos, clamp_y_to_viewport);
        let content_pos = self.text_area_to_content(clamped_text_area_pos, scroll_position);

        point(content_pos.x.max(px(0.0)), content_pos.y.max(px(0.0)))
    }

    pub fn x_overshoot(x: Pixels, line_width: Pixels) -> (Pixels, Pixels) {
        if x > line_width {
            (line_width, x - line_width)
        } else {
            (x, px(0.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};

    use super::*;

    fn test_geometry() -> EditorSurfaceGeometry {
        EditorSurfaceGeometry::new(
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(80.0))),
            2,
            px(10.0),
        )
    }

    #[test]
    fn text_bounds_exclude_gutter_and_padding() {
        let geometry = test_geometry();

        assert_eq!(
            geometry.text_bounds(),
            Bounds::new(point(px(20.0), px(1.0)), size(px(60.0), px(79.0)))
        );
    }

    #[test]
    fn viewport_columns_use_text_area_width_once() {
        let geometry = test_geometry();

        assert_eq!(geometry.viewport_columns(1), 6);
    }

    #[test]
    fn window_to_text_area_preserves_out_of_bounds_positions() {
        let geometry = test_geometry();

        assert_eq!(
            geometry.window_to_text_area(point(px(15.0), px(-4.0))),
            point(px(-5.0), px(-5.0))
        );
    }

    #[test]
    fn content_position_adds_positive_scroll_position() {
        let geometry = test_geometry();

        assert_eq!(
            geometry.text_area_to_content(point(px(15.0), px(25.0)), point(px(5.0), px(40.0))),
            point(px(20.0), px(65.0))
        );
    }

    #[test]
    fn clamp_text_area_position_can_leave_y_unbounded() {
        let geometry = test_geometry();

        assert_eq!(
            geometry.clamp_text_area_position(point(px(90.0), px(120.0)), false),
            point(px(60.0), px(120.0))
        );
        assert_eq!(
            geometry.clamp_text_area_position(point(px(90.0), px(120.0)), true),
            point(px(60.0), px(79.0))
        );
    }

    #[test]
    fn window_to_content_clamps_negative_coordinates() {
        let geometry = test_geometry();

        assert_eq!(
            geometry.window_to_content(point(px(10.0), px(-5.0)), point(px(0.0), px(20.0)), true),
            point(px(0.0), px(20.0))
        );
    }

    #[test]
    fn x_overshoot_tracks_distance_past_line_end() {
        assert_eq!(
            EditorSurfaceGeometry::x_overshoot(px(120.0), px(80.0)),
            (px(80.0), px(40.0))
        );
        assert_eq!(
            EditorSurfaceGeometry::x_overshoot(px(64.0), px(80.0)),
            (px(64.0), px(0.0))
        );
    }
}
