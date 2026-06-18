// ABOUTME: Native editor hit testing against rendered line layouts
// ABOUTME: Converts GPUI surface pointer events into Helix document character positions

use gpui::{Pixels, px};
use helix_view::Document;

use crate::{EditorSurfaceGeometry, EditorSurfacePointerEvent, LineLayout, LineLayoutCache};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorHitTestResult {
    pub line_idx: usize,
    pub char_offset: usize,
    pub char_idx: usize,
}

pub fn hit_test_document_position(
    event: EditorSurfacePointerEvent,
    gutter_columns: u16,
    line_cache: &LineLayoutCache,
    document: &Document,
) -> Option<EditorHitTestResult> {
    let geometry = EditorSurfaceGeometry::new(event.bounds, gutter_columns, event.cell_width);
    let text_bounds = geometry.text_bounds();
    let text_area_pos = geometry.window_to_text_area(event.position);
    let clamped_text_area_pos = geometry.clamp_text_area_position(text_area_pos, false);

    let line_layout = line_cache.find_line_at_position(
        clamped_text_area_pos,
        text_bounds.size.width,
        event.line_height,
    )?;

    let relative_x = clamped_text_area_pos.x - line_layout.origin.x;
    let byte_index = byte_index_for_relative_x(&line_layout, relative_x);

    let text = document.text();
    let line_start = text.line_to_char(line_layout.line_idx);
    let line_text = text.line(line_layout.line_idx).to_string();
    let char_offset = char_offset_for_byte_index(&line_layout, &line_text, byte_index);
    let char_idx = (line_start + char_offset).min(text.len_chars());

    Some(EditorHitTestResult {
        line_idx: line_layout.line_idx,
        char_offset,
        char_idx,
    })
}

fn byte_index_for_relative_x(line_layout: &LineLayout, relative_x: Pixels) -> usize {
    if relative_x < px(0.0) {
        0
    } else if relative_x > line_layout.shaped_line.width {
        line_layout.shaped_line.len()
    } else {
        line_layout.shaped_line.index_for_x(relative_x).unwrap_or(0)
    }
}

fn char_offset_for_byte_index(
    line_layout: &LineLayout,
    line_text: &str,
    byte_index: usize,
) -> usize {
    let adjusted_byte_index = line_layout.source_byte_for_display_byte(byte_index);

    let mut consumed_bytes = 0;
    let mut consumed_chars = 0;

    for ch in line_text.chars().skip(line_layout.segment_char_offset) {
        if consumed_bytes >= adjusted_byte_index {
            break;
        }

        consumed_bytes += ch.len_utf8();
        consumed_chars += 1;
    }

    line_layout.segment_char_offset + consumed_chars
}

#[cfg(test)]
mod tests {
    use gpui::{ShapedLine, point, px};

    use super::*;
    use crate::line_text::{DisplayLineText, DisplayTextMap};

    fn test_layout(segment_char_offset: usize, text_start_byte_offset: usize) -> LineLayout {
        LineLayout {
            line_idx: 0,
            shaped_line: ShapedLine::default(),
            origin: point(px(0.0), px(0.0)),
            segment_char_offset,
            text_start_byte_offset,
            display_map: DisplayTextMap::identity(64),
        }
    }

    #[test]
    fn byte_offsets_map_to_character_offsets() {
        let layout = test_layout(0, 0);

        assert_eq!(char_offset_for_byte_index(&layout, "abcd", 0), 0);
        assert_eq!(char_offset_for_byte_index(&layout, "abcd", 2), 2);
        assert_eq!(char_offset_for_byte_index(&layout, "abcd", 8), 4);
    }

    #[test]
    fn multibyte_offsets_count_characters_not_bytes() {
        let layout = test_layout(0, 0);

        assert_eq!(char_offset_for_byte_index(&layout, "aé日", 1), 1);
        assert_eq!(char_offset_for_byte_index(&layout, "aé日", 2), 2);
        assert_eq!(char_offset_for_byte_index(&layout, "aé日", 3), 2);
        assert_eq!(char_offset_for_byte_index(&layout, "aé日", 6), 3);
    }

    #[test]
    fn wrapped_segments_account_for_virtual_prefix_bytes() {
        let mut layout = test_layout(3, 2);
        layout.display_map =
            DisplayLineText::from_source_with_prefix("  ", "def".to_string(), 2, 4).map;

        assert_eq!(char_offset_for_byte_index(&layout, "abcdef", 2), 3);
        assert_eq!(char_offset_for_byte_index(&layout, "abcdef", 4), 5);
        assert_eq!(char_offset_for_byte_index(&layout, "abcdef", 8), 6);
    }

    #[test]
    fn expanded_tab_display_bytes_map_back_to_source_tab() {
        let mut layout = test_layout(0, 0);
        layout.display_map = DisplayLineText::from_source("a\tb".to_string(), 4).map;

        assert_eq!(char_offset_for_byte_index(&layout, "a\tb", 1), 1);
        assert_eq!(char_offset_for_byte_index(&layout, "a\tb", 3), 1);
        assert_eq!(char_offset_for_byte_index(&layout, "a\tb", 4), 2);
    }
}
