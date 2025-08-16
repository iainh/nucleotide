// ABOUTME: Integration tests for trait implementations on real components
// ABOUTME: Ensures Button and ListItem work correctly with the trait system

#[cfg(test)]
mod tests {
    use crate::{
        Button, ButtonSize, ButtonVariant, Component, ComponentFactory, Interactive, ListItem,
        ListItemSpacing, Styled as UIStyled,
    };

    #[test]
    fn test_button_trait_integration() {
        // Test that Button implements all expected traits
        let button = Button::new("test-btn", "Click me")
            .with_variant(ButtonVariant::Secondary)
            .with_size(ButtonSize::Large)
            .disabled(true);

        // Test Component trait
        let _id = button.id();
        assert!(button.is_disabled());

        // Test Styled trait
        assert_eq!(*UIStyled::variant(&button), ButtonVariant::Secondary);
        assert_eq!(*UIStyled::size(&button), ButtonSize::Large);

        // Test that we can create using ComponentFactory
        let factory_button = <Button as ComponentFactory>::new("factory-btn");
        assert_eq!(*UIStyled::variant(&factory_button), ButtonVariant::Primary); // Default
        assert_eq!(*UIStyled::size(&factory_button), ButtonSize::Medium); // Default
    }

    #[test]
    fn test_button_builder_pattern() {
        // Test new trait-based builder methods
        let button = Button::new("builder-test", "Save")
            .with_variant(ButtonVariant::Primary)
            .with_size(ButtonSize::Small)
            .disabled(false)
            .on_click(|_event, _window, _cx| {
                // Click handler
            });

        assert_eq!(*UIStyled::variant(&button), ButtonVariant::Primary);
        assert_eq!(*UIStyled::size(&button), ButtonSize::Small);
        assert!(!button.is_disabled());
    }

    #[test]
    fn test_list_item_trait_integration() {
        // Test that ListItem implements all expected traits
        let item = ListItem::new("test-item")
            .with_size(ListItemSpacing::Compact)
            .disabled(true)
            .tooltip("Test item tooltip");

        // Test Component trait
        let _id = item.id();
        assert!(item.is_disabled());

        // Test Styled trait (using spacing as size)
        assert_eq!(*UIStyled::size(&item), ListItemSpacing::Compact);
    }

    #[test]
    fn test_list_item_composition() {
        // Test Composable and Slotted traits
        let item = ListItem::new("composed-item")
            .start_slot("🔥") // Icon slot
            .child("Main content")
            .child("Secondary content")
            .end_slot("badge")
            .children(vec!["Item 1", "Item 2"]);

        // Just verify it compiles and can be created
        let _id = item.id();
    }

    #[test]
    fn test_list_item_interaction() {
        // Test Interactive trait with new listener pattern
        let item = ListItem::new("interactive-item")
            .on_click(Box::new(|div| {
                // Primary click handler - just return the div as-is for test
                div
            }))
            .on_secondary_click(Box::new(|div| {
                // Secondary click handler - just return the div as-is for test
                div
            }));

        let _id = item.id();
    }

    #[test]
    fn test_component_factory_patterns() {
        // Test ComponentFactory trait for both components
        let button =
            <Button as ComponentFactory>::with_variant("variant-btn", ButtonVariant::Danger);
        assert_eq!(*UIStyled::variant(&button), ButtonVariant::Danger);

        let list_item =
            <ListItem as ComponentFactory>::with_size("size-item", ListItemSpacing::Spacious);
        assert_eq!(*UIStyled::size(&list_item), ListItemSpacing::Spacious);
    }

    #[test]
    fn test_trait_composition() {
        // Test that traits work together seamlessly
        fn create_styled_button() -> Button {
            <Button as ComponentFactory>::new("generic-btn")
                .with_variant(ButtonVariant::Primary)
                .disabled(false)
        }

        let button = create_styled_button();
        assert_eq!(*UIStyled::variant(&button), ButtonVariant::Primary);
        assert!(!button.is_disabled());
    }

    #[test]
    fn test_backward_compatibility() {
        // Test that old APIs still work alongside new ones
        let button = Button::new("compat-test", "Old API")
            .variant(ButtonVariant::Ghost)     // Old method
            .size(ButtonSize::Large)           // Old method  
            .disabled(true)                    // Same method
; // Same method

        // Should work with both old and new APIs
        let enhanced_button = button
            .with_variant(ButtonVariant::Primary) // New method
            .with_size(ButtonSize::Medium); // New method

        assert_eq!(*UIStyled::variant(&enhanced_button), ButtonVariant::Primary);
        assert_eq!(*UIStyled::size(&enhanced_button), ButtonSize::Medium);
    }
}
