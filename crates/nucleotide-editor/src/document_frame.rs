// ABOUTME: Per-frame document/view facts for native editor rendering
// ABOUTME: Builds owned render state from Helix document and view inputs

use gpui::{Bounds, Pixels};
use helix_view::{
    Document, Theme, View, ViewId,
    document::Mode,
    editor::CursorShapeConfig,
    graphics::{CursorKind, Style},
};

use crate::{
    DiagnosticOverlaySpans, DiagnosticSeverityByLine, DocumentSoftWrapRenderPlanParams,
    EditorCursorPresentation, EditorCursorPresentationParams, EditorRenderSnapshot,
    SoftWrapRenderPlan, UnwrappedRenderPlan, UnwrappedRenderPlanParams, diagnostic_overlay_spans,
    diagnostic_severity_by_line, document_render_snapshot, document_soft_wrap_render_plan,
    editor_cursor_presentation, unwrapped_render_plan,
};

pub struct EditorDocumentFrameParams<'a> {
    pub document: &'a Document,
    pub view: &'a View,
    pub view_id: ViewId,
    pub theme: &'a Theme,
    pub syntax_loader: &'a helix_core::syntax::Loader,
    pub first_row: usize,
    pub last_row_from_scroll: usize,
    pub soft_wrap_enabled: bool,
    pub bounds: Bounds<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
    pub soft_wrap_minimum_columns: u16,
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
        diagnostic_overlay_spans: diagnostic_overlay_spans(params.document, params.theme),
        diagnostic_severity_by_line: diagnostic_severity_by_line(params.document),
        soft_wrap_render_plan,
        unwrapped_render_plan,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use gpui::{Bounds, point, px, size};
    use helix_core::{Rope, Selection, syntax};
    use helix_view::{
        Document, DocumentId, View,
        editor::{Config, GutterConfig},
        graphics::{CursorKind, Style},
        theme::Loader as ThemeLoader,
    };

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
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0))),
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(0.0),
            soft_wrap_minimum_columns: 10,
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
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(240.0), px(120.0))),
            cell_width: px(8.0),
            line_height: px(20.0),
            scroll_line_offset: px(5.0),
            soft_wrap_minimum_columns: 10,
            editor_mode: Mode::Normal,
            cursor_kind: CursorKind::Block,
            cursor_style: Style::default(),
            cursor_shape: CursorShapeConfig::default(),
            editor_rulers: vec![80],
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
    }
}
