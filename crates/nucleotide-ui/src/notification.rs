use std::time::Duration;

use crate::Theme;
use gpui::{
    App, Context, FontWeight, IntoElement, ParentElement, Render, RenderOnce, Result, Styled,
    Window, div, prelude::FluentBuilder, px,
};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationPlacement {
    StatusLine,
    Banner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBarNotificationSeverity {
    Info,
    Success,
    Warning,
    Error,
}

impl From<NotificationSeverity> for StatusBarNotificationSeverity {
    fn from(severity: NotificationSeverity) -> Self {
        match severity {
            NotificationSeverity::Info => Self::Info,
            NotificationSeverity::Success => Self::Success,
            NotificationSeverity::Warning => Self::Warning,
            NotificationSeverity::Error => Self::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusBarNotification {
    pub label: String,
    pub message: String,
    pub severity: StatusBarNotificationSeverity,
}

impl StatusBarNotification {
    fn from_notification(notification: &Notification) -> Self {
        Self {
            label: notification.title.to_uppercase(),
            message: notification
                .message
                .clone()
                .unwrap_or_else(|| notification.title.clone()),
            severity: notification.severity.into(),
        }
    }
}

#[derive(Clone)]
struct Notification {
    id: u64,
    title: String,
    message: Option<String>,
    severity: NotificationSeverity,
    placement: NotificationPlacement,
}

impl Notification {
    fn new(
        title: impl Into<String>,
        message: Option<String>,
        severity: NotificationSeverity,
        placement: NotificationPlacement,
    ) -> Self {
        Self {
            id: 0,
            title: title.into(),
            message,
            severity,
            placement,
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

        Notification::new(
            title,
            Some(message),
            severity,
            NotificationPlacement::StatusLine,
        )
    }

    fn from_editor_status(status: &EditorStatus) -> Self {
        use nucleotide_types::Severity;
        let (title, severity) = match status.severity {
            Severity::Info => ("info", NotificationSeverity::Info),
            Severity::Hint => ("hint", NotificationSeverity::Info),
            Severity::Error => ("error", NotificationSeverity::Error),
            Severity::Warning => ("warning", NotificationSeverity::Warning),
        };

        Notification::new(
            title,
            Some(status.status.clone()),
            severity,
            NotificationPlacement::StatusLine,
        )
    }
}

#[derive(IntoElement)]
struct BannerNotification(Notification);

pub struct NotificationView {
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
            Notification::new(
                title,
                Some(message.into()),
                NotificationSeverity::Success,
                NotificationPlacement::StatusLine,
            ),
            cx,
        );
    }

    pub fn push_banner(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        status: EditorStatus,
        cx: &mut Context<Self>,
    ) {
        let notification = Notification::from_editor_status(&status);
        self.push_notification(
            Notification::new(
                title,
                Some(message.into()),
                notification.severity,
                NotificationPlacement::Banner,
            ),
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

    pub fn status_bar_notification(&self) -> Option<StatusBarNotification> {
        self.transient_notifications
            .iter()
            .rev()
            .find(|notification| notification.placement == NotificationPlacement::StatusLine)
            .map(StatusBarNotification::from_notification)
    }
}

impl Render for NotificationView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let banners = self
            .transient_notifications
            .iter()
            .filter(|notification| notification.placement == NotificationPlacement::Banner)
            .cloned()
            .collect::<Vec<_>>();

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .when(!banners.is_empty(), |view| {
                view.child(
                    div()
                        .flex()
                        .flex_col()
                        .children(banners.into_iter().map(BannerNotification)),
                )
            })
    }
}

impl RenderOnce for BannerNotification {
    fn render(mut self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let notification_tokens = theme.tokens.notification_tokens();
        let message = self.0.message.take();

        let (bg_color, text_color, border_color) = match self.0.severity {
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
            .gap_1()
            .min_h(px(36.0))
            .w_full()
            .px_3()
            .py_2()
            .bg(bg_color)
            .text_color(text_color)
            .border_b_1()
            .border_color(border_color)
            .font(
                cx.global::<nucleotide_types::FontSettings>()
                    .var_font
                    .clone()
                    .into(),
            )
            .text_size(px(cx.global::<nucleotide_types::UiFontConfig>().size - 1.0))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.0.title),
            )
            .when_some(message, |banner, message| {
                banner.child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(message),
                )
            })
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
        assert_eq!(notification.placement, NotificationPlacement::StatusLine);
    }

    #[test]
    fn dismiss_notification_removes_matching_entry_only() {
        let mut view = NotificationView::new();
        view.transient_notifications.push(Notification {
            id: 1,
            title: "first".to_string(),
            message: None,
            severity: NotificationSeverity::Info,
            placement: NotificationPlacement::StatusLine,
        });
        view.transient_notifications.push(Notification {
            id: 2,
            title: "second".to_string(),
            message: None,
            severity: NotificationSeverity::Warning,
            placement: NotificationPlacement::Banner,
        });

        view.dismiss_notification(1);

        assert_eq!(view.transient_notifications.len(), 1);
        assert_eq!(view.transient_notifications[0].id, 2);
    }

    #[test]
    fn status_bar_notification_returns_latest_status_line() {
        let mut view = NotificationView::new();
        view.transient_notifications.push(Notification {
            id: 1,
            title: "info".to_string(),
            message: Some("older".to_string()),
            severity: NotificationSeverity::Info,
            placement: NotificationPlacement::StatusLine,
        });
        view.transient_notifications.push(Notification {
            id: 2,
            title: "warning".to_string(),
            message: Some("banner".to_string()),
            severity: NotificationSeverity::Warning,
            placement: NotificationPlacement::Banner,
        });
        view.transient_notifications.push(Notification {
            id: 3,
            title: "error".to_string(),
            message: Some("latest".to_string()),
            severity: NotificationSeverity::Error,
            placement: NotificationPlacement::StatusLine,
        });

        let notification = view.status_bar_notification().unwrap();

        assert_eq!(notification.label, "ERROR");
        assert_eq!(notification.message, "latest");
        assert_eq!(notification.severity, StatusBarNotificationSeverity::Error);
    }
}
