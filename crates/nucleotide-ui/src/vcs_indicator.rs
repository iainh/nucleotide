// ABOUTME: VCS status indicator component for consistent git status display
// ABOUTME: Provides colored dots and overlays showing file modification status

use gpui::{IntoElement, Styled, div, px};
use nucleotide_types::VcsStatus;
use std::path::Path;

/// VCS status indicator component
#[derive(Clone)]
pub struct VcsIndicator {
    /// The VCS status to display
    status: VcsStatus,
    /// Size of the indicator dot in pixels
    size: f32,
    /// Whether to show as an overlay (with positioning) or inline
    overlay_mode: bool,
}

impl VcsIndicator {
    /// Create a new VCS indicator for the given status
    pub fn new(status: VcsStatus) -> Self {
        Self {
            status,
            size: 8.0, // Default size
            overlay_mode: false,
        }
    }

    /// Create a VCS indicator from a file path by checking git status
    pub fn from_path<P: AsRef<Path>>(_path: P) -> Self {
        // TODO: Integrate with actual VCS system
        // For now, return Clean as default
        Self::new(VcsStatus::Clean)
    }

    /// Set the size of the indicator dot
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Enable overlay mode for positioning over icons
    pub fn overlay(mut self) -> Self {
        self.overlay_mode = true;
        self
    }

    /// Check if this status should be visible
    fn should_show(&self) -> bool {
        !matches!(self.status, VcsStatus::Clean)
    }
}

impl IntoElement for VcsIndicator {
    type Element = gpui::Div;

    fn into_element(self) -> Self::Element {
        // Don't render anything for up-to-date files
        if !self.should_show() {
            return div();
        }

        // Use fallback colors since we can't access theme in IntoElement
        let color = match self.status {
            VcsStatus::Modified => gpui::hsla(0.15, 0.8, 0.6, 1.0), // Orange/warning
            VcsStatus::Added => gpui::hsla(0.33, 0.6, 0.5, 1.0),    // Green/success
            VcsStatus::Deleted => gpui::hsla(0.0, 0.8, 0.5, 1.0),   // Red/error
            VcsStatus::Untracked => gpui::hsla(0.0, 0.0, 0.7, 1.0), // Muted
            VcsStatus::Renamed => gpui::hsla(0.61, 0.6, 0.5, 1.0),  // Accent/blue
            VcsStatus::Conflicted => gpui::hsla(0.0, 0.8, 0.5, 1.0), // Red/error
            VcsStatus::Clean => return div(),                       // Shouldn't reach here
            VcsStatus::Unknown => gpui::hsla(0.0, 0.0, 0.5, 1.0),   // Gray/unknown
        };

        let mut indicator = div()
            .w(px(self.size))
            .h(px(self.size))
            .rounded_full()
            .bg(color)
            .border_1()
            .border_color(gpui::hsla(0.0, 0.0, 0.1, 1.0)) // Dark border
            .flex_shrink_0();

        if self.overlay_mode {
            indicator = indicator.absolute().bottom(px(-2.0)).left(px(-2.0));
        }

        indicator
    }
}
