// ABOUTME: Button component following Zed's design patterns
// ABOUTME: Provides consistent button styling and behavior

use crate::{
    ComponentFactory, Composable, Interactive, Slotted, StyleSize, StyleState, StyleVariant,
    Styled as UIStyled, ThemedContext, Tooltipped,
    styling::{ComputedStyle, TimingFunction, Transition, TransitionProperty},
    tokens::{ButtonTokens, DesignTokens},
};
use gpui::prelude::FluentBuilder;
use gpui::px;
use gpui::{
    App, AppContext, ClickEvent, Context, ElementId, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, relative, svg,
};
use std::time::Duration;

fn button_shadow_stack(
    shadow: &crate::styling::BoxShadow,
    inset_highlight: crate::tokens::ShadowToken,
    inset_shadow: crate::tokens::ShadowToken,
    pressed: bool,
) -> Vec<gpui::BoxShadow> {
    let mut shadows = vec![gpui::BoxShadow {
        color: shadow.color,
        offset: gpui::point(shadow.offset_x, shadow.offset_y),
        blur_radius: shadow.blur_radius,
        spread_radius: shadow.spread_radius,
        inset: false,
    }];

    shadows.push(if pressed {
        inset_shadow.to_box_shadow(true)
    } else {
        inset_highlight.to_box_shadow(true)
    });

    shadows
}

#[derive(Clone, Copy)]
struct ButtonMetrics {
    height: Pixels,
    padding_x: Pixels,
    padding_y: Pixels,
    border_radius: Pixels,
    font_size: Pixels,
    icon_size: Pixels,
    gap: Pixels,
}

fn button_metrics(size: ButtonSize, tokens: &DesignTokens) -> ButtonMetrics {
    match size {
        ButtonSize::ExtraSmall => ButtonMetrics {
            height: tokens.sizes.space_6,
            padding_x: tokens.sizes.space_2,
            padding_y: tokens.sizes.space_0,
            border_radius: tokens.sizes.radius_sm,
            font_size: tokens.sizes.text_xs,
            icon_size: tokens.sizes.text_sm,
            gap: tokens.sizes.space_1,
        },
        ButtonSize::Small => ButtonMetrics {
            height: tokens.sizes.button_height_sm,
            padding_x: tokens.sizes.space_4,
            padding_y: tokens.sizes.space_0,
            border_radius: tokens.sizes.radius_md,
            font_size: tokens.sizes.text_sm,
            icon_size: tokens.sizes.text_md,
            gap: tokens.sizes.space_1,
        },
        ButtonSize::Medium => ButtonMetrics {
            height: tokens.sizes.button_height_md,
            padding_x: tokens.sizes.space_5,
            padding_y: tokens.sizes.space_0,
            border_radius: tokens.sizes.radius_md,
            font_size: tokens.sizes.text_md,
            icon_size: tokens.sizes.text_lg,
            gap: tokens.sizes.space_2,
        },
        ButtonSize::Large => ButtonMetrics {
            height: tokens.sizes.button_height_md,
            padding_x: tokens.sizes.space_5,
            padding_y: tokens.sizes.space_0,
            border_radius: tokens.sizes.radius_md,
            font_size: tokens.sizes.text_md,
            icon_size: tokens.sizes.text_xl,
            gap: tokens.sizes.space_2,
        },
        ButtonSize::ExtraLarge => ButtonMetrics {
            height: tokens.sizes.button_height_lg,
            padding_x: tokens.sizes.space_6,
            padding_y: tokens.sizes.space_0,
            border_radius: tokens.sizes.radius_lg,
            font_size: tokens.sizes.text_lg,
            icon_size: tokens.sizes.space_6,
            gap: tokens.sizes.space_2,
        },
    }
}

/// Button variant styles (backward compatibility)
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ButtonVariant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
    Success,
    Warning,
    Info,
}

impl From<ButtonVariant> for StyleVariant {
    fn from(variant: ButtonVariant) -> Self {
        match variant {
            ButtonVariant::Primary => StyleVariant::Primary,
            ButtonVariant::Secondary => StyleVariant::Secondary,
            ButtonVariant::Ghost => StyleVariant::Ghost,
            ButtonVariant::Danger => StyleVariant::Danger,
            ButtonVariant::Success => StyleVariant::Success,
            ButtonVariant::Warning => StyleVariant::Warning,
            ButtonVariant::Info => StyleVariant::Info,
        }
    }
}

impl From<StyleVariant> for ButtonVariant {
    fn from(variant: StyleVariant) -> Self {
        match variant {
            StyleVariant::Primary => ButtonVariant::Primary,
            StyleVariant::Secondary => ButtonVariant::Secondary,
            StyleVariant::Ghost => ButtonVariant::Ghost,
            StyleVariant::Danger => ButtonVariant::Danger,
            StyleVariant::Success => ButtonVariant::Success,
            StyleVariant::Warning => ButtonVariant::Warning,
            StyleVariant::Info => ButtonVariant::Info,
            StyleVariant::Accent => ButtonVariant::Primary, // Map accent to primary
        }
    }
}

/// Button sizes (backward compatibility)
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ButtonSize {
    ExtraSmall,
    Small,
    #[default]
    Medium,
    Large,
    ExtraLarge,
}

impl From<ButtonSize> for StyleSize {
    fn from(size: ButtonSize) -> Self {
        match size {
            ButtonSize::ExtraSmall => StyleSize::ExtraSmall,
            ButtonSize::Small => StyleSize::Small,
            ButtonSize::Medium => StyleSize::Medium,
            ButtonSize::Large => StyleSize::Large,
            ButtonSize::ExtraLarge => StyleSize::ExtraLarge,
        }
    }
}

impl From<StyleSize> for ButtonSize {
    fn from(size: StyleSize) -> Self {
        match size {
            StyleSize::ExtraSmall => ButtonSize::ExtraSmall,
            StyleSize::Small => ButtonSize::Small,
            StyleSize::Medium => ButtonSize::Medium,
            StyleSize::Large => ButtonSize::Large,
            StyleSize::ExtraLarge => ButtonSize::ExtraLarge,
        }
    }
}

impl ButtonSize {}

/// Button interaction states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Default,
    Hover,
    Active,
    Focused,
    Loading,
    Disabled,
}

impl From<ButtonState> for StyleState {
    fn from(state: ButtonState) -> Self {
        match state {
            ButtonState::Default => StyleState::Default,
            ButtonState::Hover => StyleState::Hover,
            ButtonState::Active => StyleState::Active,
            ButtonState::Focused => StyleState::Focused,
            ButtonState::Loading => StyleState::Loading,
            ButtonState::Disabled => StyleState::Disabled,
        }
    }
}

/// Button content slot types
#[derive(Clone)]
pub enum ButtonSlot {
    Text(SharedString),
    Icon(SharedString),
}

// Type alias for button click handler
type ButtonClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

struct ButtonTooltip {
    text: SharedString,
}

impl Render for ButtonTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.theme().tokens;
        let tooltip_tokens = tokens.tooltip_tokens();

        div()
            .max_w(px(420.0))
            .px(tokens.sizes.space_2)
            .py(tokens.sizes.space_1)
            .rounded(tokens.sizes.radius_sm)
            .border_1()
            .border_color(tooltip_tokens.border)
            .bg(tooltip_tokens.background)
            .shadow(vec![tokens.chrome.shadow_md.to_box_shadow(false)])
            .text_size(tokens.sizes.text_sm)
            .text_color(tooltip_tokens.text)
            .child(self.text.clone())
    }
}

/// A reusable button component
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    loading: bool,
    state: ButtonState,
    icon_path: Option<SharedString>,
    icon_position: IconPosition,
    on_click: Option<ButtonClickHandler>,
    tooltip: Option<SharedString>,
    activate_on_mouse_down: bool,
    focus_handle: Option<FocusHandle>,
    slots: Vec<ButtonSlot>,
    class_names: Vec<SharedString>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconPosition {
    Start,
    End,
}

// Implement core traits using macros
crate::impl_component!(Button);

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: ButtonVariant::Secondary,
            size: ButtonSize::Medium,
            disabled: false,
            loading: false,
            state: ButtonState::Default,
            icon_path: None,
            icon_position: IconPosition::Start,
            on_click: None,
            tooltip: None,
            activate_on_mouse_down: false,
            focus_handle: None,
            slots: Vec::new(),
            class_names: Vec::new(),
        }
    }

    /// Create an icon-only button (no text label)
    pub fn icon_only(id: impl Into<ElementId>, icon_path: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: SharedString::default(),
            variant: ButtonVariant::Ghost,
            size: ButtonSize::Small,
            disabled: false,
            loading: false,
            state: ButtonState::Default,
            icon_path: Some(icon_path.into()),
            icon_position: IconPosition::Start,
            on_click: None,
            tooltip: None,
            activate_on_mouse_down: false,
            focus_handle: None,
            slots: Vec::new(),
            class_names: Vec::new(),
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

    /// Add an SVG icon by path
    pub fn icon(mut self, icon_path: impl Into<SharedString>) -> Self {
        self.icon_path = Some(icon_path.into());
        self
    }

    /// Set icon position
    pub fn icon_position(mut self, position: IconPosition) -> Self {
        self.icon_position = position;
        self
    }

    /// Set click handler
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// Set loading state
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        if loading {
            self.state = ButtonState::Loading;
        }
        self
    }

    /// Set button state manually
    pub fn state(mut self, state: ButtonState) -> Self {
        self.state = state;
        self
    }

    /// Add a content slot to the button
    pub fn add_slot(mut self, slot: ButtonSlot) -> Self {
        self.slots.push(slot);
        self
    }

    /// Add a CSS class name
    pub fn class(mut self, class_name: impl Into<SharedString>) -> Self {
        self.class_names.push(class_name.into());
        self
    }

    pub fn activate_on_mouse_down(mut self) -> Self {
        self.activate_on_mouse_down = true;
        self
    }

    /// Include this button in GPUI focus traversal with a caller-owned handle.
    pub fn focus_handle(mut self, focus_handle: FocusHandle) -> Self {
        self.focus_handle = Some(focus_handle);
        self
    }

    // Add new trait-based methods with different names to avoid conflicts

    /// Set button variant (trait-based API)
    pub fn with_variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set button size (trait-based API)
    pub fn with_size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Convert ButtonTokens to ComputedStyle based on variant and state
    fn compute_style_from_tokens(
        &self,
        tokens: &ButtonTokens,
        state: StyleState,
        design_tokens: &DesignTokens,
    ) -> ComputedStyle {
        let (background, text, border) = match (&self.variant, state) {
            // Primary button states
            (ButtonVariant::Primary, StyleState::Default) => (
                tokens.primary_background,
                tokens.primary_text,
                tokens.primary_border,
            ),
            (ButtonVariant::Primary, StyleState::Hover) => (
                tokens.primary_background_hover,
                tokens.primary_text,
                tokens.primary_border,
            ),
            (ButtonVariant::Primary, StyleState::Active) => (
                tokens.primary_background_active,
                tokens.primary_text,
                tokens.primary_border,
            ),

            // Secondary button states
            (ButtonVariant::Secondary, StyleState::Default) => (
                tokens.secondary_background,
                tokens.secondary_text,
                tokens.secondary_border,
            ),
            (ButtonVariant::Secondary, StyleState::Hover) => (
                tokens.secondary_background_hover,
                tokens.secondary_text,
                tokens.secondary_border,
            ),
            (ButtonVariant::Secondary, StyleState::Active) => (
                tokens.secondary_background_active,
                tokens.secondary_text,
                tokens.secondary_border,
            ),

            // Ghost button states
            (ButtonVariant::Ghost, StyleState::Default) => (
                tokens.ghost_background,
                tokens.ghost_text,
                tokens.ghost_background,
            ),
            (ButtonVariant::Ghost, StyleState::Hover) => (
                tokens.ghost_background_hover,
                tokens.ghost_text,
                tokens.ghost_background,
            ),
            (ButtonVariant::Ghost, StyleState::Active) => (
                tokens.ghost_background_active,
                tokens.ghost_text,
                tokens.ghost_background,
            ),

            // Semantic button states (use Helix colors)
            (ButtonVariant::Danger, StyleState::Default) => (
                tokens.danger_background,
                tokens.danger_text,
                tokens.danger_background,
            ),
            (ButtonVariant::Danger, StyleState::Hover) => (
                tokens.danger_background_hover,
                tokens.danger_text,
                tokens.danger_background,
            ),
            (ButtonVariant::Danger, StyleState::Active) => (
                tokens.danger_background_active,
                tokens.danger_text,
                tokens.danger_background,
            ),

            (ButtonVariant::Success, StyleState::Default) => (
                tokens.success_background,
                tokens.success_text,
                tokens.success_background,
            ),
            (ButtonVariant::Success, StyleState::Hover) => (
                tokens.success_background_hover,
                tokens.success_text,
                tokens.success_background,
            ),
            (ButtonVariant::Success, StyleState::Active) => (
                tokens.success_background_active,
                tokens.success_text,
                tokens.success_background,
            ),

            (ButtonVariant::Warning, StyleState::Default) => (
                tokens.warning_background,
                tokens.warning_text,
                tokens.warning_background,
            ),
            (ButtonVariant::Warning, StyleState::Hover) => (
                tokens.warning_background_hover,
                tokens.warning_text,
                tokens.warning_background,
            ),
            (ButtonVariant::Warning, StyleState::Active) => (
                tokens.warning_background_active,
                tokens.warning_text,
                tokens.warning_background,
            ),

            (ButtonVariant::Info, StyleState::Default) => (
                tokens.info_background,
                tokens.info_text,
                tokens.primary_border,
            ),
            (ButtonVariant::Info, StyleState::Hover) => (
                tokens.info_background_hover,
                tokens.info_text,
                tokens.primary_border,
            ),
            (ButtonVariant::Info, StyleState::Active) => (
                tokens.info_background_active,
                tokens.info_text,
                tokens.primary_border,
            ),

            // Disabled states
            (_, StyleState::Disabled) => (
                tokens.disabled_background,
                tokens.disabled_text,
                tokens.disabled_border,
            ),
            (_, StyleState::Loading) => (
                tokens.disabled_background,
                tokens.disabled_text,
                tokens.disabled_border,
            ),

            // Focus states use focus ring but maintain background
            (variant, StyleState::Focused) => {
                let (bg, text, _) = match variant {
                    ButtonVariant::Primary => (
                        tokens.primary_background,
                        tokens.primary_text,
                        tokens.primary_border,
                    ),
                    ButtonVariant::Secondary => (
                        tokens.secondary_background,
                        tokens.secondary_text,
                        tokens.secondary_border,
                    ),
                    ButtonVariant::Ghost => (
                        tokens.ghost_background,
                        tokens.ghost_text,
                        tokens.ghost_background,
                    ),
                    ButtonVariant::Danger => (
                        tokens.danger_background,
                        tokens.danger_text,
                        tokens.danger_background,
                    ),
                    ButtonVariant::Success => (
                        tokens.success_background,
                        tokens.success_text,
                        tokens.success_background,
                    ),
                    ButtonVariant::Warning => (
                        tokens.warning_background,
                        tokens.warning_text,
                        tokens.warning_background,
                    ),
                    ButtonVariant::Info => (
                        tokens.info_background,
                        tokens.info_text,
                        tokens.primary_border,
                    ),
                };
                (bg, text, tokens.focus_ring)
            }

            // Default fallback
            _ => (
                tokens.primary_background,
                tokens.primary_text,
                tokens.primary_border,
            ),
        };

        let metrics = button_metrics(self.size, design_tokens);

        ComputedStyle {
            background,
            foreground: text,
            border_color: border,
            border_width: if matches!(self.variant, ButtonVariant::Secondary) {
                px(1.0)
            } else {
                px(0.0)
            },
            border_radius: metrics.border_radius,
            padding_x: metrics.padding_x,
            padding_y: metrics.padding_y,
            font_size: metrics.font_size,
            font_weight: 500, // Medium weight for better readability
            opacity: if matches!(state, StyleState::Disabled | StyleState::Loading) {
                0.6
            } else {
                1.0
            },
            shadow: if matches!(self.variant, ButtonVariant::Ghost) {
                None // Ghost buttons should have no shadow for subtlety
            } else {
                Some(crate::styling::BoxShadow {
                    offset_x: px(tokens.shadow_offset_x),
                    offset_y: px(tokens.shadow_offset_y),
                    blur_radius: px(tokens.shadow_blur_radius),
                    spread_radius: px(0.0), // No spread for subtle shadows
                    color: tokens.shadow_color,
                })
            },
            transition: if state.is_interactive() {
                Some(Transition {
                    duration: Duration::from_millis(150),
                    timing_function: TimingFunction::EaseOut,
                    properties: vec![
                        TransitionProperty::Background,
                        TransitionProperty::BorderColor,
                        TransitionProperty::Transform,
                    ],
                })
            } else {
                None
            },
        }
    }
}

// Implement the Styled trait
impl UIStyled for Button {
    type Variant = ButtonVariant;
    type Size = ButtonSize;

    fn variant(&self) -> &Self::Variant {
        &self.variant
    }

    fn with_variant(mut self, variant: Self::Variant) -> Self {
        self.variant = variant;
        self
    }

    fn size(&self) -> &Self::Size {
        &self.size
    }

    fn with_size(mut self, size: Self::Size) -> Self {
        self.size = size;
        self
    }
}

// Implement the Interactive trait
impl Interactive for Button {
    type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

    fn on_click(mut self, handler: Self::ClickHandler) -> Self {
        self.on_click = Some(handler);
        self
    }

    fn on_secondary_click(self, _handler: Self::ClickHandler) -> Self {
        // Button doesn't support secondary click, just return self
        self
    }
}

impl Tooltipped for Button {
    fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    fn get_tooltip(&self) -> Option<&SharedString> {
        self.tooltip.as_ref()
    }
}

// Implement ComponentFactory trait
impl ComponentFactory for Button {
    fn new(id: impl Into<ElementId>) -> Self {
        Button::new(id, "")
    }
}

// Implement Composable trait
impl Composable for Button {
    fn child(self, _child: impl IntoElement) -> Self {
        // For buttons, child elements would be handled through slots instead
        // This is a simplified implementation
        self
    }

    fn children(self, _children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        // For buttons, child elements would be handled through slots instead
        // This is a simplified implementation
        self
    }
}

// Implement Slotted trait
impl Slotted for Button {
    fn start_slot(self, _slot: impl IntoElement) -> Self {
        // Convert the element to a slot - simplified for now
        // In a full implementation, this would need better conversion
        self
    }

    fn end_slot(self, _slot: impl IntoElement) -> Self {
        // Convert the element to a slot - simplified for now
        // In a full implementation, this would need better conversion
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<crate::Theme>();

        // Get button tokens using hybrid color system
        let button_tokens = theme.tokens.button_tokens();

        // Determine current style state based on component state and properties
        let current_state = if self.disabled {
            StyleState::Disabled
        } else if self.loading {
            StyleState::Loading
        } else {
            self.state.into()
        };

        // Create computed style from button tokens based on variant and state
        let computed_style =
            self.compute_style_from_tokens(&button_tokens, current_state, &theme.tokens);

        // Precompute hover style for interactive states
        let hover_style =
            self.compute_style_from_tokens(&button_tokens, StyleState::Hover, &theme.tokens);
        let active_style =
            self.compute_style_from_tokens(&button_tokens, StyleState::Active, &theme.tokens);
        let metrics = button_metrics(self.size, &theme.tokens);
        let icon_only = self.label.is_empty() && self.slots.is_empty() && self.icon_path.is_some();
        let inset_highlight = theme.tokens.chrome.inset_highlight;
        let inset_shadow = theme.tokens.chrome.inset_shadow;
        let activate_on_mouse_down = self.activate_on_mouse_down;
        let focus_handle = self.focus_handle.clone();
        // Mouse-down actions commonly repaint before mouse-up; GPUI's active
        // pseudo-state can otherwise read as latched after the action runs.
        let apply_active_style = !activate_on_mouse_down;

        let mut button = div()
            .id(self.id)
            .flex()
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(metrics.gap)
            .h(metrics.height)
            .min_w(metrics.height)
            .when(icon_only, |button| {
                button
                    .w(metrics.height)
                    .px(theme.tokens.sizes.space_0)
                    .py(theme.tokens.sizes.space_0)
            })
            .when(!icon_only, |button| {
                button
                    .py(computed_style.padding_y)
                    .px(computed_style.padding_x)
            })
            .rounded(computed_style.border_radius)
            .text_size(computed_style.font_size)
            .line_height(relative(1.0))
            .font_weight(match computed_style.font_weight {
                400 => FontWeight::NORMAL,
                700 => FontWeight::BOLD,
                _ => FontWeight::MEDIUM,
            })
            .bg(computed_style.background)
            .text_color(computed_style.foreground)
            .border_color(computed_style.border_color)
            .when(f32::from(computed_style.border_width) > 0.0, |el| {
                el.border_1().border_color(computed_style.border_color)
            })
            .when(computed_style.shadow.is_some(), |el| {
                let shadow = computed_style.shadow.as_ref().unwrap();
                el.shadow(button_shadow_stack(
                    shadow,
                    inset_highlight,
                    inset_shadow,
                    false,
                ))
            })
            .opacity(computed_style.opacity);

        if let Some(focus_handle) = focus_handle.as_ref() {
            button = button
                .track_focus(focus_handle)
                .tab_stop(!self.disabled && !self.loading)
                .focus_visible(|style| style.border_color(theme.tokens.chrome.border_focus));
        }

        // Match gpui-component's interactive treatment: hover lifts the button,
        // active swaps to the pressed inset shadow.
        if current_state.is_interactive() {
            button = button.hover(|this| {
                let mut hovered = this
                    .bg(hover_style.background)
                    .text_color(hover_style.foreground)
                    .border_color(hover_style.border_color)
                    .text_size(computed_style.font_size)
                    .font_weight(match computed_style.font_weight {
                        400 => FontWeight::NORMAL,
                        700 => FontWeight::BOLD,
                        _ => FontWeight::MEDIUM,
                    });

                if let Some(shadow) = &hover_style.shadow {
                    hovered = hovered.shadow(button_shadow_stack(
                        shadow,
                        inset_highlight,
                        inset_shadow,
                        false,
                    ));
                }

                hovered
            });

            if apply_active_style {
                button = button.active(|this| {
                    let mut active = this
                        .bg(active_style.background)
                        .text_color(active_style.foreground)
                        .border_color(active_style.border_color)
                        .text_size(computed_style.font_size)
                        .font_weight(match computed_style.font_weight {
                            400 => FontWeight::NORMAL,
                            700 => FontWeight::BOLD,
                            _ => FontWeight::MEDIUM,
                        });

                    if let Some(shadow) = &active_style.shadow {
                        active = active.shadow(button_shadow_stack(
                            shadow,
                            inset_highlight,
                            inset_shadow,
                            true,
                        ));
                    }

                    active
                });
            }
        }

        // Handle cursor and interaction states
        if self.disabled || self.loading {
            button = button.cursor_not_allowed();
        } else {
            button = button.cursor_pointer();

            if let Some(on_click) = self.on_click {
                if activate_on_mouse_down {
                    button = button.on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                        let click_event = crate::click_event_from_mouse_down(ev);
                        window.prevent_default();
                        cx.stop_propagation();
                        on_click(&click_event, window, cx);
                    });
                } else {
                    button = button
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(move |ev, window, cx| {
                            cx.stop_propagation();
                            if !ev.standard_click() {
                                return;
                            }
                            on_click(ev, window, cx);
                        });
                }
            }
        }

        // Add loading spinner if in loading state
        if self.loading {
            // Simple loading indicator - could be enhanced with actual spinner
            button = button.child("⟳").child(" ");
        }

        // Render content slots first (most flexible)
        for slot in self.slots.iter() {
            button = match slot {
                ButtonSlot::Text(text) => button.child(text.clone()),
                ButtonSlot::Icon(icon_path) => {
                    let icon_element = svg()
                        .path(icon_path.to_string())
                        .size(metrics.icon_size)
                        .text_color(computed_style.foreground)
                        .flex_shrink_0();
                    button.child(icon_element)
                }
            };
        }

        // Fall back to icon and label if no slots are used
        if self.slots.is_empty() {
            if let Some(icon_path) = self.icon_path {
                let icon_element = svg()
                    .path(icon_path.to_string())
                    .size(metrics.icon_size)
                    .text_color(computed_style.foreground)
                    .flex_shrink_0();

                if self.label.is_empty() {
                    // Icon-only button
                    button = button.child(icon_element);
                } else {
                    // Button with icon and label
                    match self.icon_position {
                        IconPosition::Start => {
                            button = button.child(icon_element).child(self.label);
                        }
                        IconPosition::End => {
                            button = button.child(self.label).child(icon_element);
                        }
                    }
                }
            } else if !self.label.is_empty() {
                // Text-only button
                button = button.child(self.label);
            }
        }

        button.when_some(self.tooltip, |button, tooltip| {
            button.tooltip(move |_window, cx| {
                cx.new(|_| ButtonTooltip {
                    text: tooltip.clone(),
                })
                .into()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StyleSize, StyleState, StyleVariant, Theme};
    use gpui::px;

    #[test]
    fn test_button_creation() {
        let button = Button::new("test-button", "Click me");
        assert_eq!(button.label, "Click me");
        assert_eq!(button.variant, ButtonVariant::Secondary);
        assert_eq!(button.size, ButtonSize::Medium);
        assert!(!button.disabled);
        assert!(!button.loading);
        assert_eq!(button.state, ButtonState::Default);
    }

    #[test]
    fn test_button_icon_only() {
        let button = Button::icon_only("icon-button", "icons/star.svg");
        assert!(button.label.is_empty());
        assert_eq!(button.variant, ButtonVariant::Ghost);
        assert_eq!(button.size, ButtonSize::Small);
        assert!(button.icon_path.is_some());
    }

    #[test]
    fn test_button_variants() {
        let primary = Button::new("btn", "Test").variant(ButtonVariant::Primary);
        let secondary = Button::new("btn", "Test").variant(ButtonVariant::Secondary);
        let danger = Button::new("btn", "Test").variant(ButtonVariant::Danger);

        assert_eq!(primary.variant, ButtonVariant::Primary);
        assert_eq!(secondary.variant, ButtonVariant::Secondary);
        assert_eq!(danger.variant, ButtonVariant::Danger);
    }

    #[test]
    fn test_button_sizes() {
        let small = Button::new("btn", "Test").size(ButtonSize::Small);
        let medium = Button::new("btn", "Test").size(ButtonSize::Medium);
        let large = Button::new("btn", "Test").size(ButtonSize::Large);

        assert_eq!(small.size, ButtonSize::Small);
        assert_eq!(medium.size, ButtonSize::Medium);
        assert_eq!(large.size, ButtonSize::Large);
    }

    #[test]
    fn test_button_states() {
        let disabled = Button::new("btn", "Test").disabled(true);
        let loading = Button::new("btn", "Test").loading(true);
        let focused = Button::new("btn", "Test").state(ButtonState::Focused);

        assert!(disabled.disabled);
        assert!(loading.loading);
        assert_eq!(loading.state, ButtonState::Loading);
        assert_eq!(focused.state, ButtonState::Focused);
    }

    #[test]
    fn test_button_variant_conversions() {
        // Test ButtonVariant to StyleVariant conversion
        assert_eq!(
            StyleVariant::from(ButtonVariant::Primary),
            StyleVariant::Primary
        );
        assert_eq!(
            StyleVariant::from(ButtonVariant::Secondary),
            StyleVariant::Secondary
        );
        assert_eq!(
            StyleVariant::from(ButtonVariant::Ghost),
            StyleVariant::Ghost
        );
        assert_eq!(
            StyleVariant::from(ButtonVariant::Danger),
            StyleVariant::Danger
        );
        assert_eq!(
            StyleVariant::from(ButtonVariant::Success),
            StyleVariant::Success
        );
        assert_eq!(
            StyleVariant::from(ButtonVariant::Warning),
            StyleVariant::Warning
        );
        assert_eq!(StyleVariant::from(ButtonVariant::Info), StyleVariant::Info);

        // Test reverse conversion
        assert_eq!(
            ButtonVariant::from(StyleVariant::Primary),
            ButtonVariant::Primary
        );
        assert_eq!(
            ButtonVariant::from(StyleVariant::Accent),
            ButtonVariant::Primary
        ); // Maps to primary
    }

    #[test]
    fn test_button_size_conversions() {
        // Test ButtonSize to StyleSize conversion
        assert_eq!(
            StyleSize::from(ButtonSize::ExtraSmall),
            StyleSize::ExtraSmall
        );
        assert_eq!(StyleSize::from(ButtonSize::Small), StyleSize::Small);
        assert_eq!(StyleSize::from(ButtonSize::Medium), StyleSize::Medium);
        assert_eq!(StyleSize::from(ButtonSize::Large), StyleSize::Large);
        assert_eq!(
            StyleSize::from(ButtonSize::ExtraLarge),
            StyleSize::ExtraLarge
        );

        // Test reverse conversion
        assert_eq!(ButtonSize::from(StyleSize::Small), ButtonSize::Small);
        assert_eq!(ButtonSize::from(StyleSize::Medium), ButtonSize::Medium);
    }

    #[test]
    fn test_button_metrics_follow_compact_gpui_component_scale() {
        let tokens = crate::tokens::DesignTokens::light();

        let xs = button_metrics(ButtonSize::ExtraSmall, &tokens);
        let sm = button_metrics(ButtonSize::Small, &tokens);
        let md = button_metrics(ButtonSize::Medium, &tokens);
        let lg = button_metrics(ButtonSize::Large, &tokens);

        assert_eq!(xs.height, tokens.sizes.space_6);
        assert_eq!(sm.height, tokens.sizes.button_height_sm);
        assert_eq!(md.height, tokens.sizes.button_height_md);
        assert_eq!(lg.height, tokens.sizes.button_height_md);
        assert_eq!(xs.icon_size, tokens.sizes.text_sm);
        assert_eq!(sm.icon_size, tokens.sizes.text_md);
        assert_eq!(md.icon_size, tokens.sizes.text_lg);
        assert_eq!(lg.icon_size, tokens.sizes.text_xl);
        assert!(sm.padding_x > xs.padding_x);
        assert!(md.padding_x > sm.padding_x);
    }

    #[test]
    fn test_button_state_conversions() {
        // Test ButtonState to StyleState conversion
        assert_eq!(StyleState::from(ButtonState::Default), StyleState::Default);
        assert_eq!(StyleState::from(ButtonState::Hover), StyleState::Hover);
        assert_eq!(StyleState::from(ButtonState::Active), StyleState::Active);
        assert_eq!(StyleState::from(ButtonState::Focused), StyleState::Focused);
        assert_eq!(StyleState::from(ButtonState::Loading), StyleState::Loading);
        assert_eq!(
            StyleState::from(ButtonState::Disabled),
            StyleState::Disabled
        );
    }

    #[test]
    fn test_button_slots() {
        let mut button = Button::new("btn", "Test");
        button = button.add_slot(ButtonSlot::Text("Extra text".into()));
        button = button.add_slot(ButtonSlot::Icon("icons/check.svg".into()));

        assert_eq!(button.slots.len(), 2);

        match &button.slots[0] {
            ButtonSlot::Text(text) => assert_eq!(text.as_ref(), "Extra text"),
            ButtonSlot::Icon(_) => panic!("Expected text slot"),
        }

        match &button.slots[1] {
            ButtonSlot::Icon(path) => assert_eq!(path.as_ref(), "icons/check.svg"),
            ButtonSlot::Text(_) => panic!("Expected icon slot"),
        }
    }

    #[test]
    fn test_button_builder_pattern() {
        let button = Button::new("complex-btn", "Complex Button")
            .variant(ButtonVariant::Success)
            .size(ButtonSize::Large)
            .disabled(false)
            .icon("icons/success.svg")
            .icon_position(IconPosition::End)
            .class("custom-button-class");

        assert_eq!(button.variant, ButtonVariant::Success);
        assert_eq!(button.size, ButtonSize::Large);
        assert!(!button.disabled);
        assert!(button.icon_path.is_some());
        assert_eq!(button.icon_position, IconPosition::End);
        assert_eq!(button.class_names.len(), 1);
    }

    #[test]
    fn test_style_integration() {
        let theme = Theme::from_tokens(crate::tokens::DesignTokens::dark());

        // Test style computation for different variants
        let primary_style = crate::compute_component_style(
            &theme,
            StyleState::Default,
            StyleVariant::Primary.as_str(),
            StyleSize::Medium.as_str(),
        );

        let secondary_style = crate::compute_component_style(
            &theme,
            StyleState::Default,
            StyleVariant::Secondary.as_str(),
            StyleSize::Medium.as_str(),
        );

        // Primary should use primary background and a high-contrast foreground
        assert_eq!(primary_style.background, theme.tokens.chrome.primary);
        let contrast = crate::styling::ColorTheory::contrast_ratio(
            primary_style.background,
            primary_style.foreground,
        );
        assert!(contrast >= crate::styling::ContrastRatios::AA_NORMAL);

        // Secondary should use surface colors and have border; foreground must be readable
        assert_eq!(secondary_style.background, theme.tokens.chrome.surface);
        let sec_contrast = crate::styling::ColorTheory::contrast_ratio(
            secondary_style.background,
            secondary_style.foreground,
        );
        assert!(sec_contrast >= crate::styling::ContrastRatios::AA_NORMAL);
        assert_eq!(secondary_style.border_width, px(1.0));
    }
}
