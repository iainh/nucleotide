//! Status bar model, background activity, and rendering.

use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder;
use gpui::{
    Context, FontWeight, IntoElement, ParentElement, Pixels, SharedString, Styled, Window, div, px,
    svg,
};
use nucleotide_lsp::{LspStatusKind, LspStatusSummary};
use nucleotide_ui::ThemedContext as UIThemedContext;
use nucleotide_ui::notification::{StatusBarNotification, StatusBarNotificationSeverity};
use nucleotide_ui::{
    Button, ButtonSize, ButtonVariant, IndeterminateProgressIndicator, StatusBar, Tooltipped,
};
use nucleotide_vcs::VcsServiceHandle;

use super::image_viewer::image_zoom_percent;
use super::{
    Workspace, macos_system_sidebar_enabled, should_extend_translucent_sidebar_into_status_bar,
};

const STATUSBAR_NOTIFICATION_MESSAGE_MAX_CHARS: usize = 64;

const STATUSBAR_BACKGROUND_ACTIVITY_MAX_CHARS: usize = 72;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct BackgroundActivityId(pub(super) u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BackgroundActivity {
    pub(super) id: BackgroundActivityId,
    pub(super) message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatusBarDensity {
    Wide,
    Medium,
    Compact,
}

impl StatusBarDensity {
    pub(super) fn for_available_width(available_width: f32) -> Self {
        if available_width >= 780.0 {
            Self::Wide
        } else if available_width >= 400.0 {
            Self::Medium
        } else {
            Self::Compact
        }
    }

    pub(super) fn lsp_width(self, sizes: &nucleotide_ui::tokens::SizeTokens) -> Pixels {
        match self {
            Self::Wide => sizes.statusbar_lsp_width_wide,
            Self::Medium => sizes.statusbar_lsp_width_medium,
            Self::Compact => sizes.statusbar_lsp_width_compact,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct StatusBarGeometry {
    pub(super) sidebar_width: Pixels,
    pub(super) density: StatusBarDensity,
    pub(super) mode_width: Pixels,
    pub(super) position_width: Pixels,
    pub(super) lsp_width: Pixels,
    pub(super) utility_width: Pixels,
}

impl StatusBarGeometry {
    pub(super) fn new(
        viewport_width: f32,
        show_file_tree: bool,
        file_tree_width: f32,
        sizes: &nucleotide_ui::tokens::SizeTokens,
    ) -> Self {
        let sidebar_width = if show_file_tree { file_tree_width } else { 0.0 };
        let available_width =
            (viewport_width - sidebar_width - f32::from(sizes.statusbar_utility_width)).max(0.0);
        let density = StatusBarDensity::for_available_width(available_width);
        Self {
            sidebar_width: px(sidebar_width),
            density,
            mode_width: sizes.statusbar_mode_width,
            position_width: sizes.statusbar_position_width,
            lsp_width: density.lsp_width(sizes),
            utility_width: sizes.statusbar_utility_width,
        }
    }
}

#[derive(Clone, Debug)]
struct StatusBarModel {
    mode: helix_view::document::Mode,
    mode_label: &'static str,
    file_name: String,
    position_text: String,
    document_metadata: Option<StatusBarDocumentMetadata>,
    vcs_ref: Option<String>,
    lsp: Option<LspStatusSummary>,
    notification: Option<StatusBarNotification>,
    background_activity: Option<String>,
    density: StatusBarDensity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusBarDocumentMetadata {
    encoding: String,
    line_ending: &'static str,
    language: String,
    errors: usize,
    warnings: usize,
}

pub(super) fn statusbar_line_ending_label(line_ending: &str) -> &'static str {
    match line_ending {
        "\r\n" => "CRLF",
        "\r" => "CR",
        "\x0b" => "VT",
        "\x0c" => "FF",
        "\u{0085}" => "NEL",
        "\u{2028}" => "LS",
        "\u{2029}" => "PS",
        _ => "LF",
    }
}

pub(super) fn statusbar_language_label(language: Option<&str>) -> String {
    let Some(language) = language.filter(|language| !language.is_empty()) else {
        return "Plain Text".to_string();
    };
    let mut chars = language.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_else(|| "Plain Text".to_string())
}

pub(super) fn statusbar_file_label(path: &Path) -> String {
    path.file_name()
        .map(|file_name| file_name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub(super) fn statusbar_show_secondary_document_metadata(
    density: StatusBarDensity,
    vcs_ref: Option<&str>,
) -> bool {
    density == StatusBarDensity::Wide && vcs_ref.is_none_or(|vcs_ref| vcs_ref.chars().count() <= 16)
}

pub(super) fn abbreviated_vcs_ref(head: &str) -> String {
    head.chars().take(8).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LspRestartPlan {
    pub(super) workspace_root: PathBuf,
    pub(super) server_count: usize,
}

pub(super) fn lsp_restart_plan(state: &nucleotide_lsp::LspState) -> Option<LspRestartPlan> {
    let session = state.project_session.as_ref()?;
    let server_count = session
        .servers
        .iter()
        .filter(|planned| {
            state
                .servers
                .values()
                .any(|server| server.name == planned.server_name)
        })
        .count();

    (server_count > 0).then(|| LspRestartPlan {
        workspace_root: session.workspace_root.clone(),
        server_count,
    })
}

pub(super) fn latest_background_activity(
    activities: &[BackgroundActivity],
) -> Option<&BackgroundActivity> {
    activities.last()
}

pub(super) fn shorten_statusbar_text(text: &str, max_chars: usize) -> String {
    debug_assert!(max_chars >= 2);

    let mut normalized = String::new();
    let mut saw_whitespace = false;

    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if !saw_whitespace && !normalized.is_empty() {
                normalized.push(' ');
            }
            saw_whitespace = true;
        } else {
            normalized.push(ch);
            saw_whitespace = false;
        }
    }

    if normalized.chars().count() <= max_chars {
        return normalized;
    }

    let mut shortened: String = normalized.chars().take(max_chars - 1).collect();
    shortened.push('…');
    shortened
}

pub(super) fn statusbar_notification_icon(severity: StatusBarNotificationSeverity) -> &'static str {
    match severity {
        StatusBarNotificationSeverity::Info => "icons/info.svg",
        StatusBarNotificationSeverity::Success => "icons/square-check-big.svg",
        StatusBarNotificationSeverity::Warning => "icons/triangle-alert.svg",
        StatusBarNotificationSeverity::Error => "icons/circle-x.svg",
    }
}

fn short_lsp_server_name(server_name: &str) -> String {
    match server_name {
        "rust-analyzer" => "rust".to_string(),
        "typescript-language-server" => "ts".to_string(),
        "java-language-server" => "java".to_string(),
        name if name.chars().count() > 8 => {
            let prefix = name.chars().take(6).collect::<String>();
            format!("{prefix}…")
        }
        name => name.to_string(),
    }
}

pub(super) fn statusbar_lsp_summary_for_state(
    state: &nucleotide_lsp::LspState,
    preferred_server_id: Option<helix_lsp::LanguageServerId>,
) -> LspStatusSummary {
    state.status_summary(preferred_server_id)
}

impl Workspace {
    pub(super) fn start_background_activity(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> BackgroundActivityId {
        let id = BackgroundActivityId(self.next_background_activity_id);
        self.next_background_activity_id = self.next_background_activity_id.wrapping_add(1).max(1);
        self.background_activities.push(BackgroundActivity {
            id,
            message: message.into(),
        });
        cx.notify();
        id
    }

    pub(super) fn update_background_activity(
        &mut self,
        id: BackgroundActivityId,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        if let Some(activity) = self
            .background_activities
            .iter_mut()
            .find(|activity| activity.id == id)
            && activity.message != message
        {
            activity.message = message;
            cx.notify();
        }
    }

    pub(super) fn finish_background_activity(
        &mut self,
        id: BackgroundActivityId,
        cx: &mut Context<Self>,
    ) {
        let previous_len = self.background_activities.len();
        self.background_activities
            .retain(|activity| activity.id != id);
        if self.background_activities.len() != previous_len {
            cx.notify();
        }
    }

    fn current_background_activity(&self) -> Option<&BackgroundActivity> {
        latest_background_activity(&self.background_activities)
    }

    /// Compute document and LSP context for the status bar without triggering borrow conflicts.
    fn statusbar_doc_info(
        &self,
        cx: &mut Context<Self>,
    ) -> (
        helix_view::document::Mode,          // current mode
        &'static str,                        // mode label
        String,                              // file name display
        String,                              // position text
        Option<StatusBarDocumentMetadata>,   // encoding, language, diagnostics
        bool,                                // has LSP state
        Option<helix_lsp::LanguageServerId>, // preferred server for current doc
    ) {
        let core = self.core.read(cx);
        let editor = &core.editor;

        let mut mode = helix_view::document::Mode::Normal;
        let mut mode_name = "NORMAL";
        let mut file_name = "[no file]".to_string();
        let mut position_text = "1:1".to_string();
        let mut document_metadata = None;

        if let Some(tab) = self
            .active_image_id()
            .and_then(|doc_id| self.image_tabs.iter().find(|tab| tab.id == doc_id))
        {
            file_name = tab.path.display().to_string();
            position_text = image_zoom_percent(
                self.image_presentations
                    .get(self.focused_pane_id(), tab.id)
                    .map(|state| state.zoom)
                    .unwrap_or(1.0),
            );
            return (mode, mode_name, file_name, position_text, None, false, None);
        }

        // Get info from focused view if available
        if let Some(view_id) = self.view_manager.focused_view_id()
            && let Some((view, doc)) = editor
                .tree
                .try_get(view_id)
                .and_then(|v| editor.document(v.doc).map(|d| (v, d)))
        {
            mode = editor.mode();
            mode_name = match mode {
                helix_view::document::Mode::Normal => "NORMAL",
                helix_view::document::Mode::Insert => "INSERT",
                helix_view::document::Mode::Select => "SELECT",
            };

            file_name = doc
                .path()
                .map(statusbar_file_label)
                .unwrap_or_else(|| "[scratch]".to_string());

            let position = helix_core::coords_at_pos(
                doc.text().slice(..),
                doc.selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
            );
            position_text = format!("{}:{}", position.row + 1, position.col + 1);
            document_metadata = Some(StatusBarDocumentMetadata {
                encoding: doc.encoding().name().to_string(),
                line_ending: statusbar_line_ending_label(doc.line_ending.as_str()),
                language: statusbar_language_label(doc.language_name()),
                errors: doc
                    .diagnostics()
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.severity == Some(helix_core::diagnostic::Severity::Error)
                    })
                    .count(),
                warnings: doc
                    .diagnostics()
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.severity == Some(helix_core::diagnostic::Severity::Warning)
                    })
                    .count(),
            });
        }

        // Determine preferred LSP server for the current document
        let preferred_server_id = if let Some(view_id) = self.view_manager.focused_view_id()
            && let Some(view) = editor.tree.try_get(view_id)
            && let Some(doc) = editor.document(view.doc)
        {
            doc.language_servers().next().map(|ls| ls.id())
        } else {
            None
        };

        let has_lsp_state = core.lsp_state.is_some();
        (
            mode,
            mode_name,
            file_name,
            position_text,
            document_metadata,
            has_lsp_state,
            preferred_server_id,
        )
    }

    /// Build structured LSP status, preferring active work over the focused document server.
    fn compute_statusbar_lsp_summary(
        &self,
        cx: &mut Context<Self>,
        has_lsp_state: bool,
        preferred_server_id: Option<helix_lsp::LanguageServerId>,
    ) -> Option<LspStatusSummary> {
        if !has_lsp_state {
            return None;
        }

        let lsp_state_entity = {
            let core = self.core.read(cx);
            core.lsp_state.clone()
        }?;

        Some(statusbar_lsp_summary_for_state(
            lsp_state_entity.read(cx),
            preferred_server_id,
        ))
    }

    fn statusbar_model(&self, density: StatusBarDensity, cx: &mut Context<Self>) -> StatusBarModel {
        let (
            mode,
            mode_label,
            file_name,
            position_text,
            document_metadata,
            has_lsp_state,
            preferred_server_id,
        ) = self.statusbar_doc_info(cx);
        StatusBarModel {
            mode,
            mode_label,
            file_name,
            position_text,
            document_metadata,
            vcs_ref: cx
                .has_global::<VcsServiceHandle>()
                .then(|| {
                    let vcs = cx.global::<VcsServiceHandle>();
                    vcs.repository_ref(cx).or_else(|| {
                        vcs.repository_head(cx)
                            .map(|head| abbreviated_vcs_ref(&head))
                    })
                })
                .flatten(),
            lsp: self.compute_statusbar_lsp_summary(cx, has_lsp_state, preferred_server_id),
            notification: self.notifications.read(cx).status_bar_notification(),
            background_activity: self
                .current_background_activity()
                .map(|activity| activity.message.clone()),
            density,
        }
    }

    fn statusbar_text_item(
        &self,
        text: impl Into<SharedString>,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
    ) -> gpui::AnyElement {
        div()
            .h_full()
            .px_2()
            .max_w(px(120.0))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(status_bar_tokens.text_secondary)
            .child(text.into())
            .into_any_element()
    }

    fn statusbar_diagnostics_item(
        &self,
        metadata: &StatusBarDocumentMetadata,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
    ) -> gpui::AnyElement {
        div()
            .h_full()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .text_color(status_bar_tokens.text_secondary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        svg()
                            .path("icons/triangle-alert.svg")
                            .size(px(12.0))
                            .text_color(status_bar_tokens.warning),
                    )
                    .child(metadata.warnings.to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        svg()
                            .path("icons/circle-x.svg")
                            .size(px(12.0))
                            .text_color(status_bar_tokens.error),
                    )
                    .child(metadata.errors.to_string()),
            )
            .into_any_element()
    }

    fn statusbar_vcs_item(
        &self,
        vcs_ref: &str,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
    ) -> gpui::AnyElement {
        div()
            .h_full()
            .max_w(px(200.0))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .overflow_hidden()
            .text_color(status_bar_tokens.text_secondary)
            .child(
                svg()
                    .path("icons/git-branch.svg")
                    .size(px(12.0))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(vcs_ref.to_string()),
            )
            .into_any_element()
    }

    fn statusbar_mode_item(
        &self,
        model: &StatusBarModel,
        geometry: StatusBarGeometry,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
    ) -> gpui::AnyElement {
        let (mode_color, mode_text) = match model.mode {
            helix_view::document::Mode::Normal => (
                status_bar_tokens.mode_normal,
                status_bar_tokens.mode_normal_text,
            ),
            helix_view::document::Mode::Insert => (
                status_bar_tokens.mode_insert,
                status_bar_tokens.mode_insert_text,
            ),
            helix_view::document::Mode::Select => (
                status_bar_tokens.mode_select,
                status_bar_tokens.mode_select_text,
            ),
        };
        div()
            .flex_none()
            .w(geometry.mode_width)
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .size_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(mode_color)
                    .text_color(mode_text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(model.mode_label),
            )
            .into_any_element()
    }

    fn statusbar_lsp_item(
        &self,
        summary: Option<&LspStatusSummary>,
        density: StatusBarDensity,
        geometry: StatusBarGeometry,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let unavailable = LspStatusSummary {
            server_id: None,
            server_name: "LSP:".to_string(),
            is_remote: false,
            kind: LspStatusKind::Unavailable,
            title: "No servers".to_string(),
            message: None,
            percentage: None,
            server_count: 0,
        };
        let summary = summary.unwrap_or(&unavailable);
        let full_text = summary.full_text();
        let accessible_label = format!("Language server status: {full_text}");
        let server_width = match density {
            StatusBarDensity::Wide => px(88.0),
            StatusBarDensity::Medium => px(62.0),
            StatusBarDensity::Compact => px(0.0),
        };
        let server_name = match density {
            StatusBarDensity::Wide => summary.server_name.clone(),
            StatusBarDensity::Medium => short_lsp_server_name(&summary.server_name),
            StatusBarDensity::Compact => String::new(),
        };
        let activity = summary
            .message
            .as_deref()
            .filter(|message| !message.is_empty())
            .unwrap_or(&summary.title)
            .to_string();
        let icon_color = match summary.kind {
            LspStatusKind::Failed => cx.theme().tokens.notification_tokens().error_text,
            LspStatusKind::Working | LspStatusKind::Starting => status_bar_tokens.text_accent,
            LspStatusKind::Idle => status_bar_tokens.text_primary,
            LspStatusKind::Unavailable => status_bar_tokens.text_secondary,
        };

        let state_icon = match summary.kind {
            LspStatusKind::Working | LspStatusKind::Starting => {
                IndeterminateProgressIndicator::new("statusbar-lsp-spinner")
                    .size(13.0)
                    .text_color(icon_color)
                    .into_any_element()
            }
            LspStatusKind::Failed => svg()
                .path("icons/circle-x.svg")
                .size(px(13.0))
                .text_color(icon_color)
                .into_any_element(),
            LspStatusKind::Idle => div().text_color(icon_color).child("●").into_any_element(),
            LspStatusKind::Unavailable => {
                div().text_color(icon_color).child("○").into_any_element()
            }
        };

        let mut content = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .w(px(14.0))
                    .flex()
                    .justify_center()
                    .child(state_icon),
            );

        if summary.is_remote && density != StatusBarDensity::Compact {
            content = content.child(
                svg()
                    .path("icons/monitor.svg")
                    .size(px(13.0))
                    .text_color(status_bar_tokens.text_secondary)
                    .flex_shrink_0(),
            );
        }

        if density != StatusBarDensity::Compact {
            let percentage = summary
                .percentage
                .map(|percentage| format!("{percentage:>3}%"))
                .unwrap_or_default();
            content = content
                .child(
                    div()
                        .flex_none()
                        .w(server_width)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_weight(FontWeight::MEDIUM)
                        .child(server_name),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(status_bar_tokens.text_secondary)
                        .child(activity),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(34.0))
                        .flex()
                        .justify_end()
                        .text_color(status_bar_tokens.text_secondary)
                        .child(percentage),
                );
        }

        if density != StatusBarDensity::Compact {
            content = content.child(
                svg()
                    .path("icons/chevron-up.svg")
                    .size(px(12.0))
                    .text_color(status_bar_tokens.text_secondary)
                    .flex_shrink_0(),
            );
        }

        Button::new("lsp-status-trigger", "")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::ExtraSmall)
            .width(geometry.lsp_width)
            .content(content)
            .tooltip(full_text)
            .aria_label(accessible_label)
            .focus_handle(self.statusbar_lsp_focus.clone())
            .activate_on_mouse_down()
            .on_click(
                cx.listener(|this: &mut Workspace, ev: &gpui::ClickEvent, _window, cx| {
                    this.lsp_menu_open = true;
                    let position = ev.position();
                    this.lsp_menu_pos = (f32::from(position.x), f32::from(position.y));
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Build the editor-aligned content lane for the unified status bar.
    fn statusbar_main_content(
        &self,
        model: &StatusBarModel,
        geometry: StatusBarGeometry,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut context = div().flex_none().h_full().flex().items_center();

        if model.density != StatusBarDensity::Compact {
            context = context.child(
                div()
                    .flex_none()
                    .w(geometry.position_width)
                    .h_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_end()
                    .whitespace_nowrap()
                    .text_color(status_bar_tokens.text_secondary)
                    .child(model.position_text.clone()),
            );
        }

        if model.density == StatusBarDensity::Wide
            && let Some(metadata) = model.document_metadata.as_ref()
        {
            if statusbar_show_secondary_document_metadata(model.density, model.vcs_ref.as_deref()) {
                context = context
                    .child(self.statusbar_text_item(metadata.encoding.clone(), status_bar_tokens))
                    .child(self.statusbar_text_item(metadata.line_ending, status_bar_tokens));
            }
            context = context
                .child(self.statusbar_text_item(metadata.language.clone(), status_bar_tokens));
        }

        context = context.child(self.statusbar_lsp_item(
            model.lsp.as_ref(),
            model.density,
            geometry,
            status_bar_tokens,
            cx,
        ));

        if model.density == StatusBarDensity::Wide {
            if let Some(metadata) = model.document_metadata.as_ref() {
                context =
                    context.child(self.statusbar_diagnostics_item(metadata, status_bar_tokens));
            }
        }

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .flex_row()
            .items_center()
            .h_full()
            .child(self.statusbar_mode_item(model, geometry, status_bar_tokens))
            .when(model.density == StatusBarDensity::Wide, |content| {
                content.when_some(model.vcs_ref.as_ref(), |content, vcs_ref| {
                    content.child(self.statusbar_vcs_item(vcs_ref, status_bar_tokens))
                })
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_3()
                    .child(self.statusbar_message_slot(
                        model.file_name.clone(),
                        model.notification.clone(),
                        model.background_activity.clone(),
                        status_bar_tokens,
                        cx,
                    )),
            )
            .child(context)
            .into_any_element()
    }

    fn statusbar_message_slot(
        &self,
        file_name: String,
        notification: Option<StatusBarNotification>,
        background_activity: Option<String>,
        status_bar_tokens: &nucleotide_ui::tokens::StatusBarTokens,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if let Some(activity_message) = background_activity {
            let notification_tokens = cx.theme().tokens.notification_tokens();
            let message =
                shorten_statusbar_text(&activity_message, STATUSBAR_BACKGROUND_ACTIVITY_MAX_CHARS);

            return gpui::div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .overflow_hidden()
                .child(
                    IndeterminateProgressIndicator::new("statusbar-background-activity")
                        .size(13.0)
                        .text_color(notification_tokens.info_text),
                )
                .child(
                    gpui::div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(status_bar_tokens.text_primary)
                        .child(message),
                )
                .into_any_element();
        }

        let Some(notification) = notification else {
            return gpui::div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(file_name)
                .into_any_element();
        };

        let notification_tokens = cx.theme().tokens.notification_tokens();
        let label_color = match notification.severity {
            StatusBarNotificationSeverity::Info => notification_tokens.info_text,
            StatusBarNotificationSeverity::Success => notification_tokens.success_text,
            StatusBarNotificationSeverity::Warning => notification_tokens.warning_text,
            StatusBarNotificationSeverity::Error => notification_tokens.error_text,
        };
        let status_icon = statusbar_notification_icon(notification.severity);
        let message = shorten_statusbar_text(
            &notification.message,
            STATUSBAR_NOTIFICATION_MESSAGE_MAX_CHARS,
        );

        gpui::div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_2()
            .overflow_hidden()
            .child(
                svg()
                    .path(status_icon)
                    .size(px(14.0))
                    .flex_shrink_0()
                    .text_color(label_color),
            )
            .child(
                gpui::div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(status_bar_tokens.text_primary)
                    .child(message),
            )
            .into_any_element()
    }

    /// Render unified status bar with file tree toggle and status information
    pub(super) fn render_unified_status_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (status_bar_tokens, translucent_file_tree_tokens, sizes) = {
            let ui_theme = cx.global::<nucleotide_ui::Theme>();
            (
                ui_theme.tokens.status_bar_tokens(),
                ui_theme.tokens.file_tree_tokens().translucent_sidebar(),
                ui_theme.tokens.sizes,
            )
        };
        let viewport_width = f32::from(window.viewport_size().width);
        let native_sidebar_enabled = macos_system_sidebar_enabled(&self.core.read(cx).config.gui);
        let geometry = StatusBarGeometry::new(
            viewport_width,
            self.show_file_tree && native_sidebar_enabled,
            self.file_tree_width,
            &sizes,
        );
        let model = self.statusbar_model(geometry.density, cx);
        let chrome_metrics =
            nucleotide_ui::DensityMetrics::for_density(nucleotide_ui::ControlDensity::Comfortable);
        let divider_color = status_bar_tokens.border;
        let extend_sidebar_into_status_bar = should_extend_translucent_sidebar_into_status_bar(
            self.show_file_tree,
            self.file_tree_width,
            native_sidebar_enabled,
        );
        let sidebar_background = if extend_sidebar_into_status_bar {
            translucent_file_tree_tokens.background
        } else {
            status_bar_tokens.background_active
        };

        let leading = div().size_full().bg(sidebar_background);

        let content = div()
            .size_full()
            .when(extend_sidebar_into_status_bar, |content| {
                content.border_t_1().border_color(divider_color)
            })
            .child(self.statusbar_main_content(&model, geometry, &status_bar_tokens, cx));

        let workspace_entity = cx.entity().clone();
        let file_tree_button = Button::icon_only("file-tree-toggle", "icons/folder-tree.svg")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::ExtraSmall)
            .tooltip("Toggle File Tree")
            .aria_label("Toggle file tree")
            .focus_handle(self.statusbar_file_tree_focus.clone())
            .activate_on_mouse_down()
            .on_click(move |_event, _window, app_cx| {
                workspace_entity.update(app_cx, |workspace, cx| {
                    workspace.show_file_tree = !workspace.show_file_tree;
                    cx.notify();
                });
            });

        let workspace_entity = cx.entity().clone();
        let terminal_button = Button::icon_only("terminal-toggle", "icons/terminal.svg")
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::ExtraSmall)
            .tooltip("New Terminal")
            .aria_label("New terminal")
            .focus_handle(self.statusbar_terminal_focus.clone())
            .activate_on_mouse_down()
            .on_click(move |_event, _window, app_cx| {
                workspace_entity.update(app_cx, |workspace, cx| {
                    workspace.tab_bar_action_new_terminal(cx);
                });
            });

        let utilities = div()
            .w(geometry.utility_width)
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .when(extend_sidebar_into_status_bar, |utilities| {
                utilities.border_t_1().border_color(divider_color)
            })
            .child(
                div()
                    .w(chrome_metrics.row_height)
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(file_tree_button),
            )
            .child(
                div()
                    .w(chrome_metrics.row_height)
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(terminal_button),
            );

        StatusBar::new("workspace-status-bar")
            .top_border(!extend_sidebar_into_status_bar)
            .leading(geometry.sidebar_width, leading)
            .content(content)
            .trailing(utilities)
    }
}
