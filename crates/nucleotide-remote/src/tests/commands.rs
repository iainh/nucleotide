// Command construction, configuration, deployment, and helper-install tests.

fn loopback_identity() -> RemoteWorkspaceIdentity {
    RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Other("loopback".to_string()),
        name: "loopback".to_string(),
    }
}

#[test]
fn process_output_response_defaults_missing_timeout_flag() {
    let response: ProcessOutputResponse = serde_json::from_value(serde_json::json!({
        "status_code": 0,
        "success": true,
        "stdout_truncated": false,
        "stderr_truncated": false,
        "stdout_len": 0,
        "stderr_len": 0
    }))
    .unwrap();

    assert!(!response.timed_out);
}

#[test]
fn remote_time_conversion_preserves_sub_millisecond_precision() {
    let time = UNIX_EPOCH + Duration::new(42, 123_456_789);
    let millis = system_time_unix_millis(time);
    let nanos = system_time_unix_nanos(time);

    assert_eq!(
        system_time_from_unix_millis_and_nanos(millis, nanos),
        Some(time)
    );
    assert_ne!(millis.and_then(system_time_from_unix_millis), Some(time));
}

#[test]
fn local_service_command_runs_helper_directly() {
    let spec = local_service_command("/tmp/nucleotide-remote", "/workspace/project");

    assert_eq!(spec.program, OsString::from("/tmp/nucleotide-remote"));
    assert_eq!(
        spec.args,
        vec![
            OsString::from("serve"),
            OsString::from("--workspace"),
            OsString::from("/workspace/project"),
            OsString::from("--protocol"),
            OsString::from("v5")
        ]
    );
    assert_eq!(spec.current_dir, Some(PathBuf::from("/workspace/project")));
    assert_arg_pair(&spec.args, "--protocol", "v5");
}

#[test]
fn service_command_display_quotes_arguments_and_cwd() {
    let spec = local_service_command(
        "/tmp/nucleotide remote",
        "/workspace/project with spaces/it's",
    );
    let quoted_workspace = "'/workspace/project with spaces/it'\"'\"'s'";

    assert_eq!(
        spec.display_invocation(),
        format!("'/tmp/nucleotide remote' serve --workspace {quoted_workspace} --protocol v5")
    );
    assert_eq!(
        spec.display_context(),
        format!(
            "'/tmp/nucleotide remote' serve --workspace {quoted_workspace} --protocol v5 (cwd {quoted_workspace})"
        )
    );
}

#[test]
fn wsl_service_command_uses_exec_without_shell() {
    let spec = wsl_service_command("Ubuntu", "/home/me/project", "/home/me/.cache/nucl/remote");

    assert_eq!(spec.program, OsString::from("wsl.exe"));
    assert_eq!(
        spec.args,
        vec![
            OsString::from("--distribution"),
            OsString::from("Ubuntu"),
            OsString::from("--cd"),
            OsString::from("/home/me/project"),
            OsString::from("--exec"),
            OsString::from("/home/me/.cache/nucl/remote"),
            OsString::from("serve"),
            OsString::from("--workspace"),
            OsString::from("/home/me/project"),
            OsString::from("--protocol"),
            OsString::from("v5")
        ]
    );
    assert_eq!(spec.current_dir, None);
    assert_arg_pair(&spec.args, "--protocol", "v5");
}

#[test]
fn wsl_shell_command_passes_deployment_script_as_one_argument() {
    let spec = wsl_shell_command(
        "Ubuntu Preview",
        "printf 'NUCL_PLATFORM '; uname -sm".to_string(),
    );

    assert_eq!(spec.program, OsString::from("wsl.exe"));
    assert_eq!(
        spec.args,
        vec![
            OsString::from("--distribution"),
            OsString::from("Ubuntu Preview"),
            OsString::from("--exec"),
            OsString::from("sh"),
            OsString::from("-lc"),
            OsString::from("printf 'NUCL_PLATFORM '; uname -sm"),
        ]
    );
    assert_eq!(spec.current_dir, None);
}

#[test]
fn wsl_custom_helper_path_bypasses_auto_install() {
    let helper_path = PathBuf::from("/opt/nucleotide/nucleotide-remote");
    let options = RemoteWorkspaceBackendOptions {
        wsl_helper_path: Some(helper_path.clone()),
        wsl_helper_path_is_override: true,
        ..RemoteWorkspaceBackendOptions::default()
    };
    let location = WorkspaceLocation::Wsl {
        original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
        distro: "Ubuntu".to_string(),
        linux_path: PathBuf::from("/home/me/project"),
    };

    assert_eq!(
        RemoteHelperManager::new(&options)
            .resolve_helper_for_location(&location)
            .unwrap(),
        helper_path
    );
}

#[test]
fn transport_specific_helper_paths_do_not_leak_between_ssh_and_wsl() {
    let ssh_helper = PathBuf::from("/opt/ssh/nucleotide-remote");
    let wsl_helper = PathBuf::from("/opt/wsl/nucleotide-remote");
    let options = RemoteWorkspaceBackendOptions {
        ssh_helper_path: Some(ssh_helper.clone()),
        ssh_helper_path_is_override: true,
        wsl_helper_path: Some(wsl_helper.clone()),
        wsl_helper_path_is_override: true,
        ..RemoteWorkspaceBackendOptions::default()
    };
    let ssh_location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: None,
        },
        path: PathBuf::from("/home/me/project"),
    };
    let wsl_location = WorkspaceLocation::Wsl {
        original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
        distro: "Ubuntu".to_string(),
        linux_path: PathBuf::from("/home/me/project"),
    };
    let manager = RemoteHelperManager::new(&options);

    assert_eq!(
        manager.resolve_helper_for_location(&ssh_location).unwrap(),
        ssh_helper
    );
    assert_eq!(
        manager.resolve_helper_for_location(&wsl_location).unwrap(),
        wsl_helper
    );
}

#[test]
fn wsl_lsp_proxy_command_uses_remote_helper() {
    let spec = wsl_lsp_proxy_command(
        "Ubuntu",
        "/home/me/project",
        "/home/me/.cache/nucl/remote",
        "rust-analyzer",
    );

    assert_eq!(spec.program, OsString::from("wsl.exe"));
    assert_eq!(
        spec.args,
        vec![
            OsString::from("--distribution"),
            OsString::from("Ubuntu"),
            OsString::from("--cd"),
            OsString::from("/home/me/project"),
            OsString::from("--exec"),
            OsString::from("/home/me/.cache/nucl/remote"),
            OsString::from("lsp-proxy"),
            OsString::from("--workspace"),
            OsString::from("/home/me/project"),
            OsString::from("--server"),
            OsString::from("rust-analyzer"),
            OsString::from("--"),
        ]
    );
    assert_eq!(spec.current_dir, None);
}

#[test]
fn wsl_terminal_proxy_command_uses_remote_helper() {
    let command_args = vec!["test".to_string()];
    let spec = wsl_terminal_proxy_command(
        "Ubuntu",
        "/home/me/project",
        "/home/me/.cache/nucl/remote",
        Some("/bin/zsh"),
        Some(("cargo", &command_args)),
        &[("RUST_LOG".to_string(), "debug".to_string())],
    );

    assert_eq!(spec.program, OsString::from("wsl.exe"));
    assert_eq!(
        spec.args,
        vec![
            OsString::from("--distribution"),
            OsString::from("Ubuntu"),
            OsString::from("--cd"),
            OsString::from("/home/me/project"),
            OsString::from("--exec"),
            OsString::from("/home/me/.cache/nucl/remote"),
            OsString::from("terminal-proxy"),
            OsString::from("--workspace"),
            OsString::from("/home/me/project"),
            OsString::from("--shell"),
            OsString::from("/bin/zsh"),
            OsString::from("--env"),
            OsString::from("RUST_LOG=debug"),
            OsString::from("--"),
            OsString::from("cargo"),
            OsString::from("test"),
        ]
    );
    assert_eq!(spec.current_dir, None);
}

#[test]
fn wsl_interactive_terminal_command_uses_distro_and_directory_without_helper() {
    let spec = wsl_interactive_terminal_command("Ubuntu", "/home/me/project");

    assert_eq!(spec.program, OsString::from("wsl.exe"));
    assert_eq!(
        spec.args,
        vec![
            OsString::from("--distribution"),
            OsString::from("Ubuntu"),
            OsString::from("--cd"),
            OsString::from("/home/me/project"),
        ]
    );
    assert_eq!(spec.current_dir, None);
}

#[test]
fn ssh_service_command_quotes_remote_paths() {
    let mut target = SshTarget::new("devbox");
    target.user = Some("me".to_string());
    target.port = Some(2222);

    let spec = ssh_service_command(
        target,
        "/home/me/project with spaces/it's",
        "/home/me/.cache/nucleotide remote/bin",
    );

    assert_eq!(spec.program, OsString::from("ssh"));
    assert_eq!(spec.args[0], OsString::from("-T"));
    assert_ssh_non_interactive_defaults(&spec.args);
    assert_arg_pair(&spec.args, "-p", "2222");
    let separator = ssh_target_separator_index(&spec.args);
    assert_eq!(spec.args[separator + 1], OsString::from("me@devbox"));
    let command = spec.args[separator + 2].to_string_lossy();
    assert!(command.starts_with("exec "));
    assert!(command.contains("'/home/me/.cache/nucleotide remote/bin'"));
    assert!(command.contains("'/home/me/project with spaces/it'\"'\"'s'"));
    assert!(command.contains("--protocol v5"));
}

#[test]
fn ssh_commands_normalize_remote_paths_to_posix() {
    let spec = ssh_service_command(
        SshTarget::new("devbox"),
        r"\home\me\project",
        r"\home\me\.cache\nucl\remote",
    );
    let separator = ssh_target_separator_index(&spec.args);
    let command = spec.args[separator + 2].to_string_lossy();

    assert!(command.contains("'/home/me/.cache/nucl/remote'"));
    assert!(command.contains("'/home/me/project'"));
    assert!(command.contains("--protocol v5"));

    let spec = ssh_terminal_proxy_command(
        SshTarget::new("devbox"),
        r"\home\me\project",
        r"\home\me\.cache\nucl\remote",
        None,
        None,
        &[],
    );
    let separator = ssh_target_separator_index(&spec.args);
    let command = spec.args[separator + 2].to_string_lossy();

    assert!(command.contains("'/home/me/.cache/nucl/remote'"));
    assert!(command.contains("'/home/me/project'"));
}

#[cfg(windows)]
#[test]
fn ssh_service_command_resolves_system_openssh_on_windows() {
    let Some(windir) = std::env::var_os("WINDIR") else {
        return;
    };
    let system_ssh = PathBuf::from(windir)
        .join("System32")
        .join("OpenSSH")
        .join("ssh.exe");
    if !system_ssh.is_file() {
        return;
    }

    let spec = ssh_service_command(
        SshTarget::new("devbox"),
        "/home/me/project",
        "/home/me/.cache/nucl/remote",
    );
    let command = spec.command();

    assert_eq!(
        command.get_program().to_string_lossy().to_ascii_lowercase(),
        system_ssh.to_string_lossy().to_ascii_lowercase()
    );
}

#[test]
fn ssh_lsp_proxy_command_quotes_remote_paths_and_server() {
    let mut target = SshTarget::new("devbox");
    target.user = Some("me".to_string());
    target.port = Some(2222);

    let spec = ssh_lsp_proxy_command(
        target,
        "/home/me/project with spaces/it's",
        "/home/me/.cache/nucleotide remote/bin",
        "typescript-language-server",
    );

    assert_eq!(spec.program, OsString::from("ssh"));
    assert_eq!(spec.args[0], OsString::from("-T"));
    assert_ssh_non_interactive_defaults(&spec.args);
    assert_arg_pair(&spec.args, "-p", "2222");
    let separator = ssh_target_separator_index(&spec.args);
    assert_eq!(spec.args[separator + 1], OsString::from("me@devbox"));
    let command = spec.args[separator + 2].to_string_lossy();
    assert!(command.starts_with("exec "));
    assert!(command.contains("'/home/me/.cache/nucleotide remote/bin'"));
    assert!(command.contains(" lsp-proxy "));
    assert!(command.contains("'/home/me/project with spaces/it'\"'\"'s'"));
    assert!(command.contains("typescript-language-server"));
    assert!(command.ends_with(" --"));
}

#[test]
fn ssh_interactive_terminal_command_reuses_ssh_options_and_starts_login_shell() {
    let mut target = SshTarget::new("devbox");
    target.user = Some("me".to_string());
    target.port = Some(2222);
    target.control_path = Some(PathBuf::from("/tmp/nucl-ssh/%C"));

    let spec = ssh_interactive_terminal_command(target, "/home/me/project with spaces");

    assert_eq!(spec.program, OsString::from("ssh"));
    assert_ssh_non_interactive_defaults(&spec.args);
    assert_arg_pair(&spec.args, "-p", "2222");
    assert_arg_pair(&spec.args, "-o", "ControlMaster=auto");
    let tty = arg_index(&spec.args, "-tt");
    let separator = ssh_target_separator_index(&spec.args);
    assert!(tty < separator);
    assert_eq!(spec.args[separator + 1], OsString::from("me@devbox"));
    let command = spec.args[separator + 2].to_string_lossy();
    assert!(command.starts_with("cd "));
    assert!(command.contains("'/home/me/project with spaces'"));
    assert!(command.contains("exec \"${SHELL:-/bin/sh}\" -l"));
}

#[test]
fn ssh_terminal_proxy_command_quotes_remote_command_and_forces_tty() {
    let mut target = SshTarget::new("devbox");
    target.user = Some("me".to_string());
    target.port = Some(2222);
    let command_args = vec!["test".to_string(), "--workspace".to_string()];

    let spec = ssh_terminal_proxy_command(
        target,
        "/home/me/project with spaces/it's",
        "/home/me/.cache/nucleotide remote/bin",
        None,
        Some(("cargo", &command_args)),
        &[("RUST_LOG".to_string(), "debug".to_string())],
    );

    assert_eq!(spec.program, OsString::from("ssh"));
    assert_ssh_non_interactive_defaults(&spec.args);
    assert_arg_pair(&spec.args, "-p", "2222");
    let tty = arg_index(&spec.args, "-tt");
    let separator = ssh_target_separator_index(&spec.args);
    assert!(tty < separator);
    assert_eq!(spec.args[separator + 1], OsString::from("me@devbox"));
    let command = spec.args[separator + 2].to_string_lossy();
    assert!(command.starts_with("exec "));
    assert!(command.contains("'/home/me/.cache/nucleotide remote/bin'"));
    assert!(command.contains(" terminal-proxy "));
    assert!(command.contains("'/home/me/project with spaces/it'\"'\"'s'"));
    assert!(command.contains("--env 'RUST_LOG=debug'"));
    assert!(command.contains(" -- 'cargo' 'test' "));
    assert!(command.ends_with("'--workspace'"));
}

#[test]
fn ssh_service_command_applies_connection_options_before_target() {
    let mut target = SshTarget::new("devbox");
    target.connect_timeout_secs = Some(12);
    target.control_path = Some(PathBuf::from("/tmp/nucl-ssh/%C"));
    target.extra_args = vec![
        OsString::from("-J"),
        OsString::from("bastion"),
        OsString::from("-F"),
        OsString::from("/tmp/ssh config"),
    ];

    let spec = ssh_service_command(target, "/home/me/project", "/remote/bin/nucleotide-remote");

    let separator = ssh_target_separator_index(&spec.args);
    assert_eq!(spec.args[0], OsString::from("-T"));
    assert_ssh_non_interactive_defaults(&spec.args);
    assert_arg_pair(&spec.args, "-o", "ConnectTimeout=12");
    assert_arg_pair(&spec.args, "-o", "ControlMaster=auto");
    assert_arg_pair(&spec.args, "-o", "ControlPersist=10m");
    assert_arg_pair(&spec.args, "-o", "ControlPath=/tmp/nucl-ssh/%C");
    assert_arg_pair(&spec.args, "-J", "bastion");
    assert_arg_pair(&spec.args, "-F", "/tmp/ssh config");
    assert!(arg_index(&spec.args, "ConnectTimeout=12") < separator);
    assert!(arg_index(&spec.args, "ControlPath=/tmp/nucl-ssh/%C") < separator);
    assert!(arg_index(&spec.args, "bastion") < separator);
    assert!(arg_index(&spec.args, "/tmp/ssh config") < separator);
    assert_eq!(spec.args[separator + 1], OsString::from("devbox"));
    assert!(
        spec.args[separator + 2]
            .to_string_lossy()
            .contains("--protocol v5")
    );
}

#[test]
fn remote_workspace_identity_uses_wsl_distro() {
    let location = WorkspaceLocation::Wsl {
        original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
        distro: "Ubuntu".to_string(),
        linux_path: PathBuf::from("/home/me/project"),
    };

    let identity = remote_workspace_identity_for_location(&location).unwrap();

    assert_eq!(identity.kind, RemoteWorkspaceKind::Wsl);
    assert_eq!(identity.name, "Ubuntu");
}

#[test]
fn remote_workspace_identity_formats_ssh_target() {
    let location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com:2222/home/me/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: Some(2222),
        },
        path: PathBuf::from("/home/me/project"),
    };

    let identity = remote_workspace_identity_for_location(&location).unwrap();

    assert_eq!(identity.kind, RemoteWorkspaceKind::Ssh);
    assert_eq!(identity.name, "me@example.com:2222");
}

#[test]
fn remote_workspace_identity_formats_ssh_ipv6_target() {
    let location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@[2001:db8::1]:2222/home/me/project"),
        target: SshWorkspaceTarget {
            host: "2001:db8::1".to_string(),
            user: Some("me".to_string()),
            port: Some(2222),
        },
        path: PathBuf::from("/home/me/project"),
    };

    let identity = remote_workspace_identity_for_location(&location).unwrap();

    assert_eq!(identity.kind, RemoteWorkspaceKind::Ssh);
    assert_eq!(identity.name, "me@[2001:db8::1]:2222");
}

#[test]
fn ssh_display_host_brackets_ipv6_hosts() {
    assert_eq!(ssh_display_host("example.com"), "example.com");
    assert_eq!(ssh_display_host("2001:db8::1"), "[2001:db8::1]");
    assert_eq!(ssh_display_host("[2001:db8::1]"), "[2001:db8::1]");
}

#[test]
fn remote_service_command_for_wsl_uses_native_root() {
    let location = WorkspaceLocation::Wsl {
        original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
        distro: "Ubuntu".to_string(),
        linux_path: PathBuf::from("/home/me/project"),
    };

    let spec =
        remote_service_command_for_location(&location, "/remote/bin/nucleotide-remote").unwrap();

    assert_eq!(spec.program, OsString::from("wsl.exe"));
    assert_eq!(spec.args[3], OsString::from("/home/me/project"));
    assert_eq!(
        spec.args[5],
        OsString::from("/remote/bin/nucleotide-remote")
    );
    assert_eq!(spec.args[8], OsString::from("/home/me/project"));
    assert_arg_pair(&spec.args, "--protocol", "v5");
}

#[test]
fn remote_lsp_proxy_command_for_wsl_uses_native_root() {
    let location = WorkspaceLocation::Wsl {
        original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
        distro: "Ubuntu".to_string(),
        linux_path: PathBuf::from("/home/me/project"),
    };

    let spec = remote_lsp_proxy_command_for_location(
        &location,
        "/remote/bin/nucleotide-remote",
        "rust-analyzer",
    )
    .unwrap();

    assert_eq!(spec.program, OsString::from("wsl.exe"));
    assert_eq!(spec.args[3], OsString::from("/home/me/project"));
    assert_eq!(
        spec.args[5],
        OsString::from("/remote/bin/nucleotide-remote")
    );
    assert_eq!(spec.args[6], OsString::from("lsp-proxy"));
    assert_eq!(spec.args[8], OsString::from("/home/me/project"));
    assert_eq!(spec.args[10], OsString::from("rust-analyzer"));
}

#[test]
fn remote_service_command_for_ssh_uses_target_and_native_root() {
    let location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com:2222/home/me/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: Some(2222),
        },
        path: PathBuf::from("/home/me/project"),
    };

    let spec =
        remote_service_command_for_location(&location, "/remote/bin/nucleotide-remote").unwrap();

    assert_eq!(spec.program, OsString::from("ssh"));
    assert_eq!(spec.args[0], OsString::from("-T"));
    assert_ssh_non_interactive_defaults(&spec.args);
    assert_arg_pair(&spec.args, "-p", "2222");
    let separator = ssh_target_separator_index(&spec.args);
    assert_eq!(spec.args[separator + 1], OsString::from("me@example.com"));
    let command = spec.args[separator + 2].to_string_lossy();
    assert!(command.contains("/remote/bin/nucleotide-remote"));
    assert!(command.contains("/home/me/project"));
    assert!(command.contains("--protocol v5"));
}

#[test]
fn remote_service_command_with_options_applies_ssh_settings() {
    let location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: None,
        },
        path: PathBuf::from("/home/me/project"),
    };
    let options = RemoteWorkspaceBackendOptions {
        ssh_connect_timeout_secs: Some(4),
        ssh_control_path: None,
        ssh_extra_args: vec![OsString::from("-J"), OsString::from("bastion")],
        ..RemoteWorkspaceBackendOptions::default()
    };

    let spec = remote_service_command_for_location_with_options(
        &location,
        "/remote/bin/nucleotide-remote",
        &options,
    )
    .unwrap();

    let separator = ssh_target_separator_index(&spec.args);
    assert_eq!(spec.args[0], OsString::from("-T"));
    assert_ssh_non_interactive_defaults(&spec.args);
    assert_arg_pair(&spec.args, "-o", "ConnectTimeout=4");
    assert_arg_pair(&spec.args, "-J", "bastion");
    assert!(arg_index(&spec.args, "ConnectTimeout=4") < separator);
    assert!(arg_index(&spec.args, "bastion") < separator);
    assert_eq!(spec.args[separator + 1], OsString::from("me@example.com"));
    assert!(
        spec.args[separator + 2]
            .to_string_lossy()
            .contains("--protocol v5")
    );
}

#[test]
fn remote_lsp_proxy_command_for_ssh_uses_target_and_native_root() {
    let location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com:2222/home/me/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: Some(2222),
        },
        path: PathBuf::from("/home/me/project"),
    };

    let spec = remote_lsp_proxy_command_for_location(
        &location,
        "/remote/bin/nucleotide-remote",
        "rust-analyzer",
    )
    .unwrap();

    assert_eq!(spec.program, OsString::from("ssh"));
    assert_eq!(spec.args[0], OsString::from("-T"));
    assert_ssh_non_interactive_defaults(&spec.args);
    assert_arg_pair(&spec.args, "-p", "2222");
    let separator = ssh_target_separator_index(&spec.args);
    assert_eq!(spec.args[separator + 1], OsString::from("me@example.com"));
    let command = spec.args[separator + 2].to_string_lossy();
    assert!(command.contains("/remote/bin/nucleotide-remote"));
    assert!(command.contains("lsp-proxy"));
    assert!(command.contains("/home/me/project"));
    assert!(command.contains("rust-analyzer"));
}

#[test]
fn ssh_startup_protocol_error_allows_helper_reinstall_retry() {
    let location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: None,
        },
        path: PathBuf::from("/home/me/project"),
    };
    let error = anyhow::anyhow!(
        "failed to connect to v5 remote workspace service after starting ssh helper; verify the helper speaks protocol v5"
    );

    assert!(remote_startup_error_can_retry_helper_install(
        &location, &error
    ));
}

#[test]
fn startup_retry_is_limited_to_remote_linux_helper_failures() {
    let ssh_location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: None,
        },
        path: PathBuf::from("/home/me/project"),
    };
    let local_location = WorkspaceLocation::Local {
        path: PathBuf::from("/home/me/project"),
    };
    let wsl_location = WorkspaceLocation::Wsl {
        original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
        distro: "Ubuntu".to_string(),
        linux_path: PathBuf::from("/home/me/project"),
    };
    let auth_error = anyhow::anyhow!("Permission denied (publickey)");
    let protocol_error = anyhow::anyhow!("invalid frame magic; expected NUC2");

    assert!(!remote_startup_error_can_retry_helper_install(
        &ssh_location,
        &auth_error
    ));
    assert!(!remote_startup_error_can_retry_helper_install(
        &local_location,
        &protocol_error
    ));
    assert!(remote_startup_error_can_retry_helper_install(
        &wsl_location,
        &protocol_error
    ));
}

#[test]
fn workspace_backend_factory_keeps_local_backend_in_process_by_default() {
    let temp = tempfile::tempdir().unwrap();
    let location = WorkspaceLocation::Local {
        path: temp.path().to_path_buf(),
    };

    let connection =
        connect_workspace_backend_for_location(location, &RemoteWorkspaceBackendOptions::default())
            .unwrap();

    assert_eq!(connection.backend.identity(), WorkspaceIdentity::Local);
    assert_eq!(connection.hello, None);
}

#[test]
fn backend_options_discover_bundled_local_helper() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("nucl");
    let helper = temp.path().join(local_helper_binary_name());
    std::fs::write(&executable, "").unwrap();
    std::fs::write(&helper, "").unwrap();

    let options =
        RemoteWorkspaceBackendOptions::from_environment_values(RemoteWorkspaceBackendEnvironment {
            use_local_service: true,
            current_exe: Some(executable),
            ssh_control_master: Some("false".to_string()),
            ..RemoteWorkspaceBackendEnvironment::default()
        });

    assert_eq!(options.local_helper_path.as_deref(), Some(helper.as_path()));
    assert!(options.use_local_service);
}

#[test]
fn backend_options_prefer_local_helper_env_over_bundled_helper() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("nucl");
    let bundled_helper = temp.path().join(local_helper_binary_name());
    let env_helper = temp.path().join("custom-helper");
    std::fs::write(&executable, "").unwrap();
    std::fs::write(&bundled_helper, "").unwrap();

    let options =
        RemoteWorkspaceBackendOptions::from_environment_values(RemoteWorkspaceBackendEnvironment {
            local_helper_path: Some(env_helper.clone().into_os_string()),
            use_local_service: true,
            current_exe: Some(executable),
            ssh_control_master: Some("false".to_string()),
            ..RemoteWorkspaceBackendEnvironment::default()
        });

    assert_eq!(
        options.local_helper_path.as_deref(),
        Some(env_helper.as_path())
    );
}

#[test]
fn backend_options_discover_ssh_helper_upload_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("nucl");
    let upload_helper = temp.path().join("nucleotide-remote-linux-x86_64");
    std::fs::write(&executable, "").unwrap();

    let options =
        RemoteWorkspaceBackendOptions::from_environment_values(RemoteWorkspaceBackendEnvironment {
            ssh_helper_upload_path: Some(upload_helper.clone().into_os_string()),
            ssh_helper_install_policy: Some("upload".to_string()),
            current_exe: Some(executable),
            ssh_control_master: Some("false".to_string()),
            ..RemoteWorkspaceBackendEnvironment::default()
        });

    assert_eq!(
        options.ssh_helper_upload_path.as_deref(),
        Some(upload_helper.as_path())
    );
    assert_eq!(
        options.ssh_helper_install_policy,
        RemoteHelperInstallPolicy::Upload
    );
    assert_eq!(
        options.wsl_helper_install_policy,
        RemoteHelperInstallPolicy::Upload
    );
}

#[test]
fn generic_remote_helper_environment_overrides_ssh_and_wsl() {
    let helper = PathBuf::from("/opt/nucleotide/nucleotide-remote");
    let options =
        RemoteWorkspaceBackendOptions::from_environment_values(RemoteWorkspaceBackendEnvironment {
            remote_helper_path: Some(helper.clone().into_os_string()),
            ssh_helper_install_policy: Some("never".to_string()),
            ssh_control_master: Some("false".to_string()),
            ..RemoteWorkspaceBackendEnvironment::default()
        });

    assert_eq!(options.remote_helper_path, helper);
    assert_eq!(options.ssh_helper_path.as_deref(), Some(helper.as_path()));
    assert_eq!(options.wsl_helper_path.as_deref(), Some(helper.as_path()));
    assert!(options.remote_helper_path_is_override);
    assert!(options.ssh_helper_path_is_override);
    assert!(options.wsl_helper_path_is_override);
    assert_eq!(
        options.ssh_helper_install_policy,
        RemoteHelperInstallPolicy::Never
    );
    assert_eq!(
        options.wsl_helper_install_policy,
        RemoteHelperInstallPolicy::Never
    );
}

#[test]
fn backend_options_parse_ssh_connection_environment_values() {
    let control_path = PathBuf::from("/tmp/nucl-control/%C");

    let options =
        RemoteWorkspaceBackendOptions::from_environment_values(RemoteWorkspaceBackendEnvironment {
            ssh_connect_timeout_secs: Some("9".to_string()),
            ssh_extra_args: Some(OsString::from("-J bastion -F '/tmp/ssh config'")),
            ssh_control_master: Some("true".to_string()),
            ssh_control_path: Some(control_path.clone().into_os_string()),
            ssh_helper_download_base_url: Some("https://mirror.example/releases/v1".to_string()),
            ..RemoteWorkspaceBackendEnvironment::default()
        });

    assert_eq!(options.ssh_connect_timeout_secs, Some(9));
    assert_eq!(
        options.ssh_extra_args,
        [
            OsString::from("-J"),
            OsString::from("bastion"),
            OsString::from("-F"),
            OsString::from("/tmp/ssh config"),
        ]
    );
    assert_eq!(
        options.ssh_control_path.as_deref(),
        Some(control_path.as_path())
    );
    assert_eq!(
        options.ssh_helper_download_base_url.as_deref(),
        Some("https://mirror.example/releases/v1")
    );
}

#[test]
fn default_ssh_control_path_leaves_room_for_openssh_suffix() {
    let Some(control_path) = default_ssh_control_path() else {
        return;
    };

    let expanded_hash = "0123456789abcdef0123456789abcdef01234567";
    let openssh_bind_suffix = ".abcdefghijklmnop";
    let expanded_path = control_path
        .display()
        .to_string()
        .replace("%C", expanded_hash);
    let bind_path = format!("{expanded_path}{openssh_bind_suffix}");

    assert!(
        bind_path.len() < 104,
        "OpenSSH ControlPath is too long for macOS Unix sockets: {bind_path}"
    );
}

#[test]
fn backend_options_discover_platform_named_ssh_helper_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("nucl");
    let artifact = temp.path().join("nucleotide-remote-linux-x86_64");
    std::fs::write(&executable, "").unwrap();
    std::fs::write(&artifact, "").unwrap();

    let options =
        RemoteWorkspaceBackendOptions::from_environment_values(RemoteWorkspaceBackendEnvironment {
            current_exe: Some(executable),
            ssh_control_master: Some("false".to_string()),
            ..RemoteWorkspaceBackendEnvironment::default()
        });
    let platform = SshRemotePlatform {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
    };

    assert_eq!(
        RemoteHelperManager::new(&options).local_upload_artifact_for_platform(&platform),
        Some(artifact)
    );
}

#[test]
fn helper_version_command_writes_json_probe_payload() {
    let mut output = Vec::new();

    print_version(["--json".to_string()], &mut output).unwrap();

    let info: HelperVersionInfo = serde_json::from_slice(&output).unwrap();
    assert_eq!(info.helper_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(info.helper_revision, REMOTE_HELPER_REVISION);
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert_eq!(info.frame_version, FRAME_VERSION);
    assert_eq!(info.os, std::env::consts::OS);
    assert_eq!(info.arch, std::env::consts::ARCH);
}

#[test]
fn linux_probe_parser_accepts_shell_noise_and_platform_markers() {
    let probe = parse_linux_probe_output(
        "profile says hi\nNUCL_PLATFORM Linux aarch64\nNUCL_CACHE /home/me/.cache\n",
    )
    .unwrap();

    assert_eq!(
        probe.platform,
        SshRemotePlatform {
            os: "linux".to_string(),
            arch: "aarch64".to_string(),
        }
    );
    assert_eq!(probe.cache_root, "/home/me/.cache");
}

#[test]
fn linux_helper_cache_path_includes_protocol_version_and_platform() {
    let probe = SshRemoteProbe {
        platform: SshRemotePlatform {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
        },
        cache_root: "/home/me/.cache".to_string(),
    };

    assert_eq!(
        remote_linux_helper_path(&probe),
        PathBuf::from(format!(
            "/home/me/.cache/nucleotide/remote/protocol-{PROTOCOL_VERSION}/revision-{REMOTE_HELPER_REVISION}/nucleotide-remote-{}-linux-x86_64",
            env!("CARGO_PKG_VERSION")
        ))
    );
}

#[test]
fn helper_version_match_checks_protocol_version_and_platform() {
    let platform = SshRemotePlatform {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
    };
    let mut info = HelperVersionInfo {
        helper_version: env!("CARGO_PKG_VERSION").to_string(),
        helper_revision: REMOTE_HELPER_REVISION,
        protocol_version: PROTOCOL_VERSION,
        frame_version: FRAME_VERSION,
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
    };

    assert!(helper_version_matches_current(&info, &platform));

    info.helper_revision += 1;
    assert!(!helper_version_matches_current(&info, &platform));
    info.helper_revision = REMOTE_HELPER_REVISION;

    info.protocol_version += 1;
    assert!(!helper_version_matches_current(&info, &platform));
}

#[test]
fn helper_version_without_revision_is_treated_as_stale() {
    let info: HelperVersionInfo = serde_json::from_value(serde_json::json!({
        "helper_version": env!("CARGO_PKG_VERSION"),
        "protocol_version": PROTOCOL_VERSION,
        "frame_version": FRAME_VERSION,
        "os": "linux",
        "arch": "x86_64"
    }))
    .unwrap();
    let platform = SshRemotePlatform {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
    };

    assert_eq!(info.helper_revision, 0);
    assert!(!helper_version_matches_current(&info, &platform));
}

#[test]
fn remote_deployment_progress_formats_status_message() {
    let progress = RemoteDeploymentProgress {
        phase: RemoteDeploymentPhase::InstallingRemoteHelper,
        target: Some("me@example.com".to_string()),
        detail: Some("download nucleotide-remote-linux-x86_64".to_string()),
    };

    assert_eq!(
        progress.message(),
        "Installing nucleotide-remote: me@example.com (download nucleotide-remote-linux-x86_64)"
    );
}

#[test]
fn wsl_deployment_progress_names_distribution_startup() {
    let progress = RemoteDeploymentProgress {
        phase: RemoteDeploymentPhase::StartingWslDistro,
        target: Some("Ubuntu".to_string()),
        detail: None,
    };

    assert_eq!(progress.message(), "Starting WSL distribution: Ubuntu");
}

#[test]
fn remote_helper_download_urls_use_release_assets_and_checksums() {
    let options = RemoteWorkspaceBackendOptions {
        ssh_helper_download_base_url: Some("https://downloads.example/nucleotide/v1/".to_string()),
        ..RemoteWorkspaceBackendOptions::default()
    };
    let manager = RemoteHelperManager::new(&options);
    let platform = SshRemotePlatform {
        os: "linux".to_string(),
        arch: "aarch64".to_string(),
    };

    let (asset_url, checksums_url) = manager.remote_helper_download_urls(&platform).unwrap();

    assert_eq!(
        asset_url,
        "https://downloads.example/nucleotide/v1/nucleotide-remote-linux-aarch64"
    );
    assert_eq!(
        checksums_url,
        "https://downloads.example/nucleotide/v1/SHA256SUMS"
    );
}

#[test]
fn remote_helper_upload_command_registers_temporary_file_cleanup() {
    let expected_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let command = remote_helper_upload_command(
        "/home/me/.cache/nucleotide",
        "/home/me/.cache/nucleotide/helper tmp",
        "/home/me/.cache/nucleotide/helper",
        expected_sha256,
    );

    assert!(command.starts_with("sh -lc "));
    assert!(command.contains("sha256sum"));
    assert!(command.contains("shasum -a 256"));
    assert!(command.contains("checksum mismatch for uploaded helper"));
    assert!(command.contains("cleanup() { rm -f \"$tmp\"; }"));
    assert!(command.contains("trap cleanup EXIT"));
    assert!(command.contains("trap \"exit 1\" INT TERM HUP"));
    assert!(command.contains("cat > \"$tmp\""));
    assert!(command.contains("mv -f \"$tmp\" \"$final\""));
    assert!(command.contains("'/home/me/.cache/nucleotide/helper tmp'"));
    assert!(command.contains(expected_sha256));
}

#[cfg(unix)]
#[test]
fn remote_helper_upload_command_rejects_truncated_input_without_replacing_helper() {
    let temp = tempfile::tempdir().unwrap();
    let helper_dir = temp.path().join("helper dir");
    let tmp_path = helper_dir.join("helper tmp");
    let helper_path = helper_dir.join("helper");
    let input_path = temp.path().join("input helper");
    let complete_helper = b"complete helper bytes that must arrive intact";
    let expected_sha256 = sha256_reader(&mut complete_helper.as_slice()).unwrap();
    std::fs::create_dir_all(&helper_dir).unwrap();
    std::fs::write(&helper_path, b"existing working helper").unwrap();
    std::fs::write(&input_path, b"partial helper bytes").unwrap();
    let command = remote_helper_upload_command(
        helper_dir.to_str().unwrap(),
        tmp_path.to_str().unwrap(),
        helper_path.to_str().unwrap(),
        &expected_sha256,
    );

    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &command])
        .stdin(Stdio::from(std::fs::File::open(input_path).unwrap()))
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("checksum mismatch"));
    assert!(!tmp_path.exists(), "failed upload left its temporary file");
    assert_eq!(
        std::fs::read(&helper_path).unwrap(),
        b"existing working helper"
    );
}

#[cfg(unix)]
#[test]
fn remote_helper_upload_command_installs_complete_verified_input() {
    let temp = tempfile::tempdir().unwrap();
    let helper_dir = temp.path().join("helper dir");
    let tmp_path = helper_dir.join("helper tmp");
    let helper_path = helper_dir.join("helper");
    let input_path = temp.path().join("input helper");
    let helper = b"complete verified helper bytes";
    let expected_sha256 = sha256_reader(&mut helper.as_slice()).unwrap();
    std::fs::write(&input_path, helper).unwrap();
    let command = remote_helper_upload_command(
        helper_dir.to_str().unwrap(),
        tmp_path.to_str().unwrap(),
        helper_path.to_str().unwrap(),
        &expected_sha256,
    );

    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &command])
        .stdin(Stdio::from(std::fs::File::open(input_path).unwrap()))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "verified upload failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!tmp_path.exists());
    assert_eq!(std::fs::read(&helper_path).unwrap(), helper);
}

#[test]
fn remote_helper_download_command_verifies_checksum_before_install() {
    let command = remote_helper_download_command(
        "/home/me/.cache/nucleotide/remote",
        "/home/me/.cache/nucleotide/remote/helper tmp",
        "/home/me/.cache/nucleotide/remote/helper",
        "https://downloads.example/nucleotide-remote-linux-x86_64",
        "https://downloads.example/SHA256SUMS",
        "nucleotide-remote-linux-x86_64",
    );

    assert!(command.starts_with("sh -lc "));
    assert!(command.contains("curl -fsSL"));
    assert!(command.contains("wget -qO"));
    assert!(command.contains("sha256sum"));
    assert!(command.contains("shasum -a 256"));
    assert!(command.contains("checksum mismatch"));
    assert!(command.contains("trap cleanup EXIT"));
    assert!(command.contains("trap \"exit 1\" INT TERM HUP"));
    assert!(command.contains("mv -f"));
    assert!(command.contains("'/home/me/.cache/nucleotide/remote/helper tmp'"));
    assert!(command.contains("nucleotide-remote-linux-x86_64"));
    assert!(command.contains("SHA256SUMS"));
}

#[test]
fn remote_helper_hints_name_transport_and_env_var() {
    let wsl_location = WorkspaceLocation::Wsl {
        original_path: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project"),
        distro: "Ubuntu".to_string(),
        linux_path: PathBuf::from("/home/me/project"),
    };
    let ssh_location = WorkspaceLocation::Ssh {
        original_path: PathBuf::from("ssh://me@example.com/home/me/project"),
        target: SshWorkspaceTarget {
            host: "example.com".to_string(),
            user: Some("me".to_string()),
            port: None,
        },
        path: PathBuf::from("/home/me/project"),
    };

    let wsl_hint = remote_helper_setup_hint(&wsl_location, Path::new("/remote/nucl"));
    let ssh_hint = remote_helper_setup_hint(&ssh_location, Path::new("/remote/nucl"));

    assert!(wsl_hint.contains("WSL distro Ubuntu"));
    assert!(wsl_hint.contains("NUCLEOTIDE_REMOTE_HELPER"));
    assert!(ssh_hint.contains("SSH target me@example.com"));
    assert!(ssh_hint.contains("NUCLEOTIDE_REMOTE_HELPER"));
}
