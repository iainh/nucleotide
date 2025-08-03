// ABOUTME: This file implements the KeyHintView component that displays keybinding hints
// ABOUTME: when a pending keymap state is active (e.g., after pressing space leader)

use gpui::{
    div, px, rgb, AnyElement, Context, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, Hsla,
};
use helix_view::{
    info::Info,
    theme::Theme,
};

const PADDING: f32 = 8.0;
const LINE_HEIGHT: f32 = 20.0;
const CHAR_WIDTH: f32 = 9.0;

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
        use crate::utils::color_to_hsla;
        
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
            "ui.popup" => rgb(0x2a2a3e).into(), // Slightly lighter than pure black
            "ui.popup.info" => rgb(0x3b3b52).into(), // Even lighter for info popups
            "ui.window" => rgb(0x414559).into(), // Border color
            "ui.text.info" | "ui.text" => rgb(0xc6d0f5).into(),
            _ => rgb(0xc6d0f5).into(),
        }
    }

    fn render_line(&self, line: &str, _cx: &mut Context<Self>) -> AnyElement {
        // Don't trim the line yet - we need to preserve spacing
        let clean_line = line.replace('\n', " ").replace('\r', " ");
        
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
                            .w(px(30.0))
                    )
                    .child(
                        div()
                            .text_color(self.get_theme_color("ui.text.info"))
                            .child(desc)
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

    fn calculate_dimensions(&self) -> (f32, f32) {
        if let Some(info) = &self.info {
            let mut max_width: f32 = 0.0;
            let mut total_height = PADDING * 2.0;

            // Add title height if present
            if !info.title.is_empty() {
                max_width = max_width.max(info.title.len() as f32 * CHAR_WIDTH);
                total_height += LINE_HEIGHT + PADDING;
            }

            // Calculate dimensions for text lines
            let lines_count = info.text.lines().count();
            total_height += lines_count as f32 * LINE_HEIGHT;
            
            // Use the info's width property
            max_width = max_width.max(info.width as f32 * CHAR_WIDTH);

            (max_width + PADDING * 2.0, total_height)
        } else {
            (0.0, 0.0)
        }
    }
}

impl Render for KeyHintView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(info) = &self.info {
            let (width, height) = self.calculate_dimensions();
            
            let bg_color = self.get_theme_color("ui.popup.info");
            let border_color = self.get_theme_color("ui.window");
            let title_color = self.get_theme_color("ui.text.info");
            
            // Clean title
            let clean_title = info.title.replace('\n', " ").replace('\r', " ").trim().to_string();
            
            div()
                .absolute()
                .bottom_4()
                .right_4()
                .w(px(width))
                .h(px(height))
                .bg(bg_color)
                .border_2()
                .border_color(border_color)
                .rounded_md()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
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
                                .child(clean_title)
                        )
                    } else {
                        None
                    }
                )
                .children(
                    // Content lines
                    info.text.lines().map(|line| {
                        self.render_line(line, cx)
                    })
                )
        } else {
            div().w_0().h_0()
        }
    }
}

impl EventEmitter<()> for KeyHintView {}