// ABOUTME: Shared modal presentation layer for GPUI-managed views
// ABOUTME: Owns modal focus, dismiss, occlusion, and focus restoration behaviour

use gpui::{
    AnyView, App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable as _, InteractiveElement, IntoElement, KeyBinding, ManagedView, MouseButton,
    ParentElement, Render, Styled, Subscription, Window, div, px,
};

use crate::actions::dialog::Cancel as CancelDialogAction;

const MODAL_BACKDROP_ALPHA_LIGHT: f32 = 0.70;
const MODAL_BACKDROP_ALPHA_DARK: f32 = 0.45;
pub(crate) const MODAL_LAYER_CONTEXT: &str = "ModalLayer";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelDialogAction,
        Some(MODAL_LAYER_CONTEXT),
    )]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DismissDecision {
    Dismiss(bool),
    Pending,
}

pub trait ModalView: ManagedView {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> DismissDecision {
        DismissDecision::Dismiss(true)
    }

    fn fade_out_background(&self) -> bool {
        true
    }

    fn render_bare(&self) -> bool {
        false
    }
}

trait ModalViewHandle {
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut App) -> DismissDecision;
    fn view(&self) -> AnyView;
    fn fade_out_background(&self, cx: &mut App) -> bool;
    fn render_bare(&self, cx: &mut App) -> bool;
}

impl<V: ModalView> ModalViewHandle for Entity<V> {
    fn on_before_dismiss(&mut self, window: &mut Window, cx: &mut App) -> DismissDecision {
        self.update(cx, |this, cx| this.on_before_dismiss(window, cx))
    }

    fn view(&self) -> AnyView {
        self.clone().into()
    }

    fn fade_out_background(&self, cx: &mut App) -> bool {
        self.read(cx).fade_out_background()
    }

    fn render_bare(&self, cx: &mut App) -> bool {
        self.read(cx).render_bare()
    }
}

struct ActiveModal {
    modal: Box<dyn ModalViewHandle>,
    _subscriptions: [Subscription; 2],
    previous_focus_handle: Option<FocusHandle>,
    focus_handle: FocusHandle,
}

pub struct ModalLayer {
    active_modal: Option<ActiveModal>,
    dismiss_on_focus_lost: bool,
}

pub struct ModalOpenedEvent;

impl EventEmitter<ModalOpenedEvent> for ModalLayer {}

impl ModalLayer {
    pub fn new() -> Self {
        Self {
            active_modal: None,
            dismiss_on_focus_lost: false,
        }
    }

    pub fn toggle_modal<V, B>(&mut self, window: &mut Window, cx: &mut Context<Self>, build_view: B)
    where
        V: ModalView,
        B: FnOnce(&mut Window, &mut Context<V>) -> V,
    {
        if let Some(active_modal) = &self.active_modal {
            let should_close = active_modal.modal.view().downcast::<V>().is_ok();
            let did_close = self.hide_modal(window, cx);
            if should_close || !did_close {
                return;
            }
        }

        let new_modal = cx.new(|cx| build_view(window, cx));
        self.show_modal(new_modal, window, cx);
        cx.emit(ModalOpenedEvent);
    }

    pub fn show_modal<V>(
        &mut self,
        new_modal: Entity<V>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        V: ModalView,
    {
        let focus_handle = cx.focus_handle();
        self.active_modal = Some(ActiveModal {
            modal: Box::new(new_modal.clone()),
            _subscriptions: [
                cx.subscribe_in(
                    &new_modal,
                    window,
                    |this, _, _: &DismissEvent, window, cx| {
                        this.hide_modal(window, cx);
                    },
                ),
                cx.on_focus_out(&focus_handle, window, |this, _event, window, cx| {
                    if this.dismiss_on_focus_lost {
                        this.hide_modal(window, cx);
                    }
                }),
            ],
            previous_focus_handle: window.focused(cx),
            focus_handle,
        });

        cx.defer_in(window, move |_, window, cx| {
            window.focus(&new_modal.focus_handle(cx), cx);
        });
        cx.notify();
    }

    pub fn hide_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(active_modal) = self.active_modal.as_mut() else {
            self.dismiss_on_focus_lost = false;
            return false;
        };

        match active_modal.modal.on_before_dismiss(window, cx) {
            DismissDecision::Dismiss(true) => {}
            DismissDecision::Dismiss(false) => {
                self.dismiss_on_focus_lost = true;
                return false;
            }
            DismissDecision::Pending => {
                self.dismiss_on_focus_lost = false;
                return false;
            }
        }

        if let Some(active_modal) = self.active_modal.take() {
            if let Some(previous_focus) = active_modal.previous_focus_handle
                && active_modal.focus_handle.contains_focused(window, cx)
            {
                previous_focus.focus(window, cx);
            }
            cx.notify();
        }

        self.dismiss_on_focus_lost = false;
        true
    }

    pub fn active_modal<V>(&self) -> Option<Entity<V>>
    where
        V: 'static,
    {
        let active_modal = self.active_modal.as_ref()?;
        active_modal.modal.view().downcast::<V>().ok()
    }

    pub fn has_active_modal(&self) -> bool {
        self.active_modal.is_some()
    }

    fn dismiss(&mut self, _: &CancelDialogAction, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_modal(window, cx);
        cx.stop_propagation();
    }
}

impl Default for ModalLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for ModalLayer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(active_modal) = &self.active_modal else {
            return div().into_any_element();
        };

        if active_modal.modal.render_bare(cx) {
            return active_modal.modal.view().into_any_element();
        }

        let (background_lightness, surface_overlay) = {
            let tokens = &cx.global::<crate::Theme>().tokens;
            (tokens.editor.background.l, tokens.chrome.surface_overlay)
        };
        let backdrop_alpha = if background_lightness < 0.5 {
            MODAL_BACKDROP_ALPHA_DARK
        } else {
            MODAL_BACKDROP_ALPHA_LIGHT
        };

        let mut backdrop = div()
            .absolute()
            .inset_0()
            .size_full()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.hide_modal(window, cx);
                }),
            );

        if active_modal.modal.fade_out_background(cx) {
            backdrop = backdrop.bg(surface_overlay.alpha(backdrop_alpha));
        }

        backdrop
            .child(
                div()
                    .absolute()
                    .top(px(96.0))
                    .left_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .key_context(MODAL_LAYER_CONTEXT)
                    .track_focus(&active_modal.focus_handle)
                    .on_action(cx.listener(Self::dismiss))
                    .child(
                        div()
                            .occlude()
                            .child(active_modal.modal.view())
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            }),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        Context, DismissEvent, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
        IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, Window, div, px,
    };

    use super::*;

    fn init_theme(cx: &mut TestAppContext) {
        cx.update(|cx| {
            init(cx);
            cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark()));
        });
    }

    struct TestModal {
        focus_handle: FocusHandle,
        dismiss_allowed: Rc<Cell<bool>>,
        before_dismiss_count: Rc<Cell<usize>>,
    }

    impl TestModal {
        fn new(
            cx: &mut Context<Self>,
            dismiss_allowed: Rc<Cell<bool>>,
            before_dismiss_count: Rc<Cell<usize>>,
        ) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                dismiss_allowed,
                before_dismiss_count,
            }
        }
    }

    impl Focusable for TestModal {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    impl EventEmitter<DismissEvent> for TestModal {}

    impl ModalView for TestModal {
        fn on_before_dismiss(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> DismissDecision {
            self.before_dismiss_count
                .set(self.before_dismiss_count.get() + 1);
            DismissDecision::Dismiss(self.dismiss_allowed.get())
        }
    }

    impl Render for TestModal {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.focus_handle)
                .size(px(200.0))
                .child("modal")
        }
    }

    struct ModalLayerHarness {
        previous_focus: FocusHandle,
        layer: Entity<ModalLayer>,
    }

    impl ModalLayerHarness {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let previous_focus = cx.focus_handle();
            window.focus(&previous_focus, cx);
            Self {
                previous_focus,
                layer: cx.new(|_| ModalLayer::new()),
            }
        }
    }

    impl Render for ModalLayerHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(
                    div()
                        .id("previous-focus")
                        .track_focus(&self.previous_focus)
                        .size(px(10.0)),
                )
                .child(self.layer.clone())
        }
    }

    fn new_test_modal(
        cx: &mut gpui::VisualTestContext,
        dismiss_allowed: Rc<Cell<bool>>,
        before_dismiss_count: Rc<Cell<usize>>,
    ) -> Entity<TestModal> {
        cx.new(|cx| TestModal::new(cx, dismiss_allowed, before_dismiss_count))
    }

    #[gpui::test]
    fn hide_modal_restores_previous_focus(cx: &mut TestAppContext) {
        init_theme(cx);
        let (harness, cx) = cx.add_window_view(ModalLayerHarness::new);
        let (layer, previous_focus) = harness.read_with(cx, |harness, _| {
            (harness.layer.clone(), harness.previous_focus.clone())
        });
        let modal = new_test_modal(cx, Rc::new(Cell::new(true)), Rc::new(Cell::new(0)));

        cx.update(|window, cx| {
            layer.update(cx, |layer, cx| {
                layer.show_modal(modal.clone(), window, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            assert!(modal.read(cx).focus_handle.is_focused(window));
            assert!(layer.update(cx, |layer, cx| layer.hide_modal(window, cx)));
            assert!(previous_focus.is_focused(window));
        });
    }

    #[gpui::test]
    fn dismiss_event_hides_active_modal(cx: &mut TestAppContext) {
        init_theme(cx);
        let (harness, cx) = cx.add_window_view(ModalLayerHarness::new);
        let layer = harness.read_with(cx, |harness, _| harness.layer.clone());
        let modal = new_test_modal(cx, Rc::new(Cell::new(true)), Rc::new(Cell::new(0)));

        cx.update(|window, cx| {
            layer.update(cx, |layer, cx| {
                layer.show_modal(modal.clone(), window, cx);
                assert!(layer.has_active_modal());
            });
        });

        cx.update(|_window, cx| {
            modal.update(cx, |_, cx| cx.emit(DismissEvent));
        });

        cx.update(|_window, cx| {
            assert!(!layer.read(cx).has_active_modal());
        });
    }

    #[gpui::test]
    fn escape_hides_active_modal(cx: &mut TestAppContext) {
        init_theme(cx);
        let (harness, cx) = cx.add_window_view(ModalLayerHarness::new);
        let (layer, previous_focus) = harness.read_with(cx, |harness, _| {
            (harness.layer.clone(), harness.previous_focus.clone())
        });
        let before_dismiss_count = Rc::new(Cell::new(0));
        let modal = new_test_modal(
            cx,
            Rc::new(Cell::new(true)),
            Rc::clone(&before_dismiss_count),
        );

        cx.update(|window, cx| {
            layer.update(cx, |layer, cx| {
                layer.show_modal(modal.clone(), window, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            window.dispatch_keystroke(gpui::Keystroke::parse("escape").unwrap(), cx);
            assert!(!layer.read(cx).has_active_modal());
            assert!(previous_focus.is_focused(window));
        });
        assert_eq!(before_dismiss_count.get(), 1);
    }

    #[gpui::test]
    fn escape_respects_before_dismiss(cx: &mut TestAppContext) {
        init_theme(cx);
        let (harness, cx) = cx.add_window_view(ModalLayerHarness::new);
        let layer = harness.read_with(cx, |harness, _| harness.layer.clone());
        let before_dismiss_count = Rc::new(Cell::new(0));
        let modal = new_test_modal(
            cx,
            Rc::new(Cell::new(false)),
            Rc::clone(&before_dismiss_count),
        );

        cx.update(|window, cx| {
            layer.update(cx, |layer, cx| {
                layer.show_modal(modal.clone(), window, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            window.dispatch_keystroke(gpui::Keystroke::parse("escape").unwrap(), cx);
            assert!(layer.read(cx).has_active_modal());
            assert!(modal.read(cx).focus_handle.is_focused(window));
        });
        assert_eq!(before_dismiss_count.get(), 1);
    }

    #[gpui::test]
    fn before_dismiss_can_block_closure(cx: &mut TestAppContext) {
        init_theme(cx);
        let (harness, cx) = cx.add_window_view(ModalLayerHarness::new);
        let layer = harness.read_with(cx, |harness, _| harness.layer.clone());
        let dismiss_allowed = Rc::new(Cell::new(false));
        let before_dismiss_count = Rc::new(Cell::new(0));
        let modal = new_test_modal(cx, dismiss_allowed, Rc::clone(&before_dismiss_count));

        cx.update(|window, cx| {
            layer.update(cx, |layer, cx| {
                layer.show_modal(modal, window, cx);
                assert!(!layer.hide_modal(window, cx));
                assert!(layer.has_active_modal());
            });
        });

        assert_eq!(before_dismiss_count.get(), 1);
    }
}
