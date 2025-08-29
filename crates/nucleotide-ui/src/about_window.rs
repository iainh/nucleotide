// ABOUTME: About window component for displaying app information
// ABOUTME: Shows app icon, name, author, version, and commit hash

use crate::{Button, ButtonVariant, Theme};
use gpui::prelude::FluentBuilder;
use gpui::{
    Context, DismissEvent, EventEmitter, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Window, div, img, px,
};

#[derive(Debug)]
pub struct AboutWindow {
    app_name: SharedString,
    version: SharedString,
    author: SharedString,
    commit_hash: Option<SharedString>,
    visible: bool,
}

impl AboutWindow {
    pub fn new() -> Self {
        let app_name = "Nucleotide".into();
        let version = env!("CARGO_PKG_VERSION").into();
        let author = "The Nucleotide Contributors".into();

        // Try to get git commit hash
        let commit_hash = Self::get_git_commit_hash();

        Self {
            app_name,
            version,
            author,
            commit_hash,
            visible: false,
        }
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.visible = true;
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        cx.notify();
        cx.emit(DismissEvent);
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    fn get_git_commit_hash() -> Option<SharedString> {
        // Try to get git commit hash at runtime
        if let Ok(output) = std::process::Command::new("git")
            .args(&["rev-parse", "--short", "HEAD"])
            .output()
        {
            if output.status.success() {
                let commit_hash = String::from_utf8_lossy(&output.stdout).to_string();
                let commit_hash = commit_hash.trim().to_string();
                if !commit_hash.is_empty() {
                    return Some(commit_hash.into());
                }
            }
        }
        None
    }

    fn handle_click(&mut self, cx: &mut Context<Self>) {
        self.hide(cx);
    }
}

impl Default for AboutWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl EventEmitter<DismissEvent> for AboutWindow {}

impl Render for AboutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div(); // Return empty div when not visible
        }

        let theme = cx.global::<Theme>();
        let tokens = &theme.tokens;

        // Create backdrop overlay
        div()
            .absolute()
            .inset_0()
            .bg(gpui::rgba(0x00000080)) // Semi-transparent backdrop
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.handle_click(cx);
                }),
            )
            .child(
                // About dialog box
                div()
                    .bg(tokens.chrome.surface_elevated)
                    .border_1()
                    .border_color(tokens.chrome.border_strong)
                    .rounded_lg()
                    .shadow_lg()
                    .p_6()
                    .w(px(400.0))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .on_mouse_down(gpui::MouseButton::Left, |_event, _window, _cx| {
                        // Prevent event from bubbling up to backdrop
                    })
                    // App icon using PNG logo
                    .child(
                        div()
                            .mx_auto()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                img("nucleotide.iconset/icon_128x128.png")
                                    .size(px(80.0)) // Increased from 56px to 80px
                                    .flex_shrink_0(),
                            ),
                    )
                    // App name
                    .child(
                        div()
                            .mx_auto()
                            .text_size(px(24.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(tokens.chrome.text_on_chrome)
                            .child(self.app_name.clone()),
                    )
                    // Version
                    .child(
                        div()
                            .mx_auto()
                            .text_size(px(14.0))
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child(format!("Version {}", self.version)),
                    )
                    // Commit hash if available
                    .when_some(self.commit_hash.as_ref(), |this, commit| {
                        this.child(
                            div()
                                .mx_auto()
                                .text_size(px(12.0))
                                .text_color(tokens.chrome.text_chrome_secondary)
                                .child(format!("Commit {}", commit)),
                        )
                    })
                    // Author
                    .child(
                        div()
                            .mx_auto()
                            .text_size(px(14.0))
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child(self.author.clone()),
                    )
                    // Separator
                    .child(
                        div()
                            .w_full()
                            .h(px(1.0))
                            .bg(tokens.chrome.separator_color)
                            .my_2(),
                    )
                    // Description
                    .child(
                        div()
                            .mx_auto()
                            .text_size(px(12.0))
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .text_center()
                            .child("A native GUI implementation of the Helix modal text editor"),
                    )
                    // Built with info
                    .child(
                        div()
                            .mx_auto()
                            .text_size(px(11.0))
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .text_center()
                            .child("Built with GPUI and Rust"),
                    )
                    // Close button
                    .child(
                        div().mx_auto().mt_4().child(
                            Button::new("about-ok", "OK")
                                .variant(ButtonVariant::Secondary)
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.handle_click(cx);
                                })),
                        ),
                    ),
            )
    }
}
