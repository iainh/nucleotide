// ABOUTME: About window component for displaying app information
// ABOUTME: Shows app icon, name, author, version, and commit hash

use crate::modal_layer::ModalView;
use crate::{Button, ButtonSize, ButtonVariant, FocusTraversal, Theme};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, img,
    px,
};

#[derive(Debug)]
pub struct AboutWindow {
    app_name: SharedString,
    version: SharedString,
    author: SharedString,
    commit_hash: Option<SharedString>,
    focus_handle: FocusHandle,
    ok_focus_handle: FocusHandle,
}

impl AboutWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
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
            focus_handle: cx.focus_handle().tab_stop(false),
            ok_focus_handle: cx.focus_handle().tab_index(1).tab_stop(true),
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn get_git_commit_hash() -> Option<SharedString> {
        // Try to get git commit hash at runtime
        if let Ok(output) = nucleotide_process::command("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            && output.status.success()
        {
            let commit_hash = String::from_utf8_lossy(&output.stdout).to_string();
            let commit_hash = commit_hash.trim().to_string();
            if !commit_hash.is_empty() {
                return Some(commit_hash.into());
            }
        }
        None
    }
}

impl EventEmitter<DismissEvent> for AboutWindow {}

impl Focusable for AboutWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for AboutWindow {}

impl Render for AboutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let tokens = &theme.tokens;

        FocusTraversal::new(
            div()
                .track_focus(&self.focus_handle)
                .occlude()
                .bg(tokens.chrome.surface_elevated)
                .border_1()
                .border_color(tokens.chrome.border_strong)
                .rounded_lg()
                .shadow(vec![tokens.chrome.shadow_lg.to_box_shadow(false)])
                .p_6()
                .w(px(400.0))
                .flex()
                .flex_col()
                .gap_4()
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .mx_auto()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img("nucleotide.iconset/icon_128x128.png")
                                .size(px(80.0))
                                .flex_shrink_0(),
                        ),
                )
                .child(
                    div()
                        .mx_auto()
                        .text_size(tokens.sizes.text_xl)
                        .font_weight(FontWeight::BOLD)
                        .text_color(tokens.chrome.text_on_chrome)
                        .child(self.app_name.clone()),
                )
                .child(
                    div()
                        .mx_auto()
                        .text_size(tokens.sizes.text_md)
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .child(format!("Version {}", self.version)),
                )
                .when_some(self.commit_hash.as_ref(), |this, commit| {
                    this.child(
                        div()
                            .mx_auto()
                            .text_size(tokens.sizes.text_sm)
                            .text_color(tokens.chrome.text_chrome_secondary)
                            .child(format!("Commit {}", commit)),
                    )
                })
                .child(
                    div()
                        .mx_auto()
                        .text_size(tokens.sizes.text_md)
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .child(self.author.clone()),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(1.0))
                        .bg(tokens.chrome.separator_color)
                        .my_2(),
                )
                .child(
                    div()
                        .mx_auto()
                        .text_size(tokens.sizes.text_sm)
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .text_center()
                        .child("A native GUI implementation of the Helix modal text editor"),
                )
                .child(
                    div()
                        .mx_auto()
                        .text_size(tokens.sizes.text_xs)
                        .text_color(tokens.chrome.text_chrome_secondary)
                        .text_center()
                        .child("Built with GPUI and Rust"),
                )
                .child(
                    div().mx_auto().mt_4().child(
                        Button::new("about-ok", "OK")
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Small)
                            .focus_handle(self.ok_focus_handle.clone())
                            .activate_on_mouse_down()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.dismiss(cx);
                                cx.stop_propagation();
                            })),
                    ),
                ),
        )
    }
}
