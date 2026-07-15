// CLI parsing and proxy launch tests.

#[test]
fn serve_options_parse_v5_protocol() {
    let temp = tempfile::tempdir().unwrap();

    let options = parse_serve_options([
        "--workspace".to_string(),
        temp.path().display().to_string(),
        "--protocol".to_string(),
        "v5".to_string(),
    ])
    .unwrap();

    assert_eq!(options.workspace_root, temp.path());
}

// CLI parsing and LSP/terminal proxy tests.

#[test]
fn serve_options_reject_unknown_protocol() {
    let error = parse_serve_options(["--protocol".to_string(), "v9".to_string()])
        .expect_err("unsupported protocol should fail");

    assert!(error.to_string().contains("unsupported serve protocol"));
}

#[test]
fn lsp_proxy_options_parse_workspace_server_and_args() {
    let temp = tempfile::tempdir().unwrap();

    let options = parse_lsp_proxy_options([
        "--workspace".to_string(),
        temp.path().display().to_string(),
        "--server".to_string(),
        "rust-analyzer".to_string(),
        "--".to_string(),
        "--log-file".to_string(),
        "ra.log".to_string(),
    ])
    .unwrap();

    assert_eq!(options.workspace_root, temp.path());
    assert_eq!(options.server, "rust-analyzer");
    assert_eq!(options.server_args, ["--log-file", "ra.log"]);
}

#[test]
fn terminal_proxy_options_parse_shell_env_and_command() {
    let temp = tempfile::tempdir().unwrap();

    let options = parse_terminal_proxy_options([
        "--workspace".to_string(),
        temp.path().display().to_string(),
        "--shell".to_string(),
        "/bin/zsh".to_string(),
        "--env".to_string(),
        "RUST_LOG=debug".to_string(),
        "--".to_string(),
        "cargo".to_string(),
        "test".to_string(),
        "--workspace".to_string(),
    ])
    .unwrap();

    assert_eq!(options.workspace_root, temp.path());
    assert_eq!(options.shell.as_deref(), Some("/bin/zsh"));
    assert_eq!(
        options.env,
        vec![("RUST_LOG".to_string(), "debug".to_string())]
    );
    assert_eq!(
        options.command,
        Some((
            "cargo".to_string(),
            vec!["test".to_string(), "--workspace".to_string()]
        ))
    );
}

#[test]
fn terminal_proxy_options_reject_invalid_env_entry() {
    let error = parse_terminal_proxy_options(["--env".to_string(), "BAD".to_string()]).unwrap_err();

    assert!(error.to_string().contains("KEY=VALUE"));
}

#[test]
fn terminal_proxy_process_uses_environment_shell_as_login_shell_without_extra_flags() {
    let options = TerminalProxyOptions {
        workspace_root: PathBuf::from("/workspace"),
        shell: None,
        env: Vec::new(),
        command: None,
    };
    let environment = HashMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]);

    let process = terminal_proxy_process(&options, &environment);

    assert_eq!(process.program, "/bin/zsh");
    assert!(process.args.is_empty());
    assert!(process.login_shell);
}

#[test]
fn terminal_proxy_process_keeps_command_sessions_non_login() {
    let options = TerminalProxyOptions {
        workspace_root: PathBuf::from("/workspace"),
        shell: None,
        env: Vec::new(),
        command: Some((
            "cargo".to_string(),
            vec!["test".to_string(), "--workspace".to_string()],
        )),
    };
    let environment = HashMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]);

    let process = terminal_proxy_process(&options, &environment);

    assert_eq!(process.program, "cargo");
    assert_eq!(process.args, ["test", "--workspace"]);
    assert!(!process.login_shell);
}

#[test]
fn terminal_proxy_environment_removes_prompt_and_shell_startup_state() {
    let mut environment = HashMap::from([
        ("BASH_ENV".to_string(), "/tmp/bash-env".to_string()),
        ("BASHOPTS".to_string(), "cmdhist:progcomp".to_string()),
        ("ENV".to_string(), "/tmp/sh-env".to_string()),
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("POSIXLY_CORRECT".to_string(), "1".to_string()),
        ("PROMPT_COMMAND".to_string(), "echo prompt".to_string()),
        ("PS1".to_string(), "\\[broken\\]$ ".to_string()),
        ("SHELL".to_string(), "/bin/zsh".to_string()),
        ("SHELLOPTS".to_string(), "posix".to_string()),
    ]);

    remove_interactive_shell_state(&mut environment);

    for key in INTERACTIVE_SHELL_STATE_ENV_VARS {
        assert!(
            !environment.contains_key(*key),
            "{key} should not leak into remote terminal"
        );
    }
    assert_eq!(
        environment.get("SHELL").map(String::as_str),
        Some("/bin/zsh")
    );
    assert_eq!(
        environment.get("PATH").map(String::as_str),
        Some("/usr/bin:/bin")
    );
}

#[test]
fn login_shell_arg0_prefixes_program_basename() {
    assert_eq!(
        login_shell_arg0(Path::new("/bin/zsh")),
        OsString::from("-zsh")
    );
    assert_eq!(login_shell_arg0(Path::new("bash")), OsString::from("-bash"));
}

#[test]
fn lsp_proxy_resolves_server_from_project_environment_path() {
    let temp = tempfile::tempdir().unwrap();
    let server = temp.path().join("rust-analyzer");
    std::fs::write(&server, "").unwrap();
    let environment = HashMap::from([(
        "PATH".to_string(),
        std::env::join_paths([
            temp.path().to_path_buf(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ])
        .unwrap()
        .to_string_lossy()
        .into_owned(),
    )]);

    assert_eq!(
        resolve_program_from_environment_path("rust-analyzer", &environment, temp.path()),
        server
    );
    let absolute_server = temp.path().join("custom").join("rust-analyzer");
    assert_eq!(
        resolve_program_from_environment_path(
            &absolute_server.to_string_lossy(),
            &environment,
            temp.path()
        ),
        absolute_server
    );
    assert_eq!(
        resolve_program_from_environment_path(
            "./node_modules/.bin/typescript-language-server",
            &environment,
            temp.path()
        ),
        temp.path()
            .join("node_modules")
            .join(".bin")
            .join("typescript-language-server")
    );
}

struct CancelThenDisconnectProtocolClient {
    calls: Arc<AtomicUsize>,
}

impl RemoteWorkspaceProtocolClient for CancelThenDisconnectProtocolClient {
    fn request(
        &self,
        _request: RemoteRequest,
        _body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        Err(RemoteClientError::Disconnected)
    }

    fn request_with_context_and_cancellation(
        &self,
        _request: RemoteRequest,
        _body: Vec<u8>,
        _context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        cancellation.cancel();
        Err(RemoteClientError::Disconnected)
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        Ok(())
    }
}

#[derive(Default)]
struct CancellationObservingState {
    started: bool,
    cancelled: bool,
    finished: bool,
}

struct CancellationObservingProtocolClient {
    state: Arc<(StdMutex<CancellationObservingState>, Condvar)>,
}

impl RemoteWorkspaceProtocolClient for CancellationObservingProtocolClient {
    fn request(
        &self,
        _request: RemoteRequest,
        _body: Vec<u8>,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        Err(RemoteClientError::Disconnected)
    }

    fn request_with_context_and_cancellation(
        &self,
        request: RemoteRequest,
        _body: Vec<u8>,
        _context: RemoteRequestContext,
        cancellation: &RemoteRequestCancellation,
    ) -> std::result::Result<(RemoteResponse, Vec<u8>), RemoteClientError> {
        let (state, wake) = &*self.state;
        {
            let mut state = state.lock().unwrap();
            state.started = true;
            wake.notify_all();
        }
        let callback_state = Arc::clone(&self.state);
        cancellation.register(move || {
            let (state, wake) = &*callback_state;
            state.lock().unwrap().cancelled = true;
            wake.notify_all();
        });
        let mut state = state.lock().unwrap();
        while !state.cancelled {
            state = wake.wait(state).unwrap();
        }
        state.finished = true;
        wake.notify_all();
        Err(remote_request_cancelled_error(request.v5_method()))
    }

    fn shutdown(&self) -> std::result::Result<(), RemoteClientError> {
        Ok(())
    }
}
