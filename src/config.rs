// ABOUTME: This file implements the GUI-specific configuration system for nucleotide
// ABOUTME: It loads nucleotide.toml and falls back to config.toml for unspecified values

use serde::{Deserialize, Serialize};
use std::path::Path;
use helix_loader::config_dir;
use helix_term::config::Config as HelixConfig;

/// Font weight enumeration matching common font weights
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum FontWeight {
    Thin,
    ExtraLight,
    Light,
    #[default]
    Normal,
    Medium,
    SemiBold,
    Bold,
    ExtraBold,
    Black,
}


impl From<FontWeight> for gpui::FontWeight {
    fn from(weight: FontWeight) -> Self {
        match weight {
            FontWeight::Thin => gpui::FontWeight::THIN,
            FontWeight::ExtraLight => gpui::FontWeight::EXTRA_LIGHT,
            FontWeight::Light => gpui::FontWeight::LIGHT,
            FontWeight::Normal => gpui::FontWeight::NORMAL,
            FontWeight::Medium => gpui::FontWeight::MEDIUM,
            FontWeight::SemiBold => gpui::FontWeight::SEMIBOLD,
            FontWeight::Bold => gpui::FontWeight::BOLD,
            FontWeight::ExtraBold => gpui::FontWeight::EXTRA_BOLD,
            FontWeight::Black => gpui::FontWeight::BLACK,
        }
    }
}

/// Font configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontConfig {
    /// Font family name
    pub family: String,
    /// Font weight
    #[serde(default)]
    pub weight: FontWeight,
    /// Font size in pixels
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "SF Mono".to_string(),
            weight: FontWeight::Normal,
            size: 14.0,
        }
    }
}

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

/// GUI-specific configuration that extends Helix configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuiConfig {
    /// UI-specific settings
    #[serde(default)]
    pub ui: UiConfig,
    
    /// Editor GUI settings
    #[serde(default)]
    pub editor: EditorGuiConfig,
}

/// Combined configuration merging GUI and Helix configs
#[derive(Debug, Clone)]
pub struct Config {
    /// Base Helix configuration
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
            }
        })
    }
}

/// Load Helix configuration from config.toml
fn load_helix_config(_dir: &Path) -> anyhow::Result<HelixConfig> {
    use helix_term::config::{Config, ConfigLoadError};
    
    match Config::load_default() {
        Ok(config) => Ok(config),
        Err(ConfigLoadError::Error(err)) 
            if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(Config::default())
        }
        Err(ConfigLoadError::Error(err)) => Err(err.into()),
        Err(ConfigLoadError::BadConfig(err)) => Err(err.into()),
    }
}

/// Load GUI configuration from nucleotide.toml
fn load_gui_config(dir: &Path) -> anyhow::Result<GuiConfig> {
    let gui_config_path = dir.join("nucleotide.toml");
    
    if gui_config_path.exists() {
        let config_str = std::fs::read_to_string(&gui_config_path)?;
        let config: GuiConfig = toml::from_str(&config_str)?;
        Ok(config)
    } else {
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
mod tests {
    use super::*;
    
    #[test]
    fn test_font_weight_serialization() {
        // Test deserialization from JSON (since TOML doesn't support bare enum values)
        let deserialized: FontWeight = serde_json::from_str("\"semibold\"").expect("Failed to deserialize FontWeight");
        assert_eq!(deserialized, FontWeight::SemiBold);
        
        let deserialized: FontWeight = serde_json::from_str("\"bold\"").expect("Failed to deserialize FontWeight");
        assert_eq!(deserialized, FontWeight::Bold);
        
        // Test that FontWeight converts correctly to gpui::FontWeight
        assert_eq!(gpui::FontWeight::from(FontWeight::Bold), gpui::FontWeight::BOLD);
        assert_eq!(gpui::FontWeight::from(FontWeight::Normal), gpui::FontWeight::NORMAL);
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
        
        let editor_font = config.editor.font.as_ref().expect("Editor font should be set");
        assert_eq!(editor_font.family, "JetBrains Mono");
        assert_eq!(editor_font.weight, FontWeight::Normal);
        assert_eq!(editor_font.size, 14.5);
    }
}