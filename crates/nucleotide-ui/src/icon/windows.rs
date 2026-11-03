// ABOUTME: Paints system icon glyphs through GPUI's native DirectWrite text system.
// ABOUTME: Uses Segoe Fluent Icons on Windows 11 and Segoe MDL2 Assets on Windows 10.

use gpui::{App, Bounds, Hsla, Pixels, TextAlign, TextRun, Window, font, point};
use std::sync::OnceLock;

pub(super) fn paint(
    glyph: char,
    bounds: Bounds<Pixels>,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    static FAMILY: OnceLock<Option<&'static str>> = OnceLock::new();
    let Some(family) = FAMILY.get_or_init(|| {
        let names = window.text_system().all_font_names();
        ["Segoe Fluent Icons", "Segoe MDL2 Assets"]
            .into_iter()
            .find(|family| names.iter().any(|name| name == family))
    }) else {
        return false;
    };
    let font = font(*family);
    // System glyphs fill their em square more densely than the bundled artwork.
    // Inset the ink, not the element: spacing and pointer targets stay unchanged.
    let font_size = (bounds.size.width.min(bounds.size.height) * 0.8).round();
    let font_id = window.text_system().resolve_font(&font);
    // Never paint a missing-glyph box on older systems.
    if window
        .text_system()
        .advance(font_id, font_size, glyph)
        .is_err()
    {
        return false;
    }
    let text = glyph.to_string();
    let line = window.text_system().shape_line(
        text.clone().into(),
        font_size,
        &[TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    line.paint(
        point(
            bounds.left(),
            bounds.top() + (bounds.size.height - font_size) / 2.0,
        ),
        font_size,
        TextAlign::Center,
        Some(bounds.size.width),
        window,
        cx,
    )
    .is_ok()
}
