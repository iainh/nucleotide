// ABOUTME: Advanced theme system with runtime switching, validation, and dynamic theme creation
// ABOUTME: Builds on the provider system to offer comprehensive theme management capabilities

use crate::Theme;
use gpui::{Hsla, SharedString};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub mod helix_bridge;
pub mod runtime_switcher;
pub mod theme_animator;
pub mod theme_builder;
pub mod theme_validator;

pub use helix_bridge::*;
pub use runtime_switcher::*;
pub use theme_animator::*;
pub use theme_builder::*;
pub use theme_validator::*;

/// Advanced theme manager with runtime capabilities
pub struct AdvancedThemeManager {
    /// Current active theme
    current_theme: Arc<RwLock<Theme>>,
    /// Theme registry for all available themes
    theme_registry: Arc<RwLock<ThemeRegistry>>,
    /// Theme validator for ensuring theme completeness
    validator: ThemeValidator,
    /// Theme animator for smooth transitions
    animator: ThemeAnimator,
    /// Helix theme bridge for compatibility
    helix_bridge: HelixThemeBridge,
    /// Runtime switcher for hot-swapping themes
    runtime_switcher: RuntimeThemeSwitcher,
    /// Theme event listeners
    event_listeners: Arc<RwLock<Vec<ThemeEventListener>>>,
}

/// Theme registry for managing all available themes
#[derive(Debug, Default)]
pub struct ThemeRegistry {
    /// Registered themes by name
    themes: HashMap<SharedString, Theme>,
    /// Theme metadata
    metadata: HashMap<SharedString, ThemeMetadata>,
    /// Theme categories for organization
    categories: HashMap<SharedString, Vec<SharedString>>,
    /// Theme dependencies and inheritance
    inheritance: HashMap<SharedString, ThemeInheritance>,
}

/// Metadata for a theme
#[derive(Debug, Clone)]
pub struct ThemeMetadata {
    /// Theme name
    pub name: SharedString,
    /// Human-readable display name
    pub display_name: SharedString,
    /// Theme description
    pub description: Option<SharedString>,
    /// Theme author
    pub author: Option<SharedString>,
    /// Theme version
    pub version: String,
    /// Whether this is a dark theme
    pub is_dark: bool,
    /// Theme category
    pub category: ThemeCategory,
    /// Theme tags for filtering
    pub tags: Vec<SharedString>,
    /// Creation timestamp
    pub created_at: std::time::SystemTime,
    /// Last modified timestamp
    pub modified_at: std::time::SystemTime,
}

/// Theme categories for organization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeCategory {
    /// Built-in system themes
    System,
    /// User-created themes
    Custom,
    /// Community-contributed themes
    Community,
    /// Themes imported from other editors
    Imported,
    /// High contrast accessibility themes
    Accessibility,
    /// Experimental or beta themes
    Experimental,
}

/// Theme inheritance configuration
#[derive(Debug, Clone)]
pub struct ThemeInheritance {
    /// Parent theme to inherit from
    pub parent_theme: Option<SharedString>,
    /// Specific overrides applied to parent
    pub overrides: ThemeOverrides,
    /// Whether to allow further inheritance
    pub allow_inheritance: bool,
}

/// Theme overrides for customization
#[derive(Debug, Clone, Default)]
pub struct ThemeOverrides {
    /// Color overrides
    pub colors: HashMap<String, Hsla>,
    /// Size/spacing overrides
    pub sizes: HashMap<String, gpui::Pixels>,
    /// Typography overrides
    pub typography: TypographyOverrides,
    /// Animation overrides
    pub animations: AnimationOverrides,
}

/// Typography override settings
#[derive(Debug, Clone, Default)]
pub struct TypographyOverrides {
    /// Font family override
    pub font_family: Option<SharedString>,
    /// Font size scale factor
    pub font_scale: Option<f32>,
    /// Line height multiplier
    pub line_height: Option<f32>,
    /// Letter spacing adjustment
    pub letter_spacing: Option<gpui::Pixels>,
    /// Font weight overrides
    pub font_weights: HashMap<String, u16>,
}

/// Animation override settings
#[derive(Debug, Clone, Default)]
pub struct AnimationOverrides {
    /// Global animation enable/disable
    pub enable_animations: Option<bool>,
    /// Animation duration multiplier
    pub duration_scale: Option<f32>,
    /// Custom easing functions
    pub easing_overrides: HashMap<String, EasingFunction>,
}

/// Easing function definitions
#[derive(Debug, Clone)]
pub enum EasingFunction {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Cubic { x1: f32, y1: f32, x2: f32, y2: f32 },
    Spring { tension: f32, friction: f32 },
}

/// Theme event listener for notifications
pub type ThemeEventListener = Arc<dyn Fn(ThemeEvent) + Send + Sync>;

/// Theme events for notifications and reactions
#[derive(Debug, Clone)]
pub enum ThemeEvent {
    /// Theme was changed
    ThemeChanged {
        from: Option<SharedString>,
        to: SharedString,
        animation_duration: Duration,
    },
    /// Theme was registered
    ThemeRegistered {
        name: SharedString,
        metadata: ThemeMetadata,
    },
    /// Theme was unregistered
    ThemeUnregistered { name: SharedString },
    /// Theme validation completed
    ThemeValidated {
        name: SharedString,
        result: ValidationResult,
    },
    /// Theme import completed
    ThemeImported {
        source: ThemeSource,
        name: SharedString,
        success: bool,
    },
}

/// Theme source for imports
#[derive(Debug, Clone)]
pub enum ThemeSource {
    /// Imported from a file
    File(String),
    /// Imported from Helix
    Helix(String),
    /// Created by user
    UserCreated,
    /// Downloaded from repository
    Repository(String),
}

impl AdvancedThemeManager {
    /// Create a new advanced theme manager
    pub fn new() -> Self {
        Self {
            current_theme: Arc::new(RwLock::new(Theme::dark())),
            theme_registry: Arc::new(RwLock::new(ThemeRegistry::default())),
            validator: ThemeValidator::new(),
            animator: ThemeAnimator::new(),
            helix_bridge: HelixThemeBridge::new(),
            runtime_switcher: RuntimeThemeSwitcher::new(),
            event_listeners: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Initialize with default themes
    pub fn with_default_themes(mut self) -> Self {
        self.register_default_themes();
        self
    }

    /// Register default system themes
    fn register_default_themes(&mut self) {
        // Register built-in themes
        let themes = vec![
            ("dark", Theme::dark(), "Default Dark Theme"),
            ("light", Theme::light(), "Default Light Theme"),
        ];

        for (name, theme, description) in themes {
            let metadata = ThemeMetadata {
                name: name.into(),
                display_name: description.into(),
                description: Some(description.into()),
                author: Some("Nucleotide".into()),
                version: "1.0.0".to_string(),
                is_dark: theme.is_dark(),
                category: ThemeCategory::System,
                tags: vec!["default".into(), "system".into()],
                created_at: std::time::SystemTime::now(),
                modified_at: std::time::SystemTime::now(),
            };

            if let Err(e) = self.register_theme(name.into(), theme, metadata) {
                nucleotide_logging::error!(
                    theme_name = name,
                    error = %e,
                    "Failed to register default theme"
                );
            }
        }
    }

    /// Register a new theme
    pub fn register_theme(
        &mut self,
        name: SharedString,
        theme: Theme,
        metadata: ThemeMetadata,
    ) -> Result<(), ThemeError> {
        // Validate theme before registration
        let validation_result = self.validator.validate_theme(&theme, &metadata)?;
        if !validation_result.is_valid() {
            return Err(ThemeError::ValidationFailed(validation_result));
        }

        // Register in theme registry
        if let Ok(mut registry) = self.theme_registry.write() {
            registry.themes.insert(name.clone(), theme);
            registry.metadata.insert(name.clone(), metadata.clone());

            // Add to category
            let category_name: SharedString =
                format!("{:?}", metadata.category).to_lowercase().into();
            registry
                .categories
                .entry(category_name)
                .or_insert_with(Vec::new)
                .push(name.clone());
        }

        // Emit event
        self.emit_event(ThemeEvent::ThemeRegistered {
            name: name.clone(),
            metadata,
        });
        self.emit_event(ThemeEvent::ThemeValidated {
            name: name.clone(),
            result: validation_result,
        });

        nucleotide_logging::info!(
            theme_name = %name,
            "Theme registered successfully"
        );

        Ok(())
    }

    /// Unregister a theme
    pub fn unregister_theme(&mut self, name: &str) -> Result<Theme, ThemeError> {
        if let Ok(mut registry) = self.theme_registry.write() {
            let theme = registry
                .themes
                .remove(name)
                .ok_or_else(|| ThemeError::ThemeNotFound(name.to_string()))?;

            // Remove metadata
            if let Some(metadata) = registry.metadata.remove(name) {
                // Remove from category
                let category_name: SharedString =
                    format!("{:?}", metadata.category).to_lowercase().into();
                if let Some(category_themes) = registry.categories.get_mut(&category_name) {
                    category_themes.retain(|n| n.as_ref() != name);
                }
            }

            // Remove inheritance references
            registry.inheritance.remove(name);

            self.emit_event(ThemeEvent::ThemeUnregistered {
                name: name.to_string().into(),
            });

            nucleotide_logging::info!(theme_name = name, "Theme unregistered");

            Ok(theme)
        } else {
            Err(ThemeError::LockError(
                "Failed to acquire registry lock".into(),
            ))
        }
    }

    /// Switch to a different theme with optional animation
    pub fn switch_theme(
        &mut self,
        name: &str,
        animate: bool,
        duration: Option<Duration>,
    ) -> Result<(), ThemeError> {
        let new_theme = {
            if let Ok(registry) = self.theme_registry.read() {
                registry
                    .themes
                    .get(name)
                    .cloned()
                    .ok_or_else(|| ThemeError::ThemeNotFound(name.to_string()))?
            } else {
                return Err(ThemeError::LockError(
                    "Failed to acquire registry lock".into(),
                ));
            }
        };

        let old_theme_name = self.get_current_theme_name();
        let animation_duration = if animate {
            duration.unwrap_or_else(|| Duration::from_millis(300))
        } else {
            Duration::ZERO
        };

        if animate && animation_duration > Duration::ZERO {
            // Use animator for smooth transition
            self.animator.animate_theme_transition(
                self.current_theme.clone(),
                new_theme,
                animation_duration,
            )?;
        } else {
            // Immediate switch
            if let Ok(mut current) = self.current_theme.write() {
                *current = new_theme;
            } else {
                return Err(ThemeError::LockError("Failed to acquire theme lock".into()));
            }
        }

        self.emit_event(ThemeEvent::ThemeChanged {
            from: old_theme_name.clone(),
            to: name.to_string().into(),
            animation_duration,
        });

        nucleotide_logging::info!(
            from_theme = ?old_theme_name,
            to_theme = name,
            animated = animate,
            duration_ms = animation_duration.as_millis(),
            "Theme switched"
        );

        Ok(())
    }

    /// Get the current theme
    pub fn get_current_theme(&self) -> Result<Theme, ThemeError> {
        self.current_theme
            .read()
            .map(|theme| theme.clone())
            .map_err(|_| ThemeError::LockError("Failed to acquire theme lock".into()))
    }

    /// Get the current theme name
    pub fn get_current_theme_name(&self) -> Option<SharedString> {
        if let Ok(current_theme) = self.current_theme.read()
            && let Ok(registry) = self.theme_registry.read()
        {
            // Find theme name by comparing themes
            for (name, theme) in &registry.themes {
                // Simple comparison - in a real implementation you might want
                // to store the current theme name separately
                if theme.is_dark() == current_theme.is_dark() {
                    return Some(name.clone());
                }
            }
        }
        None
    }

    /// List all available themes
    pub fn list_themes(&self) -> Result<Vec<(SharedString, ThemeMetadata)>, ThemeError> {
        if let Ok(registry) = self.theme_registry.read() {
            let mut themes = Vec::new();
            for (name, metadata) in &registry.metadata {
                themes.push((name.clone(), metadata.clone()));
            }
            themes.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(themes)
        } else {
            Err(ThemeError::LockError(
                "Failed to acquire registry lock".into(),
            ))
        }
    }

    /// List themes by category
    pub fn list_themes_by_category(
        &self,
        category: ThemeCategory,
    ) -> Result<Vec<SharedString>, ThemeError> {
        if let Ok(registry) = self.theme_registry.read() {
            let category_name: SharedString = format!("{:?}", category).to_lowercase().into();
            Ok(registry
                .categories
                .get(&category_name)
                .cloned()
                .unwrap_or_default())
        } else {
            Err(ThemeError::LockError(
                "Failed to acquire registry lock".into(),
            ))
        }
    }

    /// Search themes by tags or metadata
    pub fn search_themes(&self, query: &str) -> Result<Vec<SharedString>, ThemeError> {
        if let Ok(registry) = self.theme_registry.read() {
            let query_lower = query.to_lowercase();
            let mut matches = Vec::new();

            for (name, metadata) in &registry.metadata {
                // Search in name, display name, description, and tags
                if name.to_lowercase().contains(&query_lower)
                    || metadata.display_name.to_lowercase().contains(&query_lower)
                    || metadata
                        .description
                        .as_ref()
                        .is_some_and(|d| d.to_lowercase().contains(&query_lower))
                    || metadata
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&query_lower))
                {
                    matches.push(name.clone());
                }
            }

            Ok(matches)
        } else {
            Err(ThemeError::LockError(
                "Failed to acquire registry lock".into(),
            ))
        }
    }

    /// Create a theme from inheritance
    pub fn create_inherited_theme(
        &self,
        parent_name: &str,
        overrides: ThemeOverrides,
        new_name: SharedString,
        _metadata: ThemeMetadata,
    ) -> Result<Theme, ThemeError> {
        let parent_theme = {
            if let Ok(registry) = self.theme_registry.read() {
                registry
                    .themes
                    .get(parent_name)
                    .cloned()
                    .ok_or_else(|| ThemeError::ThemeNotFound(parent_name.to_string()))?
            } else {
                return Err(ThemeError::LockError(
                    "Failed to acquire registry lock".into(),
                ));
            }
        };

        let mut new_theme = parent_theme;

        // Apply color overrides
        for (key, color) in &overrides.colors {
            match key.as_str() {
                "primary" => new_theme.tokens.colors.primary = *color,
                "secondary" => new_theme.tokens.colors.text_secondary = *color,
                "background" => new_theme.tokens.colors.background = *color,
                "surface" => new_theme.tokens.colors.surface = *color,
                "text_primary" => new_theme.tokens.colors.text_primary = *color,
                "text_secondary" => new_theme.tokens.colors.text_secondary = *color,
                "error" => new_theme.tokens.colors.error = *color,
                "warning" => new_theme.tokens.colors.warning = *color,
                "success" => new_theme.tokens.colors.success = *color,
                _ => {
                    nucleotide_logging::debug!(
                        color_key = key,
                        "Unknown color key in theme override"
                    );
                }
            }
        }

        // Apply size overrides
        for (key, size) in &overrides.sizes {
            match key.as_str() {
                "space_1" => new_theme.tokens.sizes.space_1 = *size,
                "space_2" => new_theme.tokens.sizes.space_2 = *size,
                "space_3" => new_theme.tokens.sizes.space_3 = *size,
                "space_4" => new_theme.tokens.sizes.space_4 = *size,
                "radius_sm" => new_theme.tokens.sizes.radius_sm = *size,
                "radius_md" => new_theme.tokens.sizes.radius_md = *size,
                "radius_lg" => new_theme.tokens.sizes.radius_lg = *size,
                _ => {
                    nucleotide_logging::debug!(
                        size_key = key,
                        "Unknown size key in theme override"
                    );
                }
            }
        }

        // Rebuild legacy fields from tokens
        new_theme = Theme::from_tokens(new_theme.tokens);

        // Store inheritance information
        if let Ok(mut registry) = self.theme_registry.write() {
            registry.inheritance.insert(
                new_name.clone(),
                ThemeInheritance {
                    parent_theme: Some(parent_name.to_string().into()),
                    overrides,
                    allow_inheritance: true,
                },
            );
        }

        nucleotide_logging::info!(
            new_theme = %new_name,
            parent_theme = parent_name,
            "Created inherited theme"
        );

        Ok(new_theme)
    }

    /// Add a theme event listener
    pub fn add_event_listener(&mut self, listener: ThemeEventListener) {
        if let Ok(mut listeners) = self.event_listeners.write() {
            listeners.push(listener);
        }
    }

    /// Emit a theme event to all listeners
    fn emit_event(&self, event: ThemeEvent) {
        if let Ok(listeners) = self.event_listeners.read() {
            for listener in listeners.iter() {
                listener(event.clone());
            }
        }
    }

    /// Get theme validator
    pub fn validator(&self) -> &ThemeValidator {
        &self.validator
    }

    /// Get theme animator
    pub fn animator(&self) -> &ThemeAnimator {
        &self.animator
    }

    /// Get Helix bridge
    pub fn helix_bridge(&self) -> &HelixThemeBridge {
        &self.helix_bridge
    }

    /// Get runtime switcher
    pub fn runtime_switcher(&self) -> &RuntimeThemeSwitcher {
        &self.runtime_switcher
    }
}

/// Theme-related errors
#[derive(Debug, Clone)]
pub enum ThemeError {
    /// Theme not found
    ThemeNotFound(String),
    /// Theme validation failed
    ValidationFailed(ValidationResult),
    /// Lock acquisition failed
    LockError(String),
    /// Animation error
    AnimationError(String),
    /// Import/export error
    ImportError(String),
    /// General theme operation error
    OperationError(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::ThemeNotFound(name) => write!(f, "Theme not found: {}", name),
            ThemeError::ValidationFailed(result) => {
                write!(f, "Theme validation failed: {:?}", result)
            }
            ThemeError::LockError(msg) => write!(f, "Lock error: {}", msg),
            ThemeError::AnimationError(msg) => write!(f, "Animation error: {}", msg),
            ThemeError::ImportError(msg) => write!(f, "Import error: {}", msg),
            ThemeError::OperationError(msg) => write!(f, "Operation error: {}", msg),
        }
    }
}

impl std::error::Error for ThemeError {}

impl From<ValidationError> for ThemeError {
    fn from(error: ValidationError) -> Self {
        ThemeError::OperationError(error.to_string())
    }
}

impl From<AnimationError> for ThemeError {
    fn from(error: AnimationError) -> Self {
        ThemeError::AnimationError(error.to_string())
    }
}

impl Default for AdvancedThemeManager {
    fn default() -> Self {
        Self::new().with_default_themes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_advanced_theme_manager_creation() {
        let manager = AdvancedThemeManager::new();
        assert!(manager.get_current_theme().is_ok());
    }

    #[test]
    fn test_theme_registration() {
        let mut manager = AdvancedThemeManager::new();

        let theme = Theme::light();
        let metadata = ThemeMetadata {
            name: "test-theme".into(),
            display_name: "Test Theme".into(),
            description: Some("A test theme".into()),
            author: Some("Test Author".into()),
            version: "1.0.0".to_string(),
            is_dark: false,
            category: ThemeCategory::Custom,
            tags: vec!["test".into()],
            created_at: std::time::SystemTime::now(),
            modified_at: std::time::SystemTime::now(),
        };

        let result = manager.register_theme("test-theme".into(), theme, metadata);
        assert!(result.is_ok());

        let themes = manager.list_themes().unwrap();
        assert!(!themes.is_empty());
    }

    #[test]
    fn test_theme_search() {
        let mut manager = AdvancedThemeManager::new().with_default_themes();

        let results = manager.search_themes("dark").unwrap();
        assert!(!results.is_empty());

        let results = manager.search_themes("nonexistent").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_theme_categories() {
        let manager = AdvancedThemeManager::new().with_default_themes();

        let system_themes = manager
            .list_themes_by_category(ThemeCategory::System)
            .unwrap();
        assert!(!system_themes.is_empty());

        let custom_themes = manager
            .list_themes_by_category(ThemeCategory::Custom)
            .unwrap();
        assert!(custom_themes.is_empty()); // No custom themes registered yet
    }

    #[test]
    fn test_theme_inheritance() {
        let manager = AdvancedThemeManager::new().with_default_themes();

        let mut overrides = ThemeOverrides::default();
        overrides.colors.insert(
            "primary".to_string(),
            Hsla {
                h: 200.0,
                s: 0.8,
                l: 0.5,
                a: 1.0,
            },
        );

        let metadata = ThemeMetadata {
            name: "inherited-theme".into(),
            display_name: "Inherited Theme".into(),
            description: Some("Theme inherited from dark".into()),
            author: Some("Test".into()),
            version: "1.0.0".to_string(),
            is_dark: true,
            category: ThemeCategory::Custom,
            tags: vec!["inherited".into()],
            created_at: std::time::SystemTime::now(),
            modified_at: std::time::SystemTime::now(),
        };

        let result =
            manager.create_inherited_theme("dark", overrides, "inherited-theme".into(), metadata);
        assert!(result.is_ok());

        let inherited_theme = result.unwrap();
        assert_eq!(
            inherited_theme.tokens.colors.primary,
            Hsla {
                h: 200.0,
                s: 0.8,
                l: 0.5,
                a: 1.0
            }
        );
    }

    #[test]
    fn test_theme_events() {
        let mut manager = AdvancedThemeManager::new().with_default_themes();

        let events = Arc::new(RwLock::new(Vec::new()));
        let events_clone = events.clone();

        manager.add_event_listener(Arc::new(move |event| {
            if let Ok(mut event_list) = events_clone.write() {
                event_list.push(event);
            }
        }));

        // This should trigger events
        let theme = Theme::light();
        let metadata = ThemeMetadata {
            name: "event-test".into(),
            display_name: "Event Test".into(),
            description: None,
            author: None,
            version: "1.0.0".to_string(),
            is_dark: false,
            category: ThemeCategory::Custom,
            tags: vec![],
            created_at: std::time::SystemTime::now(),
            modified_at: std::time::SystemTime::now(),
        };

        let _ = manager.register_theme("event-test".into(), theme, metadata);

        // Check events were recorded
        {
            let event_list = events.read().unwrap();
            assert!(!event_list.is_empty());
        }
    }
}
