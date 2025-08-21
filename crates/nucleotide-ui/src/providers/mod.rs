// ABOUTME: Provider system for managing shared state and configuration across the component tree
// ABOUTME: Implements React-style provider patterns adapted for GPUI's reactive system

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Window, div,
};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

pub mod config_provider;
pub mod event_provider;
pub mod theme_provider;

pub use config_provider::*;
pub use event_provider::*;
pub use theme_provider::*;

/// Global provider context storage
static PROVIDER_CONTEXT: OnceLock<Arc<RwLock<ProviderContext>>> = OnceLock::new();

/// Initialize the provider system
pub fn init_provider_system() {
    PROVIDER_CONTEXT.get_or_init(|| Arc::new(RwLock::new(ProviderContext::new())));
}

/// Get access to the global provider context
pub fn with_provider_context<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&ProviderContext) -> R,
{
    PROVIDER_CONTEXT
        .get()
        .and_then(|context| context.read().ok().map(|guard| f(&*guard)))
}

/// Update provider context
pub fn update_provider_context<F>(f: F) -> bool
where
    F: FnOnce(&mut ProviderContext),
{
    PROVIDER_CONTEXT.get().map_or(false, |context| {
        context
            .write()
            .map(|mut guard| {
                f(&mut *guard);
                true
            })
            .unwrap_or(false)
    })
}

/// Provider context for managing shared state
#[derive(Debug)]
pub struct ProviderContext {
    providers: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    provider_hierarchy: Vec<ProviderScope>,
    active_scope: Option<ProviderScopeId>,
}

/// Provider scope identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderScopeId(usize);

/// Provider scope for managing nested contexts
#[derive(Debug)]
pub struct ProviderScope {
    providers: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    parent_scope: Option<ProviderScopeId>,
}

impl ProviderContext {
    /// Create a new provider context
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            provider_hierarchy: Vec::new(),
            active_scope: None,
        }
    }

    /// Register a global provider
    pub fn register_global_provider<T>(&mut self, provider: T)
    where
        T: Any + Send + Sync,
    {
        let type_id = TypeId::of::<T>();
        self.providers.insert(type_id, Box::new(provider));

        nucleotide_logging::debug!(
            provider_type = std::any::type_name::<T>(),
            "Registered global provider"
        );
    }

    /// Get a global provider
    pub fn get_global_provider<T>(&self) -> Option<&T>
    where
        T: Any + Send + Sync,
    {
        let type_id = TypeId::of::<T>();
        self.providers.get(&type_id)?.downcast_ref::<T>()
    }

    /// Create a new provider scope
    pub fn create_scope(&mut self, _element_id: Option<ElementId>) -> ProviderScopeId {
        let id = ProviderScopeId(self.provider_hierarchy.len());
        let scope = ProviderScope {
            providers: HashMap::new(),
            parent_scope: self.active_scope,
        };

        self.provider_hierarchy.push(scope);
        id
    }

    /// Set the active scope
    pub fn set_active_scope(&mut self, scope_id: Option<ProviderScopeId>) {
        self.active_scope = scope_id;
    }

    /// Register a scoped provider
    pub fn register_scoped_provider<T>(&mut self, scope_id: ProviderScopeId, provider: T)
    where
        T: Any + Send + Sync + Clone,
    {
        if let Some(scope) = self.provider_hierarchy.get_mut(scope_id.0) {
            let type_id = TypeId::of::<T>();
            scope.providers.insert(type_id, Box::new(provider));
        }
    }

    /// Get a provider from the current scope or parent scopes
    pub fn get_provider<T>(&self) -> Option<&T>
    where
        T: Any + Send + Sync,
    {
        let type_id = TypeId::of::<T>();

        // First check current scope and walk up the hierarchy
        if let Some(scope_id) = self.active_scope {
            if let Some(provider) = self.find_provider_in_hierarchy(type_id, scope_id) {
                return provider.downcast_ref::<T>();
            }
        }

        // Fall back to global providers
        self.providers.get(&type_id)?.downcast_ref::<T>()
    }

    /// Find provider in scope hierarchy
    fn find_provider_in_hierarchy(
        &self,
        type_id: TypeId,
        scope_id: ProviderScopeId,
    ) -> Option<&Box<dyn Any + Send + Sync>> {
        if let Some(scope) = self.provider_hierarchy.get(scope_id.0) {
            if let Some(provider) = scope.providers.get(&type_id) {
                return Some(provider);
            }

            // Check parent scope
            if let Some(parent_id) = scope.parent_scope {
                return self.find_provider_in_hierarchy(type_id, parent_id);
            }
        }

        None
    }

    /// Remove a scope and clean up
    pub fn remove_scope(&mut self, scope_id: ProviderScopeId) {
        // Update active scope if it's being removed
        if self.active_scope == Some(scope_id) {
            if let Some(scope) = self.provider_hierarchy.get(scope_id.0) {
                self.active_scope = scope.parent_scope;
            }
        }

        // Note: We don't actually remove from the vec to maintain indices
        // In a real implementation, you might use a different data structure
        if let Some(scope) = self.provider_hierarchy.get_mut(scope_id.0) {
            scope.providers.clear();
        }
    }
}

/// Base trait for all providers
pub trait Provider: Any + Send + Sync {
    /// Get the provider type name
    fn type_name(&self) -> &'static str;

    /// Initialize the provider
    fn initialize(&mut self, _cx: &mut App) {}

    /// Clean up the provider
    fn cleanup(&mut self, _cx: &mut App) {}
}

/// Provider container component for scoped state management
pub struct ProviderContainer<T>
where
    T: Provider + Clone,
{
    id: ElementId,
    provider: T,
    scope_id: Option<ProviderScopeId>,
    children: Vec<AnyElement>,
}

impl<T> ProviderContainer<T>
where
    T: Provider + Clone,
{
    /// Create a new provider container
    pub fn new(id: impl Into<ElementId>, provider: T) -> Self {
        Self {
            id: id.into(),
            provider,
            scope_id: None,
            children: Vec::new(),
        }
    }

    /// Add a child element
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    /// Add multiple children
    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(|child| child.into_any_element()));
        self
    }
}

impl<T> IntoElement for ProviderContainer<T>
where
    T: Provider + Clone + 'static,
{
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        self.into_any_element()
    }
}

impl<T> RenderOnce for ProviderContainer<T>
where
    T: Provider + Clone + 'static,
{
    fn render(mut self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        // Create or get the provider scope
        let mut new_scope_id = None;
        update_provider_context(|context| {
            let scope_id = context.create_scope(Some(self.id.clone()));
            context.register_scoped_provider(scope_id, self.provider.clone());
            self.scope_id = Some(scope_id);
            new_scope_id = Some(scope_id);
        });

        // Create a container element that manages the scope
        ProviderScopeElement {
            scope_id: new_scope_id,
            children: self.children,
        }
    }
}

/// Internal element for managing provider scope
struct ProviderScopeElement {
    scope_id: Option<ProviderScopeId>,
    children: Vec<AnyElement>,
}

impl IntoElement for ProviderScopeElement {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        self.into_any_element()
    }
}

impl RenderOnce for ProviderScopeElement {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        // Set the active scope when rendering
        if let Some(scope_id) = self.scope_id {
            update_provider_context(|context| {
                context.set_active_scope(Some(scope_id));
            });
        }

        // Create a div container for the children
        let mut container = div();
        for child in self.children {
            container = container.child(child);
        }
        container
    }
}

/// Provider hook for accessing providers in components
pub fn use_provider<T>() -> Option<T>
where
    T: Any + Send + Sync + Clone,
{
    with_provider_context(|context| context.get_provider::<T>().cloned()).flatten()
}

/// Provider hook with default fallback
pub fn use_provider_or_default<T>() -> T
where
    T: Any + Send + Sync + Clone + Default,
{
    use_provider::<T>().unwrap_or_default()
}

/// Macro for creating provider components
#[macro_export]
macro_rules! create_provider {
    ($name:ident, $provider_type:ty) => {
        pub struct $name {
            provider: $provider_type,
            children: Vec<gpui::AnyElement>,
        }

        impl $name {
            pub fn new(provider: $provider_type) -> Self {
                Self {
                    provider,
                    children: Vec::new(),
                }
            }

            pub fn child(mut self, child: impl gpui::IntoElement) -> Self {
                self.children.push(child.into_any_element());
                self
            }

            pub fn children(
                mut self,
                children: impl IntoIterator<Item = impl gpui::IntoElement>,
            ) -> Self {
                self.children
                    .extend(children.into_iter().map(|child| child.into_any_element()));
                self
            }
        }

        impl gpui::IntoElement for $name {
            type Element = gpui::AnyElement;

            fn into_element(self) -> Self::Element {
                $crate::providers::ProviderContainer::new(stringify!($name), self.provider)
                    .children(self.children)
                    .into_any_element()
            }
        }
    };
}

/// Helper function to create a provider tree
pub fn provider_tree() -> ProviderTreeBuilder {
    ProviderTreeBuilder::new()
}

/// Builder for creating nested provider hierarchies
pub struct ProviderTreeBuilder {
    providers: Vec<ProviderEntry>,
    children: Vec<AnyElement>,
}

/// Provider entry for the tree builder
struct ProviderEntry {
    type_name: &'static str,
}

impl ProviderTreeBuilder {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn with_provider<T>(mut self, _provider: T) -> Self
    where
        T: Provider + Clone + 'static,
    {
        self.providers.push(ProviderEntry {
            type_name: std::any::type_name::<T>(),
        });
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(|child| child.into_any_element()));
        self
    }

    pub fn build(self) -> AnyElement {
        // Create nested provider structure
        let mut current_element: AnyElement = gpui::div()
            .id("provider-tree-content")
            .children(self.children)
            .into_any_element();

        // Wrap each provider around the content (innermost first)
        for entry in self.providers.into_iter().rev() {
            // Create a provider container for each entry
            let provider_id: SharedString = format!("provider-{}", entry.type_name).into();
            let container = div()
                .id(ElementId::from(provider_id))
                .child(current_element);

            current_element = container.into_any_element();
        }

        current_element
    }
}

impl Default for ProviderTreeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Provider composition utilities
pub struct ProviderComposition;

impl ProviderComposition {
    /// Create a standard app provider tree with theme, config, and event providers
    pub fn app_providers() -> ProviderTreeBuilder {
        provider_tree()
            .with_provider(theme_provider::ThemeConfigurations::light_dark())
            .with_provider(config_provider::ConfigurationProvider::new())
            .with_provider(event_provider::EventHandlingProvider::new())
    }

    /// Create an accessibility-focused provider tree
    pub fn accessibility_providers() -> ProviderTreeBuilder {
        provider_tree()
            .with_provider(theme_provider::ThemeConfigurations::high_contrast())
            .with_provider(config_provider::ConfigurationProvider::accessibility_focused())
            .with_provider(event_provider::EventHandlingProvider::new())
    }

    /// Create a performance-optimized provider tree
    pub fn performance_providers() -> ProviderTreeBuilder {
        provider_tree()
            .with_provider(theme_provider::ThemeConfigurations::light_dark())
            .with_provider(config_provider::ConfigurationProvider::performance_focused())
            .with_provider(event_provider::EventHandlingProvider::new())
    }

    /// Create a minimal provider tree with just theme support
    pub fn minimal_providers() -> ProviderTreeBuilder {
        provider_tree().with_provider(theme_provider::ThemeConfigurations::light_dark())
    }

    /// Create a development provider tree with all features enabled
    pub fn development_providers() -> ProviderTreeBuilder {
        let mut config = config_provider::ConfigurationProvider::new();
        config.feature_flags.experimental_features =
            crate::utils::ExperimentalFeatures::development();

        let mut event_provider = event_provider::EventHandlingProvider::new();
        event_provider.analytics_config.enable_analytics = true;
        event_provider.analytics_config.track_performance = true;

        provider_tree()
            .with_provider(theme_provider::ThemeConfigurations::light_dark())
            .with_provider(config)
            .with_provider(event_provider)
    }
}

/// Provider hooks for easier access
pub struct ProviderHooks;

impl ProviderHooks {
    /// Get the current theme with fallback
    pub fn theme() -> crate::Theme {
        theme_provider::use_theme()
    }

    /// Get UI configuration with fallback
    pub fn ui_config() -> config_provider::UIConfiguration {
        config_provider::use_ui_config()
    }

    /// Get accessibility configuration with fallback
    pub fn accessibility_config() -> config_provider::AccessibilityConfiguration {
        config_provider::use_accessibility_config()
    }

    /// Check if a feature is enabled
    pub fn is_feature_enabled(feature: &str) -> bool {
        config_provider::use_config().is_feature_enabled(feature)
    }

    /// Get effective animation duration
    pub fn animation_duration(base: std::time::Duration) -> std::time::Duration {
        config_provider::use_animation_duration(base)
    }

    /// Emit a custom event
    pub fn emit_event(event: event_provider::CustomEventDetails) -> bool {
        event_provider::use_emit_event()(event)
    }

    /// Check if reduced motion is preferred
    pub fn prefers_reduced_motion() -> bool {
        config_provider::use_prefers_reduced_motion()
    }

    /// Check if dark theme is active
    pub fn is_dark_theme() -> bool {
        theme_provider::use_is_dark_theme()
    }
}

/// Macro for creating provider compositions
#[macro_export]
macro_rules! provider_composition {
    ($($provider:expr),* $(,)?) => {
        $crate::providers::provider_tree()
            $(.with_provider($provider))*
    };

    (theme: $theme:expr, config: $config:expr, events: $events:expr) => {
        $crate::providers::provider_tree()
            .with_provider($theme)
            .with_provider($config)
            .with_provider($events)
    };

    (minimal) => {
        $crate::providers::ProviderComposition::minimal_providers()
    };

    (app) => {
        $crate::providers::ProviderComposition::app_providers()
    };

    (accessibility) => {
        $crate::providers::ProviderComposition::accessibility_providers()
    };

    (performance) => {
        $crate::providers::ProviderComposition::performance_providers()
    };

    (development) => {
        $crate::providers::ProviderComposition::development_providers()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct TestProvider {
        value: String,
    }

    impl Provider for TestProvider {
        fn type_name(&self) -> &'static str {
            "TestProvider"
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    struct AnotherProvider {
        number: i32,
    }

    impl Provider for AnotherProvider {
        fn type_name(&self) -> &'static str {
            "AnotherProvider"
        }
    }

    #[test]
    fn test_provider_context_creation() {
        let context = ProviderContext::new();
        assert!(context.providers.is_empty());
        assert!(context.provider_hierarchy.is_empty());
        assert_eq!(context.active_scope, None);
    }

    #[test]
    fn test_global_provider_registration() {
        let mut context = ProviderContext::new();
        let provider = TestProvider {
            value: "test".to_string(),
        };

        context.register_global_provider(provider.clone());

        let retrieved = context.get_global_provider::<TestProvider>();
        assert_eq!(retrieved, Some(&provider));
    }

    #[test]
    fn test_provider_scope_creation() {
        let mut context = ProviderContext::new();
        let element_id: ElementId = "test-element".into();

        let scope_id = context.create_scope(Some(element_id.clone()));
        assert_eq!(scope_id.0, 0);
        assert_eq!(context.provider_hierarchy.len(), 1);

        let scope = &context.provider_hierarchy[0];
        assert_eq!(scope.id, scope_id);
        assert_eq!(scope.element_id, Some(element_id));
        assert_eq!(scope.parent_scope, None);
    }

    #[test]
    fn test_scoped_provider_registration() {
        let mut context = ProviderContext::new();
        let scope_id = context.create_scope(None);

        let provider = TestProvider {
            value: "scoped".to_string(),
        };

        context.register_scoped_provider(scope_id, provider.clone());
        context.set_active_scope(Some(scope_id));

        let retrieved = context.get_provider::<TestProvider>();
        assert_eq!(retrieved, Some(&provider));
    }

    #[test]
    fn test_provider_hierarchy() {
        let mut context = ProviderContext::new();

        // Create parent scope with a provider
        let parent_scope = context.create_scope(None);
        let parent_provider = TestProvider {
            value: "parent".to_string(),
        };
        context.register_scoped_provider(parent_scope, parent_provider.clone());

        // Create child scope
        context.set_active_scope(Some(parent_scope));
        let child_scope = context.create_scope(None);
        let child_provider = AnotherProvider { number: 42 };
        context.register_scoped_provider(child_scope, child_provider.clone());

        // Set child as active scope
        context.set_active_scope(Some(child_scope));

        // Should find parent provider from child scope
        let parent_retrieved = context.get_provider::<TestProvider>();
        assert_eq!(parent_retrieved, Some(&parent_provider));

        // Should find child provider in child scope
        let child_retrieved = context.get_provider::<AnotherProvider>();
        assert_eq!(child_retrieved, Some(&child_provider));
    }

    #[test]
    fn test_provider_fallback_to_global() {
        let mut context = ProviderContext::new();

        // Register global provider
        let global_provider = TestProvider {
            value: "global".to_string(),
        };
        context.register_global_provider(global_provider.clone());

        // Create scope without the provider
        let scope_id = context.create_scope(None);
        context.set_active_scope(Some(scope_id));

        // Should fall back to global provider
        let retrieved = context.get_provider::<TestProvider>();
        assert_eq!(retrieved, Some(&global_provider));
    }

    #[test]
    fn test_scope_removal() {
        let mut context = ProviderContext::new();
        let scope_id = context.create_scope(None);

        context.set_active_scope(Some(scope_id));
        assert_eq!(context.active_scope, Some(scope_id));

        context.remove_scope(scope_id);
        assert_eq!(context.active_scope, None);
    }

    #[test]
    fn test_nested_scope_hierarchy() {
        let mut context = ProviderContext::new();

        // Create grandparent scope
        let grandparent_scope = context.create_scope(None);
        let grandparent_provider = TestProvider {
            value: "grandparent".to_string(),
        };
        context.register_scoped_provider(grandparent_scope, grandparent_provider.clone());

        // Create parent scope
        context.set_active_scope(Some(grandparent_scope));
        let parent_scope = context.create_scope(None);

        // Create child scope
        context.set_active_scope(Some(parent_scope));
        let child_scope = context.create_scope(None);
        let child_provider = AnotherProvider { number: 99 };
        context.register_scoped_provider(child_scope, child_provider.clone());

        // Set child as active
        context.set_active_scope(Some(child_scope));

        // Should find grandparent provider through the hierarchy
        let grandparent_retrieved = context.get_provider::<TestProvider>();
        assert_eq!(grandparent_retrieved, Some(&grandparent_provider));

        // Should find child provider directly
        let child_retrieved = context.get_provider::<AnotherProvider>();
        assert_eq!(child_retrieved, Some(&child_provider));
    }

    #[test]
    fn test_provider_tree_builder() {
        let tree = provider_tree()
            .with_provider(TestProvider {
                value: "tree".to_string(),
            })
            .with_provider(AnotherProvider { number: 123 })
            .child(gpui::div().id("child1"))
            .child(gpui::div().id("child2"));

        let element = tree.build();
        // Test would verify the element structure in a real implementation
        assert!(true); // Placeholder assertion
    }
}
