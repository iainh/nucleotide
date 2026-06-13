// ABOUTME: Native GPUI cursor painter for editor surfaces
// ABOUTME: Draws Helix cursor shapes and optional block-cursor text overlays

use gpui::{App, Bounds, Hsla, Pixels, Point, ShapedLine, Window, fill, px, size};
use helix_view::graphics::CursorKind;
use nucleotide_logging::error;

pub struct EditorCursor {
    pub origin: Point<Pixels>,
    pub kind: CursorKind,
    pub color: Hsla,
    pub block_width: Pixels,
    pub line_height: Pixels,
    pub text: Option<ShapedLine>,
}

impl EditorCursor {
    pub fn bounds(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        match self.kind {
            CursorKind::Bar => Bounds {
                origin: self.origin + origin,
                size: size(px(2.0), self.line_height),
            },
            CursorKind::Block => Bounds {
                origin: self.origin + origin,
                size: size(self.block_width, self.line_height),
            },
            CursorKind::Underline => Bounds {
                origin: self.origin + origin + Point::new(Pixels::ZERO, self.line_height - px(2.0)),
                size: size(self.block_width, px(2.0)),
            },
            CursorKind::Hidden => Bounds {
                origin: self.origin + origin,
                size: size(px(0.0), px(0.0)),
            },
        }
    }

    pub fn paint(&mut self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = self.bounds(origin);
        window.paint_quad(fill(bounds, self.color));

        if let Some(text) = &self.text
            && let Err(error) = text.paint(bounds.origin, self.line_height, window, cx)
        {
            error!(error = ?error, "Failed to paint cursor text");
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{black, point, px, size};

    use super::*;

    fn test_cursor(kind: CursorKind) -> EditorCursor {
        EditorCursor {
            origin: point(px(3.0), px(5.0)),
            kind,
            color: black(),
            block_width: px(8.0),
            line_height: px(20.0),
            text: None,
        }
    }

    #[test]
    fn block_cursor_uses_configured_cell_width() {
        assert_eq!(
            test_cursor(CursorKind::Block).bounds(point(px(10.0), px(20.0))),
            Bounds {
                origin: point(px(13.0), px(25.0)),
                size: size(px(8.0), px(20.0)),
            }
        );
    }

    #[test]
    fn bar_cursor_uses_fixed_two_pixel_width() {
        assert_eq!(
            test_cursor(CursorKind::Bar).bounds(point(px(10.0), px(20.0))),
            Bounds {
                origin: point(px(13.0), px(25.0)),
                size: size(px(2.0), px(20.0)),
            }
        );
    }

    #[test]
    fn underline_cursor_sits_at_line_bottom() {
        assert_eq!(
            test_cursor(CursorKind::Underline).bounds(point(px(10.0), px(20.0))),
            Bounds {
                origin: point(px(13.0), px(43.0)),
                size: size(px(8.0), px(2.0)),
            }
        );
    }
}
