// ABOUTME: Comprehensive tests for the component trait system
// ABOUTME: Ensures trait consistency, integration, and proper behavior

#[cfg(test)]
use crate::{
    Component, ComponentFactory, ComponentState, ComponentStyles, Styled, Theme, Tooltipped,
    ValidationState, compute_component_state,
};
use gpui::{ElementId, SharedString};

// Mock component for testing
#[derive(Debug)]
struct MockComponent {
    id: ElementId,
    disabled: bool,
    variant: MockVariant,
    size: MockSize,
    tooltip: Option<SharedString>,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum MockVariant {
    #[default]
    Primary,
    Secondary,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Default)]
enum MockSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl MockComponent {
    fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            disabled: false,
            variant: MockVariant::default(),
            size: MockSize::default(),
            tooltip: None,
        }
    }
}

// Use the macro to implement Component trait
crate::impl_component!(MockComponent);
crate::impl_tooltipped!(MockComponent);

impl Styled for MockComponent {
    type Variant = MockVariant;
    type Size = MockSize;

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

impl ComponentFactory for MockComponent {
    fn new(id: impl Into<ElementId>) -> Self {
        MockComponent::new(id)
    }
}

#[test]
fn test_component_trait() {
    let mut component = MockComponent::new("test-id");

    // Test that we can access the ID (we can't easily test the value without knowing ElementId internals)
    let _id = component.id();

    component = component.with_id("new-id");
    let _new_id = component.id();

    // Test disabled state
    assert!(!component.is_disabled());

    component = component.disabled(true);
    assert!(component.is_disabled());

    component = component.disabled(false);
    assert!(!component.is_disabled());
}

#[test]
fn test_styled_trait() {
    let component = MockComponent::new("test")
        .with_variant(MockVariant::Secondary)
        .with_size(MockSize::Large);

    assert_eq!(*component.variant(), MockVariant::Secondary);
    assert_eq!(*component.size(), MockSize::Large);

    // Test theme styling
    let theme = Theme::from_tokens(crate::tokens::DesignTokens::dark());
    let styles = component.apply_theme_styling(&theme);

    assert_eq!(styles.background, theme.tokens.chrome.surface);
    assert_eq!(styles.text_color, theme.tokens.chrome.text_on_chrome);
}

#[test]
fn test_tooltipped_trait() {
    let component = MockComponent::new("test").tooltip("Test tooltip");

    assert_eq!(component.get_tooltip().unwrap().as_ref(), "Test tooltip");
}

#[test]
fn test_component_factory() {
    // Test basic creation
    let component = MockComponent::new("factory-test");
    let _id = component.id(); // Just verify we can access the ID
    assert_eq!(*component.variant(), MockVariant::Primary);
    assert_eq!(*component.size(), MockSize::Medium);

    // Test creation with variant using ComponentFactory trait
    let variant_component =
        <MockComponent as ComponentFactory>::with_variant("variant-test", MockVariant::Secondary);
    assert_eq!(*variant_component.variant(), MockVariant::Secondary);

    // Test creation with size using ComponentFactory trait
    let size_component =
        <MockComponent as ComponentFactory>::with_size("size-test", MockSize::Large);
    assert_eq!(*size_component.size(), MockSize::Large);
}

#[test]
fn test_component_styles() {
    let theme = Theme::from_tokens(crate::tokens::DesignTokens::dark());
    let variant = MockVariant::Primary;
    let size = MockSize::Medium;

    let base_styles = ComponentStyles::from_theme(&theme, &variant, &size);

    // Test base styles
    assert_eq!(base_styles.background, theme.tokens.chrome.surface);
    assert_eq!(base_styles.text_color, theme.tokens.chrome.text_on_chrome);
    assert_eq!(base_styles.border_color, theme.tokens.chrome.border_default);
    assert_eq!(base_styles.padding, theme.tokens.sizes.space_3);
    assert_eq!(base_styles.border_radius, theme.tokens.sizes.radius_md);

    // Test state variants
    let hover_styles = base_styles.hover_state(&theme);
    assert_eq!(hover_styles.background, theme.tokens.chrome.surface_hover);
    assert_eq!(hover_styles.text_color, base_styles.text_color);

    let active_styles = base_styles.active_state(&theme);
    assert_eq!(active_styles.background, theme.tokens.chrome.surface_active);

    let disabled_styles = base_styles.disabled_state(&theme);
    assert_eq!(
        disabled_styles.background,
        theme.tokens.chrome.surface_disabled
    );
    assert_eq!(
        disabled_styles.text_color,
        theme.tokens.chrome.text_chrome_disabled
    );
}

#[test]
fn test_component_state() {
    // Test state computation
    assert_eq!(
        compute_component_state(true, false, false, false, false),
        ComponentState::Disabled
    );

    assert_eq!(
        compute_component_state(false, true, false, false, false),
        ComponentState::Loading
    );

    assert_eq!(
        compute_component_state(false, false, false, false, true),
        ComponentState::Active
    );

    assert_eq!(
        compute_component_state(false, false, true, false, false),
        ComponentState::Focused
    );

    assert_eq!(
        compute_component_state(false, false, false, true, false),
        ComponentState::Hover
    );

    assert_eq!(
        compute_component_state(false, false, false, false, false),
        ComponentState::Default
    );

    // Test state properties
    assert!(ComponentState::Hover.is_interactive());
    assert!(ComponentState::Active.is_interactive());
    assert!(ComponentState::Focused.is_interactive());
    assert!(!ComponentState::Default.is_interactive());

    assert!(ComponentState::Disabled.prevents_interaction());
    assert!(ComponentState::Loading.prevents_interaction());
    assert!(!ComponentState::Hover.prevents_interaction());
}

#[test]
fn test_validation_state() {
    let valid = ValidationState::Valid;
    assert!(!valid.is_error());
    assert!(!valid.is_warning());
    assert_eq!(valid.message(), None);

    let warning = ValidationState::Warning("This is a warning".to_string());
    assert!(!warning.is_error());
    assert!(warning.is_warning());
    assert_eq!(warning.message(), Some("This is a warning"));

    let error = ValidationState::Error("This is an error".to_string());
    assert!(error.is_error());
    assert!(!error.is_warning());
    assert_eq!(error.message(), Some("This is an error"));
}
