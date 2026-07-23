// ABOUTME: Integration tests for trait implementations on real components
// ABOUTME: Ensures Button and ListItem work correctly with the trait system

#[cfg(test)]
mod tests {
    use crate::{
        Button, ButtonSize, ButtonVariant, Component, ComponentFactory, ListItem, ListItemSpacing,
        Styled,
    };

    #[test]
    fn button_implements_component_style_and_factory_contracts() {
        let button = <Button as ComponentFactory>::new("button");
        assert_eq!(
            <Button as Styled>::variant(&button),
            &ButtonVariant::Secondary
        );
        assert_eq!(<Button as Styled>::size(&button), &ButtonSize::Medium);

        let button = <Button as Styled>::with_variant(button, ButtonVariant::Danger);
        let button = <Button as Styled>::with_size(button, ButtonSize::Large);
        let button = <Button as Component>::disabled(button, true);

        let _ = <Button as Component>::id(&button);
        assert!(<Button as Component>::is_disabled(&button));
        assert_eq!(<Button as Styled>::variant(&button), &ButtonVariant::Danger);
        assert_eq!(<Button as Styled>::size(&button), &ButtonSize::Large);
    }

    #[test]
    fn list_item_implements_component_style_and_factory_contracts() {
        let item = <ListItem as ComponentFactory>::new("item");
        let item = <ListItem as Styled>::with_size(item, ListItemSpacing::Compact);
        let item = <ListItem as Component>::disabled(item, true);

        let _ = <ListItem as Component>::id(&item);
        assert!(<ListItem as Component>::is_disabled(&item));
        assert_eq!(<ListItem as Styled>::size(&item), &ListItemSpacing::Compact);
    }
}
