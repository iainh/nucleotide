// ABOUTME: Native editor overlay paint state shared with GPUI integration
// ABOUTME: Tracks cursor overlay bounds and gutter width from frame painting

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Pixels, Point, Size, px};

use crate::CursorOverlayPlan;

#[derive(Clone, Debug)]
pub struct EditorOverlayState {
    cursor_position: Rc<Cell<Option<Point<Pixels>>>>,
    cursor_size: Rc<Cell<Option<Size<Pixels>>>>,
    gutter_width: Rc<Cell<Pixels>>,
}

impl Default for EditorOverlayState {
    fn default() -> Self {
        Self {
            cursor_position: Rc::new(Cell::new(None)),
            cursor_size: Rc::new(Cell::new(None)),
            gutter_width: Rc::new(Cell::new(px(0.0))),
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

        assert_eq!(state.gutter_width(), px(48.0));
    }
}
