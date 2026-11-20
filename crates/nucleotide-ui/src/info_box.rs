use crate::Theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, DismissEvent, EventEmitter, FontWeight, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div,
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
        let tokens = &theme.tokens;
        let tooltip_tokens = tokens.tooltip_tokens();
        let bottom_offset = tokens.sizes.statusbar_height + tokens.sizes.space_2;

        div()
            .absolute()
            .bottom(bottom_offset)
            .right(tokens.sizes.space_2)
            .flex()
            .flex_row()
            .child(
                div()
                    .rounded(tokens.sizes.radius_sm)
                    .shadow(vec![tokens.chrome.shadow_sm.to_box_shadow(false)])
                    .text_size(tokens.sizes.text_sm)
                    .bg(tooltip_tokens.background)
                    .text_color(tooltip_tokens.text)
                    .border_1()
                    .border_color(tooltip_tokens.border)
                    .p(tokens.sizes.space_3)
                    .flex()
                    .flex_row()
                    .content_end()
                    .when_some(self.title.as_ref(), |this, title| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(tokens.sizes.space_1)
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
