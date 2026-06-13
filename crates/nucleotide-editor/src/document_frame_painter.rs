// ABOUTME: Paint helpers for frame-owned native editor render state
// ABOUTME: Converts EditorDocumentFrame plans into GPUI paint calls

use gpui::{App, Bounds, Hsla, Pixels, TextStyle, Window};
use helix_core::RopeSlice;
use nucleotide_logging::{debug, error};

use crate::{
    CursorOverlayPlan, CursorTextShape, EditorDocumentFrame, EditorLayout,
    EditorLineBackgroundStyle, EditorSurfaceGeometry, LineLayoutCache,
    ShapedEditorCursorPaintParams, UnwrappedCursorPaintPlanParams, UnwrappedEditorLinePaintParams,
    build_gutter_lines_from_plans, paint_gutter_lines, paint_shaped_editor_cursor,
    paint_unwrapped_editor_line, unwrapped_cursor_paint_plan,
};

pub struct UnwrappedDocumentFramePaintParams<'a> {
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

pub fn paint_unwrapped_document_frame(
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
