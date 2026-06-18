// ABOUTME: Text normalization helpers for native editor line rendering
// ABOUTME: Prepares Rope line slices for GPUI shaping and byte-index lookups

use gpui::{SharedString, TextRun};
use helix_core::{
    RopeSlice,
    graphemes::{grapheme_width, tab_width_at},
};

pub fn line_text_without_trailing_newline(line: RopeSlice<'_>) -> String {
    let mut line_text = line.to_string();
    while line_text.ends_with('\n') || line_text.ends_with('\r') {
        line_text.pop();
    }
    line_text
}

pub fn shared_line_text_without_trailing_newline(line: RopeSlice<'_>) -> SharedString {
    SharedString::from(line_text_without_trailing_newline(line))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayTextMap {
    source_to_display: Vec<usize>,
    display_to_source: Vec<usize>,
}

impl Default for DisplayTextMap {
    fn default() -> Self {
        Self::identity(0)
    }
}

impl DisplayTextMap {
    pub fn identity(byte_len: usize) -> Self {
        let offsets = (0..=byte_len).collect::<Vec<_>>();
        Self {
            source_to_display: offsets.clone(),
            display_to_source: offsets,
        }
    }

    pub fn display_byte_for_source_byte(&self, source_byte: usize) -> usize {
        let last = self.source_to_display.len().saturating_sub(1);
        self.source_to_display[source_byte.min(last)]
    }

    pub fn source_byte_for_display_byte(&self, display_byte: usize) -> usize {
        let last = self.display_to_source.len().saturating_sub(1);
        self.display_to_source[display_byte.min(last)]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLineText {
    pub source: String,
    pub display: SharedString,
    pub map: DisplayTextMap,
}

impl DisplayLineText {
    pub fn from_source(source: String, tab_width: u16) -> Self {
        Self::from_source_with_prefix("", source, 0, tab_width)
    }

    pub fn from_source_with_prefix(
        prefix: &str,
        source: String,
        initial_col: usize,
        tab_width: u16,
    ) -> Self {
        let mut display = String::with_capacity(prefix.len() + source.len());
        display.push_str(prefix);

        let prefix_len = display.len();
        let mut source_to_display = vec![prefix_len; source.len() + 1];
        let mut spans = Vec::new();
        if prefix_len > 0 {
            spans.push((0, prefix_len, 0, 0));
        }

        let mut visual_col = initial_col;
        for (source_start, ch) in source.char_indices() {
            let source_end = source_start + ch.len_utf8();
            let display_start = display.len();

            if ch == '\t' {
                let tab_width = tab_width_at(visual_col, tab_width);
                display.extend(std::iter::repeat_n(' ', tab_width));
                visual_col += tab_width;
            } else {
                display.push(ch);
                visual_col += char_display_width(ch);
            }

            let display_end = display.len();
            for source_byte in source_start..source_end {
                source_to_display[source_byte] = display_start;
            }
            source_to_display[source_end] = display_end;
            spans.push((display_start, display_end, source_start, source_end));
        }
        source_to_display[source.len()] = display.len();

        let mut display_to_source = vec![source.len(); display.len() + 1];
        for (display_start, display_end, source_start, source_end) in spans {
            for display_byte in display_start..display_end {
                display_to_source[display_byte] = source_start;
            }
            display_to_source[display_end] = source_end;
        }
        if display.is_empty() {
            display_to_source[0] = 0;
        }

        Self {
            source,
            display: display.into(),
            map: DisplayTextMap {
                source_to_display,
                display_to_source,
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.display.is_empty()
    }
}

pub fn expand_text_runs_for_display(
    runs: &[TextRun],
    display_map: &DisplayTextMap,
) -> Vec<TextRun> {
    let mut source_byte: usize = 0;
    let mut display_runs = Vec::with_capacity(runs.len());

    for run in runs {
        let source_run_start = source_byte;
        let source_run_end = source_run_start.saturating_add(run.len);
        source_byte = source_run_end;

        let display_start = display_map.display_byte_for_source_byte(source_run_start);
        let display_end = display_map.display_byte_for_source_byte(source_run_end);
        let display_len = display_end.saturating_sub(display_start);
        if display_len == 0 {
            continue;
        }

        let mut display_run = run.clone();
        display_run.len = display_len;
        display_runs.push(display_run);
    }

    display_runs
}

pub fn visual_columns_for_text(text: &str, initial_col: usize, tab_width: u16) -> usize {
    text.chars().fold(initial_col, |visual_col, ch| {
        if ch == '\t' {
            visual_col + tab_width_at(visual_col, tab_width)
        } else {
            visual_col + char_display_width(ch)
        }
    })
}

pub fn byte_offset_for_char_offset(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map_or(text.len(), |(byte_idx, _)| byte_idx)
}

fn char_display_width(ch: char) -> usize {
    let mut buf = [0; 4];
    grapheme_width(ch.encode_utf8(&mut buf))
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayLineText, DisplayTextMap, byte_offset_for_char_offset,
        line_text_without_trailing_newline, shared_line_text_without_trailing_newline,
        visual_columns_for_text,
    };
    use gpui::{TextRun, black, font};

    #[test]
    fn strips_lf() {
        assert_eq!(line_text_without_trailing_newline("abc\n".into()), "abc");
    }

    #[test]
    fn strips_crlf() {
        assert_eq!(line_text_without_trailing_newline("abc\r\n".into()), "abc");
    }

    #[test]
    fn preserves_interior_newlines() {
        assert_eq!(
            line_text_without_trailing_newline("abc\ndef".into()),
            "abc\ndef"
        );
    }

    #[test]
    fn converts_to_shared_string() {
        assert_eq!(
            shared_line_text_without_trailing_newline("abc\n".into()).to_string(),
            "abc"
        );
    }

    #[test]
    fn converts_char_offsets_to_byte_offsets() {
        assert_eq!(byte_offset_for_char_offset("aé𝌆z", 3), "aé𝌆".len());
        assert_eq!(byte_offset_for_char_offset("abc", 99), 3);
    }

    #[test]
    fn expands_tabs_to_spaces_at_visual_tab_stops() {
        let text = DisplayLineText::from_source("a\tb".to_string(), 4);

        assert_eq!(text.source, "a\tb");
        assert_eq!(text.display.to_string(), "a   b");
        assert_eq!(text.map.display_byte_for_source_byte(0), 0);
        assert_eq!(text.map.display_byte_for_source_byte(1), 1);
        assert_eq!(text.map.display_byte_for_source_byte(2), 4);
        assert_eq!(text.map.display_byte_for_source_byte(3), 5);
        assert_eq!(text.map.source_byte_for_display_byte(1), 1);
        assert_eq!(text.map.source_byte_for_display_byte(3), 1);
        assert_eq!(text.map.source_byte_for_display_byte(4), 2);
    }

    #[test]
    fn expands_tabs_after_virtual_prefix() {
        let text = DisplayLineText::from_source_with_prefix("..", "\tx".to_string(), 2, 4);

        assert_eq!(text.display.to_string(), "..  x");
        assert_eq!(text.map.display_byte_for_source_byte(0), 2);
        assert_eq!(text.map.display_byte_for_source_byte(1), 4);
        assert_eq!(text.map.source_byte_for_display_byte(0), 0);
        assert_eq!(text.map.source_byte_for_display_byte(3), 0);
        assert_eq!(text.map.source_byte_for_display_byte(4), 1);
    }

    #[test]
    fn maps_text_run_lengths_to_expanded_display_bytes() {
        let text = DisplayLineText::from_source("a\tbc".to_string(), 4);
        let runs = vec![run(2), run(2)];
        let display_runs = super::expand_text_runs_for_display(&runs, &text.map);

        assert_eq!(
            display_runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            vec![4, 2]
        );
    }

    #[test]
    fn identity_map_preserves_offsets() {
        let map = DisplayTextMap::identity(3);

        assert_eq!(map.display_byte_for_source_byte(2), 2);
        assert_eq!(map.source_byte_for_display_byte(4), 3);
    }

    #[test]
    fn computes_visual_columns_with_tab_stops() {
        assert_eq!(visual_columns_for_text("\t", 0, 4), 4);
        assert_eq!(visual_columns_for_text("a\t", 0, 4), 4);
        assert_eq!(visual_columns_for_text("abcd\t", 0, 4), 8);
    }

    fn run(len: usize) -> TextRun {
        TextRun {
            len,
            font: font("TestFont"),
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }
}
