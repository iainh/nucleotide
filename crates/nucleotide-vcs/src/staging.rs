// ABOUTME: Line-level staging model for one file, built from its HEAD and working-tree contents
// ABOUTME: Computes index contents for a selection of changes and reads a selection back from the index

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use std::ops::Range;

use imara_diff::sources::byte_lines;
use imara_diff::{Algorithm, Diff, IndentHeuristic, IndentLevel, InternedInput};

/// Largest file, on any side, that is staged line by line.
pub const MAX_LINE_STAGING_BYTES: usize = 4 * 1024 * 1024;

/// Git treats a file as binary when its first 8000 bytes contain a NUL.
const BINARY_PROBE_BYTES: usize = 8000;

/// Whether a file can be staged line by line rather than only as a whole.
pub fn supports_line_staging(contents: &[&[u8]]) -> bool {
    contents.iter().all(|bytes| {
        bytes.len() <= MAX_LINE_STAGING_BYTES
            && !bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0)
    })
}

/// One change between HEAD and the working tree: HEAD lines `before` are
/// replaced by working-tree lines `after`. Either range may be empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingHunk {
    pub before: Range<u32>,
    pub after: Range<u32>,
}

/// How much of a hunk or file is staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionState {
    Unstaged,
    Partial,
    Staged,
}

/// The changed lines that are staged: removals by HEAD line number and
/// additions by working-tree line number.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    removed: BTreeSet<u32>,
    added: BTreeSet<u32>,
}

impl Selection {
    pub fn is_removal_staged(&self, head_line: u32) -> bool {
        self.removed.contains(&head_line)
    }

    pub fn is_addition_staged(&self, worktree_line: u32) -> bool {
        self.added.contains(&worktree_line)
    }

    pub fn set_removal(&mut self, head_line: u32, staged: bool) {
        set_member(&mut self.removed, head_line, staged);
    }

    pub fn set_addition(&mut self, worktree_line: u32, staged: bool) {
        set_member(&mut self.added, worktree_line, staged);
    }

    pub fn set_hunk(&mut self, hunk: &StagingHunk, staged: bool) {
        for line in hunk.before.clone() {
            self.set_removal(line, staged);
        }
        for line in hunk.after.clone() {
            self.set_addition(line, staged);
        }
    }

    pub fn hunk_state(&self, hunk: &StagingHunk) -> SelectionState {
        let staged = hunk
            .before
            .clone()
            .filter(|line| self.is_removal_staged(*line))
            .count()
            + hunk
                .after
                .clone()
                .filter(|line| self.is_addition_staged(*line))
                .count();
        let total = hunk.before.len() + hunk.after.len();
        match staged {
            0 => SelectionState::Unstaged,
            staged if staged == total => SelectionState::Staged,
            _ => SelectionState::Partial,
        }
    }
}

fn set_member(set: &mut BTreeSet<u32>, line: u32, present: bool) {
    if present {
        set.insert(line);
    } else {
        set.remove(&line);
    }
}

/// A file's changes from HEAD to the working tree, split into lines that can
/// be staged one at a time. Lines keep their terminators, so CRLF endings and
/// a missing final newline survive staging byte for byte.
#[derive(Clone)]
pub struct FileChanges {
    head: Text,
    worktree: Text,
    hunks: Vec<StagingHunk>,
    newline: &'static [u8],
}

impl FileChanges {
    /// `head` is empty for a file that is not in HEAD, and `worktree` is
    /// empty for a deleted file.
    pub fn new(head: &[u8], worktree: &[u8]) -> Self {
        let head = Text::new(head);
        let worktree = Text::new(worktree);
        let hunks = line_diff(&head, &worktree)
            .hunks()
            .map(|hunk| StagingHunk {
                before: hunk.before,
                after: hunk.after,
            })
            .collect();
        let newline = [&worktree, &head]
            .into_iter()
            .flat_map(|text| text.lines())
            .find(|line| line.ends_with(b"\n"))
            .map_or(b"\n".as_slice(), |line| {
                if line.ends_with(b"\r\n") {
                    b"\r\n"
                } else {
                    b"\n"
                }
            });
        Self {
            head,
            worktree,
            hunks,
            newline,
        }
    }

    pub fn hunks(&self) -> &[StagingHunk] {
        &self.hunks
    }

    /// Number of HEAD lines.
    pub fn head_len(&self) -> u32 {
        self.head.len()
    }

    /// Number of working-tree lines.
    pub fn worktree_len(&self) -> u32 {
        self.worktree.len()
    }

    /// A HEAD line, including its line terminator.
    pub fn head_line(&self, line: u32) -> &[u8] {
        self.head.line(line)
    }

    /// A working-tree line, including its line terminator.
    pub fn worktree_line(&self, line: u32) -> &[u8] {
        self.worktree.line(line)
    }

    pub fn select_all(&self) -> Selection {
        let mut selection = Selection::default();
        for hunk in &self.hunks {
            selection.set_hunk(hunk, true);
        }
        selection
    }

    pub fn selection_state(&self, selection: &Selection) -> SelectionState {
        let mut states = self.hunks.iter().map(|hunk| selection.hunk_state(hunk));
        let Some(first) = states.next() else {
            return SelectionState::Unstaged;
        };
        if states.all(|state| state == first) {
            first
        } else {
            SelectionState::Partial
        }
    }

    /// The index contents that stage exactly `selection`: HEAD with the
    /// selected removals dropped and the selected additions inserted. Pair
    /// replacements by line position so staging the first of two adjacent
    /// replacements does not move it after the second, retained HEAD line.
    pub fn index_content(&self, selection: &Selection) -> Vec<u8> {
        let mut content = Vec::with_capacity(self.head.bytes.len().max(self.worktree.bytes.len()));
        let mut next_head_line = 0;
        for hunk in &self.hunks {
            for line in next_head_line..hunk.before.start {
                self.push_line(&mut content, self.head.line(line));
            }
            for offset in 0..hunk.before.len().max(hunk.after.len()) as u32 {
                let before = hunk.before.start + offset;
                let after = hunk.after.start + offset;
                if before < hunk.before.end && !selection.is_removal_staged(before) {
                    self.push_line(&mut content, self.head.line(before));
                }
                if after < hunk.after.end && selection.is_addition_staged(after) {
                    self.push_line(&mut content, self.worktree.line(after));
                }
            }
            next_head_line = hunk.before.end;
        }
        for line in next_head_line..self.head.len() {
            self.push_line(&mut content, self.head.line(line));
        }
        content
    }

    /// A selection that reproduces the index exactly. Returns `None` for
    /// divergent contents or when ambiguous repeated lines exceed the bounded
    /// matching budget; callers must then offer an explicit whole-file action.
    pub fn selection_from_index(&self, index: &[u8]) -> Option<Selection> {
        let index = Text::new(index);
        let head_to_index = line_diff(&self.head, &index);
        let index_to_worktree = line_diff(&index, &self.worktree);
        let mut selection = Selection::default();
        for hunk in &self.hunks {
            for line in hunk.before.clone() {
                selection.set_removal(line, head_to_index.is_removed(line));
            }
            for line in hunk.after.clone() {
                selection.set_addition(line, !index_to_worktree.is_added(line));
            }
        }
        // The line mapping is a heuristic; only report it when it reproduces
        // the index exactly.
        if self.index_content(&selection) == index.bytes {
            return Some(selection);
        }
        self.match_index(&index.bytes)
    }

    /// Repeated lines can cause the two independent diffs to choose incompatible
    /// alignments. Match the index against the actual sequence emitted by
    /// index_content: unchanged lines are required, changed lines are optional.
    fn match_index(&self, index: &[u8]) -> Option<Selection> {
        let mut lines = Vec::new();
        let mut previous = 0;
        let mut selection = Selection::default();
        for hunk in &self.hunks {
            lines.extend((previous..hunk.before.start).map(|n| (self.head.line(n), None)));
            for offset in 0..hunk.before.len().max(hunk.after.len()) as u32 {
                let before = hunk.before.start + offset;
                let after = hunk.after.start + offset;
                if before < hunk.before.end {
                    lines.push((self.head.line(before), Some((true, before))));
                    selection.set_removal(before, true);
                }
                if after < hunk.after.end {
                    lines.push((self.worktree.line(after), Some((false, after))));
                }
            }
            previous = hunk.before.end;
        }
        lines.extend((previous..self.head.len()).map(|n| (self.head.line(n), None)));

        // Offset -> linked decisions. Deduplicating equal offsets prevents an
        // exponential search; cap total work for pathological repeated files.
        let mut offsets = BTreeMap::from([(0, None)]);
        let mut decisions = Vec::new();
        let mut budget = 1_000_000usize;
        for (line, change) in lines {
            budget = budget.checked_sub(offsets.len())?;
            let mut next = if change.is_some() {
                offsets.clone()
            } else {
                BTreeMap::new()
            };
            for (offset, parent) in offsets {
                // index_content inserts a separator after an unterminated
                // retained line when another line follows it.
                let mut start = offset;
                if offset > 0 && index[offset - 1] != b'\n' {
                    if !index[offset..].starts_with(self.newline) {
                        continue;
                    }
                    start += self.newline.len();
                }
                if !index[start..].starts_with(line) {
                    continue;
                }
                let end = start + line.len();
                if let Entry::Vacant(entry) = next.entry(end) {
                    let parent = if let Some((removal, n)) = change {
                        decisions.push((parent, removal, n));
                        Some(decisions.len() - 1)
                    } else {
                        parent
                    };
                    entry.insert(parent);
                }
            }
            if next.is_empty() {
                return None;
            }
            offsets = next;
        }
        let mut decision = *offsets.get(&index.len())?;
        while let Some(i) = decision {
            let (parent, removal, line) = decisions[i];
            if removal {
                selection.set_removal(line, false);
            } else {
                selection.set_addition(line, true);
            }
            decision = parent;
        }
        (self.index_content(&selection) == index).then_some(selection)
    }

    /// A line that ends without a terminator (the last line of a file) gains
    /// one when another line follows it, instead of being joined to it.
    fn push_line(&self, content: &mut Vec<u8>, line: &[u8]) {
        if content.last().is_some_and(|byte| *byte != b'\n') {
            content.extend_from_slice(self.newline);
        }
        content.extend_from_slice(line);
    }
}

#[derive(Clone)]
struct Text {
    bytes: Vec<u8>,
    line_starts: Vec<usize>,
}

impl Text {
    fn new(bytes: &[u8]) -> Self {
        let mut line_starts = Vec::new();
        let mut start = 0;
        for line in byte_lines(bytes) {
            line_starts.push(start);
            start += line.len();
        }
        Self {
            bytes: bytes.to_vec(),
            line_starts,
        }
    }

    fn len(&self) -> u32 {
        self.line_starts.len() as u32
    }

    fn line(&self, line: u32) -> &[u8] {
        let line = line as usize;
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.bytes.len());
        &self.bytes[self.line_starts[line]..end]
    }

    fn lines(&self) -> impl Iterator<Item = &[u8]> {
        (0..self.len()).map(|line| self.line(line))
    }
}

/// The same line diff and slider heuristic as the editor's diff gutter, so the
/// hunks here line up with the gutter markers.
fn line_diff(before: &Text, after: &Text) -> Diff {
    let input = InternedInput::new(before.bytes.as_slice(), after.bytes.as_slice());
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_with_heuristic(
        &input,
        IndentHeuristic::new(|token| {
            IndentLevel::for_ascii_line(input.interner[token].iter().copied(), 4)
        }),
    );
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged(changes: &FileChanges, removed: &[u32], added: &[u32]) -> Vec<u8> {
        let mut selection = Selection::default();
        for line in removed {
            selection.set_removal(*line, true);
        }
        for line in added {
            selection.set_addition(*line, true);
        }
        changes.index_content(&selection)
    }

    #[test]
    fn staging_only_the_addition_of_a_modified_line_keeps_the_original() {
        let changes = FileChanges::new(b"a\nb\nc\n", b"a\nB\nc\n");
        assert_eq!(
            changes.hunks(),
            [StagingHunk {
                before: 1..2,
                after: 1..2
            }]
        );
        assert_eq!(staged(&changes, &[], &[1]), b"a\nb\nB\nc\n");
        assert_eq!(staged(&changes, &[1], &[]), b"a\nc\n");
        assert_eq!(staged(&changes, &[1], &[1]), b"a\nB\nc\n");
    }

    #[test]
    fn hunks_separated_by_one_line_stage_independently() {
        let changes = FileChanges::new(b"1\n2\n3\n", b"1x\n2\n3x\n");
        assert_eq!(changes.hunks().len(), 2);
        assert_eq!(staged(&changes, &[2], &[2]), b"1\n2\n3x\n");
        assert_eq!(staged(&changes, &[0], &[0]), b"1x\n2\n3\n");
    }

    #[test]
    fn a_line_without_a_final_newline_gains_one_when_a_line_is_added_after_it() {
        let changes = FileChanges::new(b"a\nb", b"a\nb\nc\n");
        assert_eq!(
            changes.hunks(),
            [StagingHunk {
                before: 1..2,
                after: 1..3
            }]
        );
        // Only `c` is staged; `b` must not be joined to it.
        assert_eq!(staged(&changes, &[], &[2]), b"a\nb\nc\n");
    }

    #[test]
    fn crlf_lines_keep_their_endings() {
        let changes = FileChanges::new(b"a\r\nb\r\n", b"a\r\nx\r\nb\r\ny");
        assert_eq!(staged(&changes, &[], &[1]), b"a\r\nx\r\nb\r\n");
        assert_eq!(staged(&changes, &[], &[3]), b"a\r\nb\r\ny");
    }

    #[test]
    fn new_and_deleted_files_stage_line_by_line() {
        let new_file = FileChanges::new(b"", b"one\ntwo\n");
        assert_eq!(staged(&new_file, &[], &[1]), b"two\n");
        let deleted = FileChanges::new(b"one\ntwo\n", b"");
        assert_eq!(staged(&deleted, &[0], &[]), b"two\n");
        assert_eq!(staged(&deleted, &[0, 1], &[]), b"");
    }

    #[test]
    fn an_index_edited_after_staging_is_not_a_selection() {
        let changes = FileChanges::new(b"a\nb\n", b"a\nB\n");
        assert_eq!(
            changes.selection_from_index(b"a\nb\n"),
            Some(Selection::default())
        );
        assert_eq!(changes.selection_from_index(b"a\nC\n"), None);
    }

    #[test]
    fn adjacent_replacements_preserve_the_unstaged_lines_position() {
        let changes = FileChanges::new(
            b"start\nold one\nold two\nend\n",
            b"start\nnew one\nnew two\nend\n",
        );
        for (line, expected) in [
            (1, b"start\nnew one\nold two\nend\n"),
            (2, b"start\nold one\nnew two\nend\n"),
        ] {
            assert_eq!(staged(&changes, &[line], &[line]), expected);
            let selection = changes
                .selection_from_index(expected)
                .expect("ordinary Git partial index");
            assert_eq!(changes.index_content(&selection), expected);
            assert!(selection.is_removal_staged(line));
            assert!(selection.is_addition_staged(line));
        }
    }

    #[test]
    fn selection_states_follow_the_staged_lines() {
        let changes = FileChanges::new(b"1\n2\n3\n", b"1x\n2\n3x\n");
        let [first, second] = changes.hunks() else {
            panic!("expected two hunks");
        };
        let mut selection = Selection::default();
        assert_eq!(
            changes.selection_state(&selection),
            SelectionState::Unstaged
        );
        selection.set_addition(first.after.start, true);
        assert_eq!(selection.hunk_state(first), SelectionState::Partial);
        selection.set_hunk(first, true);
        assert_eq!(selection.hunk_state(first), SelectionState::Staged);
        assert_eq!(selection.hunk_state(second), SelectionState::Unstaged);
        assert_eq!(changes.selection_state(&selection), SelectionState::Partial);
        assert_eq!(
            changes.selection_state(&changes.select_all()),
            SelectionState::Staged
        );
    }

    /// Every way of staging the changed lines must read back from the index
    /// as a selection that produces the same index.
    #[test]
    fn every_selection_reads_back_from_the_index() {
        let cases: [(&[u8], &[u8]); 7] = [
            (b"a\nb\nc\nd\ne\n", b"a\nB\nc\nd2\nd3\ne\n"),
            (
                b"fn x() {\n    1\n}\n",
                b"fn x() {\n    1\n    2\n}\n\nfn y() {}\n",
            ),
            (b"same\nsame\nend\n", b"same\nend\nsame\n"),
            (b"a\nb", b"a\nb\nc"),
            (b"", b"x\ny\nx\n"),
            (b"x\r\ny\r\n", b"y\r\nx\r\n"),
            (b"b\n", b"a\nb\nb\n"),
        ];
        for (head, worktree) in cases {
            let changes = FileChanges::new(head, worktree);
            let lines: Vec<(bool, u32)> = changes
                .hunks()
                .iter()
                .flat_map(|hunk| {
                    hunk.before
                        .clone()
                        .map(|line| (true, line))
                        .chain(hunk.after.clone().map(|line| (false, line)))
                })
                .collect();
            assert!(lines.len() <= 10, "too many combinations");
            for mask in 0u32..(1 << lines.len()) {
                let mut selection = Selection::default();
                for (bit, (removal, line)) in lines.iter().enumerate() {
                    let on = mask & (1 << bit) != 0;
                    if *removal {
                        selection.set_removal(*line, on);
                    } else {
                        selection.set_addition(*line, on);
                    }
                }
                let index = changes.index_content(&selection);
                let read_back = changes.selection_from_index(&index).unwrap_or_else(|| {
                    panic!(
                        "no selection for index {:?} (head {:?}, worktree {:?})",
                        String::from_utf8_lossy(&index),
                        String::from_utf8_lossy(head),
                        String::from_utf8_lossy(worktree),
                    )
                });
                assert_eq!(changes.index_content(&read_back), index);
            }
            assert_eq!(changes.index_content(&changes.select_all()), worktree);
            assert_eq!(changes.index_content(&Selection::default()), head);
        }
    }
}
