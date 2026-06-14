// ABOUTME: Per-frame document/view facts for native editor rendering
// ABOUTME: Builds owned render state from Helix document and view inputs

use gpui::{Bounds, Font, Hsla, Pixels, TextRun, px};
use helix_view::{
    Document, Editor, Theme, View, ViewId,
    document::Mode,
    editor::CursorShapeConfig,
    graphics::{CursorKind, Style},
};

use crate::{
    DiagnosticOverlaySpans, DiagnosticSeverityByLine, DocumentRulerPaintParams,
    DocumentSoftWrapRenderPlanParams, EditorCursorPresentation, EditorCursorPresentationParams,
    EditorLayout, EditorLineHighlightContext, EditorRenderSnapshot, EditorSurfaceGeometry,
    GutterLinePlan, GutterLinePlanParams, RulerPaintPlan, SoftWrapHighlightedLineRunsParams,
    SoftWrapRenderPlan, UnwrappedHighlightedLine, UnwrappedHighlightedLineParams,
    UnwrappedRenderPlan, UnwrappedRenderPlanParams, build_gutter_line_plans,
    diagnostic_overlay_spans, diagnostic_severity_by_line, document_render_snapshot,
    document_ruler_paint_plans, document_soft_wrap_render_plan, editor_cursor_presentation,
    soft_wrap_highlighted_line_runs, unwrapped_highlighted_line, unwrapped_render_plan,
};

pub struct EditorDocumentFrameGutterParams<'a> {
    pub editor: &'a Editor,
    pub layout: &'a EditorLayout,
}

pub struct EditorDocumentFrameParams<'a> {
    pub document: &'a Document,
    pub view: &'a View,
    pub view_id: ViewId,
    pub theme: &'a Theme,
    pub syntax_loader: &'a helix_core::syntax::Loader,
    pub first_row: usize,
    pub last_row_from_scroll: usize,
    pub soft_wrap_enabled: bool,
    pub unwrapped_gutter: Option<EditorDocumentFrameGutterParams<'a>>,
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
    pub unwrapped_gutter_line_plans: Vec<GutterLinePlan>,
}

pub fn editor_document_frame(params: EditorDocumentFrameParams<'_>) -> EditorDocumentFrame {
    let text = params.document.text();
    let render_snapshot = document_render_snapshot(
        params.document,
        params.view_id,
        params.first_row,
        params.last_row_from_scroll,
    );
    let cursor_presentation = editor_cursor_presentation(EditorCursorPresentationParams {
        document: params.document,
        view_id: params.view_id,
        kind: params.cursor_kind,
        cursor_style: params.cursor_style,
        theme: params.theme,
        syntax_loader: params.syntax_loader,
        is_focused: params.is_focused,
    });
    let primary_cursor_idx = cursor_presentation.cursor_char_idx;
    let primary_cursor_line = text.char_to_line(primary_cursor_idx);
    let line_start = text.line_to_char(primary_cursor_line);
    let primary_cursor_col = primary_cursor_idx - line_start;
    let gutter_width = params.view.gutter_offset(params.document);
    let ruler_geometry = EditorSurfaceGeometry::new(params.bounds, gutter_width, params.cell_width);
    let ruler_paint_plans = document_ruler_paint_plans(DocumentRulerPaintParams {
        document: params.document,
        view_id: params.view_id,
        editor_rulers: &params.editor_rulers,
        geometry: ruler_geometry,
        color: params.ruler_color,
    });
    let unwrapped_gutter_line_plans = if !params.soft_wrap_enabled
        && let Some(gutter) = params.unwrapped_gutter
    {
        let mut gutter_origin = params.bounds.origin;
        gutter_origin.x += px(2.);
        gutter_origin.y += px(1.) - params.scroll_line_offset;

        build_gutter_line_plans(GutterLinePlanParams {
            layout: gutter.layout,
            origin: gutter_origin,
            first_row: params.first_row,
            last_row: render_snapshot.last_row,
            editor: gutter.editor,
            document: params.document,
            view: params.view,
            theme: params.theme,
            is_focused: params.is_focused,
        })
    } else {
        Vec::new()
    };
    let soft_wrap_render_plan = params.soft_wrap_enabled.then(|| {
        document_soft_wrap_render_plan(DocumentSoftWrapRenderPlanParams {
            document: params.document,
            theme: Some(params.theme),
            view_id: params.view_id,
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
        fg_color: params.fg_color,
        font: params.font.clone(),
        default_text_style: params.default_text_style,
        default_bg: params.default_bg,
        diagnostic_overlay_spans: diagnostic_overlay_spans.as_ref(),
    };

    let soft_wrap_line_runs = soft_wrap_render_plan
        .as_ref()
        .map(|plan| {
            plan.visual_lines
                .iter()
                .map(|visual| {
                    soft_wrap_highlighted_line_runs(SoftWrapHighlightedLineRunsParams {
                        context: highlight_context(),
                        visual,
                        wrap_indicator_color: params.wrap_indicator_color,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let unwrapped_highlighted_lines = unwrapped_render_plan
        .as_ref()
        .map(|plan| {
            plan.visible_lines
                .iter()
                .map(|line| {
                    unwrapped_highlighted_line(UnwrappedHighlightedLineParams {
                        context: highlight_context(),
                        text: text.slice(..),
                        line,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

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
        unwrapped_gutter_line_plans,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use gpui::{Bounds, black, font, point, px, size, white};
    use helix_core::{Rope, Selection, syntax};
    use helix_view::{
        Document, DocumentId, View,
        editor::{Config, GutterConfig},
        graphics::{CursorKind, Style},
        theme::Loader as ThemeLoader,
    };

    use crate::EDITOR_MINIMUM_VIEWPORT_COLUMNS;

    use super::*;

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
            soft_wrap_enabled: true,
            unwrapped_gutter: None,
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
        assert!(frame.unwrapped_gutter_line_plans.is_empty());
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
            soft_wrap_enabled: false,
            unwrapped_gutter: None,
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
        assert!(frame.unwrapped_gutter_line_plans.is_empty());
    }
}
