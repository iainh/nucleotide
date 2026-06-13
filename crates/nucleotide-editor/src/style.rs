// ABOUTME: Shared Helix-style to GPUI text-run conversion helpers
// ABOUTME: Applies terminal-style modifiers to native GPUI text rendering

use gpui::{
    Font, FontStyle, FontWeight, Hsla, Rgba, StrikethroughStyle, TextRun, UnderlineStyle, px,
};
use helix_view::graphics::{Color, Modifier, Style};

pub(crate) fn apply_font_modifiers(base_font: &Font, style: &Style) -> Font {
    let mut font = base_font.clone();

    if style.add_modifier.contains(Modifier::BOLD) && !style.sub_modifier.contains(Modifier::BOLD) {
        font.weight = FontWeight::BOLD;
    }

    if style.add_modifier.contains(Modifier::ITALIC)
        && !style.sub_modifier.contains(Modifier::ITALIC)
    {
        font.style = FontStyle::Italic;
    }

    font
}

pub(crate) fn apply_color_modifiers(
    fg: Hsla,
    bg: Option<Hsla>,
    style: &Style,
    default_bg: Hsla,
) -> (Hsla, Option<Hsla>) {
    let mut fg = fg;
    let mut bg = bg;

    if style.add_modifier.contains(Modifier::REVERSED)
        && !style.sub_modifier.contains(Modifier::REVERSED)
    {
        let old_fg = fg;
        fg = bg.unwrap_or(default_bg);
        bg = Some(old_fg);
    }

    if style.add_modifier.contains(Modifier::DIM) && !style.sub_modifier.contains(Modifier::DIM) {
        fg.a *= 0.5;
    }

    (fg, bg)
}

pub(crate) fn should_strikethrough(style: &Style) -> bool {
    style.add_modifier.contains(Modifier::CROSSED_OUT)
        && !style.sub_modifier.contains(Modifier::CROSSED_OUT)
}

pub(crate) fn create_styled_text_run(
    text_len: usize,
    base_font: &Font,
    style: &Style,
    base_fg: Hsla,
    base_bg: Option<Hsla>,
    default_bg: Hsla,
    underline_color: Option<Hsla>,
) -> TextRun {
    let font = apply_font_modifiers(base_font, style);
    let (fg, bg) = apply_color_modifiers(base_fg, base_bg, style, default_bg);

    let underline = underline_color.map(|color| UnderlineStyle {
        thickness: px(1.),
        color: Some(color),
        wavy: true,
    });

    let strikethrough = if should_strikethrough(style) {
        Some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(fg),
        })
    } else {
        None
    };

    TextRun {
        len: text_len,
        font,
        color: fg,
        background_color: bg,
        underline,
        strikethrough,
    }
}

pub(crate) fn helix_color_to_hsla(color: Color) -> Option<Hsla> {
    match color {
        Color::Rgb(r, g, b) => Some(Hsla::from(Rgba {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: 1.0,
        })),
        Color::Indexed(index) => {
            let (r, g, b) = match index {
                0 => (0, 0, 0),
                1 => (128, 0, 0),
                2 => (0, 128, 0),
                3 => (128, 128, 0),
                4 => (0, 0, 128),
                5 => (128, 0, 128),
                6 => (0, 128, 128),
                7 => (192, 192, 192),
                8 => (128, 128, 128),
                9 => (255, 0, 0),
                10 => (0, 255, 0),
                11 => (255, 255, 0),
                12 => (0, 0, 255),
                13 => (255, 0, 255),
                14 => (0, 255, 255),
                15 => (255, 255, 255),
                _ => (128, 128, 128),
            };
            Some(Hsla::from(Rgba {
                r: r as f32 / 255.0,
                g: g as f32 / 255.0,
                b: b as f32 / 255.0,
                a: 1.0,
            }))
        }
        Color::Black => Some(Hsla::from(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })),
        Color::Red => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })),
        Color::Green => Some(Hsla::from(Rgba {
            r: 0.0,
            g: 0.5,
            b: 0.0,
            a: 1.0,
        })),
        Color::Yellow => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.0,
            a: 1.0,
        })),
        Color::Blue => Some(Hsla::from(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.5,
            a: 1.0,
        })),
        Color::Magenta => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.0,
            b: 0.5,
            a: 1.0,
        })),
        Color::Cyan => Some(Hsla::from(Rgba {
            r: 0.0,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        })),
        Color::Gray => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        })),
        Color::White => Some(Hsla::from(Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        })),
        Color::Reset => None,
        _ => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        })),
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Font, FontWeight, hsla};
    use helix_view::graphics::{Modifier, Style};

    use super::{apply_color_modifiers, apply_font_modifiers};

    #[test]
    fn applies_font_modifiers() {
        let font = Font {
            family: ".SystemUIFont".into(),
            features: gpui::FontFeatures::default(),
            weight: FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
            fallbacks: None,
        };
        let style = Style::default()
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::ITALIC);

        let modified = apply_font_modifiers(&font, &style);

        assert_eq!(modified.weight, FontWeight::BOLD);
        assert_eq!(modified.style, gpui::FontStyle::Italic);
    }

    #[test]
    fn reversed_modifier_swaps_foreground_and_background() {
        let fg = hsla(0.0, 1.0, 0.5, 1.0);
        let bg = hsla(0.5, 1.0, 0.5, 1.0);
        let default_bg = hsla(0.0, 0.0, 0.0, 1.0);
        let style = Style::default().add_modifier(Modifier::REVERSED);

        let (actual_fg, actual_bg) = apply_color_modifiers(fg, Some(bg), &style, default_bg);

        assert_eq!(actual_fg, bg);
        assert_eq!(actual_bg, Some(fg));
    }
}
