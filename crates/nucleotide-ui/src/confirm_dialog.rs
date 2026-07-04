// ABOUTME: Shared native dialog primitives and confirmation dialog wrapper
// ABOUTME: Adapted from longbridge/gpui-component dialog components (Apache-2.0)
// Copyright 2024-2025 Longbridge. Locally adapted for Nucleotide.

use gpui::{
    AnyElement, App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, ParentElement,
    Pixels, Render, RenderOnce, SharedString, Styled, Window, div, px,
};

use crate::actions::dialog::{Cancel as CancelDialogAction, Confirm as ConfirmDialogAction};
use crate::actions::focus::{FocusNext, FocusPrevious};
use crate::modal_layer::{DismissDecision, ModalView};
use crate::{Button, ButtonSize, ButtonVariant, FocusTraversal, ThemedContext};

type OverlayHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>;

const DIALOG_OVERLAY_ALPHA_LIGHT: f32 = 0.70;
const DIALOG_OVERLAY_ALPHA_DARK: f32 = 0.45;
pub(crate) const CONFIRM_DIALOG_CONTEXT: &str = "ConfirmDialog";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", ConfirmDialogAction, Some(CONFIRM_DIALOG_CONTEXT)),
        KeyBinding::new("escape", CancelDialogAction, Some(CONFIRM_DIALOG_CONTEXT)),
        KeyBinding::new("tab", FocusNext, Some(CONFIRM_DIALOG_CONTEXT)),
        KeyBinding::new("shift-tab", FocusPrevious, Some(CONFIRM_DIALOG_CONTEXT)),
    ]);
}

/// A modal dialog container with a backdrop and centred panel.
#[derive(IntoElement)]
pub struct Dialog {
    children: Vec<AnyElement>,
    footer: Option<AnyElement>,
    width: Pixels,
    top: Pixels,
    overlay_closable: bool,
    on_overlay_mouse_down: Option<OverlayHandler>,
}

impl Dialog {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            footer: None,
            width: px(448.0),
            top: px(120.0),
            overlay_closable: true,
            on_overlay_mouse_down: None,
        }
    }

    pub fn width(mut self, width: impl Into<Pixels>) -> Self {
        self.width = width.into();
        self
    }

    pub fn top(mut self, top: impl Into<Pixels>) -> Self {
        self.top = top.into();
        self
    }

    pub fn overlay_closable(mut self, overlay_closable: bool) -> Self {
        self.overlay_closable = overlay_closable;
        self
    }

    pub fn on_overlay_mouse_down(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_overlay_mouse_down = Some(Box::new(handler));
        self
    }

    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }
}

impl Default for Dialog {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Dialog {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = &cx.theme().tokens;
        let overlay_alpha = if cx.is_dark_theme() {
            DIALOG_OVERLAY_ALPHA_DARK
        } else {
            DIALOG_OVERLAY_ALPHA_LIGHT
        };

        let mut backdrop = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(tokens.chrome.surface_overlay.alpha(overlay_alpha));

        if self.overlay_closable
            && let Some(on_overlay_mouse_down) = self.on_overlay_mouse_down
        {
            backdrop = backdrop.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                on_overlay_mouse_down(event, window, cx);
                cx.stop_propagation();
            });
        }

        let mut panel = div()
            .occlude()
            .bg(tokens.chrome.surface)
            .border_1()
            .border_color(tokens.chrome.border_default)
            .rounded(tokens.sizes.radius_lg)
            .shadow(vec![
                tokens.chrome.shadow_lg.to_box_shadow(false),
                tokens.chrome.inset_highlight.to_box_shadow(true),
            ])
            .w(self.width)
            .p(tokens.sizes.space_4)
            .flex()
            .flex_col()
            .gap(tokens.sizes.space_3)
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .children(self.children);

        if let Some(footer) = self.footer {
            panel = panel.child(footer);
        }

        let dialog_panel = div()
            .absolute()
            .top(self.top)
            .left_0()
            .w_full()
            .flex()
            .justify_center()
            .child(panel);

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .child(backdrop)
            .child(dialog_panel)
    }
}

/// Content container for dialog body content.
#[derive(IntoElement)]
pub struct DialogContent {
    children: Vec<AnyElement>,
}

impl DialogContent {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for DialogContent {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for DialogContent {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for DialogContent {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(cx.theme().tokens.sizes.space_3)
            .children(self.children)
    }
}

/// Header section of a dialog, typically containing title and description.
#[derive(IntoElement)]
pub struct DialogHeader {
    children: Vec<AnyElement>,
}

impl DialogHeader {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for DialogHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for DialogHeader {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for DialogHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(cx.theme().tokens.sizes.space_2)
            .children(self.children)
    }
}

/// Title element for a dialog header.
#[derive(IntoElement)]
pub struct DialogTitle {
    children: Vec<AnyElement>,
}

impl DialogTitle {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for DialogTitle {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for DialogTitle {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for DialogTitle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id("dialog-title")
            .text_size(cx.theme().tokens.sizes.text_md)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(cx.theme().tokens.chrome.text_on_chrome)
            .children(self.children)
    }
}

/// Description element for secondary dialog text.
#[derive(IntoElement)]
pub struct DialogDescription {
    children: Vec<AnyElement>,
}

impl DialogDescription {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for DialogDescription {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for DialogDescription {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for DialogDescription {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id("dialog-description")
            .text_size(cx.theme().tokens.sizes.text_sm)
            .text_color(cx.theme().tokens.chrome.text_chrome_secondary)
            .children(self.children)
    }
}

/// Footer section of a dialog, typically containing action buttons.
#[derive(IntoElement)]
pub struct DialogFooter {
    children: Vec<AnyElement>,
}

impl DialogFooter {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }
}

impl Default for DialogFooter {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for DialogFooter {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for DialogFooter {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .gap(cx.theme().tokens.sizes.space_2)
            .justify_end()
            .children(self.children)
    }
}

#[derive(Clone, Debug)]
pub struct ConfirmDialog {
    pub title: SharedString,
    pub message: SharedString,
    pub cancel_label: SharedString,
    pub confirm_label: SharedString,
    pub confirm_variant: ButtonVariant,
    pub width: Pixels,
    pub top: Pixels,
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
            width: px(420.0),
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

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn top(mut self, top: Pixels) -> Self {
        self.top = top;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmDialogEvent {
    Cancelled,
    Confirmed,
}

pub struct ConfirmDialogView {
    dialog: ConfirmDialog,
    focus_handle: FocusHandle,
    cancel_focus_handle: FocusHandle,
    confirm_focus_handle: FocusHandle,
    dismissed_by_action: bool,
}

impl ConfirmDialogView {
    pub fn new(dialog: ConfirmDialog, cx: &mut Context<Self>) -> Self {
        Self {
            dialog,
            focus_handle: cx.focus_handle().tab_stop(false),
            cancel_focus_handle: cx.focus_handle().tab_index(1).tab_stop(true),
            confirm_focus_handle: cx.focus_handle().tab_index(2).tab_stop(true),
            dismissed_by_action: false,
        }
    }

    fn emit_cancelled(&mut self, cx: &mut Context<Self>) {
        self.dismissed_by_action = true;
        cx.emit(ConfirmDialogEvent::Cancelled);
        cx.emit(DismissEvent);
    }

    fn emit_confirmed(&mut self, cx: &mut Context<Self>) {
        self.dismissed_by_action = true;
        cx.emit(ConfirmDialogEvent::Confirmed);
        cx.emit(DismissEvent);
    }

    fn emit_cancelled_without_dismiss(&mut self, cx: &mut Context<Self>) {
        self.dismissed_by_action = true;
        cx.emit(ConfirmDialogEvent::Cancelled);
    }

    fn cancel(&mut self, _: &CancelDialogAction, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_cancelled(cx);
        cx.stop_propagation();
    }

    fn confirm(&mut self, _: &ConfirmDialogAction, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_confirmed(cx);
        cx.stop_propagation();
    }
}

impl Focusable for ConfirmDialogView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<ConfirmDialogEvent> for ConfirmDialogView {}

impl EventEmitter<DismissEvent> for ConfirmDialogView {}

impl ModalView for ConfirmDialogView {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DismissDecision {
        if !self.dismissed_by_action {
            self.emit_cancelled_without_dismiss(cx);
        }
        DismissDecision::Dismiss(true)
    }
}

impl Render for ConfirmDialogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = &cx.theme().tokens;
        let cancel_button = Button::new("confirm-dialog-cancel", self.dialog.cancel_label.clone())
            .variant(ButtonVariant::Secondary)
            .size(ButtonSize::Small)
            .focus_handle(self.cancel_focus_handle.clone())
            .activate_on_mouse_down()
            .on_click(cx.listener(|view, _event, _window, cx| {
                view.emit_cancelled(cx);
                cx.stop_propagation();
            }));

        let confirm_button =
            Button::new("confirm-dialog-confirm", self.dialog.confirm_label.clone())
                .variant(self.dialog.confirm_variant)
                .size(ButtonSize::Small)
                .focus_handle(self.confirm_focus_handle.clone())
                .activate_on_mouse_down()
                .on_click(cx.listener(|view, _event, _window, cx| {
                    view.emit_confirmed(cx);
                    cx.stop_propagation();
                }));

        FocusTraversal::new(
            div()
                .key_context(CONFIRM_DIALOG_CONTEXT)
                .track_focus(&self.focus_handle)
                .occlude()
                .bg(tokens.chrome.surface)
                .border_1()
                .border_color(tokens.chrome.border_default)
                .rounded(tokens.sizes.radius_lg)
                .shadow(vec![
                    tokens.chrome.shadow_lg.to_box_shadow(false),
                    tokens.chrome.inset_highlight.to_box_shadow(true),
                ])
                .w(self.dialog.width)
                .p(tokens.sizes.space_4)
                .flex()
                .flex_col()
                .gap(tokens.sizes.space_3)
                .on_action(cx.listener(Self::confirm))
                .on_action(cx.listener(Self::cancel))
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .child(
                    DialogHeader::new()
                        .child(DialogTitle::new().child(self.dialog.title.clone()))
                        .child(DialogDescription::new().child(self.dialog.message.clone())),
                )
                .child(
                    DialogFooter::new()
                        .child(cancel_button)
                        .child(confirm_button),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        AppContext as _, Context, Focusable, InputEvent as _, IntoElement, KeyUpEvent, Keystroke,
        ParentElement as _, Render, Styled as _, TestAppContext, Window, div,
    };

    use super::*;

    struct ConfirmDialogHarness {
        dialog: gpui::Entity<ConfirmDialogView>,
        events: Rc<RefCell<Vec<ConfirmDialogEvent>>>,
    }

    impl ConfirmDialogHarness {
        fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
            let dialog = cx.new(|cx| {
                ConfirmDialogView::new(
                    ConfirmDialog::new("Delete File", "This cannot be undone.", "Delete")
                        .confirm_variant(ButtonVariant::Danger),
                    cx,
                )
            });
            let events = Rc::new(RefCell::new(Vec::new()));
            let events_for_subscription = Rc::clone(&events);
            cx.subscribe(&dialog, move |_harness: &mut Self, _dialog, event, _cx| {
                events_for_subscription.borrow_mut().push(*event);
            })
            .detach();

            Self { dialog, events }
        }
    }

    impl Render for ConfirmDialogHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.dialog.clone())
        }
    }

    fn init_confirm_dialog_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx);
            cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
        });
    }

    #[gpui::test]
    fn confirm_dialog_view_emits_confirm_action(cx: &mut TestAppContext) {
        init_confirm_dialog_test(cx);
        let (harness, cx) = cx.add_window_view(ConfirmDialogHarness::new);
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        let focus = dialog.read_with(cx, |dialog, cx| dialog.focus_handle(cx));

        cx.update(|window, cx| {
            window.focus(&focus, cx);
            focus.dispatch_action(&ConfirmDialogAction, window, cx);
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(
                harness.events.borrow().as_slice(),
                &[ConfirmDialogEvent::Confirmed]
            );
        });
    }

    #[gpui::test]
    fn confirm_dialog_view_emits_cancel_action(cx: &mut TestAppContext) {
        init_confirm_dialog_test(cx);
        let (harness, cx) = cx.add_window_view(ConfirmDialogHarness::new);
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        let focus = dialog.read_with(cx, |dialog, cx| dialog.focus_handle(cx));

        cx.update(|window, cx| {
            window.focus(&focus, cx);
            focus.dispatch_action(&CancelDialogAction, window, cx);
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(
                harness.events.borrow().as_slice(),
                &[ConfirmDialogEvent::Cancelled]
            );
        });
    }

    #[gpui::test]
    fn confirm_dialog_view_uses_gpui_focus_traversal(cx: &mut TestAppContext) {
        init_confirm_dialog_test(cx);
        let (harness, cx) = cx.add_window_view(ConfirmDialogHarness::new);
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        let (dialog_focus, cancel_focus, confirm_focus) = dialog.read_with(cx, |dialog, cx| {
            (
                dialog.focus_handle(cx),
                dialog.cancel_focus_handle.clone(),
                dialog.confirm_focus_handle.clone(),
            )
        });

        cx.update(|window, cx| {
            window.focus(&dialog_focus, cx);
            dialog_focus.dispatch_action(&FocusNext, window, cx);
            assert!(cancel_focus.is_focused(window));

            cancel_focus.dispatch_action(&FocusNext, window, cx);
            assert!(confirm_focus.is_focused(window));

            confirm_focus.dispatch_action(&FocusPrevious, window, cx);
            assert!(cancel_focus.is_focused(window));
        });
    }

    #[gpui::test]
    fn focused_confirm_dialog_button_activates_with_space(cx: &mut TestAppContext) {
        init_confirm_dialog_test(cx);
        let (harness, cx) = cx.add_window_view(ConfirmDialogHarness::new);
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        let confirm_focus = dialog.read_with(cx, |dialog, _| dialog.confirm_focus_handle.clone());

        cx.update(|window, cx| {
            window.focus(&confirm_focus, cx);
            let keystroke = Keystroke::parse("space").unwrap();
            window.dispatch_keystroke(keystroke.clone(), cx);
            window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(
                harness.events.borrow().as_slice(),
                &[ConfirmDialogEvent::Confirmed]
            );
        });
    }

    #[gpui::test]
    fn confirm_dialog_view_treats_modal_dismiss_as_cancel(cx: &mut TestAppContext) {
        init_confirm_dialog_test(cx);
        let (harness, cx) = cx.add_window_view(ConfirmDialogHarness::new);
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());

        cx.update(|window, cx| {
            let decision = dialog.update(cx, |dialog, cx| dialog.on_before_dismiss(window, cx));
            assert_eq!(decision, DismissDecision::Dismiss(true));
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(
                harness.events.borrow().as_slice(),
                &[ConfirmDialogEvent::Cancelled]
            );
        });
    }

    #[gpui::test]
    fn confirm_dialog_view_does_not_cancel_after_action_dismiss(cx: &mut TestAppContext) {
        init_confirm_dialog_test(cx);
        let (harness, cx) = cx.add_window_view(ConfirmDialogHarness::new);
        let dialog = harness.read_with(cx, |harness, _| harness.dialog.clone());
        let focus = dialog.read_with(cx, |dialog, cx| dialog.focus_handle(cx));

        cx.update(|window, cx| {
            window.focus(&focus, cx);
            focus.dispatch_action(&ConfirmDialogAction, window, cx);
            let decision = dialog.update(cx, |dialog, cx| dialog.on_before_dismiss(window, cx));
            assert_eq!(decision, DismissDecision::Dismiss(true));
        });

        harness.read_with(cx, |harness, _| {
            assert_eq!(
                harness.events.borrow().as_slice(),
                &[ConfirmDialogEvent::Confirmed]
            );
        });
    }
}
