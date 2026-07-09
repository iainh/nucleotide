// ABOUTME: Tests for the nucleotide-ui initialization and configuration system
// ABOUTME: Ensures proper setup of global state, themes, and component registry

#[cfg(test)]
mod tests {
    use crate::{
        BUILT_IN_COMPONENTS, ComponentRegistry, Theme, UIConfig, UIFeatures,
        configuration_provider_from_ui_config,
    };

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
    fn test_component_registry_operations() {
        let mut registry = ComponentRegistry::default();

        // Test registration
        registry.register_component("TestComponent");
        assert!(registry.is_registered("TestComponent"));

        // Test duplicate registration (should be safe)
        registry.register_component("TestComponent");
        assert!(registry.is_registered("TestComponent"));

        // Test listing components
        registry.register_component("AnotherComponent");
        let components: Vec<_> = registry.registered_components().collect();
        assert!(components.contains(&"TestComponent"));
        assert!(components.contains(&"AnotherComponent"));
        assert_eq!(components.len(), 2);

        // Test non-existent component
        assert!(!registry.is_registered("NonExistentComponent"));
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
    fn test_component_registry_empty_state() {
        let registry = ComponentRegistry::default();

        // Test empty registry
        assert!(!registry.is_registered("AnyComponent"));

        let components: Vec<_> = registry.registered_components().collect();
        assert!(components.is_empty());
    }

    #[test]
    fn test_component_registry_with_builtin_components() {
        let mut registry = ComponentRegistry::default();

        // Simulate registering built-in components (like init() does)
        for component in BUILT_IN_COMPONENTS {
            registry.register_component(component);
        }

        // Test that all built-in components are registered
        for component in BUILT_IN_COMPONENTS {
            assert!(registry.is_registered(component));
        }

        // Test component count
        let components: Vec<_> = registry.registered_components().collect();
        assert_eq!(components.len(), BUILT_IN_COMPONENTS.len());
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

    #[test]
    fn test_ui_config_initializes_provider_performance_flags() {
        let config = UIConfig {
            default_theme: Theme::from_tokens(crate::tokens::DesignTokens::dark()),
            enable_performance_monitoring: true,
            features: UIFeatures {
                enable_virtualization: true,
                enable_animations: false,
                enable_accessibility: false,
                enable_debug_utils: false,
            },
        };

        let provider = configuration_provider_from_ui_config(&config);

        assert!(provider.performance_config.enable_virtualization);
        assert!(
            provider
                .feature_flags
                .performance_features
                .enable_performance_monitoring
        );
        assert!(
            provider
                .feature_flags
                .performance_features
                .enable_virtualization
        );
        assert!(!provider.ui_config.animation_config.enable_animations);
    }

    #[test]
    fn test_ui_config_initializes_provider_accessibility_flags() {
        let config = UIConfig {
            default_theme: Theme::from_tokens(crate::tokens::DesignTokens::dark()),
            enable_performance_monitoring: false,
            features: UIFeatures {
                enable_virtualization: false,
                enable_animations: true,
                enable_accessibility: true,
                enable_debug_utils: false,
            },
        };

        let provider = configuration_provider_from_ui_config(&config);

        assert!(provider.accessibility_config.screen_reader_support);
        assert!(provider.accessibility_config.high_contrast_mode);
        assert!(
            provider
                .accessibility_config
                .focus_config
                .show_focus_indicators
        );
        assert!(provider.feature_flags.ui_features.enable_high_contrast);
    }

    #[gpui::test]
    async fn init_registers_required_global_providers(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            crate::init(cx, None);

            crate::providers::with_provider_context(|context| {
                assert!(
                    context
                        .get_global_provider::<crate::providers::ThemeProvider>()
                        .is_some(),
                    "theme provider should be installed during UI initialization"
                );
                assert!(
                    context
                        .get_global_provider::<crate::providers::ConfigurationProvider>()
                        .is_some(),
                    "configuration provider should be installed during UI initialization"
                );
            })
            .expect("provider system should be initialized");
        });
    }
}
