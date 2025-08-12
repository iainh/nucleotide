// ABOUTME: Configuration data types
// ABOUTME: Pure data structures for application configuration

use serde::{Deserialize, Serialize};

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

#[cfg(feature = "gpui-bridge")]
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
    #[serde(default = "default_font_size")]
    pub size: f32,
    /// Line height multiplier (e.g., 1.5 for 150% line height)
    #[serde(default = "default_line_height")]
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: default_font_family(),
            weight: FontWeight::default(),
            size: default_font_size(),
            line_height: default_line_height(),
        }
    }
}

/// Default font size in pixels
fn default_font_size() -> f32 {
    14.0
}

/// Default line height multiplier
fn default_line_height() -> f32 {
    1.5
}

/// Default font family
fn default_font_family() -> String {
    if cfg!(target_os = "macos") {
        "SF Mono".to_string()
    } else if cfg!(target_os = "windows") {
        "Consolas".to_string()
    } else {
        "monospace".to_string()
    }
}
