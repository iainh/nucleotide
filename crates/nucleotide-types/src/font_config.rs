// ABOUTME: Font configuration types
// ABOUTME: Pure data structures for font settings

use gpui::{FontWeight, Global};

/// Font settings for the application
pub struct FontSettings {
    pub fixed_font: gpui::Font,
    pub var_font: gpui::Font,
}

impl Global for FontSettings {}

/// UI font configuration
#[derive(Clone, Debug)]
pub struct UiFontConfig {
    pub family: String,
    pub size: f32,
    pub weight: FontWeight,
}

impl Global for UiFontConfig {}

/// Editor font configuration
#[derive(Clone, Debug)]
pub struct EditorFontConfig {
    pub family: String,
    pub size: f32,
    pub weight: FontWeight,
    pub line_height: f32,
}

impl Global for EditorFontConfig {}
