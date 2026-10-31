// ABOUTME: Application module decomposition for V2 event system migration
// ABOUTME: Contains domain-specific handlers and main Application implementation

mod completion;
pub mod editor_input;
mod inlay_hints;
mod project_lsp;
mod pull_diagnostics;
mod symbols;
#[cfg(feature = "terminal-emulator-core")]
pub mod terminal_handler;
mod workspace_edit;
pub mod workspace_file_ops;

#[cfg(test)]
use completion::CompletionServerFuture;
pub use completion::{LspCompletionTrigger, PendingCompletionRequest};
#[cfg(test)]
use inlay_hints::truncate_inlay_hint_label;
use pull_diagnostics::{
    PullAllDocumentsDiagnosticsHandler, PullDiagnosticsHandler, register_pull_diagnostic_hooks,
    request_all_document_diagnostics_for_language_server,
};
use workspace_edit::PendingLspWorkspaceEdit;

#[cfg(feature = "terminal-emulator-core")]
pub use terminal_handler::RemoteTerminalSessionProvider;
#[cfg(feature = "terminal-emulator-core")]
pub use terminal_handler::TerminalRuntimeHandle;
#[cfg(not(feature = "terminal-emulator-core"))]
#[derive(Clone, Default)]
pub struct TerminalRuntimeHandle;
#[cfg(not(feature = "terminal-emulator-core"))]
impl TerminalRuntimeHandle {
    pub fn new() -> Self {
        Self
    }

    pub fn dispatch(&self, _event: &nucleotide_events::terminal::Event) {}

    pub fn send_input(
        &self,
        _id: nucleotide_events::terminal::TerminalId,
        _bytes: Vec<u8>,
    ) -> bool {
        false
    }

    pub fn has_remote_terminal_provider_for(&self, _workspace: &std::path::Path) -> bool {
        false
    }

    pub fn handles_remote_terminal(
        &self,
        _workspace: &std::path::Path,
        _cwd: &std::path::Path,
    ) -> bool {
        false
    }
}
pub use workspace_file_ops::WorkspaceFileOpHandler;

use arc_swap::{ArcSwap, access::Map};
use futures_util::{
    future::{BoxFuture, FutureExt},
    stream::{FuturesOrdered, StreamExt},
};
use helix_core::{Position, Rope, RopeSlice, Selection, Uri, pos_at_coords, syntax};
use helix_loader::workspace_trust::WorkspaceTrust;
use helix_lsp::{LanguageServerId, LspProgressMap, OffsetEncoding, lsp};
use helix_stdx::path::get_relative_path;
use helix_view::{
    document::{
        Document, DocumentOpenMetadata, DocumentSaveData, DocumentWriteResult, Mode, to_writer,
    },
    input::KeyEvent,
    keyboard::{KeyCode, KeyModifiers},
};
use nucleotide_events::{ProjectLspCommand, ProjectLspCommandError};
use nucleotide_lsp::{
    HelixLspBridge, LspLaunchProxy, LspLaunchProxyProvider, ProjectEnvironmentSource,
    ProjectServerLifecycle, RemoteLspSessionProvider, ServerStatus,
};
use nucleotide_workspace::{
    FileKind, FileRead, FileVersion, ProcessSpec, ReadOptions, WorkspaceBackendHandle,
    WorkspaceIdentity, WorkspaceLocation, WriteOptions, classify_workspace_location,
    local_workspace_backend, posix_path_string, remote_startup_workspace_root,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll, Wake, Waker},
    time::{Duration, Instant, SystemTime},
};

// Import our shell environment system
use nucleotide_env::ProjectEnvironment;

/// Implementation of EnvironmentProvider trait for our ProjectEnvironment
/// This bridges our environment system with the LSP system
pub struct ProjectEnvironmentProvider {
    project_environment: Arc<ProjectEnvironment>,
    workspace_backend: Mutex<WorkspaceBackendHandle>,
}

impl ProjectEnvironmentProvider {
    pub fn new(project_environment: Arc<ProjectEnvironment>) -> Self {
        Self::with_workspace_backend(project_environment, local_workspace_backend())
    }

    pub fn with_workspace_backend(
        project_environment: Arc<ProjectEnvironment>,
        workspace_backend: WorkspaceBackendHandle,
    ) -> Self {
        Self {
            project_environment,
            workspace_backend: Mutex::new(workspace_backend),
        }
    }

    pub fn set_workspace_backend(&self, workspace_backend: WorkspaceBackendHandle) {
        *self
            .workspace_backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = workspace_backend;
    }
}

impl nucleotide_lsp::EnvironmentProvider for ProjectEnvironmentProvider {
    fn get_lsp_environment(
        &self,
        directory: &std::path::Path,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        std::collections::HashMap<String, String>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send
                + '_,
        >,
    > {
        let project_env = self.project_environment.clone();
        let workspace_backend = self
            .workspace_backend
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let directory = directory.to_path_buf();

        Box::pin(async move {
            if !matches!(workspace_backend.identity(), WorkspaceIdentity::Local) {
                return workspace_backend
                    .project_environment(&directory)
                    .await
                    .map(|snapshot| snapshot.variables.into_iter().collect())
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
            }

            match project_env.get_lsp_environment(&directory).await {
                Ok(env) => Ok(env),
                Err(e) => Err(Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            }
        })
    }
}

pub struct WorkspaceDocumentSaveHandler {
    workspace_backend: WorkspaceBackendHandle,
}

impl WorkspaceDocumentSaveHandler {
    pub fn new(workspace_backend: WorkspaceBackendHandle) -> Self {
        Self { workspace_backend }
    }
}

impl helix_view::editor::DocumentSaveHandler for WorkspaceDocumentSaveHandler {
    fn save(
        &self,
        doc: &mut Document,
        path: Option<PathBuf>,
        force: bool,
    ) -> Option<anyhow::Result<helix_view::document::DocumentSavedEventFuture>> {
        let target_path =
            workspace_document_save_target(&self.workspace_backend.identity(), doc, path)?;
        let workspace_backend = self.workspace_backend.clone();
        let save = doc.save_with(Some(target_path), force, move |save| {
            let workspace_backend = workspace_backend.clone();
            async move { write_remote_document(workspace_backend, save).await }
        });

        Some(save.map(|future| future.boxed()))
    }
}

fn should_use_workspace_backend_for_document_io(
    backend_identity: &WorkspaceIdentity,
    path: &Path,
) -> bool {
    !matches!(backend_identity, WorkspaceIdentity::Local)
        || classify_workspace_location(path).is_remote()
}

fn workspace_document_save_target(
    backend_identity: &WorkspaceIdentity,
    doc: &Document,
    requested_path: Option<PathBuf>,
) -> Option<PathBuf> {
    let doc_path = doc.path();
    if should_use_native_save_for_settings_file(doc_path, requested_path.as_deref()) {
        return None;
    }

    match requested_path {
        Some(path) if should_use_workspace_backend_for_document_io(backend_identity, &path) => {
            Some(path)
        }
        Some(path) if path.is_relative() => doc
            .path()
            .filter(|doc_path| {
                should_use_workspace_backend_for_document_io(backend_identity, doc_path)
            })
            .and_then(|doc_path| doc_path.parent().map(|parent| parent.join(path))),
        Some(_) => None,
        None => doc
            .path()
            .filter(|path| should_use_workspace_backend_for_document_io(backend_identity, path))
            .map(std::path::Path::to_path_buf),
    }
}

fn should_use_native_save_for_settings_file(
    doc_path: Option<&Path>,
    requested_path: Option<&Path>,
) -> bool {
    let local_settings_path = helix_loader::config_dir().join("nucleotide.toml");

    match requested_path {
        Some(path) if path == local_settings_path => true,
        Some(path) if path.is_relative() => {
            doc_path.is_some_and(|doc_path| doc_path == local_settings_path)
        }
        None => doc_path.is_some_and(|doc_path| doc_path == local_settings_path),
        Some(_) => false,
    }
}

async fn write_remote_document(
    workspace_backend: WorkspaceBackendHandle,
    save: DocumentSaveData,
) -> anyhow::Result<DocumentWriteResult> {
    let mut bytes = Vec::new();
    to_writer(&mut bytes, save.encoding_with_bom_info, &save.text).await?;
    let expected_modified = (!save.force
        && save.previous_path.as_deref() == Some(save.path.as_path()))
    .then_some(save.last_saved_time);
    let expected_version = (!save.force
        && save.previous_path.as_deref() == Some(save.path.as_path()))
    .then(|| save.file_version.map(FileVersion::from_bytes))
    .flatten();

    let result = workspace_backend
        .write_file(
            &save.path,
            &bytes,
            WriteOptions {
                create_parent_dirs: save.force,
                expected_modified,
                expected_version,
            },
        )
        .await?;

    Ok(DocumentWriteResult {
        save_time: result.modified.unwrap_or_else(SystemTime::now),
        file_version: result.version.map(FileVersion::into_bytes),
    })
}

const REMOTE_DIFF_BASE_READ_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const REMOTE_DIFF_BASE_TIMEOUT_MS: u64 = 10_000;

async fn read_remote_git_diff_base(
    workspace_backend: &WorkspaceBackendHandle,
    path: &Path,
) -> anyhow::Result<Option<Vec<u8>>> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    let Some(file_name) = path.file_name() else {
        return Ok(None);
    };

    let ls_files = workspace_backend
        .run_process(ProcessSpec {
            program: "git".to_string(),
            args: vec![
                "ls-files".to_string(),
                "-z".to_string(),
                "--full-name".to_string(),
                "--".to_string(),
                file_name.to_string_lossy().into_owned(),
            ],
            cwd: parent.to_path_buf(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: Some(REMOTE_DIFF_BASE_READ_LIMIT_BYTES),
            timeout_ms: Some(REMOTE_DIFF_BASE_TIMEOUT_MS),
        })
        .await?;

    if !ls_files.success || ls_files.timed_out || ls_files.stdout_truncated {
        return Ok(None);
    }

    let relative_path = ls_files
        .stdout
        .split(|byte| *byte == 0)
        .find(|entry| !entry.is_empty());
    let Some(relative_path) = relative_path else {
        return Ok(None);
    };
    let relative_path = String::from_utf8_lossy(relative_path);

    let show = workspace_backend
        .run_process(ProcessSpec {
            program: "git".to_string(),
            args: vec!["show".to_string(), format!("HEAD:{relative_path}")],
            cwd: parent.to_path_buf(),
            env: BTreeMap::new(),
            clear_env: false,
            inherit_project_environment: false,
            stdin: Vec::new(),
            max_output_bytes: Some(REMOTE_DIFF_BASE_READ_LIMIT_BYTES),
            timeout_ms: Some(REMOTE_DIFF_BASE_TIMEOUT_MS),
        })
        .await?;

    if show.success && !show.timed_out && !show.stdout_truncated {
        Ok(Some(show.stdout))
    } else {
        Ok(None)
    }
}

pub struct RemoteLspLaunchProxyProvider {
    options: nucleotide_remote::RemoteWorkspaceBackendOptions,
    helper_path_cache: Mutex<HashMap<WorkspaceLocation, PathBuf>>,
}

impl RemoteLspLaunchProxyProvider {
    pub fn from_environment() -> Self {
        Self {
            options: nucleotide_remote::RemoteWorkspaceBackendOptions::from_environment(),
            helper_path_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            options: config.remote_workspace_backend_options(),
            helper_path_cache: Mutex::new(HashMap::new()),
        }
    }

    fn resolved_helper_path(
        &self,
        location: &WorkspaceLocation,
    ) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(helper_path) = self
            .helper_path_cache
            .lock()
            .map_err(|_| std::io::Error::other("remote LSP helper cache lock poisoned"))?
            .get(location)
            .cloned()
        {
            return Ok(helper_path);
        }

        let helper_path =
            nucleotide_remote::resolve_remote_helper_for_location(location, &self.options)?;
        self.helper_path_cache
            .lock()
            .map_err(|_| std::io::Error::other("remote LSP helper cache lock poisoned"))?
            .insert(location.clone(), helper_path.clone());

        Ok(helper_path)
    }
}

impl LspLaunchProxyProvider for RemoteLspLaunchProxyProvider {
    fn create_lsp_launch_proxy(
        &self,
        workspace_root: &std::path::Path,
        _server_name: &str,
        server_command: &str,
    ) -> Result<Option<LspLaunchProxy>, Box<dyn std::error::Error + Send + Sync>> {
        let location = classify_workspace_location(workspace_root);
        if !location.is_remote() {
            return Ok(None);
        }

        if Path::new(server_command).components().count() != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("remote LSP command must be resolved through PATH: {server_command}"),
            )
            .into());
        }

        let helper_path = self.resolved_helper_path(&location)?;
        let Some(command) = nucleotide_remote::remote_lsp_proxy_command_for_location_with_options(
            &location,
            helper_path,
            &self.options,
            server_command,
        ) else {
            return Ok(None);
        };

        let shim_dir = std::env::temp_dir().join(format!(
            "nuc-remote-lsp-shims-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_micros()
        ));
        std::fs::create_dir_all(&shim_dir)?;
        let shim_path = shim_dir.join(lsp_shim_file_name(server_command));
        write_lsp_proxy_shim(&shim_path, &command)?;

        Ok(Some(LspLaunchProxy {
            path_dir: shim_dir.clone(),
            cleanup_paths: vec![shim_path, shim_dir],
            description: command.display_context(),
        }))
    }
}

/// Adapts the negotiated workspace service session capability to Helix's
/// externally-owned stdio transport.
struct ServiceRemoteLspSessionProvider {
    display_root: PathBuf,
    native_root: PathBuf,
    launcher: Arc<dyn nucleotide_remote::RemotePipedProcessSessionLauncher>,
}

struct ServiceRemoteLspLifecycle {
    // Drop order is intentional: cancellation guard first, then completion.
    _lifecycle: Box<dyn Send>,
    _completion: nucleotide_remote::RemoteProcessSessionCompletionFuture,
}

fn service_remote_lsp_session_provider(
    connection: &nucleotide_remote::WorkspaceBackendConnection,
) -> Option<Arc<dyn RemoteLspSessionProvider>> {
    connection
        .process_session_launcher
        .as_ref()
        .map(|launcher| {
            Arc::new(ServiceRemoteLspSessionProvider {
                display_root: connection.location.display_root().to_path_buf(),
                native_root: connection.location.native_root().to_path_buf(),
                launcher: Arc::clone(launcher),
            }) as Arc<dyn RemoteLspSessionProvider>
        })
}

impl RemoteLspSessionProvider for ServiceRemoteLspSessionProvider {
    fn handles_workspace(&self, workspace_root: &Path) -> bool {
        workspace_root.starts_with(&self.display_root)
    }

    fn start_remote_lsp_session(
        &self,
        workspace_root: &Path,
        _server_name: &str,
        command: &str,
        args: &[String],
        process_cwd: &Path,
        environment: &HashMap<String, String>,
    ) -> Result<helix_lsp::ExternalTransport, Box<dyn std::error::Error + Send + Sync>> {
        let native_workspace_root = nucleotide_workspace::WorkspacePathMapping::new(
            self.display_root.clone(),
            self.native_root.clone(),
        )
        .to_native_path(workspace_root);
        // The bridge gives direct sessions native cwd. Keep accepting a display
        // path here so the adapter remains robust when called directly in tests.
        let native_cwd = if process_cwd.starts_with(&self.display_root) {
            nucleotide_workspace::WorkspacePathMapping::new(
                self.display_root.clone(),
                self.native_root.clone(),
            )
            .to_native_path(process_cwd)
        } else {
            process_cwd.to_path_buf()
        };
        let parts = self
            .launcher
            .launch(nucleotide_remote::RemotePipedProcessSessionRequest {
                program: command.to_string(),
                args: args.to_vec(),
                cwd: native_cwd,
                environment_anchor: native_workspace_root,
                env_overrides: environment.clone().into_iter().collect(),
            })?;
        Ok(helix_lsp::ExternalTransport {
            stdin: Box::new(parts.stdin),
            stdout: Box::new(parts.stdout),
            stderr: Box::new(parts.stderr),
            lifecycle_guard: Box::new(ServiceRemoteLspLifecycle {
                _lifecycle: parts.lifecycle,
                _completion: parts.completion,
            }),
        })
    }
}

fn lsp_shim_file_name(server_name: &str) -> String {
    let sanitized = server_name.replace(['/', '\\'], "_");
    if cfg!(windows) {
        format!("{sanitized}.cmd")
    } else {
        sanitized
    }
}

fn write_lsp_proxy_shim(
    shim_path: &Path,
    command: &nucleotide_remote::RemoteServiceCommand,
) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let script = format!(
            "@echo off\r\n{} %*\r\n",
            windows_command_invocation(command)
        );
        std::fs::write(shim_path, script)
    }

    #[cfg(not(windows))]
    {
        let script = format!("#!/bin/sh\nexec {} \"$@\"\n", command.display_invocation());
        std::fs::write(shim_path, script)?;
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(shim_path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(shim_path, permissions)
    }
}

#[cfg(windows)]
fn windows_command_invocation(command: &nucleotide_remote::RemoteServiceCommand) -> String {
    let program = resolve_windows_command_program(&command.program);

    std::iter::once(program.as_os_str())
        .chain(command.args.iter().map(std::ffi::OsString::as_os_str))
        .map(quote_windows_command_arg)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(windows)]
fn resolve_windows_command_program(program: &std::ffi::OsStr) -> std::ffi::OsString {
    if let Ok(path) = helix_stdx::env::which(program) {
        return path.into_os_string();
    }

    if program.to_string_lossy().eq_ignore_ascii_case("ssh") {
        if let Some(windir) = std::env::var_os("WINDIR") {
            let ssh_path = PathBuf::from(windir)
                .join("System32")
                .join("OpenSSH")
                .join("ssh.exe");
            if ssh_path.is_file() {
                return ssh_path.into_os_string();
            }
        }
    }

    program.to_os_string()
}

#[cfg(windows)]
fn quote_windows_command_arg(value: &std::ffi::OsStr) -> String {
    let value = value.to_string_lossy();
    if value.is_empty() {
        return "\"\"".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '=' | ':'))
    {
        return value.into_owned();
    }

    format!("\"{}\"", value.replace('"', "\\\""))
}

fn should_stat_picker_root_with_backend(identity: &WorkspaceIdentity) -> bool {
    !matches!(identity, WorkspaceIdentity::Local)
}

fn document_lsp_identifier(doc: &Document) -> Option<lsp::TextDocumentIdentifier> {
    doc.url().map(lsp::TextDocumentIdentifier::new)
}

#[allow(dead_code)]
fn str_prefix_at_byte_limit(value: &str, max_bytes: usize) -> &str {
    let limit = max_bytes.min(value.len());
    if value.is_char_boundary(limit) {
        return &value[..limit];
    }

    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index < limit)
        .last()
        .unwrap_or(0);
    &value[..boundary]
}

#[allow(dead_code)]
fn byte_limited_preview(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }

    format!(
        "{}... ({} bytes total)",
        str_prefix_at_byte_limit(value, max_bytes),
        value.len()
    )
}

fn char_index_for_line_col(text: RopeSlice<'_>, line: usize, col: usize) -> usize {
    let len = text.len_chars();
    let Ok(line_start) = text.try_line_to_char(line) else {
        return len;
    };
    let line_end = text.try_line_to_char(line.saturating_add(1)).unwrap_or(len);

    line_start.saturating_add(col).min(line_end).min(len)
}

use gpui::AppContext;
use helix_term::{args::Args, compositor::Compositor, config::Config, job::Jobs, keymap::Keymaps};
use helix_view::document::DocumentSavedEventResult;
use helix_view::{DocumentId, ViewId};
use helix_view::{Editor, doc_mut, graphics::Rect, handlers::Handlers};

// Helper function to find workspace root from a specific directory
pub fn find_workspace_root_from(start_dir: &Path) -> PathBuf {
    if let Some(root) = find_cargo_workspace_root(start_dir) {
        return root;
    }

    // Fallback: VCS root detection
    const VCS_DIRS: &[&str] = &[".git", ".helix", ".hg", ".jj", ".svn"];
    for ancestor in start_dir.ancestors() {
        let mut vcs_path = ancestor.to_path_buf();
        for &vcs_dir in VCS_DIRS {
            vcs_path.push(vcs_dir);
            if vcs_path.exists() {
                return ancestor.to_path_buf();
            }
            vcs_path.pop();
        }
    }
    start_dir.to_path_buf()
}

fn find_cargo_workspace_root(start_dir: &Path) -> Option<PathBuf> {
    start_dir.ancestors().find_map(|ancestor| {
        let manifest = std::fs::read_to_string(ancestor.join("Cargo.toml")).ok()?;
        let manifest = toml::from_str::<toml::Table>(&manifest).ok()?;
        manifest
            .get("workspace")
            .filter(|workspace| workspace.is_table())
            .map(|_| ancestor.to_path_buf())
    })
}

#[cfg(any(target_os = "windows", test))]
fn path_matches_existing_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(any(target_os = "windows", test))]
fn current_dir_is_executable_dir(current_dir: &Path, current_exe: &Path) -> bool {
    current_exe
        .parent()
        .is_some_and(|exe_dir| path_matches_existing_path(current_dir, exe_dir))
}

fn workspace_marker_exists(path: &Path) -> bool {
    path.join(".git").exists()
        || path.join(".svn").exists()
        || path.join(".hg").exists()
        || path.join(".jj").exists()
        || path.join(".helix").exists()
}

fn buffer_text_matches_string(text: &Rope, file_text: &str) -> bool {
    file_text == *text
}

#[cfg(test)]
fn buffer_text_matches_path(text: &Rope, path: &Path) -> bool {
    std::fs::read_to_string(path)
        .is_ok_and(|file_text| buffer_text_matches_string(text, &file_text))
}

struct DiffResetWorkspaceReconcile {
    doc_id: DocumentId,
    view_id: ViewId,
    path: PathBuf,
    expected_text: Rope,
    workspace_backend: WorkspaceBackendHandle,
}

fn remote_lsp_project_root_for_document(
    workspace_identity: &WorkspaceIdentity,
    project_directory: Option<&Path>,
    document_path: Option<&Path>,
) -> Option<PathBuf> {
    if !matches!(workspace_identity, WorkspaceIdentity::Remote(_)) {
        return None;
    }

    project_directory
        .filter(|path| classify_workspace_location(path).is_remote())
        .map(Path::to_path_buf)
        .or_else(|| {
            document_path
                .filter(|path| classify_workspace_location(path).is_remote())
                .and_then(|path| path.parent().map(Path::to_path_buf))
        })
}

fn remote_lsp_root_uri_matches_document_workspace(
    workspace_identity: &WorkspaceIdentity,
    project_directory: Option<&Path>,
    document_path: Option<&Path>,
    root_path: &Path,
    root_uri: Option<&lsp::Url>,
) -> bool {
    if !matches!(workspace_identity, WorkspaceIdentity::Remote(_)) {
        return true;
    }

    let Some(display_root) =
        remote_lsp_project_root_for_document(workspace_identity, project_directory, document_path)
    else {
        return true;
    };
    let location = classify_workspace_location(&display_root);
    if !location.is_remote() {
        return true;
    }

    let native_root = location.native_root();
    posix_path_string(root_path) == posix_path_string(native_root)
        && root_uri
            .map(|uri| uri.as_str())
            .zip(remote_native_file_uri(native_root).map(|uri| uri.to_string()))
            .is_some_and(|(actual, expected)| actual == expected)
}

fn remote_native_file_uri(path: &Path) -> Option<lsp::Url> {
    helix_lsp::file_uri_from_path(Path::new(&posix_path_string(path)))
}

fn lsp_diagnostic_document_uri(
    uri: lsp::Url,
    project_directory: Option<&Path>,
    workspace_identity: &WorkspaceIdentity,
) -> Result<Uri, String> {
    if matches!(workspace_identity, WorkspaceIdentity::Remote(_))
        && let Some(native_path) = remote_lsp_file_url_path(&uri)
    {
        let display_path =
            navigation_display_path(&native_path, project_directory, workspace_identity);
        return Ok(Uri::from(display_path));
    }

    Uri::try_from(uri).map_err(|error| error.to_string())
}

fn remote_lsp_file_url_path(uri: &lsp::Url) -> Option<PathBuf> {
    if uri.scheme() != "file" {
        return None;
    }
    if has_non_local_file_authority(uri.as_str()) {
        return None;
    }
    if file_url_path_has_windows_drive(uri.path()) {
        return None;
    }

    percent_decode_uri_path(uri.path()).map(PathBuf::from)
}

fn file_url_path_has_windows_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':'
}

fn has_non_local_file_authority(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("file:") else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("//") else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost")
}

fn percent_decode_uri_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).copied().and_then(hex_value)?;
            let low = bytes.get(index + 2).copied().and_then(hex_value)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn file_picker_current_directory(
    workspace_identity: &WorkspaceIdentity,
    project_directory: Option<&Path>,
    current_working_dir: PathBuf,
) -> Option<PathBuf> {
    if matches!(workspace_identity, WorkspaceIdentity::Local) {
        current_working_dir.exists().then_some(current_working_dir)
    } else {
        project_directory.map(Path::to_path_buf)
    }
}

pub fn implicit_workspace_root_from_current_dir() -> Option<PathBuf> {
    let current_dir = std::env::current_dir().ok()?;

    #[cfg(target_os = "windows")]
    if let Ok(current_exe) = std::env::current_exe()
        && current_dir_is_executable_dir(&current_dir, &current_exe)
    {
        info!(
            current_dir = %current_dir.display(),
            executable = %current_exe.display(),
            "Skipping implicit workspace detection from executable directory"
        );
        return None;
    }

    let workspace_root = find_workspace_root_from(&current_dir);
    workspace_marker_exists(&workspace_root).then_some(workspace_root)
}

// Removed unused structs - now using event-driven architecture instead

// Removed unused Tag-related structs and enums

use anyhow::{Context as _, Error, bail};
use nucleotide_core::event_bridge;
use nucleotide_logging::{PerfTimer, debug, error, info, instrument, timed, trace, warn};
use nucleotide_lsp::lsp_state::DiagnosticInfo;

use crate::types::Update;
use editor_input::EditorInputBridge;
use gpui::EventEmitter;
use project_lsp::{
    ProjectLspCoordinator, canonical_project_lsp_root, commands_to_probe_for_servers,
    configured_language_server_commands, configured_project_servers,
    detect_project_lsp_plan_with_backend, discover_project_languages_with_backend,
    probe_available_language_server_commands, project_lsp_error_is_retryable,
    project_lsp_plan_from_discovered_languages, project_lsp_preparation_status,
    project_lsp_startup_status, retain_servers_with_available_commands,
};
#[cfg(test)]
use project_lsp::{project_lsp_plan_from_names, project_marker_names};

const MAINTENANCE_DRAIN_WARN_THRESHOLD: Duration = Duration::from_millis(8);
const MAINTENANCE_ITERATION_WARN_THRESHOLD: Duration = Duration::from_millis(2);
const MAINTENANCE_POLLER_WARN_THRESHOLD: Duration = Duration::from_millis(2);
const MAINTENANCE_TURN_BUDGET: Duration = Duration::from_millis(6);
const MAINTENANCE_HELIX_EVENT_BATCH: usize = 64;
const MAINTENANCE_LSP_COMMAND_BATCH: usize = 1;

fn coalesce_helix_events(
    helix_events: Vec<event_bridge::HelixEvent>,
) -> Vec<event_bridge::HelixEvent> {
    let mut seen_diagnostics = HashSet::new();
    let mut coalesced = Vec::with_capacity(helix_events.len());

    for helix_event in helix_events.into_iter().rev() {
        match &helix_event {
            event_bridge::HelixEvent::DiagnosticsChanged { doc_id } => {
                if !seen_diagnostics.insert(*doc_id) {
                    continue;
                }
            }
            event_bridge::HelixEvent::DocumentChanged { .. }
            | event_bridge::HelixEvent::DocumentOpened { .. }
            | event_bridge::HelixEvent::DocumentClosed { .. }
            | event_bridge::HelixEvent::LanguageServerInitialized { .. }
            | event_bridge::HelixEvent::LanguageServerExited { .. } => {}
        }

        coalesced.push(helix_event);
    }

    coalesced.reverse();
    coalesced
}

#[derive(Clone)]
pub struct MaintenanceWake {
    tx: tokio::sync::mpsc::UnboundedSender<()>,
}

impl MaintenanceWake {
    pub fn channel() -> (Self, tokio::sync::mpsc::UnboundedReceiver<()>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    pub fn notify(&self) {
        let _ = self.tx.send(());
    }

    pub fn waker(&self) -> Waker {
        Waker::from(Arc::new(MaintenanceWakeTask { wake: self.clone() }))
    }
}

struct MaintenanceWakeTask {
    wake: MaintenanceWake,
}

impl Wake for MaintenanceWakeTask {
    fn wake(self: Arc<Self>) {
        self.wake.notify();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake.notify();
    }
}

pub struct Application {
    pub editor: Editor,
    pub compositor: Compositor,
    pub editor_input: EditorInputBridge,
    pub jobs: Jobs,
    pub lsp_progress: LspProgressMap,
    pub lsp_state: Option<gpui::Entity<nucleotide_lsp::LspState>>,
    pub project_directory: Option<PathBuf>,
    pub workspace_backend: WorkspaceBackendHandle,
    pub helix_event_rx: Option<event_bridge::HelixEventReceiver>,
    pub config: crate::config::Config,
    pub helix_config_arc: Arc<ArcSwap<helix_term::config::Config>>,
    project_lsp: ProjectLspCoordinator,
    pub project_environment: Arc<ProjectEnvironment>,
    pub workspace_file_ops: WorkspaceFileOpHandler,
    pending_lsp_workspace_edits: VecDeque<PendingLspWorkspaceEdit>,
    pub terminal_runtime: TerminalRuntimeHandle,
    maintenance_wake: Option<MaintenanceWake>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputKeyEvent {
    pub key: KeyEvent,
    pub is_held: bool,
}

impl InputKeyEvent {
    pub fn new(key: KeyEvent) -> Self {
        Self {
            key,
            is_held: false,
        }
    }

    pub fn from_key_down(key: KeyEvent, is_held: bool) -> Self {
        Self { key, is_held }
    }
}

#[derive(Debug, Clone)]
pub enum InputEvent {
    Key(InputKeyEvent),
    Semantic(editor_input::EditorSemanticAction),
}

impl InputEvent {
    pub fn key(key: KeyEvent) -> Self {
        Self::Key(InputKeyEvent::new(key))
    }

    pub fn key_down(key: KeyEvent, is_held: bool) -> Self {
        Self::Key(InputKeyEvent::from_key_down(key, is_held))
    }

    pub fn semantic(action: editor_input::EditorSemanticAction) -> Self {
        Self::Semantic(action)
    }
}

pub struct Input;

impl EventEmitter<Update> for Application {}

impl gpui::EventEmitter<InputEvent> for Input {}

// Crank struct removed - replaced with event-driven LSP completion processing

const LSP_TOKEN_ACTIVITY: &str = "activity";

fn is_navigation_repeat_key(key: &KeyEvent, mode: Mode) -> bool {
    let has_shortcut_modifier = key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::SUPER);
    match key.code {
        KeyCode::Left
        | KeyCode::Right
        | KeyCode::Up
        | KeyCode::Down
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown => !has_shortcut_modifier,
        KeyCode::Char('h' | 'j' | 'k' | 'l') => {
            mode != Mode::Insert && key.modifiers == KeyModifiers::NONE
        }
        _ => false,
    }
}

impl Application {
    pub fn set_maintenance_wake(&mut self, wake: MaintenanceWake) {
        self.maintenance_wake = Some(wake);
    }

    pub fn request_event_driven_maintenance(&self) {
        if let Some(wake) = &self.maintenance_wake {
            wake.notify();
        }
    }

    fn notify_language_server_stream_added(&self) {
        // `helix_lsp::Registry::incoming` is a dynamically populated
        // `SelectAll`. Pushing a new receiver does not wake the task that last
        // polled the previously empty set, so explicitly poll it once after a
        // project server is registered. Subsequent server messages wake the
        // maintenance task through the receiver's registered waker.
        self.request_event_driven_maintenance();
    }

    pub(crate) fn set_workspace_backend(&mut self, workspace_backend: WorkspaceBackendHandle) {
        self.workspace_backend = workspace_backend.clone();
        #[cfg(feature = "terminal-emulator-core")]
        self.terminal_runtime.set_remote_provider(None);
        self.project_lsp
            .set_workspace_backend(workspace_backend.clone());
        self.editor
            .set_save_handler(Some(Arc::new(WorkspaceDocumentSaveHandler::new(
                workspace_backend,
            ))));
    }

    pub(crate) fn set_workspace_backend_connection(
        &mut self,
        connection: nucleotide_remote::WorkspaceBackendConnection,
    ) {
        let provider = service_remote_lsp_session_provider(&connection);
        #[cfg(feature = "terminal-emulator-core")]
        let terminal_provider = RemoteTerminalSessionProvider::from_connection(&connection);
        self.set_workspace_backend(connection.backend);
        #[cfg(feature = "terminal-emulator-core")]
        self.terminal_runtime.set_remote_provider(terminal_provider);
        self.project_lsp.set_remote_session_provider(provider);
    }

    pub(crate) fn set_editor_status_feedback(
        &mut self,
        cx: &mut gpui::Context<crate::Core>,
        message: String,
        severity: crate::types::Severity,
    ) {
        match severity {
            crate::types::Severity::Error => self.editor.set_error(message.clone()),
            _ => self.editor.set_status(message.clone()),
        }

        cx.emit(crate::Update::EditorStatus(crate::types::EditorStatus {
            status: message,
            severity,
        }));
    }

    fn handle_job_status_message(
        &mut self,
        msg: helix_event::status::StatusMessage,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        let severity = match msg.severity {
            helix_event::status::Severity::Hint => crate::types::Severity::Hint,
            helix_event::status::Severity::Info => crate::types::Severity::Info,
            helix_event::status::Severity::Warning => crate::types::Severity::Warning,
            helix_event::status::Severity::Error => crate::types::Severity::Error,
        };

        let helix_severity = match msg.severity {
            helix_event::status::Severity::Hint => helix_view::editor::Severity::Hint,
            helix_event::status::Severity::Info => helix_view::editor::Severity::Info,
            helix_event::status::Severity::Warning => helix_view::editor::Severity::Warning,
            helix_event::status::Severity::Error => helix_view::editor::Severity::Error,
        };

        let message = msg.message.to_string();
        cx.emit(crate::Update::EditorStatus(crate::types::EditorStatus {
            status: message,
            severity,
        }));

        // Keep Helix's single status slot current while the GPUI notification
        // view keeps recent status messages stacked.
        self.editor.status_msg = Some((msg.message, helix_severity));
        helix_event::request_redraw();
    }

    fn poll_pending_helix_events(
        &mut self,
        cx: &mut gpui::Context<crate::Core>,
        handle: &tokio::runtime::Handle,
        task_cx: &mut TaskContext<'_>,
    ) -> bool {
        let mut helix_events = Vec::new();
        let mut disconnected = false;

        if let Some(ref mut rx) = self.helix_event_rx {
            for _ in 0..MAINTENANCE_HELIX_EVENT_BATCH {
                match rx.poll_recv(task_cx) {
                    Poll::Ready(Some(event)) => helix_events.push(event),
                    Poll::Ready(None) => {
                        info!("Helix event channel disconnected");
                        disconnected = true;
                        break;
                    }
                    Poll::Pending => break,
                }
            }
        }

        if disconnected {
            self.helix_event_rx = None;
        }

        let received_count = helix_events.len();
        let helix_events = coalesce_helix_events(helix_events);
        let progressed = !helix_events.is_empty();

        if received_count >= MAINTENANCE_HELIX_EVENT_BATCH {
            debug!(
                received_count = received_count,
                processed_count = helix_events.len(),
                batch_limit = MAINTENANCE_HELIX_EVENT_BATCH,
                "Maintenance Helix-event poll reached batch limit"
            );
        } else if received_count != helix_events.len() {
            debug!(
                received_count = received_count,
                processed_count = helix_events.len(),
                "Maintenance coalesced Helix events"
            );
        }

        for helix_event in helix_events {
            self.handle_helix_event(&helix_event, handle, cx);
        }

        progressed
    }

    fn handle_helix_event(
        &mut self,
        helix_event: &event_bridge::HelixEvent,
        handle: &tokio::runtime::Handle,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        use nucleotide_events::{LspEvent, document::Event as DocumentEvent};

        match helix_event {
            event_bridge::HelixEvent::DocumentChanged {
                doc_id,
                change_summary,
                line_change,
            } => {
                let revision = if let Some(document) = self.editor.document_mut(*doc_id) {
                    document.get_current_revision() as u64
                } else {
                    warn!(doc_id = ?doc_id, "Document not found when processing DocumentChanged event");
                    0
                };

                cx.emit(Update::Document(DocumentEvent::ContentChanged {
                    doc_id: *doc_id,
                    revision,
                    change_summary: *change_summary,
                    line_change: line_change.clone(),
                }));
            }
            event_bridge::HelixEvent::DocumentOpened { doc_id } => {
                let (path, language_id) = if let Some(document) = self.editor.document(*doc_id) {
                    let path = document
                        .path()
                        .map(std::path::Path::to_path_buf)
                        .unwrap_or_else(|| std::path::PathBuf::from("untitled"));
                    let language_id = document.language_name().map(str::to_string);
                    (path, language_id)
                } else {
                    (std::path::PathBuf::from("unknown"), None)
                };

                if matches!(
                    self.workspace_backend.identity(),
                    WorkspaceIdentity::Remote(_)
                ) {
                    if let Some(document) = self.editor.document_mut(*doc_id) {
                        set_remote_document_lsp_url(document, &path);
                    }
                    self.attach_or_start_remote_document_servers(*doc_id, handle, cx);
                }

                cx.emit(Update::Document(DocumentEvent::Opened {
                    doc_id: *doc_id,
                    path,
                    language_id,
                }));
            }
            event_bridge::HelixEvent::DocumentClosed {
                doc_id,
                was_modified,
            } => {
                cx.emit(Update::Document(DocumentEvent::Closed {
                    doc_id: *doc_id,
                    was_modified: *was_modified,
                }));
            }
            event_bridge::HelixEvent::DiagnosticsChanged { doc_id } => {
                let (diagnostic_count, error_count, warning_count) = if let Some(document) =
                    self.editor.document(*doc_id)
                {
                    let diagnostics = document.diagnostics();
                    let total = diagnostics.len();
                    let errors = diagnostics
                        .iter()
                        .filter(|d| {
                            matches!(d.severity, Some(helix_core::diagnostic::Severity::Error))
                        })
                        .count();
                    let warnings = diagnostics
                        .iter()
                        .filter(|d| {
                            matches!(d.severity, Some(helix_core::diagnostic::Severity::Warning))
                        })
                        .count();

                    if let (Some(lsp_state), Some(path)) = (&self.lsp_state, document.path()) {
                        let uri = helix_core::Uri::from(path);
                        let infos: Vec<DiagnosticInfo> = diagnostics
                            .iter()
                            .filter_map(|diagnostic| {
                                diagnostic.provider.language_server_id().map(|server_id| {
                                    DiagnosticInfo {
                                        diagnostic: diagnostic.clone(),
                                        server_id,
                                    }
                                })
                            })
                            .collect();
                        lsp_state.update(cx, |state, cx| {
                            state.set_diagnostics(uri, infos);
                            cx.notify();
                        });
                    }

                    (total, errors, warnings)
                } else {
                    (0, 0, 0)
                };

                cx.emit(Update::Document(DocumentEvent::DiagnosticsUpdated {
                    doc_id: *doc_id,
                    diagnostic_count,
                    error_count,
                    warning_count,
                }));
            }
            event_bridge::HelixEvent::LanguageServerInitialized { server_id } => {
                debug!(
                    server_id = ?server_id,
                    "Processing LanguageServerInitialized event"
                );

                if let Some(lsp_state) = &self.lsp_state {
                    lsp_state.update(cx, |state, cx| {
                        let server_name = self
                            .editor
                            .language_server_by_id(*server_id)
                            .map(|ls| ls.name().to_string())
                            .unwrap_or_else(|| format!("Server-{}", server_id));
                        let workspace_path = self
                            .project_directory
                            .as_ref()
                            .map(|p| p.display().to_string());

                        debug!(
                            server_id = ?server_id,
                            server_name = %server_name,
                            workspace = ?workspace_path,
                            "MAIN_LOOP: Registering LSP server in statusline state"
                        );

                        state.register_server(*server_id, server_name, workspace_path);
                        if let Some(server) = state.servers.get_mut(server_id) {
                            server.is_remote = self
                                .helix_lsp_bridge_handle()
                                .is_some_and(|bridge| bridge.is_remote_server(*server_id));
                        }
                        state.update_server_status(
                            *server_id,
                            nucleotide_lsp::ServerStatus::Running,
                        );
                        cx.notify();
                    });
                }

                cx.emit(Update::Lsp(LspEvent::ServerInitialized {
                    server_id: *server_id,
                }));
            }
            event_bridge::HelixEvent::LanguageServerExited { server_id } => {
                debug!(
                    server_id = ?server_id,
                    "Processing LanguageServerExited event"
                );

                if let Some(lsp_state) = &self.lsp_state {
                    lsp_state.update(cx, |state, cx| {
                        debug!(
                            server_id = ?server_id,
                            "MAIN_LOOP: Removing LSP server from statusline state"
                        );

                        state.remove_server(*server_id);
                        cx.notify();
                    });
                }

                cx.emit(Update::Lsp(LspEvent::ServerExited {
                    server_id: *server_id,
                }));
            }
        }
    }

    fn poll_pending_helix_jobs(
        &mut self,
        cx: &mut gpui::Context<crate::Core>,
        task_cx: &mut TaskContext<'_>,
    ) -> bool {
        let mut progressed = false;

        loop {
            match self.jobs.callbacks.poll_recv(task_cx) {
                Poll::Ready(Some(callback)) => {
                    progressed = true;
                    crate::completion_interception::hook_19_job_system("callback_received");
                    info!("📨 JOB CALLBACK RECEIVED: Processing job callback");
                    self.handle_job_callback(callback);
                }
                Poll::Ready(None) => {
                    info!("Helix job callback channel disconnected");
                    break;
                }
                Poll::Pending => break,
            }
        }

        loop {
            match self.jobs.status_messages.poll_recv(task_cx) {
                Poll::Ready(Some(msg)) => {
                    progressed = true;
                    self.handle_job_status_message(msg, cx);
                }
                Poll::Ready(None) => {
                    info!("Helix job status channel disconnected");
                    break;
                }
                Poll::Pending => break,
            }
        }

        while let Poll::Ready(Some(callback)) = self.jobs.wait_futures.poll_next_unpin(task_cx) {
            progressed = true;
            if let Some(job) =
                self.jobs
                    .handle_callback(&mut self.editor, &mut self.compositor, callback)
            {
                self.jobs.add(job);
            }
        }

        progressed
    }

    fn poll_pending_lsp_commands(
        &mut self,
        cx: &mut gpui::Context<crate::Core>,
        handle: &tokio::runtime::Handle,
        task_cx: &mut TaskContext<'_>,
    ) -> bool {
        let _guard = handle.enter();

        debug!("🔧 SYNC: Starting event-driven LSP command processing");

        if self.project_lsp.begin_initialization() {
            info!("🚀 INIT: Initializing project LSP system");
            self.initialize_project_lsp_system(handle);
        }

        let mut commands_processed = 0;
        for _ in 0..MAINTENANCE_LSP_COMMAND_BATCH {
            let (command, disconnected) = self.project_lsp.poll_command(task_cx);

            let Some(lsp_command) = command else {
                if !disconnected {
                    debug!(
                        commands_processed = commands_processed,
                        "🔧 SYNC: No more commands available, exiting loop"
                    );
                }
                break;
            };

            commands_processed += 1;
            info!(
                command_type = ?std::mem::discriminant(&lsp_command),
                command_number = commands_processed,
                "Scheduling non-blocking LSP command"
            );
            self.dispatch_project_lsp_command(lsp_command, cx, handle);

            info!(
                command_number = commands_processed,
                "🔧 SYNC: LSP command processing completed"
            );
        }

        if commands_processed >= MAINTENANCE_LSP_COMMAND_BATCH {
            debug!(
                commands_processed = commands_processed,
                batch_limit = MAINTENANCE_LSP_COMMAND_BATCH,
                "LSP command maintenance reached batch limit"
            );
        }

        debug!(
            total_commands_processed = commands_processed,
            "🔧 SYNC: Completed event-driven LSP command processing"
        );

        commands_processed > 0
    }

    fn dispatch_project_lsp_command(
        &mut self,
        command: ProjectLspCommand,
        cx: &mut gpui::Context<crate::Core>,
        handle: &tokio::runtime::Handle,
    ) {
        match command {
            ProjectLspCommand::OpenProjectSession {
                workspace_root,
                response,
                ..
            } => self.schedule_project_lsp_session(workspace_root, response, false, cx, handle),
            ProjectLspCommand::RestartProjectSession {
                workspace_root,
                response,
                ..
            } => self.schedule_project_lsp_session(workspace_root, response, true, cx, handle),
        }
    }

    fn schedule_project_lsp_session(
        &mut self,
        workspace_root: PathBuf,
        response: tokio::sync::oneshot::Sender<
            Result<nucleotide_events::ProjectSessionResult, ProjectLspCommandError>,
        >,
        restart: bool,
        cx: &mut gpui::Context<crate::Core>,
        handle: &tokio::runtime::Handle,
    ) {
        let workspace_root = canonical_project_lsp_root(&workspace_root);
        let transition = self
            .project_lsp
            .queue_session(workspace_root.clone(), response, restart);
        let Some(transition) = transition else {
            return;
        };
        let generation = transition.generation;

        self.end_previous_project_session(transition.previous_root, cx, handle);

        if let Some(state) = &self.lsp_state {
            let status_root = workspace_root.clone();
            state.update(cx, |state, cx| {
                state.begin_project_session(generation, status_root);
                cx.notify();
            });
        }

        if !classify_workspace_location(&workspace_root).is_remote()
            && let Err(error) = self.editor.set_cwd(&workspace_root)
        {
            warn!(%error, workspace_root = %workspace_root.display(), "Failed to update Editor working directory");
        }

        let remote_workspace = classify_workspace_location(&workspace_root).is_remote();
        let proactive_startup_enabled = self.config.gui.lsp.project_lsp_startup;
        self.set_editor_status_feedback(
            cx,
            project_lsp_preparation_status(remote_workspace, proactive_startup_enabled),
            crate::types::Severity::Info,
        );

        let bridge = self.helix_lsp_bridge_handle();
        let inventory_bridge = bridge.clone();
        let workspace_backend = self.workspace_backend.clone();
        let inventory_backend = workspace_backend.clone();
        let server_commands = configured_language_server_commands(&self.editor);
        let inventory_server_commands = server_commands.clone();
        let syntax_loader = self.editor.syn_loader.load_full();
        let timeout = Duration::from_millis(self.config.gui.lsp.startup_timeout_ms);
        let runtime = handle.clone();

        cx.spawn(async move |this, cx| {
            let task_root = workspace_root.clone();
            let preparation = runtime
                .spawn(async move {
                    let availability_backend = workspace_backend.clone();
                    let plan_future =
                        detect_project_lsp_plan_with_backend(&task_root, workspace_backend);
                    let environment_future = async {
                        match bridge {
                            Some(bridge) => tokio::time::timeout(
                                timeout,
                                bridge.prepare_server_environment(&task_root),
                            )
                            .await
                            .map_err(|_| {
                                format!(
                                    "environment preparation timed out after {}ms",
                                    timeout.as_millis()
                                )
                            })
                            .and_then(|result| result),
                            None => Err("HelixLspBridge not initialized".to_string()),
                        }
                    };
                    let (plan, environment) = tokio::join!(plan_future, environment_future);
                    // Probe only the commands the planned servers need. Checking every
                    // configured language server over the remote backend delays the first
                    // server start by seconds; the language inventory probes the rest later.
                    let planned_servers = configured_project_servers(&syntax_loader, &plan);
                    let available_commands = if remote_workspace && proactive_startup_enabled {
                        probe_available_language_server_commands(
                            &task_root,
                            availability_backend,
                            commands_to_probe_for_servers(&planned_servers, &server_commands, None),
                            environment.as_ref().ok().and_then(Option::as_ref),
                            timeout,
                        )
                        .await
                        .map(Some)
                    } else {
                        Ok(None)
                    };
                    (plan, planned_servers, environment, available_commands)
                })
                .await;

            let Ok((plan, planned_servers, environment, available_commands)) = preparation else {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |app, _cx| {
                        app.project_lsp.complete_session(
                            generation,
                            Err(ProjectLspCommandError::Internal(
                                "project session preparation task failed".to_string(),
                            )),
                        );
                    });
                }
                return;
            };

            if let Some(this) = this.upgrade() {
                this.update(cx, move |app, cx| {
                    if !app
                        .project_lsp
                        .is_current(generation, &workspace_root)
                    {
                        return;
                    }

                    let proactive_startup_enabled = proactive_startup_enabled
                        && app.config.gui.lsp.project_lsp_startup;
                    let mut planned_servers = if proactive_startup_enabled {
                        planned_servers
                    } else {
                        Vec::new()
                    };
                    let available_commands = match available_commands {
                        Ok(available_commands) => available_commands,
                        Err(error) => {
                            warn!(
                                %error,
                                workspace_root = %workspace_root.display(),
                                "Failed to probe remote language server availability; deferring proactive startup"
                            );
                            Some(HashSet::new())
                        }
                    };
                    retain_servers_with_available_commands(
                        &mut planned_servers,
                        &inventory_server_commands,
                        available_commands.as_ref(),
                    );
                    if let Some(state) = &app.lsp_state {
                        let environment_source = match &environment {
                            Ok(Some(_)) => ProjectEnvironmentSource::Project,
                            Ok(None) | Err(_) => ProjectEnvironmentSource::ProcessFallback,
                        };
                        state.update(cx, |state, cx| {
                            state.set_project_server_plan(&planned_servers);
                            state.set_project_environment_source(environment_source);
                            cx.notify();
                        });
                    }
                    let server_names = planned_servers
                        .iter()
                        .map(|(_, server_name)| server_name.clone())
                        .collect::<Vec<_>>();
                    app.set_editor_status_feedback(
                        cx,
                        project_lsp_startup_status(&server_names, proactive_startup_enabled),
                        crate::types::Severity::Info,
                    );
                    let servers_started = app.start_planned_project_servers_prepared(
                        &runtime,
                        generation,
                        &workspace_root,
                        &planned_servers,
                        environment,
                        cx,
                    );

                    let session_result = nucleotide_events::ProjectSessionResult {
                        generation,
                        plan: plan.clone(),
                        language_servers: server_names,
                        servers_started,
                    };
                    app.project_lsp
                        .complete_session(generation, Ok(session_result));
                    let planned_language_ids = plan
                        .languages
                        .iter()
                        .map(|language| language.language_id.clone())
                        .collect::<HashSet<_>>();
                    if proactive_startup_enabled {
                        app.schedule_project_language_inventory(
                            &runtime,
                            generation,
                            workspace_root.clone(),
                            inventory_backend,
                            inventory_bridge,
                            planned_language_ids,
                            inventory_server_commands,
                            available_commands,
                            cx,
                        );
                    }
                    app.sync_lsp_state(cx);
                    cx.emit(crate::Update::Redraw);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule_project_language_inventory(
        &mut self,
        handle: &tokio::runtime::Handle,
        generation: u64,
        workspace_root: PathBuf,
        workspace_backend: WorkspaceBackendHandle,
        bridge: Option<HelixLspBridge>,
        existing_languages: HashSet<String>,
        server_commands: HashMap<String, String>,
        available_commands: Option<HashSet<String>>,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        let runtime = handle.clone();
        let timeout = Duration::from_millis(self.config.gui.lsp.startup_timeout_ms);
        let syntax_loader = self.editor.syn_loader.load_full();
        let remote_workspace = classify_workspace_location(&workspace_root).is_remote();
        let probe_server_commands = server_commands.clone();
        cx.spawn(async move |this, cx| {
            let task_root = workspace_root.clone();
            let preparation = runtime
                .spawn(async move {
                    let server_commands = probe_server_commands;
                    let availability_backend = workspace_backend.clone();
                    let discovered = discover_project_languages_with_backend(
                        &task_root,
                        workspace_backend,
                        &existing_languages,
                    )
                    .await;
                    if discovered.is_empty() {
                        return (Vec::new(), Ok(None), available_commands);
                    }
                    let environment = match bridge {
                        Some(bridge) => tokio::time::timeout(
                            timeout,
                            bridge.prepare_server_environment(&task_root),
                        )
                        .await
                        .map_err(|_| {
                            format!(
                                "environment preparation timed out after {}ms",
                                timeout.as_millis()
                            )
                        })
                        .and_then(|result| result),
                        None => Err("HelixLspBridge not initialized".to_string()),
                    };

                    let discovered_plan = project_lsp_plan_from_discovered_languages(discovered);
                    let servers = configured_project_servers(&syntax_loader, &discovered_plan);

                    // The initial probe covered only the planned servers; check the commands
                    // the discovered languages add and fold them into the known set.
                    let mut available_commands = available_commands;
                    if remote_workspace {
                        let missing = commands_to_probe_for_servers(
                            &servers,
                            &server_commands,
                            available_commands.as_ref(),
                        );
                        if !missing.is_empty() {
                            match probe_available_language_server_commands(
                                &task_root,
                                availability_backend,
                                missing,
                                environment.as_ref().ok().and_then(Option::as_ref),
                                timeout,
                            )
                            .await
                            {
                                Ok(found) => {
                                    available_commands
                                        .get_or_insert_with(HashSet::new)
                                        .extend(found);
                                }
                                Err(error) => {
                                    warn!(
                                        %error,
                                        workspace_root = %task_root.display(),
                                        "Failed to probe remote language server availability for discovered languages; deferring their startup"
                                    );
                                    available_commands.get_or_insert_with(HashSet::new);
                                }
                            }
                        }
                    }
                    (servers, environment, available_commands)
                })
                .await;

            let Ok((mut servers, environment, available_commands)) = preparation else {
                return;
            };
            if servers.is_empty() {
                return;
            }

            if let Some(this) = this.upgrade() {
                this.update(cx, move |app, cx| {
                    if !app.project_lsp.is_current(generation, &workspace_root)
                        || !app.config.gui.lsp.project_lsp_startup
                    {
                        return;
                    }

                    retain_servers_with_available_commands(
                        &mut servers,
                        &server_commands,
                        available_commands.as_ref(),
                    );
                    if let Some(state) = &app.lsp_state {
                        let existing_servers =
                            state.read(cx).project_session.as_ref().map(|session| {
                                session
                                    .servers
                                    .iter()
                                    .map(|server| {
                                        (server.language_id.clone(), server.server_name.clone())
                                    })
                                    .collect::<HashSet<_>>()
                            });
                        if let Some(existing_servers) = existing_servers {
                            servers.retain(|server| !existing_servers.contains(server));
                        }
                        state.update(cx, |state, cx| {
                            state.extend_project_server_plan(&servers);
                            cx.notify();
                        });
                    }
                    if servers.is_empty() {
                        return;
                    }

                    app.start_planned_project_servers_prepared(
                        &runtime,
                        generation,
                        &workspace_root,
                        &servers,
                        environment,
                        cx,
                    );
                    app.sync_lsp_state(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn end_previous_project_session(
        &mut self,
        previous_root: Option<PathBuf>,
        cx: &mut gpui::Context<crate::Core>,
        handle: &tokio::runtime::Handle,
    ) {
        let Some(previous_root) = previous_root else {
            return;
        };

        let stopped_clients = self
            .helix_lsp_bridge_handle()
            .map(|bridge| bridge.stop_workspace_servers(&mut self.editor, &previous_root))
            .unwrap_or_default();

        if let Some(state) = &self.lsp_state {
            state.update(cx, |state, cx| {
                state.clear_all_state();
                cx.notify();
            });
        }
        self.lsp_progress.clear();

        for client in stopped_clients {
            handle.spawn(async move {
                let _ = tokio::time::timeout(
                    Duration::from_millis(500),
                    client.wait_shutdown_flushed(),
                )
                .await;
                drop(client);
            });
        }

        info!(workspace_root = %previous_root.display(), "Ended previous project LSP session");
    }

    fn start_planned_project_servers_prepared(
        &mut self,
        handle: &tokio::runtime::Handle,
        generation: u64,
        workspace_root: &Path,
        planned_servers: &[(String, String)],
        environment: Result<Option<HashMap<String, String>>, String>,
        cx: &mut gpui::Context<crate::Core>,
    ) -> Vec<nucleotide_events::ServerStartResult> {
        let mut results = Vec::new();
        for (language_id, server_name) in planned_servers {
            if !self.project_lsp.is_current(generation, workspace_root) {
                break;
            }
            if let Some(state) = &self.lsp_state {
                state.update(cx, |state, _cx| {
                    state.update_project_server_lifecycle(
                        language_id,
                        server_name,
                        ProjectServerLifecycle::Starting,
                        None,
                    );
                });
            }
            match self.start_lsp_server_prepared(
                workspace_root,
                server_name,
                language_id,
                environment.clone(),
            ) {
                Ok(server_result) => {
                    if let Some(state) = &self.lsp_state {
                        state.update(cx, |state, _cx| {
                            state.update_project_server_lifecycle(
                                language_id,
                                server_name,
                                ProjectServerLifecycle::Initializing,
                                None,
                            );
                        });
                    }
                    self.schedule_project_server_initialization_monitor(
                        handle,
                        generation,
                        workspace_root.to_path_buf(),
                        language_id.clone(),
                        server_name.clone(),
                        server_result.server_id,
                        0,
                        cx,
                    );
                    results.push(server_result);
                }
                Err(error) => {
                    error!(
                        %error,
                        %server_name,
                        %language_id,
                        workspace_root = %workspace_root.display(),
                        "Failed to start planned project language server"
                    );
                    if let Some(state) = &self.lsp_state {
                        let error_message = error.to_string();
                        state.update(cx, |state, _cx| {
                            state.update_project_server_lifecycle(
                                language_id,
                                server_name,
                                ProjectServerLifecycle::Failed {
                                    error: error_message.clone(),
                                },
                                Some(error_message),
                            );
                        });
                    }
                    if project_lsp_error_is_retryable(&error)
                        && self
                            .project_lsp
                            .begin_retry(generation, language_id, server_name)
                    {
                        self.schedule_project_server_retry(
                            handle,
                            generation,
                            workspace_root.to_path_buf(),
                            language_id.clone(),
                            server_name.clone(),
                            0,
                            cx,
                        );
                    }
                }
            }
        }
        results
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule_project_server_initialization_monitor(
        &mut self,
        handle: &tokio::runtime::Handle,
        generation: u64,
        workspace_root: PathBuf,
        language_id: String,
        server_name: String,
        server_id: LanguageServerId,
        retry_attempt: usize,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        let Some(client) = self.editor.language_servers.get_by_id(server_id).cloned() else {
            return;
        };
        let runtime = handle.clone();
        let initialization_timeout =
            Duration::from_millis(self.config.gui.lsp.startup_timeout_ms.max(1_000));

        cx.spawn(async move |this, cx| {
            let initialized = runtime
                .spawn(async move {
                    tokio::time::timeout(initialization_timeout, async {
                        while !client.is_initialized() {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                        }
                    })
                    .await
                    .is_ok()
                })
                .await
                .unwrap_or(false);

            if let Some(this) = this.upgrade() {
                this.update(cx, move |app, cx| {
                    if !app.project_lsp.is_current(generation, &workspace_root) {
                        return;
                    }
                    if app.editor.language_server_by_id(server_id).is_none() {
                        return;
                    }

                    if initialized {
                        app.project_lsp
                            .finish_retry(generation, &language_id, &server_name);
                        if let Some(state) = &app.lsp_state {
                            state.update(cx, |state, cx| {
                                state.update_project_server_lifecycle(
                                    &language_id,
                                    &server_name,
                                    ProjectServerLifecycle::Running,
                                    None,
                                );
                                state.update_server_status(server_id, ServerStatus::Running);
                                cx.notify();
                            });
                        }
                        app.sync_lsp_state(cx);
                        return;
                    }

                    let error = format!(
                        "{} initialization timed out after {}ms",
                        server_name,
                        initialization_timeout.as_millis()
                    );
                    if let Some(bridge) = app.helix_lsp_bridge_handle() {
                        let _ = bridge.stop_server(&mut app.editor, server_id);
                    }
                    if let Some(state) = &app.lsp_state {
                        let status_error = error.clone();
                        state.update(cx, |state, cx| {
                            state.remove_server(server_id);
                            state.update_project_server_lifecycle(
                                &language_id,
                                &server_name,
                                ProjectServerLifecycle::Failed {
                                    error: status_error.clone(),
                                },
                                Some(status_error),
                            );
                            cx.notify();
                        });
                    }

                    let retry_registered = retry_attempt > 0
                        || app
                            .project_lsp
                            .begin_retry(generation, &language_id, &server_name);
                    if retry_registered {
                        app.schedule_project_server_retry(
                            &runtime,
                            generation,
                            workspace_root,
                            language_id,
                            server_name,
                            retry_attempt,
                            cx,
                        );
                    }
                });
            }
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn schedule_project_server_retry(
        &mut self,
        handle: &tokio::runtime::Handle,
        generation: u64,
        workspace_root: PathBuf,
        language_id: String,
        server_name: String,
        attempt: usize,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        const RETRY_DELAYS: [Duration; 3] = [
            Duration::from_millis(250),
            Duration::from_secs(1),
            Duration::from_secs(5),
        ];
        let Some(delay) = RETRY_DELAYS.get(attempt).copied() else {
            self.project_lsp
                .finish_retry(generation, &language_id, &server_name);
            return;
        };
        let Some(bridge) = self.helix_lsp_bridge_handle() else {
            self.project_lsp
                .finish_retry(generation, &language_id, &server_name);
            return;
        };
        let runtime = handle.clone();
        let timeout = Duration::from_millis(self.config.gui.lsp.startup_timeout_ms);
        if let Some(state) = &self.lsp_state {
            let status_language = language_id.clone();
            let status_server = server_name.clone();
            state.update(cx, |state, cx| {
                let last_error = state
                    .project_session
                    .as_ref()
                    .and_then(|session| {
                        session.servers.iter().find(|server| {
                            server.language_id == status_language
                                && server.server_name == status_server
                        })
                    })
                    .and_then(|server| server.last_error.clone());
                state.update_project_server_lifecycle(
                    &status_language,
                    &status_server,
                    ProjectServerLifecycle::RetryScheduled {
                        attempt: attempt + 1,
                        next_retry_at: Instant::now() + delay,
                    },
                    last_error,
                );
                cx.notify();
            });
        }

        info!(
            generation,
            %server_name,
            %language_id,
            attempt = attempt + 1,
            delay_ms = delay.as_millis(),
            "Scheduled project language server retry"
        );

        cx.spawn(async move |this, cx| {
            let prepare_root = workspace_root.clone();
            let environment = runtime
                .spawn(async move {
                    tokio::time::sleep(delay).await;
                    tokio::time::timeout(timeout, bridge.prepare_server_environment(&prepare_root))
                        .await
                        .map_err(|_| {
                            format!(
                                "environment preparation timed out after {}ms",
                                timeout.as_millis()
                            )
                        })
                        .and_then(|result| result)
                })
                .await
                .unwrap_or_else(|error| Err(format!("environment task failed: {error}")));

            if let Some(this) = this.upgrade() {
                this.update(cx, move |app, cx| {
                    if !app.project_lsp.is_current(generation, &workspace_root) {
                        return;
                    }

                    match app.start_lsp_server_prepared(
                        &workspace_root,
                        &server_name,
                        &language_id,
                        environment,
                    ) {
                        Ok(server_result) => {
                            app.schedule_project_server_initialization_monitor(
                                &runtime,
                                generation,
                                workspace_root,
                                language_id.clone(),
                                server_name.clone(),
                                server_result.server_id,
                                attempt + 1,
                                cx,
                            );
                            if let Some(state) = &app.lsp_state {
                                state.update(cx, |state, _cx| {
                                    state.update_project_server_lifecycle(
                                        &language_id,
                                        &server_name,
                                        ProjectServerLifecycle::Initializing,
                                        None,
                                    );
                                });
                            }
                            app.sync_lsp_state(cx);
                            cx.notify();
                        }
                        Err(error) if project_lsp_error_is_retryable(&error) => {
                            app.schedule_project_server_retry(
                                &runtime,
                                generation,
                                workspace_root,
                                language_id,
                                server_name,
                                attempt + 1,
                                cx,
                            );
                        }
                        Err(error) => {
                            app.project_lsp
                                .finish_retry(generation, &language_id, &server_name);
                            if let Some(state) = &app.lsp_state {
                                let error_message = error.to_string();
                                state.update(cx, |state, _cx| {
                                    state.update_project_server_lifecycle(
                                        &language_id,
                                        &server_name,
                                        ProjectServerLifecycle::Failed {
                                            error: error_message.clone(),
                                        },
                                        Some(error_message),
                                    );
                                });
                            }
                            warn!(%error, %server_name, "Project language server retry stopped");
                        }
                    }
                });
            }
        })
        .detach();
    }

    /// Dispatch a workspace operation through the active backend.
    pub fn dispatch_workspace_event(&self, event: nucleotide_events::workspace::Event) {
        self.workspace_file_ops.dispatch(&event);
    }
    /// Initialize the application with its own entity handle for LSP completion
    pub fn post_init(&mut self, cx: &mut gpui::Context<Self>) {
        nucleotide_logging::info!("POST_INIT: Starting application post-initialization");

        // Initialize LSP state entity for statusline indicator
        if self.lsp_state.is_none() {
            nucleotide_logging::info!("POST_INIT: Creating new LspState entity");
            self.lsp_state = Some(cx.new(|_cx| nucleotide_lsp::LspState::new()));
        } else {
            nucleotide_logging::warn!("POST_INIT: LspState entity already exists");
        }

        // Log current LSP server state before initial sync
        let server_count = self.editor.language_servers.iter_clients().count();
        nucleotide_logging::info!(
            server_count = server_count,
            "POST_INIT: Active LSP servers before initial sync"
        );

        // Perform initial LSP state sync to populate any existing servers
        self.sync_lsp_state_initial(cx);

        // Initialize shotgun hook system for comprehensive completion pipeline tracing
        crate::completion_interception::initialize_shotgun_hooks();

        // NOTE: step() should be started as a background task in main.rs initialization
        // Not in post_init to avoid GPUI context complexity

        nucleotide_logging::info!(
            lsp_state_created = self.lsp_state.is_some(),
            "POST_INIT: Application post-initialization completed - LSP completion ready"
        );
    }

    /// Handle language server message, adapted from Helix's implementation
    #[instrument(skip(self, call))]
    pub fn handle_language_server_message(
        &mut self,
        call: helix_lsp::Call,
        server_id: helix_lsp::LanguageServerId,
    ) {
        use helix_lsp::lsp::notification::Notification as _;
        use helix_lsp::{Call, MethodCall, Notification};

        let Some(active_server) = self.editor.language_server_by_id(server_id) else {
            debug!(
                server_id = ?server_id,
                "Ignoring language-server message from a closed project generation"
            );
            return;
        };
        let server_name_for_log = active_server.name().to_string();

        macro_rules! language_server {
            () => {
                match self.editor.language_server_by_id(server_id) {
                    Some(language_server) => language_server,
                    None => {
                        warn!(server_id = ?server_id, "Can't find language server");
                        return;
                    }
                }
            };
        }

        match call {
            Call::Notification(helix_lsp::jsonrpc::Notification { method, params, .. }) => {
                // Log raw incoming notification traffic (JSONL per server when enabled)
                crate::lsp_traffic_logger::log_incoming(
                    server_id,
                    &server_name_for_log,
                    &method,
                    &serde_json::json!({ "raw": format!("{:?}", params) }),
                );
                // Also capture $/logTrace hint (payload shape is server-specific)
                if method == "$/logTrace" {
                    crate::lsp_traffic_logger::log_server_trace(
                        server_id,
                        &server_name_for_log,
                        &format!("{:?}", params),
                    );
                }
                let notification = match Notification::parse(&method, params) {
                    Ok(notification) => notification,
                    Err(helix_lsp::Error::Unhandled) => {
                        debug!(method = %method, "Ignoring unhandled LSP notification");
                        return;
                    }
                    Err(err) => {
                        error!(
                            server_id = ?server_id,
                            method = %method,
                            error = %err,
                            "Failed to parse LSP notification"
                        );
                        return;
                    }
                };

                debug!(
                    server_id = ?server_id,
                    method = %method,
                    "Processing LSP notification"
                );

                // Handle the notification directly like Helix does
                // Note: We only implement the most important notifications for now
                // Additional notifications can be added as needed
                match notification {
                    Notification::Initialized => {
                        let language_server = language_server!();
                        // Trigger workspace configuration if available; force-enable diagnostics for RA
                        let server_name = language_server.name().to_string();
                        if let Some(mut cfg) = language_server.config().cloned() {
                            // DIAG: Try to force-enable diagnostics for rust-analyzer
                            if server_name == "rust-analyzer" {
                                use serde_json::{Value, json};
                                fn merge(a: &mut serde_json::Value, b: serde_json::Value) {
                                    match (a, b) {
                                        (Value::Object(ao), Value::Object(bo)) => {
                                            for (k, v) in bo {
                                                merge(ao.entry(k).or_insert(Value::Null), v);
                                            }
                                        }
                                        (a_slot, b_val) => {
                                            *a_slot = b_val;
                                        }
                                    }
                                }
                                let mut override_cfg =
                                    serde_json::Value::Object(serde_json::Map::new());
                                merge(
                                    &mut override_cfg,
                                    json!({
                                        "rust-analyzer": {"diagnostics": {"enable": true}}
                                    }),
                                );
                                merge(&mut cfg, override_cfg);
                                debug!(
                                    "DIAG: Sending didChangeConfiguration with diagnostics enabled for rust-analyzer"
                                );
                            } else {
                                debug!(server = %server_name, "DIAG: Sending didChangeConfiguration (no RA-specific overrides)");
                            }
                            language_server.did_change_configuration(cfg);
                        } else {
                            debug!(server = %server_name, "DIAG: No config available to send in didChangeConfiguration");
                        }
                        // The frontend initialization hook owns didOpen for existing
                        // attachments; later attachments open their documents immediately.
                        helix_event::dispatch(helix_view::events::LanguageServerInitialized {
                            editor: &mut self.editor,
                            server_id,
                        });
                        debug!(server_id = ?server_id, "LSP server initialized");
                    }
                    Notification::PublishDiagnostics(params) => {
                        let lsp::PublishDiagnosticsParams {
                            uri: diagnostic_uri,
                            diagnostics,
                            version,
                        } = params;
                        let workspace_identity = self.workspace_backend.identity();
                        let uri = match lsp_diagnostic_document_uri(
                            diagnostic_uri,
                            self.project_directory.as_deref(),
                            &workspace_identity,
                        ) {
                            Ok(uri) => uri,
                            Err(err) => {
                                error!(error = %err, "Invalid URI in diagnostics");
                                return;
                            }
                        };
                        let language_server = language_server!();
                        if !language_server.is_initialized() {
                            error!(
                                server_id = ?server_id,
                                server_name = language_server.name(),
                                "Discarding diagnostics from uninitialized server"
                            );
                            return;
                        }

                        // DIAG: Summarize incoming diagnostics by severity
                        let total = diagnostics.len();
                        let mut errors = 0usize;
                        let mut warnings = 0usize;
                        let mut infos = 0usize;
                        let mut hints = 0usize;
                        for d in &diagnostics {
                            match d.severity {
                                Some(helix_lsp::lsp::DiagnosticSeverity::ERROR) => errors += 1,
                                Some(helix_lsp::lsp::DiagnosticSeverity::WARNING) => warnings += 1,
                                Some(helix_lsp::lsp::DiagnosticSeverity::INFORMATION) => infos += 1,
                                Some(helix_lsp::lsp::DiagnosticSeverity::HINT) => hints += 1,
                                Some(_) => {}
                                None => {}
                            }
                        }
                        debug!(
                            server_id = ?server_id,
                            server_name = %language_server.name(),
                            uri = %uri.to_string(),
                            version = ?version,
                            total = total,
                            errors = errors,
                            warnings = warnings,
                            infos = infos,
                            hints = hints,
                            "DIAG: Received LSP publishDiagnostics"
                        );

                        // Handle diagnostics through the editor like Helix does
                        let provider = helix_core::diagnostic::DiagnosticProvider::Lsp {
                            server_id,
                            identifier: None,
                        };

                        self.editor
                            .handle_lsp_diagnostics(&provider, uri, version, diagnostics);
                        trace!("DIAG: Forwarded diagnostics to Helix editor for application");
                    }
                    Notification::ProgressMessage(params) => {
                        use helix_lsp::lsp;

                        let lsp::ProgressParams {
                            token,
                            value: lsp::ProgressParamsValue::WorkDone(work),
                        } = params;

                        // Get server name early to avoid borrowing conflicts
                        let server_name = {
                            let language_server = language_server!();
                            language_server.name().to_string()
                        };

                        debug!(
                            server_id = ?server_id,
                            server_name = %server_name,
                            token = ?token,
                            "Processing LSP progress message"
                        );

                        if let lsp::WorkDoneProgress::End(lsp::WorkDoneProgressEnd {
                            message: None,
                        }) = &work
                        {
                            debug!(
                                server_id = ?server_id,
                                server_name = %server_name,
                                token = ?token,
                                "Processing progress END message"
                            );
                            self.lsp_progress.end_progress(server_id, &token);
                            return;
                        }

                        // Update progress tracking
                        match work {
                            lsp::WorkDoneProgress::Begin(begin_status) => {
                                self.lsp_progress
                                    .begin(server_id, token.clone(), begin_status);
                                info!(
                                    server_id = ?server_id,
                                    server_name = %server_name,
                                    token = ?token,
                                    "Started progress tracking"
                                );
                            }
                            lsp::WorkDoneProgress::Report(report_status) => {
                                self.lsp_progress
                                    .update(server_id, token.clone(), report_status);
                                debug!(
                                    server_id = ?server_id,
                                    server_name = %server_name,
                                    token = ?token,
                                    "Updated progress tracking"
                                );
                            }
                            lsp::WorkDoneProgress::End(_) => {
                                self.lsp_progress.end_progress(server_id, &token);
                                info!(
                                    server_id = ?server_id,
                                    server_name = %server_name,
                                    token = ?token,
                                    "Ended progress tracking"
                                );
                            }
                        }
                    }
                    _ => {
                        debug!(
                            server_id = ?server_id,
                            notification_type = ?notification,
                            "Unhandled LSP notification (this may be expected)"
                        );
                    }
                }
            }
            Call::MethodCall(helix_lsp::jsonrpc::MethodCall {
                method, params, id, ..
            }) => {
                // Log raw incoming server->client method call traffic
                crate::lsp_traffic_logger::log_incoming(
                    server_id,
                    &server_name_for_log,
                    &method,
                    &serde_json::json!({ "raw": format!("{:?}", params) }),
                );
                debug!(
                    server_id = ?server_id,
                    method = %method,
                    id = ?id,
                    "Handling LSP method call"
                );

                let reply = match MethodCall::parse(&method, params) {
                    Err(helix_lsp::Error::Unhandled) => {
                        error!(
                            server_id = ?server_id,
                            method = %method,
                            id = ?id,
                            "Unhandled LSP method call"
                        );
                        Err(helix_lsp::jsonrpc::Error {
                            code: helix_lsp::jsonrpc::ErrorCode::MethodNotFound,
                            message: format!("Method not found: {}", method),
                            data: None,
                        })
                    }
                    Err(err) => {
                        error!(
                            server_id = ?server_id,
                            method = %method,
                            id = ?id,
                            error = %err,
                            "Malformed LSP method call"
                        );
                        Err(helix_lsp::jsonrpc::Error {
                            code: helix_lsp::jsonrpc::ErrorCode::ParseError,
                            message: format!("Malformed method call: {}", method),
                            data: None,
                        })
                    }
                    Ok(MethodCall::WorkDoneProgressCreate(params)) => {
                        // Handle work done progress creation
                        let token = params.token.clone();
                        self.lsp_progress.create(server_id, params.token);
                        debug!(server_id = ?server_id, token = ?token, "Created work done progress");
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::WorkspaceDiagnosticRefresh) => {
                        request_all_document_diagnostics_for_language_server(
                            &mut self.editor,
                            server_id,
                        );
                        debug!(
                            server_id = ?server_id,
                            method = %method,
                            id = ?id,
                            "Refreshing pull diagnostics for language server"
                        );
                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::WorkspaceConfiguration(params)) => {
                        // Reply with per-item configuration values. Prefer the client's config() if available.
                        let cfg_value = {
                            let lang = language_server!();
                            lang.config().cloned()
                        };

                        let mut result = Vec::with_capacity(params.items.len());
                        for _item in &params.items {
                            // Mirror Helix behavior: return the same config per requested item section
                            result.push(cfg_value.clone().unwrap_or(serde_json::Value::Null));
                        }

                        debug!(
                            server_id = ?server_id,
                            items = params.items.len(),
                            has_config = cfg_value.is_some(),
                            "Responding to WorkspaceConfiguration with client config"
                        );
                        Ok(serde_json::Value::Array(result))
                    }
                    Ok(MethodCall::ApplyWorkspaceEdit(params)) => {
                        // Handle workspace edit requests
                        let (is_initialized, offset_encoding) = {
                            let language_server = language_server!();
                            (
                                language_server.is_initialized(),
                                language_server.offset_encoding(),
                            )
                        };

                        if is_initialized {
                            if !matches!(
                                self.workspace_backend.identity(),
                                WorkspaceIdentity::Local
                            ) {
                                self.enqueue_lsp_workspace_edit(
                                    server_id,
                                    id,
                                    offset_encoding,
                                    params.edit,
                                );
                                return;
                            }
                            let res = self.apply_lsp_workspace_edit(offset_encoding, &params.edit);
                            Ok(serde_json::json!({
                                "applied": res.is_ok(),
                                "failureReason": if res.is_err() {
                                    Some("Failed to apply workspace edit".to_string())
                                } else {
                                    None
                                }
                            }))
                        } else {
                            Err(helix_lsp::jsonrpc::Error {
                                code: helix_lsp::jsonrpc::ErrorCode::InvalidRequest,
                                message: "Server not initialized".to_string(),
                                data: None,
                            })
                        }
                    }
                    Ok(MethodCall::RegisterCapability(params)) => {
                        if let Some(client) = self.editor.language_servers.get_by_id(server_id) {
                            for registration in params.registrations {
                                match registration.method.as_str() {
                                    lsp::notification::DidChangeWatchedFiles::METHOD => {
                                        let Some(options) = registration.register_options else {
                                            warn!(
                                                server_id = ?server_id,
                                                registration_id = %registration.id,
                                                "Ignoring watched-file registration without options"
                                            );
                                            continue;
                                        };

                                        let options: lsp::DidChangeWatchedFilesRegistrationOptions =
                                            match serde_json::from_value(options) {
                                                Ok(options) => options,
                                                Err(err) => {
                                                    warn!(
                                                        server_id = ?server_id,
                                                        registration_id = %registration.id,
                                                        error = %err,
                                                        "Failed to deserialize watched-file registration options"
                                                    );
                                                    continue;
                                                }
                                            };

                                        self.editor.language_servers.file_event_handler.register(
                                            client.id(),
                                            Arc::downgrade(client),
                                            registration.id,
                                            options,
                                        );
                                    }
                                    unsupported_method => {
                                        warn!(
                                            server_id = ?server_id,
                                            method = %unsupported_method,
                                            "Ignoring unsupported dynamic capability registration"
                                        );
                                    }
                                }
                            }
                        }

                        Ok(serde_json::Value::Null)
                    }
                    Ok(MethodCall::UnregisterCapability(params)) => {
                        for unregistration in params.unregisterations {
                            match unregistration.method.as_str() {
                                lsp::notification::DidChangeWatchedFiles::METHOD => {
                                    self.editor
                                        .language_servers
                                        .file_event_handler
                                        .unregister(server_id, unregistration.id);
                                }
                                unsupported_method => {
                                    warn!(
                                        server_id = ?server_id,
                                        method = %unsupported_method,
                                        "Ignoring unsupported dynamic capability unregistration"
                                    );
                                }
                            }
                        }

                        Ok(serde_json::Value::Null)
                    }
                    Ok(method_call) => {
                        warn!(
                            server_id = ?server_id,
                            method_call = ?method_call,
                            "Unimplemented LSP method call (returning null)"
                        );
                        Ok(serde_json::Value::Null)
                    }
                };

                // Send the reply
                if let Some(language_server) = self.editor.language_server_by_id(server_id) {
                    if let Err(err) = language_server.reply(id, reply) {
                        error!(
                            server_id = ?server_id,
                            error = %err,
                            "Failed to send LSP method call reply"
                        );
                    }
                } else {
                    warn!(server_id = ?server_id, "Language server not found for reply");
                }
            }
            Call::Invalid { id } => {
                error!(
                    server_id = ?server_id,
                    id = ?id,
                    "Received invalid LSP call"
                );
                // No response needed for invalid calls
            }
        }
    }

    /// Initial LSP state sync during application initialization
    #[instrument(skip(self, cx))]
    pub fn sync_lsp_state_initial(&self, cx: &mut gpui::Context<Self>) {
        nucleotide_logging::info!("INITIAL_SYNC: Starting LSP state initial sync");

        if let Some(lsp_state) = &self.lsp_state {
            nucleotide_logging::info!(
                "INITIAL_SYNC: LspState entity found, checking for active servers"
            );

            // Check for active language servers
            let active_servers: Vec<(LanguageServerId, String, bool)> = self
                .editor
                .language_servers
                .iter_clients()
                .map(|client| {
                    (
                        client.id(),
                        client.name().to_string(),
                        client.is_initialized(),
                    )
                })
                .collect();

            nucleotide_logging::info!(
                server_count = active_servers.len(),
                active_servers = ?active_servers,
                "INITIAL_SYNC: Found active language servers"
            );

            if !active_servers.is_empty() {
                lsp_state.update(cx, |state, cx| {
                    let initial_server_count = state.servers.len();
                    nucleotide_logging::info!(
                        initial_count = initial_server_count,
                        "INITIAL_SYNC: LspState had servers before registration"
                    );

                    // Register all active servers
                    for (id, name, initialized) in active_servers {
                        if !state.servers.contains_key(&id) {
                            nucleotide_logging::info!(
                                server_id = ?id,
                                server_name = %name,
                                "INITIAL_SYNC: Registering LSP server during initial sync"
                            );
                            state.register_server(id, name, None);
                            state.update_server_status(
                                id,
                                if initialized {
                                    ServerStatus::Running
                                } else {
                                    ServerStatus::Initializing
                                },
                            );
                        } else {
                            nucleotide_logging::warn!(
                                server_id = ?id,
                                server_name = %name,
                                "INITIAL_SYNC: Server already registered, skipping"
                            );
                        }
                    }

                    let final_server_count = state.servers.len();
                    nucleotide_logging::info!(
                        final_count = final_server_count,
                        "INITIAL_SYNC: LspState after registration"
                    );

                    cx.notify();
                });
                nucleotide_logging::info!(
                    "INITIAL_SYNC: Initial LSP state sync completed - registered active servers"
                );
            } else {
                nucleotide_logging::warn!(
                    "INITIAL_SYNC: No active LSP servers found during initial sync"
                );
            }
        } else {
            nucleotide_logging::error!("INITIAL_SYNC: No LspState entity found - cannot sync");
        }
    }

    /// Format LSP progress message in Zed's clean style
    fn format_lsp_progress_message(
        &self,
        title: Option<&str>,
        message: Option<&str>,
        percentage: Option<u32>,
        token: &str,
    ) -> String {
        // Start with title, fallback to token
        let mut display_message = title.filter(|s| !s.is_empty()).unwrap_or(token).to_string();

        // Add percentage if available
        if let Some(pct) = percentage {
            display_message.push_str(&format!(" ({}%)", pct));
        }

        // Add detailed message if available and different from title
        if let Some(msg) = message.filter(|s| !s.is_empty() && Some(*s) != title) {
            display_message.push_str(": ");
            display_message.push_str(msg);
        }

        display_message
    }

    /// Sync LSP state from the editor and progress map
    #[instrument(skip(self, cx))]
    pub fn sync_lsp_state(&self, cx: &mut gpui::App) {
        if let Some(lsp_state) = &self.lsp_state {
            let active_servers = self.active_servers();
            let active_server_roots = self
                .editor
                .language_servers
                .iter_clients()
                .map(|client| {
                    let root = client
                        .root_uri()
                        .map(|uri| uri.as_str().to_string())
                        .or_else(|| {
                            let root = client.root_path();
                            (!root.as_os_str().is_empty()).then(|| root.display().to_string())
                        });
                    (client.id(), root)
                })
                .collect::<HashMap<_, _>>();
            let initialized_servers = self
                .editor
                .language_servers
                .iter_clients()
                .filter(|client| client.is_initialized())
                .map(|client| client.id())
                .collect::<HashSet<_>>();
            let project_server_initialization = self
                .editor
                .language_servers
                .iter_clients()
                .map(|client| (client.name().to_string(), client.is_initialized()))
                .collect::<HashMap<_, _>>();
            debug!(active_servers = ?active_servers, "Syncing LSP state");

            let progressing_servers = self.progressing_servers(&active_servers);
            debug!(
                progressing_servers = ?progressing_servers,
                "Servers currently progressing according to lsp_progress"
            );

            let editor_status = self.editor.get_status();
            debug!(editor_status = ?editor_status, "Current editor status from Helix");

            let entries = self.compute_progress_entries(&active_servers, &progressing_servers);

            lsp_state.update(cx, |state, cx| {
                let old_progress_count = state.progress.len();
                debug!(
                    old_progress_count = old_progress_count,
                    "UI state before sync - clearing old progress"
                );
                state.progress.clear();

                // Register servers not yet known
                for (id, name) in &active_servers {
                    if !state.servers.contains_key(id) {
                        info!(server_id = ?id, server_name = %name, "Registering new LSP server");
                        state.register_server(
                            *id,
                            name.clone(),
                            active_server_roots.get(id).cloned().flatten(),
                        );
                    } else if let Some(server) = state.servers.get_mut(id) {
                        server.root_uri = active_server_roots.get(id).cloned().flatten();
                    }
                    if let Some(server) = state.servers.get_mut(id) {
                        server.is_remote = self
                            .helix_lsp_bridge_handle()
                            .is_some_and(|bridge| bridge.is_remote_server(*id));
                    }
                    state.update_server_status(
                        *id,
                        if initialized_servers.contains(id) {
                            ServerStatus::Running
                        } else {
                            ServerStatus::Initializing
                        },
                    );
                }

                if let Some(session) = &mut state.project_session {
                    for server in &mut session.servers {
                        if let Some(initialized) =
                            project_server_initialization.get(&server.server_name)
                        {
                            server.lifecycle = if *initialized {
                                ProjectServerLifecycle::Running
                            } else {
                                ProjectServerLifecycle::Initializing
                            };
                            if *initialized {
                                server.last_error = None;
                            }
                        }
                    }
                }

                for (key, progress) in entries {
                    state.progress.insert(key, progress);
                }

                debug!(
                    final_progress_count = state.progress.len(),
                    server_count = state.servers.len(),
                    "UI state after sync"
                );
                cx.notify();
            });
        }
    }

    /// Safe document access API - read only
    pub fn with_document<F, R>(&self, doc_id: DocumentId, f: F) -> Option<R>
    where
        F: FnOnce(&helix_view::Document) -> R,
    {
        let doc_manager = nucleotide_lsp::DocumentManager::new(&self.editor);
        doc_manager.with_document(doc_id, f)
    }

    /// Safe document access API - mutable
    pub fn with_document_mut<F, R>(&mut self, doc_id: DocumentId, f: F) -> Option<R>
    where
        F: FnOnce(&mut helix_view::Document) -> R,
    {
        let mut doc_manager = nucleotide_lsp::DocumentManagerMut::new(&mut self.editor);
        doc_manager.with_document_mut(doc_id, f)
    }

    /// Safe document access API - returns Result instead of Option
    pub fn try_with_document<F, R, E>(&self, doc_id: DocumentId, f: F) -> Result<R, E>
    where
        F: FnOnce(&helix_view::Document) -> Result<R, E>,
        E: From<String>,
    {
        let doc_manager = nucleotide_lsp::DocumentManager::new(&self.editor);
        doc_manager.try_with_document(doc_id, f)
    }

    /// Safe document access API - mutable with Result
    pub fn try_with_document_mut<F, R, E>(&mut self, doc_id: DocumentId, f: F) -> Result<R, E>
    where
        F: FnOnce(&mut helix_view::Document) -> Result<R, E>,
        E: From<String>,
    {
        let mut doc_manager = nucleotide_lsp::DocumentManagerMut::new(&mut self.editor);
        doc_manager.try_with_document_mut(doc_id, f)
    }

    pub fn create_sample_native_prompt(&self) -> crate::prompt::Prompt {
        crate::prompt::Prompt::native("Search:", "", |_input| {
            // For now, just show the input - we'll handle the actual search via a different mechanism
        })
        .with_cancel(|| {
            // Prompt cancelled
        })
    }

    pub fn create_sample_completion_items(
        &self,
    ) -> Vec<nucleotide_ui::completion_v2::CompletionItem> {
        use nucleotide_ui::completion_v2::{CompletionItem, CompletionItemKind};

        // Create sample completion items with enhanced signature and type information
        vec![
            CompletionItem::new("println!")
                .with_kind(CompletionItemKind::Snippet)
                .with_description("macro")
                .with_signature_info("($($arg:tt)*)")
                .with_type_info("()")
                .with_documentation("Prints to the standard output, with a newline."),
            CompletionItem::new("String")
                .with_kind(CompletionItemKind::Struct)
                .with_description("UTF-8 encoded, growable string")
                .with_type_info("std::string::String")
                .with_documentation("A UTF-8 encoded, growable string."),
            CompletionItem::new("Vec")
                .with_kind(CompletionItemKind::Struct)
                .with_description("Contiguous growable array")
                .with_type_info("std::vec::Vec<T>")
                .with_documentation("A contiguous growable array type."),
            CompletionItem::new("HashMap")
                .with_kind(CompletionItemKind::Struct)
                .with_description("Hash map implementation")
                .with_type_info("std::collections::HashMap<K, V>")
                .with_documentation("A hash map implementation."),
            CompletionItem::new("println")
                .with_kind(CompletionItemKind::Function)
                .with_description("Print with newline")
                .with_signature_info("(&str)")
                .with_type_info("()")
                .with_documentation("Print to stdout with newline"),
            CompletionItem::new("print")
                .with_kind(CompletionItemKind::Function)
                .with_description("Print without newline")
                .with_signature_info("(&str)")
                .with_type_info("()")
                .with_documentation("Print to stdout without newline"),
            CompletionItem::new("format")
                .with_kind(CompletionItemKind::Function)
                .with_description("Create formatted string")
                .with_signature_info("(&str, ...)")
                .with_type_info("String")
                .with_documentation("Create a formatted string"),
        ]
    }

    #[instrument(skip(self))]
    pub fn open_file(&mut self, path: &Path) -> Result<(), anyhow::Error> {
        timed!("open_file", warn_threshold: std::time::Duration::from_millis(500), {
            let mut doc_manager = nucleotide_lsp::DocumentManagerMut::new(&mut self.editor);
            doc_manager.open_file(path)
        })
    }

    fn create_jumplist_picker(&mut self) -> Option<crate::picker::Picker> {
        use crate::picker_view::PickerItem;

        {
            let editor = &mut self.editor;
            let documents = &mut editor.documents;

            for (view, _) in editor.tree.views_mut() {
                let doc_ids = view
                    .jumps
                    .iter()
                    .map(|(doc_id, _)| *doc_id)
                    .collect::<Vec<_>>();

                for doc_id in doc_ids {
                    if let Some(doc) = documents.get_mut(&doc_id) {
                        view.sync_changes(doc);
                    }
                }
            }
        }

        let mut items = Vec::new();

        for (view, _) in self.editor.tree.views() {
            for (doc_id, selection) in view.jumps.iter().rev() {
                let Some(doc) = self.editor.documents.get(doc_id) else {
                    continue;
                };

                let path = doc.path().map(std::path::Path::to_path_buf);
                let path_label = path
                    .as_deref()
                    .map(get_relative_path)
                    .and_then(|path| path.to_str().map(str::to_owned))
                    .unwrap_or_else(|| "[scratch]".to_string());
                let cursor_line = selection.primary().cursor_line(doc.text().slice(..)) + 1;
                let label = if view.doc == *doc_id {
                    format!("* {path_label}:{cursor_line}")
                } else {
                    format!("{path_label}:{cursor_line}")
                };
                let text = selection
                    .fragments(doc.text().slice(..))
                    .map(|fragment| fragment.into_owned())
                    .collect::<Vec<_>>()
                    .join(" ");
                let data = crate::types::JumpLocation {
                    doc_id: *doc_id,
                    selection: selection.clone(),
                };

                items.push(PickerItem {
                    label: label.into(),
                    sublabel: (!text.is_empty()).then(|| text.into()),
                    data: Arc::new(data),
                    file_path: path,
                    vcs_status: None,
                    columns: None,
                });
            }
        }

        if items.is_empty() {
            None
        } else {
            Some(crate::picker::Picker::native(
                "Jump List",
                items,
                |_index| {
                    // Jumplist selection is handled by the overlay via typed item data.
                },
            ))
        }
    }

    fn create_native_prompt(request: editor_input::NativePromptRequest) -> crate::prompt::Prompt {
        let prompt = match request {
            editor_input::NativePromptRequest::Command => ":",
            editor_input::NativePromptRequest::Search => "search:",
            editor_input::NativePromptRequest::ReverseSearch => "rsearch:",
            editor_input::NativePromptRequest::GlobalSearch => "global-search:",
            editor_input::NativePromptRequest::RegexSelection(action) => match action {
                crate::types::RegexSelectionAction::Select => "select:",
                crate::types::RegexSelectionAction::Split => "split:",
                crate::types::RegexSelectionAction::Keep => "keep:",
                crate::types::RegexSelectionAction::Remove => "remove:",
            },
        };

        crate::prompt::Prompt::native(prompt, "", |_| {}).with_cancel(|| {})
    }

    fn emit_overlays(&mut self, cx: &mut gpui::Context<crate::Core>) {
        // Don't take() the autoinfo - just clone it so it persists
        if let Some(info) = &self.editor.autoinfo {
            cx.emit(crate::Update::Info(helix_view::info::Info {
                title: info.title.clone(),
                text: info.text.clone(),
                width: info.width,
                height: info.height,
            }));
        }
    }

    pub(crate) fn reconcile_vcs_after_diff_reset(
        &mut self,
        cx: &mut gpui::Context<crate::Core>,
        handle: &tokio::runtime::Handle,
    ) {
        let view_id = self.editor.tree.focus;
        let Some(doc_id) = self.editor.tree.try_get(view_id).map(|view| view.doc) else {
            return;
        };

        let Some((path, text)) = self.editor.document(doc_id).and_then(|doc| {
            doc.path()
                .map(|path| (path.to_path_buf(), doc.text().clone()))
        }) else {
            return;
        };

        let workspace_backend = self.workspace_backend.clone();
        let modified_reset = if matches!(workspace_backend.identity(), WorkspaceIdentity::Local) {
            self.reset_modified_if_buffer_matches_disk(doc_id, view_id, &path)
        } else {
            self.reset_modified_if_buffer_matches_workspace_async(
                DiffResetWorkspaceReconcile {
                    doc_id,
                    view_id,
                    path: path.clone(),
                    expected_text: text.clone(),
                    workspace_backend,
                },
                handle.clone(),
                cx,
            );
            false
        };

        let vcs_service = cx
            .try_global::<nucleotide_vcs::VcsServiceHandle>()
            .map(|handle| handle.service().clone());
        if let Some(vcs_service) = vcs_service {
            vcs_service.update(cx, |service, cx| {
                service.update_file_diff(&path, text, cx);
            });
        }

        if modified_reset {
            cx.emit(crate::Update::Redraw);
        }
    }

    fn reset_modified_if_buffer_matches_workspace_async(
        &mut self,
        request: DiffResetWorkspaceReconcile,
        handle: tokio::runtime::Handle,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        let DiffResetWorkspaceReconcile {
            doc_id,
            view_id,
            path,
            expected_text,
            workspace_backend,
        } = request;

        cx.spawn(async move |core, cx| {
            let read_path = path.clone();
            let result = handle
                .spawn(async move {
                    workspace_backend
                        .read_file(&read_path, ReadOptions::default())
                        .await
                })
                .await;

            let matches = match result {
                Ok(Ok(read)) if !read.truncated => String::from_utf8(read.bytes)
                    .is_ok_and(|file_text| buffer_text_matches_string(&expected_text, &file_text)),
                Ok(Ok(_)) => false,
                Ok(Err(error)) => {
                    debug!(
                        path = %path.display(),
                        error = %error,
                        "Workspace file read failed while reconciling diff reset"
                    );
                    false
                }
                Err(error) => {
                    warn!(
                        path = %path.display(),
                        error = %error,
                        "Workspace file read task failed while reconciling diff reset"
                    );
                    false
                }
            };

            if !matches {
                return;
            }

            if let Some(core) = core.upgrade() {
                core.update(cx, |core, cx| {
                    if core.reset_modified_if_buffer_matches_text(
                        doc_id,
                        view_id,
                        &path,
                        &expected_text,
                    ) {
                        cx.emit(crate::Update::Redraw);
                    }
                });
            }
        })
        .detach();
    }

    fn reset_modified_if_buffer_matches_disk(
        &mut self,
        doc_id: DocumentId,
        view_id: ViewId,
        path: &Path,
    ) -> bool {
        let Ok(file_text) = std::fs::read_to_string(path) else {
            return false;
        };
        let expected_text = Rope::from_str(&file_text);
        self.reset_modified_if_buffer_matches_text(doc_id, view_id, path, &expected_text)
    }

    fn reset_modified_if_buffer_matches_text(
        &mut self,
        doc_id: DocumentId,
        view_id: ViewId,
        path: &Path,
        expected_text: &Rope,
    ) -> bool {
        let Some(doc) = self.editor.document(doc_id) else {
            return false;
        };
        if doc.path().is_none_or(|doc_path| doc_path != path) {
            return false;
        }
        if doc.text() != expected_text {
            return false;
        }

        let tree = &mut self.editor.tree;
        let documents = &mut self.editor.documents;
        let view = tree.get_mut(view_id);
        let Some(doc) = documents.get_mut(&doc_id) else {
            return false;
        };
        doc.append_changes_to_history(view);
        doc.reset_modified();
        true
    }

    pub fn handle_input_event(
        &mut self,
        event: InputEvent,
        cx: &mut gpui::Context<crate::Core>,
        handle: tokio::runtime::Handle,
    ) {
        let _guard = handle.enter();
        match event {
            InputEvent::Semantic(action) => {
                let outcome = self.editor_input.handle_semantic_action(
                    action,
                    &mut self.compositor,
                    &mut self.editor,
                    &mut self.jobs,
                );
                if outcome.selection_changed
                    && let Some(doc_id) = outcome.focused_doc_id
                {
                    cx.emit(crate::Update::SelectionChanged {
                        doc_id,
                        view_id: outcome.focused_view_id,
                    });
                }
                if let Some(request) = outcome.prompt_requested {
                    cx.emit(crate::Update::Prompt(Self::create_native_prompt(request)));
                }
                self.emit_overlays(cx);
                cx.emit(crate::Update::Redraw);
            }
            InputEvent::Key(input) => {
                let key = input.key;
                let mode_before = self.editor.mode();
                let is_navigation_repeat =
                    input.is_held && is_navigation_repeat_key(&key, mode_before);
                let _timer = PerfTimer::new(if input.is_held {
                    "application_handle_held_key"
                } else {
                    "application_handle_key"
                })
                .with_warn_threshold(std::time::Duration::from_millis(8));
                nucleotide_logging::trace!(
                    key = ?key,
                    is_held = input.is_held,
                    is_navigation_repeat,
                    "Handling key event in Application"
                );

                let outcome = self.editor_input.handle_key(
                    key,
                    &mut self.compositor,
                    &mut self.editor,
                    &mut self.jobs,
                );
                let has_explicit_ui_request = outcome.completion_requested.is_some()
                    || outcome.picker_requested.is_some()
                    || outcome.prompt_requested.is_some()
                    || outcome.lsp_navigation_requested.is_some()
                    || outcome.workspace_requested.is_some()
                    || outcome.semantic_shortcut_requested.is_some();
                let selection_or_viewport_updated = outcome.selection_changed
                    || outcome.viewport_scroll_requested.is_some()
                    || outcome.viewport_cursor_requested.is_some();

                if outcome.reset_diff_change_executed {
                    self.reconcile_vcs_after_diff_reset(cx, &handle);
                }

                if outcome.selection_changed
                    && let Some(doc_id) = outcome.focused_doc_id
                {
                    cx.emit(crate::Update::SelectionChanged {
                        doc_id,
                        view_id: outcome.focused_view_id,
                    });
                }

                if let Some(request) = outcome.completion_requested {
                    if let Some(doc) = self.editor.document(request.doc_id) {
                        let cursor = doc
                            .selection(request.view_id)
                            .primary()
                            .cursor(doc.text().slice(..));
                        cx.emit(crate::Update::CompletionEvent(
                            helix_view::handlers::completion::CompletionEvent::ManualTrigger {
                                cursor,
                                doc: request.doc_id,
                                view: request.view_id,
                            },
                        ));
                    } else {
                        nucleotide_logging::warn!(
                            doc_id = ?request.doc_id,
                            view_id = ?request.view_id,
                            "Document not found for native completion request"
                        );
                    }
                }

                if let Some(request) = outcome.lsp_navigation_requested {
                    self.trigger_lsp_navigation(request, cx);
                }

                if let Some(request) = outcome.workspace_requested {
                    match request {
                        editor_input::NativeWorkspaceRequest::ToggleFileTree => {
                            cx.emit(crate::Update::ToggleFileTree);
                        }
                        editor_input::NativeWorkspaceRequest::OpenFiles { paths, action } => {
                            for path in paths {
                                self.handle_native_open_file_request(
                                    path,
                                    action.into(),
                                    handle.clone(),
                                    cx,
                                );
                            }
                        }
                    }
                }

                if let Some(request) = outcome.semantic_shortcut_requested {
                    cx.emit(crate::Update::SemanticShortcut(request));
                }

                if let Some(request) = outcome.viewport_scroll_requested {
                    cx.emit(crate::Update::ViewportScroll {
                        view_id: outcome.focused_view_id,
                        request,
                    });
                }

                if let Some(request) = outcome.viewport_cursor_requested {
                    cx.emit(crate::Update::ViewportCursor {
                        view_id: outcome.focused_view_id,
                        request,
                    });
                }

                if let Some(request) = outcome.prompt_requested {
                    cx.emit(crate::Update::Prompt(Self::create_native_prompt(request)));
                }

                if let Some(request) = outcome.picker_requested {
                    match request {
                        editor_input::NativePickerRequest::File => {
                            cx.emit(crate::Update::ShowFilePicker);
                        }
                        editor_input::NativePickerRequest::FileAt(path) => {
                            let workspace_backend = self.workspace_backend.clone();
                            let workspace_identity = workspace_backend.identity();
                            if should_stat_picker_root_with_backend(&workspace_identity) {
                                self.editor.set_status(format!(
                                    "Checking file picker path: {}",
                                    path.display()
                                ));
                                let runtime_handle = handle.clone();
                                cx.spawn(async move |core, cx| {
                                    let path_for_stat = path.clone();
                                    let path_exists = match runtime_handle
                                        .spawn(async move {
                                            workspace_backend.stat(&path_for_stat).await.is_ok()
                                        })
                                        .await
                                    {
                                        Ok(path_exists) => path_exists,
                                        Err(error) => {
                                            warn!(
                                                error = %error,
                                                "File picker path stat task failed"
                                            );
                                            false
                                        }
                                    };

                                    if let Some(core) = core.upgrade() {
                                        core.update(cx, |core, cx| {
                                            if path_exists {
                                                cx.emit(crate::Update::ShowFilePickerAt(path));
                                            } else {
                                                core.editor.set_error(format!(
                                                    "File picker path does not exist: {}",
                                                    path.display()
                                                ));
                                            }
                                        });
                                    }
                                })
                                .detach();
                                return;
                            }

                            if path.exists() {
                                cx.emit(crate::Update::ShowFilePickerAt(path));
                            } else {
                                self.editor.set_error(format!(
                                    "File picker path does not exist: {}",
                                    path.display()
                                ));
                            }
                        }
                        editor_input::NativePickerRequest::FileCurrentDirectory => {
                            let cwd = helix_stdx::env::current_working_dir();
                            let workspace_identity = self.workspace_backend.identity();
                            if let Some(directory) = file_picker_current_directory(
                                &workspace_identity,
                                self.project_directory.as_deref(),
                                cwd,
                            ) {
                                cx.emit(crate::Update::ShowFilePickerAt(directory));
                            } else {
                                self.editor
                                    .set_error("Current working directory does not exist");
                            }
                        }
                        editor_input::NativePickerRequest::FileCurrentBufferDirectory => {
                            let current_buffer_directory = self
                                .editor
                                .tree
                                .try_get(self.editor.tree.focus)
                                .and_then(|view| self.editor.document(view.doc))
                                .and_then(|doc| doc.path())
                                .and_then(|path| path.parent())
                                .map(Path::to_path_buf);

                            if let Some(path) = current_buffer_directory {
                                cx.emit(crate::Update::ShowFilePickerAt(path));
                            } else {
                                self.editor
                                    .set_error("current buffer has no path or parent");
                            }
                        }
                        editor_input::NativePickerRequest::Buffer => {
                            cx.emit(crate::Update::ShowBufferPicker);
                        }
                        editor_input::NativePickerRequest::JumpList => {
                            if let Some(picker) = self.create_jumplist_picker() {
                                cx.emit(crate::Update::Picker(picker));
                            } else {
                                self.editor.set_status("Jumplist is empty");
                            }
                        }
                        editor_input::NativePickerRequest::Symbols { workspace } => {
                            self.trigger_lsp_symbol_picker(workspace, cx);
                        }
                        editor_input::NativePickerRequest::Diagnostics { workspace } => {
                            self.emit_diagnostics_picker(workspace, cx);
                        }
                        editor_input::NativePickerRequest::CodeActions => {
                            cx.emit(crate::Update::ShowCodeActions);
                        }
                        editor_input::NativePickerRequest::HoverDocs => {
                            cx.emit(crate::Update::ShowHoverDocs);
                        }
                    }
                }

                if !is_navigation_repeat || has_explicit_ui_request {
                    self.emit_overlays(cx);
                }

                if !is_navigation_repeat || !selection_or_viewport_updated {
                    cx.emit(crate::Update::Redraw);
                }
            }
        }
    }

    fn emit_diagnostics_picker(&mut self, workspace: bool, cx: &mut gpui::Context<crate::Core>) {
        debug!(workspace = workspace, "DIAG: Diagnostics picker requested");

        let focused_doc_id = self
            .editor
            .tree
            .try_get(self.editor.tree.focus)
            .map(|view| view.doc);
        let mut items = Vec::new();

        for (doc_id, doc) in self.editor.documents.iter() {
            if !workspace && Some(*doc_id) != focused_doc_id {
                continue;
            }

            let path = doc.path().map(std::path::Path::to_path_buf);
            let path_label =
                diagnostic_picker_path_label(path.as_deref(), self.project_directory.as_deref());
            let text = doc.text();

            for diagnostic in doc.diagnostics() {
                let offset = diagnostic.range.start.min(text.len_chars());
                let severity = diagnostic.severity();
                let severity_label = diagnostic_severity_label(severity);
                let Position { row, col } = helix_core::coords_at_pos(text.slice(..), offset);
                let line = row + 1;
                let column = col + 1;
                let code = diagnostic.code.as_ref().map(|code| match code {
                    helix_core::diagnostic::NumberOrString::Number(code) => code.to_string(),
                    helix_core::diagnostic::NumberOrString::String(code) => code.clone(),
                });
                let label = format!(
                    "{severity_label} {path_label}:{line}:{column} {}",
                    diagnostic.message
                );
                let data = crate::types::DiagnosticLocation {
                    doc_id: *doc_id,
                    path: path.clone(),
                    offset,
                };

                items.push(crate::picker_view::PickerItem {
                    label: label.into(),
                    sublabel: None,
                    data: Arc::new(data),
                    file_path: path.clone(),
                    vcs_status: None,
                    columns: Some(crate::picker_view::ColumnData::Diagnostic {
                        severity,
                        icon_path: nucleotide_editor::diagnostic_severity_icon_path(severity)
                            .to_string(),
                        path: path_label.clone(),
                        line,
                        column,
                        message: diagnostic.message.clone(),
                        source: diagnostic.source.clone(),
                        code,
                        tags: diagnostic.tags.clone(),
                    }),
                });
            }
        }

        if items.is_empty() {
            self.editor.set_status(if workspace {
                "No workspace diagnostics"
            } else {
                "No diagnostics"
            });
        } else {
            let title = if workspace {
                "Workspace Diagnostics"
            } else {
                "Diagnostics"
            };
            let picker = crate::picker::Picker::native(title, items, |_index| {
                // Selection is handled by OverlayView via DiagnosticLocation payloads.
            });
            cx.emit(crate::Update::Picker(picker));
        }
    }

    fn handle_document_write(&mut self, doc_save_event: &DocumentSavedEventResult) {
        let doc_save_event = match doc_save_event {
            Ok(event) => event,
            Err(err) => {
                self.editor.set_error(err.to_string());
                return;
            }
        };

        let doc = match self.editor.document_mut(doc_save_event.doc_id) {
            None => {
                warn!(
                    "received document saved event for non-existent doc id: {}",
                    doc_save_event.doc_id
                );

                return;
            }
            Some(doc) => doc,
        };

        debug!(
            "document {:?} saved with revision {}",
            doc.path(),
            doc_save_event.revision
        );

        doc.set_last_saved_revision(doc_save_event.revision, doc_save_event.save_time);

        let lines = doc_save_event.text.len_lines();
        let bytes = doc_save_event.text.len_bytes();

        self.editor
            .set_doc_path(doc_save_event.doc_id, &doc_save_event.path);
        if let Some(doc) = self.editor.document_mut(doc_save_event.doc_id) {
            doc.set_file_version(doc_save_event.file_version.clone());
        }
        self.editor.set_status(format!(
            "'{}' written, {}L {}B",
            get_relative_path(&doc_save_event.path).to_string_lossy(),
            lines,
            bytes
        ));
    }

    fn cleanup_zombie_lsp_progress(&mut self) {
        use helix_lsp::lsp;

        let server_ids: Vec<LanguageServerId> = self
            .editor
            .language_servers
            .iter_clients()
            .map(|client| client.id())
            .collect();

        let mut zombie_tokens: Vec<(LanguageServerId, lsp::ProgressToken)> = Vec::new();

        for server_id in server_ids {
            if let Some(progress_map) = self.lsp_progress.progress_map(server_id) {
                if progress_map.is_empty() {
                    continue; // Skip servers with no progress
                }

                let server_name = self
                    .editor
                    .language_servers
                    .iter_clients()
                    .find(|client| client.id() == server_id)
                    .map(|client| client.name())
                    .unwrap_or("unknown");

                debug!(
                    server_id = ?server_id,
                    server_name = %server_name,
                    progress_count = progress_map.len(),
                    "🧟 ZOMBIE: Checking server progress"
                );

                for (token, status) in progress_map {
                    match status {
                        helix_lsp::ProgressStatus::Created => {
                            debug!(
                                server_id = ?server_id,
                                token = ?token,
                                "🧟 ZOMBIE: Found Created status"
                            );
                        }
                        helix_lsp::ProgressStatus::Started { title, progress } => {
                            // Check the actual progress message for "(100%)"
                            let message = match progress {
                                lsp::WorkDoneProgress::Begin(begin) => &begin.message,
                                lsp::WorkDoneProgress::Report(report) => &report.message,
                                lsp::WorkDoneProgress::End(end) => &end.message,
                            };

                            debug!(
                                server_id = ?server_id,
                                token = ?token,
                                title = %title,
                                message = ?message,
                                progress_type = match progress {
                                    lsp::WorkDoneProgress::Begin(_) => "Begin",
                                    lsp::WorkDoneProgress::Report(_) => "Report",
                                    lsp::WorkDoneProgress::End(_) => "End",
                                },
                                "🧟 ZOMBIE: Found Started status"
                            );

                            if let Some(msg) = message {
                                // Check for various zombie patterns:
                                // 1. "(100%)" - explicit percentage
                                // 2. "X/X" - where both numbers are equal (e.g., "637/637")
                                let is_zombie = msg.contains("(100%)") || {
                                    // Check for "number/number" pattern where both are equal
                                    if let Some(slash_pos) = msg.find('/') {
                                        let before = &msg[..slash_pos];
                                        let after = &msg[slash_pos + 1..];

                                        // Try to parse both sides as numbers
                                        if let (Ok(num1), Ok(num2)) =
                                            (before.parse::<u32>(), after.parse::<u32>())
                                        {
                                            num1 > 0 && num1 == num2
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    }
                                };

                                if is_zombie {
                                    info!(
                                        server_id = ?server_id,
                                        token = ?token,
                                        message = %msg,
                                        "🧟 ZOMBIE: Found zombie token!"
                                    );
                                    zombie_tokens.push((server_id, token.clone()));
                                }
                            }
                        }
                    }
                }
            } else {
                let server_name = self
                    .editor
                    .language_servers
                    .iter_clients()
                    .find(|client| client.id() == server_id)
                    .map(|client| client.name())
                    .unwrap_or("unknown");
                debug!(
                    server_id = ?server_id,
                    server_name = %server_name,
                    "🧟 ZOMBIE: No progress map found for server"
                );
            }
        }

        // Force end any zombie operations we found
        for (server_id, token) in zombie_tokens {
            let server_name = self
                .editor
                .language_servers
                .iter_clients()
                .find(|client| client.id() == server_id)
                .map(|client| client.name())
                .unwrap_or("unknown");

            // Expected housekeeping for servers that report 100% without an `end` message
            // (rust-analyzer does this routinely); not worth a warning on every occurrence.
            debug!(
                server_id = ?server_id,
                server_name = %server_name,
                token = ?token,
                "Force-ending zombie progress operation at 100%"
            );

            self.lsp_progress.end_progress(server_id, &token);
        }
    }

    pub fn drive_event_driven_maintenance(
        &mut self,
        cx: &mut gpui::Context<crate::Core>,
        handle: tokio::runtime::Handle,
        wake: &MaintenanceWake,
    ) -> bool {
        let _timer = PerfTimer::new("Application::drive_event_driven_maintenance")
            .with_warn_threshold(MAINTENANCE_DRAIN_WARN_THRESHOLD);
        let _guard = handle.enter();
        let waker = wake.waker();
        let mut task_cx = TaskContext::from_waker(&waker);
        let mut made_progress = false;
        let turn_started = Instant::now();
        let mut iterations = 0usize;
        let mut yielded_for_budget = false;

        loop {
            iterations += 1;
            let _iteration_timer =
                PerfTimer::new("Application::drive_event_driven_maintenance.iteration")
                    .with_warn_threshold(MAINTENANCE_ITERATION_WARN_THRESHOLD);
            let mut progressed = false;
            progressed |= {
                let _timer = PerfTimer::new("Application::poll_pending_helix_jobs")
                    .with_warn_threshold(MAINTENANCE_POLLER_WARN_THRESHOLD);
                self.poll_pending_helix_jobs(cx, &mut task_cx)
            };

            if progressed && turn_started.elapsed() >= MAINTENANCE_TURN_BUDGET {
                made_progress = true;
                yielded_for_budget = true;
                break;
            }

            progressed |= {
                let _timer = PerfTimer::new("Application::poll_ready_editor_events")
                    .with_warn_threshold(MAINTENANCE_POLLER_WARN_THRESHOLD);
                self.poll_ready_editor_events(cx, &mut task_cx)
            };

            progressed |= {
                let _timer = PerfTimer::new("Application::poll_pending_lsp_workspace_edits")
                    .with_warn_threshold(MAINTENANCE_POLLER_WARN_THRESHOLD);
                self.poll_pending_lsp_workspace_edits(&mut task_cx)
            };

            if progressed && turn_started.elapsed() >= MAINTENANCE_TURN_BUDGET {
                made_progress = true;
                yielded_for_budget = true;
                break;
            }

            progressed |= {
                let _timer = PerfTimer::new("Application::poll_pending_helix_events")
                    .with_warn_threshold(MAINTENANCE_POLLER_WARN_THRESHOLD);
                self.poll_pending_helix_events(cx, &handle, &mut task_cx)
            };

            if progressed && turn_started.elapsed() >= MAINTENANCE_TURN_BUDGET {
                made_progress = true;
                yielded_for_budget = true;
                break;
            }

            progressed |= {
                let _timer = PerfTimer::new("Application::poll_pending_lsp_commands")
                    .with_warn_threshold(MAINTENANCE_POLLER_WARN_THRESHOLD);
                self.poll_pending_lsp_commands(cx, &handle, &mut task_cx)
            };

            if !progressed {
                break;
            }

            made_progress = true;

            if turn_started.elapsed() >= MAINTENANCE_TURN_BUDGET {
                yielded_for_budget = true;
                break;
            }
        }

        if made_progress {
            self.sync_lsp_state(cx);
            self.cleanup_zombie_lsp_progress();
            cx.emit(crate::Update::Redraw);
            cx.notify();
        }

        if yielded_for_budget {
            debug!(
                iterations = iterations,
                elapsed_ms = turn_started.elapsed().as_millis(),
                budget_ms = MAINTENANCE_TURN_BUDGET.as_millis(),
                "Maintenance yielded after exhausting turn budget"
            );
            wake.notify();
        }

        self.editor.tree.views().count() > 0
    }

    fn handle_editor_event_sync(
        &mut self,
        event: helix_view::editor::EditorEvent,
        cx: &mut gpui::Context<crate::Core>,
    ) -> bool {
        use helix_view::editor::EditorEvent;
        use nucleotide_events::document::Event as DocumentEvent;

        debug!(
            event_type = ?std::mem::discriminant(&event),
            "MAINTENANCE: Processing ready editor event"
        );

        match event {
            EditorEvent::DocumentSaved(event) => {
                self.handle_document_write(&event);
                if let Ok(event) = event {
                    let v2_event = DocumentEvent::Saved {
                        doc_id: event.doc_id,
                        path: event.path.clone(),
                        revision: event.revision as u64,
                    };
                    cx.emit(crate::Update::Document(v2_event));
                }
            }
            EditorEvent::IdleTimer => {
                self.editor.clear_idle_timer();
            }
            EditorEvent::Redraw => {
                if self.editor.tree.views().count() == 0 {
                    cx.emit(crate::Update::ShouldQuit);
                    return false;
                }
                cx.emit(crate::Update::Redraw);
            }
            EditorEvent::ConfigEvent(config_event) => {
                self.handle_config_event(config_event, cx);
            }
            EditorEvent::LanguageServerMessage((id, call)) => {
                debug!(
                    server_id = ?id,
                    call_type = ?std::mem::discriminant(&call),
                    "Received EditorEvent::LanguageServerMessage"
                );
                self.handle_language_server_message(call, id);
                self.sync_lsp_state(cx);
                cx.emit(crate::Update::Redraw);
            }
            EditorEvent::DebuggerEvent(event) => {
                debug!(
                    event = ?event,
                    "Received debugger event; debugger integration is not active"
                );
            }
        }

        true
    }

    fn poll_ready_editor_events(
        &mut self,
        cx: &mut gpui::Context<crate::Core>,
        task_cx: &mut TaskContext<'_>,
    ) -> bool {
        let mut progressed = false;

        loop {
            let poll = {
                let future = self.editor.wait_event();
                tokio::pin!(future);
                Future::poll(Pin::as_mut(&mut future), task_cx)
            };

            match poll {
                Poll::Ready(event) => {
                    progressed = true;
                    if !self.handle_editor_event_sync(event, cx) {
                        break;
                    }
                }
                Poll::Pending => break,
            }
        }

        progressed
    }

    pub(crate) fn apply_reloaded_config(
        &mut self,
        new_config: crate::config::Config,
        cx: &mut gpui::Context<Self>,
    ) {
        let old_config = self.editor.config();
        debug!("Old bufferline config: {:?}", old_config.bufferline);

        self.config = new_config;
        let mut updated_helix_config = self.config.to_helix_config();
        // Nucleotide always runs Helix in GUI true-colour mode. Preserve the
        // startup invariant when replacing the runtime config arc.
        updated_helix_config.editor.true_color = true;
        self.config.helix.editor.true_color = true;

        debug!(
            "Updated helix config bufferline: {:?}",
            updated_helix_config.editor.bufferline
        );
        self.helix_config_arc.store(Arc::new(updated_helix_config));
        let native_keys = Box::new(Map::new(
            Arc::clone(&self.helix_config_arc),
            |config: &Config| &config.keys,
        ));
        self.editor_input = EditorInputBridge::new(Keymaps::new(native_keys));

        self.editor.refresh_config(&old_config);
        debug!(
            "After refresh_config, editor bufferline: {:?}",
            self.editor.config().bufferline
        );

        for client in self.editor.language_servers.iter_clients() {
            let cfg = client.config();
            if let Some(cfg) = cfg {
                debug!(server = %client.name(), "Re-sending LSP didChangeConfiguration with current settings");
                client.did_change_configuration(cfg.clone());
            }
        }

        cx.emit(crate::Update::Redraw);
    }

    fn handle_config_event(
        &mut self,
        config_event: helix_view::editor::ConfigEvent,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        debug!("Application received ConfigEvent: {:?}", config_event);

        match &config_event {
            helix_view::editor::ConfigEvent::Update(new_editor_config) => {
                debug!(
                    "New bufferline config in Update event: {:?}",
                    new_editor_config.bufferline
                );
                let mut new_config = self.config.clone();
                new_config.apply_helix_config_update(new_editor_config);
                self.apply_reloaded_config(new_config, cx);

                info!("Config updated via generic patching system");
            }
            helix_view::editor::ConfigEvent::Refresh => {
                info!("Config refresh requested - reloading from files");
                match crate::config::Config::load() {
                    Ok(fresh_config) => self.apply_reloaded_config(fresh_config, cx),
                    Err(error) => {
                        error!(%error, "Failed to refresh config from files");
                        self.editor.set_error(error.to_string());
                    }
                }
            }
            helix_view::editor::ConfigEvent::ThemeChanged => {
                cx.emit(crate::Update::Redraw);
            }
        }

        debug!("Forwarding ConfigEvent to workspace");
        cx.emit(crate::Update::EditorConfigChanged(config_event));
    }

    pub async fn step(&mut self, cx: &mut gpui::Context<'_, crate::Core>) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            warn!("Cannot run application step without Tokio runtime");
            return;
        };

        let wake = self
            .maintenance_wake
            .clone()
            .unwrap_or_else(|| MaintenanceWake::channel().0);
        self.drive_event_driven_maintenance(cx, handle, &wake);
    }

    // Removed unused handle_language_server_message - now handled via events
}

// Helper methods to improve function shape and centralize control flow
impl Application {
    /// Return active language servers and their names.
    fn active_servers(&self) -> Vec<(helix_lsp::LanguageServerId, String)> {
        self.editor
            .language_servers
            .iter_clients()
            .map(|client| (client.id(), client.name().to_string()))
            .collect()
    }

    /// Return the subset of active servers that are currently progressing.
    fn progressing_servers(
        &self,
        active: &[(helix_lsp::LanguageServerId, String)],
    ) -> Vec<helix_lsp::LanguageServerId> {
        active
            .iter()
            .filter(|(id, _)| self.lsp_progress.is_progressing(*id))
            .map(|(id, _)| *id)
            .collect()
    }

    /// Compute the progress entries (key, LspProgress) for UI from active/progressing servers.
    fn compute_progress_entries(
        &self,
        active: &[(helix_lsp::LanguageServerId, String)],
        progressing: &[helix_lsp::LanguageServerId],
    ) -> Vec<(String, nucleotide_lsp::LspProgress)> {
        use helix_lsp::lsp::NumberOrString;

        let editor_status = self.editor.get_status();

        let message_for_server = |server_id: helix_lsp::LanguageServerId| -> String {
            let current_progress = self.lsp_progress.progress_map(server_id);
            let active_token_count = current_progress.map(|p| p.len()).unwrap_or(0);

            if active_token_count > 0 {
                if let Some(progress_map) = current_progress {
                    let pending_work: Vec<(String, Option<String>, Option<u32>, String)> =
                        progress_map
                            .iter()
                            .filter_map(|(token, status)| match status {
                                helix_lsp::ProgressStatus::Started { title, progress } => {
                                    let (message, percentage) = match progress {
                                        helix_lsp::lsp::WorkDoneProgress::Begin(begin) => {
                                            (begin.message.clone(), begin.percentage)
                                        }
                                        helix_lsp::lsp::WorkDoneProgress::Report(report) => {
                                            (report.message.clone(), report.percentage)
                                        }
                                        helix_lsp::lsp::WorkDoneProgress::End(end) => {
                                            (end.message.clone(), None)
                                        }
                                    };
                                    let token_str = match token {
                                        NumberOrString::Number(n) => n.to_string(),
                                        NumberOrString::String(s) => s.clone(),
                                    };
                                    Some((title.clone(), message, percentage, token_str))
                                }
                                _ => None,
                            })
                            .collect();

                    if let Some((title, message, percentage, token)) = pending_work.first() {
                        let additional_work_count = pending_work.len().saturating_sub(1);
                        let mut formatted = self.format_lsp_progress_message(
                            Some(title.as_str()),
                            message.as_deref(),
                            *percentage,
                            token.as_str(),
                        );
                        if additional_work_count > 0 {
                            formatted.push_str(&format!(" + {} more", additional_work_count));
                        }
                        formatted
                    } else {
                        "Indexing".to_string()
                    }
                } else {
                    "Indexing".to_string()
                }
            } else {
                editor_status
                    .as_ref()
                    .filter(|(msg, _)| !msg.is_empty())
                    .map(|(msg, _)| msg.to_string())
                    .unwrap_or_else(|| "Ready".to_string())
            }
        };

        let mut out = Vec::with_capacity(progressing.len());
        for (server_id, _name) in active {
            if !progressing.contains(server_id) {
                continue;
            }
            let message = message_for_server(*server_id);
            let key = format!("{}-{}", server_id, LSP_TOKEN_ACTIVITY);

            let progress = nucleotide_lsp::LspProgress {
                server_id: *server_id,
                token: LSP_TOKEN_ACTIVITY.to_string(),
                title: "Processing".to_string(),
                message: Some(message),
                percentage: None,
            };
            out.push((key, progress));
        }

        out
    }

    // Note: Terminal panel is managed by Workspace via its overlay; no app-level show method.
}

impl editor_input::NativeLspNavigationRequest {
    fn language_server_feature(self) -> syntax::config::LanguageServerFeature {
        match self {
            Self::GotoDeclaration => syntax::config::LanguageServerFeature::GotoDeclaration,
            Self::GotoDefinition => syntax::config::LanguageServerFeature::GotoDefinition,
            Self::GotoTypeDefinition => syntax::config::LanguageServerFeature::GotoTypeDefinition,
            Self::GotoImplementation => syntax::config::LanguageServerFeature::GotoImplementation,
            Self::GotoReference => syntax::config::LanguageServerFeature::GotoReference,
        }
    }

    fn picker_title(self) -> &'static str {
        match self {
            Self::GotoDeclaration => "Declarations",
            Self::GotoDefinition => "Definitions",
            Self::GotoTypeDefinition => "Type Definitions",
            Self::GotoImplementation => "Implementations",
            Self::GotoReference => "References",
        }
    }

    fn unsupported_message(self) -> &'static str {
        match self {
            Self::GotoDeclaration => "No configured language server supports goto declaration",
            Self::GotoDefinition => "No configured language server supports goto definition",
            Self::GotoTypeDefinition => {
                "No configured language server supports goto type definition"
            }
            Self::GotoImplementation => {
                "No configured language server supports goto implementation"
            }
            Self::GotoReference => "No configured language server supports goto reference",
        }
    }

    fn empty_message(self) -> &'static str {
        match self {
            Self::GotoDeclaration => "No declaration found.",
            Self::GotoDefinition => "No definition found.",
            Self::GotoTypeDefinition => "No type definition found.",
            Self::GotoImplementation => "No implementation found.",
            Self::GotoReference => "No references found.",
        }
    }
}

fn lsp_locations_from_definition_response(
    response: Option<lsp::GotoDefinitionResponse>,
    offset_encoding: OffsetEncoding,
) -> Vec<crate::types::LspLocation> {
    match response {
        Some(lsp::GotoDefinitionResponse::Scalar(location)) => {
            lsp_location_from_location(location, offset_encoding)
                .into_iter()
                .collect()
        }
        Some(lsp::GotoDefinitionResponse::Array(locations)) => locations
            .into_iter()
            .filter_map(|location| lsp_location_from_location(location, offset_encoding))
            .collect(),
        Some(lsp::GotoDefinitionResponse::Link(location_links)) => location_links
            .into_iter()
            .filter_map(|location_link| {
                lsp_location_from_location(
                    lsp::Location::new(location_link.target_uri, location_link.target_range),
                    offset_encoding,
                )
            })
            .collect(),
        None => Vec::new(),
    }
}

type LspLocationFuture = BoxFuture<'static, anyhow::Result<Vec<crate::types::LspLocation>>>;

const REMOTE_LSP_NAVIGATION_READY_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_LSP_NAVIGATION_READY_POLL_INTERVAL: Duration = Duration::from_millis(50);

fn lsp_location_from_location(
    location: lsp::Location,
    offset_encoding: OffsetEncoding,
) -> Option<crate::types::LspLocation> {
    let path = match lsp_location_path_from_url(location.uri) {
        Ok(path) => path,
        Err(err) => {
            warn!(error = %err, "Discarding LSP location with unsupported URI");
            return None;
        }
    };

    Some(crate::types::LspLocation {
        path,
        range: location.range,
        offset_encoding,
    })
}

fn lsp_location_path_from_url(uri: lsp::Url) -> Result<PathBuf, String> {
    if let Some(path) = remote_lsp_file_url_path(&uri) {
        return Ok(path);
    }

    match Uri::try_from(uri.clone()) {
        Ok(uri) => uri
            .as_path()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("URI does not contain a file path: {uri}")),
        Err(error) => remote_lsp_file_url_path(&uri).ok_or_else(|| error.to_string()),
    }
}

fn lsp_locations_picker(
    title: &str,
    locations: Vec<crate::types::LspLocation>,
    project_directory: Option<&Path>,
) -> crate::picker::Picker {
    use crate::picker_view::PickerItem;

    let items = locations
        .into_iter()
        .map(|location| {
            let line = location.range.start.line + 1;
            let character = location.range.start.character + 1;
            let file_name = location
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| location.path.display().to_string());
            let label = format!("{file_name}:{line}:{character}");
            let path_label = project_directory
                .and_then(|root| location.path.strip_prefix(root).ok())
                .unwrap_or(&location.path)
                .display()
                .to_string();

            PickerItem::with_sublabel_and_path(
                label,
                path_label,
                location.path.clone(),
                Arc::new(location),
            )
        })
        .collect();

    crate::picker::Picker::native(title.to_string(), items, |_index| {
        // LSP location selection is handled by the overlay via typed item data.
    })
}

fn diagnostic_picker_path_label(path: Option<&Path>, project_directory: Option<&Path>) -> String {
    path.map(|path| {
        if let Some(relative_path) = project_directory.and_then(|root| path.strip_prefix(root).ok())
        {
            relative_path.display().to_string()
        } else {
            get_relative_path(path).display().to_string()
        }
    })
    .unwrap_or_else(|| "[scratch]".to_string())
}

fn diagnostic_severity_label(severity: helix_core::diagnostic::Severity) -> &'static str {
    match severity {
        helix_core::diagnostic::Severity::Error => "error",
        helix_core::diagnostic::Severity::Warning => "warning",
        helix_core::diagnostic::Severity::Info => "info",
        helix_core::diagnostic::Severity::Hint => "hint",
    }
}

impl Application {
    /// Get the project LSP command sender for external coordination
    pub fn get_project_lsp_command_sender(
        &self,
    ) -> tokio::sync::mpsc::UnboundedSender<nucleotide_events::ProjectLspCommand> {
        self.project_lsp.command_sender()
    }

    fn helix_lsp_bridge_handle(&self) -> Option<HelixLspBridge> {
        self.project_lsp.bridge()
    }

    /// Initialize the sole adapter to Helix's language-server registry.
    pub fn initialize_project_lsp_system(&mut self, _handle: &tokio::runtime::Handle) {
        if self.project_lsp.is_initialized() {
            debug!("Project LSP system already initialized, skipping");
            return;
        }

        info!("Initializing project LSP system");

        // Create HelixLspBridge with environment provider
        let env_provider = Arc::new(ProjectEnvironmentProvider::with_workspace_backend(
            self.project_environment.clone(),
            self.workspace_backend.clone(),
        ));
        let launch_proxy_provider =
            Arc::new(RemoteLspLaunchProxyProvider::from_config(&self.config));
        let helix_bridge = nucleotide_lsp::HelixLspBridge::new_with_environment_and_launch_proxy(
            env_provider.clone(),
            launch_proxy_provider,
        );
        self.project_lsp.initialize(helix_bridge, env_provider);

        if let Some(wake) = &self.maintenance_wake {
            wake.notify();
        }
        info!("Project LSP system initialized; Helix owns server lifecycle");
    }

    /// Stop the active Helix project session and release its bridge.
    pub async fn cleanup_project_lsp_system(
        &mut self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Cleaning up project LSP system");

        if let Some(workspace_root) = self.project_lsp.take_active_root()
            && let Some(bridge) = self.helix_lsp_bridge_handle()
        {
            let stopped = bridge.stop_workspace_servers(&mut self.editor, &workspace_root);
            for client in stopped {
                let _ = tokio::time::timeout(
                    Duration::from_millis(500),
                    client.wait_shutdown_flushed(),
                )
                .await;
            }
        }
        self.lsp_progress.clear();

        self.project_lsp.clear_system();

        info!("Project LSP system cleanup completed");
        Ok(())
    }

    fn start_lsp_server_prepared(
        &mut self,
        workspace_root: &Path,
        server_name: &str,
        language_id: &str,
        environment: Result<Option<HashMap<String, String>>, String>,
    ) -> Result<nucleotide_events::ServerStartResult, ProjectLspCommandError> {
        use nucleotide_events::ServerStartResult;

        let bridge = self.helix_lsp_bridge_handle().ok_or_else(|| {
            ProjectLspCommandError::Internal("HelixLspBridge not initialized".to_string())
        })?;

        match bridge.start_server_prepared(
            &mut self.editor,
            workspace_root,
            server_name,
            language_id,
            environment,
        ) {
            Ok(server_id) => {
                self.notify_language_server_stream_added();
                info!(
                    server_id = ?server_id,
                    server_name = %server_name,
                    language_id = %language_id,
                    "Language server process started; awaiting initialization"
                );

                // Ensure currently open documents of this language are tracked by the server
                let target_lang = language_id.to_ascii_lowercase();
                let mut docs_to_track: Vec<helix_view::DocumentId> = Vec::new();
                for (view, _focused) in self.editor.tree.views() {
                    let doc_id = view.doc;
                    if let Some(doc) = self.editor.document(doc_id) {
                        if !doc
                            .path()
                            .is_some_and(|path| path.starts_with(workspace_root))
                        {
                            continue;
                        }
                        let doc_lang: String = doc
                            .language_id()
                            .map(|s| s.to_ascii_lowercase())
                            .or_else(|| doc.language_name().map(|s| s.to_ascii_lowercase()))
                            .unwrap_or_default();
                        if doc_lang == target_lang {
                            docs_to_track.push(doc_id);
                        }
                    }
                }
                for doc_id in docs_to_track {
                    if let Err(e) =
                        bridge.ensure_document_tracked(&mut self.editor, server_id, doc_id)
                    {
                        nucleotide_logging::warn!(
                            error = %e,
                            doc_id = ?doc_id,
                            server_id = ?server_id,
                            "Failed to ensure document tracking for started server"
                        );
                    } else {
                        nucleotide_logging::info!(
                            doc_id = ?doc_id,
                            server_id = ?server_id,
                            "Ensured document is tracked by started server"
                        );
                    }
                }

                Ok(ServerStartResult {
                    server_id,
                    server_name: server_name.to_string(),
                    language_id: language_id.to_string(),
                })
            }
            Err(e) => {
                error!(
                    error = %e,
                    server_name = %server_name,
                    language_id = %language_id,
                    "Failed to start LSP server"
                );

                Err(ProjectLspCommandError::ServerStartup(format!(
                    "Failed to start {} server: {}",
                    server_name, e
                )))
            }
        }
    }

    pub(crate) fn ensure_document_tracked_by_running_servers(
        &mut self,
        doc_id: DocumentId,
    ) -> usize {
        let workspace_identity = self.workspace_backend.identity();
        let project_directory = self.project_directory.clone();
        let Some((doc_path, doc_url, language_id, configured_server_names)) =
            self.editor.document(doc_id).map(|doc| {
                let configured_server_names = doc
                    .language_config()
                    .map(|config| {
                        config
                            .language_servers
                            .iter()
                            .map(|features| features.name.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                (
                    doc.path().map(std::path::Path::to_path_buf),
                    doc.url().map(|url| url.to_string()),
                    doc.language_id().map(ToOwned::to_owned),
                    configured_server_names,
                )
            })
        else {
            warn!(doc_id = ?doc_id, "Cannot attach running LSP servers; document not found");
            return 0;
        };

        if configured_server_names.is_empty() {
            debug!(
                doc_id = ?doc_id,
                path = ?doc_path,
                language_id = ?language_id,
                "Cannot attach running LSP servers; document has no configured language servers"
            );
            return 0;
        }

        if matches!(workspace_identity, WorkspaceIdentity::Remote(_)) {
            let stale_servers = self
                .editor
                .document(doc_id)
                .map(|doc| {
                    doc.language_servers()
                        .filter(|client| {
                            !remote_lsp_root_uri_matches_document_workspace(
                                &workspace_identity,
                                project_directory.as_deref(),
                                doc.path(),
                                client.root_path(),
                                client.root_uri(),
                            )
                        })
                        .map(|client| {
                            (
                                client.name().to_string(),
                                client.id(),
                                client.root_path().to_path_buf(),
                                client.root_uri().map(|url| url.as_str().to_string()),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            if !stale_servers.is_empty()
                && let Some(doc) = self.editor.document_mut(doc_id)
            {
                let identifier = document_lsp_identifier(doc);
                for (server_name, server_id, root_path, root_uri) in stale_servers {
                    if let Some(language_server) = doc.remove_language_server_by_name(&server_name)
                        && let Some(identifier) = identifier.as_ref()
                    {
                        language_server.text_document_did_close(identifier.clone());
                    }
                    warn!(
                        doc_id = ?doc_id,
                        server_id = ?server_id,
                        server_name = %server_name,
                        path = ?doc_path,
                        url = ?doc_url,
                        client_root = %root_path.display(),
                        client_root_uri = ?root_uri,
                        "Removed stale remote LSP document attachment"
                    );
                }
            }
        }

        let candidates = self
            .editor
            .language_servers
            .iter_clients()
            .filter(|client| {
                client.is_initialized()
                    && configured_server_names
                        .iter()
                        .any(|server_name| server_name == client.name())
                    && remote_lsp_root_uri_matches_document_workspace(
                        &workspace_identity,
                        project_directory.as_deref(),
                        doc_path.as_deref(),
                        client.root_path(),
                        client.root_uri(),
                    )
            })
            .map(|client| {
                (
                    client.id(),
                    client.name().to_string(),
                    client.root_path().to_path_buf(),
                    client.root_uri().map(|url| url.as_str().to_string()),
                )
            })
            .collect::<Vec<_>>();

        if candidates.is_empty() {
            debug!(
                doc_id = ?doc_id,
                path = ?doc_path,
                language_id = ?language_id,
                configured_servers = ?configured_server_names,
                project_directory = ?project_directory,
                "No initialized running LSP servers match document configuration"
            );
            return 0;
        }

        let mut tracked_count = 0;
        for (server_id, server_name, root_path, root_uri) in candidates {
            if self
                .editor
                .ensure_document_tracked_by_language_server(doc_id, server_id)
            {
                tracked_count += 1;
                info!(
                    doc_id = ?doc_id,
                    server_id = ?server_id,
                    server_name = %server_name,
                    path = ?doc_path,
                    url = ?doc_url,
                    client_root = %root_path.display(),
                    client_root_uri = ?root_uri,
                    "Ensured document is tracked by running LSP server"
                );
            } else {
                warn!(
                    doc_id = ?doc_id,
                    server_id = ?server_id,
                    server_name = %server_name,
                    path = ?doc_path,
                    url = ?doc_url,
                    client_root = %root_path.display(),
                    client_root_uri = ?root_uri,
                    language_id = ?language_id,
                    configured_servers = ?configured_server_names,
                    "Running LSP server could not track document"
                );
            }
        }

        tracked_count
    }

    /// Attach a freshly opened remote document to running servers, or start its configured
    /// servers when none are running. Remote documents open with `launch_language_servers:
    /// false` because Helix cannot spawn the server on the remote side, so this is the
    /// file-based startup path for remote workspaces regardless of `project_lsp_startup`.
    pub(crate) fn attach_or_start_remote_document_servers(
        &mut self,
        doc_id: DocumentId,
        handle: &tokio::runtime::Handle,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        if self.ensure_document_tracked_by_running_servers(doc_id) > 0 {
            return;
        }

        let workspace_identity = self.workspace_backend.identity();
        if !matches!(workspace_identity, WorkspaceIdentity::Remote(_)) {
            return;
        }
        let Some((doc_path, language_id, configured_server_names)) =
            self.editor.document(doc_id).map(|doc| {
                (
                    doc.path().map(Path::to_path_buf),
                    doc.language_id().map(ToOwned::to_owned),
                    doc.language_config()
                        .map(|config| {
                            config
                                .language_servers
                                .iter()
                                .map(|features| features.name.clone())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                )
            })
        else {
            return;
        };
        let Some(language_id) = language_id else {
            return;
        };
        if configured_server_names.is_empty() {
            return;
        }
        let Some(project_root) = remote_lsp_project_root_for_document(
            &workspace_identity,
            self.project_directory.as_deref(),
            doc_path.as_deref(),
        ) else {
            return;
        };
        let workspace_root = canonical_project_lsp_root(&project_root);
        let Some(generation) = self.project_lsp.settled_generation(&workspace_root) else {
            debug!(
                doc_id = ?doc_id,
                workspace_root = %workspace_root.display(),
                "Project session still preparing; it decides which servers to start"
            );
            return;
        };

        // Servers already launched for this root (initialized or not) must not be started twice.
        let launched_server_names = self
            .editor
            .language_servers
            .iter_clients()
            .filter(|client| {
                remote_lsp_root_uri_matches_document_workspace(
                    &workspace_identity,
                    self.project_directory.as_deref(),
                    doc_path.as_deref(),
                    client.root_path(),
                    client.root_uri(),
                )
            })
            .map(|client| client.name().to_string())
            .collect::<HashSet<_>>();
        let planned_servers = configured_server_names
            .into_iter()
            .filter(|server_name| !launched_server_names.contains(server_name))
            .filter(|server_name| {
                self.project_lsp
                    .begin_document_start(generation, &language_id, server_name)
            })
            .map(|server_name| (language_id.clone(), server_name))
            .collect::<Vec<_>>();
        if planned_servers.is_empty() {
            return;
        }
        let Some(bridge) = self.helix_lsp_bridge_handle() else {
            return;
        };

        let server_names = planned_servers
            .iter()
            .map(|(_, server_name)| server_name.clone())
            .collect::<Vec<_>>();
        info!(
            doc_id = ?doc_id,
            path = ?doc_path,
            %language_id,
            servers = ?server_names,
            workspace_root = %workspace_root.display(),
            "Starting language servers for opened remote document"
        );
        self.set_editor_status_feedback(
            cx,
            project_lsp_startup_status(&server_names, true),
            crate::types::Severity::Info,
        );
        if let Some(state) = &self.lsp_state {
            let plan_servers = planned_servers.clone();
            state.update(cx, |state, cx| {
                state.extend_project_server_plan(&plan_servers);
                cx.notify();
            });
        }

        let runtime = handle.clone();
        let timeout = Duration::from_millis(self.config.gui.lsp.startup_timeout_ms);
        cx.spawn(async move |this, cx| {
            let prepare_root = workspace_root.clone();
            let environment = runtime
                .spawn(async move {
                    tokio::time::timeout(timeout, bridge.prepare_server_environment(&prepare_root))
                        .await
                        .map_err(|_| {
                            format!(
                                "environment preparation timed out after {}ms",
                                timeout.as_millis()
                            )
                        })
                        .and_then(|result| result)
                })
                .await
                .unwrap_or_else(|error| Err(format!("environment task failed: {error}")));

            if let Some(this) = this.upgrade() {
                this.update(cx, move |app, cx| {
                    if !app.project_lsp.is_current(generation, &workspace_root) {
                        return;
                    }
                    app.start_planned_project_servers_prepared(
                        &runtime,
                        generation,
                        &workspace_root,
                        &planned_servers,
                        environment,
                        cx,
                    );
                    app.sync_lsp_state(cx);
                    cx.emit(crate::Update::Redraw);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn trigger_lsp_navigation(
        &mut self,
        request: editor_input::NativeLspNavigationRequest,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        self.trigger_lsp_navigation_with_remote_startup(request, true, cx);
    }

    fn trigger_lsp_navigation_with_remote_startup(
        &mut self,
        request: editor_input::NativeLspNavigationRequest,
        allow_remote_startup: bool,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        let feature = request.language_server_feature();
        let include_declaration = self.editor.config().lsp.goto_reference_include_declaration;
        let mut futures: FuturesOrdered<LspLocationFuture> = FuturesOrdered::new();
        let workspace_identity = self.workspace_backend.identity();

        {
            let Some(view) = self.editor.tree.try_get(self.editor.tree.focus) else {
                self.editor.set_error("No active view for LSP navigation");
                return;
            };
            let view_id = view.id;
            let doc_id = view.doc;
            if matches!(workspace_identity, WorkspaceIdentity::Remote(_)) {
                self.ensure_document_tracked_by_running_servers(doc_id);
            }
            let Some(view) = self.editor.tree.try_get(view_id) else {
                self.editor.set_error("No active view for LSP navigation");
                return;
            };
            let Some(doc) = self.editor.document(view.doc) else {
                self.editor
                    .set_error("No active document for LSP navigation");
                return;
            };
            let Some(identifier) = document_lsp_identifier(doc) else {
                self.editor
                    .set_error("LSP navigation requires a file-backed document");
                return;
            };

            for language_server in doc.language_servers_with_feature(feature) {
                let offset_encoding = language_server.offset_encoding();
                let position = doc.position(view.id, offset_encoding);

                match request {
                    editor_input::NativeLspNavigationRequest::GotoDeclaration => {
                        if let Some(future) =
                            language_server.goto_declaration(identifier.clone(), position, None)
                        {
                            futures.push_back(
                                async move {
                                    let response = future.await?;
                                    Ok(lsp_locations_from_definition_response(
                                        response,
                                        offset_encoding,
                                    ))
                                }
                                .boxed(),
                            );
                        }
                    }
                    editor_input::NativeLspNavigationRequest::GotoDefinition => {
                        if let Some(future) =
                            language_server.goto_definition(identifier.clone(), position, None)
                        {
                            futures.push_back(
                                async move {
                                    let response = future.await?;
                                    Ok(lsp_locations_from_definition_response(
                                        response,
                                        offset_encoding,
                                    ))
                                }
                                .boxed(),
                            );
                        }
                    }
                    editor_input::NativeLspNavigationRequest::GotoTypeDefinition => {
                        if let Some(future) =
                            language_server.goto_type_definition(identifier.clone(), position, None)
                        {
                            futures.push_back(
                                async move {
                                    let response = future.await?;
                                    Ok(lsp_locations_from_definition_response(
                                        response,
                                        offset_encoding,
                                    ))
                                }
                                .boxed(),
                            );
                        }
                    }
                    editor_input::NativeLspNavigationRequest::GotoImplementation => {
                        if let Some(future) =
                            language_server.goto_implementation(identifier.clone(), position, None)
                        {
                            futures.push_back(
                                async move {
                                    let response = future.await?;
                                    Ok(lsp_locations_from_definition_response(
                                        response,
                                        offset_encoding,
                                    ))
                                }
                                .boxed(),
                            );
                        }
                    }
                    editor_input::NativeLspNavigationRequest::GotoReference => {
                        if let Some(future) = language_server.goto_reference(
                            identifier.clone(),
                            position,
                            include_declaration,
                            None,
                        ) {
                            futures.push_back(
                                async move {
                                    let locations = future.await?;
                                    Ok(locations
                                        .into_iter()
                                        .flatten()
                                        .filter_map(|location| {
                                            lsp_location_from_location(location, offset_encoding)
                                        })
                                        .collect())
                                }
                                .boxed(),
                            );
                        }
                    }
                }
            }
        }

        if futures.is_empty() {
            if allow_remote_startup && self.trigger_remote_lsp_navigation_startup(request, cx) {
                return;
            }
            self.editor.set_error(request.unsupported_message());
            return;
        }

        let title = request.picker_title().to_string();
        let empty_message = request.empty_message().to_string();
        cx.spawn(async move |core, cx| {
            let mut locations = Vec::new();
            while let Some(response) = futures_util::StreamExt::next(&mut futures).await {
                match response {
                    Ok(mut response_locations) => locations.append(&mut response_locations),
                    Err(err) => warn!(error = %err, "LSP navigation request failed"),
                }
            }

            if let Some(core) = core.upgrade() {
                core.update(cx, move |core, cx| {
                    core.finish_lsp_navigation(title, empty_message, locations, cx);
                });
            }
        })
        .detach();
    }

    fn active_document_lsp_navigation_readiness(
        &mut self,
        request: editor_input::NativeLspNavigationRequest,
    ) -> (bool, Vec<String>) {
        let feature = request.language_server_feature();
        let workspace_identity = self.workspace_backend.identity();

        let Some(view) = self.editor.tree.try_get(self.editor.tree.focus) else {
            return (false, vec!["no active view".to_string()]);
        };
        let doc_id = view.doc;

        if matches!(workspace_identity, WorkspaceIdentity::Remote(_)) {
            self.ensure_document_tracked_by_running_servers(doc_id);
        }

        let Some(doc) = self.editor.document(doc_id) else {
            return (false, vec![format!("document {doc_id:?} not found")]);
        };

        let configured_servers = doc
            .language_config()
            .map(|config| {
                config
                    .language_servers
                    .iter()
                    .map(|features| {
                        format!(
                            "{}(feature_enabled={})",
                            features.name,
                            features.has_feature(feature)
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let active_doc_servers = doc
            .language_servers()
            .map(|server| format!("{}({:?})", server.name(), server.id()))
            .collect::<Vec<_>>();

        let registry_servers = self
            .editor
            .language_servers
            .iter_clients()
            .map(|client| {
                let initialized = client.is_initialized();
                let supports_feature = initialized && client.supports_feature(feature);
                let configured_for_feature = doc.language_config().is_some_and(|config| {
                    config.language_servers.iter().any(|features| {
                        features.name == client.name() && features.has_feature(feature)
                    })
                });
                format!(
                    "{}({:?}, initialized={}, configured_feature={}, supports_feature={})",
                    client.name(),
                    client.id(),
                    initialized,
                    configured_for_feature,
                    supports_feature
                )
            })
            .collect::<Vec<_>>();

        let ready = doc.language_servers_with_feature(feature).next().is_some();
        let state = vec![
            format!("doc_id={doc_id:?}"),
            format!("path={:?}", doc.path()),
            format!("url={:?}", doc.url()),
            format!("language_id={:?}", doc.language_id()),
            format!("configured_servers={configured_servers:?}"),
            format!("active_doc_servers={active_doc_servers:?}"),
            format!("registry_servers={registry_servers:?}"),
        ];

        (ready, state)
    }

    fn trigger_remote_lsp_navigation_startup(
        &mut self,
        request: editor_input::NativeLspNavigationRequest,
        cx: &mut gpui::Context<crate::Core>,
    ) -> bool {
        let workspace_identity = self.workspace_backend.identity();
        let document_path = self
            .editor
            .tree
            .try_get(self.editor.tree.focus)
            .and_then(|view| self.editor.document(view.doc))
            .and_then(|doc| doc.path())
            .map(|path| path.to_path_buf());
        let Some(project_root) = remote_lsp_project_root_for_document(
            &workspace_identity,
            self.project_directory.as_deref(),
            document_path.as_deref(),
        ) else {
            return false;
        };
        let command_tx = self.project_lsp.command_sender();

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let span = tracing::info_span!(
            "remote_lsp_navigation_startup",
            workspace_root = %project_root.display(),
            request = ?request,
        );
        let command = ProjectLspCommand::OpenProjectSession {
            workspace_root: project_root.clone(),
            response: response_tx,
            span,
        };

        if let Err(error) = command_tx.send(command) {
            warn!(
                error = %error,
                workspace_root = %project_root.display(),
                "Failed to request remote project LSP startup for navigation"
            );
            return false;
        }

        self.editor.set_status(format!(
            "Starting remote language server for {}...",
            request.picker_title().to_ascii_lowercase()
        ));
        self.request_event_driven_maintenance();

        cx.spawn(async move |core, cx| {
            let result = tokio::time::timeout(Duration::from_secs(30), response_rx).await;
            let Some(core) = core.upgrade() else {
                return;
            };

            match result {
                Ok(Ok(Ok(project_result))) => {
                    let started_servers = project_result
                        .servers_started
                        .iter()
                        .map(|server| format!("{}({:?})", server.server_name, server.server_id))
                        .collect::<Vec<_>>();
                    let deadline = Instant::now() + REMOTE_LSP_NAVIGATION_READY_TIMEOUT;
                    let mut readiness_state = Vec::new();
                    let mut ready_for_retry = false;

                    loop {
                        let ready = core.update(cx, |core, _cx| {
                            let (ready, state) =
                                core.active_document_lsp_navigation_readiness(request);
                            readiness_state = state;
                            if !ready {
                                core.request_event_driven_maintenance();
                            }
                            ready
                        });
                        if ready {
                            ready_for_retry = true;
                            break;
                        }

                        if Instant::now() >= deadline {
                            warn!(
                                request = ?request,
                                started_servers = ?started_servers,
                                readiness_state = ?readiness_state,
                                "Remote language server startup completed but navigation capability did not become ready"
                            );
                            break;
                        }

                        tokio::time::sleep(REMOTE_LSP_NAVIGATION_READY_POLL_INTERVAL).await;
                    }

                    if ready_for_retry {
                        core.update(cx, move |core, cx| {
                            core.trigger_lsp_navigation_with_remote_startup(request, false, cx);
                        });
                    } else {
                        core.update(cx, move |core, _cx| {
                            core.editor.set_error(format!(
                                "Remote language server did not become ready for {}",
                                request.picker_title().to_ascii_lowercase()
                            ));
                        });
                    }
                }
                Ok(Ok(Err(error))) => {
                    core.update(cx, move |core, _cx| {
                        core.editor
                            .set_error(format!("Remote language server startup failed: {error}"));
                    });
                }
                Ok(Err(_)) => {
                    core.update(cx, |core, _cx| {
                        core.editor
                            .set_error("Remote language server startup was cancelled");
                    });
                }
                Err(_) => {
                    core.update(cx, |core, _cx| {
                        core.editor
                            .set_error("Remote language server startup timed out");
                    });
                }
            }
        })
        .detach();

        true
    }

    fn finish_lsp_navigation(
        &mut self,
        title: String,
        empty_message: String,
        locations: Vec<crate::types::LspLocation>,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        match locations.as_slice() {
            [] => self.editor.set_error(empty_message),
            [location] => match self.jump_to_lsp_location(location) {
                Ok((doc_id, view_id)) => {
                    cx.emit(crate::Update::SelectionChanged { doc_id, view_id });
                }
                Err(err) => self.editor.set_error(err.to_string()),
            },
            _ => {
                let picker =
                    lsp_locations_picker(&title, locations, self.project_directory.as_deref());
                cx.emit(crate::Update::Picker(picker));
            }
        }

        cx.emit(crate::Update::Redraw);
    }

    pub fn jump_to_lsp_location(
        &mut self,
        location: &crate::types::LspLocation,
    ) -> anyhow::Result<(DocumentId, ViewId)> {
        let doc_id = self.open_navigation_document(&location.path)?;
        let view_id = self.editor.tree.focus;

        if self.editor.tree.try_get(view_id).map(|view| view.doc) != Some(doc_id) {
            self.editor
                .switch(doc_id, helix_view::editor::Action::Replace);
        }

        let view_id = self.editor.tree.focus;
        let doc = self
            .editor
            .document_mut(doc_id)
            .ok_or_else(|| anyhow::anyhow!("LSP target document is not open"))?;
        let range = helix_lsp::util::lsp_range_to_range(
            doc.text(),
            location.range,
            location.offset_encoding,
        )
        .ok_or_else(|| anyhow::anyhow!("LSP target range is out of bounds"))?;

        doc.set_selection(view_id, Selection::single(range.head, range.anchor));
        self.editor.ensure_cursor_in_view(view_id);

        Ok((doc_id, view_id))
    }

    pub fn jump_to_jumplist_location(
        &mut self,
        location: &crate::types::JumpLocation,
    ) -> anyhow::Result<(DocumentId, ViewId)> {
        let doc_id = location.doc_id;
        if self.editor.document(doc_id).is_none() {
            anyhow::bail!("Jumplist target document is no longer open");
        }

        self.editor
            .switch(doc_id, helix_view::editor::Action::Replace);
        let view_id = self.editor.tree.focus;
        let doc = self
            .editor
            .document_mut(doc_id)
            .ok_or_else(|| anyhow::anyhow!("Jumplist target document is not open"))?;

        doc.set_selection(view_id, location.selection.clone());
        self.editor.ensure_cursor_in_view(view_id);

        Ok((doc_id, view_id))
    }

    pub fn jump_to_diagnostic_location(
        &mut self,
        location: &crate::types::DiagnosticLocation,
    ) -> anyhow::Result<(DocumentId, ViewId)> {
        let doc_id = if let Some(path) = &location.path {
            self.open_navigation_document(path)?
        } else {
            if self.editor.document(location.doc_id).is_none() {
                anyhow::bail!("Diagnostic target document is no longer open");
            }
            self.editor
                .switch(location.doc_id, helix_view::editor::Action::Replace);
            location.doc_id
        };
        let view_id = self.editor.tree.focus;
        let doc = self
            .editor
            .document_mut(doc_id)
            .ok_or_else(|| anyhow::anyhow!("Diagnostic target document is not open"))?;
        let len_chars = doc.text().len_chars();
        if location.offset > len_chars {
            anyhow::bail!(
                "The diagnostic location does not exist anymore because the file has changed"
            );
        }

        doc.set_selection(view_id, Selection::point(location.offset));
        self.editor.ensure_cursor_in_view(view_id);

        Ok((doc_id, view_id))
    }

    pub fn jump_to_syntax_file_location(
        &mut self,
        location: &crate::types::SyntaxFileLocation,
    ) -> anyhow::Result<(DocumentId, ViewId)> {
        let doc_id = self.open_navigation_document(&location.path)?;
        let view_id = self.editor.tree.focus;
        let doc = self
            .editor
            .document_mut(doc_id)
            .ok_or_else(|| anyhow::anyhow!("Syntax symbol target document is not open"))?;
        let len_chars = doc.text().len_chars();
        if location.start >= len_chars || location.end > len_chars {
            anyhow::bail!(
                "The location you jumped to does not exist anymore because the file has changed"
            );
        }

        doc.set_selection(view_id, Selection::single(location.start, location.end));
        self.editor.ensure_cursor_in_view(view_id);

        Ok((doc_id, view_id))
    }

    pub fn jump_to_global_search_location(
        &mut self,
        location: &crate::types::GlobalSearchLocation,
    ) -> anyhow::Result<(DocumentId, ViewId)> {
        let doc_id = self.open_navigation_document(&location.path)?;
        let view_id = self.editor.tree.focus;
        let doc = self
            .editor
            .document_mut(doc_id)
            .ok_or_else(|| anyhow::anyhow!("Global search target document is not open"))?;
        let text = doc.text();
        if location.line >= text.len_lines() {
            anyhow::bail!(
                "The line you jumped to does not exist anymore because the file has changed"
            );
        }

        let start = text.line_to_char(location.line);
        let end = text.line_to_char((location.line + 1).min(text.len_lines()));
        doc.set_selection(view_id, Selection::single(start, end));
        self.editor.ensure_cursor_in_view(view_id);

        Ok((doc_id, view_id))
    }

    fn open_navigation_document(&mut self, path: &Path) -> anyhow::Result<DocumentId> {
        let workspace_backend = self.workspace_backend.clone();
        let path = navigation_display_path(
            path,
            self.project_directory.as_deref(),
            &workspace_backend.identity(),
        );
        let action = if self.editor.tree.views().count() == 0 {
            helix_view::editor::Action::VerticalSplit
        } else {
            helix_view::editor::Action::Replace
        };
        open_workspace_document(&mut self.editor, &workspace_backend, &path, action)
    }

    // NOTE: handle_crank_event is defined earlier in the file and includes completion processing
}

fn navigation_display_path(
    path: &Path,
    project_directory: Option<&Path>,
    workspace_identity: &WorkspaceIdentity,
) -> PathBuf {
    if !matches!(workspace_identity, WorkspaceIdentity::Remote(_)) {
        return path.to_path_buf();
    }

    let Some(project_directory) = project_directory else {
        return path.to_path_buf();
    };
    let location = classify_workspace_location(project_directory);
    if !location.is_remote() {
        return path.to_path_buf();
    }

    location.path_mapping().to_display_path(path)
}

/// Detect project root by walking up parent directories looking for project markers
fn detect_project_root_from_file(file_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let start_dir = file_path.parent()?;
    if let Some(root) = find_cargo_workspace_root(start_dir) {
        return Some(root);
    }

    // Common project markers to look for
    let project_markers = [
        "Cargo.toml",       // Rust
        "package.json",     // Node.js/JavaScript
        "pyproject.toml",   // Python
        "requirements.txt", // Python
        "go.mod",           // Go
        "pom.xml",          // Java Maven
        "build.gradle",     // Java Gradle
        ".git",             // Git repository
        ".hg",              // Mercurial repository
        ".svn",             // Subversion repository
    ];

    start_dir
        .ancestors()
        .find(|dir| {
            project_markers
                .iter()
                .any(|marker| dir.join(marker).exists())
        })
        .map(Path::to_path_buf)
}

/// Detect if we need CLI environment and create appropriate ProjectEnvironment.
/// GUI launchers on Unix-like desktops usually provide a minimal environment, so
/// those launches use login-shell capture. CLI launches keep their inherited env.
#[cfg(unix)]
fn detect_and_create_project_environment() -> ProjectEnvironment {
    use std::env;

    nucleotide_logging::info!("ENV_DETECT: Detecting launch environment for Unix platform");

    // Check if we have a minimal PATH that indicates dock launch
    // Dock launches typically have very limited PATH like /usr/bin:/bin
    let current_path = env::var("PATH").unwrap_or_default();
    let path_components: Vec<&str> = current_path.split(':').collect();

    // Indicators of dock launch (minimal environment):
    // - Very few PATH components (typically 2-3)
    // - Missing common development paths like /usr/local/bin
    // - Missing user-specific paths like ~/.cargo/bin
    // - Missing Nix system paths like /run/current-system/sw/bin
    let has_cargo_bin = path_components.iter().any(|&p| p.contains(".cargo/bin"));
    let has_usr_local = path_components.contains(&"/usr/local/bin");
    let has_homebrew = path_components.iter().any(|&p| p.contains("homebrew"));
    let has_nix_system = path_components.contains(&"/run/current-system/sw/bin");
    let path_count = path_components.len();
    let current_home = env::var("HOME").ok();
    let current_cargo_home = env::var("CARGO_HOME").ok();
    let current_xdg_cache_home = env::var("XDG_CACHE_HOME").ok();
    let home_requires_bootstrap = home_requires_login_shell_capture(current_home.as_deref());

    let minimal_launcher_path =
        !has_cargo_bin && !has_usr_local && !has_homebrew && !has_nix_system && path_count <= 4;
    let likely_gui_launch = minimal_launcher_path || home_requires_bootstrap;

    nucleotide_logging::info!(
        path_components = path_count,
        has_cargo_bin = has_cargo_bin,
        has_usr_local = has_usr_local,
        has_homebrew = has_homebrew,
        has_nix_system = has_nix_system,
        minimal_launcher_path = minimal_launcher_path,
        home_requires_bootstrap = home_requires_bootstrap,
        likely_gui_launch = likely_gui_launch,
        current_home = %current_home.as_deref().unwrap_or("<unset>"),
        current_cargo_home = %current_cargo_home.as_deref().unwrap_or("<unset>"),
        current_xdg_cache_home = %current_xdg_cache_home.as_deref().unwrap_or("<unset>"),
        current_path = %current_path,
        "ENV_DETECT: Environment analysis"
    );

    if likely_gui_launch {
        nucleotide_logging::info!(
            "ENV_DETECT: GUI-style launch detected - enabling login shell environment capture"
        );

        ProjectEnvironment::new(None)
    } else {
        nucleotide_logging::info!(
            "ENV_DETECT: Command-line launch detected - using process environment"
        );

        // For command-line launches, we already have the full environment
        // Pass the current environment as CLI environment to maintain it
        let cli_env: std::collections::HashMap<String, String> = env::vars().collect();
        ProjectEnvironment::new(Some(cli_env))
    }
}

#[cfg(any(unix, test))]
fn home_requires_login_shell_capture(home: Option<&str>) -> bool {
    let Some(home) = home else {
        return true;
    };

    home.trim().is_empty() || home == "/homeless-shelter" || home.contains("/homeless-shelter/")
}

#[cfg(not(unix))]
fn detect_and_create_project_environment() -> ProjectEnvironment {
    // On platforms without Unix-style login shell capture, use the process environment.
    let cli_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    ProjectEnvironment::new(Some(cli_env))
}

pub(crate) fn workspace_backend_for_project_directory_with_bootstrap_progress_and_startup_context(
    project_directory: Option<&Path>,
    bootstrap: &nucleotide_remote::RemoteWorkspaceBootstrap,
    progress: &dyn Fn(nucleotide_remote::RemoteDeploymentProgress),
    startup: &nucleotide_remote::RemoteStartupContext,
) -> Result<WorkspaceBackendHandle, Error> {
    workspace_backend_connection_for_project_directory_with_bootstrap_progress_and_startup_context(
        project_directory,
        bootstrap,
        progress,
        startup,
    )
    .map(|connection| connection.backend)
}

pub(crate) fn workspace_backend_connection_for_project_directory_with_bootstrap_progress_and_startup_context(
    project_directory: Option<&Path>,
    bootstrap: &nucleotide_remote::RemoteWorkspaceBootstrap,
    progress: &dyn Fn(nucleotide_remote::RemoteDeploymentProgress),
    startup: &nucleotide_remote::RemoteStartupContext,
) -> Result<nucleotide_remote::WorkspaceBackendConnection, Error> {
    workspace_backend_connection_for_project_directory_with_bootstrap(
        project_directory,
        bootstrap,
        Some(progress),
        Some(startup),
    )
}

fn workspace_backend_connection_for_project_directory_with_bootstrap(
    project_directory: Option<&Path>,
    bootstrap: &nucleotide_remote::RemoteWorkspaceBootstrap,
    progress_handler: Option<&dyn Fn(nucleotide_remote::RemoteDeploymentProgress)>,
    startup: Option<&nucleotide_remote::RemoteStartupContext>,
) -> Result<nucleotide_remote::WorkspaceBackendConnection, Error> {
    let Some(project_directory) = project_directory else {
        return nucleotide_remote::connect_workspace_backend_for_location(
            classify_workspace_location(Path::new(".")),
            bootstrap.options(),
        );
    };
    connect_workspace_backend_connection(project_directory, bootstrap, progress_handler, startup)
}

fn connect_workspace_backend_connection(
    project_directory: &Path,
    bootstrap: &nucleotide_remote::RemoteWorkspaceBootstrap,
    progress_handler: Option<&dyn Fn(nucleotide_remote::RemoteDeploymentProgress)>,
    startup: Option<&nucleotide_remote::RemoteStartupContext>,
) -> Result<nucleotide_remote::WorkspaceBackendConnection, Error> {
    let location = classify_workspace_location(project_directory);
    let display_root = location.display_root().to_path_buf();
    let native_root = location.native_root().to_path_buf();
    let deployment_progress = |progress: nucleotide_remote::RemoteDeploymentProgress| {
        let message = progress.message();
        nucleotide_logging::info!(
            phase = ?progress.phase,
            target = ?progress.target,
            detail = ?progress.detail,
            "Remote workspace deployment progress: {message}"
        );
        if let Some(progress_handler) = progress_handler {
            progress_handler(progress.clone());
        }
    };
    let connection = match startup {
        Some(startup) => {
            nucleotide_remote::connect_workspace_backend_for_location_with_bootstrap_progress_and_startup_context(
                location,
                bootstrap,
                &deployment_progress,
                startup,
            )
        }
        None => nucleotide_remote::connect_workspace_backend_for_location_with_progress(
            location,
            bootstrap.options(),
            &deployment_progress,
        ),
    }
    .with_context(|| {
        format!(
            "failed to initialize workspace backend for {}",
            display_root.display()
        )
    })?;

    match &connection.hello {
        Some(hello) => {
            nucleotide_logging::info!(
                workspace_root = %display_root.display(),
                native_root = %native_root.display(),
                backend = ?connection.backend.identity(),
                helper_version = %hello.helper_version,
                helper_os = %hello.os,
                helper_arch = %hello.arch,
                "Initialized remote workspace backend"
            );
        }
        None => {
            nucleotide_logging::info!(
                workspace_root = %display_root.display(),
                "Initialized local workspace backend"
            );
        }
    }

    Ok(connection)
}

pub(crate) fn workspace_backend_connection_for_project_directory_with_config(
    project_directory: Option<&Path>,
    config: &crate::config::Config,
) -> Result<nucleotide_remote::WorkspaceBackendConnection, Error> {
    let options = config.remote_workspace_backend_options();
    let bootstrap = nucleotide_remote::RemoteWorkspaceBootstrap::new(options);
    workspace_backend_connection_for_project_directory_with_bootstrap(
        project_directory,
        &bootstrap,
        None,
        None,
    )
}

pub fn init_editor(
    args: Args,
    helix_config: Config,
    gui_config: crate::config::Config,
    lang_loader: syntax::Loader,
) -> Result<Application, Error> {
    use helix_view::editor::Action;

    // Determine project directory from args before consuming args.files
    nucleotide_logging::info!(
        files_count = args.files.len(),
        working_directory = ?args.working_directory,
        "Starting project directory detection"
    );
    let project_directory = if let Some(path) = &args.working_directory {
        Some(path.clone())
    } else if let Some((path, _)) = args.files.first()
        && let Some(remote_root) = remote_startup_workspace_root(path)
    {
        nucleotide_logging::info!(
            file_path = %path.display(),
            project_root = %remote_root.display(),
            "Using remote startup workspace root without host filesystem probes"
        );
        Some(remote_root)
    } else if let Some((path, _)) = args.files.first().filter(|p| p.0.is_dir()) {
        // If the first file is a directory, use it as the project directory
        Some(path.clone())
    } else if let Some((file_path, _)) = args.files.first() {
        // If the first file is a file, try to detect project root from its parent directories
        let detected_root = detect_project_root_from_file(file_path);
        if let Some(ref root) = detected_root {
            nucleotide_logging::info!(
                file_path = %file_path.display(),
                project_root = %root.display(),
                "Detected project root from file"
            );
        } else {
            nucleotide_logging::warn!(
                file_path = %file_path.display(),
                "No project root detected from file"
            );
        }
        detected_root
    } else {
        // Fallback: Use current working directory if it's a valid project root
        // This handles the case where workspace_root was detected in main.rs and set as CWD,
        // but no explicit files or working directory were passed via command line args
        if let Some(workspace_root) = implicit_workspace_root_from_current_dir() {
            nucleotide_logging::info!(
                project_directory = %workspace_root.display(),
                "Using current working directory as project directory (workspace_root fallback)"
            );
            Some(workspace_root)
        } else {
            None
        }
    };
    let workspace_connection = workspace_backend_connection_for_project_directory_with_config(
        project_directory.as_deref(),
        &gui_config,
    )?;
    let remote_lsp_session_provider = service_remote_lsp_session_provider(&workspace_connection);
    #[cfg(feature = "terminal-emulator-core")]
    let remote_terminal_session_provider =
        RemoteTerminalSessionProvider::from_connection(&workspace_connection);
    let workspace_backend = workspace_connection.backend;

    let mut theme_parent_dirs = vec![helix_loader::config_dir()];
    theme_parent_dirs.extend(helix_loader::runtime_dirs().iter().cloned());

    // Developer-friendly: include our repo assets path so `nucleotide-*` themes are found in dev runs
    if let Ok(cwd) = std::env::current_dir() {
        let repo_assets = cwd.join("crates").join("nucleotide").join("assets");
        if repo_assets.join("themes").is_dir() {
            nucleotide_logging::info!(
                dev_assets = %repo_assets.display(),
                "Adding repo assets path for theme discovery"
            );
            theme_parent_dirs.push(repo_assets);
        }
    }

    // Add bundle runtime as a backup for macOS
    #[cfg(target_os = "macos")]
    if let Some(rt) = crate::utils::detect_bundle_runtime()
        && !theme_parent_dirs.contains(&rt)
    {
        theme_parent_dirs.push(rt);
    }

    let theme_loader = std::sync::Arc::new(helix_view::theme::Loader::new(&theme_parent_dirs));

    let true_color = true;

    // Load initial theme
    // For System mode, choose initial theme based on OS appearance when available to avoid purple flicker from Helix default.
    let theme_name = match gui_config.gui.theme.mode {
        crate::config::ThemeMode::Light => Some(gui_config.gui.theme.get_light_theme()),
        crate::config::ThemeMode::Dark => Some(gui_config.gui.theme.get_dark_theme()),
        crate::config::ThemeMode::System => {
            // Best-effort OS appearance detection prior to window creation
            // macOS: AppleInterfaceStyle=Dark indicates dark appearance
            let initial_dark = std::env::var("AppleInterfaceStyle")
                .map(|v| v.eq_ignore_ascii_case("dark"))
                .unwrap_or(false);

            if initial_dark {
                Some(gui_config.gui.theme.get_dark_theme())
            } else {
                Some(gui_config.gui.theme.get_light_theme())
            }
        }
    };

    // Check if theme loading should be disabled for testing fallback colors
    let disable_theme_loading = std::env::var("NUCLEOTIDE_DISABLE_THEME_LOADING")
        .map(|val| val == "1" || val.to_lowercase() == "true")
        .unwrap_or(false);

    let theme = if disable_theme_loading {
        warn!(
            "Theme loading disabled via NUCLEOTIDE_DISABLE_THEME_LOADING - using default theme but derive_ui_theme will ignore it"
        );
        // Use any theme here - the derive_ui_theme function will ignore it when testing mode is enabled
        helix_view::Theme::default()
    } else {
        // Try the chosen theme, then the opposite (dark vs light), then fall back to loader default.
        let mut try_names = vec![];
        if let Some(primary) = theme_name.clone() {
            try_names.push(primary);
        }
        // Opposite mode counterpart as a second try
        match gui_config.gui.theme.mode {
            crate::config::ThemeMode::Light => {
                try_names.push(gui_config.gui.theme.get_dark_theme())
            }
            crate::config::ThemeMode::Dark => {
                try_names.push(gui_config.gui.theme.get_light_theme())
            }
            crate::config::ThemeMode::System => {
                // If we picked dark first, also try light (and vice versa)
                let initial_dark = std::env::var("AppleInterfaceStyle")
                    .map(|v| v.eq_ignore_ascii_case("dark"))
                    .unwrap_or(false);
                if initial_dark {
                    try_names.push(gui_config.gui.theme.get_light_theme());
                } else {
                    try_names.push(gui_config.gui.theme.get_dark_theme());
                }
            }
        }

        let mut loaded = None;
        for name in try_names {
            match theme_loader.load(&name) {
                Ok(theme) => {
                    loaded = Some(theme);
                    break;
                }
                Err(e) => {
                    warn!(theme_name = %name, error = %e, "Failed to load theme; trying fallback")
                }
            }
        }
        loaded.unwrap_or_else(|| theme_loader.default_theme(true_color))
    };

    // Log theme's ui.background and ui.window raw styles to verify inputs
    {
        let ui_bg = theme.get("ui.background");
        let ui_window = theme.get("ui.window");
        let ui_menu = theme.get("ui.menu");
        info!(
            helix_theme_name = %theme.name(),
            ui_background_bg = ?ui_bg.bg,
            ui_window_bg = ?ui_window.bg,
            ui_menu_bg = ?ui_menu.bg,
            "Theme load: Helix theme background candidates"
        );
    }

    let syn_loader = Arc::new(ArcSwap::from_pointee(lang_loader));

    // CRITICAL: Enable true_color support for GUI mode before creating the editor
    // This is required for themes to work correctly
    let mut helix_config = helix_config;
    helix_config.editor.true_color = true;

    let config = Arc::new(ArcSwap::from_pointee(helix_config));

    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 25,
    };
    // CRITICAL: Register events FIRST, before creating handlers
    helix_term::events::register();

    let (signature_tx, _signature_rx) = tokio::sync::mpsc::channel(1);
    let (auto_save_tx, _auto_save_rx) = tokio::sync::mpsc::channel(1);
    let (doc_colors_tx, _doc_colors_rx) = tokio::sync::mpsc::channel(1);
    let (doc_links_tx, _doc_links_rx) = tokio::sync::mpsc::channel(1);
    use helix_event::AsyncHook as _;
    let pull_diagnostics_tx = PullDiagnosticsHandler::default().spawn();
    let pull_all_diagnostics_tx = PullAllDocumentsDiagnosticsHandler::default().spawn();
    let (code_action_hint_tx, _code_action_hint_rx) = tokio::sync::mpsc::channel(1);
    // Create a dummy completion channel since Helix CompletionHandler expects one
    // We'll register our own hooks to capture completion results directly
    let (completion_tx, _completion_rx) = tokio::sync::mpsc::channel(1);

    let handlers = Handlers {
        completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
        signature_hints: signature_tx,
        auto_save: auto_save_tx,
        document_colors: doc_colors_tx,
        document_links: doc_links_tx,
        word_index: helix_view::handlers::word_index::Handler::spawn(),
        pull_diagnostics: pull_diagnostics_tx,
        pull_all_documents_diagnostics: pull_all_diagnostics_tx,
        code_action_hint: code_action_hint_tx,
    };

    // Register handler hooks to enable LSP features
    helix_view::handlers::register_hooks(&handlers);
    register_pull_diagnostic_hooks(&handlers);

    // Initialize event bridge system for Helix -> GPUI event forwarding
    let (bridge_tx, bridge_rx) = event_bridge::create_bridge_channel();
    event_bridge::initialize_bridge(bridge_tx);
    event_bridge::register_event_hooks();

    // The Application consumes this channel as the single project-session dispatcher.
    nucleotide_logging::info!("Initialized project LSP session command channel");

    let mut editor = Editor::new(
        area,
        theme_loader.clone(),
        syn_loader.clone(),
        Arc::new(Map::new(Arc::clone(&config), |config: &Config| {
            &config.editor
        })),
        handlers,
        WorkspaceTrust::fully_trusted(),
    );
    editor.set_save_handler(Some(Arc::new(WorkspaceDocumentSaveHandler::new(
        workspace_backend.clone(),
    ))));

    if args.load_tutor {
        let path = helix_loader::runtime_file(Path::new("tutor"));
        let doc_id = editor.open(&path, Action::VerticalSplit)?;
        let view_id = editor.tree.focus;
        // Check if the view exists before setting selection
        if editor.tree.contains(view_id) {
            let doc = doc_mut!(editor, &doc_id);
            let pos = Selection::point(pos_at_coords(
                doc.text().slice(..),
                Position::new(0, 0),
                true,
            ));
            doc.set_selection(view_id, pos);
        }

        // Unset path to prevent accidentally saving to the original tutor file.
        doc_mut!(editor).set_path(None);
    } else if !args.files.is_empty() {
        // Open files from command line arguments
        let mut first = true;
        for (file, pos) in args.files {
            // Skip directories
            if !startup_path_should_open_as_file(&file) {
                continue;
            }

            let action = if first {
                // Use split layout from command line args, default to vertical split
                match args.split {
                    Some(helix_view::tree::Layout::Horizontal) => Action::HorizontalSplit,
                    Some(helix_view::tree::Layout::Vertical) | None => Action::VerticalSplit,
                }
            } else {
                // For subsequent files, use the same split layout if specified
                match args.split {
                    Some(helix_view::tree::Layout::Horizontal) => Action::HorizontalSplit,
                    Some(helix_view::tree::Layout::Vertical) => Action::VerticalSplit,
                    None => Action::Load, // Default to loading in same view when no split specified
                }
            };

            info!(
                file = ?file,
                action = ?action,
                "Opening file from command line"
            );
            match open_workspace_document(&mut editor, &workspace_backend, &file, action) {
                Ok(doc_id) => {
                    info!(
                        file = ?file,
                        doc_id = ?doc_id,
                        "Successfully opened file from CLI"
                    );

                    // Log document info
                    if let Some(doc) = editor.document(doc_id) {
                        info!(
                            language = ?doc.language_name(),
                            path = ?doc.path(),
                            "Document information"
                        );
                    }
                    let view_id = editor.tree.focus;
                    if !pos.is_empty() {
                        // Set cursor position if specified (use first position)
                        if editor.tree.contains(view_id) {
                            let doc = doc_mut!(editor, &doc_id);
                            let text = doc.text();
                            if let Some(first_pos) = pos.first() {
                                let line = first_pos.row.saturating_sub(1); // Convert to 0-indexed
                                let col = first_pos.col;
                                let char_pos = char_index_for_line_col(text.slice(..), line, col);
                                let selection = Selection::point(char_pos);
                                doc.set_selection(view_id, selection);
                            }
                        }
                    }
                    first = false;
                }
                Err(e) => {
                    error!(file = ?file, error = %e, "Failed to open file");
                }
            }
        }

        // If no files were successfully opened, create a new file
        if first {
            editor.new_file(Action::VerticalSplit);
        }
    } else {
        editor.new_file(Action::VerticalSplit);
    }

    let _ = editor.set_theme(theme);

    let native_keys = Box::new(Map::new(Arc::clone(&config), |config: &Config| {
        &config.keys
    }));
    let compositor = Compositor::new(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 25,
    });
    let native_keymaps = Keymaps::new(native_keys);
    let editor_input = EditorInputBridge::new(native_keymaps);
    let jobs = Jobs::new();

    // CRITICAL: Create ProjectEnvironment BEFORE LSP system so LSP can get proper environment
    let project_environment = Arc::new(detect_and_create_project_environment());
    if project_environment.cli_environment().is_none() {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let env = handle.block_on(project_environment.bootstrap_process_environment());
                nucleotide_logging::info!(
                    env_var_count = env.len(),
                    "Bootstrapped process environment from login shell for GUI-style launch"
                );
            }
            Err(error) => {
                nucleotide_logging::warn!(
                    error = %error,
                    "Could not bootstrap process environment before runtime was available"
                );
            }
        }
    }

    let file_op_runtime = tokio::runtime::Handle::try_current()
        .context("workspace file operations require an active Tokio runtime")?;
    let workspace_file_ops =
        WorkspaceFileOpHandler::new(workspace_backend.clone(), file_op_runtime);

    let terminal_runtime = TerminalRuntimeHandle::new();
    #[cfg(feature = "terminal-emulator-core")]
    terminal_runtime.set_remote_provider(remote_terminal_session_provider);

    Ok(Application {
        editor,
        compositor,
        editor_input,
        jobs,
        lsp_progress: LspProgressMap::new(),
        lsp_state: None, // Will be initialized when Application is wrapped in a GPUI entity
        project_directory,
        workspace_backend,
        helix_event_rx: Some(bridge_rx),
        config: gui_config,
        helix_config_arc: config,
        project_lsp: ProjectLspCoordinator::with_remote_session_provider(
            remote_lsp_session_provider,
        ),
        project_environment, // Already created above before LSP system initialization
        workspace_file_ops,
        pending_lsp_workspace_edits: VecDeque::new(),
        terminal_runtime,
        maintenance_wake: None,
    })
}

fn startup_path_should_open_as_file(path: &Path) -> bool {
    nucleotide_workspace::remote_path_is_probably_file(path).unwrap_or_else(|| !path.is_dir())
}

pub(crate) fn open_workspace_document(
    editor: &mut Editor,
    workspace_backend: &WorkspaceBackendHandle,
    path: &Path,
    action: helix_view::editor::Action,
) -> Result<DocumentId, Error> {
    if !should_use_workspace_backend_for_document_io(&workspace_backend.identity(), path) {
        return editor.open(path, action).map_err(Error::from);
    }

    let document_read = futures_executor::block_on(read_workspace_document(
        workspace_backend.clone(),
        path.to_path_buf(),
    ))?;

    open_workspace_document_from_read(editor, document_read, action)
}

pub(crate) struct WorkspaceDocumentRead {
    pub read: FileRead,
    pub diff_base: Option<Vec<u8>>,
}

pub(crate) async fn read_workspace_document(
    workspace_backend: WorkspaceBackendHandle,
    path: PathBuf,
) -> Result<WorkspaceDocumentRead, Error> {
    let read = workspace_backend
        .read_file(&path, ReadOptions::default())
        .await
        .with_context(|| format!("failed to read workspace file {}", path.display()))?;
    if read.truncated {
        bail!(
            "workspace file was truncated while opening {}",
            path.display()
        );
    }

    let diff_base = match read_remote_git_diff_base(&workspace_backend, &read.path).await {
        Ok(diff_base) => diff_base,
        Err(error) => {
            debug!(
                path = %read.path.display(),
                error = %error,
                "Could not load remote git diff base"
            );
            None
        }
    };

    Ok(WorkspaceDocumentRead { read, diff_base })
}

pub(crate) fn open_workspace_document_from_read(
    editor: &mut Editor,
    document_read: WorkspaceDocumentRead,
    action: helix_view::editor::Action,
) -> Result<DocumentId, Error> {
    let WorkspaceDocumentRead { read, diff_base } = document_read;
    let metadata = DocumentOpenMetadata {
        readonly: read.readonly,
        last_saved_time: read.modified,
        file_version: read.version.map(FileVersion::into_bytes),
    };
    let mut bytes = read.bytes.as_slice();
    let mut doc = Document::open_from_reader(
        &read.path,
        &mut bytes,
        None,
        true,
        metadata,
        editor.config.clone(),
        editor.syn_loader.clone(),
    )
    .with_context(|| format!("failed to decode workspace file {}", read.path.display()))?;

    set_remote_document_lsp_url(&mut doc, &read.path);

    if let Some(diff_base) = diff_base {
        doc.set_diff_base(diff_base);
    }

    let doc_id = editor.open_document_with_options(
        &read.path,
        action,
        doc,
        helix_view::editor::OpenDocumentOptions {
            local_diff: false,
            launch_language_servers: false,
        },
    );
    if let Some(doc) = editor.document_mut(doc_id) {
        set_remote_document_lsp_url(doc, &read.path);
    }

    Ok(doc_id)
}

/// Register a read-only placeholder immediately while a remote document read is in flight.
pub(crate) fn open_loading_workspace_document(
    editor: &mut Editor,
    path: &Path,
    action: helix_view::editor::Action,
) -> Result<DocumentId, Error> {
    let metadata = DocumentOpenMetadata {
        readonly: true,
        last_saved_time: Some(SystemTime::UNIX_EPOCH),
        file_version: None,
    };
    let mut bytes = std::io::empty();
    let mut doc = Document::open_from_reader(
        path,
        &mut bytes,
        None,
        true,
        metadata,
        editor.config.clone(),
        editor.syn_loader.clone(),
    )
    .with_context(|| format!("failed to create workspace placeholder {}", path.display()))?;

    set_remote_document_lsp_url(&mut doc, path);

    Ok(editor.open_document_with_options(
        path,
        action,
        doc,
        helix_view::editor::OpenDocumentOptions {
            local_diff: false,
            launch_language_servers: false,
        },
    ))
}

/// Fill an existing placeholder without changing its document ID or stealing focus.
pub(crate) fn hydrate_workspace_document_from_read(
    editor: &mut Editor,
    doc_id: DocumentId,
    preferred_view_id: ViewId,
    document_read: WorkspaceDocumentRead,
) -> Result<ViewId, Error> {
    let WorkspaceDocumentRead { read, diff_base } = document_read;
    let path_matches = editor
        .document(doc_id)
        .with_context(|| format!("document {doc_id:?} is no longer open"))?
        .path()
        == Some(read.path.as_path());
    if !path_matches {
        bail!(
            "document path changed while loading: expected {}, found {:?}",
            read.path.display(),
            editor.document(doc_id).and_then(Document::path)
        );
    }

    let view_id = {
        let doc = editor
            .document(doc_id)
            .with_context(|| format!("document {doc_id:?} is no longer open"))?;
        if editor.tree.try_get(preferred_view_id).is_some()
            && doc.selections().contains_key(&preferred_view_id)
        {
            preferred_view_id
        } else {
            doc.selections()
                .keys()
                .copied()
                .find(|view_id| editor.tree.try_get(*view_id).is_some())
                .unwrap_or(editor.tree.focus)
        }
    };

    let metadata = DocumentOpenMetadata {
        readonly: read.readonly,
        last_saved_time: read.modified,
        file_version: read.version.map(FileVersion::into_bytes),
    };
    let mut bytes = read.bytes.as_slice();
    let tree = &mut editor.tree;
    let documents = &mut editor.documents;
    let view = tree.get_mut(view_id);
    let doc = documents
        .get_mut(&doc_id)
        .with_context(|| format!("document {doc_id:?} is no longer open"))?;
    doc.ensure_view_init(view_id);
    doc.hydrate_from_reader(&mut bytes, view, metadata)
        .with_context(|| format!("failed to decode workspace file {}", read.path.display()))?;
    // Hydration inserts the entire file into an empty placeholder, which maps the
    // placeholder's cursor from offset zero to the end of the inserted text.
    doc.set_selection(view_id, Selection::point(0));
    set_remote_document_lsp_url(doc, &read.path);

    if let Some(diff_base) = diff_base {
        doc.set_diff_base(diff_base);
    }

    Ok(view_id)
}

pub(crate) fn reload_workspace_document_from_read(
    editor: &mut Editor,
    doc_id: DocumentId,
    view_id: ViewId,
    document_read: WorkspaceDocumentRead,
) -> Result<(), Error> {
    let WorkspaceDocumentRead { read, diff_base } = document_read;
    let encoding = editor
        .document(doc_id)
        .with_context(|| format!("document {doc_id:?} is no longer open"))?
        .encoding();

    let mut bytes = read.bytes.as_slice();
    let (rope, ..) = helix_view::document::from_reader(&mut bytes, Some(encoding))
        .with_context(|| format!("failed to decode workspace file {}", read.path.display()))?;

    let transaction = {
        let doc = editor
            .document(doc_id)
            .with_context(|| format!("document {doc_id:?} is no longer open"))?;
        helix_core::diff::compare_ropes(doc.text(), &rope)
    };

    let view = editor
        .tree
        .try_get(view_id)
        .with_context(|| format!("view {view_id:?} is no longer open"))?;
    if view.doc != doc_id {
        bail!("view {view_id:?} no longer displays document {doc_id:?}");
    }

    let tree = &mut editor.tree;
    let documents = &mut editor.documents;
    let view = tree.get_mut(view_id);
    let doc = documents
        .get_mut(&doc_id)
        .with_context(|| format!("document {doc_id:?} is no longer open"))?;

    if doc.path() != Some(read.path.as_path()) {
        bail!(
            "document path changed before reload: expected {}, found {:?}",
            read.path.display(),
            doc.path()
        );
    }

    doc.set_path_with_metadata(Some(&read.path), Some(read.readonly), read.modified);
    doc.set_file_version(
        read.version
            .as_ref()
            .map(|version| version.as_bytes().to_vec()),
    );
    set_remote_document_lsp_url(doc, &read.path);
    if !doc.apply(&transaction, view.id) {
        bail!(
            "failed to apply reload transaction for {}",
            read.path.display()
        );
    }
    doc.append_changes_to_history(view);
    doc.reset_modified();
    doc.detect_indent_and_line_ending();

    if let Some(diff_base) = diff_base {
        doc.set_diff_base(diff_base);
    }

    Ok(())
}

fn set_remote_document_lsp_url(doc: &mut Document, display_path: &Path) {
    let location = classify_workspace_location(display_path);
    if !location.is_remote() {
        return;
    }

    match remote_native_file_uri(location.native_root()) {
        Some(url) => doc.set_lsp_url(Some(url)),
        None => warn!(
            path = %display_path.display(),
            native_path = %location.native_root().display(),
            "Remote document path cannot be represented as an LSP file URL"
        ),
    }
}

enum NativeOpenFileOutcome {
    Directory(PathBuf),
    Document(WorkspaceDocumentRead),
}

async fn native_open_file_with_backend(
    workspace_backend: WorkspaceBackendHandle,
    path: PathBuf,
) -> Result<NativeOpenFileOutcome, Error> {
    let stat = workspace_backend
        .stat(&path)
        .await
        .with_context(|| format!("failed to stat workspace path {}", path.display()))?;

    if matches!(stat.kind, FileKind::Directory) {
        return Ok(NativeOpenFileOutcome::Directory(path));
    }

    read_workspace_document(workspace_backend, path)
        .await
        .map(NativeOpenFileOutcome::Document)
}

impl Application {
    fn handle_native_open_file_request(
        &mut self,
        path: PathBuf,
        action: helix_view::editor::Action,
        handle: tokio::runtime::Handle,
        cx: &mut gpui::Context<crate::Core>,
    ) {
        let workspace_backend = self.workspace_backend.clone();
        let workspace_identity = workspace_backend.identity();

        if matches!(workspace_identity, WorkspaceIdentity::Local) {
            if path.is_dir() {
                cx.emit(crate::Update::ShowFilePickerAt(path));
                return;
            }

            if let Err(error) =
                open_workspace_document(&mut self.editor, &workspace_backend, &path, action)
            {
                self.editor
                    .set_error(format!("Open file failed: {error:?}"));
            }
            return;
        }

        self.editor
            .set_status(format!("Opening workspace file: {}", path.display()));
        cx.spawn(async move |core, cx| {
            let result = match handle
                .spawn(native_open_file_with_backend(
                    workspace_backend,
                    path.clone(),
                ))
                .await
            {
                Ok(result) => result,
                Err(error) => Err(anyhow::anyhow!("workspace open task failed: {error}")),
            };

            if let Some(core) = core.upgrade() {
                core.update(cx, |core, cx| match result {
                    Ok(NativeOpenFileOutcome::Directory(path)) => {
                        cx.emit(crate::Update::ShowFilePickerAt(path));
                    }
                    Ok(NativeOpenFileOutcome::Document(document_read)) => {
                        match open_workspace_document_from_read(
                            &mut core.editor,
                            document_read,
                            action,
                        ) {
                            Ok(doc_id) => {
                                core.attach_or_start_remote_document_servers(doc_id, &handle, cx);
                            }
                            Err(error) => {
                                core.editor
                                    .set_error(format!("Open remote file failed: {error:?}"));
                            }
                        }
                    }
                    Err(error) => {
                        core.editor
                            .set_error(format!("Open remote file failed: {error:?}"));
                    }
                });
            }
        })
        .detach();
    }

    fn handle_job_callback(&mut self, callback: helix_term::job::Callback) {
        use helix_term::job::Callback;

        crate::completion_interception::hook_19_job_system("normal_callback_processing");

        match callback {
            Callback::EditorCompositor(callback) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    callback(&mut self.editor, &mut self.compositor);
                }));

                if let Err(payload) = result {
                    let panic_message = panic_payload_message(payload.as_ref());
                    warn!(
                        panic = %panic_message,
                        "Skipped Helix compositor callback after panic in native GPUI mode"
                    );
                    self.editor.set_error(format!(
                        "Skipped terminal compositor callback: {panic_message}"
                    ));
                }
            }
            Callback::Editor(callback) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    callback(&mut self.editor);
                }));

                if let Err(payload) = result {
                    let panic_message = panic_payload_message(payload.as_ref());
                    warn!(
                        panic = %panic_message,
                        "Skipped Helix editor callback after panic in native GPUI mode"
                    );
                    self.editor
                        .set_error(format!("Skipped editor callback: {panic_message}"));
                }
            }
            Callback::Followup(callback) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    callback(&mut self.editor)
                }));

                match result {
                    Ok(Some(job)) => self.jobs.add(job),
                    Ok(None) => {}
                    Err(payload) => {
                        let panic_message = panic_payload_message(payload.as_ref());
                        warn!(
                            panic = %panic_message,
                            "Skipped Helix follow-up callback after panic in native GPUI mode"
                        );
                        self.editor
                            .set_error(format!("Skipped follow-up callback: {panic_message}"));
                    }
                }
            }
        }
    }
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod job_callback_tests {
    use super::*;

    use std::num::NonZeroU8;

    #[test]
    fn panic_payload_message_reads_static_str_payloads() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("missing compositor component");

        assert_eq!(
            panic_payload_message(payload.as_ref()),
            "missing compositor component"
        );
    }

    #[test]
    fn panic_payload_message_reads_string_payloads() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("terminal callback"));

        assert_eq!(panic_payload_message(payload.as_ref()), "terminal callback");
    }

    #[test]
    fn truncate_inlay_hint_label_leaves_short_label_under_limit() {
        let mut label = String::from("Result");

        truncate_inlay_hint_label(&mut label, NonZeroU8::new(26));

        assert_eq!(label, "Result");
    }

    #[test]
    fn truncate_inlay_hint_label_uses_grapheme_boundaries() {
        let mut label = String::from("ééé");

        truncate_inlay_hint_label(&mut label, NonZeroU8::new(2));

        assert_eq!(label, "éé…");
    }

    #[test]
    fn truncate_inlay_hint_label_handles_first_grapheme_over_limit() {
        let mut label = String::from("你");

        truncate_inlay_hint_label(&mut label, NonZeroU8::new(1));

        assert_eq!(label, "…");
    }
}

// Tests moved to tests/integration_test.rs to avoid GPUI proc macro compilation issues
// The issue: When compiling with --test, GPUI proc macros cause stack overflow
// when processing certain patterns in our codebase
#[cfg(test)]
#[allow(dead_code)]
mod tests;
