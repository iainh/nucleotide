// Registered by integration as a child of application::tests to reuse its real
// Core/GPUI fixture without exposing test-only helpers to application code.
use super::{EditorInputBridge, TEST_RUNTIME, assert_same_path, new_test_workspace};
use helix_term::{config::ConfigRaw, keymap::Keymaps};
use helix_view::editor::Action;
use std::fs;
use tempfile::tempdir;

// Both real-transport fixtures share Helix's process-global event registry.
pub(super) fn init_lsp_test_hooks(handlers: &helix_view::handlers::Handlers) {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        helix_event::register_event::<helix_view::events::LanguageServerInitialized>();
        helix_event::register_event::<helix_view::events::DocumentDidChange>();
        helix_event::register_event::<helix_view::events::DocumentDidClose>();
        helix_view::handlers::lsp::register_hooks(handlers);
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
fn split_aliases_preserve_file_arguments_and_source_document(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let original = root.path().join("original.txt");
    let target = root.path().join("split target.txt");
    fs::write(&original, "original document\n").unwrap();
    fs::write(&target, "different split document\n").unwrap();

    for alias in ["sp", "hsplit", "vs", "vsplit"] {
        let (core, workspace, cx) = new_test_workspace(cx, &original);
        let (source_view, source_doc) = core.read_with(cx, |core, _| {
            let source_view = core.editor.tree.focus;
            (source_view, core.editor.tree.get(source_view).doc)
        });
        // Quoting a path with spaces distinguishes argument preservation from
        // merely invoking a split of the current buffer. Forward slashes also
        // avoid treating Windows path separators as command-line escapes.
        let argument = target.to_string_lossy().replace('\\', "/");
        submit_command(cx, &format!("{alias} '{argument}'"));
        core.read_with(cx, |core, _| {
            assert_eq!(core.editor.tree.views().count(), 2, "{alias}");
            assert_eq!(core.editor.tree.get(source_view).doc, source_doc);
            let focused = core.editor.tree.get(core.editor.tree.focus);
            assert_ne!(focused.id, source_view);
            assert_ne!(focused.doc, source_doc);
            let document = &core.editor.documents[&focused.doc];
            assert_same_path(document.path().unwrap(), &target);
            assert_eq!(document.text().to_string(), "different split document\n");
            assert_eq!(
                core.editor.documents[&source_doc].text().to_string(),
                "original document\n"
            );
        });
        cx.update(|window, _| window.remove_window());
        drop(workspace);
        drop(core);
        cx.run_until_parked();
    }
}

#[gpui::test]
fn mapped_split_focus_buffer_edit_targets_the_final_document(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let original = root.path().join("original.txt");
    let next = root.path().join("next.txt");
    fs::write(&original, "original\n").unwrap();
    fs::write(&next, "second\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &original);
    let (source_view, source_doc, next_doc) = core.update(cx, |core, _| {
        let source_view = core.editor.tree.focus;
        let source_doc = core.editor.tree.get(source_view).doc;
        let next_doc = core.editor.open(&next, Action::Load).unwrap();
        let config: ConfigRaw = toml::from_str(
            r#"
[keys.normal]
F6 = ["vsplit", "jump_view_left", "goto_next_buffer", "delete_selection"]
"#,
        )
        .unwrap();
        let mut keys = helix_term::config::Config::default().keys;
        helix_term::keymap::merge_keys(&mut keys, config.keys.unwrap());
        core.editor_input =
            EditorInputBridge::new(Keymaps::new(Box::new(arc_swap::access::Constant(keys))));
        (source_view, source_doc, next_doc)
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("f6");
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 2);
        assert_eq!(core.editor.tree.focus, source_view);
        assert_eq!(core.editor.tree.get(source_view).doc, next_doc);
        assert_eq!(
            core.editor.documents[&next_doc].text().to_string(),
            "econd\n"
        );
        assert_eq!(
            core.editor.documents[&source_doc].text().to_string(),
            "original\n"
        );
        let other = core
            .editor
            .tree
            .views()
            .find(|(view, _)| view.id != source_view)
            .unwrap()
            .0;
        assert_eq!(other.doc, source_doc);
    });
    assert_eq!(fs::read_to_string(next).unwrap(), "second\n");
}

#[gpui::test]
fn window_navigation_only_and_close_preserve_other_buffers(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("original.txt");
    fs::write(&path, "original\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    let original_view = core.read_with(cx, |core, _| core.editor.tree.focus);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("ctrl-w v");
    let split_view = core.read_with(cx, |core, _| core.editor.tree.focus);
    assert_ne!(split_view, original_view);
    // Use the native keymap for navigation and static-command fallback for
    // keep-only/close, so both public command entry paths are characterized.
    cx.simulate_keystrokes("ctrl-w h");
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.focus, original_view)
    });
    submit_command(cx, "wonly");
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 1);
        assert_eq!(core.editor.tree.focus, original_view);
    });
    submit_command(cx, "vsplit_new");
    let scratch = core.read_with(cx, |core, _| {
        let view = core.editor.tree.get(core.editor.tree.focus);
        assert_ne!(view.id, original_view);
        assert!(core.editor.documents[&view.doc].path().is_none());
        view.doc
    });
    submit_command(cx, "wclose");
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 1);
        assert_eq!(core.editor.tree.focus, original_view);
        assert!(core.editor.documents.contains_key(&scratch));
        let document = &core.editor.documents[&core.editor.tree.get(original_view).doc];
        assert_same_path(document.path().unwrap(), &path);
        assert_eq!(document.text().to_string(), "original\n");
    });
}

#[gpui::test]
fn buffer_close_alias_closes_shared_placement_then_cancels_last_dirty_close(
    cx: &mut gpui::TestAppContext,
) {
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("dirty.txt");
    fs::write(&path, "saved\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    let document = core.read_with(cx, |core, _| {
        core.editor.tree.get(core.editor.tree.focus).doc
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("unsaved ");
    cx.simulate_keystrokes("escape ctrl-w v");
    submit_command(cx, "bclose");
    core.read_with(cx, |core, _| {
        // No-argument GUI aliases retain placement-close compatibility.
        assert_eq!(core.editor.tree.views().count(), 1);
        assert!(
            core.editor
                .tree
                .views()
                .all(|(view, _)| view.doc == document)
        );
        assert!(core.editor.documents[&document].is_modified());
        assert_eq!(
            core.editor.documents[&document].text().to_string(),
            "unsaved saved\n"
        );
    });
    submit_command(cx, "bclose");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 1);
        assert_eq!(core.editor.tree.get(core.editor.tree.focus).doc, document);
        assert!(core.editor.documents[&document].is_modified());
    });
    cx.dispatch_action(crate::actions::workspace::ShowCommandPrompt);
    cx.run_until_parked();
    cx.simulate_input("bclose!");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 1);
        assert_eq!(
            core.editor.documents[&document].text().to_string(),
            "unsaved saved\n"
        );
    });
    assert_eq!(fs::read_to_string(path).unwrap(), "saved\n");
}

#[gpui::test]
fn targeted_buffer_close_refusal_preserves_all_copies_until_forced(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("dirty.txt");
    let other = root.path().join("other.txt");
    fs::write(&path, "saved\n").unwrap();
    fs::write(&other, "unrelated\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    let (document, unrelated) = core.update(cx, |core, _| {
        let document = core.editor.tree.get(core.editor.tree.focus).doc;
        let unrelated = core.editor.open(&other, Action::Load).unwrap();
        (document, unrelated)
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("unsaved ");
    cx.simulate_keystrokes("escape ctrl-w v");
    // Explicit buffer arguments bypass no-argument placement interception.
    let target = core.read_with(cx, |core, _| {
        core.editor.documents[&document]
            .path()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    });
    submit_command(cx, &format!("bclose '{target}'"));
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 2);
        assert!(
            core.editor
                .tree
                .views()
                .all(|(view, _)| view.doc == document)
        );
        assert!(core.editor.documents[&document].is_modified());
        assert_eq!(
            core.editor.documents[&document].text().to_string(),
            "unsaved saved\n"
        );
    });
    cx.dispatch_action(crate::actions::workspace::ShowCommandPrompt);
    cx.run_until_parked();
    cx.simulate_input(&format!("bclose! '{target}'"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 2);
        assert_eq!(
            core.editor.documents[&document].text().to_string(),
            "unsaved saved\n"
        );
    });
    submit_command(cx, &format!("bclose! '{target}'"));
    core.read_with(cx, |core, _| {
        assert!(!core.editor.documents.contains_key(&document));
        assert!(
            core.editor
                .tree
                .views()
                .all(|(view, _)| view.doc != document)
        );
        assert_eq!(
            core.editor.documents[&unrelated].text().to_string(),
            "unrelated\n"
        );
        assert!(!core.editor.should_close());
    });
    assert_eq!(fs::read_to_string(path).unwrap(), "saved\n");
}

// Migration-only regression: the preceding scenarios also run against the
// pre-hosted baseline. This one requires the delivered hosted editor bridge.
#[gpui::test]
fn hosted_zero_slot_workspace_keeps_maintenance_and_redraw_alive(cx: &mut gpui::TestAppContext) {
    use super::{MaintenanceWake, new_test_application};
    use std::{cell::Cell, rc::Rc};

    let _runtime = TEST_RUNTIME.enter();
    let app = new_test_application(cx);
    let quit = Rc::new(Cell::new(false));
    let redraw = Rc::new(Cell::new(false));
    let observed_quit = quit.clone();
    let observed_redraw = redraw.clone();
    cx.update(|cx| {
        cx.subscribe(&app, move |_, event: &crate::Update, _| match event {
            crate::Update::ShouldQuit => observed_quit.set(true),
            crate::Update::Redraw => observed_redraw.set(true),
            _ => {}
        })
        .detach();
    });
    let (wake, _wake_rx) = MaintenanceWake::channel();
    app.update(cx, |app, cx| {
        let document = app.editor.new_file(Action::VerticalSplit);
        app.editor.enable_hosted();
        assert!(app.editor.close_document(document, true).is_ok());
        assert_eq!(app.editor.tree.views().count(), 0);
        assert!(app.drive_event_driven_maintenance(cx, TEST_RUNTIME.handle().clone(), &wake));
        assert!(app.handle_editor_event_sync(helix_view::editor::EditorEvent::Redraw, cx));
    });
    cx.run_until_parked();
    assert!(
        redraw.get(),
        "hosted zero-slot redraw must reach the workspace"
    );
    assert!(!quit.get(), "editor emptiness is not workspace quit");
}

#[gpui::test]
fn viewport_mapping_finishes_before_another_pane_can_receive_input(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton};

    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("A.txt");
    let other_path = root.path().join("C.txt");
    // No trailing empty line: the bottom visual row is unambiguously "third".
    fs::write(&path, "first\nsecond\nthird").unwrap();
    fs::write(&other_path, "untouched C").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    let (a, a_slot, c, c_slot) = core.update(cx, |core, cx| {
        // Isolate the continuation boundary from short-document scrolloff
        // clamping: with no margin, bottom means the third visual row.
        let mut config = (**core.helix_config_arc.load()).clone();
        config.editor.scrolloff = 0;
        core.helix_config_arc.store(std::sync::Arc::new(config));
        let a_slot = core.editor.tree.focus;
        let a = core.editor.tree.get(a_slot).doc;
        let c = core
            .editor
            .open(&other_path, Action::VerticalSplit)
            .unwrap();
        let c_slot = core.editor.tree.focus;
        let config: ConfigRaw = toml::from_str(
            r#"
[keys.normal]
F12 = ["goto_window_bottom", "delete_selection"]
"#,
        )
        .unwrap();
        let mut keys = helix_term::config::Config::default().keys;
        helix_term::keymap::merge_keys(&mut keys, config.keys.unwrap());
        core.editor_input =
            EditorInputBridge::new(Keymaps::new(Box::new(arc_swap::access::Constant(keys))));
        core.editor_input.enable_hosted_commands();
        cx.emit(crate::Update::Redraw);
        (a, a_slot, c, c_slot)
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("ctrl-w h");
    cx.run_until_parked();
    core.read_with(cx, |core, _| assert_eq!(core.editor.tree.focus, a_slot));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.simulate_keystrokes("f12");
    cx.run_until_parked();
    // This assertion precedes every later physical key, so a queued tail
    // released accidentally by the click or C-h cannot make the test pass.
    core.read_with(cx, |core, _| {
        assert_eq!(
            core.editor.documents[&a].text().to_string(),
            "first\nsecond\nhird"
        );
        assert_eq!(core.editor.documents[&c].text().to_string(), "untouched C");
    });
    let c_pane = workspace.read_with(cx, |workspace, _| {
        workspace.pane_id_for_slot(c_slot).unwrap()
    });
    let selector = Box::leak(format!("pane-drop-body-{c_pane:?}").into_boxed_str());
    cx.update(|window, cx| window.draw(cx).clear());
    let position = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    cx.simulate_keystrokes("ctrl-h");
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.focus, c_slot);
        assert_eq!(core.editor.documents[&c].text().to_string(), "untouched C");
        assert_eq!(
            core.editor.documents[&a].text().to_string(),
            "first\nsecond\nhird"
        );
    });
}

#[gpui::test]
fn targeted_close_to_terminal_cancels_mapped_edit_instead_of_editing_helix_fallback(
    cx: &mut gpui::TestAppContext,
) {
    use crate::tab::TabId;
    use nucleotide_events::terminal::TerminalId;

    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let b_path = root.path().join("B.txt");
    let a_path = root.path().join("A.txt");
    fs::write(&b_path, "B must remain intact\n").unwrap();
    fs::write(&a_path, "A to close\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &b_path);
    let (slot, b, a) = core.update(cx, |core, cx| {
        let slot = core.editor.tree.focus;
        let b = core.editor.tree.get(slot).doc;
        let a = core.editor.open(&a_path, Action::Load).unwrap();
        let target = core.editor.documents[&a]
            .path()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let commands = vec![
            format!(":bclose! '{target}'"),
            "delete_selection".to_string(),
        ];
        let config: ConfigRaw = toml::from_str(&format!(
            "[keys.normal]\nF12 = {}\n",
            toml::Value::try_from(commands).unwrap()
        ))
        .unwrap();
        let mut keys = helix_term::config::Config::default().keys;
        helix_term::keymap::merge_keys(&mut keys, config.keys.unwrap());
        core.editor_input =
            EditorInputBridge::new(Keymaps::new(Box::new(arc_swap::access::Constant(keys))));
        core.editor_input.enable_hosted_commands();
        cx.emit(crate::Update::Redraw);
        (slot, b, a)
    });
    cx.run_until_parked();
    let pane = workspace.read_with(cx, |workspace, _| workspace.pane_id_for_slot(slot).unwrap());
    let terminal = TabId::Terminal(TerminalId(702));
    // Visit B -> T -> A. Closing A must restore the workspace history T,
    // not execute delete_selection against Helix's same-slot B fallback.
    workspace.update(cx, |workspace, cx| {
        workspace.activate_test_pane_tab(pane, terminal, cx);
        workspace.activate_test_pane_tab(pane, TabId::Document(a), cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.get(core.editor.tree.focus).doc, a)
    });
    core.update(cx, |core, cx| {
        core.handle_input_event(
            crate::InputEvent::key("F12".parse().unwrap()),
            cx,
            TEST_RUNTIME.handle().clone(),
        );
    });
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert!(
            !core.editor.documents.contains_key(&a),
            "targeted close did not finish: status {:?}, documents {:?}",
            core.editor.get_status(),
            core.editor
                .documents
                .iter()
                .map(|(id, doc)| (*id, doc.text().to_string()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            core.editor.documents[&b].text().to_string(),
            "B must remain intact\n"
        );
        assert!(!core.editor.documents[&b].is_modified());
        assert_eq!(core.editor.hosted_active_slot(), None);
        assert!(!core.editor.should_close());
    });
    assert_eq!(
        workspace.read_with(cx, |workspace, _| workspace.active_test_pane_tab(pane)),
        Some(terminal)
    );
}

#[gpui::test]
fn final_main_quit_after_last_dock_terminal_close_validates_dirty_documents(
    cx: &mut gpui::TestAppContext,
) {
    use std::{cell::Cell, rc::Rc};

    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("last.txt");
    fs::write(&path, "saved\n").unwrap();
    for dirty in [false, true] {
        let (core, workspace, cx) = new_test_workspace(cx, &path);
        let quit = Rc::new(Cell::new(false));
        let observed = quit.clone();
        cx.update(|_, cx| {
            cx.subscribe(&workspace, move |_, event: &crate::Update, _| {
                if matches!(event, crate::Update::ShouldQuit) {
                    observed.set(true);
                }
            })
            .detach();
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.focus_editor(window, cx)
        });
        if dirty {
            cx.simulate_keystrokes("i");
            cx.simulate_input("unsaved ");
            cx.simulate_keystrokes("escape");
        }
        cx.dispatch_action(crate::actions::workspace::ToggleTerminal);
        cx.run_until_parked();
        cx.dispatch_action(crate::actions::editor::CloseFile);
        cx.run_until_parked();
        submit_command(cx, "q");
        if dirty {
            assert!(
                !quit.get(),
                "dirty final main pane must refuse unforced quit"
            );
            core.read_with(cx, |core, _| {
                assert_eq!(core.editor.documents.len(), 1);
                let doc = core.editor.documents.values().next().unwrap();
                assert_eq!(doc.text().to_string(), "unsaved saved\n");
                assert!(doc.is_modified());
                assert!(!core.editor.should_close());
            });
            submit_command(cx, "q!");
        }
        assert!(
            quit.get(),
            "an empty hidden dock must not turn final quit into LastPane"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved\n");
        cx.update(|window, _| window.remove_window());
        drop(workspace);
        drop(core);
        cx.run_until_parked();
    }
}

#[gpui::test]
fn delayed_definition_cannot_release_an_old_edit_into_another_pane(cx: &mut gpui::TestAppContext) {
    use super::new_test_workspace_with_runtime;
    use futures_util::StreamExt;
    use gpui::{Modifiers, MouseButton};
    use helix_lsp::{ExternalTransport, ExternalTransportProvider};
    use std::{
        collections::HashMap,
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};

    struct Transport(Mutex<Option<ExternalTransport>>);
    impl ExternalTransportProvider for Transport {
        fn provide(
            &self,
            _: &str,
            _: &[String],
            _: &Path,
            _: &HashMap<String, String>,
        ) -> helix_lsp::Result<Option<ExternalTransport>> {
            Ok(self.0.lock().unwrap().take())
        }
    }
    async fn read(reader: &mut BufReader<DuplexStream>) -> serde_json::Value {
        let mut length = None;
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length: ") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; length.unwrap()];
        reader.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }
    async fn write(writer: &mut DuplexStream, message: serde_json::Value) {
        let body = serde_json::to_vec(&message).unwrap();
        writer
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await
            .unwrap();
        writer.write_all(&body).await.unwrap();
    }

    cx.background_executor.allow_parking();
    // A private runtime prevents polling detached tasks from unrelated GPUI tests.
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
    );
    let keep_runtime = runtime.clone();
    cx.update(|cx| {
        cx.on_app_quit(move |_| {
            let runtime = keep_runtime.clone();
            async move { drop(runtime) }
        })
        .detach()
    });
    let _runtime = runtime.enter();
    let root = tempdir().unwrap();
    let a_path = root.path().join("A.c");
    let c_path = root.path().join("C.txt");
    fs::write(&a_path, "alpha symbol\n").unwrap();
    fs::write(&c_path, "charlie\n").unwrap();
    let (core, workspace, cx) =
        new_test_workspace_with_runtime(cx, &a_path, runtime.handle().clone());
    let (stdin, server_stdin) = tokio::io::duplex(65536);
    let (mut server_stdout, stdout) = tokio::io::duplex(65536);
    let provider = Transport(Mutex::new(Some(ExternalTransport {
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
        stderr: Box::new(tokio::io::empty()),
        lifecycle_guard: Box::new(()),
    })));
    let mut reader = BufReader::new(server_stdin);
    let (a, a_slot, c, c_slot, server_id) = core.update(cx, |core, cx| {
        init_lsp_test_hooks(&core.editor.handlers);
        let a_slot = core.editor.tree.focus;
        let a = core.editor.tree.get(a_slot).doc;
        let c = core.editor.open(&c_path, Action::VerticalSplit).unwrap();
        let c_slot = core.editor.tree.focus;
        let loader = Arc::new(
            helix_core::syntax::Loader::new(
                helix_loader::config::default_lang_config()
                    .try_into()
                    .unwrap(),
            )
            .unwrap(),
        );
        let language = loader
            .language_configs()
            .find(|language| language.language_id == "c")
            .unwrap();
        core.editor.syn_loader.store(loader.clone());
        core.editor
            .document_mut(a)
            .unwrap()
            .set_language_by_language_id("c", &loader)
            .unwrap();
        let client = core
            .editor
            .language_servers
            .get_named_with_workspace_context_and_transport_provider(
                language,
                "clangd",
                None,
                &[],
                false,
                None,
                Some(&provider),
            )
            .unwrap()
            .unwrap();
        let server_id = client.id();
        assert!(
            core.editor
                .ensure_document_tracked_by_language_server(a, server_id)
        );
        let config: ConfigRaw =
            toml::from_str("[keys.normal]\nF12 = [\"goto_definition\", \"delete_selection\"]")
                .unwrap();
        let mut keys = helix_term::config::Config::default().keys;
        helix_term::keymap::merge_keys(&mut keys, config.keys.unwrap());
        core.editor_input =
            EditorInputBridge::new(Keymaps::new(Box::new(arc_swap::access::Constant(keys))));
        core.editor_input.enable_hosted_commands();
        cx.emit(crate::Update::Redraw);
        (a, a_slot, c, c_slot, server_id)
    });
    runtime.block_on(async {
        let initialize = tokio::time::timeout(Duration::from_secs(5), read(&mut reader)).await.unwrap();
        assert_eq!(initialize["method"], "initialize");
        write(&mut server_stdout, serde_json::json!({"jsonrpc":"2.0", "id":initialize["id"], "result":{"capabilities":{"definitionProvider":true}}})).await;
    });
    core.update(cx, |core, _| {
        let (id, call) = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                core.editor.language_servers.incoming.next(),
            )
            .await
            .unwrap()
            .unwrap()
        });
        assert_eq!(id, server_id);
        core.handle_language_server_message(call, id);
    });
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("ctrl-w h");
    cx.run_until_parked();
    core.read_with(cx, |core, _| assert_eq!(core.editor.tree.focus, a_slot));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.simulate_keystrokes("f12");
    cx.run_until_parked();
    let request = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let message = read(&mut reader).await;
                if message["method"] == "textDocument/definition" {
                    break message;
                }
            }
        })
        .await
        .expect("the real server must observe a definition request")
    });
    core.read_with(cx, |core, _| {
        assert_eq!(
            core.editor.documents[&a].text().to_string(),
            "alpha symbol\n"
        );
        assert_eq!(core.editor.documents[&c].text().to_string(), "charlie\n");
    });
    let pane = workspace.read_with(cx, |workspace, _| {
        workspace.pane_id_for_slot(c_slot).unwrap()
    });
    let selector = Box::leak(format!("pane-drop-body-{pane:?}").into_boxed_str());
    let position = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
    cx.simulate_keystrokes("h");
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.focus, c_slot);
        assert_eq!(
            core.editor.documents[&c].text().to_string(),
            "charlie\n",
            "an unrelated physical key must not release the old tail"
        );
    });
    // Release exactly the retained request, using its real JSON-RPC ID.
    let uri = helix_lsp::Url::from_file_path(&a_path).unwrap();
    runtime.block_on(write(&mut server_stdout, serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":{"uri":uri, "range":{"start":{"line":0,"character":6},"end":{"line":0,"character":12}}}})));
    for _ in 0..20 {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(5)).await });
        cx.run_until_parked();
    }
    core.read_with(cx, |core, _| {
        assert_eq!(
            core.editor.tree.focus, c_slot,
            "stale definition completion must not steal focus"
        );
        assert_eq!(core.editor.documents[&c].text().to_string(), "charlie\n");
        assert_eq!(
            core.editor.documents[&a].text().to_string(),
            "alpha symbol\n"
        );
    });
    // Positive control: a second request with unchanged origin must release its
    // own tail after its matching response, without needing another key.
    cx.simulate_keystrokes("ctrl-w h");
    cx.run_until_parked();
    core.read_with(cx, |core, _| assert_eq!(core.editor.tree.focus, a_slot));
    cx.simulate_keystrokes("f12");
    cx.run_until_parked();
    let matching = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let message = read(&mut reader).await;
                if message["method"] == "textDocument/definition" {
                    break message;
                }
            }
        })
        .await
        .expect("the positive control must issue a second real request")
    });
    assert_ne!(matching["id"], request["id"]);
    core.read_with(cx, |core, _| {
        assert_eq!(
            core.editor.documents[&a].text().to_string(),
            "alpha symbol\n"
        )
    });
    runtime.block_on(write(&mut server_stdout, serde_json::json!({"jsonrpc":"2.0", "id":matching["id"], "result":{"uri":uri, "range":{"start":{"line":0,"character":6},"end":{"line":0,"character":12}}}})));
    for _ in 0..100 {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(5)).await });
        cx.run_until_parked();
        if core.read_with(cx, |core, _| core.editor.documents[&a].is_modified()) {
            break;
        }
    }
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.focus, a_slot);
        // The definition response selects the whole 6..12 symbol range.
        assert_eq!(core.editor.documents[&a].text().to_string(), "alpha \n");
        assert_eq!(core.editor.documents[&c].text().to_string(), "charlie\n");
    });
    cx.update(|window, _| window.remove_window());
    drop(workspace);
    drop(core);
    cx.run_until_parked();
}
