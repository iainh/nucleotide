// ABOUTME: This file implements the GUI-specific configuration system for nucleotide
// ABOUTME: It loads nucleotide.toml and falls back to config.toml for unspecified values

use helix_loader::config_dir;
use helix_term::config::Config as HelixConfig;
use nucleotide_types::{FontConfig, FontWeight};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Default theme for light mode
pub const DEFAULT_LIGHT_THEME: &str = "nucleotide-outdoors";

/// Default theme for dark mode
pub const DEFAULT_DARK_THEME: &str = "nucleotide-teal";

/// UI-specific configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiConfig {
    /// Font used for UI elements (menus, dialogs, etc.)
    #[serde(default)]
    pub font: Option<FontConfig>,
}

/// Editor-specific GUI configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditorGuiConfig {
    /// Font used in the editor
    #[serde(default)]
    pub font: Option<FontConfig>,
}

/// Theme mode selection
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow system appearance
    #[default]
    System,
    /// Always use light theme
    Light,
    /// Always use dark theme
    Dark,
}

/// Theme configuration for automatic switching
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Theme mode selection
    #[serde(default)]
    pub mode: ThemeMode,

    /// Theme to use in light mode (defaults to "nucleotide-outdoors" if not specified)
    #[serde(default)]
    pub light_theme: Option<String>,

    /// Theme to use in dark mode (defaults to "nucleotide-teal" if not specified)
    #[serde(default)]
    pub dark_theme: Option<String>,
}

impl ThemeConfig {
    /// Get the light theme name with default fallback
    pub fn get_light_theme(&self) -> String {
        self.light_theme
            .clone()
            .unwrap_or_else(|| DEFAULT_LIGHT_THEME.to_string())
    }

    /// Get the dark theme name with default fallback
    pub fn get_dark_theme(&self) -> String {
        self.dark_theme
            .clone()
            .unwrap_or_else(|| DEFAULT_DARK_THEME.to_string())
    }
}

/// Window appearance configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowConfig {
    /// Enable blur for dark themes
    #[serde(default)]
    pub blur_dark_themes: bool,

    /// Automatically adjust window appearance based on theme
    #[serde(default = "default_true")]
    pub appearance_follows_theme: bool,
}

fn default_true() -> bool {
    true
}

/// GUI-specific configuration that extends Helix configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuiConfig {
    /// UI-specific settings
    #[serde(default)]
    pub ui: UiConfig,

    /// Editor GUI settings
    #[serde(default)]
    pub editor: EditorGuiConfig,

    /// Theme configuration
    #[serde(default)]
    pub theme: ThemeConfig,

    /// Window appearance configuration
    #[serde(default)]
    pub window: WindowConfig,
}

/// Combined configuration merging GUI and Helix configs
#[derive(Debug, Clone)]
pub struct Config {
    /// Current Helix configuration (includes both file config and runtime changes)
    pub helix: HelixConfig,

    /// GUI-specific configuration
    pub gui: GuiConfig,
}

impl Config {
    /// Load configuration from the standard locations
    pub fn load() -> anyhow::Result<Self> {
        let config_dir = config_dir();
        Self::load_from_dir(&config_dir)
    }

    /// Load configuration from a specific directory
    pub fn load_from_dir(dir: &Path) -> anyhow::Result<Self> {
        // First, load the base Helix configuration
        let helix_config = load_helix_config(dir)?;

        // Then, load GUI-specific configuration if it exists
        let gui_config = load_gui_config(dir).unwrap_or_default();

        Ok(Self {
            helix: helix_config,
            gui: gui_config,
        })
    }

    /// Apply a config update from Helix (e.g., from toggle command)
    /// We don't need to know what the config keys mean - we just store the new config
    pub fn apply_helix_config_update(&mut self, new_editor_config: &helix_view::editor::Config) {
        self.helix.editor = new_editor_config.clone();
    }

    /// Get the current Helix config
    pub fn to_helix_config(&self) -> HelixConfig {
        self.helix.clone()
    }

    /// Get the editor font configuration
    pub fn editor_font(&self) -> FontConfig {
        self.gui.editor.font.clone().unwrap_or_else(|| {
            // Fall back to UI font if specified
            self.gui.ui.font.clone().unwrap_or_default()
        })
    }

    /// Get the UI font configuration
    pub fn ui_font(&self) -> FontConfig {
        self.gui.ui.font.clone().unwrap_or_else(|| {
            // Default UI font
            FontConfig {
                family: "SF Pro Display".to_string(),
                weight: FontWeight::Normal,
                size: 13.0,
                line_height: 1.5,
            }
        })
    }
}

/// Load Helix configuration from config.toml
fn load_helix_config(_dir: &Path) -> anyhow::Result<HelixConfig> {
    use helix_term::config::{Config, ConfigLoadError};

    match Config::load_default() {
        Ok(config) => Ok(config),
        Err(ConfigLoadError::Error(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(Config::default())
        }
        Err(ConfigLoadError::Error(err)) => Err(err.into()),
        Err(ConfigLoadError::BadConfig(err)) => Err(err.into()),
    }
}

/// Load GUI configuration from nucleotide.toml
fn load_gui_config(dir: &Path) -> anyhow::Result<GuiConfig> {
    let gui_config_path = dir.join("nucleotide.toml");

    nucleotide_logging::info!(
        config_dir = %dir.display(),
        config_path = %gui_config_path.display(),
        config_exists = gui_config_path.exists(),
        "Loading GUI configuration"
    );

    if gui_config_path.exists() {
        let config_str = std::fs::read_to_string(&gui_config_path)?;
        let config: GuiConfig = toml::from_str(&config_str)?;
        nucleotide_logging::info!(
            theme_mode = ?config.theme.mode,
            light_theme = %config.theme.get_light_theme(),
            dark_theme = %config.theme.get_dark_theme(),
            "Loaded GUI configuration"
        );
        Ok(config)
    } else {
        nucleotide_logging::info!("No GUI configuration file found, using defaults");
        // Return default GUI configuration if file doesn't exist
        Ok(GuiConfig::default())
    }
}

/// Example nucleotide.toml configuration:
/// ```toml
/// [ui]
/// [ui.font]
/// family = "SF Pro Display"
/// weight = "normal"
/// size = 13.0
///
/// [editor]
/// [editor.font]
/// family = "SF Mono"
/// weight = "medium"
/// size = 14.0
/// ```
#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;

    #[test]
    fn test_font_weight_serialization() {
        // Test deserialization from JSON (since TOML doesn't support bare enum values)
        let deserialized: FontWeight =
            serde_json::from_str("\"semibold\"").expect("Failed to deserialize FontWeight");
        assert_eq!(deserialized, FontWeight::SemiBold);

        let deserialized: FontWeight =
            serde_json::from_str("\"bold\"").expect("Failed to deserialize FontWeight");
        assert_eq!(deserialized, FontWeight::Bold);

        // Test that FontWeight converts correctly to gpui::FontWeight
        assert_eq!(
            gpui::FontWeight::from(FontWeight::Bold),
            gpui::FontWeight::BOLD
        );
        assert_eq!(
            gpui::FontWeight::from(FontWeight::Normal),
            gpui::FontWeight::NORMAL
        );
    }

    #[test]
    fn test_gui_config_parsing() {
        let config_str = r#"
[ui.font]
family = "Inter"
weight = "medium"
size = 13.0

[editor.font]
family = "JetBrains Mono"
weight = "normal"
size = 14.5
"#;

        let config: GuiConfig = toml::from_str(config_str).expect("Failed to parse GuiConfig");

        let ui_font = config.ui.font.as_ref().expect("UI font should be set");
        assert_eq!(ui_font.family, "Inter");
        assert_eq!(ui_font.weight, FontWeight::Medium);
        assert_eq!(ui_font.size, 13.0);

        let editor_font = config
            .editor
            .font
            .as_ref()
            .expect("Editor font should be set");
        assert_eq!(editor_font.family, "JetBrains Mono");
        assert_eq!(editor_font.weight, FontWeight::Normal);
        assert_eq!(editor_font.size, 14.5);
    }
}
