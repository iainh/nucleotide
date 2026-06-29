// ABOUTME: Native editor inline diagnostic planning and painting
// ABOUTME: Converts Helix diagnostics/config into GPUI virtual diagnostic rows

use std::collections::BTreeMap;

use gpui::{
    App, Font, Hsla, PathBuilder, Pixels, Point, SharedString, TextAlign, TextRun, Window, point,
    px,
};
use helix_core::{
    Diagnostic, RopeSlice,
    diagnostic::Severity,
    graphemes::{grapheme_width, tab_width_at},
};
use helix_view::{
    Document, Theme, View, ViewId,
    annotations::diagnostics::{DiagnosticFilter, InlineDiagnosticsConfig},
    document::Mode,
    graphics::{Color, Style},
};

use crate::{LineLayoutCache, style::helix_color_to_hsla};

#[derive(Debug, Clone, PartialEq)]
pub struct InlineDiagnosticTextLine {
    pub text: SharedString,
    pub severity: Severity,
    pub color: Hsla,
    pub text_col: usize,
    pub connector: Option<InlineDiagnosticConnector>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineDiagnosticConnector {
    pub anchor_col: usize,
    pub prefix_len: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InlineDiagnosticLinePlan {
    pub doc_line: usize,
    pub eol: Option<InlineDiagnosticTextLine>,
    pub rows: Vec<InlineDiagnosticTextLine>,
}

impl InlineDiagnosticLinePlan {
    pub fn virtual_row_count(&self) -> usize {
        self.rows.len()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InlineDiagnosticFramePlan {
    pub lines: BTreeMap<usize, InlineDiagnosticLinePlan>,
}

impl InlineDiagnosticFramePlan {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn line(&self, doc_line: usize) -> Option<&InlineDiagnosticLinePlan> {
        self.lines.get(&doc_line)
    }

    pub fn virtual_rows_by_line(&self) -> BTreeMap<usize, usize> {
        self.lines
            .iter()
            .filter_map(|(&line, plan)| {
                let count = plan.virtual_row_count();
                (count > 0).then_some((line, count))
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
pub struct InlineDiagnosticColors {
    pub hint: Hsla,
    pub info: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
}

impl InlineDiagnosticColors {
    pub fn color_for(self, severity: Severity) -> Hsla {
        match severity {
            Severity::Hint => self.hint,
            Severity::Info => self.info,
            Severity::Warning => self.warning,
            Severity::Error => self.error,
        }
    }
}

pub struct InlineDiagnosticFramePlanParams<'a> {
    pub document: &'a Document,
    pub view: &'a View,
    pub view_id: ViewId,
    pub theme: &'a Theme,
    pub editor_mode: Mode,
    pub viewport_columns: u16,
    pub horizontal_offset: usize,
    pub tab_width: u16,
}

pub struct InlineDiagnosticPaintParams<'a> {
    pub line_plan: &'a InlineDiagnosticLinePlan,
    pub line_cache: &'a LineLayoutCache,
    pub font: Font,
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub viewport_width: Pixels,
    pub line_height: Pixels,
    pub text_origin_x: Pixels,
    pub source_line_y: Pixels,
    pub source_line_width: Pixels,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FallbackDiagnosticFramePlan {
    pub lines: Vec<InlineDiagnosticTextLine>,
}

pub struct FallbackDiagnosticFramePlanParams<'a> {
    pub document: &'a Document,
    pub view_id: ViewId,
    pub theme: &'a Theme,
}

pub struct FallbackDiagnosticPaintParams<'a> {
    pub plan: &'a FallbackDiagnosticFramePlan,
    pub line_cache: &'a LineLayoutCache,
    pub font: Font,
    pub font_size: Pixels,
    pub bounds: gpui::Bounds<Pixels>,
    pub line_height: Pixels,
    pub cell_width: Pixels,
}

struct InlineDiagnosticTextPaintParams<'a> {
    line_cache: &'a LineLayoutCache,
    line: &'a InlineDiagnosticTextLine,
    font: Font,
    x: Pixels,
    y: Pixels,
    font_size: Pixels,
    viewport_width: Pixels,
    line_height: Pixels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct InlineDiagnosticConnectorPaintPlan {
    start: Point<Pixels>,
    corner_start: Point<Pixels>,
    corner_control: Point<Pixels>,
    corner_end: Point<Pixels>,
    end: Point<Pixels>,
    stroke_width: Pixels,
    color: Hsla,
}

pub fn inline_diagnostic_frame_plan(
    params: InlineDiagnosticFramePlanParams<'_>,
) -> InlineDiagnosticFramePlan {
    let config = params.document.config.load();
    let enable_cursor_line = params.editor_mode != Mode::Insert
        && params
            .view
            .diagnostics_handler
            .show_cursorline_diagnostics(params.document, params.view_id);
    let inline_config = config
        .inline_diagnostics
        .prepare(params.viewport_columns, enable_cursor_line);

    inline_diagnostic_frame_plan_from_config(InlineDiagnosticFramePlanFromConfigParams {
        document: params.document,
        view_id: params.view_id,
        theme: params.theme,
        inline_config,
        eol_diagnostics: config.end_of_line_diagnostics,
        viewport_columns: params.viewport_columns,
        horizontal_offset: params.horizontal_offset,
        tab_width: params.tab_width,
    })
}

pub fn fallback_diagnostic_frame_plan(
    params: FallbackDiagnosticFramePlanParams<'_>,
) -> FallbackDiagnosticFramePlan {
    let config = params.document.config.load();
    let inline_config = config.inline_diagnostics.prepare(0, false);
    if !inline_config.disabled() || config.end_of_line_diagnostics != DiagnosticFilter::Disable {
        return FallbackDiagnosticFramePlan::default();
    }

    let text = params.document.text().slice(..);
    let cursor = params
        .document
        .selection(params.view_id)
        .primary()
        .cursor(text);
    let colors = inline_diagnostic_colors(params.theme);
    let mut lines = Vec::new();
    for diagnostic in params
        .document
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.range.start <= cursor && diagnostic.range.end >= cursor)
    {
        let severity = diagnostic_severity(diagnostic);
        let color = colors.color_for(severity);
        for line in diagnostic.message.lines() {
            lines.push(InlineDiagnosticTextLine {
                text: SharedString::from(line.trim()),
                severity,
                color,
                text_col: 0,
                connector: None,
            });
        }
        if let Some(code) = diagnostic.code.as_ref().map(|code| match code {
            helix_core::diagnostic::NumberOrString::Number(number) => format!("({number})"),
            helix_core::diagnostic::NumberOrString::String(value) => format!("({value})"),
        }) {
            lines.push(InlineDiagnosticTextLine {
                text: SharedString::from(code),
                severity,
                color,
                text_col: 0,
                connector: None,
            });
        }
    }

    FallbackDiagnosticFramePlan { lines }
}

struct InlineDiagnosticFramePlanFromConfigParams<'a> {
    document: &'a Document,
    view_id: ViewId,
    theme: &'a Theme,
    inline_config: InlineDiagnosticsConfig,
    eol_diagnostics: DiagnosticFilter,
    viewport_columns: u16,
    horizontal_offset: usize,
    tab_width: u16,
}

fn inline_diagnostic_frame_plan_from_config(
    params: InlineDiagnosticFramePlanFromConfigParams<'_>,
) -> InlineDiagnosticFramePlan {
    if params.document.diagnostics().is_empty()
        || (params.inline_config.disabled() && params.eol_diagnostics == DiagnosticFilter::Disable)
    {
        return InlineDiagnosticFramePlan::default();
    }

    let text = params.document.text().slice(..);
    let cursor_line = params
        .document
        .selection(params.view_id)
        .primary()
        .cursor_line(text);
    let colors = inline_diagnostic_colors(params.theme);
    let mut grouped: BTreeMap<usize, Vec<&Diagnostic>> = BTreeMap::new();

    for diagnostic in params.document.diagnostics() {
        let line = text.char_to_line(diagnostic.range.start.min(text.len_chars()));
        grouped.entry(line).or_default().push(diagnostic);
    }

    let mut lines = BTreeMap::new();
    for (doc_line, mut diagnostics) in grouped {
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.range.start,
                diagnostic.severity.unwrap_or_default(),
            )
        });

        let inline_filter = if doc_line == cursor_line {
            params.inline_config.cursor_line
        } else {
            params.inline_config.other_lines
        };
        let inline_diagnostics = filtered_inline_diagnostics(
            &diagnostics,
            inline_filter,
            params.inline_config.max_diagnostics,
        );
        let eol =
            eol_diagnostic(&diagnostics, inline_filter, params.eol_diagnostics).map(|diagnostic| {
                InlineDiagnosticTextLine {
                    text: SharedString::from(format!(
                        " {}",
                        first_message_line(&diagnostic.message)
                    )),
                    severity: diagnostic_severity(diagnostic),
                    color: colors.color_for(diagnostic_severity(diagnostic)),
                    text_col: 0,
                    connector: None,
                }
            });

        let mut rows = Vec::new();
        for diagnostic in inline_diagnostics {
            let anchor_col = diagnostic_anchor_column(
                text,
                doc_line,
                diagnostic.range.start,
                params.horizontal_offset,
                params.tab_width,
            );
            rows.extend(inline_diagnostic_rows(
                diagnostic,
                anchor_col,
                params.viewport_columns,
                &params.inline_config,
                colors,
            ));
        }

        if eol.is_some() || !rows.is_empty() {
            lines.insert(
                doc_line,
                InlineDiagnosticLinePlan {
                    doc_line,
                    eol,
                    rows,
                },
            );
        }
    }

    InlineDiagnosticFramePlan { lines }
}

pub fn paint_inline_diagnostic_plan(
    window: &mut Window,
    cx: &mut App,
    params: InlineDiagnosticPaintParams<'_>,
) {
    if let Some(eol) = &params.line_plan.eol {
        paint_inline_diagnostic_text_line(
            window,
            cx,
            InlineDiagnosticTextPaintParams {
                line_cache: params.line_cache,
                line: eol,
                font: params.font.clone(),
                x: params.text_origin_x + params.source_line_width,
                y: params.source_line_y,
                font_size: params.font_size,
                viewport_width: params.viewport_width,
                line_height: params.line_height,
            },
        );
    }

    for (index, row) in params.line_plan.rows.iter().enumerate() {
        let row_y = params.source_line_y + params.line_height * (index + 1) as f32;
        if let Some(connector) = row.connector {
            paint_inline_diagnostic_connector(
                window,
                inline_diagnostic_connector_paint_plan(
                    params.text_origin_x,
                    row_y,
                    params.cell_width,
                    params.line_height,
                    connector,
                    row.color,
                ),
            );
        }

        paint_inline_diagnostic_text_line(
            window,
            cx,
            InlineDiagnosticTextPaintParams {
                line_cache: params.line_cache,
                line: row,
                font: params.font.clone(),
                x: params.text_origin_x + params.cell_width * row.text_col as f32,
                y: row_y,
                font_size: params.font_size,
                viewport_width: params.viewport_width,
                line_height: params.line_height,
            },
        );
    }
}

fn paint_inline_diagnostic_text_line(
    window: &mut Window,
    cx: &mut App,
    params: InlineDiagnosticTextPaintParams<'_>,
) {
    if params.line.text.is_empty() {
        return;
    }

    let runs = [TextRun {
        len: params.line.text.len(),
        font: params.font,
        color: params.line.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let text_system = window.text_system().clone();
    let shaped_line = params.line_cache.shape_line_cached(
        text_system.as_ref(),
        params.line.text.clone(),
        params.font_size,
        params.viewport_width,
        &runs,
    );

    let _ = shaped_line.paint(
        point(params.x, params.y),
        params.line_height,
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

pub fn paint_fallback_diagnostic_plan(
    window: &mut Window,
    cx: &mut App,
    params: FallbackDiagnosticPaintParams<'_>,
) {
    if params.plan.lines.is_empty() {
        return;
    }

    let max_width = params.cell_width * 100.0;
    let width = params.bounds.size.width.min(max_width);
    let right = params.bounds.origin.x + params.bounds.size.width;
    let top = params.bounds.origin.y + params.line_height;
    let max_lines = ((params.bounds.size.height / params.line_height) as usize)
        .saturating_sub(1)
        .min(15);

    for (index, line) in params.plan.lines.iter().take(max_lines).enumerate() {
        let runs = [TextRun {
            len: line.text.len(),
            font: params.font.clone(),
            color: line.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        let text_system = window.text_system().clone();
        let shaped_line = params.line_cache.shape_line_cached(
            text_system.as_ref(),
            line.text.clone(),
            params.font_size,
            width,
            &runs,
        );
        let x = (right - shaped_line.width).max(right - width);
        let y = top + params.line_height * index as f32;
        let _ = shaped_line.paint(
            point(x, y),
            params.line_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

fn paint_inline_diagnostic_connector(
    window: &mut Window,
    plan: InlineDiagnosticConnectorPaintPlan,
) {
    let mut builder = PathBuilder::stroke(plan.stroke_width);
    builder.move_to(plan.start);
    builder.line_to(plan.corner_start);
    builder.curve_to(plan.corner_end, plan.corner_control);
    builder.line_to(plan.end);

    if let Ok(path) = builder.build() {
        window.paint_path(path, plan.color);
    }
}

fn inline_diagnostic_connector_paint_plan(
    text_origin_x: Pixels,
    row_y: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
    connector: InlineDiagnosticConnector,
    color: Hsla,
) -> InlineDiagnosticConnectorPaintPlan {
    let stroke_width = (line_height * 0.075).max(px(1.0)).min(px(2.0));
    let anchor_x = text_origin_x + cell_width * connector.anchor_col as f32 + cell_width * 0.5;
    let elbow_y = row_y + line_height * 0.54;
    let horizontal_end_x = text_origin_x
        + cell_width * (connector.anchor_col + usize::from(connector.prefix_len) + 1) as f32;
    let radius = (cell_width * 0.45)
        .min((elbow_y - row_y) * 0.6)
        .max(px(2.0));

    InlineDiagnosticConnectorPaintPlan {
        start: point(anchor_x, row_y),
        corner_start: point(anchor_x, elbow_y - radius),
        corner_control: point(anchor_x, elbow_y),
        corner_end: point(anchor_x + radius, elbow_y),
        end: point(horizontal_end_x, elbow_y),
        stroke_width,
        color,
    }
}

fn filtered_inline_diagnostics<'a>(
    diagnostics: &'a [&'a Diagnostic],
    filter: DiagnosticFilter,
    max_diagnostics: usize,
) -> Vec<&'a Diagnostic> {
    let DiagnosticFilter::Enable(filter) = filter else {
        return Vec::new();
    };

    diagnostics
        .iter()
        .copied()
        .filter(|diagnostic| diagnostic_severity(diagnostic) >= filter)
        .take(max_diagnostics)
        .collect()
}

fn eol_diagnostic<'a>(
    diagnostics: &'a [&'a Diagnostic],
    inline_filter: DiagnosticFilter,
    eol_filter: DiagnosticFilter,
) -> Option<&'a Diagnostic> {
    let DiagnosticFilter::Enable(eol_filter) = eol_filter else {
        return None;
    };

    diagnostics
        .iter()
        .copied()
        .filter(|diagnostic| diagnostic_severity(diagnostic) >= eol_filter)
        .filter(|diagnostic| match inline_filter {
            DiagnosticFilter::Enable(inline_filter) => {
                diagnostic_severity(diagnostic) < inline_filter
            }
            DiagnosticFilter::Disable => true,
        })
        .max_by_key(|diagnostic| diagnostic_severity(diagnostic))
}

fn inline_diagnostic_rows(
    diagnostic: &Diagnostic,
    anchor_col: usize,
    viewport_columns: u16,
    config: &InlineDiagnosticsConfig,
    colors: InlineDiagnosticColors,
) -> Vec<InlineDiagnosticTextLine> {
    let severity = diagnostic_severity(diagnostic);
    let color = colors.color_for(severity);
    let anchor_col = anchor_col.min(config.max_diagnostic_start(viewport_columns) as usize);
    let connector = InlineDiagnosticConnector {
        anchor_col,
        prefix_len: config.prefix_len,
    };
    let text_col = diagnostic_text_col(anchor_col, config.prefix_len);
    let text_width = usize::from(viewport_columns)
        .saturating_sub(text_col)
        .max(1);
    let mut rows = Vec::new();

    for message_line in diagnostic.message.trim().lines() {
        let message_line = message_line.trim();
        if message_line.is_empty() {
            continue;
        }

        for wrapped in wrap_message_line(message_line, text_width) {
            rows.push(InlineDiagnosticTextLine {
                text: SharedString::from(wrapped),
                severity,
                color,
                text_col,
                connector: rows.is_empty().then_some(connector),
            });
        }
    }

    rows
}

fn diagnostic_text_col(anchor_col: usize, prefix_len: u16) -> usize {
    anchor_col + usize::from(prefix_len) + 2
}

fn wrap_message_line(message: &str, width: usize) -> Vec<String> {
    if message.is_empty() {
        return vec![String::new()];
    }

    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in message.split_whitespace() {
        let current_width = display_width(&current);
        let word_width = display_width(word);
        let separator = usize::from(!current.is_empty());
        if current_width > 0 && current_width + separator + word_width > width {
            lines.push(current);
            current = String::new();
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

fn diagnostic_anchor_column(
    text: RopeSlice<'_>,
    doc_line: usize,
    diagnostic_start: usize,
    horizontal_offset: usize,
    tab_width: u16,
) -> usize {
    let line_start = text.line_to_char(doc_line);
    let line_end = if doc_line + 1 < text.len_lines() {
        text.line_to_char(doc_line + 1)
    } else {
        text.len_chars()
    };
    let diagnostic_start = diagnostic_start.min(line_end).max(line_start);
    let mut column = 0;

    for ch in text.slice(line_start..diagnostic_start).chars() {
        if ch == '\t' {
            column += tab_width_at(column, tab_width);
        } else {
            column += grapheme_width(&ch.to_string());
        }
    }

    column.saturating_sub(horizontal_offset)
}

fn first_message_line(message: &str) -> &str {
    message.trim().lines().next().unwrap_or("")
}

fn display_width(text: &str) -> usize {
    text.chars().map(|ch| grapheme_width(&ch.to_string())).sum()
}

fn inline_diagnostic_colors(theme: &Theme) -> InlineDiagnosticColors {
    InlineDiagnosticColors {
        hint: diagnostic_text_color(theme, "hint", "diagnostic.hint"),
        info: diagnostic_text_color(theme, "info", "diagnostic.info"),
        warning: diagnostic_text_color(theme, "warning", "diagnostic.warning"),
        error: diagnostic_text_color(theme, "error", "diagnostic.error"),
    }
}

fn diagnostic_text_color(theme: &Theme, primary: &str, fallback: &str) -> Hsla {
    style_color(theme.get(primary))
        .or_else(|| style_color(theme.get(fallback)))
        .unwrap_or_else(|| match primary {
            "error" => gpui::rgb(0xff5f5f).into(),
            "warning" => gpui::rgb(0xe5c07b).into(),
            "info" => gpui::rgb(0x61afef).into(),
            _ => gpui::rgb(0x98c379).into(),
        })
}

fn style_color(style: Style) -> Option<Hsla> {
    style
        .fg
        .or(style.underline_color)
        .or(style.bg)
        .and_then(|color| {
            if color == Color::Reset {
                None
            } else {
                helix_color_to_hsla(color)
            }
        })
}

fn diagnostic_severity(diagnostic: &Diagnostic) -> Severity {
    diagnostic.severity.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::rgb;
    use helix_core::diagnostic::{
        DiagnosticProvider, LanguageServerId, NumberOrString, Range as DiagnosticRange,
    };

    fn diagnostic(start: usize, end: usize, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            range: DiagnosticRange { start, end },
            ends_at_word: false,
            starts_at_word: false,
            zero_width: start == end,
            line: 0,
            message: message.to_string(),
            severity: Some(severity),
            code: Some(NumberOrString::String("E000".to_string())),
            provider: DiagnosticProvider::Lsp {
                server_id: LanguageServerId::default(),
                identifier: None,
            },
            tags: Vec::new(),
            source: None,
            data: None,
        }
    }

    #[test]
    fn eol_diagnostic_uses_message_not_shown_inline() {
        let warning = diagnostic(0, 1, Severity::Warning, "warning");
        let hint = diagnostic(0, 1, Severity::Hint, "hint");
        let diagnostics = vec![&warning, &hint];

        let eol = eol_diagnostic(
            &diagnostics,
            DiagnosticFilter::Enable(Severity::Warning),
            DiagnosticFilter::Enable(Severity::Hint),
        )
        .expect("eol diagnostic");

        assert_eq!(eol.message, "hint");
    }

    #[test]
    fn inline_rows_respect_anchor_and_prefix_geometry() {
        let diag = diagnostic(4, 5, Severity::Error, "missing value");
        let config = InlineDiagnosticsConfig {
            cursor_line: DiagnosticFilter::Enable(Severity::Warning),
            other_lines: DiagnosticFilter::Disable,
            min_diagnostic_width: 10,
            prefix_len: 2,
            max_wrap: 20,
            max_diagnostics: 10,
        };
        let colors = InlineDiagnosticColors {
            hint: rgb(0x111111).into(),
            info: rgb(0x222222).into(),
            warning: rgb(0x333333).into(),
            error: rgb(0x444444).into(),
        };

        let rows = inline_diagnostic_rows(&diag, 4, 80, &config, colors);

        assert_eq!(rows[0].text.as_ref(), "missing value");
        assert_eq!(rows[0].text_col, 8);
        assert_eq!(
            rows[0].connector,
            Some(InlineDiagnosticConnector {
                anchor_col: 4,
                prefix_len: 2,
            })
        );
        assert_eq!(rows[0].severity, Severity::Error);
    }

    #[test]
    fn wrapped_inline_rows_align_without_repeating_connector() {
        let diag = diagnostic(4, 5, Severity::Error, "alpha beta gamma");
        let config = InlineDiagnosticsConfig {
            cursor_line: DiagnosticFilter::Enable(Severity::Warning),
            other_lines: DiagnosticFilter::Disable,
            min_diagnostic_width: 10,
            prefix_len: 1,
            max_wrap: 20,
            max_diagnostics: 10,
        };
        let colors = InlineDiagnosticColors {
            hint: rgb(0x111111).into(),
            info: rgb(0x222222).into(),
            warning: rgb(0x333333).into(),
            error: rgb(0x444444).into(),
        };

        let rows = inline_diagnostic_rows(&diag, 4, 17, &config, colors);

        assert_eq!(rows[0].text.as_ref(), "alpha beta");
        assert!(rows[0].connector.is_some());
        assert_eq!(rows[1].text.as_ref(), "gamma");
        assert_eq!(rows[1].text_col, rows[0].text_col);
        assert_eq!(rows[1].connector, None);
    }

    #[test]
    fn inline_rows_skip_blank_message_lines() {
        let diag = diagnostic(4, 5, Severity::Error, "cannot find value\n\nnot found");
        let config = InlineDiagnosticsConfig {
            cursor_line: DiagnosticFilter::Enable(Severity::Warning),
            other_lines: DiagnosticFilter::Disable,
            min_diagnostic_width: 10,
            prefix_len: 1,
            max_wrap: 20,
            max_diagnostics: 10,
        };
        let colors = InlineDiagnosticColors {
            hint: rgb(0x111111).into(),
            info: rgb(0x222222).into(),
            warning: rgb(0x333333).into(),
            error: rgb(0x444444).into(),
        };

        let rows = inline_diagnostic_rows(&diag, 4, 80, &config, colors);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text.as_ref(), "cannot find value");
        assert!(rows[0].connector.is_some());
        assert_eq!(rows[1].text.as_ref(), "not found");
        assert_eq!(rows[1].connector, None);
    }

    #[test]
    fn inline_rows_do_not_reserve_space_for_empty_message() {
        let diag = diagnostic(4, 5, Severity::Error, " \n\t ");
        let config = InlineDiagnosticsConfig {
            cursor_line: DiagnosticFilter::Enable(Severity::Warning),
            other_lines: DiagnosticFilter::Disable,
            min_diagnostic_width: 10,
            prefix_len: 1,
            max_wrap: 20,
            max_diagnostics: 10,
        };
        let colors = InlineDiagnosticColors {
            hint: rgb(0x111111).into(),
            info: rgb(0x222222).into(),
            warning: rgb(0x333333).into(),
            error: rgb(0x444444).into(),
        };

        let rows = inline_diagnostic_rows(&diag, 4, 80, &config, colors);

        assert!(rows.is_empty());
    }

    #[test]
    fn connector_paint_plan_uses_grid_columns() {
        let color = rgb(0x444444).into();
        let plan = inline_diagnostic_connector_paint_plan(
            px(100.0),
            px(50.0),
            px(8.0),
            px(20.0),
            InlineDiagnosticConnector {
                anchor_col: 4,
                prefix_len: 2,
            },
            color,
        );

        assert_point_close(plan.start, point(px(136.0), px(50.0)));
        assert_point_close(plan.corner_start, point(px(136.0), px(57.2)));
        assert_point_close(plan.corner_control, point(px(136.0), px(60.8)));
        assert_point_close(plan.corner_end, point(px(139.6), px(60.8)));
        assert_point_close(plan.end, point(px(156.0), px(60.8)));
        assert_pixels_close(plan.stroke_width, px(1.5));
        assert_eq!(plan.color, color);
    }

    fn assert_point_close(actual: Point<Pixels>, expected: Point<Pixels>) {
        assert_pixels_close(actual.x, expected.x);
        assert_pixels_close(actual.y, expected.y);
    }

    fn assert_pixels_close(actual: Pixels, expected: Pixels) {
        let delta = (f32::from(actual) - f32::from(expected)).abs();
        assert!(
            delta < 0.001,
            "expected {actual:?} to be within 0.001px of {expected:?}"
        );
    }

    #[test]
    fn wrap_message_line_splits_on_words() {
        assert_eq!(
            wrap_message_line("alpha beta gamma", 10),
            vec!["alpha beta".to_string(), "gamma".to_string()]
        );
    }

    #[test]
    fn diagnostic_anchor_column_expands_tabs() {
        let text: RopeSlice<'_> = "\tlet value".into();

        assert_eq!(diagnostic_anchor_column(text, 0, 1, 0, 4), 4);
    }
}
