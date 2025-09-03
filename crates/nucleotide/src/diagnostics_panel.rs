// ABOUTME: Diagnostics list panel rendering diagnostics from LspState with simple filters

use gpui::{prelude::FluentBuilder, *};
use helix_core::{Uri, diagnostic::Severity};
use nucleotide_lsp::lsp_state::DiagnosticInfo;
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
}

impl DiagnosticsPanel {
    pub fn new(
        lsp_state: Entity<nucleotide_lsp::LspState>,
        filter: DiagnosticsFilter,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            lsp_state,
            filter,
            focus: cx.focus_handle(),
        }
    }

    /// Update filter and request re-render
    pub fn set_filter(&mut self, filter: DiagnosticsFilter, cx: &mut Context<Self>) {
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

impl Render for DiagnosticsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lsp_state = self.lsp_state.clone();
        let filter = self.filter.clone();
        let _theme = cx.global::<crate::ThemeManager>().helix_theme().clone();

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
            Severity::Error => ("E", "diagnostic.error"),
            Severity::Warning => ("W", "diagnostic.warning"),
            Severity::Info => ("I", "diagnostic.info"),
            Severity::Hint => ("H", "diagnostic.hint"),
        };

        // Emit jump-to event on click if an event provider is present
        let _emit = nucleotide_ui::providers::use_emit_event();

        let list = rows.into_iter().map(|(uri, info)| {
            let path_display = uri
                .as_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| uri.to_string());
            let (label, _style_key) = sev_label(info.diagnostic.severity());

            let secondary = path_display.clone();
            let primary = info.diagnostic.message.clone();

            let id = SharedString::from(format!(
                "diag-{}-{}",
                path_display, info.diagnostic.range.start
            ));

            // Build list item
            let item = ListItem::new(id.clone())
                .spacing(ListItemSpacing::Compact)
                .variant(ListItemVariant::Default)
                .start_slot(div().child(label))
                .child(div().child(primary))
                .end_slot(div().child(secondary.clone()));

            // Emit navigation event on click via GPUI event
            let path_for_event = uri
                .as_path()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(path_display.clone()));
            let offset = info.diagnostic.range.start;
            div()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this: &mut DiagnosticsPanel, _e, _w, cx| {
                        cx.emit(DiagnosticsJumpEvent {
                            path: path_for_event.clone(),
                            offset,
                        });
                        // Also dismiss panel after emitting
                        cx.emit(DismissEvent);
                    }),
                )
                .child(item)
        });

        div()
            .id("diagnostics-panel")
            .flex()
            .flex_col()
            .gap_2()
            .children(list)
    }
}
