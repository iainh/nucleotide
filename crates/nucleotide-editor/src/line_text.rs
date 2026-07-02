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
    pub whitespace_ranges: Vec<VirtualTextRange<()>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualTextRange<T> {
    pub display_start: usize,
    pub display_len: usize,
    pub metadata: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayWhitespace {
    pub space: Option<char>,
    pub nbsp: Option<char>,
    pub nnbsp: Option<char>,
    pub tab: Option<(char, char)>,
}

#[derive(Debug, Clone)]
pub struct DisplayLineTextBuilder<T> {
    source: String,
    display: String,
    source_to_display: Vec<usize>,
    display_to_source: Vec<usize>,
    visual_col: usize,
    virtual_ranges: Vec<VirtualTextRange<T>>,
    whitespace_ranges: Vec<VirtualTextRange<()>>,
    whitespace: Option<DisplayWhitespace>,
}

impl<T: PartialEq> DisplayLineTextBuilder<T> {
    pub fn new(initial_col: usize) -> Self {
        Self {
            source: String::new(),
            display: String::new(),
            source_to_display: vec![0],
            display_to_source: vec![0],
            visual_col: initial_col,
            virtual_ranges: Vec::new(),
            whitespace_ranges: Vec::new(),
            whitespace: None,
        }
    }

    pub fn with_whitespace(initial_col: usize, whitespace: Option<DisplayWhitespace>) -> Self {
        Self {
            whitespace,
            ..Self::new(initial_col)
        }
    }

    pub fn push_virtual(&mut self, text: &str, metadata: T, tab_width: u16) {
        let display_start = self.display.len();
        for ch in text.chars() {
            self.push_display_char(ch, tab_width, self.source.len());
        }
        let display_len = self.display.len().saturating_sub(display_start);
        if display_len > 0 {
            if let Some(last) = self.virtual_ranges.last_mut()
                && last.display_start + last.display_len == display_start
                && last.metadata == metadata
            {
                last.display_len += display_len;
            } else {
                self.virtual_ranges.push(VirtualTextRange {
                    display_start,
                    display_len,
                    metadata,
                });
            }
        }
    }

    pub fn push_prefix(&mut self, text: &str, tab_width: u16) {
        for ch in text.chars() {
            self.push_display_char(ch, tab_width, 0);
        }
        self.source_to_display[0] = self.display.len();
    }

    pub fn push_source_char(&mut self, ch: char, tab_width: u16) {
        let source_start = self.source.len();
        self.source.push(ch);
        let source_end = self.source.len();
        let display_start = self.display.len();

        self.push_display_char(ch, tab_width, source_start);

        let display_end = self.display.len();
        self.display_to_source[display_end] = source_end;
        self.source_to_display.resize(source_end + 1, display_start);
        self.source_to_display[source_start..source_end].fill(display_start);
        self.source_to_display[source_end] = display_end;
    }

    pub fn finish(self) -> (DisplayLineText, Vec<VirtualTextRange<T>>) {
        let display_to_source = if self.display_to_source.is_empty() {
            vec![0]
        } else {
            self.display_to_source
        };

        (
            DisplayLineText {
                source: self.source,
                display: self.display.into(),
                map: DisplayTextMap {
                    source_to_display: self.source_to_display,
                    display_to_source,
                },
                whitespace_ranges: self.whitespace_ranges,
            },
            self.virtual_ranges,
        )
    }

    fn push_display_char(&mut self, ch: char, tab_width: u16, source_byte: usize) {
        let display_start = self.display.len();
        if ch == '\t' {
            let tab_width = tab_width_at(self.visual_col, tab_width);
            if let Some((tab, tabpad)) = self.whitespace.and_then(|ws| ws.tab) {
                self.display.push(tab);
                self.display
                    .extend(std::iter::repeat_n(tabpad, tab_width.saturating_sub(1)));
            } else {
                self.display.extend(std::iter::repeat_n(' ', tab_width));
            }
            self.display_to_source
                .resize(self.display.len() + 1, source_byte);
            self.visual_col += tab_width;
        } else {
            self.display
                .push(visible_whitespace_char(ch, self.whitespace));
            self.display_to_source
                .resize(self.display.len() + 1, source_byte);
            self.visual_col += char_display_width(ch);
        }
        if should_style_whitespace(ch, self.whitespace) {
            self.whitespace_ranges.push(VirtualTextRange {
                display_start,
                display_len: self.display.len().saturating_sub(display_start),
                metadata: (),
            });
        }
        if let Some(last) = self.display_to_source.last_mut() {
            *last = source_byte;
        }
    }
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
        Self::from_source_with_prefix_and_whitespace(prefix, source, initial_col, tab_width, None)
    }

    pub fn from_source_with_whitespace(
        source: String,
        tab_width: u16,
        whitespace: Option<DisplayWhitespace>,
    ) -> Self {
        Self::from_source_with_prefix_and_whitespace("", source, 0, tab_width, whitespace)
    }

    pub fn from_source_with_prefix_and_whitespace(
        prefix: &str,
        source: String,
        initial_col: usize,
        tab_width: u16,
        whitespace: Option<DisplayWhitespace>,
    ) -> Self {
        let mut display = String::with_capacity(prefix.len() + source.len());
        display.push_str(prefix);

        let prefix_len = display.len();
        let mut source_to_display = vec![prefix_len; source.len() + 1];
        let mut spans = Vec::new();
        let mut whitespace_ranges = Vec::new();
        if prefix_len > 0 {
            spans.push((0, prefix_len, 0, 0));
        }

        let mut visual_col = initial_col;
        for (source_start, ch) in source.char_indices() {
            let source_end = source_start + ch.len_utf8();
            let display_start = display.len();

            if ch == '\t' {
                let tab_width = tab_width_at(visual_col, tab_width);
                if let Some((tab, tabpad)) = whitespace.and_then(|ws| ws.tab) {
                    display.push(tab);
                    display.extend(std::iter::repeat_n(tabpad, tab_width.saturating_sub(1)));
                } else {
                    display.extend(std::iter::repeat_n(' ', tab_width));
                }
                visual_col += tab_width;
            } else {
                display.push(visible_whitespace_char(ch, whitespace));
                visual_col += char_display_width(ch);
            }

            let display_end = display.len();
            if should_style_whitespace(ch, whitespace) {
                whitespace_ranges.push(VirtualTextRange {
                    display_start,
                    display_len: display_end.saturating_sub(display_start),
                    metadata: (),
                });
            }
            source_to_display[source_start..source_end].fill(display_start);
            source_to_display[source_end] = display_end;
            spans.push((display_start, display_end, source_start, source_end));
        }
        source_to_display[source.len()] = display.len();

        let mut display_to_source = vec![source.len(); display.len() + 1];
        for (display_start, display_end, source_start, source_end) in spans {
            display_to_source[display_start..display_end].fill(source_start);
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
            whitespace_ranges,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.display.is_empty()
    }
}

fn visible_whitespace_char(ch: char, whitespace: Option<DisplayWhitespace>) -> char {
    match ch {
        ' ' => whitespace.and_then(|ws| ws.space).unwrap_or(ch),
        '\u{00A0}' => whitespace.and_then(|ws| ws.nbsp).unwrap_or(ch),
        '\u{202F}' => whitespace.and_then(|ws| ws.nnbsp).unwrap_or(ch),
        _ => ch,
    }
}

fn should_style_whitespace(ch: char, whitespace: Option<DisplayWhitespace>) -> bool {
    match ch {
        '\t' => whitespace.and_then(|ws| ws.tab).is_some(),
        ' ' => whitespace.and_then(|ws| ws.space).is_some(),
        '\u{00A0}' => whitespace.and_then(|ws| ws.nbsp).is_some(),
        '\u{202F}' => whitespace.and_then(|ws| ws.nnbsp).is_some(),
        _ => false,
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

pub fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut boundary = index.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

pub fn text_run_boundaries(text: &str, boundaries: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut normalized = Vec::new();
    normalized.push(0);
    normalized.push(text.len());
    normalized.extend(
        boundaries
            .into_iter()
            .map(|boundary| floor_char_boundary(text, boundary)),
    );
    normalized.sort_unstable();
    normalized.dedup();
    normalized
}

fn char_display_width(ch: char) -> usize {
    let mut buf = [0; 4];
    grapheme_width(ch.encode_utf8(&mut buf))
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayLineText, DisplayLineTextBuilder, DisplayTextMap, DisplayWhitespace,
        byte_offset_for_char_offset, floor_char_boundary, line_text_without_trailing_newline,
        shared_line_text_without_trailing_newline, text_run_boundaries, visual_columns_for_text,
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
    fn floors_byte_offsets_to_utf8_boundaries() {
        assert_eq!(floor_char_boundary("↪abc", 2), 0);
        assert_eq!(floor_char_boundary("↪abc", "↪".len()), "↪".len());
    }

    #[test]
    fn text_run_boundaries_never_split_utf8_characters() {
        let text = "↪abc";
        let boundaries = text_run_boundaries(text, [2, "↪".len(), "↪a".len()]);

        assert_eq!(boundaries, vec![0, "↪".len(), "↪a".len(), text.len()]);
        assert!(boundaries.iter().all(|index| text.is_char_boundary(*index)));
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
    fn builder_preserves_virtual_text_in_display() {
        let mut builder = DisplayLineTextBuilder::new(0);
        builder.push_source_char('a', 4);
        builder.push_virtual(": hint", Some("hint"), 4);
        builder.push_source_char('b', 4);

        let (text, virtual_ranges) = builder.finish();

        assert_eq!(text.source, "ab");
        assert_eq!(text.display.to_string(), "a: hintb");
        assert_eq!(virtual_ranges.len(), 1);
        assert_eq!(virtual_ranges[0].display_start, 1);
        assert_eq!(virtual_ranges[0].display_len, ": hint".len());
        assert_eq!(virtual_ranges[0].metadata, Some("hint"));
        assert_eq!(text.map.display_byte_for_source_byte(0), 0);
        assert_eq!(text.map.display_byte_for_source_byte(1), "a: hint".len());
        assert_eq!(text.map.display_byte_for_source_byte(2), "a: hintb".len());
    }

    #[test]
    fn renders_configured_visible_whitespace() {
        let text = DisplayLineText::from_source_with_whitespace(
            " \t\u{00a0}".to_string(),
            4,
            Some(DisplayWhitespace {
                space: Some('·'),
                nbsp: Some('⍽'),
                nnbsp: None,
                tab: Some(('→', '·')),
            }),
        );

        assert_eq!(text.display.to_string(), "·→··⍽");
        assert_eq!(
            text.whitespace_ranges
                .iter()
                .map(|range| (range.display_start, range.display_len))
                .collect::<Vec<_>>(),
            vec![
                (0, "·".len()),
                ("·".len(), "→··".len()),
                ("·→··".len(), "⍽".len())
            ]
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
