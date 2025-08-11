// ABOUTME: This file implements the KeyHintView component that displays keybinding hints
// ABOUTME: when a pending keymap state is active (e.g., after pressing space leader)

use gpui::{
    div, px, rgb, AnyElement, Context, EventEmitter, Hsla, IntoElement, ParentElement, Render,
    Styled, Window,
};
use helix_view::{info::Info, theme::Theme};

#[derive(Debug)]
pub struct KeyHintView {
    info: Option<Info>,
    theme: Option<Theme>,
}

impl KeyHintView {
    pub fn new() -> Self {
        Self {
            info: None,
            theme: None,
        }
    }

    pub fn set_info(&mut self, info: Option<Info>) {
        self.info = info;
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = Some(theme);
    }

    pub fn has_info(&self) -> bool {
        self.info.is_some()
    }

    fn get_theme_color(&self, key: &str) -> Hsla {
        use nucleotide_ui::theme_utils::color_to_hsla;

        if let Some(theme) = &self.theme {
            let style = theme.get(key);

            // For popup backgrounds, use bg color
            if key.contains("popup") || key == "ui.window" {
                if let Some(color) = style.bg {
                    if let Some(hsla) = color_to_hsla(color) {
                        return hsla;
                    }
                }
            }

            // For text and other elements, use fg color
            if let Some(color) = style.fg {
                if let Some(hsla) = color_to_hsla(color) {
                    return hsla;
                }
            }
        }

        // Fallback colors - use lighter backgrounds for popups
        match key {
            "ui.popup" => rgb(0x002a_2a3e).into(), // Slightly lighter than pure black
            "ui.popup.info" => rgb(0x003b_3b52).into(), // Even lighter for info popups
            "ui.window" => rgb(0x0041_4559).into(), // Border color
            "ui.text.info" | "ui.text" => rgb(0x00c6_d0f5).into(),
            _ => rgb(0x00c6_d0f5).into(),
        }
    }

    fn render_line(&self, line: &str, _cx: &mut Context<Self>) -> AnyElement {
        // Don't trim the line yet - we need to preserve spacing
        let clean_line = line.replace(['\n', '\r'], " ");

        // The format from Info has keys padded to a fixed width followed by description
        // Let's find where the description starts by looking for the first letter after spaces
        let mut key_end = 0;
        let mut in_spaces = false;
        let chars: Vec<char> = clean_line.chars().collect();

        for (i, &ch) in chars.iter().enumerate() {
            if ch == ' ' {
                if !in_spaces && i > 0 {
                    key_end = i;
                    in_spaces = true;
                }
            } else if in_spaces {
                // Found start of description
                let key = clean_line[..key_end].trim().to_string();
                let desc = clean_line[i..].trim().to_string();

                if key.is_empty() {
                    return div()
                        .text_color(self.get_theme_color("ui.text.info"))
                        .child(desc)
                        .into_any_element();
                }

                // Render key in one color, description in another
                return div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .child(
                        div()
                            .text_color(self.get_theme_color("ui.text.info"))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(key)
                            .w(px(30.0)),
                    )
                    .child(
                        div()
                            .text_color(self.get_theme_color("ui.text.info"))
                            .child(desc),
                    )
                    .into_any_element();
            }
        }

        // If we didn't find a proper separator, just render as plain text
        div()
            .text_color(self.get_theme_color("ui.text.info"))
            .child(clean_line.trim().to_string())
            .into_any_element()
    }
}

impl Render for KeyHintView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(info) = &self.info {
            let bg_color = self.get_theme_color("ui.popup.info");
            let border_color = self.get_theme_color("ui.window");
            let title_color = self.get_theme_color("ui.text.info");

            // Clean title
            let clean_title = info.title.replace(['\n', '\r'], " ").trim().to_string();

            div()
                .absolute()
                .bottom_4()
                .right_4()
                .bg(bg_color)
                .border_2()
                .border_color(border_color)
                .rounded_md()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .font(cx.global::<crate::FontSettings>().var_font.clone())
                .text_size(px(cx.global::<crate::UiFontConfig>().size - 1.0))
                .children(
                    // Title if present
                    if !clean_title.is_empty() {
                        Some(
                            div()
                                .text_color(title_color)
                                .font_weight(gpui::FontWeight::BOLD)
                                .border_b_1()
                                .border_color(border_color)
                                .pb_1()
                                .mb_1()
                                .child(clean_title),
                        )
                    } else {
                        None
                    },
                )
                .children(
                    // Content lines
                    info.text.lines().map(|line| self.render_line(line, cx)),
                )
        } else {
            div().w_0().h_0()
        }
    }
}

impl EventEmitter<()> for KeyHintView {}
