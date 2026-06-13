// ABOUTME: Native editor diagnostic metadata helpers
// ABOUTME: Derives per-line diagnostic severity for gutter and highlight rendering

use std::collections::BTreeMap;

use helix_core::diagnostic::Severity;
use helix_view::Document;

pub type DiagnosticSeverityByLine = BTreeMap<usize, Severity>;

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
}
