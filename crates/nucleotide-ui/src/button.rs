// ABOUTME: Button component following Zed's design patterns
// ABOUTME: Provides consistent button styling and behavior

use crate::spacing;
use gpui::prelude::FluentBuilder;
use gpui::*;

/// Button variant styles
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// Button sizes
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ButtonSize {
    Small,
    Medium,
    Large,
}

impl ButtonSize {
    fn padding(&self) -> (Pixels, Pixels) {
        match self {
            Self::Small => (spacing::XS, spacing::SM),
            Self::Medium => (spacing::SM, spacing::MD),
            Self::Large => (spacing::MD, spacing::LG),
        }
    }

    fn text_size(&self) -> Pixels {
        match self {
            Self::Small => px(12.),
            Self::Medium => px(14.),
            Self::Large => px(16.),
        }
    }
}

// Type alias for button click handler
type ButtonClickHandler = Box<dyn Fn(&MouseDownEvent, &mut App) + 'static>;

/// A reusable button component
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    icon: Option<SharedString>,
    icon_position: IconPosition,
    on_click: Option<ButtonClickHandler>,
    tooltip: Option<SharedString>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconPosition {
    Start,
    End,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: ButtonVariant::Primary,
            size: ButtonSize::Medium,
            disabled: false,
            icon: None,
            icon_position: IconPosition::Start,
            on_click: None,
            tooltip: None,
        }
    }

    /// Set button variant
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set button size
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Set disabled state
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Add an icon
    pub fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set icon position
    pub fn icon_position(mut self, position: IconPosition) -> Self {
        self.icon_position = position;
        self
    }

    /// Set click handler
    pub fn on_click(mut self, handler: impl Fn(&MouseDownEvent, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// Set tooltip
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<crate::Theme>();
        let (py, px) = self.size.padding();
        let text_size = self.size.text_size();

        let mut button = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(spacing::SM)
            .py(py)
            .px(px)
            .rounded_md()
            .text_size(text_size)
            .font_weight(FontWeight::MEDIUM);

        // Apply variant styling
        button = match self.variant {
            ButtonVariant::Primary => button
                .bg(if self.disabled {
                    hsla(theme.accent.h, theme.accent.s, theme.accent.l, 0.5)
                } else {
                    theme.accent
                })
                .text_color(white())
                .when(!self.disabled, |this| {
                    this.hover(|this| this.bg(theme.accent_hover))
                        .active(|this| this.bg(theme.accent_active))
                }),
            ButtonVariant::Secondary => button
                .bg(if self.disabled {
                    hsla(theme.surface.h, theme.surface.s, theme.surface.l, 0.5)
                } else {
                    theme.surface
                })
                .text_color(if self.disabled {
                    theme.text_disabled
                } else {
                    theme.text
                })
                .border_1()
                .border_color(if self.disabled {
                    hsla(theme.border.h, theme.border.s, theme.border.l, 0.5)
                } else {
                    theme.border
                })
                .when(!self.disabled, |this| {
                    this.hover(|this| this.bg(theme.surface_hover))
                        .active(|this| this.bg(theme.surface_active))
                }),
            ButtonVariant::Ghost => button
                .bg(hsla(0.0, 0.0, 0.0, 0.0))
                .text_color(if self.disabled {
                    theme.text_disabled
                } else {
                    theme.text
                })
                .when(!self.disabled, |this| {
                    this.hover(|this| this.bg(theme.surface_hover))
                        .active(|this| this.bg(theme.surface_active))
                }),
            ButtonVariant::Danger => button
                .bg(if self.disabled {
                    hsla(theme.error.h, theme.error.s, theme.error.l, 0.5)
                } else {
                    theme.error
                })
                .text_color(white())
                .when(!self.disabled, |this| {
                    this.hover(|this| this.bg(theme.error))
                        .active(|this| this.bg(theme.error))
                }),
        };

        // Handle disabled state
        if self.disabled {
            button = button.cursor_not_allowed();
        } else {
            button = button.cursor_pointer();

            if let Some(on_click) = self.on_click {
                button = button.on_mouse_down(MouseButton::Left, move |ev, _window, cx| {
                    on_click(ev, cx);
                });
            }
        }

        // TODO: Add tooltip support when GPUI API is stable
        // Tooltip implementation removed temporarily

        // Add icon and label
        if let Some(icon) = self.icon {
            match self.icon_position {
                IconPosition::Start => {
                    button = button.child(icon).child(self.label);
                }
                IconPosition::End => {
                    button = button.child(self.label).child(icon);
                }
            }
        } else {
            button = button.child(self.label);
        }

        button
    }
}
