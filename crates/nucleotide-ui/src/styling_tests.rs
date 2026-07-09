#[cfg(test)]
mod tests {
    use crate::{StyleContext, StyleSize, StyleState, StyleVariant, Theme};
    use gpui::px;

    #[test]
    fn style_state_interactivity() {
        assert!(StyleState::Default.is_interactive());
        assert!(StyleState::Hover.is_interactive());
        assert!(!StyleState::Disabled.is_interactive());
        assert!(!StyleState::Loading.is_interactive());
    }

    #[test]
    fn base_style_uses_theme_tokens() {
        let theme = Theme::from_tokens(crate::tokens::DesignTokens::dark());
        let context = StyleContext::new(&theme, StyleState::Default, "primary", "medium");
        let style = context.compute_base_style();

        assert_eq!(style.background, theme.tokens.chrome.surface);
        assert_eq!(style.foreground, theme.tokens.chrome.text_on_chrome);
        assert_eq!(style.border_color, theme.tokens.chrome.border_default);
        assert_eq!(style.padding_x, theme.tokens.sizes.space_3);
        assert_eq!(style.padding_y, theme.tokens.sizes.space_2);
        assert_eq!(style.font_size, px(14.0));
        assert_eq!(style.border_radius, theme.tokens.sizes.radius_md);
    }

    #[test]
    fn style_identifiers_are_stable() {
        assert_eq!(StyleVariant::Primary.as_str(), "primary");
        assert_eq!(StyleVariant::Danger.as_str(), "danger");
        assert_eq!(StyleSize::Small.as_str(), "sm");
        assert_eq!(StyleSize::Large.as_str(), "lg");
    }
}
