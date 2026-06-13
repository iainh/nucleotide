// ABOUTME: Per-frame document/view facts for native editor rendering
// ABOUTME: Builds owned render state from Helix document and view inputs

use helix_view::{
    Document, Theme, View, ViewId,
    document::Mode,
    editor::CursorShapeConfig,
    graphics::{CursorKind, Style},
};

use crate::{
    DiagnosticOverlaySpans, EditorCursorPresentation, EditorCursorPresentationParams,
    EditorRenderSnapshot, diagnostic_overlay_spans, document_render_snapshot,
    editor_cursor_presentation,
};

pub struct EditorDocumentFrameParams<'a> {
    pub document: &'a Document,
    pub view: &'a View,
    pub view_id: ViewId,
    pub theme: &'a Theme,
    pub syntax_loader: &'a helix_core::syntax::Loader,
    pub first_row: usize,
    pub last_row_from_scroll: usize,
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

    EditorDocumentFrame {
        gutter_width: params.view.gutter_offset(params.document),
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
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;
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
    }
}
