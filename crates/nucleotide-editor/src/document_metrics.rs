// ABOUTME: Native editor document viewport metrics
// ABOUTME: Computes visual row counts and text formatting for GPUI editor surfaces

use std::time::Duration;

use gpui::{Bounds, Pixels};
use helix_core::{RopeSlice, doc_formatter::TextFormat, softwrapped_dimensions};
use helix_view::{Document, Theme};
use nucleotide_logging::PerfTimer;

use crate::EditorSurfaceGeometry;

pub struct EditorDocumentMetrics {
    pub viewport_columns: u16,
    pub soft_wrap: bool,
    pub visual_rows: usize,
    pub text_format: TextFormat,
}

impl EditorDocumentMetrics {
    pub fn resolve(
        document: &Document,
        theme: Option<&Theme>,
        bounds: Bounds<Pixels>,
        gutter_columns: u16,
        cell_width: Pixels,
        minimum_columns: u16,
    ) -> Self {
        let _timer = PerfTimer::new("EditorDocumentMetrics::resolve")
            .with_warn_threshold(Duration::from_millis(4));
        let (viewport_columns, text_format) = document_text_format_for_surface(
            document,
            theme,
            bounds,
            gutter_columns,
            cell_width,
            minimum_columns,
        );
        let visual_rows = visual_rows_for_text(document.text().slice(..), &text_format);

        Self {
            viewport_columns,
            soft_wrap: text_format.soft_wrap,
            visual_rows,
            text_format,
        }
    }
}

pub fn document_text_format_for_surface(
    document: &Document,
    theme: Option<&Theme>,
    bounds: Bounds<Pixels>,
    gutter_columns: u16,
    cell_width: Pixels,
    minimum_columns: u16,
) -> (u16, TextFormat) {
    let viewport_columns = EditorSurfaceGeometry::new(bounds, gutter_columns, cell_width)
        .viewport_columns(minimum_columns);

    (
        viewport_columns,
        document.text_format(viewport_columns, theme),
    )
}

pub fn visual_rows_for_text(text: RopeSlice<'_>, text_format: &TextFormat) -> usize {
    if text_format.soft_wrap {
        softwrapped_dimensions(text, text_format).0.max(1)
    } else {
        text.len_lines().max(1)
    }
}

#[cfg(test)]
mod tests {
    use helix_core::Rope;

    use super::*;

    #[test]
    fn non_wrapped_visual_rows_match_rope_lines() {
        let text = Rope::from("one\ntwo\nthree");
        let text_format = TextFormat::default();

        assert_eq!(visual_rows_for_text(text.slice(..), &text_format), 3);
    }

    #[test]
    fn empty_documents_still_have_one_visual_row() {
        let text = Rope::from("");
        let text_format = TextFormat::default();

        assert_eq!(visual_rows_for_text(text.slice(..), &text_format), 1);
    }

    #[test]
    fn soft_wrapped_visual_rows_use_formatted_dimensions() {
        let text = Rope::from("abcdef");
        let text_format = TextFormat {
            soft_wrap: true,
            viewport_width: 3,
            ..TextFormat::default()
        };

        assert!(visual_rows_for_text(text.slice(..), &text_format) > 1);
    }
}
