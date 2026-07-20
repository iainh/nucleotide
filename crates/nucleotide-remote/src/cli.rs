// ABOUTME: nucleotide-remote command-line parsing and proxy entry points
// ABOUTME: Dispatches service, LSP proxy, and terminal proxy modes

use super::*;

pub fn run_from_args<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_else(|| "help".to_string());

    match command.as_str() {
        "serve" => {
            let options = parse_serve_options(args)?;
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            serve_local_workspace_v5(options.workspace_root, stdin, stdout)
        }
        "lsp-proxy" => {
            let options = parse_lsp_proxy_options(args)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to create remote LSP proxy runtime")?;
            runtime.block_on(run_lsp_proxy(options))
        }
        "terminal-proxy" => {
            let options = parse_terminal_proxy_options(args)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to create remote terminal proxy runtime")?;
            runtime.block_on(run_terminal_proxy(options))
        }
        "version" => print_version(args, &mut std::io::stdout()).context("failed to write version"),
        "--help" | "-h" | "help" => {
            print_help(&mut std::io::stdout()).context("failed to write help")
        }
        other => bail!("unknown nucleotide-remote command: {other}"),
    }
}

pub(crate) fn print_version<I, W>(args: I, writer: &mut W) -> io::Result<()>
where
    I: IntoIterator<Item = String>,
    W: Write,
{
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--help" | "-h" => {
                writeln!(writer, "nucleotide-remote version [--json]")?;
                return Ok(());
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown version argument: {other}"),
                ));
            }
        }
    }

    let info = HelperVersionInfo::current();
    if json {
        serde_json::to_writer(&mut *writer, &info).map_err(io::Error::other)?;
        writeln!(writer)
    } else {
        writeln!(
            writer,
            "nucleotide-remote {} revision {} protocol {} frame {} {}-{}",
            info.helper_version,
            info.helper_revision,
            info.protocol_version,
            info.frame_version,
            info.os,
            info.arch
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServeOptions {
    pub(crate) workspace_root: PathBuf,
}

pub(crate) fn parse_serve_options<I>(args: I) -> Result<ServeOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut workspace_root = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let path = args
                    .next()
                    .context("--workspace requires a remote workspace path")?;
                let path = PathBuf::from(path);
                workspace_root = Some(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .context("failed to resolve current directory")?
                        .join(path)
                });
            }
            "--protocol" => {
                let value = args.next().context("--protocol requires v5")?;
                if !matches!(value.as_str(), "5" | "v5" | "V5") {
                    bail!("unsupported serve protocol: {value}");
                }
            }
            other => bail!("unknown serve argument: {other}"),
        }
    }

    let workspace_root = workspace_root
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .context("failed to resolve workspace root")?;
    Ok(ServeOptions { workspace_root })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LspProxyOptions {
    pub(crate) workspace_root: PathBuf,
    pub(crate) server: String,
    pub(crate) server_args: Vec<String>,
}

pub(crate) fn parse_lsp_proxy_options<I>(args: I) -> Result<LspProxyOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut workspace_root = None;
    let mut server = None;
    let mut server_args = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let path = args
                    .next()
                    .context("--workspace requires a remote workspace path")?;
                let path = PathBuf::from(path);
                workspace_root = Some(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .context("failed to resolve current directory")?
                        .join(path)
                });
            }
            "--server" => {
                server = Some(args.next().context("--server requires a language server")?);
            }
            "--server-arg" => {
                server_args.push(
                    args.next()
                        .context("--server-arg requires a language server argument")?,
                );
            }
            "--" => {
                server_args.extend(args);
                break;
            }
            other if server.is_none() => {
                server = Some(other.to_string());
            }
            other => {
                server_args.push(other.to_string());
            }
        }
    }

    Ok(LspProxyOptions {
        workspace_root: workspace_root
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .context("failed to resolve workspace root")?,
        server: server.context("lsp-proxy requires --server <language-server>")?,
        server_args,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalProxyOptions {
    pub(crate) workspace_root: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) shell: Option<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) command: Option<(String, Vec<String>)>,
}

pub(crate) fn parse_terminal_proxy_options<I>(args: I) -> Result<TerminalProxyOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let mut workspace_root = None;
    let mut cwd = None;
    let mut shell = None;
    let mut env = Vec::new();
    let mut command = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                let path = args
                    .next()
                    .context("--workspace requires a remote workspace path")?;
                let path = PathBuf::from(path);
                workspace_root = Some(if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .context("failed to resolve current directory")?
                        .join(path)
                });
            }
            "--cwd" => {
                cwd = Some(PathBuf::from(
                    args.next()
                        .context("--cwd requires a command working directory")?,
                ));
            }
            "--shell" => {
                shell = Some(args.next().context("--shell requires a shell path")?);
            }
            "--env" => {
                let entry = args.next().context("--env requires KEY=VALUE")?;
                let (key, value) = entry
                    .split_once('=')
                    .with_context(|| format!("terminal env entry must be KEY=VALUE: {entry}"))?;
                if !terminal_env_entry_is_valid(key, value) {
                    bail!("terminal env entry is invalid: {key}");
                }
                env.push((key.to_string(), value.to_string()));
            }
            "--" => {
                if let Some(program) = args.next() {
                    command = Some((program, args.collect()));
                }
                break;
            }
            other => bail!("unknown terminal-proxy argument: {other}"),
        }
    }

    let workspace_root = workspace_root
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .context("failed to resolve workspace root")?;
    let cwd = cwd
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace_root.join(path)
            }
        })
        .unwrap_or_else(|| workspace_root.clone());

    Ok(TerminalProxyOptions {
        workspace_root,
        cwd,
        shell,
        env,
        command,
    })
}

pub(crate) fn print_help<W: Write>(writer: &mut W) -> io::Result<()> {
    writeln!(writer, "nucleotide-remote serve [--workspace <path>]")?;
    writeln!(writer, "nucleotide-remote version [--json]")?;
    writeln!(
        writer,
        "nucleotide-remote lsp-proxy [--workspace <path>] --server <name> [-- <args>...]"
    )?;
    writeln!(
        writer,
        "nucleotide-remote terminal-proxy [--workspace <path>] [--cwd <path>] [--shell <path>] [--env KEY=VALUE]... [-- <command> <args>...]"
    )?;
    writeln!(writer)?;
    writeln!(
        writer,
        "Protocol traffic uses framed messages on stdin/stdout."
    )?;
    writeln!(
        writer,
        "Proxy diagnostics are written to stderr so protocol and terminal streams stay clean."
    )
}
