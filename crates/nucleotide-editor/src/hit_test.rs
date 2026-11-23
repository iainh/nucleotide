// ABOUTME: Native editor hit testing against rendered line layouts
// ABOUTME: Converts GPUI surface pointer events into Helix document character positions

use gpui::{Bounds, Pixels, point, px, size};
use helix_core::chars::char_is_word;
use helix_view::Document;

use crate::line_cache::grapheme_index_for_x;
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
    let mut clamped_text_area_pos = geometry.clamp_text_area_position(text_area_pos, true);
    clamped_text_area_pos.y = clamped_text_area_pos
        .y
        .min(text_bounds.size.height - px(0.01));

    let line_layout = line_cache.find_line_at_clamped_position(
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

/// The word under the pointer, for hover requests: where to ask the language
/// server and the window bounds of the painted word to anchor a card to.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorHoverTarget {
    pub char_idx: usize,
    /// Document character range of the hovered word.
    pub word: std::ops::Range<usize>,
    /// Window bounds of the word on the hovered visual row.
    pub anchor: Bounds<Pixels>,
}

/// Unlike selection hit testing, this does not clamp: the pointer must be over
/// painted text, not the gutter or the space past a line's end, and over a word.
pub fn hover_target_at_position(
    event: EditorSurfacePointerEvent,
    gutter_columns: u16,
    line_cache: &LineLayoutCache,
    document: &Document,
) -> Option<EditorHoverTarget> {
    let geometry = EditorSurfaceGeometry::new(event.bounds, gutter_columns, event.cell_width);
    let text_bounds = geometry.text_bounds();
    if !text_bounds.contains(&event.position) {
        return None;
    }
    let text_area_pos = geometry.window_to_text_area(event.position);
    let line_layout = line_cache.find_line_at_position(
        text_area_pos,
        text_bounds.size.width,
        event.line_height,
    )?;
    let relative_x = text_area_pos.x - line_layout.origin.x;
    if relative_x < px(0.0) || relative_x >= line_layout.shaped_line.width {
        return None;
    }

    let byte_index = grapheme_index_for_x(&line_layout.shaped_line, relative_x);
    let text = document.text();
    let line_text = text.line(line_layout.line_idx).to_string();
    let char_offset = char_offset_for_byte_index(&line_layout, &line_text, byte_index);
    let chars: Vec<char> = line_text.chars().collect();
    if !chars.get(char_offset).copied().is_some_and(char_is_word) {
        return None;
    }

    // Keep the word to this visual row so the anchor matches what is painted.
    let segment_start = line_layout.segment_char_offset;
    let segment_end = segment_start
        + source_chars_in_segment(&line_layout, &chars[segment_start.min(chars.len())..]);
    let mut start = char_offset;
    while start > segment_start && char_is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = char_offset + 1;
    while end < segment_end && char_is_word(chars[end]) {
        end += 1;
    }

    let x_for_char = |offset: usize| {
        let source_byte: usize = chars[segment_start..offset]
            .iter()
            .map(|ch| ch.len_utf8())
            .sum();
        let display_byte = line_layout.display_byte_for_source_byte(source_byte);
        crate::line_cache::grapheme_x_for_index(&line_layout.shaped_line, display_byte)
    };
    let left = text_bounds.origin.x + line_layout.origin.x + x_for_char(start);
    let right = text_bounds.origin.x + line_layout.origin.x + x_for_char(end);
    let top = text_bounds.origin.y + line_layout.origin.y;

    let line_start = text.line_to_char(line_layout.line_idx);
    Some(EditorHoverTarget {
        char_idx: line_start + char_offset,
        word: line_start + start..line_start + end,
        anchor: Bounds {
            origin: point(left, top),
            size: size((right - left).max(px(0.0)), event.line_height),
        },
    })
}

/// Number of source characters a visual row paints, so word ranges stop at a soft wrap.
fn source_chars_in_segment(line_layout: &LineLayout, chars: &[char]) -> usize {
    let source_bytes = line_layout.source_byte_for_display_byte(line_layout.shaped_line.len());
    let mut bytes = 0;
    chars
        .iter()
        .take_while(|ch| {
            let inside = bytes < source_bytes;
            bytes += ch.len_utf8();
            inside
        })
        .count()
}

fn byte_index_for_relative_x(line_layout: &LineLayout, relative_x: Pixels) -> usize {
    if relative_x < px(0.0) {
        0
    } else if relative_x > line_layout.shaped_line.width {
        line_layout.shaped_line.len()
    } else {
        grapheme_index_for_x(&line_layout.shaped_line, relative_x)
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
