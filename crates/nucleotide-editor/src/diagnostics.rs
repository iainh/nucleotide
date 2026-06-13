// ABOUTME: Native editor diagnostic metadata helpers
// ABOUTME: Derives per-line diagnostic severity for gutter and highlight rendering

use std::collections::BTreeMap;

use gpui::{Bounds, Pixels, Point, point, px, size};
use helix_core::diagnostic::Severity;
use helix_view::Document;

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
    use gpui::{Bounds, point, px, size};
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
}
