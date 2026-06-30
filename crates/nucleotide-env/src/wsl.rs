// ABOUTME: WSL workspace detection and command construction helpers
// ABOUTME: Converts Windows WSL UNC paths into Linux paths for remote tooling

use std::path::Path;
use std::process::Command;

const WSL_LOCALHOST_PREFIX: &str = "wsl.localhost";
const WSL_LEGACY_PREFIX: &str = "wsl$";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslWorkspace {
    distro: String,
    linux_path: String,
}

impl WslWorkspace {
    pub fn from_unc_path(path: impl AsRef<Path>) -> Option<Self> {
        parse_wsl_unc_path(&path.as_ref().as_os_str().to_string_lossy())
    }

    pub fn distro(&self) -> &str {
        &self.distro
    }

    pub fn linux_path(&self) -> &str {
        &self.linux_path
    }

    pub fn to_unc_path(&self) -> String {
        let mut unc = format!(r"\\wsl.localhost\{}", self.distro);
        if self.linux_path != "/" {
            unc.push_str(&self.linux_path.replace('/', r"\"));
        }
        unc
    }
}

pub fn build_wsl_environment_capture_command(workspace: &WslWorkspace) -> Command {
    build_wsl_shell_command(workspace, "/bin/sh", "env -0")
}

pub fn build_wsl_remote_hello_command(workspace: &WslWorkspace) -> Command {
    let mut command = nucleotide_process::command("wsl.exe");
    add_wsl_base_args(&mut command, workspace);
    command.arg("nucleotide-remote").arg("hello");
    command
}

pub fn build_wsl_environment_capture_tokio_command(
    workspace: &WslWorkspace,
) -> tokio::process::Command {
    build_wsl_tokio_shell_command(workspace, "/bin/sh", "env -0")
}

pub fn build_wsl_shell_command(workspace: &WslWorkspace, shell: &str, script: &str) -> Command {
    let mut command = nucleotide_process::command("wsl.exe");
    add_wsl_shell_args(&mut command, workspace, shell, script);
    command
}

pub fn build_wsl_tokio_shell_command(
    workspace: &WslWorkspace,
    shell: &str,
    script: &str,
) -> tokio::process::Command {
    let mut command = nucleotide_process::tokio_command("wsl.exe");
    add_wsl_shell_args(&mut command, workspace, shell, script);
    command
}

fn add_wsl_shell_args<C>(command: &mut C, workspace: &WslWorkspace, shell: &str, script: &str)
where
    C: WslCommandArgs,
{
    add_wsl_base_args(command, workspace);
    command.push_arg(shell).push_arg("-lc").push_arg(script);
}

fn add_wsl_base_args<C>(command: &mut C, workspace: &WslWorkspace)
where
    C: WslCommandArgs,
{
    command
        .push_arg("--distribution")
        .push_arg(workspace.distro())
        .push_arg("--cd")
        .push_arg(workspace.linux_path())
        .push_arg("--");
}

trait WslCommandArgs {
    fn push_arg(&mut self, arg: &str) -> &mut Self;
}

impl WslCommandArgs for Command {
    fn push_arg(&mut self, arg: &str) -> &mut Self {
        self.arg(arg)
    }
}

impl WslCommandArgs for tokio::process::Command {
    fn push_arg(&mut self, arg: &str) -> &mut Self {
        self.arg(arg)
    }
}

fn parse_wsl_unc_path(path: &str) -> Option<WslWorkspace> {
    let trimmed = path.trim();
    let without_verbatim = trimmed
        .strip_prefix(r"\\?\UNC\")
        .map(|path| format!(r"\\{path}"));
    let normalized = without_verbatim.as_deref().unwrap_or(trimmed);

    let rest = normalized.strip_prefix(r"\\")?;
    let mut parts = rest.split(['\\', '/']).filter(|part| !part.is_empty());
    let server = parts.next()?;
    if !server.eq_ignore_ascii_case(WSL_LOCALHOST_PREFIX)
        && !server.eq_ignore_ascii_case(WSL_LEGACY_PREFIX)
    {
        return None;
    }

    let distro = parts.next()?.trim();
    if distro.is_empty() {
        return None;
    }

    let mut linux_path = String::new();
    for part in parts {
        match part {
            "." => {}
            "" => {}
            segment => {
                linux_path.push('/');
                linux_path.push_str(segment);
            }
        }
    }

    if linux_path.is_empty() {
        linux_path.push('/');
    }

    Some(WslWorkspace {
        distro: distro.to_string(),
        linux_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_wsl_localhost_unc_paths() {
        let workspace =
            WslWorkspace::from_unc_path(Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo"))
                .expect("expected WSL workspace");

        assert_eq!(workspace.distro(), "Ubuntu");
        assert_eq!(workspace.linux_path(), "/home/iain/repo");
    }

    #[test]
    fn detects_legacy_wsl_unc_paths() {
        let workspace = WslWorkspace::from_unc_path(Path::new(
            r"\\wsl$\archlinux\tomato\.\paprika\..\aubergine.txt",
        ))
        .expect("expected WSL workspace");

        assert_eq!(workspace.distro(), "archlinux");
        assert_eq!(workspace.linux_path(), "/tomato/paprika/../aubergine.txt");
    }

    #[test]
    fn detects_verbatim_unc_paths() {
        let workspace =
            WslWorkspace::from_unc_path(Path::new(r"\\?\UNC\wsl.localhost\Debian\workspace"))
                .expect("expected WSL workspace");

        assert_eq!(workspace.distro(), "Debian");
        assert_eq!(workspace.linux_path(), "/workspace");
    }

    #[test]
    fn ignores_non_wsl_paths() {
        assert!(WslWorkspace::from_unc_path(Path::new(r"C:\Users\iain\repo")).is_none());
        assert!(WslWorkspace::from_unc_path(Path::new(r"\\server\share\repo")).is_none());
        assert!(WslWorkspace::from_unc_path(Path::new(r"\\wsl.localhost")).is_none());
    }

    #[test]
    fn builds_wsl_environment_capture_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_environment_capture_command(&workspace);
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo"));
        assert!(debug.contains("env -0"));
    }

    #[test]
    fn builds_wsl_remote_hello_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_remote_hello_command(&workspace);
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("hello"));
    }
}
