// ABOUTME: Shared native confirmation dialog
// ABOUTME: Provides reusable modal chrome and action layout for destructive confirmations

use gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString, Styled,
    Window, div, px,
};

use crate::{Button, ButtonSize, ButtonVariant, ThemedContext};

#[derive(Clone, Debug)]
pub struct ConfirmDialog {
    pub title: SharedString,
    pub message: SharedString,
    pub cancel_label: SharedString,
    pub confirm_label: SharedString,
    pub confirm_variant: ButtonVariant,
    pub width: gpui::Pixels,
    pub top: gpui::Pixels,
}

impl ConfirmDialog {
    pub fn new(
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        confirm_label: impl Into<SharedString>,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            cancel_label: "Cancel".into(),
            confirm_label: confirm_label.into(),
            confirm_variant: ButtonVariant::Primary,
            width: px(380.0),
            top: px(120.0),
        }
    }

    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    pub fn confirm_variant(mut self, variant: ButtonVariant) -> Self {
        self.confirm_variant = variant;
        self
    }

    pub fn width(mut self, width: gpui::Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn top(mut self, top: gpui::Pixels) -> Self {
        self.top = top;
        self
    }
}

pub struct ConfirmDialogCallbacks<Cancel, Confirm> {
    pub on_cancel: Cancel,
    pub on_confirm: Confirm,
}

pub fn render_confirm_dialog<T, Cancel, Confirm>(
    dialog: ConfirmDialog,
    cx: &mut Context<T>,
    callbacks: ConfirmDialogCallbacks<Cancel, Confirm>,
) -> gpui::AnyElement
where
    T: 'static,
    Cancel: Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
    Confirm: Fn(&mut T, &mut Window, &mut Context<T>) + Copy + 'static,
{
    let ConfirmDialogCallbacks {
        on_cancel,
        on_confirm,
    } = callbacks;
    let tokens = &cx.theme().tokens;
    let picker_tokens = tokens.picker_tokens();

    let backdrop = div()
        .absolute()
        .size_full()
        .top_0()
        .left_0()
        .occlude()
        .bg(tokens.chrome.surface_overlay)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |state, _event, window, cx| {
                on_cancel(state, window, cx);
                cx.stop_propagation();
            }),
        );

    let dialog_panel = div()
        .absolute()
        .top(dialog.top)
        .w_full()
        .flex()
        .justify_center()
        .child(
            div()
                .bg(picker_tokens.container_background)
                .border_1()
                .border_color(picker_tokens.border)
                .rounded(tokens.sizes.radius_lg)
                .shadow(vec![
                    tokens.chrome.shadow_lg.to_box_shadow(false),
                    tokens.chrome.inset_highlight.to_box_shadow(true),
                ])
                .w(dialog.width)
                .p(tokens.sizes.space_4)
                .flex()
                .flex_col()
                .gap(tokens.sizes.space_3)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(tokens.sizes.text_md)
                        .text_color(tokens.chrome.text_on_chrome)
                        .child(dialog.title),
                )
                .child(
                    div()
                        .text_size(tokens.sizes.text_sm)
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .child(dialog.message),
                )
                .child(
                    div()
                        .flex()
                        .gap(tokens.sizes.space_2)
                        .justify_end()
                        .child(
                            Button::new("confirm-dialog-cancel", dialog.cancel_label)
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(move |state, _event, window, cx| {
                                    on_cancel(state, window, cx);
                                })),
                        )
                        .child(
                            Button::new("confirm-dialog-confirm", dialog.confirm_label)
                                .variant(dialog.confirm_variant)
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(move |state, _event, window, cx| {
                                    on_confirm(state, window, cx);
                                })),
                        ),
                ),
        );

    div().child(backdrop).child(dialog_panel).into_any_element()
}
