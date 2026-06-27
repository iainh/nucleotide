// ABOUTME: Native editor document viewport metrics
// ABOUTME: Computes visual row counts and text formatting for GPUI editor surfaces

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use gpui::{Bounds, Pixels};
use helix_core::{
    RopeSlice,
    diagnostic::Severity,
    doc_formatter::TextFormat,
    graphemes::{grapheme_width, tab_width_at},
    text_annotations::TextAnnotations,
    visual_offset_from_block,
};
use helix_stdx::rope::RopeSliceExt;
use helix_view::{Document, DocumentId, Theme};
use nucleotide_logging::PerfTimer;

use crate::EditorSurfaceGeometry;

#[derive(Clone, Debug)]
pub struct EditorDocumentMetrics {
    pub viewport_columns: u16,
    pub content_columns: usize,
    pub soft_wrap: bool,
    pub visual_rows: usize,
    pub text_format: TextFormat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditorTextFormatCacheKey {
    soft_wrap: bool,
    tab_width: u16,
    max_wrap: u16,
    max_indent_retain: u16,
    wrap_indicator: Box<str>,
    wrap_indicator_highlight: Option<u32>,
    viewport_width: u16,
    soft_wrap_at_text_width: bool,
}

impl From<&TextFormat> for EditorTextFormatCacheKey {
    fn from(text_format: &TextFormat) -> Self {
        Self {
            soft_wrap: text_format.soft_wrap,
            tab_width: text_format.tab_width,
            max_wrap: text_format.max_wrap,
            max_indent_retain: text_format.max_indent_retain,
            wrap_indicator: text_format.wrap_indicator.clone(),
            wrap_indicator_highlight: text_format
                .wrap_indicator_highlight
                .map(|highlight| highlight.get()),
            viewport_width: text_format.viewport_width,
            soft_wrap_at_text_width: text_format.soft_wrap_at_text_width,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditorDocumentMetricsCacheKey {
    document_id: DocumentId,
    document_version: i32,
    text_len: usize,
    diagnostics_hash: u64,
    gutter_columns: u16,
    viewport_columns: u16,
    minimum_columns: u16,
    text_format: EditorTextFormatCacheKey,
}

#[derive(Clone, Debug)]
struct CachedEditorDocumentMetrics {
    key: EditorDocumentMetricsCacheKey,
    metrics: EditorDocumentMetrics,
}

#[derive(Clone, Debug, Default)]
pub struct EditorDocumentMetricsCache {
    entries: Vec<CachedEditorDocumentMetrics>,
}

pub struct EditorDocumentMetricsCacheResolveParams<'a> {
    pub document: &'a Document,
    pub view: &'a helix_view::View,
    pub theme: Option<&'a Theme>,
    pub bounds: Bounds<Pixels>,
    pub gutter_columns: u16,
    pub cell_width: Pixels,
    pub minimum_columns: u16,
}

const DOCUMENT_METRICS_CACHE_CAPACITY: usize = 4;

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
        Self::from_text_format(document, viewport_columns, text_format)
    }

    pub fn resolve_for_view(
        document: &Document,
        view: &helix_view::View,
        theme: Option<&Theme>,
        bounds: Bounds<Pixels>,
        gutter_columns: u16,
        cell_width: Pixels,
        minimum_columns: u16,
    ) -> Self {
        let _timer = PerfTimer::new("EditorDocumentMetrics::resolve_for_view")
            .with_warn_threshold(Duration::from_millis(4));
        let (viewport_columns, text_format) = document_text_format_for_surface(
            document,
            theme,
            bounds,
            gutter_columns,
            cell_width,
            minimum_columns,
        );
        let annotations = view.text_annotations(document, theme);
        Self::from_text_format_with_annotations(
            document,
            viewport_columns,
            text_format,
            &annotations,
        )
    }

    fn from_text_format(
        document: &Document,
        viewport_columns: u16,
        text_format: TextFormat,
    ) -> Self {
        let annotations = TextAnnotations::default();
        Self::from_text_format_with_annotations(
            document,
            viewport_columns,
            text_format,
            &annotations,
        )
    }

    fn from_text_format_with_annotations(
        document: &Document,
        viewport_columns: u16,
        text_format: TextFormat,
        annotations: &TextAnnotations<'_>,
    ) -> Self {
        let visual_rows = visual_rows_for_text_with_annotations(
            document.text().slice(..),
            &text_format,
            annotations,
        );
        let content_columns =
            content_columns_for_text(document.text().slice(..), &text_format, viewport_columns);
        Self {
            viewport_columns,
            content_columns,
            soft_wrap: text_format.soft_wrap,
            visual_rows,
            text_format,
        }
    }
}

impl EditorDocumentMetricsCache {
    pub fn resolve(
        &mut self,
        params: EditorDocumentMetricsCacheResolveParams<'_>,
    ) -> EditorDocumentMetrics {
        let (viewport_columns, text_format) = document_text_format_for_surface(
            params.document,
            params.theme,
            params.bounds,
            params.gutter_columns,
            params.cell_width,
            params.minimum_columns,
        );
        let key = EditorDocumentMetricsCacheKey {
            document_id: params.document.id(),
            document_version: params.document.version(),
            text_len: params.document.text().len_chars(),
            diagnostics_hash: diagnostics_hash(params.document),
            gutter_columns: params.gutter_columns,
            viewport_columns,
            minimum_columns: params.minimum_columns,
            text_format: EditorTextFormatCacheKey::from(&text_format),
        };

        if let Some(index) = self.entries.iter().position(|cached| cached.key == key) {
            let cached = self.entries.remove(index);
            let metrics = cached.metrics.clone();
            self.entries.insert(0, cached);
            return metrics;
        }

        let annotations = params.view.text_annotations(params.document, params.theme);
        let metrics = EditorDocumentMetrics::from_text_format_with_annotations(
            params.document,
            viewport_columns,
            text_format,
            &annotations,
        );
        self.entries.insert(
            0,
            CachedEditorDocumentMetrics {
                key,
                metrics: metrics.clone(),
            },
        );
        if self.entries.len() > DOCUMENT_METRICS_CACHE_CAPACITY {
            self.entries.truncate(DOCUMENT_METRICS_CACHE_CAPACITY);
        }
        metrics
    }
}

#[cfg(test)]
impl EditorDocumentMetricsCache {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn front_viewport_columns(&self) -> Option<u16> {
        self.entries
            .first()
            .map(|entry| entry.metrics.viewport_columns)
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
    let annotations = TextAnnotations::default();
    visual_rows_for_text_with_annotations(text, text_format, &annotations)
}

pub fn visual_rows_for_text_with_annotations(
    text: RopeSlice<'_>,
    text_format: &TextFormat,
    annotations: &TextAnnotations<'_>,
) -> usize {
    visual_offset_from_block(text, 0, text.len_chars(), text_format, annotations)
        .0
        .row
        .saturating_add(1)
        .max(1)
}

fn diagnostics_hash(document: &Document) -> u64 {
    let mut hasher = DefaultHasher::new();
    document.diagnostics().len().hash(&mut hasher);
    for diagnostic in document.diagnostics() {
        diagnostic.range.start.hash(&mut hasher);
        diagnostic.range.end.hash(&mut hasher);
        diagnostic.severity.map(severity_rank).hash(&mut hasher);
        diagnostic.message.hash(&mut hasher);
    }
    hasher.finish()
}

fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Hint => 0,
        Severity::Info => 1,
        Severity::Warning => 2,
        Severity::Error => 3,
    }
}

pub fn content_columns_for_text(
    text: RopeSlice<'_>,
    text_format: &TextFormat,
    viewport_columns: u16,
) -> usize {
    let viewport_columns = usize::from(viewport_columns).max(1);
    if text_format.soft_wrap {
        return viewport_columns;
    }

    let total_lines = text.len_lines();
    let mut max_columns = viewport_columns;

    for line_idx in 0..total_lines {
        let line_start = text.line_to_char(line_idx);
        let line_end = if line_idx + 1 < total_lines {
            text.line_to_char(line_idx + 1).saturating_sub(1)
        } else {
            text.len_chars()
        };
        let line_end = line_end.max(line_start);
        let mut visual_columns = 0;
        for grapheme in text.slice(line_start..line_end).graphemes() {
            if grapheme.len_chars() == 1 && grapheme.char(0) == '\t' {
                visual_columns += tab_width_at(visual_columns, text_format.tab_width);
            } else {
                let width = if let Some(grapheme) = grapheme.as_str() {
                    grapheme_width(grapheme)
                } else {
                    grapheme_width(&grapheme.to_string())
                };
                visual_columns += width;
            }
        }
        max_columns = max_columns.max(visual_columns);
    }

    max_columns
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use gpui::{point, px, size};
    use helix_core::{Rope, Transaction, syntax};
    use helix_view::{Document, DocumentId, View, editor::Config, editor::GutterConfig};

    use super::*;

    fn test_document_with_config(config: Config, text: &str) -> (Document, View) {
        let config = Arc::new(ArcSwap::new(Arc::new(config)));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let mut document = Document::from(Rope::from(text), None, config, syntax_loader);
        let view = View::new(DocumentId::default(), GutterConfig::default());
        document.ensure_view_init(view.id);

        (document, view)
    }

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

    #[test]
    fn content_columns_track_longest_unwrapped_line() {
        let text = Rope::from("short\nmuch-longer");
        let text_format = TextFormat::default();

        assert_eq!(
            content_columns_for_text(text.slice(..), &text_format, 4),
            11
        );
    }

    #[test]
    fn content_columns_respect_tab_stops() {
        let text = Rope::from("a\tb");
        let text_format = TextFormat {
            tab_width: 4,
            ..TextFormat::default()
        };

        assert_eq!(content_columns_for_text(text.slice(..), &text_format, 1), 5);
    }

    #[test]
    fn content_columns_use_viewport_for_soft_wrap() {
        let text = Rope::from("abcdefghijklmnopqrstuvwxyz");
        let text_format = TextFormat {
            soft_wrap: true,
            viewport_width: 3,
            ..TextFormat::default()
        };

        assert_eq!(content_columns_for_text(text.slice(..), &text_format, 7), 7);
    }

    #[test]
    fn metrics_cache_reuses_matching_soft_wrap_layout() {
        let mut config = Config::default();
        config.soft_wrap.enable = Some(true);
        let (document, view) = test_document_with_config(config, "abcdefghijklmnopqrstuvwxyz");
        let mut cache = EditorDocumentMetricsCache::default();
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(160.0), px(80.0)));
        let gutter_columns = view.gutter_offset(&document);

        let first = cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });
        let second = cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });

        assert!(first.soft_wrap);
        assert_eq!(second.visual_rows, first.visual_rows);
        assert_eq!(second.viewport_columns, first.viewport_columns);
    }

    #[test]
    fn metrics_cache_keeps_multiple_layouts() {
        let mut config = Config::default();
        config.soft_wrap.enable = Some(true);
        let (document, view) = test_document_with_config(config, "abcdefghijklmnopqrstuvwxyz");
        let mut cache = EditorDocumentMetricsCache::default();
        let narrow_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(160.0), px(80.0)));
        let wide_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(80.0)));
        let gutter_columns = view.gutter_offset(&document);

        let narrow = cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds: narrow_bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });
        let wide = cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds: wide_bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });

        assert_ne!(narrow.viewport_columns, wide.viewport_columns);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.front_viewport_columns(), Some(wide.viewport_columns));

        let narrow_again = cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds: narrow_bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });

        assert_eq!(narrow_again.viewport_columns, narrow.viewport_columns);
        assert_eq!(cache.len(), 2);
        assert_eq!(
            cache.front_viewport_columns(),
            Some(narrow.viewport_columns)
        );
    }

    #[test]
    fn metrics_cache_invalidates_when_document_version_changes() {
        let mut config = Config::default();
        config.soft_wrap.enable = Some(true);
        let (mut document, view) = test_document_with_config(config, "abcdefghijklmnopqrstuvwxyz");
        let mut cache = EditorDocumentMetricsCache::default();
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(160.0), px(80.0)));
        let gutter_columns = view.gutter_offset(&document);

        let first = cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });
        let insert_at = document.text().len_chars();
        let inserted_text = "abcdefghijklmnopqrstuvwxyz".repeat(20);
        let transaction = Transaction::change(
            document.text(),
            [(insert_at, insert_at, Some(inserted_text.into()))].into_iter(),
        );
        document.apply(&transaction, view.id);
        let second = cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });

        assert!(second.visual_rows > first.visual_rows);
    }
}
