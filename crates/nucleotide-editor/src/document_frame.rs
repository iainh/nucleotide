// ABOUTME: Per-frame document/view facts for native editor rendering
// ABOUTME: Builds owned render state from Helix document and view inputs

use std::time::Duration;

use gpui::{Bounds, Font, Hsla, Pixels, TextRun};
use helix_view::{
    Document, Theme, View, ViewId,
    document::Mode,
    editor::CursorShapeConfig,
    graphics::{CursorKind, Style},
    view::ViewPosition,
};

use crate::{
    DiagnosticOverlaySpans, DiagnosticSeverityByLine, DocumentRulerPaintParams,
    DocumentSoftWrapRenderPlanParams, EditorCursorPresentation, EditorCursorPresentationParams,
    EditorLineHighlightContext, EditorRenderSnapshot, EditorSurfaceGeometry, GutterLinePlan,
    RulerPaintPlan, SoftWrapHighlightedLineRunsBatchParams, SoftWrapRenderPlan,
    UnwrappedHighlightedLine, UnwrappedHighlightedLinesParams, UnwrappedRenderPlan,
    UnwrappedRenderPlanParams, diagnostic_overlay_spans, diagnostic_severity_by_line,
    document_render_snapshot, document_ruler_paint_plans, document_soft_wrap_render_plan,
    document_text_format_for_surface, editor_cursor_presentation,
    soft_wrap_highlighted_line_runs_batch, unwrapped_highlighted_lines, unwrapped_render_plan,
};
use nucleotide_logging::PerfTimer;

pub struct EditorDocumentFrameParams<'a> {
    pub document: &'a Document,
    pub view: &'a View,
    pub view_id: ViewId,
    pub theme: &'a Theme,
    pub syntax_loader: &'a helix_core::syntax::Loader,
    pub first_row: usize,
    pub last_row_from_scroll: usize,
    pub view_position: ViewPosition,
    pub soft_wrap_enabled: bool,
    pub gutter_line_plans: Vec<GutterLinePlan>,
    pub bounds: Bounds<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
    pub soft_wrap_minimum_columns: u16,
    pub fg_color: Hsla,
    pub font: Font,
    pub default_text_style: Style,
    pub default_bg: Hsla,
    pub wrap_indicator_color: Option<Hsla>,
    pub ruler_color: Hsla,
    pub editor_mode: Mode,
    pub cursor_kind: CursorKind,
    pub cursor_style: Style,
    pub cursor_shape: CursorShapeConfig,
    pub editor_rulers: Vec<u16>,
    pub cursorline_enabled: bool,
    pub is_focused: bool,
}

#[derive(Debug, Clone)]
pub struct EditorDocumentFrame {
    pub gutter_width: u16,
    pub primary_cursor_idx: usize,
    pub primary_cursor_line: usize,
    pub primary_cursor_col: usize,
    pub total_lines: usize,
    pub editor_mode: Mode,
    pub cursor_shape: CursorShapeConfig,
    pub editor_rulers: Vec<u16>,
    pub cursorline_enabled: bool,
    pub render_snapshot: EditorRenderSnapshot,
    pub cursor_presentation: EditorCursorPresentation,
    pub diagnostic_overlay_spans: Option<DiagnosticOverlaySpans>,
    pub diagnostic_severity_by_line: DiagnosticSeverityByLine,
    pub soft_wrap_render_plan: Option<SoftWrapRenderPlan>,
    pub unwrapped_render_plan: Option<UnwrappedRenderPlan>,
    pub soft_wrap_line_runs: Vec<Vec<TextRun>>,
    pub unwrapped_highlighted_lines: Vec<UnwrappedHighlightedLine>,
    pub ruler_paint_plans: Vec<RulerPaintPlan>,
    pub gutter_line_plans: Vec<GutterLinePlan>,
}

pub fn editor_document_frame(params: EditorDocumentFrameParams<'_>) -> EditorDocumentFrame {
    let _timer =
        PerfTimer::new("editor_document_frame").with_warn_threshold(Duration::from_millis(12));
    let text = params.document.text();
    let render_snapshot = document_render_snapshot(
        params.document,
        params.view_id,
        params.first_row,
        params.last_row_from_scroll,
    );
    let gutter_width = params.view.gutter_offset(params.document);
    let (_, text_format) = document_text_format_for_surface(
        params.document,
        Some(params.theme),
        params.bounds,
        gutter_width,
        params.cell_width,
        params.soft_wrap_minimum_columns,
    );
    let cursor_presentation = editor_cursor_presentation(EditorCursorPresentationParams {
        document: params.document,
        view_id: params.view_id,
        view_position: params.view_position,
        kind: params.cursor_kind,
        cursor_style: params.cursor_style,
        theme: params.theme,
        syntax_loader: params.syntax_loader,
        is_focused: params.is_focused,
        tab_width: text_format.tab_width,
    });
    let primary_cursor_idx = cursor_presentation.cursor_char_idx;
    let primary_cursor_line = text.char_to_line(primary_cursor_idx);
    let line_start = text.line_to_char(primary_cursor_line);
    let primary_cursor_col = primary_cursor_idx - line_start;
    let ruler_geometry = EditorSurfaceGeometry::new(params.bounds, gutter_width, params.cell_width);
    let ruler_paint_plans = document_ruler_paint_plans(DocumentRulerPaintParams {
        document: params.document,
        horizontal_offset: params.view_position.horizontal_offset,
        editor_rulers: &params.editor_rulers,
        geometry: ruler_geometry,
        color: params.ruler_color,
    });
    let soft_wrap_render_plan = params.soft_wrap_enabled.then(|| {
        document_soft_wrap_render_plan(DocumentSoftWrapRenderPlanParams {
            document: params.document,
            theme: Some(params.theme),
            view_position: params.view_position,
            bounds: params.bounds,
            gutter_columns: gutter_width,
            cell_width: params.cell_width,
            line_height: params.line_height,
            scroll_line_offset: params.scroll_line_offset,
            minimum_columns: params.soft_wrap_minimum_columns,
        })
    });
    let unwrapped_render_plan = (!params.soft_wrap_enabled).then(|| {
        unwrapped_render_plan(UnwrappedRenderPlanParams {
            text: text.slice(..),
            line_viewport: render_snapshot.line_viewport,
            bounds: params.bounds,
            gutter_columns: gutter_width,
            cell_width: params.cell_width,
            line_height: params.line_height,
            scroll_line_offset: params.scroll_line_offset,
            horizontal_offset: params.view_position.horizontal_offset,
            cursor_line: render_snapshot.cursor_line,
        })
    });
    let diagnostic_overlay_spans = diagnostic_overlay_spans(params.document, params.theme);

    let highlight_context = || EditorLineHighlightContext {
        doc: params.document,
        view: params.view,
        theme: params.theme,
        syntax_loader: params.syntax_loader,
        editor_mode: params.editor_mode,
        cursor_shape: &params.cursor_shape,
        is_view_focused: params.is_focused,
        view_position: params.view_position,
        fg_color: params.fg_color,
        font: params.font.clone(),
        default_text_style: params.default_text_style,
        default_bg: params.default_bg,
        diagnostic_overlay_spans: diagnostic_overlay_spans.as_ref(),
        tab_width: text_format.tab_width,
    };

    let soft_wrap_line_runs = {
        let _timer = PerfTimer::new("editor_document_frame.soft_wrap_highlights")
            .with_warn_threshold(Duration::from_millis(6));
        soft_wrap_render_plan
            .as_ref()
            .map(|plan| {
                soft_wrap_highlighted_line_runs_batch(SoftWrapHighlightedLineRunsBatchParams {
                    context: highlight_context(),
                    visual_lines: &plan.visual_lines,
                    wrap_indicator_color: params.wrap_indicator_color,
                })
            })
            .unwrap_or_default()
    };

    let unwrapped_highlighted_lines = {
        let _timer = PerfTimer::new("editor_document_frame.unwrapped_highlights")
            .with_warn_threshold(Duration::from_millis(6));
        unwrapped_render_plan
            .as_ref()
            .map(|plan| {
                unwrapped_highlighted_lines(UnwrappedHighlightedLinesParams {
                    context: highlight_context(),
                    text: text.slice(..),
                    lines: &plan.visible_lines,
                })
            })
            .unwrap_or_default()
    };

    EditorDocumentFrame {
        gutter_width,
        primary_cursor_idx,
        primary_cursor_line,
        primary_cursor_col,
        total_lines: text.len_lines(),
        editor_mode: params.editor_mode,
        cursor_shape: params.cursor_shape,
        editor_rulers: params.editor_rulers,
        cursorline_enabled: params.cursorline_enabled,
        render_snapshot,
        cursor_presentation,
        diagnostic_overlay_spans,
        diagnostic_severity_by_line: diagnostic_severity_by_line(params.document),
        soft_wrap_render_plan,
        unwrapped_render_plan,
        soft_wrap_line_runs,
        unwrapped_highlighted_lines,
        ruler_paint_plans,
        gutter_line_plans: params.gutter_line_plans,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::{ArcSwap, access::Map};
    use gpui::{Bounds, black, font, point, px, size, white};
    use helix_core::{Rope, Selection, Transaction, syntax};
    use helix_view::{
        Document, DocumentId, Editor, View,
        editor::{Action, Config, GutterConfig},
        graphics::{CursorKind, Rect, Style},
        handlers::Handlers,
        theme::Loader as ThemeLoader,
    };

    use crate::{
        EDITOR_MINIMUM_VIEWPORT_COLUMNS, EditorLayout, UnwrappedGutterLinePlanParams,
        build_unwrapped_gutter_line_plans, visible_ruler_paint_plans,
    };

    use super::*;

    fn test_handlers() -> Handlers {
        let (completion_tx, _) = tokio::sync::mpsc::channel(1);
        let (signature_tx, _) = tokio::sync::mpsc::channel(1);
        let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
        let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);

        Handlers {
            completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
            signature_hints: signature_tx,
            auto_save: auto_save_tx,
            document_colors: doc_colors_tx,
            word_index: helix_view::handlers::word_index::Handler::spawn(),
        }
    }

    fn test_editor_with_config(config: Config) -> (Editor, DocumentId, ViewId) {
        let config = Arc::new(ArcSwap::new(Arc::new(config)));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(ThemeLoader::new(&[]));
        let mut editor = Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(Arc::clone(&config), |config: &Config| config)),
            test_handlers(),
        );
        let doc_id = editor.new_file(Action::VerticalSplit);
        let view_id = editor.tree.focus;
        let doc = editor.document_mut(doc_id).unwrap();
        let transaction =
            Transaction::change(doc.text(), [(0, 0, Some("one\ntwo\n".into()))].into_iter());
        doc.apply(&transaction, view_id);

        (editor, doc_id, view_id)
    }

    #[test]
    fn frame_collects_document_view_render_facts() {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = syntax::Loader::default();
        let syntax_loader_swap = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let mut document =
            Document::from(Rope::from("one\ntwo\n"), None, config, syntax_loader_swap);
        let view = View::new(DocumentId::default(), GutterConfig::default());
        document.ensure_view_init(view.id);
        document.set_selection(view.id, Selection::single(5, 5));
        let theme = ThemeLoader::new(&[]).default_theme(true);

        let frame = editor_document_frame(EditorDocumentFrameParams {
            document: &document,
            view: &view,
            view_id: view.id,
            theme: &theme,
            syntax_loader: &syntax_loader,
            first_row: 0,
            last_row_from_scroll: 2,
            view_position: document.view_offset(view.id),
            soft_wrap_enabled: true,
            gutter_line_plans: Vec::new(),
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0))),
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(0.0),
            soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            fg_color: black(),
            font: font("TestFont"),
            default_text_style: Style::default(),
            default_bg: white(),
            wrap_indicator_color: None,
            ruler_color: black(),
            editor_mode: Mode::Normal,
            cursor_kind: CursorKind::Block,
            cursor_style: Style::default(),
            cursor_shape: CursorShapeConfig::default(),
            editor_rulers: vec![80],
            cursorline_enabled: true,
            is_focused: true,
        });

        assert_eq!(frame.primary_cursor_idx, 5);
        assert_eq!(frame.primary_cursor_line, 1);
        assert_eq!(frame.primary_cursor_col, 1);
        assert_eq!(frame.total_lines, 3);
        assert_eq!(frame.render_snapshot.cursor_char_idx, 5);
        assert_eq!(frame.cursor_presentation.cursor_char_idx, 5);
        assert_eq!(frame.editor_rulers, vec![80]);
        assert!(frame.cursorline_enabled);
        assert!(frame.soft_wrap_render_plan.is_some());
        assert!(frame.unwrapped_render_plan.is_none());
        assert_eq!(
            frame.soft_wrap_line_runs.len(),
            frame
                .soft_wrap_render_plan
                .as_ref()
                .unwrap()
                .visual_lines
                .len()
        );
        assert!(frame.unwrapped_highlighted_lines.is_empty());
        assert!(frame.ruler_paint_plans.is_empty());
        assert!(frame.gutter_line_plans.is_empty());
    }

    #[test]
    fn soft_wrap_frame_uses_supplied_view_position() {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = syntax::Loader::default();
        let syntax_loader_swap = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let mut document = Document::from(
            Rope::from("one\ntwo\nthree\n"),
            None,
            config,
            syntax_loader_swap,
        );
        let view = View::new(DocumentId::default(), GutterConfig::default());
        document.ensure_view_init(view.id);
        let theme = ThemeLoader::new(&[]).default_theme(true);
        let view_position = ViewPosition {
            anchor: 0,
            vertical_offset: 1,
            horizontal_offset: 0,
        };

        let frame = editor_document_frame(EditorDocumentFrameParams {
            document: &document,
            view: &view,
            view_id: view.id,
            theme: &theme,
            syntax_loader: &syntax_loader,
            first_row: 0,
            last_row_from_scroll: 2,
            view_position,
            soft_wrap_enabled: true,
            gutter_line_plans: Vec::new(),
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0))),
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(0.0),
            soft_wrap_minimum_columns: 10,
            fg_color: black(),
            font: font("TestFont"),
            default_text_style: Style::default(),
            default_bg: white(),
            wrap_indicator_color: None,
            ruler_color: black(),
            editor_mode: Mode::Normal,
            cursor_kind: CursorKind::Block,
            cursor_style: Style::default(),
            cursor_shape: CursorShapeConfig::default(),
            editor_rulers: Vec::new(),
            cursorline_enabled: true,
            is_focused: true,
        });

        let soft_wrap = frame
            .soft_wrap_render_plan
            .as_ref()
            .expect("soft-wrap plan");

        assert_eq!(soft_wrap.view_offset, view_position);
        assert_eq!(soft_wrap.visual_lines[0].visual_line, 1);
    }

    #[test]
    fn frame_rulers_use_supplied_view_position_horizontal_offset() {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = syntax::Loader::default();
        let syntax_loader_swap = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let mut document = Document::from(
            Rope::from("one\ntwo\nthree\n"),
            None,
            config,
            syntax_loader_swap,
        );
        let view = View::new(DocumentId::default(), GutterConfig::default());
        document.ensure_view_init(view.id);
        let theme = ThemeLoader::new(&[]).default_theme(true);
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0)));
        let view_position = ViewPosition {
            anchor: 0,
            vertical_offset: 0,
            horizontal_offset: 3,
        };

        let frame = editor_document_frame(EditorDocumentFrameParams {
            document: &document,
            view: &view,
            view_id: view.id,
            theme: &theme,
            syntax_loader: &syntax_loader,
            first_row: 0,
            last_row_from_scroll: 3,
            view_position,
            soft_wrap_enabled: false,
            gutter_line_plans: Vec::new(),
            bounds,
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(0.0),
            soft_wrap_minimum_columns: 10,
            fg_color: black(),
            font: font("TestFont"),
            default_text_style: Style::default(),
            default_bg: white(),
            wrap_indicator_color: None,
            ruler_color: black(),
            editor_mode: Mode::Normal,
            cursor_kind: CursorKind::Block,
            cursor_style: Style::default(),
            cursor_shape: CursorShapeConfig::default(),
            editor_rulers: vec![1, 4, 10],
            cursorline_enabled: true,
            is_focused: true,
        });

        let geometry = EditorSurfaceGeometry::new(bounds, frame.gutter_width, px(8.0));
        assert_eq!(
            frame.ruler_paint_plans,
            visible_ruler_paint_plans(geometry, &[1, 4, 10], 3, black())
        );
    }

    #[test]
    fn frame_collects_unwrapped_render_plan_when_soft_wrap_disabled() {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = syntax::Loader::default();
        let syntax_loader_swap = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let mut document = Document::from(
            Rope::from("one\ntwo\nthree\n"),
            None,
            config,
            syntax_loader_swap,
        );
        let view = View::new(DocumentId::default(), GutterConfig::default());
        document.ensure_view_init(view.id);
        document.set_selection(view.id, Selection::single(5, 5));
        let theme = ThemeLoader::new(&[]).default_theme(true);

        let frame = editor_document_frame(EditorDocumentFrameParams {
            document: &document,
            view: &view,
            view_id: view.id,
            theme: &theme,
            syntax_loader: &syntax_loader,
            first_row: 0,
            last_row_from_scroll: 3,
            view_position: document.view_offset(view.id),
            soft_wrap_enabled: false,
            gutter_line_plans: Vec::new(),
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(120.0))),
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(5.0),
            soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            fg_color: black(),
            font: font("TestFont"),
            default_text_style: Style::default(),
            default_bg: white(),
            wrap_indicator_color: None,
            ruler_color: black(),
            editor_mode: Mode::Normal,
            cursor_kind: CursorKind::Block,
            cursor_style: Style::default(),
            cursor_shape: CursorShapeConfig::default(),
            editor_rulers: vec![1, 4, 80],
            cursorline_enabled: true,
            is_focused: true,
        });

        let unwrapped = frame
            .unwrapped_render_plan
            .as_ref()
            .expect("unwrapped plan should be built when soft wrap is disabled");

        assert!(frame.soft_wrap_render_plan.is_none());
        assert_eq!(unwrapped.visible_lines.len(), 3);
        assert_eq!(unwrapped.visible_lines[0].line_idx, 0);
        assert_eq!(unwrapped.visible_lines[0].y_offset, px(-5.0));
        assert_eq!(unwrapped.cursor_line, frame.render_snapshot.cursor_line);
        assert_eq!(
            frame.unwrapped_highlighted_lines.len(),
            unwrapped.visible_lines.len()
        );
        assert!(frame.soft_wrap_line_runs.is_empty());
        assert!(!frame.ruler_paint_plans.is_empty());
        assert!(frame.gutter_line_plans.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn frame_uses_explicit_editor_render_state() {
        let mut config = Config::default();
        config.cursorline = true;
        config.rulers = vec![2, 4];
        let (editor, doc_id, view_id) = test_editor_with_config(config);
        let document = editor.document(doc_id).unwrap();
        let view = editor.tree.try_get(view_id).unwrap();
        let syntax_loader = editor.syn_loader.load();
        let editor_config = editor.config();
        let editor_mode = editor.mode();
        let (_, cursor_kind) = editor.cursor();
        let theme = ThemeLoader::new(&[]).default_theme(true);
        let layout = EditorLayout {
            rows: 6,
            columns: 30,
            line_height: px(20.0),
            font_size: px(16.0),
            cell_width: px(8.0),
        };
        let render_snapshot = document_render_snapshot(document, view_id, 0, 2);
        let gutter_line_plans = build_unwrapped_gutter_line_plans(UnwrappedGutterLinePlanParams {
            editor: &editor,
            document,
            view,
            theme: &theme,
            layout: &layout,
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0))),
            scroll_line_offset: px(0.0),
            horizontal_offset: 0,
            first_row: 0,
            last_row: render_snapshot.last_row,
            is_focused: true,
        });

        let frame = editor_document_frame(EditorDocumentFrameParams {
            document,
            view,
            view_id,
            theme: &theme,
            syntax_loader: &syntax_loader,
            first_row: 0,
            last_row_from_scroll: 2,
            view_position: document.view_offset(view_id),
            soft_wrap_enabled: false,
            gutter_line_plans,
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0))),
            cell_width: layout.cell_width,
            line_height: layout.line_height,
            scroll_line_offset: px(0.0),
            soft_wrap_minimum_columns: EDITOR_MINIMUM_VIEWPORT_COLUMNS,
            fg_color: black(),
            font: font("TestFont"),
            default_text_style: Style::default(),
            default_bg: white(),
            wrap_indicator_color: None,
            ruler_color: black(),
            editor_mode,
            cursor_kind,
            cursor_style: Style::default(),
            cursor_shape: editor_config.cursor_shape.clone(),
            editor_rulers: editor_config.rulers.clone(),
            cursorline_enabled: editor_config.cursorline,
            is_focused: true,
        });

        assert_eq!(frame.editor_mode, Mode::Normal);
        assert_eq!(frame.editor_rulers, vec![2, 4]);
        assert!(frame.cursorline_enabled);
        assert_eq!(frame.cursor_presentation.kind, CursorKind::Block);
        assert!(!frame.gutter_line_plans.is_empty());
    }
}
