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

/// Geometry and interactions from Microsoft's ScrollBar_themeresources.xaml.
/// Keep the themed scrollbar contract unchanged on other looks/platforms.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarStyle {
    #[default]
    Themed,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarPart {
    Decrease,
    PageDecrease,
    Thumb,
    PageIncrease,
    Increase,
}

impl ScrollbarPart {
    pub fn scroll_position(
        self,
        current: Pixels,
        maximum: Pixels,
        viewport: Pixels,
        line: Pixels,
    ) -> Pixels {
        let delta = match self {
            Self::Decrease => -line,
            Self::PageDecrease => -viewport,
            Self::Thumb => px(0.0),
            Self::PageIncrease => viewport,
            Self::Increase => line,
        };
        (current + delta).clamp(px(0.0), maximum)
    }

    /// Segoe Fluent Icons' Caret*Solid8 glyphs used by the WinUI template.
    pub fn arrow_glyph(self, axis: Axis) -> &'static str {
        match (self, axis) {
            (Self::Decrease, Axis::Vertical) => "\u{eddb}",
            (Self::Increase, Axis::Vertical) => "\u{eddc}",
            (Self::Decrease, Axis::Horizontal) => "\u{edd9}",
            (Self::Increase, Axis::Horizontal) => "\u{edda}",
            _ => "",
        }
    }
}

impl ScrollbarStyle {
    pub fn thumb(
        self,
        track: Pixels,
        viewport: Pixels,
        maximum: Pixels,
        position: Pixels,
    ) -> Option<ScrollbarThumb> {
        if self == Self::Themed {
            return scrollbar_thumb(
                scrollbar_padded_track_length(track),
                viewport,
                maximum,
                position,
            );
        }
        let length = (track - SCROLLBAR_THICKNESS * 2.0).max(px(0.0));
        let mut thumb = scrollbar_thumb(length, viewport, maximum, position)?;
        thumb.length = thumb.length.max(px(30.0)).min(length);
        thumb.start = (length - thumb.length) * (position / maximum).clamp(0.0, 1.0);
        Some(thumb)
    }

    pub fn visual(self, thumb: ScrollbarThumb, track: Pixels, expanded: bool) -> ScrollbarVisual {
        if self == Self::Themed {
            return scrollbar_visual(thumb, track, scrollbar_width_ratio(expanded));
        }
        // WinUI's 12px thumb has a 6px transparent stroke (3px per edge).
        // The collapsed mouse thumb is 8px wide, translated 2px: its 2px
        // visible fill remains centred on the expanded thumb.
        ScrollbarVisual {
            along_offset: SCROLLBAR_THICKNESS + thumb.start + px(3.0),
            along_size: (thumb.length - px(6.0)).max(px(0.0)),
            cross_offset: if expanded { px(3.0) } else { px(5.0) },
            cross_size: if expanded { px(6.0) } else { px(2.0) },
        }
    }

    pub fn thumb_bounds(
        self,
        thumb: ScrollbarThumb,
        axis: Axis,
        bounds: Bounds<Pixels>,
        expanded: bool,
    ) -> Bounds<Pixels> {
        if self == Self::Themed {
            return scrollbar_thumb_bounds(thumb, axis, bounds, scrollbar_width_ratio(expanded));
        }
        // The transparent stroke belongs to the thumb's hit target.
        Bounds::new(
            bounds
                .origin
                .apply_along(axis, |p| p + SCROLLBAR_THICKNESS + thumb.start),
            bounds.size.apply_along(axis, |_| thumb.length),
        )
    }

    pub fn scroll_position(
        self,
        track: Pixels,
        maximum: Pixels,
        thumb: ScrollbarThumb,
        pointer: Pixels,
        drag_offset: Pixels,
    ) -> Pixels {
        if self == Self::Themed {
            return scrollbar_scroll_position_for_pointer(
                track,
                maximum,
                thumb,
                pointer,
                drag_offset,
            );
        }
        let travel = (track - SCROLLBAR_THICKNESS * 2.0 - thumb.length).max(px(0.0));
        if travel <= px(0.0) {
            return px(0.0);
        }
        maximum * ((pointer - SCROLLBAR_THICKNESS - drag_offset) / travel).clamp(0.0, 1.0)
    }

    pub fn part(self, thumb: ScrollbarThumb, track: Pixels, pointer: Pixels) -> ScrollbarPart {
        if pointer < SCROLLBAR_THICKNESS {
            ScrollbarPart::Decrease
        } else if pointer >= track - SCROLLBAR_THICKNESS {
            ScrollbarPart::Increase
        } else if pointer < SCROLLBAR_THICKNESS + thumb.start {
            ScrollbarPart::PageDecrease
        } else if pointer >= SCROLLBAR_THICKNESS + thumb.start + thumb.length {
            ScrollbarPart::PageIncrease
        } else {
            ScrollbarPart::Thumb
        }
    }
}

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

    #[test]
    fn windows_thumb_reserves_arrows_and_drag_reaches_both_ends() {
        let style = ScrollbarStyle::Windows;
        let thumb = style
            .thumb(px(224.0), px(100.0), px(900.0), px(225.0))
            .unwrap();
        assert_eq!(
            thumb,
            ScrollbarThumb {
                start: px(42.5),
                length: px(30.0)
            }
        );
        let compact = style.visual(thumb, px(224.0), false);
        let expanded = style.visual(thumb, px(224.0), true);
        assert_eq!(compact.cross_size, px(2.0));
        assert_eq!(compact.cross_offset, px(5.0));
        assert_eq!(expanded.cross_size, px(6.0));
        assert_eq!(expanded.along_offset, px(57.5));
        assert_eq!(expanded.along_size, px(24.0));
        for (pointer, expected) in [
            (px(-20.0), px(0.0)),
            (px(59.5), px(225.0)),
            (px(250.0), px(900.0)),
        ] {
            assert_eq!(
                style.scroll_position(px(224.0), px(900.0), thumb, pointer, px(5.0)),
                expected
            );
        }
        for axis in [Axis::Vertical, Axis::Horizontal] {
            let track_size = if axis == Axis::Vertical {
                size(px(12.0), px(224.0))
            } else {
                size(px(224.0), px(12.0))
            };
            let bounds = Bounds::new(point(px(7.0), px(11.0)), track_size);
            let hit = style.thumb_bounds(thumb, axis, bounds, false);
            assert_eq!(hit.origin.along(axis), bounds.origin.along(axis) + px(54.5));
            assert_eq!(hit.size.along(axis), px(30.0));
            assert_eq!(hit.size.along(axis.invert()), px(12.0));
        }
        assert_eq!(style.thumb(px(24.0), px(100.0), px(900.0), px(0.0)), None);
        assert_eq!(
            style
                .thumb(px(44.0), px(100.0), px(900.0), px(1000.0))
                .unwrap(),
            ScrollbarThumb {
                start: px(0.0),
                length: px(20.0)
            }
        );
    }

    #[test]
    fn windows_arrow_and_page_regions_have_distinct_clamped_actions() {
        let style = ScrollbarStyle::Windows;
        let thumb = ScrollbarThumb {
            start: px(42.5),
            length: px(30.0),
        };
        for (pointer, expected) in [
            (px(11.0), ScrollbarPart::Decrease),
            (px(12.0), ScrollbarPart::PageDecrease),
            (px(54.5), ScrollbarPart::Thumb),
            (px(84.5), ScrollbarPart::PageIncrease),
            (px(211.0), ScrollbarPart::PageIncrease),
            (px(212.0), ScrollbarPart::Increase),
        ] {
            assert_eq!(style.part(thumb, px(224.0), pointer), expected);
        }
        for (part, expected) in [
            (ScrollbarPart::Decrease, 208.0),
            (ScrollbarPart::PageDecrease, 125.0),
            (ScrollbarPart::PageIncrease, 325.0),
            (ScrollbarPart::Increase, 242.0),
        ] {
            assert_eq!(
                part.scroll_position(px(225.0), px(900.0), px(100.0), px(17.0)),
                px(expected)
            );
        }
        assert_eq!(
            ScrollbarPart::PageDecrease.scroll_position(px(10.0), px(900.0), px(100.0), px(17.0)),
            px(0.0)
        );
        assert_eq!(
            ScrollbarPart::Increase.scroll_position(px(899.0), px(900.0), px(100.0), px(17.0)),
            px(900.0)
        );
    }
}
