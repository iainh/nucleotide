// ABOUTME: Native editor virtual ruler geometry helpers
// ABOUTME: Converts configured ruler columns into visible GPUI paint bounds

use gpui::{Bounds, Pixels, point, px, size};

use crate::EditorSurfaceGeometry;

pub fn visible_ruler_bounds(
    geometry: EditorSurfaceGeometry,
    ruler_columns: &[u16],
    horizontal_offset: usize,
) -> Vec<Bounds<Pixels>> {
    let text_bounds = geometry.text_bounds();
    let right_edge = geometry.bounds.origin.x + geometry.bounds.size.width;
    let horizontal_offset = horizontal_offset as f32;

    ruler_columns
        .iter()
        .filter_map(|&ruler_col| {
            let zero_based_col = ruler_col.checked_sub(1)?;
            let ruler_x = text_bounds.origin.x
                + geometry.cell_width * (f32::from(zero_based_col) - horizontal_offset);

            (ruler_x >= text_bounds.origin.x && ruler_x < right_edge).then(|| {
                Bounds::new(
                    point(ruler_x, geometry.bounds.origin.y),
                    size(px(1.0), geometry.bounds.size.height),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};

    use super::*;

    fn geometry() -> EditorSurfaceGeometry {
        EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        )
    }

    #[test]
    fn ruler_bounds_use_text_origin_and_editor_height() {
        let rulers = visible_ruler_bounds(geometry(), &[1, 4, 80], 0);

        assert_eq!(
            rulers,
            vec![
                Bounds::new(point(px(132.0), px(40.0)), size(px(1.0), px(300.0))),
                Bounds::new(point(px(156.0), px(40.0)), size(px(1.0), px(300.0))),
            ]
        );
    }

    #[test]
    fn ruler_bounds_account_for_horizontal_offset() {
        let rulers = visible_ruler_bounds(geometry(), &[1, 4, 10], 3);

        assert_eq!(
            rulers,
            vec![
                Bounds::new(point(px(132.0), px(40.0)), size(px(1.0), px(300.0))),
                Bounds::new(point(px(180.0), px(40.0)), size(px(1.0), px(300.0))),
            ]
        );
    }

    #[test]
    fn ruler_bounds_skip_invalid_zero_column() {
        let rulers = visible_ruler_bounds(geometry(), &[0, 1], 0);

        assert_eq!(
            rulers,
            vec![Bounds::new(
                point(px(132.0), px(40.0)),
                size(px(1.0), px(300.0)),
            )]
        );
    }
}
