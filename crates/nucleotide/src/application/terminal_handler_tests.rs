use super::*;
use std::sync::atomic::AtomicU64;

static NEXT_ID: AtomicU64 = AtomicU64::new(800_000);

fn id() -> TerminalId {
    TerminalId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

async fn eventually(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("terminal contract did not become true");
}

fn record(runtime: &TerminalRuntimeHandle) -> Arc<Mutex<Vec<TerminalEvent>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let received = events.clone();
    runtime.set_event_sink(Arc::new(move |event| received.lock().unwrap().push(event)));
    events
}

fn title(model: &Arc<Mutex<TerminalViewModel>>) -> String {
    model
        .lock()
        .unwrap()
        .window_title()
        .unwrap_or_default()
        .to_owned()
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn simultaneous_native_ptys_have_independent_io_size_and_hidden_output() {
    let runtime = TerminalRuntimeHandle::new();
    let events = record(&runtime);
    let a = id();
    let b = id();
    let model_a = runtime.prepare(a).unwrap();
    let model_b = runtime.prepare(b).unwrap();
    for (id, tag, code) in [(a, "alpha", "7"), (b, "beta", "3")] {
        runtime.dispatch(&TerminalEvent::CommandSpawnRequested {
            id, cwd: None, program: "/bin/sh".into(),
            args: vec!["-c".into(), r#"stty -echo; while IFS= read -r line; do if [ "$line" = quit ]; then exit "$CODE"; fi; printf '\033]2;%s|%s|%s|%s\007' "$TAG" "$$" "$line" "$(stty size)"; done"#.into()],
            env: vec![("TAG".into(), tag.into()), ("CODE".into(), code.into())],
        });
        // This is deliberately before the asynchronous spawn finishes.
        assert!(runtime.send_input(id, b"initial\n".to_vec()));
    }
    eventually(|| title(&model_a).contains("alpha|") && title(&model_b).contains("beta|")).await;
    let pid_a = title(&model_a).split('|').nth(1).unwrap().to_owned();
    let pid_b = title(&model_b).split('|').nth(1).unwrap().to_owned();
    assert_ne!(pid_a, pid_b);
    for (id, cols, rows) in [(a, 93, 27), (b, 61, 19)] {
        runtime.dispatch(&TerminalEvent::Resized {
            id,
            cols,
            rows,
            cell_width: 8.0,
            cell_height: 16.0,
        });
    }
    // A hidden/unlaid-out surface must not replace the last usable PTY size with zero.
    runtime.dispatch(&TerminalEvent::Resized {
        id: a,
        cols: 0,
        rows: 0,
        cell_width: 8.0,
        cell_height: 16.0,
    });
    // Query the child PTYs, not cached runtime bounds. No TerminalView is mounted.
    eventually(|| {
        runtime.send_input(a, b"hidden-a\n".to_vec());
        runtime.send_input(b, b"hidden-b\n".to_vec());
        title(&model_a) == format!("alpha|{pid_a}|hidden-a|27 93")
            && title(&model_b) == format!("beta|{pid_b}|hidden-b|19 61")
    })
    .await;
    assert!(Arc::ptr_eq(&model_a, &runtime.prepare(a).unwrap()));
    assert!(Arc::ptr_eq(&model_b, &runtime.prepare(b).unwrap()));

    // An observed fact is never a request to kill this process.
    runtime.dispatch(&TerminalEvent::Exited {
        id: a,
        code: Some(99),
        signal: None,
    });
    runtime.send_input(a, b"still-alive\n".to_vec());
    eventually(|| title(&model_a).contains("|still-alive|27 93")).await;
    runtime.send_input(a, b"quit\n".to_vec());
    eventually(|| {
        events.lock().unwrap().iter().any(
            |event| matches!(event, TerminalEvent::Exited { id, code: Some(7), .. } if *id == a),
        )
    })
    .await;
    assert!(model_a.lock().unwrap().has_exited());
    assert!(Arc::ptr_eq(&model_a, &runtime.prepare(a).unwrap()));
    runtime.send_input(b, b"independent\n".to_vec());
    eventually(|| title(&model_b).contains("|independent|19 61")).await;
    runtime.dispatch(&TerminalEvent::TerminateRequested { id: b });
    runtime.dispatch(&TerminalEvent::TerminateRequested { id: b });
    eventually(|| {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exited { id, .. } if *id == b))
    })
    .await;
    for id in [a, b] {
        let events = events.lock().unwrap();
        assert_eq!(events.iter().filter(|event| matches!(event, TerminalEvent::Exited { id: event_id, .. } if *event_id == id)).count(), 1);
        assert!(matches!(events.iter().find(|event| matches!(event, TerminalEvent::Spawned { id: event_id } | TerminalEvent::Exited { id: event_id, .. } if *event_id == id)), Some(TerminalEvent::Spawned { .. })));
    }
    runtime.dispatch(&TerminalEvent::TerminateRequested { id: a });
    assert!(runtime.prepare(a).is_none());
    assert!(nucleotide_terminal_view::get_view_model(a).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_failure_keeps_same_model_and_duplicate_requests_do_not_restart() {
    let runtime = TerminalRuntimeHandle::new();
    let events = record(&runtime);
    let id = id();
    let model = runtime.prepare(id).unwrap();
    let request = TerminalEvent::CommandSpawnRequested {
        id,
        cwd: None,
        program: "/nucleotide-does-not-exist/shell".into(),
        args: vec![],
        env: vec![],
    };
    runtime.dispatch(&request);
    eventually(|| {
        events.lock().unwrap().iter().any(
            |event| matches!(event, TerminalEvent::SpawnFailed { id: failed, .. } if *failed == id),
        )
    })
    .await;
    assert!(Arc::ptr_eq(&model, &runtime.prepare(id).unwrap()));
    assert!(
        model
            .lock()
            .unwrap()
            .spawn_failure()
            .unwrap()
            .details
            .iter()
            .any(|detail| detail.contains("nucleotide-does-not-exist"))
    );
    runtime.dispatch(&request);
    assert_eq!(events.lock().unwrap().len(), 1);
    runtime.dispatch(&TerminalEvent::TerminateRequested { id });
    runtime.dispatch(&request); // Late asynchronous environment resolution must not resurrect it.
    assert!(runtime.prepare(id).is_none());
    assert!(nucleotide_terminal_view::get_view_model(id).is_none());
}

struct TestControl(Arc<AtomicBool>);
impl nucleotide_remote::RemotePtySessionControl for TestControl {
    fn resize(
        &self,
        _: u16,
        _: u16,
        _: u16,
        _: u16,
    ) -> Result<(), nucleotide_remote::RemoteClientError> {
        Ok(())
    }
    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct DelayedLauncher {
    gate: Arc<(Mutex<bool>, Condvar)>,
    started: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    request: Arc<Mutex<Option<nucleotide_remote::RemotePtySessionRequest>>>,
}
impl nucleotide_remote::RemotePtySessionLauncher for DelayedLauncher {
    fn launch(
        &self,
        request: nucleotide_remote::RemotePtySessionRequest,
    ) -> Result<nucleotide_remote::RemotePtySessionParts, nucleotide_remote::RemoteClientError>
    {
        *self.request.lock().unwrap() = Some(request);
        self.started.store(true, Ordering::SeqCst);
        let (lock, wake) = &*self.gate;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = wake.wait(ready).unwrap();
        }
        Ok(nucleotide_remote::RemotePtySessionParts {
            input: Box::pin(tokio::io::sink()),
            output: Box::pin(tokio::io::empty()),
            completion: Box::pin(async {
                Ok(nucleotide_remote::ProcessSessionCompletion {
                    status_code: None,
                    success: true,
                })
            }),
            control: Box::new(TestControl(self.cancelled.clone())),
            lifecycle: Box::new(()),
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn delayed_remote_spawn_close_cancels_before_registration_and_unknown_exit_is_observed() {
    for close_before_registration in [true, false] {
        let runtime = TerminalRuntimeHandle::new();
        let events = record(&runtime);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let started = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = Arc::new(Mutex::new(None));
        runtime.set_remote_provider(Some(RemoteTerminalSessionProvider {
            display_root: "remote-workspace".into(),
            native_root: "/native".into(),
            launcher: Arc::new(DelayedLauncher {
                gate: gate.clone(),
                started: started.clone(),
                cancelled: cancelled.clone(),
                request: request.clone(),
            }),
        }));
        let id = id();
        let model = runtime.prepare(id).unwrap();
        runtime.dispatch(&TerminalEvent::ServiceCommandSpawnRequested {
            id,
            cwd: "remote-workspace/src".into(),
            program: "task".into(),
            args: vec!["--test".into()],
            env: vec![("EXTRA".into(), "yes".into())],
        });
        eventually(|| started.load(Ordering::SeqCst)).await;
        if close_before_registration {
            runtime.dispatch(&TerminalEvent::TerminateRequested { id });
            assert!(runtime.prepare(id).is_none());
        }
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        if close_before_registration {
            eventually(|| cancelled.load(Ordering::SeqCst)).await;
            assert!(events.lock().unwrap().is_empty());
            assert!(nucleotide_terminal_view::get_view_model(id).is_none());
        } else {
            eventually(|| events.lock().unwrap().iter().any(|event| matches!(event, TerminalEvent::Exited { id: exited, code: None, .. } if *exited == id))).await;
            assert_eq!(events.lock().unwrap().len(), 2);
            assert!(model.lock().unwrap().has_exited());
            assert!(Arc::ptr_eq(&model, &runtime.prepare(id).unwrap()));
            runtime.dispatch(&TerminalEvent::TerminateRequested { id });
        }
        let request = request.lock().unwrap();
        let request = request.as_ref().unwrap();
        assert_eq!(request.cwd, PathBuf::from("/native/src"));
        assert_eq!(request.env_overrides.get("EXTRA").unwrap(), "yes");
        assert_eq!(
            request.command,
            nucleotide_remote::PtySessionCommand::Command {
                program: "task".into(),
                args: vec!["--test".into()],
            }
        );
    }
}
