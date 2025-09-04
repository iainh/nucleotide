// ABOUTME: Combined file icon and VCS status indicator component for consistent UI
// ABOUTME: Provides unified icon display with integrated VCS status using design system colors

use gpui::{Context, Hsla, IntoElement, ParentElement, Styled, div, px};
use std::path::Path;

use crate::{FileIcon, Theme};
use nucleotide_types::VcsStatus;

/// Combined file icon and VCS status indicator component
///
/// This component replaces the pattern of manually combining FileIcon and VcsIndicator
/// by providing a single, cohesive component that handles both concerns.
#[derive(Clone)]
pub struct VcsIcon {
    /// The underlying file icon
    file_icon: FileIcon,
    /// Optional VCS status to display as an overlay
    vcs_status: Option<VcsStatus>,
    /// Container size (used for consistent sizing)
    container_size: f32,
}

impl VcsIcon {
    /// Create a VCS icon from a file path
    pub fn from_path(path: &Path, is_expanded: bool) -> Self {
        Self {
            file_icon: FileIcon::from_path(path, is_expanded),
            vcs_status: None,
            container_size: 16.0,
        }
    }

    /// Create a VCS icon from file extension
    pub fn from_extension(extension: Option<&str>) -> Self {
        Self {
            file_icon: FileIcon::from_extension(extension),
            vcs_status: None,
            container_size: 16.0,
        }
    }

    /// Create a directory VCS icon
    pub fn directory(is_expanded: bool) -> Self {
        Self {
            file_icon: FileIcon::directory(is_expanded),
            vcs_status: None,
            container_size: 16.0,
        }
    }

    /// Create a scratch buffer VCS icon
    pub fn scratch() -> Self {
        Self {
            file_icon: FileIcon::scratch(),
            vcs_status: None,
            container_size: 16.0,
        }
    }

    /// Create a symlink VCS icon
    pub fn symlink(target_exists: bool) -> Self {
        Self {
            file_icon: FileIcon::symlink(target_exists),
            vcs_status: None,
            container_size: 16.0,
        }
    }

    /// Set the icon size (affects both file icon and container)
    pub fn size(mut self, size: f32) -> Self {
        self.file_icon = self.file_icon.size(size);
        self.container_size = size;
        self
    }

    /// Set the file icon color
    pub fn text_color(mut self, color: Hsla) -> Self {
        self.file_icon = self.file_icon.text_color(color);
        self
    }

    /// Set the VCS status to display
    pub fn vcs_status(mut self, status: Option<VcsStatus>) -> Self {
        self.vcs_status = status;
        self
    }

    /// Set the VCS status from a status value (convenience method)
    pub fn with_vcs_status(mut self, status: VcsStatus) -> Self {
        self.vcs_status = Some(status);
        self
    }

    /// Check if VCS status should be shown
    fn should_show_vcs_status(&self) -> bool {
        match &self.vcs_status {
            Some(VcsStatus::Clean) | None => false,
            Some(_) => true,
        }
    }

    /// Get the VCS status indicator color using design system colors
    /// Use editor VCS token colors so indicators match gutter, file tree, and tabs
    fn get_vcs_status_color(&self, theme: &Theme) -> Option<Hsla> {
        let dt = &theme.tokens;
        match &self.vcs_status {
            Some(VcsStatus::Modified) => Some(dt.editor.vcs_modified),
            Some(VcsStatus::Added) => Some(dt.editor.vcs_added),
            Some(VcsStatus::Deleted) => Some(dt.editor.vcs_deleted),
            Some(VcsStatus::Untracked) => Some(dt.chrome.text_chrome_secondary),
            Some(VcsStatus::Renamed) => Some(dt.chrome.primary),
            Some(VcsStatus::Conflicted) => Some(dt.editor.error),
            Some(VcsStatus::Unknown) => Some(dt.chrome.text_chrome_secondary),
            Some(VcsStatus::Clean) | None => None,
        }
    }

    /// Render the VCS status overlay
    fn render_vcs_overlay(&self, theme: &Theme) -> impl IntoElement {
        if !self.should_show_vcs_status() {
            return div();
        }

        let color = self.get_vcs_status_color(theme).unwrap_or(theme.text_muted);
        let indicator_size = (self.container_size * 0.5).max(6.0); // 50% of container size, min 6px

        div()
            .absolute()
            .bottom(px(0.0)) // Position at bottom edge
            .left(px(0.0)) // Position at left edge (no negative offset)
            .w(px(indicator_size))
            .h(px(indicator_size))
            .rounded_full()
            .bg(color)
            .border_1()
            .border_color(theme.tokens.chrome.border_default) // Border to separate from icon
    }

    /// Render this VcsIcon using a provided Theme (for contexts where the
    /// generic `VcsIconRenderer` trait cannot be used due to Context types).
    pub fn render_with_theme(self, theme: &Theme) -> gpui::Div {
        // Extract components to avoid partial move issues
        let VcsIcon {
            file_icon,
            vcs_status,
            container_size,
        } = self;

        // Base container
        let mut container = div()
            .w(px(container_size))
            .h(px(container_size))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .child(file_icon);

        // Determine if we should show overlay
        let should_show = match &vcs_status {
            Some(VcsStatus::Clean) | None => false,
            Some(_) => true,
        };

        if should_show {
            let color = match &vcs_status {
                Some(VcsStatus::Modified) => theme.tokens.editor.vcs_modified,
                Some(VcsStatus::Added) => theme.tokens.editor.vcs_added,
                Some(VcsStatus::Deleted) => theme.tokens.editor.vcs_deleted,
                Some(VcsStatus::Untracked) => theme.tokens.chrome.text_chrome_secondary,
                Some(VcsStatus::Renamed) => theme.tokens.chrome.primary,
                Some(VcsStatus::Conflicted) => theme.tokens.editor.error,
                _ => theme.tokens.chrome.text_chrome_secondary,
            };

            let indicator_size = (container_size * 0.5).max(6.0);
            let overlay = div()
                .absolute()
                .bottom(px(0.0))
                .left(px(0.0))
                .w(px(indicator_size))
                .h(px(indicator_size))
                .rounded_full()
                .bg(color)
                .border_1()
                .border_color(theme.tokens.chrome.border_default);

            container = container.child(overlay);
        }

        container
    }
}

/// Trait for easy VCS icon rendering in components that have access to GPUI context
pub trait VcsIconRenderer {
    /// Render a VCS icon with access to theme context
    fn render_vcs_icon(&self, icon: VcsIcon, cx: &mut Context<Self>) -> impl IntoElement
    where
        Self: Sized;
}

impl<T> VcsIconRenderer for T {
    fn render_vcs_icon(&self, icon: VcsIcon, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();

        // Extract the components to avoid partial move issues
        let VcsIcon {
            file_icon,
            vcs_status,
            container_size,
        } = icon;
        let icon_for_overlay = VcsIcon {
            file_icon: file_icon.clone(),
            vcs_status,
            container_size,
        };

        // Container with relative positioning for the icon and overlay
        div()
            .w(px(container_size))
            .h(px(container_size))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .child(file_icon)
            .child(icon_for_overlay.render_vcs_overlay(theme))
    }
}

impl IntoElement for VcsIcon {
    type Element = gpui::Div;

    fn into_element(self) -> Self::Element {
        // When used without context, we can't access the theme
        // In this case, render just the file icon with a fallback VCS indicator

        // Extract components to avoid partial move issues
        let VcsIcon {
            file_icon,
            vcs_status,
            container_size,
        } = self;

        let mut container = div()
            .w(px(container_size))
            .h(px(container_size))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .child(file_icon);

        // Add VCS overlay with fallback colors if status exists
        let should_show = match &vcs_status {
            Some(VcsStatus::Clean) | None => false,
            Some(_) => true,
        };

        if should_show {
            let fallback_color = match &vcs_status {
                Some(VcsStatus::Modified) => gpui::hsla(0.15, 0.8, 0.6, 1.0), // Orange
                Some(VcsStatus::Added) => gpui::hsla(0.33, 0.6, 0.5, 1.0),    // Green
                Some(VcsStatus::Deleted) => gpui::hsla(0.0, 0.8, 0.5, 1.0),   // Red
                Some(VcsStatus::Untracked) => gpui::hsla(0.0, 0.0, 0.7, 1.0), // Muted
                Some(VcsStatus::Renamed) => gpui::hsla(0.61, 0.6, 0.5, 1.0),  // Blue
                Some(VcsStatus::Conflicted) => gpui::hsla(0.0, 0.8, 0.5, 1.0), // Red
                _ => gpui::hsla(0.0, 0.0, 0.7, 1.0),                          // Default muted
            };

            let indicator_size = (container_size * 0.5).max(6.0);

            let overlay = div()
                .absolute()
                .bottom(px(0.0)) // Position at bottom edge
                .left(px(0.0)) // Position at left edge (no negative offset)
                .w(px(indicator_size))
                .h(px(indicator_size))
                .rounded_full()
                .bg(fallback_color)
                .border_1()
                .border_color(gpui::hsla(0.0, 0.0, 0.1, 1.0)); // Dark border

            container = container.child(overlay);
        }

        container
    }
}
