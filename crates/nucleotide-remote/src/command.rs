// ABOUTME: Local, WSL, and SSH command construction for remote workspace helpers
// ABOUTME: Keeps transport argument building and shell quoting in one module

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteServiceCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
}

impl RemoteServiceCommand {
    pub fn command(&self) -> Command {
        let program = self.resolved_program();
        let mut command = nucleotide_process::command(&program);
        command.args(&self.args);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        command
    }

    pub fn contained_command(&self) -> nucleotide_process::ContainedCommand {
        let program = self.resolved_program();
        let mut command = nucleotide_process::contained_command(&program);
        command.args(&self.args);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        command
    }

    pub fn resolved_program(&self) -> OsString {
        resolve_service_program(&self.program)
    }

    pub fn display_invocation(&self) -> String {
        std::iter::once(self.program.as_os_str())
            .chain(self.args.iter().map(OsString::as_os_str))
            .map(quote_command_display_arg)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn display_context(&self) -> String {
        match &self.current_dir {
            Some(current_dir) => format!(
                "{} (cwd {})",
                self.display_invocation(),
                quote_command_display_arg(current_dir.as_os_str())
            ),
            None => self.display_invocation(),
        }
    }
}

pub(crate) fn resolve_service_program(program: &OsStr) -> OsString {
    #[cfg(windows)]
    {
        if let Some(path) = resolve_windows_program(program) {
            return path.into_os_string();
        }
    }

    program.to_os_string()
}

#[cfg(windows)]
pub(crate) fn resolve_windows_program(program: &OsStr) -> Option<PathBuf> {
    let program_text = program.to_string_lossy();
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.is_file().then(|| program_path.to_path_buf());
    }

    resolve_windows_program_from_path(&program_text).or_else(|| {
        let windir = std::env::var_os("WINDIR")?;
        let system32 = PathBuf::from(windir).join("System32");
        if program_text.eq_ignore_ascii_case("ssh") || program_text.eq_ignore_ascii_case("ssh.exe")
        {
            let ssh = system32.join("OpenSSH").join("ssh.exe");
            return ssh.is_file().then_some(ssh);
        }
        if program_text.eq_ignore_ascii_case("wsl") || program_text.eq_ignore_ascii_case("wsl.exe")
        {
            let wsl = system32.join("wsl.exe");
            return wsl.is_file().then_some(wsl);
        }
        None
    })
}

#[cfg(windows)]
pub(crate) fn resolve_windows_program_from_path(program: &str) -> Option<PathBuf> {
    let path_exts = std::env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()]);
    let candidates = if Path::new(program).extension().is_some() {
        vec![program.to_string()]
    } else {
        path_exts
            .iter()
            .map(|ext| format!("{program}{ext}"))
            .chain(std::iter::once(program.to_string()))
            .collect()
    };

    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|directory| {
            candidates
                .iter()
                .map(|candidate| directory.join(candidate))
                .find(|candidate| candidate.is_file())
        })
    })
}

pub fn local_service_command(
    helper_path: impl AsRef<Path>,
    workspace_root: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let helper_path = helper_path.as_ref();
    let workspace_root = workspace_root.as_ref();
    let args = vec![
        OsString::from("serve"),
        OsString::from("--workspace"),
        workspace_root.as_os_str().to_os_string(),
        OsString::from("--protocol"),
        OsString::from("v5"),
    ];
    RemoteServiceCommand {
        program: helper_path.as_os_str().to_os_string(),
        args,
        current_dir: Some(workspace_root.to_path_buf()),
    }
}

pub fn wsl_service_command(
    distro: impl AsRef<OsStr>,
    linux_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let linux_root = linux_root.as_ref();
    let helper_path = helper_path.as_ref();
    let args = vec![
        OsString::from("--distribution"),
        distro.as_ref().to_os_string(),
        OsString::from("--cd"),
        linux_root.as_os_str().to_os_string(),
        OsString::from("--exec"),
        helper_path.as_os_str().to_os_string(),
        OsString::from("serve"),
        OsString::from("--workspace"),
        linux_root.as_os_str().to_os_string(),
        OsString::from("--protocol"),
        OsString::from("v5"),
    ];
    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args,
        current_dir: None,
    }
}

pub fn wsl_lsp_proxy_command(
    distro: impl AsRef<OsStr>,
    linux_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
    server: impl AsRef<OsStr>,
) -> RemoteServiceCommand {
    let linux_root = linux_root.as_ref();
    let helper_path = helper_path.as_ref();
    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args: vec![
            OsString::from("--distribution"),
            distro.as_ref().to_os_string(),
            OsString::from("--cd"),
            linux_root.as_os_str().to_os_string(),
            OsString::from("--exec"),
            helper_path.as_os_str().to_os_string(),
            OsString::from("lsp-proxy"),
            OsString::from("--workspace"),
            linux_root.as_os_str().to_os_string(),
            OsString::from("--server"),
            server.as_ref().to_os_string(),
            OsString::from("--"),
        ],
        current_dir: None,
    }
}

pub fn wsl_interactive_terminal_command(
    distro: impl AsRef<OsStr>,
    linux_root: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let linux_root = linux_root.as_ref();
    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args: vec![
            OsString::from("--distribution"),
            distro.as_ref().to_os_string(),
            OsString::from("--cd"),
            linux_root.as_os_str().to_os_string(),
        ],
        current_dir: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub connect_timeout_secs: Option<u64>,
    pub extra_args: Vec<OsString>,
    pub control_path: Option<PathBuf>,
}

impl SshTarget {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            user: None,
            port: None,
            connect_timeout_secs: None,
            extra_args: Vec::new(),
            control_path: None,
        }
    }

    pub(crate) fn target_arg(&self) -> String {
        match &self.user {
            Some(user) if !user.is_empty() => format!("{user}@{}", self.host),
            _ => self.host.clone(),
        }
    }
}

pub fn ssh_interactive_terminal_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let remote_root = posix_path_string(remote_root);
    let remote_command = format!(
        "cd {} && exec \"${{SHELL:-/bin/sh}}\" -l",
        quote_posix_shell(&remote_root)
    );
    let mut args = Vec::new();
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("-tt"));
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

pub fn ssh_service_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
) -> RemoteServiceCommand {
    let remote_root = posix_path_string(remote_root);
    let helper_path = posix_path_string(helper_path);
    let remote_command = format!(
        "exec {} serve --workspace {} --protocol v5",
        quote_posix_shell(&helper_path),
        quote_posix_shell(&remote_root)
    );
    let mut args = Vec::new();
    args.push(OsString::from("-T"));
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

pub fn ssh_lsp_proxy_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
    server: impl AsRef<OsStr>,
) -> RemoteServiceCommand {
    let remote_root = posix_path_string(remote_root);
    let helper_path = posix_path_string(helper_path);
    let server = server.as_ref().to_string_lossy();
    let remote_command = format!(
        "exec {} lsp-proxy --workspace {} --server {} --",
        quote_posix_shell(&helper_path),
        quote_posix_shell(&remote_root),
        quote_posix_shell(&server)
    );
    let mut args = Vec::new();
    args.push(OsString::from("-T"));
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

pub fn wsl_terminal_proxy_command(
    distro: impl AsRef<OsStr>,
    linux_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> RemoteServiceCommand {
    let linux_root = linux_root.as_ref();
    let helper_path = helper_path.as_ref();
    let mut args = vec![
        OsString::from("--distribution"),
        distro.as_ref().to_os_string(),
        OsString::from("--cd"),
        linux_root.as_os_str().to_os_string(),
        OsString::from("--exec"),
        helper_path.as_os_str().to_os_string(),
        OsString::from("terminal-proxy"),
        OsString::from("--workspace"),
        linux_root.as_os_str().to_os_string(),
    ];
    append_terminal_proxy_args(&mut args, shell, command, env);

    RemoteServiceCommand {
        program: OsString::from("wsl.exe"),
        args,
        current_dir: None,
    }
}

pub fn ssh_terminal_proxy_command(
    target: SshTarget,
    remote_root: impl AsRef<Path>,
    helper_path: impl AsRef<Path>,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> RemoteServiceCommand {
    let remote_command = terminal_proxy_shell_command(
        helper_path.as_ref(),
        remote_root.as_ref(),
        shell,
        command,
        env,
    );
    let mut args = Vec::new();
    append_ssh_connection_args(&mut args, &target);
    if let Some(port) = target.port {
        args.push(OsString::from("-p"));
        args.push(OsString::from(port.to_string()));
    }
    args.push(OsString::from("-tt"));
    args.push(OsString::from("--"));
    args.push(OsString::from(target.target_arg()));
    args.push(OsString::from(remote_command));

    RemoteServiceCommand {
        program: OsString::from("ssh"),
        args,
        current_dir: None,
    }
}

pub(crate) fn append_ssh_connection_args(args: &mut Vec<OsString>, target: &SshTarget) {
    args.push(OsString::from("-o"));
    args.push(OsString::from("BatchMode=yes"));
    args.push(OsString::from("-o"));
    args.push(OsString::from("NumberOfPasswordPrompts=0"));
    args.push(OsString::from("-o"));
    args.push(OsString::from("ConnectionAttempts=1"));
    args.push(OsString::from("-o"));
    args.push(OsString::from("StrictHostKeyChecking=accept-new"));
    args.push(OsString::from("-o"));
    args.push(OsString::from(format!(
        "ServerAliveInterval={DEFAULT_SSH_SERVER_ALIVE_INTERVAL_SECS}"
    )));
    args.push(OsString::from("-o"));
    args.push(OsString::from(format!(
        "ServerAliveCountMax={DEFAULT_SSH_SERVER_ALIVE_COUNT_MAX}"
    )));

    if let Some(timeout_secs) = target.connect_timeout_secs {
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!("ConnectTimeout={timeout_secs}")));
    }

    if let Some(control_path) = target.control_path.as_ref() {
        if let Some(parent) = control_path.parent() {
            let _ = std::fs::create_dir_all(parent);
            ensure_private_ssh_control_dir(parent);
        }

        args.push(OsString::from("-o"));
        args.push(OsString::from("ControlMaster=auto"));
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!(
            "ControlPersist={DEFAULT_SSH_CONTROL_PERSIST}"
        )));
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!(
            "ControlPath={}",
            control_path.display()
        )));
    }

    args.extend(target.extra_args.iter().cloned());
}

pub(crate) fn append_terminal_proxy_args(
    args: &mut Vec<OsString>,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) {
    if let Some(shell) = shell.filter(|shell| !shell.is_empty()) {
        args.push(OsString::from("--shell"));
        args.push(OsString::from(shell));
    }
    for (key, value) in env {
        if terminal_env_entry_is_valid(key, value) {
            args.push(OsString::from("--env"));
            args.push(OsString::from(format!("{key}={value}")));
        }
    }
    if let Some((program, program_args)) = command {
        args.push(OsString::from("--"));
        args.push(OsString::from(program));
        args.extend(program_args.iter().map(OsString::from));
    }
}

pub(crate) fn terminal_proxy_shell_command(
    helper_path: &Path,
    remote_root: &Path,
    shell: Option<&str>,
    command: Option<(&str, &[String])>,
    env: &[(String, String)],
) -> String {
    let helper_path = posix_path_string(helper_path);
    let remote_root = posix_path_string(remote_root);
    let mut parts = vec![
        "exec".to_string(),
        quote_posix_shell(&helper_path),
        "terminal-proxy".to_string(),
        "--workspace".to_string(),
        quote_posix_shell(&remote_root),
    ];
    if let Some(shell) = shell.filter(|shell| !shell.is_empty()) {
        parts.push("--shell".to_string());
        parts.push(quote_posix_shell(shell));
    }
    for (key, value) in env {
        if terminal_env_entry_is_valid(key, value) {
            parts.push("--env".to_string());
            parts.push(quote_posix_shell(&format!("{key}={value}")));
        }
    }
    if let Some((program, program_args)) = command {
        parts.push("--".to_string());
        parts.push(quote_posix_shell(program));
        parts.extend(program_args.iter().map(|arg| quote_posix_shell(arg)));
    }
    parts.join(" ")
}

pub(crate) fn terminal_env_entry_is_valid(key: &str, value: &str) -> bool {
    !key.is_empty() && !key.contains('=') && !key.contains('\0') && !value.contains('\0')
}
