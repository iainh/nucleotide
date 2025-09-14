// ABOUTME: Tests for the enhanced Button component
// ABOUTME: Tests new styling system integration, state management, and slot composition

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Theme, StyleState, StyleVariant, StyleSize};
    use gpui::{px, TestApp, TestWindow};

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
        assert_eq!(StyleVariant::from(ButtonVariant::Primary), StyleVariant::Primary);
        assert_eq!(StyleVariant::from(ButtonVariant::Secondary), StyleVariant::Secondary);
        assert_eq!(StyleVariant::from(ButtonVariant::Ghost), StyleVariant::Ghost);
        assert_eq!(StyleVariant::from(ButtonVariant::Danger), StyleVariant::Danger);
        assert_eq!(StyleVariant::from(ButtonVariant::Success), StyleVariant::Success);
        assert_eq!(StyleVariant::from(ButtonVariant::Warning), StyleVariant::Warning);
        assert_eq!(StyleVariant::from(ButtonVariant::Info), StyleVariant::Info);
        
        // Test reverse conversion
        assert_eq!(ButtonVariant::from(StyleVariant::Primary), ButtonVariant::Primary);
        assert_eq!(ButtonVariant::from(StyleVariant::Accent), ButtonVariant::Primary); // Maps to primary
    }

    #[test]
    fn test_button_size_conversions() {
        // Test ButtonSize to StyleSize conversion
        assert_eq!(StyleSize::from(ButtonSize::ExtraSmall), StyleSize::ExtraSmall);
        assert_eq!(StyleSize::from(ButtonSize::Small), StyleSize::Small);
        assert_eq!(StyleSize::from(ButtonSize::Medium), StyleSize::Medium);
        assert_eq!(StyleSize::from(ButtonSize::Large), StyleSize::Large);
        assert_eq!(StyleSize::from(ButtonSize::ExtraLarge), StyleSize::ExtraLarge);
        
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
        assert_eq!(StyleState::from(ButtonState::Disabled), StyleState::Disabled);
    }

    #[test]
    fn test_button_slots() {
        let mut button = Button::new("btn", "Test");
        button = button.add_slot(ButtonSlot::Text("Extra text".into()));
        button = button.add_slot(ButtonSlot::Icon("icons/check.svg".into()));
        
        assert_eq!(button.slots.len(), 2);
        
        match &button.slots[0] {
            ButtonSlot::Text(text) => assert_eq!(text.as_ref(), "Extra text"),
            _ => panic!("Expected text slot"),
        }
        
        match &button.slots[1] {
            ButtonSlot::Icon(path) => assert_eq!(path.as_ref(), "icons/check.svg"),
            _ => panic!("Expected icon slot"),
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
            .tooltip("This is a success button")
            .class("custom-button-class");
            
        assert_eq!(button.variant, ButtonVariant::Success);
        assert_eq!(button.size, ButtonSize::Large);
        assert!(!button.disabled);
        assert!(button.icon_path.is_some());
        assert_eq!(button.icon_position, IconPosition::End);
        assert!(button.tooltip.is_some());
        assert_eq!(button.class_names.len(), 1);
    }

    #[test]
    fn test_button_styled_trait() {
        let button = Button::new("btn", "Test")
            .variant(ButtonVariant::Secondary)
            .size(ButtonSize::Large);
            
        // Test Styled trait methods
        assert_eq!(*button.variant(), ButtonVariant::Secondary);
        assert_eq!(*button.size(), ButtonSize::Large);
        
        let updated = button.with_variant(ButtonVariant::Danger);
        assert_eq!(*updated.variant(), ButtonVariant::Danger);
        
        let resized = updated.with_size(ButtonSize::Small);
        assert_eq!(*resized.size(), ButtonSize::Small);
    }

    #[test]
    fn test_component_factory() {
        let button = Button::new("factory-btn");
        assert_eq!(button.label, "");
        assert_eq!(button.variant, ButtonVariant::Primary);
        assert_eq!(button.size, ButtonSize::Medium);
    }

    #[test]
    fn test_button_size_methods() {
        // Test ButtonSize helper methods
        assert_eq!(ButtonSize::Small.padding(), (px(2.), px(4.)));
        assert_eq!(ButtonSize::Medium.padding(), (px(4.), px(8.)));
        assert_eq!(ButtonSize::Large.padding(), (px(8.), px(12.)));
        
        assert_eq!(ButtonSize::Small.text_size(), px(12.));
        assert_eq!(ButtonSize::Medium.text_size(), px(14.));
        assert_eq!(ButtonSize::Large.text_size(), px(16.));
    }

    #[test]
    fn test_button_state_determination() {
        // Test that loading state overrides normal state
        let loading_button = Button::new("btn", "Test").loading(true);
        assert_eq!(loading_button.state, ButtonState::Loading);
        
        // Test manual state setting
        let focused_button = Button::new("btn", "Test").state(ButtonState::Focused);
        assert_eq!(focused_button.state, ButtonState::Focused);
    }

    #[test]
    fn test_style_integration() {
        let theme = Theme::from_tokens(crate::tokens::DesignTokens::dark());
        
        // Test style computation for different variants
        let primary_style = crate::compute_component_style(
            &theme,
            StyleState::Default,
            StyleVariant::Primary.as_str(),
            StyleSize::Medium.as_str()
        );
        
        let secondary_style = crate::compute_component_style(
            &theme,
            StyleState::Default,
            StyleVariant::Secondary.as_str(),
            StyleSize::Medium.as_str()
        );
        
        // Primary should use primary colors
        assert_eq!(primary_style.background, theme.tokens.chrome.primary);
        let contrast = crate::styling::ColorTheory::contrast_ratio(
            primary_style.background,
            primary_style.foreground,
        );
        assert!(contrast >= crate::styling::ContrastRatios::AA_NORMAL);
        
        // Secondary should use surface colors and have border
        assert_eq!(secondary_style.background, theme.tokens.chrome.surface);
        let sec_contrast = crate::styling::ColorTheory::contrast_ratio(
            secondary_style.background,
            secondary_style.foreground,
        );
        assert!(sec_contrast >= crate::styling::ContrastRatios::AA_NORMAL);
        assert_eq!(secondary_style.border_width, px(1.0));
    }
}
