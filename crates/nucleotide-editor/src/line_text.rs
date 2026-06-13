// ABOUTME: Text normalization helpers for native editor line rendering
// ABOUTME: Prepares Rope line slices for GPUI shaping and byte-index lookups

use gpui::SharedString;
use helix_core::RopeSlice;

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

pub fn byte_offset_for_char_offset(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map_or(text.len(), |(byte_idx, _)| byte_idx)
}

#[cfg(test)]
mod tests {
    use super::{
        byte_offset_for_char_offset, line_text_without_trailing_newline,
        shared_line_text_without_trailing_newline,
    };

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
}
