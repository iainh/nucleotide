// ABOUTME: This file implements the KeyHintView component that displays keybinding hints
// ABOUTME: when a pending keymap state is active (e.g., after pressing space leader)

use crate::key_format::{KeyPlatform, key_label, modifier_labels};
use crate::{Theme, styling::ColorTheory};
use gpui::{
    AnyElement, Context, EventEmitter, InteractiveElement, IntoElement, Keystroke, ParentElement,
    Pixels, Render, StatefulInteractiveElement, Styled, Window, canvas, div,
    prelude::FluentBuilder, px, relative,
};
use helix_view::info::Info;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq)]
struct HintLine {
    key: Option<String>,
    description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeycapLabel {
    text: String,
    is_modifier: bool,
}

impl KeycapLabel {
    fn key(text: String) -> Self {
        Self {
            text,
            is_modifier: false,
        }
    }
}

const HINT_COLUMN_WIDTH: f32 = 292.0;
const HINT_COLUMN_GAP: f32 = 14.0;
const HINT_KEY_SLOT_WIDTH: f32 = 84.0;

#[derive(Debug)]
pub struct KeyHintView {
    info: Option<Info>,
    /// Width of the editor area the popup may use, measured each frame.
    frame_width: Rc<Cell<Option<Pixels>>>,
}

impl Default for KeyHintView {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyHintView {
    pub fn new() -> Self {
        Self {
            info: None,
            frame_width: Rc::default(),
        }
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

    /// One keycap per modifier, then the key, in the platform's native style.
    /// Helix lists alternative bindings as `s, C-s`; native hints show the first.
    fn key_badge_labels(key: &str, platform: KeyPlatform) -> Vec<KeycapLabel> {
        let key = key.split(", ").next().unwrap_or(key);
        let Some(stroke) = Self::parse_keystroke(key) else {
            return vec![KeycapLabel::key(key.trim().to_string())];
        };

        let mut labels = modifier_labels(&stroke.modifiers, platform)
            .into_iter()
            .map(|label| KeycapLabel {
                text: label.to_string(),
                is_modifier: true,
            })
            .collect::<Vec<_>>();
        labels.push(KeycapLabel::key(hint_key_label(&stroke, platform)));

        labels
    }

    fn render_keycap(theme: &Theme, key_font: &gpui::Font, label: KeycapLabel) -> AnyElement {
        let tokens = &theme.tokens;
        let tooltip = tokens.tooltip_tokens();
        let key_bg = Self::keycap_background(theme);
        // Word modifiers (Ctrl, Shift) need a wider cap than single macOS symbols.
        let is_modifier = label.is_modifier && label.text.chars().count() > 1;
        let min_width = if is_modifier { 34.0 } else { 18.0 };
        let label = label.text;

        div()
            .min_w(px(min_width))
            .h(tokens.sizes.space_6)
            .px(if is_modifier {
                tokens.sizes.space_2 + tokens.sizes.space_1 / 2.0
            } else {
                tokens.sizes.space_2
            })
            .border_1()
            .border_color(tooltip.border)
            .rounded(tokens.sizes.radius_sm)
            .flex()
            .items_center()
            .justify_center()
            .text_align(gpui::TextAlign::Center)
            .font(key_font.clone())
            .text_size(tokens.sizes.text_xs)
            .line_height(relative(1.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(tooltip.text)
            .bg(key_bg)
            .child(label)
            .into_any_element()
    }

    fn keycap_background(theme: &Theme) -> gpui::Hsla {
        let tooltip = theme.tokens.tooltip_tokens();
        let background = ColorTheory::surface_variant(tooltip.background, 0.06);
        let minimum_alpha = if tooltip.background.l > 0.5 {
            0.92
        } else {
            0.88
        };

        ColorTheory::with_alpha(background, tooltip.background.a.max(minimum_alpha))
    }

    fn render_key_badges(theme: &Theme, key_font: &gpui::Font, key: &str) -> AnyElement {
        let labels = Self::key_badge_labels(key, KeyPlatform::current());

        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(theme.tokens.sizes.space_1)
            .children(
                labels
                    .into_iter()
                    .map(|label| Self::render_keycap(theme, key_font, label)),
            )
            .into_any_element()
    }

    fn render_row(
        theme: &Theme,
        key_font: &gpui::Font,
        first: HintLine,
        second: Option<HintLine>,
    ) -> AnyElement {
        div()
            .flex()
            .items_start()
            .gap(px(HINT_COLUMN_GAP))
            .child(Self::render_line(theme, key_font, first, false))
            .when_some(second, |row, line| {
                row.child(Self::render_line(theme, key_font, line, false))
            })
            .into_any_element()
    }

    /// `full_width` fills a single column; a definite width lets wrapped
    /// descriptions be measured at the width they are painted at.
    fn render_line(
        theme: &Theme,
        key_font: &gpui::Font,
        line: HintLine,
        full_width: bool,
    ) -> AnyElement {
        let tokens = &theme.tokens;
        let tooltip = tokens.tooltip_tokens();

        div()
            .when(full_width, |row| row.w_full())
            .when(!full_width, |row| row.w(px(HINT_COLUMN_WIDTH)))
            .flex_none()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(tokens.sizes.space_3)
            .px(tokens.sizes.space_2)
            .py(tokens.sizes.space_1)
            .rounded(tokens.sizes.radius_sm)
            .child(div().min_w(px(HINT_KEY_SLOT_WIDTH)).flex_none().when_some(
                line.key.as_deref(),
                |slot, key| {
                    slot.flex()
                        .items_center()
                        .justify_end()
                        .child(Self::render_key_badges(theme, key_font, key))
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
}

impl Render for KeyHintView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(info) = &self.info {
            let theme = cx.global::<Theme>().clone();
            let tokens = &theme.tokens;
            let tooltip = tokens.tooltip_tokens();
            let font_settings = cx.global::<nucleotide_types::FontSettings>();
            let ui_font: gpui::Font = font_settings.var_font.clone().into();
            let key_font: gpui::Font = font_settings.fixed_font.clone().into();
            let clean_title = info.title.replace(['\n', '\r'], " ").trim().to_string();
            let lines = info
                .text
                .lines()
                .filter_map(Self::parse_line)
                .collect::<Vec<_>>();

            // The inset frame spans the editor so the popup can shrink to fit it:
            // two columns when there is room, one when the editor is narrow,
            // instead of spilling under the sidebar.
            let two_column_width = px((HINT_COLUMN_WIDTH * 2.0) + HINT_COLUMN_GAP)
                + tokens.sizes.space_3 * 2.0
                + px(2.0);
            // Until the editor area has been measured, assume it is wide.
            let two_columns = self
                .frame_width
                .get()
                .is_none_or(|width| width >= two_column_width);
            let rows: Vec<AnyElement> = if two_columns {
                let mut lines = lines.into_iter();
                std::iter::from_fn(|| {
                    let first = lines.next()?;
                    Some(Self::render_row(&theme, &key_font, first, lines.next()))
                })
                .collect()
            } else {
                lines
                    .into_iter()
                    .map(|line| Self::render_line(&theme, &key_font, line, true))
                    .collect()
            };

            let popup = div()
                .when(two_columns, |popup| popup.w(two_column_width))
                .when(!two_columns, |popup| popup.w_full())
                .max_h(relative(1.0))
                .min_h(px(0.0))
                .bg(tooltip.background)
                .border_1()
                .border_color(tooltip.border)
                .rounded(tokens.sizes.radius_md)
                .shadow(vec![tokens.chrome.shadow_md.to_box_shadow(false)])
                .p(tokens.sizes.space_3)
                .flex()
                .flex_col()
                .gap(tokens.sizes.space_2)
                .font(ui_font)
                .text_size(tokens.sizes.text_sm)
                .when(!clean_title.is_empty(), |this| {
                    this.child(
                        div()
                            .w_full()
                            .text_color(tooltip.text)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .border_b_1()
                            .border_color(tooltip.border)
                            .pb(tokens.sizes.space_2)
                            .mb(tokens.sizes.space_1)
                            .child(clean_title),
                    )
                })
                .child(
                    div()
                        .id("key-hint-lines")
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap(tokens.sizes.space_2)
                        .children(rows),
                );

            // Inset on every side so a short editor caps the popup's height and
            // the hints scroll instead of running under the titlebar.
            div()
                .absolute()
                .top(tokens.sizes.space_4)
                .left(tokens.sizes.space_4)
                .right(tokens.sizes.space_4)
                .bottom(tokens.sizes.space_4)
                .flex()
                .flex_col()
                .justify_end()
                .items_end()
                .child({
                    // Records the editor area's width; a change of column count
                    // re-renders on the next frame.
                    let frame_width = self.frame_width.clone();
                    canvas(
                        move |bounds, window, _| {
                            let width = bounds.size.width;
                            let was_two =
                                frame_width.get().is_none_or(|old| old >= two_column_width);
                            frame_width.set(Some(width));
                            if was_two != (width >= two_column_width) {
                                window.refresh();
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full()
                })
                .child(popup)
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

/// Helix keys are case-sensitive, so an unmodified `g` stays lower-case while
/// shifted and other modified letters use the upper-case keycap (`⇧ G`, `Ctrl O`).
fn hint_key_label(stroke: &Keystroke, platform: KeyPlatform) -> String {
    let key = stroke.key.as_str();
    if key.chars().count() == 1 && !stroke.modifiers.modified() {
        key.to_string()
    } else {
        key_label(key, platform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::{ChromeTokens, DesignTokens, EditorTokens, SizeTokens};
    use gpui::hsla;
    use nucleotide_appearance::{NativeChromePalette, SystemAppearance};

    fn system_theme(is_dark: bool) -> Theme {
        Theme::from_tokens(DesignTokens {
            editor: EditorTokens::fallback(is_dark),
            chrome: ChromeTokens::from_native_chrome_palette(NativeChromePalette::with_accent(
                if is_dark {
                    SystemAppearance::Dark
                } else {
                    SystemAppearance::Light
                },
                hsla(0.0, 0.70, 0.45, 1.0),
            )),
            sizes: SizeTokens::default(),
        })
    }

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

    fn badge_texts(key: &str, platform: KeyPlatform) -> Vec<String> {
        KeyHintView::key_badge_labels(key, platform)
            .into_iter()
            .map(|label| label.text)
            .collect()
    }

    #[test]
    fn renders_platform_native_modifier_keycaps() {
        assert_eq!(badge_texts("G", KeyPlatform::MacOS), vec!["⇧", "G"]);
        assert_eq!(badge_texts("G", KeyPlatform::Linux), vec!["Shift", "G"]);
        // Written with Shift first; native order puts Control first.
        assert_eq!(
            badge_texts("S-C-o", KeyPlatform::MacOS),
            vec!["⌃", "⇧", "O"]
        );
        assert_eq!(
            badge_texts("C-A-o", KeyPlatform::Windows),
            vec!["Ctrl", "Alt", "O"]
        );
        assert_eq!(badge_texts("<ret>", KeyPlatform::Windows), vec!["Enter"]);
        assert_eq!(badge_texts("<ret>", KeyPlatform::MacOS), vec!["↩"]);
        // Plain keys keep Helix's case so `g` is not mistaken for `G`.
        assert_eq!(badge_texts("g", KeyPlatform::Windows), vec!["g"]);
        // Helix alternatives show only the first binding.
        assert_eq!(
            badge_texts("C-s, s", KeyPlatform::Windows),
            vec!["Ctrl", "S"]
        );
        assert_eq!(badge_texts(",", KeyPlatform::Windows), vec![","]);
    }

    #[test]
    fn keycap_background_tracks_system_light_and_dark_surfaces() {
        let light = KeyHintView::keycap_background(&system_theme(false));
        let dark = KeyHintView::keycap_background(&system_theme(true));

        assert!(
            light.l > 0.85,
            "light keycap should remain light: {light:?}"
        );
        assert!(dark.l < 0.35, "dark keycap should remain dark: {dark:?}");
        assert!(light.a >= 0.92);
        assert!(dark.a >= 0.88);
    }
}
