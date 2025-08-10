// ABOUTME: Theme conversion utilities for bridging helix themes to GPUI
// ABOUTME: Centralized location for color conversions and theme extraction

use gpui::Hsla;
use helix_view::graphics::Color;

/// Convert helix Color to GPUI Hsla
pub fn color_to_hsla(color: Color) -> Option<Hsla> {
    crate::utils::color_to_hsla(color)
}
