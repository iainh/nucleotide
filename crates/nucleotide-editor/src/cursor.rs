// ABOUTME: Native GPUI cursor painter for editor surfaces
// ABOUTME: Draws Helix cursor shapes and optional block-cursor text overlays

use std::borrow::Cow;

use gpui::{
    App, Bounds, Font, Hsla, Pixels, Point, ShapedLine, SharedString, TextRun, Window,
    WindowTextSystem, fill, px, size, white,
};
use helix_core::{RopeSlice, graphemes::next_grapheme_boundary};
use helix_view::graphics::{CursorKind, Style};
use nucleotide_logging::error;

use crate::{
    cursor_has_reversed_modifier,
    style::{create_styled_text_run, helix_color_to_hsla},
};

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

#[derive(Clone)]
pub struct CursorTextShape {
    pub shaped_line: Option<ShapedLine>,
    pub len: usize,
}

impl CursorTextShape {
    pub fn width_or(&self, fallback: Pixels) -> Pixels {
        self.shaped_line
            .as_ref()
            .map_or(fallback, |shaped| shaped.x_for_index(self.len))
    }

    pub fn into_shaped_line(self) -> Option<ShapedLine> {
        self.shaped_line
    }
}

pub fn cursor_text_run(
    base_font: &Font,
    text_len: usize,
    text_style_at_cursor: &Style,
    text_color: Hsla,
    default_bg: Hsla,
) -> TextRun {
    let underline_color = text_style_at_cursor
        .underline_color
        .and_then(helix_color_to_hsla);

    create_styled_text_run(
        text_len,
        base_font,
        text_style_at_cursor,
        text_color,
        None,
        default_bg,
        underline_color,
    )
}

pub fn block_cursor_text(
    text: RopeSlice<'_>,
    cursor_char_idx: usize,
    cursor_kind: CursorKind,
    is_focused: bool,
) -> Option<SharedString> {
    if !matches!(cursor_kind, CursorKind::Block)
        || !is_focused
        || cursor_char_idx >= text.len_chars()
    {
        return None;
    }

    let grapheme_end = next_grapheme_boundary(text, cursor_char_idx);
    let char_slice = text.slice(cursor_char_idx..grapheme_end);
    let char_text: Cow<'_, str> = char_slice.into();
    let char_text = match char_text.as_ref() {
        "\n" | "\r\n" | "\r" => " ".into(),
        _ => SharedString::from(char_text.into_owned()),
    };

    (!char_text.is_empty()).then_some(char_text)
}

pub fn cursor_foreground_color(cursor_style: &Style, has_reversed: bool, default_bg: Hsla) -> Hsla {
    if has_reversed {
        default_bg
    } else if let Some(fg) = cursor_style.fg {
        helix_color_to_hsla(fg).unwrap_or_else(white)
    } else {
        white()
    }
}

pub fn cursor_background_color(
    cursor_style: &Style,
    text_style_at_cursor: &Style,
    fallback_fg: Hsla,
) -> Hsla {
    if cursor_has_reversed_modifier(cursor_style) {
        text_style_at_cursor
            .fg
            .and_then(helix_color_to_hsla)
            .unwrap_or(fallback_fg)
    } else {
        cursor_style
            .bg
            .and_then(helix_color_to_hsla)
            .or_else(|| cursor_style.fg.and_then(helix_color_to_hsla))
            .unwrap_or(fallback_fg)
    }
}

pub fn shape_cursor_text(
    text_system: &WindowTextSystem,
    text: Option<SharedString>,
    font: &Font,
    font_size: Pixels,
    text_style_at_cursor: &Style,
    text_color: Hsla,
    default_bg: Hsla,
) -> CursorTextShape {
    let Some(text) = text else {
        return CursorTextShape {
            shaped_line: None,
            len: 0,
        };
    };

    let len = text.len();
    let run = cursor_text_run(font, len, text_style_at_cursor, text_color, default_bg);
    let shaped_line = text_system.shape_line(text, font_size, &[run], None);

    CursorTextShape {
        shaped_line: Some(shaped_line),
        len,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{black, hsla, point, px, size};
    use helix_view::graphics::{Color, Modifier};

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

    #[test]
    fn block_cursor_text_uses_space_for_newlines() {
        assert_eq!(
            block_cursor_text("\n".into(), 0, CursorKind::Block, true).map(|text| text.to_string()),
            Some(" ".to_string())
        );
    }

    #[test]
    fn block_cursor_text_uses_grapheme_under_cursor() {
        assert_eq!(
            block_cursor_text("abc".into(), 1, CursorKind::Block, true)
                .map(|text| text.to_string()),
            Some("b".to_string())
        );
    }

    #[test]
    fn block_cursor_text_requires_block_cursor_and_focus() {
        assert!(block_cursor_text("abc".into(), 1, CursorKind::Bar, true).is_none());
        assert!(block_cursor_text("abc".into(), 1, CursorKind::Block, false).is_none());
    }

    #[test]
    fn cursor_foreground_uses_background_when_reversed() {
        let bg = hsla(0.2, 0.3, 0.4, 1.0);

        assert_eq!(cursor_foreground_color(&Style::default(), true, bg), bg);
    }

    #[test]
    fn cursor_background_uses_text_style_when_reversed() {
        let cursor_style = Style::default().add_modifier(Modifier::REVERSED);
        let text_style = Style::default().fg(Color::Rgb(255, 0, 0));

        assert_ne!(
            cursor_background_color(&cursor_style, &text_style, black()),
            black()
        );
    }
}
