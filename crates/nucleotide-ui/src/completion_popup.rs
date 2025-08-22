// ABOUTME: Smart popup positioning system for completion views
// ABOUTME: Handles constraint-aware placement and multi-monitor support

use gpui::{
    Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Point, Render, Size,
    Styled, div, point, px, size,
};
use std::cmp::max;

/// Position preference for popup placement
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PopupPlacement {
    /// Place below the anchor point (preferred)
    Below,
    /// Place above the anchor point  
    Above,
    /// Place to the right of anchor point
    Right,
    /// Place to the left of anchor point
    Left,
    /// Automatically choose best position
    Auto,
}

/// Constraints for popup positioning
#[derive(Debug, Clone)]
pub struct PopupConstraints {
    /// Minimum distance from screen edges
    pub margin: Pixels,
    /// Maximum width of the popup
    pub max_width: Pixels,
    /// Maximum height of the popup
    pub max_height: Pixels,
    /// Minimum width of the popup
    pub min_width: Pixels,
    /// Minimum height of the popup
    pub min_height: Pixels,
    /// Whether popup can overlap the anchor
    pub allow_overlap: bool,
    /// Preferred placement direction
    pub placement: PopupPlacement,
}

impl Default for PopupConstraints {
    fn default() -> Self {
        Self {
            margin: px(8.0),
            max_width: px(400.0),
            max_height: px(300.0),
            min_width: px(200.0),
            min_height: px(100.0),
            allow_overlap: false,
            placement: PopupPlacement::Auto,
        }
    }
}

/// Result of popup positioning calculation
#[derive(Debug, Clone)]
pub struct PopupPosition {
    /// Final position of the popup
    pub bounds: Bounds<Pixels>,
    /// Actual placement used
    pub placement: PopupPlacement,
    /// Whether the popup was constrained by screen boundaries
    pub constrained: bool,
    /// Available space in each direction
    pub available_space: AvailableSpace,
}

/// Available space around the anchor point
#[derive(Debug, Clone)]
pub struct AvailableSpace {
    pub above: Pixels,
    pub below: Pixels,
    pub left: Pixels,
    pub right: Pixels,
}

/// Smart popup positioning calculator
pub struct PopupPositioner {
    constraints: PopupConstraints,
}

impl PopupPositioner {
    pub fn new(constraints: PopupConstraints) -> Self {
        Self { constraints }
    }

    pub fn with_placement(mut self, placement: PopupPlacement) -> Self {
        self.constraints.placement = placement;
        self
    }

    pub fn with_max_size(mut self, width: Pixels, height: Pixels) -> Self {
        self.constraints.max_width = width;
        self.constraints.max_height = height;
        self
    }

    /// Calculate optimal popup position
    pub fn calculate_position(
        &self,
        anchor: Point<Pixels>,
        content_size: Size<Pixels>,
        window_bounds: Bounds<Pixels>,
    ) -> PopupPosition {
        let available_space = self.calculate_available_space(anchor, window_bounds);

        let optimal_size = self.calculate_optimal_size(content_size, &available_space);

        let (placement, position, constrained) = match self.constraints.placement {
            PopupPlacement::Auto => {
                self.find_best_placement(anchor, optimal_size, &available_space)
            }
            placement => {
                self.place_with_preference(anchor, optimal_size, placement, &available_space)
            }
        };

        PopupPosition {
            bounds: Bounds {
                origin: position,
                size: optimal_size,
            },
            placement,
            constrained,
            available_space,
        }
    }

    /// Calculate available space in all directions from anchor
    fn calculate_available_space(
        &self,
        anchor: Point<Pixels>,
        window_bounds: Bounds<Pixels>,
    ) -> AvailableSpace {
        let margin = self.constraints.margin;

        AvailableSpace {
            above: max(px(0.0), anchor.y - window_bounds.origin.y - margin),
            below: max(
                px(0.0),
                window_bounds.origin.y + window_bounds.size.height - anchor.y - margin,
            ),
            left: max(px(0.0), anchor.x - window_bounds.origin.x - margin),
            right: max(
                px(0.0),
                window_bounds.origin.x + window_bounds.size.width - anchor.x - margin,
            ),
        }
    }

    /// Calculate optimal size given constraints and available space
    fn calculate_optimal_size(
        &self,
        content_size: Size<Pixels>,
        _available_space: &AvailableSpace,
    ) -> Size<Pixels> {
        let width = content_size
            .width
            .max(self.constraints.min_width)
            .min(self.constraints.max_width);

        let height = content_size
            .height
            .max(self.constraints.min_height)
            .min(self.constraints.max_height);

        Size { width, height }
    }

    /// Find the best placement automatically
    fn find_best_placement(
        &self,
        anchor: Point<Pixels>,
        size: Size<Pixels>,
        available_space: &AvailableSpace,
    ) -> (PopupPlacement, Point<Pixels>, bool) {
        // Score each placement option
        let placements = [
            (
                PopupPlacement::Below,
                self.score_placement(PopupPlacement::Below, size, available_space),
            ),
            (
                PopupPlacement::Above,
                self.score_placement(PopupPlacement::Above, size, available_space),
            ),
            (
                PopupPlacement::Right,
                self.score_placement(PopupPlacement::Right, size, available_space),
            ),
            (
                PopupPlacement::Left,
                self.score_placement(PopupPlacement::Left, size, available_space),
            ),
        ];

        // Choose the placement with the highest score
        let best_placement = placements
            .iter()
            .max_by(|(_, score_a), (_, score_b)| score_a.partial_cmp(score_b).unwrap())
            .map(|(placement, _)| *placement)
            .unwrap_or(PopupPlacement::Below);

        self.place_with_preference(anchor, size, best_placement, available_space)
    }

    /// Score a placement option (higher is better)
    fn score_placement(
        &self,
        placement: PopupPlacement,
        size: Size<Pixels>,
        available_space: &AvailableSpace,
    ) -> f32 {
        let required_space = match placement {
            PopupPlacement::Below => available_space.below,
            PopupPlacement::Above => available_space.above,
            PopupPlacement::Right => available_space.right,
            PopupPlacement::Left => available_space.left,
            PopupPlacement::Auto => px(0.0), // Not used in scoring
        };

        let space_factor = if required_space >= size.height || required_space >= size.width {
            1.0 // Full space available
        } else {
            required_space.0 / size.height.max(size.width).0 // Partial space
        };

        // Prefer Below > Right > Above > Left
        let preference_bonus = match placement {
            PopupPlacement::Below => 3.0,
            PopupPlacement::Right => 2.0,
            PopupPlacement::Above => 1.0,
            PopupPlacement::Left => 0.0,
            PopupPlacement::Auto => 0.0,
        };

        space_factor * 10.0 + preference_bonus
    }

    /// Place popup with specific placement preference
    fn place_with_preference(
        &self,
        anchor: Point<Pixels>,
        size: Size<Pixels>,
        placement: PopupPlacement,
        available_space: &AvailableSpace,
    ) -> (PopupPlacement, Point<Pixels>, bool) {
        let margin = self.constraints.margin;
        let mut constrained = false;

        let position = match placement {
            PopupPlacement::Below => {
                let y = anchor.y + margin;
                let x = self.constrain_horizontal(anchor.x, size.width, available_space);
                if available_space.below < size.height {
                    constrained = true;
                }
                point(x, y)
            }
            PopupPlacement::Above => {
                let y = anchor.y - size.height - margin;
                let x = self.constrain_horizontal(anchor.x, size.width, available_space);
                if available_space.above < size.height {
                    constrained = true;
                }
                point(x, y)
            }
            PopupPlacement::Right => {
                let x = anchor.x + margin;
                let y = self.constrain_vertical(anchor.y, size.height, available_space);
                if available_space.right < size.width {
                    constrained = true;
                }
                point(x, y)
            }
            PopupPlacement::Left => {
                let x = anchor.x - size.width - margin;
                let y = self.constrain_vertical(anchor.y, size.height, available_space);
                if available_space.left < size.width {
                    constrained = true;
                }
                point(x, y)
            }
            PopupPlacement::Auto => {
                // This shouldn't happen as Auto is handled by find_best_placement
                point(anchor.x, anchor.y + margin)
            }
        };

        (placement, position, constrained)
    }

    /// Constrain horizontal position to fit within available space
    fn constrain_horizontal(
        &self,
        preferred_x: Pixels,
        width: Pixels,
        available_space: &AvailableSpace,
    ) -> Pixels {
        let margin = self.constraints.margin;

        // Try to center on the preferred x position
        let left_aligned = preferred_x - width / 2.0;
        let right_edge = left_aligned + width;

        // Adjust if it goes outside available space
        if left_aligned < margin {
            margin
        } else if right_edge > available_space.right + margin {
            available_space.right + margin - width
        } else {
            left_aligned
        }
    }

    /// Constrain vertical position to fit within available space
    fn constrain_vertical(
        &self,
        preferred_y: Pixels,
        height: Pixels,
        available_space: &AvailableSpace,
    ) -> Pixels {
        let margin = self.constraints.margin;

        // Try to center on the preferred y position
        let top_aligned = preferred_y - height / 2.0;
        let bottom_edge = top_aligned + height;

        // Adjust if it goes outside available space
        if top_aligned < margin {
            margin
        } else if bottom_edge > available_space.below + margin {
            available_space.below + margin - height
        } else {
            top_aligned
        }
    }
}

/// Popup component with smart positioning
pub struct SmartPopup<T> {
    anchor: Point<Pixels>,
    content: T,
    positioner: PopupPositioner,
    visible: bool,
}

impl<T> SmartPopup<T>
where
    T: IntoElement + Clone,
{
    pub fn new(anchor: Point<Pixels>, content: T) -> Self {
        Self {
            anchor,
            content,
            positioner: PopupPositioner::new(PopupConstraints::default()),
            visible: true,
        }
    }

    pub fn with_constraints(mut self, constraints: PopupConstraints) -> Self {
        self.positioner = PopupPositioner::new(constraints);
        self
    }

    pub fn with_placement(mut self, placement: PopupPlacement) -> Self {
        self.positioner = self.positioner.with_placement(placement);
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }
}

impl<T> Render for SmartPopup<T>
where
    T: IntoElement + Clone + 'static,
{
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().id("hidden-popup");
        }

        // TODO: Get window bounds for positioning calculation when API is available
        // For now, use default positioning
        let estimated_content_size = size(px(300.0), px(200.0));
        let window_bounds = gpui::Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: size(px(1920.0), px(1080.0)),
        };

        let position =
            self.positioner
                .calculate_position(self.anchor, estimated_content_size, window_bounds);

        div()
            .id("smart-popup")
            .absolute()
            .top(position.bounds.origin.y)
            .left(position.bounds.origin.x)
            .w(position.bounds.size.width)
            .h(position.bounds.size.height)
            // TODO: Add proper z-index when API is available
            .child(self.content.clone())
    }
}

/// Helper function to create completion popup with smart positioning
pub fn create_completion_popup<T: IntoElement + Clone + 'static>(
    anchor: Point<Pixels>,
    content: T,
) -> SmartPopup<T> {
    let constraints = PopupConstraints {
        max_width: px(500.0),
        max_height: px(400.0),
        min_width: px(250.0),
        min_height: px(100.0),
        placement: PopupPlacement::Below,
        margin: px(8.0),
        allow_overlap: false,
    };

    SmartPopup::new(anchor, content).with_constraints(constraints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_popup_constraints_default() {
        let constraints = PopupConstraints::default();
        assert_eq!(constraints.margin, px(8.0));
        assert_eq!(constraints.placement, PopupPlacement::Auto);
        assert!(!constraints.allow_overlap);
    }

    #[test]
    fn test_available_space_calculation() {
        let positioner = PopupPositioner::new(PopupConstraints::default());
        let anchor = point(px(100.0), px(200.0));
        let window_bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(800.0), px(600.0)),
        };

        let space = positioner.calculate_available_space(anchor, window_bounds);

        // With 8px margin
        assert_eq!(space.above, px(192.0)); // 200 - 0 - 8
        assert_eq!(space.below, px(392.0)); // 600 - 200 - 8
        assert_eq!(space.left, px(92.0)); // 100 - 0 - 8  
        assert_eq!(space.right, px(692.0)); // 800 - 100 - 8
    }

    #[test]
    fn test_placement_scoring() {
        let positioner = PopupPositioner::new(PopupConstraints::default());
        let size = size(px(200.0), px(150.0));
        let available_space = AvailableSpace {
            above: px(100.0),
            below: px(300.0),
            left: px(150.0),
            right: px(250.0),
        };

        let below_score = positioner.score_placement(PopupPlacement::Below, size, &available_space);
        let above_score = positioner.score_placement(PopupPlacement::Above, size, &available_space);

        // Below should score higher due to more space and preference
        assert!(below_score > above_score);
    }

    #[test]
    fn test_optimal_size_calculation() {
        let constraints = PopupConstraints {
            min_width: px(100.0),
            max_width: px(400.0),
            min_height: px(80.0),
            max_height: px(300.0),
            ..Default::default()
        };

        let positioner = PopupPositioner::new(constraints);
        let available_space = AvailableSpace {
            above: px(200.0),
            below: px(200.0),
            left: px(200.0),
            right: px(200.0),
        };

        // Test size within bounds
        let content_size = size(px(250.0), px(150.0));
        let optimal = positioner.calculate_optimal_size(content_size, &available_space);
        assert_eq!(optimal.width, px(250.0));
        assert_eq!(optimal.height, px(150.0));

        // Test size too small
        let small_size = size(px(50.0), px(50.0));
        let optimal = positioner.calculate_optimal_size(small_size, &available_space);
        assert_eq!(optimal.width, px(100.0)); // Clamped to min
        assert_eq!(optimal.height, px(80.0)); // Clamped to min

        // Test size too large
        let large_size = size(px(500.0), px(400.0));
        let optimal = positioner.calculate_optimal_size(large_size, &available_space);
        assert_eq!(optimal.width, px(400.0)); // Clamped to max
        assert_eq!(optimal.height, px(300.0)); // Clamped to max
    }

    #[test]
    fn test_horizontal_constraint() {
        let positioner = PopupPositioner::new(PopupConstraints::default());
        let available_space = AvailableSpace {
            above: px(200.0),
            below: px(200.0),
            left: px(50.0),
            right: px(300.0),
        };

        // Test normal positioning
        let x = positioner.constrain_horizontal(px(100.0), px(150.0), &available_space);
        assert_eq!(x, px(25.0)); // 100 - 150/2 = 25

        // Test left edge constraint
        let x = positioner.constrain_horizontal(px(0.0), px(150.0), &available_space);
        assert_eq!(x, px(8.0)); // Margin

        // Test right edge constraint
        let x = positioner.constrain_horizontal(px(300.0), px(150.0), &available_space);
        assert!(x < px(300.0)); // Should be constrained
    }

    #[test]
    fn test_smart_popup_creation() {
        let anchor = point(px(100.0), px(200.0));
        let content = "Test content";

        let popup = create_completion_popup(anchor, content);
        assert_eq!(popup.anchor, anchor);
        assert!(popup.visible);
    }

    #[test]
    fn test_popup_visibility() {
        let anchor = point(px(100.0), px(200.0));
        let content = "Test content";

        let popup = SmartPopup::new(anchor, content).visible(false);
        assert!(!popup.visible);

        let popup = popup.visible(true);
        assert!(popup.visible);
    }
}
