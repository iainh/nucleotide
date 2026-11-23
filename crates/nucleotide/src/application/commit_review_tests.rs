//! Exercises the real Commit tab controls, editor save queue and Git index.
use super::*;
use gpui::{Modifiers, MouseButton, point, px};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn repository() -> tempfile::TempDir {
    // Helix normalizes document paths (on Windows, it expands 8.3 short names
    // such as RUNNER~1), so the project directory must use the same spelling.
    let root = tempfile::tempdir_in(helix_stdx::path::normalize(std::env::temp_dir())).unwrap();
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["config", "user.name", "Test"]);
    git(root.path(), &["config", "commit.gpgsign", "false"]);
    // Windows runners enable autocrlf globally; these fixtures write LF files.
    git(root.path(), &["config", "core.autocrlf", "false"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    fs::write(root.path().join("file.txt"), "alpha\nbeta\n").unwrap();
    git(root.path(), &["add", "file.txt"]);
    git(root.path(), &["commit", "-qm", "initial"]);
    root
}

fn pump(core: &Entity<Application>, cx: &mut gpui::VisualTestContext) {
    core.update(cx, |core, cx| {
        core.drive_event_driven_maintenance(
            cx,
            TEST_RUNTIME.handle().clone(),
            &MaintenanceWake::channel().0,
        );
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
}

#[track_caller]
fn wait_for(
    core: &Entity<Application>,
    cx: &mut gpui::VisualTestContext,
    mut done: impl FnMut(&mut gpui::VisualTestContext) -> bool,
) {
    for _ in 0..200 {
        pump(core, cx);
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let status = core.read_with(cx, |core, _| {
        core.editor.get_status().map(|(s, _)| s.to_string())
    });
    panic!("Commit review did not reach the expected state: {status:?}");
}

fn click_line(cx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = cx.debug_bounds(selector).expect("visible change row");
    let position = point(bounds.left() + px(8.0), bounds.center().y);
    cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
}

fn install_commit_language(core: &Entity<Application>, cx: &mut gpui::VisualTestContext) {
    core.update(cx, |core, _| {
        let loader = syntax::Loader::new(
            helix_loader::config::default_lang_config()
                .try_into()
                .unwrap(),
        )
        .unwrap();
        core.editor.syn_loader.store(Arc::new(loader));
    });
}

fn submit_command(cx: &mut gpui::VisualTestContext, command: &str) {
    cx.dispatch_action(crate::actions::workspace::ShowCommandPrompt);
    cx.run_until_parked();
    cx.simulate_input(command);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
}

#[gpui::test]
fn commit_message_edits_are_isolated_and_file_commands_preserve_the_draft(
    cx: &mut gpui::TestAppContext,
) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    install_commit_language(&core, cx);
    let (file, slot) = core.read_with(cx, |core, _| {
        (
            core.editor.tree.get(core.editor.tree.focus).doc,
            core.editor.tree.focus,
        )
    });
    let file_pane =
        workspace.read_with(cx, |workspace, _| workspace.pane_id_for_slot(slot).unwrap());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    submit_command(cx, "vsplit");
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-message-editor").is_some()
    });
    cx.simulate_keystrokes("c i");
    cx.simulate_input("feat: keep the message separate\n\nPreserve the file in the other pane.");
    cx.simulate_keystrokes("escape");
    pump(&core, cx);
    let message = core.read_with(cx, |core, _| {
        let id = core.editor.tree.get(core.editor.tree.focus).doc;
        assert!(core.editor.is_hosted_auxiliary(id));
        assert_eq!(
            core.editor.documents[&file].text().to_string(),
            "alpha\nbeta\n"
        );
        let doc = &core.editor.documents[&id];
        assert!(doc.path().is_none());
        assert_eq!(doc.text_width(), 72);
        assert_eq!(doc.language_config().unwrap().rulers, Some(vec![51, 73]));
        assert_eq!(
            doc.text().to_string(),
            "feat: keep the message separate\n\nPreserve the file in the other pane.\n"
        );
        id
    });
    let draft = core.read_with(cx, |core, _| {
        core.editor.documents[&message].text().to_string()
    });
    for command in ["w", "wq", "x", "q", "bc!", "wa"] {
        submit_command(cx, command);
        core.read_with(cx, |core, _| {
            assert!(
                core.editor
                    .get_status()
                    .unwrap()
                    .0
                    .contains("Use the Commit button"),
                "{command}"
            );
            assert_eq!(core.editor.documents[&message].text().to_string(), draft);
        });
    }
    // Normal-mode Escape returns to the list; its keys must not edit the draft.
    cx.simulate_keystrokes("escape j k");
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.hosted_active_slot(), None)
    });
    cx.simulate_keystrokes("c u");
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents[&message].text().to_string(), "\n")
    });
    cx.simulate_keystrokes("U");
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents[&message].text().to_string(), draft)
    });

    // A file pane retains its own editing target. Bulk save ignores session drafts.
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.activate_test_pane_tab(file_pane, crate::tab::TabId::Document(file), cx);
        workspace.focus_editor(window, cx);
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("file-only\n");
    cx.simulate_keystrokes("escape");
    submit_command(cx, "wa");
    wait_for(&core, cx, |_| {
        fs::read_to_string(&path).is_ok_and(|text| text.starts_with("file-only\n"))
    });
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents[&message].text().to_string(), draft)
    });
    submit_command(cx, "bn");
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.get(core.editor.tree.focus).doc, file)
    });
    cx.dispatch_action(crate::actions::workspace::ShowBufferPicker);
    pump(&core, cx);
    assert!(cx.debug_bounds("picker-row-0").is_some());
    assert!(
        cx.debug_bounds("picker-row-1").is_none(),
        "message must not appear in the buffer picker"
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(
        git(root.path(), &["log", "-1", "--format=%s"]).trim(),
        "initial"
    );
}

#[gpui::test]
fn commit_message_drafts_survive_close_and_are_scoped_to_repository(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let first = repository();
    let second = repository();
    let (core, _, cx) = new_test_workspace(cx, &first.path().join("file.txt"));
    install_commit_language(&core, cx);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-message-editor").is_some()
    });
    cx.simulate_keystrokes("c i");
    cx.simulate_input("first repository");
    cx.simulate_keystrokes("escape");
    core.update(cx, |_, cx| {
        cx.emit(crate::Update::SemanticShortcut(
            crate::types::SemanticShortcutIntent::CloseFile,
        ))
    });
    pump(&core, cx);
    core.update(cx, |core, cx| {
        core.project_directory = Some(second.path().to_path_buf());
        cx.emit(crate::Update::ShowCommit);
    });
    wait_for(&core, cx, |cx| {
        core.read_with(cx, |core, _| core.editor.documents.len() == 3)
    });
    cx.simulate_keystrokes("c i");
    cx.simulate_input("second repository");
    cx.simulate_keystrokes("escape");
    core.update(cx, |core, cx| {
        core.project_directory = Some(first.path().to_path_buf());
        cx.emit(crate::Update::ShowCommit);
    });
    wait_for(&core, cx, |cx| {
        core.read_with(cx, |core, _| {
            core.editor
                .tree
                .try_get(core.editor.tree.focus)
                .is_some_and(|view| core.editor.documents[&view.doc].text() == "first repository\n")
        })
    });
    core.read_with(cx, |core, _| assert_eq!(core.editor.documents().count(), 1));
}

#[gpui::test]
fn commit_message_click_focus_and_reflow_keep_helix_wrapping(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let (core, _, cx) = new_test_workspace(cx, &root.path().join("file.txt"));
    install_commit_language(&core, cx);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-message-editor").is_some()
    });
    cx.update(|window, cx| {
        window.resize(gpui::size(px(780.0), px(600.0)));
        window.bounds_changed(cx);
    });
    pump(&core, cx);
    let bounds = cx.debug_bounds("commit-message-editor").unwrap();
    assert!(bounds.size.height >= px(100.0));
    assert!(bounds.right() <= px(780.0));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.simulate_keystrokes("i");
    let body = "This paragraph deliberately extends beyond seventy-two columns so that reflow must split it without changing its words.";
    cx.simulate_input(&format!("fix: retain Helix wrapping\n\n{body}"));
    cx.simulate_keystrokes("escape");
    let message = core.read_with(cx, |core, _| {
        let id = core.editor.tree.get(core.editor.tree.focus).doc;
        assert!(core.editor.is_hosted_auxiliary(id));
        assert_eq!(
            core.editor.documents[&id].text().line(2).to_string(),
            format!("{body}\n")
        );
        id
    });
    cx.simulate_keystrokes("x");
    submit_command(cx, "reflow");
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents[&message].text().to_string(), "fix: retain Helix wrapping\n\nThis paragraph deliberately extends beyond seventy-two columns so that\nreflow must split it without changing its words.\n");
    });
    // Mouse focus must reactivate the same slot after Escape disarms it.
    cx.simulate_keystrokes("escape");
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.simulate_keystrokes("g g i");
    cx.simulate_input("test: ");
    core.read_with(cx, |core, _| {
        assert!(
            core.editor.documents[&message]
                .text()
                .to_string()
                .starts_with("test: fix:")
        )
    });
}

#[gpui::test]
fn commit_review_saves_buffer_before_staging_only_the_clicked_line(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("first\nsecond\n");
    cx.simulate_keystrokes("escape");
    assert_eq!(fs::read_to_string(&path).unwrap(), "alpha\nbeta\n");
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-row-0-false-1").is_some()
    });
    click_line(cx, "commit-row-0-false-1");
    wait_for(&core, cx, |_| {
        git(root.path(), &["show", ":file.txt"]) == "second\nalpha\nbeta\n"
    });
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "first\nsecond\nalpha\nbeta\n"
    );
    core.read_with(cx, |core, _| {
        assert!(!core.editor.documents.values().next().unwrap().is_modified())
    });

    // The refreshed checkbox reflects the actual index, so a second click unstages.
    click_line(cx, "commit-row-0-false-1");
    wait_for(&core, cx, |_| {
        git(root.path(), &["show", ":file.txt"]) == "alpha\nbeta\n"
    });
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "first\nsecond\nalpha\nbeta\n"
    );

    // The unified layout stages the same line as the default side-by-side one.
    click_line(cx, "commit-layout-unified");
    pump(&core, cx);
    click_line(cx, "commit-row-0-false-0");
    wait_for(&core, cx, |_| {
        git(root.path(), &["show", ":file.txt"]) == "first\nalpha\nbeta\n"
    });
}

#[gpui::test]
fn commit_review_stages_lines_clicked_while_an_earlier_stage_runs(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    fs::write(&path, "alpha\nbeta\nlast\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("first\n");
    cx.simulate_keystrokes("escape");
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-row-0-false-3").is_some()
    });

    // The first stage waits for the buffer to save; the second click lands meanwhile.
    click_line(cx, "commit-row-0-false-0");
    cx.update(|window, cx| window.draw(cx).clear());
    click_line(cx, "commit-row-0-false-3");
    wait_for(&core, cx, |_| {
        git(root.path(), &["show", ":file.txt"]) == "first\nalpha\nbeta\nlast\n"
    });
}

#[gpui::test]
fn commit_review_stages_a_file_clicked_while_a_line_stage_runs(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    fs::write(&path, "alpha\nbeta\nlast\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("first\n");
    cx.simulate_keystrokes("escape");
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-row-0-false-3").is_some()
    });

    // The line stage waits for the buffer to save; the file checkbox lands meanwhile.
    click_line(cx, "commit-row-0-false-0");
    cx.update(|window, cx| window.draw(cx).clear());
    click_line(cx, "changes-stage-0");
    wait_for(&core, cx, |_| {
        git(root.path(), &["show", ":file.txt"]) == "first\nalpha\nbeta\nlast\n"
    });
}

#[gpui::test]
fn long_diffs_scroll_to_their_last_change_in_both_layouts(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    let lines = |changed: bool| -> String {
        (0..600)
            .map(|i| match changed && i % 12 == 0 {
                true => format!("changed {i}\n"),
                false => format!("line {i}\n"),
            })
            .collect()
    };
    fs::write(&path, lines(false)).unwrap();
    git(root.path(), &["commit", "-qam", "long"]);
    fs::write(&path, lines(true)).unwrap();
    let (core, _, cx) = new_test_workspace(cx, &path);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-row-0-false-0").is_some()
    });
    // Each change is laid out only once it scrolls into view.
    for layout in ["commit-layout-split", "commit-layout-unified"] {
        click_line(cx, layout);
        pump(&core, cx);
        assert!(cx.debug_bounds("commit-row-0-false-588").is_none());
        let header = cx.debug_bounds("commit-shown-0").unwrap();
        let position = point(header.center().x, header.bottom() + px(40.0));
        for direction in [-1.0, 1.0] {
            for _ in 0..60 {
                cx.simulate_event(gpui::ScrollWheelEvent {
                    position,
                    delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(400.0 * direction))),
                    ..Default::default()
                });
                cx.update(|window, cx| window.draw(cx).clear());
            }
            let row = if direction < 0.0 {
                "commit-row-0-false-588"
            } else {
                "commit-row-0-false-0"
            };
            assert!(cx.debug_bounds(row).is_some(), "{layout}: {row}");
        }
    }
}

#[gpui::test]
fn commit_review_shows_one_file_and_brackets_move_between_files(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    for index in 0..20 {
        let text = (0..40)
            .map(|line| format!("file {index} line {line}\n"))
            .collect::<String>();
        fs::write(root.path().join(format!("change-{index:02}.txt")), text).unwrap();
    }
    let (core, _, cx) = new_test_workspace(cx, &root.path().join("file.txt"));
    install_commit_language(&core, cx);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-row-0-false-0").is_some()
    });
    assert!(
        cx.debug_bounds("commit-row-19-false-0").is_none(),
        "only the selected file's diff is laid out"
    );
    for _ in 0..19 {
        cx.simulate_keystrokes("]");
    }
    pump(&core, cx);
    assert!(cx.debug_bounds("commit-row-19-false-1").is_some());
    assert!(cx.debug_bounds("commit-row-0-false-0").is_none());
    // File -> hunk -> first line -> second line of the newly selected file.
    cx.simulate_keystrokes("j j j space");
    wait_for(&core, cx, |_| {
        git(root.path(), &["diff", "--cached", "--name-only"]).trim() == "change-19.txt"
    });
    assert_eq!(
        git(root.path(), &["show", ":change-19.txt"]),
        "file 19 line 1\n"
    );

    // Mouse scrolling must not orphan the focused file's key dispatch path.
    cx.simulate_keystrokes("k k k");
    let position = cx.debug_bounds("commit-row-19-false-1").unwrap().center();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(50_000.0))),
        ..Default::default()
    });
    pump(&core, cx);
    cx.simulate_keystrokes("[ space");
    wait_for(&core, cx, |_| {
        git(root.path(), &["diff", "--cached", "--name-only"]).trim()
            == "change-18.txt\nchange-19.txt"
    });
    assert_eq!(
        git(root.path(), &["show", ":change-18.txt"]),
        fs::read_to_string(root.path().join("change-18.txt")).unwrap()
    );

    // Folding changes item heights without dropping navigation or the draft.
    cx.simulate_keystrokes("] z a");
    pump(&core, cx);
    assert!(cx.debug_bounds("commit-row-19-false-1").is_none());
    cx.simulate_keystrokes("z a c i");
    cx.simulate_input("fix: responsive review\n\nKeep the exact message.");
    cx.simulate_keystrokes("escape");
    core.read_with(cx, |core, _| {
        let doc = core
            .editor
            .document(core.editor.tree.get(core.editor.tree.focus).doc)
            .unwrap();
        assert_eq!(
            doc.text().to_string(),
            "fix: responsive review\n\nKeep the exact message.\n"
        );
    });
}

#[gpui::test]
fn changes_sidebar_selects_the_shown_file_and_stages_whole_files(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    fs::write(root.path().join("gone.txt"), "old\n").unwrap();
    git(root.path(), &["add", "gone.txt"]);
    git(root.path(), &["commit", "-qm", "gone"]);
    fs::remove_file(root.path().join("gone.txt")).unwrap();
    fs::write(root.path().join("added.txt"), "new\n").unwrap();
    fs::write(root.path().join("file.txt"), "alpha\nchanged\n").unwrap();
    let (core, _, cx) = new_test_workspace(cx, &root.path().join("file.txt"));
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    // Files are listed by path: added.txt, file.txt, gone.txt.
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("changes-row-2").is_some() && cx.debug_bounds("commit-shown-0").is_some()
    });
    assert!(cx.debug_bounds("commit-shown-1").is_none());

    click_line(cx, "changes-row-1");
    wait_for(&core, cx, |cx| cx.debug_bounds("commit-shown-1").is_some());
    assert!(cx.debug_bounds("commit-shown-0").is_none());
    assert!(cx.debug_bounds("commit-row-1-false-1").is_some());

    // The sidebar checkbox stages its own file, not the one being shown.
    click_line(cx, "changes-stage-0");
    wait_for(&core, cx, |_| {
        git(root.path(), &["diff", "--cached", "--name-only"]).trim() == "added.txt"
    });
    click_line(cx, "changes-stage-2");
    wait_for(&core, cx, |_| {
        git(root.path(), &["diff", "--cached", "--name-only"]).trim() == "added.txt\ngone.txt"
    });
    assert!(cx.debug_bounds("commit-shown-1").is_some());
}

#[gpui::test]
fn commit_review_save_failure_leaves_index_untouched(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("unsaved\n");
    cx.simulate_keystrokes("escape");
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-row-0-false-0").is_some()
    });
    // External replacement makes the ordinary, non-forced editor save fail.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    click_line(cx, "commit-row-0-false-0");
    wait_for(&core, cx, |cx| {
        core.read_with(cx, |core, _| {
            core.editor
                .get_status()
                .is_some_and(|(s, _)| s.contains("Save failed"))
        })
    });
    assert_eq!(git(root.path(), &["show", ":file.txt"]), "alpha\nbeta\n");
    core.read_with(cx, |core, _| {
        assert!(core.editor.documents.values().next().unwrap().is_modified())
    });
}

#[gpui::test]
fn commit_review_reopens_one_tab_and_split_keeps_one_review(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let (core, workspace, cx) = new_test_workspace(cx, &root.path().join("file.txt"));
    let slot = core.read_with(cx, |core, _| core.editor.tree.focus);
    let pane = workspace.read_with(cx, |workspace, _| workspace.pane_id_for_slot(slot).unwrap());
    for _ in 0..2 {
        core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
        pump(&core, cx);
    }
    let document_count = core.read_with(cx, |core, _| core.editor.documents.len());
    cx.dispatch_action(crate::actions::workspace::ShowCommandPrompt);
    cx.simulate_input("vsplit");
    cx.simulate_keystrokes("enter");
    pump(&core, cx);
    assert_eq!(
        core.read_with(cx, |core, _| core.editor.documents.len()),
        document_count + 1
    );
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    pump(&core, cx);
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(
            workspace.active_test_pane_tab(pane),
            Some(crate::tab::TabId::Commit)
        )
    });
}

#[gpui::test]
fn commit_button_commits_index_not_unsaved_buffer_and_amend_rewords(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    fs::write(&path, "staged\nbeta\n").unwrap();
    git(root.path(), &["add", "file.txt"]);
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    install_commit_language(&core, cx);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("unsaved\n");
    cx.simulate_keystrokes("escape");
    let file = core.read_with(cx, |core, _| {
        core.editor.tree.get(core.editor.tree.focus).doc
    });
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-message-editor").is_some()
    });
    cx.simulate_keystrokes("c i");
    cx.simulate_input("# comments alone are not a message");
    cx.simulate_keystrokes("escape");
    click_line(cx, "commit-submit");
    pump(&core, cx);
    assert_eq!(
        git(root.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "1"
    );
    let message_bounds = cx.debug_bounds("commit-message-editor").unwrap();
    cx.simulate_click(message_bounds.center(), Modifiers::none());
    cx.simulate_keystrokes("% c");
    cx.simulate_input("feat: commit the index\n\nKeep the buffer unsaved.\n# omit this comment");
    cx.simulate_keystrokes("escape");
    let message = core.read_with(cx, |core, _| {
        core.editor.tree.get(core.editor.tree.focus).doc
    });
    click_line(cx, "commit-submit");
    wait_for(&core, cx, |cx| {
        core.read_with(cx, |core, _| core.editor.documents[&message].text() == "\n")
    });
    assert_eq!(
        git(root.path(), &["show", "HEAD:file.txt"]),
        "staged\nbeta\n"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "staged\nbeta\n");
    assert_eq!(
        git(root.path(), &["log", "-1", "--format=%B"]).trim(),
        "feat: commit the index\n\nKeep the buffer unsaved."
    );
    core.read_with(cx, |core, _| {
        assert!(core.editor.documents[&file].is_modified());
        assert_eq!(
            core.editor.documents[&file].text().to_string(),
            "unsaved\nstaged\nbeta\n"
        );
    });
    // Wait for the refreshed HEAD before invoking Amend.
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-push-only").is_some()
    });
    cx.simulate_keystrokes("alt-a");
    core.read_with(cx, |core, _| {
        assert!(
            core.editor.documents[&message]
                .text()
                .to_string()
                .starts_with("feat: commit the index")
        )
    });
    let message_bounds = cx.debug_bounds("commit-message-editor").unwrap();
    cx.simulate_click(message_bounds.center(), Modifiers::none());
    cx.simulate_keystrokes("% c");
    cx.simulate_input("fix: reword the commit");
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-enter"
    } else {
        "ctrl-enter"
    });
    wait_for(&core, cx, |_| {
        git(root.path(), &["log", "-1", "--format=%s"]).trim() == "fix: reword the commit"
    });
    assert_eq!(
        git(root.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "2"
    );
}

#[cfg(unix)]
fn hook(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[gpui::test]
fn commit_and_push_keeps_commit_on_rejection_and_push_retry_does_not_commit_again(
    cx: &mut gpui::TestAppContext,
) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let remote = tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-q"]);
    git(
        root.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    hook(
        &remote.path().join("hooks/pre-receive"),
        "#!/bin/sh\necho preview-rejected >&2\nexit 1\n",
    );
    fs::write(root.path().join("file.txt"), "push this\n").unwrap();
    git(root.path(), &["add", "file.txt"]);
    let (core, _, cx) = new_test_workspace(cx, &root.path().join("file.txt"));
    install_commit_language(&core, cx);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-message-editor").is_some()
    });
    cx.simulate_keystrokes("c i");
    cx.simulate_input("feat: publish changes");
    cx.simulate_keystrokes("escape");
    let message = core.read_with(cx, |core, _| {
        core.editor.tree.get(core.editor.tree.focus).doc
    });
    // A failing commit hook must not attempt the push or discard the draft.
    let pre_commit = root.path().join(".git/hooks/pre-commit");
    hook(&pre_commit, "#!/bin/sh\necho commit-rejected >&2\nexit 1\n");
    click_line(cx, "commit-push");
    wait_for(&core, cx, |cx| {
        core.read_with(cx, |core, _| {
            core.editor
                .get_status()
                .is_some_and(|(s, _)| s.contains("commit-rejected"))
        })
    });
    assert_eq!(
        git(root.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "1"
    );
    assert_eq!(git(root.path(), &["show", ":file.txt"]), "push this\n");
    core.read_with(cx, |core, _| {
        assert_eq!(
            core.editor.documents[&message].text().to_string(),
            "feat: publish changes\n"
        )
    });
    fs::remove_file(pre_commit).unwrap();
    // Refresh completion is observable through the return of the Push control.
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-push-only").is_some()
    });
    click_line(cx, "commit-push");
    wait_for(&core, cx, |cx| {
        core.read_with(cx, |core, _| {
            core.editor
                .get_status()
                .is_some_and(|(s, _)| s.contains("Commit created, but push failed"))
        })
    });
    let head = git(root.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        git(root.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "2"
    );
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents[&message].text().to_string(), "\n")
    });
    fs::remove_file(remote.path().join("hooks/pre-receive")).unwrap();
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-push-only").is_some()
    });
    click_line(cx, "commit-push-only");
    wait_for(&core, cx, |_| {
        !git(remote.path(), &["for-each-ref", "--format=%(objectname)"])
            .trim()
            .is_empty()
    });
    assert_eq!(
        git(remote.path(), &["for-each-ref", "--format=%(objectname)"]),
        head
    );
    assert_eq!(git(root.path(), &["rev-parse", "HEAD"]), head);
    assert!(git(root.path(), &["rev-parse", "--abbrev-ref", "@{u}"]).starts_with("origin/"));
    wait_for(&core, cx, |cx| {
        core.read_with(cx, |core, _| {
            core.editor
                .get_status()
                .is_some_and(|(status, _)| status == "Push completed")
        })
    });
}

#[cfg(unix)]
#[gpui::test]
fn commit_template_and_amend_preserve_the_original_draft(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    fs::write(
        root.path().join(".git/template"),
        "feat: from template\n\n# describe why\n",
    )
    .unwrap();
    git(root.path(), &["config", "commit.template", ".git/template"]);
    fs::write(root.path().join("file.txt"), "ready\n").unwrap();
    git(root.path(), &["add", "file.txt"]);
    let (core, _, cx) = new_test_workspace(cx, &root.path().join("file.txt"));
    install_commit_language(&core, cx);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-message-editor").is_some()
    });
    cx.simulate_keystrokes("c");
    let message = core.read_with(cx, |core, _| {
        core.editor.tree.get(core.editor.tree.focus).doc
    });
    core.read_with(cx, |core, _| {
        assert_eq!(
            core.editor.documents[&message].text().to_string(),
            "feat: from template\n\n# describe why\n"
        )
    });
    cx.simulate_keystrokes("alt-a");
    core.read_with(cx, |core, _| {
        assert_eq!(
            core.editor.documents[&message].text().to_string(),
            "initial\n"
        )
    });
    cx.simulate_keystrokes("alt-a");
    core.read_with(cx, |core, _| {
        assert!(
            core.editor.documents[&message]
                .text()
                .to_string()
                .starts_with("feat: from template")
        )
    });
    click_line(cx, "commit-submit");
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-push-only").is_some()
    });
    assert_eq!(
        git(root.path(), &["log", "-1", "--format=%B"]).trim(),
        "feat: from template"
    );
    assert_eq!(
        git(root.path(), &["rev-list", "--count", "HEAD"]).trim(),
        "2"
    );
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents[&message].text().to_string(), "\n")
    });
    cx.simulate_keystrokes("alt-a");
    git(
        root.path(),
        &["commit", "--allow-empty", "-qm", "external commit"],
    );
    let external = git(root.path(), &["rev-parse", "HEAD"]);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| cx.debug_bounds("commit-error").is_some());
    click_line(cx, "commit-submit");
    pump(&core, cx);
    assert_eq!(
        git(root.path(), &["rev-parse", "HEAD"]),
        external,
        "Amend must never silently switch to an external commit"
    );
}

#[gpui::test]
fn side_by_side_divider_drags_and_double_click_recentres(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    fs::write(root.path().join("file.txt"), "alpha\nchanged\nbeta\n").unwrap();
    let (core, _, cx) = new_test_workspace(cx, &root.path().join("file.txt"));
    install_commit_language(&core, cx);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-diff-0-split").is_some()
    });
    let widths = |cx: &mut gpui::VisualTestContext| {
        [
            cx.debug_bounds("commit-diff-0-old").unwrap().size.width,
            cx.debug_bounds("commit-diff-0-new").unwrap().size.width,
        ]
    };
    let [old, new] = widths(cx);
    assert!((old - new).abs() < px(1.0), "columns start equal");

    // Grab above the hunk checkbox, which sits in the middle of the ribbon.
    let grab = |cx: &mut gpui::VisualTestContext| {
        let bounds = cx.debug_bounds("commit-diff-0-split").unwrap();
        point(bounds.center().x, bounds.top() + px(4.0))
    };
    let start = grab(cx);
    let end = point(start.x + px(80.0), start.y);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(start.x + px(10.0), start.y),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    pump(&core, cx);
    let [dragged_old, dragged_new] = widths(cx);
    // The divider follows the pointer, so HEAD gains what the working tree loses.
    assert!(
        (dragged_old - old - px(80.0)).abs() < px(1.0),
        "{dragged_old:?}"
    );
    assert!(
        (new - dragged_new - px(80.0)).abs() < px(1.0),
        "{dragged_new:?}"
    );
    let handle = grab(cx);
    assert!((handle.x - end.x).abs() < px(1.0));

    cx.simulate_click(handle, Modifiers::none());
    cx.simulate_event(gpui::MouseDownEvent {
        position: handle,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        position: handle,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
    });
    pump(&core, cx);
    let [old, new] = widths(cx);
    assert!((old - new).abs() < px(1.0), "double click recentres");
}

#[gpui::test]
fn message_divider_resizes_editor_against_diff_and_double_click_restores(
    cx: &mut gpui::TestAppContext,
) {
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    fs::write(root.path().join("file.txt"), "alpha\nchanged\nbeta\n").unwrap();
    let (core, _, cx) = new_test_workspace(cx, &root.path().join("file.txt"));
    install_commit_language(&core, cx);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-message-editor").is_some()
            && cx.debug_bounds("commit-diff-0-split").is_some()
    });
    let heights = |cx: &mut gpui::VisualTestContext| {
        [
            cx.debug_bounds("commit-message-editor")
                .unwrap()
                .size
                .height,
            cx.debug_bounds("commit-diff-0-split").unwrap().top(),
        ]
    };
    let [editor, diff_top] = heights(cx);
    let start = cx.debug_bounds("commit-message-divider").unwrap().center();
    let end = point(start.x, start.y + px(60.0));
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        point(start.x, start.y + px(10.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    pump(&core, cx);
    // The editor grows by the pointer's travel and pushes the diff down with it.
    let [dragged_editor, dragged_top] = heights(cx);
    assert!(
        (dragged_editor - editor - px(60.0)).abs() < px(1.0),
        "{editor:?} -> {dragged_editor:?}"
    );
    assert!(
        (dragged_top - diff_top - px(60.0)).abs() < px(1.0),
        "{diff_top:?} -> {dragged_top:?}"
    );
    let handle = cx.debug_bounds("commit-message-divider").unwrap().center();
    assert!((handle.y - end.y).abs() < px(1.0));

    cx.simulate_click(handle, Modifiers::none());
    cx.simulate_event(gpui::MouseDownEvent {
        position: handle,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        position: handle,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 2,
    });
    pump(&core, cx);
    assert!(
        (heights(cx)[0] - editor).abs() < px(1.0),
        "double click restores"
    );
}

fn right_click(cx: &mut gpui::VisualTestContext, selector: &'static str) {
    let bounds = cx.debug_bounds(selector).expect("visible change row");
    let position = bounds.center();
    cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn review_menu_discards_only_after_confirmation_and_reloads_the_buffer(
    cx: &mut gpui::TestAppContext,
) {
    use crate::workspace::ReviewMenuIntent;
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    fs::write(&path, "alpha\nchanged\n").unwrap();
    git(root.path(), &["add", "file.txt"]);
    fs::write(&path, "alpha\nchanged again\n").unwrap();
    fs::write(root.path().join("other.txt"), "keep\n").unwrap();
    let (core, _, cx) = new_test_workspace(cx, &path);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| cx.debug_bounds("changes-row-1").is_some());

    // Cancelling the confirmation leaves the file alone.
    right_click(cx, "changes-row-0");
    cx.dispatch_action(crate::actions::commit_menu::Operation {
        intent: ReviewMenuIntent::Discard,
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    pump(&core, cx);
    assert_eq!(fs::read_to_string(&path).unwrap(), "alpha\nchanged again\n");

    right_click(cx, "changes-row-0");
    cx.dispatch_action(crate::actions::commit_menu::Operation {
        intent: ReviewMenuIntent::Discard,
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_for(&core, cx, |cx| {
        fs::read_to_string(&path).unwrap() == "alpha\nbeta\n"
            && core.read_with(cx, |core, _| {
                core.editor.documents.values().next().unwrap().text() == "alpha\nbeta\n"
            })
    });
    assert_eq!(
        git(root.path(), &["status", "--porcelain"]),
        "?? other.txt\n"
    );

    // Deleting an untracked file also asks first.
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("changes-row-0").is_some() && cx.debug_bounds("changes-row-1").is_none()
    });
    right_click(cx, "changes-row-0");
    cx.dispatch_action(crate::actions::commit_menu::Operation {
        intent: ReviewMenuIntent::Discard,
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    wait_for(&core, cx, |_| !root.path().join("other.txt").exists());
}

#[gpui::test]
fn review_hunk_menu_stages_and_discards_one_hunk(cx: &mut gpui::TestAppContext) {
    use crate::workspace::ReviewMenuIntent;
    let _runtime = TEST_RUNTIME.enter();
    let root = repository();
    let path = root.path().join("file.txt");
    let base: String = (0..20).map(|n| format!("line {n}\n")).collect();
    fs::write(&path, &base).unwrap();
    git(root.path(), &["commit", "-qam", "lines"]);
    let edited = base
        .replace("line 1\n", "first\n")
        .replace("line 18\n", "second\n");
    fs::write(&path, &edited).unwrap();
    let (core, _, cx) = new_test_workspace(cx, &path);
    core.update(cx, |_, cx| cx.emit(crate::Update::ShowCommit));
    wait_for(&core, cx, |cx| {
        cx.debug_bounds("commit-row-0-false-18").is_some()
    });

    right_click(cx, "commit-row-0-false-18");
    cx.dispatch_action(crate::actions::commit_menu::Operation {
        intent: ReviewMenuIntent::StageHunk,
    });
    wait_for(&core, cx, |_| {
        git(root.path(), &["show", ":file.txt"]) == base.replace("line 18\n", "second\n")
    });

    right_click(cx, "commit-row-0-false-1");
    cx.dispatch_action(crate::actions::commit_menu::Operation {
        intent: ReviewMenuIntent::DiscardHunk,
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    let kept = base.replace("line 18\n", "second\n");
    wait_for(&core, cx, |_| fs::read_to_string(&path).unwrap() == kept);
    // The staged hunk stays staged.
    assert_eq!(git(root.path(), &["show", ":file.txt"]), kept);
}
