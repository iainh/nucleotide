// ABOUTME: Token-integrated checkbox component for binary UI settings.
// ABOUTME: Renders icon-backed checked and unchecked states with GPUI focus handling.

use crate::icon as svg;
use crate::tokens::CheckboxTokens;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, ClickEvent, ElementId, FocusHandle, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, Window, div, relative,
};
use std::sync::Arc;

type CheckboxChangeHandler = Arc<dyn Fn(bool, &mut Window, &mut App)>;

/// The bundled Pierre squircle spans the icon's full 16 units.
const BOX_SCALE: f32 = 1.0;

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
            // Fills the row, so the drawn box is as large as the control allows.
            icon_size: tokens.sizes.space_6,
            gap: tokens.sizes.space_1,
            padding_x: tokens.sizes.space_1,
            font_size: tokens.sizes.text_sm,
        },
        CheckboxSize::Medium => CheckboxMetrics {
            height: tokens.sizes.button_height_md,
            icon_size: tokens.sizes.space_7,
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
    indeterminate: bool,
    disabled: bool,
    size: CheckboxSize,
    focus_handle: Option<FocusHandle>,
    on_change: Option<CheckboxChangeHandler>,
    tooltip: Option<SharedString>,
    tooltip_detail: Option<SharedString>,
    aria_label: Option<SharedString>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            checked: false,
            indeterminate: false,
            disabled: false,
            size: CheckboxSize::default(),
            focus_handle: None,
            on_change: None,
            tooltip: None,
            tooltip_detail: None,
            aria_label: None,
        }
    }

    /// Text shown when the pointer rests on the checkbox.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// A second, quieter tooltip line, such as the keys that do the same thing.
    pub fn tooltip_detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.tooltip_detail = Some(detail.into());
        self
    }

    /// The name assistive technology announces, for a checkbox without a
    /// visible label or whose label alone is ambiguous.
    pub fn aria_label(mut self, label: impl Into<SharedString>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// The name assistive technology announces: the accessible label, else
    /// the visible label, else none.
    pub fn accessible_label(&self) -> Option<&SharedString> {
        self.aria_label
            .as_ref()
            .or(Some(&self.label).filter(|label| !label.is_empty()))
    }

    pub fn tooltip_text(&self) -> Option<&SharedString> {
        self.tooltip.as_ref()
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Mixed selection. Activating a mixed checkbox selects all children.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
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
        self.indeterminate || !self.checked
    }

    fn icon_color(&self, tokens: &CheckboxTokens) -> gpui::Hsla {
        if self.disabled {
            tokens.disabled_text
        } else if self.checked || self.indeterminate {
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
        // Drawn boxes match the SVG square and keep the label where the icon puts it.
        let drawn_box = |content: gpui::Div| {
            div()
                .size(metrics.icon_size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(content.size(metrics.icon_size * BOX_SCALE))
                .into_any_element()
        };
        let icon = if let Some(c) = theme.tokens.chrome.contrast {
            // Draw an opaque box in contrast mode rather than tinting the icon.
            let selected = (self.checked || self.indeterminate) && !disabled;
            drawn_box(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(if disabled { c.gray_text } else { c.button_text })
                    .bg(if selected { c.highlight } else { c.button_face })
                    .text_color(if selected {
                        c.highlight_text
                    } else {
                        c.gray_text
                    })
                    .when(self.checked || self.indeterminate, |icon| {
                        icon.child(if self.indeterminate { "−" } else { "✓" })
                    }),
            )
        } else if self.indeterminate {
            drawn_box(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .rounded(theme.tokens.sizes.radius_sm)
                    .border_color(icon_color)
                    .text_color(icon_color)
                    .child("−"),
            )
        } else {
            icon.into_any_element()
        };

        let accessible_label = self.accessible_label().cloned();
        let mut element = div()
            .id(self.id)
            .role(gpui::Role::CheckBox)
            .aria_toggled(if self.indeterminate {
                gpui::Toggled::Mixed
            } else if self.checked {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .when_some(accessible_label, |element, label| element.aria_label(label))
            .when_some(self.tooltip, |element, tooltip| {
                let detail = self.tooltip_detail;
                element.tooltip(move |_, cx| {
                    cx.new(|_| {
                        let tooltip = crate::TextTooltip::new(tooltip.clone());
                        match detail.clone() {
                            Some(detail) => tooltip.detail(detail),
                            None => tooltip,
                        }
                    })
                    .into()
                })
            })
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
            .opacity(if disabled && theme.tokens.chrome.contrast.is_none() {
                0.7
            } else {
                1.0
            })
            .child(icon)
            .child(self.label);

        if let Some(focus_handle) = focus_handle.as_ref() {
            element = element
                .track_focus(focus_handle)
                .tab_stop(!disabled)
                .focus_visible(|style| {
                    let style = style.bg(checkbox_tokens.focus_background);
                    if let Some(c) = theme.tokens.chrome.contrast {
                        style.border_1().border_color(c.button_text)
                    } else {
                        style
                    }
                });
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
    fn unlabelled_checkbox_takes_its_accessible_name_from_aria_label() {
        let line = Checkbox::new("stage-line-42", "")
            .aria_label("Stage line 42")
            .tooltip("Stage line 42");
        assert_eq!(
            line.accessible_label().map(|l| l.as_ref()),
            Some("Stage line 42")
        );
        assert_eq!(
            line.tooltip_text().map(|t| t.as_ref()),
            Some("Stage line 42")
        );

        // A visible label names the checkbox unless an accessible label overrides it.
        let amend = Checkbox::new("amend", "Amend");
        assert_eq!(amend.accessible_label().map(|l| l.as_ref()), Some("Amend"));
        assert_eq!(amend.tooltip_text(), None);
        let file = Checkbox::new("file", "src/lib.rs").aria_label("Stage src/lib.rs");
        assert_eq!(
            file.accessible_label().map(|l| l.as_ref()),
            Some("Stage src/lib.rs")
        );
        assert_eq!(Checkbox::new("bare", "").accessible_label(), None);
    }

    #[gpui::test]
    fn hovering_a_checkbox_shows_its_tooltip(cx: &mut gpui::TestAppContext) {
        use gpui::{Context, Modifiers, Render, VisualTestContext};

        struct Harness;
        impl Render for Harness {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(
                    Checkbox::new("stage-line", "")
                        .tooltip("Stage line 42")
                        .aria_label("Stage line 42"),
                )
            }
        }

        cx.update(|cx| cx.set_global(crate::Theme::from_tokens(crate::DesignTokens::dark())));
        let (_, cx) = cx.add_window_view(|_, _| Harness);
        let tooltip_shown = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear();
            });
            cx.debug_bounds("text-tooltip").is_some()
        };
        assert!(!tooltip_shown(cx));
        cx.simulate_mouse_move(
            gpui::point(gpui::px(8.0), gpui::px(8.0)),
            None,
            Modifiers::none(),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert!(tooltip_shown(cx));
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
