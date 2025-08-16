// ABOUTME: Reusable UI components library following Zed's patterns
// ABOUTME: Provides consistent, styled components for the application

pub mod actions;
pub mod assets;
pub mod button;
pub mod common;
pub mod completion;
pub mod file_icon;
pub mod info_box;
pub mod key_hint_view;
pub mod list_item;
pub mod notification;
pub mod picker;
pub mod picker_delegate;
pub mod picker_element;
pub mod picker_view;
pub mod prompt;
pub mod prompt_view;
pub mod scrollbar;
pub mod style_utils;
pub mod text_utils;
pub mod theme_manager;
pub mod theme_utils;
pub mod titlebar;
pub mod tokens;
pub mod traits;
pub mod vcs_indicator;
pub mod styling;
pub mod keyboard_navigation;
pub mod utils;
pub mod providers;
pub mod advanced_theming;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod initialization_tests;

#[cfg(test)]
mod styling_tests;

pub use assets::Assets;
pub use button::{Button, ButtonSize, ButtonVariant, IconPosition};
pub use file_icon::FileIcon;
pub use list_item::{ListItem, ListItemSpacing, ListItemVariant, ListItemState, SelectionMode, SelectionState};
pub use keyboard_navigation::{
    KeyboardNavigationHandler, NavigationDirection, NavigationAction, NavigationResult, ListVirtualization
};
pub use picker::Picker;
pub use prompt::{Prompt, PromptElement};
pub use tokens::{DesignTokens, SemanticColors, SizeTokens};
pub use traits::{
    Component, Styled, Interactive, Tooltipped, Composable, Slotted,
    ComponentStyles, ThemedContext, ComponentBuilder, Measurable, 
    Validatable, ValidationState, ComponentFactory, KeyboardNavigable,
    Loadable, ComponentState, compute_component_state
};
pub use vcs_indicator::{VcsIndicator, VcsStatus};
pub use styling::{
    StyleState, ComputedStyle, BoxShadow, Transition, TimingFunction, TransitionProperty,
    StyleContext, compute_component_style, compute_style_for_states, should_enable_animations,
    StyleVariant, StyleSize, VariantColors, VariantStyle, VariantStyler,
    Breakpoint, ResponsiveValue, ResponsiveSizes, ResponsiveTypography, ViewportContext,
    AnimationDuration, AnimationPreset, AnimationProperty, AnimationConfig, AnimationType,
    MergeStrategy, StyleCombiner, merge_styles, ConditionalStyle, StyleComposer, StyleUtils, StylePresets
};
pub use providers::{
    Provider, ProviderContainer, use_provider, use_provider_or_default, provider_tree, 
    ProviderComposition, ProviderHooks, ThemeProvider, ConfigurationProvider, EventHandlingProvider,
    UIConfiguration, AccessibilityConfiguration, PerformanceConfiguration, 
    CustomEventDetails, CustomEventData, EventResult
};
pub use utils::{
    FeatureFlags, PerformanceFeatures, ExperimentalFeatures,
    is_feature_enabled as is_utils_feature_enabled, is_named_feature_enabled,
    PerfTimer, Profiler, MemoryTracker, FocusManager, FocusGroup, KeyboardShortcut, ShortcutRegistry
};
pub use advanced_theming::{
    AdvancedThemeManager, ThemeBuilder, ThemeValidator, ThemeAnimator, HelixThemeBridge, RuntimeThemeSwitcher,
    ThemeRegistry, ThemeMetadata, ThemeCategory, ValidationResult, AnimationStep, HelixThemeDiscovery
};

// Export initialization and configuration types 
// (Functions are defined in this module, types can be re-exported)

use gpui::{App, Hsla};

/// Standard spacing values following Zed's design system
pub mod spacing {
    use gpui::px;

    pub const XS: gpui::Pixels = px(2.);
    pub const SM: gpui::Pixels = px(4.);
    pub const MD: gpui::Pixels = px(8.);
    pub const LG: gpui::Pixels = px(12.);
}

/// Theme trait for consistent styling
pub trait Themed {
    fn theme(&self, cx: &App) -> &Theme;
}

/// Application theme following Zed's pattern
/// Enhanced with design token integration while maintaining backward compatibility
#[derive(Clone, Debug)]
pub struct Theme {
    // Legacy fields for backward compatibility
    pub background: Hsla,
    pub surface: Hsla,
    pub surface_background: Hsla,
    pub surface_hover: Hsla,
    pub surface_active: Hsla,
    pub border: Hsla,
    pub border_focused: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_disabled: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_active: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    pub success: Hsla,

    // Design token integration
    pub tokens: DesignTokens,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        let tokens = DesignTokens::dark();
        Self {
            // Legacy fields mapped from design tokens
            background: tokens.colors.background,
            surface: tokens.colors.surface,
            surface_background: tokens.colors.surface_elevated,
            surface_hover: tokens.colors.surface_hover,
            surface_active: tokens.colors.surface_active,
            border: tokens.colors.border_default,
            border_focused: tokens.colors.border_focus,
            text: tokens.colors.text_primary,
            text_muted: tokens.colors.text_secondary,
            text_disabled: tokens.colors.text_disabled,
            accent: tokens.colors.primary,
            accent_hover: tokens.colors.primary_hover,
            accent_active: tokens.colors.primary_active,
            error: tokens.colors.error,
            warning: tokens.colors.warning,
            success: tokens.colors.success,

            // Design tokens
            tokens,
        }
    }

    pub fn light() -> Self {
        let tokens = DesignTokens::light();
        Self {
            // Legacy fields mapped from design tokens
            background: tokens.colors.background,
            surface: tokens.colors.surface,
            surface_background: tokens.colors.surface_elevated,
            surface_hover: tokens.colors.surface_hover,
            surface_active: tokens.colors.surface_active,
            border: tokens.colors.border_default,
            border_focused: tokens.colors.border_focus,
            text: tokens.colors.text_primary,
            text_muted: tokens.colors.text_secondary,
            text_disabled: tokens.colors.text_disabled,
            accent: tokens.colors.primary,
            accent_hover: tokens.colors.primary_hover,
            accent_active: tokens.colors.primary_active,
            error: tokens.colors.error,
            warning: tokens.colors.warning,
            success: tokens.colors.success,

            // Design tokens
            tokens,
        }
    }

    /// Create a theme from design tokens (new API)
    pub fn from_tokens(tokens: DesignTokens) -> Self {
        Self {
            // Legacy fields mapped from design tokens
            background: tokens.colors.background,
            surface: tokens.colors.surface,
            surface_background: tokens.colors.surface_elevated,
            surface_hover: tokens.colors.surface_hover,
            surface_active: tokens.colors.surface_active,
            border: tokens.colors.border_default,
            border_focused: tokens.colors.border_focus,
            text: tokens.colors.text_primary,
            text_muted: tokens.colors.text_secondary,
            text_disabled: tokens.colors.text_disabled,
            accent: tokens.colors.primary,
            accent_hover: tokens.colors.primary_hover,
            accent_active: tokens.colors.primary_active,
            error: tokens.colors.error,
            warning: tokens.colors.warning,
            success: tokens.colors.success,

            // Design tokens
            tokens,
        }
    }

    /// Access design tokens directly
    pub fn tokens(&self) -> &DesignTokens {
        &self.tokens
    }

    /// Check if this is a dark theme based on background lightness
    pub fn is_dark(&self) -> bool {
        self.background.l < 0.5
    }

    /// Get a surface color with the specified elevation
    pub fn surface_at_elevation(&self, elevation: u8) -> Hsla {
        match elevation {
            0 => self.tokens.colors.background,
            1 => self.tokens.colors.surface,
            2 => self.tokens.colors.surface_elevated,
            _ => {
                // For higher elevations, add more lightness/darkness
                let base = self.tokens.colors.surface_elevated;
                let adjustment = (elevation as f32 - 2.0) * 0.02;
                if self.is_dark() {
                    tokens::lighten(base, adjustment)
                } else {
                    tokens::darken(base, adjustment)
                }
            }
        }
    }
}

impl gpui::Global for Theme {}

/// Configuration for the nucleotide-ui component system
#[derive(Debug, Clone)]
pub struct UIConfig {
    /// Default theme to use when no theme is specified
    pub default_theme: Theme,
    /// Whether to enable performance monitoring
    pub enable_performance_monitoring: bool,
    /// Feature flags for optional components
    pub features: UIFeatures,
}

/// Feature flags for optional components and functionality
#[derive(Debug, Clone, Default)]
pub struct UIFeatures {
    /// Enable virtualization for large lists
    pub enable_virtualization: bool,
    /// Enable animation transitions
    pub enable_animations: bool,
    /// Enable accessibility features
    pub enable_accessibility: bool,
    /// Enable debugging utilities in development
    pub enable_debug_utils: bool,
}

impl Default for UIConfig {
    fn default() -> Self {
        Self {
            default_theme: Theme::dark(),
            enable_performance_monitoring: cfg!(debug_assertions),
            features: UIFeatures::default(),
        }
    }
}

impl gpui::Global for UIConfig {}

/// Component registration system for future extensibility
#[derive(Debug, Default)]
pub struct ComponentRegistry {
    /// Registered component types
    registered_components: std::collections::HashSet<&'static str>,
}

impl ComponentRegistry {
    /// Register a component type
    pub fn register_component(&mut self, component_type: &'static str) {
        self.registered_components.insert(component_type);
    }

    /// Check if a component type is registered
    pub fn is_registered(&self, component_type: &'static str) -> bool {
        self.registered_components.contains(component_type)
    }

    /// Get all registered component types
    pub fn registered_components(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.registered_components.iter().copied()
    }
}

impl gpui::Global for ComponentRegistry {}

/// Initialize the nucleotide-ui component system
/// 
/// This function should be called once during application startup to:
/// - Setup global state management for themes and configuration
/// - Initialize the component registration system
/// - Setup performance monitoring (if enabled)
/// - Configure default themes and tokens
/// 
/// # Arguments
/// 
/// * `cx` - GPUI app context for setting up global state
/// * `config` - Optional configuration, uses default if None
/// 
/// # Example
/// 
/// ```no_run
/// use nucleotide_ui::{init, UIConfig};
/// use gpui::App;
/// 
/// fn main() {
///     App::new().run(|cx| {
///         // Initialize UI system with default config
///         nucleotide_ui::init(cx, None);
///         
///         // Your app code here
///     });
/// }
/// ```
/// 
/// # Safety
/// 
/// This function is safe to call multiple times - subsequent calls will update
/// the configuration but won't cause any issues.
pub fn init(cx: &mut App, config: Option<UIConfig>) {
    let config = config.unwrap_or_default();
    
    // Setup global theme
    cx.set_global(config.default_theme.clone());
    
    // Setup global configuration
    cx.set_global(config);
    
    // Initialize component registry
    let mut registry = ComponentRegistry::default();
    
    // Register built-in components
    registry.register_component("Button");
    registry.register_component("ListItem");
    registry.register_component("VcsIndicator");
    registry.register_component("FileIcon");
    registry.register_component("Picker");
    registry.register_component("Prompt");
    
    cx.set_global(registry);
    
    // TODO: Setup performance monitoring when enabled
    // TODO: Setup event handling system
    // TODO: Initialize accessibility features
}

/// Get the current UI configuration
/// 
/// # Panics
/// 
/// Panics if `init()` has not been called first to setup the global configuration.
pub fn get_config(cx: &App) -> &UIConfig {
    cx.global::<UIConfig>()
}

/// Get the current component registry
/// 
/// # Panics
/// 
/// Panics if `init()` has not been called first to setup the component registry.
pub fn get_registry(cx: &App) -> &ComponentRegistry {
    cx.global::<ComponentRegistry>()
}

/// Check if a feature is enabled in the current configuration
/// 
/// # Arguments
/// 
/// * `cx` - GPUI app context
/// * `feature_check` - Closure that takes UIFeatures and returns bool
/// 
/// # Example
/// 
/// ```no_run
/// use nucleotide_ui::is_feature_enabled;
/// use gpui::App;
/// 
/// fn my_component(cx: &App) {
///     if is_feature_enabled(cx, |features| features.enable_animations) {
///         // Use animations
///     }
/// }
/// ```
pub fn is_feature_enabled<F>(cx: &App, feature_check: F) -> bool 
where
    F: FnOnce(&UIFeatures) -> bool,
{
    let config = get_config(cx);
    feature_check(&config.features)
}

/// Update the global theme
/// 
/// This allows runtime theme switching without restarting the application.
/// 
/// # Arguments
/// 
/// * `cx` - Mutable GPUI app context  
/// * `theme` - New theme to apply
pub fn update_theme(cx: &mut App, theme: Theme) {
    cx.set_global(theme);
}
