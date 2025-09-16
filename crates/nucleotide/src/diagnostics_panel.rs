// ABOUTME: Diagnostics list panel rendering diagnostics from LspState with simple filters

use gpui::*;
use helix_core::{Uri, diagnostic::Severity};
use nucleotide_lsp::lsp_state::DiagnosticInfo;
use nucleotide_ui::ThemedContext;
use nucleotide_ui::common::FocusableModal;
// use nucleotide_ui::theme_manager::HelixThemedContext; // not used in this module
use nucleotide_ui::tokens::utils; // for color utilities (darken/with_alpha)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavKey {
    Up,
    Down,
    Enter,
    Escape,
}

fn nav_key(event: &KeyDownEvent) -> Option<NavKey> {
    match event.keystroke.key.as_str() {
        "up" => Some(NavKey::Up),
        "down" => Some(NavKey::Down),
        "enter" => Some(NavKey::Enter),
        "escape" => Some(NavKey::Escape),
        _ => None,
    }
}

impl DiagnosticsPanel {
    pub fn new(
        lsp_state: Entity<nucleotide_lsp::LspState>,
        filter: DiagnosticsFilter,
        cx: &mut Context<Self>,
    ) -> Self {
        nucleotide_logging::info!("DIAG: DiagnosticsPanel created");
        let focus_handle = cx.focus_handle();
        if let Some(coord) = cx.try_global::<nucleotide_ui::FocusCoordinator>() {
            coord.set_diagnostics_focus(focus_handle.clone());
        }
        Self {
            lsp_state,
            filter,
            focus: focus_handle,
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

    /// Build the header row aligned to the body columns.
    fn build_header_row(
        &self,
        cx: &mut Context<Self>,
        col_severity_w: f32,
        col_source_w: f32,
        col_code_w: f32,
        col_prefix_w: f32,
    ) -> gpui::AnyElement {
        let picker_tokens = cx.theme().tokens.picker_tokens();
        let color = picker_tokens.header_text;
        div()
            .flex()
            .px_3()
            .py_1()
            .gap_4()
            .child(div().w(px(col_prefix_w)).flex_shrink_0().child(" "))
            .child(
                div()
                    .w(px(col_severity_w))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .text_color(color)
                    .child("severity"),
            )
            .child(
                div()
                    .w(px(col_source_w))
                    .flex_shrink_0()
                    .text_color(color)
                    .text_left()
                    .child("source"),
            )
            .child(
                div()
                    .w(px(col_code_w))
                    .flex_shrink_0()
                    .text_color(color)
                    .text_left()
                    .child("code"),
            )
            .child(
                div()
                    .flex_1()
                    .text_color(color)
                    .text_left()
                    .child("message"),
            )
            .into_any_element()
    }

    /// Return filtered diagnostics as (Uri, DiagnosticInfo) pairs.
    fn filtered_rows(&self, cx: &mut Context<Self>) -> Vec<(Uri, DiagnosticInfo)> {
        let mut rows: Vec<(Uri, DiagnosticInfo)> = Vec::new();
        let filter = self.filter.clone();
        self.lsp_state.update(cx, |state, _| {
            for (uri, infos) in state.diagnostics.iter() {
                if let Some(only) = &filter.only_uri
                    && uri != only
                {
                    continue;
                }
                for info in infos {
                    let sev_ok = filter
                        .min_severity
                        .is_none_or(|min| info.diagnostic.severity() >= min);
                    if !sev_ok {
                        continue;
                    }
                    let msg_ok = filter.query.as_ref().is_none_or(|q| {
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
        rows
    }

    /// Return filtered and sorted diagnostics.
    fn sorted_filtered_rows(&self, cx: &mut Context<Self>) -> Vec<(Uri, DiagnosticInfo)> {
        let mut rows = self.filtered_rows(cx);
        rows.sort_by(|a, b| {
            let sa = a.1.diagnostic.severity();
            let sb = b.1.diagnostic.severity();
            sb.cmp(&sa)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
                .then_with(|| a.1.diagnostic.range.start.cmp(&b.1.diagnostic.range.start))
        });
        rows
    }

    /// Count filtered rows (used for navigation bounds).
    fn count_filtered_rows(&self, cx: &mut Context<Self>) -> usize {
        self.filtered_rows(cx).len()
    }

    /// Insert soft wrap opportunities into long strings (paths, messages).
    fn soft_wrap(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 16);
        for ch in s.chars() {
            out.push(ch);
            match ch {
                '/' | '-' | '_' | '.' | ':' => out.push('\u{200B}'),
                _ => {}
            }
        }
        out
    }

    /// Render a severity icon sized to a standard cell.
    fn render_severity_icon(
        sev: Severity,
        color: Hsla,
        width: f32,
        height: f32,
    ) -> gpui::AnyElement {
        gpui::canvas(
            |_bounds, _window, _cx| (),
            move |bounds, _state, window: &mut Window, _cx: &mut App| {
                let marker_size = bounds.size.height * 0.70;
                let marker_x = bounds.origin.x + (bounds.size.width - marker_size) * 0.5;
                let marker_y = bounds.origin.y + (bounds.size.height - marker_size) * 0.5;

                let base_fill = color;
                let border_col = utils::with_alpha(utils::darken(color, 0.15), 0.9);

                match sev {
                    Severity::Error => {
                        let marker_bounds = Bounds {
                            origin: point(marker_x, marker_y),
                            size: size(marker_size, marker_size),
                        };
                        window.paint_quad(gpui::quad(
                            marker_bounds,
                            px(1.0),
                            base_fill,
                            px(1.0),
                            border_col,
                            gpui::BorderStyle::default(),
                        ));
                        let h_size = marker_size * 0.22;
                        let h_bounds = Bounds {
                            origin: point(
                                marker_x + marker_size * 0.18,
                                marker_y + marker_size * 0.18,
                            ),
                            size: size(h_size, h_size),
                        };
                        let h_color = utils::with_alpha(gpui::white(), 0.18);
                        window.paint_quad(gpui::quad(
                            h_bounds,
                            h_size * 0.5,
                            h_color,
                            0.0,
                            gpui::transparent_black(),
                            gpui::BorderStyle::default(),
                        ));
                    }
                    Severity::Warning => {
                        let top = point(marker_x + marker_size * 0.5, marker_y);
                        let bl = point(marker_x, marker_y + marker_size);
                        let br = point(marker_x + marker_size, marker_y + marker_size);
                        let mut pb = gpui::PathBuilder::fill();
                        pb.move_to(top);
                        pb.line_to(bl);
                        pb.line_to(br);
                        pb.close();
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, base_fill);
                        }
                        let h_size = marker_size * 0.20;
                        let h_bounds = Bounds {
                            origin: point(
                                marker_x + marker_size * 0.22,
                                marker_y + marker_size * 0.18,
                            ),
                            size: size(h_size, h_size),
                        };
                        let h_color = utils::with_alpha(gpui::white(), 0.14);
                        window.paint_quad(gpui::quad(
                            h_bounds,
                            h_size * 0.5,
                            h_color,
                            0.0,
                            gpui::transparent_black(),
                            gpui::BorderStyle::default(),
                        ));
                    }
                    Severity::Info | Severity::Hint => {
                        let marker_bounds = Bounds {
                            origin: point(marker_x, marker_y),
                            size: size(marker_size, marker_size),
                        };
                        let radius = marker_size * 0.5;
                        window.paint_quad(gpui::quad(
                            marker_bounds,
                            radius,
                            base_fill,
                            px(1.0),
                            border_col,
                            gpui::BorderStyle::default(),
                        ));
                        let offset = marker_size * 0.14;
                        let halo_size = marker_size * 0.52;
                        let core_size = marker_size * 0.26;
                        let halo_bounds = Bounds {
                            origin: point(marker_x + offset, marker_y + offset),
                            size: size(halo_size, halo_size),
                        };
                        let core_bounds = Bounds {
                            origin: point(
                                marker_x + offset + (halo_size - core_size) * 0.25,
                                marker_y + offset + (halo_size - core_size) * 0.25,
                            ),
                            size: size(core_size, core_size),
                        };
                        let highlight_halo = utils::with_alpha(gpui::white(), 0.14);
                        let highlight_core = utils::with_alpha(gpui::white(), 0.45);
                        window.paint_quad(gpui::quad(
                            halo_bounds,
                            halo_size * 0.5,
                            highlight_halo,
                            0.0,
                            gpui::transparent_black(),
                            gpui::BorderStyle::default(),
                        ));
                        window.paint_quad(gpui::quad(
                            core_bounds,
                            core_size * 0.5,
                            highlight_core,
                            0.0,
                            gpui::transparent_black(),
                            gpui::BorderStyle::default(),
                        ));
                    }
                }
            },
        )
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .into_any_element()
    }

    /// Build a single diagnostics row element.
    #[allow(clippy::too_many_arguments)]
    fn build_row(
        &mut self,
        cx: &mut Context<Self>,
        idx: usize,
        uri: &Uri,
        info: &DiagnosticInfo,
        source: &str,
        code: &str,
        picker_tokens: &nucleotide_ui::tokens::PickerTokens,
        sev_color: Hsla,
        col_prefix_w: f32,
        col_severity_w: f32,
        col_source_w: f32,
        col_code_w: f32,
    ) -> gpui::AnyElement {
        let path_display = uri
            .as_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| uri.to_string());
        let primary = info.diagnostic.message.clone();

        let is_selected = idx == self.selected_index;
        let row_text = if is_selected {
            picker_tokens.item_text_selected
        } else {
            picker_tokens.item_text
        };
        let row_text_secondary = if is_selected {
            picker_tokens.item_text_selected
        } else {
            picker_tokens.item_text_secondary
        };
        let row_bg = if is_selected {
            picker_tokens.item_background_selected
        } else {
            picker_tokens.item_background
        };

        let content = div()
            .flex()
            .items_start()
            .px_3()
            .py_1()
            .gap_4()
            .bg(row_bg)
            .text_color(row_text)
            .child(div().w(px(col_prefix_w)).flex_shrink_0().child(" "))
            .child(
                div()
                    .w(px(col_severity_w))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .child(Self::render_severity_icon(
                        info.diagnostic.severity(),
                        sev_color,
                        col_severity_w,
                        18.0,
                    )),
            )
            .child(
                div()
                    .w(px(col_source_w))
                    .flex_shrink_0()
                    .text_color(row_text_secondary)
                    .child(source.to_string()),
            )
            .child(
                div()
                    .w(px(col_code_w))
                    .flex_shrink_0()
                    .text_color(row_text_secondary)
                    .child(code.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .flex_grow()
                    .whitespace_normal()
                    .child(Self::soft_wrap(&primary)),
            );

        let path_for_event = uri
            .as_path()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(path_display));
        let offset = info.diagnostic.range.start;
        let idx_here = idx;

        div()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this: &mut DiagnosticsPanel, _e, window, cx| {
                    nucleotide_logging::info!(
                        path = %path_for_event.display(),
                        offset = offset,
                        "DIAG: DiagnosticsPanel item clicked"
                    );
                    this.selected_index = idx_here;
                    window.disable_focus();
                    cx.emit(DiagnosticsJumpEvent {
                        path: path_for_event.clone(),
                        offset,
                    });
                    cx.emit(DismissEvent);
                }),
            )
            .child(content)
            .into_any_element()
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
                if let Some(only) = &filter_ref.only_uri
                    && uri != only
                {
                    continue;
                }
                for info in infos {
                    let sev_ok = filter_ref
                        .min_severity
                        .is_none_or(|min| info.diagnostic.severity() >= min);
                    if !sev_ok {
                        continue;
                    }
                    let msg_ok = filter_ref.query.as_ref().is_none_or(|q| {
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
            let sa = a.1.diagnostic.severity();
            let sb = b.1.diagnostic.severity();
            sb.cmp(&sa) // higher severity first
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
                .then_with(|| a.1.diagnostic.range.start.cmp(&b.1.diagnostic.range.start))
        });

        // Column widths (keep header and rows in sync)
        const COL_PREFIX_W: f32 = 16.0;
        const COL_SEVERITY_W: f32 = 64.0; // compact column that still fits the header label
        const COL_SOURCE_W: f32 = 120.0;
        const COL_CODE_W: f32 = 70.0;

        // (soft_wrap helper moved into DiagnosticsPanel)

        // Emit jump-to event on click if an event provider is present
        let _emit = nucleotide_ui::providers::use_emit_event();
        // Picker design tokens for consistent colors
        let picker_tokens = cx.theme().tokens.picker_tokens();

        // Header row (aligned with body columns)
        let header =
            self.build_header_row(cx, COL_SEVERITY_W, COL_SOURCE_W, COL_CODE_W, COL_PREFIX_W);

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

        // Build body rows using a helper to centralize layout
        // Helper: resolve diagnostic color same as Document renderer (prefer underline color)
        let severity_hsla = |sev: Severity| -> Hsla {
            let key = match sev {
                Severity::Error => "diagnostic.error",
                Severity::Warning => "diagnostic.warning",
                Severity::Info => "diagnostic.info",
                Severity::Hint => "diagnostic.hint",
            };
            let style = theme.get(key);
            style
                .underline_color
                .or(style.fg)
                .and_then(nucleotide_ui::theme_utils::color_to_hsla)
                .unwrap_or(picker_tokens.item_text)
        };
        // (severity icon rendering moved into DiagnosticsPanel::render_severity_icon)
        let list: Vec<gpui::AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(idx, (uri, info))| {
                // precompute strings used in row builder
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

                // Colors for severity icon (match gutter: prefer underline color fallback to fg)
                let sev_color = severity_hsla(info.diagnostic.severity());
                self.build_row(
                    cx,
                    idx,
                    &uri,
                    &info,
                    &source,
                    &code,
                    &picker_tokens,
                    sev_color,
                    COL_PREFIX_W,
                    COL_SEVERITY_W,
                    COL_SOURCE_W,
                    COL_CODE_W,
                )
            })
            .collect();

        // Container styling using design tokens (match picker style and font)
        let modal_tokens = &cx.theme().tokens;
        let ui_font = cx
            .global::<nucleotide_types::FontSettings>()
            .var_font
            .clone()
            .into();
        let ui_text_size = px(cx.global::<nucleotide_types::UiFontConfig>().size);
        let container = div()
            .key_context("DiagnosticsPicker")
            .w(gpui::px(900.0))
            .max_h(gpui::px(520.0))
            .bg(modal_tokens.picker_tokens().container_background)
            .border_1()
            .border_color(modal_tokens.picker_tokens().border)
            .rounded_md()
            .shadow_lg()
            .font(ui_font)
            .text_size(ui_text_size)
            .overflow_hidden()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                cx.stop_propagation();
                match nav_key(event) {
                    Some(NavKey::Up) => {
                        if this.selected_index > 0 {
                            this.selected_index -= 1;
                            cx.notify();
                        }
                    }
                    Some(NavKey::Down) => {
                        // Recompute row count to clamp
                        let count = this.count_filtered_rows(cx);
                        if count > 0 {
                            this.selected_index = (this.selected_index + 1).min(count - 1);
                            cx.notify();
                        }
                    }
                    Some(NavKey::Enter) => {
                        // Find selected row and emit jump
                        let rows = this.sorted_filtered_rows(cx);
                        if !rows.is_empty() {
                            let idx = this.selected_index.min(rows.len() - 1);
                            let (uri, info) = rows[idx].clone();
                            let path_for_event = uri
                                .as_path()
                                .map(|p| p.to_path_buf())
                                .unwrap_or_else(|| std::path::PathBuf::from(uri.to_string()));
                            let offset = info.diagnostic.range.start;
                            // Drop focus immediately so the editor can regain it after dismissal
                            window.disable_focus();
                            cx.emit(DiagnosticsJumpEvent {
                                path: path_for_event,
                                offset,
                            });
                            cx.emit(DismissEvent);
                        }
                    }
                    Some(NavKey::Escape) => {
                        cx.emit(DismissEvent);
                    }
                    None => {}
                }
            }))
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
