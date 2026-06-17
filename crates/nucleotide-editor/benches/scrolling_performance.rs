use std::{fmt::Write as _, hint::black_box, sync::Arc};

use arc_swap::ArcSwap;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use gpui::{Bounds, Font, Hsla, black, font, point, px, size, white};
use helix_core::{Rope, Selection, syntax};
use helix_view::{
    Document, DocumentId, Theme, View, ViewId,
    document::Mode,
    editor::{Config, CursorShapeConfig, GutterConfig},
    graphics::{CursorKind, Style},
    theme::Loader as ThemeLoader,
    view::ViewPosition,
};
use nucleotide_editor::{
    EDITOR_MINIMUM_VIEWPORT_COLUMNS, EditorDocumentFrame, EditorDocumentFrameParams,
    EditorLineHighlightContext, EditorViewport, EditorViewportSurfaceLayout,
    SoftWrapHighlightedLineRunsBatchParams, SoftWrapVisualLine, UnwrappedHighlightedLine,
    UnwrappedHighlightedLinesParams, VisibleLinePlan, document_soft_wrap_render_plan,
    editor_document_frame, line_viewport_plan, soft_wrap_highlighted_line_runs_batch,
    unwrapped_highlighted_lines, unwrapped_visible_line_plans,
};

const LARGE_LINE_COUNT: usize = 20_000;
const LONG_LINE_LEN: usize = 180;
const VISIBLE_ROWS: usize = 48;

fn generated_document_text(line_count: usize, line_len: usize) -> String {
    let mut text = String::with_capacity(line_count * (line_len + 1));

    for line_idx in 0..line_count {
        let mut line = format!("fn generated_line_{line_idx:05}() {{ ");
        let target_len = line_len.saturating_sub(2).max(line.len());
        while line.len() < target_len {
            line.push_str("let value = value.wrapping_add(1); ");
        }
        line.truncate(target_len);
        let _ = writeln!(text, "{line}}}");
    }

    text
}

fn config_with_soft_wrap(soft_wrap: bool) -> Arc<ArcSwap<Config>> {
    let mut config = Config::default();
    config.soft_wrap.enable = Some(soft_wrap);
    Arc::new(ArcSwap::new(Arc::new(config)))
}

fn document_for_text(text: &str, soft_wrap: bool) -> (Document, View, ViewId) {
    let config = config_with_soft_wrap(soft_wrap);
    let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
    let mut document = Document::from(Rope::from(text), None, config, syntax_loader);
    let view = View::new(DocumentId::default(), GutterConfig::default());
    let view_id = view.id;

    document.ensure_view_init(view_id);
    document.set_selection(view_id, Selection::single(0, 0));

    (document, view, view_id)
}

struct FrameFixture {
    document: Document,
    view: View,
    view_id: ViewId,
    theme: Theme,
    syntax_loader: syntax::Loader,
    bounds: Bounds<gpui::Pixels>,
    cell_width: gpui::Pixels,
    line_height: gpui::Pixels,
    fg_color: Hsla,
    font: Font,
    default_bg: Hsla,
    cursor_shape: CursorShapeConfig,
}

impl FrameFixture {
    fn new(line_count: usize, line_len: usize, soft_wrap: bool, bounds_width: f32) -> Self {
        let text = generated_document_text(line_count, line_len);
        let (document, view, view_id) = document_for_text(&text, soft_wrap);
        let theme = ThemeLoader::new(&[]).default_theme(true);

        Self {
            document,
            view,
            view_id,
            theme,
            syntax_loader: syntax::Loader::default(),
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(bounds_width), px(960.0))),
            cell_width: px(8.0),
            line_height: px(20.0),
            fg_color: black(),
            font: font("Benchmark"),
            default_bg: white(),
            cursor_shape: CursorShapeConfig::default(),
        }
    }

    fn view_position_for_line(&self, line_idx: usize) -> ViewPosition {
        let text = self.document.text();
        let line_idx = line_idx.min(text.len_lines().saturating_sub(1));

        ViewPosition {
            anchor: text.line_to_char(line_idx),
            vertical_offset: 0,
            horizontal_offset: 0,
        }
    }

    fn frame(&self, soft_wrap_enabled: bool, first_row: usize) -> EditorDocumentFrame {
        let view_position = self.view_position_for_line(first_row);

        editor_document_frame(EditorDocumentFrameParams {
            document: &self.document,
            view: &self.view,
            view_id: self.view_id,
            theme: &self.theme,
            syntax_loader: &self.syntax_loader,
            first_row,
            last_row_from_scroll: first_row.saturating_add(VISIBLE_ROWS),
            view_position,
            soft_wrap_enabled,
            gutter_line_plans: Vec::new(),
            bounds: self.bounds,
            cell_width: self.cell_width,
            line_height: self.line_height,
            scroll_line_offset: px(0.0),
            soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            fg_color: self.fg_color,
            font: self.font.clone(),
            default_text_style: Style::default(),
            default_bg: self.default_bg,
            wrap_indicator_color: None,
            ruler_color: self.fg_color,
            editor_mode: Mode::Normal,
            cursor_kind: CursorKind::Block,
            cursor_style: Style::default(),
            cursor_shape: self.cursor_shape.clone(),
            editor_rulers: Vec::new(),
            cursorline_enabled: false,
            is_focused: true,
        })
    }

    fn highlight_context(&self, view_position: ViewPosition) -> EditorLineHighlightContext<'_> {
        EditorLineHighlightContext {
            doc: &self.document,
            view: &self.view,
            theme: &self.theme,
            syntax_loader: &self.syntax_loader,
            editor_mode: Mode::Normal,
            cursor_shape: &self.cursor_shape,
            is_view_focused: true,
            view_position,
            fg_color: self.fg_color,
            font: self.font.clone(),
            default_text_style: Style::default(),
            default_bg: self.default_bg,
            diagnostic_overlay_spans: None,
        }
    }

    fn visible_unwrapped_lines(&self, first_row: usize) -> Vec<VisibleLinePlan> {
        let text = self.document.text().slice(..);
        let viewport = line_viewport_plan(text, first_row, first_row + VISIBLE_ROWS, 0);

        unwrapped_visible_line_plans(text, viewport, self.line_height, px(0.0))
    }

    fn visible_soft_wrap_lines(&self, first_row: usize) -> Vec<SoftWrapVisualLine> {
        let view_position = self.view_position_for_line(first_row);
        let plan =
            document_soft_wrap_render_plan(nucleotide_editor::DocumentSoftWrapRenderPlanParams {
                document: &self.document,
                theme: Some(&self.theme),
                view_position,
                bounds: self.bounds,
                gutter_columns: self.view.gutter_offset(&self.document),
                cell_width: self.cell_width,
                line_height: self.line_height,
                scroll_line_offset: px(0.0),
                minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            });

        plan.visual_lines
    }

    fn highlight_unwrapped(
        &self,
        first_row: usize,
        lines: &[VisibleLinePlan],
    ) -> Vec<UnwrappedHighlightedLine> {
        let view_position = self.view_position_for_line(first_row);
        unwrapped_highlighted_lines(UnwrappedHighlightedLinesParams {
            context: self.highlight_context(view_position),
            text: self.document.text().slice(..),
            lines,
        })
    }

    fn highlight_soft_wrap(
        &self,
        first_row: usize,
        visual_lines: &[SoftWrapVisualLine],
    ) -> Vec<Vec<gpui::TextRun>> {
        let view_position = self.view_position_for_line(first_row);
        soft_wrap_highlighted_line_runs_batch(SoftWrapHighlightedLineRunsBatchParams {
            context: self.highlight_context(view_position),
            visual_lines,
            wrap_indicator_color: None,
        })
    }
}

struct ViewportFixture {
    document: Document,
    view: View,
    view_id: ViewId,
    theme: Theme,
    viewport: EditorViewport,
    bounds: Bounds<gpui::Pixels>,
    cell_width: gpui::Pixels,
    line_height: gpui::Pixels,
    max_target_row: usize,
}

impl ViewportFixture {
    fn new(line_count: usize, line_len: usize) -> Self {
        let text = generated_document_text(line_count, line_len);
        let (mut document, mut view, view_id) = document_for_text(&text, true);
        let theme = ThemeLoader::new(&[]).default_theme(true);
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(480.0), px(960.0)));
        let cell_width = px(8.0);
        let line_height = px(20.0);
        let mut viewport = EditorViewport::new(line_height);
        let update = viewport.sync_surface_layout_for_view(
            &mut document,
            &mut view,
            view_id,
            EditorViewportSurfaceLayout::for_editor(
                Some(&theme),
                bounds,
                cell_width,
                line_height,
                5,
                None,
            ),
        );
        let max_target_row = update
            .visual_rows
            .saturating_sub(viewport.visible_visual_rows().saturating_add(2))
            .max(1);

        Self {
            document,
            view,
            view_id,
            theme,
            viewport,
            bounds,
            cell_width,
            line_height,
            max_target_row,
        }
    }

    fn sync_at_visual_row(
        &mut self,
        top_visual_row: usize,
    ) -> nucleotide_editor::EditorViewportSurfaceUpdate {
        let top_visual_row = top_visual_row.min(self.max_target_row);
        self.viewport
            .scroll_to_vertical_position_from_scrollbar(self.line_height * top_visual_row as f32);

        self.viewport.sync_surface_layout_for_view(
            &mut self.document,
            &mut self.view,
            self.view_id,
            EditorViewportSurfaceLayout::for_editor(
                Some(&self.theme),
                self.bounds,
                self.cell_width,
                self.line_height,
                5,
                None,
            ),
        )
    }
}

fn bench_frame_prep(c: &mut Criterion) {
    let mut group = c.benchmark_group("scrolling_frame_prep");
    let unwrapped = FrameFixture::new(LARGE_LINE_COUNT, LONG_LINE_LEN, false, 1_200.0);
    let soft_wrap = FrameFixture::new(LARGE_LINE_COUNT, LONG_LINE_LEN, true, 480.0);
    let first_row = LARGE_LINE_COUNT / 2;

    group.bench_function("unwrapped_large_document", |b| {
        b.iter(|| black_box(unwrapped.frame(false, first_row)));
    });
    group.bench_function("soft_wrap_large_document", |b| {
        b.iter(|| black_box(soft_wrap.frame(true, first_row)));
    });

    group.finish();
}

fn bench_soft_wrap_viewport_sync(c: &mut Criterion) {
    let mut group = c.benchmark_group("scrolling_viewport_sync");

    group.bench_function("soft_wrap_deep_scroll", |b| {
        b.iter_batched(
            || {
                let fixture = ViewportFixture::new(LARGE_LINE_COUNT, LONG_LINE_LEN);
                (fixture, LARGE_LINE_COUNT / 2)
            },
            |(mut fixture, mut row)| {
                for _ in 0..32 {
                    row = if row + 37 < fixture.max_target_row {
                        row + 37
                    } else {
                        fixture.max_target_row / 2
                    };
                    black_box(fixture.sync_at_visual_row(row));
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_visible_highlighting(c: &mut Criterion) {
    let mut group = c.benchmark_group("scrolling_visible_highlights");
    let first_row = LARGE_LINE_COUNT / 2;
    let unwrapped = FrameFixture::new(LARGE_LINE_COUNT, LONG_LINE_LEN, false, 1_200.0);
    let soft_wrap = FrameFixture::new(LARGE_LINE_COUNT, LONG_LINE_LEN, true, 480.0);
    let visible_lines = unwrapped.visible_unwrapped_lines(first_row);
    let visual_lines = soft_wrap.visible_soft_wrap_lines(first_row);

    group.bench_function("unwrapped_visible_range", |b| {
        b.iter(|| black_box(unwrapped.highlight_unwrapped(first_row, &visible_lines)));
    });
    group.bench_function("soft_wrap_visible_range", |b| {
        b.iter(|| black_box(soft_wrap.highlight_soft_wrap(first_row, &visual_lines)));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_frame_prep,
    bench_soft_wrap_viewport_sync,
    bench_visible_highlighting
);
criterion_main!(benches);
