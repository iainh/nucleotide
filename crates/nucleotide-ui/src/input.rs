// ABOUTME: Input component following Zed's design patterns
// ABOUTME: Provides consistent input styling and behavior

use crate::ComponentFactory;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, ElementId, FocusHandle, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px,
};

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum InputVariant {
    #[default]
    Default,
    Ghost,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum InputSize {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(IntoElement)]
pub struct Input {
    id: ElementId,
    variant: InputVariant,
    size: InputSize,
    disabled: bool,
    focus_handle: FocusHandle,
    value: SharedString,
    placeholder: Option<SharedString>,
    error: Option<SharedString>,
    start_slot: Option<gpui::AnyElement>,
    end_slot: Option<gpui::AnyElement>,
}

impl Input {
    pub fn new(id: impl Into<ElementId>, focus_handle: FocusHandle) -> Self {
        Self {
            id: id.into(),
            variant: InputVariant::Default,
            size: InputSize::Medium,
            disabled: false,
            focus_handle,
            value: SharedString::default(),
            placeholder: None,
            error: None,
            start_slot: None,
            end_slot: None,
        }
    }

    pub fn variant(mut self, variant: InputVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: InputSize) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = value.into();
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn start_slot(mut self, slot: impl IntoElement) -> Self {
        self.start_slot = Some(slot.into_any_element());
        self
    }

    pub fn end_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_slot = Some(slot.into_any_element());
        self
    }
}

crate::impl_component!(Input);

impl ComponentFactory for Input {
    fn new(_id: impl Into<ElementId>) -> Self {
        // Input requires a focus handle, so we can't implement ComponentFactory safely without context
        // But ComponentFactory trait doesn't take context or allow creating handle.
        // So we might have to panic or use a dummy handle if used via factory?
        // Or we just don't implement ComponentFactory for Input if it requires FocusHandle.
        // But for now, let's skip ComponentFactory or use a default handle?
        // GPUI FocusHandle requires WindowContext.
        // So Input cannot implement ComponentFactory cleanly if `new` doesn't take cx.
        unimplemented!("Input requires FocusHandle")
    }
}

impl RenderOnce for Input {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<crate::Theme>();
        let input_tokens = theme.tokens.input_tokens();

        let is_focused = self.focus_handle.is_focused(_window);
        let has_error = self.error.is_some();

        div()
            .id(self.id)
            .track_focus(&self.focus_handle)
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .bg(if self.disabled {
                input_tokens.background_disabled
            } else if is_focused {
                input_tokens.background_focus
            } else {
                input_tokens.background
            })
            .border_1()
            .border_color(if self.disabled {
                input_tokens.border_disabled
            } else if has_error {
                input_tokens.border_error
            } else if is_focused {
                input_tokens.border_focus
            } else {
                input_tokens.border
            })
            .rounded_md()
            .px_2()
            .py_1()
            .text_size(match self.size {
                InputSize::Small => theme.tokens.sizes.text_sm,
                InputSize::Medium => theme.tokens.sizes.text_md,
                InputSize::Large => theme.tokens.sizes.text_lg,
            })
            .text_color(if self.disabled {
                input_tokens.text_disabled
            } else {
                input_tokens.text
            })
            .when(is_focused && !has_error, |this| {
                // Add focus ring
                this.shadow(vec![gpui::BoxShadow {
                    color: input_tokens.focus_ring,
                    offset: gpui::point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.0),
                }])
            })
            .when(has_error, |this| {
                // Add error ring
                this.shadow(vec![gpui::BoxShadow {
                    color: input_tokens.border_error, // Use border error color for ring
                    offset: gpui::point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.0),
                }])
            })
            .when_some(self.start_slot, |this, slot| {
                this.child(div().mr_2().child(slot))
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(if self.value.is_empty() {
                        if let Some(placeholder) = self.placeholder {
                            div()
                                .text_color(input_tokens.placeholder)
                                .child(placeholder)
                        } else {
                            div()
                        }
                    } else {
                        div().child(self.value)
                    }),
            )
            .when_some(self.end_slot, |this, slot| {
                this.child(div().ml_2().child(slot))
            })
    }
}
