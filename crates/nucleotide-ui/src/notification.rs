use std::collections::HashMap;

use crate::Theme;
use gpui::{
    App, Context, DefiniteLength, FontWeight, IntoElement, ParentElement, Render, RenderOnce,
    Result, Styled, Window, div, prelude::FluentBuilder, px,
};
use helix_lsp::LanguageServerId;
use helix_view::document::DocumentSavedEvent;
use nucleotide_types::EditorStatus;

#[derive(Debug, Clone, Copy)]
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

#[derive(IntoElement)]
struct Notification {
    title: String,
    message: Option<String>,
    severity: NotificationSeverity,
}

impl Notification {
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

        Notification {
            title,
            message: Some(message),
            severity,
        }
    }

    fn from_editor_status(status: &EditorStatus) -> Self {
        use nucleotide_types::Severity;
        let (title, severity) = match status.severity {
            Severity::Info => ("info", NotificationSeverity::Info),
            Severity::Hint => ("hint", NotificationSeverity::Info),
            Severity::Error => ("error", NotificationSeverity::Error),
            Severity::Warning => ("warning", NotificationSeverity::Warning),
        };

        Notification {
            title: title.to_string(),
            message: Some(status.status.clone()),
            severity,
        }
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
        Notification {
            title,
            message: status.message.clone(),
            severity: NotificationSeverity::Info, // LSP notifications are typically informational
        }
    }
}

pub struct NotificationView {
    lsp_status: HashMap<LanguageServerId, LspStatus>,
    editor_status: Option<EditorStatus>,
    saved: Option<Result<DocumentSavedEvent, String>>,
}

impl Default for NotificationView {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationView {
    pub fn new() -> Self {
        Self {
            saved: None,
            editor_status: None,
            lsp_status: HashMap::new(),
        }
    }
}

impl Render for NotificationView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut notifications = vec![];
        for status in self.lsp_status.values() {
            if status.is_empty() {
                continue;
            }
            notifications.push(Notification::from_lsp(status));
        }
        if let Some(status) = &self.editor_status {
            notifications.push(Notification::from_editor_status(status));
        }
        if let Some(saved) = self.saved.take() {
            notifications.push(Notification::from_save_event(&saved));
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
            .shadow_sm()
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
