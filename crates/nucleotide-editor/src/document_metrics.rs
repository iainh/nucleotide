// ABOUTME: Native editor document viewport metrics
// ABOUTME: Computes visual row counts and text formatting for GPUI editor surfaces

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::Range,
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
use nucleotide_logging::{PerfTimer, trace};

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
    line_metrics: Vec<Option<EditorLineMetrics>>,
    pending_incremental_update: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EditorLineMetrics {
    visual_rows: usize,
    content_columns: usize,
}

#[derive(Clone, Debug, Default)]
pub struct EditorDocumentMetricsCache {
    entries: Vec<CachedEditorDocumentMetrics>,
    stats: EditorDocumentMetricsCacheStats,
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
const DOCUMENT_METRICS_SCAN_WARN_THRESHOLD: Duration = Duration::from_millis(4);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorDocumentMetricsCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub full_visual_row_scans: u64,
    pub incremental_updates: u64,
    pub incremental_line_scans: u64,
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
    pub fn invalidate_document_lines(
        &mut self,
        document_id: DocumentId,
        old_lines: Range<usize>,
        new_lines: Range<usize>,
    ) {
        for cached in self
            .entries
            .iter_mut()
            .filter(|cached| cached.key.document_id == document_id)
        {
            let old_start = old_lines.start.min(cached.line_metrics.len());
            let old_end = old_lines.end.min(cached.line_metrics.len()).max(old_start);
            let replacement_len = new_lines.end.saturating_sub(new_lines.start);
            cached.line_metrics.splice(
                old_start..old_end,
                std::iter::repeat_n(None, replacement_len),
            );
            cached.pending_incremental_update = true;
        }
    }

    pub fn invalidate_document_annotations(&mut self, document_id: DocumentId) {
        self.entries
            .retain(|cached| cached.key.document_id != document_id);
    }

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

        if let Some(index) = self
            .entries
            .iter()
            .position(|cached| cached.key == key && !cached.pending_incremental_update)
        {
            self.stats.hits += 1;
            trace!(
                document_id = ?params.document.id(),
                document_version = params.document.version(),
                viewport_columns = viewport_columns,
                gutter_columns = params.gutter_columns,
                cache_entries = self.entries.len(),
                "Editor document metrics cache hit"
            );
            let cached = self.entries.remove(index);
            let metrics = cached.metrics.clone();
            self.entries.insert(0, cached);
            return metrics;
        }

        self.stats.misses += 1;
        trace!(
            document_id = ?params.document.id(),
            document_version = params.document.version(),
            viewport_columns = viewport_columns,
            gutter_columns = params.gutter_columns,
            cache_entries = self.entries.len(),
            "Editor document metrics cache miss"
        );

        let annotations = params.view.text_annotations(params.document, params.theme);
        if let Some(index) = self.entries.iter().position(|cached| {
            cached.pending_incremental_update
                && cached.line_metrics.len() == params.document.text().len_lines()
                && metrics_layout_matches(&cached.key, &key)
        }) {
            let _timer = PerfTimer::new("EditorDocumentMetrics::incremental_line_scan")
                .with_warn_threshold(DOCUMENT_METRICS_SCAN_WARN_THRESHOLD);
            let mut cached = self.entries.remove(index);
            let text = params.document.text().slice(..);
            let mut scanned_lines = 0_u64;
            for (line_idx, line_metrics) in cached.line_metrics.iter_mut().enumerate() {
                if line_metrics.is_none() {
                    *line_metrics = Some(editor_line_metrics(
                        text,
                        line_idx,
                        &text_format,
                        &annotations,
                        viewport_columns,
                    ));
                    scanned_lines += 1;
                }
            }
            let metrics =
                editor_metrics_from_lines(&cached.line_metrics, viewport_columns, text_format);
            cached.key = key;
            cached.metrics = metrics.clone();
            cached.pending_incremental_update = false;
            self.entries.insert(0, cached);
            self.stats.incremental_updates += 1;
            self.stats.incremental_line_scans += scanned_lines;
            return metrics;
        }

        self.stats.full_visual_row_scans += 1;
        let _timer = PerfTimer::new("EditorDocumentMetrics::full_line_scan")
            .with_warn_threshold(DOCUMENT_METRICS_SCAN_WARN_THRESHOLD);
        let line_metrics = editor_line_metrics_for_document(
            params.document,
            &text_format,
            &annotations,
            viewport_columns,
        );
        let metrics = editor_metrics_from_lines(&line_metrics, viewport_columns, text_format);
        self.entries.insert(
            0,
            CachedEditorDocumentMetrics {
                key,
                metrics: metrics.clone(),
                line_metrics,
                pending_incremental_update: false,
            },
        );
        if self.entries.len() > DOCUMENT_METRICS_CACHE_CAPACITY {
            let evicted = self.entries.len() - DOCUMENT_METRICS_CACHE_CAPACITY;
            self.stats.evictions += evicted as u64;
            self.entries.truncate(DOCUMENT_METRICS_CACHE_CAPACITY);
        }
        metrics
    }

    pub fn stats(&self) -> EditorDocumentMetricsCacheStats {
        self.stats
    }
}

fn metrics_layout_matches(
    cached: &EditorDocumentMetricsCacheKey,
    current: &EditorDocumentMetricsCacheKey,
) -> bool {
    cached.document_id == current.document_id
        && cached.gutter_columns == current.gutter_columns
        && cached.viewport_columns == current.viewport_columns
        && cached.minimum_columns == current.minimum_columns
        && cached.text_format == current.text_format
}

fn editor_line_metrics_for_document(
    document: &Document,
    text_format: &TextFormat,
    annotations: &TextAnnotations<'_>,
    viewport_columns: u16,
) -> Vec<Option<EditorLineMetrics>> {
    let text = document.text().slice(..);
    (0..text.len_lines())
        .map(|line_idx| {
            Some(editor_line_metrics(
                text,
                line_idx,
                text_format,
                annotations,
                viewport_columns,
            ))
        })
        .collect()
}

fn editor_line_metrics(
    text: RopeSlice<'_>,
    line_idx: usize,
    text_format: &TextFormat,
    annotations: &TextAnnotations<'_>,
    viewport_columns: u16,
) -> EditorLineMetrics {
    let line_start = text.line_to_char(line_idx);
    let has_following_line = line_idx + 1 < text.len_lines();
    let line_end = if has_following_line {
        text.line_to_char(line_idx + 1)
    } else {
        text.len_chars()
    };
    let visual_end =
        visual_offset_from_block(text, line_start, line_end, text_format, annotations).0;
    let visual_rows = if has_following_line {
        visual_end.row.max(1)
    } else {
        visual_end.row.saturating_add(1).max(1)
    };
    let content_columns = if text_format.soft_wrap {
        usize::from(viewport_columns).max(1)
    } else {
        content_columns_for_line(text.slice(line_start..line_end), text_format)
    };

    EditorLineMetrics {
        visual_rows,
        content_columns,
    }
}

fn editor_metrics_from_lines(
    line_metrics: &[Option<EditorLineMetrics>],
    viewport_columns: u16,
    text_format: TextFormat,
) -> EditorDocumentMetrics {
    let viewport_columns_usize = usize::from(viewport_columns).max(1);
    let visual_rows = line_metrics
        .iter()
        .flatten()
        .map(|line| line.visual_rows)
        .sum::<usize>()
        .max(1);
    let content_columns = if text_format.soft_wrap {
        viewport_columns_usize
    } else {
        line_metrics
            .iter()
            .flatten()
            .map(|line| line.content_columns)
            .max()
            .unwrap_or(viewport_columns_usize)
            .max(viewport_columns_usize)
    };

    EditorDocumentMetrics {
        viewport_columns,
        content_columns,
        soft_wrap: text_format.soft_wrap,
        visual_rows,
        text_format,
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
    let _timer = PerfTimer::new("EditorDocumentMetrics::visual_rows_for_text")
        .with_warn_threshold(DOCUMENT_METRICS_SCAN_WARN_THRESHOLD);
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
            text.line_to_char(line_idx + 1)
        } else {
            text.len_chars()
        };
        max_columns = max_columns.max(content_columns_for_line(
            text.slice(line_start..line_end),
            text_format,
        ));
    }

    max_columns
}

fn content_columns_for_line(line: RopeSlice<'_>, text_format: &TextFormat) -> usize {
    let mut visual_columns = 0;
    for grapheme in line.graphemes() {
        if grapheme.len_chars() == 1 {
            match grapheme.char(0) {
                '\t' => {
                    visual_columns += tab_width_at(visual_columns, text_format.tab_width);
                    continue;
                }
                '\n' | '\r' => continue,
                _ => {}
            }
        }
        let width = if let Some(grapheme) = grapheme.as_str() {
            grapheme_width(grapheme)
        } else {
            grapheme_width(&grapheme.to_string())
        };
        visual_columns += width;
    }
    visual_columns
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
    fn per_line_metrics_match_full_document_scans() {
        let cases = [
            ("", TextFormat::default(), 8),
            ("one\ntwo\nthree", TextFormat::default(), 8),
            ("one\ntwo\nthree\n", TextFormat::default(), 8),
            (
                "abcdefghij\nx",
                TextFormat {
                    soft_wrap: true,
                    viewport_width: 4,
                    ..TextFormat::default()
                },
                4,
            ),
        ];

        for (source, text_format, viewport_columns) in cases {
            let text = Rope::from(source);
            let annotations = TextAnnotations::default();
            let line_metrics = (0..text.len_lines())
                .map(|line_idx| {
                    Some(editor_line_metrics(
                        text.slice(..),
                        line_idx,
                        &text_format,
                        &annotations,
                        viewport_columns,
                    ))
                })
                .collect::<Vec<_>>();
            let metrics =
                editor_metrics_from_lines(&line_metrics, viewport_columns, text_format.clone());

            assert_eq!(
                metrics.visual_rows,
                visual_rows_for_text(text.slice(..), &text_format),
                "visual row mismatch for {source:?}",
            );
            assert_eq!(
                metrics.content_columns,
                content_columns_for_text(text.slice(..), &text_format, viewport_columns),
                "content width mismatch for {source:?}",
            );
        }
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
        assert_eq!(
            cache.stats(),
            EditorDocumentMetricsCacheStats {
                hits: 1,
                misses: 1,
                evictions: 0,
                full_visual_row_scans: 1,
                incremental_updates: 0,
                incremental_line_scans: 0,
            }
        );
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
        assert_eq!(
            cache.stats(),
            EditorDocumentMetricsCacheStats {
                hits: 1,
                misses: 2,
                evictions: 0,
                full_visual_row_scans: 2,
                incremental_updates: 0,
                incremental_line_scans: 0,
            }
        );
    }

    #[test]
    fn metrics_cache_counts_evictions() {
        let mut config = Config::default();
        config.soft_wrap.enable = Some(true);
        let (document, view) = test_document_with_config(config, "abcdefghijklmnopqrstuvwxyz");
        let mut cache = EditorDocumentMetricsCache::default();
        let gutter_columns = view.gutter_offset(&document);

        for width in [160.0, 240.0, 320.0, 400.0, 480.0] {
            let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(width), px(80.0)));
            cache.resolve(EditorDocumentMetricsCacheResolveParams {
                document: &document,
                view: &view,
                theme: None,
                bounds,
                gutter_columns,
                cell_width: px(8.0),
                minimum_columns: 1,
            });
        }

        assert_eq!(cache.len(), DOCUMENT_METRICS_CACHE_CAPACITY);
        assert_eq!(
            cache.stats(),
            EditorDocumentMetricsCacheStats {
                hits: 0,
                misses: 5,
                evictions: 1,
                full_visual_row_scans: 5,
                incremental_updates: 0,
                incremental_line_scans: 0,
            }
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
        assert_eq!(
            cache.stats(),
            EditorDocumentMetricsCacheStats {
                hits: 0,
                misses: 2,
                evictions: 0,
                full_visual_row_scans: 2,
                incremental_updates: 0,
                incremental_line_scans: 0,
            }
        );
    }

    #[test]
    fn metrics_cache_rescans_only_changed_lines() {
        let (mut document, view) =
            test_document_with_config(Config::default(), "one\ntwo\nthree\n");
        let mut cache = EditorDocumentMetricsCache::default();
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(160.0), px(80.0)));
        let gutter_columns = view.gutter_offset(&document);

        cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });

        let transaction = Transaction::change(
            document.text(),
            [(4, 4, Some("alpha\nbeta\n".into()))].into_iter(),
        );
        document.apply(&transaction, view.id);
        cache.invalidate_document_lines(document.id(), 1..2, 1..4);

        let incremental = cache.resolve(EditorDocumentMetricsCacheResolveParams {
            document: &document,
            view: &view,
            theme: None,
            bounds,
            gutter_columns,
            cell_width: px(8.0),
            minimum_columns: 1,
        });
        let full = EditorDocumentMetrics::resolve_for_view(
            &document,
            &view,
            None,
            bounds,
            gutter_columns,
            px(8.0),
            1,
        );

        assert_eq!(incremental.visual_rows, full.visual_rows);
        assert_eq!(incremental.content_columns, full.content_columns);
        assert_eq!(
            cache.stats(),
            EditorDocumentMetricsCacheStats {
                hits: 0,
                misses: 2,
                evictions: 0,
                full_visual_row_scans: 1,
                incremental_updates: 1,
                incremental_line_scans: 3,
            }
        );
    }
}
