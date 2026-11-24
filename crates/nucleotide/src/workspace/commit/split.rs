//! Side-by-side diff in the style of IntelliJ: each version keeps its own line
//! layout, and ribbons in a shared gutter connect the blocks that changed.
use super::view::{MAX_VISIBLE_LINES, line_text, row_height};
use super::*;
use crate::diff::DiffStyle;
use gpui::{App, HighlightStyle, Hsla, PathBuilder, StyledText, canvas, point, size};
use helix_core::syntax::{Highlight, Loader};
use imara_diff::{Algorithm, Diff, InternedInput};
use nucleotide_types::scrollbar::SCROLLBAR_THICKNESS;
use nucleotide_ui::theme_manager::HelixThemedContext;
use nucleotide_vcs::staging::StagingHunk;
use std::cell::OnceCell;
use std::ops::Range;

/// Unchanged lines shown around each change. Longer unchanged runs fold.
const CONTEXT: u32 = 3;
/// A fold replaces a row with its label, so it must hide at least two lines.
const MIN_FOLD: u32 = 2;
/// Files larger than this are shown without syntax highlighting.
const MAX_HIGHLIGHT_BYTES: usize = 1024 * 1024;
const LINE_TINT: f32 = 0.16;
const WORD_TINT: f32 = 0.4;
const MARKER_TINT: f32 = 0.7;

/// Syntax highlights of each line, by byte range within the line.
pub(super) type LineHighlights = Vec<Vec<(Range<usize>, Vec<Highlight>)>>;

pub(super) fn line_highlights(text: &[u8], path: &Path, loader: &Loader) -> Option<LineHighlights> {
    if text.len() > MAX_HIGHLIGHT_BYTES {
        return None;
    }
    let text = std::str::from_utf8(text).ok()?;
    let spans = crate::overlay::picker_preview_highlights(text, Some(path), loader)?;
    let mut lines = Vec::new();
    let (mut start, mut first) = (0, 0);
    for line in text.split_inclusive('\n') {
        let end = start + line.len();
        while spans
            .get(first)
            .is_some_and(|(range, _)| range.end <= start)
        {
            first += 1;
        }
        // Spans such as block comments continue across lines.
        lines.push(
            spans[first..]
                .iter()
                .take_while(|(range, _)| range.start < end)
                .map(|(range, stack)| {
                    (
                        range.start.max(start) - start..range.end.min(end) - start,
                        stack.clone(),
                    )
                })
                .collect(),
        );
        start = end;
    }
    Some(lines)
}

/// Hunks shown together with their context, and the HEAD and new lines they span.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Region {
    pub old: Range<u32>,
    pub new: Range<u32>,
    pub hunks: Range<usize>,
}

/// Groups hunks with their context. An unchanged run that does not fit in the
/// context folds unless `expanded` holds the HEAD line where its hidden part starts.
pub(super) fn regions(
    hunks: &[StagingHunk],
    old_len: u32,
    expanded: impl Fn(u32) -> bool,
) -> Vec<Region> {
    let mut regions: Vec<Region> = Vec::new();
    // Unchanged lines line up one to one, so gaps have the same length on both sides.
    let mut previous = (0, 0);
    for (index, hunk) in hunks.iter().enumerate() {
        let gap = hunk.before.start - previous.0;
        let shown = match regions.last_mut() {
            None if gap < CONTEXT + MIN_FOLD || expanded(0) => gap,
            None => CONTEXT,
            Some(region) if gap < 2 * CONTEXT + MIN_FOLD || expanded(previous.0 + CONTEXT) => {
                region.hunks.end = index + 1;
                previous = (hunk.before.end, hunk.after.end);
                continue;
            }
            Some(region) => {
                region.old.end = previous.0 + CONTEXT;
                region.new.end = previous.1 + CONTEXT;
                CONTEXT
            }
        };
        regions.push(Region {
            old: hunk.before.start - shown..0,
            new: hunk.after.start - shown..0,
            hunks: index..index + 1,
        });
        previous = (hunk.before.end, hunk.after.end);
    }
    if let Some(region) = regions.last_mut() {
        let gap = old_len - previous.0;
        let shown = if gap < CONTEXT + MIN_FOLD || expanded(previous.0 + CONTEXT) {
            gap
        } else {
            CONTEXT
        };
        region.old.end = previous.0 + shown;
        region.new.end = previous.1 + shown;
    }
    regions
}

/// How a word changed between two versions of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edit {
    Inserted,
    Deleted,
    Changed,
}

/// Changed byte ranges of each line.
pub(super) type LineEdits = Vec<Vec<(Range<usize>, Edit)>>;

struct Token<'a> {
    line: usize,
    range: Range<usize>,
    text: &'a str,
}

/// Words, whitespace runs and single punctuation characters. Line breaks are
/// tokens too, so the diff can match words across lines of a block.
fn tokenize<'a>(lines: &[&'a str]) -> Vec<Token<'a>> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let mut tokens = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        if line > 0 {
            tokens.push(Token {
                line,
                range: 0..0,
                text: "\n",
            });
        }
        let mut chars = text.char_indices().peekable();
        while let Some((start, ch)) = chars.next() {
            let mut end = start + ch.len_utf8();
            while class(ch) != 2
                && let Some(&(index, next)) = chars.peek()
                && class(next) == class(ch)
            {
                end = index + next.len_utf8();
                chars.next();
            }
            tokens.push(Token {
                line,
                range: start..end,
                text: &text[start..end],
            });
        }
    }
    tokens
}

/// The words that differ between the HEAD and new lines of one changed block.
pub(super) fn word_edits(old: &[&str], new: &[&str]) -> [LineEdits; 2] {
    let tokens = [tokenize(old), tokenize(new)];
    let mut input = InternedInput::default();
    input.update_before(tokens[0].iter().map(|token| token.text));
    input.update_after(tokens[1].iter().map(|token| token.text));
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_no_heuristic(&input);
    let mut edits = [vec![Vec::new(); old.len()], vec![Vec::new(); new.len()]];
    for hunk in diff.hunks() {
        let edit = if hunk.before.is_empty() {
            Edit::Inserted
        } else if hunk.after.is_empty() {
            Edit::Deleted
        } else {
            Edit::Changed
        };
        for (side, range) in [hunk.before, hunk.after].into_iter().enumerate() {
            for token in &tokens[side][range.start as usize..range.end as usize] {
                let line: &mut Vec<(Range<usize>, Edit)> = &mut edits[side][token.line];
                match line.last_mut() {
                    _ if token.range.is_empty() => {}
                    Some((last, kind)) if last.end == token.range.start && *kind == edit => {
                        last.end = token.range.end
                    }
                    _ => line.push((token.range.clone(), edit)),
                }
            }
        }
    }
    edits
}

/// Expands tabs and maps each byte offset of `line` to its offset in the result.
fn expand_tabs_mapped(line: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(line.len());
    let mut map = vec![0; line.len() + 1];
    let mut column = 0;
    for (index, ch) in line.char_indices() {
        map[index] = out.len();
        if ch == '\t' {
            let width = 4 - column % 4;
            out.extend(std::iter::repeat_n(' ', width));
            column += width;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    map[line.len()] = out.len();
    (out, map)
}

#[derive(Clone, Copy)]
struct Colors {
    added: Hsla,
    modified: Hsla,
    deleted: Hsla,
}

impl Colors {
    fn of(&self, edit: Edit) -> Hsla {
        match edit {
            Edit::Inserted => self.added,
            Edit::Deleted => self.deleted,
            Edit::Changed => self.modified,
        }
    }
}

fn code_text(
    cx: &App,
    line: &str,
    highlights: Option<&[(Range<usize>, Vec<Highlight>)]>,
    edits: &[(Range<usize>, Edit)],
    colors: Colors,
) -> StyledText {
    let (text, map) = expand_tabs_mapped(line);
    let at = |offset: usize| map[offset.min(line.len())];
    let theme = cx.helix_theme();
    let syntax: Vec<_> = highlights
        .unwrap_or_default()
        .iter()
        .filter_map(|(range, stack)| {
            let color = stack.iter().rev().find_map(|highlight| {
                theme
                    .highlight(*highlight)
                    .fg
                    .and_then(nucleotide_ui::theme_utils::color_to_hsla)
            })?;
            Some((at(range.start)..at(range.end), HighlightStyle::color(color)))
        })
        .collect();
    let words = edits.iter().map(|(range, edit)| {
        (
            at(range.start)..at(range.end),
            HighlightStyle {
                background_color: Some(colors.of(*edit).opacity(WORD_TINT)),
                ..Default::default()
            },
        )
    });
    StyledText::new(text).with_highlights(gpui::combine_highlights(syntax, words))
}

/// One version's code rows and the matching gutter rows.
#[derive(Default)]
struct Side {
    code: Vec<gpui::AnyElement>,
    gutter: Vec<gpui::AnyElement>,
    /// Rows above which the other version inserted or removed lines.
    markers: Vec<(usize, Hsla)>,
}

/// Drag payload of a side-by-side diff's divider, naming the diff it belongs to.
struct SplitHandle(String);

/// Neither side of a side-by-side diff can be dragged narrower than this share.
const MIN_SPLIT: f32 = 0.15;

struct Block {
    rows: [Range<usize>; 2],
    color: Hsla,
    checkbox: Option<gpui::AnyElement>,
}

/// A part of a diff, rendered as its own list item so that only the parts
/// on screen are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiffPart {
    /// The side-by-side column titles.
    Titles,
    /// A side-by-side region and the fold before it, which starts at
    /// `shown_from`. `visible` changed lines are shown before it.
    Region {
        index: usize,
        shown_from: u32,
        visible: usize,
    },
    /// The fold after the last side-by-side region.
    Tail { from: u32 },
    /// A unified hunk, with `visible` changed lines shown before it.
    Hunk { index: usize, visible: usize },
    /// The notice that the preview stops at `MAX_VISIBLE_LINES`.
    Limited,
}

/// The unified parts of `changes`: one per hunk until the line limit.
pub(super) fn unified_parts(changes: &FileChanges) -> Vec<DiffPart> {
    let mut parts = Vec::new();
    let mut visible = 0;
    for (index, hunk) in changes.hunks().iter().enumerate() {
        if visible >= MAX_VISIBLE_LINES {
            parts.push(DiffPart::Limited);
            return parts;
        }
        parts.push(DiffPart::Hunk { index, visible });
        visible += hunk.before.len() + hunk.after.len();
    }
    if visible > MAX_VISIBLE_LINES {
        parts.push(DiffPart::Limited);
    }
    parts
}

/// Display columns of a line once tabs expand, as `expand_tabs_mapped` lays it out.
fn line_columns(line: &[u8]) -> u32 {
    let mut column = 0;
    for ch in String::from_utf8_lossy(line)
        .trim_end_matches(['\r', '\n'])
        .chars()
    {
        column += if ch == '\t' { 4 - column % 4 } else { 1 };
    }
    column
}

/// Columns of the widest HEAD and newer line within `ranges`. Every part of
/// a diff is this wide, so the parts scroll sideways together.
pub(super) fn widest(
    changes: &FileChanges,
    ranges: impl Iterator<Item = (Range<u32>, Range<u32>)>,
) -> [u32; 2] {
    let mut widest = [0; 2];
    for (old, new) in ranges {
        for line in old {
            widest[0] = widest[0].max(line_columns(changes.head_line(line)));
        }
        for line in new {
            widest[1] = widest[1].max(line_columns(changes.worktree_line(line)));
        }
    }
    widest
}

/// A side-by-side diff of `changes`, from HEAD on the left to a newer version.
pub(super) struct SplitDiff<'a> {
    pub id: String,
    pub path: &'a Path,
    /// Whether this is the read-only diff of the index, for fold state.
    pub staged: bool,
    pub titles: [&'static str; 2],
    pub changes: &'a FileChanges,
    pub old_len: u32,
    pub highlights: [Option<&'a LineHighlights>; 2],
    /// Staging checkboxes for the working-tree changes of file `index`.
    pub staging: Option<(usize, &'a Arc<ReviewFile>, &'a FileContent)>,
}

/// What the parts of one side-by-side diff share. The Commit review keeps it
/// until the diff or its folds change, so parts on screen do not recompute
/// it every frame.
pub(super) struct SplitLayout {
    pub regions: Vec<Region>,
    /// Columns of the widest HEAD and newer line in any region.
    pub widest: [u32; 2],
    /// Word highlights of each hunk's shown lines, computed when first drawn.
    edits: Vec<OnceCell<[LineEdits; 2]>>,
}

/// The side-by-side parts of `diff` with these `regions`, up to the line limit.
pub(super) fn split_parts(diff: &SplitDiff, regions: &[Region]) -> Vec<DiffPart> {
    let hunks = diff.changes.hunks();
    let mut parts = vec![DiffPart::Titles];
    let (mut visible, mut shown_to) = (0, 0);
    for (index, region) in regions.iter().enumerate() {
        if visible >= MAX_VISIBLE_LINES {
            parts.push(DiffPart::Limited);
            return parts;
        }
        parts.push(DiffPart::Region {
            index,
            shown_from: shown_to,
            visible,
        });
        for hunk in &hunks[region.hunks.clone()] {
            visible += hunk.before.len() + hunk.after.len();
            if visible > MAX_VISIBLE_LINES {
                parts.push(DiffPart::Limited);
                return parts;
            }
        }
        shown_to = region.old.end;
    }
    if diff.old_len > shown_to {
        parts.push(DiffPart::Tail { from: shown_to });
    }
    parts
}

impl Workspace {
    pub(super) fn split_regions(&self, diff: &SplitDiff) -> Vec<Region> {
        regions(diff.changes.hunks(), diff.old_len, |line| {
            self.commit_review
                .expanded
                .contains(&(diff.path.to_path_buf(), diff.staged, line))
        })
    }

    pub(super) fn split_layout(&self, diff: &SplitDiff) -> SplitLayout {
        let regions = self.split_regions(diff);
        SplitLayout {
            widest: widest(
                diff.changes,
                regions.iter().map(|r| (r.old.clone(), r.new.clone())),
            ),
            edits: diff
                .changes
                .hunks()
                .iter()
                .map(|_| OnceCell::new())
                .collect(),
            regions,
        }
    }

    /// The scroll handles a diff's parts share, one per column.
    pub(super) fn diff_scroll(&mut self, id: &str) -> [gpui::ScrollHandle; 2] {
        self.commit_review
            .scrolls
            .entry(id.to_string())
            .or_default()
            .each_ref()
            .map(|scrolled| scrolled.handle.clone())
    }

    /// Horizontal scroll bars along the bottom of a diff list for diff `id`:
    /// one under each column of a side-by-side diff, or one across a unified
    /// one. Each shows only while its column clips long lines.
    pub(super) fn diff_scrollbars(&mut self, id: &str, split: bool, cx: &App) -> gpui::AnyElement {
        let [old, new] = self
            .commit_review
            .scrolls
            .entry(id.to_string())
            .or_default()
            .each_ref()
            .map(|scrolled| scrolled.bar.clone());
        let bars = div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(SCROLLBAR_THICKNESS)
            .flex();
        if !split {
            return bars
                .child(
                    div()
                        .relative()
                        .size_full()
                        .child(Scrollbar::horizontal_overlay(old)),
                )
                .into_any_element();
        }
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let column = |share: f32, bar| {
            div()
                .relative()
                .h_full()
                .flex_basis(gpui::relative(0.0))
                .flex_grow(share)
                .min_w_0()
                .child(Scrollbar::horizontal_overlay(bar))
        };
        let split = self.commit_review.split.unwrap_or(0.5);
        bars.child(column(split, old))
            // The gutters and ribbon between the columns, as render_split_part lays them out.
            .child(
                div()
                    .flex_none()
                    .w((tokens.sizes.space_7 + tokens.sizes.space_9) * 2.0 + tokens.sizes.space_9),
            )
            .child(column(1.0 - split, new))
            .into_any_element()
    }

    /// The pixel widths of `columns` in the diff font, padded like code rows.
    pub(super) fn columns_width(columns: [u32; 2], style: &DiffStyle, cx: &App) -> [Pixels; 2] {
        let text = cx.text_system();
        let advance = text
            .ch_advance(
                text.resolve_font(&gpui::font(style.font_family.clone())),
                style.font_size,
            )
            .unwrap_or(style.font_size * 0.6);
        let padding = cx.global::<nucleotide_ui::Theme>().tokens.sizes.space_3 * 2.0;
        columns.map(|columns| advance * columns as f32 + padding)
    }

    pub(super) fn render_split_part(
        &mut self,
        diff: SplitDiff,
        layout: &SplitLayout,
        part: DiffPart,
        style: &DiffStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let tokens = cx.global::<nucleotide_ui::Theme>().tokens;
        let row = row_height(cx);
        let checkbox_width = tokens.sizes.space_7;
        let number_width = tokens.sizes.space_9;
        let gutter = checkbox_width + number_width;
        let ribbon = tokens.sizes.space_9;
        let colors = Colors {
            added: tokens.editor.vcs_added,
            modified: tokens.editor.vcs_modified,
            deleted: tokens.editor.vcs_deleted,
        };
        let split = self.commit_review.split.unwrap_or(0.5);
        let shares = [split, 1.0 - split];
        // The ribbon between the line numbers is the divider.
        let middle = gutter * 2.0 + ribbon;
        match part {
            DiffPart::Titles => {
                let title = |text: &'static str, share: f32| {
                    div()
                        .flex_basis(gpui::relative(0.0))
                        .flex_grow(share)
                        .min_w_0()
                        .px(tokens.sizes.space_3)
                        .child(text)
                };
                return div()
                    .flex()
                    .py(tokens.sizes.space_1)
                    .bg(style.surface)
                    .text_color(style.text_secondary)
                    .child(title(diff.titles[0], shares[0]))
                    .child(div().w(middle).flex_none())
                    .child(title(diff.titles[1], shares[1]))
                    .into_any_element();
            }
            DiffPart::Limited => {
                return div()
                    .p(tokens.sizes.space_3)
                    .child(format!(
                        "Preview limited to {MAX_VISIBLE_LINES} changed lines. Whole-file actions include all changes."
                    ))
                    .into_any_element();
            }
            _ => {}
        }
        // Parts of one diff share its scroll handles and selectors, not element ids.
        let part_id = match part {
            DiffPart::Region { index, .. } => format!("{}-{index}", diff.id),
            _ => format!("{}-tail", diff.id),
        };
        let hunks = diff.changes.hunks();
        let fold_key = |line| (diff.path.to_path_buf(), diff.staged, line);
        let regions = &layout.regions;
        let scroll = self.diff_scroll(&diff.id);
        let widths = Self::columns_width(layout.widest, style, cx);
        // Lines and hunks stay clickable while a stage runs; clicks queue behind it.
        let locked = self.commit_review.operation.is_some();

        let mut sides = [Side::default(), Side::default()];
        let mut blocks = Vec::new();
        let mut folds = Vec::new();
        let mut visible = 0;

        let number = |side: usize, line: u32| {
            div()
                .w(number_width)
                .flex_none()
                .when(side == 0, |n| n.text_right().pr(tokens.sizes.space_2))
                .when(side == 1, |n| n.pl(tokens.sizes.space_2))
                .child((line + 1).to_string())
        };
        let gutter_row = |side: usize, line: u32, checkbox: Option<gpui::AnyElement>| {
            let slot = div()
                .w(checkbox_width)
                .flex_none()
                .flex()
                .justify_center()
                .children(checkbox);
            let cells = div().h(row).flex().items_center();
            if side == 0 {
                cells.child(slot).child(number(0, line))
            } else {
                cells.child(number(1, line)).child(slot)
            }
            .into_any_element()
        };
        let code_row = |text: StyledText, tint: Option<Hsla>| {
            div()
                .h(row)
                .flex()
                .items_center()
                .px(tokens.sizes.space_3)
                .whitespace_nowrap()
                .when_some(tint, |r, tint| r.bg(tint.opacity(LINE_TINT)))
                .child(text)
                .into_any_element()
        };
        let blank = || div().h(row).into_any_element();
        let line_of = |side: usize, line: u32| {
            line_text(if side == 0 {
                diff.changes.head_line(line)
            } else {
                diff.changes.worktree_line(line)
            })
        };
        let highlights = |side: usize, line: u32| {
            diff.highlights[side]
                .and_then(|lines| lines.get(line as usize))
                .map(Vec::as_slice)
        };

        // Unchanged rows are not connected, so the columns can drift apart inside a
        // region. Folds realign them, which keeps the drift local like scrolling would.
        let mut fold = |sides: &mut [Side; 2], start: u32, count: u32, cx: &mut Context<Self>| {
            let rows = sides[0].code.len().max(sides[1].code.len());
            for side in sides.iter_mut() {
                while side.code.len() < rows {
                    side.code.push(blank());
                    side.gutter.push(blank());
                }
            }
            if count == 0 {
                return;
            }
            folds.push(rows);
            let label = format!(
                "{count} unchanged line{}",
                if count == 1 { "" } else { "s" }
            );
            for (index, side) in sides.iter_mut().enumerate() {
                let key = fold_key(start);
                let hover_bg = tokens.chrome.surface_hover;
                side.code.push(
                    div()
                        .id(SharedString::from(format!(
                            "{}-fold-{start}-{index}",
                            diff.id
                        )))
                        .h(row)
                        .flex()
                        .items_center()
                        .gap(tokens.sizes.space_2)
                        .px(tokens.sizes.space_3)
                        .bg(style.surface)
                        .text_color(style.text_secondary)
                        .cursor_pointer()
                        .hover(move |el| el.bg(hover_bg))
                        .child(
                            nucleotide_ui::icon()
                                .path("icons/unfold.svg")
                                .size(tokens.sizes.space_4)
                                .flex_none(),
                        )
                        .child(label.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.commit_review.expanded.insert(key.clone());
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                side.gutter
                    .push(div().h(row).bg(style.surface).into_any_element());
            }
        };

        let (target, mut shown_to) = match part {
            DiffPart::Region {
                index,
                shown_from,
                visible: before,
            } => {
                visible = before;
                (Some(index), shown_from)
            }
            DiffPart::Tail { from } => (None, from),
            _ => (None, diff.old_len),
        };
        if let Some(region) = target.and_then(|index| regions.get(index)) {
            'regions: {
                fold(&mut sides, shown_to, region.old.start - shown_to, cx);
                let mut cursor = (region.old.start, region.new.start);
                for hunk_index in region.hunks.clone() {
                    if visible >= MAX_VISIBLE_LINES {
                        break 'regions;
                    }
                    let hunk = &hunks[hunk_index];
                    for (old, new) in (cursor.0..hunk.before.start).zip(cursor.1..) {
                        for (side, line) in [(0, old), (1, new)] {
                            let text = code_text(
                                cx,
                                &line_of(side, line),
                                highlights(side, line),
                                &[],
                                colors,
                            );
                            sides[side].code.push(code_row(text, None));
                            sides[side].gutter.push(gutter_row(side, line, None));
                        }
                    }
                    let color = match (hunk.before.is_empty(), hunk.after.is_empty()) {
                        (true, _) => colors.added,
                        (_, true) => colors.deleted,
                        _ => colors.modified,
                    };
                    let removed = hunk.before.clone().take(MAX_VISIBLE_LINES - visible);
                    let added = hunk
                        .after
                        .clone()
                        .take((MAX_VISIBLE_LINES - visible).saturating_sub(removed.len()));
                    let lines = [removed.collect::<Vec<_>>(), added.collect::<Vec<_>>()];
                    visible += lines[0].len() + lines[1].len();
                    let texts: [Vec<String>; 2] = [
                        lines[0].iter().map(|&n| line_of(0, n)).collect(),
                        lines[1].iter().map(|&n| line_of(1, n)).collect(),
                    ];
                    // Whole-line insertions and deletions need no word highlights.
                    let edits = layout.edits[hunk_index].get_or_init(|| {
                        if texts[0].is_empty() || texts[1].is_empty() {
                            [
                                vec![Vec::new(); texts[0].len()],
                                vec![Vec::new(); texts[1].len()],
                            ]
                        } else {
                            let refs = texts
                                .each_ref()
                                .map(|t| t.iter().map(String::as_str).collect::<Vec<_>>());
                            word_edits(&refs[0], &refs[1])
                        }
                    });
                    let start = [sides[0].code.len(), sides[1].code.len()];
                    for side in 0..2 {
                        if lines[side].is_empty() {
                            sides[side].markers.push((start[side], color));
                        }
                        for (offset, &line) in lines[side].iter().enumerate() {
                            let text = code_text(
                                cx,
                                &texts[side][offset],
                                highlights(side, line),
                                &edits[side][offset],
                                colors,
                            );
                            let row = code_row(text, Some(color));
                            let row = self.with_hunk_menu(row, &diff, hunk_index, cx);
                            sides[side].code.push(row);
                            let checkbox = diff.staging.map(|(index, file, content)| {
                                let removal = side == 0;
                                let staged = content.selection.as_ref().is_some_and(|s| {
                                    if removal {
                                        s.is_removal_staged(line)
                                    } else {
                                        s.is_addition_staged(line)
                                    }
                                });
                                let checkbox = self
                                    .review_checkbox(
                                        index,
                                        String::new(),
                                        if staged {
                                            SelectionState::Staged
                                        } else {
                                            SelectionState::Unstaged
                                        },
                                        file.clone(),
                                        StageChange::Line {
                                            removal,
                                            line,
                                            staged: !staged,
                                        },
                                        cx,
                                    )
                                    .disabled(locked || content.selection.is_none());
                                div()
                                    .when(cfg!(test), |cell| {
                                        cell.debug_selector(move || {
                                            format!("commit-row-{index}-{removal}-{line}")
                                        })
                                    })
                                    .child(checkbox)
                                    .into_any_element()
                            });
                            let row = gutter_row(side, line, checkbox);
                            let row = self.with_hunk_menu(row, &diff, hunk_index, cx);
                            sides[side].gutter.push(row);
                        }
                    }
                    let checkbox = diff.staging.map(|(index, file, content)| {
                        let state = content
                            .selection
                            .as_ref()
                            .map(|s| s.hunk_state(hunk))
                            .unwrap_or(SelectionState::Unstaged);
                        self.review_checkbox(
                            index,
                            String::new(),
                            state,
                            file.clone(),
                            StageChange::Hunk(hunk_index, state != SelectionState::Staged),
                            cx,
                        )
                        .disabled(locked || content.selection.is_none())
                        .into_any_element()
                    });
                    blocks.push(Block {
                        rows: [start[0]..sides[0].code.len(), start[1]..sides[1].code.len()],
                        color,
                        checkbox,
                    });
                    if lines[0].len() < hunk.before.len() || lines[1].len() < hunk.after.len() {
                        break 'regions;
                    }
                    cursor = (hunk.before.end, hunk.after.end);
                }
                for (old, new) in (cursor.0..region.old.end).zip(cursor.1..) {
                    for (side, line) in [(0, old), (1, new)] {
                        let text = code_text(
                            cx,
                            &line_of(side, line),
                            highlights(side, line),
                            &[],
                            colors,
                        );
                        sides[side].code.push(code_row(text, None));
                        sides[side].gutter.push(gutter_row(side, line, None));
                    }
                }
                shown_to = region.old.end;
            }
        }
        // A region pads its columns to the same length; the tail is one fold.
        let trailing = if target.is_none() {
            diff.old_len - shown_to
        } else {
            0
        };
        fold(&mut sides, shown_to, trailing, cx);

        let rows = sides[0].code.len();
        let [old, new] = sides;
        let (old_gutter, new_gutter) = (old.gutter, new.gutter);
        let ribbons = blocks
            .iter()
            .map(|block| (block.rows.clone(), block.color))
            .collect::<Vec<_>>();
        let surface = style.surface;
        let paint = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let origin = bounds.origin;
                let y = |row_index: usize| origin.y + row * row_index as f32;
                for &fold_row in &folds {
                    window.paint_quad(gpui::fill(
                        gpui::Bounds::new(
                            point(origin.x, y(fold_row)),
                            size(bounds.size.width, row),
                        ),
                        surface,
                    ));
                }
                let (a, b) = (origin.x + gutter, origin.x + gutter + ribbon);
                let middle = origin.x + gutter + ribbon * 0.5;
                let right = origin.x + bounds.size.width;
                for ([old, new], color) in &ribbons {
                    let mut path = PathBuilder::fill();
                    path.move_to(point(origin.x, y(old.start)));
                    path.line_to(point(a, y(old.start)));
                    path.cubic_bezier_to(
                        point(b, y(new.start)),
                        point(middle, y(old.start)),
                        point(middle, y(new.start)),
                    );
                    path.line_to(point(right, y(new.start)));
                    path.line_to(point(right, y(new.end)));
                    path.line_to(point(b, y(new.end)));
                    path.cubic_bezier_to(
                        point(a, y(old.end)),
                        point(middle, y(new.end)),
                        point(middle, y(old.end)),
                    );
                    path.line_to(point(origin.x, y(old.end)));
                    path.close();
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color.opacity(LINE_TINT));
                    }
                    // An empty side marks where the other side's lines belong.
                    for (rows, from, to) in [(old, origin.x, a), (new, b, right)] {
                        if rows.is_empty() {
                            window.paint_quad(gpui::fill(
                                gpui::Bounds::new(
                                    point(from, y(rows.start) - px(0.5)),
                                    size(to - from, px(1.0)),
                                ),
                                color.opacity(MARKER_TINT),
                            ));
                        }
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let checkbox_size = tokens.sizes.space_6;
        let hunk_checkboxes = blocks.into_iter().filter_map(|block| {
            let [old, new] = &block.rows;
            // Sit in the row holding the ribbon's midpoint rather than across two rows.
            let middle = (old.start + old.end + new.start + new.end) as f32 / 4.0;
            Some(
                div()
                    .absolute()
                    .left(gutter + (ribbon - checkbox_size) * 0.5)
                    .top(row * middle.floor() + (row - checkbox_size) * 0.5)
                    .child(block.checkbox?),
            )
        });
        let column =
            |code: Vec<gpui::AnyElement>, markers: Vec<(usize, Hsla)>, side: usize, name: &str| {
                let mut column = div()
                    .id(SharedString::from(format!("{part_id}-{name}")))
                    .track_scroll(&scroll[side]);
                // A vertical wheel scrolls the file list, not the code sideways.
                column.style().restrict_scroll_to_axis = Some(true);
                column
                    .when(cfg!(test), |column| {
                        let selector = format!("{}-{name}", diff.id);
                        column.debug_selector(move || selector)
                    })
                    .flex_basis(gpui::relative(0.0))
                    .flex_grow(shares[side])
                    .min_w_0()
                    .flex()
                    .overflow_x_scroll()
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .min_w_full()
                            .flex()
                            .flex_col()
                            // As wide as the widest line in any part.
                            .child(div().w(widths[side]))
                            .children(code)
                            .children(markers.into_iter().map(|(at, color)| {
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .top(row * at as f32 - px(0.5))
                                    .h(px(1.0))
                                    .bg(color.opacity(MARKER_TINT))
                            })),
                    )
            };
        let gutter_column = |rows: Vec<gpui::AnyElement>| {
            div().w(gutter).flex_none().flex().flex_col().children(rows)
        };
        // Dragging the divider moves the middle strip with the pointer, and a
        // double click recentres it.
        let handle_id = diff.id.clone();
        let handle = div()
            .id(SharedString::from(format!("{part_id}-split")))
            .when(cfg!(test), |handle| {
                let selector = format!("{}-split", diff.id);
                handle.debug_selector(move || selector)
            })
            .w(ribbon)
            .flex_none()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .on_drag(SplitHandle(diff.id.clone()), |_, _, _, cx| {
                cx.new(|_| gpui::Empty)
            })
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.commit_review.split = None;
                    cx.notify();
                }
            }));
        let resize = cx.listener(
            move |this, event: &gpui::DragMoveEvent<SplitHandle>, _, cx| {
                if event.drag(cx).0 != handle_id {
                    return;
                }
                let available = event.bounds.size.width - middle;
                if available <= px(0.0) {
                    return;
                }
                let old = event.event.position.x - event.bounds.origin.x - middle * 0.5;
                let split = (old / available).clamp(MIN_SPLIT, 1.0 - MIN_SPLIT);
                if this.commit_review.split != Some(split) {
                    this.commit_review.split = Some(split);
                    cx.notify();
                }
            },
        );
        div()
            .flex()
            .font_family(style.font_family.clone())
            .text_size(style.font_size)
            .line_height(row)
            .on_drag_move(resize)
            .child(column(old.code, old.markers, 0, "old"))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .flex()
                    .h(row * rows as f32)
                    .text_color(style.text_secondary)
                    .child(paint)
                    .child(gutter_column(old_gutter))
                    .child(handle)
                    .child(gutter_column(new_gutter))
                    .children(hunk_checkboxes),
            )
            .child(column(new.code, new.markers, 1, "new"))
            .into_any_element()
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    fn hunk(before: Range<u32>, after: Range<u32>) -> StagingHunk {
        StagingHunk { before, after }
    }

    #[test]
    fn regions_fold_long_unchanged_runs_and_keep_both_sides_in_step() {
        // Two lines become four at HEAD line 10, then a line is inserted after HEAD line 30.
        let hunks = [hunk(10..12, 10..14), hunk(30..30, 32..33)];
        assert_eq!(
            regions(&hunks, 40, |_| false),
            [
                Region {
                    old: 7..15,
                    new: 7..17,
                    hunks: 0..1
                },
                Region {
                    old: 27..33,
                    new: 29..36,
                    hunks: 1..2
                },
            ]
        );
        // Expanding the run hidden between them joins the regions.
        assert_eq!(
            regions(&hunks, 40, |line| line == 15),
            [Region {
                old: 7..33,
                new: 7..36,
                hunks: 0..2
            }]
        );
        // The leading and trailing runs expand on their own.
        assert_eq!(regions(&hunks, 40, |line| line == 0)[0].old.start, 0);
        assert_eq!(regions(&hunks, 40, |line| line == 33)[1].new.end, 43);
    }

    #[test]
    fn regions_never_fold_a_single_line() {
        // A seven-line gap would fold one line between two contexts, so it shows whole.
        let seven = [hunk(10..11, 10..11), hunk(18..19, 18..19)];
        assert_eq!(regions(&seven, 30, |_| false).len(), 1);
        let eight = [hunk(10..11, 10..11), hunk(19..20, 19..20)];
        assert_eq!(regions(&eight, 30, |_| false).len(), 2);
        // Leading and trailing runs one line longer than the context are shown whole.
        assert_eq!(
            regions(&[hunk(4..5, 4..4)], 9, |_| false),
            [Region {
                old: 0..9,
                new: 0..8,
                hunks: 0..1
            }]
        );
    }

    #[test]
    fn word_edits_distinguish_changed_inserted_and_deleted_words() {
        let [old, new] = word_edits(&["let x = foo(a);"], &["let y = foo(a, b);"]);
        assert_eq!(old, [vec![(4..5, Edit::Changed)]]);
        assert_eq!(new, [vec![(4..5, Edit::Changed), (13..16, Edit::Inserted)]]);

        let [old, new] = word_edits(&["a();", "\tb();"], &["a();"]);
        assert_eq!(old, [vec![], vec![(0..5, Edit::Deleted)]]);
        assert_eq!(new, [vec![]]);
    }

    #[test]
    fn expanded_tabs_keep_ranges_on_the_same_characters() {
        let (text, map) = expand_tabs_mapped("a\tbé\tc");
        assert_eq!(text, "a   bé  c");
        assert_eq!(&text[map[2]..map[5]], "bé");
        assert_eq!(map[7], text.len());
    }
}
