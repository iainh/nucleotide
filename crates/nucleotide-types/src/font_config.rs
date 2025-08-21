// ABOUTME: Font configuration types
// ABOUTME: Pure data structures for font settings

use crate::config::FontWeight;
use serde::{Deserialize, Serialize};

#[cfg(feature = "gpui-bridge")]
use gpui::FontFeatures;

/// Font descriptor - lightweight representation of a font
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Font {
    pub family: String,
    pub weight: FontWeight,
    pub style: FontStyle,
}

/// Font style
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FontStyle {
    Normal,
    Italic,
    Oblique,
}

/// Font settings for the application
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FontSettings {
    pub fixed_font: Font,
    pub var_font: Font,
}

#[cfg(feature = "gpui-bridge")]
impl gpui::Global for FontSettings {}

/// UI font configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiFontConfig {
    pub family: String,
    pub size: f32,
    pub weight: FontWeight,
}

#[cfg(feature = "gpui-bridge")]
impl gpui::Global for UiFontConfig {}

/// Editor font configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditorFontConfig {
    pub family: String,
    pub size: f32,
    pub weight: FontWeight,
    pub line_height: f32,
}

#[cfg(feature = "gpui-bridge")]
impl gpui::Global for EditorFontConfig {}

#[cfg(feature = "gpui-bridge")]
impl From<Font> for gpui::Font {
    fn from(font: Font) -> Self {
        gpui::Font {
            family: font.family.into(),
            weight: font.weight.into(),
            style: match font.style {
                FontStyle::Normal => gpui::FontStyle::Normal,
                FontStyle::Italic => gpui::FontStyle::Italic,
                FontStyle::Oblique => gpui::FontStyle::Oblique,
            },
            #[cfg(feature = "gpui-bridge")]
            features: FontFeatures::default(),
            #[cfg(not(feature = "gpui-bridge"))]
            features: Default::default(),
            fallbacks: None,
        }
    }
}
