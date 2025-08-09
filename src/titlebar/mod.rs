// ABOUTME: Titlebar module implementing custom window decorations with platform-specific controls
// ABOUTME: Based on Zed's titlebar implementation pattern for consistent GPUI window management

mod platform_titlebar;
mod window_controls;

pub use platform_titlebar::PlatformTitleBar;

use gpui::{
    px, Context, Entity, IntoElement, Render, 
    WeakEntity, Window,
};
use gpui::AppContext;
use crate::workspace::Workspace;

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    workspace: WeakEntity<Workspace>,
}

impl TitleBar {
    pub fn new(
        id: impl Into<gpui::ElementId>,
        workspace: &Entity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let platform_titlebar = cx.new(|_cx| PlatformTitleBar::new(id));
        
        Self {
            platform_titlebar,
            workspace: workspace.downgrade(),
        }
    }
    
    pub fn height(window: &Window) -> gpui::Pixels {
        // Match Zed's titlebar height calculation
        #[cfg(target_os = "windows")]
        return px(32.0);
        
        #[cfg(not(target_os = "windows"))]
        return (1.75 * window.rem_size()).max(px(34.0));
    }
}

impl Render for TitleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Get the current filename from workspace
        let filename = self.workspace
            .upgrade()
            .and_then(|workspace| {
                workspace.read(cx).current_filename(cx)
            })
            .unwrap_or_else(|| "Nucleotide".to_string());
        
        // Update platform titlebar with content
        self.platform_titlebar.update(cx, |titlebar, _cx| {
            titlebar.set_title(filename);
        });
        
        self.platform_titlebar.clone()
    }
}