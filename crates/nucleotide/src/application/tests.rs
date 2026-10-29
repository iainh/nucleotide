use super::completion::{
    buffer_word_completion_items, completion_context_for_trigger, dedupe_completion_items,
    local_path_completion_context, lsp_completion_insert_text, lsp_completion_insert_text_format,
    lsp_completion_items_from_response, lsp_completion_items_from_response_for_server,
    lsp_completion_resolve_supported, lsp_completion_response_is_incomplete, path_completion_items,
    path_completion_items_from_listing, suppress_shadowed_buffer_word_completion_items,
};
use super::editor_input::NativeLspNavigationRequest;
use super::symbols::{
    NativeSymbolItem, NativeSymbolTarget, lsp_symbol_picker, native_symbol_item_from_lsp,
    should_use_workspace_syntax_symbol_fallback, syntax_symbol_kind_from_capture_name,
};
use super::{
    Application, EditorInputBridge, LspCompletionTrigger, MaintenanceWake, NativeOpenFileOutcome,
    PendingCompletionRequest, ProjectEnvironmentProvider, ProjectLspCoordinator,
    RemoteLspLaunchProxyProvider, WorkspaceDocumentSaveHandler, buffer_text_matches_path,
    buffer_text_matches_string, char_index_for_line_col, coalesce_helix_events,
    commands_to_probe_for_servers, configured_project_servers, current_dir_is_executable_dir,
    detect_project_lsp_plan_with_backend, diagnostic_picker_path_label, diagnostic_severity_label,
    discover_project_languages_with_backend, file_picker_current_directory,
    home_requires_login_shell_capture, hydrate_workspace_document_from_read,
    lsp_diagnostic_document_uri, lsp_location_from_location, lsp_location_path_from_url,
    native_open_file_with_backend, navigation_display_path, open_loading_workspace_document,
    open_workspace_document, probe_available_language_server_commands, project_lsp_plan_from_names,
    project_marker_names, read_workspace_document, remote_lsp_project_root_for_document,
    remote_lsp_root_uri_matches_document_workspace, remote_native_file_uri,
    retain_servers_with_available_commands, should_stat_picker_root_with_backend,
    should_use_native_save_for_settings_file, startup_path_should_open_as_file,
    str_prefix_at_byte_limit,
};
use arc_swap::{ArcSwap, access::Map};
use futures_util::{FutureExt, stream::FuturesOrdered};
use gpui::{AppContext, Entity};
use helix_core::{Rope, Transaction, diagnostic::Severity, syntax};
use helix_lsp::{LspProgressMap, OffsetEncoding, lsp};
use helix_term::{
    compositor::Compositor,
    config::Config as HelixConfig,
    job::{Callback, Job, Jobs},
    keymap::Keymaps,
};
use helix_view::{editor::Action, graphics::Rect, handlers::Handlers, theme};
use nucleotide_core::event_bridge;
use nucleotide_events::completion::{CompletionItem, CompletionItemKind};
use nucleotide_workspace::{
    DirectoryEntry, DirectoryListing, FileKind, FileSearchQuery, FileSearchResult, FileStat,
    GitHeadResult, GitStatusOptions, GitStatusResult, ProcessOutput, ProcessSpec,
    ProjectEnvironmentOrigin, ProjectEnvironmentSnapshot, ReadOptions, RemoteWorkspaceIdentity,
    RemoteWorkspaceKind, TextSearchQuery, TextSearchResult, WorkspaceBackend,
    WorkspaceBackendHandle, WorkspaceIdentity, WorkspacePathMapping, WriteOptions,
    local_workspace_backend, path_mapped_workspace_backend,
};
use slotmap::{Key, KeyData};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::mpsc;

#[test]
fn workspace_roots_prefer_outer_cargo_workspace_over_member_markers() {
    let dir = tempdir().unwrap();
    let member = dir.path().join("crates/member");
    let src = member.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = ['crates/member']\n",
    )
    .unwrap();
    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname = 'member'\nversion = '0.1.0'\n",
    )
    .unwrap();
    fs::create_dir(member.join(".git")).unwrap();
    fs::write(src.join("package.json"), "{}").unwrap();

    assert_eq!(super::find_workspace_root_from(&src), dir.path());
    assert_eq!(
        super::detect_project_root_from_file(&src.join("lib.rs")),
        Some(dir.path().to_path_buf())
    );

    // An explicit nested workspace takes precedence over an outer workspace.
    fs::write(member.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    assert_eq!(super::find_workspace_root_from(&src), member);
    assert_eq!(
        super::detect_project_root_from_file(&src.join("lib.rs")),
        Some(member)
    );
}

#[test]
fn workspace_roots_keep_fallbacks_without_a_cargo_workspace() {
    let dir = tempdir().unwrap();
    let member = dir.path().join("member");
    let src = member.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(
        member.join("Cargo.toml"),
        "# [workspace] is only a comment\n[package]\nname = 'member'\n",
    )
    .unwrap();

    assert_eq!(super::find_workspace_root_from(&src), dir.path());
    assert_eq!(
        super::detect_project_root_from_file(&src.join("lib.rs")),
        Some(member)
    );

    fs::remove_dir(dir.path().join(".git")).unwrap();
    fs::remove_file(dir.path().join("member/Cargo.toml")).unwrap();
    assert_eq!(super::find_workspace_root_from(&src), src);
    assert_eq!(
        super::detect_project_root_from_file(&src.join("lib.rs")),
        None
    );
}

fn assert_same_path(actual: &Path, expected: &Path) {
    fn comparable_path(path: &Path) -> PathBuf {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        #[cfg(windows)]
        {
            let text = path.to_string_lossy();
            return text
                .strip_prefix(r"\\?\")
                .map(PathBuf::from)
                .unwrap_or(path);
        }
        #[cfg(not(windows))]
        path
    }

    let actual = comparable_path(actual);
    let expected = comparable_path(expected);
    assert_eq!(actual, expected);
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CapturedUpdate {
    DiagnosticPicker(Vec<Severity>),
    LspInitialized(helix_lsp::LanguageServerId),
    StatusChanged(String, crate::types::Severity),
}

static TEST_RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test tokio runtime")
});

fn directory_entry(name: &str, path: PathBuf, kind: FileKind) -> DirectoryEntry {
    DirectoryEntry {
        name: name.to_string(),
        path: path.clone(),
        stat: FileStat {
            path,
            kind,
            size: 0,
            modified: None,
            version: None,
            readonly: false,
        },
        symlink_target: None,
        target_exists: None,
        ignored: None,
    }
}

#[test]
fn current_dir_is_executable_dir_matches_executable_parent() {
    let exe_dir = tempdir().unwrap();
    let exe_path = exe_dir.path().join("nucl.exe");
    fs::write(&exe_path, "").unwrap();

    assert!(current_dir_is_executable_dir(exe_dir.path(), &exe_path));
}

#[test]
fn current_dir_is_executable_dir_rejects_other_directories() {
    let exe_dir = tempdir().unwrap();
    let current_dir = tempdir().unwrap();
    let exe_path = exe_dir.path().join("nucl.exe");
    fs::write(&exe_path, "").unwrap();

    assert!(!current_dir_is_executable_dir(
        current_dir.path(),
        &exe_path
    ));
}

#[test]
fn remote_lsp_launch_proxy_provider_creates_remote_helper_shim() {
    let provider = RemoteLspLaunchProxyProvider {
        options: nucleotide_remote::RemoteWorkspaceBackendOptions {
            remote_helper_path: PathBuf::from("/remote/bin/nucleotide-remote"),
            remote_helper_path_is_override: true,
            ssh_helper_path: None,
            ssh_helper_path_is_override: false,
            wsl_helper_path: None,
            wsl_helper_path_is_override: false,
            local_helper_path: None,
            ssh_helper_upload_path: None,
            ssh_helper_artifact_dir: None,
            ssh_helper_download_base_url: None,
            ssh_helper_install_policy: nucleotide_remote::RemoteHelperInstallPolicy::Never,
            wsl_helper_install_policy: nucleotide_remote::RemoteHelperInstallPolicy::Never,
            ssh_connect_timeout_secs: None,
            ssh_extra_args: Vec::new(),
            ssh_control_path: None,
            use_local_service: false,
        },
        helper_path_cache: std::sync::Mutex::new(HashMap::new()),
    };

    let proxy = nucleotide_lsp::LspLaunchProxyProvider::create_lsp_launch_proxy(
        &provider,
        Path::new("ssh://me@example.com/home/me/project"),
        "jedi",
        "jedi-language-server",
    )
    .unwrap()
    .expect("remote proxy");

    assert!(proxy.path_dir.is_dir());
    assert!(proxy.description.contains("lsp-proxy"));
    assert!(proxy.description.contains("jedi-language-server"));
    assert!(
        proxy.cleanup_paths[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("jedi-language-server")
    );

    let script = fs::read_to_string(&proxy.cleanup_paths[0]).unwrap();
    assert!(script.contains("nucleotide-remote"));
    assert!(script.contains("lsp-proxy"));
    assert!(script.contains("jedi-language-server"));
    #[cfg(windows)]
    if let Ok(ssh) = helix_stdx::env::which("ssh") {
        assert!(script.contains(&ssh.to_string_lossy().into_owned()));
    }
    assert_eq!(provider.helper_path_cache.lock().unwrap().len(), 1);

    for path in proxy.cleanup_paths {
        if path.is_dir() {
            let _ = fs::remove_dir(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

#[test]
fn buffer_text_matches_path_requires_exact_saved_text() {
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().join("sample.txt");
    fs::write(&path, "one\ntwo\n").unwrap();

    assert!(buffer_text_matches_path(
        &helix_core::Rope::from_str("one\ntwo\n"),
        &path
    ));
    assert!(!buffer_text_matches_path(
        &helix_core::Rope::from_str("one\ntwo"),
        &path
    ));
}

#[test]
fn buffer_text_matches_string_requires_exact_saved_text() {
    assert!(buffer_text_matches_string(
        &helix_core::Rope::from_str("one\ntwo\n"),
        "one\ntwo\n"
    ));
    assert!(!buffer_text_matches_string(
        &helix_core::Rope::from_str("one\ntwo"),
        "one\ntwo\n"
    ));
}

#[test]
fn diagnostic_picker_path_label_prefers_project_relative_path() {
    let project_root = Path::new("/workspace/nucleotide");
    let diagnostic_path = project_root.join("crates/nucleotide/src/main.rs");

    assert_eq!(
        diagnostic_picker_path_label(Some(&diagnostic_path), Some(project_root)),
        "crates/nucleotide/src/main.rs"
    );
}

#[test]
fn diagnostic_picker_path_label_handles_scratch_documents() {
    assert_eq!(diagnostic_picker_path_label(None, None), "[scratch]");
}

#[test]
fn diagnostic_severity_label_matches_picker_terms() {
    assert_eq!(diagnostic_severity_label(Severity::Error), "error");
    assert_eq!(diagnostic_severity_label(Severity::Warning), "warning");
    assert_eq!(diagnostic_severity_label(Severity::Info), "info");
    assert_eq!(diagnostic_severity_label(Severity::Hint), "hint");
}

#[test]
fn workspace_diagnostic_refresh_parses_as_typed_method_call() {
    let method_call = helix_lsp::MethodCall::parse(
        "workspace/diagnostic/refresh",
        helix_lsp::jsonrpc::Params::None,
    )
    .expect("workspace diagnostic refresh should be supported");

    assert!(matches!(
        method_call,
        helix_lsp::MethodCall::WorkspaceDiagnosticRefresh
    ));
}

#[test]
fn str_prefix_at_byte_limit_uses_utf8_boundary() {
    assert_eq!(str_prefix_at_byte_limit("abc", 2), "ab");
    assert_eq!(str_prefix_at_byte_limit("éclair", 1), "");
    assert_eq!(str_prefix_at_byte_limit("éclair", 2), "é");
}

#[test]
fn char_index_for_line_col_clamps_to_valid_document_position() {
    let text = Rope::from_str("éx\nlast");

    assert_eq!(char_index_for_line_col(text.slice(..), 0, 1), 1);
    assert_eq!(char_index_for_line_col(text.slice(..), 0, 100), 3);
    assert_eq!(
        char_index_for_line_col(text.slice(..), 99, 0),
        text.len_chars()
    );
}

fn test_gui_config() -> crate::config::Config {
    let mut gui = crate::config::GuiConfig::default();
    gui.lsp.project_lsp_startup = false;
    crate::config::Config {
        helix: HelixConfig::default(),
        gui,
    }
}

fn test_handlers() -> Handlers {
    let _runtime = TEST_RUNTIME.enter();
    let (completion_tx, _) = tokio::sync::mpsc::channel(1);
    let (signature_tx, _) = tokio::sync::mpsc::channel(1);
    let (auto_save_tx, _) = tokio::sync::mpsc::channel(1);
    let (doc_colors_tx, _) = tokio::sync::mpsc::channel(1);
    let (doc_links_tx, _) = tokio::sync::mpsc::channel(1);
    let (pull_diagnostics_tx, _) = tokio::sync::mpsc::channel(1);
    let (pull_all_diagnostics_tx, _) = tokio::sync::mpsc::channel(1);
    let (code_action_hint_tx, _) = tokio::sync::mpsc::channel(1);

    Handlers {
        completions: helix_view::handlers::completion::CompletionHandler::new(completion_tx),
        signature_hints: signature_tx,
        auto_save: auto_save_tx,
        document_colors: doc_colors_tx,
        document_links: doc_links_tx,
        word_index: helix_view::handlers::word_index::Handler::spawn(),
        pull_diagnostics: pull_diagnostics_tx,
        pull_all_documents_diagnostics: pull_all_diagnostics_tx,
        code_action_hint: code_action_hint_tx,
    }
}

fn new_test_editor() -> helix_view::Editor {
    let _runtime = TEST_RUNTIME.enter();
    let mut helix_config = HelixConfig::default();
    helix_config.editor.lsp.enable = false;
    let helix_config = Arc::new(ArcSwap::from_pointee(helix_config));
    let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
    let theme_loader = Arc::new(theme::Loader::new(&[]));

    helix_view::Editor::new(
        Rect::new(0, 0, 80, 24),
        theme_loader,
        syntax_loader,
        Arc::new(Map::new(
            Arc::clone(&helix_config),
            |config: &HelixConfig| &config.editor,
        )),
        test_handlers(),
        helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
    )
}

#[derive(Debug)]
struct RemoteIdentityLocalBackend;

impl RemoteIdentityLocalBackend {
    fn delegate() -> WorkspaceBackendHandle {
        local_workspace_backend()
    }
}

#[async_trait::async_trait]
impl WorkspaceBackend for RemoteIdentityLocalBackend {
    fn identity(&self) -> WorkspaceIdentity {
        WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Other("local-service".to_string()),
            name: "local-service".to_string(),
        })
    }

    async fn stat(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        Self::delegate().stat(path).await
    }

    async fn list_dir(&self, path: &Path) -> nucleotide_workspace::Result<DirectoryListing> {
        Self::delegate().list_dir(path).await
    }

    async fn find_ancestor_file(
        &self,
        start: &Path,
        file_name: &str,
        limit: usize,
    ) -> nucleotide_workspace::Result<Option<PathBuf>> {
        Self::delegate()
            .find_ancestor_file(start, file_name, limit)
            .await
    }

    async fn create_file(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        Self::delegate().create_file(path).await
    }

    async fn create_dir(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        Self::delegate().create_dir(path).await
    }

    async fn rename_path(&self, from: &Path, to: &Path) -> nucleotide_workspace::Result<FileStat> {
        Self::delegate().rename_path(from, to).await
    }

    async fn delete_path(&self, path: &Path) -> nucleotide_workspace::Result<FileStat> {
        Self::delegate().delete_path(path).await
    }

    async fn copy_path(&self, from: &Path, to: &Path) -> nucleotide_workspace::Result<FileStat> {
        Self::delegate().copy_path(from, to).await
    }

    async fn read_file(
        &self,
        path: &Path,
        options: ReadOptions,
    ) -> nucleotide_workspace::Result<nucleotide_workspace::FileRead> {
        Self::delegate().read_file(path, options).await
    }

    async fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        options: WriteOptions,
    ) -> nucleotide_workspace::Result<nucleotide_workspace::WriteResult> {
        Self::delegate().write_file(path, bytes, options).await
    }

    async fn file_search(
        &self,
        query: FileSearchQuery,
    ) -> nucleotide_workspace::Result<FileSearchResult> {
        Self::delegate().file_search(query).await
    }

    async fn text_search(
        &self,
        query: TextSearchQuery,
    ) -> nucleotide_workspace::Result<TextSearchResult> {
        Self::delegate().text_search(query).await
    }

    async fn project_environment(
        &self,
        root: &Path,
    ) -> nucleotide_workspace::Result<ProjectEnvironmentSnapshot> {
        Ok(ProjectEnvironmentSnapshot {
            root: root.to_path_buf(),
            variables: BTreeMap::from([(
                "NUCLEOTIDE_TEST_REMOTE_ENV".to_string(),
                "remote".to_string(),
            )]),
            origin: ProjectEnvironmentOrigin::Cli,
            diagnostics: Vec::new(),
        })
    }

    async fn git_head(&self, root: &Path) -> nucleotide_workspace::Result<GitHeadResult> {
        Self::delegate().git_head(root).await
    }

    async fn git_status(
        &self,
        root: &Path,
        options: GitStatusOptions,
    ) -> nucleotide_workspace::Result<GitStatusResult> {
        Self::delegate().git_status(root, options).await
    }

    async fn run_process(&self, spec: ProcessSpec) -> nucleotide_workspace::Result<ProcessOutput> {
        Self::delegate().run_process(spec).await
    }
}

#[test]
fn project_environment_provider_follows_workspace_backend_switches() {
    let project = tempdir().unwrap();
    let provider = ProjectEnvironmentProvider::with_workspace_backend(
        Arc::new(nucleotide_env::ProjectEnvironment::new(Some(
            HashMap::from([("NUCLEOTIDE_TEST_LOCAL_ENV".to_string(), "local".to_string())]),
        ))),
        local_workspace_backend(),
    );

    let local_environment = TEST_RUNTIME
        .block_on(nucleotide_lsp::EnvironmentProvider::get_lsp_environment(
            &provider,
            project.path(),
        ))
        .unwrap();
    assert_eq!(
        local_environment.get("NUCLEOTIDE_TEST_LOCAL_ENV"),
        Some(&"local".to_string())
    );

    provider.set_workspace_backend(Arc::new(RemoteIdentityLocalBackend));
    let remote_environment = TEST_RUNTIME
        .block_on(nucleotide_lsp::EnvironmentProvider::get_lsp_environment(
            &provider,
            project.path(),
        ))
        .unwrap();

    assert_eq!(
        remote_environment.get("NUCLEOTIDE_TEST_REMOTE_ENV"),
        Some(&"remote".to_string())
    );
    assert!(!remote_environment.contains_key("NUCLEOTIDE_TEST_LOCAL_ENV"));
}

#[test]
fn open_workspace_document_reads_remote_content_through_backend() {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src").join("main.rs"), "fn remote() {}\n").unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src").join("main.rs");
    let backend = path_mapped_workspace_backend(
        local_workspace_backend(),
        WorkspacePathMapping::new(display_root, temp.path()),
    );
    let mut editor = new_test_editor();
    editor.set_save_handler(Some(Arc::new(WorkspaceDocumentSaveHandler::new(
        backend.clone(),
    ))));

    let doc_id = open_workspace_document(
        &mut editor,
        &backend,
        &display_path,
        helix_view::editor::Action::VerticalSplit,
    )
    .unwrap();

    let doc = editor.document(doc_id).unwrap();
    assert_eq!(doc.path(), Some(display_path.as_path()));
    assert_eq!(
        doc.url().map(|url| url.to_string()),
        Some("file:///home/me/project/src/main.rs".to_string())
    );
    assert_eq!(
        doc.uri()
            .and_then(|uri| uri.as_path().map(Path::to_path_buf)),
        Some(display_path.clone())
    );
    assert_eq!(editor.language_servers.iter_clients().count(), 0);
    assert_eq!(doc.text(), "fn remote() {}\n");
    assert!(!doc.is_modified());
    let initial_file_version = doc.file_version().expect("open file version").to_vec();

    let view_id = editor.tree.focus;
    {
        let tree = &mut editor.tree;
        let documents = &mut editor.documents;
        let view = tree.get_mut(view_id);
        let doc = documents.get_mut(&doc_id).unwrap();
        let transaction =
            Transaction::change(doc.text(), [(0, 0, Some("updated\n".into()))].into_iter());
        doc.apply(&transaction, view_id);
        doc.append_changes_to_history(view);
    }

    editor.save::<PathBuf>(doc_id, None, false).unwrap();
    TEST_RUNTIME.block_on(editor.flush_writes()).unwrap();

    assert_eq!(
        fs::read_to_string(temp.path().join("src").join("main.rs")).unwrap(),
        "updated\nfn remote() {}\n"
    );
    let doc = editor.document(doc_id).unwrap();
    assert!(!doc.is_modified());
    assert_ne!(
        doc.file_version().expect("saved file version"),
        initial_file_version
    );
}

#[test]
fn remote_placeholders_hydrate_in_place_without_stealing_focus() {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src").join("first.rs"), "fn first() {}\n").unwrap();
    fs::write(
        temp.path().join("src").join("second.rs"),
        "fn second() {}\n",
    )
    .unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let first_path = display_root.join("src").join("first.rs");
    let second_path = display_root.join("src").join("second.rs");
    let backend = path_mapped_workspace_backend(
        local_workspace_backend(),
        WorkspacePathMapping::new(display_root, temp.path()),
    );
    let mut editor = new_test_editor();

    let first_doc_id = open_loading_workspace_document(
        &mut editor,
        &first_path,
        helix_view::editor::Action::VerticalSplit,
    )
    .unwrap();
    let view_id = editor.tree.focus;
    let second_doc_id = open_loading_workspace_document(
        &mut editor,
        &second_path,
        helix_view::editor::Action::Replace,
    )
    .unwrap();

    assert_eq!(editor.tree.get(view_id).doc, second_doc_id);
    assert_eq!(editor.document(first_doc_id).unwrap().text().len_bytes(), 0);
    assert!(editor.document(first_doc_id).unwrap().readonly);

    let first_read =
        TEST_RUNTIME.block_on(read_workspace_document(backend.clone(), first_path.clone()));
    let hydrated_view_id = hydrate_workspace_document_from_read(
        &mut editor,
        first_doc_id,
        view_id,
        first_read.unwrap(),
    )
    .unwrap();

    assert_eq!(hydrated_view_id, view_id);
    assert_eq!(editor.tree.get(view_id).doc, second_doc_id);
    let first_doc = editor.document(first_doc_id).unwrap();
    assert_eq!(first_doc.path(), Some(first_path.as_path()));
    assert_eq!(first_doc.text(), "fn first() {}\n");
    assert_eq!(
        first_doc
            .selection(view_id)
            .primary()
            .cursor(first_doc.text().slice(..)),
        0
    );
    assert!(!first_doc.readonly);
    assert!(!first_doc.is_modified());
    assert!(first_doc.file_version().is_some());
}

#[gpui::test]
async fn remote_lsp_navigation_readiness_is_false_without_capable_server(
    cx: &mut gpui::TestAppContext,
) {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src").join("main.rs"), "fn remote() {}\n").unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src").join("main.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, temp.path()),
    );
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.project_directory = Some(display_root);
        app.workspace_backend = backend.clone();
        open_workspace_document(
            &mut app.editor,
            &backend,
            &display_path,
            helix_view::editor::Action::VerticalSplit,
        )
        .unwrap();

        assert!(
            !app.active_document_lsp_navigation_readiness(
                NativeLspNavigationRequest::GotoDefinition,
            )
            .0
        );
    });
}

#[test]
fn native_open_file_with_backend_loads_remote_file() {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src").join("main.rs"), "fn remote() {}\n").unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src").join("main.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, temp.path()),
    );

    let outcome =
        TEST_RUNTIME.block_on(native_open_file_with_backend(backend, display_path.clone()));

    match outcome.unwrap() {
        NativeOpenFileOutcome::Document(document_read) => {
            assert_eq!(document_read.read.path, display_path);
            assert_eq!(document_read.read.bytes, b"fn remote() {}\n");
        }
        NativeOpenFileOutcome::Directory(path) => {
            panic!(
                "expected document outcome, got directory {}",
                path.display()
            );
        }
    }
}

#[test]
fn native_open_file_with_backend_reports_remote_directory() {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, temp.path()),
    );

    let outcome =
        TEST_RUNTIME.block_on(native_open_file_with_backend(backend, display_path.clone()));

    match outcome.unwrap() {
        NativeOpenFileOutcome::Directory(path) => {
            assert_eq!(path, display_path);
        }
        NativeOpenFileOutcome::Document(_) => panic!("expected directory outcome"),
    }
}

#[gpui::test]
async fn remote_workspace_edit_applies_text_changes_through_backend(cx: &mut gpui::TestAppContext) {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    let native_path = temp.path().join("src").join("main.rs");
    fs::write(&native_path, "fn remote() {}\n").unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src").join("main.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, temp.path()),
    );
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.project_directory = Some(display_root.clone());
        app.set_workspace_backend(backend.clone());

        let edit = lsp::WorkspaceEdit::new(HashMap::from([(
            lsp::Url::parse("file:///home/me/project/src/main.rs").unwrap(),
            vec![lsp::TextEdit::new(
                lsp::Range::new(lsp::Position::new(0, 3), lsp::Position::new(0, 9)),
                "edited".to_string(),
            )],
        )]));

        app.apply_lsp_workspace_edit(OffsetEncoding::Utf8, &edit)
            .unwrap();

        let doc = app
            .editor
            .document_by_path(&display_path)
            .expect("workspace edit opens display-backed document");
        assert_eq!(doc.text(), "fn edited() {}\n");
        assert_eq!(
            doc.url().as_ref().map(|url| url.as_str()),
            Some("file:///home/me/project/src/main.rs")
        );
        assert!(!display_path.exists());
    });

    assert_eq!(
        fs::read_to_string(native_path).unwrap(),
        "fn remote() {}\n",
        "workspace edits update the buffer but do not implicitly save"
    );
}

#[gpui::test]
async fn remote_workspace_edit_queue_polls_backend_without_blocking_gpui(
    cx: &mut gpui::TestAppContext,
) {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn remote() {}\n").unwrap();
    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src/main.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, temp.path()),
    );
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.project_directory = Some(display_root);
        app.set_workspace_backend(backend);
        app.enqueue_lsp_workspace_edit(
            helix_lsp::LanguageServerId::default(),
            helix_lsp::jsonrpc::Id::Num(1),
            OffsetEncoding::Utf8,
            lsp::WorkspaceEdit::new(HashMap::from([(
                lsp::Url::parse("file:///home/me/project/src/main.rs").unwrap(),
                vec![lsp::TextEdit::new(
                    lsp::Range::new(lsp::Position::new(0, 3), lsp::Position::new(0, 9)),
                    "queued".to_string(),
                )],
            )])),
        );
    });

    for _ in 0..100 {
        let complete = app.update(cx, |app, _cx| {
            let _runtime = TEST_RUNTIME.enter();
            let waker = futures_util::task::noop_waker();
            let mut task_cx = std::task::Context::from_waker(&waker);
            app.poll_pending_lsp_workspace_edits(&mut task_cx);
            app.pending_lsp_workspace_edits.is_empty()
        });
        if complete {
            break;
        }
        tokio::task::yield_now().await;
    }

    app.read_with(cx, |app, _cx| {
        assert!(app.pending_lsp_workspace_edits.is_empty());
        let doc = app
            .editor
            .document_by_path(&display_path)
            .expect("queued edit opens document");
        assert_eq!(doc.text(), "fn queued() {}\n");
    });
}

#[gpui::test]
async fn remote_workspace_edit_applies_resource_ops_through_backend(cx: &mut gpui::TestAppContext) {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    let native_old = temp.path().join("src").join("old.rs");
    let native_new = temp.path().join("src").join("renamed.rs");
    let native_created = temp.path().join("src").join("created.rs");
    fs::write(&native_old, "pub fn old() {}\n").unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_old = display_root.join("src").join("old.rs");
    let display_new = display_root.join("src").join("renamed.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, temp.path()),
    );
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.project_directory = Some(display_root.clone());
        app.set_workspace_backend(backend.clone());
        let doc_id = open_workspace_document(
            &mut app.editor,
            &backend,
            &display_old,
            helix_view::editor::Action::VerticalSplit,
        )
        .unwrap();

        let edit = lsp::WorkspaceEdit {
            changes: None,
            document_changes: Some(lsp::DocumentChanges::Operations(vec![
                lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Create(lsp::CreateFile {
                    uri: lsp::Url::parse("file:///home/me/project/src/created.rs").unwrap(),
                    options: None,
                    annotation_id: None,
                })),
                lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(lsp::RenameFile {
                    old_uri: lsp::Url::parse("file:///home/me/project/src/old.rs").unwrap(),
                    new_uri: lsp::Url::parse("file:///home/me/project/src/renamed.rs").unwrap(),
                    options: None,
                    annotation_id: None,
                })),
                lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Delete(lsp::DeleteFile {
                    uri: lsp::Url::parse("file:///home/me/project/src/created.rs").unwrap(),
                    options: None,
                })),
            ])),
            change_annotations: None,
        };

        app.apply_lsp_workspace_edit(OffsetEncoding::Utf8, &edit)
            .unwrap();

        let doc = app.editor.document(doc_id).expect("open renamed document");
        assert_eq!(doc.path(), Some(display_new.as_path()));
        assert_eq!(
            doc.url().as_ref().map(|url| url.as_str()),
            Some("file:///home/me/project/src/renamed.rs")
        );
        assert_eq!(doc.text(), "pub fn old() {}\n");
        assert!(doc.file_version().is_some());
        assert!(!display_new.exists());
    });

    assert!(!native_old.exists());
    assert!(native_new.exists());
    assert!(!native_created.exists());
    assert_eq!(fs::read_to_string(native_new).unwrap(), "pub fn old() {}\n");
}

#[test]
fn open_workspace_document_uses_backend_for_service_identity() {
    let temp = tempdir().unwrap();
    let temp_root = temp.path().canonicalize().unwrap();
    let native_root = temp_root.join("native");
    let display_root = temp_root.join("display");
    let native_path = native_root.join("src").join("main.rs");
    fs::create_dir_all(native_path.parent().unwrap()).unwrap();
    fs::write(&native_path, "fn service_backed() {}\n").unwrap();

    let display_path = display_root.join("src").join("main.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, &native_root),
    );
    let mut editor = new_test_editor();
    editor.set_save_handler(Some(Arc::new(WorkspaceDocumentSaveHandler::new(
        backend.clone(),
    ))));

    let doc_id = open_workspace_document(
        &mut editor,
        &backend,
        &display_path,
        helix_view::editor::Action::VerticalSplit,
    )
    .unwrap();

    let doc = editor.document(doc_id).unwrap();
    assert_same_path(doc.path().unwrap(), &display_path);
    assert_eq!(doc.text(), "fn service_backed() {}\n");
    assert!(!display_path.exists());

    let view_id = editor.tree.focus;
    {
        let tree = &mut editor.tree;
        let documents = &mut editor.documents;
        let view = tree.get_mut(view_id);
        let doc = documents.get_mut(&doc_id).unwrap();
        let transaction =
            Transaction::change(doc.text(), [(0, 0, Some("updated\n".into()))].into_iter());
        doc.apply(&transaction, view_id);
        doc.append_changes_to_history(view);
    }

    editor.save::<PathBuf>(doc_id, None, false).unwrap();
    TEST_RUNTIME.block_on(editor.flush_writes()).unwrap();

    assert_eq!(
        fs::read_to_string(&native_path).unwrap(),
        "updated\nfn service_backed() {}\n"
    );
    assert!(!display_path.exists());
}

#[test]
fn settings_file_save_uses_native_handler_even_in_remote_workspace() {
    let settings_path = helix_loader::config_dir().join("nucleotide.toml");
    let project_settings_path = Path::new("/remote/project/nucleotide.toml");

    assert!(should_use_native_save_for_settings_file(
        Some(&settings_path),
        None
    ));
    assert!(should_use_native_save_for_settings_file(
        Some(&settings_path),
        Some(Path::new("nucleotide-copy.toml")),
    ));
    assert!(should_use_native_save_for_settings_file(
        None,
        Some(&settings_path)
    ));
    assert!(!should_use_native_save_for_settings_file(
        Some(project_settings_path),
        None
    ));
}

#[test]
fn open_workspace_document_sets_remote_diff_base_from_git() {
    let temp = tempdir().unwrap();
    init_git_repo(temp.path());
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src").join("main.rs"), "fn remote() {}\n").unwrap();
    run_git(temp.path(), &["add", "src/main.rs"]);
    run_git(temp.path(), &["commit", "-m", "initial"]);
    fs::write(
        temp.path().join("src").join("main.rs"),
        "fn remote() {\n    changed();\n}\n",
    )
    .unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src").join("main.rs");
    let backend = path_mapped_workspace_backend(
        local_workspace_backend(),
        WorkspacePathMapping::new(display_root, temp.path()),
    );
    let mut editor = new_test_editor();
    let _runtime = TEST_RUNTIME.enter();

    let doc_id = open_workspace_document(
        &mut editor,
        &backend,
        &display_path,
        helix_view::editor::Action::VerticalSplit,
    )
    .unwrap();

    let doc = editor.document(doc_id).unwrap();
    assert!(doc.diff_handle().is_some());
}

#[test]
fn navigation_display_path_maps_remote_native_paths_to_display_root() {
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Ssh,
        name: "example.com".to_string(),
    });
    let project_root = Path::new("ssh://me@example.com/home/me/project");

    assert_eq!(
        navigation_display_path(
            Path::new("/home/me/project/src/main.rs"),
            Some(project_root),
            &identity,
        ),
        PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs")
    );
    assert_eq!(
        navigation_display_path(
            Path::new("ssh://me@example.com/home/me/project/src/main.rs"),
            Some(project_root),
            &identity,
        ),
        PathBuf::from("ssh://me@example.com/home/me/project/src/main.rs")
    );
    assert_eq!(
        navigation_display_path(
            Path::new("/nix/store/rust/src/lib.rs"),
            Some(project_root),
            &identity,
        ),
        PathBuf::from("ssh://me@example.com/nix/store/rust/src/lib.rs")
    );
}

#[gpui::test]
async fn jump_to_global_search_location_uses_workspace_backend_for_remote_path(
    cx: &mut gpui::TestAppContext,
) {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src").join("main.rs"), "one\ntwo\nthree\n").unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src").join("main.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, temp.path()),
    );
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.workspace_backend = backend;
        let (doc_id, view_id) = app
            .jump_to_global_search_location(&crate::types::GlobalSearchLocation {
                path: display_path.clone(),
                line: 1,
            })
            .expect("remote global search jump");
        let doc = app.editor.document(doc_id).expect("opened remote doc");

        assert_eq!(doc.path(), Some(display_path.as_path()));
        assert_eq!(doc.text(), "one\ntwo\nthree\n");
        assert_eq!(
            doc.selection(view_id)
                .primary()
                .cursor_line(doc.text().slice(..)),
            1
        );
    });

    assert!(!display_path.exists());
}

#[gpui::test]
async fn jump_to_lsp_location_maps_remote_native_path_to_display_path(
    cx: &mut gpui::TestAppContext,
) {
    let temp = tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src").join("main.rs"), "one\ntwo\n").unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = display_root.join("src").join("main.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        WorkspacePathMapping::new(&display_root, temp.path()),
    );
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.project_directory = Some(display_root.clone());
        app.workspace_backend = backend;
        let (doc_id, view_id) = app
            .jump_to_lsp_location(&crate::types::LspLocation {
                path: PathBuf::from("/home/me/project/src/main.rs"),
                range: lsp::Range::new(lsp::Position::new(1, 0), lsp::Position::new(1, 1)),
                offset_encoding: OffsetEncoding::Utf8,
            })
            .expect("remote LSP jump");
        let doc = app.editor.document(doc_id).expect("opened remote doc");

        assert_eq!(doc.path(), Some(display_path.as_path()));
        assert_eq!(
            doc.selection(view_id)
                .primary()
                .cursor_line(doc.text().slice(..)),
            1
        );
    });

    assert!(!display_path.exists());
}

#[gpui::test]
async fn jump_to_lsp_location_maps_external_remote_native_path_to_same_remote(
    cx: &mut gpui::TestAppContext,
) {
    let temp = tempdir().unwrap();
    let temp_root = temp.path().canonicalize().unwrap();
    let project_backing_root = temp_root.join("home").join("me").join("project");
    let external_backing_root = temp_root.join("cargo").join("git");
    fs::create_dir_all(project_backing_root.join("src")).unwrap();
    fs::create_dir_all(&external_backing_root).unwrap();
    fs::write(external_backing_root.join("lib.rs"), "zero\none\n").unwrap();

    let display_root = PathBuf::from("ssh://me@example.com/home/me/project");
    let display_path = PathBuf::from("ssh://me@example.com/cargo/git/lib.rs");
    let external_native_path = PathBuf::from("/cargo/git/lib.rs");
    let backend = path_mapped_workspace_backend(
        Arc::new(RemoteIdentityLocalBackend),
        // Map the simulated remote filesystem root onto the temporary local
        // tree so this fixture remains valid on Windows hosts as well as Unix.
        WorkspacePathMapping::new("ssh://me@example.com/", &temp_root),
    );
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.project_directory = Some(display_root.clone());
        app.workspace_backend = backend;
        let (doc_id, view_id) = app
            .jump_to_lsp_location(&crate::types::LspLocation {
                path: external_native_path.clone(),
                range: lsp::Range::new(lsp::Position::new(1, 0), lsp::Position::new(1, 1)),
                offset_encoding: OffsetEncoding::Utf8,
            })
            .expect("external remote LSP jump");
        let doc = app.editor.document(doc_id).expect("opened remote doc");

        assert_eq!(doc.path(), Some(display_path.as_path()));
        assert_eq!(
            doc.selection(view_id)
                .primary()
                .cursor_line(doc.text().slice(..)),
            1
        );
    });

    assert!(!display_path.exists());
}

fn init_git_repo(root: &Path) {
    run_git(root, &["init"]);
    run_git(root, &["config", "user.email", "nucleotide@example.test"]);
    run_git(root, &["config", "user.name", "Nucleotide Tests"]);
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn new_test_application(cx: &mut gpui::TestAppContext) -> Entity<Application> {
    cx.new(|cx| {
        let _runtime = TEST_RUNTIME.enter();
        let helix_config = Arc::new(ArcSwap::from_pointee(HelixConfig::default()));
        let syntax_loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let theme_loader = Arc::new(theme::Loader::new(&[]));
        let editor = helix_view::Editor::new(
            Rect::new(0, 0, 80, 24),
            theme_loader,
            syntax_loader,
            Arc::new(Map::new(
                Arc::clone(&helix_config),
                |config: &HelixConfig| &config.editor,
            )),
            test_handlers(),
            helix_loader::workspace_trust::WorkspaceTrust::fully_trusted(),
        );
        let compositor = Compositor::new(Rect::new(0, 0, 80, 24));
        let native_keymaps = Keymaps::default();
        let editor_input = EditorInputBridge::new(native_keymaps);
        let gui_config = test_gui_config();
        let mut cli_env = HashMap::new();
        cli_env.insert("HOME".to_string(), "/tmp".to_string());

        let mut app = Application {
            editor,
            compositor,
            editor_input,
            jobs: Jobs::new(),
            lsp_progress: LspProgressMap::new(),
            lsp_state: None,
            project_directory: None,
            workspace_backend: local_workspace_backend(),
            helix_event_rx: None,
            config: gui_config,
            helix_config_arc: helix_config,
            project_lsp: ProjectLspCoordinator::new_initialized(),
            project_environment: Arc::new(nucleotide_env::ProjectEnvironment::new(Some(cli_env))),
            workspace_file_ops: crate::application::WorkspaceFileOpHandler::new(
                local_workspace_backend(),
                TEST_RUNTIME.handle().clone(),
            ),
            pending_lsp_workspace_edits: std::collections::VecDeque::new(),
            terminal_runtime: crate::application::TerminalRuntimeHandle::new(),
            maintenance_wake: None,
        };
        app.lsp_state = Some(cx.new(|_cx| nucleotide_lsp::LspState::new()));
        app
    })
}

#[gpui::test]
fn completion_overlay_passes_outside_mouse_input_to_editor(cx: &mut gpui::TestAppContext) {
    use crate::overlay::{OverlayView, WorkspaceLayoutInfo};
    use gpui::{
        Context, FocusHandle, InteractiveElement, IntoElement, Modifiers, MouseButton,
        ParentElement, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
        StatefulInteractiveElement, Styled, Window, div, point, px, size,
    };

    struct Harness {
        overlay: Entity<OverlayView>,
        focus: FocusHandle,
        scroll: ScrollHandle,
        clicks: usize,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .id("editor")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .track_focus(&self.focus)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.clicks += 1;
                                window.focus(&this.focus, cx);
                            }),
                        )
                        .children((0..100).map(|i| div().h(px(24.0)).child(format!("line {i}")))),
                )
                .child(self.overlay.clone())
        }
    }

    let _runtime = TEST_RUNTIME.enter();
    let core = new_test_application(cx);
    cx.update(|cx| {
        cx.set_global(nucleotide_ui::Theme::default());
        cx.set_global(WorkspaceLayoutInfo {
            cursor_position: Some(point(px(60.0), px(40.0))),
            cursor_size: Some(size(px(10.0), px(20.0))),
            line_height: px(20.0),
            ..Default::default()
        });
    });
    let (harness, cx) = cx.add_window_view(|window, cx| {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let overlay = cx.new(|_| OverlayView::new(&focus, &core, TEST_RUNTIME.handle().clone()));
        let completion = cx.new(|cx| {
            let mut view = nucleotide_ui::CompletionView::new(cx);
            view.set_items(
                (0..12)
                    .map(|i| nucleotide_ui::CompletionItem::new(format!("item-{i}")))
                    .collect(),
                cx,
            );
            view
        });
        overlay.update(cx, |overlay, cx| {
            overlay.handle_event(&crate::Update::Completion(completion), cx)
        });
        Harness {
            overlay,
            focus,
            scroll: ScrollHandle::new(),
            clicks: 0,
        }
    });
    cx.refresh().unwrap();

    // Popup wheel events must not scroll the editor underneath it.
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(90.0), px(90.0)),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-48.0))),
        ..Default::default()
    });
    harness.read_with(cx, |harness, _| {
        assert_eq!(harness.scroll.offset().y, px(0.0))
    });
    // Clicking popup chrome must not dismiss it or place the editor cursor.
    cx.simulate_click(point(px(90.0), px(270.0)), Modifiers::none());
    harness.read_with(cx, |harness, cx| {
        assert_eq!(harness.clicks, 0);
        assert!(!harness.overlay.read(cx).is_empty());
    });

    let outside = point(px(90.0), px(400.0));
    cx.simulate_event(ScrollWheelEvent {
        position: outside,
        delta: ScrollDelta::Pixels(point(px(0.0), px(-48.0))),
        ..Default::default()
    });
    harness.read_with(cx, |harness, cx| {
        assert!(harness.scroll.offset().y < px(0.0));
        assert!(!harness.overlay.read(cx).is_empty());
    });
    cx.simulate_click(outside, Modifiers::none());
    cx.update(|window, cx| {
        let harness = harness.read(cx);
        assert_eq!(harness.clicks, 1);
        assert!(harness.overlay.read(cx).is_empty());
        assert!(harness.focus.is_focused(window));
    });
}

#[gpui::test]
fn editor_native_composition_preserves_unicode_multicursors_and_undo(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::EntityInputHandler;
    use helix_core::{Range, Selection};
    use helix_view::document::Mode;
    use std::str::FromStr;

    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("native.txt");
    fs::write(&path, "a😀b|cdé\n").unwrap();
    let core = new_test_application(cx);
    let view_id = core.update(cx, |core, _| {
        core.editor.open(&path, Action::VerticalSplit).unwrap();
        core.editor.mode = Mode::Insert;
        let view_id = core.editor.tree.focus;
        let doc_id = core.editor.tree.get(view_id).doc;
        core.editor
            .documents
            .get_mut(&doc_id)
            .unwrap()
            .set_selection(
                view_id,
                Selection::new(vec![Range::point(1), Range::point(5)].into(), 1),
            );
        view_id
    });
    let view = cx.new(|cx| {
        crate::document::DocumentView::new(
            core.clone(),
            None,
            view_id,
            gpui::TextStyle::default(),
            &cx.focus_handle(),
            true,
        )
    });
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        view.update(cx, |view, cx| {
            let mut actual = None;
            // A surrogate-interior offset must not slice an emoji in half.
            assert_eq!(
                view.text_for_range(2..4, &mut actual, window, cx)
                    .as_deref(),
                Some("😀b")
            );
            assert_eq!(actual, Some(1..4));
            view.replace_and_mark_text_in_range(None, "ni", None, window, cx);
            view.replace_and_mark_text_in_range(None, "你好😀", Some(2..4), window, cx);
            assert_eq!(view.marked_text_range(window, cx), Some(10..14));
            assert_eq!(
                view.selected_text_range(false, window, cx).unwrap().range,
                12..14
            );
            view.replace_and_mark_text_in_range(Some(12..14), "é", Some(0..1), window, cx);
            assert_eq!(view.marked_text_range(window, cx), Some(9..12));
            let mut actual = None;
            assert_eq!(
                view.text_for_range(9..12, &mut actual, window, cx)
                    .as_deref(),
                Some("你好é")
            );
            // Native cancellation restores the full original selection, not just primary.
            let marked = view.marked_text_range(window, cx);
            view.replace_text_in_range(marked, "", window, cx);
        });
    });
    window.update(|_, cx| {
        core.update(cx, |core, _| {
            let doc_id = core.editor.tree.get(view_id).doc;
            let doc = &core.editor.documents[&doc_id];
            assert_eq!(doc.text().to_string(), "a😀b|cdé\n");
            assert_eq!(
                doc.selection(view_id).ranges(),
                &[Range::new(1, 2), Range::new(5, 6)]
            );
        })
    });
    window.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "ni hao", None, window, cx);
            view.replace_text_in_range(None, "你好", window, cx);
        });
    });
    window.update(|_, cx| {
        core.update(cx, |core, _| {
            let doc_id = core.editor.tree.get(view_id).doc;
            assert_eq!(
                core.editor.documents[&doc_id].text().to_string(),
                "a你好😀b|c你好dé\n"
            );
            core.editor_input.handle_key(
                helix_view::input::KeyEvent::from_str("esc").unwrap(),
                &mut core.compositor,
                &mut core.editor,
                &mut core.jobs,
            );
            core.editor_input.handle_semantic_action(
                super::editor_input::EditorSemanticAction::Undo,
                &mut core.compositor,
                &mut core.editor,
                &mut core.jobs,
            );
            assert_eq!(
                core.editor.documents[&doc_id].text().to_string(),
                "a😀b|cdé\n"
            );
            core.editor.mode = Mode::Insert;
            core.editor
                .documents
                .get_mut(&doc_id)
                .unwrap()
                .set_selection(
                    view_id,
                    Selection::new(vec![Range::point(1), Range::point(5)].into(), 0),
                );
        })
    });
    window.update(|window, cx| {
        view.update(cx, |view, cx| {
            // Explicit native replacement uses UTF-16 and must not insert
            // another copy at an unrelated secondary cursor.
            view.replace_text_in_range(Some(1..3), "日", window, cx);
        });
    });
    window.update(|_, cx| {
        core.update(cx, |core, _| {
            let doc_id = core.editor.tree.get(view_id).doc;
            assert_eq!(
                core.editor.documents[&doc_id].text().to_string(),
                "a日b|cdé\n"
            );
        })
    });
    window.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(Some(1..2), "ni", None, window, cx);
            view.unmark_text(window, cx);
            assert_eq!(view.marked_text_range(window, cx), None);
        });
        core.update(cx, |core, _| {
            let doc_id = core.editor.tree.get(view_id).doc;
            assert_eq!(
                core.editor.documents[&doc_id].text().to_string(),
                "anib|cdé\n"
            );
        });
    });
    window.update(|window, cx| {
        view.update(cx, |view, cx| {
            // An outside replacement finalizes the old mark before starting
            // a new one. Cancelling the new mark must retain that commit.
            view.replace_and_mark_text_in_range(Some(1..3), "你好", None, window, cx);
            view.replace_and_mark_text_in_range(Some(3..4), "日", None, window, cx);
            let marked = view.marked_text_range(window, cx);
            assert_eq!(marked, Some(3..4));
            view.replace_text_in_range(marked, "", window, cx);
            assert_eq!(
                view.text_for_range(0..usize::MAX, &mut None, window, cx)
                    .as_deref(),
                Some("a你好b|cdé\n")
            );
            view.replace_text_in_range(Some(1..3), "ni", window, cx);
        });
    });
    window.update(|window, cx| {
        core.update(cx, |core, _| {
            let doc_id = core.editor.tree.get(view_id).doc;
            core.editor
                .documents
                .get_mut(&doc_id)
                .unwrap()
                .set_selection(
                    view_id,
                    Selection::new(
                        vec![Range::point(1), Range::point(2), Range::point(5)].into(),
                        1,
                    ),
                );
        });
        view.update(cx, |view, cx| {
            // Deletion merges cursors inside the replaced span. The
            // primary index must come from Helix's mapped selection.
            view.replace_text_in_range(Some(1..3), "", window, cx);
        });
        core.read_with(cx, |core, _| {
            let doc_id = core.editor.tree.get(view_id).doc;
            let doc = &core.editor.documents[&doc_id];
            assert_eq!(doc.text().to_string(), "ab|cdé\n");
            assert_eq!(doc.selection(view_id).primary_index(), 0);
            assert_eq!(
                doc.selection(view_id).ranges(),
                &[Range::new(1, 2), Range::new(3, 4)]
            );
        });
    });
    window.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "ni hao", None, window, cx);
        });
        let replacement = root.path().join("replacement.txt");
        fs::write(&replacement, "other document\n").unwrap();
        let original_doc = core.update(cx, |core, _| {
            let original_doc = core.editor.tree.get(view_id).doc;
            core.editor.open(&replacement, Action::Replace).unwrap();
            core.editor.mode = Mode::Insert;
            original_doc
        });
        // File-tree preview can replace the document before GPUI delivers
        // blur. Native cancellation still belongs to the original buffer.
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "", None, window, cx);
            assert_eq!(view.marked_text_range(window, cx), None);
        });
        core.read_with(cx, |core, _| {
            assert_eq!(
                core.editor.documents[&original_doc].text().to_string(),
                "ab|cdé\n"
            );
            let replacement_doc = core.editor.tree.get(view_id).doc;
            assert_eq!(
                core.editor.documents[&replacement_doc].text().to_string(),
                "other document\n"
            );
        });
    });
}

#[gpui::test]
fn editor_native_composition_rejects_stale_document_callbacks(cx: &mut gpui::TestAppContext) {
    use gpui::EntityInputHandler;
    use helix_view::document::Mode;

    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let original = root.path().join("original.txt");
    let replacement = root.path().join("replacement.txt");
    fs::write(&original, "longer😀original\n").unwrap();
    fs::write(&replacement, "x\n").unwrap();

    for removed in ["neither", "original", "current", "view"] {
        for callback in ["commit", "replace", "mark", "replace_mark", "unmark"] {
            let core = new_test_application(cx);
            let (view_id, original_doc) = core.update(cx, |core, _| {
                let doc_id = core.editor.open(&original, Action::VerticalSplit).unwrap();
                core.editor.mode = Mode::Insert;
                (core.editor.tree.focus, doc_id)
            });
            let view = cx.new(|cx| {
                crate::document::DocumentView::new(
                    core.clone(),
                    None,
                    view_id,
                    gpui::TextStyle::default(),
                    &cx.focus_handle(),
                    true,
                )
            });
            let window = cx.add_empty_window();
            window.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.replace_and_mark_text_in_range(Some(8..12), "候補😀", None, window, cx);
                });
                let current_doc = core.update(cx, |core, _| {
                    let doc_id = core.editor.open(&replacement, Action::Replace).unwrap();
                    match removed {
                        "original" => {
                            assert!(core.editor.close_document(original_doc, true).is_ok());
                        }
                        "current" => {
                            core.editor.documents.remove(&doc_id);
                        }
                        "view" => core.editor.close(view_id),
                        _ => {}
                    }
                    core.editor.mode = Mode::Insert;
                    doc_id
                });
                view.update(cx, |view, cx| {
                    assert!(view.selected_text_range(true, window, cx).is_none());
                    assert_eq!(view.marked_text_range(window, cx), None);
                    match callback {
                        "commit" => view.replace_text_in_range(None, "日", window, cx),
                        "replace" => view.replace_text_in_range(Some(8..12), "日", window, cx),
                        "mark" => view.replace_and_mark_text_in_range(None, "日", None, window, cx),
                        "replace_mark" => {
                            view.replace_and_mark_text_in_range(Some(8..12), "日", None, window, cx)
                        }
                        "unmark" => view.unmark_text(window, cx),
                        _ => unreachable!(),
                    }
                    assert!(!view.has_native_composition(), "{removed}: {callback}");
                    assert_eq!(view.marked_text_range(window, cx), None);
                    // Reject subsequent OS callbacks until the native input reset.
                    view.replace_text_in_range(None, "late", window, cx);
                });
                core.read_with(cx, |core, _| {
                    if removed != "current" {
                        assert_eq!(
                            core.editor.documents[&current_doc].text().to_string(),
                            "x\n",
                            "{removed}: {callback} must not modify the replacement buffer"
                        );
                    }
                    if removed != "original" && removed != "view" {
                        assert_eq!(
                            core.editor.documents[&original_doc].text().to_string(),
                            "longer😀original\n",
                            "{removed}: {callback} must restore the original buffer"
                        );
                    }
                });
            });
        }
    }
}

fn new_test_workspace<'a>(
    cx: &'a mut gpui::TestAppContext,
    path: &Path,
) -> (
    Entity<Application>,
    Entity<crate::workspace::Workspace>,
    &'a mut gpui::VisualTestContext,
) {
    new_test_workspace_with_runtime(cx, path, TEST_RUNTIME.handle().clone())
}

fn new_test_workspace_with_runtime<'a>(
    cx: &'a mut gpui::TestAppContext,
    path: &Path,
    handle: tokio::runtime::Handle,
) -> (
    Entity<Application>,
    Entity<crate::workspace::Workspace>,
    &'a mut gpui::VisualTestContext,
) {
    use crate::workspace::Workspace;
    use nucleotide_ui::FocusCoordinator;

    let _runtime = handle.enter();
    let core = new_test_application(cx);
    core.update(cx, |core, _cx| {
        let _runtime = handle.enter();
        core.workspace_file_ops = crate::application::WorkspaceFileOpHandler::new(
            local_workspace_backend(),
            handle.clone(),
        );
        core.project_directory = Some(path.parent().unwrap().to_path_buf());
        core.editor.open(path, Action::VerticalSplit).unwrap();
    });
    cx.update(|cx| {
        nucleotide_appearance::SystemAppearance::init(cx);
        nucleotide_ui::init(cx, None);
        let config = &core.read(cx).config;
        let editor_font = config.editor_font();
        let ui_font = config.ui_font();
        let font = nucleotide_types::Font {
            family: editor_font.family.clone(),
            weight: editor_font.weight,
            style: nucleotide_types::FontStyle::Normal,
        };
        cx.set_global(crate::types::FontSettings {
            fixed_font: font.clone(),
            var_font: font,
        });
        cx.set_global(crate::types::EditorFontConfig {
            family: editor_font.family,
            size: editor_font.size,
            weight: editor_font.weight,
            line_height: editor_font.line_height,
            ligatures: editor_font.ligatures,
        });
        cx.set_global(crate::types::UiFontConfig {
            family: ui_font.family,
            size: ui_font.size,
            weight: ui_font.weight,
        });
        let theme = crate::ThemeManager::new(core.read(cx).editor.theme.clone());
        cx.set_global(theme.ui_theme().clone());
        cx.set_global(theme);
        cx.set_global(FocusCoordinator::default());
        let vcs = nucleotide_vcs::VcsServiceHandle::new(Default::default(), cx);
        cx.set_global(vcs);
    });
    let input = cx.new(|_| crate::Input);
    let core_for_input = core.clone();
    let input_handle = handle.clone();
    cx.update(|cx| {
        cx.subscribe(&input, move |_, event: &crate::InputEvent, cx| {
            core_for_input.update(cx, |core, cx| {
                core.handle_input_event(event.clone(), cx, input_handle.clone());
            });
        })
        .detach();
    });
    let core_for_workspace = core.clone();
    let workspace_handle = handle.clone();
    let (workspace, cx) = cx.add_window_view(move |window, cx| {
        let _runtime = workspace_handle.enter();
        let overlay = cx.new(|cx| {
            let view = crate::overlay::OverlayView::new(
                &cx.focus_handle(),
                &core_for_workspace,
                workspace_handle.clone(),
            );
            view.subscribe(&core_for_workspace, cx);
            view
        });
        let notifications = cx.new(|_| crate::notification::NotificationView::new());
        let info = cx.new(|_| crate::info_box::InfoBoxView::new());
        let updates = cx.new(|cx| {
            crate::updates::UpdateController::new(
                crate::config::UpdatesConfig {
                    enabled: false,
                    ..Default::default()
                },
                cx,
            )
        });
        Workspace::with_views(
            core_for_workspace,
            input,
            workspace_handle.clone(),
            overlay,
            notifications,
            info,
            Arc::new(crate::input_coordinator::InputCoordinator::new()),
            updates,
            window,
            cx,
        )
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (core, workspace, cx)
}

#[gpui::test]
fn external_file_drop_opens_files_only_on_submit(cx: &mut gpui::TestAppContext) {
    use gpui::{ExternalPaths, FileDropEvent};

    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    // Windows temp paths can contain 8.3 aliases; Helix expands them.
    let root_path = helix_stdx::path::canonicalize(root.path());
    let original = root_path.join("original.txt");
    let first = root_path.join("\u{e9}cole \u{6771}\u{4eac}.txt");
    let last = root_path.join("second file.txt");
    let missing = root_path.join("missing.txt");
    fs::write(&original, "original\n").unwrap();
    fs::write(&first, "first dropped document\n").unwrap();
    fs::write(&last, "second dropped document\n").unwrap();
    let (core, _workspace, cx) = new_test_workspace(cx, &original);
    let position = cx.update(|window, cx| {
        window.draw(cx).clear();
        let size = window.viewport_size();
        gpui::point(size.width / 2.0, size.height / 2.0)
    });
    let paths = ExternalPaths(
        [
            first.clone(),
            root_path.clone(),
            missing.clone(),
            last.clone(),
        ]
        .into_iter()
        .collect(),
    );
    let initial_count = core.read_with(cx, |core, _| core.editor.documents.len());

    // Entering, moving, and leaving must not open files or leave a stale drag.
    cx.simulate_event(FileDropEvent::Entered {
        position,
        paths: paths.clone(),
    });
    cx.simulate_event(FileDropEvent::Pending { position });
    cx.simulate_event(FileDropEvent::Exited);
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents.len(), initial_count);
        let active = core.editor.tree.get(core.editor.tree.focus).doc;
        assert_eq!(
            core.editor.documents[&active].path(),
            Some(original.as_path())
        );
    });

    // Drop over the default sidebar's occluding resize handle first, then
    // repeat over the editor to verify existing buffers are reused.
    for position in [gpui::point(gpui::px(240.0), position.y), position] {
        cx.simulate_event(FileDropEvent::Entered {
            position,
            paths: paths.clone(),
        });
        cx.update(|window, cx| {
            window.draw(cx).clear();
        });
        cx.simulate_event(FileDropEvent::Submit { position });
        cx.run_until_parked();
        core.read_with(cx, |core, _| {
            assert_eq!(core.editor.documents.len(), initial_count + 2);
            assert_eq!(core.project_directory.as_deref(), Some(root_path.as_path()));
            for (path, expected) in [
                (&original, "original\n"),
                (&first, "first dropped document\n"),
                (&last, "second dropped document\n"),
            ] {
                let doc = core
                    .editor
                    .documents
                    .values()
                    .find(|doc| doc.path() == Some(path))
                    .unwrap();
                assert_eq!(doc.text().to_string(), expected);
            }
            let active = core.editor.tree.get(core.editor.tree.focus).doc;
            assert_eq!(core.editor.documents[&active].path(), Some(last.as_path()));
        });
    }

    // A local OS path must never be sent to the remote workspace backend.
    let local_only = root_path.join("local only.txt");
    fs::write(&local_only, "local contents\n").unwrap();
    core.update(cx, |core, _| {
        core.set_workspace_backend(Arc::new(RemoteIdentityLocalBackend));
    });
    cx.simulate_event(FileDropEvent::Entered {
        position,
        paths: ExternalPaths([local_only].into_iter().collect()),
    });
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    cx.simulate_event(FileDropEvent::Submit { position });
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents.len(), initial_count + 2);
        let active = core.editor.tree.get(core.editor.tree.focus).doc;
        assert_eq!(core.editor.documents[&active].path(), Some(last.as_path()));
    });
}

#[gpui::test]
fn tab_copy_actions_preserve_unicode_paths(cx: &mut gpui::TestAppContext) {
    use crate::workspace::TabContextMenuIntent;
    use gpui::{Modifiers, MouseButton};

    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let file_name = "\u{e9}cole \u{6771}\u{4eac}.txt";
    let path = helix_stdx::path::canonicalize(root.path()).join(file_name);
    fs::write(&path, "clipboard fixture\n").unwrap();
    let (core, _workspace, cx) = new_test_workspace(cx, &path);
    let (pane, doc) = core.update(cx, |core, cx| {
        let mut config = (**core.helix_config_arc.load()).clone();
        config.editor.bufferline = helix_view::editor::BufferLine::Always;
        core.helix_config_arc.store(Arc::new(config));
        let view = core.editor.tree.get(core.editor.tree.focus);
        let result = (view.id, view.doc);
        cx.emit(crate::Update::Redraw);
        result
    });
    let tab = Box::leak(format!("pane-tab-{pane:?}-{doc}").into_boxed_str());

    for (intent, expected) in [
        (TabContextMenuIntent::CopyPath, path.display().to_string()),
        (
            TabContextMenuIntent::CopyRelativePath,
            file_name.to_string(),
        ),
    ] {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into()));
        cx.update(|window, cx| {
            window.draw(cx).clear();
        });
        let position = cx.debug_bounds(tab).unwrap().center();
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::none());
        cx.run_until_parked();
        cx.dispatch_action(crate::actions::tab_menus::ContextOperation { intent });
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(expected)
        );
    }
}

#[gpui::test]
fn command_prompt_action_accepts_and_executes_native_commands(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("original.txt");
    fs::write(&path, "original document\n").unwrap();
    let (core, _workspace, cx) = new_test_workspace(cx, &path);
    let before_count = core.read_with(cx, |core, _| core.editor.documents.len());

    cx.dispatch_action(crate::actions::workspace::ShowCommandPrompt);
    cx.run_until_parked();
    cx.simulate_keystrokes("n e w enter");
    cx.run_until_parked();

    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents.len(), before_count + 1);
        let view = core.editor.tree.get(core.editor.tree.focus);
        assert!(core.editor.documents[&view.doc].path().is_none());
        assert!(
            core.editor.documents[&view.doc]
                .text()
                .to_string()
                .trim()
                .is_empty()
        );
    });
}

#[gpui::test]
fn file_name_prompts_do_not_complete_or_capture_commands(cx: &mut gpui::TestAppContext) {
    use crate::actions::file_tree;
    use nucleotide_ui::FocusCoordinator;

    // File operations and the tree watcher use real filesystem I/O.
    cx.background_executor.allow_parking();
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
    );
    // GPUI drains detached tasks after the test function returns.
    let runtime_for_app = runtime.clone();
    cx.update(|cx| {
        cx.on_app_quit(move |_| {
            let runtime = runtime_for_app.clone();
            async move { drop(runtime) }
        })
        .detach();
    });
    let _runtime = runtime.enter();
    for operation in ["new-file", "new-folder", "rename", "duplicate"] {
        for outcome in ["cancel", "replace", "submit"] {
            let root = tempdir().unwrap();
            let root_path = helix_stdx::path::canonicalize(root.path());
            let path = root_path.join("original.txt");
            fs::write(&path, "original document\n").unwrap();
            let (core, workspace, cx) =
                new_test_workspace_with_runtime(cx, &path, runtime.handle().clone());
            // Drive Tokio as well as GPUI while the file tree loads.
            for _ in 0..100 {
                runtime.block_on(async {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                });
                cx.run_until_parked();
                workspace.update_in(cx, |workspace, window, cx| {
                    workspace.focus_file_tree(window, cx)
                });
                match operation {
                    "new-file" => cx.dispatch_action(file_tree::NewFile),
                    "new-folder" => cx.dispatch_action(file_tree::NewFolder),
                    "rename" => cx.dispatch_action(file_tree::Rename),
                    "duplicate" => cx.dispatch_action(file_tree::Duplicate),
                    _ => unreachable!(),
                }
                if cx.update(|_, cx| cx.global::<FocusCoordinator>().prompt_focus().is_some()) {
                    break;
                }
            }
            cx.run_until_parked();
            cx.update(|window, cx| {
                assert!(
                    cx.global::<FocusCoordinator>()
                        .prompt_focus()
                        .expect("file name prompt opened")
                        .is_focused(window)
                );
            });
            // `w` is a command-completion prefix. Tab must leave this file
            // name untouched, and one Escape must dismiss the prompt.
            let select_all_and_type_w = if cfg!(target_os = "macos") {
                "cmd-a w tab"
            } else {
                "ctrl-a w tab"
            };
            cx.simulate_keystrokes(select_all_and_type_w);
            if outcome == "submit" {
                cx.simulate_keystrokes("enter");
                let target = root_path.join("w");
                for _ in 0..100 {
                    cx.run_until_parked();
                    if target.exists() {
                        break;
                    }
                    runtime.block_on(async {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    });
                }
                assert!(target.exists(), "{operation} must use the literal name w");
                match operation {
                    "new-file" => assert_eq!(fs::read_to_string(&target).unwrap(), ""),
                    "new-folder" => assert!(target.is_dir()),
                    "rename" | "duplicate" => {
                        assert_eq!(fs::read_to_string(&target).unwrap(), "original document\n");
                        assert_eq!(path.exists(), operation == "duplicate");
                    }
                    _ => unreachable!(),
                }
            } else {
                if outcome == "cancel" {
                    cx.simulate_keystrokes("escape");
                    cx.run_until_parked();
                    cx.update(|_, cx| {
                        assert!(cx.global::<FocusCoordinator>().prompt_focus().is_none());
                    });
                }
                let before = core.read_with(cx, |core, _| core.editor.documents.len());
                cx.dispatch_action(crate::actions::workspace::ShowCommandPrompt);
                cx.run_until_parked();
                cx.simulate_keystrokes("n e w enter");
                cx.run_until_parked();
                core.read_with(cx, |core, _| {
                    assert_eq!(
                        core.editor.documents.len(),
                        before + 1,
                        "{operation}: {outcome}"
                    );
                    let view = core.editor.tree.get(core.editor.tree.focus);
                    assert!(core.editor.documents[&view.doc].path().is_none());
                });
                assert_eq!(fs::read_to_string(&path).unwrap(), "original document\n");
                assert!(!root.path().join("new").exists());
                assert!(!root.path().join("w").exists());
            }
            cx.update(|window, _| window.remove_window());
            drop(workspace);
            drop(core);
            cx.run_until_parked();
        }
    }
}

#[gpui::test]
fn opening_directory_closes_file_buffers_but_preserves_scratch(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let alpha_path = root.path().join("alpha.txt");
    let beta_path = root.path().join("beta.txt");
    let next_directory = root.path().join("next");
    fs::write(&alpha_path, "alpha\n").unwrap();
    fs::write(&beta_path, "beta\n").unwrap();
    fs::create_dir(&next_directory).unwrap();
    let (core, _workspace, cx) = new_test_workspace(cx, &alpha_path);
    let scratch = core.update(cx, |core, cx| {
        // Exercise the common directory-open path without changing the test
        // process's working directory (which is shared by other tests).
        core.set_workspace_backend(Arc::new(RemoteIdentityLocalBackend));
        core.editor.open(&beta_path, Action::VerticalSplit).unwrap();
        let scratch = core.editor.new_file(Action::VerticalSplit);
        let view = core.editor.tree.focus;
        let doc = core.editor.documents.get_mut(&scratch).unwrap();
        let transaction = Transaction::change(
            doc.text(),
            std::iter::once((0, 0, Some("scratch notes".into()))),
        );
        doc.apply(&transaction, view);
        cx.emit(crate::Update::OpenDirectory(next_directory.clone()));
        scratch
    });
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.project_directory.as_ref(), Some(&next_directory));
        assert_eq!(core.editor.documents.len(), 1);
        assert_eq!(
            core.editor.documents[&scratch].text().to_string(),
            format!(
                "scratch notes{}",
                helix_core::line_ending::NATIVE_LINE_ENDING.as_str()
            )
        );
        assert!(core.editor.documents[&scratch].is_modified());
        assert!(
            core.editor
                .tree
                .views()
                .all(|(view, _)| view.doc == scratch)
        );
    });

    // With no existing scratch, closing the last file must still leave
    // a usable editor containing a fresh scratch buffer.
    core.update(cx, |core, cx| {
        assert!(core.editor.close_document(scratch, true).is_ok());
        let empty_scratch = core.editor.tree.get(core.editor.tree.focus).doc;
        core.editor.open(&alpha_path, Action::Replace).unwrap();
        assert!(!core.editor.documents.contains_key(&empty_scratch));
        cx.emit(crate::Update::OpenDirectory(next_directory.clone()));
    });
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.documents.len(), 1);
        let doc = core.editor.documents.values().next().unwrap();
        assert!(doc.path().is_none());
        assert!(!doc.is_modified());
        assert!(core.editor.tree.views().next().is_some());
    });
}

#[gpui::test]
fn opening_directory_waits_for_unsaved_buffer_confirmation(cx: &mut gpui::TestAppContext) {
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let alpha_path = root.path().join("alpha.txt");
    let beta_path = root.path().join("beta.txt");
    let next_directory = root.path().join("next");
    fs::write(&alpha_path, "alpha\n").unwrap();
    fs::write(&beta_path, "beta\n").unwrap();
    fs::create_dir(&next_directory).unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &alpha_path);
    let dirty = core.update(cx, |core, _| {
        core.set_workspace_backend(Arc::new(RemoteIdentityLocalBackend));
        core.editor.open(&beta_path, Action::Load).unwrap();
        core.editor.tree.get(core.editor.tree.focus).doc
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("i");
    cx.simulate_input("changed");
    cx.simulate_keystrokes("escape");
    core.update(cx, |_, cx| {
        cx.emit(crate::Update::OpenDirectory(next_directory.clone()))
    });
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.project_directory.as_deref(), Some(root.path()));
        assert_eq!(core.editor.documents.len(), 2);
        assert!(core.editor.documents[&dirty].is_modified());
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.project_directory.as_deref(), Some(root.path()));
        assert_eq!(core.editor.documents.len(), 2);
    });

    core.update(cx, |_, cx| {
        cx.emit(crate::Update::OpenDirectory(next_directory.clone()))
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.project_directory.as_ref(), Some(&next_directory));
        assert_eq!(core.editor.documents.len(), 1);
        assert!(
            core.editor
                .documents
                .values()
                .all(|doc| doc.path().is_none())
        );
    });
    assert_eq!(fs::read_to_string(alpha_path).unwrap(), "alpha\n");
}

#[gpui::test]
fn split_tabs_transfer_through_drag_and_drop(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let alpha_path = root.path().join("alpha.txt");
    let beta_path = root.path().join("beta.txt");
    fs::write(&alpha_path, "alpha\n").unwrap();
    fs::write(&beta_path, "beta\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &alpha_path);
    let (left, alpha, beta) = core.update(cx, |core, cx| {
        let mut config = (**core.helix_config_arc.load()).clone();
        config.editor.bufferline = helix_view::editor::BufferLine::Always;
        core.helix_config_arc.store(Arc::new(config));
        let left = core.editor.tree.focus;
        let alpha = core.editor.tree.get(left).doc;
        let beta = core.editor.open(&beta_path, Action::Load).unwrap();
        cx.emit(crate::Update::Redraw);
        (left, alpha, beta)
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    cx.simulate_keystrokes("ctrl-w v");
    let right = core.read_with(cx, |core, _| core.editor.tree.focus);
    assert_ne!(left, right);
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });

    // Selectors locate public tab hitboxes; clicks and drops exercise the
    // actual callbacks rather than calling workspace transfer helpers.
    let alpha_left = Box::leak(format!("pane-tab-{:?}-{}", left, alpha).into_boxed_str());
    let beta_left = Box::leak(format!("pane-tab-{:?}-{}", left, beta).into_boxed_str());
    let alpha_right = Box::leak(format!("pane-tab-{:?}-{}", right, alpha).into_boxed_str());
    let beta_right = Box::leak(format!("pane-tab-{:?}-{}", right, beta).into_boxed_str());
    let start = cx.debug_bounds(beta_left).unwrap().center();
    let end = cx.debug_bounds(alpha_right).unwrap().center();
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    cx.simulate_mouse_move(
        start + point(px(20.0), px(0.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 2);
        assert_eq!(core.editor.tree.get(left).doc, alpha);
        assert_eq!(core.editor.tree.get(right).doc, beta);
    });
    assert!(cx.debug_bounds(beta_left).is_none());
    assert!(cx.debug_bounds(beta_right).is_some());
    assert!(cx.debug_bounds(alpha_right).is_some());

    // A click in the unfocused pane targets that pane, not the destination.
    let position = cx.debug_bounds(alpha_left).unwrap().center();
    cx.simulate_click(position, Modifiers::none());
    core.read_with(cx, |core, _| assert_eq!(core.editor.tree.focus, left));

    // Moving its last tab onto an existing copy closes only the empty pane.
    let start = cx.debug_bounds(alpha_left).unwrap().center();
    let end = cx.debug_bounds(alpha_right).unwrap().center();
    cx.simulate_mouse_move(start, None, Modifiers::none());
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        start + point(px(20.0), px(0.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 1);
        assert_eq!(core.editor.tree.get(right).doc, alpha);
        assert_eq!(core.editor.documents[&beta].text().to_string(), "beta\n");
    });

    // Closing a shared dirty tab removes only that pane's copy; closing
    // the last copy must keep its unsaved buffer until confirmed.
    cx.simulate_keystrokes("i");
    cx.simulate_input("changed");
    cx.simulate_keystrokes("escape ctrl-w v");
    let copy = core.read_with(cx, |core, _| core.editor.tree.focus);
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    let alpha_copy = Box::leak(format!("pane-tab-{:?}-{}", copy, alpha).into_boxed_str());
    let point = cx.debug_bounds(alpha_copy).unwrap().center();
    cx.simulate_mouse_down(point, MouseButton::Middle, Modifiers::none());
    cx.simulate_mouse_up(point, MouseButton::Middle, Modifiers::none());
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 1);
        assert!(core.editor.documents[&alpha].is_modified());
    });
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    let point = cx.debug_bounds(alpha_right).unwrap().center();
    cx.simulate_mouse_down(point, MouseButton::Middle, Modifiers::none());
    cx.simulate_mouse_up(point, MouseButton::Middle, Modifiers::none());
    cx.run_until_parked();
    core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.get(right).doc, alpha);
        assert_eq!(
            core.editor.documents[&alpha].text().to_string(),
            "changedalpha\n"
        );
    });
}

#[gpui::test]
fn pane_edge_drag_previews_and_splits_only_the_destination(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let alpha_path = root.path().join("alpha.txt");
    let beta_path = root.path().join("beta.txt");
    fs::write(&alpha_path, "alpha\n").unwrap();
    fs::write(&beta_path, "beta\n").unwrap();

    for (horizontal, before) in [(true, true), (true, false), (false, true), (false, false)] {
        let (core, workspace, cx) = new_test_workspace(cx, &alpha_path);
        let (source, alpha, beta) = core.update(cx, |core, cx| {
            let mut config = (**core.helix_config_arc.load()).clone();
            config.editor.bufferline = helix_view::editor::BufferLine::Always;
            core.helix_config_arc.store(Arc::new(config));
            let source = core.editor.tree.focus;
            let alpha = core.editor.tree.get(source).doc;
            let beta = core.editor.open(&beta_path, Action::Load).unwrap();
            cx.emit(crate::Update::Redraw);
            (source, alpha, beta)
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        let (target, sibling) = core.update(cx, |core, cx| {
            core.editor.switch(alpha, Action::VerticalSplit);
            let target = core.editor.tree.focus;
            core.editor.switch(alpha, Action::VerticalSplit);
            let sibling = core.editor.tree.focus;
            cx.emit(crate::Update::Redraw);
            (target, sibling)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.focus_editor(window, cx)
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        };
        draw(cx);
        let tab = Box::leak(format!("pane-tab-{source:?}-{beta}").into_boxed_str());
        let target_selector = Box::leak(format!("editor-pane-{target:?}").into_boxed_str());
        let source_body = Box::leak(format!("pane-drop-body-{source:?}").into_boxed_str());
        let target_body = Box::leak(format!("pane-drop-body-{target:?}").into_boxed_str());
        let start = cx.debug_bounds(tab).unwrap().center();
        let bounds = cx.debug_bounds(target_selector).unwrap();
        let body = cx.debug_bounds(target_body).unwrap();
        let end = if horizontal {
            point(
                if before {
                    bounds.left() + px(8.0)
                } else {
                    bounds.right() - px(8.0)
                },
                body.center().y,
            )
        } else {
            point(
                bounds.center().x,
                if before {
                    body.top() + px(8.0)
                } else {
                    bounds.bottom() - px(8.0)
                },
            )
        };
        let original = core.read_with(cx, |core, _| {
            [source, target, sibling].map(|id| core.editor.tree.get(id).area)
        });
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            start + point(px(20.0), px(0.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(cx);
        // This tall, narrow pane distinguishes normalized edge distance
        // from pixel distance: the latter would incorrectly choose left.
        cx.simulate_mouse_move(
            bounds.origin + point(bounds.size.width * 0.2, bounds.size.height * 0.175),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(cx);
        let normalized_preview = cx.debug_bounds("pane-drop-preview").unwrap();
        assert!((normalized_preview.size.width - bounds.size.width).abs() < px(1.0));
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
        draw(cx);
        let preview = cx.debug_bounds("pane-drop-preview").unwrap();
        assert!(
            (preview.size.width - bounds.size.width / if horizontal { 2.0 } else { 1.0 }).abs()
                < px(1.0)
        );
        assert!(
            (preview.size.height - bounds.size.height / if horizontal { 1.0 } else { 2.0 }).abs()
                < px(1.0)
        );
        assert!(
            (preview.left()
                - if horizontal && !before {
                    bounds.center().x
                } else {
                    bounds.left()
                })
            .abs()
                < px(1.0)
        );
        assert!(
            (preview.top()
                - if !horizontal && !before {
                    bounds.center().y
                } else {
                    bounds.top()
                })
            .abs()
                < px(1.0)
        );
        core.read_with(cx, |core, _| {
            assert_eq!(core.editor.tree.views().count(), 3);
            assert_eq!(core.editor.tree.get(target).area, original[1]);
        });

        // Leaving the target and cancelling cannot change the layout or
        // leave a stale shaded rectangle behind. The multi-tab source
        // also offers a split preview without changing the layout.
        cx.simulate_mouse_move(
            point(px(5.0), px(5.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(cx);
        assert!(cx.debug_bounds("pane-drop-preview").is_none());
        let source_center = cx.debug_bounds(source_body).unwrap().center();
        cx.simulate_mouse_move(source_center, MouseButton::Left, Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("pane-drop-preview").is_some());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("pane-drop-preview").is_none());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        core.read_with(cx, |core, _| {
            assert_eq!(core.editor.tree.views().count(), 3)
        });

        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(
            start + point(px(20.0), px(0.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(cx);
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
        draw(cx);
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        draw(cx);
        assert!(cx.debug_bounds("pane-drop-preview").is_none());
        let destination = core.read_with(cx, |core, _| {
            let destination = core.editor.tree.focus;
            assert_eq!(core.editor.tree.views().count(), 4);
            assert_eq!(core.editor.tree.get(source).area, original[0]);
            assert_eq!(core.editor.tree.get(sibling).area, original[2]);
            assert_eq!(core.editor.tree.get(source).doc, alpha);
            assert_eq!(core.editor.tree.get(target).doc, alpha);
            assert_eq!(core.editor.tree.get(destination).doc, beta);
            let moved = core.editor.tree.get(destination).area;
            let remaining = core.editor.tree.get(target).area;
            if horizontal {
                assert_eq!(moved.height, original[1].height);
                assert!(moved.width.abs_diff(remaining.width) <= 1);
                assert_eq!(moved.x < remaining.x, before);
            } else {
                assert_eq!(moved.width, original[1].width);
                assert!(moved.height.abs_diff(remaining.height) <= 1);
                assert_eq!(moved.y < remaining.y, before);
            }
            destination
        });
        assert!(cx.debug_bounds(tab).is_none());
        let moved_tab = Box::leak(format!("pane-tab-{destination:?}-{beta}").into_boxed_str());
        let retained_tab = Box::leak(format!("pane-tab-{source:?}-{alpha}").into_boxed_str());
        assert!(cx.debug_bounds(moved_tab).is_some());
        assert!(cx.debug_bounds(retained_tab).is_some());
    }
}

#[gpui::test]
fn pane_tab_drag_creates_first_split(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let alpha_path = root.path().join("alpha.txt");
    let beta_path = root.path().join("beta.txt");
    let gamma_path = root.path().join("gamma.txt");
    fs::write(&alpha_path, "alpha\n").unwrap();
    fs::write(&beta_path, "beta\n").unwrap();
    fs::write(&gamma_path, "gamma\n").unwrap();

    for (horizontal, before) in [(true, true), (true, false), (false, true), (false, false)] {
        for active in [false, true] {
            let (core, workspace, cx) = new_test_workspace(cx, &alpha_path);
            let (source, alpha, beta, gamma) = core.update(cx, |core, cx| {
                let mut config = (**core.helix_config_arc.load()).clone();
                config.editor.bufferline = helix_view::editor::BufferLine::Always;
                core.helix_config_arc.store(Arc::new(config));
                let source = core.editor.tree.focus;
                let alpha = core.editor.tree.get(source).doc;
                let beta = core.editor.open(&beta_path, Action::Load).unwrap();
                let gamma = core.editor.open(&gamma_path, Action::Load).unwrap();
                if active {
                    core.editor.switch(beta, Action::Replace);
                }
                cx.emit(crate::Update::Redraw);
                (source, alpha, beta, gamma)
            });
            workspace.update_in(cx, |workspace, window, cx| {
                workspace.focus_editor(window, cx)
            });
            let draw = |cx: &mut gpui::VisualTestContext| {
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear());
            };
            draw(cx);
            let pane = Box::leak(format!("editor-pane-{source:?}").into_boxed_str());
            let bounds = cx.debug_bounds(pane).unwrap();
            let end = if horizontal {
                point(
                    if before {
                        bounds.left() + px(8.0)
                    } else {
                        bounds.right() - px(8.0)
                    },
                    bounds.center().y,
                )
            } else {
                point(
                    bounds.center().x,
                    if before {
                        bounds.top() + px(80.0)
                    } else {
                        bounds.bottom() - px(8.0)
                    },
                )
            };
            let lone_tab = Box::leak(format!("pane-tab-{source:?}-{alpha}").into_boxed_str());
            let tab = Box::leak(format!("pane-tab-{source:?}-{beta}").into_boxed_str());
            let start = cx.debug_bounds(tab).unwrap().center();
            cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(
                start + point(px(20.0), px(0.0)),
                MouseButton::Left,
                Modifiers::none(),
            );
            draw(cx);
            cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
            draw(cx);
            let preview = cx.debug_bounds("pane-drop-preview").unwrap();
            assert!(
                (preview.size.width - bounds.size.width / if horizontal { 2.0 } else { 1.0 }).abs()
                    < px(1.0)
            );
            assert!(
                (preview.size.height - bounds.size.height / if horizontal { 1.0 } else { 2.0 })
                    .abs()
                    < px(1.0)
            );
            core.read_with(cx, |core, _| {
                assert_eq!(core.editor.tree.views().count(), 1)
            });
            cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
            draw(cx);
            assert!(cx.debug_bounds("pane-drop-preview").is_none());
            let destination = core.read_with(cx, |core, _| {
                assert_eq!(core.editor.tree.views().count(), 2);
                let destination = core.editor.tree.focus;
                assert_ne!(destination, source);
                assert_eq!(core.editor.tree.get(source).doc, alpha);
                assert_eq!(core.editor.tree.get(destination).doc, beta);
                let moved = core.editor.tree.get(destination).area;
                let remaining = core.editor.tree.get(source).area;
                if horizontal {
                    assert_eq!(moved.height, remaining.height);
                    assert!(moved.width.abs_diff(remaining.width) <= 1);
                    assert_eq!(moved.x < remaining.x, before);
                } else {
                    assert_eq!(moved.width, remaining.width);
                    assert!(moved.height.abs_diff(remaining.height) <= 1);
                    assert_eq!(moved.y < remaining.y, before);
                }
                destination
            });
            assert!(cx.debug_bounds(lone_tab).is_some());
            assert!(cx.debug_bounds(tab).is_none());
            let moved_tab = Box::leak(format!("pane-tab-{destination:?}-{beta}").into_boxed_str());
            assert!(cx.debug_bounds(moved_tab).is_some());

            // A lone tab cannot be transferred into a split of its own pane.
            let destination_pane =
                Box::leak(format!("editor-pane-{destination:?}").into_boxed_str());
            let start = cx.debug_bounds(moved_tab).unwrap().center();
            let end = cx.debug_bounds(destination_pane).unwrap().center();
            cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(
                start + point(px(20.0), px(0.0)),
                MouseButton::Left,
                Modifiers::none(),
            );
            draw(cx);
            cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
            draw(cx);
            assert!(cx.debug_bounds("pane-drop-preview").is_none());
            cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
            draw(cx);
            core.read_with(cx, |core, _| {
                assert_eq!(core.editor.tree.views().count(), 2)
            });

            // Closing the moved tab must close its split, even if that
            // view inherited history for documents in the source pane.
            let position = cx.debug_bounds(moved_tab).unwrap().center();
            cx.simulate_mouse_down(position, MouseButton::Middle, Modifiers::none());
            cx.simulate_mouse_up(position, MouseButton::Middle, Modifiers::none());
            draw(cx);
            core.read_with(cx, |core, _| {
                assert_eq!(core.editor.tree.views().count(), 1);
                assert!(!core.editor.tree.contains(destination));
                assert_eq!(core.editor.tree.get(source).doc, alpha);
                assert!(!core.editor.documents.contains_key(&beta));
                assert_eq!(core.editor.documents[&gamma].text().to_string(), "gamma\n");
            });
            assert!(cx.debug_bounds(destination_pane).is_none());
            assert!(cx.debug_bounds(moved_tab).is_none());
            assert!(cx.debug_bounds(lone_tab).is_some());
            let gamma_tab = Box::leak(format!("pane-tab-{source:?}-{gamma}").into_boxed_str());
            assert!(cx.debug_bounds(gamma_tab).is_some());

            // Closing the remaining tabs must leave the final editor pane.
            for tab in [&*gamma_tab, &*lone_tab] {
                let position = cx.debug_bounds(tab).unwrap().center();
                cx.simulate_mouse_down(position, MouseButton::Middle, Modifiers::none());
                cx.simulate_mouse_up(position, MouseButton::Middle, Modifiers::none());
                draw(cx);
                core.read_with(cx, |core, _| {
                    assert_eq!(core.editor.tree.views().count(), 1)
                });
            }
        }
    }
}

#[gpui::test]
fn workspace_focus_follows_splits_and_overlay_dismissal_without_stealing_tree_focus(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::Focusable;
    use nucleotide_ui::FocusCoordinator;
    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("focus.txt");
    fs::write(&path, "first line\nsecond line\n").unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_file_tree(window, cx)
    });
    // A non-focus update and redraw must leave the tree's keyboard receiver intact.
    core.update(cx, |_, cx| {
        cx.emit(crate::Update::Info(helix_view::info::Info {
            title: "hint".into(),
            text: "hint".into(),
            width: 4,
            height: 1,
        }));
        cx.emit(crate::Update::Redraw);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(
            cx.global::<FocusCoordinator>()
                .file_tree_focus()
                .unwrap()
                .is_focused(window)
        );
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_editor(window, cx)
    });
    // A completion may remain behind a prompt but must not block restoration.
    let completion = cx.new(nucleotide_ui::completion_v2::CompletionView::new);
    core.update(cx, |_, cx| cx.emit(crate::Update::Completion(completion)));
    cx.simulate_keystrokes(":");
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(
            cx.global::<FocusCoordinator>()
                .prompt_focus()
                .unwrap()
                .is_focused(window)
        );
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(
            cx.global::<FocusCoordinator>()
                .editor_focus()
                .unwrap()
                .is_focused(window)
        );
    });
    cx.simulate_keystrokes("ctrl-w v");
    cx.run_until_parked();
    let right_view = core.read_with(cx, |core, _| {
        assert_eq!(core.editor.tree.views().count(), 2);
        core.editor.tree.focus
    });
    let right_focus = cx.update(|_, cx| cx.global::<FocusCoordinator>().editor_focus().unwrap());
    cx.update(|window, cx| {
        assert!(
            cx.global::<FocusCoordinator>()
                .editor_focus()
                .unwrap()
                .is_focused(window)
        );
    });
    cx.simulate_keystrokes("ctrl-w h");
    cx.run_until_parked();
    core.read_with(cx, |core, _| assert_ne!(core.editor.tree.focus, right_view));
    cx.update(|window, cx| {
        assert!(
            cx.global::<FocusCoordinator>()
                .editor_focus()
                .unwrap()
                .is_focused(window)
        );
        assert!(workspace.focus_handle(cx).contains_focused(window, cx));
    });
    // GPUI traversal into a split must activate it even without a pointer event.
    cx.update(|window, cx| {
        window.focus(&right_focus, cx);
        window.draw(cx).clear();
    });
    cx.run_until_parked();
    core.read_with(cx, |core, _| assert_eq!(core.editor.tree.focus, right_view));

    // The terminal must retain focus even before its async model is registered.
    cx.dispatch_action(crate::actions::workspace::ToggleTerminal);
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(
            cx.global::<FocusCoordinator>()
                .terminal_focus()
                .unwrap()
                .is_focused(window)
        );
    });
    cx.dispatch_action(crate::actions::workspace::ToggleTerminal);
    cx.update(|window, cx| {
        window.draw(cx).clear();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(
            cx.global::<FocusCoordinator>()
                .editor_focus()
                .unwrap()
                .is_focused(window)
        );
    });
}

#[gpui::test]
fn native_completion_accepts_prefix_edits_but_rejects_unrelated_changes(
    cx: &mut gpui::TestAppContext,
) {
    use helix_core::{Selection, Transaction};
    use helix_view::document::Mode;
    use nucleotide_events::completion::{
        CompletionEdit, CompletionItem, CompletionItemKind, CompletionOffsetEncoding,
        CompletionPosition, CompletionRange, CompletionTextEdit,
    };

    let _runtime = TEST_RUNTIME.enter();
    let root = tempdir().unwrap();
    let path = root.path().join("completion.txt");
    let original = "😀:al|TAIL\n";
    fs::write(&path, original).unwrap();
    let (core, workspace, cx) = new_test_workspace(cx, &path);
    for case in ["typed", "backspace", "commit", "unrelated", "cursor"] {
        let (doc_id, view_id) = core.update(cx, |core, cx| {
            core.editor.mode = Mode::Insert;
            let view_id = core.editor.tree.focus;
            let doc_id = core.editor.tree.get(view_id).doc;
            let doc = core.editor.documents.get_mut(&doc_id).unwrap();
            let transaction = Transaction::change(
                doc.text(),
                [(0, doc.text().len_chars(), Some(original.into()))].into_iter(),
            )
            .with_selection(Selection::point(4));
            doc.apply(&transaction, view_id);
            cx.notify();
            (doc_id, view_id)
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.focus_editor(window, cx);
            let item = CompletionItem::new("alpineZ9".into(), CompletionItemKind::Variable)
                .with_commit_characters(vec![".".into()])
                .with_edit(CompletionEdit {
                    offset_encoding: CompletionOffsetEncoding::Utf16,
                    text_edit: Some(CompletionTextEdit {
                        range: CompletionRange {
                            start: CompletionPosition {
                                line: 0,
                                character: 3,
                            },
                            end: CompletionPosition {
                                line: 0,
                                character: 5,
                            },
                        },
                        new_text: "alpineZ9".into(),
                    }),
                    additional_text_edits: Vec::new(),
                });
            workspace.show_completion_items_with_prefix(
                vec![item],
                "al".into(),
                doc_id,
                view_id,
                false,
                Vec::new(),
                cx,
            );
        });
        cx.run_until_parked();
        match case {
            "typed" => cx.simulate_input("p"),
            "backspace" => cx.simulate_keystrokes("backspace"),
            "commit" => cx.simulate_input("p."),
            "unrelated" => {
                core.update(cx, |core, cx| {
                    let doc = core.editor.documents.get_mut(&doc_id).unwrap();
                    let transaction =
                        Transaction::change(doc.text(), [(5, 9, Some("EVIL".into()))].into_iter());
                    doc.apply(&transaction, view_id);
                    cx.notify();
                });
                cx.run_until_parked();
            }
            "cursor" => {
                core.update(cx, |core, _| {
                    core.editor
                        .documents
                        .get_mut(&doc_id)
                        .unwrap()
                        .set_selection(view_id, Selection::single(5, 6));
                });
            }
            _ => unreachable!(),
        }
        if case != "commit" {
            cx.simulate_keystrokes("tab");
        }
        let expected = match case {
            "commit" => "😀:alpineZ9.|TAIL\n",
            "unrelated" => "😀:al|EVIL\n",
            "cursor" => original,
            _ => "😀:alpineZ9|TAIL\n",
        };
        core.read_with(cx, |core, _| {
            assert_eq!(
                core.editor.documents[&doc_id].text().to_string(),
                expected,
                "{case}"
            );
        });
    }
}

fn subscribe_application_updates(
    cx: &mut gpui::TestAppContext,
    app: &Entity<Application>,
) -> Rc<RefCell<Vec<CapturedUpdate>>> {
    let updates = Rc::new(RefCell::new(Vec::new()));
    let updates_for_subscription = updates.clone();

    cx.update(|cx| {
        cx.subscribe(app, move |_app, update: &crate::Update, _cx| match update {
            crate::Update::Picker(crate::picker::Picker::Native { items, .. }) => {
                let severities = items
                    .iter()
                    .filter_map(|item| match &item.columns {
                        Some(crate::picker_view::ColumnData::Diagnostic { severity, .. }) => {
                            Some(*severity)
                        }
                        _ => None,
                    })
                    .collect();
                updates_for_subscription
                    .borrow_mut()
                    .push(CapturedUpdate::DiagnosticPicker(severities));
            }
            crate::Update::Lsp(nucleotide_events::LspEvent::ServerInitialized { server_id }) => {
                updates_for_subscription
                    .borrow_mut()
                    .push(CapturedUpdate::LspInitialized(*server_id));
            }
            crate::Update::EditorStatus(crate::types::EditorStatus {
                status: message,
                severity,
            }) => {
                updates_for_subscription
                    .borrow_mut()
                    .push(CapturedUpdate::StatusChanged(message.clone(), *severity));
            }
            _ => {}
        })
        .detach();
    });

    updates
}

#[gpui::test]
fn diagnostics_picker_keeps_out_of_bounds_diagnostics(cx: &mut gpui::TestAppContext) {
    use helix_core::diagnostic::{
        Diagnostic, DiagnosticProvider, LanguageServerId, Range as DiagnosticRange,
    };

    let app = new_test_application(cx);
    let updates = subscribe_application_updates(cx, &app);

    app.update(cx, |app, cx| {
        let doc_id = app.editor.new_file(Action::VerticalSplit);
        let diagnostic = Diagnostic {
            range: DiagnosticRange { start: 10, end: 10 },
            ends_at_word: false,
            starts_at_word: false,
            zero_width: true,
            line: 0,
            message: "diagnostic beyond current text".into(),
            severity: None,
            code: None,
            provider: DiagnosticProvider::Lsp {
                server_id: LanguageServerId::default(),
                identifier: None,
            },
            tags: Vec::new(),
            source: Some("test-lsp".into()),
            data: None,
        };
        app.editor
            .document_mut(doc_id)
            .expect("new document")
            .replace_diagnostics([diagnostic], &[], None);

        app.emit_diagnostics_picker(false, cx);
    });

    assert!(
        updates
            .borrow()
            .contains(&CapturedUpdate::DiagnosticPicker(vec![Severity::Warning]))
    );
}

fn run_event_driven_maintenance(cx: &mut gpui::TestAppContext, app: &Entity<Application>) {
    let handle = TEST_RUNTIME.handle().clone();
    let (wake, _wake_rx) = MaintenanceWake::channel();
    app.update(cx, |app, cx| {
        app.drive_event_driven_maintenance(cx, handle, &wake);
    });
}

#[gpui::test]
async fn adding_language_server_stream_wakes_event_maintenance(cx: &mut gpui::TestAppContext) {
    let app = new_test_application(cx);
    let (wake, mut wake_rx) = MaintenanceWake::channel();

    app.update(cx, |app, _cx| {
        app.set_maintenance_wake(wake);
        app.notify_language_server_stream_added();
    });

    assert!(wake_rx.try_recv().is_ok());
}

fn status_update_seen(
    updates: &Rc<RefCell<Vec<CapturedUpdate>>>,
    expected_message: &str,
    expected_severity: crate::types::Severity,
) -> bool {
    updates.borrow().iter().any(|update| {
        matches!(
            update,
            CapturedUpdate::StatusChanged(message, severity)
                if message == expected_message && *severity == expected_severity
        )
    })
}

#[test]
fn coalesce_helix_events_drops_duplicate_diagnostics() {
    let doc_id = helix_view::DocumentId::default();
    let server_id = helix_lsp::LanguageServerId::default();
    let events = vec![
        event_bridge::HelixEvent::DiagnosticsChanged { doc_id },
        event_bridge::HelixEvent::LanguageServerInitialized { server_id },
        event_bridge::HelixEvent::DiagnosticsChanged { doc_id },
    ];

    let coalesced = coalesce_helix_events(events);

    assert_eq!(coalesced.len(), 2);
    assert!(matches!(
        coalesced[0],
        event_bridge::HelixEvent::LanguageServerInitialized { .. }
    ));
    assert!(matches!(
        coalesced[1],
        event_bridge::HelixEvent::DiagnosticsChanged { .. }
    ));
}

#[gpui::test]
async fn apply_reloaded_config_updates_live_editor_config(cx: &mut gpui::TestAppContext) {
    let app = new_test_application(cx);

    app.update(cx, |app, cx| {
        let mut reloaded = app.config.clone();
        reloaded.helix.editor.scrolloff = 17;
        reloaded.helix.editor.idle_timeout = Duration::from_millis(750);

        app.apply_reloaded_config(reloaded, cx);

        assert_eq!(app.config.helix.editor.scrolloff, 17);
        assert_eq!(app.helix_config_arc.load().editor.scrolloff, 17);
        assert_eq!(app.editor.config().scrolloff, 17);
        assert_eq!(app.editor.config().idle_timeout, Duration::from_millis(750));
        assert!(app.editor.config().true_color);
    });
}

#[gpui::test]
async fn maintenance_handles_lsp_initialized_through_application_event(
    cx: &mut gpui::TestAppContext,
) {
    let app = new_test_application(cx);
    let updates = subscribe_application_updates(cx, &app);
    let (tx, rx) = mpsc::unbounded_channel();
    let server_id = helix_lsp::LanguageServerId::default();

    app.update(cx, |app, _cx| {
        app.helix_event_rx = Some(rx);
    });

    tx.send(event_bridge::HelixEvent::LanguageServerInitialized { server_id })
        .expect("language server initialized event");

    run_event_driven_maintenance(cx, &app);

    let lsp_state = app.read_with(cx, |app, _cx| {
        app.lsp_state.clone().expect("test lsp state")
    });
    lsp_state.read_with(cx, |state, _cx| {
        let server = state.servers.get(&server_id).expect("registered server");
        assert_eq!(server.status, nucleotide_lsp::ServerStatus::Running);
    });
    assert!(
        updates
            .borrow()
            .contains(&CapturedUpdate::LspInitialized(server_id))
    );
}

#[gpui::test]
async fn lsp_progress_entries_preserve_active_server_output_and_omit_idle_servers(
    cx: &mut gpui::TestAppContext,
) {
    let app = new_test_application(cx);
    let rust_analyzer_id: helix_lsp::LanguageServerId = KeyData::from_ffi(51).into();
    let taplo_id: helix_lsp::LanguageServerId = KeyData::from_ffi(52).into();

    app.update(cx, |app, _cx| {
        app.lsp_progress.begin(
            rust_analyzer_id,
            lsp::NumberOrString::String("rustAnalyzer/cachePriming".to_string()),
            lsp::WorkDoneProgressBegin {
                title: "Indexing".to_string(),
                cancellable: Some(true),
                message: None,
                percentage: Some(0),
            },
        );
        app.lsp_progress.update(
            rust_analyzer_id,
            lsp::NumberOrString::String("rustAnalyzer/cachePriming".to_string()),
            lsp::WorkDoneProgressReport {
                cancellable: Some(true),
                message: Some("723/724 (nucl)".to_string()),
                percentage: Some(99),
            },
        );

        let entries = app.compute_progress_entries(
            &[
                (rust_analyzer_id, "rust-analyzer".to_string()),
                (taplo_id, "taplo".to_string()),
            ],
            &[rust_analyzer_id],
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1.server_id, rust_analyzer_id);
        let message = entries[0].1.message.as_deref().unwrap();
        assert!(message.contains("Indexing"));
        assert!(message.contains("723/724 (nucl)"));
        assert!(message.contains("99%"));
    });
}

#[gpui::test]
async fn lsp_progress_report_without_begin_is_displayed(cx: &mut gpui::TestAppContext) {
    let app = new_test_application(cx);
    let rust_analyzer_id: helix_lsp::LanguageServerId = KeyData::from_ffi(53).into();
    let token = lsp::NumberOrString::String("rustAnalyzer/cachePriming".to_string());

    app.update(cx, |app, _cx| {
        app.lsp_progress.create(rust_analyzer_id, token.clone());
        app.lsp_progress.update(
            rust_analyzer_id,
            token,
            lsp::WorkDoneProgressReport {
                cancellable: Some(true),
                message: Some("723/724 (nucl)".to_string()),
                percentage: Some(99),
            },
        );

        let entries = app.compute_progress_entries(
            &[(rust_analyzer_id, "rust-analyzer".to_string())],
            &[rust_analyzer_id],
        );

        assert_eq!(entries.len(), 1);
        let message = entries[0].1.message.as_deref().unwrap();
        assert!(message.contains("cachePriming"));
        assert!(message.contains("723/724 (nucl)"));
        assert!(message.contains("99%"));
    });
}

#[gpui::test]
async fn maintenance_drains_job_status_to_editor_and_notifications(cx: &mut gpui::TestAppContext) {
    let app = new_test_application(cx);
    let updates = subscribe_application_updates(cx, &app);
    let (tx, rx) = mpsc::channel(4);

    app.update(cx, |app, _cx| {
        app.jobs.status_messages = rx;
    });

    tx.send(helix_event::status::StatusMessage {
        severity: helix_event::status::Severity::Warning,
        message: "indexing workspace".into(),
    })
    .await
    .expect("status message");

    run_event_driven_maintenance(cx, &app);

    assert!(updates.borrow().contains(&CapturedUpdate::StatusChanged(
        "indexing workspace".to_string(),
        crate::types::Severity::Warning,
    )));
    app.read_with(cx, |app, _cx| {
        let (message, severity) = app.editor.get_status().expect("editor status");
        assert_eq!(message.as_ref(), "indexing workspace");
        assert_eq!(*severity, helix_view::editor::Severity::Warning);
    });
}

#[gpui::test]
async fn maintenance_schedules_followup_returned_by_wait_job(cx: &mut gpui::TestAppContext) {
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        let followup = Job::with_callback(async {
            Ok(Callback::Editor(Box::new(|editor| {
                editor.set_error("follow-up completed");
            })))
        })
        .wait_before_exiting();
        let initial = Job::with_callback(async {
            Ok(Callback::Followup(Box::new(move |_editor| Some(followup))))
        })
        .wait_before_exiting();
        app.jobs.add(initial);
    });

    run_event_driven_maintenance(cx, &app);

    app.read_with(cx, |app, _cx| {
        let (message, severity) = app.editor.get_status().expect("editor status");
        assert_eq!(message.as_ref(), "follow-up completed");
        assert_eq!(*severity, helix_view::editor::Severity::Error);
    });
}

#[gpui::test]
async fn editor_job_callback_panic_updates_status(cx: &mut gpui::TestAppContext) {
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.handle_job_callback(helix_term::job::Callback::Editor(Box::new(|_editor| {
            panic!("inlay hint callback")
        })));

        let (message, severity) = app.editor.get_status().expect("editor status");
        assert_eq!(*severity, helix_view::editor::Severity::Error);
        assert_eq!(
            message.as_ref(),
            "Skipped editor callback: inlay hint callback"
        );
    });
}

#[test]
fn home_bootstrap_detection_flags_missing_or_placeholder_home() {
    assert!(home_requires_login_shell_capture(None));
    assert!(home_requires_login_shell_capture(Some("")));
    assert!(home_requires_login_shell_capture(Some("/homeless-shelter")));
    assert!(home_requires_login_shell_capture(Some(
        "/homeless-shelter/.cargo"
    )));
    assert!(!home_requires_login_shell_capture(Some("/Users/test")));
}

#[test]
fn syntax_symbol_kind_matches_helix_definition_captures() {
    assert_eq!(
        syntax_symbol_kind_from_capture_name("definition.function"),
        Some("function")
    );
    assert_eq!(
        syntax_symbol_kind_from_capture_name("definition.struct"),
        Some("struct")
    );
    assert_eq!(
        syntax_symbol_kind_from_capture_name("definition.constant"),
        Some("constant")
    );
    assert_eq!(
        syntax_symbol_kind_from_capture_name("reference.function"),
        None
    );
    assert_eq!(
        syntax_symbol_kind_from_capture_name("definition.method"),
        None
    );
}

#[test]
fn workspace_syntax_symbol_fallback_is_local_only() {
    assert!(should_use_workspace_syntax_symbol_fallback(
        &WorkspaceIdentity::Local
    ));
    assert!(!should_use_workspace_syntax_symbol_fallback(
        &WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Ssh,
            name: "devbox".to_string(),
        })
    ));
}

#[test]
fn startup_path_should_open_local_files_only() {
    let temp_dir = tempdir().unwrap();
    let file_path = temp_dir.path().join("main.rs");
    std::fs::write(&file_path, "fn main() {}\n").unwrap();

    assert!(startup_path_should_open_as_file(&file_path));
    assert!(!startup_path_should_open_as_file(temp_dir.path()));
}

#[test]
fn startup_path_should_open_remote_file_hints_only() {
    assert!(startup_path_should_open_as_file(&PathBuf::from(
        "ssh://me@example.com/home/me/project/src/main.rs"
    )));
    assert!(!startup_path_should_open_as_file(&PathBuf::from(
        "ssh://me@example.com/home/me/project/"
    )));
    assert!(!startup_path_should_open_as_file(&PathBuf::from(
        r"\\wsl.localhost\Ubuntu-24.04\home\me\project\"
    )));
}

#[test]
fn picker_root_stat_uses_backend_for_remote_workspaces() {
    assert!(!should_stat_picker_root_with_backend(
        &WorkspaceIdentity::Local
    ));
    assert!(should_stat_picker_root_with_backend(
        &WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Ssh,
            name: "devbox".to_string(),
        })
    ));
}

#[test]
fn remote_lsp_project_root_uses_remote_project_directory() {
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Ssh,
        name: "devbox".to_string(),
    });
    let project_directory = Path::new("ssh://devbox/home/me/project");
    let document_path = Path::new("ssh://devbox/home/me/project/src/main.rs");

    let root = remote_lsp_project_root_for_document(
        &identity,
        Some(project_directory),
        Some(document_path),
    );

    assert_eq!(root.as_deref(), Some(project_directory));
}

#[test]
fn remote_lsp_project_root_falls_back_to_remote_document_parent() {
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Ssh,
        name: "devbox".to_string(),
    });
    let document_path = Path::new("ssh://devbox/home/me/project/src/main.rs");

    let root = remote_lsp_project_root_for_document(&identity, None, Some(document_path));

    assert_eq!(
        root.as_deref(),
        Some(Path::new("ssh://devbox/home/me/project/src"))
    );
}

#[test]
fn remote_lsp_project_root_is_remote_only() {
    let project_directory = Path::new("ssh://devbox/home/me/project");
    let document_path = Path::new("ssh://devbox/home/me/project/src/main.rs");

    let root = remote_lsp_project_root_for_document(
        &WorkspaceIdentity::Local,
        Some(project_directory),
        Some(document_path),
    );

    assert_eq!(root, None);
}

#[test]
fn remote_lsp_root_uri_matches_native_remote_workspace() {
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Ssh,
        name: "devbox".to_string(),
    });
    let project_directory = Path::new("ssh://devbox/home/me/project");
    let document_path = Path::new("ssh://devbox/home/me/project/src/main.rs");
    let native_uri = lsp::Url::parse("file:///home/me/project").unwrap();

    assert!(remote_lsp_root_uri_matches_document_workspace(
        &identity,
        Some(project_directory),
        Some(document_path),
        Path::new("/home/me/project"),
        Some(&native_uri),
    ));
}

#[test]
fn remote_lsp_root_uri_rejects_missing_or_display_root_uri() {
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Ssh,
        name: "devbox".to_string(),
    });
    let project_directory = Path::new("ssh://devbox/home/me/project");
    let document_path = Path::new("ssh://devbox/home/me/project/src/main.rs");
    let native_uri = lsp::Url::parse("file:///home/me/project").unwrap();
    let display_uri = lsp::Url::parse("file:///ssh%3A/devbox/home/me/project").unwrap();

    assert!(!remote_lsp_root_uri_matches_document_workspace(
        &identity,
        Some(project_directory),
        Some(document_path),
        Path::new("/home/me/project"),
        None,
    ));
    assert!(!remote_lsp_root_uri_matches_document_workspace(
        &identity,
        Some(project_directory),
        Some(document_path),
        Path::new("/home/me/project"),
        Some(&display_uri),
    ));
    assert!(!remote_lsp_root_uri_matches_document_workspace(
        &identity,
        Some(project_directory),
        Some(document_path),
        Path::new("ssh://devbox/home/me/project"),
        Some(&native_uri),
    ));
}

#[test]
fn remote_lsp_root_uri_matches_encoded_native_remote_workspace() {
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Ssh,
        name: "devbox".to_string(),
    });
    let project_directory = Path::new("ssh://devbox/home/me/Project%20One");
    let document_path = Path::new("ssh://devbox/home/me/Project%20One/src/main.rs");
    let native_uri = lsp::Url::parse("file:///home/me/Project%20One").unwrap();

    assert!(remote_lsp_root_uri_matches_document_workspace(
        &identity,
        Some(project_directory),
        Some(document_path),
        Path::new("/home/me/Project One"),
        Some(&native_uri),
    ));
}

#[test]
fn remote_native_file_uri_uses_posix_file_urls_for_remote_paths() {
    let uri = remote_native_file_uri(Path::new(r"\home\me\Project One\src\main.rs")).unwrap();

    assert_eq!(uri.as_str(), "file:///home/me/Project%20One/src/main.rs");
}

#[test]
fn lsp_diagnostic_uri_maps_remote_file_url_to_display_document_uri() {
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Ssh,
        name: "example.com".to_string(),
    });
    let project_directory = Path::new("ssh://me@example.com/home/me/Project One");
    let diagnostic_uri = lsp::Url::parse("file:///home/me/Project%20One/src/cursor.rs").unwrap();

    let document_uri =
        lsp_diagnostic_document_uri(diagnostic_uri, Some(project_directory), &identity).unwrap();

    assert_eq!(
        document_uri.as_path(),
        Some(Path::new(
            "ssh://me@example.com/home/me/Project One/src/cursor.rs"
        ))
    );
}

#[test]
fn lsp_diagnostic_uri_keeps_local_file_urls_host_native() {
    let local_path = std::env::current_dir()
        .unwrap()
        .join("crates")
        .join("nucleotide")
        .join("Cargo.toml");
    let diagnostic_uri = lsp::Url::from_file_path(&local_path).unwrap();

    let document_uri =
        lsp_diagnostic_document_uri(diagnostic_uri, None, &WorkspaceIdentity::Local).unwrap();

    assert_eq!(document_uri.as_path(), Some(local_path.as_path()));
}

#[test]
fn lsp_location_path_accepts_remote_posix_file_urls_on_windows() {
    let url = lsp::Url::parse("file:///home/me/.cargo/git/checkouts/helix/helix-view/src/view.rs")
        .unwrap();

    let path = lsp_location_path_from_url(url).unwrap();

    assert_eq!(
        path,
        PathBuf::from("/home/me/.cargo/git/checkouts/helix/helix-view/src/view.rs")
    );
}

#[test]
fn lsp_location_from_location_accepts_remote_posix_file_urls() {
    let range = lsp::Range::new(lsp::Position::new(7, 3), lsp::Position::new(7, 9));
    let location = lsp::Location::new(
        lsp::Url::parse("file:///home/me/project/src/lib.rs").unwrap(),
        range,
    );

    let location = lsp_location_from_location(location, OffsetEncoding::Utf8).unwrap();

    assert_eq!(location.path, PathBuf::from("/home/me/project/src/lib.rs"));
    assert_eq!(location.range, range);
    assert_eq!(location.offset_encoding, OffsetEncoding::Utf8);
}

#[test]
fn lsp_location_path_keeps_local_file_urls_host_native() {
    let local_path = std::env::current_dir()
        .unwrap()
        .join("crates")
        .join("nucleotide")
        .join("Cargo.toml");
    let url = lsp::Url::from_file_path(&local_path).unwrap();

    let path = lsp_location_path_from_url(url).unwrap();

    assert_eq!(path, local_path);
}

#[test]
fn remote_lsp_root_uri_check_leaves_local_workspaces_unchanged() {
    assert!(remote_lsp_root_uri_matches_document_workspace(
        &WorkspaceIdentity::Local,
        Some(Path::new("/Users/me/project")),
        Some(Path::new("/Users/me/project/src/main.rs")),
        Path::new("/tmp/stale"),
        None,
    ));
}

#[test]
fn file_picker_current_directory_uses_project_directory_for_remote_workspaces() {
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Ssh,
        name: "devbox".to_string(),
    });
    let project_directory = Path::new("ssh://devbox/home/me/project");
    let host_cwd = PathBuf::from("/definitely/not/a/remote/project");

    let directory = file_picker_current_directory(&identity, Some(project_directory), host_cwd);

    assert_eq!(directory.as_deref(), Some(project_directory));
}

#[test]
fn lsp_symbol_item_preserves_display_metadata_and_target_payload() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let location = crate::types::LspLocation {
        path: path.clone(),
        range: lsp::Range::new(lsp::Position::new(3, 5), lsp::Position::new(3, 9)),
        offset_encoding: OffsetEncoding::Utf8,
    };

    let item = native_symbol_item_from_lsp(
        "render".to_string(),
        "function",
        Some("Widget".to_string()),
        location,
    );

    assert_eq!(item.name, "render");
    assert_eq!(item.kind, "function");
    assert_eq!(item.container_name.as_deref(), Some("Widget"));
    assert_eq!(item.path.as_deref(), Some(path.as_path()));
    assert_eq!(item.line, 4);
    match item.target {
        NativeSymbolTarget::Lsp(location) => {
            assert_eq!(location.path, path);
            assert_eq!(location.range.start.line, 3);
            assert_eq!(location.offset_encoding, OffsetEncoding::Utf8);
        }
        NativeSymbolTarget::Jump(_) => panic!("expected LSP-backed symbol target"),
        NativeSymbolTarget::SyntaxFile(_) => panic!("expected LSP-backed symbol target"),
    }
}

#[test]
fn symbol_picker_preserves_syntax_file_target_payload() {
    let path = PathBuf::from("/workspace/src/lib.rs");
    let symbol = NativeSymbolItem {
        name: "render".to_string(),
        kind: "function",
        container_name: None,
        path: Some(path.clone()),
        line: 12,
        target: NativeSymbolTarget::SyntaxFile(crate::types::SyntaxFileLocation {
            path: path.clone(),
            start: 120,
            end: 126,
        }),
    };

    let picker = lsp_symbol_picker(
        "Workspace Symbols",
        vec![symbol],
        Some(Path::new("/workspace")),
    );
    match picker {
        crate::picker::Picker::Native { title, items, .. } => {
            assert_eq!(title.as_ref(), "Workspace Symbols");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].label.as_ref(), "function render");
            assert_eq!(
                items[0].sublabel.as_ref().map(|label| label.as_ref()),
                Some("src/lib.rs:12")
            );
            let location = items[0]
                .data
                .downcast_ref::<crate::types::SyntaxFileLocation>()
                .expect("syntax file symbol item should carry a syntax file location");
            assert_eq!(location.path, path);
            assert_eq!(location.start, 120);
            assert_eq!(location.end, 126);
        }
    }
}

#[test]
fn project_lsp_plan_starts_manifest_languages_without_open_documents() {
    use nucleotide_events::ProjectLanguageEvidence;

    let plan =
        project_lsp_plan_from_names(&["Cargo.toml".to_string(), "rust-toolchain.toml".to_string()]);

    assert!(matches!(
        plan.project_type,
        nucleotide_events::ProjectType::Rust
    ));
    assert_eq!(plan.languages.len(), 2);
    assert_eq!(plan.languages[0].language_id, "rust");
    assert_eq!(
        plan.languages[0].evidence,
        ProjectLanguageEvidence::EagerProject
    );
    assert_eq!(plan.languages[1].language_id, "toml");
    assert_eq!(
        plan.languages[1].evidence,
        ProjectLanguageEvidence::EagerManifest
    );
}

#[test]
fn project_lsp_plan_starts_each_language_in_a_mixed_project() {
    let plan =
        project_lsp_plan_from_names(&["Cargo.toml".to_string(), "pyproject.toml".to_string()]);

    assert_eq!(
        plan.project_type,
        nucleotide_events::ProjectType::Mixed(vec![
            nucleotide_events::ProjectType::Rust,
            nucleotide_events::ProjectType::Python,
        ])
    );
    assert_eq!(
        plan.languages
            .iter()
            .map(|language| language.language_id.as_str())
            .collect::<Vec<_>>(),
        vec!["rust", "python", "toml"]
    );
}

#[gpui::test]
async fn project_lsp_plan_resolves_servers_from_effective_language_configuration(
    cx: &mut gpui::TestAppContext,
) {
    let configuration: syntax::config::Configuration = toml::from_str(
        r#"
[language-server.rust-analyzer]
command = "rust-analyzer"

[language-server.taplo]
command = "taplo"
args = ["lsp", "stdio"]

[[language]]
name = "rust"
scope = "source.rust"
file-types = ["rs"]
roots = ["Cargo.toml"]
language-servers = ["rust-analyzer"]

[[language]]
name = "toml"
scope = "source.toml"
file-types = ["toml"]
language-servers = ["taplo"]
"#,
    )
    .unwrap();
    let loader = syntax::Loader::new(configuration).unwrap();
    let plan = project_lsp_plan_from_names(&["Cargo.toml".to_string()]);
    let app = new_test_application(cx);

    app.update(cx, |app, _cx| {
        app.editor.syn_loader.store(Arc::new(loader));
        assert_eq!(
            configured_project_servers(&app.editor.syn_loader.load(), &plan),
            vec![
                ("rust".to_string(), "rust-analyzer".to_string()),
                ("toml".to_string(), "taplo".to_string()),
            ]
        );
    });
}

#[test]
fn project_lsp_probe_covers_only_planned_commands_not_already_known() {
    let servers = vec![
        ("rust".to_string(), "rust-analyzer".to_string()),
        ("toml".to_string(), "taplo".to_string()),
        ("toml".to_string(), "tombi".to_string()),
        ("python".to_string(), "unconfigured".to_string()),
    ];
    let commands = HashMap::from([
        ("rust-analyzer".to_string(), "rust-analyzer".to_string()),
        ("taplo".to_string(), "taplo".to_string()),
        ("tombi".to_string(), "tombi-cli".to_string()),
        ("marksman".to_string(), "marksman".to_string()),
    ]);

    assert_eq!(
        commands_to_probe_for_servers(&servers, &commands, None),
        vec![
            "rust-analyzer".to_string(),
            "taplo".to_string(),
            "tombi-cli".to_string()
        ]
    );

    let known = HashSet::from(["rust-analyzer".to_string()]);
    assert_eq!(
        commands_to_probe_for_servers(&servers, &commands, Some(&known)),
        vec!["taplo".to_string(), "tombi-cli".to_string()]
    );
}

#[test]
fn project_lsp_candidates_are_filtered_by_configured_command_availability() {
    let mut servers = vec![
        ("toml".to_string(), "taplo".to_string()),
        ("toml".to_string(), "tombi".to_string()),
        ("markdown".to_string(), "marksman".to_string()),
    ];
    let commands = HashMap::from([
        ("taplo".to_string(), "taplo".to_string()),
        ("tombi".to_string(), "tombi-cli".to_string()),
        ("marksman".to_string(), "marksman".to_string()),
    ]);
    let available = HashSet::from(["tombi-cli".to_string(), "marksman".to_string()]);

    retain_servers_with_available_commands(&mut servers, &commands, Some(&available));

    assert_eq!(
        servers,
        vec![
            ("toml".to_string(), "tombi".to_string()),
            ("markdown".to_string(), "marksman".to_string()),
        ]
    );
}

#[cfg(unix)]
#[test]
fn language_server_command_probe_uses_project_path_and_reports_available_commands() {
    let project = tempdir().unwrap();

    let available = TEST_RUNTIME
        .block_on(probe_available_language_server_commands(
            project.path(),
            local_workspace_backend(),
            vec![
                "sh".to_string(),
                "nucleotide-language-server-that-does-not-exist".to_string(),
            ],
            None,
            Duration::from_secs(5),
        ))
        .unwrap();

    assert!(available.contains("sh"));
    assert!(!available.contains("nucleotide-language-server-that-does-not-exist"));
}

#[test]
fn language_server_command_probe_rejects_captured_environment_without_path() {
    let project = tempdir().unwrap();
    let environment = HashMap::new();

    let error = TEST_RUNTIME
        .block_on(probe_available_language_server_commands(
            project.path(),
            local_workspace_backend(),
            vec!["rust-analyzer".to_string()],
            Some(&environment),
            Duration::from_secs(5),
        ))
        .unwrap_err();

    assert!(error.contains("no PATH"));
}

#[test]
fn project_lsp_inventory_discovers_nested_languages_before_document_open() {
    let project = tempdir().unwrap();
    fs::create_dir_all(project.path().join("services/api")).unwrap();
    fs::write(
        project.path().join("services/api/app.py"),
        "print('ready')\n",
    )
    .unwrap();
    fs::write(project.path().join("README.md"), "# Project\n").unwrap();
    fs::write(project.path().join("src.rs"), "fn main() {}\n").unwrap();
    let existing = HashSet::from(["rust".to_string()]);

    let languages = TEST_RUNTIME.block_on(discover_project_languages_with_backend(
        project.path(),
        local_workspace_backend(),
        &existing,
    ));

    assert!(languages.contains(&"python".to_string()));
    assert!(languages.contains(&"markdown".to_string()));
    assert!(!languages.contains(&"rust".to_string()));
}

#[test]
fn project_lsp_plan_uses_workspace_backend_listing() {
    let project = tempdir().unwrap();
    fs::write(project.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(project.path().join("pyproject.toml"), "[project]\n").unwrap();

    let plan = TEST_RUNTIME.block_on(detect_project_lsp_plan_with_backend(
        project.path(),
        local_workspace_backend(),
    ));

    assert_eq!(
        plan.project_type,
        nucleotide_events::ProjectType::Mixed(vec![
            nucleotide_events::ProjectType::Rust,
            nucleotide_events::ProjectType::Python,
        ])
    );
}

#[test]
fn project_marker_names_ignore_directory_names() {
    let root = PathBuf::from("/remote/project");
    let listing = DirectoryListing {
        path: root.clone(),
        entries: vec![
            directory_entry("package.json", root.join("package.json"), FileKind::File),
            directory_entry(
                "tsconfig.json",
                root.join("tsconfig.json"),
                FileKind::Directory,
            ),
            directory_entry("Cargo.toml", root.join("Cargo.toml"), FileKind::File),
        ],
    };

    let names = project_marker_names(listing);
    let plan = project_lsp_plan_from_names(&names);

    assert_eq!(
        plan.project_type,
        nucleotide_events::ProjectType::Mixed(vec![
            nucleotide_events::ProjectType::Rust,
            nucleotide_events::ProjectType::JavaScript,
        ])
    );
}

#[test]
fn lsp_completion_insert_text_prefers_text_edit() {
    let item = lsp::CompletionItem {
        label: "label".to_string(),
        insert_text: Some("insert".to_string()),
        text_edit: Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit {
            range: lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(0, 5)),
            new_text: "edited".to_string(),
        })),
        ..Default::default()
    };

    assert_eq!(lsp_completion_insert_text(&item), "edited");
}

#[test]
fn lsp_completion_insert_text_handles_insert_replace_edits() {
    let item = lsp::CompletionItem {
        label: "label".to_string(),
        text_edit: Some(lsp::CompletionTextEdit::InsertAndReplace(
            lsp::InsertReplaceEdit {
                new_text: "replacement".to_string(),
                insert: lsp::Range::new(lsp::Position::new(0, 1), lsp::Position::new(0, 3)),
                replace: lsp::Range::new(lsp::Position::new(0, 1), lsp::Position::new(0, 5)),
            },
        )),
        ..Default::default()
    };

    assert_eq!(lsp_completion_insert_text(&item), "replacement");
}

#[test]
fn lsp_completion_insert_text_format_preserves_snippets() {
    let explicit_snippet = lsp::CompletionItem {
        label: "snippet".to_string(),
        insert_text_format: Some(lsp::InsertTextFormat::SNIPPET),
        ..Default::default()
    };
    let snippet_kind = lsp::CompletionItem {
        label: "snippet-kind".to_string(),
        kind: Some(lsp::CompletionItemKind::SNIPPET),
        ..Default::default()
    };
    let plain = lsp::CompletionItem {
        label: "plain".to_string(),
        ..Default::default()
    };

    assert_eq!(
        lsp_completion_insert_text_format(&explicit_snippet),
        nucleotide_events::completion::InsertTextFormat::Snippet
    );
    assert_eq!(
        lsp_completion_insert_text_format(&snippet_kind),
        nucleotide_events::completion::InsertTextFormat::Snippet
    );
    assert_eq!(
        lsp_completion_insert_text_format(&plain),
        nucleotide_events::completion::InsertTextFormat::PlainText
    );
}

#[test]
fn completion_context_keeps_manual_invocation_after_trigger_text() {
    let trigger_characters = vec![":".to_string()];

    let context = completion_context_for_trigger(
        LspCompletionTrigger::Manual,
        "HttpBinClient::",
        Some(&trigger_characters),
    );

    assert_eq!(context.trigger_kind, lsp::CompletionTriggerKind::INVOKED);
    assert_eq!(context.trigger_character, None);
}

#[test]
fn completion_context_uses_advertised_trigger_strings() {
    let trigger_characters = vec![":".to_string(), "::".to_string(), ".".to_string()];

    let context = completion_context_for_trigger(
        LspCompletionTrigger::Character(':'),
        "HttpBinClient::",
        Some(&trigger_characters),
    );

    assert_eq!(
        context.trigger_kind,
        lsp::CompletionTriggerKind::TRIGGER_CHARACTER
    );
    assert_eq!(context.trigger_character.as_deref(), Some("::"));
}

#[test]
fn completion_context_ignores_unadvertised_trigger_characters() {
    let trigger_characters = vec![".".to_string()];

    let context = completion_context_for_trigger(
        LspCompletionTrigger::Character(':'),
        "HttpBinClient::",
        Some(&trigger_characters),
    );

    assert_eq!(context.trigger_kind, lsp::CompletionTriggerKind::INVOKED);
    assert_eq!(context.trigger_character, None);
}

#[test]
fn completion_context_marks_incomplete_retrigger() {
    let context = completion_context_for_trigger(
        LspCompletionTrigger::Incomplete,
        "println",
        Some(&[":".to_string()]),
    );

    assert_eq!(
        context.trigger_kind,
        lsp::CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS
    );
    assert_eq!(context.trigger_character, None);
}

#[test]
fn lsp_completion_response_is_incomplete_tracks_completion_lists() {
    let array_response = lsp::CompletionResponse::Array(vec![]);
    let list_response = lsp::CompletionResponse::List(lsp::CompletionList {
        is_incomplete: true,
        items: vec![],
    });

    assert!(!lsp_completion_response_is_incomplete(&array_response));
    assert!(lsp_completion_response_is_incomplete(&list_response));
}

#[test]
fn lsp_completion_resolve_supported_uses_server_capability() {
    assert!(!lsp_completion_resolve_supported(None));
    assert!(!lsp_completion_resolve_supported(Some(
        &lsp::CompletionOptions::default()
    )));

    let options = lsp::CompletionOptions {
        resolve_provider: Some(true),
        ..Default::default()
    };

    assert!(lsp_completion_resolve_supported(Some(&options)));
}

#[test]
fn lsp_completion_items_from_response_converts_all_items() {
    let response = lsp::CompletionResponse::Array(vec![
        lsp::CompletionItem {
            label: "function".to_string(),
            kind: Some(lsp::CompletionItemKind::FUNCTION),
            insert_text: Some("function()".to_string()),
            ..Default::default()
        },
        lsp::CompletionItem {
            label: "snippet".to_string(),
            kind: Some(lsp::CompletionItemKind::SNIPPET),
            insert_text_format: Some(lsp::InsertTextFormat::SNIPPET),
            ..Default::default()
        },
    ]);

    let items = lsp_completion_items_from_response(response, OffsetEncoding::Utf16);

    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].kind,
        nucleotide_events::completion::CompletionItemKind::Function
    );
    assert_eq!(items[0].insert_text, "function()");
    assert_eq!(
        items[1].insert_text_format,
        nucleotide_events::completion::InsertTextFormat::Snippet
    );
}

#[test]
fn lsp_completion_items_from_response_preserves_edit_metadata() {
    let response = lsp::CompletionResponse::Array(vec![lsp::CompletionItem {
        label: "HashMap".to_string(),
        text_edit: Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit {
            range: lsp::Range::new(lsp::Position::new(2, 4), lsp::Position::new(2, 7)),
            new_text: "HashMap".to_string(),
        })),
        additional_text_edits: Some(vec![lsp::TextEdit {
            range: lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(0, 0)),
            new_text: "use std::collections::HashMap;\n".to_string(),
        }]),
        ..Default::default()
    }]);

    let items = lsp_completion_items_from_response(response, OffsetEncoding::Utf8);
    let edit = items[0].edit.as_ref().expect("completion edit metadata");

    assert_eq!(
        edit.offset_encoding,
        nucleotide_events::completion::CompletionOffsetEncoding::Utf8
    );
    assert_eq!(
        edit.text_edit.as_ref().unwrap().range.start,
        nucleotide_events::completion::CompletionPosition {
            line: 2,
            character: 4
        }
    );
    assert_eq!(edit.additional_text_edits.len(), 1);
    assert_eq!(
        edit.additional_text_edits[0].new_text,
        "use std::collections::HashMap;\n"
    );
}

#[test]
fn lsp_completion_items_from_response_preserves_ranking_metadata() {
    let response = lsp::CompletionResponse::Array(vec![lsp::CompletionItem {
        label: "fmt(...)".to_string(),
        sort_text: Some("0001".to_string()),
        filter_text: Some("fmt".to_string()),
        preselect: Some(true),
        commit_characters: Some(vec!["(".to_string(), ".".to_string()]),
        tags: Some(vec![lsp::CompletionItemTag::DEPRECATED]),
        data: Some(serde_json::json!({"provider": "rust-analyzer"})),
        ..Default::default()
    }]);

    let items = lsp_completion_items_from_response(response, OffsetEncoding::Utf8);
    let item = &items[0];

    assert_eq!(item.sort_text.as_deref(), Some("0001"));
    assert_eq!(item.filter_text.as_deref(), Some("fmt"));
    assert!(item.preselect);
    assert_eq!(item.commit_characters, vec!["(", "."]);
    assert_eq!(
        item.tags,
        vec![nucleotide_events::completion::CompletionItemTag::Deprecated]
    );
    assert_eq!(
        item.data,
        Some(serde_json::json!({"provider": "rust-analyzer"}))
    );
    assert_eq!(item.source_index, 0);
    let raw_lsp_item = item.raw_lsp_item.as_ref().expect("raw LSP item");
    assert_eq!(raw_lsp_item["label"], "fmt(...)");
    assert_eq!(
        raw_lsp_item["data"],
        serde_json::json!({"provider": "rust-analyzer"})
    );
}

#[test]
fn lsp_completion_items_from_response_preserves_server_id() {
    let server_id: helix_lsp::LanguageServerId = KeyData::from_ffi(42).into();
    let response = lsp::CompletionResponse::Array(vec![lsp::CompletionItem {
        label: "clone".to_string(),
        ..Default::default()
    }]);

    let items = lsp_completion_items_from_response_for_server(
        response,
        OffsetEncoding::Utf8,
        Some(server_id),
    );

    assert_eq!(items[0].server_id, Some(server_id.data().as_ffi()));
}

#[test]
fn lsp_completion_items_from_response_applies_list_defaults() {
    let response: lsp::CompletionResponse = serde_json::from_value(serde_json::json!({
        "isIncomplete": false,
        "itemDefaults": {
            "editRange": {
                "start": { "line": 4, "character": 12 },
                "end": { "line": 4, "character": 15 }
            },
            "insertTextFormat": 2
        },
        "items": [{
            "label": "fmt(...)",
            "textEditText": "fmt(${1:f})$0"
        }]
    }))
    .expect("completion response with item defaults");

    let items = lsp_completion_items_from_response(response, OffsetEncoding::Utf16);
    let item = &items[0];
    let edit = item.edit.as_ref().expect("default edit metadata");
    let text_edit = edit.text_edit.as_ref().expect("default text edit");

    assert_eq!(item.insert_text, "fmt(${1:f})$0");
    assert_eq!(
        item.insert_text_format,
        nucleotide_events::completion::InsertTextFormat::Snippet
    );
    assert_eq!(
        text_edit.range,
        nucleotide_events::completion::CompletionRange {
            start: nucleotide_events::completion::CompletionPosition {
                line: 4,
                character: 12,
            },
            end: nucleotide_events::completion::CompletionPosition {
                line: 4,
                character: 15,
            },
        }
    );
    assert_eq!(text_edit.new_text, "fmt(${1:f})$0");
}

#[test]
fn buffer_word_completion_items_match_prefix_and_dedupe() {
    let items = buffer_word_completion_items("apple application app apple banana".chars(), "app");

    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["apple", "application"]);
    assert!(items.iter().all(|item| {
        item.kind == nucleotide_events::completion::CompletionItemKind::Text
            && item.detail.as_deref() == Some("buffer")
    }));
}

#[test]
fn buffer_word_completion_items_ignore_short_prefixes() {
    let items = buffer_word_completion_items("apple application".chars(), "a");

    assert!(items.is_empty());
}

#[test]
fn local_path_completion_context_resolves_relative_paths_from_document() {
    let context = local_path_completion_context(
        "src/ma",
        Some(Path::new("/workspace/project/lib.rs")),
        WorkspaceIdentity::Local,
    )
    .expect("relative path context");

    assert_eq!(context.dir_path, PathBuf::from("/workspace/project/src"));
    assert_eq!(context.typed_file_name.as_deref(), Some("ma"));
}

#[test]
fn local_path_completion_context_handles_trailing_slash() {
    let context = local_path_completion_context(
        "src/",
        Some(Path::new("/workspace/project/lib.rs")),
        WorkspaceIdentity::Local,
    )
    .expect("path context with trailing slash");

    assert_eq!(context.dir_path, PathBuf::from("/workspace/project/src"));
    assert_eq!(context.typed_file_name, None);
}

#[test]
fn local_path_completion_context_keeps_remote_uri_rooted() {
    let context = local_path_completion_context(
        "ssh://devbox/home/me/project/src/ma",
        Some(Path::new("ssh://devbox/home/me/project/lib.rs")),
        WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Ssh,
            name: "devbox".to_string(),
        }),
    )
    .expect("remote URI path context");

    assert_eq!(
        context.dir_path,
        PathBuf::from("ssh://devbox/home/me/project/src")
    );
    assert_eq!(context.typed_file_name.as_deref(), Some("ma"));
}

#[test]
fn local_path_completion_context_accepts_remote_posix_file_urls() {
    let context = local_path_completion_context(
        "file:///home/me/Project%20One/src/ma",
        Some(Path::new("ssh://devbox/home/me/Project One/lib.rs")),
        WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
            kind: RemoteWorkspaceKind::Ssh,
            name: "devbox".to_string(),
        }),
    )
    .expect("remote file URL path context");

    assert_eq!(context.dir_path, PathBuf::from("/home/me/Project One/src"));
    assert_eq!(context.typed_file_name.as_deref(), Some("ma"));
}

#[tokio::test]
async fn path_completion_items_filter_and_classify_entries() {
    let temp_dir = tempdir().expect("tempdir");
    fs::write(temp_dir.path().join("main.rs"), "").expect("main.rs");
    fs::write(temp_dir.path().join("mod.rs"), "").expect("mod.rs");
    fs::create_dir(temp_dir.path().join("module")).expect("module dir");
    fs::write(temp_dir.path().join("readme.md"), "").expect("readme.md");

    let items = path_completion_items(
        local_workspace_backend(),
        temp_dir.path().to_path_buf(),
        Some("m".to_string()),
    )
    .await;

    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["module", "main.rs", "mod.rs"]);
    assert_eq!(
        items[0].kind,
        nucleotide_events::completion::CompletionItemKind::Folder
    );
    assert_eq!(
        items[1].kind,
        nucleotide_events::completion::CompletionItemKind::File
    );
    assert!(items.iter().all(|item| item.documentation.is_some()));
}

#[test]
fn path_completion_items_from_remote_listing_skip_local_canonicalize() {
    let remote_src = PathBuf::from("/remote/project/src");
    let listing = DirectoryListing {
        path: remote_src.clone(),
        entries: vec![
            directory_entry("main.rs", remote_src.join("main.rs"), FileKind::File),
            directory_entry("module", remote_src.join("module"), FileKind::Directory),
            directory_entry("ignored.rs", remote_src.join("ignored.rs"), FileKind::File),
        ],
    };
    let identity = WorkspaceIdentity::Remote(RemoteWorkspaceIdentity {
        kind: RemoteWorkspaceKind::Wsl,
        name: "Ubuntu".to_string(),
    });

    let items = path_completion_items_from_listing(&listing, Some("m"), identity);

    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["module", "main.rs"]);
    assert_eq!(items[0].kind, CompletionItemKind::Folder);
    assert_eq!(items[1].kind, CompletionItemKind::File);
    assert_eq!(
        items[1].documentation.as_deref(),
        Some("type: `file`\nfull path: `/remote/project/src/main.rs`")
    );
}

#[test]
fn dedupe_completion_items_preserves_first_match() {
    let mut items = vec![
        nucleotide_events::completion::CompletionItem::new(
            "println".to_string(),
            nucleotide_events::completion::CompletionItemKind::Function,
        )
        .with_insert_text("println!".to_string()),
        nucleotide_events::completion::CompletionItem::new(
            "println".to_string(),
            nucleotide_events::completion::CompletionItemKind::Text,
        )
        .with_insert_text("println!".to_string()),
        nucleotide_events::completion::CompletionItem::new(
            "print".to_string(),
            nucleotide_events::completion::CompletionItemKind::Text,
        )
        .with_insert_text("print".to_string()),
    ];

    dedupe_completion_items(&mut items);

    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].kind,
        nucleotide_events::completion::CompletionItemKind::Function
    );
    assert_eq!(items[1].label, "print");
}

#[test]
fn suppress_shadowed_buffer_word_completion_items_hides_macro_duplicate() {
    let existing_items = vec![
        nucleotide_events::completion::CompletionItem::new(
            "println!".to_string(),
            nucleotide_events::completion::CompletionItemKind::Snippet,
        )
        .with_insert_text("println!(\"${1}\");$0".to_string()),
    ];
    let mut local_items = vec![
        nucleotide_events::completion::CompletionItem::new(
            "println".to_string(),
            nucleotide_events::completion::CompletionItemKind::Text,
        )
        .with_detail("buffer".to_string()),
        nucleotide_events::completion::CompletionItem::new(
            "printer".to_string(),
            nucleotide_events::completion::CompletionItemKind::Text,
        )
        .with_detail("buffer".to_string()),
    ];

    suppress_shadowed_buffer_word_completion_items(&existing_items, &mut local_items);

    let labels: Vec<_> = local_items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["printer"]);
}

#[tokio::test]
async fn pending_completion_request_collects_lsp_and_local_items() {
    let mut lsp_futures: FuturesOrdered<super::CompletionServerFuture> = FuturesOrdered::new();
    lsp_futures.push_back(
        async {
            Ok::<_, anyhow::Error>((
                helix_lsp::LanguageServerId::default(),
                OffsetEncoding::Utf16,
                Some(lsp::CompletionResponse::Array(vec![lsp::CompletionItem {
                    label: "println".to_string(),
                    kind: Some(lsp::CompletionItemKind::FUNCTION),
                    insert_text: Some("println!()".to_string()),
                    ..Default::default()
                }])),
            ))
        }
        .boxed(),
    );

    let request = PendingCompletionRequest {
        prefix: "pri".to_string(),
        retained_items: vec![],
        local_items: vec![CompletionItem::new(
            "private".to_string(),
            CompletionItemKind::Text,
        )],
        local_futures: FuturesOrdered::new(),
        lsp_error: None,
        lsp_futures,
    };

    let (items, prefix, is_incomplete, incomplete_server_ids) =
        request.collect().await.expect("completion results");

    assert_eq!(prefix, "pri");
    assert!(!is_incomplete);
    assert!(incomplete_server_ids.is_empty());
    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["println", "private"]);
}

#[tokio::test]
async fn pending_completion_request_suppresses_buffer_word_shadowed_by_lsp_macro() {
    let mut lsp_futures: FuturesOrdered<super::CompletionServerFuture> = FuturesOrdered::new();
    lsp_futures.push_back(
        async {
            Ok::<_, anyhow::Error>((
                helix_lsp::LanguageServerId::default(),
                OffsetEncoding::Utf16,
                Some(lsp::CompletionResponse::Array(vec![lsp::CompletionItem {
                    label: "println!".to_string(),
                    kind: Some(lsp::CompletionItemKind::FUNCTION),
                    insert_text: Some("println!(${1:value})$0".to_string()),
                    insert_text_format: Some(lsp::InsertTextFormat::SNIPPET),
                    ..Default::default()
                }])),
            ))
        }
        .boxed(),
    );

    let request = PendingCompletionRequest {
        prefix: "pri".to_string(),
        retained_items: vec![],
        local_items: vec![
            CompletionItem::new("println".to_string(), CompletionItemKind::Text)
                .with_detail("buffer".to_string()),
            CompletionItem::new("private".to_string(), CompletionItemKind::Text)
                .with_detail("buffer".to_string()),
        ],
        local_futures: FuturesOrdered::new(),
        lsp_error: None,
        lsp_futures,
    };

    let (items, _, _, _) = request.collect().await.expect("completion results");

    let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, vec!["println!", "private"]);
}

#[tokio::test]
async fn pending_completion_request_keeps_local_items_when_lsp_fails() {
    let request = PendingCompletionRequest {
        prefix: "src".to_string(),
        retained_items: vec![],
        local_items: vec![CompletionItem::new(
            "src/lib.rs".to_string(),
            CompletionItemKind::File,
        )],
        local_futures: FuturesOrdered::new(),
        lsp_error: Some(anyhow::anyhow!("no completion server")),
        lsp_futures: FuturesOrdered::new(),
    };

    let (items, prefix, is_incomplete, incomplete_server_ids) =
        request.collect().await.expect("local fallback");

    assert_eq!(prefix, "src");
    assert!(!is_incomplete);
    assert!(incomplete_server_ids.is_empty());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "src/lib.rs");
}

#[tokio::test]
async fn pending_completion_request_preserves_incomplete_lsp_lists() {
    let mut lsp_futures: FuturesOrdered<super::CompletionServerFuture> = FuturesOrdered::new();
    lsp_futures.push_back(
        async {
            Ok::<_, anyhow::Error>((
                helix_lsp::LanguageServerId::default(),
                OffsetEncoding::Utf16,
                Some(lsp::CompletionResponse::List(lsp::CompletionList {
                    is_incomplete: true,
                    items: vec![lsp::CompletionItem {
                        label: "println".to_string(),
                        kind: Some(lsp::CompletionItemKind::FUNCTION),
                        ..Default::default()
                    }],
                })),
            ))
        }
        .boxed(),
    );

    let request = PendingCompletionRequest {
        prefix: "pri".to_string(),
        retained_items: vec![],
        local_items: vec![],
        local_futures: FuturesOrdered::new(),
        lsp_error: None,
        lsp_futures,
    };

    let (items, prefix, is_incomplete, incomplete_server_ids) =
        request.collect().await.expect("completion results");

    assert_eq!(prefix, "pri");
    assert!(is_incomplete);
    assert_eq!(
        incomplete_server_ids,
        vec![helix_lsp::LanguageServerId::default().data().as_ffi()]
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "println");
}

// Note: Prefix extraction tests require complex setup with Editor and Document
// The core functionality is tested through integration tests when running the application

// NOTE: Unit tests for extract_completion_prefix require complex Editor/Document setup
// that is difficult to configure in unit tests due to missing test utilities like FakeClipboard.
// The functionality is thoroughly tested through integration tests and manual testing.

// NOTE: Test for extract_completion_prefix with non-existent document removed due to
// Application constructor requiring complex setup. This case is handled by returning
// an empty string when the document is not found, as seen in the implementation.
