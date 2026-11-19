use super::*;

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["config", "user.name", "Test"]);
    // Windows runners enable autocrlf globally; these fixtures write LF files.
    git(root.path(), &["config", "core.autocrlf", "false"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    std::fs::write(root.path().join("file"), "base\n").unwrap();
    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "-qm", "initial"]);
    root
}

#[tokio::test]
async fn stale_worktree_or_index_never_applies_a_reviewed_selection() {
    for change_index in [false, true] {
        let root = fixture();
        let path = root.path().join("file");
        std::fs::write(&path, "base\nreviewed\n").unwrap();
        let snapshot = load_review(
            local_workspace_backend(),
            root.path().into(),
            HashMap::new(),
            None,
        )
        .await
        .unwrap();
        std::fs::write(&path, "base\nexternal\n").unwrap();
        if change_index {
            git(root.path(), &["add", "file"]);
            std::fs::write(&path, "base\nreviewed\n").unwrap();
        }
        let expected = git(root.path(), &["show", ":file"]);
        let pending = PendingStage {
            repository: snapshot.repository,
            backend: snapshot.backend,
            directory: snapshot.directory,
            head: snapshot.head,
            files: vec![(
                snapshot.files[0].clone(),
                StageChange::Line {
                    removal: false,
                    line: 1,
                    staged: true,
                },
            )],
            saves: HashMap::new(),
        };
        assert!(
            pending
                .apply()
                .await
                .unwrap_err()
                .contains("changed since review")
        );
        assert_eq!(git(root.path(), &["show", ":file"]), expected);
    }
}

#[tokio::test]
async fn review_retains_divergent_index_and_limits_special_files_to_whole_file_actions() {
    let root = fixture();
    let path = root.path().join("file");
    std::fs::write(&path, "staged\n").unwrap();
    git(root.path(), &["add", "file"]);
    std::fs::write(&path, "worktree\n").unwrap();
    std::fs::write(root.path().join("binary"), b"\0binary").unwrap();
    let snapshot = load_review(
        local_workspace_backend(),
        root.path().into(),
        HashMap::new(),
        None,
    )
    .await
    .unwrap();
    let file = snapshot
        .files
        .iter()
        .find(|f| f.entry.relative_path == Path::new("file"))
        .unwrap();
    let content = file.content.as_ref().unwrap();
    assert!(content.selection.is_none());
    assert_eq!(content.index.as_deref(), Some(b"staged\n".as_slice()));
    assert!(
        snapshot
            .files
            .iter()
            .find(|f| f.entry.relative_path == Path::new("binary"))
            .unwrap()
            .content
            .is_err()
    );
    assert_eq!(git(root.path(), &["show", ":file"]), "staged\n");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        git(root.path(), &["reset", "--hard", "HEAD"]);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(&path, "base\nextra\n").unwrap();
        let snapshot = load_review(
            local_workspace_backend(),
            root.path().into(),
            HashMap::new(),
            None,
        )
        .await
        .unwrap();
        assert!(
            snapshot
                .files
                .iter()
                .find(|f| f.entry.relative_path == Path::new("file"))
                .unwrap()
                .content
                .as_ref()
                .err()
                .unwrap()
                .contains("mode change")
        );
    }
}

#[tokio::test]
async fn deletions_and_renames_keep_whole_file_staging_with_a_read_only_diff() {
    let root = fixture();
    std::fs::write(root.path().join("moved"), "kept\n").unwrap();
    git(root.path(), &["add", "moved"]);
    git(root.path(), &["commit", "-qm", "moved"]);
    std::fs::remove_file(root.path().join("file")).unwrap();
    git(root.path(), &["mv", "moved", "renamed"]);
    std::fs::write(root.path().join("renamed"), "kept\nadded\n").unwrap();
    let snapshot = load_review(
        local_workspace_backend(),
        root.path().into(),
        HashMap::new(),
        None,
    )
    .await
    .unwrap();
    let file = |path: &str| {
        snapshot
            .files
            .iter()
            .find(|f| f.entry.relative_path == Path::new(path))
            .unwrap()
    };
    let deleted = file("file");
    assert!(deleted.content.is_err());
    let preview = deleted.preview.as_ref().unwrap();
    assert_eq!(preview.old_len, 1);
    assert_eq!(
        preview.changes.hunks(),
        [nucleotide_vcs::staging::StagingHunk {
            before: 0..1,
            after: 0..0
        }]
    );
    // A rename compares the original path in HEAD with the new file on disk.
    let renamed = file("renamed");
    assert!(renamed.content.is_err());
    let preview = renamed.preview.as_ref().unwrap();
    assert_eq!(
        preview.changes.hunks(),
        [nucleotide_vcs::staging::StagingHunk {
            before: 1..1,
            after: 1..2
        }]
    );
}
