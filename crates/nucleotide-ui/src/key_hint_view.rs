// ABOUTME: This file implements the KeyHintView component that displays keybinding hints
// ABOUTME: when a pending keymap state is active (e.g., after pressing space leader)

use crate::{Theme, styling::ColorTheory};
use gpui::{
    AnyElement, Context, EventEmitter, IntoElement, Keystroke, ParentElement, Render, Styled,
    Window, div, prelude::FluentBuilder, px, relative,
};
use helix_view::info::Info;

#[derive(Debug, Clone, PartialEq, Eq)]
struct HintLine {
    key: Option<String>,
    description: String,
}

const HINT_COLUMN_WIDTH: f32 = 292.0;
const HINT_COLUMN_GAP: f32 = 14.0;
const HINT_KEY_SLOT_WIDTH: f32 = 64.0;

#[derive(Debug)]
pub struct KeyHintView {
    info: Option<Info>,
}

impl Default for KeyHintView {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyHintView {
    pub fn new() -> Self {
        Self { info: None }
    }

    pub fn set_info(&mut self, info: Option<Info>) {
        self.info = info;
    }

    pub fn has_info(&self) -> bool {
        self.info.is_some()
    }

    fn parse_line(line: &str) -> Option<HintLine> {
        let clean_line = line.replace(['\n', '\r'], " ");
        let trimmed = clean_line.trim();
        if trimmed.is_empty() {
            return None;
        }

        if let Some(split_at) = clean_line.find("  ") {
            let key = clean_line[..split_at].trim();
            let description = clean_line[split_at..].trim();
            if !key.is_empty() && !description.is_empty() {
                return Some(HintLine {
                    key: Some(key.to_string()),
                    description: description.to_string(),
                });
            }
        }

        Some(HintLine {
            key: None,
            description: trimmed.to_string(),
        })
    }

    fn parse_keystroke(key: &str) -> Option<Keystroke> {
        let normalized = normalize_hint_key(key);
        Keystroke::parse(&normalized).ok()
    }

    fn key_badge_labels(key: &str) -> Vec<String> {
        let Some(stroke) = Self::parse_keystroke(key) else {
            return vec![key.trim().to_string()];
        };

        let mut labels = Vec::new();

        if stroke.modifiers.control {
            labels.push("ctrl".to_string());
        }
        if stroke.modifiers.alt {
            labels.push("alt".to_string());
        }
        if stroke.modifiers.shift {
            labels.push("shift".to_string());
        }
        if stroke.modifiers.platform {
            labels.push("cmd".to_string());
        }
        labels.push(key_label(stroke.key.as_str()));

        labels
    }

    fn render_keycap(
        theme: &Theme,
        key_font: &gpui::Font,
        text_size: f32,
        label: String,
    ) -> AnyElement {
        let tokens = &theme.tokens;
        let tooltip = tokens.tooltip_tokens();
        let key_bg = ColorTheory::with_alpha(tokens.chrome.surface_hover, 0.65);
        let is_modifier = matches!(label.as_str(), "ctrl" | "alt" | "shift" | "cmd");
        let min_width = if is_modifier { 34.0 } else { 18.0 };

        div()
            .min_w(px(min_width))
            .h(px(20.0))
            .px(px(if is_modifier { 5.0 } else { 4.0 }))
            .border_1()
            .border_color(tooltip.border)
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .text_align(gpui::TextAlign::Center)
            .font(key_font.clone())
            .text_size(px((text_size - 2.0).max(9.0)))
            .line_height(relative(1.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(tooltip.text)
            .bg(key_bg)
            .child(label)
            .into_any_element()
    }

    fn render_key_badges(
        theme: &Theme,
        key_font: &gpui::Font,
        text_size: f32,
        key: &str,
    ) -> AnyElement {
        let labels = Self::key_badge_labels(key);

        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(3.0))
            .children(
                labels
                    .into_iter()
                    .map(|label| Self::render_keycap(theme, key_font, text_size, label)),
            )
            .into_any_element()
    }

    fn pair_lines(lines: Vec<HintLine>) -> Vec<(HintLine, Option<HintLine>)> {
        let mut pairs = Vec::with_capacity(lines.len().div_ceil(2));
        let mut lines = lines.into_iter();

        while let Some(first) = lines.next() {
            pairs.push((first, lines.next()));
        }

        pairs
    }

    fn render_line(
        theme: &Theme,
        key_font: &gpui::Font,
        key_text_size: f32,
        line: HintLine,
    ) -> AnyElement {
        let tooltip = theme.tokens.tooltip_tokens();

        div()
            .w(px(HINT_COLUMN_WIDTH))
            .flex_none()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(4.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .child(div().w(px(HINT_KEY_SLOT_WIDTH)).flex_none().when_some(
                line.key.as_deref(),
                |slot, key| {
                    slot.flex()
                        .items_center()
                        .justify_end()
                        .child(Self::render_key_badges(theme, key_font, key_text_size, key))
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(tooltip.text)
                    .child(line.description),
            )
            .into_any_element()
    }

    fn render_row(
        theme: &Theme,
        key_font: &gpui::Font,
        key_text_size: f32,
        first: HintLine,
        second: Option<HintLine>,
    ) -> AnyElement {
        let second = second.map_or_else(
            || {
                div()
                    .w(px(HINT_COLUMN_WIDTH))
                    .flex_none()
                    .min_w(px(0.0))
                    .into_any_element()
            },
            |line| Self::render_line(theme, key_font, key_text_size, line),
        );

        div()
            .w_full()
            .flex()
            .items_start()
            .justify_start()
            .gap(px(HINT_COLUMN_GAP))
            .child(Self::render_line(theme, key_font, key_text_size, first))
            .child(second)
            .into_any_element()
    }
}

impl Render for KeyHintView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(info) = &self.info {
            let theme = cx.global::<Theme>().clone();
            let tooltip = theme.tokens.tooltip_tokens();
            let font_settings = cx.global::<nucleotide_types::FontSettings>();
            let ui_font: gpui::Font = font_settings.var_font.clone().into();
            let key_font: gpui::Font = font_settings.fixed_font.clone().into();
            let ui_text_size = cx.global::<nucleotide_types::UiFontConfig>().size - 1.0;
            let clean_title = info.title.replace(['\n', '\r'], " ").trim().to_string();
            let lines = info
                .text
                .lines()
                .filter_map(Self::parse_line)
                .collect::<Vec<_>>();
            let rows = Self::pair_lines(lines);

            div()
                .absolute()
                .bottom_4()
                .right_4()
                .w(px((HINT_COLUMN_WIDTH * 2.0) + HINT_COLUMN_GAP))
                .bg(tooltip.background)
                .border_1()
                .border_color(tooltip.border)
                .rounded(px(6.0))
                .shadow(vec![
                    theme.tokens.chrome.shadow_md.to_box_shadow(false),
                    theme.tokens.chrome.inset_highlight.to_box_shadow(true),
                ])
                .p(px(8.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .font(ui_font)
                .text_size(px(ui_text_size))
                .when(!clean_title.is_empty(), |this| {
                    this.child(
                        div()
                            .w_full()
                            .text_color(tooltip.text)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .border_b_1()
                            .border_color(tooltip.border)
                            .pb(px(6.0))
                            .mb(px(2.0))
                            .child(clean_title),
                    )
                })
                .children(rows.into_iter().map(|(first, second)| {
                    Self::render_row(&theme, &key_font, ui_text_size, first, second)
                }))
        } else {
            div().w_0().h_0()
        }
    }
}

impl EventEmitter<()> for KeyHintView {}

fn normalize_hint_key(key: &str) -> String {
    let key = key.trim();
    if key.len() == 1
        && let Some(ch) = key.chars().next()
        && ch.is_ascii_uppercase()
    {
        return format!("shift-{}", ch.to_ascii_lowercase());
    }

    key.replace("C-", "ctrl-")
        .replace("A-", "alt-")
        .replace("S-", "shift-")
        .replace("<space>", "space")
        .replace("<ret>", "enter")
        .replace("<enter>", "enter")
}

fn key_label(key: &str) -> String {
    match key {
        "space" => "space".to_string(),
        "enter" => "enter".to_string(),
        "backspace" => "backspace".to_string(),
        "delete" => "delete".to_string(),
        "escape" => "esc".to_string(),
        key if key.len() == 1 => key.to_lowercase(),
        key => key.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_padded_hint_lines() {
        assert_eq!(
            KeyHintView::parse_line("d          goto definition"),
            Some(HintLine {
                key: Some("d".to_string()),
                description: "goto definition".to_string(),
            })
        );
    }

    #[test]
    fn parses_plain_hint_lines() {
        assert_eq!(
            KeyHintView::parse_line("goto"),
            Some(HintLine {
                key: None,
                description: "goto".to_string(),
            })
        );
    }

    #[test]
    fn normalizes_helix_hint_keys_for_gpui() {
        assert_eq!(normalize_hint_key("C-o"), "ctrl-o");
        assert_eq!(normalize_hint_key("D"), "shift-d");
        assert_eq!(normalize_hint_key("<ret>"), "enter");
    }

    #[test]
    fn renders_modifiers_before_keycaps() {
        assert_eq!(KeyHintView::key_badge_labels("G"), vec!["shift", "g"]);
        assert_eq!(KeyHintView::key_badge_labels("C-o"), vec!["ctrl", "o"]);
    }

    #[test]
    fn pairs_hint_lines_for_two_columns() {
        let pairs = KeyHintView::pair_lines(vec![
            HintLine {
                key: Some("d".to_string()),
                description: "definition".to_string(),
            },
            HintLine {
                key: Some("r".to_string()),
                description: "references".to_string(),
            },
            HintLine {
                key: Some("i".to_string()),
                description: "implementation".to_string(),
            },
        ]);

        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0.key.as_deref(), Some("d"));
        assert_eq!(
            pairs[0].1.as_ref().and_then(|line| line.key.as_deref()),
            Some("r")
        );
        assert_eq!(pairs[1].0.key.as_deref(), Some("i"));
        assert!(pairs[1].1.is_none());
    }
}
