// ABOUTME: Native editor inline diagnostic planning and painting
// ABOUTME: Converts Helix diagnostics/config into GPUI virtual diagnostic rows

use std::collections::BTreeMap;

use gpui::{App, Font, Hsla, Pixels, SharedString, TextAlign, TextRun, Window, point};
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
    pub viewport_width: Pixels,
    pub line_height: Pixels,
    pub text_origin_x: Pixels,
    pub source_line_y: Pixels,
    pub source_line_width: Pixels,
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
        paint_inline_diagnostic_text_line(
            window,
            cx,
            InlineDiagnosticTextPaintParams {
                line_cache: params.line_cache,
                line: row,
                font: params.font.clone(),
                x: params.text_origin_x,
                y: params.source_line_y + params.line_height * (index + 1) as f32,
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
    let prefix = diagnostic_prefix(anchor_col, config.prefix_len);
    let continuation_prefix = " ".repeat(prefix.chars().count());
    let text_width = usize::from(viewport_columns)
        .saturating_sub(prefix.chars().count())
        .max(1);
    let mut rows = Vec::new();

    for (message_index, message_line) in diagnostic.message.trim().lines().enumerate() {
        for (wrap_index, wrapped) in wrap_message_line(message_line.trim(), text_width)
            .into_iter()
            .enumerate()
        {
            let prefix = if message_index == 0 && wrap_index == 0 {
                prefix.as_str()
            } else {
                continuation_prefix.as_str()
            };
            rows.push(InlineDiagnosticTextLine {
                text: SharedString::from(format!("{prefix}{wrapped}")),
                severity,
                color,
            });
        }
    }

    if rows.is_empty() {
        rows.push(InlineDiagnosticTextLine {
            text: SharedString::from(prefix),
            severity,
            color,
        });
    }

    rows
}

fn diagnostic_prefix(anchor_col: usize, prefix_len: u16) -> String {
    let mut prefix = String::new();
    prefix.extend(std::iter::repeat_n(' ', anchor_col));
    prefix.push('└');
    prefix.extend(std::iter::repeat_n('─', usize::from(prefix_len)));
    prefix.push(' ');
    prefix
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
    fn inline_rows_respect_anchor_and_prefix() {
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

        assert_eq!(rows[0].text.as_ref(), "    └── missing value");
        assert_eq!(rows[0].severity, Severity::Error);
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
