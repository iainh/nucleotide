// ABOUTME: Style conversion utilities for Helix text modifiers to GPUI rendering
// ABOUTME: Handles bold, italic, dim, reversed, and crossed_out modifiers

use gpui::{Font, FontStyle, FontWeight, Hsla, StrikethroughStyle, TextRun, UnderlineStyle, px};
use helix_view::graphics::{Modifier, Style};

/// Apply text modifiers to create a modified font variant
pub fn apply_font_modifiers(base_font: &Font, style: &Style) -> Font {
    let mut font = base_font.clone();

    // Check if bold modifier is added (and not subtracted)
    if style.add_modifier.contains(Modifier::BOLD) && !style.sub_modifier.contains(Modifier::BOLD) {
        font.weight = FontWeight::BOLD;
    }

    // Check if italic modifier is added (and not subtracted)
    if style.add_modifier.contains(Modifier::ITALIC)
        && !style.sub_modifier.contains(Modifier::ITALIC)
    {
        font.style = FontStyle::Italic;
    }

    font
}

/// Apply color modifiers (reversed, dim) to foreground and background colors
pub fn apply_color_modifiers(
    fg: Hsla,
    bg: Option<Hsla>,
    style: &Style,
    default_bg: Hsla,
) -> (Hsla, Option<Hsla>) {
    let mut fg = fg;
    let mut bg = bg;

    // Handle reversed modifier - swap fg and bg
    if style.add_modifier.contains(Modifier::REVERSED)
        && !style.sub_modifier.contains(Modifier::REVERSED)
    {
        let old_fg = fg;
        fg = bg.unwrap_or(default_bg);
        bg = Some(old_fg);
    }

    // Handle dim modifier - reduce opacity
    if style.add_modifier.contains(Modifier::DIM) && !style.sub_modifier.contains(Modifier::DIM) {
        fg.a *= 0.5;
    }

    (fg, bg)
}

/// Check if text should have strikethrough
pub fn should_strikethrough(style: &Style) -> bool {
    style.add_modifier.contains(Modifier::CROSSED_OUT)
        && !style.sub_modifier.contains(Modifier::CROSSED_OUT)
}

/// Create a TextRun with all modifiers applied
pub fn create_styled_text_run(
    text_len: usize,
    base_font: &Font,
    style: &Style,
    base_fg: Hsla,
    base_bg: Option<Hsla>,
    default_bg: Hsla,
    underline_color: Option<Hsla>,
) -> TextRun {
    // Apply font modifiers
    let font = apply_font_modifiers(base_font, style);

    // Apply color modifiers
    let (fg, bg) = apply_color_modifiers(base_fg, base_bg, style, default_bg);

    // Handle underline
    let underline = underline_color.map(|color| UnderlineStyle {
        thickness: px(1.),
        color: Some(color),
        wavy: true,
    });

    // Handle strikethrough
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
