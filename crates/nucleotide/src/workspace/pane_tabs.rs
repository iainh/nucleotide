//! Pointer geometry for pane edge drops. Membership lives only in PaneLayout.
use gpui::{Bounds, Pixels, Point, px};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaneDropEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl PaneDropEdge {
    pub(super) fn nearest(position: Point<Pixels>, bounds: Bounds<Pixels>) -> Option<Self> {
        if bounds.size.width <= px(0.0)
            || bounds.size.height <= px(0.0)
            || !bounds.contains(&position)
        {
            return None;
        }
        let x = (position.x - bounds.left()) / bounds.size.width;
        let y = (position.y - bounds.top()) / bounds.size.height;
        [
            (Self::Left, x),
            (Self::Right, 1.0 - x),
            (Self::Top, y),
            (Self::Bottom, 1.0 - y),
        ]
        .into_iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(edge, _)| edge)
    }

    pub(super) fn preview_bounds(self, mut bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        match self {
            Self::Left | Self::Right => {
                bounds.size.width /= 2.0;
                if self == Self::Right {
                    bounds.origin.x += bounds.size.width;
                }
            }
            Self::Top | Self::Bottom => {
                bounds.size.height /= 2.0;
                if self == Self::Bottom {
                    bounds.origin.y += bounds.size.height;
                }
            }
        }
        bounds
    }
}
