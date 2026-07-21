use gpui::{
    Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    Window, div, px,
};
use nucleotide_ui::{
    Button, ButtonSize, ButtonVariant, FocusTraversal, MarkdownStyle, ModalView, ThemedContext,
    markdown,
};

use crate::actions::updates::Restart;

use super::{UpdateController, UpdateState};

#[derive(Clone, Copy)]
enum DialogAction {
    Check,
    Download,
    Retry,
    Restart,
}

impl DialogAction {
    fn icon(self) -> &'static str {
        match self {
            Self::Check | Self::Retry | Self::Restart => "icons/rotate-ccw.svg",
            Self::Download => "icons/download.svg",
        }
    }
}

pub struct UpdateDialog {
    controller: Entity<UpdateController>,
    focus_handle: FocusHandle,
    primary_focus_handle: FocusHandle,
    close_focus_handle: FocusHandle,
}

impl UpdateDialog {
    pub fn new(controller: Entity<UpdateController>, cx: &mut Context<Self>) -> Self {
        cx.observe(&controller, |_, _, cx| cx.notify()).detach();
        Self {
            controller,
            focus_handle: cx.focus_handle().tab_stop(false),
            primary_focus_handle: cx.focus_handle().tab_index(1).tab_stop(true),
            close_focus_handle: cx.focus_handle().tab_index(2).tab_stop(true),
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for UpdateDialog {}

impl Focusable for UpdateDialog {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for UpdateDialog {}

impl Render for UpdateDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.theme().tokens;
        let state = self.controller.read(cx).state().clone();

        let (title, detail, notes, action, action_label, action_disabled) = match &state {
            UpdateState::Disabled => (
                "Updates disabled".to_string(),
                "Automatic application updates are disabled in Nucleotide's configuration."
                    .to_string(),
                None,
                None,
                "".to_string(),
                false,
            ),
            UpdateState::Unsupported { .. } => (
                "Updates unavailable".to_string(),
                "This build was not installed by Velopack. Install a packaged Nucleotide release to receive application updates."
                    .to_string(),
                None,
                None,
                "".to_string(),
                false,
            ),
            UpdateState::Idle { .. } => (
                "Nucleotide updates".to_string(),
                format!("Current version: {}", env!("CARGO_PKG_VERSION")),
                None,
                Some(DialogAction::Check),
                "Check for Updates".to_string(),
                false,
            ),
            UpdateState::Checking { .. } => (
                "Checking for updates".to_string(),
                format!("Current version: {}", env!("CARGO_PKG_VERSION")),
                None,
                Some(DialogAction::Check),
                "Checking…".to_string(),
                true,
            ),
            UpdateState::UpToDate { .. } => (
                "Nucleotide is up to date".to_string(),
                format!("You are running version {}.", env!("CARGO_PKG_VERSION")),
                None,
                None,
                "".to_string(),
                false,
            ),
            UpdateState::Available(update) => (
                format!("Nucleotide {} is available", update.version),
                format!(
                    "Download {} in the background and keep working.",
                    format_download_size(update.download_bytes)
                ),
                nonempty_notes(&update.release_notes_markdown),
                Some(DialogAction::Download),
                "Download Update".to_string(),
                false,
            ),
            UpdateState::Downloading { update, percent } => (
                format!("Downloading Nucleotide {}", update.version),
                format!("{percent}% complete. You can keep working while the update downloads."),
                nonempty_notes(&update.release_notes_markdown),
                Some(DialogAction::Download),
                "Downloading…".to_string(),
                true,
            ),
            UpdateState::ReadyToRestart(update) => (
                "Update ready to install".to_string(),
                format!(
                    "Restart Nucleotide to finish updating to version {}.",
                    update.version
                ),
                nonempty_notes(&update.release_notes_markdown),
                Some(DialogAction::Restart),
                "Restart to Update".to_string(),
                false,
            ),
            UpdateState::Applying(update) => (
                "Preparing to restart".to_string(),
                format!("Nucleotide {} is ready to be applied.", update.version),
                nonempty_notes(&update.release_notes_markdown),
                None,
                "".to_string(),
                false,
            ),
            UpdateState::Failed {
                message,
                retryable,
                ..
            } => (
                "Update failed".to_string(),
                message.clone(),
                None,
                retryable.then_some(DialogAction::Retry),
                "Retry".to_string(),
                false,
            ),
        };

        let mut body = div()
            .flex()
            .flex_col()
            .gap(tokens.sizes.space_3)
            .child(
                div()
                    .text_size(tokens.sizes.text_lg)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tokens.chrome.text_on_chrome)
                    .child(title),
            )
            .child(
                div()
                    .text_size(tokens.sizes.text_sm)
                    .text_color(tokens.chrome.text_chrome_secondary)
                    .child(detail),
            );

        if let Some(notes) = notes {
            body = body.child(
                div()
                    .id("update-release-notes")
                    .mt_2()
                    .max_h(px(260.0))
                    .overflow_y_scroll()
                    .pr_2()
                    .child(markdown(
                        notes,
                        MarkdownStyle::from_tokens(&tokens).compact(),
                    )),
            );
        }

        let controller = self.controller.clone();
        let primary_button = action.map(|action| {
            Button::new("update-dialog-primary", action_label)
                .variant(ButtonVariant::Primary)
                .size(ButtonSize::Small)
                .icon(action.icon())
                .disabled(action_disabled)
                .focus_handle(self.primary_focus_handle.clone())
                .activate_on_mouse_down()
                .on_click(move |_event, window, cx| {
                    match action {
                        DialogAction::Check => controller.update(cx, |controller, cx| {
                            controller.check_now(cx);
                        }),
                        DialogAction::Download => controller.update(cx, |controller, cx| {
                            controller.download(cx);
                        }),
                        DialogAction::Retry => controller.update(cx, |controller, cx| {
                            controller.retry(cx);
                        }),
                        DialogAction::Restart => {
                            window.dispatch_action(Box::new(Restart), cx);
                        }
                    }
                    cx.stop_propagation();
                })
        });

        FocusTraversal::new(
            div()
                .track_focus(&self.focus_handle)
                .occlude()
                .w(px(560.0))
                .max_h(px(560.0))
                .p_5()
                .flex()
                .flex_col()
                .gap(tokens.sizes.space_4)
                .rounded_lg()
                .border_1()
                .border_color(tokens.chrome.border_strong)
                .bg(tokens.chrome.surface_elevated)
                .shadow(vec![tokens.chrome.shadow_lg.to_box_shadow(false)])
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .child(body)
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .items_center()
                        .gap(tokens.sizes.space_2)
                        .child(
                            Button::new("update-dialog-close", "Later")
                                .variant(ButtonVariant::Secondary)
                                .size(ButtonSize::Small)
                                .icon("icons/circle-x.svg")
                                .focus_handle(self.close_focus_handle.clone())
                                .activate_on_mouse_down()
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.dismiss(cx);
                                    cx.stop_propagation();
                                })),
                        )
                        .children(primary_button),
                ),
        )
    }
}

fn nonempty_notes(notes: &str) -> Option<String> {
    (!notes.trim().is_empty()).then(|| notes.to_string())
}

fn format_download_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KB", bytes / KIB)
    } else {
        format!("{} bytes", bytes as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_sizes_use_readable_binary_thresholds() {
        assert_eq!(format_download_size(512), "512 bytes");
        assert_eq!(format_download_size(1024), "1.0 KB");
        assert_eq!(format_download_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn blank_release_notes_are_omitted() {
        assert_eq!(nonempty_notes("  \n"), None);
        assert_eq!(nonempty_notes("## Fixed"), Some("## Fixed".to_string()));
    }

    #[test]
    fn update_actions_use_phosphor_icons() {
        assert_eq!(DialogAction::Check.icon(), "icons/rotate-ccw.svg");
        assert_eq!(DialogAction::Download.icon(), "icons/download.svg");
        assert_eq!(DialogAction::Retry.icon(), "icons/rotate-ccw.svg");
        assert_eq!(DialogAction::Restart.icon(), "icons/rotate-ccw.svg");
    }
}
