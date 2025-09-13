// ABOUTME: Feature flag system for conditional compilation and runtime features
// ABOUTME: Provides compile-time and runtime feature toggling for nucleotide-ui components

use gpui::SharedString;
use std::collections::HashMap;

/// Feature flag configuration
#[derive(Debug, Clone, Default)]
pub struct FeatureFlags {
    /// UI feature flags
    pub ui_features: UIFeatures,
    /// Performance feature flags
    pub performance_features: PerformanceFeatures,
    /// Experimental feature flags
    pub experimental_features: ExperimentalFeatures,
    /// Custom runtime flags
    pub runtime_flags: HashMap<SharedString, bool>,
}

impl FeatureFlags {
    /// Create feature flags with all stable features enabled
    pub fn stable() -> Self {
        Self {
            ui_features: UIFeatures::stable(),
            performance_features: PerformanceFeatures::stable(),
            experimental_features: ExperimentalFeatures::disabled(),
            runtime_flags: HashMap::new(),
        }
    }

    /// Create feature flags with all features enabled (including experimental)
    pub fn all_enabled() -> Self {
        Self {
            ui_features: UIFeatures::all_enabled(),
            performance_features: PerformanceFeatures::all_enabled(),
            experimental_features: ExperimentalFeatures::all_enabled(),
            runtime_flags: HashMap::new(),
        }
    }

    /// Set a runtime flag
    pub fn set_flag(&mut self, name: impl Into<SharedString>, enabled: bool) {
        self.runtime_flags.insert(name.into(), enabled);
    }

    /// Get a runtime flag value
    pub fn get_flag(&self, name: &str) -> bool {
        self.runtime_flags.get(name).copied().unwrap_or(false)
    }

    /// Check if a specific feature is enabled
    pub fn is_enabled(&self, feature: &str) -> bool {
        match feature {
            // UI features
            "animations" => self.ui_features.enable_animations,
            "tooltips" => self.ui_features.enable_tooltips,
            "dark_mode" => self.ui_features.enable_dark_mode,
            "high_contrast" => self.ui_features.enable_high_contrast,
            "reduced_motion" => self.ui_features.enable_reduced_motion,
            "keyboard_navigation" => self.ui_features.enable_keyboard_navigation,

            // Performance features
            "virtualization" => self.performance_features.enable_virtualization,
            "lazy_loading" => self.performance_features.enable_lazy_loading,
            "caching" => self.performance_features.enable_caching,
            "performance_monitoring" => self.performance_features.enable_performance_monitoring,
            "memory_optimization" => self.performance_features.enable_memory_optimization,

            // Experimental features
            "advanced_theming" => self.experimental_features.enable_advanced_theming,
            "component_hot_reload" => self.experimental_features.enable_component_hot_reload,
            "debug_overlays" => self.experimental_features.enable_debug_overlays,
            "beta_components" => self.experimental_features.enable_beta_components,

            // Runtime flags
            _ => self.get_flag(feature),
        }
    }
}

/// UI-specific feature flags
#[derive(Debug, Clone)]
pub struct UIFeatures {
    pub enable_animations: bool,
    pub enable_tooltips: bool,
    pub enable_dark_mode: bool,
    pub enable_high_contrast: bool,
    pub enable_reduced_motion: bool,
    pub enable_keyboard_navigation: bool,
}

impl Default for UIFeatures {
    fn default() -> Self {
        Self {
            enable_animations: true,
            enable_tooltips: true,
            enable_dark_mode: true,
            enable_high_contrast: false,
            enable_reduced_motion: false,
            enable_keyboard_navigation: true,
        }
    }
}

impl UIFeatures {
    pub fn stable() -> Self {
        Self::default()
    }

    pub fn all_enabled() -> Self {
        Self {
            enable_animations: true,
            enable_tooltips: true,
            enable_dark_mode: true,
            enable_high_contrast: true,
            enable_reduced_motion: false, // Reduced motion means fewer animations
            enable_keyboard_navigation: true,
        }
    }

    pub fn accessibility_focused() -> Self {
        Self {
            enable_animations: false,
            enable_tooltips: true,
            enable_dark_mode: true,
            enable_high_contrast: true,
            enable_reduced_motion: true,
            enable_keyboard_navigation: true,
        }
    }
}

/// Performance-specific feature flags
#[derive(Debug, Clone)]
pub struct PerformanceFeatures {
    pub enable_virtualization: bool,
    pub enable_lazy_loading: bool,
    pub enable_caching: bool,
    pub enable_performance_monitoring: bool,
    pub enable_memory_optimization: bool,
}

impl Default for PerformanceFeatures {
    fn default() -> Self {
        Self {
            enable_virtualization: true,
            enable_lazy_loading: true,
            enable_caching: true,
            enable_performance_monitoring: cfg!(debug_assertions),
            enable_memory_optimization: true,
        }
    }
}

impl PerformanceFeatures {
    pub fn stable() -> Self {
        Self::default()
    }

    pub fn all_enabled() -> Self {
        Self {
            enable_virtualization: true,
            enable_lazy_loading: true,
            enable_caching: true,
            enable_performance_monitoring: true,
            enable_memory_optimization: true,
        }
    }

    pub fn minimal() -> Self {
        Self {
            enable_virtualization: false,
            enable_lazy_loading: false,
            enable_caching: false,
            enable_performance_monitoring: false,
            enable_memory_optimization: false,
        }
    }
}

/// Experimental feature flags
#[derive(Debug, Clone)]
pub struct ExperimentalFeatures {
    pub enable_advanced_theming: bool,
    pub enable_component_hot_reload: bool,
    pub enable_debug_overlays: bool,
    pub enable_beta_components: bool,
}

impl Default for ExperimentalFeatures {
    fn default() -> Self {
        Self {
            enable_advanced_theming: false,
            enable_component_hot_reload: cfg!(debug_assertions),
            enable_debug_overlays: cfg!(debug_assertions),
            enable_beta_components: false,
        }
    }
}

impl ExperimentalFeatures {
    pub fn disabled() -> Self {
        Self {
            enable_advanced_theming: false,
            enable_component_hot_reload: false,
            enable_debug_overlays: false,
            enable_beta_components: false,
        }
    }

    pub fn all_enabled() -> Self {
        Self {
            enable_advanced_theming: true,
            enable_component_hot_reload: true,
            enable_debug_overlays: true,
            enable_beta_components: true,
        }
    }

    pub fn development() -> Self {
        Self {
            enable_advanced_theming: true,
            enable_component_hot_reload: true,
            enable_debug_overlays: true,
            enable_beta_components: true,
        }
    }
}

// Removed global feature registry and feature flag macros; prefer passing FeatureFlags via providers

/// Feature configuration loading
pub struct FeatureConfig;

impl FeatureConfig {
    /// Load feature flags from environment variables
    pub fn from_env() -> FeatureFlags {
        let mut flags = FeatureFlags::default();

        // UI features
        if let Ok(val) = std::env::var("NUCLEOTIDE_ENABLE_ANIMATIONS") {
            flags.ui_features.enable_animations = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("NUCLEOTIDE_ENABLE_TOOLTIPS") {
            flags.ui_features.enable_tooltips = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("NUCLEOTIDE_ENABLE_DARK_MODE") {
            flags.ui_features.enable_dark_mode = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("NUCLEOTIDE_ENABLE_HIGH_CONTRAST") {
            flags.ui_features.enable_high_contrast = val.parse().unwrap_or(false);
        }
        if let Ok(val) = std::env::var("NUCLEOTIDE_ENABLE_REDUCED_MOTION") {
            flags.ui_features.enable_reduced_motion = val.parse().unwrap_or(false);
        }

        // Performance features
        if let Ok(val) = std::env::var("NUCLEOTIDE_ENABLE_VIRTUALIZATION") {
            flags.performance_features.enable_virtualization = val.parse().unwrap_or(true);
        }
        if let Ok(val) = std::env::var("NUCLEOTIDE_ENABLE_PERFORMANCE_MONITORING") {
            flags.performance_features.enable_performance_monitoring =
                val.parse().unwrap_or(cfg!(debug_assertions));
        }

        // Experimental features
        if let Ok(val) = std::env::var("NUCLEOTIDE_ENABLE_EXPERIMENTAL")
            && val.parse().unwrap_or(false)
        {
            flags.experimental_features = ExperimentalFeatures::all_enabled();
        }

        flags
    }

    /// Create a configuration for development
    pub fn development() -> FeatureFlags {
        FeatureFlags {
            ui_features: UIFeatures::all_enabled(),
            performance_features: PerformanceFeatures::all_enabled(),
            experimental_features: ExperimentalFeatures::development(),
            runtime_flags: HashMap::new(),
        }
    }

    /// Create a configuration for production
    pub fn production() -> FeatureFlags {
        FeatureFlags {
            ui_features: UIFeatures::stable(),
            performance_features: PerformanceFeatures::stable(),
            experimental_features: ExperimentalFeatures::disabled(),
            runtime_flags: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_flags_default() {
        let flags = FeatureFlags::default();

        assert!(flags.ui_features.enable_animations);
        assert!(flags.ui_features.enable_tooltips);
        assert!(flags.performance_features.enable_virtualization);
        assert!(!flags.experimental_features.enable_advanced_theming);
    }

    #[test]
    fn test_feature_flags_stable() {
        let flags = FeatureFlags::stable();

        assert!(flags.ui_features.enable_animations);
        assert!(!flags.experimental_features.enable_advanced_theming);
        assert!(!flags.experimental_features.enable_beta_components);
    }

    #[test]
    fn test_feature_flags_all_enabled() {
        let flags = FeatureFlags::all_enabled();

        assert!(flags.ui_features.enable_animations);
        assert!(flags.ui_features.enable_high_contrast);
        assert!(flags.performance_features.enable_performance_monitoring);
        assert!(flags.experimental_features.enable_advanced_theming);
    }

    #[test]
    fn test_runtime_flags() {
        let mut flags = FeatureFlags::default();

        assert!(!flags.get_flag("custom_feature"));

        flags.set_flag("custom_feature", true);
        assert!(flags.get_flag("custom_feature"));

        flags.set_flag("custom_feature", false);
        assert!(!flags.get_flag("custom_feature"));
    }

    #[test]
    fn test_named_feature_checking() {
        let flags = FeatureFlags::default();

        assert!(flags.is_enabled("animations"));
        assert!(flags.is_enabled("tooltips"));
        assert!(!flags.is_enabled("advanced_theming"));
        assert!(!flags.is_enabled("nonexistent_feature"));
    }

    #[test]
    fn test_ui_features_accessibility() {
        let flags = UIFeatures::accessibility_focused();

        assert!(!flags.enable_animations);
        assert!(flags.enable_tooltips);
        assert!(flags.enable_high_contrast);
        assert!(flags.enable_reduced_motion);
        assert!(flags.enable_keyboard_navigation);
    }

    #[test]
    fn test_performance_features_minimal() {
        let flags = PerformanceFeatures::minimal();

        assert!(!flags.enable_virtualization);
        assert!(!flags.enable_lazy_loading);
        assert!(!flags.enable_caching);
        assert!(!flags.enable_performance_monitoring);
        assert!(!flags.enable_memory_optimization);
    }

    #[test]
    fn test_experimental_features() {
        let disabled = ExperimentalFeatures::disabled();
        assert!(!disabled.enable_advanced_theming);
        assert!(!disabled.enable_debug_overlays);

        let enabled = ExperimentalFeatures::all_enabled();
        assert!(enabled.enable_advanced_theming);
        assert!(enabled.enable_debug_overlays);
        assert!(enabled.enable_component_hot_reload);
    }

    #[test]
    fn test_feature_config() {
        let dev_config = FeatureConfig::development();
        assert!(dev_config.ui_features.enable_animations);
        assert!(dev_config.experimental_features.enable_advanced_theming);

        let prod_config = FeatureConfig::production();
        assert!(prod_config.ui_features.enable_animations);
        assert!(!prod_config.experimental_features.enable_advanced_theming);
    }
}
