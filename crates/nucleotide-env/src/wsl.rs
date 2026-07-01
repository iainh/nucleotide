// ABOUTME: WSL workspace detection and command construction helpers
// ABOUTME: Converts Windows WSL UNC paths into Linux paths for remote tooling

use nucleotide_remote::{
    DirectoryListingResponse, EnvironmentResponse, FileCreateResponse, FileReadResponse,
    FileRenameResponse, FileSearchResponse, GlobalSearchResponse, HelloResponse, PROTOCOL_VERSION,
    WorkspaceMetadataResponse, WorkspaceRootResponse, WorkspaceSymbolFilesOptions,
    WorkspaceSymbolFilesResponse,
};
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::time::{Duration, Instant};
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
        self.unc_path_for_linux_path(&self.linux_path)
            .unwrap_or_else(|| format!(r"\\wsl.localhost\{}", self.distro))
    }

    pub fn unc_path_for_linux_path(&self, linux_path: impl AsRef<Path>) -> Option<String> {
        let linux_path = linux_path.as_ref().as_os_str().to_string_lossy();
        if !linux_path.starts_with('/') {
            return None;
        }

        let mut unc = format!(r"\\wsl.localhost\{}", self.distro);
        for segment in linux_path
            .trim_start_matches('/')
            .split('/')
            .filter(|segment| !segment.is_empty())
        {
            unc.push('\\');
            unc.push_str(segment);
        }
        Some(unc)
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

pub fn build_wsl_remote_metadata_command(workspace: &WslWorkspace) -> Command {
    build_wsl_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("metadata"),
    )
}

pub fn build_wsl_remote_metadata_tokio_command(
    workspace: &WslWorkspace,
) -> tokio::process::Command {
    build_wsl_tokio_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("metadata"),
    )
}

pub fn build_wsl_remote_workspace_root_command(workspace: &WslWorkspace) -> Command {
    build_wsl_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("root"),
    )
}

pub fn build_wsl_remote_directory_listing_command(workspace: &WslWorkspace) -> Command {
    build_wsl_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("list"),
    )
}

pub fn build_wsl_remote_file_search_command(workspace: &WslWorkspace) -> Command {
    build_wsl_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("files"),
    )
}

pub fn build_wsl_remote_global_search_command(
    workspace: &WslWorkspace,
    query: &str,
    smart_case: bool,
    limit: usize,
) -> Command {
    build_wsl_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_global_search_script(query, smart_case, limit),
    )
}

pub fn build_wsl_remote_file_read_command(path: &Path, limit: usize) -> Option<Command> {
    let (workspace, file_name) = wsl_file_parent_workspace(path)?;
    Some(build_wsl_shell_command(
        &workspace,
        "/bin/sh",
        &wsl_remote_helper_file_read_script(&file_name, limit),
    ))
}

pub fn build_wsl_remote_create_file_command(parent: &Path, name: &str) -> Option<Command> {
    let workspace = WslWorkspace::from_unc_path(parent)?;
    Some(build_wsl_shell_command(
        &workspace,
        "/bin/sh",
        &wsl_remote_helper_create_file_script(name),
    ))
}

pub fn build_wsl_remote_create_directory_command(parent: &Path, name: &str) -> Option<Command> {
    let workspace = WslWorkspace::from_unc_path(parent)?;
    Some(build_wsl_shell_command(
        &workspace,
        "/bin/sh",
        &wsl_remote_helper_create_directory_script(name),
    ))
}

pub fn build_wsl_remote_rename_path_command(path: &Path, new_name: &str) -> Option<Command> {
    let (workspace, old_name) = wsl_file_parent_workspace(path)?;
    Some(build_wsl_shell_command(
        &workspace,
        "/bin/sh",
        &wsl_remote_helper_rename_path_script(&old_name, new_name),
    ))
}

pub fn build_wsl_remote_workspace_symbol_files_command(
    workspace: &WslWorkspace,
    options: &WorkspaceSymbolFilesOptions,
) -> Command {
    build_wsl_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_workspace_symbol_files_script(options),
    )
}

pub fn build_wsl_remote_directory_listing_tokio_command(
    workspace: &WslWorkspace,
) -> tokio::process::Command {
    build_wsl_tokio_shell_command(
        workspace,
        "/bin/sh",
        &wsl_remote_helper_command_script("list"),
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

pub fn wsl_remote_helper_metadata_script() -> String {
    wsl_remote_helper_command_script("metadata")
}

pub fn wsl_remote_helper_workspace_root_script() -> String {
    wsl_remote_helper_command_script("root")
}

pub fn wsl_remote_helper_directory_listing_script() -> String {
    wsl_remote_helper_command_script("list")
}

pub fn wsl_remote_helper_file_search_script() -> String {
    wsl_remote_helper_command_script("files")
}

pub fn wsl_remote_helper_create_file_script(name: &str) -> String {
    let name = quote_posix_single(name);
    let helper_command = wsl_remote_helper_command_script("create-file");
    format!(
        r#"NUCLEOTIDE_REMOTE_CREATE_NAME={name}
export NUCLEOTIDE_REMOTE_CREATE_NAME
{helper_command}"#
    )
}

pub fn wsl_remote_helper_create_directory_script(name: &str) -> String {
    let name = quote_posix_single(name);
    let helper_command = wsl_remote_helper_command_script("create-directory");
    format!(
        r#"NUCLEOTIDE_REMOTE_CREATE_NAME={name}
export NUCLEOTIDE_REMOTE_CREATE_NAME
{helper_command}"#
    )
}

pub fn wsl_remote_helper_rename_path_script(old_name: &str, new_name: &str) -> String {
    let old_name = quote_posix_single(old_name);
    let new_name = quote_posix_single(new_name);
    let helper_command = wsl_remote_helper_command_script("rename");
    format!(
        r#"NUCLEOTIDE_REMOTE_RENAME_OLD_NAME={old_name}
NUCLEOTIDE_REMOTE_RENAME_NEW_NAME={new_name}
export NUCLEOTIDE_REMOTE_RENAME_OLD_NAME NUCLEOTIDE_REMOTE_RENAME_NEW_NAME
{helper_command}"#
    )
}

pub fn wsl_remote_helper_global_search_script(
    query: &str,
    smart_case: bool,
    limit: usize,
) -> String {
    let query = quote_posix_single(query);
    let smart_case = if smart_case { "1" } else { "0" };
    let helper_command = wsl_remote_helper_command_script("search");
    format!(
        r#"NUCLEOTIDE_REMOTE_SEARCH_QUERY={query}
NUCLEOTIDE_REMOTE_SEARCH_SMART_CASE={smart_case}
NUCLEOTIDE_REMOTE_SEARCH_LIMIT={limit}
export NUCLEOTIDE_REMOTE_SEARCH_QUERY NUCLEOTIDE_REMOTE_SEARCH_SMART_CASE NUCLEOTIDE_REMOTE_SEARCH_LIMIT
{helper_command}"#
    )
}

pub fn wsl_remote_helper_file_read_script(path: &str, limit: usize) -> String {
    let path = quote_posix_single(path);
    let helper_command = wsl_remote_helper_command_script("read");
    format!(
        r#"NUCLEOTIDE_REMOTE_READ_PATH={path}
NUCLEOTIDE_REMOTE_READ_LIMIT={limit}
export NUCLEOTIDE_REMOTE_READ_PATH NUCLEOTIDE_REMOTE_READ_LIMIT
{helper_command}"#
    )
}

pub fn wsl_remote_helper_workspace_symbol_files_script(
    options: &WorkspaceSymbolFilesOptions,
) -> String {
    let helper_command = wsl_remote_helper_command_script("symbol-files");
    let max_depth = options
        .max_depth
        .map(|depth| depth.to_string())
        .unwrap_or_else(|| usize::MAX.to_string());
    format!(
        r#"NUCLEOTIDE_REMOTE_SYMBOLS_HIDDEN={hidden}
NUCLEOTIDE_REMOTE_SYMBOLS_PARENTS={parents}
NUCLEOTIDE_REMOTE_SYMBOLS_IGNORE={ignore}
NUCLEOTIDE_REMOTE_SYMBOLS_FOLLOW_LINKS={follow_links}
NUCLEOTIDE_REMOTE_SYMBOLS_GIT_IGNORE={git_ignore}
NUCLEOTIDE_REMOTE_SYMBOLS_GIT_GLOBAL={git_global}
NUCLEOTIDE_REMOTE_SYMBOLS_GIT_EXCLUDE={git_exclude}
NUCLEOTIDE_REMOTE_SYMBOLS_DEDUP_LINKS={dedup_links}
NUCLEOTIDE_REMOTE_SYMBOLS_MAX_DEPTH={max_depth}
NUCLEOTIDE_REMOTE_SYMBOLS_FILE_LIMIT={file_limit}
NUCLEOTIDE_REMOTE_SYMBOLS_FILE_BYTE_LIMIT={file_byte_limit}
NUCLEOTIDE_REMOTE_SYMBOLS_TOTAL_BYTE_LIMIT={total_byte_limit}
export NUCLEOTIDE_REMOTE_SYMBOLS_HIDDEN NUCLEOTIDE_REMOTE_SYMBOLS_PARENTS NUCLEOTIDE_REMOTE_SYMBOLS_IGNORE NUCLEOTIDE_REMOTE_SYMBOLS_FOLLOW_LINKS
export NUCLEOTIDE_REMOTE_SYMBOLS_GIT_IGNORE NUCLEOTIDE_REMOTE_SYMBOLS_GIT_GLOBAL NUCLEOTIDE_REMOTE_SYMBOLS_GIT_EXCLUDE NUCLEOTIDE_REMOTE_SYMBOLS_DEDUP_LINKS
export NUCLEOTIDE_REMOTE_SYMBOLS_MAX_DEPTH NUCLEOTIDE_REMOTE_SYMBOLS_FILE_LIMIT NUCLEOTIDE_REMOTE_SYMBOLS_FILE_BYTE_LIMIT NUCLEOTIDE_REMOTE_SYMBOLS_TOTAL_BYTE_LIMIT
{helper_command}"#,
        hidden = env_bool_value(options.hidden),
        parents = env_bool_value(options.parents),
        ignore = env_bool_value(options.ignore),
        follow_links = env_bool_value(options.follow_links),
        git_ignore = env_bool_value(options.git_ignore),
        git_global = env_bool_value(options.git_global),
        git_exclude = env_bool_value(options.git_exclude),
        dedup_links = env_bool_value(options.deduplicate_links),
        file_limit = options.file_limit,
        file_byte_limit = options.file_byte_limit,
        total_byte_limit = options.total_byte_limit,
    )
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

pub async fn load_wsl_remote_metadata(
    workspace: &WslWorkspace,
    timeout_duration: Duration,
) -> Result<WorkspaceMetadataResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_metadata_tokio_command(workspace);
    let output = timeout(timeout_duration, command.output())
        .await
        .map_err(|_| WslRemoteHelperError::Timeout(timeout_duration))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(WslRemoteHelperError::CommandFailed(stderr));
    }

    parse_remote_metadata_output(&output.stdout)
}

pub async fn load_wsl_remote_directory_listing(
    workspace: &WslWorkspace,
    timeout_duration: Duration,
) -> Result<DirectoryListingResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_directory_listing_tokio_command(workspace);
    let output = timeout(timeout_duration, command.output())
        .await
        .map_err(|_| WslRemoteHelperError::Timeout(timeout_duration))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(WslRemoteHelperError::CommandFailed(stderr));
    }

    parse_remote_directory_listing_output(&output.stdout)
}

pub fn load_wsl_remote_directory_listing_blocking(
    workspace: &WslWorkspace,
    timeout_duration: Duration,
) -> Result<DirectoryListingResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_directory_listing_command(workspace);
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_directory_listing_output(&stdout)
}

pub fn load_wsl_remote_workspace_root_blocking(
    workspace: &WslWorkspace,
    timeout_duration: Duration,
) -> Result<WorkspaceRootResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_workspace_root_command(workspace);
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_workspace_root_output(&stdout)
}

pub fn load_wsl_remote_file_search_blocking(
    workspace: &WslWorkspace,
    timeout_duration: Duration,
) -> Result<FileSearchResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_file_search_command(workspace);
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_file_search_output(&stdout)
}

pub fn load_wsl_remote_global_search_blocking(
    workspace: &WslWorkspace,
    query: &str,
    smart_case: bool,
    limit: usize,
    timeout_duration: Duration,
) -> Result<GlobalSearchResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_global_search_command(workspace, query, smart_case, limit);
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_global_search_output(&stdout)
}

pub fn load_wsl_remote_workspace_symbol_files_blocking(
    workspace: &WslWorkspace,
    options: &WorkspaceSymbolFilesOptions,
    timeout_duration: Duration,
) -> Result<WorkspaceSymbolFilesResponse, WslRemoteHelperError> {
    let mut command = build_wsl_remote_workspace_symbol_files_command(workspace, options);
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_workspace_symbol_files_output(&stdout)
}

pub fn load_wsl_remote_file_read_blocking(
    path: &Path,
    limit: usize,
    timeout_duration: Duration,
) -> Result<FileReadResponse, WslRemoteHelperError> {
    let Some(mut command) = build_wsl_remote_file_read_command(path, limit) else {
        return Err(WslRemoteHelperError::CommandFailed(format!(
            "not a readable WSL file path: {}",
            path.display()
        )));
    };
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_file_read_output(&stdout)
}

pub fn create_wsl_remote_file_blocking(
    parent: &Path,
    name: &str,
    timeout_duration: Duration,
) -> Result<FileCreateResponse, WslRemoteHelperError> {
    let Some(mut command) = build_wsl_remote_create_file_command(parent, name) else {
        return Err(WslRemoteHelperError::CommandFailed(format!(
            "not a WSL directory path: {}",
            parent.display()
        )));
    };
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_file_create_output(&stdout)
}

pub fn create_wsl_remote_directory_blocking(
    parent: &Path,
    name: &str,
    timeout_duration: Duration,
) -> Result<FileCreateResponse, WslRemoteHelperError> {
    let Some(mut command) = build_wsl_remote_create_directory_command(parent, name) else {
        return Err(WslRemoteHelperError::CommandFailed(format!(
            "not a WSL directory path: {}",
            parent.display()
        )));
    };
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_file_create_output(&stdout)
}

pub fn rename_wsl_remote_path_blocking(
    path: &Path,
    new_name: &str,
    timeout_duration: Duration,
) -> Result<FileRenameResponse, WslRemoteHelperError> {
    let Some(mut command) = build_wsl_remote_rename_path_command(path, new_name) else {
        return Err(WslRemoteHelperError::CommandFailed(format!(
            "not a WSL file path: {}",
            path.display()
        )));
    };
    let stdout = run_blocking_wsl_remote_command(&mut command, timeout_duration)?;
    parse_remote_file_rename_output(&stdout)
}

fn run_blocking_wsl_remote_command(
    command: &mut Command,
    timeout_duration: Duration,
) -> Result<Vec<u8>, WslRemoteHelperError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut output = Vec::new();
        if let Some(mut stdout) = stdout {
            stdout.read_to_end(&mut output)?;
        }
        Ok(output)
    });
    let stderr_reader = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut output = Vec::new();
        if let Some(mut stderr) = stderr {
            stderr.read_to_end(&mut output)?;
        }
        Ok(output)
    });
    let deadline = Instant::now() + timeout_duration;

    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = stdout_reader
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("stdout reader panicked")))?;
            let stderr = stderr_reader
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("stderr reader panicked")))?;

            if !status.success() {
                let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
                return Err(WslRemoteHelperError::CommandFailed(stderr));
            }

            return Ok(stdout);
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(WslRemoteHelperError::Timeout(timeout_duration));
        }

        std::thread::sleep(Duration::from_millis(10));
    }
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

fn parse_remote_metadata_output(
    output: &[u8],
) -> Result<WorkspaceMetadataResponse, WslRemoteHelperError> {
    let response: WorkspaceMetadataResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_workspace_root_output(
    output: &[u8],
) -> Result<WorkspaceRootResponse, WslRemoteHelperError> {
    let response: WorkspaceRootResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_directory_listing_output(
    output: &[u8],
) -> Result<DirectoryListingResponse, WslRemoteHelperError> {
    let response: DirectoryListingResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_file_search_output(
    output: &[u8],
) -> Result<FileSearchResponse, WslRemoteHelperError> {
    let response: FileSearchResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_global_search_output(
    output: &[u8],
) -> Result<GlobalSearchResponse, WslRemoteHelperError> {
    let response: GlobalSearchResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_file_read_output(output: &[u8]) -> Result<FileReadResponse, WslRemoteHelperError> {
    let response: FileReadResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_file_create_output(
    output: &[u8],
) -> Result<FileCreateResponse, WslRemoteHelperError> {
    let response: FileCreateResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_file_rename_output(
    output: &[u8],
) -> Result<FileRenameResponse, WslRemoteHelperError> {
    let response: FileRenameResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn parse_remote_workspace_symbol_files_output(
    output: &[u8],
) -> Result<WorkspaceSymbolFilesResponse, WslRemoteHelperError> {
    let response: WorkspaceSymbolFilesResponse = serde_json::from_slice(output)?;
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(WslRemoteHelperError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            actual: response.protocol_version,
        });
    }

    Ok(response)
}

fn wsl_file_parent_workspace(path: &Path) -> Option<(WslWorkspace, String)> {
    let workspace = WslWorkspace::from_unc_path(path)?;
    let linux_path = workspace.linux_path();
    let (parent, file_name) = linux_path.rsplit_once('/')?;
    if file_name.is_empty() {
        return None;
    }
    let parent = if parent.is_empty() { "/" } else { parent };

    Some((
        WslWorkspace {
            distro: workspace.distro().to_string(),
            linux_path: parent.to_string(),
        },
        file_name.to_string(),
    ))
}

fn env_bool_value(value: bool) -> &'static str {
    if value { "1" } else { "0" }
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
    let (flag, script) = wsl_shell_invocation(shell, script);
    command
        .push_arg(shell)
        .push_arg(flag)
        .push_arg(script.as_str());
}

fn wsl_shell_invocation(shell: &str, script: &str) -> (&'static str, String) {
    if shell.ends_with("/sh") || shell == "sh" {
        ("-c", wsl_user_login_shell_wrapper(script))
    } else {
        ("-lc", script.to_string())
    }
}

fn wsl_user_login_shell_wrapper(script: &str) -> String {
    let quoted_script = quote_posix_single(script);
    format!(
        r#"script={quoted_script}
shell="${{SHELL:-/bin/sh}}"
case "$shell" in
  ""|sh|*/sh) exec /bin/sh -c "$script" ;;
  *) if [ -x "$shell" ]; then exec "$shell" -lc "$script"; fi; exec /bin/sh -c "$script" ;;
esac"#
    )
}

fn quote_posix_single(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
    fn maps_linux_paths_back_to_wsl_unc_paths() {
        let workspace =
            WslWorkspace::from_unc_path(Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo"))
                .expect("expected WSL workspace");

        assert_eq!(
            workspace.unc_path_for_linux_path(Path::new("/home/iain/repo/src")),
            Some(r"\\wsl.localhost\Ubuntu\home\iain\repo\src".to_string())
        );
        assert_eq!(
            workspace.unc_path_for_linux_path(Path::new("/")),
            Some(r"\\wsl.localhost\Ubuntu".to_string())
        );
        assert_eq!(
            workspace.unc_path_for_linux_path(Path::new("relative")),
            None
        );
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
        assert!(debug.contains("-c"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("hello"));
    }

    #[test]
    fn wsl_shell_invocation_uses_portable_wrapper_for_sh() {
        let (flag, script) = wsl_shell_invocation("/bin/sh", "echo 'hello'");

        assert_eq!(flag, "-c");
        assert!(script.contains(r#"shell="${SHELL:-/bin/sh}""#));
        assert!(script.contains(r#"exec "$shell" -lc "$script""#));
        assert!(script.contains(r#"echo '"'"'hello'"'"#));
    }

    #[test]
    fn wsl_shell_invocation_keeps_login_mode_for_explicit_login_shells() {
        let (flag, script) = wsl_shell_invocation("/bin/bash", "env -0");

        assert_eq!(flag, "-lc");
        assert_eq!(script, "env -0");
    }

    #[test]
    fn remote_helper_cache_path_is_versioned() {
        assert_eq!(
            wsl_remote_helper_cache_path(),
            "$HOME/.cache/nucleotide/remote-helper/10/nucleotide-remote"
        );
    }

    #[test]
    fn remote_helper_hello_script_prefers_cached_helper_before_path() {
        let script = wsl_remote_helper_hello_script();

        assert!(script.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" hello"#));
        assert!(script.contains("exec nucleotide-remote hello"));
    }

    #[test]
    fn remote_helper_env_script_prefers_cached_helper_before_path() {
        let script = wsl_remote_helper_env_script();

        assert!(script.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" env"#));
        assert!(script.contains("exec nucleotide-remote env"));
    }

    #[test]
    fn remote_helper_metadata_script_prefers_cached_helper_before_path() {
        let script = wsl_remote_helper_metadata_script();

        assert!(script.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" metadata"#));
        assert!(script.contains("exec nucleotide-remote metadata"));
    }

    #[test]
    fn remote_helper_workspace_root_script_prefers_cached_helper_before_path() {
        let script = wsl_remote_helper_workspace_root_script();

        assert!(script.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" root"#));
        assert!(script.contains("exec nucleotide-remote root"));
    }

    #[test]
    fn remote_helper_directory_listing_script_prefers_cached_helper_before_path() {
        let script = wsl_remote_helper_directory_listing_script();

        assert!(script.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" list"#));
        assert!(script.contains("exec nucleotide-remote list"));
    }

    #[test]
    fn remote_helper_file_search_script_prefers_cached_helper_before_path() {
        let script = wsl_remote_helper_file_search_script();

        assert!(script.contains("NUCLEOTIDE_REMOTE_HELPER"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" files"#));
        assert!(script.contains("exec nucleotide-remote files"));
    }

    #[test]
    fn remote_helper_create_file_script_passes_create_environment() {
        let script = wsl_remote_helper_create_file_script("it isn't easy.rs");

        assert!(script.contains("NUCLEOTIDE_REMOTE_CREATE_NAME='it isn'\"'\"'t easy.rs'"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" create-file"#));
        assert!(script.contains("exec nucleotide-remote create-file"));
    }

    #[test]
    fn remote_helper_create_directory_script_passes_create_environment() {
        let script = wsl_remote_helper_create_directory_script("new folder");

        assert!(script.contains("NUCLEOTIDE_REMOTE_CREATE_NAME='new folder'"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" create-directory"#));
        assert!(script.contains("exec nucleotide-remote create-directory"));
    }

    #[test]
    fn remote_helper_rename_path_script_passes_rename_environment() {
        let script = wsl_remote_helper_rename_path_script("old file.rs", "new file.rs");

        assert!(script.contains("NUCLEOTIDE_REMOTE_RENAME_OLD_NAME='old file.rs'"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_RENAME_NEW_NAME='new file.rs'"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" rename"#));
        assert!(script.contains("exec nucleotide-remote rename"));
    }

    #[test]
    fn remote_helper_global_search_script_passes_search_environment() {
        let script = wsl_remote_helper_global_search_script("needle's haystack", true, 25);

        assert!(script.contains("NUCLEOTIDE_REMOTE_SEARCH_QUERY='needle'\"'\"'s haystack'"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_SEARCH_SMART_CASE=1"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_SEARCH_LIMIT=25"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" search"#));
        assert!(script.contains("exec nucleotide-remote search"));
    }

    #[test]
    fn remote_helper_file_read_script_passes_read_environment() {
        let script = wsl_remote_helper_file_read_script("it isn't easy.rs", 128);

        assert!(script.contains("NUCLEOTIDE_REMOTE_READ_PATH='it isn'\"'\"'t easy.rs'"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_READ_LIMIT=128"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" read"#));
        assert!(script.contains("exec nucleotide-remote read"));
    }

    #[test]
    fn remote_helper_workspace_symbol_files_script_passes_scan_environment() {
        let script =
            wsl_remote_helper_workspace_symbol_files_script(&WorkspaceSymbolFilesOptions {
                hidden: true,
                parents: false,
                ignore: true,
                follow_links: true,
                git_ignore: true,
                git_global: false,
                git_exclude: true,
                deduplicate_links: false,
                max_depth: Some(3),
                file_limit: 42,
                file_byte_limit: 1024,
                total_byte_limit: 4096,
            });

        assert!(script.contains("NUCLEOTIDE_REMOTE_SYMBOLS_HIDDEN=1"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_SYMBOLS_PARENTS=0"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_SYMBOLS_FOLLOW_LINKS=1"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_SYMBOLS_MAX_DEPTH=3"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_SYMBOLS_FILE_LIMIT=42"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_SYMBOLS_FILE_BYTE_LIMIT=1024"));
        assert!(script.contains("NUCLEOTIDE_REMOTE_SYMBOLS_TOTAL_BYTE_LIMIT=4096"));
        assert!(script.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(script.contains(r#"exec "$helper" symbol-files"#));
        assert!(script.contains("exec nucleotide-remote symbol-files"));
    }

    #[test]
    fn builds_wsl_remote_directory_listing_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo/src".to_string(),
        };
        let command = build_wsl_remote_directory_listing_command(&workspace);
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo/src"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("list"));
    }

    #[test]
    fn builds_wsl_remote_workspace_root_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo/src".to_string(),
        };
        let command = build_wsl_remote_workspace_root_command(&workspace);
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo/src"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("root"));
    }

    #[test]
    fn builds_wsl_remote_file_search_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_remote_file_search_command(&workspace);
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("files"));
    }

    #[test]
    fn builds_wsl_remote_create_file_command() {
        let parent = Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo\src");
        let command =
            build_wsl_remote_create_file_command(parent, "main.rs").expect("create file command");
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo/src"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_CREATE_NAME"));
        assert!(debug.contains("main.rs"));
        assert!(debug.contains("create-file"));
    }

    #[test]
    fn builds_wsl_remote_create_directory_command() {
        let parent = Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo\src");
        let command = build_wsl_remote_create_directory_command(parent, "components")
            .expect("create directory command");
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo/src"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_CREATE_NAME"));
        assert!(debug.contains("components"));
        assert!(debug.contains("create-directory"));
    }

    #[test]
    fn builds_wsl_remote_rename_path_command() {
        let path = Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo\src\old.rs");
        let command =
            build_wsl_remote_rename_path_command(path, "new.rs").expect("rename path command");
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo/src"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_RENAME_OLD_NAME"));
        assert!(debug.contains("old.rs"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_RENAME_NEW_NAME"));
        assert!(debug.contains("new.rs"));
        assert!(debug.contains("rename"));
    }

    #[test]
    fn builds_wsl_remote_workspace_symbol_files_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_remote_workspace_symbol_files_command(
            &workspace,
            &WorkspaceSymbolFilesOptions::default(),
        );
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("symbol-files"));
    }

    #[test]
    fn builds_wsl_remote_global_search_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_remote_global_search_command(&workspace, "needle", false, 50);
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_SEARCH_QUERY"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_SEARCH_SMART_CASE=0"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_SEARCH_LIMIT=50"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("search"));
    }

    #[test]
    fn builds_wsl_remote_file_read_command_from_file_parent() {
        let command = build_wsl_remote_file_read_command(
            Path::new(r"\\wsl.localhost\Ubuntu\home\iain\repo\src\main.rs"),
            256,
        )
        .expect("expected WSL read command");
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo/src"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_READ_PATH"));
        assert!(debug.contains("main.rs"));
        assert!(debug.contains("NUCLEOTIDE_REMOTE_READ_LIMIT=256"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("read"));
    }

    #[test]
    fn wsl_file_parent_workspace_rejects_distribution_root() {
        assert!(
            build_wsl_remote_file_read_command(Path::new(r"\\wsl.localhost\Ubuntu"), 256).is_none()
        );
    }

    #[test]
    fn remote_helper_install_script_writes_versioned_cache_path() {
        let script = wsl_remote_helper_install_script();

        assert!(
            script
                .contains(r#"helper="$HOME/.cache/nucleotide/remote-helper/10/nucleotide-remote""#)
        );
        assert!(script.contains(r#"mkdir -p "$dir""#));
        assert!(script.contains(r#"cat > "$tmp""#));
        assert!(script.contains(r#"chmod 755 "$tmp""#));
        assert!(script.contains(r#"mv "$tmp" "$helper""#));
        assert!(script.contains(r#""$helper" hello >/dev/null"#));
    }

    #[test]
    fn parses_remote_hello_response() {
        let output = br#"{"protocol_version":10,"helper_version":"0.1.0","os":"linux","arch":"x86_64","current_dir":"/home/iain/repo"}
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
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo","variables":{"PATH":"/usr/bin","SHELL":"/bin/bash"}}
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
    fn parses_remote_metadata_response() {
        let output = br#"{"protocol_version":10,"helper_version":"0.1.0","os":"linux","arch":"x86_64","current_dir":"/home/iain/repo","home_dir":"/home/iain","path_separator":"/"}
"#;

        let response = parse_remote_metadata_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.helper_version, "0.1.0");
        assert_eq!(response.os, "linux");
        assert_eq!(response.arch, "x86_64");
        assert_eq!(response.current_dir, Path::new("/home/iain/repo"));
        assert_eq!(response.home_dir.as_deref(), Some(Path::new("/home/iain")));
        assert_eq!(response.path_separator, "/");
    }

    #[test]
    fn rejects_remote_metadata_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"helper_version":"0.1.0","os":"linux","arch":"x86_64","current_dir":"/home/iain/repo","home_dir":null,"path_separator":"/"}"#;

        let error = parse_remote_metadata_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn parses_remote_workspace_root_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo/src","workspace_root":"/home/iain/repo","workspace_marker":".git","project_root":"/home/iain/repo","project_marker":"Cargo.toml"}
"#;

        let response = parse_remote_workspace_root_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo/src"));
        assert_eq!(
            response.workspace_root.as_deref(),
            Some(Path::new("/home/iain/repo"))
        );
        assert_eq!(response.workspace_marker.as_deref(), Some(".git"));
        assert_eq!(
            response.project_root.as_deref(),
            Some(Path::new("/home/iain/repo"))
        );
        assert_eq!(response.project_marker.as_deref(), Some("Cargo.toml"));
    }

    #[test]
    fn rejects_remote_workspace_root_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"current_dir":"/home/iain/repo","workspace_root":null,"workspace_marker":null,"project_root":null,"project_marker":null}"#;

        let error = parse_remote_workspace_root_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn parses_remote_directory_listing_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo","entries":[{"name":"src","kind":"directory","size":4096,"modified_unix_millis":1000,"symlink_target":null,"target_exists":null}]}
"#;

        let response = parse_remote_directory_listing_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo"));
        assert_eq!(response.entries.len(), 1);
        assert_eq!(response.entries[0].name, "src");
    }

    #[test]
    fn rejects_remote_directory_listing_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"current_dir":"/home/iain/repo","entries":[]}"#;

        let error = parse_remote_directory_listing_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn parses_remote_file_search_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo","files":[{"relative_path":"src/main.rs"}],"truncated":false}
"#;

        let response = parse_remote_file_search_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo"));
        assert_eq!(response.files.len(), 1);
        assert_eq!(response.files[0].relative_path, Path::new("src/main.rs"));
        assert!(!response.truncated);
    }

    #[test]
    fn rejects_remote_file_search_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"current_dir":"/home/iain/repo","files":[],"truncated":false}"#;

        let error = parse_remote_file_search_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn parses_remote_global_search_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo","matches":[{"relative_path":"src/main.rs","line":7,"line_text":"needle"}],"truncated":false}
"#;

        let response = parse_remote_global_search_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo"));
        assert_eq!(response.matches.len(), 1);
        assert_eq!(response.matches[0].relative_path, Path::new("src/main.rs"));
        assert_eq!(response.matches[0].line, 7);
        assert_eq!(response.matches[0].line_text, "needle");
        assert!(!response.truncated);
    }

    #[test]
    fn rejects_remote_global_search_protocol_mismatch() {
        let output =
            br#"{"protocol_version":999,"current_dir":"/home/iain/repo","matches":[],"truncated":false}"#;

        let error = parse_remote_global_search_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn parses_remote_file_read_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo/src","path":"main.rs","content":"fn main() {}\n","binary":false,"size":13,"truncated":false}
"#;

        let response = parse_remote_file_read_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo/src"));
        assert_eq!(response.path, Path::new("main.rs"));
        assert_eq!(response.content.as_deref(), Some("fn main() {}\n"));
        assert!(!response.binary);
        assert_eq!(response.size, 13);
        assert!(!response.truncated);
    }

    #[test]
    fn rejects_remote_file_read_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"current_dir":"/home/iain/repo","path":"main.rs","content":null,"binary":true,"size":3,"truncated":false}"#;

        let error = parse_remote_file_read_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn parses_remote_file_create_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo/src","path":"/home/iain/repo/src/main.rs","kind":"file"}
"#;

        let response = parse_remote_file_create_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo/src"));
        assert_eq!(response.path, Path::new("/home/iain/repo/src/main.rs"));
        assert_eq!(response.kind, nucleotide_remote::RemoteFileKind::File);
    }

    #[test]
    fn parses_remote_directory_create_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo/src","path":"/home/iain/repo/src/components","kind":"directory"}
"#;

        let response = parse_remote_file_create_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo/src"));
        assert_eq!(response.path, Path::new("/home/iain/repo/src/components"));
        assert_eq!(response.kind, nucleotide_remote::RemoteFileKind::Directory);
    }

    #[test]
    fn parses_remote_file_rename_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo/src","old_path":"/home/iain/repo/src/old.rs","new_path":"/home/iain/repo/src/new.rs","kind":"file"}
"#;

        let response = parse_remote_file_rename_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo/src"));
        assert_eq!(response.old_path, Path::new("/home/iain/repo/src/old.rs"));
        assert_eq!(response.new_path, Path::new("/home/iain/repo/src/new.rs"));
        assert_eq!(response.kind, nucleotide_remote::RemoteFileKind::File);
    }

    #[test]
    fn rejects_remote_file_rename_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"current_dir":"/home/iain/repo/src","old_path":"/home/iain/repo/src/old.rs","new_path":"/home/iain/repo/src/new.rs","kind":"file"}"#;

        let error = parse_remote_file_rename_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn rejects_remote_file_create_protocol_mismatch() {
        let output = br#"{"protocol_version":999,"current_dir":"/home/iain/repo/src","path":"/home/iain/repo/src/main.rs","kind":"file"}"#;

        let error = parse_remote_file_create_output(output).unwrap_err();

        assert!(matches!(
            error,
            WslRemoteHelperError::ProtocolMismatch {
                expected: PROTOCOL_VERSION,
                actual: 999
            }
        ));
    }

    #[test]
    fn parses_remote_workspace_symbol_files_response() {
        let output = br#"{"protocol_version":10,"current_dir":"/home/iain/repo","files":[{"relative_path":"src/main.rs","content":"fn main() {}\n","size":13}],"truncated":false}
"#;

        let response = parse_remote_workspace_symbol_files_output(output).unwrap();

        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert_eq!(response.current_dir, Path::new("/home/iain/repo"));
        assert_eq!(response.files.len(), 1);
        assert_eq!(response.files[0].relative_path, Path::new("src/main.rs"));
        assert_eq!(response.files[0].content, "fn main() {}\n");
        assert_eq!(response.files[0].size, 13);
        assert!(!response.truncated);
    }

    #[test]
    fn rejects_remote_workspace_symbol_files_protocol_mismatch() {
        let output =
            br#"{"protocol_version":999,"current_dir":"/home/iain/repo","files":[],"truncated":false}"#;

        let error = parse_remote_workspace_symbol_files_output(output).unwrap_err();

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
        assert!(debug.contains("-c"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("env"));
    }

    #[test]
    fn builds_wsl_remote_metadata_command() {
        let workspace = WslWorkspace {
            distro: "Ubuntu".to_string(),
            linux_path: "/home/iain/repo".to_string(),
        };
        let command = build_wsl_remote_metadata_command(&workspace);
        let debug = format!("{command:?}");

        assert_eq!(command.get_program(), "wsl.exe");
        assert!(debug.contains("--distribution"));
        assert!(debug.contains("Ubuntu"));
        assert!(debug.contains("--cd"));
        assert!(debug.contains("/home/iain/repo"));
        assert!(debug.contains("/bin/sh"));
        assert!(debug.contains("-c"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("metadata"));
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
        assert!(debug.contains("-c"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
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
        assert!(debug.contains("-c"));
        assert!(debug.contains(".cache/nucleotide/remote-helper/10/nucleotide-remote"));
        assert!(debug.contains("nucleotide-remote"));
        assert!(debug.contains("hello"));
    }
}
