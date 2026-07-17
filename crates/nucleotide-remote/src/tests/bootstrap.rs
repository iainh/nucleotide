// Startup, helper-process, and bootstrap single-flight tests.

#[cfg(any(unix, windows))]
fn silent_service_command() -> RemoteServiceCommand {
    #[cfg(unix)]
    return RemoteServiceCommand {
        program: OsString::from("/bin/sleep"),
        args: vec![OsString::from("60")],
        current_dir: None,
    };

    #[cfg(windows)]
    RemoteServiceCommand {
        program: OsString::from("cmd.exe"),
        args: ["/D", "/Q", "/C", "ping.exe -n 60 127.0.0.1 >NUL"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        current_dir: None,
    }
}

#[cfg(any(unix, windows))]
#[test]
fn child_handshake_watchdog_physically_aborts_and_reaps_silent_helper() {
    let command = silent_service_command();
    let (io, control) = spawn_child_process_v5_io(&command).unwrap();

    let result = connect_child_process_v5_client_with_timeout(
        io,
        Arc::clone(&control),
        protocol_v5::ClientHello::nucleotide("test-client"),
        Duration::from_millis(50),
    );

    assert!(result.is_err());
    let started = Instant::now();
    while !control.was_reaped() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for silent v5 child to be reaped"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(any(unix, windows))]
#[test]
fn child_handshake_cancellation_physically_aborts_and_reaps_silent_helper() {
    let command = silent_service_command();
    let (io, control) = spawn_child_process_v5_io(&command).unwrap();
    let cancellation = WorkspaceCancellationToken::new();
    let cancel = cancellation.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancel.cancel();
    });

    let result = connect_child_process_v5_client_with_timeout_and_cancellation(
        io,
        Arc::clone(&control),
        protocol_v5::ClientHello::nucleotide("test-client"),
        Duration::from_secs(5),
        Some(cancellation),
    );
    canceller.join().unwrap();

    assert!(result.is_err());
    let started = Instant::now();
    while !control.was_reaped() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out waiting for cancelled v5 child to be reaped"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn pre_cancelled_startup_does_not_spawn_remote_service() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("spawned");
    let command = RemoteServiceCommand {
        program: OsString::from("/bin/sh"),
        args: vec![
            OsString::from("-c"),
            OsString::from(format!("touch '{}'; sleep 60", marker.display())),
        ],
        current_dir: None,
    };
    let cancellation = WorkspaceCancellationToken::new();
    cancellation.cancel();
    let startup =
        RemoteStartupContext::with_cancellation(cancellation, DEFAULT_REMOTE_STARTUP_TIMEOUT);

    let error = match spawn_child_process_workspace_backend_with_startup_context(
        loopback_identity(),
        &command,
        &startup,
    ) {
        Ok(_) => panic!("pre-cancelled startup unexpectedly spawned a service"),
        Err(error) => error,
    };

    assert!(remote_startup_was_cancelled(&error));
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn helper_commands_share_one_startup_deadline() {
    let options = RemoteWorkspaceBackendOptions::default();
    let startup = RemoteStartupContext::new(Duration::from_secs(2));
    let manager = RemoteHelperManager::with_progress_and_startup_context(&options, None, &startup);
    let mut first = nucleotide_process::contained_command("/bin/sleep");
    first.arg("0.75");
    manager
        .run_bounded_command(
            &mut first,
            nucleotide_process::OutputLimits::new(
                Duration::from_secs(5),
                REMOTE_STARTUP_OUTPUT_LIMIT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
            ),
        )
        .unwrap();
    let mut second = nucleotide_process::contained_command("/bin/sleep");
    second.arg("60");
    let started = Instant::now();

    let error = manager
        .run_bounded_command(
            &mut second,
            nucleotide_process::OutputLimits::new(
                Duration::from_secs(5),
                REMOTE_STARTUP_OUTPUT_LIMIT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
            ),
        )
        .unwrap_err();

    assert!(remote_startup_deadline_was_exceeded(&error));
    assert!(
        started.elapsed() < Duration::from_millis(1600),
        "second stage restarted the aggregate startup deadline"
    );
}

#[cfg(unix)]
#[test]
fn helper_command_cancellation_is_typed_and_prompt() {
    let options = RemoteWorkspaceBackendOptions::default();
    let cancellation = WorkspaceCancellationToken::new();
    let startup = RemoteStartupContext::with_cancellation(
        cancellation.clone(),
        DEFAULT_REMOTE_STARTUP_TIMEOUT,
    );
    let manager = RemoteHelperManager::with_progress_and_startup_context(&options, None, &startup);
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancellation.cancel();
    });
    let mut command = nucleotide_process::contained_command("/bin/sleep");
    command.arg("60");
    let started = Instant::now();

    let error = manager
        .run_bounded_command(
            &mut command,
            nucleotide_process::OutputLimits::new(
                Duration::from_secs(5),
                REMOTE_STARTUP_OUTPUT_LIMIT,
                REMOTE_STARTUP_OUTPUT_LIMIT,
            ),
        )
        .unwrap_err();
    canceller.join().unwrap();

    assert!(remote_startup_was_cancelled(&error));
    assert!(started.elapsed() < Duration::from_secs(1));
}
#[test]
fn owned_startup_attempt_cancels_on_drop_and_can_be_disarmed() {
    let cancelled_attempt = RemoteStartupAttempt::new(DEFAULT_REMOTE_STARTUP_TIMEOUT);
    let cancelled_context = cancelled_attempt.context();
    drop(cancelled_attempt);
    assert!(remote_startup_was_cancelled(
        &cancelled_context.check().unwrap_err()
    ));

    let mut completed_attempt = RemoteStartupAttempt::new(DEFAULT_REMOTE_STARTUP_TIMEOUT);
    let completed_context = completed_attempt.context();
    completed_attempt.disarm();
    drop(completed_attempt);
    assert!(completed_context.check().is_ok());
}

#[test]
fn startup_context_preserves_large_timeout_without_shortening() {
    let startup = RemoteStartupContext::new(Duration::MAX);

    assert!(startup.remaining().unwrap() > DEFAULT_REMOTE_STARTUP_TIMEOUT);
}

#[test]
fn bootstrap_cache_single_flights_concurrent_callers() {
    let cache = Arc::new(RemoteSingleFlightCache::new(4, Duration::from_secs(5)));
    let calls = Arc::new(AtomicUsize::new(0));
    let start = Arc::new(Barrier::new(9));
    let callers = (0..8)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                let startup = RemoteStartupContext::new(Duration::from_secs(5));
                start.wait();
                cache.get_or_try_init("host".to_string(), &startup, || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(50));
                    Ok(42)
                })
            })
        })
        .collect::<Vec<_>>();
    start.wait();

    for caller in callers {
        assert_eq!(caller.join().unwrap().unwrap(), 42);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn bootstrap_cache_follower_cancellation_does_not_cancel_leader() {
    let cache = Arc::new(RemoteSingleFlightCache::new(4, Duration::from_secs(5)));
    let calls = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let leader = {
        let cache = Arc::clone(&cache);
        let calls = Arc::clone(&calls);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            cache.get_or_try_init("host".to_string(), &startup, || {
                calls.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(11)
            })
        })
    };
    started_rx.recv().unwrap();

    let cancellation = WorkspaceCancellationToken::new();
    let follower = {
        let cache = Arc::clone(&cache);
        let startup =
            RemoteStartupContext::with_cancellation(cancellation.clone(), Duration::from_secs(5));
        std::thread::spawn(move || cache.get_or_try_init("host".to_string(), &startup, || Ok(22)))
    };
    cancellation.cancel();
    let follower_error = follower.join().unwrap().unwrap_err();
    assert!(remote_startup_was_cancelled(&follower_error));

    release_tx.send(()).unwrap();
    assert_eq!(leader.join().unwrap().unwrap(), 11);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let startup = RemoteStartupContext::new(Duration::from_secs(5));
    assert_eq!(
        cache
            .get_or_try_init("host".to_string(), &startup, || {
                panic!("successful leader result was not cached")
            })
            .unwrap(),
        11
    );
}

#[test]
fn bootstrap_cache_failure_and_cancelled_publication_are_not_sticky() {
    let cache = RemoteSingleFlightCache::new(4, Duration::from_secs(5));
    let startup = RemoteStartupContext::new(Duration::from_secs(5));
    let first_error = cache
        .get_or_try_init("failed".to_string(), &startup, || {
            Err::<usize, _>(anyhow::anyhow!("probe failed"))
        })
        .unwrap_err();
    assert!(first_error.to_string().contains("probe failed"));
    assert_eq!(
        cache
            .get_or_try_init("failed".to_string(), &startup, || Ok(7))
            .unwrap(),
        7
    );

    let cancellation = WorkspaceCancellationToken::new();
    let cancelled_startup =
        RemoteStartupContext::with_cancellation(cancellation.clone(), Duration::from_secs(5));
    let cancelled_error = cache
        .get_or_try_init("cancelled".to_string(), &cancelled_startup, || {
            cancellation.cancel();
            Ok(1)
        })
        .unwrap_err();
    assert!(remote_startup_was_cancelled(&cancelled_error));
    assert_eq!(
        cache
            .get_or_try_init("cancelled".to_string(), &startup, || Ok(2))
            .unwrap(),
        2
    );
}

#[test]
fn bootstrap_cache_forced_refresh_excludes_ordinary_resolution() {
    let cache = Arc::new(RemoteSingleFlightCache::new(4, Duration::from_secs(5)));
    let startup = RemoteStartupContext::new(Duration::from_secs(5));
    let original = cache
        .get_or_try_init_controlled("host".to_string(), &startup, || {
            Ok(RemoteSingleFlightLoad::Cache("old".to_string()))
        })
        .unwrap();
    let refresh_calls = Arc::new(AtomicUsize::new(0));
    let ordinary_calls = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let refresh = {
        let cache = Arc::clone(&cache);
        let refresh_calls = Arc::clone(&refresh_calls);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            cache.refresh_after("host".to_string(), original.generation, &startup, || {
                refresh_calls.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok("new".to_string())
            })
        })
    };
    started_rx.recv().unwrap();

    let ordinary = {
        let cache = Arc::clone(&cache);
        let ordinary_calls = Arc::clone(&ordinary_calls);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            cache.get_or_try_init("host".to_string(), &startup, || {
                ordinary_calls.fetch_add(1, Ordering::SeqCst);
                Ok("ordinary".to_string())
            })
        })
    };
    let refresh_follower = {
        let cache = Arc::clone(&cache);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            cache.refresh_after("host".to_string(), original.generation, &startup, || {
                panic!("refresh follower unexpectedly became the leader")
            })
        })
    };

    release_tx.send(()).unwrap();
    let refreshed = refresh.join().unwrap().unwrap();
    assert_eq!(refreshed.value, "new");
    assert_eq!(ordinary.join().unwrap().unwrap(), "new");
    let follower = refresh_follower.join().unwrap().unwrap();
    assert_eq!(follower.value, "new");
    assert_eq!(follower.generation, refreshed.generation);
    assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
    assert_eq!(ordinary_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn bootstrap_cache_stale_refresh_cannot_replace_newer_generation() {
    let cache = RemoteSingleFlightCache::new(4, Duration::from_secs(5));
    let startup = RemoteStartupContext::new(Duration::from_secs(5));
    let original = cache
        .get_or_try_init_controlled("host".to_string(), &startup, || {
            Ok(RemoteSingleFlightLoad::Cache("old".to_string()))
        })
        .unwrap();
    let refreshed = cache
        .refresh_after("host".to_string(), original.generation, &startup, || {
            Ok("new".to_string())
        })
        .unwrap();
    assert_ne!(refreshed.generation, original.generation);

    let stale = cache
        .refresh_after("host".to_string(), original.generation, &startup, || {
            panic!("stale observation replaced a newer refresh")
        })
        .unwrap();
    assert_eq!(stale.value, "new");
    assert_eq!(stale.generation, refreshed.generation);

    let next = cache
        .refresh_after("host".to_string(), refreshed.generation, &startup, || {
            Ok("newer".to_string())
        })
        .unwrap();
    assert_eq!(next.value, "newer");
    assert_ne!(next.generation, refreshed.generation);
}

#[test]
fn bootstrap_does_not_cache_unvalidated_helper_overrides() {
    let helper_path = PathBuf::from("/opt/custom/nucleotide-remote");
    let options = RemoteWorkspaceBackendOptions {
        ssh_helper_path: Some(helper_path.clone()),
        ssh_helper_path_is_override: true,
        ..RemoteWorkspaceBackendOptions::default()
    };
    let bootstrap = RemoteWorkspaceBootstrap::new(options);
    let startup = RemoteStartupContext::new(Duration::from_secs(5));
    let manager =
        RemoteHelperManager::with_bootstrap_and_startup_context(&bootstrap, None, &startup);
    let location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: None,
        },
        path: PathBuf::from("/project"),
    };

    assert_eq!(
        manager.resolve_helper_for_location(&location).unwrap(),
        helper_path
    );
    assert!(bootstrap.cache.helpers.lock_state().entries.is_empty());
}

#[test]
fn bootstrap_cache_is_bounded_when_all_slots_are_resolving() {
    let cache = Arc::new(RemoteSingleFlightCache::new(1, Duration::from_secs(5)));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let leader = {
        let cache = Arc::clone(&cache);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            cache.get_or_try_init("first".to_string(), &startup, || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(1)
            })
        })
    };
    started_rx.recv().unwrap();

    let (second_started_tx, second_started_rx) = mpsc::channel();
    let second = {
        let cache = Arc::clone(&cache);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            cache.get_or_try_init("second".to_string(), &startup, || {
                second_started_tx.send(()).unwrap();
                Ok(2)
            })
        })
    };
    std::thread::sleep(Duration::from_millis(25));
    assert!(matches!(
        second_started_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    {
        let state = cache.lock_state();
        assert_eq!(state.entries.len(), 1);
        assert!(state.entries.contains_key("first"));
        assert!(!state.entries.contains_key("second"));
    }

    release_tx.send(()).unwrap();
    assert_eq!(leader.join().unwrap().unwrap(), 1);
    second_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert_eq!(second.join().unwrap().unwrap(), 2);
}

#[test]
fn bootstrap_cache_serializes_refresh_and_load_when_capacity_is_saturated() {
    let cache = Arc::new(RemoteSingleFlightCache::new(1, Duration::from_secs(5)));
    let startup = RemoteStartupContext::new(Duration::from_secs(5));
    let original = cache
        .get_or_try_init_controlled("target".to_string(), &startup, || {
            Ok(RemoteSingleFlightLoad::Cache("old".to_string()))
        })
        .unwrap();
    let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
    let (release_blocker_tx, release_blocker_rx) = mpsc::channel();
    let blocker = {
        let cache = Arc::clone(&cache);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            cache.get_or_try_init("blocker".to_string(), &startup, || {
                blocker_started_tx.send(()).unwrap();
                release_blocker_rx.recv().unwrap();
                Ok("blocker".to_string())
            })
        })
    };
    blocker_started_rx.recv().unwrap();

    let start = Arc::new(Barrier::new(3));
    let active_initializers = Arc::new(AtomicUsize::new(0));
    let overlaps = Arc::new(AtomicUsize::new(0));
    let refresh_calls = Arc::new(AtomicUsize::new(0));
    let ordinary_calls = Arc::new(AtomicUsize::new(0));
    let refresh = {
        let cache = Arc::clone(&cache);
        let start = Arc::clone(&start);
        let active_initializers = Arc::clone(&active_initializers);
        let overlaps = Arc::clone(&overlaps);
        let refresh_calls = Arc::clone(&refresh_calls);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            start.wait();
            cache.refresh_after("target".to_string(), original.generation, &startup, || {
                if active_initializers.fetch_add(1, Ordering::SeqCst) != 0 {
                    overlaps.fetch_add(1, Ordering::SeqCst);
                }
                refresh_calls.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(25));
                active_initializers.fetch_sub(1, Ordering::SeqCst);
                Ok("new".to_string())
            })
        })
    };
    let ordinary = {
        let cache = Arc::clone(&cache);
        let start = Arc::clone(&start);
        let active_initializers = Arc::clone(&active_initializers);
        let overlaps = Arc::clone(&overlaps);
        let ordinary_calls = Arc::clone(&ordinary_calls);
        std::thread::spawn(move || {
            let startup = RemoteStartupContext::new(Duration::from_secs(5));
            start.wait();
            cache.get_or_try_init("target".to_string(), &startup, || {
                if active_initializers.fetch_add(1, Ordering::SeqCst) != 0 {
                    overlaps.fetch_add(1, Ordering::SeqCst);
                }
                ordinary_calls.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(25));
                active_initializers.fetch_sub(1, Ordering::SeqCst);
                Ok("ordinary".to_string())
            })
        })
    };
    start.wait();
    release_blocker_tx.send(()).unwrap();
    assert_eq!(blocker.join().unwrap().unwrap(), "blocker");

    let refreshed = refresh.join().unwrap().unwrap();
    let ordinary = ordinary.join().unwrap().unwrap();
    assert_eq!(refreshed.value, "new");
    assert!(ordinary == "ordinary" || ordinary == "new");
    assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
    assert!(ordinary_calls.load(Ordering::SeqCst) <= 1);
    assert_eq!(overlaps.load(Ordering::SeqCst), 0);
    assert_eq!(
        cache
            .get_or_try_init("target".to_string(), &startup, || {
                panic!("final refreshed generation was not cached")
            })
            .unwrap(),
        "new"
    );
}

#[test]
fn bootstrap_cache_expires_entries_and_evicts_least_recently_used() {
    let cache = RemoteSingleFlightCache::new(2, Duration::from_secs(5));
    let startup = RemoteStartupContext::new(Duration::from_secs(5));
    assert_eq!(
        cache
            .get_or_try_init("first".to_string(), &startup, || Ok(1))
            .unwrap(),
        1
    );
    assert_eq!(
        cache
            .get_or_try_init("second".to_string(), &startup, || Ok(2))
            .unwrap(),
        2
    );
    assert_eq!(
        cache
            .get_or_try_init("first".to_string(), &startup, || {
                panic!("cache hit unexpectedly invoked initializer")
            })
            .unwrap(),
        1
    );
    assert_eq!(
        cache
            .get_or_try_init("third".to_string(), &startup, || Ok(3))
            .unwrap(),
        3
    );
    {
        let state = cache.lock_state();
        assert_eq!(state.entries.len(), 2);
        assert!(state.entries.contains_key("first"));
        assert!(!state.entries.contains_key("second"));
        assert!(state.entries.contains_key("third"));
    }

    let expiring = RemoteSingleFlightCache::new(1, Duration::from_millis(10));
    assert_eq!(
        expiring
            .get_or_try_init("host".to_string(), &startup, || Ok(4))
            .unwrap(),
        4
    );
    std::thread::sleep(Duration::from_millis(25));
    assert_eq!(
        expiring
            .get_or_try_init("host".to_string(), &startup, || Ok(5))
            .unwrap(),
        5
    );
}

#[test]
fn bootstrap_transport_keys_ignore_roots_but_include_exact_ssh_options() {
    let first = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com/one"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: Some(2222),
        },
        path: PathBuf::from("/one"),
    };
    let second = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com/two"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: Some(2222),
        },
        path: PathBuf::from("/two"),
    };
    let options = RemoteWorkspaceBackendOptions::default();
    assert_eq!(
        RemoteBootstrapTransportKey::from_location(&first, &options),
        RemoteBootstrapTransportKey::from_location(&second, &options)
    );

    let mut different_options = options.clone();
    different_options
        .ssh_extra_args
        .push(OsString::from("ProxyJump=bastion"));
    different_options.ssh_control_path = Some(PathBuf::from("/tmp/different-control"));
    assert_ne!(
        RemoteBootstrapTransportKey::from_location(&first, &options),
        RemoteBootstrapTransportKey::from_location(&first, &different_options)
    );
    assert_ne!(
        RemoteBootstrapTransportKey::from_location(
            &WorkspaceLocation::Wsl {
                original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me"),
                distro: "Ubuntu".to_string(),
                linux_path: PathBuf::from("/home/me"),
            },
            &options,
        ),
        RemoteBootstrapTransportKey::from_location(
            &WorkspaceLocation::Wsl {
                original_path: PathBuf::from(r"\\wsl.localhost\Debian\home\me"),
                distro: "Debian".to_string(),
                linux_path: PathBuf::from("/home/me"),
            },
            &options,
        )
    );
}
