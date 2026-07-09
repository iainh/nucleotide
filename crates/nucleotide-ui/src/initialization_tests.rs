// ABOUTME: Tests for the nucleotide-ui initialization and configuration system
// ABOUTME: Ensures proper setup of global state, themes, and component registry

#[cfg(test)]
mod tests {
    use crate::{Theme, UIConfig, UIFeatures};

    #[test]
    fn test_ui_config_default() {
        let config = UIConfig::default();

        assert!(config.default_theme.is_dark());
        assert_eq!(config.enable_performance_monitoring, cfg!(debug_assertions));
        assert!(!config.features.enable_virtualization);
        assert!(!config.features.enable_animations);
        assert!(!config.features.enable_accessibility);
        assert!(!config.features.enable_debug_utils);
    }

    #[test]
    fn test_ui_features_default() {
        let features = UIFeatures::default();

        assert!(!features.enable_virtualization);
        assert!(!features.enable_animations);
        assert!(!features.enable_accessibility);
        assert!(!features.enable_debug_utils);
    }

    #[test]
    fn test_ui_config_creation_with_custom_features() {
        let custom_features = UIFeatures {
            enable_virtualization: true,
            enable_animations: false,
            enable_accessibility: true,
            enable_debug_utils: false,
        };

        let config = UIConfig {
            default_theme: Theme::from_tokens(crate::tokens::DesignTokens::light()),
            enable_performance_monitoring: true,
            features: custom_features.clone(),
        };

        assert!(!config.default_theme.is_dark()); // Light theme
        assert!(config.enable_performance_monitoring);
        assert!(config.features.enable_virtualization);
        assert!(!config.features.enable_animations);
        assert!(config.features.enable_accessibility);
        assert!(!config.features.enable_debug_utils);
    }

    #[test]
    fn test_ui_config_with_all_features_enabled() {
        let all_features = UIFeatures {
            enable_virtualization: true,
            enable_animations: true,
            enable_accessibility: true,
            enable_debug_utils: true,
        };

        let config = UIConfig {
            default_theme: Theme::from_tokens(crate::tokens::DesignTokens::dark()),
            enable_performance_monitoring: false,
            features: all_features,
        };

        assert!(config.default_theme.is_dark());
        assert!(!config.enable_performance_monitoring);
        assert!(config.features.enable_virtualization);
        assert!(config.features.enable_animations);
        assert!(config.features.enable_accessibility);
        assert!(config.features.enable_debug_utils);
    }

    #[test]
    fn test_ui_features_combinations() {
        // Test various feature combinations
        let minimal = UIFeatures {
            enable_virtualization: true,
            ..Default::default()
        };
        assert!(minimal.enable_virtualization);
        assert!(!minimal.enable_animations);
        assert!(!minimal.enable_accessibility);
        assert!(!minimal.enable_debug_utils);

        let accessibility_focused = UIFeatures {
            enable_accessibility: true,
            enable_debug_utils: true,
            ..Default::default()
        };
        assert!(!accessibility_focused.enable_virtualization);
        assert!(!accessibility_focused.enable_animations);
        assert!(accessibility_focused.enable_accessibility);
        assert!(accessibility_focused.enable_debug_utils);

        let performance_focused = UIFeatures {
            enable_virtualization: true,
            enable_animations: false,    // Disable for performance
            enable_accessibility: false, // Disable for performance
            enable_debug_utils: false,
        };
        assert!(performance_focused.enable_virtualization);
        assert!(!performance_focused.enable_animations);
        assert!(!performance_focused.enable_accessibility);
        assert!(!performance_focused.enable_debug_utils);
    }

    #[test]
    fn test_theme_integration_with_config() {
        // Test that themes integrate properly with config
        let dark_config = UIConfig {
            default_theme: Theme::from_tokens(crate::tokens::DesignTokens::dark()),
            enable_performance_monitoring: false,
            features: UIFeatures::default(),
        };

        assert!(dark_config.default_theme.is_dark());
        // Editor background should be defined
        assert!(dark_config.default_theme.tokens.editor.background.a > 0.0);

        let light_config = UIConfig {
            default_theme: Theme::from_tokens(crate::tokens::DesignTokens::light()),
            enable_performance_monitoring: false,
            features: UIFeatures::default(),
        };

        assert!(!light_config.default_theme.is_dark());
        assert!(light_config.default_theme.tokens.editor.background.a > 0.0);
    }

    #[test]
    fn test_performance_monitoring_default() {
        let config = UIConfig::default();

        // Performance monitoring should be enabled in debug builds by default
        #[cfg(debug_assertions)]
        assert!(config.enable_performance_monitoring);

        #[cfg(not(debug_assertions))]
        assert!(!config.enable_performance_monitoring);
    }

    #[gpui::test]
    async fn init_registers_required_globals(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::init(cx, None);

            assert!(cx.has_global::<crate::Theme>());
            assert!(cx.has_global::<crate::UIConfig>());
        });
    }
}
