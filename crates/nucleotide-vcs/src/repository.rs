// ABOUTME: Git index, commit and push operations, each run as a git process through the workspace backend
// ABOUTME: Works the same in local, WSL and remote workspaces; paths are relative to the repository root

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use nucleotide_workspace::{ProcessOutput, ProcessSpec, WorkspaceBackendHandle};

use crate::staging::MAX_LINE_STAGING_BYTES;

const METADATA_OUTPUT_LIMIT: usize = 64 * 1024;
const READ_TIMEOUT_MS: u64 = 10_000;
const WRITE_TIMEOUT_MS: u64 = 30_000;
/// Commit hooks may run formatters or tests.
const COMMIT_TIMEOUT_MS: u64 = 10 * 60_000;
const PUSH_TIMEOUT_MS: u64 = 2 * 60_000;

/// The branch's upstream and how far the two have diverged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpstreamState {
    /// The current branch; `None` for a detached HEAD.
    pub branch: Option<String>,
    /// The upstream, such as `origin/main`; `None` when there is none.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// Message defaults read on the repository's host, including linked worktrees.
#[derive(Clone)]
pub struct CommitMessageContext {
    pub seed: String,
    pub last_message: Option<String>,
    pub comment_prefix: String,
}

/// A file's HEAD and index contents in working-tree form, `None` when absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersions {
    pub head: Option<Vec<u8>>,
    pub index: Option<Vec<u8>>,
    /// The file mode differs between HEAD and the index or the working tree.
    pub mode_changed: bool,
}

/// A git repository reached through a workspace backend.
#[derive(Clone)]
pub struct GitRepository {
    backend: WorkspaceBackendHandle,
    root: PathBuf,
}

impl GitRepository {
    /// The repository containing directory `dir`, or `None` outside a repository.
    pub async fn discover(
        backend: WorkspaceBackendHandle,
        dir: &Path,
    ) -> Result<Option<Self>, String> {
        let probe = Self {
            backend,
            root: dir.to_path_buf(),
        };
        let output = probe
            .run(&["rev-parse", "--show-prefix"], Vec::new(), READ_TIMEOUT_MS)
            .await?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not a git repository") {
                return Ok(None);
            }
            return Err(command_error("rev-parse", &output));
        }
        // Strip the prefix's components from `dir` rather than joining `..`,
        // which keeps remote display paths intact.
        let prefix = String::from_utf8_lossy(&output.stdout);
        let mut root = dir;
        for _ in prefix.trim().split('/').filter(|part| !part.is_empty()) {
            root = root.parent().ok_or_else(|| {
                format!("Cannot find the repository root above {}", dir.display())
            })?;
        }
        Ok(Some(Self {
            root: root.to_path_buf(),
            backend: probe.backend,
        }))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A file's contents in HEAD, in working-tree form (with smudge filters
    /// such as CRLF conversion applied), or `None` when HEAD has no such file.
    pub async fn read_head(&self, path: &Path) -> Result<Option<Vec<u8>>, String> {
        let path = git_path(path)?;
        let Some(head) = self
            .optional_line(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .await?
        else {
            return Ok(None);
        };
        let entry = self
            .git(
                &["--literal-pathspecs", "ls-tree", "-z", &head, "--", &path],
                READ_TIMEOUT_MS,
            )
            .await?;
        if entry.is_empty() {
            return Ok(None);
        }
        self.read_blob(format!("{head}:{path}"), None)
            .await
            .map(Some)
    }

    /// A regular file's staged contents in working-tree form, or `None` when
    /// absent. Conflicts and unsupported file modes are errors, not empty files.
    pub async fn read_index(&self, path: &Path) -> Result<Option<Vec<u8>>, String> {
        let path = git_path(path)?;
        if self.index_mode(&path).await?.is_none() {
            return Ok(None);
        }
        self.read_blob(format!(":0:{path}"), None).await.map(Some)
    }

    /// HEAD and index contents of each path, as `read_head` and `read_index`
    /// return them, and whether its file mode changed. Uses a fixed number of
    /// git processes however many paths there are. Unsupported paths and
    /// unreadable entries fail individually; git failures fail the whole read.
    pub async fn read_versions(
        &self,
        paths: &[&Path],
    ) -> Result<Vec<Result<FileVersions, String>>, String> {
        let names: Vec<_> = paths.iter().map(|path| git_path(path)).collect();
        let valid: Vec<&str> = names.iter().flatten().map(String::as_str).collect();
        if valid.is_empty() {
            // Without paths, git would list every file.
            return Ok(names
                .into_iter()
                .map(|name| Err(name.err().unwrap_or_default()))
                .collect());
        }
        let head = self
            .optional_line(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .await?;
        // Object, or why it cannot be read line by line, of each path in HEAD and the index.
        let mut head_objects: HashMap<String, Result<String, String>> = HashMap::new();
        let mut index_objects: HashMap<String, Result<String, String>> = HashMap::new();
        let mut mode_changed = HashSet::new();
        if let Some(head) = &head {
            let mut args = vec!["--literal-pathspecs", "ls-tree", "-z", head, "--"];
            args.extend(&valid);
            let tree = self.read_paths(&args).await?;
            for (header, path) in tab_records(&tree) {
                // "<mode> <type> <object>"
                let fields: Vec<_> = header.split(' ').collect();
                if let [_, kind, object] = fields.as_slice() {
                    head_objects.insert(
                        path.to_string(),
                        (*kind == "blob")
                            .then(|| object.to_string())
                            .ok_or_else(|| format!("{path} in HEAD is not a file")),
                    );
                }
            }
            for cached in [false, true] {
                let mut args = vec![
                    "--literal-pathspecs",
                    "diff",
                    "--raw",
                    "-z",
                    "--no-ext-diff",
                    "--no-renames",
                ];
                if cached {
                    args.push("--cached");
                }
                args.extend([head.as_str(), "--"]);
                args.extend(&valid);
                let raw = self.read_paths(&args).await?;
                for (header, path) in raw_diff_records(&raw) {
                    let header = String::from_utf8_lossy(header);
                    let mut fields = header.trim_start_matches(':').split_whitespace();
                    if let (Some(before), Some(after)) = (fields.next(), fields.next())
                        && before != "000000"
                        && after != "000000"
                        && before != after
                    {
                        mode_changed.insert(String::from_utf8_lossy(path).into_owned());
                    }
                }
            }
        }
        let mut args = vec!["--literal-pathspecs", "ls-files", "--stage", "-z", "--"];
        args.extend(&valid);
        let index = self.read_paths(&args).await?;
        for (header, path) in tab_records(&index) {
            // "<mode> <object> <stage>". Never replace conflict stages or a
            // symlink/submodule with contents from a text diff.
            let fields: Vec<_> = header.split(' ').collect();
            let object = match fields.as_slice() {
                [mode, object, "0"] if matches!(*mode, "100644" | "100755") => {
                    Ok(object.to_string())
                }
                _ => Err(format!(
                    "{path} is not a merged regular file; stage it as a whole file"
                )),
            };
            // Conflicts list one entry per stage.
            let object = match index_objects.contains_key(path) {
                true => Err(format!("{path} has conflicts; stage it as a whole file")),
                false => object,
            };
            index_objects.insert(path.to_string(), object);
        }
        let mut blobs = Vec::new();
        for path in &valid {
            for object in [head_objects.get(*path), index_objects.get(*path)]
                .into_iter()
                .flatten()
                .flatten()
            {
                blobs.push((object.as_str(), *path));
            }
        }
        // HEAD and the index often hold the same blob.
        blobs.sort_unstable();
        blobs.dedup();
        let contents = self.read_blobs(&blobs).await?;
        let blob = |object: Option<&Result<String, String>>, path: &str| match object {
            None => Ok(None),
            Some(Err(error)) => Err(error.clone()),
            Some(Ok(object)) => contents
                .get(&(object.clone(), path.to_string()))
                .cloned()
                .unwrap_or_else(|| Err(format!("Could not read {path}")))
                .map(Some),
        };
        Ok(names
            .into_iter()
            .map(|name| {
                let name = name?;
                Ok(FileVersions {
                    head: blob(head_objects.get(name.as_str()), &name)?,
                    index: blob(index_objects.get(name.as_str()), &name)?,
                    mode_changed: mode_changed.contains(&name),
                })
            })
            .collect())
    }

    /// The stdout of a read that lists many paths.
    async fn read_paths(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let output = self
            .run_limited(args, READ_TIMEOUT_MS, 16 * 1024 * 1024)
            .await?;
        if !output.success || output.stdout_truncated {
            return Err(command_error(args[1], &output));
        }
        Ok(output.stdout)
    }

    /// Blobs in working-tree form, read by `(object, path)` with two git
    /// processes. Blobs too large to stage line by line fail individually.
    async fn read_blobs(
        &self,
        blobs: &[(&str, &str)],
    ) -> Result<HashMap<(String, String), Result<Vec<u8>, String>>, String> {
        let mut contents = HashMap::new();
        // Batch input is one line per blob; read any other path on its own.
        let (batch, single): (Vec<_>, Vec<_>) =
            blobs.iter().partition(|(_, path)| !path.contains('\n'));
        for (object, path) in single {
            let content = self.read_blob(object.to_string(), Some(path)).await;
            contents.insert((object.to_string(), path.to_string()), content);
        }
        if batch.is_empty() {
            return Ok(contents);
        }
        let objects: String = batch
            .iter()
            .map(|(object, _)| format!("{object}\n"))
            .collect();
        let sizes = self
            .run(
                &["cat-file", "--batch-check=%(objectsize)"],
                objects.into_bytes(),
                READ_TIMEOUT_MS,
            )
            .await?;
        if !sizes.success || sizes.stdout_truncated {
            return Err(command_error("cat-file", &sizes));
        }
        let sizes: Vec<usize> = String::from_utf8_lossy(&sizes.stdout)
            .lines()
            .map(|line| line.parse().unwrap_or(usize::MAX))
            .collect();
        let mut input = String::new();
        let mut limit = 0;
        let mut read = Vec::new();
        for ((object, path), size) in batch.into_iter().zip(sizes) {
            if size > MAX_LINE_STAGING_BYTES {
                contents.insert(
                    (object.to_string(), path.to_string()),
                    Err(format!("{path} is too large to stage line by line")),
                );
            } else {
                input.push_str(&format!("{object} {path}\n"));
                // Smudge filters such as CRLF conversion can double the size.
                limit += 2 * size + 128;
                read.push((object, path, size));
            }
        }
        if read.is_empty() {
            return Ok(contents);
        }
        let output = self
            .run_process(
                &["cat-file", "--batch", "--filters"],
                input.into_bytes(),
                READ_TIMEOUT_MS,
                limit,
            )
            .await?;
        if !output.success || output.stdout_truncated {
            return Err(command_error("cat-file", &output));
        }
        // Each blob is "<object> blob <size>\n<contents>\n", but the header
        // gives the size before filters. Use the batch only when every
        // boundary falls where the unfiltered sizes put it, which means no
        // filter changed a size; otherwise read each blob on its own.
        let mut rest = output.stdout.as_slice();
        let mut filtered = Vec::new();
        for (object, _, size) in &read {
            let Some(blob) = rest
                .strip_prefix(format!("{object} blob {size}\n").as_bytes())
                .filter(|blob| blob.get(*size) == Some(&b'\n'))
            else {
                break;
            };
            filtered.push(blob[..*size].to_vec());
            rest = &blob[size + 1..];
        }
        if filtered.len() != read.len() || !rest.is_empty() {
            for (object, path, _) in read {
                let content = self.read_blob(object.to_string(), Some(path)).await;
                contents.insert((object.to_string(), path.to_string()), content);
            }
            return Ok(contents);
        }
        for ((object, path, _), content) in read.into_iter().zip(filtered) {
            contents.insert((object.to_string(), path.to_string()), Ok(content));
        }
        Ok(contents)
    }

    /// Replaces a file's staged contents. `contents` is in working-tree form;
    /// git's clean filters convert it as `git add` would.
    /// This is a low-level replacement: callers must serialize their mutations
    /// and refresh the HEAD/index/worktree snapshot before applying a selection.
    /// On an uncertain transport outcome, reread the index rather than retrying.
    pub async fn write_index(&self, path: &Path, contents: Vec<u8>) -> Result<(), String> {
        let path = git_path(path)?;
        let mode = self.index_mode(&path).await?;
        let path_option = format!("--path={path}");
        let output = self
            .run(
                &["hash-object", "-w", "--stdin", &path_option],
                contents,
                WRITE_TIMEOUT_MS,
            )
            .await?;
        if !output.success {
            return Err(command_error("hash-object", &output));
        }
        let object = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mode = match mode {
            Some(mode) => mode,
            None => {
                // Do not touch the index until clean filters have succeeded.
                // Intent-to-add asks git to determine the working-tree mode.
                self.git(
                    &["--literal-pathspecs", "add", "--intent-to-add", "--", &path],
                    WRITE_TIMEOUT_MS,
                )
                .await?;
                self.index_mode(&path)
                    .await?
                    .ok_or_else(|| format!("git add --intent-to-add did not stage {path}"))?
            }
        };
        let entry = format!("{mode},{object},{path}");
        self.git(
            &["update-index", "--add", "--cacheinfo", &entry],
            WRITE_TIMEOUT_MS,
        )
        .await
        .map(drop)
    }

    /// Stages whole files, including deletions and untracked files.
    pub async fn stage(&self, paths: &[&Path]) -> Result<(), String> {
        self.git_with_paths(&["add", "--all"], paths).await
    }

    /// Unstages whole files, leaving the working tree unchanged. A file that is
    /// not in HEAD leaves the index and becomes untracked again.
    pub async fn unstage(&self, paths: &[&Path]) -> Result<(), String> {
        self.git_with_paths(&["reset", "--quiet"], paths).await
    }

    /// A read-only snapshot of staged changes, including conflict stages.
    pub async fn index_signature(&self) -> Result<Vec<u8>, String> {
        let output = self
            .run_limited(
                &[
                    "diff",
                    "--cached",
                    "--raw",
                    "-z",
                    "--no-abbrev",
                    "--no-renames",
                    "--no-ext-diff",
                ],
                READ_TIMEOUT_MS,
                16 * 1024 * 1024,
            )
            .await?;
        if !output.success || output.stdout_truncated {
            return Err(command_error("diff --cached", &output));
        }
        Ok(output.stdout)
    }

    pub async fn message_context(&self) -> Result<CommitMessageContext, String> {
        let seed_path = if self
            .optional_line(&["rev-parse", "--verify", "--quiet", "MERGE_HEAD"])
            .await?
            .is_some()
        {
            let path = self
                .git(&["rev-parse", "--git-path", "MERGE_MSG"], READ_TIMEOUT_MS)
                .await?;
            Some(self.root.join(String::from_utf8_lossy(&path).trim()))
        } else {
            self.optional_line(&["config", "--path", "--get", "commit.template"])
                .await?
                .map(|path| self.root.join(path))
        };
        let seed = if let Some(path) = seed_path {
            let read = self
                .backend
                .read_file(
                    &path,
                    nucleotide_workspace::ReadOptions {
                        max_bytes: Some(METADATA_OUTPUT_LIMIT as u64),
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            if read.truncated {
                return Err("Commit message template exceeds 64 KiB".into());
            }
            String::from_utf8(read.bytes).map_err(|e| e.to_string())?
        } else {
            String::new()
        };
        let last_message = if self
            .optional_line(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .await?
            .is_some()
        {
            Some(
                String::from_utf8(
                    self.git(&["log", "-1", "--format=%B"], READ_TIMEOUT_MS)
                        .await?,
                )
                .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let comment_prefix = match self
            .optional_line(&["config", "--get", "core.commentString"])
            .await?
        {
            Some(prefix) => prefix,
            None => self
                .optional_line(&["config", "--get", "core.commentChar"])
                .await?
                .filter(|prefix| prefix != "auto")
                .unwrap_or_else(|| "#".into()),
        };
        Ok(CommitMessageContext {
            seed,
            last_message,
            comment_prefix,
        })
    }

    /// Commits the index. Comment lines and surrounding blank lines are
    /// stripped from `message`, as in git's own editor flow.
    pub async fn commit(&self, message: &str, amend: bool) -> Result<(), String> {
        let mut args = vec!["commit", "--cleanup=strip", "--file=-"];
        if amend {
            args.push("--amend");
        }
        let output = self
            .run(&args, message.as_bytes().to_vec(), COMMIT_TIMEOUT_MS)
            .await?;
        if output.success {
            Ok(())
        } else {
            Err(command_error("commit", &output))
        }
    }

    pub async fn upstream_state(&self) -> Result<UpstreamState, String> {
        let branch = self.current_branch().await?;
        let upstream = match &branch {
            Some(branch) => {
                let output = self
                    .git(
                        &[
                            "for-each-ref",
                            "--format=%(upstream:short)",
                            &format!("refs/heads/{branch}"),
                        ],
                        READ_TIMEOUT_MS,
                    )
                    .await?;
                let upstream = String::from_utf8_lossy(&output).trim().to_string();
                (!upstream.is_empty()).then_some(upstream)
            }
            None => None,
        };
        let mut state = UpstreamState {
            branch,
            upstream,
            ..UpstreamState::default()
        };
        if state.upstream.is_some() {
            let counts = self
                .git(
                    &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
                    READ_TIMEOUT_MS,
                )
                .await?;
            let counts = String::from_utf8_lossy(&counts);
            let mut counts = counts.split_whitespace().map(str::parse::<u32>);
            match (counts.next(), counts.next()) {
                (Some(Ok(behind)), Some(Ok(ahead))) => {
                    state.behind = behind;
                    state.ahead = ahead;
                }
                _ => return Err(format!("Unexpected git rev-list output: {counts:?}")),
            }
        }
        Ok(state)
    }

    /// Pushes only the current branch, without force, mirror or extra tag
    /// refspecs. Preserves the user's SSH setup; terminal prompting is disabled
    /// and a timeout bounds credential helpers and SSH askpass programs.
    pub async fn push(&self) -> Result<(), String> {
        let branch = self
            .current_branch()
            .await?
            .ok_or_else(|| "Cannot push a detached HEAD; check out a branch first".to_string())?;
        let remote = self.push_remote(&branch).await?;
        let tracking_remote = self
            .optional_line(&["config", "--get", &format!("branch.{branch}.remote")])
            .await?;
        let tracking_ref = self
            .optional_line(&["config", "--get", &format!("branch.{branch}.merge")])
            .await?;
        let has_upstream = tracking_remote.is_some() && tracking_ref.is_some();
        let destination = if tracking_remote.as_deref() == Some(remote.as_str()) {
            tracking_ref.unwrap_or_else(|| format!("refs/heads/{branch}"))
        } else {
            format!("refs/heads/{branch}")
        };
        let refspec = format!("HEAD:{destination}");
        let mut args = vec![
            "push",
            "--no-force",
            "--no-mirror",
            "--no-follow-tags",
            "--recurse-submodules=no",
        ];
        if !has_upstream {
            args.push("--set-upstream");
        }
        args.extend(["--", remote.as_str(), refspec.as_str()]);
        let head = self
            .optional_line(&["rev-parse", "--verify", "HEAD"])
            .await?;
        let output = match self.run(&args, Vec::new(), PUSH_TIMEOUT_MS).await {
            Ok(output) => output,
            Err(error) => {
                // A timeout or disconnected transport can happen after the
                // remote accepted the commit. Inspect, never automatically retry.
                let probe = async {
                    // Fetch and push URLs may differ, and Git pushes to every
                    // configured push URL. Inspect those destinations, not the
                    // fetch URL that ls-remote <remote> would otherwise use.
                    let urls = if remote == "." {
                        remote.clone()
                    } else {
                        String::from_utf8(
                            self.git(
                                &["remote", "get-url", "--push", "--all", &remote],
                                READ_TIMEOUT_MS,
                            )
                            .await?,
                        )
                        .map_err(|e| e.to_string())?
                    };
                    let mut states = Vec::new();
                    for url in urls.lines() {
                        let bytes = self
                            .git(
                                &["ls-remote", "--refs", "--", url, &destination],
                                READ_TIMEOUT_MS,
                            )
                            .await?;
                        let refs = String::from_utf8_lossy(&bytes);
                        let state = match refs.split_whitespace().next() {
                            Some(tip) if Some(tip) == head.as_deref() => {
                                "The remote already contains this commit; do not commit again."
                                    .to_string()
                            }
                            Some(tip) => format!(
                                "The remote currently points to {tip}. Review it before retrying Push."
                            ),
                            None => "The destination branch is absent on the remote.".to_string(),
                        };
                        states.push(format!("Push destination {}: {state}", states.len() + 1));
                    }
                    Ok::<_, String>(states.join("\n"))
                }
                .await;
                let remote_state = match probe {
                    Ok(state) => state,
                    Err(probe) => format!(
                        "Remote state could not be confirmed: {probe}. Inspect the remote before retrying."
                    ),
                };
                return Err(format!("{error}\n{remote_state}"));
            }
        };
        if output.success {
            Ok(())
        } else {
            Err(command_error("push", &output))
        }
    }

    async fn push_remote(&self, branch: &str) -> Result<String, String> {
        let branch_setting = format!("branch.{branch}.pushRemote");
        let tracking_setting = format!("branch.{branch}.remote");
        for setting in [
            branch_setting.as_str(),
            "remote.pushDefault",
            tracking_setting.as_str(),
        ] {
            if let Some(remote) = self.optional_line(&["config", "--get", setting]).await? {
                return Ok(remote);
            }
        }
        let remotes = self.git(&["remote"], READ_TIMEOUT_MS).await?;
        let remotes = String::from_utf8_lossy(&remotes);
        let remotes: Vec<&str> = remotes.lines().filter(|line| !line.is_empty()).collect();
        match remotes.as_slice() {
            [remote] => Ok((*remote).to_string()),
            [] => Err("This repository has no remote to push to".to_string()),
            _ => Err(format!(
                "Several remotes ({}); set remote.pushDefault to choose one",
                remotes.join(", ")
            )),
        }
    }

    async fn current_branch(&self) -> Result<Option<String>, String> {
        self.optional_line(&["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await
    }

    async fn index_mode(&self, path: &str) -> Result<Option<String>, String> {
        let output = self
            .git(
                &[
                    "--literal-pathspecs",
                    "ls-files",
                    "--stage",
                    "-z",
                    "--",
                    path,
                ],
                READ_TIMEOUT_MS,
            )
            .await?;
        if output.is_empty() {
            return Ok(None);
        }
        // "<mode> <object> <stage>\t<path>\0". Never replace conflict stages
        // or a symlink/submodule with contents from a text diff.
        let header = output
            .split(|byte| *byte == b'\t')
            .next()
            .unwrap_or_default();
        let fields: Vec<_> = header.split(|byte| *byte == b' ').collect();
        match fields.as_slice() {
            [mode, _, b"0"] if *mode == b"100644" || *mode == b"100755" => {
                Ok(Some(String::from_utf8_lossy(mode).into_owned()))
            }
            _ => Err(format!(
                "{path} is not a merged regular file; stage it as a whole file"
            )),
        }
    }

    /// A blob in working-tree form. `path` selects the filters for a bare object.
    async fn read_blob(&self, object: String, path: Option<&str>) -> Result<Vec<u8>, String> {
        let path = path.map(|path| format!("--path={path}"));
        let mut args = vec!["cat-file", "--filters"];
        args.extend(path.as_deref());
        args.push(&object);
        let output = self
            .run_limited(&args, READ_TIMEOUT_MS, MAX_LINE_STAGING_BYTES)
            .await?;
        if output.stdout_truncated {
            return Err(format!("{object} is too large to stage line by line"));
        }
        if !output.success {
            return Err(command_error("cat-file", &output));
        }
        Ok(output.stdout)
    }

    /// The first output line of a command that fails when the value is unset.
    async fn optional_line(&self, args: &[&str]) -> Result<Option<String>, String> {
        let output = self.run(args, Vec::new(), READ_TIMEOUT_MS).await?;
        if !output.success && output.status_code != Some(1) {
            return Err(command_error(args[0], &output));
        }
        if output.stdout_truncated {
            return Err(format!("git {} output exceeded the read limit", args[0]));
        }
        Ok(output
            .success
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|line| !line.is_empty()))
    }

    async fn git_with_paths(&self, args: &[&str], paths: &[&Path]) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let paths: Vec<String> = paths
            .iter()
            .map(|path| git_path(path))
            .collect::<Result<_, _>>()?;
        let mut full = vec!["--literal-pathspecs"];
        full.extend_from_slice(args);
        full.push("--");
        full.extend(paths.iter().map(String::as_str));
        self.git(&full, WRITE_TIMEOUT_MS).await.map(drop)
    }

    /// Runs git and returns its stdout, failing with git's stderr.
    async fn git(&self, args: &[&str], timeout_ms: u64) -> Result<Vec<u8>, String> {
        let output = self.run(args, Vec::new(), timeout_ms).await?;
        if output.stdout_truncated {
            return Err(format!("git {} output exceeded the read limit", args[0]));
        }
        if output.success {
            Ok(output.stdout)
        } else {
            Err(command_error(args[0], &output))
        }
    }

    async fn run(
        &self,
        args: &[&str],
        stdin: Vec<u8>,
        timeout_ms: u64,
    ) -> Result<ProcessOutput, String> {
        self.run_process(args, stdin, timeout_ms, METADATA_OUTPUT_LIMIT)
            .await
    }

    async fn run_limited(
        &self,
        args: &[&str],
        timeout_ms: u64,
        max_output_bytes: usize,
    ) -> Result<ProcessOutput, String> {
        self.run_process(args, Vec::new(), timeout_ms, max_output_bytes)
            .await
    }

    async fn run_process(
        &self,
        args: &[&str],
        stdin: Vec<u8>,
        timeout_ms: u64,
        max_output_bytes: usize,
    ) -> Result<ProcessOutput, String> {
        let output = self
            .backend
            .run_process(ProcessSpec {
                program: "git".to_string(),
                args: args.iter().map(|arg| (*arg).to_string()).collect(),
                cwd: self.root.clone(),
                env: BTreeMap::from([("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())]),
                clear_env: false,
                inherit_project_environment: false,
                stdin,
                max_output_bytes: Some(max_output_bytes),
                timeout_ms: Some(timeout_ms),
            })
            .await
            .map_err(|error| format!("Could not run git: {error}"))?;
        if output.timed_out {
            return Err(format!(
                "git {} timed out after {} seconds",
                args.first().copied().unwrap_or_default(),
                timeout_ms / 1000
            ));
        }
        Ok(output)
    }
}

/// The `(header, path)` records of a `--raw -z --no-renames` diff. Without
/// renames every header is followed by one path.
fn raw_diff_records(raw: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut fields = raw.split(|b| *b == 0);
    std::iter::from_fn(move || {
        let header = fields.next().filter(|header| !header.is_empty())?;
        Some((header, fields.next()?))
    })
}

/// The `(header, path)` records of `ls-tree -z` and `ls-files --stage -z`.
fn tab_records(raw: &[u8]) -> impl Iterator<Item = (&str, &str)> {
    raw.split(|b| *b == 0).filter_map(|record| {
        let record = std::str::from_utf8(record).ok()?;
        record.split_once('\t')
    })
}

fn command_error(operation: &str, output: &ProcessOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        format!("git {operation} failed: {}", stdout.trim())
    } else {
        format!("git {operation} failed: {stderr}")
    }
}

/// Git paths use `/` on every platform.
fn git_path(path: &Path) -> Result<String, String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("Expected a nonempty repository-relative path".to_string());
    }
    // ProcessSpec carries UTF-8 arguments; lossy conversion could stage a
    // different file whose name contains the replacement character.
    let path = path
        .to_str()
        .ok_or_else(|| "Git paths must be UTF-8 for workspace processes".to_string())?;
    Ok(if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::staging::{FileChanges, Selection};
    use nucleotide_workspace::local_workspace_backend;

    fn git(root: &Path, args: &[&str]) -> String {
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
        String::from_utf8(output.stdout).unwrap()
    }

    fn init(root: &Path) {
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "Staging Test"]);
        git(root, &["config", "user.email", "staging@example.com"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        git(root, &["config", "core.autocrlf", "false"]);
    }

    fn repository_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        init(temp.path());
        for (path, contents) in files {
            let path = temp.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        git(temp.path(), &["add", "."]);
        git(temp.path(), &["commit", "-q", "-m", "initial"]);
        temp
    }

    async fn open(dir: &Path) -> GitRepository {
        GitRepository::discover(local_workspace_backend(), dir)
            .await
            .unwrap()
            .expect("a repository")
    }

    /// Stages exactly the given lines of `path`, the way the Commit tab will.
    async fn stage_lines(repo: &GitRepository, path: &str, removed: &[u32], added: &[u32]) {
        let path = Path::new(path);
        let head = repo.read_head(path).await.unwrap().unwrap_or_default();
        let worktree = std::fs::read(repo.root().join(path)).unwrap();
        let changes = FileChanges::new(&head, &worktree);
        let mut selection = Selection::default();
        for line in removed {
            selection.set_removal(*line, true);
        }
        for line in added {
            selection.set_addition(*line, true);
        }
        repo.write_index(path, changes.index_content(&selection))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn batched_reads_match_reading_each_path() {
        let temp = repository_with(&[
            (".gitattributes", "crlf.txt eol=crlf\n"),
            ("crlf.txt", "one\ntwo\n"),
            ("with space.txt", "head\n"),
            ("same.txt", "same\n"),
        ]);
        let root = temp.path();
        std::fs::write(root.join("with space.txt"), "staged\n").unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        git(root, &["add", "with space.txt", "new.txt"]);
        std::fs::write(root.join("with space.txt"), "worktree\n").unwrap();
        std::fs::write(root.join("untracked.txt"), "untracked\n").unwrap();
        let repo = open(root).await;
        // Unfiltered files come from one batch; a filter that changes sizes
        // makes the batch fall back to reading each blob.
        for paths in [
            &["with space.txt", "same.txt", "new.txt", "untracked.txt"][..],
            &["crlf.txt", "same.txt"],
        ] {
            let paths: Vec<_> = paths.iter().map(Path::new).collect();
            let versions = repo.read_versions(&paths).await.unwrap();
            for (path, versions) in paths.iter().zip(versions) {
                assert_eq!(
                    versions.unwrap(),
                    FileVersions {
                        head: repo.read_head(path).await.unwrap(),
                        index: repo.read_index(path).await.unwrap(),
                        mode_changed: false,
                    },
                    "{}",
                    path.display()
                );
            }
        }
        assert_eq!(
            repo.read_head(Path::new("crlf.txt"))
                .await
                .unwrap()
                .unwrap(),
            b"one\r\ntwo\r\n"
        );
        // Without paths git would read every file.
        assert!(repo.read_versions(&[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn discovers_the_root_from_a_subdirectory_and_nothing_outside() {
        let temp = repository_with(&[("sub/dir/file.txt", "x\n")]);
        let repo = open(&temp.path().join("sub/dir")).await;
        assert_eq!(repo.root(), temp.path());

        let outside = tempfile::tempdir().unwrap();
        assert!(
            GitRepository::discover(local_workspace_backend(), outside.path())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn staging_selected_lines_stages_only_those_lines() {
        let temp = repository_with(&[("src/lib.rs", "one\ntwo\nthree\nfour\n")]);
        std::fs::write(
            temp.path().join("src/lib.rs"),
            "one\nTWO\nthree\nfour\nfive\n",
        )
        .unwrap();
        let repo = open(temp.path()).await;
        // Stage the new last line, but neither side of the TWO change.
        stage_lines(&repo, "src/lib.rs", &[], &[4]).await;

        assert_eq!(
            git(temp.path(), &["show", ":src/lib.rs"]),
            "one\ntwo\nthree\nfour\nfive\n"
        );
        assert_eq!(
            git(temp.path(), &["diff", "--name-only"]),
            "src/lib.rs\n",
            "TWO should remain unstaged"
        );

        let head = repo
            .read_head(Path::new("src/lib.rs"))
            .await
            .unwrap()
            .unwrap();
        let index = repo
            .read_index(Path::new("src/lib.rs"))
            .await
            .unwrap()
            .unwrap();
        let worktree = std::fs::read(temp.path().join("src/lib.rs")).unwrap();
        let selection = FileChanges::new(&head, &worktree)
            .selection_from_index(&index)
            .unwrap();
        assert!(selection.is_addition_staged(4));
        assert!(!selection.is_addition_staged(1));
        assert!(!selection.is_removal_staged(1));
    }

    #[tokio::test]
    async fn line_staging_applies_crlf_conversion_like_git_add() {
        let temp = tempfile::tempdir().unwrap();
        init(temp.path());
        git(temp.path(), &["config", "core.autocrlf", "true"]);
        std::fs::write(temp.path().join("notes.txt"), "a\r\nb\r\n").unwrap();
        git(temp.path(), &["add", "."]);
        git(temp.path(), &["commit", "-q", "-m", "initial"]);
        std::fs::write(temp.path().join("notes.txt"), "a\r\nx\r\nb\r\ny\r\n").unwrap();
        let repo = open(temp.path()).await;

        let head = repo
            .read_head(Path::new("notes.txt"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head, b"a\r\nb\r\n", "HEAD is read in working-tree form");
        stage_lines(&repo, "notes.txt", &[], &[1]).await;

        let staged = nucleotide_process::command("git")
            .args(["cat-file", "blob", ":notes.txt"])
            .current_dir(temp.path())
            .output()
            .unwrap()
            .stdout;
        assert_eq!(staged, b"a\nx\nb\n", "the index holds LF endings");
    }

    #[tokio::test]
    async fn untracked_files_stage_line_by_line_and_unstage_back_to_untracked() {
        let temp = repository_with(&[("kept.txt", "kept\n")]);
        std::fs::write(temp.path().join("new.txt"), "first\nsecond\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                temp.path().join("new.txt"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let repo = open(temp.path()).await;
        assert_eq!(repo.read_head(Path::new("new.txt")).await.unwrap(), None);
        assert_eq!(repo.read_index(Path::new("new.txt")).await.unwrap(), None);

        stage_lines(&repo, "new.txt", &[], &[1]).await;
        assert_eq!(git(temp.path(), &["show", ":new.txt"]), "second\n");
        assert_eq!(git(temp.path(), &["status", "--porcelain"]), "AM new.txt\n");
        #[cfg(unix)]
        assert!(git(temp.path(), &["ls-files", "--stage", "new.txt"]).starts_with("100755 "));

        repo.unstage(&[Path::new("new.txt")]).await.unwrap();
        assert_eq!(git(temp.path(), &["status", "--porcelain"]), "?? new.txt\n");
    }

    #[tokio::test]
    async fn unborn_repository_stages_and_unstages_but_bad_index_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        init(temp.path());
        std::fs::write(temp.path().join("new.txt"), "one\ntwo\n").unwrap();
        let repo = open(temp.path()).await;
        assert_eq!(repo.read_head(Path::new("new.txt")).await.unwrap(), None);
        stage_lines(&repo, "new.txt", &[], &[1]).await;
        assert_eq!(git(temp.path(), &["show", ":new.txt"]), "two\n");
        repo.unstage(&[Path::new("new.txt")]).await.unwrap();
        assert_eq!(repo.read_index(Path::new("new.txt")).await.unwrap(), None);
        std::fs::write(temp.path().join(".git/index"), b"invalid index").unwrap();
        assert!(repo.read_index(Path::new("new.txt")).await.is_err());
    }

    #[tokio::test]
    async fn empty_path_lists_do_not_change_the_index() {
        let temp = repository_with(&[("one.txt", "one\n"), ("two.txt", "two\n")]);
        std::fs::write(temp.path().join("one.txt"), "staged\n").unwrap();
        std::fs::write(temp.path().join("two.txt"), "unstaged\n").unwrap();
        git(temp.path(), &["add", "one.txt"]);
        let repo = open(temp.path()).await;
        let before = git(temp.path(), &["write-tree"]);
        repo.stage(&[]).await.unwrap();
        assert_eq!(git(temp.path(), &["write-tree"]), before);
        repo.unstage(&[]).await.unwrap();
        assert_eq!(git(temp.path(), &["write-tree"]), before);
        assert!(repo.stage(&[Path::new("")]).await.is_err());
        assert!(repo.stage(&[Path::new("../one.txt")]).await.is_err());
        assert_eq!(git(temp.path(), &["write-tree"]), before);
    }

    #[tokio::test]
    async fn partial_staging_does_not_resolve_conflicts() {
        let temp = repository_with(&[("file.txt", "base\n")]);
        let object = git(temp.path(), &["rev-parse", "HEAD:file.txt"]);
        let mut child = nucleotide_process::command("git")
            .args(["update-index", "--index-info"])
            .current_dir(temp.path())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        write!(
            child.stdin.take().unwrap(),
            "0 {}\tfile.txt\n100644 {} 1\tfile.txt\n100644 {} 2\tfile.txt\n",
            object.trim(),
            object.trim(),
            object.trim()
        )
        .unwrap();
        assert!(child.wait().unwrap().success());
        let before = git(temp.path(), &["ls-files", "--stage"]);
        let repo = open(temp.path()).await;
        assert!(
            repo.write_index(Path::new("file.txt"), b"replacement\n".to_vec())
                .await
                .is_err()
        );
        assert_eq!(git(temp.path(), &["ls-files", "--stage"]), before);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn partial_staging_uses_literal_paths_and_failed_filters_leave_index_unchanged() {
        let temp = repository_with(&[("kept.txt", "kept\n")]);
        std::fs::write(temp.path().join("[x].txt"), "first\nsecond\n").unwrap();
        std::fs::write(temp.path().join("x.txt"), "other\n").unwrap();
        let repo = open(temp.path()).await;
        repo.write_index(Path::new("[x].txt"), b"second\n".to_vec())
            .await
            .unwrap();
        assert_eq!(git(temp.path(), &["show", ":[x].txt"]), "second\n");
        assert_eq!(git(temp.path(), &["ls-files", "x.txt"]), "");

        git(temp.path(), &["config", "filter.reject.clean", "exit 1"]);
        git(temp.path(), &["config", "filter.reject.required", "true"]);
        std::fs::write(
            temp.path().join(".gitattributes"),
            "bad.txt filter=reject\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("bad.txt"), "not staged\n").unwrap();
        let before = git(temp.path(), &["ls-files", "--stage"]);
        assert!(
            repo.write_index(Path::new("bad.txt"), b"partial\n".to_vec())
                .await
                .is_err()
        );
        assert_eq!(git(temp.path(), &["ls-files", "--stage"]), before);
    }

    #[tokio::test]
    async fn whole_file_staging_includes_deletions() {
        let temp = repository_with(&[("gone.txt", "x\n"), ("edit.txt", "a\n")]);
        std::fs::remove_file(temp.path().join("gone.txt")).unwrap();
        std::fs::write(temp.path().join("edit.txt"), "b\n").unwrap();
        let repo = open(temp.path()).await;

        repo.stage(&[Path::new("gone.txt"), Path::new("edit.txt")])
            .await
            .unwrap();
        assert_eq!(
            git(temp.path(), &["status", "--porcelain"]),
            "M  edit.txt\nD  gone.txt\n"
        );
        repo.unstage(&[Path::new("gone.txt")]).await.unwrap();
        assert_eq!(
            git(temp.path(), &["status", "--porcelain"]),
            "M  edit.txt\n D gone.txt\n"
        );
    }

    #[tokio::test]
    async fn commit_strips_comments_and_amend_replaces_the_commit() {
        let temp = repository_with(&[("file.txt", "a\n")]);
        std::fs::write(temp.path().join("file.txt"), "b\n").unwrap();
        let repo = open(temp.path()).await;
        repo.stage(&[Path::new("file.txt")]).await.unwrap();

        repo.commit("feat: change a to b\n\n# a comment\nBody line.\n", false)
            .await
            .unwrap();
        assert_eq!(
            git(temp.path(), &["log", "-1", "--format=%B"]),
            "feat: change a to b\n\nBody line.\n\n"
        );
        assert_eq!(git(temp.path(), &["status", "--porcelain"]), "");

        repo.commit("fix: reword", true).await.unwrap();
        assert_eq!(git(temp.path(), &["rev-list", "--count", "HEAD"]), "2\n");
        assert_eq!(
            git(temp.path(), &["log", "-1", "--format=%s"]),
            "fix: reword\n"
        );

        let error = repo.commit("nothing staged", false).await.unwrap_err();
        assert!(error.starts_with("git commit failed:"), "{error}");
    }

    #[tokio::test]
    async fn push_sets_an_upstream_then_reports_rejections_without_losing_the_commit() {
        let remote = tempfile::tempdir().unwrap();
        git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
        let temp = repository_with(&[("file.txt", "a\n")]);
        let remote_path = remote.path().to_str().unwrap();
        git(temp.path(), &["remote", "add", "origin", remote_path]);
        let repo = open(temp.path()).await;

        let state = repo.upstream_state().await.unwrap();
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert_eq!(state.upstream, None);

        repo.push().await.unwrap();
        let head = git(temp.path(), &["rev-parse", "HEAD"]);
        assert_eq!(git(remote.path(), &["rev-parse", "main"]), head);
        let state = repo.upstream_state().await.unwrap();
        assert_eq!(state.upstream.as_deref(), Some("origin/main"));
        assert_eq!((state.ahead, state.behind), (0, 0));

        // Someone else pushes, then we commit without pulling.
        let other = tempfile::tempdir().unwrap();
        git(other.path(), &["clone", "-q", remote_path, "."]);
        git(other.path(), &["config", "user.name", "Other"]);
        git(other.path(), &["config", "user.email", "other@example.com"]);
        git(
            other.path(),
            &["commit", "-q", "--allow-empty", "-m", "theirs"],
        );
        git(other.path(), &["push", "-q"]);
        git(temp.path(), &["fetch", "-q"]);
        git(
            temp.path(),
            &["commit", "-q", "--allow-empty", "-m", "ours"],
        );
        let ours = git(temp.path(), &["rev-parse", "HEAD"]);

        let state = repo.upstream_state().await.unwrap();
        assert_eq!((state.ahead, state.behind), (1, 1));
        // A configured force refspec must not turn this button into a force push.
        git(
            temp.path(),
            &["config", "remote.origin.push", "+HEAD:refs/heads/main"],
        );
        let error = repo.push().await.unwrap_err();
        assert!(error.contains("rejected"), "{error}");
        assert_eq!(git(temp.path(), &["rev-parse", "HEAD"]), ours);
    }

    #[tokio::test]
    async fn message_defaults_use_template_then_merge_message_in_linked_worktree() {
        let root = repository_with(&[("file", "base\n")]);
        let template = root.path().join("template");
        std::fs::write(&template, "; describe the change\n").unwrap();
        git(
            root.path(),
            &["config", "commit.template", template.to_str().unwrap()],
        );
        git(root.path(), &["config", "core.commentChar", ";"]);
        let repo = open(root.path()).await;
        let context = repo.message_context().await.unwrap();
        assert_eq!(context.seed, "; describe the change\n");
        assert_eq!(context.last_message.as_deref(), Some("initial\n\n"));
        assert_eq!(context.comment_prefix, ";");

        let linked = tempfile::tempdir().unwrap();
        git(
            root.path(),
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                linked.path().to_str().unwrap(),
            ],
        );
        let head = git(linked.path(), &["rev-parse", "HEAD"]);
        for (name, contents) in [
            ("MERGE_HEAD", head.as_str()),
            ("MERGE_MSG", "Merge topic\n"),
        ] {
            let path = git(linked.path(), &["rev-parse", "--git-path", name]);
            std::fs::write(linked.path().join(path.trim()), contents).unwrap();
        }
        let repo = open(linked.path()).await;
        assert_eq!(repo.message_context().await.unwrap().seed, "Merge topic\n");
    }

    #[tokio::test]
    async fn push_without_a_remote_explains_why() {
        let temp = repository_with(&[("file.txt", "a\n")]);
        let repo = open(temp.path()).await;
        assert_eq!(
            repo.push().await.unwrap_err(),
            "This repository has no remote to push to"
        );
    }
}
