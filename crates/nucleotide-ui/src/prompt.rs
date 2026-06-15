use gpui::{
    App, FocusHandle, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    Styled, TextStyle, Window, div, px,
};
use std::sync::Arc;

use crate::text_utils::TextWithStyle;

#[derive(Clone)]
pub enum Prompt {
    Legacy(TextWithStyle),
    Native {
        prompt: SharedString,
        initial_input: SharedString,
        on_submit: Arc<dyn Fn(&str) + Send + Sync>,
        on_cancel: Option<Arc<dyn Fn() + Send + Sync>>,
    },
}

impl Prompt {
    pub fn legacy(text: TextWithStyle) -> Self {
        Self::Legacy(text)
    }

    pub fn native(
        prompt: impl Into<SharedString>,
        initial_input: impl Into<SharedString>,
        on_submit: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        Self::Native {
            prompt: prompt.into(),
            initial_input: initial_input.into(),
            on_submit: Arc::new(on_submit),
            on_cancel: None,
        }
    }

    pub fn with_cancel(mut self, on_cancel: impl Fn() + Send + Sync + 'static) -> Self {
        if let Self::Native {
            on_cancel: ref mut cancel,
            ..
        } = self
        {
            *cancel = Some(Arc::new(on_cancel));
        }
        self
    }

    pub fn as_legacy(&self) -> Option<&TextWithStyle> {
        match self {
            Self::Legacy(text) => Some(text),
            _ => None,
        }
    }
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Legacy(text) => f.debug_tuple("Legacy").field(text).finish(),
            Self::Native {
                prompt,
                initial_input,
                ..
            } => f
                .debug_struct("Native")
                .field("prompt", prompt)
                .field("initial_input", initial_input)
                .field("on_submit", &"<callback>")
                .field("on_cancel", &"<callback>")
                .finish(),
        }
    }
}

#[derive(IntoElement)]
pub struct PromptElement {
    pub prompt: Prompt,
    pub focus: FocusHandle,
    pub theme: Option<helix_view::Theme>,
}

impl RenderOnce for PromptElement {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        match &self.prompt {
            Prompt::Legacy(text_with_style) => {
                let ui_theme = cx.global::<crate::Theme>().clone();
                let bg_color = ui_theme.tokens.chrome.popup_background;
                let text_color = crate::styling::ColorTheory::ensure_contrast(
                    bg_color,
                    ui_theme.tokens.chrome.text_on_chrome,
                    crate::styling::color_theory::ContrastRatios::AA_NORMAL,
                );
                let border_color = ui_theme.tokens.chrome.popup_border;

                let default_style = TextStyle {
                    font_family: "JetBrains Mono".into(),
                    font_size: px(14.).into(),
                    background_color: Some(bg_color),
                    ..Default::default()
                };

                let text = text_with_style.clone().into_styled_text(&default_style);
                self.focus.focus(window, cx);

                div()
                    .track_focus(&self.focus)
                    .flex()
                    .flex_col()
                    .p_2()
                    .bg(bg_color)
                    .border_1()
                    .border_color(border_color)
                    .rounded_md()
                    .shadow(vec![
                        ui_theme.tokens.chrome.shadow_lg.to_box_shadow(false),
                        ui_theme.tokens.chrome.inset_highlight.to_box_shadow(true),
                    ])
                    .text_color(text_color)
                    .font(
                        cx.global::<nucleotide_types::FontSettings>()
                            .var_font
                            .clone()
                            .into(),
                    )
                    .text_size(px(cx.global::<nucleotide_types::UiFontConfig>().size))
                    .line_height(px(1.3 * cx.global::<nucleotide_types::UiFontConfig>().size))
                    .child(text)
            }
            Prompt::Native { .. } => {
                // For native prompts, we shouldn't render them here
                // They should be handled by the native PromptView component
                div()
                    .track_focus(&self.focus)
                    .child("Native prompt should use PromptView")
            }
        }
    }
}
