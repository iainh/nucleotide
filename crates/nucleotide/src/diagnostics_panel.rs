// ABOUTME: Diagnostics list panel rendering diagnostics from LspState with simple filters

use gpui::{prelude::FluentBuilder, *};
use helix_core::{Uri, diagnostic::Severity};
use nucleotide_lsp::lsp_state::DiagnosticInfo;
use nucleotide_ui::ThemedContext;
use nucleotide_ui::common::FocusableModal;
use nucleotide_ui::theme_manager::HelixThemedContext;
use nucleotide_ui::{ListItem, ListItemSpacing, ListItemVariant};
use std::path::PathBuf;

/// Simple filters for the diagnostics panel
#[derive(Clone, Debug, Default)]
pub struct DiagnosticsFilter {
    /// Only show diagnostics for this file (URI) if set
    pub only_uri: Option<Uri>,
    /// Minimum severity to show; None shows all
    pub min_severity: Option<Severity>,
    /// Case-insensitive substring filter on diagnostic message
    pub query: Option<String>,
}

/// A simple diagnostics panel that reads from LspState diagnostics
pub struct DiagnosticsPanel {
    lsp_state: Entity<nucleotide_lsp::LspState>,
    filter: DiagnosticsFilter,
    focus: FocusHandle,
    selected_index: usize,
}

impl DiagnosticsPanel {
    pub fn new(
        lsp_state: Entity<nucleotide_lsp::LspState>,
        filter: DiagnosticsFilter,
        cx: &mut Context<Self>,
    ) -> Self {
        nucleotide_logging::info!("DIAG: DiagnosticsPanel created");
        Self {
            lsp_state,
            filter,
            focus: cx.focus_handle(),
            selected_index: 0,
        }
    }

    /// Update filter and request re-render
    pub fn set_filter(&mut self, filter: DiagnosticsFilter, cx: &mut Context<Self>) {
        nucleotide_logging::info!(
            only_uri = filter.only_uri.as_ref().map(|u| u.to_string()).unwrap_or_else(|| "<all>".into()),
            min_severity = ?filter.min_severity,
            query = filter.query.as_deref().unwrap_or("") ,
            "DIAG: DiagnosticsPanel filter updated"
        );
        self.filter = filter;
        cx.notify();
    }
}

#[derive(Clone, Debug)]
pub struct DiagnosticsJumpEvent {
    pub path: PathBuf,
    pub offset: usize,
}

impl EventEmitter<DismissEvent> for DiagnosticsPanel {}
impl EventEmitter<DiagnosticsJumpEvent> for DiagnosticsPanel {}

impl Focusable for DiagnosticsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FocusableModal for DiagnosticsPanel {}

impl Render for DiagnosticsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Ensure the panel obtains focus to capture keyboard input
        self.ensure_focus(window, &self.focus);
        let lsp_state = self.lsp_state.clone();
        let filter = self.filter.clone();
        let theme = cx.global::<crate::ThemeManager>().helix_theme().clone();

        // Build a flattened, filtered list of diagnostics
        let mut rows: Vec<(Uri, DiagnosticInfo)> = Vec::new();
        let filter_ref = &filter;
        lsp_state.update(cx, |state, _| {
            for (uri, infos) in state.diagnostics.iter() {
                if let Some(only) = &filter_ref.only_uri {
                    if uri != only {
                        continue;
                    }
                }
                for info in infos {
                    let sev_ok = filter_ref
                        .min_severity
                        .map_or(true, |min| info.diagnostic.severity() >= min);
                    if !sev_ok {
                        continue;
                    }
                    let msg_ok = filter_ref.query.as_ref().map_or(true, |q| {
                        info.diagnostic
                            .message
                            .to_lowercase()
                            .contains(&q.to_lowercase())
                    });
                    if !msg_ok {
                        continue;
                    }
                    rows.push((uri.clone(), info.clone()));
                }
            }
        });

        nucleotide_logging::info!(
            count = rows.len(),
            only_uri = filter.only_uri.as_ref().map(|u| u.to_string()).unwrap_or_else(|| "<all>".into()),
            min_severity = ?filter.min_severity,
            query = filter.query.as_deref().unwrap_or("") ,
            "DIAG: DiagnosticsPanel rendered rows"
        );

        // Sort by severity desc, then by uri, then by start position
        rows.sort_by(|a, b| {
            use std::cmp::Ordering;
            let sa = a.1.diagnostic.severity();
            let sb = b.1.diagnostic.severity();
            sb.cmp(&sa) // higher severity first
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
                .then_with(|| a.1.diagnostic.range.start.cmp(&b.1.diagnostic.range.start))
        });

        // Map severity to label/icon
        let sev_label = |s: Severity| match s {
            Severity::Error => ("ERROR", "diagnostic.error"),
            Severity::Warning => ("WARN", "diagnostic.warning"),
            Severity::Info => ("INFO", "diagnostic.info"),
            Severity::Hint => ("HINT", "diagnostic.hint"),
        };

        // Emit jump-to event on click if an event provider is present
        let _emit = nucleotide_ui::providers::use_emit_event();

        // Header row
        let header = {
            let header_style = cx.theme_style("ui.text.muted");
            let color = header_style
                .fg
                .and_then(nucleotide_ui::theme_utils::color_to_hsla)
                .unwrap_or(gpui::white());
            div()
                .flex()
                .px_3()
                .py_1()
                .gap_4()
                .child(div().w(gpui::px(16.0)).child(" "))
                .child(div().w(gpui::px(90.0)).text_color(color).child("severity"))
                .child(div().w(gpui::px(120.0)).text_color(color).child("source"))
                .child(div().w(gpui::px(70.0)).text_color(color).child("code"))
                .child(div().flex_1().text_color(color).child("message"))
        };

        let total = rows.len();
        // Snapshot server names to avoid borrowing cx/LspState inside row map closure
        let server_names: std::collections::HashMap<helix_lsp::LanguageServerId, String> = {
            let mut m = std::collections::HashMap::new();
            lsp_state.update(cx, |state, _| {
                for (id, server) in state.servers.iter() {
                    m.insert(*id, server.name.clone());
                }
            });
            m
        };

        let mut idx_counter = 0usize;
        let list = rows.into_iter().map(|(uri, info)| {
            let path_display = uri
                .as_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| uri.to_string());
            let (label, _style_key) = sev_label(info.diagnostic.severity());

            let primary = info.diagnostic.message.clone();
            let source = info
                .diagnostic
                .source
                .clone()
                .or_else(|| server_names.get(&info.server_id).cloned())
                .unwrap_or_else(|| "lsp".to_string());
            let code = match &info.diagnostic.code {
                Some(helix_core::diagnostic::NumberOrString::Number(n)) => n.to_string(),
                Some(helix_core::diagnostic::NumberOrString::String(s)) => s.clone(),
                None => String::new(),
            };

            let id = SharedString::from(format!(
                "diag-{}-{}",
                path_display, info.diagnostic.range.start
            ));

            // Colors for severity label
            let sev_color = match info.diagnostic.severity() {
                Severity::Error => theme.get("diagnostic.error").fg,
                Severity::Warning => theme.get("diagnostic.warning").fg,
                Severity::Info => theme.get("diagnostic.info").fg,
                Severity::Hint => theme.get("diagnostic.hint").fg,
            }
            .and_then(nucleotide_ui::theme_utils::color_to_hsla)
            .unwrap_or(gpui::white());

            let is_selected = idx_counter == self.selected_index;
            let prefix = if is_selected { ">" } else { " " };

            let row = div()
                .id(id.clone())
                .flex()
                .items_center()
                .px_3()
                .py_1()
                .gap_4()
                .child(div().w(gpui::px(16.0)).child(prefix))
                .child(div().w(gpui::px(90.0)).text_color(sev_color).child(label))
                .child(div().w(gpui::px(120.0)).child(source.clone()))
                .child(div().w(gpui::px(70.0)).child(code.clone()))
                .child(div().flex_1().child(primary.clone()));

            // Emit navigation event on click via GPUI event
            let path_for_event = uri
                .as_path()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(path_display.clone()));
            let offset = info.diagnostic.range.start;
            let idx_here = idx_counter;
            let row = div()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this: &mut DiagnosticsPanel, _e, _w, cx| {
                        nucleotide_logging::info!(
                            path = %path_for_event.display(),
                            offset = offset,
                            "DIAG: DiagnosticsPanel item clicked"
                        );
                        this.selected_index = idx_here;
                        cx.emit(DiagnosticsJumpEvent {
                            path: path_for_event.clone(),
                            offset,
                        });
                        // Also dismiss panel after emitting
                        cx.emit(DismissEvent);
                    }),
                )
                .child(row);
            idx_counter += 1;
            row
        });

        // Top bar with count (like 1/1)
        let count_text = format!("{}/{}", total, total);
        let top_bar = div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_1()
            .child(div().child(""))
            .child(div().child(count_text));

        // Container styling similar to file picker
        let modal_tokens = &cx.theme().tokens;
        let container = div()
            .key_context("DiagnosticsPicker")
            .w(gpui::px(900.0))
            .max_h(gpui::px(520.0))
            .bg(modal_tokens.picker_tokens().container_background)
            .border_1()
            .border_color(modal_tokens.picker_tokens().border)
            .rounded_md()
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => {
                        if this.selected_index > 0 {
                            this.selected_index -= 1;
                            cx.notify();
                        }
                    }
                    "down" => {
                        // Recompute row count to clamp
                        let mut count = 0usize;
                        let filter = this.filter.clone();
                        this.lsp_state.update(cx, |state, _| {
                            for (_uri, infos) in state.diagnostics.iter() {
                                for info in infos {
                                    let sev_ok = filter
                                        .min_severity
                                        .map_or(true, |min| info.diagnostic.severity() >= min);
                                    let msg_ok = filter.query.as_ref().map_or(true, |q| {
                                        info.diagnostic
                                            .message
                                            .to_lowercase()
                                            .contains(&q.to_lowercase())
                                    });
                                    if sev_ok && msg_ok {
                                        count += 1;
                                    }
                                }
                            }
                        });
                        if count > 0 {
                            this.selected_index = (this.selected_index + 1).min(count - 1);
                            cx.notify();
                        }
                    }
                    "enter" => {
                        // Find selected row and emit jump
                        let mut rows: Vec<(Uri, DiagnosticInfo)> = Vec::new();
                        let filter = this.filter.clone();
                        this.lsp_state.update(cx, |state, _| {
                            for (uri, infos) in state.diagnostics.iter() {
                                for info in infos {
                                    let sev_ok = filter
                                        .min_severity
                                        .map_or(true, |min| info.diagnostic.severity() >= min);
                                    let msg_ok = filter.query.as_ref().map_or(true, |q| {
                                        info.diagnostic
                                            .message
                                            .to_lowercase()
                                            .contains(&q.to_lowercase())
                                    });
                                    if sev_ok && msg_ok {
                                        rows.push((uri.clone(), info.clone()));
                                    }
                                }
                            }
                        });
                        rows.sort_by(|a, b| {
                            let sa = a.1.diagnostic.severity();
                            let sb = b.1.diagnostic.severity();
                            sb.cmp(&sa)
                                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
                                .then_with(|| {
                                    a.1.diagnostic.range.start.cmp(&b.1.diagnostic.range.start)
                                })
                        });
                        if !rows.is_empty() {
                            let idx = this.selected_index.min(rows.len() - 1);
                            let (uri, info) = rows[idx].clone();
                            let path_for_event = uri
                                .as_path()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| std::path::PathBuf::from(uri.to_string()));
                            let offset = info.diagnostic.range.start;
                            cx.emit(DiagnosticsJumpEvent {
                                path: path_for_event,
                                offset,
                            });
                            cx.emit(DismissEvent);
                        }
                    }
                    "escape" => {
                        cx.emit(DismissEvent);
                    }
                    _ => {}
                }
            }))
            .child(top_bar)
            .child(header)
            .children(list);

        // Root wrapper (no visual styling needed; overlay adds centering)
        div()
            .id("diagnostics-panel")
            .flex()
            .flex_col()
            .gap_2()
            .child(container)
    }
}
