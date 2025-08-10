// ABOUTME: Generic overlay component that can wrap any view with modal styling
// ABOUTME: Provides consistent overlay behavior and dismissal handling

#![allow(dead_code)]

use crate::ui::common::{ModalContainer, ModalStyle};
use gpui::prelude::FluentBuilder;
use gpui::*;

/// Generic overlay that can wrap any view
pub struct Overlay<V: EventEmitter<DismissEvent>> {
    /// The wrapped view
    content: Entity<V>,
    /// Whether to show a backdrop
    show_backdrop: bool,
    /// Optional modal style override
    modal_style: Option<ModalStyle>,
}

impl<V: EventEmitter<DismissEvent>> Overlay<V> {
    /// Create a new overlay wrapping the given view
    pub fn new(content: Entity<V>) -> Self {
        Self {
            content,
            show_backdrop: true,
            modal_style: None,
        }
    }

    /// Set whether to show a backdrop
    pub fn with_backdrop(mut self, show: bool) -> Self {
        self.show_backdrop = show;
        self
    }

    /// Override the modal style
    pub fn with_style(mut self, style: ModalStyle) -> Self {
        self.modal_style = Some(style);
        self
    }
}

impl<V: EventEmitter<DismissEvent>> EventEmitter<DismissEvent> for Overlay<V> {}

impl<V: EventEmitter<DismissEvent>> Focusable for Overlay<V>
where
    V: Focusable,
{
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Delegate focus to the wrapped content
        self.content.focus_handle(cx)
    }
}

impl<V> Render for Overlay<V>
where
    V: EventEmitter<DismissEvent> + Focusable + Render,
{
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Subscribe to dismiss events from the content
        cx.subscribe(
            &self.content,
            |_this, _content, _event: &DismissEvent, cx| {
                // Re-emit the dismiss event
                cx.emit(DismissEvent);
            },
        )
        .detach();

        div()
            .key_context("Overlay")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .occlude()
            .when(self.show_backdrop, |this| {
                this.child(ModalContainer::container())
            })
            .child(
                div()
                    .flex()
                    .size_full()
                    .justify_center()
                    .items_start()
                    .pt_20()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| {
                            // Dismiss on backdrop click
                            cx.emit(DismissEvent);
                        }),
                    )
                    .child(
                        // Prevent clicks on content from dismissing
                        div()
                            .on_mouse_down(MouseButton::Left, |_event, _window, _cx| {
                                // Stop propagation
                            })
                            .child(self.content.clone()),
                    ),
            )
    }
}

/// Builder methods for creating overlays
impl<V: EventEmitter<DismissEvent>> Overlay<V> {
    /// Create an overlay that positions content at the top
    pub fn top(self) -> Self {
        // The default positioning is already at the top with pt_20
        self
    }

    /// Create an overlay that centers content
    pub fn centered(self) -> CenteredOverlay<V> {
        CenteredOverlay { inner: self }
    }

    /// Create an overlay that positions content at the bottom
    pub fn bottom(self) -> BottomOverlay<V> {
        BottomOverlay { inner: self }
    }
}

/// Centered overlay variant
pub struct CenteredOverlay<V: EventEmitter<DismissEvent>> {
    inner: Overlay<V>,
}

impl<V: EventEmitter<DismissEvent>> EventEmitter<DismissEvent> for CenteredOverlay<V> {}

impl<V: EventEmitter<DismissEvent>> Focusable for CenteredOverlay<V>
where
    V: Focusable,
{
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.inner.focus_handle(cx)
    }
}

impl<V> Render for CenteredOverlay<V>
where
    V: EventEmitter<DismissEvent> + Focusable + Render,
{
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Subscribe to dismiss events from the inner overlay
        cx.subscribe(
            &self.inner.content,
            |_this, _content, _event: &DismissEvent, cx| {
                cx.emit(DismissEvent);
            },
        )
        .detach();

        div()
            .key_context("CenteredOverlay")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .occlude()
            .when(self.inner.show_backdrop, |this| {
                this.child(ModalContainer::container())
            })
            .child(
                div()
                    .flex()
                    .size_full()
                    .justify_center()
                    .items_center() // Center vertically
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| {
                            cx.emit(DismissEvent);
                        }),
                    )
                    .child(
                        div()
                            .on_mouse_down(MouseButton::Left, |_event, _window, _cx| {
                                // Stop propagation
                            })
                            .child(self.inner.content.clone()),
                    ),
            )
    }
}

/// Bottom overlay variant
pub struct BottomOverlay<V: EventEmitter<DismissEvent>> {
    inner: Overlay<V>,
}

impl<V: EventEmitter<DismissEvent>> EventEmitter<DismissEvent> for BottomOverlay<V> {}

impl<V: EventEmitter<DismissEvent>> Focusable for BottomOverlay<V>
where
    V: Focusable,
{
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.inner.focus_handle(cx)
    }
}

impl<V> Render for BottomOverlay<V>
where
    V: EventEmitter<DismissEvent> + Focusable + Render,
{
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Subscribe to dismiss events from the inner overlay
        cx.subscribe(
            &self.inner.content,
            |_this, _content, _event: &DismissEvent, cx| {
                cx.emit(DismissEvent);
            },
        )
        .detach();

        div()
            .key_context("BottomOverlay")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .occlude()
            .when(self.inner.show_backdrop, |this| {
                this.child(ModalContainer::container())
            })
            .child(
                div()
                    .flex()
                    .size_full()
                    .justify_center()
                    .items_end() // Align to bottom
                    .pb_4() // Small padding from bottom
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| {
                            cx.emit(DismissEvent);
                        }),
                    )
                    .child(
                        div()
                            .on_mouse_down(MouseButton::Left, |_event, _window, _cx| {
                                // Stop propagation
                            })
                            .child(self.inner.content.clone()),
                    ),
            )
    }
}

/// Type aliases for common overlay types
pub type PickerOverlay = Overlay<crate::picker_view::PickerView>;
pub type PromptOverlay = Overlay<crate::prompt_view::PromptView>;
pub type CompletionOverlay = Overlay<crate::completion::CompletionView>;
