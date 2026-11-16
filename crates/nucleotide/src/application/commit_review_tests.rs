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
    let root = tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["config", "user.name", "Test"]);
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
    panic!("Commit review did not reach the expected state");
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
