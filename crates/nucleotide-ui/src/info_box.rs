use crate::Theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, DismissEvent, EventEmitter, FontWeight, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, px,
};
use helix_view::info::Info;

#[derive(Debug)]
pub struct InfoBoxView {
    title: Option<SharedString>,
    text: Option<SharedString>,
}

impl Default for InfoBoxView {
    fn default() -> Self {
        Self::new()
    }
}

impl InfoBoxView {
    pub fn new() -> Self {
        InfoBoxView {
            title: None,
            text: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.title.is_none()
    }

    pub fn set_info(&mut self, info: &Info) {
        self.title = Some(info.title.clone().into());
        self.text = Some(info.text.clone().into());
    }
}

impl EventEmitter<DismissEvent> for InfoBoxView {}

impl Render for InfoBoxView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let tooltip_tokens = theme.tokens.tooltip_tokens();

        div()
            .absolute()
            .bottom_7()
            .right_1()
            .flex()
            .flex_row()
            .child(
                div()
                    .rounded_sm()
                    .shadow(vec![
                        theme.tokens.chrome.shadow_sm.to_box_shadow(false),
                        theme.tokens.chrome.inset_highlight.to_box_shadow(true),
                    ])
                    .text_size(px(cx.global::<nucleotide_types::UiFontConfig>().size - 1.0))
                    .bg(tooltip_tokens.background)
                    .text_color(tooltip_tokens.text)
                    .border_1()
                    .border_color(tooltip_tokens.border)
                    .p_2()
                    .flex()
                    .flex_row()
                    .content_end()
                    .when_some(self.title.as_ref(), |this, title| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex()
                                        .font_weight(FontWeight::BOLD)
                                        .flex_none()
                                        .justify_center()
                                        .items_center()
                                        .child(title.clone()),
                                )
                                .when_some(self.text.as_ref(), |this, text| {
                                    this.child(
                                        div()
                                            .text_color(tooltip_tokens.text_secondary)
                                            .child(text.clone()),
                                    )
                                }),
                        )
                    }),
            )
    }
}
