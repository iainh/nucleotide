// ABOUTME: Native GPUI cursor painter for editor surfaces
// ABOUTME: Draws Helix cursor shapes and optional block-cursor text overlays

use std::borrow::Cow;

use gpui::{
    App, BorderStyle, Bounds, Font, Hsla, Pixels, Point, ShapedLine, SharedString, Size, TextAlign,
    TextRun, Window, WindowTextSystem, fill, point, px, quad, size, transparent_black, white,
};
use helix_core::{
    RopeSlice,
    doc_formatter::TextFormat,
    graphemes::{next_grapheme_boundary, tab_width_at},
    text_annotations::TextAnnotations,
};
use helix_view::{
    Document, Theme, ViewId,
    graphics::{CursorKind, Style},
    view::ViewPosition,
};
use nucleotide_logging::error;

use crate::{
    cursor_has_reversed_modifier,
    geometry::EditorSurfaceGeometry,
    highlight::text_style_at_position,
    line_cache::LineLayout,
    line_text::{
        byte_offset_for_char_offset, line_text_without_trailing_newline, visual_columns_for_text,
    },
    soft_wrap::{SoftWrapVisualPosition, soft_wrap_visual_position},
    style::{create_styled_text_run, helix_color_to_hsla},
};

pub struct EditorCursor {
    pub origin: Point<Pixels>,
    pub kind: CursorKind,
    pub color: Hsla,
    pub block_width: Pixels,
    pub line_height: Pixels,
    pub text: Option<ShapedLine>,
    pub hollow: bool,
}

impl EditorCursor {
    pub fn from_paint_position(
        paint_position: CursorPaintPosition,
        kind: CursorKind,
        color: Hsla,
        block_width: Pixels,
        line_height: Pixels,
        text: Option<ShapedLine>,
    ) -> Self {
        Self {
            origin: paint_position.cursor_origin,
            kind,
            color,
            block_width,
            line_height,
            text,
            hollow: false,
        }
    }

    pub fn bounds(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        match self.kind {
            CursorKind::Bar => Bounds {
                origin: self.origin + origin,
                size: size(px(2.0), self.line_height),
            },
            CursorKind::Block => Bounds {
                origin: self.origin + origin,
                size: size(self.block_width, self.line_height),
            },
            CursorKind::Underline => Bounds {
                origin: self.origin + origin + Point::new(Pixels::ZERO, self.line_height - px(2.0)),
                size: size(self.block_width, px(2.0)),
            },
            CursorKind::Hidden => Bounds {
                origin: self.origin + origin,
                size: size(px(0.0), px(0.0)),
            },
        }
    }

    pub fn paint(&mut self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = self.bounds(origin);
        if self.hollow && matches!(self.kind, CursorKind::Block) {
            window.paint_quad(quad(
                bounds,
                px(0.0),
                transparent_black(),
                px(1.0),
                self.color,
                BorderStyle::default(),
            ));
        } else {
            window.paint_quad(fill(bounds, self.color));
        }

        if let Some(text) = &self.text
            && let Err(error) = text.paint(
                bounds.origin,
                self.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            )
        {
            error!(error = ?error, "Failed to paint cursor text");
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorLinePosition {
    pub line: usize,
    pub line_start: usize,
    pub line_end: usize,
    pub line_text: String,
    pub cursor_char_offset: usize,
    pub cursor_byte_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorViewportPosition {
    pub line: usize,
    pub viewport_row: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorPaintPosition {
    pub paint_origin: Point<Pixels>,
    pub cursor_origin: Point<Pixels>,
}

impl CursorPaintPosition {
    pub fn cursor_point(&self) -> Point<Pixels> {
        self.paint_origin + self.cursor_origin
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorOverlayPlan {
    pub cursor_position: Point<Pixels>,
    pub cursor_size: Size<Pixels>,
}

pub fn cursor_overlay_plan(
    paint_position: CursorPaintPosition,
    cursor_width: Pixels,
    line_height: Pixels,
) -> CursorOverlayPlan {
    CursorOverlayPlan {
        cursor_position: paint_position.cursor_point(),
        cursor_size: size(cursor_width, line_height),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedEditorCursorPlan {
    pub color: Hsla,
    pub width: Pixels,
    pub overlay: CursorOverlayPlan,
}

pub struct ShapedEditorCursorPlanParams<'a> {
    pub paint_position: CursorPaintPosition,
    pub cursor_style: &'a Style,
    pub text_style_at_cursor: &'a Style,
    pub cursor_text_shape: &'a CursorTextShape,
    pub fallback_fg: Hsla,
    pub fallback_width: Pixels,
    pub line_height: Pixels,
}

pub struct ShapedEditorCursorPaintParams<'a> {
    pub paint_position: CursorPaintPosition,
    pub kind: CursorKind,
    pub cursor_style: &'a Style,
    pub text_style_at_cursor: &'a Style,
    pub cursor_text_shape: CursorTextShape,
    pub is_focused: bool,
    pub fallback_fg: Hsla,
    pub fallback_width: Pixels,
    pub line_height: Pixels,
}

pub struct EditorCursorTextPaintParams<'a> {
    pub paint_position: CursorPaintPosition,
    pub kind: CursorKind,
    pub cursor_style: &'a Style,
    pub text_style_at_cursor: &'a Style,
    pub cursor_text: Option<SharedString>,
    pub font: &'a Font,
    pub font_size: Pixels,
    pub fallback_fg: Hsla,
    pub default_bg: Hsla,
    pub is_focused: bool,
    pub fallback_width: Pixels,
    pub line_height: Pixels,
}

#[derive(Debug, Clone)]
pub struct EditorCursorPresentation {
    pub cursor_char_idx: usize,
    pub kind: CursorKind,
    pub cursor_style: Style,
    pub text_style_at_cursor: Style,
    pub block_text: Option<SharedString>,
}

pub struct EditorCursorPresentationParams<'a> {
    pub document: &'a Document,
    pub view_id: ViewId,
    pub view_position: ViewPosition,
    pub kind: CursorKind,
    pub cursor_style: Style,
    pub theme: &'a Theme,
    pub syntax_loader: &'a helix_core::syntax::Loader,
    pub is_focused: bool,
    pub tab_width: u16,
}

pub fn editor_cursor_presentation(
    params: EditorCursorPresentationParams<'_>,
) -> EditorCursorPresentation {
    let text = params.document.text().slice(..);
    let cursor_char_idx = params
        .document
        .selection(params.view_id)
        .primary()
        .cursor(text);
    let text_style_at_cursor = text_style_at_position(
        params.document,
        params.view_position,
        params.theme,
        params.syntax_loader,
        cursor_char_idx,
    );
    let block_text = block_cursor_text(
        text,
        cursor_char_idx,
        params.kind,
        params.is_focused,
        params.tab_width,
    );

    EditorCursorPresentation {
        cursor_char_idx,
        kind: params.kind,
        cursor_style: params.cursor_style,
        text_style_at_cursor,
        block_text,
    }
}

impl EditorCursorPresentation {
    pub fn block_text_color(&self, default_bg: Hsla) -> Hsla {
        cursor_foreground_color(
            &self.cursor_style,
            cursor_has_reversed_modifier(&self.cursor_style),
            default_bg,
        )
    }
}

pub fn shaped_editor_cursor_plan(
    params: ShapedEditorCursorPlanParams<'_>,
) -> ShapedEditorCursorPlan {
    let color = cursor_background_color(
        params.cursor_style,
        params.text_style_at_cursor,
        params.fallback_fg,
    );
    let width = params.cursor_text_shape.width_or(params.fallback_width);

    ShapedEditorCursorPlan {
        color,
        width,
        overlay: cursor_overlay_plan(params.paint_position, width, params.line_height),
    }
}

fn cursor_kind_for_focus(kind: CursorKind, is_focused: bool) -> CursorKind {
    if is_focused || matches!(kind, CursorKind::Hidden) {
        kind
    } else {
        CursorKind::Block
    }
}

fn should_paint_hollow_cursor(kind: CursorKind, is_focused: bool) -> bool {
    matches!(cursor_kind_for_focus(kind, is_focused), CursorKind::Block) && !is_focused
}

pub fn paint_shaped_editor_cursor(
    window: &mut Window,
    cx: &mut App,
    params: ShapedEditorCursorPaintParams<'_>,
) -> CursorOverlayPlan {
    let plan = shaped_editor_cursor_plan(ShapedEditorCursorPlanParams {
        paint_position: params.paint_position,
        cursor_style: params.cursor_style,
        text_style_at_cursor: params.text_style_at_cursor,
        cursor_text_shape: &params.cursor_text_shape,
        fallback_fg: params.fallback_fg,
        fallback_width: params.fallback_width,
        line_height: params.line_height,
    });

    let paint_kind = cursor_kind_for_focus(params.kind, params.is_focused);
    let hollow = should_paint_hollow_cursor(params.kind, params.is_focused);
    let cursor_text = if hollow {
        None
    } else {
        params.cursor_text_shape.into_shaped_line()
    };
    let mut cursor = EditorCursor::from_paint_position(
        params.paint_position,
        paint_kind,
        plan.color,
        plan.width,
        params.line_height,
        cursor_text,
    );
    cursor.hollow = hollow;
    cursor.paint(params.paint_position.paint_origin, window, cx);

    plan.overlay
}

pub fn shape_and_paint_editor_cursor(
    window: &mut Window,
    cx: &mut App,
    params: EditorCursorTextPaintParams<'_>,
) -> CursorOverlayPlan {
    let has_reversed = cursor_has_reversed_modifier(params.cursor_style);
    let cursor_text_color =
        cursor_foreground_color(params.cursor_style, has_reversed, params.default_bg);
    let text_system = window.text_system().clone();
    let cursor_text_shape = shape_cursor_text(
        text_system.as_ref(),
        params.cursor_text,
        params.font,
        params.font_size,
        params.text_style_at_cursor,
        cursor_text_color,
        params.default_bg,
    );

    paint_shaped_editor_cursor(
        window,
        cx,
        ShapedEditorCursorPaintParams {
            paint_position: params.paint_position,
            kind: params.kind,
            cursor_style: params.cursor_style,
            text_style_at_cursor: params.text_style_at_cursor,
            cursor_text_shape,
            is_focused: params.is_focused,
            fallback_fg: params.fallback_fg,
            fallback_width: params.fallback_width,
            line_height: params.line_height,
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnwrappedCursorPaintPlanSource {
    LineLayout,
    PhantomTrailingNewline,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnwrappedCursorPaintPlan {
    pub source: UnwrappedCursorPaintPlanSource,
    pub paint_position: CursorPaintPosition,
    pub line_position: Option<CursorLinePosition>,
}

pub struct UnwrappedCursorPaintPlanParams<'a> {
    pub text: RopeSlice<'a>,
    pub geometry: EditorSurfaceGeometry,
    pub cursor_char_idx: usize,
    pub cursor_at_trailing_newline: bool,
    pub cursor_viewport_position: Option<CursorViewportPosition>,
    pub line_layout: Option<&'a LineLayout>,
    pub line_height: Pixels,
    pub scroll_line_offset: Pixels,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoftWrapCursorPaintPlan {
    pub visual_position: SoftWrapVisualPosition,
    pub paint_position: CursorPaintPosition,
}

pub struct SoftWrapCursorPaintPlanParams<'a, 'b> {
    pub text: RopeSlice<'a>,
    pub text_format: &'b TextFormat,
    pub text_annotations: Option<&'b TextAnnotations<'a>>,
    pub precomputed_visual_position: Option<SoftWrapVisualPosition>,
    pub anchor: usize,
    pub cursor_char_idx: usize,
    pub geometry: EditorSurfaceGeometry,
    pub line_height: Pixels,
    pub cell_width: Pixels,
    pub scroll_line_offset: Pixels,
    pub vertical_offset: usize,
    pub viewport_height: usize,
    pub horizontal_offset: usize,
}

pub struct SoftWrapCursorPaintPositionParams {
    pub geometry: EditorSurfaceGeometry,
    pub line_height: Pixels,
    pub cell_width: Pixels,
    pub cursor_position: SoftWrapVisualPosition,
    pub scroll_line_offset: Pixels,
    pub vertical_offset: usize,
    pub viewport_height: usize,
    pub horizontal_offset: usize,
}

#[derive(Clone)]
pub struct CursorTextShape {
    pub shaped_line: Option<ShapedLine>,
    pub len: usize,
}

impl CursorTextShape {
    pub fn width_or(&self, fallback: Pixels) -> Pixels {
        self.shaped_line
            .as_ref()
            .map_or(fallback, |shaped| shaped.x_for_index(self.len))
    }

    pub fn into_shaped_line(self) -> Option<ShapedLine> {
        self.shaped_line
    }
}

pub fn cursor_text_run(
    base_font: &Font,
    text_len: usize,
    text_style_at_cursor: &Style,
    text_color: Hsla,
    default_bg: Hsla,
) -> TextRun {
    let underline_color = text_style_at_cursor
        .underline_color
        .and_then(helix_color_to_hsla);

    create_styled_text_run(
        text_len,
        base_font,
        text_style_at_cursor,
        text_color,
        None,
        default_bg,
        underline_color,
    )
}

pub fn block_cursor_text(
    text: RopeSlice<'_>,
    cursor_char_idx: usize,
    cursor_kind: CursorKind,
    is_focused: bool,
    tab_width: u16,
) -> Option<SharedString> {
    if !matches!(cursor_kind, CursorKind::Block)
        || !is_focused
        || cursor_char_idx >= text.len_chars()
    {
        return None;
    }

    let grapheme_end = next_grapheme_boundary(text, cursor_char_idx);
    let char_slice = text.slice(cursor_char_idx..grapheme_end);
    let char_text: Cow<'_, str> = char_slice.into();
    let char_text = match char_text.as_ref() {
        "\n" | "\r\n" | "\r" => " ".into(),
        "\t" => {
            let cursor_line = text.char_to_line(cursor_char_idx);
            let line_start = text.line_to_char(cursor_line);
            let line_prefix: Cow<'_, str> = text.slice(line_start..cursor_char_idx).into();
            let visual_col = visual_columns_for_text(line_prefix.as_ref(), 0, tab_width);
            SharedString::from(" ".repeat(tab_width_at(visual_col, tab_width)))
        }
        _ => SharedString::from(char_text.into_owned()),
    };

    (!char_text.is_empty()).then_some(char_text)
}

pub fn cursor_line_position(
    text: RopeSlice<'_>,
    cursor_line: usize,
    cursor_char_idx: usize,
    cursor_at_trailing_newline: bool,
) -> CursorLinePosition {
    let line = cursor_line.min(text.len_lines().saturating_sub(1));
    let line_start = text.line_to_char(line);
    let line_end = if line + 1 < text.len_lines() {
        text.line_to_char(line + 1)
    } else {
        text.len_chars()
    };
    let line_text = line_text_without_trailing_newline(text.slice(line_start..line_end));
    let line_char_count = line_text.chars().count();
    let cursor_char_offset = if cursor_at_trailing_newline {
        line_char_count
    } else {
        cursor_char_idx.saturating_sub(line_start)
    }
    .min(line_char_count);
    let cursor_byte_offset = byte_offset_for_char_offset(&line_text, cursor_char_offset);

    CursorLinePosition {
        line,
        line_start,
        line_end,
        line_text,
        cursor_char_offset,
        cursor_byte_offset,
    }
}

pub fn cursor_document_line(
    text: RopeSlice<'_>,
    cursor_char_idx: usize,
    cursor_at_trailing_newline: bool,
) -> usize {
    if cursor_at_trailing_newline {
        return text.len_lines().saturating_sub(1);
    }

    text.char_to_line(cursor_char_idx.min(text.len_chars()))
}

pub fn cursor_document_line_for_view(document: &Document, view_id: ViewId) -> usize {
    let text = document.text();
    let cursor_char_idx = document.selection(view_id).primary().cursor(text.slice(..));
    let cursor_at_trailing_newline = cursor_char_idx == text.len_chars()
        && text.len_chars() > 0
        && text.char(text.len_chars() - 1) == '\n';

    cursor_document_line(text.slice(..), cursor_char_idx, cursor_at_trailing_newline)
}

pub fn cursor_viewport_position(
    cursor_line: usize,
    first_row: usize,
    last_row: usize,
) -> Option<CursorViewportPosition> {
    (cursor_line >= first_row && cursor_line < last_row).then_some(CursorViewportPosition {
        line: cursor_line,
        viewport_row: cursor_line.saturating_sub(first_row),
    })
}

pub fn unwrapped_cursor_paint_position(
    geometry: EditorSurfaceGeometry,
    line_layout: &LineLayout,
    cursor_byte_offset: usize,
) -> CursorPaintPosition {
    let text_bounds = geometry.text_bounds();
    let cursor_display_byte_offset = line_layout.display_byte_for_source_byte(cursor_byte_offset);
    let cursor_x = line_layout
        .shaped_line
        .x_for_index(cursor_display_byte_offset);

    CursorPaintPosition {
        paint_origin: text_bounds.origin + line_layout.origin,
        cursor_origin: point(cursor_x, px(0.0)),
    }
}

pub fn phantom_line_cursor_paint_position(
    geometry: EditorSurfaceGeometry,
    y_offset: Pixels,
) -> CursorPaintPosition {
    let text_bounds = geometry.text_bounds();

    CursorPaintPosition {
        paint_origin: point(text_bounds.origin.x, text_bounds.origin.y + y_offset),
        cursor_origin: point(px(0.0), px(0.0)),
    }
}

pub fn unwrapped_cursor_paint_plan(
    params: UnwrappedCursorPaintPlanParams<'_>,
) -> Option<UnwrappedCursorPaintPlan> {
    let cursor_viewport_position = params.cursor_viewport_position?;
    let cursor_line = cursor_viewport_position.line;

    if let Some(line_layout) = params.line_layout
        && line_layout.line_idx == cursor_line
    {
        let line_position = cursor_line_position(
            params.text,
            cursor_line,
            params.cursor_char_idx,
            params.cursor_at_trailing_newline,
        );
        let paint_position = unwrapped_cursor_paint_position(
            params.geometry,
            line_layout,
            line_position.cursor_byte_offset,
        );

        return Some(UnwrappedCursorPaintPlan {
            source: UnwrappedCursorPaintPlanSource::LineLayout,
            paint_position,
            line_position: Some(line_position),
        });
    }

    if params.cursor_at_trailing_newline && params.cursor_char_idx >= params.text.len_chars() {
        let y_offset = params.line_height * cursor_viewport_position.viewport_row as f32
            - params.scroll_line_offset;

        return Some(UnwrappedCursorPaintPlan {
            source: UnwrappedCursorPaintPlanSource::PhantomTrailingNewline,
            paint_position: phantom_line_cursor_paint_position(params.geometry, y_offset),
            line_position: None,
        });
    }

    None
}

pub fn soft_wrap_cursor_paint_position(
    params: SoftWrapCursorPaintPositionParams,
) -> Option<CursorPaintPosition> {
    if params.cursor_position.visual_line < params.vertical_offset
        || params.cursor_position.visual_line
            >= params
                .vertical_offset
                .saturating_add(params.viewport_height)
    {
        return None;
    }

    let text_bounds = params.geometry.text_bounds();
    let relative_line = params.cursor_position.visual_line - params.vertical_offset;
    let visual_col_in_viewport =
        params.cursor_position.visual_col as f32 - params.horizontal_offset as f32;

    Some(CursorPaintPosition {
        paint_origin: point(
            text_bounds.origin.x + params.cell_width * visual_col_in_viewport,
            text_bounds.origin.y - params.scroll_line_offset
                + params.line_height * relative_line as f32,
        ),
        cursor_origin: point(px(0.0), px(0.0)),
    })
}

pub fn soft_wrap_cursor_paint_plan(
    params: SoftWrapCursorPaintPlanParams<'_, '_>,
) -> Option<SoftWrapCursorPaintPlan> {
    let visual_position = params.precomputed_visual_position.or_else(|| {
        soft_wrap_visual_position(
            params.text,
            params.text_format,
            params.text_annotations,
            params.anchor,
            params.cursor_char_idx,
        )
    })?;
    let paint_position = soft_wrap_cursor_paint_position(SoftWrapCursorPaintPositionParams {
        geometry: params.geometry,
        line_height: params.line_height,
        cell_width: params.cell_width,
        cursor_position: visual_position.clone(),
        scroll_line_offset: params.scroll_line_offset,
        vertical_offset: params.vertical_offset,
        viewport_height: params.viewport_height,
        horizontal_offset: params.horizontal_offset,
    })?;

    Some(SoftWrapCursorPaintPlan {
        visual_position,
        paint_position,
    })
}

pub fn cursor_foreground_color(cursor_style: &Style, has_reversed: bool, default_bg: Hsla) -> Hsla {
    if has_reversed {
        default_bg
    } else if let Some(fg) = cursor_style.fg {
        helix_color_to_hsla(fg).unwrap_or_else(white)
    } else {
        white()
    }
}

pub fn cursor_background_color(
    cursor_style: &Style,
    text_style_at_cursor: &Style,
    fallback_fg: Hsla,
) -> Hsla {
    if cursor_has_reversed_modifier(cursor_style) {
        text_style_at_cursor
            .fg
            .and_then(helix_color_to_hsla)
            .unwrap_or(fallback_fg)
    } else {
        cursor_style
            .bg
            .and_then(helix_color_to_hsla)
            .or_else(|| cursor_style.fg.and_then(helix_color_to_hsla))
            .unwrap_or(fallback_fg)
    }
}

pub fn shape_cursor_text(
    text_system: &WindowTextSystem,
    text: Option<SharedString>,
    font: &Font,
    font_size: Pixels,
    text_style_at_cursor: &Style,
    text_color: Hsla,
    default_bg: Hsla,
) -> CursorTextShape {
    let Some(text) = text else {
        return CursorTextShape {
            shaped_line: None,
            len: 0,
        };
    };

    let len = text.len();
    let run = cursor_text_run(font, len, text_style_at_cursor, text_color, default_bg);
    let shaped_line = text_system.shape_line(text, font_size, &[run], None);

    CursorTextShape {
        shaped_line: Some(shaped_line),
        len,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::{ArcSwap, access::Map};
    use gpui::{black, hsla, point, px, size};
    use helix_core::{
        Selection, Transaction,
        doc_formatter::TextFormat,
        syntax,
        text_annotations::{InlineAnnotation, TextAnnotations},
    };
    use helix_view::{
        DocumentId, Editor,
        editor::{Action, Config},
        graphics::{Color, Modifier, Rect},
        handlers::Handlers,
        theme,
    };

    use super::*;

    fn test_cursor(kind: CursorKind) -> EditorCursor {
        EditorCursor {
            origin: point(px(3.0), px(5.0)),
            kind,
            color: black(),
            block_width: px(8.0),
            line_height: px(20.0),
            text: None,
            hollow: false,
        }
    }

    fn soft_wrap_text_format() -> TextFormat {
        TextFormat {
            soft_wrap: true,
            tab_width: 2,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: ".".into(),
            wrap_indicator_highlight: None,
            viewport_width: 17,
            soft_wrap_at_text_width: false,
        }
    }

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

    fn test_editor_with_text(text: &str) -> (Editor, DocumentId, ViewId) {
        let config = Arc::new(ArcSwap::new(Arc::new(Config::default())));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));
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
        let transaction = Transaction::change(doc.text(), [(0, 0, Some(text.into()))].into_iter());
        doc.apply(&transaction, view_id);

        (editor, doc_id, view_id)
    }

    #[test]
    fn block_cursor_uses_configured_cell_width() {
        assert_eq!(
            test_cursor(CursorKind::Block).bounds(point(px(10.0), px(20.0))),
            Bounds {
                origin: point(px(13.0), px(25.0)),
                size: size(px(8.0), px(20.0)),
            }
        );
    }

    #[test]
    fn bar_cursor_uses_fixed_two_pixel_width() {
        assert_eq!(
            test_cursor(CursorKind::Bar).bounds(point(px(10.0), px(20.0))),
            Bounds {
                origin: point(px(13.0), px(25.0)),
                size: size(px(2.0), px(20.0)),
            }
        );
    }

    #[test]
    fn underline_cursor_sits_at_line_bottom() {
        assert_eq!(
            test_cursor(CursorKind::Underline).bounds(point(px(10.0), px(20.0))),
            Bounds {
                origin: point(px(13.0), px(43.0)),
                size: size(px(8.0), px(2.0)),
            }
        );
    }

    #[test]
    fn cursor_from_paint_position_uses_relative_cursor_origin() {
        let paint_position = CursorPaintPosition {
            paint_origin: point(px(10.0), px(20.0)),
            cursor_origin: point(px(3.0), px(5.0)),
        };

        let cursor = EditorCursor::from_paint_position(
            paint_position,
            CursorKind::Block,
            black(),
            px(8.0),
            px(20.0),
            None,
        );

        assert_eq!(cursor.origin, point(px(3.0), px(5.0)));
        assert!(!cursor.hollow);
        assert_eq!(
            cursor.bounds(paint_position.paint_origin).origin,
            point(px(13.0), px(25.0))
        );
    }

    #[test]
    fn inactive_visible_cursors_paint_as_hollow_blocks() {
        assert_eq!(
            cursor_kind_for_focus(CursorKind::Block, false),
            CursorKind::Block
        );
        assert_eq!(
            cursor_kind_for_focus(CursorKind::Bar, false),
            CursorKind::Block
        );
        assert_eq!(
            cursor_kind_for_focus(CursorKind::Underline, false),
            CursorKind::Block
        );
        assert!(should_paint_hollow_cursor(CursorKind::Block, false));
        assert!(should_paint_hollow_cursor(CursorKind::Bar, false));
        assert!(should_paint_hollow_cursor(CursorKind::Underline, false));
    }

    #[test]
    fn focused_and_hidden_cursors_do_not_paint_hollow() {
        assert_eq!(
            cursor_kind_for_focus(CursorKind::Block, true),
            CursorKind::Block
        );
        assert_eq!(
            cursor_kind_for_focus(CursorKind::Bar, true),
            CursorKind::Bar
        );
        assert_eq!(
            cursor_kind_for_focus(CursorKind::Underline, true),
            CursorKind::Underline
        );
        assert_eq!(
            cursor_kind_for_focus(CursorKind::Hidden, false),
            CursorKind::Hidden
        );
        assert!(!should_paint_hollow_cursor(CursorKind::Block, true));
        assert!(!should_paint_hollow_cursor(CursorKind::Bar, true));
        assert!(!should_paint_hollow_cursor(CursorKind::Underline, true));
        assert!(!should_paint_hollow_cursor(CursorKind::Hidden, false));
    }

    #[test]
    fn cursor_overlay_plan_uses_absolute_cursor_point() {
        let paint_position = CursorPaintPosition {
            paint_origin: point(px(10.0), px(20.0)),
            cursor_origin: point(px(3.0), px(5.0)),
        };

        let overlay = cursor_overlay_plan(paint_position, px(8.0), px(20.0));

        assert_eq!(overlay.cursor_position, point(px(13.0), px(25.0)));
        assert_eq!(overlay.cursor_size, size(px(8.0), px(20.0)));
    }

    #[test]
    fn shaped_editor_cursor_plan_uses_fallback_width_and_overlay() {
        let paint_position = CursorPaintPosition {
            paint_origin: point(px(10.0), px(20.0)),
            cursor_origin: point(px(3.0), px(5.0)),
        };
        let cursor_text_shape = CursorTextShape {
            shaped_line: None,
            len: 0,
        };

        let plan = shaped_editor_cursor_plan(ShapedEditorCursorPlanParams {
            paint_position,
            cursor_style: &Style::default(),
            text_style_at_cursor: &Style::default(),
            cursor_text_shape: &cursor_text_shape,
            fallback_fg: black(),
            fallback_width: px(8.0),
            line_height: px(20.0),
        });

        assert_eq!(plan.color, black());
        assert_eq!(plan.width, px(8.0));
        assert_eq!(plan.overlay.cursor_position, point(px(13.0), px(25.0)));
        assert_eq!(plan.overlay.cursor_size, size(px(8.0), px(20.0)));
    }

    #[test]
    fn block_cursor_text_uses_space_for_newlines() {
        assert_eq!(
            block_cursor_text("\n".into(), 0, CursorKind::Block, true, 4)
                .map(|text| text.to_string()),
            Some(" ".to_string())
        );
    }

    #[test]
    fn block_cursor_text_uses_grapheme_under_cursor() {
        assert_eq!(
            block_cursor_text("abc".into(), 1, CursorKind::Block, true, 4)
                .map(|text| text.to_string()),
            Some("b".to_string())
        );
    }

    #[test]
    fn block_cursor_text_expands_tabs_to_display_width() {
        assert_eq!(
            block_cursor_text("\thints".into(), 0, CursorKind::Block, true, 4)
                .map(|text| text.to_string()),
            Some("    ".to_string())
        );
        assert_eq!(
            block_cursor_text("a\tb".into(), 1, CursorKind::Block, true, 4)
                .map(|text| text.to_string()),
            Some("   ".to_string())
        );
    }

    #[test]
    fn block_cursor_text_requires_block_cursor_and_focus() {
        assert!(block_cursor_text("abc".into(), 1, CursorKind::Bar, true, 4).is_none());
        assert!(block_cursor_text("abc".into(), 1, CursorKind::Block, false, 4).is_none());
    }

    #[test]
    fn cursor_line_position_reports_ascii_offsets() {
        let position = cursor_line_position("abc\ndef".into(), 0, 2, false);

        assert_eq!(
            position,
            CursorLinePosition {
                line: 0,
                line_start: 0,
                line_end: 4,
                line_text: "abc".to_string(),
                cursor_char_offset: 2,
                cursor_byte_offset: 2,
            }
        );
    }

    #[test]
    fn cursor_line_position_converts_unicode_char_offset_to_byte_offset() {
        let position = cursor_line_position("aé𝌆z".into(), 0, 3, false);

        assert_eq!(position.line_text, "aé𝌆z");
        assert_eq!(position.cursor_char_offset, 3);
        assert_eq!(position.cursor_byte_offset, "aé𝌆".len());
    }

    #[test]
    fn cursor_line_position_clamps_to_line_text() {
        let position = cursor_line_position("abc\ndef".into(), 0, 99, false);

        assert_eq!(position.cursor_char_offset, 3);
        assert_eq!(position.cursor_byte_offset, 3);
    }

    #[test]
    fn cursor_line_position_uses_line_end_for_trailing_newline_cursor() {
        let position = cursor_line_position("abc\n".into(), 0, 4, true);

        assert_eq!(position.line_text, "abc");
        assert_eq!(position.cursor_char_offset, 3);
        assert_eq!(position.cursor_byte_offset, 3);
    }

    #[test]
    fn cursor_line_position_strips_crlf() {
        let position = cursor_line_position("abc\r\ndef".into(), 0, 3, false);

        assert_eq!(position.line_text, "abc");
        assert_eq!(position.cursor_char_offset, 3);
        assert_eq!(position.cursor_byte_offset, 3);
    }

    #[test]
    fn cursor_document_line_uses_cursor_char_line() {
        assert_eq!(cursor_document_line("abc\ndef".into(), 5, false), 1);
    }

    #[test]
    fn cursor_document_line_uses_final_empty_line_for_trailing_newline() {
        assert_eq!(cursor_document_line("abc\n".into(), 4, true), 1);
    }

    #[test]
    fn cursor_document_line_clamps_to_document_end() {
        assert_eq!(cursor_document_line("abc\ndef".into(), 99, false), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cursor_document_line_for_view_uses_primary_selection() {
        let (mut editor, doc_id, view_id) = test_editor_with_text("abc\ndef\n");
        let document = editor.document_mut(doc_id).unwrap();
        document.set_selection(view_id, Selection::point(5));

        assert_eq!(cursor_document_line_for_view(document, view_id), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cursor_document_line_for_view_uses_final_empty_line_at_trailing_newline() {
        let (mut editor, doc_id, view_id) = test_editor_with_text("abc\n");
        let document = editor.document_mut(doc_id).unwrap();
        document.set_selection(view_id, Selection::point(document.text().len_chars()));

        assert_eq!(
            cursor_document_line_for_view(document, view_id),
            document.text().len_lines().saturating_sub(1)
        );
    }

    #[test]
    fn cursor_viewport_position_reports_visible_row() {
        assert_eq!(
            cursor_viewport_position(7, 5, 10),
            Some(CursorViewportPosition {
                line: 7,
                viewport_row: 2,
            })
        );
    }

    #[test]
    fn cursor_viewport_position_rejects_rows_outside_range() {
        assert_eq!(cursor_viewport_position(4, 5, 10), None);
        assert_eq!(cursor_viewport_position(10, 5, 10), None);
    }

    #[test]
    fn soft_wrap_cursor_paint_plan_maps_visible_visual_position() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let text = "foo ".repeat(10);
        let text_format = soft_wrap_text_format();
        let plan = soft_wrap_cursor_paint_plan(SoftWrapCursorPaintPlanParams {
            text: text.as_str().into(),
            text_format: &text_format,
            text_annotations: None,
            precomputed_visual_position: None,
            anchor: 0,
            cursor_char_idx: 16,
            geometry,
            line_height: px(20.0),
            cell_width: px(8.0),
            scroll_line_offset: px(0.0),
            vertical_offset: 1,
            viewport_height: 4,
            horizontal_offset: 0,
        })
        .expect("visible cursor plan");

        assert_eq!(
            plan.visual_position,
            SoftWrapVisualPosition {
                visual_line: 1,
                visual_col: 1,
            }
        );
        assert_eq!(plan.paint_position.paint_origin, point(px(140.0), px(41.0)));
        assert_eq!(plan.paint_position.cursor_origin, point(px(0.0), px(0.0)));
    }

    #[test]
    fn soft_wrap_cursor_paint_plan_counts_inline_annotations_before_cursor() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let text = "ab";
        let text_format = soft_wrap_text_format();
        let annotations = [InlineAnnotation::new(1, ": hint")];
        let mut text_annotations = TextAnnotations::default();
        text_annotations.add_inline_annotations(&annotations, None);

        let plan = soft_wrap_cursor_paint_plan(SoftWrapCursorPaintPlanParams {
            text: text.into(),
            text_format: &text_format,
            text_annotations: Some(&text_annotations),
            precomputed_visual_position: None,
            anchor: 0,
            cursor_char_idx: 2,
            geometry,
            line_height: px(20.0),
            cell_width: px(8.0),
            scroll_line_offset: px(0.0),
            vertical_offset: 0,
            viewport_height: 4,
            horizontal_offset: 0,
        })
        .expect("visible cursor plan");

        assert_eq!(
            plan.visual_position,
            SoftWrapVisualPosition {
                visual_line: 0,
                visual_col: "a: hintb".len(),
            }
        );
        assert_eq!(plan.paint_position.paint_origin, point(px(196.0), px(41.0)));
    }

    #[test]
    fn soft_wrap_cursor_paint_plan_rejects_cursor_outside_viewport() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let text = "foo ".repeat(10);
        let text_format = soft_wrap_text_format();

        assert!(
            soft_wrap_cursor_paint_plan(SoftWrapCursorPaintPlanParams {
                text: text.as_str().into(),
                text_format: &text_format,
                text_annotations: None,
                precomputed_visual_position: None,
                anchor: 0,
                cursor_char_idx: 16,
                geometry,
                line_height: px(20.0),
                cell_width: px(8.0),
                scroll_line_offset: px(0.0),
                vertical_offset: 2,
                viewport_height: 4,
                horizontal_offset: 0,
            })
            .is_none()
        );
    }

    #[test]
    fn soft_wrap_cursor_paint_plan_rejects_cursor_before_anchor_row() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let text = "foo ".repeat(10);
        let text_format = soft_wrap_text_format();

        assert!(
            soft_wrap_cursor_paint_plan(SoftWrapCursorPaintPlanParams {
                text: text.as_str().into(),
                text_format: &text_format,
                text_annotations: None,
                precomputed_visual_position: None,
                anchor: 16,
                cursor_char_idx: 0,
                geometry,
                line_height: px(20.0),
                cell_width: px(8.0),
                scroll_line_offset: px(0.0),
                vertical_offset: 0,
                viewport_height: 4,
                horizontal_offset: 0,
            })
            .is_none()
        );
    }

    #[test]
    fn soft_wrap_cursor_paint_position_uses_text_bounds_and_offsets() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let cursor_position = SoftWrapVisualPosition {
            visual_line: 7,
            visual_col: 12,
        };

        let position = soft_wrap_cursor_paint_position(SoftWrapCursorPaintPositionParams {
            geometry,
            cursor_position,
            line_height: px(20.0),
            cell_width: px(8.0),
            scroll_line_offset: px(0.0),
            vertical_offset: 5,
            viewport_height: 10,
            horizontal_offset: 3,
        })
        .expect("visible cursor");

        assert_eq!(position.cursor_origin, point(px(0.0), px(0.0)));
        assert_eq!(position.paint_origin, point(px(204.0), px(81.0)));
        assert_eq!(position.cursor_point(), point(px(204.0), px(81.0)));
    }

    #[test]
    fn soft_wrap_cursor_paint_position_subtracts_scroll_offset() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let cursor_position = SoftWrapVisualPosition {
            visual_line: 7,
            visual_col: 12,
        };

        let position = soft_wrap_cursor_paint_position(SoftWrapCursorPaintPositionParams {
            geometry,
            cursor_position,
            line_height: px(20.0),
            cell_width: px(8.0),
            scroll_line_offset: px(5.0),
            vertical_offset: 5,
            viewport_height: 10,
            horizontal_offset: 3,
        })
        .expect("visible cursor");

        assert_eq!(position.paint_origin, point(px(204.0), px(76.0)));
    }

    #[test]
    fn soft_wrap_cursor_paint_position_rejects_rows_outside_viewport() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(0.0), px(0.0)), size(px(500.0), px(300.0))),
            0,
            px(8.0),
        );

        assert!(
            soft_wrap_cursor_paint_position(SoftWrapCursorPaintPositionParams {
                geometry,
                line_height: px(20.0),
                cell_width: px(8.0),
                cursor_position: SoftWrapVisualPosition {
                    visual_line: 4,
                    visual_col: 0,
                },
                scroll_line_offset: px(0.0),
                vertical_offset: 5,
                viewport_height: 10,
                horizontal_offset: 0,
            })
            .is_none()
        );
        assert!(
            soft_wrap_cursor_paint_position(SoftWrapCursorPaintPositionParams {
                geometry,
                line_height: px(20.0),
                cell_width: px(8.0),
                cursor_position: SoftWrapVisualPosition {
                    visual_line: 15,
                    visual_col: 0,
                },
                scroll_line_offset: px(0.0),
                vertical_offset: 5,
                viewport_height: 10,
                horizontal_offset: 0,
            })
            .is_none()
        );
    }

    #[test]
    fn unwrapped_cursor_paint_position_uses_line_layout_origin() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let layout = LineLayout::unwrapped(3, ShapedLine::default(), px(60.0));

        let position = unwrapped_cursor_paint_position(geometry, &layout, 0);

        assert_eq!(position.paint_origin, point(px(132.0), px(101.0)));
        assert_eq!(position.cursor_origin, point(px(0.0), px(0.0)));
    }

    #[test]
    fn phantom_line_cursor_paint_position_uses_text_origin() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );

        let position = phantom_line_cursor_paint_position(geometry, px(60.0));

        assert_eq!(position.paint_origin, point(px(132.0), px(101.0)));
        assert_eq!(position.cursor_origin, point(px(0.0), px(0.0)));
    }

    #[test]
    fn unwrapped_cursor_paint_plan_uses_matching_line_layout() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let layout = LineLayout::unwrapped(1, ShapedLine::default(), px(20.0));
        let plan = unwrapped_cursor_paint_plan(UnwrappedCursorPaintPlanParams {
            text: "zero\none".into(),
            geometry,
            cursor_char_idx: 5,
            cursor_at_trailing_newline: false,
            cursor_viewport_position: Some(CursorViewportPosition {
                line: 1,
                viewport_row: 1,
            }),
            line_layout: Some(&layout),
            line_height: px(20.0),
            scroll_line_offset: px(0.0),
        })
        .expect("visible cursor plan");

        assert_eq!(plan.source, UnwrappedCursorPaintPlanSource::LineLayout);
        assert_eq!(plan.paint_position.paint_origin, point(px(132.0), px(61.0)));
        assert_eq!(
            plan.line_position
                .as_ref()
                .map(|position| position.cursor_char_offset),
            Some(0)
        );
    }

    #[test]
    fn unwrapped_cursor_paint_plan_uses_phantom_line_for_trailing_newline() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let plan = unwrapped_cursor_paint_plan(UnwrappedCursorPaintPlanParams {
            text: "one\n".into(),
            geometry,
            cursor_char_idx: 4,
            cursor_at_trailing_newline: true,
            cursor_viewport_position: Some(CursorViewportPosition {
                line: 1,
                viewport_row: 1,
            }),
            line_layout: None,
            line_height: px(20.0),
            scroll_line_offset: px(0.0),
        })
        .expect("phantom cursor plan");

        assert_eq!(
            plan.source,
            UnwrappedCursorPaintPlanSource::PhantomTrailingNewline
        );
        assert_eq!(plan.paint_position.paint_origin, point(px(132.0), px(61.0)));
        assert!(plan.line_position.is_none());
    }

    #[test]
    fn unwrapped_cursor_paint_plan_anchors_trailing_newline_to_viewport_row() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(100.0), px(40.0)), size(px(500.0), px(300.0))),
            4,
            px(8.0),
        );
        let plan = unwrapped_cursor_paint_plan(UnwrappedCursorPaintPlanParams {
            text: "one\ntwo\n".into(),
            geometry,
            cursor_char_idx: 8,
            cursor_at_trailing_newline: true,
            cursor_viewport_position: Some(CursorViewportPosition {
                line: 2,
                viewport_row: 0,
            }),
            line_layout: None,
            line_height: px(20.0),
            scroll_line_offset: px(7.0),
        })
        .expect("phantom cursor plan");

        assert_eq!(
            plan.source,
            UnwrappedCursorPaintPlanSource::PhantomTrailingNewline
        );
        assert_eq!(plan.paint_position.paint_origin, point(px(132.0), px(34.0)));
    }

    #[test]
    fn unwrapped_cursor_paint_plan_rejects_hidden_or_missing_layouts() {
        let geometry = EditorSurfaceGeometry::new(
            Bounds::new(point(px(0.0), px(0.0)), size(px(500.0), px(300.0))),
            0,
            px(8.0),
        );
        let layout = LineLayout::unwrapped(2, ShapedLine::default(), px(20.0));

        assert!(
            unwrapped_cursor_paint_plan(UnwrappedCursorPaintPlanParams {
                text: "one\ntwo".into(),
                geometry,
                cursor_char_idx: 0,
                cursor_at_trailing_newline: false,
                cursor_viewport_position: None,
                line_layout: Some(&layout),
                line_height: px(20.0),
                scroll_line_offset: px(0.0),
            })
            .is_none()
        );
        assert!(
            unwrapped_cursor_paint_plan(UnwrappedCursorPaintPlanParams {
                text: "one\ntwo".into(),
                geometry,
                cursor_char_idx: 0,
                cursor_at_trailing_newline: false,
                cursor_viewport_position: Some(CursorViewportPosition {
                    line: 1,
                    viewport_row: 1,
                }),
                line_layout: Some(&layout),
                line_height: px(20.0),
                scroll_line_offset: px(0.0),
            })
            .is_none()
        );
    }

    #[test]
    fn cursor_foreground_uses_background_when_reversed() {
        let bg = hsla(0.2, 0.3, 0.4, 1.0);

        assert_eq!(cursor_foreground_color(&Style::default(), true, bg), bg);
    }

    #[test]
    fn cursor_presentation_block_text_color_respects_reversed_style() {
        let bg = hsla(0.2, 0.3, 0.4, 1.0);
        let presentation = EditorCursorPresentation {
            cursor_char_idx: 0,
            kind: CursorKind::Block,
            cursor_style: Style::default().add_modifier(Modifier::REVERSED),
            text_style_at_cursor: Style::default(),
            block_text: Some("x".into()),
        };

        assert_eq!(presentation.block_text_color(bg), bg);
    }

    #[test]
    fn cursor_background_uses_text_style_when_reversed() {
        let cursor_style = Style::default().add_modifier(Modifier::REVERSED);
        let text_style = Style::default().fg(Color::Rgb(255, 0, 0));

        assert_ne!(
            cursor_background_color(&cursor_style, &text_style, black()),
            black()
        );
    }
}
