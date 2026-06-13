// ABOUTME: Native editor virtual ruler geometry helpers
// ABOUTME: Converts configured ruler columns into visible GPUI paint bounds

use gpui::{Bounds, Hsla, Pixels, Window, fill, point, px, size};
use helix_view::{Document, ViewId};

use crate::EditorSurfaceGeometry;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerPaintPlan {
    pub bounds: Bounds<Pixels>,
    pub color: Hsla,
}

pub struct DocumentRulerPaintParams<'a> {
    pub document: &'a Document,
    pub view_id: ViewId,
    pub editor_rulers: &'a [u16],
    pub geometry: EditorSurfaceGeometry,
    pub color: Hsla,
}

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

pub fn visible_ruler_paint_plans(
    geometry: EditorSurfaceGeometry,
    ruler_columns: &[u16],
    horizontal_offset: usize,
    color: Hsla,
) -> Vec<RulerPaintPlan> {
    visible_ruler_bounds(geometry, ruler_columns, horizontal_offset)
        .into_iter()
        .map(|bounds| RulerPaintPlan { bounds, color })
        .collect()
}

pub fn document_ruler_paint_plans(params: DocumentRulerPaintParams<'_>) -> Vec<RulerPaintPlan> {
    let rulers = params
        .document
        .language_config()
        .and_then(|config| config.rulers.as_ref())
        .map(Vec::as_slice)
        .unwrap_or(params.editor_rulers);
    let view_offset = params.document.view_offset(params.view_id);

    visible_ruler_paint_plans(
        params.geometry,
        rulers,
        view_offset.horizontal_offset,
        params.color,
    )
}

pub fn paint_document_rulers(
    window: &mut Window,
    params: DocumentRulerPaintParams<'_>,
) -> Vec<RulerPaintPlan> {
    let plans = document_ruler_paint_plans(params);
    paint_visible_rulers(window, &plans);
    plans
}

pub fn paint_visible_rulers(window: &mut Window, plans: &[RulerPaintPlan]) {
    for plan in plans {
        window.paint_quad(fill(plan.bounds, plan.color));
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, rgb, size};

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

    #[test]
    fn paint_plans_attach_color_to_visible_rulers() {
        let color = rgb(0x336699).into();
        let plans = visible_ruler_paint_plans(geometry(), &[1, 4, 80], 0, color);

        assert_eq!(
            plans,
            vec![
                RulerPaintPlan {
                    bounds: Bounds::new(point(px(132.0), px(40.0)), size(px(1.0), px(300.0))),
                    color,
                },
                RulerPaintPlan {
                    bounds: Bounds::new(point(px(156.0), px(40.0)), size(px(1.0), px(300.0))),
                    color,
                },
            ]
        );
    }
}
