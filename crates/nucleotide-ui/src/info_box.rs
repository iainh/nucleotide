use crate::Theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, DismissEvent, EventEmitter, FontWeight, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, px,
};
use helix_view::info::Info;
use nucleotide_core::{AppEvent, UiEvent};

#[derive(Debug)]
pub struct InfoBoxView {
    title: Option<SharedString>,
    text: Option<SharedString>,
}

impl InfoBoxView {
    pub fn new() -> Self {
        InfoBoxView {
            title: None,
            text: None,
        }
    }

    #[allow(dead_code)]
    fn handle_event(&mut self, ev: &AppEvent, cx: &mut Context<Self>) {
        if let AppEvent::Ui(UiEvent::OverlayShown { overlay_type, .. }) = ev
            && matches!(
                *overlay_type,
                nucleotide_events::v2::ui::OverlayType::Tooltip
            )
        {
            // Handle info overlay shown
            cx.notify();
        }
    }

    // TODO: Replace with event bus subscription
    // pub fn subscribe(&self, event_bus: &EventBus, cx: &mut Context<Self>) {
    //     event_bus.subscribe_ui(|this, event| {
    //         this.handle_event(event, cx);
    //     })
    // }

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
                    .shadow_sm()
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
