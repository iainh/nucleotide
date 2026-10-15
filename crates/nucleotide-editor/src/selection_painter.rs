// ABOUTME: Projects document selections through shaped display text
// ABOUTME: Paints connected text contours and subdued multi-line gutter ranges

use std::ops::Range;

use gpui::{
    Bounds, ContentMask, Hsla, Pixels, Point, SharedString, TextRun, Window, fill, point, size,
};
use helix_core::RopeSlice;

use crate::{
    DocumentFramePaintParams, EditorSurfaceGeometry,
    line_cache::grapheme_x_for_index,
    line_painter::paint_line_backgrounds,
    line_text::{DisplayTextMap, normalize_text_runs_for_display_text},
    selection_shape::selection_paths,
};

/// Paint all run backgrounds before the selection contours and glyphs. Shaping
/// uses the same cache and display maps as the subsequent text paint pass.
pub(crate) fn paint_document_selection_backgrounds(
    window: &mut Window,
    params: &DocumentFramePaintParams<'_>,
) {
    let frame = params.frame;
    let geometry =
        EditorSurfaceGeometry::new(params.bounds, frame.gutter_width, params.layout.cell_width);
    let text_bounds = geometry.text_bounds();
    let gutter_bounds = Bounds::new(
        params.bounds.origin,
        size(geometry.gutter_width(), params.bounds.size.height),
    );
    let mut text_rows = vec![Vec::new(); frame.selection_spans.len()];
    let mut gutter_rows: Vec<(Hsla, Vec<Bounds<Pixels>>)> = Vec::new();
    let mut matching_bracket_rows = Vec::new();
    let text_system = window.text_system().clone();

    let mut collect_line = |window: &mut Window,
                            display: SharedString,
                            map: &DisplayTextMap,
                            prefix_len: usize,
                            char_start: usize,
                            char_end: usize,
                            doc_line: usize,
                            origin: Point<Pixels>,
                            runs: &[TextRun]| {
        let runs = normalize_text_runs_for_display_text(display.as_ref(), runs);
        let shaped = params.line_cache.shape_line_cached(
            text_system.as_ref(),
            display.clone(),
            params.font_size,
            params.bounds.size.width,
            runs.as_ref(),
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: text_bounds,
            }),
            |window| {
                paint_line_backgrounds(
                    window,
                    &shaped,
                    runs.as_ref(),
                    origin,
                    params.layout.line_height,
                );
            },
        );
        if let Some((range, _)) = &frame.matching_bracket_background
            && let Some(bytes) = selection_display_range(
                params.text,
                char_start,
                display.as_ref(),
                map,
                prefix_len,
                range,
            )
        {
            let start = grapheme_x_for_index(&shaped, bytes.start);
            let end = grapheme_x_for_index(&shaped, bytes.end);
            matching_bracket_rows.push(Bounds::new(
                point(origin.x + start, origin.y),
                size(end - start, params.layout.line_height),
            ));
        }
        for ((selection, _), rows) in frame.selection_spans.iter().zip(&mut text_rows) {
            if selection.end <= char_start || selection.start > char_end {
                continue;
            }
            let bytes = selection_display_range(
                params.text,
                char_start,
                display.as_ref(),
                map,
                prefix_len,
                selection,
            );
            let newline_selected = char_end < params.text.len_chars()
                && matches!(params.text.char(char_end), '\n' | '\r')
                && selection.contains(&char_end);
            if bytes.is_none() && !newline_selected {
                continue;
            }
            let start = bytes.as_ref().map_or(shaped.width, |range| {
                grapheme_x_for_index(&shaped, range.start)
            });
            let end = if newline_selected {
                shaped.width + params.layout.cell_width
            } else {
                grapheme_x_for_index(&shaped, bytes.as_ref().unwrap().end)
            };
            if end > start {
                rows.push(Bounds::new(
                    point(origin.x + start, origin.y),
                    size(end - start, params.layout.line_height),
                ));
            }
        }
        // A gutter row is filled once even when multiple selections touch it.
        // Primary spans come first; equal colours share one seamless contour.
        if let Some((_, color)) = frame
            .gutter_selection_spans
            .iter()
            .find(|(lines, _)| lines.contains(&doc_line))
        {
            let index = if let Some(index) = gutter_rows
                .iter()
                .position(|(existing, _)| existing == color)
            {
                index
            } else {
                gutter_rows.push((*color, Vec::new()));
                gutter_rows.len() - 1
            };
            gutter_rows[index].1.push(Bounds::new(
                point(gutter_bounds.origin.x, origin.y),
                size(gutter_bounds.size.width, params.layout.line_height),
            ));
        }
    };

    if let Some(plan) = &frame.soft_wrap_render_plan {
        for (line, runs) in plan
            .line_paint_plans(
                params.layout.line_height,
                params.scroll_line_offset,
                frame.primary_cursor_line,
            )
            .into_iter()
            .zip(&frame.soft_wrap_line_runs)
        {
            if let (Some(start), Some(end)) =
                (line.visual.line_start_char, line.visual.line_end_char)
            {
                collect_line(
                    window,
                    line.visual.text.clone(),
                    &line.visual.display_map,
                    line.visual.text_start_byte_offset,
                    start,
                    end,
                    line.visual.doc_line,
                    line.text_origin,
                    runs,
                );
            }
        }
    } else if let Some(plan) = &frame.unwrapped_render_plan {
        for (line, highlighted) in plan
            .line_paint_plans()
            .into_iter()
            .zip(&frame.unwrapped_highlighted_lines)
        {
            collect_line(
                window,
                highlighted.line_text.display.clone(),
                &highlighted.line_text.map,
                0,
                line.line.line_start,
                line.line.line_end,
                line.line.line_idx,
                line.text_origin,
                &highlighted.line_runs,
            );
        }
    }

    for ((mut color, rows), (viewport, opacity)) in frame
        .selection_spans
        .iter()
        .zip(text_rows)
        .map(|((_, color), rows)| ((*color, rows), (text_bounds, 1.0)))
        .chain(
            gutter_rows
                .into_iter()
                .map(|entry| (entry, (gutter_bounds, 0.4))),
        )
    {
        color.a *= opacity;
        window.with_content_mask(Some(ContentMask { bounds: viewport }), |window| {
            for path in selection_paths(
                &rows,
                params.layout.cell_width,
                params.layout.line_height,
                viewport,
            ) {
                window.paint_path(path, color);
            }
        });
    }
    // Bracket matching has higher priority than selection in Helix's overlay
    // stack. Restore that cell above the contours, still below the glyphs.
    if let Some((_, color)) = &frame.matching_bracket_background {
        window.with_content_mask(
            Some(ContentMask {
                bounds: text_bounds,
            }),
            |window| {
                for bounds in matching_bracket_rows {
                    window.paint_quad(fill(bounds, *color));
                }
            },
        );
    }
}

/// Display bytes are mapped back to document anchors so expanded tabs, Unicode
/// and selected inlay hints participate in the same contour as their source.
pub(crate) fn selection_display_range(
    text: RopeSlice<'_>,
    char_start: usize,
    display: &str,
    map: &DisplayTextMap,
    prefix_len: usize,
    selection: &Range<usize>,
) -> Option<Range<usize>> {
    let source_start = text.char_to_byte(char_start);
    let mut selected = None;
    for (byte, ch) in display
        .char_indices()
        .filter(|(byte, _)| *byte >= prefix_len)
    {
        let source_byte = source_start + map.source_byte_for_display_byte(byte);
        let char_idx = text.byte_to_char(source_byte.min(text.len_bytes()));
        if selection.contains(&char_idx) {
            let range = selected.get_or_insert(byte..byte);
            range.end = byte + ch.len_utf8();
        }
    }
    selected
}
