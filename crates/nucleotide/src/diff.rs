//! Diff rows shared by the inline peek and Commit review.
use gpui::prelude::FluentBuilder;
use gpui::{Hsla, IntoElement, ParentElement, Pixels, SharedString, Styled, div, px};
use std::ops::Range;

pub(crate) fn shared_affixes(old: &str, new: &str) -> (usize, usize) {
    let old: Vec<char> = old.chars().collect();
    let new: Vec<char> = new.chars().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(old.len().min(new.len()) - prefix)
        .take_while(|(a, b)| a == b)
        .count();
    (prefix, suffix)
}

pub(crate) fn expand_tabs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for ch in line.chars() {
        if ch == '\t' {
            let width = 4 - out.chars().count() % 4;
            out.extend(std::iter::repeat_n(' ', width));
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(Clone, PartialEq)]
pub(crate) struct DiffStyle {
    pub font_family: SharedString,
    pub font_size: Pixels,
    pub surface: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub added: Hsla,
    pub removed: Hsla,
    /// Width of the column holding each changed line's + or − sign.
    pub sign_width: Pixels,
}

pub(crate) struct DiffLine {
    sign: char,
    text: String,
    changed: Option<Range<usize>>,
    color: Hsla,
    sign_width: Pixels,
}

pub(crate) fn diff_lines(removed: &[String], added: &[String], style: &DiffStyle) -> Vec<DiffLine> {
    let removed: Vec<String> = removed.iter().map(|line| expand_tabs(line)).collect();
    let added: Vec<String> = added.iter().map(|line| expand_tabs(line)).collect();
    let paired = removed.len().min(added.len());
    let changed = |index: usize, old: bool| -> Option<Range<usize>> {
        if index >= paired {
            return None;
        }
        let (prefix, suffix) = shared_affixes(&removed[index], &added[index]);
        let len = if old { &removed[index] } else { &added[index] }
            .chars()
            .count();
        let range = prefix..len - suffix;
        (!range.is_empty()).then_some(range)
    };
    let removed_lines = removed.iter().enumerate().map(|(index, text)| DiffLine {
        sign: '−',
        text: text.clone(),
        changed: changed(index, true),
        color: style.removed,
        sign_width: style.sign_width,
    });
    let added_lines = added.iter().enumerate().map(|(index, text)| DiffLine {
        sign: '+',
        text: text.clone(),
        changed: changed(index, false),
        color: style.added,
        sign_width: style.sign_width,
    });
    removed_lines.chain(added_lines).collect()
}

pub(crate) fn render_diff_line(line: DiffLine, line_height: Pixels) -> impl IntoElement {
    let chars: Vec<char> = line.text.chars().collect();
    let segment = |range: Range<usize>| chars[range].iter().collect::<String>();
    let (before, changed, after) = match &line.changed {
        Some(range) => (
            segment(0..range.start),
            Some(segment(range.clone())),
            segment(range.end..chars.len()),
        ),
        None => (line.text.clone(), None, String::new()),
    };
    div()
        .flex()
        .flex_row()
        .flex_none()
        .h(line_height)
        .items_center()
        .whitespace_nowrap()
        .bg(line.color.opacity(0.12))
        .child(
            div()
                .flex_none()
                .w(line.sign_width)
                .text_center()
                .text_color(line.color)
                .child(line.sign.to_string()),
        )
        .child(before)
        .when_some(changed, |row, changed| {
            row.child(
                div()
                    .rounded(px(3.0))
                    .bg(line.color.opacity(0.32))
                    .child(changed),
            )
        })
        .child(after)
}
