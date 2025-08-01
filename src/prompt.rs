use gpui::*;
use std::sync::Arc;

use crate::utils::TextWithStyle;

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
        if let Self::Native { on_cancel: ref mut cancel, .. } = self {
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
            Self::Native { prompt, initial_input, .. } => f
                .debug_struct("Native")
                .field("prompt", prompt)
                .field("initial_input", initial_input)
                .field("on_submit", &"<callback>")
                .field("on_cancel", &"<callback>")
                .finish(),
        }
    }
}

impl Prompt {
    pub fn make(editor: &mut helix_view::Editor, prompt: &mut helix_term::ui::Prompt) -> Prompt {
        let area = editor.tree.area();
        let compositor_rect = helix_view::graphics::Rect {
            x: 0,
            y: 0,
            width: area.width * 2 / 3,
            height: area.height,
        };

        let mut comp_ctx = helix_term::compositor::Context {
            editor,
            scroll: None,
            jobs: &mut helix_term::job::Jobs::new(),
        };
        let mut buf = tui::buffer::Buffer::empty(compositor_rect);
        prompt.render_prompt(compositor_rect, &mut buf, &mut comp_ctx);
        Prompt::legacy(TextWithStyle::from_buffer(buf))
    }
}

#[derive(IntoElement)]
pub struct PromptElement {
    pub prompt: Prompt,
    pub focus: FocusHandle,
}

impl RenderOnce for PromptElement {
    fn render(self, cx: &mut WindowContext) -> impl IntoElement {
        match &self.prompt {
            Prompt::Legacy(text_with_style) => {
                let bg_color = text_with_style
                    .style(0)
                    .and_then(|style| style.background_color);
                let mut default_style = TextStyle::default();
                default_style.font_family = "JetBrains Mono".into();
                default_style.font_size = px(12.).into();
                default_style.background_color = bg_color;

                let text = text_with_style.clone().into_styled_text(&default_style);
                cx.focus(&self.focus);
                div()
                    .track_focus(&self.focus)
                    .flex()
                    .flex_col()
                    .p_5()
                    .bg(bg_color.unwrap_or(black()))
                    .shadow_sm()
                    .rounded_sm()
                    .text_color(hsla(1., 1., 1., 1.))
                    .font(cx.global::<crate::FontSettings>().fixed_font.clone())
                    .text_size(px(12.))
                    .line_height(px(1.3) * px(12.))
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
