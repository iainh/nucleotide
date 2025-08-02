use gpui::prelude::FluentBuilder;
use gpui::*;
use helix_view::info::Info;

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

    fn handle_event(&mut self, ev: &crate::Update, cx: &mut Context<Self>) {
        if let crate::Update::Info(info) = ev {
            self.set_info(info);
            cx.notify();
        }
    }

    pub fn subscribe(&self, editor: &Entity<crate::Core>, cx: &mut Context<Self>) {
        cx.subscribe(editor, |this, _, ev, cx| {
            this.handle_event(ev, cx);
        })
        .detach()
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font = cx.global::<crate::FontSettings>().fixed_font.clone();

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
                    .font(font)
                    .text_size(px(12.))
                    .text_color(self.style.text.color.unwrap())
                    .bg(self.style.background.as_ref().cloned().unwrap())
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
