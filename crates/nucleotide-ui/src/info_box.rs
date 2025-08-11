use gpui::prelude::FluentBuilder;
use gpui::*;
use helix_view::info::Info;
use nucleotide_core::events::{AppEvent, UiEvent};

#[derive(Debug)]
pub struct InfoBoxView {
    title: Option<SharedString>,
    text: Option<SharedString>,
    style: Style,
}

impl InfoBoxView {
    pub fn new(style: Style) -> Self {
        InfoBoxView {
            title: None,
            text: None,
            style,
        }
    }

    fn handle_event(&mut self, ev: &AppEvent, cx: &mut Context<Self>) {
        if let AppEvent::Ui(UiEvent::ShowInfo { title, body }) = ev {
            self.title = Some(title.clone().into());
            self.text = Some(body.join("\n").into());
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
                    .text_size(px(cx
                        .global::<nucleotide_core::shared_types::UiFontConfig>()
                        .size
                        - 1.0))
                    .when_some(self.style.text.color, |this, color| this.text_color(color))
                    .bg(gpui::rgb(0x2a2a3e))
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
                                    this.child(text.clone())
                                }),
                        )
                    }),
            )
    }
}
