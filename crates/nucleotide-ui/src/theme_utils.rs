// ABOUTME: Theme conversion utilities for bridging helix themes to GPUI
// ABOUTME: Centralized location for color conversions and theme extraction

use gpui::Hsla;
use helix_view::graphics::Color;

/// Convert helix Color to GPUI Hsla
pub fn color_to_hsla(color: Color) -> Option<Hsla> {
    use gpui::Rgba;

    match color {
        Color::Rgb(r, g, b) => {
            let rgba = Rgba {
                r: f32::from(r) / 255.0,
                g: f32::from(g) / 255.0,
                b: f32::from(b) / 255.0,
                a: 1.0,
            };
            Some(Hsla::from(rgba))
        }
        Color::Indexed(index) => {
            // Convert ANSI color index to RGB approximation
            let (r, g, b) = match index {
                0 => (0, 0, 0),        // Black
                1 => (128, 0, 0),      // Red
                2 => (0, 128, 0),      // Green
                3 => (128, 128, 0),    // Yellow
                4 => (0, 0, 128),      // Blue
                5 => (128, 0, 128),    // Magenta
                6 => (0, 128, 128),    // Cyan
                7 => (192, 192, 192),  // White
                8 => (128, 128, 128),  // Bright Black
                9 => (255, 0, 0),      // Bright Red
                10 => (0, 255, 0),     // Bright Green
                11 => (255, 255, 0),   // Bright Yellow
                12 => (0, 0, 255),     // Bright Blue
                13 => (255, 0, 255),   // Bright Magenta
                14 => (0, 255, 255),   // Bright Cyan
                15 => (255, 255, 255), // Bright White
                _ => (128, 128, 128),  // Default gray for other indices
            };
            let rgba = Rgba {
                r: r as f32 / 255.0,
                g: g as f32 / 255.0,
                b: b as f32 / 255.0,
                a: 1.0,
            };
            Some(Hsla::from(rgba))
        }
        // Named color variants - handle all with default mappings
        Color::Black => Some(Hsla::from(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })),
        Color::Red => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })),
        Color::Green => Some(Hsla::from(Rgba {
            r: 0.0,
            g: 0.5,
            b: 0.0,
            a: 1.0,
        })),
        Color::Yellow => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.0,
            a: 1.0,
        })),
        Color::Blue => Some(Hsla::from(Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.5,
            a: 1.0,
        })),
        Color::Magenta => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.0,
            b: 0.5,
            a: 1.0,
        })),
        Color::Cyan => Some(Hsla::from(Rgba {
            r: 0.0,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        })),
        Color::Gray => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        })),
        Color::White => Some(Hsla::from(Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        })),
        Color::Reset => None,
        // Handle any other color variants with a reasonable default
        _ => Some(Hsla::from(Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        })), // Default gray
    }
}
