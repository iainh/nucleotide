use std::collections::HashMap;
use std::time::Duration;

use crate::Theme;
use gpui::{
    App, Context, DefiniteLength, FontWeight, IntoElement, ParentElement, Render, RenderOnce,
    Result, Styled, Window, div, prelude::FluentBuilder, px,
};
use helix_lsp::LanguageServerId;
use helix_view::document::DocumentSavedEvent;
use nucleotide_types::EditorStatus;

const DEFAULT_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TRANSIENT_NOTIFICATIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationSeverity {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Default, Debug)]
struct LspStatus {
    token: String,
    title: String,
    message: Option<String>,
    percentage: Option<u32>,
}

impl LspStatus {
    fn is_empty(&self) -> bool {
        self.token.is_empty() && self.title.is_empty() && self.message.is_none()
    }
}

#[derive(Clone, IntoElement)]
struct Notification {
    id: u64,
    title: String,
    message: Option<String>,
    severity: NotificationSeverity,
}

impl Notification {
    fn new(
        title: impl Into<String>,
        message: Option<String>,
        severity: NotificationSeverity,
    ) -> Self {
        Self {
            id: 0,
            title: title.into(),
            message,
            severity,
        }
    }

    fn from_save_event(event: &Result<DocumentSavedEvent, String>) -> Self {
        let (title, message, severity) = match event {
            Ok(saved) => (
                "Saved".to_string(),
                format!("saved to {}", saved.path.display()),
                NotificationSeverity::Success,
            ),
            Err(err) => (
                "Error".to_string(),
                format!("error saving: {err}"),
                NotificationSeverity::Error,
            ),
        };

        Notification::new(title, Some(message), severity)
    }

    fn from_editor_status(status: &EditorStatus) -> Self {
        use nucleotide_types::Severity;
        let (title, severity) = match status.severity {
            Severity::Info => ("info", NotificationSeverity::Info),
            Severity::Hint => ("hint", NotificationSeverity::Info),
            Severity::Error => ("error", NotificationSeverity::Error),
            Severity::Warning => ("warning", NotificationSeverity::Warning),
        };

        Notification::new(title, Some(status.status.clone()), severity)
    }

    fn from_lsp(status: &LspStatus) -> Self {
        let title = format!(
            "{}: {} {}",
            status.token,
            status.title,
            status
                .percentage
                .map(|s| format!("{s}%"))
                .unwrap_or_default()
        );
        Notification::new(
            title,
            status.message.clone(),
            NotificationSeverity::Info, // LSP notifications are typically informational
        )
    }
}

pub struct NotificationView {
    lsp_status: HashMap<LanguageServerId, LspStatus>,
    transient_notifications: Vec<Notification>,
    next_notification_id: u64,
}

impl Default for NotificationView {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationView {
    pub fn new() -> Self {
        Self {
            lsp_status: HashMap::new(),
            transient_notifications: Vec::new(),
            next_notification_id: 1,
        }
    }

    pub fn push_editor_status(&mut self, status: EditorStatus, cx: &mut Context<Self>) {
        if status.status.trim().is_empty() {
            return;
        }

        self.push_notification(Notification::from_editor_status(&status), cx);
    }

    pub fn push_document_saved(
        &mut self,
        event: Result<DocumentSavedEvent, String>,
        cx: &mut Context<Self>,
    ) {
        self.push_notification(Notification::from_save_event(&event), cx);
    }

    pub fn push_success(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.push_notification(
            Notification::new(title, Some(message.into()), NotificationSeverity::Success),
            cx,
        );
    }

    fn push_notification(&mut self, mut notification: Notification, cx: &mut Context<Self>) {
        notification.id = self.next_notification_id;
        self.next_notification_id = self.next_notification_id.wrapping_add(1).max(1);
        let notification_id = notification.id;

        self.transient_notifications.push(notification);
        while self.transient_notifications.len() > MAX_TRANSIENT_NOTIFICATIONS {
            self.transient_notifications.remove(0);
        }

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(DEFAULT_NOTIFICATION_TIMEOUT)
                .await;

            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    view.dismiss_notification(notification_id);
                    cx.notify();
                });
            }
        })
        .detach();

        cx.notify();
    }

    fn dismiss_notification(&mut self, notification_id: u64) {
        if let Some(index) = self
            .transient_notifications
            .iter()
            .position(|notification| notification.id == notification_id)
        {
            self.transient_notifications.remove(index);
        }
    }
}

impl Render for NotificationView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut notifications = self.transient_notifications.clone();
        for status in self.lsp_status.values() {
            if status.is_empty() {
                continue;
            }
            notifications.push(Notification::from_lsp(status));
        }
        div()
            .absolute()
            .w(DefiniteLength::Fraction(0.33))
            .top_8()
            .right_5()
            .flex_col()
            .gap_8()
            .justify_start()
            .items_center()
            .children(notifications)
    }
}

impl RenderOnce for Notification {
    fn render(mut self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let notification_tokens = theme.tokens.notification_tokens();
        let message = self.message.take();

        // Select colors based on notification severity using hybrid tokens
        let (bg_color, text_color, border_color) = match self.severity {
            NotificationSeverity::Info => (
                notification_tokens.info_background,
                notification_tokens.info_text,
                notification_tokens.info_border,
            ),
            NotificationSeverity::Success => (
                notification_tokens.success_background,
                notification_tokens.success_text,
                notification_tokens.success_border,
            ),
            NotificationSeverity::Warning => (
                notification_tokens.warning_background,
                notification_tokens.warning_text,
                notification_tokens.warning_border,
            ),
            NotificationSeverity::Error => (
                notification_tokens.error_background,
                notification_tokens.error_text,
                notification_tokens.error_border,
            ),
        };

        div()
            .flex()
            .flex_col()
            .flex()
            .p_2()
            .gap_4()
            .min_h(px(100.))
            .bg(bg_color)
            .text_color(text_color)
            .border_1()
            .border_color(border_color)
            .shadow(vec![
                theme.tokens.chrome.shadow_sm.to_box_shadow(false),
                theme.tokens.chrome.inset_highlight.to_box_shadow(true),
            ])
            .rounded_sm()
            .font(
                cx.global::<nucleotide_types::FontSettings>()
                    .var_font
                    .clone()
                    .into(),
            )
            .text_size(px(cx.global::<nucleotide_types::UiFontConfig>().size - 1.0))
            .child(
                div()
                    .flex()
                    .font_weight(FontWeight::BOLD)
                    .flex_none()
                    .justify_center()
                    .items_center()
                    .child(self.title),
            )
            .when_some(message, gpui::ParentElement::child)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nucleotide_types::Severity;

    #[test]
    fn editor_status_maps_error_to_error_notification() {
        let status = EditorStatus {
            status: "1 unsaved buffer remaining: [\"main.rs\"]".to_string(),
            severity: Severity::Error,
        };

        let notification = Notification::from_editor_status(&status);

        assert_eq!(notification.title, "error");
        assert_eq!(
            notification.message.as_deref(),
            Some(status.status.as_str())
        );
        assert_eq!(notification.severity, NotificationSeverity::Error);
    }

    #[test]
    fn dismiss_notification_removes_matching_entry_only() {
        let mut view = NotificationView::new();
        view.transient_notifications.push(Notification {
            id: 1,
            title: "first".to_string(),
            message: None,
            severity: NotificationSeverity::Info,
        });
        view.transient_notifications.push(Notification {
            id: 2,
            title: "second".to_string(),
            message: None,
            severity: NotificationSeverity::Warning,
        });

        view.dismiss_notification(1);

        assert_eq!(view.transient_notifications.len(), 1);
        assert_eq!(view.transient_notifications[0].id, 2);
    }
}
