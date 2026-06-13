// ABOUTME: Native editor diagnostic metadata helpers
// ABOUTME: Derives per-line diagnostic severity for gutter and highlight rendering

use std::collections::BTreeMap;

use gpui::{
    BorderStyle, Bounds, Hsla, Pixels, Point, Window, fill, point, px, quad, size,
    transparent_black,
};
use helix_core::diagnostic::Severity;
use helix_view::{Document, Theme};

use crate::style::helix_color_to_hsla;

pub type DiagnosticSeverityByLine = BTreeMap<usize, Severity>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiagnosticMarkerShape {
    Square {
        corner_radius: Pixels,
    },
    Triangle {
        top: Point<Pixels>,
        bottom_left: Point<Pixels>,
        bottom_right: Point<Pixels>,
    },
    Circle {
        radius: Pixels,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiagnosticMarkerHighlight {
    pub bounds: Bounds<Pixels>,
    pub radius: Pixels,
    pub alpha: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticMarkerPlan {
    pub severity: Severity,
    pub strip_bounds: Bounds<Pixels>,
    pub marker_bounds: Bounds<Pixels>,
    pub shape: DiagnosticMarkerShape,
    pub highlights: Vec<DiagnosticMarkerHighlight>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiagnosticMarkerPaintStyle {
    pub strip_fill: Option<Hsla>,
    pub marker_fill: Hsla,
    pub marker_border: Hsla,
    pub highlight_base: Hsla,
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
    pub marker_color: Hsla,
    pub highlight_base: Hsla,
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
            params.marker_color,
            params.highlight_base,
            params.gutter_bg,
        ),
    })
}

pub fn diagnostic_marker_plan(
    gutter_origin: Point<Pixels>,
    row_y: Pixels,
    line_height: Pixels,
    severity: Severity,
) -> DiagnosticMarkerPlan {
    let marker_size = (line_height * 0.6).max(px(2.0));
    let marker_x = gutter_origin.x + px(2.0);
    let marker_y = row_y + (line_height - marker_size) * 0.5;
    let marker_bounds = Bounds::new(point(marker_x, marker_y), size(marker_size, marker_size));
    let strip_bounds = Bounds::new(
        point(gutter_origin.x, row_y),
        size(marker_size + px(4.0), line_height),
    );

    let (shape, highlights) = match severity {
        Severity::Error => {
            let h_size = marker_size * 0.22;
            (
                DiagnosticMarkerShape::Square {
                    corner_radius: px(1.0),
                },
                vec![DiagnosticMarkerHighlight {
                    bounds: Bounds::new(
                        point(marker_x + marker_size * 0.18, marker_y + marker_size * 0.18),
                        size(h_size, h_size),
                    ),
                    radius: h_size * 0.5,
                    alpha: 0.18,
                }],
            )
        }
        Severity::Warning => {
            let h_size = marker_size * 0.2;
            (
                DiagnosticMarkerShape::Triangle {
                    top: point(marker_x + marker_size * 0.5, marker_y),
                    bottom_left: point(marker_x, marker_y + marker_size),
                    bottom_right: point(marker_x + marker_size, marker_y + marker_size),
                },
                vec![DiagnosticMarkerHighlight {
                    bounds: Bounds::new(
                        point(marker_x + marker_size * 0.22, marker_y + marker_size * 0.18),
                        size(h_size, h_size),
                    ),
                    radius: h_size * 0.5,
                    alpha: 0.14,
                }],
            )
        }
        Severity::Info | Severity::Hint => {
            let offset = marker_size * 0.14;
            let halo_size = marker_size * 0.52;
            let core_size = marker_size * 0.26;
            (
                DiagnosticMarkerShape::Circle {
                    radius: marker_size * 0.5,
                },
                vec![
                    DiagnosticMarkerHighlight {
                        bounds: Bounds::new(
                            point(marker_x + offset, marker_y + offset),
                            size(halo_size, halo_size),
                        ),
                        radius: halo_size * 0.5,
                        alpha: 0.14,
                    },
                    DiagnosticMarkerHighlight {
                        bounds: Bounds::new(
                            point(
                                marker_x + offset + (halo_size - core_size) * 0.25,
                                marker_y + offset + (halo_size - core_size) * 0.25,
                            ),
                            size(core_size, core_size),
                        ),
                        radius: core_size * 0.5,
                        alpha: 0.45,
                    },
                ],
            )
        }
    };

    DiagnosticMarkerPlan {
        severity,
        strip_bounds,
        marker_bounds,
        shape,
        highlights,
    }
}

pub fn diagnostic_marker_paint_style(
    marker_color: Hsla,
    highlight_base: Hsla,
    strip_fill: Option<Hsla>,
) -> DiagnosticMarkerPaintStyle {
    DiagnosticMarkerPaintStyle {
        strip_fill,
        marker_fill: with_alpha(marker_color, 0.85),
        marker_border: with_alpha(darken(marker_color, 0.15), 0.9),
        highlight_base,
    }
}

pub fn paint_diagnostic_marker(
    window: &mut Window,
    plan: &DiagnosticMarkerPlan,
    style: DiagnosticMarkerPaintStyle,
) {
    if let Some(strip_fill) = style.strip_fill {
        window.paint_quad(fill(plan.strip_bounds, strip_fill));
    }

    match plan.shape {
        DiagnosticMarkerShape::Square { corner_radius } => {
            window.paint_quad(quad(
                plan.marker_bounds,
                corner_radius,
                style.marker_fill,
                px(1.0),
                style.marker_border,
                BorderStyle::default(),
            ));
        }
        DiagnosticMarkerShape::Triangle {
            top,
            bottom_left,
            bottom_right,
        } => {
            let mut path = gpui::PathBuilder::fill();
            path.move_to(top);
            path.line_to(bottom_left);
            path.line_to(bottom_right);
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, style.marker_fill);
            }
        }
        DiagnosticMarkerShape::Circle { radius } => {
            window.paint_quad(quad(
                plan.marker_bounds,
                radius,
                style.marker_fill,
                px(1.0),
                style.marker_border,
                BorderStyle::default(),
            ));
        }
    }

    for highlight in &plan.highlights {
        window.paint_quad(quad(
            highlight.bounds,
            highlight.radius,
            with_alpha(style.highlight_base, highlight.alpha),
            0.0,
            transparent_black(),
            BorderStyle::default(),
        ));
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

fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

fn darken(color: Hsla, amount: f32) -> Hsla {
    Hsla {
        l: (color.l - amount).max(0.0),
        ..color
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
        let marker_color = hsla(0.1, 0.8, 0.6, 1.0);
        let highlight_base = hsla(0.0, 0.0, 1.0, 1.0);
        let gutter_bg = Some(hsla(0.2, 0.3, 0.4, 1.0));

        let plan = diagnostic_gutter_marker_paint_plan(DiagnosticGutterMarkerPaintPlanParams {
            severity_by_line: &severities,
            doc_line: 4,
            row_y: px(40.0),
            gutter_origin: point(px(10.0), px(0.0)),
            line_height: px(20.0),
            marker_color,
            highlight_base,
            gutter_bg,
        })
        .expect("diagnostic marker plan");

        assert_eq!(plan.marker.severity, Severity::Warning);
        assert_eq!(plan.style.strip_fill, gutter_bg);
        assert_eq!(plan.style.marker_fill, hsla(0.1, 0.8, 0.6, 0.85));
        assert_eq!(plan.style.highlight_base, highlight_base);
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
                marker_color: hsla(0.1, 0.8, 0.6, 1.0),
                highlight_base: hsla(0.0, 0.0, 1.0, 1.0),
                gutter_bg: None,
            })
            .is_none()
        );
    }

    #[test]
    fn marker_plan_centers_error_square_in_gutter_row() {
        let plan = diagnostic_marker_plan(
            point(px(10.0), px(0.0)),
            px(40.0),
            px(20.0),
            Severity::Error,
        );

        assert_eq!(
            plan.strip_bounds,
            Bounds::new(point(px(10.0), px(40.0)), size(px(16.0), px(20.0)))
        );
        assert_eq!(
            plan.marker_bounds,
            Bounds::new(point(px(12.0), px(44.0)), size(px(12.0), px(12.0)))
        );
        assert_eq!(
            plan.shape,
            DiagnosticMarkerShape::Square {
                corner_radius: px(1.0),
            }
        );
        assert_eq!(plan.highlights.len(), 1);
        assert_eq!(plan.highlights[0].alpha, 0.18);
    }

    #[test]
    fn marker_plan_uses_triangle_points_for_warning() {
        let plan = diagnostic_marker_plan(
            point(px(10.0), px(0.0)),
            px(40.0),
            px(20.0),
            Severity::Warning,
        );

        assert_eq!(
            plan.shape,
            DiagnosticMarkerShape::Triangle {
                top: point(px(18.0), px(44.0)),
                bottom_left: point(px(12.0), px(56.0)),
                bottom_right: point(px(24.0), px(56.0)),
            }
        );
        assert_eq!(plan.highlights.len(), 1);
        assert_eq!(plan.highlights[0].alpha, 0.14);
    }

    #[test]
    fn marker_plan_uses_two_highlights_for_info_circle() {
        let plan =
            diagnostic_marker_plan(point(px(10.0), px(0.0)), px(40.0), px(20.0), Severity::Info);

        assert_eq!(
            plan.shape,
            DiagnosticMarkerShape::Circle { radius: px(6.0) }
        );
        assert_eq!(plan.highlights.len(), 2);
        assert_eq!(plan.highlights[0].alpha, 0.14);
        assert_eq!(plan.highlights[1].alpha, 0.45);
    }

    #[test]
    fn marker_plan_enforces_minimum_marker_size() {
        let plan =
            diagnostic_marker_plan(point(px(10.0), px(0.0)), px(40.0), px(1.0), Severity::Hint);

        assert_eq!(plan.marker_bounds.size, size(px(2.0), px(2.0)));
    }

    #[test]
    fn marker_paint_style_derives_fill_border_and_highlight() {
        let marker = hsla(0.5, 0.6, 0.7, 1.0);
        let highlight = hsla(0.0, 0.0, 1.0, 1.0);
        let strip = hsla(0.2, 0.3, 0.4, 1.0);

        let style = diagnostic_marker_paint_style(marker, highlight, Some(strip));

        assert_eq!(style.strip_fill, Some(strip));
        assert_eq!(style.marker_fill, hsla(0.5, 0.6, 0.7, 0.85));
        assert_eq!(style.marker_border.h, 0.5);
        assert_eq!(style.marker_border.s, 0.6);
        assert!((style.marker_border.l - 0.55).abs() < f32::EPSILON);
        assert_eq!(style.marker_border.a, 0.9);
        assert_eq!(style.highlight_base, highlight);
    }

    #[test]
    fn marker_paint_style_clamps_darkened_border_lightness() {
        let style = diagnostic_marker_paint_style(
            hsla(0.5, 0.6, 0.05, 1.0),
            hsla(0.0, 0.0, 1.0, 1.0),
            None,
        );

        assert_eq!(style.marker_border.l, 0.0);
    }
}
