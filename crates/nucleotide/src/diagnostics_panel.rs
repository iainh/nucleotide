// ABOUTME: Diagnostics list panel rendering diagnostics from LspState with simple filters

use gpui::{prelude::FluentBuilder, *};
use helix_core::{Uri, diagnostic::Severity};
use nucleotide_lsp::lsp_state::DiagnosticInfo;
use nucleotide_ui::ThemedContext;
use nucleotide_ui::common::FocusableModal;
use nucleotide_ui::theme_manager::HelixThemedContext;
use nucleotide_ui::tokens::utils; // for color utilities (darken/with_alpha)
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

        // Column widths (keep header and rows in sync)
        const COL_PREFIX_W: f32 = 16.0;
        const COL_SEVERITY_W: f32 = 90.0; // keep header text visible; rows show symbol inside
        const COL_SOURCE_W: f32 = 120.0;
        const COL_CODE_W: f32 = 70.0;

        // Insert soft wrap points into long tokens like file paths so text can wrap
        fn soft_wrap_message(s: &str) -> String {
            let mut out = String::with_capacity(s.len() + 16);
            for ch in s.chars() {
                out.push(ch);
                match ch {
                    '/' | '-' | '_' | '.' | ':' => out.push('\u{200B}'), // zero-width space
                    _ => {}
                }
            }
            out
        }

        // Emit jump-to event on click if an event provider is present
        let _emit = nucleotide_ui::providers::use_emit_event();
        // Picker design tokens for consistent colors
        let picker_tokens = cx.theme().tokens.picker_tokens();

        // Header row (aligned with body columns)
        let header = {
            let color = picker_tokens.header_text;
            div()
                .flex()
                .px_3()
                .py_1()
                .gap_4()
                .child(div().w(gpui::px(COL_PREFIX_W)).flex_shrink_0().child(" "))
                .child(
                    div()
                        .w(gpui::px(COL_SEVERITY_W))
                        .flex_shrink_0()
                        .text_color(color)
                        .text_left()
                        .child("severity"),
                )
                .child(
                    div()
                        .w(gpui::px(COL_SOURCE_W))
                        .flex_shrink_0()
                        .text_color(color)
                        .text_left()
                        .child("source"),
                )
                .child(
                    div()
                        .w(gpui::px(COL_CODE_W))
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
        };

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
        // Helper: draw the same gutter-style severity marker inside bounds
        let severity_icon = |sev: Severity, color: Hsla| {
            gpui::canvas(
                |_bounds, _window, _cx| (),
                move |bounds, _state, window: &mut Window, _cx: &mut App| {
                    // Compute square area centered inside the provided bounds
                    let height = bounds.size.height;
                    let width = bounds.size.width;
                    let marker_size = height * 0.70;
                    let marker_x = bounds.origin.x + (width - marker_size) * 0.5;
                    let marker_y = bounds.origin.y + (height - marker_size) * 0.5;

                    // Use solid color (no extra alpha) so it matches gutter over any background
                    let base_fill = color;
                    let border_col = utils::with_alpha(utils::darken(color, 0.15), 0.9);

                    match sev {
                        Severity::Error => {
                            // Slightly rounded square with border and glossy dot
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

                            // Small top-left highlight
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
                            // Upright triangle
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

                            // Small internal highlight
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
                            // Circle with subtle highlight
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

                            // Highlights
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
            .w(gpui::px(COL_SEVERITY_W))
            .h(gpui::px(18.0))
            .flex_shrink_0()
        };
        let list = rows.into_iter().map(|(uri, info)| {
            let path_display = uri
                .as_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| uri.to_string());
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

            // Colors for severity icon (match gutter: prefer underline color fallback to fg)
            let sev_color = severity_hsla(info.diagnostic.severity());

            let is_selected = idx_counter == self.selected_index;
            // No chevron prefix; selection shown via tokens
            let prefix = " ";

            let item_text = picker_tokens.item_text;
            let item_text_secondary = picker_tokens.item_text_secondary;
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
            let row_hover_bg = picker_tokens.item_background_hover;

            let row = div()
                .id(id.clone())
                .flex()
                .items_start()
                .px_3()
                .py_1()
                .gap_4()
                .bg(row_bg)
                .text_color(row_text)
                .child(
                    div()
                        .w(gpui::px(COL_PREFIX_W))
                        .flex_shrink_0()
                        .child(prefix),
                )
                .child(severity_icon(info.diagnostic.severity(), sev_color))
                .child(
                    div()
                        .w(gpui::px(COL_SOURCE_W))
                        .flex_shrink_0()
                        .text_color(row_text_secondary)
                        .child(source.clone()),
                )
                .child(
                    div()
                        .w(gpui::px(COL_CODE_W))
                        .flex_shrink_0()
                        .text_color(row_text_secondary)
                        .child(code.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .flex_grow()
                        .whitespace_normal()
                        .child(soft_wrap_message(&primary)),
                );

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
                    cx.listener(move |this: &mut DiagnosticsPanel, _e, window, cx| {
                        nucleotide_logging::info!(
                            path = %path_for_event.display(),
                            offset = offset,
                            "DIAG: DiagnosticsPanel item clicked"
                        );
                        this.selected_index = idx_here;
                        // Drop focus immediately so the editor can regain it after dismissal
                        window.disable_focus();
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
                            // Drop focus immediately so the editor can regain it after dismissal
                            window.disable_focus();
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
