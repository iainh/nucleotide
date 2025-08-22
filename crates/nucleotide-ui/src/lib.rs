// ABOUTME: Reusable UI components library following Zed's patterns
// ABOUTME: Provides consistent, styled components for the application

pub mod actions;
pub mod advanced_theming;
pub mod assets;
pub mod button;
pub mod common;
// Old completion module removed - now using completion_v2
pub mod completion_cache;
pub mod completion_docs;
pub mod completion_error;
pub mod completion_keyboard;
pub mod completion_perf;
pub mod completion_popup;
pub mod completion_renderer;
pub mod completion_v2;
pub mod debouncer;
pub mod file_icon;
pub mod focus_indicator;
pub mod fuzzy;
pub mod global_input;
pub mod info_box;
pub mod key_hint_view;
pub mod keyboard_navigation;
pub mod list_item;
pub mod notification;
pub mod picker;
pub mod picker_delegate;
pub mod picker_element;
pub mod picker_view;
pub mod prompt;
pub mod prompt_view;
pub mod providers;
pub mod scrollbar;
pub mod style_utils;
pub mod styling;
pub mod text_utils;
pub mod theme_manager;
pub mod theme_utils;
pub mod titlebar;
pub mod tokens;
pub mod traits;
pub mod utils;
pub mod vcs_icon;
pub mod vcs_indicator;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod initialization_tests;

#[cfg(test)]
mod styling_tests;

#[cfg(test)]
mod theme_mapping_test;

pub use advanced_theming::{
    AdvancedThemeManager, AnimationStep, HelixThemeBridge, HelixThemeDiscovery,
    RuntimeThemeSwitcher, ThemeAnimator, ThemeBuilder, ThemeCategory, ThemeMetadata, ThemeRegistry,
    ThemeValidator, ValidationResult,
};
pub use assets::Assets;
pub use button::{Button, ButtonSize, ButtonVariant, IconPosition};
pub use completion_docs::{
    DocumentationCache, DocumentationCacheConfig, DocumentationContent, DocumentationLoader,
    DocumentationPanel, DocumentationSource, DocumentationState, MarkdownRenderer,
};
pub use completion_error::{
    CompletionError, CompletionErrorHandler, ErrorContext, ErrorHandlingConfig,
    ErrorHandlingResult, ErrorRecoveryExecutor, ErrorSeverity, ErrorStats, RecoveryAction,
    RecoveryResult,
};
pub use completion_keyboard::{
    CompletionAction, CompletionFocusManager, CompletionKeyboardHandler, KeyboardConfig,
    KeyboardNavigationResult, TriggerDetector,
};
pub use completion_popup::{
    AvailableSpace, PopupConstraints, PopupPlacement, PopupPosition, PopupPositioner, SmartPopup,
    create_completion_popup,
};
pub use completion_renderer::{
    CompletionIcon, CompletionItemElement, CompletionListState, get_completion_icon,
    render_completion_list,
};
pub use completion_v2::{
    CompletionAcceptedEvent, CompletionItem, CompletionItemKind, CompletionView, Position,
    StringMatch, StringMatchCandidate,
};
pub use file_icon::FileIcon;
pub use focus_indicator::{
    FocusIndicator, focused_element, high_contrast_focus_ring, subtle_focus_ring,
};
pub use global_input::{
    DismissTarget, FocusElement, FocusElementType, FocusGroup as GlobalFocusGroup, FocusGroupInfo,
    FocusIndicatorConfig, FocusIndicatorStyle, FocusIndicatorStyles, FocusPriority,
    FocusedElementInfo, GlobalInputDispatcher, InputContext,
    NavigationDirection as GlobalNavigationDirection, NavigationOptions, ShortcutAction,
    ShortcutDefinition, ShortcutInfo,
};
pub use keyboard_navigation::{
    KeyboardNavigationHandler, ListVirtualization, NavigationAction,
    NavigationDirection as KeyboardNavigationDirection, NavigationResult,
};
pub use list_item::{
    ListItem, ListItemSpacing, ListItemState, ListItemVariant, SelectionMode, SelectionState,
};
pub use picker::Picker;
pub use prompt::{Prompt, PromptElement};
pub use providers::{
    AccessibilityConfiguration, ConfigurationProvider, CustomEventData, CustomEventDetails,
    EventHandlingProvider, EventResult, PerformanceConfiguration, Provider, ProviderComposition,
    ProviderContainer, ProviderHooks, ThemeProvider, UIConfiguration, provider_tree, use_provider,
    use_provider_or_default,
};
pub use styling::{
    AnimationConfig, AnimationDuration, AnimationPreset, AnimationProperty, AnimationType,
    BoxShadow, Breakpoint, ColorTheory, ComputedStyle, ConditionalStyle, ContextualColors,
    ContrastRatios, MergeStrategy, ResponsiveSizes, ResponsiveTypography, ResponsiveValue,
    StyleCombiner, StyleComposer, StyleContext, StylePresets, StyleSize, StyleState, StyleUtils,
    StyleVariant, TimingFunction, Transition, TransitionProperty, VariantColors, VariantStyle,
    VariantStyler, ViewportContext, compute_component_style, compute_contextual_style,
    compute_style_for_states, merge_styles, should_enable_animations,
};
pub use tokens::{ColorContext, DesignTokens, SemanticColors, SizeTokens, TitleBarTokens};
pub use traits::{
    Component, ComponentBuilder, ComponentFactory, ComponentState, ComponentStyles, Composable,
    Interactive, KeyboardNavigable, Loadable, Measurable, Slotted, Styled, ThemedContext,
    Tooltipped, Validatable, ValidationState, compute_component_state,
};
pub use utils::{
    ExperimentalFeatures, FeatureFlags, FocusGroup as UtilsFocusGroup, FocusManager,
    KeyboardShortcut, MemoryTracker, PerfTimer, PerformanceFeatures, Profiler,
    ShortcutRegistry as UtilsShortcutRegistry, is_feature_enabled as is_utils_feature_enabled,
    is_named_feature_enabled,
};
pub use vcs_icon::{VcsIcon, VcsIconRenderer};
pub use vcs_indicator::{VcsIndicator, VcsStatus};

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
    registry.register_component("CompletionView");
    registry.register_component("CompletionRenderer");
    registry.register_component("SmartPopup");
    registry.register_component("DocumentationPanel");

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
