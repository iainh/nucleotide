// ABOUTME: Reusable UI components library following Zed's patterns
// ABOUTME: Provides consistent, styled components for the application

pub mod about_window;
pub mod actions;
pub mod assets;
pub mod button;
pub mod checkbox;
pub mod common;
// Old completion module removed - now using completion_v2
pub mod completion_cache;
pub mod completion_docs;
pub mod completion_error;
pub mod completion_icons;
pub mod completion_perf;
pub mod completion_popup;
pub mod completion_renderer;
pub mod completion_v2;
pub mod component_gallery;
pub mod confirm_dialog;
pub mod context_menu;
pub mod debouncer;
pub mod file_icon;
pub mod focus;
pub mod info_box;
pub mod input;
pub mod key_hint_view;
pub mod layout;
pub mod list_item;
pub mod markdown;
pub mod menu;
pub mod modal_layer;
pub mod navigable;
pub mod notification;
pub mod overlay_surface;
pub mod picker;
pub mod picker_view;
pub mod progress_indicator;
pub mod prompt;
pub mod prompt_view;
pub mod providers;
pub mod scrollbar;
pub mod split;
pub mod style_utils;
pub mod styling;
pub mod terminal_keys;
pub mod text_input;
pub mod text_utils;
pub mod theme_debug;
pub mod theme_manager;
pub mod theme_utils;
pub mod titlebar;
pub mod tokens;
pub mod traits;
pub mod utils;
pub mod vcs_icon;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod initialization_tests;

#[cfg(test)]
mod styling_tests;

pub use about_window::AboutWindow;
pub use assets::Assets;
pub use button::{Button, ButtonSize, ButtonVariant, IconPosition};
pub use checkbox::{Checkbox, CheckboxSize};
pub use completion_docs::{
    DocumentationCache, DocumentationCacheConfig, DocumentationContent, DocumentationLoader,
    DocumentationPanel, DocumentationSource, DocumentationState,
};
pub use completion_error::{
    CompletionError, CompletionErrorHandler, ErrorContext, ErrorHandlingConfig,
    ErrorHandlingResult, ErrorRecoveryExecutor, ErrorSeverity, ErrorStats, RecoveryAction,
    RecoveryResult,
};
pub use completion_icons::{
    CompletionIconConfig, create_completion_icon, create_themed_completion_icon,
    get_completion_icon_color, get_completion_icon_svg,
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
    CompleteViaHelixEvent, CompletionEdit, CompletionItem, CompletionItemKind,
    CompletionMenuAction, CompletionOffsetEncoding, CompletionPosition, CompletionRange,
    CompletionTextEdit, CompletionView, CompletionWarningEvent, Position, StringMatch,
    StringMatchCandidate, completion_menu_action_for_key,
};
pub use component_gallery::ComponentGallery;
pub use confirm_dialog::{
    ConfirmDialog, ConfirmDialogEvent, ConfirmDialogView, DialogDescription, DialogFooter,
    DialogHeader, DialogTitle,
};
pub use context_menu::ContextMenuController;
pub use file_icon::FileIcon;
pub use focus::{FOCUS_TRAVERSAL_CONTEXT, FocusCoordinator, FocusRole, FocusTraversal};
pub use input::{InputSize, InputVariant};
pub use layout::{
    AppShell, BottomPanel, EditorPaneGrid, Panel, PanelLayout, PanelVariant, StatusBar, Toolbar,
    WorkspaceChrome,
};
pub use list_item::{
    ListItem, ListItemSpacing, ListItemState, ListItemVariant, SelectionMode, SelectionState,
};
pub use markdown::{
    MarkdownElement, MarkdownParseMode, MarkdownStyle, markdown, markdown_extended,
};
pub use menu::{MenuCheckSide, PopupMenu, PopupMenuItem, PopupMenuSurface};
pub use modal_layer::{DismissDecision, ModalLayer, ModalOpenedEvent, ModalView};
pub use navigable::{NAVIGABLE_CONTEXT, Navigable, NavigableEntry};
pub use overlay_surface::{OVERLAY_SURFACE_CONTEXT, OverlaySurface};
pub use picker::Picker;
pub use progress_indicator::IndeterminateProgressIndicator;
pub use prompt::Prompt;
pub use providers::{
    AccessibilityConfiguration, ConfigurationProvider, CustomEventData, CustomEventDetails,
    EventHandlingProvider, EventResult, PerformanceConfiguration, Provider, ProviderComposition,
    ProviderContainer, ProviderHooks, ThemeProvider, UIConfiguration, provider_tree, use_provider,
    use_provider_or_default,
};
pub use split::{
    ResizeDragController, SPLITTER_HITBOX_PX, SPLITTER_LINE_PX, SplitterAxis, bottom_panel_split,
    resize_capture_area, resize_handle, right_sidebar_split, sidebar_split, splitter,
    two_pane_split,
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
pub use terminal_keys::{
    TerminalKeyEncodingMode, encode_terminal_key_event, encode_terminal_key_event_with_mode,
};
pub use text_input::{TextInput, TextInputEvent, TextInputFocusStyle};
pub use theme_debug::ThemeDebugView;
pub use tokens::{
    CheckboxTokens, ChromeTokens, ColorContext, CompletionIconTokens, DesignTokens, EditorTokens,
    FileTreeTokens, ShadowToken, SizeTokens, StatusBarTokens, TabBarTokens, TitleBarTokens,
};
pub use traits::{
    Component, ComponentBuilder, ComponentFactory, ComponentState, ComponentStyles, Composable,
    Interactive, KeyboardNavigable, Loadable, Measurable, Slotted, Styled, ThemedContext,
    Tooltipped, Validatable, ValidationState, compute_component_state,
};
pub use utils::{ExperimentalFeatures, FeatureFlags, PerformanceFeatures};
pub use vcs_icon::{VcsIcon, VcsIconRenderer};
// VcsStatus is now re-exported from nucleotide-types

// Export initialization and configuration types
// (Functions are defined in this module, types can be re-exported)

use gpui::{App, ClickEvent, Hsla, MouseClickEvent, MouseDownEvent, MouseUpEvent};

pub fn click_event_from_mouse_down(event: &MouseDownEvent) -> ClickEvent {
    ClickEvent::Mouse(MouseClickEvent {
        down: event.clone(),
        up: MouseUpEvent {
            button: event.button,
            position: event.position,
            modifiers: event.modifiers,
            click_count: event.click_count,
        },
    })
}

/// Application theme built on DesignTokens only
#[derive(Clone, Debug)]
pub struct Theme {
    pub tokens: DesignTokens,
}

impl Default for Theme {
    fn default() -> Self {
        // Default to dark tokens; callers should prefer from_tokens(...)
        Self::from_tokens(DesignTokens::dark())
    }
}

impl Theme {
    /// Create a theme from design tokens (new API)
    pub fn from_tokens(tokens: DesignTokens) -> Self {
        Self { tokens }
    }

    /// Access design tokens directly
    pub fn tokens(&self) -> &DesignTokens {
        &self.tokens
    }

    /// Check if this is a dark theme based on background lightness
    pub fn is_dark(&self) -> bool {
        self.tokens.editor.background.l < 0.5
    }

    /// Get a surface color with the specified elevation
    pub fn surface_at_elevation(&self, elevation: u8) -> Hsla {
        match elevation {
            0 => self.tokens.editor.background,
            1 => self.tokens.chrome.surface,
            2 => self.tokens.chrome.surface_elevated,
            _ => {
                let base = self.tokens.chrome.surface_elevated;
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
            default_theme: Theme::from_tokens(DesignTokens::dark()),
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

pub(crate) const BUILT_IN_COMPONENTS: &[&str] = &[
    "AboutWindow",
    "AppShell",
    "BottomPanel",
    "Button",
    "Checkbox",
    "ComponentGallery",
    "CompletionRenderer",
    "CompletionView",
    "ConfirmDialog",
    "ConfirmDialogView",
    "DocumentationPanel",
    "EditorPaneGrid",
    "FileIcon",
    "FocusTraversal",
    "ListItem",
    "ModalLayer",
    "Navigable",
    "OverlaySurface",
    "Panel",
    "Picker",
    "PopupMenu",
    "PopupMenuSurface",
    "IndeterminateProgressIndicator",
    "Prompt",
    "ResizeDragController",
    "SmartPopup",
    "StatusBar",
    "TextInput",
    "ThemeDebugView",
    "Toolbar",
    "WorkspaceChrome",
];

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
/// ```ignore
/// use nucleotide_ui::{init, UIConfig};
/// use gpui::{App, AppContext};
///
/// fn main() {
///     // Note: This is a simplified example - actual GPUI app initialization
///     // requires more complex setup with entity management
///     
///     // Initialize UI system with default config
///     // nucleotide_ui::init(cx, None);
/// }
/// ```
///
/// # Safety
///
/// This function is safe to call multiple times - subsequent calls will update
/// the configuration but won't cause any issues.
pub fn init(cx: &mut App, config: Option<UIConfig>) {
    let config = config.unwrap_or_default();

    providers::init_provider_system();
    completion_v2::init(cx);
    confirm_dialog::init(cx);
    focus::init(cx);
    menu::init(cx);
    modal_layer::init(cx);
    navigable::init(cx);
    overlay_surface::init(cx);
    picker_view::init(cx);
    prompt_view::init(cx);
    text_input::init(cx);

    let mut theme_provider = providers::ThemeProvider::new(config.default_theme.clone());
    theme_provider.initialize(cx);

    let mut configuration_provider = configuration_provider_from_ui_config(&config);
    configuration_provider.initialize(cx);

    let mut event_provider = providers::EventHandlingProvider::new();
    event_provider.analytics_config.enable_analytics = config.enable_performance_monitoring;
    event_provider.analytics_config.track_performance = config.enable_performance_monitoring;
    event_provider.initialize(cx);

    providers::update_provider_context(|context| {
        context.register_global_provider(theme_provider);
        context.register_global_provider(configuration_provider);
        context.register_global_provider(event_provider);
    });

    // Setup global theme
    cx.set_global(config.default_theme.clone());

    // Setup global configuration
    cx.set_global(config);

    // Initialize component registry
    let mut registry = ComponentRegistry::default();

    // Register built-in components
    for component in BUILT_IN_COMPONENTS {
        registry.register_component(component);
    }

    cx.set_global(registry);
}

fn configuration_provider_from_ui_config(config: &UIConfig) -> providers::ConfigurationProvider {
    let mut provider = if config.features.enable_accessibility {
        providers::ConfigurationProvider::accessibility_focused()
    } else if config.enable_performance_monitoring || config.features.enable_virtualization {
        providers::ConfigurationProvider::performance_focused()
    } else {
        providers::ConfigurationProvider::new()
    };

    provider.ui_config.animation_config.enable_animations = config.features.enable_animations;
    provider.performance_config.enable_virtualization = config.features.enable_virtualization;
    provider
        .feature_flags
        .performance_features
        .enable_virtualization = config.features.enable_virtualization;
    provider
        .feature_flags
        .performance_features
        .enable_performance_monitoring = config.enable_performance_monitoring;
    provider.feature_flags.ui_features.enable_animations = config.features.enable_animations;

    if config.features.enable_accessibility {
        provider.accessibility_config.screen_reader_support = true;
        provider.accessibility_config.high_contrast_mode = true;
        provider
            .accessibility_config
            .focus_config
            .show_focus_indicators = true;
        provider.feature_flags.ui_features.enable_high_contrast = true;
    }

    provider
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
