// ABOUTME: Focus indicator styling utilities for applying visual focus indicators to UI elements
// ABOUTME: Provides traits and helpers for consistent focus indication across the application

use crate::global_input::FocusIndicatorStyles;
use gpui::{IntoElement, ParentElement, Styled, div};

/// Trait for applying focus indicator styles to elements
pub trait FocusIndicator: IntoElement + Styled + Sized {
    /// Apply focus indicator styles to an element
    fn with_focus_indicators(mut self, styles: &FocusIndicatorStyles, is_focused: bool) -> Self {
        if !is_focused || !styles.has_any_indicators() {
            return self;
        }

        // Apply border styles
        if let (Some(color), Some(width)) = (styles.border_color, styles.border_width) {
            self = self.border_color(color).border(width);
        }

        // Apply background styles
        if let Some(color) = styles.background_color {
            let background_color = if let Some(opacity) = styles.background_opacity {
                color.alpha(opacity)
            } else {
                color
            };
            self = self.bg(background_color);
        }

        // Apply outline styles (using border as GPUI doesn't have outline)
        if let (Some(color), Some(width)) = (styles.outline_color, styles.outline_width) {
            // Use a stronger border to simulate outline
            let outline_width = width + gpui::px(1.0);
            self = self.border_color(color).border(outline_width);
        }

        self
    }

    /// Apply focus indicator styles with animation support
    fn with_animated_focus_indicators(
        self,
        styles: &FocusIndicatorStyles,
        is_focused: bool,
    ) -> Self {
        if styles.animation_duration.as_millis() > 0 {
            // TODO: Implement animation support when GPUI animation APIs are available
            // For now, just apply static styles
        }

        self.with_focus_indicators(styles, is_focused)
    }
}

/// Blanket implementation for all GPUI elements
impl<T: IntoElement + Styled> FocusIndicator for T {}

/// Helper function to create a focused wrapper element
pub fn focused_element<E: IntoElement>(
    element: E,
    styles: &FocusIndicatorStyles,
    is_focused: bool,
) -> impl IntoElement {
    if !is_focused || !styles.has_any_indicators() {
        return div().child(element);
    }

    let mut wrapper = div().child(element);

    // Apply border styles to wrapper
    if let (Some(color), Some(width)) = (styles.border_color, styles.border_width) {
        wrapper = wrapper.border_color(color).border(width);
    }

    // Apply background styles to wrapper
    if let Some(color) = styles.background_color {
        let background_color = if let Some(opacity) = styles.background_opacity {
            color.alpha(opacity)
        } else {
            color
        };
        wrapper = wrapper.bg(background_color);
    }

    // Apply outline styles (using border + padding to simulate outline with offset)
    if let (Some(color), Some(width), Some(offset)) = (
        styles.outline_color,
        styles.outline_width,
        styles.outline_offset,
    ) {
        wrapper = wrapper.border_color(color).border(width).p(offset);
    }

    wrapper
}

/// Keyboard focus ring styles for high contrast accessibility
pub fn high_contrast_focus_ring() -> FocusIndicatorStyles {
    FocusIndicatorStyles {
        border_color: Some(gpui::hsla(220.0 / 360.0, 1.0, 0.5, 1.0)), // Bright blue
        border_width: Some(gpui::px(3.0)),
        background_color: None,
        background_opacity: None,
        outline_color: Some(gpui::hsla(0.0, 0.0, 1.0, 1.0)), // White outline
        outline_width: Some(gpui::px(1.0)),
        outline_offset: Some(gpui::px(2.0)),
        animation_duration: std::time::Duration::from_millis(0),
    }
}

/// Subtle focus ring styles for regular use
pub fn subtle_focus_ring() -> FocusIndicatorStyles {
    FocusIndicatorStyles {
        border_color: Some(gpui::hsla(220.0 / 360.0, 0.8, 0.6, 0.8)), // Soft blue
        border_width: Some(gpui::px(2.0)),
        background_color: Some(gpui::hsla(220.0 / 360.0, 0.3, 0.9, 0.1)), // Very light blue background
        background_opacity: Some(0.1),
        outline_color: None,
        outline_width: None,
        outline_offset: None,
        animation_duration: std::time::Duration::from_millis(150),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focus_indicator_styles_none() {
        let styles = FocusIndicatorStyles::none();
        assert!(!styles.has_any_indicators());
        assert_eq!(
            styles.animation_duration,
            std::time::Duration::from_millis(0)
        );
    }

    #[test]
    fn test_focus_indicator_styles_with_border() {
        let styles = FocusIndicatorStyles {
            border_color: Some(gpui::hsla(0.5, 0.5, 0.5, 1.0)),
            border_width: Some(gpui::px(2.0)),
            ..FocusIndicatorStyles::none()
        };
        assert!(styles.has_any_indicators());
    }

    #[test]
    fn test_high_contrast_focus_ring() {
        let styles = high_contrast_focus_ring();
        assert!(styles.has_any_indicators());
        assert!(styles.border_color.is_some());
        assert!(styles.outline_color.is_some());
        assert_eq!(styles.border_width, Some(gpui::px(3.0)));
    }

    #[test]
    fn test_subtle_focus_ring() {
        let styles = subtle_focus_ring();
        assert!(styles.has_any_indicators());
        assert!(styles.border_color.is_some());
        assert!(styles.background_color.is_some());
        assert_eq!(
            styles.animation_duration,
            std::time::Duration::from_millis(150)
        );
    }
}
