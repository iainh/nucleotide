// ABOUTME: Native editor diagnostic metadata helpers
// ABOUTME: Derives per-line diagnostic severity for gutter and highlight rendering

use std::collections::{BTreeMap, BTreeSet};

use gpui::{
    App, Bounds, Hsla, Pixels, Point, SharedString, TransformationMatrix, Window, fill, point, px,
    size,
};
use helix_core::diagnostic::Severity;
use helix_view::{Document, Theme};
use nucleotide_logging::error;

use crate::{GutterLine, style::helix_color_to_hsla};

pub type DiagnosticSeverityByLine = BTreeMap<usize, Severity>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiagnosticSeverityIconColors {
    pub error: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub hint: Hsla,
}

impl DiagnosticSeverityIconColors {
    pub fn color_for(self, severity: Severity) -> Hsla {
        match severity {
            Severity::Error => self.error,
            Severity::Warning => self.warning,
            Severity::Info => self.info,
            Severity::Hint => self.hint,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticMarkerPlan {
    pub severity: Severity,
    pub strip_bounds: Bounds<Pixels>,
    pub icon_bounds: Bounds<Pixels>,
    pub icon_path: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiagnosticMarkerPaintStyle {
    pub strip_fill: Option<Hsla>,
    pub icon_color: Hsla,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticGutterMarkerPaintPlan {
    pub marker: DiagnosticMarkerPlan,
    pub style: DiagnosticMarkerPaintStyle,
}

#[derive(Debug, Clone, Copy)]
pub struct DiagnosticGutterMarkerPaintPlanParams<'a> {
    pub severity_by_line: &'a DiagnosticSeverityByLine,
    pub doc_line: usize,
    pub row_y: Pixels,
    pub gutter_origin: Point<Pixels>,
    pub line_height: Pixels,
    pub icon_colors: DiagnosticSeverityIconColors,
    pub gutter_bg: Option<Hsla>,
}

pub struct DiagnosticGutterMarkersPaintParams<'a> {
    pub severity_by_line: &'a DiagnosticSeverityByLine,
    pub gutter_lines: &'a [GutterLine],
    pub gutter_origin: Point<Pixels>,
    pub line_height: Pixels,
    pub icon_colors: DiagnosticSeverityIconColors,
    pub gutter_bg: Option<Hsla>,
}

pub fn diagnostic_severity_by_line(document: &Document) -> DiagnosticSeverityByLine {
    let text = document.text();
    let text_len = text.len_chars();
    let mut severities = DiagnosticSeverityByLine::new();

    for diagnostic in document.diagnostics().iter() {
        let Some(severity) = diagnostic.severity else {
            continue;
        };

        let start = diagnostic.range.start.min(text_len);
        let end = diagnostic.range.end.min(text_len);
        let start_line = text.char_to_line(start);
        let end_line = text.char_to_line(end);

        for line in start_line..=end_line {
            severities
                .entry(line)
                .and_modify(|existing| *existing = strongest_severity(*existing, severity))
                .or_insert(severity);
        }
    }

    severities
}

pub fn diagnostic_severity_theme_key(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "diagnostic.error",
        Severity::Warning => "diagnostic.warning",
        Severity::Info => "diagnostic.info",
        Severity::Hint => "diagnostic.hint",
    }
}

pub fn diagnostic_severity_color(theme: &Theme, severity: Severity) -> Option<Hsla> {
    let style = theme.get(diagnostic_severity_theme_key(severity));
    style
        .underline_color
        .or(style.fg)
        .and_then(helix_color_to_hsla)
}

pub fn diagnostic_gutter_marker_paint_plan(
    params: DiagnosticGutterMarkerPaintPlanParams<'_>,
) -> Option<DiagnosticGutterMarkerPaintPlan> {
    let severity = params.severity_by_line.get(&params.doc_line).copied()?;
    Some(DiagnosticGutterMarkerPaintPlan {
        marker: diagnostic_marker_plan(
            params.gutter_origin,
            params.row_y,
            params.line_height,
            severity,
        ),
        style: diagnostic_marker_paint_style(
            params.icon_colors.color_for(severity),
            params.gutter_bg,
        ),
    })
}

pub fn paint_diagnostic_gutter_markers(
    window: &mut Window,
    cx: &mut App,
    params: DiagnosticGutterMarkersPaintParams<'_>,
) {
    let mut painted_rows = BTreeSet::new();
    for gutter_line in params.gutter_lines {
        if !gutter_line.first_visual_line
            || !painted_rows.insert((gutter_line.doc_line, gutter_line.visual_line))
        {
            continue;
        }

        let Some(marker_plan) =
            diagnostic_gutter_marker_paint_plan(DiagnosticGutterMarkerPaintPlanParams {
                severity_by_line: params.severity_by_line,
                doc_line: gutter_line.doc_line,
                row_y: gutter_line.origin.y,
                gutter_origin: params.gutter_origin,
                line_height: params.line_height,
                icon_colors: params.icon_colors,
                gutter_bg: params.gutter_bg,
            })
        else {
            continue;
        };

        paint_diagnostic_marker(window, cx, &marker_plan.marker, marker_plan.style);
    }
}

pub fn diagnostic_marker_plan(
    gutter_origin: Point<Pixels>,
    row_y: Pixels,
    line_height: Pixels,
    severity: Severity,
) -> DiagnosticMarkerPlan {
    let icon_size = (line_height * 0.7).max(px(2.0)).min(px(16.0));
    let icon_x = gutter_origin.x + px(2.0);
    let icon_y = row_y + (line_height - icon_size) * 0.5;
    let icon_bounds = Bounds::new(point(icon_x, icon_y), size(icon_size, icon_size));
    let strip_bounds = Bounds::new(
        point(gutter_origin.x, row_y),
        size(icon_size + px(4.0), line_height),
    );

    DiagnosticMarkerPlan {
        severity,
        strip_bounds,
        icon_bounds,
        icon_path: diagnostic_severity_icon_path(severity),
    }
}

pub fn diagnostic_marker_paint_style(
    icon_color: Hsla,
    strip_fill: Option<Hsla>,
) -> DiagnosticMarkerPaintStyle {
    DiagnosticMarkerPaintStyle {
        strip_fill,
        icon_color,
    }
}

pub fn paint_diagnostic_marker(
    window: &mut Window,
    cx: &mut App,
    plan: &DiagnosticMarkerPlan,
    style: DiagnosticMarkerPaintStyle,
) {
    if let Some(strip_fill) = style.strip_fill {
        window.paint_quad(fill(plan.strip_bounds, strip_fill));
    }

    if let Err(err) = window.paint_svg(
        plan.icon_bounds,
        SharedString::from(plan.icon_path),
        None,
        TransformationMatrix::default(),
        style.icon_color,
        cx,
    ) {
        error!(error = ?err, icon = plan.icon_path, "Failed to paint diagnostic gutter icon");
    }
}

pub fn diagnostic_severity_icon_path(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "icons/circle-x.svg",
        Severity::Warning => "icons/triangle-alert.svg",
        Severity::Info => "icons/info.svg",
        Severity::Hint => "icons/lightbulb.svg",
    }
}

fn strongest_severity(current: Severity, candidate: Severity) -> Severity {
    if severity_rank(candidate) < severity_rank(current) {
        candidate
    } else {
        current
    }
}

fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
        Severity::Hint => 3,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, hsla, point, px, size};
    use helix_core::diagnostic::Severity;

    use super::*;

    #[test]
    fn strongest_severity_keeps_more_serious_value() {
        assert_eq!(
            strongest_severity(Severity::Warning, Severity::Error),
            Severity::Error
        );
        assert_eq!(
            strongest_severity(Severity::Warning, Severity::Info),
            Severity::Warning
        );
    }

    #[test]
    fn severity_rank_orders_lsp_severity() {
        assert!(severity_rank(Severity::Error) < severity_rank(Severity::Warning));
        assert!(severity_rank(Severity::Warning) < severity_rank(Severity::Info));
        assert!(severity_rank(Severity::Info) < severity_rank(Severity::Hint));
    }

    #[test]
    fn severity_theme_keys_match_helix_diagnostic_scopes() {
        assert_eq!(
            diagnostic_severity_theme_key(Severity::Error),
            "diagnostic.error"
        );
        assert_eq!(
            diagnostic_severity_theme_key(Severity::Warning),
            "diagnostic.warning"
        );
        assert_eq!(
            diagnostic_severity_theme_key(Severity::Info),
            "diagnostic.info"
        );
        assert_eq!(
            diagnostic_severity_theme_key(Severity::Hint),
            "diagnostic.hint"
        );
    }

    #[test]
    fn gutter_marker_paint_plan_uses_line_severity() {
        let mut severities = DiagnosticSeverityByLine::new();
        severities.insert(4, Severity::Warning);
        let icon_colors = test_icon_colors();
        let gutter_bg = Some(hsla(0.2, 0.3, 0.4, 1.0));

        let plan = diagnostic_gutter_marker_paint_plan(DiagnosticGutterMarkerPaintPlanParams {
            severity_by_line: &severities,
            doc_line: 4,
            row_y: px(40.0),
            gutter_origin: point(px(10.0), px(0.0)),
            line_height: px(20.0),
            icon_colors,
            gutter_bg,
        })
        .expect("diagnostic marker plan");

        assert_eq!(plan.marker.severity, Severity::Warning);
        assert_eq!(plan.marker.icon_path, "icons/triangle-alert.svg");
        assert_eq!(plan.style.strip_fill, gutter_bg);
        assert_eq!(plan.style.icon_color, icon_colors.warning);
    }

    #[test]
    fn gutter_marker_paint_plan_rejects_lines_without_diagnostics() {
        let severities = DiagnosticSeverityByLine::new();

        assert!(
            diagnostic_gutter_marker_paint_plan(DiagnosticGutterMarkerPaintPlanParams {
                severity_by_line: &severities,
                doc_line: 4,
                row_y: px(40.0),
                gutter_origin: point(px(10.0), px(0.0)),
                line_height: px(20.0),
                icon_colors: test_icon_colors(),
                gutter_bg: None,
            })
            .is_none()
        );
    }

    #[test]
    fn marker_plan_centers_error_icon_in_gutter_row() {
        let plan = diagnostic_marker_plan(
            point(px(10.0), px(0.0)),
            px(40.0),
            px(20.0),
            Severity::Error,
        );

        assert_eq!(
            plan.strip_bounds,
            Bounds::new(point(px(10.0), px(40.0)), size(px(18.0), px(20.0)))
        );
        assert_eq!(
            plan.icon_bounds,
            Bounds::new(point(px(12.0), px(43.0)), size(px(14.0), px(14.0)))
        );
        assert_eq!(plan.icon_path, "icons/circle-x.svg");
    }

    #[test]
    fn severity_icon_paths_use_lucide_assets() {
        assert_eq!(
            diagnostic_severity_icon_path(Severity::Error),
            "icons/circle-x.svg"
        );
        assert_eq!(
            diagnostic_severity_icon_path(Severity::Warning),
            "icons/triangle-alert.svg"
        );
        assert_eq!(
            diagnostic_severity_icon_path(Severity::Info),
            "icons/info.svg"
        );
        assert_eq!(
            diagnostic_severity_icon_path(Severity::Hint),
            "icons/lightbulb.svg"
        );
    }

    #[test]
    fn severity_icon_colors_use_matching_token_slots() {
        let colors = test_icon_colors();

        assert_eq!(colors.color_for(Severity::Error), colors.error);
        assert_eq!(colors.color_for(Severity::Warning), colors.warning);
        assert_eq!(colors.color_for(Severity::Info), colors.info);
        assert_eq!(colors.color_for(Severity::Hint), colors.hint);
    }

    #[test]
    fn marker_plan_enforces_minimum_icon_size() {
        let plan =
            diagnostic_marker_plan(point(px(10.0), px(0.0)), px(40.0), px(1.0), Severity::Hint);

        assert_eq!(plan.icon_bounds.size, size(px(2.0), px(2.0)));
        assert_eq!(plan.icon_path, "icons/lightbulb.svg");
    }

    #[test]
    fn marker_paint_style_preserves_icon_color_and_strip() {
        let icon = hsla(0.5, 0.6, 0.7, 1.0);
        let strip = hsla(0.2, 0.3, 0.4, 1.0);

        let style = diagnostic_marker_paint_style(icon, Some(strip));

        assert_eq!(style.strip_fill, Some(strip));
        assert_eq!(style.icon_color, icon);
    }

    fn test_icon_colors() -> DiagnosticSeverityIconColors {
        DiagnosticSeverityIconColors {
            error: hsla(0.0, 0.8, 0.6, 1.0),
            warning: hsla(0.1, 0.8, 0.6, 1.0),
            info: hsla(0.2, 0.8, 0.6, 1.0),
            hint: hsla(0.3, 0.8, 0.6, 1.0),
        }
    }
}
