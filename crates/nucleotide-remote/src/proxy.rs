// ABOUTME: LSP and terminal proxy process execution for nucleotide-remote
// ABOUTME: Loads project environments and keeps protocol streams free of diagnostics

use super::*;

pub(crate) async fn run_lsp_proxy(options: LspProxyOptions) -> Result<()> {
    let environment = load_lsp_proxy_environment(&options.workspace_root).await?;
    let server_program = resolve_program_from_environment_path(
        &options.server,
        &environment,
        &options.workspace_root,
    );

    let mut child = nucleotide_process::tokio_command(&server_program)
        .args(&options.server_args)
        .current_dir(&options.workspace_root)
        .env_clear()
        .envs(&environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn language server {} in {}",
                server_program.display(),
                options.workspace_root.display()
            )
        })?;

    let mut server_stdin = child
        .stdin
        .take()
        .context("language server child did not expose stdin")?;
    let mut server_stdout = child
        .stdout
        .take()
        .context("language server child did not expose stdout")?;
    let mut client_stdin = tokio::io::stdin();
    let mut client_stdout = tokio::io::stdout();

    let mut stdin_task = tokio::spawn(async move {
        let copied = tokio::io::copy(&mut client_stdin, &mut server_stdin).await;
        let _ = server_stdin.shutdown().await;
        copied
    });
    let mut stdout_task =
        tokio::spawn(async move { tokio::io::copy(&mut server_stdout, &mut client_stdout).await });

    let status = tokio::select! {
        result = &mut stdin_task => {
            pipe_task_result(result, "copy LSP client stdin to server")?;
            child.wait().await.context("failed waiting for language server after stdin closed")?
        }
        result = &mut stdout_task => {
            pipe_task_result(result, "copy language server stdout to client")?;
            child.wait().await.context("failed waiting for language server after stdout closed")?
        }
        status = child.wait() => {
            status.context("failed waiting for language server")?
        }
    };

    stdin_task.abort();
    stdout_task.abort();

    if status.success() {
        Ok(())
    } else {
        bail!("language server exited with status {status}")
    }
}

pub(crate) async fn run_terminal_proxy(options: TerminalProxyOptions) -> Result<()> {
    let mut environment = load_proxy_environment("terminal-proxy", &options.workspace_root).await?;
    environment.extend(options.env.iter().cloned());
    remove_interactive_shell_state(&mut environment);

    let process = terminal_proxy_process(&options, &environment);
    let program_path = resolve_program_from_environment_path(
        &process.program,
        &environment,
        &options.workspace_root,
    );

    exec_terminal_proxy_process(
        &program_path,
        &process.args,
        process.login_shell,
        &environment,
        &options.workspace_root,
    )
    .with_context(|| {
        format!(
            "failed to run terminal command {} in {}",
            program_path.display(),
            options.workspace_root.display()
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalProxyProcess {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) login_shell: bool,
}

pub(crate) fn terminal_proxy_process(
    options: &TerminalProxyOptions,
    environment: &HashMap<String, String>,
) -> TerminalProxyProcess {
    match &options.command {
        Some((program, args)) => TerminalProxyProcess {
            program: program.clone(),
            args: args.clone(),
            login_shell: false,
        },
        None => {
            let shell = options
                .shell
                .as_deref()
                .filter(|shell| !shell.is_empty())
                .or_else(|| environment.get("SHELL").map(String::as_str))
                .filter(|shell| !shell.is_empty())
                .unwrap_or("/bin/sh")
                .to_string();
            TerminalProxyProcess {
                program: shell,
                args: Vec::new(),
                login_shell: true,
            }
        }
    }
}

pub(crate) const INTERACTIVE_SHELL_STATE_ENV_VARS: &[&str] = &[
    "BASH_ENV",
    "BASHOPTS",
    "ENV",
    "POSIXLY_CORRECT",
    "PROMPT_COMMAND",
    "PS1",
    "SHELLOPTS",
];

pub(crate) fn remove_interactive_shell_state(environment: &mut HashMap<String, String>) {
    for key in INTERACTIVE_SHELL_STATE_ENV_VARS {
        environment.remove(*key);
    }
}

#[cfg(unix)]
pub(crate) fn exec_terminal_proxy_process(
    program: &Path,
    args: &[String],
    login_shell: bool,
    environment: &HashMap<String, String>,
    workspace_root: &Path,
) -> io::Result<()> {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(program);
    if login_shell {
        command.arg0(login_shell_arg0(program));
    }
    let error = command
        .args(args)
        .current_dir(workspace_root)
        .env_clear()
        .envs(environment)
        .exec();
    Err(error)
}

#[cfg(not(unix))]
pub(crate) fn exec_terminal_proxy_process(
    program: &Path,
    args: &[String],
    _login_shell: bool,
    environment: &HashMap<String, String>,
    workspace_root: &Path,
) -> io::Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(workspace_root)
        .env_clear()
        .envs(environment)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "terminal command exited with status {status}"
        )))
    }
}

#[cfg(any(unix, test))]
pub(crate) fn login_shell_arg0(program: &Path) -> OsString {
    let name = program.file_name().unwrap_or(program.as_os_str());
    let mut arg0 = OsString::from("-");
    arg0.push(name);
    arg0
}

pub(crate) fn pipe_task_result(
    result: std::result::Result<std::io::Result<u64>, tokio::task::JoinError>,
    operation: &'static str,
) -> Result<u64> {
    result
        .with_context(|| format!("{operation} task panicked"))?
        .with_context(|| format!("{operation} failed"))
}

pub(crate) async fn load_lsp_proxy_environment(root: &Path) -> Result<HashMap<String, String>> {
    load_proxy_environment("lsp-proxy", root).await
}

pub(crate) async fn load_proxy_environment(
    label: &str,
    root: &Path,
) -> Result<HashMap<String, String>> {
    let project_environment = ProjectEnvironment::new(Some(std::env::vars().collect()));
    let environment = project_environment
        .get_environment_for_directory(root)
        .await
        .with_context(|| format!("failed to load project environment for {}", root.display()))?;

    for diagnostic in project_environment.get_environment_diagnostics(root).await {
        eprintln!("nucleotide-remote {label} environment diagnostic: {diagnostic}");
    }

    Ok(environment)
}

pub(crate) fn resolve_program_from_environment_path(
    program: &str,
    environment: &HashMap<String, String>,
    workspace_root: &Path,
) -> PathBuf {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return if program_path.is_absolute() {
            program_path.to_path_buf()
        } else {
            workspace_root.join(program_path)
        };
    }

    environment
        .get("PATH")
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|directory| {
            if directory.is_absolute() {
                directory.join(program)
            } else {
                workspace_root.join(directory).join(program)
            }
        })
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| program_path.to_path_buf())
}
