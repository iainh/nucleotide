// ABOUTME: VCS status indicator component for consistent git status display
// ABOUTME: Provides colored dots and overlays showing file modification status

use gpui::{Context, IntoElement, Styled, div, px};
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

/// Renderer to produce a theme-aware VCS indicator with access to tokens
pub trait VcsIndicatorRenderer {
    fn render_vcs_indicator(&self, indicator: VcsIndicator, cx: &mut Context<Self>) -> gpui::Div
    where
        Self: Sized;
}

impl<T> VcsIndicatorRenderer for T {
    fn render_vcs_indicator(&self, indicator: VcsIndicator, cx: &mut Context<Self>) -> gpui::Div {
        // Don't render anything for up-to-date files
        if !indicator.should_show() {
            return div();
        }

        // Fetch tokens from the current Theme via provider
        let theme = cx.global::<crate::Theme>();
        let dt = theme.tokens;

        // Map VCS status to token colors
        let color = match indicator.status {
            VcsStatus::Modified => dt.editor.warning,
            VcsStatus::Added => dt.editor.success,
            VcsStatus::Deleted => dt.editor.error,
            VcsStatus::Untracked => dt.chrome.text_chrome_secondary,
            VcsStatus::Renamed => dt.chrome.primary,
            VcsStatus::Conflicted => dt.editor.error,
            VcsStatus::Clean => return div(),
            VcsStatus::Unknown => dt.chrome.text_chrome_secondary,
        };

        let mut indicator_div = div()
            .w(px(indicator.size))
            .h(px(indicator.size))
            .rounded_full()
            .bg(color)
            .border_1()
            .border_color(dt.chrome.border_muted)
            .flex_shrink_0();

        if indicator.overlay_mode {
            indicator_div = indicator_div.absolute().bottom(px(-2.0)).left(px(-2.0));
        }

        indicator_div
    }
}

impl IntoElement for VcsIndicator {
    type Element = gpui::Div;

    fn into_element(self) -> Self::Element {
        // Fallback rendering without context (no provider access)
        if !self.should_show() {
            return div();
        }
        let dt = crate::DesignTokens::dark();
        let color = match self.status {
            VcsStatus::Modified => dt.editor.warning,
            VcsStatus::Added => dt.editor.success,
            VcsStatus::Deleted => dt.editor.error,
            VcsStatus::Untracked => dt.chrome.text_chrome_secondary,
            VcsStatus::Renamed => dt.chrome.primary,
            VcsStatus::Conflicted => dt.editor.error,
            VcsStatus::Clean => return div(),
            VcsStatus::Unknown => dt.chrome.text_chrome_secondary,
        };
        let mut indicator = div()
            .w(px(self.size))
            .h(px(self.size))
            .rounded_full()
            .bg(color)
            .border_1()
            .border_color(dt.chrome.border_muted)
            .flex_shrink_0();
        if self.overlay_mode {
            indicator = indicator.absolute().bottom(px(-2.0)).left(px(-2.0));
        }
        indicator
    }
}
