// ABOUTME: Native editor font and cell metrics
// ABOUTME: Resolves GPUI text style measurements used by editor layout and input surfaces

use gpui::{Bounds, Pixels, TextStyle, TextSystem, px};

use crate::EditorLayout;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorTextMetrics {
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub em_width: Pixels,
    pub cell_width: Pixels,
}

impl EditorTextMetrics {
    pub fn resolve(text_system: &TextSystem, style: &TextStyle) -> Self {
        let font_id = text_system.resolve_font(&style.font());
        let font_size = style.font_size.to_pixels(px(16.0));
        let line_height = style.line_height_in_pixels(font_size);
        let em_width = text_system
            .typographic_bounds(font_id, font_size, 'm')
            .map(|bounds| bounds.size.width)
            .unwrap_or(px(8.0));
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|advance| advance.width)
            .unwrap_or(em_width);

        Self {
            font_size,
            line_height,
            em_width,
            cell_width,
        }
    }

    pub fn layout_for_bounds(&self, bounds: Bounds<Pixels>) -> EditorLayout {
        let columns = ((bounds.size.width / self.em_width).floor() as usize).max(1);
        let rows = ((bounds.size.height / self.line_height).floor() as usize).max(1);

        EditorLayout {
            rows,
            columns,
            line_height: self.line_height,
            font_size: self.font_size,
            cell_width: self.cell_width,
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};

    use super::*;

    #[test]
    fn layout_for_bounds_uses_em_width_and_line_height() {
        let metrics = EditorTextMetrics {
            font_size: px(16.0),
            line_height: px(20.0),
            em_width: px(8.0),
            cell_width: px(9.0),
        };

        let layout = metrics.layout_for_bounds(Bounds::new(
            point(px(0.0), px(0.0)),
            size(px(81.0), px(41.0)),
        ));

        assert_eq!(layout.columns, 10);
        assert_eq!(layout.rows, 2);
        assert_eq!(layout.cell_width, px(9.0));
    }

    #[test]
    fn layout_for_bounds_keeps_minimum_row_and_column() {
        let metrics = EditorTextMetrics {
            font_size: px(16.0),
            line_height: px(20.0),
            em_width: px(8.0),
            cell_width: px(8.0),
        };

        let layout =
            metrics.layout_for_bounds(Bounds::new(point(px(0.0), px(0.0)), size(px(0.0), px(0.0))));

        assert_eq!(layout.columns, 1);
        assert_eq!(layout.rows, 1);
    }
}
