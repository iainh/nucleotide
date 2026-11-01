use super::*;
use gpui::{Focusable, TestAppContext};

#[gpui::test]
fn move_hide_remount_preserves_identity_and_rejects_old_owner(cx: &mut TestAppContext) {
    let runtime = TerminalRuntimeHandle::new();
    let mut sessions = TerminalSessions::new(41);
    let id = TerminalId(900_001);
    cx.update(|cx| {
        assert!(sessions.open(
            id,
            TerminalSessionCfg::default(),
            Some(RunId(7)),
            &runtime,
            cx
        ));
        assert!(!sessions.open(id, TerminalSessionCfg::default(), None, &runtime, cx));
        let view = sessions.get(id).unwrap().view.clone();
        let focus = sessions.get(id).unwrap().focus.clone();
        let model = view.read(cx).model.clone();
        let first = TerminalBounds::from_cells(8.0, 16.0, 93, 27);
        let second = TerminalBounds::from_cells(9.0, 18.0, 61, 19);
        assert!(sessions.set_host(id, TerminalHost(1), true));
        assert!(sessions.resize(id, TerminalHost(1), first, &runtime));
        assert!(sessions.set_host(id, TerminalHost(2), false));
        assert!(!sessions.resize(id, TerminalHost(1), second, &runtime));
        assert!(!sessions.resize(id, TerminalHost(2), second, &runtime));
        assert_eq!(sessions.get(id).unwrap().last_bounds, Some(first));
        assert!(sessions.set_host(id, TerminalHost(2), true));
        assert!(sessions.resize(id, TerminalHost(2), second, &runtime));

        // Recreate chrome, not the terminal: both pane content and dock use this entity.
        let panel = cx.new(|cx| {
            nucleotide_terminal_panel::TerminalPanel::with_view(id, 240.0, view.clone(), cx)
        });
        assert_eq!(panel.focus_handle(cx), focus);
        assert_eq!(panel.read(cx).view_entity.as_ref().unwrap(), &view);
        drop(panel);
        assert_eq!(sessions.get(id).unwrap().view, view);
        assert_eq!(sessions.get(id).unwrap().focus, focus);
        assert!(std::sync::Arc::ptr_eq(&view.read(cx).model, &model));
        assert_eq!(sessions.close(id, &runtime).unwrap().run_id, Some(RunId(7)));
        assert!(!sessions.open(id, TerminalSessionCfg::default(), None, &runtime, cx));
    });
}

#[gpui::test]
fn completion_is_identified_retained_and_idempotent_across_generations(cx: &mut TestAppContext) {
    let runtime = TerminalRuntimeHandle::new();
    let mut sessions = TerminalSessions::new(8);
    let a = TerminalId(900_002);
    let b = TerminalId(900_003);
    cx.update(|cx| {
        assert!(sessions.open(
            a,
            TerminalSessionCfg::default(),
            Some(RunId(11)),
            &runtime,
            cx
        ));
        assert!(sessions.open(
            b,
            TerminalSessionCfg::default(),
            Some(RunId(12)),
            &runtime,
            cx
        ));
    });
    let exit = TerminalEvent::Exited {
        id: a,
        code: Some(7),
        signal: None,
    };
    assert!(!sessions.apply_event(7, &exit));
    assert!(sessions.apply_event(8, &TerminalEvent::Spawned { id: a }));
    assert!(!sessions.apply_event(8, &TerminalEvent::Spawned { id: a }));
    assert!(sessions.apply_event(8, &exit));
    assert!(!sessions.apply_event(8, &exit));
    assert!(!sessions.apply_event(8, &TerminalEvent::Spawned { id: a }));
    assert_eq!(sessions.get(a).unwrap().run_id, Some(RunId(11)));
    assert_eq!(
        sessions.get(a).unwrap().state,
        TerminalSessionState::Exited {
            code: Some(7),
            signal: None
        }
    );
    assert_eq!(
        sessions.get(b).unwrap().state,
        TerminalSessionState::Spawning
    );
    assert!(sessions.apply_event(
        8,
        &TerminalEvent::SpawnFailed {
            id: b,
            error: "missing shell".into()
        }
    ));
    assert!(!sessions.apply_event(
        8,
        &TerminalEvent::Exited {
            id: b,
            code: None,
            signal: None
        }
    ));
    assert_eq!(sessions.iter().count(), 2);
    let closed = sessions.close(a, &runtime).unwrap();
    assert_eq!(
        closed.state,
        TerminalSessionState::Exited {
            code: Some(7),
            signal: None
        }
    );
    assert!(sessions.close(a, &runtime).is_none());
    sessions.reset(9, &runtime);
    assert!(!sessions.apply_event(8, &exit));
    assert!(sessions.get(a).is_none());
    assert!(runtime.prepare(a).is_none());
}

#[cfg(unix)]
#[gpui::test]
fn native_process_survives_host_move_hidden_output_and_chrome_remount(cx: &mut TestAppContext) {
    use nucleotide_terminal_view::TerminalViewModel;
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    async fn wait_title(model: &Arc<Mutex<TerminalViewModel>>, suffix: &str) -> String {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let title = model
                    .lock()
                    .unwrap()
                    .window_title()
                    .unwrap_or_default()
                    .to_owned();
                if title.ends_with(suffix) {
                    break title;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }

    let executor = tokio::runtime::Runtime::new().unwrap();
    let runtime = TerminalRuntimeHandle::new();
    let mut sessions = TerminalSessions::new(1);
    let id = TerminalId(900_004);
    let spec = TerminalSessionCfg {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), r#"stty -echo; while IFS= read -r line; do printf '\033]2;%s|%s\007' "$$" "$line"; done"#.into()],
        ..Default::default()
    };
    let (view, focus, model) = cx.update(|cx| {
        assert!(sessions.open(id, spec.clone(), None, &runtime, cx));
        let session = sessions.get(id).unwrap();
        (
            session.view.clone(),
            session.focus.clone(),
            session.view.read(cx).model.clone(),
        )
    });
    executor.block_on(async {
        runtime.dispatch(&TerminalEvent::CommandSpawnRequested {
            id,
            cwd: spec.cwd,
            program: spec.program.unwrap(),
            args: spec.args,
            env: spec.env,
        });
        runtime.send_input(id, b"before\n".to_vec());
        let before = wait_title(&model, "|before").await;
        let pid = before.split('|').next().unwrap();
        sessions.set_host(id, TerminalHost(10), true);
        sessions.set_host(id, TerminalHost(20), false);
        runtime.send_input(id, b"hidden\n".to_vec());
        assert_eq!(wait_title(&model, "|hidden").await, format!("{pid}|hidden"));
        sessions.set_host(id, TerminalHost(30), true);
        cx.update(|cx| {
            let panel = cx.new(|cx| {
                nucleotide_terminal_panel::TerminalPanel::with_view(id, 240.0, view.clone(), cx)
            });
            assert_eq!(panel.focus_handle(cx), focus);
            assert_eq!(panel.read(cx).view_entity.as_ref(), Some(&view));
            drop(panel);
            assert_eq!(sessions.get(id).unwrap().view, view);
        });
        runtime.send_input(id, b"remounted\n".to_vec());
        assert_eq!(
            wait_title(&model, "|remounted").await,
            format!("{pid}|remounted")
        );
        sessions.close(id, &runtime);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !model.lock().unwrap().has_exited() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    });
}
