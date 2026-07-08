// ABOUTME: Indeterminate progress indicator components for compact status surfaces
// ABOUTME: Provides reusable animated loading affordances using design system colors

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, ElementId, Hsla, IntoElement, Styled, Transformation,
    percentage, px, svg,
};

const DEFAULT_SPINNER_SIZE: f32 = 14.0;
const DEFAULT_SPINNER_DURATION: Duration = Duration::from_millis(900);

#[derive(Clone)]
pub struct IndeterminateProgressIndicator {
    id: ElementId,
    size: f32,
    color: Option<Hsla>,
    duration: Duration,
}

impl IndeterminateProgressIndicator {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            size: DEFAULT_SPINNER_SIZE,
            color: None,
            duration: DEFAULT_SPINNER_DURATION,
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn text_color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }
}

impl IntoElement for IndeterminateProgressIndicator {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        let mut indicator = svg()
            .path("icons/loader-circle.svg")
            .size(px(self.size))
            .flex_shrink_0();

        if let Some(color) = self.color {
            indicator = indicator.text_color(color);
        }

        indicator
            .with_animation(
                self.id,
                Animation::new(self.duration).repeat(),
                |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_compact_status_bar_size() {
        let indicator = IndeterminateProgressIndicator::new("test-spinner");

        assert_eq!(indicator.size, DEFAULT_SPINNER_SIZE);
        assert_eq!(indicator.duration, DEFAULT_SPINNER_DURATION);
        assert!(indicator.color.is_none());
    }

    #[test]
    fn builder_updates_visual_properties() {
        let color = gpui::hsla(0.4, 0.5, 0.6, 1.0);
        let duration = Duration::from_millis(1200);
        let indicator = IndeterminateProgressIndicator::new("test-spinner")
            .size(18.0)
            .text_color(color)
            .duration(duration);

        assert_eq!(indicator.size, 18.0);
        assert_eq!(indicator.color, Some(color));
        assert_eq!(indicator.duration, duration);
    }
}
