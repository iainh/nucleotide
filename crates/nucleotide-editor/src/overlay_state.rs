// ABOUTME: Native editor overlay paint state shared with GPUI integration
// ABOUTME: Tracks cursor overlay bounds and gutter width from frame painting

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{Pixels, Point, Size, point, px};

use crate::{CursorOverlayPlan, GutterLinePlan, GutterRunButtonHit};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GutterLineAnchor {
    pub doc_line: usize,
    pub visual_line: u16,
    pub first_visual_line: bool,
    pub origin: Point<Pixels>,
}

#[derive(Clone, Debug)]
pub struct EditorOverlayState {
    cursor_position: Rc<Cell<Option<Point<Pixels>>>>,
    cursor_size: Rc<Cell<Option<Size<Pixels>>>>,
    gutter_width: Rc<Cell<Pixels>>,
    gutter_extra_columns: Rc<Cell<u16>>,
    gutter_line_anchors: Rc<RefCell<Vec<GutterLineAnchor>>>,
    gutter_run_button_hits: Rc<RefCell<Vec<GutterRunButtonHit>>>,
}

impl Default for EditorOverlayState {
    fn default() -> Self {
        Self {
            cursor_position: Rc::new(Cell::new(None)),
            cursor_size: Rc::new(Cell::new(None)),
            gutter_width: Rc::new(Cell::new(px(0.0))),
            gutter_extra_columns: Rc::new(Cell::new(0)),
            gutter_line_anchors: Rc::new(RefCell::new(Vec::new())),
            gutter_run_button_hits: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl EditorOverlayState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_cursor_overlay_plan(&self, overlay_plan: Option<CursorOverlayPlan>) {
        if let Some(overlay_plan) = overlay_plan {
            self.cursor_position.set(Some(overlay_plan.cursor_position));
            self.cursor_size.set(Some(overlay_plan.cursor_size));
        } else {
            self.cursor_position.set(None);
            self.cursor_size.set(None);
        }
    }

    pub fn cursor_overlay_bounds(&self) -> Option<(Point<Pixels>, Size<Pixels>)> {
        self.cursor_position.get().zip(self.cursor_size.get())
    }

    pub fn cursor_completion_anchor(&self) -> Option<(Point<Pixels>, Size<Pixels>)> {
        cursor_completion_anchor(self.cursor_overlay_bounds())
    }

    pub fn set_gutter_width_from_columns(&self, gutter_columns: u16, cell_width: Pixels) {
        self.gutter_width
            .set(gutter_width_from_columns(gutter_columns, cell_width));
    }

    pub fn gutter_width(&self) -> Pixels {
        self.gutter_width.get()
    }

    pub fn set_applied_gutter_extra_columns(&self, columns: u16) {
        self.gutter_extra_columns.set(columns);
    }

    pub fn gutter_extra_columns(&self) -> u16 {
        self.gutter_extra_columns.get()
    }

    pub fn set_gutter_line_anchors_from_plans(
        &self,
        plans: &[GutterLinePlan],
        surface_origin: Point<Pixels>,
    ) {
        let mut anchors = self.gutter_line_anchors.borrow_mut();
        anchors.clear();
        anchors.extend(plans.iter().map(|plan| GutterLineAnchor {
            doc_line: plan.doc_line,
            visual_line: plan.visual_line,
            first_visual_line: plan.first_visual_line,
            origin: point(
                plan.origin.x - surface_origin.x,
                plan.origin.y - surface_origin.y,
            ),
        }));
    }

    pub fn gutter_line_anchors(&self) -> Vec<GutterLineAnchor> {
        self.gutter_line_anchors.borrow().clone()
    }

    pub fn set_gutter_run_button_hits(&self, hits: Vec<GutterRunButtonHit>) {
        *self.gutter_run_button_hits.borrow_mut() = hits;
    }

    pub fn clear_gutter_run_button_hits(&self) {
        self.gutter_run_button_hits.borrow_mut().clear();
    }

    pub fn gutter_run_button_hits(&self) -> Vec<GutterRunButtonHit> {
        self.gutter_run_button_hits.borrow().clone()
    }

    pub fn gutter_run_button_line_at(&self, position: Point<Pixels>) -> Option<usize> {
        self.gutter_run_button_hits
            .borrow()
            .iter()
            .find(|hit| hit.bounds.contains(&position))
            .map(|hit| hit.doc_line)
    }
}

fn cursor_completion_anchor(
    cursor_overlay_bounds: Option<(Point<Pixels>, Size<Pixels>)>,
) -> Option<(Point<Pixels>, Size<Pixels>)> {
    let (position, size) = cursor_overlay_bounds?;
    Some((
        Point {
            x: position.x,
            y: position.y + size.height,
        },
        size,
    ))
}

fn gutter_width_from_columns(gutter_columns: u16, cell_width: Pixels) -> Pixels {
    cell_width * f32::from(gutter_columns)
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::*;

    #[test]
    fn cursor_completion_anchor_uses_cursor_bottom_left() {
        let position = point(px(12.0), px(34.0));
        let size = size(px(8.0), px(20.0));

        let Some((anchor, returned_size)) = cursor_completion_anchor(Some((position, size))) else {
            panic!("expected cursor anchor");
        };

        assert_eq!(anchor, point(px(12.0), px(54.0)));
        assert_eq!(returned_size, size);
    }

    #[test]
    fn cursor_completion_anchor_requires_overlay_bounds() {
        assert!(cursor_completion_anchor(None).is_none());
    }

    #[test]
    fn gutter_width_uses_gutter_columns_and_cell_width() {
        assert_eq!(gutter_width_from_columns(0, px(8.0)), px(0.0));
        assert_eq!(gutter_width_from_columns(6, px(8.0)), px(48.0));
    }

    #[test]
    fn overlay_state_tracks_and_clears_cursor_bounds() {
        let state = EditorOverlayState::new();
        let overlay_plan = CursorOverlayPlan {
            cursor_position: point(px(4.0), px(6.0)),
            cursor_size: size(px(8.0), px(20.0)),
        };

        state.apply_cursor_overlay_plan(Some(overlay_plan));

        assert_eq!(
            state.cursor_overlay_bounds(),
            Some((overlay_plan.cursor_position, overlay_plan.cursor_size))
        );

        state.apply_cursor_overlay_plan(None);

        assert_eq!(state.cursor_overlay_bounds(), None);
    }

    #[test]
    fn overlay_state_updates_gutter_width_from_columns() {
        let state = EditorOverlayState::new();

        state.set_gutter_width_from_columns(6, px(8.0));
        state.set_applied_gutter_extra_columns(2);

        assert_eq!(state.gutter_width(), px(48.0));
        assert_eq!(state.gutter_extra_columns(), 2);
    }

    #[test]
    fn overlay_state_tracks_gutter_line_anchors() {
        let state = EditorOverlayState::new();
        let plans = vec![
            GutterLinePlan {
                doc_line: 2,
                visual_line: 0,
                first_visual_line: true,
                origin: point(px(4.0), px(10.0)),
                text: "3".to_string(),
                style: Default::default(),
                kind: crate::GutterLineKind::Text,
            },
            GutterLinePlan {
                doc_line: 2,
                visual_line: 1,
                first_visual_line: false,
                origin: point(px(4.0), px(30.0)),
                text: " ".to_string(),
                style: Default::default(),
                kind: crate::GutterLineKind::Text,
            },
        ];

        state.set_gutter_line_anchors_from_plans(&plans, point(px(1.0), px(2.0)));

        assert_eq!(
            state.gutter_line_anchors(),
            vec![
                GutterLineAnchor {
                    doc_line: 2,
                    visual_line: 0,
                    first_visual_line: true,
                    origin: point(px(3.0), px(8.0)),
                },
                GutterLineAnchor {
                    doc_line: 2,
                    visual_line: 1,
                    first_visual_line: false,
                    origin: point(px(3.0), px(28.0)),
                },
            ]
        );

        state.set_gutter_line_anchors_from_plans(&[], point(px(0.0), px(0.0)));

        assert!(state.gutter_line_anchors().is_empty());
    }

    #[test]
    fn overlay_state_tracks_run_button_hits() {
        let state = EditorOverlayState::new();
        let hit = GutterRunButtonHit {
            doc_line: 12,
            bounds: gpui::Bounds::new(point(px(4.0), px(8.0)), size(px(14.0), px(14.0))),
        };

        state.set_gutter_run_button_hits(vec![hit]);

        assert_eq!(state.gutter_run_button_hits(), vec![hit]);
        assert_eq!(
            state.gutter_run_button_line_at(point(px(10.0), px(12.0))),
            Some(12)
        );
        assert_eq!(
            state.gutter_run_button_line_at(point(px(40.0), px(12.0))),
            None
        );

        state.clear_gutter_run_button_hits();

        assert!(state.gutter_run_button_hits().is_empty());
    }
}
