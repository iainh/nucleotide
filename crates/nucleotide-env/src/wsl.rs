// ABOUTME: WSL workspace detection and command construction helpers
// ABOUTME: Converts Windows WSL UNC paths into Linux paths for remote tooling

use nucleotide_remote::{EnvironmentResponse, HelloResponse, PROTOCOL_VERSION};
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use tokio::time::timeout;

const WSL_LOCALHOST_PREFIX: &str = "wsl.localhost";
const WSL_LEGACY_PREFIX: &str = "wsl$";
const WSL_REMOTE_HELPER_CACHE_ROOT: &str = ".cache/nucleotide/remote-helper";

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
    build_wsl_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("hello"),
    )
}

pub fn build_wsl_remote_hello_tokio_command(workspace: &WslWorkspace) -> tokio::process::Command {
    build_wsl_tokio_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("hello"),
    )
}

pub fn build_wsl_remote_env_command(workspace: &WslWorkspace) -> Command {
    build_wsl_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("env"),
    )
}

pub fn build_wsl_remote_env_tokio_command(workspace: &WslWorkspace) -> tokio::process::Command {
    build_wsl_tokio_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("env"),
    )
}

pub fn build_wsl_remote_helper_install_command(
    workspace: &WslWorkspace,
) -> tokio::process::Command {
    let mut command =
        build_wsl_tokio_shell_command(workspace, "/bin/sh", &wsl_remote_helper_install_script());
    command.stdin(Stdio::piped());
    command
}

pub fn wsl_remote_helper_cache_path() -> String {
    format!(
        "$HOME/{}/{}/nucleotide-remote",
        WSL_REMOTE_HELPER_CACHE_ROOT, PROTOCOL_VERSION
    )
}

pub fn wsl_remote_helper_hello_script() -> String {
    wsl_remote_helper_command_script("hello")
}

pub fn wsl_remote_helper_env_script() -> String {
    wsl_remote_helper_command_script("env")
}

pub fn wsl_remote_helper_install_script() -> String {
    let helper_path = wsl_remote_helper_cache_path();
    format!(
        r#"helper="{helper_path}"
dir="$(dirname "$helper")"
tmp="$helper.tmp.$$"
mkdir -p "$dir"
cat > "$tmp"
chmod 755 "$tmp"
mv "$tmp" "$helper"
"$helper" hello >/dev/null"#
    )
}

fn wsl_remote_helper_command_script(command: &str) -> String {
    let helper_path = wsl_remote_helper_cache_path();
    format!(
        r#"helper="${{NUCLEOTIDE_REMOTE_HELPER:-{helper_path}}}"
if [ -x "$helper" ]; then
  exec "$helper" {command}
fi
exec nucleotide-remote {command}"#
    )
}

#[derive(Debug, thiserror::Error)]
pub enum WslRemoteHelperError {
    #[error("WSL remote helper probe timed out after {0:?}")]
    Timeout(Duration),

    #[error("failed to run WSL remote helper probe: {0}")]
    Io(#[from] std::io::Error),

    #[error("WSL remote helper probe failed: {0}")]
    CommandFailed(String),

    #[error("failed to parse WSL remote helper response: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("WSL remote helper protocol mismatch: expected {expected}, got {actual}")]
    ProtocolMismatch { expected: u32, actual: u32 },
}

pub async fn probe_wsl_remote_helper(
    workspace: &WslWorkspace,
    timeout_duration: Duration,
) -> Result<HelloResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_hello_tokio_command(workspace);
    let output = timeout(timeout_duration, command.output())
        .await
        .map_err(|_| WslRemoteHelperError::Timeout(timeout_duration))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(WslRemoteHelperError::CommandFailed(stderr));
    }

    parse_remote_hello_output(&output.stdout)
}

pub async fn load_wsl_remote_environment(
    workspace: &WslWorkspace,
    timeout_duration: Duration,
) -> Result<EnvironmentResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_env_tokio_command(workspace);
    let output = timeout(timeout_duration, command.output())
        .await
        .map_err(|_| WslRemoteHelperError::Timeout(timeout_duration))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(WslRemoteHelperError::CommandFailed(stderr));
    }

    parse_remote_environment_output(&output.stdout)
}

pub async fn install_wsl_remote_helper(
    workspace: &WslWorkspace,
    local_helper_path: &Path,
    timeout_duration: Duration,
) -> Result<(), WslRemoteHelperError> {
    let helper_file = std::fs::File::open(local_helper_path)?;
    let mut command = build_wsl_remote_helper_install_command(workspace);
    command.stdin(Stdio::from(helper_file));

    let output = timeout(timeout_duration, command.output())
        .await
        .map_err(|_| WslRemoteHelperError::Timeout(timeout_duration))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(WslRemoteHelperError::CommandFailed(stderr));
    }

    Ok(())
}

fn parse_remote_hello_output(output: &[u8]) -> Result<HelloResponse, WslRemoteHelperError> {
    let response: HelloResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_environment_output(
    output: &[u8],
) -> Result<EnvironmentResponse, WslRemoteHelperError> {
    let response: EnvironmentResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
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
    use std::collections::BTreeMap;
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
        assert!(debug.contains("/bin/sh"));
        assert!(debug.contains("-lc"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/1/nucleotide-remote"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("hello"));
    }

    #[test]
    fn remote_helper_cache_path_is_versioned() {
        assert_eq!(
            wsl_remote_helper_cache_path(),
            "$HOME/.cache/nucleotide/remote-helper/1/nucleotide-remote"
        );
    }

    #[test]
    fn remote_helper_hello_script_prefers_cached_helper_before_path() {
        let script = wsl_remote_helper_hello_script();

        assert!(script.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(script.contains(".cache/nucleotide/remote-helper/1/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" hello"#));
        assert!(script.contains("exec nucleotide-remote hello"));
    }

    #[test]
    fn remote_helper_env_script_prefers_cached_helper_before_path() {
        let script = wsl_remote_helper_env_script();

        assert!(script.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(script.contains(".cache/nucleotide/remote-helper/1/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" env"#));
        assert!(script.contains("exec nucleotide-remote env"));
    }

    #[test]
    fn remote_helper_install_script_writes_versioned_cache_path() {
        let script = wsl_remote_helper_install_script();

        assert!(
            script
                .contains(r#"helper="$HOME/.cache/nucleotide/remote-helper/1/nucleotide-remote""#)
        );
        assert!(script.contains(r#"mkdir -p "$dir""#));
        assert!(script.contains(r#"cat > "$tmp""#));
        assert!(script.contains(r#"chmod 755 "$tmp""#));
        assert!(script.contains(r#"mv "$tmp" "$helper""#));
        assert!(script.contains(r#""$helper" hello >/dev/null"#));
    }

    #[test]
    fn parses_remote_hello_response() {
        let output = br#"{"protocol_version":1,"helper_version":"0.1.0","os":"linux","arch":"x86_64","current_dir":"/home/iain/repo"}
"#;

        let response = parse_remote_hello_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.os, "linux");
        assert_eq!(response.current_dir, Path::new("/home/iain/repo"));
    }

    #[test]
    fn rejects_remote_hello_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"helper_version":"0.1.0","os":"linux","arch":"x86_64","current_dir":"/home/iain/repo"}"#;

        let error = parse_remote_hello_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn parses_remote_environment_response() {
        let output = br#"{"protocol_version":1,"current_dir":"/home/iain/repo","variables":{"PATH":"/usr/bin","SHELL":"/bin/bash"}}
"#;

        let response = parse_remote_environment_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo"));
        assert_eq!(
            response.variables,
            BTreeMap::from([
                ("PATH".to_string(), "/usr/bin".to_string()),
                ("SHELL".to_string(), "/bin/bash".to_string()),
            ])
        );
    }

    #[test]
    fn rejects_remote_environment_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"current_dir":"/home/iain/repo","variables":{}}"#;

        let error = parse_remote_environment_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn builds_wsl_remote_env_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_remote_env_command(&workspace);
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo"));
        assert!(debug.contains("/bin/sh"));
        assert!(debug.contains("-lc"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/1/nucleotide-remote"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("env"));
    }

    #[test]
    fn builds_wsl_remote_helper_install_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_remote_helper_install_command(&workspace);
        let debug = format!("{command:?}");

        assert!(debug.contains("wsl.exe"));
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo"));
        assert!(debug.contains("/bin/sh"));
        assert!(debug.contains("-lc"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/1/nucleotide-remote"));
        assert!(debug.contains("cat >"));
        assert!(debug.contains("chmod 755"));
    }

    #[tokio::test]
    async fn install_wsl_remote_helper_rejects_missing_local_binary_before_wsl() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };

        let error = install_wsl_remote_helper(
            &workspace,
            Path::new(r"C:\definitely\missing\nucleotide-remote.exe"),
            Duration::from_millis(1),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, WslRemoteHelperError::Io(_)));
    }

    #[test]
    fn builds_wsl_remote_hello_tokio_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_remote_hello_tokio_command(&workspace);
        let debug = format!("{command:?}");

        assert!(debug.contains("wsl.exe"));
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("/bin/sh"));
        assert!(debug.contains("-lc"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/1/nucleotide-remote"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("hello"));
    }
}
