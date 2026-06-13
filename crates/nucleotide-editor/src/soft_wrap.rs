// ABOUTME: Soft-wrap viewport collection for native editor rendering
// ABOUTME: Converts Helix formatter graphemes into owned visual-line records

use helix_core::{
    RopeSlice,
    doc_formatter::{DocumentFormatter, FormattedGrapheme, TextFormat},
    graphemes::Grapheme,
    text_annotations::TextAnnotations,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftWrapVisualLine {
    pub visual_line: usize,
    pub doc_line: usize,
    pub text: String,
    pub line_start_col: usize,
    pub wrap_indicator_len: usize,
    pub line_start_char: Option<usize>,
    pub line_end_char: Option<usize>,
    pub segment_char_offset: usize,
    pub text_start_byte_offset: usize,
    pub is_phantom_line: bool,
}

impl SoftWrapVisualLine {
    pub fn relative_row(&self, vertical_offset: usize) -> usize {
        self.visual_line.saturating_sub(vertical_offset)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftWrapVisualPosition {
    pub visual_line: usize,
    pub visual_col: usize,
}

pub fn soft_wrap_visual_lines(
    text: RopeSlice<'_>,
    text_format: &TextFormat,
    anchor: usize,
    vertical_offset: usize,
    viewport_height: usize,
) -> Vec<SoftWrapVisualLine> {
    let annotations = TextAnnotations::default();
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_format, &annotations, anchor);
    let mut visual_line = 0;
    let mut current_doc_line = text.char_to_line(anchor);
    let mut pending_grapheme = None;

    while visual_line < vertical_offset {
        for grapheme in formatter.by_ref() {
            if grapheme.visual_pos.row > visual_line {
                visual_line = grapheme.visual_pos.row;
                if visual_line >= vertical_offset {
                    pending_grapheme = Some(grapheme);
                    break;
                }
            }
        }

        if visual_line < vertical_offset {
            visual_line += 1;
        }
    }

    let mut lines = Vec::new();
    let end_visual_line = vertical_offset.saturating_add(viewport_height);
    while visual_line < end_visual_line {
        let mut line_graphemes = Vec::new();

        if let Some(grapheme) = pending_grapheme.take() {
            if grapheme.visual_pos.row == visual_line {
                line_graphemes.push(grapheme);
            } else if grapheme.visual_pos.row > visual_line {
                pending_grapheme = Some(grapheme);
            }
        }

        let mut has_content = !line_graphemes.is_empty();
        for grapheme in formatter.by_ref() {
            if grapheme.visual_pos.row > visual_line {
                pending_grapheme = Some(grapheme);
                break;
            } else if grapheme.visual_pos.row == visual_line {
                line_graphemes.push(grapheme);
                has_content = true;
            }
        }

        if !has_content && line_graphemes.is_empty() && pending_grapheme.is_none() {
            break;
        }

        let visual = build_visual_line(text, visual_line, current_doc_line, &line_graphemes);
        current_doc_line = visual.doc_line;
        lines.push(visual);

        visual_line += 1;
    }

    lines
}

pub fn soft_wrap_visual_position(
    text: RopeSlice<'_>,
    text_format: &TextFormat,
    anchor: usize,
    char_idx: usize,
) -> Option<SoftWrapVisualPosition> {
    let annotations = TextAnnotations::default();
    let formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_format, &annotations, anchor);
    let mut last_visual_line = 0;

    for grapheme in formatter {
        let next_char_pos = grapheme.char_idx + grapheme.doc_chars();
        if next_char_pos > char_idx {
            return Some(SoftWrapVisualPosition {
                visual_line: grapheme.visual_pos.row,
                visual_col: grapheme.visual_pos.col,
            });
        }
        last_visual_line = grapheme.visual_pos.row;
    }

    (char_idx >= text.len_chars()).then_some(SoftWrapVisualPosition {
        visual_line: last_visual_line,
        visual_col: 0,
    })
}

fn build_visual_line(
    text: RopeSlice<'_>,
    visual_line: usize,
    fallback_doc_line: usize,
    line_graphemes: &[FormattedGrapheme<'_>],
) -> SoftWrapVisualLine {
    let doc_line = line_graphemes
        .first()
        .map_or(fallback_doc_line, |grapheme| grapheme.line_idx);
    let line_start_col = line_graphemes
        .first()
        .map_or(0, |grapheme| grapheme.visual_pos.col);
    let mut line_text = String::new();
    let mut wrap_indicator_len = 0;

    line_text.extend(std::iter::repeat_n(' ', line_start_col));

    for grapheme in line_graphemes {
        if grapheme.is_virtual() {
            if let Grapheme::Other { g } = &grapheme.raw {
                wrap_indicator_len += g.len();
                line_text.push_str(g);
            }
        } else {
            match &grapheme.raw {
                Grapheme::Tab { .. } => line_text.push('\t'),
                Grapheme::Other { g } => line_text.push_str(g),
                Grapheme::Newline => {}
            }
        }
    }

    let first_real = line_graphemes
        .iter()
        .find(|grapheme| !grapheme.is_virtual());
    let line_start_char = first_real.map(|grapheme| grapheme.char_idx);
    let line_end_char = first_real.map(|first_grapheme| {
        let last_real = line_graphemes
            .iter()
            .rev()
            .find(|grapheme| !grapheme.is_virtual());
        let mut line_end = last_real
            .map(|grapheme| grapheme.char_idx + grapheme.doc_chars())
            .unwrap_or(first_grapheme.char_idx);

        if let Some(last_grapheme) = last_real
            && matches!(last_grapheme.raw, Grapheme::Newline)
        {
            line_end = last_grapheme.char_idx;
        }

        line_end
    });

    let line_start = text.line_to_char(doc_line);
    let line_end = if doc_line + 1 < text.len_lines() {
        text.line_to_char(doc_line + 1)
    } else {
        text.len_chars()
    };
    let segment_char_offset =
        first_real.map_or(0, |grapheme| grapheme.char_idx.saturating_sub(line_start));

    SoftWrapVisualLine {
        visual_line,
        doc_line,
        text: line_text,
        line_start_col,
        wrap_indicator_len,
        line_start_char,
        line_end_char,
        segment_char_offset,
        text_start_byte_offset: line_start_col + wrap_indicator_len,
        is_phantom_line: line_start >= line_end,
    }
}

#[cfg(test)]
mod tests {
    use super::{soft_wrap_visual_lines, soft_wrap_visual_position};
    use helix_core::doc_formatter::TextFormat;

    fn text_format() -> TextFormat {
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

    #[test]
    fn collects_wrapped_visual_lines_with_metadata() {
        let text = "foo ".repeat(10);
        let lines = soft_wrap_visual_lines(text.as_str().into(), &text_format(), 0, 0, 3);

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].visual_line, 0);
        assert_eq!(lines[0].doc_line, 0);
        assert_eq!(lines[0].text, "foo foo foo foo ");
        assert_eq!(lines[0].line_start_char, Some(0));
        assert_eq!(lines[0].line_end_char, Some(16));
        assert_eq!(lines[0].wrap_indicator_len, 0);
        assert_eq!(lines[0].segment_char_offset, 0);
        assert_eq!(lines[0].text_start_byte_offset, 0);

        assert_eq!(lines[1].visual_line, 1);
        assert_eq!(lines[1].doc_line, 0);
        assert_eq!(lines[1].text, ".foo foo foo foo ");
        assert_eq!(lines[1].line_start_char, Some(16));
        assert_eq!(lines[1].line_end_char, Some(32));
        assert_eq!(lines[1].wrap_indicator_len, 1);
        assert_eq!(lines[1].segment_char_offset, 16);
        assert_eq!(lines[1].text_start_byte_offset, 1);

        assert_eq!(lines[2].visual_line, 2);
        assert_eq!(lines[2].text, ".foo foo  ");
        assert_eq!(lines[2].line_start_char, Some(32));
        assert_eq!(lines[2].line_end_char, Some(40));
        assert_eq!(lines[2].segment_char_offset, 32);
    }

    #[test]
    fn starts_at_viewport_vertical_offset() {
        let text = "foo ".repeat(10);
        let lines = soft_wrap_visual_lines(text.as_str().into(), &text_format(), 0, 1, 1);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].visual_line, 1);
        assert_eq!(lines[0].relative_row(1), 0);
        assert_eq!(lines[0].text, ".foo foo foo foo ");
    }

    #[test]
    fn finds_visual_position_after_wrap_indicator() {
        let text = "foo ".repeat(10);
        let position = soft_wrap_visual_position(text.as_str().into(), &text_format(), 0, 16)
            .expect("cursor position");

        assert_eq!(position.visual_line, 1);
        assert_eq!(position.visual_col, 1);
    }
}
