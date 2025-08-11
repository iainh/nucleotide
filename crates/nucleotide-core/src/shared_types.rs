// ABOUTME: Minimal shared types used across crates
// ABOUTME: Only contains simple types with no cross-crate dependencies

use gpui::{FontWeight, Global};
use helix_core::diagnostic::Severity;

// Placeholder type for Core during migration
// TODO: Replace with capability traits
pub type CoreEntity = gpui::Entity<()>;

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

/// Editor status information
#[derive(Debug, Clone)]
pub struct EditorStatus {
    pub status: String,
    pub severity: Severity,
}

/// Completion trigger types
#[derive(Debug, Clone)]
pub enum CompletionTrigger {
    /// Triggered automatically (e.g., after typing '.')
    Automatic,
    /// Triggered manually by user (e.g., Ctrl+Space)
    Manual,
    /// Triggered by a specific character
    Character(char),
}
