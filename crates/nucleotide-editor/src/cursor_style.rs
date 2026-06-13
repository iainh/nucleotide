// ABOUTME: Cursor theme style resolution shared by native editor render paths
// ABOUTME: Keeps mode-specific Helix style patching out of app painting code

use helix_view::{
    document::Mode,
    graphics::{Modifier, Style},
};

pub fn cursor_style_for_mode(mode: Mode, mut style_for_key: impl FnMut(&str) -> Style) -> Style {
    let base_cursor_style = style_for_key("ui.cursor");
    let base_primary_cursor_style = style_for_key("ui.cursor.primary");
    let mode_style = style_for_key(cursor_style_key_for_mode(mode));

    if mode_style.fg.is_some() || mode_style.bg.is_some() {
        base_cursor_style.patch(mode_style)
    } else {
        base_cursor_style.patch(base_primary_cursor_style)
    }
}

pub fn cursor_has_reversed_modifier(style: &Style) -> bool {
    style.add_modifier.contains(Modifier::REVERSED)
        && !style.sub_modifier.contains(Modifier::REVERSED)
}

fn cursor_style_key_for_mode(mode: Mode) -> &'static str {
    match mode {
        Mode::Insert => "ui.cursor.primary.insert",
        Mode::Select => "ui.cursor.primary.select",
        Mode::Normal => "ui.cursor.primary.normal",
    }
}

#[cfg(test)]
mod tests {
    use super::{cursor_has_reversed_modifier, cursor_style_for_mode};
    use helix_view::{
        document::Mode,
        graphics::{Color, Modifier, Style},
    };

    fn style_for_key(key: &str) -> Style {
        match key {
            "ui.cursor" => Style::default().add_modifier(Modifier::BOLD),
            "ui.cursor.primary" => Style::default().bg(Color::Rgb(10, 20, 30)),
            "ui.cursor.primary.insert" => Style::default().fg(Color::Rgb(1, 2, 3)),
            "ui.cursor.primary.select" => Style::default(),
            "ui.cursor.primary.normal" => Style::default(),
            _ => Style::default(),
        }
    }

    #[test]
    fn patches_mode_specific_style_when_color_is_present() {
        let style = cursor_style_for_mode(Mode::Insert, style_for_key);

        assert_eq!(style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(style.bg, None);
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn falls_back_to_primary_style_without_mode_color() {
        let style = cursor_style_for_mode(Mode::Select, style_for_key);

        assert_eq!(style.fg, None);
        assert_eq!(style.bg, Some(Color::Rgb(10, 20, 30)));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn detects_effective_reversed_modifier() {
        let reversed = Style::default().add_modifier(Modifier::REVERSED);
        assert!(cursor_has_reversed_modifier(&reversed));

        let cancelled = Style::default()
            .add_modifier(Modifier::REVERSED)
            .remove_modifier(Modifier::REVERSED);
        assert!(!cursor_has_reversed_modifier(&cancelled));
    }
}
