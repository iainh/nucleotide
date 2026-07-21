// ABOUTME: Token-integrated checkbox component for binary UI settings.
// ABOUTME: Renders Phosphor-duotone-backed checked and unchecked states with GPUI focus handling.

use crate::tokens::CheckboxTokens;
use gpui::{
    App, ClickEvent, ElementId, FocusHandle, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
    div, relative, svg,
};
use std::sync::Arc;

type CheckboxChangeHandler = Arc<dyn Fn(bool, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckboxSize {
    Small,
    #[default]
    Medium,
}

#[derive(Clone, Copy)]
struct CheckboxMetrics {
    height: Pixels,
    icon_size: Pixels,
    gap: Pixels,
    padding_x: Pixels,
    font_size: Pixels,
}

fn checkbox_metrics(size: CheckboxSize, tokens: &crate::DesignTokens) -> CheckboxMetrics {
    match size {
        CheckboxSize::Small => CheckboxMetrics {
            height: tokens.sizes.space_6,
            icon_size: tokens.sizes.text_lg,
            gap: tokens.sizes.space_1,
            padding_x: tokens.sizes.space_1,
            font_size: tokens.sizes.text_sm,
        },
        CheckboxSize::Medium => CheckboxMetrics {
            height: tokens.sizes.button_height_md,
            icon_size: tokens.sizes.text_xl,
            gap: tokens.sizes.space_2,
            padding_x: tokens.sizes.space_2,
            font_size: tokens.sizes.text_md,
        },
    }
}

/// A reusable checkbox component for binary settings.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    label: SharedString,
    checked: bool,
    disabled: bool,
    size: CheckboxSize,
    focus_handle: Option<FocusHandle>,
    on_change: Option<CheckboxChangeHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            checked: false,
            disabled: false,
            size: CheckboxSize::default(),
            focus_handle: None,
            on_change: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn size(mut self, size: CheckboxSize) -> Self {
        self.size = size;
        self
    }

    pub fn focus_handle(mut self, focus_handle: FocusHandle) -> Self {
        self.focus_handle = Some(focus_handle);
        self
    }

    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }

    pub fn is_checked(&self) -> bool {
        self.checked
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    fn icon_path(&self) -> &'static str {
        if self.checked {
            "icons/square-check-big.svg"
        } else {
            "icons/square.svg"
        }
    }

    fn toggled_checked(&self) -> bool {
        !self.checked
    }

    fn icon_color(&self, tokens: &CheckboxTokens) -> gpui::Hsla {
        if self.disabled {
            tokens.disabled_text
        } else if self.checked {
            tokens.checked_background
        } else {
            tokens.text_secondary
        }
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<crate::Theme>();
        let checkbox_tokens = theme.tokens.checkbox_tokens();
        let metrics = checkbox_metrics(self.size, &theme.tokens);
        let disabled = self.disabled;
        let toggled_checked = self.toggled_checked();
        let focus_handle = self.focus_handle.clone();
        let icon_color = self.icon_color(&checkbox_tokens);
        let text_color = if disabled {
            checkbox_tokens.disabled_text
        } else {
            checkbox_tokens.text
        };

        let icon = svg()
            .path(self.icon_path())
            .size(metrics.icon_size)
            .text_color(icon_color)
            .flex_shrink_0();

        let mut element = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .gap(metrics.gap)
            .h(metrics.height)
            .px(metrics.padding_x)
            .rounded(theme.tokens.sizes.radius_sm)
            .line_height(relative(1.0))
            .text_size(metrics.font_size)
            .text_color(text_color)
            .bg(if disabled {
                checkbox_tokens.disabled_background
            } else {
                checkbox_tokens.background
            })
            .opacity(if disabled { 0.7 } else { 1.0 })
            .child(icon)
            .child(self.label);

        if let Some(focus_handle) = focus_handle.as_ref() {
            element = element
                .track_focus(focus_handle)
                .tab_stop(!disabled)
                .focus_visible(|style| style.bg(checkbox_tokens.focus_background));
        }

        if disabled {
            element = element.cursor_not_allowed();
        } else {
            element = element.cursor_pointer().hover({
                let hover_bg = checkbox_tokens.background_hover;
                move |this| this.bg(hover_bg)
            });

            if let Some(on_change) = self.on_change {
                let mouse_on_change = on_change.clone();
                element = element
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        mouse_on_change(toggled_checked, window, cx);
                    })
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        if !matches!(event, ClickEvent::Keyboard(_)) {
                            return;
                        }
                        on_change(toggled_checked, window, cx);
                    });
            }
        }

        element
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkbox_builder_tracks_checked_and_disabled_state() {
        let checkbox = Checkbox::new("save-connection", "Save this connection")
            .checked(true)
            .disabled(true)
            .size(CheckboxSize::Small);

        assert!(checkbox.is_checked());
        assert!(checkbox.is_disabled());
        assert_eq!(checkbox.size, CheckboxSize::Small);
        assert_eq!(checkbox.icon_path(), "icons/square-check-big.svg");
        assert!(!checkbox.toggled_checked());
    }

    #[test]
    fn unchecked_checkbox_uses_square_icon() {
        let checkbox = Checkbox::new("save-connection", "Save this connection");

        assert!(!checkbox.is_checked());
        assert_eq!(checkbox.icon_path(), "icons/square.svg");
        assert!(checkbox.toggled_checked());
    }

    #[test]
    fn checkbox_tokens_are_theme_backed() {
        let tokens = crate::DesignTokens::dark().checkbox_tokens();

        assert!(tokens.checked_background.a > 0.0);
        assert!(tokens.focus_background.a > 0.0);
        assert!(tokens.disabled_text.a < tokens.text.a);
    }

    #[test]
    fn checkbox_tokens_do_not_define_a_control_border() {
        let tokens = crate::DesignTokens::dark().checkbox_tokens();

        assert_eq!(tokens.background.a, 0.0);
        assert_eq!(tokens.disabled_background.a, 0.0);
    }
}
