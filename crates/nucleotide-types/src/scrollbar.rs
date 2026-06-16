//! Shared GPUI scrollbar geometry and visual constants.

use gpui::{Along, Axis, Bounds, Pixels, px};

pub const SCROLLBAR_THICKNESS: Pixels = px(12.0);
pub const SCROLLBAR_EDGE_PADDING: Pixels = px(2.0);
pub const SCROLLBAR_MIN_THUMB_LENGTH: Pixels = px(20.0);

pub const SCROLLBAR_COMPACT_WIDTH_RATIO: f32 = 0.35;
pub const SCROLLBAR_EXPANDED_WIDTH_RATIO: f32 = 0.70;

pub const SCROLLBAR_ALPHA_INACTIVE: f32 = 0.25;
pub const SCROLLBAR_ALPHA_TRACK_HOVER: f32 = 0.45;
pub const SCROLLBAR_ALPHA_THUMB_HOVER: f32 = 0.60;
pub const SCROLLBAR_ALPHA_DRAGGING: f32 = 0.75;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarThumb {
    pub start: Pixels,
    pub length: Pixels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarVisual {
    pub along_offset: Pixels,
    pub along_size: Pixels,
    pub cross_offset: Pixels,
    pub cross_size: Pixels,
}

pub fn scrollbar_width_ratio(is_expanded: bool) -> f32 {
    if is_expanded {
        SCROLLBAR_EXPANDED_WIDTH_RATIO
    } else {
        SCROLLBAR_COMPACT_WIDTH_RATIO
    }
}

pub fn scrollbar_padded_track_length(track_length: Pixels) -> Pixels {
    (track_length - SCROLLBAR_EDGE_PADDING * 2.0).max(px(0.0))
}

pub fn scrollbar_thumb(
    track_length: Pixels,
    viewport_length: Pixels,
    max_scroll: Pixels,
    scroll_position: Pixels,
) -> Option<ScrollbarThumb> {
    if track_length <= px(0.0) || viewport_length <= px(0.0) || max_scroll <= px(0.0) {
        return None;
    }

    let content_length = viewport_length + max_scroll;
    if content_length <= viewport_length {
        return None;
    }

    let thumb_length = (track_length * (viewport_length / content_length))
        .max(SCROLLBAR_MIN_THUMB_LENGTH)
        .min(track_length);
    let max_thumb_start = (track_length - thumb_length).max(px(0.0));
    let scroll_ratio = (scroll_position / max_scroll).clamp(0.0, 1.0);

    Some(ScrollbarThumb {
        start: max_thumb_start * scroll_ratio,
        length: thumb_length,
    })
}

pub fn scrollbar_visual(
    thumb: ScrollbarThumb,
    track_length: Pixels,
    width_ratio: f32,
) -> ScrollbarVisual {
    let padded_length = scrollbar_padded_track_length(track_length);
    let along_size = thumb.length.clamp(px(0.0), padded_length);
    let max_along_start = (padded_length - along_size).max(px(0.0));
    let along_offset = SCROLLBAR_EDGE_PADDING + thumb.start.clamp(px(0.0), max_along_start);
    let cross_size = SCROLLBAR_THICKNESS * width_ratio;
    let cross_offset = (SCROLLBAR_THICKNESS - cross_size) / 2.0;

    ScrollbarVisual {
        along_offset,
        along_size,
        cross_offset,
        cross_size,
    }
}

pub fn scrollbar_thumb_bounds(
    thumb: ScrollbarThumb,
    axis: Axis,
    track_bounds: Bounds<Pixels>,
    width_ratio: f32,
) -> Bounds<Pixels> {
    let visual = scrollbar_visual(thumb, track_bounds.size.along(axis), width_ratio);

    Bounds::new(
        track_bounds
            .origin
            .apply_along(axis, |origin| origin + visual.along_offset)
            .apply_along(axis.invert(), |origin| origin + visual.cross_offset),
        track_bounds
            .size
            .apply_along(axis, |_| visual.along_size)
            .apply_along(axis.invert(), |_| visual.cross_size),
    )
}

pub fn scrollbar_scroll_position_for_pointer(
    track_length: Pixels,
    max_scroll: Pixels,
    thumb: ScrollbarThumb,
    pointer: Pixels,
    drag_offset: Pixels,
) -> Pixels {
    let track_length = scrollbar_padded_track_length(track_length);
    let max_thumb_start = (track_length - thumb.length).max(px(0.0));
    if max_thumb_start <= px(0.0) || max_scroll <= px(0.0) {
        return px(0.0);
    }

    let thumb_start =
        (pointer - SCROLLBAR_EDGE_PADDING - drag_offset).clamp(px(0.0), max_thumb_start);
    max_scroll * (thumb_start / max_thumb_start)
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;

    #[test]
    fn thumb_scales_to_track_fraction() {
        let thumb = scrollbar_thumb(px(196.0), px(200.0), px(800.0), px(0.0)).unwrap();

        assert_eq!(
            thumb,
            ScrollbarThumb {
                start: px(0.0),
                length: px(39.2),
            }
        );
    }

    #[test]
    fn visual_applies_edge_padding_and_width_ratio() {
        let visual = scrollbar_visual(
            ScrollbarThumb {
                start: px(25.0),
                length: px(50.0),
            },
            px(104.0),
            0.5,
        );

        assert_eq!(visual.along_offset, px(27.0));
        assert_eq!(visual.along_size, px(50.0));
        assert_eq!(visual.cross_offset, px(3.0));
        assert_eq!(visual.cross_size, px(6.0));
    }

    #[test]
    fn bounds_are_axis_aware() {
        let thumb = ScrollbarThumb {
            start: px(10.0),
            length: px(30.0),
        };
        let bounds = scrollbar_thumb_bounds(
            thumb,
            Axis::Vertical,
            Bounds::new(point(px(5.0), px(7.0)), size(px(12.0), px(100.0))),
            0.5,
        );

        assert_eq!(bounds.origin, point(px(8.0), px(19.0)));
        assert_eq!(bounds.size, size(px(6.0), px(30.0)));
    }

    #[test]
    fn pointer_position_maps_to_scroll_position() {
        let thumb = ScrollbarThumb {
            start: px(0.0),
            length: px(40.0),
        };

        let scroll_y =
            scrollbar_scroll_position_for_pointer(px(204.0), px(800.0), thumb, px(102.0), px(20.0));

        assert_eq!(scroll_y, px(400.0));
    }
}
