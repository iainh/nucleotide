// ABOUTME: Button component following Zed's design patterns
// ABOUTME: Provides consistent button styling and behavior

use crate::{
    ComponentFactory, Composable, Interactive, Slotted, StyleSize, StyleState, StyleVariant,
    Styled as UIStyled, compute_component_style, is_feature_enabled, should_enable_animations,
};
use gpui::prelude::FluentBuilder;
use gpui::px;
use gpui::{
    App, ElementId, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div, svg,
};

/// Button variant styles (backward compatibility)
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ButtonVariant {
    #[default]
    Primary,
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
type ButtonClickHandler = Box<dyn Fn(&MouseUpEvent, &mut Window, &mut App) + 'static>;

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
            variant: ButtonVariant::Primary,
            size: ButtonSize::Medium,
            disabled: false,
            loading: false,
            state: ButtonState::Default,
            icon_path: None,
            icon_position: IconPosition::Start,
            on_click: None,
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
        handler: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
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
    type ClickHandler = Box<dyn Fn(&MouseUpEvent, &mut Window, &mut App) + 'static>;

    fn on_click(mut self, handler: Self::ClickHandler) -> Self {
        self.on_click = Some(handler);
        self
    }

    fn on_secondary_click(self, _handler: Self::ClickHandler) -> Self {
        // Button doesn't support secondary click, just return self
        self
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

        // Determine current style state based on component state and properties
        let current_state = if self.disabled {
            StyleState::Disabled
        } else if self.loading {
            StyleState::Loading
        } else {
            self.state.into()
        };

        // Compute style using the new style system
        let computed_style = compute_component_style(
            theme,
            current_state,
            StyleVariant::from(self.variant).as_str(),
            StyleSize::from(self.size).as_str(),
        );

        // Check if animations should be enabled
        let enable_animations = is_feature_enabled(cx, |features| features.enable_animations)
            && should_enable_animations(theme, current_state);

        let mut button = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .py(computed_style.padding_y)
            .px(computed_style.padding_x)
            .rounded(computed_style.border_radius)
            .text_size(computed_style.font_size)
            .font_weight(match computed_style.font_weight {
                400 => FontWeight::NORMAL,
                700 => FontWeight::BOLD,
                _ => FontWeight::MEDIUM,
            })
            .bg(computed_style.background)
            .text_color(computed_style.foreground)
            .border_color(computed_style.border_color)
            .when(computed_style.border_width.0 > 0.0, |el| {
                el.border_1().border_color(computed_style.border_color)
            })
            .when(computed_style.shadow.is_some(), |el| {
                let shadow = computed_style.shadow.as_ref().unwrap();
                el.shadow(vec![gpui::BoxShadow {
                    color: shadow.color,
                    offset: gpui::point(shadow.offset_x, shadow.offset_y),
                    blur_radius: shadow.blur_radius,
                    spread_radius: shadow.spread_radius,
                }])
            })
            .opacity(computed_style.opacity);

        // Apply interactive hover/active states if enabled and not disabled/loading
        if current_state.is_interactive() && enable_animations {
            let hover_style = compute_component_style(
                theme,
                StyleState::Hover,
                StyleVariant::from(self.variant).as_str(),
                StyleSize::from(self.size).as_str(),
            );
            let active_style = compute_component_style(
                theme,
                StyleState::Active,
                StyleVariant::from(self.variant).as_str(),
                StyleSize::from(self.size).as_str(),
            );

            button = button
                .hover(|this| {
                    let mut hovered = this
                        .bg(hover_style.background)
                        .text_color(hover_style.foreground)
                        .border_color(hover_style.border_color);

                    if let Some(shadow) = &hover_style.shadow {
                        hovered = hovered.shadow(vec![gpui::BoxShadow {
                            color: shadow.color,
                            offset: gpui::point(shadow.offset_x, shadow.offset_y),
                            blur_radius: shadow.blur_radius,
                            spread_radius: shadow.spread_radius,
                        }]);
                    }

                    hovered
                })
                .active(|this| {
                    let mut activated = this
                        .bg(active_style.background)
                        .text_color(active_style.foreground)
                        .border_color(active_style.border_color);

                    if let Some(shadow) = &active_style.shadow {
                        activated = activated.shadow(vec![gpui::BoxShadow {
                            color: shadow.color,
                            offset: gpui::point(shadow.offset_x, shadow.offset_y),
                            blur_radius: shadow.blur_radius,
                            spread_radius: shadow.spread_radius,
                        }]);
                    }

                    activated
                });
        }

        // Handle cursor and interaction states
        if self.disabled || self.loading {
            button = button.cursor_not_allowed();
        } else {
            button = button.cursor_pointer();

            if let Some(on_click) = self.on_click {
                button = button.on_mouse_up(MouseButton::Left, move |ev, window, cx| {
                    on_click(ev, window, cx);
                });
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
                    let icon_size = StyleSize::from(self.size).icon_size();
                    let icon_element = svg()
                        .path(icon_path.to_string())
                        .size(icon_size)
                        .text_color(computed_style.foreground)
                        .flex_shrink_0();
                    button.child(icon_element)
                }
            };
        }

        // Fall back to icon and label if no slots are used
        if self.slots.is_empty() {
            if let Some(icon_path) = self.icon_path {
                let icon_size = StyleSize::from(self.size).icon_size();
                let icon_element = svg()
                    .path(icon_path.to_string())
                    .size(icon_size)
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

        button
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
        assert_eq!(button.variant, ButtonVariant::Primary);
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
        let theme = Theme::dark();

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

        // Primary should use primary colors
        assert_eq!(primary_style.background, theme.tokens.colors.primary);
        assert_eq!(
            primary_style.foreground,
            theme.tokens.colors.text_on_primary
        );

        // Secondary should use surface colors and have border
        assert_eq!(secondary_style.background, theme.tokens.colors.surface);
        assert_eq!(secondary_style.foreground, theme.tokens.colors.text_primary);
        assert_eq!(secondary_style.border_width, px(1.0));
    }
}
