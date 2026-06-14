// ABOUTME: Paint helpers for frame-owned native editor render state
// ABOUTME: Converts EditorDocumentFrame plans into GPUI paint calls

use gpui::{App, Bounds, Hsla, Pixels, TextStyle, Window, px};
use helix_core::RopeSlice;
use helix_view::Theme;
use nucleotide_logging::{debug, error};

use crate::{
    CursorOverlayPlan, CursorTextShape, DiagnosticGutterMarkersPaintParams,
    EditorCursorTextPaintParams, EditorDocumentFrame, EditorLayout, EditorLineBackgroundStyle,
    EditorSurfaceGeometry, LineLayoutCache, ShapedEditorCursorPaintParams,
    SoftWrapCursorPaintPlanParams, SoftWrapEditorLinePaintParams, SoftWrapGutterPaintParams,
    UnwrappedCursorPaintPlanParams, UnwrappedEditorLinePaintParams, build_gutter_lines_from_plans,
    gutter::SoftWrapGutterLine, paint_diagnostic_gutter_markers, paint_gutter_lines,
    paint_shaped_editor_cursor, paint_soft_wrap_editor_line, paint_soft_wrap_gutter,
    paint_unwrapped_editor_line, paint_visible_rulers, shape_and_paint_editor_cursor,
    soft_wrap_cursor_paint_plan, unwrapped_cursor_paint_plan,
};

pub struct DocumentFramePaintParams<'a> {
    pub frame: &'a EditorDocumentFrame,
    pub text: RopeSlice<'a>,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub line_cache: &'a LineLayoutCache,
    pub font_size: Pixels,
    pub fg_color: Hsla,
    pub default_bg: Hsla,
    pub cursorline_color: Option<Hsla>,
    pub cursor_text_shape: &'a CursorTextShape,
    pub is_focused: bool,
    pub element_focused: bool,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
    pub gutter_color: Hsla,
    pub gutter_selected_color: Hsla,
    pub diagnostic_theme: &'a Theme,
    pub diagnostic_highlight_base: Hsla,
    pub gutter_bg: Option<Hsla>,
    pub scroll_line_offset: Pixels,
}

struct UnwrappedDocumentFramePaintParams<'a> {
    pub frame: &'a EditorDocumentFrame,
    pub text: RopeSlice<'a>,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub line_cache: &'a LineLayoutCache,
    pub font_size: Pixels,
    pub fg_color: Hsla,
    pub cursorline_color: Option<Hsla>,
    pub cursor_text_shape: &'a CursorTextShape,
    pub is_focused: bool,
    pub element_focused: bool,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
}

struct SoftWrapDocumentFramePaintParams<'a> {
    pub frame: &'a EditorDocumentFrame,
    pub text: RopeSlice<'a>,
    pub bounds: Bounds<Pixels>,
    pub layout: &'a EditorLayout,
    pub text_style: &'a TextStyle,
    pub line_cache: &'a LineLayoutCache,
    pub font_size: Pixels,
    pub fg_color: Hsla,
    pub default_bg: Hsla,
    pub cursorline_color: Option<Hsla>,
    pub is_focused: bool,
    pub element_focused: bool,
    pub selection_primary: Hsla,
    pub selection_secondary: Hsla,
    pub gutter_color: Hsla,
    pub gutter_selected_color: Hsla,
    pub diagnostic_theme: &'a Theme,
    pub diagnostic_highlight_base: Hsla,
    pub gutter_bg: Option<Hsla>,
    pub scroll_line_offset: Pixels,
}

pub fn paint_document_frame(
    window: &mut Window,
    cx: &mut App,
    params: DocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    paint_visible_rulers(window, &params.frame.ruler_paint_plans);

    if params.frame.soft_wrap_render_plan.is_some() {
        return paint_soft_wrap_document_frame(
            window,
            cx,
            SoftWrapDocumentFramePaintParams {
                frame: params.frame,
                text: params.text,
                bounds: params.bounds,
                layout: params.layout,
                text_style: params.text_style,
                line_cache: params.line_cache,
                font_size: params.font_size,
                fg_color: params.fg_color,
                default_bg: params.default_bg,
                cursorline_color: params.cursorline_color,
                is_focused: params.is_focused,
                element_focused: params.element_focused,
                selection_primary: params.selection_primary,
                selection_secondary: params.selection_secondary,
                gutter_color: params.gutter_color,
                gutter_selected_color: params.gutter_selected_color,
                diagnostic_theme: params.diagnostic_theme,
                diagnostic_highlight_base: params.diagnostic_highlight_base,
                gutter_bg: params.gutter_bg,
                scroll_line_offset: params.scroll_line_offset,
            },
        );
    }

    paint_unwrapped_document_frame(
        window,
        cx,
        UnwrappedDocumentFramePaintParams {
            frame: params.frame,
            text: params.text,
            bounds: params.bounds,
            layout: params.layout,
            text_style: params.text_style,
            line_cache: params.line_cache,
            font_size: params.font_size,
            fg_color: params.fg_color,
            cursorline_color: params.cursorline_color,
            cursor_text_shape: params.cursor_text_shape,
            is_focused: params.is_focused,
            element_focused: params.element_focused,
            selection_primary: params.selection_primary,
            selection_secondary: params.selection_secondary,
        },
    )
}

fn paint_soft_wrap_document_frame(
    window: &mut Window,
    cx: &mut App,
    params: SoftWrapDocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    let frame = params.frame;
    let soft_wrap_render_plan = frame.soft_wrap_render_plan.as_ref()?;

    let soft_wrap_paint_plans = soft_wrap_render_plan.line_paint_plans(
        params.layout.line_height,
        params.scroll_line_offset,
        frame.render_snapshot.cursor_line,
    );

    for (line_plan, line_runs) in soft_wrap_paint_plans
        .into_iter()
        .zip(frame.soft_wrap_line_runs.iter())
    {
        match paint_soft_wrap_editor_line(
            window,
            cx,
            SoftWrapEditorLinePaintParams {
                plan: line_plan,
                line_runs,
                line_cache: params.line_cache,
                font_size: params.font_size,
                viewport_width: params.bounds.size.width,
                line_height: params.layout.line_height,
                cursorline_color: params.cursorline_color,
                background_style: EditorLineBackgroundStyle {
                    only_selection_backgrounds: line_plan.is_cursor_visual_line,
                    selection_primary: params.selection_primary,
                    selection_secondary: params.selection_secondary,
                },
            },
        ) {
            Ok(Some(layout)) => params.line_cache.push(layout),
            Ok(None) => {}
            Err(e) => {
                error!(error = ?e, "Failed to paint text");
            }
        }
    }

    let gutter_lines = paint_soft_wrap_frame_gutter(window, cx, &params);
    paint_diagnostic_gutter_markers(
        window,
        DiagnosticGutterMarkersPaintParams {
            severity_by_line: &frame.diagnostic_severity_by_line,
            gutter_lines: &gutter_lines,
            theme: params.diagnostic_theme,
            gutter_origin: soft_wrap_gutter_origin(params.bounds),
            line_height: params.layout.line_height,
            highlight_base: params.diagnostic_highlight_base,
            gutter_bg: params.gutter_bg,
        },
    );

    paint_soft_wrap_cursor(window, cx, &params)
}

fn paint_unwrapped_document_frame(
    window: &mut Window,
    cx: &mut App,
    params: UnwrappedDocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    let frame = params.frame;
    let unwrapped_plan = frame.unwrapped_render_plan.as_ref()?;

    let next_unwrapped_line_y_offset = unwrapped_plan.next_line_y_offset;
    let unwrapped_paint_plans = unwrapped_plan.line_paint_plans();

    for (unwrapped_plan, highlighted_line) in unwrapped_paint_plans
        .into_iter()
        .zip(frame.unwrapped_highlighted_lines.iter())
    {
        let line_plan = unwrapped_plan.line;
        let line_idx = line_plan.line_idx;
        let y_offset = line_plan.y_offset;
        let line_text = highlighted_line.line_text.clone();
        let line_runs = &highlighted_line.line_runs;

        if unwrapped_plan.is_cursor_line && params.cursorline_color.is_some() {
            debug!(
                "Painting cursorline for line {} (cursor at line {})",
                line_idx, frame.render_snapshot.cursor_line
            );
        }

        let layout = match paint_unwrapped_editor_line(
            window,
            cx,
            UnwrappedEditorLinePaintParams {
                plan: unwrapped_plan,
                line_text,
                line_runs,
                line_cache: params.line_cache,
                font_size: params.font_size,
                viewport_width: params.bounds.size.width,
                line_height: params.layout.line_height,
                cursorline_color: params.cursorline_color,
                background_style: EditorLineBackgroundStyle {
                    only_selection_backgrounds: unwrapped_plan.is_cursor_line,
                    selection_primary: params.selection_primary,
                    selection_secondary: params.selection_secondary,
                },
            },
        ) {
            Ok(layout) => layout,
            Err(e) => {
                error!(error = ?e, "Failed to paint text");
                continue;
            }
        };

        debug!(
            "LINE LAYOUT CACHED: line_idx={}, y_offset={:?}, is_phantom={}",
            line_idx, y_offset, false
        );
        params.line_cache.push(layout);
    }

    let cursor_overlay = paint_unwrapped_cursor(window, cx, &params, next_unwrapped_line_y_offset);
    paint_unwrapped_gutter(window, cx, &params);

    cursor_overlay
}

fn paint_soft_wrap_frame_gutter(
    window: &mut Window,
    cx: &mut App,
    params: &SoftWrapDocumentFramePaintParams<'_>,
) -> Vec<SoftWrapGutterLine> {
    let frame = params.frame;
    let Some(soft_wrap_render_plan) = frame.soft_wrap_render_plan.as_ref() else {
        return Vec::new();
    };

    paint_soft_wrap_gutter(
        window,
        cx,
        SoftWrapGutterPaintParams {
            text_system: window.text_system().clone(),
            text_style: params.text_style,
            font_size: params.font_size,
            visual_lines: &soft_wrap_render_plan.visual_lines,
            vertical_offset: soft_wrap_render_plan.view_offset.vertical_offset,
            line_height: params.layout.line_height,
            scroll_line_offset: params.scroll_line_offset,
            cursor_lines: &frame.render_snapshot.cursor_lines,
            origin: soft_wrap_gutter_origin(params.bounds),
            gutter_color: params.gutter_color,
            gutter_selected_color: params.gutter_selected_color,
        },
        |_| {},
    )
}

fn paint_soft_wrap_cursor(
    window: &mut Window,
    cx: &mut App,
    params: &SoftWrapDocumentFramePaintParams<'_>,
) -> Option<CursorOverlayPlan> {
    if !(params.is_focused || params.element_focused) {
        return None;
    }

    let frame = params.frame;
    let soft_wrap_render_plan = frame.soft_wrap_render_plan.as_ref()?;
    let cursor_paint_plan = soft_wrap_cursor_paint_plan(SoftWrapCursorPaintPlanParams {
        text: params.text,
        text_format: &soft_wrap_render_plan.text_format,
        anchor: soft_wrap_render_plan.view_offset.anchor,
        cursor_char_idx: frame.cursor_presentation.cursor_char_idx,
        geometry: EditorSurfaceGeometry::new(
            params.bounds,
            frame.gutter_width,
            params.layout.cell_width,
        ),
        line_height: params.layout.line_height,
        cell_width: params.layout.cell_width,
        scroll_line_offset: params.scroll_line_offset,
        vertical_offset: soft_wrap_render_plan.view_offset.vertical_offset,
        viewport_height: soft_wrap_render_plan.viewport_height,
        horizontal_offset: soft_wrap_render_plan.view_offset.horizontal_offset,
    })?;

    let font = params.text_style.font();
    Some(shape_and_paint_editor_cursor(
        window,
        cx,
        EditorCursorTextPaintParams {
            paint_position: cursor_paint_plan.paint_position,
            kind: frame.cursor_presentation.kind,
            cursor_style: &frame.cursor_presentation.cursor_style,
            text_style_at_cursor: &frame.cursor_presentation.text_style_at_cursor,
            cursor_text: frame.cursor_presentation.block_text.clone(),
            font: &font,
            font_size: params.font_size,
            fallback_fg: params.fg_color,
            default_bg: params.default_bg,
            fallback_width: params.layout.cell_width,
            line_height: params.layout.line_height,
        },
    ))
}

fn soft_wrap_gutter_origin(bounds: Bounds<Pixels>) -> gpui::Point<Pixels> {
    let mut gutter_origin = bounds.origin;
    gutter_origin.x += px(2.);
    gutter_origin.y += px(1.);
    gutter_origin
}

fn paint_unwrapped_cursor(
    window: &mut Window,
    cx: &mut App,
    params: &UnwrappedDocumentFramePaintParams<'_>,
    next_unwrapped_line_y_offset: Pixels,
) -> Option<CursorOverlayPlan> {
    let frame = params.frame;
    let cursor_viewport_pos = frame.render_snapshot.cursor_viewport_position;
    debug!(
        "Cursor rendering check - is_focused: {}, element_focused: {}, cursor_viewport_pos: {:?}",
        params.is_focused, params.element_focused, cursor_viewport_pos
    );
    debug!(
        "Cursor char idx: {}, line: {}",
        frame.render_snapshot.cursor_char_idx, frame.render_snapshot.cursor_line
    );

    if !(params.is_focused || params.element_focused) {
        debug!(
            "Cursor rendering skipped - is_focused: {}, element_focused: {}",
            params.is_focused, params.element_focused
        );
        return None;
    }

    let Some(cursor_viewport_pos) = cursor_viewport_pos else {
        debug!(
            "Cursor line {} is outside rendered range {}..{}",
            frame.render_snapshot.cursor_doc_line,
            frame.render_snapshot.line_viewport.first_row,
            frame.render_snapshot.last_row
        );
        return None;
    };

    let cursor_line = cursor_viewport_pos.line;
    debug!(
        "Looking for cursor line {cursor_line} in range {}..{}",
        frame.render_snapshot.line_viewport.first_row, frame.render_snapshot.last_row
    );

    let line_layout = params.line_cache.find_line_by_index(cursor_line);
    let line_viewport = frame.render_snapshot.line_viewport;
    let cursor_paint_plan = unwrapped_cursor_paint_plan(UnwrappedCursorPaintPlanParams {
        text: params.text.slice(..),
        geometry: EditorSurfaceGeometry::new(
            params.bounds,
            frame.gutter_width,
            params.layout.cell_width,
        ),
        cursor_char_idx: frame.cursor_presentation.cursor_char_idx,
        cursor_at_trailing_newline: line_viewport.cursor_at_end
            && line_viewport.file_ends_with_newline,
        cursor_viewport_position: Some(cursor_viewport_pos),
        line_layout: line_layout.as_ref(),
        next_line_y_offset: next_unwrapped_line_y_offset,
    });

    let Some(cursor_paint_plan) = cursor_paint_plan else {
        debug!(
            "Cursor paint plan unavailable for visible line {} (at_eof: {})",
            cursor_line,
            line_viewport.cursor_at_end && line_viewport.file_ends_with_newline
        );
        return None;
    };

    let cursor_paint_position = cursor_paint_plan.paint_position;
    if let Some(line_position) = &cursor_paint_plan.line_position {
        debug!(
            "Cursor rendering - line: {}, char_offset: {}, byte_offset: {}, x_relative: {:?}, x_absolute: {:?}, viewport_row: {}",
            line_position.line,
            line_position.cursor_char_offset,
            line_position.cursor_byte_offset,
            cursor_paint_position.cursor_origin.x,
            cursor_paint_position.cursor_point().x,
            cursor_viewport_pos.viewport_row
        );
    } else {
        debug!(
            "Cursor rendering - source: {:?}, x_absolute: {:?}, viewport_row: {}",
            cursor_paint_plan.source,
            cursor_paint_position.cursor_point().x,
            cursor_viewport_pos.viewport_row
        );
    }
    debug!("Cursor paint plan selected: {:?}", cursor_paint_plan.source);

    Some(paint_shaped_editor_cursor(
        window,
        cx,
        ShapedEditorCursorPaintParams {
            paint_position: cursor_paint_position,
            kind: frame.cursor_presentation.kind,
            cursor_style: &frame.cursor_presentation.cursor_style,
            text_style_at_cursor: &frame.cursor_presentation.text_style_at_cursor,
            cursor_text_shape: params.cursor_text_shape.clone(),
            fallback_fg: params.fg_color,
            fallback_width: params.layout.cell_width,
            line_height: params.layout.line_height,
        },
    ))
}

fn paint_unwrapped_gutter(
    window: &mut Window,
    cx: &mut App,
    params: &UnwrappedDocumentFramePaintParams<'_>,
) {
    let gutter_lines = build_gutter_lines_from_plans(
        window.text_system().clone(),
        params.text_style,
        params.layout.font_size,
        &params.frame.unwrapped_gutter_line_plans,
    );

    paint_gutter_lines(
        window,
        cx,
        &gutter_lines,
        params.layout.line_height,
        |result| {
            let Err(e) = result else {
                return;
            };
            error!(error = ?e, "Failed to paint gutter line");
        },
    );
}
