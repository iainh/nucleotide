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

    #[gpui::test]
    fn icons_preserve_sizing_spacing_and_pointer_handlers(cx: &mut gpui::TestAppContext) {
        use gpui::{
            Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
            Styled as _, Window, div, px,
        };
        use std::{cell::Cell, rc::Rc};

        struct Icons(Rc<Cell<usize>>);
        impl Render for Icons {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let first = self.0.clone();
                let second = self.0.clone();
                div().flex().gap(px(7.0)).children([
                    crate::icon()
                        .path("icons/plus.svg")
                        .debug_selector(|| "native-icon".into())
                        .w(px(20.0))
                        .h(px(14.0))
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            first.set(first.get() + 1)
                        }),
                    crate::icon()
                        .path("icons/completion-function.svg")
                        .debug_selector(|| "fallback-icon".into())
                        .w(px(36.0))
                        .h(px(28.0))
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            second.set(second.get() + 10)
                        }),
                ])
            }
        }

        let clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|_, _| Icons(clicks.clone()));
        cx.run_until_parked();
        let first = cx.debug_bounds("native-icon").unwrap();
        let second = cx.debug_bounds("fallback-icon").unwrap();
        assert_eq!(first.size, gpui::size(px(20.0), px(14.0)));
        assert_eq!(second.size, gpui::size(px(36.0), px(28.0)));
        assert_eq!(second.left() - first.right(), px(7.0));
        cx.simulate_click(first.center(), gpui::Modifiers::default());
        assert_eq!(clicks.get(), 1);
        cx.simulate_click(second.center(), gpui::Modifiers::default());
        assert_eq!(clicks.get(), 11);
    }
}
