// ABOUTME: Commit history for the History sidebar: paged log, each commit's file changes,
// ABOUTME: and an incremental lane layout that draws the commits as a graph

use std::path::PathBuf;

use crate::repository::{GitRepository, READ_TIMEOUT_MS, command_error};
use crate::staging::MAX_LINE_STAGING_BYTES;

/// Output limit of one log page or ref listing.
const LIST_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
/// Contents read for one commit's diff, across all its files.
const COMMIT_CONTENTS_LIMIT: usize = 64 * 1024 * 1024;
/// Files of one commit whose contents are read; the rest are listed only.
pub const MAX_COMMIT_FILES: usize = 200;
const NULL_OBJECT: &str = "0000000000000000000000000000000000000000";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// A detached HEAD.
    Head,
    Branch,
    Remote,
    Tag,
}

/// A branch, remote branch or tag pointing at a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRef {
    pub kind: RefKind,
    /// The short name, such as `main`, `origin/main` or `v0.17.0`.
    pub name: String,
    /// HEAD points at this branch.
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    pub id: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Author time, in seconds since the Unix epoch.
    pub time: i64,
    pub subject: String,
    pub refs: Vec<CommitRef>,
}

/// How a file changed in a commit, as `git diff --raw` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
    Other,
}

#[derive(Debug, Clone)]
pub struct CommitFile {
    pub path: PathBuf,
    /// Where a renamed or copied file came from.
    pub old_path: Option<PathBuf>,
    pub kind: FileChangeKind,
    /// The file before and after the commit, `None` where it is absent, or why
    /// it cannot be shown line by line.
    pub contents: Result<[Option<Vec<u8>>; 2], String>,
}

#[derive(Debug, Clone)]
pub struct CommitChanges {
    /// The message after its subject line.
    pub body: String,
    /// Changes against the first parent, or against nothing for a root commit.
    pub files: Vec<CommitFile>,
}

impl GitRepository {
    /// The commits the history starts from: HEAD and every branch, remote
    /// branch and tag. Paging from fixed tips keeps later pages consistent
    /// with earlier ones while refs move. Empty when there are no commits.
    pub async fn history_tips(&self) -> Result<Vec<String>, String> {
        let mut tips: Vec<String> = self
            .optional_line(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .await?
            .into_iter()
            .collect();
        let output = self
            .run_process(
                &[
                    "for-each-ref",
                    "--format=%(objectname)",
                    "refs/heads",
                    "refs/remotes",
                    "refs/tags",
                ],
                Vec::new(),
                READ_TIMEOUT_MS,
                LIST_OUTPUT_LIMIT,
            )
            .await?;
        if !output.success || output.stdout_truncated {
            return Err(command_error("for-each-ref", &output));
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if !line.is_empty() && !tips.iter().any(|tip| tip == line) {
                tips.push(line.to_string());
            }
        }
        Ok(tips)
    }

    /// Up to `limit` commits reachable from `tips` after skipping `skip`, newest
    /// first. Date order never lists a parent before its children, which the
    /// graph layout relies on.
    pub async fn log(
        &self,
        tips: &[String],
        skip: usize,
        limit: usize,
    ) -> Result<Vec<CommitSummary>, String> {
        if tips.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let skip = format!("--skip={skip}");
        let limit = format!("--max-count={limit}");
        let args = [
            "log",
            "--date-order",
            "--decorate=full",
            "--no-color",
            "--format=%H%x1f%P%x1f%aN%x1f%at%x1f%D%x1f%s%x1e",
            &skip,
            &limit,
            "--stdin",
        ];
        let mut stdin = tips.join("\n").into_bytes();
        stdin.push(b'\n');
        let output = self
            .run_process(&args, stdin, READ_TIMEOUT_MS, LIST_OUTPUT_LIMIT)
            .await?;
        if !output.success || output.stdout_truncated {
            return Err(command_error("log", &output));
        }
        Ok(parse_log(&String::from_utf8_lossy(&output.stdout)))
    }

    /// The message body and file changes of commit `id`. File contents are
    /// read with two git processes however many files changed.
    pub async fn commit_changes(&self, id: &str) -> Result<CommitChanges, String> {
        let body = self
            .run_process(
                &["show", "-s", "--format=%b", id],
                Vec::new(),
                READ_TIMEOUT_MS,
                LIST_OUTPUT_LIMIT,
            )
            .await?;
        if !body.success {
            return Err(command_error("show", &body));
        }
        let parent = self
            .optional_line(&["rev-parse", "--verify", "--quiet", &format!("{id}^1")])
            .await?;
        let mut args = vec![
            "diff-tree",
            "-r",
            "-z",
            "-M",
            "--raw",
            "--no-abbrev",
            "--no-commit-id",
        ];
        match &parent {
            Some(parent) => args.push(parent),
            None => args.push("--root"),
        }
        args.push(id);
        let raw = self
            .run_process(&args, Vec::new(), READ_TIMEOUT_MS, LIST_OUTPUT_LIMIT)
            .await?;
        if !raw.success || raw.stdout_truncated {
            return Err(command_error("diff-tree", &raw));
        }
        let entries = parse_raw_diff(&raw.stdout);
        let objects: Vec<&str> = entries
            .iter()
            .take(MAX_COMMIT_FILES)
            .flat_map(|entry| entry.objects.iter())
            .filter_map(|(mode, object)| {
                (object != NULL_OBJECT && !mode.starts_with("160")).then_some(object.as_str())
            })
            .collect();
        let blobs = self.read_objects(&objects).await?;
        let files: Vec<CommitFile> = entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| {
                let contents = if index >= MAX_COMMIT_FILES {
                    Err(format!(
                        "Diff not shown: only the first {MAX_COMMIT_FILES} files are loaded"
                    ))
                } else if entry
                    .objects
                    .iter()
                    .any(|(mode, _)| mode.starts_with("160"))
                {
                    Err("Submodule change".to_string())
                } else {
                    let side = |(_, object): &(String, String)| {
                        if object == NULL_OBJECT {
                            return Ok(None);
                        }
                        blobs
                            .iter()
                            .find(|(id, _)| id == object)
                            .and_then(|(_, bytes)| bytes.clone())
                            .map(Some)
                            .ok_or_else(|| "Diff not shown: file too large".to_string())
                    };
                    side(&entry.objects[0]).and_then(|old| Ok([old, side(&entry.objects[1])?]))
                };
                CommitFile {
                    path: entry.path,
                    old_path: entry.old_path,
                    kind: entry.kind,
                    contents,
                }
            })
            .collect();
        Ok(CommitChanges {
            body: String::from_utf8_lossy(&body.stdout).trim().to_string(),
            files,
        })
    }

    /// Raw contents of each object, `None` for objects too large to diff.
    async fn read_objects(
        &self,
        objects: &[&str],
    ) -> Result<Vec<(String, Option<Vec<u8>>)>, String> {
        if objects.is_empty() {
            return Ok(Vec::new());
        }
        let mut stdin = objects.join("\n").into_bytes();
        stdin.push(b'\n');
        let check = self
            .run_process(
                &["cat-file", "--batch-check"],
                stdin,
                READ_TIMEOUT_MS,
                LIST_OUTPUT_LIMIT,
            )
            .await?;
        if !check.success || check.stdout_truncated {
            return Err(command_error("cat-file", &check));
        }
        // "<object> <type> <size>"; read the blobs that fit, in order, until the budget runs out.
        let mut budget = COMMIT_CONTENTS_LIMIT;
        let mut wanted = Vec::new();
        let mut result = Vec::new();
        for line in String::from_utf8_lossy(&check.stdout).lines() {
            let fields: Vec<_> = line.split(' ').collect();
            let [object, "blob", size] = fields.as_slice() else {
                continue;
            };
            let size: usize = size.parse().unwrap_or(usize::MAX);
            if size <= MAX_LINE_STAGING_BYTES && size <= budget {
                budget -= size;
                wanted.push(object.to_string());
            } else {
                result.push((object.to_string(), None));
            }
        }
        if wanted.is_empty() {
            return Ok(result);
        }
        let mut stdin = wanted.join("\n").into_bytes();
        stdin.push(b'\n');
        let limit = COMMIT_CONTENTS_LIMIT - budget + wanted.len() * 128;
        let output = self
            .run_process(&["cat-file", "--batch"], stdin, READ_TIMEOUT_MS, limit)
            .await?;
        if !output.success || output.stdout_truncated {
            return Err(command_error("cat-file", &output));
        }
        // "<object> <type> <size>\n<contents>\n" for each object.
        let mut rest = output.stdout.as_slice();
        while let Some(end) = rest.iter().position(|b| *b == b'\n') {
            let header = String::from_utf8_lossy(&rest[..end]).into_owned();
            let mut fields = header.split(' ');
            let (Some(object), Some(_), Some(size)) = (fields.next(), fields.next(), fields.next())
            else {
                break;
            };
            let size: usize = size.parse().map_err(|_| "Unexpected cat-file output")?;
            let start = end + 1;
            let bytes = rest
                .get(start..start + size)
                .ok_or("Unexpected cat-file output")?;
            result.push((object.to_string(), Some(bytes.to_vec())));
            rest = rest.get(start + size + 1..).unwrap_or_default();
        }
        Ok(result)
    }
}

fn parse_log(output: &str) -> Vec<CommitSummary> {
    output
        .split('\x1e')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let mut fields = record.splitn(6, '\x1f');
            let id = fields.next().filter(|id| !id.is_empty())?.to_string();
            let parents = fields
                .next()?
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let author = fields.next()?.to_string();
            let time = fields.next()?.parse().unwrap_or_default();
            let refs = parse_decorations(fields.next()?);
            let subject = fields.next()?.to_string();
            Some(CommitSummary {
                id,
                parents,
                author,
                time,
                subject,
                refs,
            })
        })
        .collect()
}

/// Refs from `%D` with `--decorate=full`, such as
/// `HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1`.
/// Local branches come first, then remote branches, then tags.
fn parse_decorations(decorations: &str) -> Vec<CommitRef> {
    let mut refs = Vec::new();
    for decoration in decorations.split(", ").filter(|d| !d.is_empty()) {
        let (current, name) = match decoration.strip_prefix("HEAD -> ") {
            Some(name) => (true, name),
            None => (false, decoration),
        };
        let name = name.strip_prefix("tag: ").unwrap_or(name);
        let (kind, short) = if name == "HEAD" {
            (RefKind::Head, "HEAD")
        } else if let Some(short) = name.strip_prefix("refs/heads/") {
            (RefKind::Branch, short)
        } else if let Some(short) = name.strip_prefix("refs/remotes/") {
            // A remote's default branch duplicates the branch it names.
            if short.ends_with("/HEAD") {
                continue;
            }
            (RefKind::Remote, short)
        } else if let Some(short) = name.strip_prefix("refs/tags/") {
            (RefKind::Tag, short)
        } else {
            // Stashes, grafts and other refs are not branches.
            continue;
        };
        refs.push(CommitRef {
            kind,
            name: short.to_string(),
            current,
        });
    }
    refs.sort_by_key(|r| match r.kind {
        RefKind::Head | RefKind::Branch => 0,
        RefKind::Remote => 1,
        RefKind::Tag => 2,
    });
    refs
}

struct RawEntry {
    /// Mode and object before and after.
    objects: [(String, String); 2],
    kind: FileChangeKind,
    path: PathBuf,
    old_path: Option<PathBuf>,
}

/// Records of `diff-tree -r -z -M --raw --no-abbrev`. A rename or copy names
/// its source and then its destination.
fn parse_raw_diff(raw: &[u8]) -> Vec<RawEntry> {
    let mut fields = raw.split(|b| *b == 0);
    let mut entries = Vec::new();
    while let Some(header) = fields.next() {
        let header = String::from_utf8_lossy(header);
        // ":<old mode> <new mode> <old object> <new object> <status>"
        let parts: Vec<_> = header.trim_start_matches(':').split(' ').collect();
        let [old_mode, new_mode, old_object, new_object, status] = parts.as_slice() else {
            break;
        };
        let path = |bytes: &[u8]| PathBuf::from(String::from_utf8_lossy(bytes).into_owned());
        let Some(first) = fields.next() else {
            break;
        };
        let kind = match status.chars().next() {
            Some('A') => FileChangeKind::Added,
            Some('D') => FileChangeKind::Deleted,
            Some('M') => FileChangeKind::Modified,
            Some('R') => FileChangeKind::Renamed,
            Some('C') => FileChangeKind::Copied,
            Some('T') => FileChangeKind::TypeChanged,
            _ => FileChangeKind::Other,
        };
        let (path, old_path) = if matches!(kind, FileChangeKind::Renamed | FileChangeKind::Copied) {
            let Some(second) = fields.next() else {
                break;
            };
            (path(second), Some(path(first)))
        } else {
            (path(first), None)
        };
        entries.push(RawEntry {
            objects: [
                (old_mode.to_string(), old_object.to_string()),
                (new_mode.to_string(), new_object.to_string()),
            ],
            kind,
            path,
            old_path,
        });
    }
    entries
}

/// Which half of a graph row a line is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    /// From the top edge to the middle of the row.
    Upper,
    /// From the middle of the row to the bottom edge.
    Lower,
}

/// A line between two lanes within half a row. A line whose lanes differ is
/// drawn as a curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphLine {
    pub half: Half,
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

/// One commit's row of the graph: its dot and the lines around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub color: usize,
    pub lines: Vec<GraphLine>,
    /// Lanes in use in this row, for its width.
    pub lanes: usize,
}

#[derive(Clone)]
struct Lane {
    /// The commit this lane leads to.
    commit: String,
    color: usize,
}

/// Assigns commits to lanes, one page at a time. Commits must come children
/// first, as `GitRepository::log` lists them. A lane keeps its column until it
/// ends, and new lanes take the leftmost free column.
#[derive(Default, Clone)]
pub struct CommitGraph {
    lanes: Vec<Option<Lane>>,
    next_color: usize,
}

impl CommitGraph {
    pub fn add(&mut self, commit: &CommitSummary) -> GraphRow {
        let mut lines = Vec::new();
        let width_before = self.lanes.len();
        let mut lane = None;
        for (index, slot) in self.lanes.iter_mut().enumerate() {
            let Some(existing) = slot else { continue };
            if existing.commit == commit.id {
                // Every lane leading to this commit joins its dot.
                let target = *lane.get_or_insert((index, existing.color));
                lines.push(GraphLine {
                    half: Half::Upper,
                    from: index,
                    to: target.0,
                    color: existing.color,
                });
                *slot = None;
            } else {
                lines.push(GraphLine {
                    half: Half::Upper,
                    from: index,
                    to: index,
                    color: existing.color,
                });
            }
        }
        // A branch tip starts a lane of its own.
        let (lane, color) = match lane {
            Some(lane) => lane,
            None => (self.free_lane(), self.new_color()),
        };
        let passing: Vec<usize> = (0..self.lanes.len())
            .filter(|index| self.lanes[*index].is_some())
            .collect();
        for (number, parent) in commit.parents.iter().enumerate() {
            // A parent another lane already leads to joins that lane.
            if let Some(index) = self
                .lanes
                .iter()
                .position(|slot| slot.as_ref().is_some_and(|l| &l.commit == parent))
            {
                let target_color = self.lanes[index].as_ref().map_or(color, |l| l.color);
                lines.push(GraphLine {
                    half: Half::Lower,
                    from: lane,
                    to: index,
                    color: if number == 0 { color } else { target_color },
                });
                continue;
            }
            // The first parent continues this commit's lane; merged parents
            // branch off into lanes of their own.
            let (index, parent_color) = if number == 0 && self.lane_is_free(lane) {
                (lane, color)
            } else {
                (self.free_lane(), self.new_color())
            };
            self.set_lane(
                index,
                Lane {
                    commit: parent.clone(),
                    color: parent_color,
                },
            );
            lines.push(GraphLine {
                half: Half::Lower,
                from: lane,
                to: index,
                color: parent_color,
            });
        }
        for index in passing {
            let color = self.lanes[index].as_ref().map_or(0, |l| l.color);
            lines.push(GraphLine {
                half: Half::Lower,
                from: index,
                to: index,
                color,
            });
        }
        let lanes = width_before.max(self.lanes.len()).max(lane + 1);
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        GraphRow {
            lane,
            color,
            lines,
            lanes,
        }
    }

    fn lane_is_free(&self, index: usize) -> bool {
        self.lanes.get(index).is_none_or(Option::is_none)
    }

    fn free_lane(&self) -> usize {
        self.lanes
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.lanes.len())
    }

    fn set_lane(&mut self, index: usize, lane: Lane) {
        if index >= self.lanes.len() {
            self.lanes.resize(index + 1, None);
        }
        self.lanes[index] = Some(lane);
    }

    fn new_color(&mut self) -> usize {
        self.next_color += 1;
        self.next_color - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> CommitSummary {
        CommitSummary {
            id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            time: 0,
            subject: String::new(),
            refs: Vec::new(),
        }
    }

    fn line(half: Half, from: usize, to: usize, color: usize) -> GraphLine {
        GraphLine {
            half,
            from,
            to,
            color,
        }
    }

    #[test]
    fn merges_fork_into_new_lanes_and_rejoin_at_their_base() {
        use Half::*;
        // m merges feature (f2, f1) into main (a); both branch from base.
        let history = [
            commit("m", &["a", "f2"]),
            commit("f2", &["f1"]),
            commit("a", &["base"]),
            commit("f1", &["base"]),
            commit("base", &[]),
        ];
        let mut graph = CommitGraph::default();
        let rows: Vec<_> = history.iter().map(|c| graph.add(c)).collect();
        // The merge stays in lane 0 and its second parent opens lane 1.
        assert_eq!(rows[0].lines, [line(Lower, 0, 0, 0), line(Lower, 0, 1, 1)]);
        assert_eq!((rows[1].lane, rows[1].color), (1, 1));
        assert_eq!((rows[2].lane, rows[2].color), (0, 0));
        assert_eq!((rows[3].lane, rows[3].color), (1, 1));
        // f1's parent is already lane 0's, so f1 joins it and lane 1 ends.
        assert!(rows[3].lines.contains(&line(Lower, 1, 0, 1)));
        assert_eq!(rows[4].lane, 0);
        assert_eq!(rows[4].lines, [line(Upper, 0, 0, 0)]);
        assert_eq!(rows[4].lanes, 1);
    }

    #[test]
    fn a_second_tip_takes_the_next_free_lane_and_both_join_their_parent() {
        use Half::*;
        // Two branch tips over one parent: b's lane curves into a's dot.
        let history = [commit("a", &["p"]), commit("b", &["p"]), commit("p", &[])];
        let mut graph = CommitGraph::default();
        let rows: Vec<_> = history.iter().map(|c| graph.add(c)).collect();
        assert_eq!((rows[1].lane, rows[1].color), (1, 1));
        // b's parent is already lane 0's, so b curves into it right away.
        assert_eq!(
            rows[1].lines,
            [
                line(Upper, 0, 0, 0),
                line(Lower, 1, 0, 1),
                line(Lower, 0, 0, 0)
            ]
        );
        assert_eq!(rows[2].lines, [line(Upper, 0, 0, 0)]);
    }

    fn git(root: &std::path::Path, args: &[&str]) -> String {
        let output = nucleotide_process::command("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    #[tokio::test]
    async fn log_pages_and_commit_changes_read_a_real_repository() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "History Test"]);
        git(root, &["config", "user.email", "history@example.com"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("old.txt"), "moved\nlines\nstay\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "first"]);
        git(root, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(root.join("a.txt"), "one\n2\n").unwrap();
        git(root, &["mv", "old.txt", "new.txt"]);
        git(
            root,
            &["commit", "-q", "-am", "edit and rename\n\nThe body."],
        );
        git(root, &["tag", "v1"]);
        let feature = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);
        let repo = GitRepository::discover(nucleotide_workspace::local_workspace_backend(), root)
            .await
            .unwrap()
            .unwrap();
        let tips = repo.history_tips().await.unwrap();
        assert_eq!(tips.len(), 2, "HEAD and main coincide: {tips:?}");
        let all = repo.log(&tips, 0, 10).await.unwrap();
        assert_eq!(
            all.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(),
            ["edit and rename", "first"]
        );
        assert_eq!(all[0].parents, [all[1].id.clone()]);
        assert_eq!(all[0].author, "History Test");
        let refs: Vec<_> = all[0].refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(refs, ["feature", "v1"]);
        assert!(all[1].refs[0].current && all[1].refs[0].name == "main");
        // Pages continue where the previous one stopped.
        assert_eq!(repo.log(&tips, 1, 10).await.unwrap(), all[1..]);

        let changes = repo.commit_changes(&feature).await.unwrap();
        assert_eq!(changes.body, "The body.");
        let files: Vec<_> = changes
            .files
            .iter()
            .map(|f| (f.path.to_str().unwrap(), f.kind, f.old_path.clone()))
            .collect();
        assert_eq!(
            files,
            [
                ("a.txt", FileChangeKind::Modified, None),
                (
                    "new.txt",
                    FileChangeKind::Renamed,
                    Some(PathBuf::from("old.txt"))
                ),
            ]
        );
        let [old, new] = changes.files[0].contents.clone().unwrap();
        assert_eq!(
            (old.unwrap(), new.unwrap()),
            (b"one\ntwo\n".to_vec(), b"one\n2\n".to_vec())
        );
        // A root commit adds every file.
        let root_changes = repo.commit_changes(&all[1].id).await.unwrap();
        assert!(
            root_changes
                .files
                .iter()
                .all(|f| f.kind == FileChangeKind::Added
                    && f.contents.as_ref().unwrap()[0].is_none())
        );
    }

    #[test]
    fn decorations_name_branches_remotes_and_tags() {
        let refs = parse_decorations(
            "tag: refs/tags/v0.17.0, HEAD -> refs/heads/main, refs/remotes/origin/main, refs/remotes/origin/HEAD, grafted, refs/heads/feature/x",
        );
        let names: Vec<_> = refs
            .iter()
            .map(|r| (r.kind, r.name.as_str(), r.current))
            .collect();
        assert_eq!(
            names,
            [
                (RefKind::Branch, "main", true),
                (RefKind::Branch, "feature/x", false),
                (RefKind::Remote, "origin/main", false),
                (RefKind::Tag, "v0.17.0", false),
            ]
        );
        assert_eq!(parse_decorations("HEAD")[0].kind, RefKind::Head);
    }
}
