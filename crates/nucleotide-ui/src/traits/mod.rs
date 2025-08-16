// ABOUTME: Core trait system providing consistent APIs across all components
// ABOUTME: Defines interfaces for component lifecycle, styling, and interaction patterns

use gpui::{App, ElementId, IntoElement, SharedString, Context, KeyDownEvent};
use crate::{DesignTokens, Theme};

/// Core component trait that all nucleotide-ui components should implement
/// Provides consistent lifecycle and identification patterns
pub trait Component {
    /// Get the component's unique identifier
    fn id(&self) -> &ElementId;
    
    /// Set the component's identifier (builder pattern)
    fn with_id(self, id: impl Into<ElementId>) -> Self
    where
        Self: Sized;
    
    /// Check if the component is in a disabled state
    fn is_disabled(&self) -> bool {
        false
    }
    
    /// Set the disabled state (builder pattern)
    fn disabled(self, disabled: bool) -> Self
    where
        Self: Sized;
}

/// Trait for components that can be styled with design tokens
/// Provides consistent access to theme and styling utilities
pub trait Styled {
    /// Get the current variant/style of the component
    type Variant: Clone + Default;
    
    /// Get the current size of the component  
    type Size: Clone + Default;
    
    /// Get the component's current variant
    fn variant(&self) -> &Self::Variant;
    
    /// Set the component's variant (builder pattern)
    fn with_variant(self, variant: Self::Variant) -> Self
    where
        Self: Sized;
    
    /// Get the component's current size
    fn size(&self) -> &Self::Size;
    
    /// Set the component's size (builder pattern) 
    fn with_size(self, size: Self::Size) -> Self
    where
        Self: Sized;
    
    /// Apply theme-aware styling to the component
    /// This is called during rendering to compute final styles
    fn apply_theme_styling(&self, theme: &Theme) -> ComponentStyles {
        ComponentStyles::from_theme(theme, self.variant(), self.size())
    }
}

/// Trait for components that handle user interactions
/// Provides consistent event handling patterns
pub trait Interactive {
    /// Type for click event handlers
    type ClickHandler: 'static;
    
    /// Set primary click handler (builder pattern)
    fn on_click(self, handler: Self::ClickHandler) -> Self
    where
        Self: Sized;
    
    /// Set secondary click handler (builder pattern)  
    fn on_secondary_click(self, handler: Self::ClickHandler) -> Self
    where
        Self: Sized;
    
    /// Check if the component can receive focus
    fn is_focusable(&self) -> bool {
        true
    }
    
    /// Check if the component is currently focused
    fn is_focused(&self) -> bool {
        false
    }
}

/// Trait for components that can display tooltips
pub trait Tooltipped {
    /// Set tooltip text (builder pattern)
    fn tooltip(self, tooltip: impl Into<SharedString>) -> Self
    where
        Self: Sized;
    
    /// Get the current tooltip text
    fn get_tooltip(&self) -> Option<&SharedString>;
}

/// Trait for components that support composition with child elements
pub trait Composable {
    /// Add a child element (builder pattern)
    fn child(self, child: impl IntoElement) -> Self
    where
        Self: Sized;
    
    /// Add multiple children (builder pattern)
    fn children(self, children: impl IntoIterator<Item = impl IntoElement>) -> Self
    where
        Self: Sized;
}

/// Trait for components that support slot-based composition
pub trait Slotted {
    /// Set the start slot (icon, prefix, etc.)
    fn start_slot(self, slot: impl IntoElement) -> Self
    where
        Self: Sized;
    
    /// Set the end slot (badge, suffix, etc.)
    fn end_slot(self, slot: impl IntoElement) -> Self
    where
        Self: Sized;
}

/// Computed styles for a component based on theme and state
#[derive(Debug, Clone)]
pub struct ComponentStyles {
    pub background: gpui::Hsla,
    pub text_color: gpui::Hsla,
    pub border_color: gpui::Hsla,
    pub padding: gpui::Pixels,
    pub border_radius: gpui::Pixels,
}

impl ComponentStyles {
    /// Create component styles from theme and component properties
    pub fn from_theme<V, S>(theme: &Theme, _variant: &V, _size: &S) -> Self {
        Self {
            background: theme.tokens.colors.surface,
            text_color: theme.tokens.colors.text_primary,
            border_color: theme.tokens.colors.border_default,
            padding: theme.tokens.sizes.space_3,
            border_radius: theme.tokens.sizes.radius_md,
        }
    }
    
    /// Create hover state styles
    pub fn hover_state(&self, theme: &Theme) -> Self {
        Self {
            background: theme.tokens.colors.surface_hover,
            text_color: self.text_color,
            border_color: self.border_color,
            padding: self.padding,
            border_radius: self.border_radius,
        }
    }
    
    /// Create active state styles
    pub fn active_state(&self, theme: &Theme) -> Self {
        Self {
            background: theme.tokens.colors.surface_active,
            text_color: self.text_color,
            border_color: self.border_color,
            padding: self.padding,
            border_radius: self.border_radius,
        }
    }
    
    /// Create disabled state styles
    pub fn disabled_state(&self, theme: &Theme) -> Self {
        Self {
            background: theme.tokens.colors.surface_disabled,
            text_color: theme.tokens.colors.text_disabled,
            border_color: theme.tokens.colors.border_muted,
            padding: self.padding,
            border_radius: self.border_radius,
        }
    }
}

/// Extension trait for easy access to theme and tokens from GPUI contexts
pub trait ThemedContext {
    /// Get the current theme
    fn theme(&self) -> &Theme;
    
    /// Get design tokens
    fn tokens(&self) -> &DesignTokens {
        &self.theme().tokens
    }
    
    /// Check if the current theme is dark
    fn is_dark_theme(&self) -> bool {
        self.theme().is_dark()
    }
}

impl ThemedContext for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

impl<V: 'static> ThemedContext for Context<'_, V> {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

/// Extension trait providing common builder methods for all components
pub trait ComponentBuilder: Sized {
    /// Set component as disabled
    fn disabled(self) -> Self {
        self.with_disabled(true)
    }
    
    /// Set component as enabled 
    fn enabled(self) -> Self {
        self.with_disabled(false)
    }
    
    /// Set disabled state
    fn with_disabled(self, disabled: bool) -> Self;
    
    /// Set component tooltip
    fn with_tooltip(self, tooltip: impl Into<SharedString>) -> Self;
}

/// Trait for components that can be measured and layouted
pub trait Measurable {
    /// Get the component's preferred size
    fn preferred_size(&self, theme: &Theme) -> (gpui::Pixels, gpui::Pixels);
    
    /// Get minimum size constraints
    fn min_size(&self, theme: &Theme) -> (gpui::Pixels, gpui::Pixels) {
        self.preferred_size(theme)
    }
    
    /// Check if the component should grow to fill available space
    fn should_grow(&self) -> bool {
        false
    }
}

/// Trait for components with validation states
pub trait Validatable {
    /// Validation state enumeration
    type ValidationState: Clone + Default;
    
    /// Get the current validation state
    fn validation_state(&self) -> &Self::ValidationState;
    
    /// Set validation state (builder pattern)
    fn with_validation_state(self, state: Self::ValidationState) -> Self
    where
        Self: Sized;
    
    /// Check if the component is in an error state
    fn has_error(&self) -> bool {
        false
    }
    
    /// Get error message if any
    fn error_message(&self) -> Option<&str> {
        None
    }
}

/// Common validation states
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ValidationState {
    #[default]
    Valid,
    Warning(String),
    Error(String),
}

impl ValidationState {
    pub fn is_error(&self) -> bool {
        matches!(self, ValidationState::Error(_))
    }
    
    pub fn is_warning(&self) -> bool {
        matches!(self, ValidationState::Warning(_))
    }
    
    pub fn message(&self) -> Option<&str> {
        match self {
            ValidationState::Valid => None,
            ValidationState::Warning(msg) | ValidationState::Error(msg) => Some(msg),
        }
    }
}

/// Macro to implement basic Component trait for a struct
#[macro_export]
macro_rules! impl_component {
    ($struct_name:ident) => {
        impl $crate::Component for $struct_name {
            fn id(&self) -> &gpui::ElementId {
                &self.id
            }
            
            fn with_id(mut self, id: impl Into<gpui::ElementId>) -> Self {
                self.id = id.into();
                self
            }
            
            fn is_disabled(&self) -> bool {
                self.disabled
            }
            
            fn disabled(mut self, disabled: bool) -> Self {
                self.disabled = disabled;
                self
            }
        }
    };
}

/// Macro to implement Tooltipped trait for a struct
#[macro_export]
macro_rules! impl_tooltipped {
    ($struct_name:ident) => {
        impl $crate::Tooltipped for $struct_name {
            fn tooltip(mut self, tooltip: impl Into<gpui::SharedString>) -> Self {
                self.tooltip = Some(tooltip.into());
                self
            }
            
            fn get_tooltip(&self) -> Option<&gpui::SharedString> {
                self.tooltip.as_ref()
            }
        }
    };
}

/// Helper trait for component creation with consistent patterns
pub trait ComponentFactory {
    /// Create a new component with default styling
    fn new(id: impl Into<ElementId>) -> Self;
    
    /// Create a new component with specified variant
    fn with_variant<V>(id: impl Into<ElementId>, variant: V) -> Self
    where
        Self: Styled<Variant = V> + Sized,
    {
        Self::new(id).with_variant(variant)
    }
    
    /// Create a new component with specified size
    fn with_size<S>(id: impl Into<ElementId>, size: S) -> Self
    where
        Self: Styled<Size = S> + Sized,
    {
        Self::new(id).with_size(size)
    }
}

/// Trait for components that support keyboard navigation
pub trait KeyboardNavigable {
    /// Handle keyboard navigation event
    fn handle_key_event(&mut self, _event: &KeyDownEvent) -> bool {
        false
    }
    
    /// Get the component's tab index for keyboard navigation
    fn tab_index(&self) -> Option<i32> {
        None
    }
    
    /// Check if component should be included in tab navigation
    fn is_tab_navigable(&self) -> bool {
        self.tab_index().is_some()
    }
}

/// Trait for components with loading states
pub trait Loadable {
    /// Check if component is in loading state
    fn is_loading(&self) -> bool {
        false
    }
    
    /// Set loading state (builder pattern)
    fn loading(self, loading: bool) -> Self
    where
        Self: Sized;
    
    /// Get loading message
    fn loading_message(&self) -> Option<&str> {
        None
    }
}

/// Common component states that affect styling
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentState {
    Default,
    Hover,
    Active,
    Focused,
    Disabled,
    Loading,
}

impl ComponentState {
    /// Check if state represents an interactive state
    pub fn is_interactive(&self) -> bool {
        matches!(self, ComponentState::Hover | ComponentState::Active | ComponentState::Focused)
    }
    
    /// Check if state prevents interaction
    pub fn prevents_interaction(&self) -> bool {
        matches!(self, ComponentState::Disabled | ComponentState::Loading)
    }
}

/// Helper function to determine component state from various flags
pub fn compute_component_state(
    disabled: bool,
    loading: bool,
    focused: bool,
    hovered: bool,
    active: bool,
) -> ComponentState {
    if disabled {
        ComponentState::Disabled
    } else if loading {
        ComponentState::Loading
    } else if active {
        ComponentState::Active
    } else if focused {
        ComponentState::Focused
    } else if hovered {
        ComponentState::Hover
    } else {
        ComponentState::Default
    }
}

/// Re-export commonly used types for convenience
pub use ComponentStyles as Styles;
pub use ValidationState as Validation;
pub use ComponentState as State;

#[cfg(test)]
mod tests;